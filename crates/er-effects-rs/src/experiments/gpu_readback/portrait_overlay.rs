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
pub(crate) use er_telemetry::counters::PORTRAIT_ONTO_DRAW_HITS;

/// Last measured alpha-coverage of the captured portrait, in percent of the full source area (telemetry
/// `oracle_portrait_alpha_cover_pct`). The captured head sits in a central region of the square source with
/// transparent padding around it; this is how much of that square the head's bounding box actually fills, so
/// a low value confirms most of the source is margin (why scaling the padded square did not enlarge the head).
pub(crate) use er_telemetry::counters::PORTRAIT_ALPHA_COVER_PCT;

/// Stable crop envelope: the union of the head's alpha bounding box over the first `PORTRAIT_CROP_SEED_N`
/// frames, then FROZEN. Re-cropping to a fresh per-frame bounding box made the rect chase the swaying head,
/// which showed as horizontal jitter and cancelled the real idle animation. Freezing the envelope lets the
/// head's actual sway play WITHIN a fixed rect. Single render thread, so plain atomics need no ordering care.
pub(crate) use er_telemetry::counters::PORTRAIT_CROP_MINX;
pub(crate) use er_telemetry::counters::PORTRAIT_CROP_MINY;
pub(crate) use er_telemetry::counters::PORTRAIT_CROP_MAXX;
pub(crate) use er_telemetry::counters::PORTRAIT_CROP_MAXY;
pub(crate) use er_telemetry::counters::PORTRAIT_CROP_SEED_FRAMES;
const PORTRAIT_CROP_SEED_N: usize = 40;

/// True when the overlay should composite the captured character portrait: a published head exists and the
/// save picker is not owning the screen (the picker has no character context). Cheap -- just the lock +
/// presence check -- so it is safe to poll every frame from boot_view_render_frame to drive full_frame.
pub(crate) fn portrait_overlay_active() -> bool {
    if save_picker_overlay_active() {
        return false;
    }
    LOADING_BG_PORTRAIT_RGBA
        .lock()
        .ok()
        .map(|g| g.as_ref().is_some_and(|(sw, sh, px)| *sw > 0 && *sh > 0 && !px.is_empty()))
        .unwrap_or(false)
}

/// Composite the captured character portrait onto the overlay's full-frame RGBA buffer (`w`x`h`). Reads the
/// alpha-keyed head from LOADING_BG_PORTRAIT_RGBA and nearest-neighbour scale-blits it (alpha-over) into an
/// upper-left rect sized to the screen, so the background/black shows through the keyed-out head silhouette.
/// Returns false when no portrait is published. Pure CPU; render-thread safe; no game-device calls.
pub(crate) fn portrait_onto(buf: &mut [u8], w: usize, h: usize) -> bool {
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
    const ATHRESH: u8 = 8;
    const STRIDE: usize = 3;
    let (mut minx, mut miny, mut maxx, mut maxy) = (sw, sh, 0usize, 0usize);
    let mut any = false;
    let mut y = 0;
    while y < sh {
        let row = y * sw;
        let mut x = 0;
        while x < sw {
            if spx[(row + x) * 4 + 3] > ATHRESH {
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
    let cmaxx = PORTRAIT_CROP_MAXX.load(Ordering::SeqCst).min(sw - 1).max(cminx);
    let cmaxy = PORTRAIT_CROP_MAXY.load(Ordering::SeqCst).min(sh - 1).max(cminy);
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
