//! The observation surface: every input-read API a mod can plausibly bind a hotkey to.
//!
//! # The constraint that shapes all of this
//!
//! It must work against any author's DLL. A third-party mod exports nothing you can query, has no
//! config you can parse and will never cooperate. The only general mechanism left is to sit on the
//! APIs everybody has to call and record who called them -- so this DLL hooks the input functions
//! and attributes each call to the module it came from.
//!
//! # Passive means passive
//!
//! Every detour here has the same body: call the rest of the chain first, then record what was
//! asked for, then return the chain's value byte for byte. Nothing is swallowed, nothing is
//! altered, nothing is injected, and no buffer is written. The bookkeeping cannot even delay the
//! call, because it happens after the value is already in hand.
//!
//! # Through the union, never a private MinHook
//!
//! `er-effects-rs`, `er-net-effects` and `er-charm-enemies` all detour the DirectInput
//! `GetDeviceState` slot. A fourth private MinHook instance on that prologue would overwrite
//! somebody's trampoline and silently disable their feature -- which is exactly the class of bug
//! this DLL was written to report on, and it would be absurd to cause one. So every registration
//! goes through [`er_hook::register_shared_hook`], which chains into the single instance the
//! product DLL owns when the product is loaded, and into this DLL's own union when it is not.
//!
//! # What this cannot see, stated where it is implemented
//!
//! * A mod that reads `inputmgr+0x90+eventId` directly -- the game's own decoded per-action
//!   keystate bitmap -- calls no API at all. Catching that needs a data breakpoint on the bitmap,
//!   which means stealing the debug registers process-wide and taking every write through a
//!   vectored handler. That is not a passive observer, so it is not done. It is the one input
//!   path in this file's list with no coverage.
//! * A mod that reads raw input (`WM_INPUT` / `GetRawInputData`) from its own subclassed window
//!   procedure. Reachable in principle; not hooked here.

#![cfg(windows)]

use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use er_hook::{HookRoute, MH_STATUS, UnionFn, register_shared_hook_with_budget};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::core::{GUID, PCSTR, s};

use crate::attribution::{Frames, MAX_FRAMES, RawTally};
use crate::census::{InputId, Surface};
use crate::dik::KEYBOARD_STATE_BYTES;
use crate::log::conflict_log;
use crate::modules;

/// Poll budget for the union-export lookup when installing from this DLL's own thread. me3 loads
/// natives in profile order and nothing guarantees the product comes first, so a companion's
/// install thread can outrun the product's `LoadLibrary`.
const INSTALL_RESOLVE_TRIES: u32 = 40;
const INSTALL_RESOLVE_SLEEP_MS: u32 = 25;

/// The same, from a game frame: by then every native in the profile is loaded, so one probe is
/// already the right answer and polling would be a stall on the game thread.
const FRAME_RESOLVE_TRIES: u32 = 1;
const FRAME_RESOLVE_SLEEP_MS: u32 = 0;

/// `DIMOUSESTATE` and `DIMOUSESTATE2`. Sizes, not a `<=`: a `DIJOYSTATE2` is 272 bytes and an
/// inequality would let joystick axes be counted as mouse buttons.
const MOUSE_STATE_BYTES: [usize; 2] = [16, 20];

/// After the report has been printed, attribute only one call in this many.
///
/// The stack walk is the expensive part of an observation, and once the report exists its only
/// remaining job is to catch a hotkey that is polled for the first time an hour in -- a mod whose
/// key only works inside a menu. One in sixteen finds that within a second of it happening and
/// costs the game essentially nothing for the rest of the session.
const POST_REPORT_ATTRIBUTION_DIVISOR: u64 = 16;

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
const GUID_SYS_MOUSE: GUID = GUID::from_values(
    0x6F1D2B60,
    0xD5A0,
    0x11CF,
    [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00],
);

type RawObj = *mut *const usize;
type DInput8CreateFn =
    unsafe extern "system" fn(usize, u32, *const GUID, *mut RawObj, usize) -> i32;
