//! Invasion spawn points for the maps the `.aip` table does not cover.
//!
//! # Why a second source exists at all
//!
//! `other:/AutoInvadePoint.aipbnd` holds 7073 points across 365 blocks -- and every one of them is
//! in area **60** (Lands Between overworld) or **61** (Shadow Lands). Leyndell, Stormveil, Farum
//! Azula, the Haligtree, every cave, catacomb and tunnel have **no `.aip` entries whatsoever**, so
//! a map surface built only from that table can never show a marker in them.
//!
//! That is not an oversight in the shipped data. The engine selects between two entirely separate
//! mechanisms on a param bit: `CS::PlayRegionParamLookupResult::isAutoIntrudePoint`
//! (`0x140d44a20`) returns `_PLAY_REGION_PARAM_ST` byte `0x45` bit 0. Of the 593 `PlayRegionParam`
//! rows in the shipped `regulation.bin`, exactly **90** have that bit set, and all 90 are area 60,
//! area 61, or an `areaNo == 0` row whose id sits in the 6100000..=6941010 overworld band. Every
//! row for areas 10..=45 has it clear. `CSBreakInPointManager` accordingly has two consumers:
//!
//! * `_GetCurBreakInPointVecFromAutoIntrudePoint` (`0x140a0c4f0`) -- the `.aip` path, and
//! * `FUN_140a0c100` (`0x140a0c100`) -- the general path, which enumerates **MSB `POINT_PARAM_ST`
//!   regions of subtype `InvasionPoint`** out of each resident map.
//!
//! This module is the second one. An offline harvest of all 1347 shipped MSBs found **2807**
//! `InvasionPoint` regions across 113 maps, 2596 of them outside the overworld (Leyndell 168,
//! Farum Azula 119, Volcano Manor 115, Stormveil 94, Haligtree 88, catacombs 285, caves 229,
//! the m12 underground 399, ...). That harvest is a CHECK, never the product's data: per the
//! project rule the surface must reflect what is actually loaded, because a mod can rewrite it.
//!
//! # Why the catalog accumulates instead of being read once
//!
//! `.aip` is a single global table (`CSAutoInvadePoint`) that is resident for the whole session,
//! so it can be read once and be complete. MSB point data is **per map**, lives on that map's
//! `MsbResCap`, and is evicted when the map unloads. There is no moment at which every map's
//! points are simultaneously readable. So a complete-in-one-read design is not merely awkward
//! here, it is impossible.
//!
//! What is possible is to read whichever maps are resident right now and *remember* them. Every
//! visit adds coverage and nothing is ever lost, so the surface strictly improves as the session
//! goes on. The alternative -- resolving non-resident maps by reading their MSB through the
//! engine's own virtual file system -- is a larger piece of work and is deliberately not attempted
//! here; this type is the accumulator either path needs.
//!
//! Nothing in this module is a multiplayer call. `CSBreakInPointManager` is where session state
//! lives and it is not entered: the point geometry is read, the same way the `.aip` table is.

use crate::invasion_warp::BlockKey;

/// `MsbPointType::InvasionPoint`, the `EDX` value byte-verified at `0x140a0c1f1` in the general
/// break-in-point consumer's call to [`GET_POINT_DATA_SECTION_ITEM_COUNT_RVA`].
pub const MSB_POINT_TYPE_INVASION_POINT: u32 = 1;

