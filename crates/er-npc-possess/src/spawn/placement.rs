//! How far in front of the player a spawned creature goes, once its own size is known.
//!
//! # The defect this exists to close
//!
//! `[spawn].distance_m` was the whole answer, and it is one number for every creature from a
//! Ballista to a Fallingstar Beast. Measured, from the run that produced this module: c6310's
//! physics capsule is `hitRadius` **4.00 m** (`NpcParam 63100000`, and the same value the camera
//! layer read off `CSChrPhysicsModule+0x344` that session), and `distance_m` was **3.0**. So the
//! creature was created with the player already a metre inside its own capsule. Eight other
//! possessions in that same session placed creatures whose capsules were 0.80-2.70 m tall and
//! none of them overlapped; the one that did is the one the player did not walk away from.
//!
//! # `distance_m` is now a floor, not the answer
//!
//! The placed distance is `max(distance_m, creature_radius + player_radius + `[`CLEARANCE_M`]`)`,
//! so a small creature still lands exactly where the file says and a large one is pushed out until
//! its capsule clears the player's. The config file says so in the comment beside the key; a
//! player who wants a creature closer than its own body allows cannot have it, and that is the
//! point rather than a limitation.
//!
//! # Why the radius and not the height
//!
//! `hitHeight` is what the camera layer sizes itself on, because the camera cares how tall the
//! subject is. Overlap is horizontal: two capsules intersect when their centres are closer than
//! the sum of their radii, whatever their heights. `+0x344` is the field that decides it.
//!
//! # The chicken and egg, and where it is actually resolved
//!
//! A creature's `CSChrPhysicsModule` does not exist until `CSChrPhysicsModule::InitForEnemy` has
//! run, which is long after `SpawnDynamicChr` returns -- so the radius cannot be read when the
//! request is built. It does not need to be: the request's position field is not read on the
//! creature path at all (see [`crate::possess::layout::chr_spawn_request`]), and the creature is
//! actually put somewhere by `finish_spawn`, once it is drivable. That is 166-249 ms later on the
//! nine spawns in the reference log, and by then the capsule reads. So this arithmetic runs at
//! placement time, on the real number, and never on a guess.
//!
//! Everything here is pure, so `cargo test` proves it on the host with no game running.

// Pure arithmetic; it stays ungated so its tests run on the host.
#![cfg_attr(not(windows), allow(dead_code))]

/// The gap left between the two capsules, in metres, on top of both radii.
///
/// One metre: far enough that a creature which settles or takes one step on its first frame does
/// not immediately re-enclose the player, and short enough that the creature is still obviously
/// "in front of you" rather than across the clearing.
pub(crate) const CLEARANCE_M: f32 = 1.0;

/// The furthest a creature may be pushed out, in metres.
///
/// The same fifty metres `crate::possess::game::MAX_TARGET_DISTANCE_SQUARED` bounds a target search
/// at, and for the same reason: a creature further away than the mod would ever look for one is not
/// a spawn the player can make sense of. Nothing in the shipped `NpcParam` gets near it -- the
/// largest `hitRadius` is well under twenty -- so this is a bound on the arithmetic rather than on
/// any real creature.
pub(crate) const MAX_DISTANCE_M: f32 = 50.0;

/// The largest capsule radius that is believed rather than discarded, in metres.
///
/// A physics module that has not finished initialising, or a pointer chain that landed somewhere
/// wrong, reads as a float rather than as a failure. Anything outside `(0, 25]` is treated as
/// unreadable, which falls back to the configured distance instead of hurling the creature to the
/// clamp.
const MAX_BELIEVABLE_RADIUS_M: f32 = 25.0;

/// Where the creature goes, and enough of why for the log line to say it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placement {
    /// Metres in front of the player.
    pub(crate) distance_m: f32,
    /// The creature's own capsule radius, or `None` when it did not read believably.
    pub(crate) creature_radius_m: Option<f32>,
    /// The player's, likewise. `None` costs the player's own half-width and nothing else.
    pub(crate) player_radius_m: Option<f32>,
    /// Did the capsules push the creature past `[spawn].distance_m`?
    pub(crate) widened: bool,
}

impl Placement {
    /// One clause for the `became drivable` log line.
    pub(crate) fn describe(&self) -> String {
        match (self.creature_radius_m, self.widened) {
            (None, _) => format!(
                "at the configured {:.2} m -- its capsule radius did not read, so nothing could be \
                 cleared",
                self.distance_m
            ),
            (Some(creature), false) => format!(
                "at the configured {:.2} m, which already clears its {creature:.2} m capsule",
                self.distance_m
            ),
            (Some(creature), true) => format!(
                "at {:.2} m rather than the configured distance, to clear its {creature:.2} m \
                 capsule plus your own {:.2} m and {CLEARANCE_M:.2} m of gap",
                self.distance_m,
                self.player_radius_m.unwrap_or(0.0)
            ),
        }
    }
}

/// A radius that is safe to do arithmetic with, or `None`.
fn believable(radius: Option<f32>) -> Option<f32> {
    radius.filter(|r| r.is_finite() && *r > 0.0 && *r <= MAX_BELIEVABLE_RADIUS_M)
}

