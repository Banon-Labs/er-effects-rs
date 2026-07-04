
/// DEPTH-KEYED TRANSPARENT BACKGROUND: read back the offscreen scene's depth plane and set the color
/// buffer's per-pixel alpha to 0 for every BACKGROUND pixel. The portrait depth is BIMODAL -- the model
/// (head+shoulders) forms one cluster and the dark IBL surround another, separated by an empty depth GAP
/// (the "air" between the model and the backdrop). We histogram the depth, put the threshold in the WIDEST
/// empty gap, and KEEP whichever cluster the centered head sits in, cutting the other. Keying on the head's
/// own cluster (not an assumed clear value) makes this robust to the engine's Z direction and to the fact
/// that the corners are NOT all background (a portrait's shoulders fill the bottom corners). FAIL-OPEN on
/// every uncertainty (no depth buffer, dims mismatch, no separable gap) -> the color buffer is left fully
/// opaque, exactly the pre-existing display, so this can only ADD the cutout, never regress the head. Emits
/// a one-shot `depth-key` diagnostic and drives the `oracle_depth_key_*` RAM semaphores. `cpx` is the
/// tightly-packed RGBA8 the caller is about to publish (mutated in place).
pub(crate) unsafe fn apply_depth_alpha_key(gpu_child: usize, w: u32, h: u32, cpx: &mut [u8]) {
    let w = w as usize;
    let h = h as usize;
    if cpx.len() < w * h * 4 {
        return;
    }
    // (1) RECALCULATE the mask from the CURRENT depth buffer whenever it carries real content.
    let fresh = unsafe { compute_depth_mask(gpu_child, w, h) };
    // (2) On a fresh mask, CACHE it; on a dead frame (depth read back cleared -> no gap), REUSE the last
    //     cached mask so the cutout stays stable. Recalculated whenever fresh depth is available (tracks a
    //     genuine re-render), cached only for the frames in between -- never a frozen one-shot.
    let mask = if let Some(m) = fresh {
        DEPTH_KEY_FRESH.fetch_add(1, Ordering::SeqCst);
        // Tag the cache with the character it was computed for, so a later cross-character reuse is
        // detectable (the stale-reuse desync semaphore below).
        LAST_DEPTH_MASK_INCARNATION.store(
            PROFILE_PORTRAIT_INCARNATION.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        if let Ok(mut g) = LAST_DEPTH_MASK.lock() {
            *g = Some((w, h, m.clone()));
        }
        Some(m)
    } else {
        let reused = LAST_DEPTH_MASK.lock().ok().and_then(|g| match g.as_ref() {
            Some((cw, ch, m)) if *cw == w && *ch == h => Some(m.clone()),
            _ => None,
        });
        if reused.is_some() {
            // FAIL-FAST desync semaphore: this depth was dead (no fresh gap) so we are reusing the cached
            // mask -- but if it was computed for a DIFFERENT character incarnation than the one rendering
            // now, its silhouette will not match this head (the 2nd-character depth-mask desync). Detect
            // it as a run-stopping RAM oracle rather than only seeing it on screen.
            let cur = PROFILE_PORTRAIT_INCARNATION.load(Ordering::SeqCst);
            let cached = LAST_DEPTH_MASK_INCARNATION.load(Ordering::SeqCst);
            if cur != 0 && cached != 0 && cur != cached {
                let n = PROFILE_MASK_STALE_REUSE.fetch_add(1, Ordering::SeqCst);
                if PROFILE_MASK_STALE_REUSE_LOGGED.swap(1, Ordering::SeqCst) == 0 {
                    append_autoload_debug(format_args!(
                        "MASK-STALE-REUSE-DESYNC: reusing depth mask from portrait incarnation {cached} on incarnation {cur} (prior character's silhouette on the new head) -- #{}",
                        n + 1
                    ));
                    // HARD FAIL-FAST during the diagnostic repro ONLY: abort the process the instant the
                    // desync is detected so the run stops in ~40s instead of six minutes. A fast crash ==
                    // the semaphore caught the stale-reuse; no crash == the desync is NOT stale-reuse (a
                    // different mechanism) and the hypothesis is refuted. Never fires in product (gated on
                    // the System-Quit repro being active), so it cannot crash a real player session.
                    if system_quit_repro_enabled() {
                        append_autoload_debug(format_args!(
                            "MASK-STALE-REUSE-DESYNC: FAIL-FAST abort() (repro diagnostic stop)"
                        ));
                        std::process::abort();
                    }
                }
            }
        }
        reused
    };
    // No fresh gap and no cache yet -> fail open (opaque), no regression.
    let Some(mask) = mask else {
        return;
    };
    let mut masked = 0usize;
    for i in 0..(w * h) {
        if mask[i] != 0 {
            cpx[i * 4 + 3] = 0;
            masked += 1;
        }
    }
    if masked > 0 {
        DEPTH_KEY_APPLIED.fetch_add(1, Ordering::SeqCst);
        DEPTH_KEY_BG_PCT.store(masked * 100 / (w * h), Ordering::SeqCst);
        // FAIL-FAST mask/head coherence (2nd-character desync): does the KEPT cutout match THIS head?
        let iou = mask_head_iou(&mask, cpx, w, h);
        PROFILE_MASK_HEAD_IOU_LAST.store(iou, Ordering::SeqCst);
        if iou < MASK_HEAD_IOU_MIN {
            let streak = PROFILE_MASK_HEAD_MISMATCH_STREAK.fetch_add(1, Ordering::SeqCst) + 1;
            PROFILE_MASK_HEAD_MISMATCH_TOTAL.fetch_add(1, Ordering::SeqCst);
            if streak == 1 || streak % 16 == 0 {
                append_autoload_debug(format_args!(
                    "MASK-HEAD-MISMATCH: IoU={iou}% < {MASK_HEAD_IOU_MIN}% (kept cutout vs colour head) streak={streak} -- fresh-but-wrong depth silhouette on this head"
                ));
            }
            // Abort only on a SUSTAINED gross mismatch (a whole loading screen desyncs; a transient
            // build glitch is a few frames), and only during the repro (never a real player session).
            if streak >= MASK_HEAD_ABORT_STREAK && system_quit_repro_enabled() {
                append_autoload_debug(format_args!(
                    "MASK-HEAD-MISMATCH: FAIL-FAST abort() after {streak} sustained mismatch frames (IoU={iou}%) -- repro diagnostic stop"
                ));
                std::process::abort();
            }
        } else {
            PROFILE_MASK_HEAD_MISMATCH_STREAK.store(0, Ordering::SeqCst);
        }
    }
}

/// IoU (0..100) of the depth cutout's KEPT region (mask==0) vs the colour's OWN head (pixels whose colour
/// is clearly far from the corner background). ~high when the mask matches the head; low when a fresh mask
/// of the WRONG depth silhouette is applied to this head (the 2nd-character desync). Subsampled by 2 for
/// cost; 100 (perfect) on any degenerate input so it never false-trips.
fn mask_head_iou(mask: &[u8], cpx: &[u8], w: usize, h: usize) -> usize {
    if w < 16 || h < 16 || cpx.len() < w * h * 4 || mask.len() < w * h {
        return 100;
    }
    let (mut br, mut bgc, mut bb, mut bn) = (0u64, 0u64, 0u64, 0u64);
    for &(ox, oy) in &[(0usize, 0usize), (w - 8, 0), (0, h - 8), (w - 8, h - 8)] {
        for yy in oy..oy + 8 {
            for xx in ox..ox + 8 {
                let p = (yy * w + xx) * 4;
                br += cpx[p] as u64;
                bgc += cpx[p + 1] as u64;
                bb += cpx[p + 2] as u64;
                bn += 1;
            }
        }
    }
    if bn == 0 {
        return 100;
    }
    let (br, bgc, bb) = ((br / bn) as i32, (bgc / bn) as i32, (bb / bn) as i32);
    // Foreground = clearly far from the background colour (sum-abs channel distance, 0..765).
    const FG_DIST: i32 = 90;
    let (mut inter, mut union) = (0u64, 0u64);
    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let i = y * w + x;
            let p = i * 4;
            let kept = mask[i] == 0; // mask==1 => background/cut; ==0 => keep (the cutout foreground)
            let dist = (cpx[p] as i32 - br).abs()
                + (cpx[p + 1] as i32 - bgc).abs()
                + (cpx[p + 2] as i32 - bb).abs();
            let head = dist > FG_DIST;
            if kept || head {
                union += 1;
                if kept && head {
                    inter += 1;
                }
            }
            x += 2;
        }
        y += 1;
    }
    if union == 0 {
        100
    } else {
        (inter * 100 / union) as usize
    }
}

