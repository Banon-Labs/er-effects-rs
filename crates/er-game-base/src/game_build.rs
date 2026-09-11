//! Which ELDEN RING this DLL is loaded into, read from the running image's own version resource.
//!
//! # Why this exists
//!
//! Every game address in this workspace is a 1.16.2 RVA. On 2026-08-27 the game shipped 1.17,
//! which moved code, and a detour installed at a 1.16.2 RVA landed in the middle of a different
//! 1.17 function: `0x1407ada40` is a real prologue in 1.16.2 (`push rbp; push rsi; push rdi`) and
//! `xor r15d, r15d` in 1.17. The result was an access violation ~3.5s into boot with a fabricated
//! `image_base | 0x64` pointer in `rcx` -- a crash whose backtrace points into game code and gives
//! no hint that the real fault was an address from a previous patch.
//!
//! A hook installed on an unrecognised build cannot be made safe by trying harder at the call
//! site; the address itself is meaningless. So the useful thing to know, before any detour goes
//! in, is simply *which build is this* -- and the image says so itself.
//!
//! # What it does not do
//!
//! It does not gate `er-hook`'s `write_code_byte`, which takes an absolute address its caller
//! discovered by scanning rather than a 1.16.2 RVA -- there is nothing to translate, so a gate here
//! could only refuse work that is already version-agnostic. Its motivating caller was
//! `er-ersc-sigshim`, patching a foreign module on builds this one calls unsupported; that crate
//! was retired on 2026-09-03 when the mod dropped support for old Seamless builds. The reasoning
//! outlives it: a scanned absolute address is not this module's business either way.
//!
//! `patch_3byte_stub` / `apply_xor_ret_stub` were named here too until 2026-08-30 and should not
//! have been: they take a 1.16.2 `rva`, not a scanned address, so they have something to translate
//! and now go through [`resolve_game_address`] like everything else.

// Reading the running image's own PE headers is a Windows-only operation, and these externs are
// undefined at link time on the host -- which host-unit-tested crates hit through `describe_build`.
#[cfg(windows)]
use crate::mem::{game_module_base, read_bytes};

/// `FileVersion` of the build every RVA in this workspace was reverse-engineered against:
/// ELDEN RING 1.16.2, whose PE `FileVersion` is 2.6.2.0.
pub const SUPPORTED_FILE_VERSION: FileVersion = FileVersion {
    major: 2,
    minor: 6,
    build: 2,
    revision: 0,
};

/// `FileVersion` of the build the 1.17 half of the address map was measured against: 1.17.0.
pub const MAPPED_FILE_VERSION: FileVersion = FileVersion {
    major: 2,
    minor: 7,
    build: 0,
    revision: 0,
};

/// `FileVersion` of 1.17.1, which shipped 2026-09-08.
pub const MAPPED_FILE_VERSION_1171: FileVersion = FileVersion {
    major: 2,
    minor: 7,
    build: 1,
    revision: 0,
};

/// End of the primary `.text`, as an rva. Both 1.17 builds put the section table in the same
/// place, so this bound is the same on either of them.
const TEXT_RVA_END: u32 = 0x29a5800;

/// The lowest rva that 1.17.1 moved. Below it the two 1.17 builds hold the same code at the same
/// address; at or above it a function entry sits [`CARRY_1171_SHIFT`] bytes higher.
const CARRY_1171_BOUNDARY_RVA: u32 = 0xafefe9;

/// How far 1.17.1 pushed the code above [`CARRY_1171_BOUNDARY_RVA`].
///
/// Read only by `carry_shift_for_running_build`, which is windows-gated, and by the tests. A host
/// build with no `cfg(test)` therefore sees no use of it at all, and this crate denies dead code.
#[cfg_attr(not(windows), allow(dead_code))]
const CARRY_1171_SHIFT: u32 = 0x70;

/// A PE `VS_FIXEDFILEINFO` file version, in display order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FileVersion {
    pub major: u16,
    pub minor: u16,
    pub build: u16,
    pub revision: u16,
}

impl core::fmt::Display for FileVersion {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{}.{}.{}.{}",
            self.major, self.minor, self.build, self.revision
        )
    }
}

/// Signature at the head of `VS_FIXEDFILEINFO`.
#[cfg(windows)]
const VS_FIXEDFILEINFO_SIGNATURE: u32 = 0xFEEF_04BD;

/// `IMAGE_DIRECTORY_ENTRY_RESOURCE`.
#[cfg(windows)]
const RESOURCE_DIRECTORY_INDEX: usize = 2;

/// `RT_VERSION`.
#[cfg(windows)]
const RT_VERSION: u32 = 16;

/// Bytes of `.rsrc` searched for the fixed-file-info signature once the resource directory has
/// been located. The version resource sits at the front of it; this bound keeps a corrupt or
/// unexpected directory from turning into a long scan.
#[cfg(windows)]
const VERSION_SEARCH_LIMIT: usize = 0x4000;
/// High bit of a resource directory entry's `OffsetToData`: set means the entry points at another
/// directory rather than at a data leaf.
#[cfg(windows)]
const RESOURCE_SUBDIRECTORY_FLAG: u32 = 0x8000_0000;

/// High bit of a resource directory entry's `Name`: set means the entry is keyed by a string,
/// not by an integer id. A different field from the one above, and conflating the two is why this
/// is spelled out.
#[cfg(windows)]
const RESOURCE_NAME_IS_STRING_FLAG: u32 = 0x8000_0000;

/// The running game image's `FileVersion`, or `None` when the headers or the version resource
/// cannot be read (which is itself a reason to treat the build as unrecognised).
///
/// A host build has no running game image, so it answers `None` -- the same answer, for the same
/// reason, as a Windows build whose headers are unreadable. It is a separate function body rather
/// than an early return because the Win32 externs below are undefined at link time on the host,
/// and crates that are host-unit-tested (`er-save-loader`) reach this through `describe_build`.
#[cfg(not(windows))]
pub fn game_file_version() -> Option<FileVersion> {
    None
}

/// The running game image's `FileVersion`; see the host counterpart above.
#[cfg(windows)]
pub fn game_file_version() -> Option<FileVersion> {
    let base = game_module_base().ok()?;
    let (rsrc_rva, rsrc_size) = resource_directory(base)?;
    let version_rva = version_resource_data(base, rsrc_rva)?;
    // Search from the version resource's own data, bounded by the section it lives in.
    let span = rsrc_size
        .saturating_sub(version_rva.saturating_sub(rsrc_rva))
        .min(VERSION_SEARCH_LIMIT);
    let mut buf = vec![0u8; span];
    if !unsafe { read_bytes(base + version_rva, &mut buf) } {
        return None;
    }
    let signature = VS_FIXEDFILEINFO_SIGNATURE.to_le_bytes();
    let at = buf.windows(signature.len()).position(|w| w == signature)?;
    // VS_FIXEDFILEINFO: dwSignature, dwStrucVersion, dwFileVersionMS, dwFileVersionLS.
    let ms = u32::from_le_bytes(buf.get(at + 8..at + 12)?.try_into().ok()?);
    let ls = u32::from_le_bytes(buf.get(at + 12..at + 16)?.try_into().ok()?);
    Some(FileVersion {
        major: (ms >> 16) as u16,
        minor: (ms & 0xFFFF) as u16,
        build: (ls >> 16) as u16,
        revision: (ls & 0xFFFF) as u16,
    })
}

/// `(rva, size)` of the resource data directory.
#[cfg(windows)]
fn resource_directory(base: usize) -> Option<(usize, usize)> {
    let mut word = [0u8; 4];
    if !unsafe { read_bytes(base + 0x3C, &mut word) } {
        return None;
    }
    let pe = base + u32::from_le_bytes(word) as usize;
    let mut signature = [0u8; 4];
    if !unsafe { read_bytes(pe, &mut signature) } || &signature != b"PE\0\0" {
        return None;
    }
    let mut magic = [0u8; 2];
    if !unsafe { read_bytes(pe + 0x18, &mut magic) } {
        return None;
    }
    // Data directories start at +0x70 in PE32+ and +0x60 in PE32; ELDEN RING is PE32+, but the
    // offset is derived rather than assumed so a wrong guess cannot silently read garbage.
    let directories = if u16::from_le_bytes(magic) == 0x20B {
        pe + 0x18 + 0x70
    } else {
        pe + 0x18 + 0x60
    };
    let mut entry = [0u8; 8];
    if !unsafe { read_bytes(directories + RESOURCE_DIRECTORY_INDEX * 8, &mut entry) } {
        return None;
    }
    let rva = u32::from_le_bytes(entry[0..4].try_into().ok()?) as usize;
    let size = u32::from_le_bytes(entry[4..8].try_into().ok()?) as usize;
    if rva == 0 || size == 0 {
        return None;
    }
    Some((rva, size))
}

/// RVA of the first data block under the `RT_VERSION` type, walking type -> name -> language.
#[cfg(windows)]
fn version_resource_data(base: usize, rsrc_rva: usize) -> Option<usize> {
    let type_dir = resource_entry(base, rsrc_rva, rsrc_rva, Some(RT_VERSION))?;
    let name_dir = resource_entry(base, rsrc_rva, type_dir, None)?;
    let language_entry = resource_entry(base, rsrc_rva, name_dir, None)?;
    // A leaf entry points at IMAGE_RESOURCE_DATA_ENTRY: OffsetToData, Size, CodePage, Reserved.
    let mut leaf = [0u8; 4];
    if !unsafe { read_bytes(base + language_entry, &mut leaf) } {
        return None;
    }
    Some(u32::from_le_bytes(leaf) as usize)
}

/// One step of the resource tree walk. Returns the RVA the matching entry points at -- a
/// subdirectory RVA for the first two levels, a data-entry RVA for the last.
///
/// `want_id` selects an entry by id; `None` takes the first, which is what the name and language
/// levels want (the version resource has exactly one of each).
#[cfg(windows)]
fn resource_entry(
    base: usize,
    rsrc_rva: usize,
    directory_rva: usize,
    want_id: Option<u32>,
) -> Option<usize> {
    // IMAGE_RESOURCE_DIRECTORY: ...+0x0c NumberOfNamedEntries, +0x0e NumberOfIdEntries.
    let mut header = [0u8; 16];
    if !unsafe { read_bytes(base + directory_rva, &mut header) } {
        return None;
    }
    let named = u16::from_le_bytes(header[12..14].try_into().ok()?) as usize;
    let ids = u16::from_le_bytes(header[14..16].try_into().ok()?) as usize;
    for index in 0..named + ids {
        let mut entry = [0u8; 8];
        if !unsafe { read_bytes(base + directory_rva + 16 + index * 8, &mut entry) } {
            return None;
        }
        let name = u32::from_le_bytes(entry[0..4].try_into().ok()?);
        let offset = u32::from_le_bytes(entry[4..8].try_into().ok()?);
        if let Some(want) = want_id
            && (name & RESOURCE_NAME_IS_STRING_FLAG != 0 || name != want)
        {
            continue;
        }
        return Some(rsrc_rva + (offset & !RESOURCE_SUBDIRECTORY_FLAG) as usize);
    }
    None
}

/// `[base, base + SizeOfImage)` of the running game module, or `None` if the headers are
/// unreadable.
///
/// This is what separates "an address from a previous patch" from "an address that has nothing to
/// do with the game build". A detour on `user32!CreateWindowExW` or `kernel32!ExitProcess` is
/// resolved through `GetProcAddress` at runtime and is equally correct on every ELDEN RING
/// version; only addresses inside this range carry a version assumption.
#[cfg(windows)]
pub fn game_image_range() -> Option<(usize, usize)> {
    let base = game_module_base().ok()?;
    let mut word = [0u8; 4];
    if !unsafe { read_bytes(base + 0x3C, &mut word) } {
        return None;
    }
    let pe = base + u32::from_le_bytes(word) as usize;
    let mut signature = [0u8; 4];
    if !unsafe { read_bytes(pe, &mut signature) } || &signature != b"PE\0\0" {
        return None;
    }
    // Optional header +0x38 is SizeOfImage in both PE32 and PE32+.
    let mut size = [0u8; 4];
    if !unsafe { read_bytes(pe + 0x18 + 0x38, &mut size) } {
        return None;
    }
    let size = u32::from_le_bytes(size) as usize;
    if size == 0 {
        return None;
    }
    Some((base, base + size))
}

