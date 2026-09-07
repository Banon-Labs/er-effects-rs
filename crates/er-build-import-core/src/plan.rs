//! Turning a parsed build into concrete item grants.
//!
//! # The grant record
//!
//! The layout below is not invented here: it is the record the community's
//! long-standing `ItemGib` routine consumes, recovered from the planner's own
//! Cheat Engine exporter (`yF`), which emits per item
//! `dd <id+upgrade> <quantity> <reinforceLv:16><0:16> <weaponSkill>` and
//! terminates the array with `00000000 00000000 00000000 FFFFFFFF`.
//!
//! Three details are easy to get wrong and are pinned by tests:
//!
//! 1. The affinity is an **offset added into the item id** (`Occult` = +1200),
//!    not an index and not a separate field.
//! 2. The upgrade level goes in **both** places, exactly as the record above spells
//!    it: folded into the id (`<id+upgrade>`) and passed as the separate 16-bit
//!    `reinforceLv`. The id is the half that is actually read back -- see
//!    [`armament_item_id`] for the measurement. `EquipParamWeapon::GetEntry`
//!    normalises to `(paramId / 100) * 100` because the last two digits are the
//!    level, not because they are noise; setting only `CSWepGaitemIns::reinforcement`
//!    yields a `+0` weapon whose `GetReinforcement` still answers 25.
//! 3. `weaponSkill` is the ash of war's **`EquipParamGem` row**, re-tagged with
//!    the game's gem category (see [`GEM_ITEM_CATEGORY`]). The planner's own
//!    database tags ashes with nibble 2, which is the planner's convention and
//!    not the game's, so the nibble is REPLACED rather than kept.

use crate::catalog::{Catalog, Entry, Kind};
use crate::model::{BuildDoc, Slot};

/// Sentinel meaning "leave this armament's default skill alone".
pub const NO_SKILL: u32 = 0xFFFF_FFFF;

/// The game's item-category nibble for `EquipParamGem`, i.e. for an ash of war.
///
/// Not a "flag", and not a spare bit: it is the same category tag every other item kind carries
/// (weapons 0, protectors 1, accessories 2, goods 4) and the engine dispatches on it.
/// `CS::CSGaitemImp::GetGaitemHandleByItemId` switches on `itemId >> 28` and sends 8 to
/// `GetGaItemHandleGem`; `CS::GaitemLookupResult::GetSwordArtsParamId` refuses any item id whose
/// `& 0xF000_0000` is not `0x8000_0000`, then looks the low 28 bits up in `EquipParamGem`.
///
/// So the 28 bits under this nibble must be a gem row, never a `SwordArtsParam` row. The two id
/// spaces are easy to swap -- most ashes sit at `gem = arts * 100` -- and swapping them is
/// SILENT: the value still passes every shape check and simply names a row that does not exist.
/// That swap is the bug that shipped: the runtime catalog resolved ash names to `SwordArtsParam`
/// rows, every name resolved, and no weapon came out carrying its ash.
pub const GEM_ITEM_CATEGORY: u32 = 0x8000_0000;

/// Affinity name -> the offset it adds to an armament's item id.
const INFUSIONS: &[(&str, u32)] = &[
    ("Standard", 0),
    ("Heavy", 100),
    ("Keen", 200),
    ("Quality", 300),
    ("Fire", 400),
    ("Flame Art", 500),
    ("Lightning", 600),
    ("Sacred", 700),
    ("Magic", 800),
    ("Cold", 900),
    ("Poison", 1000),
    ("Blood", 1100),
    ("Occult", 1200),
];

/// Flasks the exporter deliberately never grants -- the character already owns
/// them and duplicating them corrupts the flask UI.
const NEVER_GRANT: &[&str] = &["Flask of Crimson Tears", "Flask of Cerulean Tears"];

