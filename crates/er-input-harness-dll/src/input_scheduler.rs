//! Semantic input buffering and edge timing for menu automation.
//!
//! This module intentionally buffers *intent* ("confirm this upgrade once it is valid"), not raw
//! button downs. Raw late key replay is unsafe in ER menus because selection/dialog ownership can
//! change between the user-visible prompt and the first frame where the engine will accept OK. The
//! runtime driver supplies per-intent readiness/effect/stale predicates; the scheduler only decides
//! when to emit a clean edge and when to advance/drop the pending intent.

use std::collections::VecDeque;

/// Native modal/dialog OK-readiness gate extracted from the `CS::MessageBoxDialog` OK path.
///
/// The engine can render a dialog before OK is valid. The OK handler commits only when its elapsed
/// fade/settle accumulator has reached the required duration (`dialog+0x2300 >= dialog+0x1278` in
/// the 1.16.x dump/deobf evidence). This helper keeps that predicate explicit and testable so the
/// scheduler can wait instead of replaying raw confirms blindly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DialogAcceptGate {
    pub(crate) required_elapsed: f32,
    pub(crate) elapsed: f32,
}

impl DialogAcceptGate {
    pub(crate) const fn new(required_elapsed: f32, elapsed: f32) -> Self {
        Self {
            required_elapsed,
            elapsed,
        }
    }

    pub(crate) fn is_ready(self) -> bool {
        self.required_elapsed <= self.elapsed
    }
}

/// High-level input the harness intends to perform once its native readiness predicate passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputIntentKind {
    /// Generic modal/dialog affirmative action. For weapon upgrade confirms this must be paired with
    /// a dialog/readiness predicate and a reinforcement/economy effect predicate.
    Confirm,
    /// Generic cancel/back action.
    Cancel,
    /// Move selection in a menu row/list.
    Navigate(NavDirection),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InputIntent {
    pub(crate) kind: InputIntentKind,
    /// Number of consecutive frames to hold the input down once readiness opens.
    pub(crate) hold_frames: u8,
    /// Number of release frames required before another intent may emit.
    pub(crate) release_gap_frames: u8,
    /// Hard cap for an intent that never becomes ready/effective. This is a frame budget, not a
    /// synchronization mechanism; real completion should come from `effect_observed`.
    pub(crate) max_wait_frames: u16,
}

