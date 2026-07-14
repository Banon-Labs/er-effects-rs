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

use crate::input_blocker::{InputBlocker, InputFlags};
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use windows::{
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
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, ffi::*, hooks::*, telemetry::*};

use super::*;

/// Render-thread liveness + bootstrap probe. Runs from an optional render callback (a
/// separate thread from the game-task scheduler), so it keeps reporting after the
/// title->menu phase transition stops the title CSTask. Distinguishes "the title
/// advanced (render alive + CSFeMan builds)" from "the game hung (render frozen)".
#[allow(dead_code)]
/// When set, ALL game input is hard-blocked at the API layer (see `enforce_input_block`):
/// DInput8 keyboard+mouse (state zeroed by the `InputBlocker` hook) AND XInput
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
/// The System->Quit repro autopilot is ACTIVELY DRIVING MENUS (issuing button edges): every state
/// except the waits and DONE. During the between-switch reload (WAIT_RELOAD) the autopilot injects
/// nothing (set_pad 0) and must NOT fabricate a live pad or hold the block past in-world, because a
/// fabricated connected pad fed through the title->world advance bounces the reload back to the
/// front-end/title (observed: switch #1's SetState5 loaded the char then the game jumped to 01_000_FE
/// + SetState 2/3/10 = press-any-button softlock). Treating WAIT_RELOAD like DONE makes the reload
/// byte-identical to the proven single-switch case (block falls through to the autoload_armed
/// path, which blocks until in-world with no pad fabrication); the block re-engages at the next
/// switch's OPEN_MENU. WAIT_WORLD (boot) keeps blocking so the first switch behaves as before.
pub(crate) fn sq_repro_actively_driving() -> bool {
    if !system_quit_repro_enabled() {
        return false;
    }
    let state = SQ_REPRO_STATE.load(Ordering::SeqCst);
    state != SQ_REPRO_STATE_DONE && state != SQ_REPRO_STATE_WAIT_RELOAD
}

