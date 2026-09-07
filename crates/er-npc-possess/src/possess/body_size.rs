//! Why the lock-on RETICLE sat at the creature'S feet, and the one number that moves it.
//!
//! # What the reticle is actually on
//!
//! Not the creature. **The player's own body**, and that is settled rather than suspected.
//!
//! `WorldChrManDbg+0xb8 camOverrideChrIns` makes the possessed creature the lock-on *subject*:
//! `LockTgtMan`'s per-frame update (1.16.2 `FUN_140716260`, 1.17 `0x1407170b0`) opens by calling
//! `WorldChrManImp::GetMainPlayerIns` (1.16.2 `0x140507ff0`), which returns the override when one
//! is set and only falls back to the raw `+0x1e508 mainPlayerIns`. So every candidate the search
//! considers is measured against the creature -- and the one self-test in the whole scan is
//! `CMP R14,R15` at 1.16.2 `0x1407166e3`, raw pointer identity against that same overridden
//! pointer. The real `PlayerIns` is a different pointer, so it is not excluded. It is standing
//! exactly where the creature is (this engine puts it there every frame), it is still
//! lock-on-able on purpose, and it is therefore the nearest legal target in the world.
//!
//! That is what the player is locking onto, which is why the reticle appeared at their feet: it
//! was drawn on a 1.5 m body at the bottom of a 12 m dragon.
//!
//! # this module is now the FALLBACK, not the answer (2026-09-02)
//!
//! The paragraph above is still true of the pointer identity check, but it stopped being the whole
//! story: the scan's other filter is `CS::ChrIns::CanTargetTeamType(subject, candidate)`, and with
//! the creature as subject that made every real enemy a friend and the player's own body the only
//! opposing character in the world. A player reported exactly that -- "I've never been able to
//! lock on to anyone but myself". [`crate::possess::layout::chr_ins::TEAM_TYPE_CHARMED`] is the
//! one-byte fix, and while it holds the body is not a candidate at all and nothing is drawn on it.
//!
//! The scale below is kept because that write can fail to take: the byte may not read, or a
//! network `TemporaryTeamType` slot may outrank it in `GetTeamType`. In either case the old world
//! comes back exactly as described above, and the reticle needs to be somewhere sensible.
//!
//! # Why the reticle cannot be moved directly
//!
//! Its world position is a **dummy polygon on the target's model**. `CS::ChrSlotSys::AddActPntSlot`
//! (`0x140499980`) registers each character's lock points over the dummy-id range `0xdc..0xe4`
//! (220..228), selected per character by `NpcParam.lockGazePoint0..7`; `FUN_14049ce50` resolves
//! each one through `ChrIns::GetDmypolyPosition` and caches the result at `ActPntNode+0x00`; and
//! `FUN_140716260` copies that vector into the subject's `ChrIns+0xd0 lockOnTargetPos`. There is
//! no per-character "lock point offset" field anywhere in that chain -- the one offset field in
//! the neighbourhood, `ChrCtrl+0x1a0 lockOnTagOffset`, is added by
//! `CS::CSFeManImp::UpdateChrEnemyTagEntries` (`0x140776a30`) to the **name/HP tag**, which uses
//! dummy 210 (`MOV dword ptr [RSP+0x20],0xd2` at `0x140776c58`) and is a different surface.
//!
//! So the only way to move the anchor is to move the model the dummy is attached to.
//!
//! # The lever: the body wears the creature's height
//!
//! `CS::ChrCtrl::RecalculateChrMatrix` builds `ChrCtrl.modelMatrix` from the physics matrix,
//! `verticalPositionOffset` and `scaleSize{X,Y,Z}`, and then copies that matrix straight into
//! `locationMtx44ChrEntity->mtx` -- the root of the location tree that
//! `GetLocationMdlDmypolyModifier` resolves dummies against. Scale the body and every dummy on it
//! moves with it, the lock-gaze points included. Only `Y` is written; see [`worn_scale`] for why
//! the other two would be cost without benefit.
//!
//! Of the two inputs, **scale is the one that holds**. `verticalPositionOffset` is recomputed
//! every frame by `FUN_1403cbc50` -- `verticalPositionOffset = offsetY` (from
//! `NpcParam.hitYOffset`) and then `+= footIkCorrection` -- so a write to it survives until the
//! next frame at best. `scaleSize` has exactly two writers in the whole character range: the
//! `ChrCtrl` constructor's `1.0` init and `ChrCtrl::SetScaleSize`, whose single caller
//! (`FUN_1403f74a0` <- `FUN_140403320`) is character construction. Nothing recomputes it, which
//! makes it the same class of lever as `ChrExFollowCam+0x468`: write it once, put it back on
//! release.
//!
//! And it is free of side effects on anything that matters here, which is settled RE rather than
//! hope: body scale is render-only. `SetScaleSize` writes `ChrCtrl+0x2d4..0x2dc` and mirrors to
//! `CSChrDataModule+0x54..0x5c` and touches `CSChrPhysicsModule` nowhere, so the hknp capsule --
//! the body's collision, its hurtbox and its `hitHeight` -- does not move. See `bd`
//! `possession-camera-size-adaptation-levers-2026-09-01`, which reached the same conclusion from
//! the other direction and warned the camera layer off scaling for exactly this reason.
//!
//! # What the scale is, and why this module contains no notion of "centre"
//!
//! [`scale_for`] is one division: `creature hitHeight / player hitHeight`. That is deliberate, and
//! it is the answer to "is `hitHeight` the right notion of the centre of the model" -- **the
//! question does not arise**, because this layer never decides where the centre is. The player's
//! model already carries its own lock-gaze dummy at whatever fraction of its height FromSoftware
//! put it; scaling by `s` multiplies that fraction's height by `s` and nothing else. The anchor
//! lands where the game's own artists would have put it on a body of that size.
//!
//! All `hitHeight` has to be, then, is proportional to model size -- and it is the only scalar in
//! the game that is. ELDEN RING ships no chr-scale param in any of its 179 paramdefs, and the
//! FLVER bounding box is build-time-only and degenerate (`FLT_MAX`) on eleven chrs including
//! `c0000`, so the two obvious alternatives do not exist. [`crate::camera::geometry`] reached the
//! same dead end and rests on the same field.
//!
//! **The honest limitation**: `hitHeight` is the physics capsule, and on a long-necked or
//! long-tailed creature the visual model is taller than its capsule. The ratio then understates
//! the size and the reticle sits lower than the *visual* centre -- lower, never at the feet. There
//! is no better source; this is the same ceiling the camera layer works under.

