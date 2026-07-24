//! Runtime GFX template edit for the equip menu (`data0:/menu/02_011_equip.gfx`),
//! isolated in this badge DLL (bd er-effects-rs-pe98).
//!
//! The Ash-of-War badge draws into the tile's `ArtsIcon/IconImage` child, but
//! vanilla leaves that child empty (zero bounds) so the icon setter's rect/tex
//! scale is zero and nothing paints (proven: runtime rect trace 20260723-155530).
//! `er_gfx::equip_02_011` re-points `ArtsIcon`'s `IconImage` to `ItemIcon`'s proven
//! sized `IconImage`, giving it a real 160px rect. This module applies that
//! content-addressed edit in memory: it hooks the game's Scaleform file-open path,
//! and when the equip movie is opened it derives the edited movie from the game's
//! OWN vanilla payload and swaps the MemoryFile's data/len onto the cached buffer.
//! Fail-closed: any anomaly leaves the native (vanilla) movie untouched.
//!
//! This mirrors the product crate's title/options GFX swap
//! (`profile_table_gfx_files.rs`) but is self-contained here so the badge DLL keeps
//! no dependency on the product. Constants are the same ground-truthed 1.16.1 RVAs.

#![cfg(windows)]

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use er_game_base::mem::{safe_read_i32, safe_read_u8, safe_read_usize};

use crate::log_message;

/// Scaleform file-open observer target (dump-ground-truthed 1.16.1 deobf RVA; the
/// product crate hooks the same `TITLE_SCALEFORM_FILE_OPEN_RVA`). Signature
/// `fn(loader, url, flags) -> MemoryFile*`.
const FILE_OPEN_RVA: usize = 0x11ced80;
/// Scaleform `MemoryFile` vtable RVA; a returned file whose first qword equals
/// `base + this` is a MemoryFile with the data/len layout below.
const MEMORY_FILE_VTABLE_RVA: usize = 0x2ba4c80;
/// MemoryFile field offsets (data ptr / byte len / read cursor).
const MEMORY_FILE_DATA_OFFSET: usize = 0x18;
const MEMORY_FILE_LEN_OFFSET: usize = 0x20;
const MEMORY_FILE_CURSOR_OFFSET: usize = 0x24;

// --- Cache-miss forcing (bd armament-icons-rootcause-createmovie-cache-hit-blocks-swap) ---
// The file-open swap alone does NOT reach 02_011's parse: `GFxLoader::CreateMovie`
// does a URL-keyed resource-lib cache lookup and only opens+parses on a MISS. 02_011
// is parsed+cached at boot, so the equip mount is a cache HIT and file-open (our swap)
// never runs. We hook CreateMovie to recognize the equip URL and arm a one-shot, and
// hook the cache lookup to evict the cached entry (forcing a miss+reparse) so the
// file-open swap applies exactly once, after which the cache serves our edited def.
//
// Version-agnostic function discovery. Hardcoded RVAs drift between game patches
// (1.16.1 addresses crash-hooked 1.16.2 -- bd armament-icons-cachemiss-hooks-crash-1162-
// address-drift), so each function is located by a UNIQUE prologue AOB scanned in the
// live `.text`. Signatures come from the 1.16.2 dump (== live process bytes); `??` is a
// wildcard for relative-offset bytes. An empty signature DISABLES that hook (fail-closed)
// until a verified signature is filled in -- so an unresolved/ambiguous scan never
// crash-hooks; it just leaves the badge un-reached (no regression vs file-open-only).
//
/// `GFxLoader::CreateMovie(loader, name, flags, a4, a5) -> Movie*` (1.16.2, verified
/// count==1; 37 position-independent bytes, no wildcards).
const CREATE_MOVIE_SIG: &str = "48 89 5C 24 10 4C 89 4C 24 20 48 89 4C 24 08 55 56 57 41 54 41 55 41 56 41 57 48 8D 6C 24 E1 48 81 EC A0 00 00 00";
/// `ResourceLib::GetOrAdd(reslib, out_result, &key) -> int` (1/2=hit, 3=miss, 4=alloc-fail).
/// Prologue through the reslib lock-acquire setup (`add rcx,0x18; ...; mov r14,rdx`), unique.
const CACHE_GETORADD_SIG: &str = "48 89 5C 24 10 48 89 6C 24 18 56 57 41 55 41 56 41 57 48 83 EC 30 4C 8B E9 49 8B E8 48 83 C1 18 4C 8B F2";
/// `ResourceLib::Remove(hashtable = reslib+0x48, &key)`. The 20-byte prologue alone hits 3
/// template siblings; the tail (indirect hash vcall `FF 50 20` + 0x20 stride) disambiguates.
/// `75 ??` / `74 ??` are rel8 short-jumps wildcarded.
const CACHE_EVICT_SIG: &str = "40 56 41 57 48 83 EC 28 48 83 39 00 4C 8B FA 48 8B F1 75 ?? 32 C0 48 83 C4 28 41 5F 5E C3 48 8B 0A 48 89 5C 24 40 48 89 7C 24 50 48 85 C9 74 ?? 48 8B 01 48 8B 52 08 FF 50 20";
/// The reslib (resource library / movie-def cache) pointer lives at `loader+0x60`.
const LOADER_RESLIB_OFFSET: usize = 0x60;
/// The reslib's hash-table base offset (eviction operates on `reslib+0x48`).
const RESLIB_HASHTABLE_OFFSET: usize = 0x48;

