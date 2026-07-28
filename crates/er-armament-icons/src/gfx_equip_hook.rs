//! Runtime GFX ArtsBadge edit for the equip menu, reaching the parse of the
//! BOOT-PRELOADED movie (bd armament-icons-reach-fix-hook-parse-fn-1411cf180).
//!
//! `02_011_equip.gfx` is preloaded+parsed ONCE at boot into a cached MovieDataDef, so
//! the file-open MemoryFile swap that works for ON-DEMAND menus (title/options, which
//! reparse on display) never reaches it -- at equip display CreateMovie is a cache HIT
//! with no reopen, so a post-open field swap has no read to intercept (proven: an
//! injected existing-char sibling stayed unbound; bd
//! armament-icons-boot-preload-swap-not-reach-createmovie-name-empty).
//!
//! Instead we hook the GFx tag-parse function `FUN_1411cf1a0(loadProcess /*rcx*/,
//! File* /*rdx*/)`. Every menu movie -- preload or on-demand -- funnels through it to
//! build its MovieDataDef, and its 2nd arg is the File the tag reader (`FUN_141162800`)
//! consumes via the MemoryFile vtable Read (which memcpy's from File+0x18 using the
//! +0x24 cursor, clamped to +0x20 len). When that File fingerprints as the vanilla
//! 02_011 movie we derive the ArtsBadge-edited movie and swap the File's
//! data/len/cursor, so the parser builds the EDITED MovieDataDef that the equip screen
//! then binds. Since our hooks install before the boot preload, this catches 02_011's
//! boot parse -- no forced reparse needed.
//!
//! The parse fn is located by a UNIQUE `.text` AOB scan (version-agnostic; hardcoded
//! RVAs drift between patches). A non-unique scan disables the hook FAIL-CLOSED (badge
//! simply not reached) rather than crash-hooking.

#![cfg(windows)]

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use er_game_base::mem::{read_bytes, safe_read_i32, safe_read_u8, safe_read_usize};

use crate::log_message;

/// Scaleform `MemoryFile` vtable RVA; a File whose first qword == `base + this` is a
/// MemoryFile with the data/len/cursor layout below (`Read` memcpy's from +0x18).
const MEMORY_FILE_VTABLE_RVA: usize = 0x2ba4c80;
const MEMORY_FILE_DATA_OFFSET: usize = 0x18;
const MEMORY_FILE_LEN_OFFSET: usize = 0x20;
const MEMORY_FILE_CURSOR_OFFSET: usize = 0x24;

/// GFx tag-parse entry `FUN_1411cf1a0(loadProcess /*rcx*/, File* /*rdx*/) -> bool`
/// (verified 1.16.2: passes param_2 as the File to the tag reader `FUN_141162800`,
/// which reads it via vtable +0x50 Read / +0x20 Tell). Unique 30-byte
/// position-independent prologue (verified count==1 in the 1.16.2 image).
const PARSE_SIG: &str =
    "40 53 48 83 EC 40 48 8B 41 18 48 8B D9 C6 44 24 30 01 48 83 C1 50 4C 8B 50 20 4C 8B 58 48";

/// File-open observer (logs the `.gfx` open sequence AND provides a live loader to resolve
/// FileOpener::OpenFile from). Known-good hardcoded 1.16.2 RVA.
const FILE_OPEN_RVA: usize = 0x11ced80;

/// URL fragment identifying the equip movie (matched on FileOpener::OpenFile's url arg).
const EQUIP_URL_NEEDLE: &[u8] = b"02_011_equip";

/// Sentinel for "trampoline not installed yet".
const ORIG_UNSET: usize = 0;

static GAME_BASE: AtomicUsize = AtomicUsize::new(0);
static PARSE_ORIG: AtomicUsize = AtomicUsize::new(ORIG_UNSET);
static FILE_OPEN_ORIG: AtomicUsize = AtomicUsize::new(ORIG_UNSET);
static HOOK_ACTIVE: AtomicUsize = AtomicUsize::new(0);

