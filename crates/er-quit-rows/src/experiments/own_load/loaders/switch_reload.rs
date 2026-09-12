use super::*;

/// Restore `GLOBAL_CSGaitem` to constructor-pristine (empty gaitemInsTable + full free-queue) at a
/// clean title before the switch reload's fresh deserialize, so char#2's deserialize does not
/// exhaust the free-queue on char#1's leaked items (the AV at live 0x67141a, bd
/// system-quit-postswitch-crash-gaitem-freequeue-exhaustion-2026-07-02). Mechanism: sweep all
/// 0x1400 gaitemInsTable slots; for each occupied slot call the native per-item release
/// RemoveCSGaitemIns(gaitem, &entries[i].unindexedGaItemHandle) -- it destructs+deallocates the ins
/// (no leak) and returns index i to freeTableIdxQueue. This is the exact primitive the native
/// world/inventory teardown uses; we drive it because our lightweight return-title chain skips it.
///
/// Save-SAFETY / correctness preconditions (the caller must guarantee, and this fn re-checks what it
/// can): the old world is torn down (local player absent) so nothing live holds pointers to these
/// ins objects -- PlayerGameData/inventory hold only integer handles, which char#2's deserialize
/// overwrites. Structural validation (heap-aligned singleton, head/end within [0,0x1400)) fails
/// closed rather than sweeping a bogus pointer. Returns Some((released, slack_before, slack_after))
/// on success (slack = 0x13ff - free_count; healthy = slack_after 0), None if it declined.
pub(crate) unsafe fn own_load_reset_gaitem_singleton(base: usize) -> Option<(u32, u32, u32)> {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const RING_USABLE: u32 = (CSGAITEM_TABLE_CAPACITY as u32) - 1; // 0x13ff (one sentinel slot)
    let gaitem = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            GLOBAL_CSGAITEM_SINGLETON_RVA,
            "GLOBAL_CSGAITEM_SINGLETON_RVA",
        ))
    }
    .unwrap_or(NULL);
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
    let remove_ins: unsafe extern "system" fn(usize, usize) = unsafe {
        std::mem::transmute(crate::experiments::gated_game_fn(
            CSGAITEM_REMOVE_INS_RVA,
            "CSGAITEM_REMOVE_INS_RVA",
        )?)
    };
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
        er_game_base::mem::game_data_addr(base, CSGAITEM_REMOVE_INS_RVA, "CSGAITEM_REMOVE_INS_RVA")
    ));
    Some((released, slack_before, slack_after))
}

/// Synchronous fresh picked-slot feed-deserialize for the System->Quit->Load-Profile switch (the
/// continue_confirm hook calls this before forwarding, so the c30/PGD the confirm streams belong to
/// the picked slot -- bd system-quit-cleantitle-load-is-stale-restream-not-slot-source-2026-07-02).
/// Same proven mechanism as `own_load_drive` steps 1-4: read the on-disk save (native save-DIR
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
    if own_load_save_rejection_terminal() {
        append_autoload_debug(format_args!(
            "own-load-feed: terminal save rejection already published (fingerprint=0x{:016x}) -- switch remains fail-closed without a resolver retry",
            own_load_save_rejection_fingerprint()
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
    // Install the STEP_RequestWait guard alongside the feed hook: it is pass-through until this
    // switch arms it, and installing here means the detour is already live by the time
    // `continue_confirm` lets the incoming world's RequestWait tick.
    crate::experiments::own_load::install_request_wait_guard();
    if !install_own_load_hook() {
        append_autoload_debug(format_args!(
            "own-load-feed: hook install failed -- ABORT (no-write)"
        ));
        return false;
    }
    let c30_before =
        unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(GAME_MAN_C30_UNSET);
    OWN_LOAD_GATE.store(true, Ordering::SeqCst);
    let parser: unsafe extern "system" fn(i32) -> i32 = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(DESERIALIZE_SLOT_RVA, "DESERIALIZE_SLOT_RVA") {
                Some(address) => address,
                None => return false,
            },
        )
    };
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
        er_game_base::mem::game_data_addr(base, DESERIALIZE_SLOT_RVA, "DESERIALIZE_SLOT_RVA")
    ));
    ok
}

// FD4-IO residency for the menu-free switch reload (bd er-effects-rs-9fmm, 2026-07-19) is now default
// behavior in own_load_switch_reload_fire (the boot native-fullread submit -> drain(b80==resident) ->
// commit sequence), replacing the old resource-less one-shot. No marker/env gate.

