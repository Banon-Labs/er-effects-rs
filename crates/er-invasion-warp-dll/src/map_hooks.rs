//! The world-map detours.
//!
//! # The injection, and why it is gated the way it is
//!
//! The seam is the `CS::WorldMapViewModel` constructor: the pin-row list at `+0x2d8` is populated
//! there and nowhere else. Appending anywhere else is unsafe -- `CS::WorldMapWarpData+0x08` holds
//! RAW pointers into this buffer, and the reserve relocates it, so a later append dangles every
//! live dialog row pointer. At the ctor epilogue no dialog exists yet.
//!
//! **The ViewModel is NOT built once per session.** The RE said it was, and a first run appeared
//! to confirm it (`ctor #1`). A later run measured `ctor #1 this=0x2d41be80` followed by
//! `ctor #2 this=0x8400a580` -- a second, different instance, with its own freshly-built
//! 420-row list, and that second one got nothing. That single log is both reported symptoms at
//! once: the markers vanish when the map is reopened, and views other than the first are bare.
//!
//! So injection runs on EVERY constructor call and remembers nothing between them. The ctor
//! builds a fresh list each time, which makes re-injection the correct behaviour rather than a
//! hazard, and makes bookkeeping the only thing that can be wrong -- as it twice was, first as a
//! process-wide flag and then as a `this`-pointer table that a recycled menu-heap address
//! defeats. What IS shared is the catalog-derived registry and the synthetic param rows, built
//! once: a pin does not own its param row, and rebuilding them per open would both leak and drag
//! a 7073-point catalog walk into the frame where the player opens the map.
//!
//! The observation that shipped first measured the list before anything was written: rows=420,
//! capacity=474, **54 spare**, vftable `0x142ad82a8`, and `356160 / 0x350` dividing exactly. That
//! is why the append reserves rather than assuming room.
//!
//! Every step is fail-closed, because the failure modes here are not soft:
//!
//! * allocation failure inside the reserve is a **hard `DLPanic`**, not a null return, and both
//!   buffers are alive during it -- so the pin set is capped per-block (365, ~310 KB) rather
//!   than per-point (7073, ~5.8 MB);
//! * the reserve happens **once** with the final count; per-row reserves copy-construct every
//!   existing element each time;
//! * rows are built by the engine's own ctor and placed with the engine's own copy-ctor -- never
//!   `memcpy`, which would double-free the row's two owned heap regions at teardown;
//! * the temp row is destroyed with the engine's dtor, never `free`;
//! * `end` is re-read every iteration and written back, exactly as the ctor's own append does.
//!
//! If any check fails the append is skipped and the reason is logged. A map without invasion
//! pins is a disappointment; a corrupted MenuHeap is a crash.
//!
//! # Hooking rules this module obeys
//!
//! * Every detour goes through the `er_hook` UNION, never a bare `MhHook`. Two MinHook instances
//!   patching one prologue corrupt each other's trampolines, and `er_effects_rs.dll` may be
//!   loaded alongside this DLL.
//! * Nothing is patched until [`crate::map_seams::verify_seam`] has re-read the live prologue.
//! * A handler that finds no trampoline does NOT invent a return value -- see
//!   [`worldmap_viewmodel_ctor_hook`].
//!
//! # Open hazard
//!
//! The RE contract says to enter the ctor trampoline by JMP, never CALL: the prologue is
//! `mov rax, rsp` and later frame references derive from it, so a pushed return address shifts
//! the frame. This handler CALLs the trampoline and it worked live (the ctor ran, the list read
//! back coherent, the world loaded) -- most likely because the ctor takes <= 4 register args and
//! builds its own frame with `sub rsp, 0x170`, leaving a shifted-but-self-consistent anchor. One
//! success is not proof. If a fifth stack argument or a caller-frame-relative read is ever found
//! in this ctor, the CALL form breaks and this must become a JMP-entry trampoline.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::map_seams::WORLDMAP_VIEWMODEL_CTOR;
// `verify_seam` reads live process memory, so it only exists on the game target. The import has
// to be gated with it: an ungated `use` of a `cfg(windows)` item fails the HOST build outright,
// which took this crate's unit tests -- the pin-list geometry and span-table checks that need no
// game at all -- out of reach on Linux.
#[cfg(windows)]
use crate::map_seams::verify_seam;

/// Offsets into `CS::WorldMapViewModel` for the pin-row list, from the RE
/// (docs/plans/world-map-invasion-warp.md section 5.3).
pub const PIN_LIST_VFTABLE_OFFSET: usize = 0x2d8;
/// `+0x2e0` -- the list's allocator.
pub const PIN_LIST_ALLOCATOR_OFFSET: usize = 0x2e0;
/// `+0x2e8` -- first row.
pub const PIN_LIST_BEGIN_OFFSET: usize = 0x2e8;
/// `+0x2f0` -- one past the last row.
pub const PIN_LIST_END_OFFSET: usize = 0x2f0;
/// `+0x2f8` -- one past the last ALLOCATED row.
pub const PIN_LIST_CAPACITY_OFFSET: usize = 0x2f8;
/// `CS::WorldMapWarpPinData` stride. `(end - begin)` must divide by this or the layout is wrong.
pub const PIN_ROW_STRIDE: usize = 0x350;

/// Trampoline to the original ViewModel ctor, installed by the union.
static ORIG_WORLDMAP_VIEWMODEL_CTOR: AtomicUsize = AtomicUsize::new(0);

/// How many times the ctor hook has fired. Measured >1 in practice: one ViewModel per map view,
/// not one per session. Kept as the counter that refuted the original lifetime assumption.
static VIEWMODEL_CTOR_HITS: AtomicUsize = AtomicUsize::new(0);

/// Row count observed on the last ctor return, or `usize::MAX` when never read.
static OBSERVED_ROW_COUNT: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Set when `(end - begin)` did not divide by [`PIN_ROW_STRIDE`] -- i.e. the list is not the
/// shape the RE describes and NOTHING should be appended to it.
static ROW_STRIDE_MISMATCH: AtomicUsize = AtomicUsize::new(0);

/// Whether the ctor hook is installed.
static CTOR_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);

/// A read-back of the pin-row list, as observed on the game thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinListGeometry {
    pub vftable: usize,
    pub begin: usize,
    pub end: usize,
    pub capacity: usize,
}

impl PinListGeometry {
    /// Bytes currently occupied by rows.
    #[must_use]
    pub const fn used_bytes(&self) -> usize {
        self.end.saturating_sub(self.begin)
    }

    /// Bytes the allocation spans.
    #[must_use]
    pub const fn capacity_bytes(&self) -> usize {
        self.capacity.saturating_sub(self.begin)
    }

    /// Row count, or `None` when the span does not divide by the stride -- which means the
    /// layout is not what we reversed and no append may be attempted.
    #[must_use]
    pub const fn row_count(&self) -> Option<usize> {
        let used = self.used_bytes();
        if used % PIN_ROW_STRIDE != 0 {
            return None;
        }
        Some(used / PIN_ROW_STRIDE)
    }

    /// Rows that would fit without growing the allocation.
    #[must_use]
    pub const fn spare_rows(&self) -> usize {
        let spare = self.capacity.saturating_sub(self.end);
        spare / PIN_ROW_STRIDE
    }

    /// Cheap sanity: a plausible, ordered, non-null span.
    #[must_use]
    pub const fn is_plausible(&self) -> bool {
        self.begin != 0
            && self.begin <= self.end
            && self.end <= self.capacity
            && self.row_count().is_some()
    }
}

/// Read the pin-row list out of a `CS::WorldMapViewModel`.
///
/// # Safety
///
/// `view_model` must point at a live ViewModel. Every read goes through the fault-tolerant
/// primitive, so a bad pointer yields `None` rather than a fault.
#[cfg(windows)]
#[must_use]
pub unsafe fn read_pin_list(view_model: usize) -> Option<PinListGeometry> {
    if view_model == 0 {
        return None;
    }
    let read = |offset: usize| unsafe { er_game_base::mem::safe_read_usize(view_model + offset) };
    Some(PinListGeometry {
        vftable: read(PIN_LIST_VFTABLE_OFFSET)?,
        begin: read(PIN_LIST_BEGIN_OFFSET)?,
        end: read(PIN_LIST_END_OFFSET)?,
        capacity: read(PIN_LIST_CAPACITY_OFFSET)?,
    })
}

/// `viewModel + 0x2E0` -- the `Vector*` every list helper takes. NOT `+0x2d8`.
pub const PIN_VECTOR_OFFSET: usize = 0x2e0;
/// Within the vector: `begin` at `+0x08`, `end` at `+0x10`, `capacity` at `+0x18`.
pub const VECTOR_END_OFFSET: usize = 0x10;
/// `viewModel + 0xF8` -- `DLFixedVector<WorldMapAreaConverter, 8>`.
pub const AREA_CONVERTERS_OFFSET: usize = 0xf8;
/// Stride of one `WorldMapAreaConverter`.
pub const AREA_CONVERTER_STRIDE: usize = 0x30;
/// `viewModel + 0x280` -- converter count (8).
pub const AREA_CONVERTER_COUNT_OFFSET: usize = 0x280;
/// Row field `+0x240` -- the `BonfireWarpParam*` a row was built from.
pub const ROW_PARAM_POINTER_OFFSET: usize = 0x240;
/// Row field `+0x50` -- the bonfire entity id the ctor copies from param `+0x08`.
pub const ROW_ENTITY_ID_OFFSET: usize = 0x50;
/// Row field `+0x08` -- `CS::WorldMapPinDataBase`'s per-row id.
///
/// Assigned from a global counter by the base constructor, but COPIED by the copy-ctor, which
/// is how every injected row ends up sharing one value unless it is stamped. The marker draw
/// treats it as a change-detection token, not as an identity: see the stamp in [`inject_pins`].
pub const ROW_ID_OFFSET: usize = 0x08;
/// First id stamped into an injected row's `+0x08`.
///
/// Chosen far above the engine's own counter, which starts at 0 and increments per constructed
/// pin (a map holds a few hundred), so an injected row's id can never alias a shipped one. It is
/// deliberately NOT `-1`: the engine treats `-1` as its counter's wrap sentinel.
pub const INJECTED_ROW_ID_BASE: i32 = 0x4000_0000;