// Pure arithmetic; ungated so its tests run on the host with no game running.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::camera::geometry::MAX_PLAUSIBLE_HEIGHT;

/// Smallest height that is a creature rather than a broken row. The shortest thing in the shipped
/// regulation is around 0.6 m; anything at or below zero is a read that did not work.
pub(crate) const MIN_PLAUSIBLE_HEIGHT: f32 = 0.05;

/// The widest the body is allowed to be scaled, in either direction.
///
/// The tallest creature the shipped regulation contains is 59 m (`c4450`), which against a 1.5 m
/// player is a ratio of about 39. The band is set wider than that on purpose -- it exists to catch
/// a modded or corrupt row, not to second-guess a real one.
pub(crate) const MAX_SCALE: f32 = 64.0;
/// ...and the other end, for the case where the possessed thing is smaller than the player.
pub(crate) const MIN_SCALE: f32 = 1.0 / MAX_SCALE;

/// The uniform scale that puts the possessing player's body at the creature's size.
///
/// `None` when either height is unreadable or implausible, which is a real and ordinary outcome:
/// `CSChrPhysicsModule.hitHeight` is populated from `NpcParam` by `InitForEnemy`, so a character
/// whose param row has not been applied yet reads zero. The caller leaves the body alone in that
/// case rather than scaling it by a number derived from a failed read.
#[must_use]
pub(crate) fn scale_for(creature_height: f32, player_height: f32) -> Option<f32> {
    let plausible = |height: f32| {
        height.is_finite() && (MIN_PLAUSIBLE_HEIGHT..=MAX_PLAUSIBLE_HEIGHT).contains(&height)
    };
    if !plausible(creature_height) || !plausible(player_height) {
        return None;
    }
    Some((creature_height / player_height).clamp(MIN_SCALE, MAX_SCALE))
}

/// Is this scale different enough from the one already in force to be worth writing?
///
/// Possessing something player-sized must be a genuine no-op rather than a write of `1.0000001`,
/// because "the body was scaled" is a thing the release path then has to undo and the log then has
/// to explain. A tenth of a percent is far below anything the anchor can express on screen.
#[must_use]
pub(crate) fn worth_applying(scale: f32) -> bool {
    (scale - 1.0).abs() > 0.001
}