/// `CS::MsbResCap::GetPointDataSectionItemCount(MsbResCap*, MsbPointType)` -- `0x140cf6300`.
///
/// Preferred over walking `MsbResCap+0x318 + type*0x10` by hand: the count is the sum of that
/// static TOC entry AND a dynamic overflow vector at `MsbResCap+0xa70 + type*0x18`, so a
/// pointer-walk that knows only about the TOC silently undercounts any map that populates the
/// vector.
pub const GET_POINT_DATA_SECTION_ITEM_COUNT_RVA: usize = 0xcf_6300;
/// `CS::CSMsbPoint::CSMsbPoint(out, MsbResCap*, 0, MsbPointType, index)` -- `0x140cf9300`.
pub const CS_MSB_POINT_CTOR_RVA: usize = 0xcf_9300;
/// `CS::CSMsbPoint::~CSMsbPoint` -- `0x140cf9500`. The ctor takes a reference on the cap, so the
/// dtor is not optional bookkeeping.
pub const CS_MSB_POINT_DTOR_RVA: usize = 0xcf_9500;
/// `CS::CSMsbPoint::ComputePosition(CSMsbPoint*, FloatVector4* out)` -- `0x140cfaff0`.
pub const CS_MSB_POINT_COMPUTE_POSITION_RVA: usize = 0xcf_aff0;
/// `CS::CSMsbPoint::GetAngle(CSMsbPoint*, FloatVector3* out)` -- `0x140cfae60`. Euler degrees;
/// only `.y` is meaningful for a point, exactly as with an `.aip` record's yaw.
pub const CS_MSB_POINT_GET_ANGLE_RVA: usize = 0xcf_ae60;
/// `CS::CSMsbPoint::HasNoShapeData(CSMsbPoint*)` -- `0x140cfbc30`. A point whose shape data is
/// absent has no position to read; the engine's own consumer skips those and so must we.
pub const CS_MSB_POINT_HAS_NO_SHAPE_DATA_RVA: usize = 0xcf_bc30;
/// `FUN_140669af0(WorldInfoOwner*, vector* out, 0)` -- `0x140669af0`. Appends the `BlockId` of
/// every resident world block.
pub const WORLD_INFO_OWNER_RESIDENT_BLOCKS_RVA: usize = 0x66_9af0;
/// `FUN_140669ea0(WorldInfoOwner*, BlockId*)` -- `0x140669ea0`. Resolves a block to its
/// `MsbResCap`.
pub const WORLD_INFO_OWNER_GET_MSB_RES_CAP_RVA: usize = 0x66_9ea0;
/// `FieldArea+0x18` -- `worldInfoOwner2`, the owner both calls above take.
pub const FIELD_AREA_WORLD_INFO_OWNER2_OFFSET: usize = 0x18;
/// `CSMsbPoint+0x18` -- `shapeData`.
pub const CS_MSB_POINT_SHAPE_DATA_OFFSET: usize = 0x18;
/// Size of a stack-allocated `CS::CSMsbPoint`.
pub const CS_MSB_POINT_SIZE: usize = 0x58;

/// One invasion spawn point read out of a map's MSB.
///
/// Deliberately the same shape as an `.aip` record -- block, block-local position, yaw -- so the
/// map surface and the warp can consume both without caring which table a target came from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MsbInvasionPoint {
    /// The map the point belongs to.
    pub block: BlockKey,
    /// Index within that map's `InvasionPoint` section. Together with `block` this is the point's
    /// identity: MSB region *names* are not usable as keys (629 of the 2807 harvested points
    /// carry a duplicate or generic name).
    pub index: u32,
    /// Map-local position, untouched. Converting here would double-apply the block origin, the
    /// same trap the `.aip` path documents.
    pub position: [f32; 3],
    /// Euler yaw in degrees.
    pub yaw: f32,
}

impl MsbInvasionPoint {
    /// Identity key: the map plus the index within it.
    #[must_use]
    pub const fn key(&self) -> (u32, u32) {
        (self.block.raw(), self.index)
    }
}

/// Points accumulated from every map that has been resident so far this session.
///
/// Sorted and deduped by [`MsbInvasionPoint::key`], so repeated visits to the same map are free
/// and the iteration order is stable -- which matters because the map surface derives a pin's
/// synthetic entity id from its position in this list.
#[derive(Clone, Debug, Default)]
pub struct MsbInvasionCatalog {
    points: Vec<MsbInvasionPoint>,
    /// Maps observed at least once, even if they contained no points. Distinguishing "this map has
    /// no invasion points" from "this map has never been looked at" is the difference between a
    /// complete answer and an unknown one.
    observed_blocks: Vec<u32>,
}

