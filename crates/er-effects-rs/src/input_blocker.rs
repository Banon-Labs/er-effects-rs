use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use bitflags::bitflags;
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::core::{GUID, s};

use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};

static INPUT_BLOCKER: OnceLock<&'static InputBlocker> = OnceLock::new();
static INJECTED_KEY: AtomicU8 = AtomicU8::new(0);
static SUPPRESS_ARROW_KEYS: AtomicBool = AtomicBool::new(false);
pub(crate) static DINPUT_SUPPRESSED_ARROW_KEYS: AtomicUsize = AtomicUsize::new(0);
/// DIAGNOSTIC: how many times the game actually CALLS the DInput keyboard/mouse `GetDeviceState`
/// (i.e. whether native ER reads input via DInput at all). If the keyboard counter stays 0 while the
/// harness holds, ER does NOT read keyboard via DInput on native -> our `set_injected_key` stamp never
/// reaches the game and a different injection path (WM_KEYDOWN / RawInput) is required.
pub static DINPUT_KB_HOOK_FIRES: AtomicUsize = AtomicUsize::new(0);
pub static DINPUT_MOUSE_HOOK_FIRES: AtomicUsize = AtomicUsize::new(0);

#[derive(Default)]
pub struct InputBlocker {
    flags: AtomicU8,
    hooks_installed: AtomicBool,
}

impl InputBlocker {
    pub const fn new() -> Self {
        Self {
            flags: AtomicU8::new(0),
            hooks_installed: AtomicBool::new(false),
        }
    }