/// Whether `address` lies inside the running game image, and therefore whether it is an address
/// whose meaning depends on which patch is installed.
///
/// A `None` image range answers `true`: if the headers cannot be read, treating an address as
/// version-sensitive is the side that refuses rather than the side that detours blind.
#[cfg(windows)]
pub fn is_game_image_address(address: usize) -> bool {
    match game_image_range() {
        Some((start, end)) => (start..end).contains(&address),
        None => true,
    }
}

/// Whether the running image is the build this workspace's RVAs were reverse-engineered against.
///
/// An unreadable version resource counts as unsupported: "cannot tell" and "wrong build" have
/// the same consequence for an address taken from a different patch.
pub fn is_supported_build() -> bool {
    game_file_version() == Some(SUPPORTED_FILE_VERSION)
}

/// One line naming the running build and the supported one, for a refusal log.
pub fn describe_build() -> String {
    match game_file_version() {
        Some(found) => {
            format!("game FileVersion {found} (this build supports {SUPPORTED_FILE_VERSION})")
        }
        None => {
            format!("game FileVersion UNREADABLE (this build supports {SUPPORTED_FILE_VERSION})")
        }
    }
}

// ============================================================================
// address resolution across builds.
//
// `is_supported_build` above answers "is this the build our RVAs were written against". This
// section answers the question that follows from a NO: where, if anywhere, did that address go.
//
// It lives here rather than in `er-hook` because the danger is not specific to detours. A detour
// on a stale address is caught by the hook path, but a stale address is equally reachable as a
// call (`transmute(base + RVA)`) and as a data pointer, and neither goes anywhere near MinHook.
// A call through a stale address is in fact the worse of the two: it transfers control into
// whatever now occupies those bytes, which on a patched build is routinely the middle of an
// unrelated function -- an execute-fault with no unwind information and no exception record
// naming anything of ours.
// ============================================================================

include!(concat!(env!("OUT_DIR"), "/address_map_1170.rs"));

/// Signature of the sink refusal/translation lines are written to.
pub type AddressLogFn = fn(core::fmt::Arguments<'_>);
static ADDRESS_LOGGER: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

// Imported rather than spelled out at each use because the announcement bitsets below are array
// types whose element type and length both appear twice, and the fully-qualified form pushes them
// past what rustfmt can lay out readably.
use core::sync::atomic::{AtomicU64, Ordering};

// ============================================================================
// one translation line per address, not per resolution.
//
// `ADDRESS TRANSLATED` states a fact about an address -- this 1.16.2 constant was carried to 1.17
// and the pair was verified as the same function. That fact does not change between two calls, so
// emitting it per call says nothing new and costs a formatted file append every time.
//
// Measured on the 2026-08-30 session's `er-quickload-autoload-debug.log`: 1.955 GB from a single
// run, of which a 40 MB sample taken at five points across the file was **99.03%** this one
// message (145,006 of 146,434 lines). The largest single contributor is
// `GX_RESERVE_CMD_QUEUE_SLOT_RVA (cmd-queue producer attribution band)` at ~18,500 lines per 8 MB
// in the tail -- `resolve_call_site_band` re-resolving the same anchor from the per-frame
// command-queue producer path. Extrapolated over the file that is ~6.8 million lines, ~4.5 million
// of them that one address.
//
// The second cost is the one that actually blocked work: the log became unreadable. A scan of the
// last 8 MB for `ADDRESS REFUSED`, `HOOK REFUSED`, `catalog: N named`, `GRANTED:`, `EQUIP LEDGER`,
// `loading-bar:`, `backstop` and `icon_id=` returned zero hits on all eight, because the tail is
// effectively 100% this message. The acceptance evidence for several landed fixes was in there
// somewhere and could not be found.
//
// So the line is emitted the first time each row translates, and suppressed after. REFUSALS are
// bounded separately and much more loosely -- see the refusal ledger below. They are a different
// kind of fact and must never be reduced to one line per address, because a refusal is attributed
// to its caller (`mem::game_rva` puts the asking `file:line` in the label) and two callers
// refusing the same address are two different dead features.
// ============================================================================

/// Rows of an address table covered by one word of its announcement bitset.
const ANNOUNCEMENT_ROWS_PER_WORD: usize = 64;

/// Words needed to give every row of a `rows`-row table its own announcement bit.
const fn announcement_words(rows: usize) -> usize {
    rows.div_ceil(ANNOUNCEMENT_ROWS_PER_WORD)
}

/// One bit per row of [`VERIFIED_1162_TO_1170`]: set once that row's translation has been logged.
static CALL_TRANSLATION_ANNOUNCED: [AtomicU64; announcement_words(VERIFIED_1162_TO_1170.len())] =
    [const { AtomicU64::new(0) }; announcement_words(VERIFIED_1162_TO_1170.len())];

/// One bit per row of [`DETOUR_SAFE_1162_TO_1170`]; see [`CALL_TRANSLATION_ANNOUNCED`].
static DETOUR_TRANSLATION_ANNOUNCED: [AtomicU64;
    announcement_words(DETOUR_SAFE_1162_TO_1170.len())] =
    [const { AtomicU64::new(0) }; announcement_words(DETOUR_SAFE_1162_TO_1170.len())];

/// CALL/READ translations performed, whether or not they were logged.
static CALL_TRANSLATIONS: AtomicU64 = AtomicU64::new(0);

/// Detour translations performed, whether or not they were logged.
static DETOUR_TRANSLATIONS: AtomicU64 = AtomicU64::new(0);

/// Claim the right to announce `row`'s translation: `true` for exactly one caller, ever.
///
/// # Why a bitset and not a set
///
/// The check now runs where the log line used to, which for
/// `GX_RESERVE_CMD_QUEUE_SLOT_RVA` is a per-frame command-queue producer path -- so the check
/// itself must not become the new cost. A `Mutex<HashSet>` would take a lock and hash a key on
/// every resolution from every thread that resolves anything, and would allocate on first insert.
/// The row index is already in hand from the table lookup, and the tables are fixed-size `const`
/// arrays, so each whole set is one `u64` per 64 rows of static storage -- under ten words apiece
/// at the tables' current sizes -- with no allocation, no hashing and no lock.
///
/// The steady state -- every call after the first for a given row -- is a single relaxed load and
/// a bit test, with no bus-locked read-modify-write at all. The `fetch_or` runs at most once per
/// row per process. `Relaxed` is the right ordering because no data is published through this
/// flag: it orders nothing, it only has to be atomic, and `fetch_or` returning the previous value
/// is what makes exactly one racing thread see the bit as clear.
///
/// Host builds have no running game to resolve an address for, so only the tests below reach it
/// there -- and it is testable on the host precisely because it is a pure function of a bitset.
#[cfg_attr(not(windows), allow(dead_code))]
fn announce_translation_once(announced: &[AtomicU64], row: usize) -> bool {
    let mask = 1u64 << (row % ANNOUNCEMENT_ROWS_PER_WORD);
    // A row index out of range cannot occur -- it came from a lookup in the very table this bitset
    // is sized from -- so the fallback only decides what an impossible state does, and logging is
    // the side that reports rather than the side that hides.
    let Some(word) = announced.get(row / ANNOUNCEMENT_ROWS_PER_WORD) else {
        return true;
    };
    if word.load(Ordering::Relaxed) & mask != 0 {
        return false;
    }
    word.fetch_or(mask, Ordering::Relaxed) & mask == 0
}

// ============================================================================
// a refusal is a failure signal. Bound its repetition; Never silence it.
//
// The governing asymmetry of this whole migration is that a missing address must cost a feature
// loudly while a wrong one corrupts silently. So the translation gate above -- one line per
// address, ever -- is exactly the wrong shape for a refusal, and the ledger here is deliberately
// far looser than it.
//
// Why any bound at all. The refusal path emits a formatted file append per call at whatever rate
// its caller runs. Measured twice:
//
//   * 339,764 lines of `ADDRESS REFUSED (game_rva): 0x140000000` in one 25-hour session, from
//     `delay_delete_pending` resolving RVA 0 on the 4 Hz telemetry write purely to obtain the
//     module base. Root-caused and gated by `scripts/check-no-rva-zero.py`.
//   * 628 lines of `ADDRESS REFUSED (CS_MSB_POINT_CTOR_RVA): 0x140cf9300` in the 2026-08-30 21:16
//     session's `er-invasion-warp.log`. That one is not a bug: `docs/recon/rva-map-1162-to-1170
//     .verified.tsv` records the row as deliberately absent (its 1.17 pair is correct by 16 caller
//     votes but verifies DIVERGES 0.09 on an Arxan entry-jmp, and writing the row would drop the
//     constructor from the call map too). The address is genuinely unmapped on 1.17, the feature
//     is genuinely unavailable, and the map-point reader asked again on every map open.
//
// The second one is the point: fixing individual callers does not close this, because the next
// unmapped address does it again. The bound belongs here, once.
//
// What is bounded, and what is not.
//
//   * The first [`REFUSALS_LOGGED_PER_ADDRESS`] refusals of an address are logged in full,
//     unconditionally. The cap is 12 rather than 1 precisely because of the caller attribution
//     above: several distinct callers refusing the same address all get their own line.
//   * Refusal `REFUSALS_LOGGED_PER_ADDRESS + 1` logs a went-quiet marker, so a reader can tell
//     "it stopped happening" from "it stopped being written down".
//   * After that the address logs again at each power of ten -- 100, 1,000, 10,000, ... -- each
//     line stating the running count. Growth is therefore logarithmic in the number of refusals
//     while the true magnitude still reaches the log. This is the part that matters: a bound that
//     merely truncated would leave a reader of 13 lines unable to tell 13 refusals from 4.5
//     million, and the magnitude is what says how hot the dead path is.
//
// Every one of those lines keeps the `ADDRESS REFUSED (<label>): 0x<addr>` prefix, so
// `scripts/record-1170-refusals.py` -- which harvests the distinct addresses a real run asked for
// and was refused -- sees exactly what it saw before. It de-duplicates into a set, so fewer
// repeats of an address it already has cannot change its output.
// ============================================================================

/// Refusals of one address logged in full before it goes quiet.
///
/// 12 matches `er_hook::detour_site::MAX_REFUSAL_LINES`, the existing in-repo precedent for this
/// shape. It is deliberately not 1: a refusal names its caller, and one address is refused by
/// several callers whose labels differ, so a cap of 1 would report one dead feature and hide the
/// rest. `what` is `core::fmt::Arguments` and cannot be keyed on without formatting it into a
/// `String` on the failure path, so a loose per-address cap is how caller diversity survives.
const REFUSALS_LOGGED_PER_ADDRESS: u64 = 12;

/// Slots in each refusal ledger.
///
/// Sized by measurement, not by taste. The worst case actually observed is the 2026-08-29 boot:
/// 54 distinct addresses refused on the call path and 72 more behind the `FOR DETOUR` wording,
/// 126 together -- and each path has its own ledger, so 126 in one is already double the real
/// load. Replaying the verified map's own source addresses (real RVAs, 896 of 950 of them 16-byte
/// aligned, so their clustering is the clustering that matters) through this exact placement:
///
/// | addresses | 256 slots / 8 probes | 256 / 16 | 512 / 8 | 512 / 16 |
/// |-----------|----------------------|----------|---------|----------|
/// | 126       | 1 spilled            | 0        | 0       | 0        |
/// | 200       | 6                    | 2        | 0       | 0        |
/// | 256       | 26                   | 20       | 1       | 0        |
///
/// The first draft was 256/8 and spilled on the very load it was sized for. 512 slots and 16
/// probes hold twice the measured worst case with nothing spilled, for 4 KiB of statics per
/// ledger. An address that still finds no slot is not silenced; see [`note_refusal`].
const REFUSAL_SLOTS: usize = 512;

/// Probe length for the open-addressed refusal ledger. See [`REFUSAL_SLOTS`] for the measurement.
#[cfg_attr(not(windows), allow(dead_code))]
const REFUSAL_PROBES: usize = 16;

/// Occupied-slot encoding: the low 32 bits hold `rva + 1`, so a zero word means free.
#[cfg_attr(not(windows), allow(dead_code))]
const REFUSAL_KEY_MASK: u64 = 0xffff_ffff;

/// One increment of the count packed into the high 32 bits of a slot.
#[cfg_attr(not(windows), allow(dead_code))]
const REFUSAL_COUNT_STEP: u64 = 1u64 << 32;

/// Stop incrementing here, leaving slack for threads already inside `fetch_add`, so a count can
/// never carry into the key bits and rename an address. 4.29 billion refusals of one address is
/// 34 years at 4 Hz; the guard is one compare on the failure path and it costs nothing to be
/// exact rather than nearly exact.
#[cfg_attr(not(windows), allow(dead_code))]
const REFUSAL_COUNT_CEILING: u64 = (u32::MAX as u64) - 64;

/// The powers of ten a bounded address logs at after going quiet.
///
/// It stops at 1e9 because a per-address count saturates at [`REFUSAL_COUNT_CEILING`] (~4.29e9),
/// so 1e10 is unreachable for a slotted address. The shared overflow counter is a full `u64` and
/// can pass 1e9 without another line; that is the same resolution loss overflow already carries,
/// and it needs more than [`REFUSAL_SLOTS`] distinct refused addresses to be reached at all.
const REFUSAL_MILESTONES: [u64; 8] = [
    100,
    1_000,
    10_000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
    1_000_000_000,
];

/// Refusals that found no ledger slot, counted together. See [`note_refusal`].
static CALL_REFUSAL_OVERFLOW: AtomicU64 = AtomicU64::new(0);

/// See [`CALL_REFUSAL_OVERFLOW`].
static DETOUR_REFUSAL_OVERFLOW: AtomicU64 = AtomicU64::new(0);

/// Per-address refusal counts for the CALL/READ path.
static CALL_REFUSALS: [AtomicU64; REFUSAL_SLOTS] = [const { AtomicU64::new(0) }; REFUSAL_SLOTS];

/// Per-address refusal counts for the detour path. A separate ledger from the call one, for the
/// same reason the announcement bitsets are separate: the two refusals make different claims
/// about the same address and answer to different tables, so neither may quieten the other.
static DETOUR_REFUSALS: [AtomicU64; REFUSAL_SLOTS] = [const { AtomicU64::new(0) }; REFUSAL_SLOTS];

/// Which line, if any, refusal number `occurrence` of one address should emit.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RefusalLine {
    /// The full refusal, with the caller label and the reason.
    Full,
    /// The last line before this address goes quiet.
    WentQuiet,
    /// A power-of-ten restatement carrying the running count.
    Milestone,
    /// Counted, not written.
    Suppressed,
}

