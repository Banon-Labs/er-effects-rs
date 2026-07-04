
/// One-time setup for the per-frame composite: derive the device from the backbuffer, read the captured
/// portrait, build the persistent source texture + command allocator/list/fence + our OWN private DIRECT
/// queue. We do NOT submit on the game's command queue -- doing so from the Present hook caused a vkd3d
/// access violation; instead we CPU-fence-wait our copy to completion before the original Present runs.
/// Stores every object as a leaked raw pointer. `false` on any failure. The step logs localize a hardware
/// fault (catch_unwind cannot catch an access violation inside a D3D12 call).
unsafe fn init_overlay_draw_state(backbuffer: &ID3D12Resource) -> bool {
    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { backbuffer.GetDevice(&mut device_opt) }.is_err() {
        return false;
    }
    let Some(device) = device_opt else {
        return false;
    };

    let (pw, ph, pixels) = {
        let Ok(g) = LOADING_BG_PORTRAIT_RGBA.lock() else {
            return false;
        };
        match g.as_ref() {
            Some((w, h, px)) => (*w, *h, px.clone()),
            None => return false,
        }
    };
    if pw == 0 || ph == 0 || pw > MAX_RT_DIM || ph > MAX_RT_DIM {
        return false;
    }
    if pixels.len() < (pw as usize) * (ph as usize) * RGBA8_BPP {
        return false;
    }
    append_autoload_debug(format_args!(
        "present-overlay: draw init step1 device + portrait ok ({pw}x{ph}, {} bytes)",
        pixels.len()
    ));

    // NOTE: no GPU source texture is created here anymore -- the composite blends the portrait onto the
    // backbuffer on the CPU (readback region -> alpha-blend -> writeback), sourcing the pixels directly
    // from `LOADING_BG_PORTRAIT_RGBA` each frame. That is what lets the transparent (alpha-0) background
    // show the loading screen through; a raw CopyTextureRegion could not honor per-pixel alpha.
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
    // CreateCommandList returns the list OPEN; close it so the first per-frame `Reset` is valid.
    if unsafe { list.Close() }.is_err() {
        return false;
    }
    let Ok(fence) = (unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }) else {
        return false;
    };

    // Our OWN persistent DIRECT queue (the proven readback/upload pattern). We never touch the game's queue.
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
    append_autoload_debug(format_args!(
        "present-overlay: draw init step3 cmd objects + own queue ready"
    ));

    OVERLAY_ALLOCATOR.store(allocator.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_LIST.store(list.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_FENCE.store(fence.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_QUEUE.store(queue.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_PORTRAIT_W.store(pw as usize, Ordering::SeqCst);
    OVERLAY_PORTRAIT_H.store(ph as usize, Ordering::SeqCst);
    true
}

/// Composite the captured portrait onto the swapchain backbuffer. Called from the Present detour every
/// frame while the now-loading screen is up. `catch_unwind` + every COM call checked -> never panics or
/// crashes on the game's render thread; on any failure it draws nothing and returns `false`.
pub(crate) unsafe fn composite_portrait_on_swapchain(base: usize, swapchain_raw: usize) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        composite_portrait_inner(base, swapchain_raw)
    }))
    .unwrap_or(false)
}