pub(crate) use er_telemetry_core::counters::SWITCH_RELOAD_FD4IO_COMMITTED;
pub(crate) use er_telemetry_core::counters::SWITCH_RELOAD_FD4IO_DRAIN_WAITS;
/// Phase machine state for the reload FD4-IO SUBMIT/DRAIN (own_load_switch_reload_fire), persisted
/// across the caller's per-frame retries. 0=idle (do submit once), 1=drain (tick until b80==3),
/// 2=commit (fall through to feed+continue_confirm).
pub(crate) use er_telemetry_core::counters::SWITCH_RELOAD_FD4IO_PHASE;
// The phase values moved next to the atomic in er-telemetry-core (2026-07-31, bd er-effects-rs-9jbe):
// er-title-flow's b78 guard now reads this phase to detect that fd4io owns GameMan+0xb78, and that
// crate must not depend on the root crate. This file remains the only writer of the phase machine.
pub(crate) use er_telemetry_core::counters::SWITCH_RELOAD_FD4IO_COMMIT;
pub(crate) use er_telemetry_core::counters::SWITCH_RELOAD_FD4IO_DRAIN;
pub(crate) use er_telemetry_core::counters::SWITCH_RELOAD_FD4IO_IDLE;
/// Bound the reload drain far below the boot's FULLREAD_DRAIN_MAX (1200): the b80 2->3 save-file read
/// residency is fast (~17 ticks at boot); if it does not resident within this many frames the read is
/// not draining at the clean-title timing -> fall through to commit without residency (fail-soft to the
/// old behavior) rather than hang the switch.
const SWITCH_RELOAD_FD4IO_DRAIN_MAX: usize = 600;

/// Phase-3 (bd PHASE3-render-release-is-CommonFinalize): max frames `own_load_switch_reload_fire` holds the
/// reload's continue_confirm waiting for the outgoing world's `_Common_Finalize`. In the success path the
/// outgoing world is released in-world (before the title owner even appears -- the scoped menuData+0x5d
/// ending-drive walks its MoveMapStep 18->19->20) so this wait is ~0. The bound only matters when the
/// native teardown never completes -> fail-soft to the old in-place reload (the two holds re-engage), so a
/// stalled outgoing teardown can never softlock the switch. ~15s at 60fps, well under the runtime cap.
const OUTGOING_TEARDOWN_WAIT_MAX: usize = 900;

/// Reset the switch-reload FD4-IO phase machine so a new switch re-runs submit -> drain -> commit.
/// Without this the one-shot stays claimed after the first switch (phase stuck at commit +
/// SWITCH_RELOAD_FD4IO_COMMITTED=1), so the second switch's own_load_switch_reload_fire hits the
/// already-committed guard and returns immediately without loading -> the 2nd reload (load3) never
/// initiates and the game sits at a clean/PRESS-any-button title (run 110005: switch #1 loaded load2
/// via SUBMIT/DRAIN/COMMIT; switch #2 armed + tore the world down but emitted no reload-fd4io submit,
/// so load3 stalled at bar step 1). switch_slot_arm_programmatic calls this on every switch arm so each
/// switch gets a fresh phase machine.
pub(crate) fn reset_switch_reload_fd4io_phase() {
    SWITCH_RELOAD_FD4IO_PHASE.store(SWITCH_RELOAD_FD4IO_IDLE, Ordering::SeqCst);
    SWITCH_RELOAD_FD4IO_COMMITTED.store(0, Ordering::SeqCst);
    SWITCH_RELOAD_FD4IO_DRAIN_WAITS.store(0, Ordering::SeqCst);
}

