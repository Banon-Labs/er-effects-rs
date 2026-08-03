use super::*;

pub(crate) fn install_system_quit_continue_confirm_hook() {
    if SYSTEM_QUIT_CONTINUE_CONFIRM_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MH_Initialize for continue_confirm guard failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(CONTINUE_CONFIRM_RVA as u32) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve continue_confirm rva 0x{CONTINUE_CONFIRM_RVA:x}"
        ));
        return;
    };
    // CROSS-DLL UNION (2026-07-18): install through the union, NOT a bare MhHook. The companion
    // er-reload-trace-dll also observes this address (0xb0e180) and routes through THIS DLL's union
    // via the `er_effects_union_register` export. A bare MhHook here would let whichever DLL grabbed
    // the single MinHook slot first win and silently drop the other -- and if the trace preempted,
    // this CRITICAL continue-confirm guard (it drives a fresh picked-slot deserialize before SetState5)
    // would never fire and the reload would break. The union chains both handlers regardless of order.
    // `system_quit_continue_confirm_hook` is a 4-arg UnionFn and its orig call
    // (system_quit_repro_guards.rs) already invokes SYSTEM_QUIT_CONTINUE_CONFIRM_ORIG as
    // `fn(usize,usize,usize,usize)->usize`, which the union fills with the trampoline (or the next
    // chained handler) -- so chaining is transparent to the hook body.
    let handler: crate::mh::UnionFn = unsafe {
        std::mem::transmute::<*mut c_void, crate::mh::UnionFn>(
            system_quit_continue_confirm_hook as *mut c_void,
        )
    };
    match unsafe {
        crate::mh::register_union_hook(addr, handler, &SYSTEM_QUIT_CONTINUE_CONFIRM_ORIG)
    } {
        Ok(()) => {
            SYSTEM_QUIT_CONTINUE_CONFIRM_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "system-quit-quickload: UNIONED title Continue confirm 0x{addr:x}; active switch drives a fresh picked-slot deserialize before SetState5 (fail-closed); chains with any companion trace observer"
            ));
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-quickload: union register continue_confirm guard failed: {status:?}"
        )),
    }
}

/// READ-ONLY trace on `EzChildStepBase::RequestFinish` (`EZ_CHILD_STEP_REQUEST_FINISH_RVA`). The
/// quit-to-title teardown ends the in-world MoveMapStep session through this one-shot; the
/// post-switch reload bounce is the SAME call arriving against the freshly-created MoveMapStep
/// child right after streaming completes. Logs which InGameStep child wrapper is being finished
/// (stay/movemap) plus the first game-image caller RVA, so the stale requester can be identified.
pub(crate) unsafe extern "system" fn system_quit_child_finish_request_hook(wrapper: usize) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let n = SYSTEM_QUIT_CHILD_FINISH_TRACE_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 64 {
            let mut owner = TITLE_OWNER_PTR.load(Ordering::SeqCst);
            if owner == TITLE_OWNER_SCAN_START_ADDRESS {
                owner = TITLE_SETSTATE_TRACE_LAST_OWNER.load(Ordering::SeqCst);
            }
            let ig = if owner != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(owner + TITLE_STEP_IN_GAME_STEP_2E8_OFFSET) }.unwrap_or(0)
            } else {
                0
            };
            let kind = if ig != 0 && wrapper == ig + IN_GAME_STEP_MOVE_MAP_WRAPPER_E0_OFFSET {
                "MOVEMAP-CHILD"
            } else if ig != 0 && wrapper == ig + IN_GAME_STEP_STAY_WRAPPER_B8_OFFSET {
                "stay-child"
            } else {
                "other"
            };
            let child =
                unsafe { safe_read_usize(wrapper + EZ_CHILD_STEP_STEPPER_OFFSET) }.unwrap_or(0);
            let caller_rva = crate::crashlog::trace_first_game_caller_rva();
            append_autoload_debug(format_args!(
                "child-finish-request #{n}: kind={kind} wrapper=0x{wrapper:x} child=0x{child:x} ig=0x{ig:x} caller_rva=0x{caller_rva:x}"
            ));
        }
    }));
    let orig = SYSTEM_QUIT_CHILD_FINISH_TRACE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return;
    }
    let original: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(orig) };
    unsafe { original(wrapper) }
}