/// `WorldMapCoordinates` -- the 8 bytes a pin renders at (`row+0x10`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MapCoordinates {
    pub x: f32,
    pub z: f32,
}

/// `BonfireWarpParamLookupResult` -- `{paramId, pad, BonfireWarpParam*}`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BonfireLookupResult {
    pub param_id: i32,
    pub pad: i32,
    pub param_row: *const u8,
}

/// A 0x350 row buffer, 8-byte aligned as the list allocator guarantees.
#[repr(C, align(8))]
struct TempPinRow([u8; PIN_ROW_STRIDE]);

/// Fields sampled off a REAL row so synthetic pins behave like shipped ones.
///
/// Sampling beats guessing: the subcategory id decides which tab a row lands in, the category
/// bits decide whether it survives the caller's mask, and the label text id is re-resolved from
/// the live param row by vtable `+0x38` -- so a fabricated text id blanks the name LATER even
/// when construction looked right.
#[derive(Clone, Copy, Debug)]
struct DonorParamFields {
    subcategory_id: i32,
    category_bits: u8,
    icon_id: u16,
    label_text_id: i32,
    /// Which shipped row it came from, so the log shows whether row 0 was skipped.
    donor_row_index: usize,
}

/// How far to scan for a usable donor. The shipped list is ~420 rows and a filter-passing grace
/// appears far earlier; an unbounded scan on the game thread is not worth the risk.
const MAX_DONOR_SCAN_ROWS: usize = 128;

/// Half-open address spans covering the rows we appended, one entry per injection.
///
/// A filter callback is "ours" when the row falls inside any recorded span -- an address test,
/// which stays correct even though the reserve relocated the buffer, because a span is recorded
/// AFTER the reserve.
///
/// A TABLE rather than one pair, because more than one ViewModel is alive at a time and each has
/// its own row buffer. With a single pair, the newest injection overwrote the span of every older
/// live view, so the filter observer stopped recognising that view's rows and under-counted
/// `ours` -- turning the visibility oracle into a source of false negatives exactly when there is
/// more than one map view to explain. Oldest entries are overwritten once the table is full,
/// which is harmless: a ViewModel that old has been destroyed and its rows freed.
const MAX_INJECTED_SPANS: usize = 16;
static INJECTED_SPANS: [(AtomicUsize, AtomicUsize); MAX_INJECTED_SPANS] =
    [const { (AtomicUsize::new(0), AtomicUsize::new(0)) }; MAX_INJECTED_SPANS];
/// Next span slot to write, monotonically increasing and taken modulo the table size.
static INJECTED_SPAN_CURSOR: AtomicUsize = AtomicUsize::new(0);

/// Write one span into `spans`, wrapping at the end of the table.
///
/// Split from [`record_injected_span`] so the wrap and containment rules can be tested against a
/// caller-owned table: the global one is shared process-wide, and tests that mutate it would
/// evict each other's entries when the harness runs them in parallel.
fn record_span(
    spans: &[(AtomicUsize, AtomicUsize)],
    cursor: &AtomicUsize,
    begin: usize,
    end: usize,
) {
    let slot = cursor.fetch_add(1, Ordering::SeqCst) % spans.len();
    // End first: a concurrent reader that sees a non-zero begin must already see a valid end,
    // otherwise it would test against an empty span and report a row of ours as not ours.
    spans[slot].1.store(end, Ordering::SeqCst);
    spans[slot].0.store(begin, Ordering::SeqCst);
}

/// Whether `row` falls inside any recorded span.
fn span_contains(spans: &[(AtomicUsize, AtomicUsize)], row: usize) -> bool {
    spans.iter().any(|(begin, end)| {
        let begin = begin.load(Ordering::SeqCst);
        begin != 0 && row >= begin && row < end.load(Ordering::SeqCst)
    })
}

/// Record the rows one injection appended, so the filter observer can recognise them.
fn record_injected_span(begin: usize, end: usize) {
    record_span(&INJECTED_SPANS, &INJECTED_SPAN_CURSOR, begin, end);
}

/// Whether `row` is one of the rows we appended, in any live view.
fn row_is_ours(row: usize) -> bool {
    span_contains(&INJECTED_SPANS, row)
}

/// Filter verdicts for OUR rows: how many were asked about, and how many were accepted.
static FILTER_QUERIES_OURS: AtomicUsize = AtomicUsize::new(0);
static FILTER_PASSES_OURS: AtomicUsize = AtomicUsize::new(0);
/// Same for the shipped rows, as a control: if the shipped rows also fail, the mask being used
/// is simply not one our rows were ever going to match, and the fault is not in our fields.
static FILTER_QUERIES_SHIPPED: AtomicUsize = AtomicUsize::new(0);
static FILTER_PASSES_SHIPPED: AtomicUsize = AtomicUsize::new(0);
/// Log only the first few verdicts; the filter runs once per row per list build.
static FILTER_TRACE_BUDGET: AtomicUsize = AtomicUsize::new(6);

/// Trampoline to the original row filter.
static ORIG_ROW_FILTER: AtomicUsize = AtomicUsize::new(0);

/// Pins appended by the MOST RECENT injection.
static PINS_INJECTED: AtomicUsize = AtomicUsize::new(0);
/// Injections that actually appended at least one pin, for the whole session. Paired with
/// [`VIEWMODEL_CTOR_HITS`] this is the oracle for "every map open got pins": the two must stay
/// equal. A gap is the bug, and it is visible in RAM without looking at the screen.
static INJECTIONS_PERFORMED: AtomicUsize = AtomicUsize::new(0);
/// Ctor calls that reached [`inject_pins`] and appended nothing. Non-zero means a map view was
/// left bare, and the log line above the increment says why.
static INJECTIONS_SKIPPED: AtomicUsize = AtomicUsize::new(0);

/// The leaked param rows and registry, built once and shared by every ViewModel.
///
/// A pin does not own its param row, so one immutable set serves every view. Building them per
/// injection would leak a fresh copy of both on every single map open -- and now that injection
/// runs on EVERY ctor rather than once, that is a per-open leak of ~365 param rows plus a
/// 365-entry registry, which is a slow but real memory bleed for a player who opens the map a
/// hundred times.
static SHARED_PARAM_ROWS_PTR: AtomicUsize = AtomicUsize::new(0);
static SHARED_PARAM_ROWS_LEN: AtomicUsize = AtomicUsize::new(0);
/// Icon frame the cached rows currently carry, so a change can be detected and re-stamped.
/// `usize::MAX` until the first stamp, which no real frame number can collide with.
static SHARED_PARAM_ROWS_ICON: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Signature of the spawn table the cached registry was built from.
static CATALOG_SIGNATURE: AtomicUsize = AtomicUsize::new(0);

/// A cheap fingerprint of a pin set, used to notice that the loaded spawn table changed.
///
/// It folds every target's block AND its position, because a mod can move a spawn without
/// changing how many there are -- counting alone would call an ersc-rewritten table identical to
/// the vanilla one and keep serving stale pins. This is not a cryptographic digest and does not
/// need to be; it needs to change when the data changes.
fn catalog_signature(registry: &er_invasion_warp::map_surface::InvasionRowRegistry) -> usize {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut mix = |value: u64| {
        hash ^= value;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    };
    mix(registry.len() as u64);
    for target in registry.targets() {
        mix(u64::from(target.block.raw()));
        mix(u64::from(target.point_index));
        for axis in target.position {
            mix(u64::from(axis.to_bits()));
        }
    }
    hash as usize
}

/// There is deliberately NO "already injected" bookkeeping.
///
/// Two earlier shapes both failed, and they failed for the same underlying reason:
///
/// * a process-wide flag left every map view after the first with no pins;
/// * keying on the ViewModel's `this` pointer fixed the *observed* case (two live instances at
///   different addresses) but is only as good as the assumption that a later ViewModel never
///   lands where an earlier one was. These objects are allocated out of a menu heap and freed
///   when the map closes, so a reopen reusing the address is not exotic -- it is the normal
///   behaviour of a size-bucketed allocator. Under that dedupe, the reopen is silently skipped
///   and the map is bare, which is precisely the reported symptom.
///
/// Injection is idempotent-by-construction instead: the ctor builds a FRESH row list every time
/// it runs (measured -- `rows=420` on both observed instances, never 785), so "has this list
/// already got our pins" is answerable from the list itself and the answer is always "no" at the
/// ctor epilogue. Nothing needs to be remembered between calls, so nothing can be remembered
/// WRONG.

