//! Read-side, PASSIVE RAM oracle modules for the standalone `er-telemetry-dll`.
//!
//! Each oracle is a self-contained, independently marker-gated module that does
//! ONLY fault-safe `ReadProcessMemory` reads (via `er_game_base::mem::safe_read_*`)
//! plus, for the title-owner resolver, a bounded `VirtualQuery` address-space
//! walk. NO hooks, NO MinHook trampolines, NO D3D12 device/queue/command-list
//! touch, NO game-code / vtable-fn calls -- so they are safe to run while
//! RenderDoc is capturing (RPM/VirtualQuery never touch the GPU device or call
//! game code). See the ORACLES/SEMAPHORES specs + PHASE-A build plan (bd
//! decoupled-diagnostics-architecture-buildplan-2026-07-24).
//!
//! DECOUPLING CONTRACT: this module depends only on `er-game-base`
//! (`game_module_base`, `safe_read_*`, `vtable_in_game_image`) + `eldenring`
//! typed singletons + `fromsoftware-shared` (the `FromStatic` singleton trait),
//! never on the product (`er-effects-rs`) crate or any product static. Each
//! oracle is gated by its OWN game-dir marker file, checked once and cached,
//! DEFAULT OFF -- when its marker is absent the oracle costs a single atomic load
//! and performs zero file I/O, so a plain product run carries no diagnostic cost
//! and one A/B run can enable exactly the signals it needs.

#[cfg(windows)]
mod dialog_active;
#[cfg(windows)]
mod epoch;
#[cfg(windows)]
mod stream_overlap;
#[cfg(windows)]
mod title_binding;

/// True if a game-dir (or CWD-relative) marker file `name` exists. Stateless fs
/// check; callers cache the first result (DEFAULT OFF). Env vars do NOT propagate
/// through me3/Proton to the game process, so a `.exists()` marker in the game
/// directory is the reliable per-category enable (same rationale as
/// `renderdoc_slow_ms`'s `er-effects-rdoc-slow-ms.txt`).
#[cfg(windows)]
fn marker_exists(name: &str) -> bool {
    if let Some(dir) = er_game_base::log::game_directory_path()
        && dir.join(name).exists()
    {
        return true;
    }
    std::path::Path::new(name).exists()
}

/// Append one already-newline-terminated JSON line to `file` in the game dir
/// (falling back to CWD). Open/append/close per write, matching the crate's
/// `standalone_tick` json-append idiom.
///
/// Fresh per run: truncated by this process's first line to that path (previous run kept one
/// generation as `.prev`), so each read-side oracle's jsonl covers exactly one launch.
#[cfg(windows)]
fn append_line(file: &str, line: &str) {
    use std::io::Write as _;
    let path = er_game_base::log::game_directory_path()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(file);
    if let Some(mut f) = er_game_base::log::open_fresh_run_append(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// JSON-render an optional pointer: `"0x..."` when read, `null` when the read
/// faulted / the chain was null (distinguishes an unread slot from a real 0).
#[cfg(windows)]
fn json_hex_opt(o: Option<usize>) -> String {
    match o {
        Some(v) => format!("\"0x{v:x}\""),
        None => "null".to_string(),
    }
}

/// JSON-render an optional i32: the number when read, `null` when unread.
#[cfg(windows)]
fn json_i32_opt(o: Option<i32>) -> String {
    match o {
        Some(v) => v.to_string(),
        None => "null".to_string(),
    }
}

/// JSON-render an optional u8: the number when read, `null` when unread.
#[cfg(windows)]
fn json_u8_opt(o: Option<u8>) -> String {
    match o {
        Some(v) => v.to_string(),
        None => "null".to_string(),
    }
}

/// Drive both read-side oracles for this write-tick. Called from `standalone_tick`
/// AFTER the base / play_time / frame-time reads. The load epoch is advanced ONCE
/// here (so the flat-play_time boundary is measured consistently regardless of how
/// many oracles are enabled) and passed to both. Each oracle is independently
/// gated by its own cached marker; an absent marker means that oracle no-ops.
#[cfg(windows)]
pub fn tick(base: usize, play_time_ms: i64, flip_task_delta: f32) {
    let epoch = epoch::advance(play_time_ms);
    title_binding::tick(base, epoch, play_time_ms);
    stream_overlap::tick(epoch, play_time_ms, flip_task_delta);
    dialog_active::tick(base, epoch, play_time_ms);
}

#[cfg(not(windows))]
pub fn tick(_base: usize, _play_time_ms: i64, _flip_task_delta: f32) {}
