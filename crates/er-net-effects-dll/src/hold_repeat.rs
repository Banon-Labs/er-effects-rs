//! Arrow-key hold-to-repeat for the effect selector.
//!
//! A press moves ONE step. Moving further is a deliberate hold, and the hold has three phases:
//!
//! 1. **LATCH** -- nothing repeats until the key has been down for [`RepeatTimings::latch`].
//!    A press is a press however long the finger lingers on it; only a hold that outlasts the
//!    latch is read as "keep going".
//! 2. **STEADY** -- repeats arrive one at a time at a fixed [`RepeatTimings::steady`] interval,
//!    slow enough to watch the selection land on each entry and stop on the one you wanted.
//! 3. **ACCELERATE** -- only after [`RepeatTimings::accelerate_after`] of steady repeating does
//!    the interval start shortening, by [`RepeatTimings::accel_step`] per repeat down to
//!    [`RepeatTimings::min_interval`], for crossing a long catalog.
//!
//! The window that matters is phase 2: acceleration is measured from the FIRST REPEAT, not from
//! the press, so "held long enough to repeat" and "held long enough to speed up" are two clearly
//! separate commitments rather than one ramp that starts the instant repeating does.
//!
//! Deliberately free of Windows and of global state: `now` is a parameter, so every phase
//! boundary above is asserted offline in this file's tests rather than felt for in the game.

// Only the DirectInput hook (Windows-only) drives this, but the module itself is deliberately
// portable so its phase boundaries are asserted by `cargo test` on the host rather than felt for
// in a running game.
#![cfg_attr(not(windows), allow(dead_code))]

use std::time::{Duration, Instant};

/// Directions that repeat, in the order the caller indexes them.
pub(crate) const REPEAT_KEY_COUNT: usize = 4;

/// The shipped feel, taken from the platform conventions a user's fingers are already calibrated
/// to rather than from taste:
///
/// * `latch` 500ms -- the Windows typematic default (`SPI_GETKEYBOARDDELAY`), which Unity's
///   Standalone Input Module and Godot's UI-echo proposal both mirror. A deliberate keypress
///   runs 80-150ms and a slow one rarely passes 300ms, so half a second cannot be hit by
///   accident.
/// * `steady` 125ms -- 8 steps/second. Windows' own 20-30/s typematic rate is tuned for TEXT,
///   where a wrong character is trivially deleted; a list selection is not, and Unity's UI
///   default is the far slower 10 actions/second. 8/s sits just under that, so releasing on the
///   entry you wanted is a reflex rather than a gamble.
/// * `accelerate_after` 1.5s -- twelve steady steps. Long enough that nudging a few entries
///   along never reaches it, short enough to not feel stuck crossing an 843-entry catalog.
/// * `accel_step` 10ms / `min_interval` 40ms -- the floor is 25/s, inside the 20-30/s band
///   Windows itself uses, reached over ~8 repeats so the speed-up is a ramp rather than a jump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RepeatTimings {
    pub(crate) latch: Duration,
    pub(crate) steady: Duration,
    pub(crate) accelerate_after: Duration,
    pub(crate) accel_step: Duration,
    pub(crate) min_interval: Duration,
}

impl Default for RepeatTimings {
    fn default() -> Self {
        Self {
            latch: Duration::from_millis(500),
            steady: Duration::from_millis(125),
            accelerate_after: Duration::from_millis(1500),
            accel_step: Duration::from_millis(10),
            min_interval: Duration::from_millis(40),
        }
    }
}

#[derive(Clone, Copy, Default)]
struct KeyState {
    /// When the next repeat is due. `None` means the key is not held, or is held but was never
    /// latched -- both of which re-arm from the press.
    next_repeat_at: Option<Instant>,
    /// The instant acceleration becomes legal: first repeat + `accelerate_after`.
    accelerate_from: Option<Instant>,
    interval: Duration,
}

/// One independent hold per direction. Left accelerating says nothing about Up.
pub(crate) struct HoldRepeat {
    timings: RepeatTimings,
    keys: [KeyState; REPEAT_KEY_COUNT],
}

impl Default for HoldRepeat {
    fn default() -> Self {
        Self::new(RepeatTimings::default())
    }
}

impl HoldRepeat {
    pub(crate) fn new(timings: RepeatTimings) -> Self {
        Self {
            timings,
            keys: [KeyState::default(); REPEAT_KEY_COUNT],
        }
    }

    /// Forget every hold. Used when the selector closes or the device read fails: a key that was
    /// down while we were not watching must not be credited with the hold it had then.
    pub(crate) fn reset(&mut self) {
        self.keys = [KeyState::default(); REPEAT_KEY_COUNT];
    }