/// Read the donor fields off the first existing row.
///
/// # Safety
/// Game thread; `begin` must point at a constructed row.
#[cfg(windows)]
unsafe fn sample_donor(begin: usize, row_count: usize) -> Option<DonorParamFields> {
    use er_invasion_warp::param_row::{
        CATEGORY_BITS_MASK, PARAM_CATEGORY_BITS_OFFSET, PARAM_ICON_ID_OFFSET,
        PARAM_LABEL_TEXT_ID_BASE, PARAM_SUBCATEGORY_ID_OFFSET,
    };
    // SCAN -- do NOT just take row 0. Measured live, the first shipped row has
    // `category_bits == 0x0` and `subcategory == 0`, and cloning it produces pins the row
    // filter discards: FUN_14088be50 requires `(row+0x60 & category_mask) != 0`. A donor is
    // only useful if it would itself survive that test.
    for index in 0..row_count.min(MAX_DONOR_SCAN_ROWS) {
        let row = begin + index * PIN_ROW_STRIDE;
        let Some(param) =
            (unsafe { er_game_base::mem::safe_read_usize(row + ROW_PARAM_POINTER_OFFSET) })
        else {
            continue;
        };
        if param == 0 {
            continue;
        }
        let Some(category_bits) =
            (unsafe { er_game_base::mem::safe_read_u8(param + PARAM_CATEGORY_BITS_OFFSET) })
        else {
            continue;
        };
        if category_bits & CATEGORY_BITS_MASK == 0 {
            continue;
        }
        let Some(subcategory_id) =
            (unsafe { er_game_base::mem::safe_read_i32(param + PARAM_SUBCATEGORY_ID_OFFSET) })
        else {
            continue;
        };
        let Some(icon_id) =
            (unsafe { er_game_base::mem::safe_read_u16(param + PARAM_ICON_ID_OFFSET) })
        else {
            continue;
        };
        let Some(label_text_id) =
            (unsafe { er_game_base::mem::safe_read_i32(param + PARAM_LABEL_TEXT_ID_BASE) })
        else {
            continue;
        };
        // A negative text id blanks the name when vtable +0x38 re-resolves it later.
        if label_text_id < 0 {
            continue;
        }
        return Some(DonorParamFields {
            subcategory_id,
            category_bits,
            icon_id,
            label_text_id,
            donor_row_index: index,
        });
    }
    None
}

/// Project a block-local `.aip` point into map space by looping the ViewModel's converters,
/// exactly as the engine does. `None` when no converter owns the area -- a free fail-closed
/// filter, so an unplaceable point never becomes a pin.
///
/// # Safety
/// Game thread; `view_model` live.
#[cfg(windows)]
unsafe fn project_to_map(
    base: usize,
    view_model: usize,
    block_id: u32,
    msb_pos: [f32; 3],
) -> Option<(MapCoordinates, usize, u8)> {
    type ConvertFn =
        unsafe extern "system" fn(usize, *mut MapCoordinates, *const u32, *const [f32; 3]) -> bool;
    let convert: ConvertFn = unsafe {
        core::mem::transmute(base + crate::map_seams::CONVERT_MSB_COORDS_TO_MAP_COORDS.rva)
    };
    let count =
        unsafe { er_game_base::mem::safe_read_usize(view_model + AREA_CONVERTER_COUNT_OFFSET) }?;
    // Bounded: the field is a DLFixedVector<_, 8>, so a larger value is corruption.
    let count = count.min(8);
    let mut out = MapCoordinates::default();
    for index in 0..count {
        let converter = view_model + AREA_CONVERTERS_OFFSET + index * AREA_CONVERTER_STRIDE;
        if unsafe {
            convert(
                converter,
                &raw mut out,
                &raw const block_id,
                &raw const msb_pos,
            )
        } {
            // `refBlock` sits at converter+0x08; its AREA is byte 3 of the packed BlockId.
            // Reporting it distinguishes "an area-61 point matched a DLC converter" from "an
            // area-61 point was accepted by a BASE converter and is now drawn at a meaningless
            // place on the base map" -- the leading hypothesis for the missing DLC pins.
            let converter_area = unsafe { er_game_base::mem::safe_read_u8(converter + 0x0b) };
            return Some((out, index, converter_area.unwrap_or(0)));
        }
    }
    None
}

/// `DAT_142ad82f8` -- the engine's converter-index -> map-layer-id table, `{0, 1, 10}`.
///
/// Read live rather than hard-coded: if a patch ever reorders the converters, a baked table would
/// keep assigning confidently wrong layers, whereas a live read that fails its shape check
/// assigns none.
pub const LAYER_ID_TABLE_RVA: usize = 0x2ad_82f8;
/// Converter slots that have a layer entry. The engine gates every use on `(byte)i < 3`, so a
/// pin projected by slot 3..7 can never be drawn on any map.
pub const LAYERED_CONVERTER_COUNT: usize = 3;

/// The single `row+0x60` bit a pin projected by converter `converter_index` must carry.
///
/// `None` means "this pin cannot be drawn anywhere" -- an unlayered converter slot, or a table
/// that is not the `{0, 1, 10}` the RE describes. Both are refusals, not defaults: giving such a
/// pin a bit anyway is how a marker ends up painted onto a map whose coordinate space it was
/// never projected into.
#[cfg(windows)]
#[must_use]
fn layer_bit_for_converter(base: usize, converter_index: usize, block_area: u8) -> Option<u8> {
    if converter_index >= LAYERED_CONVERTER_COUNT {
        return None;
    }
    let table: [u8; LAYERED_CONVERTER_COUNT] = core::array::from_fn(|i| {
        unsafe { er_game_base::mem::safe_read_u8(base + LAYER_ID_TABLE_RVA + i) }.unwrap_or(0xFF)
    });
    // Fail closed on an unexpected table: the layer ids are what the whole mapping rests on.
    if table != [0, 1, 10] {
        return None;
    }
    let mut layer_id = table[converter_index];
    // The underground has NO converter of its own. Siofra, Ainsel, Deeproot and Mohgwyn are area
    // 12, and they reach map space by having the legacy converter rewrite them into an overworld
    // block, which slot 0 then accepts -- so they arrive here looking like layer 0. The engine
    // corrects that with exactly this test (`FUN_140887870`: `if (mapId == 0 && areaId == 0x0C)
    // mapId = 1`), and it is mirrored rather than skipped so the mapping stays right if a catalog
    // ever does contain area-12 points. The shipped one does not, which is why the underground
    // map honestly has no invasion pins.
    if layer_id == 0 && block_area == er_invasion_warp::param_row::AREA_UNDERGROUND {
        layer_id = 1;
    }
    // `FUN_140887e90`: layer 0 -> bit 0, 1 -> bit 1, 10 -> bit 2, anything else -> invisible.
    match layer_id {
        0 => Some(er_invasion_warp::param_row::LAYER_BIT_SURFACE),
        1 => Some(er_invasion_warp::param_row::LAYER_BIT_UNDERGROUND),
        10 => Some(er_invasion_warp::param_row::LAYER_BIT_SHADOW_LANDS),
        _ => None,
    }
}

/// `BonfireWarpParam+0x20` -- the row's own area number, used to keep a base-map pin from
/// borrowing a name whose coordinates live in the DLC's frame.
pub const PARAM_AREA_NO_OFFSET: usize = 0x20;

/// The `PlaceName` text id of the shipped warp row nearest `coords`, or `-1` when there is none.
///
/// Every injected pin previously carried the DONOR row's label, so all 365 read "Godrick the
/// Grafted" -- the donor is a Site of Grace and its name was copied wholesale. The game has no
/// block-to-place-name function to ask instead, but it does not need one: 225 of the shipped warp
/// rows in areas 60/61 carry a valid `PlaceName` text id, and they are already sitting in the
/// list being appended to, already projected into map space by the engine. Naming a pin after the
/// nearest one costs a walk over resident memory and no engine calls at all.
///
/// `-1` is returned rather than any fallback id, and that choice is load-bearing: an id that
/// resolves in no FMG does NOT render blank, it renders the literal `?PlaceName?` on the pin.
/// Only `-1` produces an empty label.
///
/// # Safety
/// Game thread; `begin` must point at the first constructed row. Every read is fault-tolerant.
#[cfg(windows)]
unsafe fn nearest_place_name_text_id(
    begin: usize,
    existing_rows: usize,
    area: u8,
    coords: MapCoordinates,
) -> i32 {
    use er_invasion_warp::param_row::{PARAM_LABEL_KIND_BASE, PARAM_LABEL_TEXT_ID_BASE};

    let mut best_text_id = -1_i32;
    let mut best_distance = f32::INFINITY;
    for index in 0..existing_rows {
        let row = begin + index * PIN_ROW_STRIDE;
        let Some(param) =
            (unsafe { er_game_base::mem::safe_read_usize(row + ROW_PARAM_POINTER_OFFSET) })
        else {
            continue;
        };
        if param == 0 {
            continue;
        }
        // Same area only. A base-map pin latching onto a DLC row would take a name whose
        // coordinates mean something in a different frame entirely, which is the same class of
        // mistake as drawing a DLC pin on the base map.
        if unsafe { er_game_base::mem::safe_read_u8(param + PARAM_AREA_NO_OFFSET) } != Some(area) {
            continue;
        }
        // Label 0 must be a PlaceName; an NpcName would give the pin a character's name.
        if unsafe { er_game_base::mem::safe_read_u8(param + PARAM_LABEL_KIND_BASE) } != Some(0) {
            continue;
        }
        let Some(text_id) =
            (unsafe { er_game_base::mem::safe_read_i32(param + PARAM_LABEL_TEXT_ID_BASE) })
        else {
            continue;
        };
        if text_id <= 0 {
            continue;
        }
        let Some(x) = (unsafe { er_game_base::mem::safe_read_f32(row + 0x10) }) else {
            continue;
        };
        let Some(z) = (unsafe { er_game_base::mem::safe_read_f32(row + 0x14) }) else {
            continue;
        };
        let distance = (coords.x - x).powi(2) + (coords.z - z).powi(2);
        if distance < best_distance {
            best_distance = distance;
            best_text_id = text_id;
        }
    }
    best_text_id
}

