//! Can-move readiness probe (2026-07-18, user-directed): PROVE that input actually moves the
//! character, not just that it is render-ready. "render-ready" says the character can be SEEN;
//! this says input MOVES it. `play_time` advancing is necessary but not sufficient (it ticks during
//! the freeze), so movement is proven by a havok-POSITION delta under a KNOWN injected forward stick,
//! sustained for `MOVE_PROBE_REQUIRED_FRAMES` (60) consecutive frames per load -- a real walk, not a
//! one-frame twitch. Runs on the game thread (safe to drive input); the XInput hook stamps the stick
//! when `MOVE_PROBE_ACTIVE`.
//!
//! Per load epoch (fresh_deser_count) the probe resets, then each render-ready frame it injects the
//! forward stick and counts consecutive frames whose horizontal displacement clears the threshold.
//! A static/frozen character repeats its position exactly (delta ~0), so it never accumulates; a
//! walking character clears 60 frames quickly and latches `CAN_MOVE_CONFIRMED`.

use std::sync::Mutex;
use std::sync::atomic::Ordering;

use crate::constants::{
    CAN_MOVE_CONFIRMED, MOVE_PROBE_ACTIVE, MOVE_PROBE_EPOCH, MOVE_PROBE_MOVED_FRAMES,
    MOVE_PROBE_PER_FRAME_THRESHOLD, MOVE_PROBE_REQUIRED_FRAMES, MOVE_PROBE_STICK_FORWARD,
    MOVE_PROBE_STICK_LY, SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT,
};
use crate::telemetry::append_autoload_debug;

/// Previous frame's world position while a probe is active (game thread only touches this).
static PREV_POS: Mutex<Option<(f32, f32, f32)>> = Mutex::new(None);

fn lock_prev() -> std::sync::MutexGuard<'static, Option<(f32, f32, f32)>> {
    PREV_POS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Drive one frame of the can-move probe. `render_ready` is the exact oracle combination
/// (chr-model present AND draw-group AND render-group AND enable-render); `pos` is the local
/// player's havok position this frame.
pub(crate) fn tick(render_ready: bool, pos: (f32, f32, f32)) {
    let epoch = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
    // New load epoch -> reset the probe (each load must re-prove movement on its own).
    if MOVE_PROBE_EPOCH.swap(epoch, Ordering::SeqCst) != epoch {
        CAN_MOVE_CONFIRMED.store(false, Ordering::SeqCst);
        MOVE_PROBE_MOVED_FRAMES.store(0, Ordering::SeqCst);
        MOVE_PROBE_ACTIVE.store(false, Ordering::SeqCst);
        MOVE_PROBE_STICK_LY.store(0, Ordering::SeqCst);
        *lock_prev() = None;
    }

    // Already proven for this load -> stop injecting; let the user/world be.
    if CAN_MOVE_CONFIRMED.load(Ordering::SeqCst) {
        MOVE_PROBE_ACTIVE.store(false, Ordering::SeqCst);
        MOVE_PROBE_STICK_LY.store(0, Ordering::SeqCst);
        return;
    }

    // Not render-ready -> cannot be moving; hold the probe idle and reset the consecutive counter
    // (a frozen load must never accumulate movement frames).
    if !render_ready {
        MOVE_PROBE_ACTIVE.store(false, Ordering::SeqCst);
        MOVE_PROBE_STICK_LY.store(0, Ordering::SeqCst);
        MOVE_PROBE_MOVED_FRAMES.store(0, Ordering::SeqCst);
        *lock_prev() = None;
        return;
    }

    // Render-ready: inject a forward stick and measure this frame's horizontal displacement.
    MOVE_PROBE_STICK_LY.store(MOVE_PROBE_STICK_FORWARD, Ordering::SeqCst);
    MOVE_PROBE_ACTIVE.store(true, Ordering::SeqCst);
    let mut prev = lock_prev();
    if let Some((px, _py, pz)) = *prev {
        let dx = pos.0 - px;
        let dz = pos.2 - pz;
        let horiz = (dx * dx + dz * dz).sqrt();
        if horiz >= MOVE_PROBE_PER_FRAME_THRESHOLD {
            let moved = MOVE_PROBE_MOVED_FRAMES.fetch_add(1, Ordering::SeqCst) + 1;
            if moved >= MOVE_PROBE_REQUIRED_FRAMES {
                CAN_MOVE_CONFIRMED.store(true, Ordering::SeqCst);
                MOVE_PROBE_ACTIVE.store(false, Ordering::SeqCst);
                MOVE_PROBE_STICK_LY.store(0, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "can-move: PROVEN for load epoch {epoch} -- {moved} consecutive frames of injected-forward movement (last frame horiz={horiz:.4})"
                ));
            }
        } else {
            // Not moving this frame -> the run must be CONSECUTIVE, so reset.
            MOVE_PROBE_MOVED_FRAMES.store(0, Ordering::SeqCst);
        }
    }
    *prev = Some(pos);
}
