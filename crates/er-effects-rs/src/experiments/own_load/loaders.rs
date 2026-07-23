
/// Restore `GLOBAL_CSGaitem` to constructor-pristine (empty gaitemInsTable + full free-queue) at a
/// clean title BEFORE the switch reload's fresh deserialize, so char#2's deserialize does not
/// exhaust the free-queue on char#1's leaked items (the AV at live 0x67141a, bd
/// system-quit-postswitch-crash-gaitem-freequeue-exhaustion-2026-07-02). Mechanism: sweep all
/// 0x1400 gaitemInsTable slots; for each occupied slot call the NATIVE per-item release
/// RemoveCSGaitemIns(gaitem, &entries[i].unindexedGaItemHandle) -- it destructs+deallocates the ins
/// (no leak) and returns index i to freeTableIdxQueue. This is the exact primitive the native
/// world/inventory teardown uses; we drive it because our lightweight return-title chain skips it.
///
/// SAVE-SAFETY / correctness preconditions (the CALLER must guarantee, and this fn re-checks what it
/// can): the old world is torn down (local player absent) so nothing live holds POINTERS to these
/// ins objects -- PlayerGameData/inventory hold only integer handles, which char#2's deserialize
/// overwrites. Structural validation (heap-aligned singleton, head/end within [0,0x1400)) fails
/// closed rather than sweeping a bogus pointer. Returns Some((released, slack_before, slack_after))
/// on success (slack = 0x13ff - free_count; healthy = slack_after 0), None if it declined.
pub(crate) unsafe fn own_load_reset_gaitem_singleton(base: usize) -> Option<(u32, u32, u32)> {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const RING_USABLE: u32 = (CSGAITEM_TABLE_CAPACITY as u32) - 1; // 0x13ff (one sentinel slot)
    let gaitem = unsafe { safe_read_usize(base + GLOBAL_CSGAITEM_SINGLETON_RVA) }.unwrap_or(NULL);
    if gaitem == NULL || !unsafe { is_heap_aligned_ptr(gaitem) } {
        append_autoload_debug(format_args!(
            "gaitem-reset: GLOBAL_CSGaitem not resident/aligned (0x{gaitem:x}) -- declining pristine-restore (no-op)"
        ));
        return None;
    }
    let free_count = |head: u32, end: u32| -> u32 {
        // Ring distance head..end over capacity 0x1400 = number of poppable free indices.
        end.wrapping_sub(head)
            .wrapping_add(CSGAITEM_TABLE_CAPACITY as u32)
            % (CSGAITEM_TABLE_CAPACITY as u32)
    };
    let head0 =
        unsafe { safe_read_i32(gaitem + CSGAITEM_FREE_QUEUE_HEAD_OFFSET) }.unwrap_or(-1) as u32;
    let end0 =
        unsafe { safe_read_i32(gaitem + CSGAITEM_FREE_QUEUE_END_OFFSET) }.unwrap_or(-1) as u32;
    if head0 as usize >= CSGAITEM_TABLE_CAPACITY || end0 as usize >= CSGAITEM_TABLE_CAPACITY {
        append_autoload_debug(format_args!(
            "gaitem-reset: free-queue head/end out of range (head=0x{head0:x} end=0x{end0:x} cap=0x{:x}) -- singleton not the expected CSGaitemImp; declining (no-op)",
            CSGAITEM_TABLE_CAPACITY
        ));
        return None;
    }
    let slack_before = RING_USABLE.saturating_sub(free_count(head0, end0));
    let remove_ins: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(base + CSGAITEM_REMOVE_INS_RVA) };
    let mut released: u32 = 0;
    for i in 0..CSGAITEM_TABLE_CAPACITY {
        let slot = gaitem + CSGAITEM_INS_TABLE_OFFSET + i * core::mem::size_of::<usize>();
        let ins = unsafe { safe_read_usize(slot) }.unwrap_or(NULL);
        if ins == NULL {
            continue;
        }
        // &entries[i].unindexedGaItemHandle -- its embedded index maps back to slot i (ctor seeds it,
        // alloc preserves it), so RemoveCSGaitemIns frees gaitemInsTable[i] and returns index i.
        let handle_ptr = gaitem + CSGAITEM_ENTRIES_OFFSET + i * CSGAITEM_ENTRY_STRIDE;
        unsafe { remove_ins(gaitem, handle_ptr) };
        released += 1;
    }
    let head1 =
        unsafe { safe_read_i32(gaitem + CSGAITEM_FREE_QUEUE_HEAD_OFFSET) }.unwrap_or(-1) as u32;
    let end1 =
        unsafe { safe_read_i32(gaitem + CSGAITEM_FREE_QUEUE_END_OFFSET) }.unwrap_or(-1) as u32;
    let slack_after = RING_USABLE.saturating_sub(free_count(head1, end1));
    SYSTEM_QUIT_GAITEM_RESET_INVOCATIONS.fetch_add(1, Ordering::SeqCst);
    SYSTEM_QUIT_GAITEM_RESET_RELEASED_COUNT.fetch_add(released as usize, Ordering::SeqCst);
    SYSTEM_QUIT_GAITEM_RESET_LAST_SLACK_BEFORE.store(slack_before as usize, Ordering::SeqCst);
    SYSTEM_QUIT_GAITEM_RESET_LAST_SLACK_AFTER.store(slack_after as usize, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "gaitem-reset: pristine-restore gaitem=0x{gaitem:x} released={released} free-queue head/end 0x{head0:x}/0x{end0:x} -> 0x{head1:x}/0x{end1:x} slack {slack_before}->{slack_after} (0=full); native RemoveCSGaitemIns 0x{:x} per occupied slot",
        base + CSGAITEM_REMOVE_INS_RVA
    ));
    Some((released, slack_before, slack_after))
}

/// SYNCHRONOUS fresh picked-slot feed-deserialize for the System->Quit->Load-Profile switch (the
/// continue_confirm hook calls this BEFORE forwarding, so the c30/PGD the confirm streams belong to
/// the PICKED slot -- bd system-quit-cleantitle-load-is-stale-restream-not-slot-source-2026-07-02).
/// Same proven mechanism as `own_load_drive` steps 1-4: read the on-disk save (native SAVE-DIR
/// builder path -- post-first-load the redirect has reverted, so this is the file the quit-save
/// just wrote), slice slot `want_slot`'s plaintext body, arm the gated 0x67b100 read detour, call
/// the native parser 0x67b290(slot) in-process. Returns true only when the parse produced a real
/// c30 + a real PlayerGameData fingerprint. Save-safe: read-only on the .sl2 (no SetState5, no
/// save write; the deserialize also repoints GameMan+0xac0 to `want_slot` as its normal byproduct).
pub(crate) unsafe fn own_load_feed_deserialize(base: usize, gm: usize, want_slot: i32) -> bool {
    const C30_ZERO: i32 = 0;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if gm == null || want_slot < OWN_STEPPER_SLOT_ZERO {
        append_autoload_debug(format_args!(
            "own-load-feed: rejected gm=0x{gm:x} slot={want_slot} -- need GameMan + explicit slot (no-write)"
        ));
        return false;
    }
    let Some(sl2_bytes) = (unsafe { own_load_read_sl2_bytes(base) }) else {
        return false;
    };
    let body: &[u8] = match er_save_loader::bnd4::slot_body(&sl2_bytes, want_slot as usize) {
        Ok(b) => b,
        Err(e) => {
            append_autoload_debug(format_args!(
                "own-load-feed: slot_body(slot={want_slot}) failed: {e:?} -- ABORT (no-write)"
            ));
            return false;
        }
    };
    // Leak the sliced body so it stays valid for the detour to memcpy (one bounded copy per switch).
    let leaked: &'static [u8] = Box::leak(body.to_vec().into_boxed_slice());
    OWN_LOAD_BODY_PTR.store(leaked.as_ptr() as usize, Ordering::SeqCst);
    OWN_LOAD_BODY_LEN.store(leaked.len(), Ordering::SeqCst);
    if !install_own_load_hook() {
        append_autoload_debug(format_args!(
            "own-load-feed: hook install failed -- ABORT (no-write)"
        ));
        return false;
    }
    let c30_before =
        unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(GAME_MAN_C30_UNSET);
    OWN_LOAD_GATE.store(true, Ordering::SeqCst);
    let parser: unsafe extern "system" fn(i32) -> i32 =
        unsafe { std::mem::transmute(base + DESERIALIZE_SLOT_RVA) };
    let pret = unsafe { parser(want_slot) };
    OWN_LOAD_GATE.store(false, Ordering::SeqCst);
    let fed = OWN_LOAD_FED_BYTES.load(Ordering::SeqCst);
    let c30 =
        unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(GAME_MAN_C30_UNSET);
    let ac0 = unsafe { safe_read_i32(gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) }
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
    let c30_real = c30 != GAME_MAN_C30_UNSET && c30 != C30_ZERO && c30 != FULLREAD_C30_M10_DEFAULT;
    let ok = c30_real && fp_real;
    if ok {
        OWN_STEPPER_MOUNT_C30.store(c30, Ordering::SeqCst);
        OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_OK, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "own-load-feed: parser 0x{:x}(slot={want_slot}) ret={pret} fed_bytes=0x{fed:x} c30 0x{c30_before:x}->0x{c30:x} c30_real={c30_real} ac0={ac0} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) ok={ok} (read-only deserialize; NO SetState5, NO save write)",
        base + DESERIALIZE_SLOT_RVA
    ));
    ok
}

