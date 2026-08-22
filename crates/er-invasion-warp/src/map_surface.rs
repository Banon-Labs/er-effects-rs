//! Which invasion points become map pins, and how a pin is recognised on the way back.
//!
//! # Why not all 7073
//!
//! The catalog holds 7073 points. A `CS::WorldMapWarpPinData` row is `0x350` bytes and owns a
//! `MenuString` plus a `DLFixedVector<MenuString, 8>` of resolved label strings, so 7073 rows is
//! roughly 6 MB of rows before their string allocations -- injected into a MenuHeap, walked by
//! two list builders on every tab change, and drawn as 7073 pins on one map. The RE flagged that
//! survivability as an open question rather than an established fact, which is reason enough not
//! to bet the feature on it.
//!
//! It is also the wrong surface. Auto-invasion points cluster densely inside a tile; 20 pins
//! within a few metres of each other are not 20 destinations a person can choose between. The
//! useful unit for exploration is "take me to this tile's invasion spawn", so the default
//! granularity is ONE pin per block ([`PinGranularity::PerBlock`]) -- 365 pins, the same order
//! as the game's own Site-of-Grace count, and each one still warps to a real authored point.
//!
//! [`PinGranularity::PerPoint`] keeps every target for anyone who wants it, and exists so the
//! choice is a parameter with a stated cost rather than a silent cap. Whatever it is set to,
//! [`InvasionRowRegistry::len`] is what actually gets injected -- nothing is dropped later
//! without the count reflecting it.
//!
//! # The recognition scheme
//!
//! The engine reads the row's bonfire entity id and hands it to the warp-job assembler, so the
//! id is the natural place to encode "this is ours". Each injected row gets a synthetic id in a
//! private band; a confirm hook recognises one by range and maps it back to the exact
//! [`InvasionWarpTarget`] to warp to.
//!
//! The id lives at row `+0x50`, NOT `+0x238` -- `+0x238` is the 16-byte
//! `BonfireWarpParamLookupResult` (`{paramId, pad, BonfireWarpParam*}`), and the row constructor
//! copies the id into `+0x50` from the param row's `+0x08` (mapping `-1` to `0`). So the id a
//! synthetic row carries is whatever its DLL-owned `BonfireWarpParam` puts at `+0x08`; it is not
//! written into the row directly.
//!
//! The band must not collide with a real bonfire entity id. Real ids are map-derived and sit
//! well below `0x4000_0000`; the band here starts at [`INVASION_ENTITY_ID_BASE`], far above
//! them and still clear of `i32::MAX`. A collision would not crash -- the param lookup misses
//! and returns NULL, and every caller null-checks -- it would make a real grace warp silently
//! run our code, which is worse. Hence the deliberate distance rather than a tight fit.

use std::collections::BTreeSet;

use crate::invasion_warp::{BlockKey, InvasionWarpCatalog, InvasionWarpTarget};

/// First synthetic bonfire entity id. Chosen far above real map-derived ids (which sit below
/// `0x4000_0000`) so the band cannot be reached by a shipped row, and below `i32::MAX` so the
/// whole band stays a positive `i32` -- `GetBonfireEntityId` answers `-1` as `0`, so a negative
/// id is indistinguishable from "none".
pub const INVASION_ENTITY_ID_BASE: i32 = 0x7F00_0000;

/// Size of the private band. Comfortably larger than the 7073-point catalog, so even
/// [`PinGranularity::PerPoint`] fits without the base having to move.
pub const INVASION_ENTITY_ID_COUNT: i32 = 0x0010_0000;

/// True when `entity_id` is one of ours.
///
/// Range-checked rather than "greater than base": an id past the end of the band is NOT ours,
/// and treating it as ours would index off the end of the registry.
#[must_use]
pub const fn is_invasion_entity_id(entity_id: i32) -> bool {
    entity_id >= INVASION_ENTITY_ID_BASE
        && (entity_id as i64) < (INVASION_ENTITY_ID_BASE as i64 + INVASION_ENTITY_ID_COUNT as i64)
}

/// How many pins the surface offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PinGranularity {
    /// One pin per block, using that block's first authored point. 365 pins for the shipped
    /// catalog. The default, for the reasons in the module docs.
    #[default]
    PerBlock,
    /// Every catalog target. 7073 pins; see the module docs before choosing this.
    PerPoint,
}

