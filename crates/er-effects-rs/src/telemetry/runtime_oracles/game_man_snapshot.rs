/// Last-seen GameMan snapshot (for change-detection). Packed: save_slot, req_slot, save_state, and a
/// flags byte (save_requested|new_game_plus_requested|warp_requested), plus saved-map c30.
static GM_SNAP_LAST_SLOT: AtomicUsize = AtomicUsize::new(usize::MAX);
static GM_SNAP_LAST_REQ: AtomicUsize = AtomicUsize::new(usize::MAX);
static GM_SNAP_LAST_STATE: AtomicUsize = AtomicUsize::new(usize::MAX);
static GM_SNAP_LAST_FLAGS: AtomicUsize = AtomicUsize::new(usize::MAX);
static GM_SNAP_LAST_C30: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Packed TitleStep-side session-liveness words: InGameStep request code (+0xd8) in the low half,
/// TitleStep committed state in the high half.
static GM_SNAP_LAST_SESSION: AtomicUsize = AtomicUsize::new(usize::MAX);
/// The in-game menu job qword at CSMenuMan+0x798 (the STEP_RequestWait liveness gate).
static GM_SNAP_LAST_MENU_JOB: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Diagnostic: log GameMan's key save/load fields (typed, via `GameManTelemetry` -- NO hardcoded
/// offsets) whenever ANY of them CHANGES. Called each game-task frame; change-detection turns it into
/// a compact transition trace so the STABLE boot-load trajectory (Patches) and the BOUNCE switch-load
/// trajectory (Speed Bean) can be diffed side by side to find which GameMan field re-triggers the
/// title. `save_requested`/`new_game_plus_requested`/`warp_requested` are the prime suspects for a
/// post-load revert. c30 (saved map) uses our own RE offset const (not a fromsoftware field).
pub(crate) fn snapshot_game_man_on_change() {
    // GameMan resolves only once the boot is far along; the session-liveness words below matter
    // EARLIER (the boot load), so sample with a default GameMan view instead of returning.
    let t = unsafe { GameMan::instance() }
        .map(|game_man| GameManTelemetry::from_game_man(game_man))
        .unwrap_or_default();
    let gm = game_man_ptr_or_null();
    let (c30, bc4, b73) = if gm != TITLE_OWNER_SCAN_START_ADDRESS {
        (
            unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(-1),
            unsafe { safe_read_i32(gm + GAME_MAN_RETURN_TITLE_JOB_PREDICATE_BC4_OFFSET) }
                .unwrap_or(-1),
            // field_0xb73 (unnamed in fromsoftware-rs); set to 1 by the return-title REQUEST.
            unsafe { safe_read_i32(gm + 0xb73) }.unwrap_or(-1) & 0xff,
        )
    } else {
        (-1, -1, -1)
    };
    let flags = (t.save_requested as usize)
        | ((t.new_game_plus_requested as usize) << 1)
        | ((t.warp_requested as usize) << 2)
        | ((bc4 as u32 as usize) << 8)
        | ((b73 as u32 as usize) << 16);
    let slot = t.save_slot as u32 as usize;
    let req = t.requested_save_slot_load_index as u32 as usize;
    let state = t.save_state as usize;
    let c30u = c30 as u32 as usize;
    // Session-liveness words (the post-reload bounce gate, see constants.rs IN_GAME_STEP_* block):
    // TitleStep committed state, InGameStep request code (+0xd8), and the in-game menu job qword at
    // CSMenuMan+0x798 that STEP_RequestWait polls at code 2.
    let mut owner = TITLE_OWNER_PTR.load(Ordering::SeqCst);
    if owner == TITLE_OWNER_SCAN_START_ADDRESS {
        // The scan caches the owner late (~+31s); the SetState trace detour sees it from the first
        // title transition (~+12s), covering the boot-load window.
        owner = TITLE_SETSTATE_TRACE_LAST_OWNER.load(Ordering::SeqCst);
    }
    let (committed, ig_d8) = if owner != TITLE_OWNER_SCAN_START_ADDRESS {
        (
            unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }.unwrap_or(-1),
            unsafe { safe_read_usize(owner + TITLE_STEP_IN_GAME_STEP_2E8_OFFSET) }
                .filter(|ig| *ig != TITLE_OWNER_SCAN_START_ADDRESS)
                .and_then(|ig| unsafe { safe_read_i32(ig + IN_GAME_STEP_REQUEST_CODE_D8_OFFSET) })
                .unwrap_or(-1),
        )
    } else {
        (-1, -1)
    };
    let base = crate::experiments::game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let menu_job = if base != TITLE_OWNER_SCAN_START_ADDRESS {
        Some(cs_menu_man_ptr_or_null())
            .filter(|mm| *mm != TITLE_OWNER_SCAN_START_ADDRESS)
            .and_then(|mm| unsafe { safe_read_usize(mm + CS_MENU_MAN_IN_GAME_MENU_JOB_798_OFFSET) })
            .unwrap_or(usize::MAX)
    } else {
        usize::MAX
    };
    let session = (ig_d8 as u32 as usize) | ((committed as u32 as usize) << 32);
    // Swap every field's stored last-value unconditionally (so none is missed), OR the per-field
    // change flags. `|` (not `||`) so all swaps always run.
    let changed = (GM_SNAP_LAST_SLOT.swap(slot, Ordering::SeqCst) != slot)
        | (GM_SNAP_LAST_REQ.swap(req, Ordering::SeqCst) != req)
        | (GM_SNAP_LAST_STATE.swap(state, Ordering::SeqCst) != state)
        | (GM_SNAP_LAST_FLAGS.swap(flags, Ordering::SeqCst) != flags)
        | (GM_SNAP_LAST_C30.swap(c30u, Ordering::SeqCst) != c30u)
        | (GM_SNAP_LAST_SESSION.swap(session, Ordering::SeqCst) != session)
        | (GM_SNAP_LAST_MENU_JOB.swap(menu_job, Ordering::SeqCst) != menu_job);
    if changed {
        append_autoload_debug(format_args!(
            "gm-snap: save_slot={} req_slot={} save_state={} save_requested={} ngp_requested={} warp_requested={} bc4={bc4} b73={b73} c30=0x{c30:x} committed={committed} ig_d8={ig_d8} menu_job=0x{menu_job:x}",
            t.save_slot,
            t.requested_save_slot_load_index,
            t.save_state,
            t.save_requested,
            t.new_game_plus_requested,
            t.warp_requested
        ));
    }
}

