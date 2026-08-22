//! Pure decision logic for the loading-screen portrait's CHARACTER-IDENTITY semaphores.
//!
//! Split out of the DLL so the rules that decide "is the portrait showing the right character"
//! are host-`cargo test`-able instead of only reachable through a game launch. Nothing here
//! reads game memory; the callers pass in values they already read.
//!
//! Two independent questions live here:
//!
//! 1. **Is a packed map id worth comparing at all** ([`packed_map_is_plausible`])? The old gate
//!    was `map > 0`, which is meaningless for a packed `BlockId` — its high byte is the areaId,
//!    so any area >= 0x80 reads negative and silently switched the map comparison OFF.
//! 2. **Did the portrait we PUBLISHED match the character that actually loaded**
//!    ([`published_identity_verdict`])? This is the comparison the loading screen's pixels
//!    depend on, and nothing asserted it before (bd er-effects-rs-qoqc / er-effects-rs-91zb: a
//!    wrong face was on screen for 29.7s with every existing oracle reporting ok).

/// New-game / not-yet-resolved saved-map sentinel (`m10_01_00_00`). Excluded from every map
/// comparison so a transient `c30` during a loading screen cannot false-fire.
pub const DEFAULT_MAP_C30: i32 = 0x0a01_0000;

/// Lowest areaId seen on a real ER map. A packed `BlockId` is `{indexId, regionId, blockId,
/// areaId}` little-endian, so the areaId is the HIGH byte of the dword.
pub const MAP_AREA_ID_MIN: u8 = 0x0a;
/// Highest areaId seen on a real ER map (DLC included).
pub const MAP_AREA_ID_MAX: u8 = 0x3d;

/// The areaId of a packed `BlockId` (its high byte).
#[must_use]
pub const fn packed_map_area_id(map: i32) -> u8 {
    ((map as u32) >> 24) as u8
}

/// True when `map` looks like a real packed `BlockId` worth comparing against another one.
///
/// REPLACES the `map > 0` sign gate. A `BlockId`'s sign bit is just bit 7 of the areaId and
/// carries no meaning; treating it as a validity flag meant a garbage word with bit31 set turned
/// the map axis off instead of failing it. Empirically every one of 726 active corpus slots has
/// an areaId in `MAP_AREA_ID_MIN..=MAP_AREA_ID_MAX` (pinned by er-save-loader's corpus test),
/// while random noise clears it under 10% of the time.
#[must_use]
pub const fn packed_map_is_plausible(map: i32) -> bool {
    let area = packed_map_area_id(map);
    area >= MAP_AREA_ID_MIN && area <= MAP_AREA_ID_MAX && map != DEFAULT_MAP_C30
}

/// Whether the two map ids are comparable AND disagree. Only a mismatch between two plausible
/// maps counts; an implausible value on either side means "no map evidence", never "mismatch".
#[must_use]
pub const fn packed_maps_disagree(ours: i32, live: i32) -> bool {
    packed_map_is_plausible(ours) && packed_map_is_plausible(live) && ours != live
}

/// What the published loading-screen portrait was compared against the character that loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishedIdentity {
    /// Nothing was published in this window — not a mismatch, just no evidence.
    NothingPublished,
    /// No load completed to compare against (no deserialize, no confirmed slot).
    NoLoadedSlot,
    /// Published portrait belongs to the slot that loaded.
    Match,
    /// Published portrait belongs to a DIFFERENT slot than the one that loaded. This is the
    /// 29.7s-wrong-face class.
    SlotMismatch { published: i32, loaded: i32 },
}

impl PublishedIdentity {
    /// True only for a genuine wrong-character publish, so callers can bump a fail counter
    /// without also counting "we had nothing to check".
    #[must_use]
    pub const fn is_mismatch(self) -> bool {
        matches!(self, Self::SlotMismatch { .. })
    }
}

