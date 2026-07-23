//! MinHook FFI + hook union for this DLL.
//!
//! The generic implementation (the `MH_*` externs, `MH_STATUS`, the `MhHook` wrapper, and the union:
//! `register_union_hook` + the cross-DLL chaining) moved to the shared `er-hook` crate so all three
//! game cdylibs share one copy and MinHook's C source is compiled once. This module re-exports it, so
//! every existing `crate::mh::{MhHook, MH_*, MH_STATUS, register_union_hook, ...}` reference is
//! unchanged.
//!
//! The `#[no_mangle] er_effects_union_register` C export stays HERE (not in `er-hook`): it is a
//! cross-DLL contract other DLLs resolve by name, and keeping it in this crate ensures ONLY
//! `er_effects_rs.dll` exports it -- exactly as before the extraction.
#![allow(dead_code, non_snake_case, non_camel_case_types, missing_docs)]

use std::sync::atomic::AtomicUsize;

pub use er_hook::*;

/// C-ABI export (2026-07-18, user-directed cross-DLL union). A COMPANION DLL loaded into the same
/// process (the log-only `er-reload-trace-dll`) hooks ~40 native load/menu functions that OVERLAP
/// this DLL's own hooks (e.g. `0xb0e180` continue-confirm, `0xb0d960` title-SetState). If the
/// companion drove its OWN MinHook instance, two instances patching the same address would corrupt
/// each other's trampolines (the exact silent race the internal union was built to fix, now across
/// DLLs). So the companion calls THIS export instead: every shared address is owned by this DLL's
/// single MinHook instance + union, and the companion's handler is CHAINED like any internal one.
///
/// `orig_slot_ptr` points at a `usize`-sized cell (an `AtomicUsize`) that lives in the COMPANION's
/// image; the union stores the trampoline (or next chained handler) there for the companion handler
/// to call. The companion image stays loaded for the process lifetime, so treating it as `'static`
/// is sound. Returns `0` on success, `-1` for a null `orig_slot_ptr`, or the `MH_STATUS` code as a
/// positive `i32` on MinHook failure.
///
/// # Safety
/// `handler` must be a valid `UnionFn` matching `target`'s ABI (≤4 integer/pointer args); `target`
/// must be a real code address in this process; `orig_slot_ptr` must point at a live, aligned
/// `usize` cell that outlives every dispatch (a companion `'static`).
#[unsafe(no_mangle)]
pub unsafe extern "system" fn er_effects_union_register(
    target: usize,
    handler: UnionFn,
    orig_slot_ptr: *mut usize,
) -> i32 {
    if orig_slot_ptr.is_null() {
        return -1;
    }
    // AtomicUsize is a repr(transparent) wrapper over usize, so a *mut usize aliases it soundly.
    let orig_slot: &'static AtomicUsize = unsafe { &*(orig_slot_ptr as *const AtomicUsize) };
    match unsafe { register_union_hook(target, handler, orig_slot) } {
        Ok(()) => 0,
        Err(status) => status as i32,
    }
}
