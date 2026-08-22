//! The invasion-spawn warp CATALOG: `CSAutoInvadePoint` -> stable, sortable Rust records.
//!
//! # What this is
//!
//! Elden Ring loads `other:/AutoInvadePoint.aipbnd` (+ `_dlc02`) at boot into the
//! `CSAutoInvadePoint` singleton: a `DLMap<BlockId, {count, points}>` of fixed spawn
//! coordinates the engine uses when a play region has `isAutoIntrudePoint` set. It is a
//! plain coordinate table. This module reads it, flattens it into [`InvasionWarpTarget`]
//! records, and provides the block grouping / summary / coordinate math the map UI needs.
//!
//! # What this is NOT
//!
//! It never fakes an invasion and never enters any session path. The engine's own consumer
//! of this table is `CS::CSBreakInPointManager::_GetCurBreakInPointVecFromAutoIntrudePoint`
//! (0x140a0c4f0), which threads through `CSNetMan->quickmatchManager`; that function is
//! deliberately NOT called. Its block-local -> world-space arithmetic was read out of it
//! statically and reimplemented here ([`InvasionWarpTarget::world_position`]), so the
//! multiplayer code is never entered.
//!
//! # Reverse-engineering provenance (1.16.2; dump VA == deobf VA == runtime VA, shift 0)
//!
//! Every address below is byte-checked with
//! `python3 scripts/check-dump-deobf-identity.py <va>`.
//!
//! | address | symbol | what it proves |
//! |---|---|---|
//! | `0x140ae9860` | `CS::InGameStayStep::STEP_InGameStayLoad` | requests both `.aipbnd` files at boot |
//! | `0x1401f0620` | `CS::CSFileImp::LoadAutoInvadePointBnd` | registers the file cap (does NOT parse) |
//! | `0x140201660` | `CS::AutoInvadePointBndFileCap::Process` | walks the BND, one `AddForBlockId` per entry |
//! | `0x140a69550` | `CS::CSAutoInvadePoint::AddForBlockId` | the on-disk record layout (see [`crate::aip`]) |
//! | `0x140a68ea0` | `CS::CSAutoInvadePoint::CSAutoInvadePoint` | the singleton layout (`DLMap` at +0x00, size 0x70) |
//! | `0x140a693e0` | `CSAutoInvadePoint` lookup-by-`BlockId` | returns the `{count, points}` value, NOT the node |
//! | `0x140660d20` | `BlockId::BlockId` | disk byte order + the BCD index packing |
//! | `0x140a0c4f0` | `_GetCurBreakInPointVecFromAutoIntrudePoint` | `world = point.xyz + block_origin.xyz` |
//! | `0x1406338d0` | `WorldGridAreaInfo::GetWorldAreaInfoCoordinates` | the block-origin source |

use std::fmt;

/// The `area` range for which `BlockId::BlockId` (0x140660d20) BCD-packs the index byte
/// instead of storing it raw: `area - 0x32 < 0x27`, i.e. 50..=88. Both overworld areas that
/// carry auto-invade points (60 = Lands Between, 61 = Shadow of the Erdtree) are inside it.
/// `eldenring::cs::BlockId::is_overworld` uses the same range for the same reason.
pub const BCD_INDEX_AREA_RANGE: std::ops::RangeInclusive<u8> = 50..=88;

/// `BlockId(-1)`: "global / not segregated by map".
pub const BLOCK_KEY_NONE_RAW: u32 = 0xFFFF_FFFF;

/// A host-portable mirror of the engine's `BlockId`.
///
/// The engine stores it as a little-endian `i32` whose bytes are
/// `[index, region, block, area]` (low to high), so the packed value reads
/// `0xAABBCCDD` = `m{AA}_{BB}_{CC}_{DD}` in hex-digit order.
///
/// This type exists separately from `eldenring::cs::BlockId` because that binding is
/// Windows-only, and because it does NOT decode the BCD index (see [`BlockKey::index`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct BlockKey(u32);

