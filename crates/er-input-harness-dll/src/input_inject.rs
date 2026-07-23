//! DIRECT in-process input injection -- the VERIFIED ER input lever (user-confirmed 2026-07-19; the
//! SendInput/XInput/window-focus path was a DEAD END). No OS input is synthesized: the game's own
//! input memory is written on the game thread each frame.
//!
//! Two writes, both ported verbatim from the product with their exact reverse-engineered addresses:
//!
//!  1. MENU EVENTS -- the front-end/menu reads a KEYSTATE BITMAP at `inputmgr+0x90+eventId`, edge-
//!     triggered (`&1`). `inputmgr = *(base + 0x3d6b7b0)` (CSMenuMan / SelectBot input manager). Tap
//!     an event by OR-ing bit0 into `inputmgr+0x90+eventId`; the bitmap is re-polled every frame, so
//!     assert for a couple frames then gap for a clean single edge (no auto-repeat). Verified event
//!     ids (RE 2026-06-17, `frontend-menu-input-injection-ids-2026`): vertical-move = 0x00 AND 0x45
//!     (inject both; only Down advances, Up saturates), Confirm/OK = 0x3d. Mirrors the product's
//!     `menu_input_probe` (crates/er-effects-rs/src/experiments/continue_load/product_continue.rs).
//!
//!  2. STAY-ACTIVE (unfocused input) -- ER clears `[DLUID+0x88d]` every frame it is not
//!     `GetActiveWindow`; re-setting it to 1 lets the injected input apply while the window is
//!     UNFOCUSED (bd `breakthrough-pad-boundary-injection-moves-char-needs-focus`). `DLUID =
//!     *(base + 0x485dc18)` (input-device manager). This is why the direct path needs no window focus.
//!
//! Both writes are guarded by a fault-safe readability probe first, so a not-yet-initialized singleton
//! pointer can never fault the game thread.

use crate::log::harness_log;
use crate::win32::{read_u8, read_usize};

/// `inputmgr`/CSMenuMan singleton RVA (`SELECTBOT_INPUT_MANAGER_GLOBAL_RVA` /
/// `GLOBAL_CSMENUMAN_RVA` in the product constant tree).
const INPUT_MANAGER_GLOBAL_RVA: usize = 0x3d6b7b0;
/// Keystate bitmap base within the input manager (`INPUTMGR_BITMAP_90_OFFSET`).
const INPUTMGR_BITMAP_90_OFFSET: usize = 0x90;
/// Edge bit written per event (`MENU_EVENT_PRESSED_BIT`).
const MENU_EVENT_PRESSED_BIT: u8 = 1;

/// DLUID (input-device manager) singleton RVA (`RuntimeGlobalRva::DluidInputManager`).
const DLUID_SINGLETON_RVA: usize = 0x485dc18;
/// Input-active flag offset within DLUID (`DLUID_INPUT_ACTIVE_FLAG_OFFSET`).
const DLUID_INPUT_ACTIVE_FLAG_OFFSET: usize = 0x88d;

const HEAP_LO: usize = 0x10000;

/// Verified front-end/menu event ids (see module doc). No reversed id exists for the OptionSetting
/// tab-switch -- that is mouse-only on native and is the known self-drive gap.
#[derive(Clone, Copy)]
pub enum MenuEvent {
    /// One of the two vertical-move ids. Callers inject BOTH (`MoveA` and `MoveB`) for a "Down": only
    /// Down advances the cursor, Up saturates at the top so it is harmless.
    MoveA,
    MoveB,
    Confirm,
}

impl MenuEvent {
    const fn id(self) -> usize {
        match self {
            MenuEvent::MoveA => 0x00,
            MenuEvent::MoveB => 0x45,
            MenuEvent::Confirm => 0x3d,
        }
    }
}

/// Resolve the dereferenced input-manager pointer, or `None` before it is initialized.
pub fn input_manager(base: usize) -> Option<usize> {
    unsafe { read_usize(base + INPUT_MANAGER_GLOBAL_RVA) }.filter(|p| *p >= HEAP_LO)
}

/// Tap one menu event into the keystate bitmap (edge OR). Fault-safe: only writes once the target
/// byte is confirmed readable. Must be called on the game thread (from the per-frame drive hook) so
/// the write lands in the same frame the game re-polls the bitmap.
pub fn tap_menu_event(input_manager_ptr: usize, event: MenuEvent) {
    let addr = input_manager_ptr + INPUTMGR_BITMAP_90_OFFSET + event.id();
    if unsafe { read_u8(addr) }.is_none() {
        return;
    }
    // SAFETY: `addr` is a confirmed-readable byte inside the live input manager; OR-ing the edge bit
    // is exactly what the native input producer does at 0x1407ad509.
    unsafe {
        *(addr as *mut u8) |= MENU_EVENT_PRESSED_BIT;
    }
}

/// Re-set `[DLUID+0x88d] = 1` so injected input applies while the ER window is UNFOCUSED. Fault-safe;
/// call every frame from the drive hook (ER clears it each unfocused frame). Returns true once the
/// flag was written at least once (for logging).
pub fn keep_input_active(base: usize) -> bool {
    let Some(dluid) = (unsafe { read_usize(base + DLUID_SINGLETON_RVA) }).filter(|p| *p >= HEAP_LO)
    else {
        return false;
    };
    let flag = dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET;
    if unsafe { read_u8(flag) }.is_none() {
        return false;
    }
    // SAFETY: confirmed-readable flag byte inside the live DLUID singleton.
    unsafe {
        *(flag as *mut u8) = 1;
    }
    true
}

/// Log the resolved singletons once, for the evidence trail.
pub fn log_resolution(base: usize) {
    harness_log!(
        "input-inject: base=0x{base:x} input_manager=0x{:x} dluid_present={} (direct keystate-bitmap + DLUID stay-active channel; no SendInput/XInput)",
        input_manager(base).unwrap_or(0),
        (unsafe { read_usize(base + DLUID_SINGLETON_RVA) })
            .filter(|p| *p >= HEAP_LO)
            .is_some() as u8
    );
}
