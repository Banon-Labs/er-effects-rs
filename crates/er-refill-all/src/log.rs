//! One log file beside the game executable, sequence-numbered.
//!
//! Sequence numbers rather than timestamps: what these lines are read for is order -- did the
//! config reload land before or after the press that behaved oddly -- and a wall clock does not
//! answer that when several lines share a frame.

use std::{
    fmt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use er_game_base::log::{append_line, game_directory_path};

const LOG_FILE_NAME: &str = "er-refill-all.log";

static LOG_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn refill_log(args: fmt::Arguments<'_>) {
    let path = game_directory_path()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(LOG_FILE_NAME);
    let seq = LOG_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1;
    append_line(&path, format_args!("[{seq:06}] {args}"));
}
