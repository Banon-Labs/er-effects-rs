use std::{
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Instant,
};

use er_hook::{MH_STATUS, UnionFn, register_shared_hook_with_budget};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::core::{GUID, s};

use crate::{
    bindings::{self, CURSOR_SLOT_MASK},
    dinput_state, effects,
    hold_repeat::HoldRepeat,
    log::net_effects_log,
    selector_gate::{self, SelectorKey},
};

/// Is the selector list on screen and able to act? See [`crate::selector_gate`] -- this is not
/// "the bar exists": a bar minimized to its `[+]` button is closed, and so is one on the title
/// screen, and neither may touch the player's keys.
static SELECTOR_OPEN: AtomicBool = AtomicBool::new(false);
static HOOKS_INSTALLED: AtomicBool = AtomicBool::new(false);
static DINPUT_KB_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static DINPUT_MOUSE_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static DINPUT_KB_ALSO_MOUSE: AtomicBool = AtomicBool::new(false);
static DINPUT_KB_HOOK_FIRES: AtomicUsize = AtomicUsize::new(0);
static DINPUT_MOUSE_HOOK_FIRES: AtomicUsize = AtomicUsize::new(0);
static DINPUT_SUPPRESSED_ARROW_KEYS: AtomicUsize = AtomicUsize::new(0);
/// Set while the mouse pointer sits inside the overlay's minimize/maximize button.
static POINTER_OVER_OVERLAY: AtomicBool = AtomicBool::new(false);
/// Mouse reads whose left button was blanked because the pointer was over that button. Elden
/// Ring polls the mouse through DirectInput, so without this a click on the overlay is also a
/// weapon swing.
static DINPUT_SUPPRESSED_MOUSE_CLICKS: AtomicUsize = AtomicUsize::new(0);
static DINPUT_PREVIOUS_SELECTOR_KEYS: AtomicUsize = AtomicUsize::new(0);
static DINPUT_QUEUED_SELECTOR_KEYS: AtomicUsize = AtomicUsize::new(0);
static DINPUT_REPEATED_SELECTOR_KEYS: AtomicUsize = AtomicUsize::new(0);
/// State reads through the hooked vtable entry that were not the keyboard's DIK table. A live
/// count well above zero is the shared-vtable case, and every one of these used to manufacture a
/// phantom key release.
static DINPUT_NON_KEYBOARD_READS: AtomicUsize = AtomicUsize::new(0);
static DINPUT_REPEAT_STATE: OnceLock<Mutex<HoldRepeat>> = OnceLock::new();

/// A single non-blocking probe for the product DLL's union export. The install is driven from the
/// FrameBegin tick, and by the time a game frame runs every native in the profile is loaded, so
/// there is nothing left to poll for -- and polling on the game thread would be a visible stall.
const FRAME_DRIVEN_RESOLVE_TRIES: u32 = 1;
const FRAME_DRIVEN_RESOLVE_SLEEP_MS: u32 = 0;

const DIRECTINPUT_VERSION: u32 = 0x0800;
const DIK_LEFT_ALT: usize = 0x38;
const DIK_RIGHT_ALT: usize = 0xb8;

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

