//! The RAM/pixel semaphores the invasion-warp feature must go green on to be PROVEN.
//!
//! AGENTS.md is explicit: a rendered/behavioural feature is never proven by build success,
//! launch success, "no crash", hook counters, or "the draw task ran". So the oracles are
//! designed HERE, before any runtime exists, and this module is the single place their names
//! and pass conditions are written down.
//!
//! Most of these are NAMES and CONTRACTS, not counters. A counter that is read to emit an
//! oracle but never written reports 0 forever and actively misinforms
//! (`scripts/check-oracle-writers.py`), so each atomic lands in the same change that first
//! WRITES it. Today that is oracle 1 only: [`INVASION_WARP_CATALOG_TARGETS`],
//! [`INVASION_WARP_CATALOG_BLOCKS`] and [`INVASION_WARP_CATALOG_AREAS`] are written by
//! [`crate::sampler`] every time the live singleton is read. Oracles 2-5 stay names until the
//! UI interception that can write them exists.
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
//!
//! ## Both are UNMEASURED as of the catalog slice, and deliberately have no counter
//!
//! Neither has an `AtomicUsize` yet, and adding one now would be the exact defect this module
//! opens by warning about:
//!
//! * SESSION_TOUCHES would have no writer, because there is no call site to count. The catalog
//!   slice makes exactly one kind of engine access -- a fault-tolerant READ of
//!   `CSAutoInvadePoint` (see [`crate::live_read`]) -- and calls nothing under `CSNetMan` /
//!   `QuickmatchManager` / `CSBreakInPointManager`. A counter incremented from a branch no
//!   reachable code takes reports 0 for a structural reason, not a measured one, and a reader
//!   cannot tell those apart. What DOES carry evidence today is the absence of any such call in
//!   a crate whose only unsafe engine surface is one read -- reviewable, but not a semaphore.
//! * MSGBOX_BUILDS needs either a detour on the `CS::MessageBoxDialog` builder (`0x1409275b0`)
//!   or the passive full-address-space vtable scan `er_telemetry::read::dialog_active` runs.
//!   The DLL that hosts this crate installs no detours at all, and the passive scan answers
//!   "a box is on screen", not "THIS feature built one" -- so it could not attribute a hit
//!   even if it were cheap enough to run every tick.
//!
//! [`catalog_oracle_json`] therefore reports both as JSON `null` with
//! `negative_oracles_measured: false`, which a reader cannot mistake for a measured zero.
//! They land with the UI/warp interception that first gives them a call site.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::aip::{AIP_FINGERPRINT_BASE, AIP_FINGERPRINT_DLC02};
use crate::invasion_warp::InvasionWarpCatalogSummary;

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

/// How many legacy-dungeon (non-area-60/61) targets were OFFERED to the world-map injection.
///
/// Zero means no such map has been resident this session yet -- the MSB source accumulates as maps
/// load, so a fresh boot in the overworld legitimately offers none.
pub const ORACLE_INVASION_WARP_LEGACY_PINS_SEEN: &str = "oracle_invasion_warp_legacy_pins_seen";
/// How many of those the world-map coordinate converters actually ACCEPTED.
///
/// This is the decisive number for legacy-dungeon coverage, and the reason it is an oracle rather
/// than a log line: `seen > 0 && placed == 0` says the converter set cannot place a dungeon pin at
/// all, in which case reading MORE dungeon MSBs (the whole non-resident-map sweep) would produce
/// nothing visible and the converter is what needs fixing first. `placed` tracking `seen` says the
/// opposite. No amount of build or launch success answers that question; only this pair does.
pub const ORACLE_INVASION_WARP_LEGACY_PINS_PLACED: &str = "oracle_invasion_warp_legacy_pins_placed";

// --- ORACLE 1 counters --------------------------------------------------------------------
//
// Written by `crate::sampler` on every successful read of the live `CSAutoInvadePoint`, read
// by `catalog_oracle_json` to emit the three `oracle_invasion_warp_catalog_*` fields. They are
// the ONLY oracle atomics in this crate; see the module docs for why the negative oracles have
// none.

/// Total targets in the catalog last read out of the live singleton.
pub static INVASION_WARP_CATALOG_TARGETS: AtomicUsize = AtomicUsize::new(0);
/// Distinct blocks in that catalog.
pub static INVASION_WARP_CATALOG_BLOCKS: AtomicUsize = AtomicUsize::new(0);
/// Distinct map areas in that catalog.
pub static INVASION_WARP_CATALOG_AREAS: AtomicUsize = AtomicUsize::new(0);