/// The most of any one consumable a build import will hand out.
///
/// # Why a ceiling exists over the game's own maximum
///
/// [`Entry::max_stored`] is `EquipParamGoods.maxNum`, and that field has a tail this importer
/// must not follow. Measured against the installed 1.17 `regulation.bin`, 21 ordinary consumables
/// declare `maxNum = 999` -- Furlcalling Finger Remedy, Ruin Fragment, Roundrock -- and a build
/// that merely lists one of those is not asking for nine hundred of it. Emptying that into the
/// player's inventory is the unasked-for mutation, not the missing feature.
///
/// # Why this number and not a comfortable one
///
/// 99 is the engine's own ceiling, not one chosen here. It is the only literal in
/// `CS::EquipInventoryData::GetMaxAmountForItem` (1.16.2 `0x14024e570`, same address on 1.17),
/// which returns it twice: once when a goods row declares no `maxNum` at all, and again for a
/// pot-group item in a session that is not enforcing pot limits. Clamping to it costs nothing in
/// any realistic case -- boluses declare exactly 99 and are untouched, a Fire Pot declares 10 --
/// and removes only the absurd tail.
const MAX_GRANTED_PER_CONSUMABLE: u32 = 99;

/// How many of one consumable the build is asking for.
///
/// # The payload does not say, and the number still has to come from somewhere
///
/// A planner slot carries `{name, order, upgrade, infusion, weaponArt, equipIndex, equipSet}` and
/// nothing else -- there is no count field anywhere in the document, on tools or on any other
/// category. So the importer picks, and the only defensible pick is the item's own limit rather
/// than a number invented here: [`Entry::max_stored`], clamped by
/// [`MAX_GRANTED_PER_CONSUMABLE`].
///
/// # Why not one, which is what the planner's own exporter emits
///
/// The planner's Cheat Engine exporter hard-codes `quantity = 1`, and that was this function's
/// behaviour by accident -- `max_stored` was declared, never populated, and `unwrap_or(1)` made
/// every consumable a single item. One Fire Pot is not a build; the tools list is the character's
/// pouch, and reproducing it with one of each reproduces the names and none of the loadout.
///
/// # Why the target is safe to set high
///
/// The grant path RECONCILES to this number rather than adding it: it grants the shortfall
/// between what the character already holds and what is asked for, and grants nothing at all when
/// the character is already at or over it. Re-importing the same build twice is therefore a
/// no-op, not a doubled stack.
fn consumable_quantity(entry: Option<Entry>) -> u32 {
    entry
        .and_then(|found| found.max_stored)
        .unwrap_or(1)
        .clamp(1, MAX_GRANTED_PER_CONSUMABLE)
}

/// How many arrows or bolts the build is asking for.
///
/// # A different field, because ammunition is not a consumable
///
/// Arrows and bolts are `EquipParamWeapon` rows, not `EquipParamGoods` -- which is why they equip
/// into dedicated `ChrAsmSlot` positions instead of the quickbar -- so `maxNum`, the number behind
/// [`consumable_quantity`], does not describe them and is not even read for them. The engine says
/// so itself: `CS::EquipInventoryData::GetMaxAmountForItem` (1.16.2 and 1.17 both `0x14024e570`)
/// handles the goods category inline and tail-jumps every other category to `::GetMaxItemQuantity`
/// (1.16.2 `0x140674680`, 1.17 `0x1406754d0`), whose weapon branch is:
///
/// ```text
/// movzbl 0xe6(%rcx),%edx   ; EquipParamWeapon.weaponCategory
/// cmp    $0xd,%dl          ; 13 -- arrows
/// je     take_it
/// cmp    $0xe,%dl          ; 14 -- bolts
/// jne    return_1
/// take_it:
/// movzbl 0x235(%rcx),%eax  ; EquipParamWeapon.maxArrowQuantity
/// ```
///
/// Any other weapon row falls through to `mov $0x1,%eax`, which is why every armament in this
/// module is granted a literal 1: that is not a convention, it is the engine's answer.
///
/// # No ceiling of our own, and the histogram that says why one is not needed
///
/// [`MAX_GRANTED_PER_CONSUMABLE`] exists because `maxNum` has an absurd tail -- 21 goods rows
/// declare 999. `maxArrowQuantity` has none. Measured over the 73 ammunition rows of the installed
/// 1.17 `regulation.bin` (`scripts/regulation-ammo-census.py`): `{1: 2, 20: 5, 30: 8, 99: 58}`.
/// The 20s are the five Ballista Bolts, the 30s the eight Great Arrows, and the 99s every ordinary
/// arrow and bolt -- the game's own quiver limits, and 99 is the same ceiling the goods path caps
/// at anyway. Clamping would be machinery guarding against a tail that does not exist.
///
/// The two rows declaring 1 are `47000000` and `47010000`, which the message repository does not
/// name at all, so the catalog never resolves them and this never sees them.
///
/// A row that declares nothing gets one, exactly as [`consumable_quantity`] does: handing a player
/// the engine's fallback for an item the game has no opinion about is the over-grant this avoids.
fn ammo_quantity(entry: Option<Entry>) -> u32 {
    entry.and_then(|found| found.max_stored).unwrap_or(1).max(1)
}

