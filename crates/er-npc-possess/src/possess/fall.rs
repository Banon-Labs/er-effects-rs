//! The fall BOOKKEEPING of a body that is being carried, and the one clamp that makes it safe at
//! every subject size.
//!
//! # The arithmetic the engine actually kills on
//!
//! `CS::CSChrFallModule::GetFallDeathTime` (1.16.2 `0x14044dce0`, the `SUBSS` at `0x14044dd1f`)
//! is one subtraction:
//!
//! ```text
//! fallHeight = physics->lastGroundedPosition.y - CSChrPhysicsModule::GetPosition(physics).y
//! ```
//!
//! and its caller `CSChrFallModule::Update` (`0x14044df00`) evaluates it only on a frame where
//! `physics->standingOnSolidGround` is set and `IsSliding` is false -- i.e. on the frame the body
//! lands -- then charges `hpMax * ratio(fallHeight)` through `CSChrDataModule::ChangeHP` and
//! stamps `serverLogDataTracker->deathType = Fall`.
//!
//! Two facts follow, and this module exists because of the second one.
//!
//! **The invincibility bit does cover it, contrary to what this crate used to say.** The damage
//! block is gated on `FUN_14044e730`, whose tail is byte-read here rather than guessed
//! (`0x14044e79a` is `XOR EAX,EAX; RET`, `0x14044e8f1` is `MOV AL,1; RET`):
//!
//! ```text
//! 14044e866  MOV RCX,RAX ; MOV R9,[RAX] ; CALL [R9+0x1d8]   ; ChrIns::IsImmuneToAttack
//! 14044e873  TEST AL,AL ; JNZ 14044e79a                     ; immune -> return FALSE
//! 14044e88d  CMPL $0x0,0x138(RAX) ; JLE 14044e79a           ; hp <= 0 -> return FALSE
//! 14044e8a5  CALL 0x1403f4510 ; TEST AL,AL ; JNZ 14044e79a  ; IsDead -> return FALSE
//! 14044e8e4  CALL 0x140454be0 ; TEST AL,AL ; JZ  14044e79a  ; material+0x1B clear -> return FALSE
//! ```
//!
//! So a body carrying `chrFlags1c5 & 0x10` cannot be charged for a fall at all. (`0x140454be0` is
//! `MOVZBL 0x1b(RCX),EAX; RET` and `0x140454c00` is `MOV DL -> 0x1b(RCX); RET`: the byte at
//! `CSChrMaterialModule+0x1B` must be set for fall damage to apply, so it enables rather than
//! disables it, whatever the accessor is named.)
//!
//! **Which is exactly why the exposure is at the edges.** The bit is only on while the possession
//! is running. Release takes it off, and the frames after that are ordinary mortal frames in which
//! `lastGroundedPosition` is still whatever the possession left behind. If the body has ended up
//! below the point the co-location was writing -- and it can, because the body keeps its own
//! full-size character proxy and `ChrCtrl::updatePos` runs a complete `CSChrPhysicsModule::doUpdates`
//! immediately after draining our teleport -- then the difference above is a fall the body never
//! took, and it is charged the moment the body next touches ground.
//!
//! # The clamp
//!
//! [`grounded_pin`] writes `lastGroundedPosition` at the co-location target, but never above where
//! the body actually is. That makes `lastGroundedPosition.y - position.y` non-positive by
//! construction, at any subject size, on any frame, through any release path -- including the one
//! where the final `request_move` fails outright and the body is left exactly where the physics
//! put it. It is not a tuned number: it is the same subtraction the engine performs, arranged so
//! its result cannot be a lie.
//!
//! It is also not a lie in the other direction. The field means "the last place this character
//! stood", and a body that has been pushed below the creature it is riding is standing lower than
//! the creature, not higher. Pinning it to the creature's height is the fiction; pinning it to the
//! body's own is the fact.
//!
//! # Why the alarm threshold is read from the character
//!
//! [`drift_alarm_m`] answers "how far below the co-location target does the body have to be before
//! the log should shout about it". The engine's own two candidates are used, in order: the body's
//! `CSChrPhysicsModule+0x104 maxStepHeight` -- the height it can walk up without it counting as
//! anything -- and, when that does not read, the `0.3` at `0x14329e658` that `CSChrFallModule::Update`
//! compares `fallHeight` against before it notifies the manipulator that a landing happened
//! (`COMISS XMM6,[0x14329e658]` at `0x14044e11b`). Both are the game's numbers. Neither came from
//! a creature this mod happened to be possessing when somebody died.

