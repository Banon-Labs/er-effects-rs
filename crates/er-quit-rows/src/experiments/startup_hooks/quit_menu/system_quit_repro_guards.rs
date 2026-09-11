use super::*;

pub(crate) unsafe extern "system" fn system_quit_profile_load_confirmed_hook(
    action_obj: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let orig = SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: ProfileLoadDialog confirmed-load trampoline unset for action=0x{action_obj:x} -- fail-closed return 0"
        ));
        return 0;
    }
    // Through the union's own shape: this rides `mh_install_hook_once` -> `register_union_hook`,
    // so the slot may hold the next handler on the address rather than the game trampoline.
    let original: crate::mh::UnionFn = unsafe { std::mem::transmute(orig) };
    let dialog =
        unsafe { safe_read_usize(action_obj + 0x8) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let profile_window = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    let system_quit_profile_active = dialog != TITLE_OWNER_SCAN_START_ADDRESS
        && profile_window != 0
        && dialog == profile_window
        && SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) != 0;
    if !system_quit_profile_active {
        return unsafe { original(action_obj, b, c, d) };
    }

    if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst) >= SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED
        && SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_BLOCK_COUNT.load(Ordering::SeqCst) != 0
    {
        // Close ProfileSelect via the native cancel/back close (FUN_1407ac980: SetResult(Failed) +
        // window close vmethod) instead of arming the confirm-load. Arming the load (writing
        // load_job_ctx+0x14c=2, the coupled Success-close path) makes the game enter an in-world
        // load/warp transition (GameMan.saveState/b80 -> 2 -> DoSaveStuff). Even with the actual
        // deserialize skipped by the FUN_14067b290 guard, that half-started transition sticks the game
        // at a loading screen and blocks the return-title chain from ever running (observed 2026-07-01:
        // stuck, return_title functor_call_count=0, save_state=3, player still present). The cancel-close
        // pops the ProfileSelect window without starting any load, so the menu-pump return-title chain
        // tears the world down cleanly and the autoload loads the picked slot at a clean title. This
        // runs in menu-pump ownership (this is the native confirm callback) and one-shot -- not the racy
        // game-task tick. See bd system-quit-load-profile-6runs-state-2026-07-01.
        let load_job_ctx = unsafe { safe_read_usize(dialog + 0x1cc8) }.unwrap_or(0);
        if dialog != 0 && dialog != TITLE_OWNER_SCAN_START_ADDRESS {
            match game_rva(SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA) {
                Ok(close_addr) => {
                    let close_fn: unsafe extern "system" fn(usize) =
                        unsafe { std::mem::transmute(close_addr) };
                    unsafe { close_fn(dialog) };
                    SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_FIRED.store(1, Ordering::SeqCst);
                    SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_COUNT.fetch_add(1, Ordering::SeqCst);
                }
                Err(_) => append_autoload_debug(format_args!(
                    "system-quit-dup: confirm cancel-close ABORT -- failed to resolve close rva 0x{SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA:x}"
                )),
            }
        }
        SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-dup: ProfileSelect confirm CANCEL-CLOSED action=0x{action_obj:x} dialog=0x{dialog:x} load_job_ctx=0x{load_job_ctx:x}; NO load-mode armed -> no in-world load transition -> return-title tears down + autoload loads at clean title"
        ));
        return 0;
    }

    SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: ProfileSelect confirmed-load transition ALLOWED action=0x{action_obj:x} dialog=0x{dialog:x}; actual load/deser is guarded at LoadJobContext::Run"
    ));
    unsafe { original(action_obj, b, c, d) }
}

