// Extracted from title_tick_cover.rs (pure code move, no behavior change): the harness
// switch-slot control-file driven programmatic character-switch helpers. include!'d into the
// same `title` module (experiments/title.rs), so module scope is identical to the original.
/// Path of the harness control file carrying the next switch's target slot (a decimal slot index).
fn switch_slot_control_path() -> Option<std::path::PathBuf> {
    game_directory_path().map(|d| d.join("er-effects-switch-slot.txt"))
}

/// Eligibility of the in-world session for a programmatic switch, plus a best-effort MoveMapStep step
/// for logging. Returns `(eligible, mms_step)`. NOTE: `title_owner(base)` is None during stable in-world
/// gameplay (it exists only at title/return-title states), so the InGameStep at owner+0x2e8 is not
/// reachable that way here -- eligibility is player-present + a live in-game menu job (CSMenuMan+0x798,
/// no title owner needed). The RE-claimed resident `mms_step==18` is READ best-effort and LOGGED (to
/// close the verification gap empirically) but NOT required, because the read path is title-owner-gated.
unsafe fn switch_world_resident_state(base: usize) -> (bool, i32) {
    if unsafe { PlayerIns::local_player_mut() }.is_err() {
        return (false, -1);
    }
    // menu_job (an OPEN in-game menu at CSMenuMan+0x798) is read for context but is NOT required for
    // eligibility: switch_slot_arm_programmatic is MENU-FREE (sq-repro's auto-chain at guards.rs:1117
    // arms "the SAME menu-free programmatic way instead of OPEN_MENU"). Requiring it made the
    // deterministic control-file poller defer forever (9873x, 'eligible=false') once sq-repro -- which
    // used to open the menu via nav -- was disabled, even though the char was fully movable. bd
    // DECISIVE-poller-eligibility-menujob-overconservative-arm-is-menufree-2026-07-21.
    let _menu_job = unsafe { safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }
        .filter(|&m| m > 0x10000)
        .and_then(|m| unsafe { safe_read_usize(m + CS_MENU_MAN_IN_GAME_MENU_JOB_798_OFFSET) })
        .unwrap_or(0);
    // best-effort mms_step (title-owner path; -1 when the owner is None in-world)
    let mms_step = unsafe { title_owner(base) }
        .and_then(|owner| {
            let owner = owner as usize;
            unsafe { safe_read_usize(owner + TITLE_STEP_IN_GAME_STEP_2E8_OFFSET) }
                .filter(|&v| v > 0x10000)
                .and_then(|ig| unsafe { safe_read_usize(ig + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) })
                .filter(|&v| v > 0x10000)
                .and_then(|m| unsafe { safe_read_i32(m + INGAMESTEP_STEP_STATE_OFFSET) })
        })
        .unwrap_or(-1);
    // Eligible whenever the local player is present (the is_err gate above already enforces that);
    // menu_job is NOT required -- the arm is menu-free.
    (true, mms_step)
}

