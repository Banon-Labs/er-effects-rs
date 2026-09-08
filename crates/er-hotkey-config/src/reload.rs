//! Noticing that a config file changed, while the game is running.
//!
//! # Why the file's text and not its mtime
//!
//! The obvious cheap check is the modification time, and this workspace used it twice. It has a
//! resolution problem: several filesystems -- and every network/VM share a Wine prefix is likely to
//! sit on -- stamp mtime to a whole second. Save a file twice inside one second and the second save
//! is invisible, which in this feature reads as "changing the key did nothing", the exact
//! indistinguishable-from-broken outcome the whole design is trying to avoid. Comparing the text
//! costs one read of a file measured in hundreds of bytes and cannot miss an edit.
//!
//! It also removes a second class of false positive: `touch`, a re-save with no changes, and this
//! DLL's own write-back of a setting all move mtime without changing anything. Each of those would
//! be reported as a reload, and a reload RESETS the key edge detector -- so a key held at that
//! moment fires again.
//!
//! # Why it is throttled, and not per frame
//!
//! The game task runs at ~60Hz and this is called from it. Reading a file sixty times a second to
//! notice an edit a human makes at most once a minute is a silly way to spend a frame budget, so a
//! poll inside [`DEFAULT_POLL_INTERVAL_MS`] of the last read costs one integer comparison and does
//! no I/O at all. A second is far below the threshold at which a person editing a file would call
//! it "not working", and far above the frame rate.
//!
//! # Testing
//!
//! [`HotFile::poll_with`] takes the clock and the read as arguments, so every decision this module
//! makes is provable on the host with no filesystem and no waiting. [`HotFile::poll`] is the thin
//! wrapper that supplies the real two.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Instant,
};

/// How long a poll goes without touching the disk. Roughly a second: below human patience, far
/// above the frame rate.
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 1000;

/// What a poll found, when it found anything. `None` from a poll means "nothing to do" -- either
/// the interval has not elapsed, or the file reads exactly as it did last time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileChange {
    /// The file's text differs from what was last seen. Carries the whole new text.
    Text(String),
    /// A file that was readable no longer is -- deleted, renamed, or permission-denied.
    Missing,
}

/// A config file watched by content.
#[derive(Debug)]
pub struct HotFile {
    path: PathBuf,
    text: Option<String>,
    interval_ms: u64,
    /// Monotonic ms at which the next poll is allowed to read. 0 so the first poll always does.
    next_read_ms: u64,
    polls: u64,
    reads: u64,
    changes: u64,
}