/// Area byte of a packed `BlockId` (byte 3).
#[must_use]
pub const fn block_area(block_id: u32) -> u8 {
    ((block_id >> 24) & 0xFF) as u8
}

/// Append the invasion pins to a freshly-constructed ViewModel's row list.
///
/// Runs at the ctor EPILOGUE and nowhere else: `CS::WorldMapWarpData+0x08` holds raw pointers
/// into this buffer, and the reserve below relocates it. At ctor time no dialog exists, so
/// nothing can be holding a stale pointer.
///
/// # Safety
/// Game task thread, immediately after the original ctor returned.
#[cfg(windows)]
unsafe fn inject_pins(base: usize, view_model: usize) {
    use er_invasion_warp::map_surface::{InvasionRowRegistry, PinGranularity};
    use er_invasion_warp::param_row::{SYNTHETIC_PARAM_ROW_LEN, SyntheticParamSpec};

    let Some(before) = (unsafe { read_pin_list(view_model) }) else {
        INJECTIONS_SKIPPED.fetch_add(1, Ordering::SeqCst);
        crate::standalone_log(format_args!(
            "map-inject: pin list unreadable; no pins injected"
        ));
        return;
    };
    if !before.is_plausible() {
        INJECTIONS_SKIPPED.fetch_add(1, Ordering::SeqCst);
        crate::standalone_log(format_args!(
            "map-inject: pin list implausible (begin=0x{:x} end=0x{:x} cap=0x{:x}); no pins \
             injected",
            before.begin, before.end, before.capacity
        ));
        return;
    }
    let Some(existing_rows) = before.row_count() else {
        INJECTIONS_SKIPPED.fetch_add(1, Ordering::SeqCst);
        crate::standalone_log(format_args!(
            "map-inject: row span does not divide by the 0x350 stride; refusing to append"
        ));
        return;
    };
    if existing_rows == 0 {
        // Nothing to sample a donor from, and a shipped map always has warp rows.
        INJECTIONS_SKIPPED.fetch_add(1, Ordering::SeqCst);
        crate::standalone_log(format_args!(
            "map-inject: list is empty, so there is no donor row to sample; no pins injected"
        ));
        return;
    }
    // Enumerate the icon ids the shipped rows actually use, so a distinct one can be chosen
    // from what the game has rather than guessed at.
    {
        use er_invasion_warp::param_row::PARAM_ICON_ID_OFFSET;
        let mut seen: Vec<u16> = Vec::new();
        for index in 0..existing_rows.min(MAX_DONOR_SCAN_ROWS) {
            let row = before.begin + index * PIN_ROW_STRIDE;
            let Some(param) =
                (unsafe { er_game_base::mem::safe_read_usize(row + ROW_PARAM_POINTER_OFFSET) })
            else {
                continue;
            };
            if param == 0 {
                continue;
            }
            if let Some(icon) =
                unsafe { er_game_base::mem::safe_read_u16(param + PARAM_ICON_ID_OFFSET) }
                && !seen.contains(&icon)
            {
                seen.push(icon);
            }
        }
        seen.sort_unstable();
        let red_installed = crate::map_gfx::red_pin_frame_installed();
        crate::standalone_log(format_args!(
            "map-inject: shipped rows use icon frames {seen:?}; invasion pins will use frame {} \
             (red marker installed: {red_installed})",
            er_invasion_warp::param_row::invasion_pin_icon_id(red_installed)
        ));
    }
    let Some(donor) = (unsafe { sample_donor(before.begin, existing_rows) }) else {
        INJECTIONS_SKIPPED.fetch_add(1, Ordering::SeqCst);
        crate::standalone_log(format_args!(
            "map-inject: no shipped row among the first {} has non-zero category bits and a \
                 non-negative label text id; without a filter-passing donor the pins would be \
                 discarded, so none were injected",
            MAX_DONOR_SCAN_ROWS.min(existing_rows)
        ));
        return;
    };

    // The catalog is RE-READ on every injection, and the derived registry is rebuilt only when
    // the data actually changed.
    //
    // The spawn table is not a constant. Seamless Co-op's `ersc.dll` rewrites the invasion spawn
    // regions the vanilla game ships, and the whole point of reading `CSAutoInvadePoint` live is
    // that the pins show whatever is actually loaded: vanilla points under a vanilla profile,
    // ersc's under a Seamless one. An earlier version cached the registry for the session to stop
    // a per-injection leak, which quietly broke that -- read once before ersc patched and the map
    // shows the wrong spawns for the rest of the session, with nothing to indicate it.
    //
    // The leak is avoided by comparing a cheap signature instead of by refusing to look. Only a
    // genuine change re-leaks, and injection runs per WORLD LOAD (the ViewModel is built in
    // `MoveMapStep`), not per frame and not per map open -- so the walk sits inside a load the
    // player is already waiting through.
    let catalog = match unsafe { er_invasion_warp::invasion_warp::collect_invasion_warp_catalog() }
    {
        Ok(catalog) => catalog,
        Err(error) => {
            INJECTIONS_SKIPPED.fetch_add(1, Ordering::SeqCst);
            crate::standalone_log(format_args!(
                "map-inject: invasion catalog unavailable at ViewModel ctor time ({error}); no \
                 pins injected"
            ));
            return;
        }
    };
    // Is this the shipped table, or has a mod rewritten it in memory?
    //
    // The count oracle cannot tell: Seamless Co-op modifies invasion locations at runtime, and a
    // mod that MOVES points without adding or removing any leaves 365 blocks / 7073 points
    // looking untouched. This folds every position and yaw into the same canonical form the
    // on-disk containers hash to, so a moved point is visible. It is also the measurement that
    // decides whether on-disk data could ever describe what a player will actually encounter --
    // if the live table differs, offline sources are wrong by construction.
    {
        let content: Vec<_> = catalog
            .targets()
            .iter()
            .map(|target| (target.block, target.position, target.yaw))
            .collect();
        let digest = er_invasion_warp::aip::catalog_content_digest(&content);
        let vanilla = er_invasion_warp::aip::AIP_CATALOG_CONTENT_DIGEST_VANILLA;
        crate::standalone_log(format_args!(
            "map-inject: live spawn table digest {digest:#018x} vs vanilla on-disk \
             {vanilla:#018x} -> {}",
            if digest == vanilla {
                "IDENTICAL (no mod has rewritten the points)"
            } else {
                "DIFFERENT -- a mod has rewritten the spawn points in memory; the pins show the \
                 LIVE data, and any on-disk source would be wrong"
            }
        ));
    }
    let fresh = InvasionRowRegistry::from_catalog(&catalog, PinGranularity::PerBlock);
    let signature = catalog_signature(&fresh);
    let cached_registry = INJECTED_REGISTRY.load(Ordering::SeqCst);
    let registry: &'static InvasionRowRegistry =
        if cached_registry != 0 && CATALOG_SIGNATURE.load(Ordering::SeqCst) == signature {
            // SAFETY: leaked by this function, never freed, and only replaced (never mutated).
            unsafe { &*(cached_registry as *const InvasionRowRegistry) }
        } else {
            if cached_registry != 0 {
                crate::standalone_log(format_args!(
                    "map-inject: the invasion spawn table CHANGED under us (signature \
                     {:#018x} -> {signature:#018x}); rebuilding the pin set so the map shows the \
                     spawns that are actually loaded",
                    CATALOG_SIGNATURE.load(Ordering::SeqCst)
                ));
                // The param rows describe the old set, so they must be rebuilt too.
                SHARED_PARAM_ROWS_PTR.store(0, Ordering::SeqCst);
                SHARED_PARAM_ROWS_LEN.store(0, Ordering::SeqCst);
                SHARED_PARAM_ROWS_ICON.store(usize::MAX, Ordering::SeqCst);
            }
            let leaked: &'static InvasionRowRegistry = Box::leak(Box::new(fresh));
            INJECTED_REGISTRY.store(core::ptr::from_ref(leaked) as usize, Ordering::SeqCst);
            CATALOG_SIGNATURE.store(signature, Ordering::SeqCst);
            leaked
        };
    let wanted = registry.len();
    if wanted == 0 {
        INJECTIONS_SKIPPED.fetch_add(1, Ordering::SeqCst);
        crate::standalone_log(format_args!(
            "map-inject: registry is empty; no pins injected"
        ));
        return;
    }

    // PROJECT FIRST. The layer bit a pin must carry is decided by WHICH converter accepted it,
    // not by anything readable off the block on its own, so the projection has to run before the
    // param rows are authored rather than after them.
    let projections: Vec<Option<(MapCoordinates, usize, u8)>> = registry
        .targets()
        .iter()
        .map(|target| unsafe {
            project_to_map(base, view_model, target.block.raw(), target.position)
        })
        .collect();

    // One param row per pin, leaked on purpose: the pin does not own it, its dtor never touches
    // it, but IsOpen / the row filter / the label refresh all dereference it on demand for the
    // rest of the session.
    // Build the param rows ONCE and share them across every map view. A pin does not own its
    // param row, so one immutable set serves all views; rebuilding per view would leak a fresh
    // copy every time the player switched between the overworld, underground and Shadow Lands.
    let cached = SHARED_PARAM_ROWS_PTR.load(Ordering::SeqCst);
    let param_rows: &'static [[u8; SYNTHETIC_PARAM_ROW_LEN]] = if cached != 0 {
        let len = SHARED_PARAM_ROWS_LEN.load(Ordering::SeqCst);
        // SAFETY: leaked on the first injection and never freed or mutated.
        unsafe { core::slice::from_raw_parts(cached as *const [u8; SYNTHETIC_PARAM_ROW_LEN], len) }
    } else {
        let mut rows: Vec<[u8; SYNTHETIC_PARAM_ROW_LEN]> = Vec::with_capacity(wanted);
        for index in 0..wanted {
            let Some(entity_id) = registry.entity_id_at(index) else {
                break;
            };
            // The layer bit follows the CONVERTER THAT ACCEPTED THIS PIN, because a row carries
            // exactly one coordinate and that coordinate only means anything on the map whose
            // converter produced it. A pin nothing accepted gets no bit at all -- it is dropped
            // below rather than given a default that would draw it somewhere arbitrary.
            let block_area_byte = registry
                .targets()
                .get(index)
                .map_or(0, |target| block_area(target.block.raw()));
            let projection = projections.get(index).and_then(|p| p.as_ref());
            let layer_bits = projection
                .and_then(|(_, converter_index, _)| {
                    layer_bit_for_converter(base, *converter_index, block_area_byte)
                })
                .unwrap_or(0);
            // Name the pin after the nearest shipped warp row in its own area, instead of
            // cloning the donor's name onto all 365.
            let place_name_text_id = projection.map_or(-1, |(coords, _, _)| unsafe {
                nearest_place_name_text_id(before.begin, existing_rows, block_area_byte, *coords)
            });
            rows.push(
                SyntheticParamSpec {
                    entity_id,
                    subcategory_id: donor.subcategory_id,
                    // Deliberately NOT the donor's icon: the donor is a grace, and the id is a
                    // GFx frame number, so copying it draws a Site of Grace.
                    icon_id: er_invasion_warp::param_row::invasion_pin_icon_id(
                        crate::map_gfx::red_pin_frame_installed(),
                    ),
                    // NOT the donor's bits, and NOT all three. These are per-map-layer
                    // visibility bits over a row that holds ONE coordinate, so all-three drew
                    // every Shadow Lands pin on the Lands Between map too -- at Shadow Lands
                    // coordinates, i.e. out in the sea, while still warping correctly because
                    // the warp reads the block id and never the map position.
                    category_bits: layer_bits,
                    place_name_text_id,
                }
                .to_row_bytes(),
            );
        }
        {
            use er_invasion_warp::param_row::PARAM_LABEL_TEXT_ID_BASE;
            let named = rows
                .iter()
                .filter(|row| {
                    i32::from_le_bytes(
                        row[PARAM_LABEL_TEXT_ID_BASE..PARAM_LABEL_TEXT_ID_BASE + 4]
                            .try_into()
                            .unwrap_or([0; 4]),
                    ) > 0
                })
                .count();
            crate::standalone_log(format_args!(
                "map-inject: named {named}/{} pins from the nearest shipped warp row; the rest \
                 carry -1 and render an EMPTY label (never a fallback id -- an id no FMG \
                 resolves prints \"?PlaceName?\" on the pin)",
                rows.len()
            ));
        }
        let leaked: &'static [[u8; SYNTHETIC_PARAM_ROW_LEN]] = Box::leak(rows.into_boxed_slice());
        SHARED_PARAM_ROWS_PTR.store(leaked.as_ptr() as usize, Ordering::SeqCst);
        SHARED_PARAM_ROWS_LEN.store(leaked.len(), Ordering::SeqCst);
        leaked
    };
    // Re-stamp the icon frame every injection rather than trusting the one baked in at the first.
    //
    // The frame depends on whether the edited world-map movie has been served, which is an
    // OBSERVED fact that starts out false. Both live runs parsed the movie during boot, before
    // any world load, so the first injection already saw `true` -- but that ordering is the
    // loader's business, not ours. If a world ever loads first, the cached rows would be frozen
    // on the fallback icon for the whole session and no later swap could rescue them.
    //
    // Writing the param bytes is safe at any time: a pin copies the icon out of its param at
    // construction (`param+0x1C` -> `pin+0x248`), so a re-stamp cannot disturb a pin that already
    // exists -- it only decides what the NEXT ViewModel's pins are built with, which is exactly
    // the scope wanted.
    {
        use er_invasion_warp::param_row::PARAM_ICON_ID_OFFSET;
        let desired = er_invasion_warp::param_row::invasion_pin_icon_id(
            crate::map_gfx::red_pin_frame_installed(),
        );
        let stamped = SHARED_PARAM_ROWS_ICON.swap(desired as usize, Ordering::SeqCst);
        if stamped != desired as usize {
            // SAFETY: this slice was leaked by this function and is never freed; the game only
            // ever reads it, and this runs on the game thread inside the ctor.
            let rows: &mut [[u8; SYNTHETIC_PARAM_ROW_LEN]] = unsafe {
                core::slice::from_raw_parts_mut(
                    SHARED_PARAM_ROWS_PTR.load(Ordering::SeqCst)
                        as *mut [u8; SYNTHETIC_PARAM_ROW_LEN],
                    SHARED_PARAM_ROWS_LEN.load(Ordering::SeqCst),
                )
            };
            for row in rows.iter_mut() {
                row[PARAM_ICON_ID_OFFSET..PARAM_ICON_ID_OFFSET + 2]
                    .copy_from_slice(&desired.to_le_bytes());
            }
            crate::standalone_log(format_args!(
                "map-inject: re-stamped {} param rows onto icon frame {desired}",
                rows.len()
            ));
        }
    }
    if param_rows.len() < wanted {
        crate::standalone_log(format_args!(
            "map-inject: cached param rows hold {} entries but {wanted} pins were wanted; \
             injecting only what is backed",
            param_rows.len()
        ));
    }
    let wanted = wanted.min(param_rows.len());

    // Reserve ONCE with the final count. Each reserve copy-constructs every existing element
    // into a new block and destructs the originals, so per-row reserves are O(N*size) and
    // transiently double the peak menu-heap footprint.
    type ReserveFn = unsafe extern "system" fn(usize, usize);
    let reserve: ReserveFn =
        unsafe { core::mem::transmute(base + crate::map_seams::WORLDMAP_PIN_LIST_GROW.rva) };
    let vector = view_model + PIN_VECTOR_OFFSET;
    unsafe { reserve(vector, wanted) };

    // Re-read: the reserve moved the buffer.
    let Some(after_reserve) = (unsafe { read_pin_list(view_model) }) else {
        INJECTIONS_SKIPPED.fetch_add(1, Ordering::SeqCst);
        crate::standalone_log(format_args!(
            "map-inject: pin list unreadable after reserve; NOT appending"
        ));
        return;
    };
    if after_reserve.spare_rows() < wanted {
        INJECTIONS_SKIPPED.fetch_add(1, Ordering::SeqCst);
        crate::standalone_log(format_args!(
            "map-inject: reserve gave {} spare rows for {wanted} pins; NOT appending",
            after_reserve.spare_rows()
        ));
        return;
    }

    type MakeRowFn = unsafe extern "system" fn(
        *mut u8,
        *const MapCoordinates,
        *const BonfireLookupResult,
    ) -> *mut u8;
    type CopyCtorFn = unsafe extern "system" fn(*mut u8, *const u8) -> *mut u8;
    type DtorFn = unsafe extern "system" fn(*mut u8);
    let make_row: MakeRowFn =
        unsafe { core::mem::transmute(base + crate::map_seams::WORLDMAP_PIN_ROW_CTOR.rva) };
    let copy_ctor: CopyCtorFn =
        unsafe { core::mem::transmute(base + crate::map_seams::WORLDMAP_PIN_ROW_COPY_CTOR.rva) };
    let dtor: DtorFn =
        unsafe { core::mem::transmute(base + crate::map_seams::WORLDMAP_PIN_ROW_DTOR.rva) };

    let mut injected = 0_usize;
    let mut unplaceable = 0_usize;
    // Per-area and per-converter tallies. The "0 CROSS-AREA" line was reassuring and nearly
    // meaningless: area 60 covers BOTH the surface and the underground, so an area match cannot
    // tell a Siofra block from a Limgrave one. Counting which converter actually accepted each
    // pin is the measurement that can, because the converters are what differ per map.
    let mut per_area: [usize; 2] = [0, 0]; // [area 60, area 61]
    let mut per_converter: [usize; 8] = [0; 8];
    /// How many pins were accepted by a converter belonging to a DIFFERENT area than the
    /// target's own -- those land in the wrong map's coordinate space.
    let mut cross_area_projections = 0_usize;
    let mut cross_area_trace = 4_usize;
    let mut area_trace = 4_usize;
    for (index, target) in registry.targets().iter().enumerate() {
        // Reuse the projection computed above rather than re-running it: the layer bit and the
        // coordinate must come from the SAME converter decision, and projecting twice invites
        // them to disagree as well as doubling 365 native calls inside a world load.
        let Some((coords, converter_index, converter_area)) =
            projections.get(index).copied().flatten()
        else {
            unplaceable += 1;
            continue;
        };
        // A pin whose converter carries no layer entry can never be drawn on any map, so it is
        // dropped rather than appended with a zero mask that would make it permanently invisible
        // while still occupying a row and a clip-pool slot.
        if layer_bit_for_converter(base, converter_index, block_area(target.block.raw())).is_none()
        {
            unplaceable += 1;
            continue;
        }
        let target_area = block_area(target.block.raw());
        if target_area == er_invasion_warp::param_row::AREA_SHADOW_LANDS {
            per_area[1] += 1;
        } else {
            per_area[0] += 1;
        }
        if let Some(slot) = per_converter.get_mut(converter_index) {
            *slot += 1;
        }
        if converter_area != target_area {
            // The converter that accepted this point belongs to a DIFFERENT area, so the map
            // coordinates are in that area's space and the pin renders somewhere meaningless.
            // This is the leading explanation for "markers on the base map, none on the DLC map,
            // and not where I'd expect".
            cross_area_projections += 1;
            if cross_area_trace > 0 {
                cross_area_trace -= 1;
                crate::standalone_log(format_args!(
                    "map-inject: CROSS-AREA projection: block {} (area {target_area}) accepted by \
                     converter #{converter_index} (area {converter_area}) -> map[{:.1}, {:.1}]",
                    target.block, coords.x, coords.z
                ));
            }
        } else if area_trace > 0 {
            area_trace -= 1;
            crate::standalone_log(format_args!(
                "map-inject: sample: block {} area={target_area} converter=#{converter_index} \
                 map[{:.1}, {:.1}] aip[{:.1}, {:.1}, {:.1}]",
                target.block,
                coords.x,
                coords.z,
                target.position[0],
                target.position[1],
                target.position[2]
            ));
        }
        let lookup = BonfireLookupResult {
            param_id: registry.entity_id_at(index).unwrap_or(0),
            pad: 0,
            param_row: param_rows[index].as_ptr(),
        };
        let mut temp = TempPinRow([0_u8; PIN_ROW_STRIDE]);
        unsafe { make_row(temp.0.as_mut_ptr(), &raw const coords, &raw const lookup) };

        // Re-read `end` every iteration and write it back, exactly as the ctor's own append does.
        let Some(end) = (unsafe { er_game_base::mem::safe_read_usize(vector + VECTOR_END_OFFSET) })
        else {
            unsafe { dtor(temp.0.as_mut_ptr()) };
            break;
        };
        if end != 0 {
            unsafe { copy_ctor(end as *mut u8, temp.0.as_ptr()) };
            // Stamp a DISTINCT row id. The base ctor draws `+0x8` from the engine's counter
            // (`CS::WorldMapPinDataBase::WorldMapPinDataBase`), but the copy-ctor copies it
            // verbatim and never re-runs that ctor -- so every row cloned from one temp would
            // otherwise carry the SAME id.
            //
            // That is not cosmetic. The marker draw uses `+0x8` purely as a change-detection
            // token: a clip slot is re-bound (`SetTo`, which is what sets the icon and the
            // visibility) only when `idCache[slot] != row+0x8`. With duplicate ids the engine
            // concludes the slot already shows this row, skips the re-bind, and then moves the
            // clip to the new row's coordinates -- leaving the PREVIOUS pin's icon sitting at
            // this pin's position. Distinct ids are what make each pin render as itself.
            unsafe {
                *((end + ROW_ID_OFFSET) as *mut i32) =
                    INJECTED_ROW_ID_BASE.wrapping_add(index as i32);
            }
            unsafe { *((vector + VECTOR_END_OFFSET) as *mut usize) = end + PIN_ROW_STRIDE };
            injected += 1;
        }
        // MUST use the engine dtor: the temp owns its MenuString and up to 8 label DLStrings.
        unsafe { dtor(temp.0.as_mut_ptr()) };
    }

    PINS_INJECTED.store(injected, Ordering::SeqCst);
    if injected > 0 {
        INJECTIONS_PERFORMED.fetch_add(1, Ordering::SeqCst);
    } else {
        INJECTIONS_SKIPPED.fetch_add(1, Ordering::SeqCst);
    }
    // Record the span AFTER the appends: the reserve already relocated the buffer, so these are
    // the final addresses the filter will be asked about.
    if injected > 0
        && let Some(final_geometry) = unsafe { read_pin_list(view_model) }
    {
        let first = final_geometry.begin + existing_rows * PIN_ROW_STRIDE;
        record_injected_span(first, first + injected * PIN_ROW_STRIDE);
    }
    crate::standalone_log(format_args!(
        "map-inject: layer split: area60(surface bit)={} area61(shadow-lands bit)={}; converter \
         usage={per_converter:?} -- a pin is drawn ONLY on the layer whose bit it carries, \
         because its single coordinate is only valid in that converter's space",
        per_area[0], per_area[1]
    ));
    let settled = unsafe { read_pin_list(view_model) };
    crate::standalone_log(format_args!(
        "map-inject: appended {injected} invasion pins ({unplaceable} unplaceable, \
         {cross_area_projections} CROSS-AREA (wrong map's coordinate space), {wanted} wanted, \
         {existing_rows} shipped rows before) -> list now rows={} spare={} plausible={} \
         donor[row={} subcategory={} category_bits=0x{:x} icon={} label_text_id={}]",
        settled
            .and_then(|g| g.row_count())
            .map_or_else(|| "UNREADABLE".to_string(), |r| r.to_string()),
        settled.map_or(0, |g| g.spare_rows()),
        settled.is_some_and(|g| g.is_plausible()),
        donor.donor_row_index,
        donor.subcategory_id,
        donor.category_bits,
        donor.icon_id,
        donor.label_text_id,
    ));
}