// FD4-IO residency for the menu-free switch reload (bd er-effects-rs-9fmm, 2026-07-19) is now DEFAULT
// behavior in own_load_switch_reload_fire (the boot native-fullread SUBMIT -> DRAIN(b80==RESIDENT) ->
// COMMIT sequence), replacing the old resource-less one-shot. No marker/env gate.

/// Phase machine state for the reload FD4-IO SUBMIT/DRAIN (own_load_switch_reload_fire), persisted
/// across the caller's per-frame retries. 0=IDLE (do SUBMIT once), 1=DRAIN (tick until b80==3),
/// 2=COMMIT (fall through to feed+continue_confirm).
pub(crate) use er_telemetry::counters::SWITCH_RELOAD_FD4IO_PHASE;
pub(crate) use er_telemetry::counters::SWITCH_RELOAD_FD4IO_DRAIN_WAITS;
pub(crate) use er_telemetry::counters::SWITCH_RELOAD_FD4IO_COMMITTED;
const SWITCH_RELOAD_FD4IO_IDLE: usize = 0;
const SWITCH_RELOAD_FD4IO_DRAIN: usize = 1;
const SWITCH_RELOAD_FD4IO_COMMIT: usize = 2;
/// Bound the reload drain far below the boot's FULLREAD_DRAIN_MAX (1200): the b80 2->3 save-file read
/// residency is fast (~17 ticks at boot); if it does not resident within this many frames the read is
/// not draining at the clean-title timing -> fall through to COMMIT without residency (fail-soft to the
/// old behavior) rather than hang the switch.
const SWITCH_RELOAD_FD4IO_DRAIN_MAX: usize = 600;

/// Reset the switch-reload FD4-IO phase machine so a NEW switch re-runs SUBMIT -> DRAIN -> COMMIT.
/// Without this the one-shot stays claimed after the FIRST switch (PHASE stuck at COMMIT +
/// SWITCH_RELOAD_FD4IO_COMMITTED=1), so the SECOND switch's own_load_switch_reload_fire hits the
/// already-committed guard and returns immediately WITHOUT loading -> the 2nd reload (load3) never
/// initiates and the game sits at a clean/PRESS-ANY-BUTTON title (run 110005: switch #1 loaded load2
/// via SUBMIT/DRAIN/COMMIT; switch #2 armed + tore the world down but emitted NO reload-fd4io SUBMIT,
/// so load3 stalled at bar step 1). switch_slot_arm_programmatic calls this on every switch arm so each
/// switch gets a fresh phase machine.
pub(crate) fn reset_switch_reload_fd4io_phase() {
    SWITCH_RELOAD_FD4IO_PHASE.store(SWITCH_RELOAD_FD4IO_IDLE, Ordering::SeqCst);
    SWITCH_RELOAD_FD4IO_COMMITTED.store(0, Ordering::SeqCst);
    SWITCH_RELOAD_FD4IO_DRAIN_WAITS.store(0, Ordering::SeqCst);
}

