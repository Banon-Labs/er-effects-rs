//! Is the game's own loading screen still MAKING PROGRESS, or is it frozen?
//!
//! This crate already samples the native `CS::LoadingScreen` every frame it ticks
//! ([`crate::dlstring_lookat_math::sample_loading_screen_bar`], which reads the Gauge_3 movieclip's
//! current/max frame straight out of the menu component). This module is the one question the
//! cover's backstop needs to ask of those samples, kept next to the code that produces them and
//! written as a pure function so it can be tested on the host rather than only in the game.
//!
//! # Why this exists (run br-20260831-160354-2513)
//!
//! The user drove one System->Quit->Load Character reload. It ended in
//! `boot-view: FPS BAIL stop (... composite_ms=20015 ...)` at boot-view 239858 ms -- the own-menu
//! epoch's 20 s composite cap. The native loading screen sent its own finish 501 ms later:
//! `loading-bar: native LoadingScreen finish/result sent (hits=2, frame=500/500, ..., now_ms=240359)`.
//! So a wall-clock budget pre-empted, by half a second, a release that was already arriving, and
//! filed the window as `oracle_boot_view_stop_reason=2` with the log text "handoff signals never
//! fired (frozen load2)" -- on a window whose handoff had fired 15 s earlier
//! (`boot-view: COVER RELEASE #2 at 224208ms`).
//!
//! ## The load was not frozen, and the gauge says so directly
//!
//! Across exactly the stretch the cover was accused of over-holding, the game's own gauge advanced
//! monotonically -- `frame=19/500` at +222605 ms, `72/500` at +227459 ms, `435/500` at +238084 ms,
//! terminal `500/500` at +241377 ms. A frozen load2, which is the failure the cap was added to
//! survive, cannot do that: its gauge stops. So "frozen" and "slow" are separable from RAM alone,
//! and the cap was not separating them -- it was reading a clock.
//!
//! This is the repo's own doctrine applied one level down. AGENTS.md, on runtime teardown:
//! "Distinguish a real hang (the `oracle_system_step_label` / loading substep FROZEN) from slow
//! progress (label still advancing); tear down only on a genuinely frozen substep", and "the time
//! bound is a safety backstop, NOT the primary synchronization mechanism".
//!
//! ## What this deliberately does NOT do
//!
//! It does not touch `BOOT_VIEW_NATIVE_LOADING_QUIET_HOLD_MS` (900 ms), the gate that actually held
//! the fade. That gate was correct: `CS::LoadingScreen::Update` really was ticking every frame,
//! because the screen really was still loading, and fading out over it is the vanilla flash-through
//! (er-effects-rs-wmw defect #1). The same run also PROVES the 900 ms is reachable -- the boot
//! window cleared it (`update_quiet` 2 ms -> 287 ms, then `native loading fade/quiet hold complete`
//! at +34384 ms and `release fade complete -> stop cover (... reason=1)`). Lowering it would have
//! released the cover onto a loading screen reading 39 %.
//!
//! And it does not extend a frozen window by one millisecond: with the gauge stuck,
//! [`CapDecision::bar_stalled`] is true and the cap fires exactly when it always did.

/// The native Gauge_3 frame must be FROZEN this long before an expired composite cap may fire.
///
/// Measured cadence inside the held window of that run: the boot-view hold lines report
/// `native_hits` 105 (+225527 ms) -> 232 (+236005 ms), i.e. ~12 native update ticks per second, so
/// 2 s is ~24 consecutive ticks with no frame movement. It is also 2x `LOADWIN_QUIET_CLOSE_MS`
/// (1500 ms in [`crate::portrait_load_windows`]), the threshold this repo already uses to declare a
/// native loading window closed -- so a gauge quiet for longer than that is stalled by the
/// codebase's existing definition rather than by a fresh number invented here.
pub const NATIVE_BAR_STALL_MS: u64 = 2_000;

