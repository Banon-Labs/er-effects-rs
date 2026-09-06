//! The session-scoped map DATA this crate accumulates: MSB invasion points, and the place
//! names the pins were given.
//!
//! Split out of `map_hooks` on 2026-08-30, when that file stood 29 lines under the 3200-line
//! FAIL threshold in `scripts/check-rust-file-sizes.py` with two writers still appending to it.
//! The seam is not the line count -- it is that NOTHING here is a detour. `map_hooks` owns the
//! three MinHook sites and what runs inside them; this owns two session-lifetime caches that the
//! injection reads and the local-invasion filter reads long after the map row list is gone:
//!
//! * [`MSB_CATALOG`] -- `InvasionPoint` regions harvested from the MSBs of maps that have been
//!   resident this session. It is the ONLY source of pins for a legacy dungeon, cave, catacomb or
//!   tunnel, because the `.aip` table has no entries outside areas 60/61.
//! * [`PLACE_NAMES_BY_BLOCK`] -- block -> `PlaceName` text ids, recorded as pins are named, so
//!   "somewhere in the Haligtree" is still answerable when a match arrives.
//!
//! Both are grown and never reset, and both are read from the game task thread and from the
//! filter, hence the mutexes. The harvest itself (`refresh_msb_catalog`,
//! `harvest_resident_msb_points`) is `cfg(windows)` because it reads live process memory; the
//! accessors are not, so the host tests can still reach them.

use super::*;

/// Invasion points harvested from the MSBs of maps that have been resident this session.
///
/// Session-scoped and only ever grown. It is NOT reset when the catalog signature changes: a mod
/// rewriting the `.aip` table says nothing about MSB region data, and throwing away coverage the
/// player has already walked past would make the surface worse for no reason.
static MSB_CATALOG: std::sync::Mutex<
    er_invasion_warp_core::msb_invasion_points::MsbInvasionCatalog,
> = std::sync::Mutex::new(er_invasion_warp_core::msb_invasion_points::MsbInvasionCatalog::new());

/// Read every resident map's `InvasionPoint` regions into [`MSB_CATALOG`].
///
/// Returns `(points known, maps read)` after the fold. Skips maps already read: the geometry is
/// static per map, so re-reading one is pure cost on the game thread during a map open.
///
/// # Safety
/// Game task thread, with the world up.
#[cfg(windows)]
pub(crate) unsafe fn refresh_msb_catalog() -> (usize, usize) {
    use er_invasion_warp_core::msb_invasion_points::{read_map_invasion_points, resident_blocks};
    let Ok(base) = er_game_base::mem::game_module_base() else {
        return (0, 0);
    };
    let mut catalog = match MSB_CATALOG.lock() {
        Ok(catalog) => catalog,
        Err(poisoned) => poisoned.into_inner(),
    };
    for (block, cap) in unsafe { resident_blocks() } {
        if catalog.has_observed(block) {
            continue;
        }
        // `None` = the map is not loaded, so nothing was looked at. Leaving it UNOBSERVED is what
        // makes the harvest accumulate: `resident_blocks` walks the world's static block list, so
        // most entries are dead caps on any given frame, and absorbing them would mark every map in
        // the game as read during the boot pass and skip them forever afterwards. That is exactly
        // what happened before this check existed -- the player reached the Haligtree and its 88
        // invasion points were never read, because m15 had been "observed" at boot with a null cap.
        if let Some(read) = unsafe { read_map_invasion_points(base, block, cap) } {
            // SAY WHAT WAS LOST. The engine reports a region count; only the regions that carry shape
            // data yield a position. Absorbing the difference in silence is what made "not all of a
            // dungeon's icons" indistinguishable from "that dungeon only has that many spawns" -- the
            // catalog recorded 40 points and nothing anywhere recorded that 88 were on offer.
            //
            // Emitted per map and only when the two numbers disagree, so a clean read is silent.
            let dropped = read.dropped();
            if dropped > 0 {
                crate::standalone_log(format_args!(
                    "map-msb: block {:#010x} reported {} InvasionPoint region(s) but only {} carried \
                     shape data -- {dropped} produced NO pin. The map will show fewer markers than \
                     the map actually has spawns, and this is the only place that difference is \
                     visible.",
                    block.raw(),
                    read.reported,
                    read.points.len()
                ));
            }
            catalog.absorb(block, read.points);
        }
    }
    (catalog.len(), catalog.observed_block_count())
}

