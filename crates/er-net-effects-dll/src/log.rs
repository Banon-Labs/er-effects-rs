use std::{
    io::Write,
    path::PathBuf,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

const LOG_FILE_NAME: &str = "er-net-effects.log";
static START_MS: OnceLock<u128> = OnceLock::new();

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn elapsed_ms() -> u128 {
    let start = *START_MS.get_or_init(now_ms);
    now_ms().saturating_sub(start)
}

fn log_path() -> PathBuf {
    PathBuf::from(LOG_FILE_NAME)
}

/// Anchor the `[+Nms]` stamps to DLL attach and start this run's log clean.
///
/// The truncation is delegated rather than done here (this used to `fs::write("")`) so the
/// previous run's file is rotated aside as `.log.prev` instead of being destroyed, and so the
/// log still starts fresh even if a line somehow beats attach to the file.
pub(crate) fn reset_log_file() {
    let _ = START_MS.set(now_ms());
    er_game_base::log::begin_fresh_run(&log_path());
}

pub(crate) fn net_effects_log(args: std::fmt::Arguments<'_>) {
    let line = format!("[+{}ms] {args}\n", elapsed_ms());
    if let Some(mut file) = er_game_base::log::open_fresh_run_append(&log_path()) {
        let _ = file.write_all(line.as_bytes());
    }
}
