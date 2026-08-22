//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

use std::{
    ffi::c_void,
    sync::atomic::{AtomicIsize, AtomicUsize, Ordering},
};

use crate::input_blocker::{InputBlocker, InputFlags};
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use eldenring::cs::GameMan;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, RECT},
        System::{
            LibraryLoader::{GetModuleHandleA, GetProcAddress},
            Threading::GetCurrentProcessId,
        },
        UI::{
            Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
                SendInput, VIRTUAL_KEY,
            },
            WindowsAndMessaging::{
                EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowRect, GetWindowTextW,
                GetWindowThreadProcessId, IsWindowVisible,
            },
        },
    },
    core::{BOOL, PCSTR, s},
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, ffi::*, hooks::*, telemetry::*};

use super::*;

#[allow(dead_code)]
/// When set, foreign KEYBOARD + GAMEPAD game input is blocked at the API layer (see
/// `enforce_input_block`): DInput8 keyboard (state zeroed by the `InputBlocker` hook) AND XInput
/// gamepad (this module's hook). The MOUSE is never blocked and the cursor is never confined. Read by
/// `xinput_get_state_hook` each poll so the block is authoritative regardless of window focus.
pub(crate) use er_telemetry::counters::BLOCK_INPUT_ACTIVE;
const BLOCK_INPUT_ON: usize = 1;
/// Cached ER main window HWND for WM keyboard injection (0 = not found yet). Native ER does NOT read
/// keyboard via DInput (proven 2026-07-17: dinput_kb_fires==0) nor route fabricated XInput to menu
/// actions, so the self-drive posts real WM_KEYDOWN/WM_KEYUP to this window (ER reads keyboard via
/// window messages / RawInput; PostMessageW reaches it without foreground).
pub(crate) use er_telemetry::counters::SQ_REPRO_ER_HWND;
/// The VK currently "held" by the WM key driver (0 = none), so we post one clean KEYDOWN on press and
/// one KEYUP on release instead of spamming per frame.
pub(crate) use er_telemetry::counters::SQ_REPRO_HELD_VK;
/// Original `XInputGetCapabilities` (minhook trampoline). 0 until the hook installs. The game uses
/// this to ENUMERATE which pad slots exist; with no controller it returns DEVICE_NOT_CONNECTED and
/// the game then never polls `XInputGetState(0)`. The harness forces slot 0 connected here too.
pub(crate) use er_telemetry::counters::XINPUT_GET_CAPABILITIES_ORIG;
/// Original `XInputGetState` (minhook trampoline). 0 until the hook installs.
pub(crate) use er_telemetry::counters::XINPUT_GET_STATE_ORIG;
/// Monotonic `dwPacketNumber` for the no-controller "connected idle pad" keepalive the
/// `xinput_get_state_hook` presents on slot 0 while an XInput harness is armed (see the hook doc).
/// Private to the keepalive so it never perturbs the fabrication cadence in `INJECT_NAV_FRAME`.
pub(crate) use er_telemetry::counters::XINPUT_KEEPALIVE_PACKET;
/// DIAGNOSTIC: total `XInputGetCapabilities(user_index==0)` calls (the ENUMERATION probe). Non-zero
/// means the game re-enumerated slot 0 after our hook installed (so forcing "connected" there can
/// convince it slot 0 exists); 0 means it enumerated once at startup and cached the result.
pub(crate) use er_telemetry::counters::XINPUT_SLOT0_CAPS_QUERIES;
/// DIAGNOSTIC: times we wrote a NON-ZERO fabricated button into a slot-0 poll (so the log can show
/// the game both polled slot 0 AND received a real button edge from us).
pub(crate) use er_telemetry::counters::XINPUT_SLOT0_FABRICATED_BUTTONS;
/// DIAGNOSTIC: total `XInputGetState(user_index==0)` calls the game makes (the poll counter). If this
/// stays 0 while the sq-repro harness holds at OPEN_MENU, native ER is NOT polling slot 0 (cached
/// "no controller" from a pre-hook enumeration -> our button fabrication can never land, and a device
/// re-scan is required). If it climbs but the menu still does not open, ER polls but ignores the
/// fabricated buttons (a different problem). Read/logged from `system_quit_repro_tick`.
pub(crate) use er_telemetry::counters::XINPUT_SLOT0_POLLS;

pub(crate) use er_telemetry::counters::SQ_REPRO_BEST_AREA;
/// Best (largest-area) candidate window + its area, tracked across the EnumWindows callback.
pub(crate) use er_telemetry::counters::SQ_REPRO_BEST_HWND;

pub(crate) const SAVE_PICKER_NAV_LEFT_MASK: usize = 1 << 0;
pub(crate) const SAVE_PICKER_NAV_RIGHT_MASK: usize = 1 << 1;
/// Up/down edges exist so a list longer than the ten native rows scrolls on an explicit PRESS at
/// the edge row. The picker previously slid the window from a pointer DWELL on the edge, which
/// moved the list under a player who was only resting there.
pub(crate) const SAVE_PICKER_NAV_UP_MASK: usize = 1 << 2;
pub(crate) const SAVE_PICKER_NAV_DOWN_MASK: usize = 1 << 3;
/// Wheel detents are latched SEPARATELY from key/pad directions because the native list treats them
/// differently: a key or pad press at an extreme row makes the list wrap, and the picker rides that
/// wrap as the step signal -- but a wheel detent there moves nothing at all, so a wheel edge that
/// waited for a wrap would wait forever and the wheel would appear dead at exactly the top and
/// bottom rows (reported 2026-08-12). Same directions, different arrival contract.
pub(crate) const SAVE_PICKER_NAV_WHEEL_UP_MASK: usize = 1 << 4;
pub(crate) const SAVE_PICKER_NAV_WHEEL_DOWN_MASK: usize = 1 << 5;
const SAVE_PICKER_NAV_ALL_MASK: usize = SAVE_PICKER_NAV_LEFT_MASK
    | SAVE_PICKER_NAV_RIGHT_MASK
    | SAVE_PICKER_NAV_UP_MASK
    | SAVE_PICKER_NAV_DOWN_MASK
    | SAVE_PICKER_NAV_WHEEL_UP_MASK
    | SAVE_PICKER_NAV_WHEEL_DOWN_MASK;