/// Compare the published portrait's slot against the slot whose load actually completed.
///
/// `published_slot_tag` is the wire form stored in `LS_PORTRAIT_PUBLISHED_SLOT`: **slot + 1**, so
/// 0 means "never published". `loaded_slot` is `None` when no load has completed yet.
///
/// The +1 biasing is the whole reason this needs a test: the counter cannot distinguish "slot 0"
/// from "unset" without it, and an off-by-one here would either hide every mismatch or invent one
/// on every boot.
#[must_use]
pub fn published_identity_verdict(
    published_slot_tag: usize,
    loaded_slot: Option<i32>,
) -> PublishedIdentity {
    let Some(published) = published_slot_tag.checked_sub(1) else {
        return PublishedIdentity::NothingPublished;
    };
    let Some(loaded) = loaded_slot else {
        return PublishedIdentity::NoLoadedSlot;
    };
    let published = published as i32;
    if published == loaded {
        PublishedIdentity::Match
    } else {
        PublishedIdentity::SlotMismatch { published, loaded }
    }
}

/// The slot the portrait pipeline should TARGET while a load is in flight, given the three
/// sources that can name it. Higher precedence first:
///
/// 1. `picker_slot` — the user's explicit on-screen pick. Nothing the game infers outranks what
///    the user selected, and the pick is known before any of the others settle.
/// 2. `request_slot` — `GameMan+0xb78`, the native load-REQUEST register. When it names a slot
///    different from `save_slot`, a load for THAT slot is in flight, so `save_slot` is stale by
///    definition.
/// 3. `save_slot` — `GameMan.save_slot` (ac0). Last resort: both the game's own selector and our
///    own `set_save_slot` write it for reasons unrelated to "which character is loading", so it
///    is the weakest of the three (bd er-effects-rs-91zb).
///
/// `None` when no source names a valid slot — callers must NOT collapse that to slot 0.
#[must_use]
pub fn portrait_target_slot_from_sources(
    picker_slot: Option<i32>,
    request_slot: Option<i32>,
    save_slot: Option<i32>,
    slot_count: i32,
) -> Option<i32> {
    let valid = |s: Option<i32>| s.filter(|v| (0..slot_count).contains(v));
    valid(picker_slot)
        .or_else(|| valid(request_slot))
        .or(valid(save_slot))
}

/// STABILITY over freshness, for the duration of ONE loading-screen window.
///
/// [`portrait_target_slot_from_sources`] is a precedence ordering evaluated fresh on every kick,
/// so its answer can CHANGE while a single loading screen is on screen — and when it does, the
/// face the user is looking at is replaced by a different character's mid-load. Measured
/// 2026-08-02 21:05: the user picked slot 0, the pipeline built and published slot 0 at
/// +17775ms, then the picker term expired (it is spent on `IN_WORLD_REACHED`, i.e. *a* world
/// existing, not *that slot's* world), precedence fell through `request_slot = -1` to
/// `save_slot = 9`, and kick #2 retargeted the SAME window to slot 9 at +20998ms. The window did
/// not close until +29989ms, so the user watched the portrait change out from under the character
/// they clicked.
///
/// This is the fix for that, and it is deliberately the smallest one that can work: once a window
/// has committed to a target, that target is what the window keeps. A newly-resolved slot is
/// adopted only when the window has no target yet.
///
/// `latched` is the target this window already committed to (`None` before the first resolution).
/// Returns the slot the window should use, plus whether it is newly latching — callers reset
/// `latched` to `None` on window close, which is what allows the NEXT load to pick a new target.
///
/// It intentionally does NOT try to decide which slot is *correct*: that question belongs to the
/// load path, and a portrait that stays wrong for one window is strictly better than one that
/// changes identity while a user is looking at it.
#[must_use]
pub fn portrait_window_target_slot(
    latched: Option<i32>,
    resolved: Option<i32>,
) -> (Option<i32>, bool) {
    match (latched, resolved) {
        // Window already committed: keep it, whatever the sources say now.
        (Some(held), _) => (Some(held), false),
        // First resolution of this window: adopt it.
        (None, Some(fresh)) => (Some(fresh), true),
        // Nothing named a slot yet; stay uncommitted rather than inventing one.
        (None, None) => (None, false),
    }
}