/// Full per-switch latch reset for an armed switch reload: the FD4-IO phase machine and the Phase-3
/// outgoing-world teardown latches (baseline snapshot + DONE/WAIT_TICKS/FAILSOFT). Both arm paths --
/// the programmatic `switch_slot_arm_programmatic` (agent/control-file drive) and the user ProfileSelect
/// `system_quit_arm_quickload_autoload` -- Must call this or the two drift. That drift was the load3
/// softlock: the user path reset only FRESH_DESER_DONE/MENU_FREE_RELOAD_FIRED, leaving
/// `SWITCH_RELOAD_FD4IO_COMMITTED=1` stale from load2, so a user-driven load3 hit the already-committed
/// guard in `own_load_switch_reload_fire`, emitted no submit, left FRESH_DESER_DONE=0, and the b78 guard
/// wrote GameMan requestedSaveSlotLoad=-1 every frame -> native pump gate false -> world torn down at
/// entering world (bd compounding-reload-two-roots-...-chainB-stale-fd4io-latch-b78-2026-07-23).
pub(crate) fn reset_switch_reload_latches() {
    reset_switch_reload_fd4io_phase();
    // Snapshot the finalize baseline for this switch + clear the per-switch teardown latches, so the reload
    // gate detects the outgoing world's `_Common_Finalize` (COMMON_FINALIZE_CALLS crossing the baseline).
    er_telemetry_core::counters::OUTGOING_TEARDOWN_BASELINE.store(
        er_telemetry_core::counters::COMMON_FINALIZE_CALLS.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    er_telemetry_core::counters::OUTGOING_TEARDOWN_DONE.store(0, Ordering::SeqCst);
    er_telemetry_core::counters::OUTGOING_TEARDOWN_WAIT_TICKS.store(0, Ordering::SeqCst);
    er_telemetry_core::counters::OUTGOING_TEARDOWN_FAILSOFT.store(0, Ordering::SeqCst);
    // Per-switch WorldResWait defer-release hold latches: clear armed + residency/hold state so each
    // switch gets a fresh hold and a stale armed can never leak into a later load (bd reload-overlap-fix-
    // design-worldreswait-defer-release-on-streaming-settle-2026-07-24).
    reset_worldreswait_hold_latches();
}

/// Submit the native full-save-read for `picked` so the FD4 IO worker pool loads it resident, exactly
/// as the boot native-fullread submit phase (slot_resolution.rs). Mirrors its calls/RVAs. On accept it
/// sets GameMan+0xb80=2 (the deserialize arm) and the drain tick then advances it to resident(3).
///
/// Returns the INITIATOR'S answer, and the caller must act on it: `0x67b1a0` is
/// `if (b80 == 0) { r = builder(dev, 10, flag); if (r) b80 = 2; return r; } return 0;`, so a zero
/// means no request exists and `b80` stays 0 forever -- a drain armed on it waits for
/// `b80 == RESIDENT(3)` that nothing can produce. The device is sampled immediately before and
/// after the call because `iodev+0x18`/`+0x20` are the only operands that can refuse it (see
/// `er_save_suppress::open_the_device_for_a_load`), so the line is a verdict, not a symptom.
unsafe fn own_load_fd4io_submit(base: usize, gm: usize, picked: i32, gate: &str) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Mark the slot occupied so the native save-load gate accepts it (idempotent, no other effect).
    let gdm = game_data_man_ptr_or_null();
    let summary = if gdm != null {
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(null)
    } else {
        null
    };
    if summary != null {
        let mark: unsafe extern "system" fn(usize, i32) -> u8 = unsafe {
            std::mem::transmute(
                match crate::experiments::gated_game_fn(
                    PROFILE_MARK_SLOT_USED_RVA,
                    "PROFILE_MARK_SLOT_USED_RVA",
                ) {
                    Some(address) => address,
                    None => return false,
                },
            )
        };
        let _ = unsafe { mark(summary, picked) };
    }
    // Resolve our slot + submit the full read (type-0xa; sets b80=2).
    unsafe { *((gm + GAME_MAN_SLOT_SELECT_B78_OFFSET) as *mut i32) = picked };
    let set_save_slot: unsafe extern "system" fn(i32) = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(
                FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA,
                "FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA",
            ) {
                Some(address) => address,
                None => return false,
            },
        )
    };
    unsafe { set_save_slot(picked) };
    let submit: unsafe extern "system" fn(i32) -> i32 = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(
                B80_FULL_LOAD_INITIATOR_RVA,
                "B80_FULL_LOAD_INITIATOR_RVA",
            ) {
                Some(address) => address,
                None => return false,
            },
        )
    };
    // Not `submit(picked)`: the argument is a flag the game always passes as 0, and the slot was
    // already set by `set_save_slot` above. Passing the slot here is what refused every non-zero
    // slot and soft-locked System->Quit->Load Character. See `B80_FULL_LOAD_SUBMIT_FLAG`.
    let device_before = er_save_suppress::sample_sl_request_slot();
    let sret = unsafe { submit(B80_FULL_LOAD_SUBMIT_FLAG) };
    let device_after = er_save_suppress::sample_sl_request_slot();
    let b80 = unsafe { safe_read_i32(gm + GAME_MAN_SAVE_STATE_B80_OFFSET) }.unwrap_or(-1);
    append_autoload_debug(format_args!(
        "reload-fd4io: SUBMIT slot={picked} submit 0x{:x} {}",
        er_game_base::mem::game_data_addr(
            base,
            B80_FULL_LOAD_INITIATOR_RVA,
            "B80_FULL_LOAD_INITIATOR_RVA"
        ),
        er_save_suppress::describe_load_submit(sret, b80, gate, device_before, device_after)
    ));
    sret != 0
}

/// One drain tick of the load we submitted: pump the b80 poll and return GameMan+0xb80 so the caller
/// can detect resident(3). Pumps only the lane we own -- `er_save_suppress::load_poll_may_run` carries
/// why, and why the 0x679510 call that stood here could only tick somebody else's request.
unsafe fn own_load_fd4io_drain_tick(_base: usize, gm: usize) -> i32 {
    let b80 = || unsafe { safe_read_i32(gm + GAME_MAN_SAVE_STATE_B80_OFFSET) }.unwrap_or(-1);
    if !er_save_suppress::load_poll_may_run_now() {
        return b80();
    }
    let poll: unsafe extern "system" fn(u8, u8) -> i32 = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(B80_POLL_RVA, "B80_POLL_RVA") {
                Some(address) => address,
                None => return b80(),
            },
        )
    };
    let _ = unsafe { poll(FULLREAD_POLL_ARG, FULLREAD_POLL_ARG) };
    b80()
}