/// How many blocks the world lists that the catalog has NOT read yet.
///
/// Reported alongside coverage because the two together are the whole diagnosis: `read` climbing
/// while `pending` falls is the harvest working; `pending` frozen at the full block count means
/// every cap is dead, which is what a boot-time-only pass looks like.
/// Whether the harvest has actually READ this block's MSB this session.
///
/// Distinguishes "we have not looked inside this dungeon yet" from "we looked and it has no
/// invasion points at all". Only the first deserves a provisional marker; the second would be a
/// marker promising an invasion spawn that does not exist.
#[cfg(windows)]
pub(super) fn msb_has_observed(block: er_invasion_warp_core::invasion_warp::BlockKey) -> bool {
    let catalog = match MSB_CATALOG.lock() {
        Ok(catalog) => catalog,
        Err(poisoned) => poisoned.into_inner(),
    };
    catalog.has_observed(block)
}

#[cfg(not(windows))]
pub(super) fn msb_has_observed(_block: er_invasion_warp_core::invasion_warp::BlockKey) -> bool {
    false
}

#[cfg(windows)]
fn msb_pending_block_count() -> usize {
    use er_invasion_warp_core::msb_invasion_points::resident_blocks;
    let catalog = match MSB_CATALOG.lock() {
        Ok(catalog) => catalog,
        Err(poisoned) => poisoned.into_inner(),
    };
    unsafe { resident_blocks() }
        .into_iter()
        .filter(|(block, _)| !catalog.has_observed(*block))
        .count()
}

/// Resident blocks that have answered "no invasion points" at least once but are not believed yet.
///
/// Without this, a block mid-confirmation and a block never looked at are both just "pending", and
/// the distinction is the whole point of requiring repeated empty reads: one says the map answered
/// and we are waiting to be sure, the other says its cap has never been live. A block stuck here
/// across many seconds while the player stands in it means the map genuinely has no invasion
/// points; a block that leaves it by gaining points was a mistimed read caught in the act.
#[cfg(windows)]
fn msb_confirming_block_count() -> usize {
    use er_invasion_warp_core::msb_invasion_points::resident_blocks;
    let catalog = match MSB_CATALOG.lock() {
        Ok(catalog) => catalog,
        Err(poisoned) => poisoned.into_inner(),
    };
    unsafe { resident_blocks() }
        .into_iter()
        .filter(|(block, _)| catalog.pending_empty_reads(*block) > 0)
        .count()
}

#[cfg(not(windows))]
pub(crate) unsafe fn refresh_msb_catalog() -> (usize, usize) {
    (0, 0)
}

/// How many frames between resident-map harvests.
///
/// The harvest itself is nearly free once a map has been read (`has_observed` short-circuits), but
/// the walk that finds the resident maps is a native call plus a list iteration, and running it on
/// every single frame buys nothing: the resident set only changes when the player crosses a load
/// boundary. A one-second stride bounds the cost while keeping the latency between "the player walks
/// into a catacomb" and "that catacomb can contribute a marker" far below the time it takes to open
/// the map. This is a cost/latency choice, not a guess at an unknown -- correctness does not depend
/// on the value.
#[cfg(windows)]
const MSB_HARVEST_FRAME_STRIDE: u64 = 60;

