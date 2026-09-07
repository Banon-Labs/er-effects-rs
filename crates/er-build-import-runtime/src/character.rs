//! Level, attributes, starting class and memorised spells.
//!
//! Everything here is read back after it is written. A previous version of the equip path
//! reported "10/10" from a call counter while the character wore nothing, so no number in
//! this module is derived from a call having been made.

use er_build_import_core::BuildDoc;
use er_build_import_core::equip::EquipRef;
// Declared once in `er-game-base::rva`; the exporter reads the same two spell getters.
use er_game_base::rva::{
    GET_EQUIP_MAGIC_ID_RVA as GET_EQUIP_MAGIC_ID,
    GET_MAGIC_SLOTS_COUNT_RVA as GET_MAGIC_SLOTS_COUNT,
};
// For the layout pins below. The struct, not a number.
use eldenring::cs::PlayerGameData;

/// `GetMainPlayerStats(int out[10])`.
const GET_MAIN_PLAYER_STATS_RVA: usize = 0x788360;
/// `ApplyMainPlayerStats(const int in[10])` -- writes the fields **and** recomputes every
/// derived value: base/current max HP, FP and stamina, attack rating, max equip load,
/// resistance gauges and the spell-slot count. Writing the fields directly would leave all
/// of those stale, which is the whole reason this function is used.
const APPLY_MAIN_PLAYER_STATS_RVA: usize = 0x788cf0;

/// `CS::EquipMagicData::GetMagicSlotsCount(emd, SpecialEffect*)` -- pass null and it derives
/// the effect itself. Clamps to 14. The source of capacity; never hardcode a number.
/// `EquipMagicInSlot(emd, ChrAsmSlot slot, uint magicParamId) -> bool`.
const EQUIP_MAGIC_IN_SLOT_RVA: usize = 0x250490;

/// `EquipMagicData::entries`, an `EquipMagicItem[14]`.
///
/// The array bound, and deliberately not the character's capacity. Both natives this module
/// calls refuse `slot >= 0xe` themselves (`GetEquipMagicId` returns -1, `EquipMagicInSlot`
/// returns 0 having written nothing), so a slot below it can never be a write past the array --
/// which is what makes the clear below safe to run over the whole array rather than over the
/// number of slots the character may use. See [`memorise_spells`] for why it has to.
const MAGIC_SLOT_ENTRIES: usize = 14;

/// `EquipMagicItem::paramId` for a slot holding nothing.
///
/// It is both the value the read-back returns for an empty slot and the id handed to
/// `EquipMagicInSlot` to make one: the writer looks the id up in `MAGIC_PARAM_ST`, finds no row
/// for -1, and stores `{paramId: -1, charges: -1}`. That is not an inferred use of an invalid
/// argument -- `ChangeMagicEquipSlot` (`0x1407881c0`), the menu's own handler, computes
/// `magicId = 0xffffffff` when the gaitem it was handed carries no item id and passes exactly
/// that to exactly this function.
const EMPTY_MAGIC_SLOT: i32 = -1;

/// `GameDataMan + 0x08 -> PlayerGameData*`.
///
/// Measured: `GetMainPlayerGameData` (`0x140e9fc30`) is a twelve-byte leaf reading exactly this
/// slot, and the `GameDataMan` constructor pairs 351/351 with zero moved offsets across
/// 1.16.2/1.17. Full witness in `storage::GAME_DATA_MAN_PLAYER_OFFSET`.
const GAME_DATA_MAN_PLAYER_OFFSET: usize = 0x08;
/// `EquipGameData::equipMagicData`, a pointer.
const EQUIP_GAME_DATA_MAGIC_OFFSET: usize = 0x280;
/// `PlayerGameData::level`.
const PGD_LEVEL: usize = 0x68;
/// `PlayerGameData::archetype`, one byte: the starting class.
const PGD_ARCHETYPE: usize = 0xbf;
/// `PlayerGameData` attribute offsets, in **struct** order.
const PGD_VIGOR: usize = 0x3c;
const PGD_MIND: usize = 0x40;
const PGD_ENDURANCE: usize = 0x44;
const PGD_STRENGTH: usize = 0x48;
const PGD_DEXTERITY: usize = 0x4c;
const PGD_INTELLIGENCE: usize = 0x50;
const PGD_FAITH: usize = 0x54;
const PGD_ARCANE: usize = 0x58;

