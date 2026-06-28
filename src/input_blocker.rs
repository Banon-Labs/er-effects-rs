use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::OnceLock;

use bitflags::bitflags;
use ilhook::x64::*;
use windows::core::{s, GUID};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

pub use ilhook::HookError;

static INPUT_BLOCKER: OnceLock<&'static InputBlocker> = OnceLock::new();
static INJECTED_KEY: AtomicU8 = AtomicU8::new(0);

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
    pub unsafe fn install_hooks(&self) -> Result<(), HookError> {
        if self.hooks_installed.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        unsafe { install_dinput_hooks() }
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

fn make_get_state_closure(
    flags: InputFlags,
) -> impl Fn(*mut ilhook::x64::Registers, usize) -> usize {
    move |reg, original| {
        let size = unsafe { (*reg).rdx };
        let data = unsafe { (*reg).r8 as *mut u8 };

        let original: unsafe extern "system" fn(u64, u64, u64) -> usize =
            unsafe { std::mem::transmute(original) };
        let hr = unsafe { original((*reg).rcx, size, data as u64) };

        if hr == 0 && is_blocked(flags) {
            unsafe { std::ptr::write_bytes(data, 0, size as usize) };
            if flags.contains(InputFlags::Keyboard) {
                const DIK_PRESSED: u8 = 0x80;
                let dik = INJECTED_KEY.load(Ordering::Relaxed);
                if dik != 0 && (dik as u64) < size {
                    unsafe { *data.add(dik as usize) = DIK_PRESSED };
                }
            }
        }
        hr
    }
}

unsafe fn install_dinput_hooks() -> Result<(), ilhook::HookError> {
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

    let kb_flags = if keyboard_addr == mouse_addr {
        InputFlags::Keyboard | InputFlags::Mouse
    } else {
        InputFlags::Keyboard
    };

    let kb_hook = unsafe {
        hook_closure_retn(
            keyboard_addr,
            make_get_state_closure(kb_flags),
            CallbackOption::None,
            HookFlags::empty(),
        )?
    };

    let mut hooks = vec![kb_hook];

    if keyboard_addr != mouse_addr {
        let ms_hook = unsafe {
            hook_closure_retn(
                mouse_addr,
                make_get_state_closure(InputFlags::Mouse),
                CallbackOption::None,
                HookFlags::empty(),
            )?
        };
        hooks.push(ms_hook);
    }

    std::mem::forget(hooks);
    Ok(())
}
