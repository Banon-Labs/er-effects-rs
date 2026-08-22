//! Detects a Seamless connection that has stopped progressing, so the reject loop can recover.
//!
//! POLICY (user, 2026-08-06, verbatim intent): the reject loop "never stops until they use the
//! lynchpin again, and it should self recover from seamless connection stalls. Invasion connections
//! should be reliably quick and for some reason, seamless doesn't auto-cancel its own connections
//! during these edge cases."
//!
//! So this is a STALL DETECTOR, never a cap. It counts no attempts, bounds no total time, and has
//! no notion of giving up. The only thing it can conclude is "this particular handshake step has sat
//! still far longer than one ever does", and the only thing it can ask for is the cancel the player
//! could have pressed by hand -- after which the existing auto-search rearm carries on exactly as
//! before. A watchdog that could end the loop would be the wrong shape whatever its threshold.
//!
//! WHICH STATES ARE TIMED, AND WHY THAT IS THE WHOLE DESIGN
//! -------------------------------------------------------
//! Observed session walk for an invader:
//!
//! ```text
//!   0x00 IDLE -> 0x0d SEARCHING -> 0x0e -> 0x12 -> 0x13 -> 0x15 (join data)
//!             -> 0x22 CANCELLING -> 0x23 -> 0x00 IDLE -> restart
//! ```
//!
//! `SEARCHING` is UNBOUNDED BY NATURE -- it means "looking, nobody matched yet", and a player in a
//! quiet bracket can legitimately sit there for many minutes. Measured 2026-08-06: three consecutive
//! filtered queries returned 0, 0, then 1 lobby. Timing that state would fire a stall on an
//! perfectly healthy empty search, which is the single most obvious way to get this wrong.
//!
//! The handshake states are the opposite: measured `0x0d -> 0x15` in under 1s (n=10) and
//! `0x22 -> 0x00` in 2s or less (n=8), with exactly ONE outlier where cancelling hung for 30s. That
//! outlier is the failure this exists for, and the gap between 2s and 30s is where the threshold
//! goes.
//!
//! `IDLE` is untimed for the same reason as `SEARCHING`: it is a resting state, not a step.

/// Session states, mirroring `local_invasion_filter::ersc`. Duplicated deliberately: this module is
/// pure so it can be unit-tested without the game, and a `use` of the game-facing module would drag
/// in memory reads that cannot run on a test host.
pub mod state {
    pub const IDLE: u32 = 0x00;
    pub const SEARCHING: u32 = 0x0d;
    /// Seamless's own "nobody matched, go round again" step. Part of the healthy idle cycle
    /// `0x0e -> 0x11 -> 0x0d`, NOT a handshake -- see [`TIMED_STATES`] for what timing it cost.
    pub const RETRYING: u32 = 0x11;
    pub const CANCELLING: u32 = 0x22;
}

/// The ONLY states this detector times, listed because each one was measured to be brief.
///
/// # This is an allowlist, and it is an allowlist because a blocklist shipped and broke a live run
///
/// The first version asked "is this state neither IDLE nor SEARCHING?" and timed everything else.
/// That makes every state nobody has measured a stall by default, which is exactly backwards: an
/// unmeasured state is one we know nothing about, and the safe treatment of it is to leave it
/// alone. Measured 2026-08-06, one run, after that version shipped:
///
/// ```text
///   0x11 -> 0x0d     0 times   <- Seamless's own retry, extinct
///   0x11 -> 0x22    33 times   <- this detector cancelling it instead
///   "connection stalled at state 0x11 for 5000ms"  x31
/// ```
///
/// `0x11` is the step Seamless passes through when a search found nobody and is about to go round
/// again -- the same `0x0e -> 0x11 -> 0x0d` cycle that ran 12 times in an earlier healthy capture.
/// Cancelling it every five seconds killed the retry the game was about to do by itself, and the
/// player could not match anyone for as long as the build was loaded. The detector was strictly
/// worse than not existing.
///
/// So: a state earns its place here by having been observed to complete quickly. Anything absent --
/// unknown, newly introduced by a Seamless update, or simply never seen -- is never timed.
const TIMED_STATES: [u32; 5] = [
    0x0e, // search -> connect handoff
    0x12, // connect
    0x13, // connect
    0x22, // cancelling
    0x23, // cancel settling
];

