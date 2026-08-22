use super::*;

/// Reads `CSNowLoadingHelperImp::load_done` off the NowLoading singleton. WARNING (RE-corrected
/// 2026-07-02): despite the name this is a load-COMPLETE latch, not "loading screen visible" -- `Update`
/// copies it from `request_load_done` (raised by the map-load system), so it reads true AFTER the load
/// finishes and lingers into gameplay. Do NOT use it to decide the portrait overlay lifetime; kept for
/// telemetry/parity. Fault-guarded.
pub(crate) unsafe fn now_loading_active(base: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let helper = unsafe { safe_read_usize(base + RuntimeGlobalRva::NowLoadingSingleton as usize) }
        .unwrap_or(0);
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
    let helper =
        unsafe { safe_read_usize(base + RuntimeGlobalRva::FakeLoadingScreenSingleton as usize) }
            .unwrap_or(0);
    if helper == 0 || helper == null {
        0
    } else {
        helper
    }
}

/// True while the `CSFakeLoadingScreenImp` cover plate is VISIBLE: `visible` (+0x8) & 0xff. This is the
/// render-pipeline cover the game draws to HIDE the world teardown/rebuild during a map load. Fault-guarded.
pub(crate) unsafe fn fake_loading_screen_visible(base: usize) -> bool {
    let helper = unsafe { fake_loading_screen_ptr(base) };
    if helper == 0 {
        return false;
    }
    unsafe { safe_read_usize(helper + FAKE_LOADING_SCREEN_VISIBLE_OFFSET) }
        .map(|v| (v & 0xff) != 0)
        .unwrap_or(false)
}

/// The portrait build + draw pipeline must PAUSE only during ACTIVE GAMEPLAY -- the player has reached the
/// world AND the current load has COMPLETED (`load_done`, via now_loading_active) AND no loading cover is
/// up. It MUST re-engage for every subsequent loading screen (notably a System Quit -> Load Profile
/// character switch). The old gate was the bare `IN_WORLD_REACHED == YES` latch, which is set the first
/// time the player reaches the world and NEVER resets -> after the first load the build/draw ticks froze
/// forever, so the head only ever rendered on the FIRST character load (the subsequent-load bug, run
/// head-popfix-loaddone 2026-07-02: after the 2nd deserialize the whole pipeline was silent). Fault-guarded.
pub(crate) unsafe fn portrait_pipeline_idle_in_gameplay(base: usize) -> bool {
    // Also idle while the game's ProfileSelect (Load) menu is open: it renders its own portraits,
    // and our drive/readback stacking on top overflows the GX command queue (see the build gate in
    // maybe_build_profile_table_for_loading). Our pipeline is for the loading SCREEN, after the menu.
    if SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) != 0 {
        return true;
    }
    IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES
        && unsafe { now_loading_active(base) }
        && !unsafe { fake_loading_screen_visible(base) }
}

/// True while the game's native NOW-LOADING screen is actively rendering -- CS::LoadingScreen::Update
/// ticked within the last ~250ms (LOADING_SCREEN_UPDATE_HITS increments each of its frames and stops the
/// instant the screen is destroyed). The portrait build/drive pipeline keys off this to stay engaged
/// THROUGH the native loading screen: on a fast load PlayerIns resolves (IN_WORLD_REACHED) ~1.7s before the
/// loading screen clears, so portrait_pipeline_idle_in_gameplay flips true mid-load and the build/drive
/// pipeline returned before the model could build+render (run32: force_profile_render_tick never reached
/// maybe_build). Wall-clock recency (not a per-call decrement) makes it safe to poll multiple times a frame.
pub(crate) fn native_loading_screen_active() -> bool {
    pub(crate) use er_telemetry::counters::LAST_HITS;
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

/// Count profile-table renderers that currently hold a LIVE character model (+0x778 valid). The game's
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
                == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                .map(&valid)
                .unwrap_or(false)
        {
            n += 1;
        }
    }
    n
}

/// EXPERIMENT (gated by `disable_loading_cover_enabled`): clamp the `CSFakeLoadingScreenImp` cover plate's
/// `visible` byte to 0 so the render pipeline skips drawing it -- exposing the world underneath during a
/// map load. Called every game-task frame; the map-load system raises `visible` once at load start and it
/// stays raised, so a per-frame write to 0 wins for the draw. Only writes when the byte is currently
/// non-zero (no needless writes), and only when a valid cover object is resolved. Reversible: with the gate
/// off this is never called and the game draws its cover normally. Counts writes into a RAM oracle so we
/// can confirm the clamp actually engaged. Fault-guarded (validated pointer + catch_unwind at the caller).
pub(crate) unsafe fn suppress_loading_cover_tick(base: usize) {
    if !disable_loading_cover_enabled() {
        return;
    }
    let helper = unsafe { fake_loading_screen_ptr(base) };
    if helper == 0 {
        return;
    }
    let vis_addr = helper + FAKE_LOADING_SCREEN_VISIBLE_OFFSET;
    let cur = unsafe { safe_read_u8(vis_addr) }.unwrap_or(0);
    if cur != 0 {
        unsafe { core::ptr::write_volatile(vis_addr as *mut u8, 0) };
        let n = LOADING_COVER_SUPPRESS_WRITES.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 4 {
            append_autoload_debug(format_args!(
                "loading-cover-experiment: cleared CSFakeLoadingScreenImp.visible (was {cur}) at 0x{vis_addr:x} (write #{n}) -- world drawn uncovered this frame"
            ));
        }
    }
}

/// POST-CONTINUE PORTRAIT: when the now-loading screen is up but the profile-renderer title table has been
/// torn down (native-continue is menu-free, so the menu never built it, or Continue tore it down), call
/// the engine's own builder ONCE to repopulate the 10-slot table. The existing mark+refresh feed +
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
    // ROOT FIX (2026-07-03, run gxguard2): do NOT build our portrait table while the game's own
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
    // existing mark+refresh feed + look-at + draw + oracle drive it. A live table also RE-ARMS the latch:
    // a subsequent Continue teardown empties it again and we rebuild our own for that load window. Exception:
    // native LoadingScreen is already ticking while the table is still populated. That table is stale for the
    // loadscreen portrait; force the normal builder now so the model build overlaps the earliest loading bar.
    let t0 = unsafe { safe_read_usize(portrait_renderer_table_entry(base, 0)) }.unwrap_or(0);
    let populated = t0 != 0
        && t0 != null
        && unsafe { safe_read_usize(t0) }.unwrap_or(0)
            == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA;
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
    // Table is EMPTY this tick -- count the streak. The menu's own teardown+rebuild is synchronous, so a
    // sustained-empty table across ticks means the Continue teardown ran with no menu rebuild (we've left
    // the menu into the load), which happens ~17s -- well before the now-loading flag flips (~21s on the
    // fast gold-save load). Build as soon as EITHER signal fires so ResMan has time to build the model.
    let streak = if force_loading_screen_rebuild {
        0
    } else {
        PROFILE_TABLE_EMPTY_STREAK.fetch_add(1, Ordering::SeqCst) + 1
    };
    if PROFILE_LOADSCREEN_REBUILT.load(Ordering::SeqCst) != 0 {
        return; // already built our table for this load window
    }
    // HARD SAFETY: never call the builder until the engine/ResMan is provably up, or it access-violates at
    // the title (empty table, ResMan not up). Normally we require the game's own ProfileSelect menu to have
    // built its portrait table once (PROFILE_TABLE_WAS_POPULATED). But a MENU-FREE autoload (native product /
    // staged / native-continue) NEVER shows ProfileSelect, so that latch stays 0 and the builder was blocked
    // forever -- the loading-portrait pipeline never engaged on native (run30 2026-07-15:
    // loadscreen_table_builds=0, model drive never fired). During an ACTIVE native loading screen
    // (loading_screen_started: CS::LoadingScreen::Update ticking + Gauge_3 enabled + max-frame set) ResMan is
    // definitively up (the world is streaming), so the builder is safe THERE even without the menu having
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
    // ORPHAN RECLAIM BACKSTOP (second-load foreign-head fix; primary reclaim is at the switch confirm in
    // system_quit_arm_quickload_autoload). If a prior window's spared renderer is still parked in
    // PROFILE_SPARE_ORPHAN when a NEW loading window takes ownership, it is a live foreign producer: its
    // model + offscreen scene are still registered and keep rendering the PREVIOUS character's head,
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
    // CSMenuProfModelRend self-registers its ResMan model build/draw tasks, so it builds + OWNS its own
    // model with our lifetime -- not borrowed from the torn-down menu. Self-contained off process-lifetime
    // singletons (RE-confirmed).
    let builder: unsafe extern "system" fn() =
        unsafe { core::mem::transmute(base + PROFILE_TABLE_BUILDER_RVA) };
    unsafe { builder() };
    // The loading-cover observer (CSNowLoadingHelperImp ctor/update) is the overlay's PRIMARY end-of-cover
    // signal (update pulses stop == the game dismissed the tips+bar screen). Install it here, at the start
    // of every loading window, instead of relying on the accept-byte-gated product path (which never fired
    // on the strip-default run -> hooks_installed=0 and the overlay had to lean on the in-world latch).
    install_now_loading_helper_observer_hooks();
    // Kick the model build THIS tick: the mark+refresh feed that requests the async character-model build
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
    // BOOT-EPOCH publish-latency anchor (bd er-effects-rs-io53): the switch path stamps
    // PORTRAIT_CONFIRM_MS at the switch confirm, so PORTRAIT_CONFIRM_TO_PUBLISH_MS_LAST never measured
    // the boot window. Stamp the boot anchor here -- the loading-owned table build, observed ~90ms
    // after the boot loading screen opens -- so the boot window's first publish computes the same
    // latency oracle. compare_exchange from 0 so a pending switch-confirm stamp is never clobbered;
    // epoch 0 only (switch epochs keep the confirm-press anchor).
    if crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst) == 0
    {
        let _ = er_telemetry::counters::PORTRAIT_CONFIRM_MS.compare_exchange(
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
        base + PROFILE_TABLE_BUILDER_RVA
    ));
}