/// SUBMIT the native full-save-read for `picked` so the FD4 IO worker pool loads it resident, exactly
/// as the boot native-fullread SUBMIT phase (slot_resolution.rs). Mirrors its calls/RVAs. Sets
/// GameMan+0xb80=2 (the deserialize arm); the DRAIN tick then advances it to RESIDENT(3).
unsafe fn own_load_fd4io_submit(base: usize, gm: usize, picked: i32) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Mark the slot occupied so the native save-load gate accepts it (idempotent, no other effect).
    let gdm = game_data_man_ptr_or_null();
    let summary = if gdm != null {
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(null)
    } else {
        null
    };
    if summary != null {
        let mark: unsafe extern "system" fn(usize, i32) -> u8 =
            unsafe { std::mem::transmute(base + PROFILE_MARK_SLOT_USED_RVA) };
        let _ = unsafe { mark(summary, picked) };
    }
    // Resolve OUR slot + submit the full read (type-0xa; sets b80=2).
    unsafe { *((gm + GAME_MAN_SLOT_SELECT_B78_OFFSET) as *mut i32) = picked };
    let set_save_slot: unsafe extern "system" fn(i32) =
        unsafe { std::mem::transmute(base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
    unsafe { set_save_slot(picked) };
    let submit: unsafe extern "system" fn(i32) -> i32 =
        unsafe { std::mem::transmute(base + B80_FULL_LOAD_INITIATOR_RVA) };
    let sret = unsafe { submit(picked) };
    let b80 = unsafe { safe_read_i32(gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) }.unwrap_or(-1);
    append_autoload_debug(format_args!(
        "reload-fd4io: SUBMIT slot={picked} submit 0x{:x} ret={sret} b80={b80} -> DRAIN (replicating boot native-fullread residency before feed+continue_confirm)",
        base + B80_FULL_LOAD_INITIATOR_RVA
    ));
}

/// One DRAIN tick: pump the b80 IO lane + poll (exact boot native-fullread calls) and return the
/// current GameMan+0xb80 so the caller can detect RESIDENT(3).
unsafe fn own_load_fd4io_drain_tick(base: usize, gm: usize) -> i32 {
    let lane: unsafe extern "system" fn() -> i32 =
        unsafe { std::mem::transmute(base + B80_LANE1_DRIVER_RVA) };
    let _ = unsafe { lane() };
    let poll: unsafe extern "system" fn(u8, u8) -> i32 =
        unsafe { std::mem::transmute(base + B80_POLL_RVA) };
    let _ = unsafe { poll(FULLREAD_POLL_ARG, FULLREAD_POLL_ARG) };
    unsafe { safe_read_i32(gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) }.unwrap_or(-1)
}

/// MENU-FREE clean-title reload of the PICKED slot for a genuine System->Quit->Load-Profile switch.
/// The warm-rebuilt TitleTopDialog never reaches Loop post-return-title (press-start SceneObjProxy at
/// dialog+0xb78 unbound), so the title accept-byte/open-menu path deadlocks; native_fullread_tick also
/// stands down for a switch. Drive the picked slot through the same native-ownership commit the boot
/// autoload uses, exactly like the (now-dead) native-fullread DESER switch_feed_case
/// (slot_resolution.rs:275-296): reset the gaitem singleton -> feed the picked slot's on-disk bytes
/// through the native parser (real c30 + PGD) -> latch FRESH_DESER_DONE -> native continue_confirm
/// (intercepted by system_quit_continue_confirm_hook -> SetState5 streams the world + performs the
/// switch cleanup). ONE-SHOT per switch (SYSTEM_QUIT_SWITCH_MENU_FREE_RELOAD_FIRED). Returns true only
/// when it fired continue_confirm; false = "not yet / could not" (nothing consumed unless the one-shot
/// was legitimately claimed) and the caller keeps waiting. Caller MUST have proven the old world is
/// torn down (player absent) so the gaitem reset + deserialize never touch a live world.
/// See bd live-switch-teardown-fixed-now-menu-open-stall-2026-07-18 + the RE workflow.
pub(crate) unsafe fn own_load_switch_reload_fire(
    base: usize,
    gm: usize,
    owner: usize,
    picked: i32,
    n: u64,
) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // (a) Validate the title owner FIRST -- it flickers during the warm rebuild. No state consumed:
    // a bad-owner frame returns false and the caller retries next frame. Must be a live owner with the
    // new-game flag clear (continue_confirm's LOAD branch; nonzero = NewGame path / mid-rebuild).
    if owner == null {
        return false;
    }
    let new_game_flag = match unsafe { safe_read_usize(owner + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET) }
    {
        Some(v) => v as u8,
        None => return false,
    };
    if new_game_flag != FULLREAD_OWNER_NEW_GAME_OK {
        return false;
    }
    // (b) FD4-IO residency phase machine (DEFAULT behavior -- no marker/env toggle; bd er-effects-rs-9fmm):
    // SUBMIT the full read, DRAIN until GameMan+0xb80==RESIDENT(3), THEN fall through to
    // feed+continue_confirm -- so the reload's streamed world has the resources natively resident (the
    // boot path's behavior) instead of entering resource-less and reverting to title. Owner is already
    // validated, so a flickering frame never burns the one-shot (claimed by SWITCH_RELOAD_FD4IO_COMMITTED).
    {
        let phase = SWITCH_RELOAD_FD4IO_PHASE.load(Ordering::SeqCst);
        if phase == SWITCH_RELOAD_FD4IO_IDLE {
            if SWITCH_RELOAD_FD4IO_PHASE
                .compare_exchange(
                    SWITCH_RELOAD_FD4IO_IDLE,
                    SWITCH_RELOAD_FD4IO_DRAIN,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            {
                SWITCH_RELOAD_FD4IO_DRAIN_WAITS.store(0, Ordering::SeqCst);
                unsafe { own_load_fd4io_submit(base, gm, picked) };
            }
            return false;
        }
        if phase == SWITCH_RELOAD_FD4IO_DRAIN {
            let b80 = unsafe { own_load_fd4io_drain_tick(base, gm) };
            let w = SWITCH_RELOAD_FD4IO_DRAIN_WAITS.fetch_add(1, Ordering::SeqCst);
            let resident = b80 == FULLREAD_B80_RESIDENT;
            if resident || w >= SWITCH_RELOAD_FD4IO_DRAIN_MAX {
                // Do NOT disarm b78 here. Unlike the boot native-fullread (which disarms on commit),
                // the System->Quit SWITCH path MUST keep GameMan+0xb78 armed (= picked slot) through
                // SetState5/MoveMap finalize: it is the warp target that MoveMapStep finalize case 8
                // consumes to warp the character and autoclear warpRequested before advancing mms18
                // (system_quit_repro_guards.rs:1720-1754). Clearing it early leaves the load with no
                // warp target -> warp_requested stuck at 1 and STEP_MoveMap self-loops at 18 (observed
                // in the b78-disarm build: world resident, real char, but mms18 next=18/done50=0
                // warp=1 forever). The continue_confirm hook's post-resident proof point clears b78.
                append_autoload_debug(format_args!(
                    "reload-fd4io: DRAIN done b80={b80} waits={w} resident={resident}{} -> COMMIT (feed+continue_confirm); b78 kept armed (warp target) through finalize",
                    if resident { "" } else { " (TIMEOUT -- committing without residency, fail-soft to old behavior)" }
                ));
                SWITCH_RELOAD_FD4IO_PHASE.store(SWITCH_RELOAD_FD4IO_COMMIT, Ordering::SeqCst);
                // fall through to feed+continue_confirm this frame
            } else {
                return false; // keep draining
            }
        }
        // phase == COMMIT (reached this frame or a prior one): commit exactly once.
        if SWITCH_RELOAD_FD4IO_COMMITTED
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
    }
    // (c) Defuse the CSGaitemImp free-queue exhaustion AV (live 0x67141a): char#1's leaked gaitem
    // entries still populate the gaitem singleton at the clean title (the lightweight return-title
    // chain skips the native inventory teardown). Safe now because the old world is torn down.
    let _ = unsafe { own_load_reset_gaitem_singleton(base) };
    // (d) Feed the picked slot's on-disk bytes through the native parser -> GameMan+0xc30 becomes the
    // picked character's REAL map + a real PGD fingerprint. No FD4 IO SUBMIT/DRAIN, no b80==3 needed.
    if !unsafe { own_load_feed_deserialize(base, gm, picked) } {
        append_autoload_debug(format_args!(
            "own-load-switch-reload: feed-deserialize of picked slot {picked} FAILED -- NOT firing continue_confirm; switch fails closed (one-shot claimed, no re-attempt)"
        ));
        return false;
    }
    // (e) Latch native_slot_proven BEFORE firing: the continue_confirm hook reads FRESH_DESER_DONE==1
    // to take the forward->SetState5 path (not the no-proof forward) and to prevent any double-feed.
    SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.store(1, Ordering::SeqCst);
    // (f) Re-read the freshly-mounted c30 + fingerprint and fire the GUARDED native continue_confirm
    // (own_load_continue_fire re-guards c30_real && fp_real && owner+0x284==0 internally -- the only
    // save-writing SetState5 is behind that hard guard).
    let c30 = unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(GAME_MAN_C30_UNSET);
    let c30_real = c30 != GAME_MAN_C30_UNSET && c30 != 0 && c30 != FULLREAD_C30_M10_DEFAULT;
    let (fp_real, fp_level, _nl) = unsafe { char_fingerprint(base) };
    append_autoload_debug(format_args!(
        "own-load-switch-reload: picked slot {picked} mounted (c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}); firing native continue_confirm owner=0x{owner:x} (hook forwards -> SetState5 streams + performs switch cleanup) presses=0 (#{n})"
    ));
    unsafe { own_load_continue_fire(base, owner, c30, c30_real, fp_real, fp_level, n) };
    true
}

/// SAVE-SAFE verify-only OWN-LOAD buffer-feed drive (one-shot, phased). Reads the .sl2 from disk,
/// slices slot `want_slot`'s plaintext body, installs+arms the gated 0x67b100 hook, calls the native
/// parser 0x67b290(slot) in-process so it parses OUR body, then reads back GameMan+0xc30 + the
/// PlayerGameData fingerprint. NO SetState5, NO autosave, NO continue_confirm. Records presses==0.
pub(crate) unsafe fn own_load_drive(base: usize, gm: usize, owner: usize, want_slot: i32, n: u64) {
    const PHASE_INIT: usize = 0;
    const PHASE_DONE: usize = 1;
    const C30_ZERO: i32 = 0;
    static OWN_LOAD_PHASE: AtomicUsize = AtomicUsize::new(PHASE_INIT);
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let phase = OWN_LOAD_PHASE.load(Ordering::SeqCst);
    // Publish phase+1 so the readiness watcher tears down on terminal completion (PHASE_DONE -> 2).
    OWN_LOAD_PHASE_PUB.store(phase + 1, Ordering::SeqCst);
    if phase != PHASE_INIT {
        return;
    }
    if gm == null {
        return;
    }
    if want_slot < OWN_STEPPER_SLOT_ZERO {
        append_autoload_debug(format_args!(
            "own-load: needs an EXPLICIT slot (slot={want_slot}); set slot=N in er-effects-autoload.txt -- ABORT (no-write)"
        ));
        OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        return;
    }
    // (1) Read + slice the plaintext slot body. er_save_loader::bnd4 is the only glue: the engine's
    // read path is FSM-gated, so OWN-LOAD must hand it the buffer itself (bd reuse-native-fns).
    let Some(sl2_bytes) = (unsafe { own_load_read_sl2_bytes(base) }) else {
        OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        return;
    };
    let body: &[u8] = match er_save_loader::bnd4::slot_body(&sl2_bytes, want_slot as usize) {
        Ok(b) => b,
        Err(e) => {
            append_autoload_debug(format_args!(
                "own-load: slot_body(slot={want_slot}) failed: {e:?} -- ABORT (no-write)"
            ));
            OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
            return;
        }
    };
    // Leak the sliced body so it outlives this frame and stays valid for the detour to memcpy. One
    // copy of the (small fraction of the) save -- never the whole file -- kept for the session.
    let leaked: &'static [u8] = Box::leak(body.to_vec().into_boxed_slice());
    OWN_LOAD_BODY_PTR.store(leaked.as_ptr() as usize, Ordering::SeqCst);
    OWN_LOAD_BODY_LEN.store(leaked.len(), Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "own-load: sliced slot {want_slot} body len=0x{:x} (expected 0x{:x}) -> install+arm gate, call native parser 0x{:x}",
        leaked.len(),
        er_save_loader::bnd4::SLOT_BODY_LEN,
        base + DESERIALIZE_SLOT_RVA
    ));
    // (2) Install the gated 0x67b100 detour (harmless pass-through until armed).
    if !install_own_load_hook() {
        append_autoload_debug(format_args!(
            "own-load: hook install failed -- ABORT (no-write)"
        ));
        OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        return;
    }
    let c30_before = unsafe { *((gm + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) };
    // (3) Set the gate, call native 0x67b290(slot) in-process, clear the gate. 0x67b290 does NOT
    // re-check b80 after the read (static-confirmed), so our al=1 + body flow into the native parse.
    OWN_LOAD_GATE.store(true, Ordering::SeqCst);
    let parser: unsafe extern "system" fn(i32) -> i32 =
        unsafe { std::mem::transmute(base + DESERIALIZE_SLOT_RVA) };
    let pret = unsafe { parser(want_slot) };
    OWN_LOAD_GATE.store(false, Ordering::SeqCst);
    let fed = OWN_LOAD_FED_BYTES.load(Ordering::SeqCst);
    // (4) VERIFY (read-back only): GameMan+0xc30 (map id) + the PlayerGameData char fingerprint.
    let c30 = unsafe { *((gm + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) };
    let ac0 = unsafe { *((gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
    let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
    let c30_real = c30 != GAME_MAN_C30_UNSET && c30 != C30_ZERO && c30 != FULLREAD_C30_M10_DEFAULT;
    if c30_real && fp_real {
        OWN_STEPPER_MOUNT_C30.store(c30, Ordering::SeqCst);
        OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_OK, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "own-load: VERIFY parser 0x{:x}(slot={want_slot}) ret={pret} fed_bytes=0x{fed:x} c30 0x{c30_before:x}->0x{c30:x} c30_real={c30_real} ac0={ac0} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) presses=0 (NO SetState5/NO save write)",
        base + DESERIALIZE_SLOT_RVA
    ));
    unsafe { dump_load_correctness(base, n) };
    // OWNER DIAGNOSTIC (er-effects-rs-mr2, save-safe pure reads): the prior continue crash used the
    // WRONG owner (*(GameDataMan+0x8)). Log EVERY continue_confirm owner candidate + each one's
    // +0x284 (new-game flag) byte so a VERIFY-ONLY run reveals which is the SetState-able title
    // owner BEFORE we ever fire continue_confirm. This is independent of the gated continue step.
    //   title  = the threaded SetState-able title owner the caller validated (own_stepper_idx10),
    //   recipe = *(base + CONTINUE_MANAGER_GLOBAL_RVA + 8)  (the native-fullread COMMIT recipe's literal),
    //   mgr_vt = *(base + CONTINUE_MANAGER_GLOBAL_RVA)      (the manager object's vtable ptr),
    //   gdm8   = *(GameDataMan + 0x8)                       (the prior crash owner).
    let read284 = |obj: usize| -> u8 {
        if obj == null {
            0
        } else {
            unsafe { safe_read_usize(obj + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET) }
                .map(|v| v as u8)
                .unwrap_or(0)
        }
    };
    let recipe_owner = unsafe {
        safe_read_usize(base + CONTINUE_MANAGER_GLOBAL_RVA + FULLREAD_OWNER_GDM_08_OFFSET)
    }
    .unwrap_or(null);
    let manager_vtable =
        unsafe { safe_read_usize(base + CONTINUE_MANAGER_GLOBAL_RVA) }.unwrap_or(null);
    let game_data_man = game_data_man_ptr_or_null();
    let gdm8 = if game_data_man == null {
        null
    } else {
        unsafe { safe_read_usize(game_data_man + FULLREAD_OWNER_GDM_08_OFFSET) }.unwrap_or(null)
    };
    append_autoload_debug(format_args!(
        "own-load-OWNER-DIAG: title=0x{owner:x} (+284={}) recipe=0x{recipe_owner:x} (+284={}) mgr_vt=0x{manager_vtable:x} gdm8=0x{gdm8:x} (+284={})",
        read284(owner),
        read284(recipe_owner),
        read284(gdm8)
    ));
    // (5) FINAL STEP. Two mutually-exclusive armed levers (both OFF by default; verify-only is the
    // default). The LoadGame-JOB INSTALL lever (own_load_install_job) takes precedence: it is the
    // SAVE-SAFE, NON-SetState5 path (build + install the LoadGame MenuJob into owner+0x130 so
    // STEP_MenuJobWait ticks it -> self-build -> deser -> world stream; no SetState5, no save write).
    // Only if it is NOT armed do we fall back to the legacy GUARDED continue_confirm/SetState5 lever
    // (own_load_continue), which is SAVE-WRITING (SetState5 autosaves) behind the hard c30/fp guard.
    // PATH B (own_load_pump) takes precedence: BUILD the LoadGame job with REAL mss-derived ctx, then
    // privately pump its Run every frame from the recurring game task to completion (deser -> m28 stream)
    // and drive the transition on Success. No owner+0x130 install, no queue, no dialog -- the proven
    // menu-free "own the load". SAVE-SAFE at build (only the final SetState5 transition writes, gated).
    if own_load_pump_enabled() {
        unsafe { own_load_pump_fire(base, owner, c30, c30_real, fp_real, fp_level, n) };
    } else if own_load_install_job_enabled() {
        unsafe { own_load_install_job_fire(base, owner, c30, c30_real, fp_real, fp_level, n) };
    } else if own_load_continue_enabled() {
        unsafe { own_load_continue_fire(base, owner, c30, c30_real, fp_real, fp_level, n) };
    }
    OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
    OWN_LOAD_PHASE_PUB.store(PHASE_DONE + 1, Ordering::SeqCst);
}

/// OWN-LOAD FINAL STEP (er-effects-rs-mr2): after the PROVEN verify-only parse mounted a REAL c30 +
/// real character, fire the GUARDED native `continue_confirm` 0x140b0e180 -> `SetState5` 0x140b0d960
/// to stream the character into the PLAYABLE world. `continue_confirm` reads owner = [rcx+8] off
/// the shim, reads GameMan+0xc30 (already REAL from our parse) into owner+0xbc, then
/// SetState(owner, 5) -> the per-frame title-flow step machine streams the world.
///
/// OWNER (er-effects-rs-mr2 fix): the owner MUST be the SetState-able TITLE owner threaded in from
/// `own_stepper_idx10` (the validated title-flow object), NOT *(GameDataMan+0x8). The prior crash
/// passed *(GameDataMan+0x8) (a DIFFERENT object) into continue_confirm and crashed inside
/// SetState5. The OWNER DIAGNOSTIC in the verify path logs all candidates for cross-checking.
///
/// SAVE-SAFETY ABSOLUTE (SetState5 AUTOSAVES). HARD GUARD before firing -- ABORT with a logged
/// no-write if ANY fails:
///   * `c30_real` (c30 != 0xa010000 m10-default AND != 0xffffffff unset AND != 0): same flag the
///     verify path computed -- never fire SetState5 on an unverified/default c30 (the prior crash
///     cause -- real char streamed to the wrong map then autosaved over).
///   * `fp_real`: the PlayerGameData char fingerprint is real (level/stats non-default).
///   * `title_owner` non-null AND title_owner+0x284 (new-game flag) == 0 (continue_confirm's LOAD
///     branch; non-zero would take the NewGame path -- fail closed).
/// Keeps `simulated_button_presses_total = 0`: this is a pure in-process native call, no input.
unsafe fn own_load_continue_fire(
    base: usize,
    title_owner: usize,
    c30: i32,
    c30_real: bool,
    fp_real: bool,
    fp_level: u32,
    n: u64,
) {
    // CALLER-TRACE DIAG (2026-07-23, bd trace own_load arming): log the FULL runtime caller chain each
    // time the continue actually fires, so the ACTUAL entry/arming path is captured from evidence (static
    // tracing was repeatedly wrong -- own_load fired in run71 despite no autoload file + DIAG_NO_AUTOLOAD).
    append_autoload_debug(format_args!(
        "OWN_LOAD_CONTINUE_FIRE ENTRY c30_real={c30_real} fp_real={fp_real} own_load_continue_enabled={} CALLERS: {}",
        own_load_continue_enabled(),
        crate::crashlog::trace_callers_summary(),
    ));
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Hard c30 + fingerprint guard (absolute save-safety backstop). NOTE: unlike the native-fullread
    // COMMIT path (which needs a level>=10 floor to reject the level-9 NEW-GAME PREVIEW), OWN-LOAD has
    // a STRONGER per-slot signal: `c30_real` means GameMan+0xc30 became the slot's REAL map
    // (0x1c000000 etc.), NOT the new-game default 0xa010000 -- so a real save is proven directly.
    // `fp_real` already requires level>=1 AND a non-empty name (see char_fingerprint), so it admits
    // legitimate LOW-LEVEL real characters (e.g. a level-7 Hero-class save) that a >=10 floor would
    // wrongly reject. c30_real + fp_real is the correct, save-safe gate here.
    if !(c30_real && fp_real) {
        append_autoload_debug(format_args!(
            "own-load-continue: GUARD FAIL (c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}) -- NO continue_confirm, NO SetState5, NO save write -> ABORT (save-safe)"
        ));
        return;
    }
    // OWNER = the SetState-able TITLE owner threaded in from own_stepper_idx10 (NOT *(GameDataMan+0x8),
    // which caused the prior crash). It is the validated title-flow object the DLL already SetState's.
    if title_owner == null {
        append_autoload_debug(format_args!(
            "own-load-continue: ABORT -- threaded title_owner is null -> no write"
        ));
        return;
    }
    let new_game_flag = match unsafe {
        safe_read_usize(title_owner + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET)
    } {
        Some(v) => v as u8,
        None => {
            append_autoload_debug(format_args!(
                "own-load-continue: ABORT -- title_owner+0x284 (new-game flag) unreadable (title_owner=0x{title_owner:x}) -> no write"
            ));
            return;
        }
    };
    if new_game_flag != FULLREAD_OWNER_NEW_GAME_OK {
        append_autoload_debug(format_args!(
            "own-load-continue: ABORT -- title_owner+0x284={new_game_flag} != 0 (continue_confirm LOAD branch requires the new-game flag clear) -> no write"
        ));
        return;
    }
    // GUARD PASSED. Build the {[OWNER_IDX]=title_owner} shim and fire the native continue_confirm.
    let shim = &raw mut OWN_STEPPER_SHIM;
    unsafe { (*shim)[OWN_STEPPER_SHIM_OWNER_IDX] = title_owner };
    let shim_ptr = shim as usize;
    let confirm: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + CONTINUE_CONFIRM_RVA) };
    append_autoload_debug(format_args!(
        "own-load-continue: *** GUARD PASS -- COMMIT continue_confirm 0x{:x}(shim=0x{shim_ptr:x} title_owner=0x{title_owner:x}) c30=0x{c30:x} level={fp_level} title_owner+0x284=0 -- continue_confirm fires SetState5 internally (AUTOSAVES) presses=0 ***",
        base + CONTINUE_CONFIRM_RVA
    ));
    timeline_event(
        "T_own_load_continue",
        n,
        format_args!("c30=0x{c30:x} level={fp_level}"),
    );
    unsafe { confirm(shim_ptr) };
    // Cache the pointers the RECURRING world-stream observer needs, then arm it. own_stepper_idx10 (a
    // TITLE-PHASE task) STOPS ticking once SetState5 starts this transition, so the title `owner` and
    // its InGameStep (owner+0x2e8) will no longer be threaded in. Snapshot them HERE (InGameStep was
    // already non-null at frame 0) so the recurring game task can keep walking owner->InGameStep->
    // MoveMapStep through the whole loading screen. (own-load-stream-observer-must-be-recurring-task-2026-06-22)
    OWN_LOAD_OWNER_CACHED.store(title_owner, Ordering::SeqCst);
    let ingame_cached = unsafe { safe_read_usize(title_owner + TITLE_OWNER_JOB_OFFSET) }
        .filter(|&v| v != null)
        .unwrap_or(0);
    OWN_LOAD_INGAMESTEP_CACHED.store(ingame_cached, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_FIRED.store(true, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "own-load-continue: continue_confirm returned -- native pump now streams the real world (#{n}); recurring world-stream observer ARMED (owner=0x{title_owner:x} ingame=0x{ingame_cached:x}) -> DONE"
    ));
}

/// Snapshot of the `owner+0x130` MenuJob slot for the before/after vtable-flip + self-build evidence.
/// All pure fault-tolerant reads -- never changes load behavior.
fn own_load_install_job_slot_snapshot(slot_addr: usize) -> (usize, usize, usize, u8, usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // The job pointer currently in the slot.
    let job = unsafe { safe_read_usize(slot_addr) }.unwrap_or(null);
    if job == null {
        return (null, null, null, 0, null);
    }
    let vtable = unsafe { safe_read_usize(job) }.unwrap_or(null);
    let inner_seq = unsafe { safe_read_usize(job + MENUJOB_INNER_SEQ_70_OFFSET) }.unwrap_or(null);
    let built_flag = unsafe { safe_read_usize(job + MENUJOB_BUILT_FLAG_68_OFFSET) }
        .map(|v| v as u8)
        .unwrap_or(0);
    let current_job_index =
        unsafe { safe_read_usize(job + MENUJOB_CURRENT_JOB_INDEX_10_OFFSET) }.unwrap_or(null);
    (job, vtable, inner_seq, built_flag, current_job_index)
}

/// OWN-LOAD FINAL STEP -- LoadGame-JOB INSTALL lever (`own_load_install_job`). The SAVE-SAFE,
/// NON-SetState5 alternative to `own_load_continue_fire`: after the PROVEN verify-only parse mounted a
/// REAL c30 + real character, BUILD the native LoadGame `CS::MenuJobWithContext<LoadJobContext>` and
/// INSTALL it into the title owner's `+0x130` MenuJob slot, replacing the idle `IfElseJob`.
/// `CS::TitleStep::STEP_MenuJobWait` already ticks `ExecuteMenuJob(&owner->+0x130)` every frame, so the
/// installed job then self-builds (its `Run` builds the inner FixOrderJobSequence on the first tick:
/// `+0x68`/`+0x70` flip), deserializes the save, and streams the world -- WITHOUT `SetState5`.
///
/// SAVE-SAFETY ABSOLUTE: NO `SetState5`, NO autosave, NO save write. The BUILD factory only allocates +
/// copies a template; the first-tick deser step (`FUN_14082c330`) only READS the save
/// (`AllocateAligned` -> read -> `SetSaveSlot` -> decrypt -> `ReadBytes` -> dealloc) up to world-stream.
/// Static-verified against the runtime dump. Same hard c30/fp guard as the continue lever is kept as a
/// belt-and-braces precondition even though no write occurs. Keeps `simulated_button_presses_total = 0`.
///
/// ARG SOURCING (static RE, 2026-06-22): the BUILD factory `FUN_140826510(out, ctx_parent, slot,
/// owner_ctx)` needs only `out` (our local) + `slot` (the int slot) for the deser/map self-build; the
/// `ctx_parent`/`owner_ctx` args are the OUTER profile-selection UI context, stored as lambda captures
/// whose every build-path deref is null-guarded -- so we pass them as 0. RESIDUAL RISK: if the engine's
/// `EnableProfileSelection` release flag is set AND the outer sequence ticks the profile-selection
/// sub-job, a captured-null deref could fault -- watch the install-fire log for that. The two native
/// calls are wrapped in `catch_unwind` (catches a Rust-unwinding panic; a hardware AV is NOT caught).
unsafe fn own_load_install_job_fire(
    base: usize,
    title_owner: usize,
    c30: i32,
    c30_real: bool,
    fp_real: bool,
    fp_level: u32,
    n: u64,
) {
    const NO_CTX: usize = 0;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Belt-and-braces guard (no write occurs, but never act on an unverified parse).
    if !(c30_real && fp_real) {
        append_autoload_debug(format_args!(
            "own-load-install-job: GUARD FAIL (c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}) -- NO build/install -> ABORT (save-safe)"
        ));
        return;
    }
    if title_owner == null {
        append_autoload_debug(format_args!(
            "own-load-install-job: ABORT -- threaded title_owner is null -> no install (save-safe)"
        ));
        return;
    }
    let want_slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst) as i32;
    let slot_addr = title_owner + TITLE_OWNER_MENUJOB_SLOT_130_OFFSET;
    // BEFORE: dump owner+0x130 (the idle IfElseJob it replaces). Pure reads.
    let (b_job, b_vt, b_seq, b_built, b_idx) = own_load_install_job_slot_snapshot(slot_addr);
    append_autoload_debug(format_args!(
        "own-load-install-job: BEFORE slot=owner+0x130=0x{slot_addr:x} job=0x{b_job:x} vt=0x{b_vt:x} (expect IfElseJob dump 0x{:x}) +0x68_built={b_built} +0x70_seq=0x{b_seq:x} +0x10_idx=0x{b_idx:x} -- BUILD 0x{:x}(out,ctx=0,slot={want_slot},owner_ctx=0) presses=0",
        MENUJOB_IFELSE_VTABLE_DUMP_VA,
        base + LOADGAME_JOB_BUILD_RVA,
    ));
    // (a) BUILD the LoadGame MenuJobWithContext into a local DLRefCountPtr (the factory writes the job
    //     ptr into *out with refcount 1). Win64 fastcall (out, ctx_parent, save_slot, owner_ctx).
    // Justify the transmute: LOADGAME_JOB_BUILD_RVA is the prologue-grounded live entry of the menu-heap
    // LoadGame-job factory; the signature matches the static decompile of FUN_140826510.
    let build: unsafe extern "system" fn(*mut usize, usize, i32, usize) -> *mut usize =
        unsafe { std::mem::transmute(base + LOADGAME_JOB_BUILD_RVA) };
    let mut built_job: usize = 0;
    let build_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        build(&raw mut built_job, NO_CTX, want_slot, NO_CTX)
    }));
    match build_ret {
        Ok(_) => {}
        Err(_) => {
            append_autoload_debug(format_args!(
                "own-load-install-job: BUILD PANICKED (caught) -- NO install -> ABORT (save-safe)"
            ));
            return;
        }
    }
    if built_job == null || built_job == 0 {
        append_autoload_debug(format_args!(
            "own-load-install-job: BUILD returned a NULL job (built_job=0x{built_job:x}) -- NO install -> ABORT (save-safe)"
        ));
        return;
    }
    let built_vt = unsafe { safe_read_usize(built_job) }.unwrap_or(null);
    append_autoload_debug(format_args!(
        "own-load-install-job: BUILD OK job=0x{built_job:x} vt=0x{built_vt:x} (expect LoadGame dump 0x{:x}) -- INSTALL via assign 0x{:x}(slot=0x{slot_addr:x}, src=&job)",
        MENUJOB_LOADGAME_VTABLE_DUMP_VA,
        base + MENUJOB_ASSIGN_RVA,
    ));
    // (b) APPEND our built job into the owner+0x130 MenuJobQueue via PushBackJob (NOT a slot-overwrite).
    //     owner+0x130 is a CS::MenuJobQueue (active job +0x130, ring +0x138, count +0x178). The prior
    //     move-assign overwrite ORPHANED the title IfElseJob's sibling CS::MenuWindowJobs -> AV at
    //     CS::DLFixedVector::push_back 0x140733fea. PushBackJob(queue_base=&owner+0x130, src=&built_job)
    //     appends behind the still-active IfElseJob (no tear, AtomicIncrements the job, does not zero
    //     src); STEP_MenuJobWait's ExecuteMenuJob then pops + ticks our queued job.
    // Justify the transmute: MENUJOB_PUSHBACK_RVA is the prologue-grounded live entry of
    // CS::MenuJobQueue::PushBackJob (FUN_1407a9254).
    let queue_count_before =
        unsafe { safe_read_i32(slot_addr + MENUJOB_QUEUE_COUNT_178_OFFSET) }.unwrap_or(-1);
    let pushback: unsafe extern "system" fn(*mut usize, *mut usize) -> *mut usize =
        unsafe { std::mem::transmute(base + MENUJOB_PUSHBACK_RVA) };
    let install_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        pushback(slot_addr as *mut usize, &raw mut built_job)
    }));
    if install_ret.is_err() {
        append_autoload_debug(format_args!(
            "own-load-install-job: PUSHBACK PANICKED (caught) after build (job=0x{built_job:x}) -> ABORT"
        ));
        return;
    }
    // AFTER: the active job at owner+0x130 should be UNCHANGED (still the IfElseJob) -- our job is in the
    // ring; the queue count at +0x178 should have grown by 1. Pure reads.
    let (a_job, a_vt, a_seq, a_built, a_idx) = own_load_install_job_slot_snapshot(slot_addr);
    let queue_count_after =
        unsafe { safe_read_i32(slot_addr + MENUJOB_QUEUE_COUNT_178_OFFSET) }.unwrap_or(-1);
    OWN_LOAD_INSTALL_JOB_FIRED.fetch_add(1, Ordering::SeqCst);
    // Cache the owner so the recurring world-stream observer keeps logging through the loading screen
    // (own_stepper_idx10 stops once the title transitions). Mirror own_load_continue_fire's caching.
    OWN_LOAD_OWNER_CACHED.store(title_owner, Ordering::SeqCst);
    let ingame_cached = unsafe { safe_read_usize(title_owner + TITLE_OWNER_JOB_OFFSET) }
        .filter(|&v| v != null)
        .unwrap_or(0);
    OWN_LOAD_INGAMESTEP_CACHED.store(ingame_cached, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_FIRED.store(true, Ordering::SeqCst);
    timeline_event(
        "T_own_load_install_job",
        n,
        format_args!("c30=0x{c30:x} level={fp_level}"),
    );
    append_autoload_debug(format_args!(
        "own-load-install-job: *** APPENDED -- AFTER queue=owner+0x130=0x{slot_addr:x} active_job=0x{a_job:x} vt=0x{a_vt:x} (active stays IfElseJob dump 0x{:x}, NOT torn) active+0x68={a_built} +0x70=0x{a_seq:x} +0x10_idx=0x{a_idx:x} | queue_count {queue_count_before}->{queue_count_after} (expect +1) | our_job=0x{built_job:x} (LoadGame dump 0x{:x}) ingame=0x{ingame_cached:x} -- STEP_MenuJobWait pops+ticks queued job -> self-build -> deser -> world stream (NO SetState5/NO save write) presses=0 (#{n}) -> DONE ***",
        MENUJOB_IFELSE_VTABLE_DUMP_VA, MENUJOB_LOADGAME_VTABLE_DUMP_VA,
    ));
    let _ = (b_seq, b_idx, b_built, b_vt, b_job);
}

