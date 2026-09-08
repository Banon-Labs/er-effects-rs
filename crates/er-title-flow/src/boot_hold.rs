//! Host-safe boot hold decisions for the missing-save picker/title-flow seam.
//!
//! The runtime detours that enforce these decisions still live in the product DLL for now. Keeping
//! the predicates here gives standalone save-picker work a shared, testable title-flow contract
//! without importing product telemetry, hook installation, or runtime proof surfaces.

use crate::constants_moved::{TITLE_STEP_BEGIN_NEW_GAME, TITLE_STEP_PLAY_GAME};

/// `CS::ShowProgressJob` progress type for the boot save-data check/load.
///
/// This job owns the boot ProfileSummary read. While a missing-save picker is pending it must be
/// held in `Continue`; once the picker resolves it must pass through to the real delegate.
pub const SHOW_PROGRESS_SAVE_CHECK_TYPE: u32 = 10;

/// True when a `ShowProgressJob::Run` invocation is the boot save-data check that should be held
/// while the missing-save picker is pending.
pub fn should_hold_save_check(progress_type: Option<u32>, missing_save_pending: bool) -> bool {
    missing_save_pending && progress_type == Some(SHOW_PROGRESS_SAVE_CHECK_TYPE)
}

/// True when native title menu opening should be suppressed until the picked save is present.
pub fn should_suppress_title_open_menu(missing_save_pending: bool) -> bool {
    missing_save_pending
}

/// True when a title `SetState(owner, state)` should be denied while the missing-save picker is
/// pending.
///
/// Only the world-entry states are denied. Menu/title states must keep flowing so the title thread,
/// overlay input, and the picker UI stay alive.
pub fn should_deny_world_entry(missing_save_pending: bool, state: i32) -> bool {
    missing_save_pending && matches!(state, TITLE_STEP_BEGIN_NEW_GAME | TITLE_STEP_PLAY_GAME)
}

/// Consecutive ticks the autoload will sit on an empty-like Continue-slot profile before it gives
/// up on its own save selection and hands the choice back to the user.
///
/// 1800 ticks. The unit is the product autoload's game-task tick, the same unit its existing log
/// throttle counts in, so this reads as "sixty log lines" rather than as a clock -- and it is
/// deliberately far past anything a slow boot can produce:
///
///   * measured (run br-20260826-174240-385e, the dead end this exists to end): the game task ran
///     at 16.8 ms/tick, and the empty-like branch emitted an identical fingerprint
///     (`map=0xffffffff level=0 name_len=0`) on every tick from +14231 ms to +104119 ms without
///     one field changing. 1800 ticks is ~30 s there -- and that run had already burned 5370.
///   * The one legitimate transient this must not cut short is the boot save-data job still
///     filling `ProfileSummary` after the autoload reaches its submit phase. In that same run the
///     window between the branch's first tick and the last save-container read was ~0.9 s. 30 s is
///     roughly thirty times it.
///   * It is a frozen-state threshold, not a slow-progress one, which is what lets it be this
///     tight: the branch republishes the same fingerprint every tick, so unlike the loading bar
///     (whose stall window had to be loosened to 60 s because early boot legitimately crawls)
///     there is no progress here that could be merely slow. Either the profile fills or it never
///     will.
pub const EMPTY_PROFILE_ESCALATE_TICKS: u64 = 1800;

/// What the autoload should do on a tick where the Continue slot's profile reads empty-like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyProfileAction {
    /// Keep waiting silently -- the throttle has already logged this window.
    Wait,
    /// Log the fingerprint (first tick of the window, then once per throttle period).
    Log,
    /// Stop waiting: reject this save selection and arm the picker so the user can supersede it.
    Escalate,
}

/// Decide the empty-like-profile branch's action for one tick.
///
/// `consecutive_ticks` is 1-based (1 on the first tick of an unbroken run of empty-like reads) and
/// resets to 0 the moment the profile reads real, so a boot that is merely slow to fill
/// `ProfileSummary` can never accumulate toward the escalation. `already_escalated` latches the
/// one-shot: the arm itself is idempotent, but re-asking it every tick would bury the loud line
/// that says the hand-back happened.
pub fn empty_profile_action(
    consecutive_ticks: u64,
    log_every: u64,
    already_escalated: bool,
) -> EmptyProfileAction {
    if !already_escalated && consecutive_ticks >= EMPTY_PROFILE_ESCALATE_TICKS {
        return EmptyProfileAction::Escalate;
    }
    let period = log_every.max(1);
    if consecutive_ticks % period == 1 % period {
        EmptyProfileAction::Log
    } else {
        EmptyProfileAction::Wait
    }
}

