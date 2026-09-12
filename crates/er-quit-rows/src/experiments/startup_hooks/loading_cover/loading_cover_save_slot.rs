use super::*;

/// Reads `CSNowLoadingHelperImp::load_done` off the NowLoading singleton. Warning (RE-corrected
/// 2026-07-02): despite the name this is a load-complete latch, not "loading screen visible" -- `Update`
/// copies it from `request_load_done` (raised by the map-load system), so it reads true after the load
/// finishes and lingers into gameplay. Do not use it to decide the portrait overlay lifetime; kept for
/// telemetry/parity. Fault-guarded.
pub(crate) unsafe fn now_loading_active(base: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // `read_global_ptr` resolves the global for the running build and then reads it
    // fault-tolerantly. The raw `base + RVA` form read the 1.16.2 slot, and this singleton moved
    // (0x3d60ec8 -> 0x3d64f28) -- so the read succeeded and returned a neighbouring global.
    let helper = er_game_base::mem::read_global_ptr(
        base,
        RuntimeGlobalRva::NowLoadingSingleton as usize,
        "RuntimeGlobalRva::NowLoadingSingleton",
    );
    if helper == 0 || helper == null {
        return false;
    }
    let off = core::mem::offset_of!(CSNowLoadingHelperImp, load_done);
    unsafe { safe_read_usize(helper + off) }
        .map(|v| (v & 0xff) != 0)
        .unwrap_or(false)
}

/// Resolve the live `CSFakeLoadingScreenImp` (the render-pipeline cover plate) or 0. Singleton =
/// `*(base + FakeLoadingScreenSingleton)`. Fault-guarded.
pub(crate) unsafe fn fake_loading_screen_ptr(base: usize) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Resolved, for the reason given on `now_loading_active`: this singleton moved too
    // (0x3d74868 -> 0x3d788d8), and a stale read of it succeeds with the wrong pointer.
    let helper = er_game_base::mem::read_global_ptr(
        base,
        RuntimeGlobalRva::FakeLoadingScreenSingleton as usize,
        "RuntimeGlobalRva::FakeLoadingScreenSingleton",
    );
    if helper == 0 || helper == null {
        0
    } else {
        helper
    }
}

/// True while the `CSFakeLoadingScreenImp` cover plate is VISIBLE: `visible` (+0x8) & 0xff. This is the
/// render-pipeline cover the game draws to hide the world teardown/rebuild during a map load. Fault-guarded.
pub(crate) unsafe fn fake_loading_screen_visible(base: usize) -> bool {
    let helper = unsafe { fake_loading_screen_ptr(base) };
    if helper == 0 {
        return false;
    }
    unsafe { safe_read_usize(helper + FAKE_LOADING_SCREEN_VISIBLE_OFFSET) }
        .map(|v| (v & 0xff) != 0)
        .unwrap_or(false)
}

/// The portrait build + draw pipeline must pause only during active gameplay -- the player has reached the
/// world and the current load has completed (`load_done`, via now_loading_active) and no loading cover is
/// up. It must re-engage for every subsequent loading screen (notably a System Quit -> Load Profile
/// character switch). The old gate was the bare `IN_WORLD_REACHED == YES` latch, which is set the first
/// time the player reaches the world and never resets -> after the first load the build/draw ticks froze
/// forever, so the head only ever rendered on the first character load (the subsequent-load bug, run
/// head-popfix-loaddone 2026-07-02: after the 2nd deserialize the whole pipeline was silent). Fault-guarded.
pub(crate) unsafe fn portrait_pipeline_idle_in_gameplay(base: usize) -> bool {
    // Also idle while the game's ProfileSelect (Load) menu is open: it renders its own portraits,
    // and our drive/readback stacking on top overflows the GX command queue (see the build gate in
    // maybe_build_profile_table_for_loading). Our pipeline is for the loading screen, after the menu.
    if SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) != 0 {
        return true;
    }
    // Never idle while our own cover is still on screen (2026-09-05, user goal: the portrait lives
    // as long as the rest of the loading screen and animates for as long as it lives).
    //
    // Every other clause below describes the game's loading screen -- its `now_loading` state, its
    // `CSFakeLoadingScreenImp` plate. Ours outlives all of them by the release fade, and the head is
    // on our surface, so idling on the game's timeline freezes a portrait the user is still looking
    // at. Measured on br-20260906-001201-c84f with the timestamps added for this goal:
    // `last_draw_tick_ms=39022 last_publish_ms=39051 cover_stop_ms=42201
    // frozen_before_cover_stop_ms=3150` -- the pipeline stopped being driven 3.18 s before the cover
    // came down and the head sat still for that whole stretch.
    //
    // `BOOT_VIEW_DRAW_HITS != 0` is what makes this a narrowing rather than a new way to run
    // forever: it asks whether the cover ever actually composited anything. A window that never
    // armed has no stop latch to wait for, so it falls straight through to the gameplay clauses and
    // this clause cannot pin the pipeline on in gameplay.
    let cover_live = er_telemetry_core::counters::BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst) != 0
        && er_telemetry_core::counters::BOOT_VIEW_STOPPED.load(Ordering::SeqCst) == 0;
    if cover_live {
        return false;
    }
    IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES
        && unsafe { now_loading_active(base) }
        && !unsafe { fake_loading_screen_visible(base) }
}

/// True while the game's native now-loading screen is actively rendering -- CS::LoadingScreen::Update
/// ticked within the last ~250ms (LOADING_SCREEN_UPDATE_HITS increments each of its frames and stops the
/// instant the screen is destroyed). The portrait build/drive pipeline keys off this to stay engaged
/// through the native loading screen: on a fast load PlayerIns resolves (IN_WORLD_REACHED) ~1.7s before the
/// loading screen clears, so portrait_pipeline_idle_in_gameplay flips true mid-load and the build/drive
/// pipeline returned before the model could build+render (run32: force_profile_render_tick never reached
/// maybe_build). Wall-clock recency (not a per-call decrement) makes it safe to poll multiple times a frame.
pub(crate) fn native_loading_screen_active() -> bool {
    pub(crate) use er_telemetry_core::counters::LAST_HITS;
    static LAST_CHANGE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let now_ms = EPOCH
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64;
    let hits = LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst);
    if LAST_HITS.swap(hits, Ordering::SeqCst) != hits {
        LAST_CHANGE_MS.store(now_ms, Ordering::SeqCst);
    }
    let last = LAST_CHANGE_MS.load(Ordering::SeqCst);
    last != 0 && now_ms.saturating_sub(last) < 250
}

/// Count profile-table renderers that currently hold a live character model (+0x778 valid). The game's
/// Load Profile menu builds all 10 (one per save), so this reads ~10 during the menu; our post-Continue
/// rebuild leaves only the loaded character's model live, so it reads 1 on the loading screen. The display
/// publish gates on `<= 1` to avoid reading back the wrong character while multiple models are live (the
/// subsequent-load cascade). Fault-guarded.
pub(crate) unsafe fn count_live_profile_models(base: usize) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let mut n = 0usize;
    for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
        let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
        if valid(r)
            && unsafe { safe_read_usize(r) }.unwrap_or(0)
                == er_game_base::mem::game_data_addr(
                    base,
                    TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
                    "TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA",
                )
            && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                .map(&valid)
                .unwrap_or(false)
        {
            n += 1;
        }
    }
    n
}