/// Resolve `mss = GameDataMan->menuSystemSaveLoad = *(*(base + GAME_DATA_MAN_GLOBAL_RVA) +
/// GAME_DATA_MAN_MENU_SAVELOAD_60_OFFSET)` (static-verified: `GetMenuSystemSaveLoad` 0x140256410 is
/// exactly `GLOBAL_GameDataMan->menuSystemSaveLoad`). Returns `None` (never `null`/`0`) on any
/// fault-tolerant read failure. Pure reads.
pub(crate) unsafe fn resolve_menu_system_save_load(base: usize) -> Option<usize> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let gdm = unsafe { safe_read_usize(base + GAME_DATA_MAN_GLOBAL_RVA) }
        .filter(|&v| v != null && v != 0)?;
    unsafe { safe_read_usize(gdm + GAME_DATA_MAN_MENU_SAVELOAD_60_OFFSET) }
        .filter(|&v| v != null && v != 0)
}

/// The "engine filled enough to drive our own load" gate -- distinct from "GameMan instance pointer
/// resolved" (`game_man_instance_resolved`), which flips true at BootPhase4, LONG before the load
/// machinery is usable. True iff GameDataMan + menuSystemSaveLoad (mss) resolve AND the TitleFlowContext
/// at `mss+0xa38` is a PLAUSIBLE heap pointer. The plausibility range matters: before the GameFlow
/// constructs the TitleFlowContext it reads back as uninitialized garbage (e.g. 0x8080808080808080),
/// which a `!= 0` check would wrongly accept -- then the LoadGame job's first `Run` derefs it and
/// access-violates (the ~25s AV observed when arming at the bare title). When this returns true, the
/// native LoadGame job (`own_load_pump_fire`) can be built + pumped without that crash. The bypass arms
/// its own-load on THIS, not on `game_man_instance_resolved`.
/// (loadgame-build-ctx-ready-precondition-2026-06-22)
pub(crate) unsafe fn loadgame_build_ctx_ready(base: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // CORRECTED (bd loadgame-owner-ctx-is-DIALOG-a38-not-mss-CORRECTION-2026-06-22): the buildable
    // TitleFlowContext is `*(CS::TitleTopDialog+0xa38)`, NOT `*(mss+0xa38)` (the mss reading was a red
    // herring -- r13 at the golden factory site is the dialog). Read it off the live dialog
    // (owner+0xe0, vtable-gated) via the cached title owner, so this arming signal matches exactly the
    // ctx `own_load_pump_fire` builds with.
    let owner = TITLE_OWNER_PTR.load(Ordering::SeqCst);
    if owner == null || owner == 0 {
        return false;
    }
    let dialog = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(0);
    if dialog == 0 {
        return false;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(0);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return false;
    }
    let ctx = unsafe { safe_read_usize(dialog + DIALOG_OWNER_CTX_A38_OFFSET) }.unwrap_or(0);
    if !(ctx > OWNER_CTX_MIN_PLAUSIBLE_PTR && ctx < OWNER_CTX_MAX_PLAUSIBLE_PTR) {
        return false;
    }
    // Native `FUN_14082d090` checks this singleton before comparing regulation versions; our readiness
    // predicate must not claim the title/load context is usable before the same singleton exists.
    let regulation_manager =
        unsafe { safe_read_usize(base + GLOBAL_CS_REGULATION_MANAGER_RVA) }.unwrap_or(0);
    regulation_manager != 0 && regulation_manager != null
}