/// One item to hand to the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// Category-tagged item id with any affinity offset already applied.
    pub item_id: u32,
    /// Other ids the same name resolved to, if the game has more than one row under it.
    ///
    /// The grant path must treat all of these as "this item" when it asks whether the player
    /// already holds one. Checking only `item_id` is how a build import handed out a second
    /// Flask of Wondrous Physick: the catalog had resolved the name to one row, the player's
    /// existing flask was the other, the held-count came back zero, and a duplicate was granted.
    pub also_known_as: Vec<u32>,
    /// How many to give.
    pub quantity: u32,
    /// Upgrade level, as its own field.
    ///
    /// Whose number this is matters: see [`Self::upgrade_is_character_default`].
    pub reinforce_lv: u16,
    /// Whether [`Self::reinforce_lv`] came from the character-wide `weaponUpgrade` rather than
    /// from this slot's own `upgrade`.
    ///
    /// The two are on different scales, and only for somber armaments. The planner's
    /// character-wide number is always regular smithing-stone levels 0..=25 and is mapped down per
    /// armament when it renders (`U_(weaponUpgrade, weapon)` -> `lr[level]`). A per-slot `upgrade`
    /// is not mapped: the planner's own editor caps that input at the mapped maximum -- 10 for a
    /// somber armament -- writes the typed number straight into the slot, and adds it straight to
    /// the item id when it exports. So a somber armament at `weaponUpgrade: 25` means +10, while
    /// the same armament at `upgrade: 25` would be nonsense the planner cannot produce.
    ///
    /// Which mapping to apply is therefore decided here, at the source of the number, and applied
    /// in the runtime, which is the only side that can measure whether an armament is somber.
    pub upgrade_is_character_default: bool,
    /// Ash of war as a gem item id ([`GEM_ITEM_CATEGORY`] `| EquipParamGem row`), or
    /// [`NO_SKILL`].
    pub weapon_skill: u32,
    /// What this grant is, for logs and for the user.
    pub label: String,
    /// `EquipParamGoods.potGroupId` when the game POT-caps this item, else `None`.
    ///
    /// Carried from [`crate::catalog::Entry::pot_group`], which documents the mechanism. It is on
    /// the grant because it changes what "grant five of these" MEANS: for a pot-capped item the
    /// engine clamps the add to `potItemsCapacity[g] - potItemsCount[g]` without saying so, so a
    /// grant path that wants to deliver the requested number has to free space in the group
    /// first -- and the group id is what tells it which other carried items would free any.
    pub pot_group: Option<u8>,
    /// Whether this is an armament -- the only kind that mints a per-instance gaitem.
    ///
    /// # Why the category nibble cannot answer this
    ///
    /// The runtime used to decide it arithmetically: `item_id & 0xF000_0000 == 0` means the weapon
    /// category, and the weapon category means an armament. That was true only while ammunition
    /// was unimplemented. Arrows and bolts are `EquipParamWeapon` rows and carry the same nibble,
    /// so the test now answers "armament" for a quiver of Bone Arrows -- and answering it wrongly
    /// is not cosmetic. The armament path mints one `GaItemHandle` through
    /// `GetGaitemHandleWeaponWithGem`, writes an upgrade level into the instance and mounts a gem;
    /// none of those exist for ammunition, and a stack of 99 arrows is not one instance.
    ///
    /// So the fact travels with the grant, decided where the catalog kind is still known, rather
    /// than being re-derived from an id that no longer distinguishes the two.
    pub armament: bool,
}

impl Grant {
    /// Encode as the 16-byte `ItemGib` record.
    pub fn to_record(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[0..4].copy_from_slice(&self.item_id.to_le_bytes());
        out[4..8].copy_from_slice(&self.quantity.to_le_bytes());
        out[8..10].copy_from_slice(&self.reinforce_lv.to_le_bytes());
        // out[10..12] stays zero: the exporter emits an explicit zero half-word.
        out[12..16].copy_from_slice(&self.weapon_skill.to_le_bytes());
        out
    }
}