impl BlockKey {
    /// Wrap a raw runtime `BlockId` value.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// The raw runtime `BlockId` value.
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// `m{AA}` -- the map area (60 = Lands Between, 61 = Shadow of the Erdtree).
    #[must_use]
    pub const fn area(self) -> u8 {
        (self.0 >> 24) as u8
    }

    /// `m{..}_{BB}` -- the block (the smallest loadable map unit).
    #[must_use]
    pub const fn block(self) -> u8 {
        (self.0 >> 16) as u8
    }

    /// `m{..}_{..}_{CC}` -- the region.
    #[must_use]
    pub const fn region(self) -> u8 {
        (self.0 >> 8) as u8
    }

    /// The index byte EXACTLY as it sits in memory, BCD packing included.
    #[must_use]
    pub const fn index_packed(self) -> u8 {
        self.0 as u8
    }

    /// `m{..}_{..}_{..}_{DD}` -- the decimal index.
    ///
    /// For an area inside [`BCD_INDEX_AREA_RANGE`] the engine stores this byte BCD-packed
    /// (`BlockId::BlockId` @ 0x140660d20 computes
    /// `(i + (i/10)*6) & 0xf | ((i/10) % 10) << 4`), so the raw byte is decoded here.
    /// Outside that range the byte is the index verbatim.
    ///
    /// NOTE: `eldenring::cs::BlockId::index()` returns the raw byte and does not decode,
    /// so the two disagree for an overworld block with a two-digit index. Values disagree
    /// only from index 10 upward; every entry in the shipped `.aipbnd` files has index 0.
    #[must_use]
    pub const fn index(self) -> u8 {
        let packed = self.index_packed();
        if self.area() >= *BCD_INDEX_AREA_RANGE.start()
            && self.area() <= *BCD_INDEX_AREA_RANGE.end()
        {
            (packed >> 4) * 10 + (packed & 0x0F)
        } else {
            packed
        }
    }

    /// True when the engine BCD-packs this area's index byte.
    #[must_use]
    pub const fn uses_bcd_index(self) -> bool {
        self.area() >= *BCD_INDEX_AREA_RANGE.start() && self.area() <= *BCD_INDEX_AREA_RANGE.end()
    }

    /// Build a key the way `BlockId::BlockId` (0x140660d20) does, BCD packing included.
    #[must_use]
    pub const fn from_parts(area: u8, block: u8, region: u8, index: u8) -> Self {
        let bcd = area >= *BCD_INDEX_AREA_RANGE.start() && area <= *BCD_INDEX_AREA_RANGE.end();
        // The engine's expression, evaluated in u32 so the intermediate cannot wrap a u8.
        let wide = index as u32;
        let packed = if bcd {
            (((wide + (wide / 10) * 6) & 0x0F) | (((wide / 10) % 10) << 4)) as u8
        } else {
            index
        };
        Self(
            ((area as u32) << 24) | ((block as u32) << 16) | ((region as u32) << 8) | packed as u32,
        )
    }

    /// Decode the four `BlockId` bytes as they sit in an on-disk `.aip` header (offset 0x08).
    ///
    /// PROVEN by `CSAutoInvadePoint::AddForBlockId` (0x140a69550), which passes the four disk
    /// bytes to `BlockId::BlockId` as `(area, block, region, index)` -- i.e. the disk order is
    /// the REVERSE of the in-memory byte order. Reading the disk bytes as a little-endian u32
    /// and using it as a map key misses every entry.
    #[must_use]
    pub const fn from_disk_bytes(bytes: [u8; 4]) -> Self {
        Self::from_parts(bytes[0], bytes[1], bytes[2], bytes[3])
    }

    /// True for the sentinel `BlockId(-1)`.
    #[must_use]
    pub const fn is_none(self) -> bool {
        self.0 == BLOCK_KEY_NONE_RAW
    }
}

impl fmt::Display for BlockKey {
    /// The map name the game's own debug output uses: `m60_34_51_00`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "m{:02}_{:02}_{:02}_{:02}",
            self.area(),
            self.block(),
            self.region(),
            self.index()
        )
    }
}

