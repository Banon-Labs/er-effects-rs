//! The one-shot behind "a redirected save open failed, so ask the player".

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::is_primary_save_file_path;

/// One-shot decision: has a redirected save open just failed in a way the player must resolve?
///
/// A failed redirected open used to be a log line and nothing else, on the reasoning that the
/// game "falls back to no-save". It does not -- the boot keeps asking for the same file. Measured
/// on the 2026-09-09 stall: 3,571,867 detour calls and 33,241 opens of one staged path, none
/// successful, `saveState` never leaving 0, and no picker armed. A save the runtime cannot read is
/// the player's to replace, so the picker is the honest answer.
///
/// Only the primary container counts. A missing `.bak` is ordinary on a first boot, and a healthy
/// run does fail one redirected open -- `GraphicsConfig.xml`, measured across five world-reaching
/// runs -- so a looser predicate would raise the picker on a good boot.
///
/// The one-shot lives here rather than at the call site because the call site is a `CreateFileW`
/// detour that runs millions of times: "at most once per process" is the property that makes the
/// arming safe, and it is testable only if it is owned by something with a test.
#[derive(Debug, Default)]
pub struct FailedRedirectPicker {
    armed: AtomicUsize,
}

impl FailedRedirectPicker {
    pub const fn new() -> Self {
        Self {
            armed: AtomicUsize::new(0),
        }
    }

    /// True exactly once, on the first failed open of a primary save container.
    pub fn should_arm(&self, open_failed: bool, path: &[u16]) -> bool {
        if !open_failed || !is_primary_save_file_path(path) {
            return false;
        }
        self.armed.swap(1, Ordering::SeqCst) == 0
    }

    /// Whether this picker has already armed. A semaphore for telemetry.
    pub fn armed(&self) -> bool {
        self.armed.load(Ordering::SeqCst) != 0
    }
}
