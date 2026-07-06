
unsafe fn system_quit_apply_foreign_profile_summary_preview(base: usize, bytes: &[u8]) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let summary = unsafe { system_quit_profile_summary_ptr() };
    if summary == null {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: cannot preview replacement save -- live ProfileSummary unavailable"
        ));
        return 0;
    }
    let mut st = system_quit_save_swap_lock();
    if st.summary_snapshot.is_empty() || st.summary_ptr != summary {
        st.summary_ptr = summary;
        st.summary_snapshot = unsafe {
            core::slice::from_raw_parts(summary as *const u8, PROFILE_SUMMARY_TOTAL_BYTES).to_vec()
        };
    }
    let summary_snapshot = st.summary_snapshot.clone();
    let fallback_slot = (0..TITLE_PROFILE_SLOT_COUNT).find(|slot| {
        summary_snapshot
            .get(PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + *slot)
            .copied()
            .unwrap_or(0)
            != 0
    });
    unsafe {
        for slot in 0..TITLE_PROFILE_SLOT_COUNT {
            core::ptr::write_bytes(
                (summary + PROFILE_SUMMARY_RECORD_BASE + slot * PROFILE_SUMMARY_RECORD_STRIDE)
                    as *mut u8,
                0,
                PROFILE_SUMMARY_RECORD_STRIDE,
            );
            *((summary + PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + slot) as *mut u8) = 0;
            PROFILE_PREVIEW_FACE_HASH[slot].store(0, Ordering::SeqCst);
        }
    }
    drop(st);

    let mut mask = 0usize;
    let mut preview_stats = vec![Vec::new(); TITLE_PROFILE_SLOT_COUNT];
    for slot in 0..TITLE_PROFILE_SLOT_COUNT {
        if let Ok(body) = er_save_loader::bnd4::slot_body(bytes, slot) {
            let slot_body = SerializedSaveSlot::new(body);
            let Some(pgd) = slot_body.player_game_data() else {
                continue;
            };
            let Some(saved_map) = slot_body.saved_map() else {
                continue;
            };
            let fallback_src_slot = if summary_snapshot
                .get(PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + slot)
                .copied()
                .unwrap_or(0)
                != 0
            {
                Some(slot)
            } else {
                fallback_slot
            };
            let fallback = fallback_src_slot.and_then(|src_slot| {
                let start = PROFILE_SUMMARY_RECORD_BASE + src_slot * PROFILE_SUMMARY_RECORD_STRIDE;
                summary_snapshot.get(start..start + PROFILE_SUMMARY_RECORD_STRIDE)
            });
            let playtime_ticks = slot_body.in_game_timer_ticks(pgd).unwrap_or(0);
            let face_bytes = slot_body.face_data_buffer_bytes(pgd);
            let chr_asm_image = slot_body.runtime_chr_asm_image(pgd);
            if unsafe {
                pgd.write_profile_summary_record(
                    base,
                    summary,
                    slot,
                    saved_map,
                    playtime_ticks,
                    fallback,
                    face_bytes,
                    chr_asm_image.as_ref(),
                )
            } {
                append_autoload_debug(format_args!(
                    "system-quit-load-save-profiles: preview slot {slot} playtime_ticks={playtime_ticks}"
                ));
                if let Some(stats) = pgd.stats_text_utf16() {
                    preview_stats[slot] = stats;
                }
                mask |= 1usize << slot;
            }
        }
    }
    if mask != 0 {
        {
            let mut st = system_quit_save_swap_lock();
            st.candidate_slot_mask = mask;
            st.candidate_stats_utf16 = preview_stats;
            st.preview_applied = true;
        }
        PROFILE_STATS_PREVIEW_ROW_CURSOR.store(0, Ordering::SeqCst);
        let refresh: unsafe extern "system" fn() =
            unsafe { std::mem::transmute(base + PROFILE_RENDERER_REFRESH_RVA) };
        unsafe { refresh() };
    }
    mask
}

fn system_quit_save_swap_restore_original_file(st: &SystemQuitSaveSwapState, reason: &str) -> bool {
    if st.path.is_empty() || st.original_bytes.is_empty() {
        return false;
    }
    match fs::write(&st.path, &st.original_bytes) {
        Ok(()) => {
            append_autoload_debug(format_args!(
                "system-quit-save-swap: restored active save file for {reason} path='{}' len={} hash=0x{:016x}",
                st.path,
                st.original_bytes.len(),
                st.original_hash
            ));
            true
        }
        Err(err) => {
            append_autoload_debug(format_args!(
                "system-quit-save-swap: FAILED to restore active save file for {reason} path='{}': {err}",
                st.path
            ));
            false
        }
    }
}

unsafe fn system_quit_save_swap_restore_profile_summary(reason: &str) {
    let mut st = system_quit_save_swap_lock();
    if !st.preview_applied || st.committed {
        return;
    }
    if st.summary_ptr >= 0x10000 && !st.summary_snapshot.is_empty() {
        unsafe {
            core::ptr::copy_nonoverlapping(
                st.summary_snapshot.as_ptr(),
                st.summary_ptr as *mut u8,
                st.summary_snapshot.len(),
            );
        }
        if let Ok(base) = game_module_base() {
            let refresh: unsafe extern "system" fn() =
                unsafe { std::mem::transmute(base + PROFILE_RENDERER_REFRESH_RVA) };
            unsafe { refresh() };
        }
        append_autoload_debug(format_args!(
            "system-quit-save-swap: restored live ProfileSummary snapshot for {reason} summary=0x{:x} bytes={}",
            st.summary_ptr,
            st.summary_snapshot.len()
        ));
    }
    // The restored snapshot's records are the ORIGINAL save's characters -- the foreign preview face
    // fingerprints no longer describe any slot.
    for slot in 0..TITLE_PROFILE_SLOT_COUNT {
        PROFILE_PREVIEW_FACE_HASH[slot].store(0, Ordering::SeqCst);
    }
    let _ = system_quit_save_swap_restore_original_file(&st, reason);
    *st = SystemQuitSaveSwapState::default();
}

