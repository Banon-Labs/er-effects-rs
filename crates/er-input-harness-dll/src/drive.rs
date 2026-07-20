//! Self-drive state machine, run once per game frame from the union-anchored hook (union.rs). It
//! exercises the VERIFIED direct-input-memory primitives (input_inject) and is honest about the two
//! parts of the System->Quit->Load-Profile path that are NOT reversed as menu-event ids.
//!
//! DEFAULT-ON, NO ENV/MARKER GATE: the drive is active whenever this DLL is in the ME3 profile; the
//! only conditions are real game-state reads (player present, a menu window present). Its mere
//! PRESENCE enables it; omit the DLL from the profile for production. There is no environment-variable
//! read, no marker text file, and no product static -- state is re-derived from game memory (game_mem).
//!
//! COVERAGE (honest):
//!  * REVERSED + driven here: keep-input-active (unfocused), and menu cursor nav (Move 0x00/0x45) +
//!    Confirm (0x3d) via the keystate bitmap -- the input ids proven in
//!    `frontend-menu-input-injection-ids-2026`.
//!  * NOT reversed -> logged as gaps, never faked:
//!      - the in-world ESCAPE-MENU OPEN event id (no verified menu-event id; the old harness used
//!        SendInput Esc, now a dead path),
//!      - the OptionSetting -> Quit TAB-SWITCH (mouse-only on native; the product works around it by
//!        invoking a CAPTURED NATIVE ROUTE object from its own ProfileSelect hooks -- product-side,
//!        not a menu-event id and not available cross-DLL).

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::game_mem::{menu_data_ptr, player_present};
use crate::input_inject::{MenuEvent, input_manager, keep_input_active, tap_menu_event};
use crate::log::harness_log;

// Menu tap cadence (product `MenuTapSchedule`): assert the edge for SET frames, then GAP idle frames,
// one clean cursor step per cycle (edge-triggered nav, no auto-repeat).
const TAP_SET_FRAMES: u64 = 2;
const TAP_GAP_FRAMES: u64 = 10;
const TAP_CYCLE_FRAMES: u64 = TAP_SET_FRAMES + TAP_GAP_FRAMES;
/// Bounded Down taps before Confirm (menus are short; Down saturates without wrap so overshoot is
/// bounded too). Exercises the reversed nav primitive; a cursor-feedback stop needs the per-menu list
/// object pointer, which is not resolved generically here.
const NAV_MAX_TAPS: u64 = 4;
const CONFIRM_HOLD_FRAMES: u64 = TAP_SET_FRAMES;

const STATE_WAIT_MENU: usize = 0;
const STATE_NAV: usize = 1;
const STATE_CONFIRM: usize = 2;
const STATE_TAB_GAP_HALT: usize = 3;
const STATE_DONE: usize = 4;

static STATE: AtomicUsize = AtomicUsize::new(STATE_WAIT_MENU);
static PHASE_FRAME: AtomicUsize = AtomicUsize::new(0);
static OPEN_GAP_LOGGED: AtomicUsize = AtomicUsize::new(0);
static TAB_GAP_LOGGED: AtomicUsize = AtomicUsize::new(0);

fn set_state(next: usize) {
    STATE.store(next, Ordering::SeqCst);
    PHASE_FRAME.store(0, Ordering::SeqCst);
}

/// Run one frame of the drive. Called on the game thread from the anchor detour.
pub fn on_frame(base: usize) {
    // STAY-ACTIVE every frame so injected input applies while the window is unfocused (the whole
    // reason the direct-memory channel needs no window focus).
    keep_input_active(base);

    let state = STATE.load(Ordering::SeqCst);
    if state == STATE_DONE {
        return;
    }
    let Some(im) = input_manager(base) else {
        return;
    };
    let frame = PHASE_FRAME.fetch_add(1, Ordering::SeqCst) as u64;
    let in_set_window = (frame % TAP_CYCLE_FRAMES) < TAP_SET_FRAMES;

    match state {
        STATE_WAIT_MENU => {
            // We can only navigate a menu that is already up: there is no reversed menu-event id to
            // OPEN the in-world escape menu (the old SendInput-Esc open is the dead path). Log that
            // gap once, then wait for a menu to appear (opened by the user or by the product path).
            if player_present() && OPEN_GAP_LOGGED.swap(1, Ordering::SeqCst) == 0 {
                harness_log!(
                    "drive: player present, no menu up. GAP: no reversed menu-event id to OPEN the in-world escape menu (SendInput-Esc was the dead path). Waiting for a menu window before driving reversed Move/Confirm."
                );
            }
            if menu_data_ptr() != 0 {
                harness_log!(
                    "drive: menu window present -> NAV (inject Down via keystate bitmap events 0x00+0x45, cadence {TAP_SET_FRAMES}/{TAP_GAP_FRAMES})"
                );
                set_state(STATE_NAV);
            }
        }
        STATE_NAV => {
            let taps_done = frame / TAP_CYCLE_FRAMES;
            if taps_done >= NAV_MAX_TAPS {
                harness_log!("drive: NAV issued {NAV_MAX_TAPS} Down taps -> CONFIRM (event 0x3d)");
                set_state(STATE_CONFIRM);
                return;
            }
            if in_set_window {
                // Inject BOTH vertical-move ids; only Down advances, Up saturates (harmless).
                tap_menu_event(im, MenuEvent::MoveA);
                tap_menu_event(im, MenuEvent::MoveB);
            }
        }
        STATE_CONFIRM => {
            if frame < CONFIRM_HOLD_FRAMES {
                tap_menu_event(im, MenuEvent::Confirm);
            } else {
                set_state(STATE_TAB_GAP_HALT);
            }
        }
        STATE_TAB_GAP_HALT => {
            // The OptionSetting -> Quit tab-switch has NO reversed menu-event id (mouse-only). The
            // product finishes by invoking a captured native route object from its own ProfileSelect
            // hooks -- product-side, not reachable from this DLL. Halt honestly here.
            if TAB_GAP_LOGGED.swap(1, Ordering::SeqCst) == 0 {
                harness_log!(
                    "drive: reached the OptionSetting->Quit TAB-SWITCH. GAP: no reversed menu-event id for the tab-switch (mouse-only on native). The product finishes via a captured native route object (ProfileSelect hooks, product-side). Standalone self-drive HALTS here -- NOT claiming an autonomous finish."
                );
            }
            set_state(STATE_DONE);
        }
        _ => {}
    }
}