    /// Advance one direction by one poll. Returns whether THIS poll owes a repeat -- at most one,
    /// however late the poll is. A frame hitch therefore costs repeats rather than paying them
    /// back in a burst, which is the difference between a stutter and the selection bolting.
    ///
    /// `edge` is the press itself, already delivered as the single step by the caller; here it
    /// only starts the clock.
    pub(crate) fn observe(&mut self, index: usize, held: bool, edge: bool, now: Instant) -> bool {
        let timings = self.timings;
        let Some(key) = self.keys.get_mut(index) else {
            return false;
        };

        if !held {
            *key = KeyState::default();
            return false;
        }

        // Either a fresh press, or a key found already down with no clock running (the selector
        // just opened under a held key). Both start at the latch: no repeat is owed yet.
        if edge || key.next_repeat_at.is_none() {
            let first_repeat_at = now + timings.latch;
            key.next_repeat_at = Some(first_repeat_at);
            key.accelerate_from = Some(first_repeat_at + timings.accelerate_after);
            key.interval = timings.steady;
            return false;
        }

        let Some(due_at) = key.next_repeat_at else {
            return false;
        };
        if now < due_at {
            return false;
        }

        let accelerating = key.accelerate_from.is_some_and(|from| now >= from);
        let interval = if accelerating {
            key.interval
                .checked_sub(timings.accel_step)
                .unwrap_or(timings.min_interval)
                .max(timings.min_interval)
        } else {
            timings.steady
        };
        key.interval = interval;
        key.next_repeat_at = Some(now + interval);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: usize = 0;
    const RIGHT: usize = 1;

    fn timings() -> RepeatTimings {
        RepeatTimings::default()
    }

    /// Drive one held key from `start` in `step` increments and collect the instants that repeat.
    fn repeats_while_held(
        repeat: &mut HoldRepeat,
        index: usize,
        start: Instant,
        step: Duration,
        span: Duration,
    ) -> Vec<Duration> {
        let mut fired = Vec::new();
        let mut now = start;
        repeat.observe(index, true, true, now);
        while now <= start + span {
            now += step;
            if repeat.observe(index, true, false, now) {
                fired.push(now - start);
            }
        }
        fired
    }

    #[test]
    fn a_press_and_release_never_repeats() {
        let mut repeat = HoldRepeat::new(timings());
        let start = Instant::now();
        assert!(
            !repeat.observe(LEFT, true, true, start),
            "the press itself is not a repeat"
        );
        // Held for 400ms -- a slow, deliberate press, still short of the latch.
        for tick in 1..=25 {
            let now = start + Duration::from_millis(16 * tick);
            assert!(
                !repeat.observe(LEFT, true, false, now),
                "no repeat before the latch, at {}ms",
                16 * tick
            );
        }
        assert!(!repeat.observe(LEFT, false, false, start + Duration::from_millis(410)));
    }

    #[test]
    fn the_first_repeat_waits_the_full_latch() {
        let mut repeat = HoldRepeat::new(timings());
        let start = Instant::now();
        let fired = repeats_while_held(
            &mut repeat,
            LEFT,
            start,
            Duration::from_millis(16),
            Duration::from_millis(600),
        );
        let first = *fired
            .first()
            .expect("a 600ms hold outlasts the 500ms latch");
        assert!(
            (Duration::from_millis(500)..Duration::from_millis(520)).contains(&first),
            "first repeat landed at {first:?}, not just after the 500ms latch"
        );
    }

    #[test]
    fn steady_phase_repeats_one_at_a_time_and_does_not_accelerate() {
        let timings = timings();
        let mut repeat = HoldRepeat::new(timings);
        let start = Instant::now();
        let poll = Duration::from_millis(16);
        // The latch plus the whole no-acceleration window, stopping just short of its end.
        let fired = repeats_while_held(
            &mut repeat,
            LEFT,
            start,
            poll,
            timings.latch + timings.accelerate_after - Duration::from_millis(50),
        );
        assert!(
            fired.len() >= 7,
            "expected the steady cadence to keep stepping, got {fired:?}"
        );
        for pair in fired.windows(2) {
            let gap = pair[1] - pair[0];
            assert!(
                (timings.steady..=timings.steady + poll + Duration::from_millis(4)).contains(&gap),
                "steady gap drifted to {gap:?}; acceleration started inside the steady window"
            );
        }
    }

    #[test]
    fn acceleration_starts_only_after_the_steady_window_and_ramps_to_the_floor() {
        let timings = timings();
        let mut repeat = HoldRepeat::new(timings);
        let start = Instant::now();
        let fired = repeats_while_held(
            &mut repeat,
            LEFT,
            start,
            Duration::from_millis(16),
            Duration::from_millis(8_000),
        );
        let accel_begins = timings.latch + timings.accelerate_after;
        let mut last_gap = None;
        for pair in fired.windows(2) {
            let gap = pair[1] - pair[0];
            if pair[1] <= accel_begins {
                assert!(
                    gap >= timings.steady,
                    "a gap of {gap:?} before {accel_begins:?} means it sped up too early"
                );
            } else {
                last_gap = Some(gap);
            }
        }
        let last_gap = last_gap.expect("an 8s hold repeats well past the acceleration threshold");
        assert!(
            last_gap < timings.steady,
            "after a long hold the repeats never sped up (last gap {last_gap:?})"
        );
        assert!(
            last_gap >= timings.min_interval,
            "repeats outran the {:?} floor ({last_gap:?})",
            timings.min_interval
        );
    }

    #[test]
    fn releasing_resets_the_whole_ramp() {
        let mut repeat = HoldRepeat::new(timings());
        let start = Instant::now();
        let _ = repeats_while_held(
            &mut repeat,
            LEFT,
            start,
            Duration::from_millis(16),
            Duration::from_millis(6_000),
        );
        let released = start + Duration::from_millis(6_100);
        assert!(!repeat.observe(LEFT, false, false, released));

        // The next press is a press again: one step, then the full latch before anything repeats.
        let again = released + Duration::from_millis(500);
        assert!(!repeat.observe(LEFT, true, true, again));
        assert!(
            !repeat.observe(LEFT, true, false, again + Duration::from_millis(400)),
            "a fresh press inherited the previous hold's acceleration"
        );
        assert!(repeat.observe(LEFT, true, false, again + Duration::from_millis(505)));
    }

    #[test]
    fn each_direction_holds_independently() {
        let mut repeat = HoldRepeat::new(timings());
        let start = Instant::now();
        let _ = repeats_while_held(
            &mut repeat,
            LEFT,
            start,
            Duration::from_millis(16),
            Duration::from_millis(6_000),
        );
        // RIGHT is pressed while LEFT is deep into acceleration; it starts from its own latch.
        let press_right = start + Duration::from_millis(6_016);
        assert!(!repeat.observe(RIGHT, true, true, press_right));
        assert!(!repeat.observe(RIGHT, true, false, press_right + Duration::from_millis(400)));
        assert!(repeat.observe(RIGHT, true, false, press_right + Duration::from_millis(505)));
    }

    #[test]
    fn a_stalled_poll_owes_one_repeat_not_a_burst() {
        let mut repeat = HoldRepeat::new(timings());
        let start = Instant::now();
        assert!(!repeat.observe(LEFT, true, true, start));
        // The game hitches for two full seconds while the key is held. One poll, one repeat --
        // the missed ones are lost, not paid back all at once into the selector.
        assert!(repeat.observe(LEFT, true, false, start + Duration::from_millis(2_500)));
        assert!(
            !repeat.observe(LEFT, true, false, start + Duration::from_millis(2_600)),
            "the next repeat is still one steady interval away"
        );
        assert!(repeat.observe(LEFT, true, false, start + Duration::from_millis(2_760)));
    }

    #[test]
    fn a_key_found_already_down_latches_instead_of_firing() {
        let mut repeat = HoldRepeat::new(timings());
        let start = Instant::now();
        // No edge: the selector opened under a finger that was already holding the key.
        assert!(!repeat.observe(LEFT, true, false, start));
        assert!(!repeat.observe(LEFT, true, false, start + Duration::from_millis(400)));
        assert!(repeat.observe(LEFT, true, false, start + Duration::from_millis(510)));
    }

    #[test]
    fn reset_forgets_a_hold_in_flight() {
        let mut repeat = HoldRepeat::new(timings());
        let start = Instant::now();
        let _ = repeats_while_held(
            &mut repeat,
            LEFT,
            start,
            Duration::from_millis(16),
            Duration::from_millis(3_000),
        );
        repeat.reset();
        // Still physically held, but the clock restarts: no repeat until a fresh latch elapses.
        let resumed = start + Duration::from_millis(3_016);
        assert!(!repeat.observe(LEFT, true, false, resumed));
        assert!(!repeat.observe(LEFT, true, false, resumed + Duration::from_millis(400)));
        assert!(repeat.observe(LEFT, true, false, resumed + Duration::from_millis(505)));
    }
}