/// Kick the ASYNC character-model build for ONE profile slot -- a faithful per-slot replica of the body
/// of the engine's global refresh (dump `FUN_1409aa7d0`), which we no longer call from the post-Continue
/// feed: the global form iterates all 10 slots and kicks every real+marked one, building EVERY save
/// character mid-load (the cross-slot portrait swap). Writing the +0x754/+0x755 latches on the other
/// renderers to mute the global refresh CRASHED (GX command-queue overflow; the latches only mean
/// "requested" on a CONFIGURED renderer). This replica performs the engine's exact per-slot sequence --
/// record lookup, ChrAsm/model-source config, FaceData copy, stream index, then the two request latches --
/// so the target slot builds exactly as the engine would build it, and the non-target renderers stay in
/// the natural never-configured state (flags 0, stepper idle -- the same state empty slots hold forever).
/// Returns true when the kick fired. Fault-guarded reads; skips when the slot was already requested.
pub(crate) unsafe fn kick_target_profile_slot(
    base: usize,
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
    // ONE KICK PER SLOT VALUE PER LOAD WINDOW (engine "refresh on profile-data change" semantics;
    // see PORTRAIT_KICK_SLOT_KEY). Re-kicking on a cadence poisoned the state machine (mid-pipeline
    // the model is dead + latches consumed, so the re-kick re-raised +0x754/+0x755 and Wait_Play
    // re-entered the rebuild state forever = the ~1/s rebuild storm, static portrait, shadow
    // flicker). But a blanket one-shot freezes the WRONG character: `portrait_loaded_slot()` (ac0)
    // can still hold the PREVIOUS session's slot when the first kick fires, and the storm's
    // accidental self-correction was the "swap to the actual character" the user always saw. Keying
    // the latch to the slot gives exactly one corrective kick when ac0 flips to the real slot --
    // a deterministic swap -- and no storm (the same slot never re-kicks). No live-model guard:
    // the corrective kick MUST fire on a live (wrong-record) model, exactly like the engine's
    // data-change refresh.
    if PORTRAIT_KICK_SLOT_KEY.load(Ordering::SeqCst) == (slot + 1) as usize
        && PORTRAIT_KICK_RENDERER.load(Ordering::SeqCst) == renderer
    {
        return false;
    }
    // Engine parity: kick only when BOTH request latches read 0 (a kick is not already in flight).
    if unsafe { safe_read_u8(renderer + 0x754) }.unwrap_or(1) != 0
        || unsafe { safe_read_u8(renderer + 0x755) }.unwrap_or(1) != 0
    {
        return false;
    }
    let record_of: unsafe extern "system" fn(usize, i32) -> usize =
        unsafe { core::mem::transmute(base + PROFILE_SUMMARY_RECORD_RVA) };
    let record = unsafe { record_of(summary, slot) };
    if !valid(record) {
        return false;
    }
    let set_model_source: unsafe extern "system" fn(usize, usize) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_MODEL_SOURCE_RVA) };
    let facedata_buffer: unsafe extern "system" fn(usize, u8) -> usize =
        unsafe { core::mem::transmute(base + PROFILE_FACEDATA_BUFFER_RVA) };
    let set_facedata: unsafe extern "system" fn(usize, usize) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_FACEDATA_RVA) };
    let set_byte290: unsafe extern "system" fn(usize, u8) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_BYTE290_RVA) };
    let set_flag_one: unsafe extern "system" fn(usize, u8) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_FLAG_ONE_RVA) };
    let set_byte294: unsafe extern "system" fn(usize, u8) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_BYTE294_RVA) };
    let set_stream_index: unsafe extern "system" fn(usize, u32) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_STREAM_INDEX_RVA) };
    let set_req_754: unsafe extern "system" fn(usize) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_REQ_754_RVA) };
    let set_req_755: unsafe extern "system" fn(usize) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_REQ_755_RVA) };
    let b290 = unsafe { safe_read_u8(record + PROFILE_SUMMARY_GENDER_OFFSET) }.unwrap_or(0);
    let b294 = unsafe { safe_read_u8(record + PROFILE_SUMMARY_FIELD_294_OFFSET) }.unwrap_or(0);
    // LATCH SEMANTICS (static RE 2026-07-03): the state machine is Wait_Request --754--> build
    // pipeline --> Wait_Play (live), and Wait_Play routes 755/756 to STEP_Finish_Play = a 6-tick
    // TEARDOWN (unregisters the offscreen scene, destroys the model, clears 755+756). So 754+755
    // together mean "tear down the CURRENT model, then rebuild" -- the engine's data-change
    // sequence for a LIVE renderer. On a renderer with NO model (our post-Continue case, machine
    // in Wait_Request) the 754 is consumed immediately and the still-armed 755 then DESTROYS the
    // freshly built model six ticks after it reaches Wait_Play, latches clear, dead forever (runs
    // #7/#8: 754 gone 96ms post-kick, ~9 live frames, rgba_version=1). Arm 755 only when there is
    // actually a model to tear down.
    let model_live =
        unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
    unsafe {
        // Runs the WHOLE native model-source sequence synchronously: ChrAsm::Copy from the record,
        // EIGHT `EquipItemBySpecialIndex` clears (edx = {0,1,2,3,6,7,8,9} -- slots 4 and 5 are
        // skipped, so it is NOT a 0..9 sweep), then equip a freshly-resolved DEFAULT (bare-body)
        // protector into slots 2 (hands) and 3 (legs). Nothing is re-equipped on top: the record's
        // ChrAsm already carries the character's own protector param ids, and the render path resolves
        // armor from those ids alone (see `runtime_chr_asm_image`). What the resulting live ChrAsm
        // actually resolves to is measured per frame by `portrait_equip_oracle_sample`.
        set_model_source(renderer, record + PROFILE_SUMMARY_CHR_ASM_OFFSET);
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
    // FACE-IDENTITY SEMAPHORE (user directive 2026-07-06): re-hash the record's inner FaceDataBuffer
    // at kick time and compare against the fingerprint stored when the foreign-save preview wrote this
    // slot. Drift means the portrait model is about to be built from a DIFFERENT character's face than
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
        // AND ACT ON IT, not just count it. This comparison is the pipeline's ONLY identity signal
        // whose two sides come from different places (the live record vs a fingerprint taken from
        // the picked save's own bytes at preview time), so it is the only one that can falsify the
        // same-identity bridge hold -- whose own predicate compares slot N's record with slot N's
        // record and therefore matches whenever the same slot is re-selected. On 2026-08-22 that
        // hold kept the previous character's head and its crop envelope for a whole 3.1s window
        // while this check disagreed twice and nothing consumed the disagreement.
        er_loading_portrait::loading_portrait_bridge_hold_face_check(slot, got, expected_face);
    }
    PORTRAIT_KICK_SLOT_KEY.store((slot + 1) as usize, Ordering::SeqCst);
    PORTRAIT_KICK_RENDERER.store(renderer, Ordering::SeqCst);
    // KICK IDENTITY (bd er-effects-rs-dpf6 Phase 1): stamp the target's name hash on the game thread
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

/// [`portrait_loaded_slot_confirmed`] COLLAPSED to a bare slot index, with `0` standing in for
/// "nothing named a slot". Routing every portrait site through one loaded-character source is the
/// er-effects-rs-j3r correlation fix; this form exists only for DISPLAY-side readers where a wrong
/// slot reads inert (no model is built for it, so it draws nothing).
///
/// PREFER [`portrait_loaded_slot_confirmed`] AND HANDLE `None`. The `unwrap_or(0)` here is a lie by
/// omission -- it reports slot 0 with the same confidence as a real answer. Anything that BUILDS,
/// CAPTURES or PUBLISHES must not use it: the publish gate did, and with no confirmed slot it
/// published slot 0's head as though that were the loaded character (bd er-effects-rs-91zb).
pub(crate) fn portrait_loaded_slot() -> i32 {
    portrait_loaded_slot_confirmed().unwrap_or(0)
}

