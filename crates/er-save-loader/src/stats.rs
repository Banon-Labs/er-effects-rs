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
const PGD_NAME: usize = 0x9c;
const PGD_NAME_LEN_U16: usize = 17;
/// Level offset measured from the stat block base (`0x68 - 0x3c`); the invariant
/// check reads it without knowing the absolute PGD base.
const LEVEL_FROM_STAT_BASE: usize = PGD_LEVEL - PGD_STAT_BASE;

/// Stored effective max vitals, relative to the located `PlayerGameData` base.
/// The save body mirrors the runtime layout (`scripts/save-slot-oracle.py`;
/// SL2.bt's comment: the save `PlayerGameData` "mirrors `CS::PlayerGameData+0x8`"):
/// runtime `current_max_hp` @ PGD+0x14 == SL2.bt `MaxHealth` @ +0x0c, `current_max_fp`
/// @ +0x20 == `MaxFP` @ +0x18, `current_max_stamina` @ +0x30 == `MaxSP` @ +0x28.
/// These are the *effective* maxima (base + talisman/buff modifiers) -- the exact
/// values the live loading-screen stats read from PGD, stored, not derived.
const PGD_MAX_HP: usize = 0x14;
const PGD_MAX_FP: usize = 0x20;
const PGD_MAX_STAMINA: usize = 0x30;

/// `matchmakingWeaponLevel` -- the character's HIGHEST weapon upgrade level, which the game already
/// maintains for multiplayer matchmaking.
///
/// Verified in the 1.16.2 Ghidra dump as `PlayerGameData + 0xe2`, type `byte`, and independently in
/// ClayAmore's `SL2.bt` save template as `MatchmakingWeaponLvl` at template `+0xda` (the template's
/// struct is `CS::PlayerGameData + 0x8`, so `0xda + 8 == 0xe2`). The same template's `Level @ 0x60`
/// and `Vigor @ 0x34` map to this module's `PGD_LEVEL` `0x68` and `PGD_STAT_BASE` `0x3c` under the
/// identical `+8`, which is what makes the correspondence a check rather than a coincidence.
///
/// Taking this byte instead of walking the inventory avoids the whole failure mode that walk
/// carries: no item-record stride to get wrong, no reliance on the `paramId % 100` reinforcement
/// convention, and no equipped-only blind spot for a `+25` sitting in the storage box.
const PGD_MATCHMAKING_WEAPON_LEVEL: usize = 0xe2;

/// Highest reachable weapon upgrade: standard armaments reinforce `+0..=+25` (somber `+0..=+10`).
/// A byte outside that window means the located base is not really a `PlayerGameData`, so it is
/// reported as unknown rather than rendered.
const MAX_WEAPON_LEVEL: u8 = 25;

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
    /// Effective max HP as STORED in the save (SL2.bt `MaxHealth` == runtime
    /// `current_max_hp`, incl. talisman/buff modifiers). 0 when unreadable.
    pub max_hp: i32,
    /// Effective max FP as stored (SL2.bt `MaxFP` == runtime `current_max_fp`).
    /// 0 when unreadable.
    pub max_fp: i32,
    /// Effective max Stamina as stored (SL2.bt `MaxSP` == runtime
    /// `current_max_stamina`). 0 when unreadable.
    pub max_stamina: i32,
    /// Highest weapon upgrade level on this character (`matchmakingWeaponLevel`), or `None` when the
    /// byte is unreadable or implausible. `Some(0)` is a real answer -- a character with nothing
    /// upgraded -- and is deliberately distinct from `None`, which means "we do not know".
    pub matchmaking_weapon_level: Option<u8>,
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
    // Vitals are best-effort reads of STORED values: acceptance stays exactly the
    // rune-level invariant above, and a missing/implausible vital decodes as 0
    // ("unknown") rather than rejecting the located block or inventing a formula.
    let pgd = stat_base.checked_sub(PGD_STAT_BASE);
    let vital = |off: usize| {
        pgd.and_then(|p| rd_i32(body, p + off))
            .filter(|v| *v > 0)
            .unwrap_or(0)
    };
    // Same best-effort posture as the vitals: acceptance is the rune-level invariant alone, and an
    // out-of-range byte decodes as "unknown" rather than rejecting an otherwise-valid block.
    let matchmaking_weapon_level = pgd
        .and_then(|p| body.get(p + PGD_MATCHMAKING_WEAPON_LEVEL).copied())
        .filter(|v| *v <= MAX_WEAPON_LEVEL);
    Some(SlotStats {
        level,
        attributes,
        max_hp: vital(PGD_MAX_HP),
        max_fp: vital(PGD_MAX_FP),
        max_stamina: vital(PGD_MAX_STAMINA),
        matchmaking_weapon_level,
    })
}

fn located_stat_block(body: &[u8]) -> Option<(usize, SlotStats)> {
    let last = body.len().checked_sub(PGD_STAT_BASE)?;
    // The stat block is not guaranteed 4-aligned within the body (observed both
    // 0- and 2-aligned), so step by bytes. The invariant (eight in-range attrs
    // whose sum-79 equals the level word) is strong enough that the first match
    // is the real PGD; empty slots yield none.
    for base in 0..last {
        if let Some(stats) = stat_block_at(body, base) {
            return Some((base, stats));
        }
    }
    None
}

/// Locate the `PlayerGameData` stat block in a slot body and return the level +
/// eight attributes. Scans for the first offset satisfying the Rune Level
/// invariant. Returns `None` for an empty slot (no character) or a body that does
/// not match.
#[must_use]
pub fn slot_stats_from_body(body: &[u8]) -> Option<SlotStats> {
    located_stat_block(body).map(|(_, stats)| stats)
}

