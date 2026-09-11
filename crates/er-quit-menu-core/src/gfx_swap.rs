//! The two Scaleform movies the System>Quit rows stand on, derived in place from the game's own
//! vanilla payload.
//!
//! Moved out of `er-quickload`'s `experiments/startup_hooks/loading_cover/profile_table_gfx_files.rs`,
//! which still serves the loading cover, the title `05_000` and the profile table `05_010` from the
//! same file-open observer. Only the quit-menu half is here, because only the quit-menu half is
//! what a standalone quit-menu shell has to serve when the product is absent:
//!
//! * `02_040_optionsetting` -- the Quit Game panel's grid. Vanilla ships two cells and the mod's
//!   rows are cells three through six, so a shell that clones rows without this swap clones into
//!   cells that do not exist;
//! * `02_990_textinput` -- the link field the **Load Build from URL** row opens, under its own
//!   Scaleform cache key.
//!
//! # One deriver, by construction
//!
//! `er_gfx::options_02_040::quit6` fail-closes when its input is not the vanilla movie
//! (`Quit6Error::KnownInputBadOutput`), so a second deriver handed already-derived bytes correctly
//! refuses. That is why `scripts/me3-dll-conflicts.toml` pairs `er-quit-menu` with `er-quickload`
//! as a duplicate owner rather than a shared hook: whichever of them is loaded is the sole deriver,
//! and they are never loaded together.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use er_game_base::mem::{game_data_addr, safe_read_i32, safe_read_u8, safe_read_usize};

use crate::host::append_autoload_debug;

/// `Scaleform::MemoryFile`, the three fields a derive-and-swap rewrites.
const SCALEFORM_MEMORY_FILE_DATA_OFFSET: usize = 0x18;
const SCALEFORM_MEMORY_FILE_LEN_OFFSET: usize = 0x20;
const SCALEFORM_MEMORY_FILE_CURSOR_OFFSET: usize = 0x24;

/// A pointer value that is never a live object. Spelled as a named constant because a bare `0`
/// beside a heap address reads as arithmetic rather than as a sentinel.
const NOT_A_POINTER: usize = usize::MIN;

/// The canonical vanilla payload both 02_990 cache keys are redirected to, so the game's own shared
/// `02_990` cache entry is never mutated.
pub static TEXT_INPUT_02_990_CANONICAL_URL: &[u8] = b"data0:/menu/win/02_990_textinput.gfx\0";

static OPTIONS_02_040_QUIT6_EDITED: OnceLock<Vec<u8>> = OnceLock::new();
static OPTIONS_02_040_QUIT6_SERVES: AtomicUsize = AtomicUsize::new(0);
static OPTIONS_02_040_QUIT6_FAILURES: AtomicUsize = AtomicUsize::new(0);

static TEXT_INPUT_02_990_INLINE_EDITED: OnceLock<Vec<u8>> = OnceLock::new();
static TEXT_INPUT_02_990_INLINE_SERVES: AtomicUsize = AtomicUsize::new(0);
static TEXT_INPUT_02_990_INLINE_FAILURES: AtomicUsize = AtomicUsize::new(0);

static BUILD_URL_02_990_EDITED: OnceLock<Vec<u8>> = OnceLock::new();
static BUILD_URL_02_990_SERVES: AtomicUsize = AtomicUsize::new(0);
static BUILD_URL_02_990_FAILURES: AtomicUsize = AtomicUsize::new(0);

/// How many times each derivation has been served, for the telemetry line a run reads back.
pub fn gfx_swap_serves() -> (usize, usize, usize) {
    (
        OPTIONS_02_040_QUIT6_SERVES.load(Ordering::SeqCst),
        TEXT_INPUT_02_990_INLINE_SERVES.load(Ordering::SeqCst),
        BUILD_URL_02_990_SERVES.load(Ordering::SeqCst),
    )
}

/// Where one derivation's cache, counters, log tag and transform live together.
///
/// Two cache keys reach the 02_990 path (`02_990_TextInput_PathEditor` and
/// `02_990_TextInput_BuildUrl`), each redirected to the same canonical vanilla payload and each
/// deriving a different movie from it. Sharing one derivation is what put an unstyled link field in
/// the corner of the screen, so the two are kept apart by construction rather than by a flag.
struct Derivation {
    cache: &'static OnceLock<Vec<u8>>,
    serves: &'static AtomicUsize,
    failures: &'static AtomicUsize,
    tag: &'static str,
    derive: fn(&[u8]) -> Result<Vec<u8>, String>,
}