/// Fold whatever maps are resident right now into the session catalog.
///
/// WHY THIS RUNS PER FRAME AND NOT FROM THE MAP HOOK (2026-08-04). The harvest used to be called only
/// from [`inject_pins`], which runs from the `WorldMapViewModel` constructor -- and that constructor
/// has exactly one call site in the image, reached only from `STEP_MoveMap_Init`. So it fires once
/// per WORLD ENTRY, during the loading screen, before `MoveMapStep` has ticked and before the
/// destination's `MsbResCap`s exist. It does NOT fire when the player opens the map. That made the
/// legacy-dungeon source able to see only whatever happened to be resident at world-entry init --
/// never the catacomb the player is standing in. Harvesting from the recurring task instead means a
/// map contributes from the moment the player has actually been in it.
///
/// # Safety
/// Game task thread with the world up; the harvest itself is fault-closed.
///
/// Say which blocks the world list actually offers a usable `MsbResCap` for, and which one the
/// player is standing in.
///
/// THIS IS THE MEASUREMENT, not decoration. The feature rests on one unverified assumption: that
/// while the player is inside a legacy dungeon, that dungeon's block appears in
/// `world_block_info()` with a live cap at `+0x48`. Run 1615 falsified the old code but could not
/// distinguish WHY -- the player was in the Haligtree (`block=0x0f000000`, m15, 88 invasion points
/// on disk) and coverage read `0 points/111 maps`, which is equally consistent with "m15 is absent
/// from the list", "m15 is listed but its cap is null", and "the cap is there but the liveness test
/// rejects it". Those need three different fixes, so the next run must name which one it is.
///
/// Bounded: emits only when the non-null-cap population CHANGES, so travelling logs a handful of
/// lines rather than one per second.
/// The area byte of a packed `BlockId`: `area.block.region.index`, one byte each, area highest.
const BLOCK_ID_AREA_SHIFT: u32 = 24;

/// Areas 50..89 are the open world, matching upstream `BlockId::is_overworld`.
///
/// Duplicated here as a plain integer test rather than reached through `BlockId` because the
/// census already holds the raw `u32` and the only thing at stake is which collection a block
/// would be found in.
fn is_overworld_block(raw: u32) -> bool {
    let area = (raw >> BLOCK_ID_AREA_SHIFT) & 0xff;
    (50..89).contains(&area)
}

