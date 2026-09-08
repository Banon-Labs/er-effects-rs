//! Tier A: fault-safe RAM readers + game module base / RVA resolution.
//!
//! Implemented over raw `#[link(name = "kernel32")]` externs so this stays a
//! zero-dependency leaf that all three DLLs (product, reload-trace, input-harness)
//! and er-telemetry-core can sit on without re-implementing `ReadProcessMemory` reads.
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

/// `game_module_base() + rva`, resolved for the running build.
///
/// Every RVA in this workspace is a 1.16.2 RVA. On a build that moved the code, this returns the
/// translated address when one has been verified and an `Err` when it has not -- so a caller
/// that ignores the error cannot call into whatever now occupies those bytes. Plain addition on
/// the supported build, and for anything outside the game image.
///
/// # Why `#[track_caller]`
///
/// This is the unnamed spelling, so its refusals used to be labelled `game_rva` and nothing else
/// -- which names ~150 call sites at once and therefore names none of them. It has now cost two
/// hunts. The first was 57 refusals in one boot. The second was a session that logged **339,764**
/// refusals of `0x140000000` (image base + RVA 0, which is never a meaningful address) with no
/// way to tell from the log which site was asking; it turned out to be `delay_delete_pending`
/// resolving RVA 0 just to obtain the module base, four times a second for 25 hours.
///
/// The caller's `file:line` goes into the refusal line so that class of hunt cannot recur. It is
/// a property of the function rather than a discipline expected of ~150 call sites, which is the
/// only reason it holds.
#[track_caller]
pub fn game_rva(rva: u32) -> Result<usize, String> {
    resolve_rva(rva, "game_rva", core::panic::Location::caller())
}

/// `game_module_base() + rva`, deliberately not resolved for the running build.
///
/// # The one caller shape this is for
///
/// An address about to be handed to a hook API that resolves it itself -- `er_hook::MhHook::new`,
/// `register_union_hook`, `register_shared_hook`, or a local helper that forwards into one. Those
/// own the single 1.16.2 -> 1.17 resolve, and [`game_rva`] would perform a second one.
///
/// # Why a second resolve is not merely redundant
///
/// It is usually a no-op: the address is a 1.17 destination, `already_translated_in` recognises it
/// and hands it straight back. That is exactly why this survived so long unnoticed. But an address
/// can be both a 1.17 destination of one row and the 1.16.2 source of a different row -- which is
/// what happens whenever a region's shift equals the local spacing between two functions, so
/// `B - A == C - B`. On such an address translation wins over the shortcut (it must; see
/// `already_translated_in`), and the second resolve silently returns C.
///
/// Measured on the 2026-08-30 18:42 boot, three detours installed on unrelated functions:
///
/// | intended                  | resolve 1     | resolve 2 (the detour that was installed) |
/// |---------------------------|---------------|-------------------------------------------|
/// | `WorldBlockRes::Update`   | `0x1406156c0` | `0x140616510`                             |
/// | `native_submit_7ac890`    | `0x1407ad710` | `0x1407ae590` (hot Scaleform, 16 callers) |
/// | profile per-frame push    | `0x140bbbd90` | `0x140bbd440` (`CSMenuFaceModelRend`)     |
///
/// No error, no refusal, no log line -- and each feature then logged the address it meant, which
/// is why nobody noticed. `scripts/check-double-resolved-hook-targets.py` is the gate that keeps
/// the shape out.
///
/// # What the `Result` means here, and what moved
///
/// It is the module-base lookup and nothing else, so a call site that already had an `else` branch
/// for a failed [`game_rva`] keeps it unchanged. What moves is where an unmappable address is
/// refused: no longer here, but inside the hook API, which logs `HOOK REFUSED` naming the address
/// and the build. That is the better place for it anyway -- it is the layer that knows whether the
/// row is merely callable or actually audited as a detour target, which are different questions
/// with different tables.
pub fn game_rva_for_hook(rva: u32) -> Result<usize, String> {
    Ok(game_module_base()? + rva as usize)
}

/// [`game_rva`], but the caller names the address so a refusal is attributable by name as well
/// as by source line.
///
/// Both halves earn their place: the name says which constant went inert (the thing a reader
/// wants), and the location says which of the several sites that resolve it was asking (the
/// thing that makes it fixable). Prefer this form when a name exists.
#[track_caller]
pub fn game_rva_named(rva: u32, what: &'static str) -> Result<usize, String> {
    resolve_rva(rva, what, core::panic::Location::caller())
}