/// The loaded slot ONLY when a real source names it -- `None` while none is valid yet. The BUILD
/// KICK must use this form: the old fallback-to-0 kicked a
/// SLOT-0 build ~340ms before ac0 flipped to the real slot (run anim-bind5, kicks #1 slot0 /
/// #2 slot5), and with the rebuild storm fixed that foreign model now PERSISTS -- the
/// `count_live_profile_models == 1` stability gate then blocks the whole live-drive/publish/anim
/// pipeline for the rest of the load (1 motion sample all window). Display-side readers may still
/// use the collapsed `portrait_loaded_slot()` form (with no model built, a wrong slot reads inert).
///
/// IMPLEMENTED BUT UNPROVEN (bd er-effects-rs-91zb; needs one live run -- build success and kick
/// counters prove nothing about which face reached the screen). `GameMan.save_slot` (ac0) is NOT
/// authoritative for "which character is loading": both the game's own selector and our own
/// `set_save_slot(picked)` (own_load/loaders.rs, ~5s before the deserialize) write it for reasons
/// unrelated to that question. Measured 2026-08-02 18:07: the user picked slot 5, ac0 was
/// correctly set to 5, the native selector then dragged ac0 to 9 while the REQUEST register held
/// the correct 5 the whole time -- and slot 9's face sat on the loading screen for 29.7 seconds.
///
/// So the sources are consulted strongest-first (see
/// `er_loading_portrait::portrait_target_slot_from_sources`, which is host-tested):
///   1. the user's explicit on-screen save-picker pick, until that slot's load completes;
///   2. `GameMan+0xb78`, the native load-REQUEST register -- a load for it is in flight, so ac0
///      is stale by definition. `-1` is its no-request sentinel and falls through;
///   3. ac0, then the `OWN_STEPPER_SLOT` autoload hint.
pub(crate) fn portrait_loaded_slot_confirmed() -> Option<i32> {
    let ac0 = (unsafe { eldenring::cs::GameMan::instance() })
        .map(er_save_loader::GameManSaveAccess::save_slot)
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    // The user's boot pick outranks everything the game infers -- but ONLY for its own load. The
    // missing-save picker is a BOOT surface, so its pick is spent the moment the world it asked for
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
    let resolved = er_loading_portrait::portrait_target_slot_from_sources(
        picker,
        request,
        Some(ac0),
        TITLE_PROFILE_SLOT_COUNT as i32,
    )
    .or_else(|| {
        let own = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
        (0..TITLE_PROFILE_SLOT_COUNT as i32)
            .contains(&own)
            .then_some(own)
    });
    // PER-WINDOW LATCH. Everything above is a precedence ordering re-evaluated on every kick, so
    // its answer can change WHILE a loading screen is on screen -- and when it does, the user
    // watches the face of the character they clicked be replaced by someone else's. Measured
    // 2026-08-02 21:05: picked slot 0, published slot 0 at +17775ms, the picker term expired
    // (spent on IN_WORLD_REACHED = *a* world exists, not *that slot's* world), precedence fell
    // through to ac0=9, kick #2 retargeted the same window at +20998ms, window closed +29989ms.
    //
    // The window keeps whatever it first committed to; the reset in
    // `loading_portrait_window_reset_inner` releases it so the next load can differ. Deliberately
    // NOT an attempt to decide which slot is correct -- that belongs to the load path, and a
    // portrait that stays wrong for one window beats one that changes identity mid-load.
    let latched = match PORTRAIT_WINDOW_TARGET_SLOT.load(Ordering::SeqCst) {
        0 => None,
        packed => Some((packed - 1) as i32),
    };
    let (target, latching) = er_loading_portrait::portrait_window_target_slot(latched, resolved);
    if latching && let Some(slot) = target {
        PORTRAIT_WINDOW_TARGET_SLOT.store(slot as usize + 1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "loading-portrait: window LATCHED portrait target slot {slot} (picker={picker:?} b78={request:?} ac0={ac0}) -- held until this loading screen closes"
        ));
    } else if let (Some(held), Some(fresh)) = (target, resolved)
        && held != fresh
        && PORTRAIT_WINDOW_RETARGETS_SUPPRESSED.fetch_add(1, Ordering::SeqCst) == 0
    {
        append_autoload_debug(format_args!(
            "loading-portrait: SUPPRESSED a mid-window retarget {held} -> {fresh} (picker={picker:?} b78={request:?} ac0={ac0}) -- the face the user clicked stays on screen for this window"
        ));
    }
    target
}

/// TORN-READBACK score: average absolute VERTICAL luma step across the masked (alpha != 0, i.e. head)
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

/// The slot whose portrait the loading-screen pipeline should TARGET (spare + render + display): the
/// character the user just SELECTED for a System->Quit->Load switch
/// (`SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT`, set at the confirm press -- known BEFORE the deserialize
/// flips ac0), falling back to `portrait_loaded_slot()` (ac0 / the boot autoload hint) when no switch
/// selection is pending. This is what lets the loading portrait show the NEWLY-selected character
/// during the pre-continue window instead of the still-resident old one: at the confirm the new slot's
/// renderer is already built + live in the ProfileSelect table, so we can spare/render IT, while ac0
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

