//! The in-game hotkey, read where Elden Ring itself reads the keyboard.
//!
//! A `WH_KEYBOARD_LL` hook is not enough: with the game focused, its keyboard state comes from
//! `IDirectInputDevice8::GetDeviceState`, so the DIK table that call fills is the only place a
//! keypress is guaranteed to be visible from inside the process. This hooks vtable slot 9 on a
//! throwaway probe device -- every DirectInput device of a class shares one vtable, so the probe's
//! slot address is the game's -- and reads the combination out of the buffer on the way back.
//!
//! Devices of different classes can also share that implementation, and then the mouse arrives at
//! the keyboard hook with a 16-byte `DIMOUSESTATE`. Reading DIK offsets out of one finds nothing,
//! which reads as "the hotkey was released" and re-arms the press on the very next keyboard poll.
//! The 256-byte size check is what keeps a held hotkey from toggling once per interleaved poll.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use er_hook::{HookRoute, MH_STATUS, UnionFn, register_shared_hook_with_budget};
use er_hotkey_config::{AtomicChord, chord_name, keys::DIK_DOWN_BIT};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::core::{GUID, s};

use crate::{
    keys::{Chord, hotkey_is_down},
    log::charm_log,
};

/// `BYTE[256]` -- the DIK table filled by a keyboard `GetDeviceState`, and the only state buffer
/// that is 256 bytes. `DIJOYSTATE2` is 272, so an inequality would let joystick axis bytes be read
/// as keys.
const KEYBOARD_STATE_BYTES: u32 = 256;

/// A single non-blocking probe for the product DLL's union export. The install is retried from the
/// FrameBegin tick, and by the time a game frame runs every native in the profile is loaded, so
/// there is nothing left to poll for -- and polling on the game thread would be a visible stall.
const FRAME_DRIVEN_RESOLVE_TRIES: u32 = 1;
const FRAME_DRIVEN_RESOLVE_SLEEP_MS: u32 = 0;

const DIRECTINPUT_VERSION: u32 = 0x0800;
const VTBL_RELEASE: usize = 2;
const VTBL_CREATE_DEVICE: usize = 3;
const VTBL_GET_DEVICE_STATE: usize = 9;

const IID_IDIRECTINPUT8W: GUID = GUID::from_values(
    0xbf798031,
    0x483a,
    0x4da2,
    [0xaa, 0x99, 0x5d, 0x64, 0xed, 0x36, 0x97, 0x00],
);
const GUID_SYS_KEYBOARD: GUID = GUID::from_values(
    0x6F1D2B61,
    0xD5A0,
    0x11CF,
    [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00],
);

type RawObj = *mut *const usize;
type DInput8CreateFn =
    unsafe extern "system" fn(usize, u32, *const GUID, *mut RawObj, usize) -> i32;
type CreateDeviceFn = unsafe extern "system" fn(RawObj, *const GUID, *mut RawObj, usize) -> i32;
type ReleaseFn = unsafe extern "system" fn(RawObj) -> u32;

/// Reported by a repeat [`install_hotkey_hook`] call, which does nothing: the real route was
/// already logged by the call that installed the detour.
const HOOK_ROUTE_WHEN_INSTALLED: HookRoute = HookRoute::LocalUnion;

/// The hotkey in force, read by the detour below without a lock and replaced by
/// [`rebind`] when the config file changes. It was a `OnceLock`, which is why changing the
/// hotkey used to mean quitting the game.
static HOTKEY: AtomicChord = AtomicChord::unset();
static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);
static GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
/// Whether the combination was already held on the previous keyboard poll, so a hold produces one
/// toggle rather than one per frame.
static HOTKEY_HELD: AtomicBool = AtomicBool::new(false);
/// Presses the game task has not consumed yet.
static PENDING_TOGGLES: AtomicUsize = AtomicUsize::new(0);
static KEYBOARD_READS: AtomicUsize = AtomicUsize::new(0);
static NON_KEYBOARD_READS: AtomicUsize = AtomicUsize::new(0);
/// Trigger-key bytes blanked so the hotkey does not also reach whatever the game binds that key
/// to. Only the trigger is blanked; the modifiers are left alone.
static SUPPRESSED_TRIGGER_READS: AtomicUsize = AtomicUsize::new(0);