type CreateDeviceFn = unsafe extern "system" fn(RawObj, *const GUID, *mut RawObj, usize) -> i32;
type ReleaseFn = unsafe extern "system" fn(RawObj) -> u32;

/// Everything observed and not yet folded into a report.
static RAW: Mutex<RawTally> = Mutex::new(RawTally::new());

/// Calls seen, including ones that were not attributed. Read by the settle gate, which must not
/// take the tally lock on every frame to ask a question this answers.
static CALLS: AtomicUsize = AtomicUsize::new(0);

/// Set once the report has been printed; switches attribution to the sampled rate.
static REPORTED: AtomicBool = AtomicBool::new(false);

/// The DirectInput keyboard state as the game will receive it, captured at this DLL's position in
/// the handler chain. Fed to the consumption check; see [`crate::dik`] for what that can and
/// cannot prove.
static KEYBOARD_SNAPSHOT: Mutex<[u8; KEYBOARD_STATE_BYTES]> = Mutex::new([0; KEYBOARD_STATE_BYTES]);

/// Frames on which the snapshot was refreshed. Zero means no DirectInput keyboard read has been
/// seen, which makes any consumption verdict meaningless rather than negative.
static SNAPSHOT_UPDATES: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// True while this DLL is the one calling an input API -- the periodic physical-key scan the
    /// consumption check needs. Without it the scan would observe itself and the census would
    /// report this DLL as a mod that binds every key on the keyboard.
    static IN_SELF_PROBE: Cell<bool> = const { Cell::new(false) };
}

/// Run `body` with this DLL's own input calls excluded from the census.
///
/// `try_with` throughout, never `with`: `with` PANICS once a thread's local storage is being torn
/// down, and this flag is read from inside a detour that any thread in the process can enter --
/// including one on its way out. A panic there unwinds into the game's own input poll across an
/// `extern "system"` boundary, which aborts the process. Failing to set the flag merely records
/// one of this DLL's own probe calls in the census, and the report says how many calls it could
/// not trace; a dead game says nothing at all.
pub fn without_observing<T>(body: impl FnOnce() -> T) -> T {
    let _ = IN_SELF_PROBE.try_with(|flag| flag.set(true));
    let value = body();
    let _ = IN_SELF_PROBE.try_with(|flag| flag.set(false));
    value
}

