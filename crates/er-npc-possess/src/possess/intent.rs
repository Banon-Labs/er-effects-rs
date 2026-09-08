//! Where the possessed creature is told to go, and the four fields that takes.
//!
//! # The one field that decides whether anything happens
//!
//! This module used to emit two writes and the creature did not move. The reason was not the two
//! it emitted; it was the two it did not. `[vt+0x50]` (`FUN_1403d0250`) -- the slot we forward,
//! because it is the one that moves the body -- computes the frame's move vector inside
//!
//! ```text
//! if (aiIns->walkType != 0 && ChrIns::GetMoveType(chr) != 0) { ... wantToMoveTo - riderPos ... }
//! ```
//!
//! and otherwise leaves it `DL_ZERO_VECTOR`. `walkType` is written by `CSAiFunc::MoveTo`, by
//! `ClearMoveRequest`, and by the AI goal bodies -- all of them downstream of the goal selection
//! this crate no-ops at `[vt+0x48]`. So a possessed creature's `walkType` is frozen at whatever
//! the last goal left, which for a fresh spawn is `0`: a perfect `wantToMoveTo`, written every
//! frame, consumed by a branch that never runs.
//!
//! `GetMoveType` is the other half of that gate and is not ours to set -- it comes from the
//! creature's own `NpcParam` row, so a creature the game ships as stationary stays stationary and
//! that is correct. See [`crate::possess::layout::ai_ins`] for the byte proof of all of it.
//!
//! # which way is FORWARD: the sign, and its two proofs
//!
//! `forward = (-sin yaw, 0, -cos yaw)`. The minus signs are not a convention someone picked; they
//! are read out of the binary twice, by routes that share nothing.
//!
//! **Route one -- the loop this module's writes actually feed.** `FUN_1402c9410`, the
//! `turnTarget` dispatch `AiIns::UpdateMovement` ends with, takes the direction to face
//! `d = wantToMoveTo - GetPhysicsPosition()` and stores an `AngleBundle` whose yaw is
//!
//! ```text
//! atan2f(d.x, d.z) + PI          ; 1.16.2 0x1402ca1c4, ADDSS XMM0,[0x1430b2614] = 3.14159274
//! ```
//!
//! into `aiIns+0xc3f0` (`FUN_1402c6850` writes `param_1[0x187e]`, i.e. byte `0xc3f0`). `[vt+0x50]`
//! then forms the frame's turn delta as `aiIns[0xc3f0] - ChrIns::GetOrientation()`. A body that
//! already faces `d` must produce a zero delta, so `GetOrientation().y = atan2(d.x, d.z) + PI`,
//! and inverting that gives `d = (-sin yaw, 0, -cos yaw)`. The `+ PI` is the minus sign.
//!
//! **Route two -- the engine's own "which way is this character facing".** `GetForward` is
//! `CS::ChrCtrl::GetPhysicsOrientation`, which builds the rotation matrix from
//! `qInterpolatedOrientation` and multiplies it by `DL_Z_VECTOR ^ FloatVector4_14329f470` -- the
//! `+Z` basis vector XOR'd with `(-0.0, -0.0, -0.0, -0.0)`, i.e. **negated**. So the character's
//! forward is minus the image of local `+Z`: the models face local `-Z`.
//!
//! # `right` was also wrong, and the note here used to say otherwise
//!
//! An earlier version of this section concluded that `right` did not need changing, deriving
//! `forward x up = (cos yaw, 0, -sin yaw)` and calling that the image of local `+X`. The
//! arithmetic is right and the conclusion is wrong, which is the worst shape a note can have: it
//! reads as proof. `forward x up` is the right-hand-rule construction, and Elden Ring is
//! left-handed (`+X` right, `+Y` up, `+Z` forward), so in this basis that cross product yields
//! left. A model whose nose is local `-Z` -- which the forward proof above establishes -- has its
//! right hand at local `-X`, not `+X`.
//!
//! So `right = (-cos yaw, 0, sin yaw)`, the negation of what shipped between 2026-09-02 and the
//! fix below.
//!
//! **The evidence for this one is the observable, and that is stated rather than dressed up.**
//! There is no `GetRight`/`GetSide` export to pair with `GetForward` -- searched, none exists --
//! and the one binary route that looked promising is ambiguous: `CalcSelfToDirectionPos` sets
//! `AI_DIR_TYPE_L` to `-row1`, which would settle it, except that the same switch sets
//! `AI_DIR_TYPE_F` to `+row3` while `GetPhysicsOrientation` proves the body's forward is `-row3`.
//! Those two cannot both be the body's own frame, so the `DIR_TYPE` frame is rotated by something
//! this note cannot pin, and reading `L` out of it would be a guess wearing a citation.
//!
//! What is not ambiguous: pressing A (`stick.x = -1`) rotated every creature clockwise viewed
//! from above -- toward its own right -- consistently and across creature types. A left input that
//! turns the body right is a sign error on the lateral vector and nothing else. The handedness
//! argument above agrees with that measurement, which is why it is kept; the measurement is what
//! decides it.
//!
//! # The race this module still has to lose safely
//!
//! `[vt+0x50]` runs `AiIns::UpdateMovement` before reading any of this, and that function branches
//! three ways over `wantToMoveTo`:
//!
//! ```text
//! if HasFollowPathMoveTarget(pathData) && <a second path predicate> && !IsArrived()
//!         wantToMoveTo = own physics position          // "stop"
//! else if HasFollowPathMoveTarget(pathData) && !aiIns[0xe990] && !IsArrived()
//!         wantToMoveTo = pathData->target              // the path wins
//! else    wantToMoveTo unchanged                        // OUR value survives
//! ```
//!
//! A possessed creature has no follow-path target -- building one is `FUN_1402c65e0`, the third
//! thing `MoveTo` does and the one this crate does not reproduce -- so both predicates are false
//! and the third branch is the live one. [`IntentWrite`] still emits `pathData->target` as well as
//! `wantToMoveTo`, with the same value, because the two can never disagree when they are one
//! number written twice and it is the branch-two answer for free. Only branch one still escapes
//! us, and no field write reaches it.
//!
//! # `turnTarget` is a steering wheel, in exactly one of its values
//!
//! An earlier version of this note said writing `turnTarget` steers nothing, because the named
//! points an `AiTargetPointType` selects stop being refreshed once goal selection is dead. That is
//! true of every value except `TARGET_SELF`, which is refreshed from a field we write ourselves.
//! `UpdateMovement` ends with `FUN_1402c9410(aiIns, aiIns->turnTarget)`, whose `TARGET_SELF`
//! branch takes `wantToMoveTo - GetPhysicsPosition()` as the direction to face and stores the
//! resulting angles where `[vt+0x50]` differences them against the body's live orientation.
//!
//! So `TARGET_SELF` means "face wherever you have been told to walk". The body still converges at
//! its own `NpcParam` turn rate -- that is the locomotion executor's business and we do not touch
//! it -- but it now converges on something the player chose.
//!
//! # Nothing here writes a velocity
//!
//! The move vector `[vt+0x50]` builds is a normalised direction, transformed by the body's model
//! matrix and handed to `CSChrActionRequestModule` -- the same request module the player's own pad
//! feeds. The behaviour graph turns that into locomotion clips and their root motion moves the
//! body. There is no velocity anywhere on the path, which is why `[movement]` no longer carries a
//! `root_motion_only` switch: it was never a choice.

// Pure math; stays ungated so its tests run on the host.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::possess::layout::ai_ins;

