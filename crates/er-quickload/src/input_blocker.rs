use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use bitflags::bitflags;
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::core::{GUID, s};

use crate::mh::{MH_STATUS, UnionFn, register_union_hook};

static INPUT_BLOCKER: OnceLock<&'static InputBlocker> = OnceLock::new();
pub use er_telemetry_core::counters::DINPUT_INJECTED_KEY_STAMPS;
/// DIAGNOSTIC: how many times the game actually calls the DInput keyboard `GetDeviceState`
/// (i.e. whether native ER reads keyboard input via DInput at all). If the keyboard counter stays 0
/// while the harness holds, ER does not read keyboard via DInput on native -> our `set_injected_key`
/// stamp never reaches the game and a different injection path (WM_KEYDOWN / RawInput) is required.
pub use er_telemetry_core::counters::DINPUT_KB_HOOK_FIRES;
pub(crate) use er_telemetry_core::counters::DINPUT_SUPPRESSED_ARROW_KEYS;
pub(crate) use er_telemetry_core::counters::INJECTED_KEY;
pub(crate) use er_telemetry_core::counters::SUPPRESS_ARROW_KEYS;

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
    #[allow(dead_code)] // InputBlocker API surface, retained: no in-crate caller today.
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

    #[allow(dead_code)] // InputBlocker API surface, retained: no in-crate caller today.
    pub fn block(&self, inputs: InputFlags) {
        self.flags.fetch_or(inputs.bits(), Ordering::Relaxed);
    }

    pub fn block_only(&self, inputs: InputFlags) {
        self.flags.store(inputs.bits(), Ordering::Relaxed);
    }

    #[allow(dead_code)] // InputBlocker API surface, retained: no in-crate caller today.
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
    #[allow(dead_code)] // InputBlocker API surface, retained: no in-crate caller today.
    pub fn set_arrow_key_suppression(&self, enabled: bool) {
        SUPPRESS_ARROW_KEYS.store(enabled, Ordering::Relaxed);
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct InputFlags: u8 {
        const GamePad  = 0b001;
        const Keyboard = 0b010;
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

pub(crate) use er_telemetry_core::counters::DINPUT_KB_GET_STATE_ORIG;

/// The keyboard detour, in the hook union's four-`usize` shape.
///
/// `DINPUT_KB_GET_STATE_ORIG` may hold the next handler in the chain rather than the game
/// trampoline, so it is called through [`UnionFn`] and not through the narrower three-argument
/// `GetDeviceState` signature. The `usize` return carries the `HRESULT` in its low 32 bits, which
/// is where the caller reads it from, so it is passed straight back.
unsafe extern "system" fn dinput_kb_get_state_hook(
    device: usize,
    size: usize,
    data: usize,
    unused: usize,
) -> usize {
    DINPUT_KB_HOOK_FIRES.fetch_add(1, Ordering::Relaxed);
    let next = DINPUT_KB_GET_STATE_ORIG.load(Ordering::Relaxed);
    if next == 0 {
        return 0;
    }
    let call: UnionFn = unsafe { std::mem::transmute::<usize, UnionFn>(next) };
    let raw = unsafe { call(device, size, data, unused) };
    let (hr, size, data) = (raw as i32, size as u32, data as *mut u8);
    // The stamp must PRECEDE the latch (2026-09-05). The save-file picker does not read this buffer
    // when the game does -- it LATCHES a copy here, and the copy used to be taken before
    // `stamp_injected_dinput_key` wrote anything into it. So the picker was the one consumer in the
    // process structurally incapable of seeing an injected key: the pause menu, which reads the
    // buffer later, consumed injected Up and E perfectly (cell 0 -> 6, then the System pane opened),
    // while an injected Right inside the file browser changed nothing in any of the fourteen live
    // GridControls -- measured by diffing them across the press on br-20260905-181031-f1a0. Both
    // halves of that comparison came from the same run and the same channel, so the only variable
    // left was which side of this line the consumer sat on.
    zero_blocked_dinput_state(hr, size, data, InputFlags::Keyboard);
    if hr == 0 && !data.is_null() {
        crate::experiments::save_picker_latch_dinput_keyboard_state(
            data as *const u8,
            size as usize,
        );
    }
    raw
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

    if is_blocked(flags) {
        unsafe { std::ptr::write_bytes(data, 0, size) };
    }

    // Focus-independent keyboard injection (2026-09-05). The stamp used to live inside the
    // `is_blocked` branch, so the harness could only put a key in front of the game while the input
    // block was engaged -- and the can-move probe never engages it in-world, so its key never landed.
    // Worse, the probe's actual delivery path was `SendInput` -> RawInput, which ER cannot read:
    // `eldenring.exe` (1.17) imports no RawInput API at all -- not `GetRawInputData`, not
    // `GetRawInputBuffer`, not `RegisterRawInputDevices` (the string is absent from the image; the
    // USER32 import block carries `GetKeyState`/`GetKeyboardState`/`ToAscii` and `DINPUT8`'s
    // `DirectInput8Create`). That is why run br-20260905-031610-5406 reported
    // `supplied_movement_input_frames=150` beside `rawinput_key_events=0`: 150 SendInputs the game
    // had no code path to observe. The 4325 `GetRawInputData` calls our counter saw are the overlay's,
    // not the game's.
    //
    // Stamping here is focus-independent by construction: it runs after DInput has filled (or, when
    // the window is not foreground, zeroed) the buffer, so the byte the game reads is ours either way.
    // No `SetForegroundWindow`, no focus theft -- the point of the change.
    stamp_injected_dinput_key(size, data, flags);
}

/// Write the harness's injected DirectInput scancode into the keyboard buffer the game is about to
/// read. Runs on every keyboard `GetDeviceState` regardless of the input block, so injection does not
/// depend on either the block being engaged or ER holding the keyboard focus. A zero `INJECTED_KEY`
/// (the resting value) stamps nothing, so this is inert outside an active injection window.
fn stamp_injected_dinput_key(size: usize, data: *mut u8, flags: InputFlags) {
    if !flags.contains(InputFlags::Keyboard) || size < DINPUT_KEYBOARD_BUFFER_LEN {
        return;
    }
    const DIK_PRESSED: u8 = 0x80;
    let dik = INJECTED_KEY.load(Ordering::Relaxed);
    if dik == 0 || (dik as usize) >= DINPUT_KEYBOARD_BUFFER_LEN {
        return;
    }
    unsafe { *data.add(dik as usize) = DIK_PRESSED };
    DINPUT_INJECTED_KEY_STAMPS.fetch_add(1, Ordering::Relaxed);
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
    }
}

const DINPUT_KEYBOARD_BUFFER_LEN: usize = 256;

unsafe fn install_dinput_hooks() -> Result<(), MH_STATUS> {
    let dinput8 = unsafe { GetModuleHandleA(s!("dinput8.dll")).expect("dinput8.dll not loaded") };
    let di8_create: DInput8CreateFn = unsafe {
        std::mem::transmute(
            GetProcAddress(dinput8, s!("DirectInput8Create"))
                .expect("DirectInput8Create not found"),
        )
    };
    let hinstance = unsafe { GetModuleHandleA(None).expect("GetModuleHandle failed").0 as usize };

    let mut keyboard_addr = 0usize;

    unsafe {
        with_probe_device(di8_create, hinstance, &GUID_SYS_KEYBOARD, |a| {
            keyboard_addr = a
        })
    };

    // Through the union, never a bare `MhHook`. `er-net-effects` and `er-enemynpc-effects`
    // detour this same `GetDeviceState` slot, and each links its own MinHook instance -- two
    // instances on one prologue overwrite each other's trampolines and the loser silently never
    // runs. Owning it in the union is also what lets those DLLs chain in through this DLL's
    // `er_effects_union_register` export. See the [[shared]] rows in
    // scripts/me3-dll-conflicts.toml.
    unsafe {
        register_union_hook(
            keyboard_addr,
            dinput_kb_get_state_hook,
            &DINPUT_KB_GET_STATE_ORIG,
        )?
    };
    Ok(())
}
