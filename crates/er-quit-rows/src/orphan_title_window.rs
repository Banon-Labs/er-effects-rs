//! When a title menu window has outlived the title, as a pure decision.
//!
//! Its own module, and outside the `#[cfg(windows)]` half of this crate, so the decision runs under
//! `cargo test -p er-quit-rows` on the host -- the same reason `menu_window_run_install` sits here.
//!
//! # The defect
//!
//! `System>Quit -> Load Character` returns to the title on purpose and loads from there. Measured in
//! `er-quit-rows-debug.log`, 2026-09-11 17:34, build `65dce10a`, on the `ProfileSelectSlotActivate`
//! switch, four log lines in order:
//!
//! ```text
//! +38919ms  WORLD LOST #1: c30 0x1c000000 -> 0xa010000     the outgoing world reverts to the title
//! +38924ms  AcquireMenuResource ... filename='05_000_Title'  the title screen is genuinely rebuilt
//! +39246ms  own-load-feed: ... c30 0xa010000->0x1c000000   323 ms later the switch mounts its slot
//! +39247ms  own-load-continue: COMMIT continue_confirm     and hands the world off
//! ```
//!
//! The world streams and `T_controllable` lands at `+40473ms` -- and the title's `PRESS ANY BUTTON`
//! prompt and its publisher footer are still drawn over it, permanently.
//!
//! The same log carries the negative case. The second switch, `Load Character from File` at
//! `+169567ms`, never lost its world (`c30` holds `0x1c000000` until the feed moves it straight to
//! `0xe000000`), never acquired `05_000_Title`, and has no orphan. So the defect is not a property
//! of the row: it is a property of whether the outgoing teardown reached the title before the
//! switch committed, which is a race the switch does not control.
//!
//! # Who should have torn the window down
//!
//! `CS::TitleStep` is a twelve-state machine whose table is built at `FUN_1400a4f50` into
//! `0x143d71580` (`INNER_TITLE_STATE_TABLE_RVA`), and `STEP_BeginTitle` (index 3, `0x140b0c5b0`)
//! composes the title's `MenuJob` chain -- `FUN_14081f9f0` builds the `05_000_Title` job -- and
//! submits it through `FUN_140b0e530`, which assigns it into `TitleStep+0x130` and requests state
//! 10, `STEP_MenuJobWait`. That step (`0x140b0d400`) does one thing that matters here: it calls
//! `ExecuteMenuJob(&TitleStep->field85_0x130, dt)`.
//!
//! Two native functions reap the title, and both are gated on a job reporting a terminal result:
//!
//! * `ExecuteMenuJob` (`0x1407a9600`) runs the job, then `if (MenuJobResult::ShouldContinue(&r))`
//!   unrefs it and writes null back over `TitleStep+0x130`. `ShouldContinue` is `0x1407a9200`, three
//!   instructions -- `CMP dword ptr [RCX],0x1; SETA AL; RET` -- so it is `state > Continue(1)`, i.e.
//!   the predicate is "has a result", not "wants another tick".
//! * `CS::MenuWindowJob::Run` (`0x1407ad1c0`) ends by reading its own window's result at
//!   `MenuWindow+0x1e8`, and on the same `ShouldContinue` calls `FUN_1407ada40` -- the teardown that
//!   deregisters the window from `CSMenuMan` -- and propagates the terminal result to its caller.
//!
//! # Why neither runs on a switch
//!
//! `continue_confirm` (`0x140b0e180`) ends with `FUN_140b0d960(titleStep, 5)`, which writes
//! `FD4StepTemplateBase.requestedState = 5` (`STEP_PlayGame`). The step machine leaves
//! `STEP_MenuJobWait` on the next tick, so `ExecuteMenuJob` is never called against that job again,
//! and `STEP_PlayGame` / `STEP_GameStepWait` never touch `+0x130`. The only other writer of that
//! slot is `FUN_140b0e530`, reachable only from the four title-entry steps, none of which runs again
//! until the next return to title. Meanwhile the window's `MenuWindowJob` is still in `CSMenuMan`'s
//! own pump -- the detour at `MENU_WINDOW_JOB_RUN_RVA` shows it ticking for the rest of the session --
//! so it keeps drawing.
//!
//! On the vanilla title the same `continue_confirm` runs, but it runs as the `TitleTopDialog`
//! Continue row's functor from inside the job chain: the dialog job returns its terminal result on
//! that same tick and the two reapers above fire before the step change takes effect. Our call comes
//! from outside the chain, so nothing ever terminates.
//!
//! # What this crate may do about it
//!
//! Ask the window to close, through the game's own per-window close: `FUN_1407ac890(MenuWindow*)`
//! builds a `Failed` `MenuJobResult` and invokes the window's own virtual at `vtable+0x60`. It is
//! the identical call `CS::MenuWindowJob::Run` makes when its close policy at `job+0xf0` returns
//! that verdict, so every step after it -- the window setting its result, `FUN_1407ada40`
//! deregistering it, the chain reaping, `ExecuteMenuJob` nulling `TitleStep+0x130` -- is the game's
//! own code in the game's own order on the menu-pump thread.
//!
//! The rejected alternative is recorded here because it is the obvious one and it is already known
//! to crash: overwriting `TitleStep+0x130` with a fresh job "orphaned the title IfElseJob's sibling
//! `CS::MenuWindowJob`s -> AV at `CS::DLFixedVector::push_back 0x140733fea`"
//! (`experiments::own_load::loaders::load_drive`, the `PushBackJob` comment). Killing the parent
//! does not deregister the children; asking each window to close does.