/// The injected rows, and the mapping from a synthetic entity id back to what to warp to.
///
/// Built once from the catalog and then read-only, so the confirm hook's lookup is a bounds
/// check and an index -- no locking on the path the engine calls.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InvasionRowRegistry {
    targets: Vec<InvasionWarpTarget>,
}

impl InvasionRowRegistry {
    /// Select the pin set from a catalog.
    ///
    /// `PerBlock` takes each block's FIRST target. The catalog is sorted by block then point
    /// index, so "first" is deterministic across runs rather than whichever the walk happened
    /// to reach first.
    #[must_use]
    pub fn from_catalog(catalog: &InvasionWarpCatalog, granularity: PinGranularity) -> Self {
        let targets = match granularity {
            PinGranularity::PerPoint => catalog.targets().to_vec(),
            PinGranularity::PerBlock => {
                let mut picked: Vec<InvasionWarpTarget> = Vec::new();
                let mut current: Option<BlockKey> = None;
                for target in catalog.targets() {
                    if current != Some(target.block) {
                        current = Some(target.block);
                        picked.push(*target);
                    }
                }
                picked
            }
        };
        Self::from_targets(targets)
    }

    /// Build directly from an already-chosen target list, truncating to the band size.
    ///
    /// Truncation is a hard cap, not a silent one: [`Self::len`] is the number actually
    /// registered, and a caller that logs it reports the real pin count.
    #[must_use]
    pub fn from_targets(mut targets: Vec<InvasionWarpTarget>) -> Self {
        targets.truncate(INVASION_ENTITY_ID_COUNT as usize);
        Self { targets }
    }