/// Read back the offscreen depth plane and compute the per-pixel background mask (1 = background/cut, 0 =
/// keep) via the bimodal-gap threshold. Returns `None` when the depth buffer has no content this frame or
/// no separable gap (the caller then reuses the cached mask). Emits the one-shot `depth-key` diagnostic
/// (success) and a separate one-shot skip diagnostic (dims mismatch / no gap) so both are visible once.
unsafe fn compute_depth_mask(gpu_child: usize, w: usize, h: usize) -> Option<Vec<u8>> {
    // Prefer the depth captured COHERENTLY with this tick's color (same single fence -- bug #3 fix);
    // fall back to a fresh, separately-fenced depth read when the coherent path was unavailable.
    let (dw, dh, depth, depth_cand) = match take_coherent_depth() {
        Some(d) => d,
        None => unsafe { readback_depth_fast(gpu_child) }?,
    };
    if dw as usize != w || dh as usize != h || depth.len() < w * h {
        // At higher-res (1024) the depth sibling MUST match the color RT or the mask can't align.
        if DEPTH_KEY_NOGAP_LOGGED.swap(1, Ordering::SeqCst) == 0 {
            append_autoload_debug(format_args!(
                "depth-key: SKIP dims-mismatch color={w}x{h} depth={dw}x{dh} depthlen={}",
                depth.len()
            ));
        }
        return None;
    }
    let idx = |x: usize, y: usize| y * w + x;
    // Frame depth extent over finite values.
    let mut dmin = f32::INFINITY;
    let mut dmax = f32::NEG_INFINITY;
    for &d in depth.iter().take(w * h) {
        if d.is_finite() {
            dmin = dmin.min(d);
            dmax = dmax.max(d);
        }
    }
    let range = dmax - dmin;

    // FOREGROUND reference = the CENTERED head's depth: sample a small central patch and take its median.
    // We KEEP the cluster this belongs to and CUT the other -- so the cut is robust to the engine's Z
    // direction (we never assume near/far == 0). The portrait's head+shoulders are the high-count
    // foreground cluster; the dark IBL surround is the other cluster, separated by an empty depth GAP
    // (the "air" between the model and the backdrop).
    let cr = (w.min(h) / 16).max(2);
    let (cx, cy) = (w / 2, h / 2);
    let mut cpatch: Vec<f32> = Vec::new();
    let mut yy = cy.saturating_sub(cr);
    while yy <= (cy + cr).min(h.saturating_sub(1)) {
        let mut xx = cx.saturating_sub(cr);
        while xx <= (cx + cr).min(w.saturating_sub(1)) {
            let d = depth[idx(xx, yy)];
            if d.is_finite() {
                cpatch.push(d);
            }
            xx += 1;
        }
        yy += 1;
    }
    cpatch.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let center = if cpatch.is_empty() {
        f32::NAN
    } else {
        cpatch[cpatch.len() / 2]
    };

    // Histogram over [dmin,dmax]; the threshold is the midpoint of the WIDEST run of near-empty bins --
    // i.e. the gap between the background cluster and the foreground cluster.
    const NB: usize = 128;
    let mut hist = [0u32; NB];
    if range > 0.0 {
        for &d in depth.iter().take(w * h) {
            if d.is_finite() {
                let bi = ((((d - dmin) / range) * NB as f32) as isize).clamp(0, NB as isize - 1);
                hist[bi as usize] += 1;
            }
        }
    }
    // A bin is "empty" if it holds < ~0.05% of the frame (tolerates a few depth-edge stragglers).
    let empty_thresh = ((w * h) / 2000).max(1) as u32;
    let (mut best_lo, mut best_len) = (0usize, 0usize);
    let (mut cur_lo, mut cur_len) = (0usize, 0usize);
    for b in 0..NB {
        if hist[b] <= empty_thresh {
            if cur_len == 0 {
                cur_lo = b;
            }
            cur_len += 1;
            if cur_len > best_len {
                best_len = cur_len;
                best_lo = cur_lo;
            }
        } else {
            cur_len = 0;
        }
    }
    let gap_mid_bin = best_lo as f32 + best_len as f32 * 0.5;
    let threshold = dmin + (gap_mid_bin / NB as f32) * range;
    // Require a REAL gap (>= ~4% of the range empty) and a valid center, else no separable background
    // this frame -> return None (caller reuses the cached mask; a unimodal depth would slice the head).
    let have_gap = range > 0.0 && (best_len as f32 / NB as f32) >= 0.04 && center.is_finite();
    // Keep the side of the gap the centered head sits on; the other side is the background.
    let keep_high = center > threshold;
    let mut mask = vec![0u8; w * h];
    let mut masked = 0usize;
    if have_gap {
        for i in 0..(w * h) {
            let d = depth[i];
            let is_bg = if keep_high {
                d < threshold
            } else {
                d > threshold
            };
            if is_bg {
                mask[i] = 1;
                masked += 1;
            }
        }
    }
    // DEGENERATE-MASK REJECTION (er-effects-rs-hi2): a fresh mask cutting under the publish floor is
    // not a real bg/head separation -- accepting it used to CACHE it, and every later gapless frame
    // reused the poisoned cache, so a whole window sat in the lowmask band (run 2026-07-03 ~21:17,
    // window slot4: lowmask=203 clean=0, prior head stuck on screen ~30s). Reject it (None) so the
    // cache keeps the last REAL mask and later frames retry fresh.
    let share_pct = masked * 100 / (w * h).max(1);
    let degenerate = !have_gap || share_pct < PORTRAIT_MIN_TRANSPARENT_PCT;
    let first_diag = DEPTH_KEY_DIAG_LOGGED.swap(1, Ordering::SeqCst) == 0;
    let deg_n = if degenerate {
        DEPTH_KEY_DEGENERATE.fetch_add(1, Ordering::SeqCst)
    } else {
        0
    };
    // Diagnostic: once at first-ever mask, then throttled on every degenerate frame -- the depth
    // picture of a collapsing window is the evidence the one-shot boot diag never captured.
    if first_diag || (degenerate && deg_n % 64 == 0) {
        let inset = (w.min(h) / 32).max(2);
        let (tl, tr, bl, br) = (
            depth[idx(inset, inset)],
            depth[idx(w - 1 - inset, inset)],
            depth[idx(inset, h - 1 - inset)],
            depth[idx(w - 1 - inset, h - 1 - inset)],
        );
        append_autoload_debug(format_args!(
            "depth-key: {w}x{h} min={dmin} max={dmax} center(head)={center} corners[tl={tl} tr={tr} bl={bl} br={br}] gap[bins {best_lo}..+{best_len}/{NB}] thr={threshold} keep_high={keep_high} have_gap={have_gap} masked={masked}/{} ({share_pct}%) degenerate={degenerate} deg_n={deg_n}",
            w * h,
        ));
    }
    if !degenerate {
        // A clean bimodal bg/head separation confirms this depth buffer belongs to OUR portrait scene --
        // pin its candidate so later scans can't drift to another slot's same-size depth sibling.
        PROFILE_DEPTH_PIN.store(depth_cand, Ordering::SeqCst);
        return Some(mask);
    }
    // SECOND PASS (backdrop-geometry recovery -- er-effects-rs-hi2 root fix, runs 2026-07-03).
    // Some characters' portrait scenes render backdrop GEOMETRY at a depth just behind the head
    // (observed: box ~0.0199 vs head ~0.0210) plus a sliver of true cleared depth (exact dmin=0).
    // The widest histogram gap is then the WRONG gap (cleared..geometry, bins 1..112), so the "bg"
    // cut is only the cleared sliver (~0%) and the whole window starves (slot4 Speed Bean
    // unkeyed=204, slot9 Moonsent Bean unkeyed=260). Excluding the BIT-EXACT extreme values (the
    // clear planes) and re-running the same histogram over the interior geometry makes the real
    // head/backdrop air gap dominant; excluded extremes are then classified by the same threshold
    // (cleared lands on the bg side). Only runs when the validated first pass failed.
    if range > 0.0 && center.is_finite() {
        let dmin_bits = dmin.to_bits();
        let dmax_bits = dmax.to_bits();
        let interior =
            |d: f32| d.is_finite() && d.to_bits() != dmin_bits && d.to_bits() != dmax_bits;
        let mut imin = f32::INFINITY;
        let mut imax = f32::NEG_INFINITY;
        for &d in depth.iter().take(w * h) {
            if interior(d) {
                imin = imin.min(d);
                imax = imax.max(d);
            }
        }
        let irange = imax - imin;
        if irange > 0.0 {
            let mut hist2 = [0u32; NB];
            for &d in depth.iter().take(w * h) {
                if interior(d) {
                    let bi =
                        ((((d - imin) / irange) * NB as f32) as isize).clamp(0, NB as isize - 1);
                    hist2[bi as usize] += 1;
                }
            }
            // VALLEY, not empty run (run 7: the model's own body fills intermediate depths, so no
            // EMPTY bins exist between the backdrop and head clusters at any binning -- the empty-
            // run second pass never fired). A LOW-DENSITY run (<= 0.5% of the frame per bin, vs the
            // first pass's 0.05%) between the two dominant clusters is the real separator; min run
            // 2% of bins so single noisy bins can't split a cluster.
            let low_thresh = ((w * h) / 200).max(1) as u32;
            let (mut b_lo, mut b_len) = (0usize, 0usize);
            let (mut c_lo, mut c_len) = (0usize, 0usize);
            for b in 0..NB {
                if hist2[b] <= low_thresh {
                    if c_len == 0 {
                        c_lo = b;
                    }
                    c_len += 1;
                    if c_len > b_len {
                        b_len = c_len;
                        b_lo = c_lo;
                    }
                } else {
                    c_len = 0;
                }
            }
            let thr2 = imin + ((b_lo as f32 + b_len as f32 * 0.5) / NB as f32) * irange;
            if (b_len as f32 / NB as f32) >= 0.02 {
                let keep_high2 = center > thr2;
                let mut mask2 = vec![0u8; w * h];
                let mut masked2 = 0usize;
                for i in 0..(w * h) {
                    let d = depth[i];
                    let is_bg = if keep_high2 { d < thr2 } else { d > thr2 };
                    if is_bg {
                        mask2[i] = 1;
                        masked2 += 1;
                    }
                }
                let share2 = masked2 * 100 / (w * h).max(1);
                if share2 >= PORTRAIT_MIN_TRANSPARENT_PCT {
                    let n = DEPTH_KEY_SECOND_PASS.fetch_add(1, Ordering::SeqCst);
                    if n % 64 == 0 {
                        append_autoload_debug(format_args!(
                            "depth-key: SECOND-PASS recovered mask -- interior[{imin},{imax}] gap[bins {b_lo}..+{b_len}/{NB}] thr={thr2} keep_high={keep_high2} masked={share2}% (first pass degenerate: clear-plane extremes excluded)"
                        ));
                    }
                    PROFILE_DEPTH_PIN.store(depth_cand, Ordering::SeqCst);
                    return Some(mask2);
                }
            }
            // Ground-truth dump, once per run, when even the valley pass fails: the compact
            // interior histogram (nonzero-bin runs) is the evidence for the NEXT split design.
            if DEPTH_KEY_HIST_DUMPED.swap(1, Ordering::SeqCst) == 0 {
                let mut runs = String::new();
                let mut b = 0usize;
                while b < NB {
                    if hist2[b] > 0 {
                        let lo = b;
                        let mut px = 0u64;
                        while b < NB && hist2[b] > 0 {
                            px += hist2[b] as u64;
                            b += 1;
                        }
                        runs.push_str(&format!(" [{lo}..{}]={px}", b - 1));
                    } else {
                        b += 1;
                    }
                }
                append_autoload_debug(format_args!(
                    "depth-key: VALLEY-FAIL interior[{imin},{imax}] low_thresh={low_thresh} nonzero-runs:{runs}"
                ));
            }
        }
    }
    None
}

