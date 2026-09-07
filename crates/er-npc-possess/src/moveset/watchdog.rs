//! The thing that stops a bad animation from ending the session.
//!
//! # Why this exists at all
//!
//! The offline classifier gates on everything it can see: a transition consumes the event, the
//! target state has a clip, the TimeAct opens a damage window, the behaviour row resolves. What it
//! cannot see is whether the animation TERMINATES for a creature nobody is steering. Some states
//! only exit on a condition the AI would have satisfied -- and the AI is exactly what possession
//! turned off, because `[vt+0x48] UpdateAi` is no-oped. A state like that leaves the player stuck
//! in a pose with no way out, which ends the session rather than merely disappointing them.
//!
//! So the classifier is allowed to be wrong, and this catches it: non-neutral, the playhead not
//! advancing, no input consumed, for long enough that nothing else explains it. The animation is then forced
//! back to idle and written into `er-npc-possess.derived.toml` as `unusable`, so the same move
//! cannot cost the player a second session. The classifier heals from its own failures rather than
//! needing a corpus change.
//!
//! # Why all three conditions, and not just the timer
//!
//! Each one alone has a legitimate long case. A slow wind-up is non-neutral for seconds. A clip
//! can be paused by the engine for a frame. A player who put the pad down is consuming no input.
//! Only the conjunction -- non-neutral, the playhead frozen, and nobody asking for anything -- has
//! no innocent reading.
//!
//! # It asks whether the animation advanced, not whether the body moved
//!
//! It used to ask the second, and that was wrong in a way that got worse the longer a possession
//! lasted. A stance, a charge and a wind-up all animate correctly while translating nothing; more
//! damagingly, while locomotion is broken every attack translates nothing, so the watchdog would
//! deny one animation every four seconds until the creature's moveset was empty. Denials are
//! permanent for the session, so that is unrecoverable without releasing. The playhead --
//! `animQueue[readIdx].localTime` -- answers the question actually being asked.

// Pure state machine over observations; ungated so `cargo test` proves it on the host.
#![cfg_attr(not(windows), allow(dead_code))]

/// How far the playhead must move between two frames to count as advancing.
///
/// A sixtieth of a second is one frame at 60 Hz; a tenth of that is under any real advance and
/// over the float noise of re-reading the same value.
const ADVANCED_SECONDS: f32 = 1.0 / 600.0;

/// What the driver saw this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sample {
    /// The animation the TimeAct queue says is playing, from
    /// `CSChrTimeActModule::anim_queue[read_idx].anim_id`.
    pub(crate) animation: i32,
    /// Is the playing animation one of this creature's shipped moves?
    ///
    /// Resolved by the dispatcher against the table rather than from the id's magnitude. A
    /// possessed Battlemage idles in animation 43000 and spawns through 3009000/3009500; the old
    /// `animation < 3000` test read all three as "attacking", so the watchdog armed on an idle
    /// loop and the chain gate refused every press. The ids the engine reports are raw TimeAct
    /// ids in a per-creature space, and no threshold over them separates a swing from a stance.
    pub(crate) is_known_move: bool,
    /// `animQueue[readIdx].localTime` -- how far into the clip the playhead is.
    ///
    /// This is the LIVENESS test, and it replaced root motion on 2026-09-02 because root motion
    /// was measuring the wrong thing. The question is whether the animation is progressing; the
    /// old test asked whether the body was translating, and those come apart badly. A stance, a
    /// charge and a wind-up all animate perfectly while going nowhere -- and, decisively, while
    /// locomotion is broken every attack in the game goes nowhere, so the watchdog would work its
    /// way through the moveset denying one animation per four seconds until the creature had
    /// nothing left. That is a mod that gets worse the longer you wear it, and it is exactly what
    /// the live run showed.
    ///
    /// `None` when the field did not read, which is treated as advancing -- failing the other way
    /// would force idle out of a healthy attack whenever a pointer chain missed.
    pub(crate) local_time: Option<f32>,
    /// Did the engine act on the player's input this frame -- did a request actually land?
    ///
    /// Consumed, not held, and the distinction is the whole value of this field. Reading it as
    /// "the player is touching the controller" hands the softlock case a free pass: somebody
    /// stuck in a pose mashes and holds, which would reset the timer on every frame and mean the
    /// watchdog could never fire in the one situation it exists for. Input that produced nothing
    /// is evidence of being stuck, not evidence against it.
    pub(crate) input_consumed: bool,
    pub(crate) now_ms: u64,
}

