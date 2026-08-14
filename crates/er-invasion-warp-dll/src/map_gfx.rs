//! Serve an edited world-map movie so the invasion pins have a red icon to point at.
//!
//! # Why a movie edit is the colour lever
//!
//! A pin's icon id is a **1-based GFx frame number**, not a texture id and not a tint:
//! `CS::WorldMapPinData::SetTo` resolves it to `Icon_0.gotoAndStop(id)`, and frame `N` of that
//! clip places one `MENU_MAP_*` bitmap. So the set of looks a pin can have is exactly the set of
//! frames the movie populates, and "make the pins red" means "put a red bitmap on a frame".
//! [`er_gfx::world_map_pin`] does that on a frame the game never uses; this module is what gets
//! those bytes in front of Scaleform.
//!
//! # How the swap works
//!
//! Scaleform reads a movie out of a `MemoryFile` -- a small object holding `{data, len, cursor}`.
//! Detouring the tag-parse entry gives us that object before the header reader consumes it, so
//! repointing `data`/`len`/`cursor` at our own buffer serves the edited movie without touching
//! the game's allocation or the file on disk. This is the same mechanism
//! `er-armament-icons`'s `gfx_equip_hook` uses; the constants below are duplicated from it
//! rather than shared because the two DLLs ship independently, and each notes the other.
//!
//! # Identification is by CONTENT, not by name
//!
//! The parse path has no URL, only bytes. The movie is recognised by its GFX magic, its own
//! declared length, and an FNV fingerprint of the whole file. That is deliberately strict: it
//! recognises the VANILLA world-map movie and nothing else, so a player running another menu
//! mod that replaces `02_120_worldmap.gfx` gets THEIR movie untouched. They lose the red icon
//! (the pins fall back to a vanilla frame -- see
//! [`er_invasion_warp::param_row::invasion_pin_icon_id`]) and keep their mod, which is the right
//! way round.
//!
//! # Fail-closed
//!
//! Every step can decline, and declining always means "serve the game's own bytes". A map with
//! the wrong icon is a disappointment; a malformed tag stream handed to Scaleform is a crash on
//! a menu the player opens constantly.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use er_game_base::fnv1a::fnv1a64;
#[cfg(windows)]
use er_game_base::mem::{read_bytes, safe_read_i32, safe_read_u8, safe_read_usize};

/// Scaleform `MemoryFile` vtable RVA; a File whose first qword is `base + this` has the
/// `{data, len, cursor}` layout below.
const MEMORY_FILE_VTABLE_RVA: usize = er_game_base::rva::SCALEFORM_MEMORY_FILE_VTABLE_RVA;
const MEMORY_FILE_DATA_OFFSET: usize = 0x18;
const MEMORY_FILE_LEN_OFFSET: usize = 0x20;
const MEMORY_FILE_CURSOR_OFFSET: usize = 0x24;

/// GFx tag-parse entry `(loadProcess /*rcx*/, File* /*rdx*/) -> bool`, located by a unique
/// 30-byte position-independent prologue (verified unique in the 1.16.2 image). There is no
/// stable RVA recorded for it, which is why this is an AOB rather than a seam constant.
const PARSE_SIG: &str =
    "40 53 48 83 EC 40 48 8B 41 18 48 8B D9 C6 44 24 30 01 48 83 C1 50 4C 8B 50 20 4C 8B 58 48";

/// Vanilla `02_120_worldmap.gfx`, fingerprinted rather than vendored: this repo does not commit
/// game-derived bytes. Verified identical across two independent extractions of this build.
const WORLD_MAP_VANILLA_LEN: usize = 68_763;
const WORLD_MAP_VANILLA_FNV1A64: u64 = 0xed66_8483_91a2_d273;

/// Vanilla `01_080_emergencynotice.gfx` -- the announcement banner, fingerprinted the same way.
///
/// This is the surface the rejection notice is written to. Its one text field ships LEFT-aligned
/// inside a box ~1726 px wide, so a short line sat against the far edge; the edit centres it. Kept
/// in step with `er-gfx`'s own corpus test, which asserts the identical pair.
const NOTICE_VANILLA_LEN: usize = 3_205;
const NOTICE_VANILLA_FNV1A64: u64 = 0x6973_a088_b693_13f8;

/// Live game base, stored at install so the handler can validate the MemoryFile vtable.
static GAME_BASE: AtomicUsize = AtomicUsize::new(0);
/// Trampoline to the original tag-parse entry.
static ORIG_PARSE: AtomicUsize = AtomicUsize::new(0);
/// Whether the hook is installed.
static HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);