/// Force the offscreen RT->SRV resolve in D3D12: CopyResource the render-target texture behind
/// `src_gpu_child` (the renderer's offscreen RT, which holds the rendered head) into the sampleable SRV
/// texture behind `dst_gpu_child` (offscreen+0x10's CSGxTexture, what the loading-screen forge binds and
/// GFx samples). The engine's own per-frame resolve almost never fires post-Continue (RT has content, SRV
/// stays black), so we do the copy ourselves every render-thread frame. Returns true on a completed copy
/// (or when src==dst so no copy is needed). Same safety contract as the readback: our OWN
/// queue/allocator/list/fence, game resources borrowed (never Released), never panics/crashes.
pub(crate) unsafe fn copy_offscreen_rt_to_srv(src_gpu_child: usize, dst_gpu_child: usize) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        copy_offscreen_rt_to_srv_inner(src_gpu_child, dst_gpu_child)
    }))
    .unwrap_or(false)
}

unsafe fn copy_offscreen_rt_to_srv_inner(src_gpu_child: usize, dst_gpu_child: usize) -> bool {
    // Resolve the SRV (dst) first from its OWN single-texture nest -> deterministic, plus its candidate
    // pointer. Then resolve the source as the largest texture in the offscreen nest EXCLUDING that SRV,
    // so we never pick the (black) SRV as the source and self-skip.
    let Some((dst, dst_v)) = (unsafe { find_d3d12_resource_ex(dst_gpu_child, 0, false, 0) }) else {
        return false;
    };
    // Prefer the pinned content RT for the source too, so the SRV the native forge samples is fed from
    // the same confirmed-ours texture the display publishes (never a foreign slot's RT).
    let rt_pin = PROFILE_RT_PIN.load(Ordering::SeqCst);
    let Some((src, _src_v)) =
        (unsafe { find_d3d12_resource_ex(src_gpu_child, dst_v, false, rt_pin) })
    else {
        return false;
    };
    // CopyResource requires identical dimensions + format.
    let sd: D3D12_RESOURCE_DESC = unsafe { src.GetDesc() };
    let dd: D3D12_RESOURCE_DESC = unsafe { dst.GetDesc() };
    // One-shot diagnostic: are src (RT) and dst (SRV) distinct resources, and do their descs match? If
    // find_d3d12_resource returns the SAME resource for both starts (RT==SRV by BFS), the copy is a
    // self-skip and can never populate the SRV -- the signature of the BFS "largest texture" ambiguity.
    if PROFILE_RT_SRV_COPY_DIAGGED.fetch_add(1, Ordering::SeqCst) < 6 {
        append_autoload_debug(format_args!(
            "rt-srv-copy-diag: src=0x{:x} dst=0x{:x} same={} src_dims={}x{} fmt={} dst_dims={}x{} fmt={}",
            src.as_raw() as usize,
            dst.as_raw() as usize,
            (src.as_raw() == dst.as_raw()) as u8,
            sd.Width,
            sd.Height,
            sd.Format.0,
            dd.Width,
            dd.Height,
            dd.Format.0,
        ));
    }
    // Same physical resource (already resolved / single texture) -> nothing to copy.
    if src.as_raw() == dst.as_raw() {
        return true;
    }
    if sd.Width != dd.Width || sd.Height != dd.Height || sd.Format != dd.Format {
        return false;
    }
    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { src.GetDevice(&mut device_opt) }.is_err() {
        return false;
    }
    let Some(device) = device_opt else {
        return false;
    };
    let queue_desc = D3D12_COMMAND_QUEUE_DESC {
        Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
        Priority: 0,
        Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
        NodeMask: 0,
    };
    let Ok(queue) = (unsafe { device.CreateCommandQueue::<ID3D12CommandQueue>(&queue_desc) })
    else {
        return false;
    };
    let Ok(allocator) = (unsafe {
        device.CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT)
    }) else {
        return false;
    };
    let Ok(list) = (unsafe {
        device.CreateCommandList::<_, _, ID3D12GraphicsCommandList>(
            0,
            D3D12_COMMAND_LIST_TYPE_DIRECT,
            &allocator,
            None,
        )
    }) else {
        return false;
    };
    // src (RT): COMMON -> COPY_SOURCE; dst (SRV): COMMON -> COPY_DEST; copy; both back to COMMON.
    unsafe {
        record_transition(
            &list,
            &src,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )
    };
    unsafe {
        record_transition(
            &list,
            &dst,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )
    };
    unsafe { list.CopyResource(&dst, &src) };
    unsafe {
        record_transition(
            &list,
            &src,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_RESOURCE_STATE_COMMON,
        )
    };
    unsafe {
        record_transition(
            &list,
            &dst,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_COMMON,
        )
    };
    if unsafe { list.Close() }.is_err() {
        return false;
    }
    let Ok(base_list) = list.cast::<ID3D12CommandList>() else {
        return false;
    };
    unsafe { queue.ExecuteCommandLists(&[Some(base_list)]) };
    let Ok(fence) = (unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }) else {
        return false;
    };
    if unsafe { queue.Signal(&fence, READBACK_FENCE_TARGET) }.is_err() {
        return false;
    }
    if unsafe { fence.GetCompletedValue() } < READBACK_FENCE_TARGET {
        let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else {
            return false;
        };
        if unsafe { fence.SetEventOnCompletion(READBACK_FENCE_TARGET, event) }.is_err() {
            let _ = unsafe { CloseHandle(event) };
            return false;
        }
        let wait = unsafe { WaitForSingleObject(event, READBACK_FENCE_WAIT_MS) };
        let _ = unsafe { CloseHandle(event) };
        if wait != WAIT_OBJECT_0 {
            return false;
        }
    }
    true
}

