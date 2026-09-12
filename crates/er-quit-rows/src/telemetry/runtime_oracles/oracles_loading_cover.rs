// Loading-cover oracles: the stats panel, the portrait camera lever, the display/keepalive path,
// swapchain-find attribution, the native-Windows overlay, and the boot-progress view semaphores --
// the cover window, its release, its absolute backstop, and the loading-screen portrait defect
// detectors.
//
// Self-contained: no local crosses either boundary. [`boot_view_present_cover_failed`] lives here
// rather than in the spine because its only caller is the cover-failure field below, and the unit
// tests reach it through the flat `telemetry` module the include files all share.

/// A completed self-pump draw can lose the handoff race before it is presented, so successful
/// draws must be compared with self-attributed full clears rather than submitted self-presents.
fn boot_view_present_cover_failed(
    pump_stop_reason: usize,
    boot_view_stopped: usize,
    draws: usize,
    self_full_clears: usize,
    present_full_clears: usize,
) -> bool {
    pump_stop_reason == 1
        && boot_view_stopped == 0
        && draws > self_full_clears
        && present_full_clears == 0
}

fn write_loading_cover_oracles(body: &mut String) {
    // Removed 2026-07-31 (er-effects-rs-56fx): oracle_tpf_texture_registered / _last_rescap /
    // _bound / _failures / _last_error. All five counters had zero writers -- the TPF cover
    // texture path never reports through them -- and three carried sentinel initialisers
    // (ER_TPF_COVER_ERR_NONE and friends), so they emitted a plausible-looking value forever
    // rather than an obviously-absent 0. `oracle_tpf_texture_key` stays: it is a static string,
    // not a counter. Re-add these with writers at the register/bind sites if that path is built.
    // Stats-panel neutral-background wire-up oracles (memory-read telemetry, not screenshot). A
    // runtime watcher confirms the character render is blanked, each per-slot neutral bg registered
    // into the repos, and each visible face bind redirected to our key -- all without an image.
    // `stats_panel_enabled` == the render-blank / stats-panel product mode is active.
    push_json_bool(body, "oracle_stats_panel_enabled", stats_panel_enabled());
    push_json_usize(
        body,
        "oracle_stats_panel_registered_mask",
        STATS_PANEL_TEX_REGISTERED_MASK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_stats_panel_register_attempts",
        STATS_PANEL_TEX_REGISTER_ATTEMPTS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_stats_panel_register_failures",
        STATS_PANEL_TEX_REGISTER_FAILURES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_stats_panel_redirect_mask",
        STATS_PANEL_BIND_REDIRECT_MASK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_stats_panel_redirects",
        STATS_PANEL_BIND_REDIRECTS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_stats_panel_last_error",
        STATS_PANEL_LAST_ERROR.load(Ordering::SeqCst),
    );
    // Stats-panel native text oracles (row-populate push design): native row fills observed,
    // successful ErStats pushes, and rejected pushes. subs>0 == the attribute line reached the
    // GFX-edit `ErStats` field (rendered in MenuFont_01) in its own field; failures>0 with
    // subs==0 == the 05_010 edit was not live (field missing) or SetText rejected the value.
    push_json_usize(
        body,
        "oracle_stats_text_installed",
        TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_INSTALLED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_stats_text_row_populates",
        PROFILE_STATS_ROW_POPULATES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_stats_text_settext_subs",
        PROFILE_STATS_SETTEXT_SUBS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_stats_text_push_failures",
        PROFILE_STATS_PUSH_FAILURES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_player_name_push_attempts",
        PROFILE_PLAYER_NAME_PUSH_ATTEMPTS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_player_name_settext_subs",
        PROFILE_PLAYER_NAME_SETTEXT_SUBS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_player_name_push_failures",
        PROFILE_PLAYER_NAME_PUSH_FAILURES.load(Ordering::SeqCst),
    );
    // 7e7 fail-closed guard: pushes skipped because the resolved component was stale (crash
    // avoided), plus the last stale component/vtable pointers for root-causing the bad link.
    push_json_usize(
        body,
        "oracle_stats_text_push_stale_skips",
        PROFILE_STATS_PUSH_STALE_SKIPS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_stats_text_push_stale_last_comp",
        PROFILE_STATS_PUSH_STALE_LAST_COMP.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_stats_text_push_stale_last_vt",
        PROFILE_STATS_PUSH_STALE_LAST_VT.load(Ordering::SeqCst),
    );
    // Per-slot save-stats cache (bd er-effects-rs-l90): cache_state 1 == the live `.sl2` was read
    // and parsed (each row shows its own character's attributes); 2 == read failed (fell back to
    // the loaded character). decoded == how many of the 10 save slots held a real character.
    push_json_usize(
        body,
        "oracle_stats_text_slot_cache_state",
        PROFILE_SLOT_STATS_CACHE_STATE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_stats_text_slot_decoded",
        PROFILE_SLOT_STATS_DECODED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_player_name_slot_decoded",
        PROFILE_SLOT_NAMES_DECODED.load(Ordering::SeqCst),
    );
    // Bit N set == save slot N has a name but no decoded stat block, i.e. its Load Character row
    // renders the merged header with an empty attribute line and no `WL`. Non-zero is a defect,
    // not a state: it is the semaphore for the 2026-09-01 report ("the row for the slot we
    // currently have loaded shows none of the stats"), which reached the user as a visual
    // observation because `decoded`/`named` were published as two independent counts and nothing
    // compared them.
    push_json_usize(
        body,
        "oracle_stats_text_slot_named_without_stats_mask",
        PROFILE_SLOT_STATS_NAMED_WITHOUT_STATS_MASK.load(Ordering::SeqCst),
    );
    // Bit N set == live `CS::ProfileSummary` slot N was marked occupied while holding something
    // that is not a character, sampled at a moment when no save picker owned the rows. That is the
    // RAM signature of `er-effects-rs-fmy6`: the in-game picker renders by writing its browse-row
    // labels into these game-owned records, and a restore that does not run leaves `[..] EldenRing`
    // / `[ new ]` where a character name belongs -- which the user then reads off a loading screen.
    // Non-zero is a defect, not a state, and it is sticky (`fetch_or`) because the per-frame sweep
    // heals an orphaned stomp within a frame: a clearable counter would read 0 in the very run that
    // proved the bug. Read it with `..._scans`, which distinguishes "checked and clean" from
    // "never checked".
    push_json_usize(
        body,
        "oracle_profile_summary_orphaned_record_mask",
        er_telemetry_core::counters::PROFILE_SUMMARY_ORPHANED_RECORD_MASK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_summary_orphaned_record_scans",
        er_telemetry_core::counters::PROFILE_SUMMARY_ORPHANED_RECORD_SCANS.load(Ordering::SeqCst),
    );
    // Stats-panel 05_010 runtime GFX edit oracles (mirror the 05_000 runtime-strip set).
    push_json_usize(
        body,
        "oracle_profile_05_010_runtime_edit_armed",
        PROFILE_05_010_RUNTIME_EDIT_ARMED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_05_010_runtime_edit_serves",
        PROFILE_05_010_RUNTIME_EDIT_SERVES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_05_010_runtime_edit_failures",
        PROFILE_05_010_RUNTIME_EDIT_FAILURES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_05_010_runtime_edit_input_len",
        PROFILE_05_010_RUNTIME_EDIT_INPUT_LEN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_05_010_runtime_edit_output_len",
        PROFILE_05_010_RUNTIME_EDIT_OUTPUT_LEN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_05_010_runtime_edit_input_class",
        PROFILE_05_010_RUNTIME_EDIT_INPUT_CLASS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_05_010_runtime_edit_output_validated",
        PROFILE_05_010_RUNTIME_EDIT_OUTPUT_VALIDATED.load(Ordering::SeqCst),
    );
    // Camera-lever (custom profile-portrait viewport) RAM semaphores: a runtime watcher can confirm
    // the override path ran and produced a sane matrix without an image. See bd
    // `camera-lever-RE-VERIFIED-offsets-and-call-addrs-2026-06-29`.
    push_json_usize(
        body,
        "oracle_profile_cam_apply_calls",
        PROFILE_CAM_APPLY_CALLS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_cam_latched_mask",
        PROFILE_CAM_LATCHED_MASK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_cam_face_yaw_latched_mask",
        PROFILE_CAM_FACE_YAW_LATCHED_MASK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_cam_last_slot",
        PROFILE_CAM_LAST_SLOT.load(Ordering::SeqCst),
    );
    push_json_bool(
        body,
        "oracle_profile_cam_last_matrix_ok",
        PROFILE_CAM_LAST_MATRIX_OK.load(Ordering::SeqCst) != 0,
    );
    // Display path (keepalive): the loading-screen image refreshes per-frame only if the
    // display path (keepalive): the loading-screen image follows the cursor per-frame only if the
    // Present overlay composites + re-uploads each frame. present_hook_hits = Present detour frames;
    // overlay_draw_hits = backbuffer composites; overlay_reuploads = per-frame texture rebuilds from a
    // version-bumped LOADING_BG_PORTRAIT_RGBA (the displayed portrait refreshed, not frozen).
    push_json_usize(
        body,
        "oracle_profile_readback_some",
        PROFILE_READBACK_SOME.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_profile_readback_checker",
        PROFILE_READBACK_CHECKER.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_present_hook_hits",
        PRESENT_HOOK_HITS.load(Ordering::SeqCst),
    );
    // Swapchain-find reject attribution (present_overlay.rs FIND_STAGE_*): stage 1-4 = chain link
    // null, 5-9 = candidate rejected (6=vt not module-backed, 7=vt in game exe, 8=stability wait,
    // 9=QI rejected), 10/11 = accepted (exact vtable match / QI fallback). Added after the
    // 2026-07-15 native-Windows runs where an opaque "chain miss" hid which predicate refused the
    // real swapchain for three full probes.
    push_json_usize(
        body,
        "oracle_present_find_tries",
        GAME_SWAPCHAIN_FIND_TRIES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_present_find_stage",
        PRESENT_FIND_STAGE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_present_find_candidate",
        PRESENT_FIND_CANDIDATE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_present_find_candidate_vt",
        PRESENT_FIND_CANDIDATE_VT.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_present_find_vt_module_kind",
        PRESENT_FIND_VT_MODULE_KIND.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_present_find_got8",
        PRESENT_FIND_GOT8.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_present_find_got22",
        PRESENT_FIND_GOT22.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_present_find_streak",
        PRESENT_FIND_STREAK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_present_accept_path",
        PRESENT_ACCEPT_PATH.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_present_backbuffer_format",
        PRESENT_BACKBUFFER_FORMAT.load(Ordering::SeqCst),
    );
    // Presents where we skipped all compositing because the now-loading display window had not opened
    // yet -- the pure-passthrough gate that keeps our GPU work out of the fragile early-boot crash
    // window on native Windows (er-effects-rs-n4x). High during boot, stops once now-loading opens.
    push_json_usize(
        body,
        "oracle_present_composite_early_skips",
        PRESENT_COMPOSITE_EARLY_SKIPS.load(Ordering::SeqCst),
    );
    // Native-Windows loading overlay (separate window + own D3D12 device, er-effects-rs-8jz):
    // stage = how far init got (10 = render loop live); frames = frames presented on our swapchain
    // (proof the isolated overlay is rendering); show = current visibility request from loading state.
    push_json_usize(
        body,
        "oracle_native_overlay_stage",
        NATIVE_OVERLAY_STAGE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_native_overlay_frames",
        NATIVE_OVERLAY_FRAMES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_native_overlay_show",
        NATIVE_OVERLAY_SHOW.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_overlay_stats_draw_hits",
        OVERLAY_STATS_DRAW_HITS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_onto_draw_hits",
        PORTRAIT_ONTO_DRAW_HITS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_alpha_cover_pct",
        PORTRAIT_ALPHA_COVER_PCT.load(Ordering::SeqCst),
    );
    // The raw crop envelope behind that percentage, plus the orbit camera the portrait was
    // rendered with. Split into `portrait_framing_oracles.rs` because this file has no room left
    // under the hard size gate; see that file's header for what each key answers.
    write_portrait_framing_oracles(body);
    // Boot-progress view semaphores: draw_hits = strip composites actually reaching the backbuffer
    // (the pre-Continue black frames are covered); last_permille = displayed progress; milestone_mask/
    // idx = which boot semaphores latched (bit order: Boot, game, offline, title, menu, continue,
    // loading); stopped = the handoff to the loading-portrait window fired.
    push_json_usize(
        body,
        "oracle_boot_view_draw_hits",
        BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_last_permille",
        BOOT_VIEW_LAST_PERMILLE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_milestone_mask",
        BOOT_VIEW_REACHED_MASK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_milestone_idx",
        BOOT_VIEW_MILESTONE_IDX.load(Ordering::SeqCst),
    );
    // Load epoch identity (bd er-effects-rs-ok8d): seq increments on every bar rearm, kind selects
    // which phase sequence the bar publishes (0 = process boot, 1 = character reload). Together
    // they make "did the bar actually start a fresh epoch, with the right phase set?" a RAM oracle
    // instead of something only inferable from the debug log.
    push_json_usize(
        body,
        "oracle_boot_view_epoch_seq",
        BOOT_VIEW_EPOCH_SEQ.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_epoch_kind",
        BOOT_VIEW_EPOCH_KIND.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_own_menu_load_active",
        BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_loadscreen_table_baseline",
        BOOT_VIEW_LOADSCREEN_TABLE_BASELINE.load(Ordering::SeqCst),
    );
    // Seamless-handoff semaphores: handoff_seen_ms = boot-view epoch ms when the loading/world
    // handoff was first detected (0 = not yet; the cover holds fully lit from here);
    // stop_native_hits = CS::LoadingScreen update ticks (baselined per load) when the cover
    // cut -- >= the lit threshold proves the cut landed on a lit loading screen.
    push_json_usize(
        body,
        "oracle_boot_view_handoff_seen_ms",
        BOOT_VIEW_HANDOFF_SEEN_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_stop_native_hits",
        BOOT_VIEW_STOP_NATIVE_HITS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_dark_gap_failures",
        BOOT_VIEW_DARK_GAP_FAILURES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_dark_gap_last_held_ms",
        BOOT_VIEW_DARK_GAP_LAST_HELD_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_dark_gap_last_native_hits",
        BOOT_VIEW_DARK_GAP_LAST_NATIVE_HITS.load(Ordering::SeqCst),
    );
    // Cover-window measurability (bd er-effects-rs-dpf6 Phase 1): why the cover last stopped
    // (0 armed/none, 1 release-fade, 2 fps-bail, 3 can-move world handoff, 4 absolute
    // BACKSTOP), the last window's rearm->stop duration, and how many times the fps-bail was
    // resumed by a fresh publish. Reason 4 is the cover having failed, not having worked.
    push_json_usize(
        body,
        "oracle_boot_view_stop_reason",
        er_telemetry_core::counters::BOOT_VIEW_STOP_REASON.load(Ordering::SeqCst),
    );
    // Absolute cover BACKSTOP (user report 2026-08-30). `releases` must stay 0: any nonzero
    // value means a cover window had no reachable exit and had to be torn down by force, which
    // is a defect to investigate (start at the CS::LoadingScreen hook install, not the cover).
    // `trigger` 1 = the world was demonstrably live under an opaque cover, 2 = the window
    // outlived the wall-clock cover lifetime.
    push_json_usize(
        body,
        "oracle_boot_view_backstop_releases",
        er_telemetry_core::counters::BOOT_VIEW_BACKSTOP_RELEASES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_backstop_first_ms",
        er_telemetry_core::counters::BOOT_VIEW_BACKSTOP_FIRST_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_backstop_trigger",
        er_telemetry_core::counters::BOOT_VIEW_BACKSTOP_TRIGGER.load(Ordering::SeqCst),
    );
    // Cover end-condition health (er-effects-rs-drb7). semantic_releases should equal the load
    // window count; the two latches say which half is missing when it does not.
    push_json_usize(
        body,
        "oracle_boot_view_semantic_releases",
        er_telemetry_core::counters::BOOT_VIEW_SEMANTIC_RELEASES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_release_render_ready_seen",
        er_telemetry_core::counters::BOOT_VIEW_RELEASE_RENDER_READY_SEEN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_release_native_done_seen",
        er_telemetry_core::counters::BOOT_VIEW_RELEASE_NATIVE_DONE_SEEN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_release_ready_ms",
        er_telemetry_core::counters::BOOT_VIEW_RELEASE_READY_MS.load(Ordering::SeqCst),
    );
    // q6vk character-load gate. held_for_confirm > 0 proves the gate engaged on a switch;
    // before_confirm must stay 0 -- a release without its character load is the defect back.
    push_json_usize(
        body,
        "oracle_boot_view_release_held_for_confirm",
        er_telemetry_core::counters::BOOT_VIEW_RELEASE_HELD_FOR_CONFIRM.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_release_before_confirm",
        er_telemetry_core::counters::BOOT_VIEW_RELEASE_BEFORE_CONFIRM.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_release_require_confirm",
        er_telemetry_core::counters::BOOT_VIEW_RELEASE_REQUIRE_CONFIRM.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_cover_window_ms",
        er_telemetry_core::counters::BOOT_VIEW_COVER_WINDOW_MS_LAST.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_fps_bail_resumes",
        er_telemetry_core::counters::BOOT_VIEW_FPS_BAIL_RESUMES.load(Ordering::SeqCst),
    );
    let native_loading_updates = LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst);
    let forced_continue_observed = SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT.load(Ordering::SeqCst)
        != 0
        || TFC_CONTINUE_FIRED.load(Ordering::SeqCst) != 0
        || OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst);
    let real_handoff_observed = forced_continue_observed || native_loading_updates != 0;
    if BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst) != 0
        && real_handoff_observed
        && BOOT_VIEW_HANDOFF_SEEN_MS.load(Ordering::SeqCst) == 0
    {
        let now_ms = crate::experiments::boot_view_epoch_ms().max(1) as usize;
        if BOOT_VIEW_HANDOFF_SEEN_MS
            .compare_exchange(0, now_ms, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            BOOT_VIEW_HANDOFF_NATIVE_HITS_BASELINE.store(native_loading_updates, Ordering::SeqCst);
            BOOT_VIEW_TELEMETRY_HANDOFF_STAMPS.fetch_add(1, Ordering::SeqCst);
        }
    }
    push_json_usize(
        body,
        "oracle_boot_view_telemetry_handoff_stamps",
        BOOT_VIEW_TELEMETRY_HANDOFF_STAMPS.load(Ordering::SeqCst),
    );
    let boot_view_missed_handoff = (BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst) != 0
        && real_handoff_observed
        && BOOT_VIEW_HANDOFF_SEEN_MS.load(Ordering::SeqCst) == 0)
        as usize;
    push_json_usize(
        body,
        "oracle_boot_view_missed_handoff_failures",
        boot_view_missed_handoff,
    );
    // Window-reconfiguration timeline semaphores (bd er-effects-rs-rzow): user32 call counts
    // from the observe-only hooks, plus the early final-geometry apply result (1 = applied,
    // 2 = skipped WINDOWED, 3 = no window, 4 = no monitor, 5 = no config, 6 = already final).
    push_json_usize(
        body,
        "oracle_winreconfig_create_window_calls",
        WINRECONFIG_CREATE_WINDOW_CALLS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_winreconfig_set_window_pos_calls",
        WINRECONFIG_SET_WINDOW_POS_CALLS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_winreconfig_set_window_long_calls",
        WINRECONFIG_SET_WINDOW_LONG_CALLS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_winreconfig_move_window_calls",
        WINRECONFIG_MOVE_WINDOW_CALLS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_winreconfig_change_display_calls",
        WINRECONFIG_CHANGE_DISPLAY_CALLS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_winreconfig_early_apply_result",
        WINRECONFIG_EARLY_APPLY_RESULT.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_winreconfig_early_apply_ms",
        WINRECONFIG_EARLY_APPLY_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_winreconfig_early_apply_rect",
        WINRECONFIG_EARLY_APPLY_RECT.load(Ordering::SeqCst),
    );
    let pump_stop_reason = BOOT_VIEW_PUMP_STOP_REASON.load(Ordering::SeqCst);
    let boot_view_stopped = BOOT_VIEW_STOPPED.load(Ordering::SeqCst);
    // A composite attributes its clear before incrementing DRAW_HITS. Read draws first so a
    // concurrent composite can only make attribution newer than draws, never the reverse tuple
    // that would latch a false failure while the Present-path clear is still in flight.
    let draws = BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst);
    let self_presents = BOOT_VIEW_SELF_PRESENTS.load(Ordering::SeqCst);
    let self_full_clears = BOOT_VIEW_SELF_FULL_CLEAR_HITS.load(Ordering::SeqCst);
    let present_full_clears = BOOT_VIEW_PRESENT_FULL_CLEAR_HITS.load(Ordering::SeqCst);
    push_json_usize(body, "oracle_boot_view_self_presents", self_presents);
    push_json_usize(
        body,
        "oracle_boot_view_self_full_clear_hits",
        self_full_clears,
    );
    push_json_usize(
        body,
        "oracle_boot_view_present_full_clear_hits",
        present_full_clears,
    );
    if boot_view_present_cover_failed(
        pump_stop_reason,
        boot_view_stopped,
        draws,
        self_full_clears,
        present_full_clears,
    ) {
        BOOT_VIEW_PRESENT_COVER_FAILURES.store(1, Ordering::SeqCst);
    }
    let cur_deser =
        crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
    let can_move = crate::constants::CAN_MOVE_CONFIRMED.load(Ordering::SeqCst)
        && crate::constants::MOVE_PROBE_EPOCH.load(Ordering::SeqCst) == cur_deser;
    let render_ready = match unsafe { PlayerIns::local_player_mut() } {
        Ok(p) => {
            let m = p.chr_ins.chr_model_ins.as_ptr() as usize;
            let c = p.chr_ins.chr_ctrl.as_ptr() as usize;
            m != TITLE_OWNER_SCAN_START_ADDRESS
                && c != TITLE_OWNER_SCAN_START_ADDRESS
                && p.chr_ins.chr_flags1c4.is_render_group_enabled()
                && p.chr_ins.chr_flags1c5.enable_render()
        }
        Err(_) => false,
    };
    let pre_world_stop_failure =
        (BOOT_VIEW_STOPPED.load(Ordering::SeqCst) != 0 && !(can_move || render_ready)) as usize;
    if pre_world_stop_failure != 0 {
        BOOT_VIEW_PRE_WORLD_STOP_FAILURES.store(1, Ordering::SeqCst);
    }
    push_json_usize(
        body,
        "oracle_boot_view_present_cover_failures",
        BOOT_VIEW_PRESENT_COVER_FAILURES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_pre_world_stop_failures",
        BOOT_VIEW_PRE_WORLD_STOP_FAILURES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_swapchain_found_ms",
        BOOT_VIEW_SWAPCHAIN_FOUND_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_fade_start_ms",
        BOOT_VIEW_FADE_START_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_fade_complete_ms",
        BOOT_VIEW_FADE_COMPLETE_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_fade_hits",
        BOOT_VIEW_FADE_HITS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_fade_last_alpha",
        BOOT_VIEW_FADE_LAST_ALPHA.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_fade_failures",
        BOOT_VIEW_FADE_FAILURES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_native_gfx_fade_hold_hits",
        BOOT_VIEW_NATIVE_GFX_FADE_HOLD_HITS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_native_gfx_fade_hold_complete_ms",
        BOOT_VIEW_NATIVE_GFX_FADE_HOLD_COMPLETE_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_pump_stop_reason",
        BOOT_VIEW_PUMP_STOP_REASON.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_pump_stop_ms",
        BOOT_VIEW_PUMP_STOP_MS.load(Ordering::SeqCst) as usize,
    );
    push_json_usize(
        body,
        "oracle_depth_key_applied",
        DEPTH_KEY_APPLIED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_coherent_read_ok",
        COHERENT_READ_OK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_coherent_read_fallback",
        COHERENT_READ_FALLBACK.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_mask_stale_reuse",
        PROFILE_MASK_STALE_REUSE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_mask_head_iou_last",
        PROFILE_MASK_HEAD_IOU_LAST.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_mask_head_mismatch_total",
        PROFILE_MASK_HEAD_MISMATCH_TOTAL.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_depth_key_bg_pct",
        DEPTH_KEY_BG_PCT.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_depth_key_fresh",
        DEPTH_KEY_FRESH.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_loading_bg_portrait_rgba_version",
        LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst),
    );
    // Loading-screen portrait bug SEMAPHORES (2026-07-04). Detection runs at capture time
    // (`note_ls_portrait_capture`, called wherever a portrait RGBA is stored) so a transient
    // wrong-source frame -- our neutral texture (RGB 30,28,26) flashing in right after Continue (Bug
    // B), or a too-small 256px head (Bug A) -- cannot slip between telemetry writes. Here we just
    // publish the latched values.
    push_json_usize(
        body,
        "oracle_ls_portrait_w",
        LS_PORTRAIT_LAST_W.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_ls_portrait_h",
        LS_PORTRAIT_LAST_H.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_ls_portrait_neutral_pct",
        LS_PORTRAIT_LAST_NEUTRAL_PCT.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_ls_portrait_too_small_seen_version",
        LS_PORTRAIT_TOO_SMALL_SEEN_VERSION.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_ls_portrait_neutral_leak_seen_version",
        LS_PORTRAIT_NEUTRAL_LEAK_SEEN_VERSION.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_ls_portrait_rejected_publishes",
        LS_PORTRAIT_REJECTED_PUBLISHES.load(Ordering::SeqCst),
    );
    // Reject attribution (er-effects-rs-k979). The bare count above cannot say whether the
    // neutral gate was doing its job or the pipeline broke. `_after_window_publish` is the one
    // a proof should gate on; `_before_window_publish` is warm-up and expected. Both are scoped
    // to the current window -- the version counter they derive from is process-cumulative.
    push_json_usize(
        body,
        "oracle_ls_portrait_rejects_before_window_publish",
        er_telemetry_core::counters::LS_PORTRAIT_REJECTS_BEFORE_WINDOW_PUBLISH
            .load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_ls_portrait_rejects_after_window_publish",
        er_telemetry_core::counters::LS_PORTRAIT_REJECTS_AFTER_WINDOW_PUBLISH
            .load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_ls_portrait_reject_last_version",
        er_telemetry_core::counters::LS_PORTRAIT_REJECT_LAST_VERSION.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_ls_portrait_reject_last_neutral_pct",
        er_telemetry_core::counters::LS_PORTRAIT_REJECT_LAST_NEUTRAL_PCT.load(Ordering::SeqCst),
    );
    // Publish identity + race measurability (bd er-effects-rs-dpf6 Phase 1): which character the
    // published head belongs to (slot+1 / FNV-1a64 name hash; 0 = no head/unknown), the last
    // measured switch-confirm -> publish latency, and the Phase-3 same-identity bridge holds
    // (with their outcomes, in `portrait_bridge_hold_oracles.rs`).
    push_json_usize(
        body,
        "oracle_ls_portrait_slot",
        er_telemetry_core::counters::LS_PORTRAIT_PUBLISHED_SLOT.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_ls_portrait_name_hash",
        er_telemetry_core::counters::LS_PORTRAIT_PUBLISHED_NAME_HASH.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_confirm_to_publish_ms",
        er_telemetry_core::counters::PORTRAIT_CONFIRM_TO_PUBLISH_MS_LAST.load(Ordering::SeqCst),
    );
}
