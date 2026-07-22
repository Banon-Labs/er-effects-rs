//! er-telemetry: the telemetry subsystem lifted out of the product DLL.
//!
//! STATUS: skeleton + shared log/oracle scaffolding. The full body of the 8
//! telemetry source files (write_telemetry / write_game_module_oracles /
//! write_oracle / game_man_snapshot / bootstrap / save_policy_logs) is migrated
//! here file-group by file-group as the ~900-symbol ownership inversion described
//! in the extraction plan is completed. This crate depends ONLY on er-game-base +
//! upstream game libs, never on er-effects-rs (product).
//!
//! Per-tick product data enters via [`TelemetryFrameInput`] rather than a direct
//! read of the product's `EffectsState` behind its `Arc<Mutex<>>` lock, so
//! telemetry never needs the product lock type.

pub mod counters;

use std::path::PathBuf;
use std::sync::atomic::Ordering;

/// The handful of per-frame product-owned values telemetry actually reads,
/// built by the product BEFORE calling into telemetry (so telemetry never
/// touches the product's `Arc<Mutex<EffectsState>>`). Extended as write_telemetry
/// migrates over.
#[derive(Clone, Copy, Debug, Default)]
pub struct TelemetryFrameInput {
    /// Whether the local player pointer resolved this frame (product-observed).
    pub player_available: bool,
    /// Monotonic per-frame game-task tick counter (product-owned).
    pub game_task_ticks: u64,
}

/// CWD-relative artifact written by the standalone telemetry-only DLL. Distinct
/// from the product's `er-effects-telemetry.json` so a combined run keeps both.
const STANDALONE_JSON: &str = "er-telemetry-timeseries.jsonl";

fn standalone_json_path() -> PathBuf {
    er_game_base::log::game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(STANDALONE_JSON)
}

/// Read-side-only telemetry tick for the standalone `er-telemetry-dll`.
///
/// Emits exactly the subset of oracle_* fields derivable from game RAM/PE alone
/// (no product hooks, no `EffectsState`): the game module base and the three
/// stable singleton pointers. As the real oracle bodies migrate here, this grows
/// to call `write_game_module_oracles` / `write_oracle_telemetry` with an absent
/// [`TelemetryFrameInput`] and default (product-unwritten) counters.
pub fn standalone_tick() {
    let n = counters::STANDALONE_TICKS.fetch_add(1, Ordering::SeqCst) + 1;

    // Throttle disk writes to every 4th tick -- dense enough to sample the game frame time across a
    // ~3s vanilla-reload playable window (at 20fps that is ~0.2s between writes), still a snapshot.
    const WRITE_EVERY: u64 = 4;
    if n % WRITE_EVERY != 0 {
        return;
    }

    let base = er_game_base::mem::game_module_base().unwrap_or(0);
    let read_singleton = |rva: usize| -> usize {
        if base == 0 {
            return 0;
        }
        unsafe { er_game_base::mem::safe_read_usize(base + rva) }.unwrap_or(0)
    };
    let game_data_man = read_singleton(er_game_base::rva::GAME_DATA_MAN_GLOBAL_RVA);
    let game_man = read_singleton(er_game_base::rva::GAME_MAN_SINGLETON_RVA);
    let cs_menu_man = read_singleton(er_game_base::rva::CS_MENU_MAN_GLOBAL_RVA);

    // VANILLA-RELOAD FPS COMPARISON (2026-07-22): read the game's own frame timer. CSFlipperImp
    // singleton at base+0x4589ad8; task_delta (+0x268) = the game loop frame time (1/task_delta = fps),
    // fixed_spf (+0x1c) = the flip target (0.0167=60). play_time (GameDataMan+0xa0, u32 ms) rises only
    // while the world simulates -> the in-world/playable gate. Lets a telemetry-only run measure a
    // user-driven native reload's playable fps to compare against our reload path. bd
    // USER-chose-vanilla-reload-comparison-2026-07-22.
    const CS_FLIPPER_SINGLETON_RVA: usize = 0x4589ad8;
    const GAME_DATA_MAN_PLAY_TIME_A0_OFFSET: usize = 0xa0;
    let flipper = read_singleton(CS_FLIPPER_SINGLETON_RVA);
    let read_f32 = |ptr: usize, off: usize| -> f32 {
        if ptr == 0 {
            return -1.0;
        }
        unsafe { er_game_base::mem::safe_read_usize(ptr + off) }
            .map_or(-1.0, |v| f32::from_bits((v & 0xffff_ffff) as u32))
    };
    let flip_task_delta = read_f32(flipper, 0x268);
    let flip_fixed_spf = read_f32(flipper, 0x1c);
    let play_time_ms: i64 = if game_data_man == 0 {
        -1
    } else {
        unsafe {
            er_game_base::mem::safe_read_usize(game_data_man + GAME_DATA_MAN_PLAY_TIME_A0_OFFSET)
        }
        .map_or(-1, |v| i64::from((v & 0xffff_ffff) as u32))
    };

    let body = format!(
        "{{\"oracle_standalone_ticks\":{n},\
\"oracle_game_module_base\":\"0x{base:x}\",\
\"oracle_game_data_man_ptr\":\"0x{game_data_man:x}\",\
\"oracle_game_man_ptr\":\"0x{game_man:x}\",\
\"oracle_cs_menu_man_ptr\":\"0x{cs_menu_man:x}\",\
\"oracle_flip_task_delta\":{flip_task_delta:.6},\
\"oracle_flip_fixed_spf\":{flip_fixed_spf:.6},\
\"oracle_play_time_ms\":{play_time_ms}}}\n"
    );
    // APPEND one JSON line per write -> a timeseries jsonl the agent reads AFTER the run (no polling,
    // no sleep). body already ends in '\n'.
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(standalone_json_path())
    {
        let _ = f.write_all(body.as_bytes());
    }
}