impl MsbInvasionCatalog {
    /// An empty catalog: nothing observed, which is NOT the same as "nothing exists".
    #[must_use]
    pub const fn new() -> Self {
        Self {
            points: Vec::new(),
            observed_blocks: Vec::new(),
        }
    }

    /// Fold in everything read from one map.
    ///
    /// `block` is recorded as observed even when `points` is empty, so a map with genuinely no
    /// invasion points stops being re-reported as unknown.
    pub fn absorb(&mut self, block: BlockKey, points: impl IntoIterator<Item = MsbInvasionPoint>) {
        let raw = block.raw();
        if let Err(at) = self.observed_blocks.binary_search(&raw) {
            self.observed_blocks.insert(at, raw);
        }
        for point in points {
            match self.points.binary_search_by_key(&point.key(), |p| p.key()) {
                // Already known. The engine hands out the same geometry every visit, so a
                // re-read is not new information and must not duplicate a pin.
                Ok(_) => {}
                Err(at) => self.points.insert(at, point),
            }
        }
    }

    /// Every point known so far, in stable key order.
    #[must_use]
    pub fn points(&self) -> &[MsbInvasionPoint] {
        &self.points
    }

    /// How many points are known.
    #[must_use]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Whether nothing has been collected yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// How many distinct maps have been read.
    #[must_use]
    pub fn observed_block_count(&self) -> usize {
        self.observed_blocks.len()
    }

    /// Whether this map has already been read, points or not.
    #[must_use]
    pub fn has_observed(&self, block: BlockKey) -> bool {
        self.observed_blocks.binary_search(&block.raw()).is_ok()
    }

    /// One representative point per map that has any.
    ///
    /// A legacy dungeon is a single place on the world map, so 285 catacomb points must not stack
    /// 285 markers on one icon; this matches the `PinGranularity::PerBlock` the `.aip` side uses.
    /// The representative is the map's lowest-index point, which is stable across visits because
    /// [`Self::points`] is key-ordered -- so a pin keeps its identity (and therefore its synthetic
    /// entity id) as coverage grows around it.
    #[must_use]
    pub fn block_representatives(&self) -> Vec<MsbInvasionPoint> {
        let mut out: Vec<MsbInvasionPoint> = Vec::new();
        let mut last: Option<u32> = None;
        for point in &self.points {
            let raw = point.block.raw();
            if last == Some(raw) {
                continue;
            }
            last = Some(raw);
            out.push(*point);
        }
        out
    }

    /// Distinct areas represented, for the boot-time coverage report.
    #[must_use]
    pub fn area_count(&self) -> usize {
        let mut areas: Vec<u8> = self
            .points
            .iter()
            .map(|point| (point.block.raw() >> 24) as u8)
            .collect();
        areas.sort_unstable();
        areas.dedup();
        areas.len()
    }
}

/// `WorldBlockInfo+0x48` -- `msbResCap`.
///
/// The field is private on the upstream binding, so it is reached by offset. Derived by walking
/// that `#[repr(C)]` layout: `vtable 0x0`, `block_id 0x8`, `unkc 0xc`, `world_info_owner 0x10`,
/// `area_info 0x18`, `world_area_info 0x20`, `world_grid_area_info 0x28`, `unk30 0x30`,
/// `block_id_2 0x34`, `world_area_info_index 0x38`, `unk3c 0x3c`, `unk40 0x40`, `unk41[7] 0x41`,
/// -> `msb_res_cap 0x48`.
pub const WORLD_BLOCK_INFO_MSB_RES_CAP_OFFSET: usize = 0x48;

