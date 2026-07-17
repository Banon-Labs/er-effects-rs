
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
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_continue_confirm_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_CONTINUE_CONFIRM_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-quickload: queue_enable continue_confirm guard failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_CONTINUE_CONFIRM_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-quickload: hooked title Continue confirm 0x{addr:x}; active switch drives a fresh picked-slot deserialize before SetState5 (fail-closed)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: MH_ApplyQueued continue_confirm guard failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-quickload: MhHook::new continue_confirm guard failed: {status:?}"
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

pub(crate) unsafe extern "system" fn system_quit_gaitem_deserialize_hook(
    gaitem: usize,
    input_stream: usize,
) {
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase)
    {
        let skips = SYSTEM_QUIT_GAITEM_DESERIALIZE_SKIP_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if skips <= 8 || skips % 64 == 0 {
            append_autoload_debug(format_args!(
                "system-quit-quickload: CSGaitemImp::Deserialize SKIPPED during return-title transition #{skips} phase={phase} gaitem=0x{gaitem:x} input_stream=0x{input_stream:x}; lets native return-title load job advance without in-world inventory deserialize crash"
            ));
        }
        return;
    }
    SYSTEM_QUIT_GAITEM_DESERIALIZE_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    // 2ND-IN-PROCESS RELOAD FIX: when a profile-switch has picked a slot (SELECTED_SLOT in 0..10; usize::MAX
    // on a clean boot), this deserialize is char#2's load running on the CSGaitemImp table that still holds
    // char#1's items (freed by teardown). The native deserialize then exhausts the free-queue and dispatches a
    // stale CSGaitemIns vtable -> the AV at CSGaitemImp::Deserialize (0x67141a). Reset the singleton to pristine
    // BEFORE the native deserialize so it starts from an empty table. Never fires on a clean boot (no pick, so
    // the table is already fresh); idempotent with the continue_confirm reset (clearing an empty table no-ops).
    let picked = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
    if picked < TITLE_PROFILE_SLOT_COUNT {
        if let Ok(base) = game_module_base() {
            let n = SYSTEM_QUIT_GAITEM_DESERIALIZE_RESET_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            append_autoload_debug(format_args!(
                "system-quit-quickload: reset CSGaitemImp singleton #{n} before native deserialize (picked slot={picked}) -- clears char#1 stale items so char#2 deserialize does not dispatch a freed vtable (0x67141a)"
            ));
            unsafe { own_load_reset_gaitem_singleton(base) };
        }
    }
    let orig = SYSTEM_QUIT_GAITEM_DESERIALIZE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-quickload: CSGaitemImp::Deserialize trampoline unset phase={phase} gaitem=0x{gaitem:x}; fail-closed skip"
        ));
        return;
    }
    let original: unsafe extern "system" fn(usize, usize) = unsafe { std::mem::transmute(orig) };
    unsafe { original(gaitem, input_stream) };
}

pub(crate) unsafe extern "system" fn system_quit_gameman_load_save_hook(
    game_man: usize,
    save_arg: usize,
    load_kind: u32,
) -> usize {
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase)
    {
        let blocks = SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        append_autoload_debug(format_args!(
            "system-quit-quickload: GameMan load-save BLOCKED during return-title transition #{blocks} phase={phase} game_man=0x{game_man:x} save_arg=0x{save_arg:x} load_kind=0x{load_kind:x}; prevents in-world CSGaitemImp::Deserialize crash before title rebuild"
        ));
        return 0;
    }
    SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-quickload: GameMan load-save trampoline unset phase={phase} game_man=0x{game_man:x}; fail-closed return 0"
        ));
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize, u32) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { original(game_man, save_arg, load_kind) }
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

pub(crate) fn install_system_quit_gaitem_finalize_hook() {
    let installed = SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED.load(Ordering::SeqCst);
    if installed == SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED_YES {
        return;
    }
    if installed == SYSTEM_QUIT_GAITEM_FINALIZE_DISABLED {
        let addr = SYSTEM_QUIT_GAITEM_FINALIZE_ADDR.load(Ordering::SeqCst);
        if addr != 0 {
            match unsafe { MH_QueueEnableHook(addr as *mut c_void) } {
                MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
                    MH_STATUS::MH_OK => {
                        SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED
                            .store(SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED_YES, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "system-quit-quickload: re-enabled CSGaitemImp finalize hook 0x{addr:x}; transition finalize skipped until native title handoff"
                        ));
                    }
                    status => append_autoload_debug(format_args!(
                        "system-quit-quickload: MH_ApplyQueued re-enable CSGaitemImp finalize hook failed: {status:?}"
                    )),
                },
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: queue re-enable CSGaitemImp finalize hook failed: {status:?}"
                )),
            }
        }
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MH_Initialize for CSGaitemImp finalize hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_GAITEM_FINALIZE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve CSGaitemImp finalize rva 0x{SYSTEM_QUIT_GAITEM_FINALIZE_RVA:x}"
        ));
        return;
    };
    SYSTEM_QUIT_GAITEM_FINALIZE_ADDR.store(addr, Ordering::SeqCst);
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_gaitem_finalize_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_GAITEM_FINALIZE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-quickload: queue_enable CSGaitemImp finalize hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED
                        .store(SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-quickload: hooked CSGaitemImp finalize 0x{addr:x}; transition finalize skipped until native title handoff"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: MH_ApplyQueued CSGaitemImp finalize hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-quickload: MhHook::new CSGaitemImp finalize hook failed: {status:?}"
        )),
    }
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