/// Write a derived payload over a `MemoryFile`'s data pointer, length and cursor.
///
/// # Safety
///
/// `file` must be a live `Scaleform::MemoryFile` whose vtable the caller has already checked, and
/// `edited` must live for the process lifetime -- the engine keeps the pointer.
unsafe fn install_payload(file: usize, edited: &'static [u8]) {
    unsafe {
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) as *mut usize,
            edited.as_ptr() as usize,
        );
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) as *mut u32,
            edited.len() as u32,
        );
        core::ptr::write((file + SCALEFORM_MEMORY_FILE_CURSOR_OFFSET) as *mut u32, 0);
    }
}

/// True when `file` is a `Scaleform::MemoryFile` of the running build.
///
/// # Safety
///
/// `file` is read through the fault-safe readers, so a wild value is a `false` rather than a fault.
unsafe fn memory_file_vtable_matches(base: usize, file: usize) -> bool {
    let want = game_data_addr(
        base,
        er_game_base::rva::SCALEFORM_MEMORY_FILE_VTABLE_RVA,
        "SCALEFORM_MEMORY_FILE_VTABLE_RVA",
    );
    unsafe { safe_read_usize(file) }.unwrap_or(0) == want
}

/// Derive-and-swap one 02_990 `MemoryFile` in place, fail-closed onto the untouched native payload.
///
/// # Safety
///
/// Called from the Scaleform file-open prologue with the file the native loader just returned.
unsafe fn text_input_02_990_swap(base: usize, file: usize, derivation: &Derivation) -> bool {
    let tag = derivation.tag;
    let fail = |reason: core::fmt::Arguments<'_>| {
        derivation.failures.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!("{tag}: 02_990 GFX edit FAIL-CLOSED: {reason}"));
        false
    };
    if file == 0 || file == NOT_A_POINTER {
        return fail(format_args!("invalid MemoryFile 0x{file:x}"));
    }
    if !unsafe { memory_file_vtable_matches(base, file) } {
        return fail(format_args!(
            "unexpected MemoryFile vtable 0x{:x}",
            unsafe { safe_read_usize(file) }.unwrap_or(0)
        ));
    }
    let edited = match derivation.cache.get() {
        Some(edited) => edited,
        None => {
            let data =
                unsafe { safe_read_usize(file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) }.unwrap_or(0);
            let len =
                unsafe { safe_read_i32(file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) }.unwrap_or(0);
            if data == 0 || data == NOT_A_POINTER || !(64..=0x0010_0000).contains(&len) {
                return fail(format_args!(
                    "implausible payload data=0x{data:x} len={len}"
                ));
            }
            // Safety: the length came from the file the native loader just populated, and the
            // vtable check above establishes that this really is a `MemoryFile`.
            let vanilla = unsafe { core::slice::from_raw_parts(data as *const u8, len as usize) };
            match (derivation.derive)(vanilla) {
                Ok(edited) => {
                    append_autoload_debug(format_args!(
                        "{tag}: derived 02_990 GFX in={} out={} fnv=0x{:016x}",
                        vanilla.len(),
                        edited.len(),
                        er_gfx::title_05_000::fnv1a64(&edited)
                    ));
                    derivation.cache.get_or_init(|| edited)
                }
                Err(error) => return fail(format_args!("{error}")),
            }
        }
    };
    unsafe { install_payload(file, edited) };
    derivation.serves.fetch_add(1, Ordering::SeqCst);
    true
}

/// Inline `02_990_textinput` over the save picker's CurrentPath field. Gated by the path editor's
/// own cache key at the file-open observer, so ordinary game text inputs retain vanilla geometry.
///
/// # Safety
///
/// See [`text_input_02_990_swap`].
pub unsafe fn text_input_02_990_swap_to_inline(base: usize, file: usize) -> bool {
    unsafe {
        text_input_02_990_swap(
            base,
            file,
            &Derivation {
                cache: &TEXT_INPUT_02_990_INLINE_EDITED,
                serves: &TEXT_INPUT_02_990_INLINE_SERVES,
                failures: &TEXT_INPUT_02_990_INLINE_FAILURES,
                tag: "save-picker-path",
                derive: |vanilla| {
                    er_gfx::text_input_02_990::inline_current_path_editor(vanilla)
                        .map_err(|error| error.to_string())
                },
            },
        )
    }
}

