use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::core::{GUID, s};

use crate::{effects, log::net_effects_log};

static SUPPRESS_ARROW_KEYS: AtomicBool = AtomicBool::new(false);
static HOOKS_INSTALLED: AtomicBool = AtomicBool::new(false);
static DINPUT_KB_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static DINPUT_MOUSE_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static DINPUT_KB_ALSO_MOUSE: AtomicBool = AtomicBool::new(false);
static DINPUT_KB_HOOK_FIRES: AtomicUsize = AtomicUsize::new(0);
static DINPUT_MOUSE_HOOK_FIRES: AtomicUsize = AtomicUsize::new(0);
static DINPUT_SUPPRESSED_ARROW_KEYS: AtomicUsize = AtomicUsize::new(0);

const DIRECTINPUT_VERSION: u32 = 0x0800;
const DINPUT_KEYBOARD_BUFFER_LEN: usize = 256;
const DIK_LEFT: usize = 0xcb;
const DIK_RIGHT: usize = 0xcd;
const DIK_UP: usize = 0xc8;
const DIK_DOWN: usize = 0xd0;
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
const GUID_SYS_MOUSE: GUID = GUID::from_values(
    0x6F1D2B60,
    0xD5A0,
    0x11CF,
    [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00],
);

const VTBL_RELEASE: usize = 2;
const VTBL_CREATE_DEVICE: usize = 3;
const VTBL_GET_DEVICE_STATE: usize = 9;

type RawObj = *mut *const usize;
type DInput8CreateFn =
    unsafe extern "system" fn(usize, u32, *const GUID, *mut RawObj, usize) -> i32;
type CreateDeviceFn = unsafe extern "system" fn(RawObj, *const GUID, *mut RawObj, usize) -> i32;
type ReleaseFn = unsafe extern "system" fn(RawObj) -> u32;
type GetDeviceStateFn = unsafe extern "system" fn(usize, u32, *mut u8) -> i32;

pub(crate) fn set_arrow_key_suppression(enabled: bool) {
    SUPPRESS_ARROW_KEYS.store(enabled, Ordering::Relaxed);
    if enabled && !HOOKS_INSTALLED.load(Ordering::Relaxed) {
        match unsafe { install_dinput_hooks() } {
            Ok(()) => net_effects_log(format_args!(
                "input-suppression: DirectInput arrow suppression hook installed"
            )),
            Err(status) => net_effects_log(format_args!(
                "input-suppression: DirectInput hook install failed: {status:?}"
            )),
        }
    }
}

pub(crate) fn dinput_kb_hook_fires() -> usize {
    DINPUT_KB_HOOK_FIRES.load(Ordering::Relaxed)
}

pub(crate) fn dinput_mouse_hook_fires() -> usize {
    DINPUT_MOUSE_HOOK_FIRES.load(Ordering::Relaxed)
}

pub(crate) fn dinput_suppressed_arrow_keys() -> usize {
    DINPUT_SUPPRESSED_ARROW_KEYS.load(Ordering::Relaxed)
}

unsafe fn vtable_fn<F: Copy>(obj: RawObj, slot: usize) -> F {
    unsafe { std::mem::transmute_copy(&*(*obj).add(slot)) }
}

unsafe fn with_probe_device(
    di8_create: DInput8CreateFn,
    hinstance: usize,
    guid: &GUID,
    f: impl FnOnce(usize),
) -> Result<(), MH_STATUS> {
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
    let hr = unsafe { create_device(di8, guid, &mut device, 0) };
    if hr != 0 || device.is_null() {
        let release_di8: ReleaseFn = unsafe { vtable_fn(di8, VTBL_RELEASE) };
        unsafe { release_di8(di8) };
        return Err(MH_STATUS::MH_ERROR_FUNCTION_NOT_FOUND);
    }

    let get_state_addr = unsafe { *(*device).add(VTBL_GET_DEVICE_STATE) as usize };
    f(get_state_addr);

    let release_device: ReleaseFn = unsafe { vtable_fn(device, VTBL_RELEASE) };
    let release_di8: ReleaseFn = unsafe { vtable_fn(di8, VTBL_RELEASE) };
    unsafe { release_device(device) };
    unsafe { release_di8(di8) };
    Ok(())
}

