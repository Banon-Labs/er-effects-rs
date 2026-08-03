//! The world-map detours.
//!
//! # Why the first thing installed here only OBSERVES
//!
//! The injection seam is the `CS::WorldMapViewModel` constructor: the pin-row list at `+0x2d8`
//! is populated there and nowhere else, and the ViewModel is built exactly once per session.
//! Before rows are appended into that list, three things have to be true at RUNTIME and none of
//! them is settled by static RE:
//!
//! * the ctor hook actually installs and fires (the prologue guard passes on the real build);
//! * the list geometry at `+0x2d8..+0x2f8` is the `{vfptr, allocator, begin, end, capacity}`
//!   shape the RE describes, with a row count that divides cleanly by the `0x350` stride;
//! * the list is not already at capacity, so an append would have to grow it.
//!
//! So the ctor hook lands first as a pure observer that calls the original and reads the list
//! back. It writes nothing into the engine. That makes the first runtime step falsifiable on its
//! own terms -- if the stride does not divide, or capacity equals end, the design is wrong and we
//! learn it before a single byte is written into a MenuHeap structure.
//!
//! # Hooking rules this module obeys
//!
//! * Every detour goes through the `er_hook` UNION, never a bare `MhHook`. Two MinHook instances
//!   patching one prologue corrupt each other's trampolines, and `er_effects_rs.dll` may be
//!   loaded alongside this DLL.
//! * Nothing is patched until [`crate::map_seams::verify_seam`] has re-read the live prologue.
//! * A handler that finds no trampoline does NOT invent a return value -- see
//!   [`worldmap_viewmodel_ctor_hook`].

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
    result
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
                "map-hooks: observing {} @0x{address:x} (verified prologue; observation only, \
                 nothing is written into the engine)",
                WORLDMAP_VIEWMODEL_CTOR.name
            ));
            1
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