/// The edited movie, derived once and then served for the process lifetime.
static EDITED_MOVIE: OnceLock<Vec<u8>> = OnceLock::new();
/// The edited announcement movie, likewise.
static EDITED_NOTICE: OnceLock<Vec<u8>> = OnceLock::new();
/// Notice movies seen and served, so a run can say whether the centring reached the game.
static NOTICE_SEEN: AtomicUsize = AtomicUsize::new(0);
static NOTICE_SERVED: AtomicUsize = AtomicUsize::new(0);

/// Notice-movie tallies: `(seen, served)`.
#[must_use]
pub fn notice_tallies() -> (usize, usize) {
    (
        NOTICE_SEEN.load(Ordering::SeqCst),
        NOTICE_SERVED.load(Ordering::SeqCst),
    )
}

/// Counters, so a run can say what happened without a screenshot.
static PARSE_CALLS: AtomicUsize = AtomicUsize::new(0);
static WORLD_MAP_SEEN: AtomicUsize = AtomicUsize::new(0);
static SWAPS_SERVED: AtomicUsize = AtomicUsize::new(0);
static DERIVE_FAILURES: AtomicUsize = AtomicUsize::new(0);

/// Whether the red pin frame is actually in front of the game.
///
/// This is what decides the pins' icon id, and it is deliberately an OBSERVED fact rather than a
/// build-time constant: if the swap never happened -- another mod owns the movie, the hook did
/// not arm, the derive failed -- then pointing the pins at the red frame would point them at an
/// EMPTY frame and they would render no icon at all. Reading the outcome keeps the failure mode
/// "pins look like a vanilla icon" instead of "pins are invisible".
#[must_use]
pub fn red_pin_frame_installed() -> bool {
    SWAPS_SERVED.load(Ordering::SeqCst) > 0
}

/// `(parse_calls, world_map_seen, swaps_served, derive_failures)`.
#[must_use]
pub fn gfx_tallies() -> (usize, usize, usize, usize) {
    (
        PARSE_CALLS.load(Ordering::SeqCst),
        WORLD_MAP_SEEN.load(Ordering::SeqCst),
        SWAPS_SERVED.load(Ordering::SeqCst),
        DERIVE_FAILURES.load(Ordering::SeqCst),
    )
}

/// Parse a space-separated hex AOB with `??` wildcards into (bytes, mask).
fn parse_sig(sig: &str) -> Option<(Vec<u8>, Vec<bool>)> {
    let mut bytes = Vec::new();
    let mut mask = Vec::new();
    for token in sig.split_whitespace() {
        if token == "??" {
            bytes.push(0);
            mask.push(false);
        } else {
            bytes.push(u8::from_str_radix(token, 16).ok()?);
            mask.push(true);
        }
    }
    (!bytes.is_empty()).then_some((bytes, mask))
}

/// Find `sig` in `text`, requiring it to occur EXACTLY once.
///
/// Uniqueness is the point: a signature that matches twice does not identify a function, and
/// hooking the wrong one of two candidates patches a prologue we never verified.
fn scan_unique(text: &[u8], text_start: usize, sig: &str) -> Option<usize> {
    let (bytes, mask) = parse_sig(sig)?;
    let mut found = None;
    for (offset, window) in text.windows(bytes.len()).enumerate() {
        if window
            .iter()
            .zip(&bytes)
            .zip(&mask)
            .all(|((have, want), keep)| !*keep || have == want)
        {
            if found.is_some() {
                return None; // not unique
            }
            found = Some(text_start + offset);
        }
    }
    found
}

