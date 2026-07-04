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

/// Result of rewriting a PC save's embedded SteamID64 values.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SteamIdNormalizeReport {
    /// Character slot bodies whose dynamic layout was readable enough to locate the slot SteamID64.
    pub character_slots_seen: usize,
    /// Character slot bodies whose SteamID64 was changed.
    pub character_slots_patched: usize,
    /// Whether `USER_DATA010` (system/profile data) was present and large enough to inspect.
    pub user_data10_seen: bool,
    /// Whether `USER_DATA010`'s SteamID64 was changed.
    pub user_data10_patched: bool,
    /// Number of BND4 entry MD5 prefixes rewritten after content changes.
    pub md5_rewritten: usize,
}

impl SteamIdNormalizeReport {
    #[must_use]
    pub const fn changed(self) -> bool {
        self.character_slots_patched != 0 || self.user_data10_patched
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamIdLocation {
    pub entry_name: String,
    pub body_offset: usize,
    pub file_offset: usize,
    pub value: u64,
}

// `USER_DATA010` parser reads the full BND entry as `[0x10 MD5][body]`; this module's
// `body_start` is already after that leading MD5, so the entry-relative steam offset 0x14 becomes
// body-relative 0x04.
const USER_DATA10_STEAM_ID_BODY_OFFSET: usize = 0x04;
const STEAM_ID64_LEN: usize = 8;
const SAVE_FACE_MAGIC: &[u8; 4] = b"FACE";
const SAVE_PGD_SCAN_LEADING_FACE_COUNT: usize = 4;
const SAVE_PGD_FACE_DELTA_WINDOW_LOW: usize = 0xa000;
const SAVE_PGD_FACE_DELTA_WINDOW_HIGH: usize = 0xa600;
const SAVE_PLAYER_GAME_DATA_MIN_SIZE: usize = 0x1b0;
const SAVE_PGD_HEALTH_OFFSET: usize = 0x08;
const SAVE_PGD_MAX_HEALTH_OFFSET: usize = 0x0c;
const SAVE_PGD_BASE_MAX_HEALTH_OFFSET: usize = 0x10;
const SAVE_PGD_STAT_BASE_OFFSET: usize = 0x34;
const SAVE_PGD_STAT_COUNT: usize = 8;
const SAVE_PGD_LEVEL_OFFSET: usize = 0x60;
const SAVE_PGD_CHARACTER_NAME_OFFSET: usize = 0x94;
const SAVE_PGD_CHARACTER_NAME_BYTES: usize = 0x20;
const SAVE_PGD_GENDER_OFFSET: usize = 0xb6;
const SAVE_PGD_MAX_CRIMSON_FLASK_OFFSET: usize = 0xf9;
const SAVE_PGD_MAX_CERULEAN_FLASK_OFFSET: usize = 0xfa;
const SAVE_SPEFFECT_COUNT: usize = 0x0d;
const SAVE_SPEFFECT_SIZE: usize = 0x10;
const SAVE_CHR_ASM_EQUIPMENT_SIZE: usize = 0x58;
const SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE: usize = 0x1c;
const SAVE_INVENTORY_HELD_SIZE: usize = 0x9010;
const SAVE_EQUIP_MAGIC_SIZE: usize = 0x74;
const SAVE_EQUIP_ITEM_SIZE: usize = 0x8c;
const SAVE_GESTURE_EQUIP_SIZE: usize = 0x18;
const SAVE_PROJECTILE_ENTRY_SIZE: usize = 0x08;
const SAVE_PROJECTILE_COUNT_MAX: u32 = 0x400;
const SAVE_EQUIPPED_ARMAMENTS_AND_ITEMS_SIZE: usize = 0x9c;
const SAVE_PHYSIC_EQUIP_SIZE: usize = 0x0c;
const SAVE_FACE_DATA_FULL_SIZE: usize = 0x12f;
const SAVE_INVENTORY_STORAGE_SIZE: usize = 0x6010;
const SAVE_GESTURE_GAME_DATA_SIZE: usize = 0x100;
const SAVE_REGION_ID_SIZE: usize = 0x04;
const SAVE_REGION_COUNT_MAX: u32 = 0x400;
const SAVE_RIDE_GAME_DATA_SIZE: usize = 0x28;
const SAVE_BLOODSTAIN_DATA_SIZE: usize = 0x44;
const SAVE_MENU_PROFILE_SAVE_LOAD_SIZE: usize = 0x1008;
const SAVE_TROPHY_EQUIP_DATA_SIZE: usize = 0x34;
const SAVE_GAITEM_GAME_DATA_SIZE: usize = 0x1b588;
const SAVE_TUTORIAL_DATA_SIZE: usize = 0x408;
const SAVE_EVENT_FLAGS_SIZE: usize = 0x1bf99f;
const SAVE_PLAYER_COORDS_SIZE: usize = 0x39;
const SAVE_CS_NET_DATA_CHUNKS_SIZE: usize = 0x20000;
const SAVE_WORLD_AREA_WEATHER_SIZE: usize = 0x0c;
const SAVE_WORLD_AREA_TIME_SIZE: usize = 0x0c;

fn rd_u8(d: &[u8], at: usize) -> Option<u8> {
    d.get(at).copied()
}

fn rd_u32(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn rd_slot_i32(d: &[u8], at: usize) -> Option<i32> {
    d.get(at..at + 4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn slot_field(d: &[u8], at: usize, len: usize) -> Option<&[u8]> {
    d.get(at..at.checked_add(len)?)
}

fn slot_name_units(body: &[u8], pgd_offset: usize) -> Option<Vec<u16>> {
    let bytes = slot_field(
        body,
        pgd_offset.checked_add(SAVE_PGD_CHARACTER_NAME_OFFSET)?,
        SAVE_PGD_CHARACTER_NAME_BYTES,
    )?;
    Some(
        bytes
            .chunks_exact(2)
            .map(|u| u16::from_le_bytes([u[0], u[1]]))
            .take_while(|u| *u != 0)
            .collect(),
    )
}

fn slot_has_real_name(body: &[u8], pgd_offset: usize) -> bool {
    slot_name_units(body, pgd_offset).is_some_and(|units| {
        !units.is_empty()
            && units.iter().any(|u| *u != b'_' as u16)
            && String::from_utf16(&units)
                .ok()
                .is_some_and(|s| s.chars().all(|c| !c.is_control()))
    })
}

fn slot_pgd_core_plausible(body: &[u8], offset: usize) -> bool {
    if offset
        .checked_add(SAVE_PLAYER_GAME_DATA_MIN_SIZE)
        .is_none_or(|end| end > body.len())
        || !slot_has_real_name(body, offset)
    {
        return false;
    }
    let Some(level) = rd_u32(body, offset + SAVE_PGD_LEVEL_OFFSET) else {
        return false;
    };
    let Some(health) = rd_u32(body, offset + SAVE_PGD_HEALTH_OFFSET) else {
        return false;
    };
    let Some(max_health) = rd_u32(body, offset + SAVE_PGD_MAX_HEALTH_OFFSET) else {
        return false;
    };
    let Some(base_max_health) = rd_u32(body, offset + SAVE_PGD_BASE_MAX_HEALTH_OFFSET) else {
        return false;
    };
    let Some(gender) = rd_u8(body, offset + SAVE_PGD_GENDER_OFFSET) else {
        return false;
    };
    let Some(max_crimson) = rd_u8(body, offset + SAVE_PGD_MAX_CRIMSON_FLASK_OFFSET) else {
        return false;
    };
    let Some(max_cerulean) = rd_u8(body, offset + SAVE_PGD_MAX_CERULEAN_FLASK_OFFSET) else {
        return false;
    };
    let mut stats = [0u32; SAVE_PGD_STAT_COUNT];
    for (index, stat) in stats.iter_mut().enumerate() {
        let Some(value) = rd_u32(body, offset + SAVE_PGD_STAT_BASE_OFFSET + index * 4) else {
            return false;
        };
        *stat = value;
    }
    (1..=713).contains(&level)
        && (1..=100_000).contains(&health)
        && (1..=100_000).contains(&max_health)
        && (1..=100_000).contains(&base_max_health)
        && health <= max_health
        && base_max_health <= max_health
        && gender <= 1
        && max_crimson <= 14
        && max_cerulean <= 14
        && stats.iter().all(|stat| (1..=99).contains(stat))
}

fn slot_pgd_score(body: &[u8], offset: usize) -> usize {
    slot_name_units(body, offset).map_or(0, |units| units.len())
        + (0..SAVE_PGD_STAT_COUNT)
            .filter(|index| {
                rd_u32(body, offset + SAVE_PGD_STAT_BASE_OFFSET + index * 4).unwrap_or(0) > 0
            })
            .count()
        + usize::from(rd_u32(body, offset + SAVE_PGD_LEVEL_OFFSET).unwrap_or(0) > 0)
}

fn slot_player_game_data_offset(body: &[u8]) -> Option<usize> {
    let mut best = None;
    let mut best_score = 0usize;
    let mut search_from = 0usize;
    for _ in 0..SAVE_PGD_SCAN_LEADING_FACE_COUNT {
        let Some(tail) = body.get(search_from..) else {
            break;
        };
        let Some(rel) = tail
            .windows(SAVE_FACE_MAGIC.len())
            .position(|w| w == SAVE_FACE_MAGIC)
        else {
            break;
        };
        let face_offset = search_from + rel;
        search_from = face_offset + 1;
        let start = face_offset.saturating_sub(SAVE_PGD_FACE_DELTA_WINDOW_HIGH);
        let stop = face_offset.saturating_sub(SAVE_PGD_FACE_DELTA_WINDOW_LOW);
        for offset in start..=stop {
            if !slot_pgd_core_plausible(body, offset) {
                continue;
            }
            let score = slot_pgd_score(body, offset);
            if score > best_score {
                best_score = score;
                best = Some(offset);
            }
        }
    }
    best
}

fn slot_add_offset(offset: &mut usize, len: usize) -> Option<()> {
    *offset = offset.checked_add(len)?;
    Some(())
}

fn slot_add_counted_region(
    body: &[u8],
    offset: &mut usize,
    entry_size: usize,
    max_count: u32,
) -> Option<()> {
    let count = rd_u32(body, *offset)?;
    if count > max_count {
        return None;
    }
    let bytes = (count as usize).checked_mul(entry_size)?.checked_add(4)?;
    slot_add_offset(offset, bytes)
}

fn slot_add_unknown_list(body: &[u8], offset: &mut usize) -> Option<()> {
    let length = rd_slot_i32(body, *offset)?;
    if length < 0 {
        return None;
    }
    let length = usize::try_from(length).ok()?;
    if offset.checked_add(4)?.checked_add(length)? > body.len() {
        return None;
    }
    slot_add_offset(offset, 4 + length)
}

fn slot_steam_id_offset(body: &[u8]) -> Option<usize> {
    let mut offset = slot_player_game_data_offset(body)?;
    slot_add_offset(&mut offset, SAVE_PLAYER_GAME_DATA_MIN_SIZE)?;
    slot_add_offset(&mut offset, SAVE_SPEFFECT_COUNT * SAVE_SPEFFECT_SIZE)?;
    slot_add_offset(&mut offset, SAVE_CHR_ASM_EQUIPMENT_SIZE)?;
    slot_add_offset(&mut offset, SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE)?;
    slot_add_offset(&mut offset, SAVE_CHR_ASM_EQUIPMENT_SIZE)?;
    slot_add_offset(&mut offset, SAVE_CHR_ASM_EQUIPMENT_SIZE)?;
    slot_add_offset(&mut offset, SAVE_INVENTORY_HELD_SIZE)?;
    slot_add_offset(&mut offset, SAVE_EQUIP_MAGIC_SIZE)?;
    slot_add_offset(&mut offset, SAVE_EQUIP_ITEM_SIZE)?;
    slot_add_offset(&mut offset, SAVE_GESTURE_EQUIP_SIZE)?;
    slot_add_counted_region(
        body,
        &mut offset,
        SAVE_PROJECTILE_ENTRY_SIZE,
        SAVE_PROJECTILE_COUNT_MAX,
    )?;
    slot_add_offset(&mut offset, SAVE_EQUIPPED_ARMAMENTS_AND_ITEMS_SIZE)?;
    slot_add_offset(&mut offset, SAVE_PHYSIC_EQUIP_SIZE)?;
    slot_add_offset(&mut offset, SAVE_FACE_DATA_FULL_SIZE)?;
    slot_add_offset(&mut offset, SAVE_INVENTORY_STORAGE_SIZE)?;
    slot_add_offset(&mut offset, SAVE_GESTURE_GAME_DATA_SIZE)?;
    slot_add_counted_region(
        body,
        &mut offset,
        SAVE_REGION_ID_SIZE,
        SAVE_REGION_COUNT_MAX,
    )?;
    slot_add_offset(&mut offset, SAVE_RIDE_GAME_DATA_SIZE)?;
    slot_add_offset(&mut offset, 1)?;
    slot_add_offset(&mut offset, 0x40)?;
    slot_add_offset(&mut offset, 3 * 4)?;
    slot_add_offset(&mut offset, SAVE_MENU_PROFILE_SAVE_LOAD_SIZE)?;
    slot_add_offset(&mut offset, SAVE_TROPHY_EQUIP_DATA_SIZE)?;
    slot_add_offset(&mut offset, SAVE_GAITEM_GAME_DATA_SIZE)?;
    slot_add_offset(&mut offset, SAVE_TUTORIAL_DATA_SIZE)?;
    slot_add_offset(&mut offset, 0x1d)?;
    slot_add_offset(&mut offset, SAVE_EVENT_FLAGS_SIZE)?;
    slot_add_offset(&mut offset, 1)?;
    for _ in 0..5 {
        slot_add_unknown_list(body, &mut offset)?;
    }
    slot_add_offset(&mut offset, SAVE_PLAYER_COORDS_SIZE)?;
    slot_add_offset(&mut offset, 0x0f)?;
    slot_add_offset(&mut offset, 4)?;
    slot_add_offset(&mut offset, SAVE_CS_NET_DATA_CHUNKS_SIZE)?;
    slot_add_offset(&mut offset, SAVE_WORLD_AREA_WEATHER_SIZE)?;
    slot_add_offset(&mut offset, SAVE_WORLD_AREA_TIME_SIZE)?;
    // The char-slot SteamID64 sits after a 0x14-byte tail pad here. The previous 0x10 landed four
    // bytes early, producing qwords like `00 00 00 00 <first 4 bytes of SteamID>` and corrupting an
    // otherwise same-account save. Runtime proof: 150-Banon slot0 real SteamID is at body+0x21be7b,
    // not body+0x21be77.
    slot_add_offset(&mut offset, 0x14)?;
    (offset.checked_add(STEAM_ID64_LEN)? <= body.len()).then_some(offset)
}

fn entry_body_bounds(entry: &Entry, data_len: usize) -> Result<(usize, usize), Bnd4Error> {
    let body_len = entry
        .entry_size
        .checked_sub(ENTRY_MD5_LEN)
        .ok_or(Bnd4Error::BadEntry)?;
    let body_start = entry
        .data_offset
        .checked_add(ENTRY_MD5_LEN)
        .ok_or(Bnd4Error::Truncated)?;
    let body_end = body_start
        .checked_add(body_len)
        .ok_or(Bnd4Error::Truncated)?;
    if entry
        .data_offset
        .checked_add(ENTRY_MD5_LEN)
        .is_none_or(|end| end > data_len)
        || body_end > data_len
    {
        return Err(Bnd4Error::Truncated);
    }
    Ok((body_start, body_end))
}

fn rewrite_entry_md5(data: &mut [u8], entry: &Entry) -> Result<(), Bnd4Error> {
    let (body_start, body_end) = entry_body_bounds(entry, data.len())?;
    let digest = md5_digest(&data[body_start..body_end]);
    data.get_mut(entry.data_offset..entry.data_offset + ENTRY_MD5_LEN)
        .ok_or(Bnd4Error::Truncated)?
        .copy_from_slice(&digest);
    Ok(())
}

fn patch_u64_le(data: &mut [u8], offset: usize, value: u64) -> Result<bool, Bnd4Error> {
    let dst = data
        .get_mut(offset..offset + STEAM_ID64_LEN)
        .ok_or(Bnd4Error::Truncated)?;
    let current = u64::from_le_bytes([
        dst[0], dst[1], dst[2], dst[3], dst[4], dst[5], dst[6], dst[7],
    ]);
    if current == value {
        return Ok(false);
    }
    dst.copy_from_slice(&value.to_le_bytes());
    Ok(true)
}

pub fn steam_id_locations(data: &[u8]) -> Result<Vec<SteamIdLocation>, Bnd4Error> {
    let entries = parse_entries(data)?;
    let mut out = Vec::new();
    for entry in &entries {
        if let Some(slot_digits) = entry.name.strip_prefix("USER_DATA") {
            if let Ok(slot) = slot_digits.parse::<usize>() {
                if slot < 10 {
                    let (body_start, body_end) = entry_body_bounds(entry, data.len())?;
                    let Some(body_offset) = slot_steam_id_offset(&data[body_start..body_end])
                    else {
                        continue;
                    };
                    let file_offset = body_start + body_offset;
                    if file_offset + STEAM_ID64_LEN <= body_end {
                        let value = u64::from_le_bytes(
                            data[file_offset..file_offset + STEAM_ID64_LEN]
                                .try_into()
                                .map_err(|_| Bnd4Error::Truncated)?,
                        );
                        out.push(SteamIdLocation {
                            entry_name: entry.name.clone(),
                            body_offset,
                            file_offset,
                            value,
                        });
                    }
                    continue;
                }
            }
        }
        if entry.name == "USER_DATA010" {
            let (body_start, body_end) = entry_body_bounds(entry, data.len())?;
            let file_offset = body_start + USER_DATA10_STEAM_ID_BODY_OFFSET;
            if file_offset + STEAM_ID64_LEN <= body_end {
                let value = u64::from_le_bytes(
                    data[file_offset..file_offset + STEAM_ID64_LEN]
                        .try_into()
                        .map_err(|_| Bnd4Error::Truncated)?,
                );
                out.push(SteamIdLocation {
                    entry_name: entry.name.clone(),
                    body_offset: USER_DATA10_STEAM_ID_BODY_OFFSET,
                    file_offset,
                    value,
                });
            }
        }
    }
    Ok(out)
}

/// Rewrite a PC `.sl2`/`.co2` BND4 save so every readable character slot and `USER_DATA010` carry
/// `steam_id`, then refresh the affected per-entry MD5 prefixes.
///
/// This is intentionally byte-level and source-preserving: it does not rewrite the BND4 directory or
/// rebuild character bodies, it only edits the known SteamID64 fields and their checksums. Character
/// slot SteamID offsets are derived by parsing the variable-length slot body layout instead of using a
/// fixed offset, because GaItem/projectile/region/unknown-list counts shift the tail fields per slot.
pub fn normalize_steam_id_in_place(
    data: &mut [u8],
    steam_id: u64,
) -> Result<SteamIdNormalizeReport, Bnd4Error> {
    let entries = parse_entries(data)?;
    let mut report = SteamIdNormalizeReport::default();
    for entry in &entries {
        if let Some(slot_digits) = entry.name.strip_prefix("USER_DATA") {
            if let Ok(slot) = slot_digits.parse::<usize>() {
                if slot < 10 {
                    let (body_start, body_end) = entry_body_bounds(entry, data.len())?;
                    let Some(slot_offset) = slot_steam_id_offset(&data[body_start..body_end])
                    else {
                        continue;
                    };
                    report.character_slots_seen += 1;
                    if patch_u64_le(data, body_start + slot_offset, steam_id)? {
                        report.character_slots_patched += 1;
                        rewrite_entry_md5(data, entry)?;
                        report.md5_rewritten += 1;
                    }
                    continue;
                }
            }
        }
        if entry.name == "USER_DATA010" {
            let (body_start, body_end) = entry_body_bounds(entry, data.len())?;
            if body_start + USER_DATA10_STEAM_ID_BODY_OFFSET + STEAM_ID64_LEN <= body_end {
                report.user_data10_seen = true;
                if patch_u64_le(
                    data,
                    body_start + USER_DATA10_STEAM_ID_BODY_OFFSET,
                    steam_id,
                )? {
                    report.user_data10_patched = true;
                    rewrite_entry_md5(data, entry)?;
                    report.md5_rewritten += 1;
                }
            }
        }
    }
    Ok(report)
}

fn md5_digest(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity((input.len() + 9 + 63) & !63);
    msg.extend_from_slice(input);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let mut a0 = 0x67452301u32;
    let mut b0 = 0xefcdab89u32;
    let mut c0 = 0x98badcfeu32;
    let mut d0 = 0x10325476u32;

    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, word) in m.iter_mut().enumerate() {
            let j = i * 4;
            *word = u32::from_le_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
        }
        let mut a = a0;
        let mut b = b0;
        let mut c = c0;
        let mut d = d0;
        for i in 0..64 {
            let (f, g) = if i < 16 {
                ((b & c) | ((!b) & d), i)
            } else if i < 32 {
                ((d & b) | ((!d) & c), (5 * i + 1) % 16)
            } else if i < 48 {
                (b ^ c ^ d, (3 * i + 5) % 16)
            } else {
                (c ^ (b | !d), (7 * i) % 16)
            };
            let next = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(K[i])
                    .wrapping_add(m[g])
                    .rotate_left(S[i]),
            );
            a = d;
            d = c;
            c = b;
            b = next;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
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

    fn load_150_banon_fixture() -> Option<Vec<u8>> {
        let p = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../save-files/150-Banon/ER0000.sl2"
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
    fn local_md5_matches_known_vectors() {
        assert_eq!(
            md5_digest(b""),
            [
                0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04, 0xe9, 0x80, 0x09, 0x98, 0xec, 0xf8,
                0x42, 0x7e,
            ]
        );
        assert_eq!(
            md5_digest(b"abc"),
            [
                0x90, 0x01, 0x50, 0x98, 0x3c, 0xd2, 0x4f, 0xb0, 0xd6, 0x96, 0x3f, 0x7d, 0x28, 0xe1,
                0x7f, 0x72,
            ]
        );
    }

    #[test]
    fn same_steam_id_is_byte_stable_for_150_banon() {
        let Some(mut data) = load_150_banon_fixture() else {
            eprintln!("150-Banon fixture missing; skipping");
            return;
        };
        let before = data.clone();
        let target = 76_561_197_986_456_766u64;
        let report = normalize_steam_id_in_place(&mut data, target).expect("normalize");
        assert_eq!(report.character_slots_seen, 10);
        assert!(report.user_data10_seen);
        assert_eq!(report.character_slots_patched, 0);
        assert!(!report.user_data10_patched);
        assert_eq!(report.md5_rewritten, 0);
        assert_eq!(
            data, before,
            "same SteamID normalization must be byte-stable"
        );
    }

    #[test]
    fn normalizes_steam_id_and_refreshes_md5() {
        let Some(mut data) = load_fixture() else {
            eprintln!("fixture missing; skipping");
            return;
        };
        let target = 76_561_197_986_456_767u64;
        let report = normalize_steam_id_in_place(&mut data, target).expect("normalize");
        assert!(report.character_slots_seen > 0);
        assert!(report.character_slots_patched > 0);
        assert!(report.user_data10_seen);
        assert!(report.user_data10_patched);
        assert_eq!(report.md5_rewritten, report.character_slots_patched + 1);

        let entries = parse_entries(&data).expect("entries");
        for entry in entries
            .iter()
            .filter(|entry| entry.name.starts_with("USER_DATA"))
        {
            let (body_start, body_end) = entry_body_bounds(entry, data.len()).expect("body bounds");
            assert_eq!(
                &data[entry.data_offset..entry.data_offset + ENTRY_MD5_LEN],
                &md5_digest(&data[body_start..body_end]),
                "{} md5",
                entry.name
            );
            if entry.name == "USER_DATA010" {
                let at = body_start + USER_DATA10_STEAM_ID_BODY_OFFSET;
                assert_eq!(
                    u64::from_le_bytes(data[at..at + STEAM_ID64_LEN].try_into().unwrap()),
                    target
                );
            } else if let Some(digits) = entry.name.strip_prefix("USER_DATA") {
                let slot: usize = digits.parse().unwrap();
                if slot < 10 {
                    let body = &data[body_start..body_end];
                    let Some(slot_offset) = slot_steam_id_offset(body) else {
                        continue;
                    };
                    assert_eq!(
                        u64::from_le_bytes(
                            body[slot_offset..slot_offset + STEAM_ID64_LEN]
                                .try_into()
                                .unwrap()
                        ),
                        target,
                        "slot {slot} steam id"
                    );
                }
            }
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
