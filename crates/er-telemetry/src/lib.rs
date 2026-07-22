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
const STANDALONE_JSON: &str = "er-telemetry-standalone.json";

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

    // Throttle disk writes: only every 64th tick (read-side snapshot, not a stream).
    const WRITE_EVERY: u64 = 64;
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

    let body = format!(
        "{{\"oracle_standalone_ticks\":{n},\
\"oracle_game_module_base\":\"0x{base:x}\",\
\"oracle_game_data_man_ptr\":\"0x{game_data_man:x}\",\
\"oracle_game_man_ptr\":\"0x{game_man:x}\",\
\"oracle_cs_menu_man_ptr\":\"0x{cs_menu_man:x}\"}}\n"
    );
    let _ = std::fs::write(standalone_json_path(), body);
}
