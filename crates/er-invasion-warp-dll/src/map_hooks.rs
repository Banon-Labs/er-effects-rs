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

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use er_game_base::fnv1a::{fnv1a64, fnv1a64_mix};

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

/// How many times the ctor hook has fired. Measured >1 in practice, which refuted the original
/// "once per session" assumption -- but the replacement "once per map view" was wrong too. The
/// static call graph pins it: one ViewModel per WORLD ENTRY, destroyed with `MoveMapStep`. So a
/// value above 1 counts MAP MOVES, not layer toggles or map opens.
static VIEWMODEL_CTOR_HITS: AtomicUsize = AtomicUsize::new(0);
/// The ViewModel the last injection wrote into. Exactly one is ever alive.
///
/// Kept only to recognise the object the recorded span belongs to. It is NEVER the authority --
/// see [`authoritative_view_model`], which reads the engine's own slot.
pub(crate) static LIVE_VIEW_MODEL: AtomicUsize = AtomicUsize::new(0);

/// `CSPopupMenu+0x250` -- `CS::WorldMapViewModel*`, the engine's single authoritative slot.
///
/// Allocated by `FUN_1407ed840` only when this field is NULL, and freed AND NULLED by
/// `FUN_1407ed790` from `~MoveMapStep`. So reading it live cannot return a destroyed ViewModel,
/// which a stored pointer very much can: MenuHeap recycles a freed 0x450 block at the same size
/// class, and its pages stay mapped, so no amount of fault-tolerant reading detects the swap.
/// That is exactly how 456 rows got written inside other objects on 2026-08-05.
pub const POPUP_MENU_WORLD_MAP_VIEW_MODEL_OFFSET: usize = 0x250;

/// `WorldMapViewModel+0x08` -- the currently attached `CS::WorldMapDialog`, or null.
///
/// `FUN_140886750(viewModel, dialog)` is a compare-and-clear on this field and is called from the
/// dialog's destructor, so the slot is the engine's own "is the map open" answer. Reading it needs
/// no hook of ours, which matters: the obvious place to hook (`FUN_1409cef10`) takes SEVEN
/// arguments, and the union dispatcher forwards four.
pub const VIEW_MODEL_ATTACHED_DIALOG_OFFSET: usize = 0x08;

/// The live ViewModel, read from the engine's own slot rather than remembered.
///
/// `None` means there is no world map right now (no `MoveMapStep`), which is a refusal, not an
/// error.
#[cfg(windows)]
pub(crate) fn authoritative_view_model() -> Option<usize> {
    use fromsoftware_shared::FromStatic;
    let menu_man = unsafe { eldenring::cs::CSMenuManImp::instance() }.ok()?;
    let popup = menu_man.popup_menu?;
    let view_model = unsafe {
        er_game_base::mem::safe_read_usize(
            popup.as_ptr() as usize + POPUP_MENU_WORLD_MAP_VIEW_MODEL_OFFSET,
        )
    }?;
    (view_model != 0).then_some(view_model)
}

#[cfg(not(windows))]
pub(crate) fn authoritative_view_model() -> Option<usize> {
    None
}

/// Whether a world-map dialog is currently attached to `view_model`.
///
/// A dialog holds raw row pointers (`CS::WorldMapWarpData+0x08`) and its clip pool caches the list
/// base, so it is the thing that must not be alive while rows are being retargeted.
///
/// The field is written by exactly two functions, a matched compare-and-set pair, both proven
/// against the 1.16.2 image: the attach `0x140886540` (`MOV [RCX+0x8],RDX` at `0x14088654a`), whose
/// only caller is `CS::WorldMapDialogBase`'s constructor at `0x1409beeba`, and the detach
/// `0x140886750` (`MOV [RCX+0x8],0`) from that dialog's destructor. The `WorldMapViewModel`
/// constructor `0x1408855b0` zeroes it unconditionally four instructions in. So the slot is null
/// exactly while no world-map dialog object is alive -- opening the map, not entering the world, is
/// what closes this gate.
///
/// # It fails CLOSED, and the first version did not
///
/// An unreadable slot returns `true`. Reading it with `is_some_and` instead reported an unreadable
/// slot as "no dialog attached" and let the caller write rows -- inverted, because a read that
/// fails is the case where it is LEAST known whether a dialog is holding raw row pointers into the
/// buffer about to be retargeted. The `cfg(not(windows))` stub below has always returned `true`;
/// this is the Windows path agreeing with it.
#[cfg(windows)]
pub(crate) fn map_dialog_is_attached(view_model: usize) -> bool {
    unsafe { er_game_base::mem::safe_read_usize(view_model + VIEW_MODEL_ATTACHED_DIALOG_OFFSET) }
        .is_none_or(|dialog| dialog != 0)
}