/// Record one observed call, unless it is ours.
fn observe(surface: Surface, input: InputId) {
    if IN_SELF_PROBE.try_with(Cell::get).unwrap_or(false) {
        return;
    }
    let seen = CALLS.fetch_add(1, Ordering::Relaxed) as u64;
    if REPORTED.load(Ordering::Relaxed) && !seen.is_multiple_of(POST_REPORT_ATTRIBUTION_DIVISOR) {
        return;
    }
    let mut frames = [0usize; MAX_FRAMES];
    let captured = modules::capture_frames(&mut frames);
    // `try_lock`, never `lock`: this runs inside the game's own input poll, and blocking there to
    // record a diagnostic would make an observer that changes what it observes. A dropped sample
    // is counted in an atomic -- retrying the lock to record the failure to take the lock would be
    // absurd -- and the count is folded in when the tally is next read.
    match RAW.try_lock() {
        Ok(mut raw) => raw.record(surface, input, Frames::from_slice(&frames[..captured])),
        Err(_) => {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Observations the hot path could not store because the tally was momentarily locked.
static DROPPED: AtomicUsize = AtomicUsize::new(0);

/// Calls observed so far.
pub fn calls_seen() -> u64 {
    CALLS.load(Ordering::Relaxed) as u64
}

/// Switch to the sampled attribution rate. Called once, when the report is printed.
pub fn mark_reported() {
    REPORTED.store(true, Ordering::Relaxed);
}

/// Take a copy of the tally for folding. The tally is not cleared: attribution depends on the
/// longest common prefix over every chain seen, so a later report must be able to reconsider
/// earlier calls under a better-informed prefix.
pub fn snapshot_tally() -> RawTally {
    let mut tally = RAW
        .lock()
        .map(|raw| raw.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
    tally.record_dropped(DROPPED.load(Ordering::Relaxed) as u64);
    tally
}

/// The DirectInput keyboard state most recently seen, and how many reads have been seen at all.
pub fn keyboard_snapshot() -> Option<[u8; KEYBOARD_STATE_BYTES]> {
    if SNAPSHOT_UPDATES.load(Ordering::Relaxed) == 0 {
        return None;
    }
    KEYBOARD_SNAPSHOT.try_lock().ok().map(|state| *state)
}

// ============================================================================
// the DETOURS. Every one: chain, then record, then return the chain's value untouched.
// ============================================================================

macro_rules! chain {
    ($orig:expr, $a:expr, $b:expr, $c:expr, $d:expr) => {{
        let next = $orig.load(Ordering::Relaxed);
        if next == 0 {
            return 0;
        }
        // SAFETY: the slot holds either the game trampoline or the next handler in the union
        // chain, both of which are `UnionFn`-shaped by the registrar's contract.
        let call: UnionFn = unsafe { std::mem::transmute::<usize, UnionFn>(next) };
        unsafe { call($a, $b, $c, $d) }
    }};
}

static ASYNC_KEY_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static KEY_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static KEYBOARD_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static REGISTER_HOTKEY_ORIG: AtomicUsize = AtomicUsize::new(0);
static SET_HOOK_W_ORIG: AtomicUsize = AtomicUsize::new(0);
static SET_HOOK_A_ORIG: AtomicUsize = AtomicUsize::new(0);
static DINPUT_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static DINPUT_MOUSE_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static XINPUT_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);

/// A virtual-key code of zero is not a key; `GetAsyncKeyState(0)` is a no-op some code does to
/// flush the per-thread "pressed since" bits, and counting it would put a phantom binding on every
/// module that does.
const VK_NONE: usize = 0;

unsafe extern "system" fn hook_get_async_key_state(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let result = chain!(ASYNC_KEY_STATE_ORIG, a, b, c, d);
    if a != VK_NONE {
        observe(Surface::AsyncKeyState, InputId::Key(a as u16));
    }
    result
}

unsafe extern "system" fn hook_get_key_state(a: usize, b: usize, c: usize, d: usize) -> usize {
    let result = chain!(KEY_STATE_ORIG, a, b, c, d);
    if a != VK_NONE {
        observe(Surface::KeyState, InputId::Key(a as u16));
    }
    result
}

unsafe extern "system" fn hook_get_keyboard_state(a: usize, b: usize, c: usize, d: usize) -> usize {
    let result = chain!(KEYBOARD_STATE_ORIG, a, b, c, d);
    observe(Surface::KeyboardState, InputId::WholeKeyboard);
    result
}

/// `RegisterHotKey(hWnd, id, fsModifiers, vk)` -- the virtual key is the fourth argument, so this
/// is the one surface that names a key without any inference at all.
unsafe extern "system" fn hook_register_hotkey(a: usize, b: usize, c: usize, d: usize) -> usize {
    let result = chain!(REGISTER_HOTKEY_ORIG, a, b, c, d);
    if d != VK_NONE {
        observe(Surface::RegisterHotKey, InputId::Key(d as u16));
    }
    result
}

/// `WH_KEYBOARD_LL`, from winuser.h.
const WH_KEYBOARD_LL: usize = 13;
/// `WH_MOUSE_LL`.
const WH_MOUSE_LL: usize = 14;

/// `SetWindowsHookEx(idHook, lpfn, hMod, dwThreadId)`.
///
/// A low-level keyboard hook sees every keystroke in the process, so its installer is a claimant
/// on the entire keyboard. The hook procedure's address (`lpfn`) names the installing module
/// directly, which is better evidence than the stack walk -- but the observation still goes
/// through the same path so that both agree or the disagreement is visible in the chain dump.
fn observe_windows_hook(id_hook: usize) {
    match id_hook {
        WH_KEYBOARD_LL => observe(Surface::LowLevelKeyboardHook, InputId::WholeKeyboard),
        WH_MOUSE_LL => observe(Surface::LowLevelMouseHook, InputId::WholeMouse),
        _ => {}
    }
}

unsafe extern "system" fn hook_set_windows_hook_w(a: usize, b: usize, c: usize, d: usize) -> usize {
    let result = chain!(SET_HOOK_W_ORIG, a, b, c, d);
    observe_windows_hook(a);
    result
}

unsafe extern "system" fn hook_set_windows_hook_a(a: usize, b: usize, c: usize, d: usize) -> usize {
    let result = chain!(SET_HOOK_A_ORIG, a, b, c, d);
    observe_windows_hook(a);
    result
}

/// `IDirectInputDevice8::GetDeviceState(this, cbData, lpvData)`.
///
/// The device class is decided by the buffer size, not by which slot was hooked: devices of
/// different classes can share one vtable implementation, so a mouse arrives at the keyboard slot
/// with a 16-byte `DIMOUSESTATE`. Reading scancode offsets out of one finds noise.
fn observe_device_state(hr: usize, size: usize, data: usize) {
    if size == KEYBOARD_STATE_BYTES {
        observe(Surface::DirectInputKeyboard, InputId::WholeKeyboard);
        // Snapshot only a successful read; `DIERR_INPUTLOST` leaves the buffer untouched and a
        // stale copy would read as "every key was taken".
        if hr as i32 == 0
            && data != 0
            && let Ok(mut snapshot) = KEYBOARD_SNAPSHOT.try_lock()
        {
            // SAFETY: DirectInput has just filled `size` bytes at `data`, and `size` was checked
            // to be exactly the destination's length.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data as *const u8,
                    snapshot.as_mut_ptr(),
                    KEYBOARD_STATE_BYTES,
                );
            }
            SNAPSHOT_UPDATES.fetch_add(1, Ordering::Relaxed);
        }
    } else if MOUSE_STATE_BYTES.contains(&size) {
        observe(Surface::DirectInputMouse, InputId::WholeMouse);
    }
}

unsafe extern "system" fn hook_get_device_state(a: usize, b: usize, c: usize, d: usize) -> usize {
    let result = chain!(DINPUT_GET_STATE_ORIG, a, b, c, d);
    observe_device_state(result, b, c);
    result
}

/// The mouse device's own slot, when it is a different address from the keyboard's.
unsafe extern "system" fn hook_get_device_state_mouse(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let result = chain!(DINPUT_MOUSE_GET_STATE_ORIG, a, b, c, d);
    observe_device_state(result, b, c);
    result
}

unsafe extern "system" fn hook_xinput_get_state(a: usize, b: usize, c: usize, d: usize) -> usize {
    let result = chain!(XINPUT_GET_STATE_ORIG, a, b, c, d);
    observe(Surface::XInput, InputId::WholeGamepad);
    result
}

// ============================================================================
// installation
// ============================================================================

/// Which surfaces are armed, in the order they are attempted. Reported verbatim so a reader can
/// see what the verdict actually covers.
static ARMED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
static MISSED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
static USER32_DONE: AtomicBool = AtomicBool::new(false);
static DINPUT_DONE: AtomicBool = AtomicBool::new(false);
static XINPUT_DONE: AtomicBool = AtomicBool::new(false);

/// The product DLL as me3 loads it. Named here because a detour registered through its union has
/// the product's `union_dispatch` frame sitting between the patched prologue and our handler, and
/// [`crate::attribution`] has to strip exactly that frame -- clause 2 of its rule.
///
/// `er_hook` knows the same name privately for its own module probe; this is the one place it has
/// to be spelled twice, because [`HookRoute`] says which union answered and not what it is called.
const PRODUCT_MODULE_NAME: &str = "er_effects_rs.dll";

/// Set when a detour was taken by the product's union rather than by this DLL's own.
static ON_PRODUCT_UNION: AtomicBool = AtomicBool::new(false);

/// The module whose dispatcher frame stands between the patched prologue and our handlers.
///
/// `None` for this DLL's own union: that frame is in our own image, and clause 1 of the
/// attribution rule has already stripped it.
pub fn union_host_module() -> Option<&'static str> {
    ON_PRODUCT_UNION
        .load(Ordering::Relaxed)
        .then_some(PRODUCT_MODULE_NAME)
}