pub(crate) fn install_system_quit_child_finish_trace_hook() {
    if SYSTEM_QUIT_CHILD_FINISH_TRACE_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "child-finish-request: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(EZ_CHILD_STEP_REQUEST_FINISH_RVA) else {
        append_autoload_debug(format_args!(
            "child-finish-request: failed to resolve rva 0x{EZ_CHILD_STEP_REQUEST_FINISH_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_child_finish_request_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_CHILD_FINISH_TRACE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "child-finish-request: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_CHILD_FINISH_TRACE_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "child-finish-request: hooked EzChildStepBase::RequestFinish 0x{addr:x} -- read-only teardown-requester trace armed"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "child-finish-request: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "child-finish-request: MhHook::new failed: {status:?}"
        )),
    }
}

pub(crate) use er_telemetry::counters::TESTNET_FF_FIRED_EPOCH;
pub(crate) use er_telemetry::counters::TESTNET_FF_LAST_MMS;
/// Stuck-frame + one-shot state for the load2/boot testNetStep force-finish below.
pub(crate) use er_telemetry::counters::TESTNET_FF_STUCK_FRAMES;
pub(crate) const TESTNET_FF_STUCK_FRAME_THRESHOLD: usize = 120;

/// LOAD2 WORLD-COMPLETION FIX (bd load2-fires-but-stalls-at-mms18-world-completion-2026-07-19). A
/// DRIVEN reload (`fresh_deser>=1`) reaches MoveMapStep STEP_Finish but its testNetStep child never
/// finishes -- observed LOAD2-ONLY: load1's testNetStep finishes so requestCode latches 1->2 and the
/// world completes; load2's HANGS so requestCode stays 1, STEP_GameStepWait never gets a completed
/// world, and there is no readiness (mms=18, warmup=0, testnet_stepper_present=True, csremo idle).
/// Force the hung child via the RE'd SAVE-SAFE lever `EzChildStepBase::RequestFinish` (0xeb5570) on the
/// testNetStep wrapper at `MoveMapStep+0x108`. TIGHTLY GATED so it can never touch a healthy load1 or a
/// still-progressing load: only after a reload committed (epoch>=1), only while requestCode==1, only
/// when the inner stepper (+0x110) is non-null (unfinished), only after STUCK frames of no mms_state
/// progress, and ONCE per reload epoch. Called per-frame from `tick_before_player_lookup`.
pub(crate) unsafe fn maybe_force_finish_stuck_testnet_step() {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let epoch = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
    let boot_epoch = epoch == 0;
    let mut owner = TITLE_OWNER_PTR.load(Ordering::SeqCst);
    if owner == null {
        owner = TITLE_SETSTATE_TRACE_LAST_OWNER.load(Ordering::SeqCst);
    }
    let ig = if owner != null {
        unsafe { safe_read_usize(owner + TITLE_STEP_IN_GAME_STEP_2E8_OFFSET) }
            .filter(|v| *v >= 0x10000)
    } else {
        None
    };
    // requestCode must be 1 (world loading, not latched to 2 = done). Any other value: reset + bail.
    // Fall back to the same oracle path the report uses; the fresh title-owner scan can be stale during
    // the boot-autoload handoff while write_oracle has already resolved the live MoveMapStep.
    let request_code = ig
        .map(|ig| unsafe { safe_read_i32(ig + IN_GAME_STEP_REQUEST_CODE_D8_OFFSET) }.unwrap_or(-1))
        .unwrap_or_else(|| SWITCH_ORACLE_REQUEST_CODE.load(Ordering::SeqCst));
    if request_code != 1 {
        TESTNET_FF_STUCK_FRAMES.store(0, Ordering::Relaxed);
        return;
    }
    let mms_from_ingame = ig.and_then(|ig| {
        unsafe { safe_read_usize(ig + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) }.filter(|v| *v >= 0x10000)
    });
    let mms_from_oracle = ORACLE_RELIABLE_MMS_PTR.load(Ordering::SeqCst);
    let mms = mms_from_ingame.or_else(|| (mms_from_oracle >= 0x10000).then_some(mms_from_oracle));
    let Some(mms) = mms else {
        return;
    };
    // FINALIZE SUBSTATE force-advance (bd load2-real-blocker-finalize-advancer-stuck-substate7): the
    // load2 softlock parks at MoveMapStep+0x12a == 7 ("REMO/SAVE-DRAIN WAIT") -- the advancer FUN_140afa7c0's
    // 7->8 gate (FUN_14067a170 && !ShouldSave && !FUN_140679460 && FUN_140a9ceb0(CSRemo)) never passes for a
    // warm reload. A just-deserialized load2 has NOT played, so that save/remo drain is spurious. Force the
    // substate 7->8 (WARP/SERVER FINALIZE) so the advancer continues to 9, STEP_MoveMap_Update latches
    // requestCode=2, and the world completes. Writing +0x12a is LOAD-STATE progression, NOT a save write.
    let fin = unsafe { safe_read_u8(mms + MOVEMAPSTEP_FINALIZE_SUBSTATE_12A_OFFSET) }
        .map(i32::from)
        .unwrap_or(-1);
    let mms_state = unsafe { safe_read_i32(mms + MOVEMAPSTEP_STATE_48_RE_OFFSET) }.unwrap_or(-1);
    if boot_epoch {
        let testnet_stepper =
            unsafe { safe_read_usize(mms + MOVEMAPSTEP_TESTNETSTEP_STEPPER_110_OFFSET) }
                .unwrap_or(0);
        let boot_stuck_signature = request_code == 1
            && mms_state == MOVEMAPSTEP_STEP_MOVEMAP_INDEX
            && fin == 0
            && testnet_stepper >= 0x10000;
        if !boot_stuck_signature {
            TESTNET_FF_STUCK_FRAMES.store(0, Ordering::Relaxed);
            TESTNET_FF_LAST_MMS.store(usize::MAX, Ordering::Relaxed);
            return;
        }
        let stuck_frames = if TESTNET_FF_LAST_MMS.swap(mms, Ordering::SeqCst) == mms {
            TESTNET_FF_STUCK_FRAMES.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            TESTNET_FF_STUCK_FRAMES.store(1, Ordering::SeqCst);
            1
        };
        if stuck_frames >= TESTNET_FF_STUCK_FRAME_THRESHOLD
            && TESTNET_FF_FIRED_EPOCH.swap(epoch, Ordering::SeqCst) != epoch
        {
            let wrapper = mms + MOVEMAPSTEP_TESTNETSTEP_WRAPPER_108_OFFSET;
            match game_rva(EZ_CHILD_STEP_REQUEST_FINISH_RVA) {
                Ok(addr) => {
                    let request_finish: unsafe extern "system" fn(usize) =
                        unsafe { std::mem::transmute(addr) };
                    unsafe { request_finish(wrapper) };
                    append_autoload_debug(format_args!(
                        "testnet-ff: boot epoch {epoch} stuck {stuck_frames} frames at requestCode={request_code} mms={mms_state} fin={fin} testnet=0x{testnet_stepper:x} -> RequestFinish(wrapper=0x{wrapper:x})"
                    ));
                }
                Err(_) => append_autoload_debug(format_args!(
                    "testnet-ff: boot epoch {epoch} stuck {stuck_frames} frames but failed to resolve RequestFinish rva 0x{EZ_CHILD_STEP_REQUEST_FINISH_RVA:x}"
                )),
            }
        }
        return;
    }
    // FRAMERATE FIX (2026-07-21, case 7 gate decompiled from FUN_140afa6d0 via the ghidra MCP): the
    // finalize walk (fin 5->9) is a cleanup that runs AFTER the fin=0 movable window; load1 becomes
    // movable, THEN walks and settles (mms 18->-1) -> exits loading mode -> fps recovers. Holding fin=0
    // FOREVER (below) keeps load2/load3 in loading mode -> flat 20fps. So: once THIS reload's 60-frame
    // movement proof has LATCHED, RELEASE the hold and satisfy the case-7 save-drain gate so the game's
    // OWN advancer completes the walk. Gate 7->8 = (saveState==0 && !ShouldSave() && !ngp && CSRemo-
    // drained); ShouldSave() reads GameMan.saveRequested (0xb72), left set because the finalize's own
    // RequestSave(false) autosave is suppressed on a warm reload (gaitem-crash dodge). saveState/ngp/
    // CSRemo already pass -> saveRequested is the SOLE blocker. Clear saveState + saveRequested native-
    // flow (let the game run 7->8->9; NOT a forced field25 write, which tore the world down). Movement is
    // already proven, so completing the walk now cannot regress the proof.
    let move_proven_for_reload = crate::constants::CAN_MOVE_CONFIRMED.load(Ordering::SeqCst)
        && crate::constants::MOVE_PROBE_EPOCH.load(Ordering::SeqCst) == epoch;
    if move_proven_for_reload && (13..=18).contains(&mms_state) {
        if let Ok(gm) = unsafe { eldenring::cs::GameMan::instance() } {
            let gm_addr = gm as *const _ as usize;
            let ss = core::mem::offset_of!(eldenring::cs::GameMan, save_state);
            let sr = core::mem::offset_of!(eldenring::cs::GameMan, save_requested);
            if unsafe { safe_read_u8(gm_addr + ss) }.unwrap_or(0) != 0 {
                unsafe { *((gm_addr + ss) as *mut u8) = 0 };
            }
            if unsafe { safe_read_u8(gm_addr + sr) }.unwrap_or(0) != 0 {
                unsafe { *((gm_addr + sr) as *mut u8) = 0 };
            }
            static SATISFY_LOG_EPOCH: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(usize::MAX);
            if SATISFY_LOG_EPOCH.swap(epoch, Ordering::SeqCst) != epoch {
                append_autoload_debug(format_args!(
                    "case7-savedrain-satisfy: epoch {epoch} move-proven mms={mms_state} fin={fin} -> cleared saveState+saveRequested(0xb72) so the finalize completes 7->8->9 natively (loading mode exits, fps -> load1 parity)"
                ));
            }
        }
        return;
    }
    // MOVABLE-WINDOW PRESERVATION (bd complete-cvar10-ending-request-9-inputs +
    // precise-ordered-divergence-load1-movable-at-fin0): load1 reaches genuine readiness by becoming
    // MOVABLE at mms=18/fin=0 -- FUN_140afa7c0 case 0 does `if (cVar10 == 0) return;`, so the finalize
    // STAYS at 0 (movable window; can_move ramps to 60 frames) while the ending-request cVar10 is 0; the
    // finalize walk (fin 5->9) is a quick cleanup AFTER, not a prerequisite. Load2 diverges because an
    // ending-request input is left set (dominant: warpRequested / GameMan+0x10, residue of the
    // return-title), so cVar10=1 -> the finalize runs early (fin 5->7) -> the character FREEZES ->
    // can_move never ramps. Fix = keep cVar10 = 0 by clearing warpRequested (and idle saveState) every
    // frame during the pre-finalize load window, so load2 gets load1's fin=0 movable window. We PREVENT
    // the ending walk rather than forcing/advancing the finalize (forcing tore the world down). Clear
    // during 13..=18 (character loaded, warp already consumed) so it is 0 before FUN_140afa7c0 reaches
    // case 0 at mms=18. Epoch-scoped: load1 (epoch 0) is never touched.
    if !(13..=18).contains(&mms_state) || fin >= 5 {
        return;
    }
    let ss_off = core::mem::offset_of!(eldenring::cs::GameMan, save_state);
    if let Ok(gm) = unsafe { eldenring::cs::GameMan::instance() } {
        let gm_addr = gm as *const _ as usize;
        if unsafe { safe_read_u8(gm_addr + ss_off) }.unwrap_or(0) > 0 {
            unsafe { *((gm_addr + ss_off) as *mut u8) = 0 };
        }
        let warp_off = crate::constants::GAME_MAN_WARP_REQUESTED_10_OFFSET;
        let warp = unsafe { safe_read_u8(gm_addr + warp_off) }.unwrap_or(0);
        if warp != 0 {
            unsafe { *((gm_addr + warp_off) as *mut u8) = 0 };
            if TESTNET_FF_FIRED_EPOCH.swap(epoch, Ordering::SeqCst) != epoch {
                append_autoload_debug(format_args!(
                    "cvar10-warp-clear: load2 epoch {epoch} mms={mms_state} fin={fin} warpRequested was set -> cleared GameMan+0x10 to hold cVar10=0 (fin=0 movable window like load1)"
                ));
            }
        }
    }
}