/// Legacy-dungeon targets offered to the last world-map injection.
pub static INVASION_WARP_LEGACY_PINS_SEEN: AtomicUsize = AtomicUsize::new(0);
/// Legacy-dungeon targets the converters accepted in that injection.
pub static INVASION_WARP_LEGACY_PINS_PLACED: AtomicUsize = AtomicUsize::new(0);

/// Publish the legacy-dungeon placement pair measured by a world-map injection.
pub fn publish_legacy_pin_oracles(seen: usize, placed: usize) {
    INVASION_WARP_LEGACY_PINS_SEEN.store(seen, Ordering::SeqCst);
    INVASION_WARP_LEGACY_PINS_PLACED.store(placed, Ordering::SeqCst);
}

/// The legacy-dungeon placement pair as it currently stands, `(seen, placed)`.
#[must_use]
pub fn legacy_pin_oracle_snapshot() -> (usize, usize) {
    (
        INVASION_WARP_LEGACY_PINS_SEEN.load(Ordering::SeqCst),
        INVASION_WARP_LEGACY_PINS_PLACED.load(Ordering::SeqCst),
    )
}

/// What `(seen, placed)` means for the non-resident-map sweep, in one line.
#[must_use]
pub fn describe_legacy_pin_oracle(seen: usize, placed: usize) -> &'static str {
    match (seen, placed) {
        (0, _) => {
            "no legacy-dungeon map has been resident this session yet -- coverage accumulates as \
             maps load, so this is not a failure"
        }
        (_, 0) => {
            "the world-map converters REFUSED every legacy-dungeon pin -- reading more dungeon MSBs \
             cannot help until a converter can place one"
        }
        _ if placed == seen => "every offered legacy-dungeon pin was placed",
        _ => {
            "some legacy-dungeon pins were placed and some refused -- the converter set is partial"
        }
    }
}

/// Publish a freshly-read catalog's totals to the oracle-1 counters.
pub fn publish_catalog_oracles(summary: InvasionWarpCatalogSummary) {
    INVASION_WARP_CATALOG_TARGETS.store(summary.target_count, Ordering::SeqCst);
    INVASION_WARP_CATALOG_BLOCKS.store(summary.block_count, Ordering::SeqCst);
    INVASION_WARP_CATALOG_AREAS.store(summary.area_count, Ordering::SeqCst);
}

/// The oracle-1 counters as they currently stand, in the emission order
/// `(targets, blocks, areas)`.
#[must_use]
pub fn catalog_oracle_snapshot() -> (usize, usize, usize) {
    (
        INVASION_WARP_CATALOG_TARGETS.load(Ordering::SeqCst),
        INVASION_WARP_CATALOG_BLOCKS.load(Ordering::SeqCst),
        INVASION_WARP_CATALOG_AREAS.load(Ordering::SeqCst),
    )
}

// --- ORACLE 1 pass conditions ---------------------------------------------------------------

/// Totals for an install with only `other:/AutoInvadePoint.aipbnd` mounted: one area (60).
pub const EXPECTED_CATALOG_BASE: InvasionWarpCatalogSummary = InvasionWarpCatalogSummary {
    block_count: AIP_FINGERPRINT_BASE.entry_count,
    target_count: AIP_FINGERPRINT_BASE.point_count,
    area_count: 1,
};

/// Totals with `_dlc02` mounted as well: areas 60 and 61.
pub const EXPECTED_CATALOG_BASE_DLC02: InvasionWarpCatalogSummary = InvasionWarpCatalogSummary {
    block_count: AIP_FINGERPRINT_BASE.entry_count + AIP_FINGERPRINT_DLC02.entry_count,
    target_count: AIP_FINGERPRINT_BASE.point_count + AIP_FINGERPRINT_DLC02.point_count,
    area_count: 2,
};

/// Which shipped fingerprint a live read matched -- oracle 1's pass condition.
///
/// The condition is EXACT equality, never `> 0`: a smaller total means the read raced the
/// loader, a larger one means it double-counted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogFingerprintVerdict {
    /// Exactly [`EXPECTED_CATALOG_BASE`] -- a base-game install with no DLC container.
    MatchBase,
    /// Exactly [`EXPECTED_CATALOG_BASE_DLC02`] -- both containers mounted.
    MatchBaseAndDlc02,
    /// Neither. Oracle 1 FAILS.
    Mismatch,
}

