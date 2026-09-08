//! MinHook FFI + hook union for this DLL.
//!
//! The generic implementation (the `MH_*` externs, `MH_STATUS`, the `MhHook` wrapper, and the union:
//! `register_union_hook` + the cross-DLL chaining) moved to the shared `er-hook` crate so all three
//! game cdylibs share one copy and MinHook's C source is compiled once. This module re-exports it, so
//! every existing `crate::mh::{MhHook, MH_*, MH_STATUS, register_union_hook, ...}` reference is
//! unchanged.
//!
//! The `#[no_mangle] er_effects_union_register` C export stays here (not in `er-hook`): it is a
//! cross-DLL contract other DLLs resolve by name, and keeping it in this crate ensures only
//! `er_quickload.dll` exports it -- exactly as before the extraction.
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

/// C-ABI export (2026-07-18, user-directed cross-DLL union). A companion DLL loaded into the same
/// process (the log-only `er-reload-trace`) hooks ~40 native load/menu functions that overlap
/// this DLL's own hooks (e.g. `0xb0e180` continue-confirm, `0xb0d960` title-SetState). If the
/// companion drove its own MinHook instance, two instances patching the same address would corrupt
/// each other's trampolines (the exact silent race the internal union was built to fix, now across
/// DLLs). So the companion calls this export instead: every shared address is owned by this DLL's
/// single MinHook instance + union, and the companion's handler is chained like any internal one.
///
/// `orig_slot_ptr` points at a `usize`-sized cell (an `AtomicUsize`) that lives in the companion's
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

/// C-ABI export: hold (or release, with 0) a DirectInput keyboard scancode in front of the game.
///
/// The only keyboard stage ER 1.17 reads. `eldenring.exe` imports no RawInput API at all, so a
/// `SendInput` press has no code path to reach the game; what does reach it is this DLL's detour on
/// the DInput8 keyboard `GetDeviceState`, which stamps the scancode into the 256-byte buffer after
/// DInput has filled it. That makes the press focus-independent -- it applies with the window in the
/// background and without ever forcing ER foreground -- and it is the same channel that carried the
/// measured in-world displacement on br-20260905-161450-ec54.
///
/// It is an export rather than a second hook because the DInput vtable slot is shared: three DLLs
/// detour it, and each linking its own MinHook instance overwrites the others' trampolines (the
/// conflict class in `scripts/me3-dll-conflicts.toml`). `er-input-harness` needs to press keys, not
/// to own the prologue, so it asks this DLL to stamp for it -- one instance, one owner.
///
/// The stamp is inert while the held code is 0, so a companion that never calls this changes nothing.
#[unsafe(no_mangle)]
pub extern "system" fn er_quickload_hold_dinput_key(dik: u8) {
    crate::input_blocker::InputBlocker::get_instance().set_injected_key(dik);
}

/// C-ABI export: the live `05_010_ProfileSelect` dialog our save-file picker runs on, or 0.
///
/// This is the only way to know which cursor is the PICKER'S. The picker's cursor is a
/// `CS::GridControl` at `dialog + 0xa38`, whose selected cell at `+0xd4` is the field
/// `DIALOG_SLOT_CURSOR_B0C_OFFSET` already names (`0xa38 + 0xd4 == 0xb0c`). A memory scan for the
/// GridControl vtable finds it -- along with thirteen other live grids, indistinguishable by
/// address. Watching the wrong one is not a small error: a drive reports "the press did nothing"
/// while the press worked perfectly, which is exactly what happened on br-20260905-181031-f1a0 when
/// a Right in the file browser moved a cursor nobody was looking at.
///
/// The dialog is whatever `SYSTEM_QUIT_PROFILE_SELECT_WINDOW` currently holds, so this answers 0
/// until the picker is actually up -- "not open" and "open at row 0" stay distinguishable.
#[unsafe(no_mangle)]
pub extern "system" fn er_quickload_save_picker_dialog() -> usize {
    er_telemetry_core::counters::SYSTEM_QUIT_PROFILE_SELECT_WINDOW
        .load(std::sync::atomic::Ordering::SeqCst)
}

/// C-ABI export: hold (or release, with 0) a Win32 virtual key -- including the mouse buttons.
///
/// `VK_LBUTTON` is 0x01, and a left click is how a pointer-driven menu is confirmed. ELDEN RING 1.17
/// imports USER32's `GetKeyState`/`GetKeyboardState`, and this DLL detours both, authoring the answer
/// after the original call: focus-independent, and invisible outside the process.
///
/// Separate from `er_quickload_hold_dinput_key` because they are different stages, not different
/// spellings of one -- that one writes a DirectInput SCANCODE into the keyboard buffer, this one
/// answers a virtual-key query. A run that confuses them cannot tell "the click never arrived" from
/// "the click arrived at the wrong layer", which is the distinction every menu-drive attempt here
/// has turned on.
#[unsafe(no_mangle)]
pub extern "system" fn er_quickload_hold_vk(vk: u8) {
    crate::experiments::input_block::set_injected_vk_public(vk);
}

/// C-ABI export: tell the game the cursor is at `(x, y)`. Pass `u64::MAX` as `packed` to stop.
///
/// `packed` is `(x << 32) | y`, one value so the pair cannot be read half-updated by the game
/// thread mid-hit-test. This does not move the user's real pointer -- it authors the answer the
/// game gets from USER32's `GetCursorPos`, so nothing is visible outside the process and the OS
/// cannot fight it.
///
/// It exists because the ELDEN RING pause menu is a pointer hit-test, not a list index: a real
/// mouse nudge walked one `CS::GridControl`'s hovered cell through 5, 6, 1, 2, 4, 3, 2, 17 while
/// every other live grid held still, and no pad axis or button write has ever moved it. Writing the
/// menu's own pointer pair does not work either -- the game refreshes it from the cursor each frame
/// (five coordinates written, `wrote=true` each time, hovered cell never left 0 on
/// br-20260905-175511-72a6). USER32 is where the mouse actually enters the process.
///
/// An export rather than a second hook for the reason every other one here is: `er-input-harness`
/// needs to point, not to own the USER32 prologue.
#[unsafe(no_mangle)]
pub extern "system" fn er_quickload_hold_cursor_pos(packed: u64) {
    crate::experiments::input_block::set_injected_cursor_pos(packed);
}

/// C-ABI export: the live `CS::LoadingScreenData*`, or 0 when no loading screen is up.
///
/// Published for the standalone `er-crash-logging` hang watchdog, which needs this object to
/// detect a stuck load -- a failure its frame counter structurally cannot see, because frames keep
/// advancing through a loading screen (measured on a Seamless invasion-load softlock, 2026-08-15:
/// eleven minutes at 12% with the frame counter ticking throughout).
///
/// It is an export rather than a second hook for the same reason `er_effects_union_register` exists.
/// This DLL already detours the loading-screen update (`er-loading-portrait-core`, RVA 0x90a6b0) and
/// records the object there; a companion installing its own MinHook on that same prologue would
/// corrupt trampolines, which is the conflict class tracked in `scripts/me3-dll-conflicts.toml`. So
/// the companion polls this instead, on the thread it already runs.
///
/// Returns 0 before the first loading screen (the underlying cell starts at `usize::MIN`), which
/// callers must treat as "no data" rather than as an address.
#[unsafe(no_mangle)]
pub extern "system" fn er_quickload_loading_screen_data() -> usize {
    er_loading_portrait_core::layout::LOADING_SCREEN_LAST_DATA
        .load(std::sync::atomic::Ordering::SeqCst)
}
