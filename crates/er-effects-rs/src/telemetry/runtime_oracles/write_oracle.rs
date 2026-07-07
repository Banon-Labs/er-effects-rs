/// ORACLE reads for the proof bundle (per the goal): the LIVE in-world facts the harness asserts
/// on, independent of any agent narrative. Re-fetches the local player (the lib.rs player borrow
/// has ended before this runs). For a ZERO-INPUT run, `simulated_button_presses_total` MUST be 0;
/// `oracle_grounded` + a valid `oracle_block_id` + finite non-origin `oracle_havok_pos`
/// distinguish "in the playable world" from "frozen on a loading screen".
pub(crate) fn write_oracle_telemetry(body: &mut String) {
    write_title_menu_flow_oracles(body);
    write_game_module_oracles(body);
    write_player_presence_oracle(body);
}

fn format_optional_oracle_ptr(value: usize) -> String {
    if value == TITLE_OWNER_SCAN_START_ADDRESS {
        "null".to_owned()
    } else {
        format!("\"0x{value:x}\"")
    }
}

fn write_title_menu_flow_oracles(body: &mut String) {
    body.push_str(&format!(
        "  \"simulated_button_presses_total\": {},\n",
        crate::hooks::SIMULATED_INPUT_PRESSES_TOTAL.load(Ordering::SeqCst)
    ));
    let continue_task_node = MENU_CONTINUE_TASK_NODE.load(Ordering::SeqCst);
    let continue_member_node = MENU_CONTINUE_MEMBER_NODE.load(Ordering::SeqCst);
    let format_optional_ptr = format_optional_oracle_ptr;
    body.push_str(&format!(
        "  \"oracle_continue_task_node\": {},\n  \"oracle_continue_member_node\": {},\n  \"oracle_menu_window_ctor_hits\": {},\n  \"oracle_menu_window_ctor_semantic_hits\": {},\n  \"oracle_menu_window_ctor_last_item\": {},\n  \"oracle_menu_window_ctor_last_vt\": {},\n  \"oracle_menu_window_ctor_last_functor\": {},\n  \"oracle_menu_window_ctor_last_docall\": {},\n  \"oracle_menu_window_ctor_last_accept\": {},\n  \"oracle_menu_window_native_ctor_b_hits\": {},\n  \"oracle_menu_window_native_ctor_b_continue_hits\": {},\n  \"oracle_menu_window_native_ctor_b_last_caller_rva\": {},\n  \"oracle_menu_window_native_ctor_b_last_item\": {},\n  \"oracle_menu_window_native_ctor_b_last_out_slot\": {},\n  \"oracle_menu_window_native_ctor_b_last_vt\": {},\n  \"oracle_menu_window_native_ctor_b_last_functor\": {},\n  \"oracle_menu_window_native_ctor_b_last_docall\": {},\n  \"oracle_menu_window_native_ctor_b_last_accept\": {},\n  \"oracle_menu_window_idle_ctor_hits\": {},\n  \"oracle_menu_window_idle_ctor_continue_hits\": {},\n  \"oracle_menu_window_idle_ctor_continue_last_caller_rva\": {},\n  \"oracle_menu_window_idle_ctor_continue_last_item\": {},\n  \"oracle_menu_window_idle_ctor_continue_last_out_slot\": {},\n  \"oracle_menu_window_idle_ctor_continue_last_docall\": {},\n  \"oracle_menu_window_idle_ctor_continue_last_accept\": {},\n  \"oracle_menu_continue_idle_insert_hits\": {},\n  \"oracle_menu_continue_idle_insert_last_caller_rva\": {},\n  \"oracle_menu_continue_idle_insert_last_arg0\": {},\n  \"oracle_menu_continue_idle_insert_last_arg1\": {},\n  \"oracle_menu_continue_idle_insert_last_ret\": {},\n  \"oracle_menu_continue_idle_insert_last_arg1_update_rva\": {},\n  \"oracle_menu_continue_idle_insert_last_ret_update_rva\": {},\n  \"oracle_task_enqueue_generic_hits\": {},\n  \"oracle_task_enqueue_generic_last_caller_rva\": {},\n  \"oracle_task_enqueue_generic_last_arg0\": {},\n  \"oracle_task_enqueue_generic_last_arg0_pointee\": {},\n  \"oracle_task_enqueue_generic_last_arg1\": {},\n  \"oracle_task_enqueue_generic_last_ret\": {},\n  \"oracle_task_enqueue_generic_sample0_caller_rva\": {},\n  \"oracle_task_enqueue_generic_sample0_arg0\": {},\n  \"oracle_task_enqueue_generic_sample0_arg0_pointee\": {},\n  \"oracle_task_enqueue_generic_sample0_arg1\": {},\n  \"oracle_task_enqueue_generic_sample0_ret\": {},\n  \"oracle_task_enqueue_generic_sample1_caller_rva\": {},\n  \"oracle_task_enqueue_generic_sample1_arg0\": {},\n  \"oracle_task_enqueue_generic_sample1_arg0_pointee\": {},\n  \"oracle_task_enqueue_generic_sample1_arg1\": {},\n  \"oracle_task_enqueue_generic_sample1_ret\": {},\n  \"oracle_task_enqueue_generic_idle_item_match_hits\": {},\n  \"oracle_task_enqueue_generic_idle_item_last_match_kind\": {},\n  \"oracle_menu_window_idle_ctor_last_caller_rva\": {},\n  \"oracle_menu_window_idle_ctor_last_item\": {},\n  \"oracle_menu_window_idle_ctor_last_vt\": {},\n  \"oracle_menu_window_idle_ctor_last_functor\": {},\n  \"oracle_menu_window_idle_ctor_last_docall\": {},\n  \"oracle_menu_window_idle_ctor_last_accept\": {},\n  \"oracle_menu_item_update_hits\": {},\n  \"oracle_menu_item_update_semantic_hits\": {},\n  \"oracle_menu_item_update_last_item\": {},\n  \"oracle_menu_item_update_last_vt\": {},\n  \"oracle_menu_item_update_last_functor\": {},\n  \"oracle_menu_item_update_last_docall\": {},\n  \"oracle_menu_item_update_last_accept\": {},\n  \"oracle_menu_continue_candidate_item\": {},\n  \"oracle_menu_continue_candidate_hits\": {},\n  \"oracle_menu_continue_candidate_idle_accept_hits\": {},\n  \"oracle_menu_continue_candidate_native_accept_hits\": {},\n  \"oracle_menu_continue_candidate_other_accept_hits\": {},\n  \"oracle_menu_continue_candidate_accept_changes\": {},\n  \"oracle_menu_continue_candidate_last_accept\": {},\n  \"oracle_title_native_ready_hits\": {},\n  \"oracle_title_native_ready_last_caller_rva\": {},\n  \"oracle_title_native_ready_last_this\": {},\n  \"oracle_title_native_ready_last_vtable\": {},\n  \"oracle_title_native_ready_last_getter\": {},\n  \"oracle_title_native_ready_last_object\": {},\n  \"oracle_title_native_ready_last_flags\": {},\n  \"oracle_title_native_ready_last_masked\": {},\n  \"oracle_title_native_ready_last_ret\": {},\n  \"oracle_title_langselect_ready_last_object\": {},\n  \"oracle_title_langselect_ready_last_flags\": {},\n  \"oracle_title_langselect_ready_last_masked\": {},\n  \"oracle_title_langselect_ready_last_ret\": {},\n",
        format_optional_ptr(continue_task_node),
        format_optional_ptr(continue_member_node),
        MENU_WINDOW_JOB_CTOR_HITS.load(Ordering::SeqCst),
        MENU_WINDOW_JOB_CTOR_SEMANTIC_HITS.load(Ordering::SeqCst),
        format_optional_ptr(MENU_WINDOW_JOB_CTOR_LAST_ITEM.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_CTOR_LAST_VT.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_CTOR_LAST_FUNCTOR.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_CTOR_LAST_DOCALL.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_CTOR_LAST_ACCEPT.load(Ordering::SeqCst)),
        MENU_WINDOW_JOB_NATIVE_CTOR_B_HITS.load(Ordering::SeqCst),
        MENU_WINDOW_JOB_NATIVE_CTOR_B_CONTINUE_HITS.load(Ordering::SeqCst),
        format_optional_ptr(MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_ITEM.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_OUT_SLOT.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_VT.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_FUNCTOR.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_DOCALL.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_ACCEPT.load(Ordering::SeqCst)),
        MENU_WINDOW_JOB_IDLE_CTOR_HITS.load(Ordering::SeqCst),
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_HITS.load(Ordering::SeqCst),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_ITEM.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_OUT_SLOT.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_DOCALL.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_ACCEPT.load(Ordering::SeqCst)),
        MENU_CONTINUE_IDLE_INSERT_HITS.load(Ordering::SeqCst),
        format_optional_ptr(MENU_CONTINUE_IDLE_INSERT_LAST_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_CONTINUE_IDLE_INSERT_LAST_ARG0.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_CONTINUE_IDLE_INSERT_LAST_ARG1.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_CONTINUE_IDLE_INSERT_LAST_RET.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_CONTINUE_IDLE_INSERT_LAST_ARG1_UPDATE_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_CONTINUE_IDLE_INSERT_LAST_RET_UPDATE_RVA.load(Ordering::SeqCst)),
        TASK_ENQUEUE_GENERIC_HITS.load(Ordering::SeqCst),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_LAST_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_LAST_ARG0.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_LAST_ARG0_POINTEE.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_LAST_ARG1.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_LAST_RET.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE0_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE0_ARG0.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE0_ARG0_POINTEE.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE0_ARG1.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE0_RET.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE1_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE1_ARG0.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE1_ARG0_POINTEE.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE1_ARG1.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE1_RET.load(Ordering::SeqCst)),
        TASK_ENQUEUE_GENERIC_IDLE_ITEM_MATCH_HITS.load(Ordering::SeqCst),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_IDLE_ITEM_LAST_MATCH_KIND.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_LAST_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_LAST_ITEM.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_LAST_VT.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_LAST_FUNCTOR.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_LAST_DOCALL.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_LAST_ACCEPT.load(Ordering::SeqCst)),
        MENU_ITEM_UPDATE_HITS.load(Ordering::SeqCst),
        MENU_ITEM_UPDATE_SEMANTIC_HITS.load(Ordering::SeqCst),
        format_optional_ptr(MENU_ITEM_UPDATE_LAST_ITEM.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_ITEM_UPDATE_LAST_VT.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_ITEM_UPDATE_LAST_FUNCTOR.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_ITEM_UPDATE_LAST_DOCALL.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_ITEM_UPDATE_LAST_ACCEPT.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_CONTINUE_CANDIDATE_ITEM.load(Ordering::SeqCst)),
        MENU_CONTINUE_CANDIDATE_HITS.load(Ordering::SeqCst),
        MENU_CONTINUE_CANDIDATE_IDLE_ACCEPT_HITS.load(Ordering::SeqCst),
        MENU_CONTINUE_CANDIDATE_NATIVE_ACCEPT_HITS.load(Ordering::SeqCst),
        MENU_CONTINUE_CANDIDATE_OTHER_ACCEPT_HITS.load(Ordering::SeqCst),
        MENU_CONTINUE_CANDIDATE_ACCEPT_CHANGES.load(Ordering::SeqCst),
        format_optional_ptr(MENU_CONTINUE_CANDIDATE_LAST_ACCEPT.load(Ordering::SeqCst)),
        TITLE_NATIVE_READY_PREDICATE_HITS.load(Ordering::SeqCst),
        format_optional_ptr(TITLE_NATIVE_READY_PREDICATE_LAST_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(TITLE_NATIVE_READY_PREDICATE_LAST_THIS.load(Ordering::SeqCst)),
        format_optional_ptr(TITLE_NATIVE_READY_PREDICATE_LAST_VTABLE.load(Ordering::SeqCst)),
        format_optional_ptr(TITLE_NATIVE_READY_PREDICATE_LAST_GETTER.load(Ordering::SeqCst)),
        format_optional_ptr(TITLE_NATIVE_READY_PREDICATE_LAST_OBJECT.load(Ordering::SeqCst)),
        TITLE_NATIVE_READY_PREDICATE_LAST_FLAGS.load(Ordering::SeqCst),
        TITLE_NATIVE_READY_PREDICATE_LAST_MASKED.load(Ordering::SeqCst),
        TITLE_NATIVE_READY_PREDICATE_LAST_RET.load(Ordering::SeqCst),
        format_optional_ptr(TITLE_NATIVE_READY_PREDICATE_LAST_OBJECT.load(Ordering::SeqCst)),
        TITLE_NATIVE_READY_PREDICATE_LAST_FLAGS.load(Ordering::SeqCst),
        TITLE_NATIVE_READY_PREDICATE_LAST_MASKED.load(Ordering::SeqCst),
        TITLE_NATIVE_READY_PREDICATE_LAST_RET.load(Ordering::SeqCst)
    ));
    body.push_str(&format!(
        "  \"oracle_native_submit_hits\": {},\n  \"oracle_native_submit_last_result\": {},\n  \"oracle_result_event_handler_hits\": {},\n  \"oracle_result_action_builder_hits\": {},\n  \"oracle_result_event_last_result\": {},\n  \"oracle_result_event_last_event\": {},\n  \"oracle_result_event_last_raw_qword0\": {},\n  \"oracle_result_event_last_fd4_code\": {},\n  \"oracle_result_event_last_fd4_arg\": {},\n  \"oracle_result_action_last_result\": {},\n  \"oracle_result_action_last_event\": {},\n  \"oracle_result_action_last_word0\": {},\n  \"oracle_result_action_last_word1\": {},\n  \"oracle_result_action_insert_hits\": {},\n  \"oracle_result_action_last_insert_arg0\": {},\n  \"oracle_result_action_last_insert_arg1\": {},\n  \"oracle_result_action_last_insert_ret\": {},\n  \"oracle_result_action_last_insert_arg1_update_rva\": {},\n  \"oracle_result_action_last_insert_ret_update_rva\": {},\n  \"oracle_result_action_wrapper_builder_hits\": {},\n  \"oracle_result_action_last_wrapper_builder_rcx\": {},\n  \"oracle_result_action_last_wrapper_builder_rdx\": {},\n  \"oracle_result_action_last_wrapper_builder_r8\": {},\n  \"oracle_result_action_last_wrapper_builder_ret\": {},\n  \"oracle_result_action_last_wrapper_builder_ret_update_rva\": {},\n",
        NATIVE_SUBMIT_HITS.load(Ordering::SeqCst),
        format_optional_ptr(NATIVE_SUBMIT_LAST_RESULT.load(Ordering::SeqCst)),
        RESULT_EVENT_HANDLER_HITS.load(Ordering::SeqCst),
        RESULT_ACTION_BUILDER_HITS.load(Ordering::SeqCst),
        format_optional_ptr(RESULT_EVENT_LAST_RESULT.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_EVENT_LAST_EVENT.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_EVENT_LAST_RAW_QWORD0.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_EVENT_LAST_FD4_CODE.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_EVENT_LAST_FD4_ARG.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_RESULT.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_EVENT.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_WORD0.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_WORD1.load(Ordering::SeqCst)),
        RESULT_ACTION_INSERT_HITS.load(Ordering::SeqCst),
        format_optional_ptr(RESULT_ACTION_LAST_INSERT_ARG0.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_INSERT_ARG1.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_INSERT_RET.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_INSERT_ARG1_UPDATE_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_INSERT_RET_UPDATE_RVA.load(Ordering::SeqCst)),
        RESULT_ACTION_WRAPPER_BUILDER_HITS.load(Ordering::SeqCst),
        format_optional_ptr(RESULT_ACTION_LAST_WRAPPER_BUILDER_RCX.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_WRAPPER_BUILDER_RDX.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_WRAPPER_BUILDER_R8.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_WRAPPER_BUILDER_RET.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_WRAPPER_BUILDER_RET_UPDATE_RVA.load(Ordering::SeqCst))
    ));
    body.push_str(&format!(
        // NOTE: oracle_continue_deser_fired / oracle_continue_confirmed were REMOVED
        // (2026-06-24): they tracked OWN_STEPPER_DESER_FIRED/OWN_STEPPER_CONFIRMED -- the
        // own_stepper/native_continue confirm-FIRE chain -- NOT whether the character loaded.
        // The default zero-input autoload (pab-advance + title-accept-byte natural menu-open)
        // loads without that chain, so the fields read 0 on success and were repeatedly misread
        // as "load failed". The real load semaphore is world_loaded (player_present + world_stable
        // + saved_map_c30), already emitted below. The backing statics stay (they gate block_input
        // release + own_stepper STAGE2).
        "  \"oracle_continue_phase\": {},\n  \"oracle_continue_expected_slot\": {},\n  \"oracle_continue_mount_c30\": {},\n  \"oracle_continue_guard_waits\": {},\n",
        FULLREAD_PHASE.load(Ordering::SeqCst),
        OWN_STEPPER_EXPECTED_SLOT.load(Ordering::SeqCst),
        OWN_STEPPER_MOUNT_C30.load(Ordering::SeqCst),
        FULLREAD_DRAIN_WAITS.load(Ordering::SeqCst)
    ));
}