/// Portrait RETARGET + boot-view cover rearm for an own-menu character switch. It used to be shared
/// between the user ProfileSelect arm (`system_quit_arm_quickload_autoload`) and a menu-free
/// control-file arm; that second arm was deleted on 2026-09-05, so ProfileSelect is now the sole
/// caller and the no-drift argument this doc used to make has nothing left to drift against.
/// Game thread only.
///
/// Portrait RETARGET (user 2026-07-03): the user just confirmed a new character for load, so the
/// loading-screen portrait should render that character, not the one still resident (ac0). Make it
/// before-break: retarget the spare/render to the selected slot (portrait_target_slot now returns
/// it) and RE-engage the drive (clear the per-window freeze) so the new model renders + gets its
/// depth mask -- but do not touch LOADING_BG_PORTRAIT_RGBA / PROFILE_HAVE_KEYED_FRAME, so the prior
/// masked head keeps displaying until the new model's first KEYED frame replaces it (no opaque
/// flash, no blank). Clear the stale spare candidate (captured for the old character before this
/// confirm) so the teardown-spare re-targets the new slot, and drop the depth-mask cache so the new
/// silhouette is computed fresh rather than bridged from the old head.
pub(crate) unsafe fn portrait_retarget_and_rearm_for_switch(selected_slot: i32, source: &str) {
    PROFILE_SPARE_CANDIDATE.store(0, Ordering::SeqCst);
    PROFILE_SPARE_CANDIDATE_MODEL.store(0, Ordering::SeqCst);
    PROFILE_BAKE_RGBA_CAPTURED.store(0, Ordering::SeqCst);
    invalidate_portrait_depth_mask();
    // Orphan reclaim at switch arm (second-load foreign-head fix, pixel-proven 2026-07-06 run
    // jsm-slotstats2-switchqa). The prior window's spared renderer parks in PROFILE_SPARE_ORPHAN at the
    // load-complete reset and was only delete-enqueued inside profile_renderer_teardown_spare_hook --
    // but the System-Quit switch path never fires that native teardown-all (spare_hits stayed 1,
    // orphans_deleted 0 across the whole run), so the orphan lived through the next loading window with
    // its model + offscreen scene still registered, rendering the previous character's head every frame.
    // The new window's readback then published that head under the correctly-kicked new renderer
    // (window-2 RT dump structure-correlated 0.92 with the window-1 character). Reclaim it here, on the
    // game thread at the confirm press (same delay-delete path as the spare hook), so the new window's
    // offscreen render belongs to the new character alone.
    let orphan = PROFILE_SPARE_ORPHAN.swap(0, Ordering::SeqCst);
    if orphan != 0 {
        let deleted = unsafe { delay_delete_enqueue_renderer(orphan) };
        ownership_release(OwnedClass::SparedRenderer);
        append_autoload_debug(format_args!(
            "loading-portrait: reclaimed prior spared renderer 0x{orphan:x} at switch confirm via CSDelayDeleteMan enqueued={deleted} (second-load foreign-head fix)"
        ));
    }
    PROFILE_PORTRAIT_RETARGETS.fetch_add(1, Ordering::SeqCst);
    // OFFSCREEN-size row first (2026-07-30, different-slot 256x256 root cause): the selected slot is
    // known here, before any of this switch's profile-table builds (ours at the loading screen and the
    // native TitleTopDialog rebuild at the title transition). Patch its offscreen-size row now so every
    // renderer constructed for this switch snapshots the full-size RT; waiting for the loaded-slot flip
    // left the row native 128 until after both builds (run 20260730-202840).
    {
        let base = er_game_base::mem::game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        if base != 0 && base != TITLE_OWNER_SCAN_START_ADDRESS {
            let patched = unsafe { patch_profile_offscreen_size_for_slot(base, selected_slot) };
            if !patched {
                append_autoload_debug(format_args!(
                    "loading-portrait: offscreen-size row patch DID NOT land for selected slot {selected_slot} at retarget -- switch-window portrait will build native-size (256) until a later patch succeeds"
                ));
            }
        }
    }
    // Confirm TIMESTAMP (bd er-effects-rs-dpf6 Phase 1): the first portrait publish after this confirm
    // consumes it to measure oracle_portrait_confirm_to_publish_ms (the publish-race latency).
    PORTRAIT_CONFIRM_MS.store(
        crate::experiments::boot_view_epoch_ms().max(1) as usize,
        Ordering::SeqCst,
    );
    append_autoload_debug(format_args!(
        "loading-portrait: RETARGET to selected slot {selected_slot} at confirm (make-before-break: drive re-engaged, prior masked head holds until the new keyed frame; source={source})"
    ));
    // Reopen our cover for this switch. It is cover machinery, not switch machinery -- the row
    // works without it and the player sees the game's own loading screen instead.
    #[cfg(feature = "loading-cover")]
    rearm_boot_progress_for_own_menu_load(selected_slot, source);
}

/// Hand System->Quit->Load Character back so it works a second time, and a third.
///
/// Everything this touches is a one-shot the switch just spent. The teardown that clears the old
/// world runs exactly once per session unless these are handed back: the native return-title
/// request fires only while `RETURN_TITLE_REQUEST_COUNT == 0`, the menu-pump submit only while
/// `DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT == 0`, and the final functor is a
/// `FINAL_FUNCTOR_CALL_COUNT` compare_exchange 0 -> 1. Spent, the next switch arms, reports
/// `direct_chain_submitted=true` without submitting anything, and never asks for a teardown: the
/// character you were playing stays standing, no loading screen appears, and the third character
/// never loads.
///
/// The menu-window trackers are the same failure wearing its UI face. They still point at the
/// IngameTop/OptionSetting/ProfileSelect windows this switch destroyed, and the quit menu's hide
/// keys off a tracked window being valid -- so on the next open the vtable read on a torn-down
/// window fails, the menu does not hide behind ProfileSelect, it draws on top of a dead one, and
/// its rows do nothing. Resetting them makes the next quit-menu open repopulate through the
/// MenuWindowJob::Run hook, exactly as it did the first time.
///
/// When it is safe to call. Only once this switch's return-title machinery is fully consumed --
/// i.e. after the load has been committed. Resetting at arm time was tried and reverted on
/// 2026-07-02 (the commented-out pair below): the counters are re-consumed during the teardown
/// still in flight, the chain double-submits, and even a single switch bounces back to the title.
/// At the commit point both remaining gates are independently shut -- `SYSTEM_QUIT_QUICKLOAD_PHASE`
/// is idle (every return-title gate requires >= RETURN_TITLE_REQUESTED) and `GameMan+0xbc4` is 0
/// (the final functor requires ready) -- so handing the counters back opens nothing until the next
/// arm deliberately re-opens it.
pub(crate) unsafe fn system_quit_rearm_switch_for_next_load(source: &str) {
    let spent = (
        SYSTEM_QUIT_QUICKLOAD_RETURN_TITLE_REQUEST_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT.load(Ordering::SeqCst),
    );
    SYSTEM_QUIT_QUICKLOAD_RETURN_TITLE_REQUEST_COUNT.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT.store(0, Ordering::SeqCst);
    unsafe { system_quit_reset_profile_select_state(source) };
    SYSTEM_QUIT_INGAME_TOP_WINDOW.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_OPTION_SETTING_WINDOW.store(0, Ordering::SeqCst);
    // `load=` is not decoration. Every switch hands back the same values (1/1/1), so without a
    // per-call discriminator the second call's line is byte-identical to the first and the debug
    // log's repeat filter SUPPRESSES it outright -- silently, because two occurrences is below its
    // first restatement milestone. Measured on run br-20260905-215045-c4c6: the re-arm ran for both
    // switches (all three counters read 0 afterwards) and the log showed it once, which reads as
    // "it did not run for switch #2". ALLOW_COUNT is the forwarded-confirm total, so it already
    // differs per switch and needs no new state.
    let load = SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT.load(Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-quickload: System->Quit->Load Character re-armed for the next load (load={load} source={source}) -- handed back the return-title one-shots this switch spent (request={} chain_submit={} final_functor={} -> 0/0/0) and dropped the destroyed IngameTop/OptionSetting/ProfileSelect window trackers; without this the next switch submits nothing, never tears the old world down, and opens its quit menu over a dead ProfileSelect",
        spent.0, spent.1, spent.2
    ));
}