// --- FileOpener::OpenFile hook (the SINGLE point both the header open and the async
// tag-dict re-open flow through -- bd armament-icons-reach-root-async-reopen-hook-fileopener-openfile).
// The loader's FileOpener lives at `*(*(loader+0x10)+0x10)`; OpenFile is its vtable+0x18.
// Resolved at RUNTIME from a live loader (obtained in the file-open observer) and hooked
// once. `OpenFile(this, url /*rdx*/, log, 0x21, 0x1b6) -> File*`.
const LOADER_FILEOPENER_HOLDER_OFFSET: usize = 0x10; // loader+0x10 -> holder
const HOLDER_FILEOPENER_OFFSET: usize = 0x10; // holder+0x10 -> FileOpener
const FILEOPENER_OPENFILE_VTABLE_SLOT: usize = 0x18; // FileOpener vtable +0x18 -> OpenFile
static OPENFILE_ORIG: AtomicUsize = AtomicUsize::new(ORIG_UNSET);
/// 0 = not attempted, 1 = installed, 2 = failed (don't retry).
static OPENFILE_STATE: AtomicUsize = AtomicUsize::new(0);

// -- oracle counters (machine-checkable) --
static PARSE_TOTAL: AtomicU64 = AtomicU64::new(0);
static EQUIP_PARSE_SWAPS: AtomicU64 = AtomicU64::new(0);
static EQUIP_OPENFILE_SWAPS: AtomicU64 = AtomicU64::new(0);
static EQUIP_SWAP_FAILURES: AtomicU64 = AtomicU64::new(0);
static GFX_OPEN_TOTAL: AtomicU64 = AtomicU64::new(0);
/// How many `.gfx` open URLs to log before going quiet.
const DIAG_LOG_LIMIT: u64 = 80;

/// Process-lifetime cache of the derived edited movie. The swapped File data ptr
/// aliases this buffer, so it must never move or free.
static EQUIP_EDITED: OnceLock<Vec<u8>> = OnceLock::new();

// ---- AOB scanner (version-agnostic function discovery) ----

/// Parse an IDA-style AOB signature ("48 89 ?? 24 10") into `(bytes, mask)`; `mask[i]`
/// false = wildcard. None for an empty/invalid signature or a wildcard first byte (the
/// scan anchors on byte 0, which must be concrete).
fn parse_sig(sig: &str) -> Option<(Vec<u8>, Vec<bool>)> {
    let mut bytes = Vec::new();
    let mut mask = Vec::new();
    for tok in sig.split_whitespace() {
        if tok == "??" || tok == "?" {
            bytes.push(0u8);
            mask.push(false);
        } else {
            bytes.push(u8::from_str_radix(tok, 16).ok()?);
            mask.push(true);
        }
    }
    if bytes.is_empty() || !mask[0] {
        return None;
    }
    Some((bytes, mask))
}

/// Find the UNIQUE address in `text` (mapped at `text_start`) matching `sig`. None if the
/// signature is empty/invalid, has zero matches, or has MORE THAN ONE match (ambiguous ->
/// fail closed so a drifted/duplicated pattern never hooks the wrong function).
fn scan_unique(text: &[u8], text_start: usize, sig: &str) -> Option<usize> {
    let (bytes, mask) = parse_sig(sig)?;
    let m = bytes.len();
    if m == 0 || text.len() < m {
        return None;
    }
    let anchor = bytes[0];
    let last = text.len() - m;
    let mut found: Option<usize> = None;
    let mut i = 0usize;
    while i <= last {
        if text[i] != anchor {
            match text[i + 1..=last].iter().position(|&b| b == anchor) {
                Some(k) => {
                    i += k + 1;
                    continue;
                }
                None => break,
            }
        }
        let mut ok = true;
        for j in 1..m {
            if mask[j] && text[i + j] != bytes[j] {
                ok = false;
                break;
            }
        }
        if ok {
            if found.is_some() {
                return None; // ambiguous -> fail closed
            }
            found = Some(text_start + i);
        }
        i += 1;
    }
    found
}

/// Read a bounded NUL-terminated ASCII name at `url` into `out`, returning the filled
/// length. For the `.gfx` open diagnostic.
unsafe fn read_bounded_name(url: usize, out: &mut [u8]) -> usize {
    let mut n = 0usize;
    if url == 0 {
        return 0;
    }
    while n < out.len() {
        match unsafe { safe_read_u8(url + n) } {
            Some(0) | None => break,
            Some(b) => {
                out[n] = b;
                n += 1;
            }
        }
    }
    n
}