/// Close the loading-portrait window: clear the published snapshot + the "have a head" gate so a later
/// window cannot flash the PREVIOUS character, drop the RT/depth candidate pins (the next window's
/// renderers are new objects), and clear the teardown-spared renderer so the NEXT load's teardown re-spares
/// the new character (LOADING_BG_PORTRAIT_SPARED_RENDERER is gated `== 0` and was otherwise never reset --
/// it stayed pinned to the first character's now-stale renderer, and driving that leaked renderer risks a
/// use-after-free). Called from the overlay stop at load completion; idempotent.
pub(crate) fn loading_portrait_window_reset(reason: &str) {
    // Make-before-break bridge (user 2026-07-03): KEEP the last published keyed frame + its
    // display-available flag so the just-loaded character stays on screen as the bridge when the NEXT
    // load window opens -- it is replaced the instant that window's newly-selected character produces
    // its own keyed frame (the drive re-engages because the freeze latch below is cleared). Previously
    // this nulled the snapshot to avoid flashing the previous character; that flash IS now the desired
    // behavior (old head held until the new masked head is ready), bounded by the keyed-publish gate.
    // Only the per-window drive-freeze latch is cleared here so the next window re-renders.
    PROFILE_BAKE_RGBA_CAPTURED.store(0, Ordering::SeqCst);
    PROFILE_RT_PIN.store(0, Ordering::SeqCst);
    PROFILE_DEPTH_PIN.store(0, Ordering::SeqCst);
    // Fresh adaptive tear baseline for the next window's character (honest content scores differ
    // per character: speckled textures sit ~40, smooth skin ~3).
    PROFILE_TEAR_EMA.store(0, Ordering::SeqCst);
    OVERLAY_NOW_LOADING_SEEN.store(0, Ordering::SeqCst);
    // Do NOT drop the spared renderer -- that leaked one live CSMenuProfModelRend per switch (it was
    // excluded from the native delete and its offscreen draw task kept filling the 192-slot GX
    // command queue -> 0x1aeaf05 overflow ~switch #4). MOVE it to the orphan slot; the game-thread
    // teardown-spare hook delete-enqueues it via CSDelayDeleteMan at the next teardown (this reset
    // runs off the game thread, so it stashes rather than deleting in place).
    let prev_spared = LOADING_BG_PORTRAIT_SPARED_RENDERER.swap(0, Ordering::SeqCst);
    if prev_spared != 0 {
        PROFILE_SPARE_ORPHAN.store(prev_spared, Ordering::SeqCst);
    }
    PROFILE_SPARE_CANDIDATE.store(0, Ordering::SeqCst);
    // Re-arm the idle-anim bind + drop the motion-metric history so the NEXT load window binds its
    // own renderer and starts a fresh inter-frame diff (cumulative attempt/max oracles are kept).
    PORTRAIT_ANIM_BIND_STATE.store(0, Ordering::SeqCst);
    PORTRAIT_ANIM_BOUND_RENDERER.store(0, Ordering::SeqCst);
    PORTRAIT_ANIM_BOUND_LOC.store(0, Ordering::SeqCst);
    PORTRAIT_KICK_SLOT_KEY.store(0, Ordering::SeqCst);
    PORTRAIT_KICK_RENDERER.store(0, Ordering::SeqCst);
    if let Ok(mut g) = PORTRAIT_MOTION_PREV_PLANES.lock() {
        *g = None;
    }
    if let Ok(mut g) = LAST_DEPTH_MASK.lock() {
        *g = None;
    }
    // Cache cleared -> forget which character it was for (a fresh compute re-tags it).
    LAST_DEPTH_MASK_INCARNATION.store(0, Ordering::SeqCst);
    // Animation-stall semaphore: snapshot this window's animated-vs-displayed frame counts, then zero
    // for the next window. drive << display == the head froze early (freeze-after-capture); the
    // user's "stopped animating / frozen the whole loading screen" symptom shows here as a low ratio.
    let drive = PROFILE_DRIVE_FRAMES_WINDOW.swap(0, Ordering::SeqCst);
    let display = PROFILE_DISPLAY_FRAMES_WINDOW.swap(0, Ordering::SeqCst);
    PROFILE_DRIVE_FRAMES_WINDOW_LAST.store(drive, Ordering::SeqCst);
    PROFILE_DISPLAY_FRAMES_WINDOW_LAST.store(display, Ordering::SeqCst);
    // PUBLISH-STARVATION ATTRIBUTION (2026-07-03 soak: windows froze on the PRIOR character with the
    // drive running ~1:1, so the starving class is publish-side and the cumulative oracles cannot say
    // WHICH window starved or WHY). Snapshot each publish/skip class per window (delta vs the previous
    // reset) so a frozen window names its own cause: published==0 with a dominant torn/unkeyed/multi
    // count is the starvation signature; pin_moves counts content-RT recreations inside the window.
    let winof = |cum: &AtomicUsize, last: &AtomicUsize| -> usize {
        let c = cum.load(Ordering::SeqCst);
        c.saturating_sub(last.swap(c, Ordering::SeqCst))
    };
    let published = winof(&PROFILE_PUBLISH_CLEAN, &PROFILE_PUBLISH_CLEAN_WINDOW_MARK);
    let torn = winof(
        &PROFILE_PUBLISH_SKIPPED_TORN,
        &PROFILE_PUBLISH_SKIPPED_TORN_WINDOW_MARK,
    );
    let unkeyed = winof(
        &PROFILE_PUBLISH_SKIPPED_UNKEYED,
        &PROFILE_PUBLISH_SKIPPED_UNKEYED_WINDOW_MARK,
    );
    let multi = winof(
        &PROFILE_MULTI_MODEL_PUBLISH_SKIPS,
        &PROFILE_MULTI_MODEL_PUBLISH_SKIPS_WINDOW_MARK,
    );
    let pin_moves = winof(
        &PROFILE_RT_PIN_SWITCHES,
        &PROFILE_RT_PIN_SWITCHES_WINDOW_MARK,
    );
    let fence_skips = winof(
        &PROFILE_DRIVE_FENCE_SKIPS,
        &PROFILE_DRIVE_FENCE_SKIPS_WINDOW_MARK,
    );
    // Source provenance per window: cb/cs = color ticks resolved from the scene bundle vs the scan;
    // dc/db = depth via the deterministic chain vs the BFS; unpaired = real frames held back for
    // lacking bundle provenance (the green-face wrong-buffer class). A starved window (clean=0)
    // with cs/db dominant convicts a chain miss for that window's renderer.
    let cb = winof(
        &PROFILE_COLOR_FROM_BUNDLE,
        &PROFILE_COLOR_FROM_BUNDLE_WINDOW_MARK,
    );
    let cs = winof(
        &PROFILE_COLOR_FROM_SCAN,
        &PROFILE_COLOR_FROM_SCAN_WINDOW_MARK,
    );
    let dc = winof(
        &PROFILE_DEPTH_FROM_CHAIN,
        &PROFILE_DEPTH_FROM_CHAIN_WINDOW_MARK,
    );
    let db = winof(&PROFILE_DEPTH_FROM_BFS, &PROFILE_DEPTH_FROM_BFS_WINDOW_MARK);
    let unpaired = winof(
        &PROFILE_PUBLISH_SKIPPED_UNPAIRED,
        &PROFILE_PUBLISH_SKIPPED_UNPAIRED_WINDOW_MARK,
    );
    let lowmask = winof(
        &PROFILE_PUBLISH_SKIPPED_LOWMASK,
        &PROFILE_PUBLISH_SKIPPED_LOWMASK_WINDOW_MARK,
    );
    // First-keyed latency: display-frame index of this window's first publish ('-' = never
    // published; the whole window rode the bridge). Snapshot + re-arm for the next window.
    let first_keyed = PROFILE_WINDOW_FIRST_KEYED_DISPLAY.swap(usize::MAX, Ordering::SeqCst);
    PROFILE_WINDOW_FIRST_KEYED_DISPLAY_LAST.store(
        if first_keyed == usize::MAX {
            0
        } else {
            first_keyed
        },
        Ordering::SeqCst,
    );
    let first_keyed_s = if first_keyed == usize::MAX {
        "-".to_owned()
    } else {
        first_keyed.to_string()
    };
    // Floor-evidence: min transparent share among floor-passing frames vs max among lowmask-held
    // frames this window ('-' = no frame in that class). Sets PORTRAIT_MIN_TRANSPARENT_PCT from data.
    let share_min = PROFILE_PUBLISH_SHARE_MIN.swap(usize::MAX, Ordering::SeqCst);
    let share_min_s = if share_min == usize::MAX {
        "-".to_owned()
    } else {
        share_min.to_string()
    };
    let held_max = PROFILE_LOWMASK_SHARE_MAX.swap(0, Ordering::SeqCst);
    let checker = winof(
        &PROFILE_READBACK_CHECKER,
        &PROFILE_READBACK_CHECKER_WINDOW_MARK,
    );
    let badiou = winof(
        &PROFILE_PUBLISH_SKIPPED_BADIOU,
        &PROFILE_PUBLISH_SKIPPED_BADIOU_WINDOW_MARK,
    );
    append_autoload_debug(format_args!(
        "present-overlay: loading-portrait window reset ({reason}) -- animated {drive} / displayed {display} frames (drive<<display == froze early); publish[clean={published} torn={torn} unkeyed={unkeyed} lowmask={lowmask} badiou={badiou} checker={checker} multi={multi} pin_moves={pin_moves} fence_skips={fence_skips} unpaired={unpaired} first_keyed={first_keyed_s}] share[pass_min={share_min_s} held_max={held_max}] src[color bundle={cb}/scan={cs} depth chain={dc}/bfs={db}] (clean=0 == frozen on prior character; the dominant skip class is the cause); pins/spare cleared for the next load"
    ));
}