/// Advance the consecutive empty-like-profile counter for one tick.
///
/// Separated from [`empty_profile_action`] so the reset is testable on its own: a profile that
/// reads real -- even for a single tick in the middle of a bad window -- must put the count back
/// to zero, or a boot that flickers its way to a good load would still trip the hand-back.
pub fn empty_profile_next_ticks(previous_ticks: u64, profile_real: bool) -> u64 {
    if profile_real {
        0
    } else {
        previous_ticks.saturating_add(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_check_hold_only_applies_to_pending_save_progress() {
        assert!(should_hold_save_check(
            Some(SHOW_PROGRESS_SAVE_CHECK_TYPE),
            true
        ));
        assert!(!should_hold_save_check(
            Some(SHOW_PROGRESS_SAVE_CHECK_TYPE),
            false
        ));
        assert!(!should_hold_save_check(Some(20), true));
        assert!(!should_hold_save_check(None, true));
    }

    #[test]
    fn title_menu_suppression_follows_only_the_pending_latch() {
        assert!(should_suppress_title_open_menu(true));
        assert!(!should_suppress_title_open_menu(false));
    }

    #[test]
    fn world_entry_denial_is_limited_to_world_entry_states() {
        assert!(should_deny_world_entry(true, TITLE_STEP_BEGIN_NEW_GAME));
        assert!(should_deny_world_entry(true, TITLE_STEP_PLAY_GAME));
        assert!(!should_deny_world_entry(false, TITLE_STEP_PLAY_GAME));
        assert!(!should_deny_world_entry(true, 0));
        assert!(!should_deny_world_entry(true, 3));
        assert!(!should_deny_world_entry(true, 10));
        assert!(!should_deny_world_entry(true, 11));
    }
    #[test]
    fn empty_profile_counter_resets_the_moment_the_profile_reads_real() {
        assert_eq!(empty_profile_next_ticks(0, false), 1);
        assert_eq!(empty_profile_next_ticks(1, false), 2);
        assert_eq!(
            empty_profile_next_ticks(EMPTY_PROFILE_ESCALATE_TICKS - 1, true),
            0
        );
        assert_eq!(empty_profile_next_ticks(u64::MAX, false), u64::MAX);
    }

    #[test]
    fn empty_profile_logs_on_the_first_tick_then_on_the_throttle() {
        assert_eq!(empty_profile_action(1, 30, false), EmptyProfileAction::Log);
        assert_eq!(empty_profile_action(2, 30, false), EmptyProfileAction::Wait);
        assert_eq!(
            empty_profile_action(30, 30, false),
            EmptyProfileAction::Wait
        );
        assert_eq!(empty_profile_action(31, 30, false), EmptyProfileAction::Log);
        assert_eq!(empty_profile_action(61, 30, false), EmptyProfileAction::Log);
    }

    #[test]
    fn empty_profile_escalates_exactly_at_the_threshold_and_only_once() {
        assert_ne!(
            empty_profile_action(EMPTY_PROFILE_ESCALATE_TICKS - 1, 30, false),
            EmptyProfileAction::Escalate
        );
        assert_eq!(
            empty_profile_action(EMPTY_PROFILE_ESCALATE_TICKS, 30, false),
            EmptyProfileAction::Escalate
        );
        // Latched: the arm has already run, so later ticks fall back to the log throttle and the
        // loud hand-back line is never repeated.
        assert_ne!(
            empty_profile_action(EMPTY_PROFILE_ESCALATE_TICKS + 1, 30, true),
            EmptyProfileAction::Escalate
        );
        assert_ne!(
            empty_profile_action(EMPTY_PROFILE_ESCALATE_TICKS * 4, 30, true),
            EmptyProfileAction::Escalate
        );
    }

    #[test]
    fn a_slow_boot_that_eventually_fills_the_profile_never_escalates() {
        // Drive the real loop: empty for well past the threshold's worth of ticks, but with the
        // profile flickering real every 100 ticks the way a still-filling ProfileSummary would.
        let mut ticks = 0u64;
        let mut escalated = false;
        for i in 1..=(EMPTY_PROFILE_ESCALATE_TICKS * 3) {
            let profile_real = i % 100 == 0;
            ticks = empty_profile_next_ticks(ticks, profile_real);
            if profile_real {
                continue;
            }
            if empty_profile_action(ticks, 30, escalated) == EmptyProfileAction::Escalate {
                escalated = true;
            }
        }
        assert!(
            !escalated,
            "a profile that reads real at all must never reach the hand-back"
        );
    }

    #[test]
    fn an_unbroken_empty_window_escalates_once_and_stays_quiet() {
        let mut ticks = 0u64;
        let mut escalated = false;
        let mut escalations = 0usize;
        for _ in 0..(EMPTY_PROFILE_ESCALATE_TICKS * 4) {
            ticks = empty_profile_next_ticks(ticks, false);
            if empty_profile_action(ticks, 30, escalated) == EmptyProfileAction::Escalate {
                escalations += 1;
                escalated = true;
            }
        }
        assert_eq!(escalations, 1);
    }

    #[test]
    fn a_zero_log_period_cannot_divide_by_zero() {
        assert_eq!(empty_profile_action(1, 0, false), EmptyProfileAction::Log);
        assert_eq!(empty_profile_action(7, 0, false), EmptyProfileAction::Log);
    }
}
