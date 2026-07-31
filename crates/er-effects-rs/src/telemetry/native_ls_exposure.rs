// NATIVE LOADING-SCREEN EXPOSURE SEMAPHORE (er-effects-rs-wmw defect #1, user report 2026-07-30:
// "the custom loading screen disappeared for about one frame and the vanilla loading screen flashed
// through").
//
// The defect is a PER-FRAME event, so it needs a per-frame oracle: the aggregate counters we already
// had (`oracle_boot_view_stop_reason`, `oracle_portrait_onto_draw_hits`) can say the cover stopped,
// but never that a frame REACHED THE SCREEN with the game's own loading screen visible. This module
// closes that gap. On every Present the detour reports which gate decided this frame's cover; when
// the game's `CS::LoadingScreen` is live and the cover did NOT draw, the frame is latched as an
// EXPOSURE frame -- literally the frame the user saw vanilla -- and attributed to the blocking gate.
//
// Telemetry-only: nothing here changes what is drawn. It converts a visual report into a RAM oracle
// so the flash-through can be reproduced and attributed without reading a screenshot.
//
// NOT the same thing as `BOOT_VIEW_NATIVE_EXPOSURE_FRAMES` / `oracle_boot_view_native_exposure_*`
// (branch loading-portrait-semaphores-20260730). That one lives in the cover state machine and
// counts times the cover was RESUMED because the native loading screen reappeared -- a mitigation,
// measured at the decision layer, which cannot say whether a frame actually reached the screen. This
// one is measured at Present, the last point before the user's eyes, and counts the frames
// themselves. Whether that resume mitigation works is exactly a question these counters can answer.
//
// Reading the oracles:
//   oracle_native_ls_exposure_frames   > 0  the defect occurred; the user COULD see vanilla
//   oracle_native_ls_exposure_max_run  == 1 a one-frame flash (the exact user report)
//                                      >> 1 a sustained hole (e.g. the FPS bail killing the cover)
//   oracle_native_ls_exposure_by_gate  which gate blocked the cover, by `NATIVE_LS_GATE_*` code
//   oracle_native_ls_exposure_last_stop_reason  BOOT_VIEW_STOP_REASON at the last exposure frame
//                                      (1 = release fade, 2 = FPS bail, 3 = world handoff)
//
// A frame is only judged while the native loading screen is LIVE. `LOADING_SCREEN_UPDATE_LAST_MS` is
// stamped by the native CS::LoadingScreen::Update hook every frame the screen ticks, so freshness is
// the live signal. The window is deliberately wider than one frame (the game presents as slowly as
// ~5 fps during loading, bd FPS-DELTA-CONFIRMED-load2-20fps-load1-45fps) so a slow frame is not
// misread as the screen having closed; it is still far below the 1500 ms window-close quiet period
// used by [`portrait_loadwin_tick`].
const NATIVE_LS_LIVE_FRESH_MS: u64 = 250;

/// Called once per Present on the game swapchain with the gate that decided this frame's cover.
/// Cheap by construction (a handful of relaxed atomics) -- it runs on the render thread.
pub(crate) fn native_ls_exposure_record(gate: usize) {
    use er_telemetry::counters::{
        BOOT_VIEW_STOP_REASON, LOADING_SCREEN_UPDATE_LAST_MS, NATIVE_LS_COVERED_FRAMES,
        NATIVE_LS_EXPOSURE_BY_GATE, NATIVE_LS_EXPOSURE_CUR_RUN, NATIVE_LS_EXPOSURE_FIRST_MS,
        NATIVE_LS_EXPOSURE_FRAMES, NATIVE_LS_EXPOSURE_LAST_GATE, NATIVE_LS_EXPOSURE_LAST_MS,
        NATIVE_LS_EXPOSURE_LAST_STOP_REASON, NATIVE_LS_EXPOSURE_MAX_RUN, NATIVE_LS_GATE_COUNT,
        NATIVE_LS_GATE_DREW,
    };
    let update_last = LOADING_SCREEN_UPDATE_LAST_MS.load(Ordering::SeqCst) as u64;
    if update_last == 0 {
        return;
    }
    let now_ms = crate::experiments::boot_view_epoch_ms().max(1);
    if now_ms.saturating_sub(update_last) > NATIVE_LS_LIVE_FRESH_MS {
        // The game's loading screen is not on screen this frame; nothing to be exposed.
        return;
    }
    if gate == NATIVE_LS_GATE_DREW {
        NATIVE_LS_COVERED_FRAMES.fetch_add(1, Ordering::SeqCst);
        NATIVE_LS_EXPOSURE_CUR_RUN.store(0, Ordering::SeqCst);
        return;
    }
    let n = NATIVE_LS_EXPOSURE_FRAMES.fetch_add(1, Ordering::SeqCst) + 1;
    let run = NATIVE_LS_EXPOSURE_CUR_RUN.fetch_add(1, Ordering::SeqCst) + 1;
    NATIVE_LS_EXPOSURE_MAX_RUN.fetch_max(run, Ordering::SeqCst);
    NATIVE_LS_EXPOSURE_LAST_GATE.store(gate, Ordering::SeqCst);
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
    if run == 1 {
        append_autoload_debug(format_args!(
            "NATIVE-LS EXPOSURE #{n}: the vanilla loading screen reached the screen at {now_ms}ms -- cover blocked by gate={gate} ({}) boot_view_stop_reason={stop_reason}",
            native_ls_gate_name(gate)
        ));
    }
}

pub(crate) fn native_ls_gate_name(gate: usize) -> &'static str {
    use er_telemetry::counters::{
        NATIVE_LS_GATE_COVER_STOPPED, NATIVE_LS_GATE_DREW, NATIVE_LS_GATE_EPOCH_WORLD_LIVE,
        NATIVE_LS_GATE_NATIVE_SUPPRESSED, NATIVE_LS_GATE_OVERLAY_DISABLED,
    };
    match gate {
        NATIVE_LS_GATE_DREW => "drew",
        NATIVE_LS_GATE_OVERLAY_DISABLED => "overlay-disabled",
        NATIVE_LS_GATE_EPOCH_WORLD_LIVE => "epoch-world-live-skip",
        NATIVE_LS_GATE_NATIVE_SUPPRESSED => "native-windows-suppressed",
        NATIVE_LS_GATE_COVER_STOPPED => "cover-stopped-or-nothing-to-draw",
        _ => "unknown",
    }
}

/// Emit the exposure oracles. Called from the telemetry writer.
pub(crate) fn native_ls_exposure_write(body: &mut String) {
    use er_telemetry::counters::{
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
