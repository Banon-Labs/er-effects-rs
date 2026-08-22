//! The per-slot **profile summary** the game stores in `USER_DATA010`.
//!
//! This is the table Elden Ring deserializes into `CS::ProfileSummary` at boot, and it is what the
//! Load Character rows are drawn from. Reading it is how a save's slots can be described without
//! touching the ~26 MB of character bodies: `USER_DATA010`'s body is `0x60000` bytes at a known BND4
//! entry offset, and the whole ten-slot table lives in its first `0x1800`.
//!
//! # Why this exists
//!
//! The mod's save-file picker previews a foreign save by rebuilding each `CS::ProfileSummary` record
//! field by field out of `PlayerGameData`. That reconstruction has no source for the row's LOCATION:
//! the game fills that field from `CSFeMan` (where the player currently is) when it writes its own
//! summary, so it cannot be recovered from a character body. The result was a save swap where every
//! row field updated except the place name, which kept describing the previous save.
//!
//! It turns out not to need deriving at all -- the id is right here in the file.
//!
//! # When the file's own table is wrong
//!
//! A save ASSEMBLED OUTSIDE the game (a manager copying `USER_DATA00N` bodies between files) keeps
//! the donor's `USER_DATA010`, so a slot's record can describe whoever used to hold it.
//! `save-files/100-Lilbro` carries `45-Slots`' table: seven of its ten records name a different
//! character than the body beside them, which is how the Load Game rows came to print another
//! character's place (user-reported 2026-08-07). [`record_describes_slot_body`] detects it by the one
//! field both sides carry -- the map -- and [`place_name_by_block`] answers it from the records the
//! game DID write in the same file.
//!
//! # Layout, and the two bytes that hid it
//!
//! Relative to a record base, the on-disk layout is the same as the in-memory `CS::ProfileSummary`
//! record, EXCEPT that the name sits at `+0x02` rather than `+0x00`. Anchoring on the name (the
//! obvious thing to do, since it is the only human-readable field) puts every subsequent offset two
//! bytes early, which reads `level` and `play_time` correctly by luck -- both survive a two-byte
//! shift as plausible numbers -- while turning the place-name id into `0xffff0000`. That near-miss
//! is what made the field look absent.
//!
//! The anchor that settles it is `play_time`: for `save-files/125-Frenzy` slot 0 it reads 388174
//! seconds = 107:49:34, the exact play time the game rendered for that character.
//!
//! ```text
//! +0x00  u16    unidentified
//! +0x02  0x22   character name, UTF-16LE, NUL-padded
//! +0x24  u32    level
//! +0x28  u32    play time, SECONDS
//! +0x2c  u32    rune memory
//! +0x30  u32    BlockId (map)
//! +0x34  u32    PlaceName message id  <- the row's Location
//! +0x3c  "FACE" face-data block
//! ```
//!
//! `PlaceName` ids land in the base-game 10000..68999 band or the DLC 6.0M..7.0M band, and are a
//! function of the map: `0x0e000000 -> 14000`, `0x1c000000 -> 28000`, `0x3c332700 -> 6411800`.
//! `CS::MenuSaveDataSummary` renders it through `MsgRepository::GetAndFormat(id, "PlaceName", "PN")`.

use crate::bnd4::{Bnd4Error, SAVE_SLOT_COUNT, active_slots, entry_body};

/// BND4 entry holding the menu/system data, including this table.
const USER_DATA010_NAME: &str = "USER_DATA010";
/// Body offset of slot 0's record. Fixed across every save in the corpus test below.
pub const SUMMARY_TABLE_OFFSET: usize = 0x195c;
/// Distance between consecutive slot records. Note this is NOT the in-memory `0x2a0`: the on-disk
/// record carries an embedded `FACE` block and packs to `0x24c`.
pub const SUMMARY_RECORD_STRIDE: usize = 0x24c;

/// Record-relative field offsets. The name starts at `+0x02`, not `+0x00` -- see the module docs.
pub const REC_NAME_OFFSET: usize = 0x02;
/// Bytes of UTF-16 name storage (16 code units plus a terminator).
pub const REC_NAME_BYTES: usize = 0x22;
pub const REC_LEVEL_OFFSET: usize = 0x24;
pub const REC_PLAY_TIME_OFFSET: usize = 0x28;
pub const REC_RUNE_MEMORY_OFFSET: usize = 0x2c;
pub const REC_BLOCK_ID_OFFSET: usize = 0x30;
pub const REC_PLACE_NAME_OFFSET: usize = 0x34;

/// Highest level the game will accept, reused as the record-plausibility test: an unoccupied or
/// mis-anchored record fails it rather than being reported as a character.
const MAX_LEVEL: u32 = 713;