const VK_LEFT: u16 = 0x25;
const VK_RIGHT: u16 = 0x27;
const VK_UP: u16 = 0x26;
const VK_DOWN: u16 = 0x28;
static SAVE_PICKER_USER_NAV_LATCH: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_XINPUT_NAV_DOWN_MASK: AtomicUsize = AtomicUsize::new(0);
static SAVE_PICKER_DINPUT_ARROW_DOWN_MASK: AtomicUsize = AtomicUsize::new(0);

/// Drain EVERY pending nav edge. Used on the paths that discard input wholesale (picker not live,
/// native text editor owns the screen) so a press made elsewhere cannot replay later.
pub(crate) fn save_picker_take_user_nav_edges() -> usize {
    SAVE_PICKER_USER_NAV_LATCH.swap(0, Ordering::SeqCst) & SAVE_PICKER_NAV_ALL_MASK
}

/// Directions currently HELD on a real device, without consuming anything.
///
/// Distinct from the edge latch on purpose. Elden Ring's menus auto-repeat while a direction is
/// held, and each repeat moves the native list cursor -- but only the FIRST press produces an edge.
/// A consumer that reacts to edges alone therefore handles one step of a held press and lets the
/// native list do whatever it likes for the rest, which at the last row means wrapping to the top.
/// Read from the DInput/XInput device state the game itself polls, so it reflects the real device.
pub(crate) fn save_picker_user_nav_held() -> usize {
    (SAVE_PICKER_DINPUT_ARROW_DOWN_MASK.load(Ordering::SeqCst)
        | SAVE_PICKER_XINPUT_NAV_DOWN_MASK.load(Ordering::SeqCst))
        & SAVE_PICKER_NAV_ALL_MASK
}

/// Drain only the requested directions, leaving the others latched for their own consumer.
///
/// Left/right (drive strip) and up/down (edge scroll) are consumed by two different pumps in the
/// same maintenance tick. A shared consume-everything take would let whichever ran first swallow
/// the other's edges, which is a race by construction rather than an ordering detail to get right.
pub(crate) fn save_picker_take_user_nav_edges_for(mask: usize) -> usize {
    let mask = mask & SAVE_PICKER_NAV_ALL_MASK;
    SAVE_PICKER_USER_NAV_LATCH.fetch_and(!mask, Ordering::SeqCst) & mask
}

fn save_picker_latch_keyboard_nav(vkey: u16) {
    match vkey {
        VK_LEFT => {
            SAVE_PICKER_USER_NAV_LATCH.fetch_or(SAVE_PICKER_NAV_LEFT_MASK, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-nav: rawinput arrow left edge vkey=0x{vkey:x}"
            ));
        }
        VK_RIGHT => {
            SAVE_PICKER_USER_NAV_LATCH.fetch_or(SAVE_PICKER_NAV_RIGHT_MASK, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-nav: rawinput arrow right edge vkey=0x{vkey:x}"
            ));
        }
        VK_UP => {
            SAVE_PICKER_USER_NAV_LATCH.fetch_or(SAVE_PICKER_NAV_UP_MASK, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-nav: rawinput arrow up edge vkey=0x{vkey:x}"
            ));
        }
        VK_DOWN => {
            SAVE_PICKER_USER_NAV_LATCH.fetch_or(SAVE_PICKER_NAV_DOWN_MASK, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-nav: rawinput arrow down edge vkey=0x{vkey:x}"
            ));
        }
        _ => {}
    }
}

pub(crate) fn save_picker_latch_dinput_keyboard_state(data: *const u8, size: usize) {
    const DIK_UP: usize = 0xc8;
    const DIK_LEFT: usize = 0xcb;
    const DIK_RIGHT: usize = 0xcd;
    const DIK_DOWN: usize = 0xd0;
    const DIK_PRESSED: u8 = 0x80;
    // DIK_DOWN is the highest scancode read here, so it sets the buffer-length floor.
    if data.is_null() || size <= DIK_DOWN {
        return;
    }
    let mut down = 0usize;
    if unsafe { *data.add(DIK_LEFT) } & DIK_PRESSED != 0 {
        down |= SAVE_PICKER_NAV_LEFT_MASK;
    }
    if unsafe { *data.add(DIK_RIGHT) } & DIK_PRESSED != 0 {
        down |= SAVE_PICKER_NAV_RIGHT_MASK;
    }
    if unsafe { *data.add(DIK_UP) } & DIK_PRESSED != 0 {
        down |= SAVE_PICKER_NAV_UP_MASK;
    }
    if unsafe { *data.add(DIK_DOWN) } & DIK_PRESSED != 0 {
        down |= SAVE_PICKER_NAV_DOWN_MASK;
    }
    let prev = SAVE_PICKER_DINPUT_ARROW_DOWN_MASK.swap(down, Ordering::SeqCst);
    let edges = down & !prev;
    if edges != 0 {
        SAVE_PICKER_USER_NAV_LATCH.fetch_or(edges, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker-nav: dinput arrow edge mask=0x{edges:x} down=0x{down:x}"
        ));
    }
}

/// Deflection at which a left-stick axis counts as a directional press, and the smaller value it
/// must fall back under before it can press again.
///
/// TWO thresholds, not one: a stick held near a single threshold jitters across it and would latch
/// a stream of phantom edges from one deliberate push. The press value is ~50% deflection, well
/// above XInput's 7849 rest deadzone, so stick drift never navigates on its own.
const XINPUT_STICK_NAV_PRESS: i32 = 16384;
const XINPUT_STICK_NAV_RELEASE: i32 = 8192;

/// Directions the left stick is currently pushed, with the hysteresis above applied against the
/// previously-held mask.
fn xinput_stick_nav_mask(prev: usize, thumb_lx: i16, thumb_ly: i16) -> usize {
    let mut down = 0usize;
    for (mask, value) in [
        (SAVE_PICKER_NAV_LEFT_MASK, -i32::from(thumb_lx)),
        (SAVE_PICKER_NAV_RIGHT_MASK, i32::from(thumb_lx)),
        // XInput reports +Y as UP, which is the opposite of the list's row order.
        (SAVE_PICKER_NAV_UP_MASK, i32::from(thumb_ly)),
        (SAVE_PICKER_NAV_DOWN_MASK, -i32::from(thumb_ly)),
    ] {
        let held = prev & mask != 0;
        let threshold = if held {
            XINPUT_STICK_NAV_RELEASE
        } else {
            XINPUT_STICK_NAV_PRESS
        };
        if value >= threshold {
            down |= mask;
        }
    }
    down
}