/// The scale to write, given what the body is already carrying.
///
/// # Y only, and that is the deliberate half of this module
///
/// The lock-gaze dummies sit on the model's centreline, so scaling `X` and `Z` moves the anchor by
/// approximately nothing -- it buys no part of the fix. What it does buy is a second full-size
/// body inside the creature for every render-side consumer of the model matrix to deal with:
/// the shadow pass, the exported AABB (`CSFD4LocationGxModelMatricesAndAabbExporter`), culling,
/// and every VFX attach point. A vertical stretch is the smallest change that reaches the anchor,
/// so it is the one taken. The field is three separate floats and `SetScaleSize` takes a vector,
/// so a non-uniform value is what the layout is for rather than a trick played on it.
///
/// # Multiplied, not assigned
///
/// `original` is whatever the body was carrying -- `1.0` on a stock character, but not necessarily
/// on one another mod has already scaled. Composing rather than overwriting means this layer says
/// "twice as tall as it was" instead of "this tall, and never mind what you wanted", and the
/// release restores `original` either way.
#[must_use]
pub(crate) fn worn_scale(original: [f32; 3], scale: f32) -> [f32; 3] {
    [original[0], original[1] * scale, original[2]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::geometry::PLAYER_HIT_HEIGHT;

    /// The whole point, stated as the case the user reported: a 12 m Flying Dragon.
    ///
    /// `NpcParam` gives `c4500` a `hitHeight` of 12.0 and the player 1.5, so the body is worn
    /// eight times its own size and its lock-gaze dummy rises by the same factor. Whatever
    /// fraction `f` of its height that dummy sits at, it moves from `1.5 * f` to `12.0 * f` --
    /// which is the same fraction of the dragon.
    #[test]
    fn a_flying_dragon_scales_the_body_by_eight() {
        let scale = scale_for(12.0, PLAYER_HIT_HEIGHT).expect("both heights are plausible");
        assert!((scale - 8.0).abs() < 1e-6, "{scale}");
        assert!(worth_applying(scale));
    }

    /// Possessing something player-sized changes nothing, and that is an invariant rather than a
    /// coincidence: the reticle is already in the right place, so the body must not be touched and
    /// the release must have nothing to undo.
    #[test]
    fn a_body_the_players_own_size_is_left_alone() {
        let scale = scale_for(PLAYER_HIT_HEIGHT, PLAYER_HIT_HEIGHT).expect("plausible");
        assert!((scale - 1.0).abs() < f32::EPSILON, "{scale}");
        assert!(!worth_applying(scale));
    }

    /// A creature smaller than the player shrinks the body, rather than being clamped to 1.0.
    ///
    /// Wearing a rat (`hitHeight` 1.0) and locking on should put the reticle on the rat, not a
    /// head-height above it.
    #[test]
    fn a_creature_smaller_than_the_player_shrinks_the_body() {
        let scale = scale_for(1.0, PLAYER_HIT_HEIGHT).expect("plausible");
        assert!(scale < 1.0, "{scale}");
        assert!(worth_applying(scale));
    }

    /// A failed read is not a scale of zero. `hitHeight` reads zero until `InitForEnemy` has
    /// applied the `NpcParam` row, and a zero-scaled body is a body collapsed onto a point.
    #[test]
    fn an_unreadable_height_refuses_rather_than_collapsing_the_body() {
        assert_eq!(scale_for(0.0, PLAYER_HIT_HEIGHT), None);
        assert_eq!(scale_for(12.0, 0.0), None);
        assert_eq!(scale_for(f32::NAN, PLAYER_HIT_HEIGHT), None);
        assert_eq!(scale_for(12.0, f32::NAN), None);
        assert_eq!(scale_for(f32::INFINITY, PLAYER_HIT_HEIGHT), None);
        assert_eq!(scale_for(-3.0, PLAYER_HIT_HEIGHT), None);
    }

    /// The write is vertical and nothing else. `X` and `Z` come back exactly as they went in,
    /// whatever they were -- the anchor is on the centreline, so widening the body would be pure
    /// cost to the shadow pass and the exported AABB.
    #[test]
    fn only_the_vertical_axis_is_touched_and_it_composes_with_what_was_there() {
        assert_eq!(worn_scale([1.0, 1.0, 1.0], 8.0), [1.0, 8.0, 1.0]);
        // A body another mod had already scaled is multiplied, not overwritten.
        assert_eq!(worn_scale([2.0, 3.0, 2.0], 4.0), [2.0, 12.0, 2.0]);
    }

    /// The tallest creature the shipped regulation contains is inside the band, so the clamp is
    /// reachable only by a modded or corrupt row.
    #[test]
    fn the_tallest_shipped_creature_is_not_clamped() {
        let tallest = 59.0;
        let scale = scale_for(tallest, PLAYER_HIT_HEIGHT).expect("plausible");
        assert!(
            (scale - tallest / PLAYER_HIT_HEIGHT).abs() < 1e-4,
            "{scale}"
        );
        assert!(scale < MAX_SCALE, "{scale}");
    }

    /// ...and a row beyond anything the game contains is clamped rather than refused, matching
    /// what [`crate::camera::geometry`] does with the same input.
    #[test]
    fn an_implausible_height_is_refused_and_an_extreme_ratio_is_clamped() {
        assert_eq!(
            scale_for(MAX_PLAUSIBLE_HEIGHT + 1.0, PLAYER_HIT_HEIGHT),
            None
        );
        // Plausible heights whose ratio is extreme: a 200 m subject worn by a 0.05 m body.
        let scale = scale_for(MAX_PLAUSIBLE_HEIGHT, MIN_PLAUSIBLE_HEIGHT).expect("plausible");
        assert!((scale - MAX_SCALE).abs() < f32::EPSILON, "{scale}");
    }
}
