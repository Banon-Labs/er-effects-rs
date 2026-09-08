//! The size LAW: one creature height in, one camera row out. Pure arithmetic, no game.
//!
//! # What the law is for, stated as a measurement
//!
//! Possessing something six times your size used to crop its head off the top of the screen. The
//! user reported the vanilla composition they wanted in the same breath: *"when I'm in my normal
//! form, I have at least a full character's height between my head and the top of the screen"*.
//! That is not a matter of taste, it is a ratio, and it can be computed from the shipped params:
//!
//! * `LockCamParam` row 0 puts the camera **3.8 m** from a pivot **1.45 m** up a **1.5 m** body,
//!   with a vertical field of view of **48 degrees**.
//! * A 48-degree vertical FOV spans `tan(24 deg) = 0.4452` of the depth either side of the axis,
//!   so at 3.8 m the frame is 1.6919 m tall above centre.
//! * The head sits 1.5 - 1.45 = 0.05 m above the aim point, i.e. **2.96%** of the way from the
//!   centre of the screen to the top.
//! * So the gap from the head to the top edge is `(1 - 0.0296) * 1.6919 = 1.642 m` -- **1.09 times
//!   the subject's own height.**
//!
//! "At least a full character's height" is 1.09 character-heights. The user's eyes and the
//! binary's projection matrix agree to two significant figures, which is what makes this module an
//! arithmetic problem rather than a tuning problem.
//!
//! # The law
//!
//! Hold that composition for a subject of height `H`. Two conditions, and they have one solution:
//!
//! 1. **The head lands in the same place on screen.** The head is `H - pivot` above the aim point
//!    at distance `distance`, and it projects to the same screen position when that ratio is the
//!    player's: `(H - pivot) / distance == (1.5 - 1.45) / 3.8`. Solve for the pivot ->
//!    [`Shape::pivot_height`].
//! 2. **The headroom is the same fraction of the subject.** The frame's half-height at the subject
//!    is `distance * tan(fov/2)`, so headroom-as-a-fraction-of-`H` is held only when
//!    `distance` scales with `H`: `distance = 3.8 * H / 1.5`.
//!
//! Together they are just similarity: scale the whole shot by `H / 1.5`, which puts the pivot at
//! `1.45 * H / 1.5` and reproduces every angle in the frame. Two consequences worth stating
//! because they are what make this robust rather than fitted:
//!
//! * **The FOV does not appear in the answer.** It was needed to learn *what* the vanilla framing
//!   is (1.09 heights of headroom); it is not needed to *hold* it. The patched row copies
//!   `camFovY` from the base row untouched, so whatever the FOV is, the framing follows.
//! * **The camera's pitch does not appear either.** Angles are scale-invariant, so the shot is the
//!   player's at every pitch the player can reach -- level, orbiting overhead, or looking up from
//!   below. That is why this module writes no pitch limit; see "What it deliberately does not do".
//!
//! Condition 1 is written as the pivot solve rather than as `1.45 * H / 1.5` on purpose. The two
//! agree exactly whenever the distance follows condition 2, but the distance can be pushed off it
//! -- by the clearance floor, by the configured ceiling, by a per-creature `camera_distance_scale`
//! -- and solving the pivot against the distance that will actually be used keeps the head on
//! screen anyway. It degrades to "the framing is wider than the player's" instead of to "the head
//! is cropped".
//!
//! # Where every anchor number comes from
//!
//! Each is a value read out of the shipped `regulation.bin` or proven in the binary, and each is
//! named with its source so the next reader can check it rather than trust this comment:
//!
//! | constant | value | source |
//! |---|---|---|
//! | [`PLAYER_HIT_HEIGHT`] | 1.5 | `NpcParam` row 0 (`c0000`, the player) `hitHeight` |
//! | [`PLAYER_CAM_DIST`] | 3.8 | `LockCamParam` row 0 `camDistTarget` |
//! | [`PLAYER_PIVOT`] | 1.45 | `LockCamParam` row 0 `chrOrgOffset_Y` |
//! | [`PLAYER_FOV_Y_DEG`] | 48.0 | `LockCamParam` row 0 `camFovY` |
//!
//! Read them back with `python3 scripts/er-param-read.py LockCamParam --row 0`. That
//! `camFovY` is a vertical angle in degrees is proven, not assumed:
//! `CS::ChrExFollowCam::ApplyZoomLerp` (1.16.2 `0x1403b7560`) multiplies it by
//! `GLOBAL_DegreeToRadian` into `CSCam.fov`, and `CS::CSPersCam::ToPerspective` (`0x1403e9ac0`)
//! builds the projection as `m11 = cot(fov/2)` with `m00 = cot(fov/2) / aspectRatio` -- the extra
//! division by the aspect ratio on X is exactly what makes the stored angle the vertical one.
//!
//! # The dataset that looks like it should have decided this, and why it did not
//!
//! Joining every `NpcParam` row that names a `lockCameraParamId` onto that `LockCamParam` row
//! gives 155 hand-tuned (creature height -> camera) pairs, and fitting them yields
//! `camDistTarget ~ 3.75 * H^0.10` with `chrOrgOffset_Y` almost flat at 1.3-2.15 m across a range
//! running from 0.6 m to 42 m. Reading that as the size law would be a category error.
//!
//! Those rows describe **a 1.5 m player fighting an H-metre target**, not an H-metre subject.
//! `ApplyZoomLerp` applies `chrOrgOffset_Y` to the camera's subject, and the subject in vanilla is
//! always the player -- which is exactly why the offset stays at human chest height however big
//! the thing being fought is. The row is *selected* by the target and *applied* to the subject.
//! Possession swaps the subject, so the selection side of that dataset says nothing about what we
//! need. The same objection retires the other thing that dataset appears to show, that
//! `rotRangeMinX` relaxes with size: it relaxes for a player fighting a giant, which is a
//! statement about where the player's camera may orbit, not about wearing the giant.
//!
//! What it does confirm and this module follows: `camFovY` does not move with size at all (48-50
//! across the entire range), which is why the FOV is copied from the base row and never written.
//!
//! # Checking it, over the creatures that exist rather than two picked by hand
//!
//! `scripts/er-possess-camera-framing.py` runs this law over every possessable creature's real
//! `NpcParam.hitHeight` and prints where the head lands. Over the 405 creatures in
//! `data/moveset.tbl` with a usable height (0.30 m to 59.00 m, median 2.00 m) the head sits at
//! `+0.0296` half-frames and the headroom is `1.0946` subject-heights -- the player's numbers, for
//! every one of them, at every pitch. The two exceptions are `c5472`/`c6082` Great Dragonfly,
//! 0.8 m tall and 2.0 m wide, where the clearance floor pushes the camera further out than the law
//! asks and the headroom grows to 1.35 heights. Wider is a safe direction; cropped is not.
//! [`tests::the_framing_is_the_players_for_every_size_the_game_ships`] pins that in Rust.
//!
//! # What it deliberately does not do
//!
//! * **It writes no pitch limit.** `rotRangeMinX` used to be lerped from -40 to -15 degrees as the
//!   subject grew, on the theory that a tall creature needs the camera to swing higher. It does
//!   not do that. The sign was measured: `ChrExFollowCam.anglesEuler.x` comes from
//!   `angleFromXZPlane` (`0x1403b0b70`), which returns `atan2(v.y, |v.xz|)` NEGATED for `v` the
//!   camera-to-target vector -- so **positive means the camera is above the subject looking
//!   down**. `applyControlMovement` (`0x1403b7be0`) clamps that angle below by `+0x258`, which is
//!   where `ApplyZoomLerp` writes `rotRangeMinX`, and above by `+0x25c`, which the constructor
//!   fixes at `0x3f9c61aa` = 1.2217 rad = **+70 degrees** and nothing else ever writes (all three
//!   fields, the same constants and the same shape, are present on the installed 1.17 build at
//!   `+0x10`: `uv run --with capstone python3 scripts/scan-struct-field-access.py --image
//!   eldenring-deobf-1.17.bin --range 0x1403b0000-0x1403c0000 --disp 0x258,0x25c,0x2d4`). So
//!   `rotRangeMinX` is the limit on how far below the subject the camera may drop to look up at
//!   it, and raising it from -40 to -15 took away a shot without buying a single degree of
//!   overhead. The lever for overhead is `+0x25c`, it is a field of the camera rather than of the
//!   row, and it needs no change because the framing is pitch-invariant. Leaving `rotRangeMinX`
//!   alone also means a map region that narrowed the pitch range keeps its narrowing through a
//!   possession.
//! * **It does not chase the model, only the physics capsule.** `hitHeight` is the capsule, and a
//!   creature whose model is proportionally taller than its capsule gets proportionally less
//!   headroom. It was worth measuring how badly, so it was measured: the FLVER bounding box of
//!   every chr in the game, against that chr's `NpcParam.hitHeight`, 400 pairs
//!   (`scripts/chr-flver-bbox-census.py` + `scripts/chr-hitheight-bbox-ratio.py`). The ratio runs
//!   0.24 to 12.4 with p10 0.89 and p90 2.08 -- real scatter -- but it does **not trend with
//!   size**: the median is 1.15/1.20/1.17/1.26/1.06/1.18 across the 0-1/1-2/2-4/4-8/8-15/15+ metre
//!   buckets and the rank correlation between height and ratio is **-0.03**. That is the shape
//!   that matters here. A size-independent scatter cancels out of a ratio law's slope, so the
//!   framing does not degrade as creatures get bigger -- which is the complaint this module was
//!   written for; what is left is individual creatures off by a roughly constant factor, and a
//!   per-creature constant is exactly what `[chr.cNNNN].camera_distance_scale` is. (The bbox is
//!   itself a loose proxy -- it is the bind-pose mesh extent, wings and props included, which is
//!   why `c3670` reads 12.4 -- so that scatter is an upper bound on the real mis-framing.)
//!
//!   There is also no better scalar available. ELDEN RING has no chr-scale param in any of its 179
//!   paramdefs, and the bbox is degenerate (`FLT_MAX`, `meshCount = 0`) on eleven chrs *including
//!   `c0000`* -- so the player, the one body the whole law is anchored on, has no readable model
//!   height and there is no ratio to anchor. `hitHeight` is also what the engine's own lock-on
//!   manager uses for its search-origin height, i.e. it is the game's own answer to "how tall is
//!   this".
//! * **The mount blend.** `ApplyZoomLerp`'s second branch, taken when `ChrExFollowCam+0x488` is
//!   set, adds a constructor-cached delta to everything derived here and then clamps the pivot to
//!   10 m. A 59 m subject wants a 57 m pivot, so possessing a giant while mounted will be clamped
//!   into a wrong shot. Nobody has measured how it looks; the alternative is fighting the game's
//!   own Torrent zoom, which is worse.