unsafe fn system_quit_save_swap_poll_preview(base: usize) {
    let tick = SYSTEM_QUIT_SAVE_SWAP_POLL_TICK.fetch_add(1, Ordering::SeqCst);
    if tick % SYSTEM_QUIT_SAVE_SWAP_POLL_INTERVAL_TICKS != 0 {
        return;
    }
    let (path, original_hash, original_len, original_modified_ns, preview_applied) = {
        let st = system_quit_save_swap_lock();
        if !st.armed || st.committed || st.path.is_empty() {
            return;
        }
        (
            st.path.clone(),
            st.original_hash,
            st.original_len,
            st.original_modified_ns,
            st.preview_applied,
        )
    };
    if preview_applied {
        return;
    }
    let Some((len, modified_ns)) = system_quit_file_stamp(&path) else {
        return;
    };
    if len == original_len && modified_ns == original_modified_ns {
        return;
    }
    let Ok(mut bytes) = fs::read(&path) else {
        return;
    };
    let raw_hash = system_quit_hash_bytes(&bytes);
    if raw_hash == original_hash {
        return;
    }
    // Validate before restoring the active redirected save. A partial copy must not be captured as a
    // foreign preview, and the old in-world save must remain the write target until the user commits.
    if er_save_loader::bnd4::parse_entries(&bytes).is_err() {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: replacement candidate changed but is not a valid BND4 yet path='{path}' len={len} hash=0x{raw_hash:016x}; waiting"
        ));
        return;
    }
    normalize_save_bytes_to_active_steam_id(base, &mut bytes, "system-quit-polled-candidate");
    let hash = system_quit_hash_bytes(&bytes);
    {
        let st = system_quit_save_swap_lock();
        if !system_quit_save_swap_restore_original_file(&st, "candidate-captured") {
            return;
        }
    }
    let mask = unsafe { system_quit_apply_foreign_profile_summary_preview(base, &bytes) };
    if mask == 0 {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: valid replacement candidate had no readable character slots path='{path}' len={len} hash=0x{hash:016x}; active file restored, preview not applied"
        ));
        return;
    }
    let mut st = system_quit_save_swap_lock();
    st.candidate_bytes = bytes;
    st.candidate_hash = hash;
    st.candidate_slot_mask = mask;
    st.preview_applied = true;
    append_autoload_debug(format_args!(
        "system-quit-save-swap: applied FOREIGN ProfileSummary preview from replacement path='{path}' len={len} hash=0x{hash:016x} slot_mask=0x{mask:x}; active save file restored until the user selects a foreign slot"
    ));
}

unsafe fn system_quit_save_swap_prepare_selected_slot(slot: i32) -> Result<bool, ()> {
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot) {
        return Ok(false);
    }
    let mut st = system_quit_save_swap_lock();
    if !st.preview_applied || st.committed {
        return Ok(false);
    }
    let bit = 1usize << slot as usize;
    if st.candidate_slot_mask & bit == 0 {
        append_autoload_debug(format_args!(
            "system-quit-save-swap: refusing ProfileSelect activation for slot {slot}; foreign preview active but slot bit is absent mask=0x{:x}",
            st.candidate_slot_mask
        ));
        return Err(());
    }
    if st.path.is_empty() || st.candidate_bytes.is_empty() {
        return Err(());
    }
    match fs::write(&st.path, &st.candidate_bytes) {
        Ok(()) => {
            st.committed = true;
            st.armed = false;
            append_autoload_debug(format_args!(
                "system-quit-save-swap: committed foreign save before slot activation path='{}' slot={slot} len={} hash=0x{:016x}; fresh deserialize will read this file",
                st.path,
                st.candidate_bytes.len(),
                st.candidate_hash
            ));
            Ok(true)
        }
        Err(err) => {
            append_autoload_debug(format_args!(
                "system-quit-save-swap: FAILED to commit foreign save for slot {slot} path='{}': {err}; blocking activation to avoid loading stale/original bytes",
                st.path
            ));
            Err(())
        }
    }
}

/// Patch the loaded slot's profile offscreen RT size BEFORE any post-Continue profile renderer is
/// constructed. The constructor snapshots this table; patching after `PROFILE_TABLE_BUILDER_RVA` runs is
/// too late and produces the 256x256 loading-screen portrait (Bug A). Returns true only when the loaded
/// slot is known and its row is confirmed at the configured target size.
unsafe fn patch_profile_offscreen_size_for_loaded_slot(base: usize) -> bool {
    if !portrait_real_pixels_enabled() {
        return true;
    }
    let Some(target) = portrait_loaded_slot_confirmed() else {
        return false;
    };
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&target) {
        return false;
    }
    let bit = 1usize << (target as usize);
    if PROFILE_SIZE_PATCHED.load(Ordering::SeqCst) & bit != 0 {
        return true;
    }
    let table = base + PROFILE_OFFSCREEN_SIZE_TABLE_RVA;
    let row = table + target as usize * PROFILE_OFFSCREEN_SIZE_TABLE_STRIDE;
    let cur = unsafe { safe_read_usize(row) }.unwrap_or(0);
    let patched = if cur == PROFILE_OFFSCREEN_SIZE_TARGET {
        true
    } else if cur == PROFILE_OFFSCREEN_SIZE_INIT {
        unsafe {
            core::ptr::write_volatile(row as *mut u64, PROFILE_OFFSCREEN_SIZE_TARGET as u64);
            core::ptr::write_volatile(
                (row + PROFILE_OFFSCREEN_SIZE_SUPERSAMPLE_FLAG_OFFSET) as *mut u8,
                0,
            );
        }
        true
    } else {
        false
    };
    if patched {
        PROFILE_SIZE_PATCHED.fetch_or(bit, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "portrait-res: pre-builder target slot {target} row=0x{cur:x} patched={} -> base 2056x2056, native supersample off (expected RT 2056x2056); other slots left native 128",
        if patched { 1 } else { 0 }
    ));
    patched
}