/// Post-continue PORTRAIT: when the now-loading screen is up but the profile-renderer title table has been
/// torn down (native-continue is menu-free, so the menu never built it, or Continue tore it down), call
/// the engine's own builder once to repopulate the 10-slot table. The existing mark+refresh feed +
/// per-frame look-at hook + draw + pixel oracle then re-engage on the loading screen automatically (they
/// all key off this table). Latched per load (reset when now-loading drops) so there's no per-frame churn.
pub(crate) unsafe fn maybe_build_profile_table_for_loading(base: usize) {
    if !portrait_overlay_enabled() {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == 0 || base == null {
        return;
    }
    // The renderer ctor snapshots the per-slot offscreen-size table. If we build the post-Continue
    // profile table before the loaded slot's row is patched, the target RT is permanently native-size
    // (128 base * x2 supersample = observed 256) for this window. Wait until the loaded slot is named
    // and patched, then call the builder.
    if portrait_real_pixels_enabled()
        && !unsafe { patch_profile_offscreen_size_for_loaded_slot(base) }
    {
        return;
    }
    // Root fix (2026-07-03, run gxguard2): do not build our portrait table while the game's own
    // ProfileSelect (Load Character) menu still owns a populated portrait table. Once Continue teardown
    // has emptied that table, the lingering window-owner flag is stale for our purpose; build immediately
    // instead of waiting for that flag to clear, so the loading-owned renderer is ready when the loading
    // screen appears.
    let profile_select_window_open = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) != 0;
    // Native LoadingScreen::Update is a stronger "the menu is done; the loading surface is already on
    // screen" signal than waiting for the profile renderer table to become empty. Prior behavior waited for
    // the stale native table to drain, building at +19011ms even though Gauge_3 was visible at +16650ms; the
    // async model then came live ~560ms later. Once this native screen is ticking, rebuilding the table is
    // just the game's own builder's first step (FUN_1409b2db0 delay-deletes the old 10 renderers before
    // constructing new ones), so do it immediately instead of burning the leading portrait gap.
    let loading_screen_started = LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst) != 0
        && LOADING_SCREEN_BAR_ENABLED.load(Ordering::SeqCst) != 0
        && LOADING_SCREEN_BAR_MAX_FRAME.load(Ordering::SeqCst) != 0;
    // If the table is already populated (menu built it, or our own build already ran), leave it -- the
    // existing mark+refresh feed + look-at + draw + oracle drive it. A live table also RE-arms the latch:
    // a subsequent Continue teardown empties it again and we rebuild our own for that load window. Exception:
    // native LoadingScreen is already ticking while the table is still populated. That table is stale for the
    // loadscreen portrait; force the normal builder now so the model build overlaps the earliest loading bar.
    let t0 = unsafe { safe_read_usize(portrait_renderer_table_entry(base, 0)) }.unwrap_or(0);
    let populated = t0 != 0
        && t0 != null
        && unsafe { safe_read_usize(t0) }.unwrap_or(0)
            == er_game_base::mem::game_data_addr(
                base,
                TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
                "TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA",
            );
    let force_loading_screen_rebuild = populated
        && loading_screen_started
        && PROFILE_LOADSCREEN_TABLE_OWNED.load(Ordering::SeqCst) == 0
        && PROFILE_LOADSCREEN_REBUILT.load(Ordering::SeqCst) == 0;
    if populated && !force_loading_screen_rebuild {
        PROFILE_TABLE_EMPTY_STREAK.store(0, Ordering::SeqCst);
        PROFILE_TABLE_WAS_POPULATED.store(1, Ordering::SeqCst);
        if PROFILE_LOADSCREEN_TABLE_OWNED.load(Ordering::SeqCst) == 0 {
            PROFILE_LOADSCREEN_REBUILT.store(0, Ordering::SeqCst);
        }
        return;
    }
    if populated {
        PROFILE_TABLE_EMPTY_STREAK.store(0, Ordering::SeqCst);
        PROFILE_TABLE_WAS_POPULATED.store(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "loading-portrait: native LoadingScreen already updating while profile table is still populated -- rebuilding loading-owned renderer immediately to close the leading portrait gap"
        ));
    } else if profile_select_window_open {
        append_autoload_debug(format_args!(
            "loading-portrait: ProfileSelect owner flag still set, but renderer table is empty -- treating as Continue teardown and building loading-owned renderer now"
        ));
    }
    // Table is empty this tick -- count the streak. The menu's own teardown+rebuild is synchronous, so a
    // sustained-empty table across ticks means the Continue teardown ran with no menu rebuild (we've left
    // the menu into the load), which happens ~17s -- well before the now-loading flag flips (~21s on the
    // fast gold-save load). Build as soon as either signal fires so ResMan has time to build the model.
    let streak = if force_loading_screen_rebuild {
        0
    } else {
        PROFILE_TABLE_EMPTY_STREAK.fetch_add(1, Ordering::SeqCst) + 1
    };
    if PROFILE_LOADSCREEN_REBUILT.load(Ordering::SeqCst) != 0 {
        return; // already built our table for this load window
    }
    // Hard SAFETY: never call the builder until the engine/ResMan is provably up, or it access-violates at
    // the title (empty table, ResMan not up). Normally we require the game's own ProfileSelect menu to have
    // built its portrait table once (PROFILE_TABLE_WAS_POPULATED). But a menu-free autoload (native product /
    // staged / native-continue) never shows ProfileSelect, so that latch stays 0 and the builder was blocked
    // forever -- the loading-portrait pipeline never engaged on native (run30 2026-07-15:
    // loadscreen_table_builds=0, model drive never fired). During an active native loading screen
    // (loading_screen_started: CS::LoadingScreen::Update ticking + Gauge_3 enabled + max-frame set) ResMan is
    // definitively up (the world is streaming), so the builder is safe there even without the menu having
    // built its table. Accept that context as the ResMan-up proof for the menu-free autoload path.
    if PROFILE_TABLE_WAS_POPULATED.load(Ordering::SeqCst) == 0 && !loading_screen_started {
        return;
    }
    let nowload = unsafe { now_loading_active(base) };
    if !(force_loading_screen_rebuild
        || nowload
        || profile_select_window_open
        || loading_screen_started
        || streak >= PROFILE_TABLE_EMPTY_STREAK_BUILD_THRESHOLD)
    {
        return;
    }
    // Orphan reclaim BACKSTOP (second-load foreign-head fix; primary reclaim is at the switch confirm in
    // system_quit_arm_quickload_autoload). If a prior window's spared renderer is still parked in
    // PROFILE_SPARE_ORPHAN when a new loading window takes ownership, it is a live foreign producer: its
    // model + offscreen scene are still registered and keep rendering the previous character's head,
    // which the new window's readback then publishes. Delete-enqueue it before constructing this
    // window's renderers. Game thread (force_profile_render_tick task), same delay-delete path as the
    // teardown-spare hook; swap(0) keeps the two reclaim sites mutually exclusive.
    let orphan = PROFILE_SPARE_ORPHAN.swap(0, Ordering::SeqCst);
    if orphan != 0 {
        let deleted = unsafe { delay_delete_enqueue_renderer(orphan) };
        ownership_release(OwnedClass::SparedRenderer);
        append_autoload_debug(format_args!(
            "loading-portrait: reclaimed prior spared renderer 0x{orphan:x} at loading-table build via CSDelayDeleteMan enqueued={deleted} (second-load foreign-head backstop)"
        ));
    }
    // Build it via the engine's own 10-slot builder (teardown is a no-op on a null table). Each fresh
    // CSMenuProfModelRend self-registers its ResMan model build/draw tasks, so it builds + owns its own
    // model with our lifetime -- not borrowed from the torn-down menu. Self-contained off process-lifetime
    // singletons (RE-confirmed).
    let builder: unsafe extern "system" fn() = unsafe {
        core::mem::transmute(
            match crate::experiments::gated_game_fn(
                PROFILE_TABLE_BUILDER_RVA,
                "PROFILE_TABLE_BUILDER_RVA",
            ) {
                Some(address) => address,
                None => return,
            },
        )
    };
    unsafe { builder() };
    // The loading-cover observer (CSNowLoadingHelperImp ctor/update) is the overlay's primary end-of-cover
    // signal (update pulses stop == the game dismissed the tips+bar screen). Install it here, at the start
    // of every loading window, instead of relying on the accept-byte-gated product path (which never fired
    // on the strip-default run -> hooks_installed=0 and the overlay had to lean on the in-world latch).
    install_now_loading_helper_observer_hooks();
    // Kick the model build this tick: the mark+refresh feed that requests the async character-model build
    // only runs every 240 ticks (counter % 240 == 0). The post-Continue now-loading window is shorter than
    // 240 ticks, so without this the freshly-built renderers are never fed -> they stay model-less (m=0).
    // Resetting the counter to 0 makes the feed fire on the very next pass through force_profile_render_tick.
    PROFILE_FORCE_TICK_COUNTER.store(0, Ordering::SeqCst);
    // Open the post-Continue feed window so the mark+refresh runs frequently (not just every 240 ticks) and
    // drives the async ResMan model build to completion + keeps it latched through the loading screen.
    PROFILE_LOADSCREEN_FEED_TICKS.store(PROFILE_LOADSCREEN_FEED_WINDOW_TICKS, Ordering::SeqCst);
    PROFILE_LOADSCREEN_REBUILT.store(1, Ordering::SeqCst);
    PROFILE_LOADSCREEN_TABLE_OWNED.store(1, Ordering::SeqCst);
    PROFILE_LOADSCREEN_TABLE_BUILDS.fetch_add(1, Ordering::SeqCst);
    // Boot-epoch publish-latency anchor (bd er-effects-rs-io53): the switch path stamps
    // PORTRAIT_CONFIRM_MS at the switch confirm, so PORTRAIT_CONFIRM_TO_PUBLISH_MS_LAST never measured
    // the boot window. Stamp the boot anchor here -- the loading-owned table build, observed ~90ms
    // after the boot loading screen opens -- so the boot window's first publish computes the same
    // latency oracle. compare_exchange from 0 so a pending switch-confirm stamp is never clobbered;
    // epoch 0 only (switch epochs keep the confirm-press anchor).
    if crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst) == 0
    {
        let _ = er_telemetry_core::counters::PORTRAIT_CONFIRM_MS.compare_exchange(
            0,
            crate::experiments::boot_view_epoch_ms().max(1) as usize,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
    append_autoload_debug(format_args!(
        "loading-portrait: profile table rebuild (trigger={} streak={streak}) -> called builder 0x{:x} to build our own renderers for the post-Continue portrait",
        if force_loading_screen_rebuild {
            "native-loading-screen"
        } else if nowload {
            "now-loading"
        } else if profile_select_window_open {
            "profile-window-empty"
        } else {
            "empty-streak"
        },
        er_game_base::mem::game_data_addr(
            base,
            PROFILE_TABLE_BUILDER_RVA,
            "PROFILE_TABLE_BUILDER_RVA"
        )
    ));
}

/// Kick the ASYNC character-model build for one profile slot -- a faithful per-slot replica of the body
/// of the engine's global refresh (dump `FUN_1409aa7d0`), which we no longer call from the post-Continue
/// feed: the global form iterates all 10 slots and kicks every real+marked one, building every save
/// character mid-load (the cross-slot portrait swap). Writing the +0x754/+0x755 latches on the other
/// renderers to mute the global refresh crashed (GX command-queue overflow; the latches only mean
/// "requested" on a configured renderer). This replica performs the engine's exact per-slot sequence --
/// record lookup, ChrAsm/model-source config, FaceData copy, stream index, then the two request latches --
/// so the target slot builds exactly as the engine would build it, and the non-target renderers stay in
/// the natural never-configured state (flags 0, stepper idle -- the same state empty slots hold forever).
/// Returns true when the kick fired. Fault-guarded reads; skips when the slot was already requested.
pub(crate) unsafe fn kick_target_profile_slot(
    _base: usize,
    summary: usize,
    renderer: usize,
    slot: i32,
) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    if !valid(summary) || !valid(renderer) || !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot)
    {
        return false;
    }
    // One kick per slot value per load window (engine "refresh on profile-data change" semantics;
    // see PORTRAIT_KICK_SLOT_KEY). Re-kicking on a cadence poisoned the state machine (mid-pipeline
    // the model is dead + latches consumed, so the re-kick re-raised +0x754/+0x755 and Wait_Play
    // re-entered the rebuild state forever = the ~1/s rebuild storm, static portrait, shadow
    // flicker). But a blanket one-shot freezes the wrong character: `portrait_loaded_slot()` (ac0)
    // can still hold the previous session's slot when the first kick fires, and the storm's
    // accidental self-correction was the "swap to the actual character" the user always saw. Keying
    // the latch to the slot gives exactly one corrective kick when ac0 flips to the real slot --
    // a deterministic swap -- and no storm (the same slot never re-kicks). No live-model guard:
    // the corrective kick must fire on a live (wrong-record) model, exactly like the engine's
    // data-change refresh.
    if PORTRAIT_KICK_SLOT_KEY.load(Ordering::SeqCst) == (slot + 1) as usize
        && PORTRAIT_KICK_RENDERER.load(Ordering::SeqCst) == renderer
    {
        return false;
    }
    // Engine parity: kick only when both request latches read 0 (a kick is not already in flight).
    if unsafe { safe_read_u8(renderer + 0x754) }.unwrap_or(1) != 0
        || unsafe { safe_read_u8(renderer + 0x755) }.unwrap_or(1) != 0
    {
        return false;
    }
    let record_of: unsafe extern "system" fn(usize, i32) -> usize = unsafe {
        core::mem::transmute(
            match crate::experiments::gated_game_fn(
                PROFILE_SUMMARY_RECORD_RVA,
                "PROFILE_SUMMARY_RECORD_RVA",
            ) {
                Some(address) => address,
                None => return false,
            },
        )
    };
    let record = unsafe { record_of(summary, slot) };
    if !valid(record) {
        return false;
    }
    let set_model_source: unsafe extern "system" fn(usize, usize) = unsafe {
        core::mem::transmute(
            match crate::experiments::gated_game_fn(
                PROFILE_RENDERER_SET_MODEL_SOURCE_RVA,
                "PROFILE_RENDERER_SET_MODEL_SOURCE_RVA",
            ) {
                Some(address) => address,
                None => return false,
            },
        )
    };
    let facedata_buffer: unsafe extern "system" fn(usize, u8) -> usize = unsafe {
        core::mem::transmute(
            match crate::experiments::gated_game_fn(
                PROFILE_FACEDATA_BUFFER_RVA,
                "PROFILE_FACEDATA_BUFFER_RVA",
            ) {
                Some(address) => address,
                None => return false,
            },
        )
    };
    let set_facedata: unsafe extern "system" fn(usize, usize) = unsafe {
        core::mem::transmute(
            match crate::experiments::gated_game_fn(
                PROFILE_RENDERER_SET_FACEDATA_RVA,
                "PROFILE_RENDERER_SET_FACEDATA_RVA",
            ) {
                Some(address) => address,
                None => return false,
            },
        )
    };
    let set_byte290: unsafe extern "system" fn(usize, u8) = unsafe {
        core::mem::transmute(
            match crate::experiments::gated_game_fn(
                PROFILE_RENDERER_SET_BYTE290_RVA,
                "PROFILE_RENDERER_SET_BYTE290_RVA",
            ) {
                Some(address) => address,
                None => return false,
            },
        )
    };
    let set_flag_one: unsafe extern "system" fn(usize, u8) = unsafe {
        core::mem::transmute(
            match crate::experiments::gated_game_fn(
                PROFILE_RENDERER_SET_FLAG_ONE_RVA,
                "PROFILE_RENDERER_SET_FLAG_ONE_RVA",
            ) {
                Some(address) => address,
                None => return false,
            },
        )
    };
    let set_byte294: unsafe extern "system" fn(usize, u8) = unsafe {
        core::mem::transmute(
            match crate::experiments::gated_game_fn(
                PROFILE_RENDERER_SET_BYTE294_RVA,
                "PROFILE_RENDERER_SET_BYTE294_RVA",
            ) {
                Some(address) => address,
                None => return false,
            },
        )
    };
    let set_stream_index: unsafe extern "system" fn(usize, u32) = unsafe {
        core::mem::transmute(
            match crate::experiments::gated_game_fn(
                PROFILE_RENDERER_SET_STREAM_INDEX_RVA,
                "PROFILE_RENDERER_SET_STREAM_INDEX_RVA",
            ) {
                Some(address) => address,
                None => return false,
            },
        )
    };
    let set_req_754: unsafe extern "system" fn(usize) = unsafe {
        core::mem::transmute(
            match crate::experiments::gated_game_fn(
                PROFILE_RENDERER_SET_REQ_754_RVA,
                "PROFILE_RENDERER_SET_REQ_754_RVA",
            ) {
                Some(address) => address,
                None => return false,
            },
        )
    };
    let set_req_755: unsafe extern "system" fn(usize) = unsafe {
        core::mem::transmute(
            match crate::experiments::gated_game_fn(
                PROFILE_RENDERER_SET_REQ_755_RVA,
                "PROFILE_RENDERER_SET_REQ_755_RVA",
            ) {
                Some(address) => address,
                None => return false,
            },
        )
    };
    let b290 = unsafe { safe_read_u8(record + PROFILE_SUMMARY_GENDER_OFFSET) }.unwrap_or(0);
    let b294 = unsafe { safe_read_u8(record + PROFILE_SUMMARY_FIELD_294_OFFSET) }.unwrap_or(0);
    // Latch semantics (static RE 2026-07-03): the state machine is Wait_Request --754--> build
    // pipeline --> Wait_Play (live), and Wait_Play routes 755/756 to STEP_Finish_Play = a 6-tick
    // TEARDOWN (unregisters the offscreen scene, destroys the model, clears 755+756). So 754+755
    // together mean "tear down the CURRENT model, then rebuild" -- the engine's data-change
    // sequence for a live renderer. On a renderer with no model (our post-Continue case, machine
    // in Wait_Request) the 754 is consumed immediately and the still-armed 755 then destroys the
    // freshly built model six ticks after it reaches Wait_Play, latches clear, dead forever (runs
    // #7/#8: 754 gone 96ms post-kick, ~9 live frames, rgba_version=1). Arm 755 only when there is
    // actually a model to tear down.
    let model_live =
        unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
    unsafe {
        // Runs the whole native model-source sequence synchronously: ChrAsm::Copy from the record,
        // eight `EquipItemBySpecialIndex` clears (edx = {0,1,2,3,6,7,8,9} -- slots 4 and 5 are
        // skipped, so it is not a 0..9 sweep), then equip a freshly-resolved default (bare-body)
        // protector into slots 2 (hands) and 3 (legs). Nothing is re-equipped on top: the record's
        // ChrAsm already carries the character's own protector param ids, and the render path resolves
        // armor from those ids alone (see `runtime_chr_asm_image`). What the resulting live ChrAsm
        // actually resolves to is measured per frame by `portrait_equip_oracle_sample`.
        set_model_source(renderer, record + PROFILE_SUMMARY_CHR_ASM_OFFSET);
        // Undo it: see `er_loading_portrait_core::portrait_equip_restore`. Only window there is.
        er_loading_portrait_core::portrait_equip_restore_apply(
            renderer,
            record + PROFILE_SUMMARY_CHR_ASM_OFFSET,
            slot,
        );
        let fd = facedata_buffer(record + PROFILE_SUMMARY_FACE_DATA_OFFSET, 1);
        set_facedata(renderer, fd);
        set_byte290(renderer, b290);
        set_flag_one(renderer, 1);
        set_byte294(renderer, b294);
        set_stream_index(renderer, (slot as u32) * 2);
        set_req_754(renderer);
        if valid(model_live) {
            set_req_755(renderer);
        }
    }
    // Face-identity SEMAPHORE (user directive 2026-07-06): re-hash the record's inner FaceDataBuffer
    // at kick time and compare against the fingerprint stored when the foreign-save preview wrote this
    // slot. Drift means the portrait model is about to be built from a different character's face than
    // the one the user picked -- the wrong-head class that previously only human eyes caught (Banon
    // rendered under HopeAfterRainTTV's name across three QA runs). Telemetry-only per the default
    // non-fatal research posture: counters + log line; probe watchers fail-fast on the oracle.
    let expected_face = PROFILE_PREVIEW_FACE_HASH[slot as usize].load(Ordering::SeqCst);
    if expected_face != 0 {
        PORTRAIT_FACE_IDENTITY_CHECKS.fetch_add(1, Ordering::SeqCst);
        let inner = record + PROFILE_SUMMARY_FACE_DATA_OFFSET + FACE_DATA_BUFFER_OFFSET;
        let bytes =
            unsafe { core::slice::from_raw_parts(inner as *const u8, FACE_DATA_BUFFER_TOTAL_SIZE) };
        let got = er_gfx::title_05_000::fnv1a64(bytes) as usize;
        if got != expected_face {
            let n = PORTRAIT_FACE_IDENTITY_MISMATCHES.fetch_add(1, Ordering::SeqCst) + 1;
            append_autoload_debug(format_args!(
                "loading-portrait: FACE-IDENTITY MISMATCH #{n} at build kick slot={slot}: record face hash 0x{got:x} != preview 0x{expected_face:x} -- the portrait would render the WRONG character"
            ));
        }
        // And act on it, not just count it. This comparison is the pipeline's only identity signal
        // whose two sides come from different places (the live record vs a fingerprint taken from
        // the picked save's own bytes at preview time), so it is the only one that can falsify the
        // same-identity bridge hold -- whose own predicate compares slot N's record with slot N's
        // record and therefore matches whenever the same slot is re-selected. On 2026-08-22 that
        // hold kept the previous character's head and its crop envelope for a whole 3.1s window
        // while this check disagreed twice and nothing consumed the disagreement.
        er_loading_portrait_core::loading_portrait_bridge_hold_face_check(slot, got, expected_face);
    }
    PORTRAIT_KICK_SLOT_KEY.store((slot + 1) as usize, Ordering::SeqCst);
    PORTRAIT_KICK_RENDERER.store(renderer, Ordering::SeqCst);
    // Kick identity (bd er-effects-rs-dpf6 Phase 1): stamp the target's name hash on the game thread
    // (from the same summary record the kick configures) so the consume worker -- which may not read
    // game memory -- can copy it next to the bridge at publish. `record` == summary record base
    // (name UTF-16 units at +0; verified: kick log record=0x92041c98 == summary 0x92041c80 + 0x18).
    PORTRAIT_TARGET_NAME_HASH.store(
        unsafe { portrait_record_name_hash(record) },
        Ordering::SeqCst,
    );
    let kicks = PROFILE_TARGET_KICKS.fetch_add(1, Ordering::SeqCst) + 1;
    if kicks <= 4 {
        append_autoload_debug(format_args!(
            "loading-portrait: per-slot build kick #{kicks} for LOADED slot {slot} (renderer=0x{renderer:x} record=0x{record:x}) -- global refresh not called, other slots stay unbuilt"
        ));
    }
    true
}

