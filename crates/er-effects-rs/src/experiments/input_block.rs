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
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM},
        System::{
            LibraryLoader::{GetModuleHandleA, GetProcAddress},
            Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery},
            SystemServices::DLL_PROCESS_ATTACH,
            Threading::{AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId},
        },
        UI::{
            Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
                SendInput, SetFocus, VIRTUAL_KEY,
            },
            WindowsAndMessaging::{
                BringWindowToTop, ClipCursor, EnumWindows, GetClassNameW, GetForegroundWindow,
                GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
                PostMessageW, SetForegroundWindow, WM_KEYDOWN, WM_KEYUP,
            },
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
/// Monotonic `dwPacketNumber` for the no-controller "connected idle pad" keepalive the
/// `xinput_get_state_hook` presents on slot 0 while an XInput harness is armed (see the hook doc).
/// Private to the keepalive so it never perturbs the fabrication cadence in `INJECT_NAV_FRAME`.
static XINPUT_KEEPALIVE_PACKET: AtomicUsize = AtomicUsize::new(0);
/// Original `XInputGetCapabilities` (minhook trampoline). 0 until the hook installs. The game uses
/// this to ENUMERATE which pad slots exist; with no controller it returns DEVICE_NOT_CONNECTED and
/// the game then never polls `XInputGetState(0)`. The harness forces slot 0 connected here too.
static XINPUT_GET_CAPABILITIES_ORIG: AtomicUsize = AtomicUsize::new(0);
/// DIAGNOSTIC: total `XInputGetState(user_index==0)` calls the game makes (the poll counter). If this
/// stays 0 while the sq-repro harness holds at OPEN_MENU, native ER is NOT polling slot 0 (cached
/// "no controller" from a pre-hook enumeration -> our button fabrication can never land, and a device
/// re-scan is required). If it climbs but the menu still does not open, ER polls but ignores the
/// fabricated buttons (a different problem). Read/logged from `system_quit_repro_tick`.
pub(crate) static XINPUT_SLOT0_POLLS: AtomicUsize = AtomicUsize::new(0);
/// DIAGNOSTIC: times we wrote a NON-ZERO fabricated button into a slot-0 poll (so the log can show
/// the game both polled slot 0 AND received a real button edge from us).
pub(crate) static XINPUT_SLOT0_FABRICATED_BUTTONS: AtomicUsize = AtomicUsize::new(0);
/// DIAGNOSTIC: total `XInputGetCapabilities(user_index==0)` calls (the ENUMERATION probe). Non-zero
/// means the game re-enumerated slot 0 after our hook installed (so forcing "connected" there can
/// convince it slot 0 exists); 0 means it enumerated once at startup and cached the result.
pub(crate) static XINPUT_SLOT0_CAPS_QUERIES: AtomicUsize = AtomicUsize::new(0);
/// Cached ER main window HWND for WM keyboard injection (0 = not found yet). Native ER does NOT read
/// keyboard via DInput (proven 2026-07-17: dinput_kb_fires==0) nor route fabricated XInput to menu
/// actions, so the self-drive posts real WM_KEYDOWN/WM_KEYUP to this window (ER reads keyboard via
/// window messages / RawInput; PostMessageW reaches it without foreground).
static SQ_REPRO_ER_HWND: AtomicUsize = AtomicUsize::new(0);
/// The VK currently "held" by the WM key driver (0 = none), so we post one clean KEYDOWN on press and
/// one KEYUP on release instead of spamming per frame.
static SQ_REPRO_HELD_VK: AtomicUsize = AtomicUsize::new(0);

