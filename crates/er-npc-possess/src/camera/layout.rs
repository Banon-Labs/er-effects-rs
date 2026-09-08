//! Every game STRUCT offset the camera layer touches, and the build gate over them.
//!
//! Same shape and same reasons as [`crate::possess::layout`]: a table of constants rather than
//! typed field access, because the answer for a build nobody has measured has to be **`None`**
//! and a struct field cannot give that answer.
//!
//! # What was measured, and how
//!
//! All seven offsets below were byte-proven identical on 1.16.2 (`eldenring-deobf.bin`) and 1.17
//! (`eldenring-deobf-1.17.bin`), which is the build actually installed. The evidence, in the order
//! a reader would want to re-run it:
//!
//! * **`WorldChrMan+0x1ece0 chrCam`** -- the 1.16.2 named dump types the field. `FUN_1404a6c30`
//!   (92 bytes: `UpdateRecursive`, then `GLOBAL_WorldChrMan`, then `[+0x1ece0]`, then a 4x4 matrix
//!   copy) matches uniquely in the 1.17 image at `0x1404a7190` with the displacement still
//!   `0x1ece0`, off the 1.17 `GLOBAL_WorldChrMan` at `0x143d69ff8`.
//! * **`ChrCam+0x60 chrExFollowCam`** -- `CS::ChrCam::Update` matches uniquely in 1.17 at
//!   `0x1403b11d0` (its 0x60-byte prologue is byte-identical bar one rip-relative operand), and
//!   three sites in it do `MOV RCX,[RDI+0x60]` immediately before `CALL 0x1403b5ad0`, which is
//!   `ChrExFollowCam::Update`. The crate's `ChrCam` models the same field; the `const` assertion
//!   in [`crate::camera::game`] pins the two together through `size_of::<CSPersCam>()`.
//! * **`ChrExFollowCam+0x468`** -- see [`chr_ex_follow_cam::LOCK_CAM_PARAM_OVERRIDE`].
//! * **`ChrExFollowCam+0x460`** -- written by `ApplyZoomLerp` at 1.17 `0x1403b76da`.
//! * **`ChrExFollowCam+0x150/+0x154 anglesEuler`** -- see
//!   [`chr_ex_follow_cam::ANGLES_EULER_YAW`], whose 73-byte window is unique in both images.
//! * **`CSChrPhysicsModule+0x340/+0x344`** -- `ChrIns::GetPhysicsHitHeight` is
//!   `[ChrIns+0x190] -> [+0x68] -> MOVSS XMM0,[RCX+0x340]; RET` in the 1.16.2 named dump, and the
//!   same three-instruction chain sits at 1.17 `0x1403efe50 -> 0x14045e590` with the same `0x340`.
//!   Its neighbour `0x1403efe60 -> 0x14045e5a0` reads `0x344`, the radius.
//!
//! Identical on both is not the same as identical forever, which is why this module still gates:
//! a third build gets `None`, the camera is left alone, and the derived file says
//! `unmeasured-build` rather than the mod writing a float into whatever now lives at `+0x468`.

// Pure arithmetic; ungated so its tests run on the host.
#![cfg_attr(not(windows), allow(dead_code))]

use er_game_base::game_build::{FileVersion, SUPPORTED_FILE_VERSION};

use crate::possess::layout::FILE_VERSION_1170;

/// `WorldChrManImp`.
pub(crate) mod world_chr_man {
    /// `WorldChrManImp.chrCam`. Cross-checked against `offset_of!(WorldChrMan, chr_cam)`.
    pub(crate) const CHR_CAM: usize = 0x1ece0;
}

/// `ChrCam`, the object `WorldChrMan.chrCam` points at.
pub(crate) mod chr_cam {
    /// `ChrCam.chrExFollowCam`. The crate spells this field privately, so the compile-time
    /// cross-check goes through `size_of::<CSPersCam>()` instead -- `ChrCam` opens with its
    /// `CSPersCam` superclass and this pointer is the very next field.
    pub(crate) const EX_FOLLOW_CAM: usize = 0x60;
}