/// Fail-fast CHARACTER-IDENTITY semaphore for the loading-screen portrait (er-effects-rs-j3r; user
/// directive 2026-07-02: verify IN-GAME, from RAM identity -- NOT rendered pixels -- that the
/// character our portrait code renders is the one the game actually loaded). Two INDEPENDENT sources:
///   OUR side  = the ProfileSummary save RECORD of the slot our portrait targets (`render_target_slot`
///               = `portrait_loaded_slot()`): its stored character NAME + saved MAP (record+0x30).
///   GAME side = the LIVE loaded character: PlayerGameData NAME (`char_fingerprint`) + GameMan c30 map.
/// The save-record table and the in-world character live in distinct memory, so a wrong-slot render (or
/// a wrong-character load) makes them disagree -- NON-tautological even though our target derives from
/// ac0 (a slot index): this compares the CHARACTER stored in that slot against who is actually resident.
/// Determines "is it the expected slot" without any pixel readback (the user's constraint: pixels are
/// too slow / the wrong tool). On a mismatch (a real character is loaded but its NAME/MAP != our target
/// slot's record), record the oracle + a crash-log line. Deliberate faulting is release/fail-fast-only;
/// normal runtime research must leave the game alive so the underlying game/DLL behavior can continue
/// producing evidence. Gated on a real loaded character AND a real record, so pre-load transients and
/// empty slots never fire.
pub(crate) unsafe fn portrait_render_slot_semaphore(base: usize, render_target_slot: i32) {
    // The new-game / not-yet-resolved saved-map sentinel and the "is this a real packed BlockId"
    // predicate both live in er-loading-portrait, where they are host-tested.
    use er_loading_portrait::portrait_identity::packed_maps_disagree;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;

    // GAME side: require a REAL loaded character before asserting anything.
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

    // OUR side: the save-RECORD identity of the slot our portrait code targets.
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

    // Compare RAM identities. NAME is the character identity and carries the check on its own.
    let name_match = our_len == live_len && our_name[..our_len] == live_name[..live_len];

    // MAP is only a second discriminator, and only when the record is OURS. Both sides are the
    // same TYPE but not always the same QUANTITY: our foreign-save preview writes record+0x30 from
    // the slot body's saved `BlockId` (= exactly what deserializes into c30), whereas a
    // GAME-written record is filled from `GetCurrentMapId` (`FUN_140262270`). Measured on the
    // corpus: 65 of 726 active slots have body+0x04 = 0x0e000000 while the game's own record says
    // 0x1c000000 -- a systematic, entirely legitimate difference that would fire this semaphore on
    // every one of those characters. `PROFILE_PREVIEW_FACE_HASH[slot] != 0` marks a record our
    // preview wrote with foreign visual data (set in the same call as the +0x30 write, cleared when
    // the preview is dropped), so the map term applies only where the two sides are comparable.
    // It errs toward silence: a preview that wrote +0x30 but could not locate the face leaves the
    // hash at 0 and the map term off. That is a MISSED detection, never a false alarm, and the NAME
    // axis still carries the check -- `record_is_ours` is logged so the distinction is visible.
    //
    // The plausibility test is an areaId RANGE, not the old `> 0` sign gate: a packed BlockId's
    // sign bit is just bit 7 of its areaId and means nothing, so garbage with bit31 set silently
    // switched the map term OFF instead of failing it (2 of the 6 logged FAILs).
    let record_is_ours =
        PROFILE_PREVIEW_FACE_HASH[render_target_slot as usize].load(Ordering::SeqCst) != 0;
    let map_mismatch = record_is_ours && packed_maps_disagree(our_map, live_map);
    if name_match && !map_mismatch {
        return; // our portrait's character == the loaded character (RAM identity match).
    }

    // Self-classifying failure line: everything needed to tell a real wrong-character render from a
    // pre-load transient, without re-deriving it from a second log. Previously our_name/live_name
    // were in scope and thrown away, so every FAIL had to be re-litigated by hand.
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
    let cond = ((!name_match) as usize) | ((map_mismatch as usize) << 1);
    // areaId (the high byte), NOT `& 0xff` -- the low byte of a packed BlockId is the indexId and
    // is 0 on nearly every real map, so the old packing threw away the only discriminating byte.
    PORTRAIT_RENDER_SEMAPHORE_STATE.store(
        ((render_target_slot as u32 as usize) << 16)
            | ((er_loading_portrait::portrait_identity::packed_map_area_id(our_map) as usize) << 8)
            | cond,
        Ordering::SeqCst,
    );
    let our_text = String::from_utf16_lossy(&our_name[..our_len]);
    let live_text = String::from_utf16_lossy(&live_name[..live_len]);
    if PORTRAIT_RENDER_SEMAPHORE_LOGGED.swap(1, Ordering::SeqCst) == 0 {
        append_crash_log(format_args!(
            "PORTRAIT-IDENTITY-SEMAPHORE FAIL: our portrait targets slot={render_target_slot} (record name='{our_text}' len={our_len} map=0x{our_map:x}) but the LOADED character is name='{live_text}' len={live_len} map=0x{live_map:x} -- name_match={name_match} map_mismatch={map_mismatch} record_is_ours={record_is_ours} ac0={ac0} b78={b78} quickload_phase={quickload_phase} fresh_deser={fresh_deser}. Our portrait is not the loaded character (er-effects-rs-j3r); deliberate fault only if ER_EFFECTS_FAIL_FAST=1"
        ));
        append_autoload_debug(format_args!(
            "PORTRAIT-IDENTITY-SEMAPHORE FAIL: target_slot={render_target_slot} record(name='{our_text}' len={our_len} map=0x{our_map:x}) vs loaded(name='{live_text}' len={live_len} map=0x{live_map:x}) name_match={name_match} map_mismatch={map_mismatch} record_is_ours={record_is_ours} ac0={ac0} b78={b78} quickload_phase={quickload_phase} fresh_deser={fresh_deser}"
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

pub(crate) const SAVE_FACE_MAGIC: &[u8; 4] = b"FACE";
#[allow(dead_code)] // Retained: Save-format fact beside the live SAVE_FACE_MAGIC.
pub(crate) const SAVE_FACE_DATA_BUFFER_SIZE: usize = 0x120;

/// Per-slot FNV-1a64 (truncated to usize) of the FOREIGN character's inner `FaceDataBuffer` as written
/// into the RAM ProfileSummary record by the save-swap preview -- the EXPECTED portrait identity for
/// that slot. 0 = no foreign preview owns the slot. The build kick re-hashes the record at kick time
/// and trips `PORTRAIT_FACE_IDENTITY_MISMATCHES` on drift, so a wrong-face portrait can never again
/// pass a run silently (user directive 2026-07-06: the run must detect the wrong rendered character
/// itself instead of relying on human review of RT dumps).
pub(crate) static PROFILE_PREVIEW_FACE_HASH: [AtomicUsize; TITLE_PROFILE_SLOT_COUNT] =
    [const { AtomicUsize::new(0) }; TITLE_PROFILE_SLOT_COUNT];

/// Bit per slot: the preview rebuilt this slot's record but could NOT source a place name for it.
///
/// The rebuild copies a STRUCTURAL template from the original save's record before overwriting the
/// fields it can supply (see `write_profile_summary_record`), so a slot whose place name the previewed
/// save cannot supply keeps the TEMPLATE character's -- and the map field, which is written from the
/// body, then agrees with the body and makes the record look self-consistent. The map comparison that
/// catches a stale record on a normally-loaded save therefore cannot catch this one; this mask is the
/// record of the fact, set where the failure to source actually happens.
pub(crate) static PROFILE_PREVIEW_PLACE_NAME_UNSOURCED: AtomicUsize = AtomicUsize::new(0);
pub(crate) use er_telemetry::counters::PORTRAIT_FACE_IDENTITY_CHECKS;
pub(crate) use er_telemetry::counters::PORTRAIT_FACE_IDENTITY_MISMATCHES;
pub(crate) const SAVE_PGD_SCAN_LEADING_FACE_COUNT: usize = 4;
pub(crate) const SAVE_PGD_FACE_DELTA_WINDOW_LOW: usize = 0xa000;
pub(crate) const SAVE_PGD_FACE_DELTA_WINDOW_HIGH: usize = 0xa600;
pub(crate) const SAVE_PLAYER_GAME_DATA_MIN_SIZE: usize = 0x1b0;
pub(crate) const SAVE_PGD_HEALTH_OFFSET: usize = 0x08;
pub(crate) const SAVE_PGD_MAX_HEALTH_OFFSET: usize = 0x0c;
pub(crate) const SAVE_PGD_BASE_MAX_HEALTH_OFFSET: usize = 0x10;
pub(crate) const SAVE_PGD_STAT_BASE_OFFSET: usize = 0x34;
pub(crate) const SAVE_PGD_STAT_COUNT: usize = 8;
pub(crate) const SAVE_PGD_LEVEL_OFFSET: usize = 0x60;
pub(crate) const SAVE_PGD_RUNE_MEMORY_OFFSET: usize = 0x68;
pub(crate) const SAVE_PGD_CHARACTER_NAME_OFFSET: usize = 0x94;
pub(crate) const SAVE_PGD_CHARACTER_NAME_UNITS: usize = 0x10;
pub(crate) const SAVE_PGD_CHARACTER_NAME_BYTES: usize = SAVE_PGD_CHARACTER_NAME_UNITS * 2;
pub(crate) const SAVE_PGD_GENDER_OFFSET: usize = 0xb6;
// The serialized PGD tracks the runtime struct at (runtime - 8) for these early scalars (level
// 0x68->0x60, name 0x9c->0x94, gender 0xbe->0xb6): archetype/gift/c4 follow the same shift.
pub(crate) const SAVE_PGD_ARCHETYPE_OFFSET: usize = 0xb7;
pub(crate) const SAVE_PGD_STARTING_GIFT_OFFSET: usize = 0xbb;
pub(crate) const SAVE_PGD_FIELD_C4_OFFSET: usize = 0xbc;
pub(crate) const SAVE_PGD_MAX_CRIMSON_FLASK_OFFSET: usize = 0xf9;
pub(crate) const SAVE_PGD_MAX_CERULEAN_FLASK_OFFSET: usize = 0xfa;
pub(crate) const SAVE_SPEFFECT_COUNT: usize = 0x0d;
pub(crate) const SAVE_SPEFFECT_SIZE: usize = 0x10;
pub(crate) const SAVE_CHR_ASM_EQUIPMENT_SIZE: usize = 0x58;
pub(crate) const SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE: usize = 0x1c;
pub(crate) const SAVE_INVENTORY_HELD_SIZE: usize = 0x9010;
pub(crate) const SAVE_EQUIP_MAGIC_SIZE: usize = 0x74;
pub(crate) const SAVE_EQUIP_ITEM_SIZE: usize = 0x8c;
pub(crate) const SAVE_GESTURE_EQUIP_SIZE: usize = 0x18;
pub(crate) const SAVE_PROJECTILE_ENTRY_SIZE: usize = 0x08;
pub(crate) const SAVE_PROJECTILE_COUNT_MAX: u32 = 0x400;
pub(crate) const SAVE_EQUIPPED_ARMAMENTS_AND_ITEMS_SIZE: usize = 0x9c;
pub(crate) const SAVE_PHYSIC_EQUIP_SIZE: usize = 0x0c;
pub(crate) const SAVE_FACE_DATA_FULL_SIZE: usize = 0x12f;
pub(crate) const SAVE_INVENTORY_STORAGE_SIZE: usize = 0x6010;
pub(crate) const SAVE_GESTURE_GAME_DATA_SIZE: usize = 0x100;
pub(crate) const SAVE_REGION_COUNT_MAX: u32 = 0x400;
pub(crate) const SAVE_REGION_ID_SIZE: usize = 0x04;
pub(crate) const SAVE_RIDE_GAME_DATA_SIZE: usize = 0x28;
pub(crate) const SAVE_CONTROL_BYTE_SIZE: usize = 0x01;
pub(crate) const SAVE_BLOODSTAIN_DATA_SIZE: usize = 0x44;
pub(crate) const SAVE_MENU_PROFILE_SAVE_LOAD_SIZE: usize = 0x1008;
pub(crate) const SAVE_TROPHY_EQUIP_DATA_SIZE: usize = 0x34;
pub(crate) const SAVE_GAITEM_GAME_DATA_SIZE: usize = 0x1b588;
pub(crate) const SAVE_TUTORIAL_DATA_SIZE: usize = 0x408;
pub(crate) const SAVE_GLOBAL_GAME_MAN_FLAGS_SIZE: usize = 0x03;
pub(crate) const SAVE_TOTAL_DEATHS_SIZE: usize = 0x04;
pub(crate) const SAVE_CHARACTER_TYPE_SIZE: usize = 0x04;
pub(crate) const SAVE_ONLINE_SESSION_FLAG_SIZE: usize = 0x01;
pub(crate) const SAVE_ONLINE_CHARACTER_TYPE_FLAG_SIZE: usize = 0x04;
pub(crate) const SAVE_LAST_RESTED_GRACE_SIZE: usize = 0x04;
pub(crate) const SAVE_NOT_ALONE_FLAG_SIZE: usize = 0x01;
pub(crate) const SAVE_INGAME_TIMER_PADDING_AFTER_NOT_ALONE: usize = 0x04;
pub(crate) const SAVE_INGAME_TIMER_TICKS_MAX: u32 = 999 * 60 * 60 / 10 + 59 * 60 / 10 + 59 / 10 + 1;
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
    /// The candidate bytes were written a SECOND time, after the game's return-title save finished
    /// (same-slot clobber fix; see `system_quit_save_swap_recommit_after_return_title_save`).
    pub(crate) recommitted: bool,
    pub(crate) summary_ptr: usize,
    pub(crate) summary_snapshot: Vec<u8>,
}

pub(crate) static SYSTEM_QUIT_SAVE_SWAP_STATE: OnceLock<Mutex<SystemQuitSaveSwapState>> =
    OnceLock::new();

/// True if ProfileSummary slot `slot` holds a real character (non-empty saved name). Used to gate the
/// human-driven in-world Load-Profile pick so activating an EMPTY slot never arms a switch (which
/// would tear the world down to a clean title and then fail the fresh deserialize, stranding the game
/// at a blank title). Reads the same save-record table the identity semaphore uses -- fault-guarded,
/// returns false on any unreadable pointer so an empty/unknown slot is treated as "no character".
pub(crate) unsafe fn profile_slot_has_character(slot: i32) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot) {
        return false;
    }
    let gdm = game_data_man_ptr_or_null();
    if gdm == null {
        return false;
    }
    let profile_summary =
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(null);
    if profile_summary == null {
        return false;
    }
    let rec = profile_summary_record_address(profile_summary, slot as usize);
    let (name, len) = unsafe { read_utf16_name_units(rec) };
    !utf16_name_empty_like(&name, len)
}

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
    *st = SystemQuitSaveSwapState {
        armed: true,
        path: path.to_owned(),
        original_bytes: bytes,
        original_hash: hash,
        original_len: len,
        original_modified_ns: modified_ns,
        ..SystemQuitSaveSwapState::default()
    };
    append_autoload_debug(format_args!(
        "system-quit-save-swap: armed active-save snapshot path='{path}' len={len} hash=0x{hash:016x}; replacement preview will restore this file unless a foreign slot is selected"
    ));
    true
}

