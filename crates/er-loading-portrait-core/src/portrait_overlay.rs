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
pub use er_telemetry_core::counters::PORTRAIT_ONTO_DRAW_HITS;

/// PER-WINDOW twin of [`PORTRAIT_ONTO_DRAW_HITS`], incremented at the SAME site so the two can never
/// disagree about what "displayed" means.
///
/// THE DEAD COUNTER (run br-20260831-160354-2513). This static had NO writer anywhere in the
/// workspace -- only a `.load()` in `portrait_worker` and a `.swap(0)` in the window reset -- so it
/// read 0 unconditionally, and three separate outputs reported that 0 as fact:
///   1. `present-overlay: loading-portrait window reset ... displayed {display} frames`, which said
///      `displayed 0` for a window whose `PORTRAIT-LOADWIN VERDICT` said `displayed=270`;
///   2. `oracle_portrait_display_frames_last_window`, 0 in every telemetry snapshot ever archived
///      under `save-files/` -- the corroboration that this was structural, not a one-run fluke;
///   3. `first_keyed=` (and `oracle_portrait_first_keyed_display_last_window`), which
///      `portrait_worker` stamps FROM this counter, so every window claimed its first keyed publish
///      landed on display frame 0 -- i.e. instantly -- while `oracle_portrait_confirm_to_publish_ms`
///      measured 3609 ms for the same window.
///
/// Incrementing it here repairs all three at once, because all three ultimately want the number this
/// line produces: overlay frames on which a portrait was actually blended.
pub use er_telemetry_core::counters::PROFILE_DISPLAY_FRAMES_WINDOW;

/// Last measured alpha-coverage of the captured portrait, in percent of the full source area (telemetry
/// `oracle_portrait_alpha_cover_pct`). The captured head sits in a central region of the square source with
/// transparent padding around it; this is how much of that square the head's bounding box actually fills, so
/// a low value confirms most of the source is margin (why scaling the padded square did not enlarge the head).
pub use er_telemetry_core::counters::PORTRAIT_ALPHA_COVER_PCT;

/// How many seeding frames actually MOVED one of the four bounds below -- i.e. how many visible size
/// steps the head took while the envelope settled. Its companion `PORTRAIT_CROP_SEED_FRAMES` counts the
/// folds; this counts the ones that changed anything. The per-event detail (which bound, by how much,
/// resulting `crop_h`) goes to the autoload debug log as the `portrait-crop[..]` lines below.
pub use er_telemetry_core::counters::PORTRAIT_CROP_GROWTH_EVENTS;
pub use er_telemetry_core::counters::PORTRAIT_CROP_MAXX;
pub use er_telemetry_core::counters::PORTRAIT_CROP_MAXY;
/// Stable crop envelope: the union of the head's alpha bounding box over the first `PORTRAIT_CROP_SEED_N`
/// frames, then FROZEN. Re-cropping to a fresh per-frame bounding box made the rect chase the swaying head,
/// which showed as horizontal jitter and cancelled the real idle animation. Freezing the envelope lets the
/// head's actual sway play WITHIN a fixed rect. Single render thread, so plain atomics need no ordering care.
pub use er_telemetry_core::counters::PORTRAIT_CROP_MINX;
pub use er_telemetry_core::counters::PORTRAIT_CROP_MINY;
pub use er_telemetry_core::counters::PORTRAIT_CROP_SEED_FRAMES;

