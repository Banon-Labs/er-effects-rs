use crate::prelude::*;

// Character-portrait compositor for the native-Windows isolated overlay.
//
// The portrait PIPELINE (build + engine idle-animation render + our safe readback) publishes the rendered,
// alpha-keyed character head into LOADING_BG_PORTRAIT_RGBA -- proven live on native (run35: 224 clean
// readbacks, 203 publishes, motion_metric_max=4482 == a MOVING head, zero AVs). On Wine that buffer is
// displayed by the in-swapchain Present composite; on native the composite is suppressed (it crashes the
// game device), so nothing drew it. THIS is the missing display half: a pure-CPU alpha-scale-blit of that
// already-captured buffer onto the isolated overlay's own backbuffer -- ZERO game-device work (mirrors
// overlay_save_picker_onto / overlay_stats_onto). Included into gpu_readback.rs, so LOADING_BG_PORTRAIT_RGBA
// and the boot helpers are in-namespace.

/// Cumulative count of overlay frames where the portrait actually blended onto the backbuffer (telemetry
/// `oracle_portrait_onto_draw_hits`). RAM proof the captured head reached the isolated overlay; distinct
/// from the readback/publish counters (which prove the head was CAPTURED, not displayed).
pub use er_telemetry::counters::PORTRAIT_ONTO_DRAW_HITS;

/// Last measured alpha-coverage of the captured portrait, in percent of the full source area (telemetry
/// `oracle_portrait_alpha_cover_pct`). The captured head sits in a central region of the square source with
/// transparent padding around it; this is how much of that square the head's bounding box actually fills, so
/// a low value confirms most of the source is margin (why scaling the padded square did not enlarge the head).
pub use er_telemetry::counters::PORTRAIT_ALPHA_COVER_PCT;

pub use er_telemetry::counters::PORTRAIT_CROP_MAXX;
pub use er_telemetry::counters::PORTRAIT_CROP_MAXY;
/// Stable crop envelope: the union of the head's alpha bounding box over the first `PORTRAIT_CROP_SEED_N`
/// frames, then FROZEN. Re-cropping to a fresh per-frame bounding box made the rect chase the swaying head,
/// which showed as horizontal jitter and cancelled the real idle animation. Freezing the envelope lets the
/// head's actual sway play WITHIN a fixed rect. Single render thread, so plain atomics need no ordering care.
pub use er_telemetry::counters::PORTRAIT_CROP_MINX;
pub use er_telemetry::counters::PORTRAIT_CROP_MINY;
pub use er_telemetry::counters::PORTRAIT_CROP_SEED_FRAMES;

/// Unmasked-frame refusal counters. Incremented by the mask gate in `portrait_onto` below and by the
/// two publish-side gates (`save_swap_profile_table.rs`, `dlstring_lookat_math.rs`), and read by
/// `oracle_portrait_draw_refused_unmasked` / `oracle_portrait_bake_publish_refused_unmasked`. See the
/// counters' own docs in er-telemetry for what each refusal means.
pub use er_telemetry::counters::PORTRAIT_BAKE_PUBLISH_REFUSED_UNMASKED;
pub use er_telemetry::counters::PORTRAIT_DRAW_REFUSED_UNMASKED;
const PORTRAIT_CROP_SEED_N: usize = 40;

/// True when the overlay should composite the captured character portrait: a published head exists and the
/// save picker is not owning the screen (the picker has no character context). Cheap -- just the lock +
/// presence check -- so it is safe to poll every frame from boot_view_render_frame to drive full_frame.
///
/// DELIBERATELY DOES NOT TEST THE MASK, and this is not the gate (2026-08-21). The authoritative
/// unmasked-frame refusal is inside `portrait_onto`, for two reasons.
///
/// First, cost: answering "is this buffer masked" needs a walk of the alpha channel. `portrait_onto`
/// ALREADY walks it -- it has to, to find the head's bounding box -- so the gate rides that existing
/// pass for free, whereas this function is polled at least twice per frame (`boot_view_render_frame`
/// and the Present path both call it to decide `full_frame`) and is documented as a lock plus a
/// presence check. Putting the walk here would add two full alpha scans per frame to buy nothing.
///
/// Second, and the reason it would be wrong even if it were free: this predicate decides the overlay's
/// GEOMETRY, not its content. A true answer forces the full-screen canvas instead of the tight progress
/// strip. If it flipped with the mask, the overlay would start as a strip, then jump to full-screen the
/// instant the first keyed frame landed -- a visible layout snap in the middle of the loading screen,
/// caused by the fix rather than by the defect. Reporting "a portrait is published, lay out for it" and
/// then declining to draw an unmasked one keeps the canvas stable and simply leaves the head absent
/// until it is ready, which is exactly the requested behaviour.
///
/// Both display hosts reach the refusal because both go through `portrait_onto`: the product boot view
/// (`boot_progress.rs` -> `boot_view_rasterize`, which also feeds the native isolated overlay and the
/// Wine in-swapchain compositor) and the standalone `er-loading-portrait-dll` compositor.
pub fn portrait_overlay_active() -> bool {
    if save_picker_overlay_active() {
        return false;
    }
    LOADING_BG_PORTRAIT_RGBA
        .lock()
        .ok()
        .map(|g| {
            g.as_ref()
                .is_some_and(|(sw, sh, px)| *sw > 0 && *sh > 0 && !px.is_empty())
        })
        .unwrap_or(false)
}

