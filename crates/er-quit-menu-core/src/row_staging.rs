//! The snapshot that puts the game's character records back after the picker borrows them.
//!
//! The in-game file picker renders its browse rows by writing them into the live
//! `CS::ProfileSummary`: each 0x2a0-byte record is zeroed and the row label copied into the name
//! field. That destroys the game's own records -- face data at `+0x38`, `ChrAsm` at `+0x1a8`, level
//! at `+0x24`, map at `+0x30` -- so they have to be put back on every exit from the picker: close,
//! commit, and abort alike.
//!
//! # Why this is not the save-swap ledger
//!
//! It used to ride on the product's `preview_applied`/`committed` latches, and that conflation was
//! a bug with a visible symptom. Row staging and the foreign-save preview write the same
//! allocation but are different things, and only the preview may be suppressed by `committed` -- a
//! committed preview has to survive in order to be loaded. Sharing one bit meant an earlier
//! cross-file load commit set `committed`, which nothing resets except the tail of the restore it
//! was blocking, and every later picker left its rows in the records for the rest of the process:
//! loading screens rendered `[..] EldenRing` and `[ new ]` as character names.
//!
//! So the staging keeps its own latch and its own snapshot, restored unconditionally -- and now
//! its own module, in the crate that owns the picker, so a standalone shell has it without the
//! product's save-swap ledger behind it.

use std::sync::{Mutex, MutexGuard, OnceLock};

use er_game_base::profile_summary::PROFILE_SUMMARY_TOTAL_BYTES;

/// The picker's borrow of the live records, and what it borrowed them from.
#[derive(Default)]
pub struct RowStaging {
    /// The live ProfileSummary currently holds picker browse-row labels, not the game's records.
    pub rows_staged: bool,
    /// The allocation `rows_snapshot` was taken from; a restore refuses to write anywhere else.
    pub rows_summary_ptr: usize,
    /// The `PROFILE_SUMMARY_TOTAL_BYTES` image of that allocation as it looked before the first
    /// staging of the current picker session.
    pub rows_snapshot: Vec<u8>,
}

static ROW_STAGING: OnceLock<Mutex<RowStaging>> = OnceLock::new();

/// Lock the staging state, recovering from a poisoned lock rather than panicking on the game
/// thread -- a panic here would take the process down inside a menu tick.
pub fn row_staging_lock() -> MutexGuard<'static, RowStaging> {
    ROW_STAGING
        .get_or_init(|| Mutex::new(RowStaging::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Whether the live records currently hold the picker's rows.
pub fn rows_staged() -> bool {
    row_staging_lock().rows_staged
}

/// Take the pre-staging image of `summary`, once per picker session.
///
/// # Safety
///
/// `summary` must be the live `CS::ProfileSummary` allocation: this reads
/// `PROFILE_SUMMARY_TOTAL_BYTES` from it through a raw pointer.
pub unsafe fn arm_row_snapshot(summary: usize) {
    let mut staging = row_staging_lock();
    if staging.rows_staged && staging.rows_summary_ptr == summary {
        return;
    }
    staging.rows_summary_ptr = summary;
    staging.rows_snapshot = unsafe {
        core::slice::from_raw_parts(summary as *const u8, PROFILE_SUMMARY_TOTAL_BYTES).to_vec()
    };
    staging.rows_staged = true;
}