/// The bound, as a pure function of the occurrence number so it can be tested for its values
/// rather than for the shape of the code that calls it.
#[cfg_attr(not(windows), allow(dead_code))]
fn refusal_line_for(occurrence: u64) -> RefusalLine {
    if occurrence <= REFUSALS_LOGGED_PER_ADDRESS {
        return RefusalLine::Full;
    }
    if occurrence == REFUSALS_LOGGED_PER_ADDRESS + 1 {
        return RefusalLine::WentQuiet;
    }
    if REFUSAL_MILESTONES.contains(&occurrence) {
        return RefusalLine::Milestone;
    }
    RefusalLine::Suppressed
}

/// Slot an RVA hashes to. Multiply-shift on the golden ratio, because RVAs are dense, aligned and
/// clustered -- the low bits are nearly constant across a table of them, so `rva % SLOTS` would
/// pile the whole set onto a handful of slots and overflow a ledger with room to spare.
#[cfg_attr(not(windows), allow(dead_code))]
fn refusal_slot(rva: u32, slots: usize) -> usize {
    const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;
    ((rva as u64).wrapping_mul(GOLDEN) >> 32) as usize % slots
}

/// Record one refusal of `rva` and return its 1-based occurrence number for that address.
///
/// # Cost
///
/// Steady state, for an address already in the ledger: a multiply-shift, one relaxed load, a
/// compare, one relaxed `fetch_add`, and the compare in [`refusal_line_for`]. No lock, no
/// allocation, no string hashing, and nothing at all on the success path -- this is only reached
/// where the code was already about to format a message and append it to a file.
///
/// `Relaxed` throughout is right for the same reason it is right in `announce_translation_once`:
/// no data is published through these words. They order nothing; they only have to be atomic, and
/// the compare-exchange is what makes exactly one racing thread claim a free slot.
///
/// # When the ledger is full
///
/// An address that finds no free slot within [`REFUSAL_PROBES`] falls to a shared overflow
/// counter, which is itself bounded by the same rule. That loses per-address resolution for those
/// addresses -- but reaching it means more than [`REFUSAL_SLOTS`] distinct addresses were refused,
/// at which point the log has already said the map is wrong wholesale, and the alternative
/// (logging every overflow refusal in full) is the unbounded behaviour this exists to remove.
#[cfg_attr(not(windows), allow(dead_code))]
fn note_refusal(slots: &[AtomicU64], overflow: &AtomicU64, rva: u32) -> u64 {
    let key = rva as u64 + 1;
    let start = refusal_slot(rva, slots.len());
    for probe in 0..REFUSAL_PROBES {
        let slot = &slots[(start + probe) % slots.len()];
        let mut current = slot.load(Ordering::Relaxed);
        loop {
            if current == 0 {
                match slot.compare_exchange_weak(
                    0,
                    REFUSAL_COUNT_STEP | key,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return 1,
                    // Lost the claim -- to this same address or to another one. Re-examine the
                    // word rather than probing on, or two threads racing on a fresh slot would
                    // give the same address two slots and two independent counts.
                    Err(actual) => current = actual,
                }
                continue;
            }
            if current & REFUSAL_KEY_MASK != key {
                break;
            }
            let count = current >> 32;
            if count >= REFUSAL_COUNT_CEILING {
                return count;
            }
            return (slot.fetch_add(REFUSAL_COUNT_STEP, Ordering::Relaxed) >> 32) + 1;
        }
    }
    overflow.fetch_add(1, Ordering::Relaxed) + 1
}

/// Total refusals recorded in one ledger, and how many distinct addresses hold a slot.
fn refusal_totals(slots: &[AtomicU64], overflow: &AtomicU64) -> (u64, u32) {
    let mut total = overflow.load(Ordering::Relaxed);
    let mut addresses = 0u32;
    for slot in slots {
        let word = slot.load(Ordering::Relaxed);
        if word != 0 {
            addresses += 1;
            total += word >> 32;
        }
    }
    (total, addresses)
}

/// How much address translation has happened, and how much of it the log shows.
///
/// The `*_addresses` counts are what a reader of the log sees -- one `ADDRESS TRANSLATED` line
/// each. The `*_resolutions` counts are what actually happened, and they are the reason this
/// struct exists: suppressing the repeats would otherwise destroy the only evidence that one
/// address was resolved four and a half million times, which is the fact that made the log
/// unreadable in the first place and the fact a reader most needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AddressTranslationStats {
    /// Distinct CALL/READ addresses announced.
    pub call_addresses: u32,
    /// CALL/READ translations performed.
    pub call_resolutions: u64,
    /// Distinct detour addresses announced.
    pub detour_addresses: u32,
    /// Detour translations performed.
    pub detour_resolutions: u64,
    /// Distinct CALL/READ addresses refused that hold a ledger slot.
    pub call_refused_addresses: u32,
    /// CALL/READ refusals, including the ones the bound did not write down.
    pub call_refusals: u64,
    /// Distinct detour addresses refused that hold a ledger slot.
    pub detour_refused_addresses: u32,
    /// Detour refusals, including the ones the bound did not write down.
    pub detour_refusals: u64,
}

/// Read [`AddressTranslationStats`] for this process.
///
/// This crate deliberately does not emit the summary itself. It is a zero-dependency leaf with no
/// thread, no clock and no shutdown hook; a periodic emitter would need a time read back on the
/// hot path, and `DLL_PROCESS_DETACH` is not reached when the game is killed, which is how these
/// sessions actually end. So the numbers are exposed here and printed by whichever crate already
/// owns a periodic or teardown telemetry line.
pub fn address_translation_stats() -> AddressTranslationStats {
    fn announced(words: &[AtomicU64]) -> u32 {
        words
            .iter()
            .map(|word| word.load(Ordering::Relaxed).count_ones())
            .sum()
    }
    let (call_refusals, call_refused_addresses) =
        refusal_totals(&CALL_REFUSALS, &CALL_REFUSAL_OVERFLOW);
    let (detour_refusals, detour_refused_addresses) =
        refusal_totals(&DETOUR_REFUSALS, &DETOUR_REFUSAL_OVERFLOW);
    AddressTranslationStats {
        call_addresses: announced(&CALL_TRANSLATION_ANNOUNCED),
        call_resolutions: CALL_TRANSLATIONS.load(Ordering::Relaxed),
        detour_addresses: announced(&DETOUR_TRANSLATION_ANNOUNCED),
        detour_resolutions: DETOUR_TRANSLATIONS.load(Ordering::Relaxed),
        call_refused_addresses,
        call_refusals,
        detour_refused_addresses,
        detour_refusals,
    }
}

/// Install the sink for address-resolution lines. Call once, early, before any resolution.
///
/// Default is a no-op: this crate is a zero-dependency leaf and has no log of its own, so the
/// product DLL points this at the same file its hook refusals already go to.
pub fn set_address_logger(logger: AddressLogFn) {
    ADDRESS_LOGGER.store(logger as usize, core::sync::atomic::Ordering::Release);
}

/// Windows-only: the resolver is the sole caller, and on a host build there is no running game
/// to refuse an address for, so the sink would never be reached.
#[cfg(windows)]
fn address_log(args: core::fmt::Arguments<'_>) {
    let raw = ADDRESS_LOGGER.load(core::sync::atomic::Ordering::Acquire);
    if raw != 0 {
        // SAFETY: `raw` is only ever an `AddressLogFn` stored by `set_address_logger`.
        let logger: AddressLogFn = unsafe { core::mem::transmute::<usize, AddressLogFn>(raw) };
        logger(args);
    }
}

/// Where `address` lives on the running build, or `None` if that is not known.
///
/// * `Some(address)` unchanged -- the address is outside the game image (an import resolved by
///   `GetProcAddress` means the same thing on every patch), or the running build is the one the
///   address was written for.
/// * `Some(translated)` -- the build moved, and this address has a mapping that two independent
///   passes agreed on: `scripts/map-rvas-1162-to-1170.py` found where the function's masked
///   signature re-occurs, and `scripts/verify-rva-map-1170.py` then confirmed the normalised
///   instruction sequences match over the body. `scripts/audit-1170-hook-targets.py` separately
///   confirms the destination is a real function entry, by the references the image itself makes
///   to it.
/// * `None` -- the build moved and nothing here knows where to. The caller must not proceed.
///
/// `what` names the caller in the log line, because a refusal is only actionable if a reader can
/// tell which feature just went inert.
pub fn resolve_game_address(address: usize, what: &str) -> Option<usize> {
    resolve_game_address_fmt(address, format_args!("{what}"))
}

