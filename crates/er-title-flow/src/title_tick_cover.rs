/// Per-frame PUMP for the built LoadGame job (bd drain-dialog-plus8-not-menujob-pump-our-job-directly).
/// Runs from the recurring game task once `maybe_fire_tfc_continue` armed `TFC_DRAIN_JOB`. Calls
/// `ExecuteMenuJob(rcx = &job_slot, rdx = &FD4Time)` DIRECTLY on our built job -- it invokes the job's
/// own `vtable[2]` (the LoadGame chain's Execute), advancing deser/world-stream, and zeroes the slot
/// when done (`ShouldContinue==false`). We pump OUR job (not the dialog's `+0x8` slot, which is not a
/// MenuJob and AV'd the queue-drain wrapper). Pure native call (no input). Stops on completion (slot
/// cleared), in-world, panic, or the tick cap. Every call is `catch_unwind`-guarded.
pub unsafe fn tfc_continue_drain_tick(base: usize, frame_delta: f32) {
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
    if ticks == 1 || ticks.is_multiple_of(OWN_LOAD_STREAM_LOG_INTERVAL as usize) {
        append_autoload_debug(format_args!(
            "tfc-drain: tick {ticks} ExecuteMenuJob(job=0x{job:x}) delta={frame_delta} (pumping)"
        ));
    }
}
/// The D-pad Down button mask to inject for poll-frame `n` (counted from the first poll after
/// menu-open), per the INJECT_NAV schedule: settle, then `INJECT_NAV_MAX_CYCLES` tap+gap cycles
/// with Down asserted for the first `INJECT_NAV_TAP_LEN` frames of each cycle. Returns 0 (no
/// input) during settle, gaps, and after the cycles complete.
pub fn inject_nav_buttons(n: usize) -> u16 {
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
pub fn auto_confirm_tap() {
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
pub unsafe fn title_press_button_component_ready(
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
pub unsafe fn title_dialog_state(dialog: usize, base: usize) -> TitleDialogState {
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
pub unsafe fn title_boot_ready(owner: usize, base: usize) -> bool {
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
pub unsafe fn title_scheduler_ready(owner: usize, base: usize) -> bool {
    unsafe { title_boot_ready(owner, base) }
}
pub unsafe fn product_core_autoload_ready(
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

pub unsafe fn maybe_hide_title_press_start(base: usize, ready: &ProductCoreAutoloadReady) {
    unsafe {
        hide_title_press_start_proxy(
            base,
            ready.title_dialog,
            ready.press_start_proxy,
            ready.press_start_context,
        )
    };
}

pub unsafe fn maybe_hide_title_logo_surface(base: usize, ready: &ProductCoreAutoloadReady) {
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

pub unsafe fn sample_title_profile_portrait_source(base: usize, slot: i32) -> bool {
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


pub unsafe fn maybe_refresh_title_profile_cover(
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

pub unsafe fn product_core_autoload_tick(module_base: usize, slot: i32, tick: u64) -> bool {
    if !product_autoload_enabled() {
        return false;
    }
    PRODUCT_CORE_AUTOLOAD_TICKS.fetch_add(1, Ordering::SeqCst);
    let phase = OWN_STEPPER_PHASE.load(Ordering::SeqCst);
    PRODUCT_CORE_LAST_PHASE.store(phase, Ordering::SeqCst);
    // NOTE: the stats-panel neutral-bg register is NOT called here -- this product-core tick only runs
    // on the `direct_menu_load` path (product_autoload_armed), whereas the product `save_requested`
    // autoload never enters it. The register lives on the always-running FrameBegin game task in
    // `spawn_recurring_effects_task` (src/lib.rs) so it fires on every autoload path.
    if phase == OWN_STEPPER_PHASE_DONE {
        return true;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // IN-WORLD FINALIZE-DRIVE RECOVERY (bd rt5d-drive-blocked-by-title-owner-gate-early-return-inworld-
    // 2026-07-20). The product-core tick EARLY-RETURNS at the title_owner gate just below, and
    // title_owner() is None during stable in-world -- so the rt5d recovery further down NEVER runs for
    // load2's in-world frozen mms18. Resolve MoveMapStep via the CACHED owner here (write_oracle.rs path),
    // BEFORE that gate, and drive menuData+0x5d=1 at the exact frozen-finalize signature so load2 walks
    // 18->19->20 the SAME non-warp way load1 does (load1 proven: rt5d/end5e=1, warp=0, run 1042). Purely
    // ADDITIVE (does not alter the existing flow); tightly gated on active-switch + requestCode==1 +
    // mms_state(+0x48)==18 + finalize(+0x12a)==0 + cVar10 inputs (0x5d/0x5e)==0 after a sustained streak,
    // so a healthy load never trips it; clears 0x5d the frame mms leaves 18 (avoids the ~4s bounce).
    {
        let active_switch = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
            >= SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED;
        let mut cowner = TITLE_OWNER_PTR.load(Ordering::SeqCst);
        if cowner == null {
            cowner = TITLE_SETSTATE_TRACE_LAST_OWNER.load(Ordering::SeqCst);
        }
        let cingame = if cowner != null {
            unsafe { safe_read_usize(cowner + TITLE_STEP_IN_GAME_STEP_2E8_OFFSET) }
                .filter(|&v| v > 0x10000)
        } else {
            None
        };
        let creq = cingame
            .and_then(|ig| unsafe { safe_read_i32(ig + IN_GAME_STEP_REQUEST_CODE_D8_OFFSET) })
            .unwrap_or(-1);
        // Prefer write_oracle's reliably-resolved MoveMapStep pointer: the cached-owner walk here reads a
        // STALE step for load2 (proven -- it saw 13-16, not the true 18). Fall back to the local walk only
        // if the oracle has not published a pointer yet this session.
        let reliable_mms = ORACLE_RELIABLE_MMS_PTR.load(Ordering::SeqCst);
        let cmms = if reliable_mms > 0x10000 {
            Some(reliable_mms)
        } else {
            cingame
                .and_then(|ig| unsafe { safe_read_usize(ig + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) })
                .filter(|&v| v > 0x10000)
        };
        let cstate = cmms
            .and_then(|m| unsafe { safe_read_i32(m + INGAMESTEP_STEP_STATE_OFFSET) })
            .unwrap_or(-1);
        let cfin = cmms
            .and_then(|m| unsafe { safe_read_u8(m + MOVEMAPSTEP_FINALIZE_SUBSTATE_12A_OFFSET) })
            .unwrap_or(0xff);
        let cmenu = unsafe { safe_read_usize(module_base + CS_MENU_MAN_GLOBAL_RVA) }
            .filter(|&m| m > 0x10000)
            .and_then(|m| unsafe { safe_read_usize(m + CS_MENU_MAN_MENU_DATA_OFFSET) })
            .filter(|&d| d > 0x10000);
        let c5d = cmenu
            .and_then(|d| unsafe { safe_read_u8(d + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET) })
            .unwrap_or(0xff);
        let c5e = cmenu
            .and_then(|d| unsafe { safe_read_u8(d + CS_MENU_DATA_ENDING_FLAG_5E_OFFSET) })
            .unwrap_or(0xff);
        // Gate on the RELIABLE mms_state/finalize (creq dropped from the gate -- it comes from the
        // possibly-stale cached-owner ingame; kept in the log only). mms_state==18 && finalize==0 with
        // cVar10 inputs (0x5d/0x5e) both 0 is load2's exact frozen-finalize signature.
        // FINALIZE-FORCING was disabled 2026-07-21 because it was shoving mms 18->19->20 on the INCOMING
        // load (diverging from vanilla). PHASE-3 (bd PHASE3-render-release-is-CommonFinalize, 2026-07-23)
        // RE-ENABLES it but SCOPED to the OUTGOING switch window ONLY (phase RETURN_TITLE_REQUESTED..
        // AUTOLOAD_HANDOFF). This block runs BEFORE the title_owner gate, i.e. while the OUTGOING world is
        // still the live in-world InGameStep, so driving menuData+0x5d=1 here walks the OUTGOING MoveMapStep
        // 18->Cleanup(19)->Finish(20)->_Common_Finalize -- releasing the pre-quit world's render globals
        // BEFORE own_load rebuilds fresh. It is NOT applied at AUTOLOAD_HANDOFF (the incoming/vanilla load),
        // honoring the 2026-07-21 directive. `active_switch` (phase >= RETURN_TITLE_REQUESTED) is the
        // switch-reload gate: on the boot autoload (LOAD1, phase IDLE) this drive never fires. OPT-IN via
        // er-effects-enable-outgoing-teardown.txt (default-off; reverted after run angre-phase3fix-1).
        let finalize_forcing_enabled = crate::compat::gating::outgoing_teardown_enabled()
            && active_switch
            && SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
                < SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF;
        let frozen_mms18 = finalize_forcing_enabled
            && cstate == MOVEMAPSTEP_STEP_MOVEMAP_INDEX
            && cfin == 0
            && c5d == 0
            && c5e == 0;
        if frozen_mms18 {
            if let Some(d) = cmenu {
                // Fire after ~2s of a HELD frozen signature (40 frames; load2 froze 26s in prior runs,
                // and a healthy load leaves 18 / walks cfin within a few frames, so this can't trip on a
                // transient). Short so the RAM-gated drive completes before an incidental unfocused-mouse
                // click can contaminate the run (bd er-accepts-unfocused-mouse-input-contaminates-runs).
                let streak = INWORLD_FINALIZE_DRIVE_STREAK.fetch_add(1, Ordering::SeqCst) + 1;
                if streak >= INWORLD_FINALIZE_DRIVE_RELEASE_FRAMES
                    && INWORLD_FINALIZE_DRIVE_SET
                        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    unsafe {
                        *((d + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET) as *mut u8) = 1;
                    }
                    let n = INWORLD_FINALIZE_DRIVE_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                    append_autoload_debug(format_args!(
                        "IN-WORLD FINALIZE DRIVE #{n}: drove menuData+0x5d=1 at frozen in-world mms18 (streak={streak} creq={creq} cstate={cstate} cfin={cfin} c5d=0 c5e=0) -- cached-owner path past the title_owner gate; non-warp finalize driver (load1 path)"
                    ));
                }
            }
        } else {
            // WHY-NOT: load2 at mms18 but the frozen signature was not met -> name which field blocks so
            // a run is conclusive even if the drive never fires (throttled). Only at cstate==18.
            if cstate == MOVEMAPSTEP_STEP_MOVEMAP_INDEX {
                let w = INWORLD_FINALIZE_DRIVE_WHYNOT_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                if w <= 20 || w.is_multiple_of(20) {
                    append_autoload_debug(format_args!(
                        "IN-WORLD FINALIZE DRIVE WHY-NOT #{w}: frozen_mms18=false at cstate=18 -- active_switch={active_switch}(phase={}) creq={creq} cfin={cfin} c5d={c5d} c5e={c5e} cmenu={} (needs active_switch && creq==1 && cfin==0 && c5d==0 && c5e==0)",
                        SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst),
                        cmenu.is_some()
                    ));
                }
            }
            INWORLD_FINALIZE_DRIVE_STREAK.store(0, Ordering::SeqCst);
            if INWORLD_FINALIZE_DRIVE_SET.load(Ordering::SeqCst) == 1
                && cstate != MOVEMAPSTEP_STEP_MOVEMAP_INDEX
            {
                INWORLD_FINALIZE_DRIVE_SET.store(0, Ordering::SeqCst);
                if let Some(d) = cmenu {
                    unsafe {
                        *((d + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET) as *mut u8) = 0;
                    }
                }
                let n = INWORLD_FINALIZE_DRIVE_COUNT.load(Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "IN-WORLD FINALIZE DRIVE: mms left step 18 (cstate={cstate}) after {n} drive(s); cleared menuData+0x5d"
                ));
            }
        }
    }
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
    // TITLE_STEP_END_FLOW (7) / TITLE_STEP_END_FLOW_WAIT (8) are the enum-backed teardown-state
    // constants (constants::stats_panel_background); the parent-fix below forces them to GameStepWait(6).
    if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        == SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF
        && SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst) > 0
    {
        let committed =
            unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }.unwrap_or(-1);
        let requested = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_OFFSET) }.unwrap_or(-1);
        if matches!(committed, TITLE_STEP_END_FLOW | TITLE_STEP_END_FLOW_WAIT)
            || matches!(requested, TITLE_STEP_END_FLOW | TITLE_STEP_END_FLOW_WAIT)
        {
            unsafe {
                *((owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *mut i32) =
                    TITLE_STEP_GAME_STEP_WAIT;
                *((owner + TITLE_OWNER_STATE_OFFSET) as *mut i32) = TITLE_STEP_GAME_STEP_WAIT;
            }
            append_autoload_debug(format_args!(
                "AUTOLOAD-HANDOFF PRODUCT-CORE PARENT FIX: TitleStep {committed}/{requested} -> GameStepWait(6) for reload epoch {}; prevents EndFlow/EndFlowWait returning the loaded world to title",
                SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst)
            ));
        }
    }
    let gm = game_man_ptr_or_null();
    let mut return_title_job_predicate_bc4 = if gm != null {
        unsafe { safe_read_usize(gm + GAME_MAN_RETURN_TITLE_JOB_PREDICATE_BC4_OFFSET) }
            .map(|v| v as u32 as usize)
            .unwrap_or(usize::MAX)
    } else {
        usize::MAX
    };
    PRODUCT_CORE_LAST_RETURN_TITLE_JOB_PREDICATE_BC4
        .store(return_title_job_predicate_bc4, Ordering::SeqCst);
    // (Deferred return-title-context clear experiment REMOVED 2026-07-16: it was inert -- the stuck load's
    // menuData+0x5d is 0 to begin with, the functor never set it. The real root is the incomplete teardown
    // functor, not our clear. See bd ending-request-fix-was-wrong / rt5d-never-set findings.)
    // SWITCH-OUTCOME ORACLE (read-only, user-mandated reliable semaphore). Runs whenever a slot is picked,
    // OUTSIDE the dormancy gate below, so it observes the post-commit native session too. Literals: CSMenuMan
    // global +0x3d6b7b0, in-game menu job +0x798; InGameStep = TitleStep(owner)+0x2e8, requestCode +0xd8.
    // The trace classifies the outcome with no eyeballs: stable_frames (player present + requestCode==2 +
    // menu_job!=0) climbing high = LOADED_STABLE; resetting after a peak = the world DROPPED (bounce/reload);
    // the line STOPPING = FROZE; bc4 stuck at 1 = never-tears-down freeze.
    if slot >= OWN_STEPPER_SLOT_ZERO && gm != null {
        let player_present = unsafe { PlayerIns::local_player_mut() }.is_ok();
        let ig_d8 = if owner != null {
            unsafe { safe_read_usize(owner + TITLE_STEP_IN_GAME_STEP_2E8_OFFSET) }
                .filter(|&ig| ig > 0x10000)
                .and_then(|ig| unsafe {
                    safe_read_i32(ig + IN_GAME_STEP_REQUEST_CODE_D8_OFFSET)
                })
                .unwrap_or(-1)
        } else {
            -1
        };
        let menu_man = unsafe { safe_read_usize(module_base + 0x3d6b7b0) }.filter(|&m| m > 0x10000);
        let menu_job = menu_man
            .and_then(|m| unsafe { safe_read_usize(m + 0x798) })
            .unwrap_or(0);
        let loading_screen_field10 = menu_man
            .and_then(|m| unsafe { safe_read_u8(m + 0x730) })
            .map(|v| v as i32)
            .unwrap_or(-1);
        let loading_screen_field11 = menu_man
            .and_then(|m| unsafe { safe_read_u8(m + 0x731) })
            .map(|v| v as i32)
            .unwrap_or(-1);
        // MoveMapStep CHILD state (3rd-load root, 2026-07-16 Ghidra). InGameStep step 7
        // STEP_MoveMap_Update loops until the MoveMapStep child's own step machine FINISHES (gate
        // FUN_140eb5550); only then does requestCode(ig_d8) go 1->2. On the softlock the child is
        // created (step 6 STEP_MoveMap_Init ran) but never finishes, so ig_d8 stays 1 and step 7
        // self-loops. Read the child's internal step + world-res streaming state so the true stuck
        // point is a RAM semaphore, not the eyeballed "stuck at LOADING SAVE" bar. All reads are
        // safe_read (null/garbage -> None) on the game-thread tick; identical chain to submit.rs.
        let ig_ptr = if owner != null {
            unsafe { safe_read_usize(owner + TITLE_STEP_IN_GAME_STEP_2E8_OFFSET) }
                .filter(|&v| v > 0x10000)
        } else {
            None
        };
        let mms = ig_ptr
            .and_then(|ig| unsafe { safe_read_usize(ig + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) })
            .filter(|&v| v > 0x10000);
        // LOADLIST ROOT LEAD: the world-res loadlist is built by STEP_MoveMap_LoadlistInit only when
        // worldloadlistlistVirtualPath.size (InGameStep+0x220) != 0, storing the cap at +0x238. At the
        // step-3 stall, ll_size==0 + ll_fcap==0 == "loadlist never built for the target area".
        let ll_size = ig_ptr
            .and_then(|ig| unsafe {
                safe_read_usize(ig + INGAMESTEP_WORLDLOADLIST_VPATH_SIZE_220_OFFSET)
            })
            .unwrap_or(usize::MAX);
        let ll_fcap = ig_ptr
            .and_then(|ig| unsafe {
                safe_read_usize(ig + INGAMESTEP_LOADLISTLIST_FILECAP_238_OFFSET)
            })
            .unwrap_or(0);
        // The loadlist virtual path CONTENT: is it the TARGET map (m28) or a STALE map (m60)? DLString
        // wchar; size 35 > 7 so it is heap (data = *(base+0x210)). Read the ASCII low byte of each wchar
        // (map paths are ASCII) into a string -- reveals the map id in the path (e.g. .../m28.. vs /m60..).
        let ll_path = if ll_size > 0 && ll_size < 200 {
            ig_ptr
                .and_then(|ig| unsafe {
                    safe_read_usize(ig + INGAMESTEP_WORLDLOADLIST_VPATH_BASE_210_OFFSET)
                })
                .filter(|&v| v > 0x10000)
                .map(|heap| {
                    let mut s = String::new();
                    for i in 0..ll_size.min(72) {
                        let b = unsafe { safe_read_u8(heap + i * 2) }.unwrap_or(0);
                        if b == 0 {
                            break;
                        }
                        s.push(if (0x20..0x7f).contains(&b) {
                            b as char
                        } else {
                            '?'
                        });
                    }
                    s
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
        let mms_step = mms
            .and_then(|m| unsafe { safe_read_i32(m + INGAMESTEP_STEP_STATE_OFFSET) })
            .unwrap_or(-1);
        let mms_wrm = mms
            .and_then(|m| unsafe { safe_read_usize(m + MOVEMAPSTEP_WORLDRES_F0_OFFSET) })
            .filter(|&v| v > 0x10000);
        let mms_resmgr = mms_wrm
            .and_then(|w| unsafe { safe_read_usize(w + WORLDRES_RESMGR_10_OFFSET) })
            .filter(|&v| v > 0x10000);
        // DEST-BLOCK REGISTER (RE wf_3f1e7d9a): worldres+0xb798 = current dest BlockId the WorldResWait
        // drives to, +0xb79c = previous. ResetAreaResLists sets b798=-1 each load; FUN_14066da30 pushes
        // the target on a "changed" transition. If on the reload b798 stays -1/stale (not 0x1c000000)
        // while it is 0x1c000000 on the first load, the dest was never seeded -> area never activates
        // -> WORLD RES WAIT. Read-only.
        let (mms_b798, mms_b79c) = if let Some(wio) = mms_resmgr {
            (
                unsafe { safe_read_i32(wio + 0xb798) }.unwrap_or(-2),
                unsafe { safe_read_i32(wio + 0xb79c) }.unwrap_or(-2),
            )
        } else {
            (-2, -2)
        };
        let mms_b7c1 = mms_resmgr
            .and_then(|r| unsafe { safe_read_u8(r + RESMGR_STREAM_ENABLE_B7C1_OFFSET) })
            .map(|b| b as i32)
            .unwrap_or(-1);
        let mms_blocks = mms_resmgr
            .and_then(|r| unsafe { safe_read_i32(r + RESMGR_BLOCK_COUNT_B3140_OFFSET) })
            .unwrap_or(-1);
        // STEP-3 (WORLD RES WAIT) DETERMINANT: the BlockId step 3 waits on (FieldArea+0x2c) and whether
        // its areaId is present in the world's loaded-block list. `mms_wrm` = FieldArea, `mms_resmgr` =
        // worldInfoOwner. Only scan the list at step 3 (bounded cost). `mms_cur_block` = the target
        // BlockId; `mms_block_found` = is that block's areaId among the loaded blocks (1 yes / 0 no /
        // -1 not scanned); `mms_block_areas` = the loaded blocks' areaIds (first few) for comparison.
        // The BlockId areaId is one byte of the u32; we match against every byte to stay byte-order-safe.
        let mms_cur_block = mms_wrm
            .and_then(|fa| unsafe { safe_read_i32(fa + FIELDAREA_CURRENT_BLOCK_ID_2C_OFFSET) })
            .map(|v| v as u32)
            .unwrap_or(u32::MAX);
        let (
            mms_block_found,
            mms_block_areas,
            mms_blk_2c,
            mms_blk_2d,
            mms_blk_35,
            mms_blk_ls,
            mms_blk_vt,
            mms_ar_wanted,
            mms_ar_state,
            mms_ar_bres,
            mms_ar_ptr,
            mms_blk_phase2,
        ) = if (MOVEMAPSTEP_STEP_WORLDRESWAIT_INDEX..=8).contains(&mms_step)
            && mms_cur_block != u32::MAX
            && mms_blocks > 0
            && mms_blocks < 256
        {
            let wio = mms_resmgr.unwrap_or(null);
            // areaId is BlockId byte[3] (disasm FUN_14066d4d0: `MOVZX R11D, byte ptr [RDX + 0x3]`,
            // then `CMP dword ptr [inner + 0xc], R11D`). So the block the step waits on is the one whose
            // inner+0xc == (curblk >> 24) & 0xff. Match EXACTLY that (the earlier byte-set match false-
            // positived on the 0x00 bytes of curblk).
            let cur_area = (mms_cur_block >> 24) & 0xff;
            let mut found = false;
            // The matched block's load-state: the getter return `ls_ptr` (0 = NULL load-state, i.e. the
            // block's load was never kicked off = FUN_14066d4d0's `if(loadstate==0) return 0` bail), and
            // when non-null its bytes +0x2c (load-request flag, set by FUN_14066d8d0), +0x2d (ready), and
            // +0x35 (stream phase; == 10 means loaded). -1 = not read.
            let mut ls_ptr: usize = 0;
            let mut ls_vt: usize = 0;
            let mut ls_2c: i32 = -1;
            let mut ls_2d: i32 = -1;
            let mut ls_35: i32 = -1;
            // Rendered once at the end rather than threaded out as seven more tuple slots.
            let mut ls_phase2 = String::new();
            let mut areas = String::new();
            // WorldAreaRes ACTIVATION state (RE wf_3f1e7d9a): the matched +0xb3030 entry IS the area's
            // WorldAreaRes. +0x1a = "area wanted" flag, +0x1c = activation state machine (0..7; ==6 is
            // the state that permits the block load to start), +0xcd8 = the area's WorldBlockRes count.
            // If on the reload the area's state never reaches 6 (not wanted), the block load never kicks.
            let mut ar_wanted: i32 = -1;
            let mut ar_state: i32 = -1;
            let mut ar_bres: i32 = -1;
            let mut ar_ptr: usize = 0;
            for i in 0..(mms_blocks as usize) {
                let Some(bp) =
                    (unsafe { safe_read_usize(wio + WORLDINFO_BLOCK_LIST_B3030_OFFSET + i * 8) })
                        .filter(|&v| v > 0x10000)
                else {
                    continue;
                };
                let Some(inner) =
                    (unsafe { safe_read_usize(bp + WORLDINFO_BLOCK_ENTRY_INNER_8_OFFSET) })
                        .filter(|&v| v > 0x10000)
                else {
                    continue;
                };
                let area = unsafe { safe_read_i32(inner + WORLDINFO_BLOCK_AREA_ID_C_OFFSET) }
                    .map(|v| v as u32)
                    .unwrap_or(u32::MAX);
                if area == cur_area && !found {
                    found = true;
                    // The matched entry `bp` is area cur_area's WorldAreaRes: read its activation state.
                    ar_wanted = unsafe { safe_read_u8(bp + 0x1a) }
                        .map(|v| v as i32)
                        .unwrap_or(-1);
                    ar_state = unsafe { safe_read_i32(bp + 0x1c) }.unwrap_or(-1);
                    ar_bres = unsafe { safe_read_i32(bp + 0xcd8) }.unwrap_or(-1);
                    ar_ptr = bp;
                    // STALE +0xce0 DUMP (run-validated root probe 2026-07-17): the getter
                    // (deobf 0x14062f470) searches WorldAreaRes+0xce0[i] (stride 0xb98), reads
                    // *(entry+0x8)=worldBlockInfo, +0x34=mapId2, matching the requested BlockId
                    // (0x1c000000). ar_bres=1 yet the getter returns null => the resident entry's
                    // mapId2 != 0x1c000000, i.e. a STALE block-res left over from the prior in-world
                    // load and never reset on the switch. Dump each entry's mapId2 to identify the
                    // stale block. Change-detected (base ^ first ^ count) so a steady stall logs once.
                    if ar_bres > 0 && ar_bres <= 64
                        && let Some(ce0_base) =
                            unsafe { safe_read_usize(bp + 0xce0) }.filter(|&v| v > 0x10000)
                        {
                            let mut mapids = String::new();
                            let mut first: u32 = u32::MAX;
                            for k in 0..ar_bres {
                                let entry = ce0_base + (k as usize) * 0xb98;
                                let wbi = unsafe { safe_read_usize(entry + 0x8) }.unwrap_or(0);
                                let m = if wbi > 0x10000 {
                                    unsafe { safe_read_i32(wbi + 0x34) }
                                        .map(|v| v as u32)
                                        .unwrap_or(u32::MAX)
                                } else {
                                    u32::MAX
                                };
                                if k == 0 {
                                    first = m;
                                }
                                let _ = core::fmt::Write::write_fmt(
                                    &mut mapids,
                                    format_args!("{m:#x},"),
                                );
                            }
                            static LAST_CE0_SIG: core::sync::atomic::AtomicUsize =
                                core::sync::atomic::AtomicUsize::new(0);
                            let sig = ce0_base ^ ((first as usize) << 8) ^ (ar_bres as usize);
                            if LAST_CE0_SIG.swap(sig, core::sync::atomic::Ordering::SeqCst) != sig {
                                append_autoload_debug(format_args!(
                                    "STALE-CE0 dump: area=0x{cur_area:x} bp=0x{bp:x} ce0_base=0x{ce0_base:x} cnt={ar_bres} entry_mapids=[{mapids}] -- getter wants 0x{cur_area:x}000000 (blk_ls null => none match)"
                                ));
                            }
                        }
                    // Read the load-state exactly like FUN_14066d4d0: block->vtable[0x10](block) returns
                    // the load-state object; then +0x2d / +0x35. This is the same getter the game polls
                    // on this same block every frame, so it is safe. RCX = block (Windows x64 ABI).
                    if let Some(vt) = unsafe { safe_read_usize(bp) }.filter(|&v| v > 0x10000) {
                        ls_vt = vt;
                        if let Some(getter) =
                            unsafe { safe_read_usize(vt + BLOCK_LOADSTATE_GETTER_VT_10_OFFSET) }
                                .filter(|&v| v > 0x10000)
                        {
                            // 2-ARG call, matching the game's check FUN_14066d4d0 (deobf
                            // 0x14066d3e0): vtable[0x10](rcx=WorldAreaRes, rdx=&blockId) returns the
                            // LOADSTATE for that block; the earlier 1-arg call passed garbage in rdx
                            // and got null (the blk_ls=0 red herring). blockId = area<<24 (0x1c000000
                            // for the legacy angrE block, byte[3]==area as the check reads *(blockId+3)).
                            let get_ls: unsafe extern "system" fn(usize, *const u32) -> usize =
                                unsafe { core::mem::transmute(getter) };
                            let block_id: u32 = (cur_area as u32) << 24;
                            let ls = unsafe { get_ls(bp, &block_id as *const u32) };
                            ls_ptr = ls;
                            if ls > 0x10000 {
                                ls_2c =
                                    unsafe { safe_read_u8(ls + BLOCK_LOADSTATE_REQUEST_2C_OFFSET) }
                                        .map(|v| v as i32)
                                        .unwrap_or(-1);
                                ls_2d =
                                    unsafe { safe_read_u8(ls + BLOCK_LOADSTATE_FLAG_2D_OFFSET) }
                                        .map(|v| v as i32)
                                        .unwrap_or(-1);
                                ls_35 =
                                    unsafe { safe_read_u8(ls + BLOCK_LOADSTATE_PHASE_35_OFFSET) }
                                        .map(|v| v as i32)
                                        .unwrap_or(-1);
                                // PHASE-2 STALL DISCRIMINATORS. `fc_*` above scans the caps on the
                                // WorldAreaRes and reported all-loaded while this block sat at phase
                                // 2, so the caps phase 2 actually waits on may be the BLOCK's own.
                                // Sample both those and the `[+0x2f]` gate that decides whether phase
                                // 2 polls at all. Read-only; no call, no write.
                                let ls_2f = unsafe {
                                    safe_read_u8(ls + BLOCK_LOADSTATE_ASSET_GATE_2F_OFFSET)
                                }
                                .map(|v| v as i32)
                                .unwrap_or(-1);
                                let ls_3c = unsafe {
                                    safe_read_u8(ls + BLOCK_LOADSTATE_COUNTDOWN_3C_OFFSET)
                                }
                                .map(|v| v as i32)
                                .unwrap_or(-1);
                                let ls_06 =
                                    unsafe { safe_read_u8(ls + BLOCK_LOADSTATE_GAVEUP_06_OFFSET) }
                                        .map(|v| v as i32)
                                        .unwrap_or(-1);
                                let mut caps = String::new();
                                for slot in BLOCK_LOADSTATE_FILECAP_SLOTS {
                                    let Some(cap) = (unsafe { safe_read_usize(ls + slot) })
                                        .filter(|&v| v > 0x10000)
                                    else {
                                        let _ = core::fmt::Write::write_fmt(
                                            &mut caps,
                                            format_args!("+{slot:x}:none,"),
                                        );
                                        continue;
                                    };
                                    let st =
                                        unsafe { safe_read_u8(cap + FD4_FILECAP_STATUS_88_OFFSET) }
                                            .map(|v| v as i32)
                                            .unwrap_or(-1);
                                    let bytes = unsafe {
                                        safe_read_usize(cap + FD4_FILECAP_BYTES_90_OFFSET)
                                    }
                                    .unwrap_or(0);
                                    // WHY `msbResCap == 0` -- the two surviving explanations.
                                    // `bytes` is `MsbFileCap::msbResCap`, written at exactly ONE
                                    // site, guarded on the cap's CONTENT being non-null (see the
                                    // constant's docs). So a null here means the parse never ran,
                                    // and the question is whether this cap is (a) FRESH, built by
                                    // this reload, whose file read came back empty, or (b) a
                                    // CACHE-HIT SURVIVOR from the previous load that was never
                                    // reparsed. `rc`/`fl` separate those; `ct` shows whether the
                                    // content the callback gates on is present right now.
                                    let rc = unsafe {
                                        safe_read_usize(cap + FD4_FILECAP_REFCOUNT_58_OFFSET)
                                    }
                                    .map(|v| (v & 0xffff_ffff) as i64)
                                    .unwrap_or(-1);
                                    let fl = unsafe { safe_read_u8(cap + FD4_FILECAP_FLAGS_89_OFFSET) }
                                        .map(|v| v as i32)
                                        .unwrap_or(-1);
                                    let lp = unsafe {
                                        safe_read_usize(cap + FD4_FILECAP_LOADPROCESS_78_OFFSET)
                                    }
                                    .unwrap_or(0);
                                    let (proc_ptr, content, csize, acq) = unsafe {
                                        fd4_filecap_content_state(lp)
                                    };
                                    let name = unsafe { fd4_filecap_name(cap) };
                                    let _ = core::fmt::Write::write_fmt(
                                        &mut caps,
                                        format_args!(
                                            "+{slot:x}:st={st}/bytes={bytes:#x}/rc={rc}/fl={fl:#x}\
                                             /lp={lp:#x}/proc={proc_ptr:#x}/ct={content:#x}\
                                             /csz={csize:#x}/acq={acq}/name='{name}',"
                                        ),
                                    );
                                }
                                // The user reports the NATIVE loading screen owns the screen at the
                                // softlock, where a healthy load only flashes it for about a vblank.
                                // Pin that to RAM rather than leaving it a visual observation: sample
                                // both loading-screen surfaces at the stall so the report becomes a
                                // semaphore.
                                let module_base = game_module_base().ok().filter(|&b| b != 0);
                                let (now_loading, fake_cover) = match module_base {
                                    Some(b) => (
                                        unsafe { now_loading_active(b) } as i32,
                                        unsafe { fake_loading_screen_visible(b) } as i32,
                                    ),
                                    None => (-1, -1),
                                };
                                // The caps above name `mapstudio_dlc2:/m28_*.msb`, so dump the DLIO
                                // alias that prefix resolves through. An alias present but EMPTY is
                                // a 0-byte read by construction, which is the null `msbResCap`.
                                let roots = match module_base {
                                    Some(b) => unsafe { dlio_virtual_roots_summary(b) },
                                    None => String::from("<nobase>"),
                                };
                                let _ = core::fmt::Write::write_fmt(
                                    &mut ls_phase2,
                                    format_args!(
                                        " blk_2f={ls_2f} blk_3c={ls_3c} blk_06={ls_06}\
                                         blk_nowloading={now_loading} blk_fakecover={fake_cover}\
                                         blk_roots=[{roots}] blk_caps=[{caps}]"
                                    ),
                                );
                            }
                        }
                    }
                }
                if i < 40 {
                    let _ = core::fmt::Write::write_fmt(&mut areas, format_args!("{area:#x},"));
                }
            }
            (
                if found { 1 } else { 0 },
                areas,
                ls_2c,
                ls_2d,
                ls_35,
                ls_ptr,
                ls_vt,
                ar_wanted,
                ar_state,
                ar_bres,
                ar_ptr,
                ls_phase2,
            )
        } else {
            (-1, String::new(), -1, -1, -1, 0, 0, -1, -1, -1, 0, String::new())
        };
        // FILE-CAP READINESS (RE FUN_140613710): the state-2 handler advances 2->3 only when every present
        // FD4FileCap slot on the WorldAreaRes has load-status +0x88 == 0x04. Scan them so the stall shows
        // whether a file cap is stuck (present but not 0x04) -- vs all-loaded (=> the CSEmkResMan gate is
        // what stalls). Read-only, bounded.
        let (mms_fc_present, mms_fc_notloaded, mms_fc_stuck) = {
            const GROUPS: &[(usize, usize)] = &[
                (0x28, 5),
                (0x50, 5),
                (0x78, 1),
                (0x80, 1),
                (0x88, 7),
                (0xc0, 1),
                (0xc8, 1),
                (0xd0, 1),
                (0x470, 1),
                (0x480, 1),
            ];
            if mms_ar_ptr > 0x10000 {
                let mut present = 0i32;
                let mut notloaded = 0i32;
                let mut stuck = String::new();
                for &(base, cnt) in GROUPS {
                    for i in 0..cnt {
                        let slot = base + i * 8;
                        let Some(cap) = (unsafe { safe_read_usize(mms_ar_ptr + slot) })
                            .filter(|&v| v > 0x10000)
                        else {
                            continue;
                        };
                        present += 1;
                        let st = unsafe { safe_read_u8(cap + 0x88) }.unwrap_or(0xff);
                        if st != 0x04 {
                            notloaded += 1;
                            if stuck.len() < 80 {
                                let _ = core::fmt::Write::write_fmt(
                                    &mut stuck,
                                    format_args!("+{slot:x}:{st:#x},"),
                                );
                            }
                        }
                    }
                }
                (present, notloaded, stuck)
            } else {
                (-1, -1, String::new())
            }
        };
        // OVERWORLD-RESIDUAL confirm: the overworld block list (+0xb3148, count +0xb31d0). If the boot
        // char's m60 overworld blocks (area 0x3c) are still resident here while step 3 waits on the
        // incoming legacy block (area 0x1c), the overworld residual is starving the legacy load-request.
        let (mms_ow_count, mms_ow_areas) = if (MOVEMAPSTEP_STEP_WORLDRESWAIT_INDEX..=8)
            .contains(&mms_step)
            && mms_resmgr.is_some()
        {
            let wio = mms_resmgr.unwrap_or(null);
            let cnt = unsafe { safe_read_i32(wio + WORLDINFO_OVERWORLD_COUNT_B31D0_OFFSET) }
                .unwrap_or(-1);
            let mut s = String::new();
            if cnt > 0 && cnt < 256 {
                for i in 0..(cnt as usize).min(24) {
                    let bid = unsafe {
                        safe_read_i32(wio + WORLDINFO_OVERWORLD_LIST_B3148_OFFSET + i * 4)
                    }
                    .map(|v| v as u32)
                    .unwrap_or(u32::MAX);
                    let a = (bid >> 24) & 0xff;
                    let _ = core::fmt::Write::write_fmt(&mut s, format_args!("{a:#x},"));
                }
            }
            (cnt, s)
        } else {
            (-1, String::new())
        };
        // STEP-3 / WORLD-RES FIX (2026-07-17, decompile+runtime grounded). When a switch reaches
        // WORLD RES WAIT (step 3) with the incoming block's load-state NULL (blk_ls=0), the block-res
        // entry was never created because the fast in-process switch skipped STEP_MoveMap_Init's
        // world-res rebuild. Re-run the game's own CS::WorldInfoOwner::ProcessMsbLoadLists(worldInfoOwner,
        // loadlistlistFileCap, dlc02=0) (0x14066b2c0) -- it runs ResetAreaResLists + PopulateLists to
        // CREATE the missing block-res, exactly like _Common_Initialize (which calls it with
        // &worldInfoOwner, loadlistlistFileCap, loadlistlistFileCap_dlc02). dlc02=0 is null-safe
        // (decompile line 214 null-checks it; base-game areas have no dlc02 loadlist). Gate on a
        // SUSTAINED null (>= 2s) so the sub-second boot-load transient (which loads fine) never trips
        // it, and one-shot per DLL load.
        // The null-block-load-state stall persists across WORLD RES WAIT (3) and CURRENT LOD BLOCK (4)
        // -- the switch advances 3->4 but both wait on the same never-created block-res -- so count the
        // streak across steps 3..=8, not step 3 alone (the switch left step 3 before 2s elapsed).
        // Count the streak on the null load-state alone (curblk flickers to 0xffffffff while the
        // world-res is half-set-up, which would keep resetting a curblk-gated streak); the valid-block
        // check stays on the actual fire below. Gate on IN_WORLD_REACHED==YES so this only counts a
        // SWITCH reload (we were already in-world), never the sub-second boot-load transient -- which is
        // what let us drop the threshold to 30 frames (the switch reaches step 3/4 late, after the old
        // world's slow step-18 teardown, so a 2s window did not fit before the run cap).
        if IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES
            && (MOVEMAPSTEP_STEP_WORLDRESWAIT_INDEX..=8).contains(&mms_step)
            && mms_blk_ls == 0
        {
            SWITCH_WORLDRES_NULL_STREAK.fetch_add(1, Ordering::SeqCst);
        } else {
            SWITCH_WORLDRES_NULL_STREAK.store(0, Ordering::SeqCst);
        }
        if SWITCH_WORLDRES_NULL_STREAK.load(Ordering::SeqCst) >= 8
            && ((mms_cur_block >> 24) & 0xff) != 0
            && mms_resmgr.unwrap_or(0) >= 0x10000
            && ll_fcap >= 0x10000
            && SWITCH_WORLDRES_REBUILD_TRIED.swap(1, Ordering::SeqCst) == 0
        {
            let owner = mms_resmgr.unwrap_or(0);
            SWITCH_WORLDRES_REBUILD_COUNT.fetch_add(1, Ordering::SeqCst);
            // DETECT-ONLY. Calling CS::WorldInfoOwner::ProcessMsbLoadLists reactively HERE (at WORLD RES
            // WAIT / step 3-4) ACCESS-VIOLATES -- runtime-proven 2026-07-17: the PRE-CALL log flushed,
            // the game died before the call returned. ProcessMsbLoadLists runs ResetAreaResLists +
            // PopulateLists, which is only safe at STEP_MoveMap_Init (BEFORE the world starts streaming);
            // resetting the area-res lists mid-stream faults. So the reactive rebuild is DISABLED. The
            // correct fix must run at STEP_MoveMap_Init (0x140aec210) / _Common_Initialize (0x140aed910)
            // with the DESTINATION map's loadlist -- i.e. make the fast switch not skip / not run that
            // native init with a stale loadlist -- not a reactive call at the stall. This block now just
            // records the confirmed stall for the next (init-point) fix. See bd
            // step3-reactive-processmsbloadlists-crashes-init-point-fix-needed-2026-07-17.
            if let Ok(addr) = game_rva(WORLDINFO_PROCESS_MSB_LOADLISTS_RVA) {
                let _ = addr;
                let _ = owner;
                append_autoload_debug(format_args!(
                    "STEP-3 STALL DETECTED (reactive ProcessMsbLoadLists disabled -- it AVs mid-stream): owner=0x{owner:x} fcap=0x{ll_fcap:x} area=0x{:x} curblk=0x{:x} streak={} -- fix belongs at STEP_MoveMap_Init, not here",
                    (mms_cur_block >> 24) & 0xff,
                    mms_cur_block,
                    SWITCH_WORLDRES_NULL_STREAK.load(Ordering::SeqCst)
                ));
            }
        }
        // STEP_MoveMap advance gate (MoveMapStep+0x4b8 low / +0x4b9 high). lo=1 -> ready to advance;
        // lo=0 -> blocked; lo=0,hi=1 -> WorldChrMan-not-ready (0x100). Blocked-with-bc4=1 is the softlock.
        let mms_gate_lo = mms
            .and_then(|m| unsafe { safe_read_u8(m + MOVEMAPSTEP_ADVANCE_GATE_LO_4B8_OFFSET) })
            .map(|b| b as i32)
            .unwrap_or(-1);
        let mms_gate_hi = mms
            .and_then(|m| unsafe { safe_read_u8(m + MOVEMAPSTEP_ADVANCE_GATE_HI_4B9_OFFSET) })
            .map(|b| b as i32)
            .unwrap_or(-1);
        // STEP_MoveMap 18->19 transition state (bc4 was blocker #1; this pins blocker #2). next=next step
        // (18 = STEP_MoveMap not requesting advance; 19 = requested but pump not committing). done50 = the
        // FD4 "step complete" flag (must go 1 to advance). hold270 = fade hold-timer bits (frozen nonzero =
        // fade stuck opaque). cd100/req248 = finalize counters.
        let mms_next = mms
            .and_then(|m| unsafe { safe_read_i32(m + MOVEMAPSTEP_NEXT_STEP_4C_OFFSET) })
            .unwrap_or(-1);
        let mms_done = mms
            .and_then(|m| unsafe { safe_read_u8(m + MOVEMAPSTEP_DONE_FLAG_50_OFFSET) })
            .map(|b| b as i32)
            .unwrap_or(-1);
        let mms_hold = mms
            .and_then(|m| unsafe { safe_read_i32(m + MOVEMAPSTEP_HOLD_TIMER_270_OFFSET) })
            .unwrap_or(0);
        let mms_cd = mms
            .and_then(|m| unsafe { safe_read_i32(m + MOVEMAPSTEP_COUNTDOWN_100_OFFSET) })
            .unwrap_or(-1);
        let mms_req248 = mms
            .and_then(|m| unsafe { safe_read_i32(m + MOVEMAPSTEP_FINALIZE_REQ_248_OFFSET) })
            .unwrap_or(-1);
        // SAME-SESSION RELOAD ADVANCE-GATE CANDIDATE (2026-07-19): state-18 disassembly shows
        // MoveMapStep::STEP_MoveMap advances to Cleanup/Finish through the native gate at +0x4b8.
        // The failing reload has gate=1/0 while movement is still false, then Cleanup/Finish destroys
        // WorldChrMan via InGameStep::_Common_Finalize. Hold only that advance bit low during the active
        // reload handoff, and release it as soon as the current reload epoch proves movement.
        let reload_epoch = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
        let movement_proven_for_reload = crate::compat::CAN_MOVE_CONFIRMED
            .load(Ordering::SeqCst)
            && crate::compat::MOVE_PROBE_EPOCH.load(Ordering::SeqCst) == reload_epoch;
        // PHASE-3 (bd PHASE3-render-release-is-CommonFinalize): while the outgoing-teardown fix is active,
        // do NOT clear the +0x4b8 advance gate. Blocking Cleanup/Finish here is exactly what keeps the
        // reused in-place world's WorldChrMan alive (the ~5x-heavier render); with the OUTGOING world torn
        // down first the reload builds fresh and this hold must not fire. Re-engages on fail-soft.
        if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
            == SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF
            && reload_epoch > 0
            && !movement_proven_for_reload
            && mms_step == 18
            && !crate::compat::gating::outgoing_teardown_suppresses_holds()
            && let Some(mms_ptr) = mms {
                let old_gate =
                    unsafe { safe_read_u8(mms_ptr + MOVEMAPSTEP_ADVANCE_GATE_LO_4B8_OFFSET) }
                        .unwrap_or(0);
                if old_gate != 0 {
                    unsafe {
                        *((mms_ptr + MOVEMAPSTEP_ADVANCE_GATE_LO_4B8_OFFSET) as *mut u8) = 0;
                    }
                    let n =
                        SYSTEM_QUIT_QUICKLOAD_MMS4B8_HOLD_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                    if n <= 8 || n.is_power_of_two() {
                        append_autoload_debug(format_args!(
                            "AUTOLOAD-HANDOFF MMS4B8 HOLD #{n}: epoch={reload_epoch} ig_d8={ig_d8} mms_step={mms_step} old_gate={old_gate}; blocking Cleanup/Finish until movement proof"
                        ));
                    }
                }
            }
        // ENDING-REQUEST diagnostic (2nd runtime-accum lock, 2026-07-16). STEP_MoveMap walks the child
        // to its -1 terminal only while the advancer FUN_140afa7c0 sets menuData+0x5e (cVar10 = an
        // ending/load-completion condition). If 0x5e stays 0 on a re-load, the child parks at resident
        // step 18 and the InGameStep parent (finished == MoveMapStep+0x48==-1) waits forever. Read the
        // OUTPUT (0x5e) + the two easy INPUTS (return-title byte 0x5d, force-flag 0x3d856a0) + the PARENT
        // step (InGameStep+0x48/+0x4c) so the next repro names why the ending request never fires.
        let ig_pstep = ig_ptr
            .and_then(|ig| unsafe { safe_read_i32(ig + INGAMESTEP_STEP_STATE_OFFSET) })
            .unwrap_or(-1);
        let ig_pnext = ig_ptr
            .and_then(|ig| unsafe { safe_read_i32(ig + INGAMESTEP_NEXT_STATE_OFFSET) })
            .unwrap_or(-1);
        let menudata = unsafe { safe_read_usize(module_base + CS_MENU_MAN_GLOBAL_RVA) }
            .filter(|&m| m > 0x10000)
            .and_then(|m| unsafe { safe_read_usize(m + CS_MENU_MAN_MENU_DATA_OFFSET) })
            .filter(|&d| d > 0x10000);
        let md_5d = menudata
            .and_then(|d| unsafe { safe_read_u8(d + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET) })
            .map(|b| b as i32)
            .unwrap_or(-1);
        let md_5e = menudata
            .and_then(|d| unsafe { safe_read_u8(d + CS_MENU_DATA_ENDING_FLAG_5E_OFFSET) })
            .map(|b| b as i32)
            .unwrap_or(-1);
        let ending_force =
            unsafe { safe_read_u8(module_base + ENDING_REQUEST_FORCE_FLAG_3D856A0_RVA) }
                .map(|b| b as i32)
                .unwrap_or(-1);
        // Remaining cVar10 ending-request INPUTS read straight off GameMan (the load-in signals): 0xb7c,
        // 0xb7d, warpRequested@0x10. On a good load one is 1; on the stuck re-load they should reveal the
        // stale one. gm is the live GameMan ptr from the outer guard; safe_read guards a bad offset.
        let gb7c = unsafe { safe_read_u8(gm + GAME_MAN_ENDING_FLAG_B7C_OFFSET) }
            .map(|b| b as i32)
            .unwrap_or(-1);
        let gb7d = unsafe { safe_read_u8(gm + GAME_MAN_ENDING_FLAG_B7D_OFFSET) }
            .map(|b| b as i32)
            .unwrap_or(-1);
        let gwarp = unsafe { safe_read_u8(gm + GAME_MAN_WARP_REQUESTED_10_OFFSET) }
            .map(|b| b as i32)
            .unwrap_or(-1);
        let csm6b0 = menu_man
            .and_then(|m| unsafe { safe_read_u8(m + CS_MENU_MAN_FIELD_6B0_OFFSET) })
            .map(|b| b as i32)
            .unwrap_or(-1);
        let lsmode = menu_man
            .and_then(|m| unsafe { safe_read_u8(m + CSMENUMAN_LOADINGSCREEN_MODE_728_OFFSET) })
            .map(|b| b as i32)
            .unwrap_or(-1);
        let gm_bf5 = unsafe { safe_read_u8(gm + GAME_MAN_LOADING_MODE_BF5_OFFSET) }
            .map(|b| b as i32)
            .unwrap_or(-1);
        let delay_delete =
            unsafe { safe_read_usize(module_base + CS_DELAY_DELETE_MAN_GLOBAL_RVA) }.unwrap_or(0);
        let dd40 = if delay_delete != null {
            unsafe { safe_read_i32(delay_delete + CS_DELAY_DELETE_PENDING_40_OFFSET) }.unwrap_or(-1)
        } else {
            -1
        };
        let dd54 = if delay_delete != null {
            unsafe { safe_read_u8(delay_delete + CS_DELAY_DELETE_FINALIZE_54_OFFSET) }
                .map(|b| b as i32)
                .unwrap_or(-1)
        } else {
            -1
        };
        let (world_chr_man, main_player) =
            if let Ok(world_chr_man) = unsafe { eldenring::cs::WorldChrMan::instance_mut() } {
                (
                    world_chr_man as *mut _ as usize,
                    world_chr_man
                        .main_player
                        .as_ref()
                        .map(|p| p.as_ptr() as usize)
                        .unwrap_or(0),
                )
            } else {
                (0, 0)
            };
        // ENDING-REQUEST RECOVERY (2026-07-18, live-proven fix for the genuine cross-char switch stall,
        // bd live-genuine-switch-stalls-mms18-end5e0-2026-07-18). A genuine switch's return-title
        // suppresses the quit-save (no-save-on-quit by design), so none of cVar10's inputs
        // (b7c/b7d/force/warp/rt5d) are set, the advancer FUN_140afa7c0 never writes menuData+0x5e=1,
        // and the OLD world's MoveMapStep child parks at STEP_MoveMap(18): the InGameStep parent (step 7)
        // waits forever on MoveMapStep+0x48 != -1, so the world never tears down (the switched-from char
        // stays resident, mms_step pinned at 18, end5e=0, rt5d=0, b7c1=1, blocks>0 -- the exact runtime
        // signature captured live). RE-proven differentiator (bd
        // ending-request-recovery-fix-applied-2026-07-16): an ADVANCING child has menuData+0x5d(rt5d)==1;
        // the stuck child has rt5d==0. So drive rt5d=1 -> the advancer computes cVar10=1 -> writes 0x5e=1
        // -> STEP_MoveMap walks the child 18->Cleanup(19)->Finish(20)->-1, tearing down the old world so
        // the clean-title autoload of the picked slot proceeds via the proven boot path. Then CLEAR rt5d
        // the frame the child leaves 18, BEFORE the ~4s resident-world bounce a lingering rt5d triggers via
        // CheckReturnToTitle (return_title.rs:1-7). This is distinct from bc4 (the 1st teardown flag): the
        // suppressed quit-save that would pump bc4->3 and set 0x5d never runs, so we supply the
        // ending-request input directly. Gate on the EXACT stuck signature + a sustained streak so a
        // healthy load (leaves 18 in a few frames) never trips it. The stricter settled-gate run
        // (target/runtime-probe/samechar-3x-settledgate-20260719-053409) proved this same signature also
        // appears after Continue/SetState5 during AUTOLOAD_HANDOFF: load2 becomes movable, but its native
        // MoveMap/requestCode handoff remains parked at mms18 with end5e=0/rt5d=0. So keep this recovery
        // enabled for the whole active switch phase, including AUTOLOAD_HANDOFF. Boot-idle use of this
        // flag was rejected by the taskadvancer run: setting 0x5d outside an active switch can settle mms18
        // by tearing down the boot world. The separate return-title chain/final-functor gates now exclude
        // AUTOLOAD_HANDOFF, and this block still clears rt5d the frame mms leaves 18, so the advancer flag
        // can finish active-switch native loads without replaying return-title. ENDING_REQUEST_SET_COUNT is
        // the semaphore.
        if let Some(md) = menudata {
            let quickload_phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
            let active_switch_phase =
                quickload_phase >= SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED;
            // FINALIZE-FORCING DISABLED (user 2026-07-21): see the IN-WORLD FINALIZE DRIVE above -- the
            // menuData+0x5d finalize-forcing is not the vanilla path; disable it so the load follows vanilla.
            const FINALIZE_FORCING_ENABLED: bool = false;
            let stuck_mms18 = FINALIZE_FORCING_ENABLED
                && active_switch_phase
                && ig_d8 == INGAMESTEP_REQUEST_CODE_MOVEMAP_PENDING
                && mms_step == MOVEMAPSTEP_STEP_MOVEMAP_INDEX
                && md_5e == 0
                && md_5d == 0
                && mms_b7c1 == 1
                && mms_blocks > 0;
            if stuck_mms18 {
                let streak = ENDING_REQUEST_STALL_STREAK.fetch_add(1, Ordering::SeqCst) + 1;
                if streak >= ENDING_REQUEST_STALL_RELEASE_FRAMES
                    && ENDING_REQUEST_SET
                        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    let n = ENDING_REQUEST_SET_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                    // DRIVE rt5d=1 -- the NON-WARP finalize input load1 uses (re-enabled from the prior
                    // observe-only downgrade, RE bd REFINED-load1-finishes-via-rt5d-end5e-warp0-load2-
                    // overcleared-both-2026-07-20). Run 20260720-101944 proved: LOAD1 completes
                    // mms18->19(CLEANUP)->20(FINISH) with rt5d/end5e=1 and warp=0, while LOAD2 arrives at
                    // mms18 with ALL cVar10 inputs 0 (warp consumed + our handoff over-cleared 0x5d/0x5e)
                    // and freezes at finalize case 0 (present but frozen; 463 move-input frames, 4 moved).
                    // Setting menuData+0x5d=1 makes the advancer FUN_140afa7c0 compute cVar10=1 and write
                    // 0x5e=1, walking the child 18->19->20 the SAME non-warp way load1 does -- no case-8
                    // re-warp (warp stays native-owned/cleared, so no warp-reload teardown loop). The
                    // ending-latch-residual-clear below clears rt5d the frame mms leaves 18, preventing the
                    // ~4s CheckReturnToTitle bounce. One-shot per stall (ENDING_REQUEST_SET latch).
                    unsafe {
                        *((md + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET) as *mut u8) = 1;
                    }
                    append_autoload_debug(format_args!(
                        "MMS18 RT5D DRIVE #{n}: drove menuData+0x5d=1 at mms18 stall (streak={streak} phase={quickload_phase} end5e=0 rt5d=0 b7c1=1 blocks={mms_blocks}) -- non-warp finalize driver (load1 path); warp left native-owned"
                    ));
                }
            } else {
                // WHY-NOT diagnostic: load2 sits FROZEN at mms18/finalize-0 but the rt5d drive above did
                // not fire, so one of stuck_mms18's sub-conditions is false. Log each one (throttled) so a
                // run names the exact blocker instead of guessing (b7c1==1/blocks>0 were captured from the
                // cross-char switch stall and may not hold for the boot-reload freeze). Fires only at the
                // frozen mms18 signature (present, ig_d8 pending) so healthy loads stay quiet.
                if mms_step == MOVEMAPSTEP_STEP_MOVEMAP_INDEX
                    && player_present
                    && ig_d8 == INGAMESTEP_REQUEST_CODE_MOVEMAP_PENDING
                {
                    let w = ENDING_REQUEST_WHYNOT_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                    let epoch_dbg =
                        SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
                    if w <= 40 || w.is_multiple_of(10) {
                        append_autoload_debug(format_args!(
                            "MMS18 RT5D DRIVE WHY-NOT #{w} epoch={epoch_dbg}: stuck_mms18=false at frozen mms18 -- active_switch={active_switch_phase}(phase={quickload_phase}) ig_d8={ig_d8} md_5e={md_5e} md_5d={md_5d} b7c1={mms_b7c1} blocks={mms_blocks} (drive needs active_switch && ig_d8==1 && md_5e==0 && md_5d==0 && b7c1==1 && blocks>0)"
                        ));
                    }
                }
                ENDING_REQUEST_STALL_STREAK.store(0, Ordering::SeqCst);
                if ENDING_REQUEST_SET.load(Ordering::SeqCst) == 1
                    && mms_step != MOVEMAPSTEP_STEP_MOVEMAP_INDEX
                {
                    ENDING_REQUEST_SET.store(0, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "MMS18 NATIVE-WARP OBSERVE: cached mms left step 18 (mms_step={mms_step}); native owns warp_requested autoclear"
                    ));
                }
            }
        }
        let mms_disp = mms.unwrap_or(0);
        let mms_step_pub = if mms.is_some() && mms_step >= 0 {
            mms_step as usize
        } else {
            usize::MAX
        };
        let prev_mms_step = SWITCH_ORACLE_MMS_STEP.swap(mms_step_pub, Ordering::SeqCst);
        let mms_step_changed = prev_mms_step != mms_step_pub;
        SWITCH_ORACLE_REQUEST_CODE.store(ig_d8, Ordering::SeqCst);
        // Publish the MoveMapStep finalize substate (+0x12a) so the loading-bar MOVE MAP phase shows
        // its real native sub-progression (0..9) instead of coarse proxies.
        SWITCH_ORACLE_FINALIZE_12A.store(
            if mms_disp != 0 {
                unsafe { safe_read_u8(mms_disp + MOVEMAPSTEP_FINALIZE_SUBSTATE_12A_OFFSET) }
                    .map(|v| v as i32)
                    .unwrap_or(-1)
            } else {
                -1
            },
            Ordering::SeqCst,
        );
        // b80 (load_in_progress) IS GameMan.save_state. Publish for the loading bar, and DRAIN the
        // FD4-IO reload's stuck residency: the reload SUBMIT/DRAIN leaves b80=3 (the resident IO buffer
        // is never consumed by the feed), and the finalize case-7 gate (FUN_14067a170 == saveState==0)
        // waits on it forever. Force b80->0 ONLY at the exact stuck signature -- AUTOLOAD_HANDOFF,
        // MoveMapStep step 18, finalize substate live (1..=9), b80==3 RESIDENT, player present (world
        // genuinely resident+live) -- so a healthy load (b80 already draining) is never touched.
        // Marker-gated (er-effects-reload-drainb80.txt) for A/B against the stuck baseline.
        let b80_now = if gm != null {
            unsafe { safe_read_i32(gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) }.unwrap_or(-1)
        } else {
            -1
        };
        SWITCH_ORACLE_B80.store(b80_now, Ordering::SeqCst);
        let finalize_now = SWITCH_ORACLE_FINALIZE_12A.load(Ordering::SeqCst);
        // EARLY b73 HOLD -- the post-warp revert ROOT FIX (RE er-effects-rs-9fmm). GameMan+0xb73 is the
        // quit-save RETURN-TITLE latch our System->Quit sets (FUN_14067a490); it persists into the
        // reload's world, and the MoveMapStep ending evaluator FUN_140afa7c0 -> FUN_140679460
        // (= b73 && !savePopup && bc4!=3) latches session-end into CSMenuMan.menuData+0x5e at load2's
        // MoveMap entry -> STEP_EndFlow -> SetState 6->2 revert. The case-7 drain below also clears b73
        // but only at finalize (mms18) -- TOO LATE, the evaluator already latched. Clear it CONTINUOUSLY
        // from reload commit (FRESH_DESER_DONE=1), before mms18, so the evaluator never sees b73=1.
        // Semantically safe: post-SetState5 the quit->return-title intent is already fulfilled (we are
        // loading, not returning to title) and nothing legitimately re-sets b73 during the stream
        // (auto-save touches saveRequested, not b73). DEFAULT behavior gated only on the real reload
        // condition (a committed switch reload); no marker/env toggle -- validated by running.
        if gm != null && SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst) == 1 {
            let b73 = unsafe { safe_read_u8(gm + GAME_MAN_SAVE_REQUEST_COMPANION_B73_OFFSET) }
                .unwrap_or(0);
            if b73 != 0 {
                unsafe { *((gm + GAME_MAN_SAVE_REQUEST_COMPANION_B73_OFFSET) as *mut u8) = 0 };
                let n = RELOAD_B73_HOLD_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                if n <= 8 || n.is_power_of_two() {
                    append_autoload_debug(format_args!(
                        "reload-b73-hold #{n}: cleared GameMan+0xb73 return-title latch (mms_step={mms_step} finalize={finalize_now}) BEFORE the MoveMapStep ending evaluator can latch session-end -- keeps the reloaded world"
                    ));
                }
            }
            // ENDING-LATCH RESIDUAL CLEAR (corrected, RE er-effects-rs-9fmm run 1650). menuData+0x5e is
            // NOT a pure return-title flag: FUN_140afa7c0 writes it = cVar10 each frame, and cVar10=1 is
            // exactly what DRIVES the MoveMap finalize (walk field25_0x12a 0..8, case 8 advances 18->19 and
            // consumes warpRequested). So while warpRequested==1 md5e=1 is the finalize DRIVER -- clearing
            // it then SABOTAGES the finalize (the earlier every-frame clear caused the MMS-CLEANUP re-drive
            // bursts + timing variance). The revert's ENDCOND was ONLY md5e=1 with warp=0: after case 8
            // consumes the warp (warpRequested 1->0), md5e stays 1 RESIDUAL and STEP_EndFlow reads that as
            // return-to-title -> SetState 6->2. FIX: clear the residual ONLY when warpRequested==0 (warp
            // consumed / not driving), never during the warp-driven finalize.
            // Gate ONLY on warpRequested==0 (warp consumed / not driving the finalize). Do NOT also gate
            // on mms_step/player: the residual persists after the child is torn down (mms==-1) and the
            // player is briefly gone, which is exactly the frame STEP_EndFlow reads it (run 1707: the
            // clear fired 0 times because the mms>=18/player sub-gate excluded that frame). While
            // warpRequested==1 md5e is the live finalize driver and is left untouched.
            let warp_req = unsafe { safe_read_u8(gm + GAME_MAN_WARP_REQUESTED_10_OFFSET) }.unwrap_or(1);
            if warp_req == 0
                && let Some(md) = (unsafe { safe_read_usize(module_base + CS_MENU_MAN_GLOBAL_RVA) })
                    .filter(|&m| m > PAB_MIN_HEAP_PTR)
                    .and_then(|m| unsafe { safe_read_usize(m + CS_MENU_MAN_MENU_DATA_OFFSET) })
                    .filter(|&m| m > PAB_MIN_HEAP_PTR)
                {
                    let e5 = unsafe { safe_read_u8(md + CS_MENU_DATA_ENDING_FLAG_5E_OFFSET) }
                        .unwrap_or(0);
                    let d5 = unsafe { safe_read_u8(md + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET) }
                        .unwrap_or(0);
                    if e5 != 0 || d5 != 0 {
                        unsafe {
                            *((md + CS_MENU_DATA_ENDING_FLAG_5E_OFFSET) as *mut u8) = 0;
                            *((md + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET) as *mut u8) = 0;
                        }
                        let n = RELOAD_ENDING_LATCH_HOLD_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                        if n <= 8 || n.is_power_of_two() {
                            append_autoload_debug(format_args!(
                                "reload-ending-latch-residual-clear #{n}: warp consumed (warp=0), cleared menuData+0x5e={e5}/+0x5d={d5} residual (mms_step={mms_step} finalize={finalize_now}) -- prevents the post-warp return-to-title revert"
                            ));
                        }
                    }
                }
        }
        // Unblock the finalize case-7->8 gate on the warm reload. The gate is
        // FUN_14067a170() (== saveState==0) && !ShouldSave() (saveRequested==0) && !FUN_140679460()
        // (b73==0) && FUN_140a9ceb0(CSRemo). Fire at the exact stuck signature and satisfy the two
        // GameMan-owned conditions we control: (a) drain b80 (== save_state) 3->0 (the reload's
        // resident IO buffer, never consumed by the feed), and (b) clear saveRequested -- the reload's
        // unwanted SetState5/advancer autosave (user 2026-07-19: the reload must NOT autosave; only an
        // explicit user save writes). Both are re-applied each frame since the native advancer re-sets
        // them. DEFAULT behavior gated only on the real stuck runtime signature (world resident+live at
        // mms18 finalize 1..9, AUTOLOAD_HANDOFF) so a healthy load is never touched; no marker toggle.
        if gm != null
            && mms_step == MOVEMAPSTEP_STEP_MOVEMAP_INDEX
            && (1..=9).contains(&finalize_now)
            && player_present
            && SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
                == SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF
        {
            if b80_now == FULLREAD_B80_RESIDENT {
                unsafe {
                    *((gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *mut i32) =
                        GAME_MAN_SAVE_STATE_IDLE;
                }
            }
            let mut cleared_save = false;
            if let Ok(gm_typed) = unsafe { eldenring::cs::GameMan::instance_mut() } {
                cleared_save = er_save_loader::GameManSaveAccess::save_requested(gm_typed);
                er_save_loader::GameManSaveAccess::set_save_requested(gm_typed, false);
            }
            // Clear the b73 save-request companion (FUN_140679460 = b73 && menu_gate && bc4!=3): our
            // return-title REQUEST set it for the switch teardown and it lingers (LEVEL flag, nothing
            // resets it), keeping the case-7 !FUN_140679460() condition false. Runtime-confirmed b73=1
            // at the finalize-7 stall.
            let b73_was = unsafe { safe_read_u8(gm + GAME_MAN_SAVE_REQUEST_COMPANION_B73_OFFSET) }
                .unwrap_or(0);
            if b73_was != 0 {
                unsafe { *((gm + GAME_MAN_SAVE_REQUEST_COMPANION_B73_OFFSET) as *mut u8) = 0 };
            }
            // Mark the finalize as effectively done once it reaches WARP/SERVER FINALIZE (8) so the
            // post-finish stable-proof can hold the world immediately (the world reverts before the
            // 60-frame move probe could latch can_move).
            if finalize_now >= 8 {
                SYSTEM_QUIT_RELOAD_FINALIZE_DONE_EPOCH.store(
                    SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst),
                    Ordering::SeqCst,
                );
            }
            let n = RELOAD_DRAIN_B80_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            if n <= 8 || n.is_power_of_two() {
                append_autoload_debug(format_args!(
                    "reload-drain-b80: unblock case-7 at mms18 finalize={finalize_now} b80={b80_now}->0 save_requested_was={cleared_save}->0 b73_was={b73_was}->0 (world resident+live) #{n}"
                ));
            }
        }
        SWITCH_ORACLE_PLAYER_PRESENT.store(usize::from(player_present), Ordering::SeqCst);
        SWITCH_ORACLE_MENU_JOB_PRESENT.store(usize::from(menu_job != 0), Ordering::SeqCst);
        SWITCH_ORACLE_LOADING_FIELD10.store(loading_screen_field10, Ordering::SeqCst);
        SWITCH_ORACLE_LOADING_FIELD11.store(loading_screen_field11, Ordering::SeqCst);
        SWITCH_ORACLE_MMS_B7C1.store(mms_b7c1, Ordering::SeqCst);
        SWITCH_ORACLE_MMS_BLOCKS.store(mms_blocks, Ordering::SeqCst);
        let bc4v = return_title_job_predicate_bc4;
        let stable =
            player_present && ig_d8 == INGAMESTEP_REQUEST_CODE_STABLE_IN_WORLD && menu_job != 0;
        let sf = if stable {
            let v = SWITCH_ORACLE_STABLE_FRAMES.fetch_add(1, Ordering::SeqCst) + 1;
            SWITCH_ORACLE_MAX_STABLE_FRAMES.fetch_max(v, Ordering::SeqCst);
            v
        } else {
            SWITCH_ORACLE_STABLE_FRAMES.store(0, Ordering::SeqCst);
            0
        };
        let peak = SWITCH_ORACLE_MAX_STABLE_FRAMES.load(Ordering::SeqCst);
        // POST-FINISH STABLE-PROOF: hold the reloaded world (phase->IDLE + clear b78) so the native
        // InGameStep does not revert to title after the MoveMap finish. Trigger on the STRONGEST
        // readiness signal available -- movement proven (can_move latched) for THIS reload epoch, which
        // latches ~immediately after the finalize completes, BEFORE the ~1.4s post-finish revert -- OR
        // the legacy 30 stable frames as a fallback. Latch is per-reload-epoch (NOT FRESH_DESER_DONE,
        // which own_load consumes at commit -- that consumption is exactly why this block never fired
        // and the world reverted after finish). bd er-effects-rs-9fmm.
        let reload_epoch_now = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
        let can_move_for_reload = reload_epoch_now > 0
            && crate::compat::CAN_MOVE_CONFIRMED.load(Ordering::SeqCst)
            && crate::compat::MOVE_PROBE_EPOCH.load(Ordering::SeqCst) == reload_epoch_now;
        // Finalize effectively done this reload epoch (reached WARP/SERVER FINALIZE) -- hold NOW, before
        // the post-finish revert, while the player is still present.
        let finalize_done_for_reload = reload_epoch_now > 0
            && SYSTEM_QUIT_RELOAD_FINALIZE_DONE_EPOCH.load(Ordering::SeqCst) == reload_epoch_now
            && player_present;
        if (finalize_done_for_reload || sf >= 30 || can_move_for_reload)
            && reload_epoch_now > 0
            && SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
                >= SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF
            && SYSTEM_QUIT_STABLE_PROOF_EPOCH.swap(reload_epoch_now, Ordering::SeqCst)
                != reload_epoch_now
        {
            SYSTEM_QUIT_QUICKLOAD_PHASE.store(SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE, Ordering::SeqCst);
            // DO NOT clear GameMan+0xb78 here. RE-confirmed (InGameStep::STEP_MoveMap_Update
            // @0x140aec810 + orchestrator FUN_140afb970, bd er-effects-rs-9fmm): b78
            // (requestedSaveSlotLoad) IS the MoveMap WARP TARGET the finalize (case 8) consumes to load
            // the destination block and rebuild the player; STEP_MoveMap_Update skips the map load when
            // the destination BlockId is 0xffffffff, which is what happens if b78 was cleared to -1
            // before the warp issues its load. This block fires at finalize>=8 -- exactly the warp window
            // -- so clearing b78 here removed the warp destination mid-warp and the world was torn down at
            // Cleanup with nothing to reload (player true->false -> native SetState 6->2 revert; runtime
            // af27ec75 +84315 clear -> +84746 player gone). The native warp consumes and autoclears b78
            // itself; leave it armed. Clearing save_requested is safe (it is not the destination and it
            // satisfies the case-7 ShouldSave gate); phase->IDLE only stops our re-drive.
            if let Ok(gm_typed) = unsafe { eldenring::cs::GameMan::instance_mut() } {
                er_save_loader::GameManSaveAccess::set_save_requested(gm_typed, false);
            }
            append_autoload_debug(format_args!(
                "system-quit-quickload: post-finish stable proof OK (can_move={can_move_for_reload} sf={sf}) epoch={reload_epoch_now} slot={slot} player_present={player_present} ig_d8={ig_d8} -> phase IDLE, cleared save_requested; b78 KEPT ARMED as the warp target (native finalize consumes+autoclears it) so the destination reloads instead of reverting"
            ));
        }
        let n = SWITCH_ORACLE_TICK.fetch_add(1, Ordering::SeqCst) + 1;
        let dropped = !stable && peak >= 30;
        // FIX: on the second load (in_world), flip the map-mount guard each tick (cooldown-bounded, self-
        // limiting once the map mounts) so the game re-enqueues the skipped map mount+bind and the block
        // cap +0x90 is repopulated -> world reaches readiness. Runs every tick (before the periodic emit).
        crate::compat::trace::map_mount_guard_flip_tick(
            IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES,
            mms_step,
            sf as i64,
        );
        if n <= 10 || n.is_multiple_of(30) || matches!(sf, 1 | 60 | 300 | 600) || dropped || mms_step_changed
        {
            let cls = if peak >= 300 {
                "LOADED_STABLE"
            } else if dropped {
                "DROPPED(bounce/reload)"
            } else if bc4v == 1 && ig_d8 == INGAMESTEP_REQUEST_CODE_MOVEMAP_PENDING && mms.is_some()
            {
                // bc4=1 + requestCode=1 + a live MoveMapStep child = the 3rd-load softlock: InGameStep
                // step 7 waiting on the child that never finishes. mms_step names the exact stuck point.
                "MMS-CHILD-STALL(step7 waiting on child)"
            } else if bc4v == 1 {
                "bc4=1(save/teardown pending)"
            } else {
                "in-progress"
            };
            // The stuck block's vtable as a module RVA -> look it up in the dump to RE the
            // vtable[0x10] load-state getter that returns null (why the legacy load is never created).
            let blk_vt_rva = mms_blk_vt.saturating_sub(module_base);
            append_autoload_debug(format_args!(
                "SWITCH-ORACLE #{n}: slot={slot} bc4={bc4v} player={player_present} ig_d8={ig_d8} pstep={ig_pstep}/{ig_pnext} menu_job=0x{menu_job:x} csm6b0={csm6b0} lsmode={lsmode} ls10={loading_screen_field10} ls11={loading_screen_field11} gm_bf5={gm_bf5} dd=0x{delay_delete:x} dd40={dd40} dd54={dd54} wcm=0x{world_chr_man:x} mainp=0x{main_player:x} stable_frames={sf} peak={peak} mms=0x{mms_disp:x} mms_step={mms_step}({}) next={mms_next} fin12a={finalize_now} epoch={reload_epoch_now} done50={mms_done} gate={mms_gate_lo}/{mms_gate_hi} end5e={md_5e} rt5d={md_5d} force={ending_force} b7c={gb7c} b7d={gb7d} warp={gwarp} hold270=0x{mms_hold:x} cd100={mms_cd} req248={mms_req248} b7c1={mms_b7c1} blocks={mms_blocks} curblk=0x{mms_cur_block:x} b798=0x{mms_b798:x} b79c=0x{mms_b79c:x} blk_found={mms_block_found} blk_ls=0x{mms_blk_ls:x} blk_2c={mms_blk_2c} blk_2d={mms_blk_2d} blk_35={mms_blk_35}{mms_blk_phase2} ar_wanted={mms_ar_wanted} ar_state={mms_ar_state} ar_bres={mms_ar_bres} fc_present={mms_fc_present} fc_notloaded={mms_fc_notloaded} fc_stuck=[{mms_fc_stuck}] blk_vt_rva=0x{blk_vt_rva:x} ll_size={ll_size} ll_fcap=0x{ll_fcap:x} ll_path='{ll_path}' ow_cnt={mms_ow_count} ow=[{mms_ow_areas}] blk_areas=[{mms_block_areas}] phase={} -- {cls}",
                movemapstep_step_name(mms_step),
                SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
            ));
            // PROBE (reliable): fire the EBL mount census the moment WORLD RES WAIT (mms_step 3) is reached
            // on a SECOND load, from this always-ticking oracle (the WORLDRES-GETTER is silent some loads).
            // One-shot; emits the `EBL-MOUNT-CENSUS DONE` measurement semaphore -> the monitor tears down 1s
            // after that exact line. m28 ABSENT in the registry => mount step skipped; m28 present but the
            // block cap +0x90 still null => bind step skipped -- discriminates WHERE the warm-reload guard is.
            // Census is the MEASUREMENT mode (fix disabled via the marker); when the guard-flip FIX is
            // active it is off, so its DONE line does not trigger a premature census-teardown.
            if mms_step == 3
                && IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES
                && !crate::compat::trace::blockres_stalecap_fix_enabled()
            {
                crate::compat::trace::run_ebl_mount_census("oracle-mms3");
            }
        }
        // RESTORE bc4 AFTER THE FUNCTOR (save-disabled switch completion, 2026-07-16). We forced bc4=READY(3)
        // at the return-title REQUEST purely to fire the final functor without a quit-save. bc4=3 has done its
        // one job the moment the functor has fired (FINAL_FUNCTOR_CALL_COUNT>0), so put it straight back to 0
        // -- leaving it at 3 would keep clearing the incoming world's STEP_MoveMap advance gate (+0x4b8)
        // (FUN_140679010 reads bc4). This is NOT the step-18 stall fix: runtime (2026-07-16) proved the child
        // parks at STEP_MoveMap with the +0x4b8 gate ALREADY ready (1/0) at bc4=0 and cd100 (field17_0x100)
        // frozen -- i.e. the child's step handler is not ticking at all, a task-starvation freeze, NOT a bc4
        // gate. So this only keeps bc4 from interfering; the real freeze is handled elsewhere. Deterministic
        // (keyed on the functor one-shot), fires effectively once. Game-thread; `gm` non-null per outer guard.
        let post_continue_stable_pending_for_bc4 = SYSTEM_QUIT_QUICKLOAD_PHASE
            .load(Ordering::SeqCst)
            >= SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF
            && SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst) != 0
            && SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst) == 0;
        let bc4_ready_done = bc4v == GAME_MAN_RETURN_TITLE_JOB_PREDICATE_READY
            && (SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT.load(Ordering::SeqCst) > 0
                || post_continue_stable_pending_for_bc4);
        if bc4_ready_done {
            unsafe {
                *((gm + GAME_MAN_RETURN_TITLE_JOB_PREDICATE_BC4_OFFSET) as *mut i32) = 0;
            }
            return_title_job_predicate_bc4 = 0;
            let n = SYSTEM_QUIT_LOAD3_FINALIZE_CLEAR_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            if n <= 5 || n.is_multiple_of(60) {
                let source = if post_continue_stable_pending_for_bc4 {
                    "post-Continue stable wait"
                } else {
                    "final functor fired"
                };
                append_autoload_debug(format_args!(
                    "system-quit-quickload: restored bc4->0 after {source} #{n} (bc4=READY had done its return-title job; cleared so it does not gate the incoming world's STEP_MoveMap)"
                ));
            }
        }
    }
    // POST-COMMIT DORMANCY (2026-07-16, runtime-confirmed instability). This whole per-frame switch block
    // (b78 re-arm guard + disableSaveMenu clear + save-gate diagnostic) touches game state every frame while
    // the switch is active. Once the picked slot's load has COMMITTED (FRESH_DESER_DONE=1 -- set by the
    // feed/continue_confirm, cleared only when a NEW switch arms), the native continue_confirm->SetState5
    // stream is establishing the in-game session on the game's worker threads; our per-frame writes racing
    // that stream is the most likely cause of the non-deterministic Windows-native instability (bc4-freeze /
    // post-load bounce / stream hard-freeze -- all after the char has already fed correctly). So go FULLY
    // dormant here once committed: emit nothing, write nothing, let the native session settle. Inert before
    // the feed (latch 0 -> block runs and fires the initial load exactly as before); re-enabled for the next
    // genuine pick when the arm clears the latch.
    // PHASE UPPER BOUND (bd er-effects-rs-af3a; ported from branch commit c1b2dccd, which shipped it
    // as "b78 guard phase-windowing" and never reached main). The window is [RETURN_TITLE_REQUESTED,
    // AUTOLOAD_HANDOFF) == phases 2..3, not [2, inf).
    //
    // WHY A PHASE AND NOT THE LATCH. FRESH_DESER_DONE alone cannot close this window, because it is
    // not monotonic: system_quit_repro_guards.rs re-arms it to 0 on EVERY user ProfileSelect arm, and
    // its own comment there records where that leads -- "FRESH_DESER_DONE stuck 0 -> the b78 guard
    // wrote GameMan requestedSaveSlotLoad=-1 every frame -> native pump gate false -> world torn down
    // at ENTERING WORLD = the load3 softlock" (bd compounding-reload-two-roots-...-chainB-stale-fd4io-
    // latch-b78-2026-07-23). c1b2dccd reached the same conclusion from the other direction: "the
    // FRESH_DESER_DONE latch may be consumed/cleared before world readiness". The quickload phase does
    // not have that problem -- it advances to AUTOLOAD_HANDOFF and stays there -- and past that point
    // the SetState5/handoff owns b78, so BOTH of this block's branches (force -1, and re-arm = slot)
    // are wrong there regardless of what the latch says.
    //
    // WHAT THIS DOES *NOT* FIX, stated so nobody re-derives it. It would NOT have prevented the PR-117
    // black screen: that write happened at quickload phase 3, which is INSIDE this window. Only the
    // fd4io COMMIT stand-down below covers that. The two conditions guard different failures and
    // neither subsumes the other.
    //
    // MEASURED IMPACT ON HEALTHY RUNS: none. Across the five user-driven and agent-driven runs captured
    // 2026-08-01 (userdrive-b78-20260801-091449, userdrive-MAIN-control-20260801-092134,
    // userdrive-COMMITscoped-20260801-092654 and their pre-run logs), every single b78-guard write --
    // 12 of them -- reported quickload phase 3. Zero fired at phase >= 4. So this bound is inert on a
    // run that behaves, and only bites the stuck-latch shape above.
    let b78_guard_quickload_phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    let b78_guard_window_open = (SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF).contains(&b78_guard_quickload_phase)
        && SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst) == 0
        && gm != null
        && slot >= OWN_STEPPER_SLOT_ZERO;
    // FD4IO OWNERSHIP STAND-DOWN (black-screen fix, user eval run 20260731-user-eval-pr117
    // switch #2 / cross-save Banon; bd er-effects-rs-9jbe). Once the switch-reload fd4io machine
    // has left IDLE it OWNS GameMan+0xb78 as the warp target ("b78 kept armed (warp target)
    // through finalize" at SUBMIT->DRAIN->COMMIT); this guard's force-(-1) below raced that
    // finalize when the quickload phase lagged at 3 (+95422ms: "kept gm_b78=-1" 650ms AFTER the
    // fd4io COMMIT) and stripped the warp destination mid-warp -- STEP_MoveMap_Update reads
    // BlockId 0xffffffff, skips the map load, and the world tears down with nothing armed (black
    // screen, mms=-1, defaulted level-9 character, loading bar frozen at 1/500). The in-world
    // protection this guard exists for is UNAFFECTED: fd4io SUBMIT only ever happens post-teardown
    // at the clean title, so IDLE still covers the whole in-world window (the 2026-07-01
    // RequestLoadSlot spin, where writing b78=slot in-world made FUN_140afb970 spin
    // RequestLoadSlot 4600+ times). Reset per switch by reset_switch_reload_latches.
    //
    // SCOPED TO COMMIT, NOT ALL OF NON-IDLE (user-driven A/B, 2026-08-01, runs
    // userdrive-b78-20260801-091449 vs userdrive-MAIN-control-20260801-092134). The first shipped
    // form of this stand-down used `!= IDLE`, which also covers DRAIN -- and standing down through
    // DRAIN broke the user's ProfileSelect switch. Measured, same save/slot on both sides:
    //   MAIN     forces "kept gm_b78=-1" three times inside the DRAIN window; the finalize's own
    //            autosave then dispatches, saveRequested+b73 clear 51ms later, 0 declines/4 calls.
    //   != IDLE  leaves b78 armed for all 81 DRAIN frames; 15s later the NATIVE save dispatcher
    //            refuses the finalize autosave with `load-job-latched-0x18+0x20` (SL request slot
    //            still holds load_content=0x97e71200 job=0x353300c0, save_content=0x0) -- 907
    //            declines/910 calls, so b72/b73 latch forever, the case-7 gate (which needs both
    //            == 0, bd CASE7-GATE-DECOMPILED-...-2026-07-21) never opens, STEP_MoveMap self-
    //            loops at 18, the cover FPS-bails and the vanilla loading screen reaches the user.
    // So the DRAIN-window clears are load-bearing: they retire the load request before the finalize
    // needs the SL slot for its save. The PR-117 black screen was never a DRAIN-window write -- its
    // evidence is a "kept gm_b78=-1" 650ms AFTER the fd4io COMMIT. COMMIT is therefore the exact
    // and only window where the finalize owns b78 as the warp target and this guard must not touch
    // it; DRAIN keeps the pre-existing behavior.
    let fd4io_owns_b78 = er_telemetry::counters::SWITCH_RELOAD_FD4IO_PHASE.load(Ordering::SeqCst)
        == er_telemetry::counters::SWITCH_RELOAD_FD4IO_COMMIT;
    if b78_guard_window_open && fd4io_owns_b78 {
        // ENGAGEMENT SEMAPHORE. Count every frame the guard would have forced b78=-1 and now does
        // not. `> 0` == a real fd4io COMMIT overlap was survived.
        //
        // EXPECT 0 ON A HEALTHY RUN, BY CONSTRUCTION (measured 2026-08-01, run
        // userdrive-COMMITscoped-20260801-092654: two switches incl. a cross-save, 0 stand-downs,
        // 0 dispatcher declines, 0 native-LS exposure). COMMIT's own feed sets
        // FRESH_DESER_DONE=1 (own_load/loaders.rs), and that is one of `b78_guard_window_open`'s
        // conditions -- so on a run whose deserialize completes normally the window shuts the
        // instant COMMIT starts and this branch is unreachable. It becomes reachable only in the
        // PR-117 failure timing, where the deserialize has NOT completed and the window is still
        // open 650ms past COMMIT. That is the intended shape (a guard that is inert until the
        // thing it guards against is actually happening), but it means a clean run can NEVER
        // prove this engaged: healthy runs are non-regression evidence only, and `> 0` will only
        // ever be seen on a run that was heading for the black screen. Do not read 0 as broken,
        // and do not read a clean run as proof. bd er-effects-rs-9jbe.
        let n = er_telemetry::counters::SWITCH_RELOAD_B78_GUARD_STANDDOWNS
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        if n <= 5 || n.is_multiple_of(120) {
            append_autoload_debug(format_args!(
                "system-quit-quickload: b78 guard STOOD DOWN #{n} -- fd4io reload phase={} (COMMIT) owns GameMan+0xb78 as the warp target through finalize; not forcing -1 (phase={} slot={slot} bc4=0x{return_title_job_predicate_bc4:x})",
                er_telemetry::counters::SWITCH_RELOAD_FD4IO_PHASE.load(Ordering::SeqCst),
                SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
            ));
        }
    }
    if b78_guard_window_open && !fd4io_owns_b78 {
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
        // SWITCH-2 SOFT-LOCK FIX (game-task path, 2026-07-16). RE of the 1.16.1 dump (Ghidra persistent
        // project) proved the switch-2 freeze is the native quit-save aborting on a stale
        // `CSMenuMan->disableSaveMenu` (+0x13c): `ShouldSave` (dump 0x1406794c0) does
        // `if (CanShowSaveMenu()) saveRequested = 0;` and `CanShowSaveMenu` (dump 0x14080d150) returns
        // `GLOBAL_CSMenuMan->disableSaveMenu != 0`, so while that byte is set the quit-save returns 0
        // regardless of saveRequested/menu-gate/save_state -- `bc4` never pumps 1->2->3 and the world
        // never tears down. This is DISTINCT from the menu gate the stall-diag below logs (`FUN_14080d660`
        // = CSMenuMan+0x80->0x290/0x298), which is why that diag reported `menu_gate_ok=true ->
        // blocked_by=NONE(orchestrator not called?)`: it never checked +0x13c. The sibling menu-pump path
        // (`system_quit_restore_real_system_windows`) already clears this byte, but that observer is not
        // re-invoked for switch 2's torn-down windows, so the stale byte survives into the game-task stall.
        // Clear it HERE, on the game task, every frame the switch-2 stall signature holds (world still up
        // AND bc4 frozen at 1 == GAME_MAN_RETURN_TITLE_JOB_PREDICATE_PENDING), so the clear fires
        // independently of the menu-pump path. Native BOOL field write of an RE-confirmed gate (not a
        // speculative poke); no-op once 0. Byte-for-byte inert on switch 1 (its byte is already 0, and this
        // whole block only runs at phase >= RETURN_TITLE_REQUESTED). SYSTEM_QUIT_DISABLE_SAVE_MENU_CLEAR_COUNT
        // is the runtime semaphore: >0 on a switch == that switch's quit-save was gated OFF and we unblocked it.
        if world_up && return_title_job_predicate_bc4 == GAME_MAN_RETURN_TITLE_JOB_PREDICATE_PENDING
        {
            let cs_menu_man =
                unsafe { safe_read_usize(module_base + CS_MENU_MAN_GLOBAL_RVA) }.unwrap_or(null);
            if cs_menu_man != null && unsafe { is_heap_aligned_ptr(cs_menu_man) } {
                let dsm = (cs_menu_man + CS_MENU_MAN_DISABLE_SAVE_MENU_OFFSET) as *mut u8;
                let prev = unsafe { core::ptr::read_volatile(dsm) };
                if prev != 0 {
                    unsafe { core::ptr::write_volatile(dsm, 0) };
                    let n = SYSTEM_QUIT_DISABLE_SAVE_MENU_CLEAR_COUNT
                        .fetch_add(1, Ordering::SeqCst)
                        + 1;
                    if n <= 5 || n.is_multiple_of(120) {
                        append_autoload_debug(format_args!(
                            "system-quit-quickload: [game-task] cleared stale CSMenuMan->disableSaveMenu (was {prev}) #{n} at switch-2 stall (world_up=true bc4=1) -- native quit-save was gated OFF via ShouldSave/CanShowSaveMenu (+0x13c); now unblocked so bc4 pumps 1->2->3 and the world tears down"
                        ));
                    }
                }
            }
        }
        // b78 GUARD (companion to the disableSaveMenu clear above). Force b78 = -1 for the WHOLE time
        // the old world is up (through bc4 1 -> 2 -> 3), and only write the picked slot once the world has
        // torn down. The switch-2-fix proposal to narrow this to only `world_up && bc4 == 1` was NOT
        // applied on this branch: with the disableSaveMenu clear now letting bc4 advance to 2/3 while the
        // player is still briefly present, gating on `bc4 == 1` alone would let b78 = slot leak at bc4 2/3
        // and re-arm the very in-world MoveMapStep load the block comment above warns against (2026-07-01:
        // b78 = slot while in-world spun RequestLoadSlot 4600+ times and stuck the map machine "loading").
        // `world_up` is the correct invariant: b78 must be -1 until the world is gone, then = slot for the
        // clean-title autoload. So the force-(-1) condition stays keyed on world_up, unchanged from switch 1.
        // POST-COMMIT RE-LOAD LOOP FIX (2026-07-16, runtime-confirmed). Once this switch's picked slot has
        // COMMITTED its load (SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE=1 -- set by the feed/continue_confirm
        // at the clean-title stream, cleared to 0 only when a genuinely NEW switch arms at
        // system_quit_repro_guards.rs:864), STOP re-arming b78=slot. Proven from the gm-snap/setstate trace:
        // after the picked char loads and reaches stable in-world (ig_d8=2, menu_job populated), the world
        // momentarily drops to world_up=false and this guard RE-WROTE b78=slot -> a redundant in-world reload
        // that tears the freshly-loaded world down and bounces it back to title (menu_job->0 -> requestCode->0
        // -> SetState 6->2 -> our autoload re-drives -> loop = the soft-lock). Holding b78=-1 once committed
        // keeps the loaded world up. Inert before the feed (latch still 0 -> b78=slot fires the initial load
        // exactly as before) and reset for the next genuine pick, so switch 1's initial load is unchanged.
        // Runtime 6d7fdd89 proved any post-Continue write to GameMan+0xb78 is still the
        // native requested-slot load branch (`FUN_14067b2f0`) and re-enters the 0x67141a
        // crash path. Keep it disarmed here; do not use b78 as a progress mechanism.
        unsafe { *((gm + GAME_MAN_REQUESTED_SLOT_B78_OFFSET) as *mut i32) = OWN_STEPPER_SLOT_NONE };
        // CLEAR THE QUIT-SAVE REQUEST FLAGS DURING TEARDOWN (2026-07-16, Ghidra + runtime proven root).
        // The old world's MoveMapStep leaves its resident STEP_MoveMap(18) step only when the "ending
        // request" sub-machine (MoveMapStep.field25_0x12a, in FUN_140afa7c0, ticked by the MoveMap update
        // FUN_140aff730) walks case 0->..->7->8; case 8 calls FUN_140af9e80 to advance the parent step
        // (18->19->20->-1), which is what lets the InGameStep finish and the world tear down. The gate that
        // FREEZES it is case 7->8: it requires `ShouldSave() == false` AND `FUN_140679460() == false`.
        // `ShouldSave` (0x1406794c0) = saveRequested(b72) && !CanShowSaveMenu() && menu_gate && bc4!=3;
        // `FUN_140679460` = GameMan+0xb73 && menu_gate && bc4!=3. Our return-title REQUEST set b72 and b73
        // (it intends a quit-save); the native quit clears them BY SAVING, but we suppress the save (no save
        // on quit, by design), so they stay set -> ShouldSave/FUN_140679460 stay true -> case 7 hangs forever
        // -> child never finishes -> world never tears down = the "MOVE MAP 18/20" stall (runtime: the stuck
        // switch had b72=1 b73=1; the one that tore down had b72=0 b73=0). So clear both here every teardown
        // frame while the OLD world is up: this makes ShouldSave()/FUN_140679460() deterministically false,
        // unblocks the ending sub-machine, and is exactly the no-save-on-quit behavior we want (no disk
        // write; we are only dropping the request flags the game would otherwise satisfy via a save). Once
        // the world is gone (world_up=false) the block stops mattering; the post-commit dormancy latch and
        // continue_confirm own the incoming char's flags. GameMan+0xb73 is the byte right after b72.
        if world_up {
            unsafe {
                *((gm + GAME_MAN_SAVE_REQUESTED_B72_OFFSET) as *mut u8) = 0;
                *((gm + GAME_MAN_SAVE_REQUEST_COMPANION_B73_OFFSET) as *mut u8) = 0;
            }
            SYSTEM_QUIT_TEARDOWN_SAVEREQ_CLEAR_COUNT.fetch_add(1, Ordering::SeqCst);
        }
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
                "system-quit-quickload: requested save-slot load index world_up={world_up} kept gm_b78=-1 (read_back={requested_slot}) selected_slot={slot} phase={} bc4=0x{return_title_job_predicate_bc4:x}",
                SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
            ));
            // save-gate diag DURING the stall (bc4=1): the quit-save orchestrator FUN_140afb970 (RE 1.16.1)
            // skips the save unless force-latch 0x3d856a0 == 0, GameMan->save_state(+0xb80) == 0, AND the
            // menu gate FUN_14080d660 (*(CSMenuMan[+0x80])->0x290==0 && ->0x298==0). Names the blocker while
            // bc4 is frozen. Literals: 0x3d856a0 = load-active latch, 0x3d6b7b0 = CSMenuMan global.
            let dsg_force = unsafe { safe_read_u8(module_base + 0x3d856a0) }.unwrap_or(0xff);
            let dsg_ss = unsafe { safe_read_i32(gm + 0xb80) }.unwrap_or(-1);
            let dsg_csm = unsafe { safe_read_usize(module_base + 0x3d6b7b0) }.unwrap_or(0);
            let dsg_sub = if dsg_csm > 0x10000 {
                unsafe { safe_read_usize(dsg_csm + 0x80) }.unwrap_or(0)
            } else {
                0
            };
            let (dsg_m290, dsg_m298) = if dsg_sub > 0x10000 {
                (
                    unsafe { safe_read_u8(dsg_sub + 0x290) }.unwrap_or(0xff),
                    unsafe { safe_read_usize(dsg_sub + 0x298) }.unwrap_or(usize::MAX),
                )
            } else {
                (0xff, usize::MAX)
            };
            let dsg_menu_ok = dsg_m290 == 0 && dsg_m298 == 0;
            // The REAL orchestrator (FUN_140afb970) pump gate is bVar5 (NOT what this diag checked before):
            // bVar5 = ShouldSave() [saveRequested(b72) && !CanShowSaveMenu() && menu_gate && bc4!=3]
            //      || FUN_140679460() [b73 && menu_gate && bc4!=3]  ||  GetRequestedSaveSlotLoad()(b78) != -1.
            // If bVar5==0 the orchestrator returns without pumping bc4. b78==-1 is likely OUR b78 guard
            // starving the third term. Read the components so the stall names the exact failing term.
            let dsg_b72 = unsafe { safe_read_u8(gm + 0xb72) }.unwrap_or(0xff);
            let dsg_b73 = unsafe { safe_read_u8(gm + 0xb73) }.unwrap_or(0xff);
            let dsg_b78 = unsafe { safe_read_i32(gm + 0xb78) }.unwrap_or(-99);
            let bc4_not3 = return_title_job_predicate_bc4 != 3;
            let dsg_should_save = dsg_b72 != 0 && dsg_menu_ok && bc4_not3;
            let dsg_679460 = dsg_b73 != 0 && dsg_menu_ok && bc4_not3;
            let bvar5_est = dsg_should_save || dsg_679460 || dsg_b78 != -1;
            // The REAL bc4 blocker (deeper than bVar5): the quit-save FUN_14067ba30 does the full
            // bc4-advancing save ONLY if saveSlot < 10 AND disableSaveMenu == 0; otherwise it FALLS BACK
            // to a plain save (FUN_14067b660) that writes disk but never advances bc4 (and clears
            // saveRequested). CanShowSaveMenu() == (CSMenuMan->disableSaveMenu != 0). Read both.
            let dsg_slot = unsafe {
                safe_read_i32(gm + core::mem::offset_of!(eldenring::cs::GameMan, save_slot))
            }
            .unwrap_or(-99);
            let dsg_dsm = if dsg_csm > 0x10000 {
                unsafe { safe_read_u8(dsg_csm + CS_MENU_MAN_DISABLE_SAVE_MENU_OFFSET) }
                    .map(|v| v as i32)
                    .unwrap_or(-1)
            } else {
                -1
            };
            let pump_fallback = !(0..=9).contains(&dsg_slot) || dsg_dsm != 0;
            let dsg_blocker = if pump_fallback {
                "QUIT-SAVE FALLBACK(saveSlot>=10 or disableSaveMenu!=0 -> plain save, bc4 NOT advanced)"
            } else if dsg_force != 0 {
                "FORCE_LATCH(0x143d856a0)"
            } else if dsg_ss != 0 {
                "save_state"
            } else if !dsg_menu_ok {
                "MENU_GATE(ProfileSelect)"
            } else if !bvar5_est {
                "bVar5=0(no save cond: b72/b73 off AND b78==-1 -- b78 likely OUR guard starving the pump)"
            } else {
                "NONE(bVar5 est OK but bc4 stuck -- CanShowSaveMenu()==true killing ShouldSave? recheck)"
            };
            append_autoload_debug(format_args!(
                "save-gate-diag(stall): force=0x{dsg_force:x} save_state={dsg_ss} bc4=0x{return_title_job_predicate_bc4:x} menu_gate_ok={dsg_menu_ok} b72={dsg_b72} b73={dsg_b73} b78={dsg_b78} bVar5_est={bvar5_est} saveSlot={dsg_slot} disableSaveMenu={dsg_dsm} pump_fallback={pump_fallback} -> blocked_by={dsg_blocker}"
            ));
            // CASE-7 7->8 GATE, computed EXACTLY from the decompiled formulas (FUN_140afa7c0 case 7,
            // bd CORRECTED-load2-substate7-NOT-save-drain-saving-disabled-shouldsave-structurally-false-2026-07-20).
            // Advance needs ALL of: c1 FUN_14067a170[saveState b80==0], c2 !ShouldSave, c3 !FUN_140679460,
            // c4 FUN_140a9ceb0(CSRemo) [historically PASSING]. ShouldSave = b72 && !CanShowSaveMenu()
            // && menu_gate && bc4!=3, and !CanShowSaveMenu()==(disableSaveMenu==0). Since saving is
            // DISABLED BY DESIGN (disableSaveMenu!=0), ShouldSave is STRUCTURALLY FALSE => c2 passes and
            // b72 is irrelevant. This line names whether C1 (saveState) or C3 (b73) is the real blocker,
            // no game-function calls (zero side-effect risk). All inputs already read above.
            let bc4_not3_c = return_title_job_predicate_bc4 != 3;
            let c1_savestate0 = dsg_ss == 0;
            let shouldsave = dsg_b72 != 0 && dsg_dsm == 0 && dsg_menu_ok && bc4_not3_c;
            let fun679460 = dsg_b73 != 0 && dsg_menu_ok && bc4_not3_c;
            let c2_not_shouldsave = !shouldsave;
            let c3_not_679460 = !fun679460;
            let case7_blocker = if !c1_savestate0 {
                "C1 saveState(b80)!=0"
            } else if !c2_not_shouldsave {
                "C2 ShouldSave==true (unexpected: saving-disabled should force it false)"
            } else if !c3_not_679460 {
                "C3 FUN_140679460==true (b73 && menu_gate && bc4!=3)"
            } else {
                "C1-3 pass -> C4 CSRemo (FUN_140a9ceb0) or advancer not ticking"
            };
            append_autoload_debug(format_args!(
                "case7-gate(4-bool): c1_savestate0={c1_savestate0} c2_not_shouldsave={c2_not_shouldsave}(shouldsave={shouldsave}) c3_not_679460={c3_not_679460}(f679460={fun679460}) [b80={dsg_ss} b72={dsg_b72} b73={dsg_b73} disableSaveMenu={dsg_dsm} menu_gate_ok={dsg_menu_ok} bc4!=3={bc4_not3_c}] -> case7_blocker={case7_blocker}"
            ));
        }
    }
    let post_continue_handoff_active = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        >= SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF
        && SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst) != 0;
    if post_continue_handoff_active
        && SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.load(Ordering::SeqCst) > 0
        && return_title_job_predicate_bc4 == GAME_MAN_RETURN_TITLE_JOB_PREDICATE_READY
        && tick % OWN_STEPPER_LOG_INTERVAL == null as u64
    {
        append_autoload_debug(format_args!(
            "system-quit-quickload: suppressing return-title final-functor retry during post-Continue SetState5/MoveMap handoff (bc4=0x{return_title_job_predicate_bc4:x}); reload stream owns the session until phase IDLE"
        ));
    }
    if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        >= SYSTEM_QUIT_QUICKLOAD_PHASE_TITLE_OWNER_SEEN
        && !post_continue_handoff_active
        && SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.load(Ordering::SeqCst) > 0
        && return_title_job_predicate_bc4 == GAME_MAN_RETURN_TITLE_JOB_PREDICATE_READY
        && SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        // The return-title save has finished writing the ACTIVE slot into the active file (bc4 is
        // terminal only after save_state returned to 0). Re-commit the foreign candidate now so a
        // SAME-SLOT switch's fresh deserialize reads the picked character instead of the clobbered
        // active one (see system_quit_save_swap_recommit_after_return_title_save).
        system_quit_save_swap_recommit_after_return_title_save();
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
        // Direct-file save sources own FULLREAD_PHASE via the native full-read chain (which reads the
        // staged save itself); let it run its own GUARD/COMMIT, not the Continue-item guard below.
        // This covers both the missing-save picker and explicit loose `save_file` config.
        if direct_save_file_source_active() {
            unsafe { native_fullread_tick(owner, module_base, tick) };
            return true;
        }
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
    // MENU-FREE SWITCH RELOAD (2026-07-18, RE workflow + bd live-switch-teardown-fixed-now-menu-open-stall).
    // The genuine System->Quit->Load-Profile switch tears the old world down cleanly (ending-request
    // recovery), but the warm-rebuilt TitleTopDialog never reaches Loop (press-start SceneObjProxy at
    // dialog+0xb78 unbound post-return-title), so product_core_autoload_ready below returns None forever
    // and the native accept-byte/open-menu path deadlocks; native_fullread_tick also stands down for a
    // switch and its direct-file call sites are inactive on the default save. Drive the picked slot
    // through the menu-free native-ownership commit (same final SetState5 as the boot load, fed from our
    // own disk bytes). Fires ONLY for a genuine in-world switch at a clean title, NEVER the boot autoload
    // or the spurious boot self-reload -- four independent discriminators, any one of which excludes boot:
    //   * QUICKLOAD_PHASE >= RETURN_TITLE_REQUESTED  -- a switch is in progress (boot=IDLE)
    //   * ARM_PLAYER_WAS_ABSENT == 0                 -- GENUINE switch (armed in-world), not the spurious
    //                                                   boot self-reload (armed while player absent -> 1)
    //   * player ABSENT now                          -- old world torn down (clean title): makes the
    //                                                   gaitem reset safe, never deser into a live world
    //   * picked slot in 0..TITLE_PROFILE_SLOT_COUNT -- a real profile slot (boot=usize::MAX)
    //   * FRESH_DESER_DONE == 0                      -- not already committed this switch
    let picked = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
    if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        >= SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
        && SYSTEM_QUIT_ARM_PLAYER_WAS_ABSENT.load(Ordering::SeqCst) == 0
        && picked < TITLE_PROFILE_SLOT_COUNT
        && gm != null
        && SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst) == 0
        && unsafe { PlayerIns::local_player_mut() }.is_err()
        // CLEAN-A/B: skip the menu-free switch-reload so the harness's menu-driven Continue is the sole
        // reload path -- isolates menu-free vs menu-driven under identical epoch1 (bd STEP4-RUNTIME-TRACE).
        && !crate::compat::gating::switch_reload_ownload_disabled()
        && unsafe {
            crate::compat::own_load::own_load_switch_reload_fire(
                module_base,
                gm,
                owner,
                picked as i32,
                tick,
            )
        }
    {
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
        // Deliver the product accept latch in the same TitleTopDialog::update frame that consumes
        // it. The game-task write can be cleared by other menu handlers before update reaches its
        // tail, leaving a40 at zero indefinitely despite a core-ready title.
        unsafe { install_title_update_hook(module_base) };
        unsafe { maybe_hide_title_press_start(module_base, &ready) };
        unsafe { maybe_hide_title_logo_surface(module_base, &ready) };
        // `TitleTopDialog::open_menu` sets a40 only while it assembles and drains its
        // native job chain.  The follow-up update returns to a40==0 even though the
        // menu transition was accepted.  Re-arming the accept byte then restarts that
        // transition forever and starves the semantic Continue row.  Once the
        // one-way own latch observed the native a40 edge, wait for that row instead.
        if ready.menu_opened_latch == OWN_STEPPER_MENU_OPENED_NO
            && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) == OWN_STEPPER_MENU_OPENED_NO
        {
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
            if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: PRESS BUTTON component ready; armed/retried native title accept byte for in-update open-menu/drain (dialog=0x{:x} press_start_proxy=0x{:x}) -- waiting for native a40/menu-open latch before declaring menu opened",
                    ready.title_dialog, ready.press_start_proxy
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
                "product-core-autoload: native title menu-open latch observed (a40={}) after accept-byte arm; Continue rows may now be driven",
                ready.menu_opened_latch
            ));
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
        if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
            >= SYSTEM_QUIT_QUICKLOAD_PHASE_TITLE_OWNER_SEEN
            && gm != null
            && slot >= OWN_STEPPER_SLOT_ZERO
        {
            let requested_slot = unsafe { safe_read_i32(gm + GAME_MAN_REQUESTED_SLOT_B78_OFFSET) }
                .unwrap_or(OWN_STEPPER_SLOT_NONE);
            if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
                append_autoload_debug(format_args!(
                    "system-quit-quickload: leaving requested save-slot load index untouched before Continue selected_slot={slot} gm_b78={requested_slot} -- SetState5 handoff owns reload; phase-4 task must not re-arm b78"
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
        let current_switch_phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
        if current_switch_phase != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE
            && unsafe { PlayerIns::local_player_mut() }.is_ok()
        {
            if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: SWITCH holding native Continue driving until old world torn down -- local player still present slot={slot} tick={tick}"
                ));
            }
            return true;
        }
        if current_switch_phase >= SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF
            && SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst) != 0
        {
            if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: SWITCH post-Continue handoff waiting for phase-IDLE stable-world proof -- phase={current_switch_phase} slot={slot}; not driving another Continue"
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
            // The three CSGaitemImp hook-disable calls that used to live here are gone with the hooks
            // themselves: install_system_quit_gaitem_{finalize,lookup,deserialize}_hook had no callers,
            // so rustc never codegen'd them, the INSTALLED latches were never set, and each disable was
            // an unconditional early return. bd er-effects-rs-57fw.
        }
        // Direct-file save source: the "native Continue row" product_continue waits for can be stale
        // or backed by an empty ProfileSummary. Picker path already used this bypass; explicit loose
        // `save_file` needs the same verified native full-read chain. It marks the slot occupied (so
        // the save-load gate 0x14067b200 accepts it), reads the staged save itself
        // (submit/drain/deserialize), and commits (continue_confirm -> SetState5) into the redirected
        // staged save. The user's original source save remains read-only.
        if direct_save_file_source_active() {
            unsafe { native_fullread_tick(owner, module_base, tick) };
            return true;
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