/// Shared body of [`game_rva`] and [`game_rva_named`]: resolve `rva` for the running build,
/// labelling any refusal with both the name and the source line that asked for it.
///
/// `at` is passed in rather than read here because `#[track_caller]` reports the caller of the
/// nearest tracked frame; reading it in this untracked helper would report `game_rva` itself and
/// re-create the exact anonymity this exists to remove.
fn resolve_rva(rva: u32, what: &str, at: &core::panic::Location<'_>) -> Result<usize, String> {
    let raw = game_module_base()? + rva as usize;
    crate::game_build::resolve_game_address_fmt(
        raw,
        format_args!("{what} @ {}:{}", at.file(), at.line()),
    )
    .ok_or_else(|| {
        format!(
            "rva 0x{rva:x} has no verified mapping for the running build: {}",
            crate::game_build::describe_build()
        )
    })
}

/// `base + rva` for a read, resolved for the running build -- or `0` when there is no mapping.
///
/// # Why zero rather than an error
///
/// The call sites this exists for are reads of game globals: `safe_read_usize(crate::mem::game_data_addr(base, FOO_RVA, "FOO_RVA"))`
/// and friends, which are already fault-tolerant and already have a "this global is not there"
/// path. Handing them address 0 puts a refusal down that existing path unchanged -- the read
/// fails, the caller takes the branch it already had for a null global, and nothing new has to
/// be decided at ~73 separate sites.
///
/// # Why reads needed this at all
///
/// A stale call announces itself: 1.16.2's `0x1405eefb0` is mid-instruction on 1.17 and the
/// process dies immediately. A stale read does not. Every `.data` global moved between the
/// builds -- most by +0x4070, `runtime_heap_allocator` by +0x4080, `multiplay_properties` by
/// +0x4000 -- so `safe_read_usize` succeeds and returns whatever now occupies the old slot. Two
/// measured consequences: a garbage repository pointer reached `CreateTpfResCap`, which divided
/// by zero 894ms into boot; and the swapchain find read a stale `GX_DRAW_CONTEXT_RVA` root, missed
/// for 1200 consecutive tries, and left a live process behind a black screen.
///
/// Never use this for a call target. Zero is a safe address to fail a read at and a fatal one to
/// jump to; call sites must take the `Option` from [`crate::game_build::resolve_game_address`]
/// and decide what refusing means for them.
pub fn game_data_addr(base: usize, rva: usize, what: &'static str) -> usize {
    crate::game_build::resolve_game_address(base + rva, what).unwrap_or(0)
}

/// [`game_data_addr`] for an indexed global -- a table row, an array element -- returning
/// `base + rva + byte_offset`, or `0` when the base RVA has no mapping.
///
/// # Why the plain form is not enough
///
/// [`game_data_addr`] answers `0` for a refusal, and every caller's null check depends on that
/// `0` surviving. `game_data_addr(base, TABLE_RVA, "TABLE") + slot * 8` destroys it: a refusal on
/// slot 3 produces the address `24`, which is not zero, so an `if address != 0` guard passes and
/// the caller dereferences page zero. Reads survive that (`safe_read_*` is kernel-validated and
/// merely fails), but a write faults -- and there is such a write: the loading-portrait teardown
/// nulls `table[slot]` to spare a renderer from the native delete.
///
/// Adding the offset here keeps the refusal a refusal all the way to the caller.
pub fn game_data_addr_offset(
    base: usize,
    rva: usize,
    what: &'static str,
    byte_offset: usize,
) -> usize {
    match game_data_addr(base, rva, what) {
        ZERO => ZERO,
        address => address + byte_offset,
    }
}

/// Read a pointer-sized game global by RVA: resolve the address for the running build, then read
/// it fault-tolerantly. `0` for a refusal, an unmapped address, or a genuinely null global.
///
/// # Why this exists as one call
///
/// The two halves are useless apart and were repeatedly written apart. Resolving without a safe
/// read turns a refusal into a crash, because [`game_data_addr`] answers 0 and `*(0 as *const _)`
/// faults. Safe-reading without resolving is worse and quieter: every `.data` global moved between
/// 1.16.2 and 1.17, so the read succeeds and returns whatever now occupies the old slot.
///
/// This is a safe function on purpose. It dereferences nothing the caller can get wrong: the
/// address is resolved here and the read is kernel-validated, so there is no precondition to
/// state and no `unsafe` block for a caller to write around it. Marking it `unsafe` would only
/// add ceremony at every site and make the safe form look like the risky one.
pub fn read_global_ptr(base: usize, rva: usize, what: &'static str) -> usize {
    unsafe { safe_read_usize(game_data_addr(base, rva, what)) }.unwrap_or(ZERO)
}