/// Unmasked-frame refusal counters. Incremented by the mask gate in `portrait_onto` below and by the
/// two publish-side gates (`save_swap_profile_table.rs`, `dlstring_lookat_math.rs`), and read by
/// `oracle_portrait_draw_refused_unmasked` / `oracle_portrait_bake_publish_refused_unmasked`. See the
/// counters' own docs in er-telemetry-core for what each refusal means.
pub use er_telemetry_core::counters::PORTRAIT_BAKE_PUBLISH_REFUSED_UNMASKED;
pub use er_telemetry_core::counters::PORTRAIT_DRAW_REFUSED_UNMASKED;
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
/// Wine in-swapchain compositor) and the standalone `er-loading-portrait` compositor.
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
    //
    // COUNT ONLY THE FRAMES ACTUALLY FOLDED IN. This was a plain `fetch_add` on every call, which made
    // `PORTRAIT_CROP_SEED_FRAMES` count composited frames rather than seeding ones: a live run reported 324
    // against a seed window of 40, so the counter could not say whether the envelope was still moving --
    // the single thing its name promises. Saturating at the window makes `== PORTRAIT_CROP_SEED_N` mean
    // FROZEN. `fetch_update` rather than a load/compare/store so the clamp is one indivisible step; this is
    // the render thread alone, but a counter whose whole point is to be trusted should not need that caveat.
    let seeded = PORTRAIT_CROP_SEED_FRAMES
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
            (n < PORTRAIT_CROP_SEED_N).then_some(n + 1)
        })
        .unwrap_or_else(|frozen| frozen);
    // The PREVIOUS bound values come back from the fold itself (`fetch_min`/`fetch_max` return the prior
    // value), so detecting growth costs four comparisons on numbers already in hand -- no extra loads, no
    // second pass, nothing on the frames that do not grow. That is what keeps this affordable on the render
    // path: the diagnostic below runs at most PORTRAIT_CROP_SEED_N times per loading window, and in practice
    // a handful, because a frame that folds in without moving a bound is silent.
    let grew = (seeded < PORTRAIT_CROP_SEED_N).then(|| {
        (
            PORTRAIT_CROP_MINX.fetch_min(minx, Ordering::SeqCst),
            PORTRAIT_CROP_MINY.fetch_min(miny, Ordering::SeqCst),
            PORTRAIT_CROP_MAXX.fetch_max(maxx, Ordering::SeqCst),
            PORTRAIT_CROP_MAXY.fetch_max(maxy, Ordering::SeqCst),
        )
    });
    let grew = grew.filter(|&(pminx, pminy, pmaxx, pmaxy)| {
        minx < pminx || miny < pminy || maxx > pmaxx || maxy > pmaxy
    });
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
    let cover_pct = crop_w * crop_h * 100 / (sw * sh);
    PORTRAIT_ALPHA_COVER_PCT.store(cover_pct, Ordering::SeqCst);
    // Target rect: the cropped head fills ~80% of screen height (aspect from the crop, not the square),
    // horizontally centered and bottom-anchored to the true screen bottom so the render clips exactly at the
    // monitor edge. The bar is drawn AFTER this (see boot_view_rasterize), so the bar sits in front.
    let dst_h = portrait_dst_height(h, cminy);
    let dst_w = (dst_h * crop_w / crop_h).max(1);
    let x0 = w.saturating_sub(dst_w) / 2;
    let y0 = h.saturating_sub(dst_h);
    // THE SETTLE, WRITTEN DOWN (2026-08-22). The user sees the portrait "make micro adjustments for a few
    // frames before settling on a camera position". It is not the camera: it is this envelope growing. The
    // blit above scales by `dst_h / crop_h` in BOTH axes (`dst_w = dst_h * crop_w / crop_h`, so `crop_w`
    // cancels out of the scale entirely), which means every frame that pushes a bound outward makes the head
    // one step SMALLER on screen, and the last such frame is where it appears to settle.
    //
    // Nothing outside the DLL can measure that. The seed window is PORTRAIT_CROP_SEED_N frames -- under a
    // second at 60fps -- and the oracles are a point-in-time latch in a JSON file with no history, so an
    // external sampler aliases the whole event and finds only the final rect once the window has closed.
    // Two runs of the SAME character settled at crop_h 1108 and 979 (a ~13% difference in apparent head
    // size) with nothing on record to say which frames moved which bound, or why.
    //
    // These lines are that record, and they are cheap for the same reason they are useful: `grew` is Some
    // only on a frame that actually moved a bound during seeding, so a steady envelope logs NOTHING, and the
    // ceiling is PORTRAIT_CROP_SEED_N lines per loading window. Both hosts reach it (product boot view and
    // the standalone compositor) because both blit through this function.
    if let Some((pminx, pminy, pmaxx, pmaxy)) = grew {
        let events = PORTRAIT_CROP_GROWTH_EVENTS.fetch_add(1, Ordering::SeqCst) + 1;
        let scale = dst_h as f64 / crop_h as f64;
        if seeded == 0 {
            // First fold: the previous bounds are the per-window reset sentinels (usize::MAX / 0), an
            // ABSENCE rather than a rect, so this event prints where the envelope started instead of a
            // delta against a number that never described anything.
            append_autoload_debug(format_args!(
                "portrait-crop[s{seeded}/{PORTRAIT_CROP_SEED_N}]: seed#{events} box=({cminx},{cminy})-({cmaxx},{cmaxy}) crop={crop_w}x{crop_h} crop_h={crop_h} cover={cover_pct}% dst={dst_w}x{dst_h} scale={scale:.4} src={sw}x{sh}"
            ));
        } else {
            // Signed deltas: a MIN bound falling is the envelope growing (dminy < 0 == the top of the head
            // rose), a MAX bound rising is the same. Both are printed for all four bounds so a past window
            // can be reconstructed line by line, and `moved=` names the bound(s) responsible -- `miny` alone
            // is the shape both observed runs took, because `maxy` saturates at the frame bottom (the last
            // STRIDE-sampled row) on the very first frame and can never move again.
            let prev_h = (pmaxy + 1).saturating_sub(pminy).max(1);
            let d = |prev: usize, now: usize| now as isize - prev as isize;
            let named = |moved: bool, name: &'static str| if moved { name } else { "" };
            append_autoload_debug(format_args!(
                "portrait-crop[s{seeded}/{PORTRAIT_CROP_SEED_N}]: grow#{events} box=({cminx},{cminy})-({cmaxx},{cmaxy}) crop={crop_w}x{crop_h} crop_h {prev_h}->{crop_h} ({:+}) head=x{:.3} moved=[{}{}{}{}] dminx={:+} dminy={:+} dmaxx={:+} dmaxy={:+} cover={cover_pct}% scale={scale:.4} src={sw}x{sh}",
                d(prev_h, crop_h),
                prev_h as f64 / crop_h as f64,
                named(minx < pminx, "minx "),
                named(miny < pminy, "miny "),
                named(maxx > pmaxx, "maxx "),
                named(maxy > pmaxy, "maxy "),
                d(pminx, cminx),
                d(pminy, cminy),
                d(pmaxx, cmaxx),
                d(pmaxy, cmaxy),
            ));
        }
    }
    // THE FREEZE. One line, and the only one that makes a window diagnosable after the fact: it fires on the
    // frame that exhausts the seed budget, after which the rect -- and so the apparent size of the head for
    // the whole rest of the loading screen -- can no longer change. `seeded + 1 == PORTRAIT_CROP_SEED_N` is
    // reachable only through the folding branch above (the counter saturates), so it fires exactly once per
    // portrait window and never on a frozen frame. A window that ends before the budget is spent has no
    // freeze line, and its growth lines plus `oracle_portrait_crop_seed_frames < 40` say exactly that.
    if seeded + 1 == PORTRAIT_CROP_SEED_N {
        append_autoload_debug(format_args!(
            "portrait-crop[s{}/{PORTRAIT_CROP_SEED_N}]: FROZEN box=({cminx},{cminy})-({cmaxx},{cmaxy}) crop={crop_w}x{crop_h} crop_h={crop_h} growths={} cover={cover_pct}% dst={dst_w}x{dst_h} scale={:.4} src={sw}x{sh} -- envelope final for this window; apparent head size is dst_h/crop_h from here on",
            seeded + 1,
            PORTRAIT_CROP_GROWTH_EVENTS.load(Ordering::SeqCst),
            dst_h as f64 / crop_h as f64,
        ));
    }
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
    // Same event, per-window. Kept adjacent on purpose: the window reset's `displayed` and the
    // `PORTRAIT-LOADWIN VERDICT`'s `displayed=` must be the same measurement, and the only way to
    // guarantee that is to count them at one site. See PROFILE_DISPLAY_FRAMES_WINDOW's doc above.
    PROFILE_DISPLAY_FRAMES_WINDOW.fetch_add(1, Ordering::SeqCst);
    true
}

