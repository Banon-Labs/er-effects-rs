// Post-release cover WATCH: product re-export facade.
//
// The watch itself -- the per-frame cover-plate sample, the in-game-menu open stamps, the log/
// telemetry clock map and the oracle emitter -- moved to
// `er_loading_portrait_core::cover_after_release` with the loading-cover crate extraction. Its
// rationale, its cost note and every oracle's meaning travelled with it.
//
// What stays here is the three product entry points, each wrapped so the loading-cover seam is
// installed before any moved code runs: the moved module reaches `append_autoload_debug`,
// `push_json_usize`, `process_log_elapsed_ms`, `boot_view_epoch_ms_if_anchored` and
// `fake_loading_screen_visible` through it.

/// Sample the post-release state for one presented frame. Called from
/// [`native_ls_exposure_record`] on exactly the frames that function declines to judge.
pub(crate) fn cover_after_release_record(base: usize, now_ms: u64) {
    crate::experiments::ensure_loading_cover_host();
    er_loading_portrait_core::cover_after_release::cover_after_release_record(base, now_ms)
}

#[cfg(feature = "quit-rows")]
/// Note one `02_000_IngameTop` `MenuWindowJob::Run` tick, and stamp the ones that start a menu
/// session. Called from the product `MenuWindowJob::Run` post-hook.
pub(crate) fn in_game_menu_note_run_tick(job: usize, window: usize) {
    crate::experiments::ensure_loading_cover_host();
    er_loading_portrait_core::cover_after_release::in_game_menu_note_run_tick(job, window)
}

/// State once, in the log, how the log's own `[+Nms]` prefix and the telemetry `*_ms` fields relate.
pub(crate) fn log_clock_map_once() {
    crate::experiments::ensure_loading_cover_host();
    er_loading_portrait_core::cover_after_release::log_clock_map_once()
}

/// Emit the post-release watch oracles plus the boot-view null detector. Called from the telemetry
/// writer.
pub(crate) fn cover_after_release_write(body: &mut String) {
    crate::experiments::ensure_loading_cover_host();
    er_loading_portrait_core::cover_after_release::cover_after_release_write(body)
}