/// [`resolve_game_address`], with the label built by the caller's own `format_args!`.
///
/// The label is the only thing a reader of a refusal line has to go on, and the useful ones are
/// composite -- `er_game_base::mem::game_rva` wants to print the constant's name and the source
/// line that asked for it. Taking `Arguments` rather than `&str` lets it do that without a
/// `format!` allocation on every resolution, including the overwhelming majority that succeed.
pub fn resolve_game_address_fmt(address: usize, what: core::fmt::Arguments<'_>) -> Option<usize> {
    #[cfg(windows)]
    {
        resolve_on_running_build(address, what)
    }
    // Host builds have no running game to resolve against, and reaching the Win32 externs from a
    // host unit test is a link error rather than a wrong answer. `er-save-loader` runs its tests
    // on the host against a fake module base, so this passthrough is what keeps the resolver
    // callable from code that is also host-tested.
    #[cfg(not(windows))]
    {
        let _ = what;
        Some(address)
    }
}

#[cfg(windows)]
fn resolve_on_running_build(address: usize, what: core::fmt::Arguments<'_>) -> Option<usize> {
    if !is_game_image_address(address) || is_supported_build() {
        return Some(address);
    }
    let base = crate::mem::game_module_base().ok()?;
    let rva = (address - base) as u32;
    let shift = carry_shift_or_refuse(what)?;
    // Already translated. Resolution is not naturally idempotent: the table is keyed by 1.16.2 RVA
    // and its values are 1.17 RVAs, so asking it where a 1.17 address moved to finds no entry and
    // the honest answer is to refuse -- which is how a correctly translated address got refused on
    // its second pass through, costing `er-armament-icons` its file-open observer at 0x1411ced80.
    // The shortcut makes that second pass hand the address back instead of refusing it.
    //
    // It does not make a double resolve safe, and nothing in this function can. The shortcut
    // declines on exactly the addresses where a double resolve goes WRONG: an address that is both
    // a 1.17 destination and the 1.16.2 source of a different row is a source, so translation wins
    // (it must -- see `already_translated_in`), and resolving it a second time returns a third,
    // unrelated function with no error, no refusal and no log line. That is not hypothetical: it
    // is what happened to 0x7ac890 -> 0x7ad710 -> 0x7ae590 on 2026-08-30.
    //
    // So the invariant this depends on is "resolve exactly once", which spans six crates and a
    // `GetProcAddress` boundary. It is a convention -- there is no type here that distinguishes a
    // 1.16.2 RVA from a 1.17 one, and no test in this file establishes it. The machine check is
    // `scripts/check-1170-translation-collisions.py`, run from `scripts/check.sh`, which fails on
    // any collision not recorded in `scripts/1170-translation-collisions.baseline.tsv` (three at
    // the time of writing; an empty baseline means the tables carry none). The test below,
    // `every_verified_row_resolves_to_its_own_destination`, checks something weaker and different:
    // that the shortcut never swallows a source the table should have translated.
    match table_answer(&VERIFIED_1162_TO_1170, rva, shift) {
        TableAnswer::AlreadyTranslated => return Some(address),
        TableAnswer::MovedTo { row, to } => {
            let translated = base + to as usize;
            CALL_TRANSLATIONS.fetch_add(1, Ordering::Relaxed);
            // Once per row -- per address -- not once per call and not once per `what`. The fact
            // stated is about the address and is identical for every caller, so `what` is neither
            // needed nor usable as the key. Measured over the same 40 MB sample: 145,006
            // translated lines carried 195 distinct source addresses and 215 distinct labels; 29
            // addresses carried more than one label, and -- decisively -- 2 labels covered more
            // than one address (`game_rva @ crates/er-save-suppress/src/lib.rs:1595` resolves 9
            // different addresses from one loop). A `what` key would therefore suppress 8 of
            // those 9, which is a wrong answer rather than a quieter one. It would also cost a
            // `String` per resolution on this path, because `what` is `Arguments` and cannot be
            // hashed without being formatted first.
            if announce_translation_once(&CALL_TRANSLATION_ANNOUNCED, row) {
                address_log(format_args!(
                    "ADDRESS TRANSLATED ({what}): 0x{address:x} -> 0x{translated:x} for {} \
                     (verified same function; see docs/recon/rva-map-1162-to-1170.verified.tsv; \
                     logged once per address, repeats are counted by \
                     `address_translation_stats`)",
                    describe_build()
                ));
            }
            return Some(translated);
        }
        TableAnswer::Unmapped => {}
    }
    // Bounded, not silenced -- see the refusal ledger. The first 12 are written in full, then a
    // went-quiet marker, then a restatement at each power of ten carrying the running count. Every
    // line keeps the harvestable `ADDRESS REFUSED (<label>): 0x<addr>` prefix.
    let occurrence = note_refusal(&CALL_REFUSALS, &CALL_REFUSAL_OVERFLOW, rva);
    match refusal_line_for(occurrence) {
        RefusalLine::Full => address_log(format_args!(
            "ADDRESS REFUSED ({what}): 0x{address:x} -- {}, and this address has no verified \
             mapping for the running build, so using it would reach whatever code now occupies it",
            describe_build()
        )),
        RefusalLine::WentQuiet => address_log(format_args!(
            "ADDRESS REFUSED ({what}): 0x{address:x} -- refusal {occurrence} of this address; \
             further refusals of it are counted, not written, and restated at each power of ten. \
             The feature asking for it is dead for the whole session, so the repeats say nothing \
             new -- but they are still happening, and `address_translation_stats` has the total"
        )),
        RefusalLine::Milestone => address_log(format_args!(
            "ADDRESS REFUSED ({what}): 0x{address:x} -- refusal {occurrence} of this address. \
             Restated so the log carries the MAGNITUDE of a dead path, not just its existence"
        )),
        RefusalLine::Suppressed => {}
    }
    None
}

/// Is `rva` a 1.17 destination of this table that must not be translated again?
///
/// Translation wins over the shortcut, and the order is the whole point. The shortcut exists so a
/// second pass over an already-translated address hands it back instead of refusing -- the bug
/// that cost er-armament-icons its file-open observer. But at 329 rows an address can be both a
/// destination of one row and the source of a different one, and if the shortcut answered first
/// it would swallow that second row's translation silently.
///
/// So an address that is a source is always translated as a source. The shortcut applies only to
/// destinations that are not sources of some other row, plus the rows that did not move, where
/// both answers are the same address anyway.
///
/// Both tables need this rule and they need the same rule. The detour table used to ask a looser
/// question -- is this address any row's destination -- which at 27 rows was safe by accident:
/// nothing was both. At 216 rows two addresses are (`0x6156c0` and `0x7ad710`), and the loose
/// form would hand back a stale 1.16.2 source untranslated on the grounds that some other row
/// moved a different function onto it. That loose form is what
/// `translation_wins_over_the_shortcut_on_a_collision` exists to keep out, on a table it builds
/// itself rather than on whichever rows the ledgers happen to hold this week.
///
/// Carry an rva from the build the map's destinations were measured on to the running build.
///
/// The 1.17.0 to 1.17.1 patch grew exactly one function -- `0xafeea0`, from 329 bytes to 441 --
/// and everything downstream of it slid by the difference. Nothing else moved. The section table
/// is byte-identical between the two builds, so an `.rdata` vtable or a `.data` global keeps its
/// address, which is why the shift is bounded to the primary `.text` rather than applied to any
/// rva above the boundary. Read exhaustively out of both de-Arxan'd images' `.pdata` by
/// `scripts/map-rvas-1170-to-1171.py`, whose selftest asserts it: all 174,389 function entries
/// above the boundary map at `+0x70`, with none left over and none needing a signature match.
///
/// A `shift` of zero is the identity, which is what 1.17.0 itself takes.
#[cfg_attr(not(windows), allow(dead_code))]
const fn carry_mapped_rva(rva: u32, shift: u32) -> u32 {
    if rva >= CARRY_1171_BOUNDARY_RVA && rva < TEXT_RVA_END {
        rva + shift
    } else {
        rva
    }
}

/// How far to carry the map's destinations for the running build, or `None` when this is a build
/// nobody has measured.
///
/// `None` refuses every game address, and that is the point. Until 1.17.1 shipped there was no
/// such check: the map's 1.17.0 destinations were handed to any build that was not 2.6.2.0, so
/// the first launch on a newly patched game would have called and detoured 1.17.0 addresses
/// inside 1.17.1 code for every address that moved -- roughly half of them -- with nothing in the
/// log to say so, because from the table's point of view the translation had succeeded.
#[cfg(windows)]
fn carry_shift_for_running_build() -> Option<u32> {
    let running = game_file_version()?;
    if running == MAPPED_FILE_VERSION {
        Some(0)
    } else if running == MAPPED_FILE_VERSION_1171 {
        Some(CARRY_1171_SHIFT)
    } else {
        None
    }
}

/// Set once an unmeasured build has been reported, so that line is written once rather than once
/// per address on a per-frame path.
#[cfg(windows)]
static UNMEASURED_BUILD_ANNOUNCED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// [`carry_shift_for_running_build`], saying so in the log the first time it refuses.
#[cfg(windows)]
fn carry_shift_or_refuse(what: core::fmt::Arguments<'_>) -> Option<u32> {
    if let Some(shift) = carry_shift_for_running_build() {
        return Some(shift);
    }
    if !UNMEASURED_BUILD_ANNOUNCED.swap(true, Ordering::Relaxed) {
        // Deliberately not an `ADDRESS REFUSED` line. That shape is per-address and carries the
        // address, and `scripts/record-1170-refusals.py` harvests it to build a work list of
        // addresses to map. This refusal is not about an address -- every address is refused, and
        // no list of them would be worth reading -- so a line in that shape would add an entry
        // with nothing to map and misreport the cause as missing coverage.
        address_log(format_args!(
            "ADDRESS MAP NOT CARRIED (first asked by {what}): {} -- the map's destinations were \
             read on {MAPPED_FILE_VERSION} and are carried to {MAPPED_FILE_VERSION_1171}; this \
             build is neither, so every game address is refused rather than translated onto a \
             build nobody has read. Carrying it forward is `scripts/map-rvas-1170-to-1171.py`",
            describe_build()
        ));
    }
    None
}

/// A pure function of the table so it can be tested on the host, where there is no game to
/// resolve against. It was called through a `VERIFIED_1162_TO_1170`-shaped wrapper named
/// `already_translated` until 2026-08-30; [`table_answer`] took that over.
///
/// `shift` carries the destination column onto the running build before the comparison, and it
/// has to happen in that order: on 1.17.1 an address that has already been through this gate is a
/// carried destination, and asking whether it equals an uncarried one answers no for every row
/// that moved. A `shift` of zero is the identity, which is what 1.17.0 takes.
#[cfg_attr(not(windows), allow(dead_code))]
fn already_translated_in(table: &[(u32, u32)], rva: u32, shift: u32) -> bool {
    let is_destination = table
        .iter()
        .any(|(from, moved)| carry_mapped_rva(*moved, shift) == rva && *from != rva);
    let is_source_of_a_move = table
        .iter()
        .any(|(from, moved)| *from == rva && carry_mapped_rva(*moved, shift) != rva);
    is_destination && !is_source_of_a_move
}

/// What a table says about one RVA, in the order the resolvers ask it.
///
/// Both resolvers used to inline this decision, and the tests then re-implemented it a third
/// time -- which is how a test could go on passing while claiming to cover the order of the two
/// checks it never actually ran. One function, asked by the resolvers and by the tests.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TableAnswer {
    /// A 1.17 destination that no row claims as a source: hand it back unchanged.
    AlreadyTranslated,
    /// A 1.16.2 source; the running build has that function at `to`.
    ///
    /// `row` is where in the table the answer was found. It rides along because the lookup already
    /// knows it and the translation log needs a per-address key that costs nothing to derive --
    /// see `announce_translation_once`. Deriving it with a second scan would double the table walk
    /// on the per-frame path this whole gate exists to quieten.
    MovedTo { row: usize, to: u32 },
    /// No row names this RVA as a source. The caller must not proceed.
    Unmapped,
}