/// URL fragment identifying the target boot-cached menu to force-reparse.
const EQUIP_URL_NEEDLE: &[u8] = b"02_011_equip";

/// Scanned live address of `ResourceLib::Remove` (evict), 0 until resolved.
static EVICT_ADDR: AtomicUsize = AtomicUsize::new(0);
static CREATE_MOVIE_ORIG: AtomicUsize = AtomicUsize::new(ORIG_UNSET);
static GETORADD_ORIG: AtomicUsize = AtomicUsize::new(ORIG_UNSET);

// One-shot: the equip def is force-reparsed exactly once; afterwards the cache serves
// our edited def and both hooks fall through untouched.
static EQUIP_REPARSE_DONE: AtomicUsize = AtomicUsize::new(0);

// oracle counters for the cache-miss forcing path
static CREATE_MOVIE_EQUIP_HITS: AtomicU64 = AtomicU64::new(0);
static CACHE_EVICTIONS: AtomicU64 = AtomicU64::new(0);

// -- DIAGNOSTIC: map the 02_011 load path (bounded logging of the boot creation/open sequence) --
static CREATE_MOVIE_TOTAL: AtomicU64 = AtomicU64::new(0);
static GFX_OPEN_TOTAL: AtomicU64 = AtomicU64::new(0);
/// How many CreateMovie names / .gfx open URLs to log before going quiet.
const DIAG_LOG_LIMIT: u64 = 80;

/// Read the NUL-terminated ASCII name at `url` into `out`, returning the filled length
/// (bounded). For bounded diagnostic logging of movie names / open URLs.
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