#[cfg(windows)]
mod native {
    use super::{
        CS_MSB_POINT_COMPUTE_POSITION_RVA, CS_MSB_POINT_CTOR_RVA, CS_MSB_POINT_DTOR_RVA,
        CS_MSB_POINT_GET_ANGLE_RVA, CS_MSB_POINT_HAS_NO_SHAPE_DATA_RVA, CS_MSB_POINT_SIZE,
        GET_POINT_DATA_SECTION_ITEM_COUNT_RVA, MSB_POINT_TYPE_INVASION_POINT, MsbInvasionPoint,
        WORLD_BLOCK_INFO_MSB_RES_CAP_OFFSET,
    };
    use crate::invasion_warp::BlockKey;
    use crate::warp::FloatVector4;

    /// A single map's `InvasionPoint` count cannot plausibly exceed this. The largest map in the
    /// offline harvest of all 1347 shipped MSBs holds 111 (`m12_01_00_00`), so this is ~18x
    /// headroom while still bounding a corrupt or mis-typed read to a fixed amount of work on the
    /// game thread. A count past it is not clamped silently -- the map is skipped and reported.
    pub const MAX_POINTS_PER_MAP: i32 = 2048;

    /// The game image is well under this; a vtable pointer further than this from the module base
    /// did not come from the executable and the object is not what we think it is.
    const MAX_IMAGE_SPAN: usize = 0x1000_0000;

    /// Whether `cap` is plausibly a live `MsbResCap` rather than a stale or uninitialised slot.
    ///
    /// `WorldInfo::world_block_info()` is the engine's block LIST, not a list of blocks whose
    /// resources are currently loaded: an entry for a block that is registered but not streamed in
    /// can carry a null or leftover `msbResCap`. Handing such a pointer to
    /// `GetPointDataSectionItemCount` is a wild call through a garbage vtable, on the game thread,
    /// during a map open -- i.e. a crash in the player's session rather than a missing marker.
    ///
    /// The check is deliberately structural rather than a null test: the object's first field is
    /// its vtable, and a real one points into the loaded image.
    ///
    /// # Safety
    /// Any address may be passed; the reads are fault-tolerant.
    /// Exported so the DLL can census which listed blocks are actually usable without duplicating
    /// (and therefore drifting from) the exact test the reader gates on.
    #[must_use]
    pub unsafe fn msb_res_cap_looks_live(base: usize, cap: usize) -> bool {
        if cap == 0 || cap < 0x1_0000 {
            return false;
        }
        let Some(vtable) = (unsafe { er_game_base::mem::safe_read_usize(cap) }) else {
            return false;
        };
        vtable >= base && vtable < base.saturating_add(MAX_IMAGE_SPAN)
    }

    type GetPointCountFn = unsafe extern "system" fn(usize, u32) -> i32;
    // `(out, MsbResCap*, 0, MsbPointType, index)` -- the 5th argument goes on the stack under the
    // Windows x64 ABI, which `extern "system"` handles.
    type MsbPointCtorFn = unsafe extern "system" fn(*mut u8, usize, u64, u32, u32);
    type MsbPointDtorFn = unsafe extern "system" fn(*mut u8);
    type HasNoShapeDataFn = unsafe extern "system" fn(*const u8) -> bool;
    type OutVectorFn = unsafe extern "system" fn(*const u8, *mut FloatVector4) -> *mut FloatVector4;

