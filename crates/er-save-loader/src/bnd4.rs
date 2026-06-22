//! Minimal BND4 reader for Elden Ring PC `.sl2` save files.
//!
//! ER PC saves are a **plaintext** BND4 container (NOT encrypted — see
//! `docs/bnd4-save-format.md`, proven by MD5). Each `USER_DATA00N` entry is
//! `[16-byte MD5 of the body][plaintext body]`. The 10 character slots
//! (`USER_DATA000`..`USER_DATA009`) each have a `0x280000`-byte body — exactly
//! the buffer the engine save parser (`0x67b290`) consumes. This module locates a
//! slot's plaintext body so the DLL can hand it straight to the engine parser:
//! no decryption, no key, no crypto deps.
//!
//! **Data-driven, not hardcoded.** BND4 is self-describing: the header carries the
//! header size and per-entry header stride, and each entry carries its own size.
//! The reader derives the body length from the parsed entry (`entry_size - MD5`)
//! and the structural strides from the header — the `SLOT_*` literals below are
//! only the *expected* char-slot values, used for sanity checks, never as the
//! slicing source of truth. (Runtime side: the buffer size the engine allocs is
//! likewise read from the engine's own `0x67b100(size)` call, not a literal.)

/// Expected plaintext body size of a *character* slot (`USER_DATA000`..`009`).
/// Sanity reference only — actual slices derive length from each entry's size.
pub const SLOT_BODY_LEN: usize = 0x280000;
/// Leading per-entry MD5 checksum length (fixed by the BND4-with-checksum format).
pub const ENTRY_MD5_LEN: usize = 0x10;
/// Expected full entry size of a character slot (`0x10` MD5 + body). Sanity only.
pub const SLOT_ENTRY_LEN: usize = ENTRY_MD5_LEN + SLOT_BODY_LEN; // 0x280010

const BND4_MAGIC: &[u8; 4] = b"BND4";
const MAGIC_LEN: usize = 4;
/// Expected BND4 file header size; the actual value is read from the header
/// (`headerSize` @ `HDR_HEADER_SIZE_OFF`). Used as a minimum-length guard.
const EXPECTED_HEADER_SIZE: usize = 0x40;
/// Expected size of one file-entry header; actual is read from the header
/// (`fileHeaderSize` @ `HDR_FILE_HEADER_SIZE_OFF`).
const EXPECTED_ENTRY_HEADER_SIZE: usize = 0x20;
/// Sane upper bound on entry count (ER saves have 12); guards malformed inputs.
const MAX_ENTRIES: usize = 64;

// Field offsets within the BND4 file header (relative to file start).
/// `int32` file (entry) count.
const HDR_FILE_COUNT_OFF: usize = 0x0c;
/// `int64` header size (where entry headers begin).
const HDR_HEADER_SIZE_OFF: usize = 0x10;
/// `int64` per-entry header stride.
const HDR_FILE_HEADER_SIZE_OFF: usize = 0x20;

// Field offsets within a file-entry header (relative to the entry header start).
/// `int64` entry size, including the leading 0x10 MD5.
const ENT_SIZE_OFF: usize = 0x08;
/// `int32` absolute file offset of this entry's data blob.
const ENT_DATA_OFFSET_OFF: usize = 0x10;
/// `int32` absolute file offset of this entry's UTF-16 name.
const ENT_NAME_OFFSET_OFF: usize = 0x14;

/// Sane upper bound on a UTF-16 entry-name length (units).
const MAX_NAME_UNITS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bnd4Error {
    NotBnd4,
    Truncated,
    SlotNotFound,
    BadEntry,
}

/// One parsed BND4 file-entry header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub data_offset: usize,
    /// Entry size including the leading 0x10 MD5 (the `+0x08` field).
    pub entry_size: usize,
}