// Pure arithmetic; ungated so its tests run on the host with no game running.
#![cfg_attr(not(windows), allow(dead_code))]

use core::sync::atomic::{AtomicU64, Ordering};

/// The engine's own "that was a landing rather than a step" height, in metres.
///
/// `DAT_14329e658`, read out of `eldenring-deobf.bin` at file offset `0x329e658` as `9a 99 99 3e`
/// = `0.30000001f`. `CSChrFallModule::Update` compares the fall height against it at
/// `0x14044e11b` and, above it, calls the character's manipulator vtable slot `+0x80` -- the
/// landing notification. Used here only as the fallback alarm threshold when the body's own
/// `maxStepHeight` does not read.
pub(crate) const ENGINE_LANDING_STEP_M: f32 = 0.3;

/// The largest step height that is believed rather than discarded, in metres.
///
/// A physics module that has not finished initialising reads as a float rather than as a failure,
/// and a garbage `maxStepHeight` would silence the alarm instead of raising it. The player's own
/// is a fraction of a metre; anything past this is not a step height.
const MAX_BELIEVABLE_STEP_M: f32 = 5.0;

/// Where `lastGroundedPosition` must be written, given where the body is being put and where the
/// body actually is right now.
///
/// `X` and `Z` come from the target -- they are not part of the subtraction the engine performs,
/// and the field is meant to name the point the body is being placed on. `Y` is the smaller of the
/// two, which is the whole safety property: see the module docs.
///
/// `body_y` is `None` when the body's position did not read, which is a real outcome (the module
/// pointer chain can fail mid-teardown). The target is then used unclamped, exactly as before this
/// module existed -- an unreadable body is not a reason to write a wrong number, and there is no
/// third option.
#[must_use]
pub(crate) fn grounded_pin(target: [f32; 3], body_y: Option<f32>) -> [f32; 3] {
    let clamped = match body_y {
        Some(body_y) if body_y.is_finite() && body_y < target[1] => body_y,
        _ => target[1],
    };
    [target[0], clamped, target[2]]
}

/// What `CS::CSChrFallModule::GetFallDeathTime` would answer for these two fields, in metres.
///
/// Positive means the engine believes the character has descended that far since it last stood on
/// something, and will charge for it on the frame it lands. Zero or negative means there is
/// nothing to charge.
#[must_use]
pub(crate) fn fall_charge_m(last_grounded_y: f32, position_y: f32) -> f32 {
    last_grounded_y - position_y
}

/// How far below the co-location target the body has to be before it is worth a log line.
///
/// The body's own `maxStepHeight` when it reads believably, and the engine's landing constant when
/// it does not. Both are the game's own numbers; neither is a clamp tuned to an observed subject.
#[must_use]
pub(crate) fn drift_alarm_m(max_step_height: Option<f32>) -> f32 {
    match max_step_height {
        Some(step) if step.is_finite() && step > 0.0 && step <= MAX_BELIEVABLE_STEP_M => step,
        _ => ENGINE_LANDING_STEP_M,
    }
}

/// How far below the co-location target the body is, when that is worth saying, in metres.
///
/// `None` when the body is at or above the target, when it is below by less than `alarm_m`, or
/// when either number is unreadable. A positive answer is a body the world has pushed down out of
/// the place this engine put it -- the "through the floor" the whole module is about.
#[must_use]
pub(crate) fn drift_below_m(target_y: f32, body_y: Option<f32>, alarm_m: f32) -> Option<f32> {
    let body_y = body_y?;
    if !body_y.is_finite() || !target_y.is_finite() {
        return None;
    }
    let below = target_y - body_y;
    (below > alarm_m).then_some(below)
}

