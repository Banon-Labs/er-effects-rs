//! When a spawned creature may be possessed, and what to do when it never can be.
//!
//! # The deadline is the mod's, because the game has none
//!
//! There is no error edge and no timeout anywhere in the eight `ChrRes` states or the eleven
//! `EneDat` ones. A chr id whose `chrbnd` does not exist does not fail -- it sits in `LoadWait`
//! forever, quietly, with a `ChrIns` allocated and registered and nothing ever completing. So the
//! deadline here is not belt-and-braces around a native failure path; it is the only thing that
//! ends that state, and its expiry is the one signal that says "that was a bad pick".
//!
//! Nothing is pumped while it waits: `EneDatManImp::Update` walks all sixty-four slots every frame
//! from the game's own `STEP_Update`, so the load progresses whether or not this mod looks at it.
//! Waiting is genuinely waiting.
//!
//! # The gate order is a safety property, not a presentation choice
//!
//! [`Gate::ORDER`] is evaluated front to back and stops at the first one that is not satisfied.
//! That matters for [`Gate::AssetsResident`]: the predicate it mirrors, `FUN_1404ca4a0`, does not
//! null-check its `EneDat*`, and the only thing that establishes there is an `EneDat` is
//! [`Gate::ChrResLoaded`] -- `ChrIns::GetEneDat` returns null unless the step is in `3..6`. The
//! live reader defends itself as well, but the ordering is where the contract lives and
//! [`Readiness::observe`] enforces it for every caller rather than asking each one to remember.
//!
//! # Three outcomes, and only one of them may call `RemoveChrIns`
//!
//! * [`Poll::Expired`] -- we gave up. The creature exists and is ours, so we remove it.
//! * [`Poll::Vanished`] -- the game removed it. `EnemyIns::InitializeCharacterRendering`
//!   self-despawns a character whose caps loaded but yielded no FLVER, and `ChrSet::RemoveChrIns`
//!   nulls the entry on its way out. Calling `RemoveChrIns` on that pointer again would hand a
//!   freed `ChrIns` to `CSDelayDeleteMan` a second time. Drop the pointer and say so.
//! * [`Poll::Ready`] -- possession may start.

// Pure state handling; ungated so `cargo test` proves it on the host with no game running.
#![cfg_attr(not(windows), allow(dead_code))]

/// One condition a freshly spawned creature has to meet, in the order they are checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Gate {
    /// `chrSet->entries[slot].chrIns` is still the pointer the spawn returned.
    ///
    /// First because it is the cheapest and because everything after it reads through that pointer:
    /// once the game has taken the character back, the rest are reads of freed memory.
    Registered = 0,
    /// `3 <= chrRes->step < 6`, which is `ChrIns::IsInLoadedState` spelled as a field read.
    ChrResLoaded = 1,
    /// The chrbnd cap has finished (`FD4FileCap+0x88 == 4`) and yielded a `FlverResCap`.
    ///
    /// Must not be evaluated before [`Self::ChrResLoaded`]; see the module docs.
    AssetsResident = 2,
    /// The `ChrCtrl` chain possession itself needs -- the control block, its back-pointer to this
    /// `ChrIns`, and the real `ComManipulator` the thunk will forward to.
    ///
    /// Last because it is the one whose failure means "not yet built" rather than "not yet loaded",
    /// and because it is the precondition for the very next thing the engine does.
    Drivable = 3,
}

impl Gate {
    /// Every gate, in the order [`Readiness::observe`] evaluates them.
    pub(crate) const ORDER: [Self; 4] = [
        Self::Registered,
        Self::ChrResLoaded,
        Self::AssetsResident,
        Self::Drivable,
    ];

    /// For the log line.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::ChrResLoaded => "chrres-loaded",
            Self::AssetsResident => "assets-resident",
            Self::Drivable => "drivable",
        }
    }

    /// What a player can do about being stuck here. The deadline message carries it, because
    /// "assets-resident timed out" is a symptom and "that chr id has no chrbnd" is the cause.
    pub(crate) const fn stuck_means(self) -> &'static str {
        match self {
            Self::Registered => {
                "the character never appeared in the roster slot the spawn returned -- something \
                 else removed it in the same frame"
            }
            Self::ChrResLoaded => {
                "the asset step machine never reached a loaded state. That is what a chr id with \
                 no chrbnd on disk looks like: there is no error edge anywhere in ChrRes, so it \
                 waits forever rather than failing. Check the id"
            }
            Self::AssetsResident => {
                "the chrbnd loaded but produced no model. The game removes such a character \
                 itself; if this timed out instead, the cap never finished"
            }
            Self::Drivable => {
                "the character loaded but its ChrCtrl or manipulator never turned up, so there is \
                 nothing to possess"
            }
        }
    }
}

/// What one poll concluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Poll {
    /// Still coming up, currently blocked on this gate.
    Waiting(Gate),
    /// Every gate is satisfied. Possession may start.
    Ready,
    /// The game took the character away. Drop the pointer; do not remove it again.
    Vanished,
    /// The deadline passed with this gate still unsatisfied. Remove the character and report.
    Expired(Gate),
}

