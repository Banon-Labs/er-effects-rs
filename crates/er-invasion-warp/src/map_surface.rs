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
//! The engine reads the row's bonfire entity id at `+0x238` and hands it to the warp-job
//! assembler, so the id is the natural place to encode "this is ours". Each injected row gets a
//! synthetic id in a private band; a confirm hook recognises one by range and maps it back to
//! the exact [`InvasionWarpTarget`] to warp to.
//!
//! The band must not collide with a real bonfire entity id. Real ids are map-derived and sit
//! well below `0x4000_0000`; the band here starts at [`INVASION_ENTITY_ID_BASE`], far above
//! them and still clear of `i32::MAX`. A collision would not crash -- the param lookup misses
//! and returns NULL, and every caller null-checks -- it would make a real grace warp silently
//! run our code, which is worse. Hence the deliberate distance rather than a tight fit.

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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(INVASION_ENTITY_ID_BASE > 0);
        let end = INVASION_ENTITY_ID_BASE as i64 + INVASION_ENTITY_ID_COUNT as i64;
        assert!(end <= i32::MAX as i64, "band overflows i32");
    }

    #[test]
    fn the_band_is_large_enough_for_the_whole_shipped_catalog() {
        // 7073 points with DLC; PerPoint must fit without moving the base.
        assert!(INVASION_ENTITY_ID_COUNT > 7073);
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