/// How far ahead of the creature the move target is placed, in physics-space units, at full stick
/// deflection and `speed_scale = 1.0`.
///
/// It is a leash, not a speed: the engine walks toward the point and stops on arrival, so a short
/// reach means a creature that keeps arriving and re-departing (visible stutter) and a long one
/// means a creature that keeps running for a while after the stick is released. Eight units is
/// roughly two of the range bands' "close" figure and holds a continuous walk at 60fps with a
/// wide margin.
pub(crate) const REACH: f32 = 8.0;

/// The narrowest squared norm [`yaw_of_quaternion`] will still call a unit quaternion.
///
/// The engine renormalises, so a live quaternion sits within a few ULP of 1.0; this window is
/// wide enough that no legitimate value is ever refused and narrow enough that four floats picked
/// out of freed memory essentially never land inside it.
const QUATERNION_NORM_MIN: f32 = 0.9;
/// The widest squared norm [`yaw_of_quaternion`] will still call a unit quaternion.
const QUATERNION_NORM_MAX: f32 = 1.1;

/// What `[vt+0x50]` multiplies the move vector by for the walk gait, and the reason there are two
/// moving gaits at all.
///
/// `DAT_14329e980`, byte-read out of `eldenring-deobf.bin` as `(0.5, 0.5, 0.5, 0.5)` -- a splatted
/// scalar, applied only when `walkType == 1`. The run gait gets the unit vector unscaled.
pub(crate) const WALK_SPEED_SCALE: f32 = 0.5;

/// How long the horizontal part of the facing vector must be before it counts as a heading.
///
/// One ten-thousandth of a unit vector is 0.006 degrees of tilt away from straight down -- far
/// tighter than any body the game will ever hand us, and far looser than the few ULP a computed
/// nose-down quaternion misses zero by.
const HEADING_MIN_HORIZONTAL: f32 = 1e-4;

/// Stick deflection below this is no input at all.
///
/// Separate from `[movement].turn_deadzone_deg`, which is about how far off-heading counts as
/// asking for a turn. This one is about the hardware: an XInput stick at rest reports a few
/// hundred counts of noise, and without a floor the creature would creep.
pub(crate) const STICK_DEADZONE: f32 = 0.25;

/// Deflection at or below this walks; above it runs.
///
/// The engine offers exactly two moving gaits -- `CSAiFunc::MoveTo` writes `2 - walk`, so the pair
/// is `{WALK_TYPE_WALK, WALK_TYPE_RUN}` and there is no third -- and `[vt+0x50]` scales the move
/// vector down for the walk value only. Half deflection is the split because it is the one place
/// on a stick a player can find without looking, and because a keyboard synthesises full
/// deflection, so WASD runs.
const RUN_DEFLECTION: f32 = 0.5;

/// Full deflection on an XInput thumbstick axis. The negative extreme is `-32768`, which is why
/// the normalisation clamps rather than dividing by the magnitude of whichever end it got.
const XINPUT_AXIS_FULL: f32 = 32767.0;

/// A normalised stick reading: `x` right-positive, `y` forward-positive, magnitude at most 1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Stick {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

impl Stick {
    /// Normalise a raw XInput axis pair, or `None` when it is inside the deadzone.
    ///
    /// Radial rather than per-axis: a per-axis deadzone leaves a cross-shaped dead region, so a
    /// diagonal push just past the corner reads as a pure-axis push and the creature walks at 45
    /// degrees to where the stick is pointing.
    #[must_use]
    pub(crate) fn from_xinput(raw_x: i16, raw_y: i16) -> Option<Self> {
        Self::from_axes(
            f32::from(raw_x) / XINPUT_AXIS_FULL,
            f32::from(raw_y) / XINPUT_AXIS_FULL,
        )
    }

    /// Normalise an already-unit-scaled axis pair, or `None` inside the deadzone.
    #[must_use]
    pub(crate) fn from_axes(x: f32, y: f32) -> Option<Self> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let magnitude = x.hypot(y);
        if magnitude < STICK_DEADZONE {
            return None;
        }
        // Clamp rather than always normalising: a half-pushed stick should mean a half-length
        // request, and only the corners of a square-gated stick exceed 1.
        let scale = if magnitude > 1.0 {
            1.0 / magnitude
        } else {
            1.0
        };
        Some(Self {
            x: x * scale,
            y: y * scale,
        })
    }

    /// How far the stick is pushed, `0.0..=1.0`. Decides walk versus run.
    #[must_use]
    fn magnitude(self) -> f32 {
        self.x.hypot(self.y)
    }

    /// The angle off the body's own forward, in radians, `-PI..=PI`.
    ///
    /// The stick is read creature-relative -- `y` is the body's forward and `x` its right -- so
    /// "how far off-heading is this push" needs no yaw and no world transform; it is the stick's
    /// own angle. That is the number `[movement].turn_deadzone_deg` is expressed in.
    #[must_use]
    fn off_heading(self) -> f32 {
        self.x.atan2(self.y)
    }
}

/// The set of writes one frame of movement intent turns into.
///
/// A value rather than four calls, so "the same number goes to both target fields" and "a frame
/// with no stick writes the stop gait" are things that exist and can be tested rather than a
/// discipline expected of the caller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct IntentWrite {
    /// The physics-space point to walk to. The movement vector the driver stages at
    /// `ComManipulator+0x140` is measured from here, and that staging is what moves the body.
    ///
    /// It is not written to any game field. The engine has its own staging into the same slot,
    /// built from `wantToMoveTo` -- i.e. from [`Self::aim`] -- but only on the frames `walkType`
    /// survives the goal churn, and ours is written every frame. With an aim in force and the
    /// stick pushed off-forward the two disagree; that contest is not new, and the resolution has
    /// not changed.
    pub(crate) target: [f32; 3],
    /// The physics-space point to face. Written to `AiIns.wantToMoveTo` and to
    /// `AiIns.pathData->target`.
    ///
    /// Equal to [`Self::target`] unless [`Self::aiming_at`] has replaced it, which is the whole
    /// of the aim bridge: `[vt+0x50]` differences `aiIns+0xc3f0` against the body's live
    /// orientation outside the `walkType` gate, and `+0xc3f0` is written by `UpdateMovement`'s
    /// closing `FUN_1402c9410(aiIns, turnTarget)` from `wantToMoveTo - GetPhysicsPosition()`. So
    /// this field is the only steering wheel the body has, and it steers with the gait stopped.
    pub(crate) aim: [f32; 3],
    /// `AiIns.walkType`. **the gate**: `0` and the engine builds no move vector at all, whatever
    /// `target` says. `1` walks, `2` runs.
    pub(crate) walk_type: i32,
    /// `AiIns.turnTarget`, always [`TURN_TARGET_SELF`] -- it means "face wherever `wantToMoveTo`
    /// is", and when `walk_type` is the stop the same branch holds the body's current facing
    /// instead, so one constant covers both moving and stopped.
    ///
    /// [`TURN_TARGET_SELF`]: crate::possess::layout::ai_ins::TURN_TARGET_SELF
    pub(crate) turn_target: i32,
}

impl IntentWrite {
    /// Is this frame asking for movement at all?
    #[must_use]
    pub(crate) const fn moving(self) -> bool {
        self.walk_type != ai_ins::WALK_TYPE_STOP
    }

    /// How long the local-frame move vector staged at
    /// [`PENDING_MOVE_VECTOR`](crate::possess::layout::manipulator::PENDING_MOVE_VECTOR) should be.
    ///
    /// The engine writes exactly three lengths and no others: `[vt+0x50]` normalises its direction
    /// to unit length, leaves it alone for the run gait, multiplies it by
    /// [`WALK_SPEED_SCALE`] for the walk gait, and stores `DL_ZERO_VECTOR` when the gate is shut.
    /// Staying inside that set is deliberate -- an analog length would be a value nothing in the
    /// game produces, and the behaviour graph is the thing that would have an opinion about it.
    #[must_use]
    pub(crate) fn gait_scale(self) -> f32 {
        match self.walk_type {
            ai_ins::WALK_TYPE_WALK => WALK_SPEED_SCALE,
            ai_ins::WALK_TYPE_RUN => 1.0,
            _ => 0.0,
        }
    }