/// Swap in a new hotkey and forget whatever the old one was doing.
///
/// The reset is the load-bearing half. `HOTKEY_HELD` latches "the combination was down on the
/// previous poll", and it is about the old key. Leave it set across a rebind and the next poll
/// that finds the new key down sees `swap(true)` return true -- so the press is swallowed -- or,
/// worse, a key that happens to be held at the moment of the swap counts as already pressed and
/// the release re-arms it into a toggle the player never asked for. Pending toggles are dropped
/// for the same reason: a press queued against the key that is no longer bound is not a press the
/// player meant for the key that now is.
pub(crate) fn rebind(hotkey: Chord) {
    HOTKEY.store(hotkey);
    HOTKEY_HELD.store(false, Ordering::SeqCst);
    PENDING_TOGGLES.store(0, Ordering::SeqCst);
    charm_log(format_args!(
        "hotkey: now listening for {}",
        chord_name(hotkey)
    ));
}

/// Take the presses queued since the last call.
pub(crate) fn take_pending_toggles() -> usize {
    PENDING_TOGGLES.swap(0, Ordering::SeqCst)
}

pub(crate) fn hook_installed() -> bool {
    HOOK_INSTALLED.load(Ordering::Relaxed)
}

pub(crate) fn keyboard_reads() -> usize {
    KEYBOARD_READS.load(Ordering::Relaxed)
}

pub(crate) fn non_keyboard_reads() -> usize {
    NON_KEYBOARD_READS.load(Ordering::Relaxed)
}

pub(crate) fn suppressed_trigger_reads() -> usize {
    SUPPRESSED_TRIGGER_READS.load(Ordering::Relaxed)
}

unsafe fn vtable_fn<F: Copy>(obj: RawObj, slot: usize) -> F {
    unsafe { std::mem::transmute_copy(&*(*obj).add(slot)) }
}

/// Create a keyboard device only to read its `GetDeviceState` slot address, then release both it
/// and the `IDirectInput8` that made it.
unsafe fn probe_get_device_state_address(
    di8_create: DInput8CreateFn,
    hinstance: usize,
) -> Result<usize, MH_STATUS> {
    let mut di8: RawObj = std::ptr::null_mut();
    let hr = unsafe {
        di8_create(
            hinstance,
            DIRECTINPUT_VERSION,
            &IID_IDIRECTINPUT8W,
            &mut di8,
            0,
        )
    };
    if hr != 0 || di8.is_null() {
        return Err(MH_STATUS::MH_ERROR_FUNCTION_NOT_FOUND);
    }

    let create_device: CreateDeviceFn = unsafe { vtable_fn(di8, VTBL_CREATE_DEVICE) };
    let mut device: RawObj = std::ptr::null_mut();
    let hr = unsafe { create_device(di8, &GUID_SYS_KEYBOARD, &mut device, 0) };
    let release_di8: ReleaseFn = unsafe { vtable_fn(di8, VTBL_RELEASE) };
    if hr != 0 || device.is_null() {
        unsafe { release_di8(di8) };
        return Err(MH_STATUS::MH_ERROR_FUNCTION_NOT_FOUND);
    }

    let address = unsafe { *(*device).add(VTBL_GET_DEVICE_STATE) as usize };
    let release_device: ReleaseFn = unsafe { vtable_fn(device, VTBL_RELEASE) };
    unsafe { release_device(device) };
    unsafe { release_di8(di8) };
    Ok(address)
}