/// An item the catalog could not resolve.
///
/// Surfaced rather than dropped: a silently missing weapon is indistinguishable
/// from a broken importer, and this is exactly how the `Miséricorde` accent bug
/// was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    /// Which catalog was searched.
    pub kind: Kind,
    /// The name as the build spelled it.
    pub name: String,
}

/// Everything needed to reproduce a build in game.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Items to grant, in build order.
    pub grants: Vec<Grant>,
    /// Spell param ids to memorise, in slot order.
    pub equip_spells: Vec<u32>,
    /// Names that resolved to nothing.
    pub unresolved: Vec<Unresolved>,
}

impl Plan {
    /// Whether every referenced item resolved.
    pub fn is_complete(&self) -> bool {
        self.unresolved.is_empty()
    }
}

/// The offset an affinity adds to an armament id, or `None` if unrecognised.
pub fn infusion_offset(infusion: Option<&str>) -> Option<u32> {
    let wanted = infusion.unwrap_or("Standard");
    INFUSIONS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
        .map(|(_, offset)| *offset)
}

/// Split an armament param id back into its base row, affinity name and upgrade level -- the
/// inverse of [`infusion_offset`] plus [`armament_item_id`], and the arithmetic the exporter runs
/// on every equipped weapon.
///
/// An armament id carries three things at once, and the exporter needs all three separated:
/// `base + affinity_offset + level`, where the affinity is a multiple of [`INFUSION_STEP`] inside
/// a [`ARMAMENT_ID_BLOCK`] block and the level is the last two digits.
///
/// **The level must come off before the row is named.** `EquipParamWeapon` has no row for a
/// levelled id -- 16110200 (Keen Cross-Naginata) is a row, 16110217 (the same armament at +17) is
/// not -- and the game's name getter is an exact `MsgRepositoryImp::LookupEntry`, so it answers
/// null for anything that is not a row. Leaving the level on therefore does not produce a slightly
/// wrong name; it produces no name, and the exporter drops the slot. That is what emptied the
/// inventory of every exported build: a finished character's armaments are all upgraded, so every
/// one of them looked unnameable. Verified against the installed regulation
/// (`scripts/regulation-params.py --contains 16110217 EquipParamWeapon` -> absent).
///
/// `Standard` comes back as `None` rather than as the string, because that is how the planner
/// spells it -- a slot with no `infusion` key. Emitting the word would import identically and diff
/// against every hand-authored build.
///
/// ```
/// use er_build_import_core::plan::{infusion_offset, split_armament_id, ArmamentId};
/// // Misericorde + Occult, the pair the importer builds as 1_070_000 + 1200.
/// assert_eq!(
///     split_armament_id(1_071_200),
///     ArmamentId {
///         row: 1_070_000,
///         row_with_affinity: 1_071_200,
///         infusion: Some("Occult"),
///         level: 0,
///     },
/// );
/// // The same armament as the player is actually carrying it: Occult, +9.
/// assert_eq!(
///     split_armament_id(1_071_209),
///     ArmamentId {
///         row: 1_070_000,
///         row_with_affinity: 1_071_200,
///         infusion: Some("Occult"),
///         level: 9,
///     },
/// );
/// // A somber armament, which has no affinity block at all, at +7.
/// assert_eq!(
///     split_armament_id(1_010_007),
///     ArmamentId {
///         row: 1_010_000,
///         row_with_affinity: 1_010_000,
///         infusion: None,
///         level: 7,
///     },
/// );
/// assert_eq!(infusion_offset(Some("Occult")), Some(1200));
/// ```
pub fn split_armament_id(param_id: u32) -> ArmamentId {
    let level = (param_id % ARMAMENT_LEVEL_STEP) as u16;
    let row_with_affinity = param_id - u32::from(level);
    let index = (row_with_affinity % ARMAMENT_ID_BLOCK / INFUSION_STEP) as usize;
    let (row, infusion) = match INFUSIONS.get(index) {
        // Index 0 is Standard, which the planner writes as an absent field.
        Some(_) if index == 0 => (row_with_affinity, None),
        Some((name, offset)) => (row_with_affinity - offset, Some(*name)),
        // An offset past the table is not an affinity at all, so the id is taken whole rather than
        // having an invented amount subtracted from it.
        None => (row_with_affinity, None),
    };
    ArmamentId {
        row,
        row_with_affinity,
        infusion,
        level,
    }
}

