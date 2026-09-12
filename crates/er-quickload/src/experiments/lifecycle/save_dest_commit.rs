//! Product facade for the save-destination commit window, whose implementation moved to
//! `er_quit_menu_core::save_dest_commit_runtime`.
//!
//! Pure code reorganization, no behavior change. Two inputs stay on this side of the boundary and
//! are supplied here rather than through the seam, in the same style as the S7
//! `save_dest_accepted_paths_for(live, native_dir)` split:
//!
//! * `save_redirect_native_source_dir()` -- owned by `experiments::save_redirect`, which stays in
//!   the product (see the roadmap's R32 rebaseline).
//! * the SL save-job body observer -- owned by `er-save-suppress`, which the quit-menu crate
//!   deliberately does not link (`crates/er-quit-menu-core/Cargo.toml` boundary note). The three
//!   counters are handed over as function pointers, so the moved code still reads each one at the
//!   exact point it always did.

use super::*;

// `quit_menu/mod.rs` supplied this to every child; `lifecycle.rs` does not, and the two
// `save_dest_arm_redirect` parameters are the only reason the module needs it.
use std::path::Path;

use er_quit_menu_core::save_dest_commit_runtime::SaveJobObserver;
pub(crate) use er_quit_menu_core::save_dest_commit_runtime::{
    SaveDestVerdict, SaveDestWriterState, save_dest_arm_live_overwrite, save_dest_clear_target,
    save_dest_commit_identity, save_dest_commit_window_armed, save_dest_live_save_path,
    save_dest_note_redirect_hit, save_dest_redirect_for_open, save_dest_set_target,
    save_dest_target,
};

/// The SL save-job body observer the moved commit window reads through.
///
/// `er-save-suppress` owns these counters and is not a dependency of the quit-menu crate, so the
/// three functions cross as pointers. Nothing is sampled here: each is called at the moment the
/// moved code calls it.
const SAVE_JOB_OBSERVER: SaveJobObserver = SaveJobObserver {
    writer_idle: er_save_suppress::save_job_writer_idle,
    starts: er_save_suppress::save_job_starts,
    completions: er_save_suppress::save_job_completions,
};

/// Arm the scoped write-open redirect for one commit. See the moved implementation for the
/// seed/snapshot contract; the accepted-path set is built from the product's redirect mapping.
pub(crate) fn save_dest_arm_redirect(live_path: &Path, target_path: &Path) -> bool {
    er_quit_menu_core::save_dest_commit_runtime::save_dest_arm_redirect(
        live_path,
        target_path,
        save_redirect_native_source_dir(),
    )
}

/// Read the writer's position from the SL job-body observer's own counters.
pub(crate) fn save_dest_writer_state(completions_at_fire: u64) -> SaveDestWriterState {
    er_quit_menu_core::save_dest_commit_runtime::save_dest_writer_state(
        completions_at_fire,
        SAVE_JOB_OBSERVER,
    )
}

/// May the commit window be torn down right now?
pub(crate) fn save_dest_teardown_allowed(completions_at_fire: u64, context: &str) -> bool {
    er_quit_menu_core::save_dest_commit_runtime::save_dest_teardown_allowed(
        completions_at_fire,
        context,
        SAVE_JOB_OBSERVER,
    )
}

/// Disarm the commit window and score the file(s) it was responsible for.
pub(crate) fn save_dest_verify_and_disarm(reason: &str) -> Option<SaveDestVerdict> {
    er_quit_menu_core::save_dest_commit_runtime::save_dest_verify_and_disarm(
        reason,
        SAVE_JOB_OBSERVER,
    )
}

/// Full teardown of the destination side of a save flow: target, commit/open latches, and any
/// still-armed redirect window. Called whenever the flow returns to idle.
pub(crate) fn save_dest_reset(reason: &str) {
    er_quit_menu_core::save_dest_commit_runtime::save_dest_reset(reason, SAVE_JOB_OBSERVER)
}
