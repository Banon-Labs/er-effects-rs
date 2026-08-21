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

use std::sync::Mutex;
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

/// Injected pins that were appended to the list but CANNOT BE DRAWN, because all eight of their
/// label text ids are negative.
///
/// This is the oracle whose absence let a whole class of missing icons look like success.
/// `CS::WorldMapPinData::UpdateVisible` (0x14087afa0) writes the clip's visible flag at `row+0x0c`
/// only when, among other terms, some label satisfies `param+0x30+12i >= 0`; `SetTo` then hands
/// that flag straight to the clip. So an unnamed pin is not a pin with a blank caption -- it is a
/// pin that never appears, while `legacy_pins_placed` counts it as placed and the row count and
/// spare-row totals all look healthy.
///
/// It was legacy dungeons that hit it, because the place-name search was area-locked to the block's
/// own area and a legacy area whose graces carry no `PlaceName` label yields `-1` for every pin in
/// every dungeon of that area.
///
/// MUST BE ZERO. Any non-zero value is missing icons.
pub const ORACLE_INVASION_WARP_UNDRAWABLE_PINS: &str = "oracle_invasion_warp_undrawable_pins";

// --- Location matchmaking: publish (host side) and hunt (invader side) ----------------------
//
// These four exist because the two halves of location matchmaking fail in ways that look
// identical from outside the process. A run that publishes nothing and a run that publishes
// perfectly both end with a live game and a clean log tail; a hunt hook that never installed and
// a hunt hook that installed but was never asked to narrow anything both produce silence. Build
// success, launch success and "no crash" separate none of those cases. Only these counters do.

/// How many times this host wrote its current map onto its own Seamless lobby.
///
/// The HOST half of location matchmaking. Above zero means an invader running this DLL can ask
/// Steam for this player by location; zero means this player is only findable the old way.
pub const ORACLE_INVASION_WARP_LOBBY_PUBLISHES: &str = "oracle_invasion_warp_lobby_publishes";

/// How many publishes were REFUSED -- Steam not ready, no lobby yet, or (the loud one) a lobby
/// that does not carry Seamless's own advertisement marker.
///
/// Not a failure on its own: the first ticks of any run refuse while Seamless is still creating
/// its lobby. `refusals > 0 && publishes == 0` is the failure, and it is the exact shape a wrong
/// lobby-id offset would produce while every other signal looked healthy.
pub const ORACLE_INVASION_WARP_LOBBY_REFUSALS: &str = "oracle_invasion_warp_lobby_refusals";

/// Whether the detour onto `ISteamMatchmaking::RequestLobbyList` is installed.
///
/// The INVADER half. This is the one fact about hunt mode that no amount of offline work can
/// establish: the hook goes onto a vtable slot inside `steamclient64.dll`, not the game image, and
/// whether our union dispatcher can take that target is only answerable in a live process. False
/// with `hunt = true` means hunt is INERT and every query went out unfiltered -- which looks
/// exactly like "nobody is hosting there" from the player's seat.
pub const ORACLE_INVASION_WARP_HUNT_HOOKED: &str = "oracle_invasion_warp_hunt_hooked";

/// How many outgoing lobby queries actually carried our location filter.
///
/// The hook firing is not the same as the filter landing: `hunt_target` can decline (hunt off, no
/// readable block, several marked locations a single equality filter cannot express). Above zero
/// is the only proof that Seamless's own search went out narrowed to one place.
pub const ORACLE_INVASION_WARP_HUNT_FILTERS: &str = "oracle_invasion_warp_hunt_filters";

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

/// Pins appended that carry no non-negative label text id, and therefore cannot draw.
pub static INVASION_WARP_UNDRAWABLE_PINS: AtomicUsize = AtomicUsize::new(0);

/// Successful writes of this host's map onto its own Seamless lobby.
pub static INVASION_WARP_LOBBY_PUBLISHES: AtomicUsize = AtomicUsize::new(0);
/// Publishes declined for any reason, including the loud wrong-lobby refusal.
pub static INVASION_WARP_LOBBY_REFUSALS: AtomicUsize = AtomicUsize::new(0);
/// `1` once the `RequestLobbyList` detour is installed, `0` before and if it failed.
pub static INVASION_WARP_HUNT_HOOKED: AtomicUsize = AtomicUsize::new(0);
/// Outgoing lobby queries that carried our location filter.
pub static INVASION_WARP_HUNT_FILTERS: AtomicUsize = AtomicUsize::new(0);

