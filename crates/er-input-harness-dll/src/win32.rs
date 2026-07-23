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