pub(crate) unsafe fn force_profile_render_tick(base: usize, _slot: i32) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == 0 || base == null {
        return;
    }
    // RE-ENGAGE on every loading screen (subsequent-character-load fix): pause the build pipeline ONLY
    // during active gameplay, not permanently after the first world -- so a System Quit character switch's
    // loading screen re-builds + re-captures the NEW character's portrait.
    if unsafe { portrait_pipeline_idle_in_gameplay(base) } {
        return;
    }
    let valid = |p: usize| p != 0 && p != null;
    // POST-CONTINUE PORTRAIT: before the table-ready guard below (which would early-return on the
    // torn-down post-Continue table), repopulate the table during now-loading so the rest of this tick
    // (mark+refresh feed) and the look-at/draw/oracle run on the loading screen.
    unsafe { maybe_build_profile_table_for_loading(base) };
    // VISIBILITY: once our built renderer's offscreen RT is live, swap it into the now-loading background
    // container the forge already injected (the background binds BEFORE our renderer exists and never
    // re-binds, so the live RT must be pushed into the displayed container after the fact).
    unsafe { refresh_loading_bg_live_gx(base) };
    // Once the real IBL-lit menu portrait has been baked into LOADING_BG_PORTRAIT_RGBA, drive the loading
    // screen to show it. Two paths, mutually exclusive:
    //  * lookat path (product default): CANDIDATE A (er-effects-rs-jsm) -- copy the live head INTO the
    //    DISPLAYED now-loading GFx texture so the movie's own tips + Gauge_3 bar render ABOVE it. This
    //    demotes the Present-overlay while it succeeds; on any miss the overlay keeps showing the head.
    //  * non-lookat path: the legacy in-place re-forge of the CS-side texture (single static head, no
    //    live tracking; the overlay is not running there).
    if portrait_lookat_enabled() {
        unsafe { maybe_update_gfx_loading_portrait(base) };
        // PIVOT (er-effects-rs-jsm): build the player-stats text bitmap (game menu font) once the stats +
        // font are readable, for the overlay to composite on top of the head in place of the native tips.
        unsafe { maybe_build_stats_text() };
    } else {
        unsafe { maybe_reforge_loading_portrait(base) };
    }
    // Product source ownership: the pre-Continue/ProfileSelect renderer is not our loading portrait
    // source. Ignore it completely (no kick, no spare candidate, no bake-capture/dump) until the
    // loading-screen-owned table has been built by maybe_build_profile_table_for_loading above.
    if PROFILE_LOADSCREEN_TABLE_OWNED.load(Ordering::SeqCst) == 0 {
        return;
    }
    // ProfileSummary = GameDataMan -> slot-manager container.
    let gdm = game_data_man_ptr_or_null();
    if !valid(gdm) {
        return;
    }
    let summary = unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(0);
    if !valid(summary) {
        return;
    }
    // SLOT->NAME dump, once per run (er-effects-rs-hi2 attribution): the anomaly hypothesis is
    // character-specific (Patches' boot/menu-path lifecycle differs on reload), so per-window
    // anomalies must be joinable to WHICH character each retarget slot holds -- readable here from
    // the ProfileSummary records the pipeline already uses.
    if PROFILE_SLOT_NAMES_DUMPED.load(Ordering::SeqCst) == 0 {
        // Only consume the one-shot once at least one REAL name is readable: this runs before the
        // boot ProfileSummary save read (~+16s), and latching on the pre-read table logged ten
        // "(empty)" slots (run 2026-07-03 ~21:14). Keep retrying until the records are populated.
        let mut names: Vec<String> = Vec::with_capacity(TITLE_PROFILE_SLOT_COUNT);
        let mut any_real = false;
        for s in 0..TITLE_PROFILE_SLOT_COUNT {
            let rec = summary + PROFILE_SUMMARY_RECORD_BASE + s * PROFILE_SUMMARY_RECORD_STRIDE;
            let (units, len) = unsafe { read_utf16_name_units(rec) };
            let name = if utf16_name_empty_like(&units, len) {
                "(empty)".to_owned()
            } else {
                any_real = true;
                String::from_utf16_lossy(&units[..len])
            };
            names.push(format!("{s}={name}"));
        }
        if any_real {
            PROFILE_SLOT_NAMES_DUMPED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!("profile-slot-names: {}", names.join(" ")));
        }
    }
    // GUARD (crash fix): only call refresh once the renderer table is LIVE -- it is populated at
    // TitleTopDialog ctor (main menu), NOT at early title. Calling refresh before the table exists
    // AVs inside refresh (observed crash rva 0x9aa6d4 = refresh+0x54 at +8939ms). Require slot-0's
    // table entry to be a valid CSMenuProfModelRend before marking/refreshing.
    let probe = unsafe { safe_read_usize(portrait_renderer_table_entry(base, 0)) }.unwrap_or(0);
    if !valid(probe)
        || unsafe { safe_read_usize(probe) }.unwrap_or(0)
            != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
    {
        return;
    }
    // IMMEDIATE BUILD KICK (regression fix -- goal issue 1, grounded in the 06-29 vs 06-30 capture diff):
    // the 240-tick / feed cadence below can fire BEFORE the native boot ProfileSummary read makes the
    // autoload target slot real (~+17s). When it does, the mark loop marks 0 real slots, refresh requests
    // nothing, the renderer's +0x754 "load-requested" latch stays 0, and the model never builds in the
    // brief now-loading window -> nothing to capture (06-30 runs: req754=0 req755=0 model=0x0). 06-29 runs
    // that captured a portrait marked the slot WHILE refresh ran (req755=1 -> model=0x<nonzero>); the
    // all-slots-mark removal (correctly gated on a real fingerprint to avoid contaminating empty slots'
    // saveSlotsStates) lost that build-request for slot 0 because the cadence no longer coincides with the
    // moment the slot goes real. So here, edge-triggered: the instant a slot's fingerprint is real AND its
    // renderer's +0x754 is still 0, mark + refresh it immediately (off-cadence) and open the feed window to
    // drive the async build to completion. Idempotent -- once +0x754 latches to 1 this no-ops, so no churn.
    // Only marks REAL slots (post-read), identical to the cadence loop's gate, so it can't pre-empt the read.
    // ONLY THE LOADED SLOT (2026-06-30, user: a DIFFERENT character showed on the loading screen). The
    // save holds multiple characters (all 10 slots build models), and the slot-0 readback grabbed a
    // neighbouring slot's identical-size 1024 RT -> wrong face. Build + mark ONLY the autoload target slot
    // so the loaded character (Banon, slot 0) is the ONLY portrait model that exists -> no wrong-slot grab.
    // CORRELATION FIX (er-effects-rs-j3r): render the slot the game ACTUALLY loaded (ac0), via the
    // shared `portrait_loaded_slot*` source used by every portrait site (build/capture/spare).
    // CONFIRMED-ONLY (run anim-bind5, 2026-07-03): before ac0/stepper name the slot, do NOTHING --
    // the old fallback-to-0 kicked a foreign slot-0 build ~340ms early; storm-free that model
    // persisted and the single-model stability gate starved the drive/publish/anim pipeline for the
    // whole load. The lever loops below are no-ops with no model built, so skipping the tick is safe.
    let Some(target_slot) = portrait_loaded_slot_confirmed() else {
        return;
    };
    // FAIL-FAST SEMAPHORE: assert the slot we're about to render IS the loaded character
    // (er-effects-rs-j3r). With the correlation fix above, condition A (wrong-slot) is structurally
    // satisfied and stands as a regression tripwire; condition B (null loaded-slot renderer) stays a
    // live guard against the 3rd-open null-deref class.
    unsafe { portrait_render_slot_semaphore(base, target_slot) };
    {
        let mark: unsafe extern "system" fn(usize, i32) -> u8 =
            unsafe { core::mem::transmute(base + PROFILE_MARK_SLOT_USED_RVA) };
        let mut kicked = 0u32;
        let mut kicked_mask = 0u32;
        for s in 0..10i32 {
            // ONE SLOT (GX-overflow revert): immediate-kick only the target (see cadence loop).
            if s != target_slot {
                continue;
            }
            if !unsafe { profile_slot_fingerprint(s).0 } {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if !valid(r)
                || unsafe { safe_read_usize(r) }.unwrap_or(0)
                    != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                continue;
            }
            // +0x754 = the refresh's "load-requested" idempotency latch. 0 = the async model build was
            // never kicked for this slot -> kick it now. Non-zero -> already requested, skip.
            if unsafe { safe_read_u8(r + 0x754) }.unwrap_or(0xff) != 0 {
                continue;
            }
            let _ = unsafe { mark(summary, s) };
            // PER-SLOT kick replica (not the engine's GLOBAL refresh, which would kick EVERY marked
            // slot and build all the save's characters mid-load -> the cross-slot portrait swap).
            if unsafe { kick_target_profile_slot(base, summary, r, s) } {
                kicked += 1;
                kicked_mask |= 1 << s;
            }
        }
        if kicked > 0 {
            // Drive the freshly-requested build to completion + keep it latched through the loading screen.
            PROFILE_LOADSCREEN_FEED_TICKS
                .store(PROFILE_LOADSCREEN_FEED_WINDOW_TICKS, Ordering::SeqCst);
            if PROFILE_REAL_SLOT_KICK_LOGGED.swap(1, Ordering::SeqCst) == 0 {
                append_autoload_debug(format_args!(
                    "force-profile-render: IMMEDIATE build kick -- {kicked} real slot(s) (mask=0x{kicked_mask:x}) became available with req754=0; marked + per-slot kicked off-cadence + opened feed window (summary=0x{summary:x})"
                ));
            }
        }
    }
    // MODEL BUILD: every ~240 ticks, mark all 10 profile slots used + call the refresh that kicks the
    // async character-model build. refresh is IDEMPOTENT per slot via the +0x754 "load-requested" latch,
    // so by default this builds each model ONCE and then leaves it -- the model stays LIVE every frame,
    // which is what the realtime look-at draw needs (an invalid/rebuilding pose-holder fails the draw).
    //
    // DESTRUCTIVE REBUILD (default OFF, `portrait_force_rebuild_enabled`): clear each build latch
    // (+0x754/+0x755) + reset the look-at slot cache to force a FRESH build. The churn leaves models
    // not-live most of the time (~88% draw failures -> flicker), so it is opt-in: flip it on briefly to
    // re-capture the post-FaceData face (an early build before LOAD GAME loads FaceData = default head),
    // then off. See `portrait_force_rebuild_enabled` and bd portrait-lookat-realtime-drawphase-design.
    let counter = PROFILE_FORCE_TICK_COUNTER.fetch_add(1, Ordering::SeqCst);
    // Post-Continue feed window: while it is open, run the (idempotent) mark+refresh every 8 ticks so the
    // freshly-built renderers' async model build is driven to completion and stays latched -- the once-per-
    // 240 baseline is too sparse for the brief now-loading window. Outside the window keep the 240 cadence.
    let feed_window = PROFILE_LOADSCREEN_FEED_TICKS.load(Ordering::SeqCst) > 0;
    if feed_window {
        PROFILE_LOADSCREEN_FEED_TICKS.fetch_sub(1, Ordering::SeqCst);
    }
    if counter % 240 == 0 || (feed_window && counter % 8 == 0) {
        let log_this = counter % 240 == 0; // throttle the in-window feed log to once per 240
        let force_rebuild = portrait_force_rebuild_enabled();
        let mark: unsafe extern "system" fn(usize, i32) -> u8 =
            unsafe { core::mem::transmute(base + PROFILE_MARK_SLOT_USED_RVA) };
        let mut marked = 0u32;
        for s in 0..10i32 {
            // ONE SLOT (GX-overflow revert, user 2026-07-03): build ONLY the autoload target. Rendering
            // every saved slot overran the 192-slot GX command queue (0x1aeaf05 null-slot-write crash) --
            // 10 concurrent live renderers' draw tasks, independent of RT size (target-only 1024 didn't
            // help). All-slots menu portraits will be handled at the GFX/surface layer (remove the
            // DummyProfileFace surface) instead of driving all 10 renderers.
            if s != target_slot {
                continue;
            }
            // Real-character gate (per the native boot ProfileSummary read: level>=1 + non-empty name).
            // Never mark before the read populates the slot (can't pre-empt it / contaminate saveSlotsStates).
            if !unsafe { profile_slot_fingerprint(s).0 } {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            let r_valid = valid(r)
                && unsafe { safe_read_usize(r) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA;
            if force_rebuild && r_valid {
                unsafe {
                    core::ptr::write_volatile((r + 0x754) as *mut u8, 0);
                    core::ptr::write_volatile((r + 0x755) as *mut u8, 0);
                }
            }
            let _ = unsafe { mark(summary, s) };
            // PER-SLOT kick replica in place of the engine's GLOBAL refresh: the global form kicked
            // every marked slot (all the save's characters) -- the cross-slot portrait swap source.
            // Idempotent via the +0x754/+0x755 gate inside, so the feed cadence just re-tries until
            // the record is real and then no-ops.
            if r_valid {
                let _ = unsafe { kick_target_profile_slot(base, summary, r, s) };
            }
            marked += 1;
        }
        // TRIPWIRE oracle: count non-target renderers holding a live model during our feed window.
        // Expected 0 with one-slot render -- any foreign live model is the swap-bug precondition.
        let mut foreign = 0usize;
        for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
            if s == target_slot {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if valid(r)
                && unsafe { safe_read_usize(r) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
                && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0)
                    != 0
            {
                foreign += 1;
            }
        }
        PROFILE_FOREIGN_MODELS_MAX.fetch_max(foreign, Ordering::SeqCst);
        if log_this {
            append_autoload_debug(format_args!(
                "force-profile-render: build cycle (counter={counter}) force_rebuild={force_rebuild} feed_window={feed_window} -- marked {marked} real slot(s) + per-slot kicked (summary=0x{summary:x} foreign_models={foreign})"
            ));
        }
        // Only when we forced a fresh build: drop the cached look-at indices/base so they re-resolve and
        // re-latch the idle base from the fresh skeleton. Without a forced rebuild the model (and its
        // skeleton) persist, so KEEP the cache -> the look-at keeps driving every frame with no re-resolve gap.
        if force_rebuild {
            match PROFILE_LOOKAT_SLOTS.lock() {
                Ok(mut g) => *g = [None; 10],
                Err(p) => *p.into_inner() = [None; 10],
            }
            match PROFILE_CAM_FACE_YAW.lock() {
                Ok(mut g) => *g = [None; 10],
                Err(p) => *p.into_inner() = [None; 10],
            }
            PROFILE_CAM_FACE_YAW_LATCHED_MASK.store(0, Ordering::SeqCst);
            // The models are being rebuilt -> the cached PoseHolder pointers are about to go stale. Drop
            // them so they re-resolve against the fresh skeletons (and re-latch a clean base) before the
            // sticky-keep path above starts driving them again.
            for h in PROFILE_LOOKAT_HOLDERS.iter() {
                h.store(0, Ordering::SeqCst);
            }
        }
    }
    // ~80 ticks AFTER each rebuild kick, reset the dump mask so the freshly-rebuilt models (not the
    // stale pre-clear model_ins) get re-dumped. Each cycle's dumps overwrite the per-slot files.
    if counter % 240 == 80 {
        PROFILE_SLOT_DUMP_MASK.store(0, Ordering::SeqCst);
    }
    // CAMERA LEVER: every tick, override each live renderer's orbit camera with our custom viewport.
    // Re-applied so a refresh that re-runs the engine camera setup can't win; the dump loop below then
    // captures the custom-framed RT. Gated under the same `portrait_real_pixels` diagnostic as the dump.
    if portrait_real_pixels_enabled() {
        for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if valid(r)
                && unsafe { safe_read_usize(r) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                unsafe { apply_profile_camera_override(base, r, s) };
            }
        }
    }
    // LOOK-AT LEVER: every tick, rotate each live renderer's Head/Neck/Spine2 bones toward the mouse
    // cursor so the portrait's gaze follows it (eyes are welded to the Head bone). Separate gate from
    // the camera/dump so the riskier bone-write path can be toggled on its own.
    if portrait_lookat_enabled() {
        for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if valid(r)
                && unsafe { safe_read_usize(r) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                // FrameBegin role (this task): REGISTER the holder + resolve Head/Neck/Spine2 indices +
                // publish the cursor. The per-frame write+recompute+DRAW that makes the head track the
                // cursor in realtime now happens in `profile_lookat_realtime_draw_tick`, a separate
                // recurring task in the GameSceneDraw phase (render thread, inside a live GX frame). The
                // old per-tick game-task offscreen drive rendered black (FrameBegin = before the GX frame
                // records); the draw-phase task is the fix.
                unsafe { apply_profile_lookat(r, s) };
                // SPARE PRE-RECORD: capture the target slot's renderer as the spare candidate on a frame
                // where its model is actually BUILT (+0x778 valid), so the teardown-spare hook can protect
                // this exact renderer through Continue even though the menu cycles model_ins. Uses
                // portrait_target_slot() so that once the user confirms a switch (SELECTED_SLOT set), the
                // candidate re-records for the NEWLY-selected character, not the still-resident old ac0.
                let target = portrait_target_slot();
                if s == target
                    && PROFILE_SPARE_CANDIDATE.load(Ordering::SeqCst) == 0
                    && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                        .map(|m| valid(m))
                        .unwrap_or(false)
                {
                    PROFILE_SPARE_CANDIDATE.store(r, Ordering::SeqCst);
                    let model = unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                        .unwrap_or(0);
                    PROFILE_SPARE_CANDIDATE_MODEL.store(model, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "loading-portrait: pre-recorded spare candidate renderer=0x{r:x} slot={s} model_ins=0x{model:x} (loading-screen-owned renderer)"
                    ));
                }
            }
        }
    }
    // Per-slot: once a slot's model (+0x778) has built, readback its COLOR offscreen RT and dump to
    // portrait-capture-slot{N}.bin ONCE (tracked via PROFILE_SLOT_DUMP_MASK). Inspect the 10 dumps
    // offline and match to the known disk characters to map renderer-slot -> character.
    if portrait_real_pixels_enabled() {
        for s in 0..10i32 {
            let bit = 1usize << s;
            if PROFILE_SLOT_DUMP_MASK.load(Ordering::SeqCst) & bit != 0 {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if !valid(r)
                || unsafe { safe_read_usize(r) }.unwrap_or(0)
                    != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                continue;
            }
            let model =
                unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
            if !valid(model) {
                continue;
            }
            let off = unsafe {
                safe_read_usize(r + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
            }
            .unwrap_or(0);
            if !valid(off) {
                continue;
            }
            // LIGHTING residency oracle: envObj = renderer+0x760; *(envObj) is the registered IBL
            // env-region id, non-zero ONLY if the GILM env map was resident when the IBL built.
            let env_obj =
                unsafe { safe_read_usize(r + PROFILE_RENDERER_ENV_REGION_OFFSET) }.unwrap_or(0);
            let ibl_region = if valid(env_obj) {
                unsafe { safe_read_usize(env_obj) }.unwrap_or(0)
            } else {
                0
            };
            if let Some((w, h, px)) = unsafe { readback_offscreen_rgba8(off) } {
                let nb = portrait_center_nonblack(w, h, &px);
                let checker = portrait_looks_like_checker(w, h, &px);
                // BAKE SOURCE: store the TARGET slot's menu portrait into LOADING_BG_PORTRAIT_RGBA so the
                // now-loading forge bakes IT into the static TPF (the proven decode-time display path) AND the
                // present-overlay composite (gated on PROFILE_BAKE_RGBA_CAPTURED) displays it. ONLY latch on a
                // REAL FACE: nonblack alone false-passes the magenta/white checker (an unrendered RT or our
                // cover placeholder) -- latching that is exactly what put a center checker square on screen and
                // made oracle_..._gx_nonblack a false success. Requiring !checker means we keep re-checking each
                // dump cycle and latch only once a real shaded head has actually rendered into the offscreen
                // (which needs the render-thread offscreen drive -- see portrait_render_drive). One-shot via swap.
                if s == portrait_loaded_slot()
                    && nb
                    && !checker
                    && PROFILE_BAKE_RGBA_CAPTURED.swap(1, Ordering::SeqCst) == 0
                {
                    let _ = ibl_region;
                    dump_portrait_rgba(110, w, h, &px);
                    // Readiness gate: hold back neutral/too-small transient captures (Bug A/B). On
                    // rejection, un-consume the one-shot (the swap fired in the condition above) so a
                    // later full-size head still bake-captures.
                    if note_ls_portrait_capture(w, h, &px) {
                        if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
                            *g = Some((w, h, px.clone()));
                        }
                        append_autoload_debug(format_args!(
                            "loading-portrait: BAKE-CAPTURED real menu portrait slot={s} dims={w}x{h} ibl_region=0x{ibl_region:x} -> LOADING_BG_PORTRAIT_RGBA (forge will bake it)"
                        ));
                    } else {
                        PROFILE_BAKE_RGBA_CAPTURED.store(0, Ordering::SeqCst);
                    }
                }
                dump_portrait_rgba(s, w, h, &px);
                PROFILE_SLOT_DUMP_MASK.fetch_or(bit, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "profile-slot-dump: slot={s} renderer=0x{r:x} model=0x{model:x} dims={w}x{h} nonblack={} env_obj=0x{env_obj:x} ibl_region=0x{ibl_region:x}",
                    nb as u8
                ));
            }
        }
    }
}

