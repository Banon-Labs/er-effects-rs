//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

#![allow(unused_imports)]

use std::{
    ffi::c_void,
    fmt::Write as _,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use std::os::windows::ffi::OsStrExt as _;

use debug::{InputBlocker, InputFlags};
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use hudhook::{
    ImguiRenderLoop, MessageFilter,
    hooks::dx12::ImguiDx12Hooks,
    imgui::{Condition, Context, Ui},
    mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook},
    windows::{
        Win32::{
            Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM},
            System::{
                LibraryLoader::{GetModuleHandleA, GetProcAddress},
                Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery},
                SystemServices::DLL_PROCESS_ATTACH,
                Threading::GetCurrentProcessId,
            },
            UI::WindowsAndMessaging::{
                ClipCursor, EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
                WM_KEYDOWN, WM_KEYUP,
            },
        },
        core::{BOOL, PCSTR},
    },
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, ffi::*, hooks::*, telemetry::*};

use super::*;

/// Render-thread liveness + bootstrap probe. Runs from the ImGui render loop (a
/// separate thread from the game-task scheduler), so it keeps reporting after the
/// title->menu phase transition stops the title CSTask. Distinguishes "the title
/// advanced (render alive + CSFeMan builds)" from "the game hung (render frozen)".
#[allow(dead_code)]
/// When set, ALL game input is hard-blocked at the API layer (see `enforce_input_block`):
/// DInput8 keyboard+mouse (state zeroed by the `debug::InputBlocker` hook) AND XInput
/// gamepad (this module's hook). Read by `xinput_get_state_hook` each poll so the block is
/// authoritative regardless of window focus.
pub(crate) static BLOCK_INPUT_ACTIVE: AtomicUsize = AtomicUsize::new(0);
const BLOCK_INPUT_ON: usize = 1;
/// Original `XInputGetState` (minhook trampoline). 0 until the hook installs.
pub(crate) static XINPUT_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);

/// STAY-ACTIVE gate (`ER_EFFECTS_STAY_ACTIVE=1` / `er-effects-stay-active.txt`). When set, keep ER's
/// input-accept flag `[DLUID+0x88d]` forced to 1 every tick so a virtual gamepad keeps driving the
/// menus while ER is UNFOCUSED -- letting the user work in another window during a golden capture.
/// Decoded: ER clears that flag each frame when it isn't `GetActiveWindow` (`0x141f292bd`); we re-set
/// it. Touches ONLY focus-input gating, never the sim/save/load.
pub(crate) fn stay_active_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_STAY_ACTIVE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-stay-active.txt")
            .exists()
}