impl HotFile {
    /// Watch `path`, reading at most once per [`DEFAULT_POLL_INTERVAL_MS`].
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_interval(path, DEFAULT_POLL_INTERVAL_MS)
    }

    /// Watch `path` with an explicit interval. Zero means every poll reads, which is what the
    /// tests want and what a game task never should.
    #[must_use]
    pub fn with_interval(path: impl Into<PathBuf>, interval_ms: u64) -> Self {
        Self {
            path: path.into(),
            text: None,
            interval_ms,
            next_read_ms: 0,
            polls: 0,
            reads: 0,
            changes: 0,
        }
    }

    /// The file being watched.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The text currently in force, or `None` before the first successful read.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// Polls, disk reads, and reloads so far -- the three numbers that separate "the file never
    /// changed" from "the poller never ran" in a log.
    #[must_use]
    pub const fn tallies(&self) -> (u64, u64, u64) {
        (self.polls, self.reads, self.changes)
    }

    /// Adopt text this process just wrote, so the write is not reported back as somebody's edit.
    ///
    /// Several of these DLLs rewrite their own config -- a mark key appending to a list, a stack
    /// edit rewriting a line. Without this the next poll sees text it has not seen before and
    /// reports a reload, which resets the key edge detectors mid-keypress.
    pub fn adopt(&mut self, text: impl Into<String>) {
        self.text = Some(text.into());
    }

    /// The decision, with the clock and the read injected.
    ///
    /// `read` is called only when the interval has elapsed, so a caller polling every frame pays
    /// one comparison. It returns `None` for a file that could not be read at all.
    pub fn poll_with(
        &mut self,
        now_ms: u64,
        read: impl FnOnce() -> Option<String>,
    ) -> Option<FileChange> {
        self.polls = self.polls.saturating_add(1);
        if now_ms < self.next_read_ms {
            return None;
        }
        self.next_read_ms = now_ms.saturating_add(self.interval_ms);
        self.reads = self.reads.saturating_add(1);
        match read() {
            Some(text) => {
                if self.text.as_deref() == Some(text.as_str()) {
                    return None;
                }
                self.changes = self.changes.saturating_add(1);
                self.text = Some(text.clone());
                Some(FileChange::Text(text))
            }
            None => {
                // Never seen a readable file: a config that does not exist yet is not a change,
                // and reporting one every second would be a log full of nothing.
                self.text.take()?;
                self.changes = self.changes.saturating_add(1);
                Some(FileChange::Missing)
            }
        }
    }

    /// The real poll: the process's own monotonic clock and `std::fs`.
    pub fn poll(&mut self) -> Option<FileChange> {
        let path = self.path.clone();
        self.poll_with(monotonic_ms(), || std::fs::read_to_string(&path).ok())
    }
}

