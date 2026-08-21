//! Zero-dependency Win32 FFI surface for the input-harness DLL.
//!
//! Mirrors the raw-`extern`/`#[link]` style of `er-reload-trace-dll` (no `windows`-crate
//! dependency, so nothing extra crosses the cargo-xwin cross-compile boundary). Only the calls the
//! DIRECT-input-memory self-drive uses are declared: module resolution (find the game image),
//! timing/log helpers, and `ReadProcessMemory` for fault-safe
//! game-memory reads. There is deliberately NO `SendInput`/`XInput`/window-focus surface: those were
//! the dead path (user, 2026-07-19) -- ER menu/gameplay input is driven by writing the game's own
//! input memory (CSMenuMan keystate bitmap + DLUID input-active flag), never synthesized OS input.

use std::ffi::c_void;

pub const CURRENT_PROCESS_PSEUDO_HANDLE: isize = -1;

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetModuleHandleA(name: *const u8) -> *mut c_void;
    pub fn GetTickCount64() -> u64;
    pub fn ReadProcessMemory(
        process: isize,
        base: *const c_void,
        buffer: *mut c_void,
        size: usize,
        read: *mut usize,
    ) -> i32;
    pub fn WriteProcessMemory(
        process: isize,
        base: *const c_void,
        buffer: *const c_void,
        size: usize,
        written: *mut usize,
    ) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    pub fn keybd_event(vk: u8, scan: u8, flags: u32, extra: usize);
    pub fn GetForegroundWindow() -> *mut c_void;
    pub fn GetWindowThreadProcessId(hwnd: *mut c_void, pid: *mut u32) -> u32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetCurrentProcessId() -> u32;
}

/// True when the FOREGROUND window belongs to THIS process (i.e. the ER game window is focused). The
/// focus gate for OS-synthesized input (bd SYNTHESIS-pause-menu-is-scaleform): keyboard events are
/// system-wide and route to the focused window, so we only ever send when ER is foreground -- never into
/// the user's other windows.
pub fn er_window_is_foreground() -> bool {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return false;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    pid != 0 && pid == unsafe { GetCurrentProcessId() }
}

/// `KEYEVENTF_KEYUP` -- the only `keybd_event` flag this surface needs. Used solely by `send_key_up`.
#[allow(dead_code)]
const KEYEVENTF_KEYUP: u32 = 0x0002;

/// Focus-gated OS key DOWN (hold) -- for a sustained press (movement test: hold W). Returns true if sent.
pub fn send_key_down(vk: u8) -> bool {
    if !er_window_is_foreground() {
        return false;
    }
    unsafe { keybd_event(vk, 0, 0, 0) };
    true
}

/// Focus-gated OS key UP (release) -- pairs with `send_key_down`. Always sent (release is safe even if the
/// window lost focus mid-hold, to avoid a stuck key).
///
/// RETAINED THOUGH CURRENTLY UNCALLED: this is the release half of `send_key_down`, which IS live (the
/// OSMOVE probe in `crate::drive` holds VK_W with it). Nothing calls this today, which means that probe
/// currently holds W without ever releasing it -- deleting the release path would remove the only way to
/// fix that, so the item stays and the gap stays visible.
#[allow(dead_code)]
pub fn send_key_up(vk: u8) {
    unsafe { keybd_event(vk, 0, KEYEVENTF_KEYUP, 0) };
}

/// Read a pointer-sized value from this process's own address space. Uses `ReadProcessMemory` on the
/// pseudo-handle (never faults on an unmapped/garbage pointer, unlike a raw deref) -- the same passive
/// read idiom `er-reload-trace-dll` uses.
pub unsafe fn read_usize(addr: usize) -> Option<usize> {
    let mut value = 0usize;
    let mut read = 0usize;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            (&mut value as *mut usize).cast(),
            std::mem::size_of::<usize>(),
            &mut read,
        )
    };
    (ok != 0 && read == std::mem::size_of::<usize>()).then_some(value)
}

/// Write a single byte to this process's own address space via `WriteProcessMemory` (fault-safe: returns
/// false instead of crashing on a stale/unmapped pointer). Used to stamp the input array without a raw
/// deref that would fault the game thread if the target was reallocated.
pub unsafe fn write_u8(addr: usize, value: u8) -> bool {
    let mut wrote = 0usize;
    let ok = unsafe {
        WriteProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            (&value as *const u8).cast(),
            1,
            &mut wrote,
        )
    };
    ok != 0 && wrote == 1
}

/// Read a single byte from this process's own address space (fault-safe). Used to confirm a keystate
/// bitmap / DLUID flag byte is READABLE before writing it, so a not-yet-initialized singleton pointer
/// can never fault the game thread.
pub unsafe fn read_u8(addr: usize) -> Option<u8> {
    let mut value = 0u8;
    let mut read = 0usize;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            (&mut value as *mut u8).cast(),
            std::mem::size_of::<u8>(),
            &mut read,
        )
    };
    (ok != 0 && read == std::mem::size_of::<u8>()).then_some(value)
}