fn note_armed(label: &'static str, route: HookRoute, address: usize) {
    if route == HookRoute::ProductUnion {
        ON_PRODUCT_UNION.store(true, Ordering::Relaxed);
    }
    if let Ok(mut armed) = ARMED.lock() {
        armed.push(label);
    }
    conflict_log(format_args!(
        "hook: {label} armed at 0x{address:x} via {route:?}"
    ));
}

fn note_missed(label: &'static str, status: MH_STATUS) {
    if let Ok(mut missed) = MISSED.lock()
        && !missed.contains(&label)
    {
        missed.push(label);
        conflict_log(format_args!(
            "hook: {label} NOT armed ({status:?}) -- bindings through it are invisible to the \
             report"
        ));
    }
}

/// Surfaces successfully hooked.
pub fn armed_surfaces() -> Vec<String> {
    ARMED
        .lock()
        .map(|armed| armed.iter().map(|label| (*label).to_string()).collect())
        .unwrap_or_default()
}

/// Surfaces that could not be hooked. Printed in the report, because an unhooked API is a hole in
/// the verdict and a verdict with an unstated hole is a false clean.
pub fn missed_surfaces() -> Vec<String> {
    MISSED
        .lock()
        .map(|missed| missed.iter().map(|label| (*label).to_string()).collect())
        .unwrap_or_default()
}