/// [`already_translated_in`] and the table lookup, in the order that makes translation win.
///
/// The order is the whole point and is asserted by
/// `translation_wins_over_the_shortcut_on_a_collision`: an address that is both a destination and
/// a source is answered as a source, because the shortcut declines on it.
/// `shift` carries the destination column onto the running build. What it never carries is the
/// source column: that is a 1.16.2 rva, and 1.16.2 is not a build this function's `shift` says
/// anything about.
#[cfg_attr(not(windows), allow(dead_code))]
fn table_answer(table: &[(u32, u32)], rva: u32, shift: u32) -> TableAnswer {
    if already_translated_in(table, rva, shift) {
        return TableAnswer::AlreadyTranslated;
    }
    match table.iter().position(|(from, _)| *from == rva) {
        Some(row) => TableAnswer::MovedTo {
            row,
            to: carry_mapped_rva(table[row].1, shift),
        },
        None => TableAnswer::Unmapped,
    }
}

/// The address to detour for `address` on the running build, or `None`.
///
/// # Why this is not [`resolve_game_address`]
///
/// Being the right address and being a safe place to write five bytes are different claims, and
/// only one of them is established by matching a function's signature. `resolve_game_address`
/// answers the first: it will happily return a pair carried by the whole-image `.pdata` map or
/// by counting code references, which is exactly what a call or a read needs. A detour needs the
/// second as well -- that the destination is a real function entry with a relocatable five-byte
/// prologue -- and nothing in a signature match speaks to that.
///
/// Measured, 2026-08-29. When the weaker rows were allowed to carry detours, er-armament-icons
/// installed five of them and the game died ~2.0s in, at the first overlay draw. Bisected over
/// eighteen DLLs: adding that one DLL to an otherwise-surviving set was the difference. Before
/// those rows existed the same five hooks were refused as unmapped and the game lived, so the
/// regression arrived with the coverage.
///
/// To promote a row into this table, `scripts/audit-1170-hook-targets.py` has to accept it.
#[cfg(windows)]
pub fn resolve_detour_address(address: usize, what: &str) -> Option<usize> {
    if !is_game_image_address(address) || is_supported_build() {
        return Some(address);
    }
    let base = crate::mem::game_module_base().ok()?;
    let rva = (address - base) as u32;
    let shift = carry_shift_or_refuse(format_args!("{what}"))?;
    // Same rule, same order, same function as the call path above -- including the part it cannot
    // establish. See the comment there.
    match table_answer(&DETOUR_SAFE_1162_TO_1170, rva, shift) {
        TableAnswer::AlreadyTranslated => return Some(address),
        TableAnswer::MovedTo { row, to } => {
            let translated = base + to as usize;
            DETOUR_TRANSLATIONS.fetch_add(1, Ordering::Relaxed);
            // A separate bitset from the call path's, indexed into a different table. The two
            // lines make different claims about the same address -- "verified same function"
            // versus "and audited as somewhere MinHook may write five bytes" -- so one must not
            // suppress the other. Sharing a key would mean whichever resolver ran first silences
            // the stronger claim, which is the one a reader of a crash log wants.
            if announce_translation_once(&DETOUR_TRANSLATION_ANNOUNCED, row) {
                address_log(format_args!(
                    "ADDRESS TRANSLATED ({what}): 0x{address:x} -> 0x{translated:x} for {} \
                     (byte-verified same function AND audited as a detour target; logged once per \
                     address, repeats are counted by `address_translation_stats`)",
                    describe_build()
                ));
            }
            return Some(translated);
        }
        TableAnswer::Unmapped => {}
    }
    // Say which refusal this is. There are three of them and they send a reader to three
    // different places, so reporting them as one wasted a day: 65 addresses were investigated as
    // missing map coverage when they were already-translated addresses arriving for a second
    // opinion, and the map that produced them was sitting right there.
    //
    // Bounded by its own ledger, separate from the call path's -- the two refusals answer to
    // different tables and neither may quieten the other. The diagnosis below costs a whole-table
    // scan and up to one `String`, so it is computed only when a line is actually emitted;
    // previously it ran on every refusal, including the 616 of the 628 that a bound now drops.
    let occurrence = note_refusal(&DETOUR_REFUSALS, &DETOUR_REFUSAL_OVERFLOW, rva);
    match refusal_line_for(occurrence) {
        RefusalLine::Full => {
            let call_only = resolve_on_running_build_quiet(rva, shift).is_some();
            let arrived_translated = VERIFIED_1162_TO_1170
                .iter()
                .find(|(from, moved)| *moved == rva && *from != rva)
                .map(|(from, _)| *from);
            address_log(format_args!(
                "ADDRESS REFUSED FOR DETOUR ({what}): 0x{address:x} -- {}, and {}",
                describe_build(),
                match (arrived_translated, call_only) {
                    // A caller resolved this through `game_rva` before asking to hook it, so the
                    // address is right and the question is only whether its row may carry a detour.
                    (Some(source), _) => format!(
                        "this is already the translation of 1.16.2 0x{:x}, whose row is not \
                         detour-safe: the pair is not verified identical over the body, or the two \
                         images disagree about where a function starts there",
                        source as usize + base
                    ),
                    (None, true) =>
                        "while this address HAS a mapping good enough to call, it has not been \
                         audited as a detour target: a signature match does not say MinHook may \
                         write five bytes there"
                            .to_string(),
                    (None, false) =>
                        "this address has no mapping at all for the running build".to_string(),
                }
            ));
        }
        RefusalLine::WentQuiet => address_log(format_args!(
            "ADDRESS REFUSED FOR DETOUR ({what}): 0x{address:x} -- refusal {occurrence} of this \
             address; further refusals of it are counted, not written, and restated at each power \
             of ten. `address_translation_stats` has the total"
        )),
        RefusalLine::Milestone => address_log(format_args!(
            "ADDRESS REFUSED FOR DETOUR ({what}): 0x{address:x} -- refusal {occurrence} of this \
             address. Restated so the log carries the MAGNITUDE of a dead path, not just its \
             existence"
        )),
        RefusalLine::Suppressed => {}
    }
    None
}

/// Host builds have no game to detour.
#[cfg(not(windows))]
pub fn resolve_detour_address(address: usize, what: &str) -> Option<usize> {
    let _ = what;
    Some(address)
}

/// The RVA of a call site on the running build, or `None` when it cannot be placed.
///
/// # The class of bug this closes
///
/// A call site is a return ADDRESS: a byte in the middle of a function, captured off the live
/// stack by `RtlCaptureStackBackTrace` and compared -- as `frame - module_base` -- against a
/// 1.16.2 constant. Nine such comparisons existed in this workspace on 2026-08-30, and every one
/// of them failed in perfect silence on 1.17: no hook is installed, no address is resolved, so
/// there is no `HOOK REFUSED` and no `ADDRESS REFUSED`. The comparison simply never matches and
/// the feature behind it never runs. Two user-visible features were dead this way -- the three
/// cloned rows on the System>Quit tab, and the title FadeIn suppression -- with nothing in any
/// log to say so.
///
/// # Why it takes a function and an offset rather than one address
///
/// The address map is keyed on `.pdata` function starts, because that is what a masked signature
/// can identify and what the linker records. A mid-function address is not a function start, so
/// it can never appear in the map -- `scripts/select-needed-1170-rows.py` cannot even see it.
///
/// What a call site does have is a stable identity: it is the return of the Nth `call` in a named
/// function, and the offset of that call within its function survives the move whenever the body
/// is unchanged. So the mappable half is the containing function, and the offset rides along.
/// `scripts/derive-callsite-1170.py` prints the evidence for a given site: the `.pdata` record
/// that contains it, the map's pair for that function, and the callee each image's `E8` reaches
/// at the same offset -- which must be the same function under the map.
///
/// # Why this is not a detour licence
///
/// It resolves through [`resolve_game_address`], which reads the CALL/READ table. A detour needs
/// `resolve_detour_address` and its separate, stricter table. That separation is the whole point:
/// putting a mid-function address in a verdict table would license it as a detour target --
/// `DETOURABLE_ENTRY_EVIDENCE` accepts `NEITHER-ENTRY` -- and MinHook would then write five bytes
/// into the middle of a live function. The offset here is added in Rust, after resolution, and
/// never enters a table at all.
///
/// `what` names the caller in the refusal line, so a reader can tell which comparison went inert.
#[cfg(windows)]
pub fn resolve_call_site_rva(
    function_rva: usize,
    offset_in_function: usize,
    what: &str,
) -> Option<usize> {
    let base = crate::mem::game_module_base().ok().filter(|&b| b != 0)?;
    let entry = resolve_game_address(base + function_rva, what)?;
    Some(entry.checked_sub(base)? + offset_in_function)
}

/// Host builds have no running game, so no stack frame can carry a game RVA to compare against.
#[cfg(not(windows))]
pub fn resolve_call_site_rva(
    function_rva: usize,
    offset_in_function: usize,
    what: &str,
) -> Option<usize> {
    let _ = (function_rva, offset_in_function, what);
    None
}

/// [`resolve_call_site_rva`] for a call-site band anchored on one function.
///
/// A band whose endpoints sit at fixed offsets from a single named function translates exactly
/// when that function does. A band spanning many functions does NOT: between 2.6.2.0 and 2.7.0.0
/// neighbouring functions moved by different deltas (+0xdf0, +0xe20, +0xe30, +0xe40, +0xe80,
/// +0xe90 and +0x1560 all occur inside `0x7a3000..0x7a4000` alone), so its width is not preserved
/// and no anchor can carry it. Such a band has to be refused, not translated -- see the caller in
/// `system_quit_dialog_handlers.rs` that does exactly that.
pub fn resolve_call_site_band(
    function_rva: usize,
    start_offset: isize,
    end_offset: isize,
    what: &str,
) -> Option<core::ops::Range<usize>> {
    let entry = resolve_call_site_rva(function_rva, 0, what)?;
    let shift = |offset: isize| {
        if offset >= 0 {
            entry.checked_add(offset as usize)
        } else {
            entry.checked_sub(offset.unsigned_abs())
        }
    };
    let start = shift(start_offset)?;
    let end = shift(end_offset)?;
    (start < end).then_some(start..end)
}

/// Does `rva` have any mapping? Used only to word a refusal accurately; logs nothing.
#[cfg(windows)]
fn resolve_on_running_build_quiet(rva: u32, shift: u32) -> Option<u32> {
    VERIFIED_1162_TO_1170
        .iter()
        .find(|(from, _)| *from == rva)
        .map(|(_, moved)| carry_mapped_rva(*moved, shift))
}

/// How many verified translations this build carries. Read by the product's startup line so a log
/// says how much of the migration is actually present, rather than leaving it to be inferred.
pub fn verified_translation_count() -> usize {
    VERIFIED_1162_TO_1170.len()
}

#[cfg(test)]
mod tests {
    use super::{
        ANNOUNCEMENT_ROWS_PER_WORD, AtomicU64, DETOUR_SAFE_1162_TO_1170, Ordering,
        REFUSAL_COUNT_CEILING, REFUSAL_KEY_MASK, REFUSAL_PROBES, REFUSAL_SLOTS,
        REFUSALS_LOGGED_PER_ADDRESS, RefusalLine, TableAnswer, VERIFIED_1162_TO_1170,
        announce_translation_once, announcement_words, note_refusal, refusal_line_for,
        refusal_slot, refusal_totals, table_answer,
    };
    use super::{
        CARRY_1171_BOUNDARY_RVA, CARRY_1171_SHIFT, TEXT_RVA_END, already_translated_in,
        carry_mapped_rva,
    };