thread_local! {
    /// Set to the `loader` pointer by the CreateMovie hook for the duration of a
    /// matched (02_011_equip) CreateMovie call, so the nested cache lookup on that
    /// loader's reslib can be recognized and evicted. 0 = not armed.
    static ARM_LOADER: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Sentinel for "trampoline not installed yet".
const ORIG_UNSET: usize = 0;

static FILE_OPEN_ORIG: AtomicUsize = AtomicUsize::new(ORIG_UNSET);
static HOOK_ACTIVE: AtomicUsize = AtomicUsize::new(0);

// -- oracle counters (machine-checkable) --
static EQUIP_OPEN_HITS: AtomicU64 = AtomicU64::new(0);
static EQUIP_SWAP_SERVES: AtomicU64 = AtomicU64::new(0);
static EQUIP_SWAP_FAILURES: AtomicU64 = AtomicU64::new(0);

/// Process-lifetime cache of the derived edited movie. The MemoryFile's data
/// pointer aliases this buffer, so it must never move or free.
static EQUIP_EDITED: OnceLock<Vec<u8>> = OnceLock::new();

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

/// Derive-and-swap the equip movie in place. Returns true only if the native
/// MemoryFile now points at the badge-enabled movie. Fail-closed otherwise.
unsafe fn swap_equip_to_edited(base: usize, file: usize) -> bool {
    let fail = |reason: core::fmt::Arguments<'_>| {
        EQUIP_SWAP_FAILURES.fetch_add(1, Ordering::SeqCst);
        log_message(format_args!(
            "gfx-equip: 02_011 swap FAIL-CLOSED (serving native vanilla): {reason}"
        ));
        false
    };
    if file == 0 {
        return false;
    }
    let vtable = unsafe { safe_read_usize(file) }.unwrap_or(0);
    if vtable != base + MEMORY_FILE_VTABLE_RVA {
        return fail(format_args!(
            "unexpected file vtable 0x{vtable:x} (want MemoryFile 0x{:x})",
            base + MEMORY_FILE_VTABLE_RVA
        ));
    }

    // DIAGNOSTIC: log the native file's data/len on EVERY open so we can tell whether the
    // game re-parses from a fresh vanilla payload each time (data != our cached buffer) or
    // returns a cached def. If a later open already shows data == our edited buffer, the swap
    // persisted; if it always shows a fresh vanilla-length payload, the def is parsed elsewhere.
    {
        let pre_data = unsafe { safe_read_usize(file + MEMORY_FILE_DATA_OFFSET) }.unwrap_or(0);
        let pre_len = unsafe { safe_read_i32(file + MEMORY_FILE_LEN_OFFSET) }.unwrap_or(0);
        let ours = EQUIP_EDITED.get().map(|v| v.as_ptr() as usize).unwrap_or(0);
        log_message(format_args!(
            "gfx-equip: 02_011 open file=0x{file:x} pre_data=0x{pre_data:x} pre_len={pre_len} \
             our_buf=0x{ours:x} (data==ours? {})",
            pre_data == ours && ours != 0
        ));
    }
    let edited: &Vec<u8> = match EQUIP_EDITED.get() {
        Some(cached) => cached,
        None => {
            let data = unsafe { safe_read_usize(file + MEMORY_FILE_DATA_OFFSET) }.unwrap_or(0);
            let len = unsafe { safe_read_i32(file + MEMORY_FILE_LEN_OFFSET) }.unwrap_or(0);
            if data == 0 || !(64..=0x0100_0000).contains(&len) {
                return fail(format_args!(
                    "implausible payload data=0x{data:x} len={len}"
                ));
            }
            let len = len as usize;
            // Probe both ends + GFX magic through the guarded reader before the bulk read.
            let magic_ok = unsafe { safe_read_u8(data) } == Some(b'G')
                && unsafe { safe_read_u8(data + 1) } == Some(b'F')
                && unsafe { safe_read_u8(data + 2) } == Some(b'X')
                && unsafe { safe_read_u8(data + len - 1) }.is_some();
            if !magic_ok {
                return fail(format_args!(
                    "payload at 0x{data:x} len={len} unreadable or not GFX-magic"
                ));
            }
            let vanilla = unsafe { core::slice::from_raw_parts(data as *const u8, len) };
            let known = er_gfx::equip_02_011::is_known_vanilla(vanilla);
            match er_gfx::equip_02_011::arts_badge(vanilla) {
                Ok(out) => {
                    log_message(format_args!(
                        "gfx-equip: 02_011 derived badge movie in={len} out={} known_vanilla={known}",
                        out.len()
                    ));
                    EQUIP_EDITED.get_or_init(|| out)
                }
                Err(err) => return fail(format_args!("derive in={len} known={known}: {err}")),
            }
        }
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
    EQUIP_SWAP_SERVES.fetch_add(1, Ordering::SeqCst);
    true
}

/// File-open post-hook: run the original, and if the equip movie was opened swap it
/// to the badge-enabled edit. Every other file passes straight through.
unsafe extern "system" fn file_open_hook(loader: usize, url: usize, flags: u32) -> usize {
    let orig = FILE_OPEN_ORIG.load(Ordering::SeqCst);
    let native = if orig != ORIG_UNSET {
        let f: unsafe extern "system" fn(usize, usize, u32) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(loader, url, flags) }
    } else {
        0
    };
    // DIAGNOSTIC: log the first DIAG_LOG_LIMIT `.gfx` opens to map the menu-movie open
    // sequence (does 02_011 open once or several times, and when relative to the mount?).
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
    if unsafe { bounded_ascii_contains(url, b"02_011_equip") } {
        let hits = EQUIP_OPEN_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        let base = er_game_base::mem::game_module_base().unwrap_or(0);
        let served = if base != 0 {
            unsafe { swap_equip_to_edited(base, native) }
        } else {
            false
        };
        log_message(format_args!(
            "gfx-equip: 02_011_equip open hit #{hits} native=0x{native:x} served={served} \
             serves={} failures={}",
            EQUIP_SWAP_SERVES.load(Ordering::SeqCst),
            EQUIP_SWAP_FAILURES.load(Ordering::SeqCst),
        ));
    }
    native
}

