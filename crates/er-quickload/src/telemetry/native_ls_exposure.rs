// Native loading-screen exposure SEMAPHORE (er-effects-rs-wmw defect #1, user report 2026-07-30:
// "the custom loading screen disappeared for about one frame and the vanilla loading screen flashed
// through").
//
// The defect is a per-frame event, so it needs a per-frame oracle: the aggregate counters we already
// had (`oracle_boot_view_stop_reason`, `oracle_portrait_onto_draw_hits`) can say the cover stopped,
// but never that a frame reached the screen with the game's own loading screen visible. This module
// closes that gap. On every Present the detour reports which gate decided this frame's cover; when
// the game's `CS::LoadingScreen` is live and the cover did not draw, the frame is latched as an
// exposure frame -- literally the frame the user saw vanilla -- and attributed to the blocking gate.
//
// Telemetry-only: nothing here changes what is drawn. It converts a visual report into a RAM oracle
// so the flash-through can be reproduced and attributed without reading a screenshot.
//
// Not the same thing as `BOOT_VIEW_NATIVE_EXPOSURE_FRAMES` / `oracle_boot_view_native_exposure_*`
// (branch loading-portrait-semaphores-20260730). That one lives in the cover state machine and
// counts times the cover was resumed because the native loading screen reappeared -- a mitigation,
// measured at the decision layer, which cannot say whether a frame actually reached the screen. This
// one is measured at Present, the last point before the user's eyes, and counts the frames
// themselves. Whether that resume mitigation works is exactly a question these counters can answer.
//
// Which loading screens this judges (2026-08-30). Not all of them. The product cover is a
// boot/character-load surface: it owns the dead early-boot gap and a System->Quit -> Load Character
// switch, and the game's own screen for a fast travel, a death respawn or an area transition was
// never ours to draw over. Those frames are now filed under `NATIVE_LS_GATE_UNOWNED_LOAD` instead
// of the defect gate -- see `er_telemetry_core::counters::cover_owns_current_loading_screen` for
// the discriminator, the measured run that forced the split, and why the obvious "a loading screen
// is up, so re-arm" signal is the one that must never be used.
//
// Reading the oracles:
//   oracle_native_ls_exposure_owned_frames  > 0  the DEFECT: a screen the cover owed the user and
//                                      did not draw. This is the acceptance number.
//   oracle_native_ls_exposure_frames   every uncovered vanilla-loading-screen frame, owned or not
//   oracle_native_ls_exposure_max_run  == 1 a one-frame flash (the exact user report)
//                                      >> 1 a sustained hole (e.g. the FPS bail killing the cover)
//   oracle_native_ls_exposure_by_gate  which gate blocked the cover, by `NATIVE_LS_GATE_*` code;
//                                      index 5 is the not-ours bucket and is expected to be nonzero
//                                      in any session with a fast travel
//   oracle_native_ls_exposure_last_stop_reason  BOOT_VIEW_STOP_REASON at the last exposure frame
//                                      (1 = release fade, 2 = FPS bail, 3 = world handoff)
//
// The other half of the frame (2026-08-22). This module only judges frames where the native
// loading screen is live, and that turned out to be a blind spot as large as the one it closed: the
// "loading screen reappears after Escape" report happens after the native screen stops ticking, so
// the only per-frame oracle in the DLL switched itself off for the whole window the user was
// describing and the reproducing run logged nothing at all. The stale frames now go to
// `cover_after_release.rs` instead of being dropped. Nothing here changed to make room for it.
//
// A frame is only judged while the native loading screen is live. `LOADING_SCREEN_UPDATE_LAST_MS` is
// stamped by the native CS::LoadingScreen::Update hook every frame the screen ticks, so freshness is
// the live signal. The window is deliberately wider than one frame (the game presents as slowly as
// ~5 fps during loading, bd FPS-delta-confirmed-load2-20fps-load1-45fps) so a slow frame is not
// misread as the screen having closed; it is still far below the 1500 ms window-close quiet period
// used by [`portrait_loadwin_tick`].
const NATIVE_LS_LIVE_FRESH_MS: u64 = 250;