/// Upload tightly-packed RGBA8 `pixels` (`w`x`h`) into the TEXTURE2D found in `dst_gpu_child`'s nest --
/// overwriting the displayed now-loading background texture's pixels in place, so the Scaleform sprite
/// (which already registered that texture by name on the first bind) composites the real portrait without
/// any re-registration. Dims/format must match the destination (R8G8B8A8_UNORM, same w/h). Our own
/// upload heap + queue/list/fence; the game's resource is borrowed (never Released). Never panics.
pub(crate) unsafe fn upload_rgba_to_texture(
    dst_gpu_child: usize,
    w: u32,
    h: u32,
    pixels: &[u8],
) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        upload_rgba_to_texture_inner(dst_gpu_child, w, h, pixels)
    }))
    .unwrap_or(false)
}

unsafe fn upload_rgba_to_texture_inner(
    dst_gpu_child: usize,
    w: u32,
    h: u32,
    pixels: &[u8],
) -> bool {
    if pixels.len() < (w as usize) * (h as usize) * RGBA8_BPP {
        return false;
    }
    let Some(dst) = (unsafe { find_d3d12_resource(dst_gpu_child) }) else {
        return false;
    };
    let desc: D3D12_RESOURCE_DESC = unsafe { dst.GetDesc() };
    if desc.Width as u32 != w || desc.Height != h {
        return false; // dim mismatch -- caller must match (e.g. 1024x1024 checker <- 1024 portrait)
    }
    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { dst.GetDevice(&mut device_opt) }.is_err() {
        return false;
    }
    let Some(device) = device_opt else {
        return false;
    };
    // Copyable footprint of subresource 0 (256-aligned row pitch).
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut num_rows: u32 = 0;
    let mut row_size_bytes: u64 = 0;
    let mut total_bytes: u64 = 0;
    unsafe {
        device.GetCopyableFootprints(
            &desc,
            0,
            1,
            0,
            Some(&mut footprint),
            Some(&mut num_rows),
            Some(&mut row_size_bytes),
            Some(&mut total_bytes),
        )
    };
    if total_bytes == 0 || footprint.Footprint.RowPitch == 0 {
        return false;
    }
    // UPLOAD-heap buffer sized to the footprint; fill it with the RGBA rows at the 256-aligned pitch.
    let heap_props = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_UPLOAD,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    };
    let buffer_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: total_bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    };
    let mut upload_opt: Option<ID3D12Resource> = None;
    if unsafe {
        device.CreateCommittedResource(
            &heap_props,
            D3D12_HEAP_FLAG_NONE,
            &buffer_desc,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut upload_opt,
        )
    }
    .is_err()
    {
        return false;
    }
    let Some(upload) = upload_opt else {
        return false;
    };
    let row_pitch = footprint.Footprint.RowPitch as usize;
    let src_row = (w as usize) * RGBA8_BPP;
    let mut mapped: *mut c_void = std::ptr::null_mut();
    if unsafe { upload.Map(0, None, Some(&mut mapped)) }.is_err() || mapped.is_null() {
        return false;
    }
    let dstp = mapped as *mut u8;
    for y in 0..h as usize {
        let so = y * src_row;
        let d_o = y * row_pitch;
        if so + src_row > pixels.len() || (d_o + src_row) as u64 > total_bytes {
            break;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(pixels.as_ptr().add(so), dstp.add(d_o), src_row);
        }
    }
    unsafe { upload.Unmap(0, None) };
    // Record list: dst COMMON -> COPY_DEST, CopyTextureRegion(upload -> dst sub 0), dst back to COMMON.
    let queue_desc = D3D12_COMMAND_QUEUE_DESC {
        Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
        Priority: 0,
        Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
        NodeMask: 0,
    };
    let Ok(queue) = (unsafe { device.CreateCommandQueue::<ID3D12CommandQueue>(&queue_desc) })
    else {
        return false;
    };
    let Ok(allocator) = (unsafe {
        device.CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT)
    }) else {
        return false;
    };
    let Ok(list) = (unsafe {
        device.CreateCommandList::<_, _, ID3D12GraphicsCommandList>(
            0,
            D3D12_COMMAND_LIST_TYPE_DIRECT,
            &allocator,
            None,
        )
    }) else {
        return false;
    };
    unsafe {
        record_transition(
            &list,
            &dst,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )
    };
    let mut src_loc = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(upload.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    let mut dst_loc = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(dst.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    unsafe { list.CopyTextureRegion(&dst_loc, 0, 0, 0, &src_loc, None) };
    unsafe { ManuallyDrop::drop(&mut src_loc.pResource) };
    unsafe { ManuallyDrop::drop(&mut dst_loc.pResource) };
    unsafe {
        record_transition(
            &list,
            &dst,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_COMMON,
        )
    };
    if unsafe { list.Close() }.is_err() {
        return false;
    }
    let Ok(base_list) = list.cast::<ID3D12CommandList>() else {
        return false;
    };
    unsafe { queue.ExecuteCommandLists(&[Some(base_list)]) };
    let Ok(fence) = (unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }) else {
        return false;
    };
    if unsafe { queue.Signal(&fence, READBACK_FENCE_TARGET) }.is_err() {
        return false;
    }
    if unsafe { fence.GetCompletedValue() } < READBACK_FENCE_TARGET {
        let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else {
            return false;
        };
        if unsafe { fence.SetEventOnCompletion(READBACK_FENCE_TARGET, event) }.is_err() {
            let _ = unsafe { CloseHandle(event) };
            return false;
        }
        let wait = unsafe { WaitForSingleObject(event, READBACK_FENCE_WAIT_MS) };
        let _ = unsafe { CloseHandle(event) };
        if wait != WAIT_OBJECT_0 {
            return false;
        }
    }
    true
}