/// One fixed invasion-spawn location, as a candidate local warp target.
///
/// `position` is BLOCK-LOCAL, exactly as the engine stores it; call
/// [`Self::world_position`] with the owning block's origin to get physics-space coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InvasionWarpTarget {
    /// The block that owns the point.
    pub block: BlockKey,
    /// Position within the owning block's point array.
    pub point_index: u32,
    /// Block-local position `[x, y, z]`.
    pub position: [f32; 3],
    /// Facing, in RADIANS. The shipped table authors this in `(-2pi, 0]`.
    pub yaw: f32,
}

/// Point index meaning "this block has a marker, but no known point inside it yet".
///
/// A legacy dungeon's invasion points live in its MSB and are only readable while that map is
/// RESIDENT, so a dungeon the player has never entered has none. It does not follow that it
/// cannot be offered: `MoveMapStep` resolves a spawn on the DESTINATION side, after the load,
/// whenever the explicit-spawn slot is not armed -- so a warp to such a block needs no
/// coordinate at all. The dungeon becomes reachable, and once the player is standing in it the
/// harvest reads its real points and the marker is replaced by precise ones.
///
/// `u32::MAX` cannot collide with a real index: `read_map_invasion_points` caps a map at
/// `MAX_POINTS_PER_MAP` (2048) points and the `.aip` blocks are far smaller.
pub const PROVISIONAL_POINT_INDEX: u32 = u32::MAX;

impl InvasionWarpTarget {
    #[must_use]
    pub const fn new(block: BlockKey, point_index: u32, position: [f32; 3], yaw: f32) -> Self {
        Self {
            block,
            point_index,
            position,
            yaw,
        }
    }

    /// A marker for a block whose interior is not known yet.
    ///
    /// The position is the block's own origin, which is what the world map's legacy converter
    /// adds its centre to -- so this projects to the dungeon's centre on the map without any
    /// knowledge of what is inside it.
    #[must_use]
    pub const fn provisional(block: BlockKey) -> Self {
        Self {
            block,
            point_index: PROVISIONAL_POINT_INDEX,
            position: [0.0, 0.0, 0.0],
            yaw: 0.0,
        }
    }

    /// Whether this target carries no known point, so the warp must let the engine pick the
    /// destination block's own spawn instead of arming a coordinate.
    #[must_use]
    pub const fn is_provisional(&self) -> bool {
        self.point_index == PROVISIONAL_POINT_INDEX
    }

    /// Block-local position + the owning block's origin = physics-space position.
    ///
    /// PROVEN from `_GetCurBreakInPointVecFromAutoIntrudePoint` (0x140a0c4f0), which for each
    /// point computes `world.{x,y,z} = point.{x,y,z} + blockOrigin.{x,y,z}` after fetching the
    /// origin from `WorldGridAreaInfo::GetWorldAreaInfoCoordinates` (0x1406338d0). The point's
    /// fourth float takes no part in that sum -- it is carried alongside as the facing.
    #[must_use]
    pub fn world_position(&self, block_origin: [f32; 3]) -> [f32; 3] {
        [
            self.position[0] + block_origin[0],
            self.position[1] + block_origin[1],
            self.position[2] + block_origin[2],
        ]
    }

    /// [`Self::yaw`] wrapped into `(-pi, pi]`, the range a compass/pin needs.
    ///
    /// The authored data is negative-only and reaches -6.28, so half of it lies outside
    /// `(-pi, pi]` until it is wrapped.
    #[must_use]
    pub fn heading_radians(&self) -> f32 {
        wrap_radians(self.yaw)
    }

    /// A stable identity for the target, independent of catalog ordering: the block key in
    /// the high 32 bits and the point index in the low 32. This is what the runtime oracle
    /// [`crate::oracles::ORACLE_INVASION_WARP_SELECTED_ID`] reports.
    #[must_use]
    pub const fn stable_id(&self) -> u64 {
        ((self.block.raw() as u64) << 32) | self.point_index as u64
    }
}