fn rd_i32(d: &[u8], at: usize) -> Option<i32> {
    d.get(at..at + 4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn rd_i64(d: &[u8], at: usize) -> Option<i64> {
    d.get(at..at + 8)
        .map(|b| i64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

/// Read a UTF-16LE, NUL-terminated entry name at `at`.
fn rd_utf16_name(d: &[u8], at: usize) -> Option<String> {
    let mut units = Vec::new();
    let mut i = at;
    loop {
        let lo = *d.get(i)?;
        let hi = *d.get(i + 1)?;
        let u = u16::from_le_bytes([lo, hi]);
        if u == 0 {
            break;
        }
        units.push(u);
        i += 2;
        if units.len() > MAX_NAME_UNITS {
            return None; // sane bound
        }
    }
    String::from_utf16(&units).ok()
}

/// Parse the BND4 header + all entry headers.
///
/// Structural strides (header size, per-entry header size) are read from the
/// self-describing header rather than assumed, so the reader follows the file
/// rather than a hardcoded layout.
pub fn parse_entries(data: &[u8]) -> Result<Vec<Entry>, Bnd4Error> {
    if data.len() < EXPECTED_HEADER_SIZE || &data[0..MAGIC_LEN] != BND4_MAGIC {
        return Err(Bnd4Error::NotBnd4);
    }
    // Header is self-describing: take the strides from it, not from literals.
    let header_size = rd_i64(data, HDR_HEADER_SIZE_OFF).ok_or(Bnd4Error::Truncated)? as usize;
    let entry_stride = rd_i64(data, HDR_FILE_HEADER_SIZE_OFF).ok_or(Bnd4Error::Truncated)? as usize;
    // Guard against absurd/corrupt strides that would make offsets meaningless.
    if header_size < EXPECTED_HEADER_SIZE || entry_stride < EXPECTED_ENTRY_HEADER_SIZE {
        return Err(Bnd4Error::BadEntry);
    }
    let file_count = rd_i32(data, HDR_FILE_COUNT_OFF).ok_or(Bnd4Error::Truncated)? as usize;
    if file_count > MAX_ENTRIES {
        return Err(Bnd4Error::BadEntry);
    }
    let mut entries = Vec::with_capacity(file_count);
    for i in 0..file_count {
        let h = header_size + i * entry_stride;
        let entry_size = rd_i64(data, h + ENT_SIZE_OFF).ok_or(Bnd4Error::Truncated)? as usize;
        let data_offset =
            rd_i32(data, h + ENT_DATA_OFFSET_OFF).ok_or(Bnd4Error::Truncated)? as usize;
        let name_offset =
            rd_i32(data, h + ENT_NAME_OFFSET_OFF).ok_or(Bnd4Error::Truncated)? as usize;
        let name = rd_utf16_name(data, name_offset).ok_or(Bnd4Error::BadEntry)?;
        entries.push(Entry {
            name,
            data_offset,
            entry_size,
        });
    }
    Ok(entries)
}

/// Locate a character slot's **plaintext body** (after the leading MD5).
/// `slot` is 0..=9 (`USER_DATA000`..`USER_DATA009`). The body length is derived
/// from the entry's own size field (`entry_size - MD5`), not assumed — for a
/// char slot this is `SLOT_BODY_LEN` (0x280000), but the file is the source of
/// truth.
pub fn slot_body(data: &[u8], slot: usize) -> Result<&[u8], Bnd4Error> {
    let want = format!("USER_DATA{:03}", slot);
    let entry = parse_entries(data)?
        .into_iter()
        .find(|e| e.name == want)
        .ok_or(Bnd4Error::SlotNotFound)?;
    let body_len = entry
        .entry_size
        .checked_sub(ENTRY_MD5_LEN)
        .ok_or(Bnd4Error::BadEntry)?;
    let start = entry.data_offset + ENTRY_MD5_LEN;
    let end = start.checked_add(body_len).ok_or(Bnd4Error::Truncated)?;
    data.get(start..end).ok_or(Bnd4Error::Truncated)
}

/// The 16-byte stored MD5 checksum prefix of a slot entry.
pub fn slot_md5(data: &[u8], slot: usize) -> Result<&[u8], Bnd4Error> {
    let want = format!("USER_DATA{:03}", slot);
    let entry = parse_entries(data)?
        .into_iter()
        .find(|e| e.name == want)
        .ok_or(Bnd4Error::SlotNotFound)?;
    data.get(entry.data_offset..entry.data_offset + ENTRY_MD5_LEN)
        .ok_or(Bnd4Error::Truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_fixture() -> Option<Vec<u8>> {
        // repo-root/save-files/45-Slots/ER0000.sl2 ; crate is crates/er-save-loader
        let p = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../save-files/45-Slots/ER0000.sl2"
        );
        std::fs::read(p).ok()
    }

    #[test]
    fn parses_bnd4_entries() {
        let Some(data) = load_fixture() else {
            eprintln!("fixture missing; skipping");
            return;
        };
        let entries = parse_entries(&data).expect("parse");
        assert_eq!(entries.len(), 12, "ER .sl2 has 12 USER_DATA entries");
        assert_eq!(entries[0].name, "USER_DATA000");
        assert_eq!(entries[9].name, "USER_DATA009");
        assert_eq!(entries[0].data_offset, 0x300);
        assert_eq!(entries[0].entry_size, SLOT_ENTRY_LEN); // 0x280010
    }

    #[test]
    fn rejects_non_bnd4_input() {
        assert_eq!(
            parse_entries(b"not a bnd4 file at all"),
            Err(Bnd4Error::NotBnd4)
        );
        assert_eq!(parse_entries(&[]), Err(Bnd4Error::NotBnd4));
        // Right magic but truncated before the full header.
        assert_eq!(parse_entries(b"BND4"), Err(Bnd4Error::NotBnd4));
    }

    #[test]
    fn out_of_range_slot_is_slot_not_found() {
        let Some(data) = load_fixture() else {
            eprintln!("fixture missing; skipping");
            return;
        };
        assert_eq!(slot_body(&data, 99), Err(Bnd4Error::SlotNotFound));
        assert_eq!(slot_md5(&data, 99), Err(Bnd4Error::SlotNotFound));
    }

    #[test]
    fn all_ten_slots_have_full_body_and_md5() {
        let Some(data) = load_fixture() else {
            eprintln!("fixture missing; skipping");
            return;
        };
        for slot in 0..=9 {
            let body = slot_body(&data, slot).expect("slot body");
            assert_eq!(body.len(), SLOT_BODY_LEN, "slot {slot} body len");
            let md5 = slot_md5(&data, slot).expect("slot md5");
            assert_eq!(md5.len(), ENTRY_MD5_LEN, "slot {slot} md5 len");
        }
    }

    #[test]
    fn slot0_body_is_plaintext_and_matches_c30() {
        let Some(data) = load_fixture() else {
            eprintln!("fixture missing; skipping");
            return;
        };
        let body = slot_body(&data, 0).expect("slot 0 body");
        assert_eq!(body.len(), SLOT_BODY_LEN);
        // c30 (saved map) candidate = body+4, proven 0x1c000000 for this save
        let c30 = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
        assert_eq!(c30, 0x1c00_0000, "slot 0 c30/map dword");
        // plaintext sanity: a real decrypted body is structured (lots of zeros),
        // not high-entropy ciphertext.
        let zeros = body[..0x10000].iter().filter(|&&b| b == 0).count();
        assert!(zeros > 0x10000 / 4, "body should be structured plaintext");
    }
}