    /// The 1.17.0 to 1.17.1 carry, at each edge of the one range it applies to.
    ///
    /// The bound that matters is the upper one. Every `.rdata` vtable and `.data` global this
    /// workspace pins sits above the end of `.text` and is numerically far above the boundary, so
    /// a carry written as "anything above 0xafefe9" rather than "anything in `.text` above
    /// 0xafefe9" would move all of them, and they did not move: the two builds have an identical
    /// section table, and `u"%s/EldenRing/%s/"` is at rva 0x2bdd9b8 in both.
    ///
    /// The edges are spelled as literals rather than as `CARRY_1171_BOUNDARY_RVA - 1` and the
    /// like, and that is not a style choice. `scripts/rva_role.py` proves that constant is a
    /// bound rather than a game address from the fact that every use of it is a comparison;
    /// passing it to a function or doing arithmetic on it here would break the proof, and
    /// `select-needed-1170-rows.py` would then require it to be carried forward in an address
    /// ledger as if a threshold were a function. The two asserts below tie the literals back to
    /// the constants without leaving comparison position.
    #[test]
    fn the_carry_moves_text_above_the_boundary_and_nothing_else() {
        let shift = CARRY_1171_SHIFT;
        const { assert!(0xafefe8 < CARRY_1171_BOUNDARY_RVA && 0xafefe9 >= CARRY_1171_BOUNDARY_RVA) };
        const { assert!(0x29a57ff < TEXT_RVA_END && 0x29a5800 >= TEXT_RVA_END) };

        assert_eq!(carry_mapped_rva(0xafefe8, shift), 0xafefe8);
        assert_eq!(carry_mapped_rva(0xafefe9, shift), 0xafefe9 + shift);
        assert_eq!(carry_mapped_rva(0x29a57ff, shift), 0x29a57ff + shift);
        // `.rdata` and `.data`, which did not move.
        assert_eq!(carry_mapped_rva(0x29a5800, shift), 0x29a5800);
        assert_eq!(carry_mapped_rva(0x2bdd9b8, shift), 0x2bdd9b8);
        assert_eq!(carry_mapped_rva(0x3d89700, shift), 0x3d89700);
        // 1.17.0 takes the identity.
        for rva in [0x1000, 0xafefe9, 0x29a57ff, 0x2bdd9b8] {
            assert_eq!(carry_mapped_rva(rva, 0), rva);
        }
    }

    /// A carried destination must still be recognised as already translated.
    ///
    /// Without this the second pass through the gate refuses an address that is already correct,
    /// which is the failure that cost `er-armament-icons` its file-open observer on 1.17.0 -- with
    /// the carry it would have come back on 1.17.1 for a second reason.
    #[test]
    fn a_carried_destination_reads_as_already_translated() {
        // One row below the boundary (does not move) and one above it (does).
        const TABLE: [(u32, u32); 2] = [(0x2000, 0x3000), (0x2001, 0xb00000)];
        let shift = CARRY_1171_SHIFT;

        assert_eq!(
            table_answer(&TABLE, 0x2001, shift),
            TableAnswer::MovedTo {
                row: 1,
                to: 0xb00000 + shift
            }
        );
        assert!(already_translated_in(&TABLE, 0xb00000 + shift, shift));
        // The uncarried destination is not the answer on this build, and claiming it is already
        // translated would hand back a 1.17.0 address as if it were right.
        assert!(!already_translated_in(&TABLE, 0xb00000, shift));
        // A row that did not move reads the same on either build.
        assert!(already_translated_in(&TABLE, 0x3000, shift));
        assert!(already_translated_in(&TABLE, 0x3000, 0));
        // Unchanged behaviour at shift 0.
        assert_eq!(
            table_answer(&TABLE, 0x2001, 0),
            TableAnswer::MovedTo {
                row: 1,
                to: 0xb00000
            }
        );
        assert_eq!(
            table_answer(&TABLE, 0x2001, 0),
            TableAnswer::MovedTo {
                row: 1,
                to: 0xb00000
            }
        );
    }

    /// Every destination the real table carries stays inside the image once carried.
    #[test]
    fn carrying_the_real_tables_keeps_every_destination_in_range() {
        for (name, table) in [
            ("VERIFIED_1162_TO_1170", &VERIFIED_1162_TO_1170[..]),
            ("DETOUR_SAFE_1162_TO_1170", &DETOUR_SAFE_1162_TO_1170[..]),
        ] {
            assert!(table.len() >= MIN_EXPECTED_ROWS, "{name} looks unpopulated");
            let moved = table
                .iter()
                .filter(|(_, to)| carry_mapped_rva(*to, CARRY_1171_SHIFT) != *to)
                .count();
            assert!(
                moved > 0,
                "{name}: no destination moves under the 1.17.1 carry, which cannot be right for a \
                 table whose addresses span the whole image"
            );
            for (from, to) in table {
                let carried = carry_mapped_rva(*to, CARRY_1171_SHIFT);
                assert!(
                    carried >= *to && carried - *to <= CARRY_1171_SHIFT,
                    "{name}: {from:#x} -> {to:#x} carried to {carried:#x}, which is not the \
                     identity nor a single {CARRY_1171_SHIFT:#x} step"
                );
            }
        }
    }

    /// VACUOUS QUANTIFICATION, and why every test below counts what it walked.
    ///
    /// Two tests here used to filter the table to rows where `from != moved` and then assert a
    /// predicate whose second conjunct is `!is_source_of_a_move(rva)` -- false, by that very
    /// filter. The filtered set and the asserted property could not co-occur, so the assertion ran
    /// over an empty set and passed unconditionally. Nothing in the output distinguished "walked
    /// 476 rows, all fine" from "walked none", and the doc on `resolve_on_running_build` cited one
    /// of them as proof that a hazard the data exhibits three times could not happen.
    ///
    /// So: no test here concludes anything from a filtered set without first asserting the set it
    /// filtered is the size it should be. This is the floor for both tables -- an order of
    /// magnitude below the ~470 and ~370 rows they carry, which is enough to catch a generator
    /// that emitted a handful of rows or none, and loose enough that deleting bad rows (which is
    /// wanted) never trips it.
    const MIN_EXPECTED_ROWS: usize = 100;

    /// A hook and a call on the same function must reach the same address.
    ///
    /// The two tables are generated from different files and nothing about their construction
    /// forces them to agree. If they ever disagree, a feature that both calls a function and
    /// detours it installs its hook at one address and invokes another -- the trampoline never
    /// fires, and the symptom is a silently inert feature rather than a crash, which is worse.
    #[test]
    fn every_detour_row_agrees_with_the_call_map() {
        assert_table_is_populated(&DETOUR_SAFE_1162_TO_1170, "DETOUR_SAFE_1162_TO_1170");
        let disagreements: Vec<(u32, u32, Option<u32>)> = DETOUR_SAFE_1162_TO_1170
            .iter()
            .map(|(from, moved)| {
                let called = VERIFIED_1162_TO_1170
                    .iter()
                    .find(|(other, _)| other == from)
                    .map(|(_, other_moved)| *other_moved);
                (*from, *moved, called)
            })
            .filter(|(_, moved, called)| *called != Some(*moved))
            .collect();
        assert!(
            disagreements.is_empty(),
            "detour rows the call map places elsewhere (from, detour, call): {disagreements:#x?}"
        );
    }