/// Invalidate the depth-key MASKING PLANE for a NEW model: drop the cached mask and the pinned depth
/// candidate so the next `apply_depth_alpha_key` RECOMPUTES the silhouette from the new model's own depth
/// buffer instead of reusing the previous character's cached mask. Without this, a System Quit -> Load
/// Profile character switch would cut the OLD character's silhouette out of the NEW head until fresh depth
/// happened to land. Fail-open in the gap (leaves the head opaque) -- never a stale wrong-shape cutout.
pub(crate) fn invalidate_portrait_depth_mask() {
    PROFILE_DEPTH_PIN.store(0, Ordering::SeqCst);
    if let Ok(mut g) = LAST_DEPTH_MASK.lock() {
        *g = None;
    }
    // Cache cleared -> forget which character it was for (a fresh compute re-tags it).
    LAST_DEPTH_MASK_INCARNATION.store(0, Ordering::SeqCst);
}

unsafe fn composite_portrait_inner(base: usize, swapchain_raw: usize) -> bool {
    // LOADING-SCREEN WINDOW gate. The head composites while the map is LOADING (`!load_done`, the corrected
    // signal) and pops the instant the load COMPLETES (`load_done` false->true). IN_WORLD_REACHED is never a
    // stop -- it latches while the loading screen is still up (PlayerIns lives through it), the premature-pop
    // bug. The captured-head snapshot (PROFILE_BAKE_RGBA_CAPTURED, cleared only at the stop) persists even
    // after the profile renderers tear down, so the head stays on screen (frozen if the renderers are gone,
    // tracking while alive) for the whole load -- never blanks mid-load, never lingers into gameplay.
    // CORRECTED SIGNAL (RE 2026-07-02, CSNowLoadingHelperImp::Update decompile): `now_loading_active`
    // reads `load_done` -- a load-COMPLETE latch that is FALSE while the map loads and TRUE once it finishes
    // (and lingers into gameplay). So "still on the loading screen" is `!load_done`. The head must show
    // while loading and pop the instant the load COMPLETES (load_done false->true), NOT when load_done later
    // drops (that only happens on the NEXT load -> the head-persists-into-gameplay bug). `fake_vis` (the
    // CSFakeLoadingScreenImp black plate) is a secondary "still covered" signal that also means loading.
    let fake_vis = unsafe { fake_loading_screen_visible(base) };
    let load_done = unsafe { now_loading_active(base) };
    let loading = !load_done || fake_vis;
    let loading_seen = if loading {
        OVERLAY_BRIDGE_PRESENTS.store(0, Ordering::SeqCst);
        OVERLAY_NOW_LOADING_SEEN.store(1, Ordering::SeqCst);
        true
    } else {
        OVERLAY_NOW_LOADING_SEEN.load(Ordering::SeqCst) != 0
    };
    if OVERLAY_STOPPED.load(Ordering::SeqCst) != 0 {
        // Stopped: re-arm ONLY on evidence of a NEW loading window -- loading reasserting, or a fresh
        // post-Continue table build (the System Quit character-switch reload path). Reset the seen latch so
        // the previous window can't instant-stop this one.
        let rebuilt = PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst)
            > OVERLAY_STOP_TABLE_BUILDS.load(Ordering::SeqCst);
        if !(loading || rebuilt) {
            return false;
        }
        OVERLAY_STOPPED.store(0, Ordering::SeqCst);
        OVERLAY_NOW_LOADING_SEEN.store(if loading { 1 } else { 0 }, Ordering::SeqCst);
        OVERLAY_BRIDGE_PRESENTS.store(0, Ordering::SeqCst);
        OVERLAY_WORLD_STOP_LOGGED.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "present-overlay: re-armed for a new loading window (loading={loading} rebuilt={rebuilt})"
        ));
    }
    // PRIMARY STOP: we were on the loading screen and the load has now COMPLETED (load_done true AND the
    // black plate gone). This is the game's own transition to gameplay -- the spec-correct pop moment (the
    // instant the bar fills). IN_WORLD_REACHED is deliberately never consulted: it latches while the loading
    // screen is still up (PlayerIns lives through it), which was the premature-pop bug.
    if loading_seen && !loading {
        OVERLAY_STOPPED.store(1, Ordering::SeqCst);
        OVERLAY_STOP_TABLE_BUILDS.store(
            PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        OVERLAY_WINDOW_STOPS.fetch_add(1, Ordering::SeqCst);
        OVERLAY_STOP_REASON.store(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "present-overlay: load completed (load_done latched, cover plate gone) -> stopped compositing at the game's transition to gameplay"
        ));
        loading_portrait_window_reset("load completed");
        return false;
    }
    // ANTI-RUNAWAY BACKSTOP: pathological case where the load reports done AND we're in-world but the
    // primary stop can't fire (e.g. the black plate's `visible` stuck at 1). Count in-world+load_done
    // frames; force a stop past a huge budget so the head can't composite over gameplay forever. Never
    // fires on a normal load (load_done + !fake_vis stops immediately). reason=3 flags the assumption broke.
    if load_done && IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES {
        let n = OVERLAY_BRIDGE_PRESENTS.fetch_add(1, Ordering::SeqCst) + 1;
        if n >= OVERLAY_NOWLOAD_BRIDGE_MAX_PRESENTS {
            OVERLAY_STOPPED.store(1, Ordering::SeqCst);
            OVERLAY_STOP_TABLE_BUILDS.store(
                PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
            OVERLAY_WINDOW_STOPS.fetch_add(1, Ordering::SeqCst);
            OVERLAY_STOP_REASON.store(3, Ordering::SeqCst);
            if OVERLAY_WORLD_STOP_LOGGED.swap(1, Ordering::SeqCst) == 0 {
                append_autoload_debug(format_args!(
                    "present-overlay: BACKSTOP stop -- load_done + in-world for {n} presents but primary stop never fired (cover plate stuck?); forcing stop"
                ));
            }
            loading_portrait_window_reset("load-done backstop");
            return false;
        }
    }
    // DISPLAY-AVAILABILITY gate, decoupled from the drive-freeze latch (make-before-break): show
    // whenever we have EVER published a keyed (masked) frame (PROFILE_HAVE_KEYED_FRAME, persistent) or
    // the diagnostic bake path latched one. This is what lets the prior masked head keep displaying
    // after a confirm clears the drive-freeze (PROFILE_BAKE_RGBA_CAPTURED) to re-render the new
    // character -- the composite keeps showing LOADING_BG_PORTRAIT_RGBA until the new model's first
    // keyed frame replaces it. Before ANY keyed frame exists, bail (no opaque/blank flash).
    if PROFILE_HAVE_KEYED_FRAME.load(Ordering::SeqCst) == 0
        && PROFILE_BAKE_RGBA_CAPTURED.load(Ordering::SeqCst) == 0
    {
        return false;
    }
    // NOTE: this used to bail when render-drive was on, back when "render-drive" meant the Present hook
    // itself drove the offscreen rasterize (so compositing here would have fought it). The rasterize now
    // runs in the draw-phase task (profile_lookat_realtime_draw_tick -> drive(r)), which re-renders the
    // posed model and the readback republishes LOADING_BG_PORTRAIT_RGBA (version bump) EVERY frame. So the
    // Present hook is free to composite -- and MUST, to push that per-frame tracking head to the screen for
    // the whole loading screen (the forge redirect only commits ~twice -> a frozen displayed head). The
    // live-re-upload block below rebuilds the overlay texture on each version bump, so the displayed head
    // follows the cursor until loading completes.
    let forge_committed = LOADING_BG_TEXTURE_REDIRECT_COMMITS.load(Ordering::SeqCst) > 0;
    // Forge-INDEPENDENT loading-window latch: PROFILE_LOADSCREEN_TABLE_BUILDS goes >0 when we (re)build our
    // profile renderers post-Continue -- i.e. we are on the loading cover, past the menu, before the world.
    // The overlay used to lean solely on `forge_committed` as its "cover is up" signal (now_loading_active /
    // fake_loading_screen_visible both read false in the direct-menu-load path), so disabling the native
    // forge silently killed the overlay too. This latch keeps the overlay live when the forge is off
    // (overlay-only mode) without depending on it. Same monotonic behaviour (never decrements) as forge.
    let loadscreen_active = PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst) > 0;
    if !(forge_committed || loadscreen_active || loading) {
        return false;
    }
    if OVERLAY_DRAW_STATE.load(Ordering::SeqCst) == 2 {
        return false;
    }

    let sc_raw = swapchain_raw as *mut c_void;
    let Some(sc) = (unsafe { IDXGISwapChain3::from_raw_borrowed(&sc_raw) }) else {
        return false;
    };
    let idx = unsafe { sc.GetCurrentBackBufferIndex() };
    let Ok(backbuffer) = (unsafe { sc.GetBuffer::<ID3D12Resource>(idx) }) else {
        return false;
    };

    if OVERLAY_DRAW_STATE.load(Ordering::SeqCst) == 0 {
        if unsafe { init_overlay_draw_state(&backbuffer) } {
            OVERLAY_DRAW_STATE.store(1, Ordering::SeqCst);
            OVERLAY_PORTRAIT_VERSION.store(
                LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
            append_autoload_debug(format_args!(
                "present-overlay: draw state READY (portrait {}x{})",
                OVERLAY_PORTRAIT_W.load(Ordering::SeqCst),
                OVERLAY_PORTRAIT_H.load(Ordering::SeqCst)
            ));
        } else {
            OVERLAY_DRAW_STATE.store(2, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "present-overlay: draw init FAILED -- giving up"
            ));
            return false;
        }
    }

    // LIVE PORTRAIT SNAPSHOT: the draw-phase task republishes LOADING_BG_PORTRAIT_RGBA (version bump) each
    // frame with the freshly rendered, DEPTH-ALPHA-KEYED head (background alpha 0), so we blend the CURRENT
    // snapshot every frame -- the displayed head follows the cursor and its background stays transparent. On
    // any snapshot failure we skip this frame (leave the last presented content).
    let cur_ver = LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst);
    let Some((sw, sh, spx)) = LOADING_BG_PORTRAIT_RGBA.lock().ok().and_then(|g| g.clone()) else {
        return false;
    };
    if sw == 0 || sh == 0 || spx.len() < (sw as usize) * (sh as usize) * RGBA8_BPP {
        return false;
    }
    // Animation-stall semaphore: a portrait frame is being displayed this present. Paired with the
    // per-drive-frame counter, a low drive/display ratio means the head froze early in the window.
    PROFILE_DISPLAY_FRAMES_WINDOW.fetch_add(1, Ordering::SeqCst);

    let alloc_raw = OVERLAY_ALLOCATOR.load(Ordering::SeqCst) as *mut c_void;
    let list_raw = OVERLAY_LIST.load(Ordering::SeqCst) as *mut c_void;
    let fence_raw = OVERLAY_FENCE.load(Ordering::SeqCst) as *mut c_void;
    let queue_raw = OVERLAY_QUEUE.load(Ordering::SeqCst) as *mut c_void;
    let (Some(allocator), Some(list), Some(fence), Some(queue)) = (unsafe {
        (
            ID3D12CommandAllocator::from_raw_borrowed(&alloc_raw),
            ID3D12GraphicsCommandList::from_raw_borrowed(&list_raw),
            ID3D12Fence::from_raw_borrowed(&fence_raw),
            ID3D12CommandQueue::from_raw_borrowed(&queue_raw),
        )
    }) else {
        return false;
    };
    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { backbuffer.GetDevice(&mut device_opt) }.is_err() {
        return false;
    }
    let Some(device) = device_opt else {
        return false;
    };

    let bb_desc = unsafe { backbuffer.GetDesc() };
    let bw = bb_desc.Width as u32;
    let bh = bb_desc.Height;
    if bw == 0 || bh == 0 {
        return false;
    }
    // Center the portrait region on the backbuffer (no scaling; region = portrait size clamped to the bb).
    let cw = sw.min(bw);
    let ch = sh.min(bh);
    let dx = (bw - cw) / 2;
    let dy = (bh - ch) / 2;

    // Alpha-honoring composite: read the live backbuffer region, blend the portrait OVER it (bg alpha 0 =>
    // the loading screen shows through), write the blended region back. All via COPY primitives -- no PSO.
    if !unsafe {
        blend_portrait_over_backbuffer(
            &device,
            queue,
            allocator,
            list,
            fence,
            &backbuffer,
            bb_desc.Format,
            dx,
            dy,
            cw,
            ch,
            sw,
            &spx,
        )
    } {
        return false;
    }

    // Preserve the "displayed head updates per frame" oracle (oracle_overlay_reuploads): count a fresh
    // published version reaching the screen.
    if cur_ver != OVERLAY_PORTRAIT_VERSION.swap(cur_ver, Ordering::SeqCst) {
        OVERLAY_REUPLOADS.fetch_add(1, Ordering::SeqCst);
    }
    let hits = OVERLAY_DRAW_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if hits == 1 {
        append_autoload_debug(format_args!(
            "present-overlay: portrait CPU-blended onto backbuffer {bw}x{bh} (portrait {sw}x{sh} at {dx},{dy}, depth-alpha-keyed bg)"
        ));
    }
    true
}