/// Hard bound on how long the cap may be deferred, so a pathological screen that keeps nudging its
/// gauge forever cannot resurrect the unbounded cover the cap exists to prevent.
///
/// The measured shortfall was 501 ms, so this is 30x the gap it has to close. Worst case an
/// own-menu window runs cap + this = 35 s, still well inside `BOOT_VIEW_BACKSTOP_LIFETIME_MS`
/// (120 s), which remains the true ceiling for every epoch.
pub const COMPOSITE_CAP_MAX_DEFER_MS: u64 = 15_000;

/// Permille at which the native bar counts as "essentially complete".
///
/// 998 rather than 1000 because the gauge reports 998/999 for a frame or two before it settles --
/// measured in run br-20260831-160354-2513's boot window, `progress=998` then `999` then `1000`
/// across four consecutive Gauge_3 samples.
pub const NATIVE_BAR_TERMINAL_PERMILLE: usize = 998;

/// Has the native Gauge_3 movieclip finished, i.e. is its playhead at or past its last frame?
///
/// `max_frame == 0` means there is no gauge on this screen at all. That counts as done: a screen
/// with no progress to report cannot be waited on, and refusing here would leave a gauge-less load
/// path with no way to satisfy the release at all.
///
/// THE SINGLE DEFINITION. Both callers that need it -- the ENTERING WORLD phase predicate and the
/// cover's release predicate -- go through this function, so the two can never drift apart again.
/// They drifted once already, which is the whole of bd er-effects-rs-t7q2.
pub fn gauge_done(cur_frame: usize, max_frame: usize) -> bool {
    max_frame == 0 || cur_frame >= max_frame
}

/// Is the gauge at its terminal frame -- COMPLETED rather than merely absent?
///
/// Stricter than [`gauge_done`] on purpose: this one is asked about a gauge that has STOPPED
/// MOVING, where "there is no gauge" and "the gauge finished" must not be conflated. A screen with
/// no gauge that stops moving tells us nothing and must stay subject to the composite cap.
pub fn gauge_terminal(cur_frame: usize, max_frame: usize) -> bool {
    max_frame != 0 && cur_frame >= max_frame
}

/// COMPLETED native loading-screen plates a switch's cover must see before it may let go.
///
/// One: the character load's own plate. See `BOOT_VIEW_SWITCH_COMPLETED_NATIVE_SCREENS` for the
/// measurement that replaced the old ordinal `2`.
pub const SWITCH_COMPLETED_PLATES_TO_RELEASE: usize = 1;

/// Fold one native loading-screen finish into this cover window's completed-plate count.
///
/// A finish whose gauge is at its terminal frame is a WORLD HAVING LOADED; a finish at frame 1 of
/// 500 is a screen going away (the switch's return-to-title teardown). `gauge_terminal` rather than
/// `gauge_done` on purpose: a plate with no gauge at all reports nothing, and counting it would let
/// the cover release on a screen that never showed the user any progress.
pub fn count_completed_close(completed_so_far: usize, cur_frame: usize, max_frame: usize) -> usize {
    completed_so_far + usize::from(gauge_terminal(cur_frame, max_frame))
}

/// May the switch cover release yet, given the completed plates counted so far this window?
pub fn switch_cover_may_release(completed_closes: usize) -> bool {
    completed_closes >= SWITCH_COMPLETED_PLATES_TO_RELEASE
}