/// True when the autoload/own-stepper probe must run UNCONTAMINATED -- no real keyboard,
/// mouse (move/click), or gamepad input may reach the game even if the user focuses the
/// window. Auto-on whenever the own-stepper drives the front-end (the whole point of that
/// probe is a zero-input load), plus an explicit env/file override for standalone use.
pub(crate) fn block_input_enabled() -> bool {
    // FORCE-BLOCK override (env/file): block UNCONDITIONALLY, even past menu-open. Used to
    // FALSIFY -- runtime-proven 2026-06-17 that blocking through menu-open lets the menu OPEN
    // (self-fire) but starves the post-open navigation, so the load never selects.
    if matches!(std::env::var("ER_EFFECTS_BLOCK_INPUT").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-block-input.txt")
            .exists()
    {
        return true;
    }
    // INJECT-NAV instrument-capture: keep the block ON past menu-open so the user's input is
    // suppressed while the XInput hook fabricates the cursor nav (so nothing pollutes the
    // capture). The fabricated Down is written INTO the otherwise-blocked gamepad state, so the
    // menu still gets a live (synthesized) input each frame -- it does not stall.
    if own_stepper_enabled() && !own_stepper_passive_enabled() && inject_nav_enabled() {
        return true;
    }
    // PASSIVE mode never blocks. Otherwise keep the block engaged through the ENTIRE headless
    // drive -- boot -> menu-open -> zero-input title-confirm Load fire -> mount -> confirm --
    // releasing ONLY once in-world (the user takes over) or on abort (phase DONE). Product
    // autoload keeps blocking after the guarded SetState5 until the in-world oracle fires, so the
    // world-stream interval cannot be contaminated by user input.
    let product_world_stream_pending = product_autoload_enabled()
        && OWN_STEPPER_CONFIRMED.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS
        && IN_WORLD_REACHED.load(Ordering::SeqCst) != IN_WORLD_REACHED_YES;
    // ZERO-INPUT INVARIANT (always-block-input-zero-input-invariant-2026-06-22): block ALL foreign
    // input whenever ANY automated load lever is armed (not just own_stepper) until in-world, so no
    // probe can be contaminated and no path can secretly rely on input. own_load covers its pump/
    // continue/install sub-levers (they all ride on own_load being armed). Normal play and user-driven
    // golden traces (no lever armed) never block; the in-world release lets the user play after load.
    (own_stepper_enabled() || own_load_enabled() || product_autoload_enabled())
        && !own_stepper_passive_enabled()
        && IN_WORLD_REACHED.load(Ordering::SeqCst) != IN_WORLD_REACHED_YES
        && (OWN_STEPPER_PHASE.load(Ordering::SeqCst) != OWN_STEPPER_PHASE_DONE
            || product_world_stream_pending)
}

/// Release the input block (DInput + XInput) once `block_input_enabled()` flips false mid-run.
/// The hooks stay installed but pass input through when `BLOCK_INPUT_ACTIVE` is clear; the
/// DInput blocker also needs its own flags cleared. Acts once on the ON->off transition.
pub(crate) fn release_input_block_now() {
    if BLOCK_INPUT_ACTIVE.swap(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst) == BLOCK_INPUT_ON {
        InputBlocker::get_instance().block_only(InputFlags::empty());
        // Release the cursor confinement (paired with the ClipCursor lockdown in enforce).
        let _ = unsafe { ClipCursor(None) };
        append_autoload_debug(format_args!(
            "input-block: RELEASED (in-world / abort) -- keyboard/mouse/gamepad + cursor live"
        ));
    }
}

/// XInput `XInputGetState(user_index, *mut XINPUT_STATE) -> DWORD` detour. Calls the real
/// function, then -- while the block is active -- zeroes the XINPUT_GAMEPAD sub-struct
/// (buttons + triggers + thumbsticks) so the game reads a connected-but-idle pad (no
/// "controller disconnected" popup, but zero input). Leaves the disconnected return code
/// untouched so a genuinely absent pad still reads absent.
pub(crate) unsafe extern "system" fn xinput_get_state_hook(user_index: u32, state: *mut u8) -> u32 {
    const XINPUT_SUCCESS: u32 = 0;
    const XINPUT_ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
    // XINPUT_STATE = { DWORD dwPacketNumber; XINPUT_GAMEPAD Gamepad; }; the gamepad sub-struct
    // (wButtons,bLeftTrigger,bRightTrigger,sThumbLX/LY/RX/RY) starts at +4 and is 12 bytes.
    const XINPUT_GAMEPAD_OFFSET: usize = 4;
    const XINPUT_GAMEPAD_SIZE: usize = 12;
    const ZERO_FILL_BYTE: u8 = 0;
    let orig = XINPUT_GET_STATE_ORIG.load(Ordering::SeqCst);
    let hr = if orig != TITLE_OWNER_SCAN_START_ADDRESS {
        let f: unsafe extern "system" fn(u32, *mut u8) -> u32 =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(user_index, state) }
    } else {
        XINPUT_ERROR_DEVICE_NOT_CONNECTED
    };
    const XINPUT_PACKET_OFFSET: usize = 0;
    const WBUTTONS_OFFSET_IN_GAMEPAD: usize = 0;
    if !state.is_null() && BLOCK_INPUT_ACTIVE.load(Ordering::SeqCst) == BLOCK_INPUT_ON {
        let inject = inject_nav_enabled()
            && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO;
        if inject {
            // Fabricate the gamepad state at the poll source from the schedule driven each frame
            // by own_stepper idx10 (this hook may never be polled if no controller, so the
            // schedule does NOT live here). Force SUCCESS + a fresh packet number so a live pad is
            // simulated; write the scheduled D-pad Down. Harmless if the game ignores XInput.
            let buttons = INJECT_NAV_CUR_BUTTONS.load(Ordering::SeqCst) as u16;
            let pkt = INJECT_NAV_FRAME.load(Ordering::SeqCst) as u32;
            unsafe {
                std::ptr::write_bytes(
                    state.add(XINPUT_GAMEPAD_OFFSET),
                    ZERO_FILL_BYTE,
                    XINPUT_GAMEPAD_SIZE,
                );
                *(state.add(XINPUT_PACKET_OFFSET) as *mut u32) = pkt;
                *(state.add(XINPUT_GAMEPAD_OFFSET + WBUTTONS_OFFSET_IN_GAMEPAD) as *mut u16) =
                    buttons;
            }
            let _ = user_index;
            return XINPUT_SUCCESS;
        }
        if hr == XINPUT_SUCCESS {
            unsafe {
                std::ptr::write_bytes(
                    state.add(XINPUT_GAMEPAD_OFFSET),
                    ZERO_FILL_BYTE,
                    XINPUT_GAMEPAD_SIZE,
                )
            };
        }
    }
    hr
}

