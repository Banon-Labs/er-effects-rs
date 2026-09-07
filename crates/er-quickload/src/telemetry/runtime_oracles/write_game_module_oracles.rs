// The game-module oracle emission spine.
//
// This file used to hold the whole 3,206-line emission as a single function. It is now the order
// that emission happens in, and nothing else: each subsystem lives in its own `oracles_*.rs`
// sibling and is called from here. Order is load-bearing -- the telemetry JSON is read positionally
// by nothing, but a reader diffing two runs relies on the field order being stable -- so the calls
// below are in exactly the sequence the fields were emitted in before the split.
//
// The seams were chosen where the source already had them: at every boundary below, at most one
// local value flows onward, and those two are returned/passed explicitly rather than recomputed.

fn write_game_module_oracles(body: &mut String) {
    // Outside the `game_module_base` gate on purpose: the frame-time counters are ours, so the FPS
    // oracle must still be emitted on a run where the game module cannot be resolved.
    write_frame_pacing_oracles(body);
    if let Ok(base) = crate::experiments::game_module_base() {
        write_engine_loop_oracles(body, base);
        let play_time_live = write_character_identity_oracles(body);
        write_render_state_oracles(body, base);
        let title_custom_cover_profile_source_ready = write_title_visual_oracles(body, base);
        write_loading_cover_oracles(body);
        write_portrait_pipeline_oracles(body, base);
        write_loading_screen_live_oracles(
            body,
            play_time_live,
            title_custom_cover_profile_source_ready,
        );
    }
}