/// Publish the legacy-dungeon placement pair measured by a world-map injection.
pub fn publish_legacy_pin_oracles(seen: usize, placed: usize) {
    INVASION_WARP_LEGACY_PINS_SEEN.store(seen, Ordering::SeqCst);
    INVASION_WARP_LEGACY_PINS_PLACED.store(placed, Ordering::SeqCst);
}

/// Publish how many injected pins cannot be drawn for want of a label. Zero is the only pass.
pub fn publish_undrawable_pin_count(count: usize) {
    INVASION_WARP_UNDRAWABLE_PINS.store(count, Ordering::SeqCst);
}

/// The undrawable-pin count as it currently stands.
#[must_use]
pub fn undrawable_pin_count() -> usize {
    INVASION_WARP_UNDRAWABLE_PINS.load(Ordering::SeqCst)
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

/// The last `(status, detail)` the catalog sampler published, so a republish can carry the
/// sampler's real phase instead of inventing one.
static LAST_DOCUMENT_STATUS: Mutex<Option<(String, String)>> = Mutex::new(None);

/// The location-matchmaking counters as of the last document write, so a republish costs four
/// comparisons in the steady state. Seeded with the all-zero state so a run that never publishes
/// or hunts never writes at all.
static LAST_DOCUMENT_MATCHMAKING: Mutex<((usize, usize), (bool, usize))> =
    Mutex::new(((0, 0), (false, 0)));

/// Write the telemetry document and remember the phase it was written in.
///
/// Every sampler emission goes through here rather than calling
/// [`crate::host::publish_oracle_json`] directly, because a later republish has to reuse the real
/// status -- a document that reported `latched` when the sampler was still `waiting` would be
/// worse than a stale one.
pub fn publish_document(status: &str, detail: &str) {
    *LAST_DOCUMENT_STATUS
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some((status.to_string(), detail.to_string()));
    *LAST_DOCUMENT_MATCHMAKING
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = (lobby_oracle_snapshot(), hunt_oracle_snapshot());
    crate::host::publish_oracle_json(&catalog_oracle_json(status, detail));
}

/// Rewrite the document if a location-matchmaking counter moved since the last write.
///
/// # The bug this exists to fix, caught by the feature it was measuring
///
/// The document was only ever written by the catalog sampler, which STOPS once the catalog totals
/// latch -- normally within the first seconds of a run. Every counter written after that moment
/// was invisible: the file froze with `hunt_filters: 0` while the in-memory counter climbed, and
/// the verdict line went on saying "no query has been narrowed" after a query had been narrowed.
///
/// Measured 2026-08-06: the DLL logged `hunt: asking Steam for hosts at m61_54_46_00 only (#1)`
/// while the telemetry document, last written 4 minutes earlier, reported zero filters -- and the
/// driver that read it concluded the detour had declined. A counter that is written but never
/// PUBLISHED misinforms exactly as badly as one that is published but never written, and this
/// module opens by warning about the second while shipping the first.
///
/// Returns whether anything was written, so a caller can tell a quiet tick from a stale one.
pub fn republish_if_location_matchmaking_changed() -> bool {
    let now = (lobby_oracle_snapshot(), hunt_oracle_snapshot());
    {
        let mut last = LAST_DOCUMENT_MATCHMAKING
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *last == now {
            return false;
        }
        *last = now;
    }
    let (status, detail) = LAST_DOCUMENT_STATUS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_else(|| {
            (
                "unsampled".to_string(),
                "the catalog sampler has not reported yet; the location-matchmaking counters below \
                 are live regardless"
                    .to_string(),
            )
        });
    crate::host::publish_oracle_json(&catalog_oracle_json(&status, &detail));
    true
}

/// Publish the host-side lobby counters, `(successful writes, declined attempts)`.
pub fn publish_lobby_oracles(publishes: usize, refusals: usize) {
    INVASION_WARP_LOBBY_PUBLISHES.store(publishes, Ordering::SeqCst);
    INVASION_WARP_LOBBY_REFUSALS.store(refusals, Ordering::SeqCst);
}

/// Publish the invader-side hunt counters.
pub fn publish_hunt_oracles(hooked: bool, filters: usize) {
    INVASION_WARP_HUNT_HOOKED.store(usize::from(hooked), Ordering::SeqCst);
    INVASION_WARP_HUNT_FILTERS.store(filters, Ordering::SeqCst);
}

/// The host-side pair as it currently stands, `(publishes, refusals)`.
#[must_use]
pub fn lobby_oracle_snapshot() -> (usize, usize) {
    (
        INVASION_WARP_LOBBY_PUBLISHES.load(Ordering::SeqCst),
        INVASION_WARP_LOBBY_REFUSALS.load(Ordering::SeqCst),
    )
}

/// The invader-side pair as it currently stands, `(hooked, filters)`.
#[must_use]
pub fn hunt_oracle_snapshot() -> (bool, usize) {
    (
        INVASION_WARP_HUNT_HOOKED.load(Ordering::SeqCst) != 0,
        INVASION_WARP_HUNT_FILTERS.load(Ordering::SeqCst),
    )
}

/// What the four location-matchmaking counters mean, in one line per half.
///
/// Written as a verdict rather than left to the reader because the failure states are the ones
/// that look like success: a host that published nothing is still a running game, and a hunt that
/// never narrowed a query is indistinguishable from an empty world.
#[must_use]
pub fn describe_location_matchmaking(
    publishes: usize,
    refusals: usize,
    hooked: bool,
    filters: usize,
) -> String {
    let host = match (publishes, refusals) {
        (0, 0) => {
            "publish: never attempted -- this player has not opened a lobby to invaders this \
                   run, so there was nothing to advertise on"
                .to_string()
        }
        (0, r) => format!(
            "publish: REFUSED {r} time(s) and never once succeeded -- this host is NOT findable by \
             location, and the log says which refusal it was"
        ),
        (p, r) => format!("publish: advertised this host's map {p} time(s) ({r} declined)"),
    };
    let invader = match (hooked, filters) {
        // Impossible by construction: the filter is only ever added from inside the detour. If
        // this ever prints, the counters disagree with the code and neither can be trusted.
        (false, f) if f > 0 => format!(
            "hunt: CONTRADICTION -- {f} filter(s) recorded with no hook installed; these counters \
             are not measuring what they claim"
        ),
        (false, _) => "hunt: no detour on RequestLobbyList, so every query went out UNFILTERED -- \
                       hunt mode is inert regardless of what the config says"
            .to_string(),
        (true, 0) => {
            "hunt: hooked, but no query has been narrowed -- hunt is off, it refused (see \
                      the log), or Seamless has not searched yet"
                .to_string()
        }
        (true, f) => format!(
            "hunt: {f} outgoing lobby quer(y/ies) asked Steam for ONE location; hosts without this \
             DLL were not returned"
        ),
    };
    format!("{host}; {invader}")
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
    let (publishes, refusals) = lobby_oracle_snapshot();
    let (hooked, filters) = hunt_oracle_snapshot();
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
\"{ORACLE_INVASION_WARP_UNDRAWABLE_PINS}\":{undrawable},\
\"undrawable_pins_note\":\"a pin whose eight label text ids are all negative is NOT DRAWN; any \
value above zero is missing icons, not missing captions\",\
\"{ORACLE_INVASION_WARP_LOBBY_PUBLISHES}\":{publishes},\
\"{ORACLE_INVASION_WARP_LOBBY_REFUSALS}\":{refusals},\
\"{ORACLE_INVASION_WARP_HUNT_HOOKED}\":{hooked},\
\"{ORACLE_INVASION_WARP_HUNT_FILTERS}\":{filters},\
\"location_matchmaking_note\":\"{matchmaking_note}\",\
\"{ORACLE_INVASION_WARP_SESSION_TOUCHES}\":null,\
\"{ORACLE_INVASION_WARP_MSGBOX_BUILDS}\":null,\
\"negative_oracles_measured\":false,\
\"negative_oracles_note\":\"{note}\"}}\n",
        status = json_escape(status),
        undrawable = undrawable_pin_count(),
        legacy_note = json_escape(describe_legacy_pin_oracle(legacy_seen, legacy_placed)),
        matchmaking_note = json_escape(&describe_location_matchmaking(
            publishes, refusals, hooked, filters
        )),
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
            ORACLE_INVASION_WARP_LEGACY_PINS_SEEN,
            ORACLE_INVASION_WARP_LEGACY_PINS_PLACED,
            ORACLE_INVASION_WARP_UNDRAWABLE_PINS,
            ORACLE_INVASION_WARP_LOBBY_PUBLISHES,
            ORACLE_INVASION_WARP_LOBBY_REFUSALS,
            ORACLE_INVASION_WARP_HUNT_HOOKED,
            ORACLE_INVASION_WARP_HUNT_FILTERS,
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
    fn a_counter_that_moves_after_the_sampler_latches_still_reaches_the_document() {
        // THE MEASURED BUG, 2026-08-06. The document was written only by the catalog sampler,
        // which stops at `Latched`. A hunt filter added minutes later left the file reporting
        // zero, and the driver reading it concluded the detour had declined -- a false negative
        // produced by the very instrument meant to prevent them.
        publish_document("latched", "totals settled");
        assert!(
            !republish_if_location_matchmaking_changed(),
            "an unchanged counter set must not rewrite the document"
        );
        publish_hunt_oracles(true, 1);
        assert!(
            republish_if_location_matchmaking_changed(),
            "a filter added after the sampler latched MUST still be published"
        );
        assert!(
            !republish_if_location_matchmaking_changed(),
            "and only once -- the republish is edge-triggered, not per tick"
        );
        publish_hunt_oracles(false, 0);
        republish_if_location_matchmaking_changed();
    }

    #[test]
    fn a_republish_carries_the_samplers_real_phase_not_an_invented_one() {
        // Reusing the last real status matters: a document claiming `latched` while the sampler
        // was still `waiting` would misreport the catalog oracle to fix the hunt one.
        publish_document("waiting", "catalog not ready");
        publish_lobby_oracles(1, 0);
        assert!(republish_if_location_matchmaking_changed());
        let document = catalog_oracle_json("waiting", "catalog not ready");
        assert!(document.contains("\"status\":\"waiting\""));
        publish_lobby_oracles(0, 0);
        republish_if_location_matchmaking_changed();
    }

    #[test]
    fn a_host_that_only_ever_refused_is_reported_as_unfindable_not_as_quiet() {
        // The failure this exists to catch: a wrong lobby-id offset publishes nothing, and every
        // other signal in the run (game alive, log clean, pins drawn) looks identical to success.
        let line = describe_location_matchmaking(0, 12, false, 0);
        assert!(
            line.contains("REFUSED 12"),
            "the refusal count has to reach the verdict: {line}"
        );
        assert!(
            line.contains("NOT findable"),
            "a host that never published must be called unfindable: {line}"
        );
    }

    #[test]
    fn never_opening_a_lobby_is_not_reported_as_a_failure() {
        // Zero and zero is the ordinary state of a player who simply is not hosting. Calling that
        // a failure would train the reader to ignore the field.
        let line = describe_location_matchmaking(0, 0, true, 0);
        assert!(
            line.contains("never attempted"),
            "not hosting must read as not-attempted: {line}"
        );
        assert!(
            !line.contains("REFUSED"),
            "not hosting is not a refusal: {line}"
        );
    }

    #[test]
    fn an_uninstalled_hunt_hook_is_called_inert_however_the_config_reads() {
        // `hunt = true` plus a hook that never landed is the silent-failure shape: the player sees
        // an empty search and concludes nobody is online.
        let line = describe_location_matchmaking(3, 0, false, 0);
        assert!(
            line.contains("UNFILTERED") && line.contains("inert"),
            "a missing hook must say the queries went out unnarrowed: {line}"
        );
    }

    #[test]
    fn a_hooked_but_never_narrowed_run_is_separated_from_a_narrowed_one() {
        let idle = describe_location_matchmaking(1, 0, true, 0);
        let firing = describe_location_matchmaking(1, 0, true, 4);
        assert!(
            idle.contains("no query has been narrowed"),
            "hooked-but-idle must be its own state: {idle}"
        );
        assert!(
            firing.contains('4'),
            "the filter count is the whole proof: {firing}"
        );
        assert_ne!(idle, firing);
    }

    #[test]
    fn filters_without_a_hook_are_reported_as_a_contradiction_not_as_success() {
        // Impossible by construction -- the filter is only added from inside the detour. If the
        // counters ever say otherwise, the honest output is "these numbers are wrong", not a
        // cheerful success line built on them.
        let line = describe_location_matchmaking(0, 0, false, 7);
        assert!(
            line.contains("CONTRADICTION"),
            "impossible counter states must be named, not smoothed over: {line}"
        );
    }

    #[test]
    fn the_telemetry_document_carries_all_four_location_matchmaking_counters() {
        publish_lobby_oracles(5, 2);
        publish_hunt_oracles(true, 3);
        let document = catalog_oracle_json("sampling", "unit test");
        for expected in [
            "\"oracle_invasion_warp_lobby_publishes\":5",
            "\"oracle_invasion_warp_lobby_refusals\":2",
            "\"oracle_invasion_warp_hunt_hooked\":true",
            "\"oracle_invasion_warp_hunt_filters\":3",
        ] {
            assert!(
                document.contains(expected),
                "{expected} missing from {document}"
            );
        }
        // The counters are useless if the document does not also say what they mean.
        assert!(document.contains("location_matchmaking_note"));
        publish_lobby_oracles(0, 0);
        publish_hunt_oracles(false, 0);
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