pub(crate) unsafe fn system_quit_profile_summary_ptr() -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let gdm = game_data_man_ptr_or_null();
    if gdm == null {
        return null;
    }
    unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(null)
}

#[derive(Clone, Copy)]
pub(crate) struct SerializedSaveSlot<'a> {
    pub(crate) body: &'a [u8],
}

#[derive(Clone, Copy)]
pub(crate) struct SerializedPlayerGameData<'a> {
    pub(crate) body: &'a [u8],
    pub(crate) offset: usize,
}

impl<'a> SerializedSaveSlot<'a> {
    pub(crate) fn new(body: &'a [u8]) -> Self {
        Self { body }
    }

    /// The saved `BlockId` / map id this slot deserializes into `GameMan+0xc30`. Delegates to
    /// `er_save_loader::bnd4::slot_saved_map`, which is the function the host corpus test asserts
    /// against, so `saved_map()` IS the tested read rather than a parallel one that can drift.
    pub(crate) fn saved_map(self) -> Option<i32> {
        er_save_loader::bnd4::slot_saved_map(self.body)
    }

    fn read_u32(self, offset: usize) -> Option<u32> {
        self.body
            .get(offset..offset + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn add_offset(offset: &mut usize, len: usize) -> Option<()> {
        *offset = offset.checked_add(len)?;
        Some(())
    }

    fn add_counted_region(
        &self,
        offset: &mut usize,
        entry_size: usize,
        max_count: u32,
    ) -> Option<()> {
        let count = self.read_u32(*offset)?;
        if count > max_count {
            return None;
        }
        let bytes = (count as usize).checked_mul(entry_size)?.checked_add(4)?;
        Self::add_offset(offset, bytes)
    }

    /// Walk from the PGD start to the serialized ChrAsm sections. Section order (data-validated
    /// offline against the gold save, 2026-07-06: param ids read as armor/weapon param ranges,
    /// handles as 0x8xxxxxxx gaitem patterns):
    /// `[gaitem slot indices 0x58][ChrAsmEquipment 0x1c][equipment param ids 0x58][gaitem handles 0x58]`.
    fn walk_to_chr_asm_sections(self, pgd: SerializedPlayerGameData<'a>) -> Option<usize> {
        let mut offset = pgd.offset;
        Self::add_offset(&mut offset, SAVE_PLAYER_GAME_DATA_MIN_SIZE)?;
        Self::add_offset(&mut offset, SAVE_SPEFFECT_COUNT * SAVE_SPEFFECT_SIZE)?;
        Some(offset)
    }

    /// Continue past the ChrAsm + inventory/equip/gesture/projectile regions to the face section
    /// (`SAVE_FACE_DATA_FULL_SIZE` bytes; the `FACE` FaceDataBuffer sits a few bytes in).
    fn walk_to_face_section(self, pgd: SerializedPlayerGameData<'a>) -> Option<usize> {
        let mut offset = self.walk_to_chr_asm_sections(pgd)?;
        Self::add_offset(&mut offset, SAVE_CHR_ASM_EQUIPMENT_SIZE)?;
        Self::add_offset(&mut offset, SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE)?;
        Self::add_offset(&mut offset, SAVE_CHR_ASM_EQUIPMENT_SIZE)?;
        Self::add_offset(&mut offset, SAVE_CHR_ASM_EQUIPMENT_SIZE)?;
        Self::add_offset(&mut offset, SAVE_INVENTORY_HELD_SIZE)?;
        Self::add_offset(&mut offset, SAVE_EQUIP_MAGIC_SIZE)?;
        Self::add_offset(&mut offset, SAVE_EQUIP_ITEM_SIZE)?;
        Self::add_offset(&mut offset, SAVE_GESTURE_EQUIP_SIZE)?;
        self.add_counted_region(
            &mut offset,
            SAVE_PROJECTILE_ENTRY_SIZE,
            SAVE_PROJECTILE_COUNT_MAX,
        )?;
        Self::add_offset(&mut offset, SAVE_EQUIPPED_ARMAMENTS_AND_ITEMS_SIZE)?;
        Self::add_offset(&mut offset, SAVE_PHYSIC_EQUIP_SIZE)?;
        Some(offset)
    }

    /// The character's serialized `FaceDataBuffer` (starts at its `FACE` magic,
    /// `FACE_DATA_BUFFER_TOTAL_SIZE` bytes) -- the exact source `FaceData::CopyFromBuffer` expects.
    /// The face section has a small prefix before the magic (observed: 4 bytes of 0xff), so the magic
    /// is scanned within the section rather than assumed at +0. `None` when the walk or magic fails
    /// (caller keeps the fallback face and logs).
    pub(crate) fn face_data_buffer_bytes(
        self,
        pgd: SerializedPlayerGameData<'a>,
    ) -> Option<&'a [u8]> {
        let sect_off = self.walk_to_face_section(pgd)?;
        let sect = self
            .body
            .get(sect_off..sect_off + SAVE_FACE_DATA_FULL_SIZE)?;
        let rel = sect
            .windows(SAVE_FACE_MAGIC.len())
            .position(|w| w == SAVE_FACE_MAGIC)?;
        self.body
            .get(sect_off + rel..sect_off + rel + FACE_DATA_BUFFER_TOTAL_SIZE)
    }

    /// Assemble a RUNTIME `ChrAsm` image from the serialized sections, so the native ChrAsm copy
    /// receives the layout it expects: runtime is `[hdr 8][ChrAsmEquipment][gaitem_handles]
    /// [equipment_param_ids][tail]` while the save serializes `[slot indices][ChrAsmEquipment]
    /// [param ids][handles]` -- a raw copy of the save bytes dresses the portrait from garbage.
    ///
    /// THE THREE OVERRIDE SENTINELS MUST BE -1, NOT ZERO (bd er-effects-rs-wncc -- the real
    /// entirely-nude root cause). `unk0` (+0x00), `unkd4` (+0xd4) and `unkd8` (+0xd8) are not padding:
    /// the model-resource request `FUN_1409e6fb0` tests them SIGNED and treats a non-negative value in
    /// any of them as a forced whole-outfit override, so a zero-filled image resolves head/chest/hands/
    /// legs to param ids 0/100/200/300 -- rows that do not exist. Nothing renders, INCLUDING the
    /// bare-body defaults the native feed equips into hands and legs, which is exactly the reported
    /// "nude, missing even the default underwear". A ctor-built `ChrAsm` holds -1 here (deobf
    /// 0x1403be1d0 and 0x1403be208), which is why BOOT was unaffected: its record is copied from the
    /// ctor-initialised `PlayerGameData.equipGameData.chrAsm`. See `CHR_ASM_OVERRIDE_ABSENT`.
    /// `unk4` (+0x04) and the +0xdc..+0xe8 tail stay ZERO -- that is also what the ctor does.
    ///
    /// THE GAITEM HANDLE ARRAY IS LEFT ZERO ON PURPOSE. A gaitem handle only has meaning against the
    /// `gaitemInsTable` of the process that minted it; the FOREIGN save's serialized handles index a
    /// table this process never populated. Both consumers of this image (`CHR_ASM_COPY_RVA` into the
    /// ProfileSummary record, then the renderer's own `ChrAsm::Copy` inside the profile feed) run
    /// `GaitemHandle::copy` 22 times -- a REFCOUNTING assign -- so copying foreign handles would
    /// mutate live refcount state on entries this process owns.
    ///
    /// Dropping them costs nothing visually: the render path resolves armor from
    /// `equipment_param_ids` alone (`CS::ChrAsm::GetProtectorParamIdBySlot` at deobf 0x1403be950 is
    /// `mov 0x7c(%rcx,%rdx,4),%eax`, and `FUN_1409e6fb0` feeds that straight to
    /// `EquipParamProtector::GetEntry`). No gaitem handle is read anywhere on the render path.
    /// Handles matter only because `CS::ChrAsm::EquipItem` WRITES a param id from a handle lookup and
    /// stores -1 when the lookup fails -- i.e. a bad handle can only DESTROY a good param id.
    pub(crate) fn runtime_chr_asm_image(
        self,
        pgd: SerializedPlayerGameData<'a>,
    ) -> Option<[u8; CHR_ASM_SIZE]> {
        let mut off = self.walk_to_chr_asm_sections(pgd)?;
        off = off.checked_add(SAVE_CHR_ASM_EQUIPMENT_SIZE)?; // slot indices: no runtime home
        let equipment = self
            .body
            .get(off..off + SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE)?;
        off = off.checked_add(SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE)?;
        let param_ids = self.body.get(off..off + SAVE_CHR_ASM_EQUIPMENT_SIZE)?;
        off = off.checked_add(SAVE_CHR_ASM_EQUIPMENT_SIZE)?;
        // Bounds only: a save truncated before the handle section is not a whole ChrAsm and must
        // still fail the walk exactly as it did before. The bytes themselves are dropped.
        let _foreign_handles = self.body.get(off..off + SAVE_CHR_ASM_EQUIPMENT_SIZE)?;
        let mut image = [0u8; CHR_ASM_SIZE];
        for offset in [
            CHR_ASM_UNK0_OFFSET,
            CHR_ASM_UNKD4_OFFSET,
            CHR_ASM_UNKD8_OFFSET,
        ] {
            image[offset..offset + core::mem::size_of::<i32>()]
                .copy_from_slice(&CHR_ASM_OVERRIDE_ABSENT.to_le_bytes());
        }
        image[CHR_ASM_EQUIPMENT_OFFSET..CHR_ASM_EQUIPMENT_OFFSET + equipment.len()]
            .copy_from_slice(equipment);
        // `image[CHR_ASM_GAITEM_HANDLES_OFFSET..]` is left at its zero-init value: see the header.
        image[CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET
            ..CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET + param_ids.len()]
            .copy_from_slice(param_ids);
        Some(image)
    }

    pub(crate) fn in_game_timer_ticks(
        self,
        player_game_data: SerializedPlayerGameData<'a>,
    ) -> Option<u32> {
        let mut offset = self.walk_to_face_section(player_game_data)?;
        Self::add_offset(&mut offset, SAVE_FACE_DATA_FULL_SIZE)?;
        Self::add_offset(&mut offset, SAVE_INVENTORY_STORAGE_SIZE)?;
        Self::add_offset(&mut offset, SAVE_GESTURE_GAME_DATA_SIZE)?;
        self.add_counted_region(&mut offset, SAVE_REGION_ID_SIZE, SAVE_REGION_COUNT_MAX)?;
        Self::add_offset(&mut offset, SAVE_RIDE_GAME_DATA_SIZE)?;
        Self::add_offset(&mut offset, SAVE_CONTROL_BYTE_SIZE)?;
        Self::add_offset(&mut offset, SAVE_BLOODSTAIN_DATA_SIZE)?;
        Self::add_offset(&mut offset, 4)?;
        Self::add_offset(&mut offset, 4)?;
        Self::add_offset(&mut offset, SAVE_MENU_PROFILE_SAVE_LOAD_SIZE)?;
        Self::add_offset(&mut offset, SAVE_TROPHY_EQUIP_DATA_SIZE)?;
        Self::add_offset(&mut offset, SAVE_GAITEM_GAME_DATA_SIZE)?;
        Self::add_offset(&mut offset, SAVE_TUTORIAL_DATA_SIZE)?;
        Self::add_offset(&mut offset, SAVE_GLOBAL_GAME_MAN_FLAGS_SIZE)?;
        Self::add_offset(&mut offset, SAVE_TOTAL_DEATHS_SIZE)?;
        Self::add_offset(&mut offset, SAVE_CHARACTER_TYPE_SIZE)?;
        Self::add_offset(&mut offset, SAVE_ONLINE_SESSION_FLAG_SIZE)?;
        Self::add_offset(&mut offset, SAVE_ONLINE_CHARACTER_TYPE_FLAG_SIZE)?;
        Self::add_offset(&mut offset, SAVE_LAST_RESTED_GRACE_SIZE)?;
        Self::add_offset(&mut offset, SAVE_NOT_ALONE_FLAG_SIZE)?;
        Self::add_offset(&mut offset, SAVE_INGAME_TIMER_PADDING_AFTER_NOT_ALONE)?;
        let timer = self.read_u32(offset)?;
        (timer <= SAVE_INGAME_TIMER_TICKS_MAX).then_some(timer)
    }

    fn face_magic_offsets(self) -> impl Iterator<Item = usize> + 'a {
        self.body
            .windows(SAVE_FACE_MAGIC.len())
            .enumerate()
            .filter_map(|(offset, window)| (window == SAVE_FACE_MAGIC).then_some(offset))
            .take(SAVE_PGD_SCAN_LEADING_FACE_COUNT)
    }