/// `ChrExFollowCam` -- the third-person follow camera, and the only thing this layer writes.
pub(crate) mod chr_ex_follow_cam {
    /// `ChrExFollowCam+0x468` -- The lever, and the reason this layer needs no detour.
    ///
    /// `CS::ChrExFollowCam::ApplyZoomLerp` (1.17 `0x1403b7570`) runs every frame out of
    /// `ChrExFollowCam::Update` and opens its param-id resolution by loading this field, before
    /// the branch that picks lock-on or normal framing. It is first in both chains:
    ///
    /// * normal: `+0x468` -> `GameMan+0x40` -> `GameMan+0x48` -> `GameMan+0x4c` -> `+0x464` -> 0
    /// * lock-on: `+0x468` -> `GameMan+0x44` -> `GameMan+0x54` -> `+0x464` -> 0
    ///
    /// **Nothing in the game ever writes it.** A byte scan of the whole camera code range
    /// (`0x1403b0000`-`0x1403c0000`) in the 1.17 image finds exactly one access at displacement
    /// `0x468` and it is the `MOV EAX,[RBX+0x468]` load at `0x1403b766a`; image-wide there are
    /// only eight 32-bit stores at that displacement and every one is in unrelated code far
    /// outside the camera module (several are `[RBP+0x468]` stack frames). The constructor sets it
    /// with a single qword store covering `+0x464|+0x468` -- `MOV qword [RDI+0x464],-1` at 1.17
    /// `0x1403b3b94` -- and never touches it again.
    ///
    /// `+0x464` is not free and must not be used instead: `FUN_1403b1140` writes it from the map
    /// region (`MOV [RCX+0x464],EDX` at 1.17 `0x1403b5950`).
    pub(crate) const LOCK_CAM_PARAM_OVERRIDE: usize = 0x468;
    /// `ChrExFollowCam+0x460` -- the id `ApplyZoomLerp` actually resolved last frame, mirrored out
    /// for free (`MOV [RBX+0x460],EDX` at 1.17 `0x1403b76da`).
    ///
    /// Read once at possession start to learn which row the camera was using a frame ago, so the
    /// fields the size law does not decide can be copied from it rather than from a guess.
    pub(crate) const RESOLVED_LOCK_CAM_PARAM: usize = 0x460;

    /// `ChrExFollowCam+0x150 anglesEuler` -- the camera's own Euler angles. `x` is the pitch.
    ///
    /// Named here only so [`ANGLES_EULER_YAW`] can say what it is one float past. Nothing reads
    /// the pitch: the body turns about Y only.
    pub(crate) const ANGLES_EULER: usize = 0x150;
    /// `ChrExFollowCam+0x154 anglesEuler.y` -- Where the player is looking, as one float.
    ///
    /// # What the number is, exactly
    ///
    /// `ChrExFollowCam::Update` closes by deriving the camera's Euler angles from the vector
    /// between the point it is aimed at and the smoothed subject anchor:
    ///
    /// ```text
    /// delta = targetCoordinates(+0xe0) - pivot         ; MOVAPS/SUBPS, 1.16.2 0x1403b62c9
    /// pitch = angleFromXZPlane(delta, anglesEuler.x)   ; +0x150 goes in as the fallback
    /// yaw   = angleOnXZPlane  (delta, anglesEuler.y)   ; +0x154 goes in as the fallback
    /// anglesEuler = (pitch, yaw, pitch, yaw)           ; three UNPCKLPS, then MOVAPS to +0x150
    /// ```
    ///
    /// and `angleOnXZPlane` (1.16.2 `0x1403b0c20`) is `atan2f(v.x, v.z)`, returning the passed-in
    /// previous angle unchanged when the delta has no horizontal part. So this field is
    /// **`atan2(look.x, look.z)`** and nothing else.
    ///
    /// # Why the angle and not the two coordinate fields
    ///
    /// Because `targetCoordinates` is differenced against the smoothed pivot above, not against
    /// `cameraCoordinates(+0x110)`. Subtracting those two points would produce a
    /// plausible-looking direction that the engine never actually computes. This is the engine's
    /// own answer, in one read.
    ///
    /// # Sign
    ///
    /// A character's yaw is `atan2(-d.x, -d.z)` for the direction `d` it faces -- two independent
    /// proofs in [`crate::possess::intent`] -- so the body heading for this look direction is this
    /// field plus PI. [`crate::possess::intent::aim`] is the one place that conversion happens.
    ///
    /// Byte-proven: the 73-byte window carrying `+0xe0`, `+0x150` and `+0x154` matches uniquely in
    /// both images -- 1.16.2 `0x1403b62c9`, 1.17 `0x1403b62d9`.
    pub(crate) const ANGLES_EULER_YAW: usize = ANGLES_EULER + core::mem::size_of::<f32>();
}