    /// The frame that stops the creature where it stands.
    ///
    /// Exactly what `CS::AiIns::ClearMoveRequest` writes -- `walkType = 0` and
    /// `wantToMoveTo = own physics position` -- so releasing the stick leaves the AI in the state
    /// the engine's own "stop" leaves it in rather than in one only this mod produces. Writing the
    /// creature's own position also reads as "arrived", which is what stops a stale target from
    /// walking the body on after the player let go.
    #[must_use]
    pub(crate) const fn stopped(at: [f32; 3]) -> Self {
        Self {
            target: at,
            aim: at,
            walk_type: ai_ins::WALK_TYPE_STOP,
            turn_target: ai_ins::TURN_TARGET_SELF,
        }
    }

    /// Face `point` instead of facing the walk target.
    ///
    /// Movement is untouched: [`Self::target`] and [`Self::gait_scale`] still describe the stick,
    /// and the driver still stages the move vector from them. Only what goes into `wantToMoveTo`
    /// changes, and `wantToMoveTo` is what the body turns toward.
    #[must_use]
    pub(crate) fn aiming_at(self, point: [f32; 3]) -> Self {
        if !point.iter().all(|v| v.is_finite()) {
            return self;
        }
        Self { aim: point, ..self }
    }
}

/// Turn a stick reading into the frame's intent.
///
/// `yaw` is the creature's own heading in radians, from [`yaw_of_quaternion`]. The basis is
/// **`forward = (-sin yaw, 0, -cos yaw)`** and `right = (cos yaw, 0, -sin yaw)`, i.e. yaw 0 faces
/// `-Z` and a quarter turn faces `-X`. See the module note for the two proofs of that minus sign;
/// it was `+` until 2026-09-02 and that is why the creature walked away from where it was pushed.
///
/// Creature-relative rather than camera-relative on purpose, and it costs nothing in feel: the
/// camera is a follow camera behind the possessed body, so its forward and the body's forward
/// agree in the steady state, and the two converge whenever they do not.
///
/// `turn_deadzone_deg` is `[movement].turn_deadzone_deg`: a push closer to straight ahead than
/// this is treated as exactly straight ahead. The engine derives the body's facing from the target
/// this function returns, so without a floor a stick a couple of degrees off centre is a standing
/// request to turn, and the body weaves down a corridor it was asked to walk straight along.
/// Snapping to the heading makes the derived turn delta exactly zero.
#[must_use]
pub(crate) fn intent(
    position: [f32; 3],
    yaw: f32,
    stick: Option<Stick>,
    speed_scale: f32,
    turn_deadzone_deg: f32,
) -> IntentWrite {
    let Some(stick) = stick else {
        return IntentWrite::stopped(position);
    };
    if !yaw.is_finite() || !speed_scale.is_finite() || speed_scale <= 0.0 {
        return IntentWrite::stopped(position);
    }
    // Inside the deadzone the push becomes pure forward at the same magnitude, so the gait below
    // is unaffected and only the heading request is straightened.
    let magnitude = stick.magnitude();
    let deadzone = turn_deadzone_deg.to_radians();
    let stick = if deadzone.is_finite() && stick.off_heading().abs() <= deadzone {
        Stick {
            x: 0.0,
            y: magnitude,
        }
    } else {
        stick
    };
    let (sin, cos) = yaw.sin_cos();
    // forward * stick.y + right * stick.x, with forward = (-sin, -cos) and right = (-cos, sin).
    // Both basis vectors are negated against the naive reading; see the module note for which
    // evidence pins which.
    let dx = (-cos).mul_add(stick.x, -sin * stick.y);
    let dz = sin.mul_add(stick.x, -cos * stick.y);
    let reach = REACH * speed_scale;
    let target = [
        reach.mul_add(dx, position[0]),
        position[1],
        reach.mul_add(dz, position[2]),
    ];
    IntentWrite {
        target,
        aim: target,
        walk_type: if magnitude > RUN_DEFLECTION {
            ai_ins::WALK_TYPE_RUN
        } else {
            ai_ins::WALK_TYPE_WALK
        },
        turn_target: ai_ins::TURN_TARGET_SELF,
    }
}

/// A point `distance` in front of a character facing `yaw`, at the same height.
///
/// The same basis as [`intent`], and that is the whole reason it lives here rather than beside its
/// caller: `forward = (-sin yaw, 0, -cos yaw)`. A second copy of that convention, written from the
/// same description, is exactly how a sign error gets in -- and the failure is a creature spawned
/// behind the player, which reads as "the spawn did nothing". That is not hypothetical: it is what
/// this function did until 2026-09-02, and the user's report of it ("the spawned enemy definitely
/// is behind the player") is what found the sign.
///
/// Height is copied, never offset. A spawn point raised off the player's own footing is a creature
/// dropped from a height, and the ground under it is not known here.
#[must_use]
pub(crate) fn ahead_of(position: [f32; 3], yaw: f32, distance: f32) -> [f32; 3] {
    if !yaw.is_finite() || !distance.is_finite() {
        return position;
    }
    let (sin, cos) = yaw.sin_cos();
    let back = -distance;
    [
        back.mul_add(sin, position[0]),
        position[1],
        back.mul_add(cos, position[2]),
    ]
}

/// How far in front of the creature the camera aim point is placed, in physics-space units.
///
/// A facing target, not a walk target: the engine turns toward `wantToMoveTo` and this distance
/// only has to be long enough that the direction to it is not swamped by the body's own radius.
/// `[vt+0x50]` calls a target closer than `hitRadius * 0.01` "arrived"; thirty units clears that
/// for every creature the game ships.
pub(crate) const AIM_REACH: f32 = 30.0;

/// The farthest a lock-on point may be and still be believed, in physics-space units.
///
/// The same fifty `game::pick_target` uses for target selection, and for the same reason: past it
/// the point is not something the player is fighting.
const LOCK_ON_MAX_DISTANCE: f32 = 50.0;

/// ...and the nearest, so a point collapsed onto the creature's own origin -- which is what an
/// untouched `lockOnTargetPos` reads as -- has no direction and is refused.
const LOCK_ON_MIN_DISTANCE: f32 = 0.1;

/// `cos(60 degrees)`: how far off the camera's own look direction a lock-on point may sit and
/// still be treated as the thing the player is aiming at.
///
/// **This is the freshness test, and it is why no lock-on state has to be read.** Nothing clears
/// `ChrIns+0xd0 lockOnTargetPos` when a lock is dropped, so the field alone cannot say whether it
/// describes now or ten seconds ago -- and the handle the rest of this crate reads for that,
/// `PlayerIns+0x6b0`, is not written during a possession at all: `FUN_140716260` gates that store
/// on `IsMainPlayerIns(subject)`, which is false once `camOverrideChrIns` has made the creature
/// the subject. But lock-on drives the camera, so a live lock point and the camera always agree.
/// A stale one agrees only while the player still happens to be looking that way, in which case
/// using it is right anyway.
const LOCK_ON_AGREEMENT_COS: f32 = 0.5;

/// Where the aim came from, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AimSource {
    /// `ChrIns+0xd0 lockOnTargetPos` on the possessed creature, agreed with the camera.
    LockOn,
    /// The camera's own look direction, which is the answer whenever there is no live lock.
    Camera,
}

impl AimSource {
    /// For the log line.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::LockOn => "lock-on",
            Self::Camera => "camera",
        }
    }
}

