//! Keyboard presses at the one stage ELDEN RING 1.17 actually reads.
//!
//! # Why this module is a phone call and not a hook
//!
//! The game's keyboard comes from DirectInput8's `IDirectInputDevice8::GetDeviceState`. That is not
//! a preference: `eldenring.exe` (1.17) imports no RawInput API whatsoever -- not `GetRawInputData`,
//! not `GetRawInputBuffer`, not `RegisterRawInputDevices`; the string is absent from the image. A
//! `SendInput` press therefore has nowhere to land, which is what run br-20260905-031610-5406
//! measured when it logged 150 supplied input frames beside 0 RawInput key events.
//!
//! `er_quickload.dll` already detours that `GetDeviceState` slot and stamps a held scancode into the
//! buffer after DInput has filled it. Two DLLs cannot both own that prologue -- each links its own
//! MinHook instance and the loser's trampoline is silently overwritten, the conflict class tracked in
//! `scripts/me3-dll-conflicts.toml`. So the harness does not install a second hook; it resolves the
//! product's `er_quickload_hold_dinput_key` export and asks the existing owner to hold the key.
//!
//! # What this buys the menu drive
//!
//! The pause menu was being driven by writing the FD4 pad device -- the axis at `+0x28`, the buttons
//! at `+0x08`/`+0x10` -- and every run derailed at `nav_to_optionsetting`. This is the channel with a
//! measured effect on the same build: the in-world displacement on br-20260905-161450-ec54 came from
//! a stamped scancode. Pressing the key the player would press is also the only path that can
//! reproduce a player's bug, which is the whole point of driving the menu rather than calling into it.
//!
//! Resolution is lazy and cached, and a miss is inert rather than fatal: run the harness without the
//! product in the profile and every press is a no-op, exactly as it was before this module existed.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::win32::{GetModuleHandleA, GetProcAddress};

/// The product DLL by base name, matched the way `er-hook` matches it -- me3 loads natives from
/// paths that differ per install, so the name is the only stable handle.
const PRODUCT_DLL_NAME: &[u8] = b"er_quickload.dll\0";
const HOLD_KEY_EXPORT: &[u8] = b"er_quickload_hold_dinput_key\0";
const HOLD_CURSOR_EXPORT: &[u8] = b"er_quickload_hold_cursor_pos\0";
const HOLD_VK_EXPORT: &[u8] = b"er_quickload_hold_vk\0";
const PICKER_DIALOG_EXPORT: &[u8] = b"er_quickload_save_picker_dialog\0";

/// Cached export address. `0` = not yet resolved, `1` = resolved to absent (the product is not in
/// this profile), anything else = the function. The sentinel keeps a missing product from costing a
/// `GetProcAddress` on every frame of every nav phase.
static HOLD_KEY_FN: AtomicUsize = AtomicUsize::new(0);
const RESOLVED_ABSENT: usize = 1;

type HoldKeyFn = unsafe extern "system" fn(u8);

fn resolve() -> Option<HoldKeyFn> {
    let cached = HOLD_KEY_FN.load(Ordering::Relaxed);
    if cached == RESOLVED_ABSENT {
        return None;
    }
    if cached != 0 {
        // SAFETY: the value was produced by `GetProcAddress` on a module me3 keeps mapped for the
        // process lifetime, so the pointer stays valid and the C-ABI shape is fixed by the export.
        return Some(unsafe { std::mem::transmute::<usize, HoldKeyFn>(cached) });
    }
    let module = unsafe { GetModuleHandleA(PRODUCT_DLL_NAME.as_ptr()) };
    let address = if module.is_null() {
        std::ptr::null_mut()
    } else {
        unsafe { GetProcAddress(module, HOLD_KEY_EXPORT.as_ptr()) }
    };
    if address.is_null() {
        // Do not latch absent here. The harness's first nav frame can precede me3's LoadLibrary of
        // the product only in a hand-written profile, but a latch would make that ordering permanent
        // for the whole session; re-probing costs one call on a path that is already frame-rate.
        return None;
    }
    HOLD_KEY_FN.store(address as usize, Ordering::Relaxed);
    // SAFETY: as above -- fixed C-ABI export in a module that stays mapped.
    Some(unsafe { std::mem::transmute::<*mut c_void, HoldKeyFn>(address) })
}

/// Hold `dik` down until the next call. `0` releases.
///
/// Returns whether the press reached the product's stamp, so a caller can log "the key was never
/// delivered" separately from "the key was delivered and the menu ignored it" -- the distinction the
/// previous pad-write drive could not make.
pub fn hold(dik: u8) -> bool {
    match resolve() {
        Some(f) => {
            unsafe { f(dik) };
            true
        }
        None => false,
    }
}