/// Publish whether the selector is open to the DirectInput detours.
///
/// The hooks install on the first call whatever the value is: they are how the mouse click on the
/// overlay button is kept out of the game, and how a later opening is noticed at all. What `open`
/// changes is only what the installed hook does -- closed, it forwards every read untouched.
pub(crate) fn set_selector_open(open: bool) {
    SELECTOR_OPEN.store(open, Ordering::Relaxed);
    if !HOOKS_INSTALLED.load(Ordering::Relaxed) {
        // Retried from the caller's FrameBegin tick until `dinput8.dll` is loaded -- no sleep, and
        // no install thread: a game frame is already proof that every native in the profile has
        // been loaded, which is what the union resolve would otherwise be polling for.
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

fn selector_open() -> bool {
    SELECTOR_OPEN.load(Ordering::Relaxed)
}

pub(crate) fn dinput_kb_hook_fires() -> usize {
    DINPUT_KB_HOOK_FIRES.load(Ordering::Relaxed)
}

pub(crate) fn dinput_mouse_hook_fires() -> usize {
    DINPUT_MOUSE_HOOK_FIRES.load(Ordering::Relaxed)
}

pub(crate) fn dinput_suppressed_mouse_clicks() -> usize {
    DINPUT_SUPPRESSED_MOUSE_CLICKS.load(Ordering::Relaxed)
}

/// Tell the DirectInput hooks whether the pointer is currently over the overlay's button.
pub(crate) fn set_pointer_over_overlay(over: bool) {
    POINTER_OVER_OVERLAY.store(over, Ordering::Relaxed);
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

pub(crate) fn dinput_non_keyboard_reads() -> usize {
    DINPUT_NON_KEYBOARD_READS.load(Ordering::Relaxed)
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

/// The keyboard detour, in the hook union's four-`usize` shape.
///
/// `DINPUT_KB_GET_STATE_ORIG` may hold the next handler in the chain rather than the game
/// trampoline, so it is called through [`UnionFn`] and not through the narrower
/// three-argument `GetDeviceState` signature. The `usize` return carries the `HRESULT` in its low 32 bits, which is
/// where the caller reads it from, so it is passed straight back.
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
    // Only a 256-byte DIK table is keyboard state. The mouse (and any other device sharing this
    // vtable entry) hands us a buffer with no key bytes in it, which reads as "every arrow
    // released" and re-arms the press on the very next keyboard poll -- see `dinput_state`.
    if dinput_state::is_keyboard_state(size) {
        queue_dinput_selector_edges(hr, size, data);
        zero_dinput_arrow_state(hr, size, data);
    } else {
        DINPUT_NON_KEYBOARD_READS.fetch_add(1, Ordering::Relaxed);
        // Same vtable entry, so the mouse arrives here too and needs the same click blanking as
        // the dedicated mouse hook below.
        blank_overlay_mouse_click(hr, size, data);
    }
    raw
}

/// The mouse detour, in the same union shape and for the same reason as the keyboard one above.
unsafe extern "system" fn dinput_mouse_get_state_hook(
    device: usize,
    size: usize,
    data: usize,
    unused: usize,
) -> usize {
    DINPUT_MOUSE_HOOK_FIRES.fetch_add(1, Ordering::Relaxed);
    let next = DINPUT_MOUSE_GET_STATE_ORIG.load(Ordering::Relaxed);
    if next == 0 {
        return 0;
    }
    let call: UnionFn = unsafe { std::mem::transmute::<usize, UnionFn>(next) };
    let raw = unsafe { call(device, size, data, unused) };
    blank_overlay_mouse_click(raw as i32, size as u32, data as *mut u8);
    raw
}

/// Blank the left mouse button in a DirectInput mouse read while the pointer is over the
/// overlay's minimize/maximize button.
///
/// The click still reaches imgui -- hudhook feeds that from the window procedure, which this
/// never touches -- so the button works while the swing it would otherwise trigger does not.
fn blank_overlay_mouse_click(hr: i32, size: u32, data: *mut u8) {
    if hr < 0 || data.is_null() || !POINTER_OVER_OVERLAY.load(Ordering::Relaxed) {
        return;
    }
    if !dinput_state::is_mouse_state(size) {
        return;
    }
    let button = unsafe { data.add(dinput_state::MOUSE_BUTTON0_OFFSET) };
    if unsafe { *button } & 0x80 == 0 {
        return;
    }
    unsafe { *button = 0 };
    DINPUT_SUPPRESSED_MOUSE_CLICKS.fetch_add(1, Ordering::Relaxed);
}

fn dinput_key_down(size: u32, data: *mut u8, offset: usize) -> bool {
    !data.is_null() && size as usize > offset && unsafe { *data.add(offset) & 0x80 } != 0
}

/// Forget every key edge, because the BINDINGS moved.
///
/// `DINPUT_PREVIOUS_SELECTOR_KEYS` is positional: bit 3 means "cursor right" only for as long as
/// the bindings that produced it are in force. Carry it across a rebind and a key held at that
/// instant either swallows its own press (the bit says it was already down) or manufactures one
/// (its bit now belongs to a different key). The hold-to-repeat ramp is dropped for the same
/// reason -- it is mid-hold on a key that is no longer bound.
pub(crate) fn forget_key_edges_after_rebind() {
    reset_dinput_repeat_state();
}

fn reset_dinput_repeat_state() {
    DINPUT_PREVIOUS_SELECTOR_KEYS.store(0, Ordering::Relaxed);
    if let Some(state) = DINPUT_REPEAT_STATE.get()
        && let Ok(mut state) = state.lock()
    {
        state.reset();
    }
}

fn queue_dinput_selector_edges(hr: i32, size: u32, data: *mut u8) {
    if hr != 0 || data.is_null() {
        reset_dinput_repeat_state();
        return;
    }

    let open = selector_open();

    // One snapshot for the whole poll. Taking it once rather than per key means a reload landing
    // mid-poll cannot classify the first half of the buffer against one binding table and the
    // second half against another.
    let bindings = bindings::live();

    let alt_down =
        dinput_key_down(size, data, DIK_LEFT_ALT) || dinput_key_down(size, data, DIK_RIGHT_ALT);
    let mut pressed_mask = 0usize;
    for key in bindings.keys() {
        // A key bound to something DirectInput cannot report simply is not in this buffer. It
        // still reaches the selector through the low-level keyboard hook.
        if let Some(offset) = key.chord.scancode_offset()
            && dinput_key_down(size, data, offset)
        {
            pressed_mask |= key.bit();
        }
    }

    let previous_mask = DINPUT_PREVIOUS_SELECTOR_KEYS.swap(pressed_mask, Ordering::Relaxed);
    let new_edges = pressed_mask & !previous_mask;

    let mut queued = 0usize;
    let mut ignored = 0usize;
    for key in bindings.keys() {
        if new_edges & key.bit() == 0 || (key.chord.needs_alt() && !alt_down) {
            continue;
        }
        // The gate is asked per key, not once for the poll: the show/hide chord has to keep
        // working while the rest of this table is deaf, or a hidden bar could never be brought
        // back.
        let classified = selector_gate::key_for_vk_in(&bindings, key.chord.vk, alt_down);
        if !selector_gate::should_handle_key(open, classified) {
            ignored = ignored.saturating_add(1);
            continue;
        }
        effects::queue_effect_keyboard_vk(key.chord.vk, alt_down);
        queued = queued.saturating_add(1);
    }
    if ignored != 0 {
        effects::record_keys_ignored_while_closed(ignored);
    }
    let repeated = queue_held_arrow_repeats(&bindings, open, pressed_mask, new_edges);
    queued = queued.saturating_add(repeated);
    if repeated != 0 {
        DINPUT_REPEATED_SELECTOR_KEYS.fetch_add(repeated, Ordering::SeqCst);
    }
    if queued != 0 {
        DINPUT_QUEUED_SELECTOR_KEYS.fetch_add(queued, Ordering::SeqCst);
    }
}

fn queue_held_arrow_repeats(
    bindings: &bindings::SelectorBindings,
    open: bool,
    pressed_mask: usize,
    new_edges: usize,
) -> usize {
    let held_arrows = pressed_mask & CURSOR_SLOT_MASK;
    if held_arrows == 0 || !selector_gate::should_handle_key(open, SelectorKey::Arrow) {
        if let Some(state) = DINPUT_REPEAT_STATE.get()
            && let Ok(mut state) = state.lock()
        {
            state.reset();
        }
        return 0;
    }

    let now = Instant::now();
    let mut queued = 0usize;
    let Ok(mut state) = DINPUT_REPEAT_STATE
        .get_or_init(|| Mutex::new(HoldRepeat::default()))
        .lock()
    else {
        return 0;
    };

    // The press itself was already queued by the caller as the single step. This only decides
    // whether a hold owes another one: latch, then a steady one-at-a-time cadence, and only
    // after a long stretch of that does it start to speed up. See `hold_repeat`.
    // The cursor slots are the repeat indices: `bindings::slot::CURSOR_UP..CURSOR_RIGHT` are 0..3
    // by construction, which is what lets a per-direction hold state be looked up by slot.
    for key in bindings.keys() {
        if key.bit() & CURSOR_SLOT_MASK == 0 {
            continue;
        }
        let held = held_arrows & key.bit() != 0;
        let edge = new_edges & key.bit() != 0;
        if state.observe(key.slot, held, edge, now) {
            effects::queue_effect_keyboard_vk(key.chord.vk, false);
            queued = queued.saturating_add(1);
        }
    }

    queued
}

/// Blank the arrow keys out of a DirectInput keyboard read -- but only while the selector is open.
///
/// This is the hard taking: the game polls this table for menu navigation and quick-item switching,
/// so a byte zeroed here is a key the player pressed and the game never saw. Closed, the buffer is
/// returned exactly as the original produced it.
fn zero_dinput_arrow_state(hr: i32, size: u32, data: *mut u8) {
    if hr != 0
        || data.is_null()
        || !selector_gate::should_consume_key(selector_open(), SelectorKey::Arrow)
    {
        return;
    }
    let mut cleared = 0usize;
    // Whatever the cursor keys are bound to, and nothing else. The per-offset bounds check
    // replaced a single `size <= DIK_DOWN` guard on the fixed table: with the offsets now coming
    // from config, one of them could sit past the end of a short buffer while the others do not,
    // and a fixed guard would either wave that through or refuse the whole poll.
    for offset in bindings::live().cursor_scancodes() {
        let offset = usize::from(offset);
        if size as usize <= offset {
            continue;
        }
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

    // Through a union registrar, never a bare `MhHook`. `er-quickload` (its input blocker) and
    // `er-enemynpc-effects` (its hotkey) detour this same `GetDeviceState` slot, and two
    // separately linked MinHook instances on one prologue overwrite each other's trampolines --
    // the loser reports installed and never runs. See the [[shared]] row for this pair in
    // scripts/me3-dll-conflicts.toml.
    let kb_route = unsafe {
        register_shared_hook_with_budget(
            keyboard_addr,
            dinput_kb_get_state_hook,
            &DINPUT_KB_GET_STATE_ORIG,
            FRAME_DRIVEN_RESOLVE_TRIES,
            FRAME_DRIVEN_RESOLVE_SLEEP_MS,
        )?
    };
    net_effects_log(format_args!(
        "input-suppression: keyboard GetDeviceState detour at 0x{keyboard_addr:x} via {kb_route:?}"
    ));

    if keyboard_addr == mouse_addr {
        DINPUT_KB_ALSO_MOUSE.store(true, Ordering::Relaxed);
    } else {
        let mouse_route = unsafe {
            register_shared_hook_with_budget(
                mouse_addr,
                dinput_mouse_get_state_hook,
                &DINPUT_MOUSE_GET_STATE_ORIG,
                FRAME_DRIVEN_RESOLVE_TRIES,
                FRAME_DRIVEN_RESOLVE_SLEEP_MS,
            )?
        };
        net_effects_log(format_args!(
            "input-suppression: mouse GetDeviceState detour at 0x{mouse_addr:x} via {mouse_route:?}"
        ));
    }

    // The registrar owns both detours for the life of the process; nothing uninstalls them.
    HOOKS_INSTALLED.store(true, Ordering::Relaxed);
    Ok(())
}
