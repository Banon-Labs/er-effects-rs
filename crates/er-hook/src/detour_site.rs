//! Is this address a legitimate place to write into the running image?
//!
//! # The question the version gate cannot answer
//!
//! `er_game_base::game_build::resolve_detour_address` answers "where did this 1.16.2 address go on
//! the running build". That is the right question for an address that came from a constant, and it
//! is a question with no meaning for an address the caller found by scanning the running image --
//! an AOB hit, a function pointer read out of a live vtable. Those addresses are already correct
//! for the build in front of them; asking the table where they moved to gets a refusal, because
//! the table is keyed by 1.16.2 RVAs and a 1.17 address is not one of its keys.
//!
//! Measured 2026-08-30: `er-armament-icons` and `er-invasion-warp` both locate the GFx tag-parse
//! function by a unique 30-byte `.text` signature -- *precisely because* hardcoded RVAs drift
//! between patches -- and the scan is correct on 1.17 (`0x1411cf1a0` -> `0x1411d0fa0`,
//! byte-identical body, `.pdata` extent `0x68` in both images). Both then handed the hit to the
//! translating installer, which refused it, and both features were off on a build where the
//! address was right all along. The one mechanism built to survive a patch was the one the
//! migration gate turned off.
//!
//! # What replaces translation, because something must
//!
//! Skipping translation must not mean skipping validation. A wrong absolute address is exactly as
//! fatal as a stale one: MinHook overwrites five bytes, and five bytes written into the middle of
//! a live function corrupt the image with no error anywhere.
//!
//! So a runtime-derived address is checked against the running image's own function table -- the
//! `.pdata` exception directory, which the linker wrote and which the OS itself binary-searches on
//! every unwind. Three answers, and the middle one is the whole reason this module exists:
//!
//! * `Entry` -- `.pdata` declares a function start exactly here. Hook it.
//! * `Leaf` -- no `.pdata` record covers it. The x64 ABI lets a function omit unwind data when it
//!   allocates no stack and calls nothing, so the game's many small getters and `jmp` thunks have
//!   no record at all. Hooking one is fine, and refusing them would throw away a large, legitimate
//!   population -- including `ONLINE_DISABLE_RVA` (`0x67a030`), a leaf in 1.16.2.
//! * `MidFunction` -- the address is inside some other function's declared extent. Refuse.
//!
//! # Why the middle of a function is the case worth a whole module
//!
//! Because it verifies beautifully. A mid-function address sits in the middle of a stable
//! neighbourhood, so every similarity metric agrees at length about the wrong thing:
//! `scripts/classify-1170-entry-kind.py` records six such addresses reaching or nearly reaching
//! the verified map on 2026-08-30, each `IDENTICAL` over 20-94 instructions. `0x140aec480` scored
//! `IDENTICAL 1.000` over 56 instructions and is `+0x360` inside `0x140aec120..0x140aec567`. That
//! script is this check's offline twin, run against the image files; this is the same question
//! asked of the image that is actually loaded, which is the only image a runtime caller can be
//! wrong about.
//!
//! The four stale stub targets are the other half of the demonstration. On 1.17, at their
//! unchanged 1.16.2 RVAs, `0x67a030`, `0xe56310`, `0x24129b0` and `0x240f490` are all
//! MID-FUNCTION -- and three of the four open with a byte that passes their caller's one-byte
//! signature check, because `0x40`/`0x48`/`0x4c` are REX prefixes and the image is full of them.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::hook_log;

/// MinHook overwrites the first five bytes of its target with a `jmp rel32`.
pub const DETOUR_PATCH_BYTES: u32 = 5;