/// The most negative yaw the shipped `.aip` catalog contains: the little-endian point bytes
/// `C3 F5 C8 C0`, i.e. `-6.28` authored to two decimals.
///
/// Measured over the real 365-file extraction (7073 points): 9 points carry exactly this value
/// -- `m60_35_44_00` point 14, `m60_35_45_00` point 1, `m60_35_48_00` point 2 among them -- and
/// nothing in the table is more negative. The next distinct value up is `-6.27`.
///
/// This is deliberately NOT `-f32::consts::TAU`. `-TAU` as `f32` is `-6.2831855` (bits
/// `C0C90FDB`), a number the game never ships, and [`wrap_radians`] maps it to exactly `0.0`
/// whereas it maps the real value to `+0.0032`. Swapping in the std constant would keep the
/// tests passing while silently retiring the shipped extreme they exist to exercise.
///
/// Spelled from its bits because to `clippy::approx_constant` the decimal literal `-6.28` is
/// indistinguishable from a truncated `TAU`, and this value is the opposite of a truncation:
/// it is exact authored data.
pub const AUTHORED_YAW_EXTREME: f32 = f32::from_bits(0xC0C8_F5C3);

/// Wrap an angle in radians into `(-pi, pi]`.
#[must_use]
pub fn wrap_radians(radians: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    if !radians.is_finite() {
        return 0.0;
    }
    let mut wrapped = radians % TAU;
    if wrapped > PI {
        wrapped -= TAU;
    } else if wrapped <= -PI {
        wrapped += TAU;
    }
    wrapped
}

/// Counts over a built catalog. Cheap enough to emit as telemetry every time it is rebuilt.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InvasionWarpCatalogSummary {
    /// Distinct blocks represented.
    pub block_count: usize,
    /// Total targets.
    pub target_count: usize,
    /// Distinct map areas represented (60 and/or 61 in the shipped data).
    pub area_count: usize,
}

/// One block's worth of targets, as the map UI groups them.
#[derive(Clone, Debug, PartialEq)]
pub struct InvasionWarpBlockGroup {
    pub block: BlockKey,
    /// Index of this group's first target in the catalog's flat target slice.
    pub first_target: usize,
    /// Number of targets in this group.
    pub target_count: usize,
}

/// Why a catalog could not be built.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InvasionWarpCatalogError {
    /// The `CSAutoInvadePoint` singleton was not resolvable (too early in boot, or absent).
    AutoInvadePointUnavailable(String),
    /// The singleton resolved but held no points at all -- the `.aipbnd` files had not been
    /// processed yet. Distinct from `AutoInvadePointUnavailable` because it is a TIMING
    /// answer ("ask again later"), not a "this build cannot see the singleton" answer.
    CatalogEmpty,
}

impl fmt::Display for InvasionWarpCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AutoInvadePointUnavailable(reason) => {
                write!(f, "CSAutoInvadePoint unavailable: {reason}")
            }
            Self::CatalogEmpty => {
                write!(f, "CSAutoInvadePoint holds no points yet")
            }
        }
    }
}

impl std::error::Error for InvasionWarpCatalogError {}

/// A built, ordered catalog of invasion-spawn warp targets.
///
/// Ordering is `(block, point_index)` ascending, so the UI list order is deterministic and
/// a selection index means the same thing across rebuilds of the same game data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InvasionWarpCatalog {
    targets: Vec<InvasionWarpTarget>,
}

impl InvasionWarpCatalog {
    /// Sort, de-duplicate and take ownership of a raw target list.
    ///
    /// De-duplication is by `(block, point_index)`: the engine's map is keyed by block and
    /// each block owns one array, so a repeated key is a read artefact (a torn or
    /// double-visited node), never real data. Keeping both would put two identical rows in
    /// the UI list and desynchronise every selection index after them.
    #[must_use]
    pub fn from_targets(mut targets: Vec<InvasionWarpTarget>) -> Self {
        targets.sort_by(|left, right| {
            left.block
                .cmp(&right.block)
                .then(left.point_index.cmp(&right.point_index))
        });
        targets.dedup_by(|left, right| {
            left.block == right.block && left.point_index == right.point_index
        });
        Self { targets }
    }

    /// Build from raw targets, refusing an empty result.
    pub fn try_from_targets(
        targets: Vec<InvasionWarpTarget>,
    ) -> Result<Self, InvasionWarpCatalogError> {
        let catalog = Self::from_targets(targets);
        if catalog.is_empty() {
            return Err(InvasionWarpCatalogError::CatalogEmpty);
        }
        Ok(catalog)
    }