/// `RAWMOUSE.usButtonFlags` bit set when this event carries a wheel delta.
const RI_MOUSE_WHEEL: u16 = 0x0400;
/// Wheel units per detent (`WHEEL_DELTA`). One detent is one row, matching one arrow press.
const WHEEL_DELTA: i32 = 120;
/// Accumulates sub-detent wheel motion so a high-resolution/free-spin wheel steps whole rows
/// instead of either flooding the list or dropping every partial notch on the floor.
static SAVE_PICKER_WHEEL_ACCUM: AtomicIsize = AtomicIsize::new(0);

/// Fold one raw wheel delta into the accumulator: returns the nav mask to latch (0 when this delta
/// did not complete a detent) and the sub-detent remainder to carry forward.
///
/// Pure so it can be tested without the process-wide latch: two tests that both drain that latch
/// race each other and report an empty mask, which looks exactly like a broken wheel.
fn wheel_nav_step(accum: isize, delta: i16) -> (usize, isize) {
    let total = accum + isize::from(delta);
    let detents = total / WHEEL_DELTA as isize;
    if detents == 0 {
        return (0, total);
    }
    let mask = if detents > 0 {
        SAVE_PICKER_NAV_WHEEL_UP_MASK
    } else {
        SAVE_PICKER_NAV_WHEEL_DOWN_MASK
    };
    (mask, total - detents * WHEEL_DELTA as isize)
}

/// `HRAWINPUT` of the last WM_INPUT message a wheel detent was taken from, and how many repeat
/// reads of that same message were ignored.
static SAVE_PICKER_WHEEL_LAST_MESSAGE: AtomicIsize = AtomicIsize::new(0);
static SAVE_PICKER_WHEEL_DUPLICATE_READS: AtomicUsize = AtomicUsize::new(0);

/// Latch a wheel detent ONCE PER WM_INPUT MESSAGE.
///
/// `GetRawInputData` is a READ of a message, not the message itself, and the same `HRAWINPUT` can
/// legitimately be read more than once (the size-then-data pattern, and any second consumer in the
/// chain). Counting reads instead of messages multiplied every physical detent: the live log shows
/// five `delta=120` reads inside 10ms, which no hand can spin, and on screen one notch of the wheel
/// moved the selection two rows (reported 2026-08-12).
fn save_picker_latch_wheel_nav_for_message(message: isize, delta: i16) {
    if delta == 0 {
        return;
    }
    if SAVE_PICKER_WHEEL_LAST_MESSAGE.swap(message, Ordering::SeqCst) == message {
        let n = SAVE_PICKER_WHEEL_DUPLICATE_READS.fetch_add(1, Ordering::SeqCst) + 1;
        if n == 1 || n.is_multiple_of(50) {
            append_autoload_debug(format_args!(
                "save-picker-nav: ignored repeat read #{n} of wheel message 0x{message:x} delta={delta}"
            ));
        }
        return;
    }
    save_picker_latch_wheel_nav(delta);
}

/// Latch nav edges from mouse-wheel detents. Wheel UP is toward the top of the list.
pub(crate) fn save_picker_latch_wheel_nav(delta: i16) {
    if delta == 0 {
        return;
    }
    let (mask, remainder) = wheel_nav_step(SAVE_PICKER_WHEEL_ACCUM.load(Ordering::SeqCst), delta);
    SAVE_PICKER_WHEEL_ACCUM.store(remainder, Ordering::SeqCst);
    if mask == 0 {
        return;
    }
    SAVE_PICKER_USER_NAV_LATCH.fetch_or(mask, Ordering::SeqCst);
    let dups = SAVE_PICKER_WHEEL_DUPLICATE_READS.load(Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-picker-nav: wheel edge mask=0x{mask:x} delta={delta} duplicate_reads_so_far={dups}"
    ));
}

/// Latch nav edges from the real slot-0 pad: D-pad buttons AND the left stick.
///
/// The stick is not a nicety. Elden Ring navigates menus with either, so a pad player who flicks
/// the stick produced NO latched edge at all -- the picker's edge handling (scroll at a window
/// edge, hold at the listing's end) never ran, and the native list wrapped from the last row to the
/// top exactly as if the feature did not exist. Keyboard-only coverage made the fix look complete
/// while the pad was still broken (2026-08-12).
fn save_picker_latch_xinput_nav_state(buttons: u16, thumb_lx: i16, thumb_ly: i16) {
    let mut down = 0usize;
    if buttons & XINPUT_GAMEPAD_DPAD_LEFT != 0 {
        down |= SAVE_PICKER_NAV_LEFT_MASK;
    }
    if buttons & XINPUT_GAMEPAD_DPAD_RIGHT != 0 {
        down |= SAVE_PICKER_NAV_RIGHT_MASK;
    }
    if buttons & XINPUT_GAMEPAD_DPAD_UP != 0 {
        down |= SAVE_PICKER_NAV_UP_MASK;
    }
    if buttons & XINPUT_GAMEPAD_DPAD_DOWN != 0 {
        down |= SAVE_PICKER_NAV_DOWN_MASK;
    }
    let prev = SAVE_PICKER_XINPUT_NAV_DOWN_MASK.load(Ordering::SeqCst);
    down |= xinput_stick_nav_mask(prev, thumb_lx, thumb_ly);
    SAVE_PICKER_XINPUT_NAV_DOWN_MASK.store(down, Ordering::SeqCst);
    let edges = down & !prev;
    if edges != 0 {
        SAVE_PICKER_USER_NAV_LATCH.fetch_or(edges, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker-nav: xinput pad edge mask=0x{edges:x} buttons=0x{buttons:x} lx={thumb_lx} ly={thumb_ly}"
        ));
    }
}