/// The injected registry, leaked so the confirm hook can map a synthetic entity id back to its
/// target for the rest of the session. 0 until the injection runs.
static INJECTED_REGISTRY: AtomicUsize = AtomicUsize::new(0);
/// Trampoline to the original warp-job assembler.
static ORIG_WARP_JOB_ASSEMBLER: AtomicUsize = AtomicUsize::new(0);
/// Confirms recognised as ours, and how many of those issued a warp.
static CONFIRMS_INTERCEPTED: AtomicUsize = AtomicUsize::new(0);
static CONFIRMS_WARPED: AtomicUsize = AtomicUsize::new(0);

/// Union handler for the warp-job assembler `FUN_1407a04f0`.
///
/// THIS IS THE SOFTLOCK FIX. All five confirm routes funnel through here, and `R8` points at the
/// bonfire entity id BEFORE any MenuJob is allocated. Without this hook, selecting an injected
/// pin hands our synthetic id to the native grace warp, which passes it to
/// `CSLuaEventManImp::CallLua_Warp`; Lua cannot resolve it, the stage transition never completes,
/// and the game hangs on the loading screen. That is exactly what a live run did.
///
/// On recognising one of ours we run the proven local warp and return a NULL job. Swallowing is
/// safe: the callers' `Clone` (0x1407a7b60) and enqueue (0x1407a9250) both NULL-check, and the
/// engine itself returns a NULL job on its own no-SpecialEffect path -- so a NULL out-slot is a
/// state the callers already handle. The map is torn down by the area reload our warp kicks.
///
/// # Safety
/// Installed by the union on a byte-verified prologue; ABI is
/// `(outJobSlot, menuOwner+0x50, const u32* entityId, MenuString* name)`.
#[cfg(windows)]
unsafe extern "system" fn warp_job_assembler_hook(
    out_job_slot: usize,
    menu_owner: usize,
    entity_id_ptr: usize,
    name: usize,
) -> usize {
    let entity_id = if entity_id_ptr != 0 {
        unsafe { er_game_base::mem::safe_read_i32(entity_id_ptr) }
    } else {
        None
    };
    let registry_ptr = INJECTED_REGISTRY.load(Ordering::SeqCst);
    if let (Some(entity_id), true) = (entity_id, registry_ptr != 0)
        && er_invasion_warp::map_surface::is_invasion_entity_id(entity_id)
    {
        // SAFETY: the registry was leaked at injection time and is never freed or mutated.
        let registry: &er_invasion_warp::map_surface::InvasionRowRegistry =
            unsafe { &*(registry_ptr as *const _) };
        if let Some(target) = registry.target_for_entity_id(entity_id) {
            CONFIRMS_INTERCEPTED.fetch_add(1, Ordering::SeqCst);
            match unsafe { er_invasion_warp::warp::request_invasion_warp(target) } {
                Ok(outcome) => {
                    CONFIRMS_WARPED.fetch_add(1, Ordering::SeqCst);
                    crate::standalone_log(format_args!(
                        "map-confirm: invasion pin entity_id={entity_id:#x} -> LOCAL warp to \
                         block {} point {} (requested {:#010x} effective {:#010x} spawn_flag={} \
                         session_touches={}); native grace warp SWALLOWED",
                        target.block,
                        target.point_index,
                        outcome.requested_block,
                        outcome.effective_block,
                        outcome.spawn_flag,
                        outcome.session_touches
                    ));
                }
                Err(error) => {
                    crate::standalone_log(format_args!(
                        "map-confirm: invasion pin entity_id={entity_id:#x} REFUSED: {error}; \
                         native warp still swallowed rather than sending a synthetic id to \
                         Lua_Warp (which softlocks)"
                    ));
                }
            }
            // NULL job either way. Letting the native path run with a synthetic id is the
            // softlock, so it is never the fallback.
            if out_job_slot != 0 {
                unsafe { *(out_job_slot as *mut usize) = 0 };
            }
            return out_job_slot;
        }
    }

    let orig = ORIG_WARP_JOB_ASSEMBLER.load(Ordering::SeqCst);
    if orig == 0 {
        // No trampoline: refuse rather than fabricate a job pointer.
        if out_job_slot != 0 {
            unsafe { *(out_job_slot as *mut usize) = 0 };
        }
        return out_job_slot;
    }
    type AssemblerFn = unsafe extern "system" fn(usize, usize, usize, usize) -> usize;
    let original: AssemblerFn = unsafe { core::mem::transmute(orig) };
    unsafe { original(out_job_slot, menu_owner, entity_id_ptr, name) }
}