// 0x15 (join data arrived) is DELIBERATELY ABSENT, and it used to be here.
//
// Judgement happens synchronously the moment join data lands, so a REJECT leaves 0x15 in zero
// ticks. The only way the session dwells there is AFTER a match was KEPT -- and that dwell is the
// player loading into the host's world, which takes far longer than any handshake. Timing it
// cancelled a successful invasion five seconds after accepting it: the log read `KEEP 0x3c2a2400
// (ExactBlock)` and then `connection stalled at state 0x15 for 5000ms -- cancelled it`, and from
// the player's seat the invasion appeared and dismissed itself instantly (2026-08-06).
//
// Removing it is defence in depth, not the fix. The fix is that the watchdog does not run at all
// once a match is kept -- see the caller's arming gate -- because a successful join also walks
// 0x22 and 0x23, so no choice of timed states could have made this safe on its own.

/// Whether a state is one we have measured to be brief, and may therefore time.
///
/// Fails CLOSED on anything unrecognised: not timed, never cancelled.
fn is_transient(state: u32) -> bool {
    TIMED_STATES.contains(&state)
}

/// How long a transient handshake state may sit without progressing before it counts as stalled.
///
/// Chosen from the measured spread, not from taste: healthy transitions complete in under 1s
/// (connect, n=10) and under 2s (cancel, n=8); the one observed stall lasted 30s. Five seconds is
/// above every healthy sample by more than 2x and far below the failure, so it cannot fire on a
/// slow-but-live handshake and cannot take 30s to notice a dead one.
pub const STALL_THRESHOLD_MS: u64 = 5_000;

/// What the watchdog wants done. There is exactly one action, and it is the one a player could
/// perform by hand -- the loop is never ended, only unstuck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StallAction {
    /// Drive ERSC's "Cancel search". The session walks back to `IDLE`, the existing auto-search
    /// rearm restarts the hunt, and the loop continues as though the stall had never happened.
    CancelAndResearch,
}

/// Watches session-state observations and reports a state that has stopped progressing.
///
/// Fed by the same poll that already traces session state, so it needs no new detour. It holds no
/// counters that could accumulate toward a limit -- only "which state, and since when".
#[derive(Debug, Default)]
pub struct StallWatchdog {
    /// The state currently being timed, and the timestamp it was entered. `None` while resting in
    /// an untimed state.
    timing: Option<(u32, u64)>,
    /// Set once a stall has been reported for the CURRENT entry into a state, so a single stall
    /// produces a single cancel rather than one per poll. Cleared by any state change, because a
    /// state change is progress and the next stall is a new event.
    reported: bool,
}