pub(crate) fn ensure_save_picker_user_nav_input_hooks_installed() {
    ensure_xinput_hook_installed_for_trace();
    let _ = std::panic::catch_unwind(|| unsafe {
        if let Err(status) = InputBlocker::get_instance().install_hooks() {
            append_autoload_debug(format_args!(
                "save-picker-nav: passive dinput hook install failed: {status:?}"
            ));
        }
    });
}

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
/// process, EXCLUDING our own overlay/helper classes. Logs the chosen window's class/title/rect once
/// so it can be confirmed as the game window. `HWND(null)` when no candidate was found.
///
/// Use this and never `hooks::own_window()`, which returns the FIRST visible window of the process
/// and does not exclude our own surfaces: the fullscreen D3D12 present-overlay
/// (`ErEffectsLoadingOverlay`) is the largest visible window we own, the naive finder picked it, and
/// that was the root cause of "no key opens the menu" (runtime-proven 2026-07-17). Anything that
/// needs the game window -- input targeting, or an `hwndOwner` for a common dialog so a window
/// manager keeps it in front of the game -- must come through here.
pub(crate) fn game_main_window() -> HWND {
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

pub(crate) use er_telemetry::counters::SQ_REPRO_IS_FOREGROUND;

/// FOCUS SEMAPHORE (2026-07-21, focus-controlled A/B): is the OS foreground window owned by THIS (the
/// game) process? Computed FRESH each call (independent of the sq-repro forcing, which stands down in
/// deterministic mode). Under Proton/Wine this reflects Wine's foreground window; we emit it as
/// oracle_window_foreground to test whether the load2/load3 20fps stall correlates with the ER surface
/// being unfocused (the surviving compositor-present-throttle theory). bd
/// CANDIDATE-A-empty-native-loadmode-excluded-compositor-B-surviving-2026-07-21.
pub(crate) fn game_window_is_foreground() -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return false;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(fg, Some(&mut pid as *mut u32));
        pid != 0 && pid == GetCurrentProcessId()
    }
}

/// Record whether the ER window is currently the OS foreground window (FOCUS SEMAPHORE) WITHOUT ever
/// forcing focus. The force-focus path (SetForegroundWindow / BringWindowToTop / SetFocus /
/// AttachThreadInput) was REMOVED (user 2026-07-23, bd harness-drive-contract-...-no-force-focus): the
/// user's window focus must never be seized. This is now OBSERVE-ONLY -- it updates SQ_REPRO_IS_FOREGROUND
/// so diagnostics can report whether ER happened to be focused, but it never brings ER to the front. The
/// legacy sq-repro SendInput menu-nav that relied on forced focus has been DELETED (nothing ever
/// transitioned into its states; the live sq-repro flow uses the menu-free programmatic switch arm),
/// and the can-move probe now delivers movement foreground-only, so no live path forces focus.
fn sq_repro_ensure_foreground(hwnd: HWND) {
    let already = unsafe { GetForegroundWindow() } == hwnd;
    SQ_REPRO_IS_FOREGROUND.store(already as usize, Ordering::SeqCst);
}

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
    let hwnd = game_main_window();
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
    let hwnd = game_main_window();
    if hwnd.0.is_null() {
        return;
    }
    let fg = unsafe { GetForegroundWindow() };
    if !std::ptr::eq(fg.0, hwnd.0) {
        // ER is not focused -> release any held key and do nothing (respect the user's other window).
        let prev = SQ_REPRO_HELD_VK.swap(0, Ordering::SeqCst) as u32;
        if prev != 0 {
            sq_repro_send_vk(prev, true);
        }
        return;
    }
    let prev = SQ_REPRO_HELD_VK.swap(vk as usize, Ordering::SeqCst) as u32;
    if prev != 0 && prev != vk {
        sq_repro_send_vk(prev, true);
    }
    if vk != 0 {
        // Movement proof is sampled per frame. Send a foreground-gated key-down every ON frame instead
        // of only on the first transition so a lost/filtered single event cannot make the proof falsely
        // report "no supplied input". This path never forces focus; if ER is not already foreground it
        // returned above. Count keyboard delivery as supplied movement input, distinct from actual motion.
        sq_repro_send_vk(vk, false);
        crate::constants::SUPPLIED_MOVEMENT_INPUT_FRAMES.fetch_add(1, Ordering::Relaxed);
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
/// front-end/title (observed: switch #1's SetState5 loaded the char then the game jumped to 01_000_FE +
/// SetState 2/3/10 = press-any-button softlock). Treating WAIT_RELOAD like DONE makes the reload
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

/// TRUE only while the harness is ACTIVELY INJECTING input THIS frame -- the can-move probe's ON burst
/// (`MOVE_PROBE_ACTIVE`) or the System->Quit repro autopilot actively driving menus
/// (`sq_repro_actively_driving`). This is the ONLY window in which the product may fabricate a device or
/// otherwise touch input state on the harness's behalf (bd input-blocking-only-in-harness-during-driving-
/// never-in-product-never-outside-window-2026-07-23). Outside it -- boot, the post-load in-world DWELL,
/// between move-probe intervals -- the harness is not injecting, so the user's keyboard and mouse must be
/// fully live and nothing here may present a phantom pad or suppress input. FALSE the instant injection
/// stops (MOVE_PROBE_ACTIVE latches false the moment the move-probe verdict is reached), so the dwell has
/// full control.
pub(crate) fn harness_injection_active() -> bool {
    MOVE_PROBE_ACTIVE.load(Ordering::SeqCst) || sq_repro_actively_driving()
}

fn native_loading_screen_started_recently() -> bool {
    const LOAD_STARTED_FRESH_MS: usize = 250;
    let last_ms = LOADING_SCREEN_UPDATE_LAST_MS.load(Ordering::SeqCst);
    if last_ms == 0 {
        return false;
    }
    let age_ms = (boot_view_epoch_ms() as usize).saturating_sub(last_ms);
    age_ms <= LOAD_STARTED_FRESH_MS
        && LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst) != 0
        && LOADING_SCREEN_BAR_ENABLED.load(Ordering::SeqCst) != 0
        && LOADING_SCREEN_BAR_MAX_FRAME.load(Ordering::SeqCst) != 0
}

