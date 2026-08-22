//! ffi module (split from lib.rs; pure code reorganization, no behavior change).

use std::ffi::c_void;

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, experiments::*, hooks::*, telemetry::*};

#[cfg(windows)]
unsafe extern "system" {
    pub(crate) fn RtlCaptureStackBackTrace(
        frames_to_skip: u32,
        frames_to_capture: u32,
        backtrace: *mut *mut c_void,
        backtrace_hash: *mut u32,
    ) -> u16;
}

/// Vectored exception handler signature: receives EXCEPTION_POINTERS, returns a
/// disposition (EXCEPTION_CONTINUE_SEARCH to leave behavior unchanged).
pub(crate) type VectoredHandler = unsafe extern "system" fn(*mut ExceptionPointersMin) -> i32;

#[cfg(windows)]
unsafe extern "system" {
    pub(crate) fn AddVectoredExceptionHandler(first: u32, handler: VectoredHandler) -> *mut c_void;
}

// USER32 window-management calls used to replicate the boot-movie WNDPROC's
// WM_CLOSE teardown natively (no posted message, no input): hide + repaint the
// dedicated movie window so the intro thread's fade/quiesce completes cleanly.
// HWND is passed as a raw pointer read from the movie object (M+8).
#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    #[allow(dead_code)]
    // Retained: USER32 declaration for the boot-movie WM_CLOSE teardown block; no live caller today.
    pub(crate) fn GetSystemMenu(hwnd: *mut c_void, brevert: i32) -> *mut c_void;
    #[allow(dead_code)]
    // Retained: USER32 declaration for the boot-movie WM_CLOSE teardown block; no live caller today.
    pub(crate) fn DeleteMenu(hmenu: *mut c_void, uposition: u32, uflags: u32) -> i32;
    #[allow(dead_code)]
    // Retained: USER32 declaration for the boot-movie WM_CLOSE teardown block; no live caller today.
    pub(crate) fn ShowWindow(hwnd: *mut c_void, ncmdshow: i32) -> i32;
    #[allow(dead_code)]
    // Retained: USER32 declaration for the boot-movie WM_CLOSE teardown block; no live caller today.
    pub(crate) fn UpdateWindow(hwnd: *mut c_void) -> i32;
    pub(crate) fn VirtualProtect(
        addr: *mut c_void,
        size: usize,
        new_protect: u32,
        old_protect: *mut u32,
    ) -> i32;
    /// Flush the CPU instruction cache after patching executable code so threads see the
    /// new bytes (current-process pseudo-handle -1; null base + 0 size = whole process).
    pub(crate) fn FlushInstructionCache(process: isize, base: *const c_void, size: usize) -> i32;
    /// Fault-tolerant read: returns FALSE on unmapped/freed memory instead of
    /// raising an access violation -- used by the title-owner scan so the TOCTOU
    /// race against the booting game (a region freed between VirtualQuery and the
    /// deref) cannot crash the process.
    #[allow(dead_code)]
    // Retained: Fault-tolerant read declaration; the live callers now go through er-game-base's copy.
    pub(crate) fn ReadProcessMemory(
        process: isize,
        base: *const c_void,
        buffer: *mut c_void,
        size: usize,
        read: *mut usize,
    ) -> i32;
}

/// EXCEPTION_RECORD (Win64) through `ExceptionInformation`. The access-violation logger
/// reads `exception_information[0]` (0=read/1=write/8=execute) and `[1]` (the faulting
/// virtual address) so a crash line records the exact bad address, not just the RIP.
/// `EXCEPTION_MAXIMUM_PARAMETERS` is 15; repr(C) inserts the 4-byte pad after
/// `number_parameters` so `exception_information` lands at the ABI offset 0x20.
#[repr(C)]
pub(crate) struct ExceptionRecordMin {
    pub(crate) exception_code: u32,
    pub(crate) exception_flags: u32,
    pub(crate) next_record: *mut ExceptionRecordMin,
    pub(crate) exception_address: *mut c_void,
    pub(crate) number_parameters: u32,
    pub(crate) exception_information: [usize; 15],
}

/// Minimal EXCEPTION_POINTERS: the record pointer + the CONTEXT pointer (the
/// CONTEXT is read/modified by the hardware-watchpoint single-step handler to
/// read Dr6/Rip and one-shot-disarm Dr7).
#[repr(C)]
pub(crate) struct ExceptionPointersMin {
    pub(crate) exception_record: *mut ExceptionRecordMin,
    pub(crate) context_record: *mut c_void,
}

/// THREADENTRY32 (ToolHelp): only the fields the watchpoint arming reads/sets.
#[repr(C)]
pub(crate) struct ThreadEntry32 {
    pub(crate) dw_size: u32,
    pub(crate) cnt_usage: u32,
    pub(crate) th32_thread_id: u32,
    pub(crate) th32_owner_process_id: u32,
    pub(crate) tpbase_pri: i32,
    pub(crate) delta_pri: i32,
    pub(crate) dw_flags: u32,
}

/// SYSTEMTIME (kernel32 `GetLocalTime`): all eight `WORD` fields, used to stamp every DLL log line
/// with a wall-clock time so a run is unambiguous in the accumulated log (the `[+Nms]` epoch resets
/// per process and cannot tell two runs apart).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct SystemTimeMin {
    pub(crate) w_year: u16,
    pub(crate) w_month: u16,
    pub(crate) w_day_of_week: u16,
    pub(crate) w_day: u16,
    pub(crate) w_hour: u16,
    pub(crate) w_minute: u16,
    pub(crate) w_second: u16,
    pub(crate) w_milliseconds: u16,
}

// Wall-clock + own-module-path FFI for the self-describing log header (local time per line + the
// DLL's on-disk path so its bytes can be md5'd to name the exact build in the log).
#[cfg(windows)]
unsafe extern "system" {
    pub(crate) fn GetLocalTime(time: *mut SystemTimeMin);
    pub(crate) fn GetModuleFileNameW(module: isize, filename: *mut u16, size: u32) -> u32;
    /// Clean process exit (flushes open file buffers on the way out, so a quit-save that has already
    /// issued its write persists). Used to turn the quit-to-desktop teardown crash into a fast, clean
    /// kill. Runs through our exit-path hook (logs the exit) before terminating.
    pub(crate) fn ExitProcess(code: u32) -> !;
}

// Thread + debug-register FFI for the GameMan+0xc30 hardware write-watchpoint:
// enumerate the game's threads (ToolHelp) and set DR0/DR7 on each via
// Suspend/Get/SetThreadContext so the writing instruction traps into our VEH.
#[cfg(windows)]
unsafe extern "system" {
    pub(crate) fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> isize;
    pub(crate) fn Thread32First(snapshot: isize, entry: *mut ThreadEntry32) -> i32;
    pub(crate) fn Thread32Next(snapshot: isize, entry: *mut ThreadEntry32) -> i32;
    pub(crate) fn OpenThread(access: u32, inherit: i32, thread_id: u32) -> isize;
    pub(crate) fn SuspendThread(thread: isize) -> u32;
    pub(crate) fn ResumeThread(thread: isize) -> u32;
    pub(crate) fn GetThreadContext(thread: isize, context: *mut c_void) -> i32;
    pub(crate) fn SetThreadContext(thread: isize, context: *const c_void) -> i32;
    pub(crate) fn GetCurrentThreadId() -> u32;
    pub(crate) fn CloseHandle(handle: isize) -> i32;
}