/// Centre `02_990_textinput` over the Quit tab for the **Load Build from URL** link field, with the
/// movie's own backing plate and frame art kept and widened to hold a planner link.
///
/// # Safety
///
/// See [`text_input_02_990_swap`].
pub unsafe fn text_input_02_990_swap_to_build_url(base: usize, file: usize) -> bool {
    unsafe {
        text_input_02_990_swap(
            base,
            file,
            &Derivation {
                cache: &BUILD_URL_02_990_EDITED,
                serves: &BUILD_URL_02_990_SERVES,
                failures: &BUILD_URL_02_990_FAILURES,
                tag: "system-quit-build-url",
                derive: |vanilla| {
                    er_gfx::build_url_02_990::centered_build_url_editor(vanilla)
                        .map_err(|error| error.to_string())
                        // Read the dim back out of the payload this is about to install, so a
                        // derivation that lost it is a counter rather than an undimmed field.
                        .and_then(crate::build_url_backdrop::attest_derived_build_url_backdrop)
                },
            },
        )
    }
}

/// Six-cell `02_040_optionsetting` runtime edit for System>Quit. This mirrors the 05_000/05_010
/// `MemoryFile` swap path, but deliberately has no env or file-backed diagnostic input: the product
/// must not ship or depend on an external GFx. The derived movie is built from the game's own
/// vanilla payload and cached for process lifetime so the native `MemoryFile`'s data pointer
/// remains valid.
///
/// # Safety
///
/// Called from the Scaleform file-open prologue with the file the native loader just returned.
pub unsafe fn options_02_040_quit6_swap_to_edited(base: usize, file: usize) -> bool {
    if file == 0 || file == NOT_A_POINTER {
        return false;
    }
    let fail = |reason: core::fmt::Arguments<'_>| {
        OPTIONS_02_040_QUIT6_FAILURES.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-gfx: 02_040 quit6 runtime edit FAIL-CLOSED (serving native vanilla): {reason}"
        ));
        false
    };
    if !unsafe { memory_file_vtable_matches(base, file) } {
        return fail(format_args!(
            "unexpected file vtable 0x{:x} (want MemoryFile 0x{:x})",
            unsafe { safe_read_usize(file) }.unwrap_or(0),
            game_data_addr(
                base,
                er_game_base::rva::SCALEFORM_MEMORY_FILE_VTABLE_RVA,
                "SCALEFORM_MEMORY_FILE_VTABLE_RVA"
            )
        ));
    }
    let edited = match OPTIONS_02_040_QUIT6_EDITED.get() {
        Some(cached) => cached,
        None => {
            let data =
                unsafe { safe_read_usize(file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) }.unwrap_or(0);
            let len =
                unsafe { safe_read_i32(file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) }.unwrap_or(0);
            if data == 0 || data == NOT_A_POINTER || !(64..=0x0100_0000).contains(&len) {
                return fail(format_args!(
                    "implausible payload data=0x{data:x} len={len}"
                ));
            }
            let len = len as usize;
            let magic_ok = unsafe { safe_read_u8(data) } == Some(b'G')
                && unsafe { safe_read_u8(data + 1) } == Some(b'F')
                && unsafe { safe_read_u8(data + 2) } == Some(b'X')
                && unsafe { safe_read_u8(data + len - 1) }.is_some();
            if !magic_ok {
                return fail(format_args!(
                    "payload at 0x{data:x} len={len} is unreadable or not GFX-magic"
                ));
            }
            // Safety: the length and the magic bytes were both read back through the fault-safe
            // readers before this slice is formed.
            let vanilla = unsafe { core::slice::from_raw_parts(data as *const u8, len) };
            let known = er_gfx::options_02_040::is_known_vanilla_win(vanilla);
            match er_gfx::options_02_040::quit6(vanilla) {
                Ok(out) => {
                    let out_fnv = er_gfx::title_05_000::fnv1a64(&out);
                    append_autoload_debug(format_args!(
                        "system-quit-gfx: 02_040 quit6 runtime edit derived in={len} out={} known_vanilla={known} out_fnv=0x{out_fnv:016x}",
                        out.len()
                    ));
                    OPTIONS_02_040_QUIT6_EDITED.get_or_init(|| out)
                }
                Err(err) => {
                    return fail(format_args!("in={len} known_vanilla={known}: {err}"));
                }
            }
        }
    };
    unsafe { install_payload(file, edited) };
    OPTIONS_02_040_QUIT6_SERVES.fetch_add(1, Ordering::SeqCst);
    true
}

// ---- serving the two movies from a standalone shell ------------------------------------------

/// This module's slot in the `er-hook` union chain for the Scaleform file-open prologue.
static FILE_OPEN_ORIG: AtomicUsize = AtomicUsize::new(0);
static FILE_OPEN_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static FILE_OPEN_HITS: AtomicUsize = AtomicUsize::new(0);

/// Scan the NUL-terminated ASCII path at `url` for `needle`, case-insensitively and bounded.
///
/// # Safety
///
/// No precondition on the address: every read goes through the fault-safe reader, so an unmapped
/// pointer answers `false` rather than faulting.
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
                buf[n] = b.to_ascii_lowercase();
                n += 1;
            }
        }
    }
    if n < needle.len() {
        return false;
    }
    buf[..n].windows(needle.len()).any(|w| w == needle)
}