// Pure arithmetic; ungated so `cargo test` proves it on the host with no game running.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::settings::CameraSettings;

/// `NpcParam` row 0 (`c0000`) `hitHeight` -- the player's physics capsule, in metres. Every
/// creature's size is expressed as a multiple of this.
pub(crate) const PLAYER_HIT_HEIGHT: f32 = 1.5;
/// `LockCamParam` row 0 `camDistTarget`.
pub(crate) const PLAYER_CAM_DIST: f32 = 3.8;
/// `LockCamParam` row 0 `chrOrgOffset_Y` -- the camera pivot, just below head height on a 1.5 m
/// body.
pub(crate) const PLAYER_PIVOT: f32 = 1.45;
/// `LockCamParam` row 0 `camFovY`, the vertical field of view in degrees. See the module docs for
/// the two functions that prove both halves of that sentence.
///
/// It is never written -- the patched row copies it from the base row -- and it is not an input to
/// the size law. It is here because it is what turns the law's output into a statement about the
/// screen, which is the only place the user can see whether the law is right.
pub(crate) const PLAYER_FOV_Y_DEG: f32 = 48.0;

/// How far the player's head sits above their camera's aim point: 0.05 m.
///
/// Spelled as the subtraction rather than as `0.05` so it cannot drift away from the two row-0
/// values it is the difference of -- and because the subtraction is exact in `f32`, which is what
/// lets [`shape`] reproduce `chrOrgOffset_Y = 1.45` to the bit at player size.
const PLAYER_HEAD_ABOVE_PIVOT: f32 = PLAYER_HIT_HEIGHT - PLAYER_PIVOT;