    /// Read every `InvasionPoint` region out of one map's `MsbResCap`.
    ///
    /// `None` means THE MAP WAS NOT LOADED -- its cap is null or does not carry an in-image vtable,
    /// so nothing was looked at. `Some(points)` means the cap was live and this is the map's real
    /// answer, empty vector included.
    ///
    /// THE DISTINCTION IS THE WHOLE POINT (2026-08-04). This used to return a bare `Vec` and the
    /// caller recorded the block as observed either way, on the reasoning that "read it, found
    /// nothing" is a real answer. It is -- but a dead cap is not that answer, it is "never looked",
    /// and conflating them broke the feature: `resident_blocks` enumerates the world's STATIC block
    /// list, so at boot all 111 entries were marked observed with dead caps, and every one of them
    /// was then skipped forever by `has_observed`. Measured in run 1615 -- the player standing in the
    /// Haligtree (`block=0x0f000000`) with `msb[0 points/111 maps]` and exactly one `map-msb:` line
    /// in the log, from boot. m15 carries 88 invasion points and not one was ever read.
    ///
    /// # Safety
    ///
    /// Game task thread, with `msb_res_cap` a live `MsbResCap*` belonging to a resident block.
    /// Constructs and destroys a `CS::CSMsbPoint` per point using the engine's own ctor/dtor pair,
    /// so the cap's refcount is balanced.
    #[must_use]
    pub unsafe fn read_map_invasion_points(
        base: usize,
        block: BlockKey,
        msb_res_cap: usize,
    ) -> Option<Vec<MsbInvasionPoint>> {
        if !unsafe { msb_res_cap_looks_live(base, msb_res_cap) } {
            // Not loaded. Say so, so the caller leaves this block unobserved and looks again once
            // the player actually goes there.
            return None;
        }
        let get_count: GetPointCountFn =
            unsafe { core::mem::transmute(base + GET_POINT_DATA_SECTION_ITEM_COUNT_RVA) };
        let count = unsafe { get_count(msb_res_cap, MSB_POINT_TYPE_INVASION_POINT) };
        if count <= 0 || count > MAX_POINTS_PER_MAP {
            // The cap IS live, so this is a genuine answer: this map has no invasion points.
            return Some(Vec::new());
        }

        let ctor: MsbPointCtorFn = unsafe { core::mem::transmute(base + CS_MSB_POINT_CTOR_RVA) };
        let dtor: MsbPointDtorFn = unsafe { core::mem::transmute(base + CS_MSB_POINT_DTOR_RVA) };
        let has_no_shape_data: HasNoShapeDataFn =
            unsafe { core::mem::transmute(base + CS_MSB_POINT_HAS_NO_SHAPE_DATA_RVA) };
        let compute_position: OutVectorFn =
            unsafe { core::mem::transmute(base + CS_MSB_POINT_COMPUTE_POSITION_RVA) };
        let get_angle: OutVectorFn =
            unsafe { core::mem::transmute(base + CS_MSB_POINT_GET_ANGLE_RVA) };

        let mut points = Vec::with_capacity(count as usize);
        for index in 0..count {
            // 16-byte aligned so the engine's MOVAPS-using vector writes are legal, and sized from
            // the RE'd struct size rather than guessed.
            #[repr(C, align(16))]
            struct MsbPointStorage([u8; CS_MSB_POINT_SIZE]);
            let mut storage = MsbPointStorage([0u8; CS_MSB_POINT_SIZE]);
            let point = core::ptr::from_mut(&mut storage).cast::<u8>();

            unsafe {
                ctor(
                    point,
                    msb_res_cap,
                    0,
                    MSB_POINT_TYPE_INVASION_POINT,
                    index as u32,
                );
            }
            // A point with no shape data has no position to read. The engine's own consumer skips
            // these; reading through one would be a wild dereference.
            let usable = !unsafe { has_no_shape_data(point.cast_const()) };
            if usable {
                let mut position = FloatVector4::default();
                let mut angle = FloatVector4::default();
                unsafe {
                    compute_position(point.cast_const(), &raw mut position);
                    get_angle(point.cast_const(), &raw mut angle);
                }
                points.push(MsbInvasionPoint {
                    block,
                    index: index as u32,
                    position: [position.x, position.y, position.z],
                    yaw: angle.y,
                });
            }
            unsafe { dtor(point) };
        }
        Some(points)
    }

