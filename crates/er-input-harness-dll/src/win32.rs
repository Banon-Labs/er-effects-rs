//! Zero-dependency Win32 FFI surface for the input-harness DLL.
//!
//! Mirrors the raw-`extern`/`#[link]` style of `er-reload-trace-dll` (no `windows`-crate
//! dependency, so nothing extra crosses the cargo-xwin cross-compile boundary). Only the calls the
//! DIRECT-input-memory self-drive uses are declared: module/proc resolution (find the game image and
//! the product's union export), timing/log helpers, and `ReadProcessMemory` for fault-safe
//! game-memory reads. There is deliberately NO `SendInput`/`XInput`/window-focus surface: those were
//! the dead path (user, 2026-07-19) -- ER menu/gameplay input is driven by writing the game's own
//! input memory (CSMenuMan keystate bitmap + DLUID input-active flag), never synthesized OS input.

#![allow(non_snake_case)]

use std::ffi::c_void;

pub const CURRENT_PROCESS_PSEUDO_HANDLE: isize = -1;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetModuleHandleA(name: *const u8) -> *mut c_void;
    pub fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    pub fn Sleep(ms: u32);
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

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    pub fn keybd_event(vk: u8, scan: u8, flags: u32, extra: usize);
    pub fn GetForegroundWindow() -> *mut c_void;
    pub fn GetWindowThreadProcessId(hwnd: *mut c_void, pid: *mut u32) -> u32;
}
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetCurrentProcessId() -> u32;
}

#[cfg(not(windows))]
pub unsafe fn GetModuleHandleA(_name: *const u8) -> *mut c_void {
    std::ptr::null_mut()
}

#[cfg(not(windows))]
pub unsafe fn GetProcAddress(_module: *mut c_void, _name: *const u8) -> *mut c_void {
    std::ptr::null_mut()
}

#[cfg(not(windows))]
pub fn Sleep(_ms: u32) {}

#[cfg(not(windows))]
pub fn GetTickCount64() -> u64 {
    0
}

#[cfg(not(windows))]
pub unsafe fn ReadProcessMemory(
    _process: isize,
    _base: *const c_void,
    _buffer: *mut c_void,
    _size: usize,
    read: *mut usize,
) -> i32 {
    if !read.is_null() {
        unsafe { *read = 0 };
    }
    0
}

#[cfg(not(windows))]
pub unsafe fn WriteProcessMemory(
    _process: isize,
    _base: *const c_void,
    _buffer: *const c_void,
    _size: usize,
    written: *mut usize,
) -> i32 {
    if !written.is_null() {
        unsafe { *written = 0 };
    }
    0
}

#[cfg(not(windows))]
pub unsafe fn keybd_event(_vk: u8, _scan: u8, _flags: u32, _extra: usize) {}

#[cfg(not(windows))]
pub unsafe fn GetForegroundWindow() -> *mut c_void {
    std::ptr::null_mut()
}

#[cfg(not(windows))]
pub unsafe fn GetWindowThreadProcessId(_hwnd: *mut c_void, pid: *mut u32) -> u32 {
    if !pid.is_null() {
        unsafe { *pid = 0 };
    }
    0
}

#[cfg(not(windows))]
pub unsafe fn GetCurrentProcessId() -> u32 {
    0
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

const KEYEVENTF_KEYUP: u32 = 0x0002;

/// Send ONE OS keyboard tap (down+up) of virtual-key `vk`, but ONLY when the ER window is foreground
/// (`er_window_is_foreground`). Returns true if the key was sent. Real input path -> reaches Scaleform.
pub fn send_key_tap(vk: u8) -> bool {
    if !er_window_is_foreground() {
        return false;
    }
    unsafe {
        keybd_event(vk, 0, 0, 0);
        keybd_event(vk, 0, KEYEVENTF_KEYUP, 0);
    }
    true
}

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

/// Read a 32-bit unsigned integer from this process's own address space (fault-safe).
pub unsafe fn read_u32(addr: usize) -> Option<u32> {
    let mut value = 0u32;
    let mut read = 0usize;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            (&mut value as *mut u32).cast(),
            std::mem::size_of::<u32>(),
            &mut read,
        )
    };
    (ok != 0 && read == std::mem::size_of::<u32>()).then_some(value)
}

/// Read a 32-bit float from this process's own address space (fault-safe).
pub unsafe fn read_f32(addr: usize) -> Option<f32> {
    let mut value = 0f32;
    let mut read = 0usize;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            (&mut value as *mut f32).cast(),
            std::mem::size_of::<f32>(),
            &mut read,
        )
    };
    (ok != 0 && read == std::mem::size_of::<f32>()).then_some(value)
}