/// One frame's worth of the body's fall bookkeeping, for the log line.
///
/// Every field is an `Option` because every one of them is a separate live read that can fail on a
/// character the game is in the middle of tearing down, and a line that says `None` is worth more
/// than a line that was never written.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FallState {
    /// `CSChrPhysicsModule+0x70 position` -- where the body actually is.
    pub(crate) position: Option<[f32; 3]>,
    /// `+0x150 lastGroundedPosition`, as it reads before this frame's pin.
    pub(crate) last_grounded: Option<[f32; 3]>,
    /// `+0x92 standingOnSolidGround` -- the gate on the engine evaluating a fall at all.
    pub(crate) on_ground: Option<bool>,
    /// `+0x1d0 falling`.
    pub(crate) falling: Option<bool>,
    /// `+0x1d1 isTouchingGround`.
    pub(crate) touching_ground: Option<bool>,
    /// `+0x104 maxStepHeight`, the character's own step height in metres.
    pub(crate) max_step_height: Option<f32>,
    /// `+0x110 capsuleHalfHeight` -- half the height of the collision the body is carrying
    /// around the creature it is wearing. Render scale does not touch it (`ChrCtrl::SetScaleSize`
    /// writes `ChrCtrl+0x2d4/+0x2dc` and `CSChrDataModule+0x54/+0x5c` and returns), so this is the
    /// number that decides whether the body fits where the creature is standing.
    pub(crate) capsule_half_height: Option<f32>,
}

impl FallState {
    /// The engine's own fall charge for this frame, or `None` when either half did not read.
    #[must_use]
    pub(crate) fn charge_m(&self) -> Option<f32> {
        Some(fall_charge_m(self.last_grounded?[1], self.position?[1]))
    }

    /// The body's `Y`, for [`grounded_pin`] and [`drift_below_m`].
    #[must_use]
    pub(crate) fn body_y(&self) -> Option<f32> {
        self.position.map(|position| position[1])
    }
}

/// Rate limiter for the co-location line, counted in calls rather than seconds.
///
/// A frame counter rather than a clock because `scripts/check-no-timeouts.py` bans an `elapsed()`
/// gate outright and is right to: the co-location runs once per frame by construction, so counting
/// calls is counting frames, it cannot drift against the thing being measured, and a stalled frame
/// loop stops the log instead of flooding it.
///
/// Module state rather than a field on the possession, deliberately: there is one possession at a
/// time by construction, and the alternative is a new field in a struct another change is editing.
static COLOCATION_TICKS: AtomicU64 = AtomicU64::new(0);

/// One line per this many co-location writes. Sixty is one a second at sixty frames.
const COLOCATION_LOG_EVERY: u64 = 60;

/// Is this frame's co-location due a routine telemetry line?
///
/// An alarm (see [`drift_below_m`]) is not throttled by this -- a body that has left the floor is
/// worth a line on the frame it happens, and it is rare by definition.
pub(crate) fn colocation_line_due() -> bool {
    COLOCATION_TICKS
        .fetch_add(1, Ordering::Relaxed)
        .is_multiple_of(COLOCATION_LOG_EVERY)
}

