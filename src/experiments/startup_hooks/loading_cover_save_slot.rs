
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
                .map(|m| valid(m))
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
/// per-frame render drive + pixel oracle then re-engage on the loading screen automatically (they
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
    // If the table is already populated (menu built it, or our own build already ran), leave it -- the
    // existing mark+refresh feed + draw + oracle drive it. A live table also RE-ARMS the latch:
    // a subsequent Continue teardown empties it again and we rebuild our own for that load window.
    let t0 = unsafe { safe_read_usize(portrait_renderer_table_entry(base, 0)) }.unwrap_or(0);
    let populated = t0 != 0
        && t0 != null
        && unsafe { safe_read_usize(t0) }.unwrap_or(0)
            == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA;
    if populated {
        PROFILE_TABLE_EMPTY_STREAK.store(0, Ordering::SeqCst);
        PROFILE_TABLE_WAS_POPULATED.store(1, Ordering::SeqCst);
        if PROFILE_LOADSCREEN_TABLE_OWNED.load(Ordering::SeqCst) == 0 {
            PROFILE_LOADSCREEN_REBUILT.store(0, Ordering::SeqCst);
        }
        return;
    }
    if profile_select_window_open {
        append_autoload_debug(format_args!(
            "loading-portrait: ProfileSelect owner flag still set, but renderer table is empty -- treating as Continue teardown and building loading-owned renderer now"
        ));
    }
    // Table is EMPTY this tick -- count the streak. The menu's own teardown+rebuild is synchronous, so a
    // sustained-empty table across ticks means the Continue teardown ran with no menu rebuild (we've left
    // the menu into the load), which happens ~17s -- well before the now-loading flag flips (~21s on the
    // fast gold-save load). Build as soon as EITHER signal fires so ResMan has time to build the model.
    let streak = PROFILE_TABLE_EMPTY_STREAK.fetch_add(1, Ordering::SeqCst) + 1;
    if PROFILE_LOADSCREEN_REBUILT.load(Ordering::SeqCst) != 0 {
        return; // already built our table for this load window
    }
    // HARD SAFETY: never call the builder until the menu has built a table at least once. At the title
    // screen the table is empty too, but the engine/ResMan are not up and the builder access-violates.
    if PROFILE_TABLE_WAS_POPULATED.load(Ordering::SeqCst) == 0 {
        return;
    }
    let nowload = unsafe { now_loading_active(base) };
    if !(nowload || profile_select_window_open || streak >= PROFILE_TABLE_EMPTY_STREAK_BUILD_THRESHOLD) {
        return;
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
    append_autoload_debug(format_args!(
        "loading-portrait: empty profile table (trigger={} streak={streak}) -> called builder 0x{:x} to build our own renderers for the post-Continue portrait",
        if nowload {
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
    let b290 = unsafe { safe_read_u8(record + 0x290) }.unwrap_or(0);
    let b294 = unsafe { safe_read_u8(record + 0x294) }.unwrap_or(0);
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
        set_model_source(renderer, record + 0x1a8);
        let fd = facedata_buffer(record + 0x38, 1);
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
    PORTRAIT_KICK_SLOT_KEY.store((slot + 1) as usize, Ordering::SeqCst);
    PORTRAIT_KICK_RENDERER.store(renderer, Ordering::SeqCst);
    let kicks = PROFILE_TARGET_KICKS.fetch_add(1, Ordering::SeqCst) + 1;
    if kicks <= 4 {
        append_autoload_debug(format_args!(
            "loading-portrait: per-slot build kick #{kicks} for LOADED slot {slot} (renderer=0x{renderer:x} record=0x{record:x}) -- global refresh not called, other slots stay unbuilt"
        ));
    }
    true
}

/// The save slot whose portrait the loading-screen pipeline should build / capture / display / spare:
/// the character the game ACTUALLY loaded (`GameMan.save_slot` = ac0), the single ground truth on a
/// boot most-recent Continue AND on our switch deserialize. Falls back to the autoload hint
/// `OWN_STEPPER_SLOT`, then 0, only pre-load when ac0 is not yet a valid slot. The raw
/// `OWN_STEPPER_SLOT` is `-1` on a most-recent boot (title.rs:113 returns early without setting it) and
/// collapsed to slot 0, so the pipeline built/captured slot 0's portrait for a non-slot-0 character
/// (wrong on load 1) and captured nothing once its gate stopped matching (blank on load 2). Routing
/// EVERY portrait site through this one loaded-character source is the er-effects-rs-j3r correlation fix.
pub(crate) fn portrait_loaded_slot() -> i32 {
    portrait_loaded_slot_confirmed().unwrap_or(0)
}

/// The loaded slot ONLY when a real source names it (ac0 or the autoload stepper hint) -- `None`
/// while neither is valid yet. The BUILD KICK must use this form: the old fallback-to-0 kicked a
/// SLOT-0 build ~340ms before ac0 flipped to the real slot (run anim-bind5, kicks #1 slot0 /
/// #2 slot5), and with the rebuild storm fixed that foreign model now PERSISTS -- the
/// `count_live_profile_models == 1` stability gate then blocks the whole live-drive/publish/anim
/// pipeline for the rest of the load (1 motion sample all window). Display-side readers may still
/// use the collapsed `portrait_loaded_slot()` form (with no model built, a wrong slot reads inert).
pub(crate) fn portrait_loaded_slot_confirmed() -> Option<i32> {
    let ac0 = (unsafe { eldenring::cs::GameMan::instance() })
        .map(|gm| er_save_loader::GameManSaveAccess::save_slot(gm))
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&ac0) {
        return Some(ac0);
    }
    let own = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&own) {
        return Some(own);
    }
    None
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
    if n == 0 { 0 } else { (sum / n) as usize }
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
unsafe fn portrait_render_slot_semaphore(base: usize, render_target_slot: i32) {
    // New-game / not-yet-resolved saved-map sentinel; excluded from the map check so a transient c30
    // during the loading screen cannot false-fire.
    const DEFAULT_MAP_C30: i32 = 0x0a01_0000;
    // ProfileSummary record layout (bd native-full-save-read-slot-resolve-chain-observe-recipe-2026):
    // records start at summary+0x18, stride 0x2a0; NAME at record+0, saved MAP at record+0x30.
    const PROFILE_RECORD_BASE: usize = 0x18;
    const PROFILE_RECORD_STRIDE: usize = 0x2a0;
    const PROFILE_RECORD_MAP_OFFSET: usize = 0x30;
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
    let rec =
        profile_summary + PROFILE_RECORD_BASE + render_target_slot as usize * PROFILE_RECORD_STRIDE;
    let (our_name, our_len) = unsafe { read_utf16_name_units(rec) };
    if utf16_name_empty_like(&our_name, our_len) {
        return; // our target slot stores no real character -- nothing meaningful to compare.
    }
    let our_map = unsafe { safe_read_i32(rec + PROFILE_RECORD_MAP_OFFSET) }.unwrap_or(-1);

    // Compare RAM identities. Name is the character identity; the saved map is a second discriminator,
    // checked only when BOTH are real resolved maps (so a default/transient c30 can't false-fire).
    let name_match = our_len == live_len && our_name[..our_len] == live_name[..live_len];
    let both_real_map =
        our_map > 0 && our_map != DEFAULT_MAP_C30 && live_map > 0 && live_map != DEFAULT_MAP_C30;
    let map_mismatch = both_real_map && our_map != live_map;
    if name_match && !map_mismatch {
        return; // our portrait's character == the loaded character (RAM identity match).
    }
    let cond = ((!name_match) as usize) | ((map_mismatch as usize) << 1);
    PORTRAIT_RENDER_SEMAPHORE_STATE.store(
        ((render_target_slot as u32 as usize) << 16)
            | ((our_map as u32 as usize & 0xff) << 8)
            | cond,
        Ordering::SeqCst,
    );
    if PORTRAIT_RENDER_SEMAPHORE_LOGGED.swap(1, Ordering::SeqCst) == 0 {
        append_crash_log(format_args!(
            "PORTRAIT-IDENTITY-SEMAPHORE FAIL: our portrait targets slot={render_target_slot} (record name_len={our_len} map=0x{our_map:x}) but the LOADED character is name_len={live_len} map=0x{live_map:x} -- name_match={name_match} map_mismatch={map_mismatch}. Our portrait is not the loaded character (er-effects-rs-j3r); deliberate fault only if ER_EFFECTS_FAIL_FAST=1"
        ));
        append_autoload_debug(format_args!(
            "PORTRAIT-IDENTITY-SEMAPHORE FAIL: target_slot={render_target_slot} record(name_len={our_len} map=0x{our_map:x}) vs loaded(name_len={live_len} map=0x{live_map:x}) name_match={name_match} map_mismatch={map_mismatch}"
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

/// ProfileSummary save-record layout (bd native-full-save-read-slot-resolve-chain-observe-recipe):
/// per-slot records start at `summary+0x18`, stride `0x2a0`; character NAME at record+0.
const PROFILE_SUMMARY_RECORD_BASE: usize = 0x18;
const PROFILE_SUMMARY_RECORD_STRIDE: usize = 0x2a0;
const PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET: usize = 0x8;
const PROFILE_SUMMARY_TOTAL_BYTES: usize =
    PROFILE_SUMMARY_RECORD_BASE + PROFILE_SUMMARY_RECORD_STRIDE * TITLE_PROFILE_SLOT_COUNT;
const PROFILE_SUMMARY_NAME_BYTES: usize = 0x22;
const PROFILE_SUMMARY_LEVEL_OFFSET: usize = 0x24;
const PROFILE_SUMMARY_PLAYTIME_OFFSET: usize = 0x28;
const PROFILE_SUMMARY_RUNE_MEMORY_OFFSET: usize = 0x2c;
const PROFILE_SUMMARY_MAP_OFFSET: usize = 0x30;
const PROFILE_SUMMARY_FACE_DATA_OFFSET: usize = 0x38;
const PROFILE_SUMMARY_CHR_ASM_OFFSET: usize = 0x1a8;
const PROFILE_SUMMARY_GENDER_OFFSET: usize = 0x290;
const PROFILE_SUMMARY_ARCHETYPE_OFFSET: usize = 0x291;
const PROFILE_SUMMARY_STARTING_GIFT_OFFSET: usize = 0x292;
const PROFILE_SUMMARY_FIELD_C4_OFFSET: usize = 0x293;
const SAVE_SLOT_MAP_OFFSET: usize = 0x14;
const SAVE_FACE_MAGIC: &[u8; 4] = b"FACE";
const SAVE_FACE_DATA_BUFFER_SIZE: usize = 0x120;
const SAVE_PGD_SCAN_LEADING_FACE_COUNT: usize = 4;
const SAVE_PGD_FACE_DELTA_WINDOW_LOW: usize = 0xa000;
const SAVE_PGD_FACE_DELTA_WINDOW_HIGH: usize = 0xa600;
const SAVE_PLAYER_GAME_DATA_MIN_SIZE: usize = 0x1b0;
const SAVE_PGD_HEALTH_OFFSET: usize = 0x08;
const SAVE_PGD_MAX_HEALTH_OFFSET: usize = 0x0c;
const SAVE_PGD_BASE_MAX_HEALTH_OFFSET: usize = 0x10;
const SAVE_PGD_STAT_BASE_OFFSET: usize = 0x34;
const SAVE_PGD_STAT_COUNT: usize = 8;
const SAVE_PGD_LEVEL_OFFSET: usize = 0x60;
const SAVE_PGD_RUNE_MEMORY_OFFSET: usize = 0x68;
const SAVE_PGD_CHARACTER_NAME_OFFSET: usize = 0x94;
const SAVE_PGD_CHARACTER_NAME_UNITS: usize = 0x10;
const SAVE_PGD_CHARACTER_NAME_BYTES: usize = SAVE_PGD_CHARACTER_NAME_UNITS * 2;
const SAVE_PGD_GENDER_OFFSET: usize = 0xb6;
const SAVE_PGD_MAX_CRIMSON_FLASK_OFFSET: usize = 0xf9;
const SAVE_PGD_MAX_CERULEAN_FLASK_OFFSET: usize = 0xfa;
const SAVE_SPEFFECT_COUNT: usize = 0x0d;
const SAVE_SPEFFECT_SIZE: usize = 0x10;
const SAVE_CHR_ASM_EQUIPMENT_SIZE: usize = 0x58;
const SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE: usize = 0x1c;
const SAVE_INVENTORY_HELD_SIZE: usize = 0x9010;
const SAVE_EQUIP_MAGIC_SIZE: usize = 0x74;
const SAVE_EQUIP_ITEM_SIZE: usize = 0x8c;
const SAVE_GESTURE_EQUIP_SIZE: usize = 0x18;
const SAVE_PROJECTILE_ENTRY_SIZE: usize = 0x08;
const SAVE_PROJECTILE_COUNT_MAX: u32 = 0x400;
const SAVE_EQUIPPED_ARMAMENTS_AND_ITEMS_SIZE: usize = 0x9c;
const SAVE_PHYSIC_EQUIP_SIZE: usize = 0x0c;
const SAVE_FACE_DATA_FULL_SIZE: usize = 0x12f;
const SAVE_INVENTORY_STORAGE_SIZE: usize = 0x6010;
const SAVE_GESTURE_GAME_DATA_SIZE: usize = 0x100;
const SAVE_REGION_COUNT_MAX: u32 = 0x400;
const SAVE_REGION_ID_SIZE: usize = 0x04;
const SAVE_RIDE_GAME_DATA_SIZE: usize = 0x28;
const SAVE_CONTROL_BYTE_SIZE: usize = 0x01;
const SAVE_BLOODSTAIN_DATA_SIZE: usize = 0x44;
const SAVE_MENU_PROFILE_SAVE_LOAD_SIZE: usize = 0x1008;
const SAVE_TROPHY_EQUIP_DATA_SIZE: usize = 0x34;
const SAVE_GAITEM_GAME_DATA_SIZE: usize = 0x1b588;
const SAVE_TUTORIAL_DATA_SIZE: usize = 0x408;
const SAVE_GLOBAL_GAME_MAN_FLAGS_SIZE: usize = 0x03;
const SAVE_TOTAL_DEATHS_SIZE: usize = 0x04;
const SAVE_CHARACTER_TYPE_SIZE: usize = 0x04;
const SAVE_ONLINE_SESSION_FLAG_SIZE: usize = 0x01;
const SAVE_ONLINE_CHARACTER_TYPE_FLAG_SIZE: usize = 0x04;
const SAVE_LAST_RESTED_GRACE_SIZE: usize = 0x04;
const SAVE_NOT_ALONE_FLAG_SIZE: usize = 0x01;
const SAVE_INGAME_TIMER_PADDING_AFTER_NOT_ALONE: usize = 0x04;
const SAVE_INGAME_TIMER_TICKS_MAX: u32 = 999 * 60 * 60 / 10 + 59 * 60 / 10 + 59 / 10 + 1;
const SYSTEM_QUIT_SAVE_SWAP_POLL_INTERVAL_TICKS: usize = 30;
static SYSTEM_QUIT_SAVE_SWAP_POLL_TICK: AtomicUsize = AtomicUsize::new(0);
static PROFILE_STATS_PREVIEW_ROW_CURSOR: AtomicUsize = AtomicUsize::new(0);

#[derive(Default)]
struct SystemQuitSaveSwapState {
    armed: bool,
    path: String,
    original_bytes: Vec<u8>,
    original_hash: u64,
    original_len: u64,
    original_modified_ns: u128,
    candidate_bytes: Vec<u8>,
    candidate_hash: u64,
    candidate_slot_mask: usize,
    candidate_stats_utf16: Vec<Vec<u16>>,
    preview_applied: bool,
    committed: bool,
    summary_ptr: usize,
    summary_snapshot: Vec<u8>,
}

static SYSTEM_QUIT_SAVE_SWAP_STATE: OnceLock<Mutex<SystemQuitSaveSwapState>> = OnceLock::new();

/// True if ProfileSummary slot `slot` holds a real character (non-empty saved name). Used to gate the
/// human-driven in-world Load-Profile pick so activating an EMPTY slot never arms a switch (which
/// would tear the world down to a clean title and then fail the fresh deserialize, stranding the game
/// at a blank title). Reads the same save-record table the identity semaphore uses -- fault-guarded,
/// returns false on any unreadable pointer so an empty/unknown slot is treated as "no character".
unsafe fn profile_slot_has_character(slot: i32) -> bool {
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
    let rec = profile_summary
        + PROFILE_SUMMARY_RECORD_BASE
        + slot as usize * PROFILE_SUMMARY_RECORD_STRIDE;
    let (name, len) = unsafe { read_utf16_name_units(rec) };
    !utf16_name_empty_like(&name, len)
}

fn system_quit_save_swap_state() -> &'static Mutex<SystemQuitSaveSwapState> {
    SYSTEM_QUIT_SAVE_SWAP_STATE.get_or_init(|| Mutex::new(SystemQuitSaveSwapState::default()))
}

fn system_quit_save_swap_lock() -> std::sync::MutexGuard<'static, SystemQuitSaveSwapState> {
    system_quit_save_swap_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn system_quit_hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in bytes.iter().copied() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn system_quit_file_stamp(path: &str) -> Option<(u64, u128)> {
    let meta = fs::metadata(path).ok()?;
    let modified_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some((meta.len(), modified_ns))
}

fn system_quit_save_swap_arm_original(path: &str) -> bool {
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

unsafe fn system_quit_profile_summary_ptr() -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let gdm = game_data_man_ptr_or_null();
    if gdm == null {
        return null;
    }
    unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(null)
}

#[derive(Clone, Copy)]
struct SerializedSaveSlot<'a> {
    body: &'a [u8],
}

#[derive(Clone, Copy)]
struct SerializedPlayerGameData<'a> {
    body: &'a [u8],
    offset: usize,
}

impl<'a> SerializedSaveSlot<'a> {
    fn new(body: &'a [u8]) -> Self {
        Self { body }
    }

    fn saved_map(self) -> Option<i32> {
        self.read_i32(SAVE_SLOT_MAP_OFFSET)
    }

    fn read_u32(self, offset: usize) -> Option<u32> {
        self.body
            .get(offset..offset + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_i32(self, offset: usize) -> Option<i32> {
        self.body
            .get(offset..offset + 4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
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

    fn in_game_timer_ticks(self, player_game_data: SerializedPlayerGameData<'a>) -> Option<u32> {
        let mut offset = player_game_data.offset;
        Self::add_offset(&mut offset, SAVE_PLAYER_GAME_DATA_MIN_SIZE)?;
        Self::add_offset(&mut offset, SAVE_SPEFFECT_COUNT * SAVE_SPEFFECT_SIZE)?;
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

    fn player_game_data(self) -> Option<SerializedPlayerGameData<'a>> {
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
                .chunks_exact(2)
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

    fn stats_text_utf16(&self) -> Option<Vec<u16>> {
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

    unsafe fn write_profile_summary_record(
        &self,
        profile_summary: usize,
        slot: usize,
        saved_map: i32,
        playtime_ticks: u32,
        fallback_record: Option<&[u8]>,
    ) -> bool {
        let Some(name_bytes) = self.name_bytes() else {
            return false;
        };
        let slot_data =
            profile_summary + PROFILE_SUMMARY_RECORD_BASE + slot * PROFILE_SUMMARY_RECORD_STRIDE;
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
            *(profile_summary.wrapping_add(PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + slot)
                as *mut u8) = 1;
        }
        true
    }
}