/// Menu-free clean-title reload of the picked slot for a genuine System->Quit->Load-Profile switch.
/// The warm-rebuilt TitleTopDialog never reaches Loop post-return-title (press-start SceneObjProxy at
/// dialog+0xb78 unbound), so the title accept-byte/open-menu path deadlocks; native_fullread_tick also
/// stands down for a switch. Drive the picked slot through the same native-ownership commit the boot
/// autoload uses, exactly like the (now-dead) native-fullread DESER switch_feed_case
/// (slot_resolution.rs:275-296): reset the gaitem singleton -> feed the picked slot's on-disk bytes
/// through the native parser (real c30 + PGD) -> latch FRESH_DESER_DONE -> native continue_confirm
/// (intercepted by system_quit_continue_confirm_hook -> SetState5 streams the world + performs the
/// switch cleanup). One-shot per switch (SYSTEM_QUIT_SWITCH_MENU_FREE_RELOAD_FIRED). Returns true only
/// when it fired continue_confirm; false = "not yet / could not" (nothing consumed unless the one-shot
/// was legitimately claimed) and the caller keeps waiting. Caller must have proven the old world is
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
    // (a) Validate the title owner first -- it flickers during the warm rebuild. No state consumed:
    // a bad-owner frame returns false and the caller retries next frame. Must be a live owner with the
    // new-game flag clear (continue_confirm's load branch; nonzero = NewGame path / mid-rebuild).
    if owner == null {
        return false;
    }
    let new_game_flag =
        match unsafe { safe_read_usize(owner + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET) } {
            Some(v) => v as u8,
            None => return false,
        };
    if new_game_flag != FULLREAD_OWNER_NEW_GAME_OK {
        return false;
    }
    // (a.5) phase-3 outgoing-world TEARDOWN gate (bd PHASE3-render-release-is-CommonFinalize). Hold the
    // reload's continue_confirm until the outgoing (pre-quit) world's native render-release has run
    // (COMMON_FINALIZE_CALLS crossed the per-switch baseline captured at arm), so the reload rebuilds a
    // fresh world instead of loading in-place over the still-live WorldChrMan/CSDistViewManager/
    // g_GxDrawContext (the ~5x-heavier-render / 5-vblank bug). The outgoing world is driven to
    // _Common_Finalize in-world by the scoped menuData+0x5d ending-drive (title_tick_cover), which runs
    // before the title owner appears, so in the success path the finalize is already observed the first
    // time we reach here. Bounded + fail-soft: on timeout, latch FAILSOFT (the two in-place holds
    // re-engage via outgoing_teardown_suppresses_holds) and fall through to the old in-place reload, so a
    // stalled teardown can never softlock. Once DONE/FAILSOFT latches, this gate is skipped for the switch.
    if crate::experiments::gating::outgoing_teardown_enabled()
        && crate::experiments::gating::switch_reload_active()
        && er_telemetry_core::counters::OUTGOING_TEARDOWN_DONE.load(Ordering::SeqCst) == 0
        && er_telemetry_core::counters::OUTGOING_TEARDOWN_FAILSOFT.load(Ordering::SeqCst) == 0
    {
        let baseline =
            er_telemetry_core::counters::OUTGOING_TEARDOWN_BASELINE.load(Ordering::SeqCst);
        let calls = er_telemetry_core::counters::COMMON_FINALIZE_CALLS.load(Ordering::SeqCst);
        if calls > baseline {
            er_telemetry_core::counters::OUTGOING_TEARDOWN_DONE.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "outgoing-teardown: OBSERVED _Common_Finalize (calls={calls} > baseline={baseline}) -- OUTGOING world released; reload rebuilds FRESH (in-place holds stay disabled) (#{n})"
            ));
        } else {
            let waited = er_telemetry_core::counters::OUTGOING_TEARDOWN_WAIT_TICKS
                .fetch_add(1, Ordering::SeqCst)
                + 1;
            if waited >= OUTGOING_TEARDOWN_WAIT_MAX {
                er_telemetry_core::counters::OUTGOING_TEARDOWN_FAILSOFT.store(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "outgoing-teardown: FAIL-SOFT after {waited} frames without _Common_Finalize (calls={calls} baseline={baseline}) -- falling back to OLD in-place reload; the two holds re-engage (#{n})"
                ));
                // fall through this frame to the normal reload path (old behavior; no softlock)
            } else {
                if waited == 1 || waited.is_multiple_of(120) {
                    append_autoload_debug(format_args!(
                        "outgoing-teardown: waiting for OUTGOING _Common_Finalize (calls={calls} baseline={baseline} waited={waited}/{OUTGOING_TEARDOWN_WAIT_MAX}) -- holding continue_confirm so the reload rebuilds fresh (#{n})"
                    ));
                }
                return false;
            }
        }
    }
    // (b) FD4-IO residency phase machine (default behavior -- no marker/env toggle; bd er-effects-rs-9fmm):
    // Submit the full read, drain until GameMan+0xb80==resident(3), then fall through to
    // feed+continue_confirm -- so the reload's streamed world has the resources natively resident (the
    // boot path's behavior) instead of entering resource-less and reverting to title. Owner is already
    // validated, so a flickering frame never burns the one-shot (claimed by SWITCH_RELOAD_FD4IO_COMMITTED).
    {
        let phase = SWITCH_RELOAD_FD4IO_PHASE.load(Ordering::SeqCst);
        if phase == SWITCH_RELOAD_FD4IO_IDLE {
            // The load builder'S real PRECONDITION, not the mutex alone: `sl_device_is_free_now`
            // reads `GameMan+0xb80` and nothing else, and the state that hung the third
            // same-character load is `saveState == 0` with the device latched by a forgotten save.
            // `open_the_device_for_a_load` reads both operands and asks the game's own poll to
            // release a drainable orphan; it carries the full derivation. Nothing here writes a
            // device field or advances a native step.
            let gate = er_save_suppress::open_the_device_for_a_load("switch reload load submit");
            let waited = SWITCH_RELOAD_FD4IO_DRAIN_WAITS.load(Ordering::SeqCst);
            let expired = waited >= SWITCH_RELOAD_FD4IO_DRAIN_MAX;
            if !gate.admits() && !expired {
                let w = SWITCH_RELOAD_FD4IO_DRAIN_WAITS.fetch_add(1, Ordering::SeqCst) + 1;
                if w == 1 || w.is_multiple_of(120) {
                    append_autoload_debug(format_args!(
                        "reload-fd4io: SUBMIT held at the gate ({w}/{SWITCH_RELOAD_FD4IO_DRAIN_MAX}) -- {}",
                        gate.describe()
                    ));
                }
                return false;
            }
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
                let accepted = unsafe {
                    own_load_fd4io_submit(
                        base,
                        gm,
                        picked,
                        er_save_suppress::load_gate_outcome_label(gate.outcome),
                    )
                };
                if !accepted {
                    // Un-arm the drain nobody can satisfy. With budget left, retry from idle and
                    // spend one tick -- the repair needs re-running because the game's poll answers
                    // "still in flight" until the SL worker finishes (177ms, measured). With the
                    // budget gone, take the drain timeout's own fail-soft. b78 + set_save_slot were
                    // written by the attempt above, so the finalize warp target is armed either way.
                    let next = if expired {
                        append_autoload_debug(format_args!(
                            "reload-fd4io: SUBMIT refused after the full {waited}-frame gate budget -- COMMIT without FD4-IO residency (the DRAIN timeout's fail-soft, reached without the empty wait); {}",
                            gate.describe()
                        ));
                        SWITCH_RELOAD_FD4IO_COMMIT
                    } else {
                        SWITCH_RELOAD_FD4IO_IDLE
                    };
                    SWITCH_RELOAD_FD4IO_PHASE.store(next, Ordering::SeqCst);
                    SWITCH_RELOAD_FD4IO_DRAIN_WAITS.store(waited + 1, Ordering::SeqCst);
                }
            }
            if SWITCH_RELOAD_FD4IO_PHASE.load(Ordering::SeqCst) != SWITCH_RELOAD_FD4IO_COMMIT {
                return false;
            }
            // fail-soft only: fall through to the commit claim below.
        }
        if phase == SWITCH_RELOAD_FD4IO_DRAIN {
            let b80 = unsafe { own_load_fd4io_drain_tick(base, gm) };
            let w = SWITCH_RELOAD_FD4IO_DRAIN_WAITS.fetch_add(1, Ordering::SeqCst);
            let resident = b80 == FULLREAD_B80_RESIDENT;
            if resident || w >= SWITCH_RELOAD_FD4IO_DRAIN_MAX {
                // Do not disarm b78 here. Unlike the boot native-fullread (which disarms on commit),
                // the System->Quit switch path must keep GameMan+0xb78 armed (= picked slot) through
                // SetState5/MoveMap finalize: it is the warp target that MoveMapStep finalize case 8
                // consumes to warp the character and autoclear warpRequested before advancing mms18
                // (system_quit_repro_guards.rs:1720-1754). Clearing it early leaves the load with no
                // warp target -> warp_requested stuck at 1 and STEP_MoveMap self-loops at 18 (observed
                // in the b78-disarm build: world resident, real char, but mms18 next=18/done50=0
                // warp=1 forever).
                // Who actually clears b78 (corrected 2026-08-01, bd er-effects-rs-0nie): Not the
                // continue_confirm hook -- that stopped writing OWN_STEPPER_SLOT_NONE at 1a0ad8e4.
                // The remaining clearer on this path is system_quit_inworld_load_skip_hook
                // (system_quit_repro_guards.rs), which hooks SYSTEM_QUIT_INWORLD_LOAD_RVA 0x67b290 --
                // the same address as DESERIALIZE_SLOT_RVA that own_load_feed_deserialize calls
                // directly, so despite the "inworld" name it runs on this commit feed. The other
                // writer, er-title-flow's b78 guard, cannot reach here: the line below sets
                // FRESH_DESER_DONE=1, which is one of that guard's window conditions, so its window
                // closes the moment commit begins.
                append_autoload_debug(format_args!(
                    "reload-fd4io: DRAIN done b80={b80} waits={w} resident={resident}{} -> COMMIT (feed+continue_confirm); b78 kept armed (warp target) through finalize",
                    if resident {
                        ""
                    } else {
                        " (TIMEOUT -- committing without residency, fail-soft to old behavior)"
                    }
                ));
                SWITCH_RELOAD_FD4IO_PHASE.store(SWITCH_RELOAD_FD4IO_COMMIT, Ordering::SeqCst);
                // fall through to feed+continue_confirm this frame
            } else {
                return false; // keep draining
            }
        }
        // phase == commit (reached this frame or a prior one): commit exactly once.
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
    // picked character's real map + a real PGD fingerprint. No FD4 IO SUBMIT/DRAIN, no b80==3 needed.
    if !unsafe { own_load_feed_deserialize(base, gm, picked) } {
        append_autoload_debug(format_args!(
            "own-load-switch-reload: feed-deserialize of picked slot {picked} FAILED -- NOT firing continue_confirm; switch fails closed (one-shot claimed, no re-attempt)"
        ));
        return false;
    }
    // (e) Latch native_slot_proven before firing: the continue_confirm hook reads FRESH_DESER_DONE==1
    // to take the forward->SetState5 path (not the no-proof forward) and to prevent any double-feed.
    SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.store(1, Ordering::SeqCst);
    // Record which slot's deserialize completed (slot+1). The published-vs-loaded portrait oracle
    // compares against this, not `GameMan.save_slot` -- ac0 is written by our own `set_save_slot`
    // above and by the game's own selector, so it does not answer "which character loaded".
    er_telemetry_core::counters::SYSTEM_QUIT_FRESH_DESER_DONE_SLOT
        .store((picked + 1) as usize, Ordering::SeqCst);
    // (f) Re-read the freshly-mounted c30 + fingerprint and fire the guarded native continue_confirm
    // (own_load_continue_fire re-guards c30_real && fp_real && owner+0x284==0 internally -- the only
    // save-writing SetState5 is behind that hard guard).
    let c30 =
        unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(GAME_MAN_C30_UNSET);
    let c30_real = c30 != GAME_MAN_C30_UNSET && c30 != 0 && c30 != FULLREAD_C30_M10_DEFAULT;
    let (fp_real, fp_level, _nl) = unsafe { char_fingerprint(base) };
    append_autoload_debug(format_args!(
        "own-load-switch-reload: picked slot {picked} mounted (c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}); firing native continue_confirm owner=0x{owner:x} (hook forwards -> SetState5 streams + performs switch cleanup) presses=0 (#{n})"
    ));
    // Arm the STEP_WorldResWait streaming-settle hold for this switch (bd reload-overlap-fix-design-
    // worldreswait-defer-release-on-streaming-settle-2026-07-24), at the SetState5/continue point -- the
    // same site as arm_request_move_map_fixup -- so the hold covers the upcoming RequestMoveMap -> MoveMap
    // -> STEP_WorldResWait. No-op unless the default-off opt-in marker is present and this is a genuine
    // in-world switch (switch_reload_active && player was present at arm), so load1/boot are never touched.
    arm_worldreswait_hold();
    // Retire our own return-title request before the incoming world INHERITS it (2026-09-04).
    //
    // `menuData+0x5d` is the return-title request byte, and it is OURS: the switch arm wrote it 1
    // (`switch_slot_arm_programmatic`, and the user's ProfileSelect path through
    // `system_quit_arm_quickload_autoload`) so the outgoing world's MoveMapStep child would walk
    // 18 -> Cleanup -> Finish and tear that world down. It is a request, it has now been served, and
    // nothing was clearing it.
    //
    // It is process-global -- `GLOBAL_CSMenuMan->menuData`, not a per-child field -- so leaving it set
    // hands the request straight to the incoming child. Measured on run br-20260905-024539-2daf: the
    // incoming world's `SetState(5 PlayGame)` frame reads `ENDCOND[... md5d=1 md5e=1]`, its child then
    // walks to `state=20 field25=9` (child-done DIAG #5, +369671ms) instead of parking at the resident
    // step 18, `STEP_MoveMap_Update` sees the child done and drains `InGameStep+0xd8` 1 -> 2 -> 0, and
    // `STEP_GameStepWait` -- whose 1.16.2 decompile has no stay-in-6 branch when d8 == 0 with
    // `GameMan+0xb7c`/`+0xb7d` clear -- does `SetMapId(0xff,0xff,0xff,0xff)` and `SetState(2 BeginLogo)`.
    // That is the black screen, 7s after a load that succeeded.
    //
    // The cold boot is immune for exactly this reason and no other: nobody ever set 0x5d, so its child
    // parks at 18 forever, d8 holds at 1, and the GameStepWait gate is never satisfied.
    //
    // Clearing here rather than later because this is the last instant the byte is unambiguously about
    // the outgoing world: `own_load_continue_fire` hands off to SetState5 and the incoming child is
    // created downstream of it. This retires a request we issued -- it is not steering a game-owned
    // state machine, and the native teardown it asked for has already completed by this point.
    if let Some(menu_data) = unsafe { resolve_menu_data(base) } {
        let previous =
            unsafe { safe_read_u8(menu_data + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET) }
                .unwrap_or(0);
        if previous != 0 {
            unsafe {
                *((menu_data + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET) as *mut u8) = 0;
            }
            let cleared = er_telemetry_core::counters::SWITCH_RETURN_TITLE_REQUEST_RETIRED_COUNT
                .fetch_add(1, Ordering::SeqCst)
                + 1;
            append_autoload_debug(format_args!(
                "own-load-switch-reload: retired our own return-title request menuData+0x5d {previous}->0 #{cleared} before the incoming world is created -- it was served by the OUTGOING teardown, and leaving it set makes the incoming child walk 18->20, drain InGameStep+0xd8, and hit the STEP_GameStepWait teardown (the black screen)"
            ));
        }
    }
    // Arm the session-end guard before the incoming world can tick. `continue_confirm` hands off to
    // SetState5, after which `STEP_RequestWait` runs against this switch's world -- and because we
    // mounted the map before firing, its first tick can already see `requestCode == 2` and take the
    // session-end arm that produced `WORLD LOST`. See `request_wait_guard` for the decompiled branch.
    crate::experiments::own_load::arm_request_wait_guard_for_switch();
    unsafe { own_load_continue_fire(base, owner, c30, c30_real, fp_real, fp_level, n) };
    // Retire the warp our own DESERIALIZE armed (2026-09-05).
    //
    // `FUN_14067b290` -- the native slot deserialize this switch just drove -- ends with
    // `SetMoveMapStepBlockId(GameMan+0xc30)`, `initialAreaEntityId = 0` and `warpRequested = true`.
    // On the native title->Continue path that warp is the load: the InGameStep consumes it as it
    // creates the world. On our path the world already exists, and `own_load_continue_fire` above has
    // just performed the transition itself, so the armed warp is a second, surplus map move.
    //
    // Measured, run br-20260905-212504-9b8d, with the guard removed so nothing masked it:
    //   +57569ms  own-load-feed drives the deserialize   -> warpRequested false -> true at +57581ms
    //   +57573ms  requestwait-tick #1 d8=1 (advance)     -- our load runs and advances correctly
    //   ~+57.9s   c30 real, world streaming              -- the world is up without the warp firing
    //   +66141ms  warpRequested -> false                 -- the surplus warp finally executes
    //   +67031ms  requestwait-tick #2 d8=2               -- its completion lands back in RequestWait
    //                                                       with the NowLoading job gone, and the
    //                                                       native code ends the session
    // The boot load never arms this: `own-load-feed` appears exactly once per run, at the switch, and
    // boot's `warp_requested` reads false from +988ms through the whole load. So the flag is ours, we
    // raised it, our own continue already did what it asked for, and leaving it set is what the user
    // sees as our cover vanishing and the game's own loading screen appearing a beat later.
    //
    // Retiring a request we issued and have already satisfied -- the same act, and the same
    // justification, as the `menuData+0x5d` retirement a few lines above. It is not steering a
    // game-owned state machine: the world this warp would move is the one we just finished loading.
    if c30_real {
        let gm = crate::constants::game_man_ptr_or_null();
        if gm > TITLE_OWNER_SCAN_START_ADDRESS {
            let previous =
                unsafe { safe_read_u8(gm + GAME_MAN_WARP_REQUESTED_10_OFFSET) }.unwrap_or(0);
            if previous != 0 {
                unsafe {
                    *((gm + GAME_MAN_WARP_REQUESTED_10_OFFSET) as *mut u8) = 0;
                }
                append_autoload_debug(format_args!(
                    "own-load-switch-reload: retired the warp our own deserialize armed (GameMan+0x10 {previous}->0) -- continue_confirm has already performed this transition, and leaving it set fires a SECOND map move ~8.5s later whose completion re-enters STEP_RequestWait at d8=2 and ends the session"
                ));
            }
        }
    }
    // Hand the switch back so it works a second time (2026-09-05).
    //
    // The load-3 failure: switch #1 loads its character, switch #2 arms and then nothing happens --
    // the world you are standing in is never torn down, no loading screen appears, and the quit menu
    // reopens over a destroyed ProfileSelect with rows that do nothing. Measured end to end on run
    // br-20260905-212926-4ed9:
    //   +53662ms  switch #1 arms (slot 1)
    //   +53980ms  return-title request fired, bc4 forced ready, final functor submitted
    //   +57543ms  world lost #1 -- the old world is gone, which is the teardown working
    //   +57853ms  slot 1 loads. Onyx Lord, player present, c30 0x1c000000
    //   +119525ms switch #2 arms (slot 0) -- and that is the last thing it does
    //   +120170ms "requested save-slot load index world_up=true" and then that line, forever
    // No request, no bc4 force, no functor, no world lost #2, no reload submit. End-of-run telemetry
    // names the cause exactly: request_count=1, chain_submit_count=1, final_functor_call_count=1 --
    // all three still spent from switch #1, and all three are the `== 0` / `compare_exchange(0,1)`
    // gates the teardown runs behind.
    //
    // Why the existing RE-arm did not run. It lives in `system_quit_continue_confirm_hook`, behind
    // `switch_active` (phase in confirmed..=AUTOLOAD_HANDOFF). Our own `own_load_feed_deserialize`
    // above calls the native parser at 0x67b290 -- the same address `system_quit_inworld_load_skip_hook`
    // guards -- and on a proven deserialize that hook stores phase = idle. It runs milliseconds before
    // `own_load_continue_fire`, so by the time the confirm arrives the phase is already idle, the hook
    // classifies it non-switch, and the whole commit branch is skipped. Confirmed across all 40 runs
    // in ~/.cache/er-me3-runs: `continue_confirm_fresh_deser_count` is 0 in every one, and
    // `non_switch_count` carries every forward. That branch is dead code on this path; this call is
    // the live edge.
    //
    // Placed after `own_load_continue_fire` on purpose, and not at the switch ARM: the 2026-07-02
    // bisect showed an arm-time reset is re-consumed by the teardown still in flight and bounces even
    // a single switch. Here the switch is committed and both gates are independently shut (phase idle,
    // bc4 0), so handing the counters back opens nothing until the next arm.
    unsafe {
        crate::experiments::system_quit_rearm_switch_for_next_load("own-load-switch-reload-commit")
    };
    true
}