/// Height of the composited portrait, given the screen height and the crop envelope's TOP row.
///
/// Normally the head fills [`PORTRAIT_DST_HEIGHT_PCT`] of the screen, bottom-anchored, so the render
/// clips at the monitor edge and the loading bar (drawn after) sits in front.
///
/// `cminy == 0` MEANS THE SOURCE IS ALREADY CUT. The crop envelope is the alpha bounding box of the
/// offscreen render, so a top row of 0 says the character's silhouette reaches the very first row of
/// the RT and is therefore truncated by the RT itself, not framed inside it. Measured 2026-09-07 on a
/// two-handed Onyx Lord: `box=(417,0)-(966,1539)` in a `1542x1542` source -- the greatsword rides
/// above the head and runs off the top. At the normal height that straight cut lands a fifth of the
/// way down the screen, in plain view, which is the artifact the user reported. Mapping the crop's
/// top row to screen row 0 instead puts the cut exactly on the game's top edge, where it cannot be
/// seen. The cost is deliberate and bounded: the portrait is drawn larger on precisely the frames
/// where it would otherwise show a seam, and untouched everywhere else.
pub fn portrait_dst_height(screen_h: usize, crop_min_y: usize) -> usize {
    if crop_min_y == 0 {
        screen_h.max(1)
    } else {
        (screen_h * PORTRAIT_DST_HEIGHT_PCT / 100).max(1)
    }
}