/// Confirm-hook tallies: `(intercepted, warped)`.
#[must_use]
pub fn confirm_tallies() -> (usize, usize) {
    (
        CONFIRMS_INTERCEPTED.load(Ordering::SeqCst),
        CONFIRMS_WARPED.load(Ordering::SeqCst),
    )
}

/// Union handler for the fast-travel list filter `FUN_14088be50`.
///
/// Observation only -- it forwards the original verdict untouched.
///
/// It was installed believing it was the MAP-MARKER visibility gate. It is not: its callers all
/// build the fast-travel list and the bookmark dialog, so `ours 0/0` here means "our rows were
/// never offered to the warp list", which is a different question from "are the pins drawn".
/// The counters are kept because that first question is still worth answering, but nothing may
/// conclude from them that the markers are missing.
///
/// # Safety
/// Installed by the union on a byte-verified prologue; ABI is `(row, mask, allowUnvisited)`.
#[cfg(windows)]
unsafe extern "system" fn worldmap_row_filter_hook(
    row: usize,
    mask: usize,
    allow_unvisited: usize,
    d: usize,
) -> usize {
    let orig = ORIG_ROW_FILTER.load(Ordering::SeqCst);
    if orig == 0 {
        // Claiming a verdict we did not compute would silently change what the map shows.
        return 0;
    }
    type FilterFn = unsafe extern "system" fn(usize, usize, usize, usize) -> usize;
    let original: FilterFn = unsafe { core::mem::transmute(orig) };
    let verdict = unsafe { original(row, mask, allow_unvisited, d) };

    let ours = row_is_ours(row);
    // The verdict is a `char`; only the low byte is meaningful.
    let passed = (verdict & 0xFF) != 0;
    if ours {
        FILTER_QUERIES_OURS.fetch_add(1, Ordering::SeqCst);
        if passed {
            FILTER_PASSES_OURS.fetch_add(1, Ordering::SeqCst);
        }
    } else {
        FILTER_QUERIES_SHIPPED.fetch_add(1, Ordering::SeqCst);
        if passed {
            FILTER_PASSES_SHIPPED.fetch_add(1, Ordering::SeqCst);
        }
    }
    if ours && FILTER_TRACE_BUDGET.fetch_sub(1, Ordering::SeqCst) > 0 {
        let bits = unsafe { er_game_base::mem::safe_read_u8(row + 0x60) };
        let entity = unsafe { er_game_base::mem::safe_read_i32(row + ROW_ENTITY_ID_OFFSET) };
        crate::standalone_log(format_args!(
            "map-filter: OUR row 0x{row:x} verdict={passed} mask=0x{:x} allow_unvisited={} \
             row+0x60=0x{:02x} entity_id={:?}",
            mask as u32,
            allow_unvisited & 0xFF,
            bits.unwrap_or(0),
            entity
        ));
    }
    verdict
}

