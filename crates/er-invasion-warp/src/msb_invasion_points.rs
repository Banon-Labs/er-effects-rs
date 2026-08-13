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
/// static TOC entry AND a dynamic overflow vector, so a pointer-walk that knows only about the TOC
/// silently undercounts any map that populates the vector.
///
/// The overflow vector's layout, corrected against the disassembly of `FUN_140cf6350` (which
/// computes `RDX = (type << 5) + resCap`, then reads begin at `[RDX+0xa98]` and end at
/// `[RDX+0xaa0]`): the container starts at `MsbResCap+0xa70`, but the per-type ELEMENT base is
/// `+0xa90` with stride `0x20`, and within an element `begin` is at `+0x8` and `end` at `+0x10`.
/// A previous version of this comment said `+0xa70 + type*0x18`, which lands in the wrong type slot
/// -- and this comment exists specifically to stop the next agent hand-walking it.
///
/// IMPORTANT: this count has NO readiness gate. `CS::MsbResCap`'s constructor zeroes the header and
/// the section tables, so a cap that has been constructed but not yet PARSED answers `0` while
/// already carrying an in-image vtable. See [`EMPTY_READS_BEFORE_OBSERVED`].
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
/// `FieldArea+0x10` -- the owned `WorldInfoOwner` pointer used by the previous typed
/// `FieldArea::instance().world_info_owner` path. Both the CI-pinned and local binding layouts pin
/// this field at the same offset.
pub const FIELD_AREA_WORLD_INFO_OWNER_OFFSET: usize = 0x10;
/// `FieldArea+0x18` -- `worldInfoOwner2`, the owner the native lookup calls above take.
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

/// One map's whole answer to "what invasion points do you have?".
///
/// Carries the count the ENGINE reported alongside the points that could actually be read, because
/// those two numbers are not the same and the difference is invisible otherwise. A point whose
/// region has no shape data has no position, so it is skipped -- correctly, the engine's own
/// consumer skips it too -- but skipping it silently means a dungeon with 88 regions can contribute
/// 40 pins and look exactly like a dungeon with 40 regions.
///
/// That indistinguishability is why "legacy dungeons aren't getting all of their icons" could not be
/// diagnosed from a log: nothing recorded what the full set was supposed to be.
#[derive(Clone, Debug, PartialEq)]
pub struct MapPointRead {
    /// What `GetPointDataSectionItemCount` said this map holds.
    pub reported: i32,
    /// The subset that had shape data and yielded a position.
    pub points: Vec<MsbInvasionPoint>,
}