    /// How many pins this registry will inject.
    #[must_use]
    pub fn len(&self) -> usize {
        self.targets.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// The targets, in injection order. Row `i` carries [`Self::entity_id_at`]`(i)`.
    #[must_use]
    pub fn targets(&self) -> &[InvasionWarpTarget] {
        &self.targets
    }

    /// The synthetic bonfire entity id row `index` must carry at `+0x238`.
    #[must_use]
    pub fn entity_id_at(&self, index: usize) -> Option<i32> {
        if index >= self.targets.len() {
            return None;
        }
        // In range by construction: `targets` is truncated to the band size.
        Some(INVASION_ENTITY_ID_BASE + index as i32)
    }

    /// Map a synthetic entity id back to the target to warp to.
    ///
    /// `None` for a real bonfire id, for an id inside the band but past the registered rows, and
    /// for anything else -- so a confirm hook that gets this wrong falls through to the native
    /// warp rather than warping somewhere arbitrary.
    #[must_use]
    pub fn target_for_entity_id(&self, entity_id: i32) -> Option<&InvasionWarpTarget> {
        if !is_invasion_entity_id(entity_id) {
            return None;
        }
        let index = (entity_id - INVASION_ENTITY_ID_BASE) as usize;
        self.targets.get(index)
    }

    /// How many distinct blocks the pin set covers -- the number a log line should report
    /// alongside the pin count so a decimated set is visibly decimated.
    #[must_use]
    pub fn block_count(&self) -> usize {
        let mut blocks: Vec<BlockKey> = self.targets.iter().map(|t| t.block).collect();
        blocks.sort_unstable_by_key(|block| block.raw());
        blocks.dedup();
        blocks.len()
    }
}

/// Harvested points that are not on the live map yet.
///
/// # Why identity is `(block, point)` and never `block`
///
/// A legacy dungeon the player has not entered carries a WHOLE-DUNGEON marker: one pin at the
/// dungeon's centre, placed precisely because its interior was unknown
/// ([`InvasionWarpTarget::provisional`]). The moment its MSB becomes resident its real points
/// arrive -- and every one of them is in that same block. Asking "does this block already have a
/// pin?" answers yes, and the dungeon's entire per-point set is discarded.
///
/// That has now happened twice, in two different filters. The first was the `.aip` suppression set
/// in the injection path, which is why that set is restricted to non-legacy areas. The second was
/// this test in the live top-up, where it made the whole function a permanent no-op: measured
/// 2026-08-05, the Haligtree's 9 markers were offered on ~35,900 consecutive frames and refused on
/// every one. Hence a named function with the rule written down, rather than a set comprehension
/// inlined at each site.
///
/// `already` holds `(block, point_index)` pairs that are shown, claimed, or known unplaceable.
#[must_use]
pub fn points_not_yet_shown(
    harvested: &[InvasionWarpTarget],
    already: &BTreeSet<(u32, u32)>,
) -> Vec<InvasionWarpTarget> {
    harvested
        .iter()
        .filter(|target| !already.contains(&(target.block.raw(), target.point_index)))
        .copied()
        .collect()
}

/// The claimed prefix of a reserved dormant span, if it is safely inside the live row list.
///
/// # Why this is a named, tested function
///
/// A world-map pin the player can see is not necessarily in the injected span. A legacy dungeon
/// harvested mid-session has its markers written into rows RESERVED as dormant by the constructor
/// and claimed later. Anything that walks "our rows" must cover both, and the live re-colour did
/// not: marking such a dungeon repainted exactly one row -- the whole-dungeon marker that had
/// already been hidden -- while every marker actually on screen went untouched.
///
/// Only the CLAIMED prefix qualifies. Unclaimed dormant rows are blank, carry a zero layer mask and
/// are not drawn, so they have no appearance to change.
///
/// Returns `None` when nothing is claimed or when the span does not lie wholly inside
/// `list_begin..list_end` -- a span outside the live list describes a buffer that has been freed.
#[must_use]
pub fn claimed_dormant_span(
    dormant_begin: usize,
    claimed_slots: usize,
    row_stride: usize,
    list_begin: usize,
    list_end: usize,
) -> Option<(usize, usize)> {
    if dormant_begin == 0 || claimed_slots == 0 || row_stride == 0 {
        return None;
    }
    let end = dormant_begin.checked_add(claimed_slots.checked_mul(row_stride)?)?;
    if dormant_begin < list_begin || end > list_end {
        return None;
    }
    Some((dormant_begin, end))
}

/// A pin's ordinal among the pins sharing its place name. `0` means it has no sibling.
pub type PlaceOrdinal = u32;

/// The ordinal of a pin whose name is unique, which is not shown.
pub const PLACE_ORDINAL_UNIQUE: PlaceOrdinal = 0;

/// Number the pins that share a place name, so a player can say "meet at X, 2".
///
/// # What "deterministic" must mean, and what it cannot
///
/// Two players have to arrive at the SAME number for the same spot or the feature is worse than
/// useless -- it sends them to different places while they agree out loud. That rules out numbering
/// by position in the currently-known pin set: a legacy dungeon's per-point markers only exist once
/// that map has been resident, so the player who has been to fewer places would number the same
/// point differently.
///
/// So the ordinal is derived ONLY from properties intrinsic to the point -- `(block, point_index)`.
/// `point_index` is the point's index in its own map's MSB region list and `block` is that map's
/// id; both are fixed by the game version, not by what the player has seen. Sorting a name group by
/// that pair yields the same sequence on any machine that knows the same points.
///
/// # The limit, stated rather than hidden
///
/// A dungeon's points arrive together -- one MSB read yields the whole map's set -- so two players
/// inside the same dungeon always agree, which is the case that matters for arranging a meeting.
/// A place name shared across SEPARATE blocks, where one player has visited a block the other has
/// not, can still disagree. Nothing available at runtime fixes that: the absent block's point count
/// is unknowable until it is read.
///
/// Returns one ordinal per input, in the input's own order.
#[must_use]
pub fn number_shared_place_names(pins: &[(InvasionWarpTarget, i32)]) -> Vec<PlaceOrdinal> {
    use std::collections::BTreeMap;

    // A negative id is "no name": those pins are never drawn, so they neither get a number nor
    // pull anything else into a group.
    let mut groups: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for (at, (_, place_name)) in pins.iter().enumerate() {
        if *place_name >= 0 {
            groups.entry(*place_name).or_default().push(at);
        }
    }

    let mut ordinals = vec![PLACE_ORDINAL_UNIQUE; pins.len()];
    for members in groups.values() {
        if members.len() < 2 {
            continue;
        }
        let mut ordered = members.clone();
        // Sort by the INTRINSIC key, never by input order -- the harvest order must not be able to
        // change what a player is told to meet at.
        ordered.sort_by_key(|at| {
            let target = &pins[*at].0;
            (target.block.raw(), target.point_index)
        });
        for (rank, at) in ordered.into_iter().enumerate() {
            ordinals[at] = u32::try_from(rank + 1).unwrap_or(u32::MAX);
        }
    }
    ordinals
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRIDE: usize = 0x350;

    fn pin(block: u32, point: u32, place_name: i32) -> (InvasionWarpTarget, i32) {
        (
            InvasionWarpTarget::new(BlockKey::from_raw(block), point, [0.0; 3], 0.0),
            place_name,
        )
    }

    #[test]
    fn pins_sharing_a_name_are_numbered_from_one() {
        let pins = [pin(0x0f00_0000, 3, 700), pin(0x0f00_0000, 1, 700)];
        // Ordered by point_index, so the SECOND input is number 1.
        assert_eq!(number_shared_place_names(&pins), vec![2, 1]);
    }

    /// THE INVARIANT THE WHOLE FEATURE RESTS ON. If harvest order could change the numbering, two
    /// players would be sent to different places while saying the same words.
    #[test]
    fn the_numbering_does_not_depend_on_input_order() {
        let a = pin(0x0f00_0000, 9, 700);
        let b = pin(0x0f00_0000, 1, 700);
        let c = pin(0x1c00_0000, 4, 700);
        assert_eq!(number_shared_place_names(&[a, b, c]), vec![2, 1, 3]);
        // Same points, reversed input -- each point keeps its own number.
        assert_eq!(number_shared_place_names(&[c, b, a]), vec![3, 1, 2]);
    }

    #[test]
    fn a_unique_name_is_not_numbered() {
        let pins = [pin(0x0f00_0000, 1, 700), pin(0x1c00_0000, 1, 800)];
        assert_eq!(
            number_shared_place_names(&pins),
            vec![PLACE_ORDINAL_UNIQUE, PLACE_ORDINAL_UNIQUE]
        );
    }

    #[test]
    fn separate_names_are_numbered_separately() {
        let pins = [
            pin(0x0f00_0000, 1, 700),
            pin(0x0f00_0000, 2, 700),
            pin(0x1c00_0000, 5, 800),
            pin(0x1c00_0000, 6, 800),
        ];
        assert_eq!(number_shared_place_names(&pins), vec![1, 2, 1, 2]);
    }

    /// An unnamed pin is never drawn, so numbering it would be meaningless -- and it must not form
    /// a phantom group with other unnamed pins.
    #[test]
    fn unnamed_pins_are_skipped_and_do_not_form_a_group() {
        let pins = [pin(0x0f00_0000, 1, -1), pin(0x1c00_0000, 2, -1)];
        assert_eq!(
            number_shared_place_names(&pins),
            vec![PLACE_ORDINAL_UNIQUE, PLACE_ORDINAL_UNIQUE]
        );
    }

    #[test]
    fn a_name_spanning_two_blocks_orders_by_block_first() {
        let pins = [pin(0x1c00_0000, 0, 700), pin(0x0f00_0000, 99, 700)];
        assert_eq!(number_shared_place_names(&pins), vec![2, 1]);
    }

    #[test]
    fn no_pins_yields_no_ordinals() {
        assert!(number_shared_place_names(&[]).is_empty());
    }

    /// THE REGRESSION: rows a live top-up claimed must be walked, or marking the dungeon they
    /// belong to changes nothing the player can see.
    #[test]
    fn the_claimed_prefix_is_returned_and_stops_at_the_last_claim() {
        let span = claimed_dormant_span(0x1000, 9, STRIDE, 0x0, 0x1000 + 512 * STRIDE);
        assert_eq!(span, Some((0x1000, 0x1000 + 9 * STRIDE)));
    }

    /// Unclaimed rows are blank and undrawn; walking them would repaint nothing and cost a pass
    /// over 512 rows.
    #[test]
    fn nothing_claimed_yields_no_span() {
        assert_eq!(
            claimed_dormant_span(0x1000, 0, STRIDE, 0x0, 0x100_000),
            None
        );
    }

    /// A span outside the live list describes a freed buffer. Writing through it is the crash this
    /// module's ownership rules exist to prevent.
    #[test]
    fn a_span_outside_the_live_list_is_refused() {
        // Ends past the list.
        assert_eq!(
            claimed_dormant_span(0x1000, 9, STRIDE, 0x1000, 0x1000 + 8 * STRIDE),
            None
        );
        // Starts before the list.
        assert_eq!(
            claimed_dormant_span(0x900, 1, STRIDE, 0x1000, 0x100_000),
            None
        );
    }

    #[test]
    fn an_unrecorded_span_is_refused_rather_than_treated_as_address_zero() {
        assert_eq!(claimed_dormant_span(0, 9, STRIDE, 0, 0x100_000), None);
    }

    /// Arithmetic that would wrap must refuse, not produce an in-range-looking span.
    #[test]
    fn an_overflowing_claim_count_is_refused() {
        assert_eq!(
            claimed_dormant_span(usize::MAX - 1, 9, STRIDE, 0, usize::MAX),
            None
        );
        assert_eq!(
            claimed_dormant_span(0x1000, usize::MAX, STRIDE, 0, usize::MAX),
            None
        );
    }

    /// THE REGRESSION. A whole-dungeon marker and the dungeon's real points share a block, so a
    /// block-keyed test hides exactly the points the marker exists to be replaced by.
    #[test]
    fn a_whole_dungeon_marker_does_not_suppress_its_own_blocks_real_points() {
        let dungeon = BlockKey::from_raw(0x1c00_0000);
        let marker = InvasionWarpTarget::provisional(dungeon);
        let points: Vec<InvasionWarpTarget> = (0..3)
            .map(|index| InvasionWarpTarget::new(dungeon, index, [1.0, 2.0, 3.0], 0.0))
            .collect();
        let already: BTreeSet<(u32, u32)> = [(marker.block.raw(), marker.point_index)]
            .into_iter()
            .collect();
        let fresh = points_not_yet_shown(&points, &already);
        assert_eq!(fresh.len(), 3, "all three points are still to be shown");
        // And the block-keyed test it replaced would have returned none of them.
        let block_keyed: Vec<&InvasionWarpTarget> = points
            .iter()
            .filter(|t| t.block.raw() != marker.block.raw())
            .collect();
        assert!(
            block_keyed.is_empty(),
            "block-keyed filtering discards the whole dungeon"
        );
    }

    /// A point already placed must not be offered again -- re-offering burns one reserved row per
    /// frame and stacks duplicate markers on the same spot.
    #[test]
    fn a_point_already_shown_is_not_offered_again() {
        let dungeon = BlockKey::from_raw(0x1c00_0000);
        let points: Vec<InvasionWarpTarget> = (0..3)
            .map(|index| InvasionWarpTarget::new(dungeon, index, [0.0; 3], 0.0))
            .collect();
        let already: BTreeSet<(u32, u32)> = [(dungeon.raw(), 0), (dungeon.raw(), 2)]
            .into_iter()
            .collect();
        let fresh = points_not_yet_shown(&points, &already);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].point_index, 1);
    }

    /// The same point index in a DIFFERENT block is a different place.
    #[test]
    fn the_same_point_index_in_another_block_is_still_fresh() {
        let a = BlockKey::from_raw(0x1c00_0000);
        let b = BlockKey::from_raw(0x1d00_0000);
        let points = vec![
            InvasionWarpTarget::new(a, 0, [0.0; 3], 0.0),
            InvasionWarpTarget::new(b, 0, [0.0; 3], 0.0),
        ];
        let already: BTreeSet<(u32, u32)> = [(a.raw(), 0)].into_iter().collect();
        let fresh = points_not_yet_shown(&points, &already);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].block, b);
    }

    #[test]
    fn nothing_harvested_yields_nothing_fresh() {
        assert!(points_not_yet_shown(&[], &BTreeSet::new()).is_empty());
    }

    fn block(area: u8, index: u8) -> BlockKey {
        BlockKey::from_parts(area, 34, 51, index)
    }

    fn catalog(points: &[(BlockKey, u32)]) -> InvasionWarpCatalog {
        InvasionWarpCatalog::from_targets(
            points
                .iter()
                .map(|(b, i)| InvasionWarpTarget::new(*b, *i, [1.0, 2.0, 3.0], -0.5))
                .collect(),
        )
    }

    #[test]
    fn per_block_takes_one_pin_per_block_not_one_per_point() {
        let catalog = catalog(&[
            (block(60, 0), 0),
            (block(60, 0), 1),
            (block(60, 0), 2),
            (block(60, 1), 0),
            (block(60, 1), 1),
        ]);
        let registry = InvasionRowRegistry::from_catalog(&catalog, PinGranularity::PerBlock);
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.block_count(), 2);
    }

    #[test]
    fn per_block_takes_the_first_point_of_each_block_deterministically() {
        let catalog = catalog(&[(block(60, 0), 0), (block(60, 0), 7), (block(60, 1), 3)]);
        let registry = InvasionRowRegistry::from_catalog(&catalog, PinGranularity::PerBlock);
        let indices: Vec<u32> = registry.targets().iter().map(|t| t.point_index).collect();
        // The catalog is sorted, so "first" is the lowest point index in each block.
        assert_eq!(indices, vec![0, 3]);
    }

    #[test]
    fn per_point_keeps_every_target() {
        let catalog = catalog(&[(block(60, 0), 0), (block(60, 0), 1), (block(60, 1), 0)]);
        let registry = InvasionRowRegistry::from_catalog(&catalog, PinGranularity::PerPoint);
        assert_eq!(registry.len(), 3);
        assert_eq!(registry.block_count(), 2);
    }

    #[test]
    fn per_block_is_the_default_granularity() {
        assert_eq!(PinGranularity::default(), PinGranularity::PerBlock);
    }

    #[test]
    fn entity_ids_are_dense_from_the_band_base_and_round_trip() {
        let catalog = catalog(&[(block(60, 0), 0), (block(60, 1), 0), (block(61, 0), 0)]);
        let registry = InvasionRowRegistry::from_catalog(&catalog, PinGranularity::PerBlock);
        for index in 0..registry.len() {
            let id = registry.entity_id_at(index).expect("id in range");
            assert_eq!(id, INVASION_ENTITY_ID_BASE + index as i32);
            assert!(is_invasion_entity_id(id));
            assert_eq!(
                registry.target_for_entity_id(id),
                Some(&registry.targets()[index])
            );
        }
    }

    #[test]
    fn a_real_bonfire_entity_id_is_never_claimed_as_ours() {
        // The failure this guards: a real grace warp silently running the invasion warp.
        let catalog = catalog(&[(block(60, 0), 0)]);
        let registry = InvasionRowRegistry::from_catalog(&catalog, PinGranularity::PerBlock);
        for real in [0_i32, 1, 71190, 1_042_361_950, 0x3FFF_FFFF] {
            assert!(!is_invasion_entity_id(real), "{real}");
            assert_eq!(registry.target_for_entity_id(real), None, "{real}");
        }
    }

    #[test]
    fn an_id_inside_the_band_but_past_the_registered_rows_resolves_to_nothing() {
        // Falls through to the native warp instead of indexing off the end.
        let catalog = catalog(&[(block(60, 0), 0)]);
        let registry = InvasionRowRegistry::from_catalog(&catalog, PinGranularity::PerBlock);
        let unused = INVASION_ENTITY_ID_BASE + 500;
        assert!(is_invasion_entity_id(unused));
        assert_eq!(registry.target_for_entity_id(unused), None);
    }

    #[test]
    fn the_band_is_bounded_at_both_ends() {
        assert!(!is_invasion_entity_id(INVASION_ENTITY_ID_BASE - 1));
        assert!(is_invasion_entity_id(INVASION_ENTITY_ID_BASE));
        let last = INVASION_ENTITY_ID_BASE + INVASION_ENTITY_ID_COUNT - 1;
        assert!(is_invasion_entity_id(last));
        assert!(!is_invasion_entity_id(last + 1));
    }

    #[test]
    fn the_whole_band_stays_a_positive_i32() {
        // GetBonfireEntityId answers -1 as 0, so a negative synthetic id would be
        // indistinguishable from "no bonfire".
        const { assert!(INVASION_ENTITY_ID_BASE > 0) };
        let end = INVASION_ENTITY_ID_BASE as i64 + INVASION_ENTITY_ID_COUNT as i64;
        assert!(end <= i32::MAX as i64, "band overflows i32");
    }

    #[test]
    fn the_band_is_large_enough_for_the_whole_shipped_catalog() {
        // 7073 points with DLC; PerPoint must fit without moving the base.
        const { assert!(INVASION_ENTITY_ID_COUNT > 7073) };
    }

    #[test]
    fn an_out_of_range_index_has_no_entity_id() {
        let catalog = catalog(&[(block(60, 0), 0)]);
        let registry = InvasionRowRegistry::from_catalog(&catalog, PinGranularity::PerBlock);
        assert_eq!(registry.entity_id_at(registry.len()), None);
    }

    #[test]
    fn an_empty_catalog_yields_an_empty_registry_rather_than_a_phantom_pin() {
        let registry = InvasionRowRegistry::from_catalog(
            &InvasionWarpCatalog::from_targets(Vec::new()),
            PinGranularity::PerBlock,
        );
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert_eq!(registry.block_count(), 0);
        assert_eq!(registry.entity_id_at(0), None);
    }
}