pub(crate) unsafe fn system_quit_arm_quickload_autoload(selected_slot: i32, source: &str) {
    const NO_SLOT: usize = usize::MAX;
    if selected_slot < 0 {
        append_autoload_debug(format_args!(
            "system-quit-quickload: not arming autoload from {source} -- invalid selected_slot={selected_slot}"
        ));
        return;
    }
    let system_dialog = SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.load(Ordering::SeqCst);
    if system_dialog == 0 || system_dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.store(NO_SLOT, Ordering::SeqCst);
        SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-quickload: not arming direct native chain from {source} -- missing preserved original System dialog selected_slot={selected_slot}"
        ));
        return;
    }
    // Disabled (2026-07-01): the CSGaitemImp deserialize/lookup/finalize guards only ever corrupt
    // the gaitem singleton -- emptying gaitemInsTable handles left a garbage non-canonical entry that
    // crashed GetGaitemIns->GetGaitemHandle (live 0x6710c0). They were a doomed attempt to make the
    // in-world load "safe"; we now block the in-world load-job entirely (see the robust gate in
    // system_quit_profile_load_job_run_hook) and return to title + autoload instead, so no in-world
    // gaitem deserialize should run. Leaving them installed would additionally corrupt the AUTOLOAD's
    // own post-title load whenever it deserializes while phase is still 1..3. Not installing them lets
    // every real deserialize run natively. (Install fns retained for reference / bisecting.)
    // Install the load-only guard so the picked slot is not deserialized into the still-live world
    // when the native confirm arms the load; it forwards the real load at a clean title (autoload).
    install_system_quit_inworld_load_guard();
    // Install the in-world load request guard: neutralizes the native RequestLoadSlot (FUN_14067b2f0)
    // so GameMan.saveState/b80 never reaches 2 during the switch. This is the true source of the
    // NowLoading transition that froze the menu pump; blocking it here (not reactively) lets the
    // menu-pump-owned return-title chain run + tear the world down. Forwarded at a clean title.
    install_system_quit_request_load_slot_guard();
    // Reverted 2026-07-16: wiring install_system_quit_gaitem_deserialize_hook() here (to backstop the
    // 0x67141a stale-table AV) was worse -- its handler's skip path during the return-title transition
    // (phase confirmed..HANDOFF) leaves the gaitem singleton inconsistent and the game DL_PANICs from inside
    // the gaitem code (crash stk showed the panic called from game+0x671843). That broke the first switch
    // (DL_PANIC before the load) whereas without the hook the first switch completes via continue_confirm.
    // The 0x67141a crash on the 2nd+ consecutive switch remains a separate, pre-existing issue to solve
    // without this skip-based hook (2026-07-01 already noted these gaitem guards corrupt the singleton).
    // Re-arm the continue_confirm guard's one-shot: the upcoming clean-title confirm must drive a
    // fresh deserialize of this switch's picked slot before it streams (the hook itself is installed
    // unconditionally at attach; see install_system_quit_continue_confirm_hook).
    SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.store(0, Ordering::SeqCst);
    // Re-arm the menu-free clean-title switch reload one-shot so every switch (not just the first) can
    // drive its own picked-slot feed-deserialize -> continue_confirm (own_load_switch_reload_fire).
    SYSTEM_QUIT_SWITCH_MENU_FREE_RELOAD_FIRED.store(0, Ordering::SeqCst);
    // Reset the switch-reload FD4-IO phase + Phase-3 outgoing-teardown latches. The user ProfileSelect arm
    // was missing these (only the since-deleted menu-free arm had them), so a user-driven consecutive load
    // inherited SWITCH_RELOAD_FD4IO_COMMITTED=1 stale from the prior load -> own_load_switch_reload_fire
    // short-circuited at the already-committed guard -> no submit -> FRESH_DESER_DONE stuck 0 -> the b78
    // guard wrote GameMan requestedSaveSlotLoad=-1 every frame -> native pump gate false -> world torn down
    // at entering world = the load3 softlock (bd compounding-reload-two-roots-...-chainB-stale-fd4io-latch-b78-2026-07-23).
    // These are the FD4IO/OUTGOING latches the deleted menu-free arm already reset safely -- Not the
    // RETURN_TITLE/FINAL_FUNCTOR counters that the 2026-07-02 bisect below found regressive.
    crate::experiments::own_load::reset_switch_reload_latches();
    // Re-arm the return-title one-shots so every switch (not just the first) tears the world down.
    // Both are consumed by the first switch and never reset otherwise, so a second switch in the same
    // session would skip the native return-title request (`== 0` gate, sets saveRequested+bc4=1) and
    // the final-functor submit (compare_exchange 0->1 gate), leaving the second switch stuck in-world.
    // Resetting them here (the per-switch arm point) is the durable fix for repeatable switching
    // (er-effects-rs-qwj). SUBMIT_COUNT is intentionally not reset: title.rs uses it as a `> 0` enable
    // and it re-increments before the final functor needs it.
    // BISECT 2026-07-02: these two resets regressed even the single-switch reload (base f59b2af
    // passes, adding them causes a second title bounce after the load / new-game flash). Disabled
    // while isolating; a switch-#2-safe re-arm will be reinstated once the mechanism is understood.
    // SYSTEM_QUIT_QUICKLOAD_RETURN_TITLE_REQUEST_COUNT.store(0, Ordering::SeqCst);
    // SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.store(selected_slot as usize, Ordering::SeqCst);
    // SPURIOUS-vs-genuine arm discriminator (2026-07-18). Record whether the local player is absent at
    // the instant of the arm. A spurious boot self-reload arms from the title/menu (player absent) while a
    // genuine in-world switch arms with the player present. The in-world time-based disarm keys on this so
    // it only cancels the spurious boot self-reload, never a real switch. See profile_render.rs
    // SYSTEM_QUIT_ARM_PLAYER_WAS_ABSENT and bd repeatable-multi-save-consolidated-plan-2026-07-18.
    let arm_player_absent = unsafe { PlayerIns::local_player_mut() }.is_err();
    SYSTEM_QUIT_ARM_PLAYER_WAS_ABSENT.store(usize::from(arm_player_absent), Ordering::SeqCst);
    unsafe { portrait_retarget_and_rearm_for_switch(selected_slot, source) };
    SYSTEM_QUIT_QUICKLOAD_PHASE.store(SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED, Ordering::SeqCst);
    OWN_STEPPER_SLOT.store(selected_slot, Ordering::SeqCst);
    PRODUCT_AUTOLOAD_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_MENU, Ordering::SeqCst);
    TFC_CONTINUE_FIRED.store(0, Ordering::SeqCst);
    TFC_FORCED_CONTINUE_HANDOFF_MS.store(0, Ordering::SeqCst);
    OWN_LOAD_FORCED_CONTINUE_HANDOFF_MS.store(0, Ordering::SeqCst);
    TFC_LOAD_VEC_WAIT_TICKS.store(0, Ordering::SeqCst);
    OWN_STEPPER_MENU_OPENED.store(OWN_STEPPER_MENU_OPENED_NO, Ordering::SeqCst);
    TITLE_ACCEPT_BYTE_GATE_FIRED.store(false, Ordering::SeqCst);
    TITLE_OWNER_PTR.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    TITLE_OWNER_SCAN_COUNTDOWN.store(TITLE_OWNER_SCAN_COUNTDOWN_READY, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_FIRED.store(false, Ordering::SeqCst);
    // Re-arm the product-core-autoload continue driver for repeatable switching (2026-07-15): FULLREAD_PHASE is
    // a one-shot that reaches FULLREAD_PHASE_DONE after the first switch's Continue and then early-returns
    // (product_continue.rs:282), so the 2ND consecutive switch's return-title reaches the title but nothing
    // drives its native Continue -> stuck at the covered title (the "black screen"). Reset the phase to submit
    // and drop the stale MENU_CONTINUE_* row/router pointers captured for the previous switch's (torn-down)
    // menu, so the driver re-captures + re-submits the Continue for this switch. Idempotent one-shots reset to
    // their init values, exactly like the per-switch latches above; the continue_confirm/world-up guards still
    // prevent driving a load into a live world. (Distinct from the return-title one-shots at 868-869, which the
    // 2026-07-02 bisect showed regress the single switch -- those stay disabled.)
    FULLREAD_PHASE.store(FULLREAD_PHASE_SUBMIT, Ordering::SeqCst);
    FULLREAD_DRAIN_WAITS.store(0, Ordering::SeqCst);
    MENU_CONTINUE_ITEM.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    MENU_CONTINUE_ENTRY.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    MENU_CONTINUE_FUNCTOR.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    MENU_CONTINUE_DOCALL.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    MENU_CONTINUE_ROUTER.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    MENU_CONTINUE_INDEX.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_LAST_TITLE_OWNER.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_AUTOLOAD_HANDOFF_COUNT.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_LOAD_JOB_POST_RETURN_TITLE_FIRED.store(0, Ordering::SeqCst);
    PROFILE_REFRESH_KICKED.store(0, Ordering::SeqCst);
    PORTRAIT_RENDER_WINDOW_DONE.store(0, Ordering::SeqCst);
    // Reset the switch-outcome oracle so this switch's classification starts fresh (see the atomics' doc).
    SWITCH_ORACLE_TICK.store(0, Ordering::SeqCst);
    SWITCH_ORACLE_STABLE_FRAMES.store(0, Ordering::SeqCst);
    SWITCH_ORACLE_MAX_STABLE_FRAMES.store(0, Ordering::SeqCst);
    // Switch-2 soft-lock fix (arm-point pre-clear, 2026-07-16). RE of the 1.16.1 dump proved the switch-2
    // freeze is the native quit-save (`ShouldSave` 0x1406794c0) aborting on a stale
    // `CSMenuMan->disableSaveMenu` (+0x13c, read by `CanShowSaveMenu` 0x14080d150) left set from the prior
    // switch's menu flow -- so `bc4` freezes at 1 and the world never tears down. Clear it here, the moment
    // this switch arms its return-title, so the gate is already open before the quit-save orchestrator runs
    // (belt-and-suspenders with the per-frame game-task clear in product_core_autoload_tick and the menu-pump
    // clear in system_quit_restore_real_system_windows). No-op / inert on switch 1 (its byte is already 0);
    // reuses the shared startup_hooks helper. SYSTEM_QUIT_DISABLE_SAVE_MENU_CLEAR_COUNT is the runtime
    // semaphore: >0 on a switch == that switch's quit-save was gated off and we unblocked it.
    let base = er_game_base::mem::game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    if base != TITLE_OWNER_SCAN_START_ADDRESS {
        let dsm_prev =
            unsafe { system_quit_clear_disable_save_menu(base, "arm-quickload-return-title") };
        append_autoload_debug(format_args!(
            "system-quit-quickload: arm pre-clear CSMenuMan->disableSaveMenu was {dsm_prev} (>0 = switch-2 quit-save was BLOCKED; cleared so bc4 can pump 1->2->3 and the world tears down) selected_slot={selected_slot} source={source}"
        ));
    }
    SYSTEM_QUIT_QUICKLOAD_PHASE.store(
        SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED,
        Ordering::SeqCst,
    );
    append_autoload_debug(format_args!(
        "system-quit-quickload: armed product Continue autoload selected_slot={selected_slot} source={source}; will direct-submit native return-title chain once ProfileSelect closes system_dialog=0x{system_dialog:x}"
    ));
}