    pub(crate) fn player_game_data(self) -> Option<SerializedPlayerGameData<'a>> {
        let mut best: Option<SerializedPlayerGameData<'a>> = None;
        let mut best_score = 0usize;
        for face_offset in self.face_magic_offsets() {
            let start = face_offset.saturating_sub(SAVE_PGD_FACE_DELTA_WINDOW_HIGH);
            let stop = face_offset.saturating_sub(SAVE_PGD_FACE_DELTA_WINDOW_LOW);
            for offset in start..=stop {
                let candidate = SerializedPlayerGameData {
                    body: self.body,
                    offset,
                };
                if !candidate.is_plausible_core() {
                    continue;
                }
                let score = candidate.score();
                if score > best_score {
                    best_score = score;
                    best = Some(candidate);
                }
            }
        }
        best
    }
}

/// True when any of the 10 save slots holds a readable character (a PlayerGameData block passing
/// the plausibility core: level/health/stat sanity). A no-save boot natively CREATES a full-size
/// EMPTY `ER0000.{sl2,co2}` container, which must not satisfy default-save discovery: observed
/// 2026-07-07, the game rewrote ER0000.sl2 during a pending missing-save-picker run, and the next
/// launch silently entered DEFAULT-USER-SAVE on that zero-character container instead of
/// re-arming the picker.
pub(crate) fn save_bytes_have_any_character(bytes: &[u8]) -> bool {
    (0..TITLE_PROFILE_SLOT_COUNT).any(|slot| {
        er_save_loader::bnd4::slot_body(bytes, slot)
            .ok()
            .and_then(|body| SerializedSaveSlot::new(body).player_game_data())
            .is_some()
    })
}