impl InputIntent {
    pub(crate) const fn confirm(
        hold_frames: u8,
        release_gap_frames: u8,
        max_wait_frames: u16,
    ) -> Self {
        Self {
            kind: InputIntentKind::Confirm,
            hold_frames,
            release_gap_frames,
            max_wait_frames,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct IntentObservation {
    /// The native predicate says this action can commit on this frame.
    pub(crate) ready: bool,
    /// The previous emitted edge produced the intended state change.
    pub(crate) effect_observed: bool,
    /// The buffered intent no longer targets the same dialog/row/item/menu state.
    pub(crate) stale: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SchedulerDecision {
    Idle,
    WaitForReady(InputIntentKind),
    Emit(InputIntentKind),
    ReleaseGap(InputIntentKind),
    Completed(InputIntentKind),
    DroppedStale(InputIntentKind),
    TimedOut(InputIntentKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActivePhase {
    WaitingReady,
    Holding { remaining: u8 },
    AwaitingEffect,
    ReleaseGap { remaining: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActiveIntent {
    intent: InputIntent,
    phase: ActivePhase,
    age_frames: u16,
}

#[derive(Debug, Default)]
pub(crate) struct SemanticInputScheduler {
    queue: VecDeque<InputIntent>,
    active: Option<ActiveIntent>,
}

impl SemanticInputScheduler {
    pub(crate) fn push(&mut self, intent: InputIntent) {
        self.queue.push_back(intent);
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.active.is_none() && self.queue.is_empty()
    }

    pub(crate) fn tick(&mut self, observation: IntentObservation) -> SchedulerDecision {
        if self.active.is_none() {
            self.active = self.queue.pop_front().map(|intent| ActiveIntent {
                intent,
                phase: ActivePhase::WaitingReady,
                age_frames: 0,
            });
        }

        let Some(mut active) = self.active else {
            return SchedulerDecision::Idle;
        };

        let kind = active.intent.kind;
        if observation.stale {
            self.active = None;
            return SchedulerDecision::DroppedStale(kind);
        }

        if active.age_frames >= active.intent.max_wait_frames {
            self.active = None;
            return SchedulerDecision::TimedOut(kind);
        }
        active.age_frames = active.age_frames.saturating_add(1);

        match active.phase {
            ActivePhase::WaitingReady => {
                if observation.ready {
                    active.phase = ActivePhase::Holding {
                        remaining: active.intent.hold_frames.saturating_sub(1),
                    };
                    self.active = Some(active);
                    SchedulerDecision::Emit(kind)
                } else {
                    self.active = Some(active);
                    SchedulerDecision::WaitForReady(kind)
                }
            }
            ActivePhase::Holding { remaining } => {
                if remaining == 0 {
                    active.phase = ActivePhase::AwaitingEffect;
                    self.active = Some(active);
                    SchedulerDecision::ReleaseGap(kind)
                } else {
                    active.phase = ActivePhase::Holding {
                        remaining: remaining - 1,
                    };
                    self.active = Some(active);
                    SchedulerDecision::Emit(kind)
                }
            }
            ActivePhase::AwaitingEffect => {
                if observation.effect_observed {
                    if active.intent.release_gap_frames == 0 {
                        self.active = None;
                        SchedulerDecision::Completed(kind)
                    } else {
                        active.phase = ActivePhase::ReleaseGap {
                            remaining: active.intent.release_gap_frames,
                        };
                        self.active = Some(active);
                        SchedulerDecision::ReleaseGap(kind)
                    }
                } else {
                    self.active = Some(active);
                    SchedulerDecision::ReleaseGap(kind)
                }
            }
            ActivePhase::ReleaseGap { remaining } => {
                if remaining <= 1 {
                    self.active = None;
                    SchedulerDecision::Completed(kind)
                } else {
                    active.phase = ActivePhase::ReleaseGap {
                        remaining: remaining - 1,
                    };
                    self.active = Some(active);
                    SchedulerDecision::ReleaseGap(kind)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confirm() -> InputIntent {
        InputIntent::confirm(2, 2, 10)
    }

    #[test]
    fn dialog_accept_gate_waits_for_native_fade_settle() {
        assert!(!DialogAcceptGate::new(0.25, 0.0).is_ready());
        assert!(!DialogAcceptGate::new(0.25, 0.249).is_ready());
        assert!(DialogAcceptGate::new(0.25, 0.25).is_ready());
        assert!(DialogAcceptGate::new(0.25, 0.5).is_ready());
    }

    #[test]
    fn dialog_accept_gate_does_not_treat_nan_as_ready() {
        assert!(!DialogAcceptGate::new(f32::NAN, 1.0).is_ready());
        assert!(!DialogAcceptGate::new(0.25, f32::NAN).is_ready());
    }

    #[test]
    fn names_future_layer_intents_without_raw_key_buffering() {
        let _ = InputIntentKind::Cancel;
        let _ = InputIntentKind::Navigate(NavDirection::Up);
        let _ = InputIntentKind::Navigate(NavDirection::Down);
        let _ = InputIntentKind::Navigate(NavDirection::Left);
        let _ = InputIntentKind::Navigate(NavDirection::Right);
    }

    #[test]
    fn waits_until_ready_before_emitting_edge() {
        let mut scheduler = SemanticInputScheduler::default();
        scheduler.push(confirm());

        assert_eq!(
            scheduler.tick(IntentObservation::default()),
            SchedulerDecision::WaitForReady(InputIntentKind::Confirm)
        );
        assert_eq!(
            scheduler.tick(IntentObservation {
                ready: true,
                ..IntentObservation::default()
            }),
            SchedulerDecision::Emit(InputIntentKind::Confirm)
        );
    }

    #[test]
    fn holds_releases_and_completes_after_effect() {
        let mut scheduler = SemanticInputScheduler::default();
        scheduler.push(confirm());

        assert_eq!(
            scheduler.tick(IntentObservation {
                ready: true,
                ..IntentObservation::default()
            }),
            SchedulerDecision::Emit(InputIntentKind::Confirm)
        );
        assert_eq!(
            scheduler.tick(IntentObservation {
                ready: true,
                ..IntentObservation::default()
            }),
            SchedulerDecision::Emit(InputIntentKind::Confirm)
        );
        assert_eq!(
            scheduler.tick(IntentObservation::default()),
            SchedulerDecision::ReleaseGap(InputIntentKind::Confirm)
        );
        assert_eq!(
            scheduler.tick(IntentObservation {
                effect_observed: true,
                ..IntentObservation::default()
            }),
            SchedulerDecision::ReleaseGap(InputIntentKind::Confirm)
        );
        assert_eq!(
            scheduler.tick(IntentObservation::default()),
            SchedulerDecision::ReleaseGap(InputIntentKind::Confirm)
        );
        assert_eq!(
            scheduler.tick(IntentObservation::default()),
            SchedulerDecision::Completed(InputIntentKind::Confirm)
        );
        assert!(scheduler.is_idle());
    }

    #[test]
    fn drops_stale_intent_without_emitting_late_confirm() {
        let mut scheduler = SemanticInputScheduler::default();
        scheduler.push(confirm());

        assert_eq!(
            scheduler.tick(IntentObservation {
                stale: true,
                ready: true,
                ..IntentObservation::default()
            }),
            SchedulerDecision::DroppedStale(InputIntentKind::Confirm)
        );
        assert!(scheduler.is_idle());
    }

    #[test]
    fn times_out_when_readiness_never_opens() {
        let mut scheduler = SemanticInputScheduler::default();
        scheduler.push(InputIntent::confirm(1, 0, 2));

        assert_eq!(
            scheduler.tick(IntentObservation::default()),
            SchedulerDecision::WaitForReady(InputIntentKind::Confirm)
        );
        assert_eq!(
            scheduler.tick(IntentObservation::default()),
            SchedulerDecision::WaitForReady(InputIntentKind::Confirm)
        );
        assert_eq!(
            scheduler.tick(IntentObservation::default()),
            SchedulerDecision::TimedOut(InputIntentKind::Confirm)
        );
        assert!(scheduler.is_idle());
    }
}