    #[must_use]
    pub fn targets(&self) -> &[InvasionWarpTarget] {
        &self.targets
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.targets.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// The target at a UI list index.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&InvasionWarpTarget> {
        self.targets.get(index)
    }

    #[must_use]
    pub fn summary(&self) -> InvasionWarpCatalogSummary {
        let mut blocks: Vec<BlockKey> = self.targets.iter().map(|target| target.block).collect();
        blocks.sort_unstable();
        blocks.dedup();
        let mut areas: Vec<u8> = blocks.iter().map(|block| block.area()).collect();
        areas.sort_unstable();
        areas.dedup();
        InvasionWarpCatalogSummary {
            block_count: blocks.len(),
            target_count: self.targets.len(),
            area_count: areas.len(),
        }
    }

    /// Contiguous per-block runs over [`Self::targets`]. Cheap because the catalog is sorted.
    #[must_use]
    pub fn block_groups(&self) -> Vec<InvasionWarpBlockGroup> {
        let mut groups: Vec<InvasionWarpBlockGroup> = Vec::new();
        for (index, target) in self.targets.iter().enumerate() {
            match groups.last_mut() {
                Some(group) if group.block == target.block => group.target_count += 1,
                _ => groups.push(InvasionWarpBlockGroup {
                    block: target.block,
                    first_target: index,
                    target_count: 1,
                }),
            }
        }
        groups
    }

    /// Only the targets in one block.
    #[must_use]
    pub fn targets_in_block(&self, block: BlockKey) -> &[InvasionWarpTarget] {
        let start = self.targets.partition_point(|target| target.block < block);
        let end = self.targets.partition_point(|target| target.block <= block);
        &self.targets[start..end]
    }
}

/// The one-line summary the host's debug log carries after a catalog build.
///
/// Split out from [`log_catalog_summary`] so the message a run is diagnosed from is provable
/// by a host test rather than only observable in a live log.
#[must_use]
pub fn describe_catalog(catalog: &InvasionWarpCatalog) -> String {
    let summary = catalog.summary();
    let first = catalog
        .targets()
        .first()
        .map(|target| target.block.to_string())
        .unwrap_or_else(|| "none".to_owned());
    let last = catalog
        .targets()
        .last()
        .map(|target| target.block.to_string())
        .unwrap_or_else(|| "none".to_owned());
    // Whether this is the shipped table or one a mod rewrote in memory. The counts above cannot
    // answer it -- a mod that MOVES points without adding or removing any leaves them identical --
    // so fold every position and yaw into the same canonical form the on-disk containers hash to.
    // Reported at catalog-read time (boot) rather than at pin injection, because it describes the
    // DATA, and waiting for a world load to learn it would be waiting for no reason.
    let content: Vec<_> = catalog
        .targets()
        .iter()
        .map(|target| (target.block, target.position, target.yaw))
        .collect();
    let digest = crate::aip::catalog_content_digest(&content);
    let vanilla = crate::aip::AIP_CATALOG_CONTENT_DIGEST_VANILLA;
    let provenance = if digest == vanilla {
        "VANILLA (byte-identical to the shipped containers)"
    } else {
        "MODIFIED IN MEMORY (a mod rewrote the spawn points; on-disk data does not describe them)"
    };
    format!(
        "invasion-warp catalog: targets={} blocks={} areas={} first_block={first} \
         last_block={last} content_digest={digest:#018x} vs vanilla {vanilla:#018x} -> \
         {provenance}",
        summary.target_count, summary.block_count, summary.area_count
    )
}

/// Emit [`describe_catalog`] through the host's log sink.
pub fn log_catalog_summary(catalog: &InvasionWarpCatalog) {
    crate::host::append_autoload_debug(format_args!("{}", describe_catalog(catalog)));
}

// ---------------------------------------------------------------------------------------
// Windows: the read of the live `CSAutoInvadePoint` singleton.
// ---------------------------------------------------------------------------------------

