// Live loading-screen oracles: the gauge reported as its live state rather than its stale
// during-load latch, the five observer install states, the knowledge-tip and Scaleform-descriptor
// guards, and the native-profile capture field.
//
// Last in the emission order, and the only subsystem that consumes values computed by earlier
// ones: `play_time_live` decides whether the gauge is reported live or zeroed, and
// `title_custom_cover_profile_source_ready` is the native-profile capture readiness. Both are
// parameters rather than re-reads, so this block cannot disagree with the fields that published
// them earlier in the same telemetry write.

fn write_loading_screen_live_oracles(
    body: &mut String,
    play_time_live: bool,
    title_custom_cover_profile_source_ready: bool,
) {
    // Live-state (bd STEP4-loadingbar-divergences-are-stale-latch): the loading-screen gauge is a stale
    // latch -- sample_loading_screen_bar stores it during the load and never fires post-load to clear it.
    // When the world is genuinely live (play_time advancing = steady gameplay) the loading screen is
    // logically closed, so report the gauge as 0 (the live state), not the stale during-load latch. This
    // makes vanilla (telemetry-only) and mod (armed) comparable instead of diverging on a leftover latch.
    let loading_bar_enabled = if play_time_live {
        0
    } else {
        LOADING_SCREEN_BAR_ENABLED.load(Ordering::SeqCst)
    };
    let loading_bar_current_frame = LOADING_SCREEN_BAR_CURRENT_FRAME.load(Ordering::SeqCst);
    let loading_bar_max_frame = LOADING_SCREEN_BAR_MAX_FRAME.load(Ordering::SeqCst);
    let loading_bar_progress_permille = LOADING_SCREEN_BAR_PROGRESS_PERMILLE.load(Ordering::SeqCst);
    let loading_bar_current_terminal = usize::from(
        loading_bar_enabled != 0
            && ((loading_bar_max_frame != 0 && loading_bar_current_frame >= loading_bar_max_frame)
                || loading_bar_progress_permille >= 998),
    );
    push_json_usize(body, "oracle_loading_bar_enabled", loading_bar_enabled);
    push_json_usize(
        body,
        "oracle_loading_bar_current_frame",
        loading_bar_current_frame,
    );
    push_json_usize(body, "oracle_loading_bar_max_frame", loading_bar_max_frame);
    push_json_usize(
        body,
        "oracle_loading_bar_progress_permille",
        loading_bar_progress_permille,
    );
    push_json_usize(
        body,
        "oracle_loading_bar_current_terminal",
        loading_bar_current_terminal,
    );
    push_json_usize(
        body,
        "oracle_loading_bar_final_hits",
        LOADING_SCREEN_BAR_FINAL_HITS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_loading_screen_close_sent",
        if play_time_live {
            0
        } else {
            LOADING_SCREEN_CLOSE_SENT.load(Ordering::SeqCst)
        },
    );
    push_json_usize(
        body,
        "oracle_loading_screen_close_sent_hits",
        LOADING_SCREEN_CLOSE_SENT_HITS.load(Ordering::SeqCst),
    );
    // Five observer install states, one field each: 0 = not attempted, 1 = installed,
    // 2 = permanently refused, 3 = queued awaiting MH_ApplyQueued. They are separate fields
    // because a shared one is what hid the defect: four detours were created and never
    // applied, and every hit counter downstream read 0 -- indistinguishable from a hook that
    // was installed and simply never called. See `install_now_loading_helper_observer_hooks`.
    push_json_usize(
        body,
        "oracle_now_loading_helper_ctor_hook_installed",
        NOW_LOADING_HELPER_CTOR_HOOK_INSTALLED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_now_loading_helper_update_hook_installed",
        NOW_LOADING_HELPER_UPDATE_HOOK_INSTALLED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_scaleform_label_goto_hook_installed",
        SCALEFORM_LABEL_GOTO_HOOK_INSTALLED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_loading_screen_gfx_fadeout_hook_installed",
        LOADING_SCREEN_GFX_FADEOUT_HOOK_INSTALLED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_loading_screen_gfx_fadeout_hits",
        LOADING_SCREEN_GFX_FADEOUT_HITS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_native_screens_seen",
        er_telemetry_core::counters::BOOT_VIEW_NATIVE_SCREENS_SEEN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_boot_view_release_held_for_second_screen",
        er_telemetry_core::counters::BOOT_VIEW_RELEASE_HELD_FOR_SECOND_SCREEN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_loading_screen_gfx_fadeout_foreign_hits",
        LOADING_SCREEN_GFX_FADEOUT_FOREIGN_HITS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_loading_screen_gfx_fadeout_first_ms",
        LOADING_SCREEN_GFX_FADEOUT_FIRST_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_loading_screen_gfx_fadeout_last_ms",
        LOADING_SCREEN_GFX_FADEOUT_LAST_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_loading_screen_update_last_ms",
        LOADING_SCREEN_UPDATE_LAST_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_loading_screen_close_sent_first_ms",
        LOADING_SCREEN_CLOSE_SENT_FIRST_MS.load(Ordering::SeqCst),
    );
    // Pivot (er-effects-rs-jsm): player-stats loading text. `stats_text_built` = cumulative count of
    // stats bitmaps rendered from the game font (content-keyed rebuilds: a character switch or the
    // record->live upgrade bumps it); `tip_suppressed_hits` = native tip-refresh calls we no-op'd.
    push_json_usize(
        body,
        "oracle_stats_text_built",
        STATS_TEXT_BUILT.load(Ordering::SeqCst),
    );
    // `stats_record_not_a_character` = stats reads refused because the loading slot's live
    // `CS::ProfileSummary` record is not a character (empty name, level 0, a save-picker browse-row
    // label such as `[..] EldenRing` / `[ new ]`, or a populated-but-implausible map). Read it
    // with `stats_text_built`: a refused panel and a disabled feature both draw nothing, and this
    // is the only field that tells them apart. Non-zero means something wrote non-character bytes
    // into the records -- the picker-restore defect that made this invisible (2026-08-30).
    push_json_usize(
        body,
        "oracle_stats_record_not_a_character",
        er_telemetry_core::counters::STATS_RECORD_NOT_A_CHARACTER.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_tip_suppressed_hits",
        KNOWLEDGE_TIP_SUPPRESSED_HITS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_tip_suppress_installed",
        KNOWLEDGE_TIP_REFRESH_INSTALLED.load(Ordering::SeqCst),
    );
    // `tip_advance_disable_installed` = the advance enabled-predicate detour is live;
    // `tip_advance_suppressed_hits` = predicate calls we forced false (keyguide hidden + press inert).
    push_json_usize(
        body,
        "oracle_tip_advance_disable_installed",
        KNOWLEDGE_TIP_ADVANCE_ENABLED_INSTALLED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_tip_advance_suppressed_hits",
        KNOWLEDGE_TIP_ADVANCE_SUPPRESSED_HITS.load(Ordering::SeqCst),
    );
    // `scaleform_desc_guard_installed` = the descriptor-heap null-guard detour is live;
    // `scaleform_desc_provider_null_hits` = advances we skipped because the provider was null
    // (the exact condition that AVs at deobf 0x140ec95d1 / rva 0xec95d1). A non-zero hit count is
    // direct evidence the guard caught the crash condition. (er-effects-rs-y22i.)
    push_json_usize(
        body,
        "oracle_scaleform_desc_guard_installed",
        SCALEFORM_DESC_ADVANCE_INSTALLED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_scaleform_desc_provider_null_hits",
        SCALEFORM_DESC_PROVIDER_NULL_HITS.load(Ordering::SeqCst),
    );
    body.push_str(&format!(
        "  \"oracle_native_profile_capture_enabled\": {},\n  \"oracle_native_profile_source_ready\": {},\n  \"oracle_native_profile_source_name\": \"{}\",\n  \"oracle_native_profile_renderer_class\": \"{}\",\n",
        // The native-profile capture mode was a permanently-off diagnostic gate; the field is
        // kept (its shape is consumed by scripts/er-readiness-watch.py) and is now always false.
        false,
        title_custom_cover_profile_source_ready,
        TITLE_CUSTOM_COVER_SYSTEX_TARGET,
        TITLE_CUSTOM_COVER_PROFILE_RENDERER_CLASS,
    ));
}