/// PATH B "OWN THE LOAD" -- BUILD the LoadGame job with REAL mss-derived ctx, store its pointer for the
/// recurring per-frame private pump. The menu-free alternative to BOTH the owner+0x130 install (a
/// proven dead end) and the SetState5-only continue (reached the loading screen but never mounted m28).
///
/// We BUILD via `FUN_140826510(out, ctx_parent=mss+0x50, save_slot, owner_ctx=*(mss+0xa38))` -- the REAL
/// non-null ctx from the golden Continue trace (the prior ctx=0 build AV'd when the outer
/// profile-selection sub-job dereffed the captured null). We do NOT install the job anywhere (no
/// owner+0x130, no MenuJobQueue, no CSMenuMan dialog). Instead the recurring game task ticks its `Run`
/// privately every frame (see `own_load_pump_tick`) until it self-builds + deserializes + map-streams
/// (m28 mount) and reaches `state==Success`, then drives the title->ingame transition once.
///
/// SAVE-SAFETY ABSOLUTE: BUILD only allocates + copies a template (no save write); the first-tick deser
/// step (`FUN_14082c330`) only READS the save up to world-stream. NO SetState5 here. The same hard
/// c30/fp guard as the other levers is kept as a belt-and-braces precondition even though no write
/// occurs at build time. The transition (the only save-writing step) is separately gated in
/// `own_load_pump_tick`. Keeps `simulated_button_presses_total = 0`.
unsafe fn own_load_pump_fire(
    base: usize,
    title_owner: usize,
    c30: i32,
    c30_real: bool,
    fp_real: bool,
    fp_level: u32,
    n: u64,
) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Belt-and-braces guard (no write occurs at build, but never act on an unverified parse).
    if !(c30_real && fp_real) {
        append_autoload_debug(format_args!(
            "own-load-pump: GUARD FAIL (c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}) -- NO build -> ABORT (save-safe)"
        ));
        return;
    }
    if title_owner == null {
        append_autoload_debug(format_args!(
            "own-load-pump: ABORT -- threaded title_owner is null -> no build (save-safe)"
        ));
        return;
    }
    if OWN_LOAD_PUMP_JOB.load(Ordering::SeqCst) != 0 {
        // Already built+armed (own_load_drive is one-shot, but guard against a re-entrant fire).
        return;
    }
    // CORRECTED ctx source (bd loadgame-owner-ctx-is-DIALOG-a38-not-mss-CORRECTION-2026-06-22): the
    // LoadGame factory's owner_ctx (r9) and ctx_parent (rdx) come from the live CS::TitleTopDialog,
    // NOT from CSMenuSystemSaveLoad. The golden factory site reads `mov 0xa38(%r13),%r9` where r13 IS
    // the dialog (the prior mss+0xa38 reading misidentified r13 as mss and read back garbage -> the AV).
    // Locate the live dialog at owner+0xe0 (vtable-gated, same recipe as locate_live_loadgame_node).
    let dialog = unsafe { safe_read_usize(title_owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }
        .filter(|&v| v != null && v != 0)
        .unwrap_or(0);
    let dialog_vt = if dialog != 0 {
        unsafe { safe_read_usize(dialog) }.unwrap_or(0)
    } else {
        0
    };
    if dialog == 0 || dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "own-load-pump: ABORT -- live TitleTopDialog not up (owner+0x{:x}=0x{dialog:x} vt=0x{dialog_vt:x} want 0x{:x}) -> no build (save-safe)",
            TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return;
    }
    let ctx_parent = dialog + DIALOG_CTX_PARENT_50_OFFSET;
    // owner_ctx = *(dialog+0xa38) = CS::TitleFlowContext (written UNCONDITIONALLY by the dialog ctor
    // 0x1409a82d0, so it is valid at the settled press-any-button title -- unlike mss+0xa38 which read
    // back uninitialized garbage). FAIL CLOSED (no build) if it is not a plausible heap pointer:
    // passing NULL is exactly what AV'd before, and a real ctx is the whole point of the correction.
    let raw_owner_ctx =
        unsafe { safe_read_usize(dialog + DIALOG_OWNER_CTX_A38_OFFSET) }.unwrap_or(0);
    if !(raw_owner_ctx > OWNER_CTX_MIN_PLAUSIBLE_PTR && raw_owner_ctx < OWNER_CTX_MAX_PLAUSIBLE_PTR)
    {
        append_autoload_debug(format_args!(
            "own-load-pump: ABORT -- owner_ctx *(dialog+0x{:x})=0x{raw_owner_ctx:x} is not a plausible TitleFlowContext (dialog=0x{dialog:x}) -> no build (save-safe)",
            DIALOG_OWNER_CTX_A38_OFFSET
        ));
        return;
    }
    let owner_ctx = raw_owner_ctx;
    let want_slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "own-load-pump: BUILD 0x{:x}(out, ctx_parent=dialog+0x{:x}=0x{ctx_parent:x}, slot={want_slot}, owner_ctx=*(dialog+0x{:x})=0x{owner_ctx:x}) dialog=0x{dialog:x} -- CORRECTED dialog-derived ctx (golden Continue args) presses=0",
        base + LOADGAME_JOB_BUILD_RVA,
        DIALOG_CTX_PARENT_50_OFFSET,
        DIALOG_OWNER_CTX_A38_OFFSET,
    ));
    // BUILD the LoadGame MenuJobWithContext into a local DLRefCountPtr (factory writes the job ptr into
    // *out with refcount 1). Win64 fastcall (out, ctx_parent, save_slot:i32, owner_ctx).
    // Justify the transmute: LOADGAME_JOB_BUILD_RVA is the prologue-grounded live entry of the menu-heap
    // LoadGame-job factory; the signature matches the static decompile of FUN_140826510.
    let build: unsafe extern "system" fn(*mut usize, usize, i32, usize) -> *mut usize =
        unsafe { std::mem::transmute(base + LOADGAME_JOB_BUILD_RVA) };
    let mut built_job: usize = 0;
    let build_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        build(&raw mut built_job, ctx_parent, want_slot, owner_ctx)
    }));
    if build_ret.is_err() {
        append_autoload_debug(format_args!(
            "own-load-pump: BUILD PANICKED (caught) -- NO pump -> ABORT (save-safe)"
        ));
        return;
    }
    if built_job == null || built_job == 0 {
        append_autoload_debug(format_args!(
            "own-load-pump: BUILD returned a NULL job (built_job=0x{built_job:x}) -- NO pump -> ABORT (save-safe)"
        ));
        return;
    }
    let built_vt = unsafe { safe_read_usize(built_job) }.unwrap_or(null);
    let built_flag = unsafe { safe_read_usize(built_job + MENUJOB_BUILT_FLAG_68_OFFSET) }
        .map(|v| v as u8)
        .unwrap_or(0);
    // Arm the recurring private pump: publish the job ptr + cache owner/InGameStep (mirror the other
    // levers) so the recurring observer keeps logging through the loading screen, and set
    // OWN_LOAD_CONTINUE_FIRED so own_load_stream_observe_recurring runs each frame. Do NOT install the
    // job anywhere -- the recurring task pumps Run directly.
    OWN_LOAD_PUMP_JOB.store(built_job, Ordering::SeqCst);
    OWN_LOAD_OWNER_CACHED.store(title_owner, Ordering::SeqCst);
    let ingame_cached = unsafe { safe_read_usize(title_owner + TITLE_OWNER_JOB_OFFSET) }
        .filter(|&v| v != null)
        .unwrap_or(0);
    OWN_LOAD_INGAMESTEP_CACHED.store(ingame_cached, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_FIRED.store(true, Ordering::SeqCst);
    timeline_event(
        "T_own_load_pump_build",
        n,
        format_args!("c30=0x{c30:x} level={fp_level}"),
    );
    append_autoload_debug(format_args!(
        "own-load-pump: *** BUILT job=0x{built_job:x} vt=0x{built_vt:x} (expect LoadGame dump 0x{:x}) +0x68_built={built_flag} -- ARMED private per-frame pump (NO owner+0x130 install, NO queue, NO dialog) ingame=0x{ingame_cached:x} -- recurring task will tick Run each frame -> self-build -> deser -> m28 stream -> SetState5 transition on Success presses=0 (#{n}) ***",
        MENUJOB_LOADGAME_VTABLE_DUMP_VA,
    ));
}