/// `GFxLoader::CreateMovie` hook. When the equip movie is (re)created and we have
/// not yet forced a reparse, arm the thread-local so the nested cache lookup on this
/// loader's reslib is recognized and evicted (forcing a miss -> open -> our swap).
unsafe extern "system" fn create_movie_hook(
    loader: usize,
    name: usize,
    flags: u32,
    a4: usize,
    a5: usize,
) -> usize {
    // DIAGNOSTIC: log the first DIAG_LOG_LIMIT CreateMovie names to map when/how 02_011 is
    // created (boot vs mount) and its exact name string.
    let cm_n = CREATE_MOVIE_TOTAL.fetch_add(1, Ordering::SeqCst) + 1;
    if cm_n <= DIAG_LOG_LIMIT {
        let mut buf = [0u8; 96];
        let n = unsafe { read_bounded_name(name, &mut buf) };
        log_message(format_args!(
            "gfx-equip: CreateMovie #{cm_n} name='{}'",
            core::str::from_utf8(&buf[..n]).unwrap_or("<non-utf8>")
        ));
    }
    let matched = EQUIP_REPARSE_DONE.load(Ordering::SeqCst) == 0
        && unsafe { bounded_ascii_contains(name, EQUIP_URL_NEEDLE) };
    if matched {
        ARM_LOADER.with(|c| c.set(loader));
        let n = CREATE_MOVIE_EQUIP_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        log_message(format_args!(
            "gfx-equip: CreateMovie(02_011_equip) #{n} loader=0x{loader:x} -- arming cache-miss force"
        ));
    }
    let orig = CREATE_MOVIE_ORIG.load(Ordering::SeqCst);
    let ret = if orig != ORIG_UNSET {
        let f: unsafe extern "system" fn(usize, usize, u32, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(loader, name, flags, a4, a5) }
    } else {
        0
    };
    if matched {
        // Disarm regardless: the nested lookup either fired (one-shot latched) or this
        // CreateMovie did not route through the reslib lookup we expected.
        ARM_LOADER.with(|c| c.set(0));
    }
    ret
}

/// `ResourceLib::GetOrAdd` hook. Only acts when armed by a matched CreateMovie AND the
/// reslib is this loader's movie-def cache (`loader+0x60`): evict the just-looked-up
/// key so the original call misses and the caller re-opens+parses (our file-open swap
/// then applies). One-shot; every other lookup passes straight through.
unsafe extern "system" fn getoradd_hook(reslib: usize, out_result: usize, key: usize) -> u32 {
    let orig = GETORADD_ORIG.load(Ordering::SeqCst);
    let call_orig = |reslib: usize, out_result: usize, key: usize| -> u32 {
        if orig != ORIG_UNSET {
            let f: unsafe extern "system" fn(usize, usize, usize) -> u32 =
                unsafe { std::mem::transmute(orig) };
            unsafe { f(reslib, out_result, key) }
        } else {
            0
        }
    };

    let armed_loader = ARM_LOADER.with(|c| c.get());
    if armed_loader != 0 && EQUIP_REPARSE_DONE.load(Ordering::SeqCst) == 0 {
        let want_reslib =
            unsafe { safe_read_usize(armed_loader + LOADER_RESLIB_OFFSET) }.unwrap_or(0);
        if want_reslib != 0 && reslib == want_reslib {
            // Latch the one-shot BEFORE re-entrant calls; disarm so only the first
            // movie-def lookup in this CreateMovie is forced to miss.
            EQUIP_REPARSE_DONE.store(1, Ordering::SeqCst);
            ARM_LOADER.with(|c| c.set(0));
            let evict_addr = EVICT_ADDR.load(Ordering::SeqCst);
            if evict_addr != 0 {
                let evict: unsafe extern "system" fn(usize, usize) -> usize =
                    unsafe { std::mem::transmute(evict_addr) };
                unsafe { evict(reslib + RESLIB_HASHTABLE_OFFSET, key) };
                CACHE_EVICTIONS.fetch_add(1, Ordering::SeqCst);
                log_message(format_args!(
                    "gfx-equip: evicted 02_011 cache entry (reslib=0x{reslib:x} key=0x{key:x}); \
                     forcing miss+reparse so the badge swap reaches parse"
                ));
            } else {
                log_message(format_args!(
                    "gfx-equip: cannot evict -- evict addr unresolved; 02_011 stays cached (no badge)"
                ));
            }
            // Original now inserts a fresh placeholder (miss) -> caller re-opens+parses.
            return call_orig(reslib, out_result, key);
        }
    }
    call_orig(reslib, out_result, key)
}

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

