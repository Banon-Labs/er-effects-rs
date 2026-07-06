
/// Q4 keepalive oracle: read the GX render-pass queue head/tail (non-destructively -- NO pop) to detect
/// whether a GX pass is queued this frame (the precondition the offscreen draw checks via FUN_1419e5850).
/// g_GxDrawContext may be a pointer-global (heap ctx) or the struct itself; resolve defensively and fall
/// back to the global address. All reads fault-guarded.
unsafe fn profile_gx_queue_sample(base: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let global = base + GX_DRAW_CONTEXT_RVA;
    let readable = |c: usize| {
        valid(c)
            && unsafe { safe_read_usize(c + GX_DRAW_CONTEXT_QUEUE_HEAD_OFFSET) }.is_some()
            && unsafe { safe_read_usize(c + GX_DRAW_CONTEXT_QUEUE_TAIL_OFFSET) }.is_some()
    };
    // Primary: g_GxDrawContext holds the context pointer (the game passes it directly as the ctx base).
    let mut ctx = unsafe { safe_read_usize(global) }.unwrap_or(0);
    if !readable(ctx) {
        ctx = global; // fallback: the global IS the context struct
    }
    if !readable(ctx) {
        return;
    }
    let head = unsafe { safe_read_usize(ctx + GX_DRAW_CONTEXT_QUEUE_HEAD_OFFSET) }.unwrap_or(0);
    let tail = unsafe { safe_read_usize(ctx + GX_DRAW_CONTEXT_QUEUE_TAIL_OFFSET) }.unwrap_or(0);
    PROFILE_GX_QUEUE_SAMPLES.fetch_add(1, Ordering::SeqCst);
    if head != tail {
        PROFILE_GX_QUEUE_NONEMPTY.fetch_add(1, Ordering::SeqCst);
    }
    // LOOK-BEFORE-BUILD: directly measure the GX subcontext pool's FREE depth this frame to settle whether
    // the ~4x head refresh is pool contention (pop fails 96%) or a readback/rasterize sync race. free =
    // (top - floor)/8; >0 means a subcontext is poppable. A min-free > 0 across the whole loading screen
    // refutes the contention theory (the pop never fails -> the black RT is a sync/rasterize problem).
    let floor = unsafe { safe_read_usize(ctx + GX_DRAW_CONTEXT_POOL_FLOOR_OFFSET) }.unwrap_or(0);
    let top = unsafe { safe_read_usize(ctx + GX_DRAW_CONTEXT_POOL_TOP_OFFSET) }.unwrap_or(0);
    if floor != 0 && top >= floor {
        let free = (top - floor) / 8;
        PROFILE_GX_POOL_FREE_LAST.store(free, Ordering::SeqCst);
        // monotonic min (CAS loop; only ever lowers)
        let mut cur = PROFILE_GX_POOL_FREE_MIN.load(Ordering::SeqCst);
        while free < cur {
            match PROFILE_GX_POOL_FREE_MIN.compare_exchange(
                cur,
                free,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(observed) => cur = observed,
            }
        }
    }
    let mask = unsafe { safe_read_usize(ctx + GX_DRAW_CONTEXT_POOL_USED_MASK_OFFSET) }.unwrap_or(0)
        & 0xffff_ffff;
    if mask != 0 {
        PROFILE_GX_POOL_USED_MASK.store(mask, Ordering::SeqCst);
    }
}