fn write_player_presence_oracle(body: &mut String) {
    const BLOCK_ID_NONE: i32 = -1;
    if let Ok(player) = unsafe { PlayerIns::local_player_mut() } {
        let pos = player.chr_ins.modules.physics.position;
        let grounded = player.chr_ins.modules.physics.standing_on_solid_ground;
        let block = player.current_block_id.0;
        let bp = player.block_position;
        let chr_model_ins_ptr = player.chr_ins.chr_model_ins.as_ptr() as usize;
        let chr_ctrl_ptr = player.chr_ins.chr_ctrl.as_ptr() as usize;
        let chr_draw_group_enabled = player.chr_ins.load_state.draw_group_enabled();
        let chr_render_group_enabled = player.chr_ins.chr_flags1c4.is_render_group_enabled();
        let chr_onscreen = player.chr_ins.chr_flags1c4.is_onscreen();
        let chr_enable_render = player.chr_ins.chr_flags1c5.enable_render();
        let player_render_ready = chr_model_ins_ptr != TITLE_OWNER_SCAN_START_ADDRESS
            && chr_ctrl_ptr != TITLE_OWNER_SCAN_START_ADDRESS
            && chr_draw_group_enabled
            && chr_render_group_enabled
            && chr_enable_render;
        body.push_str(&format!(
            "  \"oracle_player_present\": true,\n  \"oracle_havok_pos\": [{}, {}, {}],\n  \"oracle_grounded\": {},\n  \"oracle_block_id\": {},\n  \"oracle_block_id_valid\": {},\n  \"oracle_block_pos\": [{}, {}, {}],\n  \"oracle_chr_model_ins_present\": {},\n  \"oracle_chr_ctrl_present\": {},\n  \"oracle_chr_draw_group_enabled\": {},\n  \"oracle_chr_render_group_enabled\": {},\n  \"oracle_chr_onscreen\": {},\n  \"oracle_chr_enable_render\": {},\n  \"oracle_player_render_ready\": {},\n",
            pos.0,
            pos.1,
            pos.2,
            grounded,
            block,
            block != BLOCK_ID_NONE,
            bp.x,
            bp.y,
            bp.z,
            chr_model_ins_ptr != TITLE_OWNER_SCAN_START_ADDRESS,
            chr_ctrl_ptr != TITLE_OWNER_SCAN_START_ADDRESS,
            chr_draw_group_enabled,
            chr_render_group_enabled,
            chr_onscreen,
            chr_enable_render,
            player_render_ready
        ));
    } else {
        body.push_str("  \"oracle_player_present\": false,\n");
    }
}