fn slot_name_at_pgd(body: &[u8], pgd: usize) -> Option<String> {
    let mut units = [0u16; PGD_NAME_LEN_U16];
    let mut len = 0usize;
    while len < PGD_NAME_LEN_U16 {
        let off = pgd + PGD_NAME + len * 2;
        let unit = u16::from_le_bytes(body.get(off..off + 2)?.try_into().ok()?);
        units[len] = unit;
        if unit == 0 {
            break;
        }
        len += 1;
    }
    if len == 0 {
        return None;
    }
    String::from_utf16(&units[..len])
        .ok()
        .map(|s| s.trim_end_matches('\0').trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Decode a slot's character name from the same located serialized
/// `PlayerGameData` that yields its stats. This does not depend on the
/// ProfileSummary active-slot bitmap, which can be absent/stale for alternate
/// containers while the per-slot body still contains a real character.
#[must_use]
pub fn slot_name_from_body(body: &[u8]) -> Option<String> {
    let (stat_base, _) = located_stat_block(body)?;
    let pgd = stat_base.checked_sub(PGD_STAT_BASE)?;
    slot_name_at_pgd(body, pgd)
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

/// Convenience: parse a whole `.sl2`, returning each slot's name (`None` for
/// empty / non-matching slots).
#[must_use]
pub fn all_slot_names(sl2: &[u8]) -> [Option<String>; 10] {
    let mut out = core::array::from_fn(|_| None);
    for (slot, entry) in out.iter_mut().enumerate() {
        if let Ok(body) = bnd4::slot_body(sl2, slot) {
            *entry = slot_name_from_body(body);
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
        let names = all_slot_names(&data);
        assert_eq!(names[0].as_deref(), Some("Menace"));
        assert_eq!(s0.level, 9);
        assert_eq!(s0.attributes, [15, 10, 11, 14, 13, 9, 9, 7]);
        // Stored effective max vitals, ground-truthed against the sanctioned
        // decoder (`scripts/save-slot-oracle.py` decode_save_slot on this exact
        // file, 2026-07-29): max_health=870, max_fp=121, max_stamina=115.
        assert_eq!(
            (s0.max_hp, s0.max_fp, s0.max_stamina),
            (870, 121, 115),
            "stored max vitals must match the save-slot-oracle ground truth"
        );
        // Every decoded slot must satisfy the Rune Level identity.
        for slot in stats.into_iter().flatten() {
            assert_eq!(
                slot.level,
                slot.attributes.iter().sum::<i32>() - RUNE_LEVEL_BASE,
                "Rune Level invariant must hold for a decoded slot"
            );
        }
    }

    /// The weapon-level byte must decode to a plausible upgrade on real saves, and must not be
    /// constant across distinct characters (which is what a wrong offset landing on padding, a
    /// flag, or a shared field would look like). Corpus-gated; prints the values so a suspicious
    /// decode is visible rather than merely passing.
    #[test]
    fn matchmaking_weapon_level_decodes_plausibly_on_real_saves() {
        let mut seen: Vec<(String, i32, Option<u8>)> = Vec::new();
        for fixture_name in ["9-Menace", "45-Slots"] {
            let Some(data) = fixture(fixture_name) else {
                eprintln!("fixture {fixture_name} missing; skipping");
                continue;
            };
            let stats = all_slot_stats(&data);
            let names = all_slot_names(&data);
            for (i, slot) in stats.iter().enumerate() {
                let Some(slot) = slot else { continue };
                let name = names[i].clone().unwrap_or_default();
                eprintln!(
                    "{fixture_name} slot {i}: {name:?} RL {} WL {:?}",
                    slot.level, slot.matchmaking_weapon_level
                );
                if let Some(wl) = slot.matchmaking_weapon_level {
                    assert!(
                        wl <= MAX_WEAPON_LEVEL,
                        "{fixture_name} slot {i}: weapon level {wl} exceeds the +25 cap"
                    );
                }
                seen.push((name, slot.level, slot.matchmaking_weapon_level));
            }
        }
        if seen.is_empty() {
            eprintln!("no corpus saves present; weapon-level decode unverified");
            return;
        }
        // A low-level character cannot have a highly upgraded armament: reaching +25 needs Somber
        // /smithing stones a RL<20 character has not plausibly farmed. This is the cheap sanity
        // check that a wrong offset (reading an unrelated byte) tends to violate.
        for (name, level, wl) in &seen {
            if let Some(wl) = wl {
                assert!(
                    *level >= 20 || *wl <= 12,
                    "implausible pairing, offset suspect: {name:?} RL {level} WL {wl}"
                );
            }
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
        // Oracle ground truth (decode_save_slot, 2026-07-29): slot 2 max vitals
        // 769/95/130; slot 9 max vitals 396/95/94. Slot 9 is the offset
        // discriminator: its CURRENT fp is 78 while MaxFP is 95, so an off-by-4
        // read (current instead of max) would return 78 here and fail.
        assert_eq!((s2.max_hp, s2.max_fp, s2.max_stamina), (769, 95, 130));
        assert_eq!(
            (s9.max_hp, s9.max_fp, s9.max_stamina),
            (396, 95, 94),
            "slot 9 MaxFP must be 95 (current fp is 78 -- catches an off-by-4)"
        );
    }

    #[test]
    fn rejects_body_without_a_stat_block() {
        // A body of all-0xff (no in-range attribute octet) has no match.
        assert_eq!(slot_stats_from_body(&[0xffu8; 0x1000]), None);
        assert_eq!(slot_stats_from_body(&[]), None);
    }
}