/// One spawn's wait, with its own clock.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Readiness {
    deadline_ms: u64,
    /// Was [`Gate::Registered`] ever true? The difference between "not yet" and "not any more",
    /// which is the difference between waiting and a self-despawn.
    seen_registered: bool,
    /// The furthest gate satisfied so far, for the log and for the expiry message.
    reached: Option<Gate>,
}

impl Readiness {
    #[must_use]
    pub(crate) const fn new(deadline_ms: u64) -> Self {
        Self {
            deadline_ms,
            seen_registered: false,
            reached: None,
        }
    }

    /// The furthest gate that has ever been satisfied, or `None` if not even the first.
    #[must_use]
    pub(crate) const fn reached(&self) -> Option<Gate> {
        self.reached
    }

    /// Evaluate the gates in order, stopping at the first that is not satisfied.
    ///
    /// `evaluate` answers `Some(true)`, `Some(false)`, or `None` for a gate that cannot be decided
    /// on the running build -- which is [`Gate::AssetsResident`] on a build whose `EneDat` offsets
    /// nobody has measured. `None` skips the gate rather than failing it: the alternative is a
    /// spawn layer that refuses to work at all on a third build, when three of its four gates are
    /// byte-proven identical across both known ones.
    ///
    /// A gate is never asked once an earlier one has answered `false`, which is the ordering
    /// contract the module docs describe and the reason this takes a closure rather than a struct
    /// of pre-computed booleans.
    pub(crate) fn observe(
        &mut self,
        elapsed_ms: u64,
        mut evaluate: impl FnMut(Gate) -> Option<bool>,
    ) -> Poll {
        for gate in Gate::ORDER {
            match evaluate(gate) {
                // Undecidable on this build. Not a pass and not a failure: skip it, and do not let
                // it become the `reached` high-water mark either, so the log never claims a gate
                // was met that was never asked.
                None => continue,
                Some(true) => {
                    if gate == Gate::Registered {
                        self.seen_registered = true;
                    }
                    self.reached = Some(match self.reached {
                        Some(previous) if previous > gate => previous,
                        _ => gate,
                    });
                }
                Some(false) => {
                    // Registration going away after it was there is the game's own despawn, and it
                    // is not a timeout however long we have been waiting -- the pointer is gone
                    // either way, and removing it again would double-free.
                    if gate == Gate::Registered && self.seen_registered {
                        return Poll::Vanished;
                    }
                    if elapsed_ms >= self.deadline_ms {
                        return Poll::Expired(gate);
                    }
                    return Poll::Waiting(gate);
                }
            }
        }
        Poll::Ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gate closure that answers from a fixed table and records what it was asked, so the
    /// ordering contract is observable rather than assumed.
    struct Recorder {
        answers: [Option<bool>; 4],
        asked: Vec<Gate>,
    }

    impl Recorder {
        fn new(answers: [Option<bool>; 4]) -> Self {
            Self {
                answers,
                asked: Vec::new(),
            }
        }

        fn evaluate(&mut self) -> impl FnMut(Gate) -> Option<bool> + '_ {
            move |gate| {
                self.asked.push(gate);
                self.answers[gate as usize]
            }
        }
    }

    #[test]
    fn every_gate_satisfied_is_ready() {
        let mut recorder = Recorder::new([Some(true); 4]);
        let mut readiness = Readiness::new(5_000);
        let verdict = readiness.observe(0, recorder.evaluate());
        assert_eq!(verdict, Poll::Ready);
        assert_eq!(recorder.asked, Gate::ORDER.to_vec());
        assert_eq!(readiness.reached(), Some(Gate::Drivable));
    }

    /// The ordering contract. `FUN_1404ca4a0` does not null-check its `EneDat*`, and the only thing
    /// that establishes there is one is the step gate before it. A poll that asked for assets while
    /// the step said "not loaded" would be reading through a pointer the game has not published.
    #[test]
    fn no_gate_is_asked_once_an_earlier_one_has_said_no() {
        let mut recorder = Recorder::new([Some(true), Some(false), Some(true), Some(true)]);
        let mut readiness = Readiness::new(5_000);
        let verdict = readiness.observe(0, recorder.evaluate());
        assert_eq!(verdict, Poll::Waiting(Gate::ChrResLoaded));
        assert_eq!(
            recorder.asked,
            vec![Gate::Registered, Gate::ChrResLoaded],
            "assets-resident must not be asked while chrres-loaded is false"
        );
    }

