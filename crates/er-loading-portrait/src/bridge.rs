//! The portrait frame bridge: the worker publishes the captured, alpha-keyed character
//! head here; display hosts (product Wine composite, product native overlay, standalone
//! DLL compositor) read it. Moved from er-effects-rs constants/anti_debug.rs.
//! Host-buildable on purpose (std + er-telemetry only) so host tests can exercise the
//! publish/composite path.

pub use er_telemetry::counters::LOADING_BG_PORTRAIT_GX_CAPTURE_HITS;
/// The kept-alive portrait `CSGxTexture` captured during ProfileSelect (0 until captured). When set,
/// the forge swaps it into its TpfResCap container's TexResCap so the loading screen shows the real
/// rendered character portrait instead of the placeholder checker.
pub use er_telemetry::counters::LOADING_BG_PORTRAIT_GX_KEPT;
/// The live profile-portrait offscreen render target, read back via D3D12 into CPU RGBA8 once the
/// character head has rendered (`portrait_real_pixels_enabled()` gate). Tuple = (width, height,
/// tightly-packed `width*height*4` RGBA8 pixels). `None` until a successful readback. When `Some`,
/// the now-loading forge builds its TPF from these REAL pixels instead of the magenta/yellow checker.
pub static LOADING_BG_PORTRAIT_RGBA: std::sync::Mutex<Option<(u32, u32, Vec<u8>)>> =
    std::sync::Mutex::new(None);
/// 1 if the read-back portrait has any non-black texel (max(R,G,B) > 24) inside a center 64x64
/// region, else 0 (a black/blank capture). Exposed as `oracle_loading_bg_portrait_gx_nonblack`.
pub use er_telemetry::counters::LOADING_BG_PORTRAIT_NONBLACK;
/// Bumped every time LOADING_BG_PORTRAIT_RGBA is REPLACED with a fresh capture. The present-overlay
/// composite watches this: when it changes, the overlay re-uploads its source texture from the new RGBA,
/// so a LIVE per-frame (throttled) readback of the built renderer's offscreen makes the displayed head
/// UPDATE (portrait refreshes) instead of freezing on the first captured frame.
pub use er_telemetry::counters::LOADING_BG_PORTRAIT_RGBA_VERSION;
/// One-shot log latch for the live-display-feed (built RT content -> overlay).
pub use er_telemetry::counters::PROFILE_LIVE_FEED_LOGGED;

// ---------------------------------------------------------------------------------------------
// THE BRIDGE'S OWN ADMISSION RULE: "has this buffer been masked at all?"
//
// LOADING_BG_PORTRAIT_RGBA is a bare `(u32, u32, Vec<u8>)`. It carries no provenance, so a reader
// cannot ask WHO published a frame or WHETHER the depth key ran on it -- and it has more than one
// writer. The depth-keyed worker publishes masked frames; the game-thread bake capture in
// `save_swap_profile_table.rs` reads back the COLOUR offscreen alone (`readback_offscreen_rgba8`
// never touches the depth sibling), so every texel it publishes has alpha 255. That opaque frame
// then reached the compositor and the character's whole scene background was drawn to screen.
//
// The rule therefore lives HERE, next to the buffer, rather than in any one writer: whoever writes
// and whoever reads both answer the same question against the pixels themselves. That is also the
// only sound place for it, because the alternative gate -- the per-WINDOW `PROFILE_HAVE_KEYED_FRAME`
// flag -- cannot answer it. `portrait_retarget_and_rearm_for_switch` deliberately re-arms the bake
// one-shot on a switch load WITHOUT clearing that flag (so the make-before-break bridge keeps the
// prior head on screen), so from the second load onward the flag reads 1 while the buffer under it
// may be a brand-new unmasked capture. Alpha is per-buffer; the flag is not.
//
// BINARY, NOT A QUALITY SCORE. This asks only "did the mask cut ANYTHING", never "is the cutout
// good". bd `loading-portrait-live-path-deep-fix-2026-07-03` records a mask-INCOHERENCE scorer that
// was written, measured and REVERTED: it scored a noisy continuum and rejected 74% of frames that
// were fine. Mask quality already has its owners further up the pipeline (the worker's IoU
// gross-mismatch bar, its tear score, `apply_depth_alpha_key`'s own degenerate-mask rejection).
// This predicate exists for the one case none of them can see: a buffer the depth key never touched.