/// Cached cursor export, same sentinel scheme as [`HOLD_KEY_FN`].
static HOLD_CURSOR_FN: AtomicUsize = AtomicUsize::new(0);

type HoldCursorFn = unsafe extern "system" fn(u64);

/// Tell the game the cursor is at `(x, y)`. Returns whether the product export was reachable, so a
/// caller can tell "the product is not in this profile" from "the menu ignored the pointer".
///
/// The pair is packed into one `u64` (`(x << 32) | y`) by the export's contract: the game thread
/// reads it inside `GetCursorPos` and a torn read would hit-test a coordinate that never existed.
pub fn hold_cursor(x: i32, y: i32) -> bool {
    let packed = ((x as u32 as u64) << 32) | (y as u32 as u64);
    call_cursor(packed)
}

/// Stop authoring the cursor; the game sees the real one again.
pub fn release_cursor() -> bool {
    call_cursor(u64::MAX)
}

fn call_cursor(packed: u64) -> bool {
    let cached = HOLD_CURSOR_FN.load(Ordering::Relaxed);
    let address = if cached != 0 {
        cached as *mut c_void
    } else {
        let module = unsafe { GetModuleHandleA(PRODUCT_DLL_NAME.as_ptr()) };
        if module.is_null() {
            return false;
        }
        let resolved = unsafe { GetProcAddress(module, HOLD_CURSOR_EXPORT.as_ptr()) };
        if resolved.is_null() {
            return false;
        }
        HOLD_CURSOR_FN.store(resolved as usize, Ordering::Relaxed);
        resolved
    };
    // SAFETY: fixed C-ABI export in a module me3 keeps mapped for the process lifetime.
    let f: HoldCursorFn = unsafe { std::mem::transmute::<*mut c_void, HoldCursorFn>(address) };
    unsafe { f(packed) };
    true
}

/// Cached virtual-key export.
static HOLD_VK_FN: AtomicUsize = AtomicUsize::new(0);

type HoldVkFn = unsafe extern "system" fn(u8);

/// Hold a Win32 virtual key -- `0x01` is the left mouse button, which is how a pointer-driven menu
/// row is confirmed. `0` releases.
pub fn hold_vk(vk: u8) -> bool {
    let cached = HOLD_VK_FN.load(Ordering::Relaxed);
    let address = if cached != 0 {
        cached as *mut c_void
    } else {
        let module = unsafe { GetModuleHandleA(PRODUCT_DLL_NAME.as_ptr()) };
        if module.is_null() {
            return false;
        }
        let resolved = unsafe { GetProcAddress(module, HOLD_VK_EXPORT.as_ptr()) };
        if resolved.is_null() {
            return false;
        }
        HOLD_VK_FN.store(resolved as usize, Ordering::Relaxed);
        resolved
    };
    // SAFETY: fixed C-ABI export in a module me3 keeps mapped for the process lifetime.
    let f: HoldVkFn = unsafe { std::mem::transmute::<*mut c_void, HoldVkFn>(address) };
    unsafe { f(vk) };
    true
}

/// `VK_LBUTTON`.
pub const VK_LBUTTON: u8 = 0x01;

/// Cached picker-dialog export.
static PICKER_DIALOG_FN: AtomicUsize = AtomicUsize::new(0);

type PickerDialogFn = unsafe extern "system" fn() -> usize;

/// The live save-file-picker dialog, or 0 when the picker is not up (or the product is absent).
pub fn save_picker_dialog() -> usize {
    let cached = PICKER_DIALOG_FN.load(Ordering::Relaxed);
    let address = if cached != 0 {
        cached as *mut c_void
    } else {
        let module = unsafe { GetModuleHandleA(PRODUCT_DLL_NAME.as_ptr()) };
        if module.is_null() {
            return 0;
        }
        let resolved = unsafe { GetProcAddress(module, PICKER_DIALOG_EXPORT.as_ptr()) };
        if resolved.is_null() {
            return 0;
        }
        PICKER_DIALOG_FN.store(resolved as usize, Ordering::Relaxed);
        resolved
    };
    // SAFETY: fixed C-ABI export in a module me3 keeps mapped for the process lifetime.
    let f: PickerDialogFn = unsafe { std::mem::transmute::<*mut c_void, PickerDialogFn>(address) };
    unsafe { f() }
}