/// Pixel oracle sample: scan for the FIRST slot whose model is currently live (model_ins present), read
/// back its offscreen RT (AFTER the draw step) and record nonblack + whether the content hash changed vs
/// the previous sample OF THE SAME SLOT. Sampling the live slot (not a fixed one) is required because the
/// engine keeps barely one menu model built at a time (cycling); "changed" is gated to same-slot so a
/// slot switch (different character) is not mistaken for head motion. Only does the (costly) readback when
/// a live model exists, so it is free when none is present. Read-only + fault-guarded.
unsafe fn profile_lookat_rt_sample(base: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let mut chosen = usize::MAX;
    let mut off = 0usize;
    // Prefer the POST-Continue spared renderer (the persistent model) when it is set + live; it is not in
    // the menu table, so the table scan below would miss it. Use a dedicated sample index (10) so the
    // same-slot "changed" gate treats it as its own stream.
    let spared = LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst);
    if valid(spared)
        && unsafe { safe_read_usize(spared) }.unwrap_or(0)
            == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
    {
        let model =
            unsafe { safe_read_usize(spared + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
        if valid(model) {
            PROFILE_SPARED_MODEL_OK.fetch_add(1, Ordering::SeqCst);
            let o = unsafe {
                safe_read_usize(spared + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
            }
            .unwrap_or(0);
            if valid(o) {
                chosen = TITLE_PROFILE_SLOT_COUNT; // dedicated "spared" stream index
                off = o;
            }
        }
    }
    for s in (chosen == usize::MAX)
        .then_some(0..TITLE_PROFILE_SLOT_COUNT)
        .into_iter()
        .flatten()
    {
        let r =
            unsafe { safe_read_usize(portrait_renderer_table_entry(base, s as i32)) }.unwrap_or(0);
        if !valid(r)
            || unsafe { safe_read_usize(r) }.unwrap_or(0)
                != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
        {
            continue;
        }
        // model present?
        if !valid(unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0)) {
            continue;
        }
        let o = unsafe {
            safe_read_usize(r + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
        }
        .unwrap_or(0);
        if valid(o) {
            chosen = s;
            off = o;
            break;
        }
    }
    if chosen == usize::MAX {
        return; // no live model this frame -> no readback cost
    }
    let Some((w, h, px)) = (unsafe { readback_offscreen_rgba8(off) }) else {
        return;
    };
    PROFILE_LOOKAT_RT_SAMPLES.fetch_add(1, Ordering::SeqCst);
    if portrait_center_nonblack(w, h, &px) {
        PROFILE_LOOKAT_RT_NONBLACK.fetch_add(1, Ordering::SeqCst);
    }
    // ALPHA vs RGB: max RGB and max alpha over the same center region. If rgb_max>0 but alpha_max==0 the
    // RT has a portrait that GFx will composite as fully transparent (the "renders black despite content"
    // signature). Decides the color-space/alpha question without a screenshot.
    {
        let (wq, hq) = (w as usize, h as usize);
        if wq > 0 && hq > 0 && px.len() >= wq * hq * 4 {
            let (cx, cy) = (wq / 2, hq / 2);
            let (x0, x1) = (cx.saturating_sub(32), (cx + 32).min(wq));
            let (y0, y1) = (cy.saturating_sub(32), (cy + 32).min(hq));
            let (mut rgb_max, mut a_max) = (0u8, 0u8);
            for y in y0..y1 {
                for x in x0..x1 {
                    let idx = (y * wq + x) * 4;
                    rgb_max = rgb_max.max(px[idx]).max(px[idx + 1]).max(px[idx + 2]);
                    a_max = a_max.max(px[idx + 3]);
                }
            }
            PROFILE_LOOKAT_RT_RGB_MAX.store(rgb_max as usize, Ordering::SeqCst);
            PROFILE_LOOKAT_RT_ALPHA_MAX.store(a_max as usize, Ordering::SeqCst);
            // One-shot dump of the readback "content" RT (slot 100) on a frame where it actually has
            // content, so we can visually confirm whether it is the portrait or a scratch/world RT.
            if rgb_max > 24 && PROFILE_RT_CONTENT_DUMPED.swap(1, Ordering::SeqCst) == 0 {
                dump_portrait_rgba(100, w, h, &px);
            }
        }
    }
    // SAMPLEABLE-TEXTURE READBACK: read the texture actually BOUND into the now-loading container (what
    // GFx samples) and compare to the render target above. Same render-thread context (safe). If the RT
    // has content but this reads black, the bound CSGxTexture is a separate/unresolved resource.
    {
        let cap = LOADING_BG_TEXTURE_REDIRECT_LAST_PORTRAIT.load(Ordering::SeqCst);
        let mut bgx = 0usize;
        if valid(cap) {
            let container =
                unsafe { safe_read_usize(cap + TPF_FILE_CAP_TEX_RESCAP_OFFSET) }.unwrap_or(0);
            if valid(container) {
                let array =
                    unsafe { safe_read_usize(container + TPF_RESCAP_CONTAINER_ARRAY_OFFSET) }
                        .unwrap_or(0);
                if valid(array) {
                    let trc0 = unsafe { safe_read_usize(array) }.unwrap_or(0);
                    if valid(trc0) {
                        bgx = unsafe {
                            safe_read_usize(trc0 + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET)
                        }
                        .unwrap_or(0);
                    }
                }
            }
        }
        if valid(bgx) {
            if let Some((bw, bh, bpx)) = unsafe { readback_offscreen_rgba8(bgx) } {
                let (wq, hq) = (bw as usize, bh as usize);
                if wq > 0 && hq > 0 && bpx.len() >= wq * hq * 4 {
                    let (cx, cy) = (wq / 2, hq / 2);
                    let (x0, x1) = (cx.saturating_sub(32), (cx + 32).min(wq));
                    let (y0, y1) = (cy.saturating_sub(32), (cy + 32).min(hq));
                    let (mut rgb_max, mut a_max) = (0u8, 0u8);
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let idx = (y * wq + x) * 4;
                            rgb_max = rgb_max.max(bpx[idx]).max(bpx[idx + 1]).max(bpx[idx + 2]);
                            a_max = a_max.max(bpx[idx + 3]);
                        }
                    }
                    PROFILE_BOUND_GX_RGB_MAX.store(rgb_max as usize, Ordering::SeqCst);
                    PROFILE_BOUND_GX_ALPHA_MAX.store(a_max as usize, Ordering::SeqCst);
                    // One-shot dump of the bound SRV (slot 101) once we've also captured the content RT,
                    // so the two can be compared side by side (is the SRV black? is the RT the portrait?).
                    if PROFILE_RT_CONTENT_DUMPED.load(Ordering::SeqCst) != 0
                        && PROFILE_SRV_DUMPED.swap(1, Ordering::SeqCst) == 0
                    {
                        dump_portrait_rgba(101, bw, bh, &bpx);
                    }
                }
            }
        }
    }
    // Cheap strided FNV-1a hash of the RT to detect frame-to-frame content change without storing pixels.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let step = (px.len() / 4096).max(1);
    let mut i = 0;
    while i < px.len() {
        hash ^= px[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += step;
    }
    let h32 = (hash as usize) & 0xffff_ffff;
    let last_slot = PROFILE_LOOKAT_RT_LASTSLOT.swap(chosen, Ordering::SeqCst);
    let last_hash = PROFILE_LOOKAT_RT_LASTHASH.swap(h32, Ordering::SeqCst);
    // Count motion only when the same slot was sampled consecutively (so a slot switch isn't "motion").
    if last_slot == chosen && h32 != last_hash {
        PROFILE_LOOKAT_RT_CHANGED.fetch_add(1, Ordering::SeqCst);
    }
}

/// DRAW-PHASE SWEEP diagnostic, run from a FrameBegin task (ticks every frame). Throttled: (1) re-read
/// the live phase selector `er-effects-lookat-phase.txt` (a single integer index 0..LOOKAT_DRAW_PHASE_COUNT)
/// into `PROFILE_LOOKAT_SELECTED_PHASE` so the active draw phase can be switched without recompiling; and
/// (2) log each candidate phase's per-frame tick count + the draw count, so one run reveals which phases
/// actually tick per-frame at the menu (the world-gated GameSceneDraw does not). No-op unless look-at is on.
/// Walk the look-at resolution chain for a fixed probe slot (0) and bump per-stage validity counters, so
/// the sweep log pinpoints exactly which deref drops from ~100% to ~11% (instead of guessing). Read-only.
unsafe fn profile_lookat_stage_probe(base: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    PROFILE_LOOKAT_STAGE_OK[7].fetch_add(1, Ordering::SeqCst); // frames probed
    let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, 0)) }.unwrap_or(0);
    if !valid(r)
        || unsafe { safe_read_usize(r) }.unwrap_or(0)
            != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
    {
        return;
    }
    PROFILE_LOOKAT_STAGE_OK[0].fetch_add(1, Ordering::SeqCst);
    let model = unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
    if valid(model) {
        PROFILE_LOOKAT_STAGE_OK[1].fetch_add(1, Ordering::SeqCst);
    }
    let x = unsafe { safe_read_usize(r + PROFILE_LOOKAT_ANIM_LOCATION_OFFSET) }.unwrap_or(0);
    if !valid(x) {
        return;
    }
    PROFILE_LOOKAT_STAGE_OK[2].fetch_add(1, Ordering::SeqCst);
    let importer = unsafe { safe_read_usize(x + PROFILE_LOOKAT_IMPORTER_OFFSET) }.unwrap_or(0);
    if !valid(importer) {
        return;
    }
    PROFILE_LOOKAT_STAGE_OK[3].fetch_add(1, Ordering::SeqCst);
    let holder = importer + PROFILE_LOOKAT_POSEHOLDER_OFFSET;
    let skel = unsafe { safe_read_usize(holder + POSEHOLDER_SKELETON_OFFSET) }.unwrap_or(0);
    if !valid(skel) {
        return;
    }
    PROFILE_LOOKAT_STAGE_OK[4].fetch_add(1, Ordering::SeqCst);
    let local = unsafe { safe_read_usize(holder + POSEHOLDER_LOCAL_BONE_DATA_OFFSET) }.unwrap_or(0);
    if valid(local) {
        PROFILE_LOOKAT_STAGE_OK[5].fetch_add(1, Ordering::SeqCst);
    }
    let count = unsafe { safe_read_i32(skel + HKA_SKELETON_BONES_SIZE_OFFSET) }.unwrap_or(0);
    if count > 0 && count as usize <= LOOKAT_MAX_BONES {
        PROFILE_LOOKAT_STAGE_OK[6].fetch_add(1, Ordering::SeqCst);
    }
}