/// Read the engine's loaded `CSAutoInvadePoint` singleton into a catalog.
///
/// FAIL-CLOSED. This runs inside the user's live game, where a crash or a hung game thread is
/// a far worse outcome than a missing oracle, so it does NOT dereference the singleton's
/// red-black tree with the typed binding (which would `slice::from_raw_parts` a torn `count`
/// and walk node pointers on hope). It resolves the singleton ADDRESS and hands it to
/// [`crate::live_read::read_catalog`], which reads every word through a fault-tolerant
/// primitive, refuses implausible pointers/counts, and cannot loop unboundedly. A singleton
/// that is not there yet, or a tree caught mid-load, comes back as an `Err` to retry -- never
/// as a partial catalog and never as a fault.
///
/// # Safety
///
/// Must only be called after the FromSoftware runtime singletons exist and from a context
/// where reading `CSAutoInvadePoint` is stable (a game task, not an arbitrary thread).
/// Read-only: no writes, no session/multiplayer side effects.
#[cfg(windows)]
pub unsafe fn collect_invasion_warp_catalog()
-> Result<InvasionWarpCatalog, InvasionWarpCatalogError> {
    use eldenring::cs::CSAutoInvadePoint;
    use fromsoftware_shared::FromStatic;

    let auto_invade_point = CSAutoInvadePoint::instance_ptr()
        .map_err(|error| InvasionWarpCatalogError::AutoInvadePointUnavailable(error.to_string()))?;
    crate::live_read::read_catalog(&crate::live_read::ProcessMemory, auto_invade_point as u64)
}

