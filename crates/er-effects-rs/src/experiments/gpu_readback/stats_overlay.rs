// Loading-screen character-stats block on the native-Windows isolated overlay.
//
// Ports the EXISTING game-menu-font loading-screen stats (er-effects-rs-jsm) onto the separate-window
// overlay so native Windows shows the stats the USER expects: the game's OWN menu font (MenuFont_01 via
// `er_gfx::raster::RasterFont`, captured from the game's `font.gfx`), the same on-screen SIZE
// (`em_px = screen_max * 48/2056`), and the same LOCATION (top-left at 5% width / 60% height -- the exact
// mapping `stats_text_screen_position` used for the in-swapchain Present overlay in `overlay_composite.rs`).
//
// The difference vs the Wine path: the composite is PURELY CPU here (`render_lines_to_rgba` -> versioned
// screen bitmap -> `blend_rgba_over`), so it never creates resources or submits command lists on the game's
// D3D12 device -- that shared-device work is exactly what crashes the strict native AMD driver, which is why
// the overlay is a separate isolated device in the first place. The stat LINES are built on the game thread
// by `maybe_build_stats_text` (safe guarded game-memory reads, driven from lifecycle.rs on native); this
// render-thread path only re-rasters those cached lines at screen scale and alpha-blends the bitmap.
//
// Included into gpu_readback.rs (same namespace as boot_progress.rs / save_picker_overlay.rs), so it calls
// the in-namespace `save_picker_overlay_active()` directly and the flat `crate::experiments::` re-exports
// (`stats_text_available`, `stats_text_screen_bitmap`, `blend_rgba_over`) for the startup_hooks helpers.

/// Cumulative count of overlay frames where the stats block ACTUALLY blended onto the backbuffer
/// (telemetry `oracle_overlay_stats_draw_hits`). RAM semaphore that the game-font stats composite reached
/// the isolated overlay -- distinct from `STATS_TEXT_BUILT` (lines built on the game thread). NOTE per the
/// behavioral-feature-proof rule: this proves the composite RAN, not that the pixels look right; visual
/// acceptability is the user's call from the captured frame.
pub(crate) static OVERLAY_STATS_DRAW_HITS: AtomicUsize = AtomicUsize::new(0);

/// True when the overlay should composite the loading-screen stats block: whenever a stats bitmap exists
/// (the game thread has built readable lines), EXCEPT while the save picker owns the screen (the picker
/// has no character context and owns the full panel). Cheap -- no raster -- so it is safe to call from
/// `boot_view_render_frame` every frame to drive `full_frame`.
pub(crate) fn stats_overlay_active() -> bool {
    !save_picker_overlay_active() && crate::experiments::stats_text_available()
}

/// Composite the loading-screen stats block onto the overlay's full-frame RGBA buffer (`w`x`h`, RGBA8).
/// Re-rasters the game-thread-built lines at screen scale in the game's own MenuFont, then alpha-blends the
/// bitmap at the expected loading-screen location (top-left 5% width / 60% height). Returns false when no
/// stats bitmap is available yet (font not captured / no readable character), so the frame degrades to just
/// the bar -- exactly like `overlay_save_picker_onto` returns false with no model. Pure CPU; render-thread safe.
pub(crate) fn overlay_stats_onto(buf: &mut [u8], w: usize, h: usize) -> bool {
    if w == 0 || h == 0 {
        return false;
    }
    // Screen-scale raster: size the font from the screen HEIGHT (not w.max(h)). The game path derived em_px
    // from the max dimension because its stats were baked into a square portrait RT the engine then aspect-
    // cover (width-dominated) upscaled -- at 4K that yields ~90px text. On our full-screen overlay the bar is
    // fixed near the bottom, and 5 lines at 90px overrun/collide, so size height-proportionally
    // (h * 48/2056 -> ~50px at 2160p, ~25px at 1080p): readable and non-overlapping at every resolution.
    let Some((sw, sh, rgba, _ver)) = crate::experiments::stats_text_screen_bitmap(h as u32) else {
        return false;
    };
    if sw == 0 || sh == 0 || rgba.is_empty() {
        return false;
    }
    // Expected loading-screen location: text top-left at (5% width, 60% height). The in-swapchain overlay
    // mapped the same source point through the portrait aspect-cover crop; our overlay is full-screen with
    // no crop, so (5%, 60%) is the direct screen coordinate.
    let x0 = (w * 5 / 100) as i32;
    let y0 = (h * 60 / 100) as i32;
    crate::experiments::blend_rgba_over(buf, w as u32, h as u32, &rgba, sw, sh, x0, y0);
    OVERLAY_STATS_DRAW_HITS.fetch_add(1, Ordering::SeqCst);
    true
}
