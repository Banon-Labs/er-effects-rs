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
//! by the Elden Ring identity **`RuneLevel == (sum of the 8 attributes) − 79`**,
//! which holds for every class and every level the game assigned. All offsets below
//! are relative to the located `PlayerGameData` and were verified against real saves
//! 2026-07-04.
//!
//! That identity is a locator, not a licence to exist: a stored `level` word can
//! disagree with its own attribute sum (a build importer or save editor that writes
//! one side without the other), and such a character must still decode. See
//! [`slot_stats_from_body`] for the measured case and the structural fallback.

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

/// `matchmakingWeaponLevel` -- the character's highest weapon upgrade level, which the game already
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
    /// The eight attributes in struct order (VIG, MND, end, STR, DEX, INT, FAI, ARC).
    pub attributes: [i32; STAT_COUNT],
    /// Effective max HP as stored in the save (SL2.bt `MaxHealth` == runtime
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

/// Read the eight attributes, level and stored vitals at a known stat-block base,
/// with range checks only -- no Rune Level identity. `stat_base` is the offset of
/// the first attribute (`PlayerGameData + 0x3c`).
///
/// Split out of [`stat_block_at`] so the identity can stay the scan's filter without
/// also being the only way a located block may be read. See
/// [`slot_stats_from_body`] for the character this distinction exists for.
fn read_stat_block(body: &[u8], stat_base: usize) -> Option<SlotStats> {
    let mut attributes = [0i32; STAT_COUNT];
    for (i, slot) in attributes.iter_mut().enumerate() {
        let v = rd_i32(body, stat_base + i * 4)?;
        if !(MIN_ATTR..=MAX_ATTR).contains(&v) {
            return None;
        }
        *slot = v;
    }
    let level = rd_i32(body, stat_base + LEVEL_FROM_STAT_BASE)?;
    if !(MIN_ATTR..=MAX_RUNE_LEVEL).contains(&level) {
        return None;
    }
    // Vitals are best-effort reads of stored values: a missing/implausible vital
    // decodes as 0 ("unknown") rather than rejecting the located block or inventing
    // a formula.
    let pgd = stat_base.checked_sub(PGD_STAT_BASE);
    let vital = |off: usize| {
        pgd.and_then(|p| rd_i32(body, p + off))
            .filter(|v| *v > 0)
            .unwrap_or(0)
    };
    // Same best-effort posture as the vitals: an out-of-range byte decodes as "unknown"
    // rather than rejecting an otherwise-valid block.
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

/// [`read_stat_block`] plus the Rune Level identity, which is what makes a blind
/// byte-scan of a 2.6 MB body safe: nothing but a real attribute block satisfies it.
fn stat_block_at(body: &[u8], stat_base: usize) -> Option<SlotStats> {
    let stats = read_stat_block(body, stat_base)?;
    if stats.level != stats.attributes.iter().sum::<i32>() - RUNE_LEVEL_BASE {
        return None;
    }
    Some(stats)
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
/// eight attributes. Returns `None` only for a slot with no locatable character.
///
/// # the identity is a locator, not a licence to exist (2026-09-01)
///
/// The Rune Level identity used to be the only way in, which meant a character it
/// does not hold had no attributes, no vitals and no `WL` -- anywhere. On the live
/// default container that was slot 1, `Dark Moon Bean`: `level` word 150 beside
/// attributes `[60, 10, 44, 21, 50, 9, 25, 7]` summing to 226, which implies RL 147.
/// Delta +3, so `stat_block_at` refused the real `PlayerGameData` and the scan of the
/// whole 0x280000-byte body found nothing else. The container decoded 9/10 slots while
/// naming 10/10 (the name has always had a structural fallback through
/// `bnd4::active_character_slots`), and the Load Character row for that one character
/// rendered its merged header with an empty attribute line and no `WL` -- the
/// user-reported defect, measured in run `br-20260901-161521-9f7d` at +80815ms.
///
/// A stored level can disagree with the attribute sum: `er-build-import-runtime` writes
/// the level slot from a planner payload's claimed `rl` while writing the attributes
/// from the payload's stat block (`character.rs::apply_stats`), and planner links with
/// exactly that inconsistency are known (`lib.rs` records `rl: 150` beside attributes
/// summing to 228). A save editor does the same thing by hand.
///
/// So the identity stays the scan's filter -- it is what makes a blind byte-walk over
/// megabytes trustworthy -- and a body it rejects falls back to the structural locator
/// `bnd4::slot_player_game_data_offset` (face-anchored + `slot_pgd_core_plausible`:
/// real name, level `1..=713`, sane health/flasks/gender, eight attributes `1..=99`).
/// That locator is not a new risk: it is the same one `active_character_slots` already
/// uses, and it is proven to resolve this exact slot -- it is where slot 1's name came
/// from in the failing run. There is no recursion: it only ever calls
/// [`located_stat_block_offset`], never this function. It is reached through
/// `bnd4::slot_stat_block_offset` rather than directly, because the two modules anchor
/// the same struct eight bytes apart and that conversion belongs beside the constant.
#[must_use]
pub fn slot_stats_from_body(body: &[u8]) -> Option<SlotStats> {
    if let Some((_, stats)) = located_stat_block(body) {
        return Some(stats);
    }
    read_stat_block(body, bnd4::slot_stat_block_offset(body)?)
}

/// Offset of the located eight-attribute stat block within a slot body.
///
/// # Why this is exported
///
/// This crate ships two ways to find a serialized `PlayerGameData` in a slot body, and they do not
/// agree on real saves:
///
/// * [`located_stat_block`] scans the body and accepts the offset where the **Rune Level
///   invariant** holds -- eight attributes in `1..=99` whose sum is `level + 79`. It is
///   self-validating: nothing but a real attribute block satisfies it.
/// * `bnd4::slot_player_game_data_offset` (and the DLL's `SerializedSaveSlot::player_game_data`)
///   instead find the leading `FACE` magics and search a fixed `0xa000..=0xa600` window before
///   each. That window is an observation, not an invariant, and the observation was too narrow:
///   measured across the ten characters of one real container the true delta ran
///   `0x9d14..=0xa05c`, so nine of the ten fell below the window's low bound and decoded as empty
///   slots. The System>Quit "Load Character from File" preview offered one row out of ten
///   (`slot_mask=0x8`, 2026-08-25) while the same file's stats cache decoded nine.
///
/// Exporting the stat-block offset lets the face-window locators keep their own acceptance test
/// while adding this candidate, instead of a third copy of the search drifting from both.
#[must_use]
pub fn located_stat_block_offset(body: &[u8]) -> Option<usize> {
    located_stat_block(body).map(|(stat_base, _)| stat_base)
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
///
/// Falls back to the structural locator for the same reason [`slot_stats_from_body`]
/// does, so name and stats agree about which slots hold a character instead of the
/// name silently working (through the caller's `active_character_slots` patch-up)
/// while the stats do not.
#[must_use]
pub fn slot_name_from_body(body: &[u8]) -> Option<String> {
    let stat_base = match located_stat_block(body) {
        Some((stat_base, _)) => stat_base,
        None => bnd4::slot_stat_block_offset(body)?,
    };
    slot_name_at_pgd(body, stat_base.checked_sub(PGD_STAT_BASE)?)
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

/// Slots that decoded a name but no stat block, as a bitmask (bit N = slot N).
///
/// The SEMAPHORE for a row with a header and nothing under it. The two caches the
/// ProfileSelect rows read are filled by two locators, so they can disagree per slot --
/// and when they do, that slot's Load Character row renders its merged header with an
/// empty attribute line and no `WL`. That reached the user as a visual observation on
/// 2026-09-01 while the log already carried the aggregate (`9/10 slots decoded, 10/10
/// names decoded`) and no oracle carried the disagreement. A count cannot name the
/// affected row; this can. Non-zero is a defect, not a state.
#[must_use]
pub fn named_without_stats_mask(
    names: &[Option<String>; 10],
    stats: &[Option<SlotStats>; 10],
) -> u32 {
    names
        .iter()
        .zip(stats.iter())
        .enumerate()
        .filter(|(_, (name, stats))| name.is_some() && stats.is_none())
        .fold(0u32, |mask, (slot, _)| mask | (1u32 << slot))
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
        // A save with distinct characters must decode different per-slot stats —
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
        // discriminator: its current fp is 78 while MaxFP is 95, so an off-by-4
        // read (current instead of max) would return 78 here and fail.
        assert_eq!((s2.max_hp, s2.max_fp, s2.max_stamina), (769, 95, 130));
        assert_eq!(
            (s9.max_hp, s9.max_fp, s9.max_stamina),
            (396, 95, 94),
            "slot 9 MaxFP must be 95 (current fp is 78 -- catches an off-by-4)"
        );
    }

    /// SL2.bt `PlayerGameData` field offsets, i.e. `bnd4`'s anchor -- runtime base + 8.
    /// Spelled out here because the fixture has to satisfy `bnd4::slot_pgd_core_plausible`,
    /// which reads in that vocabulary.
    const SAVE_PGD_HEALTH: usize = 0x08;
    const SAVE_PGD_MAX_HEALTH: usize = 0x0c;
    const SAVE_PGD_BASE_MAX_HEALTH: usize = 0x10;
    const SAVE_PGD_STAT_BASE: usize = 0x34;
    const SAVE_PGD_LEVEL: usize = 0x60;
    const SAVE_PGD_NAME: usize = 0x94;
    const SAVE_PGD_GENDER: usize = 0xb6;
    const SAVE_PGD_MAX_CRIMSON: usize = 0xf9;
    const SAVE_PGD_MAX_CERULEAN: usize = 0xfa;
    /// Inside `bnd4`'s `0xa000..=0xa600` PGD->face window.
    const FACE_DELTA: usize = 0xa300;

    /// A slot body holding one structurally valid character at a known offset, whose
    /// `level` word and attribute sum are whatever the caller says. No game bytes: every
    /// field is written here.
    fn synthetic_slot_body(name: &str, level: u32, attributes: [u32; STAT_COUNT]) -> Vec<u8> {
        // 8 so the runtime-relative base (`pgd - 8`) is still inside the buffer.
        const PGD: usize = 0x40;
        let mut body = vec![0u8; PGD + FACE_DELTA + 0x100];
        let put_u32 = |body: &mut Vec<u8>, at: usize, value: u32| {
            body[at..at + 4].copy_from_slice(&value.to_le_bytes());
        };
        put_u32(&mut body, PGD + SAVE_PGD_HEALTH, 1900);
        put_u32(&mut body, PGD + SAVE_PGD_MAX_HEALTH, 1900);
        put_u32(&mut body, PGD + SAVE_PGD_BASE_MAX_HEALTH, 1900);
        for (index, value) in attributes.iter().enumerate() {
            put_u32(&mut body, PGD + SAVE_PGD_STAT_BASE + index * 4, *value);
        }
        put_u32(&mut body, PGD + SAVE_PGD_LEVEL, level);
        for (index, unit) in name.encode_utf16().enumerate() {
            let at = PGD + SAVE_PGD_NAME + index * 2;
            body[at..at + 2].copy_from_slice(&unit.to_le_bytes());
        }
        body[PGD + SAVE_PGD_GENDER] = 1;
        body[PGD + SAVE_PGD_MAX_CRIMSON] = 8;
        body[PGD + SAVE_PGD_MAX_CERULEAN] = 6;
        let face = PGD + FACE_DELTA;
        body[face..face + 4].copy_from_slice(b"FACE");
        body
    }

    /// The reported defect (2026-09-01), as a unit test.
    ///
    /// The live default container's slot 1, `Dark Moon Bean`: `level` 150 beside attributes
    /// summing to 226, which implies RL 147. The Rune Level identity refuses that block, so
    /// the identity-only locator decoded the slot as empty -- and the Load Character row for
    /// the one character the user was playing rendered its merged header with no attribute
    /// line and no `WL`. Both the numbers and the shape are the measured ones.
    #[test]
    fn a_character_whose_level_disagrees_with_its_attribute_sum_still_decodes() {
        const ATTRIBUTES: [u32; STAT_COUNT] = [60, 10, 44, 21, 50, 9, 25, 7];
        const STORED_LEVEL: u32 = 150;
        assert_ne!(
            STORED_LEVEL as i32,
            ATTRIBUTES.iter().sum::<u32>() as i32 - RUNE_LEVEL_BASE,
            "the fixture only tests anything while it VIOLATES the identity"
        );
        let body = synthetic_slot_body("Dark Moon Bean", STORED_LEVEL, ATTRIBUTES);
        assert_eq!(
            located_stat_block(&body).map(|(_, stats)| stats.level),
            None,
            "the identity scan must still refuse it -- that is what makes the scan safe"
        );
        let stats = slot_stats_from_body(&body)
            .expect("a structurally valid character must decode even when the identity refuses it");
        assert_eq!(
            stats.level, STORED_LEVEL as i32,
            "the STORED level, not a derived one"
        );
        assert_eq!(stats.attributes, ATTRIBUTES.map(|value| value as i32));
        assert_eq!(
            slot_name_from_body(&body).as_deref(),
            Some("Dark Moon Bean"),
            "name and stats must agree about whether this slot holds a character"
        );
    }

    /// The fallback must not invent a character out of a body that has none: an empty slot
    /// stays empty, or every ProfileSelect row gains a phantom.
    #[test]
    fn the_structural_fallback_does_not_invent_a_character() {
        assert_eq!(slot_stats_from_body(&vec![0u8; 0x40000]), None);
        assert_eq!(slot_name_from_body(&vec![0u8; 0x40000]), None);
    }

    #[test]
    fn the_named_without_stats_mask_names_exactly_the_headerless_rows() {
        let empty: [Option<SlotStats>; 10] = [None; 10];
        let no_names: [Option<String>; 10] = core::array::from_fn(|_| None);
        assert_eq!(
            named_without_stats_mask(&no_names, &empty),
            0,
            "an empty container is not a defect"
        );

        let stats = SlotStats {
            level: 9,
            attributes: [15, 10, 11, 14, 13, 9, 9, 7],
            max_hp: 870,
            max_fp: 121,
            max_stamina: 115,
            matchmaking_weapon_level: Some(0),
        };
        let mut names: [Option<String>; 10] = core::array::from_fn(|_| None);
        let mut decoded: [Option<SlotStats>; 10] = [None; 10];
        // Slot 0 decoded both; slot 1 named only (the reported defect); slot 2 neither.
        names[0] = Some("Ordinary Bean".to_owned());
        decoded[0] = Some(stats);
        names[1] = Some("Dark Moon Bean".to_owned());
        assert_eq!(named_without_stats_mask(&names, &decoded), 1 << 1);

        // A stat block with no name is the other direction and is not this defect.
        names[1] = None;
        decoded[1] = Some(stats);
        assert_eq!(named_without_stats_mask(&names, &decoded), 0);
    }

    #[test]
    fn rejects_body_without_a_stat_block() {
        // A body of all-0xff (no in-range attribute octet) has no match.
        assert_eq!(slot_stats_from_body(&[0xffu8; 0x1000]), None);
        assert_eq!(slot_stats_from_body(&[]), None);
    }
}