/// Alpha-honoring CPU composite: copy the live backbuffer region `[dx,dy .. dx+cw,dy+ch]` to a readback
/// buffer, blend the portrait (`spx`, `sw` wide, RGBA8 with per-pixel alpha) OVER it (`src.a`/`1-src.a`; a
/// background pixel with alpha 0 leaves the backbuffer untouched so the loading screen shows through), then
/// write the blended region back. Two submits on our OWN queue with a CPU fence wait between them (the blend
/// needs the readback mapped). Reuses the cached `OVERLAY_BB_*` buffers; leaves the backbuffer in PRESENT.
/// `false` on any failure (frame skipped). Never touches the game's queue.
#[allow(clippy::too_many_arguments)]
unsafe fn blend_portrait_over_backbuffer(
    device: &ID3D12Device,
    queue: &ID3D12CommandQueue,
    allocator: &ID3D12CommandAllocator,
    list: &ID3D12GraphicsCommandList,
    fence: &ID3D12Fence,
    backbuffer: &ID3D12Resource,
    bb_format: DXGI_FORMAT,
    dx: u32,
    dy: u32,
    cw: u32,
    ch: u32,
    sw: u32,
    spx: &[u8],
) -> bool {
    // Copyable footprint of a cw x ch region in the backbuffer's format (256-aligned rows).
    let region_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: cw as u64,
        Height: ch,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: bb_format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    };
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total_bytes: u64 = 0;
    unsafe {
        device.GetCopyableFootprints(
            &region_desc,
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
        return false;
    }
    // (Re)create the cached readback + upload buffers on footprint change (fixed once for a fixed bb size).
    if OVERLAY_BB_BUFSIZE.load(Ordering::SeqCst) != total_bytes {
        let rb_heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_READBACK,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
            MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
            CreationNodeMask: 1,
            VisibleNodeMask: 1,
        };
        let up_heap = D3D12_HEAP_PROPERTIES {
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
        let mut rb_opt: Option<ID3D12Resource> = None;
        if unsafe {
            device.CreateCommittedResource(
                &rb_heap,
                D3D12_HEAP_FLAG_NONE,
                &buf_desc,
                D3D12_RESOURCE_STATE_COPY_DEST,
                None,
                &mut rb_opt,
            )
        }
        .is_err()
        {
            return false;
        }
        let mut up_opt: Option<ID3D12Resource> = None;
        if unsafe {
            device.CreateCommittedResource(
                &up_heap,
                D3D12_HEAP_FLAG_NONE,
                &buf_desc,
                D3D12_RESOURCE_STATE_GENERIC_READ,
                None,
                &mut up_opt,
            )
        }
        .is_err()
        {
            return false;
        }
        let (Some(rb), Some(up)) = (rb_opt, up_opt) else {
            return false;
        };
        let old_rb = OVERLAY_BB_READBACK.swap(rb.into_raw() as usize, Ordering::SeqCst);
        if old_rb != 0 {
            drop(unsafe { ID3D12Resource::from_raw(old_rb as *mut c_void) });
        }
        let old_up = OVERLAY_BB_UPLOAD.swap(up.into_raw() as usize, Ordering::SeqCst);
        if old_up != 0 {
            drop(unsafe { ID3D12Resource::from_raw(old_up as *mut c_void) });
        }
        OVERLAY_BB_BUFSIZE.store(total_bytes, Ordering::SeqCst);
    }
    let rb_raw = OVERLAY_BB_READBACK.load(Ordering::SeqCst) as *mut c_void;
    let up_raw = OVERLAY_BB_UPLOAD.load(Ordering::SeqCst) as *mut c_void;
    let (Some(readback), Some(upload)) = (unsafe {
        (
            ID3D12Resource::from_raw_borrowed(&rb_raw),
            ID3D12Resource::from_raw_borrowed(&up_raw),
        )
    }) else {
        return false;
    };

    // ---- SUBMIT #1: backbuffer region -> readback buffer (leaves the backbuffer in COPY_SOURCE) ----
    if unsafe { allocator.Reset() }.is_err() || unsafe { list.Reset(allocator, None) }.is_err() {
        return false;
    }
    unsafe {
        record_transition(
            list,
            backbuffer,
            D3D12_RESOURCE_STATE_PRESENT,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )
    };
    let mut rb_dst = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(readback.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    let mut bb_src = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(backbuffer.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    let read_box = D3D12_BOX {
        left: dx,
        top: dy,
        front: 0,
        right: dx + cw,
        bottom: dy + ch,
        back: 1,
    };
    unsafe { list.CopyTextureRegion(&rb_dst, 0, 0, 0, &bb_src, Some(&read_box)) };
    unsafe { ManuallyDrop::drop(&mut rb_dst.pResource) };
    unsafe { ManuallyDrop::drop(&mut bb_src.pResource) };
    if !unsafe { execute_and_wait(queue, list, fence) } {
        return false;
    }

    // ---- CPU BLEND: readback (backbuffer pixels) OVER-composited with the portrait, into the upload buffer.
    let row_pitch = footprint.Footprint.RowPitch as usize;
    let total = total_bytes as usize;
    let swap = matches!(
        bb_format,
        DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
    );
    let read_range = D3D12_RANGE {
        Begin: 0,
        End: total,
    };
    let mut rmap: *mut c_void = std::ptr::null_mut();
    if unsafe { readback.Map(0, Some(&read_range), Some(&mut rmap)) }.is_err() || rmap.is_null() {
        return false;
    }
    let mut umap: *mut c_void = std::ptr::null_mut();
    if unsafe { upload.Map(0, None, Some(&mut umap)) }.is_err() || umap.is_null() {
        let empty = D3D12_RANGE { Begin: 0, End: 0 };
        unsafe { readback.Unmap(0, Some(&empty)) };
        return false;
    }
    {
        let rb_bytes = unsafe { std::slice::from_raw_parts(rmap as *const u8, total) };
        let up_bytes = unsafe { std::slice::from_raw_parts_mut(umap as *mut u8, total) };
        let sw = sw as usize;
        let cw = cw as usize;
        let ch = ch as usize;
        for y in 0..ch {
            let ro = y * row_pitch;
            for x in 0..cw {
                let o = ro + x * 4;
                let so = (y * sw + x) * 4;
                if o + 4 > total || so + 4 > spx.len() {
                    break;
                }
                let a = spx[so + 3] as u32;
                let ia = 255 - a;
                // Portrait is RGBA; place each portrait channel at the backbuffer's channel position.
                let (p0, p2) = if swap {
                    (spx[so + 2] as u32, spx[so] as u32) // bb pos0=B, pos2=R
                } else {
                    (spx[so] as u32, spx[so + 2] as u32) // bb pos0=R, pos2=B
                };
                let p1 = spx[so + 1] as u32;
                let blend = |p: u32, d: u32| ((p * a + d * ia + 127) / 255) as u8;
                up_bytes[o] = blend(p0, rb_bytes[o] as u32);
                up_bytes[o + 1] = blend(p1, rb_bytes[o + 1] as u32);
                up_bytes[o + 2] = blend(p2, rb_bytes[o + 2] as u32);
                up_bytes[o + 3] = 255;
            }
        }
    }
    let empty = D3D12_RANGE { Begin: 0, End: 0 };
    unsafe { readback.Unmap(0, Some(&empty)) };
    unsafe { upload.Unmap(0, None) };

    // ---- SUBMIT #2: upload buffer -> backbuffer region (COPY_SOURCE -> COPY_DEST -> PRESENT) ----
    if unsafe { allocator.Reset() }.is_err() || unsafe { list.Reset(allocator, None) }.is_err() {
        return false;
    }
    unsafe {
        record_transition(
            list,
            backbuffer,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )
    };
    let mut up_src = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(upload.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    let mut bb_dst = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(backbuffer.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    let up_box = D3D12_BOX {
        left: 0,
        top: 0,
        front: 0,
        right: cw,
        bottom: ch,
        back: 1,
    };
    unsafe { list.CopyTextureRegion(&bb_dst, dx, dy, 0, &up_src, Some(&up_box)) };
    unsafe { ManuallyDrop::drop(&mut up_src.pResource) };
    unsafe { ManuallyDrop::drop(&mut bb_dst.pResource) };
    unsafe {
        record_transition(
            list,
            backbuffer,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_PRESENT,
        )
    };
    unsafe { execute_and_wait(queue, list, fence) }
}

/// Close `list`, execute it on `queue`, signal `fence` with a fresh monotonic value, and CPU-wait (bounded)
/// for GPU completion. `false` on any failure. Shared by the two-submit CPU-blend composite.
unsafe fn execute_and_wait(
    queue: &ID3D12CommandQueue,
    list: &ID3D12GraphicsCommandList,
    fence: &ID3D12Fence,
) -> bool {
    if unsafe { list.Close() }.is_err() {
        return false;
    }
    let Ok(base_list) = list.cast::<ID3D12CommandList>() else {
        return false;
    };
    unsafe { queue.ExecuteCommandLists(&[Some(base_list)]) };
    let val = OVERLAY_FENCE_VAL.fetch_add(1, Ordering::SeqCst) + 1;
    if unsafe { queue.Signal(fence, val) }.is_err() {
        return false;
    }
    if unsafe { fence.GetCompletedValue() } < val {
        let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else {
            return false;
        };
        if unsafe { fence.SetEventOnCompletion(val, event) }.is_err() {
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

/// True if the read-back RGBA8 image has any non-black texel (`max(R,G,B) > 24`) inside a center
/// 64x64 region. Used to set `LOADING_BG_PORTRAIT_NONBLACK` -- a quick "did we capture a real head
/// vs a blank/black offscreen" oracle.
pub(crate) fn portrait_center_nonblack(width: u32, height: u32, pixels: &[u8]) -> bool {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || pixels.len() < w * h * RGBA8_BPP {
        return false;
    }
    const REGION: usize = 64;
    let half = REGION / 2;
    let cx = w / 2;
    let cy = h / 2;
    let x0 = cx.saturating_sub(half);
    let x1 = (cx + half).min(w);
    let y0 = cy.saturating_sub(half);
    let y1 = (cy + half).min(h);
    for y in y0..y1 {
        for x in x0..x1 {
            let idx = (y * w + x) * RGBA8_BPP;
            let r = pixels[idx];
            let g = pixels[idx + 1];
            let b = pixels[idx + 2];
            if r.max(g).max(b) > 24 {
                return true;
            }
        }
    }
    false
}

/// True if the read-back RGBA8 image looks like a SOLID-COLOR-CHECKER PLACEHOLDER (our magenta/white or
/// magenta/yellow er-tpf cover, or an unrendered RT clear pattern) rather than a real 3D head render.
///
/// WHY: `portrait_center_nonblack` only proves "not all black" -- a bright magenta checker (255,0,255)
/// trivially passes it, so `oracle_loading_bg_portrait_gx_nonblack` was a FALSE POSITIVE for the autoload
/// path (run postcontinue-lookat-smoke 2026-06-30: nonblack=True but the captured bytes were a magenta/
/// white checker, because the model builds but is never rendered into the offscreen RT once the menu's
/// render driver dies post-Continue). A real character render has many shaded colors and few fully-
/// saturated "pure" texels; a checker is ~2 colors, each with channels pinned to 0/255. Heuristic over the
/// center region: sample texels, quantize to 5 bits/channel, and call it a checker if (a) the 2 most-common
/// quantized colors cover >= 85% of samples AND (b) >= 70% of samples are "pure" (every channel <16 or >239).
pub(crate) fn portrait_looks_like_checker(width: u32, height: u32, pixels: &[u8]) -> bool {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || pixels.len() < w * h * RGBA8_BPP {
        return false;
    }
    const REGION: usize = 128;
    let half = REGION / 2;
    let (cx, cy) = (w / 2, h / 2);
    let x0 = cx.saturating_sub(half);
    let x1 = (cx + half).min(w);
    let y0 = cy.saturating_sub(half);
    let y1 = (cy + half).min(h);
    let mut counts: std::collections::HashMap<u16, u32> = std::collections::HashMap::new();
    let mut total = 0u32;
    let mut pure = 0u32;
    for y in y0..y1 {
        for x in x0..x1 {
            let idx = (y * w + x) * RGBA8_BPP;
            let (r, g, b) = (pixels[idx], pixels[idx + 1], pixels[idx + 2]);
            // pure = every channel near an extreme (0/255) -> checker/placeholder hallmark
            let is_pure = |c: u8| c < 16 || c > 239;
            if is_pure(r) && is_pure(g) && is_pure(b) {
                pure += 1;
            }
            let key = (((r >> 3) as u16) << 10) | (((g >> 3) as u16) << 5) | ((b >> 3) as u16);
            *counts.entry(key).or_insert(0) += 1;
            total += 1;
        }
    }
    if total == 0 {
        return false;
    }
    let mut vals: Vec<u32> = counts.values().copied().collect();
    vals.sort_unstable_by(|a, b| b.cmp(a));
    let top2: u32 = vals.iter().take(2).sum();
    let top2_frac = top2 as f32 / total as f32;
    let pure_frac = pure as f32 / total as f32;
    top2_frac >= 0.85 && pure_frac >= 0.70
}