pub(crate) fn profile_lookat_phase_diag_tick() {
    if !portrait_overlay_enabled() {
        return;
    }
    if let Ok(base) = game_module_base() {
        unsafe { profile_lookat_stage_probe(base) };
    }
    let n = PROFILE_LOOKAT_PHASE_DIAG_COUNTER.fetch_add(1, Ordering::SeqCst);
    if n % 60 == 0 {
        // Live phase selector: a single integer in er-effects-lookat-phase.txt picks the active draw phase.
        let path = game_directory_path()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("er-effects-lookat-phase.txt");
        if let Ok(s) = std::fs::read_to_string(&path) {
            if let Ok(idx) = s.trim().parse::<usize>() {
                if idx < LOOKAT_DRAW_PHASE_COUNT {
                    PROFILE_LOOKAT_SELECTED_PHASE.store(idx, Ordering::SeqCst);
                }
            }
        }
        // Refresh the cached selftest flag here (throttled) so the draw task never does a per-frame stat.
        PROFILE_LOOKAT_SELFTEST_ON.store(false, Ordering::SeqCst);
        PROFILE_CURSOR_SWEEP_ON.store(false, Ordering::SeqCst);
    }
    if n % 240 == 0 {
        let ticks: Vec<String> = (0..LOOKAT_DRAW_PHASE_COUNT)
            .map(|i| {
                format!(
                    "{}={}",
                    LOOKAT_DRAW_PHASE_NAMES[i],
                    PROFILE_LOOKAT_PHASE_TICKS[i].load(Ordering::SeqCst)
                )
            })
            .collect();
        let stages: Vec<String> = (0..PROFILE_LOOKAT_STAGE_COUNT)
            .map(|i| {
                format!(
                    "{}={}",
                    PROFILE_LOOKAT_STAGE_NAMES[i],
                    PROFILE_LOOKAT_STAGE_OK[i].load(Ordering::SeqCst)
                )
            })
            .collect();
        append_autoload_debug(format_args!(
            "lookat-phase-sweep: frame_begin={n} selected={}({}) selftest={} nowload={} loadbuilds={} render_drives={} hook_hits={} gx[samples={} nonempty={}] gxpool[free_min={} free_last={} N(maskpop)={}] rt[samples={} nonblack={} changed={}] readback[some={} checker={} defer_some={} defer_nonblack={}] modeldraws={} spared[ptr=0x{:x} model_ok={} draws={} hits={}] stage0[{}] phase_ticks[{}]",
            PROFILE_LOOKAT_SELECTED_PHASE.load(Ordering::SeqCst),
            LOOKAT_DRAW_PHASE_NAMES[PROFILE_LOOKAT_SELECTED_PHASE.load(Ordering::SeqCst)],
            PROFILE_LOOKAT_SELFTEST_ON.load(Ordering::SeqCst) as u8,
            game_module_base()
                .map(|b| unsafe { now_loading_active(b) } as u8)
                .unwrap_or(0),
            PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RENDER_DRIVES.load(Ordering::SeqCst),
            PROFILE_LOOKAT_HOOK_HITS.load(Ordering::SeqCst),
            PROFILE_GX_QUEUE_SAMPLES.load(Ordering::SeqCst),
            PROFILE_GX_QUEUE_NONEMPTY.load(Ordering::SeqCst),
            {
                let m = PROFILE_GX_POOL_FREE_MIN.load(Ordering::SeqCst);
                if m == usize::MAX { -1i64 } else { m as i64 }
            },
            PROFILE_GX_POOL_FREE_LAST.load(Ordering::SeqCst),
            (PROFILE_GX_POOL_USED_MASK.load(Ordering::SeqCst) as u32).count_ones(),
            PROFILE_LOOKAT_RT_SAMPLES.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_NONBLACK.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_CHANGED.load(Ordering::SeqCst),
            PROFILE_READBACK_SOME.load(Ordering::SeqCst),
            PROFILE_READBACK_CHECKER.load(Ordering::SeqCst),
            PROFILE_READBACK_DEFERRED_SOME.load(Ordering::SeqCst),
            PROFILE_READBACK_DEFERRED_NONBLACK.load(Ordering::SeqCst),
            PROFILE_PERFRAME_MODEL_DRAWS.load(Ordering::SeqCst),
            LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst),
            PROFILE_SPARED_MODEL_OK.load(Ordering::SeqCst),
            PROFILE_PERFRAME_SPARED_DRAWS.load(Ordering::SeqCst),
            PROFILE_PERFRAME_HOOK_HITS.load(Ordering::SeqCst),
            stages.join(" "),
            ticks.join(" ")
        ));
    }
    // Dense post-Continue capture: the now-loading window between the teardown-spare and world-load is
    // only ~2s on a fast gold-save load, far shorter than the 240-tick coarse sweep above. Once a renderer
    // has actually been spared (LOADING_BG_PORTRAIT_SPARED_RENDERER != 0), emit a compact sweep every 20
    // ticks so the post-Continue rasterization (model_ok / rt changed) is sampled inside that brief window.
    if LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst) != 0 && n % 20 == 0 {
        let spared_ptr = LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst);
        // Raw live read of renderer+model_ins: distinguishes the field being ZEROED (renderer detached from
        // its model) from a DANGLING pointer (field intact but the model object behind it freed).
        let model_raw =
            unsafe { safe_read_usize(spared_ptr + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
        // Liveness probe of the model OBJECT captured at record-time: read its first qword (vtable). If
        // the object is still mapped/live its vtable reads as a plausible pointer; if freed/unmapped the
        // read fails (cap_vt=0). This decides whether re-attaching cap_model into renderer+0x778 could
        // restore the portrait (object alive) or whether the model must be rebuilt/refcounted (freed).
        let cap_model = PROFILE_SPARE_CANDIDATE_MODEL.load(Ordering::SeqCst);
        let cap_vt = unsafe { safe_read_usize(cap_model) }.unwrap_or(0);
        // Scan the (re)built profile table: how many of the 10 slots now hold a valid CSMenuProfModelRend
        // (built[r]) and how many of those have a live model_ins (built[m]). This is the DIRECT measure of
        // whether our own builder's fresh renderers are constructing + latching their own models post-
        // Continue -- independent of the spared (empty) renderer the rest of this line reports.
        let null = TITLE_OWNER_SCAN_START_ADDRESS;
        let (mut built_r, mut built_m) = (0u32, 0u32);
        if let Ok(b) = game_module_base() {
            for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
                let r =
                    unsafe { safe_read_usize(portrait_renderer_table_entry(b, s)) }.unwrap_or(0);
                if r != 0
                    && r != null
                    && unsafe { safe_read_usize(r) }.unwrap_or(0)
                        == b + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
                {
                    built_r += 1;
                    let m = unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                        .unwrap_or(0);
                    if m != 0 && m != null {
                        built_m += 1;
                    }
                }
            }
        }
        // CHAIN DIAGNOSTIC: for the autoload target slot's BUILT renderer, walk renderer -> +0xa8
        // (CSEzOffscreenRend) -> +0x10 (CSRuntimeTexResCap) -> +GX (CSGxTexture) -- the exact texture the
        // forge re-bind should publish. And read the bound container's CURRENT first-TexResCap GX. If
        // chain_gx != bound_gx, the re-bind is publishing the wrong (stale menu) texture, not our live RT.
        let (mut ch_r, mut ch_off, mut ch_trc, mut ch_gx, mut bound_gx) =
            (0usize, 0usize, 0usize, 0usize, 0usize);
        if let Ok(b) = game_module_base() {
            let slot = portrait_loaded_slot();
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(b, slot)) }.unwrap_or(0);
            if r != 0 && r != null {
                ch_r = r;
                ch_off = unsafe {
                    safe_read_usize(r + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
                }
                .unwrap_or(0);
                if ch_off != 0 && ch_off != null {
                    ch_trc = unsafe {
                        safe_read_usize(
                            ch_off + TITLE_CUSTOM_COVER_PROFILE_OFFSCREEN_TEX_RESCAP_OFFSET,
                        )
                    }
                    .unwrap_or(0);
                    if ch_trc != 0 && ch_trc != null {
                        ch_gx = unsafe {
                            safe_read_usize(
                                ch_trc + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET,
                            )
                        }
                        .unwrap_or(0);
                    }
                }
            }
            // Bound container's first TexResCap GX (what the loading screen actually samples).
            let cap = LOADING_BG_TEXTURE_REDIRECT_LAST_PORTRAIT.load(Ordering::SeqCst);
            if cap != 0 && cap != null {
                let container =
                    unsafe { safe_read_usize(cap + TPF_FILE_CAP_TEX_RESCAP_OFFSET) }.unwrap_or(0);
                if container != 0 && container != null {
                    let array =
                        unsafe { safe_read_usize(container + TPF_RESCAP_CONTAINER_ARRAY_OFFSET) }
                            .unwrap_or(0);
                    if array != 0 && array != null {
                        let trc0 = unsafe { safe_read_usize(array) }.unwrap_or(0);
                        if trc0 != 0 && trc0 != null {
                            bound_gx = unsafe {
                                safe_read_usize(
                                    trc0 + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET,
                                )
                            }
                            .unwrap_or(0);
                        }
                    }
                }
            }
        }
        append_autoload_debug(format_args!(
            "loading-portrait-chain: built_slot_r=0x{ch_r:x} off=0x{ch_off:x} trc=0x{ch_trc:x} chain_gx=0x{ch_gx:x} | bound_gx=0x{bound_gx:x} copies={} rt[rgb_max={} alpha_max={}] boundtex[rgb_max={} alpha_max={}]",
            PROFILE_RT_SRV_COPIES.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_RGB_MAX.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_ALPHA_MAX.load(Ordering::SeqCst),
            PROFILE_BOUND_GX_RGB_MAX.load(Ordering::SeqCst),
            PROFILE_BOUND_GX_ALPHA_MAX.load(Ordering::SeqCst),
        ));
        append_autoload_debug(format_args!(
            "lookat-pump-blocks: draws={} r_bad={} vt_bad={} off_bad={} multi={}",
            PROFILE_PERFRAME_MODEL_DRAWS.load(Ordering::SeqCst),
            PORTRAIT_PUMP_BLOCK_R.load(Ordering::SeqCst),
            PORTRAIT_PUMP_BLOCK_VTABLE.load(Ordering::SeqCst),
            PORTRAIT_PUMP_BLOCK_OFF.load(Ordering::SeqCst),
            PORTRAIT_PUMP_BLOCK_MULTI.load(Ordering::SeqCst),
        ));
        append_autoload_debug(format_args!(
            "lookat-spared-sweep: frame={n} nowload={} loadbuilds={} built[r={built_r} m={built_m}] rebind[n={} gx=0x{:x}] model_raw=0x{model_raw:x} cap_model=0x{cap_model:x} cap_vt=0x{cap_vt:x} spared[ptr=0x{:x} model_ok={} draws={} hits={}] rt[samples={} nonblack={} changed={}]",
            game_module_base()
                .map(|b| unsafe { now_loading_active(b) } as u8)
                .unwrap_or(0),
            PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst),
            LOADING_BG_LIVE_GX_REBINDS.load(Ordering::SeqCst),
            LOADING_BG_LIVE_GX_BOUND.load(Ordering::SeqCst),
            LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst),
            PROFILE_SPARED_MODEL_OK.load(Ordering::SeqCst),
            PROFILE_PERFRAME_SPARED_DRAWS.load(Ordering::SeqCst),
            PROFILE_PERFRAME_HOOK_HITS.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_SAMPLES.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_NONBLACK.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_CHANGED.load(Ordering::SeqCst),
        ));
    }
}

