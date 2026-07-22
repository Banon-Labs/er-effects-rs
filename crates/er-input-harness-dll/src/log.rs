//! Append-only debug log, modeled on `er-reload-trace-dll`'s log helper. The harness leaves a
//! diagnosable evidence trail (default runtime research mode is telemetry/non-fatal per AGENTS.md)
//! without a `bd` memory or a screenshot -- those are separate oracles.

use std::{
    fmt,
    fs::{File, OpenOptions},
    io::Write,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::win32::GetTickCount64;

const LOG_PATH: &str = "er-input-harness.log";
/// One JSON object per line, one line per phase completion (advanced|derailed). Consumed by the run
/// oracle to diff vanilla vs. product per phase (duration + which semaphores were live at exit).
const PHASES_PATH: &str = "er-input-harness-phases.jsonl";

static LOG_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();
static PHASES_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();
static EVENT_SEQ: AtomicU64 = AtomicU64::new(0);

fn open_log_file() -> Option<Mutex<File>> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG_PATH)
        .ok()
        .map(Mutex::new)
}

fn open_phases_file() -> Option<Mutex<File>> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(PHASES_PATH)
        .ok()
        .map(Mutex::new)
}

pub fn reset_log_file() {
    let _ = File::create(LOG_PATH);
}

pub fn reset_phases_file() {
    let _ = File::create(PHASES_PATH);
}

/// Append one already-formatted JSON line to the per-phase telemetry file (no seq/tick prefix -- the
/// line is self-describing so the oracle can parse it directly).
pub fn log_phase(line: &str) {
    let Some(lock) = PHASES_FILE.get_or_init(open_phases_file) else {
        return;
    };
    let Ok(mut file) = lock.lock() else {
        return;
    };
    let _ = writeln!(file, "{line}");
}

pub fn log_line(args: fmt::Arguments<'_>) {
    let Some(lock) = LOG_FILE.get_or_init(open_log_file) else {
        return;
    };
    let Ok(mut file) = lock.lock() else {
        return;
    };
    let tick = unsafe { GetTickCount64() };
    let seq = EVENT_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let _ = writeln!(file, "[{seq:06} +{tick}ms] {args}");
}

macro_rules! harness_log {
    ($($arg:tt)*) => { $crate::log::log_line(format_args!($($arg)*)) };
}
pub(crate) use harness_log;