/// Guard on the load-only routine `FUN_14067b380(slot)`. While the in-world System->Quit->Load-Profile
/// transition is active (phase in confirmed..AUTOLOAD_HANDOFF) and the old world is still up (local
/// player present), skip the deserialize+warp and report success -- so `DoSaveStuff` completes (clears
/// its pending slot) and ProfileSelect closes, but nothing loads into the live world. At a clean title
/// (player absent, or phase past the transition) it forwards to the real load so the autoload works.
///
/// Union-shaped, not `fn(i32)` (2026-08-31). The native takes one integer in ECX and touches no
/// XMM register (checked on the 1.17 image at `0x14067c0e0`: 112 instructions, zero XMM), so the
/// union's four-integer signature fits it -- `b`/`c`/`d` are whatever the caller happened to leave
/// in RDX/R8/R9 and are forwarded untouched. The shape matters because `orig` may be the next
/// handler in the chain rather than the game trampoline, and a next handler must be called through
/// the four-argument signature.
pub(crate) unsafe extern "system" fn system_quit_inworld_load_skip_hook(
    slot_arg: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let slot = slot_arg as u32 as i32;
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    let in_transition = (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED
        ..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase);
    let world_up = unsafe { PlayerIns::local_player_mut() }.is_ok();
    if in_transition && world_up {
        let n = SYSTEM_QUIT_INWORLD_LOAD_SKIP_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        append_autoload_debug(format_args!(
            "system-quit-quickload: in-world load SKIPPED #{n} slot={slot} phase={phase} (old world still up) -- ProfileSelect close proceeds; return-title tears down; autoload loads at clean title"
        ));
        // FUN_14067b380 returns 1 on success; report success without deserializing so DoSaveStuff's
        // caller advances (it then clears MoveMapStep+0x12c) instead of retrying the in-world load.
        return 1;
    }
    SYSTEM_QUIT_INWORLD_LOAD_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_INWORLD_LOAD_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return 0;
    }
    let original: crate::mh::UnionFn = unsafe { std::mem::transmute(orig) };
    let ret = unsafe { original(slot_arg, b, c, d) };
    let selected = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
    if ret != 0 && selected < TITLE_PROFILE_SLOT_COUNT && slot == selected as i32 {
        SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.store(1, Ordering::SeqCst);
        // Which slot's deserialize completed (slot+1) -- ground truth for the published-vs-loaded
        // portrait oracle. See er_telemetry_core counters.
        er_telemetry_core::counters::SYSTEM_QUIT_FRESH_DESER_DONE_SLOT
            .store((slot + 1) as usize, Ordering::SeqCst);
        SYSTEM_QUIT_QUICKLOAD_PHASE.store(SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE, Ordering::SeqCst);
        let gm = game_man_ptr_or_null();
        if gm != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe {
                *((gm + GAME_MAN_REQUESTED_SLOT_B78_OFFSET) as *mut i32) = OWN_STEPPER_SLOT_NONE;
            }
        }
        if let Ok(gm_typed) = unsafe { eldenring::cs::GameMan::instance_mut() } {
            er_save_loader::GameManSaveAccess::set_save_requested(gm_typed, false);
        }
        let n = SYSTEM_QUIT_INWORLD_LOAD_ALLOW_COUNT.load(Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-quickload: native slot deserialize proof OK via 0x67b290 slot={slot} ret={ret} prior_phase={phase} world_up={world_up} allow_count={n} -> phase IDLE, cleared GameMan+0xb78/save_requested; native owns warp_requested finalize/autoclear"
        ));
    }
    ret
}