/// One slot's stored summary -- everything a Load Character row shows, without reading a body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotSummary {
    pub slot: usize,
    pub name: String,
    pub level: u32,
    pub play_time_seconds: u32,
    pub rune_memory: u32,
    pub block_id: u32,
    /// `PlaceName` message id -- what the row's Location field is formatted from.
    pub place_name_id: u32,
}

fn rd_u32(body: &[u8], at: usize) -> Option<u32> {
    body.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn rd_name(body: &[u8], at: usize) -> Option<String> {
    let raw = body.get(at..at + REC_NAME_BYTES)?;
    let units: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|u| *u != 0)
        .collect();
    // Lossless: a name that is not valid UTF-16 is not a name we should report.
    String::from_utf16(&units).ok()
}

/// Read one slot's record, or `None` when it does not hold a plausible character.
pub fn slot_summary_from_body(body: &[u8], slot: usize) -> Option<SlotSummary> {
    if slot >= SAVE_SLOT_COUNT {
        return None;
    }
    let base = SUMMARY_TABLE_OFFSET + slot * SUMMARY_RECORD_STRIDE;
    let level = rd_u32(body, base + REC_LEVEL_OFFSET)?;
    if !(1..=MAX_LEVEL).contains(&level) {
        return None;
    }
    Some(SlotSummary {
        slot,
        name: rd_name(body, base + REC_NAME_OFFSET)?,
        level,
        play_time_seconds: rd_u32(body, base + REC_PLAY_TIME_OFFSET)?,
        rune_memory: rd_u32(body, base + REC_RUNE_MEMORY_OFFSET)?,
        block_id: rd_u32(body, base + REC_BLOCK_ID_OFFSET)?,
        place_name_id: rd_u32(body, base + REC_PLACE_NAME_OFFSET)?,
    })
}

/// Every slot's stored summary, filtered by `USER_DATA010`'s own occupancy bitmap.
///
/// The bitmap is the authority on which slots exist: a deleted character can leave a readable
/// record behind, and reporting it would put a ghost on the Load Character screen.
pub fn slot_summaries(save: &[u8]) -> Result<[Option<SlotSummary>; SAVE_SLOT_COUNT], Bnd4Error> {
    let body = entry_body(save, USER_DATA010_NAME)?;
    let active = active_slots(save)?;
    let mut out: [Option<SlotSummary>; SAVE_SLOT_COUNT] = Default::default();
    for (slot, entry) in out.iter_mut().enumerate() {
        if active[slot] {
            *entry = slot_summary_from_body(body, slot);
        }
    }
    Ok(out)
}

/// Does slot `slot`'s stored summary record describe the character body in that same slot?
///
/// The game writes the two together, so in a save the game produced they always agree. A save
/// ASSEMBLED OUTSIDE the game does not: a manager that copies `USER_DATA00N` bodies between files
/// leaves `USER_DATA010` behind, and every field of that slot's record -- name, level, play time and
/// the `PlaceName` -- then describes whoever used to occupy the slot. `save-files/100-Lilbro`
/// (2026-08-07) is exactly this: its summary table is `45-Slots`', and seven of its ten records name a
/// different character than the body beside them.
///
/// The test is the map. `block_id` is in both places -- the record's `+0x30` and the body's own saved
/// map -- so the two can be compared without decoding either character. It is a necessary condition,
/// not a proof of identity (two characters saved in the same area agree on it), which is the right
/// shape here: it never rejects a record the game itself wrote, and it catches the copied-table case
/// whenever the characters are anywhere different.
pub fn record_describes_slot_body(save: &[u8], slot: usize) -> bool {
    let Ok(u010) = entry_body(save, USER_DATA010_NAME) else {
        return false;
    };
    let Some(record) = slot_summary_from_body(u010, slot) else {
        return false;
    };
    let Ok(body) = crate::bnd4::slot_body(save, slot) else {
        return false;
    };
    crate::bnd4::slot_saved_map(body) == Some(record.block_id as i32)
}