/// Best (largest-area) candidate window + its area, tracked across the EnumWindows callback.
static SQ_REPRO_BEST_HWND: AtomicUsize = AtomicUsize::new(0);
static SQ_REPRO_BEST_AREA: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn sq_repro_find_hwnd_cb(hwnd: HWND, _l: LPARAM) -> BOOL {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid != unsafe { GetCurrentProcessId() } || !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return BOOL(1);
    }
    // Skip OUR OWN overlay/helper windows (class 'ErEffectsLoadingOverlay', the fullscreen D3D12
    // present-overlay window). It is the LARGEST visible window owned by this process, so without this
    // filter the finder picked IT and every SendInput/foreground went to our overlay instead of the ER
    // game window -- the root cause of "no key opens the menu" (runtime-proven 2026-07-17).
    let mut cls = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, &mut cls) }.max(0) as usize;
    let cls_s = String::from_utf16_lossy(&cls[..n.min(cls.len())]);
    if cls_s.contains("ErEffects") || cls_s.contains("er-effects") {
        return BOOL(1);
    }
    // Pick the LARGEST visible window owned by this process -- the game render window, not a helper/
    // overlay/console window (focusing the wrong one is why SendInput could miss the game).
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok() {
        let w = (rect.right - rect.left).max(0) as usize;
        let h = (rect.bottom - rect.top).max(0) as usize;
        let area = w * h;
        if area > SQ_REPRO_BEST_AREA.load(Ordering::SeqCst) {
            SQ_REPRO_BEST_AREA.store(area, Ordering::SeqCst);
            SQ_REPRO_BEST_HWND.store(hwnd.0 as usize, Ordering::SeqCst);
        }
    }
    BOOL(1) // keep enumerating to find the largest
}

/// Return (and cache) the ER main game window HWND: the LARGEST visible top-level window owned by this
/// process. Logs the chosen window's class/title/rect once so it can be confirmed as the game window.
fn sq_repro_er_hwnd() -> HWND {
    let cached = SQ_REPRO_ER_HWND.load(Ordering::SeqCst);
    if cached != 0 {
        return HWND(cached as *mut core::ffi::c_void);
    }
    SQ_REPRO_BEST_HWND.store(0, Ordering::SeqCst);
    SQ_REPRO_BEST_AREA.store(0, Ordering::SeqCst);
    let _ = unsafe { EnumWindows(Some(sq_repro_find_hwnd_cb), LPARAM(0)) };
    let best = SQ_REPRO_BEST_HWND.load(Ordering::SeqCst);
    if best != 0 {
        SQ_REPRO_ER_HWND.store(best, Ordering::SeqCst);
        let hwnd = HWND(best as *mut core::ffi::c_void);
        let mut cls = [0u16; 128];
        let mut title = [0u16; 128];
        let n = unsafe { GetClassNameW(hwnd, &mut cls) }.max(0) as usize;
        let m = unsafe { GetWindowTextW(hwnd, &mut title) }.max(0) as usize;
        let cls_s = String::from_utf16_lossy(&cls[..n.min(cls.len())]);
        let title_s = String::from_utf16_lossy(&title[..m.min(title.len())]);
        let area = SQ_REPRO_BEST_AREA.load(Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "sq-repro: ER window selected hwnd=0x{best:x} class='{cls_s}' title='{title_s}' area={area}px (SendInput/foreground target)"
        ));
    }
    HWND(SQ_REPRO_ER_HWND.load(Ordering::SeqCst) as *mut core::ffi::c_void)
}

/// Count of foreground-forces performed (diagnostic) and whether the ER window is currently the
/// foreground window at the last drive (1/0), so the OPEN_MENU diag can report if focus was achieved.
pub(crate) static SQ_REPRO_FOREGROUND_FORCES: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SQ_REPRO_IS_FOREGROUND: AtomicUsize = AtomicUsize::new(0);

/// Force the ER window to the foreground (headless me3 launches often leave it non-foreground, and
/// native ER only processes RawInput menu input for the FOREGROUND window). Uses the AttachThreadInput
/// trick so `SetForegroundWindow` is not silently refused. Records whether it ended up foreground.
fn sq_repro_ensure_foreground(hwnd: HWND) {
    unsafe {
        let fg = GetForegroundWindow();
        let already = fg == hwnd;
        SQ_REPRO_IS_FOREGROUND.store(already as usize, Ordering::SeqCst);
        if already {
            return;
        }
        let cur_tid = GetCurrentThreadId();
        let fg_tid = if fg.0.is_null() {
            0
        } else {
            GetWindowThreadProcessId(fg, None)
        };
        let attached =
            fg_tid != 0 && fg_tid != cur_tid && AttachThreadInput(cur_tid, fg_tid, true).as_bool();
        let _ = BringWindowToTop(hwnd);
        let ok = SetForegroundWindow(hwnd).as_bool();
        let _ = SetFocus(Some(hwnd));
        if attached {
            let _ = AttachThreadInput(cur_tid, fg_tid, false);
        }
        SQ_REPRO_FOREGROUND_FORCES.fetch_add(1, Ordering::SeqCst);
        SQ_REPRO_IS_FOREGROUND.store(
            (ok || GetForegroundWindow() == hwnd) as usize,
            Ordering::SeqCst,
        );
    }
}