/// This guard registers through the union, and until 2026-08-31 it did not -- so it did not exist.
///
/// A bare `MhHook::new` here lost the address to the product's own menu-trace observer
/// (`b80_deserialize_67b290`, `experiments/trace/menu_trace_hooks.rs`), which unions
/// `DESERIALIZE_SLOT_RVA` -- the same `0x67b290` -- at boot. Measured in run
/// `br-20260831-160354-2513`: `register_union_hook` translated it to `0x14067c0e0` at +1288ms, this
/// installer arrived later, `MH_CreateHook` answered `MH_ERROR_ALREADY_CREATED`, and the guard was
/// simply absent for the rest of the session (`system_quit_inworld_load_skip_count = 0`). Nothing
/// crashed; the picked slot was free to deserialize into the still-live world.
///
/// `mh_install_hook_once` is the right primitive rather than a permanent `*_CLAIMED.swap(1)`,
/// because this installer is lazy and REPEATED: `system_quit_arm_quickload_autoload` calls it every
/// time a switch arms, and a failure there (module base not yet readable, a refused address) must
/// leave the next arm free to retry. The permanent-claim idiom is for installers whose every
/// reachable failure is permanent.
pub(crate) fn install_system_quit_inworld_load_guard() {
    // Unresolved on purpose: `register_union_hook` inside `mh_install_hook_once` owns the single
    // 1.16.2 -> 1.17 resolve. `game_rva` here would be the double-resolve shape
    // `scripts/check-double-resolved-hook-targets.py` refuses.
    let Ok(addr) = game_rva_for_hook(SYSTEM_QUIT_INWORLD_LOAD_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve in-world load rva 0x{SYSTEM_QUIT_INWORLD_LOAD_RVA:x}"
        ));
        return;
    };
    mh_install_hook_once(
        &SYSTEM_QUIT_INWORLD_LOAD_INSTALLED,
        SYSTEM_QUIT_INWORLD_LOAD_NOT_INSTALLED,
        SYSTEM_QUIT_INWORLD_LOAD_INSTALLED_YES,
        addr,
        system_quit_inworld_load_skip_hook as *mut c_void,
        &SYSTEM_QUIT_INWORLD_LOAD_ORIG,
        "in-world load guard",
    );
}

