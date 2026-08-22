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

/// Body-relative offset of the slot's saved `BlockId` (the packed map id that becomes
/// `GameMan.stayInMultipleAreaBlockId`, a.k.a. the runtime `c30`).
///
/// **This is `0x04`, NOT `0x14`** — re-verified against the 1.16.2 Ghidra dump 2026-08-02
/// and again 2026-08-03, both directions:
///
/// * SERIALIZER `FUN_14067dc00` builds the 16-byte body header as
///   `local_150 = CONCAT44(BVar3, GetGameDataVersion()); Write(&local_150, 0x10)` — little-endian,
///   so the data version lands at body+0x00 and the `BlockId` at body+0x04. It then writes FOUR
///   `CS::CSRandXorshift::NextInt` dwords as the *next* 0x10 bytes, so **body+0x10..0x20 is random
///   noise** and body+0x14 is merely the second random dword.
/// * DESERIALIZER `FUN_14067bd70` mirrors it: `ReadBytes(&local_50, 0x10)` then
///   `param_1->stayInMultipleAreaBlockId = local_50._4_4_` (bytes 4..8 of that read), then
///   `SkipBytes(0x10)` straight over the random block.
/// * `GameMan::stayInMultipleAreaBlockId` sits at struct offset 3120 = `0xC30`, which is exactly
///   the field `GAME_MAN_SAVED_MAP_C30_OFFSET` reads at runtime — so body+0x04 and the live `c30`
///   are the same quantity.
///
/// Byte-confirmed on the corpus: `save-files/25-Invades-patches/ER0000.sl2` `USER_DATA003` body
/// header is `fa 00 00 00 | 00 00 01 0c | 07 40 4e 17 | 00 00 00 00 | aa 52 39 d1 | 7c 45 20 3e ...`,
/// i.e. body+0x04 = `0x0c010000` (a clean packed BlockId, areaId 0x0c) and body+0x14 = `0x3e20457c`
/// (uniform garbage). Across 726 active corpus slots every body+0x04 areaId lands in
/// `0x0a..=0x3d`, while only 9% of body+0x14 words do.
///
/// The DLL's `SAVE_SLOT_MAP_OFFSET` is a re-export of THIS constant, so there is exactly one
/// literal: the `0x14` that shipped previously wrote a random dword into the live
/// `CS::ProfileSummary` record at +0x30 and made the portrait identity semaphore fire on noise.
/// Same off-by-0x10 as the sibling steam-id field below, which was already correct.
pub const SLOT_BODY_MAP_OFFSET: usize = 0x04;

/// Read a character slot body's saved `BlockId` / map id (see [`SLOT_BODY_MAP_OFFSET`]).
/// `None` when the body is too short to hold the header.
#[must_use]
pub fn slot_saved_map(body: &[u8]) -> Option<i32> {
    body.get(SLOT_BODY_MAP_OFFSET..SLOT_BODY_MAP_OFFSET + 4)
        .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

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

/// Locate an entry's **plaintext body** (after the leading MD5).
/// The body length is derived from the entry's own size field (`entry_size - MD5`), not assumed.
pub fn entry_body<'a>(data: &'a [u8], name: &str) -> Result<&'a [u8], Bnd4Error> {
    let entry = parse_entries(data)?
        .into_iter()
        .find(|e| e.name == name)
        .ok_or(Bnd4Error::SlotNotFound)?;
    let body_len = entry
        .entry_size
        .checked_sub(ENTRY_MD5_LEN)
        .ok_or(Bnd4Error::BadEntry)?;
    let start = entry.data_offset + ENTRY_MD5_LEN;
    let end = start.checked_add(body_len).ok_or(Bnd4Error::Truncated)?;
    data.get(start..end).ok_or(Bnd4Error::Truncated)
}

/// Locate a character slot's **plaintext body** (after the leading MD5).
/// `slot` is 0..=9 (`USER_DATA000`..`USER_DATA009`). The body length is derived
/// from the entry's own size field (`entry_size - MD5`), not assumed — for a
/// char slot this is `SLOT_BODY_LEN` (0x280000), but the file is the source of
/// truth.
pub fn slot_body(data: &[u8], slot: usize) -> Result<&[u8], Bnd4Error> {
    entry_body(data, &format!("USER_DATA{:03}", slot))
}