/// Has the game's own loading screen FINISHED -- the release predicate's "native done" half.
///
/// # Why the `gauge_done` conjunct is not optional (bd er-effects-rs-t7q2)
///
/// The predicate used to read `close_hits != 0 || permille >= 998`, and the bare `close_hits`
/// arm is wrong for the same reason it is wrong one phase earlier: a reload's TRANSIENT loading
/// screen sends its own finish/result while its gauge is still at frame 1 of 500 -- a screen
/// closing, not the world handing off. Measured in run br-20260831-160354-2513: the reload epoch's
/// close #1 arrived at boot-view 217892 ms reporting `frame=1/500`, and the cover latched its
/// release from it at 224208 ms -- 17 s before the gauge reached 500/500 at 240059 ms.
///
/// That is not merely 17 s of early release. `boot_view_absolute_backstop` refuses to fire while
/// the release is reachable, so a premature latch DISARMS the cover's 120 s lifetime bound, and on
/// the boot epoch -- which has no composite cap at all -- that leaves the window with no bound of
/// any kind. The same "no reachable exit" failure as er-effects-rs-55y6, entered from the far side.
///
/// The permille arm keeps its own independent path to true and is left alone: it is a reading of
/// the gauge's own fill, not of an unrelated screen's teardown, and it is what released the healthy
/// BOOT window of that same run (at 30050 ms, before that window's close at 30592 ms).
pub fn release_native_done(
    close_hits: usize,
    permille: usize,
    cur_frame: usize,
    max_frame: usize,
) -> bool {
    permille >= NATIVE_BAR_TERMINAL_PERMILLE
        || (close_hits != 0 && gauge_done(cur_frame, max_frame))
}

/// The cap's answer for one frame, with the numbers that justify it so the caller's log line can
/// state WHICH arm fired instead of asserting a cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapDecision {
    /// Stop the cover now.
    pub fire: bool,
    /// The native gauge has not moved for at least [`NATIVE_BAR_STALL_MS`] (or has never moved).
    /// This is the genuine "frozen load2" the cap was written for.
    pub bar_stalled: bool,
    /// How long the cap has already been deferred past its nominal expiry.
    pub deferred_ms: u64,
    /// How long since the gauge last moved.
    pub bar_stall_ms: u64,
    /// The gauge is sitting at its LAST frame. A stopped gauge at 500/500 has finished; a stopped
    /// gauge at 3/500 is the frozen load this cap exists for. Without this the two are the same
    /// observation and the cap cannot tell them apart.
    pub bar_terminal: bool,
    /// The gauge frame this decision saw. Filled by [`observe_and_decide`]; 0 from the pure form.
    pub bar_frame: usize,
}

/// Decide whether an expired composite cap may fire this frame.
///
/// `last_progress_ms` is the epoch-ms at which the native gauge frame last CHANGED, or 0 if it has
/// not changed once in this window -- which is a stall by any reading and never a reason to defer.
pub fn composite_cap_decision(
    composite_ms: u64,
    cap_ms: u64,
    now_ms: u64,
    last_progress_ms: u64,
    bar_terminal: bool,
) -> CapDecision {
    let bar_stall_ms = if last_progress_ms == 0 {
        u64::MAX
    } else {
        now_ms.saturating_sub(last_progress_ms)
    };
    // A gauge parked on its LAST frame is not stalled, it is DONE, however long it sits there.
    // Measured, run br-20260831-160354-2513's healthy BOOT window: the gauge made its final frame
    // change at boot-view 30124 ms and the release fade did not complete until 33747 ms -- 3623 ms
    // later, because `CS::LoadingScreen::Update` keeps ticking ~1.6 s past the close and the fade
    // then waits out `BOOT_VIEW_NATIVE_LOADING_QUIET_HOLD_MS` (900 ms) before its own 640 ms ramp.
    // Judging that tail by `NATIVE_BAR_STALL_MS` alone would call a FINISHED load frozen 1.6 s
    // before its release could possibly land, which is the bail this module was written to prevent,
    // reappearing two seconds later.
    let bar_stalled = !bar_terminal && bar_stall_ms >= NATIVE_BAR_STALL_MS;
    let deferred_ms = composite_ms.saturating_sub(cap_ms);
    // Defer ONLY while the game's own gauge is demonstrably still moving OR has demonstrably
    // finished, and only for a bounded total. Everything else -- expiry, freeze, the deferral
    // bound -- fires.
    let defer = !bar_stalled && deferred_ms < COMPOSITE_CAP_MAX_DEFER_MS;
    CapDecision {
        fire: composite_ms >= cap_ms && !defer,
        bar_stalled,
        deferred_ms,
        bar_stall_ms: if last_progress_ms == 0 {
            0
        } else {
            bar_stall_ms
        },
        bar_terminal,
        bar_frame: 0,
    }
}