/// Scan the NUL-terminated ASCII path at `url` (bounded) for `needle`.
unsafe fn bounded_ascii_contains(url: usize, needle: &[u8]) -> bool {
    if url == 0 || needle.is_empty() {
        return false;
    }
    const MAX: usize = 512;
    let mut buf = [0u8; MAX];
    let mut n = 0usize;
    while n < MAX {
        match unsafe { safe_read_u8(url + n) } {
            Some(0) | None => break,
            Some(b) => {
                buf[n] = b;
                n += 1;
            }
        }
    }
    if n < needle.len() {
        return false;
    }
    buf[..n].windows(needle.len()).any(|w| w == needle)
}

/// If `file` is a MemoryFile holding the vanilla 02_011 movie, derive the ArtsBadge edit
/// and swap the File's data/len/cursor so the reader consumes the edited stream. `via`
/// labels the caller (parse header path vs FileOpener::OpenFile async path). Content-
/// matched (length pre-filter + GFX magic + FNV fingerprint) -- no URL/string dependency.
/// Fail-closed: anything unexpected leaves the native movie untouched.
unsafe fn maybe_swap_equip_file(base: usize, file: usize, via: &str) {
    if file == 0 {
        return;
    }
    let vtable = unsafe { safe_read_usize(file) }.unwrap_or(0);
    if vtable != base + MEMORY_FILE_VTABLE_RVA {
        return; // not a MemoryFile (e.g. an image/font stream)
    }
    let data = unsafe { safe_read_usize(file + MEMORY_FILE_DATA_OFFSET) }.unwrap_or(0);
    let len = unsafe { safe_read_i32(file + MEMORY_FILE_LEN_OFFSET) }.unwrap_or(0);
    // Cheap length pre-filter: only a vanilla-length movie can BE vanilla 02_011.
    if data == 0 || len as usize != er_gfx::equip_02_011::VANILLA_LEN {
        return;
    }
    let len = len as usize;
    // GFX magic through the guarded reader before the bulk read + fingerprint.
    let magic_ok = unsafe { safe_read_u8(data) } == Some(b'G')
        && unsafe { safe_read_u8(data + 1) } == Some(b'F')
        && unsafe { safe_read_u8(data + 2) } == Some(b'X');
    if !magic_ok {
        return;
    }
    let mut vanilla = vec![0u8; len];
    if !unsafe { read_bytes(data, &mut vanilla) } {
        return;
    }
    if !er_gfx::equip_02_011::is_known_vanilla(&vanilla) {
        return; // a different 18400-byte movie, not 02_011
    }
    // Derive once (cached for the process lifetime), then swap the File to it.
    let edited = match EQUIP_EDITED.get() {
        Some(cached) => cached,
        None => match er_gfx::equip_02_011::arts_badge(&vanilla) {
            Ok(out) => {
                log_message(format_args!(
                    "gfx-equip: 02_011 derived badge movie in={len} out={} (via {via})",
                    out.len()
                ));
                EQUIP_EDITED.get_or_init(|| out)
            }
            Err(err) => {
                EQUIP_SWAP_FAILURES.fetch_add(1, Ordering::SeqCst);
                log_message(format_args!(
                    "gfx-equip: 02_011 arts_badge derive FAILED (serving native): {err}"
                ));
                return;
            }
        },
    };
    unsafe {
        core::ptr::write(
            (file + MEMORY_FILE_DATA_OFFSET) as *mut usize,
            edited.as_ptr() as usize,
        );
        core::ptr::write(
            (file + MEMORY_FILE_LEN_OFFSET) as *mut u32,
            edited.len() as u32,
        );
        core::ptr::write((file + MEMORY_FILE_CURSOR_OFFSET) as *mut u32, 0);
    }
    let n = EQUIP_PARSE_SWAPS.fetch_add(1, Ordering::SeqCst) + 1;
    log_message(format_args!(
        "gfx-equip: 02_011 swap #{n} via {via} -- served edited {} bytes (ArtsBadge should bind)",
        edited.len()
    ));
}

/// GFx tag-parse hook: swap the equip File before the header reader consumes it (covers the
/// header; the tag dictionary is covered by the FileOpener::OpenFile hook). `file` (rdx) is
/// the MemoryFile the header reader reads.
unsafe extern "system" fn parse_hook(load_process: usize, file: usize) -> usize {
    PARSE_TOTAL.fetch_add(1, Ordering::SeqCst);
    let base = GAME_BASE.load(Ordering::SeqCst);
    if base != 0 {
        unsafe { maybe_swap_equip_file(base, file, "parse") };
    }
    let orig = PARSE_ORIG.load(Ordering::SeqCst);
    if orig != ORIG_UNSET {
        let f: unsafe extern "system" fn(usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(load_process, file) }
    } else {
        0
    }
}