    pub fn get_instance() -> &'static InputBlocker {
        INPUT_BLOCKER.get_or_init(|| {
            static INSTANCE: InputBlocker = InputBlocker::new();
            &INSTANCE
        })
    }

    /// Receives the context from the pre-reload DLL.
    pub fn forward_instance(instance: &'static InputBlocker) {
        if INPUT_BLOCKER.set(instance).is_ok() {
            instance.hooks_installed.store(true, Ordering::Relaxed);
        }
    }

    /// # Safety
    ///
    /// Installs DirectInput hooks; must run in the target process after dinput8.dll is loaded.
    pub unsafe fn install_hooks(&self) -> Result<(), MH_STATUS> {
        if self.hooks_installed.load(Ordering::Relaxed) {
            return Ok(());
        }
        unsafe { install_dinput_hooks()? };
        self.hooks_installed.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn block(&self, inputs: InputFlags) {
        self.flags.fetch_or(inputs.bits(), Ordering::Relaxed);
    }

    pub fn block_only(&self, inputs: InputFlags) {
        self.flags.store(inputs.bits(), Ordering::Relaxed);
    }

    pub fn unblock(&self, inputs: InputFlags) {
        self.flags
            .fetch_and(inputs.complement().bits(), Ordering::Relaxed);
    }

    /// Inject a keyboard key (DInput DIK scancode) into the blocked keyboard state each poll until
    /// cleared (0 = none). User input remains suppressed.
    pub fn set_injected_key(&self, dik: u8) {
        INJECTED_KEY.store(dik, Ordering::Relaxed);
    }

    /// Suppress only the DInput arrow-key state while leaving the rest of the keyboard live.
    pub fn set_arrow_key_suppression(&self, enabled: bool) {
        SUPPRESS_ARROW_KEYS.store(enabled, Ordering::Relaxed);
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct InputFlags: u8 {
        const GamePad  = 0b001;
        const Keyboard = 0b010;
        const Mouse    = 0b100;
    }
}

fn is_blocked(flags: InputFlags) -> bool {
    INPUT_BLOCKER.get().is_some_and(|b| {
        InputFlags::from_bits_retain(b.flags.load(Ordering::Relaxed)).intersects(flags)
    })
}

const DIRECTINPUT_VERSION: u32 = 0x0800;
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

unsafe fn vtable_fn<F: Copy>(obj: RawObj, slot: usize) -> F {
    unsafe { std::mem::transmute_copy(&*(*obj).add(slot)) }
}

unsafe fn with_probe_device(
    di8_create: DInput8CreateFn,
    hinstance: usize,
    guid: &GUID,
    f: impl FnOnce(usize),
) {
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
    assert_eq!(hr, 0, "DirectInput8Create failed: {hr:#010x}");

    let create_device: CreateDeviceFn = unsafe { vtable_fn(di8, VTBL_CREATE_DEVICE) };
    let mut device: RawObj = std::ptr::null_mut();
    let hr = unsafe { create_device(di8, guid, &mut device, 0) };
    assert_eq!(
        hr, 0,
        "IDirectInput8::CreateDevice({guid:?}) failed: {hr:#010x}"
    );

    let get_state_addr = unsafe { *(*device).add(VTBL_GET_DEVICE_STATE) as usize };
    f(get_state_addr);

    let release_device: ReleaseFn = unsafe { vtable_fn(device, VTBL_RELEASE) };
    let release_di8: ReleaseFn = unsafe { vtable_fn(di8, VTBL_RELEASE) };
    unsafe { release_device(device) };
    unsafe { release_di8(di8) };
}

type GetDeviceStateFn = unsafe extern "system" fn(usize, u32, *mut u8) -> i32;

static DINPUT_KB_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static DINPUT_MOUSE_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static DINPUT_KB_ALSO_MOUSE: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn dinput_kb_get_state_hook(device: usize, size: u32, data: *mut u8) -> i32 {
    DINPUT_KB_HOOK_FIRES.fetch_add(1, Ordering::Relaxed);
    let original_addr = DINPUT_KB_GET_STATE_ORIG.load(Ordering::Relaxed);
    if original_addr == 0 {
        return 0;
    }
    let original: GetDeviceStateFn = unsafe { std::mem::transmute(original_addr) };
    let hr = unsafe { original(device, size, data) };
    let flags = if DINPUT_KB_ALSO_MOUSE.load(Ordering::Relaxed) {
        InputFlags::Keyboard | InputFlags::Mouse
    } else {
        InputFlags::Keyboard
    };
    zero_blocked_dinput_state(hr, size, data, flags);
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
    let hr = unsafe { original(device, size, data) };
    zero_blocked_dinput_state(hr, size, data, InputFlags::Mouse);
    hr
}

fn zero_blocked_dinput_state(hr: i32, size: u32, data: *mut u8, flags: InputFlags) {
    if hr != 0 || data.is_null() || size == 0 {
        return;
    }

    let size = size as usize;
    if flags.contains(InputFlags::Keyboard)
        && SUPPRESS_ARROW_KEYS.load(Ordering::Relaxed)
        && size >= DINPUT_KEYBOARD_BUFFER_LEN
    {
        zero_dinput_arrow_keys(data);
    }

    if !is_blocked(flags) {
        return;
    }

    unsafe { std::ptr::write_bytes(data, 0, size) };
    if flags.contains(InputFlags::Keyboard) && size >= DINPUT_KEYBOARD_BUFFER_LEN {
        const DIK_PRESSED: u8 = 0x80;
        let dik = INJECTED_KEY.load(Ordering::Relaxed);
        if dik != 0 && (dik as usize) < DINPUT_KEYBOARD_BUFFER_LEN {
            unsafe { *data.add(dik as usize) = DIK_PRESSED };
        }
    }
}

fn zero_dinput_arrow_keys(data: *mut u8) {
    const DIK_LEFT: usize = 0xcb;
    const DIK_RIGHT: usize = 0xcd;
    const DIK_UP: usize = 0xc8;
    const DIK_DOWN: usize = 0xd0;
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
        crate::effects::EFFECT_INPUT_SUPPRESSED_KEYS.fetch_add(cleared, Ordering::SeqCst);
        crate::effects::EFFECT_INPUT_SUPPRESSED_ARROW_KEYS.fetch_add(cleared, Ordering::SeqCst);
    }
}

const DINPUT_KEYBOARD_BUFFER_LEN: usize = 256;

unsafe fn install_dinput_hooks() -> Result<(), MH_STATUS> {
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => return Err(status),
    }

    let dinput8 = unsafe { GetModuleHandleA(s!("dinput8.dll")).expect("dinput8.dll not loaded") };
    let di8_create: DInput8CreateFn = unsafe {
        std::mem::transmute(
            GetProcAddress(dinput8, s!("DirectInput8Create"))
                .expect("DirectInput8Create not found"),
        )
    };
    let hinstance = unsafe { GetModuleHandleA(None).expect("GetModuleHandle failed").0 as usize };

    let mut keyboard_addr = 0usize;
    let mut mouse_addr = 0usize;

    unsafe {
        with_probe_device(di8_create, hinstance, &GUID_SYS_KEYBOARD, |a| {
            keyboard_addr = a
        })
    };
    unsafe { with_probe_device(di8_create, hinstance, &GUID_SYS_MOUSE, |a| mouse_addr = a) };

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
    Ok(())
}