/// Resource names of the title surfaces this crate will ask to close.
///
/// These are the game's own `MenuWindowJob` filenames, read from `job+0x60`, not names of ours.
/// `05_000_Title` hosts `PressStart` / `StaticSystemText_101000` -- the `PRESS ANY BUTTON` prompt --
/// and `05_020_TitleInformation` is the publisher and copyright footer under it. `05_001_Title_Logo`
/// is listed with them because `AcquireMenuResource` fetches it one millisecond after `05_000_Title`
/// on both the boot title and the rebuilt one, so it arrives and departs with them.
///
/// The list is exhaustive on purpose. Every other window the pump offers -- a message box, the
/// in-world menus, the `05_010_ProfileSelect` rows -- belongs to someone, and this crate closing one
/// would be answering for them.
pub const TITLE_SURFACE_RESOURCE_NAMES: &[&str] = &[
    "05_000_Title",
    "05_001_Title_Logo",
    "05_020_TitleInformation",
];

/// Close requests spent per switch before the gate gives up.
///
/// A request is one call to the native per-window close. The window answers it by setting its own
/// result, which takes at least the next pump tick to be read, so a small budget covers the windows
/// present (three at most) plus a few frames of latency. When the budget is gone the gate stops
/// asking and the orphan stays, which is today's behaviour -- a gate that could ask forever would
/// turn a cosmetic defect into a per-frame call into menu code, which is worse.
pub const MAX_CLOSE_REQUESTS_PER_SWITCH: usize = 16;

/// The map id `GameMan+0xc30` holds when no real world is mounted.
///
/// Named again here rather than imported because this module is the host-testable half of the crate
/// and must not reach into the `#[cfg(windows)]` constants. `FULLREAD_C30_M10_DEFAULT` in
/// `constants::autoload_state` is the same number and is the one the runtime path reads.
pub const C30_TITLE_DEFAULT: i32 = 0xa01_0000;

/// Whether the pump is looking at one of the title's own surfaces.
#[must_use]
pub fn is_title_surface(resource: &str) -> bool {
    TITLE_SURFACE_RESOURCE_NAMES.contains(&resource)
}