/// What an armament param id is made of, once [`split_armament_id`] has taken it apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArmamentId {
    /// The `EquipParamWeapon` row for this armament without its affinity -- the id the message
    /// repository names it by.
    pub row: u32,
    /// The row the affinity variant occupies, i.e. [`Self::row`] plus the affinity offset and
    /// without the level. This is the id `ReinforceParamWeapon` questions are asked about, because
    /// `reinforceTypeId` is a property of the affinity variant rather than of the base armament.
    pub row_with_affinity: u32,
    /// Affinity name, or `None` for Standard (and for an armament that takes none).
    pub infusion: Option<&'static str>,
    /// Upgrade level as the game counts it: 0..=25 for a regular armament, 0..=10 for a somber
    /// one. That is also the scale the planner's per-slot `upgrade` uses, so it is exported
    /// verbatim; its character-wide `weaponUpgrade` is the one on the other scale.
    pub level: u16,
}

/// The step the upgrade level occupies at the bottom of an armament id.
pub const ARMAMENT_LEVEL_STEP: u32 = 100;

/// The planner's regular-level -> somber-level table, transcribed from the live bundle (`lr`).
///
/// The planner counts every upgrade in regular smithing-stone levels, 0..=25, including for
/// armaments that take Somber Smithing Stones: its `getWeaponUpgradeLevel` maps the character's
/// `weaponUpgrade` through this table (`U_(level, weapon)` -> `lr[level]`) whenever the armament's
/// `upgrade_material` is `Somber Smithing Stone`, and 0 when it is `None`.
///
/// It applies to the character-wide number only. A build whose `weaponUpgrade` is 25 puts a somber
/// armament at the game's +10; the same build's per-slot `upgrade: 10` also means +10, because the
/// planner's slot editor caps that input at the mapped maximum and stores what was typed. Getting
/// the two the wrong way round is silent -- both numbers are in range, and the armament simply
/// comes out at a level nobody asked for.
const SOMBER_LEVEL_FOR_REGULAR: [u16; 26] = [
    0, 0, 1, 1, 1, 2, 2, 3, 3, 3, 4, 4, 5, 5, 5, 6, 6, 7, 7, 7, 8, 8, 9, 9, 9, 10,
];

/// Highest regular level the planner recognises.
pub const MAX_REGULAR_LEVEL: u16 = 25;
/// Highest level a somber armament reaches.
pub const MAX_SOMBER_LEVEL: u16 = 10;

/// The game level a somber armament ends up at when a planner build asks for `regular`.
///
/// ```
/// use er_build_import_core::plan::somber_level_for_regular;
/// assert_eq!(somber_level_for_regular(25), 10);
/// assert_eq!(somber_level_for_regular(17), 7);
/// assert_eq!(somber_level_for_regular(0), 0);
/// // Out of range asks for the most the armament can take rather than nothing.
/// assert_eq!(somber_level_for_regular(99), 10);
/// ```
#[must_use]
pub fn somber_level_for_regular(regular: u16) -> u16 {
    SOMBER_LEVEL_FOR_REGULAR
        .get(usize::from(regular))
        .copied()
        .unwrap_or(MAX_SOMBER_LEVEL)
}

/// The planner level that describes an armament the game holds at somber `level`.
///
/// The inverse of [`somber_level_for_regular`], which is one-to-many, so this returns the highest
/// regular level that maps back. Used for one thing: the document-wide `weaponUpgrade`, which is a
/// regular-scale number and acts as a cap on every slot (`min(slot.upgrade, lr[weaponUpgrade])`).
/// Taking the highest is what keeps a maxed somber armament from being capped below its own level.
///
/// ```
/// use er_build_import_core::plan::{regular_level_for_somber, somber_level_for_regular};
/// assert_eq!(regular_level_for_somber(10), 25);
/// assert_eq!(regular_level_for_somber(7), 19);
/// for somber in 0..=10 {
///     assert_eq!(somber_level_for_regular(regular_level_for_somber(somber)), somber);
/// }
/// ```
#[must_use]
pub fn regular_level_for_somber(somber: u16) -> u16 {
    let mut found = 0;
    let mut regular = 0;
    while regular < SOMBER_LEVEL_FOR_REGULAR.len() {
        if SOMBER_LEVEL_FOR_REGULAR[regular] == somber {
            found = regular as u16;
        }
        regular += 1;
    }
    found
}

