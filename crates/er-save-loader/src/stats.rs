//! Per-slot character attribute extraction from a plaintext ER `.sl2`.
//!
//! The ProfileSelect / Load-Game screen shows only a per-slot summary
//! (name/level/map/playtime); the eight attributes exist in no live struct until
//! a slot is actually loaded. To render them per row we read them straight out of
//! the plaintext save slot body (see [`crate::bnd4`]).
//!
//! **The `PlayerGameData` is not at a fixed offset in the slot body** — the
//! variable-length data that precedes it (event flags, inventory, ...) shifts it
//! per slot (offsets from 0xe0b6..0xe8a4 observed across real saves). We locate it
//! by the exact Elden Ring identity **`RuneLevel == (sum of the 8 attributes) −
//! 79`**, which holds for every class and level. All offsets below are relative to
//! the located `PlayerGameData` and were verified against real saves 2026-07-04.

use crate::bnd4;

/// `PlayerGameData` field offsets (relative to the located struct base). These
/// mirror the DLL's live-PGD offsets (`PGD_LEVEL_68_OFFSET`,
/// `PGD_STAT_BASE_3C_OFFSET`, `PGD_NAME_9C_OFFSET`) — the serialized save body
/// uses the same layout.
const PGD_LEVEL: usize = 0x68;
const PGD_STAT_BASE: usize = 0x3c;
/// Level offset measured from the stat block base (`0x68 - 0x3c`); the invariant
/// check reads it without knowing the absolute PGD base.
const LEVEL_FROM_STAT_BASE: usize = PGD_LEVEL - PGD_STAT_BASE;

/// Number of attributes: Vigor, Mind, Endurance, Strength, Dexterity,
/// Intelligence, Faith, Arcane.
pub const STAT_COUNT: usize = 8;

/// Elden Ring identity: a Rune Level `N` character's eight attributes sum to
/// `N + 79` (the eight class-start attributes always sum to 80 at RL1).
const RUNE_LEVEL_BASE: i32 = 79;
const MIN_ATTR: i32 = 1;
const MAX_ATTR: i32 = 99;
/// RL cap (all eight attributes at 99: `8*99 - 79 = 713`).
const MAX_RUNE_LEVEL: i32 = 713;

/// One slot's decoded stat line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlotStats {
    /// Rune Level.
    pub level: i32,
    /// The eight attributes in struct order (VIG, MND, END, STR, DEX, INT, FAI, ARC).
    pub attributes: [i32; STAT_COUNT],
}

fn rd_i32(b: &[u8], off: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

/// Read the eight attributes at a candidate stat-block base and validate them
/// against the Rune Level invariant. `stat_base` is the offset of the first
/// attribute (`PlayerGameData + 0x3c`).
fn stat_block_at(body: &[u8], stat_base: usize) -> Option<SlotStats> {
    let mut attributes = [0i32; STAT_COUNT];
    let mut sum = 0i32;
    for (i, slot) in attributes.iter_mut().enumerate() {
        let v = rd_i32(body, stat_base + i * 4)?;
        if !(MIN_ATTR..=MAX_ATTR).contains(&v) {
            return None;
        }
        sum += v;
        *slot = v;
    }
    let level = rd_i32(body, stat_base + LEVEL_FROM_STAT_BASE)?;
    if level != sum - RUNE_LEVEL_BASE || !(MIN_ATTR..=MAX_RUNE_LEVEL).contains(&level) {
        return None;
    }
    Some(SlotStats { level, attributes })
}

/// Locate the `PlayerGameData` stat block in a slot body and return the level +
/// eight attributes. Scans for the first offset satisfying the Rune Level
/// invariant. Returns `None` for an empty slot (no character) or a body that does
/// not match.
#[must_use]
pub fn slot_stats_from_body(body: &[u8]) -> Option<SlotStats> {
    let last = body.len().checked_sub(PGD_STAT_BASE)?;
    // The stat block is not guaranteed 4-aligned within the body (observed both
    // 0- and 2-aligned), so step by bytes. The invariant (eight in-range attrs
    // whose sum-79 equals the level word) is strong enough that the first match
    // is the real PGD; empty slots yield none.
    for base in 0..last {
        if let Some(stats) = stat_block_at(body, base) {
            return Some(stats);
        }
    }
    None
}

/// Convenience: parse a whole `.sl2`, returning each slot's stats (`None` for
/// empty / non-matching slots).
#[must_use]
pub fn all_slot_stats(sl2: &[u8]) -> [Option<SlotStats>; 10] {
    let mut out = [None; 10];
    for (slot, entry) in out.iter_mut().enumerate() {
        if let Ok(body) = bnd4::slot_body(sl2, slot) {
            *entry = slot_stats_from_body(body);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rel: &str) -> Option<Vec<u8>> {
        std::fs::read(format!(
            "{}/../../save-files/{rel}/ER0000.sl2",
            env!("CARGO_MANIFEST_DIR")
        ))
        .ok()
    }

    #[test]
    fn extracts_known_slot_stats_and_upholds_invariant() {
        let Some(data) = fixture("9-Menace") else {
            eprintln!("fixture missing; skipping");
            return;
        };
        let stats = all_slot_stats(&data);
        // Slot 0 of 9-Menace is the level-9 "Menace" character (verified offline).
        let s0 = stats[0].expect("slot 0 has a character");
        assert_eq!(s0.level, 9);
        assert_eq!(s0.attributes, [15, 10, 11, 14, 13, 9, 9, 7]);
        // Every decoded slot must satisfy the Rune Level identity.
        for slot in stats.into_iter().flatten() {
            assert_eq!(
                slot.level,
                slot.attributes.iter().sum::<i32>() - RUNE_LEVEL_BASE,
                "Rune Level invariant must hold for a decoded slot"
            );
        }
    }

    #[test]
    fn distinct_characters_decode_distinctly() {
        // A save with distinct characters must decode DIFFERENT per-slot stats —
        // the whole point of the per-slot read (vs pushing the loaded char to all).
        let Some(data) = fixture("45-Slots") else {
            eprintln!("fixture missing; skipping");
            return;
        };
        let stats = all_slot_stats(&data);
        // Slot 2 is a level-45 Vagabond; slot 9 is a level-6 Astro (verified offline).
        let s2 = stats[2].expect("slot 2 char");
        let s9 = stats[9].expect("slot 9 char");
        assert_eq!(s2.level, 45);
        assert_eq!(s9.level, 6);
        assert_ne!(
            s2.attributes, s9.attributes,
            "distinct characters must not decode to identical attributes"
        );
    }

    #[test]
    fn rejects_body_without_a_stat_block() {
        // A body of all-0xff (no in-range attribute octet) has no match.
        assert_eq!(slot_stats_from_body(&[0xffu8; 0x1000]), None);
        assert_eq!(slot_stats_from_body(&[]), None);
    }
}