/// Whether this crate should ask the native owner to close the window running under `resource`.
///
/// Three independent conditions, each of which alone would be wrong:
///
/// * `resource` is a title surface. Anything else belongs to the player.
/// * `c30` names a real map. During the switch's own return to title `c30` is `C30_TITLE_DEFAULT`
///   and the title is legitimately on screen -- the switch is waiting on it. Closing there would
///   break the very flow this fixes. This is a live read of `GameMan+0xc30`, not a latch, so a
///   future return to title re-protects the title automatically.
/// * `switch_committed` -- `SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE`, the durable "a switch
///   reload committed" latch, re-armed to 0 for the next switch. Without it a title surface drawn
///   during any other real-world moment would be closed by a crate that had not put it there.
///
/// The budget is last so an exhausted one reads as a refusal rather than as the window not being an
/// orphan.
#[must_use]
pub fn orphan_title_window_close_required(
    resource: &str,
    c30: i32,
    switch_committed: bool,
    close_requests_spent: usize,
) -> bool {
    is_title_surface(resource)
        && c30 != C30_TITLE_DEFAULT
        && switch_committed
        && close_requests_spent < MAX_CLOSE_REQUESTS_PER_SWITCH
}

#[cfg(test)]
mod orphan_title_window_tests {
    use super::{
        C30_TITLE_DEFAULT, MAX_CLOSE_REQUESTS_PER_SWITCH, is_title_surface,
        orphan_title_window_close_required,
    };

    /// A real map id from the log this module documents: the `angrE` switch mounted `0x1c000000`.
    const C30_REAL_MAP: i32 = 0x1c00_0000;

    /// The defect itself: `05_000_Title` still pumping after the switch put a real world up.
    #[test]
    fn the_press_any_button_window_is_closed_once_the_switch_has_a_world() {
        assert!(orphan_title_window_close_required(
            "05_000_Title",
            C30_REAL_MAP,
            true,
            0
        ));
    }

    /// The footer arrives with the prompt and has to leave with it, so it is named separately.
    #[test]
    fn the_publisher_footer_is_closed_on_the_same_terms() {
        assert!(orphan_title_window_close_required(
            "05_020_TitleInformation",
            C30_REAL_MAP,
            true,
            0
        ));
    }

    /// The window the switch is waiting for. Between `WORLD LOST` and the feed, `c30` is the title
    /// default and the title is doing its job; closing it here would break the switch.
    #[test]
    fn the_title_is_left_alone_while_the_switch_is_still_returning_to_it() {
        assert!(!orphan_title_window_close_required(
            "05_000_Title",
            C30_TITLE_DEFAULT,
            true,
            0
        ));
    }

    /// A title surface on screen in a real world that this crate did not switch into is not ours to
    /// close.
    #[test]
    fn a_title_surface_outside_a_committed_switch_is_not_touched() {
        assert!(!orphan_title_window_close_required(
            "05_000_Title",
            C30_REAL_MAP,
            false,
            0
        ));
    }

    /// The rule that keeps this from becoming a dialog dismisser. Every window the menu pump offers
    /// that is not a title surface is refused, including the ones this crate builds itself.
    #[test]
    fn no_other_window_the_pump_offers_is_ever_closed() {
        for resource in [
            "02_000_IngameTop",
            "02_040_OptionSetting",
            "05_010_ProfileSelect",
            "02_990_textinput_patheditor",
            "01_900_Black",
            "",
        ] {
            assert!(
                !is_title_surface(resource),
                "{resource} is not a title surface"
            );
            assert!(
                !orphan_title_window_close_required(resource, C30_REAL_MAP, true, 0),
                "{resource} must never be closed by this crate"
            );
        }
    }

    /// An exhausted budget refuses, so a window that never answers cannot be asked every frame.
    #[test]
    fn the_budget_stops_the_gate_asking_forever() {
        assert!(!orphan_title_window_close_required(
            "05_000_Title",
            C30_REAL_MAP,
            true,
            MAX_CLOSE_REQUESTS_PER_SWITCH
        ));
        assert!(orphan_title_window_close_required(
            "05_000_Title",
            C30_REAL_MAP,
            true,
            MAX_CLOSE_REQUESTS_PER_SWITCH - 1
        ));
    }
}