const USER_DATA010_NAME: &str = "USER_DATA010";
/// Offset in `USER_DATA010`'s plaintext body of `CSMenuSystemSaveLoad.length`.
/// The active-slot bytes immediately follow that variable-length blob.
const USER_DATA010_MENU_SAVE_LOAD_LEN_OFF: usize = 0x150;
const USER_DATA010_MENU_SAVE_LOAD_DATA_AFTER_LEN_OFF: usize = 0x154;
pub const SAVE_SLOT_COUNT: usize = 10;

/// Read the authoritative per-slot occupancy bitmap from `USER_DATA010.active_slot`.
/// This is the on-disk source the game/editor use to decide which character slots are active;
/// deleted/inactive slots may still contain stale `USER_DATA00N` character bodies and must not be
/// treated as loadable just because their body parses.
pub fn active_slots(data: &[u8]) -> Result<[bool; SAVE_SLOT_COUNT], Bnd4Error> {
    let body = entry_body(data, USER_DATA010_NAME)?;
    let len =
        rd_u32(body, USER_DATA010_MENU_SAVE_LOAD_LEN_OFF).ok_or(Bnd4Error::Truncated)? as usize;
    let active_off = USER_DATA010_MENU_SAVE_LOAD_DATA_AFTER_LEN_OFF
        .checked_add(len)
        .ok_or(Bnd4Error::BadEntry)?;
    let bytes = body
        .get(active_off..active_off + SAVE_SLOT_COUNT)
        .ok_or(Bnd4Error::Truncated)?;
    let mut slots = [false; SAVE_SLOT_COUNT];
    for (index, byte) in bytes.iter().copied().enumerate() {
        match byte {
            0 => slots[index] = false,
            1 => slots[index] = true,
            _ => return Err(Bnd4Error::BadEntry),
        }
    }
    Ok(slots)
}

/// True when a save container has at least one active character slot.
pub fn has_active_slot(data: &[u8]) -> Result<bool, Bnd4Error> {
    active_slots(data).map(|slots| slots.into_iter().any(|active| active))
}

/// One active character slot parsed from the save container bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharacterSlotInfo {
    pub slot: usize,
    pub name: String,
    pub level: i32,
}

/// Parse active, loadable character slots from a save container.
///
/// This applies the same filter the autoload path expects: `USER_DATA010.active_slot` must mark the
/// slot occupied, the slot body must contain a plausible PlayerGameData block, the name must be
/// non-empty, and the level must be at least 1.
pub fn active_character_slots(data: &[u8]) -> Result<Vec<CharacterSlotInfo>, Bnd4Error> {
    let active_slots = active_slots(data)?;
    Ok((0..SAVE_SLOT_COUNT)
        .filter_map(|slot| {
            if !active_slots.get(slot).copied().unwrap_or(false) {
                return None;
            }
            let body = slot_body(data, slot).ok()?;
            let pgd_offset = slot_player_game_data_offset(body)?;
            let units = slot_name_units(body, pgd_offset)?;
            let name = String::from_utf16(&units).ok()?;
            if name.trim().is_empty() {
                return None;
            }
            let level = rd_slot_i32(body, pgd_offset + SAVE_PGD_LEVEL_OFFSET).unwrap_or(0);
            if level < 1 {
                return None;
            }
            Some(CharacterSlotInfo { slot, name, level })
        })
        .collect())
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
#[allow(
    dead_code,
    reason = "retained RE fact: BloodstainData is part of the documented slot-body layout table even though the tail walk does not need its size"
)]
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
            .as_chunks::<2>()
            .0
            .iter()
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

fn read_u64_le(data: &[u8], offset: usize) -> Result<u64, Bnd4Error> {
    Ok(u64::from_le_bytes(
        data.get(offset..offset + STEAM_ID64_LEN)
            .ok_or(Bnd4Error::Truncated)?
            .try_into()
            .map_err(|_| Bnd4Error::Truncated)?,
    ))
}

fn patch_u64_le(data: &mut [u8], offset: usize, value: u64) -> Result<bool, Bnd4Error> {
    let current = read_u64_le(data, offset)?;
    if current == value {
        return Ok(false);
    }
    data.get_mut(offset..offset + STEAM_ID64_LEN)
        .ok_or(Bnd4Error::Truncated)?
        .copy_from_slice(&value.to_le_bytes());
    Ok(true)
}

fn push_unique_steam_id(ids: &mut Vec<u64>, value: u64, target: u64) {
    if value != 0 && value != target && !ids.contains(&value) {
        ids.push(value);
    }
}

