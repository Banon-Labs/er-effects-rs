use std::{
    ffi::c_void,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

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
static DINPUT_PREVIOUS_SELECTOR_KEYS: AtomicUsize = AtomicUsize::new(0);
static DINPUT_QUEUED_SELECTOR_KEYS: AtomicUsize = AtomicUsize::new(0);
static DINPUT_REPEATED_SELECTOR_KEYS: AtomicUsize = AtomicUsize::new(0);
static DINPUT_REPEAT_STATE: OnceLock<Mutex<DinputRepeatState>> = OnceLock::new();

const DIRECTINPUT_VERSION: u32 = 0x0800;
const DINPUT_KEYBOARD_BUFFER_LEN: usize = 256;
const DIK_0: usize = 0x0b;
const DIK_LEFT_ALT: usize = 0x38;
const DIK_NUMPAD0: usize = 0x52;
const DIK_RIGHT_ALT: usize = 0xb8;
const DIK_LEFT: usize = 0xcb;
const DIK_RIGHT: usize = 0xcd;
const DIK_UP: usize = 0xc8;
const DIK_DOWN: usize = 0xd0;
const DIK_INSERT: usize = 0xd2;

const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;
const VK_INSERT: u32 = 0x2d;
const VK_0: u32 = 0x30;
const VK_NUMPAD0: u32 = 0x60;

const DINPUT_KEY_LEFT: usize = 1 << 0;
const DINPUT_KEY_RIGHT: usize = 1 << 1;
const DINPUT_KEY_UP: usize = 1 << 2;
const DINPUT_KEY_DOWN: usize = 1 << 3;
const DINPUT_KEY_0: usize = 1 << 4;
const DINPUT_KEY_NUMPAD0: usize = 1 << 5;
const DINPUT_KEY_INSERT: usize = 1 << 6;
const DINPUT_ARROW_KEY_MASK: usize =
    DINPUT_KEY_LEFT | DINPUT_KEY_RIGHT | DINPUT_KEY_UP | DINPUT_KEY_DOWN;

const HOLD_REPEAT_LATCH_DELAY: Duration = Duration::from_millis(280);
const HOLD_REPEAT_INITIAL_INTERVAL: Duration = Duration::from_millis(120);
const HOLD_REPEAT_ACCEL_STEP: Duration = Duration::from_millis(12);
const HOLD_REPEAT_MIN_INTERVAL: Duration = Duration::from_millis(42);
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

#[derive(Clone, Copy)]
struct RepeatKey {
    bit: usize,
    vk: u32,
}

const REPEAT_KEYS: [RepeatKey; 4] = [
    RepeatKey {
        bit: DINPUT_KEY_LEFT,
        vk: VK_LEFT,
    },
    RepeatKey {
        bit: DINPUT_KEY_RIGHT,
        vk: VK_RIGHT,
    },
    RepeatKey {
        bit: DINPUT_KEY_UP,
        vk: VK_UP,
    },
    RepeatKey {
        bit: DINPUT_KEY_DOWN,
        vk: VK_DOWN,
    },
];

struct DinputRepeatState {
    next_repeat_at: [Option<Instant>; 4],
    repeat_interval: [Duration; 4],
}

impl Default for DinputRepeatState {
    fn default() -> Self {
        Self {
            next_repeat_at: [None; 4],
            repeat_interval: [HOLD_REPEAT_INITIAL_INTERVAL; 4],
        }
    }
}

pub(crate) fn set_arrow_key_suppression(enabled: bool) {
    SUPPRESS_ARROW_KEYS.store(enabled, Ordering::Relaxed);
    if !HOOKS_INSTALLED.load(Ordering::Relaxed) {
        match unsafe { install_dinput_hooks() } {
            Ok(()) => net_effects_log(format_args!(
                "input-suppression: DirectInput selector input hook installed"
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

pub(crate) fn dinput_queued_selector_keys() -> usize {
    DINPUT_QUEUED_SELECTOR_KEYS.load(Ordering::Relaxed)
}

pub(crate) fn dinput_repeated_selector_keys() -> usize {
    DINPUT_REPEATED_SELECTOR_KEYS.load(Ordering::Relaxed)
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
    queue_dinput_selector_edges(hr, size, data);
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

fn dinput_key_down(size: u32, data: *mut u8, offset: usize) -> bool {
    !data.is_null() && size as usize > offset && unsafe { *data.add(offset) & 0x80 } != 0
}

fn reset_dinput_repeat_state() {
    DINPUT_PREVIOUS_SELECTOR_KEYS.store(0, Ordering::Relaxed);
    if let Some(state) = DINPUT_REPEAT_STATE.get()
        && let Ok(mut state) = state.lock()
    {
        *state = DinputRepeatState::default();
    }
}

fn queue_dinput_selector_edges(hr: i32, size: u32, data: *mut u8) {
    if hr != 0 || data.is_null() || !effects::effect_runtime_ready() {
        reset_dinput_repeat_state();
        return;
    }

    let alt_down =
        dinput_key_down(size, data, DIK_LEFT_ALT) || dinput_key_down(size, data, DIK_RIGHT_ALT);
    let mut pressed_mask = 0usize;
    for (bit, offset) in [
        (DINPUT_KEY_LEFT, DIK_LEFT),
        (DINPUT_KEY_RIGHT, DIK_RIGHT),
        (DINPUT_KEY_UP, DIK_UP),
        (DINPUT_KEY_DOWN, DIK_DOWN),
        (DINPUT_KEY_0, DIK_0),
        (DINPUT_KEY_NUMPAD0, DIK_NUMPAD0),
        (DINPUT_KEY_INSERT, DIK_INSERT),
    ] {
        if dinput_key_down(size, data, offset) {
            pressed_mask |= bit;
        }
    }

    let previous_mask = DINPUT_PREVIOUS_SELECTOR_KEYS.swap(pressed_mask, Ordering::Relaxed);
    let new_edges = pressed_mask & !previous_mask;

    let mut queued = 0usize;
    for (bit, vk, needs_alt) in [
        (DINPUT_KEY_LEFT, VK_LEFT, false),
        (DINPUT_KEY_RIGHT, VK_RIGHT, false),
        (DINPUT_KEY_UP, VK_UP, false),
        (DINPUT_KEY_DOWN, VK_DOWN, false),
        (DINPUT_KEY_0, VK_0, true),
        (DINPUT_KEY_NUMPAD0, VK_NUMPAD0, true),
        (DINPUT_KEY_INSERT, VK_INSERT, true),
    ] {
        if new_edges & bit == 0 || (needs_alt && !alt_down) {
            continue;
        }
        effects::queue_effect_keyboard_vk(vk, alt_down);
        queued = queued.saturating_add(1);
    }
    let repeated = queue_held_arrow_repeats(pressed_mask, new_edges);
    queued = queued.saturating_add(repeated);
    if repeated != 0 {
        DINPUT_REPEATED_SELECTOR_KEYS.fetch_add(repeated, Ordering::SeqCst);
    }
    if queued != 0 {
        DINPUT_QUEUED_SELECTOR_KEYS.fetch_add(queued, Ordering::SeqCst);
    }
}

fn queue_held_arrow_repeats(pressed_mask: usize, new_edges: usize) -> usize {
    let held_arrows = pressed_mask & DINPUT_ARROW_KEY_MASK;
    if held_arrows == 0 || !SUPPRESS_ARROW_KEYS.load(Ordering::Relaxed) {
        if let Some(state) = DINPUT_REPEAT_STATE.get()
            && let Ok(mut state) = state.lock()
        {
            *state = DinputRepeatState::default();
        }
        return 0;
    }

    let now = Instant::now();
    let mut queued = 0usize;
    let Ok(mut state) = DINPUT_REPEAT_STATE
        .get_or_init(|| Mutex::new(DinputRepeatState::default()))
        .lock()
    else {
        return 0;
    };

    for (index, key) in REPEAT_KEYS.iter().enumerate() {
        if held_arrows & key.bit == 0 {
            state.next_repeat_at[index] = None;
            state.repeat_interval[index] = HOLD_REPEAT_INITIAL_INTERVAL;
            continue;
        }
        if new_edges & key.bit != 0 || state.next_repeat_at[index].is_none() {
            state.next_repeat_at[index] = Some(now + HOLD_REPEAT_LATCH_DELAY);
            state.repeat_interval[index] = HOLD_REPEAT_INITIAL_INTERVAL;
            continue;
        }
        let Some(next_repeat_at) = state.next_repeat_at[index] else {
            continue;
        };
        if now < next_repeat_at {
            continue;
        }
        effects::queue_effect_keyboard_vk(key.vk, false);
        queued = queued.saturating_add(1);
        let interval = state.repeat_interval[index]
            .checked_sub(HOLD_REPEAT_ACCEL_STEP)
            .unwrap_or(HOLD_REPEAT_MIN_INTERVAL)
            .max(HOLD_REPEAT_MIN_INTERVAL);
        state.repeat_interval[index] = interval;
        state.next_repeat_at[index] = Some(now + interval);
    }

    queued
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
