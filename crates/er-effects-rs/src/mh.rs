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
use std::sync::atomic::AtomicUsize;

pub use er_hook::*;

/// Hand an installed detour over to MinHook and stop tracking its handle here.
///
/// Every hook install site used to end in `std::mem::forget(hook)` to say "this detour
/// outlives the scope that created it". That was a no-op: `MhHook` is three raw pointers
/// with no `Drop` impl, so dropping it never uninstalled anything -- MinHook has owned the
/// detour since `MH_ApplyQueued`. `clippy::forget_non_drop` flags exactly that, so the
/// intent moved here instead, where it is stated once rather than mimed 60-odd times.
///
/// Takes `MhHook` by value (not a generic) on purpose: a generic would silently accept a
/// type that *does* implement `Drop` and really run its destructor.
pub fn leak_installed_hook(_hook: MhHook) {}

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

/// C-ABI export: the live `CS::LoadingScreenData*`, or 0 when no loading screen is up.
///
/// Published for the standalone `er-crash-logging-dll` hang watchdog, which needs this object to
/// detect a stuck LOAD -- a failure its frame counter structurally cannot see, because frames keep
/// advancing through a loading screen (measured on a Seamless invasion-load softlock, 2026-08-15:
/// eleven minutes at 12% with the frame counter ticking throughout).
///
/// It is an EXPORT rather than a second hook for the same reason `er_effects_union_register` exists.
/// This DLL already detours the loading-screen update (`er-loading-portrait`, RVA 0x90a6b0) and
/// records the object there; a companion installing its own MinHook on that same prologue would
/// corrupt trampolines, which is the conflict class tracked in `scripts/me3-dll-conflicts.toml`. So
/// the companion polls this instead, on the thread it already runs.
///
/// Returns 0 before the first loading screen (the underlying cell starts at `usize::MIN`), which
/// callers must treat as "no data" rather than as an address.
#[unsafe(no_mangle)]
pub extern "system" fn er_effects_loading_screen_data() -> usize {
    er_loading_portrait::layout::LOADING_SCREEN_LAST_DATA.load(std::sync::atomic::Ordering::SeqCst)
}