/// If `file` is a MemoryFile holding the vanilla world-map movie, serve the edited one.
///
/// # Safety
/// Game thread, inside the parse detour; every read goes through the fault-tolerant primitives.
#[cfg(windows)]
unsafe fn maybe_swap_world_map(base: usize, file: usize) {
    if file == 0 {
        return;
    }
    let vtable = unsafe { safe_read_usize(file) }.unwrap_or(0);
    if vtable != base + MEMORY_FILE_VTABLE_RVA {
        return; // not a MemoryFile -- an image or font stream
    }
    let data = unsafe { safe_read_usize(file + MEMORY_FILE_DATA_OFFSET) }.unwrap_or(0);
    let buffer_len = unsafe { safe_read_i32(file + MEMORY_FILE_LEN_OFFSET) }.unwrap_or(0);
    if data == 0 || buffer_len <= 0 {
        return;
    }
    // GFX magic through the guarded reader before any bulk read.
    if unsafe { safe_read_u8(data) } != Some(b'G')
        || unsafe { safe_read_u8(data + 1) } != Some(b'F')
        || unsafe { safe_read_u8(data + 2) } != Some(b'X')
    {
        return;
    }
    // The File's +0x20 is the ALLOCATION size, which the loader rounds up; the movie's own
    // declared length (u32 at +4) is the content length. Gating on the allocation size instead
    // is how this pattern previously matched nothing at all.
    let declared = {
        let raw: [u8; 4] =
            core::array::from_fn(|i| unsafe { safe_read_u8(data + 4 + i) }.unwrap_or(0));
        u32::from_le_bytes(raw) as usize
    };
    // Two movies are edited now, so the cheap length gate tests both before any bulk read. Every
    // other movie the game loads leaves here having cost four byte reads.
    if declared != WORLD_MAP_VANILLA_LEN && declared != NOTICE_VANILLA_LEN {
        return;
    }
    if declared > buffer_len as usize {
        return;
    }
    let mut input = vec![0_u8; declared];
    if !unsafe { read_bytes(data, &mut input) } {
        return;
    }
    let fingerprint = fnv1a64(&input);
    if declared == NOTICE_VANILLA_LEN && fingerprint == NOTICE_VANILLA_FNV1A64 {
        unsafe { swap_notice(file, &input) };
        return;
    }
    if fingerprint != WORLD_MAP_VANILLA_FNV1A64 {
        // Same length, different bytes: another mod's movie. Serve theirs untouched.
        return;
    }
    WORLD_MAP_SEEN.fetch_add(1, Ordering::SeqCst);

    let edited = match EDITED_MOVIE.get() {
        Some(cached) => cached,
        None => match er_gfx::world_map_pin::with_red_pin_frame(&input) {
            Ok(bytes) => {
                crate::standalone_log(format_args!(
                    "map-gfx: derived world-map movie with a red pin on frame {} (in={} out={})",
                    er_gfx::world_map_pin::RED_PIN_FRAME,
                    input.len(),
                    bytes.len()
                ));
                EDITED_MOVIE.get_or_init(|| bytes)
            }
            Err(error) => {
                DERIVE_FAILURES.fetch_add(1, Ordering::SeqCst);
                crate::standalone_log(format_args!(
                    "map-gfx: red pin frame NOT installed ({error}); serving the game's own \
                     movie and falling back to a vanilla pin icon"
                ));
                return;
            }
        },
    };
    // The buffer is leaked for the process lifetime by OnceLock, which is required: Scaleform
    // reads from it lazily after this returns.
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
    let served = SWAPS_SERVED.fetch_add(1, Ordering::SeqCst) + 1;
    crate::standalone_log(format_args!(
        "map-gfx: served edited world-map movie #{served} ({} bytes); invasion pins will use \
         icon frame {}",
        edited.len(),
        er_gfx::world_map_pin::RED_PIN_FRAME
    ));
}

/// Serve the announcement movie with its text field centred.
///
/// Separate from the world-map path because the two share only the MemoryFile plumbing: this one
/// derives a different edit, and a failure here must leave the banner working (left-aligned) rather
/// than take the map down with it.
///
/// # Safety
/// Game thread, inside the parse detour. Writes only the MemoryFile's `{data, len, cursor}`.
#[cfg(windows)]
unsafe fn swap_notice(file: usize, input: &[u8]) {
    NOTICE_SEEN.fetch_add(1, Ordering::SeqCst);
    let edited = match EDITED_NOTICE.get() {
        Some(cached) => cached,
        None => match er_gfx::announce_notice::with_centered_notice_text(input) {
            Ok(bytes) => {
                crate::standalone_log(format_args!(
                    "map-gfx: derived the announcement movie with its text CENTRED (in={} out={}) \
                     -- the notice field ships left-aligned in a box far wider than any line we \
                     write, which is why a short message sat against the edge",
                    input.len(),
                    bytes.len()
                ));
                EDITED_NOTICE.get_or_init(|| bytes)
            }
            Err(error) => {
                crate::standalone_log(format_args!(
                    "map-gfx: notice text NOT centred ({error}); serving the game's own movie, so \
                     the banner still works and is merely left-aligned"
                ));
                return;
            }
        },
    };
    // The buffer is leaked for the process lifetime by OnceLock, which is required: Scaleform reads
    // from it lazily after this returns.
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
    let served = NOTICE_SERVED.fetch_add(1, Ordering::SeqCst) + 1;
    crate::standalone_log(format_args!(
        "map-gfx: served centred announcement movie #{served} ({} bytes)",
        edited.len()
    ));
}