#[cfg(windows)]
unsafe fn log_msb_cap_census() {
    use er_invasion_warp_core::msb_invasion_points::resident_blocks;
    let Ok(base) = er_game_base::mem::game_module_base() else {
        return;
    };
    let blocks = unsafe { resident_blocks() };
    let total = blocks.len();
    let non_null = blocks.iter().filter(|(_, cap)| *cap != 0).count();
    let live = blocks
        .iter()
        .filter(|(_, cap)| unsafe {
            er_invasion_warp_core::msb_invasion_points::msb_res_cap_looks_live(base, *cap)
        })
        .count();

    // The block the player is actually in, and whether the list can see it. `None` means the world
    // does not list it at all, which would make retrying pointless and send the fix elsewhere.
    let player_block = unsafe { current_player_block() };

    // The dedup signature MUST include the player's block. Keying it on the population counts alone
    // meant one block going live while another died -- equal totals -- printed nothing, so walking
    // into the Haligtree could be silent, which is the one event this census exists to capture.
    static LAST: AtomicUsize = AtomicUsize::new(usize::MAX);
    let signature =
        (total << 44) | (non_null << 34) | (live << 24) | (player_block.unwrap_or(0) as usize >> 8);
    if LAST.swap(signature, Ordering::SeqCst) == signature {
        return;
    }
    let player_entry = player_block.and_then(|raw| {
        blocks
            .iter()
            .find(|(block, _)| block.raw() == raw)
            .map(|(_, cap)| *cap)
    });
    let player_desc = match (player_block, player_entry) {
        (None, _) => "player block UNKNOWN".to_owned(),
        // AN OVERWORLD BLOCK IS NOT SUPPOSED TO BE IN THIS LIST, AND SAYING IT IS MISSING WAS A
        // FALSE ALARM THAT COST TWO INVESTIGATIONS. `world_block_info()` holds the NON-overworld
        // blocks only; upstream's own `WorldInfo::world_block_info_by_map` proves it by branching
        // on `BlockId::is_overworld()` and searching `world_grid_area_info()` instead for anything
        // in areas 50..89. So every run where the player stood in the open world printed
        // "player block 0x3c212800 is NOT IN the world block list at all" -- correctly and
        // meaninglessly -- and bd `warp-hardlock-main-thread-parked-in-me3-mod-host-2026-09-02`
        // built a hard-lock hypothesis on top of it ("we warp the player into a block the world
        // block list does not contain"), which was never a symptom of anything.
        //
        // The offset is NOT the problem: `WORLD_BLOCK_INFO_MSB_RES_CAP_OFFSET = 0x48` is witnessed
        // for 1.17 by `scripts/check-object-field-offsets-1170.py` (WorldBlockInfo constructor,
        // 55/55 aligned, zero moved offsets), and Ghidra's 1.16.2 type names that member
        // `msbResCap` at 0x48.
        (Some(raw), None) if is_overworld_block(raw) => format!(
            "player block {raw:#010x} is an OVERWORLD tile (area {}), which this list does not \
             carry by design -- overworld blocks live in world_grid_area_info(). Not a miss.",
            (raw >> BLOCK_ID_AREA_SHIFT) & 0xff
        ),
        (Some(raw), None) => {
            format!("player block {raw:#010x} is NOT IN the world block list at all")
        }
        (Some(raw), Some(cap)) => {
            let live = unsafe {
                er_invasion_warp_core::msb_invasion_points::msb_res_cap_looks_live(base, cap)
            };
            format!("player block {raw:#010x} listed with cap {cap:#x} live={live}")
        }
    };
    crate::standalone_log(format_args!(
        "map-msb-census: {total} blocks listed, {non_null} with a non-null cap, {live} passing the \
         vtable-in-image liveness test -- {player_desc}"
    ));
}

#[cfg(not(windows))]
unsafe fn log_msb_cap_census() {}

/// The block id the player is currently in, read the same way the warp path reads it.
#[cfg(windows)]
unsafe fn current_player_block() -> Option<u32> {
    let base = er_game_base::mem::game_module_base().ok()?;
    unsafe { er_invasion_warp_core::warp::current_block_id(base) }
}

#[cfg(not(windows))]
unsafe fn current_player_block() -> Option<u32> {
    None
}

#[cfg(windows)]
pub(crate) unsafe fn harvest_resident_msb_points(frame: u64) {
    if !frame.is_multiple_of(MSB_HARVEST_FRAME_STRIDE) {
        return;
    }
    unsafe { log_msb_cap_census() };
    let before = msb_coverage();
    let after = unsafe { refresh_msb_catalog() };
    if after.1 != before.1 {
        crate::standalone_log(format_args!(
            "map-msb: read {} newly resident map(s) -- MSB InvasionPoint coverage is now {} points \
             across {} maps, {} block(s) still unread (a block stays unread until the player is \
             actually in it, so this falls as you travel; the .aip table has no entries outside \
             areas 60/61, making this the ONLY source for a legacy dungeon, cave or catacomb), {} \
             of them mid-confirmation (answered zero at least once; a zero is not believed until it \
             repeats, because a constructed-but-unparsed MsbResCap also answers zero and latching \
             that used to cost the map both its pins and its standby marker for the session)",
            after.1 - before.1,
            after.0,
            after.1,
            msb_pending_block_count(),
            msb_confirming_block_count()
        ));
    }
}