/// One candidate draw-phase task tick (registered once per phase index). Always bumps that phase's
/// per-frame tick counter (for the sweep), and drives the realtime look-at draw ONLY when this phase is
/// the selected active one -- so exactly one phase rasterizes per frame regardless of how many are registered.
pub(crate) unsafe fn profile_lookat_phase_draw_tick(phase_index: usize, task_data: &FD4TaskData) {
    if phase_index < LOOKAT_DRAW_PHASE_COUNT {
        PROFILE_LOOKAT_PHASE_TICKS[phase_index].fetch_add(1, Ordering::SeqCst);
    }
    if PROFILE_LOOKAT_SELECTED_PHASE.load(Ordering::SeqCst) != phase_index {
        return;
    }
    if let Ok(base) = game_module_base() {
        // Re-engage on every loading screen (subsequent-character-load fix): pause the draw/publish tick
        // ONLY during active gameplay, not permanently after the first world.
        if unsafe { portrait_pipeline_idle_in_gameplay(base) } {
            return;
        }
        unsafe { profile_lookat_realtime_draw_tick(base, task_data) };
    }
}

/// HOOK on the per-frame per-model PUSH task (deobf 0x140bba6e0). For our profile renderers, write the
/// cursor/sinusoid Head/Neck/Spine2 rotation into the importer PoseHolder (+ recompute its model-space)
/// BEFORE the original runs, so the original's submodel propagation (FUN_1409e9ac0) copies OUR pose into
/// every submodel's modelSpaceBoneData -- the buffer the GPU actually skins from -- using the engine's
/// own (correct) `frame` arg. This is the fix for "head doesn't move": our prior code wrote the importer
/// PoseHolder but never propagated to the submodels. Fires per model per frame (only when the model is
/// live), so it naturally tracks the engine's model build/teardown cycling.
pub(crate) unsafe extern "system" fn per_frame_push_hook(renderer: usize, frame: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // CAPTURE the engine's live render context (param_2/frame) on its OWN calls only (not our re-drives),
    // so our per-frame draw can enqueue the model into the SAME offscreen pass the engine routes to. Our
    // draw-phase task_data routes to the wrong pass -> nothing renders into the portrait RT.
    if !PROFILE_IN_OUR_DRIVE.load(Ordering::SeqCst) && frame != 0 && frame != null {
        PROFILE_DRAW_TASK_CTX.store(frame, Ordering::SeqCst);
        if PROFILE_DRAW_TASK_CTX_LOGGED.fetch_add(1, Ordering::SeqCst) < 3 {
            let dt = unsafe { safe_read_usize(frame + 8) }.unwrap_or(0);
            append_autoload_debug(format_args!(
                "draw-task-ctx: engine called draw task with frame=0x{frame:x} *(frame+8)=0x{dt:x} (delta-time bits) renderer=0x{renderer:x}"
            ));
        }
    }
    if portrait_overlay_enabled() && renderer != 0 && renderer != null {
        if let Ok(base) = game_module_base() {
            let vt_ok = unsafe { safe_read_usize(renderer) }.unwrap_or(0)
                == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA;
            if vt_ok {
                // Map renderer -> slot index (the look-at indices/base are cached per slot by the
                // FrameBegin apply_profile_lookat); skip if this renderer isn't in the profile table.
                let mut slot = usize::MAX;
                for s in 0..TITLE_PROFILE_SLOT_COUNT {
                    if unsafe { safe_read_usize(portrait_renderer_table_entry(base, s as i32)) }
                        .unwrap_or(0)
                        == renderer
                    {
                        slot = s;
                        break;
                    }
                }
                // Post-Continue the menu table is torn down, so the SPARED renderer isn't in it: map it to
                // its original autoload slot, whose cached look-at indices (base re-latches) we reuse.
                if slot == usize::MAX
                    && renderer == LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst)
                {
                    let own = portrait_loaded_slot();
                    if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&own) {
                        slot = own as usize;
                    }
                }
                if slot != usize::MAX {
                    if let Some(holder) = unsafe { profile_pose_holder(renderer) } {
                        let yaw =
                            f32::from_bits(PROFILE_LOOKAT_YAW_BITS.load(Ordering::SeqCst) as u32);
                        let pitch =
                            f32::from_bits(PROFILE_LOOKAT_PITCH_BITS.load(Ordering::SeqCst) as u32);
                        if unsafe { lookat_apply_realtime(holder, slot, yaw, pitch) } {
                            PROFILE_PERFRAME_HOOK_HITS.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
            }
        }
    }
    let orig = PROFILE_PERFRAME_HOOK_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, usize) = unsafe { core::mem::transmute(orig) };
        unsafe { f(renderer, frame) };
    }
}

