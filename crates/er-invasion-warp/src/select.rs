//! Choosing WHICH invasion-spawn point to warp to.
//!
//! The catalog ([`crate::invasion_warp`]) answers "what points exist"; this module answers
//! "which one does the player want". Both surfaces the feature can grow need exactly this:
//! a hotkey warp needs *nearest* / *next*, and the world-map UI needs the same ranking to
//! decide which pin the cursor lands on and to label distances.
//!
//! # Why this module is pure
//!
//! Everything here operates on [`ResolvedTarget`] -- a catalog target that has ALREADY been
//! converted to physics-space coordinates. The conversion itself is an engine call
//! (`ConvertBlockCoordsToPhysicsCoords` @ `0x14061e120`, byte-checked `MATCH ... shift 0`),
//! because [`InvasionWarpTarget::world_position`] needs a block origin the crate cannot
//! resolve on its own. Keeping the conversion at the call site and the *ranking* in here means
//! the ranking is unit-testable with no game, which is the only part of the warp that can be
//! proven offline.
//!
//! # Fail-closed by construction
//!
//! `ConvertBlockCoordsToPhysicsCoords` returns `false` when the target block's world info is
//! not resident. Those targets must never become [`ResolvedTarget`]s: a point we cannot place
//! is a point we must not warp to. This module therefore ranks only what the caller could
//! convert, and an empty input yields `None` rather than a fallback guess.

use crate::invasion_warp::InvasionWarpTarget;

/// A catalog target whose physics-space position is known.
///
/// Built by the caller from a successful `ConvertBlockCoordsToPhysicsCoords` call; a failed
/// conversion must be dropped, not defaulted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedTarget {
    /// The catalog entry this came from, block-local position and all.
    pub target: InvasionWarpTarget,
    /// Physics-space `[x, y, z]`, as the engine's own conversion produced it.
    pub world_position: [f32; 3],
}

impl ResolvedTarget {
    #[must_use]
    pub const fn new(target: InvasionWarpTarget, world_position: [f32; 3]) -> Self {
        Self {
            target,
            world_position,
        }
    }

    /// Ordering-independent identity, forwarded from the catalog entry.
    #[must_use]
    pub const fn stable_id(&self) -> u64 {
        self.target.stable_id()
    }
}

/// How close counts as "already there".
///
/// Cycling with [`next_from`] would otherwise re-select the point the player is standing on
/// every time, and "warp to nearest" would be a no-op after the first press. 8 metres is
/// comfortably outside the arrival tolerance the warp oracle uses
/// ([`crate::oracles::INVASION_WARP_POSITION_TOLERANCE_METRES`], 5.0) so a point that a
/// previous warp legitimately landed on is excluded rather than re-picked.
pub const SAME_POINT_RADIUS_METRES: f32 = 8.0;

/// Squared euclidean distance. Squared throughout: ranking never needs the square root, and
/// skipping it keeps the comparison exact rather than rounding through `sqrt`.
#[must_use]
pub fn distance_squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

/// Index of the target closest to `from`, ignoring anything within
/// [`SAME_POINT_RADIUS_METRES`] and anything with a non-finite coordinate.
///
/// Non-finite coordinates are dropped rather than propagated: a NaN would poison every
/// comparison it took part in (`NaN < x` and `NaN > x` are both false), silently making the
/// winner depend on iteration order.
///
/// Ties break toward the lower index, which makes the result deterministic for a sorted
/// catalog -- the catalog IS sorted, so a tie is two genuinely coincident points and the
/// caller gets the same answer every run.
#[must_use]
pub fn nearest_from(resolved: &[ResolvedTarget], from: [f32; 3]) -> Option<usize> {
    if !from.iter().all(|c| c.is_finite()) {
        return None;
    }
    let exclusion = SAME_POINT_RADIUS_METRES * SAME_POINT_RADIUS_METRES;
    let mut best: Option<(usize, f32)> = None;
    for (index, candidate) in resolved.iter().enumerate() {
        if !candidate.world_position.iter().all(|c| c.is_finite()) {
            continue;
        }
        let distance = distance_squared(candidate.world_position, from);
        if distance <= exclusion {
            continue;
        }
        match best {
            Some((_, best_distance)) if distance >= best_distance => {}
            _ => best = Some((index, distance)),
        }
    }
    best.map(|(index, _)| index)
}