/// How much further out than the creature's own physics radius the camera must sit. The radius is
/// `CSChrPhysicsModule+0x344`, the horizontal half-extent of the capsule the world collides with,
/// so anything at or inside it is inside the model.
pub(crate) const RADIUS_CLEARANCE: f32 = 1.25;
/// Nothing is framed from closer than this, whatever the per-chr scale says.
pub(crate) const MIN_DISTANCE: f32 = 0.5;
/// Heights above this are a broken or modded row rather than a creature -- the tallest thing the
/// shipped regulation contains is 59 m (`c4450`, the Walking Mausoleum). Clamped rather than
/// refused, because a possession that works with an odd camera beats one that refuses over a
/// number nobody will ever see.
pub(crate) const MAX_PLAUSIBLE_HEIGHT: f32 = 200.0;

/// The shipped `[camera].distance_max` default: at or above the distance the law asks for at
/// [`MAX_PLAUSIBLE_HEIGHT`], and therefore above every distance it can ever ask for.
///
/// A ceiling is only a guard if it cannot fire on a real subject, and the previous ones could: at
/// 40 m it cropped everything above 3.8 m tall, and even at 120 m it cropped the 59 m Walking
/// Mausoleum, whose framing distance is 149.5 m. Nonsense heights are already handled one step
/// earlier and better, by clamping the height -- which keeps the distance and the pivot consistent
/// with each other, where clamping only the distance breaks the composition. So this is derived
/// from that clamp instead of guessed, and is by construction unable to crop any height the law
/// will accept. It remains a real knob for anyone who wants their camera closer.
///
/// The exact derived value is `3.8 * 200 / 1.5 = 506.67`, rounded up to the next power of two so
/// that the shipped `er-npc-possess.toml` can spell it as a decimal literal that parses back to
/// the same `f32` -- the round trip a config file has to survive. [`tests::the_shipped_ceiling_
/// cannot_crop_the_tallest_creature_that_exists`] pins that it is still above the derivation.
pub(crate) const MAX_FRAMING_DISTANCE: f32 = 512.0;

/// The two `LockCamParam` fields the size law decides.
///
/// Everything else in the row -- FOV, the pitch minimum, the lock vertical offset, the chase rate,
/// the lock-on radii -- is copied from the base row untouched; see [`crate::camera`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shape {
    /// `camDistTarget`, metres.
    pub(crate) distance: f32,
    /// `chrOrgOffset_Y`, metres above the character's origin.
    pub(crate) pivot_height: f32,
}

/// Where a [`Shape`] actually puts the subject on screen. The law's output, restated in the terms
/// the complaint was made in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Framing {
    /// The top of the subject, in half-screen-heights above the centre of the screen. `0.0` is
    /// dead centre, `1.0` is the top edge, and anything above `1.0` is cropped. The player's own
    /// body sits at `+0.0296`.
    pub(crate) head_screen_y: f32,
    /// The gap from the top of the subject to the top edge of the frame, measured in subject
    /// heights. The player gets `1.0946` -- the "full character's height" of the complaint.
    pub(crate) headroom_heights: f32,
}