/// DOS header field holding the file offset of the PE header (`e_lfanew`).
const DOS_PE_OFFSET_FIELD: usize = 0x3c;
/// The four bytes that begin the PE header.
const PE_SIGNATURE: [u8; 4] = *b"PE\0\0";
/// COFF header field `SizeOfOptionalHeader`, at `pe + 20`.
const COFF_OPTIONAL_HEADER_SIZE_FIELD: usize = 20;
/// The optional header begins at `pe + 24` (4 signature + 20 COFF).
const OPTIONAL_HEADER_OFFSET: usize = 24;
/// `IMAGE_NT_OPTIONAL_HDR64_MAGIC`; ELDEN RING is PE32+, but the 32-bit layout is one constant
/// away and leaving it out would make a wrong answer look like a missing table.
const PE32PLUS_MAGIC: u16 = 0x20b;
/// Bytes from the start of a PE32+ optional header to its data directory array.
const DATA_DIRECTORY_OFFSET_PE64: usize = 112;
/// The same, for a PE32 optional header.
const DATA_DIRECTORY_OFFSET_PE32: usize = 96;
/// `IMAGE_DIRECTORY_ENTRY_EXCEPTION` -- the `.pdata` function table.
const EXCEPTION_DIRECTORY_INDEX: usize = 3;
/// Each data directory entry is `{ VirtualAddress: u32, Size: u32 }`.
const DATA_DIRECTORY_ENTRY_LEN: usize = 8;
/// `RUNTIME_FUNCTION` on x64: `{ BeginAddress, EndAddress, UnwindInfoAddress }`, all `u32`.
const RUNTIME_FUNCTION_LEN: usize = 12;
/// A function table larger than this is not a function table -- it is a misparsed header, and
/// binary-searching it would read wild addresses. 1.16.2 declares 235,823 functions and 1.17
/// declares 235,863, so this is an order of magnitude of headroom.
const MAX_PLAUSIBLE_FUNCTIONS: usize = 4_000_000;
/// Alignment padding the linker writes between functions (`int3`).
const INT3_PADDING: u8 = 0xcc;
/// How many opening bytes are read to tell code from padding.
const OPENING_BYTES: usize = 8;
/// Lower bound of the `.text` sanity window, and the smallest sensible RVA.
const FIRST_SECTION_RVA: usize = 0x1000;

/// What the running image's own `.pdata` says about an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// `.pdata` declares a function start here, spanning `extent` bytes.
    Entry { extent: u32 },
    /// No `.pdata` record covers this address: an x64 leaf, or padding. `room` is the distance to
    /// the next declared function start (`u32::MAX` when there is none after it).
    Leaf { room: u32 },
    /// Inside another function's declared extent. Never a write site.
    MidFunction { begin: u32, end: u32 },
}

/// Why an address was refused as a write site. One variant per thing a reader can act on -- a
/// single "refused" would send every one of these to the same wrong investigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The address is zero. A refused RVA resolves to `0`, and it is the caller's job to notice.
    Null,
    /// Outside the running game image's `.text`. This module can only consult the game's function
    /// table, so an address in a foreign module is unauditable here rather than acceptable.
    OutsideText,
    /// The opening bytes could not be read, so nothing is known about them.
    Unreadable,
    /// The site opens with alignment padding rather than code.
    Padding(u8),
    /// Inside `begin..end`, which some other function declares as its body.
    MidFunction { begin: u32, end: u32 },
    /// Fewer than `needed` bytes belong to this site, so the write would run into its neighbour.
    TooShort { room: u32, needed: u32 },
    /// The image's exception directory could not be parsed, so the question cannot be asked.
    NoFunctionTable,
}

impl Refusal {
    /// One clause naming what was found, for the tail of a refusal line.
    pub fn describe(self) -> String {
        match self {
            Refusal::Null => "the address is 0".to_string(),
            Refusal::OutsideText => {
                "it is outside the running game image's .text, so the game's own function table \
                 cannot say what lives there"
                    .to_string()
            }
            Refusal::Unreadable => "its opening bytes could not be read".to_string(),
            Refusal::Padding(byte) => {
                format!("it opens with 0x{byte:02x} alignment padding, not code")
            }
            Refusal::MidFunction { begin, end } => format!(
                "it is INSIDE the function the image declares at rva 0x{begin:x}..0x{end:x}, not a \
                 function entry -- writing here would land in the middle of a live body"
            ),
            Refusal::TooShort { room, needed } => {
                format!("only 0x{room:x} bytes belong to this site and the write needs {needed}")
            }
            Refusal::NoFunctionTable => {
                "the running image's exception directory could not be parsed, so its function \
                 boundaries are unknown"
                    .to_string()
            }
        }
    }
}