/// Install the XInput gamepad block once. Hooks `XInputGetState` (and ordinal-100
/// `XInputGetStateEx`, used by Steam Input) in whichever xinput runtime DLL is loaded.
/// minhook-based, mirroring `create_continue_trace_hook`.
unsafe fn install_xinput_block() {
    const XINPUT_DLLS: [&[u8]; 5] = [
        b"xinput1_4.dll\0",
        b"xinput1_3.dll\0",
        b"xinput9_1_0.dll\0",
        b"xinput1_2.dll\0",
        b"xinput1_1.dll\0",
    ];
    const XINPUT_GET_STATE_EX_ORDINAL: usize = 100;
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "xinput-block: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let mut hooked_any = false;
    for name in XINPUT_DLLS {
        let hmod = match unsafe { GetModuleHandleA(PCSTR(name.as_ptr())) } {
            Ok(h) if !h.is_invalid() => h,
            _ => continue,
        };
        let proc = unsafe { GetProcAddress(hmod, PCSTR(b"XInputGetState\0".as_ptr())) };
        let Some(addr) = proc else { continue };
        let addr = addr as usize;
        match unsafe { MhHook::new(addr as *mut c_void, xinput_get_state_hook as *mut c_void) } {
            Ok(hook) => {
                XINPUT_GET_STATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                if let Err(status) = unsafe { hook.queue_enable() } {
                    append_autoload_debug(format_args!(
                        "xinput-block: queue_enable XInputGetState failed: {status:?}"
                    ));
                } else {
                    append_autoload_debug(format_args!(
                        "xinput-block: hooked XInputGetState at 0x{addr:x}"
                    ));
                    std::mem::forget(hook);
                    hooked_any = true;
                }
            }
            Err(status) => append_autoload_debug(format_args!(
                "xinput-block: MhHook::new XInputGetState failed: {status:?}"
            )),
        }
        // Steam Input routes the guide button through ordinal-100 XInputGetStateEx; neuter it
        // too so a focused pad cannot drive menus through that path. Same zeroing detour.
        let ex = unsafe { GetProcAddress(hmod, PCSTR(XINPUT_GET_STATE_EX_ORDINAL as *const u8)) };
        if let Some(ex_addr) = ex {
            let ex_addr = ex_addr as usize;
            if ex_addr != addr {
                if let Ok(hook) = unsafe {
                    MhHook::new(ex_addr as *mut c_void, xinput_get_state_hook as *mut c_void)
                } {
                    let _ = unsafe { hook.queue_enable() };
                    std::mem::forget(hook);
                    append_autoload_debug(format_args!(
                        "xinput-block: hooked XInputGetStateEx(ord 100) at 0x{ex_addr:x}"
                    ));
                }
            }
        }
        break;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {}
        status => append_autoload_debug(format_args!(
            "xinput-block: MH_ApplyQueued failed: {status:?}"
        )),
    }
    if !hooked_any {
        append_autoload_debug(format_args!(
            "xinput-block: no xinput DLL with XInputGetState found yet (will retry next frame)"
        ));
    }
}