    /// Every resident block paired with its `MsbResCap`.
    ///
    /// Walks the typed `FieldArea -> WorldInfoOwner -> WorldRes -> WorldInfo` binding rather than
    /// calling `FUN_140669af0`: that native fills a `std::vector` with the GAME's allocator, and
    /// owning the lifetime of an engine-allocated vector from here is a leak-or-crash choice with
    /// no upside. The slice is already exactly the resident set.
    ///
    /// # Safety
    ///
    /// Game task thread with the world up. Returns an empty vector when `FieldArea` is not
    /// resolvable, which is the normal state at the title screen.
    #[must_use]
    pub unsafe fn resident_blocks() -> Vec<(BlockKey, usize)> {
        use fromsoftware_shared::FromStatic;
        let Ok(field_area) = (unsafe { eldenring::cs::FieldArea::instance() }) else {
            return Vec::new();
        };
        let world_info = &field_area.world_info_owner.world_res.world_info;
        world_info
            .world_block_info()
            .iter()
            .map(|info| {
                // `BlockId` is a newtype over `i32`; the catalog keys on the same raw `u32` the
                // `.aip` path uses, so the two sources share one identity space.
                let block = BlockKey::from_raw(i32::from(info.block_id) as u32);
                let cap = unsafe {
                    er_game_base::mem::safe_read_usize(
                        core::ptr::from_ref(info) as usize + WORLD_BLOCK_INFO_MSB_RES_CAP_OFFSET,
                    )
                }
                .unwrap_or(0);
                (block, cap)
            })
            .collect()
    }
}