fn game_man_load_sequence_started() -> bool {
    const GAME_MAN_SAVE_STATE_IDLE: usize = 0;
    let gm = crate::game_man_ptr_or_null();
    if gm == 0 || gm == TITLE_OWNER_SCAN_START_ADDRESS {
        return false;
    }
    let save_state_offset = core::mem::offset_of!(GameMan, save_state);
    unsafe { safe_read_usize(gm + save_state_offset) }
        .map(|save_state| (save_state as u32 as usize) != GAME_MAN_SAVE_STATE_IDLE)
        .unwrap_or(false)
}

fn autoload_load_started() -> bool {
    if native_loading_screen_started_recently() || game_man_load_sequence_started() {
        return true;
    }
    if let Ok(base) = game_module_base() {
        // Current render-pipeline cover visibility is a load-start/current-load signal. Do not use
        // CSNowLoadingHelperImp::load_done here: that latch is load-COMPLETE and lingers into gameplay.
        return unsafe { fake_loading_screen_visible(base) };
    }
    false
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
    // DEFAULT zero-input autoload block below -- DInput keyboard + XInput gamepad state-zeroing -- is a
    // Wine-probe PROOF feature (prove the autoload needs no foreign keyboard/gamepad input), NOT product
    // behavior: on the user's machine it would eat the user's keyboard/gamepad from boot until in-world
    // (user-reported 2026-07-15: "the DLL is moving my mouse / clicking / changing focus"; the mouse
    // forcing/blocking + cursor confinement are now fully removed, so the mouse is always live). So the
    // DEFAULT product path must never suppress the user's input on native Windows. The EXPLICIT probe
    // opt-ins above (sq_repro, ER_EFFECTS_BLOCK_INPUT env/file, inject_nav) are checked first and still
    // engage the keyboard/gamepad block when a real probe wants it, on native Windows or Wine.
    if is_native_windows() {
        return false;
    }
    let autoload_armed = own_stepper_enabled()
        || own_load_enabled()
        || product_autoload_enabled()
        || native_continue_enabled()
        || pab_advance_enabled();
    if !autoload_armed || own_stepper_passive_enabled() || autoload_load_started() {
        return false;
    }
    // PASSIVE mode never blocks. Otherwise keep the block engaged through the zero-input title/menu
    // drive and release as soon as the current load-start semaphore proves the engine committed the
    // load (native LoadingScreen update/bar, FakeLoadingScreen cover visibility, or GameMan.save_state
    // leaving idle). At that point our zero-input side is done; the user need not wait for full in-world
    // streaming before keyboard/gamepad input is live again. Product autoload still keeps blocking after
    // the guarded SetState5 only until that load-start proof appears.
    let product_world_stream_pending = product_autoload_enabled()
        && OWN_STEPPER_CONFIRMED.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS
        && IN_WORLD_REACHED.load(Ordering::SeqCst) != IN_WORLD_REACHED_YES;
    // ZERO-INPUT INVARIANT (always-block-input-zero-input-invariant-2026-06-22, extended
    // 2026-06-24 user-directive "block input until the load has started -- our side is done"):
    // block ALL foreign input whenever ANY automated load lever is armed until load start, so no probe
    // can be contaminated and no path can secretly rely on input before the engine commits the load.
    // This now INCLUDES the DEFAULT zero-input autoload path (native_continue + the readiness PAB
    // advance), which is on for every real (non-telemetry-only) run -- previously only
    // own_stepper/own_load/product_autoload engaged the block, so the default path ran with input LIVE
    // and a human Continue press could (and did, 2026-06-24 gold-load run) drive the load instead of our
    // DLL, masking that native_continue never found the Continue node. Blocking the default path makes
    // the zero-input claim honest: if our drive cannot fire the load with input suppressed, the run
    // stalls (correct failure) rather than riding on a foreign press. Normal play and user-driven golden
    // traces (no lever armed, or telemetry-only) never block; the load-start release lets the user take
    // over once the committed load no longer needs protected title/menu input.
    IN_WORLD_REACHED.load(Ordering::SeqCst) != IN_WORLD_REACHED_YES
        && (OWN_STEPPER_PHASE.load(Ordering::SeqCst) != OWN_STEPPER_PHASE_DONE
            || product_world_stream_pending
            // The default native_continue/pab path does not drive the own_stepper phase machine, so
            // its phase stays 0 (!= DONE) -- keep it blocked until load-start regardless.
            || native_continue_enabled()
            || pab_advance_enabled())
}