impl MapPointRead {
    /// How many reported regions produced no usable point.
    #[must_use]
    pub fn dropped(&self) -> usize {
        (self.reported.max(0) as usize).saturating_sub(self.points.len())
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
    /// Consecutive empty reads per block, for blocks not yet observed.
    empty_reads: Vec<(u32, u32)>,
}

/// Below this separation, two points of the same map cannot render as two icons.
///
/// The world map's projection is 1:1 IN METRES and discards Y entirely:
/// `ConvertMsbCoordsToMapCoords` (0x140876140) keeps only the converted X and Z, and the converter's
/// scale is the literal `1.0`. `ConvertLegacyDungeonPositionToOverworldPositionForMap`'s tile rebase
/// subtracts `i*256` on x/z which the projection's `+i*256` term cancels exactly, so relative XZ
/// distances survive the whole pipeline unchanged -- which is what lets this clustering run in
/// physics space without needing the ViewModel's converters.
///
/// The pin clip is counter-scaled to a constant SCREEN size, so its footprint in map units is
/// `screenPixels / zoom`, and the maximum zoom in the table is 2.25 stage-px per map unit. A 40px
/// icon therefore covers ~18 metres of map at the tightest zoom the game allows, and the declared
/// 146x146 marker art covers ~65. 20 metres is the conservative end of that range: it keeps every
/// pair a player could conceivably tell apart and merges only pairs that would draw on top of each
/// other at every zoom level.
///
/// This matters most exactly where the feature does: a legacy dungeon is stacked VERTICALLY, and Y
/// is the axis the map throws away. The Haligtree's 88 invasion points occupy 39 separable spots at
/// this radius, Leyndell's 104 occupy 78, Volcano Manor's 115 occupy 21. Injecting one row per point
/// there does not draw more markers -- it draws the same markers several times over, costs rows and
/// clip-pool slots, and makes the pin count a claim about resolution the map cannot honour.
pub const MARKER_MERGE_RADIUS_METRES: f32 = 20.0;

/// Merge points of a single map that would draw on top of each other, keeping one per cluster.
///
/// Single-linkage on XZ: a point joins a cluster when it is within `radius` of ANY member, which is
/// the right rule for "these overlap on screen" (overlap is transitive through a chain of touching
/// icons). The FIRST point of each cluster is the representative, so the result is stable under the
/// catalog's key ordering and a warp still targets a real authored spawn.
///
/// `radius <= 0` returns the input unchanged, so the merge can be disabled without a second path.
#[must_use]
pub fn merge_coincident_points(points: &[MsbInvasionPoint], radius: f32) -> Vec<MsbInvasionPoint> {
    if radius <= 0.0 {
        return points.to_vec();
    }
    let radius_squared = radius * radius;
    // Members of each cluster, so a later point can be tested against every one of them.
    let mut clusters: Vec<Vec<[f32; 3]>> = Vec::new();
    let mut representatives: Vec<MsbInvasionPoint> = Vec::new();
    for point in points {
        let joined = clusters.iter_mut().position(|members| {
            members.iter().any(|member| {
                let dx = member[0] - point.position[0];
                let dz = member[2] - point.position[2];
                dx.mul_add(dx, dz * dz) <= radius_squared
            })
        });
        match joined {
            Some(at) => clusters[at].push(point.position),
            None => {
                clusters.push(vec![point.position]);
                representatives.push(*point);
            }
        }
    }
    representatives
}

/// How many consecutive empty reads a block needs before "no invasion points" is believed.
///
/// ONE ZERO IS NOT AN ANSWER. `CS::MsbResCap`'s constructor zeroes its header and section tables,
/// and `GetPointDataSectionItemCount` is a bare read of `pointDataToc[type].entryCount` with no
/// readiness gate -- so a cap that has been CONSTRUCTED but whose MSB has not been PARSED answers
/// `0` while already carrying an in-image vtable, which is exactly what the liveness test accepts.
/// The harvest samples once a second and also synchronously from the `WorldMapViewModel` ctor,
/// which runs during the loading screen; catching that window used to latch the block as
/// "observed, empty" for the whole session, which both denied it its precise pins and RETRACTED the
/// provisional whole-dungeon marker standing in for them. One badly-timed sample removed a
/// dungeon's icon permanently.
///
/// The engine does not have this problem because it re-queries per invasion request rather than
/// caching. We cache, so we need the confirmation. Genuinely point-free maps simply stay in the
/// retry set for a few extra seconds at the cost of one native call each.
pub const EMPTY_READS_BEFORE_OBSERVED: u32 = 3;

impl MsbInvasionCatalog {
    /// An empty catalog: nothing observed, which is NOT the same as "nothing exists".
    #[must_use]
    pub const fn new() -> Self {
        Self {
            points: Vec::new(),
            observed_blocks: Vec::new(),
            empty_reads: Vec::new(),
        }
    }

    /// Fold in everything read from one map.
    ///
    /// A read that yields points marks the block observed immediately. An EMPTY read does not --
    /// it takes [`EMPTY_READS_BEFORE_OBSERVED`] consecutive empty reads, because a single zero is
    /// indistinguishable from sampling a cap whose MSB has not been parsed yet, and latching that
    /// zero costs the map both its pins and its standby marker for the rest of the session.
    pub fn absorb(&mut self, block: BlockKey, points: impl IntoIterator<Item = MsbInvasionPoint>) {
        let raw = block.raw();
        let mut any = false;
        for point in points {
            any = true;
            match self.points.binary_search_by_key(&point.key(), |p| p.key()) {
                // Already known. The engine hands out the same geometry every visit, so a
                // re-read is not new information and must not duplicate a pin.
                Ok(_) => {}
                Err(at) => self.points.insert(at, point),
            }
        }
        if any {
            // A real answer. Latch it and forget any empty reads that preceded it -- those were
            // exactly the mis-timed samples this guard exists for.
            if let Ok(at) = self.empty_reads.binary_search_by_key(&raw, |(b, _)| *b) {
                self.empty_reads.remove(at);
            }
            if let Err(at) = self.observed_blocks.binary_search(&raw) {
                self.observed_blocks.insert(at, raw);
            }
            return;
        }
        if self.observed_blocks.binary_search(&raw).is_ok() {
            return;
        }
        let strikes = match self.empty_reads.binary_search_by_key(&raw, |(b, _)| *b) {
            Ok(at) => {
                self.empty_reads[at].1 += 1;
                self.empty_reads[at].1
            }
            Err(at) => {
                self.empty_reads.insert(at, (raw, 1));
                1
            }
        };
        if strikes >= EMPTY_READS_BEFORE_OBSERVED
            && let Err(at) = self.observed_blocks.binary_search(&raw)
        {
            self.observed_blocks.insert(at, raw);
        }
    }