/// Serve the Quit tab's grid and the link field's movie, chained onto whatever else detours the
/// Scaleform file-open prologue.
///
/// Union-shaped, not game-shaped: this prologue is already detoured by `er-armament-icons`, and two
/// MinHook instances on one prologue overwrite each other's trampolines -- measured, with the
/// product reporting `installed = true` and zero hits for a whole session while every GFx swap it
/// owns went silently vanilla. The game passes three arguments; the fourth register is ignored and
/// the flags word is forwarded unchanged.
///
/// # Safety
///
/// Installed by `er-hook` and called by the game on its own loader thread.
unsafe extern "system" fn quit_menu_scaleform_file_open_hook(
    loader: usize,
    url: usize,
    flags_reg: usize,
    _unused: usize,
) -> usize {
    let orig = FILE_OPEN_ORIG.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let hit = FILE_OPEN_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    let is_options_02_040 = unsafe { bounded_ascii_contains(url, b"02_040_optionsetting") };
    let is_build_url_02_990 = unsafe { bounded_ascii_contains(url, b"02_990_textinput_buildurl") };
    // A custom cache key forces a fresh Scaleform load. Redirect only that key's file-open to the
    // canonical native movie; the game's shared 02_990 cache entry stays untouched.
    let open_url = if is_build_url_02_990 {
        TEXT_INPUT_02_990_CANONICAL_URL.as_ptr() as usize
    } else {
        url
    };
    // Safety: the union publishes either the game trampoline (three arguments, the extra register
    // harmlessly ignored) or the next handler, which is four-argument. Calling a chained handler
    // with the game's narrower signature would leave its fourth register undefined.
    let next: er_hook::UnionFn = unsafe { std::mem::transmute(orig) };
    let native = unsafe { next(loader, open_url, flags_reg, 0) };
    if !(is_options_02_040 || is_build_url_02_990) {
        return native;
    }
    let Ok(base) = er_game_base::mem::game_module_base() else {
        return native;
    };
    let served = if is_options_02_040 {
        unsafe { options_02_040_quit6_swap_to_edited(base, native) }
    } else {
        unsafe { text_input_02_990_swap_to_build_url(base, native) }
    };
    append_autoload_debug(format_args!(
        "system-quit-gfx: served {} movie #{hit} loader=0x{loader:x} ret=0x{native:x} redirected_to_canonical_02_990={is_build_url_02_990} memory_replacement={served}",
        if is_options_02_040 {
            "02_040_optionsetting"
        } else {
            "02_990_textinput_buildurl"
        }
    ));
    native
}

/// Detour the Scaleform file-open prologue so this module serves the two movies the rows stand on.
///
/// Only a host that owns the derivation calls this. The product does not: it already detours the
/// same prologue for the loading cover, the title `05_000` and the profile table, and serves these
/// two from that handler. Two derivers of one movie is not a race to be won -- `quit6` fail-closes
/// on already-derived input by design -- which is why the pair is a `duplicate-owner` conflict in
/// `scripts/me3-dll-conflicts.toml` rather than a shared hook.
///
/// # Safety
///
/// Process attach or startup-hook context, before the title has loaded its movies.
pub unsafe fn install_quit_menu_gfx_swap_hook() -> bool {
    if FILE_OPEN_INSTALLED
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return true;
    }
    let rva = er_game_base::rva::TITLE_SCALEFORM_FILE_OPEN_RVA as u32;
    let Ok(addr) = er_game_base::mem::game_rva_for_hook(rva) else {
        append_autoload_debug(format_args!(
            "system-quit-gfx: failed to resolve the Scaleform file-open prologue rva 0x{rva:x}; the Quit grid stays vanilla and no rows will be visible"
        ));
        FILE_OPEN_INSTALLED.store(0, Ordering::SeqCst);
        return false;
    };
    match unsafe {
        er_hook::register_union_hook(addr, quit_menu_scaleform_file_open_hook, &FILE_OPEN_ORIG)
    } {
        Ok(()) => {
            append_autoload_debug(format_args!(
                "system-quit-gfx: registered the Scaleform file-open prologue 0x{addr:x} on the union; will serve the six-cell Quit grid and the link field's movie"
            ));
            true
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "system-quit-gfx: register_union_hook Scaleform file-open failed: {status:?}; the Quit grid stays vanilla and no rows will be visible"
            ));
            FILE_OPEN_INSTALLED.store(0, Ordering::SeqCst);
            false
        }
    }
}