pub(crate) unsafe extern "system" fn system_quit_profile_load_job_run_hook(
    job: usize,
    result: usize,
    fd4_time: usize,
    d: usize,
) -> usize {
    let orig = SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: ProfileLoadDialog load-job trampoline unset for job=0x{job:x} -- fail-closed result=0x{result:x}"
        ));
        if result > TITLE_OWNER_SCAN_START_ADDRESS && unsafe { safe_read_usize(result) }.is_some() {
            unsafe {
                *(result as *mut i32) = MENU_JOB_STATE_SUCCESS;
                *((result + 4) as *mut i32) = 0;
            }
        }
        return result;
    }
    let original: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    let profile_window = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    let list = unsafe { safe_read_usize(job + 0x50) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let profile_id = unsafe { safe_read_i32(job + 0x58) }.unwrap_or(-1);
    let context_arg =
        unsafe { safe_read_usize(job + 0x60) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_JOB.store(job, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_LIST.store(list, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_PROFILE_ID.store(profile_id as usize, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_CONTEXT_ARG.store(context_arg, Ordering::SeqCst);
    // ROBUST block gate: block ANY ProfileLoad job while our injected in-world Load-Profile UI is up
    // (real System windows hidden + our ProfileSelect window present). The prior `list ==
    // profile_window + 0x50` match was fragile: when it failed (observed 2026-07-01), the in-world
    // deserialize ran, our gaitem guards corrupted CSGaitemImp::gaitemInsTable, and it crashed in
    // GetGaitemIns->GetGaitemHandle (live 0x6710c0) BEFORE the per-tick native close could pop
    // ProfileSelect. The only load job that runs while our injected ProfileSelect is showing IS our
    // flow's load, so hidden+profile-present is a sufficient and robust discriminator. `list` is
    // still captured above for telemetry.
    let _ = list;
    let system_quit_profile_active =
        profile_window != 0 && SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) != 0;
    if !system_quit_profile_active {
        return unsafe { original(job, result, fd4_time, d) };
    }

    if system_quit_profile_load_activation_allowed() {
        SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-dup: ProfileSelect load-job Run ALLOWED job=0x{job:x} list=0x{list:x} profile_id={profile_id}; forwarding native load path (known crash risk: CSGaitemImp::Deserialize rva 0x67141a)"
        ));
        return unsafe { original(job, result, fd4_time, d) };
    }

    SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst);
    unsafe { system_quit_arm_quickload_autoload(profile_id, "ProfileSelectLoadJobRun") };
    if result > TITLE_OWNER_SCAN_START_ADDRESS && unsafe { safe_read_usize(result) }.is_some() {
        unsafe {
            // Success(2), terminal: the load-job is the SECOND link in the native chain the slot
            // activation submits (msgbox -> loadjob -> confirm-lambda FUN_1409a4ee0). Returning Success
            // lets the chain advance to the confirm-lambda, which our confirm hook cancel-closes
            // (natively pops ProfileSelect) so the menu-pump return-title chain can submit. Returning
            // Failed(3) instead ABORTS the chain -> the confirm-lambda never runs -> ProfileSelect never
            // closes -> return-title never submits (verified live 2026-07-01). The in-world load is NOT
            // committed here: the actual saveState/b80=2 arm is the native RequestLoadSlot FUN_14067b2f0,
            // which system_quit_request_load_slot_hook neutralizes during the switch. See bd
            // system-quit-loadjob-success-commits-phantom-load-2026-07-01.
            *(result as *mut i32) = MENU_JOB_STATE_SUCCESS;
            *((result + 4) as *mut i32) = 0;
        }
    }
    if SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_FIRED.load(Ordering::SeqCst) == 0 {
        match game_rva(SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA) {
            Ok(close_addr) => {
                let close_fn: unsafe extern "system" fn(usize) =
                    unsafe { std::mem::transmute(close_addr) };
                unsafe { close_fn(profile_window) };
                SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_FIRED.store(1, Ordering::SeqCst);
                SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_COUNT.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-dup: ProfileSelect load-job Run native-closed ProfileSelect directly after save-safe block window=0x{profile_window:x} close=0x{close_addr:x}; does not depend on a later confirm-lambda callback"
                ));
            }
            Err(_) => append_autoload_debug(format_args!(
                "system-quit-dup: ProfileSelect load-job Run close skipped -- failed to resolve close rva 0x{SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA:x}"
            )),
        }
    }
    if let Ok(base) = game_module_base() {
        if fd4_time > TITLE_OWNER_SCAN_START_ADDRESS
            && unsafe { safe_read_usize(fd4_time) }.is_some()
        {
            unsafe { *(fd4_time as *mut usize) = base + FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA };
        }
    }
    append_autoload_debug(format_args!(
        "system-quit-dup: ProfileSelect load-job Run BLOCKED save-safe job=0x{job:x} result=0x{result:x} list=0x{list:x} profile_id={profile_id} context_arg=0x{context_arg:x}; returning Success after direct native-close (in-world saveState=2 arm is blocked at RequestLoadSlot); no captured LoadJob is retained or replayed"
    ));
    result
}