/// One pin per map that has MSB invasion points, using that map's first point.
///
/// Per-map rather than per-point deliberately: a legacy dungeon is a single place on the world
/// map, and 285 catacomb points would stack 285 markers on one icon. This matches the
/// `PinGranularity::PerBlock` the `.aip` side already uses.
pub(crate) fn msb_block_targets() -> Vec<er_invasion_warp_core::invasion_warp::InvasionWarpTarget> {
    let catalog = match MSB_CATALOG.lock() {
        Ok(catalog) => catalog,
        Err(poisoned) => poisoned.into_inner(),
    };
    // GRANULARITY IS PER-BLOCK FOR THE OVERWORLD AND PER-POINT FOR A LEGACY DUNGEON, because a
    // "block" means two completely different sizes of place.
    //
    // An overworld block is one map tile, so one marker per tile is already fine resolution --
    // and collapsing is what keeps the `.aip` table's 7073 points down to 365 readable markers.
    //
    // A legacy dungeon's block is the WHOLE dungeon. m15 is the entire Haligtree with 88
    // invasion points; m11 is all of Leyndell with 168. One representative for that is a marker
    // saying "somewhere in this castle", which throws away everything that makes the feature
    // worth having there. The user's report was exactly this: warped to the Haligtree and found
    // a single marker where there should have been dozens.
    //
    // Legacy points only ever exist for maps the player has actually been in, so this grows with
    // where they have been rather than all at once.
    let mut targets: Vec<_> = catalog
        .block_representatives()
        .into_iter()
        .filter(|point| !block_area_is_legacy(point.block.raw()))
        .map(|point| {
            er_invasion_warp_core::invasion_warp::InvasionWarpTarget::new(
                point.block,
                point.index,
                point.position,
                point.yaw,
            )
        })
        .collect();
    // ONE ROW PER SEPARABLE MARKER, NOT ONE PER POINT. Per-point was the right correction to
    // one-per-dungeon, but it overshot: the map projects 1:1 in metres and throws Y away, and a
    // legacy dungeon is stacked vertically -- so the Haligtree's 88 points draw as ~39 icons and
    // Volcano Manor's 115 draw as ~21 no matter how many rows are injected. The surplus rows do not
    // add markers; they stack invisibly on the ones already there while consuming list rows and
    // Scaleform clip-pool slots, and they make the pin count a claim about resolution the map
    // cannot honour. Merging per BLOCK (a cluster only means anything within one map's space).
    let mut legacy_points: std::collections::BTreeMap<u32, Vec<_>> =
        std::collections::BTreeMap::new();
    for point in catalog
        .points()
        .iter()
        .filter(|point| block_area_is_legacy(point.block.raw()))
    {
        legacy_points
            .entry(point.block.raw())
            .or_default()
            .push(*point);
    }
    let legacy_raw: usize = legacy_points.values().map(Vec::len).sum();
    let mut legacy_merged = 0_usize;
    for points in legacy_points.values() {
        let merged = er_invasion_warp_core::msb_invasion_points::merge_coincident_points(
            points,
            er_invasion_warp_core::msb_invasion_points::MARKER_MERGE_RADIUS_METRES,
        );
        legacy_merged += merged.len();
        targets.extend(merged.into_iter().map(|point| {
            er_invasion_warp_core::invasion_warp::InvasionWarpTarget::new(
                point.block,
                point.index,
                point.position,
                point.yaw,
            )
        }));
    }
    // ONCE PER OUTCOME, NOT ONCE PER FRAME. The live top-up calls this function every frame, so an
    // unconditional line here wrote 35,900 duplicates and 11.8 MB into one session's log -- noise
    // that buries the lines a diagnosis actually needs, and disk I/O on the game task thread.
    // Latched on (raw, merged), which changes exactly when the harvest does.
    let outcome = ((legacy_raw as u64) << 32) | legacy_merged as u64;
    if legacy_raw != legacy_merged && MERGE_REPORTED.swap(outcome, Ordering::SeqCst) != outcome {
        crate::standalone_log(format_args!(
            "map-msb: {legacy_raw} legacy invasion point(s) across {} map(s) -> {legacy_merged} \
             separable marker(s) after merging anything closer than {:.0}m. The map projects 1:1 in \
             metres and discards height, so points nearer than that cannot draw as separate icons \
             -- injecting them anyway would stack rows on the same pixel, not add markers.",
            legacy_points.len(),
            er_invasion_warp_core::msb_invasion_points::MARKER_MERGE_RADIUS_METRES
        ));
    }
    targets
}