/// Where an RVA falls in a sorted `.pdata` table.
///
/// `record(i)` yields the `i`th `(begin, end)` pair and `None` if it cannot be read; the table is
/// sorted by `begin`, which is not an assumption but a format requirement -- the OS itself binary-
/// searches it on every unwind. Verified on both images: 235,823 / 235,863 records, zero
/// out-of-order, zero all-zero records.
///
/// A pure function of a lookup so the classification can be tested on the host, where there is no
/// running image to consult.
pub(crate) fn classify(
    rva: u32,
    records: usize,
    mut record: impl FnMut(usize) -> Option<(u32, u32)>,
) -> Option<EntryKind> {
    if records == 0 || records > MAX_PLAUSIBLE_FUNCTIONS {
        return None;
    }
    // Last record whose `begin` is at or below `rva`; `low` ends as the index of the first record
    // strictly after it, which is also the neighbour a leaf's room is measured against.
    let mut low = 0usize;
    let mut high = records;
    let mut at_or_below = None;
    while low < high {
        let mid = low + (high - low) / 2;
        let (begin, end) = record(mid)?;
        if begin <= rva {
            at_or_below = Some((begin, end));
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    match at_or_below {
        Some((begin, end)) if begin == rva => Some(EntryKind::Entry {
            extent: end.saturating_sub(begin),
        }),
        Some((begin, end)) if rva < end => Some(EntryKind::MidFunction { begin, end }),
        // Either past the end of the record below it, or before the first record: no unwind data
        // covers this address.
        _ => {
            let room = match low < records {
                true => record(low)?.0.saturating_sub(rva),
                false => u32::MAX,
            };
            Some(EntryKind::Leaf { room })
        }
    }
}

/// May `needed` bytes be written at a site that classified as `kind` and opens with `opening`?
///
/// Pure, so the policy -- refuse mid-function, accept a leaf, require room -- is asserted on the
/// host rather than only in a game.
pub(crate) fn judge(kind: EntryKind, opening: &[u8], needed: u32) -> Result<(), Refusal> {
    if opening.first() == Some(&INT3_PADDING) {
        return Err(Refusal::Padding(INT3_PADDING));
    }
    if !opening.is_empty() && opening.iter().all(|byte| *byte == 0) {
        return Err(Refusal::Padding(0));
    }
    match kind {
        EntryKind::MidFunction { begin, end } => Err(Refusal::MidFunction { begin, end }),
        EntryKind::Entry { extent } if extent < needed => Err(Refusal::TooShort {
            room: extent,
            needed,
        }),
        EntryKind::Leaf { room } if room < needed => Err(Refusal::TooShort { room, needed }),
        EntryKind::Entry { .. } | EntryKind::Leaf { .. } => Ok(()),
    }
}

/// Read a little-endian `u32` from the running process, fault-safe.
#[cfg(windows)]
fn read_u32(address: usize) -> Option<u32> {
    let mut raw = [0u8; 4];
    match unsafe { er_game_base::mem::read_bytes(address, &mut raw) } {
        true => Some(u32::from_le_bytes(raw)),
        false => None,
    }
}

/// Read a little-endian `u16` from the running process, fault-safe.
#[cfg(windows)]
fn read_u16(address: usize) -> Option<u16> {
    let mut raw = [0u8; 2];
    match unsafe { er_game_base::mem::read_bytes(address, &mut raw) } {
        true => Some(u16::from_le_bytes(raw)),
        false => None,
    }
}

/// The running game image's function table as `(address of record 0, record count)`.
///
/// Parsed from the in-memory headers rather than from a file, for the same reason
/// `er_game_base::mem::module_text_range` is: the only image whose layout can make a runtime
/// caller wrong is the one that is actually mapped.
#[cfg(windows)]
fn function_table() -> Option<(usize, usize)> {
    let base = er_game_base::mem::game_module_base().ok()?;
    let pe = base + read_u32(base + DOS_PE_OFFSET_FIELD)? as usize;
    let mut signature = [0u8; PE_SIGNATURE.len()];
    if !unsafe { er_game_base::mem::read_bytes(pe, &mut signature) } || signature != PE_SIGNATURE {
        return None;
    }
    let optional = pe + OPTIONAL_HEADER_OFFSET;
    let optional_size = read_u16(pe + COFF_OPTIONAL_HEADER_SIZE_FIELD)? as usize;
    let directories = match read_u16(optional)? {
        PE32PLUS_MAGIC => DATA_DIRECTORY_OFFSET_PE64,
        _ => DATA_DIRECTORY_OFFSET_PE32,
    };
    // The directory array must lie inside the optional header the COFF header declares; a header
    // this walk misparsed would otherwise be read as a table of wild addresses.
    let entry = directories + EXCEPTION_DIRECTORY_INDEX * DATA_DIRECTORY_ENTRY_LEN;
    if entry + DATA_DIRECTORY_ENTRY_LEN > optional_size {
        return None;
    }
    let table_rva = read_u32(optional + entry)? as usize;
    let table_size = read_u32(optional + entry + 4)? as usize;
    if table_rva < FIRST_SECTION_RVA || table_size < RUNTIME_FUNCTION_LEN {
        return None;
    }
    Some((base + table_rva, table_size / RUNTIME_FUNCTION_LEN))
}

/// [`classify`] against the running image's `.pdata`.
#[cfg(windows)]
fn classify_live(address: usize) -> Result<EntryKind, Refusal> {
    let base = er_game_base::mem::game_module_base().map_err(|_| Refusal::NoFunctionTable)?;
    let (table, records) = function_table().ok_or(Refusal::NoFunctionTable)?;
    let rva = (address - base) as u32;
    let record = |index: usize| -> Option<(u32, u32)> {
        let mut raw = [0u8; RUNTIME_FUNCTION_LEN];
        if !unsafe { er_game_base::mem::read_bytes(table + index * RUNTIME_FUNCTION_LEN, &mut raw) }
        {
            return None;
        }
        Some((
            u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]),
            u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]),
        ))
    };
    classify(rva, records, record).ok_or(Refusal::NoFunctionTable)
}

