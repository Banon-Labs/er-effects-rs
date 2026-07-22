//! Tier A: parameterized append-only file logger + game-directory resolver.
//!
//! The three DLLs each hand-copied a `log_line` / `append_autoload_debug`
//! writer. This is the shared core, parameterized by target filename. Callers
//! keep their own named wrappers (e.g. the product's `append_autoload_debug`,
//! `append_continue_trace`) over these primitives so log paths / prefixes stay
//! owned by each caller.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

/// Directory the game exe lives in — everything writes artifacts relative to it.
pub fn game_directory_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
}

/// Append one line to `path`, creating it if absent. Opens/appends/closes per
/// call (simple, low-frequency callers). For hot paths prefer a caller-owned
/// persistent handle (see the product's `append_autoload_debug`).
pub fn append_line(path: &std::path::Path, args: std::fmt::Arguments<'_>) {
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{args}");
    }
}

/// Truncate-then-open `path` for a clean per-process log, invoking `header` to
/// write a banner line once. Returns the open handle so the caller can retain a
/// persistent `Mutex<Option<File>>` and avoid per-call open/close syscalls.
pub fn open_truncated_with_header(
    path: &std::path::Path,
    header: impl FnOnce(&mut fs::File),
) -> Option<fs::File> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .ok()?;
    header(&mut file);
    Some(file)
}