/// `GLOBAL_CSMenuMan->menuData`, or `None` on any fault-tolerant read failure. Pure reads.
unsafe fn resolve_menu_data(base: usize) -> Option<usize> {
    unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            CS_MENU_MAN_GLOBAL_RVA,
            "CS_MENU_MAN_GLOBAL_RVA",
        ))
    }
    .filter(|&m| m > 0x10000)
    .and_then(|m| unsafe { safe_read_usize(m + CS_MENU_MAN_MENU_DATA_OFFSET) })
    .filter(|&d| d > 0x10000)
}

/// Resolve `mss = GameDataMan->menuSystemSaveLoad = *(*(base + GAME_DATA_MAN_GLOBAL_RVA) +
/// GAME_DATA_MAN_MENU_SAVELOAD_60_OFFSET)` (static-verified: `GetMenuSystemSaveLoad` 0x140256410 is
/// exactly `GLOBAL_GameDataMan->menuSystemSaveLoad`). Returns `None` (never `null`/`0`) on any
/// fault-tolerant read failure. Pure reads.
pub(crate) unsafe fn resolve_menu_system_save_load(base: usize) -> Option<usize> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let gdm = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            GAME_DATA_MAN_GLOBAL_RVA,
            "GAME_DATA_MAN_GLOBAL_RVA",
        ))
    }
    .filter(|&v| v != null && v != 0)?;
    unsafe { safe_read_usize(gdm + GAME_DATA_MAN_MENU_SAVELOAD_60_OFFSET) }
        .filter(|&v| v != null && v != 0)
}