/// Create a persistent DEFAULT-heap R8G8B8A8 TEXTURE2D, upload `pixels` into it via a one-shot private
/// queue, and leave it in `COPY_SOURCE` so the per-frame composite can use it as a `CopyTextureRegion`
/// source. Returns the texture (the temp queue/allocator/list/upload-buffer are released here). `None` on
/// any failure -- never panics.
///
/// RETAINED FOR REFERENCE: the alpha-honoring composite now blends the portrait onto the backbuffer on the
/// CPU (see `blend_portrait_over_backbuffer`), so this GPU upload path is currently unused. Kept as the
/// proven RGBA->DEFAULT-heap upload for a future GPU-draw composite.
#[allow(dead_code)]
unsafe fn create_portrait_source_texture(
    device: &ID3D12Device,
    pw: u32,
    ph: u32,
    pixels: &[u8],
) -> Option<ID3D12Resource> {
    let tex_heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    };
    let tex_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: pw as u64,
        Height: ph,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    };
    let mut tex_opt: Option<ID3D12Resource> = None;
    if unsafe {
        device.CreateCommittedResource(
            &tex_heap,
            D3D12_HEAP_FLAG_NONE,
            &tex_desc,
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut tex_opt,
        )
    }
    .is_err()
    {
        return None;
    }
    let tex = tex_opt?;

    // Copyable footprint of subresource 0 (256-aligned row pitch).
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total_bytes: u64 = 0;
    unsafe {
        device.GetCopyableFootprints(
            &tex_desc,
            0,
            1,
            0,
            Some(&mut footprint),
            None,
            None,
            Some(&mut total_bytes),
        )
    };
    if total_bytes == 0 || footprint.Footprint.RowPitch == 0 {
        return None;
    }
    let buf_heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_UPLOAD,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    };
    let buf_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: total_bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    };
    let mut upload_opt: Option<ID3D12Resource> = None;
    if unsafe {
        device.CreateCommittedResource(
            &buf_heap,
            D3D12_HEAP_FLAG_NONE,
            &buf_desc,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut upload_opt,
        )
    }
    .is_err()
    {
        return None;
    }
    let upload = upload_opt?;
    let row_pitch = footprint.Footprint.RowPitch as usize;
    let src_row = (pw as usize) * RGBA8_BPP;
    let mut mapped: *mut c_void = std::ptr::null_mut();
    if unsafe { upload.Map(0, None, Some(&mut mapped)) }.is_err() || mapped.is_null() {
        return None;
    }
    let dstp = mapped as *mut u8;
    for y in 0..ph as usize {
        let so = y * src_row;
        let d_o = y * row_pitch;
        if so + src_row > pixels.len() || (d_o + src_row) as u64 > total_bytes {
            break;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(pixels.as_ptr().add(so), dstp.add(d_o), src_row);
        }
    }
    unsafe { upload.Unmap(0, None) };

    let queue_desc = D3D12_COMMAND_QUEUE_DESC {
        Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
        Priority: 0,
        Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
        NodeMask: 0,
    };
    let queue = unsafe { device.CreateCommandQueue::<ID3D12CommandQueue>(&queue_desc) }.ok()?;
    let allocator = unsafe {
        device.CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT)
    }
    .ok()?;
    let list = unsafe {
        device.CreateCommandList::<_, _, ID3D12GraphicsCommandList>(
            0,
            D3D12_COMMAND_LIST_TYPE_DIRECT,
            &allocator,
            None,
        )
    }
    .ok()?;
    // tex was created in COPY_DEST -- copy upload -> tex sub 0, then transition tex to COPY_SOURCE.
    let mut src_loc = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(upload.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    let mut dst_loc = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(tex.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    unsafe { list.CopyTextureRegion(&dst_loc, 0, 0, 0, &src_loc, None) };
    unsafe { ManuallyDrop::drop(&mut src_loc.pResource) };
    unsafe { ManuallyDrop::drop(&mut dst_loc.pResource) };
    unsafe {
        record_transition(
            &list,
            &tex,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )
    };
    if unsafe { list.Close() }.is_err() {
        return None;
    }
    let base_list = list.cast::<ID3D12CommandList>().ok()?;
    unsafe { queue.ExecuteCommandLists(&[Some(base_list)]) };
    let fence = unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }.ok()?;
    if unsafe { queue.Signal(&fence, READBACK_FENCE_TARGET) }.is_err() {
        return None;
    }
    if unsafe { fence.GetCompletedValue() } < READBACK_FENCE_TARGET {
        let event = unsafe { CreateEventW(None, false, false, None) }.ok()?;
        let _ = unsafe { fence.SetEventOnCompletion(READBACK_FENCE_TARGET, event) };
        let wait = unsafe { WaitForSingleObject(event, READBACK_FENCE_WAIT_MS) };
        let _ = unsafe { CloseHandle(event) };
        if wait != WAIT_OBJECT_0 {
            return None;
        }
    }
    Some(tex)
}