    /// How many consecutive empty reads a block has accumulated without being believed yet.
    ///
    /// Exposed so a run can tell "still confirming" apart from "confirmed empty" -- the two look
    /// identical from coverage totals alone.
    #[must_use]
    pub fn pending_empty_reads(&self, block: BlockKey) -> u32 {
        self.empty_reads
            .binary_search_by_key(&block.raw(), |(b, _)| *b)
            .map_or(0, |at| self.empty_reads[at].1)
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
        FIELD_AREA_WORLD_INFO_OWNER_OFFSET, GET_POINT_DATA_SECTION_ITEM_COUNT_RVA,
        MSB_POINT_TYPE_INVASION_POINT, MapPointRead, MsbInvasionPoint,
        WORLD_BLOCK_INFO_MSB_RES_CAP_OFFSET,
    };
    use crate::invasion_warp::BlockKey;
    use crate::warp::FloatVector4;
    use er_game_base::rva::FIELD_AREA_PTR_RVA;

    /// A single map's `InvasionPoint` count cannot plausibly exceed this. The largest map in the
    /// offline harvest of all 1347 shipped MSBs holds 111 (`m12_01_00_00`), so this is ~18x
    /// headroom while still bounding a corrupt or mis-typed read to a fixed amount of work on the
    /// game thread. A count past it is not clamped silently: the map is refused (`None`, i.e. "no
    /// answer yet") rather than reported as empty, so it keeps its standby marker and is retried.
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
    /// `None` means NO ANSWER -- either the map was not loaded (null cap, or no in-image vtable, so
    /// nothing was looked at) or the count came back implausible. `Some(read)` means the cap was
    /// live and this is the map's real answer, zero regions included.
    ///
    /// The caller must treat `None` as "look again later" and must NOT record the block as observed,
    /// because observed-and-empty retracts the provisional whole-dungeon marker.
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
    ) -> Option<MapPointRead> {
        if !unsafe { msb_res_cap_looks_live(base, msb_res_cap) } {
            // Not loaded. Say so, so the caller leaves this block unobserved and looks again once
            // the player actually goes there.
            return None;
        }
        let get_count: GetPointCountFn =
            unsafe { core::mem::transmute(base + GET_POINT_DATA_SECTION_ITEM_COUNT_RVA) };
        let count = unsafe { get_count(msb_res_cap, MSB_POINT_TYPE_INVASION_POINT) };
        if count > MAX_POINTS_PER_MAP {
            // NOT an answer. The doc above this constant says such a map "is skipped and reported",
            // but this branch used to fall in with `count <= 0` and return an empty vector -- which
            // marks the block OBSERVED. Observed-and-empty is the one state that both denies the map
            // its precise pins AND retracts the provisional whole-dungeon marker that was standing in
            // for them, so an implausible count made a dungeon's icon disappear the moment the player
            // walked into it. Refuse instead: stay unobserved, keep the provisional marker, look again.
            return None;
        }
        if count <= 0 {
            // The cap IS live, so this is a genuine answer: this map has no invasion points.
            return Some(MapPointRead {
                reported: 0,
                points: Vec::new(),
            });
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
        Some(MapPointRead {
            reported: count,
            points,
        })
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
        let Ok(base) = er_game_base::mem::game_module_base() else {
            return Vec::new();
        };
        let Some(field_area) =
            (unsafe { er_game_base::mem::safe_read_usize(base + FIELD_AREA_PTR_RVA) })
        else {
            return Vec::new();
        };
        if field_area < 0x1_0000 {
            return Vec::new();
        }
        let Some(world_info_owner) = (unsafe {
            er_game_base::mem::safe_read_usize(field_area + FIELD_AREA_WORLD_INFO_OWNER_OFFSET)
        }) else {
            return Vec::new();
        };
        if world_info_owner < 0x1_0000
            || unsafe { er_game_base::mem::safe_read_usize(world_info_owner) }.is_none()
        {
            return Vec::new();
        }
        let Some(world_info_owner) =
            (unsafe { (world_info_owner as *const eldenring::cs::WorldInfoOwner).as_ref() })
        else {
            return Vec::new();
        };
        let world_info = &world_info_owner.world_res.world_info;
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
    fn points_closer_than_the_icon_footprint_merge_into_one_marker() {
        let at = |index: u32, x: f32, z: f32| MsbInvasionPoint {
            block: block(15, 0),
            index,
            position: [x, 0.0, z],
            yaw: 0.0,
        };
        // Three within the radius of each other, plus one far away.
        let points = [
            at(0, 0.0, 0.0),
            at(1, 5.0, 0.0),
            at(2, 0.0, 5.0),
            at(3, 500.0, 500.0),
        ];
        let merged = merge_coincident_points(&points, 20.0);
        assert_eq!(merged.len(), 2);
        // The representative is the FIRST of its cluster, so the result is stable.
        assert_eq!(merged[0].index, 0);
        assert_eq!(merged[1].index, 3);
    }

    #[test]
    fn height_never_separates_two_markers_because_the_map_discards_it() {
        // The whole reason a legacy dungeon collapses: the projection keeps X and Z only, so two
        // points on different floors of the same tower are the same spot on the map.
        let stacked = |index: u32, y: f32| MsbInvasionPoint {
            block: block(11, 0),
            index,
            position: [10.0, y, 10.0],
            yaw: 0.0,
        };
        let points = [stacked(0, 0.0), stacked(1, 200.0), stacked(2, -150.0)];
        assert_eq!(merge_coincident_points(&points, 20.0).len(), 1);
    }

    #[test]
    fn a_chain_of_touching_points_is_one_cluster_not_several() {
        // Single linkage: overlap is transitive through a chain, so A-B-C all touching pairwise in
        // sequence is one blob on screen even though A and C are 30m apart.
        let at = |index: u32, x: f32| MsbInvasionPoint {
            block: block(13, 0),
            index,
            position: [x, 0.0, 0.0],
            yaw: 0.0,
        };
        let points = [at(0, 0.0), at(1, 15.0), at(2, 30.0)];
        assert_eq!(merge_coincident_points(&points, 20.0).len(), 1);
    }

    #[test]
    fn a_zero_radius_merges_nothing_so_the_behaviour_can_be_turned_off_in_one_place() {
        let at = |index: u32| MsbInvasionPoint {
            block: block(15, 0),
            index,
            position: [0.0, 0.0, 0.0],
            yaw: 0.0,
        };
        let points = [at(0), at(1), at(2)];
        assert_eq!(merge_coincident_points(&points, 0.0).len(), 3);
    }

    #[test]
    fn a_read_that_lost_regions_to_missing_shape_data_says_how_many() {
        let read = MapPointRead {
            reported: 88,
            points: (0..40).map(|index| point(15, 0, index)).collect(),
        };
        assert_eq!(read.dropped(), 48);
    }

    #[test]
    fn a_complete_read_reports_nothing_dropped() {
        let read = MapPointRead {
            reported: 3,
            points: (0..3).map(|index| point(15, 0, index)).collect(),
        };
        assert_eq!(read.dropped(), 0);
    }

    #[test]
    fn dropped_never_underflows_when_more_points_arrive_than_were_reported() {
        // Cannot happen from the engine, but `dropped` is a subtraction on a value read out of the
        // game and an underflow here would print a nonsense number in the middle of a diagnosis.
        let read = MapPointRead {
            reported: -1,
            points: vec![point(15, 0, 0)],
        };
        assert_eq!(read.dropped(), 0);
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
    fn a_map_with_no_points_is_recorded_as_read_only_after_repeated_empty_reads() {
        // "This dungeon has no invasion points" must still become distinguishable from "we have not
        // looked yet" -- but not on the FIRST zero. A cap can be constructed and answer zero before
        // its MSB is parsed, and latching that costs the map its pins and its standby marker for
        // the session.
        let mut catalog = MsbInvasionCatalog::new();
        assert!(!catalog.has_observed(block(25, 0)));
        for strike in 1..EMPTY_READS_BEFORE_OBSERVED {
            catalog.absorb(block(25, 0), []);
            assert!(
                !catalog.has_observed(block(25, 0)),
                "believed an empty read after only {strike} of {EMPTY_READS_BEFORE_OBSERVED}"
            );
            assert_eq!(catalog.pending_empty_reads(block(25, 0)), strike);
        }
        catalog.absorb(block(25, 0), []);
        assert!(catalog.has_observed(block(25, 0)));
        assert!(catalog.is_empty());
        assert_eq!(catalog.observed_block_count(), 1);
    }

    #[test]
    fn a_late_arriving_point_cancels_the_empty_reads_that_preceded_it() {
        // The exact sequence the guard exists for: the harvest samples a map during its load and
        // gets zero, then the MSB finishes parsing and the same map answers properly.
        let mut catalog = MsbInvasionCatalog::new();
        catalog.absorb(block(15, 0), []);
        catalog.absorb(block(15, 0), []);
        assert!(!catalog.has_observed(block(15, 0)));
        catalog.absorb(block(15, 0), [point(15, 0, 0)]);
        assert!(catalog.has_observed(block(15, 0)));
        assert_eq!(catalog.pending_empty_reads(block(15, 0)), 0);
        assert_eq!(catalog.len(), 1);
        // And a later empty read cannot un-observe it or resurrect the strike count.
        catalog.absorb(block(15, 0), []);
        assert!(catalog.has_observed(block(15, 0)));
        assert_eq!(catalog.pending_empty_reads(block(15, 0)), 0);
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
        for _ in 0..EMPTY_READS_BEFORE_OBSERVED {
            catalog.absorb(block(25, 0), []);
        }
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
