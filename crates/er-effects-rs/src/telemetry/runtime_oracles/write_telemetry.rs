/// The shared `EffectsState`, published so a thread that is NOT the game task can attempt a
/// telemetry flush.
///
/// Publishing shared mutable state as a global is a real cost and it is paid for one reason: the
/// only writer of the telemetry file was the game task, so the file could only ever describe events
/// the game task lived to see. When the terminal event of a feature is `ExitProcess`, that is not a
/// theoretical limit -- run pr109-boot-oscancel-20260730-110704 shipped a telemetry file that was
/// 12 seconds stale at the moment it became the only surviving record.
///
/// THE ONLY SANCTIONED ACCESS IS [`try_write_telemetry_off_game_task`], and it uses `try_lock`.
/// Never add a blocking `lock()` on this handle: the thread that reaches for it is typically doing
/// so BECAUSE the game task may be wedged, and a wedged task holding this mutex would convert a
/// stale-file problem into a hung process.
static PUBLISHED_EFFECTS_STATE: std::sync::OnceLock<Arc<Mutex<EffectsState>>> =
    std::sync::OnceLock::new();

/// Publish the shared state handle once, at bootstrap.
pub(crate) fn publish_effects_state(state: &Arc<Mutex<EffectsState>>) {
    let _ = PUBLISHED_EFFECTS_STATE.set(Arc::clone(state));
}

/// Flush the telemetry file from a thread that does not own `EffectsState`. Returns whether it
/// actually wrote.
///
/// NON-BLOCKING BY CONSTRUCTION. `try_lock` fails rather than waits, so a game task frozen mid-tick
/// costs a stale file (recorded as `oracle_save_picker_boot_telemetry_flushed = 0`) instead of
/// costing the caller its ability to finish. A short bounded retry covers the ordinary case where
/// the task merely holds the lock for the microseconds of its own tick.
pub(crate) fn try_write_telemetry_off_game_task(player_available: bool, attempts: usize) -> bool {
    let Some(handle) = PUBLISHED_EFFECTS_STATE.get() else {
        return false;
    };
    // Held-but-never-sent channel: the repo's sanctioned bounded wait (see the dim overlay and the
    // window observer). Not synchronization -- the loop exits the instant the lock is free.
    let (_pace_tx, pace_rx) = std::sync::mpsc::channel::<()>();
    for attempt in 0..attempts.max(1) {
        if let Ok(state) = handle.try_lock() {
            write_telemetry(&state, player_available);
            return true;
        }
        if attempt + 1 < attempts.max(1) {
            let _ = pace_rx.recv_timeout(Duration::from_millis(20));
        }
    }
    false
}

pub(crate) fn write_telemetry_throttled(state: &mut EffectsState, player_available: bool) {
    const TELEMETRY_INTERVAL: Duration = Duration::from_millis(250);

    let now = Instant::now();
    if state
        .last_telemetry_write
        .is_some_and(|last_write| now.duration_since(last_write) < TELEMETRY_INTERVAL)
    {
        return;
    }

    state.last_telemetry_write = Some(now);
    write_telemetry(state, player_available);
}