/// Release the input block (DInput + XInput) once `block_input_enabled()` flips false mid-run.
/// The hooks stay installed but pass input through when `BLOCK_INPUT_ACTIVE` is clear; the
/// DInput blocker also needs its own flags cleared. Acts once on the ON->off transition.
pub(crate) fn release_input_block_now() {
    if BLOCK_INPUT_ACTIVE.swap(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst) == BLOCK_INPUT_ON {
        InputBlocker::get_instance().block_only(InputFlags::empty());
        append_autoload_debug(format_args!(
            "input-block: RELEASED (load-start / in-world / abort) -- keyboard + gamepad live (mouse + cursor never touched)"
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
    // sThumbLY@6. Used by the can-move probe lane to walk the character forward and measure motion,
    // and by the picker's nav latch to read stick-driven menu navigation.
    const XINPUT_THUMB_LX_OFFSET_IN_GAMEPAD: usize = 4;
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
    // PASSIVE INPUT-TRACE CAPTURE (er-effects-input-trace.txt) + product picker D-pad capture:
    // record the REAL slot-0 pad state exactly as the original returned it, BEFORE the
    // keepalive/fabrication branches below can overwrite the caller's buffer. This never mutates
    // `state` or `hr`, so pass-through/block behavior stays byte-identical.
    if user_index == XINPUT_PRIMARY_USER_INDEX && hr == XINPUT_SUCCESS && !state.is_null() {
        let buttons = unsafe {
            *(state.add(XINPUT_GAMEPAD_OFFSET + WBUTTONS_OFFSET_IN_GAMEPAD) as *const u16)
        };
        let thumb_lx = unsafe {
            *(state.add(XINPUT_GAMEPAD_OFFSET + XINPUT_THUMB_LX_OFFSET_IN_GAMEPAD) as *const i16)
        };
        let thumb_ly = unsafe {
            *(state.add(XINPUT_GAMEPAD_OFFSET + XINPUT_THUMB_LY_OFFSET_IN_GAMEPAD) as *const i16)
        };
        save_picker_latch_xinput_nav_state(buttons, thumb_lx, thumb_ly);
        input_trace_record_real_poll(state as *const u8);
    } else if user_index == XINPUT_PRIMARY_USER_INDEX {
        save_picker_latch_xinput_nav_state(0, 0, 0);
    }
    // KEEP SLOT 0 "CONNECTED" while the harness is ACTIVELY INJECTING (only) -- when no physical pad
    // exists, present a connected idle pad with a fresh packet so ER keeps polling slot 0 and the
    // fabrication below can land. Gated STRICTLY to `harness_injection_active()` (the move-probe ON burst
    // / sq-repro driving), NOT to the whole run (bd input-blocking-only-in-harness-during-driving-never-in-
    // product-never-outside-window-2026-07-23). MOUSE-ATTACK FIX: this used to be gated on
    // `prove_movement_enabled()` (== harness DLL present), so a phantom "connected" pad with a FRESH packet
    // EVERY poll was presented for the ENTIRE run, including the in-world dwell. ER's active-input-device
    // arbitration then treated that constantly-changing phantom gamepad as the active device, so the user's
    // mouse-CLICK attacks were routed to the (idle) gamepad and ignored -- while mouse-LOOK (camera, read
    // straight off the mouse delta) still worked. Outside the injection window we now let slot 0 read its
    // real DEVICE_NOT_CONNECTED, so ER keeps mouse+keyboard as the active device and mouse-click attacks
    // work throughout the dwell. Never runs on the default/product-without-harness path.
    if user_index == XINPUT_PRIMARY_USER_INDEX
        && hr == XINPUT_ERROR_DEVICE_NOT_CONNECTED
        && !state.is_null()
        && harness_injection_active()
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
        let proc = unsafe { GetProcAddress(hmod, s!("XInputGetState")) };
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
                    crate::mh::leak_installed_hook(hook);
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
            if ex_addr != addr
                && let Ok(hook) = unsafe {
                    MhHook::new(ex_addr as *mut c_void, xinput_get_state_hook as *mut c_void)
                }
            {
                let _ = unsafe { hook.queue_enable() };
                crate::mh::leak_installed_hook(hook);
                append_autoload_debug(format_args!(
                    "xinput-block: hooked XInputGetStateEx(ord 100) at 0x{ex_addr:x}"
                ));
            }
        }
        // XInputGetCapabilities is the slot-ENUMERATION call the game uses to decide which pads to
        // poll. Hook it so a harness-armed run can keep slot 0 "connected" with no physical
        // controller (see xinput_get_capabilities_hook). Same DLL, resolved by name.
        let caps = unsafe { GetProcAddress(hmod, s!("XInputGetCapabilities")) };
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
                    crate::mh::leak_installed_hook(hook);
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
/// Deliberately does NOT install the DInput keyboard hook, and never blocks the mouse or cursor.
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

/// LEGACY telemetry field, retained for the emitted oracle schema (write_oracle reads it). It is NO
/// LONGER incremented: the RawInput hook used to DROP the user's keyboard for the whole agent-owned run
/// (the camera-only-control bug), but that drop was removed (bd input-blocking-only-in-harness-during-
/// driving-never-in-product-never-outside-window-2026-07-23) -- the user's keyboard is never dropped, so
/// this stays 0. MOUSE events were never dropped either.
pub(crate) use er_telemetry::counters::RAWINPUT_BLOCKED_UNFOCUSED_EVENTS;
/// Total GetRawInputData calls the game made (any command). If this is 0 the game is NOT routing input
/// through GetRawInputData -> the reception oracle is BLIND and a 0 event count means nothing. If >0 the
/// oracle is live and a 0 event count is a genuine "no user input this run".
pub(crate) use er_telemetry::counters::RAWINPUT_HOOK_CALLS;
pub(crate) use er_telemetry::counters::RAWINPUT_KEY_EVENTS;
pub(crate) use er_telemetry::counters::RAWINPUT_MOUSE_BUTTON_EVENTS;
/// GetRawInputData reception counters (user 2026-07-20): the oracle must RECORD whether the GAME is
/// RECEIVING user mouse/keyboard input, at the OS boundary. ER reads gameplay+menu input via RawInput;
/// the input-harness injects via the direct-memory inputmgr, NOT RawInput -- so every RawInput event
/// counted here is USER input the game received (contamination during an agent-owned run). Emitted as
/// oracle_rawinput_* and consumed by the verdict emitter.
pub(crate) use er_telemetry::counters::RAWINPUT_MOUSE_MOVE_EVENTS;
static GET_RAW_INPUT_DATA_ORIG: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

// REMOVED (bd input-blocking-only-in-harness-during-driving-never-in-product-never-outside-window-
// 2026-07-23): `harness_run_active()` (harness DLL present, whole run) was the gate for the whole-run
// RawInput keyboard DROP. That drop was removed (recording-only hook), so this predicate is gone. The
// active-injection window is now expressed by `harness_injection_active()`, which is TRUE only while the
// harness is actually injecting THIS frame -- never for the whole run.

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
    RAWINPUT_HOOK_CALLS.fetch_add(1, Ordering::Relaxed);
    let orig_addr = GET_RAW_INPUT_DATA_ORIG.load(Ordering::SeqCst);
    let orig: unsafe extern "system" fn(isize, u32, *mut c_void, *mut u32, u32) -> u32 =
        unsafe { std::mem::transmute(orig_addr) };
    let ret = unsafe { orig(h_raw_input, ui_command, p_data, pcb_size, cb_size_header) };
    const RID_INPUT: u32 = 0x1000_0003;
    if !p_data.is_null() && ui_command == RID_INPUT && ret != u32::MAX && ret >= 0x30 {
        // RECORDING ONLY (bd input-blocking-only-in-harness-during-driving-never-in-product-never-outside-
        // window-2026-07-23): this hook NEVER drops the user's input. It used to zero (drop) every user
        // KEYBOARD RawInput event for the WHOLE agent-owned run (whenever the harness DLL was loaded), which
        // killed W-move + Escape-menu for the entire in-world dwell -- the camera-only-control bug.
        // Disabling the user's input is valid ONLY inside the harness during its active driving window, never
        // in the product. And a keyboard DROP fundamentally cannot live here anyway: the can-move probe
        // injects 'W' via SendInput -> RawInput -> this SAME hook, and RawInput carries no injected-vs-user
        // flag, so dropping keyboard here would also drop the harness's own injected key and break movement
        // injection. So classify + count only (contamination oracle) and pass every event through untouched.
        // The MOUSE was never dropped either. Any user contamination of the movement proof is DETECTED (not
        // blocked) by the can-move probe's OFF-tail verdict.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let base = p_data as usize;
            let dwtype = unsafe { (base as *const u32).read_unaligned() };
            let d = base + 0x18; // past RAWINPUTHEADER
            if dwtype == 0 {
                // RIM_TYPEMOUSE: usButtonFlags @ +0x04, lLastX @ +0x0C, lLastY @ +0x10. The MOUSE is
                // NEVER dropped -- the user's real mouse (movement, buttons, camera) is always live player
                // input. Recording only; the event passes through untouched.
                let btn = unsafe { ((d + 0x04) as *const u16).read_unaligned() };
                let lx = unsafe { ((d + 0x0C) as *const i32).read_unaligned() };
                let ly = unsafe { ((d + 0x10) as *const i32).read_unaligned() };
                if lx != 0 || ly != 0 {
                    RAWINPUT_MOUSE_MOVE_EVENTS.fetch_add(1, Ordering::Relaxed);
                }
                if btn != 0 {
                    RAWINPUT_MOUSE_BUTTON_EVENTS.fetch_add(1, Ordering::Relaxed);
                }
                // RI_MOUSE_WHEEL: usButtonData @ +0x06 carries the SIGNED notch delta for this
                // event. Latched as a nav edge so the wheel reaches the picker through the same
                // path as the arrow keys and obeys the same edge/limit rules, instead of being a
                // second, differently-behaved way to move the list.
                if btn & RI_MOUSE_WHEEL != 0 {
                    let delta = unsafe { ((d + 0x06) as *const i16).read_unaligned() };
                    save_picker_latch_wheel_nav_for_message(h_raw_input, delta);
                }
            } else if dwtype == 1 {
                // RIM_TYPEKEYBOARD: MakeCode @ +0x00, VKey @ +0x06, Message @ +0x08 (WM_KEYDOWN 0x100).
                // Count the user's key as received (contamination oracle) and pass it through untouched.
                let msg = unsafe { ((d + 0x08) as *const u32).read_unaligned() };
                if msg == 0x100 || msg == 0x104 {
                    RAWINPUT_KEY_EVENTS.fetch_add(1, Ordering::Relaxed);
                    let vkey = unsafe { ((d + 0x06) as *const u16).read_unaligned() };
                    save_picker_latch_keyboard_nav(vkey);
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
    let hmod = match unsafe { GetModuleHandleA(s!("user32.dll")) } {
        Ok(h) if !h.is_invalid() => h,
        _ => return,
    };
    let Some(addr) = (unsafe { GetProcAddress(hmod, s!("GetRawInputData")) }) else {
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
                    crate::mh::leak_installed_hook(hook);
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
pub(crate) use er_telemetry::counters::DINPUT_BLOCK_INSTALLED;
pub(crate) use er_telemetry::counters::MISSING_SAVE_INPUT_RELEASE_LOGGED;

/// Enforce the comprehensive input block for this frame. Self-contained (no args) so it can
/// run from EITHER the game task OR the render loop -- critical because under the offline
/// launcher no render callback executes at the title, so the render-loop call
/// alone never engaged the block (that was the contamination hole). Driven every frame from
/// the game task while `block_input_enabled()`:
///   1. ONCE: install the DInput8 keyboard `GetDeviceState` block (panics on probe
///      failure -> contained with catch_unwind so the FD4 task never unwinds into C++).
///   2. EVERY frame: assert the block flag (sticky, overriding any overlay want-capture
///      clear) and install/retry the XInput gamepad hook until the xinput DLL is present.
///
/// Genuinely zero-input: it only SUPPRESSES keyboard + gamepad device reads -- it never synthesizes any
/// input, never blocks the mouse, and never confines the cursor.
pub(crate) fn enforce_input_block_now() {
    let blocker = InputBlocker::get_instance();
    if missing_save_selection_pending() {
        BLOCK_INPUT_ACTIVE.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
        blocker.block_only(InputFlags::empty());
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
    // MOUSE + CURSOR NEVER TOUCHED (user 2026-07-22 + 2026-07-23, bd input-block-1x1-clipcursor-traps-
    // user-native-windows-no-failsafe-release): there is NO cursor confinement here -- the per-frame 1x1
    // `ClipCursor` that used to trap the OS cursor was removed, and the DInput MOUSE `GetDeviceState` block
    // was removed too, so the user's real mouse (movement, buttons, camera) is always live player input.
    // The DInput/XInput zeroing above suppresses ONLY keyboard + gamepad; it never forces, confines, or
    // blocks the mouse.
}

// REMOVED (bd input-blocking-only-in-harness-during-driving-never-in-product-never-outside-window-
// 2026-07-23): `enforce_keyboard_game_input_disable()` zeroed the user's DInput keyboard for the WHOLE
// in-world dwell (called every frame the harness DLL was present + the player was in-world). Disabling the
// user's input is only valid inside the harness during its active driving window, never in the product
// during normal play, so this whole-in-world keyboard disable is gone. The DInput keyboard block is now
// driven ONLY by enforce_input_block_now()/release_input_block_now() (boot/reload driving windows), which
// release the block on world entry -- so the dwell keeps full keyboard control.

#[cfg(test)]
mod save_picker_stick_nav_tests {
    use super::*;

    const AT_REST: i16 = 0;
    /// Inside XInput's 7849 rest deadzone: what a worn stick reports when nobody is touching it.
    const DRIFT: i16 = 6000;
    const FLICK: i16 = 20000;

    #[test]
    fn stick_drift_never_navigates() {
        assert_eq!(xinput_stick_nav_mask(0, DRIFT, DRIFT), 0);
        assert_eq!(xinput_stick_nav_mask(0, -DRIFT, -DRIFT), 0);
    }

    /// A deliberate push down registers, and -- the point of the two thresholds -- a stick RELAXING
    /// through the press threshold does not re-register. One push must be one row, not a burst.
    #[test]
    fn one_push_is_one_edge_even_while_the_stick_relaxes() {
        let pushed = xinput_stick_nav_mask(0, AT_REST, -FLICK);
        assert_eq!(pushed, SAVE_PICKER_NAV_DOWN_MASK);

        // Still held past the release threshold: stays down, so `down & !prev` yields no new edge.
        let held = xinput_stick_nav_mask(pushed, AT_REST, -12000);
        assert_eq!(held, SAVE_PICKER_NAV_DOWN_MASK);
        assert_eq!(held & !pushed, 0, "a held stick must not re-edge");

        // Below release: clears, so the next push is a fresh edge.
        let released = xinput_stick_nav_mask(held, AT_REST, -4000);
        assert_eq!(released, 0);
        assert_eq!(
            xinput_stick_nav_mask(released, AT_REST, -FLICK),
            SAVE_PICKER_NAV_DOWN_MASK
        );
    }

    /// XInput reports +Y as UP while list rows count downward; a sign slip here would scroll the
    /// list the wrong way on a pad and be invisible on a keyboard.
    #[test]
    fn stick_axes_map_to_the_directions_the_list_uses() {
        assert_eq!(
            xinput_stick_nav_mask(0, AT_REST, FLICK),
            SAVE_PICKER_NAV_UP_MASK
        );
        assert_eq!(
            xinput_stick_nav_mask(0, AT_REST, -FLICK),
            SAVE_PICKER_NAV_DOWN_MASK
        );
        assert_eq!(
            xinput_stick_nav_mask(0, -FLICK, AT_REST),
            SAVE_PICKER_NAV_LEFT_MASK
        );
        assert_eq!(
            xinput_stick_nav_mask(0, FLICK, AT_REST),
            SAVE_PICKER_NAV_RIGHT_MASK
        );
    }
}

#[cfg(test)]
mod save_picker_wheel_nav_tests {
    use super::*;

    /// Wheel detents must latch their OWN masks, never the key/pad direction masks.
    ///
    /// The distinction is behavioural, not cosmetic: the picker rides the native list's wrap as the
    /// step signal for a key or pad press at an extreme row, but the native list does not wrap for
    /// the wheel. A wheel edge wearing a key mask therefore waits for a wrap that never comes, which
    /// is exactly how the wheel came to do nothing at the top and bottom rows.
    #[test]
    fn a_detent_yields_the_wheel_mask_not_the_key_mask() {
        let (down, _) = wheel_nav_step(0, -(WHEEL_DELTA as i16));
        assert_eq!(down, SAVE_PICKER_NAV_WHEEL_DOWN_MASK);
        assert_eq!(down & SAVE_PICKER_NAV_DOWN_MASK, 0);

        let (up, _) = wheel_nav_step(0, WHEEL_DELTA as i16);
        assert_eq!(up, SAVE_PICKER_NAV_WHEEL_UP_MASK);
        assert_eq!(up & SAVE_PICKER_NAV_UP_MASK, 0);
    }

    /// Sub-detent motion accumulates into whole rows rather than being dropped or flooding the list:
    /// a high-resolution wheel reports many small deltas for one physical notch.
    #[test]
    fn partial_detents_accumulate_into_exactly_one_row() {
        let third = (WHEEL_DELTA / 3) as i16;
        let (first, accum) = wheel_nav_step(0, third);
        assert_eq!(first, 0, "a third of a detent is not a row");
        let (second, accum) = wheel_nav_step(accum, third);
        assert_eq!(second, 0, "two thirds of a detent is not a row");
        let (third_step, accum) = wheel_nav_step(accum, third);
        assert_eq!(third_step, SAVE_PICKER_NAV_WHEEL_UP_MASK);
        assert_eq!(accum, 0, "a completed detent leaves no remainder");
    }

    /// One spin must not be spent twice: the remainder carried forward is strictly sub-detent.
    #[test]
    fn a_completed_detent_is_not_left_in_the_accumulator() {
        let (mask, accum) = wheel_nav_step(0, (WHEEL_DELTA + WHEEL_DELTA / 2) as i16);
        assert_eq!(mask, SAVE_PICKER_NAV_WHEEL_UP_MASK);
        assert_eq!(accum, (WHEEL_DELTA / 2) as isize);
        assert!(accum.abs() < WHEEL_DELTA as isize);
    }
}

#[cfg(test)]
mod save_picker_wheel_message_tests {
    use super::*;

    /// A repeat READ of the same WM_INPUT message is not a second detent.
    ///
    /// `GetRawInputData` can be called more than once for one message, so counting reads rather than
    /// messages turns one notch of the wheel into several rows of movement. Verified through the
    /// dedupe state directly: the first read of a message latches, an immediate repeat does not, and
    /// a genuinely different message latches again.
    #[test]
    fn a_repeat_read_of_one_message_is_not_a_second_detent() {
        const FIRST: isize = 0x1111;
        const SECOND: isize = 0x2222;
        let detent = WHEEL_DELTA as i16;

        SAVE_PICKER_WHEEL_LAST_MESSAGE.store(0, Ordering::SeqCst);
        let before = SAVE_PICKER_WHEEL_DUPLICATE_READS.load(Ordering::SeqCst);

        save_picker_latch_wheel_nav_for_message(FIRST, detent);
        save_picker_latch_wheel_nav_for_message(FIRST, detent);
        assert_eq!(
            SAVE_PICKER_WHEEL_DUPLICATE_READS.load(Ordering::SeqCst),
            before + 1,
            "the repeat read must be counted and dropped"
        );

        save_picker_latch_wheel_nav_for_message(SECOND, detent);
        assert_eq!(
            SAVE_PICKER_WHEEL_DUPLICATE_READS.load(Ordering::SeqCst),
            before + 1,
            "a different message is a real detent, not a duplicate"
        );
    }
}