/// What the save itself knows about which `PlaceName` belongs to which map: `block_id ->
/// place_name_id`, learned ONLY from records that describe the body beside them.
///
/// The game writes a record's map and place name together, out of one character's state, so a
/// self-consistent record is a pair the GAME asserted -- not an inference of ours. Ten slots
/// therefore carry up to ten worked examples, and a slot whose record was copied in from another
/// save can usually be answered by a sibling that was not: in `save-files/100-Lilbro` the consistent
/// records supply `0x1c000000 -> 28000` and `0x0e000000 -> 14000`, and all seven stale slots have
/// body map `0x0e000000`.
///
/// This is the only source there is. The id is in no character body (searched the corpus 2026-08-07:
/// `100-Lilbro`'s slot 0 body contains its own place id zero times), and the game has no
/// block-to-place-name function to call -- the area-name popup id is pushed in by EMEVD event
/// scripts (`SetSubareaNamePopupMessageId`'s only callers are `Cutscene2002Handler`/`Event2003`),
/// never computed from a map.
pub fn place_name_by_block(save: &[u8]) -> std::collections::BTreeMap<u32, u32> {
    let mut out = std::collections::BTreeMap::new();
    let Ok(u010) = entry_body(save, USER_DATA010_NAME) else {
        return out;
    };
    for slot in 0..SAVE_SLOT_COUNT {
        if !record_describes_slot_body(save, slot) {
            continue;
        }
        if let Some(record) = slot_summary_from_body(u010, slot)
            && record.place_name_id != 0
        {
            out.insert(record.block_id, record.place_name_id);
        }
    }
    out
}

/// The best-evidenced `PlaceName` id for the character in `slot` -- the field a foreign-save preview
/// has no other source for, and the one a copied summary table gets wrong.
///
/// Two tiers, both grounded in something the game itself wrote:
///  1. the slot's own record, when it describes the body in that slot;
///  2. otherwise the id this save pairs with that body's map elsewhere ([`place_name_by_block`]).
///
/// `None` when neither answers -- a map no consistent record covers. Blank is the right output there:
/// the alternative is printing the place of whoever used to occupy the slot, which is the defect this
/// exists to remove.
pub fn slot_place_name_id(save: &[u8], slot: usize) -> Option<u32> {
    if record_describes_slot_body(save, slot) {
        let body = entry_body(save, USER_DATA010_NAME).ok()?;
        return slot_summary_from_body(body, slot).map(|s| s.place_name_id);
    }
    let map = crate::bnd4::slot_body(save, slot)
        .ok()
        .and_then(crate::bnd4::slot_saved_map)?;
    place_name_by_block(save).get(&(map as u32)).copied()
}