pub(crate) fn disable_system_quit_gaitem_finalize_hook(source: &str) {
    if SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED_YES
    {
        return;
    }
    let addr = SYSTEM_QUIT_GAITEM_FINALIZE_ADDR.load(Ordering::SeqCst);
    if addr == 0 {
        return;
    }
    match unsafe { MH_QueueDisableHook(addr as *mut c_void) } {
        MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
            MH_STATUS::MH_OK => {
                SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED
                    .store(SYSTEM_QUIT_GAITEM_FINALIZE_DISABLED, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-quickload: disabled CSGaitemImp finalize hook 0x{addr:x} before native Continue source={source}"
                ));
            }
            status => append_autoload_debug(format_args!(
                "system-quit-quickload: MH_ApplyQueued disable CSGaitemImp finalize hook failed source={source}: {status:?}"
            )),
        },
        status => append_autoload_debug(format_args!(
            "system-quit-quickload: queue_disable CSGaitemImp finalize hook failed source={source}: {status:?}"
        )),
    }
}

pub(crate) fn disable_system_quit_gaitem_lookup_hook(source: &str) {
    if SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED_YES
    {
        return;
    }
    let addr = SYSTEM_QUIT_GAITEM_LOOKUP_ADDR.load(Ordering::SeqCst);
    if addr == 0 {
        return;
    }
    match unsafe { MH_QueueDisableHook(addr as *mut c_void) } {
        MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
            MH_STATUS::MH_OK => {
                SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED
                    .store(SYSTEM_QUIT_GAITEM_LOOKUP_DISABLED, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-quickload: disabled CSGaitemImp lookup hook 0x{addr:x} before native Continue source={source}"
                ));
            }
            status => append_autoload_debug(format_args!(
                "system-quit-quickload: MH_ApplyQueued disable CSGaitemImp lookup hook failed source={source}: {status:?}"
            )),
        },
        status => append_autoload_debug(format_args!(
            "system-quit-quickload: queue_disable CSGaitemImp lookup hook failed source={source}: {status:?}"
        )),
    }
}