/// Where the possessed creature should be pointing this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Aim {
    /// The physics-space point to face. Goes into `AiIns.wantToMoveTo`.
    pub(crate) point: [f32; 3],
    /// The heading that point implies, in the same basis as [`intent`]. For the log only.
    pub(crate) yaw: f32,
    pub(crate) source: AimSource,
}

/// Turn "where the camera looks" and "what the lock-on system last wrote" into one facing target.
///
/// `look_yaw` is `ChrExFollowCam+0x154 anglesEuler.y` (see [`crate::camera::game::look_yaw`]),
/// which the engine computes as `atan2(look.x, look.z)`; `lock_on` is `ChrIns+0xd0
/// lockOnTargetPos` read off the possessed creature, which is a physics-space point because
/// possession has made the creature the lock-on subject. `None` when the camera is unreadable,
/// which leaves the old behaviour -- face wherever you were told to walk -- exactly in place.
///
/// # the sign, which is the one thing here that can be silently wrong
///
/// The camera angle and the character heading are the same angle plus PI, and this is the only
/// place the two meet. `angleOnXZPlane` is `atan2f(v.x, v.z)`, so the look direction is
/// `(sin(look_yaw), 0, cos(look_yaw))`; a body facing `d` has yaw `atan2(-d.x, -d.z)` (see the
/// module note's two proofs); substituting gives `look_yaw + PI`. Both forms are computed below
/// from the one input, so they cannot drift apart.
///
/// # Why an NPC needs this at all
///
/// Because the camera-to-aim path the player gets is gated on being a player, by type. The bullet
/// spawn orientation comes from `FUN_1403fb0b0`, whose first line is
/// `ChrCtrl::GetPhysicsOrientation` -- the body's facing -- and which only adjusts that by the
/// camera when `(*chr->vfptr->IsPlayerIns)(chr)` is true. That is a virtual on the `ChrIns`
/// vtable, false for every `EnemyIns`, and `camOverrideChrIns` cannot move it: the override is a
/// pointer the ~40 `GetMainPlayerIns` consumers follow, not a change of class. So a possessed
/// creature's attacks fly along its body facing, always, and the only way to aim them is to turn
/// the body.
#[must_use]
pub(crate) fn aim(
    position: [f32; 3],
    look_yaw: Option<f32>,
    lock_on: Option<[f32; 3]>,
) -> Option<Aim> {
    if !position.iter().all(|value| value.is_finite()) {
        return None;
    }
    let look_yaw = look_yaw.filter(|yaw| yaw.is_finite())?;
    // The look direction the camera angle came from, as a unit vector in the XZ plane. It is
    // horizontal by construction, so the vertical part of the look never enters the heading --
    // which is what stops a steep camera from shortening the aim toward nothing.
    let (look_x, look_z) = look_yaw.sin_cos();
    if let Some(aim) = lock_on.and_then(|point| lock_on_aim(position, point, look_x, look_z)) {
        return Some(aim);
    }
    // `forward = (-sin yaw, 0, -cos yaw)`, so the body heading for this look is `atan2(-x, -z)`,
    // i.e. `look_yaw + PI`. The same inversion as [`ahead_of`], which turns it back into a point.
    let yaw = (-look_x).atan2(-look_z);
    Some(Aim {
        point: ahead_of(position, yaw, AIM_REACH),
        yaw,
        source: AimSource::Camera,
    })
}

/// The lock-on refinement, or `None` when the point is out of range or disagrees with the camera.
///
/// `look_x`/`look_z` are an already-normalised horizontal look direction.
fn lock_on_aim(position: [f32; 3], point: [f32; 3], look_x: f32, look_z: f32) -> Option<Aim> {
    if !point.iter().all(|value| value.is_finite()) {
        return None;
    }
    let (dx, dz) = (point[0] - position[0], point[2] - position[2]);
    let distance = dx.hypot(dz);
    if !(LOCK_ON_MIN_DISTANCE..=LOCK_ON_MAX_DISTANCE).contains(&distance) {
        return None;
    }
    let agreement = (dx / distance).mul_add(look_x, (dz / distance) * look_z);
    if agreement < LOCK_ON_AGREEMENT_COS {
        return None;
    }
    Some(Aim {
        // Height copied from the creature: `FUN_1402c9410` derives the facing from the X and Z of
        // `wantToMoveTo - physicsPosition` and reads no Y at all, so carrying the target's own
        // height in would be a number with no meaning attached to it.
        point: [point[0], position[1], point[2]],
        yaw: (-dx).atan2(-dz),
        source: AimSource::LockOn,
    })
}

