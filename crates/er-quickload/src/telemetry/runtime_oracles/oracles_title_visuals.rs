// Title-screen visual oracles: message-box / EULA-policy / server-status state, the native title
// visual and logo suppression, the title menu resource + Scaleform acquisition counters, PRESS
// START binding, the profile summary, and the custom title cover jobs.
//
// This is the largest single subsystem and it does NOT split further, which is a property of the
// source rather than a decision: its ~200 locals feed two enormous `format!` emissions, so any cut
// inside it would have to thread most of those locals through a signature. The one value that does
// escape -- `title_custom_cover_profile_source_ready` -- is returned, because the native-profile
// capture field emitted at the very end of the telemetry reports it.

/// Returns `title_custom_cover_profile_source_ready` for the native-profile capture field.
fn write_title_visual_oracles(body: &mut String, base: usize) -> bool {
    const NULL_PTR: usize = 0;
    let msgbox_total_builds = MSGBOX_BUILDER_LOG.load(Ordering::SeqCst);
    let msgbox_dialog = MSGBOX_LAST_DIALOG.load(Ordering::SeqCst);
    let msgbox_vtable = if msgbox_dialog == NULL_PTR {
        NULL_PTR
    } else {
        unsafe { crate::experiments::safe_read_usize(msgbox_dialog) }.unwrap_or(NULL_PTR)
    };
    let msgbox_closing_latch = if msgbox_vtable
        == er_game_base::mem::game_data_addr(
            base,
            MSGBOX_DIALOG_VTABLE_RVA,
            "MSGBOX_DIALOG_VTABLE_RVA",
        ) {
        unsafe {
            crate::experiments::safe_read_usize(msgbox_dialog + MSGBOX_CLOSING_LATCH_3B0_OFFSET)
        }
        .map(|value| value & MSGBOX_LATCH_BYTE_MASK)
        .unwrap_or(MSGBOX_CLOSING_YES)
    } else {
        MSGBOX_CLOSING_YES
    };
    let blocking_modal_present = msgbox_vtable
        == er_game_base::mem::game_data_addr(
            base,
            MSGBOX_DIALOG_VTABLE_RVA,
            "MSGBOX_DIALOG_VTABLE_RVA",
        )
        && msgbox_closing_latch != MSGBOX_CLOSING_YES;
    const NO_POLICY_BUILDS: usize = MENU_TRACE_UNSEEN_SEQ;
    let policy_total_builds = POLICY_TOS_TITLE_TOTAL_BUILDS.load(Ordering::SeqCst);
    let policy_any_seen = policy_total_builds != NO_POLICY_BUILDS;
    let policy_ptr = POLICY_TOS_TITLE_LAST_THIS.load(Ordering::SeqCst);
    let policy_vtable = POLICY_TOS_TITLE_LAST_VTABLE.load(Ordering::SeqCst);
    let policy_arg_rdx = POLICY_TOS_TITLE_LAST_ARG_RDX.load(Ordering::SeqCst);
    let policy_arg_r8 = POLICY_TOS_TITLE_LAST_ARG_R8.load(Ordering::SeqCst);
    let policy_arg_r9 = POLICY_TOS_TITLE_LAST_ARG_R9.load(Ordering::SeqCst);
    let policy_stack_arg0 = POLICY_TOS_TITLE_LAST_STACK_ARG0.load(Ordering::SeqCst);
    let policy_backing_flag_ptr = POLICY_TOS_TITLE_LAST_BACKING_FLAG_PTR.load(Ordering::SeqCst);
    let policy_stored_backing_flag_ptr =
        POLICY_TOS_TITLE_LAST_STORED_BACKING_FLAG_PTR.load(Ordering::SeqCst);
    let policy_backing_flag_value = POLICY_TOS_TITLE_LAST_BACKING_FLAG_VALUE.load(Ordering::SeqCst);
    let policy_requested_flag_value =
        POLICY_TOS_TITLE_LAST_REQUESTED_FLAG_VALUE.load(Ordering::SeqCst);
    let policy_caller_rva = POLICY_TOS_TITLE_LAST_CALLER_RVA.load(Ordering::SeqCst);
    let policy_wrapper_hits = POLICY_TOS_TITLE_WRAPPER_HITS.load(Ordering::SeqCst);
    let policy_wrapper_record = POLICY_TOS_TITLE_WRAPPER_LAST_RECORD.load(Ordering::SeqCst);
    let policy_wrapper_original_this =
        POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_THIS.load(Ordering::SeqCst);
    let policy_wrapper_original_vtable =
        POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_VTABLE.load(Ordering::SeqCst);
    let policy_wrapper_record_id = POLICY_TOS_TITLE_WRAPPER_LAST_RECORD_ID.load(Ordering::SeqCst);
    let policy_wrapper_stack_arg0 = POLICY_TOS_TITLE_WRAPPER_LAST_STACK_ARG0.load(Ordering::SeqCst);
    let policy_wrapper_backing_flag_ptr =
        POLICY_TOS_TITLE_WRAPPER_LAST_BACKING_FLAG_PTR.load(Ordering::SeqCst);
    let policy_wrapper_ret = POLICY_TOS_TITLE_WRAPPER_LAST_RET.load(Ordering::SeqCst);
    let policy_wrapper_caller_rva = POLICY_TOS_TITLE_WRAPPER_LAST_CALLER_RVA.load(Ordering::SeqCst);
    let policy_selector_hits = POLICY_TOS_SELECTOR_WRAPPER_HITS.load(Ordering::SeqCst);
    let policy_selector_record = POLICY_TOS_SELECTOR_WRAPPER_LAST_RECORD.load(Ordering::SeqCst);
    let policy_selector_original_this =
        POLICY_TOS_SELECTOR_WRAPPER_LAST_ORIGINAL_THIS.load(Ordering::SeqCst);
    let policy_selector_original_vtable =
        POLICY_TOS_SELECTOR_WRAPPER_LAST_ORIGINAL_VTABLE.load(Ordering::SeqCst);
    let policy_selector_owner = POLICY_TOS_SELECTOR_WRAPPER_LAST_OWNER.load(Ordering::SeqCst);
    let policy_selector_requested_flag =
        POLICY_TOS_SELECTOR_WRAPPER_LAST_REQUESTED_FLAG.load(Ordering::SeqCst);
    let policy_selector_arg = POLICY_TOS_SELECTOR_WRAPPER_LAST_SELECTOR_ARG.load(Ordering::SeqCst);
    let policy_selector_ret = POLICY_TOS_SELECTOR_WRAPPER_LAST_RET.load(Ordering::SeqCst);
    let policy_selector_caller_rva =
        POLICY_TOS_SELECTOR_WRAPPER_LAST_CALLER_RVA.load(Ordering::SeqCst);
    let policy_selector_ctor_hits = POLICY_TOS_SELECTOR_CTOR_HITS.load(Ordering::SeqCst);
    let policy_selector_ctor_this = POLICY_TOS_SELECTOR_CTOR_LAST_THIS.load(Ordering::SeqCst);
    let policy_selector_ctor_vtable = POLICY_TOS_SELECTOR_CTOR_LAST_VTABLE.load(Ordering::SeqCst);
    let policy_selector_ctor_owner = POLICY_TOS_SELECTOR_CTOR_LAST_OWNER.load(Ordering::SeqCst);
    let policy_selector_ctor_requested_flag_ptr =
        POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_PTR.load(Ordering::SeqCst);
    let policy_selector_ctor_requested_flag_value =
        POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_VALUE.load(Ordering::SeqCst);
    let policy_selector_ctor_selector_arg =
        POLICY_TOS_SELECTOR_CTOR_LAST_SELECTOR_ARG.load(Ordering::SeqCst);
    let policy_selector_ctor_stored_selector_arg =
        POLICY_TOS_SELECTOR_CTOR_LAST_STORED_SELECTOR_ARG.load(Ordering::SeqCst);
    let policy_selector_ctor_stored_requested_flag_ptr =
        POLICY_TOS_SELECTOR_CTOR_LAST_STORED_REQUESTED_FLAG_PTR.load(Ordering::SeqCst);
    let policy_selector_ctor_ret = POLICY_TOS_SELECTOR_CTOR_LAST_RET.load(Ordering::SeqCst);
    let policy_selector_ctor_caller_rva =
        POLICY_TOS_SELECTOR_CTOR_LAST_CALLER_RVA.load(Ordering::SeqCst);
    let policy_status_hits = POLICY_TOS_STATUS_HITS.load(Ordering::SeqCst);
    let policy_status_this = POLICY_TOS_STATUS_LAST_THIS.load(Ordering::SeqCst);
    let policy_status_owner = POLICY_TOS_STATUS_LAST_OWNER.load(Ordering::SeqCst);
    let policy_status_flag_ptr = POLICY_TOS_STATUS_LAST_FLAG_PTR.load(Ordering::SeqCst);
    let policy_status_flag_value = POLICY_TOS_STATUS_LAST_FLAG_VALUE.load(Ordering::SeqCst);
    let policy_status_ret = POLICY_TOS_STATUS_LAST_RET.load(Ordering::SeqCst);
    let policy_status_caller_rva = POLICY_TOS_STATUS_LAST_CALLER_RVA.load(Ordering::SeqCst);
    let policy_flag_setter_hits = POLICY_TOS_FLAG_SETTER_HITS.load(Ordering::SeqCst);
    let policy_flag_setter_owner = POLICY_TOS_FLAG_SETTER_LAST_OWNER.load(Ordering::SeqCst);
    let policy_flag_setter_value = POLICY_TOS_FLAG_SETTER_LAST_VALUE.load(Ordering::SeqCst);
    let policy_flag_setter_force = POLICY_TOS_FLAG_SETTER_LAST_FORCE.load(Ordering::SeqCst);
    let policy_flag_setter_before = POLICY_TOS_FLAG_SETTER_LAST_BEFORE.load(Ordering::SeqCst);
    let policy_flag_setter_after = POLICY_TOS_FLAG_SETTER_LAST_AFTER.load(Ordering::SeqCst);
    let policy_flag_setter_caller_rva =
        POLICY_TOS_FLAG_SETTER_LAST_CALLER_RVA.load(Ordering::SeqCst);
    let server_status_total_seen = SERVER_STATUS_TOTAL_SEEN.load(Ordering::SeqCst);
    let server_status_any_seen = server_status_total_seen != NO_POLICY_BUILDS;
    let server_status_state = SERVER_STATUS_LAST_STATE.load(Ordering::SeqCst);
    let server_status_text_id = SERVER_STATUS_LAST_TEXT_ID.load(Ordering::SeqCst);
    let title_visual_suppress_installed = TITLE_NATIVE_MENU_VISUAL_SUPPRESS_INSTALLED
        .load(Ordering::SeqCst)
        == TITLE_NATIVE_MENU_VISUAL_SUPPRESS_INSTALLED_YES;
    let title_visual_suppressed_builds =
        TITLE_NATIVE_MENU_VISUAL_SUPPRESSED_BUILDS.load(Ordering::SeqCst);
    let title_visual_last_out_slot = TITLE_NATIVE_MENU_VISUAL_LAST_OUT_SLOT.load(Ordering::SeqCst);
    let title_visual_last_prev_out = TITLE_NATIVE_MENU_VISUAL_LAST_PREV_OUT.load(Ordering::SeqCst);
    let title_visual_last_arg_rdx = TITLE_NATIVE_MENU_VISUAL_LAST_ARG_RDX.load(Ordering::SeqCst);
    let title_visual_last_arg_r8 = TITLE_NATIVE_MENU_VISUAL_LAST_ARG_R8.load(Ordering::SeqCst);
    let title_visual_last_caller_rva =
        TITLE_NATIVE_MENU_VISUAL_LAST_CALLER_RVA.load(Ordering::SeqCst);
    let title_visual_native_job = TITLE_NATIVE_MENU_VISUAL_NATIVE_JOB.load(Ordering::SeqCst);
    let title_visual_native_window = TITLE_NATIVE_MENU_VISUAL_NATIVE_WINDOW.load(Ordering::SeqCst);
    let title_visual_render_suppress_installed = TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED
        .load(Ordering::SeqCst)
        == TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED_YES;
    let title_visual_render_suppressed_windows =
        TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESSED_WINDOWS.load(Ordering::SeqCst);
    let title_visual_render_last_window =
        TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_WINDOW.load(Ordering::SeqCst);
    let title_visual_render_last_flags_before =
        TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_FLAGS_BEFORE.load(Ordering::SeqCst);
    let title_visual_render_last_flags_after =
        TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_FLAGS_AFTER.load(Ordering::SeqCst);
    let title_visual_render_last_caller_rva =
        TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_CALLER_RVA.load(Ordering::SeqCst);
    let (
        title_visual_current_menu_id,
        title_visual_current_flags,
        title_visual_current_draw_bit_set,
    ) = title_menu_window_id_flags(base, title_visual_native_window);
    // Actual visible logo surface telemetry: `TitleBackViewParts` / `05_001_Title_Logo` is an
    // embedded object at TitleTopDialog+0xaa8, separate from the preserved `05_000_Title`
    // MenuWindowJob. A real portrait cover depends on post-SL2 profile_summary readiness and the
    // SYSTEX_Menu_Profile render pipeline, so expose both in RAM telemetry before any mutation.
    // STALE-DIALOG UAF GUARD (er-effects-rs-3pc, ROOT fix 2026-07-03). `title_logo_gfx_current_frame`
    // CALLS a virtual on the title dialog's BackViewParts GFX handle. The title logo only exists at
    // the title screen; once we have loaded into a world that stored dialog is FREED (and, on every
    // character switch, freed+rebuilt). A freed object keeps its vtable, and worse, its reused
    // vtable+8 slot can point at a VALID-BUT-WRONG game function (observed: the factory FUN_1411d10f0),
    // so the earlier `vtable_in_game_image` check passes and the call still derefs freed memory ->
    // access violation deep in the game (crash write_oracle_telemetry -> game+0x11d10f3). You cannot
    // safely virtual-call a maybe-freed object. So skip this GFX walk entirely once in-world: the
    // oracle is a boot-title diagnostic and is meaningless (and unsafe) after the first load. This is
    // what actually surfaced as the "crash on opening escape after N switches" -- the telemetry tick,
    // not the menu, dereferencing the stale title dialog.
    let in_world = IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES;
    let title_logo_dialog = PRODUCT_CORE_LAST_TITLE_DIALOG.load(Ordering::SeqCst);
    let title_logo_back_view_parts = if !in_world
        && title_logo_dialog != NULL_PTR
        && title_logo_dialog != TITLE_OWNER_SCAN_START_ADDRESS
    {
        title_logo_dialog + TITLE_LOGO_BACK_VIEW_PARTS_AA8_OFFSET
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let title_logo_back_view_parts_vtable =
        if title_logo_back_view_parts != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { crate::experiments::safe_read_usize(title_logo_back_view_parts) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
    let title_logo_gfx_frame =
        if title_logo_back_view_parts_vtable != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { title_logo_gfx_current_frame(base, title_logo_back_view_parts) }
        } else {
            TITLE_LOGO_GFX_UNKNOWN_FRAME
        };
    let title_logo_gfx_alpha_mult_term = title_logo_gfx_alpha_for_frame(title_logo_gfx_frame);
    let title_logo_gfx_visibility = title_logo_gfx_alpha_mult_term > 0;
    let title_logo_gfx_hide_calls = TITLE_LOGO_GFX_HIDE_CALLS.load(Ordering::SeqCst);
    let title_logo_gfx_hide_last_dialog = TITLE_LOGO_GFX_HIDE_LAST_DIALOG.load(Ordering::SeqCst);
    let title_logo_gfx_hide_last_logo = TITLE_LOGO_GFX_HIDE_LAST_LOGO.load(Ordering::SeqCst);
    let title_logo_gfx_hide_last_caller_phase =
        TITLE_LOGO_GFX_HIDE_LAST_CALLER_PHASE.load(Ordering::SeqCst);
    let title_logo_gfx_hide_last_requested_visible =
        TITLE_LOGO_GFX_HIDE_LAST_REQUESTED_VISIBLE.load(Ordering::SeqCst);
    let title_menu_resource_acquire_installed =
        TITLE_MENU_RESOURCE_ACQUIRE_INSTALLED.load(Ordering::SeqCst) != 0;
    let title_menu_resource_acquire_hits = TITLE_MENU_RESOURCE_ACQUIRE_HITS.load(Ordering::SeqCst);
    let title_menu_resource_acquire_logo_hits =
        TITLE_MENU_RESOURCE_ACQUIRE_LOGO_HITS.load(Ordering::SeqCst);
    let title_menu_resource_acquire_last_this =
        TITLE_MENU_RESOURCE_ACQUIRE_LAST_THIS.load(Ordering::SeqCst);
    let title_menu_resource_acquire_last_load_params =
        TITLE_MENU_RESOURCE_ACQUIRE_LAST_LOAD_PARAMS.load(Ordering::SeqCst);
    let title_menu_resource_acquire_last_filename_ptr =
        TITLE_MENU_RESOURCE_ACQUIRE_LAST_FILENAME_PTR.load(Ordering::SeqCst);
    let title_menu_resource_acquire_last_param3 =
        TITLE_MENU_RESOURCE_ACQUIRE_LAST_PARAM3.load(Ordering::SeqCst);
    let title_menu_resource_acquire_last_ret =
        TITLE_MENU_RESOURCE_ACQUIRE_LAST_RET.load(Ordering::SeqCst);
    let title_menu_resource_acquire_last_caller_rva =
        TITLE_MENU_RESOURCE_ACQUIRE_LAST_CALLER_RVA.load(Ordering::SeqCst);
    let title_scaleform_file_open_installed =
        TITLE_SCALEFORM_FILE_OPEN_INSTALLED.load(Ordering::SeqCst) != 0;
    let title_scaleform_file_open_hits = TITLE_SCALEFORM_FILE_OPEN_HITS.load(Ordering::SeqCst);
    let title_scaleform_file_open_logo_hits =
        TITLE_SCALEFORM_FILE_OPEN_LOGO_HITS.load(Ordering::SeqCst);
    let title_scaleform_file_open_last_loader =
        TITLE_SCALEFORM_FILE_OPEN_LAST_LOADER.load(Ordering::SeqCst);
    let title_scaleform_file_open_last_url_ptr =
        TITLE_SCALEFORM_FILE_OPEN_LAST_URL_PTR.load(Ordering::SeqCst);
    let title_scaleform_file_open_last_flags =
        TITLE_SCALEFORM_FILE_OPEN_LAST_FLAGS.load(Ordering::SeqCst);
    let title_scaleform_file_open_last_ret =
        TITLE_SCALEFORM_FILE_OPEN_LAST_RET.load(Ordering::SeqCst);
    let title_scaleform_file_open_last_ret_vtable =
        TITLE_SCALEFORM_FILE_OPEN_LAST_RET_VTABLE.load(Ordering::SeqCst);
    let title_scaleform_file_open_last_caller_rva =
        TITLE_SCALEFORM_FILE_OPEN_LAST_CALLER_RVA.load(Ordering::SeqCst);
    // PINNED: the env-driven memory-GFX loader that fed these two is deleted (it had been an
    // inert no-op since 2026-07-19), so both have always emitted 0.
    let title_scaleform_memory_gfx_bytes = 0usize;
    let title_scaleform_memory_gfx_replacements =
        TITLE_SCALEFORM_MEMORY_GFX_REPLACEMENTS.load(Ordering::SeqCst);
    let title_scaleform_05_000_memory_gfx_replacements =
        TITLE_SCALEFORM_05_000_MEMORY_GFX_REPLACEMENTS.load(Ordering::SeqCst);
    let title_scaleform_memory_gfx_failures = 0usize;
    let title_scaleform_memory_gfx_last_file =
        TITLE_SCALEFORM_MEMORY_GFX_LAST_FILE.load(Ordering::SeqCst);
    let title_scaleform_resource_ctor_installed =
        TITLE_SCALEFORM_RESOURCE_CTOR_INSTALLED.load(Ordering::SeqCst) != 0;
    let title_scaleform_resource_ctor_hits =
        TITLE_SCALEFORM_RESOURCE_CTOR_HITS.load(Ordering::SeqCst);
    let title_scaleform_resource_ctor_logo_hits =
        TITLE_SCALEFORM_RESOURCE_CTOR_LOGO_HITS.load(Ordering::SeqCst);
    let title_scaleform_resource_ctor_last_out =
        TITLE_SCALEFORM_RESOURCE_CTOR_LAST_OUT.load(Ordering::SeqCst);
    let title_scaleform_resource_ctor_last_url_ptr =
        TITLE_SCALEFORM_RESOURCE_CTOR_LAST_URL_PTR.load(Ordering::SeqCst);
    let title_scaleform_resource_ctor_last_file =
        TITLE_SCALEFORM_RESOURCE_CTOR_LAST_FILE.load(Ordering::SeqCst);
    let title_scaleform_resource_ctor_last_ret =
        TITLE_SCALEFORM_RESOURCE_CTOR_LAST_RET.load(Ordering::SeqCst);
    let title_scaleform_resource_ctor_last_movie_data =
        TITLE_SCALEFORM_RESOURCE_CTOR_LAST_MOVIE_DATA.load(Ordering::SeqCst);
    let title_scaleform_resource_ctor_last_caller_rva =
        TITLE_SCALEFORM_RESOURCE_CTOR_LAST_CALLER_RVA.load(Ordering::SeqCst);
    let title_press_start_gfx_hide_calls = TITLE_PRESS_START_GFX_HIDE_CALLS.load(Ordering::SeqCst);
    let title_press_start_gfx_hide_last_dialog =
        TITLE_PRESS_START_GFX_HIDE_LAST_DIALOG.load(Ordering::SeqCst);
    let title_press_start_gfx_hide_last_proxy =
        TITLE_PRESS_START_GFX_HIDE_LAST_PROXY.load(Ordering::SeqCst);
    let title_press_start_gfx_hide_last_context =
        TITLE_PRESS_START_GFX_HIDE_LAST_CONTEXT.load(Ordering::SeqCst);
    let title_press_start_gfx_hide_last_caller_phase =
        TITLE_PRESS_START_GFX_HIDE_LAST_CALLER_PHASE.load(Ordering::SeqCst);
    let title_press_start_gfx_value = TITLE_PRESS_START_GFX_VALUE.load(Ordering::SeqCst);
    let title_press_start_gfx_force_false_calls =
        TITLE_PRESS_START_GFX_FORCE_FALSE_CALLS.load(Ordering::SeqCst);
    let title_press_start_gfx_force_false_last_value =
        TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_VALUE.load(Ordering::SeqCst);
    let title_press_start_gfx_force_false_last_requested =
        TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_REQUESTED.load(Ordering::SeqCst);
    let title_press_start_bind_hits = TITLE_PRESS_START_BIND_HITS.load(Ordering::SeqCst);
    let title_press_start_bind_last_parent =
        TITLE_PRESS_START_BIND_LAST_PARENT.load(Ordering::SeqCst);
    let title_press_start_bind_last_out = TITLE_PRESS_START_BIND_LAST_OUT.load(Ordering::SeqCst);
    let title_press_start_bind_last_name = TITLE_PRESS_START_BIND_LAST_NAME.load(Ordering::SeqCst);
    let title_press_start_bind_last_context =
        TITLE_PRESS_START_BIND_LAST_CONTEXT.load(Ordering::SeqCst);
    let title_press_start_bind_hide_calls =
        TITLE_PRESS_START_BIND_HIDE_CALLS.load(Ordering::SeqCst);
    // REMOVED (2026-07-31): the `oracle_title_overlay_cover_*` family and
    // `oracle_title_profile_cover_bound_to_logo_surface`. All six backing counters had ZERO
    // writers anywhere in the tree and the bound-to-logo-surface value was a hard-coded
    // `false` -- they were placeholders for the unbuilt custom title render surface
    // (er-effects-rs-trp). Emitting them made an ABSENT feature look like a FAILING one: a
    // reader could not tell "the title cover rendered nothing" from "nothing was ever wired to
    // count", and that is exactly how they misread -- an agent cited render_calls=0 as proof
    // the title cover painted no pixels during a loading-screen gap. When trp lands its real
    // surface, add these back WITH writers at the actual render site.
    let title_logo_profile_summary = {
        let game_data_man = crate::game_data_man_ptr_or_null();
        if game_data_man != NULL_PTR {
            unsafe {
                crate::experiments::safe_read_usize(game_data_man + SLOT_MANAGER_CONTAINER_OFFSET)
            }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        }
    };
    let title_logo_profile_summary_ready = title_logo_profile_summary
        != TITLE_OWNER_SCAN_START_ADDRESS
        && title_logo_profile_summary != NULL_PTR;
    let title_profile_render_refresh_gate_ready = unsafe {
        product_core_autoload_ready(
            PRODUCT_CORE_LAST_OWNER.load(Ordering::SeqCst),
            base,
            game_man_ptr_or_null(),
            OWN_STEPPER_SLOT_ZERO,
        )
    }
    .is_some();
    let title_profile_face_bind_hits = TITLE_PROFILE_FACE_BIND_HITS.load(Ordering::SeqCst);
    // PINNED TO THEIR SHIPPED VALUES (bd er-effects-rs-57fw). The only writer of
    // TITLE_PROFILE_FACE_TRANSFORM_APPLIED / TITLE_PROFILE_FACE_OTHER_HIDDEN was
    // title_custom_cover_menu_window_run_hook, whose only address-taker
    // (install_title_custom_cover_run_hook) had no callers -- so rustc never codegen'd either and
    // both counters read 0 in every build that has ever shipped. Emitting the same literals keeps
    // this JSON byte-identical while the counters go away. Consequence, tracked separately:
    // `title_loaded_character_portrait_rendered` below is STRUCTURALLY false, not merely unobserved.
    let title_profile_face_transform_applied = false;
    let title_profile_face_other_hidden = 0usize;
    let title_profile_face_last_proxy = TITLE_PROFILE_FACE_LAST_PROXY.load(Ordering::SeqCst);
    let title_profile_face_last_value = TITLE_PROFILE_FACE_LAST_VALUE.load(Ordering::SeqCst);
    // `title_loaded_character_portrait_rendered` and the two oracles derived from it were REMOVED
    // 2026-08-31. They were not merely unobserved -- they could not be true: of the five AND-terms,
    // `title_profile_face_transform_applied` and `title_profile_face_other_hidden` are pinned `false`
    // and `0` just above (their writer was never codegen'd), and the fifth read
    // TITLE_CUSTOM_COVER_RUN_CALLS, which had no write site anywhere in the tree. A permanent `false`
    // from a conjunction that cannot evaluate true is worse than no oracle: it reads as "the portrait
    // did not render", which is the exact defect scripts/check-oracle-writers.py exists to catch.
    // Re-derive them when title_custom_cover_menu_window_run_hook is genuinely installed and writing.
    let title_scaleform_bind_observer_hits =
        TITLE_SCALEFORM_BIND_OBSERVER_HITS.load(Ordering::SeqCst);
    let title_scaleform_bind_observer_systex_hits =
        TITLE_SCALEFORM_BIND_OBSERVER_SYSTEX_HITS.load(Ordering::SeqCst);
    let title_scaleform_bind_observer_last_owner =
        TITLE_SCALEFORM_BIND_OBSERVER_LAST_OWNER.load(Ordering::SeqCst);
    let title_scaleform_bind_observer_last_pair =
        TITLE_SCALEFORM_BIND_OBSERVER_LAST_PAIR.load(Ordering::SeqCst);
    let title_scaleform_bind_observer_last_symbol_ptr =
        TITLE_SCALEFORM_BIND_OBSERVER_LAST_SYMBOL_PTR.load(Ordering::SeqCst);
    let title_scaleform_bind_observer_last_target_ptr =
        TITLE_SCALEFORM_BIND_OBSERVER_LAST_TARGET_PTR.load(Ordering::SeqCst);
    // The six `oracle_title_portrait_visible_surface_*` keys were REMOVED 2026-08-31. Nothing in the
    // tree ever rewrote the profile visible-surface bind: only the SYMBOL constant was ever used
    // (title_resources_stats_text.rs), and all four counters behind `_bind_rewrites`, `_bound`,
    // `_bind_last_owner`, `_bind_last_pair` and `_bind_last_symbol_ptr` had no write site. They were
    // long-standing entries on scripts/oracle-writers-allowlist.txt; `_bind_rewrites` in particular
    // was copied into the loading-screen-portrait event JSON by scripts/er-readiness-watch.py, so a
    // permanent 0 sat beside the screenshot a human reads. Re-add them WITH the rewrite that fills
    // them if the bind is ever implemented.
    let now_loading_helper_hooks_installed =
        NOW_LOADING_HELPER_HOOKS_INSTALLED.load(Ordering::SeqCst);
    let now_loading_helper_ctor_hits = NOW_LOADING_HELPER_CTOR_HITS.load(Ordering::SeqCst);
    let now_loading_helper_update_hits = NOW_LOADING_HELPER_UPDATE_HITS.load(Ordering::SeqCst);
    let now_loading_helper_last_this = NOW_LOADING_HELPER_LAST_THIS.load(Ordering::SeqCst);
    let now_loading_helper_last_menu_index =
        NOW_LOADING_HELPER_LAST_MENU_INDEX.load(Ordering::SeqCst);
    let now_loading_helper_last_replace_tex_info =
        NOW_LOADING_HELPER_LAST_REPLACE_TEX_INFO.load(Ordering::SeqCst);
    let now_loading_helper_last_requested_replace_tex_info =
        NOW_LOADING_HELPER_LAST_REQUESTED_REPLACE_TEX_INFO.load(Ordering::SeqCst);
    let now_loading_helper_last_flags = NOW_LOADING_HELPER_LAST_FLAGS.load(Ordering::SeqCst);
    let loadscreen_table_builds = PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst);
    let loading_bg_portrait_gx_nonblack = LOADING_BG_PORTRAIT_NONBLACK.load(Ordering::SeqCst) != 0;
    let loading_bg_portrait_is_checker = LOADING_BG_PORTRAIT_IS_CHECKER.load(Ordering::SeqCst) != 0;
    let portrait_render_drive_hits = PROFILE_RENDER_DRIVE_HITS.load(Ordering::SeqCst);
    let loading_bg_portrait_gx_dims = LOADING_BG_PORTRAIT_DIMS.load(Ordering::SeqCst);
    let loading_bg_portrait_gx_format = LOADING_BG_PORTRAIT_FORMAT.load(Ordering::SeqCst);
    let title_custom_cover_profile_render_refresh_calls =
        TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_CALLS.load(Ordering::SeqCst);
    let title_custom_cover_profile_render_refresh_last_profile_summary =
        TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_LAST_PROFILE_SUMMARY.load(Ordering::SeqCst);
    let title_custom_cover_profile_render_refresh_last_caller_phase =
        TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_LAST_CALLER_PHASE.load(Ordering::SeqCst);
    let title_custom_cover_profile_source_sample_calls =
        TITLE_CUSTOM_COVER_PROFILE_SOURCE_SAMPLE_CALLS.load(Ordering::SeqCst);
    let title_custom_cover_profile_source_slot =
        TITLE_CUSTOM_COVER_PROFILE_SOURCE_SLOT.load(Ordering::SeqCst);
    let title_custom_cover_profile_source_renderer =
        TITLE_CUSTOM_COVER_PROFILE_SOURCE_RENDERER.load(Ordering::SeqCst);
    let title_custom_cover_profile_source_renderer_vtable =
        TITLE_CUSTOM_COVER_PROFILE_SOURCE_RENDERER_VTABLE.load(Ordering::SeqCst);
    let title_custom_cover_profile_source_offscreen_rend =
        TITLE_CUSTOM_COVER_PROFILE_SOURCE_OFFSCREEN_REND.load(Ordering::SeqCst);
    let title_custom_cover_profile_source_tex_rescap =
        TITLE_CUSTOM_COVER_PROFILE_SOURCE_TEX_RESCAP.load(Ordering::SeqCst);
    let title_custom_cover_profile_source_tex_index =
        TITLE_CUSTOM_COVER_PROFILE_SOURCE_TEX_INDEX.load(Ordering::SeqCst);
    let title_custom_cover_profile_source_ready_754 =
        TITLE_CUSTOM_COVER_PROFILE_SOURCE_READY_754.load(Ordering::SeqCst);
    let title_custom_cover_profile_source_ready_755 =
        TITLE_CUSTOM_COVER_PROFILE_SOURCE_READY_755.load(Ordering::SeqCst);
    let title_custom_cover_profile_source_ready = title_custom_cover_profile_source_renderer_vtable
        == er_game_base::mem::game_data_addr(
            base,
            TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
            "TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA",
        )
        && title_custom_cover_profile_source_offscreen_rend != TITLE_OWNER_SCAN_START_ADDRESS
        && title_custom_cover_profile_source_offscreen_rend != NULL_PTR
        && title_custom_cover_profile_source_tex_rescap != TITLE_OWNER_SCAN_START_ADDRESS
        && title_custom_cover_profile_source_tex_rescap != NULL_PTR;
    let title_custom_cover_profile_select_builds =
        TITLE_CUSTOM_COVER_PROFILE_SELECT_BUILDS.load(Ordering::SeqCst);
    let title_custom_cover_profile_select_last_ret =
        TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_RET.load(Ordering::SeqCst);
    let title_custom_cover_profile_select_last_job =
        TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_JOB.load(Ordering::SeqCst);
    let title_custom_cover_profile_select_last_caller_rva =
        TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_CALLER_RVA.load(Ordering::SeqCst);
    let title_custom_cover_black_builds = TITLE_CUSTOM_COVER_BLACK_BUILDS.load(Ordering::SeqCst);
    let title_custom_cover_black_last_ret =
        TITLE_CUSTOM_COVER_BLACK_LAST_RET.load(Ordering::SeqCst);
    let title_custom_cover_black_last_job =
        TITLE_CUSTOM_COVER_BLACK_LAST_JOB.load(Ordering::SeqCst);
    let title_custom_cover_black_last_caller_rva =
        TITLE_CUSTOM_COVER_BLACK_LAST_CALLER_RVA.load(Ordering::SeqCst);
    // The whole `title_custom_cover_run_*` family was REMOVED 2026-08-31. Its sole writer
    // (title_custom_cover_menu_window_run_hook) was never codegen'd -- its only address-taker had no
    // callers -- so `_last_native_job`/`_last_cover_job`/`_last_cover_window`/`_last_ret` were already
    // pinned literals and `_calls` was a counter nothing wrote. Six JSON keys that could only ever
    // report absence, indistinguishable from the hook running and finding nothing.
    let title_pab_information_visual_builds =
        TITLE_PAB_INFORMATION_VISUAL_BUILDS.load(Ordering::SeqCst);
    let title_pab_information_visual_last_job =
        TITLE_PAB_INFORMATION_VISUAL_LAST_JOB.load(Ordering::SeqCst);
    let title_pab_information_visual_last_window =
        TITLE_PAB_INFORMATION_VISUAL_LAST_WINDOW.load(Ordering::SeqCst);
    let title_pab_information_visual_last_caller_rva =
        TITLE_PAB_INFORMATION_VISUAL_LAST_CALLER_RVA.load(Ordering::SeqCst);
    // The first branch here used to prefer `title_custom_cover_run_last_cover_window`, one of the
    // pinned-literal `title_custom_cover_run_*` values removed 2026-08-31. It was pinned `0usize`,
    // which IS `NULL_PTR`, so the branch could never be taken -- a dead preference that read like a
    // live one.
    let title_custom_cover_black_cover_window = if title_custom_cover_black_last_job != NULL_PTR
        && title_custom_cover_black_last_job != TITLE_OWNER_SCAN_START_ADDRESS
    {
        unsafe { crate::experiments::safe_read_usize(title_custom_cover_black_last_job + 0x130) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let (
        title_custom_cover_black_cover_menu_id,
        title_custom_cover_black_cover_flags,
        title_custom_cover_black_cover_draw_bit_set,
    ) = title_menu_window_id_flags(base, title_custom_cover_black_cover_window);
    let (
        title_pab_information_visual_current_menu_id,
        title_pab_information_visual_current_flags,
        title_pab_information_visual_current_draw_bit_set,
    ) = title_menu_window_id_flags(base, title_pab_information_visual_last_window);
    // `title_custom_cover_black_exclusive_visible` removed 2026-08-31 with the run-calls counter it
    // AND-ed against: that term was permanently 0, so this could never be true either.
    // Latched peak-load proof. `oracle_load_correctness_seen > 0` proves a REAL character
    // reached the world this run, latched so a quit-to-title (which resets the live
    // oracle_char_* fields) cannot erase it.
    let loaded_peak_seen = LOADED_PEAK_SEEN_COUNT.load(Ordering::SeqCst);
    let loaded_peak_level = LOADED_PEAK_LEVEL.load(Ordering::SeqCst);
    let loaded_peak_c30 = LOADED_PEAK_C30.load(Ordering::SeqCst) as u32;
    let loaded_peak_name_len = LOADED_PEAK_NAME_LEN.load(Ordering::SeqCst);
    let loaded_peak_name = LOADED_PEAK_NAME
        .lock()
        .map(|latched| latched.clone())
        .unwrap_or_default();
    body.push_str(&format!(
        "  \"oracle_load_correctness_seen\": {loaded_peak_seen},\n  \"oracle_loaded_peak_level\": {loaded_peak_level},\n  \"oracle_loaded_peak_c30\": \"0x{loaded_peak_c30:x}\",\n  \"oracle_loaded_peak_name\": \"{}\",\n  \"oracle_loaded_peak_name_len\": {loaded_peak_name_len},\n",
        json_escape(&loaded_peak_name)
    ));
    // RELOAD-SCOPED MessageBoxDialog oracle. `oracle_msgbox_total_builds` is a PROCESS-lifetime
    // total, so "0 before the reload, 1 after" and "1 at boot, 0 across the reload" read the same
    // at teardown. AGENTS.md's bar -- "Product proof requires zero MessageBoxDialog builds" -- is
    // about the CHARACTER LOAD, so score the DELTA since the System->Quit->Load-Character switch
    // armed. `-1` on both fields means no switch has armed in this process, which is "not proven",
    // NOT "proven zero"; the pass condition for a reload is
    // `oracle_msgbox_builds_since_switch_arm == 0`.
    let msgbox_arm_baseline =
        er_telemetry_core::counters::MSGBOX_BUILDS_AT_SWITCH_ARM.load(Ordering::SeqCst);
    let (msgbox_arm_baseline_json, msgbox_builds_since_arm) = if msgbox_arm_baseline == usize::MAX {
        (-1i64, -1i64)
    } else {
        (
            msgbox_arm_baseline as i64,
            msgbox_total_builds.saturating_sub(msgbox_arm_baseline) as i64,
        )
    };
    body.push_str(&format!(
        "  \"oracle_msgbox_switch_arm_baseline\": {msgbox_arm_baseline_json},\n  \"oracle_msgbox_builds_since_switch_arm\": {msgbox_builds_since_arm},\n"
    ));
    // WORLD-LOST: the second-load teardown as a gated number. Non-zero means a genuinely loaded
    // world reverted to the title map during this run -- the black screen. Published so a checker
    // can FAIL on it; a counter read by nobody is decoration, not a semaphore.
    body.push_str(&format!(
        "  \"oracle_world_lost_to_title\": {},\n  \"oracle_switch_return_title_request_retired\": {},\n",
        er_telemetry_core::counters::WORLD_LOST_TO_TITLE_COUNT.load(Ordering::SeqCst),
        er_telemetry_core::counters::SWITCH_RETURN_TITLE_REQUEST_RETIRED_COUNT.load(Ordering::SeqCst)
    ));
    body.push_str(&format!(
        "  \"oracle_msgbox_total_builds\": {},\n  \"oracle_blocking_modal_present\": {},\n  \"oracle_blocking_modal_ptr\": {},\n  \"oracle_blocking_modal_vtable\": {},\n  \"oracle_blocking_modal_closing_latch\": {},\n  \"oracle_policy_window_total_builds\": {},\n  \"oracle_policy_window_any_seen\": {},\n  \"oracle_policy_window_ptr\": {},\n  \"oracle_policy_window_vtable\": {},\n  \"oracle_policy_window_args\": [{}, {}, {}, {}, {}],\n  \"oracle_policy_window_stack_arg0\": {},\n  \"oracle_policy_window_backing_flag_ptr\": {},\n  \"oracle_policy_window_stored_backing_flag_ptr\": {},\n  \"oracle_policy_window_backing_flag_value\": {},\n  \"oracle_policy_window_requested_flag_value\": {},\n  \"oracle_policy_window_caller_rva\": {},\n  \"oracle_policy_ctor_wrapper_hits\": {},\n  \"oracle_policy_ctor_wrapper_record\": {},\n  \"oracle_policy_ctor_wrapper_original_this\": {},\n  \"oracle_policy_ctor_wrapper_original_vtable\": {},\n  \"oracle_policy_ctor_wrapper_record_id\": {},\n  \"oracle_policy_ctor_wrapper_stack_arg0\": {},\n  \"oracle_policy_ctor_wrapper_backing_flag_ptr\": {},\n  \"oracle_policy_ctor_wrapper_ret\": {},\n  \"oracle_policy_ctor_wrapper_caller_rva\": {},\n  \"oracle_policy_selector_wrapper_hits\": {},\n  \"oracle_policy_selector_wrapper_record\": {},\n  \"oracle_policy_selector_wrapper_original_this\": {},\n  \"oracle_policy_selector_wrapper_original_vtable\": {},\n  \"oracle_policy_selector_wrapper_owner\": {},\n  \"oracle_policy_selector_wrapper_requested_flag\": {},\n  \"oracle_policy_selector_wrapper_selector_arg\": {},\n  \"oracle_policy_selector_wrapper_ret\": {},\n  \"oracle_policy_selector_wrapper_caller_rva\": {},\n  \"oracle_policy_selector_ctor_hits\": {},\n  \"oracle_policy_selector_ctor_this\": {},\n  \"oracle_policy_selector_ctor_vtable\": {},\n  \"oracle_policy_selector_ctor_owner\": {},\n  \"oracle_policy_selector_ctor_requested_flag_ptr\": {},\n  \"oracle_policy_selector_ctor_requested_flag_value\": {},\n  \"oracle_policy_selector_ctor_selector_arg\": {},\n  \"oracle_policy_selector_ctor_stored_selector_arg\": {},\n  \"oracle_policy_selector_ctor_stored_requested_flag_ptr\": {},\n  \"oracle_policy_selector_ctor_ret\": {},\n  \"oracle_policy_selector_ctor_caller_rva\": {},\n  \"oracle_policy_status_predicate_hits\": {},\n  \"oracle_policy_status_predicate_this\": {},\n  \"oracle_policy_status_predicate_owner\": {},\n  \"oracle_policy_status_predicate_flag_ptr\": {},\n  \"oracle_policy_status_predicate_flag_value\": {},\n  \"oracle_policy_status_predicate_ret\": {},\n  \"oracle_policy_status_predicate_caller_rva\": {},\n  \"oracle_policy_flag_setter_hits\": {},\n  \"oracle_policy_flag_setter_owner\": {},\n  \"oracle_policy_flag_setter_value\": {},\n  \"oracle_policy_flag_setter_force\": {},\n  \"oracle_policy_flag_setter_before\": {},\n  \"oracle_policy_flag_setter_after\": {},\n  \"oracle_policy_flag_setter_caller_rva\": {},\n  \"oracle_server_status_total_seen\": {},\n  \"oracle_server_status_any_seen\": {},\n  \"oracle_server_status_state\": {},\n  \"oracle_server_status_text_id\": {},\n",
        msgbox_total_builds,
        blocking_modal_present,
        msgbox_dialog,
        msgbox_vtable,
        msgbox_closing_latch,
        policy_total_builds,
        policy_any_seen,
        policy_ptr,
        policy_vtable,
        policy_arg_rdx,
        policy_arg_r8,
        policy_arg_r9,
        policy_stack_arg0,
        policy_backing_flag_ptr,
        policy_stack_arg0,
        policy_backing_flag_ptr,
        policy_stored_backing_flag_ptr,
        policy_backing_flag_value,
        policy_requested_flag_value,
        policy_caller_rva,
        policy_wrapper_hits,
        policy_wrapper_record,
        policy_wrapper_original_this,
        policy_wrapper_original_vtable,
        policy_wrapper_record_id,
        policy_wrapper_stack_arg0,
        policy_wrapper_backing_flag_ptr,
        policy_wrapper_ret,
        policy_wrapper_caller_rva,
        policy_selector_hits,
        policy_selector_record,
        policy_selector_original_this,
        policy_selector_original_vtable,
        policy_selector_owner,
        policy_selector_requested_flag,
        policy_selector_arg,
        policy_selector_ret,
        policy_selector_caller_rva,
        policy_selector_ctor_hits,
        policy_selector_ctor_this,
        policy_selector_ctor_vtable,
        policy_selector_ctor_owner,
        policy_selector_ctor_requested_flag_ptr,
        policy_selector_ctor_requested_flag_value,
        policy_selector_ctor_selector_arg,
        policy_selector_ctor_stored_selector_arg,
        policy_selector_ctor_stored_requested_flag_ptr,
        policy_selector_ctor_ret,
        policy_selector_ctor_caller_rva,
        policy_status_hits,
        policy_status_this,
        policy_status_owner,
        policy_status_flag_ptr,
        policy_status_flag_value,
        policy_status_ret,
        policy_status_caller_rva,
        policy_flag_setter_hits,
        policy_flag_setter_owner,
        policy_flag_setter_value,
        policy_flag_setter_force,
        policy_flag_setter_before,
        policy_flag_setter_after,
        policy_flag_setter_caller_rva,
        server_status_total_seen,
        server_status_any_seen,
        server_status_state,
        server_status_text_id
    ));
    body.push_str(&format!(
        "  \"oracle_title_native_menu_visual_suppress_installed\": {},\n  \"oracle_title_native_menu_visual_suppressed_builds\": {},\n  \"oracle_title_native_menu_visual_any_suppressed\": {},\n  \"oracle_title_native_menu_visual_last_out_slot\": {},\n  \"oracle_title_native_menu_visual_last_prev_out\": {},\n  \"oracle_title_native_menu_visual_last_args\": [{}, {}],\n  \"oracle_title_native_menu_visual_last_caller_rva\": {},\n  \"oracle_title_native_menu_visual_native_job\": {},\n  \"oracle_title_native_menu_visual_native_window\": {},\n  \"oracle_title_native_menu_visual_current_menu_id\": {},\n  \"oracle_title_native_menu_visual_current_flags\": {},\n  \"oracle_title_native_menu_visual_current_draw_bit_set\": {},\n  \"oracle_title_native_menu_visual_render_suppress_installed\": {},\n  \"oracle_title_native_menu_visual_render_suppressed_windows\": {},\n  \"oracle_title_native_menu_visual_render_any_suppressed\": {},\n  \"oracle_title_native_menu_visual_render_last_window\": {},\n  \"oracle_title_native_menu_visual_render_last_flags_before\": {},\n  \"oracle_title_native_menu_visual_render_last_flags_after\": {},\n  \"oracle_title_native_menu_visual_render_last_caller_rva\": {},\n  \"oracle_title_logo_surface_name\": \"{}\",\n  \"oracle_title_logo_resource_name\": \"{}\",\n  \"oracle_title_logo_gfx_root_depth\": {},\n  \"oracle_title_logo_gfx_root_sprite_char\": {},\n  \"oracle_title_logo_gfx_main_asset_char\": {},\n  \"oracle_title_logo_gfx_main_asset_name\": \"{}\",\n  \"oracle_title_logo_back_view_parts\": {},\n  \"oracle_title_logo_back_view_parts_vtable\": {},\n  \"oracle_title_logo_gfx_frame\": {},\n  \"oracle_title_logo_gfx_alpha_mult_term\": {},\n  \"oracle_title_logo_gfx_visibility\": {},\n  \"oracle_title_logo_gfx_hide_calls\": {},\n  \"oracle_title_logo_gfx_any_hidden\": {},\n  \"oracle_title_logo_gfx_hide_last_dialog\": {},\n  \"oracle_title_logo_gfx_hide_last_logo\": {},\n  \"oracle_title_logo_gfx_hide_last_caller_phase\": {},\n  \"oracle_title_logo_gfx_hide_last_requested_visible\": {},\n  \"oracle_title_press_start_surface_name\": \"PressStart\",\n  \"oracle_title_press_start_text_name\": \"StaticSystemText_101000\",\n  \"oracle_title_press_start_text_initial\": \"PRESS BUTTON\",\n  \"oracle_title_press_start_gfx_hide_calls\": {},\n  \"oracle_title_press_start_gfx_any_hidden\": {},\n  \"oracle_title_press_start_gfx_hide_last_dialog\": {},\n  \"oracle_title_press_start_gfx_hide_last_proxy\": {},\n  \"oracle_title_press_start_gfx_hide_last_context\": {},\n  \"oracle_title_press_start_gfx_hide_last_caller_phase\": {},\n  \"oracle_title_press_start_gfx_value\": {},\n  \"oracle_title_press_start_gfx_force_false_calls\": {},\n  \"oracle_title_press_start_gfx_force_false_any\": {},\n  \"oracle_title_press_start_gfx_force_false_last_value\": {},\n  \"oracle_title_press_start_gfx_force_false_last_requested\": {},\n  \"oracle_title_press_start_bind_hits\": {},\n  \"oracle_title_press_start_bind_any\": {},\n  \"oracle_title_press_start_bind_last_parent\": {},\n  \"oracle_title_press_start_bind_last_out\": {},\n  \"oracle_title_press_start_bind_last_name\": {},\n  \"oracle_title_press_start_bind_last_context\": {},\n  \"oracle_title_press_start_bind_hide_calls\": {},\n  \"oracle_title_press_start_bind_any_hidden\": {},\n  \"oracle_title_profile_face_bind_hits\": {},\n  \"oracle_title_profile_face_transform_applied\": {},\n  \"oracle_title_profile_face_other_hidden\": {},\n  \"oracle_title_profile_face_last_proxy\": {},\n  \"oracle_title_profile_face_last_value\": {},\n  \"oracle_title_scaleform_bind_observer_hits\": {},\n  \"oracle_title_scaleform_bind_observer_systex_hits\": {},\n  \"oracle_title_scaleform_bind_observer_last_owner\": {},\n  \"oracle_title_scaleform_bind_observer_last_pair\": {},\n  \"oracle_title_scaleform_bind_observer_last_symbol_ptr\": {},\n  \"oracle_title_scaleform_bind_observer_last_target_ptr\": {},\n  \"oracle_title_now_loading_helper_hooks_installed\": {},\n  \"oracle_title_now_loading_helper_ctor_hits\": {},\n  \"oracle_title_now_loading_helper_update_hits\": {},\n  \"oracle_title_now_loading_helper_last_this\": {},\n  \"oracle_title_now_loading_helper_last_menu_index\": {},\n  \"oracle_title_now_loading_helper_last_replace_tex_info\": {},\n  \"oracle_title_now_loading_helper_last_requested_replace_tex_info\": {},\n  \"oracle_title_now_loading_helper_last_flags\": {},\n  \"oracle_loadscreen_table_builds\": {},\n  \"oracle_loading_bg_portrait_gx_nonblack\": {},\n  \"oracle_loading_bg_portrait_is_checker\": {},\n  \"oracle_portrait_render_drive_hits\": {},\n  \"oracle_loading_bg_portrait_gx_dims\": {},\n  \"oracle_loading_bg_portrait_gx_format\": {},\n  \"oracle_title_logo_profile_summary\": {},\n  \"oracle_title_logo_profile_summary_ready\": {},\n  \"oracle_title_profile_render_refresh_gate_ready\": {},\n  \"oracle_title_custom_cover_profile_render_refresh_calls\": {},\n  \"oracle_title_custom_cover_profile_render_refresh_last_profile_summary\": {},\n  \"oracle_title_custom_cover_profile_render_refresh_last_caller_phase\": {},\n  \"oracle_title_custom_cover_profile_source_sample_calls\": {},\n  \"oracle_title_custom_cover_profile_source_slot\": {},\n  \"oracle_title_custom_cover_profile_source_renderer\": {},\n  \"oracle_title_custom_cover_profile_source_renderer_vtable\": {},\n  \"oracle_title_custom_cover_profile_source_offscreen_rend\": {},\n  \"oracle_title_custom_cover_profile_source_tex_rescap\": {},\n  \"oracle_title_custom_cover_profile_source_tex_index\": {},\n  \"oracle_title_custom_cover_profile_source_ready_754\": {},\n  \"oracle_title_custom_cover_profile_source_ready_755\": {},\n  \"oracle_title_custom_cover_profile_source_ready\": {},\n  \"oracle_title_custom_cover_profile_source_name\": \"{}\",\n  \"oracle_title_custom_cover_profile_renderer_class\": \"{}\",\n  \"oracle_title_custom_cover_profile_select_builds\": {},\n  \"oracle_title_custom_cover_profile_select_any_built\": {},\n  \"oracle_title_custom_cover_profile_select_last_ret\": {},\n  \"oracle_title_custom_cover_profile_select_last_job\": {},\n  \"oracle_title_custom_cover_profile_select_last_caller_rva\": {},\n  \"oracle_title_custom_cover_black_surface_name\": \"{}\",\n  \"oracle_title_custom_cover_black_builds\": {},\n  \"oracle_title_custom_cover_black_any_built\": {},\n  \"oracle_title_custom_cover_black_last_ret\": {},\n  \"oracle_title_custom_cover_black_last_job\": {},\n  \"oracle_title_custom_cover_black_last_caller_rva\": {},\n  \"oracle_title_pab_information_visual_name\": \"{}\",\n  \"oracle_title_pab_information_visual_builds\": {},\n  \"oracle_title_pab_information_visual_any_built\": {},\n  \"oracle_title_pab_information_visual_last_job\": {},\n  \"oracle_title_pab_information_visual_last_window\": {},\n  \"oracle_title_pab_information_visual_last_caller_rva\": {},\n",
        title_visual_suppress_installed,
        title_visual_suppressed_builds,
        title_visual_suppressed_builds != 0,
        title_visual_last_out_slot,
        title_visual_last_prev_out,
        title_visual_last_arg_rdx,
        title_visual_last_arg_r8,
        title_visual_last_caller_rva,
        title_visual_native_job,
        title_visual_native_window,
        title_visual_current_menu_id,
        title_visual_current_flags,
        title_visual_current_draw_bit_set,
        title_visual_render_suppress_installed,
        title_visual_render_suppressed_windows,
        title_visual_render_suppressed_windows != 0,
        title_visual_render_last_window,
        title_visual_render_last_flags_before,
        title_visual_render_last_flags_after,
        title_visual_render_last_caller_rva,
        TITLE_LOGO_BACK_VIEW_PARTS_NAME,
        TITLE_LOGO_RESOURCE_NAME,
        TITLE_LOGO_GFX_ROOT_DEPTH,
        TITLE_LOGO_GFX_ROOT_SPRITE_CHAR,
        TITLE_LOGO_GFX_MAIN_ASSET_CHAR,
        TITLE_LOGO_GFX_MAIN_ASSET_NAME,
        title_logo_back_view_parts,
        title_logo_back_view_parts_vtable,
        title_logo_gfx_frame,
        title_logo_gfx_alpha_mult_term,
        title_logo_gfx_visibility,
        title_logo_gfx_hide_calls,
        title_logo_gfx_hide_calls != 0,
        title_logo_gfx_hide_last_dialog,
        title_logo_gfx_hide_last_logo,
        title_logo_gfx_hide_last_caller_phase,
        title_logo_gfx_hide_last_requested_visible,
        title_press_start_gfx_hide_calls,
        title_press_start_gfx_hide_calls != 0,
        title_press_start_gfx_hide_last_dialog,
        title_press_start_gfx_hide_last_proxy,
        title_press_start_gfx_hide_last_context,
        title_press_start_gfx_hide_last_caller_phase,
        title_press_start_gfx_value,
        title_press_start_gfx_force_false_calls,
        title_press_start_gfx_force_false_calls != 0,
        title_press_start_gfx_force_false_last_value,
        title_press_start_gfx_force_false_last_requested,
        title_press_start_bind_hits,
        title_press_start_bind_hits != 0,
        title_press_start_bind_last_parent,
        title_press_start_bind_last_out,
        title_press_start_bind_last_name,
        title_press_start_bind_last_context,
        title_press_start_bind_hide_calls,
        title_press_start_bind_hide_calls != 0,
        title_profile_face_bind_hits,
        title_profile_face_transform_applied,
        title_profile_face_other_hidden,
        title_profile_face_last_proxy,
        title_profile_face_last_value,
        title_scaleform_bind_observer_hits,
        title_scaleform_bind_observer_systex_hits,
        title_scaleform_bind_observer_last_owner,
        title_scaleform_bind_observer_last_pair,
        title_scaleform_bind_observer_last_symbol_ptr,
        title_scaleform_bind_observer_last_target_ptr,
        now_loading_helper_hooks_installed,
        now_loading_helper_ctor_hits,
        now_loading_helper_update_hits,
        now_loading_helper_last_this,
        now_loading_helper_last_menu_index,
        now_loading_helper_last_replace_tex_info,
        now_loading_helper_last_requested_replace_tex_info,
        now_loading_helper_last_flags,
        loadscreen_table_builds,
        loading_bg_portrait_gx_nonblack,
        loading_bg_portrait_is_checker,
        portrait_render_drive_hits,
        loading_bg_portrait_gx_dims,
        loading_bg_portrait_gx_format,
        title_logo_profile_summary,
        title_logo_profile_summary_ready,
        title_profile_render_refresh_gate_ready,
        title_custom_cover_profile_render_refresh_calls,
        title_custom_cover_profile_render_refresh_last_profile_summary,
        title_custom_cover_profile_render_refresh_last_caller_phase,
        title_custom_cover_profile_source_sample_calls,
        title_custom_cover_profile_source_slot,
        title_custom_cover_profile_source_renderer,
        title_custom_cover_profile_source_renderer_vtable,
        title_custom_cover_profile_source_offscreen_rend,
        title_custom_cover_profile_source_tex_rescap,
        title_custom_cover_profile_source_tex_index,
        title_custom_cover_profile_source_ready_754,
        title_custom_cover_profile_source_ready_755,
        title_custom_cover_profile_source_ready,
        TITLE_CUSTOM_COVER_SYSTEX_TARGET,
        TITLE_CUSTOM_COVER_PROFILE_RENDERER_CLASS,
        title_custom_cover_profile_select_builds,
        title_custom_cover_profile_select_builds != 0,
        title_custom_cover_profile_select_last_ret,
        title_custom_cover_profile_select_last_job,
        title_custom_cover_profile_select_last_caller_rva,
        TITLE_CUSTOM_COVER_BLACK_NAME,
        title_custom_cover_black_builds,
        title_custom_cover_black_builds != 0,
        title_custom_cover_black_last_ret,
        title_custom_cover_black_last_job,
        title_custom_cover_black_last_caller_rva,
        TITLE_PAB_INFORMATION_VISUAL_NAME,
        title_pab_information_visual_builds,
        title_pab_information_visual_builds != 0,
        title_pab_information_visual_last_job,
        title_pab_information_visual_last_window,
        title_pab_information_visual_last_caller_rva
    ));
    push_json_usize(
        body,
        "oracle_title_custom_cover_black_cover_window",
        title_custom_cover_black_cover_window,
    );
    push_json_usize(
        body,
        "oracle_title_custom_cover_black_cover_menu_id",
        title_custom_cover_black_cover_menu_id,
    );
    push_json_usize(
        body,
        "oracle_title_custom_cover_black_cover_flags",
        title_custom_cover_black_cover_flags,
    );
    push_json_bool(
        body,
        "oracle_title_custom_cover_black_cover_draw_bit_set",
        title_custom_cover_black_cover_draw_bit_set,
    );
    push_json_usize(
        body,
        "oracle_title_pab_information_visual_current_menu_id",
        title_pab_information_visual_current_menu_id,
    );
    push_json_usize(
        body,
        "oracle_title_pab_information_visual_current_flags",
        title_pab_information_visual_current_flags,
    );
    push_json_bool(
        body,
        "oracle_title_pab_information_visual_current_draw_bit_set",
        title_pab_information_visual_current_draw_bit_set,
    );
    push_json_bool(
        body,
        "oracle_title_menu_resource_acquire_observer_installed",
        title_menu_resource_acquire_installed,
    );
    push_json_usize(
        body,
        "oracle_title_menu_resource_acquire_hits",
        title_menu_resource_acquire_hits,
    );
    push_json_usize(
        body,
        "oracle_title_menu_resource_acquire_logo_hits",
        title_menu_resource_acquire_logo_hits,
    );
    push_json_bool(
        body,
        "oracle_title_menu_resource_acquire_logo_seen",
        title_menu_resource_acquire_logo_hits != 0,
    );
    push_json_usize(
        body,
        "oracle_title_menu_resource_acquire_last_this",
        title_menu_resource_acquire_last_this,
    );
    push_json_usize(
        body,
        "oracle_title_menu_resource_acquire_last_load_params",
        title_menu_resource_acquire_last_load_params,
    );
    push_json_usize(
        body,
        "oracle_title_menu_resource_acquire_last_filename_ptr",
        title_menu_resource_acquire_last_filename_ptr,
    );
    push_json_usize(
        body,
        "oracle_title_menu_resource_acquire_last_param3",
        title_menu_resource_acquire_last_param3,
    );
    push_json_usize(
        body,
        "oracle_title_menu_resource_acquire_last_ret",
        title_menu_resource_acquire_last_ret,
    );
    push_json_usize(
        body,
        "oracle_title_menu_resource_acquire_last_caller_rva",
        title_menu_resource_acquire_last_caller_rva,
    );
    push_json_bool(
        body,
        "oracle_title_scaleform_file_open_observer_installed",
        title_scaleform_file_open_installed,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_file_open_hits",
        title_scaleform_file_open_hits,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_file_open_logo_hits",
        title_scaleform_file_open_logo_hits,
    );
    push_json_bool(
        body,
        "oracle_title_scaleform_file_open_logo_seen",
        title_scaleform_file_open_logo_hits != 0,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_file_open_last_loader",
        title_scaleform_file_open_last_loader,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_file_open_last_url_ptr",
        title_scaleform_file_open_last_url_ptr,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_file_open_last_flags",
        title_scaleform_file_open_last_flags,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_file_open_last_ret",
        title_scaleform_file_open_last_ret,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_file_open_last_ret_vtable",
        title_scaleform_file_open_last_ret_vtable,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_file_open_last_caller_rva",
        title_scaleform_file_open_last_caller_rva,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_memory_gfx_bytes",
        title_scaleform_memory_gfx_bytes,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_memory_gfx_replacements",
        title_scaleform_memory_gfx_replacements,
    );
    push_json_bool(
        body,
        "oracle_title_scaleform_memory_gfx_replaced",
        title_scaleform_memory_gfx_replacements != 0,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_05_000_memory_gfx_replacements",
        title_scaleform_05_000_memory_gfx_replacements,
    );
    push_json_bool(
        body,
        "oracle_title_scaleform_05_000_memory_gfx_replaced",
        title_scaleform_05_000_memory_gfx_replacements != 0,
    );
    push_json_usize(
        body,
        "oracle_title_05_000_runtime_strip_armed",
        TITLE_05_000_RUNTIME_STRIP_ARMED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_title_05_000_runtime_strip_serves",
        TITLE_05_000_RUNTIME_STRIP_SERVES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_title_05_000_runtime_strip_failures",
        TITLE_05_000_RUNTIME_STRIP_FAILURES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_title_05_000_runtime_strip_input_len",
        TITLE_05_000_RUNTIME_STRIP_INPUT_LEN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_title_05_000_runtime_strip_output_len",
        TITLE_05_000_RUNTIME_STRIP_OUTPUT_LEN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_title_05_000_runtime_strip_input_class",
        TITLE_05_000_RUNTIME_STRIP_INPUT_CLASS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_title_05_000_runtime_strip_output_validated",
        TITLE_05_000_RUNTIME_STRIP_OUTPUT_VALIDATED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_memory_gfx_failures",
        title_scaleform_memory_gfx_failures,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_memory_gfx_last_file",
        title_scaleform_memory_gfx_last_file,
    );
    push_json_bool(
        body,
        "oracle_title_scaleform_resource_ctor_observer_installed",
        title_scaleform_resource_ctor_installed,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_resource_ctor_hits",
        title_scaleform_resource_ctor_hits,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_resource_ctor_logo_hits",
        title_scaleform_resource_ctor_logo_hits,
    );
    push_json_bool(
        body,
        "oracle_title_scaleform_resource_ctor_logo_seen",
        title_scaleform_resource_ctor_logo_hits != 0,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_resource_ctor_last_out",
        title_scaleform_resource_ctor_last_out,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_resource_ctor_last_url_ptr",
        title_scaleform_resource_ctor_last_url_ptr,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_resource_ctor_last_file",
        title_scaleform_resource_ctor_last_file,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_resource_ctor_last_ret",
        title_scaleform_resource_ctor_last_ret,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_resource_ctor_last_movie_data",
        title_scaleform_resource_ctor_last_movie_data,
    );
    push_json_usize(
        body,
        "oracle_title_scaleform_resource_ctor_last_caller_rva",
        title_scaleform_resource_ctor_last_caller_rva,
    );
    title_custom_cover_profile_source_ready
}
