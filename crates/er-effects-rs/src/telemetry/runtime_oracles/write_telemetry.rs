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
    body.push_str(&format!("  \"network_sync\": {},\n", state.network_sync));
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
        "  \"oracle_own_load_stream_frames\": {},\n  \"oracle_own_load_stream_recur_frames\": {},\n  \"oracle_own_load_continue_fired\": {},\n  \"oracle_own_load_stream_owner_state\": {},\n  \"oracle_own_load_stream_owner_req_state\": {},\n  \"oracle_own_load_stream_mms_state\": {},\n  \"oracle_own_load_stream_block_count\": {},\n  \"oracle_own_load_stream_req_coord\": {},\n  \"oracle_own_load_stream_io_inflight\": {},\n  \"oracle_own_load_stream_io_reqhandle\": {},\n  \"oracle_own_load_stream_c30\": {},\n  \"oracle_own_load_stream_player_present\": {},\n  \"oracle_own_load_ingame_phase\": {},\n  \"oracle_own_load_req_blockid\": {},\n  \"oracle_own_load_target_block_present\": {},\n  \"oracle_own_load_wbr_update_calls\": {},\n  \"oracle_own_load_wbr_max_phase\": {},\n  \"oracle_own_load_wbr_any_gate_set\": {},\n  \"oracle_own_m28_dispatch_fired\": {},\n  \"oracle_own_load_install_job_fired\": {},\n  \"oracle_own_load_pump_fired\": {},\n  \"oracle_own_load_pump_state\": {},\n  \"oracle_own_load_pump_subcode\": {},\n  \"oracle_own_load_pump_done\": {},\n",
        crate::experiments::OWN_LOAD_STREAM_FRAMES.load(Ordering::SeqCst),
        crate::experiments::OWN_LOAD_STREAM_RECUR_FRAMES.load(Ordering::SeqCst),
        crate::experiments::OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst),
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
    body.push_str(&format!(
        "  \"oracle_save_picker_mode_active\": {},\n  \"oracle_save_picker_open_count\": {},\n  \"oracle_save_picker_repopulate_count\": {},\n  \"oracle_save_picker_pick_count\": {},\n  \"oracle_save_picker_pick_reject_count\": {},\n  \"oracle_save_picker_resubmit_count\": {},\n  \"oracle_save_picker_cancel_count\": {},\n  \"oracle_save_picker_staged_row_count\": {},\n",
        SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst),
        SAVE_PICKER_OPEN_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_REPOPULATE_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_PICK_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_PICK_REJECT_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_RESUBMIT_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_CANCEL_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_STAGED_ROW_COUNT.load(Ordering::SeqCst)
    ));
    body.push_str(&format!(
        "  \"oracle_save_picker_title_auto_opened\": {},\n  \"oracle_save_picker_title_open_count\": {},\n  \"oracle_save_picker_title_pick_count\": {},\n  \"oracle_save_picker_title_reload_count\": {},\n",
        SAVE_PICKER_TITLE_AUTO_OPENED.load(Ordering::SeqCst),
        SAVE_PICKER_TITLE_OPEN_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_TITLE_PICK_COUNT.load(Ordering::SeqCst),
        SAVE_PICKER_TITLE_RELOAD_COUNT.load(Ordering::SeqCst)
    ));
    body.push_str(&format!(
        "  \"system_quit_continue_confirm_fresh_deser_done\": {},\n  \"system_quit_continue_confirm_fresh_deser_count\": {},\n  \"system_quit_continue_confirm_block_count\": {},\n  \"system_quit_continue_confirm_allow_count\": {},\n",
        SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst),
        SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_CONTINUE_CONFIRM_BLOCK_COUNT.load(Ordering::SeqCst),
        SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT.load(Ordering::SeqCst)
    ));
    body.push_str(&format!(
        "  \"sq_repro_state\": {},\n  \"sq_repro_switch_index\": {},\n  \"sq_repro_paused_at_profile_select\": {},\n  \"sq_repro_profile_back_opened\": {},\n  \"sq_repro_profile_back_done\": {},\n  \"sq_repro_profile_back_restore_count\": {},\n  \"sq_repro_profile_back_final_tab\": {},\n  \"sq_repro_profile_back_baseline_mask\": {},\n  \"sq_repro_profile_back_verify_mask\": {},\n  \"sq_repro_profile_back_mismatch_mask\": {},\n  \"system_quit_optionsetting_direct_visible_reapply_count\": {},\n  \"system_quit_optionsetting_direct_visible_last_tab\": {},\n  \"system_quit_optionsetting_direct_visible_last_old_current\": {},\n  \"system_quit_optionsetting_direct_visible_last_selected\": {},\n  \"system_quit_optionsetting_direct_refresh_count\": {},\n  \"system_quit_optionsetting_direct_refresh_last_selected\": {},\n",
        SQ_REPRO_STATE.load(Ordering::SeqCst),
        SQ_REPRO_SWITCH_INDEX.load(Ordering::SeqCst),
        SQ_REPRO_PAUSED_AT_PROFILE_SELECT.load(Ordering::SeqCst),
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
        "  \"last_driver_command\": {},\n",
        state.last_driver_command.as_ref().map_or_else(
            || "null".to_owned(),
            |command| format!("\"{}\"", json_escape(command))
        )
    ));
    body.push_str("  \"calls\": [\n");
    for (index, call) in state.calls.iter().enumerate() {
        let comma = if index + NEXT_INDEX_OFFSET == state.calls.len() {
            ""
        } else {
            ","
        };
        body.push_str(&format!(
            "    {{\"index\": {index}, \"name\": \"{}\", \"kind\": \"{}\", \"enabled\": {}, \"active\": {}, \"apply_failed\": {}}}{comma}\n",
            json_escape(&call.name),
            json_escape(&call.kind.label()),
            call.enabled,
            call.active,
            call.apply_failed,
        ));
    }
    body.push_str("  ]\n}\n");

    let tmp_path = path.with_extension("json.tmp");
    if fs::write(&tmp_path, body).is_ok() {
        let _ = fs::rename(tmp_path, path);
    }
}