/// The engine's own heading angle for a character, from its orientation QUATERNION.
///
/// # Why this is not a field read
///
/// Because the field that looked like one was not. `CSChrPhysicsModule+0x2d0` is
/// `ChrPhysicsModuleInitData.initialOrientation` -- a spawn-time constant that reads `(0,0,0,0)`
/// on a live character -- and reading it made [`intent`] and [`ahead_of`] operate in a basis that
/// never rotated. See [`crate::possess::layout::chr_physics_module::Q_INTERPOLATED_ORIENTATION`].
///
/// # The formula, and where each half of it comes from
///
/// `CS::CSChrPhysicsModule::GetTargetOrientation` builds a rotation matrix from the quaternion
/// whose rows are the images of the local axes, then hands it to
/// `FloatVector4::EulerFromTransformationMatrix`, whose yaw output is
/// `-atan2(row1.z, row1.x)` (the `XOR` against `FloatVector4_14329f470`, byte-read as
/// `(-0.0, -0.0, -0.0, -0.0)`, is that negation). Writing `row3` for the image of local `+Z`:
///
/// ```text
/// row3 = ( 2(wy + xz), 2(yz - wx), 1 - 2(x^2 + y^2) )
/// ```
///
/// and the heading is `atan2(row3.x, row3.z)`, which for a level character is the same angle
/// `EulerFromTransformationMatrix` reports and which stays the horizontal heading when the body is
/// pitched on a slope.
///
/// Returns `None` for a quaternion that is not finite or not unit-length -- which is the cheapest
/// available proof that the sixteen bytes really are an orientation and not whatever now occupies
/// a freed physics module.
#[must_use]
pub(crate) fn yaw_of_quaternion(q: [f32; 4]) -> Option<f32> {
    let [x, y, z, w] = q;
    if !(x.is_finite() && y.is_finite() && z.is_finite() && w.is_finite()) {
        return None;
    }
    // A unit quaternion, within a tolerance far wider than float error and far narrower than
    // anything a garbage read would land in.
    let norm_squared = w.mul_add(w, z.mul_add(z, x.mul_add(x, y * y)));
    if !(QUATERNION_NORM_MIN..=QUATERNION_NORM_MAX).contains(&norm_squared) {
        return None;
    }
    let row3_x = 2.0 * w.mul_add(y, x * z);
    let row3_z = 2.0f32.mul_add(-x.mul_add(x, y * y), 1.0);
    // A body pitched onto its nose has its local +Z pointing straight down and no horizontal
    // heading at all. `atan2` would answer 0 there -- a silent claim that it faces -Z -- so the
    // horizontal component has to be long enough to have a direction. The threshold is a length
    // and not an equality test because the components are computed, so a nose-down quaternion
    // lands a few ULP off zero rather than on it.
    if row3_x.hypot(row3_z) < HEADING_MIN_HORIZONTAL {
        return None;
    }
    Some(row3_x.atan2(row3_z))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Within a millimetre, which is far below anything the engine can act on.
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    /// [`intent`] with the turn deadzone off.
    ///
    /// Every test that is not about the deadzone uses this, so a widened default can never
    /// silently straighten a push a basis test meant to be off-heading -- which would turn a real
    /// sign error into a passing test.
    fn drive(position: [f32; 3], yaw: f32, stick: Option<Stick>, speed_scale: f32) -> IntentWrite {
        intent(position, yaw, stick, speed_scale, 0.0)
    }

    /// `ChrExFollowCam+0x154` for a camera looking along `-Z`, which is the direction a body at
    /// yaw 0 faces. The field is `atan2(look.x, look.z)`, so `atan2(0, -1)` is PI.
    const LOOK_YAW_MINUS_Z: f32 = core::f32::consts::PI;

    /// The invariant the whole aim bridge rests ON: aiming moves what the body faces and leaves
    /// what it walks toward exactly alone.
    ///
    /// The driver measures the move vector it stages at `ComManipulator+0x140` from
    /// [`IntentWrite::target`], and that vector is the only thing that moves the body. If
    /// `aiming_at` ever touched it, pointing the camera somewhere would drag the creature there --
    /// which is a movement regression wearing an aiming feature's clothes.
    #[test]
    fn aiming_moves_the_facing_target_and_never_the_walk_target() {
        let at = [0.0, 0.0, 0.0];
        let walking = drive(at, 0.0, Stick::from_axes(0.0, 1.0), 1.0);
        let aimed = walking.aiming_at([100.0, 0.0, 100.0]);
        assert_eq!(aimed.target, walking.target, "the walk target is untouched");
        assert_eq!(aimed.walk_type, walking.walk_type, "so is the gait");
        assert!(close(aimed.gait_scale(), walking.gait_scale()));
        assert_eq!(aimed.aim, [100.0, 0.0, 100.0], "only the facing moved");
    }

    /// ...and with nothing aiming, the two are the same point, which is the behaviour every
    /// version of this module before the aim bridge had.
    #[test]
    fn an_unaimed_frame_faces_wherever_it_was_told_to_walk() {
        let at = [3.0, 4.0, 5.0];
        for stick in [
            None,
            Stick::from_axes(0.0, 1.0),
            Stick::from_axes(-1.0, 0.0),
        ] {
            let write = drive(at, 0.7, stick, 1.0);
            assert_eq!(write.aim, write.target, "{write:?}");
        }
        assert_eq!(IntentWrite::stopped(at).aim, at);
    }

    /// A junk aim point is refused rather than written, because `wantToMoveTo` is a
    /// `FloatVector4` the engine subtracts a position from and then feeds to `atan2`.
    #[test]
    fn a_non_finite_aim_point_is_refused_rather_than_written() {
        let write = IntentWrite::stopped([1.0, 2.0, 3.0]);
        assert_eq!(write.aiming_at([f32::NAN, 0.0, 0.0]).aim, write.aim);
        assert_eq!(write.aiming_at([0.0, f32::INFINITY, 0.0]).aim, write.aim);
    }

    /// With no lock, the aim is the camera's own look direction -- and it is expressed in the
    /// same basis the body walks in, so `ahead_of` reproduces the point from the yaw.
    #[test]
    fn the_camera_alone_aims_the_body_where_the_camera_looks() {
        let at = [10.0, 5.0, -20.0];
        let aim = aim(at, Some(LOOK_YAW_MINUS_Z), None).expect("a camera yaw is always a heading");
        assert_eq!(aim.source, AimSource::Camera);
        // A camera looking down -Z asks for body yaw 0, which is what the body already calls -Z.
        assert!(close(aim.yaw, 0.0), "{aim:?}");
        let placed = ahead_of(at, 0.0, AIM_REACH);
        assert!(close(aim.point[0], placed[0]), "{aim:?}");
        assert!(close(aim.point[1], at[1]), "height is copied, not aimed");
        assert!(close(aim.point[2], placed[2]), "{aim:?}");
    }

    /// The sign, pinned at every angle rather than at one.
    ///
    /// The camera field is `atan2(look.x, look.z)` and the body's heading for that same direction
    /// is `atan2(-look.x, -look.z)`; a flipped sign or a swapped pair still passes a single-angle
    /// test (`-Z` is symmetric in x) and puts the creature's back to the camera everywhere else.
    #[test]
    fn the_aim_point_is_always_straight_along_the_cameras_own_look() {
        let at = [1.0, 2.0, 3.0];
        for eighth in 0..8 {
            let camera_yaw = core::f32::consts::FRAC_PI_4 * eighth as f32;
            let aim = aim(at, Some(camera_yaw), None).expect("every angle is a heading");
            // The direction the camera field describes: atan2(x, z) == camera_yaw.
            let (look_x, look_z) = camera_yaw.sin_cos();
            assert!(
                close(aim.point[0], AIM_REACH.mul_add(look_x, at[0])),
                "{eighth}: {aim:?}"
            );
            assert!(
                close(aim.point[2], AIM_REACH.mul_add(look_z, at[2])),
                "{eighth}: {aim:?}"
            );
            // ...and the heading it reports is the one `ahead_of` would reproduce the point from.
            let placed = ahead_of(at, aim.yaw, AIM_REACH);
            assert!(close(placed[0], aim.point[0]), "{eighth}: {aim:?}");
            assert!(close(placed[2], aim.point[2]), "{eighth}: {aim:?}");
        }
    }

    /// No camera means no aim, which leaves the old behaviour in place rather than spinning the
    /// body about an invented yaw. Same for a junk angle or a junk position.
    #[test]
    fn an_unreadable_camera_or_position_produces_no_aim() {
        let at = [0.0; 3];
        assert_eq!(aim(at, None, None), None, "no camera at all");
        assert_eq!(aim(at, Some(f32::NAN), None), None);
        assert_eq!(aim(at, Some(f32::INFINITY), None), None);
        assert_eq!(
            aim([f32::NAN, 0.0, 0.0], Some(LOOK_YAW_MINUS_Z), None),
            None
        );
    }

    /// A lock point the camera agrees with wins, and the aim is at the point rather than at a
    /// fixed distance along the camera -- which is the whole reason to prefer it.
    #[test]
    fn a_lock_point_the_camera_agrees_with_is_the_aim() {
        let at = [0.0, 1.0, 0.0];
        let target = [1.0, 7.0, -9.0];
        let aim = aim(at, Some(LOOK_YAW_MINUS_Z), Some(target)).expect("a heading exists");
        assert_eq!(aim.source, AimSource::LockOn);
        assert!(close(aim.point[0], target[0]), "{aim:?}");
        assert!(close(aim.point[2], target[2]), "{aim:?}");
        assert!(
            close(aim.point[1], at[1]),
            "the target's own height is dropped: the engine reads only x and z"
        );
    }

    /// The freshness test. `lockOnTargetPos` is never cleared, so a point the player has since
    /// turned away from is stale -- and the only evidence available for that, with the lock-on
    /// handle unwritten during a possession, is that it disagrees with the camera.
    #[test]
    fn a_lock_point_the_camera_has_turned_away_from_is_refused() {
        let at = [0.0; 3];
        // Behind the camera entirely.
        let behind = aim(at, Some(LOOK_YAW_MINUS_Z), Some([0.0, 0.0, 9.0])).expect("heading");
        assert_eq!(behind.source, AimSource::Camera, "{behind:?}");
        // Ninety degrees off, which is outside the sixty-degree cone.
        let sideways = aim(at, Some(LOOK_YAW_MINUS_Z), Some([9.0, 0.0, 0.0])).expect("heading");
        assert_eq!(sideways.source, AimSource::Camera, "{sideways:?}");
        // ...and just inside it still wins, so the cone is a cone and not a rejection of
        // everything that is not dead ahead.
        let inside = aim(at, Some(LOOK_YAW_MINUS_Z), Some([-3.0, 0.0, -9.0])).expect("heading");
        assert_eq!(inside.source, AimSource::LockOn, "{inside:?}");
    }

    /// An untouched `lockOnTargetPos` reads as a point with no direction, and one past the
    /// selection range is not something the player is fighting. Both fall back to the camera
    /// rather than producing a facing the body would chase.
    #[test]
    fn a_lock_point_with_no_direction_or_out_of_range_is_refused() {
        let at = [4.0, 0.0, 4.0];
        for point in [at, [4.0, 0.0, 3.95], [4.0, 0.0, -60.0]] {
            let answer = aim(at, Some(LOOK_YAW_MINUS_Z), Some(point)).expect("heading");
            assert_eq!(answer.source, AimSource::Camera, "{point:?} -> {answer:?}");
        }
        let just_inside =
            aim(at, Some(LOOK_YAW_MINUS_Z), Some([4.0, 0.0, -45.0])).expect("heading");
        assert_eq!(just_inside.source, AimSource::LockOn);
    }

    /// The spawn point uses the same basis as the movement target, and in front means in front.
    /// A sign error here spawns the creature behind the player, which is indistinguishable from
    /// the spawn having done nothing.
    #[test]
    fn ahead_of_agrees_with_the_movement_basis_and_never_changes_height() {
        let at = [10.0, 5.0, -20.0];
        // Yaw 0 faces -Z: the models face local -Z and `GetForward` negates the axis.
        let ahead = ahead_of(at, 0.0, 3.0);
        assert!(close(ahead[0], 10.0), "{ahead:?}");
        assert!(close(ahead[1], 5.0), "height is copied, not offset");
        assert!(close(ahead[2], -23.0), "{ahead:?}");
        // A quarter turn faces -X.
        let turned = ahead_of(at, core::f32::consts::FRAC_PI_2, 3.0);
        assert!(close(turned[0], 7.0), "{turned:?}");
        assert!(close(turned[2], -20.0), "{turned:?}");
        // ...and it is the same direction `intent` walks toward on full forward stick.
        let walked = drive(at, 1.0, Stick::from_axes(0.0, 1.0), 1.0);
        let placed = ahead_of(at, 1.0, REACH);
        assert!(close(walked.target[0], placed[0]), "{walked:?} {placed:?}");
        assert!(close(walked.target[2], placed[2]), "{walked:?} {placed:?}");
    }

    /// A junk yaw or distance must place the creature exactly on the player rather than at a NaN,
    /// which the proxy drain would then feed to `ForceSetPosition`.
    #[test]
    fn a_non_finite_input_places_the_creature_on_the_player_rather_than_nowhere() {
        let at = [1.0, 2.0, 3.0];
        assert_eq!(ahead_of(at, f32::NAN, 3.0), at);
        assert_eq!(ahead_of(at, 0.0, f32::INFINITY), at);
    }

    #[test]
    fn a_resting_stick_is_no_input_and_a_pushed_one_is() {
        assert_eq!(Stick::from_xinput(0, 0), None);
        assert_eq!(Stick::from_xinput(3000, 2000), None, "inside the deadzone");
        let pushed = Stick::from_xinput(0, 32767).expect("full forward is input");
        assert!(close(pushed.x, 0.0) && close(pushed.y, 1.0));
    }

    /// A per-axis deadzone would leave a cross-shaped dead region; a diagonal just past the corner
    /// would then read as a pure axis and the creature would walk 45 degrees off.
    #[test]
    fn the_deadzone_is_radial_not_per_axis() {
        // Each axis alone is under the threshold; together they are over it.
        let each = STICK_DEADZONE * 0.8;
        let stick = Stick::from_axes(each, each).expect("the diagonal is outside the deadzone");
        assert!(close(stick.x, stick.y), "and it stays diagonal");
        assert_eq!(Stick::from_axes(each, 0.0), None, "one axis alone is not");
    }

    /// A square-gated stick reads past 1.0 in the corners; the request must not get longer there.
    #[test]
    fn a_corner_deflection_is_clamped_to_unit_length() {
        let corner = Stick::from_axes(1.0, 1.0).expect("a corner is input");
        assert!(close(corner.x.hypot(corner.y), 1.0));
        let half = Stick::from_axes(0.0, 0.5).expect("half forward is input");
        assert!(close(half.y, 0.5), "a half push stays a half push");
    }

    /// NaN out of a bad read must not become a NaN move target.
    #[test]
    fn a_non_finite_axis_is_not_input() {
        assert_eq!(Stick::from_axes(f32::NAN, 0.0), None);
        assert_eq!(Stick::from_axes(0.0, f32::INFINITY), None);
    }

    /// The basis: yaw 0 faces `-Z`, a quarter turn faces `-X`, and Y is never touched -- writing a
    /// height into a walk target is how a ground creature is asked to fly.
    #[test]
    fn forward_is_minus_z_at_yaw_zero_and_minus_x_a_quarter_turn_later() {
        let at = [10.0, 5.0, -20.0];
        let forward = Stick::from_axes(0.0, 1.0);
        let ahead = drive(at, 0.0, forward, 1.0);
        assert!(ahead.moving());
        assert!(close(ahead.target[0], 10.0), "{:?}", ahead.target);
        assert!(close(ahead.target[1], 5.0), "height is never written");
        assert!(close(ahead.target[2], -20.0 - REACH), "{:?}", ahead.target);

        let turned = drive(at, core::f32::consts::FRAC_PI_2, forward, 1.0);
        assert!(close(turned.target[0], 10.0 - REACH), "{:?}", turned.target);
        assert!(close(turned.target[2], -20.0), "{:?}", turned.target);
    }

    /// Right on the stick is right of the body, which is the other half of the basis and the half
    /// a sign error hides in.
    #[test]
    fn right_on_the_stick_is_the_bodys_own_right_which_is_minus_x_at_yaw_zero() {
        let at = [0.0, 0.0, 0.0];
        let right = drive(at, 0.0, Stick::from_axes(1.0, 0.0), 1.0);
        assert!(close(right.target[0], -REACH), "{:?}", right.target);
        assert!(close(right.target[2], 0.0), "{:?}", right.target);
        // ...and left is the other way, which is the whole of the bug this pins: pressing A used
        // to send the body clockwise, toward its own right.
        let left = drive(at, 0.0, Stick::from_axes(-1.0, 0.0), 1.0);
        assert!(close(left.target[0], REACH), "{:?}", left.target);
    }

    /// The observable, as an assertion. At yaw 0 the body faces `-Z`; a left push must send it to
    /// the `+X` side of that heading, which is counter-clockwise viewed from above. This is the
    /// test that fails if anyone re-derives `right` from `forward x up` and trusts the answer.
    #[test]
    fn a_left_push_turns_the_body_counter_clockwise() {
        let at = [0.0, 0.0, 0.0];
        for yaw in [0.0_f32, 1.0, -2.296, 3.0] {
            let left = drive(at, yaw, Stick::from_axes(-1.0, 0.0), 1.0);
            let (sin, cos) = yaw.sin_cos();
            // The cross product of forward with the requested direction, about +Y. Forward is
            // (-sin, -cos); a counter-clockwise turn puts the target on the positive side.
            let cross = (-sin) * left.target[2] - (-cos) * left.target[0];
            assert!(
                cross > 0.0,
                "yaw {yaw} turned the wrong way: {:?}",
                left.target
            );
        }
    }

    /// Releasing the stick must stop the creature. A stale target is a creature that keeps
    /// walking after the player let go, which is the single most alarming failure this can have.
    #[test]
    fn no_stick_asks_for_the_creatures_own_position_so_it_stops() {
        let at = [1.5, 2.5, 3.5];
        let idle = drive(at, 1.0, None, 1.0);
        assert!(!idle.moving());
        assert_eq!(idle.target, at, "arrived, by construction");
    }

    /// `speed_scale` lengthens the leash, and a junk one must not produce a junk target.
    #[test]
    fn speed_scale_scales_the_reach_and_nonsense_stops_the_creature() {
        let at = [0.0, 0.0, 0.0];
        let forward = Stick::from_axes(0.0, 1.0);
        let doubled = drive(at, 0.0, forward, 2.0);
        assert!(close(doubled.target[2], -REACH * 2.0));
        for junk in [0.0, -1.0, f32::NAN] {
            let out = drive(at, 0.0, forward, junk);
            assert!(!out.moving(), "{junk}");
            assert_eq!(out.target, at, "{junk}");
        }
    }

    /// A creature whose yaw could not be read must not be sent somewhere arbitrary.
    #[test]
    fn a_non_finite_yaw_stops_the_creature_rather_than_guessing_a_heading() {
        let at = [4.0, 0.0, 4.0];
        let out = drive(at, f32::NAN, Stick::from_axes(0.0, 1.0), 1.0);
        assert!(!out.moving());
        assert_eq!(out.target, at);
    }

    /// The target must stay a finite point for every stick position around the circle -- this is
    /// the value that gets written into the engine's own path data.
    #[test]
    fn every_heading_produces_a_finite_target() {
        for step in 0..64u8 {
            let yaw = core::f32::consts::TAU * f32::from(step) / 64.0;
            let out = drive([100.0, -3.0, 7.0], yaw, Stick::from_axes(0.6, -0.8), 1.0);
            assert!(out.moving());
            assert!(out.target.iter().all(|v| v.is_finite()), "yaw {yaw}");
            // ...and always exactly one reach away, horizontally.
            let dx = out.target[0] - 100.0;
            let dz = out.target[2] - 7.0;
            assert!(close(dx.hypot(dz), REACH), "yaw {yaw}");
        }
    }

    /// The gate. A frame that is asking for movement must carry a non-zero `walkType`, because
    /// zero is the value the engine reads as "build no move vector at all" -- and a target written
    /// under a zero gait is exactly the bug this module was shipped with: perfect coordinates,
    /// consumed by a branch that never runs.
    #[test]
    fn a_moving_frame_never_carries_the_stop_gait() {
        let at = [0.0, 0.0, 0.0];
        for (x, y) in [(0.0, 1.0), (1.0, 0.0), (0.0, -1.0), (-0.7, 0.7), (0.3, 0.3)] {
            let Some(stick) = Stick::from_axes(x, y) else {
                continue;
            };
            let out = drive(at, 0.0, Some(stick), 1.0);
            assert!(out.moving(), "({x}, {y})");
            assert_ne!(out.walk_type, ai_ins::WALK_TYPE_STOP, "({x}, {y})");
        }
    }

    /// ...and the inverse: every frame that is not moving must carry the stop gait, or the body
    /// keeps walking toward its own position at a non-zero gait after the player let go.
    #[test]
    fn every_non_moving_frame_carries_the_stop_gait_and_the_bodys_own_position() {
        let at = [1.5, 2.5, 3.5];
        let stops = [
            drive(at, 1.0, None, 1.0),
            drive(at, f32::NAN, Stick::from_axes(0.0, 1.0), 1.0),
            drive(at, 0.0, Stick::from_axes(0.0, 1.0), -1.0),
            IntentWrite::stopped(at),
        ];
        for out in stops {
            assert!(!out.moving());
            assert_eq!(out.walk_type, ai_ins::WALK_TYPE_STOP);
            assert_eq!(out.target, at, "arrived, by construction");
        }
    }

    /// Half deflection or less walks, past it runs. Those are the only two moving values the
    /// engine's own `MoveTo` produces, so a third would be a value nothing in the game writes.
    #[test]
    fn a_gentle_push_walks_and_a_hard_one_runs() {
        let at = [0.0, 0.0, 0.0];
        let gait = |x: f32, y: f32| drive(at, 0.0, Stick::from_axes(x, y), 1.0).walk_type;
        assert_eq!(gait(0.0, 0.3), ai_ins::WALK_TYPE_WALK);
        assert_eq!(
            gait(0.0, RUN_DEFLECTION),
            ai_ins::WALK_TYPE_WALK,
            "at, not past"
        );
        assert_eq!(gait(0.0, 1.0), ai_ins::WALK_TYPE_RUN);
        // A corner is clamped to unit length, so it runs rather than reading as 1.41.
        assert_eq!(gait(1.0, 1.0), ai_ins::WALK_TYPE_RUN);
    }

    /// The turn target is the one value that steers, on every frame including the stopped ones --
    /// with the stop gait the engine's own branch holds the body's current facing instead, so the
    /// constant does not have to be conditional.
    #[test]
    fn every_frame_asks_the_body_to_face_where_it_was_told_to_walk() {
        let at = [0.0, 0.0, 0.0];
        for out in [
            drive(at, 0.0, Stick::from_axes(0.0, 1.0), 1.0),
            drive(at, 0.0, None, 1.0),
            IntentWrite::stopped(at),
        ] {
            assert_eq!(out.turn_target, ai_ins::TURN_TARGET_SELF);
        }
    }

    /// The staged vector's length is one of the engine's own three, and it tracks the gait.
    ///
    /// This is the value that now does the moving, so a fourth length here would be a length
    /// nothing in the game produces and the behaviour graph would be the thing with an opinion.
    #[test]
    fn the_staged_move_vector_is_one_of_the_three_lengths_the_engine_writes() {
        let at = [0.0, 0.0, 0.0];
        let run = drive(at, 0.0, Stick::from_axes(0.0, 1.0), 1.0);
        assert_eq!(run.walk_type, ai_ins::WALK_TYPE_RUN);
        assert!(close(run.gait_scale(), 1.0));

        let walk = drive(at, 0.0, Stick::from_axes(0.0, 0.3), 1.0);
        assert_eq!(walk.walk_type, ai_ins::WALK_TYPE_WALK);
        assert!(close(walk.gait_scale(), WALK_SPEED_SCALE));

        // A stopped frame stages nothing, which is what `[vt+0x50]` stages with its gate shut.
        assert!(close(IntentWrite::stopped(at).gait_scale(), 0.0));
        assert!(close(drive(at, 0.0, None, 1.0).gait_scale(), 0.0));

        // ...and the walk really is the engine's half, not a number picked to feel right.
        assert!(close(WALK_SPEED_SCALE, 0.5));
        assert!(walk.gait_scale() < run.gait_scale());
    }

    /// Two angles equal modulo a full turn, within a millirad.
    fn wrapped_close(a: f32, b: f32) -> bool {
        use core::f32::consts::{PI, TAU};
        let mut delta = (a - b) % TAU;
        if delta > PI {
            delta -= TAU;
        }
        if delta < -PI {
            delta += TAU;
        }
        delta.abs() < 1e-3
    }

    /// The basis, pinned to the binary rather than to its own description.
    ///
    /// The tests above assert that yaw 0 faces `-Z`, which is exactly the shape of assertion that
    /// let the wrong sign ship: written from the same prose as the code, it agrees with whatever
    /// the code happens to do. This one is different. It asserts the engine's own equation, read
    /// out of `FUN_1402c9410` (which stores `atan2f(d.x, d.z) + PI` into `aiIns+0xc3f0`) and
    /// `FUN_1403d0250` (which turns the body by `aiIns[0xc3f0] - ChrIns::GetOrientation()`):
    ///
    /// ```text
    /// a body already facing d is a body whose turn delta is zero,
    ///   so  yaw == atan2(d.x, d.z) + PI   for d = the direction it faces.
    /// ```
    ///
    /// A flipped sign in [`ahead_of`] or [`intent`] fails this by exactly PI, and it cannot be
    /// made to pass by editing the prose.
    #[test]
    fn the_forward_direction_satisfies_the_engines_own_facing_equation() {
        use core::f32::consts::PI;
        let origin = [0.0, 0.0, 0.0];
        for step in 0..32u8 {
            let yaw = core::f32::consts::TAU * f32::from(step) / 32.0 - PI;
            // `ahead_of` places a point in the direction the body faces...
            let placed = ahead_of(origin, yaw, 5.0);
            assert!(
                wrapped_close(placed[0].atan2(placed[2]) + PI, yaw),
                "ahead_of at yaw {yaw}: {placed:?}"
            );
            // ...and a full-forward stick walks toward that same direction.
            let walked = drive(origin, yaw, Stick::from_axes(0.0, 1.0), 1.0);
            assert!(
                wrapped_close(walked.target[0].atan2(walked.target[2]) + PI, yaw),
                "intent at yaw {yaw}: {:?}",
                walked.target
            );
        }
    }

    /// `yaw_of_quaternion` must invert the rotation the engine builds from a heading.
    ///
    /// A yaw of `psi` about `+Y` is the quaternion `(0, sin(psi/2), 0, cos(psi/2))`, and
    /// `CSChrPhysicsModule::GetTargetOrientation` into `EulerFromTransformationMatrix` reports
    /// `psi` back for it. Anything else here and every heading in the game is wrong by that much.
    #[test]
    fn a_pure_yaw_quaternion_decodes_to_the_yaw_it_was_built_from() {
        use core::f32::consts::PI;
        for step in 0..32u8 {
            let psi = core::f32::consts::TAU * f32::from(step) / 32.0 - PI;
            let (half_sin, half_cos) = (psi * 0.5).sin_cos();
            let decoded =
                yaw_of_quaternion([0.0, half_sin, 0.0, half_cos]).expect("a unit quaternion");
            assert!(wrapped_close(decoded, psi), "{psi} decoded to {decoded}");
        }
        // The identity quaternion is heading zero, not a refusal.
        assert_eq!(yaw_of_quaternion([0.0, 0.0, 0.0, 1.0]), Some(0.0));
    }

    /// The decoded heading and the forward direction must agree with `GetForward`, whose answer is
    /// minus the image of local `+Z` under the rotation -- computed here straight from the
    /// quaternion, so the two halves of the basis check each other rather than the prose.
    #[test]
    fn the_decoded_heading_points_where_getforward_points() {
        // A yaw, and a yaw with a pitch on top of it: the heading must stay horizontal.
        for q in [
            [0.0f32, 0.382_683_4, 0.0, 0.923_879_5],
            [0.130_526_2, 0.353_553_4, -0.146_446_6, 0.914_527_3],
        ] {
            let [x, y, z, w] = q;
            // row3 = the image of local +Z; forward is its negation.
            let row3_x = 2.0 * w.mul_add(y, x * z);
            let row3_z = 2.0f32.mul_add(-x.mul_add(x, y * y), 1.0);
            let (forward_x, forward_z) = (-row3_x, -row3_z);

            let yaw = yaw_of_quaternion(q).expect("a unit quaternion");
            let placed = ahead_of([0.0, 0.0, 0.0], yaw, 1.0);
            let scale = forward_x.hypot(forward_z);
            assert!(close(placed[0], forward_x / scale), "{q:?} gave {placed:?}");
            assert!(close(placed[2], forward_z / scale), "{q:?} gave {placed:?}");
        }
    }

    /// The zero QUATERNION must be a refusal. `(0,0,0,0)` is exactly what
    /// `CSChrPhysicsModule+0x2d0` held on a live character -- the field this crate used to read as
    /// a heading -- and the point of answering `None` is that a dead field can no longer
    /// masquerade as "facing world zero".
    #[test]
    fn only_a_unit_quaternion_is_a_heading() {
        assert_eq!(
            yaw_of_quaternion([0.0, 0.0, 0.0, 0.0]),
            None,
            "the dead field"
        );
        assert!(yaw_of_quaternion([0.5, 0.5, 0.5, 0.5]).is_some());
        assert_eq!(yaw_of_quaternion([3.0, 0.0, 0.0, 0.0]), None, "not unit");
        assert_eq!(yaw_of_quaternion([f32::NAN, 0.0, 0.0, 1.0]), None);
        assert_eq!(yaw_of_quaternion([0.0, 0.0, f32::INFINITY, 0.0]), None);
        // A quarter turn about X stands the body on its nose: the image of local +Z is straight
        // down, there is no horizontal heading at all, and answering `0.0` would be a silent claim
        // that it faces -Z.
        let quarter = core::f32::consts::FRAC_1_SQRT_2;
        assert_eq!(yaw_of_quaternion([quarter, 0.0, 0.0, quarter]), None);
        // A half turn about X is upside down but still pointing somewhere horizontal -- forward is
        // +Z, i.e. yaw PI -- so that one is an answer and not a refusal.
        let upside_down = yaw_of_quaternion([1.0, 0.0, 0.0, 0.0]).expect("still has a heading");
        assert!(wrapped_close(upside_down, core::f32::consts::PI));
    }

    /// `turn_deadzone_deg` straightens a nearly-forward push into an exactly-forward one, so the
    /// heading the engine derives from the target is the heading the body already has and it walks
    /// straight instead of weaving.
    #[test]
    fn a_push_inside_the_turn_deadzone_asks_for_no_turn_at_all() {
        let at = [0.0, 0.0, 0.0];
        // Ten degrees off forward, with the deadzone at twenty.
        let off = 10.0_f32.to_radians();
        let nudged = Stick::from_axes(off.sin(), off.cos());
        let straightened = intent(at, 0.0, nudged, 1.0, 20.0);
        // Yaw 0 faces -Z, so "no turn asked for" is a target with no X component at all.
        assert!(close(straightened.target[0], 0.0), "{straightened:?}");
        assert!(close(straightened.target[2], -REACH), "{straightened:?}");
        // ...and the same push outside the deadzone keeps its angle. Negative, because a push
        // to the body's right is toward `-X` at yaw 0.
        let kept = intent(at, 0.0, nudged, 1.0, 5.0);
        assert!(kept.target[0] < -0.1, "{kept:?}");
    }

    /// Straightening must not change how hard the stick was pushed -- a walk that became a run
    /// (or the reverse) on the way through the deadzone would be a gait that depends on aim.
    #[test]
    fn the_turn_deadzone_changes_the_heading_and_not_the_gait_or_the_reach() {
        let at = [0.0, 0.0, 0.0];
        let off = 5.0_f32.to_radians();
        for magnitude in [0.3_f32, 0.5, 0.9, 1.0] {
            let stick = Stick::from_axes(off.sin() * magnitude, off.cos() * magnitude);
            let free = intent(at, 0.0, stick, 1.0, 0.0);
            let snapped = intent(at, 0.0, stick, 1.0, 20.0);
            assert_eq!(free.walk_type, snapped.walk_type, "{magnitude}");
            let reach = |w: IntentWrite| w.target[0].hypot(w.target[2]);
            assert!(close(reach(free), reach(snapped)), "{magnitude}");
        }
    }

    /// A junk deadzone must not straighten everything or panic; `NaN` comparisons are false, so
    /// the push simply keeps its angle.
    #[test]
    fn a_non_finite_turn_deadzone_leaves_the_push_alone() {
        let at = [0.0, 0.0, 0.0];
        let right = Stick::from_axes(1.0, 0.0);
        for junk in [f32::NAN, f32::INFINITY, -1.0] {
            let out = intent(at, 0.0, right, 1.0, junk);
            assert!(out.moving(), "{junk}");
            assert!(close(out.target[0], -REACH), "{junk} {out:?}");
        }
    }
}
