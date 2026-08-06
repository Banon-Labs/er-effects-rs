//! Tier A: fresh-per-process file logger + game-directory resolver.
//!
//! The three DLLs each hand-copied a `log_line` / `append_autoload_debug`
//! writer. This is the shared core, parameterized by target filename. Callers
//! keep their own named wrappers (e.g. the product's `append_autoload_debug`,
//! `append_continue_trace`) over these primitives so log paths / prefixes stay
//! owned by each caller.
//!
//! # A log describes exactly ONE process run
//!
//! Standing rule (2026-08-04): no product DLL, shell or harness in this repo may
//! append to a log ACROSS runs. Every log file is truncated by the first write of
//! the process that owns it; keeping an older run means copying the file aside
//! yourself, not letting it accumulate.
//!
//! The concrete failure that set the rule: `er-invasion-warp-dll` opened its log
//! with a plain `append(true)` on a fixed name next to the game executable. Twelve
//! separate launches piled into one 565 KB file, so a count taken over it ("37
//! confirms") read as ONE run doing something 37 times when it was really twelve
//! runs -- and per-run state could only be recovered by hand-splitting on the
//! module-base banner. Worse, lines from builds that no longer exist sat
//! indistinguishably next to lines from the build under test.
//!
//! [`begin_fresh_run`] is the one-shot that enforces it, and
//! [`open_fresh_run_append`] is the only sanctioned way to open a log for append.
//! `scripts/check-fresh-run-logs.py` fails the build on any other `.append(...)`
//! opener, so the rule is executable rather than a comment.

use std::ffi::OsString;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Suffix the PREVIOUS run's file is renamed to when this process freshens a log.
///
/// Exactly ONE generation is kept, and it is deliberately NOT `.prev.log`: a reader
/// or harness globbing `*.log` must not pick the stale generation up as if it were
/// live. This is not a rotation system -- run N-2 is gone.
///
/// The invariant is that `<name>.prev` holds the run IMMEDIATELY before the live file,
/// or does not exist. Several harnesses delete the log before launching (`rm -f
/// "$GAME_DIR"/er-effects-*.log`), which leaves nothing to rotate; the older `.prev` is
/// dropped in that case rather than left sitting next to a fresh log looking one run
/// old when it is three. Keeping a run means copying it somewhere of your own.
pub const PREVIOUS_RUN_SUFFIX: &str = ".prev";

/// Log paths this process has already freshened. One entry per log file (a handful
/// at most), touched only on the first write to each path.
static FRESHENED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

std::thread_local! {
    /// True while THIS thread is inside [`begin_fresh_run`].
    static FRESHENING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Directory the game exe lives in — everything writes artifacts relative to it.
pub fn game_directory_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
}

/// One-shot per (process, path): rotate the previous run's file aside and truncate,
/// so the file that follows describes this process run and nothing else.
///
/// Idempotent. The FIRST call for a path does the work; every later call in the same
/// process is a short lookup, which is what makes "truncate once, append thereafter"
/// different from "truncate on every write" (the latter would lose the run's own
/// earlier lines).
///
/// # Why a re-entrancy guard on a logger
///
/// Both file operations below reach the OS through `kernel32!CreateFileW`, which the
/// product DLL DETOURS in every save mode -- and the detour logs. So a rotate can
/// arrive straight back here on the same thread. Re-entering would deadlock on
/// `FRESHENED`; the thread-local latch turns the nested call into a no-op instead. A
/// line about opening a log is worth nothing, and the nested writer simply appends to
/// the file this call is about to truncate.
///
/// # Failure is latched on purpose
///
/// A directory that refuses the truncating open below refuses an appending open too,
/// so retrying on the next line buys nothing but a syscall per line.
pub fn begin_fresh_run(path: &Path) {
    let entered = FRESHENING
        .try_with(|freshening| !freshening.replace(true))
        .unwrap_or(false);
    if !entered {
        return;
    }
    struct ReleaseOnDrop;
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            let _ = FRESHENING.try_with(|freshening| freshening.set(false));
        }
    }
    let _release = ReleaseOnDrop;

    {
        let mut freshened = FRESHENED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if freshened.iter().any(|seen| seen == path) {
            return;
        }
        freshened.push(path.to_path_buf());
    }

    let mut previous: OsString = path.as_os_str().to_os_string();
    previous.push(PREVIOUS_RUN_SUFFIX);
    let previous = PathBuf::from(previous);
    // Unconditional, so `<name>.prev` is never older than one run: Windows `rename` refuses
    // an existing destination anyway, and when the live file is absent (a harness cleared it
    // pre-launch) there is nothing to preserve and the stale generation should not survive.
    let _ = fs::remove_file(&previous);
    if path.exists() {
        let _ = fs::rename(path, &previous);
    }
    let _ = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path);
}

