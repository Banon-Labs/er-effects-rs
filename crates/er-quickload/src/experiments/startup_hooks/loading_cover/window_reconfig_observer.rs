//! Observe-only user32 window-reconfiguration hooks: product facade.
//!
//! The hooks, the early final-geometry apply and their telemetry moved to
//! `er_loading_portrait_core::window_reconfig_observer` with the loading-cover crate extraction.
//! What stays here is the one product entry point `experiments/lifecycle/hook_installers.rs`
//! spawns, wrapped so the loading-cover seam is installed before any moved code runs (the moved
//! module reaches `trace_first_game_caller_rva`, `safe_input_proc`, `own_window` and
//! `create_absolute_hook` through it).

use super::ensure_loading_cover_host;

/// Install all observe-only user32 window-reconfiguration hooks. Runs from its own attach thread
/// (same early-attach pattern as the safe-input hooks) so CreateWindowExW is covered before the
/// game builds its startup window.
pub(crate) fn install_window_reconfig_observer_hooks() {
    ensure_loading_cover_host();
    er_loading_portrait_core::window_reconfig_observer::install_window_reconfig_observer_hooks()
}
