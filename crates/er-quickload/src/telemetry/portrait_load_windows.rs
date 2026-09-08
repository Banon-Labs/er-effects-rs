// Per-load-window portrait verdict SEMAPHORES: product re-export facade.
//
// The window state machine, the published-vs-loaded identity check, the verdict classification and
// the history ring moved to `er_loading_portrait_core::portrait_load_windows` with the
// loading-cover crate extraction. Every verdict class and its meaning travelled with it.
//
// What stays here is the one product entry point the telemetry writer calls, wrapped so the
// loading-cover seam is installed before any moved code runs (the moved module reaches
// `append_autoload_debug`, `push_json_usize` and `boot_view_epoch_ms` through the two seams).

/// Advance the window state machine and emit the loadwin oracles. Called from the telemetry
/// writer on every write (~4 Hz). Read-only against game state; writes only our own counters.
pub(crate) fn portrait_loadwin_sample_and_write(body: &mut String) {
    crate::experiments::ensure_loading_cover_host();
    er_loading_portrait_core::portrait_load_windows::portrait_loadwin_sample_and_write(body)
}
