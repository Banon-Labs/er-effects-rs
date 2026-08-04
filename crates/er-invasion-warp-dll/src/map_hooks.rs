//! The world-map detours.
//!
//! # The injection, and why it is gated the way it is
//!
//! The seam is the `CS::WorldMapViewModel` constructor: the pin-row list at `+0x2d8` is populated
//! there and nowhere else, and the ViewModel is built exactly once per session. Appending
//! anywhere else is unsafe -- `CS::WorldMapWarpData+0x08` holds RAW pointers into this buffer,
//! and the reserve relocates it, so a later append dangles every live dialog row pointer. At the
//! ctor epilogue no dialog exists yet.
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

use crate::map_seams::{WORLDMAP_VIEWMODEL_CTOR, verify_seam};

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

/// How many times the ctor hook has fired. The ViewModel is built once per session, so a value
/// above 1 means the lifetime assumption in section 5.3 is wrong and rows would need re-injecting.
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

/// First injected row address, and one past the last. A filter callback is "ours" when the row
/// falls in this half-open span -- an address test, which stays correct even though the reserve
/// relocated the buffer, because the span is recorded AFTER the reserve.
static INJECTED_ROWS_BEGIN: AtomicUsize = AtomicUsize::new(0);
static INJECTED_ROWS_END: AtomicUsize = AtomicUsize::new(0);

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

/// How many pins were injected, and why not more.
static PINS_INJECTED: AtomicUsize = AtomicUsize::new(0);
/// Set once injection has been attempted, so it can never run twice on one list.
static INJECTION_ATTEMPTED: AtomicUsize = AtomicUsize::new(0);

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

    if INJECTION_ATTEMPTED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let Some(before) = (unsafe { read_pin_list(view_model) }) else {
        crate::standalone_log(format_args!(
            "map-inject: pin list unreadable; no pins injected"
        ));
        return;
    };
    if !before.is_plausible() {
        crate::standalone_log(format_args!(
            "map-inject: pin list implausible (begin=0x{:x} end=0x{:x} cap=0x{:x}); no pins \
             injected",
            before.begin, before.end, before.capacity
        ));
        return;
    }
    let Some(existing_rows) = before.row_count() else {
        crate::standalone_log(format_args!(
            "map-inject: row span does not divide by the 0x350 stride; refusing to append"
        ));
        return;
    };
    if existing_rows == 0 {
        // Nothing to sample a donor from, and a shipped map always has warp rows.
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
        crate::standalone_log(format_args!(
            "map-inject: shipped rows use icon ids {seen:?}; invasion pins will use {}",
            er_invasion_warp::param_row::INVASION_PIN_ICON_ID
        ));
    }
    let Some(donor) = (unsafe { sample_donor(before.begin, existing_rows) }) else {
        crate::standalone_log(format_args!(
            "map-inject: no shipped row among the first {} has non-zero category bits and a \
                 non-negative label text id; without a filter-passing donor the pins would be \
                 discarded, so none were injected",
            MAX_DONOR_SCAN_ROWS.min(existing_rows)
        ));
        return;
    };

    let catalog = match unsafe { er_invasion_warp::invasion_warp::collect_invasion_warp_catalog() }
    {
        Ok(catalog) => catalog,
        Err(error) => {
            crate::standalone_log(format_args!(
                "map-inject: invasion catalog unavailable at ViewModel ctor time ({error}); no \
                 pins injected"
            ));
            return;
        }
    };
    let registry = InvasionRowRegistry::from_catalog(&catalog, PinGranularity::PerBlock);
    let wanted = registry.len();
    if wanted == 0 {
        crate::standalone_log(format_args!(
            "map-inject: registry is empty; no pins injected"
        ));
        return;
    }

    // One param row per pin, leaked on purpose: the pin does not own it, its dtor never touches
    // it, but IsOpen / the row filter / the label refresh all dereference it on demand for the
    // rest of the session.
    let mut param_rows: Vec<[u8; SYNTHETIC_PARAM_ROW_LEN]> = Vec::with_capacity(wanted);
    for index in 0..wanted {
        let Some(entity_id) = registry.entity_id_at(index) else {
            break;
        };
        param_rows.push(
            SyntheticParamSpec {
                entity_id,
                subcategory_id: donor.subcategory_id,
                // Deliberately NOT the donor's icon: cloning it made the pins look exactly
                // like Sites of Grace. This is the only visual-distinction lever at this layer.
                icon_id: er_invasion_warp::param_row::INVASION_PIN_ICON_ID,
                category_bits: donor.category_bits,
                place_name_text_id: donor.label_text_id,
            }
            .to_row_bytes(),
        );
    }
    let param_rows: &'static [[u8; SYNTHETIC_PARAM_ROW_LEN]] =
        Box::leak(param_rows.into_boxed_slice());
    // Leak the registry too: the confirm hook needs it for the rest of the session to map a
    // synthetic entity id back to the target to warp to.
    let leaked_registry: &'static InvasionRowRegistry = Box::leak(Box::new(registry.clone()));
    INJECTED_REGISTRY.store(
        core::ptr::from_ref(leaked_registry) as usize,
        Ordering::SeqCst,
    );

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
        crate::standalone_log(format_args!(
            "map-inject: pin list unreadable after reserve; NOT appending"
        ));
        return;
    };
    if after_reserve.spare_rows() < wanted {
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
    /// How many pins were accepted by a converter belonging to a DIFFERENT area than the
    /// target's own -- those land in the wrong map's coordinate space.
    let mut cross_area_projections = 0_usize;
    let mut cross_area_trace = 4_usize;
    let mut area_trace = 4_usize;
    for (index, target) in registry.targets().iter().enumerate() {
        let Some((coords, converter_index, converter_area)) =
            (unsafe { project_to_map(base, view_model, target.block.raw(), target.position) })
        else {
            unplaceable += 1;
            continue;
        };
        let target_area = block_area(target.block.raw());
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
            unsafe { *((vector + VECTOR_END_OFFSET) as *mut usize) = end + PIN_ROW_STRIDE };
            injected += 1;
        }
        // MUST use the engine dtor: the temp owns its MenuString and up to 8 label DLStrings.
        unsafe { dtor(temp.0.as_mut_ptr()) };
    }

    PINS_INJECTED.store(injected, Ordering::SeqCst);
    // Record the span AFTER the appends: the reserve already relocated the buffer, so these are
    // the final addresses the filter will be asked about.
    if injected > 0
        && let Some(final_geometry) = unsafe { read_pin_list(view_model) }
    {
        let first = final_geometry.begin + existing_rows * PIN_ROW_STRIDE;
        INJECTED_ROWS_BEGIN.store(first, Ordering::SeqCst);
        INJECTED_ROWS_END.store(first + injected * PIN_ROW_STRIDE, Ordering::SeqCst);
    }
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

/// Union handler for the row filter `FUN_14088be50`.
///
/// Observation only -- it forwards the original verdict untouched. It exists because "are the
/// pins visible" is otherwise a pixel question: this is the function that decides, so counting
/// its verdicts turns visibility into a RAM oracle. The shipped rows are counted alongside as a
/// control, so a mask that rejects EVERYTHING is distinguishable from one that rejects only ours.
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

    let begin = INJECTED_ROWS_BEGIN.load(Ordering::SeqCst);
    let end = INJECTED_ROWS_END.load(Ordering::SeqCst);
    let ours = begin != 0 && row >= begin && row < end;
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

/// Pins appended this session.
#[must_use]
pub fn pins_injected() -> usize {
    PINS_INJECTED.load(Ordering::SeqCst)
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