pub(crate) fn install_system_quit_gaitem_lookup_hook() {
    let installed = SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED.load(Ordering::SeqCst);
    if installed == SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED_YES {
        return;
    }
    if installed == SYSTEM_QUIT_GAITEM_LOOKUP_DISABLED {
        let addr = SYSTEM_QUIT_GAITEM_LOOKUP_ADDR.load(Ordering::SeqCst);
        if addr != 0 {
            match unsafe { MH_QueueEnableHook(addr as *mut c_void) } {
                MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
                    MH_STATUS::MH_OK => {
                        SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED
                            .store(SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED_YES, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "system-quit-quickload: re-enabled CSGaitemImp lookup hook 0x{addr:x}; transition equipment handle lookups empty until native title handoff"
                        ));
                    }
                    status => append_autoload_debug(format_args!(
                        "system-quit-quickload: MH_ApplyQueued re-enable CSGaitemImp lookup hook failed: {status:?}"
                    )),
                },
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: queue re-enable CSGaitemImp lookup hook failed: {status:?}"
                )),
            }
        }
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MH_Initialize for CSGaitemImp lookup hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_GAITEM_LOOKUP_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve CSGaitemImp lookup rva 0x{SYSTEM_QUIT_GAITEM_LOOKUP_RVA:x}"
        ));
        return;
    };
    SYSTEM_QUIT_GAITEM_LOOKUP_ADDR.store(addr, Ordering::SeqCst);
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_gaitem_lookup_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_GAITEM_LOOKUP_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-quickload: queue_enable CSGaitemImp lookup hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED
                        .store(SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-quickload: hooked CSGaitemImp lookup 0x{addr:x}; transition equipment handle lookups empty until native title handoff"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: MH_ApplyQueued CSGaitemImp lookup hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-quickload: MhHook::new CSGaitemImp lookup hook failed: {status:?}"
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

pub(crate) fn install_system_quit_gaitem_deserialize_hook() {
    let installed = SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED.load(Ordering::SeqCst);
    if installed == SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED_YES {
        return;
    }
    if installed == SYSTEM_QUIT_GAITEM_DESERIALIZE_DISABLED {
        let addr = SYSTEM_QUIT_GAITEM_DESERIALIZE_ADDR.load(Ordering::SeqCst);
        if addr != 0 {
            match unsafe { MH_QueueEnableHook(addr as *mut c_void) } {
                MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
                    MH_STATUS::MH_OK => {
                        SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED.store(
                            SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED_YES,
                            Ordering::SeqCst,
                        );
                        append_autoload_debug(format_args!(
                            "system-quit-quickload: re-enabled CSGaitemImp::Deserialize hook 0x{addr:x}; transition inventory deserialize leaf skipped until native title handoff"
                        ));
                    }
                    status => append_autoload_debug(format_args!(
                        "system-quit-quickload: MH_ApplyQueued re-enable CSGaitemImp::Deserialize hook failed: {status:?}"
                    )),
                },
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: queue re-enable CSGaitemImp::Deserialize hook failed: {status:?}"
                )),
            }
        }
        return;
    }
    // Atomic once-claim for the fresh install (installed == NOT_INSTALLED here): only the first caller
    // proceeds; reentrant callers see YES and bail. Prevents the double MhHook::new/enable reentrancy that
    // left the hook non-deterministically un-enabled -> the native deserialize crashed on char#1's stale table
    // (2026-07-15, same fix as MenuWindowJob::Run). Rolled back to NOT on real failure; MH_ERROR_ENABLED == on.
    if SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED
        .compare_exchange(
            SYSTEM_QUIT_GAITEM_DESERIALIZE_NOT_INSTALLED,
            SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED_YES,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_err()
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MH_Initialize for CSGaitemImp::Deserialize hook failed: {status:?}"
            ));
            SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED
                .store(SYSTEM_QUIT_GAITEM_DESERIALIZE_NOT_INSTALLED, Ordering::SeqCst);
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_GAITEM_DESERIALIZE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve CSGaitemImp::Deserialize rva 0x{SYSTEM_QUIT_GAITEM_DESERIALIZE_RVA:x}"
        ));
        SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED
            .store(SYSTEM_QUIT_GAITEM_DESERIALIZE_NOT_INSTALLED, Ordering::SeqCst);
        return;
    };
    SYSTEM_QUIT_GAITEM_DESERIALIZE_ADDR.store(addr, Ordering::SeqCst);
    let created = match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_gaitem_deserialize_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_GAITEM_DESERIALIZE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            std::mem::forget(hook);
            true
        }
        Err(MH_STATUS::MH_ERROR_ALREADY_CREATED) => false,
        Err(status) => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MhHook::new CSGaitemImp::Deserialize hook failed: {status:?}"
            ));
            SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED
                .store(SYSTEM_QUIT_GAITEM_DESERIALIZE_NOT_INSTALLED, Ordering::SeqCst);
            return;
        }
    };
    match unsafe { crate::mh::MH_EnableHook(addr as *mut c_void) } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ENABLED => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: hooked CSGaitemImp::Deserialize 0x{addr:x} (immediate enable, created={created}); transition deserialize skip + stale-table reset active"
            ));
        }
        status => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MH_EnableHook CSGaitemImp::Deserialize hook failed: {status:?}"
            ));
            SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED
                .store(SYSTEM_QUIT_GAITEM_DESERIALIZE_NOT_INSTALLED, Ordering::SeqCst);
        }
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

