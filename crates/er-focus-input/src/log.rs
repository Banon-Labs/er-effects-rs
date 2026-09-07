//! Fresh-per-run debug log for the focus-input shell.
//!
//! The path resolves through `er_game_base::log::redirected_artifact_path`, which is the launcher's
//! redirect first and `er-focus-input.log` beside `eldenring.exe` otherwise. A bare CWD-relative
//! name would be wrong here for the same reason it was wrong twice on 2026-09-04: the game's working
//! directory is not the game directory under every launch path, so the file lands somewhere no
//! artifact collector looks and the run reads as "the shell never logged".

use std::{
    fmt,
    fs::File,
    io::Write,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

/// Default artifact name, used when the launcher names no redirect.
const LOG_PATH: &str = "er-focus-input.log";
/// Launcher redirect for this shell's log. Diagnostic override only -- the shell's behaviour is
/// identical with it unset, which is what `AGENTS.md` requires of a product feature.
const LOG_PATH_ENV: &str = "ER_QUICKLOAD_FOCUS_INPUT_LOG_PATH";

static LOG_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();
static EVENT_SEQ: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetTickCount64() -> u64;
}

/// Milliseconds since boot, for the log-line prefix.
///
/// Host builds have no `kernel32`; the timestamp is only a prefix, so zero is a fine answer there
/// and keeps the crate compiling off-target without a second logging path.
fn tick_ms() -> u64 {
    #[cfg(windows)]
    {
        unsafe { GetTickCount64() }
    }
    #[cfg(not(windows))]
    {
        0
    }
}

fn log_path() -> std::path::PathBuf {
    er_game_base::log::redirected_artifact_path(LOG_PATH_ENV, LOG_PATH)
}

fn open_log_file() -> Option<Mutex<File>> {
    er_game_base::log::open_fresh_run_append(&log_path()).map(Mutex::new)
}

/// Start this run's log clean at attach, rotating the previous run's file to `.prev`.
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
    let tick = tick_ms();
    let seq = EVENT_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = writeln!(file, "[{seq:06} +{tick}ms] {args}");
}

macro_rules! focus_log {
    ($($arg:tt)*) => { $crate::log::log_line(format_args!($($arg)*)) };
}
pub(crate) use focus_log;
