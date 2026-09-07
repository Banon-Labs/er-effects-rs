//! The camera layer -- stack layer 5. Framing the thing you are wearing.
//!
//! # The problem
//!
//! Layer 2's seam (`WorldChrManDbg+0xb8 camOverrideChrIns`) already makes the camera and lock-on
//! follow the possessed creature for free. What it does not do is change the camera's SHAPE: the
//! follow camera's distance and pivot height come from a `LockCamParam` row sized for a 1.5 m
//! capsule, so wearing an Ancient Dragon puts the camera inside the model. This layer solves those
//! two numbers for the body actually being worn -- see [`geometry`], which derives them rather
//! than tuning them, from the framing the vanilla row gives the player.
//!
//! # How, in one paragraph
//!
//! At possession start it reads the creature's own physics capsule -- `CSChrPhysicsModule+0x340
//! hitHeight`, the value `CS::ChrIns::GetPhysicsHitHeight` returns -- turns it into two camera
//! numbers ([`geometry`]), writes them into a `LockCamParam` row nothing else references, and puts
//! that row's id in `ChrExFollowCam+0x468`, which is the highest-precedence input to the engine's
//! own per-frame `ApplyZoomLerp` and which nothing in the game ever writes. On release the row's
//! bytes and `+0x468` both go back exactly as they were. See [`game`] for the mechanism and
//! [`layout`] for the offsets and their proof.
//!
//! # The one thing it costs, and the one thing it does not
//!
//! It costs **no game address and no detour** -- both singletons come from `fromsoftware-rs`'s
//! DLRF name resolution and everything else is a struct field, exactly like layers 1 and 2. It does
//! write a param row, which no earlier layer did; `scripts/me3-dll-conflicts.toml` carries why that
//! is still co-loadable with the other 25 shells.
//!
//! # What it deliberately does not do, named so nobody goes looking
//!
//! * **The dummy-poly anchor.** `GameMan+0x50 cameraFollowDummyPoly` makes the camera pivot on a
//!   named bone (220 is the chest on 401 of 422 chr FLVERs) instead of on the character transform,
//!   and it would frame a quadruped better than a height offset can. It is not used, for two
//!   reasons that compound: `GameMan::_Update` resets that field to -1 every frame so it would
//!   need a per-frame write, and `GetDmypolyPosition` pre-fills an identity matrix and ignores its
//!   own found-count, so a creature without dummy 220 gets pivoted on the world origin rather than
//!   on itself. Guarding that needs `CS::ChrIns::ChrHasDmypolyId`, which is a game address this
//!   layer would otherwise not spend. The `chrOrgOffset_Y` path costs neither, and the two are
//!   alternatives rather than additions -- the offset cancels out on the dummy path.
//! * **Either pitch limit.** `ChrExFollowCam.anglesEuler.x` is clamped in `applyControlMovement`
//!   (1.16.2 `0x1403b7be0`) between `+0x258`, which `ApplyZoomLerp` fills from the row's
//!   `rotRangeMinX` (-40 degrees for the player), and `+0x25c`, which the constructor fixes at
//!   `0x3f9c61aa` = +70 degrees and which nothing else in the image writes. Positive is the camera
//!   above the subject looking down -- see [`geometry`] for how that sign was established -- so
//!   `+0x25c` is the overhead limit and `rotRangeMinX` is the limit on dropping below.
//!
//!   Neither is touched. The framing this layer holds is pitch-invariant (similarity preserves
//!   angles), so no amount of subject size makes 70 degrees of overhead insufficient; and writing
//!   `rotRangeMinX` would override a narrowing some map region asked for, in exchange for nothing.
//!   `+0x2d4 verticalAngleLimit` (60 degrees, also ctor-fixed) is a third field and is not the
//!   pitch clamp at all -- `applyControlMovement` uses `cos(it)` as a threshold on a normalised
//!   direction, in the auto-rotation branch.
//! * **The mount blend.** `ApplyZoomLerp` has a second branch, taken when
//!   `ChrExFollowCam+0x488` is set, which adds the delta between two `LockCamParamLookupResult`s
//!   the CONSTRUCTOR cached (`+0x478`/`+0x480` against `lockCamParam`) to every value it just
//!   derived from our row. That is the Torrent zoom blend, and it is left alone: the deltas are
//!   the game's own and the engine clamps the result (distance to 100, pivot to 10, FOV to pi),
//!   so possessing while mounted gets a camera that is blended rather than wrong. Nobody has
//!   measured how it looks.
//! * **Body scale.** If a later layer ever applies a visual scale (`ChrCtrl::SetScaleSize`), the
//!   hurtbox does not follow it -- that function writes `ChrCtrl+0x2d4` and mirrors to
//!   `CSChrDataModule+0x54`, and never touches `CSChrPhysicsModule` where `hitHeight` lives. The
//!   camera would then have to be driven from the same scalar the scale used, not from
//!   `hitHeight`.
//!
//!   [`crate::possess::body_size`] now does apply one -- and this layer is still correct, which is
//!   the part worth reading rather than the part worth panicking about. It scales the possessing
//!   player's own invisible body, to lift its lock-on dummy to the creature's height. The camera's
//!   subject is the creature, whose scale is untouched, so `hitHeight` remains the right scalar
//!   here. The warning above still stands for the case it was written about: a scale applied to
//!   the subject.

// The size law, the offset table and the derived-file renderer are pure, so `cargo test` proves
// them on the host with no game running. Only `game` touches live memory.
pub(crate) mod derived;
pub(crate) mod geometry;
pub(crate) mod layout;

#[cfg(windows)]
pub(crate) mod game;

#[cfg(windows)]
pub(crate) use game::Session;
