// Portrait FRAMING oracles: the frozen crop envelope and the applied orbit camera.
//
// These live in their own include file rather than inline in `write_game_module_oracles.rs` only
// because that file sits within ~20 lines of the 3200-line hard gate
// (`scripts/check-rust-file-sizes.py`) and has no room for a group this size; the split is the same
// mechanism `runtime_oracles.rs` already documents for exactly that pressure.
//
// What the group as a whole answers: "why is this character's portrait framed like that, and is the
// framing the same as the last character's?" Until 2026-08-21 an artifact set could not answer either
// question -- the apparent size of the portrait is decided by a crop rect and an orbit camera, and
// neither exported a single value, only derived summaries and "it ran" counters.

fn write_portrait_framing_oracles(body: &mut String) {
    // CROP ENVELOPE (portrait_overlay.rs): the head's alpha bounding box, unioned over the first
    // PORTRAIT_CROP_SEED_N frames and then FROZEN; the crop is rescaled so its height is 80% of the
    // screen, which makes this rect -- not the camera alone -- the last word on apparent portrait size.
    // Only the derived AREA (`oracle_portrait_alpha_cover_pct`, emitted just above) was exported, and a
    // percentage cannot say WHERE the rect is or what shape it has, so two characters' framing could not
    // be compared and an implausible percentage could not be attributed. The four bounds do both.
    //
    // Reading them: minx/miny start at `usize::MAX` and maxx/maxy at 0, so a run that never seeded
    // reports exactly that sentinel pair rather than a plausible-looking rect. `seed_frames` counts the
    // frames offered to the envelope; once it reaches PORTRAIT_CROP_SEED_N the rect can no longer move,
    // so a bad rect at that point is permanent for the rest of the loading screen. A rect covering
    // (near) the whole source is the signature of one fully-opaque, unkeyed frame having been folded in
    // -- which is what a measured cover_pct of 99 against a depth_key_bg_pct of 76 implies.
    push_json_usize(
        body,
        "oracle_portrait_crop_minx",
        PORTRAIT_CROP_MINX.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_crop_miny",
        PORTRAIT_CROP_MINY.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_crop_maxx",
        PORTRAIT_CROP_MAXX.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_crop_maxy",
        PORTRAIT_CROP_MAXY.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_crop_seed_frames",
        PORTRAIT_CROP_SEED_FRAMES.load(Ordering::SeqCst),
    );
    // APPLIED ORBIT CAMERA (lookat_stage_camera.rs `apply_profile_camera_override`): the seven values
    // the last successful apply wrote into the renderer. The neighbouring `oracle_profile_cam_*`
    // counters say the override ran, on which slot, and that the matrix came out finite -- none of them
    // says what the camera WAS, so "every character gets the same portrait camera" was a static-dump
    // inference (`MenuOffscrRendParam` row 20 for all ten slots) that no run could confirm or refute.
    // With these, two characters' artifacts either carry identical numbers or they do not.
    //
    // These are the APPLIED values, i.e. the engine baseline after PROFILE_CAM_DISTANCE_SCALE /
    // _PITCH_DELTA_RAD / _YAW_DELTA_RAD / _FOV_SCALE, because that is what the render actually used;
    // the untransformed baseline goes to the autoload debug log at its latch site. Angles are radians.
    // All-zero means no apply has succeeded yet, which `oracle_profile_cam_apply_calls == 0` also says.
    let cam = |counter: &std::sync::atomic::AtomicUsize| -> f32 {
        f32::from_bits(counter.load(Ordering::SeqCst) as u32)
    };
    body.push_str(&format!(
        "  \"oracle_profile_cam_last_target_x\": {:.4},\n  \"oracle_profile_cam_last_target_y\": {:.4},\n  \"oracle_profile_cam_last_target_z\": {:.4},\n  \"oracle_profile_cam_last_distance\": {:.4},\n  \"oracle_profile_cam_last_pitch\": {:.4},\n  \"oracle_profile_cam_last_yaw\": {:.4},\n  \"oracle_profile_cam_last_fov\": {:.4},\n",
        cam(&PROFILE_CAM_LAST_TARGET_X_BITS),
        cam(&PROFILE_CAM_LAST_TARGET_Y_BITS),
        cam(&PROFILE_CAM_LAST_TARGET_Z_BITS),
        cam(&PROFILE_CAM_LAST_DISTANCE_BITS),
        cam(&PROFILE_CAM_LAST_PITCH_BITS),
        cam(&PROFILE_CAM_LAST_YAW_BITS),
        cam(&PROFILE_CAM_LAST_FOV_BITS),
    ));
    // UNMASKED-FRAME REFUSALS (mask gate, 2026-08-21). Both counters now have real writers, so they
    // are emitted here rather than held back by `scripts/check-oracle-writers.py`.
    //
    // `draw_refused_unmasked` counts frames the compositor declined to blit because the published
    // buffer had no alpha cut (`portrait_onto`, er-loading-portrait). `bake_publish_refused_unmasked`
    // counts the same refusal one stage earlier, at the two colour-only readback writers that can put
    // an opaque buffer into LOADING_BG_PORTRAIT_RGBA in the first place (`save_swap_profile_table.rs`
    // and the default-off diagnostic in `dlstring_lookat_math.rs`).
    //
    // How to read the pair against `oracle_portrait_onto_draw_hits` and the crop bounds above:
    //   * bake > 0, draw == 0 -- the capture side produced an unmasked frame and the publish gate ate
    //     it. Expected on the live path: the bake fires at FrameBegin before the depth-keyed worker has
    //     a frame to publish. The head appears when the worker publishes; nothing wrong.
    //   * draw > 0 -- an unmasked buffer got PAST publish and only the compositor stopped it. That is a
    //     publish-side hole, not a compositor one; find the writer.
    //   * both 0 with `oracle_portrait_alpha_cover_pct` near 100 -- the gate is not catching what it was
    //     built for. The percentage and the refusals disagreeing is the signal, which is exactly why a
    //     permanently-0 counter here would have been worse than no counter.
    push_json_usize(
        body,
        "oracle_portrait_draw_refused_unmasked",
        PORTRAIT_DRAW_REFUSED_UNMASKED.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_portrait_bake_publish_refused_unmasked",
        PORTRAIT_BAKE_PUBLISH_REFUSED_UNMASKED.load(Ordering::SeqCst),
    );
}