/// Index of the next target in a stable cycle, for a "step to the following spawn point"
/// affordance.
///
/// The cycle is ordered by [`ResolvedTarget::stable_id`] rather than by distance, because a
/// distance-ordered cycle re-ranks itself after every warp and so never terminates: each hop
/// changes the player's position, which changes the ordering, which can send the cursor back
/// where it came from. A stable-id cycle visits every point exactly once per lap regardless of
/// where the player currently is.
///
/// `current` is the id last warped to; `None` starts the cycle at the lowest id. An id that is
/// no longer present (its block unloaded) restarts at the first id greater than it, so
/// unloading a block skips forward rather than resetting to the beginning.
#[must_use]
pub fn next_from(resolved: &[ResolvedTarget], current: Option<u64>) -> Option<usize> {
    if resolved.is_empty() {
        return None;
    }
    let Some(current) = current else {
        return lowest_id(resolved);
    };
    let mut best: Option<(usize, u64)> = None;
    for (index, candidate) in resolved.iter().enumerate() {
        let id = candidate.stable_id();
        if id <= current {
            continue;
        }
        match best {
            Some((_, best_id)) if id >= best_id => {}
            _ => best = Some((index, id)),
        }
    }
    // Past the highest id: wrap to the start of the lap.
    best.map(|(index, _)| index).or_else(|| lowest_id(resolved))
}

/// [`next_from`] over RAW catalog targets, which is the form that matters for crossing areas.
///
/// The cycle orders by [`InvasionWarpTarget::stable_id`] and needs no world coordinates at all,
/// so it must NOT be restricted to targets the engine could convert. Restricting it was a real
/// bug: `ConvertBlockCoordsToPhysicsCoords` only resolves blocks in the player's currently
/// resident area, so filtering the candidate set by it silently made every other area
/// unreachable -- even though the warp itself never needs the conversion (the explicit-spawn
/// slot takes BLOCK-LOCAL coordinates and `MoveMapStep` converts them after the destination
/// loads).
#[must_use]
pub fn next_target_from(targets: &[InvasionWarpTarget], current: Option<u64>) -> Option<usize> {
    if targets.is_empty() {
        return None;
    }
    let Some(current) = current else {
        return lowest_target_id(targets);
    };
    let mut best: Option<(usize, u64)> = None;
    for (index, candidate) in targets.iter().enumerate() {
        let id = candidate.stable_id();
        if id <= current {
            continue;
        }
        match best {
            Some((_, best_id)) if id >= best_id => {}
            _ => best = Some((index, id)),
        }
    }
    best.map(|(index, _)| index)
        .or_else(|| lowest_target_id(targets))
}

/// The first target belonging to an area other than `current_area`.
///
/// Deterministic because the catalog is sorted by block then point index. This exists to make
/// "can the warp leave the area it is standing in" a single decidable action rather than a
/// property inferred from a filtered candidate count.
#[must_use]
pub fn first_in_other_area(targets: &[InvasionWarpTarget], current_area: u8) -> Option<usize> {
    targets
        .iter()
        .position(|target| target.block.area() != current_area)
}

fn lowest_target_id(targets: &[InvasionWarpTarget]) -> Option<usize> {
    let mut best: Option<(usize, u64)> = None;
    for (index, candidate) in targets.iter().enumerate() {
        let id = candidate.stable_id();
        match best {
            Some((_, best_id)) if id >= best_id => {}
            _ => best = Some((index, id)),
        }
    }
    best.map(|(index, _)| index)
}

