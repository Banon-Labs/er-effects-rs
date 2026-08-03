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
            published_identity_verdict(0 + 1, Some(0)),
            PublishedIdentity::Match
        );
        assert_eq!(
            published_identity_verdict(0 + 1, Some(3)),
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
}