/// Decide the placement distance.
///
/// `configured_m` is `[spawn].distance_m`, already bounded to 1..50 by the settings validator; it
/// is re-sanitised here anyway, because this function is the last thing between a number and a
/// character being put on top of the player.
#[must_use]
pub(crate) fn place(
    configured_m: f32,
    creature_radius_m: Option<f32>,
    player_radius_m: Option<f32>,
) -> Placement {
    let creature_radius_m = believable(creature_radius_m);
    let player_radius_m = believable(player_radius_m);
    // A configured distance that is not a usable number is treated as no floor at all rather than
    // poisoning the max: the capsules still get cleared, which is the part that matters.
    let configured = if configured_m.is_finite() && configured_m > 0.0 {
        configured_m.min(MAX_DISTANCE_M)
    } else {
        0.0
    };
    let Some(creature) = creature_radius_m else {
        return Placement {
            distance_m: configured,
            creature_radius_m: None,
            player_radius_m,
            widened: false,
        };
    };
    let needed = (creature + player_radius_m.unwrap_or(0.0) + CLEARANCE_M).min(MAX_DISTANCE_M);
    let distance_m = configured.max(needed);
    Placement {
        distance_m,
        creature_radius_m: Some(creature),
        player_radius_m,
        widened: distance_m > configured,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The measured case. c6310 `hitRadius` 4.00 m, the player's own capsule, and the shipped
    /// `distance_m = 3.0` that put the creature on top of the player.
    #[test]
    fn the_fallingstar_beast_is_pushed_out_past_its_own_capsule() {
        let placed = place(3.0, Some(4.0), Some(0.45));
        assert!(placed.widened);
        // 4.00 + 0.45 + 1.00, and comfortably outside the 4.00 m the old answer sat inside.
        assert!((placed.distance_m - 5.45).abs() < 1e-4, "{placed:?}");
        assert!(placed.distance_m > 4.0 + 0.45);
        assert!(placed.describe().contains("4.00 m capsule"));
    }

    /// ...and a small creature is still put exactly where the file says, because the floor only
    /// ever raises. Every other creature in the reference session was in this case.
    #[test]
    fn a_small_creature_lands_at_the_configured_distance() {
        let placed = place(3.0, Some(0.5), Some(0.45));
        assert!(!placed.widened);
        assert!((placed.distance_m - 3.0).abs() < 1e-6);
        assert!(placed.describe().contains("already clears"));
    }

    /// The boundary is the whole point, so it is pinned: a creature whose capsule reaches exactly
    /// to the configured distance is still pushed out, because touching is overlapping once the
    /// player has a radius of their own.
    #[test]
    fn a_capsule_that_exactly_reaches_the_configured_distance_is_still_widened() {
        let placed = place(3.0, Some(3.0), Some(0.0));
        assert!(placed.widened);
        assert!(placed.distance_m > 3.0);
    }

    /// An unreadable radius must fall back to the configured distance rather than to a guess. This
    /// is the case on a half-built physics module, and inventing a size for it would be worse than
    /// the behaviour that shipped.
    #[test]
    fn an_unreadable_creature_radius_keeps_the_configured_distance() {
        for radius in [None, Some(f32::NAN), Some(0.0), Some(-2.0), Some(1.0e9)] {
            let placed = place(3.0, radius, Some(0.45));
            assert!((placed.distance_m - 3.0).abs() < 1e-6, "{radius:?}");
            assert_eq!(placed.creature_radius_m, None, "{radius:?}");
            assert!(!placed.widened);
            assert!(placed.describe().contains("did not read"));
        }
    }

    /// An unreadable player radius still clears the creature's own capsule. Losing the player's
    /// half-width costs less than half a metre and the clearance covers it; refusing to widen at
    /// all would put the creature back inside them.
    #[test]
    fn an_unreadable_player_radius_still_clears_the_creature() {
        let placed = place(3.0, Some(4.0), None);
        assert!(placed.widened);
        assert!((placed.distance_m - 5.0).abs() < 1e-4, "{placed:?}");
        assert!(placed.distance_m > 4.0);
    }

    /// Nothing may come back non-finite or unbounded, whatever it is fed -- this number is handed
    /// to `intent::ahead_of` and then written into the engine's own proxy request.
    #[test]
    fn every_answer_is_finite_and_inside_the_bound() {
        for configured in [f32::NAN, f32::INFINITY, -1.0, 0.0, 1.0, 3.0, 50.0, 1.0e9] {
            for creature in [None, Some(0.5), Some(4.0), Some(24.0), Some(f32::INFINITY)] {
                for player in [None, Some(0.45), Some(f32::NAN)] {
                    let placed = place(configured, creature, player);
                    assert!(
                        placed.distance_m.is_finite(),
                        "{configured} {creature:?} {player:?}"
                    );
                    assert!(placed.distance_m >= 0.0);
                    assert!(
                        placed.distance_m <= MAX_DISTANCE_M,
                        "{configured} {creature:?} {player:?} -> {}",
                        placed.distance_m
                    );
                }
            }
        }
    }

    /// The widened flag has to mean what the log line says it means, or the report lies about
    /// which of the two numbers was used.
    #[test]
    fn widened_is_true_exactly_when_the_capsules_beat_the_configured_distance() {
        assert!(!place(10.0, Some(1.0), Some(0.45)).widened);
        assert!(place(1.0, Some(1.0), Some(0.45)).widened);
        // The largest believable capsule still lands inside the bound, and is still reported as
        // widened rather than as "the configured distance already cleared it".
        let biggest = place(1.0, Some(MAX_BELIEVABLE_RADIUS_M), Some(0.45));
        assert!(biggest.widened);
        assert!((biggest.distance_m - (MAX_BELIEVABLE_RADIUS_M + 0.45 + CLEARANCE_M)).abs() < 1e-4);
        assert!(biggest.distance_m < MAX_DISTANCE_M);
        // ...and a configured distance above the bound is itself clamped, so nothing can ask for a
        // spawn further away than the mod would ever search for one.
        assert!((place(1.0e6, Some(1.0), Some(0.45)).distance_m - MAX_DISTANCE_M).abs() < 1e-4);
    }
}