    /// The one outcome that must not remove the character. The game self-despawns a chr whose caps
    /// loaded but yielded no FLVER, and `ChrSet::RemoveChrIns` nulls the entry on its way out.
    /// Handing that same pointer to `WorldChrManImp::RemoveChrIns` again hands a freed `ChrIns` to
    /// `CSDelayDeleteMan` twice.
    #[test]
    fn registration_going_away_after_it_was_there_is_a_self_despawn_not_a_timeout() {
        let mut readiness = Readiness::new(5_000);
        // Frame one: it is there.
        assert_eq!(
            readiness.observe(0, |gate| Some(gate == Gate::Registered)),
            Poll::Waiting(Gate::ChrResLoaded)
        );
        // Frame two: it is not.
        assert_eq!(readiness.observe(16, |_| Some(false)), Poll::Vanished);
        // ...and it stays a vanish even past the deadline, because the pointer is gone either way.
        assert_eq!(readiness.observe(99_999, |_| Some(false)), Poll::Vanished);
    }

    /// Registration that was never there is an ordinary wait, and then an ordinary expiry -- there
    /// is nothing to double-free, and something did go wrong.
    #[test]
    fn registration_that_never_arrived_expires_rather_than_vanishing() {
        let mut readiness = Readiness::new(5_000);
        assert_eq!(
            readiness.observe(0, |_| Some(false)),
            Poll::Waiting(Gate::Registered)
        );
        assert_eq!(
            readiness.observe(5_000, |_| Some(false)),
            Poll::Expired(Gate::Registered)
        );
        assert_eq!(readiness.reached(), None);
    }

    /// The deadline is inclusive at its own value, and one millisecond earlier is still a wait.
    #[test]
    fn the_deadline_expires_at_the_configured_millisecond() {
        let mut readiness = Readiness::new(5_000);
        let stuck = |gate: Gate| Some(gate == Gate::Registered);
        assert_eq!(
            readiness.observe(4_999, stuck),
            Poll::Waiting(Gate::ChrResLoaded)
        );
        assert_eq!(
            readiness.observe(5_000, stuck),
            Poll::Expired(Gate::ChrResLoaded)
        );
    }

    /// An undecidable gate is skipped, not failed: three of the four are byte-proven identical on
    /// both known builds, so a third build gets a working spawn layer with a weaker residency
    /// check rather than no spawn layer.
    #[test]
    fn a_gate_that_cannot_be_decided_on_this_build_is_skipped_not_failed() {
        let mut recorder = Recorder::new([Some(true), Some(true), None, Some(true)]);
        let mut readiness = Readiness::new(5_000);
        assert_eq!(readiness.observe(0, recorder.evaluate()), Poll::Ready);
        assert_eq!(recorder.asked, Gate::ORDER.to_vec(), "it is still asked");
        assert_eq!(
            readiness.reached(),
            Some(Gate::Drivable),
            "and a skipped gate is not a high-water mark of its own"
        );
    }

    /// A skipped gate must not be reported as reached -- the expiry line names the gate the wait
    /// died on, and claiming one that was never evaluated would send the reader after the wrong
    /// cause.
    #[test]
    fn a_skipped_gate_is_never_reported_as_the_furthest_reached() {
        let mut readiness = Readiness::new(5_000);
        let verdict = readiness.observe(0, |gate| match gate {
            Gate::Registered => Some(true),
            Gate::ChrResLoaded => Some(true),
            Gate::AssetsResident => None,
            Gate::Drivable => Some(false),
        });
        assert_eq!(verdict, Poll::Waiting(Gate::Drivable));
        assert_eq!(readiness.reached(), Some(Gate::ChrResLoaded));
    }

    /// The high-water mark only ever moves forward, so a gate that flickers false for one frame
    /// does not rewrite the progress the log has already reported.
    #[test]
    fn the_furthest_gate_reached_never_goes_backwards() {
        let mut readiness = Readiness::new(60_000);
        readiness.observe(0, |_| Some(true));
        assert_eq!(readiness.reached(), Some(Gate::Drivable));
        readiness.observe(16, |gate| Some(gate <= Gate::ChrResLoaded));
        assert_eq!(readiness.reached(), Some(Gate::Drivable));
    }

    /// Every gate has to explain what being stuck on it means, because the gate name is a symptom
    /// and the player needs the cause.
    #[test]
    fn every_gate_names_itself_and_says_what_being_stuck_there_means() {
        let mut names = Vec::new();
        for gate in Gate::ORDER {
            assert!(!gate.name().is_empty());
            assert!(gate.stuck_means().len() > 40, "{}", gate.name());
            names.push(gate.name());
        }
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "{names:?}");
        // The one a bad chr id lands on has to say so in words the player can act on.
        assert!(
            Gate::ChrResLoaded.stuck_means().contains("chrbnd"),
            "{}",
            Gate::ChrResLoaded.stuck_means()
        );
    }

    /// The order the enum declares is the order the array evaluates, so inserting a gate in the
    /// wrong place fails here rather than silently reordering the safety-critical evaluation.
    #[test]
    fn the_declared_order_is_the_evaluated_order() {
        let mut sorted = Gate::ORDER;
        sorted.sort_unstable();
        assert_eq!(sorted, Gate::ORDER);
        assert_eq!(Gate::ORDER[0], Gate::Registered, "the cheapest, and first");
        assert!(
            Gate::ChrResLoaded < Gate::AssetsResident,
            "the EneDat pointer must be established before it is dereferenced"
        );
    }
}