/// Cap on refusal lines from this module. An unbounded refusal log is not a hypothetical here:
/// one session recorded 339,764 refusals of a single address, which is the reason
/// `scripts/check-no-rva-zero.py` exists.
const MAX_REFUSAL_LINES: usize = 12;
static REFUSAL_LINES: AtomicUsize = AtomicUsize::new(0);

/// Log a refusal, then stop -- one suppression line marks where the log went quiet, so a reader
/// can tell "no more refusals" from "no more room".
fn log_refusal(what: &str, address: usize, refusal: Refusal) {
    let seen = REFUSAL_LINES.fetch_add(1, Ordering::Relaxed);
    if seen < MAX_REFUSAL_LINES {
        hook_log(format_args!(
            "SITE REFUSED ({what}): 0x{address:x} -- {}",
            refusal.describe()
        ));
    } else if seen == MAX_REFUSAL_LINES {
        hook_log(format_args!(
            "SITE REFUSED: {MAX_REFUSAL_LINES} refusals logged; further ones are suppressed"
        ));
    }
}

/// Is `address` a place this process may write `needed` bytes of code?
///
/// The address must already be correct for the running build -- this asks nothing about versions,
/// and answers only what the running image's own function table can say. Refusals are logged
/// naming which check failed, bounded; `true` means the site passed every check below.
///
/// Windows-only because the image being audited is the running game; a host build has none, and a
/// host caller would be asking about nothing.
#[cfg(windows)]
pub(crate) fn write_site_is_sound(address: usize, needed: u32, what: &str) -> bool {
    match audit_write_site(address, needed, what) {
        Ok(kind) => {
            hook_log(format_args!(
                "SITE OK ({what}): 0x{address:x} is {kind:?} in the running image's own function \
                 table; {needed} bytes may be written here"
            ));
            true
        }
        Err(refusal) => {
            log_refusal(what, address, refusal);
            false
        }
    }
}