/// Hook on the CSMenuProfModelRend teardown-all (`FUN_1409b2f00`). One-shot: before the original
/// runs, save slot-0's renderer and null its table entry so the original's null-guarded delete
/// enqueue skips it -- sparing the loaded character's portrait renderer from the Continue teardown so
/// we can keep rendering it into the now-loading screen. The original then tears down slots 1-9.
pub(crate) unsafe extern "system" fn profile_renderer_teardown_spare_hook() {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    // TEARDOWN FENCE (freeze relaxation, er-effects-rs-l1x): raise the fence BEFORE any
    // delete-enqueue below (both the orphan reclaim and the native table teardown in original()),
    // then wait out a render-thread pump caught mid-drive. The pump is one model update+draw
    // (sub-ms), so the 10ms cap is generous; a timeout is counted, not fatal -- worst case equals
    // the OLD per-frame TOCTOU exposure for exactly one frame instead of every frame. The fence is
    // lowered at the end of this hook, after the native teardown returns.
    PROFILE_RENDERER_TEARDOWN_FENCE.store(1, Ordering::SeqCst);
    if PROFILE_IN_OUR_DRIVE.load(Ordering::SeqCst) {
        PROFILE_TEARDOWN_FENCE_WAITS.fetch_add(1, Ordering::SeqCst);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(10);
        while PROFILE_IN_OUR_DRIVE.load(Ordering::SeqCst) {
            if std::time::Instant::now() > deadline {
                PROFILE_TEARDOWN_FENCE_TIMEOUTS.fetch_add(1, Ordering::SeqCst);
                break;
            }
            std::thread::yield_now();
        }
    }
    // REPEATED-SWITCH GX OVERFLOW FIX (0x1aeaf05, ~switch #4): destroy the PRIOR window's spared
    // renderer now, on the game thread, before sparing this switch's renderer. The load-complete
    // reset (render thread) moved it into PROFILE_SPARE_ORPHAN instead of dropping it; the spare
    // excluded it from the native delete (nulled its table slot), so without this it stayed alive
    // with its ResMan offscreen draw task filling the 192-slot GX command queue every frame,
    // accumulating +1 leaked renderer per switch. delay_delete_enqueue_renderer is the exact native
    // delete path (vtable-guarded), run here on the correct thread.
    let orphan = PROFILE_SPARE_ORPHAN.swap(0, Ordering::SeqCst);
    if orphan != 0 {
        let deleted = unsafe { delay_delete_enqueue_renderer(orphan) };
        // Ownership ledger: discharge our responsibility for the spared renderer (paired with the
        // ownership_take at the spare site). Released whether or not the enqueue took -- either we
        // handed it to delay-delete or it was already stale/gone; either way it is no longer ours.
        ownership_release(OwnedClass::SparedRenderer);
        append_autoload_debug(format_args!(
            "loading-portrait: reclaimed prior spared renderer 0x{orphan:x} via CSDelayDeleteMan enqueued={deleted} (repeated-switch GX command-queue leak fix)"
        ));
    }
    // Gate on the look-at/portrait feature OR product autoload -- the native-continue path does NOT set
    // PRODUCT_AUTOLOAD_ARMED, so gating on product_autoload alone never spared anything there.
    if LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst) == 0
        && (product_autoload_enabled() || portrait_lookat_enabled())
    {
        if let Ok(base) = game_module_base() {
            // The slot we render (er-effects-rs-j3r): the newly-selected character on a switch
            // (SELECTED_SLOT), else the loaded slot (ac0). portrait_target_slot() is what makes the
            // loading portrait show the character just picked, not the one still resident.
            let slot = portrait_target_slot();
            // Prefer the PRE-RECORDED candidate (captured at the menu on a model-built frame -- robust to
            // the menu's model_ins cycling). Find its table slot and protect it. Fall back to reading
            // table[slot] + a model-built guard if no candidate was recorded.
            let candidate = PROFILE_SPARE_CANDIDATE.load(Ordering::SeqCst);
            let target_te = portrait_renderer_table_entry(base, slot);
            // Honor the pre-recorded candidate ONLY if it still sits in the TARGET slot. A candidate
            // captured for the old character before a switch confirm must not be spared over the
            // newly-selected one -- in that case fall back to table[target] (its model is built, the
            // menu rendered all 10 slots). Prevents the loading portrait showing the prior character.
            let candidate_in_target =
                valid(candidate) && unsafe { safe_read_usize(target_te) }.unwrap_or(0) == candidate;
            let (renderer, table, spared_slot) = if candidate_in_target {
                (candidate, target_te, slot)
            } else {
                let r = unsafe { safe_read_usize(target_te) }.unwrap_or(0);
                let model_built = valid(r)
                    && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                        .map(|m| valid(m))
                        .unwrap_or(false);
                (if model_built { r } else { 0 }, target_te, slot)
            };
            if valid(renderer)
                && unsafe { safe_read_usize(renderer) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                LOADING_BG_PORTRAIT_SPARED_RENDERER.store(renderer, Ordering::SeqCst);
                PROFILE_RENDERER_SPARE_HITS.fetch_add(1, Ordering::SeqCst);
                // Ownership ledger: we just excluded this renderer from the native delete, so WE own
                // its destruction now. Paired with the ownership_release on the drain path below.
                ownership_take(OwnedClass::SparedRenderer);
                // Null the table entry so the original's null-guarded delete-enqueue skips it.
                if table != 0 {
                    unsafe { (table as *mut usize).write_volatile(0) };
                }
                // Re-latch the look-at base from the post-Continue model (a different model instance).
                if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&spared_slot) {
                    let mut guard = match PROFILE_LOOKAT_SLOTS.lock() {
                        Ok(g) => g,
                        Err(p) => p.into_inner(),
                    };
                    if let Some(s) = guard[spared_slot as usize].as_mut() {
                        s.base_latched = false;
                    }
                }
                let model_at_spare =
                    unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                        .unwrap_or(0);
                append_autoload_debug(format_args!(
                    "loading-portrait: SPARED slot{spared_slot} renderer=0x{renderer:x} (candidate=0x{candidate:x}) model_ins=0x{model_at_spare:x} from teardown -- drive look-at + render it post-Continue"
                ));
            }
        }
    }
    let orig = PROFILE_RENDERER_TEARDOWN_HOOK_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn() = unsafe { std::mem::transmute(orig) };
        unsafe { f() };
    }
    // Native teardown done -- the table entries are delete-enqueued/nulled, so the next pump
    // invocation's per-frame table re-read + vtable probe fails closed until the new window's
    // rebuild. Safe to let the drive back in.
    PROFILE_RENDERER_TEARDOWN_FENCE.store(0, Ordering::SeqCst);
}

