/// DEFAULT-OFF ProfileSelect load flow (gate: `profile_select_load_flow_enabled`). Runs only in
/// PHASE_MENU after menu-open (a40==1). Distinct from the PROVEN native Continue commit: it renders
/// the loaded character's profile portrait (for the now-loading screen) by firing the title menu's
/// Load-Game row to open a LIVE `ProfileLoadDialog` -- the only render context in which the profile
/// renderer refresh's per-slot gate (`ProfileSummary->saveSlotsStates[slot]`) is satisfied, so the
/// portrait actually renders (it never does at the bare main menu) -- holds the load-commit until the
/// portrait has rendered + been captured, then drives the SAME STAGE2 commit (load_activate ->
/// selector -> continue_confirm/SetState5) the Continue path's STAGE2 uses. Fail-open: commits after
/// `PORTRAIT_HOLD_MAX_TICKS` regardless of capture, so the char-load can never be permanently blocked.
///
/// State is derived from existing latches: `OWN_STEPPER_TITLE_FIRED` (Load-Game row fired) and
/// `OWN_STEPPER_DIALOG` (the live ProfileLoadDialog, latched by `cap_dialog_factory_hook` once the
/// native factory builds it -- that hook defers its STAGE2 transition to us under this gate).
unsafe fn product_profile_select_load_flow(owner: usize, base: usize, slot: i32, tick: u64) {
    const PORTRAIT_HOLD_LOG_INTERVAL: usize = 30;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // (a) Fire the Load-Game row ONCE -> opens the live ProfileLoadDialog (factory hook latches it).
    if OWN_STEPPER_TITLE_FIRED.load(Ordering::SeqCst) == null {
        let Some(action) = (unsafe { title_menu_action_ready(owner, base) }) else {
            if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
                append_autoload_debug(format_args!(
                    "profile-select-flow: waiting for native Load-Game MenuMemberFuncJob row owner=0x{owner:x} slot={slot} tick={tick}"
                ));
            }
            return;
        };
        unsafe { fire_product_title_load_action(action, base, tick, slot) };
        return;
    }
    // (b) Wait for the factory hook to latch the live ProfileLoadDialog.
    let dialog = OWN_STEPPER_DIALOG.load(Ordering::SeqCst);
    if dialog == null {
        if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
            append_autoload_debug(format_args!(
                "profile-select-flow: Load-Game fired; waiting for ProfileLoadDialog factory-hook capture (OWN_STEPPER_DIALOG) slot={slot} tick={tick}"
            ));
        }
        return;
    }
    // (b cont.) PORTRAIT HOLD: re-kick the refresh each frame (idempotent per-slot via +0x754) while
    // the ProfileLoadDialog is open, capture table[slot]'s rendered portrait, and HOLD the commit
    // until captured or the tick cap. Fail-open at the cap.
    if PORTRAIT_RENDER_WINDOW_DONE.load(Ordering::SeqCst) == 0 {
        let refresh: unsafe extern "system" fn() =
            unsafe { std::mem::transmute(base + PROFILE_RENDERER_REFRESH_RVA) };
        unsafe { refresh() };
        if PROFILE_REFRESH_KICKED.swap(1, Ordering::SeqCst) == 0 {
            append_autoload_debug(format_args!(
                "profile-select-flow: kicked profile refresh 0x{:x} with ProfileLoadDialog=0x{dialog:x} open (slot={slot}) -- saveSlotsStates[slot] now set, portrait can render",
                base + PROFILE_RENDERER_REFRESH_RVA
            ));
        }
        maybe_capture_portrait_gxtexture(base, slot);
        let captured = LOADING_BG_PORTRAIT_GX_KEPT.load(Ordering::SeqCst) != 0;
        let waited = PORTRAIT_HOLD_WAIT_TICKS.fetch_add(1, Ordering::SeqCst) + 1;
        if !captured && waited < PORTRAIT_HOLD_MAX_TICKS {
            if waited % PORTRAIT_HOLD_LOG_INTERVAL == 1 {
                append_autoload_debug(format_args!(
                    "profile-select-flow: holding load-commit for portrait render (captured={captured} waited={waited}/{PORTRAIT_HOLD_MAX_TICKS} dialog=0x{dialog:x} slot={slot})"
                ));
            }
            return;
        }
        PORTRAIT_RENDER_WINDOW_DONE.store(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "profile-select-flow: portrait window release -> commit (captured={captured} waited={waited} dialog=0x{dialog:x} slot={slot})"
        ));
    }
    // (c) COMMIT: hand the latched ProfileLoadDialog to the existing STAGE2 dispatch (it reads
    // OWN_STEPPER_DIALOG). The next product_core_autoload_tick frame runs own_stepper_stage2.
    if OWN_STEPPER_PHASE.load(Ordering::SeqCst) == OWN_STEPPER_PHASE_MENU {
        own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_ACTIVATE);
        append_autoload_debug(format_args!(
            "profile-select-flow: COMMIT -> STAGE2 ACTIVATE dialog=0x{dialog:x} slot={slot} tick={tick}"
        ));
    }
}
pub(crate) unsafe fn title_menu_action_ready(owner: usize, base: usize) -> Option<MenuActionNode> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
    if dialog == null {
        return None;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(null);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return None;
    }
    let registry =
        unsafe { safe_read_usize(dialog + DIALOG_ROW_REGISTRY_A48_OFFSET) }.unwrap_or(null);
    // In the native profile-capture direct-open path this slot can be a live heap/registry value
    // instead of an image vtable-shaped value. Treat it as provenance, not as a pre-scan hard gate:
    // the bounded scanner below still validates the actual MenuMemberFuncJob vtable/member_fn chain
    // before returning anything fireable.
    let (member_node, window_item) = unsafe { scan_dialog_for_loadgame(owner, base) };
    let node = member_node?;
    let node_vt = unsafe { safe_read_usize(node) }.unwrap_or(null);
    if node_vt != base + MEMBERFUNCJOB_VTABLE_RVA {
        return None;
    }
    const MEMBER_DIALOG_10: usize = core::mem::size_of::<usize>() + core::mem::size_of::<usize>();
    const MEMBER_FN_18: usize = 0x18;
    const MEMBER_ADJ_20: usize = 0x20;
    const JMP_HOPS: usize = 6;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    let member_dialog = unsafe { safe_read_usize(node + MEMBER_DIALOG_10) }.unwrap_or(null);
    let member_fn = unsafe { safe_read_usize(node + MEMBER_FN_18) }.unwrap_or(null);
    let member_adjust = unsafe { safe_read_usize(node + MEMBER_ADJ_20) }.unwrap_or(null);
    if member_fn == null {
        return None;
    }
    let factory_abs = base + LIVE_DIALOG_FACTORY_RVA;
    let mut target = member_fn;
    let mut hop = HOP_START;
    while hop < JMP_HOPS && target != null {
        if target == factory_abs {
            return Some(MenuActionNode {
                node,
                node_vt,
                registry,
                member_dialog,
                member_fn,
                member_adjust,
                window_item: window_item.unwrap_or(null),
            });
        }
        match unsafe { decode_thunk_hop(target) } {
            Some(next) => target = next,
            None => break,
        }
        hop += HOP_STEP;
    }
    None
}
pub(crate) unsafe fn title_live_dialog_fire_ready(
    owner: usize,
    base: usize,
) -> Option<LiveDialogFireReady> {
    const TITLE_FLOW_CONTEXT_VTABLE_RVA: usize = 0x2ac7f20;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if !unsafe { title_scheduler_ready(owner, base) } {
        return None;
    }
    let title_dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
    if title_dialog == null {
        return None;
    }
    let title_dialog_vt = unsafe { safe_read_usize(title_dialog) }.unwrap_or(null);
    if title_dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return None;
    }
    let menu_opened_latch = unsafe {
        safe_read_usize(title_dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET)
            .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
            .unwrap_or(null)
    };
    if menu_opened_latch == OWN_STEPPER_MENU_OPENED_NO {
        return None;
    }
    let registry_vt =
        unsafe { safe_read_usize(title_dialog + DIALOG_ROW_REGISTRY_A48_OFFSET) }.unwrap_or(null);
    if registry_vt != base + SCENE_OBJ_PROXY_VTABLE_RVA {
        return None;
    }
    let capture_slot = title_dialog + DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET;
    let capture = unsafe { safe_read_usize(capture_slot) }.unwrap_or(null);
    if !unsafe { is_heap_aligned_ptr(capture) } {
        return None;
    }
    let capture_vt = unsafe { safe_read_usize(capture) }.unwrap_or(null);
    if capture_vt != base + TITLE_FLOW_CONTEXT_VTABLE_RVA {
        return None;
    }
    let menu_window = LATCHED_MENU_WINDOW.load(Ordering::SeqCst);
    if !unsafe { is_heap_aligned_ptr(menu_window) } {
        return None;
    }
    let menu_window_vt = unsafe { safe_read_usize(menu_window) }.unwrap_or(null);
    Some(LiveDialogFireReady {
        title_dialog,
        title_dialog_vt,
        capture_slot,
        capture,
        capture_vt,
        registry_vt,
        menu_opened_latch,
        menu_window,
        menu_window_vt,
    })
}
/// True if `vt` is a startup MessageBoxDialog the auto-accept should drive: the base MessageBoxDialog
/// vtable OR the CS::SaveRetryDialog subclass vtable (the wrapper 0x1407af9a0 overrides base ->
/// SaveRetryDialog AFTER the builder, so a base-only check bails once the override lands). bd
/// offline-title-modal-is-saveretrydialog.
pub(crate) fn is_startup_msgbox_vtable(vt: usize, base: usize) -> bool {
    vt == base + MSGBOX_DIALOG_VTABLE_RVA || vt == base + SAVE_RETRY_DIALOG_VTABLE_RVA
}
pub(crate) fn startup_modal_blocking_state() -> StartupModalBlockingState {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog = CONNECTION_ERROR_DIALOG.load(Ordering::SeqCst);
    if dialog == null {
        return StartupModalBlockingState::Clear;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != null {
            own
        } else {
            game_module_base().unwrap_or(null)
        }
    };
    let vt = unsafe { safe_read_usize(dialog) }.unwrap_or(null);
    if base == null || !is_startup_msgbox_vtable(vt, base) {
        CONNECTION_ERROR_DIALOG.store(null, Ordering::SeqCst);
        return StartupModalBlockingState::Clear;
    }
    let closing = unsafe { safe_read_usize(dialog + MSGBOX_CLOSING_LATCH_3B0_OFFSET) }
        .map(|v| v & MSGBOX_LATCH_BYTE_MASK)
        .unwrap_or(MSGBOX_CLOSING_YES);
    if closing == MSGBOX_CLOSING_YES {
        CONNECTION_ERROR_DIALOG.store(null, Ordering::SeqCst);
        return StartupModalBlockingState::Clear;
    }
    StartupModalBlockingState::Blocking {
        dialog,
        vtable: vt,
        closing_latch: closing,
    }
}
pub(crate) unsafe fn profile_load_dialog_ready(
    base: usize,
    dialog: usize,
    want_slot: i32,
    log_pending: bool,
) -> Option<ProfileLoadDialogReady> {
    const PROFILE_LOAD_ACTIVATE_RVA: usize = 0x009a4670;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let pld_vt = base + PROFILE_LOAD_DIALOG_VTABLE_RVA;
    let dvt = if dialog != null {
        unsafe { safe_read_usize(dialog) }.unwrap_or(null)
    } else {
        null
    };
    if dvt != pld_vt {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: waiting for ProfileLoadDialog dialog=0x{dialog:x} vt=0x{dvt:x} want=0x{pld_vt:x}"
            ));
        }
        return None;
    }
    let lav =
        unsafe { safe_read_usize(dvt + DIALOG_LOAD_ACTIVATE_VTSLOT_A0_OFFSET) }.unwrap_or(null);
    if lav != base + PROFILE_LOAD_ACTIVATE_RVA {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: load_activate slot not ready lav=0x{lav:x} want=0x{:x} dvt=0x{dvt:x}",
                base + PROFILE_LOAD_ACTIVATE_RVA
            ));
        }
        return None;
    }
    let gdm = game_data_man_ptr_or_null();
    let player_game_data = if gdm != null {
        unsafe { safe_read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }.unwrap_or(null)
    } else {
        null
    };
    if player_game_data == null {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: PlayerGameData null gdm=0x{gdm:x} -- load_activate would assert"
            ));
        }
        return None;
    }
    let bound = unsafe { safe_read_i32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET) }
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    let cursor_now = unsafe { safe_read_i32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    let expected_slot = if want_slot == OWN_STEPPER_SLOT_NONE {
        cursor_now
    } else {
        want_slot
    };
    let cursor_target = if want_slot == OWN_STEPPER_SLOT_NONE {
        cursor_now
    } else if bound == OWN_STEPPER_CALL_INC as i32 {
        OWN_STEPPER_SLOT_ZERO
    } else {
        want_slot
    };
    if expected_slot < OWN_STEPPER_SLOT_ZERO
        || bound <= OWN_STEPPER_SLOT_ZERO
        || cursor_target < OWN_STEPPER_SLOT_ZERO
        || cursor_target >= bound
    {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: slot rows not ready/valid want={want_slot} expected={expected_slot} cursor_target={cursor_target} cursor={cursor_now} bound={bound} dialog=0x{dialog:x}"
            ));
        }
        return None;
    }
    let load_job_ctx = unsafe {
        safe_read_usize(dialog + core::mem::offset_of!(ProfileLoadDialogLayout, load_job_ctx))
    }
    .unwrap_or(null);
    if !unsafe { is_heap_aligned_ptr(load_job_ctx) } {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: load_job_ctx not ready dialog=0x{dialog:x} ctx=0x{load_job_ctx:x}"
            ));
        }
        return None;
    }
    let load_job_ctx_vt = unsafe { safe_read_usize(load_job_ctx) }.unwrap_or(null);
    if !vtable_in_game_image(load_job_ctx_vt, base) {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: load_job_ctx vtable invalid ctx=0x{load_job_ctx:x} vt=0x{load_job_ctx_vt:x} base=0x{base:x}"
            ));
        }
        return None;
    }
    Some(ProfileLoadDialogReady {
        dialog,
        dvt,
        bound,
        cursor_now,
        cursor_target,
        expected_slot,
        load_activate: lav,
        load_job_ctx,
        load_job_ctx_vt,
        player_game_data,
    })
}
/// Pure read-only observation (NO forcing, NO SetState) of the title -> menu -> load
/// transition. Logs a full snapshot every OBSERVE_INTERVAL ticks so we can capture
/// exactly what the REAL button press does: the title state sequence, when CSFeMan /
/// session build, when the save mounts (GameMan+0xc30 changes from the default), the
/// InGameStep/MoveMapStep appearance. Ground-truths the menu-build the static RE
/// kept mis-identifying.
pub(crate) unsafe fn title_observe_tick(module_base: usize, tick: u64) {
    let _ = OBSERVE_INTERVAL;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let owner = unsafe { title_owner(module_base) }.map(|p| p as usize);
    let state = match owner {
        Some(o) => unsafe { *((o + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) },
        None => TITLE_STATE_OWNER_GONE,
    };
    // Title->menu timing baseline (works for BOTH a true-vanilla user run and the DLL run):
    // T0 = first frame parked at the title (state 10); T_menu_open = when the TitleTopDialog SM
    // reaches TextFadeOut (menu open -- by the user's presses+modal-dismissals in vanilla). The
    // delta is the apples-to-apples title->ready-menu time to compare against the DLL's headless
    // 3.1s. Read-only (is_in_state is a pure state query).
    if state == TITLE_STEP_MENU_JOB_WAIT
        && owner.is_some()
        && OBSERVE_T0_EMITTED.swap(OBSERVE_MARKER_EMITTED, Ordering::SeqCst)
            == OBSERVE_MARKER_NOT_EMITTED
    {
        timeline_event("T0", tick, format_args!("state10 observe-baseline"));
    }
    if let Some(o) = owner {
        if OBSERVE_MENU_OPEN_EMITTED.load(Ordering::SeqCst) == OBSERVE_MARKER_NOT_EMITTED {
            let dialog =
                unsafe { safe_read_usize(o + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
            let dialog_vt = if dialog != null {
                unsafe { safe_read_usize(dialog) }.unwrap_or(null)
            } else {
                null
            };
            if dialog_vt == module_base + TITLE_TOP_DIALOG_VTABLE_RVA {
                let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
                let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
                    unsafe { std::mem::transmute(module_base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
                let textfadeout =
                    unsafe { is_in_state(sm, module_base + TITLE_STATE_DESC_TEXTFADEOUT_RVA) }
                        != OWN_STEPPER_FALSE;
                if textfadeout
                    && OBSERVE_MENU_OPEN_EMITTED.swap(OBSERVE_MARKER_EMITTED, Ordering::SeqCst)
                        == OBSERVE_MARKER_NOT_EMITTED
                {
                    timeline_event(
                        "T_menu_open",
                        tick,
                        format_args!("dialog=0x{dialog:x} observe-baseline"),
                    );
                }
            }
        }
    }
    let csfeman = cs_fe_man_ptr_or_null();
    let session = unsafe { *((module_base + SESSION_SINGLETON_RVA) as *const usize) };
    let gm = game_man_ptr_or_null();
    let read_gm = |off: usize| {
        if gm != null {
            unsafe { *((gm + off) as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        }
    };
    let c30 = read_gm(GAME_MAN_SAVED_MAP_C30_OFFSET);
    let ac0 = read_gm(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
    let b80 = read_gm(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
    let b78 = read_gm(GAME_MAN_REQUESTED_SLOT_B78_OFFSET);
    // Frame-level save-IO orchestration capture (menu-b80-mount-orchestration-sequence):
    // the iodev request handle pair [iodev+0x18]/[iodev+0x20] + [iodev+0x10] inflight.
    // Only 0x14067b4e0's preview read populates these; logging them across a real
    // load pins EXACTLY when the read goes in-flight/resident vs when b80 flips.
    let iodev = unsafe { *((module_base + IODEV_GLOBAL_RVA) as *const usize) };
    let read_iodev = |off: usize| {
        if iodev != null {
            unsafe { *((iodev + off) as *const usize) }
        } else {
            null
        }
    };
    let iodev10 = read_iodev(IODEV_INFLIGHT_10_OFFSET);
    let iodev18 = read_iodev(IODEV_REQHANDLE_18_OFFSET);
    let iodev20 = read_iodev(IODEV_REQHANDLE_20_OFFSET);
    let ingame = match owner {
        Some(o) => unsafe { *((o + TITLE_OWNER_JOB_OFFSET) as *const usize) },
        None => null,
    };
    let mms = if ingame != null {
        unsafe { *((ingame + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) as *const usize) }
    } else {
        null
    };
    let mms_state = if mms != null {
        unsafe { *((mms + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) }
    } else {
        TITLE_STATE_OWNER_GONE
    };
    let slotmgr = game_data_man_ptr_or_null();
    // World-resource streaming enable-state (the WorldResWait resolution gate):
    // resmgr = deref(deref(MoveMapStep+0xf0)+0x10); b7c1 = its streaming-enable flag;
    // driver = the streaming/session driver singleton 0x143d7c088. Capture what the
    // REAL load has enabled during mms_state=3 that our forced load lacks.
    let wrm = if mms != null {
        unsafe { *((mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET) as *const usize) }
    } else {
        null
    };
    let resmgr = if wrm != null {
        unsafe { *((wrm + WORLDRES_RESMGR_10_OFFSET) as *const usize) }
    } else {
        null
    };
    let b7c1 = if resmgr != null {
        unsafe { *((resmgr + RESMGR_STREAM_ENABLE_B7C1_OFFSET) as *const u8) as i32 }
    } else {
        TITLE_STATE_OWNER_GONE
    };
    let driver = unsafe { *((module_base + STREAMING_DRIVER_SINGLETON_RVA) as *const usize) };
    // Change-detection: only log when the signature changes (full granularity, no
    // per-frame file I/O). Captures every transition incl. the mms_state 3 -> resolve.
    let csf_nz = (csfeman != null) as i64;
    let sess_nz = (session != null) as i64;
    let ingame_nz = (ingame != null) as i64;
    let driver_nz = (driver != null) as i64;
    let mut sig = state as i64;
    sig = sig
        .wrapping_mul(OBSERVE_SIG_MULT)
        .wrapping_add(mms_state as i64);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(csf_nz);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(sess_nz);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(ingame_nz);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(c30 as i64);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(b80 as i64);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(ac0 as i64);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(b7c1 as i64);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(driver_nz);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(b78 as i64);
    sig = sig
        .wrapping_mul(OBSERVE_SIG_MULT)
        .wrapping_add((iodev10 != null) as i64);
    sig = sig
        .wrapping_mul(OBSERVE_SIG_MULT)
        .wrapping_add((iodev18 != null) as i64);
    sig = sig
        .wrapping_mul(OBSERVE_SIG_MULT)
        .wrapping_add((iodev20 != null) as i64);
    if OBSERVE_LAST_SIG.swap(sig, Ordering::SeqCst) == sig {
        return;
    }
    append_autoload_debug(format_args!(
        "observe: state={state} csfeman=0x{csfeman:x} session=0x{session:x} c30=0x{c30:x} ac0={ac0} b80={b80} b78={b78} iodev=0x{iodev:x} io10=0x{iodev10:x} io18=0x{iodev18:x} io20=0x{iodev20:x} mms=0x{mms:x} mms_state={mms_state} resmgr=0x{resmgr:x} b7c1={b7c1} driver=0x{driver:x} slotmgr=0x{slotmgr:x} tick={tick}"
    ));
}
/// Patch the `GameMan::IsOnlineMode` getter 0x14067a030 to `xor eax,eax; ret` so it always
/// reports OFFLINE. Validates the expected first opcode byte (aborts if the binary differs),
/// VirtualProtects the 3-byte stub region RWX, writes the stub, restores protection, and
/// flushes the instruction cache. Spawned early at DLL attach (timing-independent: it changes
/// what the function RETURNS, not a data field, so it works whether GameMan is constructed yet
/// or not). Mirrors `apply_splash_skip`. Equivalent to the player choosing "Play Offline" --
/// no save access, no struct mutation, no crash risk.
pub(crate) fn apply_online_disable() {
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!("online-disable: module base unavailable"));
        return;
    };
    // Patch the IsOnlineMode getter (consumers read offline). NOTE: the login-readiness predicate
    // patch (0x140cab230) was REVERTED -- it did not prevent the modal (the offline fork shows it
    // too) AND it broke the OnDecide OK-dispatch (the modal stuck instead of proceeding).
    apply_xor_ret_stub(base, ONLINE_DISABLE_RVA, "IsOnlineMode getter");
    // The THIRD menu-open popup ("Starting in offline mode", GR_System_Message 401170) is gated by
    // TitleFlowContext->notReleaseFlag55 = !Menu_IsEnableOnlineMode(). Force that getter false so the
    // game's own ctx-init (0x14082d0d0) writes notReleaseFlag55=1 each time, the title-flow offline step
    // (0x14082fda0) takes the clean no-popup branch, and the Continue/Load/NewGame rows build with ZERO
    // MessageBoxDialog builds. Race-free + offline-gated (Seamless online unaffected). bd
    // menu-open-3rd-popup-offline-mode-notice-2026-06-23 / er-effects-rs-yvf.
    let menu_online_off = patch_3byte_stub(
        base,
        MENU_ONLINE_MODE_DISABLE_RVA,
        MENU_ONLINE_MODE_EXPECTED_FIRST,
        ONLINE_DISABLE_STUB,
        "menu-online-mode-disable",
    );
    append_autoload_debug(format_args!(
        "online-disable: Menu_IsEnableOnlineMode@0x{:x} patched ok={menu_online_off} -> xor eax,eax;ret (notReleaseFlag55 becomes 1 -> no 'Starting in offline mode' popup -> title rows build)",
        base + MENU_ONLINE_MODE_DISABLE_RVA
    ));
    let _ = ONLINE_PREDICATE_DISABLE_RVA;
}
/// Force `CS::CSWindowImp::IsGameInForeground` (0x14266def0) to always return true (`mov al,1; ret`)
/// so the engine's flip pacer never applies the unfocused-window fps throttle -- the probe boots at
/// full speed regardless of focus (bd runtime-probe-unfocused-window-throttle). Same RWX/flush
/// pattern as the online-disable patch; validates the expected 0x40 prologue first.
pub(crate) fn apply_foreground_force() {
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!("foreground-force: module base unavailable"));
        return;
    };
    let target = (base + FOREGROUND_FORCE_RVA) as *mut u8;
    let existing = unsafe { *target };
    if existing != FOREGROUND_FORCE_EXPECTED_FIRST {
        append_autoload_debug(format_args!(
            "foreground-force: ABORT -- byte at 0x{:x} is 0x{existing:x}, expected 0x{FOREGROUND_FORCE_EXPECTED_FIRST:x}",
            base + FOREGROUND_FORCE_RVA
        ));
        return;
    }
    let mut old_protect = PAGE_PROTECT_UNSET;
    let protect_ok = unsafe {
        VirtualProtect(
            target as *mut c_void,
            ONLINE_DISABLE_PATCH_LEN,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if protect_ok == HOOK_FALSE_RETURN as i32 {
        append_autoload_debug(format_args!("foreground-force: VirtualProtect failed"));
        return;
    }
    let mut i = TITLE_OWNER_SCAN_START_ADDRESS;
    while i < ONLINE_DISABLE_PATCH_LEN {
        unsafe { *target.add(i) = FOREGROUND_FORCE_STUB[i] };
        i += ONLINE_DISABLE_BYTE_STEP;
    }
    let mut restored = PAGE_PROTECT_UNSET;
    unsafe {
        VirtualProtect(
            target as *mut c_void,
            ONLINE_DISABLE_PATCH_LEN,
            old_protect,
            &mut restored,
        )
    };
    unsafe {
        FlushInstructionCache(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            target as *const c_void,
            ONLINE_DISABLE_PATCH_LEN,
        )
    };
    append_autoload_debug(format_args!(
        "foreground-force: patched IsGameInForeground 0x{:x} -> mov al,1;ret (no unfocused fps throttle)",
        base + FOREGROUND_FORCE_RVA
    ));
}
/// Force the SaveLoad2 storage-select op gate to pass cold (bd b80-ROOTCAUSE-cold-no-user-signin):
/// patch the sign-in check to always return true and the user-index resolver to return 0, so the
/// select-op ctor (0x14240f1b0) builds the runnable and the load proceeds to SLLoadSession -> read
/// -> b80 RESIDENT. Save-safe (in-memory code patch; no save write). Called once from the cold-mount
/// attempt so normal play is unaffected unless a cold mount is requested.
pub(crate) fn apply_signin_force(base: usize) {
    let s = patch_3byte_stub(
        base,
        SIGNIN_FORCE_RVA,
        SIGNIN_FORCE_EXPECTED_FIRST,
        SIGNIN_FORCE_STUB,
        "signin-force",
    );
    let u = patch_3byte_stub(
        base,
        USERINDEX_FORCE_RVA,
        USERINDEX_FORCE_EXPECTED_FIRST,
        USERINDEX_FORCE_STUB,
        "userindex-force",
    );
    append_autoload_debug(format_args!(
        "signin-force: signin@0x{:x} ok={s} -> mov al,1;ret | userindex@0x{:x} ok={u} -> xor eax,eax;ret (select-op gate now passes: signed-in as user 0)",
        base + SIGNIN_FORCE_RVA,
        base + USERINDEX_FORCE_RVA
    ));
}
/// Boot-level title-accept (genuine zero input). The press-any-button wall is the
/// boot intro/movie thread parked in its movie-wait loop; the latch 0x143d856a0
/// (sole writer 0x140c8ff41) is set only AFTER that loop finishes, which is what
/// lets the inner MenuJobWait advance 10->11. The movie-dismiss gate 0x140e90820
/// has NO input check -- it finishes on decode completion or the skip-flag byte
/// 0x14458b8a5. So writing the skip-flag makes the intro thread complete its REAL
/// fade-out + teardown + latch LEGITIMATELY (proper bookkeeping, unlike the bare
/// latch poke that crashes), driving the native title-accept with zero input.
/// Watch CSFeMan 0x143d6b880 for the bootstrap.
pub(crate) unsafe fn title_accept_tick(module_base: usize, tick: u64, do_write: bool) {
    if tick < ARM_PROBE_MIN_TICK {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Module-base globals -- always safe committed reads. NO title_owner scan:
    // its full-memory VirtualQuery+deref walk raced the booting game (region freed
    // mid-scan -> AV, the boot-crash). The autoload needs none of it -- the movie
    // singleton and GameMan are fixed globals.
    let csfeman = cs_fe_man_ptr_or_null();
    let latch = unsafe { *((module_base + TITLE_ACCEPT_LATCH_RVA) as *const u8) };
    let movie = unsafe { *((module_base + MOVIE_SINGLETON_RVA) as *const usize) };
    let skip = unsafe { *((module_base + MOVIE_SKIP_FLAG_RVA) as *const u8) };
    let gm = game_man_ptr_or_null();
    let session = unsafe { *((module_base + SESSION_SINGLETON_RVA) as *const usize) };
    let log_now = (tick % ARM_PROBE_TICK_INTERVAL == null as u64)
        || (skip == MOVIE_SKIP_FLAG_SET && csfeman == null);
    // Scan-free native movie dismiss: gated on the movie singleton being present
    // with the expected vtable (= the title bg movie is up at press-any-button,
    // since splash-skip removed the logos) + a tick floor + skip-flag clear.
    if do_write && tick >= DISMISS_MIN_TICK && skip == MOVIE_SKIP_FLAG_CLEAR && movie != null {
        let movie_vtable = unsafe { *(movie as *const usize) };
        let hwnd = unsafe { *((movie + MOVIE_HWND_OFFSET) as *const usize) };
        if movie_vtable == module_base + MOVIE_VTABLE_RVA && hwnd != null {
            let hwnd_ptr = hwnd as *mut c_void;
            unsafe {
                let menu = GetSystemMenu(hwnd_ptr, WND_GET_SYSTEM_MENU_KEEP);
                if !menu.is_null() {
                    DeleteMenu(menu, WND_SC_CLOSE, WND_MF_BYCOMMAND);
                }
                ShowWindow(hwnd_ptr, WND_SW_HIDE);
                UpdateWindow(hwnd_ptr);
                *((module_base + MOVIE_SKIP_FLAG_RVA) as *mut u8) = MOVIE_SKIP_FLAG_SET;
            }
            append_autoload_debug(format_args!(
                "title_accept: native movie dismiss (movie=0x{movie:x} hwnd=0x{hwnd:x} latch={latch} tick={tick})"
            ));
        }
    }
    // Observability: GameMan load fields + session + csfeman, to see the post-
    // dismiss bootstrap/load trajectory (drives where to arm the load recipe).
    if log_now {
        let (cmd, force, slot, loading) = if gm != null {
            unsafe {
                (
                    *((gm + GAME_MAN_REQUESTED_SLOT_B78_OFFSET) as *const i32),
                    *((gm + GAME_MAN_ARM_FLAG_B72_OFFSET) as *const u8),
                    *((gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32),
                    *((gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const u8),
                )
            }
        } else {
            (
                TITLE_STATE_OWNER_GONE,
                MOVIE_SKIP_FLAG_CLEAR,
                TITLE_STATE_OWNER_GONE,
                MOVIE_SKIP_FLAG_CLEAR,
            )
        };
        append_autoload_debug(format_args!(
            "title_accept: skip={skip} movie=0x{movie:x} latch={latch} csfeman=0x{csfeman:x} session=0x{session:x} gm=0x{gm:x} cmd={cmd} force={force} slot={slot} loading={loading} tick={tick}"
        ));
    }
}
/// Per-frame native autoload arm. Recipe A set the slot once and the title reset
/// it to -1 before the save-mgr update could arm, so the latch fired Finish with
/// nothing armed -> null deref. This re-sets the slot EVERY frame (against the
/// title's reset) and sets the latch, giving the native update 0x14067f5d0 a
/// chance to arm GameMan+0xb72 before Finish. Observes b72 / b80 / CSFeMan to see
/// if the arm + bootstrap take. Crash logger should run alongside.
pub(crate) unsafe fn native_arm_loop_tick(module_base: usize, slot: i32, tick: u64) {
    if tick < ARM_PROBE_MIN_TICK {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let game_man = game_man_ptr_or_null();
    if game_man == null {
        return;
    }
    let load_in_progress =
        unsafe { *((game_man + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const u8) };
    let armed = unsafe { *((game_man + GAME_MAN_ARM_FLAG_B72_OFFSET) as *const u8) };
    let csfeman = cs_fe_man_ptr_or_null();
    if load_in_progress == TITLE_NATIVE_JOB_TASK_DATA_ZERO {
        // Re-arm each frame: persist the slot against the title's reset, set latch.
        let set_save_slot: unsafe extern "system" fn(i32) =
            unsafe { std::mem::transmute(module_base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
        unsafe { set_save_slot(slot) };
        unsafe {
            *((module_base + SELECTBOT_LOAD_GATE_RVA) as *mut u8) = TITLE_PROCEED_GATE_SET_VALUE;
        }
    }
    if tick % ARM_PROBE_TICK_INTERVAL == null as u64 {
        let ac0 = unsafe { *((game_man + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
        append_autoload_debug(format_args!(
            "native_arm_loop tick={tick} ac0={ac0} b72={armed} b80={load_in_progress} csfeman=0x{csfeman:x}"
        ));
    }
}
/// Read-only probe of the native autoload-arm preconditions at the title. The
/// decisive unknown is `[slotmgr+0x8]` (the loaded slot-record container): the
/// native save-mgr update arms autoload only when it is populated. Logs the
/// GameMan flow flags, slot manager + its data/container pointers, and whether
/// CSFeMan / the input manager exist yet. Touches no state.
pub(crate) unsafe fn arm_precondition_probe(module_base: usize, tick: u64) {
    if tick < ARM_PROBE_MIN_TICK
        || tick % ARM_PROBE_TICK_INTERVAL != TITLE_OWNER_SCAN_START_ADDRESS as u64
    {
        return;
    }
    let game_man = game_man_ptr_or_null();
    let slot_mgr = game_data_man_ptr_or_null();
    let csfeman = cs_fe_man_ptr_or_null();
    let input_mgr = cs_menu_man_ptr_or_null();
    let latch = unsafe { *((module_base + SELECTBOT_LOAD_GATE_RVA) as *const u8) };
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let gm_byte = |off: usize| {
        if game_man != null {
            i64::from(unsafe { *((game_man + off) as *const u8) })
        } else {
            ARM_PROBE_FIELD_ABSENT
        }
    };
    let gm_i32 = |off: usize| {
        if game_man != null {
            i64::from(unsafe { *((game_man + off) as *const i32) })
        } else {
            ARM_PROBE_FIELD_ABSENT
        }
    };
    let (slot_data, slot_container) = if slot_mgr != null {
        (
            game_data_man_main_player_game_data_or_null(),
            game_data_man_profile_summary_or_null(),
        )
    } else {
        (null, null)
    };
    append_autoload_debug(format_args!(
        "arm_probe tick={tick} gm=0x{game_man:x} slotmgr=0x{slot_mgr:x} slotmgr+8=0x{slot_data:x} slotmgr+78=0x{slot_container:x} csfeman=0x{csfeman:x} input_mgr=0x{input_mgr:x} latch={latch} b80={} ac0={} b72={} b73={} b75={} b78={} bc4={}",
        gm_byte(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET),
        gm_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET),
        gm_byte(GAME_MAN_ARM_FLAG_B72_OFFSET),
        gm_byte(GAME_MAN_FLAG_B73_PROBE_OFFSET),
        gm_byte(GAME_MAN_FLAG_B75_PROBE_OFFSET),
        gm_i32(GAME_MAN_REQUESTED_SLOT_B78_OFFSET),
        gm_byte(GAME_MAN_FLAG_BC4_OFFSET),
    ));
}