/// [`portrait_loaded_slot_confirmed`] collapsed to a bare slot index, with `0` standing in for
/// "nothing named a slot". Routing every portrait site through one loaded-character source is the
/// er-effects-rs-j3r correlation fix; this form exists only for display-side readers where a wrong
/// slot reads inert (no model is built for it, so it draws nothing).
///
/// Prefer [`portrait_loaded_slot_confirmed`] and handle `None`. The `unwrap_or(0)` here is a lie by
/// omission -- it reports slot 0 with the same confidence as a real answer. Anything that builds,
/// captures or publishes must not use it: the publish gate did, and with no confirmed slot it
/// published slot 0's head as though that were the loaded character (bd er-effects-rs-91zb).
pub(crate) fn portrait_loaded_slot() -> i32 {
    portrait_loaded_slot_confirmed().unwrap_or(0)
}

/// The loaded slot only when a real source names it -- `None` while none is valid yet. The build
/// kick must use this form: the old fallback-to-0 kicked a
/// slot-0 build ~340ms before ac0 flipped to the real slot (run anim-bind5, kicks #1 slot0 /
/// #2 slot5), and with the rebuild storm fixed that foreign model now persists -- the
/// `count_live_profile_models == 1` stability gate then blocks the whole live-drive/publish/anim
/// pipeline for the rest of the load (1 motion sample all window). Display-side readers may still
/// use the collapsed `portrait_loaded_slot()` form (with no model built, a wrong slot reads inert).
///
/// Implemented but UNPROVEN (bd er-effects-rs-91zb; needs one live run -- build success and kick
/// counters prove nothing about which face reached the screen). `GameMan.save_slot` (ac0) is not
/// authoritative for "which character is loading": both the game's own selector and our own
/// `set_save_slot(picked)` (own_load/loaders.rs, ~5s before the deserialize) write it for reasons
/// unrelated to that question. Measured 2026-08-02 18:07: the user picked slot 5, ac0 was
/// correctly set to 5, the native selector then dragged ac0 to 9 while the request register held
/// the correct 5 the whole time -- and slot 9's face sat on the loading screen for 29.7 seconds.
///
/// So the sources are consulted strongest-first (see
/// `er_loading_portrait_core::portrait_target_slot_attributed`, which is host-tested):
///   1. the user's explicit on-screen save-picker pick, until that slot's load completes;
///   2. `GameMan+0xb78`, the native load-request register -- a load for it is in flight, so ac0
///      is stale by definition. `-1` is its no-request sentinel and falls through;
///   3. ac0.
///
/// The `OWN_STEPPER_SLOT` AUTOLOAD hint is no longer a source, and removing it is the fix for
/// run br-20260831-014208-b1d6. It used to sit at the end of that list as an `.or_else`, i.e. at
/// the same rank as a measured read -- so when every register was still invalid the window
/// committed to a CONFIG file's opinion and then defended it as though it had observed something:
///
/// ```text
/// [+1184ms]   window LATCHED portrait target slot 0 (picker=None b78=None ac0=-1 from_pick=false)
/// [+19552ms]  SUPPRESSED a mid-window retarget 0 -> 2 (picker=None b78=Some(-1) ac0=2)
/// ```
///
/// Slot 2 (`Bean Smith`) was loading, correctly -- that run configured no slot, so the native
/// builder used the container's persisted last-used slot -- while slot 0 (`Bonky Bean`) was on
/// screen, and the latch refused the real ac0 3371 times. A configured hint and an observed fact
/// are not the same rank of evidence, and when only the hint is available the honest answer is
/// `None`: no portrait is better than a confidently wrong one. The hint still steers the load
/// (`OWN_STEPPER_SLOT`, `configured_autoload_slot`); it just no longer claims to have seen it.
pub(crate) fn portrait_loaded_slot_confirmed() -> Option<i32> {
    let ac0 = (unsafe { eldenring::cs::GameMan::instance() })
        .map(er_save_loader::GameManSaveAccess::save_slot)
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    // The user's boot pick outranks everything the game infers -- but only for its own load. The
    // missing-save picker is a boot surface, so its pick is spent the moment the world it asked for
    // exists; `IN_WORLD_REACHED` is the monotonic latch for that and never un-suppresses. Without
    // this bound the stale boot pick would outrank ac0 forever and pin the portrait to the boot
    // character across every later System->Quit->Load switch -- the same wrong-face class in the
    // opposite direction. (A switch's own selection is handled upstream by `portrait_target_slot`.)
    let picker = (IN_WORLD_REACHED.load(Ordering::SeqCst) != IN_WORLD_REACHED_YES)
        .then(missing_save_picker_selected_slot)
        .flatten();
    let gm = game_man_ptr_or_null();
    let request = (gm != TITLE_OWNER_SCAN_START_ADDRESS)
        .then(|| unsafe { safe_read_i32(gm + GAME_MAN_SLOT_SELECT_B78_OFFSET) })
        .flatten();
    let resolved = er_loading_portrait_core::portrait_target_slot_attributed(
        picker,
        request,
        Some(ac0),
        TITLE_PROFILE_SLOT_COUNT as i32,
    );
    // Per-window latch. Everything above is a precedence ordering re-evaluated on every kick, so
    // its answer can change while a loading screen is on screen -- and when it does, the user
    // watches the face of the character they clicked be replaced by someone else's. Measured
    // 2026-08-02 21:05: picked slot 0, published slot 0 at +17775ms, the picker term expired
    // (spent on IN_WORLD_REACHED = *a* world exists, not *that slot's* world), precedence fell
    // through to ac0=9, kick #2 retargeted the same window at +20998ms, window closed +29989ms.
    //
    // The window keeps whatever it first committed to; the reset in
    // `loading_portrait_window_reset_inner` releases it so the next load can differ. Deliberately
    // not an attempt to decide which slot is correct -- that belongs to the load path, and a
    // portrait that stays wrong for one window beats one that changes identity mid-load.
    let latched = match PORTRAIT_WINDOW_TARGET_SLOT.load(Ordering::SeqCst) {
        0 => None,
        packed => Some((packed - 1) as i32),
    };
    // Which source the latch rests on, on each side of the decision. A latch yields exactly once
    // per rank increase, to strictly better evidence: an ac0 read (weakest) yields to the load
    // request register and to a pick, a pick yields to nothing. The old boolean form could only
    // say "pick or not pick", which filed a stale ac0 read and the user's own click under one
    // word -- so the across-save-files switch latched slot 0 off an obsolete ac0 and then refused
    // the b78 that named the real slot 1 (bd er-effects-rs-fmy6).
    let latched_source = er_loading_portrait_core::PortraitSlotSource::from_rank(
        PORTRAIT_WINDOW_TARGET_SOURCE.load(Ordering::SeqCst),
    );
    let decision = er_loading_portrait_core::portrait_window_target_slot_by_evidence(
        latched,
        latched_source,
        resolved,
    );
    let target = decision.slot;
    // Stored on every call, not only when latching: a resolution that agrees with the held slot
    // but carries stronger evidence tightens the latch without changing the face, and dropping
    // that upgrade would leave an already-confirmed target movable.
    PORTRAIT_WINDOW_TARGET_SOURCE.store(
        decision.source.map_or(0, |source| source.rank()),
        Ordering::SeqCst,
    );
    PORTRAIT_WINDOW_TARGET_FROM_PICK.store(
        if decision.source == Some(er_loading_portrait_core::PortraitSlotSource::UserPick) {
            PORTRAIT_WINDOW_TARGET_PICK_YES
        } else {
            PORTRAIT_WINDOW_TARGET_PICK_NO
        },
        Ordering::SeqCst,
    );
    let source_label = decision.source.map_or("none", |source| source.label());
    if decision.latching
        && let Some(slot) = target
    {
        PORTRAIT_WINDOW_TARGET_SLOT.store(slot as usize + 1, Ordering::SeqCst);
        if latched.is_some() {
            if decision.promoted_by_pick {
                PORTRAIT_WINDOW_TARGET_PICK_PROMOTIONS.fetch_add(1, Ordering::SeqCst);
            }
            append_autoload_debug(format_args!(
                "loading-portrait: window PROMOTED portrait target {} -> {slot} on stronger evidence src={source_label} (was src={} picker={picker:?} b78={request:?} ac0={ac0}) -- the earlier latch rested on weaker evidence than this",
                latched.unwrap_or(-1),
                latched_source.map_or("none", |source| source.label())
            ));
        } else {
            append_autoload_debug(format_args!(
                "loading-portrait: window LATCHED portrait target slot {slot} src={source_label} (picker={picker:?} b78={request:?} ac0={ac0}) -- held until this loading screen closes, or until strictly better evidence arrives"
            ));
        }
    } else if let (Some(held), Some((fresh, fresh_source))) = (target, resolved)
        && held != fresh
        && PORTRAIT_WINDOW_RETARGETS_SUPPRESSED.fetch_add(1, Ordering::SeqCst) == 0
    {
        append_autoload_debug(format_args!(
            "loading-portrait: SUPPRESSED a mid-window retarget {held} -> {fresh} (held src={source_label} vs fresh src={} picker={picker:?} b78={request:?} ac0={ac0}) -- the fresh read is no better evidence than what this window already committed to",
            fresh_source.label()
        ));
    }
    target
}