/// THE sanctioned way to open a log for append in this repo.
///
/// Freshens the file on this process's first call for `path`, then hands back an
/// appending handle. Callers may keep the handle for the life of the process (hot
/// paths should) or drop it per line (low-frequency callers).
///
/// `scripts/check-fresh-run-logs.py` rejects `OpenOptions::…append(…)` anywhere but
/// this module, so a hand-rolled appender cannot come back.
pub fn open_fresh_run_append(path: &Path) -> Option<fs::File> {
    begin_fresh_run(path);
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

/// Append one line to `path`, creating it if absent and truncating it once per
/// process. Opens/appends/closes per call (simple, low-frequency callers). For hot
/// paths prefer a caller-owned persistent handle over [`open_fresh_run_append`].
pub fn append_line(path: &std::path::Path, args: std::fmt::Arguments<'_>) {
    if let Some(mut file) = open_fresh_run_append(path) {
        let _ = writeln!(file, "{args}");
    }
}

/// Truncate-then-open `path` for a clean per-process log, invoking `header` to
/// write a banner line once. Returns the open handle so the caller can retain a
/// persistent `Mutex<Option<File>>` and avoid per-call open/close syscalls.
///
/// Routes through [`begin_fresh_run`] so the previous run's file is rotated aside
/// rather than destroyed, matching every other writer.
pub fn open_truncated_with_header(
    path: &std::path::Path,
    header: impl FnOnce(&mut fs::File),
) -> Option<fs::File> {
    begin_fresh_run(path);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .ok()?;
    header(&mut file);
    Some(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second write in the SAME process must not lose the first: truncation is
    /// one-shot, not per-write. This is the bug the rule's shape is chosen to avoid.
    #[test]
    fn first_write_truncates_and_later_writes_append() {
        let dir = std::env::temp_dir().join(format!(
            "er-game-base-fresh-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("one-run.log");
        let _ = fs::write(&path, "STALE FROM AN EARLIER RUN\n");

        append_line(&path, format_args!("first"));
        append_line(&path, format_args!("second"));

        let body = fs::read_to_string(&path).expect("log written");
        assert_eq!(
            body, "first\nsecond\n",
            "stale run survived, or a line was lost"
        );

        let previous = fs::read_to_string(dir.join("one-run.log.prev")).expect("rotated aside");
        assert_eq!(previous, "STALE FROM AN EARLIER RUN\n");
        let _ = fs::remove_dir_all(&dir);
    }

    /// `<name>.prev` must hold the run immediately before the live file, or nothing. A
    /// harness that clears the log pre-launch leaves nothing to rotate, and a `.prev` left
    /// over from an older run would read as "the run before this one" when it is not.
    #[test]
    fn a_cleared_log_does_not_leave_an_older_generation_behind() {
        let dir = std::env::temp_dir().join(format!(
            "er-game-base-cleared-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("cleared.log");
        // Live file absent (harness `rm -f`'d it), stale generation still on disk.
        let _ = fs::write(dir.join("cleared.log.prev"), "THREE RUNS AGO\n");

        append_line(&path, format_args!("only line"));

        assert_eq!(
            fs::read_to_string(&path).expect("log written"),
            "only line\n"
        );
        assert!(
            !dir.join("cleared.log.prev").exists(),
            "a stale generation survived next to a fresh log"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
