/// Read the TitleTopDialog FD4 state machine by NAME (is_in_state) given the title `owner` (rcx of
/// STEP_MenuJobWait). Returns `(dialog_ptr, in_fadein, in_loop, in_textfadeout, menu_opened_latch)` or
/// `None` if the dialog isn't the TitleTopDialog yet. Read-only / no side effects. Mirrors STAGE1d.
unsafe fn title_dialog_sm_state(
    owner: usize,
    base: usize,
) -> Option<(usize, bool, bool, bool, usize)> {
    if owner == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let dialog = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(0);
    if dialog == 0 {
        return None;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(0);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return None;
    }
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
    let in_fadein =
        unsafe { is_in_state(sm, base + TITLE_STATE_DESC_FADEIN_RVA) } != OWN_STEPPER_FALSE;
    let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) } != OWN_STEPPER_FALSE;
    let in_textfadeout =
        unsafe { is_in_state(sm, base + TITLE_STATE_DESC_TEXTFADEOUT_RVA) } != OWN_STEPPER_FALSE;
    let latch = unsafe { safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET) }
        .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
        .unwrap_or(0);
    Some((dialog, in_fadein, in_loop, in_textfadeout, latch))
}

/// Skip the title FadeIn ONCE: the first frame the dialog SM is settled in FadeIn (menu-open latch
/// clear), drive the FD4 state machine FadeIn->Loop by calling the game's OWN transition `SetState`
/// (deobf 0x1407499e0) with `(sm = dialog+0xa60, desc = Loop 0x142a8f9e8)`. This is EXACTLY the call
/// `CS::TitleTopDialog::update`'s input-skip branch makes on a confirm/cancel press (Ghidra: bd
/// fadein-* RE), so it is save-safe and routes through the SM's own vtable[0x150] request path (no
/// struct stomp) -- but ZERO input. `SetState` internally no-ops unless the current node is settled
/// (`[node+0x20]&0x8f >= 2`), so an early call before the node is eligible cannot corrupt the SM.
/// One-shot via `TITLE_FADEIN_SKIP_FIRED`; the dt-scale / frame-burst / anim-complete-predicate levers
/// were all runtime-falsified (bd title-anim-framedelta / pab-to-menuopen-real-breakdown / fadein-
/// predicate-75cea0). The FadeIn IS frame-paced animation -- it is just skipped by the state transition,
/// not by pacing.
unsafe fn title_anim_fadein_skip(owner: usize) {
    if TITLE_FADEIN_SKIP_FIRED.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS {
        return; // one-shot: already transitioned
    }
    if IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES {
        return;
    }
    if !(title_anim_speedup_factor() > TITLE_ANIM_SPEEDUP_MIN) {
        return; // lever off / forced to 1.0
    }
    let Ok(base) = game_module_base() else {
        return;
    };
    let st = unsafe { title_dialog_sm_state(owner, base) };
    // Light diagnostic so the SM timeline stays visible across boots.
    let n = TITLE_ANIM_DIAG_CALLS.fetch_add(1, Ordering::SeqCst);
    if n % TITLE_ANIM_DIAG_INTERVAL == 0 {
        append_autoload_debug(format_args!(
            "title-anim-diag: detour#{n} sm(dialog,fadein,loop,tfo,latch)={st:?}"
        ));
    }
    let Some((dialog, true, _, _, latch)) = st else {
        return; // not the TitleTopDialog, or not in FadeIn yet
    };
    if latch != TITLE_OWNER_SCAN_START_ADDRESS {
        return; // menu already opening -> leave the SM alone
    }
    // Fire the game's own FadeIn->Loop transition once (zero-input).
    if TITLE_FADEIN_SKIP_FIRED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return; // lost the one-shot race
    }
    let set_state: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(base + TITLE_FD4_SETSTATE_RVA) };
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    unsafe { set_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) };
    append_autoload_debug(format_args!(
        "title-anim-skip: *** SetState(sm=0x{sm:x}, Loop) via 0x{:x} -- zero-input FadeIn->Loop transition (game's own input-skip path, save-safe), skipping the title fade ***",
        base + TITLE_FD4_SETSTATE_RVA
    ));
}

/// Detour for STEP_MenuJobWait (0x140b0d400, `__fastcall(rcx=owner, rdx=task_data, ...)`). Drives the
/// one-shot FadeIn->Loop skip from the live SM state, then passes through to the original unchanged.
pub(crate) unsafe extern "system" fn title_menujob_speed_detour(
    owner: usize,
    task_data: usize,
    r8: usize,
    r9: usize,
) -> usize {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        title_anim_fadein_skip(owner)
    }));
    let orig_addr = TITLE_ANIM_SPEED_ORIG.load(Ordering::SeqCst);
    if orig_addr == TITLE_OWNER_SCAN_START_ADDRESS {
        return 0;
    }
    let orig: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig_addr) };
    unsafe { orig(owner, task_data, r8, r9) }
}