/// Union handler for the GFx tag-parse entry.
///
/// # Safety
/// Installed by the union on an AOB-located, uniqueness-checked prologue; ABI is
/// `(loadProcess, File*) -> bool`, which fits the four-argument dispatcher.
#[cfg(windows)]
unsafe extern "system" fn parse_hook(
    load_process: usize,
    file: usize,
    c: usize,
    d: usize,
) -> usize {
    PARSE_CALLS.fetch_add(1, Ordering::SeqCst);
    let base = GAME_BASE.load(Ordering::SeqCst);
    if base != 0 {
        unsafe { maybe_swap_world_map(base, file) };
    }
    let orig = ORIG_PARSE.load(Ordering::SeqCst);
    if orig == 0 {
        // Claiming a parse result we did not compute would tell the loader a movie parsed when
        // nothing read it.
        return 0;
    }
    type ParseFn = unsafe extern "system" fn(usize, usize, usize, usize) -> usize;
    let original: ParseFn = unsafe { core::mem::transmute(orig) };
    unsafe { original(load_process, file, c, d) }
}

/// Install the movie-swap hook. Returns how many hooks bound (0 or 1).
///
/// A failure here costs the red icon and nothing else: the pins fall back to a vanilla frame.
///
/// # Safety
/// Call once, from the game task thread after the runtime is up.
#[cfg(windows)]
pub unsafe fn install_world_map_gfx_hook(base: usize) -> usize {
    if HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return 0;
    }
    GAME_BASE.store(base, Ordering::SeqCst);

    let Some((start, len)) = er_game_base::mem::module_text_range()
        .filter(|(_, len)| (0x1000..=0x0800_0000).contains(len))
    else {
        crate::standalone_log(format_args!(
            "map-gfx: DISABLED -- .text range unresolved; pins keep a vanilla icon"
        ));
        return 0;
    };
    let mut text = vec![0_u8; len];
    if !unsafe { read_bytes(start, &mut text) } {
        crate::standalone_log(format_args!(
            "map-gfx: DISABLED -- .text read failed (start=0x{start:x} len={len})"
        ));
        return 0;
    }
    let Some(address) = scan_unique(&text, start, PARSE_SIG) else {
        crate::standalone_log(format_args!(
            "map-gfx: DISABLED -- the GFx parse signature is absent or not unique in this \
             build's .text; refusing to guess which function to hook"
        ));
        return 0;
    };
    match unsafe {
        er_hook::register_union_hook(address, parse_hook as er_hook::UnionFn, &ORIG_PARSE)
    } {
        Ok(()) => {
            crate::standalone_log(format_args!(
                "map-gfx: hooked the GFx tag parser @0x{address:x}; the world-map movie will be \
                 served with a red pin on frame {}",
                er_gfx::world_map_pin::RED_PIN_FRAME
            ));
            1
        }
        Err(status) => {
            crate::standalone_log(format_args!(
                "map-gfx: union registration @0x{address:x} failed: {status:?} -- pins keep a \
                 vanilla icon"
            ));
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vanilla_fingerprint_matches_the_movie_the_edit_was_derived_against() {
        // Same constants the er-gfx corpus test pins. If these drift apart, the DLL would
        // decline to swap a movie the offline test happily edits -- a silent no-op feature.
        assert_eq!(WORLD_MAP_VANILLA_LEN, 68_763);
        assert_eq!(WORLD_MAP_VANILLA_FNV1A64, 0xed66_8483_91a2_d273);
    }

    #[test]
    fn a_signature_that_is_not_unique_resolves_to_nothing() {
        let text = [0x90_u8, 0x91, 0x90, 0x91];
        assert_eq!(scan_unique(&text, 0x1000, "90 91"), None, "two matches");
        assert_eq!(scan_unique(&text, 0x1000, "90 91 90"), Some(0x1000));
        assert_eq!(scan_unique(&text, 0x1000, "AB CD"), None, "no match");
    }

    #[test]
    fn wildcards_are_honoured() {
        let text = [0x40_u8, 0x53, 0x48];
        assert_eq!(scan_unique(&text, 0, "40 ?? 48"), Some(0));
    }

    #[test]
    fn the_pin_icon_falls_back_when_no_swap_was_served() {
        // The safety property: an un-swapped movie must never leave pins pointing at an empty
        // frame, because an empty frame draws nothing at all.
        use er_invasion_warp::param_row::{
            FALLBACK_INVASION_PIN_ICON_ID, RED_INVASION_PIN_FRAME, invasion_pin_icon_id,
        };
        assert_eq!(invasion_pin_icon_id(false), FALLBACK_INVASION_PIN_ICON_ID);
        assert_eq!(invasion_pin_icon_id(true), RED_INVASION_PIN_FRAME);
        assert_ne!(FALLBACK_INVASION_PIN_ICON_ID, RED_INVASION_PIN_FRAME);
        // And the red frame must be the one er-gfx actually writes.
        assert_eq!(
            RED_INVASION_PIN_FRAME,
            er_gfx::world_map_pin::RED_PIN_FRAME,
            "the icon id and the installed frame must be the same number"
        );
    }
}