impl<'a> SerializedPlayerGameData<'a> {
    fn field(&self, offset: usize, len: usize) -> Option<&'a [u8]> {
        self.body
            .get(self.offset + offset..self.offset + offset + len)
    }

    fn read_u32(&self, offset: usize) -> Option<u32> {
        self.field(offset, 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_i32(&self, offset: usize) -> Option<i32> {
        self.field(offset, 4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_u8(&self, offset: usize) -> Option<u8> {
        self.field(offset, 1).map(|b| b[0])
    }

    fn name_bytes(&self) -> Option<&'a [u8]> {
        self.field(
            SAVE_PGD_CHARACTER_NAME_OFFSET,
            SAVE_PGD_CHARACTER_NAME_BYTES,
        )
    }

    fn name_units(&self) -> Option<Vec<u16>> {
        let bytes = self.name_bytes()?;
        Some(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|u| u16::from_le_bytes([u[0], u[1]]))
                .take_while(|u| *u != 0)
                .collect(),
        )
    }

    fn has_real_name(&self) -> bool {
        self.name_units().is_some_and(|units| {
            !units.is_empty()
                && units.iter().any(|u| *u != b'_' as u16)
                && String::from_utf16(&units)
                    .ok()
                    .is_some_and(|s| s.chars().all(|c| !c.is_control()))
        })
    }

    fn stats(&self) -> Option<[u32; SAVE_PGD_STAT_COUNT]> {
        let mut stats = [0u32; SAVE_PGD_STAT_COUNT];
        for (index, stat) in stats.iter_mut().enumerate() {
            *stat = self.read_u32(SAVE_PGD_STAT_BASE_OFFSET + index * 4)?;
        }
        Some(stats)
    }

    fn is_plausible_core(&self) -> bool {
        if self.offset + SAVE_PLAYER_GAME_DATA_MIN_SIZE > self.body.len() || !self.has_real_name() {
            return false;
        }
        let Some(level) = self.read_u32(SAVE_PGD_LEVEL_OFFSET) else {
            return false;
        };
        let Some(health) = self.read_u32(SAVE_PGD_HEALTH_OFFSET) else {
            return false;
        };
        let Some(max_health) = self.read_u32(SAVE_PGD_MAX_HEALTH_OFFSET) else {
            return false;
        };
        let Some(base_max_health) = self.read_u32(SAVE_PGD_BASE_MAX_HEALTH_OFFSET) else {
            return false;
        };
        let Some(gender) = self.read_u8(SAVE_PGD_GENDER_OFFSET) else {
            return false;
        };
        let Some(max_crimson) = self.read_u8(SAVE_PGD_MAX_CRIMSON_FLASK_OFFSET) else {
            return false;
        };
        let Some(max_cerulean) = self.read_u8(SAVE_PGD_MAX_CERULEAN_FLASK_OFFSET) else {
            return false;
        };
        let Some(stats) = self.stats() else {
            return false;
        };
        (1..=713).contains(&level)
            && (1..=100_000).contains(&health)
            && (1..=100_000).contains(&max_health)
            && (1..=100_000).contains(&base_max_health)
            && health <= max_health
            && base_max_health <= max_health
            && gender <= 1
            && max_crimson <= 14
            && max_cerulean <= 14
            && stats.iter().all(|stat| (1..=99).contains(stat))
    }

    fn score(&self) -> usize {
        self.name_units().map_or(0, |units| units.len())
            + self
                .stats()
                .map_or(0, |stats| stats.iter().filter(|stat| **stat > 0).count())
            + usize::from(self.read_u32(SAVE_PGD_LEVEL_OFFSET).unwrap_or(0) > 0)
    }

    pub(crate) fn stats_text_utf16(&self) -> Option<Vec<u16>> {
        const LABELS: [&str; SAVE_PGD_STAT_COUNT] =
            ["VIG", "MND", "END", "STR", "DEX", "INT", "FAI", "ARC"];
        let stats = self.stats()?;
        let mut s = String::new();
        for (i, label) in LABELS.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            s.push_str(label);
            s.push(' ');
            s.push_str(&stats[i].to_string());
        }
        Some(s.encode_utf16().chain(core::iter::once(0)).collect())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn write_profile_summary_record(
        &self,
        base: usize,
        profile_summary: usize,
        slot: usize,
        saved_map: i32,
        place_name_id: Option<u32>,
        playtime_ticks: u32,
        fallback_record: Option<&[u8]>,
        face_bytes: Option<&[u8]>,
        chr_asm_image: Option<&[u8; CHR_ASM_SIZE]>,
    ) -> bool {
        let Some(name_bytes) = self.name_bytes() else {
            return false;
        };
        let slot_data = profile_summary_record_address(profile_summary, slot);
        unsafe {
            if let Some(record) = fallback_record {
                core::ptr::copy_nonoverlapping(
                    record.as_ptr(),
                    slot_data as *mut u8,
                    PROFILE_SUMMARY_RECORD_STRIDE,
                );
            } else {
                core::ptr::write_bytes(slot_data as *mut u8, 0, PROFILE_SUMMARY_RECORD_STRIDE);
            }
            core::ptr::write_bytes(slot_data as *mut u8, 0, PROFILE_SUMMARY_NAME_BYTES);
            core::ptr::copy_nonoverlapping(
                name_bytes.as_ptr(),
                slot_data as *mut u8,
                name_bytes.len().min(PROFILE_SUMMARY_NAME_BYTES),
            );
            *(slot_data.wrapping_add(PROFILE_SUMMARY_LEVEL_OFFSET) as *mut i32) =
                self.read_i32(SAVE_PGD_LEVEL_OFFSET).unwrap_or(0);
            *(slot_data.wrapping_add(PROFILE_SUMMARY_PLAYTIME_OFFSET) as *mut u32) = playtime_ticks;
            *(slot_data.wrapping_add(PROFILE_SUMMARY_RUNE_MEMORY_OFFSET) as *mut i32) =
                self.read_i32(SAVE_PGD_RUNE_MEMORY_OFFSET).unwrap_or(0);
            *(slot_data.wrapping_add(PROFILE_SUMMARY_MAP_OFFSET) as *mut i32) = saved_map;
            // Location. Without this the row keeps whichever place name the PREVIOUS save left in
            // the record, so a swap updated the name, level, play time and stats while the location
            // stayed put -- user-reported 2026-08-07. `None` leaves the field alone rather than
            // writing a zero that would render as an empty Location, and records that the row must
            // not SHOW it: the value still sitting there is the template character's, and the field
            // is unrecoverable otherwise (it is in no character body and the game cannot recompute
            // it from a map), so the row hides it rather than printing somebody else's place.
            let slot_bit = 1usize << slot;
            if let Some(place_name_id) = place_name_id {
                *(slot_data.wrapping_add(PROFILE_SUMMARY_PLACE_NAME_OFFSET) as *mut u32) =
                    place_name_id;
                PROFILE_PREVIEW_PLACE_NAME_UNSOURCED.fetch_and(!slot_bit, Ordering::SeqCst);
            } else {
                PROFILE_PREVIEW_PLACE_NAME_UNSOURCED.fetch_or(slot_bit, Ordering::SeqCst);
            }
            // VISUAL IDENTITY (second-load wrong-head ROOT fix, user-identified 2026-07-06: "Banon in
            // all three windows"). The fallback record above is a STRUCTURAL template cloned from the
            // ORIGINAL save's first active slot -- its FaceData (+0x38) and ChrAsm (+0x1a8) describe
            // THAT character, so every foreign row's portrait rendered the original character while
            // the overwritten name/level kept the stats text correct. Fill the real visual blocks from
            // the FOREIGN character's save bytes through the game's own copy helpers: the section-walk
            // locators handle the save's variable-length layout (fixed runtime offsets false-negatived
            // on every slot, run portrait-faceid-switchqa-20260706-142552), and the saved FaceData
            // wrapper header does not match the live one, so CopyFromBuffer -- never a raw memcpy.
            if let (Some(face), Some(chr_asm_image)) = (face_bytes, chr_asm_image) {
                let copy_face_data_from_buffer: unsafe extern "system" fn(usize, usize) =
                    std::mem::transmute(base + FACE_DATA_COPY_FROM_BUFFER_RVA);
                let copy_chr_asm: unsafe extern "system" fn(usize, usize) -> usize =
                    std::mem::transmute(base + CHR_ASM_COPY_RVA);
                copy_face_data_from_buffer(
                    slot_data.wrapping_add(PROFILE_SUMMARY_FACE_DATA_OFFSET),
                    face.as_ptr() as usize,
                );
                copy_chr_asm(
                    slot_data.wrapping_add(PROFILE_SUMMARY_CHR_ASM_OFFSET),
                    chr_asm_image.as_ptr() as usize,
                );
                for (record_off, save_pgd_off) in [
                    (PROFILE_SUMMARY_GENDER_OFFSET, SAVE_PGD_GENDER_OFFSET),
                    (PROFILE_SUMMARY_ARCHETYPE_OFFSET, SAVE_PGD_ARCHETYPE_OFFSET),
                    (
                        PROFILE_SUMMARY_STARTING_GIFT_OFFSET,
                        SAVE_PGD_STARTING_GIFT_OFFSET,
                    ),
                    (PROFILE_SUMMARY_FIELD_C4_OFFSET, SAVE_PGD_FIELD_C4_OFFSET),
                ] {
                    if let Some(v) = self.read_u8(save_pgd_off) {
                        *(slot_data.wrapping_add(record_off) as *mut u8) = v;
                    }
                }
                PROFILE_PREVIEW_FACE_HASH[slot].store(
                    er_gfx::title_05_000::fnv1a64(face) as usize,
                    Ordering::SeqCst,
                );
            } else {
                PROFILE_PREVIEW_FACE_HASH[slot].store(0, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-load-save-profiles: slot {slot} FOREIGN visual data unavailable (face_located={} chr_asm_located={}); record keeps the fallback character's face/equipment",
                    face_bytes.is_some(),
                    chr_asm_image.is_some()
                ));
            }
            *(profile_summary.wrapping_add(PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + slot)
                as *mut u8) = 1;
        }
        true
    }
}