fn lowest_id(resolved: &[ResolvedTarget]) -> Option<usize> {
    let mut best: Option<(usize, u64)> = None;
    for (index, candidate) in resolved.iter().enumerate() {
        let id = candidate.stable_id();
        match best {
            Some((_, best_id)) if id >= best_id => {}
            _ => best = Some((index, id)),
        }
    }
    best.map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invasion_warp::BlockKey;

    fn target(block: BlockKey, point_index: u32, world: [f32; 3]) -> ResolvedTarget {
        ResolvedTarget::new(
            InvasionWarpTarget::new(block, point_index, [0.0, 0.0, 0.0], 0.0),
            world,
        )
    }

    fn block(index: u8) -> BlockKey {
        BlockKey::from_parts(60, 34, 51, index)
    }

    #[test]
    fn nearest_picks_the_closest_target_outside_the_same_point_radius() {
        let resolved = [
            target(block(0), 0, [100.0, 0.0, 0.0]),
            target(block(0), 1, [20.0, 0.0, 0.0]),
            target(block(0), 2, [50.0, 0.0, 0.0]),
        ];
        assert_eq!(nearest_from(&resolved, [0.0, 0.0, 0.0]), Some(1));
    }

    #[test]
    fn nearest_skips_the_point_the_player_is_standing_on() {
        // Without the exclusion this returns index 0 forever and the hotkey does nothing.
        let resolved = [
            target(block(0), 0, [1.0, 0.0, 0.0]),
            target(block(0), 1, [40.0, 0.0, 0.0]),
        ];
        assert_eq!(nearest_from(&resolved, [0.0, 0.0, 0.0]), Some(1));
    }

    #[test]
    fn nearest_excludes_exactly_at_the_radius_and_admits_just_beyond_it() {
        let inside = [target(block(0), 0, [SAME_POINT_RADIUS_METRES, 0.0, 0.0])];
        assert_eq!(nearest_from(&inside, [0.0, 0.0, 0.0]), None);
        let outside = [target(
            block(0),
            0,
            [SAME_POINT_RADIUS_METRES + 0.5, 0.0, 0.0],
        )];
        assert_eq!(nearest_from(&outside, [0.0, 0.0, 0.0]), Some(0));
    }

    #[test]
    fn nearest_drops_non_finite_candidates_instead_of_letting_them_win() {
        // A NaN loses every comparison, so an unguarded scan can return it purely by order.
        let resolved = [
            target(block(0), 0, [f32::NAN, 0.0, 0.0]),
            target(block(0), 1, [90.0, 0.0, 0.0]),
        ];
        assert_eq!(nearest_from(&resolved, [0.0, 0.0, 0.0]), Some(1));
    }

    #[test]
    fn nearest_rejects_a_non_finite_origin_rather_than_guessing() {
        let resolved = [target(block(0), 0, [10.0, 0.0, 0.0])];
        assert_eq!(nearest_from(&resolved, [f32::NAN, 0.0, 0.0]), None);
    }

    #[test]
    fn nearest_of_nothing_is_none() {
        assert_eq!(nearest_from(&[], [0.0, 0.0, 0.0]), None);
    }

    #[test]
    fn nearest_breaks_ties_toward_the_lower_index() {
        let resolved = [
            target(block(0), 0, [30.0, 0.0, 0.0]),
            target(block(0), 1, [30.0, 0.0, 0.0]),
        ];
        assert_eq!(nearest_from(&resolved, [0.0, 0.0, 0.0]), Some(0));
    }

    #[test]
    fn the_cycle_starts_at_the_lowest_stable_id_not_at_index_zero() {
        let resolved = [
            target(block(5), 0, [0.0, 0.0, 0.0]),
            target(block(1), 0, [0.0, 0.0, 0.0]),
        ];
        assert_eq!(next_from(&resolved, None), Some(1));
    }

    #[test]
    fn the_cycle_advances_to_the_next_higher_id() {
        let resolved = [
            target(block(1), 0, [0.0, 0.0, 0.0]),
            target(block(1), 1, [0.0, 0.0, 0.0]),
            target(block(2), 0, [0.0, 0.0, 0.0]),
        ];
        let first = resolved[0].stable_id();
        assert_eq!(next_from(&resolved, Some(first)), Some(1));
    }

    #[test]
    fn the_cycle_wraps_after_the_highest_id() {
        let resolved = [
            target(block(1), 0, [0.0, 0.0, 0.0]),
            target(block(2), 0, [0.0, 0.0, 0.0]),
        ];
        let highest = resolved[1].stable_id();
        assert_eq!(next_from(&resolved, Some(highest)), Some(0));
    }

    #[test]
    fn a_vanished_id_skips_forward_rather_than_restarting_the_lap() {
        // Block unloaded under us: the id we were on is gone from the list entirely.
        let resolved = [
            target(block(1), 0, [0.0, 0.0, 0.0]),
            target(block(9), 0, [0.0, 0.0, 0.0]),
        ];
        let missing = target(block(4), 0, [0.0, 0.0, 0.0]).stable_id();
        assert_eq!(next_from(&resolved, Some(missing)), Some(1));
    }

    #[test]
    fn the_cycle_of_nothing_is_none() {
        assert_eq!(next_from(&[], None), None);
        assert_eq!(next_from(&[], Some(0)), None);
    }

    #[test]
    fn a_full_lap_visits_every_target_exactly_once() {
        // The property that motivates ordering by id instead of by distance.
        let resolved = [
            target(block(3), 1, [0.0, 0.0, 0.0]),
            target(block(1), 0, [0.0, 0.0, 0.0]),
            target(block(3), 0, [0.0, 0.0, 0.0]),
            target(block(2), 7, [0.0, 0.0, 0.0]),
        ];
        let mut seen = Vec::new();
        let mut current = None;
        for _ in 0..resolved.len() {
            let index = next_from(&resolved, current).expect("cycle yields a target");
            assert!(
                !seen.contains(&index),
                "revisited index {index} within one lap"
            );
            seen.push(index);
            current = Some(resolved[index].stable_id());
        }
        assert_eq!(seen.len(), resolved.len());
        // And the lap closes back onto its own start.
        let wrapped = next_from(&resolved, current).expect("cycle wraps");
        assert_eq!(wrapped, seen[0]);
    }

    #[test]
    fn the_raw_target_cycle_matches_the_resolved_one_when_everything_resolves() {
        let raw = [
            InvasionWarpTarget::new(block(3), 1, [0.0; 3], 0.0),
            InvasionWarpTarget::new(block(1), 0, [0.0; 3], 0.0),
            InvasionWarpTarget::new(block(2), 7, [0.0; 3], 0.0),
        ];
        let resolved: Vec<_> = raw
            .iter()
            .map(|t| ResolvedTarget::new(*t, [0.0; 3]))
            .collect();
        assert_eq!(next_target_from(&raw, None), next_from(&resolved, None));
        let first = raw[1].stable_id();
        assert_eq!(
            next_target_from(&raw, Some(first)),
            next_from(&resolved, Some(first))
        );
    }

    #[test]
    fn the_cross_area_jump_finds_a_target_outside_the_current_area() {
        // The case that matters live: standing in the DLC (61), reach the base game (60).
        let targets = [
            InvasionWarpTarget::new(BlockKey::from_parts(61, 49, 41, 0), 0, [0.0; 3], 0.0),
            InvasionWarpTarget::new(BlockKey::from_parts(61, 50, 41, 0), 0, [0.0; 3], 0.0),
            InvasionWarpTarget::new(BlockKey::from_parts(60, 33, 40, 0), 0, [0.0; 3], 0.0),
        ];
        assert_eq!(first_in_other_area(&targets, 61), Some(2));
        assert_eq!(first_in_other_area(&targets, 60), Some(0));
    }

    #[test]
    fn a_single_area_catalog_has_no_cross_area_target() {
        let targets = [InvasionWarpTarget::new(block(0), 0, [0.0; 3], 0.0)];
        assert_eq!(first_in_other_area(&targets, 60), None);
    }

    #[test]
    fn distance_squared_is_symmetric_and_zero_on_itself() {
        let a = [1.0, 2.0, 3.0];
        let b = [-4.0, 5.5, 0.25];
        assert_eq!(distance_squared(a, b), distance_squared(b, a));
        assert_eq!(distance_squared(a, a), 0.0);
    }
}