/// Guard on the native in-world load request `CS::GameMan::RequestLoadSlot(slot)` (FUN_14067b2f0, live
/// 0x67b200). This is the true source of GameMan.saveState/b80=2 for an explicit-slot in-world load:
/// the per-frame MoveMapStep load steps call it once the confirmed ProfileSelect chain pushes the map
/// machine into loading, and it sets saveState=2, which starts the 02_904_NowLoading transition that
/// freezes the menu pump so the queued return-title chain can never run. During the in-world
/// System->Quit->Load-Profile transition (phase active and old world still up / local player present)
/// we return "not armed" (0) without calling the original, so saveState never reaches 2: no NowLoading,
/// the pump keeps running, and the menu-pump-owned return-title chain tears the world down. Once the
/// world is gone (player absent) or the switch is idle, we forward to the real request -- so the
/// clean-title autoload and any normal load work. The boot/Continue autoload uses the distinct sentinel
/// variants (FUN_14067b290 slot 10 / FUN_14067b570 slot 0xb), which this hook does not touch. See bd
/// system-quit-loadjob-success-commits-phantom-load-2026-07-01.
///
/// Union-shaped for the same reason as its in-world-load sibling: one integer in ECX, zero XMM in
/// the whole body (checked on the 1.17 image at `0x14067c050`: 40 instructions), and `orig` may be
/// the next chained handler rather than the game trampoline.
pub(crate) unsafe extern "system" fn system_quit_request_load_slot_hook(
    slot_arg: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let slot = slot_arg as u32;
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    // Range-gate like the sibling system_quit_inworld_load_skip_hook (not `!= IDLE`): the clean-title
    // reload runs at AUTOLOAD_HANDOFF and re-creates a present player, so a `!= IDLE` gate would
    // neutralize the reload's own RequestLoadSlot mid-load. Neutralize only during the first-world
    // transition [confirmed, AUTOLOAD_HANDOFF); forward natively at AUTOLOAD_HANDOFF so the reload loads.
    let switch_active = (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED
        ..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase);
    let world_up = unsafe { PlayerIns::local_player_mut() }.is_ok();
    if switch_active && world_up {
        let n = SYSTEM_QUIT_REQUEST_LOAD_SLOT_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 8 || n.is_multiple_of(120) {
            append_autoload_debug(format_args!(
                "system-quit-quickload: in-world load REQUEST neutralized #{n} slot={slot} phase={phase} (old world still up) -- saveState/b80 kept idle so no NowLoading; return-title tears down + autoload loads at clean title"
            ));
        }
        // RequestLoadSlot returns 0 when it declines to arm (saveState!=0 or profile check fails). We
        // return the same "not armed" result so the caller MoveMapStep treats it as no-load-yet instead
        // of entering the in-world load transition.
        return 0;
    }
    SYSTEM_QUIT_REQUEST_LOAD_SLOT_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_REQUEST_LOAD_SLOT_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return 0;
    }
    let original: crate::mh::UnionFn = unsafe { std::mem::transmute(orig) };
    unsafe { original(slot_arg, b, c, d) }
}

/// Registers through the union, for the reason spelled out on
/// [`install_system_quit_inworld_load_guard`]. This one lost `0x67b200` to the menu trace's
/// `b80_loadsavedata_67b200` observer, translated to `0x14067c050` at +1172ms in run
/// `br-20260831-160354-2513`, leaving `system_quit_request_load_slot_block_count` and
/// `..._allow_count` both at zero -- the guard was never in the process.
pub(crate) fn install_system_quit_request_load_slot_guard() {
    // Unresolved on purpose -- see the sibling installer.
    let Ok(addr) = game_rva_for_hook(SYSTEM_QUIT_REQUEST_LOAD_SLOT_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve RequestLoadSlot rva 0x{SYSTEM_QUIT_REQUEST_LOAD_SLOT_RVA:x}"
        ));
        return;
    };
    mh_install_hook_once(
        &SYSTEM_QUIT_REQUEST_LOAD_SLOT_INSTALLED,
        SYSTEM_QUIT_REQUEST_LOAD_SLOT_NOT_INSTALLED,
        SYSTEM_QUIT_REQUEST_LOAD_SLOT_INSTALLED_YES,
        addr,
        system_quit_request_load_slot_hook as *mut c_void,
        &SYSTEM_QUIT_REQUEST_LOAD_SLOT_ORIG,
        "RequestLoadSlot guard",
    );
}