/// Tracks whether the DInput keyboard+mouse `install_hooks` has run (once).
static DINPUT_BLOCK_INSTALLED: AtomicUsize = AtomicUsize::new(0);

/// Enforce the comprehensive input block for this frame. Self-contained (no args) so it can
/// run from EITHER the game task OR the render loop -- critical because under the offline
/// launcher the hudhook render loop does NOT execute at the title, so the render-loop call
/// alone never engaged the block (that was the contamination hole). Driven every frame from
/// the game task while `block_input_enabled()`:
///   1. ONCE: install the DInput8 keyboard+mouse `GetDeviceState` block (panics on probe
///      failure -> contained with catch_unwind so the FD4 task never unwinds into C++).
///   2. EVERY frame: assert the block-all flag (sticky, overriding any overlay want-capture
///      clear) and install/retry the XInput gamepad hook until the xinput DLL is present.
/// Genuinely zero-input: it only SUPPRESSES device reads -- it never synthesizes any input.
pub(crate) fn enforce_input_block_now() {
    let blocker = InputBlocker::get_instance();
    if DINPUT_BLOCK_INSTALLED.swap(BLOCK_INPUT_ON, Ordering::SeqCst)
        == TITLE_OWNER_SCAN_START_ADDRESS
    {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            blocker.install_hooks()
        }));
        match result {
            Ok(Ok(())) => {
                append_autoload_debug(format_args!(
                    "input-block: DInput keyboard+mouse GetDeviceState hook installed"
                ));
            }
            Ok(Err(status)) => append_autoload_debug(format_args!(
                "input-block: DInput install_hooks failed: {status:?} (XInput still hooks)"
            )),
            Err(_) => append_autoload_debug(format_args!(
                "input-block: DInput install_hooks panicked (contained; XInput still hooks)"
            )),
        }
    }
    BLOCK_INPUT_ACTIVE.store(BLOCK_INPUT_ON, Ordering::SeqCst);
    blocker.block_only(InputFlags::all());
    if XINPUT_GET_STATE_ORIG.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS {
        // Not yet hooked (xinput DLL may load late): retry each frame until it sticks.
        unsafe { install_xinput_block() };
    }
    // Lock down MOUSE MOVEMENT: the DInput GetDeviceState block zeroes keyboard + mouse buttons +
    // DInput mouse deltas, but ER moves the MENU cursor via the OS cursor position (GetCursorPos),
    // which DInput blocking does NOT cover -- so the user can still move the cursor. Confine the OS
    // cursor to a 1x1 rect every frame: it physically cannot move regardless of which API reads it,
    // making the run uncontaminatable by the mouse. Released (ClipCursor(None)) when the block lifts.
    const CLIP_ORIGIN: i32 = 0;
    const CLIP_EDGE: i32 = 1;
    let clip = RECT {
        left: CLIP_ORIGIN,
        top: CLIP_ORIGIN,
        right: CLIP_EDGE,
        bottom: CLIP_EDGE,
    };
    let _ = unsafe { ClipCursor(Some(&clip)) };
}

pub(crate) fn render_liveness_probe() {
    if !title_accept_enabled() {
        return;
    }
    let frame = RENDER_FRAME_COUNT.fetch_add(AV_LOG_LINE_INCREMENT, Ordering::SeqCst);
    if frame % RENDER_PROBE_INTERVAL != TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let Ok(base) = game_module_base() else {
        return;
    };
    let csfeman = unsafe { *((base + CSFEMAN_SINGLETON_RVA) as *const usize) };
    let latch = unsafe { *((base + TITLE_ACCEPT_LATCH_RVA) as *const u8) };
    append_autoload_debug(format_args!(
        "render_probe: frame={frame} csfeman=0x{csfeman:x} latch={latch}"
    ));
}