/// [`write_site_is_sound`], as a `Result` so the checks read in order and each names itself.
#[cfg(windows)]
fn audit_write_site(address: usize, needed: u32, _what: &str) -> Result<EntryKind, Refusal> {
    if address == 0 {
        return Err(Refusal::Null);
    }
    // `.text` rather than the whole image: an address in `.data` or `.rdata` decodes as
    // instructions just as happily and is never a function entry.
    let (text_start, text_len) =
        er_game_base::mem::module_text_range().ok_or(Refusal::OutsideText)?;
    if address < text_start || address >= text_start + text_len {
        return Err(Refusal::OutsideText);
    }
    let mut opening = [0u8; OPENING_BYTES];
    if !unsafe { er_game_base::mem::read_bytes(address, &mut opening) } {
        return Err(Refusal::Unreadable);
    }
    let kind = classify_live(address)?;
    judge(kind, &opening, needed)?;
    Ok(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two functions with a gap between them, the shape every case below is cut from:
    /// `0x1000..0x1068` (the GFx tag-parse function's real extent), then padding, then
    /// `0x1080..0x1085`.
    const TABLE: [(u32, u32); 2] = [(0x1000, 0x1068), (0x1080, 0x1085)];

    fn lookup(index: usize) -> Option<(u32, u32)> {
        TABLE.get(index).copied()
    }

    fn classify_in_table(rva: u32) -> Option<EntryKind> {
        classify(rva, TABLE.len(), lookup)
    }

    /// A declared function start is an entry, and its extent comes back with it.
    #[test]
    fn a_declared_function_start_is_an_entry() {
        assert_eq!(
            classify_in_table(0x1000),
            Some(EntryKind::Entry { extent: 0x68 })
        );
        assert_eq!(
            classify_in_table(0x1080),
            Some(EntryKind::Entry { extent: 5 })
        );
    }

    /// The case this module exists for. Every byte inside a declared body is mid-function --
    /// including the one just past the entry, which is where a wrong dump shift lands.
    #[test]
    fn every_byte_inside_a_declared_body_is_mid_function() {
        for rva in [0x1001, 0x1030, 0x1067] {
            assert_eq!(
                classify_in_table(rva),
                Some(EntryKind::MidFunction {
                    begin: 0x1000,
                    end: 0x1068
                }),
                "rva 0x{rva:x} must not pass as a write site"
            );
        }
    }

    /// A gap between two functions is a leaf, and its room is the distance to the next entry --
    /// so the padding between them cannot be mistaken for unlimited space.
    #[test]
    fn a_gap_is_a_leaf_with_room_to_the_next_entry() {
        assert_eq!(
            classify_in_table(0x1068),
            Some(EntryKind::Leaf { room: 0x18 })
        );
        assert_eq!(classify_in_table(0x107e), Some(EntryKind::Leaf { room: 2 }));
    }

    /// Before the first record and after the last one there is no unwind data either. The tail has
    /// no next entry to measure against, so its room is unbounded rather than zero -- reporting 0
    /// there would refuse every function at the end of `.text`.
    #[test]
    fn outside_the_table_is_a_leaf_at_both_ends() {
        assert_eq!(
            classify_in_table(0x400),
            Some(EntryKind::Leaf { room: 0xc00 })
        );
        assert_eq!(
            classify_in_table(0x2000),
            Some(EntryKind::Leaf { room: u32::MAX })
        );
    }

    /// An unreadable record refuses rather than guessing. A binary search that silently treated a
    /// failed read as "not found" would answer leaf for a mid-function address.
    #[test]
    fn an_unreadable_record_refuses_instead_of_answering() {
        assert_eq!(classify(0x1000, TABLE.len(), |_| None), None);
    }

    /// An empty or absurd table is not a table.
    #[test]
    fn an_implausible_table_is_refused() {
        assert_eq!(classify(0x1000, 0, lookup), None);
        assert_eq!(classify(0x1000, MAX_PLAUSIBLE_FUNCTIONS + 1, lookup), None);
    }

    /// The policy: mid-function is refused, an entry and a leaf are accepted.
    #[test]
    fn only_mid_function_is_refused_outright() {
        let code = [0x40, 0x53, 0x48, 0x83, 0xec, 0x40, 0x48, 0x8b];
        assert_eq!(
            judge(
                EntryKind::MidFunction {
                    begin: 0x1000,
                    end: 0x1068
                },
                &code,
                DETOUR_PATCH_BYTES
            ),
            Err(Refusal::MidFunction {
                begin: 0x1000,
                end: 0x1068
            })
        );
        assert_eq!(
            judge(EntryKind::Entry { extent: 0x68 }, &code, DETOUR_PATCH_BYTES),
            Ok(())
        );
        assert_eq!(
            judge(EntryKind::Leaf { room: 0x18 }, &code, DETOUR_PATCH_BYTES),
            Ok(())
        );
    }

    /// MinHook writes five bytes; a site with fewer than five to give is refused, entry or leaf.
    #[test]
    fn a_site_too_short_for_the_write_is_refused() {
        let code = [0x40, 0x53, 0x48, 0x83];
        assert_eq!(
            judge(EntryKind::Entry { extent: 4 }, &code, DETOUR_PATCH_BYTES),
            Err(Refusal::TooShort {
                room: 4,
                needed: DETOUR_PATCH_BYTES
            })
        );
        assert_eq!(
            judge(EntryKind::Leaf { room: 2 }, &code, DETOUR_PATCH_BYTES),
            Err(Refusal::TooShort {
                room: 2,
                needed: DETOUR_PATCH_BYTES
            })
        );
        // The same leaf is a fine place for a 2-byte write.
        assert_eq!(judge(EntryKind::Leaf { room: 2 }, &code, 2), Ok(()));
    }

    /// Alignment padding classifies as a leaf with room, so only the opening bytes tell it from a
    /// real function. Hooking padding installs a detour nothing ever calls.
    #[test]
    fn padding_is_refused_even_where_there_is_room_for_it() {
        assert_eq!(
            judge(
                EntryKind::Leaf { room: 0x18 },
                &[0xcc; 8],
                DETOUR_PATCH_BYTES
            ),
            Err(Refusal::Padding(INT3_PADDING))
        );
        assert_eq!(
            judge(
                EntryKind::Leaf { room: 0x18 },
                &[0x00; 8],
                DETOUR_PATCH_BYTES
            ),
            Err(Refusal::Padding(0))
        );
    }

    /// Every refusal says something different. One shared string would send four different
    /// investigations to the same wrong place.
    #[test]
    fn each_refusal_describes_itself() {
        let described = [
            Refusal::Null,
            Refusal::OutsideText,
            Refusal::Unreadable,
            Refusal::Padding(INT3_PADDING),
            Refusal::MidFunction {
                begin: 0x1000,
                end: 0x1068,
            },
            Refusal::TooShort {
                room: 2,
                needed: DETOUR_PATCH_BYTES,
            },
            Refusal::NoFunctionTable,
        ]
        .map(Refusal::describe);
        for (index, text) in described.iter().enumerate() {
            assert!(!text.is_empty());
            assert!(
                !described[..index].contains(text),
                "two refusals share a description: {text}"
            );
        }
    }
}