/// Share of screen height the portrait normally occupies.
pub const PORTRAIT_DST_HEIGHT_PCT: usize = 80;

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the tests that drive `portrait_onto`. They share the process-global frame bridge AND the
    /// process-global crop envelope, and cargo runs tests in one binary on many threads, so without this
    /// they would interleave folds into the same envelope and each would read the other's counter moves.
    /// Poison is recovered rather than propagated: a panic in one test must fail that test, not convert
    /// every other test in the file into a mutex error that hides the real assertion.
    static PORTRAIT_GLOBALS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The per-window reset (`loading_portrait_drop_published_bridge`, which is `#[cfg(windows)]` and so
    /// unreachable from a host test) applied to the envelope alone, so each test starts from the state a
    /// fresh loading window would.
    fn reset_crop_envelope() {
        PORTRAIT_CROP_MINX.store(usize::MAX, Ordering::SeqCst);
        PORTRAIT_CROP_MINY.store(usize::MAX, Ordering::SeqCst);
        PORTRAIT_CROP_MAXX.store(0, Ordering::SeqCst);
        PORTRAIT_CROP_MAXY.store(0, Ordering::SeqCst);
        PORTRAIT_CROP_SEED_FRAMES.store(0, Ordering::SeqCst);
        PORTRAIT_CROP_GROWTH_EVENTS.store(0, Ordering::SeqCst);
    }

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

    /// The same 8x8 head with a TALLER opaque region: only column x=0 is cut, so the strided scan
    /// (`STRIDE = 3`, samples at 0/3/6) sees an opaque texel at y=0 and reports `miny = 0` instead of 3.
    /// That is the exact motion the user's "micro adjustments" are made of -- the top of the head rising --
    /// and it is what a growth line has to describe.
    fn tall_source() -> (u32, u32, Vec<u8>) {
        let (sw, sh) = (8usize, 8usize);
        let mut px = vec![0u8; sw * sh * 4];
        for (i, texel) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            texel[0] = 255;
            texel[3] = if i % sw == 0 { 0 } else { 255 };
        }
        (sw as u32, sh as u32, px)
    }

    /// Capture sink for the crop diagnostics. Only `portrait-crop` lines are kept, so a concurrently
    /// running test that logs something else through the shared host cannot pollute the assertions.
    static CROP_LOG: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

    fn capture_crop_log(args: std::fmt::Arguments<'_>) {
        let line = std::fmt::format(args);
        if line.starts_with("portrait-crop") {
            CROP_LOG
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(line);
        }
    }

    /// Install the capturing logger (idempotent: the host is a process-wide `OnceLock`, and every other
    /// field keeps its default, so this changes nothing but where log lines go) and drain the buffer.
    fn take_crop_log() -> Vec<String> {
        install_host(PortraitHost {
            append_autoload_debug: capture_crop_log,
            ..PortraitHost::defaults()
        });
        std::mem::take(&mut *CROP_LOG.lock().unwrap_or_else(|e| e.into_inner()))
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
        let _serial = PORTRAIT_GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
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

    /// The two crop counters mean what their names say, which is the whole reason they are worth reading.
    ///
    /// `PORTRAIT_CROP_SEED_FRAMES` used to increment on every composited frame: a live run read 324 with a
    /// seed window of 40, so it could not answer "is the envelope frozen?" and its name actively misled.
    /// Feeding one UNCHANGING source more times than the window is long is the exact shape of that bug --
    /// the old code kept counting past 40, and it also had no way to distinguish the one frame that
    /// established the envelope from the many that folded in and changed nothing.
    /// THE REPORTED SEAM. A crop whose top row is 0 is a source the RT already truncated, so the
    /// composite has to put that cut on the screen's own top edge; anything less leaves a visible
    /// horizontal line partway down. Numbers are the measured ones: a 2160-high screen and the
    /// `box=(417,0)-(966,1539)` envelope from the two-handed run.
    #[test]
    fn a_cut_source_is_mapped_to_the_screen_top_edge() {
        assert_eq!(
            portrait_dst_height(2160, 0),
            2160,
            "cut source fills the screen height"
        );
        assert_eq!(
            2160usize.saturating_sub(portrait_dst_height(2160, 0)),
            0,
            "top edge is row 0"
        );
    }

    /// An UNCUT source keeps the framing that has been shipping -- 80% of screen height, so the
    /// dest top sits a fifth of the way down and the bar has room in front of it.
    #[test]
    fn an_uncut_source_keeps_the_eighty_percent_framing() {
        assert_eq!(
            portrait_dst_height(2160, 1),
            2160 * PORTRAIT_DST_HEIGHT_PCT / 100
        );
        assert_eq!(portrait_dst_height(1080, 37), 864);
    }

    /// Never zero, at any screen size, in either branch -- the blit divides by it.
    #[test]
    fn the_destination_height_is_never_zero() {
        for h in [0usize, 1, 2, 7, 1080] {
            assert!(portrait_dst_height(h, 0) >= 1);
            assert!(portrait_dst_height(h, 5) >= 1);
        }
    }

    #[test]
    fn crop_seed_counter_saturates_and_growth_counts_only_real_moves() {
        let _serial = PORTRAIT_GLOBALS.lock().unwrap_or_else(|e| e.into_inner());
        let (w, h) = (16usize, 12usize);
        reset_crop_envelope();
        if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
            *g = Some(red_source(true));
        }
        let mut buf = vec![0u8; w * h * 4];
        let _ = take_crop_log();
        let frames = PORTRAIT_CROP_SEED_N + 5;
        for _ in 0..frames {
            assert!(portrait_onto(&mut buf, w, h), "keyed source must composite");
        }
        assert_eq!(
            PORTRAIT_CROP_SEED_FRAMES.load(Ordering::SeqCst),
            PORTRAIT_CROP_SEED_N,
            "the seed counter must saturate at the seed window, not keep counting composited frames"
        );
        assert_eq!(
            PORTRAIT_CROP_GROWTH_EVENTS.load(Ordering::SeqCst),
            1,
            "an unchanging source moves the envelope exactly once, on the frame that establishes it"
        );
        // THE DENSITY REQUIREMENT, ASSERTED. 45 composited frames, 44 of which folded in without moving a
        // bound, must produce exactly two lines: the one that established the envelope and the one that
        // froze it. A per-frame log here would be 45 lines of noise on the render path.
        let log = take_crop_log();
        assert_eq!(
            log.len(),
            2,
            "a steady envelope must log only its seed and its freeze, got {log:#?}"
        );
        assert!(
            log[0].starts_with("portrait-crop[s0/40]: seed#1 box=(3,3)-(6,6) crop=4x4 crop_h=4 "),
            "unexpected seed line: {}",
            log[0]
        );
        assert!(
            log[1].contains("FROZEN box=(3,3)-(6,6) crop=4x4 crop_h=4 growths=1"),
            "unexpected freeze line: {}",
            log[1]
        );

        // A REAL GROWTH, in a fresh window: the taller source raises the top of the head, which is the
        // one bound that actually moves in the observed runs. The line must name it and quantify the
        // size step, because `crop_h` alone decides how large the head renders.
        reset_crop_envelope();
        let _ = take_crop_log();
        assert!(portrait_onto(&mut buf, w, h), "keyed source must composite");
        if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
            *g = Some(tall_source());
        }
        assert!(
            portrait_onto(&mut buf, w, h),
            "taller source must composite"
        );
        let log = take_crop_log();
        assert_eq!(
            log.len(),
            2,
            "expected a seed line and one growth line, got {log:#?}"
        );
        for expected in [
            "portrait-crop[s1/40]: grow#2 ",
            "box=(3,0)-(6,6) crop=4x7 ",
            "crop_h 4->7 (+3) ",
            "moved=[miny ] ",
            "dminx=+0 dminy=-3 dmaxx=+0 dmaxy=+0 ",
        ] {
            assert!(
                log[1].contains(expected),
                "growth line missing {expected:?}: {}",
                log[1]
            );
        }
        assert_eq!(
            PORTRAIT_CROP_GROWTH_EVENTS.load(Ordering::SeqCst),
            2,
            "the growth counter must count the move the log line describes"
        );

        reset_crop_envelope();
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