/// Force the ER game window to the foreground NOW (find + focus), mimicking the user's first
/// interaction: clicking the game in the taskbar at world-readiness to make it the active window
/// before pressing START. Native ER only routes RawInput menu keys to the FOREGROUND window, so the
/// self-drive must own focus before its first key. Idempotent + logged once.
pub(crate) fn sq_repro_force_foreground_now() {
    let hwnd = sq_repro_er_hwnd();
    if hwnd.0.is_null() {
        return;
    }
    sq_repro_ensure_foreground(hwnd);
    if SQ_REPRO_INITIAL_FOREGROUND_LOGGED.swap(1, Ordering::SeqCst) == 0 {
        append_autoload_debug(format_args!(
            "sq-repro: INITIAL FOREGROUND FORCE at world-readiness (mimics the user's taskbar click) hwnd=0x{:x} is_foreground={}",
            hwnd.0 as usize,
            SQ_REPRO_IS_FOREGROUND.load(Ordering::SeqCst)
        ));
    }
}
static SQ_REPRO_INITIAL_FOREGROUND_LOGGED: AtomicUsize = AtomicUsize::new(0);

/// SendInput one VK keyboard event (down or up) at the OS level -> delivered as RawInput to the
/// foreground window. Native ER reads keyboard via RawInput (proven: not DInput, ignores posted
/// WM_KEYDOWN), so this is the real menu-input channel; it requires the ER window to be foreground
/// (forced by `sq_repro_ensure_foreground`).
fn sq_repro_send_vk(vk: u32, keyup: bool) {
    let ki = KEYBDINPUT {
        wVk: VIRTUAL_KEY(vk as u16),
        wScan: 0,
        dwFlags: if keyup {
            KEYEVENTF_KEYUP
        } else {
            KEYBD_EVENT_FLAGS(0)
        },
        time: 0,
        dwExtraInfo: 0,
    };
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki },
    };
    unsafe {
        SendInput(&[input], core::mem::size_of::<INPUT>() as i32);
    }
}

/// Drive a keyboard key (Win32 VK code; 0 = release the held key) to the ER window for the
/// self-driving System->Quit repro. Native ER does NOT read keyboard via DInput and ignores posted
/// WM_KEYDOWN, so this forces the window foreground and uses OS-level `SendInput` (delivered as
/// RawInput). Posts a clean key-down on press and key-up on release when the VK transitions. Gated by
/// the caller (only the sq-repro autopilot calls it) so it never touches the product path.
pub(crate) fn sq_repro_drive_wm_key(vk: u32) {
    let hwnd = sq_repro_er_hwnd();
    if hwnd.0.is_null() {
        return;
    }
    let prev = SQ_REPRO_HELD_VK.swap(vk as usize, Ordering::SeqCst) as u32;
    // Only force ER foreground when we actually have a key to deliver (pressing, holding, or
    // releasing). Doing it every idle frame (e.g. all of WAIT_WORLD during the ~60s boot) churns the
    // window focus for no reason and can disturb the boot; skip it when idle (vk==0 and none held).
    if vk != 0 || prev != 0 {
        sq_repro_ensure_foreground(hwnd);
    }
    if prev == vk {
        return;
    }
    if prev != 0 {
        sq_repro_send_vk(prev, true);
    }
    if vk != 0 {
        sq_repro_send_vk(vk, false);
    }
}