/// [`read_global_ptr`] for a byte-sized global. `0` for a refusal or an unreadable address.
pub fn read_global_u8(base: usize, rva: usize, what: &'static str) -> u8 {
    unsafe { safe_read_u8(game_data_addr(base, rva, what)) }.unwrap_or(ZERO as u8)
}

/// Store a byte into a game global by RVA. Returns whether the store happened.
///
/// A store is the one access that must never go through unresolved. Reading a moved global returns
/// nonsense the caller can at least notice; writing one corrupts whatever now lives there, and
/// writing a refusal (address 0) crashes outright. Measured 2026-08-29: the title's zero-input
/// menu-accept byte moved +0x4080 on 1.17 and its raw store landed on a neighbouring byte, logging
/// success while the title menu never opened.
///
/// # Safety
///
/// `rva` must name a byte-sized game global. The store itself is guarded: nothing is written when
/// the address cannot be resolved for the running build.
pub unsafe fn write_global_u8(base: usize, rva: usize, what: &'static str, value: u8) -> bool {
    let at = game_data_addr(base, rva, what);
    if at == ZERO {
        return false;
    }
    unsafe { *(at as *mut u8) = value };
    true
}

/// Cheap heap-pointer sanity check: above the low 64 KiB reserve and 8-byte aligned.
///
/// # Safety
///
/// There is no precondition and no unsafety here: this function dereferences nothing.
/// It is integer arithmetic on `ptr` -- a range test against the low 64 KiB reserve and
/// an alignment mask -- and would be sound as a safe `fn`. The `unsafe` marker is
/// vestigial, kept only because removing it would change the signature of a function
/// called across every DLL in this workspace. A `true` result is a cheap plausibility
/// screen, not proof that `ptr` is mapped or points at a live object.
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
///
/// # Safety
///
/// `addr` has no precondition: any value, including 0, a freed pointer, or a wholly
/// unmapped address, is safe to pass. The read goes through `ReadProcessMemory`, which
/// validates the range in the kernel and returns `FALSE` rather than raising an access
/// violation, so this function cannot fault on a bad address -- that fault-tolerance is
/// the entire reason it exists.
///
/// What the caller owns is the meaning of the bytes that come back. A successful read
/// only proves those bytes were mapped at that instant; it does not prove they are a
/// live object of the expected type, and the game may free or overwrite the region on
/// another thread immediately afterwards. Treat the value as a sample, not a borrow.
///
/// The `unsafe` marker is therefore about interpretation, not memory validity, and is
/// retained rather than removed only because dropping it would change the signature of
/// a function every DLL in this workspace calls.
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
///
/// # Safety
///
/// `addr` has no precondition: any value, including 0, a freed pointer, or a wholly
/// unmapped address, is safe to pass. The read goes through `ReadProcessMemory`, which
/// validates the range in the kernel and returns `FALSE` rather than raising an access
/// violation, so this function cannot fault on a bad address -- that fault-tolerance is
/// the entire reason it exists.
///
/// What the caller owns is the meaning of the bytes that come back. A successful read
/// only proves those bytes were mapped at that instant; it does not prove they are a
/// live object of the expected type, and the game may free or overwrite the region on
/// another thread immediately afterwards. Treat the value as a sample, not a borrow.
///
/// The `unsafe` marker is therefore about interpretation, not memory validity, and is
/// retained rather than removed only because dropping it would change the signature of
/// a function every DLL in this workspace calls.
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
///
/// # Safety
///
/// `addr` has no precondition: any value, including 0, a freed pointer, or a wholly
/// unmapped address, is safe to pass. The read goes through `ReadProcessMemory`, which
/// validates the range in the kernel and returns `FALSE` rather than raising an access
/// violation, so this function cannot fault on a bad address -- that fault-tolerance is
/// the entire reason it exists.
///
/// What the caller owns is the meaning of the bytes that come back. A successful read
/// only proves those bytes were mapped at that instant; it does not prove they are a
/// live object of the expected type, and the game may free or overwrite the region on
/// another thread immediately afterwards. Treat the value as a sample, not a borrow.
///
/// The `unsafe` marker is therefore about interpretation, not memory validity, and is
/// retained rather than removed only because dropping it would change the signature of
/// a function every DLL in this workspace calls.
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
///
/// # Safety
///
/// `addr` has no precondition: any value, including 0, a freed pointer, or a wholly
/// unmapped address, is safe to pass. The read goes through `ReadProcessMemory`, which
/// validates the range in the kernel and returns `FALSE` rather than raising an access
/// violation, so this function cannot fault on a bad address -- that fault-tolerance is
/// the entire reason it exists.
///
/// What the caller owns is the meaning of the bytes that come back. A successful read
/// only proves those bytes were mapped at that instant; it does not prove they are a
/// live object of the expected type, and the game may free or overwrite the region on
/// another thread immediately afterwards. Treat the value as a sample, not a borrow.
///
/// The `unsafe` marker is therefore about interpretation, not memory validity, and is
/// retained rather than removed only because dropping it would change the signature of
/// a function every DLL in this workspace calls.
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
///
/// # Safety
///
/// `addr` has no precondition -- see [`safe_read_usize`]; the read is performed by
/// `ReadProcessMemory` and fails closed on an unmapped range instead of faulting. This
/// is what lets the `.text` AOB scanner walk a drifted or partially-unmapped image
/// without crashing the game.
///
/// `out` is an ordinary Rust slice and is only written on a fully successful read, so a
/// `false` return leaves its contents unspecified but initialised. The caller owns the
/// meaning of the bytes, exactly as above.
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
///
/// # Safety
///
/// `addr` has no precondition: any value, including 0, a freed pointer, or a wholly
/// unmapped address, is safe to pass. The read goes through `ReadProcessMemory`, which
/// validates the range in the kernel and returns `FALSE` rather than raising an access
/// violation, so this function cannot fault on a bad address -- that fault-tolerance is
/// the entire reason it exists.
///
/// What the caller owns is the meaning of the bytes that come back. A successful read
/// only proves those bytes were mapped at that instant; it does not prove they are a
/// live object of the expected type, and the game may free or overwrite the region on
/// another thread immediately afterwards. Treat the value as a sample, not a borrow.
///
/// The `unsafe` marker is therefore about interpretation, not memory validity, and is
/// retained rather than removed only because dropping it would change the signature of
/// a function every DLL in this workspace calls.
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