/// Milliseconds since the first call, which is close enough to process start for a poll interval
/// and needs no platform clock.
///
/// `Instant` is monotonic, so this cannot be moved by the user's clock changing -- an ordinary
/// occurrence on a machine syncing time, and one that would otherwise stall or spam the poller.
#[must_use]
pub fn monotonic_ms() -> u64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    let origin = *ORIGIN.get_or_init(Instant::now);
    // Milliseconds since process start does not overflow u64 in any run this could survive.
    u64::try_from(origin.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A changed file is picked up. The whole feature in one assertion.
    #[test]
    fn a_changed_file_is_picked_up() {
        let mut hot = HotFile::with_interval("er-test.toml", 1000);
        assert_eq!(
            hot.poll_with(0, || Some("key = \"F7\"".to_owned())),
            Some(FileChange::Text("key = \"F7\"".to_owned()))
        );
        assert_eq!(
            hot.poll_with(1000, || Some("key = \"F8\"".to_owned())),
            Some(FileChange::Text("key = \"F8\"".to_owned()))
        );
        assert_eq!(hot.text(), Some("key = \"F8\""));
    }

    /// An unchanged file must not churn. A reload resets the key edge detectors, so reporting one
    /// that did not happen makes a held key fire again -- a phantom press, once a second.
    #[test]
    fn an_unchanged_file_does_not_churn() {
        let mut hot = HotFile::with_interval("er-test.toml", 1000);
        let text = || Some("key = \"F7\"".to_owned());
        assert!(hot.poll_with(0, text).is_some(), "the first read is a load");
        for tick in 1..100u64 {
            assert_eq!(hot.poll_with(tick * 1000, text), None, "tick {tick}");
        }
        assert_eq!(hot.tallies(), (100, 100, 1));
    }

    /// The same bytes with a new mtime -- a re-save, a `touch`, this DLL's own write-back -- is
    /// not an edit. This is the case an mtime watcher gets wrong.
    #[test]
    fn a_rewrite_with_identical_text_is_not_a_change() {
        let mut hot = HotFile::with_interval("er-test.toml", 0);
        assert!(hot.poll_with(0, || Some("a = 1".to_owned())).is_some());
        assert_eq!(hot.poll_with(1, || Some("a = 1".to_owned())), None);
    }

    /// ...and the case an mtime watcher gets wrong in the other direction: two saves inside one
    /// second, where a one-second-resolution mtime never moves.
    #[test]
    fn two_edits_inside_one_mtime_tick_are_both_seen() {
        let mut hot = HotFile::with_interval("er-test.toml", 0);
        assert_eq!(
            hot.poll_with(0, || Some("key = \"F7\"".to_owned())),
            Some(FileChange::Text("key = \"F7\"".to_owned()))
        );
        assert_eq!(
            hot.poll_with(1, || Some("key = \"F8\"".to_owned())),
            Some(FileChange::Text("key = \"F8\"".to_owned()))
        );
    }

    /// The throttle is the difference between one read a second and sixty. Inside the interval,
    /// the reader is not even called.
    #[test]
    fn a_poll_inside_the_interval_does_no_io_at_all() {
        let mut hot = HotFile::with_interval("er-test.toml", 1000);
        assert!(hot.poll_with(0, || Some("a = 1".to_owned())).is_some());
        for frame in 1..60u64 {
            let result = hot.poll_with(frame * 16, || panic!("read inside the interval"));
            assert_eq!(result, None);
        }
        assert_eq!(hot.tallies(), (60, 1, 1), "60 polls, 1 read, 1 load");
    }

    /// A file that has never existed is not a change, and must not be reported once a second
    /// forever. A file that existed and then vanished is one.
    #[test]
    fn a_missing_file_is_only_a_change_if_it_used_to_be_there() {
        let mut hot = HotFile::with_interval("er-test.toml", 0);
        assert_eq!(hot.poll_with(0, || None), None);
        assert_eq!(hot.poll_with(1, || None), None);
        assert_eq!(
            hot.poll_with(2, || Some("a = 1".to_owned())),
            Some(FileChange::Text("a = 1".to_owned()))
        );
        assert_eq!(hot.poll_with(3, || None), Some(FileChange::Missing));
        assert_eq!(hot.poll_with(4, || None), None, "gone stays gone");
        assert_eq!(hot.text(), None);
    }

    /// A file that appears mid-session is picked up. Deleting and recreating a config is a normal
    /// way to reset it.
    #[test]
    fn a_file_that_appears_mid_session_is_picked_up() {
        let mut hot = HotFile::with_interval("er-test.toml", 0);
        assert_eq!(hot.poll_with(0, || None), None);
        assert_eq!(
            hot.poll_with(1, || Some("key = \"F7\"".to_owned())),
            Some(FileChange::Text("key = \"F7\"".to_owned()))
        );
    }

    /// Our own write-back must not come back as somebody's edit.
    #[test]
    fn adopted_text_is_not_reported_as_an_edit() {
        let mut hot = HotFile::with_interval("er-test.toml", 0);
        assert!(
            hot.poll_with(0, || Some("blocks = []".to_owned()))
                .is_some()
        );
        hot.adopt("blocks = [1]");
        assert_eq!(hot.poll_with(1, || Some("blocks = [1]".to_owned())), None);
    }

    /// The real wrapper reads the real file. Proven against a temp path rather than asserted.
    #[test]
    fn the_real_poll_reads_the_real_file() {
        let dir = std::env::temp_dir().join(format!("er-hotkey-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("watched.toml");
        std::fs::write(&path, "key = \"F7\"\n").expect("write");

        let mut hot = HotFile::with_interval(&path, 0);
        assert_eq!(
            hot.poll(),
            Some(FileChange::Text("key = \"F7\"\n".to_owned()))
        );
        assert_eq!(hot.poll(), None);
        std::fs::write(&path, "key = \"F8\"\n").expect("rewrite");
        assert_eq!(
            hot.poll(),
            Some(FileChange::Text("key = \"F8\"\n".to_owned()))
        );
        std::fs::remove_file(&path).expect("remove");
        assert_eq!(hot.poll(), Some(FileChange::Missing));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_monotonic_clock_never_goes_backwards() {
        let first = monotonic_ms();
        let second = monotonic_ms();
        assert!(second >= first);
    }
}