fn install_per_frame_push_hook() {
    if PROFILE_PERFRAME_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "perframe-push-hook: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(target) = game_rva(PROFILE_PER_FRAME_PUSH_RVA as u32) else {
        return;
    };
    match unsafe { MhHook::new(target as *mut c_void, per_frame_push_hook as *mut c_void) } {
        Ok(hook) => {
            PROFILE_PERFRAME_HOOK_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_err() {
                append_autoload_debug(format_args!(
                    "perframe-push-hook: queue_enable failed for 0x{target:x}"
                ));
                return;
            }
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "perframe-push-hook: MhHook::new failed: {status:?}"
            ));
            return;
        }
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "perframe-push-hook: installed on per-frame push 0x{target:x} (submodel pose propagation)"
        )),
        status => append_autoload_debug(format_args!(
            "perframe-push-hook: MH_ApplyQueued failed: {status:?}"
        )),
    }
}

/// Rotate a vector by an `(x,y,z,w)` quaternion. Used for extracting the portrait model's face direction
/// from the Head bone's model-space orientation without depending on any screen-space visual heuristic.
fn quat_rotate_vec3(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let [x, y, z, w] = q;
    let [vx, vy, vz] = v;
    let tx = 2.0 * (y * vz - z * vy);
    let ty = 2.0 * (z * vx - x * vz);
    let tz = 2.0 * (x * vy - y * vx);
    [
        vx + w * tx + (y * tz - z * ty),
        vy + w * ty + (z * tx - x * tz),
        vz + w * tz + (x * ty - y * tx),
    ]
}