/// The same-identity bridge hold: may the head published for the PREVIOUS loading window keep
/// displaying across a switch rearm, instead of being cleared as a possible wrong character?
///
/// **THIS PREDICATE CANNOT FAIL ON A SAME-SLOT REPEAT LOAD, AND THAT IS THE POINT OF WRITING IT
/// DOWN HERE.** `incoming_name_hash` is hashed from slot N's ProfileSummary record at rearm time.
/// `published_name_hash` was hashed from slot N's ProfileSummary record at the previous window's
/// build kick and carried to the bridge at publish. Both operands are the same record read twice,
/// so the comparison answers "did this record's name change between the two reads" -- never "does
/// this record describe the character that is about to load". Re-select the same slot and it
/// matches trivially, which is exactly what happened on 2026-08-22 (run br-20260822-040913-f0f4):
/// slot 0's record said `Maddened Bean` while the character actually resident was `Ordinary Bean`,
/// and the hold matched anyway and kept the previous head for the whole window.
///
/// It is kept as-is, because as a CHEAP FIRST FILTER it is still right: a changed name hash proves
/// a different character and must clear. What changed is its STATUS -- a match is now provisional,
/// not a decision, and the caller must arrange for something independent to confirm or revoke it
/// (see [`bridge_hold_face_verdict`]).
///
/// `*_slot_tag` is the wire form used by `LS_PORTRAIT_PUBLISHED_SLOT`: **slot + 1**, so 0 means
/// "no slot". A 0 hash means "unknown name", which is never treated as agreement.
#[must_use]
pub const fn same_identity_bridge_hold(
    have_head: bool,
    incoming_slot_tag: usize,
    incoming_name_hash: usize,
    published_slot_tag: usize,
    published_name_hash: usize,
) -> bool {
    have_head
        && incoming_slot_tag != 0
        && incoming_slot_tag == published_slot_tag
        && incoming_name_hash != 0
        && incoming_name_hash == published_name_hash
}

/// What an independent identity signal says about an outstanding provisional bridge hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeHoldVerdict {
    /// No provisional hold is outstanding -- either none was taken, or this window already
    /// published its own head and superseded it. Nothing to say.
    NoHold,
    /// The signal is about a different slot than the held head. Not evidence either way.
    OtherSlot,
    /// No fingerprint exists for this slot, so the record cannot be checked against anything
    /// outside itself. The hold stays up, still unproven. Fails CLOSED in the sense that matters:
    /// absence of evidence is never reported as agreement.
    NoEvidence,
    /// The record agrees with its fingerprint. Deliberately NOT called "confirmed": it proves the
    /// record was not rewritten under the slot, not that the held head is the loading character
    /// (the 2026-08-22 record was intact against nothing and still named the wrong person). Only
    /// this window publishing its own frame proves that.
    Unrefuted,
    /// The record the portrait is about to build from is a DIFFERENT character than the one whose
    /// fingerprint was taken for this slot. The held head cannot be right; drop it.
    Revoke,
}