/// Whether a block belongs to a legacy dungeon rather than the open world.
///
/// Areas 60 and 61 are the two overworlds and are the only areas the `.aip` table covers;
/// everything else is a legacy dungeon, cave, catacomb or tunnel.
#[must_use]
pub const fn block_area_is_legacy(block_id: u32) -> bool {
    let area = block_area(block_id);
    area != 60 && area != er_invasion_warp_core::param_row::AREA_SHADOW_LANDS
}

/// MSB invasion-point coverage so far: `(points, maps read)`.
///
/// Surfaced on the heartbeat because it is the one number that says whether the legacy-dungeon
/// source is doing anything at all, and waiting for a map open to find out is too late.
#[must_use]
pub fn msb_coverage() -> (usize, usize) {
    let catalog = match MSB_CATALOG.lock() {
        Ok(catalog) => catalog,
        Err(poisoned) => poisoned.into_inner(),
    };
    (catalog.len(), catalog.observed_block_count())
}

/// Block -> `PlaceName` text ids, recorded as the pins are named.
///
/// The registry stores TARGETS, and the resolved name was previously written into the param row
/// and then forgotten. The local-invasion filter judges by AREA NAME, so the name has to outlive
/// injection: this is where it is kept. Recording it here costs one map insert per pin and makes
/// "somewhere in the Haligtree" answerable later, when a match arrives and the map row list is
/// long gone.
///
/// A block can carry SEVERAL names -- that is the whole point of the "five names, five places to
/// look" rule -- so the value is a set, not a single id.
static PLACE_NAMES_BY_BLOCK: std::sync::Mutex<
    Option<std::collections::BTreeMap<u32, std::collections::BTreeSet<i32>>>,
> = std::sync::Mutex::new(None);

/// Record a resolved place name for a block. `-1` (unresolved) is dropped: an unnamed pin
/// contributes no name, and storing the sentinel would make "no name" look like a name.
pub(crate) fn record_place_name(block: u32, place_name_text_id: i32) {
    if place_name_text_id < 0 {
        return;
    }
    let Ok(mut guard) = PLACE_NAMES_BY_BLOCK.lock() else {
        return;
    };
    guard
        .get_or_insert_with(std::collections::BTreeMap::new)
        .entry(block)
        .or_default()
        .insert(place_name_text_id);
}

/// `PlaceName` text ids known for a block. Empty when the map has not been opened this session --
/// which the filter treats as "no names", failing closed in the name-based modes rather than
/// matching everything.
#[must_use]
pub fn registry_place_names_for_block(block: u32) -> Vec<i32> {
    let Ok(guard) = PLACE_NAMES_BY_BLOCK.lock() else {
        return Vec::new();
    };
    guard
        .as_ref()
        .and_then(|map| map.get(&block))
        .map(|names| names.iter().copied().collect())
        .unwrap_or_default()
}

/// How many blocks have at least one recorded `PlaceName`.
///
/// Exists so a diagnostic can tell "the map has never been opened, so NOTHING has a name" apart
/// from "the map has been read and this particular block simply has no named pin". Those two have
/// opposite fixes -- open the map, versus nothing the player can do -- and a message that asserts
/// the first without checking will confidently give useless advice for the second.
#[must_use]
pub fn registry_named_block_count() -> usize {
    let Ok(guard) = PLACE_NAMES_BY_BLOCK.lock() else {
        return 0;
    };
    guard.as_ref().map_or(0, std::collections::BTreeMap::len)
}