/// Sample the live native gauge and get the cap's answer, in one call. This is the seam the DLL
/// uses; the pure [`composite_cap_decision`] beneath it is what the tests drive.
///
/// Reads `LOADING_SCREEN_BAR_CURRENT_FRAME` itself rather than taking it as an argument: this crate
/// already owns the detour that WRITES that counter
/// ([`crate::dlstring_lookat_math::sample_loading_screen_bar`]), so the DLL has no business
/// restating where the number comes from. Read, never written -- pure RAM observation.
/// `epoch` is the caller's load epoch; a change resets the per-window gauge state, so one window's
/// progress can never vouch for the next one's. Returns `Some` ONLY when the cap should fire, so a
/// caller cannot accidentally read a decision it was not given.
#[cfg(windows)]
pub fn cap_fired(epoch: usize, now_ms: u64, composite_ms: u64, cap_ms: u64) -> Option<CapDecision> {
    use core::sync::atomic::Ordering;
    if tracker::EPOCH.swap(epoch, Ordering::SeqCst) != epoch {
        tracker::reset();
    }
    let bar_frame =
        er_telemetry_core::counters::LOADING_SCREEN_BAR_CURRENT_FRAME.load(Ordering::SeqCst);
    let bar_max = er_telemetry_core::counters::LOADING_SCREEN_BAR_MAX_FRAME.load(Ordering::SeqCst);
    tracker::note_frame(now_ms, bar_frame);
    let mut d = composite_cap_decision(
        composite_ms,
        cap_ms,
        now_ms,
        tracker::progress_ms(),
        gauge_terminal(bar_frame, bar_max),
    );
    d.bar_frame = bar_frame;
    d.fire.then_some(d)
}

/// The whole diagnostic tail of the bail's log line, built where the decision is made.
///
/// It lives here so the wording can never again outrun the measurement. The old line asserted
/// "handoff signals never fired (frozen load2)" unconditionally, and run br-20260831-160354-2513
/// printed exactly that on a window whose handoff HAD fired 15 s earlier
/// (`boot-view: COVER RELEASE #2 at 224208ms`). Now the text is a function of the data.
pub fn bail_detail(d: &CapDecision) -> String {
    format!(
        "deferred_ms={} native_bar_frame={} bar_stall_ms={} -- the native loading gauge is {}",
        d.deferred_ms,
        d.bar_frame,
        d.bar_stall_ms,
        if d.bar_stalled {
            "FROZEN (no Gauge_3 movement, short of its last frame), the frozen-load2 case this bail exists for"
        } else if d.bar_terminal {
            "FINISHED (parked on its last frame), but the deferral bound ran out before the release fade landed"
        } else {
            "still advancing, but the deferral bound ran out"
        }
    )
}

/// Per-window tracker for the gauge frame, so the caller does not have to hold the state itself.
///
/// Deliberately window-scoped: one window's progress must never vouch for the next one's, which is
/// the same trap the composite clock itself already guards against (a switch inheriting the previous
/// window's first-composite timestamp instantly tripped the 20 s cap -- measured run
/// samechar-3x-threedll-20260729-203842, bail at `cover_window_ms=36`).
pub mod tracker {
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// `usize::MAX` cannot equal a real gauge frame, so the first sample after a reset always counts
    /// as a change and stamps a fresh progress timestamp.
    static LAST_FRAME: AtomicUsize = AtomicUsize::new(usize::MAX);
    static PROGRESS_MS: AtomicUsize = AtomicUsize::new(0);
    /// Load epoch this window's state belongs to. `usize::MAX` never equals a real epoch, so the
    /// first call always resets. Windows-only, like its sole reader [`super::cap_fired`] -- the
    /// pure decision the host tests drive takes the progress timestamp as an argument instead.
    #[cfg(windows)]
    pub(super) static EPOCH: AtomicUsize = AtomicUsize::new(usize::MAX);

    /// Start a new cover window. Call from the same place the composite clock is re-stamped.
    pub fn reset() {
        LAST_FRAME.store(usize::MAX, Ordering::SeqCst);
        PROGRESS_MS.store(0, Ordering::SeqCst);
    }