/// Read the model transform's horizontal facing yaw from the `CSMenuAsmModelRend` row-major matrix at
/// `renderer+0x900`. The model's face is its local `-Z`; the matrix stores basis vectors by column, so the
/// Z axis lives at row0.z/row1.z/row2.z. Identity -> face direction `(0,0,-1)` -> yaw 0.
unsafe fn profile_model_matrix_facing_yaw(renderer: usize) -> Option<f32> {
    let read_f32 = |off: usize| -> Option<f32> {
        unsafe { safe_read_i32(renderer + off) }.map(|b| f32::from_bits(b as u32))
    };
    let zx = read_f32(PROFILE_RENDERER_MODEL_MATRIX_OFFSET + 0x8)?;
    let zz = read_f32(PROFILE_RENDERER_MODEL_MATRIX_OFFSET + 0x28)?;
    if !(zx.is_finite() && zz.is_finite()) {
        return None;
    }
    if zx * zx + zz * zz < 0.0001 {
        return None;
    }
    Some(zx.atan2(zz))
}

unsafe fn resolve_head_bone_index(skel: usize, count: usize) -> Option<usize> {
    let bones = unsafe { safe_read_usize(skel + HKA_SKELETON_BONES_DATA_OFFSET) }.unwrap_or(0);
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if bones == 0 || bones == null {
        return None;
    }
    for i in 0..count.min(LOOKAT_MAX_BONES) {
        let name_ptr =
            unsafe { safe_read_usize(bones + i * HKA_BONE_STRIDE + HKA_BONE_NAME_OFFSET) }?
                & !1usize;
        let Some(name) = (unsafe { read_bone_name(name_ptr) }) else {
            continue;
        };
        if name.eq_ignore_ascii_case(LOOKAT_BONE_HEAD) {
            return Some(i);
        }
    }
    None
}

