use std::{
    fs::{self, OpenOptions},
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

pub(crate) fn reset_log_file() {
    let _ = START_MS.set(now_ms());
    let _ = fs::write(log_path(), "");
}

pub(crate) fn net_effects_log(args: std::fmt::Arguments<'_>) {
    let line = format!("[+{}ms] {args}\n", elapsed_ms());
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = file.write_all(line.as_bytes());
    }
}