fn source_steam_ids(data: &[u8], entries: &[Entry], target: u64) -> Result<Vec<u64>, Bnd4Error> {
    let mut ids = Vec::new();
    for entry in entries {
        if entry.name == "USER_DATA010" {
            let (body_start, body_end) = entry_body_bounds(entry, data.len())?;
            let file_offset = body_start + USER_DATA10_STEAM_ID_BODY_OFFSET;
            if file_offset + STEAM_ID64_LEN <= body_end {
                push_unique_steam_id(&mut ids, read_u64_le(data, file_offset)?, target);
            }
            continue;
        }
        if let Some(slot_digits) = entry.name.strip_prefix("USER_DATA")
            && let Ok(slot) = slot_digits.parse::<usize>()
            && slot < 10
        {
            let (body_start, body_end) = entry_body_bounds(entry, data.len())?;
            if let Some(body_offset) = slot_steam_id_offset(&data[body_start..body_end]) {
                let file_offset = body_start + body_offset;
                if file_offset + STEAM_ID64_LEN <= body_end {
                    push_unique_steam_id(&mut ids, read_u64_le(data, file_offset)?, target);
                }
            }
        }
    }
    Ok(ids)
}

fn replace_steam_id_occurrences(body: &mut [u8], old_id: u64, target: u64) -> usize {
    let old = old_id.to_le_bytes();
    let new = target.to_le_bytes();
    let mut count = 0;
    let mut offset = 0;
    while offset + STEAM_ID64_LEN <= body.len() {
        if body[offset..offset + STEAM_ID64_LEN] == old {
            body[offset..offset + STEAM_ID64_LEN].copy_from_slice(&new);
            count += 1;
            offset += STEAM_ID64_LEN;
        } else {
            offset += 1;
        }
    }
    count
}

