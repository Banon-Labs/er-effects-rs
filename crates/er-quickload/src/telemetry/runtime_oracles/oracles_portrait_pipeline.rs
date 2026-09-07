// Portrait-pipeline oracles: cross-slot swap tripwires, the nude-armor detector, Scaleform handler
// lifecycle, the Game-Options pane, the GX command queue, the ownership ledger, readback stall and
// tear measurement, the teardown fence, buffer provenance, render-resource release, idle-anim
// binding, and the face / published-vs-loaded identity checks.
//
// Self-contained: no local crosses either boundary.

fn write_portrait_pipeline_oracles(body: &mut String, base: usize) {
    const NULL_PTR: usize = 0;
    let format_optional_ptr = format_optional_oracle_ptr;
    write_portrait_bridge_hold_oracles(body);
    // Cross-slot SWAP tripwires: the pinned content-RT candidate (0 = never latched a confirmed head),
    // how many times the pin moved after first latch (>0 in one load window = unstable content source,
    // the swap bug's signature), how many per-slot target build kicks fired (0 = the loaded character
    // was never requested), and the max count of non-target renderers seen holding a live model during
    // the feed window (>0 = a foreign character built on the loading screen -- the swap precondition).
    push_json_usize(
        body,
        "oracle_portrait_rt_pin",
        PROFILE_RT_PIN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_rt_pin_switches",
        PROFILE_RT_PIN_SWITCHES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_target_kicks",
        PROFILE_TARGET_KICKS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_foreign_models",
        PROFILE_FOREIGN_MODELS_MAX.load(Ordering::SeqCst),
    );
    // NUDE-portrait armor oracle (bd er-effects-rs-wncc root cause, er-effects-rs-91l5 Layer 1).
    // Sampled every game tick off the profile renderer's live stage-0 ChrAsm (+0x130 -- Not the
    // +0x548 inbox), replicating FUN_1409e6fb0's override arithmetic, so these are the
    // EquipParamProtector rows the model build actually requests.
    //
    // Read it like this. `bad_frames > 0` = fail, and no later frame can erase it.
    // `sampled_frames == 0` = also FAIL: the oracle never got to look. `capture_verdict` is
    // tri-state on purpose -- 0 means "no capture-frame sample", which is not a pass. The raw
    // unk0/unkd4/unkd8 must all read -1; any non-negative value is the whole-outfit override that
    // rendered the character nude. Param-id fields read -2147483648 when never sampled, which is
    // distinct from the -1 that means "this slot is legitimately empty".
    push_json_usize(
        body,
        "oracle_portrait_equip_window",
        PORTRAIT_EQUIP_ORACLE_WINDOW.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_equip_slot",
        PORTRAIT_EQUIP_ORACLE_SLOT.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_equip_sampled_frames",
        PORTRAIT_EQUIP_SAMPLED_FRAMES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_equip_bad_frames",
        PORTRAIT_EQUIP_BAD_FRAMES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_equip_bad_frames_total",
        PORTRAIT_EQUIP_BAD_FRAMES_TOTAL.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_equip_bad_mask",
        PORTRAIT_EQUIP_BAD_MASK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_equip_windows_sampled",
        PORTRAIT_EQUIP_WINDOWS_SAMPLED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_equip_windows_bad",
        PORTRAIT_EQUIP_WINDOWS_BAD.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_equip_capture_verdict",
        PORTRAIT_EQUIP_CAPTURE_VERDICT.load(Ordering::SeqCst),
    );
    // The repair'S own SEMAPHORES. `restore_kicks` counts build kicks where the write-back ran;
    // `restore_failures > 0` means at least one portrait was built from the native feed's stripped
    // ChrAsm and the run's picture is not evidence for anything. `restore_noop_kicks` is the
    // honest-zero case -- a character genuinely holding nothing and wearing nothing on arms or legs
    // -- and is why a zero in the three slot counters is not by itself a failure. The four
    // `restore_record_*` ids are what the save says the character has, so a portrait that still
    // renders bare against non-negative ids here is a downstream defect, not a missing input.
    push_json_usize(
        body,
        "oracle_portrait_equip_restore_kicks",
        PORTRAIT_EQUIP_RESTORE_KICKS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_model_arm_style_writes",
        PORTRAIT_MODEL_ARM_STYLE_WRITES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_equip_inbox_arm_style_writes",
        PORTRAIT_EQUIP_INBOX_ARM_STYLE_WRITES.load(Ordering::SeqCst),
    );
    // The switch's own record repair. Zero on a run whose portrait showed the previous character
    // means the records were never put back after the game's return-title save (see
    // `reapply_profile_summary_after_return_title_save`); the mask says which slots it covered.
    push_json_usize(
        body,
        "oracle_profile_summary_reapplied_after_return_title",
        er_telemetry_core::counters::PROFILE_SUMMARY_REAPPLIED_AFTER_RETURN_TITLE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_summary_reapplied_slot_mask",
        er_telemetry_core::counters::PROFILE_SUMMARY_REAPPLIED_SLOT_MASK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_equip_restore_noop_kicks",
        PORTRAIT_EQUIP_RESTORE_NOOP_KICKS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_equip_restore_failures",
        PORTRAIT_EQUIP_RESTORE_FAILURES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_equip_restore_weapon_slots",
        PORTRAIT_EQUIP_RESTORE_WEAPON_SLOTS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_equip_restore_ammo_slots",
        PORTRAIT_EQUIP_RESTORE_AMMO_SLOTS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_equip_restore_protector_slots",
        PORTRAIT_EQUIP_RESTORE_PROTECTOR_SLOTS.load(Ordering::SeqCst),
    );
    {
        let unpack = crate::experiments::portrait_equip_unpack;
        let effective =
            |slot: usize| unpack(PORTRAIT_EQUIP_FIRST_EFFECTIVE_ID[slot].load(Ordering::SeqCst));
        let captured =
            |slot: usize| unpack(PORTRAIT_EQUIP_CAPTURE_EFFECTIVE_ID[slot].load(Ordering::SeqCst));
        let recorded =
            |slot: usize| unpack(PORTRAIT_EQUIP_RECORD_PARAM_ID[slot].load(Ordering::SeqCst));
        body.push_str(&format!(
            "  \"oracle_portrait_equip_effective_head\": {},\n  \"oracle_portrait_equip_effective_chest\": {},\n  \"oracle_portrait_equip_effective_hands\": {},\n  \"oracle_portrait_equip_effective_legs\": {},\n  \"oracle_portrait_equip_capture_head\": {},\n  \"oracle_portrait_equip_capture_chest\": {},\n  \"oracle_portrait_equip_capture_hands\": {},\n  \"oracle_portrait_equip_capture_legs\": {},\n  \"oracle_portrait_equip_record_head\": {},\n  \"oracle_portrait_equip_record_chest\": {},\n  \"oracle_portrait_equip_unk0\": {},\n  \"oracle_portrait_equip_unkd4\": {},\n  \"oracle_portrait_equip_unkd8\": {},\n  \"oracle_portrait_equip_restore_record_right_weapon\": {},\n  \"oracle_portrait_equip_restore_record_left_weapon\": {},\n  \"oracle_portrait_equip_restore_record_hands\": {},\n  \"oracle_portrait_equip_restore_record_legs\": {},\n  \"oracle_portrait_equip_live_right_weapon\": {},\n  \"oracle_portrait_equip_live_left_weapon\": {},\n  \"oracle_portrait_equip_live_arm_style\": {},\n  \"oracle_portrait_equip_record_arm_style\": {},\n  \"oracle_portrait_model_arm_style_readback\": {},\n  \"oracle_portrait_equip_inbox_arm_style_fed\": {},\n  \"oracle_portrait_equip_inbox_arm_style_readback\": {},\n",
            effective(0),
            effective(1),
            effective(2),
            effective(3),
            captured(0),
            captured(1),
            captured(2),
            captured(3),
            recorded(0),
            recorded(1),
            unpack(PORTRAIT_EQUIP_FIRST_UNK0.load(Ordering::SeqCst)),
            unpack(PORTRAIT_EQUIP_FIRST_UNKD4.load(Ordering::SeqCst)),
            unpack(PORTRAIT_EQUIP_FIRST_UNKD8.load(Ordering::SeqCst)),
            unpack(PORTRAIT_EQUIP_RESTORE_RECORD_ID[0].load(Ordering::SeqCst)),
            unpack(PORTRAIT_EQUIP_RESTORE_RECORD_ID[1].load(Ordering::SeqCst)),
            unpack(PORTRAIT_EQUIP_RESTORE_RECORD_ID[2].load(Ordering::SeqCst)),
            unpack(PORTRAIT_EQUIP_RESTORE_RECORD_ID[3].load(Ordering::SeqCst)),
            unpack(PORTRAIT_EQUIP_LIVE_WEAPON_ID[0].load(Ordering::SeqCst)),
            unpack(PORTRAIT_EQUIP_LIVE_WEAPON_ID[1].load(Ordering::SeqCst)),
            unpack(PORTRAIT_EQUIP_LIVE_ARM_STYLE.load(Ordering::SeqCst)),
            unpack(PORTRAIT_EQUIP_RECORD_ARM_STYLE.load(Ordering::SeqCst)),
            unpack(PORTRAIT_MODEL_ARM_STYLE_READBACK.load(Ordering::SeqCst)),
            unpack(PORTRAIT_EQUIP_INBOX_ARM_STYLE_FED.load(Ordering::SeqCst)),
            unpack(PORTRAIT_EQUIP_INBOX_ARM_STYLE_READBACK.load(Ordering::SeqCst)),
        ));
    }
    // Scaleform menu-handler lifecycle guard (repeated-switch ProfileSelect UAF). double_frees > 0
    // proves the guard caught+skipped the crash; ctors/dtors give the churn context.
    push_json_usize(
        body,
        "oracle_scaleform_handler_double_frees",
        SCALEFORM_HANDLER_DOUBLE_FREES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_scaleform_handler_ctors",
        SCALEFORM_HANDLER_CTORS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_scaleform_handler_dtors",
        SCALEFORM_HANDLER_DTORS.load(Ordering::SeqCst),
    );
    // Game-Options pane visibility oracle (read-only, blank Game Options pane detector): on
    // OptionSetting re-entry the DLL reads each option pane's DisplayInfo.Visible. blank_detected
    // > 0 = the WindowList container resolved in the tree but its pane was not visible (tabs/footer
    // render, row list black); resolved/visible masks + last_datatype + guard_skips give context.
    push_json_usize(
        body,
        "oracle_optionsetting_pane_sample_count",
        OPTIONSETTING_PANE_SAMPLE_COUNT.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_pane_windowlist_resolved",
        OPTIONSETTING_PANE_LAST_WINDOWLIST_RESOLVED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_pane_windowlist_visible",
        OPTIONSETTING_PANE_LAST_WINDOWLIST_VISIBLE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_pane_resolved_mask",
        OPTIONSETTING_PANE_LAST_RESOLVED_MASK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_pane_visible_mask",
        OPTIONSETTING_PANE_LAST_VISIBLE_MASK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_pane_last_datatype",
        OPTIONSETTING_PANE_LAST_DATATYPE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_pane_guard_skips",
        OPTIONSETTING_PANE_GUARD_SKIPS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_pane_composite_bound",
        OPTIONSETTING_PANE_COMPOSITE_BOUND.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_pane_blank_detected_count",
        OPTIONSETTING_PANE_BLANK_DETECTED_COUNT.load(Ordering::SeqCst),
    );
    // Real row-pane signal: current tab dialog (composite+0xb8) and its pane proxy (dialog+0x1200)
    // DisplayInfo.Visible -- the object the game's tab-select actually toggles. real_blank_detected
    // fires only after a healthy (visible) pane was seen and then the actively-shown pane went hidden,
    // so it cannot false-fire on boot/preload (unlike the named-child mask above).
    push_json_usize(
        body,
        "oracle_optionsetting_current_dialog",
        OPTIONSETTING_CURRENT_DIALOG.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_current_pane_visible",
        OPTIONSETTING_CURRENT_PANE_VISIBLE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_current_pane_datatype",
        OPTIONSETTING_CURRENT_PANE_DATATYPE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_actively_shown",
        OPTIONSETTING_ACTIVELY_SHOWN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_last_flag",
        OPTIONSETTING_LAST_FLAG.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_current_pane_ever_visible",
        OPTIONSETTING_CURRENT_PANE_EVER_VISIBLE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_real_blank_detected_count",
        OPTIONSETTING_REAL_BLANK_DETECTED_COUNT.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_current_tab",
        OPTIONSETTING_CURRENT_TAB.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_current_tab_at_blank",
        OPTIONSETTING_CURRENT_TAB_AT_BLANK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_pane_fix_applied",
        OPTIONSETTING_PANE_FIX_APPLIED.load(Ordering::SeqCst),
    );
    // Active OptionSetting row-table oracle: classifies the currently visible tab dialog's rows by
    // action pointers. tab 0 with cloned_mask!=0 is the Game Options contamination bug; Quit tab
    // with missing cloned_mask is the "feature not injected" bug. This is read-only and independent
    // of screenshot/OCR.
    push_json_usize(
        body,
        "oracle_optionsetting_active_row_sample_count",
        OPTIONSETTING_ACTIVE_ROW_SAMPLE_COUNT.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_active_row_dialog",
        OPTIONSETTING_ACTIVE_ROW_DIALOG.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_active_row_tab",
        OPTIONSETTING_ACTIVE_ROW_TAB.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_active_row_count",
        OPTIONSETTING_ACTIVE_ROW_COUNT.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_active_row_cloned_mask",
        OPTIONSETTING_ACTIVE_ROW_CLONED_MASK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_active_row_native_save_mask",
        OPTIONSETTING_ACTIVE_ROW_NATIVE_SAVE_MASK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_active_row_action_hash",
        OPTIONSETTING_ACTIVE_ROW_ACTION_HASH.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_active_row_label_hash",
        OPTIONSETTING_ACTIVE_ROW_LABEL_HASH.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_active_row_quit_label_mask",
        OPTIONSETTING_ACTIVE_ROW_QUIT_LABEL_MASK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_game_options_cloned_row_hits",
        OPTIONSETTING_GAME_OPTIONS_CLONED_ROW_HITS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_optionsetting_game_options_quit_label_hits",
        OPTIONSETTING_GAME_OPTIONS_QUIT_LABEL_HITS.load(Ordering::SeqCst),
    );
    // GX command-queue overflow forensics (repeated-switch crash 0x1aeaf05): max_fill climbing
    // toward cap across switches = the accumulating-producer signature; top_producers names the
    // caller RVAs (entries tagged +self passed through our DLL).
    push_json_usize(
        body,
        "oracle_gx_cmdqueue_cap",
        GX_CMD_QUEUE_CAP_SEEN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_gx_cmdqueue_max_fill",
        GX_CMD_QUEUE_MAX_FILL.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_gx_cmdqueue_switch_max_fill",
        GX_CMD_QUEUE_SWITCH_MAX_FILL.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_gx_cmdqueue_reserves",
        GX_CMD_QUEUE_SUBMITS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_gx_cmdqueue_nearfull_hits",
        GX_CMD_QUEUE_NEARFULL_HITS.load(Ordering::SeqCst),
    );
    // Repeated-switch spared-renderer leak fix: renderers reclaimed via CSDelayDeleteMan (should
    // rise ~1/switch) and the count currently spared -- proves the orphan accumulation is capped.
    push_json_usize(
        body,
        "oracle_profile_spare_orphans_deleted",
        PROFILE_SPARE_ORPHANS_DELETED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_renderer_spare_hits",
        PROFILE_RENDERER_SPARE_HITS.load(Ordering::SeqCst),
    );
    // Ownership-ledger conservation oracle: violations must stay 0 (nonzero == a native-owned
    // object taken without a paired release -- the spared-renderer leak class). spared_outstanding
    // and its high-water should track the bound (1); a climbing value is the early leak signal.
    // `oracle_ownership_ledger_violations` is deleted, not merely unwired (2026-09-05). Its only
    // writer was `ownership_ledger_check`, which ran at the sq-repro autopilot's switch boundary and
    // went with it. Left in place the field would have published a constant 0 -- a spared-renderer
    // leak detector that reports "no leaks" because it never runs, which is strictly worse than an
    // absent field. Reinstating the detector means calling the check from the live switch boundary in
    // `system_quit_arm_quickload_autoload` and validating it there; that is a deliberate change with
    // its own proof, not something to leave implied by a zero.
    push_json_usize(
        body,
        "oracle_ownership_spared_outstanding",
        crate::experiments::ownership_outstanding(crate::constants::OwnedClass::SparedRenderer),
    );
    push_json_usize(
        body,
        "oracle_ownership_spared_max_outstanding",
        OWNED_MAX_OUTSTANDING[crate::constants::OwnedClass::SparedRenderer as usize]
            .load(Ordering::SeqCst),
    );
    // Loading-portrait select-then-show: retargets = confirm-time swaps to the newly-selected
    // character; skipped_unkeyed = frames not published because the depth mask was not applied yet
    // (never render an unmasked model); have_keyed = a masked frame is available to display.
    push_json_usize(
        body,
        "oracle_portrait_retargets",
        PROFILE_PORTRAIT_RETARGETS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_publish_skipped_unkeyed",
        PROFILE_PUBLISH_SKIPPED_UNKEYED.load(Ordering::SeqCst),
    );
    // Harness-failure semaphore (user directive 2026-07-06): windows that drove the model but
    // published no portrait. The readiness watcher fails the run when this is non-zero -- the
    // publish gates must never silently degrade the product; drive this to 0 by fixing the root
    // render (per-cause in `..._fail_cause`: 1=torn 2=unkeyed 3=badiou 4=lowmask).
    push_json_usize(
        body,
        "oracle_portrait_window_publish_failures",
        PORTRAIT_WINDOW_PUBLISH_FAILURES.load(Ordering::SeqCst),
    );
    // READBACK stall split (diagnostic): average microseconds per coherent readback for the GPU-wait
    // (removable by an async ring buffer) vs the CPU de-swizzle + mask/key (stay on the render
    // thread). Decides how close to the ~7.5s floor an async readback can get before the CPU pass
    // becomes the residual bottleneck.
    {
        let n = PORTRAIT_RB_COUNT.load(Ordering::SeqCst).max(1);
        let mn = PORTRAIT_RB_MASK_COUNT.load(Ordering::SeqCst).max(1);
        push_json_usize(
            body,
            "oracle_portrait_rb_count",
            PORTRAIT_RB_COUNT.load(Ordering::SeqCst),
        );
        push_json_usize(
            body,
            "oracle_portrait_rb_wait_avg_us",
            PORTRAIT_RB_WAIT_US_SUM.load(Ordering::SeqCst) / n,
        );
        push_json_usize(
            body,
            "oracle_portrait_rb_deswizzle_avg_us",
            PORTRAIT_RB_DESWIZZLE_US_SUM.load(Ordering::SeqCst) / n,
        );
        push_json_usize(
            body,
            "oracle_portrait_rb_mask_avg_us",
            PORTRAIT_RB_MASK_US_SUM.load(Ordering::SeqCst) / mn,
        );
    }
    push_json_usize(
        body,
        "oracle_portrait_window_publish_fail_cause",
        PORTRAIT_WINDOW_PUBLISH_FAIL_CAUSE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_have_keyed_frame",
        PROFILE_HAVE_KEYED_FRAME.load(Ordering::SeqCst),
    );
    // Torn-readback semaphore: tear score of the last publish attempt + the run max, plus how many
    // keyed frames were skipped as torn vs published clean. A high max with clean>0 means clean
    // frames do land (gate suffices); clean==0 with high max means every driven frame tears (the
    // readback needs real GPU sync). clean_min is the lowest clean score seen (baseline).
    push_json_usize(
        body,
        "oracle_portrait_tear_last",
        PROFILE_TEAR_SCORE_LAST.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_tear_max",
        PROFILE_TEAR_SCORE_MAX.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_tear_clean_min",
        PROFILE_TEAR_SCORE_CLEAN_MIN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_publish_clean",
        PROFILE_PUBLISH_CLEAN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_last_publish_ms",
        er_telemetry_core::counters::PORTRAIT_LAST_PUBLISH_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_last_draw_tick_ms",
        er_telemetry_core::counters::PORTRAIT_LAST_DRAW_TICK_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_publish_skipped_torn",
        PROFILE_PUBLISH_SKIPPED_TORN.load(Ordering::SeqCst),
    );
    // Animation-stall: last loading window's animated (drive) vs displayed frames. drive<<display
    // means the head froze early (freeze-after-capture) -- the user's "stopped animating" symptom.
    push_json_usize(
        body,
        "oracle_portrait_drive_frames_last_window",
        PROFILE_DRIVE_FRAMES_WINDOW_LAST.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_display_frames_last_window",
        PROFILE_DISPLAY_FRAMES_WINDOW_LAST.load(Ordering::SeqCst),
    );
    // Teardown-fence protocol (freeze relaxation): skips = pump frames yielded to a live
    // teardown; waits = teardowns that paused for a mid-drive pump; timeouts must stay 0
    // (nonzero == one frame of the old TOCTOU exposure leaked past the 10ms cap).
    push_json_usize(
        body,
        "oracle_portrait_drive_fence_skips",
        PROFILE_DRIVE_FENCE_SKIPS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_teardown_fence_waits",
        PROFILE_TEARDOWN_FENCE_WAITS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_teardown_fence_timeouts",
        PROFILE_TEARDOWN_FENCE_TIMEOUTS.load(Ordering::SeqCst),
    );
    // Color/depth source provenance (green-face wrong-buffer fix): only bundle-provenance color
    // may display; unpaired counts real frames held back for lacking it.
    push_json_usize(
        body,
        "oracle_portrait_color_from_bundle",
        crate::experiments::PROFILE_COLOR_FROM_BUNDLE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_color_from_scan",
        crate::experiments::PROFILE_COLOR_FROM_SCAN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_depth_from_chain",
        crate::experiments::PROFILE_DEPTH_FROM_CHAIN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_depth_from_bfs",
        crate::experiments::PROFILE_DEPTH_FROM_BFS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_publish_skipped_unpaired",
        crate::experiments::PROFILE_PUBLISH_SKIPPED_UNPAIRED.load(Ordering::SeqCst),
    );
    // hi2: partial-mask band (mask cut something but under the floor) + how long the bridge
    // held before the window's first publish.
    push_json_usize(
        body,
        "oracle_portrait_publish_skipped_lowmask",
        PROFILE_PUBLISH_SKIPPED_LOWMASK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_first_keyed_display_last_window",
        PROFILE_WINDOW_FIRST_KEYED_DISPLAY_LAST.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_depth_key_degenerate",
        crate::experiments::DEPTH_KEY_DEGENERATE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_depth_key_second_pass",
        crate::experiments::DEPTH_KEY_SECOND_PASS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_publish_skipped_badiou",
        crate::experiments::PROFILE_PUBLISH_SKIPPED_BADIOU.load(Ordering::SeqCst),
    );
    push_json_str(
        body,
        "oracle_gx_cmdqueue_top_producers",
        &crate::experiments::gx_cmd_queue_hist_top(8),
    );
    push_json_str(
        body,
        "oracle_gx_cmdqueue_buckets",
        &crate::experiments::gx_cmd_queue_bucket_summary(),
    );
    push_json_usize(
        body,
        "oracle_gx_cmdarena_min_remaining",
        GX_CMD_ARENA_MIN_REMAINING.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_gx_cmdarena_switch_min_remaining",
        GX_CMD_ARENA_SWITCH_MIN_REMAINING.load(Ordering::SeqCst),
    );
    {
        let (dd_pending, dd_highwater) = unsafe { crate::experiments::delay_delete_pending() }
            .map(|(p, h)| (p as i64, h as i64))
            .unwrap_or((-1, -1));
        body.push_str(&format!(
            "  \"oracle_delaydelete_pending\": {dd_pending},\n"
        ));
        body.push_str(&format!(
            "  \"oracle_delaydelete_highwater\": {dd_highwater},\n"
        ));
    }
    // Render-resource-release oracles (bd AC-2-answered-native-reload-no-dip-mod-ownload-dips-real-
    // divergence-phase3 + PHASE3-render-release-is-CommonFinalize). The switch reload renders ~+40ms/
    // frame heavier than firstload at idle with flat GX cmdqueue fill and flat entity counts, so the
    // extra cost is render execution the reload leaves live: own_load_switch_reload_fire skips the native
    // return-title render-resource release that `CS::InGameStep::_Common_Finalize` (RVA 0xaed380)
    // performs -- that teardown resets g_GxDrawContext's per-window render outputs (FUN_1419eaf90) and
    // frees GLOBAL_CSDistViewManager / GLOBAL_WorldChrMan / GLOBAL_MapItemMan / bullet+dmg managers.
    // These are passive reads (no hooks -- per bd gpu-frame-us-ecl-piggyback-oracle-crashes-native-path
    // no per-ECL/per-draw hooks) that expose the live render-output vector plus the render managers the
    // skipped teardown would have freed, so a heavier reload frame can be told apart from a clean control
    // by structure (extra render outputs / leftover managers) rather than only the _Common_Finalize hook
    // counter (oracle_common_finalize_count). RE grounding (dump pc_eldenring_runtime.1.16.2.exe, base
    // 0x140000000): the 0xaed380 disasm loads these exact GLOBAL_* data globals; the GxDrawContext
    // render-output container layout is confirmed by its ctor GXSR::GxDrawContext::GxDrawContext
    // (0x1419e4740 -> FUN_1419e3dc0), a DLKR container = {allocator@+0x120, begin@+0x128, end@+0x130,
    // cap@+0x138}, and +0x128==begin matches the production swapchain-find chain
    // (present_overlay::find_game_swapchain, runtime-proven). NOTE: WorldChrMan entity list-counts are
    // already emitted (oracle_wcm_*/oracle_worldchrman_* in write_player_presence_oracle) and are flat
    // per AC-2, so they are not duplicated here.
    {
        const G_GX_DRAW_CONTEXT_RVA: usize = er_loading_portrait_core::GX_DRAW_CONTEXT_RVA;
        const GXDC_OUTPUT_VEC_BEGIN_OFFSET: usize = 0x128;
        const GXDC_OUTPUT_VEC_END_OFFSET: usize = 0x130;
        const GXDC_OUTPUT_VEC_CAP_OFFSET: usize = 0x138;
        // Per-window render-output entry stride (prior RE, present_overlay: each inline entry is 0x170
        // bytes, first qword = the per-window output object). Used only to derive a human-readable count;
        // the raw byte span is emitted alongside so the signal survives if the stride is ever corrected.
        const GXDC_OUTPUT_ENTRY_STRIDE: usize = 0x170;
        // GLOBAL_* render managers _Common_Finalize frees (data RVAs read straight off the 0xaed380
        // disasm; 0-shift data-global convention, same 0x143d_xxxx block as GameDataMan/CSSystemStep).
        const GLOBAL_CS_DIST_VIEW_MANAGER_RVA: usize = 0x3d675c0;
        const GLOBAL_MAP_ITEM_MAN_RVA: usize = 0x3d67a50;
        const RENDER_READ_FAIL: i64 = -1;
        const MIN_VALID_PTR: usize = 0x10000;
        let gxdc = unsafe {
            crate::experiments::safe_read_usize(er_game_base::mem::game_data_addr(
                base,
                G_GX_DRAW_CONTEXT_RVA,
                "G_GX_DRAW_CONTEXT_RVA",
            ))
        }
        .filter(|p| *p >= MIN_VALID_PTR)
        .unwrap_or(NULL_PTR);
        let (out_span_bytes, out_count, out_capacity) = if gxdc == NULL_PTR {
            (RENDER_READ_FAIL, RENDER_READ_FAIL, RENDER_READ_FAIL)
        } else {
            let begin =
                unsafe { crate::experiments::safe_read_usize(gxdc + GXDC_OUTPUT_VEC_BEGIN_OFFSET) };
            let end =
                unsafe { crate::experiments::safe_read_usize(gxdc + GXDC_OUTPUT_VEC_END_OFFSET) };
            let cap =
                unsafe { crate::experiments::safe_read_usize(gxdc + GXDC_OUTPUT_VEC_CAP_OFFSET) };
            match (begin, end, cap) {
                (Some(b), Some(e), Some(c)) if e >= b && c >= b => {
                    let span = e - b;
                    let cap_span = c - b;
                    (
                        span as i64,
                        (span / GXDC_OUTPUT_ENTRY_STRIDE) as i64,
                        (cap_span / GXDC_OUTPUT_ENTRY_STRIDE) as i64,
                    )
                }
                _ => (RENDER_READ_FAIL, RENDER_READ_FAIL, RENDER_READ_FAIL),
            }
        };
        let distview = unsafe {
            crate::experiments::safe_read_usize(er_game_base::mem::game_data_addr(
                base,
                GLOBAL_CS_DIST_VIEW_MANAGER_RVA,
                "GLOBAL_CS_DIST_VIEW_MANAGER_RVA",
            ))
        }
        .filter(|p| *p >= MIN_VALID_PTR)
        .unwrap_or(NULL_PTR);
        let mapitemman = unsafe {
            crate::experiments::safe_read_usize(er_game_base::mem::game_data_addr(
                base,
                GLOBAL_MAP_ITEM_MAN_RVA,
                "GLOBAL_MAP_ITEM_MAN_RVA",
            ))
        }
        .filter(|p| *p >= MIN_VALID_PTR)
        .unwrap_or(NULL_PTR);
        body.push_str(&format!(
            "  \"oracle_gxdc_ptr\": {},\n  \"oracle_gxdc_output_span_bytes\": {out_span_bytes},\n  \"oracle_gxdc_output_count\": {out_count},\n  \"oracle_gxdc_output_capacity\": {out_capacity},\n  \"oracle_render_distview_mgr_ptr\": {},\n  \"oracle_render_mapitem_mgr_ptr\": {},\n",
            format_optional_ptr(gxdc),
            format_optional_ptr(distview),
            format_optional_ptr(mapitemman),
        ));
    }
    push_json_usize(
        body,
        "oracle_portrait_multi_model_publish_skips",
        PROFILE_MULTI_MODEL_PUBLISH_SKIPS.load(Ordering::SeqCst),
    );
    // Idle-ANIM BIND semaphores (bd portrait-anim-bind-RE-corrects-6hz-gate-2026-07-03):
    // bind_state 1 = an engine-grounded idle anim bound (handle real), 2 = no candidate resolved;
    // handle_before != sentinel proves the native static-pose anim-0 bind had resolved (anim
    // resources are loaded); sentinel is the DAT_143b39470 null-handle global (constant if the
    // corrected RE is right). Motion vs FLICKER: motion_metric diffs the depth-keyed alpha
    // silhouette (lighting-immune), luma_flicker diffs luma on the same grid (quantifies the
    // per-frame lighting change). Product proof of "portrait animates" = bind_state 1 and
    // motion_metric_max clearly above 0 with luma_flicker as the lighting control.
    push_json_usize(
        body,
        "oracle_portrait_anim_bind_state",
        PORTRAIT_ANIM_BIND_STATE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_facedata_neq_ticks",
        PORTRAIT_FACEDATA_NEQ_TICKS.load(Ordering::SeqCst),
    );
    // Face-identity semaphore (user directive 2026-07-06): at each build kick for a slot owned by a
    // foreign-save preview, the record's inner FaceDataBuffer is re-hashed against the fingerprint
    // stored when the preview wrote it. `mismatches > 0` == the portrait was about to render a
    // different character's face than the one the user picked -- fail-fast signal for probe watchers.
    push_json_usize(
        body,
        "oracle_portrait_face_identity_checks",
        PORTRAIT_FACE_IDENTITY_CHECKS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_face_identity_mismatches",
        PORTRAIT_FACE_IDENTITY_MISMATCHES.load(Ordering::SeqCst),
    );
    // Published-vs-loaded identity (bd er-effects-rs-qoqc defect 6 / er-effects-rs-91zb).
    // Asserted at every loading-window close. The face-identity pair above catches a record
    // whose face was rewritten under a slot; these catch the portrait being built for the
    // wrong slot entirely -- the class that put slot 9's head on screen for 29.7s while slot 5
    // loaded with every other oracle reporting ok. Read `_checks` first: 0 mismatches with 0
    // checks is an unexercised path, not a pass.
    push_json_usize(
        body,
        "oracle_portrait_published_identity_checks",
        er_telemetry_core::counters::PORTRAIT_PUBLISHED_IDENTITY_CHECKS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_published_slot_mismatches",
        er_telemetry_core::counters::PORTRAIT_PUBLISHED_SLOT_MISMATCHES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_published_name_hash_mismatches",
        er_telemetry_core::counters::PORTRAIT_PUBLISHED_NAME_HASH_MISMATCHES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_pump_draws",
        PROFILE_PERFRAME_MODEL_DRAWS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_pump_block_r",
        PORTRAIT_PUMP_BLOCK_R.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_pump_block_vtable",
        PORTRAIT_PUMP_BLOCK_VTABLE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_pump_block_off",
        PORTRAIT_PUMP_BLOCK_OFF.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_pump_block_off_resource",
        PORTRAIT_PUMP_BLOCK_OFF_RESOURCE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_pump_block_multi",
        PORTRAIT_PUMP_BLOCK_MULTI.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_drive_ticks",
        PORTRAIT_DRIVE_TICKS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_anim_bind_attempts",
        PORTRAIT_ANIM_BIND_ATTEMPTS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_anim_bound_id",
        PORTRAIT_ANIM_BOUND_ID.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_anim_handle_before",
        PORTRAIT_ANIM_HANDLE_BEFORE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_anim_handle",
        PORTRAIT_ANIM_HANDLE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_anim_sentinel",
        PORTRAIT_ANIM_SENTINEL.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_motion_metric_last",
        PORTRAIT_MOTION_METRIC_LAST.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_motion_metric_max",
        PORTRAIT_MOTION_METRIC_MAX.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_luma_flicker_last",
        PORTRAIT_LUMA_FLICKER_LAST.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_luma_flicker_max",
        PORTRAIT_LUMA_FLICKER_MAX.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_loading_bar_hook_installed",
        LOADING_SCREEN_UPDATE_HOOK_INSTALLED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_loading_bar_update_hits",
        LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_loading_screen_last_this",
        LOADING_SCREEN_LAST_THIS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_loading_screen_last_data",
        LOADING_SCREEN_LAST_DATA.load(Ordering::SeqCst),
    );
}