fn arm(
    label: &'static str,
    address: usize,
    handler: UnionFn,
    slot: &'static AtomicUsize,
    tries: u32,
    sleep_ms: u32,
) -> bool {
    // SAFETY: every handler above matches the union's four-`usize` shape, and each `slot` is the
    // static its own handler reads to reach the rest of the chain.
    match unsafe { register_shared_hook_with_budget(address, handler, slot, tries, sleep_ms) } {
        Ok(route) => {
            note_armed(label, route, address);
            true
        }
        Err(status) => {
            note_missed(label, status);
            false
        }
    }
}

/// Resolve an export in an already-loaded module. Never `LoadLibrary`: bringing a DLL into the
/// process that was not already there is a change, and this DLL does not make changes.
fn export_of(module: PCSTR, symbol: PCSTR) -> Option<usize> {
    // SAFETY: both arguments are NUL-terminated literals.
    let handle = unsafe { GetModuleHandleA(module) }.ok()?;
    // SAFETY: `handle` came from the loader and `symbol` is NUL-terminated.
    unsafe { GetProcAddress(handle, symbol) }.map(|address| address as usize)
}

/// Hook the user32 surfaces. They are available from attach, so this runs on the install thread.
pub fn install_user32() {
    if USER32_DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let surfaces: [(&'static str, PCSTR, UnionFn, &'static AtomicUsize); 6] = [
        (
            "GetAsyncKeyState",
            s!("GetAsyncKeyState"),
            hook_get_async_key_state,
            &ASYNC_KEY_STATE_ORIG,
        ),
        (
            "GetKeyState",
            s!("GetKeyState"),
            hook_get_key_state,
            &KEY_STATE_ORIG,
        ),
        (
            "GetKeyboardState",
            s!("GetKeyboardState"),
            hook_get_keyboard_state,
            &KEYBOARD_STATE_ORIG,
        ),
        (
            "RegisterHotKey",
            s!("RegisterHotKey"),
            hook_register_hotkey,
            &REGISTER_HOTKEY_ORIG,
        ),
        (
            "SetWindowsHookExW",
            s!("SetWindowsHookExW"),
            hook_set_windows_hook_w,
            &SET_HOOK_W_ORIG,
        ),
        (
            "SetWindowsHookExA",
            s!("SetWindowsHookExA"),
            hook_set_windows_hook_a,
            &SET_HOOK_A_ORIG,
        ),
    ];
    for (label, symbol, handler, slot) in surfaces {
        match export_of(s!("user32.dll"), symbol) {
            Some(address) => {
                arm(
                    label,
                    address,
                    handler,
                    slot,
                    INSTALL_RESOLVE_TRIES,
                    INSTALL_RESOLVE_SLEEP_MS,
                );
            }
            None => note_missed(label, MH_STATUS::MH_ERROR_FUNCTION_NOT_FOUND),
        }
    }
}

