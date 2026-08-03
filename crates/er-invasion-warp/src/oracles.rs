//! The RAM/pixel semaphores the invasion-warp feature must go green on to be PROVEN.
//!
//! AGENTS.md is explicit: a rendered/behavioural feature is never proven by build success,
//! launch success, "no crash", hook counters, or "the draw task ran". So the oracles are
//! designed HERE, before any runtime exists, and this module is the single place their names
//! and pass conditions are written down.
//!
//! These are NAMES and CONTRACTS, not counters. There is deliberately no `AtomicUsize`
//! behind any of them yet: a counter that is read to emit an oracle but never written
//! reports 0 forever and actively misinforms (`scripts/check-oracle-writers.py`). Each
//! atomic lands in the same change that first writes it.
//!
//! # The proof chain
//!
//! Five oracles, in the order a run must satisfy them. A run that stops early is NEGATIVE or
//! UNPROVEN evidence, never product proof.
//!
//! 1. [`ORACLE_INVASION_WARP_CATALOG_TARGETS`] / [`ORACLE_INVASION_WARP_CATALOG_BLOCKS`] --
//!    the catalog was actually read out of the live `CSAutoInvadePoint`. Pass condition is an
//!    EXACT match against the shipped fingerprints, not "> 0": with only the base container
//!    mounted the totals are 257 blocks / 4482 points, and with `_dlc02` as well 365 / 7073
//!    (`crate::aip::AIP_FINGERPRINT_BASE`, `AIP_FINGERPRINT_DLC02`). A smaller number means
//!    the read raced the loader; a larger one means it double-counted.
//! 2. [`ORACLE_INVASION_WARP_LIST_ROWS`] -- the world-map warp list actually held that many
//!    invasion rows while the dialog was open. Distinguishes "the catalog built" from "the
//!    catalog reached the UI".
//! 3. [`ORACLE_INVASION_WARP_SELECTED_ID`] -- the target identity under the cursor at confirm
//!    time, as [`crate::InvasionWarpTarget::stable_id`]. Must equal the id of the row the
//!    driver moved to; proves the selection index maps to the intended target rather than to
//!    a `BonfireWarpParam` row that happens to sit at the same index.
//! 4. [`ORACLE_INVASION_WARP_REQUESTED_BLOCK`] / [`ORACLE_INVASION_WARP_REQUESTED_POSITION`]
//!    / [`ORACLE_INVASION_WARP_REQUESTED_YAW`] -- what the warp was asked to do. Must equal
//!    the selected target's block and its `world_position(block_origin)`.
//! 5. [`ORACLE_INVASION_WARP_FINAL_BLOCK`] / [`ORACLE_INVASION_WARP_FINAL_POSITION`] -- where
//!    the local player actually ENDED UP, read back from the player instance after the warp
//!    settled. This is the direct objective measurement; 1-4 only prove the request was
//!    formed. Pass condition: same block, and position within
//!    [`INVASION_WARP_POSITION_TOLERANCE_METRES`] of the requested one.
//!
//! # The negative oracle that must stay at zero
//!
//! [`ORACLE_INVASION_WARP_SESSION_TOUCHES`] counts any entry into a session/multiplayer path
//! from this feature. The user's hard boundary is that the feature never fakes an invasion,
//! so "we did not start a session" has to be MEASURED, not asserted. Any non-zero value fails
//! the run outright regardless of how the other five look.
//!
//! [`ORACLE_INVASION_WARP_MSGBOX_BUILDS`] is the standing repo-wide rule restated for this
//! feature: product proof requires zero `CS::MessageBoxDialog` builds.

/// Targets in the catalog built from the live singleton.
pub const ORACLE_INVASION_WARP_CATALOG_TARGETS: &str = "oracle_invasion_warp_catalog_targets";
/// Distinct blocks in that catalog.
pub const ORACLE_INVASION_WARP_CATALOG_BLOCKS: &str = "oracle_invasion_warp_catalog_blocks";
/// Distinct map areas in that catalog (2 once both containers are mounted).
pub const ORACLE_INVASION_WARP_CATALOG_AREAS: &str = "oracle_invasion_warp_catalog_areas";
/// Invasion rows present in the world-map warp list while the dialog was open.
pub const ORACLE_INVASION_WARP_LIST_ROWS: &str = "oracle_invasion_warp_list_rows";
/// `InvasionWarpTarget::stable_id` of the row under the cursor at confirm.
pub const ORACLE_INVASION_WARP_SELECTED_ID: &str = "oracle_invasion_warp_selected_id";
/// Raw `BlockId` the warp was requested for.
pub const ORACLE_INVASION_WARP_REQUESTED_BLOCK: &str = "oracle_invasion_warp_requested_block";
/// Requested world-space position, as three millimetre-scaled integers.
pub const ORACLE_INVASION_WARP_REQUESTED_POSITION: &str = "oracle_invasion_warp_requested_position";
/// Requested facing, in milliradians.
pub const ORACLE_INVASION_WARP_REQUESTED_YAW: &str = "oracle_invasion_warp_requested_yaw";
/// Raw `BlockId` the local player occupied after the warp settled.
pub const ORACLE_INVASION_WARP_FINAL_BLOCK: &str = "oracle_invasion_warp_final_block";
/// World-space position the local player occupied after the warp settled.
pub const ORACLE_INVASION_WARP_FINAL_POSITION: &str = "oracle_invasion_warp_final_position";
/// MUST STAY ZERO: entries into any session/multiplayer path from this feature.
pub const ORACLE_INVASION_WARP_SESSION_TOUCHES: &str = "oracle_invasion_warp_session_touches";
/// MUST STAY ZERO: `CS::MessageBoxDialog` builds during the run.
pub const ORACLE_INVASION_WARP_MSGBOX_BUILDS: &str = "oracle_invasion_warp_msgbox_builds";