pub(crate) fn install_system_quit_gameman_load_save_hook() {
    let installed = SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED.load(Ordering::SeqCst);
    if installed == SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED_YES {
        return;
    }
    if installed == SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_DISABLED {
        let addr = SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ADDR.load(Ordering::SeqCst);
        if addr != 0 {
            match unsafe { MH_QueueEnableHook(addr as *mut c_void) } {
                MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
                    MH_STATUS::MH_OK => {
                        SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED.store(
                            SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED_YES,
                            Ordering::SeqCst,
                        );
                        append_autoload_debug(format_args!(
                            "system-quit-quickload: re-enabled GameMan load-save hook 0x{addr:x}; transition loads blocked until native title handoff"
                        ));
                    }
                    status => append_autoload_debug(format_args!(
                        "system-quit-quickload: MH_ApplyQueued re-enable GameMan load-save hook failed: {status:?}"
                    )),
                },
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: queue re-enable GameMan load-save hook failed: {status:?}"
                )),
            }
        }
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MH_Initialize for GameMan load-save hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve GameMan load-save rva 0x{SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_RVA:x}"
        ));
        return;
    };
    SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ADDR.store(addr, Ordering::SeqCst);
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_gameman_load_save_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-quickload: queue_enable GameMan load-save hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED.store(
                        SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED_YES,
                        Ordering::SeqCst,
                    );
                    append_autoload_debug(format_args!(
                        "system-quit-quickload: hooked GameMan load-save 0x{addr:x}; transition loads blocked until native title handoff"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: MH_ApplyQueued GameMan load-save hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-quickload: MhHook::new GameMan load-save hook failed: {status:?}"
        )),
    }
}

pub(crate) fn disable_system_quit_gameman_load_save_hook(source: &str) {
    if SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED_YES
    {
        return;
    }
    let addr = SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ADDR.load(Ordering::SeqCst);
    if addr == 0 {
        return;
    }
    match unsafe { MH_QueueDisableHook(addr as *mut c_void) } {
        MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
            MH_STATUS::MH_OK => {
                SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED
                    .store(SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_DISABLED, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-quickload: disabled GameMan load-save hook 0x{addr:x} before native Continue source={source}"
                ));
            }
            status => append_autoload_debug(format_args!(
                "system-quit-quickload: MH_ApplyQueued disable GameMan load-save hook failed source={source}: {status:?}"
            )),
        },
        status => append_autoload_debug(format_args!(
            "system-quit-quickload: queue_disable GameMan load-save hook failed source={source}: {status:?}"
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
        .compare_exchange(not_installed, installed_yes, Ordering::SeqCst, Ordering::SeqCst)
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

fn install_system_quit_profile_load_activate_hook() {
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

fn install_system_quit_profile_load_confirmed_hook() {
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

fn install_system_quit_profile_load_job_run_hook() {
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