/// Guard on the native title Continue confirm `0x140b0e180` (rcx = the {[+8]=owner} shim; reads
/// GameMan+0xc30 -> owner+0xbc -> SetState(5); picks no slot). Static RE 2026-07-02 proved the
/// post-switch clean-title reload streams the pre-switch GameMan/PlayerGameData state: no fresh
/// deserialize of the picked slot runs anywhere on that path, so the resident (original) character
/// gets re-streamed -- the wrong-character bug. While a System->Quit->Load-Profile switch is active
/// this hook drives one fresh synchronous feed-deserialize of the picked slot
/// (`own_load_feed_deserialize`: on-disk read -> gated 0x67b100 feed -> native parser 0x67b290)
/// before forwarding, so ac0/c30/PGD all become the picked slot and the confirm streams the right
/// character. Fail-closed: if the fresh deserialize cannot be proven, the confirm is blocked --
/// streaming stale state would load the wrong character and the post-load autosave would then write
/// it back into the picked slot. Boot autoloads and normal play (phase idle) pass through
/// untouched. See bd system-quit-cleantitle-load-is-stale-restream-not-slot-source-2026-07-02.
pub(crate) unsafe extern "system" fn system_quit_continue_confirm_hook(
    shim: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    // Render-HANDOFF fix arm (bd er-effects-rs-um9g): this Continue/Load confirm is the common trigger
    // for both the boot autoload and the in-world reload; it captures GameMan+0xc30 into the TitleStep,
    // then forwards to SetState5 -> STEP_PlayGame -> InGameStep::RequestMoveMap. On our redirect load the
    // captured BlockId can be stale/-1, which makes RequestMoveMap skip building the world-res loadlist
    // path and stall at WorldResWait. Arm the RequestMoveMap fixup here so the upcoming RequestMoveMap
    // substitutes the freshly-deserialized saved-map BlockId (armed-only + invalid-param2-only, so it is
    // a no-op for a load whose BlockId is already valid).
    crate::experiments::own_load::arm_request_move_map_fixup();
    // Continue-trace compat: this unconditional hook replaced the trace-set `cap_continue_confirm`
    // hook on the same address (two MinHooks on one target fail -- the install_c30_writer_hook
    // precedent), so reproduce its logging + confirm latch exactly when tracing is on.
    if trace_continue_enabled() {
        let owner = if shim != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe {
                safe_read_usize(shim + OWN_STEPPER_SHIM_OWNER_IDX * core::mem::size_of::<usize>())
            }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        append_continue_trace(format_args!(
            "CAP continue_confirm this=0x{shim:x} owner=0x{owner:x} {} {}",
            trace_callers_summary(),
            b80_mount_trace_summary()
        ));
        OWN_STEPPER_CONFIRMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    // Inclusive of AUTOLOAD_HANDOFF (unlike the in-world guards' half-open range): the clean-title
    // reload's confirm fires at TITLE_OWNER_SEEN or AUTOLOAD_HANDOFF and the fresh deserialize is
    // exactly what phase 4 needs; the one-shot done latch prevents repeats after success.
    let switch_active = (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED
        ..=SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase);
    if switch_active {
        let selected = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
        let world_up = unsafe { PlayerIns::local_player_mut() }.is_ok();
        if world_up {
            // A title-flow confirm while the old world is still up is not a state we ever drive;
            // never deserialize into a live world (that is the crash the whole switch avoids).
            // Forward and log loudly -- the in-world load guards protect the load paths.
            // Counted, not just logged: this forward reaches the unconditional allow increment
            // below, so without its own bucket it would break the load-count decomposition
            // (`allow == fresh_deser + non_switch + world_up`) with no way to tell which bucket lost
            // it. See er_telemetry_core::load_count.
            SYSTEM_QUIT_CONTINUE_CONFIRM_WORLD_UP_COUNT.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "system-quit-quickload: continue_confirm called while OLD WORLD STILL UP phase={phase} selected={selected} shim=0x{shim:x} -- forwarding WITHOUT fresh deserialize (unexpected caller)"
            ));
        } else if SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst) == 0
            && selected >= TITLE_PROFILE_SLOT_COUNT
        {
            let n = SYSTEM_QUIT_CONTINUE_CONFIRM_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            append_autoload_debug(format_args!(
                "system-quit-quickload: continue_confirm BLOCKED #{n} -- switch active (phase={phase}) but no valid picked slot ({selected}); refusing to stream stale pre-switch state"
            ));
            return 0;
        } else {
            let slot = selected as i32;
            let base =
                er_game_base::mem::game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let _gm = game_man_ptr_or_null();
            let native_slot_proven =
                SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst) == 1;
            append_autoload_debug(format_args!(
                "system-quit-quickload: continue_confirm intercepted at clean title phase={phase} slot={slot} native_slot_proven={native_slot_proven} shim=0x{shim:x}"
            ));
            if !native_slot_proven {
                // Do not disarm GameMan+0xb78 here. The 2026-07-30 slot-1 repro hit this unproven
                // branch, cleared the requested slot to -1, then waited forever for stable-world proof
                // that the native stream could no longer reach. The same b78-as-warp-target rule below
                // applies even when the earlier proof latch missed.
                // The `#n` label comes from this branch's own counter. It used to come from
                // ALLOW_COUNT, which also increments unconditionally at the tail of this hook -- so
                // one call incremented the total-load witness twice and inflated it by one per
                // unproven reload. Both captured runs had native_slot_proven=true throughout, which
                // is the only reason their allow counts were exact.
                let n = SYSTEM_QUIT_CONTINUE_CONFIRM_UNPROVEN_FORWARD_COUNT
                    .fetch_add(1, Ordering::SeqCst)
                    + 1;
                SYSTEM_QUIT_QUICKLOAD_PHASE.store(
                    SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF,
                    Ordering::SeqCst,
                );
                append_autoload_debug(format_args!(
                    "system-quit-quickload: continue_confirm FORWARD #{n} -- native requested-slot proof did not fire for slot={slot}; leaving GameMan+0xb78 armed as the warp target and holding phase at AUTOLOAD_HANDOFF until stable-world proof fires (runtime evidence: clearing b78 here strands the native stream; setting DONE/IDLE here lets the next switch overlap unfinished MoveMap)"
                ));
            }
            {
                let n = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT
                    .fetch_add(1, Ordering::SeqCst)
                    + 1;
                // Load committed -> get out of the way. The forwarded continue_confirm below fires
                // SetState5, which streams the picked character. Return the switch machine to idle so
                // the product-core autoload's switch branch stops (title.rs: it keeps arming
                // GameMan+0xb78 = an in-world MoveMapStep load of the slot, and keeps re-driving the
                // title, while phase >= RETURN_TITLE_REQUESTED). Left armed, that redundant b78 load
                // competes with this SetState5 stream, stalls the title owner at state 6, and bounces
                // the freshly-loaded world back to the title ~4s later (the post-load instability the
                // earlier single-switch milestone missed -- it tore down before the bounce). Idle also
                // makes the in-world load guards inert (they gate on [confirmed, AUTOLOAD_HANDOFF)), so
                // the native world stream is unobstructed, and leaves the session clean for the next
                // switch (also the durable fix for the post-switch hygiene issue er-effects-rs-qwj).
                SYSTEM_QUIT_QUICKLOAD_PHASE.store(
                    SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF,
                    Ordering::SeqCst,
                );
                // Keep GameMan+0xb78 armed through SetState5/MoveMap finalize. Runtime evidence from
                // samechar-3x-phaseearly shows clearing b78 to -1 before the warp finalize leaves the
                // loaded target without a valid requested-slot/warp target and TitleStep falls back to
                // title. A later post-resident proof point must clear it, but not before the world stream
                // has survived.
                // Clear the return-title "rebuild the title" request flags the final functor set for this
                // switch's teardown (restored 2026-07-16 after the defer experiment failed: the stuck load
                // has menuData+0x5d==0 to begin with -- the functor never set it, incomplete teardown -- so
                // deferring our clear was inert). They are level flags nothing resets; left set on a
                // resident world they re-request quit-to-title (bounce). Undo them at the clean-title
                // Continue as before.
                let menu_man = unsafe {
                    safe_read_usize(er_game_base::mem::game_data_addr(
                        base,
                        CS_MENU_MAN_GLOBAL_RVA,
                        "CS_MENU_MAN_GLOBAL_RVA",
                    ))
                }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                if menu_man != TITLE_OWNER_SCAN_START_ADDRESS
                    && unsafe { is_heap_aligned_ptr(menu_man) }
                    && let Some(menu_data) =
                        unsafe { safe_read_usize(menu_man + CS_MENU_MAN_MENU_DATA_OFFSET) }
                    && menu_data != TITLE_OWNER_SCAN_START_ADDRESS
                    && unsafe { is_heap_aligned_ptr(menu_data) }
                {
                    unsafe {
                        *((menu_data + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET) as *mut u8) = 0;
                        *((menu_data + CS_MENU_DATA_ENDING_FLAG_5E_OFFSET) as *mut u8) = 0;
                    }
                }
                unsafe {
                    er_game_base::mem::write_global_u8(
                        base,
                        RETURN_TITLE_REBUILD_FLAG_DAT_RVA,
                        "RETURN_TITLE_REBUILD_FLAG_DAT_RVA",
                        0,
                    )
                };
                // Clear GameMan.save_requested defensively (typed): the return-title request set it for
                // the teardown; a residual true would drive an immediate quit-save on the reload. Do not
                // clear GameMan.warp_requested here. Native full deserialize owns that flag; MoveMapStep
                // finalize case 8 consumes/autoclears it after advancing mms18.
                if let Ok(gm_typed) = unsafe { eldenring::cs::GameMan::instance_mut() } {
                    er_save_loader::GameManSaveAccess::set_save_requested(gm_typed, false);
                }
                // REPEATABLE-switch state restore (er-effects-rs-qwj). The switch-#1 works but
                // switch-#2-stalls symptom is a pure precondition mismatch: these three return-title
                // one-shots are consumed by this switch's teardown and gate the next switch --
                // RETURN_TITLE_REQUEST_COUNT (native return-title request fires only when ==0,
                // startup_hooks 6922), DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT (menu-pump submit only
                // when ==0, 7162), FINAL_FUNCTOR_CALL_COUNT (final-functor compare_exchange 0->1,
                // title.rs 1690). Left set, switch #2 skips its return-title request + submit and
                // never tears the world down (observed: stuck at title state 10/10, bc4=0). Restoring
                // them to boot-fresh here makes every switch byte-identical to the first. This is the
                // safe edge (unlike the disabled arm-time reset above, which re-fires during teardown
                // and double-submits -> the single-switch bounce that regressed it): it runs once per
                // switch (fresh-deser latch), after this switch's return-title machinery is fully
                // consumed. The phase remains AUTOLOAD_HANDOFF until the streamed load reaches a stable
                // world, so every return-title REQUEST/submit/final-functor gate must exclude
                // AUTOLOAD_HANDOFF; otherwise the reset counts can be consumed by a spurious second
                // return-title request that leaves bc4=3 stale and blocks the incoming MoveMap finalize.
                //
                // This branch is not the only edge any more, and on the product path it is not even the
                // reached one -- see the same call at the end of `own_load_switch_reload_fire`. It stays
                // here for the confirm that arrives without our own feed having run first; the shared
                // function is what stops the two drifting.
                unsafe {
                    system_quit_rearm_switch_for_next_load("post-switch-commit-menu-hygiene")
                };
                append_autoload_debug(format_args!(
                    "system-quit-quickload: native Continue handoff commit OK #{n} slot={slot} -- forwarding continue_confirm so SetState5 streams; phase stays AUTOLOAD_HANDOFF until stable-world proof + keep GameMan+0xb78 armed through native finalize + cleared return-title rebuild flags (menuData+0x5d/0x5e, DAT, save_requested) + native-owned warp_requested finalize/autoclear + RESET return-title one-shots for the NEXT switch only (return-title gates exclude AUTOLOAD_HANDOFF)"
                ));
            }
        }
    }
    // Total world loads, boot included: exactly one increment per forwarded confirm. Every forward
    // is classified into exactly one bucket -- switch reload (FRESH_DESER_COUNT, == the load epoch),
    // old-world-up (WORLD_UP_COUNT), or neither, i.e. the boot/title Continue (NON_SWITCH_COUNT) --
    // so `allow == fresh_deser + non_switch + world_up` holds by construction and any drift is a
    // real telemetry fault. That identity is what `er_telemetry_core::load_count` audits, and the missing
    // boot bucket is exactly why a 3-load session reports oracle_current_load_epoch = 2.
    if !switch_active {
        SYSTEM_QUIT_CONTINUE_CONFIRM_NON_SWITCH_COUNT.fetch_add(1, Ordering::SeqCst);
    }
    SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_CONTINUE_CONFIRM_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { original(shim, b, c, d) }
}