/// Called once per Present on the game swapchain with the gate that decided this frame's cover.
/// Cheap by construction (a handful of relaxed atomics) -- it runs on the render thread.
///
/// `base` is the game module base, used only by the post-release mirror path below.
pub(crate) fn native_ls_exposure_record(base: usize, gate: usize) {
    use er_telemetry_core::counters::{
        BOOT_VIEW_STOP_REASON, LOADING_SCREEN_UPDATE_LAST_MS, NATIVE_LS_COVERED_FRAMES,
        NATIVE_LS_EXPOSURE_BY_GATE, NATIVE_LS_EXPOSURE_CUR_RUN, NATIVE_LS_EXPOSURE_FIRST_MS,
        NATIVE_LS_EXPOSURE_FRAMES, NATIVE_LS_EXPOSURE_LAST_GATE, NATIVE_LS_EXPOSURE_LAST_MS,
        NATIVE_LS_EXPOSURE_LAST_STOP_REASON, NATIVE_LS_EXPOSURE_MAX_RUN, NATIVE_LS_GATE_COUNT,
        NATIVE_LS_GATE_DREW,
    };
    let update_last = LOADING_SCREEN_UPDATE_LAST_MS.load(Ordering::SeqCst) as u64;
    let now_ms = crate::experiments::boot_view_epoch_ms().max(1);
    // One line, once, mapping this clock to the log's `[+Nms]` prefix. Placed here because this is
    // the earliest thing that runs every frame and already reads the boot-view clock, so it neither
    // needs a home of its own nor risks anchoring that clock. One atomic load per frame after the
    // first.
    log_clock_map_once();
    // Mirror path (2026-08-22). The staleness test below is what made this function blind to the
    // "loading screen reappears after Escape" report: the defect happens after the native screen
    // has stopped ticking, which is precisely when this returns. Those frames are not
    // uninteresting, they are the interesting ones -- hand them to the post-release watch instead
    // of dropping them. The accounting below is untouched; this is an added path, not a changed
    // one, and the two are mutually exclusive by construction.
    if update_last == 0 || now_ms.saturating_sub(update_last) > NATIVE_LS_LIVE_FRESH_MS {
        // The game's loading screen is not on screen this frame; nothing to be exposed -- but
        // something may still be covering, which is a different question with its own oracles.
        cover_after_release_record(base, now_ms);
        return;
    }
    if gate == NATIVE_LS_GATE_DREW {
        NATIVE_LS_COVERED_FRAMES.fetch_add(1, Ordering::SeqCst);
        NATIVE_LS_EXPOSURE_CUR_RUN.store(0, Ordering::SeqCst);
        return;
    }
    // Whose loading screen is this? The composite reports gate 4 for every frame it declined to
    // draw, which is the honest answer to the question it was asked ("did the cover draw?") and the
    // wrong answer to the one this module asks ("did the user see vanilla where our cover should
    // have been?"). A fast travel, a death respawn and an area transition all put a
    // `CS::LoadingScreen` up that the product has never covered. Re-file those under their own gate
    // so the defect bucket means the defect -- `er_telemetry_core::counters` carries the argument
    // and the run that forced it.
    //
    // Gate 2 reaches this too (run br-20260831-160354-2513). The re-file used to test only gate 4,
    // so the ownership question was never asked of `NATIVE_LS_GATE_EPOCH_WORLD_LIVE` -- and that
    // gate returns earlier, at `present_overlay.rs`'s epoch fast-path, upstream of the composite.
    // Every gate-2 frame was therefore counted `owned` unconditionally. Measured cost: that run
    // reported `oracle_native_ls_exposure_owned_frames = 318` with
    // `by_gate = [0,0,318,0,0,295]`, of which 310 were a plain fast travel by the reloaded
    // character (`warp_requested=true` at log 8479/8483, window 310412..319432 ms) -- the exact
    // category the gate-4 path was already excusing correctly two windows earlier, where the same
    // warp shape produced 243 frames filed as gate 5. So the identical event was a defect or not
    // depending only on which gate happened to report it first. Since `_OWNED_FRAMES` is documented
    // as "THIS is the number an acceptance gate reads", any gate on `owned_frames == 0` failed on a
    // fast travel.
    //
    // The predicate is gate-independent by construction -- it asks what is on screen, not who
    // declined to draw -- so asking it for both gates is the whole fix.
    let gate = if (gate == er_telemetry_core::counters::NATIVE_LS_GATE_COVER_STOPPED
        || gate == er_telemetry_core::counters::NATIVE_LS_GATE_EPOCH_WORLD_LIVE)
        && !er_telemetry_core::counters::cover_owns_current_loading_screen()
    {
        er_telemetry_core::counters::NATIVE_LS_GATE_UNOWNED_LOAD
    } else {
        gate
    };
    let owned = gate != er_telemetry_core::counters::NATIVE_LS_GATE_UNOWNED_LOAD;
    let n = NATIVE_LS_EXPOSURE_FRAMES.fetch_add(1, Ordering::SeqCst) + 1;
    let run = NATIVE_LS_EXPOSURE_CUR_RUN.fetch_add(1, Ordering::SeqCst) + 1;
    NATIVE_LS_EXPOSURE_MAX_RUN.fetch_max(run, Ordering::SeqCst);
    NATIVE_LS_EXPOSURE_LAST_GATE.store(gate, Ordering::SeqCst);
    if owned {
        let owned_n = er_telemetry_core::counters::NATIVE_LS_EXPOSURE_OWNED_FRAMES
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        if owned_n == 1 {
            er_telemetry_core::counters::NATIVE_LS_EXPOSURE_OWNED_FIRST_MS
                .store(now_ms as usize, Ordering::SeqCst);
        }
    }
    let stop_reason = BOOT_VIEW_STOP_REASON.load(Ordering::SeqCst);
    NATIVE_LS_EXPOSURE_LAST_STOP_REASON.store(stop_reason, Ordering::SeqCst);
    NATIVE_LS_EXPOSURE_LAST_MS.store(now_ms as usize, Ordering::SeqCst);
    let _ = NATIVE_LS_EXPOSURE_FIRST_MS.compare_exchange(
        0,
        now_ms as usize,
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
    if gate < NATIVE_LS_GATE_COUNT {
        NATIVE_LS_EXPOSURE_BY_GATE[gate].fetch_add(1, Ordering::SeqCst);
    }
    // Log the first frame of each hole, not every frame: a sustained hole is one event, and the
    // Present hook must not turn a render stall into an IO storm.
    if run == 1 && owned {
        append_autoload_debug(format_args!(
            "NATIVE-LS EXPOSURE #{n}: the vanilla loading screen reached the screen at {now_ms}ms -- cover blocked by gate={gate} ({}) boot_view_stop_reason={stop_reason}",
            native_ls_gate_name(gate)
        ));
    } else if run == 1 {
        // Said plainly, and once per hole, because the previous wording sent an investigation
        // after a cover re-arm for a load that never happened.
        append_autoload_debug(format_args!(
            "native loading screen at {now_ms}ms is the GAME's (fast travel / death / area transition) -- the product cover does not cover these, so this is not an exposure defect (gate={gate} boot_view_stop_reason={stop_reason} frames_so_far={n})"
        ));
    }
}

pub(crate) fn native_ls_gate_name(gate: usize) -> &'static str {
    use er_telemetry_core::counters::{
        NATIVE_LS_GATE_COVER_STOPPED, NATIVE_LS_GATE_DREW, NATIVE_LS_GATE_EPOCH_WORLD_LIVE,
        NATIVE_LS_GATE_NATIVE_SUPPRESSED, NATIVE_LS_GATE_OVERLAY_DISABLED,
        NATIVE_LS_GATE_UNOWNED_LOAD,
    };
    match gate {
        NATIVE_LS_GATE_DREW => "drew",
        NATIVE_LS_GATE_OVERLAY_DISABLED => "overlay-disabled",
        NATIVE_LS_GATE_EPOCH_WORLD_LIVE => "epoch-world-live-skip",
        NATIVE_LS_GATE_NATIVE_SUPPRESSED => "native-windows-suppressed",
        NATIVE_LS_GATE_COVER_STOPPED => "cover-stopped-or-nothing-to-draw",
        NATIVE_LS_GATE_UNOWNED_LOAD => "not-ours-game-owned-loading-screen",
        _ => "unknown",
    }
}