/// Install the file-open swap (known-good RVA) plus the CreateMovie/GetOrAdd cache-miss
/// forcing, the latter located by signature scan in the live `.text` (version-agnostic).
/// Idempotent. The cache-miss trio installs ATOMICALLY: all three functions must resolve
/// to a unique address AND both hooks must apply, else NONE of the cache-miss forcing is
/// enabled (fail-closed -- the badge just won't reach a boot-cached parse, no crash).
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

    // 1. File-open swap: known-good, hardcoded RVA (works on 1.16.2). Required for any badge.
    if !queue(
        base + FILE_OPEN_RVA,
        file_open_hook as *mut c_void,
        &FILE_OPEN_ORIG,
        "file_open",
    ) {
        return;
    }

    // 2. Cache-miss forcing: resolve CreateMovie / GetOrAdd / evict by signature scan of the
    //    live .text (one read, three scans). Atomic + fail-closed.
    let cache_miss_armed = 'cache: {
        let (cm_addr, ga_addr, ev_addr) = match er_game_base::mem::module_text_range() {
            Some((start, len)) if (0x1000..=0x0800_0000).contains(&len) => {
                let mut text = vec![0u8; len];
                if !unsafe { er_game_base::mem::read_bytes(start, &mut text) } {
                    log_message(format_args!(
                        "gfx-equip: cache-miss disabled -- .text read failed (start=0x{start:x} len={len})"
                    ));
                    break 'cache false;
                }
                let cm = scan_unique(&text, start, CREATE_MOVIE_SIG);
                let ga = scan_unique(&text, start, CACHE_GETORADD_SIG);
                let ev = scan_unique(&text, start, CACHE_EVICT_SIG);
                match (cm, ga, ev) {
                    (Some(c), Some(g), Some(e)) => (c, g, e),
                    _ => {
                        log_message(format_args!(
                            "gfx-equip: cache-miss disabled -- signature scan not unique \
                             (create_movie={cm:x?} getoradd={ga:x?} evict={ev:x?}); badge won't \
                             reach boot-cached parse until sigs are verified for this build"
                        ));
                        break 'cache false;
                    }
                }
            }
            other => {
                log_message(format_args!(
                    "gfx-equip: cache-miss disabled -- .text range unresolved ({other:x?})"
                ));
                break 'cache false;
            }
        };
        // ATOMIC: create BOTH detours before enabling EITHER. A drifted/unhookable address
        // fails here at MhHook::new (this is exactly how getoradd failed on the 1.16.1-addr
        // crash), so neither gets enabled and create_movie never runs half-wired.
        let cm_hook =
            unsafe { MhHook::new(cm_addr as *mut c_void, create_movie_hook as *mut c_void) };
        let ga_hook = unsafe { MhHook::new(ga_addr as *mut c_void, getoradd_hook as *mut c_void) };
        let (cm_hook, ga_hook) = match (cm_hook, ga_hook) {
            (Ok(c), Ok(g)) => (c, g),
            (c, g) => {
                log_message(format_args!(
                    "gfx-equip: cache-miss disabled -- MhHook::new failed \
                     (create_movie@0x{cm_addr:x} ok={} getoradd@0x{ga_addr:x} ok={})",
                    c.is_ok(),
                    g.is_ok()
                ));
                break 'cache false;
            }
        };
        CREATE_MOVIE_ORIG.store(cm_hook.trampoline() as usize, Ordering::SeqCst);
        GETORADD_ORIG.store(ga_hook.trampoline() as usize, Ordering::SeqCst);
        EVICT_ADDR.store(ev_addr, Ordering::SeqCst);
        if unsafe { cm_hook.queue_enable() }.is_err() || unsafe { ga_hook.queue_enable() }.is_err()
        {
            CREATE_MOVIE_ORIG.store(ORIG_UNSET, Ordering::SeqCst);
            GETORADD_ORIG.store(ORIG_UNSET, Ordering::SeqCst);
            EVICT_ADDR.store(0, Ordering::SeqCst);
            log_message(format_args!(
                "gfx-equip: cache-miss disabled -- queue_enable failed"
            ));
            break 'cache false;
        }
        log_message(format_args!(
            "gfx-equip: cache-miss forcing resolved -- create_movie@0x{cm_addr:x} \
             getoradd@0x{ga_addr:x} evict@0x{ev_addr:x}"
        ));
        true
    };

    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            HOOK_ACTIVE.store(1, Ordering::SeqCst);
            log_message(format_args!(
                "gfx-equip: hooks ACTIVE -- file_open@0x{:x}; cache-miss forcing {}",
                base + FILE_OPEN_RVA,
                if cache_miss_armed {
                    "ARMED (02_011_equip badge will reach parse)"
                } else {
                    "DISABLED (file-open swap only; fail-closed)"
                },
            ));
        }
        status => log_message(format_args!("gfx-equip: MH_ApplyQueued failed: {status:?}")),
    }
}