/// Page granularity for [`safe_read_cstr`]'s walk. Mapping is per-page, so a page is either
/// readable in full or not at all -- which is what makes a page-bounded chunk the largest read
/// that cannot fail merely because of where the string happens to sit.
const PAGE_SIZE: usize = 0x1000;

/// Fault-safe, length-bounded read of a NUL-terminated C string.
///
/// This exists because `CStr::from_ptr` on a pointer that came from outside our own code is a
/// crash waiting for the right afternoon: it calls `strlen`, `strlen` dereferences, and a
/// non-null-but-garbage pointer takes the process down. A null check does not help -- the
/// pointers that actually killed Elden Ring on two testers on 2026-08-23 were
/// `0x011000010e05acda` and `0x0110000107be5e2c`, both very much non-null (bd
/// `ersc-steam-garbage-key-ptr-crashes-lobby-publish-2026-08-24`).
///
/// Returns the bytes before the NUL, or `None` if the string is unreadable, if `addr` is null,
/// or if no NUL appears within `max_len`. That last case is deliberately a failure and not a
/// truncation: a readable region with no terminator in range is not a string we have any reason
/// to trust, and silently returning `max_len` bytes of it would launder junk into a value the
/// caller goes on to use.
///
/// # Safety
///
/// `addr` has no precondition -- see [`safe_read_usize`]. Every read goes through
/// `ReadProcessMemory`, which fails closed on an unmapped page instead of faulting. The reads
/// are page-bounded so a legitimate string ending in the last mapped page of a region is not
/// rejected just because the next page is absent.
///
/// The caller still owns the meaning of the bytes: a successful read proves only that they were
/// mapped at that instant.
pub unsafe fn safe_read_cstr(addr: usize, max_len: usize) -> Option<Vec<u8>> {
    cstr_walk(addr, max_len, &mut |at, out| unsafe { read_bytes(at, out) })
}