/// The "engine filled enough to drive our own load" gate -- distinct from "GameMan instance pointer
/// resolved" (`game_man_instance_resolved`), which flips true at BootPhase4, long before the load
/// machinery is usable. True iff GameDataMan + menuSystemSaveLoad (mss) resolve and the TitleFlowContext
/// at `mss+0xa38` is a plausible heap pointer. The plausibility range matters: before the GameFlow
/// constructs the TitleFlowContext it reads back as uninitialized garbage (e.g. 0x8080808080808080),
/// which a `!= 0` check would wrongly accept -- then the LoadGame job's first `Run` derefs it and
/// access-violates (the ~25s AV observed when arming at the bare title). When this returns true, the
/// native LoadGame job (`own_load_pump_fire`) can be built + pumped without that crash. The bypass arms
/// its own-load on this, not on `game_man_instance_resolved`.
/// (loadgame-build-ctx-ready-precondition-2026-06-22)
pub(crate) unsafe fn loadgame_build_ctx_ready(base: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Corrected (bd loadgame-owner-ctx-is-dialog-a38-not-mss-correction-2026-06-22): the buildable
    // TitleFlowContext is `*(CS::TitleTopDialog+0xa38)`, not `*(mss+0xa38)` (the mss reading was a red
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
    if dialog_vt
        != er_game_base::mem::game_data_addr(
            base,
            TITLE_TOP_DIALOG_VTABLE_RVA,
            "TITLE_TOP_DIALOG_VTABLE_RVA",
        )
    {
        return false;
    }
    let ctx = unsafe { safe_read_usize(dialog + DIALOG_OWNER_CTX_A38_OFFSET) }.unwrap_or(0);
    if !(ctx > OWNER_CTX_MIN_PLAUSIBLE_PTR && ctx < OWNER_CTX_MAX_PLAUSIBLE_PTR) {
        return false;
    }
    // Native `FUN_14082d090` checks this singleton before comparing regulation versions; our readiness
    // predicate must not claim the title/load context is usable before the same singleton exists.
    let regulation_manager = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            GLOBAL_CS_REGULATION_MANAGER_RVA,
            "GLOBAL_CS_REGULATION_MANAGER_RVA",
        ))
    }
    .unwrap_or(0);
    regulation_manager != 0 && regulation_manager != null
}