// Pinned to the shared STRUCT, not merely written down twice.
//
// These eight are a private copy of offsets `er-game-base::pgd` already pins against
// `../fromsoftware-rs`'s `#[repr(C)]` `PlayerGameData`. A private copy is outside those pins by
// construction: `pgd.rs` const-asserts vigor/mind/endurance/strength/dexterity and the struct
// could drift under these literals without one assertion firing.
//
// It matters more than the duplication suggests. The write does not use these offsets at all --
// it goes through `ApplyMainPlayerStats(const int in[10])`, an array API that takes no offsets --
// so a drift here would not corrupt anything. It would do something quieter: these offsets are
// the read-back, the oracle that decides whether the import worked. A drifted offset reports a
// correct import as a wrong attribute, which sends the next reader after a bug that is not there.
//
// intelligence/faith/arcane are three of the eight fields whose 1.17 position is bracketed rather
// than witnessed, so they are the likeliest to move. `offset_of!` is what makes that a build
// failure instead of a false bug report.
const _: () = assert!(PGD_VIGOR == core::mem::offset_of!(PlayerGameData, vigor));
const _: () = assert!(PGD_MIND == core::mem::offset_of!(PlayerGameData, mind));
const _: () = assert!(PGD_ENDURANCE == core::mem::offset_of!(PlayerGameData, endurance));
const _: () = assert!(PGD_STRENGTH == core::mem::offset_of!(PlayerGameData, strength));
const _: () = assert!(PGD_DEXTERITY == core::mem::offset_of!(PlayerGameData, dexterity));
const _: () = assert!(PGD_INTELLIGENCE == core::mem::offset_of!(PlayerGameData, intelligence));
const _: () = assert!(PGD_FAITH == core::mem::offset_of!(PlayerGameData, faith));
const _: () = assert!(PGD_ARCANE == core::mem::offset_of!(PlayerGameData, arcane));
const _: () = assert!(PGD_LEVEL == core::mem::offset_of!(PlayerGameData, level));
const _: () = assert!(PGD_ARCHETYPE == core::mem::offset_of!(PlayerGameData, archetype));

/// The ten-int array `GetMainPlayerStats` fills and `ApplyMainPlayerStats` consumes.
///
/// **Endurance and mind are swapped relative to the on-screen order.** Index 2 is endurance
/// (`PGD+0x44`) and index 3 is mind (`PGD+0x40`), verified in both functions' disassembly.
/// Getting this backwards imports a different, plausible-looking build.
const STAT_LEVEL: usize = 0;
const STAT_VIGOR: usize = 1;
const STAT_ENDURANCE: usize = 2;
const STAT_MIND: usize = 3;
const STAT_STRENGTH: usize = 4;
const STAT_DEXTERITY: usize = 5;
/// Index 6 is `baseDurability`. It round-trips verbatim and its meaning is not established,
/// so it is read and written back untouched rather than set to anything.
const STAT_INTELLIGENCE: usize = 7;
const STAT_FAITH: usize = 8;
const STAT_ARCANE: usize = 9;

type GetStatsFn = unsafe extern "system" fn(*mut i32);
type ApplyStatsFn = unsafe extern "system" fn(*const i32);
type SlotsCountFn = unsafe extern "system" fn(usize, usize) -> u32;
type EquipMagicFn = unsafe extern "system" fn(usize, i32, u32) -> u8;
type MagicIdFn = unsafe extern "system" fn(usize, i32) -> i32;