/// The catalog the world-map UI should offer, or `None` when the host has the feature off.
///
/// The gate is checked BEFORE the singleton is touched, so a profile that does not want the
/// surface performs no engine reads at all.
///
/// # Safety
///
/// Same contract as [`collect_invasion_warp_catalog`].
#[cfg(windows)]
pub unsafe fn catalog_for_ui() -> Option<Result<InvasionWarpCatalog, InvasionWarpCatalogError>> {
    if !crate::host::invasion_warp_enabled() {
        return None;
    }
    // SAFETY: forwarded from this function's own contract.
    Some(unsafe { collect_invasion_warp_catalog() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(block: u32, point_index: u32) -> InvasionWarpTarget {
        InvasionWarpTarget::new(
            BlockKey::from_raw(block),
            point_index,
            [point_index as f32, 0.0, 0.0],
            0.0,
        )
    }

    // --- BlockKey ----------------------------------------------------------------------

    #[test]
    fn block_key_splits_the_packed_id_the_way_the_engine_does() {
        // eldenring::cs::BlockId's own unit test pins m61_57_39_03 -> 0x3D392703.
        let key = BlockKey::from_raw(0x3D39_2703);
        assert_eq!(key.area(), 61);
        assert_eq!(key.block(), 57);
        assert_eq!(key.region(), 39);
        assert_eq!(key.index(), 3);
        assert_eq!(key.to_string(), "m61_57_39_03");
    }

    #[test]
    fn disk_bytes_are_the_reverse_of_the_in_memory_bytes() {
        // m60_34_51_00, the smallest shipped .aip file: disk header bytes 3c 22 33 00.
        let key = BlockKey::from_disk_bytes([0x3C, 0x22, 0x33, 0x00]);
        assert_eq!(key.raw(), 0x3C22_3300);
        assert_eq!(key.to_string(), "m60_34_51_00");
        // The trap this test exists for: reading the same disk bytes as a little-endian u32
        // gives a completely different key, which would miss every map lookup.
        assert_ne!(key.raw(), u32::from_le_bytes([0x3C, 0x22, 0x33, 0x00]));
    }

    #[test]
    fn overworld_indices_round_trip_through_the_bcd_packing() {
        for index in 0u8..100 {
            let key = BlockKey::from_parts(60, 34, 51, index);
            assert!(key.uses_bcd_index(), "area 60 must use the packed form");
            assert_eq!(key.index(), index, "BCD round-trip failed for {index}");
        }
        // 10 is the first index where packed != raw; that is the whole point of the packing.
        assert_eq!(BlockKey::from_parts(60, 0, 0, 10).index_packed(), 0x10);
        assert_eq!(BlockKey::from_parts(60, 0, 0, 9).index_packed(), 9);
    }

    #[test]
    fn non_overworld_areas_store_the_index_verbatim() {
        // Area 10 is outside 50..=88, so no packing happens in either direction.
        let key = BlockKey::from_parts(10, 0, 0, 10);
        assert!(!key.uses_bcd_index());
        assert_eq!(key.index_packed(), 10);
        assert_eq!(key.index(), 10);
    }

    #[test]
    fn the_none_sentinel_is_recognised() {
        assert!(BlockKey::from_raw(BLOCK_KEY_NONE_RAW).is_none());
        assert!(!BlockKey::from_raw(0x3C22_3300).is_none());
    }

    // --- catalog construction ----------------------------------------------------------

    #[test]
    fn the_catalog_sorts_by_block_then_point_index() {
        let catalog = InvasionWarpCatalog::from_targets(vec![
            target(0x3C22_3300, 2),
            target(0x3C21_2800, 1),
            target(0x3C22_3300, 0),
            target(0x3C21_2800, 0),
        ]);
        let order: Vec<(u32, u32)> = catalog
            .targets()
            .iter()
            .map(|t| (t.block.raw(), t.point_index))
            .collect();
        assert_eq!(
            order,
            vec![
                (0x3C21_2800, 0),
                (0x3C21_2800, 1),
                (0x3C22_3300, 0),
                (0x3C22_3300, 2),
            ]
        );
    }

    #[test]
    fn duplicate_block_and_point_index_pairs_are_dropped() {
        let catalog = InvasionWarpCatalog::from_targets(vec![
            target(0x3C21_2800, 0),
            target(0x3C21_2800, 0),
            target(0x3C21_2800, 1),
        ]);
        assert_eq!(catalog.len(), 2);
    }

    #[test]
    fn the_summary_counts_blocks_targets_and_areas() {
        let catalog = InvasionWarpCatalog::from_targets(vec![
            target(0x3C21_2800, 0),
            target(0x3C21_2800, 1),
            target(0x3C22_3300, 0),
            target(0x3D2C_2900, 0),
        ]);
        assert_eq!(
            catalog.summary(),
            InvasionWarpCatalogSummary {
                block_count: 3,
                target_count: 4,
                // areas 60 (0x3C) and 61 (0x3D)
                area_count: 2,
            }
        );
    }

    #[test]
    fn an_empty_catalog_summarises_to_zeroes_and_is_refused_by_try_from() {
        let catalog = InvasionWarpCatalog::from_targets(Vec::new());
        assert_eq!(catalog.summary(), InvasionWarpCatalogSummary::default());
        assert_eq!(
            InvasionWarpCatalog::try_from_targets(Vec::new()),
            Err(InvasionWarpCatalogError::CatalogEmpty)
        );
    }

    #[test]
    fn block_groups_are_contiguous_runs_that_cover_every_target() {
        let catalog = InvasionWarpCatalog::from_targets(vec![
            target(0x3C21_2800, 0),
            target(0x3C21_2800, 1),
            target(0x3C22_3300, 0),
            target(0x3D2C_2900, 0),
            target(0x3D2C_2900, 1),
            target(0x3D2C_2900, 2),
        ]);
        let groups = catalog.block_groups();
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].target_count, 2);
        assert_eq!(groups[1].target_count, 1);
        assert_eq!(groups[2].target_count, 3);
        // Runs tile the target slice exactly, with no gap and no overlap.
        let mut cursor = 0usize;
        for group in &groups {
            assert_eq!(group.first_target, cursor);
            cursor += group.target_count;
        }
        assert_eq!(cursor, catalog.len());
    }

    #[test]
    fn targets_in_block_slices_the_right_run() {
        let catalog = InvasionWarpCatalog::from_targets(vec![
            target(0x3C21_2800, 0),
            target(0x3C22_3300, 0),
            target(0x3C22_3300, 1),
            target(0x3D2C_2900, 0),
        ]);
        assert_eq!(
            catalog
                .targets_in_block(BlockKey::from_raw(0x3C22_3300))
                .len(),
            2
        );
        // A block with no points yields an empty slice rather than a panic or a neighbour's run.
        assert!(
            catalog
                .targets_in_block(BlockKey::from_raw(0x3C00_0000))
                .is_empty()
        );
    }

    // --- coordinate / yaw math ---------------------------------------------------------

    #[test]
    fn world_position_adds_the_block_origin_componentwise() {
        // m60_34_51_00 point 0, decoded from the shipped file: (103.79, 456.07, -66.27).
        let point = InvasionWarpTarget::new(
            BlockKey::from_raw(0x3C22_3300),
            0,
            [103.79, 456.07, -66.27],
            -1.09,
        );
        let world = point.world_position([1000.0, -20.0, 250.0]);
        assert!((world[0] - 1103.79).abs() < 1e-3);
        assert!((world[1] - 436.07).abs() < 1e-3);
        assert!((world[2] - 183.73).abs() < 1e-3);
    }

    #[test]
    fn world_position_with_a_zero_origin_is_the_block_local_position() {
        let point =
            InvasionWarpTarget::new(BlockKey::from_raw(0x3C22_3300), 0, [1.0, 2.0, 3.0], 0.0);
        assert_eq!(point.world_position([0.0; 3]), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn headings_wrap_the_authored_negative_only_yaw_into_plus_minus_pi() {
        use std::f32::consts::PI;
        // The authored range is (-2pi, 0]; AUTHORED_YAW_EXTREME (-6.28) is the shipped extreme
        // and must come back as a near-zero heading, not as a value outside the range.
        let extreme =
            InvasionWarpTarget::new(BlockKey::default(), 0, [0.0; 3], AUTHORED_YAW_EXTREME);
        assert!(extreme.heading_radians().abs() < 0.01);
        for yaw in [-0.0f32, -1.09, -2.69, -3.2, -4.0, AUTHORED_YAW_EXTREME] {
            let heading =
                InvasionWarpTarget::new(BlockKey::default(), 0, [0.0; 3], yaw).heading_radians();
            assert!(
                heading > -PI && heading <= PI,
                "yaw {yaw} wrapped to {heading}, outside (-pi, pi]"
            );
        }
    }

    #[test]
    fn wrap_radians_is_defensive_about_non_finite_input() {
        assert_eq!(wrap_radians(f32::NAN), 0.0);
        assert_eq!(wrap_radians(f32::INFINITY), 0.0);
        assert_eq!(wrap_radians(f32::NEG_INFINITY), 0.0);
    }

    #[test]
    fn stable_ids_are_unique_per_block_and_point() {
        let a = target(0x3C21_2800, 0).stable_id();
        let b = target(0x3C21_2800, 1).stable_id();
        let c = target(0x3C22_3300, 0).stable_id();
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
        assert_eq!(a, target(0x3C21_2800, 0).stable_id());
    }

    #[test]
    fn the_log_line_carries_the_counts_and_the_block_range() {
        let catalog = InvasionWarpCatalog::from_targets(vec![
            target(0x3C21_2800, 0),
            target(0x3C22_3300, 0),
            target(0x3D2C_2900, 0),
        ]);
        let line = describe_catalog(&catalog);
        assert!(line.contains("targets=3"), "{line}");
        assert!(line.contains("blocks=3"), "{line}");
        assert!(line.contains("areas=2"), "{line}");
        assert!(line.contains("first_block=m60_33_40_00"), "{line}");
        assert!(line.contains("last_block=m61_44_41_00"), "{line}");
    }

    #[test]
    fn the_log_line_says_none_rather_than_panicking_on_an_empty_catalog() {
        let line = describe_catalog(&InvasionWarpCatalog::default());
        assert!(line.contains("targets=0"), "{line}");
        assert!(line.contains("first_block=none"), "{line}");
    }

    #[test]
    fn catalog_error_messages_name_the_cause() {
        let unavailable =
            InvasionWarpCatalogError::AutoInvadePointUnavailable("singleton null".to_owned());
        assert!(unavailable.to_string().contains("singleton null"));
        assert!(
            InvasionWarpCatalogError::CatalogEmpty
                .to_string()
                .contains("no points")
        );
    }
}