impl Sample {
    /// Is the creature doing something this crate fired?
    ///
    /// The positive test. It used to be `animation < NEUTRAL_ANIMATION_CEILING`, and that
    /// threshold cannot express the question -- see [`Sample::is_known_move`].
    const fn is_neutral(self) -> bool {
        !self.is_known_move
    }

    /// Did the playhead move since `previous`? An unreadable clock counts as advancing.
    fn advanced_since(self, previous: Option<f32>) -> bool {
        match (self.local_time, previous) {
            (Some(now), Some(before)) => (now - before).abs() >= ADVANCED_SECONDS,
            _ => true,
        }
    }
}

/// What the watchdog wants done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Nothing to do.
    Fine,
    /// The creature came back to a neutral state; the dispatcher's combo cursors should reset.
    ReturnedToNeutral,
    /// Stuck. Force idle, and deny this animation for the rest of the session.
    ForceIdle {
        /// The animation to fire to get out -- always the idle clip.
        idle: i32,
        /// The animation that stuck, to be written into the derived file as `unusable`. This is
        /// the fired id, not the played one, because that is what the config would have to name.
        blame: i32,
    },
}

/// One possession's stuck-detector.
#[derive(Clone, Debug)]
pub(crate) struct Watchdog {
    /// How long the three conditions must hold together before forcing idle.
    threshold_ms: u64,
    /// The animation to request to return to idle. `W_Event0000` -> `a000_000000`.
    idle_animation: i32,
    /// The move currently being watched, and when it stopped looking healthy.
    armed: Option<Armed>,
    /// Was the previous sample non-neutral? Used to fire [`Verdict::ReturnedToNeutral`] exactly
    /// once per return rather than on every neutral frame.
    was_busy: bool,
}

#[derive(Clone, Copy, Debug)]
struct Armed {
    /// The id the dispatcher fired, which is what the derived file has to name.
    fired: i32,
    /// The first moment all three stuck conditions held. Cleared the moment any of them stops.
    suspect_since_ms: Option<u64>,
    /// The playhead at the previous sample, to tell an advancing clip from a frozen one.
    last_local_time: Option<f32>,
}

impl Watchdog {
    pub(crate) const fn new(threshold_ms: u64, idle_animation: i32) -> Self {
        Self {
            threshold_ms,
            idle_animation,
            armed: None,
            was_busy: false,
        }
    }

    /// A move was just requested. Watch it.
    pub(crate) const fn armed_with(&mut self, fired: i32) {
        self.armed = Some(Armed {
            fired,
            suspect_since_ms: None,
            last_local_time: None,
        });
        self.was_busy = true;
    }

    /// Test-only: the driver never asks, because [`Self::observe`] already handles the unarmed
    /// case. It exists so the tests can assert that a verdict really disarmed rather than
    /// inferring it from the next call's behaviour.
    #[cfg(test)]
    pub(crate) const fn is_armed(&self) -> bool {
        self.armed.is_some()
    }