/// The block an armament id's affinity offset occupies; the base row is always a multiple of it.
pub const ARMAMENT_ID_BLOCK: u32 = 10_000;
/// Step between consecutive affinities inside that block.
pub const INFUSION_STEP: u32 = 100;

/// Every affinity name the planner uses, in offset order. Exposed so the exporter can prove it
/// knows exactly the set the importer accepts, rather than keeping a second copy that can drift.
pub fn infusion_names() -> impl Iterator<Item = &'static str> {
    INFUSIONS.iter().map(|(name, _)| *name)
}

/// The item id an armament instance must carry to read as `+level` in the player's hands.
///
/// The upgrade level is part of the id, in its last two digits: the character's own weapons come
/// back from `GaitemInsLookupResult::GetItemId` as `12531125` (base `12530000` + Blood `1100` +
/// `25`), and `CSGaitemImp::GetGaItemHandleWeapon` stores whatever id it is handed verbatim
/// through `CSGaitemIns::SetItemIdWithWeaponCategory`. A weapon minted from a bare
/// `base + affinity` id is therefore a `+0` weapon, whatever is written to its `reinforcement`
/// field afterwards.
///
/// The MISREADING this exists to correct. `EquipParamWeapon::GetEntry` normalises its argument to
/// `(paramId / 100) * 100`, and this module used to conclude from that "the game throws the level
/// away, so it can only reach the game as `CSWepGaitemIns::reinforcement`". The normalisation is
/// there for the opposite reason: the last two digits are the level, so the stat row is found by
/// stripping them, and the level is read back off the same id by a different consumer. Dropping it
/// shipped 30 armaments at +0 with `GetReinforcement` cheerfully reporting 25 -- the field was set,
/// nothing read it. The reference exporter this module is ported from sets both halves, and its
/// record layout says so in the first line of the module doc: `dd <id+upgrade> ... <reinforceLv>`.
///
/// `level` must already be clamped to a level this armament has (see the runtime's
/// `ReinforceLevels::clamp`); a somber armament asked for +25 would otherwise name a row that does
/// not exist.
///
/// ```
/// use er_build_import_core::plan::armament_item_id;
/// // Great Stars + Blood, +25.
/// assert_eq!(armament_item_id(12181200, 25), 12181225);
/// // A somber armament, clamped to +10 by the caller.
/// assert_eq!(armament_item_id(11500000, 10), 11500010);
/// // +0 leaves the id exactly as the affinity left it.
/// assert_eq!(armament_item_id(2020100, 0), 2020100);
/// ```
#[must_use]
pub fn armament_item_id(base_with_affinity: u32, level: u16) -> u32 {
    base_with_affinity + u32::from(level)
}

impl Entry {
    /// The bare param row id, with the category nibble stripped.
    pub fn param_id(self) -> u32 {
        self.full_item_id & 0x0FFF_FFFF
    }
}