/// Minimum percent of transparent pixels a frame's mask must cut for the frame to count as keyed
/// (er-effects-rs-hi2): a real portrait mask removes a large background share; a partial mask
/// (few cut pixels on an opaque IBL box) previously passed "any transparent pixel" and displayed
/// as an unmasked head. 5% is far below any real mask's share and far above the partial band.
///
/// Moved here from `portrait_lookat` (2026-08-21) so the display half can apply the SAME number the
/// capture half does: `portrait_lookat` is `#[cfg(windows)]` and `portrait_overlay` is not, so a
/// compositor gate could not have referenced it there without either duplicating the constant --
/// two floors that drift apart is precisely the bug this closes -- or dragging the whole capture
/// module into host builds.
pub const PORTRAIT_MIN_TRANSPARENT_PCT: usize = 5;

/// Alpha at or above which a texel counts as OPAQUE for the keyed/unkeyed decision. Named because
/// the number is a shared predicate, not a local threshold: the worker's floor test
/// (`portrait_worker.rs`) counts `px[3] < 128` and the compositor must count identically, or a
/// frame the worker published as keyed could be refused at draw time and the head would vanish.
/// The midpoint is deliberate -- `apply_depth_alpha_key` writes 0 or 255, so anything between is a
/// resample artefact of the scaled mask upload rather than a decision either way.
pub const PORTRAIT_ALPHA_OPAQUE_MIN: u8 = 128;

/// The admission decision from an already-counted sample: `transparent_px` of `counted_px` texels
/// were below [`PORTRAIT_ALPHA_OPAQUE_MIN`]. Takes counts rather than pixels so a caller that is
/// ALREADY walking the alpha channel for another reason (the compositor walks it to find the head's
/// bounding box) pays for one pass instead of two. `counted_px == 0` is not maskedness, it is an
/// absent measurement, and an absent measurement must not admit a frame -- so it answers false.
pub fn portrait_mask_share_ok(transparent_px: usize, counted_px: usize) -> bool {
    if counted_px == 0 {
        return false;
    }
    transparent_px * 100 / counted_px >= PORTRAIT_MIN_TRANSPARENT_PCT
}

/// [`portrait_mask_share_ok`] over a whole RGBA8 buffer, for the publish side, where there is no
/// existing alpha pass to ride along on and the frame is inspected once rather than per displayed
/// frame. A buffer whose length is not a whole number of RGBA texels is malformed, and the partial
/// tail is simply not counted; an empty buffer answers false through the `counted_px == 0` rule.
///
/// Remember what this is protecting against: `apply_depth_alpha_key` FAILS OPEN. With no depth
/// buffer, or with no separable depth gap, it deliberately leaves the frame fully opaque rather
/// than inventing a cutout of the wrong shape. Fail-open output is indistinguishable from a frame
/// the key never ran on, and this predicate is meant to reject BOTH -- that is the point, not a
/// gap in it.
pub fn portrait_frame_is_masked(pixels: &[u8]) -> bool {
    // `as_chunks::<4>()` rather than `chunks_exact(4)`: the const-generic form gives the compiler a
    // fixed-size array per pixel instead of a runtime-length slice, and `clippy::chunks_exact_to_as_chunks`
    // denies the latter for a constant chunk size. The discarded `.1` remainder is any trailing bytes of a
    // buffer whose length is not a multiple of 4; ignoring it is correct here, because a partial pixel
    // carries no alpha channel to judge and must not count toward either total.
    let (rgba, _remainder) = pixels.as_chunks::<4>();
    let counted = rgba.len();
    let transparent = rgba
        .iter()
        .filter(|px| px[3] < PORTRAIT_ALPHA_OPAQUE_MIN)
        .count();
    portrait_mask_share_ok(transparent, counted)
}