/// How far the settled player position may sit from the requested one and still pass.
///
/// Not zero, and not a fudge factor either: the engine drops a warped character onto the
/// floor and resolves collision, so the settled Y in particular is expected to differ from
/// the authored point. The bound is tight enough that landing at a DIFFERENT spawn point
/// (the shipped points are tens of metres apart) still fails.
pub const INVASION_WARP_POSITION_TOLERANCE_METRES: f32 = 5.0;

/// Fixed-point scale for the position oracles: positions are reported as
/// `round(metres * 1000)` so a whole-number oracle field can carry millimetre precision.
pub const INVASION_WARP_POSITION_ORACLE_SCALE: f32 = 1000.0;

/// Encode a world position for the position oracles.
#[must_use]
pub fn encode_position_oracle(position: [f32; 3]) -> [i64; 3] {
    [
        encode_scalar_oracle(position[0]),
        encode_scalar_oracle(position[1]),
        encode_scalar_oracle(position[2]),
    ]
}

/// Encode one scalar (metres, or radians for the yaw oracle) at the oracle's fixed-point scale.
#[must_use]
pub fn encode_scalar_oracle(value: f32) -> i64 {
    if !value.is_finite() {
        return i64::MIN;
    }
    (value * INVASION_WARP_POSITION_ORACLE_SCALE).round() as i64
}

/// Does a settled position satisfy oracle 5 against the requested one?
#[must_use]
pub fn warp_arrival_within_tolerance(requested: [f32; 3], settled: [f32; 3]) -> bool {
    let dx = settled[0] - requested[0];
    let dy = settled[1] - requested[1];
    let dz = settled[2] - requested[2];
    let squared = dx * dx + dy * dy + dz * dz;
    squared.is_finite()
        && squared
            <= INVASION_WARP_POSITION_TOLERANCE_METRES * INVASION_WARP_POSITION_TOLERANCE_METRES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_oracle_name_is_distinct_and_prefixed() {
        let names = [
            ORACLE_INVASION_WARP_CATALOG_TARGETS,
            ORACLE_INVASION_WARP_CATALOG_BLOCKS,
            ORACLE_INVASION_WARP_CATALOG_AREAS,
            ORACLE_INVASION_WARP_LIST_ROWS,
            ORACLE_INVASION_WARP_SELECTED_ID,
            ORACLE_INVASION_WARP_REQUESTED_BLOCK,
            ORACLE_INVASION_WARP_REQUESTED_POSITION,
            ORACLE_INVASION_WARP_REQUESTED_YAW,
            ORACLE_INVASION_WARP_FINAL_BLOCK,
            ORACLE_INVASION_WARP_FINAL_POSITION,
            ORACLE_INVASION_WARP_SESSION_TOUCHES,
            ORACLE_INVASION_WARP_MSGBOX_BUILDS,
        ];
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(sorted.len(), before, "duplicate oracle name");
        for name in names {
            assert!(
                name.starts_with("oracle_invasion_warp_"),
                "{name} is not namespaced to this feature"
            );
        }
    }

    #[test]
    fn arrival_passes_within_tolerance_and_fails_beyond_it() {
        let requested = [100.0f32, 50.0, -20.0];
        // Settling onto the floor a couple of metres below still passes.
        assert!(warp_arrival_within_tolerance(
            requested,
            [100.4, 48.5, -20.2]
        ));
        // Landing at a different spawn point does not.
        assert!(!warp_arrival_within_tolerance(
            requested,
            [140.0, 50.0, -20.0]
        ));
    }

    #[test]
    fn arrival_refuses_non_finite_readings_rather_than_passing_them() {
        // A NaN readback is a broken measurement, not a pass.
        assert!(!warp_arrival_within_tolerance(
            [0.0; 3],
            [f32::NAN, 0.0, 0.0]
        ));
        assert!(!warp_arrival_within_tolerance(
            [0.0; 3],
            [f32::INFINITY, 0.0, 0.0]
        ));
    }

    #[test]
    fn positions_encode_at_millimetre_precision() {
        assert_eq!(encode_position_oracle([1.0, -2.5, 0.0]), [1000, -2500, 0]);
        assert_eq!(encode_scalar_oracle(-1.09), -1090);
        // A non-finite reading encodes to a value no real measurement can produce, so a
        // broken read can never be mistaken for a legitimate coordinate.
        assert_eq!(encode_scalar_oracle(f32::NAN), i64::MIN);
    }
}