/// Why a possession is being framed with the vanilla camera instead of an adapted one.
///
/// Every one of these is an ordinary outcome rather than an error: the feature is off, the build
/// is one nobody measured, the creature has no readable size, or the row the player picked is not
/// free. All of them end with the camera behaving exactly as it did before this layer existed,
/// and all of them are named in `er-npc-possess.derived.toml` so "my camera did not change" has an
/// answer that is not "read the source".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// `[camera].enabled = false`.
    Disabled,
    /// The running build is neither 1.16.2 nor 1.17, so no offset table applies.
    UnmeasuredBuild,
    /// `CSChrPhysicsModule+0x340` did not read, or read as zero/NaN.
    NoHeight,
    /// `SoloParamRepository` is not up, or its `LockCamParam` / `NpcParam` holder has not
    /// streamed in yet.
    ParamsNotReady,
    /// A `NpcParam.lockCameraParamId` or `RideParam.rideCamParamId` in the live regulation names
    /// the row `[camera].param_row` picked, so patching it would change some other character's
    /// camera. Carries that row.
    RowInUse(u32),
    /// `[camera].param_row` is not a row of `LockCamParam` at all. A missing id makes
    /// `LookupLockCamParam` return NULL and `ApplyZoomLerp` do nothing whatsoever -- the camera
    /// freezes at its current values with no crash -- so this must be caught rather than written.
    RowMissing(u32),
    /// `LockCamParam` row 0 is gone, so there is no vanilla base to copy the untouched fields from.
    BaseRowMissing,
    /// `WorldChrMan+0x1ece0 chrCam` or `ChrCam+0x60 chrExFollowCam` did not resolve.
    NoFollowCam,
    /// `ChrExFollowCam+0x468` would not take the write.
    WriteFailed,
}

impl Refusal {
    /// One line for the log and for the derived file.
    pub(crate) fn describe(self) -> String {
        match self {
            Self::Disabled => "[camera].enabled is false".to_owned(),
            Self::UnmeasuredBuild => {
                "this build is neither 1.16.2 nor 1.17, and the camera offsets were only measured \
                 on those two"
                    .to_owned()
            }
            Self::NoHeight => {
                "the creature's CSChrPhysicsModule+0x340 hitHeight did not read as a positive \
                 number"
                    .to_owned()
            }
            Self::ParamsNotReady => "SoloParamRepository has not streamed the params in".to_owned(),
            Self::RowInUse(row) => format!(
                "LockCamParam row {row} is referenced by this regulation, so patching it would \
                 move some other character's camera -- pick a free row with [camera].param_row"
            ),
            Self::RowMissing(row) => format!(
                "LockCamParam has no row {row}, and a missing id makes the engine's ApplyZoomLerp \
                 do nothing at all rather than fall back"
            ),
            Self::BaseRowMissing => {
                "LockCamParam row 0, the player's own camera row this one is derived from, is \
                 missing"
                    .to_owned()
            }
            Self::NoFollowCam => {
                "WorldChrMan+0x1ece0 chrCam or ChrCam+0x60 chrExFollowCam did not resolve"
                    .to_owned()
            }
            Self::WriteFailed => "ChrExFollowCam+0x468 would not take the write".to_owned(),
        }
    }

    /// A short token for the derived file's key, so a reader can grep for it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::UnmeasuredBuild => "unmeasured-build",
            Self::NoHeight => "no-height",
            Self::ParamsNotReady => "params-not-ready",
            Self::RowInUse(_) => "row-in-use",
            Self::RowMissing(_) => "row-missing",
            Self::BaseRowMissing => "base-row-missing",
            Self::NoFollowCam => "no-follow-cam",
            Self::WriteFailed => "write-failed",
        }
    }
}

/// What happened to the camera for one possession, in the terms the derived file prints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Report {
    /// `CSChrPhysicsModule+0x340`, or `None` when it did not read.
    pub(crate) hit_height: Option<f32>,
    /// `CSChrPhysicsModule+0x344`.
    pub(crate) hit_radius: Option<f32>,
    /// The row `[camera].param_row` named.
    pub(crate) row: u32,
    /// The row the untouched fields were copied from -- whatever the camera resolved last frame,
    /// or 0 when that was unreadable.
    pub(crate) base_row: Option<u32>,
    /// The per-chr `camera_distance_scale` in force.
    pub(crate) distance_scale: f32,
    /// What was written, when something was.
    pub(crate) applied: Option<Shape>,
    /// Why nothing was, when nothing was.
    pub(crate) refusal: Option<Refusal>,
}

impl Report {
    /// A report for a possession that never got as far as reading a size.
    pub(crate) const fn refused(row: u32, distance_scale: f32, refusal: Refusal) -> Self {
        Self {
            hit_height: None,
            hit_radius: None,
            row,
            base_row: None,
            distance_scale,
            applied: None,
            refusal: Some(refusal),
        }
    }
}