/// Filter verdict tallies: `(ours_queried, ours_passed, shipped_queried, shipped_passed)`.
#[must_use]
pub fn filter_verdicts() -> (usize, usize, usize, usize) {
    (
        FILTER_QUERIES_OURS.load(Ordering::SeqCst),
        FILTER_PASSES_OURS.load(Ordering::SeqCst),
        FILTER_QUERIES_SHIPPED.load(Ordering::SeqCst),
        FILTER_PASSES_SHIPPED.load(Ordering::SeqCst),
    )
}

/// Union handler for `CS::WorldMapViewModel::WorldMapViewModel`.
///
/// Calls the original FIRST -- the list does not exist until the ctor has run -- then reads the
/// list back. Observation only: nothing is written into the engine here.
///
/// # Safety
///
/// Installed by the union on a byte-verified prologue; the ABI is the ctor's own
/// `(this) -> this`.
#[cfg(windows)]
unsafe extern "system" fn worldmap_viewmodel_ctor_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let orig = ORIG_WORLDMAP_VIEWMODEL_CTOR.load(Ordering::SeqCst);
    if orig == 0 {
        // No trampoline means the original never ran. Returning a fabricated value would hand
        // the game a ViewModel that was never constructed. `a` is the `this` the ctor returns,
        // which is the least-wrong thing available, and the counter makes the situation visible
        // instead of silent.
        crate::standalone_log(format_args!(
            "map-hooks: BUG -- WorldMapViewModel ctor handler ran with no trampoline; the \
             ViewModel was NOT constructed"
        ));
        return a;
    }
    type CtorFn = unsafe extern "system" fn(usize, usize, usize, usize) -> usize;
    let original: CtorFn = unsafe { core::mem::transmute(orig) };
    let result = unsafe { original(a, b, c, d) };

    let hits = VIEWMODEL_CTOR_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    // `this` is in RCX and the ctor returns it; prefer the return value, fall back to the arg.
    let view_model = if result != 0 { result } else { a };
    match unsafe { read_pin_list(view_model) } {
        Some(geometry) => {
            let rows = geometry.row_count();
            if rows.is_none() {
                ROW_STRIDE_MISMATCH.fetch_add(1, Ordering::SeqCst);
            }
            OBSERVED_ROW_COUNT.store(rows.unwrap_or(usize::MAX), Ordering::SeqCst);
            crate::standalone_log(format_args!(
                "map-hooks: WorldMapViewModel ctor #{hits} this=0x{view_model:x} \
                 list[vftable=0x{:x} begin=0x{:x} end=0x{:x} capacity=0x{:x}] \
                 used={} capacity_bytes={} rows={} spare_rows={} plausible={}",
                geometry.vftable,
                geometry.begin,
                geometry.end,
                geometry.capacity,
                geometry.used_bytes(),
                geometry.capacity_bytes(),
                rows.map_or_else(|| "STRIDE-MISMATCH".to_string(), |r| r.to_string()),
                geometry.spare_rows(),
                geometry.is_plausible(),
            ));
        }
        None => {
            crate::standalone_log(format_args!(
                "map-hooks: WorldMapViewModel ctor #{hits} this=0x{view_model:x} -- pin list \
                 unreadable; NOT safe to inject rows"
            ));
        }
    }
    // SAFETY: ctor epilogue on the game thread -- the only moment no dialog can be holding a
    // raw pointer into the row buffer that the reserve is about to relocate.
    unsafe { inject_pins(base_for_inject(), view_model) };
    result
}

/// Module base for the injection's native calls; 0 makes every transmute obviously wrong, so
/// injection is skipped rather than jumping into nowhere.
#[cfg(windows)]
fn base_for_inject() -> usize {
    er_game_base::mem::game_module_base().unwrap_or(0)
}

/// Pins appended by the most recent injection.
#[must_use]
pub fn pins_injected() -> usize {
    PINS_INJECTED.load(Ordering::SeqCst)
}

/// `(ctor_hits, injections_performed, injections_skipped)`.
///
/// THE ORACLE for "the pins come back every time the map is opened". Every ViewModel
/// construction must be followed by an injection that appended rows, so a healthy session has
/// `injections_performed == ctor_hits` and `injections_skipped == 0`. Any gap means some map view
/// or some map open was left bare, and it is readable from memory without deciding anything from
/// a screenshot.
#[must_use]
pub fn injection_tallies() -> (usize, usize, usize) {
    (
        VIEWMODEL_CTOR_HITS.load(Ordering::SeqCst),
        INJECTIONS_PERFORMED.load(Ordering::SeqCst),
        INJECTIONS_SKIPPED.load(Ordering::SeqCst),
    )
}

/// Install the world-map observation hooks. Returns how many bound.
///
/// Every failure is logged and stepped over: losing an observer costs this run its evidence and
/// nothing else. Nothing here can disarm the already-proven warp.
///
/// # Safety
///
/// Call once, from the game task thread after the runtime is up.
#[cfg(windows)]
pub unsafe fn install_map_observers() -> usize {
    if CTOR_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return 0;
    }
    let address = match unsafe { verify_seam(&WORLDMAP_VIEWMODEL_CTOR) } {
        Ok(address) => address,
        Err(error) => {
            crate::standalone_log(format_args!("map-hooks: {error}"));
            return 0;
        }
    };
    match unsafe {
        er_hook::register_union_hook(
            address,
            worldmap_viewmodel_ctor_hook as er_hook::UnionFn,
            &ORIG_WORLDMAP_VIEWMODEL_CTOR,
        )
    } {
        Ok(()) => {
            crate::standalone_log(format_args!(
                "map-hooks: hooked {} @0x{address:x} (verified prologue)",
                WORLDMAP_VIEWMODEL_CTOR.name
            ));
            1 + unsafe { install_row_filter_observer() } + unsafe { install_confirm_interceptor() }
        }
        Err(status) => {
            crate::standalone_log(format_args!(
                "map-hooks: union registration for {} @0x{address:x} failed: {status:?} -- the \
                 map surface stays absent; the F7/F8/F9 warp is unaffected",
                WORLDMAP_VIEWMODEL_CTOR.name
            ));
            0
        }
    }
}