// ENV-GATE RATIONALE: ER_EFFECTS_BLOCK_INPUT is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn block_input_enabled() -> bool {
    // SYSTEM-QUIT REPRO AUTOPILOT: keep the block engaged in-world (past the normal in-world
    // release) while the self-driven repro is ACTIVELY driving menus, so the real
    // keyboard/mouse/gamepad are zeroed and the ONLY input is the fabricated XInput pad
    // (`xinput_get_state_hook` writes the autopilot's `SQ_REPRO_XINPUT_BUTTONS` each poll) -- no human
    // press can contaminate the reproduction. Releases at DONE and during the between-switch reload
    // (WAIT_RELOAD, see sq_repro_actively_driving) so the reload completes exactly like a single switch.
    if sq_repro_actively_driving() {
        return true;
    }
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
    // ZERO-INPUT INVARIANT (always-block-input-zero-input-invariant-2026-06-22, extended
    // 2026-06-24 user-directive "block input until the load has started -- our side is done"):
    // block ALL foreign input whenever ANY automated load lever is armed until in-world, so no probe
    // can be contaminated and no path can secretly rely on input. This now INCLUDES the DEFAULT
    // zero-input autoload path (native_continue + the readiness PAB advance), which is on for every
    // real (non-telemetry-only) run -- previously only own_stepper/own_load/product_autoload engaged
    // the block, so the default path ran with input LIVE and a human Continue press could (and did,
    // 2026-06-24 gold-load run) drive the load instead of our DLL, masking that native_continue never
    // found the Continue node. Blocking the default path makes the zero-input claim honest: if our
    // drive cannot fire the load with input suppressed, the run stalls (correct failure) rather than
    // riding on a foreign press. Normal play and user-driven golden traces (no lever armed, or
    // telemetry-only) never block; the in-world release lets the user take over after the load.
    //
    // TODO(load-start release): release at the LOAD-STARTED semaphore (NowLoading flag set / the
    // MoveMapStep load sequence begun) instead of full in-world, once the zero-input drive reliably
    // fires the load -- so "our side is done" releases the user the moment the engine commits, not
    // after the world finishes streaming.
    let autoload_armed = own_stepper_enabled()
        || own_load_enabled()
        || product_autoload_enabled()
        || native_continue_enabled()
        || pab_advance_enabled();
    autoload_armed
        && !own_stepper_passive_enabled()
        && IN_WORLD_REACHED.load(Ordering::SeqCst) != IN_WORLD_REACHED_YES
        && (OWN_STEPPER_PHASE.load(Ordering::SeqCst) != OWN_STEPPER_PHASE_DONE
            || product_world_stream_pending
            // The default native_continue/pab path does not drive the own_stepper phase machine, so
            // its phase stays 0 (!= DONE) -- keep it blocked until in-world regardless.
            || native_continue_enabled()
            || pab_advance_enabled())
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
        // Two drivers fabricate the pad at the poll source: the System->Quit repro autopilot (the
        // user's controller sequence, written to SQ_REPRO_XINPUT_BUTTONS every game-task frame) and
        // own_stepper title nav via inject_nav. Either replaces the (blocked) real pad so the game
        // reads our synthesized buttons.
        // Only fabricate the pad while ACTIVELY driving menus; during WAIT_RELOAD/DONE the reload
        // must not see a synthesized live pad (it bounces the title->world advance back to the FE).
        let sq_repro = sq_repro_actively_driving();
        let inject_nav = inject_nav_enabled()
            && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO;
        if sq_repro || inject_nav {
            // Force SUCCESS + a fresh packet number so a live pad is simulated; write the buttons
            // the active driver scheduled this frame. Harmless if the game ignores XInput.
            let buttons = if sq_repro {
                SQ_REPRO_XINPUT_BUTTONS.load(Ordering::SeqCst) as u16
            } else {
                INJECT_NAV_CUR_BUTTONS.load(Ordering::SeqCst) as u16
            };
            // sq-repro has no separate poll-frame schedule, so bump the shared packet counter here
            // to guarantee a fresh dwPacketNumber each poll; inject_nav keeps its own counter.
            let pkt = if sq_repro {
                INJECT_NAV_FRAME.fetch_add(1, Ordering::SeqCst) as u32
            } else {
                INJECT_NAV_FRAME.load(Ordering::SeqCst) as u32
            };
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

/// Tracks whether the DInput keyboard+mouse `install_hooks` has succeeded.
static DINPUT_BLOCK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static MISSING_SAVE_INPUT_RELEASE_LOGGED: AtomicUsize = AtomicUsize::new(0);

/// Enforce the comprehensive input block for this frame. Self-contained (no args) so it can
/// run from EITHER the game task OR the render loop -- critical because under the offline
/// launcher no render callback executes at the title, so the render-loop call
/// alone never engaged the block (that was the contamination hole). Driven every frame from
/// the game task while `block_input_enabled()`:
///   1. ONCE: install the DInput8 keyboard+mouse `GetDeviceState` block (panics on probe
///      failure -> contained with catch_unwind so the FD4 task never unwinds into C++).
///   2. EVERY frame: assert the block-all flag (sticky, overriding any overlay want-capture
///      clear) and install/retry the XInput gamepad hook until the xinput DLL is present.
/// Genuinely zero-input: it only SUPPRESSES device reads -- it never synthesizes any input.
pub(crate) fn enforce_input_block_now() {
    let blocker = InputBlocker::get_instance();
    if missing_save_selection_pending() {
        BLOCK_INPUT_ACTIVE.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
        blocker.block_only(InputFlags::empty());
        let _ = unsafe { ClipCursor(None) };
        if MISSING_SAVE_INPUT_RELEASE_LOGGED.swap(1, Ordering::SeqCst) == 0 {
            append_autoload_debug(format_args!(
                "input-block: BYPASSED/RELEASED while missing-save picker is pending -- user must be able to click OK and choose a file"
            ));
        }
        return;
    }
    let blocker = InputBlocker::get_instance();
    if DINPUT_BLOCK_INSTALLED.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS {
        let res = std::panic::catch_unwind(|| unsafe { blocker.install_hooks() });
        match res {
            Ok(Ok(())) => {
                DINPUT_BLOCK_INSTALLED.store(BLOCK_INPUT_ON, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "input-block: DInput8 GetDeviceState hooks INSTALLED (minhook, no-hudhook)"
                ));
            }
            Ok(Err(status)) => append_autoload_debug(format_args!(
                "input-block: DInput8 GetDeviceState hook install failed: {status:?}; will retry"
            )),
            Err(_) => append_autoload_debug(format_args!(
                "input-block: DInput8 probe/hook install panicked (dinput8/device not ready?); will retry"
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
    let csfeman = cs_fe_man_ptr_or_null();
    let latch = unsafe { *((base + TITLE_ACCEPT_LATCH_RVA) as *const u8) };
    append_autoload_debug(format_args!(
        "render_probe: frame={frame} csfeman=0x{csfeman:x} latch={latch}"
    ));
}