/// # Safety
/// `object` must be a live COM object whose vtable has at least `slot + 1` entries.
unsafe fn vtable_fn<F: Copy>(object: RawObj, slot: usize) -> F {
    unsafe { std::mem::transmute_copy(&*(*object).add(slot)) }
}

/// Create a throwaway device of `class` only to read its `GetDeviceState` slot address, then
/// release it. Every DirectInput device of a class shares one vtable, so the probe's slot address
/// is the address the game's own device will call.
unsafe fn probe_device_state_address(
    create: DInput8CreateFn,
    hinstance: usize,
    class: &GUID,
) -> Option<usize> {
    let mut di8: RawObj = std::ptr::null_mut();
    // SAFETY: a documented DirectInput entry point with a live out-param.
    let hr = unsafe {
        create(
            hinstance,
            DIRECTINPUT_VERSION,
            &IID_IDIRECTINPUT8W,
            &mut di8,
            0,
        )
    };
    if hr != 0 || di8.is_null() {
        return None;
    }
    // SAFETY: `di8` is a live `IDirectInput8W`.
    let create_device: CreateDeviceFn = unsafe { vtable_fn(di8, VTBL_CREATE_DEVICE) };
    // SAFETY: same object.
    let release_di8: ReleaseFn = unsafe { vtable_fn(di8, VTBL_RELEASE) };
    let mut device: RawObj = std::ptr::null_mut();
    // SAFETY: a documented vtable call with a live out-param.
    let hr = unsafe { create_device(di8, class, &mut device, 0) };
    if hr != 0 || device.is_null() {
        // SAFETY: releasing the object this function created.
        unsafe { release_di8(di8) };
        return None;
    }
    // SAFETY: `device` is live and its vtable has the DirectInput device layout.
    let address = unsafe { *(*device).add(VTBL_GET_DEVICE_STATE) as usize };
    // SAFETY: releasing the two objects this function created, in creation order.
    unsafe {
        let release_device: ReleaseFn = vtable_fn(device, VTBL_RELEASE);
        release_device(device);
        release_di8(di8);
    }
    Some(address)
}