#[cfg(windows)]
pub use native::{
    MAX_POINTS_PER_MAP, msb_res_cap_looks_live, read_map_invasion_points, resident_blocks,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn block(area: u8, index: u8) -> BlockKey {
        BlockKey::from_parts(area, index, 0, 0)
    }

    fn point(area: u8, block_index: u8, index: u32) -> MsbInvasionPoint {
        MsbInvasionPoint {
            block: block(area, block_index),
            index,
            position: [index as f32, 1.0, 2.0],
            yaw: 90.0,
        }
    }

    #[test]
    fn the_invasion_point_type_is_the_byte_verified_edx_value() {
        // `0x140a0c1f1` loads EDX = 1 for the InvasionPoint section. If this drifts, the reader
        // silently enumerates a DIFFERENT region subtype and every marker is wrong.
        assert_eq!(MSB_POINT_TYPE_INVASION_POINT, 1);
    }

    #[test]
    fn absorbing_one_map_keeps_its_points() {
        let mut catalog = MsbInvasionCatalog::new();
        catalog.absorb(block(11, 0), [point(11, 0, 0), point(11, 0, 1)]);
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog.observed_block_count(), 1);
    }

    #[test]
    fn revisiting_a_map_adds_nothing_and_never_duplicates_a_pin() {
        let mut catalog = MsbInvasionCatalog::new();
        catalog.absorb(block(11, 0), [point(11, 0, 0), point(11, 0, 1)]);
        catalog.absorb(block(11, 0), [point(11, 0, 0), point(11, 0, 1)]);
        assert_eq!(catalog.len(), 2);
    }

    #[test]
    fn coverage_only_ever_grows_as_maps_are_visited() {
        let mut catalog = MsbInvasionCatalog::new();
        catalog.absorb(block(10, 0), [point(10, 0, 0)]);
        assert_eq!(catalog.len(), 1);
        catalog.absorb(block(11, 0), [point(11, 0, 0), point(11, 0, 1)]);
        assert_eq!(catalog.len(), 3);
        // Re-reading the first map must not evict the second.
        catalog.absorb(block(10, 0), [point(10, 0, 0)]);
        assert_eq!(catalog.len(), 3);
        assert_eq!(catalog.observed_block_count(), 2);
    }

    #[test]
    fn a_map_with_no_points_is_still_recorded_as_read() {
        // Otherwise "this dungeon has no invasion points" is indistinguishable from "we have not
        // looked at this dungeon yet", and the surface can never report honest coverage.
        let mut catalog = MsbInvasionCatalog::new();
        assert!(!catalog.has_observed(block(25, 0)));
        catalog.absorb(block(25, 0), []);
        assert!(catalog.has_observed(block(25, 0)));
        assert!(catalog.is_empty());
        assert_eq!(catalog.observed_block_count(), 1);
    }

    #[test]
    fn points_iterate_in_a_stable_order_regardless_of_visit_order() {
        // The map surface derives a pin's synthetic entity id from its index in this list, so an
        // order that depended on which dungeon the player wandered into first would repoint every
        // id from one map open to the next.
        let mut forwards = MsbInvasionCatalog::new();
        forwards.absorb(block(10, 0), [point(10, 0, 1), point(10, 0, 0)]);
        forwards.absorb(block(11, 0), [point(11, 0, 0)]);

        let mut backwards = MsbInvasionCatalog::new();
        backwards.absorb(block(11, 0), [point(11, 0, 0)]);
        backwards.absorb(block(10, 0), [point(10, 0, 0), point(10, 0, 1)]);

        assert_eq!(forwards.points(), backwards.points());
    }

    #[test]
    fn the_same_index_in_two_different_maps_is_two_points() {
        let mut catalog = MsbInvasionCatalog::new();
        catalog.absorb(block(11, 0), [point(11, 0, 0)]);
        catalog.absorb(block(13, 0), [point(13, 0, 0)]);
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog.area_count(), 2);
    }

    #[test]
    fn one_representative_per_map_not_one_per_point() {
        // 285 catacomb points must not become 285 markers stacked on one dungeon icon.
        let mut catalog = MsbInvasionCatalog::new();
        catalog.absorb(
            block(30, 0),
            (0..12).map(|i| point(30, 0, i)).collect::<Vec<_>>(),
        );
        catalog.absorb(block(31, 0), [point(31, 0, 0), point(31, 0, 1)]);
        assert_eq!(catalog.len(), 14);
        let reps = catalog.block_representatives();
        assert_eq!(reps.len(), 2);
        assert_eq!(reps[0].block, block(30, 0));
        assert_eq!(reps[1].block, block(31, 0));
    }

    #[test]
    fn a_maps_representative_does_not_change_as_coverage_grows() {
        // The surface derives a pin's synthetic entity id from its position in the target list, so
        // a representative that moved when an unrelated dungeon was visited would silently
        // repoint an existing marker at a different destination.
        let mut catalog = MsbInvasionCatalog::new();
        catalog.absorb(block(11, 0), [point(11, 0, 3), point(11, 0, 7)]);
        let before = catalog.block_representatives();
        catalog.absorb(block(10, 0), [point(10, 0, 0)]);
        catalog.absorb(block(13, 0), [point(13, 0, 0)]);
        let after = catalog.block_representatives();
        let leyndell_before = before.iter().find(|p| p.block == block(11, 0)).copied();
        let leyndell_after = after.iter().find(|p| p.block == block(11, 0)).copied();
        assert_eq!(leyndell_before, leyndell_after);
        assert_eq!(leyndell_after.expect("present").index, 3);
    }

    #[test]
    fn a_map_read_with_no_points_contributes_no_representative() {
        let mut catalog = MsbInvasionCatalog::new();
        catalog.absorb(block(25, 0), []);
        assert!(catalog.has_observed(block(25, 0)));
        assert!(catalog.block_representatives().is_empty());
    }

    #[test]
    fn the_area_count_folds_blocks_of_the_same_area_together() {
        let mut catalog = MsbInvasionCatalog::new();
        catalog.absorb(block(12, 1), [point(12, 1, 0)]);
        catalog.absorb(block(12, 2), [point(12, 2, 0)]);
        assert_eq!(catalog.observed_block_count(), 2);
        assert_eq!(catalog.area_count(), 1);
    }
}