/// Torn-READBACK score: average absolute vertical luma step across the masked (alpha != 0, i.e. head)
/// region of a readback RGBA frame. A clean face render varies smoothly row-to-row (small steps); a
/// torn readback (rows captured mid-GPU-write, no cross-queue sync) has random per-row discontinuities
/// (large steps -> the scanline garbage the user saw). Returns 0..255. Columns are subsampled by 2 for
/// cost; every row is compared so single-row tears still register. 0 when there is no masked content.
pub(crate) fn portrait_tear_score(cpx: &[u8], w: usize, h: usize) -> usize {
    if w < 2 || h < 2 || cpx.len() < w * h * 4 {
        return 0;
    }
    let luma = |i: usize| -> i32 {
        let p = i * 4;
        (cpx[p] as i32 * 30 + cpx[p + 1] as i32 * 59 + cpx[p + 2] as i32 * 11) / 100
    };
    let mut sum = 0u64;
    let mut n = 0u64;
    let mut y = 1;
    while y < h {
        let mut x = 0;
        while x < w {
            let i = y * w + x;
            // Only score head pixels (alpha != 0). The mask sets background alpha to 0, so a torn
            // frame's head region is where the scanline garbage shows.
            if cpx[i * 4 + 3] != 0 {
                let d = (luma(i) - luma((y - 1) * w + x)).unsigned_abs() as u64;
                sum += d;
                n += 1;
            }
            x += 2;
        }
        y += 1;
    }
    sum.checked_div(n).unwrap_or(0) as usize
}