unsafe extern "system" fn dinput_kb_get_state_hook(device: usize, size: u32, data: *mut u8) -> i32 {
    DINPUT_KB_HOOK_FIRES.fetch_add(1, Ordering::Relaxed);
    let original_addr = DINPUT_KB_GET_STATE_ORIG.load(Ordering::Relaxed);
    if original_addr == 0 {
        return 0;
    }
    let original: GetDeviceStateFn = unsafe { std::mem::transmute(original_addr) };
    let hr = unsafe { original(device, size, data) };
    zero_dinput_arrow_state(hr, size, data);
    hr
}

unsafe extern "system" fn dinput_mouse_get_state_hook(
    device: usize,
    size: u32,
    data: *mut u8,
) -> i32 {
    DINPUT_MOUSE_HOOK_FIRES.fetch_add(1, Ordering::Relaxed);
    let original_addr = DINPUT_MOUSE_GET_STATE_ORIG.load(Ordering::Relaxed);
    if original_addr == 0 {
        return 0;
    }
    let original: GetDeviceStateFn = unsafe { std::mem::transmute(original_addr) };
    unsafe { original(device, size, data) }
}

fn zero_dinput_arrow_state(hr: i32, size: u32, data: *mut u8) {
    if hr != 0
        || data.is_null()
        || size as usize <= DIK_DOWN
        || !SUPPRESS_ARROW_KEYS.load(Ordering::Relaxed)
    {
        return;
    }
    let mut cleared = 0usize;
    for offset in [DIK_LEFT, DIK_RIGHT, DIK_UP, DIK_DOWN] {
        let slot = unsafe { data.add(offset) };
        let was_pressed = unsafe { *slot } != 0;
        unsafe { *slot = 0 };
        if was_pressed {
            cleared = cleared.saturating_add(1);
        }
    }
    if cleared != 0 {
        DINPUT_SUPPRESSED_ARROW_KEYS.fetch_add(cleared, Ordering::SeqCst);
        effects::record_suppressed_arrow_keys(cleared);
    }
}

unsafe fn install_dinput_hooks() -> Result<(), MH_STATUS> {
    if HOOKS_INSTALLED.load(Ordering::Relaxed) {
        return Ok(());
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => return Err(status),
    }

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

    let mut keyboard_addr = 0usize;
    let mut mouse_addr = 0usize;
    unsafe {
        with_probe_device(di8_create, hinstance, &GUID_SYS_KEYBOARD, |addr| {
            keyboard_addr = addr;
        })?;
        with_probe_device(di8_create, hinstance, &GUID_SYS_MOUSE, |addr| {
            mouse_addr = addr;
        })?;
    }

    let kb_hook = unsafe {
        MhHook::new(
            keyboard_addr as *mut c_void,
            dinput_kb_get_state_hook as *mut c_void,
        )?
    };
    DINPUT_KB_GET_STATE_ORIG.store(kb_hook.trampoline() as usize, Ordering::Relaxed);
    unsafe { kb_hook.queue_enable()? };

    if keyboard_addr == mouse_addr {
        DINPUT_KB_ALSO_MOUSE.store(true, Ordering::Relaxed);
    } else {
        let mouse_hook = unsafe {
            MhHook::new(
                mouse_addr as *mut c_void,
                dinput_mouse_get_state_hook as *mut c_void,
            )?
        };
        DINPUT_MOUSE_GET_STATE_ORIG.store(mouse_hook.trampoline() as usize, Ordering::Relaxed);
        unsafe { mouse_hook.queue_enable()? };
        std::mem::forget(mouse_hook);
    }

    unsafe { MH_ApplyQueued() }.ok_context("DInput MH_ApplyQueued")?;
    std::mem::forget(kb_hook);
    HOOKS_INSTALLED.store(true, Ordering::Relaxed);
    Ok(())
}