#[cfg(not(windows))]
pub(crate) fn map_dialog_is_attached(_view_model: usize) -> bool {
    true
}
/// Choice signature the live rows were last restyled for.
pub(crate) static LIVE_RESTYLE_SIGNATURE: AtomicUsize = AtomicUsize::new(usize::MAX);

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
        if !used.is_multiple_of(PIN_ROW_STRIDE) {
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
/// Row field `+0x248` -- the icon id (a GFx frame number), copied from param `+0x1C` when the row
/// is built.
///
/// Writing it on a LIVE row is the only way to change a pin that already exists. Rebuilding is not
/// available: measured 2026-08-05, closing and reopening the world map does NOT re-run the
/// ViewModel constructor (`opens=2` with no `ctor #3`), so the pin list outlives every open. The
/// ctor fires on a WORLD ENTRY (travel / area transition) -- not on open, and not on a layer switch.
///
/// THIS IS ONE OF FOUR, AND WRITING ONLY IT IS WHY THE MAP DID NOT CHANGE. See
/// [`ROW_ICON_DESCRIPTOR_OFFSETS`].
pub const ROW_ICON_ID_OFFSET: usize = 0x248;

/// Every icon descriptor a pin row carries. The drawn frame is the first DWORD of ONE of them, and
/// the engine — not us — decides which.
///
/// `CS::WorldMapWarpPinData` (vftable `0x142ad8228`, `sizeof == 0x350`, which is the row stride)
/// embeds four 0x40-byte icon descriptors, seeded by its constructor `0x14088b7b0` from four
/// separate `BonfireWarpParam` fields:
///
/// | offset  | seeded from             |
/// |---------|-------------------------|
/// | `0x248` | `iconId`                |
/// | `0x288` | `forbiddenIconId`       |
/// | `0x2c8` | `altIconId`             |
/// | `0x308` | `altForbiddenIconId`    |
///
/// `CS::WorldMapPinData::SetTo` (`0x14087ae20`) does not read any of them directly. It calls vtable
/// slot `0xc` — `0x14088bb60` for this class — which returns an INTERIOR POINTER to whichever
/// descriptor applies, and the drawn frame is that descriptor's first dword. The selection is:
///
/// ```text
///   alt = FUN_140d25b30(this + 0x238, 0)      // over the row's BonfireWarpParam event flags
///   normal-vs-forbidden = *(u8*)(this + 0x348)
///   alt && flag -> 0x2c8 | alt && !flag -> 0x308 | !alt && flag -> 0x248 | else -> 0x288
/// ```
///
/// `FUN_140d25b30` walks slots 0..7 of the row's `BonfireWarpParam*` (`+0x240`), checking an event
/// flag id at `param+0x30/0x3c/0x48/0x54/0x60/0x6c/0x78/0x84` and returning 1 when the matching
/// byte at `param+0x90+i` is 1. Our synthetic param rows do not model those fields, so WHICH
/// descriptor the engine reads from is not something this DLL controls.
///
/// So every icon write goes to all four. A row we injected should show our marker whatever the
/// engine concludes about it, and overwriting the "forbidden" variants costs nothing: they exist to
/// grey out a warp the player has not unlocked, which is meaningless for a pin we invented.
///
/// Writing only `0x248` — and as a `u16` — is what produced the reported symptom: the log showed
/// correct tiers on every pin (`chosen=0 untouched=486 excluded=48`), the rows were rebuilt from
/// scratch by a fresh injection, and the map still looked identical.
pub const ROW_ICON_DESCRIPTOR_OFFSETS: [usize; 4] = [0x248, 0x288, 0x2c8, 0x308];

/// Set every icon descriptor on a row, so the drawn frame changes whichever one the engine picks.
///
/// The field is a full `u32`: the constructor stores the param's `u16` zero-extended
/// (`*(uint *)&this->field_0x248 = (uint)iconId`) and the consumer reads a whole dword
/// (`0x140749cf4: mov edx,[rdx]`). A `u16` write only ever worked by accident, because the upper
/// half happened to be zero.
///
/// # Safety
/// `row` must be a pin row this DLL owns, already verified against the param slab.
#[cfg(windows)]
pub(crate) unsafe fn write_row_icon(row: usize, icon: u16) {
    for offset in ROW_ICON_DESCRIPTOR_OFFSETS {
        // SAFETY: the caller established ownership; each offset is the first dword of a descriptor
        // the row constructor itself initialises, so all four are in-bounds of a 0x350-byte row.
        unsafe { *((row + offset) as *mut u32) = u32::from(icon) };
    }
}

/// The icon the engine would draw from the NORMAL descriptor, as a full dword.
///
/// Read back as `u32` to match the write. A `u16` read cannot tell a correctly-written row from one
/// whose upper half is stale, which would make the "did this stick?" comparison lie.
///
/// # Safety
/// `row` must be a readable pin row.
#[cfg(windows)]
pub(crate) unsafe fn read_row_icon(row: usize) -> Option<u32> {
    // Read as i32 and reinterpret: the field is a dword and `safe_read_u32` does not exist. The
    // sign is meaningless here -- an icon id is a small positive frame number, and a negative
    // reading is a value that will simply never equal the id being compared against.
    unsafe { er_game_base::mem::safe_read_i32(row + ROW_ICON_ID_OFFSET) }
        .map(|value| value.cast_unsigned())
}
/// Row field `+0x60` -- the MAP-LAYER visibility bitmask (bit 0 Lands Between, 1 underground,
/// 2 Shadow Lands). `UpdateVisible` clears the draw flag unless the active layer's bit is set, so
/// zero here is an invisible row on every layer.
pub const ROW_LAYER_MASK_OFFSET: usize = 0x60;
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

/// The ViewModel whose rows the last injection appended to, and the exact span it appended.
///
/// ONE SLOT, NOT A TABLE, AND IT CRASHED THE GAME TO LEARN WHY. There is exactly ONE ViewModel
/// alive at a time (`CSPopupMenu+0x250`, built in `STEP_MoveMap_Init`, freed in `~MoveMapStep` --
/// see [`crate::map_seams::WORLDMAP_VIEWMODEL_CTOR`]). A version of this module walked EVERY
/// recorded span on the belief that several views were live at once; three of the four spans then
/// pointed into freed MenuHeap, and a live run repainted 456 rows inside memory that had been handed
/// to something else. Freed heap pages stay MAPPED, so a fault-tolerant read succeeds and returns
/// whatever now lives there -- the read cannot be the safety net.
pub(crate) static LIVE_LIST_BEGIN: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LIVE_SPAN_BEGIN: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LIVE_SPAN_END: AtomicUsize = AtomicUsize::new(0);

/// Bounds of the leaked synthetic param-row slab, used as the row-ownership test.
///
/// A row of ours points at a param row inside a single contiguous allocation this module leaked and
/// never frees. Requiring `row+0x240` to land inside it is a FULL 64-BIT POINTER match against an
/// address only we hand out.
///
/// The previous test -- "is `row+0x08` in `[0x4000_0000, 0x5000_0000)`" -- sounded specific and was
/// worth about FOUR BITS: it accepts any word whose high byte is `0x40..0x4F`, i.e. roughly one
/// garbage word in sixteen. Against ~1500 stale rows that is ~90 false positives per pass, and the
/// live run produced far more than that. Calling it self-validating did not make it so.
pub(crate) fn param_slab_bounds() -> Option<(usize, usize)> {
    let begin = SHARED_PARAM_ROWS_PTR.load(Ordering::SeqCst);
    let len = SHARED_PARAM_ROWS_LEN.load(Ordering::SeqCst);
    if begin == 0 || len == 0 {
        return None;
    }
    Some((
        begin,
        begin + len * er_invasion_warp::param_row::SYNTHETIC_PARAM_ROW_LEN,
    ))
}

/// Whether `row` is genuinely one of ours: it points into our param slab AND carries our stamp.
///
/// Both, not either. The slab test is what makes it safe; the stamp test is what keeps the index
/// recoverable.
pub(crate) fn row_is_verifiably_ours(row: usize, slab: (usize, usize)) -> Option<usize> {
    let param = unsafe { er_game_base::mem::safe_read_usize(row + ROW_PARAM_POINTER_OFFSET) }?;
    if param < slab.0 || param >= slab.1 {
        return None;
    }
    if !(param - slab.0).is_multiple_of(er_invasion_warp::param_row::SYNTHETIC_PARAM_ROW_LEN) {
        return None;
    }
    let id = unsafe { er_game_base::mem::safe_read_i32(row + ROW_ID_OFFSET) }?;
    if !id_is_our_stamp(id) {
        return None;
    }
    Some((param - slab.0) / er_invasion_warp::param_row::SYNTHETIC_PARAM_ROW_LEN)
}

/// Whether a `+0x08` id is one this module stamped. Split out so the range rule is testable
/// without a live row to read.
///
/// NOT an ownership test on its own -- see [`param_slab_bounds`].
pub(crate) const fn id_is_our_stamp(id: i32) -> bool {
    let delta = id.wrapping_sub(INJECTED_ROW_ID_BASE);
    delta >= 0 && delta < STAMP_SPACE
}

/// The id to stamp into row `index` for restyle generation `generation`.
///
/// THE GENERATION IS WHAT MAKES A RESTYLE VISIBLE. The marker draw treats `row+0x08` as a
/// change-detection token: it rebinds a clip to a row -- and only then re-reads the icon and issues
/// `GotoAndStop(frame)` -- when its cached id for that sprite slot differs from the row's. So
/// rewriting `row+0x248` on its own changes a field nothing will look at again, and the clip keeps
/// drawing the frame it was bound with. That is why marking a location and reopening the map
/// changed nothing on screen: the icon byte WAS being rewritten, correctly, into a row the renderer
/// had already finished with.
///
/// Bumping the generation alongside the icon forces the mismatch, so the next draw rebinds and
/// picks the new frame up. Each row keeps a DISTINCT id within a generation, which the same draw
/// path requires for a different reason: duplicate ids make it skip the rebind and leave one pin's
/// icon on another pin's coordinates.
pub(crate) const fn stamped_row_id(generation: u32, index: usize) -> i32 {
    // Low 20 bits index (the largest list measured is ~1000 rows), next 8 the generation. The whole
    // field stays inside the positive space above the base, far from the engine's own counter.
    let packed = ((generation as usize & 0xff) << 20) | (index & 0x000f_ffff);
    INJECTED_ROW_ID_BASE.wrapping_add(packed as i32)
}

/// Size of the id space reserved for our stamps: 8 generation bits over 20 index bits.
const STAMP_SPACE: i32 = 1 << 28;

/// The dormant row span in the CURRENT ViewModel, and the next unclaimed slot within it.
pub(crate) static DORMANT_SPAN_BEGIN: AtomicUsize = AtomicUsize::new(0);
pub(crate) static DORMANT_SPAN_END: AtomicUsize = AtomicUsize::new(0);
pub(crate) static DORMANT_NEXT_SLOT: AtomicUsize = AtomicUsize::new(0);

/// Rows appended beyond the real pin set, held invisible until a later harvest claims one.
///
/// THIS IS THE WHOLE ANSWER TO "WITHOUT A WORLD ENTRY". The pin row vector can only be grown by
/// `FUN_140888aa0`, which has exactly two call sites, both inside the ViewModel constructor -- and
/// growing frees the old buffer, dangling every raw row pointer a `CS::WorldMapWarpData` holds.
/// So there is no safe moment to APPEND later, and 20 independent adversarial reviews of a
/// late-append design all converged on the same conclusion. Refusing to relocate is a property;
/// guarding a relocation is a check, and a check is what failed on 2026-08-05.
///
/// Instead every row that could ever be needed exists from the constructor, and a later change is a
/// plain in-place field write to a row that is already there. Nothing moves, so nothing dangles.
///
/// Sized from what a top-up must absorb: only what becomes resident BETWEEN two world entries.
/// Warping into a dungeon IS a world entry, so the case that needs headroom is the reported one --
/// the constructor runs during the loading screen, before the destination's `MsbResCap`s exist, and
/// that map's points arrive seconds later. The largest single map is 168 raw points; 512 rows
/// covers it several times over at 0x350 bytes each, i.e. ~424 KiB of MenuHeap against the
/// ~402 KiB the shipped 420 rows already occupy.
pub(crate) const DORMANT_ROW_COUNT: usize = 512;

/// Restyle generation, bumped every time the pin tiers actually change.
pub(crate) static RESTYLE_GENERATION: AtomicUsize = AtomicUsize::new(0);

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
    let mut hash = fnv1a64(b"");
    let mut mix = |value: u64| {
        hash = fnv1a64_mix(hash, value);
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

// There is deliberately NO "already injected" bookkeeping.
//
// (Section rationale for the injection strategy below -- NOT documentation for `sample_donor`.
// Written as `///` it was silently attached to that function as its doc comment.)
//
// Two earlier shapes both failed, and they failed for the same underlying reason:
//
// * a process-wide flag left every map view after the first with no pins;
// * keying on the ViewModel's `this` pointer fixed the *observed* case (two live instances at
//   different addresses) but is only as good as the assumption that a later ViewModel never
//   lands where an earlier one was. These objects are allocated out of a menu heap and freed
//   when the map closes, so a reopen reusing the address is not exotic -- it is the normal
//   behaviour of a size-bucketed allocator. Under that dedupe, the reopen is silently skipped
//   and the map is bare, which is precisely the reported symptom.
//
// Injection is idempotent-by-construction instead: the ctor builds a FRESH row list every time
// it runs (measured -- `rows=420` on both observed instances, never 785), so "has this list
// already got our pins" is answerable from the list itself and the answer is always "no" at the
// ctor epilogue. Nothing needs to be remembered between calls, so nothing can be remembered
// WRONG.

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
pub(crate) unsafe fn project_to_map(
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

/// Every legacy dungeon this ViewModel's converters can project, deduped across converters.
///
/// Each `WorldMapAreaConverter` owns its own legacy table, and the base-game and DLC converters
/// carry different dungeons, so all of them are walked and the union taken. A converter whose
/// table is unreadable contributes nothing rather than failing the rest.
///
/// # Safety
/// Game thread; `view_model` live.
#[cfg(windows)]
#[must_use]
unsafe fn legacy_map_regions_for_view(
    view_model: usize,
) -> Vec<er_invasion_warp::legacy_map_regions::LegacyMapRegion> {
    let Some(count) =
        (unsafe { er_game_base::mem::safe_read_usize(view_model + AREA_CONVERTER_COUNT_OFFSET) })
    else {
        return Vec::new();
    };
    // Same bound as `project_to_map`: the field is a `DLFixedVector<_, 8>`.
    let count = count.min(8);
    let mut regions = Vec::new();
    let mut claimed = 0usize;
    for index in 0..count {
        let converter = view_model + AREA_CONVERTERS_OFFSET + index * AREA_CONVERTER_STRIDE;
        let walked = unsafe {
            er_invasion_warp::legacy_map_regions::legacy_regions_for_converter(converter)
        };
        // The container keeps its own count, so the walk can be checked against the engine
        // rather than against nothing. Bounded by the same guard the walk uses, so a garbage
        // read cannot turn into a huge claimed figure.
        let says = unsafe {
            er_invasion_warp::legacy_map_regions::legacy_entry_count_for_converter(converter)
        }
        .filter(|n| *n <= er_invasion_warp::legacy_map_regions::MAX_TREE_NODES)
        .unwrap_or(0);
        if says != walked.len() {
            crate::standalone_log(format_args!(
                "map-inject: legacy converter #{index} says it holds {says} entries but the walk \
                 collected {} -- the traversal is losing entries, so some dungeons will have no \
                 marker",
                walked.len()
            ));
        }
        claimed += says;
        regions.extend(walked);
    }
    regions.sort_by_key(|region| region.block.raw());
    regions.dedup_by_key(|region| region.block.raw());
    crate::standalone_log(format_args!(
        "map-inject: legacy converter walk: {} converter(s), {claimed} entries claimed, {} \
         distinct block(s) after dedup",
        count,
        regions.len()
    ));
    regions
}

#[cfg(not(windows))]
#[must_use]
unsafe fn legacy_map_regions_for_view(
    _view_model: usize,
) -> Vec<er_invasion_warp::legacy_map_regions::LegacyMapRegion> {
    Vec::new()
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
pub(crate) fn layer_bit_for_converter(
    base: usize,
    converter_index: usize,
    block_area: u8,
) -> Option<u8> {
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
/// `-1` IS NOT A HARMLESS "NO NAME". A pin whose eight label text ids are ALL negative is not
/// drawn at all: `CS::WorldMapPinData::UpdateVisible` (0x14087afa0) computes the clip's visible
/// flag at `row+0x0c` as `A && B && C && D`, and for a warp pin `D` reduces to
/// `FUN_14088bcd0` -- a loop over the 8 labels that returns false unless some
/// `param+0x30+12i >= 0`. `SetTo` then passes `row+0x0c` straight to the clip. So a nameless pin is
/// an INVISIBLE pin, and the comment that used to sit here ("only -1 produces an empty label") was
/// describing a label that never gets the chance to be empty.
///
/// This is why legacy dungeons were the family that lost icons. The search below is area-locked,
/// and a legacy block's area byte is its own (10/11/12/13/15/28/30..39/...), so its candidate pool
/// is only that area's warpable graces. An area with no grace row carrying label kind 0 and a
/// positive text id yields `-1` for EVERY pin in EVERY dungeon of that area -- all of them
/// invisible, all of them counted as placed.
///
/// So the area lock is now a PREFERENCE, not a requirement. Its original reason -- keeping a base
/// pin from borrowing a DLC name whose coordinates live in another frame -- does not apply to a
/// legacy pin at all: by the time it is projected, `ConvertLegacyDungeonPositionToOverworldPositionForMap`
/// has already rebased it into the 60/61 overworld frame, so the nearest row in THAT frame is the
/// right neighbour to take a name from. Same area first, then anywhere, then `-1`.
///
/// A fallback id is still never invented: an id that resolves in no FMG renders the literal
/// `?PlaceName?` on the pin, so only a real shipped row's id is ever used.
///
/// # Safety
/// Game thread; `begin` must point at the first constructed row. Every read is fault-tolerant.
#[cfg(windows)]
pub(crate) unsafe fn nearest_place_name_text_id(
    begin: usize,
    existing_rows: usize,
    area: u8,
    coords: MapCoordinates,
) -> i32 {
    unsafe { nearest_place_name_in_area(begin, existing_rows, Some(area), coords) }
        .or_else(|| unsafe { nearest_place_name_in_area(begin, existing_rows, None, coords) })
        .unwrap_or(-1)
}

/// Nearest shipped `PlaceName` label, restricted to `area` when it is `Some`.
#[cfg(windows)]
unsafe fn nearest_place_name_in_area(
    begin: usize,
    existing_rows: usize,
    area: Option<u8>,
    coords: MapCoordinates,
) -> Option<i32> {
    use er_invasion_warp::param_row::{PARAM_LABEL_KIND_BASE, PARAM_LABEL_TEXT_ID_BASE};

    let mut best_text_id = None;
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
        if let Some(area) = area
            && unsafe { er_game_base::mem::safe_read_u8(param + PARAM_AREA_NO_OFFSET) }
                != Some(area)
        {
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
            best_text_id = Some(text_id);
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
        let dimmed = er_invasion_warp::warp::invasion_attempt_in_flight();
        crate::standalone_log(format_args!(
            "map-inject: shipped rows use icon frames {seen:?}; invasion pins will use frames \
             chosen={} untouched={} excluded={} (markers installed: {red_installed}, invasion \
             attempt in flight: {dimmed} -- pins are drawn DIMMED and refuse to warp while it is)",
            er_invasion_warp::param_row::invasion_pin_icon_id_for(
                er_invasion_warp::param_row::PinAppearance::Chosen,
                red_installed,
                dimmed
            ),
            er_invasion_warp::param_row::invasion_pin_icon_id_for(
                er_invasion_warp::param_row::PinAppearance::Eligible,
                red_installed,
                dimmed
            ),
            er_invasion_warp::param_row::invasion_pin_icon_id_for(
                er_invasion_warp::param_row::PinAppearance::Rejected,
                red_installed,
                dimmed
            ),
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
    // The `.aip` table covers areas 60 and 61 only, so on its own it can never put a marker in
    // Leyndell, Stormveil, Farum Azula, the Haligtree or any cave/catacomb/tunnel. Those maps
    // carry their invasion spawns as MSB `InvasionPoint` regions instead, which is the branch
    // `CSBreakInPointManager` takes when `PlayRegionParam.isAutoIntrudePoint` is clear.
    //
    // Those regions are per-map and are evicted with the map, so there is no moment at which all
    // of them are readable. Coverage therefore ACCUMULATES: whatever is resident now is folded in
    // and remembered, and a map contributes from the first time the player has been near it.
    let (msb_points_now, msb_blocks_now) = unsafe { refresh_msb_catalog() };
    let mut targets = InvasionRowRegistry::from_catalog(&catalog, PinGranularity::PerBlock)
        .targets()
        .to_vec();
    let aip_pins = targets.len();
    // The two sources OVERLAP. `.aip` is areas 60/61 only, but the MSB harvest reads whatever is
    // resident -- which includes the overworld blocks the player is standing in. Both sides emit one
    // representative per block, so an m60 block present in both would stack two markers on the same
    // spot, and the second would be indistinguishable from the first in the UI while carrying a
    // different synthetic entity id. Let the `.aip` table win where it has an entry: it is the
    // table the engine's own auto-invasion path uses for those areas.
    //
    // THE SUPPRESSION SET IS RESTRICTED TO NON-LEGACY AREAS. It is keyed on BLOCK, but the MSB side
    // it filters is keyed per POINT for a legacy dungeon -- so one `.aip` entry for a legacy block
    // would delete that entire dungeon's per-point set and leave a single representative behind.
    // That is precisely the "warped into the Haligtree and found one marker where there should have
    // been dozens" defect the granularity fix was written to end, reachable again through the other
    // source. The shipped table is areas 60/61 only, but `collect_invasion_warp_catalog` reads
    // whatever is LOADED and Seamless Co-op rewrites that table at runtime -- which is why the
    // digest check above exists. Deduping overworld blocks is all this was ever for.
    let aip_blocks: std::collections::BTreeSet<u32> = targets
        .iter()
        .map(|t| t.block.raw())
        .filter(|raw| !block_area_is_legacy(*raw))
        .collect();
    let msb_targets = msb_block_targets();
    let msb_offered = msb_targets.len();
    targets.extend(
        msb_targets
            .into_iter()
            .filter(|t| !aip_blocks.contains(&t.block.raw())),
    );
    let msb_pins = targets.len() - aip_pins;
    // A legacy dungeon the player has NEVER ENTERED contributes nothing above: its invasion
    // points are in its MSB, and an MSB is only readable while its map is resident. That used to
    // be the end of it -- no marker for Leyndell, Farum Azula, the Haligtree or any catacomb
    // until the player had physically walked there.
    //
    // It does not have to be. The world map already knows where every legacy dungeon sits: the
    // `WorldMapLegacyConverter` tree carries one entry per legacy block with the map-space origin
    // the converter adds to. And a warp to such a block needs no coordinate, because
    // `MoveMapStep` resolves the destination's own spawn after the load. So a block we know
    // NOTHING about the inside of is still both drawable and reachable.
    //
    // These are placed at the block origin, which the engine's own converter turns into the
    // dungeon's centre on the map. They are superseded the moment the real points arrive: the
    // filter below drops any block the precise sources already cover.
    let covered: std::collections::BTreeSet<u32> = targets.iter().map(|t| t.block.raw()).collect();
    let legacy_regions = unsafe { legacy_map_regions_for_view(view_model) };
    let legacy_offered = legacy_regions.len();
    let mut already_precise = 0usize;
    let mut read_and_empty = 0usize;
    let provisional: Vec<_> = legacy_regions
        .into_iter()
        .filter(|region| {
            if covered.contains(&region.block.raw()) {
                already_precise += 1;
                return false;
            }
            // Visited, read, and it genuinely has no invasion points. A marker here would promise
            // a spawn that does not exist, so retract it rather than leave it standing.
            if msb_has_observed(region.block) {
                read_and_empty += 1;
                return false;
            }
            true
        })
        .map(|region| {
            er_invasion_warp::invasion_warp::InvasionWarpTarget::provisional(region.block)
        })
        .collect();
    let provisional_pins = provisional.len();
    targets.extend(provisional);
    crate::standalone_log(format_args!(
        "map-inject: legacy-dungeon table: {legacy_offered} block(s) known to the world map's \
         legacy converter -> {provisional_pins} whole-dungeon marker(s) for dungeons not yet \
         entered (warping to one needs no coordinate: the engine resolves that map's own spawn \
         after the load), {already_precise} superseded by precise points, {read_and_empty} \
         visited and found to have no invasion points at all. legacy_offered=0 means the \
         converter tree was unreadable and NO dungeon marker was placed."
    ));
    if msb_offered != msb_pins {
        crate::standalone_log(format_args!(
            "map-inject: dropped {} MSB representative(s) whose block already has an .aip pin (no \
             double markers on overworld blocks)",
            msb_offered - msb_pins
        ));
    }
    crate::standalone_log(format_args!(
        "map-inject: pin sources: {aip_pins} from the .aip table (areas 60/61 only) + {msb_pins} \
         from MSB InvasionPoint regions ({msb_points_now} points across {msb_blocks_now} maps \
         seen so far this session -- legacy dungeons, caves and catacombs have no .aip entries at \
         all, so this is the ONLY source that can mark them)"
    ));
    let fresh = InvasionRowRegistry::from_targets(targets);
    // The param rows carry a per-location ICON, so the cache that serves them has to notice the
    // user's marks changing -- not just the spawn table.
    //
    // This is the bug that made the map look frozen. `catalog_signature` hashes blocks, point
    // indices and positions; marking a location changes none of those, so the signature matched,
    // the leaked rows were reused verbatim, and every reopen served icon ids computed at the FIRST
    // injection. Measured live: three marker frames provably installed, four re-injections, zero
    // visible change. The pin COUNT did change across those runs (467 -> 500 -> 587) precisely
    // because those were catalog changes, which is what made the cache look like it was working.
    let signature =
        catalog_signature(&fresh) ^ crate::local_invasion_filter::pin_choice_signature();
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
        // PRE-SIZED WITH DORMANT HEADROOM, and never grown again.
        //
        // Every live row's `+0x240` points into this slab, and `row_is_verifiably_ours` -- the
        // ownership test that exists because the last crash happened without one -- is a 64-bit
        // containment check against its bounds. A later `Vec` push and re-leak would REALLOCATE,
        // leaving every existing row pointing at freed Rust heap and making the ownership test
        // reject every row it should accept. So the headroom is allocated here, once, at the only
        // moment anything may move.
        let mut rows: Vec<[u8; SYNTHETIC_PARAM_ROW_LEN]> =
            Vec::with_capacity(wanted + DORMANT_ROW_COUNT);
        let (mut tier_chosen, mut tier_untouched, mut tier_excluded) = (0_usize, 0, 0);
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
            // Keep the name. It is resolved here from the shipped rows, which only exist while the
            // map is being built; the local-invasion filter needs it much later, when a match
            // arrives.
            if let Some(target) = registry.targets().get(index) {
                record_place_name(target.block.raw(), place_name_text_id);
            }
            // Whether the user chose, excluded or ignored this location -- a property of the
            // LOCATION, not of where they are standing. Asked at injection time because that is
            // when the row is built; the map re-injects on every open, so marking a place and
            // reopening the map shows the new tier.
            let appearance = crate::local_invasion_filter::pin_appearance_for(
                registry
                    .targets()
                    .get(index)
                    .map(|target| target.block.raw()),
            );
            match appearance {
                er_invasion_warp::param_row::PinAppearance::Chosen => tier_chosen += 1,
                er_invasion_warp::param_row::PinAppearance::Eligible => tier_untouched += 1,
                er_invasion_warp::param_row::PinAppearance::Rejected => tier_excluded += 1,
            }
            rows.push(
                SyntheticParamSpec {
                    entity_id,
                    subcategory_id: donor.subcategory_id,
                    // Deliberately NOT the donor's icon: the donor is a grace, and the id is a
                    // GFx frame number, so copying it draws a Site of Grace.
                    icon_id: er_invasion_warp::param_row::invasion_pin_icon_id_for(
                        appearance,
                        crate::map_gfx::red_pin_frame_installed(),
                        er_invasion_warp::warp::invasion_attempt_in_flight(),
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
            // UNNAMED IS UNDRAWN, not "drawn without a caption": `UpdateVisible`'s label term is a
            // hard gate on the clip's visible flag.
            //
            // But this count is NOT the oracle, and publishing it as one read 12 on every injection
            // for a reason that had nothing to do with drawing. A param row is built for every
            // TARGET, including targets that no converter will place; an unprojected target has no
            // coordinates to name itself from, so it gets -1 -- and is then dropped before the
            // append. Counting unnamed rows here therefore counts pins that never existed. The
            // oracle is published after the append instead, over the rows that actually landed.
            let unnamed = rows.len() - named;
            crate::standalone_log(format_args!(
                "map-inject: named {named}/{} param rows from the nearest shipped warp row. {unnamed} \
                 carry -1 on all eight labels; those are the targets no converter placed, and they \
                 are dropped rather than appended. The count that matters is the undrawable-pin \
                 oracle below, which is measured over APPENDED rows.",
                rows.len()
            ));
        }
        // The dormant entries. Built here so the slab's length is final before it is leaked --
        // `into_boxed_slice` shrinks capacity to length, so a slab built with `wanted` entries has
        // EXACTLY ZERO spare no matter what capacity was reserved, and a top-up that needed one
        // would refuse forever without ever saying why.
        //
        // Their param carries no layer bit and no label, which is belt AND braces: `UpdateVisible`
        // clears the draw flag when the row's layer mask misses the active map layer, and again
        // when no label has a non-negative text id. A dormant row is invisible on both counts until
        // it is claimed.
        for _ in 0..DORMANT_ROW_COUNT {
            rows.push(
                SyntheticParamSpec {
                    entity_id: 0,
                    subcategory_id: donor.subcategory_id,
                    icon_id: er_invasion_warp::param_row::invasion_pin_icon_id_for(
                        er_invasion_warp::param_row::PinAppearance::Eligible,
                        crate::map_gfx::red_pin_frame_installed(),
                        er_invasion_warp::warp::invasion_attempt_in_flight(),
                    ),
                    category_bits: 0,
                    place_name_text_id: -1,
                }
                .to_row_bytes(),
            );
        }
        let leaked: &'static [[u8; SYNTHETIC_PARAM_ROW_LEN]] = Box::leak(rows.into_boxed_slice());
        crate::local_invasion_filter::log_pin_tier_tally(
            tier_chosen,
            tier_untouched,
            tier_excluded,
        );
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
    // The re-stamp is PER ROW, and that is the whole point of it now.
    //
    // It used to write one icon id over every row, which silently defeated the tiers: they were
    // computed correctly at build time -- a live run logged `chosen=3` and then `chosen=96` as
    // marks were added -- and then flattened here to frame 300 for every pin, so the map never
    // changed no matter what the user marked. A single-icon re-stamp cannot coexist with a
    // per-location icon; it has to recompute the same decision the build did.
    {
        let installed = crate::map_gfx::red_pin_frame_installed();
        // Re-stamp when EITHER input changes: whether the marker frames are in front of Scaleform
        // (the late-swap rescue this block was written for), or the user's lists (the tiers).
        let stamp_signature = crate::local_invasion_filter::pin_choice_signature()
            ^ usize::from(installed).wrapping_mul(0x9e37_79b9);
        let stamped = SHARED_PARAM_ROWS_ICON.swap(stamp_signature, Ordering::SeqCst);
        if stamped != stamp_signature {
            // SAFETY: this slice was leaked by this function and is never freed; the game only
            // ever reads it, and this runs on the game thread inside the ctor.
            let rows: &mut [[u8; SYNTHETIC_PARAM_ROW_LEN]] = unsafe {
                core::slice::from_raw_parts_mut(
                    SHARED_PARAM_ROWS_PTR.load(Ordering::SeqCst)
                        as *mut [u8; SYNTHETIC_PARAM_ROW_LEN],
                    SHARED_PARAM_ROWS_LEN.load(Ordering::SeqCst),
                )
            };
            let (mut chosen, mut untouched, mut excluded) = (0_usize, 0_usize, 0_usize);
            for (index, row) in rows.iter_mut().enumerate() {
                let appearance = crate::local_invasion_filter::pin_appearance_for(
                    registry
                        .targets()
                        .get(index)
                        .map(|target| target.block.raw()),
                );
                match appearance {
                    er_invasion_warp::param_row::PinAppearance::Chosen => chosen += 1,
                    er_invasion_warp::param_row::PinAppearance::Eligible => untouched += 1,
                    er_invasion_warp::param_row::PinAppearance::Rejected => excluded += 1,
                }
                let desired = er_invasion_warp::param_row::invasion_pin_icon_id_for(
                    appearance,
                    installed,
                    er_invasion_warp::warp::invasion_attempt_in_flight(),
                );
                // ALL FOUR icon slots, via the one stamper. Writing only `+0x1c` here -- which is
                // what this line used to do -- left the other three holding the icon from the FIRST
                // build, and the engine reads whichever descriptor its own event-flag predicate
                // selects. That is why marking a location changed every count in the log and
                // nothing on the map.
                er_invasion_warp::param_row::stamp_icon_id(row, desired);
            }
            crate::standalone_log(format_args!(
                "map-inject: re-stamped {} param rows -- chosen={chosen} untouched={untouched} \
                 excluded={excluded} (markers_installed={installed})",
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
    // Reserve for the dormant rows in the SAME call. This is the only relocation that will ever
    // happen to this buffer, and it happens at the one moment it is provably safe: no map dialog
    // exists yet, so no `CS::WorldMapWarpData+0x08` raw row pointer can be left dangling.
    unsafe { reserve(vector, wanted + DORMANT_ROW_COUNT) };

    // Re-read: the reserve moved the buffer.
    let Some(after_reserve) = (unsafe { read_pin_list(view_model) }) else {
        INJECTIONS_SKIPPED.fetch_add(1, Ordering::SeqCst);
        crate::standalone_log(format_args!(
            "map-inject: pin list unreadable after reserve; NOT appending"
        ));
        return;
    };
    if after_reserve.spare_rows() < wanted + DORMANT_ROW_COUNT {
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
    // How many pins were accepted by a converter belonging to a DIFFERENT area than the
    // target's own -- those land in the wrong map's coordinate space.
    let mut cross_area_projections = 0_usize;
    let mut cross_area_trace = 4_usize;
    let mut area_trace = 4_usize;
    // Legacy-dungeon accounting, kept separate from the `.aip` totals.
    //
    // Areas other than 60/61 can only come from the MSB `InvasionPoint` source, and they are the
    // whole point of that source. Folding them into one `unplaceable` number would make the two
    // failures indistinguishable: "the dungeon's MSB was never read" and "it was read and the
    // converter refused to place it" need completely different fixes, and the second one is
    // specifically a claim about `WorldMapAreaConverter::legacyConverter` being null.
    let mut legacy_seen = 0_usize;
    let mut legacy_placed = 0_usize;
    // WHICH BLOCKS WERE REFUSED, not the first six refusals. A per-refusal line budget was written
    // when a legacy target meant one whole dungeon; now that legacy targets are PER POINT, a single
    // unplaceable dungeon spends the entire budget on its own first six points and every other
    // refused dungeon goes unnamed -- while "which dungeons are missing" is the exact question the
    // symptom asks. A set of block ids answers it in one line and is bounded by the ~245-entry
    // legacy converter tree.
    let mut refused_blocks: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    let mut refused_points = 0_usize;
    // Rows abandoned by the append itself rather than refused by a converter. Counted so the
    // summary can assert `injected + unplaceable + aborted == wanted`; without it a mid-append read
    // failure silently drops an arbitrary SUFFIX of the set -- and the targets are ordered .aip
    // first, then legacy points, then provisional markers, so the suffix lost is precisely the
    // legacy pins.
    let mut aborted = 0_usize;
    // Appended rows that cannot draw for want of a label. MUST be zero.
    let mut undrawable = 0_usize;
    for (index, target) in registry.targets().iter().enumerate() {
        // Reuse the projection computed above rather than re-running it: the layer bit and the
        // coordinate must come from the SAME converter decision, and projecting twice invites
        // them to disagree as well as doubling 365 native calls inside a world load.
        let is_legacy = !matches!(
            block_area(target.block.raw()),
            er_invasion_warp::param_row::AREA_SHADOW_LANDS | 60
        );
        if is_legacy {
            legacy_seen += 1;
        }
        let Some((coords, converter_index, converter_area)) =
            projections.get(index).copied().flatten()
        else {
            unplaceable += 1;
            if is_legacy {
                refused_blocks.insert(target.block.raw());
                refused_points += 1;
            }
            continue;
        };
        // A LEGACY PIN ACCEPTED BY A 60/61 CONVERTER IS CORRECT, NOT CROSS-AREA (user-observed
        // 2026-08-04, and it reverses a "fix" made earlier the same day). `ConvertMsbCoordsToMapCoords`
        // calls `ConvertLegacyDungeonPositionToOverworldPositionForMap` FIRST and area-matches the
        // REMAPPED block, so a dungeon necessarily arrives through the ordinary overworld converter
        // and its converter area necessarily differs from the block's own. Requiring them to be equal
        // made the counter unsatisfiable for exactly the maps it exists to measure: the Haligtree pin
        // logged `m15_00_00_00 (area 15) accepted by converter #0 (area 60)` and reported 0/2 placed,
        // while the user watched it render on the Haligtree warp point.
        // NOTE: `legacy_placed` is NOT incremented here. Being accepted by a converter is not being
        // placed -- the layer-bit test below and the append itself can both still drop this pin, and
        // counting it as placed at this point made the oracle structurally unable to report the very
        // failure it exists to catch (it read N/N placed while N rows were dropped). It is counted
        // next to `injected` instead.
        //
        // A pin whose converter carries no layer entry can never be drawn on any map, so it is
        // dropped rather than appended with a zero mask that would make it permanently invisible
        // while still occupying a row and a clip-pool slot.
        if layer_bit_for_converter(base, converter_index, block_area(target.block.raw())).is_none()
        {
            unplaceable += 1;
            if is_legacy {
                refused_blocks.insert(target.block.raw());
                refused_points += 1;
            }
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
        // Cross-area is only meaningful for a pin that was NOT remapped: an area-60/61 target
        // accepted by a converter of the other area really is drawn in the wrong space. A legacy
        // block reaching a 60/61 converter went through the legacy remap and is where it belongs.
        let legacy_remap_expected = is_legacy
            && matches!(
                converter_area,
                60 | er_invasion_warp::param_row::AREA_SHADOW_LANDS
            );
        if converter_area != target_area && !legacy_remap_expected {
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
        // `param_rows.get`, not `param_rows[index]`. The clamp above (`wanted.min(param_rows.len())`)
        // advertises "injecting only what is backed", but this loop iterates `registry.targets()`,
        // not `wanted` -- so the clamp only ever shrank the reserve and the log line, and the case
        // it claims to handle would have been an out-of-bounds panic on the game thread inside a
        // hooked engine constructor. Enforce it where the indexing actually happens.
        let Some(param_row) = param_rows.get(index) else {
            aborted += registry.targets().len() - index;
            break;
        };
        let lookup = BonfireLookupResult {
            param_id: registry.entity_id_at(index).unwrap_or(0),
            pad: 0,
            param_row: param_row.as_ptr(),
        };
        let mut temp = TempPinRow([0_u8; PIN_ROW_STRIDE]);
        unsafe { make_row(temp.0.as_mut_ptr(), &raw const coords, &raw const lookup) };

        // Re-read `end` every iteration and write it back, exactly as the ctor's own append does.
        let Some(end) = (unsafe { er_game_base::mem::safe_read_usize(vector + VECTOR_END_OFFSET) })
        else {
            unsafe { dtor(temp.0.as_mut_ptr()) };
            // Everything from here on is lost, and the ordering makes that the legacy and
            // provisional pins specifically. Count it or the summary silently disagrees with itself.
            aborted += registry.targets().len() - index;
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
            //
            // Stamped at the CURRENT restyle generation so a later restyle can bump it and force
            // the same re-bind deliberately -- see `stamped_row_id`.
            unsafe {
                *((end + ROW_ID_OFFSET) as *mut i32) =
                    stamped_row_id(RESTYLE_GENERATION.load(Ordering::SeqCst) as u32, index);
            }
            unsafe { *((vector + VECTOR_END_OFFSET) as *mut usize) = end + PIN_ROW_STRIDE };
            injected += 1;
            // PLACED MEANS APPENDED. Counting it earlier is what let the oracle report 100%
            // placement for a set that had rows dropped after acceptance.
            if is_legacy {
                legacy_placed += 1;
            }
            // An APPENDED row with no non-negative label occupies a list slot and never draws.
            // Measured here, over rows that actually landed, so the number cannot be inflated by
            // targets that were dropped before they became pins.
            {
                use er_invasion_warp::param_row::PARAM_LABEL_TEXT_ID_BASE;
                if i32::from_le_bytes(
                    param_row[PARAM_LABEL_TEXT_ID_BASE..PARAM_LABEL_TEXT_ID_BASE + 4]
                        .try_into()
                        .unwrap_or([0; 4]),
                ) <= 0
                {
                    undrawable += 1;
                }
            }
        } else {
            aborted += 1;
        }
        // MUST use the engine dtor: the temp owns its MenuString and up to 8 label DLStrings.
        unsafe { dtor(temp.0.as_mut_ptr()) };
    }

    // APPEND THE DORMANT ROWS. Same engine ctor/copy-ctor path as a real pin, into capacity the
    // single reserve above already claimed, so `end` only advances and the buffer never moves.
    // Their param carries no layer bit and no label, so `UpdateVisible` leaves the draw flag clear
    // and none of them is visible until a top-up claims it.
    let mut dormant = 0_usize;
    let dormant_first = unsafe { er_game_base::mem::safe_read_usize(vector + VECTOR_END_OFFSET) };
    for slot in 0..DORMANT_ROW_COUNT {
        let Some(param_row) = param_rows.get(wanted + slot) else {
            break;
        };
        let lookup = BonfireLookupResult {
            param_id: 0,
            pad: 0,
            param_row: param_row.as_ptr(),
        };
        // Position is irrelevant while the row is invisible, and a top-up overwrites `+0x10`
        // before it makes the row visible. Zero is used rather than a real coordinate so a row that
        // somehow drew without being claimed would be obviously wrong rather than subtly misplaced.
        let coords = MapCoordinates { x: 0.0, z: 0.0 };
        let mut temp = TempPinRow([0_u8; PIN_ROW_STRIDE]);
        unsafe { make_row(temp.0.as_mut_ptr(), &raw const coords, &raw const lookup) };
        let Some(end) = (unsafe { er_game_base::mem::safe_read_usize(vector + VECTOR_END_OFFSET) })
        else {
            unsafe { dtor(temp.0.as_mut_ptr()) };
            break;
        };
        if end != 0 {
            unsafe { copy_ctor(end as *mut u8, temp.0.as_ptr()) };
            unsafe {
                *((end + ROW_ID_OFFSET) as *mut i32) = stamped_row_id(
                    RESTYLE_GENERATION.load(Ordering::SeqCst) as u32,
                    wanted + slot,
                );
            }
            unsafe { *((vector + VECTOR_END_OFFSET) as *mut usize) = end + PIN_ROW_STRIDE };
            dormant += 1;
        }
        unsafe { dtor(temp.0.as_mut_ptr()) };
    }
    // The previous world entry's top-up claims describe rows that no longer exist, and the registry
    // just rebuilt covers those blocks itself. Dropping them here keeps the claim table and the slot
    // counter describing the same set of rows.
    clear_top_up_targets();
    if let Some(first) = dormant_first
        && dormant > 0
    {
        DORMANT_SPAN_BEGIN.store(first, Ordering::SeqCst);
        DORMANT_SPAN_END.store(first + dormant * PIN_ROW_STRIDE, Ordering::SeqCst);
        DORMANT_NEXT_SLOT.store(0, Ordering::SeqCst);
        record_injected_span(first, first + dormant * PIN_ROW_STRIDE);
    } else {
        DORMANT_SPAN_BEGIN.store(0, Ordering::SeqCst);
        DORMANT_SPAN_END.store(0, Ordering::SeqCst);
    }
    crate::standalone_log(format_args!(
        "map-inject: reserved {dormant}/{DORMANT_ROW_COUNT} dormant row(s) for later top-ups. These \
         are appended here because the list can only GROW inside this constructor -- a later append \
         would relocate the buffer and dangle every raw row pointer a fast-travel dialog holds. A \
         dungeon harvested mid-session claims one of these in place instead."
    ));

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
        // The CURRENT span, kept separately from the wrap-around table. The table exists for the
        // filter observer's counters, where a stale entry costs a wrong tally; the restyle WRITES,
        // where a stale entry costs the player their game.
        LIVE_LIST_BEGIN.store(final_geometry.begin, Ordering::SeqCst);
        LIVE_SPAN_BEGIN.store(first, Ordering::SeqCst);
        LIVE_SPAN_END.store(first + injected * PIN_ROW_STRIDE, Ordering::SeqCst);
        LIVE_VIEW_MODEL.store(view_model, Ordering::SeqCst);
    }
    crate::standalone_log(format_args!(
        "map-inject: layer split: area60(surface bit)={} area61(shadow-lands bit)={}; converter \
         usage={per_converter:?} -- a pin is drawn ONLY on the layer whose bit it carries, \
         because its single coordinate is only valid in that converter's space",
        per_area[0], per_area[1]
    ));
    // The legacy line is emitted unconditionally, INCLUDING when the count is zero. A silent
    // absence would read as "legacy dungeons are handled" when the truth is "no dungeon map has
    // been resident yet, so none were even offered".
    er_invasion_warp::oracles::publish_legacy_pin_oracles(legacy_seen, legacy_placed);
    er_invasion_warp::oracles::publish_undrawable_pin_count(undrawable);
    if undrawable > 0 {
        crate::standalone_log(format_args!(
            "map-inject: {undrawable} APPENDED pin(s) carry -1 on all eight labels and therefore \
             CANNOT DRAW -- UpdateVisible gates the clip's visible flag on having some label with a \
             non-negative text id. These occupy a row and never appear. This must be zero."
        ));
    }
    crate::standalone_log(format_args!(
        "map-inject: legacy-dungeon pins: {legacy_placed}/{legacy_seen} placed on the map. These \
         are the ONLY markers possible for Leyndell, Stormveil, Farum Azula, the Haligtree and \
         every cave/catacomb/tunnel -- the .aip table has no entries outside areas 60 and 61. \
         seen=0 means no such map has been resident this session yet (coverage accumulates as \
         maps load); seen>0 with placed=0 means the converters refused them."
    ));
    // NAME EVERY REFUSED DUNGEON, once. `refused_points` and the size of this set answer different
    // questions -- 168 refused points can be one dungeon or eight -- and only the set can say WHICH
    // dungeon has no icon, which is the question the symptom actually asks.
    if !refused_blocks.is_empty() {
        let names: Vec<String> = refused_blocks
            .iter()
            .map(|raw| format!("{:#010x}", raw))
            .collect();
        crate::standalone_log(format_args!(
            "map-inject: {refused_points} legacy point(s) across {} distinct block(s) were placed \
             by NO converter: [{}]. ConvertMsbCoordsToMapCoords remaps a legacy block to overworld \
             space itself when its converter has a legacyConverter, and the lookup needs an EXACT \
             block-id key -- so refusal is per block and total, and a multi-block dungeon can lose \
             one whole sub-block while its sibling places fine.",
            refused_blocks.len(),
            names.join(", ")
        ));
    }
    let settled = unsafe { read_pin_list(view_model) };
    crate::standalone_log(format_args!(
        "map-inject: appended {injected} invasion pins ({unplaceable} unplaceable, {aborted} \
         abandoned mid-append, {cross_area_projections} CROSS-AREA (wrong map's coordinate space), \
         {wanted} wanted, {existing_rows} shipped rows before; accounted={} -- if that does not \
         equal `wanted`, pins were lost on a path with no counter) -> list now rows={} spare={} \
         plausible={} donor[row={} subcategory={} category_bits=0x{:x} icon={} label_text_id={}]",
        injected + unplaceable + aborted,
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

/// Invasion points harvested from the MSBs of maps that have been resident this session.
///
/// Session-scoped and only ever grown. It is NOT reset when the catalog signature changes: a mod
/// rewriting the `.aip` table says nothing about MSB region data, and throwing away coverage the
/// player has already walked past would make the surface worse for no reason.
static MSB_CATALOG: std::sync::Mutex<er_invasion_warp::msb_invasion_points::MsbInvasionCatalog> =
    std::sync::Mutex::new(er_invasion_warp::msb_invasion_points::MsbInvasionCatalog::new());

/// Read every resident map's `InvasionPoint` regions into [`MSB_CATALOG`].
///
/// Returns `(points known, maps read)` after the fold. Skips maps already read: the geometry is
/// static per map, so re-reading one is pure cost on the game thread during a map open.
///
/// # Safety
/// Game task thread, with the world up.
#[cfg(windows)]
pub(crate) unsafe fn refresh_msb_catalog() -> (usize, usize) {
    use er_invasion_warp::msb_invasion_points::{read_map_invasion_points, resident_blocks};
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
fn msb_has_observed(block: er_invasion_warp::invasion_warp::BlockKey) -> bool {
    let catalog = match MSB_CATALOG.lock() {
        Ok(catalog) => catalog,
        Err(poisoned) => poisoned.into_inner(),
    };
    catalog.has_observed(block)
}

#[cfg(not(windows))]
fn msb_has_observed(_block: er_invasion_warp::invasion_warp::BlockKey) -> bool {
    false
}

#[cfg(windows)]
fn msb_pending_block_count() -> usize {
    use er_invasion_warp::msb_invasion_points::resident_blocks;
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
    use er_invasion_warp::msb_invasion_points::resident_blocks;
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
#[cfg(windows)]
unsafe fn log_msb_cap_census() {
    use er_invasion_warp::msb_invasion_points::resident_blocks;
    let Ok(base) = er_game_base::mem::game_module_base() else {
        return;
    };
    let blocks = unsafe { resident_blocks() };
    let total = blocks.len();
    let non_null = blocks.iter().filter(|(_, cap)| *cap != 0).count();
    let live = blocks
        .iter()
        .filter(|(_, cap)| unsafe {
            er_invasion_warp::msb_invasion_points::msb_res_cap_looks_live(base, *cap)
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
        (Some(raw), None) => {
            format!("player block {raw:#010x} is NOT IN the world block list at all")
        }
        (Some(raw), Some(cap)) => {
            let live =
                unsafe { er_invasion_warp::msb_invasion_points::msb_res_cap_looks_live(base, cap) };
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
    unsafe { er_invasion_warp::warp::current_block_id(base) }
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
pub(crate) fn msb_block_targets() -> Vec<er_invasion_warp::invasion_warp::InvasionWarpTarget> {
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
            er_invasion_warp::invasion_warp::InvasionWarpTarget::new(
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
        let merged = er_invasion_warp::msb_invasion_points::merge_coincident_points(
            points,
            er_invasion_warp::msb_invasion_points::MARKER_MERGE_RADIUS_METRES,
        );
        legacy_merged += merged.len();
        targets.extend(merged.into_iter().map(|point| {
            er_invasion_warp::invasion_warp::InvasionWarpTarget::new(
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
            er_invasion_warp::msb_invasion_points::MARKER_MERGE_RADIUS_METRES
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
    area != 60 && area != er_invasion_warp::param_row::AREA_SHADOW_LANDS
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

/// The injected registry, leaked so the confirm hook can map a synthetic entity id back to its
/// target for the rest of the session. 0 until the injection runs.
pub(crate) static INJECTED_REGISTRY: AtomicUsize = AtomicUsize::new(0);

/// The last `(raw << 32 | merged)` point tally the merge report printed, so it prints once per
/// distinct harvest outcome instead of once per frame.
static MERGE_REPORTED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(u64::MAX);

/// Warp destinations for pins a LIVE top-up claimed, which the registry cannot describe.
///
/// # Why a second table exists at all
///
/// [`InvasionRowRegistry::target_for_entity_id`] is a dense index: entity id `BASE + i` means
/// "element `i`". A top-up claims a dormant row AFTER the registry was leaked, so the only ids left
/// to hand it are past the end -- and the lookup then MISSES. The confirm hook, correctly, refuses
/// to pass an unresolvable synthetic id to the native assembler (that is a loading-screen hang), so
/// without this table a topped-up marker would draw on the map and then do nothing when selected: a
/// pin that is visible but dead, which is worse than an absent one because it looks like a feature.
///
/// The id is stored EXPLICITLY rather than derived from a position, so it stays correct no matter
/// what the registry's length does afterwards. Cleared wherever the dormant span is re-established,
/// because that is the moment every claim it describes stops existing.
pub(crate) static TOP_UP_TARGETS: Mutex<
    Vec<(i32, er_invasion_warp::invasion_warp::InvasionWarpTarget)>,
> = Mutex::new(Vec::new());

/// Remember what a topped-up row's synthetic entity id must warp to.
pub(crate) fn record_top_up_target(
    entity_id: i32,
    target: er_invasion_warp::invasion_warp::InvasionWarpTarget,
) {
    let mut table = match TOP_UP_TARGETS.lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(slot) = table.iter_mut().find(|(id, _)| *id == entity_id) {
        slot.1 = target;
    } else {
        table.push((entity_id, target));
    }
}

/// The destination for a synthetic id the registry could not resolve, if a top-up claimed it.
pub(crate) fn top_up_target_for_entity_id(
    entity_id: i32,
) -> Option<er_invasion_warp::invasion_warp::InvasionWarpTarget> {
    let table = match TOP_UP_TARGETS.lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    };
    table
        .iter()
        .find(|(id, _)| *id == entity_id)
        .map(|(_, target)| *target)
}

/// Points a top-up examined and could not place, for THIS ViewModel.
///
/// All three refusal reasons -- no converter accepted the position, no map layer bit, no nearby
/// named place -- are decided by the converter set and shipped rows of the ViewModel currently on
/// screen, and neither changes while it lives. Remembering them lets the freshness test empty out,
/// so the function reaches its cheap early return instead of re-deriving the same refusal on every
/// frame. Cleared with the claims, because the next ViewModel gets its own answer.
static TOP_UP_REFUSED: Mutex<std::collections::BTreeSet<(u32, u32)>> =
    Mutex::new(std::collections::BTreeSet::new());

/// Remember that a point could not be placed on the live map.
pub(crate) fn record_top_up_refusal(block: u32, point_index: u32) {
    let mut refused = match TOP_UP_REFUSED.lock() {
        Ok(refused) => refused,
        Err(poisoned) => poisoned.into_inner(),
    };
    refused.insert((block, point_index));
}

/// The points already found unplaceable against the live map.
pub(crate) fn top_up_refused_points() -> std::collections::BTreeSet<(u32, u32)> {
    let refused = match TOP_UP_REFUSED.lock() {
        Ok(refused) => refused,
        Err(poisoned) => poisoned.into_inner(),
    };
    refused.clone()
}

/// The `(block, point_index)` pairs a top-up has already placed on the live map.
///
/// The registry is immutable once leaked, so a claim leaves no trace in it. Without this the
/// freshness test would keep reporting the same points as new on every single frame and burn a
/// dormant row each time -- 512 of them gone in about a second, and 512 duplicate markers stacked
/// on the map before they ran out.
pub(crate) fn top_up_claimed_points() -> std::collections::BTreeSet<(u32, u32)> {
    let table = match TOP_UP_TARGETS.lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    };
    table
        .iter()
        .map(|(_, target)| (target.block.raw(), target.point_index))
        .collect()
}

/// Forget every top-up claim. Called where the dormant span is re-established: the rows those ids
/// were written into no longer exist, and the fresh registry describes those blocks itself.
pub(crate) fn clear_top_up_targets() {
    let mut table = match TOP_UP_TARGETS.lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    };
    table.clear();
    let mut refused = match TOP_UP_REFUSED.lock() {
        Ok(refused) => refused,
        Err(poisoned) => poisoned.into_inner(),
    };
    refused.clear();
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
            crate::map_confirm::warp_job_assembler_hook as er_hook::UnionFn,
            &crate::map_confirm::ORIG_WARP_JOB_ASSEMBLER,
        )
    } {
        Ok(()) => {
            crate::standalone_log(format_args!(
                "map-hooks: intercepting {} @0x{address:x} -- selecting an invasion pin is now \
                 answered by us instead of handing a synthetic id to Lua_Warp",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn target(block: u32, point: u32) -> er_invasion_warp::invasion_warp::InvasionWarpTarget {
        er_invasion_warp::invasion_warp::InvasionWarpTarget {
            block: er_invasion_warp::invasion_warp::BlockKey::from_raw(block),
            point_index: point,
            position: [1.0, 2.0, 3.0],
            yaw: 0.0,
        }
    }

    /// A live top-up hands out ids the registry's dense index cannot describe. If those ids have no
    /// destination the confirm hook swallows the warp and the pin is visible but dead -- so the
    /// fallback table is the difference between a marker and a decoration.
    #[test]
    fn an_id_past_the_registrys_range_is_still_resolvable_after_a_top_up_records_it() {
        clear_top_up_targets();
        let id = er_invasion_warp::map_surface::INVASION_ENTITY_ID_BASE + 4_000;
        assert_eq!(top_up_target_for_entity_id(id), None);
        record_top_up_target(id, target(0x2800_0000, 7));
        assert_eq!(
            top_up_target_for_entity_id(id),
            Some(target(0x2800_0000, 7))
        );
        clear_top_up_targets();
    }

    /// Re-claiming an id must RETARGET it, not leave two answers where the first one wins.
    #[test]
    fn recording_the_same_id_twice_replaces_the_destination() {
        clear_top_up_targets();
        let id = er_invasion_warp::map_surface::INVASION_ENTITY_ID_BASE + 4_001;
        record_top_up_target(id, target(0x2800_0000, 1));
        record_top_up_target(id, target(0x2900_0000, 2));
        assert_eq!(
            top_up_target_for_entity_id(id),
            Some(target(0x2900_0000, 2))
        );
        clear_top_up_targets();
    }

    /// The claims describe rows in one ViewModel's dormant span. Once that span is re-established
    /// those rows are gone, and a stale answer would warp a NEW pin to an OLD destination.
    #[test]
    fn clearing_forgets_every_claim_so_a_rebuilt_span_cannot_inherit_a_stale_destination() {
        clear_top_up_targets();
        let id = er_invasion_warp::map_surface::INVASION_ENTITY_ID_BASE + 4_002;
        record_top_up_target(id, target(0x2800_0000, 3));
        clear_top_up_targets();
        assert_eq!(top_up_target_for_entity_id(id), None);
    }

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
    fn our_stamp_is_recognised_and_the_engines_own_pin_ids_are_not() {
        // The engine's counter starts at 0 and climbs a few hundred per map. None of that may be
        // mistaken for our stamp, or the restyle would write into shipped rows.
        for engine_id in [0_i32, 1, 419, 420, 1006, 12_345] {
            assert!(
                !id_is_our_stamp(engine_id),
                "engine pin id {engine_id} must not look like our stamp"
            );
        }
        // `-1` is the engine's wrap sentinel and is explicitly not our base.
        assert!(!id_is_our_stamp(-1));
        for generation in [0_u32, 1, 255] {
            for index in [0_usize, 1, 586, 1006] {
                assert!(
                    id_is_our_stamp(stamped_row_id(generation, index)),
                    "our own stamp at generation {generation} index {index} must be recognised"
                );
            }
        }
        // Just past the reserved space is rejected, so a garbage read cannot drift into the range.
        assert!(!id_is_our_stamp(
            INJECTED_ROW_ID_BASE.wrapping_add(STAMP_SPACE)
        ));
    }

    #[test]
    fn the_id_stamp_is_far_too_weak_to_be_an_ownership_test_on_its_own() {
        // THIS TEST EXISTS BECAUSE THE STAMP WAS USED AS ONE AND IT CRASHED THE GAME.
        //
        // `id_is_our_stamp` accepts every value whose high byte lands in 0x40..0x4F -- one word in
        // sixteen. A freed MenuHeap page stays MAPPED, so a fault-tolerant read of it succeeds and
        // returns whatever now lives there; roughly 6% of that garbage passes. Applied to ~1500
        // stale rows a live run repainted 456 of them inside memory belonging to other objects.
        //
        // Ownership is established by `row_is_verifiably_ours`, which requires the row's +0x240 to
        // point into our own leaked param slab -- a full 64-bit match on an address only we hand
        // out. The stamp's job is only to carry the row index.
        let accepted = (0..=u8::MAX)
            .filter(|high| id_is_our_stamp(i32::from_be_bytes([*high, 0x11, 0x22, 0x33])))
            .count();
        assert!(
            accepted >= 8,
            "the stamp was expected to be a weak, wide filter; if this ever tightens to a real \
             ownership test, say so explicitly rather than letting callers assume it"
        );
    }

    #[test]
    fn a_restyle_generation_changes_every_rows_id_while_keeping_rows_distinct() {
        // Both halves matter. The generation must change the id -- that is the whole mechanism that
        // forces the renderer to re-bind and pick up the new icon. And rows must stay distinct
        // WITHIN a generation, because duplicate ids make the draw skip the re-bind and leave one
        // pin's icon sitting on another pin's coordinates.
        let before: Vec<i32> = (0..1000).map(|index| stamped_row_id(3, index)).collect();
        let after: Vec<i32> = (0..1000).map(|index| stamped_row_id(4, index)).collect();
        for (index, (b, a)) in before.iter().zip(after.iter()).enumerate() {
            assert_ne!(b, a, "row {index} kept its id across a generation bump");
        }
        let distinct: std::collections::BTreeSet<i32> = after.iter().copied().collect();
        assert_eq!(
            distinct.len(),
            after.len(),
            "ids collided within a generation"
        );
    }

    #[test]
    fn the_index_survives_a_round_trip_through_the_stamp() {
        // The restyle recovers a row's index from its id rather than from its offset in the span.
        for generation in [0_u32, 7, 255] {
            for index in [0_usize, 1, 419, 1006] {
                let id = stamped_row_id(generation, index);
                let recovered = (id.wrapping_sub(INJECTED_ROW_ID_BASE) as usize) & 0x000f_ffff;
                assert_eq!(recovered, index, "generation {generation} lost the index");
            }
        }
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