/// Every slot's best-evidenced `PlaceName` id in one pass, so a caller with the bytes in hand pays
/// for [`place_name_by_block`] once instead of once per slot.
pub fn slot_place_name_ids(save: &[u8]) -> [Option<u32>; SAVE_SLOT_COUNT] {
    let by_block = place_name_by_block(save);
    let u010 = entry_body(save, USER_DATA010_NAME).ok();
    core::array::from_fn(|slot| {
        if record_describes_slot_body(save, slot) {
            return u010
                .and_then(|body| slot_summary_from_body(body, slot).map(|s| s.place_name_id));
        }
        let map = crate::bnd4::slot_body(save, slot)
            .ok()
            .and_then(crate::bnd4::slot_saved_map)?;
        by_block.get(&(map as u32)).copied()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Env-overridable so the suite is not pinned to one machine's layout (mirrors `bnd4`'s own
    /// corpus discovery, which is private to its test module).
    fn corpus_root() -> std::path::PathBuf {
        std::env::var_os("ER_SAVE_CORPUS_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../save-files")
            })
    }

    /// Every `ER0000.sl2`/`.co2` one directory under the corpus root.
    fn corpus_saves(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let Ok(dirs) = std::fs::read_dir(root) else {
            return out;
        };
        for dir in dirs.flatten() {
            for name in ["ER0000.sl2", "ER0000.co2"] {
                let path = dir.path().join(name);
                if path.is_file() {
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }

    /// Corpus gate for the table geometry AND for the field this whole module exists to supply.
    ///
    /// Pins `SUMMARY_TABLE_OFFSET`/`SUMMARY_RECORD_STRIDE` against every save present, so a wrong
    /// base cannot come back: the failure mode it guards is not a crash but a two-byte drift that
    /// still yields believable levels and play times.
    #[test]
    fn reads_every_corpus_save_slot_summary() {
        let root = corpus_root();
        if !root.is_dir() {
            eprintln!("SKIP: no local save corpus at {}", root.display());
            return;
        }
        let saves = corpus_saves(&root);
        if saves.is_empty() {
            eprintln!("SKIP: corpus at {} holds no saves", root.display());
            return;
        }
        let mut files = 0usize;
        let mut characters = 0usize;
        for path in saves {
            let Ok(data) = std::fs::read(&path) else {
                continue;
            };
            let Ok(summaries) = slot_summaries(&data) else {
                continue;
            };
            files += 1;
            for summary in summaries.iter().flatten() {
                characters += 1;
                assert!(
                    (1..=MAX_LEVEL).contains(&summary.level),
                    "{}: slot {} level {} out of range",
                    path.display(),
                    summary.slot,
                    summary.level
                );
                // A PlaceName id, not a map id and not a shifted read. The bands are the base
                // game (10000..=68999) and the DLC (6_000_000..=6_999_999 -- observed 6411800,
                // 6415400 and 6513400, so it is wider than any one save suggests); zero means the
                // slot has no recorded location, which a fresh character can legitimately have.
                let id = summary.place_name_id;
                assert!(
                    id == 0
                        || (10_000..=68_999).contains(&id)
                        || (6_000_000..=6_999_999).contains(&id),
                    "{}: slot {} place_name_id {id} is outside every known PlaceName band -- the \
                     record base has probably drifted (name reads {:?}, block 0x{:08x})",
                    path.display(),
                    summary.slot,
                    summary.name,
                    summary.block_id
                );
            }
        }
        assert!(files > 0, "corpus present but no save parsed");
        assert!(
            characters > 0,
            "corpus present but no character record read"
        );
        eprintln!("profile summary: {characters} character record(s) across {files} save(s)");
    }

    /// The copied-summary-table case, pinned on the save that exposed it.
    ///
    /// `100-Lilbro` carries `45-Slots`' `USER_DATA010`, so six of its ten records describe the wrong
    /// character. Slot 1 is the sharpest: the body is `Dark Moon Bean` (RL 90) saved in map
    /// `0x0e000000`, while the record says `Hero` (RL 7) in `0x1c000000` -- and it is that record's
    /// place name the Load Game row would print under Dark Moon Bean's name.
    #[test]
    fn a_record_copied_from_another_save_supplies_no_place_name() {
        let path = std::path::Path::new("../../save-files/100-Lilbro/ER0000.sl2");
        let Ok(data) = std::fs::read(path) else {
            eprintln!("SKIP: {} not present", path.display());
            return;
        };
        // Slot 0's record was rewritten by the game and agrees with its body.
        assert!(record_describes_slot_body(&data, 0));
        assert_eq!(slot_place_name_id(&data, 0), Some(28_000));
        // Slot 1's was not -- and it is still answerable, because a SIBLING slot whose record the
        // game did write pairs this body's map with a place name. Dark Moon Bean is saved in
        // `0x0e000000`, slots 2 and 7 are consistent records in that same map, and both say 14000.
        // The stale record's own 28000 (the `Hero` it describes) never reaches the row.
        assert!(!record_describes_slot_body(&data, 1));
        assert_eq!(slot_place_name_id(&data, 1), Some(14_000));
        assert_eq!(
            place_name_by_block(&data),
            [(0x1c00_0000_u32, 28_000_u32), (0x0e00_0000, 14_000)]
                .into_iter()
                .collect()
        );
        // Every slot resolves; blanks would mean a map no consistent record covers.
        let ids = slot_place_name_ids(&data);
        assert!(ids.iter().all(Option::is_some), "unresolved slots: {ids:?}");
        assert_eq!(ids[0], Some(28_000));
        for slot in [1usize, 3, 4, 5, 6, 8, 9] {
            assert_eq!(ids[slot], Some(14_000), "slot {slot}");
        }
        // The stale record is still READABLE -- refusing it is a judgement about whose it is, not a
        // parse failure, and a change that broke the read would otherwise pass this test.
        let u010 = entry_body(&data, USER_DATA010_NAME).expect("USER_DATA010");
        let record = slot_summary_from_body(u010, 1).expect("slot 1 record parses");
        assert_eq!(record.name, "Hero");
        assert_eq!(record.block_id, 0x1c00_0000);
        assert_eq!(
            crate::bnd4::slot_body(&data, 1)
                .ok()
                .and_then(crate::bnd4::slot_saved_map),
            Some(0x0e00_0000)
        );
    }

    /// The anchor that proved the layout, kept as a test so it cannot quietly stop being true.
    #[test]
    fn the_known_corpus_character_reads_back_exactly() {
        let path = std::path::Path::new("../../save-files/125-Frenzy/ER0000.co2");
        let Ok(data) = std::fs::read(path) else {
            eprintln!("SKIP: {} not present", path.display());
            return;
        };
        let summaries = slot_summaries(&data).expect("parse");
        let slot0 = summaries[0].as_ref().expect("slot 0 is occupied");
        assert_eq!(slot0.name, "Maddened Bean");
        assert_eq!(slot0.level, 125);
        // 388174 s = 107:49:34, the play time the game rendered for this character.
        assert_eq!(slot0.play_time_seconds, 388_174);
        assert_eq!(slot0.block_id, 0x1c00_0000);
        assert_eq!(slot0.place_name_id, 28_000);
    }
}
