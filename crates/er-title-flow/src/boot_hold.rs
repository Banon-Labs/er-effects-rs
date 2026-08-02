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
}
