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

/// Fault-tolerant bulk read into `out`. Returns true only if the whole slice was
/// read (None-equivalent for byte buffers). Used by the `.text` AOB scanner so a
/// drifted/unmapped region fails closed instead of faulting.
pub unsafe fn read_bytes(addr: usize, out: &mut [u8]) -> bool {
    if out.is_empty() {
        return true;
    }
    let mut read: usize = ZERO;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            out.as_mut_ptr() as *mut c_void,
            out.len(),
            &mut read,
        )
    };
    ok != RPM_FALSE && read == out.len()
}

/// Resolve the running game image's `.text` section as `(start_va, len)` by parsing
/// the in-memory PE headers. Returns `None` if the headers are unreadable or no
/// `.text` section is found. This is the bound for a fault-safe AOB scan; it makes
/// signature-based function discovery version-agnostic (no hardcoded RVAs).
pub fn module_text_range() -> Option<(usize, usize)> {
    let base = game_module_base().ok()?;
    unsafe {
        // DOS header: e_lfanew (u32) at +0x3C -> PE header offset.
        let mut w4 = [0u8; 4];
        if !read_bytes(base + 0x3C, &mut w4) {
            return None;
        }
        let pe = base + u32::from_le_bytes(w4) as usize;
        let mut sig = [0u8; 4];
        if !read_bytes(pe, &mut sig) || &sig != b"PE\0\0" {
            return None;
        }
        // COFF file header at pe+4: NumberOfSections (u16) at +2, SizeOfOptionalHeader (u16) at +16.
        let mut nsec = [0u8; 2];
        let mut optsz = [0u8; 2];
        if !read_bytes(pe + 6, &mut nsec) || !read_bytes(pe + 20, &mut optsz) {
            return None;
        }
        let num_sections = u16::from_le_bytes(nsec) as usize;
        let opt_size = u16::from_le_bytes(optsz) as usize;
        // Section headers (40 bytes each) begin after the optional header.
        let mut sec = pe + 24 + opt_size;
        for _ in 0..num_sections.min(96) {
            let mut hdr = [0u8; 40];
            if !read_bytes(sec, &mut hdr) {
                return None;
            }
            // name[0..8], VirtualSize[8..12], VirtualAddress[12..16].
            if &hdr[0..8] == b".text\0\0\0" {
                let vsize = u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as usize;
                let vaddr = u32::from_le_bytes([hdr[12], hdr[13], hdr[14], hdr[15]]) as usize;
                if vaddr == 0 || vsize == 0 {
                    return None;
                }
                return Some((base + vaddr, vsize));
            }
            sec += 40;
        }
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