/// Arm a menu-free character switch to `slot` PROGRAMMATICALLY (no menu navigation, no simulated
/// input). Sets exactly the state own_load_switch_reload_fire needs, clears the stale disableSaveMenu
/// gate, then writes the game-polled teardown flag menuData+0x5d=1 (which the resident STEP_MoveMap
/// advancer consumes to walk the child 18->19->20->-1, tearing the old world down to a clean title with
/// NO save request -- b72/b73/bc4 stay 0). Deliberately does NOT fire the return-title REQUEST/bc4/
/// MenuJob chain (those are the case-7 save-gate hang + Scaleform-race hazards). See RE workflow
/// wf_b4dae22c + bd repeatability-menu-free-phase-reset-fix-2026-07-18.
pub unsafe fn switch_slot_arm_programmatic(base: usize, slot: i32) {
    SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.store(slot as usize, Ordering::SeqCst);
    SYSTEM_QUIT_ARM_PLAYER_WAS_ABSENT.store(0, Ordering::SeqCst); // genuine in-world switch
    SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_SWITCH_MENU_FREE_RELOAD_FIRED.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_MENU_FREE_STABLE_TICKS.store(0, Ordering::SeqCst);
    // Reset the switch-reload FD4-IO phase machine + Phase-3 outgoing-teardown latches so THIS switch
    // re-runs SUBMIT/DRAIN/COMMIT and re-detects the outgoing _Common_Finalize. SHARED with the USER
    // ProfileSelect arm (system_quit_arm_quickload_autoload) via reset_switch_reload_latches() so the two
    // arm paths cannot drift -- that drift softlocked a user-driven load3 (bd
    // compounding-reload-two-roots-...-chainB-stale-fd4io-latch-b78-2026-07-23).
    crate::compat::own_load::reset_switch_reload_latches();
    PRODUCT_AUTOLOAD_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    OWN_STEPPER_SLOT.store(slot, Ordering::SeqCst);
    OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_MENU, Ordering::SeqCst);
    // Clear the stale CSMenuMan->disableSaveMenu (+0x13c) so the teardown is not gated (switch-2 safety).
    if let Some(csmm) =
        unsafe { safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }.filter(|&m| m > 0x10000)
    {
        unsafe {
            *((csmm + CS_MENU_MAN_DISABLE_SAVE_MENU_OFFSET) as *mut u8) = 0;
        }
    }
    let menudata = unsafe { safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }
        .filter(|&m| m > 0x10000)
        .and_then(|m| unsafe { safe_read_usize(m + CS_MENU_MAN_MENU_DATA_OFFSET) })
        .filter(|&d| d > 0x10000);
    let Some(md) = menudata else {
        SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.store(usize::MAX, Ordering::SeqCst);
        PRODUCT_AUTOLOAD_ARMED.store(0, Ordering::SeqCst);
        OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "switch-trigger: ABORT slot {slot} -- menuData ptr invalid; NOT writing teardown, rolled back arm"
        ));
        return;
    };
    ENDING_REQUEST_STALL_STREAK.store(0, Ordering::SeqCst);
    unsafe {
        *((md + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET) as *mut u8) = 1;
    }
    ENDING_REQUEST_SET.store(1, Ordering::SeqCst); // enable the existing clear-on-leave-18 latch
    SWITCH_TRIGGER_TEARDOWN_COUNT.fetch_add(1, Ordering::SeqCst);
    // Phase LAST, only after the teardown flag is set, so the fire gate can never see a half-arm.
    SYSTEM_QUIT_QUICKLOAD_PHASE.store(
        SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED,
        Ordering::SeqCst,
    );
    SWITCH_TRIGGER_LAST_SLOT.store(slot as usize, Ordering::SeqCst);
    let n = SWITCH_TRIGGER_ARM_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    append_autoload_debug(format_args!(
        "switch-trigger #{n}: PROGRAMMATIC arm slot {slot} (player present, world resident@mms18) -- menuData+0x5d=1 (teardown), phase=RETURN_TITLE_REQUESTED, ARM_PLAYER_WAS_ABSENT=0, FRESH_DESER_DONE=0, presses=0"
    ));
    // PORTRAIT RETARGET + COVER REARM -- shared with the USER ProfileSelect arm so the two arm paths
    // cannot drift (bd er-effects-rs-dpf6; same rule as reset_switch_reload_latches above). Without
    // this the programmatic switch skipped the whole portrait window/cover lifecycle: no window reset,
    // no boot-view rearm, no confirm timestamp -- so control-file switches showed the bare native
    // loading screen and agent probes could never exercise the user path's publish-race machinery.
    unsafe {
        crate::compat::portrait_retarget_and_rearm_for_switch(
            slot,
            "SwitchSlotProgrammaticArm",
        )
    };
}

/// Poll the harness switch-slot control file (mtime-gated) and, when a NEW request appears while the
/// world is resident+stable and no switch is in flight, arm a programmatic menu-free switch. Primes on
/// first sight so a stale file at boot never fires; DEFERS (leaves mtime unconsumed) until eligible.
pub unsafe fn poll_switch_slot_control_file(base: usize) {
    let Some(path) = switch_slot_control_path() else {
        return;
    };
    // mtime==0 means "no control file present". Prime UNCONDITIONALLY on the first in-world poll
    // (recording 0 if absent) so a control file CREATED after boot arms on its first write, while a
    // STALE file present at boot only arms if the harness later rewrites it to a newer mtime.
    // The moment the switch control file EXISTS, the DETERMINISTIC control-file driver owns switches, so
    // the sq-repro menu-nav switch driver stands down (env_flags::system_quit_repro_enabled) -- the two
    // were fighting (arming extra switches) AND the menu-nav suppressed the move-probe (load2 can_move
    // never latched). bd MILESTONE-detdrive-works-but-sqrepro-menunav-conflict-2026-07-21.
    if path.exists() {
        er_telemetry::counters::DETERMINISTIC_SWITCH_DRIVER_ACTIVE.store(1, Ordering::SeqCst);
    }
    let mtime = std::fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as usize)
        .unwrap_or(0);
    if SWITCH_SLOT_CONTROL_PRIMED.swap(1, Ordering::SeqCst) == 0 {
        SWITCH_SLOT_CONTROL_MTIME.store(mtime, Ordering::SeqCst);
        return;
    }
    if mtime == 0 || mtime == SWITCH_SLOT_CONTROL_MTIME.load(Ordering::SeqCst) {
        return;
    }
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(slot) = contents.trim().parse::<i32>() else {
        SWITCH_SLOT_CONTROL_MTIME.store(mtime, Ordering::SeqCst);
        return;
    };
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot) {
        SWITCH_SLOT_CONTROL_MTIME.store(mtime, Ordering::SeqCst);
        return;
    }
    let phase_idle =
        SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst) == SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE;
    let (eligible, mms_step) = unsafe { switch_world_resident_state(base) };
    if !phase_idle || !eligible {
        let n = SWITCH_TRIGGER_DEFERRED_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 3 || n.is_multiple_of(120) {
            append_autoload_debug(format_args!(
                "switch-trigger: DEFER slot {slot} -- phase_idle={phase_idle} eligible={eligible} mms_step={mms_step} (waiting for stable in-world; not consuming request)"
            ));
        }
        return;
    }
    append_autoload_debug(format_args!(
        "switch-trigger: request slot {slot} ELIGIBLE (phase IDLE, in-world, mms_step={mms_step}) -> arming"
    ));
    SWITCH_SLOT_CONTROL_MTIME.store(mtime, Ordering::SeqCst);
    unsafe { switch_slot_arm_programmatic(base, slot) };
}