/// Judge an outstanding provisional hold against the record-vs-preview FACE fingerprint taken at
/// the build kick.
///
/// This is the only portrait identity signal that compares the ProfileSummary record against a
/// source OUTSIDE that record: `preview_face_hash` is hashed from the picked save's own bytes when
/// the foreign-save preview writes the slot, `record_face_hash` is re-hashed off the live record at
/// the kick. Every other signal in the pipeline (the hold's name hashes, the published-vs-target
/// name hashes, the loadwin `identity=` tag) reads the record on both sides and therefore agrees
/// with itself no matter how wrong the record is -- which is why the 2026-08-22 window closed
/// `identity=ok` while this fingerprint had already disagreed twice.
///
/// It is NOT available when the hold is taken. The hold is decided at the switch rearm; the
/// fingerprint arrives at the first build kick, measured ~1.4s later (`+107006ms` rearm vs
/// `+108385ms` first mismatch). That timing is the whole reason the hold is provisional-then-
/// revocable rather than simply being given a better predicate up front.
///
/// `held_slot_tag` is slot+1 of the outstanding hold (0 = none); `kick_slot` is the raw slot index
/// the kick is building. A 0 `preview_face_hash` means the slot has no fingerprint.
#[must_use]
pub const fn bridge_hold_face_verdict(
    held_slot_tag: usize,
    kick_slot: i32,
    record_face_hash: usize,
    preview_face_hash: usize,
) -> BridgeHoldVerdict {
    if held_slot_tag == 0 {
        return BridgeHoldVerdict::NoHold;
    }
    if kick_slot < 0 || held_slot_tag != (kick_slot as usize) + 1 {
        return BridgeHoldVerdict::OtherSlot;
    }
    if preview_face_hash == 0 {
        return BridgeHoldVerdict::NoEvidence;
    }
    if record_face_hash == preview_face_hash {
        BridgeHoldVerdict::Unrefuted
    } else {
        BridgeHoldVerdict::Revoke
    }
}