/// The slot whose portrait the loading-screen pipeline should target (spare + render + display): the
/// character the user just selected for a System->Quit->Load switch
/// (`SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT`, set at the confirm press -- known before the deserialize
/// flips ac0), falling back to `portrait_loaded_slot()` (ac0 / the boot autoload hint) when no switch
/// selection is pending. This is what lets the loading portrait show the newly-selected character
/// during the pre-continue window instead of the still-resident old one: at the confirm the new slot's
/// renderer is already built + live in the ProfileSelect table, so we can spare/render it, while ac0
/// still names the old character until the reload deserializes.
pub(crate) fn portrait_target_slot() -> i32 {
    let sel = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
    if sel <= i32::MAX as usize {
        let sel = sel as i32;
        if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&sel) {
            return sel;
        }
    }
    portrait_loaded_slot()
}

/// Fail-fast character-identity semaphore for the loading-screen portrait (er-effects-rs-j3r; user
/// directive 2026-07-02: verify in-game, from RAM identity -- Not rendered pixels -- that the
/// character our portrait code renders is the one the game actually loaded). Two independent sources:
///   Our side  = the ProfileSummary save record of the slot our portrait targets (`render_target_slot`
///               = `portrait_loaded_slot()`): its stored character name + saved map (record+0x30).
///   Game side = the live loaded character: PlayerGameData name (`char_fingerprint`) + GameMan c30 map.
/// The save-record table and the in-world character live in distinct memory, so a wrong-slot render (or
/// a wrong-character load) makes them disagree -- Non-tautological even though our target derives from
/// ac0 (a slot index): this compares the character stored in that slot against who is actually resident.
/// Determines "is it the expected slot" without any pixel readback (the user's constraint: pixels are
/// too slow / the wrong tool). On a mismatch (a real character is loaded but its NAME/MAP != our target
/// slot's record), record the oracle + a crash-log line. Deliberate faulting is release/fail-fast-only;
/// normal runtime research must leave the game alive so the underlying game/DLL behavior can continue
/// producing evidence. Gated on a real loaded character and a real record, so pre-load transients and
/// empty slots never fire.
pub(crate) unsafe fn portrait_render_slot_semaphore(base: usize, render_target_slot: i32) {
    // The new-game / not-yet-resolved saved-map sentinel and the "is this a real packed BlockId"
    // predicate both live in er-loading-portrait-core, where they are host-tested.
    use er_loading_portrait_core::portrait_identity::packed_maps_disagree;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;

    // Game side: require a real loaded character before asserting anything.
    if !unsafe { char_fingerprint(base).0 } {
        return; // no real character loaded yet -- pre-load transient.
    }
    let gdm = game_data_man_ptr_or_null();
    if gdm == null {
        return;
    }
    let pgd =
        unsafe { safe_read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }.unwrap_or(null);
    if pgd == null {
        return;
    }
    let (live_name, live_len) = unsafe { read_utf16_name_units(pgd + PGD_NAME_9C_OFFSET) };
    let gm = game_man_ptr_or_null();
    let live_map = if gm != null {
        unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(-1)
    } else {
        -1
    };

    // Our side: the save-record identity of the slot our portrait code targets.
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&render_target_slot) {
        return;
    }
    let profile_summary =
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(null);
    if profile_summary == null {
        return;
    }
    let rec = profile_summary_record_address(profile_summary, render_target_slot as usize);
    let (our_name, our_len) = unsafe { read_utf16_name_units(rec) };
    if utf16_name_empty_like(&our_name, our_len) {
        return; // our target slot stores no real character -- nothing meaningful to compare.
    }
    let our_map = unsafe { safe_read_i32(rec + PROFILE_SUMMARY_MAP_OFFSET) }.unwrap_or(-1);

    // Compare RAM identities. Name is the character identity and carries the check on its own.
    let name_match = our_len == live_len && our_name[..our_len] == live_name[..live_len];

    // Map is only a second discriminator, and only when the record is ours. Both sides are the
    // same type but not always the same QUANTITY: our foreign-save preview writes record+0x30 from
    // the slot body's saved `BlockId` (= exactly what deserializes into c30), whereas a
    // game-written record is filled from `GetCurrentMapId` (`FUN_140262270`). Measured on the
    // corpus: 65 of 726 active slots have body+0x04 = 0x0e000000 while the game's own record says
    // 0x1c000000 -- a systematic, entirely legitimate difference that would fire this semaphore on
    // every one of those characters. `PROFILE_PREVIEW_FACE_HASH[slot] != 0` marks a record our
    // preview wrote with foreign visual data (set in the same call as the +0x30 write, cleared when
    // the preview is dropped), so the map term applies only where the two sides are comparable.
    // It errs toward silence: a preview that wrote +0x30 but could not locate the face leaves the
    // hash at 0 and the map term off. That is a missed detection, never a false alarm, and the name
    // axis still carries the check -- `record_is_ours` is logged so the distinction is visible.
    //
    // The plausibility test is an areaId range, not the old `> 0` sign gate: a packed BlockId's
    // sign bit is just bit 7 of its areaId and means nothing, so garbage with bit31 set silently
    // switched the map term off instead of failing it (2 of the 6 logged FAILs).
    let record_is_ours =
        PROFILE_PREVIEW_FACE_HASH[render_target_slot as usize].load(Ordering::SeqCst) != 0;
    let map_mismatch = record_is_ours && packed_maps_disagree(our_map, live_map);
    if name_match && !map_mismatch {
        return; // our portrait's character == the loaded character (RAM identity match).
    }

    // Self-classifying failure line: everything needed to tell a real wrong-character render from a
    // pre-load transient, without re-deriving it from a second log. Previously our_name/live_name
    // were in scope and thrown away, so every fail had to be re-litigated by hand.
    let gm_ptr = game_man_ptr_or_null();
    let ac0 = (unsafe { eldenring::cs::GameMan::instance() })
        .map(er_save_loader::GameManSaveAccess::save_slot)
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    let b78 = if gm_ptr != null {
        unsafe { safe_read_i32(gm_ptr + GAME_MAN_SLOT_SELECT_B78_OFFSET) }.unwrap_or(i32::MIN)
    } else {
        i32::MIN
    };
    let quickload_phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    let fresh_deser = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst);
    // Where the target came from, in the same line as the mismatch it caused. `ac0`/`b78` above
    // are what the registers read now; the window latched its target once, possibly minutes
    // earlier, from whatever they read then -- and the two answers being different is the entire
    // failure mode this semaphore keeps catching. Diagnosing br-20260831-014208-b1d6 meant
    // finding the `window LATCHED` line 12,000 lines away in a 1.9 GB log to learn that the latch
    // had been taken with every source invalid. That provenance belongs here.
    let window_latch = match PORTRAIT_WINDOW_TARGET_SLOT.load(Ordering::SeqCst) {
        0 => -1,
        packed => (packed - 1) as i32,
    };
    let window_src = er_loading_portrait_core::PortraitSlotSource::from_rank(
        PORTRAIT_WINDOW_TARGET_SOURCE.load(Ordering::SeqCst),
    )
    .map_or("none", |source| source.label());
    let window_from_pick =
        PORTRAIT_WINDOW_TARGET_FROM_PICK.load(Ordering::SeqCst) == PORTRAIT_WINDOW_TARGET_PICK_YES;
    let window_promotions = PORTRAIT_WINDOW_TARGET_PICK_PROMOTIONS.load(Ordering::SeqCst);
    let window_suppressed = PORTRAIT_WINDOW_RETARGETS_SUPPRESSED.load(Ordering::SeqCst);
    let provenance = format!(
        "window_latch={window_latch} window_src={window_src} from_pick={window_from_pick} promotions={window_promotions} retargets_suppressed={window_suppressed}"
    );
    let cond = ((!name_match) as usize) | ((map_mismatch as usize) << 1);
    // areaId (the high byte), not `& 0xff` -- the low byte of a packed BlockId is the indexId and
    // is 0 on nearly every real map, so the old packing threw away the only discriminating byte.
    PORTRAIT_RENDER_SEMAPHORE_STATE.store(
        ((render_target_slot as u32 as usize) << 16)
            | ((er_loading_portrait_core::portrait_identity::packed_map_area_id(our_map) as usize)
                << 8)
            | cond,
        Ordering::SeqCst,
    );
    let our_text = String::from_utf16_lossy(&our_name[..our_len]);
    let live_text = String::from_utf16_lossy(&live_name[..live_len]);
    if PORTRAIT_RENDER_SEMAPHORE_LOGGED.swap(1, Ordering::SeqCst) == 0 {
        append_crash_log(format_args!(
            "PORTRAIT-IDENTITY-SEMAPHORE FAIL: our portrait targets slot={render_target_slot} (record name='{our_text}' len={our_len} map=0x{our_map:x}) but the LOADED character is name='{live_text}' len={live_len} map=0x{live_map:x} -- name_match={name_match} map_mismatch={map_mismatch} record_is_ours={record_is_ours} ac0={ac0} b78={b78} {provenance} quickload_phase={quickload_phase} fresh_deser={fresh_deser}. Our portrait is not the loaded character (er-effects-rs-j3r); deliberate fault only if ER_QUICKLOAD_FAIL_FAST=1"
        ));
        append_autoload_debug(format_args!(
            "PORTRAIT-IDENTITY-SEMAPHORE FAIL: target_slot={render_target_slot} record(name='{our_text}' len={our_len} map=0x{our_map:x}) vs loaded(name='{live_text}' len={live_len} map=0x{live_map:x}) name_match={name_match} map_mismatch={map_mismatch} record_is_ours={record_is_ours} ac0={ac0} b78={b78} {provenance} quickload_phase={quickload_phase} fresh_deser={fresh_deser}"
        ));
    }
    if crate::crashlog::deliberate_fail_fast_enabled() {
        // Deliberate null-page fault: crash_vectored_handler logs full context, returns
        // EXCEPTION_CONTINUE_SEARCH, and the run terminates -- release/fail-fast proof mode only.
        unsafe {
            core::ptr::write_volatile(PORTRAIT_RENDER_SEMAPHORE_FAULT_ADDR as *mut u8, 0u8);
        }
    }
}