/// Compute the grants and spell list for `doc` using `catalog`.
pub fn plan(doc: &BuildDoc, catalog: &dyn Catalog) -> Plan {
    let mut out = Plan::default();

    // ARMAMENTS are granted in payload order -- the order the build lists them, which is the order
    // the player sees in their inventory and the only order they can check against the planner page.
    //
    // This used to be two passes, worn-in-the-active-set first and everything else after, and that
    // reordering was load-bearing while the equip resolved a copy through
    // `EquipInventoryData::GetItemInventoryIdx`: several copies of one armament differing only by
    // ash share an item id (the ash lives on the gaitem instance), and
    // `InventoryItemsData::InsertItemIntoLookupMap` keeps the lowest index for a repeated id, so the
    // game always answered with the earliest-granted copy. Granting the worn one first was the only
    // way to make that answer right.
    //
    // It is obsolete now: the equip carries each mint's `GaItemHandle` forward and asks
    // `GetItemIndexByGaitemHandle` (0x14024c460), which names one instance and does not care where
    // in the inventory it sits. The reorder bought nothing after that and cost the user the thing
    // they can actually see -- reported 2026-08-23 against build 94252a868b4f2a, where the two worn
    // armaments were granted at positions 1 and 2 while the payload puts them at `order` 2 and 8.
    for slot in &doc.inventory.slots {
        plan_weapon(doc, catalog, slot, &mut out);
    }
    for slot in &doc.talismans.slots {
        push_simple(catalog, Kind::Talisman, slot, 1, &mut out);
    }
    for slot in &doc.spells.slots {
        match catalog.lookup(Kind::Spell, &slot.name) {
            Some(found) => {
                out.equip_spells.push(found.param_id());
                out.grants.push(Grant {
                    item_id: found.full_item_id,
                    also_known_as: catalog.alternates(Kind::Spell, &slot.name),
                    quantity: 1,
                    reinforce_lv: 0,
                    upgrade_is_character_default: true,
                    weapon_skill: NO_SKILL,
                    label: slot.name.clone(),
                    pot_group: found.pot_group,
                    armament: false,
                });
            }
            None => out.unresolved.push(Unresolved {
                kind: Kind::Spell,
                name: slot.name.clone(),
            }),
        }
    }
    // BTreeMap iteration keeps body parts in a stable order across runs.
    for part in doc.protectors.values() {
        for slot in &part.slots {
            push_simple(catalog, Kind::Protector, slot, 1, &mut out);
        }
    }
    for slot in &doc.items.tools.slots {
        if NEVER_GRANT
            .iter()
            .any(|skip| skip.eq_ignore_ascii_case(&slot.name))
        {
            continue;
        }
        // The one category that is granted in numbers. Everything else in this function passes a
        // literal 1 because one is what the item MEANS: an armament, a piece of armour, a
        // talisman and a great rune are each a single thing to wear, and a sorcery is a single
        // thing to memorise. A consumable is the only kind whose point is the stack.
        let quantity = consumable_quantity(catalog.lookup(Kind::Tool, &slot.name));
        push_simple(catalog, Kind::Tool, slot, quantity, &mut out);
    }
    // The great rune is a goods item like any other and has to be in the inventory before it
    // can be equipped; nothing else in the payload implies it.
    if let Some(rune) = doc.great_rune.as_deref() {
        let slot = Slot {
            name: rune.to_owned(),
            ..Slot::default()
        };
        push_simple(catalog, Kind::GreatRune, &slot, 1, &mut out);
    }
    // A tear is looked up as a tool but is not granted like one. It goes in the physick, which
    // holds exactly two, and the game agrees: every crystal tear row declares `maxNum = 1`. The
    // literal here and `consumable_quantity` would return the same number today; the literal says
    // that one is the answer rather than a value that happens to be one this patch.
    for tear in doc.items.crystal_tears.iter().flatten() {
        let slot = Slot {
            name: tear.clone(),
            ..Slot::default()
        };
        push_simple(catalog, Kind::Tool, &slot, 1, &mut out);
    }
    // Ammunition, which is granted in numbers and is still not a CONSUMABLE. It is looked up in
    // its own catalog because it is its own `EquipParamWeapon` subset (see
    // `catalog::Kind::Ammo`), granted at the engine's own quiver limit rather than at one, and
    // not flagged as an armament: it mints no instance, carries no ash and has no upgrade level.
    for (_, name) in doc.items.ammo.positions() {
        let Some(name) = name else { continue };
        let slot = Slot {
            name: name.to_owned(),
            ..Slot::default()
        };
        let quantity = ammo_quantity(catalog.lookup(Kind::Ammo, name));
        push_simple(catalog, Kind::Ammo, &slot, quantity, &mut out);
    }

    out
}