pub(crate) fn disable_system_quit_gaitem_deserialize_hook(source: &str) {
    if SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED_YES
    {
        return;
    }
    let addr = SYSTEM_QUIT_GAITEM_DESERIALIZE_ADDR.load(Ordering::SeqCst);
    if addr == 0 {
        return;
    }
    match unsafe { MH_QueueDisableHook(addr as *mut c_void) } {
        MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
            MH_STATUS::MH_OK => {
                SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED
                    .store(SYSTEM_QUIT_GAITEM_DESERIALIZE_DISABLED, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-quickload: disabled CSGaitemImp::Deserialize hook 0x{addr:x} before native Continue source={source}"
                ));
            }
            status => append_autoload_debug(format_args!(
                "system-quit-quickload: MH_ApplyQueued disable CSGaitemImp::Deserialize hook failed source={source}: {status:?}"
            )),
        },
        status => append_autoload_debug(format_args!(
            "system-quit-quickload: queue_disable CSGaitemImp::Deserialize hook failed source={source}: {status:?}"
        )),
    }
}

/// Robust "install this MinHook detour exactly once" primitive shared by the boot-time hook installs. Fixes
/// the non-deterministic MinHook install races (2026-07-15): these installs are retried per game-tick until
/// they land, and the old `load()!=NOT?return` guard did not block a REENTRANT call while the first was
/// mid-install (the flag was only set on full success), so an install ran twice -> double MhHook::new
/// (ALREADY_CREATED) + a `queue_enable`+shared-`MH_ApplyQueued` race -> the handler non-deterministically
/// never fired (intermittent ghosting, dead slot-pick, reload crash). This helper: (1) atomic once-CLAIM on
/// `flag` so only the first caller proceeds; (2) atomic single-target `MH_EnableHook` (no shared queue);
/// (3) adopts `MH_ERROR_ALREADY_CREATED` and treats `MH_ERROR_ENABLED` as success. Rolls `flag` back to
/// `not_installed` only on a REAL failure so a later tick retries. `addr` is the already-resolved target VA.
pub(crate) fn mh_install_hook_once(
    flag: &AtomicUsize,
    not_installed: usize,
    installed_yes: usize,
    addr: usize,
    handler: *mut c_void,
    orig: &'static AtomicUsize,
    name: &str,
) -> bool {
    if flag
        .compare_exchange(
            not_installed,
            installed_yes,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_err()
    {
        return flag.load(Ordering::SeqCst) == installed_yes;
    }
    // UNION (2026-07-16): register through the hook union instead of a bare MhHook. If another feature
    // already hooks this game address, we CHAIN onto it (no silent drop, no install-order race) rather
    // than losing the single MinHook slot. `orig` is wired to the next handler (or the real trampoline).
    let handler_fn: crate::mh::UnionFn =
        unsafe { std::mem::transmute::<*mut c_void, crate::mh::UnionFn>(handler) };
    match unsafe { crate::mh::register_union_hook(addr, handler_fn, orig) } {
        Ok(()) => {
            append_autoload_debug(format_args!(
                "mh-install: {name} registered on union 0x{addr:x}"
            ));
            true
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "mh-install: register_union_hook {name} failed: {status:?}"
            ));
            flag.store(not_installed, Ordering::SeqCst);
            false
        }
    }
}