    /// One frame. Called whether or not anything was fired.
    pub(crate) fn observe(&mut self, sample: Sample) -> Verdict {
        if sample.is_neutral() {
            // Whatever was playing has finished. This is the ordinary exit and it disarms.
            self.armed = None;
            if core::mem::replace(&mut self.was_busy, false) {
                return Verdict::ReturnedToNeutral;
            }
            return Verdict::Fine;
        }
        self.was_busy = true;
        let Some(armed) = self.armed.as_mut() else {
            // Non-neutral without anything of ours in flight: a hit reaction, a death, a
            // gimmick the world started. Not ours to police -- forcing idle out of a stagger
            // would be a worse bug than the one this guards against.
            return Verdict::Fine;
        };
        let advanced = sample.advanced_since(armed.last_local_time);
        armed.last_local_time = sample.local_time;
        if advanced || sample.input_consumed {
            armed.suspect_since_ms = None;
            return Verdict::Fine;
        }
        let since = *armed.suspect_since_ms.get_or_insert(sample.now_ms);
        if sample.now_ms.saturating_sub(since) < self.threshold_ms {
            return Verdict::Fine;
        }
        let blame = armed.fired;
        self.armed = None;
        Verdict::ForceIdle {
            idle: self.idle_animation,
            blame,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THRESHOLD_MS: u64 = 4000;
    const IDLE: i32 = 0;

    /// A frozen playhead: same `local_time` every frame.
    const fn stuck(now_ms: u64) -> Sample {
        Sample {
            animation: 3005,
            local_time: Some(1.0),
            is_known_move: true,
            input_consumed: false,
            now_ms,
        }
    }

    fn watchdog() -> Watchdog {
        let mut dog = Watchdog::new(THRESHOLD_MS, IDLE);
        dog.armed_with(3005);
        dog
    }

    #[test]
    fn a_frozen_animation_is_forced_back_to_idle_and_blamed() {
        let mut dog = watchdog();
        // Two samples before the clock can start, and that is not slack: one reading cannot tell
        // a frozen playhead from a moving one. The first establishes the baseline, and suspicion
        // begins at the first frame that fails to advance past it.
        assert_eq!(dog.observe(stuck(0)), Verdict::Fine);
        assert_eq!(dog.observe(stuck(1)), Verdict::Fine);
        assert_eq!(dog.observe(stuck(THRESHOLD_MS)), Verdict::Fine);
        assert_eq!(
            dog.observe(stuck(THRESHOLD_MS + 1)),
            Verdict::ForceIdle {
                idle: IDLE,
                blame: 3005
            }
        );
        assert!(!dog.is_armed(), "firing the verdict must disarm");
    }

    #[test]
    fn the_playhead_advancing_alone_is_enough_to_clear_the_suspicion() {
        let mut dog = watchdog();
        assert_eq!(dog.observe(stuck(0)), Verdict::Fine);
        assert_eq!(dog.observe(stuck(1)), Verdict::Fine);
        let moving = Sample {
            local_time: Some(2.0),
            ..stuck(1000)
        };
        assert_eq!(dog.observe(moving), Verdict::Fine);
        // The clock restarts from the next suspect frame, not from arming.
        assert_eq!(dog.observe(stuck(2000)), Verdict::Fine);
        assert_eq!(dog.observe(stuck(2001)), Verdict::Fine);
        assert_eq!(dog.observe(stuck(2000 + THRESHOLD_MS)), Verdict::Fine);
        assert_eq!(
            dog.observe(stuck(2001 + THRESHOLD_MS)),
            Verdict::ForceIdle {
                idle: IDLE,
                blame: 3005
            }
        );
    }

    /// The defect this field'S definition exists to close. A player who is genuinely stuck mashes
    /// buttons; if "input" meant "a button is down" rather than "a request landed", every one of
    /// those presses would reset the timer and the watchdog could never fire in the exact
    /// situation it is for.
    #[test]
    fn mashing_at_a_softlock_does_not_hold_the_watchdog_off() {
        let mut dog = watchdog();
        for step in 0..20 {
            // Buttons going down every frame, and nothing landing -- which is what being stuck
            // looks like from the driver's side, because the request slot never clears.
            let mashing = Sample {
                input_consumed: false,
                ..stuck(step * 500)
            };
            if let Verdict::ForceIdle { blame, .. } = dog.observe(mashing) {
                assert_eq!(blame, 3005);
                return;
            }
        }
        panic!("the watchdog never fired while the player mashed at a frozen animation");
    }

    #[test]
    fn input_alone_is_enough_to_clear_the_suspicion() {
        let mut dog = watchdog();
        assert_eq!(dog.observe(stuck(0)), Verdict::Fine);
        let asking = Sample {
            input_consumed: true,
            ..stuck(3999)
        };
        assert_eq!(dog.observe(asking), Verdict::Fine);
        assert_eq!(
            dog.observe(stuck(3999 + THRESHOLD_MS)),
            Verdict::Fine,
            "the timer restarted, so one threshold from the CLEARED frame is not yet enough"
        );
    }

    #[test]
    fn a_long_but_advancing_animation_is_never_forced() {
        let mut dog = watchdog();
        for step in 0..100 {
            let moving = Sample {
                local_time: Some(step as f32),
                ..stuck(step * 1000)
            };
            assert_eq!(dog.observe(moving), Verdict::Fine);
        }
    }

    /// The regression this test exists for. An attack that animates correctly while the body goes
    /// nowhere is a stance, a charge, a wind-up -- or any attack at all while locomotion is
    /// broken. The old root-motion test denied all of them, one every four seconds, permanently
    /// for the session. The playhead advancing is what says the animation is healthy.
    #[test]
    fn an_animation_that_advances_without_moving_the_body_is_never_denied() {
        let mut dog = watchdog();
        // A hundred seconds of a clip that plays perfectly and translates the creature zero
        // distance. Root motion is not consulted at all any more, so there is nothing to set.
        for step in 0..100u64 {
            let sample = Sample {
                animation: 3005,
                local_time: Some(step as f32 * 0.016),
                is_known_move: true,
                input_consumed: false,
                now_ms: step * 1000,
            };
            assert_eq!(
                dog.observe(sample),
                Verdict::Fine,
                "an advancing clip was denied at step {step}"
            );
        }
        assert!(dog.is_armed(), "and it is still being watched");
    }

    /// ...but a clock that cannot be read must not become a licence to never fire. It counts as
    /// advancing, which is the safe direction for a healthy animation; the guard is that the
    /// watchdog was always a backstop and never the primary exit.
    #[test]
    fn an_unreadable_playhead_counts_as_advancing() {
        let mut dog = watchdog();
        for step in 0..10u64 {
            let sample = Sample {
                animation: 3005,
                local_time: None,
                is_known_move: true,
                input_consumed: false,
                now_ms: step * THRESHOLD_MS,
            };
            assert_eq!(dog.observe(sample), Verdict::Fine);
        }
    }

    #[test]
    fn returning_to_neutral_disarms_and_reports_it_once() {
        let mut dog = watchdog();
        assert_eq!(dog.observe(stuck(0)), Verdict::Fine);
        let idle = Sample {
            animation: 0,
            is_known_move: false,
            ..stuck(100)
        };
        assert_eq!(dog.observe(idle), Verdict::ReturnedToNeutral);
        assert!(!dog.is_armed());
        assert_eq!(
            dog.observe(idle),
            Verdict::Fine,
            "the return is an edge, not a level -- the dispatcher must not be reset every frame"
        );
    }

    #[test]
    fn a_stagger_nobody_asked_for_is_not_policed() {
        // Never armed: the creature is in a hit reaction the world started.
        let mut dog = Watchdog::new(THRESHOLD_MS, IDLE);
        for step in 0..20 {
            // A hit reaction is not in the shipped table, so the positive test already says it
            // is not ours -- which is a stronger statement than the old "7010 is above 3000, but
            // we never armed" and does not depend on the arming state at all.
            let reacting = Sample {
                animation: 7010,
                is_known_move: false,
                ..stuck(step * 1000)
            };
            assert_eq!(
                dog.observe(reacting),
                Verdict::Fine,
                "forcing idle out of a hit reaction would be a worse bug than the one this guards"
            );
        }
    }

    /// The ids that broke the threshold, pinned so no future ceiling can reintroduce it.
    ///
    /// Measured on the live 2026-09-02 Battlemage run: a possessed c3704 idles in a 3-second
    /// loop whose id is 43000 and spawns through 3009000 and 3009500. Its shipped moves top out
    /// at 6023. Every one of those three is above any threshold that would still call 3000 an
    /// attack, which is why the test is membership and not magnitude.
    #[test]
    fn the_battlemage_ids_that_broke_the_ceiling_are_not_mistaken_for_this_creatures_moves() {
        let moveset = crate::moveset::table::lookup(3704).expect("c3704 ships a moveset");
        for observed in [43000, 3009000, 3009500] {
            assert!(
                moveset.playing(observed).is_none(),
                "c3704 animation {observed} was taken for one of its own moves; it idles and \
                 spawns in these and every press would be refused"
            );
        }
        assert!(
            moveset.playing(3000).is_some(),
            "...but its actual first attack must still be recognised"
        );
    }

    /// The runtime id is raw and the table's is collapsed, so the same clip in a different
    /// TimeAct group has to resolve to the same move.
    #[test]
    fn a_grouped_runtime_id_finds_the_move_it_is_a_copy_of() {
        let moveset = crate::moveset::table::lookup(3704).expect("c3704 ships a moveset");
        for group in 0..4 {
            let raw = group * 1_000_000 + 3000;
            assert_eq!(
                moveset.playing(raw).map(|entry| entry.fire),
                Some(3000),
                "group {group} spelling of animation 3000 did not resolve"
            );
        }
    }

    /// Every move this crate can fire must be recognisable when it comes back from the engine --
    /// under its own id and under any group spelling of it. A move that fires and is then not
    /// recognised is one the gate would let the next press cancel.
    #[test]
    fn every_shipped_move_is_recognised_when_it_comes_back_from_the_engine() {
        let mut checked = 0;
        for chr in crate::moveset::table::chr_ids() {
            let Some(moveset) = crate::moveset::table::lookup(chr) else {
                continue;
            };
            for entry in &moveset.moves {
                assert!(
                    moveset.playing(entry.played).is_some(),
                    "c{chr} fires {} which plays {} and would not be recognised",
                    entry.fire,
                    entry.played
                );
                assert!(
                    moveset.playing(3_000_000 + entry.played).is_some(),
                    "c{chr} would not recognise the group-3 spelling of {}",
                    entry.played
                );
                checked += 1;
            }
        }
        assert!(checked > 6000, "only {checked} moves checked");
    }
}