/// Like `sq_repro_drive_wm_key` but NEVER forces the window foreground -- it delivers the held key
/// ONLY when ER is ALREADY the foreground window, and releases any held key the moment ER loses focus.
/// Used by the can-move probe so it can never steal the user's focus (the earlier probe yanked ER to
/// the front and trapped the user's keyboard). If the user alt-tabs away, the probe stops injecting.
#[allow(dead_code)] // kept: fallback OS-keyboard driver; the can-move probe now uses the pad-poll hook
pub(crate) fn move_probe_drive_key_foreground_only(vk: u32) {
    let hwnd = sq_repro_er_hwnd();
    if hwnd.0.is_null() {
        return;
    }
    let fg = unsafe { GetForegroundWindow() };
    if fg.0 as usize != hwnd.0 as usize {
        // ER is not focused -> release any held key and do nothing (respect the user's other window).
        let prev = SQ_REPRO_HELD_VK.swap(0, Ordering::SeqCst) as u32;
        if prev != 0 {
            sq_repro_send_vk(prev, true);
        }
        return;
    }
    let prev = SQ_REPRO_HELD_VK.swap(vk as usize, Ordering::SeqCst) as u32;
    if prev == vk {
        return;
    }
    if prev != 0 {
        sq_repro_send_vk(prev, true);
    }
    if vk != 0 {
        sq_repro_send_vk(vk, false);
    }
}