pub(crate) use er_telemetry_core::counters::PORTRAIT_FACE_IDENTITY_CHECKS;
pub(crate) use er_telemetry_core::counters::PORTRAIT_FACE_IDENTITY_MISMATCHES;
pub(crate) const SYSTEM_QUIT_SAVE_SWAP_POLL_INTERVAL_TICKS: usize = 30;

#[derive(Default)]
pub(crate) struct SystemQuitSaveSwapState {
    pub(crate) armed: bool,
    pub(crate) path: String,
    pub(crate) original_bytes: Vec<u8>,
    pub(crate) original_hash: u64,
    pub(crate) original_len: u64,
    pub(crate) original_modified_ns: u128,
    pub(crate) candidate_bytes: Vec<u8>,
    pub(crate) candidate_hash: u64,
    pub(crate) candidate_slot_mask: usize,
    pub(crate) candidate_stats_utf16: Vec<Vec<u16>>,
    pub(crate) preview_applied: bool,
    pub(crate) committed: bool,
    /// The candidate bytes were written a second time, after the game's return-title save finished
    /// (same-slot clobber fix; see `system_quit_save_swap_recommit_after_return_title_save`).
    pub(crate) recommitted: bool,
    pub(crate) summary_ptr: usize,
    pub(crate) summary_snapshot: Vec<u8>,
    // --- Save-PICKER row staging. A separate concern from the preview above ---------------------
    //
    // The in-game file picker renders its browse rows by writing them into the live
    // `CS::ProfileSummary`: `save_picker_write_row_records` memsets each 0x2a0-byte record to zero
    // and copies the row label into the name field. That destroys the game's own records -- face
    // data (+0x38), ChrAsm (+0x1a8), level (+0x24), map (+0x30) -- and they must be put back when
    // the picker is done, on every exit: close, commit, and abort.
    //
    // This used to ride on `preview_applied`/`committed` and that CONFLATION was the bug. Row
    // staging and the foreign-save preview are different things that happen to write the same
    // allocation, but only the preview may be suppressed by `committed` (a committed preview must
    // survive to be loaded). Sharing one bit meant an earlier cross-file load commit set
    // `committed` -- which nothing resets except the tail of the restore it was blocking -- and
    // every later picker's rows were then left in the records for the rest of the process. The
    // user's loading screens rendered `[..] EldenRing` and `[ new ]` as character names.
    //
    // So the staging keeps its own latch and its own snapshot, restored unconditionally.
    /// The live ProfileSummary currently holds picker browse-row labels, not the game's records.
    pub(crate) rows_staged: bool,
    /// The allocation `rows_snapshot` was taken from; a restore refuses to write anywhere else.
    pub(crate) rows_summary_ptr: usize,
    /// The `PROFILE_SUMMARY_TOTAL_BYTES` image of that allocation as it looked before the first
    /// staging of the current picker session. Re-taken per session (the latch is cleared by every
    /// restore), never a process-lifetime latch -- a stale one would resurrect a previous save
    /// container's records, which is the other half of `er-effects-rs-fmy6`.
    pub(crate) rows_snapshot: Vec<u8>,
}