/// Start a fresh throttle, so the first frame of every possession says something.
pub(crate) fn reset_colocation_throttle() {
    COLOCATION_TICKS.store(0, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference sizes this mod is expected to span. The tallest thing in the shipped
    /// regulation is `c4450` at 59 m; the smallest subject in the reference log is `c6072` at
    /// 0.70 m, which is the one the bug report named.
    const TINY_SUBJECT_M: f32 = 0.70;
    const HUGE_SUBJECT_M: f32 = 59.0;

    /// The invariant, and it is the whole fix: whatever is pinned, the engine's subtraction can
    /// never come out positive.
    ///
    /// Stated over the full subject range rather than at the one size somebody happened to die at,
    /// because "a clamp tuned to 0.5" is exactly what this must not be.
    #[test]
    fn the_engine_can_never_be_told_the_body_fell() {
        // A body pushed anywhere from a step to the bottom of the world, under a subject of any
        // size, at any target height.
        for target_y in [-120.0_f32, 0.0, 4.81, 120.0] {
            for drop in [0.0_f32, 0.05, TINY_SUBJECT_M, HUGE_SUBJECT_M, 500.0] {
                let body_y = target_y - drop;
                let pinned = grounded_pin([1.0, target_y, 2.0], Some(body_y));
                let charge = fall_charge_m(pinned[1], body_y);
                assert!(
                    charge <= 0.0,
                    "target {target_y} drop {drop} pinned {} charge {charge}",
                    pinned[1],
                );
            }
        }
    }

    /// A body above the target is pinned at the target, not lifted with it.
    ///
    /// This is the ordinary case on a creature walking uphill: our tick pins before the engine
    /// drains the teleport, so the body is still one frame behind and below nothing. Taking the
    /// minimum would be wrong the other way -- it would leave the field naming a point the body is
    /// about to be moved off.
    #[test]
    fn a_body_above_the_target_pins_at_the_target() {
        assert_eq!(grounded_pin([1.0, 4.0, 2.0], Some(9.0)), [1.0, 4.0, 2.0]);
        // ...and after the teleport lands, the charge is exactly zero rather than negative-huge.
        assert_eq!(fall_charge_m(4.0, 4.0), 0.0);
    }

    /// X and Z always come from the target. The subtraction is vertical; the horizontal pair name
    /// the point the body is being placed on, which is the creature's, not wherever physics left
    /// the body.
    #[test]
    fn only_the_vertical_component_is_clamped() {
        let pinned = grounded_pin([12.5, 40.0, -7.25], Some(-30.0));
        assert_eq!(pinned[0], 12.5);
        assert_eq!(pinned[2], -7.25);
        assert_eq!(pinned[1], -30.0);
    }

    /// An UNREADABLE body is not a zero. The pin falls back to the target unchanged, which is what
    /// this crate did before the clamp existed -- refusing to write at all would leave the field
    /// naming an older, higher point, which is the failure the clamp is here to prevent.
    #[test]
    fn an_unreadable_body_position_falls_back_to_the_target() {
        assert_eq!(grounded_pin([1.0, 4.0, 2.0], None), [1.0, 4.0, 2.0]);
        assert_eq!(
            grounded_pin([1.0, 4.0, 2.0], Some(f32::NAN)),
            [1.0, 4.0, 2.0]
        );
        assert_eq!(
            grounded_pin([1.0, 4.0, 2.0], Some(f32::NEG_INFINITY)),
            [1.0, 4.0, 2.0]
        );
    }

    /// The alarm comes off the character, and only falls back to the engine constant when the
    /// character's own number is missing or not believable.
    #[test]
    fn the_alarm_threshold_is_the_bodys_own_step_height() {
        assert!((drift_alarm_m(Some(0.45)) - 0.45).abs() < f32::EPSILON);
        assert!((drift_alarm_m(None) - ENGINE_LANDING_STEP_M).abs() < f32::EPSILON);
        assert!((drift_alarm_m(Some(0.0)) - ENGINE_LANDING_STEP_M).abs() < f32::EPSILON);
        assert!((drift_alarm_m(Some(-1.0)) - ENGINE_LANDING_STEP_M).abs() < f32::EPSILON);
        assert!((drift_alarm_m(Some(f32::NAN)) - ENGINE_LANDING_STEP_M).abs() < f32::EPSILON);
        // A module that has not initialised reads as a float, not as a failure.
        assert!(
            (drift_alarm_m(Some(1.0e9)) - ENGINE_LANDING_STEP_M).abs() < f32::EPSILON,
            "an implausible step height must not silence the alarm"
        );
    }

    /// The alarm fires on a body that has gone through something, and stays quiet for a body that
    /// has merely stepped down -- at both ends of the subject range, since the threshold is a
    /// property of the body and the body is the same 1.5 m player either way.
    #[test]
    fn the_alarm_separates_a_step_from_a_floor() {
        let alarm = drift_alarm_m(Some(0.4));
        assert_eq!(drift_below_m(4.81, Some(4.81), alarm), None);
        assert_eq!(drift_below_m(4.81, Some(4.51), alarm), None, "a step down");
        assert_eq!(drift_below_m(4.81, Some(9.0), alarm), None, "above it");
        // Under the tiniest subject and under the tallest, the answer is about the body.
        let through_the_floor = drift_below_m(4.81, Some(4.81 - 30.0), alarm);
        assert!(through_the_floor.is_some_and(|m| (m - 30.0).abs() < 1e-3));
        let under_a_59m_subject = drift_below_m(HUGE_SUBJECT_M, Some(-1.0), alarm);
        assert!(under_a_59m_subject.is_some_and(|m| (m - 60.0).abs() < 1e-3));
        assert_eq!(drift_below_m(4.81, None, alarm), None);
        assert_eq!(drift_below_m(f32::NAN, Some(0.0), alarm), None);
    }

    /// The size independence, stated against the one module that does vary with subject size.
    ///
    /// [`crate::possess::body_size::scale_for`] stretches the player's body by
    /// `creature hitHeight / 1.5` so the lock-on reticle lands on the creature rather than at its
    /// feet. The reference log's subjects are `c6072` at 0.70 m (0.47x) and `c3510`/`c3750` at
    /// 1.90 m (1.27x); the tallest thing in the shipped regulation is 59 m (about 39x).
    ///
    /// The pin takes no scale argument and must not: `ChrCtrl::SetScaleSize` (`0x1403c8350`) is six
    /// stores into `ChrCtrl+0x2d4/+0x2dc` and `CSChrDataModule+0x54/+0x5c` and a `RET`, and touches
    /// no `CSChrPhysicsModule` field -- so the body's capsule, its grounding test and the
    /// subtraction the fall module makes are identical at every one of those scales. This test
    /// exists to keep that an executable claim rather than a comment, because the bug report this
    /// module answers was filed as a size-dependent one.
    #[test]
    fn the_pin_is_the_same_under_a_rat_and_under_a_fifty_nine_metre_subject() {
        use crate::camera::geometry::PLAYER_HIT_HEIGHT;
        use crate::possess::body_size;

        let mut seen_scales = Vec::new();
        for subject_height in [TINY_SUBJECT_M, 1.0, 1.9, 12.0, HUGE_SUBJECT_M] {
            let scale = body_size::scale_for(subject_height, PLAYER_HIT_HEIGHT)
                .expect("every one of these is a plausible height");
            seen_scales.push(scale);
            // The body is pushed 30 m below the subject, whatever the subject is.
            let target = [0.0, 10.0, 0.0];
            let pinned = grounded_pin(target, Some(target[1] - 30.0));
            assert_eq!(
                pinned,
                [0.0, -20.0, 0.0],
                "subject {subject_height} m (body worn at {scale:.2}x) must pin identically",
            );
            assert!(fall_charge_m(pinned[1], target[1] - 30.0) <= 0.0);
        }
        // ...and the scales really did span the range, so the loop is not vacuous.
        let smallest = seen_scales.iter().copied().fold(f32::MAX, f32::min);
        let largest = seen_scales.iter().copied().fold(f32::MIN, f32::max);
        assert!(smallest < 0.5, "{smallest}");
        assert!(largest > 38.0, "{largest}");
    }

    /// The charge helper is the engine's subtraction and nothing else, in both directions.
    #[test]
    fn the_charge_is_last_grounded_minus_position() {
        assert!((fall_charge_m(40.0, 4.0) - 36.0).abs() < 1e-4);
        assert!((fall_charge_m(4.0, 40.0) + 36.0).abs() < 1e-4);
    }

    /// [`FallState`] reports the same subtraction, and reports nothing when either half is missing.
    #[test]
    fn the_reported_state_carries_the_engines_own_number() {
        let state = FallState {
            position: Some([0.0, 4.0, 0.0]),
            last_grounded: Some([0.0, 40.0, 0.0]),
            ..FallState::default()
        };
        assert!(state.charge_m().is_some_and(|m| (m - 36.0).abs() < 1e-4));
        assert_eq!(state.body_y(), Some(4.0));
        assert_eq!(FallState::default().charge_m(), None);
        assert_eq!(FallState::default().body_y(), None);
    }

    /// The throttle says yes on the first call after a reset and then holds its tongue, so the
    /// first frame of every possession carries a line.
    #[test]
    fn the_throttle_opens_on_the_first_frame_of_a_possession() {
        reset_colocation_throttle();
        assert!(colocation_line_due());
        for _ in 1..COLOCATION_LOG_EVERY {
            assert!(!colocation_line_due());
        }
        assert!(colocation_line_due(), "and again one second later");
        reset_colocation_throttle();
        assert!(colocation_line_due());
    }
}