/// STAY-ACTIVE gate (`ER_EFFECTS_STAY_ACTIVE=1` / `er-effects-stay-active.txt`). When set, keep ER's
/// input-accept flag `[DLUID+0x88d]` forced to 1 every tick so a virtual gamepad keeps driving the
/// menus while ER is UNFOCUSED -- letting the user work in another window during a golden capture.
/// Decoded: ER clears that flag each frame when it isn't `GetActiveWindow` (`0x141f292bd`); we re-set
/// it. Touches ONLY focus-input gating, never the sim/save/load.
/// DE-GATED (deprecate-env-marker-gate-allowlists-2026-07-19): stay-active forced the input-accept
/// flag `[DLUID+0x88d]` while unfocused -- a diagnostic golden-capture convenience gated by
/// env/marker. Env/marker feature gates are forbidden; retired (permanently off).
pub(crate) fn stay_active_enabled() -> bool {
    false
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
    // (DE-GATED 2026-07-19: the env/marker FORCE-BLOCK override -- block unconditionally past
    // menu-open -- was a falsification diagnostic; env/marker feature gates are forbidden, removed.)
    // INJECT-NAV instrument-capture: keep the block ON past menu-open so the user's input is
    // suppressed while the XInput hook fabricates the cursor nav (so nothing pollutes the
    // capture). The fabricated Down is written INTO the otherwise-blocked gamepad state, so the
    // menu still gets a live (synthesized) input each frame -- it does not stall.
    if own_stepper_enabled() && !own_stepper_passive_enabled() && inject_nav_enabled() {
        return true;
    }
    // NATIVE-WINDOWS PRODUCT is USER-INTERACTIVE (user drives the startup save picker, then plays). The
    // DEFAULT zero-input autoload block below -- DInput/XInput state-zeroing + a 1x1 ClipCursor cursor
    // confinement -- is a Wine-probe PROOF feature (prove the autoload needs no foreign input), NOT product
    // behavior: on the user's machine it TRAPS the mouse to the top-left and eats keyboard/mouse/gamepad from
    // boot until in-world (user-reported 2026-07-15: "the DLL is moving my mouse / clicking / changing focus").
    // So the DEFAULT product path must never confine or suppress the user's input on native Windows. The
    // EXPLICIT probe opt-ins above (sq_repro, ER_EFFECTS_BLOCK_INPUT env/file, inject_nav) are checked first
    // and still engage the block when a real probe wants it, on native Windows or Wine.
    if is_native_windows() {
        return false;
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
///
/// NO-CONTROLLER HARNESS SUPPORT (agent-owned diagnostic): the game only KEEPS polling an XInput
/// slot it believes is connected. When no physical pad is plugged in, the real `XInputGetState(0)`
/// returns ERROR_DEVICE_NOT_CONNECTED, so ER's connection detection stops polling slot 0 and our
/// button-fabrication frames (below) never reach the game -- which is exactly why the sq-repro /
/// inject-nav harness previously only worked with a controller physically attached. To make the
/// harness work with NO controller, whenever an XInput-driven harness is ARMED (env/file gate:
/// `system_quit_repro_enabled()` / `inject_nav_enabled()`) we force slot 0 to report a CONNECTED
/// idle pad (SUCCESS + fresh packet) instead of DEVICE_NOT_CONNECTED. That keeps ER polling slot 0
/// so the fabrication frames land. This is gated STRICTLY behind the existing diagnostic
/// opt-ins (never on the default/product path) and only touches slot 0; other slots and the
/// non-armed case still read a genuinely absent pad as absent.
pub(crate) unsafe extern "system" fn xinput_get_state_hook(user_index: u32, state: *mut u8) -> u32 {
    const XINPUT_SUCCESS: u32 = 0;
    const XINPUT_ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
    // XINPUT_STATE = { DWORD dwPacketNumber; XINPUT_GAMEPAD Gamepad; }; the gamepad sub-struct
    // (wButtons,bLeftTrigger,bRightTrigger,sThumbLX/LY/RX/RY) starts at +4 and is 12 bytes.
    const XINPUT_GAMEPAD_OFFSET: usize = 4;
    const XINPUT_GAMEPAD_SIZE: usize = 12;
    const ZERO_FILL_BYTE: u8 = 0;
    const XINPUT_PRIMARY_USER_INDEX: u32 = 0;
    // DIAGNOSTIC: count every slot-0 poll so we can tell whether native ER is polling XInput slot 0 at
    // all (see XINPUT_SLOT0_POLLS doc). Cheap Relaxed add on the hot poll path.
    if user_index == XINPUT_PRIMARY_USER_INDEX {
        XINPUT_SLOT0_POLLS.fetch_add(1, Ordering::Relaxed);
    }
    let orig = XINPUT_GET_STATE_ORIG.load(Ordering::SeqCst);
    let mut hr = if orig != TITLE_OWNER_SCAN_START_ADDRESS {
        let f: unsafe extern "system" fn(u32, *mut u8) -> u32 =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(user_index, state) }
    } else {
        XINPUT_ERROR_DEVICE_NOT_CONNECTED
    };
    const XINPUT_PACKET_OFFSET: usize = 0;
    const WBUTTONS_OFFSET_IN_GAMEPAD: usize = 0;
    // sThumbLY within XINPUT_GAMEPAD: wButtons(u16)@0, bLeftTrigger@2, bRightTrigger@3, sThumbLX@4,
    // sThumbLY@6. Used by the can-move probe lane to walk the character forward and measure motion.
    const XINPUT_THUMB_LY_OFFSET_IN_GAMEPAD: usize = 6;
    // CAN-MOVE PROBE lane (2026-07-18): when the readiness verifier is testing input-causes-movement,
    // present a connected slot-0 pad with ONLY the left stick set (no buttons) so the game walks the
    // character. Independent of the input-block / sq-repro gates -- the probe owns the pad for its
    // brief in-world window regardless of block state, so the injected stick always lands.
    if !state.is_null()
        && user_index == XINPUT_PRIMARY_USER_INDEX
        && MOVE_PROBE_ACTIVE.load(Ordering::SeqCst)
    {
        let ly = MOVE_PROBE_STICK_LY.load(Ordering::SeqCst) as i16;
        let pkt = INJECT_NAV_FRAME.fetch_add(1, Ordering::SeqCst) as u32;
        unsafe {
            std::ptr::write_bytes(
                state.add(XINPUT_GAMEPAD_OFFSET),
                ZERO_FILL_BYTE,
                XINPUT_GAMEPAD_SIZE,
            );
            *(state.add(XINPUT_PACKET_OFFSET) as *mut u32) = pkt;
            *(state.add(XINPUT_GAMEPAD_OFFSET + XINPUT_THUMB_LY_OFFSET_IN_GAMEPAD) as *mut i16) =
                ly;
        }
        return XINPUT_SUCCESS;
    }
    // PASSIVE INPUT-TRACE CAPTURE (er-effects-input-trace.txt): record the REAL slot-0 pad state
    // exactly as the original returned it, BEFORE the keepalive/fabrication branches below can
    // overwrite the caller's buffer. A single Relaxed flag load when the trace is off; never
    // mutates `state` or `hr`, so pass-through/block behavior stays byte-identical.
    if user_index == XINPUT_PRIMARY_USER_INDEX && hr == XINPUT_SUCCESS && !state.is_null() {
        input_trace_record_real_poll(state as *const u8);
    }
    // KEEP SLOT 0 "CONNECTED" while an XInput harness is armed, even when no physical pad exists and
    // even before the block flag flips ON -- ER's connection scan can sample the slot on any frame,
    // and if it ever sees DEVICE_NOT_CONNECTED it can stop polling slot 0 (killing the fabrication
    // below). Present a connected idle pad with a fresh packet so the slot stays live. Gated behind
    // the diagnostic harness opt-ins only; never runs on the default/product path.
    if user_index == XINPUT_PRIMARY_USER_INDEX
        && hr == XINPUT_ERROR_DEVICE_NOT_CONNECTED
        && !state.is_null()
        && (system_quit_repro_enabled() || inject_nav_enabled() || prove_movement_enabled())
    {
        // Advance a private keepalive counter (NOT INJECT_NAV_FRAME, whose cadence drives the
        // fabrication schedule) so the "connected" pad always presents a fresh, changing packet.
        let pkt = XINPUT_KEEPALIVE_PACKET.fetch_add(1, Ordering::SeqCst) as u32;
        unsafe {
            std::ptr::write_bytes(
                state.add(XINPUT_GAMEPAD_OFFSET),
                ZERO_FILL_BYTE,
                XINPUT_GAMEPAD_SIZE,
            );
            *(state.add(XINPUT_PACKET_OFFSET) as *mut u32) = pkt;
        }
        hr = XINPUT_SUCCESS;
    }
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
            // DIAGNOSTIC: record that the game polled slot 0 AND received a real fabricated button
            // edge from us this poll (so the log distinguishes "polled + got a button" from "polled
            // idle"). Only meaningful when the game actually calls this hook for slot 0.
            if buttons != 0 && user_index == XINPUT_PRIMARY_USER_INDEX {
                XINPUT_SLOT0_FABRICATED_BUTTONS.fetch_add(1, Ordering::Relaxed);
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

/// `XInputGetCapabilities(user_index, flags, *mut XINPUT_CAPABILITIES) -> DWORD` detour. The game
/// calls this to ENUMERATE connected pads; when it returns DEVICE_NOT_CONNECTED for slot 0 (no
/// physical controller) the game stops polling that slot, so the fabrication in
/// `xinput_get_state_hook` never lands (the root cause of "the harness only works with a controller
/// plugged in"). While an XInput harness is ARMED, force slot 0 to report a connected standard
/// gamepad so enumeration keeps slot 0 live. Gated strictly behind the diagnostic harness opt-ins;
/// non-armed and other slots pass through untouched (a genuinely absent pad still reads absent).
pub(crate) unsafe extern "system" fn xinput_get_capabilities_hook(
    user_index: u32,
    flags: u32,
    caps: *mut u8,
) -> u32 {
    const XINPUT_SUCCESS: u32 = 0;
    const XINPUT_ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
    const XINPUT_PRIMARY_USER_INDEX: u32 = 0;
    // XINPUT_CAPABILITIES = { BYTE Type; BYTE SubType; WORD Flags; XINPUT_GAMEPAD Gamepad;
    //                         XINPUT_VIBRATION Vibration; } == 20 bytes.
    const XINPUT_CAPABILITIES_SIZE: usize = 20;
    const XINPUT_DEVTYPE_GAMEPAD: u8 = 1;
    const XINPUT_DEVSUBTYPE_GAMEPAD: u8 = 1;
    const CAPS_TYPE_OFFSET: usize = 0;
    const CAPS_SUBTYPE_OFFSET: usize = 1;
    // DIAGNOSTIC: count slot-0 enumeration probes (see XINPUT_SLOT0_CAPS_QUERIES doc).
    if user_index == XINPUT_PRIMARY_USER_INDEX {
        XINPUT_SLOT0_CAPS_QUERIES.fetch_add(1, Ordering::Relaxed);
    }
    let orig = XINPUT_GET_CAPABILITIES_ORIG.load(Ordering::SeqCst);
    let hr = if orig != TITLE_OWNER_SCAN_START_ADDRESS {
        let f: unsafe extern "system" fn(u32, u32, *mut u8) -> u32 =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(user_index, flags, caps) }
    } else {
        XINPUT_ERROR_DEVICE_NOT_CONNECTED
    };
    if user_index == XINPUT_PRIMARY_USER_INDEX
        && hr == XINPUT_ERROR_DEVICE_NOT_CONNECTED
        && !caps.is_null()
        && (system_quit_repro_enabled() || inject_nav_enabled() || prove_movement_enabled())
    {
        unsafe {
            std::ptr::write_bytes(caps, 0, XINPUT_CAPABILITIES_SIZE);
            *caps.add(CAPS_TYPE_OFFSET) = XINPUT_DEVTYPE_GAMEPAD;
            *caps.add(CAPS_SUBTYPE_OFFSET) = XINPUT_DEVSUBTYPE_GAMEPAD;
        }
        return XINPUT_SUCCESS;
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
        // XInputGetCapabilities is the slot-ENUMERATION call the game uses to decide which pads to
        // poll. Hook it so a harness-armed run can keep slot 0 "connected" with no physical
        // controller (see xinput_get_capabilities_hook). Same DLL, resolved by name.
        let caps = unsafe { GetProcAddress(hmod, PCSTR(b"XInputGetCapabilities\0".as_ptr())) };
        if let Some(caps_addr) = caps {
            let caps_addr = caps_addr as usize;
            match unsafe {
                MhHook::new(
                    caps_addr as *mut c_void,
                    xinput_get_capabilities_hook as *mut c_void,
                )
            } {
                Ok(hook) => {
                    XINPUT_GET_CAPABILITIES_ORIG
                        .store(hook.trampoline() as usize, Ordering::SeqCst);
                    let _ = unsafe { hook.queue_enable() };
                    std::mem::forget(hook);
                    append_autoload_debug(format_args!(
                        "xinput-block: hooked XInputGetCapabilities at 0x{caps_addr:x}"
                    ));
                }
                Err(status) => append_autoload_debug(format_args!(
                    "xinput-block: MhHook::new XInputGetCapabilities failed: {status:?}"
                )),
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

/// PASSIVE INPUT-TRACE support: install the XInput hooks WITHOUT engaging any input block. With
/// `BLOCK_INPUT_ACTIVE` clear and no harness gate armed the detour is a pure pass-through (one
/// Relaxed poll counter + the trace capture), so installing it early fabricates nothing and blocks
/// nothing. Same retry-until-hooked idiom as `enforce_input_block_now` (xinput DLL may load late).
/// Deliberately does NOT install the DInput hooks or touch ClipCursor.
pub(crate) fn ensure_xinput_hook_installed_for_trace() {
    if XINPUT_GET_STATE_ORIG.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { install_xinput_block() };
    }
    ensure_rawinput_counter_installed();
}

/// Install the RawInput reception counter ONCE (idempotent). Called UNCONDITIONALLY every frame from
/// tick_before_player_lookup -- unlike the xinput trace path this must run on EVERY run (it is the
/// contamination oracle: whether the game received user mouse/keyboard input), not only when the
/// input-trace marker is armed. Pure counting pass-through; never blocks input.
pub(crate) fn ensure_rawinput_counter_installed() {
    if GET_RAW_INPUT_DATA_ORIG.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { install_rawinput_counter() };
    }
}

/// GetRawInputData reception counters (user 2026-07-20): the oracle must RECORD whether the GAME is
/// RECEIVING user mouse/keyboard input, at the OS boundary. ER reads gameplay+menu input via RawInput;
/// the input-harness injects via the direct-memory inputmgr, NOT RawInput -- so every RawInput event
/// counted here is USER input the game received (contamination during an agent-owned run). Emitted as
/// oracle_rawinput_* and consumed by the verdict emitter.
pub(crate) static RAWINPUT_MOUSE_MOVE_EVENTS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static RAWINPUT_MOUSE_BUTTON_EVENTS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static RAWINPUT_KEY_EVENTS: AtomicUsize = AtomicUsize::new(0);
static GET_RAW_INPUT_DATA_ORIG: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

/// GetRawInputData(hRawInput, uiCommand, pData, pcbSize, cbSizeHeader) pass-through detour: call the
/// original, then if it returned a RID_INPUT record, classify it and bump the reception counter. Never
/// drops input (recording only). RAWINPUTHEADER is 0x18 bytes on x64; RAWMOUSE.usButtonFlags @ +0x04,
/// lLastX @ +0x0C, lLastY @ +0x10; RAWKEYBOARD Message @ +0x08 (WM_KEYDOWN 0x100 / WM_SYSKEYDOWN 0x104).
unsafe extern "system" fn get_raw_input_data_hook(
    h_raw_input: isize,
    ui_command: u32,
    p_data: *mut c_void,
    pcb_size: *mut u32,
    cb_size_header: u32,
) -> u32 {
    let orig_addr = GET_RAW_INPUT_DATA_ORIG.load(Ordering::SeqCst);
    let orig: unsafe extern "system" fn(isize, u32, *mut c_void, *mut u32, u32) -> u32 =
        unsafe { std::mem::transmute(orig_addr) };
    let ret = unsafe { orig(h_raw_input, ui_command, p_data, pcb_size, cb_size_header) };
    const RID_INPUT: u32 = 0x1000_0003;
    if !p_data.is_null() && ui_command == RID_INPUT && ret != u32::MAX && ret >= 0x30 {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let base = p_data as usize;
            let dwtype = unsafe { (base as *const u32).read_unaligned() };
            let d = base + 0x18; // past RAWINPUTHEADER
            if dwtype == 0 {
                // RIM_TYPEMOUSE
                let btn = unsafe { ((d + 0x04) as *const u16).read_unaligned() };
                let lx = unsafe { ((d + 0x0C) as *const i32).read_unaligned() };
                let ly = unsafe { ((d + 0x10) as *const i32).read_unaligned() };
                if lx != 0 || ly != 0 {
                    RAWINPUT_MOUSE_MOVE_EVENTS.fetch_add(1, Ordering::Relaxed);
                }
                if btn != 0 {
                    RAWINPUT_MOUSE_BUTTON_EVENTS.fetch_add(1, Ordering::Relaxed);
                }
            } else if dwtype == 1 {
                // RIM_TYPEKEYBOARD -- count key-down messages only
                let msg = unsafe { ((d + 0x08) as *const u32).read_unaligned() };
                if msg == 0x100 || msg == 0x104 {
                    RAWINPUT_KEY_EVENTS.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }
    ret
}

/// Install the GetRawInputData reception counter (user32.dll). minhook, mirroring install_xinput_block.
/// Recording only -- never blocks. Retried each frame until user32 GetRawInputData resolves.
unsafe fn install_rawinput_counter() {
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "rawinput-counter: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let hmod = match unsafe { GetModuleHandleA(PCSTR(b"user32.dll\0".as_ptr())) } {
        Ok(h) if !h.is_invalid() => h,
        _ => return,
    };
    let Some(addr) = (unsafe { GetProcAddress(hmod, PCSTR(b"GetRawInputData\0".as_ptr())) }) else {
        return;
    };
    let addr = addr as usize;
    match unsafe { MhHook::new(addr as *mut c_void, get_raw_input_data_hook as *mut c_void) } {
        Ok(hook) => {
            // Store the trampoline BEFORE enabling so the detour never transmutes the unset sentinel.
            GET_RAW_INPUT_DATA_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "rawinput-counter: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    append_autoload_debug(format_args!(
                        "rawinput-counter: hooked GetRawInputData at 0x{addr:x} -- records user mouse/kb input the game receives (contamination oracle)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "rawinput-counter: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "rawinput-counter: MhHook::new GetRawInputData failed: {status:?}"
        )),
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
    // cursor to a 1x1 rect: it physically cannot move regardless of which API reads it, making the run
    // uncontaminatable by the mouse. FREEZE IT IN PLACE at its CURRENT position rather than yanking it
    // to (0,0) -- same protection, but does not disruptively teleport the user's mouse to the top-left
    // corner during a run (user 2026-07-19). Released (ClipCursor(None)) when the block lifts.
    let mut pt = windows::Win32::Foundation::POINT { x: 0, y: 0 };
    let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) };
    let clip = RECT {
        left: pt.x,
        top: pt.y,
        right: pt.x + 1,
        bottom: pt.y + 1,
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