impl CatalogFingerprintVerdict {
    /// Does this verdict pass oracle 1?
    #[must_use]
    pub const fn passed(self) -> bool {
        !matches!(self, Self::Mismatch)
    }

    /// Stable machine-readable tag for the telemetry document.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::MatchBase => "match_base",
            Self::MatchBaseAndDlc02 => "match_base_dlc02",
            Self::Mismatch => "mismatch",
        }
    }
}

/// Classify a live catalog's totals against the shipped fingerprints.
#[must_use]
pub fn classify_catalog(summary: InvasionWarpCatalogSummary) -> CatalogFingerprintVerdict {
    if summary == EXPECTED_CATALOG_BASE_DLC02 {
        CatalogFingerprintVerdict::MatchBaseAndDlc02
    } else if summary == EXPECTED_CATALOG_BASE {
        CatalogFingerprintVerdict::MatchBase
    } else {
        CatalogFingerprintVerdict::Mismatch
    }
}

/// The single log line a user can read the oracle-1 verdict off, with no cross-referencing:
/// observed totals, both expected fingerprints, and the verdict.
#[must_use]
pub fn describe_catalog_oracle(summary: InvasionWarpCatalogSummary) -> String {
    let verdict = classify_catalog(summary);
    let tail = match verdict {
        CatalogFingerprintVerdict::MatchBase => {
            "MATCH (base only -- no _dlc02 container mounted)".to_owned()
        }
        CatalogFingerprintVerdict::MatchBaseAndDlc02 => "MATCH (base + dlc02)".to_owned(),
        CatalogFingerprintVerdict::Mismatch => {
            "MISMATCH (matches NEITHER fingerprint: fewer means the read raced the loader, \
             more means it double-counted)"
                .to_owned()
        }
    };
    format!(
        "catalog: {} blocks / {} targets / {} areas (expected base {}/{}, +dlc02 {}/{}) -> {tail}",
        summary.block_count,
        summary.target_count,
        summary.area_count,
        EXPECTED_CATALOG_BASE.block_count,
        EXPECTED_CATALOG_BASE.target_count,
        EXPECTED_CATALOG_BASE_DLC02.block_count,
        EXPECTED_CATALOG_BASE_DLC02.target_count,
    )
}

/// The one-line restatement of why the two negative oracles carry no number yet. Emitted
/// alongside the verdict so a run's log never leaves their absence to be inferred.
pub const NEGATIVE_ORACLES_UNMEASURED_NOTE: &str = "oracle_invasion_warp_session_touches and \
     oracle_invasion_warp_msgbox_builds are UNMEASURED by this slice, not zero: the catalog read \
     has no session call site to count, and counting MessageBoxDialog builds needs a builder \
     detour this DLL does not install";