/// Composite the captured character portrait onto the overlay's full-frame RGBA buffer (`w`x`h`). Reads the
/// alpha-keyed head from LOADING_BG_PORTRAIT_RGBA and nearest-neighbour scale-blits it (alpha-over) into an
/// upper-left rect sized to the screen, so the background/black shows through the keyed-out head silhouette.
/// Returns false when no portrait is published. Pure CPU; render-thread safe; no game-device calls.
pub fn portrait_onto(buf: &mut [u8], w: usize, h: usize) -> bool {
    if w == 0 || h == 0 {
        return false;
    }
    let Some((sw, sh, spx)) = LOADING_BG_PORTRAIT_RGBA.lock().ok().and_then(|g| g.clone()) else {
        return false;
    };
    let (sw, sh) = (sw as usize, sh as usize);
    if sw == 0 || sh == 0 || spx.len() < sw * sh * 4 {
        return false;
    }
    // The head occupies only a central region of the square source; the rest is transparent padding. Find
    // the alpha bounding box (strided scan, alpha > 8) so we scale the HEAD to the target rect instead of the
    // padded square -- otherwise a bigger box just enlarges empty margin and the head looks unchanged.
    //
    // The SAME pass also counts how much of the frame is transparent, which is the mask-gate evidence
    // below. It is folded in here rather than measured separately because the loop already reads every
    // sampled texel's alpha byte: the count costs an add, a second pass would cost another ~264k reads
    // per frame on a 1542x1542 source. Counting on the strided sample rather than every texel is fine --
    // the decision is a 5%-vs-100% question, not a boundary one, and a uniform 1-in-9 sample answers it
    // to well within that margin.
    const ATHRESH: u8 = 8;
    const STRIDE: usize = 3;
    let (mut minx, mut miny, mut maxx, mut maxy) = (sw, sh, 0usize, 0usize);
    let mut any = false;
    let (mut counted, mut transparent) = (0usize, 0usize);
    let mut y = 0;
    while y < sh {
        let row = y * sw;
        let mut x = 0;
        while x < sw {
            let a = spx[(row + x) * 4 + 3];
            counted += 1;
            if a < PORTRAIT_ALPHA_OPAQUE_MIN {
                transparent += 1;
            }
            if a > ATHRESH {
                any = true;
                if x < minx {
                    minx = x;
                }
                if x > maxx {
                    maxx = x;
                }
                if y < miny {
                    miny = y;
                }
                if y > maxy {
                    maxy = y;
                }
            }
            x += STRIDE;
        }
        y += STRIDE;
    }
    if !any || maxx < minx || maxy < miny {
        return false;
    }
    // THE MASK GATE (user 2026-08-21: "do not render the portrait until we mask out the background").
    //
    // This is the authoritative refusal for BOTH display hosts, and it sits here -- ahead of the crop
    // fold and the blit -- because it has to stop two distinct kinds of damage, and only this position
    // stops the second one.
    //
    //   1. The frame itself. An unmasked buffer is alpha-255 everywhere, so the alpha-over blit below
    //      copies the character's entire SCENE BACKGROUND onto the loading screen. That is the visible
    //      defect: the first composited portrait frame showed the render including its backdrop.
    //   2. The crop envelope, which outlives the frame. The `PORTRAIT_CROP_*` union is seeded from the
    //      first PORTRAIT_CROP_SEED_N frames and then FROZEN forever. An unmasked frame's bounding box
    //      is the WHOLE square, so folding even one in pins the envelope at maximum and every later
    //      masked head is scaled down inside that oversized rect for the rest of the loading screen.
    //      A live run measured `oracle_portrait_alpha_cover_pct = 99` against a
    //      `oracle_depth_key_bg_pct` of 76 -- those cannot both describe a keyed frame. Returning
    //      before the fold is what makes a refused frame cost nothing beyond the frame it was in.
    //
    // The predicate is the bridge's, so it is the same floor the capture side publishes against; see
    // `portrait_mask_share_ok` for why it is a binary "was anything cut at all" and never a quality
    // score, and why it is judged per-BUFFER rather than from the per-window PROFILE_HAVE_KEYED_FRAME
    // flag (which re-arms wrong on switch loads).
    //
    // Consequence, accepted deliberately: until a masked frame exists, NOTHING draws. The overlay's own
    // canvas -- background, progress bar, phase label, stats -- is rasterized before and after this call
    // and is untouched by the refusal; the loading screen simply has no head on it yet. The worker
    // publishes a keyed frame moments later and it appears then, which is the requested behaviour.
    if !portrait_mask_share_ok(transparent, counted) {
        PORTRAIT_DRAW_REFUSED_UNMASKED.fetch_add(1, Ordering::SeqCst);
        return false;
    }
    // Fold this frame's extent into the crop envelope during the seed window, then read the FROZEN envelope
    // (the sway union). After seeding, the crop rect never moves, so the head animates inside a fixed rect
    // instead of the rect jittering to track it.
    let seeded = PORTRAIT_CROP_SEED_FRAMES.fetch_add(1, Ordering::SeqCst);
    if seeded < PORTRAIT_CROP_SEED_N {
        PORTRAIT_CROP_MINX.fetch_min(minx, Ordering::SeqCst);
        PORTRAIT_CROP_MINY.fetch_min(miny, Ordering::SeqCst);
        PORTRAIT_CROP_MAXX.fetch_max(maxx, Ordering::SeqCst);
        PORTRAIT_CROP_MAXY.fetch_max(maxy, Ordering::SeqCst);
    }
    let cminx = PORTRAIT_CROP_MINX.load(Ordering::SeqCst).min(sw - 1);
    let cminy = PORTRAIT_CROP_MINY.load(Ordering::SeqCst).min(sh - 1);
    let cmaxx = PORTRAIT_CROP_MAXX
        .load(Ordering::SeqCst)
        .min(sw - 1)
        .max(cminx);
    let cmaxy = PORTRAIT_CROP_MAXY
        .load(Ordering::SeqCst)
        .min(sh - 1)
        .max(cminy);
    let crop_w = (cmaxx - cminx + 1).max(1);
    let crop_h = (cmaxy - cminy + 1).max(1);
    PORTRAIT_ALPHA_COVER_PCT.store(crop_w * crop_h * 100 / (sw * sh), Ordering::SeqCst);
    // Target rect: the cropped head fills ~80% of screen height (aspect from the crop, not the square),
    // horizontally centered and bottom-anchored to the true screen bottom so the render clips exactly at the
    // monitor edge. The bar is drawn AFTER this (see boot_view_rasterize), so the bar sits in front.
    let dst_h = (h * 80 / 100).max(1);
    let dst_w = (dst_h * crop_w / crop_h).max(1);
    let x0 = w.saturating_sub(dst_w) / 2;
    let y0 = h.saturating_sub(dst_h);
    for dy in 0..dst_h {
        let ty = y0 + dy;
        if ty >= h {
            break;
        }
        let sy = (cminy + dy * crop_h / dst_h).min(sh - 1);
        for dx in 0..dst_w {
            let tx = x0 + dx;
            if tx >= w {
                break;
            }
            let sx = (cminx + dx * crop_w / dst_w).min(sw - 1);
            let si = (sy * sw + sx) * 4;
            let a = spx[si + 3] as u32;
            if a == 0 {
                continue; // keyed-out background: let the overlay/black show through
            }
            let di = (ty * w + tx) * 4;
            let ia = 255 - a;
            buf[di] = ((spx[si] as u32 * a + buf[di] as u32 * ia) / 255) as u8;
            buf[di + 1] = ((spx[si + 1] as u32 * a + buf[di + 1] as u32 * ia) / 255) as u8;
            buf[di + 2] = ((spx[si + 2] as u32 * a + buf[di + 2] as u32 * ia) / 255) as u8;
            buf[di + 3] = 255;
        }
    }
    PORTRAIT_ONTO_DRAW_HITS.fetch_add(1, Ordering::SeqCst);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an 8x8 red RGBA8 source. `keyed` gives it a transparent 1px border (28 of 64 texels
    /// cut, ~44%); otherwise every texel is opaque, which is what a colour-only readback produces.
    fn red_source(keyed: bool) -> (u32, u32, Vec<u8>) {
        let (sw, sh) = (8usize, 8usize);
        let mut px = vec![0u8; sw * sh * 4];
        for (i, texel) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x, y) = (i % sw, i / sw);
            let edge = x == 0 || y == 0 || x == sw - 1 || y == sh - 1;
            texel[0] = 255;
            texel[3] = if keyed && edge { 0 } else { 255 };
        }
        (sw as u32, sh as u32, px)
    }

    /// The mask gate, both verdicts, in one test because they share the process-global bridge and
    /// crop envelope and must not race each other.
    ///
    /// The unmasked half asserts more than "returned false": it asserts the destination buffer was
    /// not touched and the crop seed counter did not move. Those are the two damage paths -- the
    /// frame drawn with its background, and the FROZEN crop envelope permanently widened by an
    /// opaque frame's full-square bounding box -- and a gate that returned false after folding the
    /// envelope would still have caused the second one while looking correct from the outside.
    #[test]
    fn portrait_onto_refuses_unmasked_frame_and_draws_keyed_one() {
        let (w, h) = (16usize, 12usize);

        // UNMASKED: a fully opaque capture must not draw and must not seed the envelope.
        if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
            *g = Some(red_source(false));
        }
        let refused_before = PORTRAIT_DRAW_REFUSED_UNMASKED.load(Ordering::SeqCst);
        let seeded_before = PORTRAIT_CROP_SEED_FRAMES.load(Ordering::SeqCst);
        let hits_before = PORTRAIT_ONTO_DRAW_HITS.load(Ordering::SeqCst);
        let mut buf = vec![0u8; w * h * 4];
        assert!(
            !portrait_onto(&mut buf, w, h),
            "an unmasked (fully opaque) capture must not be composited"
        );
        assert!(
            buf.iter().all(|&b| b == 0),
            "a refused frame must leave the destination untouched"
        );
        assert_eq!(
            PORTRAIT_CROP_SEED_FRAMES.load(Ordering::SeqCst),
            seeded_before,
            "a refused frame must not seed the frozen crop envelope"
        );
        assert_eq!(
            PORTRAIT_ONTO_DRAW_HITS.load(Ordering::SeqCst),
            hits_before,
            "a refused frame is not a draw hit"
        );
        assert_eq!(
            PORTRAIT_DRAW_REFUSED_UNMASKED.load(Ordering::SeqCst),
            refused_before + 1,
            "the refusal must be counted, or a run cannot prove the gate engaged"
        );

        // KEYED: the same head with a real alpha cut composites normally.
        if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
            *g = Some(red_source(true));
        }
        assert!(
            portrait_onto(&mut buf, w, h),
            "a depth-keyed capture must still be composited"
        );
        let red = buf
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0] > 200 && px[1] < 30 && px[2] < 30 && px[3] == 255)
            .count();
        assert!(
            red > 0,
            "expected the keyed head to blend, got {red} red px"
        );
        assert_eq!(
            PORTRAIT_DRAW_REFUSED_UNMASKED.load(Ordering::SeqCst),
            refused_before + 1,
            "a keyed frame must not be counted as a refusal"
        );

        if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
            *g = None;
        }
    }

    /// The bridge's admission rule on its own, including the degenerate inputs a caller can reach:
    /// an empty buffer is an ABSENT measurement, not a masked frame, and must not admit anything.
    #[test]
    fn mask_predicate_is_binary_and_fails_closed() {
        assert!(!portrait_frame_is_masked(&red_source(false).2));
        assert!(portrait_frame_is_masked(&red_source(true).2));
        assert!(!portrait_frame_is_masked(&[]));
        assert!(!portrait_mask_share_ok(0, 0));
        // Exactly at the floor passes; one short of it does not.
        assert!(portrait_mask_share_ok(PORTRAIT_MIN_TRANSPARENT_PCT, 100));
        assert!(!portrait_mask_share_ok(
            PORTRAIT_MIN_TRANSPARENT_PCT - 1,
            100
        ));
    }
}