pub(crate) fn write_telemetry(state: &EffectsState, player_available: bool) {
    if BOOTSTRAP_TELEMETRY_SEEN
        .compare_exchange(
            BOOTSTRAP_TELEMETRY_UNSEEN,
            BOOTSTRAP_TELEMETRY_SEEN_VALUE,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
    {
        write_bootstrap_event(
            BOOTSTRAP_EVENT_TELEMETRY_WRITE,
            if player_available {
                BOOTSTRAP_DETAIL_PLAYER_AVAILABLE
            } else {
                BOOTSTRAP_DETAIL_PLAYER_UNAVAILABLE
            },
        );
    }

    let player_seen =
        player_available || IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES;
    let path = telemetry_path();
    let mut body = String::new();
    let seamless_loaded = seamless_coop_loaded();
    let runtime_mode = if seamless_loaded {
        RUNTIME_MODE_SEAMLESS
    } else {
        RUNTIME_MODE_VANILLA_OR_UNKNOWN
    };
    body.push_str("{\n");
    body.push_str(&format!("  \"player_available\": {player_available},\n"));
    body.push_str(&format!("  \"player_seen\": {player_seen},\n"));
    body.push_str(&format!("  \"runtime_mode\": \"{runtime_mode}\",\n"));
    body.push_str(&format!("  \"seamless_coop_loaded\": {seamless_loaded},\n"));
    // Loading-screen portrait fail-fast semaphore state (er-effects-rs-j3r): 0 = healthy / never
    // tripped; nonzero packs (loaded_slot<<16)|(render_target_slot<<8)|cond (cond bit0=wrong-slot,
    // bit1=null loaded renderer). On diagnostic runs a violation also crashes the run (crash log).
    body.push_str(&format!(
        "  \"oracle_portrait_render_semaphore\": {},\n",
        PORTRAIT_RENDER_SEMAPHORE_STATE.load(Ordering::SeqCst)
    ));
    // In-world ProfileSelect table guard (er-effects-rs-j3r): repairs = native-setup rebuilds of a
    // fully-empty renderer table at builder entry; guard_skips = native builder calls dropped
    // because a slot would still null-deref at [entry+0x754].
    body.push_str(&format!(
        "  \"oracle_profileselect_table_repairs\": {},\n  \"oracle_profileselect_table_guard_skips\": {},\n",
        PROFILE_SELECT_TABLE_REPAIR_COUNT.load(Ordering::SeqCst),
        PROFILE_SELECT_TABLE_GUARD_SKIP_COUNT.load(Ordering::SeqCst)
    ));
    body.push_str(&format!(
        "  \"seamless_coop_marker\": {},\n",
        if seamless_loaded {
            format!("\"{}\"", json_escape(SEAMLESS_COOP_MARKER))
        } else {
            "null".to_owned()
        }
    ));
    body.push_str(&format!(
        "  \"current_animation_id\": {},\n",
        state
            .current_animation_id
            .map_or_else(|| "null".to_owned(), |id| id.to_string())
    ));
    body.push_str(&format!(
        "  \"expected_animation_seen\": {},\n",
        state.expected_animation_seen
    ));
    body.push_str(&format!(
        "  \"autoload_save_extension\": {},\n",
        state.autoload.save_extension().map_or_else(
            || "null".to_owned(),
            |extension| format!("\"{}\"", json_escape(extension))
        )
    ));
    body.push_str(&format!(
        "  \"autoload_slot\": {},\n",
        state
            .autoload
            .slot()
            .map_or_else(|| "null".to_owned(), |slot| slot.to_string())
    ));
    body.push_str(&format!(
        "  \"autoload_method\": \"{}\",\n",
        state.autoload.method().label()
    ));
    body.push_str(&format!(
        "  \"autoload_require_title_bootstrap\": {},\n",
        state.autoload.requires_title_bootstrap()
    ));
    body.push_str(&format!(
        "  \"title_handoff_complete\": {},\n",
        TITLE_HANDOFF_COMPLETE.load(Ordering::SeqCst) != TITLE_HANDOFF_INCOMPLETE
    ));
    // Cold-char-mount progress as phase+1 (0 = never ran, 5 = PHASE_DONE = terminal/evidence
    // collected). The readiness watcher tears down on the terminal value instead of the cap.
    body.push_str(&format!(
        "  \"oracle_cold_char_mount_phase\": {},\n",
        crate::experiments::COLD_CHAR_MOUNT_PHASE_PUB.load(Ordering::SeqCst)
    ));
    // OWN-LOAD verify-only probe progress as phase+1 (0 = never ran, 2 = PHASE_DONE = terminal,
    // evidence collected). The readiness watcher tears down on the terminal value, not the cap.
    body.push_str(&format!(
        "  \"oracle_own_load_phase\": {},\n",
        crate::experiments::OWN_LOAD_PHASE_PUB.load(Ordering::SeqCst)
    ));
    // OWN-LOAD per-frame world-stream stall telemetry (own-load-reaches-loading-screen-2026-06-22 /
    // full-pipeline-traced-to-worldreswait-map-block-streaming). After own_load_continue fires
    // continue_confirm/SetState5 the engine reaches the real-char LOADING SCREEN but STALLS; these
    // mirror the deepest world-load pump values so the readiness watcher / agent can see whether ANY
    // advances (progress) or all are frozen (genuine stall). UNREAD sentinel -> JSON null (the chain
    // pointer was null / RPM faulted, distinct from a real 0). All hex except the count fields.
    let fmt_stream = |v: i64, hex: bool| -> String {
        if v == crate::experiments::OWN_LOAD_STREAM_FIELD_UNREAD {
            "null".to_owned()
        } else if hex {
            format!("\"{v:#x}\"")
        } else {
            v.to_string()
        }
    };
    body.push_str(&format!(
        "  \"oracle_own_load_stream_frames\": {},\n  \"oracle_own_load_stream_recur_frames\": {},\n  \"oracle_own_load_continue_fired\": {},\n  \"oracle_own_load_forced_continue_handoff_ms\": {},\n  \"oracle_tfc_forced_continue_handoff_ms\": {},\n  \"oracle_own_load_stream_owner_state\": {},\n  \"oracle_own_load_stream_owner_req_state\": {},\n  \"oracle_own_load_stream_mms_state\": {},\n  \"oracle_own_load_stream_block_count\": {},\n  \"oracle_own_load_stream_req_coord\": {},\n  \"oracle_own_load_stream_io_inflight\": {},\n  \"oracle_own_load_stream_io_reqhandle\": {},\n  \"oracle_own_load_stream_c30\": {},\n  \"oracle_own_load_stream_player_present\": {},\n  \"oracle_own_load_ingame_phase\": {},\n  \"oracle_own_load_req_blockid\": {},\n  \"oracle_own_load_target_block_present\": {},\n  \"oracle_own_load_wbr_update_calls\": {},\n  \"oracle_own_load_wbr_max_phase\": {},\n  \"oracle_own_load_wbr_any_gate_set\": {},\n  \"oracle_own_m28_dispatch_fired\": {},\n  \"oracle_own_load_install_job_fired\": {},\n  \"oracle_own_load_pump_fired\": {},\n  \"oracle_own_load_pump_state\": {},\n  \"oracle_own_load_pump_subcode\": {},\n  \"oracle_own_load_pump_done\": {},\n",
        crate::experiments::OWN_LOAD_STREAM_FRAMES.load(Ordering::SeqCst),
        crate::experiments::OWN_LOAD_STREAM_RECUR_FRAMES.load(Ordering::SeqCst),
        crate::experiments::OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst),
        crate::experiments::OWN_LOAD_FORCED_CONTINUE_HANDOFF_MS.load(Ordering::SeqCst),
        crate::experiments::TFC_FORCED_CONTINUE_HANDOFF_MS.load(Ordering::SeqCst),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_OWNER_STATE.load(Ordering::SeqCst),
            false
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_OWNER_REQ_STATE.load(Ordering::SeqCst),
            false
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_MMS_STATE.load(Ordering::SeqCst),
            false
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_BLOCK_COUNT.load(Ordering::SeqCst),
            false
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_REQ_COORD.load(Ordering::SeqCst),
            true
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_IO_INFLIGHT.load(Ordering::SeqCst),
            true
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_IO_REQHANDLE.load(Ordering::SeqCst),
            true
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_C30.load(Ordering::SeqCst),
            true
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_PLAYER_PRESENT.load(Ordering::SeqCst),
            false
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_INGAME_PHASE.load(Ordering::SeqCst),
            false
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_REQ_BLOCKID.load(Ordering::SeqCst),
            true
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_TARGET_BLOCK_PRESENT.load(Ordering::SeqCst),
            false
        ),
        crate::experiments::OWN_LOAD_WBR_UPDATE_CALLS.load(Ordering::SeqCst),
        fmt_stream(
            crate::experiments::OWN_LOAD_WBR_MAX_PHASE.load(Ordering::SeqCst) as i64,
            true
        ),
        crate::experiments::OWN_LOAD_WBR_ANY_GATE_SET.load(Ordering::SeqCst),
        crate::experiments::OWN_LOAD_M28_DISPATCH_FIRED.load(Ordering::SeqCst),
        crate::experiments::OWN_LOAD_INSTALL_JOB_FIRED.load(Ordering::SeqCst),
        crate::experiments::OWN_LOAD_PUMP_FIRED.load(Ordering::SeqCst),
        fmt_stream(
            crate::experiments::OWN_LOAD_PUMP_STATE.load(Ordering::SeqCst),
            false
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_PUMP_SUBCODE.load(Ordering::SeqCst),
            false
        ),
        crate::experiments::OWN_LOAD_PUMP_DONE.load(Ordering::SeqCst),
    ));
    // RequestMoveMap BlockId fix (bd er-effects-rs-um9g): fixups>=1 == the render-handoff fix fired on
    // this load (substituted last_c30 for the invalid last_before target BlockId).
    body.push_str(&format!(
        "  \"oracle_request_move_map_hook_calls\": {},\n  \"oracle_request_move_map_hook_fixups\": {},\n  \"oracle_request_move_map_last_before\": \"0x{:x}\",\n  \"oracle_request_move_map_last_c30\": \"0x{:x}\",\n",
        crate::experiments::REQUEST_MOVE_MAP_HOOK_CALLS.load(Ordering::SeqCst),
        crate::experiments::REQUEST_MOVE_MAP_FIXUPS.load(Ordering::SeqCst),
        crate::experiments::REQUEST_MOVE_MAP_LAST_BEFORE.load(Ordering::SeqCst),
        crate::experiments::REQUEST_MOVE_MAP_LAST_C30.load(Ordering::SeqCst),
    ));
    body.push_str(&format!(
        "  \"oracle_testnet_ff_stuck_frames\": {},\n  \"oracle_testnet_ff_last_mms\": \"0x{:x}\",\n  \"oracle_testnet_ff_fired_epoch\": {},\n",
        crate::experiments::TESTNET_FF_STUCK_FRAMES.load(Ordering::SeqCst),
        crate::experiments::TESTNET_FF_LAST_MMS.load(Ordering::SeqCst),
        crate::experiments::TESTNET_FF_FIRED_EPOCH.load(Ordering::SeqCst),
    ));
    let product_core_blocker = PRODUCT_CORE_LAST_BLOCKER.load(Ordering::SeqCst);
    let format_scan_ptr = |value: usize| -> String {
        if value == TITLE_OWNER_SCAN_START_ADDRESS {
            "null".to_owned()
        } else {
            format!("\"0x{value:x}\"")
        }
    };
    let title_owner_state_bits = TITLE_OWNER_SCAN_LAST_STATE_BITS.load(Ordering::SeqCst);
    let (return_title_global_flag, csmenuman, csmenuman_menu_data, csmenuman_menu_data_flag_5d) =
        if let Ok(base) = game_module_base() {
            let global_flag =
                unsafe { safe_read_u8(base + RETURN_TITLE_FINAL_FUNCTOR_GLOBAL_FLAG_RVA) };
            let menu_man = unsafe { safe_read_usize(base + GLOBAL_CSMENUMAN_RVA) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let menu_data = if menu_man != TITLE_OWNER_SCAN_START_ADDRESS && menu_man != 0 {
                unsafe { safe_read_usize(menu_man + CSMENUMAN_MENU_DATA_08_OFFSET) }
                    .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            };
            let menu_data_flag = if menu_data != TITLE_OWNER_SCAN_START_ADDRESS && menu_data != 0 {
                unsafe { safe_read_u8(menu_data + CSMENUMAN_MENU_DATA_RETURN_TITLE_FLAG_5D_OFFSET) }
            } else {
                None
            };
            (global_flag, menu_man, menu_data, menu_data_flag)
        } else {
            (
                None,
                TITLE_OWNER_SCAN_START_ADDRESS,
                TITLE_OWNER_SCAN_START_ADDRESS,
                None,
            )
        };
    let format_optional_u8 = |value: Option<u8>| -> String {
        value.map_or_else(|| "null".to_owned(), |v| v.to_string())
    };
    body.push_str(&format!(
        "  \"product_autoload_armed\": {},\n  \"product_core_callsite_ticks\": {},\n  \"product_core_callsite_base_ok_ticks\": {},\n  \"product_core_callsite_slot_ok_ticks\": {},\n  \"product_core_callsite_last_slot\": {},\n  \"product_core_autoload_ticks\": {},\n  \"product_core_ready_blocks\": {},\n  \"product_core_ready_successes\": {},\n  \"product_core_owner_ticks\": {},\n  \"product_core_last_owner\": {},\n  \"product_core_last_title_dialog\": {},\n  \"product_core_last_title_dialog_vt\": {},\n  \"product_core_last_title_in_loop\": {},\n  \"product_core_last_title_in_textfadeout\": {},\n  \"product_core_last_menu_opened_latch\": {},\n  \"product_core_last_press_start_proxy\": {},\n  \"product_core_last_press_start_vt\": {},\n  \"product_core_last_press_start_context\": {},\n  \"product_core_last_return_title_job_predicate_bc4\": {},\n  \"product_core_return_title_final_global_flag\": {},\n  \"product_core_csmenuman\": {},\n  \"product_core_csmenuman_menu_data\": {},\n  \"product_core_csmenuman_menu_data_return_title_flag_5d\": {},\n  \"product_core_last_phase\": {},\n  \"product_core_ready_blocker\": \"{}\",\n  \"title_owner_scan_attempts\": {},\n  \"title_owner_scan_vtable_hits\": {},\n  \"title_owner_scan_table_rejects\": {},\n  \"title_owner_scan_state_rejects\": {},\n  \"title_owner_scan_cached_owner\": {},\n  \"title_owner_scan_last_candidate\": {},\n  \"title_owner_scan_last_table\": {},\n  \"title_owner_scan_last_state\": {},\n",
        product_autoload_enabled(),
        PRODUCT_CORE_CALLSITE_TICKS.load(Ordering::SeqCst),
        PRODUCT_CORE_CALLSITE_BASE_OK_TICKS.load(Ordering::SeqCst),
        PRODUCT_CORE_CALLSITE_SLOT_OK_TICKS.load(Ordering::SeqCst),
        PRODUCT_CORE_CALLSITE_LAST_SLOT.load(Ordering::SeqCst),
        PRODUCT_CORE_AUTOLOAD_TICKS.load(Ordering::SeqCst),
        PRODUCT_CORE_READY_BLOCKS.load(Ordering::SeqCst),
        PRODUCT_CORE_READY_SUCCESSES.load(Ordering::SeqCst),
        PRODUCT_CORE_OWNER_TICKS.load(Ordering::SeqCst),
        format_scan_ptr(PRODUCT_CORE_LAST_OWNER.load(Ordering::SeqCst)),
        format_scan_ptr(PRODUCT_CORE_LAST_TITLE_DIALOG.load(Ordering::SeqCst)),
        format_scan_ptr(PRODUCT_CORE_LAST_TITLE_DIALOG_VT.load(Ordering::SeqCst)),
        PRODUCT_CORE_LAST_TITLE_IN_LOOP.load(Ordering::SeqCst) != 0,
        PRODUCT_CORE_LAST_TITLE_IN_TEXTFADEOUT.load(Ordering::SeqCst) != 0,
        PRODUCT_CORE_LAST_MENU_OPENED_LATCH.load(Ordering::SeqCst),
        format_scan_ptr(PRODUCT_CORE_LAST_PRESS_START_PROXY.load(Ordering::SeqCst)),
        format_scan_ptr(PRODUCT_CORE_LAST_PRESS_START_VT.load(Ordering::SeqCst)),
        format_scan_ptr(PRODUCT_CORE_LAST_PRESS_START_CONTEXT.load(Ordering::SeqCst)),
        PRODUCT_CORE_LAST_RETURN_TITLE_JOB_PREDICATE_BC4.load(Ordering::SeqCst),
        format_optional_u8(return_title_global_flag),
        format_scan_ptr(csmenuman),
        format_scan_ptr(csmenuman_menu_data),
        format_optional_u8(csmenuman_menu_data_flag_5d),
        PRODUCT_CORE_LAST_PHASE.load(Ordering::SeqCst),
        json_escape(product_core_ready_blocker_label(product_core_blocker)),
        TITLE_OWNER_SCAN_ATTEMPTS.load(Ordering::SeqCst),
        TITLE_OWNER_SCAN_VTABLE_HITS.load(Ordering::SeqCst),
        TITLE_OWNER_SCAN_TABLE_REJECTS.load(Ordering::SeqCst),
        TITLE_OWNER_SCAN_STATE_REJECTS.load(Ordering::SeqCst),
        format_scan_ptr(TITLE_OWNER_PTR.load(Ordering::SeqCst)),
        format_scan_ptr(TITLE_OWNER_SCAN_LAST_CANDIDATE.load(Ordering::SeqCst)),
        format_scan_ptr(TITLE_OWNER_SCAN_LAST_TABLE.load(Ordering::SeqCst)),
        if title_owner_state_bits == usize::MAX {
            "null".to_owned()
        } else {
            (title_owner_state_bits as u32 as i32).to_string()
        }
    ));
    body.push_str(&format!(
        "  \"autoload_attempts\": {},\n",
        state.autoload.attempts()
    ));
    body.push_str(&format!(
        "  \"game_task_ticks\": {},\n",
        state.game_task_ticks
    ));
    write_oracle_telemetry(&mut body);
    body.push_str(&format!(
        "  \"safe_input_confirm_count\": {},\n",
        state.safe_input.confirm_count
    ));
    body.push_str(&format!(
        "  \"safe_input_pulses_sent\": {},\n",
        state.safe_input.pulses_sent
    ));
    body.push_str(&format!(
        "  \"safe_input_hooks_requested\": {},\n",
        state.safe_input.hooks_requested
    ));
    body.push_str(&format!(
        "  \"safe_input_hook_frames_remaining\": {},\n",
        SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst)
    ));
    body.push_str(&format!(
        "  \"safe_input_last_status\": {},\n",
        state.safe_input.last_status.as_ref().map_or_else(
            || "null".to_owned(),
            |status| format!("\"{}\"", json_escape(status))
        )
    ));
    body.push_str(&format!(
        "  \"system_quit_profile_load_activate_count\": {},\n  \"system_quit_profile_load_confirmed_block_count\": {},\n  \"system_quit_profile_load_confirmed_allow_count\": {},\n  \"system_quit_profile_load_job_run_block_count\": {},\n  \"system_quit_profile_load_job_run_allow_count\": {},\n  \"system_quit_profile_load_job_run_last_job\": {},\n  \"system_quit_profile_load_job_run_last_list\": {},\n  \"system_quit_profile_load_job_run_last_profile_id\": {},\n  \"system_quit_profile_load_job_post_return_title_fired\": {},\n  \"system_quit_quickload_phase\": {},\n  \"system_quit_quickload_selected_slot\": {},\n  \"system_quit_quickload_return_title_request_count\": {},\n  \"system_quit_return_title_final_functor_call_count\": {},\n  \"system_quit_quickload_native_quit_action_count\": {},\n  \"system_quit_direct_return_title_chain_submit_count\": {},\n  \"system_quit_direct_return_title_chain_ready_block_count\": {},\n  \"system_quit_direct_return_title_chain_last_dialog\": {},\n  \"system_quit_direct_return_title_chain_last_queue_ready\": {},\n  \"system_quit_skip_restore_after_quickload_count\": {},\n  \"system_quit_quickload_title_owner_seen_count\": {},\n  \"system_quit_quickload_autoload_handoff_count\": {},\n  \"system_quit_quickload_last_title_owner\": {},\n  \"system_quit_profile_load_activate_last_dialog\": {},\n  \"system_quit_profile_load_activate_last_cursor\": {},\n  \"system_quit_profile_load_activate_last_bound\": {},\n  \"system_quit_profileselect_native_close_count\": {},\n  \"system_quit_save_game_text_substitution_count\": {},\n  \"system_quit_save_game_action_count\": {},\n  \"system_quit_save_game_confirm_count\": {},\n  \"system_quit_save_game_close_count\": {},\n  \"system_quit_open_save_dir_action_count\": {},\n  \"system_quit_open_save_dir_success_count\": {},\n  \"system_quit_open_save_dir_failure_count\": {},\n  \"system_quit_save_game_armed_dialog\": {},\n  \"system_quit_request_load_slot_block_count\": {},\n  \"system_quit_request_load_slot_allow_count\": {},\n  \"system_quit_inworld_load_skip_count\": {},\n",
        SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_BLOCK_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ALLOW_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_BLOCK_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_ALLOW_COUNT.load(Ordering::SeqCst),
        format_scan_ptr(SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_JOB.load(Ordering::SeqCst)),
        format_scan_ptr(SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_LIST.load(Ordering::SeqCst)),
        SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_PROFILE_ID.load(Ordering::SeqCst),
        SYSTEM_QUIT_PROFILE_LOAD_JOB_POST_RETURN_TITLE_FIRED.load(Ordering::SeqCst),
        SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst),
        SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst),
        SYSTEM_QUIT_QUICKLOAD_RETURN_TITLE_REQUEST_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_QUICKLOAD_NATIVE_QUIT_ACTION_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_READY_BLOCK_COUNT.load(Ordering::SeqCst),
        format_scan_ptr(SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_LAST_DIALOG.load(Ordering::SeqCst)),
        SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_LAST_QUEUE_READY.load(Ordering::SeqCst),
        SYSTEM_QUIT_SKIP_RESTORE_AFTER_QUICKLOAD_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_QUICKLOAD_TITLE_OWNER_SEEN_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_QUICKLOAD_AUTOLOAD_HANDOFF_COUNT.load(Ordering::SeqCst),
        format_scan_ptr(SYSTEM_QUIT_QUICKLOAD_LAST_TITLE_OWNER.load(Ordering::SeqCst)),
        format_scan_ptr(SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_DIALOG.load(Ordering::SeqCst)),
        SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_CURSOR.load(Ordering::SeqCst),
        SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_BOUND.load(Ordering::SeqCst),
        SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_SAVE_GAME_TEXT_SUBSTITUTION_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_SAVE_GAME_ACTION_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_SAVE_GAME_CONFIRM_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_SAVE_GAME_CLOSE_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_OPEN_SAVE_DIR_SUCCESS_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.load(Ordering::SeqCst),
        format_scan_ptr(SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG.load(Ordering::SeqCst)),
        SYSTEM_QUIT_REQUEST_LOAD_SLOT_BLOCK_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_REQUEST_LOAD_SLOT_ALLOW_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_INWORLD_LOAD_SKIP_COUNT.load(Ordering::SeqCst)
    ));
    // System->Quit ROW IDENTITY oracles. `oracle_system_quit_quit_refused_ambiguous_row_count` is the
    // P0 gate firing (an instant ExitProcess refused because the activated row could not be
    // positively identified as Return to Desktop);
    // `oracle_system_quit_row_last_discriminator` records WHICH evidence resolved the row. There is
    // exactly ONE source -- the dialog's list cursor -- so it reads 1 for every resolution regardless
    // of input kind (`..._last_input_kind`: 1 pad/keyboard, 2 mouse), and
    // `oracle_system_quit_row_resolved_by_cursor_row_count` must equal
    // `resolve_count - ambiguous_count`. `oracle_system_quit_row_last_ambiguity` says why a row could
    // not be named; `..._refused_disagreement_count` counts the subset where the captured row table
    // and the label read live at the cursor CONTRADICTED each other, so the row ran nothing.
    // `oracle_system_quit_grid_*` is the navigability evidence read live off the dialog's
    // `CS::GridControl`: all four rows are reachable only when `navigable_cells >= 4` (the bound of
    // the native mouse hit-test loop), `item_count == 4` (the cursor bound) and `rows >= 2` (what
    // enables the up/down axis at all).
    body.push_str(&format!(
        "  \"oracle_system_quit_row_table_dialog\": {},\n  \"oracle_system_quit_row_index_save_game\": {},\n  \"oracle_system_quit_row_index_return_desktop\": {},\n  \"oracle_system_quit_row_index_load_profile\": {},\n  \"oracle_system_quit_row_index_load_save_profiles\": {},\n  \"oracle_system_quit_row_resolve_count\": {},\n  \"oracle_system_quit_row_resolved_by_cursor_row_count\": {},\n  \"oracle_system_quit_row_ambiguous_count\": {},\n  \"oracle_system_quit_row_refused_disagreement_count\": {},\n  \"oracle_system_quit_grid_cols\": {},\n  \"oracle_system_quit_grid_rows\": {},\n  \"oracle_system_quit_grid_navigable_cells\": {},\n  \"oracle_system_quit_grid_item_count\": {},\n  \"oracle_system_quit_row_last_discriminator\": {},\n  \"oracle_system_quit_row_last_resolved_row\": {},\n  \"oracle_system_quit_row_last_ambiguity\": {},\n  \"oracle_system_quit_row_last_cursor\": {},\n  \"oracle_system_quit_row_last_cursor_label_kind\": {},\n  \"oracle_system_quit_row_last_input_kind\": {},\n  \"oracle_system_quit_quit_refused_ambiguous_row_count\": {},\n  \"oracle_system_quit_quit_authorized_count\": {},\n  \"oracle_system_quit_action_alias_false_quit_claims\": {},\n",
        format_scan_ptr(SYSTEM_QUIT_ROW_TABLE_DIALOG.load(Ordering::SeqCst)),
        SYSTEM_QUIT_ROW_INDEX_SAVE_GAME_PLUS1
            .load(Ordering::SeqCst)
            .wrapping_sub(1) as isize,
        SYSTEM_QUIT_ROW_INDEX_RETURN_DESKTOP_PLUS1
            .load(Ordering::SeqCst)
            .wrapping_sub(1) as isize,
        SYSTEM_QUIT_ROW_INDEX_LOAD_PROFILE_PLUS1
            .load(Ordering::SeqCst)
            .wrapping_sub(1) as isize,
        SYSTEM_QUIT_ROW_INDEX_LOAD_SAVE_PROFILES_PLUS1
            .load(Ordering::SeqCst)
            .wrapping_sub(1) as isize,
        SYSTEM_QUIT_ROW_RESOLVE_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_ROW_RESOLVED_BY_CURSOR_ROW_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_ROW_AMBIGUOUS_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_ROW_REFUSED_DISAGREEMENT_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_GRID_COLS.load(Ordering::SeqCst),
        SYSTEM_QUIT_GRID_ROWS.load(Ordering::SeqCst),
        SYSTEM_QUIT_GRID_NAVIGABLE_CELLS.load(Ordering::SeqCst),
        SYSTEM_QUIT_GRID_ITEM_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_ROW_LAST_DISCRIMINATOR.load(Ordering::SeqCst),
        SYSTEM_QUIT_ROW_LAST_RESOLVED_ROW.load(Ordering::SeqCst),
        SYSTEM_QUIT_ROW_LAST_AMBIGUITY.load(Ordering::SeqCst),
        SYSTEM_QUIT_ROW_LAST_CURSOR_PLUS1
            .load(Ordering::SeqCst)
            .wrapping_sub(1) as isize,
        SYSTEM_QUIT_ROW_LAST_CURSOR_LABEL_KIND.load(Ordering::SeqCst),
        SYSTEM_QUIT_ROW_LAST_INPUT_KIND.load(Ordering::SeqCst),
        SYSTEM_QUIT_QUIT_REFUSED_AMBIGUOUS_ROW_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_QUIT_AUTHORIZED_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_ACTION_ALIAS_FALSE_QUIT_CLAIMS.load(Ordering::SeqCst)
    ));
    // `oracle_save_picker_surface` states which picker this session runs (0 in-game, 1 OS dialog)
    // regardless of whether one ever opened, so a report never has to guess the mode.
    body.push_str(&format!(
        "  \"oracle_save_picker_surface\": {},\n  \"oracle_save_picker_mode_active\": {},\n  \"oracle_save_picker_open_count\": {},\n  \"oracle_save_picker_repopulate_count\": {},\n  \"oracle_save_picker_pick_count\": {},\n  \"oracle_save_picker_pick_reject_count\": {},\n  \"oracle_save_picker_resubmit_count\": {},\n  \"oracle_save_picker_cancel_count\": {},\n  \"oracle_save_picker_staged_row_count\": {},\n",
        SAVE_PICKER_SURFACE.load(Ordering::SeqCst),
        SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst),
        SAVE_PICKER_OPEN_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_REPOPULATE_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_PICK_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_PICK_REJECT_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_RESUBMIT_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_CANCEL_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_STAGED_ROW_COUNT.load(Ordering::SeqCst)
    ));
    // OS file-dialog surface. Only meaningful while `oracle_save_picker_surface` is 1, and that is
    // the point: with the default key they are all 0, which IS the non-regression proof for the
    // in-game path. `_ticks_frozen` is the load-bearing one -- it is the only thing that answers
    // whether the game task kept ticking while the menu pump was blocked, which nothing static can.
    // A `> 0` says the freeze saved the flow; a `0` with a dialog demonstrably open says the whole
    // frame stalled instead. `_savelike_opens` attributes shell browsing traffic that would
    // otherwise pollute the save CreateFileW diagnostics.
    //
    // `_owner_hwnd` must be non-zero, and READ IT WITH `_owner_is_cover` -- what it is REQUIRED to
    // be changed on 2026-07-31. It used to have to be the game window; a System>Quit open should
    // now show `_owner_is_cover = 1` and an `_owner_hwnd` equal to `_dim_hwnd`, because owning the
    // dialog to the cover is what makes "the picker is in front of the blur" a z-order invariant
    // instead of a race (an owned window is always above its owner). `_owner_is_cover = 0` on a
    // System>Quit open means the cover did not come up inside the arm handshake and the stacking
    // for that open was unguaranteed; at the missing-save BOOT it is simply correct, since that arm
    // raises no cover at all. It must still never be `ErEffectsLoadingOverlay`, which is a
    // different window entirely.
    body.push_str(&format!(
        "  \"oracle_save_picker_os_dialog_open\": {},\n  \"oracle_save_picker_os_open_count\": {},\n  \"oracle_save_picker_os_closed_with_path\": {},\n  \"oracle_save_picker_os_cancel_count\": {},\n  \"oracle_save_picker_os_error_count\": {},\n  \"oracle_save_picker_os_last_error\": {},\n  \"oracle_save_picker_os_reject_count\": {},\n  \"oracle_save_picker_os_last_reject_reason\": {},\n  \"oracle_save_picker_os_reopen_count\": {},\n  \"oracle_save_picker_os_reopen_exhausted\": {},\n  \"oracle_save_picker_os_ticks_frozen\": {},\n  \"oracle_save_picker_os_owner_hwnd\": {},\n  \"oracle_save_picker_os_owner_is_cover\": {},\n  \"oracle_save_picker_os_savelike_opens\": {},\n  \"oracle_save_dest_confirm_pending\": {},\n",
        er_telemetry::counters::SAVE_PICKER_OS_DIALOG_OPEN.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_OPEN_COUNT.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_CLOSED_WITH_PATH.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_CANCEL_COUNT.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_ERROR_COUNT.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_LAST_ERROR.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_REJECT_COUNT.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_LAST_REJECT_REASON.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_REOPEN_COUNT.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_REOPEN_EXHAUSTED.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_TICKS_FROZEN.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_OWNER_HWND.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_OWNER_IS_COVER.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_SAVELIKE_OPENS.load(Ordering::SeqCst),
        SAVE_DEST_CONFIRM_PENDING.load(Ordering::SeqCst)
    ));
    // The OS dialog at the MISSING-SAVE BOOT. Separate from the fields above because that intent's
    // outcomes are not the System>Quit ones: a cancel there QUITS THE GAME, and a terminal step has
    // to be readable from a file rather than from watching the screen.
    //
    // READ `_boot_telemetry_flushed` FIRST. It is the field that tells you whether the rest of this
    // file is about the run's end or about some earlier moment. `0` means the picker thread could
    // not take the state mutex before quitting, so every field here predates the cancel and
    // `er-effects-bootstrap.jsonl` (`boot_picker_cancel_exit`) is the authoritative record instead.
    //
    // That distinction is not hypothetical. Run pr109-boot-oscancel-20260730-110704 ended with
    // `boot_state = OPEN`, `boot_cancel_exit_count = 0` on a run where the cancel WORKED -- byte for
    // byte what a dialog that never returned would have left behind, because the game task had
    // stopped writing this file 12s before the user answered. The old published signature ("open
    // count advances while state stays OPEN") could not tell those apart, and reported a working
    // feature as broken.
    //
    // `_boot_game_ticks_at_open` / `_at_answer` are sampled by the picker thread on both sides of
    // the blocking dialog, so their DIFFERENCE is the game task's liveness across it: equal values
    // mean the game did not tick once while the dialog was up. `_boot_open_count > 1` means the
    // one-shot open latch leaked and the reopen loop came back. `_boot_fallback_count > 0` says
    // comdlg32 could not be used and the in-game browser took over -- a degraded surface, not a
    // failed boot.
    body.push_str(&format!(
        "  \"oracle_save_picker_os_boot_state\": {},\n  \"oracle_save_picker_os_boot_open_count\": {},\n  \"oracle_save_picker_os_boot_pick_count\": {},\n  \"oracle_save_picker_os_boot_cancel_exit_count\": {},\n  \"oracle_save_picker_os_boot_exit_performed\": {},\n  \"oracle_save_picker_boot_telemetry_flushed\": {},\n  \"oracle_save_picker_boot_game_ticks_at_open\": {},\n  \"oracle_save_picker_boot_game_ticks_at_answer\": {},\n  \"oracle_game_task_ticks_total\": {},\n  \"oracle_save_picker_os_boot_fallback_count\": {},\n  \"oracle_save_picker_os_boot_defer_ticks\": {},\n",
        er_telemetry::counters::SAVE_PICKER_OS_BOOT_STATE.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_BOOT_OPEN_COUNT.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_BOOT_PICK_COUNT.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_BOOT_CANCEL_EXIT_COUNT.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_BOOT_EXIT_PERFORMED.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_BOOT_TELEMETRY_FLUSHED.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_BOOT_GAME_TICKS_AT_OPEN.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_BOOT_GAME_TICKS_AT_ANSWER.load(Ordering::SeqCst),
        er_telemetry::counters::GAME_TASK_TICKS_TOTAL.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_BOOT_FALLBACK_COUNT.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_OS_BOOT_DEFER_TICKS.load(Ordering::SeqCst)
    ));
    // The screen dim raised over the game while an OS dialog is blocking the menu thread.
    //
    // `_frames` IS THE PRODUCT PROOF and nothing else in this file can stand in for it. The game
    // renders nothing for the dialog's whole life, so a frame count that ADVANCED across the
    // interval bracketed by the dialog's own OPENED/CLOSED log lines is the only evidence that the
    // animation actually ran -- on a thread we own, which is the entire reason this feature has its
    // own window. `_arm_count == _disarm_count` with `_armed == 0` is the teardown proof; the
    // opposite means a fullscreen dim is stranded over a game the user can still play. `_z_self`
    // must land BETWEEN `_z_foreign` (comdlg32) and `_z_game`, which is how the stacking is checked
    // without anyone looking at a screenshot.
    body.push_str(&format!(
        "  \"oracle_save_picker_dim_armed\": {},\n  \"oracle_save_picker_dim_arm_count\": {},\n  \"oracle_save_picker_dim_disarm_count\": {},\n  \"oracle_save_picker_dim_frames\": {},\n  \"oracle_save_picker_dim_alive_ms\": {},\n  \"oracle_save_picker_dim_teardown_reason\": {},\n  \"oracle_save_picker_dim_stage\": {},\n  \"oracle_save_picker_dim_selftest\": {},\n  \"oracle_save_picker_dim_hwnd\": {},\n  \"oracle_save_picker_dim_game_hwnd\": {},\n  \"oracle_save_picker_dim_update_fails\": {},\n  \"oracle_save_picker_dim_z_self\": {},\n  \"oracle_save_picker_dim_z_game\": {},\n  \"oracle_save_picker_dim_z_foreign\": {},\n  \"oracle_save_picker_dim_full_pushes\": {},\n  \"oracle_save_picker_dim_foreign_fg_hwnd\": {},\n  \"oracle_save_picker_dim_owner_set\": {},\n  \"oracle_save_picker_dim_owner_readback\": {},\n  \"oracle_save_picker_dim_arm_wait_ms\": {},\n  \"oracle_save_picker_dim_arm_wait_timeouts\": {},\n  \"oracle_save_picker_dim_reanchor_count\": {},\n",
        er_telemetry::counters::SAVE_PICKER_DIM_ARMED.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_ARM_COUNT.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_DISARM_COUNT.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_FRAMES.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_ALIVE_MS.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_TEARDOWN_REASON.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_STAGE.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_SELFTEST.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_HWND.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_GAME_HWND.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_UPDATE_FAILS.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_Z_SELF.load(Ordering::SeqCst) as isize,
        er_telemetry::counters::SAVE_PICKER_DIM_Z_GAME.load(Ordering::SeqCst) as isize,
        er_telemetry::counters::SAVE_PICKER_DIM_Z_FOREIGN.load(Ordering::SeqCst) as isize,
        er_telemetry::counters::SAVE_PICKER_DIM_FULL_PUSHES.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_FOREIGN_FG_HWND.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_OWNER_SET.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_OWNER_READBACK.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_ARM_WAIT_MS.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_ARM_WAIT_TIMEOUTS.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_REANCHOR_COUNT.load(Ordering::SeqCst)
    ));
    // THE Z-ORDER VERDICT, AND IT IS TWO VERDICTS, NOT ONE (er-effects-rs-mc1d). These replace the
    // single fused `oracle_save_picker_dim_z_violations`, which scored "the cover is behind the
    // game" and "the cover is over the dialog" into the same atomic even though they are opposite
    // failures. A run came back with 130 of them across 424 dim frames and could therefore neither
    // confirm nor refute the ownership fix it existed to test. READ THEM AS:
    //
    //   `_z_covering_dialog` > 0  -- REAL FAILURE of the z-order fix. Our cover was nearer the front
    //       than comdlg32 for that many frames, i.e. laid over the controls the user has to click.
    //   `_z_behind_game` > 0      -- a separate, LOWER-SEVERITY COSMETIC bug: the cover was invisible
    //       for that many frames. The dialog still worked; file it on its own, do not block on it.
    //
    // Both exclude unknown ordinals, so a frame from before comdlg32 existed is never counted. The
    // `_first_*` quartets carry the FIRST offending sample of each kind and how many milliseconds
    // into that arm's cover it happened -- `-1` means that kind never fired, and the `_ms` is what
    // separates a bring-up transient the compositor settles from a stacking that never took.
    body.push_str(&format!(
        "  \"oracle_save_picker_dim_z_behind_game\": {},\n  \"oracle_save_picker_dim_z_behind_game_first_self\": {},\n  \"oracle_save_picker_dim_z_behind_game_first_game\": {},\n  \"oracle_save_picker_dim_z_behind_game_first_foreign\": {},\n  \"oracle_save_picker_dim_z_behind_game_first_ms\": {},\n  \"oracle_save_picker_dim_z_covering_dialog\": {},\n  \"oracle_save_picker_dim_z_covering_dialog_first_self\": {},\n  \"oracle_save_picker_dim_z_covering_dialog_first_game\": {},\n  \"oracle_save_picker_dim_z_covering_dialog_first_foreign\": {},\n  \"oracle_save_picker_dim_z_covering_dialog_first_ms\": {},\n",
        er_telemetry::counters::SAVE_PICKER_DIM_Z_BEHIND_GAME.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_SELF.load(Ordering::SeqCst)
            as isize,
        er_telemetry::counters::SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_GAME.load(Ordering::SeqCst)
            as isize,
        er_telemetry::counters::SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_FOREIGN.load(Ordering::SeqCst)
            as isize,
        er_telemetry::counters::SAVE_PICKER_DIM_Z_BEHIND_GAME_FIRST_MS.load(Ordering::SeqCst)
            as isize,
        er_telemetry::counters::SAVE_PICKER_DIM_Z_COVERING_DIALOG.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_SELF.load(Ordering::SeqCst)
            as isize,
        er_telemetry::counters::SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_GAME.load(Ordering::SeqCst)
            as isize,
        er_telemetry::counters::SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_FOREIGN
            .load(Ordering::SeqCst) as isize,
        er_telemetry::counters::SAVE_PICKER_DIM_Z_COVERING_DIALOG_FIRST_MS.load(Ordering::SeqCst)
            as isize
    ));
    // Per-slot info fields (Level caption/value, PlayTime) on browse rows with no character. `_hidden`
    // > 0 proves the suppression reached real rows; `_non_display` > 0 or a `_last_datatype` other
    // than 10 says the native visibility setter ignored the field, i.e. the text is still on screen.
    body.push_str(&format!(
        "  \"oracle_profile_row_slot_info_hidden_rows\": {},\n  \"oracle_profile_row_slot_info_shown_rows\": {},\n  \"oracle_profile_row_slot_info_vis_skips\": {},\n  \"oracle_profile_row_slot_info_non_display\": {},\n  \"oracle_profile_row_slot_info_last_datatype\": {},\n",
        PROFILE_ROW_SLOT_INFO_HIDDEN_ROWS.load(Ordering::SeqCst),
        PROFILE_ROW_SLOT_INFO_SHOWN_ROWS.load(Ordering::SeqCst),
        PROFILE_ROW_SLOT_INFO_VIS_SKIPS.load(Ordering::SeqCst),
        PROFILE_ROW_SLOT_INFO_NON_DISPLAY.load(Ordering::SeqCst),
        PROFILE_ROW_SLOT_INFO_LAST_DATATYPE.load(Ordering::SeqCst) as isize
    ));
    // WHOSE MENU EACH SUMMARY POPULATE BELONGED TO. `CS::MenuSaveDataSummary`'s populate is shared by
    // every character-summary surface, the game's own System>Quit `GameEnd` panel included, so
    // `_foreign` > 0 with `_missing_field` > 0 is this mod correctly declining to draw on a movie it
    // never edited. `_own` is the ProfileSelect rows it does own. `_own` at zero while the profile
    // list is on screen would mean the probe field stopped resolving, i.e. the GFX edit is not live.
    body.push_str(&format!(
        "  \"oracle_profile_own_summary_rows\": {},\n  \"oracle_profile_foreign_summary_rows\": {},\n  \"oracle_profile_stats_push_missing_field\": {},\n",
        PROFILE_OWN_SUMMARY_ROWS.load(Ordering::SeqCst),
        PROFILE_FOREIGN_SUMMARY_ROWS.load(Ordering::SeqCst),
        PROFILE_STATS_PUSH_MISSING_FIELD.load(Ordering::SeqCst)
    ));
    // LIVE-EDITOR SAFETY GATE. `_window_run_ticks` rises once per rendered frame of the
    // ProfileSelect view; `_deferred_applies` counts web-UI edits the frame thread refused to write
    // while that view was on screen, leaving them for the in-band row populate. Deferrals are the
    // guard working: each one is a crash that did not happen.
    body.push_str(&format!(
        "  \"oracle_profile_select_window_run_ticks\": {},\n  \"oracle_profile_editor_deferred_applies\": {},\n",
        er_telemetry::counters::PROFILE_SELECT_WINDOW_RUN_TICKS.load(Ordering::SeqCst),
        er_telemetry::counters::PROFILE_EDITOR_DEFERRED_APPLIES.load(Ordering::SeqCst)
    ));
    // DO THE ROWS DESCRIBE THE SAVE ON SCREEN? The per-slot name/attribute caches were a
    // process-lifetime latch, so a session's first save described every row forever. `_reloads`
    // counts refills from the picker's own bytes when a save is previewed; `_invalidations` counts
    // drops when that preview is withdrawn. Both at zero after a save swap means the rows are
    // describing a save the user is no longer looking at.
    body.push_str(&format!(
        "  \"oracle_profile_slot_cache_preview_reloads\": {},\n  \"oracle_profile_slot_cache_invalidations\": {},\n",
        er_telemetry::counters::PROFILE_SLOT_CACHE_PREVIEW_RELOADS.load(Ordering::SeqCst),
        er_telemetry::counters::PROFILE_SLOT_CACHE_INVALIDATIONS.load(Ordering::SeqCst)
    ));
    // Save-file rows showing when the file was last written in place of the native playtime.
    // `_rows` > 0 proves the row model carried our text into the native populate; `_stage_failures`
    // > 0 means the model field was unreadable and the row kept the game's own playtime string.
    body.push_str(&format!(
        "  \"oracle_profile_row_last_saved_rows\": {},\n  \"oracle_profile_row_last_saved_stage_failures\": {},\n",
        PROFILE_ROW_LAST_SAVED_ROWS.load(Ordering::SeqCst),
        PROFILE_ROW_LAST_SAVED_STAGE_FAILURES.load(Ordering::SeqCst)
    ));
    body.push_str(&format!(
        "  \"oracle_save_picker_overlay_armed\": {},\n  \"oracle_save_picker_overlay_open_count\": {},\n  \"oracle_save_picker_overlay_draw_hits\": {},\n  \"oracle_save_picker_overlay_input_hits\": {},\n  \"oracle_save_picker_overlay_poll_count\": {},\n  \"oracle_save_picker_overlay_held_polls\": {},\n  \"oracle_save_picker_kbd_hook_hits\": {},\n  \"oracle_save_picker_overlay_pick_count\": {},\n  \"oracle_save_picker_overlay_pick_reject_count\": {},\n",
        SAVE_PICKER_OVERLAY_ARMED.load(Ordering::SeqCst),
        SAVE_PICKER_OVERLAY_OPEN_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_OVERLAY_DRAW_HITS.load(Ordering::SeqCst),
        SAVE_PICKER_OVERLAY_INPUT_HITS.load(Ordering::SeqCst),
        SAVE_PICKER_OVERLAY_POLL_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_OVERLAY_HELD_POLLS.load(Ordering::SeqCst),
        SAVE_PICKER_KBD_HOOK_HITS.load(Ordering::SeqCst),
        SAVE_PICKER_OVERLAY_PICK_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_OVERLAY_PICK_REJECT_COUNT.load(Ordering::SeqCst)
    ));
    body.push_str(&format!(
        "  \"system_quit_continue_confirm_fresh_deser_done\": {},\n  \"system_quit_continue_confirm_fresh_deser_count\": {},\n  \"system_quit_continue_confirm_block_count\": {},\n  \"system_quit_continue_confirm_allow_count\": {},\n  \"system_quit_continue_confirm_non_switch_count\": {},\n  \"system_quit_continue_confirm_world_up_count\": {},\n  \"system_quit_continue_confirm_unproven_forward_count\": {},\n",
        SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst),
        SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_CONTINUE_CONFIRM_BLOCK_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_CONTINUE_CONFIRM_NON_SWITCH_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_CONTINUE_CONFIRM_WORLD_UP_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_CONTINUE_CONFIRM_UNPROVEN_FORWARD_COUNT.load(Ordering::SeqCst)
    ));
    // LOAD COUNT, STATED RATHER THAN IMPLIED. Six existing counters describe "loads" with four
    // different values because each counts a different event class; a reader who picks one and
    // divides gets a wrong total (a captured 3-load session reads activate=4, allow=3, epoch=2,
    // pick=2). `oracle_total_world_loads` is the answer; `oracle_load_count_witness_signature` shows
    // every witness side by side so the composition never has to be reconstructed by hand again; and
    // `oracle_load_count_mismatches` is nonzero the moment those witnesses contradict each other, so
    // a future run reports the contradiction itself instead of waiting to be noticed.
    // See er_telemetry::load_count for the decomposition and why the epoch is an INDEX, not a count.
    {
        let witnesses = er_telemetry::load_count::LoadCountWitnesses {
            continue_confirm_forwards: SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT
                .load(Ordering::SeqCst),
            switch_reload_commits: SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT
                .load(Ordering::SeqCst),
            non_switch_forwards: SYSTEM_QUIT_CONTINUE_CONFIRM_NON_SWITCH_COUNT
                .load(Ordering::SeqCst),
            world_up_forwards: SYSTEM_QUIT_CONTINUE_CONFIRM_WORLD_UP_COUNT.load(Ordering::SeqCst),
            continue_confirm_blocks: SYSTEM_QUIT_CONTINUE_CONFIRM_BLOCK_COUNT
                .load(Ordering::SeqCst),
            picker_activations: SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_PICKER_COUNT
                .load(Ordering::SeqCst),
            slot_activations: SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_SLOT_COUNT.load(Ordering::SeqCst),
            total_activations: SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_COUNT.load(Ordering::SeqCst),
            picker_picks: SAVE_PICKER_PICK_COUNT.load(Ordering::SeqCst),
            picker_pick_rejects: SAVE_PICKER_PICK_REJECT_COUNT.load(Ordering::SeqCst),
            picker_repopulates: SAVE_PICKER_REPOPULATE_COUNT.load(Ordering::SeqCst),
        };
        let mismatches = witnesses.mismatches();
        er_telemetry::counters::LOAD_COUNT_MISMATCH_BITS
            .store(mismatches.bits() as usize, Ordering::SeqCst);
        body.push_str(&format!(
            "  \"oracle_total_world_loads\": {},\n  \"oracle_current_load_index\": {},\n  \"oracle_load_count_mismatches\": {},\n  \"oracle_load_count_mismatch_bits\": {},\n  \"oracle_load_count_mismatch_names\": \"{}\",\n  \"oracle_load_count_witness_signature\": \"{}\",\n",
            witnesses.total_world_loads(),
            witnesses
                .current_load_index()
                .map(|index| index as isize)
                .unwrap_or(-1),
            mismatches.count(),
            mismatches.bits(),
            mismatches.names(),
            witnesses.signature()
        ));
    }
    // Full-read chain terminal disarm (bd er-effects-rs-ns4n): count > 0 proves a non-commit exit
    // cleared the pending native slot request, so the in-game save manager cannot service a stale
    // request into the live world (the gaitem free-queue AV at 0x67141a). `_last_prev_slot` is the
    // i32 slot value the last clear removed, u32-packed (0xffffffff == the no-request sentinel, i.e.
    // nothing needed clearing on that exit).
    body.push_str(&format!(
        "  \"oracle_fullread_req_disarm_count\": {},\n  \"oracle_fullread_req_disarm_last_prev_slot\": {},\n",
        FULLREAD_REQ_DISARM_COUNT.load(Ordering::SeqCst),
        FULLREAD_REQ_DISARM_LAST_PREV_SLOT.load(Ordering::SeqCst) as u32 as i32
    ));
    // LoadGame builder slot override. `overrides` > 0 means the save container's stored last-used
    // slot disagreed with the user's pick and we redirected the job to the pick -- i.e. the wrong
    // character WOULD have loaded. `last_native_slot` is the slot we replaced (u32-packed i32).
    // Read with `game_save_slot` / `oracle_char_name`: a correct run has the picked character in
    // world, and `overrides` tells you whether the container tried to send you elsewhere.
    body.push_str(&format!(
        "  \"oracle_loadgame_builder_slot_overrides\": {},\n  \"oracle_loadgame_builder_last_native_slot\": {},\n",
        LOADGAME_BUILDER_SLOT_OVERRIDES.load(Ordering::SeqCst),
        LOADGAME_BUILDER_LAST_NATIVE_SLOT.load(Ordering::SeqCst) as u32 as i32
    ));
    // Per-window portrait target latch. `retargets_suppressed` > 0 means the precedence ordering
    // tried to change the on-screen character mid-loading-screen and was refused -- each one is a
    // face change the user did NOT see. It was exactly 1 in the 2026-08-02 21:05 repro (0 -> 9).
    // `window_target_slot` is the committed slot +1, or 0 between windows.
    body.push_str(&format!(
        "  \"oracle_portrait_window_target_slot\": {},\n  \"oracle_portrait_window_retargets_suppressed\": {},\n",
        PORTRAIT_WINDOW_TARGET_SLOT.load(Ordering::SeqCst),
        PORTRAIT_WINDOW_RETARGETS_SUPPRESSED.load(Ordering::SeqCst)
    ));
    // Missing-save picker menu-open hold (bd er-effects-rs-ns4n follow-up): count > 0 proves the native
    // title auto-menu-open was suppressed while the pick was pending, so the menu rows build post-pick
    // with the save present. On a fast/early pick this stays 0 (nothing to suppress).
    body.push_str(&format!(
        "  \"oracle_title_open_menu_suppressed_count\": {},\n",
        TITLE_OPEN_MENU_SUPPRESSED_COUNT.load(Ordering::SeqCst)
    ));
    body.push_str(&format!(
        "  \"sq_repro_state\": {},\n  \"sq_repro_switch_index\": {},\n  \"sq_repro_profile_back_opened\": {},\n  \"sq_repro_profile_back_done\": {},\n  \"sq_repro_profile_back_restore_count\": {},\n  \"sq_repro_profile_back_final_tab\": {},\n  \"sq_repro_profile_back_baseline_mask\": {},\n  \"sq_repro_profile_back_verify_mask\": {},\n  \"sq_repro_profile_back_mismatch_mask\": {},\n  \"system_quit_optionsetting_direct_visible_reapply_count\": {},\n  \"system_quit_optionsetting_direct_visible_last_tab\": {},\n  \"system_quit_optionsetting_direct_visible_last_old_current\": {},\n  \"system_quit_optionsetting_direct_visible_last_selected\": {},\n  \"system_quit_optionsetting_direct_refresh_count\": {},\n  \"system_quit_optionsetting_direct_refresh_last_selected\": {},\n",
        SQ_REPRO_STATE.load(Ordering::SeqCst),
        SQ_REPRO_SWITCH_INDEX.load(Ordering::SeqCst),
        SQ_REPRO_PROFILE_BACK_OPENED.load(Ordering::SeqCst),
        SQ_REPRO_PROFILE_BACK_DONE.load(Ordering::SeqCst),
        SQ_REPRO_PROFILE_BACK_RESTORE_COUNT.load(Ordering::SeqCst),
        SQ_REPRO_PROFILE_BACK_FINAL_TAB.load(Ordering::SeqCst),
        SQ_REPRO_PROFILE_BACK_BASELINE_MASK.load(Ordering::SeqCst),
        SQ_REPRO_PROFILE_BACK_VERIFY_MASK.load(Ordering::SeqCst),
        SQ_REPRO_PROFILE_BACK_MISMATCH_MASK.load(Ordering::SeqCst),
        SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_REAPPLY_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_LAST_TAB.load(Ordering::SeqCst),
        format_scan_ptr(SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_LAST_OLD_CURRENT.load(Ordering::SeqCst)),
        format_scan_ptr(SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_LAST_SELECTED.load(Ordering::SeqCst)),
        SYSTEM_QUIT_OPTIONSETTING_DIRECT_REFRESH_COUNT.load(Ordering::SeqCst),
        format_scan_ptr(SYSTEM_QUIT_OPTIONSETTING_DIRECT_REFRESH_LAST_SELECTED.load(Ordering::SeqCst))
    ));
    body.push_str(&format!(
        "  \"system_quit_gaitem_reset_invocations\": {},\n  \"system_quit_gaitem_reset_released_count\": {},\n  \"system_quit_gaitem_reset_last_slack_before\": {},\n  \"system_quit_gaitem_reset_last_slack_after\": {},\n",
        SYSTEM_QUIT_GAITEM_RESET_INVOCATIONS.load(Ordering::SeqCst),
        SYSTEM_QUIT_GAITEM_RESET_RELEASED_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_GAITEM_RESET_LAST_SLACK_BEFORE.load(Ordering::SeqCst),
        SYSTEM_QUIT_GAITEM_RESET_LAST_SLACK_AFTER.load(Ordering::SeqCst)
    ));
    // SAVE-FLOW / SAVE-SUPPRESS oracles (save-game-flow WP1): suppression state and the
    // one-shot bypass counters come straight from the er-save-suppress crate accessors
    // (the product wires that crate's publish sink to a no-op because THIS writer is the
    // export path); the flow stage/counters are the product-side state machine. Probes
    // must key on oracle_save_flow_stage, NOT on msgbox/modal oracles, for save-flow
    // progress. oracle_save_bypass_final_status is null until the first bypassed save
    // reports a terminal status, then latched (0 = success).
    let bypass_final_status = er_save_suppress::bypass_final_status_raw();
    body.push_str(&format!(
        "  \"oracle_save_suppress_armed\": {},\n  \"oracle_save_suppress_submits_swallowed\": {},\n  \"oracle_save_suppress_submits_passed_through\": {},\n  \"oracle_save_suppress_status_faked\": {},\n  \"oracle_save_suppress_status_faked_idle\": {},\n  \"oracle_save_suppress_prologue_mismatches\": {},\n  \"oracle_save_suppress_settle_events\": {},\n  \"oracle_save_bypass_armed_total\": {},\n  \"oracle_save_bypass_allowed_total\": {},\n  \"oracle_save_bypass_allowed_failed_total\": {},\n  \"oracle_save_bypass_expired_total\": {},\n  \"oracle_save_bypass_final_status\": {},\n  \"oracle_save_flow_stage\": {},\n  \"oracle_save_flow_gate_latch_blocked\": {},\n  \"oracle_save_flow_commit_complete_count\": {},\n  \"oracle_save_flow_row_press_count\": {},\n  \"oracle_save_flow_commit_verify_fail_count\": {},\n",
        er_save_suppress::is_armed(),
        er_save_suppress::submits_swallowed(),
        er_save_suppress::submits_passed_through(),
        er_save_suppress::status_faked(),
        er_save_suppress::status_faked_idle(),
        er_save_suppress::prologue_mismatches(),
        er_save_suppress::settle_events(),
        er_save_suppress::bypass_armed_total(),
        er_save_suppress::bypass_allowed_total(),
        er_save_suppress::bypass_allowed_failed_total(),
        er_save_suppress::bypass_expired_total(),
        if bypass_final_status == er_save_suppress::BYPASS_FINAL_STATUS_NONE {
            "null".to_owned()
        } else {
            bypass_final_status.to_string()
        },
        SAVE_FLOW_STAGE.load(Ordering::SeqCst),
        SAVE_FLOW_GATE_LATCH_BLOCKED_COUNT.load(Ordering::SeqCst),
        SAVE_FLOW_COMMIT_COMPLETE_COUNT.load(Ordering::SeqCst),
        // One arm + one commit per press: this is what separates "the user pressed Save Game
        // twice" from "the flow armed the bypass twice for one press".
        SAVE_FLOW_ROW_PRESS_COUNT.load(Ordering::SeqCst),
        // The game said the save succeeded and the file check disagreed. Hard product failure.
        SAVE_FLOW_COMMIT_VERIFY_FAIL_COUNT.load(Ordering::SeqCst),
    ));
    // WRITE-COMPLETION oracles (2026-07-28). `oracle_save_job_*` come from the observer on
    // `SaveLoad2::SLSaveSession`'s job body: the SL worker picking a save up (`starts`),
    // putting it down (`completions`) and the result the game recorded for it
    // (`last_result`, 0 = success, 4294967295 = the result object was unreadable). They are
    // the reason a successful commit no longer has to wait for a poll consumer that may not
    // exist. Read them like this:
    //
    //   * `observer_installed == false` -> completion detection is back to the watchdog for
    //     the whole session; treat every commit in that run as degraded.
    //   * `final_status_source` names WHICH observation ended the last commit
    //     ("worker-job-completion", "native-poll", "native-enqueue-failed", or "none").
    //   * `commit_watchdog_count > 0` -> that many commits ended without ever being
    //     observed. That is a FAILURE of this instrumentation even when the file is fine,
    //     and `commit_job_start_tick` (the tick the writer began, 0 = never seen to start)
    //     is the first thing to look at.
    body.push_str(&format!(
        "  \"oracle_save_job_observer_installed\": {},\n  \"oracle_save_job_starts\": {},\n  \"oracle_save_job_completions\": {},\n  \"oracle_save_job_last_result\": {},\n  \"oracle_save_job_no_trampoline\": {},\n  \"oracle_save_bypass_final_status_source\": \"{}\",\n  \"oracle_save_bypass_completed_via_job\": {},\n  \"oracle_save_bypass_completed_via_poll\": {},\n  \"oracle_save_flow_commit_watchdog_count\": {},\n  \"oracle_save_flow_commit_job_start_tick\": {},\n",
        er_save_suppress::save_job_observer_installed(),
        er_save_suppress::save_job_starts(),
        er_save_suppress::save_job_completions(),
        er_save_suppress::save_job_last_result(),
        er_save_suppress::save_job_no_trampoline(),
        er_save_suppress::bypass_final_status_source_label(
            er_save_suppress::bypass_final_status_source()
        ),
        er_save_suppress::bypass_completed_via_job_total(),
        er_save_suppress::bypass_completed_via_poll_total(),
        SAVE_FLOW_COMMIT_WATCHDOG_COUNT.load(Ordering::SeqCst),
        SAVE_FLOW_COMMIT_JOB_START_TICK.load(Ordering::SeqCst),
    ));
    // WHICH WRITE BRANCH RAN (2026-07-29). `SaveLoad2::SLSaveSession`'s job body
    // `FUN_14240fd70` has two mutually exclusive write paths and picks between them on the
    // result of a probe (`FUN_142413230`) that mounts the container already on disk and asks
    // whether every supplied block still fits its existing entry:
    //
    //   * `oracle_save_write_in_place_calls` -- `FUN_1424142e0`, the per-block patcher, the
    //     branch taken when everything fits. Counts ONCE PER SUPPLIED BLOCK, so it normally
    //     climbs by more than one per save and its magnitude is not a save count.
    //   * `oracle_save_write_full_rebuild_calls` -- `FUN_142413860`, one whole-buffer write
    //     from offset 0, taken when a block outgrew its entry or no usable container existed.
    //     The decompile says this should be rare in the steady state; nothing had ever
    //     measured it, and a non-zero value on an ordinary repeat save falsifies that.
    //
    // Read `oracle_save_write_branch_observers_installed` FIRST. At 0 (or 1) the missing
    // observer's counter can only read 0, and that is the absence of an observation, NOT the
    // absence of a write. At 2, both counters reading 0 means NO SAVE WAS WRITTEN during the
    // run -- a third outcome, distinct from either branch having fired.
    //
    // `oracle_save_write_branch_no_trampoline` must stay 0. Non-zero means an observer ran
    // before its own trampoline was stored and reported the writers' failure code (6) rather
    // than falsely reporting success (0) -- the file is untouched, but that save was refused
    // by this instrument.
    body.push_str(&format!(
        "  \"oracle_save_write_branch_observers_installed\": {},\n  \"oracle_save_write_full_rebuild_calls\": {},\n  \"oracle_save_write_in_place_calls\": {},\n  \"oracle_save_write_branch_no_trampoline\": {},\n",
        er_save_suppress::write_branch_observers_installed(),
        er_save_suppress::write_full_rebuild_calls(),
        er_save_suppress::write_in_place_calls(),
        er_save_suppress::write_branch_no_trampoline(),
    ));
    // SAVE-DISPATCH ATTRIBUTION. These read the native chain BETWEEN "the request flags are
    // set" and "an SL enqueue arrives", which the enqueue-side counters above cannot see: a
    // save lane that returns 0 touches nothing, so the request stays latched, the dispatcher
    // re-enters it every frame, and `oracle_save_bypass_expired_total` is the only trace --
    // identical to the case where nothing consumed the request at all.
    //
    // Read them as: observers_installed == 0 -> the rest are meaningless (not "no dispatch");
    // dispatch_calls == 0 -> the dispatcher never ran; declines > 0 -> it ran and refused.
    //
    // serialize_calls is the alloc/serializer discriminator. A lane only calls
    // FUN_14067dc00 after its MainHeap buffer allocations have been null-checked, so
    // serialize_calls climbing PROVES the 0x280000 (and, on the combined lane, 0x60000)
    // allocations succeeded; declines climbing while serialize_calls stands still means the
    // lane bailed before the serializer.
    //
    // serialize_failures > 0 -> the character serializer is what refused, and
    // serialize_last_fail_step NAMES the step it refused at (decoded from
    // serialize_last_fail_bytes = _DAT_143d69920, the stream position where the cascade
    // stopped). The raw byte count is kept beside it. The step name is "byte-counter-
    // unreadable" only when the counter could not be read -- it is NOT a game outcome, and
    // in particular it does NOT mean the serializer's first gate rejected the call: that
    // gate is unreachable here (see SAVE_SERIALIZE_BYTES_RVA in er-save-suppress).
    body.push_str(&format!(
        "  \"oracle_save_dispatch_observers_installed\": {},\n  \"oracle_save_dispatch_calls\": {},\n  \"oracle_save_dispatch_declines\": {},\n  \"oracle_save_dispatch_declines_with_bypass\": {},\n  \"oracle_save_dispatch_last_lane\": {},\n  \"oracle_save_serialize_calls\": {},\n  \"oracle_save_serialize_failures\": {},\n  \"oracle_save_serialize_last_fail_bytes\": {},\n  \"oracle_save_serialize_last_fail_step\": \"{}\",\n",
        er_save_suppress::dispatch_observers_installed(),
        er_save_suppress::dispatch_calls(),
        er_save_suppress::dispatch_declines(),
        er_save_suppress::dispatch_declines_with_bypass(),
        er_save_suppress::dispatch_last_lane(),
        er_save_suppress::serialize_calls(),
        er_save_suppress::serialize_failures(),
        er_save_suppress::serialize_last_fail_bytes(),
        er_save_suppress::serialize_last_fail_step(),
    ));
    // THE SL REQUEST SLOT -- the operands of the submit builders' own precondition,
    // `iodev+0x10 == 0 && iodev+0x20 == 0` (`FUN_140e6ef60` / `FUN_140e6ec70`, 1.16.2).
    //
    // Read them ONLY when `oracle_save_dispatch_declines_with_bypass` is non-zero: that is
    // the case they were built for, a lane that allocated, serialized successfully, and
    // still returned 0 -- where the builder's other four operands are statically guaranteed
    // by the call site, so the guard can ONLY have failed on these two fields.
    //
    //   save-content-latched-0x10               a previous SAVE request was never released
    //   load-job-latched-0x18+0x20              a completed LOAD still owns the shared job slot
    //   orphan-job-latched-0x20                 a job whose poll never reached a terminal case
    //   precondition-clear-builder-alloc-refused the guard PASSED; the NetworkHeap alloc refused
    //
    // `oracle_save_swallow_release_left_dirty` is the self-incrimination oracle and is
    // decisive on its own: non-zero means THIS DLL's swallow left the precondition failing,
    // and `oracle_save_swallow_slot_after_*` names the field it left populated. Zero, with
    // swallows recorded, clears the swallow and points the finger at the load side.
    let slot_hex = |field: Option<usize>| -> String {
        field.map_or_else(|| "null".to_owned(), |value| format!("\"0x{value:x}\""))
    };
    let decline_slot = er_save_suppress::decline_slot();
    let swallow_after = er_save_suppress::swallow_slot_after();
    let swallow_before = er_save_suppress::swallow_slot_before();
    body.push_str(&format!(
        "  \"oracle_save_suppress_release_unavailable\": {},\n  \"oracle_save_swallow_release_left_dirty\": {},\n  \"oracle_save_swallow_iodev_mismatch\": {},\n  \"oracle_save_iodev_slot_read_failures\": {},\n  \"oracle_save_dispatch_last_decline_reason\": \"{}\",\n  \"oracle_save_decline_iodev_save_content\": {},\n  \"oracle_save_decline_iodev_load_content\": {},\n  \"oracle_save_decline_iodev_job\": {},\n  \"oracle_save_decline_iodev_file_cap\": {},\n  \"oracle_save_swallow_before_iodev_save_content\": {},\n  \"oracle_save_swallow_before_iodev_job\": {},\n  \"oracle_save_swallow_after_iodev_save_content\": {},\n  \"oracle_save_swallow_after_iodev_load_content\": {},\n  \"oracle_save_swallow_after_iodev_job\": {},\n  \"oracle_save_swallow_after_iodev_file_cap\": {},\n  \"oracle_save_flow_request_retractions\": {},\n  \"oracle_save_flow_retract_declined\": {},\n",
        er_save_suppress::release_unavailable(),
        er_save_suppress::swallow_release_left_dirty(),
        er_save_suppress::swallow_iodev_mismatch(),
        er_save_suppress::slot_read_failures(),
        er_save_suppress::decline_bail_reason_label(),
        slot_hex(decline_slot.map(|slot| slot.save_content)),
        slot_hex(decline_slot.map(|slot| slot.load_content)),
        slot_hex(decline_slot.map(|slot| slot.job)),
        slot_hex(decline_slot.map(|slot| slot.file_cap)),
        slot_hex(swallow_before.map(|slot| slot.save_content)),
        slot_hex(swallow_before.map(|slot| slot.job)),
        slot_hex(swallow_after.map(|slot| slot.save_content)),
        slot_hex(swallow_after.map(|slot| slot.load_content)),
        slot_hex(swallow_after.map(|slot| slot.job)),
        slot_hex(swallow_after.map(|slot| slot.file_cap)),
        SAVE_FLOW_REQUEST_RETRACTIONS.load(Ordering::SeqCst),
        SAVE_FLOW_RETRACT_DECLINED.load(Ordering::SeqCst),
    ));
    // THE LOAD CONSUMER -- the other owner of the shared `iodev+0x20` job, and the reason a
    // save can be refused by something that is not a save.
    //
    // A completed load is released only by its CONSUMER (`FUN_14067b100` ->
    // `FUN_140e6e380` -> `FUN_140e6f200`); `FUN_140e6e080` case 0x14 deliberately returns
    // success without releasing, and that same return is what drives `GameMan+0xb80` to
    // RESIDENT(3). The switch reload substitutes that consumer to feed the engine its own
    // sliced `.sl2` body, so it must run the native consumer for the device side effect --
    // these oracles say whether it did.
    //
    //   oracle_save_load_consumer_stranded > 0  DECISIVE FAILURE. A completed load kept the
    //                                           shared job and the payload was substituted
    //                                           anyway: from that moment no save in the
    //                                           process can be built, and
    //                                           oracle_save_dispatch_last_decline_reason
    //                                           latches at "load-job-latched-0x18+0x20".
    //   oracle_save_load_consumer_releases > 0  the slot was FREED, by the native consumer,
    //                                           at the substitution site.
    //   oracle_save_load_consumer_still_held    times the native guard kept a load that had
    //                                           not finished. This is the race oracle: the
    //                                           release is the game's own, so a live load is
    //                                           never taken -- a non-zero value here with a
    //                                           zero `stranded` means we asked early and
    //                                           correctly got nothing.
    let consumer_after = er_save_suppress::load_consumer_slot_after();
    body.push_str(&format!(
        "  \"oracle_save_load_consumer_calls\": {},\n  \"oracle_save_load_consumer_releases\": {},\n  \"oracle_save_load_consumer_still_held\": {},\n  \"oracle_save_load_consumer_stranded\": {},\n  \"oracle_save_load_consumer_last_outcome\": \"{}\",\n  \"oracle_save_load_consumer_after_iodev_load_content\": {},\n  \"oracle_save_load_consumer_after_iodev_job\": {},\n",
        er_save_suppress::load_consumer_calls(),
        er_save_suppress::load_consumer_releases(),
        er_save_suppress::load_consumer_still_held(),
        er_save_suppress::load_consumer_stranded(),
        er_save_suppress::load_consumer_last_outcome_label(),
        slot_hex(consumer_after.map(|slot| slot.load_content)),
        slot_hex(consumer_after.map(|slot| slot.job)),
    ));
    // SAVE-FLOW CONFIRM oracles. There is ONE confirm box in the flow -- "Are you sure you want to
    // overwrite this file?" -- so there is one set of counters. The three-box spelling
    // (`oracle_save_flow_box1/2/3_*`) is GONE with the two up-front confirms it described; a probe
    // written against those names will not silently read zeros from a renamed field, it will fail
    // to find them, which is the intended way to learn the flow changed.
    //
    // A save-flow probe must key on `oracle_save_flow_stage` + these counters, NOT on the msgbox
    // oracles: the confirm box is captured into the flow's OWN dialog slot and deliberately does
    // not feed `MSGBOX_LAST_DIALOG` / `oracle_blocking_modal_present`, so the startup auto-accept
    // can never reach a user-facing save confirm and an expected, wanted confirm never reads as a
    // blocking-modal failure.
    body.push_str(&format!(
        "  \"oracle_save_flow_overwrite_box_open_count\": {},\n  \"oracle_save_flow_overwrite_box_yes_count\": {},\n  \"oracle_save_flow_overwrite_box_no_count\": {},\n  \"oracle_save_flow_abort_count\": {},\n  \"oracle_save_flow_box_build_timeout_count\": {},\n  \"oracle_save_flow_recipe_unavailable\": {},\n  \"oracle_save_dest_overwrite_unconfirmable_count\": {},\n",
        SAVE_FLOW_BOX_OPEN_COUNTS[0].load(Ordering::SeqCst),
        SAVE_FLOW_BOX_YES_COUNTS[0].load(Ordering::SeqCst),
        SAVE_FLOW_BOX_NO_COUNTS[0].load(Ordering::SeqCst),
        SAVE_FLOW_ABORT_COUNT.load(Ordering::SeqCst),
        SAVE_FLOW_BOX_BUILD_TIMEOUT_COUNT.load(Ordering::SeqCst),
        SAVE_FLOW_RECIPE_UNAVAILABLE.load(Ordering::SeqCst),
        // Non-zero = a user chose an existing destination on a build whose confirm recipe failed
        // verification, and the overwrite was REFUSED rather than performed unconfirmed.
        SAVE_DEST_OVERWRITE_UNCONFIRMABLE_COUNT.load(Ordering::SeqCst),
    ));
    // CONFIRM-BOX FAILURE oracles (2026-07-28). `..._undecidable_count` is the box the DLL
    // could not read an answer out of; it is deliberately NOT folded into the No counts, so a
    // run can always tell "the user declined" from "we failed to read the user's answer".
    // Non-zero is a FAILURE even though nothing was written. `..._emit_count` is how many times
    // the `CS::MenuJob::EmitResult` observer attributed a native verdict to a live confirm box
    // (>= 1 per answered box once the hook is installed), and `..._emit_installed` says whether
    // that observer is live at all.
    body.push_str(&format!(
        "  \"oracle_save_flow_overwrite_box_undecidable_count\": {},\n  \"oracle_save_flow_box_identity_lost_count\": {},\n  \"oracle_save_flow_box_emit_count\": {},\n  \"oracle_save_flow_box_emit_installed\": {},\n  \"oracle_save_flow_enqueue_missing_count\": {},\n",
        SAVE_FLOW_BOX_UNDECIDABLE_COUNTS[0].load(Ordering::SeqCst),
        SAVE_FLOW_BOX_IDENTITY_LOST_COUNT.load(Ordering::SeqCst),
        SAVE_FLOW_BOX_EMIT_COUNT.load(Ordering::SeqCst),
        MENU_JOB_EMIT_RESULT_INSTALLED.load(Ordering::SeqCst),
        // Non-zero = a Save Game press fired but the save never reached the writer. A hard
        // failure: the user believes they saved and nothing was written.
        SAVE_FLOW_ENQUEUE_MISSING_COUNT.load(Ordering::SeqCst),
    ));
    // SAVE-DESTINATION oracles (save-game-flow WP3): the Box2-"No" browser and the scoped
    // write-open redirect that makes the chosen destination -- not the loaded save -- receive the
    // container the native writer emits. `redirect_hits` is one PER DIRTY BLOCK (the native
    // in-place writer opens the container once per block), so any positive count is normal and
    // zero is the failure. `seeded_count` is the fix that makes those block writes land in a real
    // container; `target_structure_ok` is the proof the result is a complete BND4, not merely a
    // file of the right length. `live_file_mutated` is the hard failure oracle that says the
    // redirect leaked and the loaded save was overwritten anyway, and `live_overwrite_count` names
    // the OPPOSITE case: a commit that was SUPPOSED to rewrite the loaded save.
    //
    // `picker_open_retry_count` is the reopen-loop oracle (bd `er-effects-rs-rsxi`): opens where NO
    // picker ran and the menu pump kept the request armed. A deferred in-game MenuJob submit is the
    // only legitimate source, so with the OS surface active this must read 0 -- a positive value
    // there means a terminal outcome (a Cancel) was retried, which reopens comdlg32 forever.
    //
    // PER-COMMIT vs CUMULATIVE, because reading one as the other is how a failed save reads as a
    // successful one. `redirect_hits`, `target_written_ok`, `target_structure_ok`,
    // `live_file_mutated`, `live_bak_mutated` and `live_stat_unreadable` describe THE COMMIT THAT
    // ARMED LAST and nothing before it -- `er_telemetry::counters::save_dest_reset_commit_verdicts`
    // clears all six at every arm. The process-wide history is in the `*_count` /`commit_fail` /
    // `restore_*` counters and in `live_file_mutated_total`, none of which are ever cleared.
    body.push_str(&format!(
        "  \"oracle_save_dest_picker_open_count\": {},\n  \"oracle_save_dest_picker_open_retry_count\": {},\n  \"oracle_save_dest_target_existing_count\": {},\n  \"oracle_save_dest_target_new_count\": {},\n  \"oracle_save_dest_commit_count\": {},\n  \"oracle_save_dest_cancel_count\": {},\n  \"oracle_save_dest_redirect_armed\": {},\n  \"oracle_save_dest_redirect_hits\": {},\n  \"oracle_save_dest_seeded_count\": {},\n  \"oracle_save_dest_seed_fail_count\": {},\n  \"oracle_save_dest_target_written_ok\": {},\n  \"oracle_save_dest_target_structure_ok\": {},\n  \"oracle_save_dest_commit_fail\": {},\n  \"oracle_save_dest_live_file_mutated\": {},\n  \"oracle_save_dest_live_file_mutated_total\": {},\n  \"oracle_save_dest_live_bak_mutated\": {},\n  \"oracle_save_dest_live_overwrite_count\": {},\n",
        SAVE_DEST_PICKER_OPEN_COUNT.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_DEST_PICKER_OPEN_RETRY_COUNT.load(Ordering::SeqCst),
        SAVE_DEST_TARGET_EXISTING_COUNT.load(Ordering::SeqCst),
        SAVE_DEST_TARGET_NEW_COUNT.load(Ordering::SeqCst),
        SAVE_DEST_COMMIT_COUNT.load(Ordering::SeqCst),
        SAVE_DEST_CANCEL_COUNT.load(Ordering::SeqCst),
        SAVE_DEST_REDIRECT_ARMED.load(Ordering::SeqCst),
        SAVE_DEST_REDIRECT_HITS.load(Ordering::SeqCst),
        SAVE_DEST_SEEDED_COUNT.load(Ordering::SeqCst),
        SAVE_DEST_SEED_FAIL_COUNT.load(Ordering::SeqCst),
        SAVE_DEST_TARGET_WRITTEN_OK.load(Ordering::SeqCst),
        SAVE_DEST_TARGET_STRUCTURE_OK.load(Ordering::SeqCst),
        SAVE_DEST_COMMIT_FAIL.load(Ordering::SeqCst),
        SAVE_DEST_LIVE_FILE_MUTATED.load(Ordering::SeqCst),
        er_telemetry::counters::SAVE_DEST_LIVE_FILE_MUTATED_TOTAL.load(Ordering::SeqCst),
        SAVE_DEST_LIVE_BAK_MUTATED.load(Ordering::SeqCst),
        SAVE_DEST_LIVE_OVERWRITE_COUNT.load(Ordering::SeqCst),
    ));
    // DESTINATION-COMMIT SAFETY ORACLES (2026-07-29). Every one names a decision the commit
    // refused to guess at, or a fact it could not establish, so a run can report it instead of
    // leaving it to be inferred from a file that changed when it should not have:
    //   * `identity_unknown_abort` / `no_writer_observer_abort` -- commits that did NOT fire;
    //   * `self_redirect_blocked` -- a destination proven to BE the loaded save under a different
    //     spelling, which the old string compare would have redirected onto itself;
    //   * `foreign_open_passed` -- write-opens of a same-named save container elsewhere on the
    //     machine that were left alone instead of rerouted into the user's destination;
    //   * `disarm_deferred` / `disarm_unproven` -- the redirect window waiting for the writer, and
    //     the one case where it had to be dropped without proof the writer ever ran;
    //   * `live_stat_unreadable` / `restore_suppressed` / `restore_failed` -- the loaded save's
    //     read-only guarantee, and every time the snapshot restore was declined or failed.
    // `degraded_*` describe a commit fired with suppression unarmed, whose completion comes from
    // the SL writer's own job signal because no bypass token exists to watch.
    body.push_str(&format!(
        "  \"oracle_save_dest_identity_unknown_abort\": {},\n  \"oracle_save_dest_self_redirect_blocked\": {},\n  \"oracle_save_dest_no_writer_observer_abort\": {},\n  \"oracle_save_dest_foreign_open_passed\": {},\n  \"oracle_save_dest_disarm_deferred\": {},\n  \"oracle_save_dest_disarm_unproven\": {},\n  \"oracle_save_dest_live_stat_unreadable\": {},\n  \"oracle_save_dest_restore_suppressed\": {},\n  \"oracle_save_dest_restore_failed\": {},\n  \"oracle_save_flow_degraded_fire\": {},\n  \"oracle_save_flow_degraded_complete_count\": {},\n  \"oracle_save_flow_degraded_unobserved_count\": {},\n",
        SAVE_DEST_IDENTITY_UNKNOWN_ABORT.load(Ordering::SeqCst),
        SAVE_DEST_SELF_REDIRECT_BLOCKED.load(Ordering::SeqCst),
        SAVE_DEST_NO_WRITER_OBSERVER_ABORT.load(Ordering::SeqCst),
        SAVE_DEST_FOREIGN_OPEN_PASSED.load(Ordering::SeqCst),
        SAVE_DEST_DISARM_DEFERRED.load(Ordering::SeqCst),
        SAVE_DEST_DISARM_UNPROVEN.load(Ordering::SeqCst),
        SAVE_DEST_LIVE_STAT_UNREADABLE.load(Ordering::SeqCst),
        SAVE_DEST_RESTORE_SUPPRESSED.load(Ordering::SeqCst),
        SAVE_DEST_RESTORE_FAILED.load(Ordering::SeqCst),
        SAVE_FLOW_DEGRADED_FIRE.load(Ordering::SeqCst),
        SAVE_FLOW_DEGRADED_COMPLETE_COUNT.load(Ordering::SeqCst),
        SAVE_FLOW_DEGRADED_UNOBSERVED_COUNT.load(Ordering::SeqCst),
    ));
    body.push_str(&format!(
        "  \"autoload_last_status\": {},\n",
        state.autoload.last_status().map_or_else(
            || "null".to_owned(),
            |status| format!("\"{}\"", json_escape(status))
        )
    ));
    write_game_man_telemetry(&mut body);
    write_save_redirect_telemetry(&mut body);
    write_save_data_snapshot_telemetry(&mut body);
    body.push_str(&format!(
        "  \"last_driver_command\": {}\n",
        state.last_driver_command.as_ref().map_or_else(
            || "null".to_owned(),
            |command| format!("\"{}\"", json_escape(command))
        )
    ));
    body.push_str("}\n");

    let tmp_path = path.with_extension("json.tmp");
    if fs::write(&tmp_path, body).is_ok() {
        let _ = fs::rename(tmp_path, path);
    }
}