/// PATH B per-frame PRIVATE PUMP (runs from the recurring game task each frame, gated). If a LoadGame
/// job was built+armed by `own_load_pump_fire`, tick its `Run` exactly the way the native
/// `ExecuteMenuJob` does -- a zero-init `MenuJobResult` and an `FD4Time` carrying the frame delta -- so
/// the job self-builds, deserializes, and map-streams the world WITHOUT the menu system. When the job
/// reaches `state==Success` (deser+map done, m28 mounted), drive the title->ingame transition ONCE via
/// the guarded `continue_confirm`/SetState5 (the same save-safe guard as `own_load_continue_fire`), then
/// latch `OWN_LOAD_PUMP_DONE` so we never re-pump or re-transition.
///
/// SAVE-SAFETY: the pump itself (build+deser+map-stream) is READ-only up to world-stream. The ONLY
/// save-writing step is the final SetState5 transition, which stays HARD-gated on the verified parse
/// (`c30_real && fp_real`, re-checked from the live GameMan+0xc30 and char fingerprint) + the title
/// owner's new-game flag clear -- mirroring `own_load_continue_fire`. No save write before the world is
/// confirmed loading. Every native call is wrapped in `catch_unwind` (a Rust panic is caught; a hardware
/// AV is not). Keeps `simulated_button_presses_total = 0`.
pub(crate) unsafe fn own_load_pump_tick(base: usize, gm: usize, frame_delta: f32) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let job = OWN_LOAD_PUMP_JOB.load(Ordering::SeqCst);
    if job == 0 || job == null {
        return;
    }
    if OWN_LOAD_PUMP_DONE.load(Ordering::SeqCst) {
        return;
    }
    // Build the call buffers exactly as native ExecuteMenuJob/STEP_MenuJobWait do: a zero-init
    // MenuJobResult (8 bytes) and an FD4Time (16 bytes) whose +0x8 f32 holds the frame delta (Run only
    // reads time+8; it writes the FD4Time vtable into time+0 itself). We over-size both buffers to a
    // qword to keep them aligned and writable.
    let mut result: [u8; FD4_TIME_SIZE] = [0u8; FD4_TIME_SIZE]; // >= MENUJOB_RESULT_SIZE; zero state.
    let mut time: [u8; FD4_TIME_SIZE] = [0u8; FD4_TIME_SIZE];
    // Write the f32 frame delta at time+0x8 (Run advances the map-stream sub-job on this).
    time[FD4_TIME_DELTA_8_OFFSET..FD4_TIME_DELTA_8_OFFSET + core::mem::size_of::<f32>()]
        .copy_from_slice(&frame_delta.to_le_bytes());
    let result_ptr = result.as_mut_ptr() as usize;
    let time_ptr = time.as_mut_ptr() as usize;
    // Run(this /*rcx*/, result /*rdx*/, time /*r8*/, param4 /*r9*/) -> *MenuJobResult.
    // Justify the transmute: LOADGAME_JOB_RUN_RVA is the prologue-grounded live entry of the LoadGame
    // MenuJobWithContext::Run (vtable+0x10), signature per the static decompile of FUN_140826e40.
    let run: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(base + LOADGAME_JOB_RUN_RVA) };
    let run_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        run(job, result_ptr, time_ptr, 0)
    }));
    let fired = OWN_LOAD_PUMP_FIRED.fetch_add(1, Ordering::SeqCst) + 1;
    if run_ret.is_err() {
        // A Rust-level panic in Run -> stop pumping (latch done) so we do not re-fault every frame.
        OWN_LOAD_PUMP_DONE.store(true, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "own-load-pump: Run PANICKED (caught) at pump #{fired} (job=0x{job:x}) -> latch DONE, no transition (save-safe)"
        ));
        return;
    }
    // Read back the result state (+0x0) and the inner deser sub-code (+0x4).
    let state = i32::from_le_bytes([
        result[MENUJOB_RESULT_STATE_0_OFFSET],
        result[MENUJOB_RESULT_STATE_0_OFFSET + 1],
        result[MENUJOB_RESULT_STATE_0_OFFSET + 2],
        result[MENUJOB_RESULT_STATE_0_OFFSET + 3],
    ]);
    let subcode = i32::from_le_bytes([
        result[MENUJOB_RESULT_SUBCODE_4_OFFSET],
        result[MENUJOB_RESULT_SUBCODE_4_OFFSET + 1],
        result[MENUJOB_RESULT_SUBCODE_4_OFFSET + 2],
        result[MENUJOB_RESULT_SUBCODE_4_OFFSET + 3],
    ]);
    OWN_LOAD_PUMP_STATE.store(i64::from(state), Ordering::SeqCst);
    OWN_LOAD_PUMP_SUBCODE.store(i64::from(subcode), Ordering::SeqCst);
    // Job header diagnostics: +0x68 built flag flips 0->1 on self-build, +0x70 inner-seq ptr 0->built.
    let built_flag = unsafe { safe_read_usize(job + MENUJOB_BUILT_FLAG_68_OFFSET) }
        .map(|v| v as u8)
        .unwrap_or(0);
    let inner_seq = unsafe { safe_read_usize(job + MENUJOB_INNER_SEQ_70_OFFSET) }.unwrap_or(null);
    // Throttled log (every OWN_LOAD_STREAM_LOG_INTERVAL pumps), plus the first pump.
    if fired == 1 || fired % OWN_LOAD_STREAM_LOG_INTERVAL == 0 {
        append_autoload_debug(format_args!(
            "own-load-pump: pump #{fired} Run(job=0x{job:x}) state={state} (1=Continue 2=Success 3=Failed) subcode={subcode} (deser 5/2/6) +0x68_built={built_flag} +0x70_seq=0x{inner_seq:x} delta={frame_delta}"
        ));
    }
    if state <= MENUJOB_STATE_CONTINUE {
        // Still working (Continue) -- keep pumping next frame.
        return;
    }
    // Terminal: Success (2) or Failed (3). Latch DONE so we stop pumping regardless of the transition.
    OWN_LOAD_PUMP_DONE.store(true, Ordering::SeqCst);
    if state == MENUJOB_STATE_FAILED {
        append_autoload_debug(format_args!(
            "own-load-pump: pump #{fired} reached state=Failed(3) subcode={subcode} -- deser/map FAILED -> NO transition, latch DONE (save-safe)"
        ));
        return;
    }
    // state == Success: the job deserialized + map-streamed (m28). Drive the title->ingame transition
    // ONCE via the guarded SetState5. RE-VERIFY the parse from LIVE state (the build+pump can change
    // GameMan+0xc30) so the save-write transition is gated exactly like own_load_continue_fire.
    let owner = OWN_LOAD_OWNER_CACHED.load(Ordering::SeqCst);
    let c30_live = if gm != null && gm != 0 {
        unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    let c30_real =
        c30_live != GAME_MAN_C30_UNSET && c30_live != 0 && c30_live != FULLREAD_C30_M10_DEFAULT;
    let (fp_real, fp_level, _fp_name_len) = unsafe { char_fingerprint(base) };
    append_autoload_debug(format_args!(
        "own-load-pump: *** pump #{fired} reached state=Success(2) subcode={subcode} -- deser+map-stream DONE (m28 mounted); driving title->ingame transition ONCE (owner=0x{owner:x} c30_live=0x{c30_live:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}) ***"
    ));
    // SAVE-SAFE PROBE: if the verify-only gate is set, the pump has proven the corrected dialog-ctx
    // build reached Success (no AV) with the world map-streamed -- STOP HERE without the save-writing
    // SetState5 transition, so this can run against the real save with zero write risk.
    if own_load_pump_verify_only() {
        append_autoload_debug(format_args!(
            "own-load-pump: VERIFY-ONLY gate set -- reached Success(2) subcode={subcode} (corrected dialog-ctx build+pump OK, no AV); SKIPPING SetState5 transition -> NO save write, latch DONE (save-safe)"
        ));
        return;
    }
    // The transition is the SAME guarded continue_confirm/SetState5 path the legacy lever uses; it
    // re-checks c30_real && fp_real + the owner new-game flag internally and ABORTs (no write) on any
    // failure. Pass the live-re-verified c30 so the guard reflects the post-pump state.
    unsafe {
        own_load_continue_fire(base, owner, c30_live, c30_real, fp_real, fp_level, fired);
    }
}