pub(crate) fn write_game_man_telemetry(body: &mut String) {
    // `loadgame_build_ctx_ready`: the "engine filled enough to drive our own load" gate -- GameDataMan
    // -> menuSystemSaveLoad -> a PLAUSIBLE TitleFlowContext at mss+0xa38. This is the gate the bypass
    // arms on. It is DISTINCT from `game_man_instance_resolved` below, which only means the GameMan
    // pointer is non-null (true from BootPhase4, long before the LoadGame job can be built without an AV).
    // Computed independently of GameMan::instance() so it is always emitted (both branches below).
    let loadgame_build_ctx_ready = crate::experiments::game_module_base()
        .map(|base| unsafe { crate::experiments::loadgame_build_ctx_ready(base) })
        .unwrap_or(false);
    body.push_str(&format!(
        "  \"loadgame_build_ctx_ready\": {loadgame_build_ctx_ready},\n"
    ));

    let base = crate::experiments::game_module_base().unwrap_or(0);
    let owner = TITLE_OWNER_PTR.load(Ordering::SeqCst);
    let dialog = if owner != 0 && owner != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    let dialog_vt = if dialog != 0 {
        unsafe { safe_read_usize(dialog) }.unwrap_or(0)
    } else {
        0
    };
    let title_flow_context =
        if base != 0 && dialog != 0 && dialog_vt == base + TITLE_TOP_DIALOG_VTABLE_RVA {
            unsafe { safe_read_usize(dialog + DIALOG_OWNER_CTX_A38_OFFSET) }.unwrap_or(0)
        } else {
            0
        };
    let tfc_version = if title_flow_context > OWNER_CTX_MIN_PLAUSIBLE_PTR
        && title_flow_context < OWNER_CTX_MAX_PLAUSIBLE_PTR
    {
        unsafe { safe_read_i32(title_flow_context + TFC_REGULATION_VERSION_148_OFFSET) }
    } else {
        None
    };
    let regulation_manager = if base != 0 {
        unsafe { safe_read_usize(base + GLOBAL_CS_REGULATION_MANAGER_RVA) }.unwrap_or(0)
    } else {
        0
    };
    let regulation_manager_version =
        if regulation_manager != 0 && regulation_manager != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_i32(regulation_manager + REGULATION_MANAGER_VERSION_44_OFFSET) }
        } else {
            None
        };
    body.push_str(&format!(
        "  \"oracle_title_flow_context_ptr\": \"0x{title_flow_context:x}\",\n"
    ));
    body.push_str(&format!(
        "  \"oracle_title_flow_context_regulation_version\": {},\n",
        tfc_version.map_or_else(|| "null".to_owned(), |value| value.to_string())
    ));
    body.push_str(&format!(
        "  \"oracle_regulation_manager_ptr\": \"0x{regulation_manager:x}\",\n"
    ));
    body.push_str(&format!(
        "  \"oracle_regulation_manager_version\": {},\n",
        regulation_manager_version.map_or_else(|| "null".to_owned(), |value| value.to_string())
    ));

    let Ok(game_man) = (unsafe { GameMan::instance() }) else {
        body.push_str("  \"game_man_instance_resolved\": false,\n");
        return;
    };

    let telemetry = GameManTelemetry::from_game_man(game_man);
    body.push_str("  \"game_man_instance_resolved\": true,\n");
    body.push_str(&format!("  \"game_save_slot\": {},\n", telemetry.save_slot));
    body.push_str(&format!(
        "  \"game_requested_save_slot_load_index\": {},\n",
        telemetry.requested_save_slot_load_index
    ));
    body.push_str(&format!(
        "  \"game_save_state\": {},\n",
        telemetry.save_state
    ));
    body.push_str(&format!(
        "  \"game_save_requested\": {},\n",
        telemetry.save_requested
    ));
}

