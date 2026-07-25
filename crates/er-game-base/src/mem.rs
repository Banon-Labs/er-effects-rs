//! Tier A: fault-safe RAM readers + game module base / RVA resolution.
//!
//! Implemented over raw `#[link(name = "kernel32")]` externs so this stays a
//! zero-dependency leaf that all three DLLs (product, reload-trace, input-harness)
//! and er-telemetry can sit on without re-implementing `ReadProcessMemory` reads.
//! Ported from the product's `experiments/mem.rs` (single source of truth now).

use core::ffi::c_void;

/// `-1` cast to a handle: the current-process pseudo-handle accepted by
/// `ReadProcessMemory` without an `OpenProcess` round-trip.
const CURRENT_PROCESS_PSEUDO_HANDLE: isize = -1;
/// `ReadProcessMemory` returns a Win32 `BOOL`; zero means failure.
const RPM_FALSE: i32 = 0;
/// Init sentinel for the out-params / accumulators (was
/// `TITLE_OWNER_SCAN_START_ADDRESS` in the product tree; it is simply 0).
const ZERO: usize = 0;

unsafe extern "system" {
    fn GetModuleHandleA(module_name: *const u8) -> isize;
    fn ReadProcessMemory(
        process: isize,
        base_address: *const c_void,
        buffer: *mut c_void,
        size: usize,
        bytes_read: *mut usize,
    ) -> i32;
}

/// Resolve the running game module's base address (`GetModuleHandleA(NULL)`).
pub fn game_module_base() -> Result<usize, String> {
    let module = unsafe { GetModuleHandleA(core::ptr::null()) };
    if module == 0 {
        return Err("failed to resolve game module: GetModuleHandleA(NULL) returned 0".to_string());
    }
    Ok(module as usize)
}

/// `game_module_base() + rva`.
pub fn game_rva(rva: u32) -> Result<usize, String> {
    Ok(game_module_base()? + rva as usize)
}

/// Cheap heap-pointer sanity check: above the low 64 KiB reserve and 8-byte aligned.
pub unsafe fn is_heap_aligned_ptr(ptr: usize) -> bool {
    const HEAP_LO: usize = 0x10000;
    const PTR_ALIGN_MASK: usize = 0x7;
    ptr >= HEAP_LO && (ptr & PTR_ALIGN_MASK) == ZERO
}

/// True if `vtable` falls inside the game image span `[base+0x1000, base+0x3000000)`.
pub fn vtable_in_game_image(vtable: usize, base: usize) -> bool {
    const MODULE_MIN_OFFSET: usize = 0x1000;
    const MODULE_SPAN_FALLBACK: usize = 0x3000000;
    vtable >= base + MODULE_MIN_OFFSET && vtable < base + MODULE_SPAN_FALLBACK
}

/// Fault-tolerant pointer-sized read: returns `None` on unmapped/freed memory
/// instead of raising an access violation.
pub unsafe fn safe_read_usize(addr: usize) -> Option<usize> {
    let mut value: usize = ZERO;
    let mut read: usize = ZERO;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut usize as *mut c_void,
            core::mem::size_of::<usize>(),
            &mut read,
        )
    };
    if ok != RPM_FALSE && read == core::mem::size_of::<usize>() {
        Some(value)
    } else {
        None
    }
}

/// Fault-tolerant i32 read (None on unmapped memory).
pub unsafe fn safe_read_i32(addr: usize) -> Option<i32> {
    let mut value: i32 = 0;
    let mut read: usize = ZERO;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut i32 as *mut c_void,
            core::mem::size_of::<i32>(),
            &mut read,
        )
    };
    if ok != RPM_FALSE && read == core::mem::size_of::<i32>() {
        Some(value)
    } else {
        None
    }
}

/// Fault-tolerant f32 read (None on unmapped memory).
pub unsafe fn safe_read_f32(addr: usize) -> Option<f32> {
    let mut value: f32 = 0.0;
    let mut read: usize = ZERO;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut f32 as *mut c_void,
            core::mem::size_of::<f32>(),
            &mut read,
        )
    };
    if ok != RPM_FALSE && read == core::mem::size_of::<f32>() {
        Some(value)
    } else {
        None
    }
}

/// Fault-tolerant single-byte read (None on unmapped memory).
pub unsafe fn safe_read_u8(addr: usize) -> Option<u8> {
    let mut value: u8 = 0;
    let mut read: usize = ZERO;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut u8 as *mut c_void,
            core::mem::size_of::<u8>(),
            &mut read,
        )
    };
    if ok != RPM_FALSE && read == core::mem::size_of::<u8>() {
        Some(value)
    } else {
        None
    }
}

/// Fault-tolerant u16 read (None on unmapped memory).
pub unsafe fn safe_read_u16(addr: usize) -> Option<u16> {
    let mut value: u16 = 0;
    let mut read: usize = ZERO;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut u16 as *mut c_void,
            core::mem::size_of::<u16>(),
            &mut read,
        )
    };
    if ok != RPM_FALSE && read == core::mem::size_of::<u16>() {
        Some(value)
    } else {
        None
    }
}