#[cfg(test)]
mod loading_cover_chr_asm_image_tests {
    use super::*;

    /// Distinctive synthetic section payloads -- no game bytes are read or versioned here. Handles
    /// carry the real `0x8xxxxxxx` gaitem shape so a stray copy of them is unmistakable in the image.
    const TEST_EQUIPMENT_FILL: u8 = 0xa5;
    const TEST_PARAM_ID_BASE: i32 = 0x0001_0000;
    const TEST_HANDLE_BASE: u32 = 0x8000_0000;
    const CHR_ASM_ENTRY_COUNT: usize = SAVE_CHR_ASM_EQUIPMENT_SIZE / core::mem::size_of::<i32>();

    fn param_id_at(index: usize) -> i32 {
        TEST_PARAM_ID_BASE + index as i32
    }

    /// A save body whose ChrAsm region holds the four serialized sections in save order:
    /// `[slot indices][ChrAsmEquipment][param ids][gaitem handles]`.
    fn synthetic_save_body() -> Vec<u8> {
        let prefix = SAVE_PLAYER_GAME_DATA_MIN_SIZE + SAVE_SPEFFECT_COUNT * SAVE_SPEFFECT_SIZE;
        let mut body = vec![0u8; prefix];
        body.resize(body.len() + SAVE_CHR_ASM_EQUIPMENT_SIZE, 0u8); // slot indices
        body.resize(
            body.len() + SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE,
            TEST_EQUIPMENT_FILL,
        );
        for index in 0..CHR_ASM_ENTRY_COUNT {
            body.extend(param_id_at(index).to_le_bytes());
        }
        for index in 0..CHR_ASM_ENTRY_COUNT {
            body.extend((TEST_HANDLE_BASE | index as u32).to_le_bytes());
        }
        body
    }

    fn image_from(body: &[u8]) -> Option<[u8; CHR_ASM_SIZE]> {
        SerializedSaveSlot::new(body)
            .runtime_chr_asm_image(SerializedPlayerGameData { body, offset: 0 })
    }

    fn image_i32_at(image: &[u8; CHR_ASM_SIZE], offset: usize) -> i32 {
        i32::from_le_bytes(
            image[offset..offset + core::mem::size_of::<i32>()]
                .try_into()
                .expect("4 bytes"),
        )
    }

    /// THE FIX (bd er-effects-rs-wncc). `FUN_1409e6fb0` tests these three SIGNED and treats a
    /// non-negative value as a forced whole-outfit override, so a zero here resolves the four
    /// protector slots to 0/100/200/300 and the portrait renders entirely nude -- default underwear
    /// included. A ctor-built `ChrAsm` holds -1; our hand-built image must too.
    #[test]
    fn the_three_whole_outfit_override_sentinels_are_minus_one_not_zero() {
        let body = synthetic_save_body();
        let image = image_from(&body).expect("synthetic body walks to the ChrAsm sections");
        for (name, offset) in [
            ("unk0", CHR_ASM_UNK0_OFFSET),
            ("unkd4", CHR_ASM_UNKD4_OFFSET),
            ("unkd8", CHR_ASM_UNKD8_OFFSET),
        ] {
            assert_eq!(
                image_i32_at(&image, offset),
                CHR_ASM_OVERRIDE_ABSENT,
                "{name} (+{offset:#x}) must be the no-override sentinel"
            );
        }
    }

    /// The ctor writes `unk4` and the +0xdc..+0xe8 tail as ZERO, so the image must leave them zero --
    /// matching the ctor exactly, no wider.
    #[test]
    fn only_those_three_fields_are_seeded_the_rest_of_the_header_and_tail_stay_zero() {
        let body = synthetic_save_body();
        let image = image_from(&body).expect("synthetic body walks to the ChrAsm sections");
        let unk4 = CHR_ASM_UNK0_OFFSET + core::mem::size_of::<i32>();
        assert_eq!(image_i32_at(&image, unk4), 0, "unk4 must stay zero");
        let tail = CHR_ASM_UNKD8_OFFSET + core::mem::size_of::<i32>();
        assert!(
            image[tail..].iter().all(|byte| *byte == 0),
            "the +0xdc tail must stay zero, got {:02x?}",
            &image[tail..]
        );
    }

    /// The sentinel offsets must sit exactly one param-id array past the last public field and must
    /// stay inside the struct -- `unkd4`/`unkd8` are private in `fromsoftware-rs`, so this is what
    /// stands in for `offset_of!` if the typed layout ever moves.
    #[test]
    fn the_sentinel_offsets_agree_with_the_typed_chr_asm_layout() {
        assert_eq!(CHR_ASM_UNK0_OFFSET, 0);
        assert_eq!(CHR_ASM_UNKD4_OFFSET, 0xd4);
        assert_eq!(CHR_ASM_UNKD8_OFFSET, 0xd8);
        assert_eq!(
            CHR_ASM_EQUIPMENT_ENTRY_COUNT,
            SAVE_CHR_ASM_EQUIPMENT_SIZE / core::mem::size_of::<i32>(),
            "the save section and the runtime array must hold the same number of entries"
        );
        assert!(CHR_ASM_UNKD8_OFFSET + core::mem::size_of::<i32>() <= CHR_ASM_SIZE);
    }

    /// A FOREIGN save's gaitem handles index a `gaitemInsTable` this process never populated, so they
    /// must never reach the refcounting `ChrAsm::Copy` -- not for the ProfileSummary record and not
    /// for the renderer. Costless visually: the render path never reads a handle.
    #[test]
    fn the_foreign_saves_gaitem_handles_are_never_copied_into_the_runtime_image() {
        let body = synthetic_save_body();
        let image = image_from(&body).expect("synthetic body walks to the ChrAsm sections");
        let handles = &image[CHR_ASM_GAITEM_HANDLES_OFFSET
            ..CHR_ASM_GAITEM_HANDLES_OFFSET + SAVE_CHR_ASM_EQUIPMENT_SIZE];
        assert!(
            handles.iter().all(|byte| *byte == 0),
            "gaitem handle array must be zeroed, got {handles:02x?}"
        );
    }

    /// The param ids are the ONLY armor source the render path reads, so zeroing the handles must not
    /// take them with it.
    #[test]
    fn the_equipment_param_ids_survive_verbatim() {
        let body = synthetic_save_body();
        let image = image_from(&body).expect("synthetic body walks to the ChrAsm sections");
        for index in 0..CHR_ASM_ENTRY_COUNT {
            let at = CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET + index * core::mem::size_of::<i32>();
            let got = i32::from_le_bytes(image[at..at + 4].try_into().expect("4 bytes"));
            assert_eq!(got, param_id_at(index), "param id {index} drifted");
        }
    }

    #[test]
    fn the_chr_asm_equipment_block_still_lands_at_its_runtime_offset() {
        let body = synthetic_save_body();
        let image = image_from(&body).expect("synthetic body walks to the ChrAsm sections");
        let equipment = &image[CHR_ASM_EQUIPMENT_OFFSET
            ..CHR_ASM_EQUIPMENT_OFFSET + SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE];
        assert!(equipment.iter().all(|byte| *byte == TEST_EQUIPMENT_FILL));
    }

    /// Dropping the handle bytes must NOT drop the bounds check they carried: a body truncated
    /// inside the handle section is not a whole serialized ChrAsm and must still be rejected.
    #[test]
    fn a_body_truncated_inside_the_handle_section_is_still_rejected() {
        let mut body = synthetic_save_body();
        body.truncate(body.len() - 1);
        assert!(image_from(&body).is_none());
    }
}
