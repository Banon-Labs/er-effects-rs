//! Fresh-per-run trace log, modeled on `er-input-harness`/`er-reload-trace`'s log helper.
//!
//! The three traces used to write into the product's `er-quickload-crash.log` through
//! `append_crash_log`. They cannot any more -- they live in a different image, and a second
//! appender on one path interleaves two processes' opinions into one file. They get their own
//! file instead, and it describes exactly one process run: `er_game_base::log` truncates on this
//! process's first write (rotating the previous run's aside as `.prev`), which is what makes a
//! count over `er-diag-harness.log` a count for this run rather than for every launch since the
//! file was created.
//!
//! The path is resolved against the game directory, not the CWD: me3 launch wrappers set the
//! process CWD to arbitrary Windows directories, so a relative path scatters the evidence away
//! from the product's own logs.

use std::{
    fmt,
    fs::File,
    io::Write,
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

const LOG_FILE_NAME: &str = "er-diag-harness.log";

static LOG_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();
static EVENT_SEQ: AtomicU64 = AtomicU64::new(0);

/// The trace log's absolute path: the launcher's redirect if it set one, else beside
/// `eldenring.exe`, falling back to the CWD only when the module path cannot be resolved at all.
///
/// The redirect is what lets a run's evidence outlive the next run: the game-directory copy is
/// single-slot, so two launches lose the run before last. Resolution lives in
/// `er_game_base::log` so every per-run artifact answers "where does this go" the same way.
fn log_path() -> PathBuf {
    er_game_base::log::redirected_artifact_path("ER_QUICKLOAD_DIAG_HARNESS_PATH", LOG_FILE_NAME)
}

fn open_log_file() -> Option<Mutex<File>> {
    er_game_base::log::open_fresh_run_append(&log_path()).map(Mutex::new)
}

/// Start this run's log clean at attach. Rotates the previous run's file to `.prev` rather than
/// destroying it.
pub fn reset_log_file() {
    er_game_base::log::begin_fresh_run(&log_path());
}

pub fn log_line(args: fmt::Arguments<'_>) {
    let Some(lock) = LOG_FILE.get_or_init(open_log_file) else {
        return;
    };
    let Ok(mut file) = lock.lock() else {
        return;
    };
    let seq = EVENT_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = writeln!(file, "[{seq:06}] {args}");
}

macro_rules! diag_log {
    ($($arg:tt)*) => { $crate::log::log_line(format_args!($($arg)*)) };
}
pub(crate) use diag_log;