    /// Feed this frame's native gauge frame. Records the time only when it actually CHANGED.
    ///
    /// The FIRST sample after a reset establishes the baseline and is NOT progress. It has to work
    /// this way for the freeze case to stay reachable at all: with the first sample stamping,
    /// `progress_ms()` was non-zero from the very first call, so "the gauge has not moved once in
    /// this window" -- the state [`super::composite_cap_decision`] answers with an immediate fire,
    /// and the state its `a_gauge_that_never_moved_fires_immediately` test drives -- could never
    /// occur in the DLL, and a completely dead gauge silently bought 2 s of deferral it had not
    /// earned. Observing a value once is not evidence that it is changing.
    pub fn note_frame(now_ms: u64, bar_frame: usize) {
        let prev = LAST_FRAME.swap(bar_frame, Ordering::SeqCst);
        if prev != usize::MAX && prev != bar_frame {
            PROGRESS_MS.store(now_ms as usize, Ordering::SeqCst);
        }
    }

    /// Epoch-ms the gauge last moved; 0 if it has not moved in this window.
    pub fn progress_ms() -> u64 {
        PROGRESS_MS.load(Ordering::SeqCst) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAP: u64 = 20_000;

    /// Below the cap nothing fires, however the gauge behaves.
    #[test]
    fn under_cap_never_fires() {
        assert!(!composite_cap_decision(19_999, CAP, 100_000, 100_000, false).fire);
        assert!(!composite_cap_decision(19_999, CAP, 100_000, 0, false).fire);
        assert!(!composite_cap_decision(19_999, CAP, 100_000, 0, true).fire);
    }

    /// THE REGRESSION THIS MODULE EXISTS FOR. At the moment of the real bail the gauge had moved
    /// within the last frame, so the cap must defer and let the native finish 501 ms later win.
    #[test]
    fn defers_while_the_native_gauge_is_still_advancing() {
        let d = composite_cap_decision(20_015, CAP, 239_858, 239_800, false);
        assert!(
            !d.fire,
            "a still-advancing loading screen must not be cut off"
        );
        assert!(!d.bar_stalled);
    }

    /// THE SECOND HALF OF THAT REGRESSION, measured after the first fix was written.
    ///
    /// Deferring only while the gauge MOVES is not enough, because the release lands well after the
    /// gauge stops. Run br-20260831-160354-2513's healthy BOOT window: last gauge frame change at
    /// boot-view 30124 ms, release fade complete at 33747 ms -- a 3623 ms tail, because
    /// `CS::LoadingScreen::Update` keeps ticking ~1.6 s past the close and the fade then waits out
    /// the 900 ms quiet hold before its 640 ms ramp. Judged by `NATIVE_BAR_STALL_MS` (2000 ms)
    /// alone, the reload epoch would have bailed at 242124 ms -- ~1.4 s BEFORE its release could
    /// land -- and filed reason=2 all over again.
    #[test]
    fn a_terminal_gauge_is_finished_not_frozen() {
        // 3 s with no movement, i.e. past NATIVE_BAR_STALL_MS, on a gauge parked at its last frame.
        let d = composite_cap_decision(23_000, CAP, 243_100, 240_100, true);
        assert!(
            !d.fire,
            "a gauge parked on its final frame has FINISHED; the release tail must be allowed to land"
        );
        assert!(!d.bar_stalled);
        assert!(d.bar_terminal);
        // Same numbers, gauge short of its last frame: that IS the frozen load2 and it fires.
        let d = composite_cap_decision(23_000, CAP, 243_100, 240_100, false);
        assert!(d.fire);
        assert!(d.bar_stalled);
    }

    /// The frozen-load2 case the cap was written for: the gauge stops SHORT of its last frame, so
    /// the cap fires once the stall window passes, exactly as before this module existed.
    #[test]
    fn fires_once_a_mid_load_gauge_stops_moving() {
        // Gauge last moved at 240_358, still inside the stall window: hold.
        assert!(!composite_cap_decision(21_000, CAP, 241_000, 240_358, false).fire);
        // Past NATIVE_BAR_STALL_MS with no movement: fire, and name it a freeze.
        let d = composite_cap_decision(22_500, CAP, 242_358, 240_358, false);
        assert!(d.fire);
        assert!(d.bar_stalled);
    }

    /// A gauge that never moved at all fires the instant the cap expires. Reachable in the DLL only
    /// because `tracker::note_frame` treats the first post-reset sample as a baseline rather than
    /// as progress -- see `tracker_first_sample_is_a_baseline_not_progress`.
    #[test]
    fn a_gauge_that_never_moved_fires_immediately() {
        let d = composite_cap_decision(20_000, CAP, 100_000, 0, false);
        assert!(d.fire);
        assert!(d.bar_stalled);
        assert_eq!(
            d.bar_stall_ms, 0,
            "no movement means no meaningful stall clock"
        );
    }

    /// A screen that keeps nudging its gauge forever still cannot hold the cover open past the
    /// deferral bound...
    #[test]
    fn deferral_is_bounded() {
        let now = 300_000;
        // Gauge moved this very frame, but the cap has already been deferred to the bound.
        let d = composite_cap_decision(CAP + COMPOSITE_CAP_MAX_DEFER_MS, CAP, now, now, false);
        assert!(d.fire);
        assert!(!d.bar_stalled, "it fired on the bound, not on a freeze");
        assert_eq!(d.deferred_ms, COMPOSITE_CAP_MAX_DEFER_MS);
    }

    /// ...and neither can a terminal one. The finished-gauge arm must not become a new way to hold
    /// the cover open forever, so it rides the SAME bound.
    #[test]
    fn a_terminal_gauge_cannot_defer_past_the_bound() {
        let now = 300_000;
        let d = composite_cap_decision(
            CAP + COMPOSITE_CAP_MAX_DEFER_MS,
            CAP,
            now,
            now - 60_000,
            true,
        );
        assert!(d.fire, "worst case is cap + bound = 35 s, never unbounded");
        assert!(d.bar_terminal);
        assert!(!d.bar_stalled);
    }

    #[test]
    fn tracker_stamps_only_on_change() {
        tracker::reset();
        assert_eq!(tracker::progress_ms(), 0);
        tracker::note_frame(900, 5); // baseline
        tracker::note_frame(1_000, 6); // moved
        assert_eq!(tracker::progress_ms(), 1_000);
        tracker::note_frame(1_100, 6); // same frame: not progress
        assert_eq!(tracker::progress_ms(), 1_000);
        tracker::note_frame(1_200, 7); // moved
        assert_eq!(tracker::progress_ms(), 1_200);
        tracker::reset();
        assert_eq!(tracker::progress_ms(), 0);
    }

    /// The first sample after a reset is a BASELINE, not progress. Without this a dead gauge looked
    /// like it had just moved, `progress_ms()` was never 0 in the DLL, and the frozen-load2 arm was
    /// unreachable in the very code path it exists to protect.
    #[test]
    fn tracker_first_sample_is_a_baseline_not_progress() {
        tracker::reset();
        tracker::note_frame(50_000, 3);
        assert_eq!(
            tracker::progress_ms(),
            0,
            "observing a frame once is not evidence it is changing"
        );
        // ...so the cap sees a gauge that has never moved and fires the moment it expires.
        assert!(composite_cap_decision(CAP, CAP, 70_000, tracker::progress_ms(), false).fire);
        tracker::reset();
    }

    // ---- er-effects-rs-t7q2: the release predicate's native-done half ----

    /// THE MEASURED DEFECT. Reload epoch of run br-20260831-160354-2513: close #1 arrives with the
    /// gauge at frame 1 of 500 and permille well under 998. That is a transient screen closing, not
    /// the world handing off, and it must NOT latch the cover's release.
    #[test]
    fn a_close_on_a_frame_1_of_500_gauge_is_not_native_done() {
        assert!(
            !release_native_done(1, 40, 1, 500),
            "close #1 at frame=1/500 latched the release 17 s early (er-effects-rs-t7q2)"
        );
        // The same close once the gauge has actually finished IS the handoff.
        assert!(release_native_done(2, 996, 500, 500));
    }

    /// The permille arm keeps its independent path to true: it released the HEALTHY boot window of
    /// that same run at 30050 ms, before that window's close at 30592 ms, so removing it would
    /// regress a proven-good release.
    #[test]
    fn the_permille_arm_still_stands_alone() {
        assert!(release_native_done(
            0,
            NATIVE_BAR_TERMINAL_PERMILLE,
            499,
            500
        ));
        assert!(!release_native_done(0, 997, 499, 500));
    }

    /// A screen with no gauge at all cannot be waited on: `max_frame == 0` must not make the
    /// release unsatisfiable, or the fix would trade a 17 s early release for a stuck cover.
    #[test]
    fn a_gauge_less_screen_can_still_finish() {
        assert!(gauge_done(0, 0));
        assert!(release_native_done(1, 0, 0, 0));
        // But "no gauge" is NOT "gauge finished" when the question is whether it froze.
        assert!(!gauge_terminal(0, 0));
        assert!(gauge_terminal(500, 500));
        assert!(!gauge_terminal(499, 500));
    }

    // ---- the switch cover's release gate: COMPLETED plates, not an ordinal (2026-09-06) ----

    /// Replays the two-plate switch the gate was originally written from
    /// (br-20260905-235624-a149): the unload finishes at `frame=1/500`, the character load's plate
    /// finishes at `frame=500/500`. Counting completed plates must open the gate at the SAME
    /// moment the old `close_hits >= 2` did -- on the second finish, not the first.
    #[test]
    fn a_two_plate_switch_releases_on_the_load_plate_not_the_teardown() {
        let mut completed = 0;
        completed = count_completed_close(completed, 1, 500); // the unload
        assert!(
            !switch_cover_may_release(completed),
            "the teardown plate released the cover 12.9 s early"
        );
        completed = count_completed_close(completed, 500, 500); // the character load
        assert!(switch_cover_may_release(completed));
    }

    /// THE MEASURED DEFECT this replaced the ordinal for. The user's ProfileSelect switch shows ONE
    /// plate, already the character load's -- `loadscreen_builds` advanced by exactly one per cover
    /// window across both switches of the 2026-09-06 run, and each window's single
    /// `finish/result sent` reported `frame=500/500`. Under `close_hits >= 2` that window could
    /// never open, so the cover rode the 35 s FPS bail: `cover_window_ms=35005` and `=35017`,
    /// 15.3 s and 18.9 s after the bar filled.
    #[test]
    fn a_one_plate_switch_releases_on_its_only_plate() {
        let completed = count_completed_close(0, 500, 500);
        assert!(
            switch_cover_may_release(completed),
            "the single character-load plate left the gate shut; the cover rides the 35 s FPS bail"
        );
    }

    /// And a window that has only ever seen teardown plates stays covered -- the property the
    /// ordinal was protecting, kept.
    #[test]
    fn teardown_plates_alone_never_open_the_gate() {
        let mut completed = 0;
        for _ in 0..3 {
            completed = count_completed_close(completed, 1, 500);
        }
        assert_eq!(completed, 0);
        assert!(!switch_cover_may_release(completed));
        // A gauge-less plate reports nothing and must not count either.
        assert!(!switch_cover_may_release(count_completed_close(0, 0, 0)));
    }

    /// The two sites that ask "is the gauge done" now share one definition, which is the whole
    /// point of the fix: `boot_world_phase_reached(EnteringWorld)` already refused the bare close,
    /// and the release predicate did not.
    #[test]
    fn gauge_done_matches_the_entering_world_form() {
        for (cur, max) in [
            (0usize, 0usize),
            (1, 500),
            (499, 500),
            (500, 500),
            (501, 500),
        ] {
            assert_eq!(
                gauge_done(cur, max),
                max == 0 || cur >= max,
                "cur={cur} max={max}"
            );
        }
    }
}
