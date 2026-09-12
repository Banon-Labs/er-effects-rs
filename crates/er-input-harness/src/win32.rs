//! Zero-dependency Win32 FFI surface for the input-harness DLL.
//!
//! Mirrors the raw-`extern`/`#[link]` style of `er-reload-trace` (no `windows`-crate
//! dependency, so nothing extra crosses the cargo-xwin cross-compile boundary). Only the calls the
//! direct-input-memory self-drive uses are declared: module resolution (find the game image),
//! timing/log helpers, and `ReadProcessMemory` for fault-safe
//! game-memory reads. There is deliberately no `SendInput`/`XInput`/window-focus surface: those were
//! the dead path (user, 2026-07-19) -- ER menu/gameplay input is driven by writing the game's own
//! input memory (CSMenuMan keystate bitmap + DLUID input-active flag), never synthesized OS input.

use std::ffi::c_void;

pub const CURRENT_PROCESS_PSEUDO_HANDLE: isize = -1;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetModuleHandleA(name: *const u8) -> *mut c_void;
    pub fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
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

/// Host stand-ins for the nine Win32 entry points above, so this crate compiles and its pure logic
/// can be unit-tested off Windows.
///
/// Without them the `#[link]` blocks are emitted on a Linux host too and the test binary fails to
/// link with `unable to find library -lkernel32` -- which is how `cargo test -p er-input-harness`
/// came to report a build failure rather than a test result, leaving the crate with no host gate at
/// all. Each stub answers the way an unmapped process would: no module, no memory, no window. That
/// is the correct host semantics as well as the convenient one, because every caller already has a
/// "this is not readable" path and takes it.
#[cfg(not(windows))]
#[allow(
    non_snake_case,
    reason = "these stand in for Win32 entry points and must keep their names"
)]
mod host_stubs {
    use std::ffi::c_void;

    pub unsafe fn GetModuleHandleA(_name: *const u8) -> *mut c_void {
        std::ptr::null_mut()
    }
    pub unsafe fn GetProcAddress(_module: *mut c_void, _name: *const u8) -> *mut c_void {
        std::ptr::null_mut()
    }
    pub unsafe fn GetTickCount64() -> u64 {
        0
    }
    pub unsafe fn ReadProcessMemory(
        _process: isize,
        _base: *const c_void,
        _buffer: *mut c_void,
        _size: usize,
        _read: *mut usize,
    ) -> i32 {
        0
    }
    pub unsafe fn WriteProcessMemory(
        _process: isize,
        _base: *const c_void,
        _buffer: *const c_void,
        _size: usize,
        _written: *mut usize,
    ) -> i32 {
        0
    }
    pub unsafe fn keybd_event(_vk: u8, _scan: u8, _flags: u32, _extra: usize) {}
    pub unsafe fn GetForegroundWindow() -> *mut c_void {
        std::ptr::null_mut()
    }
    pub unsafe fn GetWindowThreadProcessId(_hwnd: *mut c_void, _pid: *mut u32) -> u32 {
        0
    }
    pub unsafe fn GetCurrentProcessId() -> u32 {
        0
    }
}

#[cfg(not(windows))]
pub use host_stubs::{
    GetCurrentProcessId, GetForegroundWindow, GetModuleHandleA, GetProcAddress, GetTickCount64,
    GetWindowThreadProcessId, ReadProcessMemory, WriteProcessMemory, keybd_event,
};

/// True when the foreground window belongs to this process (i.e. the ER game window is focused). The
/// focus gate for OS-synthesized input (bd synthesis-pause-menu-is-scaleform): keyboard events are
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

/// Focus-gated OS key down (hold) -- for a sustained press (movement test: hold W). Returns true if sent.
pub fn send_key_down(vk: u8) -> bool {
    if !er_window_is_foreground() {
        return false;
    }
    unsafe { keybd_event(vk, 0, 0, 0) };
    true
}

/// Focus-gated OS key up (release) -- pairs with `send_key_down`. Always sent (release is safe even if the
/// window lost focus mid-hold, to avoid a stuck key).
///
/// Retained though currently UNCALLED: this is the release half of `send_key_down`, which is live (the
/// OSMOVE probe in `crate::drive` holds VK_W with it). Nothing calls this today, which means that probe
/// currently holds W without ever releasing it -- deleting the release path would remove the only way to
/// fix that, so the item stays and the gap stays visible.
#[allow(dead_code)]
pub fn send_key_up(vk: u8) {
    unsafe { keybd_event(vk, 0, KEYEVENTF_KEYUP, 0) };
}

/// Read a pointer-sized value from this process's own address space. Uses `ReadProcessMemory` on the
/// pseudo-handle (never faults on an unmapped/garbage pointer, unlike a raw deref) -- the same passive
/// read idiom `er-reload-trace` uses.
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

/// Read a 32-bit value from this process's own address space (fault-safe, same `ReadProcessMemory`
/// idiom as [`read_usize`]). Needed wherever a struct field is genuinely a dword: reading one with
/// `read_usize` pulls in the next field's bytes as the high half, which is harmless when the caller
/// truncates and wrong when the field is the last one in the entry.
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

/// Write a 32-bit value into this process's own address space (fault-safe, `WriteProcessMemory`).
/// Used to drive the menu pointer, where the field is a dword and a pointer-sized write would
/// clobber the neighbouring coordinate.
pub unsafe fn write_i32(addr: usize, value: i32) -> bool {
    let mut wrote = 0usize;
    let ok = unsafe {
        WriteProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            (&value as *const i32).cast(),
            std::mem::size_of::<i32>(),
            &mut wrote,
        )
    };
    ok != 0 && wrote == std::mem::size_of::<i32>()
}

/// Read a run of bytes from this process's own address space in one call (fault-safe, same
/// `ReadProcessMemory` idiom). Returns how many bytes were read, which is 0 on an unmapped page.
///
/// One call rather than a loop of [`read_u8`] because the caller is the RTTI name reader, and a
/// mangled class name is up to 160 bytes: read byte by byte that is 160 syscalls per candidate
/// object, on the game thread, inside a graph walk that visits dozens of objects.
pub unsafe fn read_bytes(addr: usize, out: &mut [u8]) -> usize {
    let mut read = 0usize;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            out.as_mut_ptr().cast(),
            out.len(),
            &mut read,
        )
    };
    if ok != 0 { read.min(out.len()) } else { 0 }
}

/// Read a single byte from this process's own address space (fault-safe). Used to confirm a keystate
/// bitmap / DLUID flag byte is readable before writing it, so a not-yet-initialized singleton pointer
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