/// Hook the DirectInput device state slots. `dinput8.dll` is not loaded at attach, so this is
/// retried from the game frame until it succeeds.
pub fn try_install_dinput() -> bool {
    if DINPUT_DONE.load(Ordering::SeqCst) {
        return true;
    }
    let Some(create_address) = export_of(s!("dinput8.dll"), s!("DirectInput8Create")) else {
        return false;
    };
    // SAFETY: the export's signature is fixed by the DirectInput 8 ABI.
    let create: DInput8CreateFn =
        unsafe { std::mem::transmute::<usize, DInput8CreateFn>(create_address) };
    // SAFETY: `None` asks for the process's own image base.
    let Ok(hinstance) = (unsafe { GetModuleHandleA(None) }) else {
        return false;
    };
    let hinstance = hinstance.0 as usize;
    // SAFETY: a live DirectInput8Create and the process image base.
    let keyboard = unsafe { probe_device_state_address(create, hinstance, &GUID_SYS_KEYBOARD) };
    let Some(keyboard) = keyboard else {
        return false;
    };
    DINPUT_DONE.store(true, Ordering::SeqCst);
    // Written out rather than routed through `arm`, deliberately. This is the one prologue three
    // other shells in this workspace also detour, so the registration it uses is a declared fact
    // recorded in the `[[shared]]` rows of scripts/me3-dll-conflicts.toml -- and
    // `check-shared-hook-rvas.py` proves the claim by looking for the registrar beside the
    // handler. Hiding it behind a helper would put the proof out of the gate's reach and let a
    // future edit revert it to a private MinHook instance without anything going red.
    // SAFETY: the handler has the union's four-`usize` shape and the slot is its own static.
    let keyboard_route = unsafe {
        register_shared_hook_with_budget(
            keyboard,
            hook_get_device_state,
            &DINPUT_GET_STATE_ORIG,
            FRAME_RESOLVE_TRIES,
            FRAME_RESOLVE_SLEEP_MS,
        )
    };
    match keyboard_route {
        Ok(route) => note_armed("DirectInput GetDeviceState", route, keyboard),
        Err(status) => note_missed("DirectInput GetDeviceState", status),
    }
    // SAFETY: as above, for the mouse class.
    if let Some(mouse) = unsafe { probe_device_state_address(create, hinstance, &GUID_SYS_MOUSE) }
        && mouse != keyboard
    {
        // SAFETY: same contract as the keyboard slot.
        let mouse_route = unsafe {
            register_shared_hook_with_budget(
                mouse,
                hook_get_device_state_mouse,
                &DINPUT_MOUSE_GET_STATE_ORIG,
                FRAME_RESOLVE_TRIES,
                FRAME_RESOLVE_SLEEP_MS,
            )
        };
        match mouse_route {
            Ok(route) => note_armed("DirectInput GetDeviceState(mouse)", route, mouse),
            Err(status) => note_missed("DirectInput GetDeviceState(mouse)", status),
        }
    }
    true
}

/// Every XInput redistributable a process might have loaded. Whichever one is already mapped is
/// the one the game and its mods are calling; the rest are not brought in.
const XINPUT_MODULES: [PCSTR; 5] = [
    s!("xinput1_4.dll"),
    s!("xinput1_3.dll"),
    s!("xinput9_1_0.dll"),
    s!("xinput1_2.dll"),
    s!("xinput1_1.dll"),
];

/// Hook `XInputGetState` in whichever XInput module is loaded. Retried from the game frame: a pad
/// plugged in mid-session brings the module in late.
pub fn try_install_xinput() -> bool {
    if XINPUT_DONE.load(Ordering::SeqCst) {
        return true;
    }
    for module in XINPUT_MODULES {
        let Some(address) = export_of(module, s!("XInputGetState")) else {
            continue;
        };
        XINPUT_DONE.store(true, Ordering::SeqCst);
        arm(
            "XInputGetState",
            address,
            hook_xinput_get_state,
            &XINPUT_GET_STATE_ORIG,
            FRAME_RESOLVE_TRIES,
            FRAME_RESOLVE_SLEEP_MS,
        );
        return true;
    }
    false
}

/// Record the surfaces that never became available, so the report can say so rather than imply
/// they were clean. Called once, when the report is rendered.
pub fn finalise_missed() {
    if !DINPUT_DONE.load(Ordering::SeqCst) {
        note_missed(
            "DirectInput GetDeviceState",
            MH_STATUS::MH_ERROR_MODULE_NOT_FOUND,
        );
    }
    if !XINPUT_DONE.load(Ordering::SeqCst) {
        note_missed("XInputGetState", MH_STATUS::MH_ERROR_MODULE_NOT_FOUND);
    }
}