pub(crate) fn install_system_quit_profile_load_activate_hook() {
    let Ok(addr) = game_rva(SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve ProfileLoadDialog activation rva 0x{SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_RVA:x}"
        ));
        return;
    };
    mh_install_hook_once(
        &SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_INSTALLED,
        SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_NOT_INSTALLED,
        SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_INSTALLED_YES,
        addr,
        system_quit_profile_load_activate_hook as *mut c_void,
        &SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_ORIG,
        "ProfileLoadDialog activation",
    );
}

pub(crate) fn install_system_quit_profile_load_confirmed_hook() {
    let Ok(addr) = game_rva(SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve ProfileLoadDialog confirmed-load rva 0x{SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_RVA:x}"
        ));
        return;
    };
    mh_install_hook_once(
        &SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_INSTALLED,
        SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_NOT_INSTALLED,
        SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_INSTALLED_YES,
        addr,
        system_quit_profile_load_confirmed_hook as *mut c_void,
        &SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ORIG,
        "ProfileLoadDialog confirmed-load",
    );
}

pub(crate) fn install_system_quit_profile_load_job_run_hook() {
    let Ok(addr) = game_rva(SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve ProfileLoadDialog load-job Run rva 0x{SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_RVA:x}"
        ));
        return;
    };
    mh_install_hook_once(
        &SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_INSTALLED,
        SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_NOT_INSTALLED,
        SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_INSTALLED_YES,
        addr,
        system_quit_profile_load_job_run_hook as *mut c_void,
        &SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_ORIG,
        "ProfileLoadDialog load-job Run",
    );
}