/// Prefer the live Head bone's model-space quaternion when the model has already built: it captures the
/// actual face direction of the rendered pose (including any native idle/root orientation). Fall back to
/// the renderer model matrix while the skeleton is not live yet.
unsafe fn profile_model_facing_yaw(renderer: usize) -> Option<f32> {
    let holder = unsafe { profile_pose_holder(renderer) }?;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let skel = unsafe { safe_read_usize(holder + POSEHOLDER_SKELETON_OFFSET) }.unwrap_or(0);
    if !valid(skel) {
        return unsafe { profile_model_matrix_facing_yaw(renderer) };
    }
    let count = unsafe { safe_read_i32(skel + HKA_SKELETON_BONES_SIZE_OFFSET) }.unwrap_or(0);
    if count <= 0 || count as usize > LOOKAT_MAX_BONES {
        return unsafe { profile_model_matrix_facing_yaw(renderer) };
    }
    let head = match unsafe { resolve_head_bone_index(skel, count as usize) } {
        Some(idx) => idx,
        None => return unsafe { profile_model_matrix_facing_yaw(renderer) },
    };
    let model = unsafe { safe_read_usize(holder + POSEHOLDER_MODEL_BONE_DATA_OFFSET) }.unwrap_or(0);
    if !valid(model) {
        return unsafe { profile_model_matrix_facing_yaw(renderer) };
    }
    let Some(mut q) = (unsafe { read_quat(model + head * BONE_DATA_STRIDE + BONE_DATA_Q_OFFSET) })
    else {
        return unsafe { profile_model_matrix_facing_yaw(renderer) };
    };
    let len2 = q.iter().map(|v| v * v).sum::<f32>();
    if !(len2.is_finite() && len2 > 0.0001) {
        return unsafe { profile_model_matrix_facing_yaw(renderer) };
    }
    let inv_len = len2.sqrt().recip();
    for v in &mut q {
        *v *= inv_len;
    }
    // Elden Ring c0000 faces local -Z in the portrait renderer: identity pose + yaw 0 already shows the
    // character front-on. Convert the rotated face vector into the camera-orbit yaw whose target->camera
    // vector is `(-sin(yaw), 0, -cos(yaw))`.
    let face = quat_rotate_vec3(q, [0.0, 0.0, -1.0]);
    let xz2 = face[0] * face[0] + face[2] * face[2];
    if !(xz2.is_finite() && xz2 > 0.0001) {
        return unsafe { profile_model_matrix_facing_yaw(renderer) };
    }
    Some((-face[0]).atan2(-face[2]))
}

unsafe fn latched_profile_model_facing_yaw(renderer: usize, idx: usize) -> f32 {
    if idx >= TITLE_PROFILE_SLOT_COUNT {
        return 0.0;
    }
    {
        let guard = match PROFILE_CAM_FACE_YAW.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(yaw) = guard[idx] {
            return yaw;
        }
    }
    let Some(yaw) = (unsafe { profile_model_facing_yaw(renderer) }) else {
        return 0.0;
    };
    if !yaw.is_finite() {
        return 0.0;
    }
    let mut guard = match PROFILE_CAM_FACE_YAW.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let yaw = *guard[idx].get_or_insert(yaw);
    PROFILE_CAM_FACE_YAW_LATCHED_MASK.fetch_or(1usize << idx, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "profile-camera: latched model-facing yaw slot={idx} yaw_rad={yaw:.4}"
    ));
    yaw
}