/// Emit the exposure oracles. Called from the telemetry writer.
pub(crate) fn native_ls_exposure_write(body: &mut String) {
    use er_telemetry_core::counters::{
        NATIVE_LS_COVERED_FRAMES, NATIVE_LS_EXPOSURE_BY_GATE, NATIVE_LS_EXPOSURE_FIRST_MS,
        NATIVE_LS_EXPOSURE_FRAMES, NATIVE_LS_EXPOSURE_LAST_GATE, NATIVE_LS_EXPOSURE_LAST_MS,
        NATIVE_LS_EXPOSURE_LAST_STOP_REASON, NATIVE_LS_EXPOSURE_MAX_RUN, NATIVE_LS_GATE_COUNT,
    };
    push_json_usize(
        body,
        "oracle_native_ls_exposure_frames",
        NATIVE_LS_EXPOSURE_FRAMES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_native_ls_exposure_owned_frames",
        er_telemetry_core::counters::NATIVE_LS_EXPOSURE_OWNED_FRAMES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_native_ls_exposure_owned_first_ms",
        er_telemetry_core::counters::NATIVE_LS_EXPOSURE_OWNED_FIRST_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_native_ls_covered_frames",
        NATIVE_LS_COVERED_FRAMES.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_native_ls_exposure_max_run",
        NATIVE_LS_EXPOSURE_MAX_RUN.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_native_ls_exposure_first_ms",
        NATIVE_LS_EXPOSURE_FIRST_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_native_ls_exposure_last_ms",
        NATIVE_LS_EXPOSURE_LAST_MS.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_native_ls_exposure_last_gate",
        NATIVE_LS_EXPOSURE_LAST_GATE.load(Ordering::SeqCst),
    );
    push_json_usize(
        body,
        "oracle_native_ls_exposure_last_stop_reason",
        NATIVE_LS_EXPOSURE_LAST_STOP_REASON.load(Ordering::SeqCst),
    );
    let gates = (0..NATIVE_LS_GATE_COUNT)
        .map(|g| NATIVE_LS_EXPOSURE_BY_GATE[g].load(Ordering::SeqCst).to_string())
        .collect::<Vec<_>>()
        .join(",");
    body.push_str(&format!(
        "  \"oracle_native_ls_exposure_by_gate\": [{gates}],\n"
    ));
}