/// The page-bounded walk behind [`safe_read_cstr`], with the actual read injected.
///
/// Split out for one reason: `ReadProcessMemory` does not exist on the host, so a test that
/// called [`safe_read_cstr`] would not link, and the guard would ship with its logic unexercised
/// -- which is how the bug it exists to prevent got out in the first place. `read` stands in for
/// the kernel: it returns `false` for a range that is not fully readable, exactly as
/// [`read_bytes`] does.
fn cstr_walk(
    addr: usize,
    max_len: usize,
    read: &mut dyn FnMut(usize, &mut [u8]) -> bool,
) -> Option<Vec<u8>> {
    if addr == ZERO || max_len == ZERO {
        return None;
    }
    let mut out: Vec<u8> = Vec::new();
    let mut cursor = addr;
    while out.len() < max_len {
        // Never let one read span a page boundary. `ReadProcessMemory` is all-or-nothing, so a
        // read that ran on into an unmapped neighbouring page would report failure for a string
        // that is entirely present in the page it started in.
        let to_page_end = PAGE_SIZE - (cursor & (PAGE_SIZE - 1));
        let want = to_page_end.min(max_len - out.len());
        let mut chunk = vec![0u8; want];
        if !read(cursor, &mut chunk) {
            return None;
        }
        if let Some(nul) = chunk.iter().position(|&byte| byte == 0) {
            out.extend_from_slice(&chunk[..nul]);
            return Some(out);
        }
        out.extend_from_slice(&chunk);
        cursor = cursor.checked_add(want)?;
    }
    None
}

#[cfg(test)]
mod cstr_tests {
    use super::{PAGE_SIZE, cstr_walk};

    /// The two pointers that actually took Elden Ring down on 2026-08-23. Both non-null, which is
    /// the entire point: the guard they defeated was a null check.
    const CRASH_POINTERS: [usize; 2] = [0x0110_0001_0e05_acda, 0x0110_0001_07be_5e2c];

    /// A reader that models one mapped page at `base` whose contents start with `bytes`;
    /// everything outside that page is unmapped. The page is a full [`PAGE_SIZE`] because that is
    /// what mapping granularity means -- a stub page shorter than that would reject the
    /// page-bounded chunk the walk legitimately asks for, and test the stub rather than the code.
    fn one_page(base: usize, bytes: &[u8]) -> impl FnMut(usize, &mut [u8]) -> bool {
        let mut page = vec![0xffu8; PAGE_SIZE];
        page[..bytes.len()].copy_from_slice(bytes);
        move |at, out| {
            let Some(start) = at.checked_sub(base) else {
                return false;
            };
            let Some(end) = start.checked_add(out.len()) else {
                return false;
            };
            if end > page.len() {
                return false;
            }
            out.copy_from_slice(&page[start..end]);
            true
        }
    }

    #[test]
    fn an_unreadable_pointer_is_refused_rather_than_dereferenced() {
        for bad in CRASH_POINTERS {
            let mut never = |_: usize, _: &mut [u8]| false;
            assert_eq!(
                cstr_walk(bad, 255, &mut never),
                None,
                "a garbage non-null pointer must fail closed, not walk memory"
            );
        }
    }

    #[test]
    fn null_is_refused_before_any_read_is_attempted() {
        let mut reads = 0usize;
        let mut counting = |_: usize, _: &mut [u8]| {
            reads += 1;
            true
        };
        assert_eq!(cstr_walk(0, 255, &mut counting), None);
        assert_eq!(reads, 0, "null must short-circuit, not reach the reader");
    }

    #[test]
    fn a_terminated_string_comes_back_without_its_nul() {
        let page = b"lobby_key\0trailing junk".to_vec();
        let mut read = one_page(0x1_0000, &page);
        assert_eq!(
            cstr_walk(0x1_0000, 255, &mut read).as_deref(),
            Some(&b"lobby_key"[..])
        );
    }

    #[test]
    fn a_run_with_no_terminator_in_range_is_refused_not_truncated() {
        // one_page pads with 0xff, so nothing in range is a NUL.
        let mut read = one_page(0x1_0000, &[b'A'; 512]);
        assert_eq!(
            cstr_walk(0x1_0000, 64, &mut read),
            None,
            "no NUL within max_len is junk, and truncating it would launder junk into a value"
        );
    }

    #[test]
    fn a_string_at_the_end_of_a_mapped_page_survives_the_unmapped_neighbour() {
        // The string sits in the last bytes of the page, so a read that ran past the page end
        // would fail. Page-bounded chunking is what keeps this case readable.
        let tail = b"lobby_key\0";
        let mut page = vec![b'.'; PAGE_SIZE];
        page[PAGE_SIZE - tail.len()..].copy_from_slice(tail);
        let base = 0x1_0000;
        let mut read = one_page(base, &page);
        let at = base + PAGE_SIZE - tail.len();
        assert_eq!(
            cstr_walk(at, 255, &mut read).as_deref(),
            Some(&b"lobby_key"[..])
        );
    }
}