/// `CSChrPhysicsModule` -- reached through `ChrIns+0x190 -> +0x68`, the same chain
/// [`crate::possess::layout::modules`] already names.
pub(crate) mod chr_physics_module {
    /// `hitHeight`, metres. What `CS::ChrIns::GetPhysicsHitHeight` returns, populated from
    /// `NpcParam.hitHeight` by `CSChrPhysicsModule::InitForEnemy`.
    ///
    /// This is the size scalar the whole layer rests on, and it is the only one available: ELDEN
    /// RING has no chr-scale param at all, and the FLVER bounding box is build-time-only and
    /// degenerate (`FLT_MAX`) on eleven chrs including `c0000`.
    pub(crate) const HIT_HEIGHT: usize = 0x340;
    /// `hitRadius`, metres -- the horizontal half-extent, i.e. the distance inside which the
    /// camera would be inside the model.
    pub(crate) const HIT_RADIUS: usize = 0x344;
}

/// The offsets for the running build, or `None` on one nobody has measured.
///
/// Every field is the same on both measured builds, which is exactly why this returns a struct
/// rather than a bool: when one of them does move, the difference belongs here and every caller
/// picks it up without changing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Offsets {
    pub(crate) chr_cam: usize,
    pub(crate) ex_follow_cam: usize,
    pub(crate) lock_cam_param_override: usize,
    pub(crate) resolved_lock_cam_param: usize,
    pub(crate) hit_height: usize,
    pub(crate) hit_radius: usize,
    pub(crate) angles_euler_yaw: usize,
}

/// The measured offsets, or `None`.
#[must_use]
pub(crate) fn offsets(version: Option<FileVersion>) -> Option<Offsets> {
    let measured = matches!(version?, v if v == SUPPORTED_FILE_VERSION || v == FILE_VERSION_1170);
    measured.then_some(Offsets {
        chr_cam: world_chr_man::CHR_CAM,
        ex_follow_cam: chr_cam::EX_FOLLOW_CAM,
        lock_cam_param_override: chr_ex_follow_cam::LOCK_CAM_PARAM_OVERRIDE,
        resolved_lock_cam_param: chr_ex_follow_cam::RESOLVED_LOCK_CAM_PARAM,
        hit_height: chr_physics_module::HIT_HEIGHT,
        hit_radius: chr_physics_module::HIT_RADIUS,
        angles_euler_yaw: chr_ex_follow_cam::ANGLES_EULER_YAW,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_measured_builds_answer_and_nothing_else_does() {
        assert!(offsets(Some(SUPPORTED_FILE_VERSION)).is_some(), "1.16.2");
        assert!(offsets(Some(FILE_VERSION_1170)).is_some(), "1.17");
        assert_eq!(
            offsets(Some(SUPPORTED_FILE_VERSION)),
            offsets(Some(FILE_VERSION_1170)),
            "byte-proven identical on both; a divergence belongs in the table, not here"
        );
        assert_eq!(
            offsets(Some(FileVersion {
                major: 2,
                minor: 8,
                build: 0,
                revision: 0,
            })),
            None,
            "a build nobody measured"
        );
        // The host, where there is no game image at all.
        assert_eq!(offsets(None), None);
    }

    /// The override slot and the mirror are adjacent `i32`s and must not be confused: writing the
    /// mirror does nothing at all, because `ApplyZoomLerp` overwrites it every frame.
    #[test]
    fn the_override_and_the_mirror_are_distinct_adjacent_words() {
        assert_eq!(
            chr_ex_follow_cam::LOCK_CAM_PARAM_OVERRIDE - chr_ex_follow_cam::RESOLVED_LOCK_CAM_PARAM,
            core::mem::size_of::<i32>() * 2,
            "+0x460 mirror, +0x464 map-region id, +0x468 our override"
        );
    }

    /// The camera yaw is the second float of the Euler vector; the one beside it is the pitch.
    ///
    /// Reading `+0x150` instead would aim the body with the camera's elevation, which shows up as
    /// "the creature spins when I look up" rather than as anything that looks like an offset bug.
    #[test]
    fn the_camera_yaw_is_the_y_component_and_the_pitch_is_the_x() {
        assert!(
            chr_ex_follow_cam::ANGLES_EULER.is_multiple_of(16),
            "the whole vector is stored with one MOVAPS, so it is 16-aligned"
        );
        assert_eq!(
            chr_ex_follow_cam::ANGLES_EULER_YAW,
            0x154,
            "the byte-proven displacement, checked against the derivation rather than repeated \
             by it: +0x150 pitch, +0x154 yaw, because the UNPCKLPS interleave puts the yaw second"
        );
    }

    /// Height and radius are adjacent floats in the physics module, in that order.
    #[test]
    fn the_hit_extents_are_adjacent_floats() {
        assert_eq!(
            chr_physics_module::HIT_RADIUS - chr_physics_module::HIT_HEIGHT,
            core::mem::size_of::<f32>()
        );
    }
}