    /// One source, one answer -- on the detour table as on the call one.
    #[test]
    fn detour_map_has_one_answer_per_source() {
        assert_table_is_populated(&DETOUR_SAFE_1162_TO_1170, "DETOUR_SAFE_1162_TO_1170");
        let mut seen: Vec<u32> = DETOUR_SAFE_1162_TO_1170
            .iter()
            .map(|(from, _)| *from)
            .collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "the detour table names a source twice");
    }

    /// The rule, on a table this test builds itself. The one test here that no ledger edit can
    /// make vacuous.
    ///
    /// `0xb` below is both the destination of the first row and the source of the second -- the
    /// exact shape of `0x7ac890 -> 0x7ad710 -> 0x7ae590`, which on 2026-08-30 returned a third,
    /// unrelated function with no error and no log line. If the shortcut answered first, `0xb`
    /// would be handed back as already-translated and the second row would never run.
    ///
    /// The real-table tests below are worth having, but their power to catch a loosened
    /// `already_translated_in` depends on a collision existing in the ledgers, and the three that
    /// exist today are slated for deletion. This one does not depend on the data at all.
    #[test]
    fn translation_wins_over_the_shortcut_on_a_collision() {
        const TABLE: [(u32, u32); 5] = [
            (0xa, 0xb),
            // The collision: `0xb` is a destination above and a source here.
            (0xb, 0xc),
            // A row that did not move, and that nothing else points at.
            (0x20, 0x20),
            // A row that did not move but is another row's destination. Both answers are `0x30`,
            // so the shortcut claiming it is harmless -- the one case where it may.
            (0x30, 0x30),
            (0x31, 0x30),
        ];

        assert_eq!(
            table_answer(&TABLE, 0xa, 0),
            TableAnswer::MovedTo { row: 0, to: 0xb },
            "a plain source must translate, and from its OWN row"
        );
        assert_eq!(
            table_answer(&TABLE, 0xb, 0),
            TableAnswer::MovedTo { row: 1, to: 0xc },
            "an address that is BOTH a destination and a source must be answered as a source; \
             AlreadyTranslated here drops the second row silently, which is the whole hazard"
        );
        assert_eq!(
            table_answer(&TABLE, 0xc, 0),
            TableAnswer::AlreadyTranslated,
            "a destination that nothing else sources is what the shortcut is FOR: a second \
             resolve of it must hand it back, not refuse it"
        );
        assert_eq!(
            table_answer(&TABLE, 0x20, 0),
            TableAnswer::MovedTo { row: 2, to: 0x20 },
            "a row that did not move still answers from the table"
        );
        assert_eq!(
            table_answer(&TABLE, 0x30, 0),
            TableAnswer::AlreadyTranslated,
            "the shortcut may claim a source only when its answer is the row's own destination"
        );
        assert_eq!(
            table_answer(&TABLE, 0x31, 0),
            TableAnswer::MovedTo { row: 4, to: 0x30 },
            "a source whose destination did not move is still a source, answered by row 4 rather \
             than by row 3, which merely shares its destination"
        );
        assert_eq!(
            table_answer(&TABLE, 0x99, 0),
            TableAnswer::Unmapped,
            "an address no row names must be refused, not guessed at"
        );
    }

    /// Every row must resolve to its own destination -- on the call table.
    ///
    /// What this replaced. `verified_map_is_idempotent` filtered to rows where `from != moved` and
    /// then asked `already_translated(from)`, whose second conjunct is
    /// `!is_source_of_a_move(rva)` -- false for every row the filter kept. The set it asserted
    /// over was empty by construction and the test could not fail. Its name promised more than it
    /// checked as well: resolution is not idempotent on this data, because
    /// `resolve(resolve(0x614870))` is `0x616510`, a different function.
    ///
    /// What is left is the property the old doc actually described: no row is swallowed by the
    /// shortcut. Every row is resolved through the same [`table_answer`] the resolvers call, and
    /// the answer is compared to that row's own destination -- no filter in front of it, so the
    /// number of rows checked is the number of rows there are.
    #[test]
    fn every_verified_row_resolves_to_its_own_destination() {
        assert_every_row_resolves_to_itself(&VERIFIED_1162_TO_1170, "VERIFIED_1162_TO_1170");
    }

    /// The same rule on the detour table, which is generated from a different file and used by a
    /// different resolver. Replaces `detour_table_translation_wins_over_the_shortcut`, which
    /// carried the same contradiction between its filter and its predicate.
    #[test]
    fn every_detour_row_resolves_to_its_own_destination() {
        assert_every_row_resolves_to_itself(&DETOUR_SAFE_1162_TO_1170, "DETOUR_SAFE_1162_TO_1170");
    }

    /// Every destination that nothing else claims as a source is recognised, so a double resolve
    /// of one is handed back rather than refused -- the reason the shortcut exists at all.
    ///
    /// The predecessor took the first such row with `find` and then wrapped its assertion in
    /// `if let Some(..)`, so a table with no pure destination -- or an empty one -- passed in
    /// silence. This walks all of them and says how many it expected.
    #[test]
    fn every_pure_destination_is_recognised_as_already_translated() {
        let pure: Vec<u32> = VERIFIED_1162_TO_1170
            .iter()
            .filter(|(from, moved)| from != moved)
            .map(|(_, moved)| *moved)
            .filter(|moved| {
                !VERIFIED_1162_TO_1170
                    .iter()
                    .any(|(other, other_moved)| other == moved && other != other_moved)
            })
            .collect();
        assert!(
            pure.len() >= MIN_EXPECTED_ROWS,
            "only {} of {} rows in VERIFIED_1162_TO_1170 have a destination nothing else sources; \
             under {MIN_EXPECTED_ROWS} means the table is not what this test thinks it is, and \
             the assertion below would be concluding from almost nothing",
            pure.len(),
            VERIFIED_1162_TO_1170.len()
        );
        let unrecognised: Vec<u32> = pure
            .iter()
            .copied()
            .filter(|moved| super::already_translated_in(&VERIFIED_1162_TO_1170, *moved, 0))
            .count()
            .eq(&pure.len())
            .then(Vec::new)
            .unwrap_or_else(|| {
                pure.iter()
                    .copied()
                    .filter(|moved| {
                        !super::already_translated_in(&VERIFIED_1162_TO_1170, *moved, 0)
                    })
                    .collect()
            });
        assert!(
            unrecognised.is_empty(),
            "these destinations are claimed by no other row's source and were still not \
             recognised as already-translated, so a second resolve of them REFUSES: {unrecognised:#x?}"
        );
    }

    /// The table has to be usable by the resolver's linear scans without a duplicate source
    /// silently shadowing a later row. Two sources that agree would be harmless; two that
    /// disagree would make the answer depend on row order.
    #[test]
    fn verified_map_has_one_answer_per_source() {
        assert_table_is_populated(&VERIFIED_1162_TO_1170, "VERIFIED_1162_TO_1170");
        let mut seen: Vec<(u32, u32)> = Vec::new();
        for (from, moved) in VERIFIED_1162_TO_1170 {
            if let Some((_, first)) = seen.iter().find(|(other, _)| *other == from) {
                assert_eq!(
                    *first, moved,
                    "0x{from:x} appears twice with different destinations"
                );
            } else {
                seen.push((from, moved));
            }
        }
    }

    /// One line per address, and the arithmetic that decides which bit.
    ///
    /// The gate this covers stands between the log and the message that was 99% of a 1.955 GB
    /// file, so the failure that matters is the opposite one: an off-by-one in the word/bit split
    /// that silences a row which was never announced. Every row of a three-word bitset is claimed
    /// exactly once and then refused forever, and no row's claim disturbs another's -- which is
    /// what a shared word would do if the mask were built from the row rather than from the row
    /// MODULO the word width. 130 rows is deliberately not a multiple of 64: it exercises a
    /// partly-filled last word, where an off-by-one lands.
    #[test]
    fn a_row_is_announced_once_and_never_again() {
        const ROWS: usize = 130;
        let announced: Vec<AtomicU64> = (0..announcement_words(ROWS))
            .map(|_| AtomicU64::new(0))
            .collect();
        assert_eq!(
            announced.len(),
            3,
            "130 rows at {ANNOUNCEMENT_ROWS_PER_WORD} rows per word needs 3 words; a bitset short \
             by a word would push the last rows onto `get`'s None arm, which logs every time"
        );

        let first: Vec<usize> = (0..ROWS)
            .filter(|row| announce_translation_once(&announced, *row))
            .collect();
        assert_eq!(
            first.len(),
            ROWS,
            "every row must get its one line; rows that did not: {:?}",
            (0..ROWS).filter(|r| !first.contains(r)).collect::<Vec<_>>()
        );

        let repeats: Vec<usize> = (0..ROWS)
            .flat_map(|row| core::iter::repeat_n(row, 4))
            .filter(|row| announce_translation_once(&announced, *row))
            .collect();
        assert!(
            repeats.is_empty(),
            "these rows announced themselves a second time, which is the 4.5-million-line \
             defect: {repeats:?}"
        );
    }

    /// A row index past the end of its bitset logs rather than goes quiet.
    ///
    /// It cannot happen -- the index comes from a lookup in the table the bitset is sized from --
    /// but the arm has to be pinned to the side that reports, because the alternative is a state
    /// in which translations happen and nothing anywhere says so.
    #[test]
    fn an_impossible_row_still_announces() {
        let announced = [AtomicU64::new(0)];
        assert!(
            announce_translation_once(&announced, 64),
            "an out-of-range row must fall to the logging side, not the silent one"
        );
    }

    /// The once-per-address translation gate must never reach a refusal. The refusals are bounded
    /// by their own, far looser rule; being folded into the translation gate would cut them to one
    /// line per address forever, which is the one bound this file must not have.
    ///
    /// A source-level assertion, because the refusals are emitted from `#[cfg(windows)]` bodies
    /// that cannot run on the host. It reads only the code above this test module, so the pattern
    /// strings in the test itself are not counted as occurrences of the thing they look for.
    #[test]
    fn refusals_never_use_the_once_per_address_translation_gate() {
        const GATE: &str = "announce_translation_once";
        for (index, statement) in refusal_literals().iter().enumerate() {
            assert!(
                !statement.contains(GATE),
                "refusal #{index} became conditional on the once-per-ADDRESS translation gate. A \
                 refusal is attributed to its CALLER, and two callers refusing the same address \
                 are two separate dead features:\n{statement}"
            );
        }

        // ...and the translations, which are gated by it, prove the search can see the gate at
        // all. Without this the test passes just as well on a file where the gate does not exist,
        // or where it was renamed and `GATE` now matches nothing anywhere.
        assert_eq!(
            source_above_tests()
                .matches(&format!("if {GATE}(&"))
                .count(),
            2,
            "expected exactly two gated emit sites -- the CALL resolver and the DETOUR one. \
             Finding a different number means the search above is not looking at the shape it \
             thinks it is, and its silence proves nothing"
        );
    }

    /// Every refusal line keeps the prefix `scripts/record-1170-refusals.py` harvests.
    ///
    /// That script reads the distinct addresses a real run asked for and was refused, and feeds
    /// them to `select-needed-1170-rows.py` -- which is how an address whose constant the name
    /// scan cannot see (42 of 54 in one boot) gets carried at all. Its pattern is
    /// `ADDRESS REFUSED(?: FOR DETOUR)? \([^)]*\): (0x14[0-9a-f]+)`, so bounding the repeats is
    /// harmless to it (it de-duplicates into a set) but re-wording a line is not.
    #[test]
    fn every_refusal_line_stays_harvestable_by_the_refusal_recorder() {
        for statement in refusal_literals() {
            let message = statement
                .split_once("\"")
                .expect("a refusal is emitted from a string literal")
                .1;
            assert!(
                message.starts_with("ADDRESS REFUSED ({what}): 0x{address:x}")
                    || message.starts_with("ADDRESS REFUSED FOR DETOUR ({what}): 0x{address:x}"),
                "this refusal line no longer opens with the prefix record-1170-refusals.py \
                 matches, so the address it names would stop reaching the work list:\n{message}"
            );
        }
    }

    /// The source above the test module, so the test's own pattern strings are not searched.
    fn source_above_tests() -> &'static str {
        include_str!("game_build.rs")
            .split_once("#[cfg(test)]")
            .expect("this module has a test cfg")
            .0
    }

    /// Each `address_log(format_args!(...))` statement that emits a refusal.
    ///
    /// The count is asserted here rather than in one caller, and that placement is the fix for a
    /// blinded-and-passed test: a reworded line stops matching `ADDRESS REFUSED`, so it silently
    /// drops out of this list, and every caller that only inspects what the list contains then
    /// agrees the survivors are fine. Asserting the population where the population is built
    /// means a line cannot leave the search by being renamed out of it.
    fn refusal_literals() -> Vec<&'static str> {
        const EXPECTED: usize = 6;
        let code = source_above_tests();
        let mut found = Vec::new();
        let mut cursor = 0usize;
        while let Some(hit) = code[cursor..].find("ADDRESS REFUSED") {
            let at = cursor + hit;
            cursor = at + 1;
            // Only the emitted LITERALS, not the prose above them: every emitted line carries the
            // caller label right after the message name, which no doc comment does.
            if !code[at..at + 40.min(code.len() - at)].contains("({what}):") {
                continue;
            }
            let start = code[..at]
                .rfind("address_log(format_args!(")
                .expect("a refusal is emitted through address_log");
            let end = at
                + code[at..]
                    .find("))")
                    .expect("a closed address_log statement");
            found.push(&code[start..end]);
        }
        assert_eq!(
            found.len(),
            EXPECTED,
            "expected {EXPECTED} refusal literals -- full/went-quiet/milestone on each of the CALL \
             and DETOUR paths. Found {}: a line was deleted, added, or reworded out of the \
             `ADDRESS REFUSED (<label>): 0x<addr>` shape that record-1170-refusals.py harvests",
            found.len()
        );
        found
    }

    /// The first refusal of an address is always written, whichever address it is.
    ///
    /// This is the load-bearing half of the bound. The refusal recorder harvests distinct
    /// addresses, and a feature that goes inert has to cost a line the first time; a bound that
    /// could swallow a first occurrence would convert a loud failure into a silent one, which is
    /// the exact inversion this whole migration is built to prevent.
    #[test]
    fn the_first_refusal_of_every_address_is_written_in_full() {
        let ledger: Vec<AtomicU64> = (0..REFUSAL_SLOTS).map(|_| AtomicU64::new(0)).collect();
        let overflow = AtomicU64::new(0);
        // Real RVA shapes: dense, 16-byte aligned, spread over the image, plus 0 and the last
        // representable one so neither end is special-cased by accident.
        let addresses: Vec<u32> = core::iter::once(0)
            .chain((0..REFUSAL_SLOTS as u32 - 2).map(|n| 0x40_0000 + n * 0x10))
            .chain(core::iter::once(u32::MAX))
            .collect();
        let unwritten: Vec<u32> = addresses
            .iter()
            .copied()
            .filter(|rva| {
                refusal_line_for(note_refusal(&ledger, &overflow, *rva)) != RefusalLine::Full
            })
            .collect();
        assert!(
            unwritten.is_empty(),
            "{} of {} addresses had their FIRST refusal suppressed. A first refusal is the only \
             notice that a feature is dead: {unwritten:#x?}",
            unwritten.len(),
            addresses.len()
        );
    }

    /// The ledger holds the worst case actually measured without spilling to the shared counter.
    ///
    /// That case is the 2026-08-29 boot: 54 distinct addresses refused on the call path and 72
    /// more behind the `FOR DETOUR` wording, 126 together -- and the two paths have a ledger each,
    /// so 126 in one is already double the real load. A spilled address keeps its first line but
    /// loses its own count, and the count is what carries the magnitude.
    ///
    /// The addresses are real ones, taken from the verified map's own sources, and that is the
    /// whole load-bearing choice here. Synthetic RVAs at a fixed stride do not exercise the hash:
    /// a first draft of this test walked `0x400000 + n * 0x10` and stayed green with the slot
    /// index built from the low bits, because uniformly spaced keys land one per probe run
    /// whatever mixing is applied. Real game addresses are not uniform -- 896 of the map's 950
    /// sources are 16-byte aligned -- so the low-bit index collapses 126 of them onto 16 home
    /// slots (measured) and the ledger spills. Hence both assertions below: the spread is what
    /// makes the fit mean something.
    #[test]
    fn the_measured_worst_case_of_real_addresses_fits_without_spilling() {
        /// 2026-08-29: 54 call refusals plus 72 behind the `FOR DETOUR` wording.
        const MEASURED_WORST_CASE: usize = 126;
        /// Twice that, because a ledger sized exactly to the last bad day has no headroom for the
        /// next one -- and the first draft of this ledger (256 slots, 8 probes) spilled on
        /// MEASURED_WORST_CASE itself.
        const REQUIRED_HEADROOM: usize = 252;
        /// Distinct home slots the real addresses must reach. The mixed index reaches 98 of a
        /// possible 126; the low-bit one reaches 16. Anything near the latter is a hash that has
        /// stopped hashing, whatever the fit happens to work out as.
        const MIN_DISTINCT_HOME_SLOTS: usize = 64;

        assert_table_is_populated(&VERIFIED_1162_TO_1170, "VERIFIED_1162_TO_1170");
        let sources: Vec<u32> = VERIFIED_1162_TO_1170
            .iter()
            .map(|(from, _)| *from)
            .collect();
        assert!(
            sources.len() >= REQUIRED_HEADROOM,
            "the map holds only {} sources, too few to load the ledger to {REQUIRED_HEADROOM}",
            sources.len()
        );

        let home_slots: std::collections::BTreeSet<usize> = sources
            .iter()
            .take(MEASURED_WORST_CASE)
            .map(|rva| refusal_slot(*rva, REFUSAL_SLOTS))
            .collect();
        assert!(
            home_slots.len() >= MIN_DISTINCT_HOME_SLOTS,
            "{MEASURED_WORST_CASE} real game addresses reached only {} of {REFUSAL_SLOTS} home \
             slots. They are near-uniformly 16-byte aligned, so an index built from their low \
             bits piles them up; the fits below would then be luck, not headroom",
            home_slots.len()
        );

        for load in [MEASURED_WORST_CASE, REQUIRED_HEADROOM] {
            let ledger: Vec<AtomicU64> = (0..REFUSAL_SLOTS).map(|_| AtomicU64::new(0)).collect();
            let overflow = AtomicU64::new(0);
            for rva in sources.iter().take(load) {
                note_refusal(&ledger, &overflow, *rva);
            }
            let (_, held) = refusal_totals(&ledger, &overflow);
            assert_eq!(
                (overflow.load(Ordering::Relaxed), held as usize),
                (0, load),
                "{load} real addresses did not all get their own slot in a {REFUSAL_SLOTS}-slot \
                 ledger at {REFUSAL_PROBES} probes, so some would lose their per-address count"
            );
        }
    }

    /// One address logs a bounded number of lines and an UNBOUNDED count.
    ///
    /// The count is the point. A bound that merely truncated would leave a reader of thirteen
    /// lines unable to tell thirteen refusals from four and a half million, and the magnitude is
    /// what says how hot the dead path is -- 628 refusals of `CS_MSB_POINT_CTOR_RVA` in one
    /// session is a per-map-open retry; twelve would have been a startup probe.
    #[test]
    fn one_address_goes_quiet_but_its_count_keeps_running() {
        const REFUSALS: u64 = 5_000;
        let ledger: Vec<AtomicU64> = (0..REFUSAL_SLOTS).map(|_| AtomicU64::new(0)).collect();
        let overflow = AtomicU64::new(0);

        let lines: Vec<(u64, RefusalLine)> = (1..=REFUSALS)
            .map(|expected| {
                let occurrence = note_refusal(&ledger, &overflow, 0xcf_9300);
                assert_eq!(
                    occurrence, expected,
                    "the occurrence number skipped: a count that does not advance one per refusal \
                     cannot carry the magnitude"
                );
                (occurrence, refusal_line_for(occurrence))
            })
            .filter(|(_, line)| *line != RefusalLine::Suppressed)
            .collect();

        let written: Vec<u64> = lines.iter().map(|(occurrence, _)| *occurrence).collect();
        assert_eq!(
            written,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 100, 1_000],
            "expected the first {REFUSALS_LOGGED_PER_ADDRESS} in full, then one went-quiet \
             marker, then a restatement at each power of ten -- and nothing else"
        );
        assert_eq!(
            lines.last().map(|(occurrence, _)| *occurrence),
            Some(1_000),
            "the last line a reader sees must state a count, or 5,000 refusals read as 13"
        );

        let (total, addresses) = refusal_totals(&ledger, &overflow);
        assert_eq!(
            total,
            REFUSALS,
            "the ledger must count every refusal, including the {} it did not write",
            REFUSALS - written.len() as u64
        );
        assert_eq!(addresses, 1, "one address must occupy exactly one slot");
    }

    /// Two addresses that hash to the same slot keep separate counts.
    ///
    /// Collisions are not hypothetical at 256 slots, and a ledger that folded two addresses
    /// together would silence the second one's first refusal -- reporting one dead feature and
    /// hiding another, which is worse than the spam it replaced.
    #[test]
    fn colliding_addresses_keep_separate_counts() {
        let ledger: Vec<AtomicU64> = (0..REFUSAL_SLOTS).map(|_| AtomicU64::new(0)).collect();
        let overflow = AtomicU64::new(0);
        let first = 0xcf_9300u32;
        let slot = refusal_slot(first, REFUSAL_SLOTS);
        let second = (first + 1..u32::MAX)
            .find(|rva| refusal_slot(*rva, REFUSAL_SLOTS) == slot)
            .expect("some other RVA hashes to the same slot");

        assert_eq!(note_refusal(&ledger, &overflow, first), 1);
        assert_eq!(note_refusal(&ledger, &overflow, first), 2);
        assert_eq!(
            note_refusal(&ledger, &overflow, second),
            1,
            "0x{second:x} collides with 0x{first:x} and inherited its count, so its own first \
             refusal would have been reported as a repeat and suppressed"
        );
        assert_eq!(note_refusal(&ledger, &overflow, first), 3);
        assert_eq!(note_refusal(&ledger, &overflow, second), 2);
        let (total, addresses) = refusal_totals(&ledger, &overflow);
        assert_eq!((total, addresses), (5, 2));
    }

    /// An address that finds no slot is still reported, and still bounded.
    ///
    /// Both halves matter and they pull opposite ways: falling to silence would hide a refusal,
    /// and falling to unconditional logging would restore the unbounded write this exists to
    /// remove. The shared overflow counter is the compromise, and it obeys the same rule.
    #[test]
    fn an_address_with_no_room_in_the_ledger_is_still_reported() {
        // One slot, so the second distinct address cannot be placed however far it probes.
        let ledger = [AtomicU64::new(0)];
        let overflow = AtomicU64::new(0);
        assert_eq!(note_refusal(&ledger, &overflow, 0x11_1111), 1);

        let homeless: Vec<RefusalLine> = (0..40)
            .map(|_| refusal_line_for(note_refusal(&ledger, &overflow, 0x22_2222)))
            .collect();
        assert_eq!(
            homeless.first(),
            Some(&RefusalLine::Full),
            "an address the ledger has no room for must still cost its first line"
        );
        assert!(
            homeless.contains(&RefusalLine::WentQuiet),
            "the overflow path must go quiet too, or a full ledger restores the unbounded write"
        );
        assert_eq!(overflow.load(Ordering::Relaxed), 40);
    }

    /// The count cannot carry into the key bits and rename an address.
    ///
    /// Pathological, and cheap to make impossible: the alternative is a slot that silently starts
    /// counting a different address's refusals, which is a wrong answer rather than a loud one.
    #[test]
    fn a_saturated_count_never_corrupts_the_address_it_belongs_to() {
        let rva = 0xcf_9300u32;
        let ledger = [AtomicU64::new(
            (REFUSAL_COUNT_CEILING << 32) | (rva as u64 + 1),
        )];
        let overflow = AtomicU64::new(0);
        for _ in 0..8 {
            assert_eq!(note_refusal(&ledger, &overflow, rva), REFUSAL_COUNT_CEILING);
        }
        assert_eq!(
            ledger[0].load(Ordering::Relaxed) & REFUSAL_KEY_MASK,
            rva as u64 + 1,
            "the count carried into the key: this slot now counts refusals of a different address"
        );
        assert_eq!(overflow.load(Ordering::Relaxed), 0);
    }

    /// A conclusion drawn from walking a table is worth exactly as much as the walk was long.
    fn assert_table_is_populated(table: &[(u32, u32)], name: &str) {
        assert!(
            table.len() >= MIN_EXPECTED_ROWS,
            "{name} holds {} rows. Everything asserted about it is a claim over that set, and \
             under {MIN_EXPECTED_ROWS} rows the generator produced almost nothing -- a green tick \
             here would mean the check ran over an empty table, not that the table is good",
            table.len()
        );
    }

    /// `resolve(from) == moved`, for every row, through the resolvers' own decision function.
    fn assert_every_row_resolves_to_itself(table: &[(u32, u32)], name: &str) {
        assert_table_is_populated(table, name);
        let wrong: Vec<(u32, u32, TableAnswer)> = table
            .iter()
            .map(|&(from, moved)| (from, moved, table_answer(table, from, 0)))
            .filter(|&(from, moved, answer)| match answer {
                // The shortcut may claim a source only when the address it hands back is what the
                // row would have returned. That is the rows that did not move, and only them.
                TableAnswer::AlreadyTranslated => from != moved,
                TableAnswer::MovedTo { row, to } => to != moved || table[row].0 != from,
                TableAnswer::Unmapped => true,
            })
            .collect();
        assert!(
            wrong.is_empty(),
            "{name}: {} of {} rows do not resolve to their own destination. A row answered \
             AlreadyTranslated is one the shortcut swallowed -- its translation never runs and \
             nothing logs that it did not. (from, destination, answer): {wrong:#x?}",
            wrong.len(),
            table.len()
        );
    }
}