pub fn steam_id_locations(data: &[u8]) -> Result<Vec<SteamIdLocation>, Bnd4Error> {
    let entries = parse_entries(data)?;
    let mut out = Vec::new();
    for entry in &entries {
        if let Some(slot_digits) = entry.name.strip_prefix("USER_DATA")
            && let Ok(slot) = slot_digits.parse::<usize>()
            && slot < 10
        {
            let (body_start, body_end) = entry_body_bounds(entry, data.len())?;
            let Some(body_offset) = slot_steam_id_offset(&data[body_start..body_end]) else {
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
    let source_ids = source_steam_ids(data, &entries, steam_id)?;
    let mut report = SteamIdNormalizeReport::default();
    for entry in &entries {
        if let Some(slot_digits) = entry.name.strip_prefix("USER_DATA")
            && let Ok(slot) = slot_digits.parse::<usize>()
            && slot < 10
        {
            let (body_start, body_end) = entry_body_bounds(entry, data.len())?;
            if slot_steam_id_offset(&data[body_start..body_end]).is_some() {
                report.character_slots_seen += 1;
            }
            let mut replacements = 0;
            for old_id in &source_ids {
                replacements += replace_steam_id_occurrences(
                    &mut data[body_start..body_end],
                    *old_id,
                    steam_id,
                );
            }
            if replacements != 0 {
                report.character_slots_patched += 1;
                rewrite_entry_md5(data, entry)?;
                report.md5_rewritten += 1;
            } else if source_ids.is_empty()
                && let Some(slot_offset) = slot_steam_id_offset(&data[body_start..body_end])
                && patch_u64_le(data, body_start + slot_offset, steam_id)?
            {
                report.character_slots_patched += 1;
                rewrite_entry_md5(data, entry)?;
                report.md5_rewritten += 1;
            }
            continue;
        }
        if entry.name == "USER_DATA010" {
            let (body_start, body_end) = entry_body_bounds(entry, data.len())?;
            if body_start + USER_DATA10_STEAM_ID_BODY_OFFSET + STEAM_ID64_LEN <= body_end {
                report.user_data10_seen = true;
                let mut replacements = 0;
                for old_id in &source_ids {
                    replacements += replace_steam_id_occurrences(
                        &mut data[body_start..body_end],
                        *old_id,
                        steam_id,
                    );
                }
                // `||` short-circuits, so the fixed-offset patch is still attempted only when the
                // occurrence scan replaced nothing and there was no source id to scan for.
                if replacements != 0
                    || (source_ids.is_empty()
                        && patch_u64_le(
                            data,
                            body_start + USER_DATA10_STEAM_ID_BODY_OFFSET,
                            steam_id,
                        )?)
                {
                    report.user_data10_patched = true;
                    rewrite_entry_md5(data, entry)?;
                    report.md5_rewritten += 1;
                }
            }
        }
    }
    Ok(report)
}

/// MD5 of `input` (16-byte digest). Used for BND4 save-entry checksums, and re-exported for the DLL's
/// self-identifying log header (hashing its own on-disk image so a log names the exact build).
pub fn md5_digest(input: &[u8]) -> [u8; 16] {
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

    for chunk in msg.as_chunks::<64>().0 {
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

    /// Root of the local save corpus. Game-derived bytes are never versioned, so every test that
    /// reads one SKIPS when the root is absent. Env-overridable (`ER_SAVE_CORPUS_ROOT`) so the
    /// suite is not pinned to one machine's layout; defaults to the repo's `save-files/`.
    fn corpus_root() -> std::path::PathBuf {
        std::env::var_os("ER_SAVE_CORPUS_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../save-files")
            })
    }

    fn load_corpus_save(relative: &str) -> Option<Vec<u8>> {
        std::fs::read(corpus_root().join(relative)).ok()
    }

    fn load_fixture() -> Option<Vec<u8>> {
        load_corpus_save("45-Slots/ER0000.sl2")
    }

    fn load_150_banon_fixture() -> Option<Vec<u8>> {
        load_corpus_save("150-Banon/ER0000.sl2")
    }

    /// Every `.sl2`/`.co2` under the corpus root, recursively. Empty when the root is absent.
    fn corpus_containers() -> Vec<std::path::PathBuf> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("sl2") || e.eq_ignore_ascii_case("co2"))
                {
                    out.push(path);
                }
            }
        }
        let mut out = Vec::new();
        walk(&corpus_root(), &mut out);
        out.sort();
        out
    }

    fn count_bytes(haystack: &[u8], needle: &[u8]) -> usize {
        haystack
            .windows(needle.len())
            .filter(|w| *w == needle)
            .count()
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
    fn reads_user_data10_active_slot_bitmap() {
        let Some(data) = load_150_banon_fixture() else {
            eprintln!("150-Banon fixture missing; skipping");
            return;
        };
        let slots = active_slots(&data).expect("active slots");
        assert_eq!(
            slots,
            [
                true, false, false, false, false, false, false, false, false, false
            ]
        );
        assert!(has_active_slot(&data).expect("has active slot"));
    }

    #[test]
    fn parses_active_character_slot_info() {
        let Some(data) = load_150_banon_fixture() else {
            eprintln!("150-Banon fixture missing; skipping");
            return;
        };
        let slots = active_character_slots(&data).expect("active character slots");
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].slot, 0);
        assert!(!slots[0].name.trim().is_empty());
        assert!(slots[0].level >= 1);
    }

    #[test]
    fn detects_save_with_no_active_slots() {
        let Some(mut data) = load_150_banon_fixture() else {
            eprintln!("150-Banon fixture missing; skipping");
            return;
        };
        let entry = parse_entries(&data)
            .expect("entries")
            .into_iter()
            .find(|entry| entry.name == USER_DATA010_NAME)
            .expect("USER_DATA010");
        let body_start = entry.data_offset + ENTRY_MD5_LEN;
        let menu_len = rd_u32(&data, body_start + USER_DATA010_MENU_SAVE_LOAD_LEN_OFF)
            .expect("menu save-load len") as usize;
        let active_off = body_start + USER_DATA010_MENU_SAVE_LOAD_DATA_AFTER_LEN_OFF + menu_len;
        data[active_off..active_off + SAVE_SLOT_COUNT].fill(0);
        assert_eq!(
            active_slots(&data).expect("active slots"),
            [false; SAVE_SLOT_COUNT]
        );
        assert!(!has_active_slot(&data).expect("has active slot"));
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
    fn normalizes_every_source_steam_id_occurrence_in_downloads_save_when_present() {
        let Some(mut data) = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .map(|home| home.join("Downloads/ER0000.sl2"))
            .and_then(|path| std::fs::read(path).ok())
        else {
            eprintln!("~/Downloads/ER0000.sl2 missing; skipping");
            return;
        };
        let source = 76_561_198_055_502_948u64;
        let target = 76_561_197_986_456_766u64;
        // ~/Downloads is a transient location: this test's fixture is one SPECIFIC
        // foreign-SteamID save (source id embedded exactly 12 times). Any other save
        // landing at that path (observed 2026-07-29: a save already owned by the
        // target id) is "fixture absent", not a failure -- skip like the other
        // corpus-gated tests instead of asserting another file's identity.
        if count_bytes(&data, &source.to_le_bytes()) != 12 {
            eprintln!("~/Downloads/ER0000.sl2 is not the known source-id fixture; skipping");
            return;
        }
        let report = normalize_steam_id_in_place(&mut data, target).expect("normalize");
        assert_eq!(report.character_slots_patched, 10);
        assert!(report.user_data10_patched);
        assert_eq!(report.md5_rewritten, 11);
        assert_eq!(count_bytes(&data, &source.to_le_bytes()), 0);
        assert_eq!(count_bytes(&data, &target.to_le_bytes()), 12);
        for entry in parse_entries(&data).expect("entries") {
            let (body_start, body_end) = entry_body_bounds(&entry, data.len()).expect("body");
            assert_eq!(
                &data[entry.data_offset..entry.data_offset + ENTRY_MD5_LEN],
                &md5_digest(&data[body_start..body_end]),
                "{} md5",
                entry.name
            );
        }
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
        // ...and that literal body+4 IS what the accessor returns. This is the anti-drift pin for
        // `SLOT_BODY_MAP_OFFSET`: the DLL's `SAVE_SLOT_MAP_OFFSET` / `SerializedSaveSlot::saved_map`
        // are re-exports of this constant and this function, so a silent return to the old `0x14`
        // (which read a `CSRandXorshift` dword and wrote it into the live ProfileSummary record)
        // fails right here.
        assert_eq!(SLOT_BODY_MAP_OFFSET, 0x04);
        assert_eq!(
            slot_saved_map(body),
            Some(c30 as i32),
            "slot_saved_map must return the body+0x04 dword"
        );
        // plaintext sanity: a real decrypted body is structured (lots of zeros),
        // not high-entropy ciphertext.
        let zeros = body[..0x10000].iter().filter(|&&b| b == 0).count();
        assert!(zeros > 0x10000 / 4, "body should be structured plaintext");
    }

    /// Corpus-wide shape check for [`SLOT_BODY_MAP_OFFSET`]. A packed `BlockId` is
    /// `{indexId, regionId, blockId, areaId}` little-endian, so the areaId is the HIGH byte; every
    /// real ER area id observed across the corpus sits in `0x0a..=0x3d`. The word at the OLD
    /// offset (body+0x14) is one of four `CSRandXorshift::NextInt` dwords, so it clears that
    /// range only by chance. Requiring body+0x04 to pass on every active slot while body+0x14
    /// passes on a small minority is a structural, machine-checkable statement of which offset
    /// holds the map — no game bytes are versioned, and the whole test skips without the corpus.
    #[test]
    fn every_corpus_slot_map_is_a_plausible_block_id_at_body_04() {
        let containers = corpus_containers();
        if containers.is_empty() {
            eprintln!("save corpus absent (set ER_SAVE_CORPUS_ROOT); skipping");
            return;
        }
        const AREA_MIN: u8 = 0x0a;
        const AREA_MAX: u8 = 0x3d;
        let area_of = |v: i32| ((v as u32) >> 24) as u8;
        let mut active = 0usize;
        let mut noise_offset_plausible = 0usize;
        for path in &containers {
            let Ok(data) = std::fs::read(path) else {
                continue;
            };
            let Ok(active_flags) = active_slots(&data) else {
                continue;
            };
            for (slot, occupied) in active_flags.iter().enumerate() {
                if !occupied {
                    continue;
                }
                let Ok(body) = slot_body(&data, slot) else {
                    continue;
                };
                let map = slot_saved_map(body).expect("slot body holds a header");
                active += 1;
                assert!(
                    (AREA_MIN..=AREA_MAX).contains(&area_of(map)),
                    "{}: slot {slot} map 0x{map:08x} has areaId 0x{:02x} outside 0x{AREA_MIN:02x}..=0x{AREA_MAX:02x} -- body+0x{SLOT_BODY_MAP_OFFSET:02x} is not the BlockId",
                    path.display(),
                    area_of(map),
                );
                let noise = i32::from_le_bytes(body[0x14..0x18].try_into().expect("4 bytes"));
                if (AREA_MIN..=AREA_MAX).contains(&area_of(noise)) {
                    noise_offset_plausible += 1;
                }
            }
        }
        assert!(active > 0, "corpus present but held no active slots");
        // The old offset must be visibly WORSE, not merely different: if body+0x14 ever passed as
        // often as body+0x04 this discriminator would be worthless and the pin above would be the
        // only guard left.
        assert!(
            noise_offset_plausible * 4 < active,
            "body+0x14 looked like a BlockId on {noise_offset_plausible}/{active} slots -- \
             that offset is supposed to be CSRandXorshift noise"
        );
    }
}