/// Diagnostic + REPAIR detour on the native profile-portrait builder (`FUN_1409aa7d0` =
/// `PROFILE_RENDERER_REFRESH_RVA`). The builder derefs `table[slot]+0x754` with NO null check for
/// every slot whose profile record exists (Ghidra: `FUN_140261c30(summary,slot) != 0` gates the
/// walk, the entry itself is never checked), and its 10-slot table setup is called from exactly ONE
/// native site -- the TitleTopDialog constructor -- so our cloned in-world ProfileSelect reopens run
/// it against whatever the last teardown left; the 3rd in-session open found the table fully empty
/// and AV'd at `[null+0x754]` (er-effects-rs-j3r). Three layers, all fault-guarded + catch_unwind:
///   1. DIAG: log the full table once per distinct degraded (mask, caller) pattern.
///   2. REPAIR: a FULLY-empty table (the proven crash state) is rebuilt via the engine's own no-arg
///      setup (`PROFILE_TABLE_BUILDER_RVA`; its internal teardown is a no-op on an all-null table),
///      satisfying the native invariant exactly as the TitleTopDialog ctor would. Gated on
///      `PROFILE_TABLE_WAS_POPULATED` (engine/ResMan up -- the setup AVs at boot title) and on
///      fully-empty ONLY: a MIXED table is the intentional teardown-spare state during Continue
///      loading and must not be rebuilt over.
///   3. GUARD: if any slot is still null/invalid after the (possible) repair, SKIP chaining the
///      original this call (fail-soft; the per-frame builder retries) instead of letting the native
///      walk AV.
pub(crate) unsafe extern "system" fn profile_select_table_diag_hook() {
    let chain = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(base) = game_module_base() else {
            return true;
        };
        let null = TITLE_OWNER_SCAN_START_ADDRESS;
        let scan_table = |ptrs: &mut [usize; TITLE_PROFILE_SLOT_COUNT]| -> (u32, u32) {
            let mut null_mask = 0u32;
            let mut valid_mask = 0u32;
            for s in 0..TITLE_PROFILE_SLOT_COUNT {
                let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s as i32)) }
                    .unwrap_or(0);
                ptrs[s] = r;
                let is_valid = r != 0
                    && r != null
                    && unsafe { safe_read_usize(r) }.unwrap_or(0)
                        == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA;
                if is_valid {
                    valid_mask |= 1 << s;
                } else {
                    null_mask |= 1 << s;
                }
            }
            (valid_mask, null_mask)
        };
        let mut ptrs = [0usize; TITLE_PROFILE_SLOT_COUNT];
        let (valid_mask, mut null_mask) = scan_table(&mut ptrs);
        // Degraded = ANY slot lost its renderer while the builder is about to run. A HEALTHY table
        // is all 10 valid (native setup allocs all 10 unconditionally); any null is the crash-prone
        // state, INCLUDING all-null (the fully-empty table that caused the 3rd-open crash -- the
        // earlier "mixed only" check missed it). Log per distinct (mask, caller) so it never spams.
        let degraded = null_mask != 0;
        let caller_rva = crate::crashlog::trace_first_game_caller_rva();
        let key =
            ((caller_rva & 0xffffff) << 20) | ((valid_mask as usize) << 10) | null_mask as usize;
        if degraded && PROFILE_SELECT_TABLE_DIAG_LAST.swap(key, Ordering::SeqCst) != key {
            append_crash_log(format_args!(
                "PROFILESELECT-TABLE-DIAG: degraded profile-renderer table before native builder (er-effects-rs-j3r) caller_rva=0x{caller_rva:x} valid_mask=0x{valid_mask:x} null_mask=0x{null_mask:x} entries=[0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x}]",
                ptrs[0], ptrs[1], ptrs[2], ptrs[3], ptrs[4], ptrs[5], ptrs[6], ptrs[7], ptrs[8],
                ptrs[9]
            ));
        } else if !degraded {
            PROFILE_SELECT_TABLE_DIAG_LAST.store(0, Ordering::SeqCst);
            // A fully-valid table at builder entry proves the engine built renderers successfully --
            // the same "engine/ResMan up" evidence the loading-screen path latches; latching it here
            // too arms the repair even when the loading-portrait feature is disabled.
            PROFILE_TABLE_WAS_POPULATED.store(1, Ordering::SeqCst);
        }
        if null_mask == PROFILE_TABLE_ALL_SLOTS_MASK
            && PROFILE_TABLE_WAS_POPULATED.load(Ordering::SeqCst) != 0
        {
            let build: unsafe extern "system" fn() =
                unsafe { core::mem::transmute(base + PROFILE_TABLE_BUILDER_RVA) };
            unsafe { build() };
            let n = PROFILE_SELECT_TABLE_REPAIR_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            let (revalid_mask, renull_mask) = scan_table(&mut ptrs);
            null_mask = renull_mask;
            append_crash_log(format_args!(
                "PROFILESELECT-TABLE-REPAIR #{n}: fully-empty renderer table at native builder entry -> re-ran native table setup 0x{:x}; post-repair valid_mask=0x{revalid_mask:x} null_mask=0x{renull_mask:x} (er-effects-rs-j3r)",
                base + PROFILE_TABLE_BUILDER_RVA
            ));
            append_autoload_debug(format_args!(
                "profileselect-table-repair #{n}: rebuilt empty 10-slot renderer table via native setup before the native builder walked it; post-repair valid_mask=0x{revalid_mask:x} (er-effects-rs-j3r)"
            ));
        }
        if null_mask != 0 {
            let n = PROFILE_SELECT_TABLE_GUARD_SKIP_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            let skip_key = ((caller_rva & 0xffffff) << 10) | null_mask as usize;
            if PROFILE_SELECT_TABLE_GUARD_SKIP_LAST.swap(skip_key, Ordering::SeqCst) != skip_key {
                append_crash_log(format_args!(
                    "PROFILESELECT-TABLE-GUARD SKIP #{n}: null/invalid renderer slots remain (null_mask=0x{null_mask:x}) -- skipping the native builder this call so it cannot AV at [null+0x754] (er-effects-rs-j3r)"
                ));
            }
            return false;
        }
        true
    }))
    // A panicked diagnostic keeps the pre-hook behavior: chain the original.
    .unwrap_or(true);
    if !chain {
        return;
    }
    let orig = PROFILE_SELECT_TABLE_DIAG_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return;
    }
    let f: unsafe extern "system" fn() = unsafe { std::mem::transmute(orig) };
    unsafe { f() };
}