/// Escape a string for embedding in the telemetry document.
fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// The feature's oracle telemetry document, built from the CURRENT counter values.
///
/// `status` is the sampler's phase (`waiting` / `sampling` / `latched` / `gave_up`) and
/// `detail` is the human-readable reason behind it. The two negative oracles are emitted as
/// `null` with `negative_oracles_measured: false`, which cannot be misread as a measured zero.
#[must_use]
pub fn catalog_oracle_json(status: &str, detail: &str) -> String {
    let (targets, blocks, areas) = catalog_oracle_snapshot();
    let (legacy_seen, legacy_placed) = legacy_pin_oracle_snapshot();
    let summary = InvasionWarpCatalogSummary {
        block_count: blocks,
        target_count: targets,
        area_count: areas,
    };
    let verdict = classify_catalog(summary);
    format!(
        "{{\"{ORACLE_INVASION_WARP_CATALOG_TARGETS}\":{targets},\
\"{ORACLE_INVASION_WARP_CATALOG_BLOCKS}\":{blocks},\
\"{ORACLE_INVASION_WARP_CATALOG_AREAS}\":{areas},\
\"status\":\"{status}\",\
\"verdict\":\"{verdict_tag}\",\
\"passed\":{passed},\
\"detail\":\"{detail}\",\
\"expected_base\":{{\"blocks\":{base_blocks},\"targets\":{base_targets},\"areas\":{base_areas}}},\
\"expected_base_dlc02\":{{\"blocks\":{dlc_blocks},\"targets\":{dlc_targets},\"areas\":{dlc_areas}}},\
\"{ORACLE_INVASION_WARP_LEGACY_PINS_SEEN}\":{legacy_seen},\
\"{ORACLE_INVASION_WARP_LEGACY_PINS_PLACED}\":{legacy_placed},\
\"legacy_pins_note\":\"{legacy_note}\",\
\"{ORACLE_INVASION_WARP_SESSION_TOUCHES}\":null,\
\"{ORACLE_INVASION_WARP_MSGBOX_BUILDS}\":null,\
\"negative_oracles_measured\":false,\
\"negative_oracles_note\":\"{note}\"}}\n",
        status = json_escape(status),
        legacy_note = json_escape(describe_legacy_pin_oracle(legacy_seen, legacy_placed)),
        verdict_tag = verdict.tag(),
        passed = verdict.passed(),
        detail = json_escape(detail),
        base_blocks = EXPECTED_CATALOG_BASE.block_count,
        base_targets = EXPECTED_CATALOG_BASE.target_count,
        base_areas = EXPECTED_CATALOG_BASE.area_count,
        dlc_blocks = EXPECTED_CATALOG_BASE_DLC02.block_count,
        dlc_targets = EXPECTED_CATALOG_BASE_DLC02.target_count,
        dlc_areas = EXPECTED_CATALOG_BASE_DLC02.area_count,
        note = json_escape(NEGATIVE_ORACLES_UNMEASURED_NOTE),
    )
}

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
    fn the_expected_totals_are_exactly_the_shipped_fingerprints() {
        assert_eq!(EXPECTED_CATALOG_BASE.block_count, 257);
        assert_eq!(EXPECTED_CATALOG_BASE.target_count, 4482);
        assert_eq!(EXPECTED_CATALOG_BASE.area_count, 1);
        assert_eq!(EXPECTED_CATALOG_BASE_DLC02.block_count, 365);
        assert_eq!(EXPECTED_CATALOG_BASE_DLC02.target_count, 7073);
        assert_eq!(EXPECTED_CATALOG_BASE_DLC02.area_count, 2);
        // Both containers cover exactly one area each, so the DLC install adds exactly one.
        assert_eq!(AIP_FINGERPRINT_BASE.area, 60);
        assert_eq!(AIP_FINGERPRINT_DLC02.area, 61);
    }

    #[test]
    fn oracle_one_passes_only_on_an_exact_fingerprint_match() {
        assert_eq!(
            classify_catalog(EXPECTED_CATALOG_BASE),
            CatalogFingerprintVerdict::MatchBase
        );
        assert_eq!(
            classify_catalog(EXPECTED_CATALOG_BASE_DLC02),
            CatalogFingerprintVerdict::MatchBaseAndDlc02
        );
        // One block short of the DLC total: a read that raced the loader must FAIL, not pass
        // on a "> 0" reading.
        let mut short = EXPECTED_CATALOG_BASE_DLC02;
        short.block_count -= 1;
        assert_eq!(classify_catalog(short), CatalogFingerprintVerdict::Mismatch);
        // One target over: a double-count must fail too.
        let mut over = EXPECTED_CATALOG_BASE_DLC02;
        over.target_count += 1;
        assert_eq!(classify_catalog(over), CatalogFingerprintVerdict::Mismatch);
        // And an empty catalog is never a pass.
        assert_eq!(
            classify_catalog(InvasionWarpCatalogSummary::default()),
            CatalogFingerprintVerdict::Mismatch
        );
        assert!(CatalogFingerprintVerdict::MatchBase.passed());
        assert!(CatalogFingerprintVerdict::MatchBaseAndDlc02.passed());
        assert!(!CatalogFingerprintVerdict::Mismatch.passed());
    }

    #[test]
    fn the_verdict_line_carries_both_expected_fingerprints_next_to_the_observation() {
        let line = describe_catalog_oracle(EXPECTED_CATALOG_BASE_DLC02);
        assert!(
            line.contains("365 blocks / 7073 targets / 2 areas"),
            "{line}"
        );
        assert!(line.contains("expected base 257/4482"), "{line}");
        assert!(line.contains("+dlc02 365/7073"), "{line}");
        assert!(line.contains("MATCH (base + dlc02)"), "{line}");

        let base_only = describe_catalog_oracle(EXPECTED_CATALOG_BASE);
        assert!(base_only.contains("MATCH (base only"), "{base_only}");

        // A failing read still prints both fingerprints, so the user never has to look
        // anything up to see WHY it failed.
        let bad = describe_catalog_oracle(InvasionWarpCatalogSummary {
            block_count: 12,
            target_count: 40,
            area_count: 1,
        });
        assert!(bad.contains("MISMATCH"), "{bad}");
        assert!(bad.contains("expected base 257/4482"), "{bad}");
        assert!(bad.contains("raced the loader"), "{bad}");
    }

    #[test]
    fn the_telemetry_document_reports_the_live_counters_and_never_fakes_the_negative_oracles() {
        publish_catalog_oracles(EXPECTED_CATALOG_BASE_DLC02);
        assert_eq!(catalog_oracle_snapshot(), (7073, 365, 2));
        let json = catalog_oracle_json("latched", "stable for 8 samples");
        assert!(
            json.contains("\"oracle_invasion_warp_catalog_targets\":7073"),
            "{json}"
        );
        assert!(
            json.contains("\"oracle_invasion_warp_catalog_blocks\":365"),
            "{json}"
        );
        assert!(
            json.contains("\"oracle_invasion_warp_catalog_areas\":2"),
            "{json}"
        );
        assert!(json.contains("\"verdict\":\"match_base_dlc02\""), "{json}");
        assert!(json.contains("\"passed\":true"), "{json}");
        // The negative oracles are null + explicitly flagged unmeasured. A measured zero and
        // an unmeasured oracle must never look the same to a reader.
        assert!(
            json.contains("\"oracle_invasion_warp_session_touches\":null"),
            "{json}"
        );
        assert!(
            json.contains("\"oracle_invasion_warp_msgbox_builds\":null"),
            "{json}"
        );
        assert!(
            json.contains("\"negative_oracles_measured\":false"),
            "{json}"
        );
        assert!(!json.contains("_session_touches\":0"), "{json}");
        assert!(!json.contains("_msgbox_builds\":0"), "{json}");
        assert!(json.ends_with('\n'), "{json}");
    }

    #[test]
    fn the_telemetry_document_escapes_a_detail_string_that_would_break_the_json() {
        let json = catalog_oracle_json("waiting", "node \"0x7ff\\bad\" is\nunreadable");
        assert!(json.contains(r#"\"0x7ff\\bad\""#), "{json}");
        assert!(json.contains("is\\nunreadable"), "{json}");
        assert!(!json.contains('\t'), "{json}");
        // Balanced braces is the cheap structural check that the escaping did not corrupt it.
        assert_eq!(
            json.matches('{').count(),
            json.matches('}').count(),
            "{json}"
        );
    }

    #[test]
    fn the_legacy_pin_oracle_separates_never_offered_from_refused() {
        // These are the two failures that a single "no dungeon markers" number would fuse, and
        // they need opposite fixes: nothing resident yet (wait / visit a dungeon) versus the
        // converters rejecting what they were given (fix the converter, do not read more MSBs).
        assert!(describe_legacy_pin_oracle(0, 0).contains("not a failure"));
        assert!(describe_legacy_pin_oracle(12, 0).contains("REFUSED"));
        assert_eq!(
            describe_legacy_pin_oracle(12, 12),
            "every offered legacy-dungeon pin was placed"
        );
        assert!(describe_legacy_pin_oracle(12, 5).contains("partial"));
    }

    #[test]
    fn the_telemetry_document_carries_the_legacy_pin_pair() {
        publish_legacy_pin_oracles(9, 0);
        assert_eq!(legacy_pin_oracle_snapshot(), (9, 0));
        let json = catalog_oracle_json("latched", "stable");
        assert!(
            json.contains("\"oracle_invasion_warp_legacy_pins_seen\":9"),
            "{json}"
        );
        assert!(
            json.contains("\"oracle_invasion_warp_legacy_pins_placed\":0"),
            "{json}"
        );
        assert!(json.contains("REFUSED"), "{json}");
        assert_eq!(
            json.matches('{').count(),
            json.matches('}').count(),
            "{json}"
        );
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