/// CAMERA LEVER: override one profile renderer's orbit camera with a custom viewport (closer, model-facing
/// framing), proving the lever on the still dump. Replicates the tail of the engine's own camera routine
/// `FUN_140bbe190` WITHOUT its `MenuOffscrRendParam` read (so it never clobbers our override): latch the
/// engine baseline once, write the orbit fields from `baseline + offsets`, rebuild the view matrix via
/// the engine builder, copy it into the renderer's matrix slot, then push the CSPersCam into the
/// offscreen render. Re-applied every tick so a refresh that re-runs the engine setup can't win.
/// `renderer` must already be a validated live CSMenuProfModelRend (vtable checked by the caller).
/// Returns true once the camera was pushed. See bd `camera-lever-RE-VERIFIED-offsets-and-call-addrs-2026-06-29`.
unsafe fn apply_profile_camera_override(base: usize, renderer: usize, slot: i32) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if renderer == 0 || renderer == null {
        return false;
    }
    let idx = slot as usize;
    if idx >= TITLE_PROFILE_SLOT_COUNT {
        return false;
    }
    let read_f32 = |off: usize| -> Option<f32> {
        unsafe { safe_read_i32(renderer + off) }.map(|b| f32::from_bits(b as u32))
    };
    // The push dereferences the offscreen-render pointer at renderer+0xa8; if it is not populated yet
    // (or has been torn down) skip entirely, so the engine push can never fault on a null offscreen.
    if unsafe {
        safe_read_usize(renderer + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
    }
    .unwrap_or(0)
        == 0
    {
        return false;
    }
    // Latch the engine baseline ONCE per slot, BEFORE the first override write, so all overrides derive
    // from an immutable baseline. The lock is never held across a game call.
    let baseline = {
        let mut guard = match PROFILE_CAM_BASELINE.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if guard[idx].is_none() {
            let (Some(tx), Some(ty), Some(tz), Some(dist), Some(pitch), Some(yaw), Some(fov)) = (
                read_f32(PROFILE_CAM_TARGET_OFFSET),
                read_f32(PROFILE_CAM_TARGET_OFFSET + 4),
                read_f32(PROFILE_CAM_TARGET_OFFSET + 8),
                read_f32(PROFILE_CAM_DISTANCE_OFFSET),
                read_f32(PROFILE_CAM_PITCH_OFFSET),
                read_f32(PROFILE_CAM_YAW_OFFSET),
                read_f32(PROFILE_CAM_FOV_OFFSET),
            ) else {
                return false;
            };
            // The engine frames the head at a real positive distance with a real fov, and the target /
            // angles are finite. If anything is not set yet (0 / NaN), skip latching this tick and retry
            // once the ctor camera setup has run -- so a degenerate baseline is never captured.
            if !(dist.is_finite()
                && dist > 0.001
                && fov.is_finite()
                && fov > 0.0
                && tx.is_finite()
                && ty.is_finite()
                && tz.is_finite()
                && pitch.is_finite()
                && yaw.is_finite())
            {
                return false;
            }
            guard[idx] = Some(ProfileCamBaseline {
                target: [tx, ty, tz],
                distance: dist,
                pitch,
                yaw,
                fov,
            });
            PROFILE_CAM_LATCHED_MASK.fetch_or(1usize << idx, Ordering::SeqCst);
        }
        guard[idx].unwrap()
    };
    // Custom viewport derived from the immutable baseline.
    let target = baseline.target;
    let distance = baseline.distance * PROFILE_CAM_DISTANCE_SCALE;
    let pitch = baseline.pitch + PROFILE_CAM_PITCH_DELTA_RAD;
    // FACING: the engine baseline.yaw (latched from the engine's param-derived camera) ALREADY frames the
    // model FRONT-on -- the natural profile render shows the face. The detected model-facing yaw is the
    // model's intrinsic orientation, which is REDUNDANT with that baseline: adding it (here ~-π) orbits the
    // camera a further ~180deg to the BACK of the head (observed calib-6: facing latched -3.14, render = back
    // of head at every cursor position). So do NOT add it to the camera yaw; keep the detection for the
    // telemetry/log only. (If a future renderer's baseline does NOT face front, revisit -- but our own-built
    // renderer inherits the engine's front-facing param camera.)
    let _facing_yaw = unsafe { latched_profile_model_facing_yaw(renderer, idx) };
    let yaw = baseline.yaw + PROFILE_CAM_YAW_DELTA_RAD;
    let fov = baseline.fov * PROFILE_CAM_FOV_SCALE;
    // Write the orbit fields (mirrors `FUN_140bbe190`'s field writes, minus the param read). The
    // renderer is a validated live object spanning well past +0xa24, so direct volatile writes are safe.
    unsafe {
        core::ptr::write_volatile(
            (renderer + PROFILE_CAM_TARGET_OFFSET) as *mut f32,
            target[0],
        );
        core::ptr::write_volatile(
            (renderer + PROFILE_CAM_TARGET_OFFSET + 4) as *mut f32,
            target[1],
        );
        core::ptr::write_volatile(
            (renderer + PROFILE_CAM_TARGET_OFFSET + 8) as *mut f32,
            target[2],
        );
        core::ptr::write_volatile((renderer + PROFILE_CAM_TARGET_W_OFFSET) as *mut f32, 1.0);
        core::ptr::write_volatile(
            (renderer + PROFILE_CAM_DISTANCE_OFFSET) as *mut f32,
            distance,
        );
        core::ptr::write_volatile((renderer + PROFILE_CAM_PITCH_OFFSET) as *mut f32, pitch);
        core::ptr::write_volatile((renderer + PROFILE_CAM_YAW_OFFSET) as *mut f32, yaw);
        core::ptr::write_volatile((renderer + PROFILE_CAM_FOV_OFFSET) as *mut f32, fov);
    }
    // Rebuild the view matrix with the engine's own builder (correct handedness/basis), then copy the 16
    // floats into the renderer's matrix slot (== the CSPersCam view matrix).
    let build: unsafe extern "system" fn(usize, *mut f32) -> *mut f32 =
        unsafe { core::mem::transmute(base + PROFILE_CAM_BUILD_MATRIX_RVA) };
    let mut matrix = [0f32; 16];
    unsafe { build(renderer, matrix.as_mut_ptr()) };
    if !matrix.iter().all(|f| f.is_finite()) {
        PROFILE_CAM_LAST_MATRIX_OK.store(0, Ordering::SeqCst);
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            matrix.as_ptr(),
            (renderer + PROFILE_CAM_VIEW_MATRIX_OFFSET) as *mut f32,
            16,
        );
    }
    // Push the CSPersCam into the offscreen render so the next offscreen frame uses our camera.
    let push: unsafe extern "system" fn(usize, usize) =
        unsafe { core::mem::transmute(base + PROFILE_CAM_PUSH_RVA) };
    unsafe { push(renderer, renderer + PROFILE_CAM_PERSCAM_OFFSET) };
    PROFILE_CAM_APPLY_CALLS.fetch_add(1, Ordering::SeqCst);
    PROFILE_CAM_LAST_SLOT.store(idx, Ordering::SeqCst);
    PROFILE_CAM_LAST_MATRIX_OK.store(1, Ordering::SeqCst);
    true
}