/// The camera for a subject `hit_height` metres tall and `hit_radius` metres wide.
///
/// `None` when the height is not a usable number -- three shipped `NpcParam` rows carry
/// `hitHeight = 0.0`, and a modded one could carry anything -- in which case the caller leaves the
/// camera alone rather than dividing by it.
pub(crate) fn shape(
    hit_height: f32,
    hit_radius: f32,
    distance_scale: f32,
    settings: CameraSettings,
) -> Option<Shape> {
    if !hit_height.is_finite() || hit_height <= 0.0 {
        return None;
    }
    let height = hit_height.min(MAX_PLAUSIBLE_HEIGHT);
    // A negative or NaN radius is treated as no radius rather than refusing: the clearance floor
    // is a safety net, and losing the net is not a reason to lose the camera.
    let radius = if hit_radius.is_finite() && hit_radius > 0.0 {
        hit_radius
    } else {
        0.0
    };
    let scale = height / PLAYER_HIT_HEIGHT;

    // `powf` on a non-finite exponent would poison everything downstream; the settings parser
    // already rejects those, and this is the second belt.
    let exponent = if settings.distance_exponent.is_finite() {
        settings.distance_exponent
    } else {
        1.0
    };
    let per_chr = if distance_scale.is_finite() && distance_scale > 0.0 {
        distance_scale
    } else {
        1.0
    };
    let ceiling = if settings.distance_max.is_finite() && settings.distance_max > MIN_DISTANCE {
        settings.distance_max
    } else {
        f32::MAX
    };
    // The framing distance, then the two things allowed to override it. The clearance floor is
    // applied last and wins over the ceiling on purpose: a camera outside a 120 m box is a bad
    // shot, a camera inside the model is not a shot at all.
    let distance = (PLAYER_CAM_DIST * scale.powf(exponent) * per_chr)
        .clamp(MIN_DISTANCE, ceiling)
        .max(radius * RADIUS_CLEARANCE);

    // The pivot solve. Put the top of the subject exactly where the top of the player's head sits
    // on screen: the same fraction of the distance above the aim point. Solved against the
    // distance that will actually be used -- so a shot the clearance floor pushed out, or the
    // ceiling pulled in, or the player's own `camera_distance_scale` moved, still frames the head
    // rather than cropping it.
    //
    // `distance / PLAYER_CAM_DIST` is exactly 1.0 at player size and the subtraction inside
    // `PLAYER_HEAD_ABOVE_PIVOT` is exact in `f32`, so this reproduces 1.45 to the bit -- the
    // anchor `tests::at_player_size_the_derived_row_is_the_vanilla_row` pins.
    let head_above_aim = (distance / PLAYER_CAM_DIST) * PLAYER_HEAD_ABOVE_PIVOT;
    // A pivot below the creature's feet would aim the camera at the ground. Only reachable by
    // pushing `camera_distance_scale` past ~30, and clamped rather than refused for the same
    // reason an absurd height is.
    let pivot_height = (height - head_above_aim).max(0.0);

    Some(Shape {
        distance,
        pivot_height,
    })
}