/// `FileOpener::OpenFile(this, url /*rdx*/, log, flags1, flags2) -> File*` hook. This is the
/// single point BOTH the header open and the async tag-dictionary re-open flow through, so
/// swapping the returned File here reaches the parsed dictionary (not just the header).
unsafe extern "system" fn openfile_hook(
    this: usize,
    url: usize,
    log: usize,
    f1: u32,
    f2: u32,
) -> usize {
    let orig = OPENFILE_ORIG.load(Ordering::SeqCst);
    let file = if orig != ORIG_UNSET {
        let f: unsafe extern "system" fn(usize, usize, usize, u32, u32) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(this, url, log, f1, f2) }
    } else {
        0
    };
    if unsafe { bounded_ascii_contains(url, EQUIP_URL_NEEDLE) } {
        EQUIP_OPENFILE_SWAPS.fetch_add(1, Ordering::SeqCst);
        let base = GAME_BASE.load(Ordering::SeqCst);
        if base != 0 {
            unsafe { maybe_swap_equip_file(base, file, "openfile") };
        }
    }
    file
}

/// Resolve the loader's `FileOpener::OpenFile` VA from a live `loader` and hook it once.
/// Called from the file-open observer (which has a loader). Idempotent; fail-closed.
unsafe fn try_install_openfile_hook(loader: usize) {
    use std::ffi::c_void;

    use er_hook::{MH_ApplyQueued, MH_STATUS, MhHook};

    if OPENFILE_STATE.load(Ordering::SeqCst) != 0 {
        return;
    }
    let base = GAME_BASE.load(Ordering::SeqCst);
    let holder = unsafe { safe_read_usize(loader + LOADER_FILEOPENER_HOLDER_OFFSET) }.unwrap_or(0);
    let fileopener = if holder != 0 {
        unsafe { safe_read_usize(holder + HOLDER_FILEOPENER_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    let vtable = if fileopener != 0 {
        unsafe { safe_read_usize(fileopener) }.unwrap_or(0)
    } else {
        0
    };
    let openfile = if vtable != 0 {
        unsafe { safe_read_usize(vtable + FILEOPENER_OPENFILE_VTABLE_SLOT) }.unwrap_or(0)
    } else {
        0
    };
    // Sanity: the resolved OpenFile must be a code address in the game image.
    if base == 0 || openfile < base + 0x1000 || openfile >= base + 0x0800_0000 {
        OPENFILE_STATE.store(2, Ordering::SeqCst);
        log_message(format_args!(
            "gfx-equip: FileOpener::OpenFile UNRESOLVED (loader=0x{loader:x} holder=0x{holder:x} \
             fo=0x{fileopener:x} vt=0x{vtable:x} open=0x{openfile:x}); async tag swap disabled"
        ));
        return;
    }
    let hook = match unsafe { MhHook::new(openfile as *mut c_void, openfile_hook as *mut c_void) } {
        Ok(h) => h,
        Err(status) => {
            OPENFILE_STATE.store(2, Ordering::SeqCst);
            log_message(format_args!(
                "gfx-equip: MhHook::new(openfile @0x{openfile:x}) failed: {status:?}"
            ));
            return;
        }
    };
    OPENFILE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
    if unsafe { hook.queue_enable() }.is_err() {
        OPENFILE_STATE.store(2, Ordering::SeqCst);
        OPENFILE_ORIG.store(ORIG_UNSET, Ordering::SeqCst);
        return;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            OPENFILE_STATE.store(1, Ordering::SeqCst);
            log_message(format_args!(
                "gfx-equip: FileOpener::OpenFile hook ACTIVE @0x{openfile:x} \
                 (fo=0x{fileopener:x}); async tag-dict swap armed"
            ));
        }
        status => {
            OPENFILE_STATE.store(2, Ordering::SeqCst);
            OPENFILE_ORIG.store(ORIG_UNSET, Ordering::SeqCst);
            log_message(format_args!(
                "gfx-equip: openfile MH_ApplyQueued failed: {status:?}"
            ));
        }
    }
}

/// File-open observer: logs the `.gfx` open sequence AND, on first fire, resolves+hooks the
/// loader's `FileOpener::OpenFile` (the single point covering header + async tag opens).
unsafe extern "system" fn file_open_hook(loader: usize, url: usize, flags: u32) -> usize {
    // Lazily resolve+install the FileOpener::OpenFile hook from a live loader (before the
    // 02_011 preload open, which is ~#37, so it is armed in time).
    if OPENFILE_STATE.load(Ordering::SeqCst) == 0 && loader != 0 {
        unsafe { try_install_openfile_hook(loader) };
    }
    let orig = FILE_OPEN_ORIG.load(Ordering::SeqCst);
    let native = if orig != ORIG_UNSET {
        let f: unsafe extern "system" fn(usize, usize, u32) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(loader, url, flags) }
    } else {
        0
    };
    if unsafe { bounded_ascii_contains(url, b".gfx") } {
        let gn = GFX_OPEN_TOTAL.fetch_add(1, Ordering::SeqCst) + 1;
        if gn <= DIAG_LOG_LIMIT {
            let mut buf = [0u8; 128];
            let n = unsafe { read_bounded_name(url, &mut buf) };
            log_message(format_args!(
                "gfx-equip: gfx-open #{gn} url='{}'",
                core::str::from_utf8(&buf[..n]).unwrap_or("<non-utf8>")
            ));
        }
    }
    native
}

/// Install the parse hook (AOB-scanned, fail-closed) plus the diagnostic file-open
/// observer. Idempotent. If the parse signature does not resolve UNIQUELY the reach hook
/// is skipped (badge not reached) rather than crash-hooking a wrong address.
pub(crate) fn install(base: usize) {
    use std::ffi::c_void;

    use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};

    if HOOK_ACTIVE.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            log_message(format_args!("gfx-equip: MH_Initialize failed: {status:?}"));
            return;
        }
    }
    GAME_BASE.store(base, Ordering::SeqCst);

    // Queue a detour on absolute `target`, storing its trampoline into `orig`.
    let queue = |target: usize, detour: *mut c_void, orig: &AtomicUsize, label: &str| -> bool {
        let hook = match unsafe { MhHook::new(target as *mut c_void, detour) } {
            Ok(h) => h,
            Err(status) => {
                log_message(format_args!(
                    "gfx-equip: MhHook::new({label} @0x{target:x}) failed: {status:?}"
                ));
                return false;
            }
        };
        orig.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            log_message(format_args!(
                "gfx-equip: queue_enable({label} @0x{target:x}) failed: {status:?}"
            ));
            return false;
        }
        true
    };

    // Diagnostic file-open observer (optional; non-fatal if it fails).
    let _ = queue(
        base + FILE_OPEN_RVA,
        file_open_hook as *mut c_void,
        &FILE_OPEN_ORIG,
        "file_open",
    );

    // Reach hook: locate the GFx parse fn by unique AOB in the live .text, fail-closed.
    let parse_armed = 'parse: {
        let (start, len) = match er_game_base::mem::module_text_range() {
            Some((s, l)) if (0x1000..=0x0800_0000).contains(&l) => (s, l),
            other => {
                log_message(format_args!(
                    "gfx-equip: parse hook DISABLED -- .text range unresolved ({other:x?})"
                ));
                break 'parse false;
            }
        };
        let mut text = vec![0u8; len];
        if !unsafe { read_bytes(start, &mut text) } {
            log_message(format_args!(
                "gfx-equip: parse hook DISABLED -- .text read failed (start=0x{start:x} len={len})"
            ));
            break 'parse false;
        }
        let Some(addr) = scan_unique(&text, start, PARSE_SIG) else {
            log_message(format_args!(
                "gfx-equip: parse hook DISABLED -- signature not unique in live .text"
            ));
            break 'parse false;
        };
        if queue(addr, parse_hook as *mut c_void, &PARSE_ORIG, "parse") {
            log_message(format_args!("gfx-equip: parse hook resolved @0x{addr:x}"));
            true
        } else {
            false
        }
    };

    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            HOOK_ACTIVE.store(1, Ordering::SeqCst);
            log_message(format_args!(
                "gfx-equip: hooks ACTIVE -- file_open@0x{:x} (diag); parse reach {}",
                base + FILE_OPEN_RVA,
                if parse_armed {
                    "ARMED (02_011_equip ArtsBadge reaches the parse)"
                } else {
                    "DISABLED (fail-closed; no badge, no crash)"
                },
            ));
        }
        status => log_message(format_args!("gfx-equip: MH_ApplyQueued failed: {status:?}")),
    }
}
