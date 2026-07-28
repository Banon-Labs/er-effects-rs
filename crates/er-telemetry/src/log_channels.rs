//! log_channels: independently-marker-gated diagnostic log channels (Phase B decoupled diagnostics,
//! bd decoupled-diagnostics-architecture-buildplan-2026-07-24).
//!
//! Each [`Channel`] is DEFAULT OFF: its own game-dir marker file must be present to enable it. Env
//! vars do NOT propagate through me3/Proton to the game process, so a `.exists()` marker in the game
//! directory is the reliable per-channel enable (same rationale as `read::marker_exists` and
//! `renderdoc_slow_ms`). When a channel's marker is absent the [`log_channel!`] macro does ZERO file
//! I/O -- a single cached atomic check, then return -- so a plain run (the A/B baseline) carries no
//! per-line logging cost. Each channel reads ONLY its own marker, never a product-feature gate, so
//! logging is fully decoupled from product behavior.
//!
//! This module lives in er-telemetry (NO product dependency); the product reroutes its own diagnostic
//! call sites through these channels as they migrate (the firehose->channel migration is Phase D).
//! ONE channel is defined now: [`TITLE_REBUILD`].
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::OnceLock;
use std::sync::atomic::AtomicU8;

/// One independently-gated diagnostic log channel: the marker that enables it and the output file it
/// writes. `handle` lazily holds the truncated-once output file (behind a `Mutex` so the game thread
/// and render thread can both log) so an enabled channel avoids per-line open/close syscalls. All
/// fields are const-constructible, so channels are plain module statics (see [`TITLE_REBUILD`]).
pub struct Channel {
    /// Human name for the channel, used only in the one-time header banner.
    pub name: &'static str,
    /// Game-dir marker file whose presence enables the channel. DEFAULT OFF when absent.
    pub marker: &'static str,
    /// Output log file (game-dir relative), truncated once per process on the first enabled write.
    pub out_file: &'static str,
    /// Cached enable state: 0 = unknown, 1 = off, 2 = on. Checked once via the marker, then reused.
    enabled_cache: AtomicU8,
    /// Lazily-opened persistent output handle. `None` inside the cell once an open attempt failed.
    handle: OnceLock<std::sync::Mutex<Option<std::fs::File>>>,
}

impl Channel {
    /// Const constructor so a channel can be a `static` with no runtime init.
    pub const fn new(name: &'static str, marker: &'static str, out_file: &'static str) -> Self {
        Channel {
            name,
            marker,
            out_file,
            enabled_cache: AtomicU8::new(0),
            handle: OnceLock::new(),
        }
    }

    /// Marker gate, checked once and cached (0 = unknown, 1 = off, 2 = on). Off-windows always false.
    #[cfg(windows)]
    fn enabled(&self) -> bool {
        use std::sync::atomic::Ordering;
        match self.enabled_cache.load(Ordering::SeqCst) {
            1 => false,
            2 => true,
            _ => {
                let on = er_game_base::log::game_directory_path()
                    .map(|dir| dir.join(self.marker).exists())
                    .unwrap_or(false)
                    || std::path::Path::new(self.marker).exists();
                self.enabled_cache
                    .store(if on { 2 } else { 1 }, Ordering::SeqCst);
                on
            }
        }
    }

    #[cfg(not(windows))]
    fn enabled(&self) -> bool {
        false
    }
}

/// Stateless-ish common line prefix `[+<elapsed>ms]`, measured from a lazily-anchored process epoch
/// (the first channel line is T0). Cheap: one short-lived, poison-tolerant lock and no I/O. Mirrors the
/// product's `log_line_prefix` shape so channel lines cross-correlate on the same relative clock.
#[cfg(windows)]
fn line_prefix() -> String {
    use std::sync::Mutex;
    use std::time::Instant;
    static EPOCH: Mutex<Option<Instant>> = Mutex::new(None);
    let ms = {
        let mut guard = match EPOCH.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        guard.get_or_insert_with(Instant::now).elapsed().as_millis()
    };
    format!("[+{ms}ms]")
}

/// Write one already-formatted line to `chan` IFF its marker is present. Does ZERO file I/O (a single
/// cached atomic marker check, then return) when the marker is absent. Prefer the [`log_channel!`]
/// macro over calling this directly.
#[cfg(windows)]
pub fn write_channel(chan: &Channel, args: std::fmt::Arguments<'_>) {
    use std::io::Write as _;
    if !chan.enabled() {
        return;
    }
    let cell = chan.handle.get_or_init(|| std::sync::Mutex::new(None));
    let mut guard = match cell.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if guard.is_none() {
        // Truncate once per process so each run starts a clean log (matches standalone_tick's idiom).
        let path = er_game_base::log::game_directory_path()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(chan.out_file);
        let name = chan.name;
        *guard = er_game_base::log::open_truncated_with_header(&path, |file| {
            let _ = writeln!(
                file,
                "===== er-telemetry log channel {name} opened; [+Nms] = elapsed since this channel's first line ====="
            );
        });
    }
    if let Some(file) = guard.as_mut() {
        let _ = writeln!(file, "{} {args}", line_prefix());
    }
}

#[cfg(not(windows))]
pub fn write_channel(_chan: &Channel, _args: std::fmt::Arguments<'_>) {}

/// Log one formatted line to a channel IFF the channel's marker is present. ZERO file I/O (a single
/// cached atomic marker check, then return) when the marker is absent, so a plain A/B-baseline run
/// pays nothing. Usage: `er_telemetry::log_channel!(er_telemetry::log_channels::TITLE_REBUILD,
/// "rebuilt slot {slot} epoch={epoch}");`.
#[macro_export]
macro_rules! log_channel {
    ($chan:expr, $($arg:tt)*) => {{
        $crate::log_channels::write_channel(&$chan, ::core::format_args!($($arg)*));
    }};
}

/// Title-rebuild diagnostic channel: marker `er-effects-log-title-rebuild.txt` ->
/// `er-effects-title-rebuild.log`. DEFAULT OFF. Provided as the first channel now; rerouting the
/// title-rebuild call sites through it is Phase D follow-up (this only defines the channel + macro).
pub static TITLE_REBUILD: Channel = Channel::new(
    "title-rebuild",
    "er-effects-log-title-rebuild.txt",
    "er-effects-title-rebuild.log",
);