/// Observed row count, or `None` if the ctor has not fired or the stride did not divide.
#[must_use]
pub fn observed_row_count() -> Option<usize> {
    match OBSERVED_ROW_COUNT.load(Ordering::SeqCst) {
        usize::MAX => None,
        count => Some(count),
    }
}

/// How many times the ViewModel ctor fired. Above 1 refutes the once-per-session lifetime.
#[must_use]
pub fn viewmodel_ctor_hits() -> usize {
    VIEWMODEL_CTOR_HITS.load(Ordering::SeqCst)
}

/// Times the row span did not divide by the stride. Non-zero means DO NOT append.
#[must_use]
pub fn row_stride_mismatches() -> usize {
    ROW_STRIDE_MISMATCH.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(begin: usize, rows: usize, cap_rows: usize) -> PinListGeometry {
        PinListGeometry {
            vftable: 0x1_42ad_82a8,
            begin,
            end: begin + rows * PIN_ROW_STRIDE,
            capacity: begin + cap_rows * PIN_ROW_STRIDE,
        }
    }

    #[test]
    fn the_list_offsets_match_the_reverse_engineered_layout() {
        // {vfptr, allocator, begin, end, capacity} at 8-byte steps from +0x2d8.
        assert_eq!(PIN_LIST_VFTABLE_OFFSET, 0x2d8);
        assert_eq!(PIN_LIST_ALLOCATOR_OFFSET, PIN_LIST_VFTABLE_OFFSET + 8);
        assert_eq!(PIN_LIST_BEGIN_OFFSET, PIN_LIST_ALLOCATOR_OFFSET + 8);
        assert_eq!(PIN_LIST_END_OFFSET, PIN_LIST_BEGIN_OFFSET + 8);
        assert_eq!(PIN_LIST_CAPACITY_OFFSET, PIN_LIST_END_OFFSET + 8);
        assert_eq!(PIN_ROW_STRIDE, 0x350);
    }

    #[test]
    fn a_clean_span_reports_its_row_count() {
        let g = geometry(0x1000, 365, 400);
        assert_eq!(g.row_count(), Some(365));
        assert_eq!(g.used_bytes(), 365 * PIN_ROW_STRIDE);
        assert_eq!(g.spare_rows(), 35);
        assert!(g.is_plausible());
    }

    #[test]
    fn a_span_that_does_not_divide_by_the_stride_refuses_to_report_a_count() {
        // The check that stops an append into a list whose layout is not what we reversed.
        let g = PinListGeometry {
            vftable: 1,
            begin: 0x1000,
            end: 0x1000 + PIN_ROW_STRIDE + 1,
            capacity: 0x9000,
        };
        assert_eq!(g.row_count(), None);
        assert!(!g.is_plausible());
    }

    #[test]
    fn a_full_list_has_no_spare_rows() {
        let g = geometry(0x1000, 365, 365);
        assert_eq!(g.spare_rows(), 0);
        assert!(g.is_plausible(), "full is still a valid layout");
    }

    #[test]
    fn an_empty_list_is_plausible_and_reports_zero_rows() {
        let g = geometry(0x1000, 0, 0);
        assert_eq!(g.row_count(), Some(0));
        assert_eq!(g.spare_rows(), 0);
        assert!(g.is_plausible());
    }

    #[test]
    fn a_null_or_inverted_span_is_not_plausible() {
        assert!(!geometry(0, 10, 20).is_plausible(), "null begin");
        let inverted = PinListGeometry {
            vftable: 1,
            begin: 0x9000,
            end: 0x1000,
            capacity: 0x9000,
        };
        assert!(!inverted.is_plausible(), "end before begin");
        let over = PinListGeometry {
            vftable: 1,
            begin: 0x1000,
            end: 0x9000,
            capacity: 0x2000,
        };
        assert!(!over.is_plausible(), "end past capacity");
    }

    /// A private span table, so these tests never race the process-wide one.
    fn span_table() -> (
        [(AtomicUsize, AtomicUsize); MAX_INJECTED_SPANS],
        AtomicUsize,
    ) {
        (
            [const { (AtomicUsize::new(0), AtomicUsize::new(0)) }; MAX_INJECTED_SPANS],
            AtomicUsize::new(0),
        )
    }

    #[test]
    fn a_row_in_any_recorded_span_is_recognised_as_ours() {
        // The defect this pins: one live ViewModel's injection used to overwrite the recorded
        // span of every other live one, so the filter observer reported `ours 0/0` for views it
        // had genuinely injected -- a false negative in the visibility oracle.
        let (spans, cursor) = span_table();
        record_span(&spans, &cursor, 0x1000, 0x2000);
        record_span(&spans, &cursor, 0x9000, 0xA000);
        assert!(span_contains(&spans, 0x1000), "first span, first row");
        assert!(span_contains(&spans, 0x1FFF), "first span, last byte");
        assert!(
            span_contains(&spans, 0x9500),
            "second span still recognised"
        );
        assert!(
            !span_contains(&spans, 0x2000),
            "one past the end is not ours"
        );
        assert!(
            !span_contains(&spans, 0x8FFF),
            "between the spans is not ours"
        );
        assert!(!span_contains(&spans, 0), "a null row is never ours");
    }

    #[test]
    fn the_span_table_wraps_instead_of_growing_without_bound() {
        // Runs on the game thread inside a ctor, so it must stay allocation-free and bounded.
        let (spans, cursor) = span_table();
        for index in 0..MAX_INJECTED_SPANS * 2 {
            let begin = 0x10_0000 + index * 0x1000;
            record_span(&spans, &cursor, begin, begin + 0x800);
        }
        let newest = 0x10_0000 + (MAX_INJECTED_SPANS * 2 - 1) * 0x1000;
        assert!(
            span_contains(&spans, newest),
            "the newest span survives the wrap"
        );
        let oldest = 0x10_0000;
        assert!(
            !span_contains(&spans, oldest),
            "the oldest span was evicted rather than the table growing"
        );
        assert_eq!(INJECTED_SPANS.len(), MAX_INJECTED_SPANS);
    }

    #[test]
    fn saturating_arithmetic_keeps_a_garbage_span_from_wrapping() {
        let g = PinListGeometry {
            vftable: 0,
            begin: usize::MAX,
            end: 0,
            capacity: 0,
        };
        assert_eq!(g.used_bytes(), 0);
        assert_eq!(g.capacity_bytes(), 0);
        assert_eq!(g.spare_rows(), 0);
        assert!(!g.is_plausible());
    }
}

/// Install the row-filter observer. Failure costs the visibility oracle and nothing else.
///
/// # Safety
/// Game task thread.
#[cfg(windows)]
unsafe fn install_row_filter_observer() -> usize {
    let seam = crate::map_seams::WORLDMAP_ROW_FILTER;
    let address = match unsafe { verify_seam(&seam) } {
        Ok(address) => address,
        Err(error) => {
            crate::standalone_log(format_args!("map-hooks: {error}"));
            return 0;
        }
    };
    match unsafe {
        er_hook::register_union_hook(
            address,
            worldmap_row_filter_hook as er_hook::UnionFn,
            &ORIG_ROW_FILTER,
        )
    } {
        Ok(()) => {
            crate::standalone_log(format_args!(
                "map-hooks: observing {} @0x{address:x} -- this is the visibility oracle",
                seam.name
            ));
            1
        }
        Err(status) => {
            crate::standalone_log(format_args!(
                "map-hooks: union registration for {} failed: {status:?} -- pins may still be \
                 fine, but this run cannot say whether they pass the filter",
                seam.name
            ));
            0
        }
    }
}

/// Install the confirm interceptor. Without it, selecting an injected pin softlocks, so a
/// failure here is logged loudly -- the pins are already in the list by then.
///
/// # Safety
/// Game task thread.
#[cfg(windows)]
unsafe fn install_confirm_interceptor() -> usize {
    let seam = crate::map_seams::WARP_JOB_ASSEMBLER;
    let address = match unsafe { verify_seam(&seam) } {
        Ok(address) => address,
        Err(error) => {
            crate::standalone_log(format_args!(
                "map-hooks: {error} -- WITHOUT THIS HOOK, SELECTING AN INJECTED PIN SOFTLOCKS"
            ));
            return 0;
        }
    };
    match unsafe {
        er_hook::register_union_hook(
            address,
            warp_job_assembler_hook as er_hook::UnionFn,
            &ORIG_WARP_JOB_ASSEMBLER,
        )
    } {
        Ok(()) => {
            crate::standalone_log(format_args!(
                "map-hooks: intercepting {} @0x{address:x} -- invasion pins now run the LOCAL \
                 warp instead of handing a synthetic id to Lua_Warp",
                seam.name
            ));
            1
        }
        Err(status) => {
            crate::standalone_log(format_args!(
                "map-hooks: union registration for {} failed: {status:?} -- SELECTING AN \
                 INJECTED PIN WILL SOFTLOCK",
                seam.name
            ));
            0
        }
    }
}