impl StallWatchdog {
    /// `const` so it can initialise a `static Mutex<StallWatchdog>` directly. Deriving `Default`
    /// is not enough for that: `Default::default()` cannot run in a const context.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            timing: None,
            reported: false,
        }
    }

    /// Feed one observation. `now_ms` is any monotonic millisecond clock.
    ///
    /// Returns `Some(action)` exactly once per stalled state entry. Re-entering the same state
    /// later is a NEW entry and can stall again -- which is the point, since a repeatedly stalling
    /// handshake must be repeatedly recovered, not silently tolerated after the first time.
    pub fn observe(&mut self, state: u32, now_ms: u64) -> Option<StallAction> {
        match self.timing {
            // A different state than we were timing: progress. Start over.
            Some((timed, _)) if timed != state => {
                self.timing = is_transient(state).then_some((state, now_ms));
                self.reported = false;
                None
            }
            // Still in the same transient state -- the only case that can stall.
            Some((_, entered)) => {
                if self.reported {
                    return None;
                }
                // Saturating: a clock that appears to go backwards yields 0 elapsed and simply
                // does not fire, rather than underflowing into an instant false stall.
                if now_ms.saturating_sub(entered) >= STALL_THRESHOLD_MS {
                    self.reported = true;
                    return Some(StallAction::CancelAndResearch);
                }
                None
            }
            // Was resting; begin timing if this state is one that should be brief.
            None => {
                if is_transient(state) {
                    self.timing = Some((state, now_ms));
                    self.reported = false;
                }
                None
            }
        }
    }

    /// How long the current transient state has been held, for the trace line. `None` while resting.
    pub fn held_ms(&self, now_ms: u64) -> Option<u64> {
        self.timing
            .map(|(_, entered)| now_ms.saturating_sub(entered))
    }

    /// The state being timed, if any.
    pub fn timing_state(&self) -> Option<u32> {
        self.timing.map(|(s, _)| s)
    }

    /// Forget what is being timed, so a later observation starts a fresh clock.
    ///
    /// Called when the hunt stops -- a match was kept, the player cancelled, or they opened
    /// Seamless's menu. Without this, the elapsed time accumulated while hunting would carry into
    /// whatever the session does next and fire a stall against it immediately.
    pub fn stand_down(&mut self) {
        self.timing = None;
        self.reported = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The failure this module exists for: cancelling hung for 30s where 8 healthy samples took
    /// 2s or less.
    #[test]
    fn a_hung_cancel_is_recovered() {
        let mut w = StallWatchdog::new();
        assert_eq!(w.observe(state::CANCELLING, 0), None);
        assert_eq!(
            w.observe(state::CANCELLING, 4_999),
            None,
            "under threshold must not fire"
        );
        assert_eq!(
            w.observe(state::CANCELLING, 5_000),
            Some(StallAction::CancelAndResearch),
        );
    }

    /// THE MOST OBVIOUS WAY TO GET THIS WRONG. `SEARCHING` means "nobody matched yet" and is
    /// unbounded; three consecutive live queries returned 0, 0, 1 on 2026-08-06. Timing it would
    /// fire on a healthy quiet bracket and cancel a search the player wanted to continue.
    #[test]
    fn a_long_search_is_never_a_stall() {
        let mut w = StallWatchdog::new();
        for minute in 0..30 {
            assert_eq!(
                w.observe(state::SEARCHING, minute * 60_000),
                None,
                "searching for {minute} minutes is normal, not a stall",
            );
        }
    }

    /// THE REGRESSION THIS MODULE SHIPPED, 2026-08-06, caught only by a live run.
    ///
    /// `0x11` is Seamless's own "found nobody, go round again" step -- the `0x0e -> 0x11 -> 0x0d`
    /// cycle. The first version timed it because it was neither IDLE nor SEARCHING, cancelled it
    /// 31 times in one session, and the player could not match anyone at all while that build was
    /// loaded. The detector was worse than not existing.
    #[test]
    fn the_retry_step_is_never_a_stall() {
        let mut w = StallWatchdog::new();
        for second in 0..120 {
            assert_eq!(
                w.observe(state::RETRYING, second * 1_000),
                None,
                "0x11 is Seamless retrying by itself; cancelling it kills the search"
            );
        }
    }

    /// The property, not the three states I happened to think of. Anything unmeasured must fail
    /// closed -- the previous test suite asserted only that IDLE and SEARCHING were exempt, which
    /// is precisely the belief that was wrong, so it passed while the bug shipped.
    #[test]
    fn every_state_outside_the_measured_set_is_left_alone() {
        for state in 0..=0xffu32 {
            if TIMED_STATES.contains(&state) {
                continue;
            }
            let mut w = StallWatchdog::new();
            w.observe(state, 0);
            assert_eq!(
                w.observe(state, 3_600_000),
                None,
                "state {state:#04x} was never measured, so it must never be cancelled -- an \
                 unknown state is not evidence of a stall",
            );
        }
    }

    /// ...and the measured ones still do their job, so failing closed did not disarm the feature.
    #[test]
    fn every_measured_state_still_stalls() {
        for state in TIMED_STATES {
            let mut w = StallWatchdog::new();
            w.observe(state, 0);
            assert_eq!(
                w.observe(state, STALL_THRESHOLD_MS),
                Some(StallAction::CancelAndResearch),
                "state {state:#04x} is in the measured set and must still be recoverable",
            );
        }
    }

    /// THE SECOND REGRESSION, 2026-08-06: a KEPT match was cancelled 5s after being accepted.
    ///
    /// `0x15` is where the session sits while the player loads into the host's world, which is far
    /// longer than any handshake. It must never be timed.
    #[test]
    fn the_join_state_is_never_a_stall() {
        let mut w = StallWatchdog::new();
        for second in 0..90 {
            assert_eq!(
                w.observe(0x15, second * 1_000),
                None,
                "0x15 is a live invasion loading in, not a stuck handshake",
            );
        }
    }

    /// Standing down forgets the accumulated clock. Without this, time banked while hunting would
    /// carry into whatever the session does next and fire against it instantly.
    #[test]
    fn standing_down_forgets_the_clock() {
        let mut w = StallWatchdog::new();
        w.observe(0x22, 0);
        w.stand_down();
        assert!(w.timing_state().is_none());
        assert_eq!(
            w.observe(0x22, 4_000),
            None,
            "after standing down the clock restarts; 4s must not read as 4s already elapsed",
        );
        assert_eq!(
            w.observe(0x22, 9_000),
            Some(StallAction::CancelAndResearch),
            "and the fresh clock still reaches the threshold on its own terms",
        );
    }

    /// Idle is a resting state, not a handshake step.
    #[test]
    fn idle_is_never_a_stall() {
        let mut w = StallWatchdog::new();
        assert_eq!(w.observe(state::IDLE, 0), None);
        assert_eq!(w.observe(state::IDLE, 600_000), None);
        assert!(w.timing_state().is_none());
    }

    /// A healthy handshake walks through several transient states quickly; each transition restarts
    /// the clock, so no individual step ever approaches the threshold.
    #[test]
    fn a_healthy_handshake_never_fires() {
        let mut w = StallWatchdog::new();
        let walk = [0x0d, 0x0e, 0x12, 0x13, 0x15, 0x22, 0x23, 0x00];
        for (i, s) in walk.iter().enumerate() {
            // ~200ms per step: comfortably inside the measured sub-second connect.
            assert_eq!(w.observe(*s, i as u64 * 200), None, "state {s:#04x} fired");
        }
    }

    /// A step that is slow but still MOVING must not fire. The clock restarts on every transition,
    /// so total elapsed across a handshake is irrelevant -- only time in one state matters. This is
    /// what makes the detector a stall detector rather than a disguised total-time cap.
    #[test]
    fn slow_but_progressing_is_not_a_stall() {
        let mut w = StallWatchdog::new();
        let mut t = 0;
        for s in [0x0e_u32, 0x12, 0x13, 0x15] {
            t += 4_000; // 4s per step, 16s total -- far past the threshold in aggregate
            assert_eq!(w.observe(s, t), None, "progress must reset the clock");
        }
    }

    /// One stall produces ONE cancel, not one per poll. The poll runs every frame; without this a
    /// single stall would drive dozens of cancels.
    #[test]
    fn a_stall_fires_once_per_entry() {
        let mut w = StallWatchdog::new();
        w.observe(state::CANCELLING, 0);
        assert!(w.observe(state::CANCELLING, 5_000).is_some());
        for t in 5..60 {
            assert_eq!(
                w.observe(state::CANCELLING, t * 1_000),
                None,
                "re-fired at {t}s"
            );
        }
    }

    /// ...but a LATER entry into the same state is a new event. A handshake that stalls every time
    /// must be recovered every time; suppressing the second one would strand the player after the
    /// first recovery, which is exactly the "never stops" policy being violated.
    #[test]
    fn a_second_stall_in_a_later_entry_fires_again() {
        let mut w = StallWatchdog::new();
        w.observe(state::CANCELLING, 0);
        assert!(w.observe(state::CANCELLING, 5_000).is_some());
        // recovered: back to idle, then searching, then stuck again
        w.observe(state::IDLE, 6_000);
        w.observe(state::SEARCHING, 7_000);
        w.observe(state::CANCELLING, 8_000);
        assert!(
            w.observe(state::CANCELLING, 13_000).is_some(),
            "a repeat stall must be recovered again -- the loop never gives up",
        );
    }

    /// A clock that appears to move backwards must not synthesise an instant stall.
    #[test]
    fn a_backwards_clock_does_not_fire() {
        let mut w = StallWatchdog::new();
        w.observe(state::CANCELLING, 10_000);
        assert_eq!(w.observe(state::CANCELLING, 1_000), None);
    }

    /// The trace needs to say how long a state has been held; resting states report nothing rather
    /// than a misleading zero.
    #[test]
    fn held_time_is_reported_only_while_timing() {
        let mut w = StallWatchdog::new();
        w.observe(state::IDLE, 1_000);
        assert_eq!(w.held_ms(2_000), None);
        w.observe(state::CANCELLING, 3_000);
        assert_eq!(w.held_ms(4_500), Some(1_500));
        assert_eq!(w.timing_state(), Some(state::CANCELLING));
    }

    /// The threshold must sit strictly between the healthy spread and the observed failure. Pinned
    /// so a later "tidy up the constant" cannot quietly move it onto either side.
    #[test]
    fn threshold_sits_between_measured_healthy_and_measured_stall() {
        const {
            assert!(
                STALL_THRESHOLD_MS > 2_000,
                "must not fire on the slowest healthy cancel (2s, n=8)"
            )
        };
        const {
            assert!(
                STALL_THRESHOLD_MS < 30_000,
                "must notice the observed 30s stall well before it ends"
            )
        };
    }
}