/// The detour, in the union's four-`usize` shape.
///
/// `GET_STATE_ORIG` may hold the next handler in the chain rather than the game trampoline, so it
/// has to be called through [`UnionFn`] and not through the narrower `GetDeviceStateFn`. The
/// `usize` return carries the `HRESULT` in its low 32 bits, which is where the caller reads it
/// from; it is passed straight back rather than reconstructed.
unsafe extern "system" fn get_device_state_union(
    device: usize,
    size: usize,
    data: usize,
    unused: usize,
) -> usize {
    let next = GET_STATE_ORIG.load(Ordering::Relaxed);
    if next == 0 {
        return 0;
    }
    let call: UnionFn = unsafe { std::mem::transmute::<usize, UnionFn>(next) };
    let hr = unsafe { call(device, size, data, unused) };
    if size as u32 == KEYBOARD_STATE_BYTES {
        KEYBOARD_READS.fetch_add(1, Ordering::Relaxed);
        observe_keyboard_state(hr as i32, size as u32, data as *mut u8);
    } else {
        NON_KEYBOARD_READS.fetch_add(1, Ordering::Relaxed);
    }
    hr
}

/// Queue a toggle on the rising edge of the combination, and blank the trigger key so the game
/// does not also see it.
fn observe_keyboard_state(hr: i32, size: u32, data: *mut u8) {
    let Some(hotkey) = HOTKEY.load() else {
        return;
    };
    if hr != 0 || data.is_null() {
        HOTKEY_HELD.store(false, Ordering::SeqCst);
        return;
    }
    let down = unsafe { hotkey_is_down(hotkey, size, data.cast_const()) };
    if down && !HOTKEY_HELD.swap(true, Ordering::SeqCst) {
        PENDING_TOGGLES.fetch_add(1, Ordering::SeqCst);
    } else if !down {
        HOTKEY_HELD.store(false, Ordering::SeqCst);
    }
    if down
        && let Some(trigger_offset) = hotkey.scancode_offset()
        && (size as usize) > trigger_offset
    {
        let trigger = unsafe { data.add(trigger_offset) };
        if unsafe { *trigger } & DIK_DOWN_BIT != 0 {
            unsafe { *trigger = 0 };
            SUPPRESSED_TRIGGER_READS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Install the keyboard hook. Fails until `dinput8.dll` is loaded, so the caller retries.
///
/// The detour goes in through [`register_shared_hook`], never a bare `MhHook`: `er-quickload`
/// and `er-net-effects` both detour this same `GetDeviceState` slot, and two separately
/// linked MinHook instances on one prologue silently overwrite each other's trampolines -- the
/// loser reports installed and never runs. Routing through the product's union when the product
/// is co-loaded puts one instance in charge and chains the handlers instead.
pub(crate) fn install_hotkey_hook(hotkey: Chord) -> Result<HookRoute, MH_STATUS> {
    if HOOK_INSTALLED.load(Ordering::Relaxed) {
        return Ok(HOOK_ROUTE_WHEN_INSTALLED);
    }
    HOTKEY.store(hotkey);

    let dinput8 = unsafe { GetModuleHandleA(s!("dinput8.dll")) }
        .map_err(|_| MH_STATUS::MH_ERROR_MODULE_NOT_FOUND)?;
    let di8_create: DInput8CreateFn = unsafe {
        std::mem::transmute(
            GetProcAddress(dinput8, s!("DirectInput8Create"))
                .ok_or(MH_STATUS::MH_ERROR_FUNCTION_NOT_FOUND)?,
        )
    };
    let hinstance = unsafe { GetModuleHandleA(None) }
        .map_err(|_| MH_STATUS::MH_ERROR_MODULE_NOT_FOUND)?
        .0 as usize;

    let get_state_address = unsafe { probe_get_device_state_address(di8_create, hinstance)? };
    let route = unsafe {
        register_shared_hook_with_budget(
            get_state_address,
            get_device_state_union,
            &GET_STATE_ORIG,
            FRAME_DRIVEN_RESOLVE_TRIES,
            FRAME_DRIVEN_RESOLVE_SLEEP_MS,
        )?
    };
    // The registrar owns the detour for the life of the process; nothing uninstalls it.
    HOOK_INSTALLED.store(true, Ordering::Relaxed);
    charm_log(format_args!(
        "hotkey: DirectInput GetDeviceState hook installed at 0x{get_state_address:x} via {route:?}"
    ));
    Ok(route)
}