/// Is Seamless Co-op (`ersc.dll`) resident in this process?
///
/// A monotonic latch, not a sample. me3 defers native loading until after Arxan init and loads
/// Seamless through its me2 compatibility shim, so `ersc.dll` is not yet registered in the PEB when
/// a `DllMain` runs (+1ms) -- a raw module handle returns false that early and would wrongly gate
/// every Seamless decision to "vanilla". So this re-polls on each call until the module first
/// resolves, then latches true forever and never re-samples. That makes the answer correct at the
/// moment each call site needs it (title terms-of-service build ~+16.9s, save read on the first
/// game-task tick), regardless of the early false negative, and it never flaps back to false.
///
/// It lives here rather than in a product telemetry module because it has no product state at all,
/// and because the System>Quit row cloner reads it to decide which help line the
/// **Load Character from File** row advertises -- from `er-quit-menu-core`, across a crate boundary.
#[cfg(windows)]
pub fn seamless_coop_loaded() -> bool {
    use core::sync::atomic::{AtomicUsize, Ordering};

    /// 0 = not yet seen, 1 = observed resident. Once latched it never clears: Seamless is not
    /// unloaded mid-session.
    static LATCHED: AtomicUsize = AtomicUsize::new(0);
    const MODULE_NAME: &[u8] = b"ersc.dll\0";

    unsafe extern "system" {
        fn GetModuleHandleA(module_name: *const u8) -> isize;
    }

    if LATCHED.load(Ordering::Relaxed) != 0 {
        return true;
    }
    // Safety: a NUL-terminated ASCII name; the call only reads it and returns a handle or zero.
    let present = unsafe { GetModuleHandleA(MODULE_NAME.as_ptr()) } != 0;
    if present {
        LATCHED.store(1, Ordering::Relaxed);
    }
    present
}

/// Host builds have no module table to ask, and no Seamless to find.
#[cfg(not(windows))]
pub fn seamless_coop_loaded() -> bool {
    false
}