/// What the level/attribute write achieved, read back from `PlayerGameData`.
#[derive(Debug, Default)]
pub struct StatsOutcome {
    /// Level before and after.
    pub level: (i32, i32),
    /// Attributes that still differ from the build after applying, as `(name, want, got)`.
    pub wrong: Vec<(&'static str, i32, i32)>,
}

impl StatsOutcome {
    /// Whether every attribute and the level match the build.
    pub fn is_correct(&self) -> bool {
        self.wrong.is_empty()
    }
}

/// The live `PlayerGameData*`, or `None`.
///
/// # Safety
///
/// Game thread only.
pub unsafe fn player_game_data() -> Option<usize> {
    use eldenring::cs::GameDataMan;
    use fromsoftware_shared::FromStatic;

    let gdm = GameDataMan::instance_ptr().ok()? as usize;
    if gdm == 0 {
        return None;
    }
    // Safety: one pointer read at the documented offset, checked for null.
    let pgd = unsafe { *((gdm + GAME_DATA_MAN_PLAYER_OFFSET) as *const usize) };
    (pgd != 0).then_some(pgd)
}

/// The archetype index for a class name, or `None` if unrecognised.
pub fn archetype_of(class_name: &str) -> Option<u8> {
    er_build_import_core::class::archetype_for_class(class_name)
}

/// Set the starting class.
///
/// Writes `PlayerGameData::archetype` directly and deliberately does **not** call the native
/// class setter at `0x140787250`: that one re-runs character initialisation, which overwrites
/// all eight attributes and the level with the class defaults and re-issues starting gear --
/// erasing the build. Nothing caches the archetype, so the byte is the whole operation.
///
/// Returns the value read back, which is the only evidence the write landed.
///
/// # Safety
///
/// Game thread, character loaded.
pub unsafe fn set_class(pgd: usize, class_name: &str) -> Option<(u8, u8)> {
    let wanted = archetype_of(class_name)?;
    // Safety: a one-byte write into live save data at a verified offset, then a read back.
    unsafe {
        *((pgd + PGD_ARCHETYPE) as *mut u8) = wanted;
        Some((wanted, *((pgd + PGD_ARCHETYPE) as *const u8)))
    }
}

/// Apply the build's level and attributes.
///
/// # Safety
///
/// Game thread, character in the world. `ApplyMainPlayerStats` `DLPanic`s on a null
/// `CSMenuMan`/`WorldChrMan` and skips the vitals recompute if `mainPlayerIns` is null, so
/// the caller's in-world gate is a precondition, not a nicety.
pub unsafe fn apply_stats(module_base: usize, pgd: usize, doc: &BuildDoc) -> Option<StatsOutcome> {
    // Not `StatsOutcome::default()` on refusal, and the difference is the whole reason this
    // returns an `Option`. A default outcome has an empty `wrong` list, so `is_correct()` answers
    // true and the caller logs "every attribute matches the build" for a character whose stats
    // were never written. A refusal has to be unrepresentable as a success.
    let [get, apply] = crate::native::resolve_all(
        module_base,
        [
            (GET_MAIN_PLAYER_STATS_RVA, "GetMainPlayerStats"),
            (APPLY_MAIN_PLAYER_STATS_RVA, "ApplyMainPlayerStats"),
        ],
    )
    .ok()?;
    // Safety: both addresses were resolved for the running build immediately above.
    let get: GetStatsFn = unsafe { core::mem::transmute(get) };
    // Safety: as above.
    let apply: ApplyStatsFn = unsafe { core::mem::transmute(apply) };

    let mut stats = [0i32; 10];
    // Safety: the engine fills exactly ten ints.
    unsafe { get(stats.as_mut_ptr()) };
    // Safety: reading the level field for the before/after report.
    let before = unsafe { *((pgd + PGD_LEVEL) as *const i32) };

    let want = |key: &str| doc.stats.get(key).copied().unwrap_or_default() as i32;
    // `rl` is the derived level, not the payload's claim. `fetch_inner` overwrites
    // `doc.stats["rl"]` with `sum(attributes) - 79` before handing the doc over, precisely so
    // this line cannot stamp a level that contradicts the attributes two lines below it. Before
    // that (2026-09-01) a planner claiming `rl: 150` beside attributes summing to 226 produced a
    // character with `level == 150` and a stat block implying 147 -- which `er_save_loader::stats`
    // then could not locate at all, blanking that character's whole Load Character row.
    stats[STAT_LEVEL] = want("rl");
    stats[STAT_VIGOR] = want("vig");
    stats[STAT_ENDURANCE] = want("vit"); // the planner calls Endurance "vit"
    stats[STAT_MIND] = want("mnd");
    stats[STAT_STRENGTH] = want("str");
    stats[STAT_DEXTERITY] = want("dex");
    stats[STAT_INTELLIGENCE] = want("int");
    stats[STAT_FAITH] = want("fth");
    stats[STAT_ARCANE] = want("arc");
    // index 6 (baseDurability) keeps whatever the getter returned.

    // Safety: the engine reads ten ints and recomputes everything derived from them.
    unsafe { apply(stats.as_ptr()) };

    // Read back from the struct, not from the array we just handed over.
    let mut outcome = StatsOutcome::default();
    // Safety: plain reads of live save data at verified offsets.
    let after = unsafe { *((pgd + PGD_LEVEL) as *const i32) };
    outcome.level = (before, after);
    let checks: [(&'static str, usize, i32); 9] = [
        ("level", PGD_LEVEL, want("rl")),
        ("vigor", PGD_VIGOR, want("vig")),
        ("mind", PGD_MIND, want("mnd")),
        ("endurance", PGD_ENDURANCE, want("vit")),
        ("strength", PGD_STRENGTH, want("str")),
        ("dexterity", PGD_DEXTERITY, want("dex")),
        ("intelligence", PGD_INTELLIGENCE, want("int")),
        ("faith", PGD_FAITH, want("fth")),
        ("arcane", PGD_ARCANE, want("arc")),
    ];
    for (name, offset, wanted) in checks {
        // Safety: as above.
        let got = unsafe { *((pgd + offset) as *const i32) };
        if got != wanted {
            outcome.wrong.push((name, wanted, got));
        }
    }
    Some(outcome)
}

/// What the spell pass achieved.
#[derive(Debug, Default)]
pub struct SpellOutcome {
    /// Memory slots the game says the character has.
    pub capacity: u32,
    /// Spells the build wants memorised.
    pub wanted: usize,
    /// Slots whose contents afterwards match the build.
    pub verified: usize,
    /// Spells dropped because the character has too few memory slots.
    pub over_capacity: usize,
    /// Memory slots emptied before the build's spells were written.
    pub cleared: usize,
    /// Whether the clear was skipped because the build named no spells at all -- which is a
    /// different report from "the character had nothing memorised", and the only case where a
    /// spell the build does not name is left in place on purpose.
    pub clear_declined: bool,
    /// Slots still holding something past the end of the build's list once the pass finished.
    ///
    /// The proof, and the number the reported defect would have been caught by: the old pass
    /// wrote slots `0..n` and read those same slots back, so a character carrying nine spells
    /// the build never mentioned scored `1/1 memorised` while the HUD showed ten.
    pub stale: usize,
    /// `(slot, expected, actual)` for the first few failures.
    pub mismatches: Vec<(i32, i32, i32)>,
}

/// Memorise the build's spells, in order, within the character's real slot count.
///
/// # Why this empties the memory slots first
///
/// A build's spell list is dense and carries no positions: `export_doc.rs` gives a memorised
/// spell no `equipIndex`, only an `order`, and its place in the list *is* the memorisation slot.
/// So the list cannot express a hole, and a list of length `n` is a complete statement that
/// slots `n..` hold nothing -- unlike an armament or a talisman, where the build names the
/// position and a position it does not name is simply absent from the document.
///
/// Writing `0..n` and stopping therefore does not import the build, it MERGES into the character:
/// a build with one spell, imported onto a character carrying ten, leaves nine of the old ones
/// alongside the new one. Reported by the user 2026-08-30, and invisible until this session
/// because the spell natives were unmapped on 1.17 and the whole pass was inert.
///
/// # Why the clear runs over all fourteen entries and always from slot 0
///
/// `EquipMagicInSlot` ends by calling `ValidateEquipMagicData` (`0x140251010`), which
/// left-COMPACTS the array: it walks `0..14` and, for every empty entry, pulls the first
/// non-empty entry after it forward and empties the source. Two consequences, and both of them
/// break the obvious implementation:
///
/// * a clear bounded by the character's capacity is worse than no clear at all. Emptying
///   `0..capacity` makes the compaction drag whatever sits in `capacity..14` -- spells from
///   before a respec dropped the character's Memory Stone count -- forward into exactly the
///   slots the build is about to fill;
/// * slot indices do not survive a clear, so a sweep of `0..14` in order does not empty the
///   array. Every non-empty entry is a prefix of it after any write, which is what makes
///   clearing slot 0, repeatedly, correct: each call removes exactly one entry and the engine
///   re-packs the rest. Fourteen iterations therefore drain fourteen entries, and every one of
///   them touches only slot 0, which is inside the array by construction.
///
/// The fill afterwards is unaffected: writing `0, 1, 2, ...` into an empty array leaves no hole
/// for the compaction to close, so no spell moves after it is placed.
///
/// # The one case it declines to clear
///
/// `spells` empty means either "this build memorises nothing" or "this payload has no `spells`
/// key", and [`er_build_import_core::model::BuildDoc`] cannot tell them apart -- the field is
/// `#[serde(default)]`. It also covers "every spell the build named failed to resolve against
/// the catalog", which is reported separately as a rejection. Wiping a character's spells on any
/// of those readings is destructive on a guess, so an empty list clears nothing and says so.
///
/// # Safety
///
/// Game thread, character loaded, `egd` live.
pub unsafe fn memorise_spells(
    module_base: usize,
    egd: usize,
    spells: &[EquipRef],
) -> Option<SpellOutcome> {
    let mut outcome = SpellOutcome {
        wanted: spells.len(),
        ..SpellOutcome::default()
    };

    // Safety: one pointer read at the verified EquipGameData offset.
    let emd = unsafe { *((egd + EQUIP_GAME_DATA_MAGIC_OFFSET) as *const usize) };
    if emd == 0 {
        return Some(outcome);
    }

    // The three move together: without the capacity there is no bound to write within, without
    // the writer nothing is memorised, and without the read-back nothing is PROVEN memorised --
    // and this module's header says no number in it is derived from a call having been made.
    // `None` here is "the spell pass did not run", which the caller says out loud; a zeroed
    // outcome would read as "the game reports zero memory slots", which is a different claim.
    let [slots_count, equip, magic_id] = crate::native::resolve_all(
        module_base,
        [
            (
                GET_MAGIC_SLOTS_COUNT,
                "CS::EquipMagicData::GetMagicSlotsCount",
            ),
            (EQUIP_MAGIC_IN_SLOT_RVA, "EquipMagicInSlot"),
            (GET_EQUIP_MAGIC_ID, "GetEquipMagicId"),
        ],
    )
    .ok()?;
    // Safety: all three addresses were resolved for the running build immediately above.
    let slots_count: SlotsCountFn = unsafe { core::mem::transmute(slots_count) };
    // Safety: as above.
    let equip: EquipMagicFn = unsafe { core::mem::transmute(equip) };
    // Safety: as above.
    let magic_id: MagicIdFn = unsafe { core::mem::transmute(magic_id) };

    // Ask the game, never assume: this accounts for Memory Stones and talismans, and the
    // engine clamps it to 14.
    // Safety: null SpecialEffect means "derive it from the player", which the function handles.
    outcome.capacity = unsafe { slots_count(emd, 0) };

    // Empty the array, then fill it. See the header for why this is slot 0 fourteen times over
    // rather than a sweep, and why the bound is the array rather than the capacity.
    outcome.clear_declined = spells.is_empty();
    if !outcome.clear_declined {
        for _ in 0..MAGIC_SLOT_ENTRIES {
            // Safety: the read-back. Slot 0 is inside the fourteen-entry array.
            if unsafe { magic_id(emd, 0) } == EMPTY_MAGIC_SLOT {
                break;
            }
            // Safety: slot 0 is inside the array, and the native bounds-checks it again itself.
            // The id is the one the menu's own handler passes to empty a slot.
            unsafe { equip(emd, 0, EMPTY_MAGIC_SLOT as u32) };
            outcome.cleared += 1;
        }
    }

    for (index, spell) in spells.iter().enumerate() {
        let Ok(slot) = i32::try_from(index) else {
            break;
        };
        if index >= outcome.capacity as usize {
            outcome.over_capacity += 1;
            continue;
        }
        // Safety: slot is within the reported capacity; the id is a MAGIC_PARAM row id.
        unsafe { equip(emd, slot, spell.param_id) };
        // Safety: the read-back.
        let actual = unsafe { magic_id(emd, slot) };
        let expected = spell.param_id as i32;
        if actual == expected {
            outcome.verified += 1;
        } else if outcome.mismatches.len() < 8 {
            outcome.mismatches.push((slot, expected, actual));
        }
    }

    // What the old read-back could not see. Reading back the slots that were just written proves
    // the writes landed and nothing else; the defect this pass exists to close lives entirely in
    // the slots it did not write. So the tail is read too, and a non-zero count here is the
    // import failing to match the build even when every other number on the line is perfect.
    let occupied = spells.len().saturating_sub(outcome.over_capacity);
    for slot in occupied..MAGIC_SLOT_ENTRIES {
        let Ok(slot) = i32::try_from(slot) else {
            break;
        };
        // Safety: the read-back, within the array bound.
        if unsafe { magic_id(emd, slot) } != EMPTY_MAGIC_SLOT {
            outcome.stale += 1;
        }
    }

    Some(outcome)
}

/// `PlayerGameData::runeArcActive`.
const PGD_RUNE_ARC_ACTIVE: usize = 0xff;

/// Light the rune arc so an equipped great rune actually applies.
///
/// `CS::ChrIns::CanUseRuneArc` is `runeArcActive && IsHostLike(chrType)`, and
/// `ChrBigRuneSpEffectSlot::Update` applies the rune's SpEffects from there -- so this one bool
/// is what turns an equipped rune into a live buff. Returns the value read back.
///
/// # Safety
///
/// Game thread, character loaded.
pub unsafe fn activate_rune_arc() -> bool {
    let Some(pgd) = (unsafe { player_game_data() }) else {
        return false;
    };
    // Safety: one bool write into live save data at a verified offset, then a read back.
    unsafe {
        *((pgd + PGD_RUNE_ARC_ACTIVE) as *mut u8) = 1;
        *((pgd + PGD_RUNE_ARC_ACTIVE) as *const u8) != 0
    }
}