/// Grant an armament, applying affinity, upgrade level and ash of war.
fn plan_weapon(doc: &BuildDoc, catalog: &dyn Catalog, slot: &Slot, out: &mut Plan) {
    let Some(found) = catalog.lookup(Kind::Weapon, &slot.name) else {
        out.unresolved.push(Unresolved {
            kind: Kind::Weapon,
            name: slot.name.clone(),
        });
        return;
    };
    let Some(offset) = infusion_offset(slot.infusion.as_deref()) else {
        out.unresolved.push(Unresolved {
            kind: Kind::Weapon,
            name: format!("{} (unknown affinity {:?})", slot.name, slot.infusion),
        });
        return;
    };

    // The number is carried forward with its provenance rather than resolved here: whether it
    // needs the somber mapping depends on where it came from, and whether the armament is somber
    // is something only the runtime can measure (see `Grant::upgrade_is_character_default`).
    let (reinforce_lv, upgrade_is_character_default) = match slot.upgrade {
        Some(level) => (level, false),
        None => (doc.weapon_upgrade, true),
    };

    let mut weapon_skill = NO_SKILL;
    if let Some(art) = slot.weapon_art.as_deref()
        && !art.eq_ignore_ascii_case("No Skill")
    {
        match catalog.lookup(Kind::AshOfWar, art) {
            Some(ash) => weapon_skill = GEM_ITEM_CATEGORY | ash.param_id(),
            None => {
                out.unresolved.push(Unresolved {
                    kind: Kind::AshOfWar,
                    name: art.to_owned(),
                });
            }
        }
    }

    out.grants.push(Grant {
        item_id: found.full_item_id + offset,
        // An armament's id already carries affinity and upgrade, so a shared name means genuinely
        // distinct rows; they are offset the same way to stay comparable with `item_id`.
        also_known_as: catalog
            .alternates(Kind::Weapon, &slot.name)
            .into_iter()
            .map(|id| id + offset)
            .collect(),
        quantity: 1,
        reinforce_lv,
        upgrade_is_character_default,
        weapon_skill,
        label: slot.name.clone(),
        pot_group: found.pot_group,
        armament: true,
    });
}

/// Grant a non-armament item, which carries no affinity, upgrade or skill.
fn push_simple(catalog: &dyn Catalog, kind: Kind, slot: &Slot, quantity: u32, out: &mut Plan) {
    match catalog.lookup(kind, &slot.name) {
        Some(found) => out.grants.push(Grant {
            item_id: found.full_item_id,
            also_known_as: catalog.alternates(kind, &slot.name),
            quantity,
            reinforce_lv: 0,
            upgrade_is_character_default: true,
            weapon_skill: NO_SKILL,
            label: slot.name.clone(),
            pot_group: found.pot_group,
            armament: false,
        }),
        None => out.unresolved.push(Unresolved {
            kind,
            name: slot.name.clone(),
        }),
    }
}

/// The ash of war the build wants on one equipped armament slot.
///
/// Exists so the importer can check its own work. A grant that "succeeded" and an equip that
/// "succeeded" still say nothing about whether the weapon in the player's hand carries the right
/// skill -- there are three native hops between the two -- so the runtime reads the arts id back
/// out of the equipped slot and needs to know what it should have found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmamentSkill {
    /// `ChrAsmSlot` the armament is worn in.
    pub slot: i32,
    /// The armament's display name.
    pub weapon: String,
    /// The ash's display name, or `None` when the build asked for no skill.
    pub art: Option<String>,
    /// What [`Grant::weapon_skill`] carries for this armament.
    pub weapon_skill: u32,
}

/// What every equipped armament in `doc` should be holding, for the post-import read-back.
///
/// Slots the build leaves empty are absent; an armament whose ash the catalog could not resolve
/// is still listed, with [`NO_SKILL`], so the read-back reports it rather than skipping it.
pub fn equipped_armament_skills(doc: &BuildDoc, catalog: &dyn Catalog) -> Vec<ArmamentSkill> {
    let mut out = Vec::new();
    for slot in &doc.inventory.slots {
        let Some(index) = slot.equip_index else {
            continue;
        };
        let Some(chr_asm_slot) = crate::equip::armament_slot(index) else {
            continue;
        };
        let art = slot
            .weapon_art
            .as_deref()
            .filter(|art| !art.eq_ignore_ascii_case("No Skill"));
        let weapon_skill = art
            .and_then(|art| catalog.lookup(Kind::AshOfWar, art))
            .map_or(NO_SKILL, |ash| GEM_ITEM_CATEGORY | ash.param_id());
        out.push(ArmamentSkill {
            slot: chr_asm_slot,
            weapon: slot.name.clone(),
            art: art.map(str::to_owned),
            weapon_skill,
        });
    }
    out
}