/// Install the title-anim speedup hook ONCE (MinHook, mirroring `install_pab_advance_hook`). Gated by
/// `title_anim_speedup_enabled` at the call site; the detour self-gates per frame too.
pub(crate) unsafe fn install_title_anim_speed_hook(base: usize) {
    if TITLE_ANIM_SPEED_HOOK_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-anim-speed-hook: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "title_menujob_speed_b0d400",
            TITLE_MENU_JOB_WAIT_RVA as u32,
            title_menujob_speed_detour as *mut c_void,
            &TITLE_ANIM_SPEED_ORIG,
        );
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "title-anim-speed-hook: INSTALLED on STEP_MenuJobWait 0x{:x} -- one-shot FadeIn->Loop skip armed (zero-input, save-safe)",
            base + TITLE_MENU_JOB_WAIT_RVA,
        )),
        status => append_autoload_debug(format_args!(
            "title-anim-speed-hook: MH_ApplyQueued failed: {status:?}"
        )),
    }
    std::mem::forget(hooks);
}
/// READ-ONLY trace detour for the title step-setter `SetState(owner, int state)` (deobf 0x140b0d960).
/// Logs every native state transition with a timestamp + the current owner+0xe0 (TitleTopDialog
/// holder) liveness, then calls the original UNCHANGED. Pure observation -- this is the
/// "look before acting" instrument for the menu-build-overlap lever: it reveals the exact wall-clock
/// at which BeginTitle(3) fires natively (and the full state sequence during boot), so we can decide
/// whether the 05_000_Title build has any headroom to be started earlier (overlap with init) before
/// risking a forced SetState (which has NO double-build guard). bd menu-build-overlap-lever-2026-06-24.
pub(crate) unsafe extern "system" fn title_setstate_trace_detour(owner: usize, state: i32) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if owner > PAB_MIN_HEAP_PTR {
            TITLE_SETSTATE_TRACE_LAST_OWNER.store(owner, Ordering::SeqCst);
        }
        let dialog = if owner > PAB_MIN_HEAP_PTR {
            unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(0)
        } else {
            0
        };
        let committed = if owner > PAB_MIN_HEAP_PTR {
            unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }.unwrap_or(-999)
        } else {
            -999
        };
        let b8 = if owner > PAB_MIN_HEAP_PTR {
            unsafe { safe_read_usize(owner + TITLE_OWNER_BEGINLOGO_LIST_GATE_B8_OFFSET) }
                .unwrap_or(0)
        } else {
            0
        };
        if owner > PAB_MIN_HEAP_PTR
            && SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
                >= SYSTEM_QUIT_QUICKLOAD_PHASE_TITLE_OWNER_SEEN
            && (state == 2 || state == 3 || state == TITLE_STEP_MENU_JOB_WAIT)
        {
            if let Ok(base) = game_module_base() {
                let table = unsafe { safe_read_usize(owner + TITLE_OWNER_INSTANCE_TABLE_OFFSET) }
                    .unwrap_or(0);
                if table == base + INNER_TITLE_STATE_TABLE_RVA {
                    let previous = TITLE_OWNER_PTR.swap(owner, Ordering::SeqCst);
                    TITLE_OWNER_SCAN_COUNTDOWN
                        .store(TITLE_OWNER_SCAN_CALL_INTERVAL, Ordering::SeqCst);
                    if previous != owner {
                        append_autoload_debug(format_args!(
                            "system-quit-quickload: latched native SetState title owner=0x{owner:x} state={state} previous=0x{previous:x} table=0x{table:x}; overriding stale scan candidate"
                        ));
                    }
                }
            }
        }
        append_autoload_debug(format_args!(
            "title-setstate-trace: SetState(owner=0x{owner:x}, state={state}) committed_was={committed} owner+0xe0(dialog)=0x{dialog:x} owner+0xb8(gate)=0x{b8:x}"
        ));
    }));
    let orig = TITLE_SETSTATE_TRACE_ORIG.load(Ordering::SeqCst);
    if orig == TITLE_OWNER_SCAN_START_ADDRESS || orig == 0 {
        return;
    }
    wait_for_missing_save_selection_if_pending("title SetState");
    let f: unsafe extern "system" fn(usize, i32) = unsafe { std::mem::transmute(orig) };
    unsafe { f(owner, state) };
}
/// Install the READ-ONLY title step-setter trace hook ONCE. Mirrors `install_pab_advance_hook`.
/// Save-safe: the detour only logs + passes through. bd menu-build-overlap-lever-2026-06-24.
pub(crate) unsafe fn install_title_setstate_trace_hook(base: usize) {
    if TITLE_SETSTATE_TRACE_HOOK_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-setstate-trace-hook: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "title_setstate_b0d960",
            TITLE_SET_STATE_RVA as u32,
            title_setstate_trace_detour as *mut c_void,
            &TITLE_SETSTATE_TRACE_ORIG,
        );
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "title-setstate-trace-hook: INSTALLED on SetState(owner,int) 0x{:x} -- read-only native state-transition timeline armed",
            base + TITLE_SET_STATE_RVA,
        )),
        status => append_autoload_debug(format_args!(
            "title-setstate-trace-hook: MH_ApplyQueued failed: {status:?}"
        )),
    }
    std::mem::forget(hooks);
}
/// Per-frame PUMP for the built LoadGame job (bd drain-dialog-plus8-not-menujob-pump-our-job-directly).
/// Runs from the recurring game task once `maybe_fire_tfc_continue` armed `TFC_DRAIN_JOB`. Calls
/// `ExecuteMenuJob(rcx = &job_slot, rdx = &FD4Time)` DIRECTLY on our built job -- it invokes the job's
/// own `vtable[2]` (the LoadGame chain's Execute), advancing deser/world-stream, and zeroes the slot
/// when done (`ShouldContinue==false`). We pump OUR job (not the dialog's `+0x8` slot, which is not a
/// MenuJob and AV'd the queue-drain wrapper). Pure native call (no input). Stops on completion (slot
/// cleared), in-world, panic, or the tick cap. Every call is `catch_unwind`-guarded.
pub(crate) unsafe fn tfc_continue_drain_tick(base: usize, frame_delta: f32) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let job = TFC_DRAIN_JOB.load(Ordering::SeqCst);
    if job == 0 || job == null {
        return;
    }
    if IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES {
        TFC_DRAIN_JOB.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "tfc-drain: in-world reached -> stop pumping (load complete)"
        ));
        return;
    }
    let ticks = TFC_DRAIN_TICKS.fetch_add(1, Ordering::SeqCst) + 1;
    if ticks > TFC_DRAIN_TICK_CAP {
        TFC_DRAIN_JOB.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "tfc-drain: tick cap {TFC_DRAIN_TICK_CAP} hit -> stop pumping (job never completed)"
        ));
        return;
    }
    // FD4Time: ExecuteMenuJob reads only +0x8 (f32 delta). Pass a 16-byte buffer with the frame delta.
    let mut time: [u8; FD4_TIME_SIZE] = [0u8; FD4_TIME_SIZE];
    time[FD4_TIME_DELTA_8_OFFSET..FD4_TIME_DELTA_8_OFFSET + core::mem::size_of::<f32>()]
        .copy_from_slice(&frame_delta.to_le_bytes());
    let time_ptr = time.as_mut_ptr() as usize;
    // ExecuteMenuJob(rcx = &job_slot, rdx = &FD4Time): cur=*rcx; AtomicInc(cur+8); cur->vtable[2](...);
    // if done -> *rcx=0. Pass a local slot (job ptr persists in TFC_DRAIN_JOB across frames).
    let mut job_slot: usize = job;
    let slot_ptr = (&raw mut job_slot) as usize;
    let exec: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(base + EXECUTE_MENU_JOB_RVA) };
    let exec_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        exec(slot_ptr, time_ptr)
    }));
    if exec_ret.is_err() {
        TFC_DRAIN_JOB.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "tfc-drain: ExecuteMenuJob 0x{:x}(rcx=&job=0x{job:x}) PANICKED (caught) at tick {ticks} -> stop pumping",
            base + EXECUTE_MENU_JOB_RVA
        ));
        return;
    }
    if job_slot == 0 {
        TFC_DRAIN_JOB.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "tfc-drain: job 0x{job:x} COMPLETED (slot cleared by ExecuteMenuJob) at tick {ticks} -> done pumping"
        ));
        return;
    }
    if ticks == 1 || ticks % (OWN_LOAD_STREAM_LOG_INTERVAL as usize) == 0 {
        append_autoload_debug(format_args!(
            "tfc-drain: tick {ticks} ExecuteMenuJob(job=0x{job:x}) delta={frame_delta} (pumping)"
        ));
    }
}
/// The D-pad Down button mask to inject for poll-frame `n` (counted from the first poll after
/// menu-open), per the INJECT_NAV schedule: settle, then `INJECT_NAV_MAX_CYCLES` tap+gap cycles
/// with Down asserted for the first `INJECT_NAV_TAP_LEN` frames of each cycle. Returns 0 (no
/// input) during settle, gaps, and after the cycles complete.
pub(crate) fn inject_nav_buttons(n: usize) -> u16 {
    const NONE: u16 = 0;
    if n < INJECT_NAV_SETTLE_FRAMES {
        return NONE;
    }
    let m = n - INJECT_NAV_SETTLE_FRAMES;
    if m >= INJECT_NAV_MAX_CYCLES * INJECT_NAV_CYCLE {
        return NONE;
    }
    if m % INJECT_NAV_CYCLE < INJECT_NAV_TAP_LEN {
        XINPUT_GAMEPAD_DPAD_DOWN
    } else {
        NONE
    }
}
/// Tap Confirm (inputmgr+0x90+0x3d, edge) to walk the NATURAL flow:
/// press-any-button -> [confirm] -> connection-error modal -> [confirm] -> MAIN MENU.
/// STOPS once the modal has been SEEN and is now GONE, so we never confirm a main-menu item
/// (Continue = load most-recent = SetState(5) save-write risk). Pure observation of the post-modal
/// view. Uses the builder capture hook only to know when the modal is up.
pub(crate) fn auto_confirm_tap() {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let Ok(base) = game_module_base() else {
        return;
    };
    install_auto_accept_hook();
    let modal_now = CONNECTION_ERROR_DIALOG.load(Ordering::SeqCst) != null;
    if modal_now {
        AUTO_CONFIRM_MODAL_SEEN.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    let seen = AUTO_CONFIRM_MODAL_SEEN.load(Ordering::SeqCst) != null;
    if seen && !modal_now {
        // Past the modal -> stop tapping (do NOT confirm Continue on the main menu).
        return;
    }
    let inputmgr =
        unsafe { safe_read_usize(base + SELECTBOT_INPUT_MANAGER_GLOBAL_RVA) }.unwrap_or(null);
    if inputmgr == null {
        return;
    }
    let frame = AUTO_CONFIRM_FRAME.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
    if frame % AUTO_CONFIRM_CYCLE_FRAMES < AUTO_CONFIRM_SET_FRAMES {
        unsafe {
            *((inputmgr + INPUTMGR_BITMAP_90_OFFSET + MENU_EVENT_CONFIRM_3D) as *mut u8) |=
                MENU_EVENT_PRESSED_BIT;
        }
    }
    if frame % AUTO_CONFIRM_LOG_INTERVAL == null as u64 {
        append_autoload_debug(format_args!(
            "auto-confirm: tap frame={frame} modal_now={modal_now} seen={seen} inputmgr=0x{inputmgr:x}"
        ));
    }
}
pub(crate) unsafe fn title_press_button_component_ready(
    dialog: usize,
    base: usize,
) -> Option<TitlePressButtonComponent> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let proxy = dialog + TITLE_PRESS_START_SCENE_PROXY_B78_OFFSET;
    let proxy_vt = unsafe { safe_read_usize(proxy) }.unwrap_or(null);
    if proxy_vt != base + SCENE_OBJ_PROXY_VTABLE_RVA {
        return None;
    }
    let context =
        unsafe { safe_read_usize(proxy + SCENE_OBJ_PROXY_CONTEXT_20_OFFSET) }.unwrap_or(null);
    if context == null {
        return None;
    }
    Some(TitlePressButtonComponent { proxy, context })
}
pub(crate) unsafe fn title_dialog_state(dialog: usize, base: usize) -> TitleDialogState {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
    let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) } != OWN_STEPPER_FALSE;
    let in_textfadeout =
        unsafe { is_in_state(sm, base + TITLE_STATE_DESC_TEXTFADEOUT_RVA) } != OWN_STEPPER_FALSE;
    let menu_opened_latch =
        unsafe { safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET) }
            .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
            .unwrap_or(null);
    TitleDialogState {
        in_loop,
        in_textfadeout,
        menu_opened_latch,
    }
}
pub(crate) unsafe fn title_boot_ready(owner: usize, base: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let committed = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    let requested = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    let table =
        unsafe { safe_read_usize(owner + TITLE_OWNER_INSTANCE_TABLE_OFFSET) }.unwrap_or(null);
    let session =
        unsafe { safe_read_usize(base + SESSION_SINGLETON_144588E98_RVA) }.unwrap_or(null);
    let dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
    let dialog_vt = if dialog != null {
        unsafe { safe_read_usize(dialog) }.unwrap_or(null)
    } else {
        null
    };
    if committed != TITLE_STEP_MENU_JOB_WAIT
        || requested != TITLE_STEP_MENU_JOB_WAIT
        || table != base + INNER_TITLE_STATE_TABLE_RVA
        || session == null
        || dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA
        || unsafe { title_press_button_component_ready(dialog, base) }.is_none()
    {
        return false;
    }
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
    let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) } != OWN_STEPPER_FALSE;
    let in_textfadeout =
        unsafe { is_in_state(sm, base + TITLE_STATE_DESC_TEXTFADEOUT_RVA) } != OWN_STEPPER_FALSE;
    in_loop || in_textfadeout
}
pub(crate) unsafe fn title_scheduler_ready(owner: usize, base: usize) -> bool {
    unsafe { title_boot_ready(owner, base) }
}
pub(crate) unsafe fn product_core_autoload_ready(
    owner: usize,
    base: usize,
    gm: usize,
    slot: i32,
) -> Option<ProductCoreAutoloadReady> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if slot < OWN_STEPPER_SLOT_ZERO || gm == null {
        return None;
    }
    let committed = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    let requested = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    let table =
        unsafe { safe_read_usize(owner + TITLE_OWNER_INSTANCE_TABLE_OFFSET) }.unwrap_or(null);
    let session =
        unsafe { safe_read_usize(base + SESSION_SINGLETON_144588E98_RVA) }.unwrap_or(null);
    let game_data_man = game_data_man_ptr_or_null();
    let profile_summary = if game_data_man != null {
        unsafe { safe_read_usize(game_data_man + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(null)
    } else {
        null
    };
    let iodev = unsafe { safe_read_usize(base + IODEV_GLOBAL_RVA) }.unwrap_or(null);
    let heap_allocator = crate::runtime_heap_allocator_ptr_or_null();
    let dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
    let dialog_vt = if dialog != null {
        unsafe { safe_read_usize(dialog) }.unwrap_or(null)
    } else {
        null
    };
    let press_start = if dialog_vt == base + TITLE_TOP_DIALOG_VTABLE_RVA {
        unsafe { title_press_button_component_ready(dialog, base) }
    } else {
        None
    };
    let title_state = if dialog_vt == base + TITLE_TOP_DIALOG_VTABLE_RVA {
        Some(unsafe { title_dialog_state(dialog, base) })
    } else {
        None
    };
    if committed != TITLE_STEP_MENU_JOB_WAIT
        || requested != TITLE_STEP_MENU_JOB_WAIT
        || table != base + INNER_TITLE_STATE_TABLE_RVA
        || session == null
        || game_data_man == null
        || profile_summary == null
        || iodev == null
        || heap_allocator == null
        || press_start.is_none()
        || title_state.is_none()
    {
        return None;
    }
    let press_start = press_start?;
    let title_state = title_state?;
    Some(ProductCoreAutoloadReady {
        committed,
        requested,
        table,
        session,
        game_data_man,
        profile_summary,
        iodev,
        heap_allocator,
        title_dialog: dialog,
        title_in_loop: title_state.in_loop,
        title_in_textfadeout: title_state.in_textfadeout,
        menu_opened_latch: title_state.menu_opened_latch,
        press_start_proxy: press_start.proxy,
        press_start_context: press_start.context,
    })
}
unsafe fn hide_title_press_start_proxy(base: usize, dialog: usize, proxy: usize, context: usize) {
    if proxy == TITLE_OWNER_SCAN_START_ADDRESS || proxy == 0 {
        return;
    }
    let value = proxy + 0x18;
    TITLE_PRESS_START_GFX_VALUE.store(value, Ordering::SeqCst);
    let set_visible: unsafe extern "system" fn(usize, u8) =
        unsafe { std::mem::transmute(base + TITLE_PRESS_START_SET_VISIBLE_RVA) };
    unsafe { set_visible(proxy, 0) };
    let prev = TITLE_PRESS_START_GFX_HIDE_CALLS.fetch_add(1, Ordering::SeqCst);
    TITLE_PRESS_START_GFX_HIDE_LAST_DIALOG.store(dialog, Ordering::SeqCst);
    TITLE_PRESS_START_GFX_HIDE_LAST_PROXY.store(proxy, Ordering::SeqCst);
    TITLE_PRESS_START_GFX_HIDE_LAST_CONTEXT.store(context, Ordering::SeqCst);
    TITLE_PRESS_START_GFX_HIDE_LAST_CALLER_PHASE
        .store(OWN_STEPPER_PHASE.load(Ordering::SeqCst), Ordering::SeqCst);
    if prev == 0 {
        append_autoload_debug(format_args!(
            "title-cover-part-a: hid 05_000_Title PressStart/StaticSystemText_101000 via SceneObjProxy visibility wrapper 0x{:x} dialog=0x{dialog:x} proxy=0x{proxy:x} context=0x{context:x}",
            base + TITLE_PRESS_START_SET_VISIBLE_RVA,
        ));
    }
}

pub(crate) unsafe fn maybe_hide_title_press_start(base: usize, ready: &ProductCoreAutoloadReady) {
    unsafe {
        hide_title_press_start_proxy(
            base,
            ready.title_dialog,
            ready.press_start_proxy,
            ready.press_start_context,
        )
    };
}

pub(crate) unsafe fn maybe_hide_title_logo_surface(base: usize, ready: &ProductCoreAutoloadReady) {
    if ready.title_dialog == TITLE_OWNER_SCAN_START_ADDRESS || ready.title_dialog == 0 {
        return;
    }
    let logo = ready.title_dialog + TITLE_LOGO_BACK_VIEW_PARTS_AA8_OFFSET;
    if unsafe { safe_read_usize(logo) }.is_none() {
        return;
    }
    let set_visible: unsafe extern "system" fn(usize, u8) =
        unsafe { std::mem::transmute(base + TITLE_LOGO_BACK_VIEW_PARTS_SET_VISIBLE_RVA) };
    unsafe { set_visible(logo, 0) };
    let prev = TITLE_LOGO_GFX_HIDE_CALLS.fetch_add(1, Ordering::SeqCst);
    TITLE_LOGO_GFX_HIDE_LAST_DIALOG.store(ready.title_dialog, Ordering::SeqCst);
    TITLE_LOGO_GFX_HIDE_LAST_LOGO.store(logo, Ordering::SeqCst);
    TITLE_LOGO_GFX_HIDE_LAST_CALLER_PHASE
        .store(OWN_STEPPER_PHASE.load(Ordering::SeqCst), Ordering::SeqCst);
    if prev == 0 {
        append_autoload_debug(format_args!(
            "title-cover-part-a: hid {TITLE_LOGO_BACK_VIEW_PARTS_NAME}/{TITLE_LOGO_RESOURCE_NAME} via native SceneObjProxy visibility wrapper 0x{:x} dialog=0x{:x} logo=0x{logo:x}",
            base + TITLE_LOGO_BACK_VIEW_PARTS_SET_VISIBLE_RVA,
            ready.title_dialog,
        ));
    }
}

pub(crate) unsafe fn sample_title_profile_portrait_source(base: usize, slot: i32) -> bool {
    if slot < OWN_STEPPER_SLOT_ZERO {
        return false;
    }
    let slot = slot as usize;
    if slot >= TITLE_PROFILE_SLOT_COUNT {
        return false;
    }
    let renderer_slot =
        base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_TABLE_RVA + slot * core::mem::size_of::<usize>();
    let renderer =
        unsafe { safe_read_usize(renderer_slot) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let renderer_vtable = if renderer != TITLE_OWNER_SCAN_START_ADDRESS && renderer != 0 {
        unsafe { safe_read_usize(renderer) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let offscreen = if renderer_vtable == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA {
        unsafe {
            safe_read_usize(renderer + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
        }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let tex_rescap = if offscreen != TITLE_OWNER_SCAN_START_ADDRESS && offscreen != 0 {
        unsafe {
            safe_read_usize(offscreen + TITLE_CUSTOM_COVER_PROFILE_OFFSCREEN_TEX_RESCAP_OFFSET)
        }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let tex_index = if renderer_vtable == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA {
        unsafe { safe_read_usize(renderer + TITLE_CUSTOM_COVER_PROFILE_RENDERER_TEX_INDEX_OFFSET) }
            .map(|value| value & 0xffff_ffff)
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let ready_754 = if renderer != TITLE_OWNER_SCAN_START_ADDRESS && renderer != 0 {
        unsafe { safe_read_u8(renderer + TITLE_CUSTOM_COVER_PROFILE_RENDER_READY_FIELD_754) }
            .map(usize::from)
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let ready_755 = if renderer != TITLE_OWNER_SCAN_START_ADDRESS && renderer != 0 {
        unsafe { safe_read_u8(renderer + TITLE_CUSTOM_COVER_PROFILE_RENDER_READY_FIELD_755) }
            .map(usize::from)
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    TITLE_CUSTOM_COVER_PROFILE_SOURCE_SAMPLE_CALLS.fetch_add(1, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_PROFILE_SOURCE_SLOT.store(slot, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_PROFILE_SOURCE_RENDERER.store(renderer, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_PROFILE_SOURCE_RENDERER_VTABLE.store(renderer_vtable, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_PROFILE_SOURCE_OFFSCREEN_REND.store(offscreen, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_PROFILE_SOURCE_TEX_RESCAP.store(tex_rescap, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_PROFILE_SOURCE_TEX_INDEX.store(tex_index, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_PROFILE_SOURCE_READY_754.store(ready_754, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_PROFILE_SOURCE_READY_755.store(ready_755, Ordering::SeqCst);
    renderer_vtable == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
        && offscreen != TITLE_OWNER_SCAN_START_ADDRESS
        && offscreen != 0
        && tex_rescap != TITLE_OWNER_SCAN_START_ADDRESS
        && tex_rescap != 0
        && ready_754 != TITLE_OWNER_SCAN_START_ADDRESS
        && ready_755 != TITLE_OWNER_SCAN_START_ADDRESS
}

/// Build the er-tpf Tier-4 cover blob ONCE and cache it for the process lifetime. A bright
/// magenta/white checker (unmistakable on the loading-screen-portrait screenshot), encoded uncompressed
/// `R8G8B8A8_UNORM` with a LEGACY DDS header (maps to DXGI 28 and bypasses the DX10 format validator),
/// wrapped in a one-entry TPF003 whose ENTRY NAME == `ER_TPF_COVER_SYSTEX_KEY` (which becomes the
/// `GLOBAL_TexRepository` GPU key the Scaleform bridge resolves). Held alive forever so the engine's
/// deferred GPU upload can never read freed bytes. Pure CPU; no native call, no disk.
fn er_tpf_cover_blob() -> &'static [u8] {
    static BLOB: OnceLock<Vec<u8>> = OnceLock::new();
    BLOB.get_or_init(|| {
        let img = DdsImage::checker(
            ER_TPF_COVER_TEX_DIM,
            ER_TPF_COVER_TEX_DIM,
            ER_TPF_COVER_TEX_CELL,
            [255, 0, 255, 255],   // magenta
            [255, 255, 255, 255], // white
        );
        let dds = img.to_dds_bytes_with(DdsHeaderMode::LegacyRgba8);
        match Tpf::single_pc(ER_TPF_COVER_SYSTEX_KEY, dds, 1).build() {
            Ok(bytes) => {
                ER_TPF_COVER_TEXTURE_BUILT.store(1, Ordering::SeqCst);
                ER_TPF_COVER_BLOB_LEN.store(bytes.len(), Ordering::SeqCst);
                bytes
            }
            Err(_) => Vec::new(),
        }
    })
}

/// er-tpf Tier-4 ONE-SHOT, fail-closed register of our in-memory cover texture into the live texture
/// repositories via the engine's own raw-(ptr,len) TPF factory `CS::CreateTpfResCap` (deobf
/// `CREATE_TPF_RES_CAP_RVA`), mirroring the FaceGen call exactly. Runs on the CSTaskImp game-task thread
/// (post-graphics-init), NEVER from DllMain/loader. Validates every precondition before the first native
/// call (module base resolved, `GLOBAL_TpfRepository` + `GLOBAL_TexRepository` non-null == gfx up, blob
/// non-empty), wraps the call in `catch_unwind`, and on any failure bumps `ER_TPF_COVER_FAILURES` +
/// records `ER_TPF_COVER_LAST_ERROR` and bails (never crashes). Does NOT consume the one-shot until a
/// real call is attempted, so a not-yet-initialized repo simply retries next tick. The actual DRAW
/// redirect (pointing the visible profile surface's bind TARGET at our key) is a separate one-shot in
/// the Scaleform bind observer, gated on `ER_TPF_COVER_REGISTERED`.
/// RETIRED (2026-06-30, user): the `SYSTEX_ErTpf_Cover00` POC cover -- a 1024x1024 magenta/YELLOW checker
/// -- was the early "prove we own the title/loading surface" test feature. The real character portrait now
/// displays, so it is dead weight AND actively harmful: being the same 1024 size as the head RT, the
/// portrait readback's "largest TEXTURE2D" scan grabbed IT instead of the head (nondeterministic
/// magenta/yellow checker on the loading screen). The registration is removed -- this is now a no-op.
pub(crate) unsafe fn maybe_register_er_tpf_cover_texture(_base: usize) {}

pub(crate) unsafe fn maybe_refresh_title_profile_cover(
    base: usize,
    ready: &ProductCoreAutoloadReady,
) {
    if ready.profile_summary == TITLE_OWNER_SCAN_START_ADDRESS || ready.profile_summary == 0 {
        return;
    }
    if TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_CALLS
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let init: unsafe extern "system" fn() =
        unsafe { std::mem::transmute(base + TITLE_CUSTOM_COVER_PROFILE_RENDER_INIT_RVA) };
    let refresh: unsafe extern "system" fn() =
        unsafe { std::mem::transmute(base + TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_RVA) };
    unsafe { init() };
    unsafe { sample_title_profile_portrait_source(base, OWN_STEPPER_SLOT_ZERO) };
    unsafe { refresh() };
    TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_LAST_PROFILE_SUMMARY
        .store(ready.profile_summary, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_LAST_CALLER_PHASE
        .store(OWN_STEPPER_PHASE.load(Ordering::SeqCst), Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-b: initialized profile renderer table via 0x{:x}, refreshed post-SL2 profile portrait render targets via 0x{:x} profile_summary=0x{:x} target={TITLE_CUSTOM_COVER_SYSTEX_TARGET} renderer={TITLE_CUSTOM_COVER_PROFILE_RENDERER_CLASS}",
        base + TITLE_CUSTOM_COVER_PROFILE_RENDER_INIT_RVA,
        base + TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_RVA,
        ready.profile_summary,
    ));
}

pub(crate) unsafe fn product_core_autoload_tick(module_base: usize, slot: i32, tick: u64) -> bool {
    if !product_autoload_enabled() {
        return false;
    }
    PRODUCT_CORE_AUTOLOAD_TICKS.fetch_add(1, Ordering::SeqCst);
    let phase = OWN_STEPPER_PHASE.load(Ordering::SeqCst);
    PRODUCT_CORE_LAST_PHASE.store(phase, Ordering::SeqCst);
    // er-tpf Tier-4: register our in-memory cover texture into the live texture repos as soon as
    // graphics is up (self-gating one-shot; runs on this CSTaskImp game-task thread, post-gfx-init).
    // The visible-surface redirect happens in the Scaleform bind observer once this succeeds.
    unsafe { maybe_register_er_tpf_cover_texture(module_base) };
    // NOTE: the stats-panel neutral-bg register is NOT called here -- this product-core tick only runs
    // on the `direct_menu_load` path (product_autoload_armed), whereas the product `save_requested`
    // autoload never enters it. The register lives on the always-running FrameBegin game task in
    // `spawn_recurring_effects_task` (src/lib.rs) so it fires on every autoload path.
    if phase == OWN_STEPPER_PHASE_DONE {
        return true;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let Some(owner_ptr) = (unsafe { title_owner(module_base) }) else {
        PRODUCT_CORE_READY_BLOCKS.fetch_add(1, Ordering::SeqCst);
        PRODUCT_CORE_LAST_BLOCKER.store(PRODUCT_CORE_BLOCKER_NO_TITLE_OWNER, Ordering::SeqCst);
        if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
            append_autoload_debug(format_args!(
                "product-core-autoload: waiting for title owner before native save-load core tick={tick}"
            ));
        }
        return true;
    };
    let owner = owner_ptr as usize;
    if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        >= SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
    {
        SYSTEM_QUIT_QUICKLOAD_LAST_TITLE_OWNER.store(owner, Ordering::SeqCst);
        if SYSTEM_QUIT_QUICKLOAD_PHASE
            .compare_exchange(
                SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED,
                SYSTEM_QUIT_QUICKLOAD_PHASE_TITLE_OWNER_SEEN,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            SYSTEM_QUIT_QUICKLOAD_TITLE_OWNER_SEEN_COUNT.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "system-quit-quickload: title owner appeared after internal return-title request owner=0x{owner:x}; handing off to product Continue autoload"
            ));
        }
        // NOTE: the return-title chain submit is intentionally NOT done here. This product-core
        // tick runs on the game task, concurrently with the game's menu/Scaleform pump; submitting
        // the return-title job from here races that pump and corrupts Scaleform state (observed:
        // non-deterministic execute-fault into Scaleform string data). The submit is done in
        // menu-pump ownership from the MenuWindowJob::Run hook instead. See bd
        // system-quit-return-title-scaleform-race-2026-07-01.
    }
    PRODUCT_CORE_OWNER_TICKS.fetch_add(1, Ordering::SeqCst);
    PRODUCT_CORE_LAST_OWNER.store(owner, Ordering::SeqCst);
    let gm = game_man_ptr_or_null();
    let return_title_job_predicate_bc4 = if gm != null {
        unsafe { safe_read_usize(gm + GAME_MAN_RETURN_TITLE_JOB_PREDICATE_BC4_OFFSET) }
            .map(|v| v as u32 as usize)
            .unwrap_or(usize::MAX)
    } else {
        usize::MAX
    };
    PRODUCT_CORE_LAST_RETURN_TITLE_JOB_PREDICATE_BC4
        .store(return_title_job_predicate_bc4, Ordering::SeqCst);
    if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        >= SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
        && gm != null
        && slot >= OWN_STEPPER_SLOT_ZERO
    {
        // GameMan+0xb78 is CS::GameMan::GetRequestedSaveSlotLoad: the per-frame MoveMapStep load
        // orchestrator (FUN_140afb970, live) reads it and, when != -1, calls RequestLoadSlot(b78) to
        // load that slot IN-WORLD. So while the OLD world is still up (local player present) b78 MUST
        // stay -1 -- writing the picked slot here arms the very in-world load we are trying to avoid.
        // (Observed 2026-07-01: writing b78=slot while in-world made FUN_140afb970 spin
        // RequestLoadSlot(slot) 4600+ times; with that arm blocked the map machine stuck "loading" and
        // the return-title final functor never fired, so the world never tore down -- the menu just
        // closed leaving a stray cursor.) Only once the world has torn down (player absent) do we set
        // b78=slot so the clean-title autoload loads the picked slot via that same b78 -> RequestLoadSlot
        // path. See bd system-quit-loadjob-success-commits-phantom-load-2026-07-01.
        let world_up = unsafe { PlayerIns::local_player_mut() }.is_ok();
        let b78_val = if world_up {
            OWN_STEPPER_SLOT_NONE
        } else {
            slot
        };
        unsafe { *((gm + GAME_MAN_REQUESTED_SLOT_B78_OFFSET) as *mut i32) = b78_val };
        // NOTE: an earlier attempt repointed GameMan+0xac0 (set_save_slot) here at the clean title.
        // That was proven INSUFFICIENT and misleading: ac0 is a deserialize BYPRODUCT, never read as
        // load input, and repointing it forges the `ac0==expected` deser-evidence the Continue GUARD
        // relies on. The picked slot is now made authoritative by the continue_confirm guard
        // (system_quit_continue_confirm_hook), which drives a fresh feed-deserialize of the picked
        // slot (setting ac0/c30/PGD as its normal byproducts) before the confirm streams. See bd
        // system-quit-ac0-fix-insufficient-cleantitle-load-is-native-mostrecent-2026-07-02 and
        // system-quit-cleantitle-load-is-stale-restream-not-slot-source-2026-07-02.
        if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
            let requested_slot = unsafe { safe_read_i32(gm + GAME_MAN_REQUESTED_SLOT_B78_OFFSET) }
                .unwrap_or(OWN_STEPPER_SLOT_NONE);
            append_autoload_debug(format_args!(
                "system-quit-quickload: requested save-slot load index world_up={world_up} wrote gm_b78={b78_val} (read_back={requested_slot}) selected_slot={slot} phase={} bc4=0x{return_title_job_predicate_bc4:x}",
                SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
            ));
        }
    }
    if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        >= SYSTEM_QUIT_QUICKLOAD_PHASE_TITLE_OWNER_SEEN
        && SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.load(Ordering::SeqCst) > 0
        && return_title_job_predicate_bc4 == GAME_MAN_RETURN_TITLE_JOB_PREDICATE_READY
        && SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        let system_dialog = SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.load(Ordering::SeqCst);
        let queue = if system_dialog != 0 && system_dialog != TITLE_OWNER_SCAN_START_ADDRESS {
            system_dialog + 0x10
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let queue_ready = if queue != TITLE_OWNER_SCAN_START_ADDRESS {
            match game_rva(MENU_JOB_QUEUE_READY_RVA) {
                Ok(ready_addr) => {
                    let ready: unsafe extern "system" fn(usize) -> u8 =
                        unsafe { std::mem::transmute(ready_addr) };
                    unsafe { ready(queue) != 0 }
                }
                Err(_) => false,
            }
        } else {
            false
        };
        match (
            game_rva(SYSTEM_QUIT_RETURN_TITLE_FINAL_JOB_BUILDER_RVA),
            game_rva(MENU_JOB_SUBMIT_RVA),
            queue_ready,
        ) {
            (Ok(builder_addr), Ok(submit_addr), true) => {
                let builder: unsafe extern "system" fn(usize) -> usize =
                    unsafe { std::mem::transmute(builder_addr) };
                let submit: unsafe extern "system" fn(usize, usize) =
                    unsafe { std::mem::transmute(submit_addr) };
                let mut job_slot: usize = 0;
                let job_slot_ptr = (&raw mut job_slot) as usize;
                unsafe { builder(job_slot_ptr) };
                append_autoload_debug(format_args!(
                    "system-quit-quickload: native return-title predicate terminal bc4=0x{return_title_job_predicate_bc4:x}; submitting queued final-functor job builder=0x{builder_addr:x} submit=0x{submit_addr:x} system_dialog=0x{system_dialog:x} queue=0x{queue:x} job=0x{job_slot:x} after suppressed Decision UI"
                ));
                if job_slot != 0 && job_slot != TITLE_OWNER_SCAN_START_ADDRESS {
                    unsafe { submit(queue, job_slot_ptr) };
                } else {
                    SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT.store(0, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-quickload: final-functor job builder produced no plausible job builder=0x{builder_addr:x} job=0x{job_slot:x}"
                    ));
                }
            }
            (builder_result, submit_result, ready) => {
                SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT.store(0, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-quickload: final-functor queued submit deferred at bc4=0x{return_title_job_predicate_bc4:x} builder_ok={} submit_ok={} queue_ready={ready} system_dialog=0x{system_dialog:x} queue=0x{queue:x}",
                    builder_result.is_ok(),
                    submit_result.is_ok()
                ));
            }
        }
    }
    if phase == OWN_STEPPER_PHASE_S2_INVOKE
        || phase == OWN_STEPPER_PHASE_S2_ACTIVATE
        || phase == OWN_STEPPER_PHASE_S2_MOUNT_POLL
        || phase == OWN_STEPPER_PHASE_S2_CONFIRM
    {
        unsafe { own_stepper_stage2(owner, module_base, gm, slot, tick, null) };
        return true;
    }
    if phase == OWN_STEPPER_PHASE_MENU
        && FULLREAD_PHASE.load(Ordering::SeqCst) == FULLREAD_PHASE_GUARD
    {
        // Native Continue can reset title-menu visual latches while its modal-confirm branch waits.
        // The product intent is to disable that confirm wait after the native load has produced
        // loaded-slot evidence, so keep the post-submit guard running instead of re-gating on title
        // visuals that are no longer authoritative.
        let guard_ready = ProductCoreAutoloadReady {
            committed: TITLE_STATE_OWNER_GONE,
            requested: TITLE_STATE_OWNER_GONE,
            table: null,
            session: null,
            game_data_man: null,
            profile_summary: null,
            iodev: null,
            heap_allocator: null,
            title_dialog: null,
            title_in_loop: false,
            title_in_textfadeout: false,
            menu_opened_latch: null,
            press_start_proxy: null,
            press_start_context: null,
        };
        unsafe { product_continue_autoload_tick(owner, module_base, gm, slot, tick, &guard_ready) };
        return true;
    }
    let Some(ready) = (unsafe { product_core_autoload_ready(owner, module_base, gm, slot) }) else {
        let committed = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
            .unwrap_or(TITLE_STATE_OWNER_GONE);
        let requested = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_OFFSET) }
            .unwrap_or(TITLE_STATE_OWNER_GONE);
        let table =
            unsafe { safe_read_usize(owner + TITLE_OWNER_INSTANCE_TABLE_OFFSET) }.unwrap_or(null);
        let session = unsafe { safe_read_usize(module_base + SESSION_SINGLETON_144588E98_RVA) }
            .unwrap_or(null);
        let game_data_man = game_data_man_ptr_or_null();
        let profile_summary = if game_data_man != null {
            unsafe { safe_read_usize(game_data_man + SLOT_MANAGER_CONTAINER_OFFSET) }
                .unwrap_or(null)
        } else {
            null
        };
        let iodev = unsafe { safe_read_usize(module_base + IODEV_GLOBAL_RVA) }.unwrap_or(null);
        let heap_allocator = crate::runtime_heap_allocator_ptr_or_null();
        let dialog =
            unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
        let dialog_vt = if dialog != null {
            unsafe { safe_read_usize(dialog) }.unwrap_or(null)
        } else {
            null
        };
        let press_start_proxy = dialog + TITLE_PRESS_START_SCENE_PROXY_B78_OFFSET;
        let press_start_vt = if dialog != null {
            unsafe { safe_read_usize(press_start_proxy) }.unwrap_or(null)
        } else {
            null
        };
        let press_start_context = if press_start_vt == module_base + SCENE_OBJ_PROXY_VTABLE_RVA {
            unsafe { safe_read_usize(press_start_proxy + SCENE_OBJ_PROXY_CONTEXT_20_OFFSET) }
                .unwrap_or(null)
        } else {
            null
        };
        let (title_loop, title_textfadeout, menu_opened_latch) =
            if dialog_vt == module_base + TITLE_TOP_DIALOG_VTABLE_RVA {
                let state = unsafe { title_dialog_state(dialog, module_base) };
                (state.in_loop, state.in_textfadeout, state.menu_opened_latch)
            } else {
                (false, false, null)
            };
        PRODUCT_CORE_LAST_TITLE_DIALOG.store(dialog, Ordering::SeqCst);
        PRODUCT_CORE_LAST_TITLE_DIALOG_VT.store(dialog_vt, Ordering::SeqCst);
        PRODUCT_CORE_LAST_TITLE_IN_LOOP.store(title_loop as usize, Ordering::SeqCst);
        PRODUCT_CORE_LAST_TITLE_IN_TEXTFADEOUT.store(title_textfadeout as usize, Ordering::SeqCst);
        PRODUCT_CORE_LAST_MENU_OPENED_LATCH.store(menu_opened_latch, Ordering::SeqCst);
        PRODUCT_CORE_LAST_PRESS_START_PROXY.store(press_start_proxy, Ordering::SeqCst);
        PRODUCT_CORE_LAST_PRESS_START_VT.store(press_start_vt, Ordering::SeqCst);
        PRODUCT_CORE_LAST_PRESS_START_CONTEXT.store(press_start_context, Ordering::SeqCst);
        let blocker =
            if committed != TITLE_STEP_MENU_JOB_WAIT || requested != TITLE_STEP_MENU_JOB_WAIT {
                PRODUCT_CORE_BLOCKER_TITLE_OWNER_STATE
            } else if table != module_base + INNER_TITLE_STATE_TABLE_RVA {
                PRODUCT_CORE_BLOCKER_TITLE_TABLE
            } else if session == null {
                PRODUCT_CORE_BLOCKER_SESSION
            } else if game_data_man == null {
                PRODUCT_CORE_BLOCKER_GAME_DATA_MAN
            } else if profile_summary == null {
                PRODUCT_CORE_BLOCKER_PROFILE_SUMMARY
            } else if iodev == null {
                PRODUCT_CORE_BLOCKER_IODEV
            } else if heap_allocator == null {
                PRODUCT_CORE_BLOCKER_HEAP_ALLOCATOR
            } else if dialog_vt != module_base + TITLE_TOP_DIALOG_VTABLE_RVA {
                PRODUCT_CORE_BLOCKER_TITLE_DIALOG
            } else if press_start_vt != module_base + SCENE_OBJ_PROXY_VTABLE_RVA
                || press_start_context == null
            {
                PRODUCT_CORE_BLOCKER_PRESS_START
            } else if !title_loop
                && !title_textfadeout
                && menu_opened_latch == OWN_STEPPER_MENU_OPENED_NO
            {
                PRODUCT_CORE_BLOCKER_TITLE_STATE
            } else {
                PRODUCT_CORE_BLOCKER_UNKNOWN
            };
        PRODUCT_CORE_READY_BLOCKS.fetch_add(1, Ordering::SeqCst);
        PRODUCT_CORE_LAST_BLOCKER.store(blocker, Ordering::SeqCst);
        if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
            append_autoload_debug(format_args!(
                "product-core-autoload: waiting for core readiness owner=0x{owner:x} state={committed}/{requested} table=0x{table:x} session=0x{session:x} gm=0x{gm:x} return_title_bc4=0x{return_title_job_predicate_bc4:x} gdm=0x{game_data_man:x} profile=0x{profile_summary:x} iodev=0x{iodev:x} heap=0x{heap_allocator:x} title_loop={title_loop} title_textfadeout={title_textfadeout} menu_latch={menu_opened_latch} press_start_proxy=0x{press_start_proxy:x} press_start_vt=0x{press_start_vt:x} press_start_ctx=0x{press_start_context:x} slot={slot} tick={tick}"
            ));
        }
        return true;
    };
    PRODUCT_CORE_LAST_TITLE_DIALOG.store(ready.title_dialog, Ordering::SeqCst);
    PRODUCT_CORE_LAST_TITLE_DIALOG_VT.store(
        unsafe { safe_read_usize(ready.title_dialog) }.unwrap_or(null),
        Ordering::SeqCst,
    );
    PRODUCT_CORE_LAST_TITLE_IN_LOOP.store(ready.title_in_loop as usize, Ordering::SeqCst);
    PRODUCT_CORE_LAST_TITLE_IN_TEXTFADEOUT
        .store(ready.title_in_textfadeout as usize, Ordering::SeqCst);
    PRODUCT_CORE_LAST_MENU_OPENED_LATCH.store(ready.menu_opened_latch, Ordering::SeqCst);
    PRODUCT_CORE_LAST_PRESS_START_PROXY.store(ready.press_start_proxy, Ordering::SeqCst);
    PRODUCT_CORE_LAST_PRESS_START_VT.store(
        unsafe { safe_read_usize(ready.press_start_proxy) }.unwrap_or(null),
        Ordering::SeqCst,
    );
    PRODUCT_CORE_LAST_PRESS_START_CONTEXT.store(ready.press_start_context, Ordering::SeqCst);
    PRODUCT_CORE_READY_SUCCESSES.fetch_add(1, Ordering::SeqCst);
    PRODUCT_CORE_LAST_BLOCKER.store(PRODUCT_CORE_BLOCKER_READY, Ordering::SeqCst);
    if phase == OWN_STEPPER_PHASE_MENU {
        unsafe { maybe_hide_title_press_start(module_base, &ready) };
        unsafe { maybe_hide_title_logo_surface(module_base, &ready) };
        if ready.menu_opened_latch == OWN_STEPPER_MENU_OPENED_NO {
            unsafe { maybe_refresh_title_profile_cover(module_base, &ready) };
            // Main-branch preservation: do NOT call TitleTopDialog::open_menu from this game-task
            // context. Static disasm of TitleTopDialog::update shows the natural path only calls
            // open_menu from inside the live update frame after the accept gate, then immediately
            // drains the MenuWindow job pump at the tail of the same function. Direct game-task
            // open_menu set a40 but left only the idle Continue candidate observable. Use the decoded
            // zero-input accept byte lever instead and wait for the native update frame to build/drain
            // the real Continue row.
            unsafe { maybe_set_title_accept_byte(module_base) };
            if !TITLE_ACCEPT_BYTE_GATE_FIRED.load(Ordering::SeqCst) {
                if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
                    append_autoload_debug(format_args!(
                        "product-core-autoload: waiting to arm native title accept byte dialog=0x{:x} loop={} textfadeout={} latch={} slot={slot} tick={tick}",
                        ready.title_dialog,
                        ready.title_in_loop,
                        ready.title_in_textfadeout,
                        ready.menu_opened_latch
                    ));
                }
                return true;
            }
            if OWN_STEPPER_MENU_OPENED
                .compare_exchange(
                    OWN_STEPPER_MENU_OPENED_NO,
                    OWN_STEPPER_CALL_INC,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            {
                append_autoload_debug(format_args!(
                    "product-core-autoload: PRESS BUTTON component ready; armed native title accept byte for in-update open-menu/drain (dialog=0x{:x} press_start_proxy=0x{:x}) -- TitleTopDialog::open_menu writes latch and does not require Loop/TextFadeout state; no game-task open_menu self-fire",
                    ready.title_dialog, ready.press_start_proxy
                ));
            }
            return true;
        }
        // After menu-open (a40==1): commit the load. DEFAULT = the PROVEN native Continue char-load
        // (the unchanged block below). The default-OFF ProfileSelect load flow instead fires the
        // Load-Game row to open a LIVE ProfileLoadDialog (the render context in which the profile
        // renderer's per-slot refresh gate is satisfied), HOLDS for the portrait render, then drives
        // the same STAGE2 commit. `profile_select_load_flow_enabled()` is a compile-time const, so
        // when OFF this branch is dead-code-eliminated and execution falls through to the unchanged
        // Continue path below (byte-identical).
        if profile_select_load_flow_enabled() {
            unsafe { product_profile_select_load_flow(owner, module_base, slot, tick) };
            return true;
        }
        // FORCE LIVE PROFILE RENDER (diagnostic, default-OFF) in the autoload path: at the open main
        // menu (renderers live from TitleTopDialog ctor) kick the live character-model build + capture
        // the rendered gx so the now-loading forge can display the real head. One-shot mark+refresh;
        // the build is fast (~133ms, proven run 130619) and the teardown-spare hook keeps the kept gx
        // alive across Continue. NO hold -- the proven Continue commit proceeds unchanged; if the build
        // loses the race the capture simply never fires (degrades to current behavior, no crash).
        if force_profile_render_enabled() {
            unsafe { force_profile_render_tick(module_base, slot) };
        }
        // PORTRAIT RENDER WINDOW (bounded, fail-open): the main menu is OPEN (a40=1) -> valid menu
        // render context, and the load is NOT yet committed (the commit is product_continue_autoload_tick
        // below -- our own code on a later tick). Kick the async character-model build once (refresh
        // 0x9aa680, idempotent per-slot via +0x754), then HOLD our commit until the portrait has
        // rendered + been captured (maybe_capture_portrait_gxtexture sets LOADING_BG_PORTRAIT_GX_KEPT)
        // or a timeout, so the now-loading screen shows the real character portrait. Fail-open: after
        // the cap we commit regardless, so the char-load can never be permanently blocked.
        if portrait_render_window_enabled()
            && PORTRAIT_RENDER_WINDOW_DONE.load(Ordering::SeqCst) == 0
        {
            if PROFILE_REFRESH_KICKED.swap(1, Ordering::SeqCst) == 0 {
                let refresh: unsafe extern "system" fn() =
                    unsafe { std::mem::transmute(module_base + PROFILE_RENDERER_REFRESH_RVA) };
                unsafe { refresh() };
                append_autoload_debug(format_args!(
                    "portrait-window: kicked profile refresh 0x{:x} to request the model render (menu open)",
                    module_base + PROFILE_RENDERER_REFRESH_RVA
                ));
            }
            let captured = LOADING_BG_PORTRAIT_GX_KEPT.load(Ordering::SeqCst) != 0;
            let waited = PORTRAIT_HOLD_WAIT_TICKS.fetch_add(1, Ordering::SeqCst) + 1;
            if !captured && waited < PORTRAIT_HOLD_MAX_TICKS {
                if waited % 30 == 1 {
                    append_autoload_debug(format_args!(
                        "portrait-window: holding load-commit for portrait render (captured={captured} waited={waited}/{PORTRAIT_HOLD_MAX_TICKS})"
                    ));
                }
                return true;
            }
            PORTRAIT_RENDER_WINDOW_DONE.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "portrait-window: release -> commit load (captured={captured} waited={waited})"
            ));
        }
        if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
            >= SYSTEM_QUIT_QUICKLOAD_PHASE_TITLE_OWNER_SEEN
            && gm != null
            && slot >= OWN_STEPPER_SLOT_ZERO
        {
            unsafe { *((gm + GAME_MAN_REQUESTED_SLOT_B78_OFFSET) as *mut i32) = slot };
            let requested_slot = unsafe { safe_read_i32(gm + GAME_MAN_REQUESTED_SLOT_B78_OFFSET) }
                .unwrap_or(OWN_STEPPER_SLOT_NONE);
            if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
                append_autoload_debug(format_args!(
                    "system-quit-quickload: set requested save-slot load index before Continue selected_slot={slot} gm_b78={requested_slot}"
                ));
            }
        }
        // SWITCH-SAFETY: for the in-world System->Quit->Load-Profile switch, do NOT drive ANY native
        // Continue/menu-readiness probing or the autoload tick until the OLD world is actually torn
        // down (local player absent). Those calls poke native menu/Scaleform functions from the game
        // task; running them while the old world + menu pump are live races the pump and corrupts
        // Scaleform (non-deterministic execute-fault). The menu-pump-owned chain (native confirm
        // Success pops ProfileSelect -> Run-hook submits the return-title chain -> world teardown)
        // must complete first; once the player goes absent this drives the load at a CLEAN title,
        // exactly like the boot autoload. Boot has no System-Quit phase, and at a fresh title there is
        // no local player, so this passes immediately there. See bd
        // system-quit-return-title-scaleform-race-2026-07-01.
        if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst) != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE
            && unsafe { PlayerIns::local_player_mut() }.is_ok()
        {
            if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: SWITCH holding native Continue driving until old world torn down -- local player still present slot={slot} tick={tick}"
                ));
            }
            return true;
        }
        if !unsafe { product_continue_action_ready(&ready, module_base, gm, slot) } {
            if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: waiting for native Continue action readiness owner=0x{owner:x} state={}/{} dialog=0x{:x} menu_latch={} press_start_proxy=0x{:x} slot={slot} -- no direct_build/input fallback",
                    ready.committed,
                    ready.requested,
                    ready.title_dialog,
                    ready.menu_opened_latch,
                    ready.press_start_proxy
                ));
            }
            return true;
        }
        if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
            >= SYSTEM_QUIT_QUICKLOAD_PHASE_TITLE_OWNER_SEEN
        {
            SYSTEM_QUIT_QUICKLOAD_PHASE.store(
                SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF,
                Ordering::SeqCst,
            );
            SYSTEM_QUIT_QUICKLOAD_AUTOLOAD_HANDOFF_COUNT.fetch_add(1, Ordering::SeqCst);
            disable_system_quit_gaitem_deserialize_hook("native-continue-handoff");
            disable_system_quit_gaitem_lookup_hook("native-continue-handoff");
            disable_system_quit_gaitem_finalize_hook("native-continue-handoff");
        }
        unsafe { product_continue_autoload_tick(owner, module_base, gm, slot, tick, &ready) };
    }
    let phase_now = OWN_STEPPER_PHASE.load(Ordering::SeqCst);
    if phase_now == OWN_STEPPER_PHASE_S2_INVOKE
        || phase_now == OWN_STEPPER_PHASE_S2_ACTIVATE
        || phase_now == OWN_STEPPER_PHASE_S2_MOUNT_POLL
        || phase_now == OWN_STEPPER_PHASE_S2_CONFIRM
    {
        unsafe { own_stepper_stage2(owner, module_base, gm, slot, tick, null) };
    }
    true
}