pub(crate) static SYSTEM_QUIT_SAVE_SWAP_STATE: OnceLock<Mutex<SystemQuitSaveSwapState>> =
    OnceLock::new();

pub(crate) fn system_quit_save_swap_state() -> &'static Mutex<SystemQuitSaveSwapState> {
    SYSTEM_QUIT_SAVE_SWAP_STATE.get_or_init(|| Mutex::new(SystemQuitSaveSwapState::default()))
}

pub(crate) fn system_quit_save_swap_lock() -> std::sync::MutexGuard<'static, SystemQuitSaveSwapState>
{
    system_quit_save_swap_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn system_quit_hash_bytes(bytes: &[u8]) -> u64 {
    er_game_base::fnv1a::fnv1a64(bytes)
}

pub(crate) fn system_quit_file_stamp(path: &str) -> Option<(u64, u128)> {
    let meta = fs::metadata(path).ok()?;
    let modified_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some((meta.len(), modified_ns))
}

pub(crate) fn system_quit_save_swap_arm_original(path: &str) -> bool {
    let Ok(bytes) = fs::read(path) else {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: failed to snapshot active save '{path}' before opening replacement folder"
        ));
        return false;
    };
    let Some((len, modified_ns)) = system_quit_file_stamp(path) else {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: failed to stat active save '{path}' before opening replacement folder"
        ));
        return false;
    };
    let hash = system_quit_hash_bytes(&bytes);
    let mut st = system_quit_save_swap_lock();
    // Carry the row-staging snapshot across this reset. Arming is a save-file concern and this
    // wholesale reset is how the preview's bookkeeping is cleared, but the picker's staged rows are
    // live UI state that can already be on screen when a re-arm happens. Dropping their snapshot
    // here would leave labels in the records with nothing left to restore them from.
    let (rows_staged, rows_summary_ptr, rows_snapshot) = (
        st.rows_staged,
        st.rows_summary_ptr,
        core::mem::take(&mut st.rows_snapshot),
    );
    *st = SystemQuitSaveSwapState {
        armed: true,
        path: path.to_owned(),
        original_bytes: bytes,
        original_hash: hash,
        original_len: len,
        original_modified_ns: modified_ns,
        rows_staged,
        rows_summary_ptr,
        rows_snapshot,
        ..SystemQuitSaveSwapState::default()
    };
    append_autoload_debug(format_args!(
        "system-quit-save-swap: armed active-save snapshot path='{path}' len={len} hash=0x{hash:016x}; replacement preview will restore this file unless a foreign slot is selected"
    ));
    true
}