impl BridgeHoldVerdict {
    /// True only when the held head must be dropped, so a caller can act without matching on
    /// every arm.
    #[must_use]
    pub const fn revokes(self) -> bool {
        matches!(self, Self::Revoke)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defect the sign gate caused: a garbage word with bit31 set read as "not a real map" and
    /// turned the comparison OFF instead of failing it (2 of 6 logged FAILs). Both of these are
    /// real `CSRandXorshift` dwords lifted from one corpus slot's body+0x10..0x20 block.
    #[test]
    fn a_negative_word_is_rejected_as_implausible_not_treated_as_absent_evidence() {
        assert!(!packed_map_is_plausible(0xd139_52aau32 as i32)); // areaId 0xd1
        assert!(!packed_map_is_plausible(0xa504_8cb5u32 as i32)); // areaId 0xa5
        // A POSITIVE piece of garbage must be rejected too -- the old `> 0` gate passed it.
        assert!(!packed_map_is_plausible(0x3e20_457cu32 as i32)); // areaId 0x3e, one past the max
    }

    /// HONEST LIMIT of the area predicate: it is a shape check, not an identity check. One in
    /// roughly eleven random dwords lands in the valid area range (measured: 68 of 726 corpus
    /// body+0x14 words), so garbage CAN pass. That is precisely why the map term must also be
    /// gated on the record being one WE wrote -- the predicate alone cannot carry the comparison.
    #[test]
    fn a_random_word_can_still_land_inside_the_area_range() {
        assert!(packed_map_is_plausible(0x0e6d_7b38u32 as i32)); // areaId 0x0e -- pure luck
    }

    #[test]
    fn real_corpus_area_ids_are_plausible() {
        // Every distinct areaId observed across 726 active corpus slots.
        for area in [
            0x0au8, 0x0b, 0x0c, 0x0d, 0x0e, 0x10, 0x14, 0x15, 0x1c, 0x20, 0x22, 0x23, 0x3c, 0x3d,
        ] {
            // Low word 0x0002_0000 keeps area 0x0a off the new-game sentinel (0x0a01_0000).
            let map = ((u32::from(area) << 24) | 0x0002_0000) as i32;
            assert!(
                packed_map_is_plausible(map),
                "areaId 0x{area:02x} (map 0x{map:08x}) must be plausible"
            );
        }
        // The real corpus values themselves, verbatim.
        for map in [0x0c01_0000u32, 0x1c00_0000, 0x0e00_0000, 0x3d00_0000] {
            assert!(packed_map_is_plausible(map as i32), "map 0x{map:08x}");
        }
    }

    #[test]
    fn the_new_game_sentinel_is_never_plausible() {
        assert!(!packed_map_is_plausible(DEFAULT_MAP_C30));
        assert_eq!(packed_map_area_id(DEFAULT_MAP_C30), 0x0a);
    }

    #[test]
    fn implausible_values_yield_no_map_evidence_rather_than_a_mismatch() {
        let real_a = 0x0c01_0000u32 as i32;
        let real_b = 0x1c00_0000u32 as i32;
        let garbage = 0x3e20_457cu32 as i32;
        assert!(packed_maps_disagree(real_a, real_b));
        assert!(!packed_maps_disagree(real_a, garbage));
        assert!(!packed_maps_disagree(garbage, real_a));
        assert!(!packed_maps_disagree(real_a, real_a));
        assert!(!packed_maps_disagree(real_a, DEFAULT_MAP_C30));
    }

    /// The measured failure: published slot 9 while slot 5 loaded. The wire tag is slot+1.
    #[test]
    fn the_2026_08_02_wrong_face_window_is_a_slot_mismatch() {
        assert_eq!(
            published_identity_verdict(9 + 1, Some(5)),
            PublishedIdentity::SlotMismatch {
                published: 9,
                loaded: 5
            }
        );
        assert!(published_identity_verdict(9 + 1, Some(5)).is_mismatch());
    }

    /// Slot 0 must be distinguishable from "never published", or every boot either false-fires or
    /// hides a real mismatch.
    #[test]
    fn slot_zero_is_not_confused_with_nothing_published() {
        assert_eq!(
            published_identity_verdict(0, Some(0)),
            PublishedIdentity::NothingPublished
        );
        assert_eq!(
            published_identity_verdict(1, Some(0)),
            PublishedIdentity::Match
        );
        assert_eq!(
            published_identity_verdict(1, Some(3)),
            PublishedIdentity::SlotMismatch {
                published: 0,
                loaded: 3
            }
        );
        assert!(!published_identity_verdict(0, Some(0)).is_mismatch());
    }

    #[test]
    fn with_no_completed_load_there_is_nothing_to_compare() {
        assert_eq!(
            published_identity_verdict(5, None),
            PublishedIdentity::NoLoadedSlot
        );
        assert!(!published_identity_verdict(5, None).is_mismatch());
    }

    /// The precedence change (bd er-effects-rs-91zb): the user's on-screen pick outranks the
    /// request register, which outranks the stale `save_slot`.
    #[test]
    fn the_users_pick_outranks_every_inferred_slot() {
        assert_eq!(
            portrait_target_slot_from_sources(Some(5), Some(7), Some(9), 10),
            Some(5)
        );
    }

    /// The measured boot window: picker had not recorded a pick at that instant, b78 held the
    /// correct 5, ac0 had been dragged to 9 by the game's own selector.
    #[test]
    fn the_request_register_beats_a_stale_save_slot() {
        assert_eq!(
            portrait_target_slot_from_sources(None, Some(5), Some(9), 10),
            Some(5)
        );
    }

    #[test]
    fn the_no_request_sentinel_falls_through_to_save_slot() {
        assert_eq!(
            portrait_target_slot_from_sources(None, Some(-1), Some(9), 10),
            Some(9)
        );
        assert_eq!(
            portrait_target_slot_from_sources(None, None, Some(9), 10),
            Some(9)
        );
        assert_eq!(
            portrait_target_slot_from_sources(None, Some(10), Some(9), 10),
            Some(9),
            "an out-of-range request must not win"
        );
    }

    /// With nothing valid the answer is None, never slot 0. Collapsing to 0 is what built and
    /// published a slot-0 portrait for a non-slot-0 character.
    #[test]
    fn no_valid_source_yields_none_not_slot_zero() {
        assert_eq!(
            portrait_target_slot_from_sources(None, None, None, 10),
            None
        );
        assert_eq!(
            portrait_target_slot_from_sources(Some(-1), Some(-1), Some(-1), 10),
            None
        );
    }

    /// THE 2026-08-02 21:05 REGRESSION, replayed with that run's literal values. The user picked
    /// slot 0; the pipeline latched and published slot 0; then the picker term expired and the
    /// sources started naming slot 9 (`save_slot` = ac0 = 9, `request_slot` = b78 = -1). Before
    /// the latch, kick #2 retargeted the live window to 9 and the face changed on screen.
    #[test]
    fn a_window_that_committed_to_the_picked_slot_never_retargets_mid_load() {
        // Kick #1: picker still names slot 0.
        let resolved = portrait_target_slot_from_sources(Some(0), Some(-1), Some(0), 10);
        assert_eq!(resolved, Some(0));
        let (target, latching) = portrait_window_target_slot(None, resolved);
        assert_eq!(target, Some(0));
        assert!(latching, "the first resolution of a window must latch");

        // Kick #2, same window: picker spent, b78 disarmed, ac0 now 9. The SOURCES flip...
        let resolved_later = portrait_target_slot_from_sources(None, Some(-1), Some(9), 10);
        assert_eq!(
            resolved_later,
            Some(9),
            "precedence really does name slot 9 once the picker term is spent -- this is the input that caused the bug"
        );
        // ...but the WINDOW does not.
        let (target_later, latching_later) = portrait_window_target_slot(target, resolved_later);
        assert_eq!(
            target_later,
            Some(0),
            "the window must keep the character the user clicked"
        );
        assert!(!latching_later);
    }

    /// The latch must not become a permanent pin: window close clears it, and the NEXT load is
    /// free to target a different character. Without this a System->Quit->Load switch would show
    /// the boot character's face forever -- the same defect in the opposite direction.
    #[test]
    fn a_new_window_is_free_to_target_a_different_character() {
        let (first, _) = portrait_window_target_slot(None, Some(0));
        assert_eq!(first, Some(0));
        // Window closes -> caller resets the latch to None.
        let (second, latching) = portrait_window_target_slot(None, Some(9));
        assert_eq!(second, Some(9));
        assert!(latching);
    }

    /// A window that has not resolved any slot yet must stay uncommitted rather than latch a
    /// placeholder -- latching `0` here would reintroduce the `unwrap_or(0)` lie the rest of this
    /// module exists to remove.
    #[test]
    fn an_unresolved_window_latches_nothing() {
        assert_eq!(portrait_window_target_slot(None, None), (None, false));
        // ...and a later real resolution still latches normally.
        assert_eq!(portrait_window_target_slot(None, Some(3)), (Some(3), true));
    }

    /// Literal values from run br-20260822-040913-f0f4, window #3.
    const RUN_HELD_NAME_HASH: usize = 0x909a_2595_c413_a1b3;
    const RUN_RECORD_FACE_HASH: usize = 0xbbd2_ad40_6f84_9c65;
    const RUN_PREVIEW_FACE_HASH: usize = 0xc6af_b8c3_7ec7_b617;

    /// THE DEFECT, stated as a test: on a same-slot repeat the hold predicate agrees with itself.
    ///
    /// Both hashes are slot 0's ProfileSummary record read at two different times, so re-selecting
    /// the same slot makes them equal by construction -- even in the measured run where that record
    /// said `Maddened Bean` and slot 0's own deserialize produced `Ordinary Bean`. This test is not
    /// asserting desirable behaviour; it pins the blind spot that makes the hold provisional.
    #[test]
    fn the_same_slot_repeat_hold_matches_even_when_the_record_is_wrong() {
        assert!(
            same_identity_bridge_hold(true, 1, RUN_HELD_NAME_HASH, 1, RUN_HELD_NAME_HASH),
            "a same-slot reselect always matches: this predicate compares one record with itself"
        );
    }

    /// The cases it still decides correctly, which is why it survives as the first filter.
    #[test]
    fn a_changed_name_or_slot_or_missing_head_still_clears() {
        // Different character in the same slot -- the record's name DID change.
        assert!(!same_identity_bridge_hold(true, 1, 0xaaaa, 1, 0xbbbb));
        // Different slot entirely.
        assert!(!same_identity_bridge_hold(true, 2, 0xaaaa, 1, 0xaaaa));
        // Nothing published to hold on to.
        assert!(!same_identity_bridge_hold(false, 1, 0xaaaa, 1, 0xaaaa));
        // Unknown name on either side is never agreement.
        assert!(!same_identity_bridge_hold(true, 1, 0, 1, 0));
        // No incoming slot.
        assert!(!same_identity_bridge_hold(true, 0, 0xaaaa, 0, 0xaaaa));
    }

    /// The revocation, replayed with the run's literal hashes: the hold was taken on slot 0 at
    /// +107006ms, and the first build kick for slot 0 at +108385ms found a record whose face was
    /// not the one the preview fingerprinted. That is the falsification the hold could not produce
    /// for itself.
    #[test]
    fn the_2026_08_22_face_mismatch_revokes_the_held_head() {
        let verdict = bridge_hold_face_verdict(1, 0, RUN_RECORD_FACE_HASH, RUN_PREVIEW_FACE_HASH);
        assert_eq!(verdict, BridgeHoldVerdict::Revoke);
        assert!(verdict.revokes());
    }

    /// MAKE-BEFORE-BREAK MUST SURVIVE. Windows 1/2/4 of the same run held and then published
    /// 259-281 frames each; an intact record hands back `Unrefuted`, which drops nothing. A fix
    /// that revoked here would turn every legitimate same-character reload back into a flash of
    /// empty loading screen.
    #[test]
    fn an_intact_record_does_not_revoke_a_legitimate_hold() {
        let verdict = bridge_hold_face_verdict(1, 0, RUN_PREVIEW_FACE_HASH, RUN_PREVIEW_FACE_HASH);
        assert_eq!(verdict, BridgeHoldVerdict::Unrefuted);
        assert!(!verdict.revokes());
    }

    /// The three arms that must never revoke: no hold outstanding, a kick for another slot, and a
    /// slot with no fingerprint to compare against. Absence of evidence is not refutation -- but it
    /// is not agreement either, which is why `NoEvidence` is its own answer rather than `Unrefuted`.
    #[test]
    fn absent_or_unrelated_evidence_never_revokes() {
        assert_eq!(
            bridge_hold_face_verdict(0, 0, RUN_RECORD_FACE_HASH, RUN_PREVIEW_FACE_HASH),
            BridgeHoldVerdict::NoHold
        );
        assert_eq!(
            bridge_hold_face_verdict(1, 4, RUN_RECORD_FACE_HASH, RUN_PREVIEW_FACE_HASH),
            BridgeHoldVerdict::OtherSlot
        );
        assert_eq!(
            bridge_hold_face_verdict(1, -1, RUN_RECORD_FACE_HASH, RUN_PREVIEW_FACE_HASH),
            BridgeHoldVerdict::OtherSlot
        );
        assert_eq!(
            bridge_hold_face_verdict(1, 0, RUN_RECORD_FACE_HASH, 0),
            BridgeHoldVerdict::NoEvidence
        );
        for verdict in [
            BridgeHoldVerdict::NoHold,
            BridgeHoldVerdict::OtherSlot,
            BridgeHoldVerdict::NoEvidence,
        ] {
            assert!(!verdict.revokes());
        }
    }

    /// Slot 0 must be distinguishable from "no hold", the same +1 biasing trap the published-slot
    /// tag has: without it a hold on slot 0 would be unrevokable.
    #[test]
    fn a_hold_on_slot_zero_is_not_read_as_no_hold() {
        // Tag 1 IS slot 0. The whole measured failure sat on slot 0.
        assert_eq!(
            bridge_hold_face_verdict(1, 0, RUN_RECORD_FACE_HASH, RUN_PREVIEW_FACE_HASH),
            BridgeHoldVerdict::Revoke
        );
        assert_eq!(
            bridge_hold_face_verdict(0, 0, RUN_RECORD_FACE_HASH, RUN_PREVIEW_FACE_HASH),
            BridgeHoldVerdict::NoHold,
            "tag 0 is 'no hold', never slot 0"
        );
    }
}
