use std::{fs, path::PathBuf};

pub(crate) struct Log;

impl Log {
    pub(crate) fn write(args: std::fmt::Arguments<'_>) {
        use std::io::Write;

        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(LogPath::current())
        {
            let _ = writeln!(file, "{args}");
        }
    }
}

struct LogPath;

impl LogPath {
    fn current() -> PathBuf {
        std::env::var("ER_MENU_SORT_DEBUG_PATH")
            .or_else(|_| std::env::var("ER_EFFECTS_AUTOLOAD_DEBUG_PATH"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("er-menu-sort-debug.log"))
    }
}