/// Where `shape` actually puts a subject of `height` metres, with the camera at `pitch_deg`.
///
/// This is the law's own report card, and it is what the derived file prints so a player who
/// thinks the framing is wrong has a number rather than an impression. It is not an input to
/// anything.
///
/// `pitch_deg` is `ChrExFollowCam.anglesEuler.x` in degrees, positive meaning the camera is above
/// the subject looking down -- see the module docs for the proof of that sign. The answer barely
/// moves with it (the headroom runs 1.0915 to 1.1119 across the whole -40..+70 range the game
/// allows), which is the pitch-invariance of similarity showing up as a number.
pub(crate) fn framing(shape: Shape, height: f32, pitch_deg: f32) -> Framing {
    let half_frame = (PLAYER_FOV_Y_DEG / 2.0).to_radians().tan();
    let above_aim = height - shape.pivot_height;
    let (sin_pitch, cos_pitch) = pitch_deg.to_radians().sin_cos();
    // How far along the view axis the top of the subject sits: the camera pitching down shortens
    // it, pitching up lengthens it.
    let depth = above_aim.mul_add(-sin_pitch, shape.distance);
    let head_screen_y = (above_aim * cos_pitch / depth) / half_frame;
    Framing {
        head_screen_y,
        headroom_heights: (1.0 - head_screen_y) * depth * half_frame / height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> CameraSettings {
        CameraSettings::default()
    }

    /// The player's own shot, the one every other shot is required to reproduce.
    fn player() -> Shape {
        shape(PLAYER_HIT_HEIGHT, 0.4, 1.0, settings()).expect("a real height")
    }

    /// The anchor. A subject the size of the player must come out as `LockCamParam` row 0, to the
    /// float -- otherwise possessing a human-sized NPC would visibly move the camera for no
    /// reason, and the whole extrapolation would be resting on nothing.
    #[test]
    fn at_player_size_the_derived_row_is_the_vanilla_row() {
        // c0000's own `NpcParam` row: hitHeight 1.5, hitRadius 0.4.
        let shape = player();
        assert_eq!(shape.distance, PLAYER_CAM_DIST);
        assert_eq!(shape.pivot_height, PLAYER_PIVOT);
    }

    /// The product. Every creature the game ships must land where the player lands.
    ///
    /// This is the test the old law would have failed and the old tests did not catch, because
    /// they asked only whether distance rose with height -- which the old law did, while cropping
    /// the subject harder the taller it got. The claim here is the one the user can see: the top
    /// of the body sits in the same place on screen, and the gap above it is the same fraction of
    /// the body, for every size the game contains.
    ///
    /// The heights are the real distribution, not two picked by hand: `NpcParam.hitHeight` for the
    /// base variant of every creature in `data/moveset.tbl` runs 0.30 m to 59.00 m with a median
    /// of 2.00 m, so the sweep covers that range densely and names its endpoints. Re-derive with
    /// `scripts/er-possess-camera-framing.py`.
    #[test]
    fn the_framing_is_the_players_for_every_size_the_game_ships() {
        let want = framing(player(), PLAYER_HIT_HEIGHT, 0.0);
        // Radius zero: this measures the size law alone. The clearance floor is a deliberate
        // departure from it and has its own test below.
        for step in 0_u16..=400 {
            // 0.30 m (c1000 Talk Dummy) to 59.00 m (c4450 Walking Mausoleum).
            let height = 0.30 + (59.00 - 0.30) * f32::from(step) / 400.0;
            let shape = shape(height, 0.0, 1.0, settings()).expect("a real height");
            let got = framing(shape, height, 0.0);
            assert!(
                (got.head_screen_y - want.head_screen_y).abs() < 1.0e-4,
                "{height} m: head at {} half-frames, player is at {}",
                got.head_screen_y,
                want.head_screen_y
            );
            assert!(
                (got.headroom_heights - want.headroom_heights).abs() < 1.0e-3,
                "{height} m: headroom {} subject-heights, player gets {}",
                got.headroom_heights,
                want.headroom_heights
            );
        }
    }

    /// ...and it is the player's framing at every pitch the game lets the camera reach, because
    /// similarity preserves angles. `-40` is `rotRangeMinX`; `+70` is the constructor-fixed
    /// maximum at `ChrExFollowCam+0x25c`.
    #[test]
    fn the_framing_holds_across_the_whole_pitch_range() {
        for pitch in [-40.0_f32, -20.0, 0.0, 20.0, 45.0, 70.0] {
            let want = framing(player(), PLAYER_HIT_HEIGHT, pitch);
            for height in [0.3_f32, 1.5, 2.0, 7.2, 14.1, 29.0, 42.0, 59.0] {
                let shape = shape(height, 0.0, 1.0, settings()).expect("a real height");
                let got = framing(shape, height, pitch);
                assert!(
                    (got.head_screen_y - want.head_screen_y).abs() < 1.0e-4,
                    "{height} m at {pitch} deg: {} vs {}",
                    got.head_screen_y,
                    want.head_screen_y
                );
            }
        }
    }

    /// The composition the whole module exists to hold, stated as the two numbers the user
    /// described it with. A change that moves either of these is a change to what possession
    /// looks like, and should have to say so.
    #[test]
    fn the_player_framing_is_the_one_the_complaint_described() {
        let got = framing(player(), PLAYER_HIT_HEIGHT, 0.0);
        // "at least a full character's height between my head and the top of the screen"
        assert!(
            (got.headroom_heights - 1.0946).abs() < 1.0e-3,
            "{}",
            got.headroom_heights
        );
        assert!(got.headroom_heights > 1.0, "{}", got.headroom_heights);
        // ...and the head sits just above the centre of the screen, not up near the edge.
        assert!(
            (got.head_screen_y - 0.029_554).abs() < 1.0e-4,
            "{}",
            got.head_screen_y
        );
    }

    /// The tallest creature the game ships must not be cropped by the shipped ceiling.
    ///
    /// The previous ceilings both cropped real subjects -- 40 m cropped everything above 3.8 m
    /// tall, and 120 m still cropped this one, whose framing distance is 149.5 m. A ceiling that
    /// fires on a creature is not a guard, it is the bug.
    #[test]
    fn the_shipped_ceiling_cannot_crop_the_tallest_creature_that_exists() {
        // c4450, the Walking Mausoleum: the tallest `hitHeight` in the shipped regulation.
        let tallest = 59.0_f32;
        let biggest = shape(tallest, 18.0, 1.0, settings()).expect("a real height");
        assert!(
            biggest.distance < settings().distance_max,
            "{} m is at or past the {} m ceiling",
            biggest.distance,
            settings().distance_max
        );
        let want = framing(player(), PLAYER_HIT_HEIGHT, 0.0);
        let got = framing(biggest, tallest, 0.0);
        assert!(
            (got.headroom_heights - want.headroom_heights).abs() < 1.0e-3,
            "{}",
            got.headroom_heights
        );
        // ...and the ceiling can never fire on its own, because it sits above the distance the law
        // asks for at the height clamp that runs one step earlier.
        const {
            assert!(
                MAX_FRAMING_DISTANCE
                    >= PLAYER_CAM_DIST * (MAX_PLAUSIBLE_HEIGHT / PLAYER_HIT_HEIGHT),
                "the ceiling has fallen below the law it is derived from"
            );
        }
        let absurd = shape(MAX_PLAUSIBLE_HEIGHT * 10.0, 0.0, 1.0, settings()).expect("clamped");
        assert!(absurd.distance < MAX_FRAMING_DISTANCE, "{absurd:?}");
    }

    /// The clearance floor never lets the camera sit inside the physics capsule, for any of the
    /// creatures the shipped regulation actually contains.
    #[test]
    fn the_camera_clears_every_shipped_creatures_own_radius() {
        // (chr, hitHeight, hitRadius) straight out of `NpcParam`, biggest first.
        const CREATURES: [(u32, f32, f32); 9] = [
            (4450, 59.0, 18.0),
            (4504, 42.0, 17.0),
            (4760, 29.0, 10.0),
            (4501, 14.0, 14.0),
            (4500, 12.0, 12.0),
            (4520, 10.0, 10.0),
            (4600, 7.2, 1.8),
            (5210, 2.4, 3.5),
            // The two creatures in the whole game wider than the law's own distance: 0.8 m tall,
            // 2.0 m across. The floor pushes the camera out, which only ever adds headroom.
            (5472, 0.8, 2.0),
        ];
        for (chr, height, radius) in CREATURES {
            let shape = shape(height, radius, 1.0, settings()).expect("a real height");
            assert!(
                shape.distance > radius,
                "c{chr}: camera at {} is inside a {radius} m radius",
                shape.distance
            );
        }
    }

    /// A camera the clearance floor pushed out still frames the head -- wider than the player's
    /// shot, never tighter. That is the whole reason the pivot is solved against the distance that
    /// will be used rather than computed from the height.
    #[test]
    fn a_camera_pushed_out_by_the_floor_frames_wider_rather_than_cropping() {
        let want = framing(player(), PLAYER_HIT_HEIGHT, 0.0);
        // c5472 Great Dragonfly: 0.8 m tall, 2.0 m wide, so the floor beats the law.
        let pushed = shape(0.8, 2.0, 1.0, settings()).expect("a real height");
        assert_eq!(pushed.distance, 2.0 * RADIUS_CLEARANCE);
        let got = framing(pushed, 0.8, 0.0);
        // The head stays exactly where the player's is...
        assert!(
            (got.head_screen_y - want.head_screen_y).abs() < 1.0e-4,
            "{}",
            got.head_screen_y
        );
        // ...and the extra distance shows up as more room above it, never less.
        assert!(
            got.headroom_heights > want.headroom_heights,
            "{} vs {}",
            got.headroom_heights,
            want.headroom_heights
        );
    }

    /// A ceiling low enough to pull the camera in still frames the head. Same property from the
    /// other side: the pivot follows the distance wherever it ends up.
    #[test]
    fn a_camera_pulled_in_by_the_ceiling_still_frames_the_head() {
        let tight = CameraSettings {
            distance_max: 20.0,
            ..settings()
        };
        let want = framing(player(), PLAYER_HIT_HEIGHT, 0.0);
        let cropped = shape(29.0, 0.0, 1.0, tight).expect("a real height");
        assert_eq!(cropped.distance, 20.0, "the ceiling decided this one");
        let got = framing(cropped, 29.0, 0.0);
        assert!(
            (got.head_screen_y - want.head_screen_y).abs() < 1.0e-4,
            "the head must not be cropped by a low ceiling: {}",
            got.head_screen_y
        );
        // The shot is worse -- there is less room above a 29 m body seen from 20 m -- and the
        // report says so rather than the head disappearing.
        assert!(
            got.headroom_heights < want.headroom_heights,
            "{}",
            got.headroom_heights
        );
    }

    /// The clearance floor beats the ceiling, because a camera inside the model is not a shot.
    #[test]
    fn the_radius_floor_wins_over_the_distance_ceiling() {
        let tight = CameraSettings {
            distance_max: 6.0,
            ..settings()
        };
        // c4504: 42 m tall, 17 m wide. The ceiling would put the camera 6 m out, well inside it.
        let shape = shape(42.0, 17.0, 1.0, tight).expect("a real height");
        assert_eq!(shape.distance, 17.0 * RADIUS_CLEARANCE);
    }

    /// Bigger is further away and pivots higher, monotonically, over the whole shipped range.
    ///
    /// Weak on its own -- the law this replaced also passed it, while cropping the subject harder
    /// the taller it got -- and kept only as a sanity floor under
    /// [`the_framing_is_the_players_for_every_size_the_game_ships`], which is the real claim.
    #[test]
    fn distance_and_pivot_rise_with_size() {
        let heights = [0.6_f32, 1.0, 1.5, 1.9, 3.6, 7.2, 12.0, 29.0, 59.0];
        let mut previous: Option<Shape> = None;
        for height in heights {
            let shape = shape(height, 0.0, 1.0, settings()).expect("a real height");
            if let Some(before) = previous {
                assert!(
                    shape.distance > before.distance,
                    "{height} m: {} is not further than {}",
                    shape.distance,
                    before.distance
                );
                assert!(
                    shape.pivot_height > before.pivot_height,
                    "{height} m: pivot {} is not above {}",
                    shape.pivot_height,
                    before.pivot_height
                );
            }
            previous = Some(shape);
        }
    }

    /// The pivot is the player's fraction of the body, not its chest.
    ///
    /// The old law converged on `0.65 * height`, which put a 29 m subject's head at `+0.31`
    /// half-frames -- ten times the player's `+0.03`, three quarters of the way to the top edge --
    /// and cost a quarter of the headroom. That is the "not high enough" half of the complaint.
    #[test]
    fn a_big_body_is_framed_where_the_player_is_and_not_on_its_chest() {
        let huge = shape(29.0, 0.0, 1.0, settings()).expect("h");
        let vanilla_fraction = PLAYER_PIVOT / PLAYER_HIT_HEIGHT;
        assert!(
            (huge.pivot_height - 29.0 * vanilla_fraction).abs() < 1.0e-3,
            "{}",
            huge.pivot_height
        );
        // ...which is well above where the chest law aimed.
        assert!(huge.pivot_height > 0.65 * 29.0, "{}", huge.pivot_height);
        // And the framing that law produced, computed here so the regression is measured rather
        // than remembered: head at +0.31 half-frames instead of +0.03.
        let chest = Shape {
            distance: huge.distance,
            pivot_height: 0.65 * 29.0,
        };
        assert!(framing(chest, 29.0, 0.0).head_screen_y > 0.3);
        assert!(framing(huge, 29.0, 0.0).head_screen_y < 0.04);
    }

    /// The three shipped `NpcParam` rows with `hitHeight = 0.0` -- and anything a modded
    /// regulation could put there -- refuse rather than divide by it.
    #[test]
    fn an_unusable_height_yields_nothing_at_all() {
        for height in [0.0_f32, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(shape(height, 1.0, 1.0, settings()), None, "{height}");
        }
    }

    /// An absurd modded height is clamped rather than refused, and still produces finite numbers.
    #[test]
    fn an_absurd_height_is_clamped_and_stays_finite() {
        let shape = shape(1.0e9, 0.0, 1.0, settings()).expect("clamped, not refused");
        assert!(shape.distance.is_finite());
        assert!(shape.pivot_height.is_finite());
        assert!(shape.distance <= MAX_FRAMING_DISTANCE);
        // The clamp is on the height, so the pivot is consistent with the distance and the shot is
        // still the player's rather than merely finite.
        let got = framing(shape, MAX_PLAUSIBLE_HEIGHT, 0.0);
        let want = framing(player(), PLAYER_HIT_HEIGHT, 0.0);
        assert!((got.head_screen_y - want.head_screen_y).abs() < 1.0e-4);
    }

    /// A `distance_max` below the hard floor is ignored rather than made to panic.
    ///
    /// `f32::clamp` panics when its minimum exceeds its maximum, and the settings parser accepts
    /// any positive number -- so `distance_max = 0.1` reaches this function and must not take the
    /// game down over a config typo.
    #[test]
    fn a_ceiling_below_the_hard_floor_is_ignored_rather_than_a_panic() {
        let silly = CameraSettings {
            distance_max: 0.1,
            ..settings()
        };
        let ignored = shape(12.0, 0.0, 1.0, silly).expect("a real height");
        let plain = shape(12.0, 0.0, 1.0, settings()).expect("a real height");
        assert!(ignored.distance > MIN_DISTANCE);
        assert_eq!(ignored.distance, plain.distance);
    }

    /// A junk radius costs the clearance floor and nothing else.
    #[test]
    fn a_junk_radius_does_not_poison_the_distance() {
        let clean = shape(12.0, 0.0, 1.0, settings()).expect("a real height");
        for radius in [f32::NAN, -3.0, f32::INFINITY] {
            let junk = shape(12.0, radius, 1.0, settings()).expect("a real height");
            assert!(junk.distance.is_finite(), "{radius}");
            assert_eq!(junk.distance, clean.distance, "{radius}");
        }
    }

    /// `camera_distance_scale` multiplies the law and can pull the camera in as well as push it
    /// out, which is what a player tuning a quadruped needs -- and the head stays framed either
    /// way, because the pivot is solved against the distance the scale produced.
    #[test]
    fn the_per_chr_scale_moves_the_distance_both_ways_without_losing_the_head() {
        let want = framing(player(), PLAYER_HIT_HEIGHT, 0.0);
        let plain = shape(12.0, 0.0, 1.0, settings()).expect("h");
        for scale in [0.5_f32, 2.0] {
            let moved = shape(12.0, 0.0, scale, settings()).expect("h");
            assert!(
                (moved.distance - plain.distance * scale).abs() < 1.0e-3,
                "{scale}"
            );
            let got = framing(moved, 12.0, 0.0);
            assert!(
                (got.head_screen_y - want.head_screen_y).abs() < 1.0e-4,
                "{scale}: {}",
                got.head_screen_y
            );
        }
    }

    /// A junk `distance_scale` falls back to 1.0 rather than producing a NaN camera.
    #[test]
    fn a_junk_per_chr_scale_falls_back_to_one() {
        let plain = shape(12.0, 0.0, 1.0, settings()).expect("h");
        for scale in [f32::NAN, 0.0, -2.0] {
            assert_eq!(
                shape(12.0, 0.0, scale, settings()).expect("h").distance,
                plain.distance,
                "{scale}"
            );
        }
    }

    /// A zero exponent turns the size law off without turning the clearance floor off -- the one
    /// setting combination that could otherwise put a camera inside a dragon.
    #[test]
    fn a_flat_exponent_still_clears_the_body() {
        let flat = CameraSettings {
            distance_exponent: 0.0,
            ..settings()
        };
        let shape = shape(12.0, 12.0, 1.0, flat).expect("h");
        assert_eq!(shape.distance, 12.0 * RADIUS_CLEARANCE);
    }

    /// An exponent below 1.0 is the setting that caused the complaint, and it is still available
    /// as a taste knob. It must not be able to crop the head: the pivot solve follows whatever
    /// distance it produces.
    #[test]
    fn a_sublinear_exponent_tightens_the_shot_without_cropping_the_head() {
        let old = CameraSettings {
            distance_exponent: 0.7,
            ..settings()
        };
        let want = framing(player(), PLAYER_HIT_HEIGHT, 0.0);
        let tight = shape(29.0, 0.0, 1.0, old).expect("h");
        let plain = shape(29.0, 0.0, 1.0, settings()).expect("h");
        assert!(tight.distance < plain.distance, "0.7 is the tighter shot");
        let got = framing(tight, 29.0, 0.0);
        assert!(
            (got.head_screen_y - want.head_screen_y).abs() < 1.0e-4,
            "{}",
            got.head_screen_y
        );
        assert!(got.headroom_heights < want.headroom_heights);
    }

    /// A negative pivot would aim the camera underground, and only an absurd `distance_scale` can
    /// ask for one.
    #[test]
    fn an_absurd_distance_scale_cannot_aim_below_the_feet() {
        let shape = shape(1.5, 0.0, 500.0, settings()).expect("h");
        assert!(shape.pivot_height >= 0.0, "{shape:?}");
        assert!(shape.pivot_height.is_finite());
    }

    /// Every refusal spells itself differently, so a derived file naming one is unambiguous.
    #[test]
    fn every_refusal_has_its_own_name_and_a_sentence() {
        let all = [
            Refusal::Disabled,
            Refusal::UnmeasuredBuild,
            Refusal::NoHeight,
            Refusal::ParamsNotReady,
            Refusal::RowInUse(1000),
            Refusal::RowMissing(1000),
            Refusal::BaseRowMissing,
            Refusal::NoFollowCam,
            Refusal::WriteFailed,
        ];
        let mut names: Vec<&str> = all.iter().copied().map(Refusal::name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "{names:?}");
        for refusal in all {
            let sentence = refusal.describe();
            assert!(sentence.len() > 20, "{refusal:?}: {sentence}");
        }
    }
}
