//! Reading the live character back out of the game -- the exact inverse of the importer.
//!
//! # Why this is the easy direction
//!
//! The importer's hard problem is name -> id: the planner payload identifies every item by display
//! name, the game answers only id -> name, so [`crate::catalog`] enumerates several thousand rows
//! and inverts the result. Nothing here needs that. Every read below produces an ID, and turning an
//! id into a name is one call to the game's own getter ([`crate::catalog::name_for`]).
//!
//! # What is read, and what is deliberately not
//!
//! The scope is the LOADOUT: what the character is wearing, holding, has memorised, and is levelled
//! to. Not the whole inventory. That is a product decision with two independent justifications --
//! "my build" means the loadout rather than the six hundred crafting materials in the pouch, and the
//! share link carries its payload in the URL, so every item is bytes a browser has to swallow.
//!
//! # Every value here is a read-back, never an assumption
//!
//! The equipment slots are read through `CS::EquipGameData::GetParamIdInSlot`, which is the same
//! oracle the importer verifies its own writes with -- and for the same reason recorded in
//! `equip_native`: the engine's equip call returns void and declines silently, so only the slot's
//! contents are evidence. An id that resolves to no name is dropped rather than guessed at, which
//! makes a garbage read produce a smaller build rather than a wrong one.

use er_build_import_core::catalog::Kind;
use er_build_import_core::equip::{
    AMMO_SLOTS, ARMAMENT_CHR_ASM_SLOTS, CHR_ASM_SLOT_ACCESSORY_1, CHR_ASM_SLOT_AMMO_1,
    CHR_ASM_SLOT_PROTECTOR_HEAD, POUCH_SLOTS, PROTECTOR_PARTS, QUICKBAR_SLOTS,
};
use er_build_import_core::plan::{ARMAMENT_LEVEL_STEP, split_armament_id};

use crate::catalog::{ReinforceLevels, name_for};
use crate::character::player_game_data;

// Every game address this module calls is declared once in `er-game-base::rva` and derived here.
// They are all shared: the importer verifies its writes through the same equipment and spell
// getters, and `er-armament-icons` walks the same three-hop gem chain for the HUD badge. See that
// module for what each one is and for the two out-parameter signatures that are easy to get wrong.
use er_game_base::rva::{
    GET_EQUIP_MAGIC_ID_RVA as GET_EQUIP_MAGIC_ID,
    GET_EQUIPPED_GREATRUNE_RVA as GET_EQUIPPED_GREATRUNE,
    GET_MAGIC_SLOTS_COUNT_RVA as GET_MAGIC_SLOTS_COUNT,
    GET_PARAM_ID_IN_SLOT_RVA as GET_PARAM_ID_IN_SLOT,
    GET_PHYSIC_TEAR_BY_SLOT_RVA as GET_PHYSIC_TEAR_BY_SLOT, WORLD_CHR_MAN_GLOBAL_RVA,
    WORLD_CHR_MAN_PLAYER_INS_OFFSET,
};

use crate::gaitem::{GaitemLookupResult, worn_weapon_handle};

/// `CS::EquipMagicData` pointer inside `EquipGameData`. Declared beside its only two readers
/// rather than centrally: it is a field offset of one struct, not a cross-cutting singleton.
const EQUIP_GAME_DATA_MAGIC_OFFSET: usize = 0x280;

/// `PlayerGameData` field offsets. Bound to the upstream typed layout so a struct change fails the
/// build rather than reading the wrong bytes at runtime.
pub(crate) mod pgd {
    use eldenring::cs::PlayerGameData;

    pub const LEVEL: usize = core::mem::offset_of!(PlayerGameData, level);
    pub const VIGOR: usize = core::mem::offset_of!(PlayerGameData, vigor);
    pub const MIND: usize = core::mem::offset_of!(PlayerGameData, mind);
    pub const ENDURANCE: usize = core::mem::offset_of!(PlayerGameData, endurance);
    pub const STRENGTH: usize = core::mem::offset_of!(PlayerGameData, strength);
    pub const DEXTERITY: usize = core::mem::offset_of!(PlayerGameData, dexterity);
    pub const INTELLIGENCE: usize = core::mem::offset_of!(PlayerGameData, intelligence);
    pub const FAITH: usize = core::mem::offset_of!(PlayerGameData, faith);
    pub const ARCANE: usize = core::mem::offset_of!(PlayerGameData, arcane);
    pub const ARCHETYPE: usize = core::mem::offset_of!(PlayerGameData, archetype);
    /// The character's highest weapon upgrade level, maintained by the game for matchmaking. Raw
    /// `+0..=+25`, not a bucket -- `CS::ChrIns::CheckWeaponLevelMismatch` guards it with `< 0x1a`.
    pub const MATCHING_WEAPON_LEVEL: usize =
        core::mem::offset_of!(PlayerGameData, matching_weapon_level);
    pub const MAX_HP_FLASK: usize = core::mem::offset_of!(PlayerGameData, max_hp_flask);
    pub const MAX_FP_FLASK: usize = core::mem::offset_of!(PlayerGameData, max_fp_flask);

    // The character name is a fixed UTF-16 array with no length field, so its bounds are the two
    // fields on either side of it. Derived here rather than taken from `er_game_base::pgd` because
    // this crate depends on `er-game-base` without the `game-types` feature that compiles that
    // module -- and enabling a feature to reach two constants would change the feature graph of
    // every DLL this crate is linked into.
    pub const NAME: usize = core::mem::offset_of!(PlayerGameData, chr_type)
        + core::mem::size_of::<eldenring::cs::ChrType>();
    pub const NAME_LEN_U16: usize =
        (core::mem::offset_of!(PlayerGameData, gender) - NAME) / core::mem::size_of::<u16>();

    // The same two assertions `er_game_base::pgd` makes, so the two derivations cannot drift.
    const _: () = assert!(NAME == 0x9c);
    const _: () = assert!(NAME_LEN_U16 == 17);

    /// `PlayerGameData::face_data.face_data_buffer` -- the character's appearance, magic first.
    ///
    /// Bound to the upstream layout the same way every other offset here is, so a struct change
    /// breaks the build instead of exporting 288 bytes of something else.
    pub const FACE_DATA_BUFFER: usize = core::mem::offset_of!(PlayerGameData, face_data)
        + core::mem::offset_of!(eldenring::cs::FaceData, face_data_buffer);

    /// Its length: `FACE` + version + declared size + the payload itself.
    pub const FACE_DATA_BUFFER_LEN: usize = core::mem::size_of::<eldenring::cs::FaceDataBuffer>();

    // The buffer the game serialises into a save slot is 288 bytes and starts with `FACE`. Both
    // are asserted rather than assumed because the export writes them into a share link, where a
    // silently shorter blob would be indistinguishable from a valid one.
    const _: () = assert!(FACE_DATA_BUFFER_LEN == 0x120);
}

/// The four bytes every `FaceDataBuffer` begins with. A read that does not start with them is not
/// face data, and exporting it anyway would publish whatever happened to be at that address.
const FACE_DATA_MAGIC: [u8; 4] = *b"FACE";

/// Highest upgrade level `matching_weapon_level` can legitimately hold. A byte above this means we
/// are not looking at live save data, so the field is reported as unknown rather than exported.
const MATCHING_WEAPON_LEVEL_MAX: u8 = 25;

/// One equipped item, as the planner names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadSlot {
    /// Display name, straight from the game's own message repository.
    pub name: String,
    /// Affinity, for armaments only. `None` means Standard or not an armament.
    pub infusion: Option<String>,
    /// Ash of war on this armament, if the equipped gem resolved to one.
    pub weapon_art: Option<String>,
    /// Upgrade level for armaments, as the game counts it -- 0..=25, or 0..=10 for a somber
    /// armament -- which is the scale the planner's per-slot `upgrade` uses too. `None` for
    /// anything that is not an armament.
    pub upgrade: Option<u16>,
    /// The engine's own acquisition-order key for this inventory entry
    /// (`EquipInventoryDataListEntry::sort_id`), or 0 for something read off an equipment slot
    /// rather than out of the inventory. The list is ordered by it, which is what makes the
    /// planner's "Acquisition" sort agree with the game's.
    pub sort_id: u32,
    /// The `EquipParamWeapon` row (affinity included, level stripped) this armament came from.
    /// Armaments only; carried for the diagnostics that need to ask the param table about it.
    pub row_with_affinity: Option<u32>,
    /// The highest level this armament can reach -- 25 for a regular one, 10 for a somber one, 0
    /// for something that does not upgrade at all (ammunition). Measured off the live
    /// `ReinforceParamWeapon` table, and the only sound way to tell somber from regular.
    pub max_upgrade: Option<u16>,
    /// The instance this slot names: its `GaitemHandle`, whole, exactly as the engine stores it.
    ///
    /// Present for armaments only, and for two jobs -- matching a carried armament to the
    /// equipment slot holding it (several copies of one armament share an item id and differ only
    /// by the ash on them, so the id cannot pick a copy), and asking that instance which gem it
    /// carries.
    ///
    /// Not rebuilt from parts. A handle packs a selector, a category and an indexed flag, and a
    /// version of this that reassembled `(category << 28) | (1 << 31) | selector` silently dropped
    /// bits 27:24 -- which resolves a different instance, and a different instance's ash of war is
    /// how an armament ends up reported carrying a skill it cannot take.
    pub gaitem_handle: Option<u32>,
    /// Position within its category, which is also the planner's `equipIndex` -- `None` for an
    /// item the character is merely carrying.
    pub equip_index: Option<u32>,
}

/// What one equipment slot held.
///
/// Three outcomes, not two: a slot can be empty, which is nothing to report, or it can be occupied
/// by an item the message repository would not name, which is a build that comes out short and has
/// to say so. Collapsing those two into `None` is what let the count of unnameable slots sit at
/// zero no matter what happened.
enum SlotRead {
    /// Nothing equipped here.
    Empty,
    /// Something is equipped, and it could not be named.
    Unnamed,
    /// A named item.
    Item(ReadSlot),
}

/// The character, as the planner would describe it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CharacterRead {
    /// Character name, from `PlayerGameData`.
    pub name: String,
    /// Starting class, or `None` when the archetype byte is not one of the ten.
    pub character_class: Option<String>,
    /// `rl` plus the eight attributes, in the planner's own key spelling (note `vit` is endurance).
    pub stats: Vec<(&'static str, i64)>,
    /// Highest weapon upgrade level on the character, which is what the planner's `weaponUpgrade`
    /// means. `None` when the field failed its sanity bound.
    pub weapon_upgrade: Option<u16>,
    /// Armaments the character is carrying, worn ones included and marked by their
    /// [`ReadSlot::equip_index`].
    pub armaments: Vec<ReadSlot>,
    /// Armour, keyed by [`PROTECTOR_PARTS`] -- again everything carried, not only what is worn.
    pub protectors: Vec<(&'static str, ReadSlot)>,
    /// Talismans, carried and worn.
    pub talismans: Vec<ReadSlot>,
    /// Memorised spells, in slot order.
    pub spells: Vec<ReadSlot>,
    /// Arrows and bolts the character is wearing, in `ChrAsmSlot` order -- `Arrow1, Bolt1,
    /// Arrow2, Bolt2`, the engine's interleave, which is also
    /// [`er_build_import_core::model::AMMO_POSITION_KEYS`]'s order. `None` for an empty position.
    ///
    /// Distinct from [`CharacterRead::carried_ammunition`], which counts the quiver in the
    /// backpack and exports nothing. These four are the equipped ones, and they are the only
    /// ammunition the planner has anywhere to put.
    pub ammo: Vec<Option<String>>,
    /// Quickbar contents by position, [`QUICKBAR_SLOTS`] long; `None` for an unassigned position.
    pub quickbar: Vec<Option<String>>,
    /// Pouch contents by position, [`POUCH_SLOTS`] long; `None` for an unassigned position.
    pub pouch: Vec<Option<String>>,
    /// Physick tears, in flask order; `None` for an empty half.
    pub crystal_tears: Vec<Option<String>>,
    /// Equipped great rune.
    pub great_rune: Option<String>,
    /// Whether the character is two-handing.
    pub two_handing: bool,
    /// Crimson and cerulean flask counts.
    pub flask_crimson: u32,
    pub flask_cerulean: u32,
    /// Memory slots the game says the character has -- reported so a truncated spell list is
    /// visibly a capacity limit rather than a failed read.
    pub magic_capacity: u32,
    /// Slots that held an item whose id resolved to no name. Never silently dropped: a build that
    /// came out short says so.
    pub unnamed_slots: usize,
    /// The character's appearance, exactly as the game holds it: the whole `FaceDataBuffer`, magic
    /// first. `None` when the read failed or the magic was wrong, which is the only two ways this
    /// can be anything other than the player's own face.
    pub face_data: Option<Vec<u8>>,
    /// Arrows and bolts the character is carrying. Counted, not exported -- see the ammunition
    /// note in `read_carried`.
    pub carried_ammunition: usize,
    /// Consumables, crafting materials and key items the character is carrying. Counted, not
    /// exported: they are the bulk of an inventory and the planner models only the handful that
    /// sit in the quickbar, so shipping them would put several thousand characters of crafting
    /// material into a URL to say nothing. Counted rather than ignored so the omission is visible.
    pub carried_goods: usize,
    /// Whether the carried-inventory read ran at all. `false` means the lists above are the worn
    /// loadout only -- the honest state to report rather than an empty backpack.
    pub read_whole_inventory: bool,
}

type ParamInSlotFn = unsafe extern "system" fn(usize, i32) -> i32;
type OutParamGetterFn = unsafe extern "system" fn(usize, *mut i32, i32) -> *mut i32;
type MagicIdFn = unsafe extern "system" fn(usize, i32) -> i32;
type SlotsCountFn = unsafe extern "system" fn(usize, usize) -> u32;

/// Read one equipment slot and name it.
///
/// # The level has to come off the id before anything is named
///
/// `GetParamIdInSlot` answers with the id of the instance in the slot, and for an armament that id
/// carries the upgrade level in its last two digits (`16110217` = Cross-Naginata + Keen `200` +
/// `17`). `EquipParamWeapon` has no row for that -- only for `16110200` -- and the game's name
/// getter is an exact `MsgRepositoryImp::LookupEntry`, so asking it about the levelled id answers
/// NULL. That is not a slightly wrong name, it is no name, and this function used to report the
/// slot as unnameable and drop it. Every armament on a finished character is upgraded, so every
/// armament on a finished character vanished from the exported build.
///
/// # Safety
///
/// Game thread, `egd` live, `msg` a live `MsgRepositoryImp*`.
unsafe fn read_slot(
    get: ParamInSlotFn,
    module_base: usize,
    msg: usize,
    egd: usize,
    slot: i32,
    kind: Kind,
    equip_index: Option<u32>,
) -> SlotRead {
    // The GETTER is resolved by the caller, not here, and that is the point. This function is
    // called once per equipment slot; resolving inside it would ask the same question ~30 times
    // per export and, on a build with no mapping, would answer `SlotRead::Empty` thirty times --
    // an exported character wearing nothing, which is a plausible-looking document rather than a
    // failure. `read_character` resolves once and refuses the whole export instead.
    // Safety: the caller's contract.
    let raw = unsafe { get(egd, slot) };
    // An empty slot reads as -1, and the engine also uses large sentinels for "nothing"; anything
    // that is not a plausible row id is simply not an item.
    let Ok(param_id) = u32::try_from(raw) else {
        return SlotRead::Empty;
    };
    if param_id == 0 || param_id == u32::MAX {
        return SlotRead::Empty;
    }
    let (row_id, infusion, upgrade) = if kind == Kind::Weapon {
        let split = split_armament_id(param_id);
        // An empty hand is not an armament. It reads as row 110000 ("Unarmed"), which names
        // itself perfectly well -- see `UNARMED_ARMAMENT_ROW`.
        if split.row == UNARMED_ARMAMENT_ROW {
            return SlotRead::Empty;
        }
        // Verbatim, on the game's own scale. The planner's per-slot `upgrade` is on that same
        // scale -- its editor caps the input at the armament's real maximum (10 for a somber one)
        // and stores what was typed -- so no mapping belongs here. Only its character-wide
        // `weaponUpgrade` is on the regular-stone scale, and that field is written from
        // `matching_weapon_level`, which is already regular.
        (split.row, split.infusion, Some(split.level))
    } else {
        (param_id, None, None)
    };
    // Safety: the caller's contract carries through.
    let Some(name) = (unsafe { name_for(kind, msg, module_base, row_id) }) else {
        return SlotRead::Unnamed;
    };
    let (weapon_art, gaitem_handle) = if kind == Kind::Weapon {
        // Safety: same context; the whole chain is null-checked inside.
        let row_with_affinity = param_id / ARMAMENT_LEVEL_STEP * ARMAMENT_LEVEL_STEP;
        let facts = weapon_facts_for(row_with_affinity);
        let ashes = ash_of_war_arts_rows();
        let art = unsafe {
            read_weapon_art(
                module_base,
                msg,
                slot,
                facts.map(|facts| facts.default_arts),
                facts.map(|facts| facts.wep_type),
                &ashes,
            )
        };
        // Safety: as above -- one read of the player singleton and one engine getter.
        let handle = unsafe { worn_armament_handle(module_base, slot) };
        (art, handle)
    } else {
        (None, None)
    };
    SlotRead::Item(ReadSlot {
        name,
        infusion: infusion.map(str::to_owned),
        weapon_art,
        sort_id: 0,
        upgrade,
        row_with_affinity: (kind == Kind::Weapon)
            .then(|| param_id / ARMAMENT_LEVEL_STEP * ARMAMENT_LEVEL_STEP),
        max_upgrade: None,
        equip_index,
        gaitem_handle,
    })
}

/// The gaitem handle of the armament worn in `slot`, as `(selector, category)`.
///
/// The same pair the inventory entries carry, so the merge can tell which copy of an armament is
/// the worn one -- several copies share an item id and differ only by the ash mounted on them.
///
/// # Safety
///
/// Game thread; `module_base` the loaded image base.
unsafe fn worn_armament_handle(module_base: usize, slot: i32) -> Option<u32> {
    // Safety: fault-checked reads of the singleton slot and one offset inside it.
    let world = unsafe {
        er_game_base::mem::safe_read_usize(er_game_base::mem::game_data_addr(
            module_base,
            WORLD_CHR_MAN_GLOBAL_RVA,
            "WORLD_CHR_MAN_GLOBAL_RVA",
        ))
    }?;
    if world == 0 {
        return None;
    }
    // Safety: as above.
    let player =
        unsafe { er_game_base::mem::safe_read_usize(world + WORLD_CHR_MAN_PLAYER_INS_OFFSET) }?;
    if player == 0 {
        return None;
    }
    // Safety: the caller's contract; the slot came from the caller's fixed table.
    unsafe { worn_weapon_handle(module_base, player, slot) }
}

/// The ash of war on the armament in `slot`, named -- see [`worn_armament`] for how it is read.
///
/// # Safety
///
/// Game thread; `module_base` the loaded image base and `msg` a live `MsgRepositoryImp*`.
unsafe fn read_weapon_art(
    module_base: usize,
    msg: usize,
    slot: i32,
    default_arts: Option<u32>,
    wep_type: Option<u16>,
    ashes: &std::collections::BTreeSet<u32>,
) -> Option<String> {
    // Safety: the caller's contract.
    let worn = unsafe { worn_armament(module_base, slot) }?;
    let arts_id = worn.arts_id?;
    // The same tests the carried read applies -- see `is_ash_of_war` and `mounted_ash`.
    if !is_ash_of_war(arts_id, default_arts, ashes) {
        return None;
    }
    if let Some(wep_type) = wep_type
        && let Some(handle) = unsafe { worn_armament_handle(module_base, slot) }
        // Safety: the handle is the engine's own.
        && let Some(mut lookup) = unsafe { GaitemLookupResult::from_handle(module_base, handle) }
        // Safety: as above.
        && let Some(gem_row) = unsafe { lookup.mounted_gem_row(module_base) }
        && !crate::gem_mount::gem_can_mount(gem_row, wep_type)
    {
        return None;
    }
    // Safety: as above.
    unsafe { name_for(Kind::AshOfWar, msg, module_base, arts_id) }
}

/// What the armament worn in `slot` actually is: the instance's own item id, and the
/// `SwordArtsParam` row it carries.
///
/// Both halves matter, and reporting only the second is what made the last failure unreadable.
/// A slot that holds the right item but the wrong ash and a slot that holds a different armament
/// entirely produce the same "wrong arts row" -- and several copies of one armament, differing
/// only by the ash mounted on them, share one item id, so even the id alone cannot pick a copy.
/// With both in hand the log line adjudicates itself instead of listing what it might have been.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WornArmament {
    /// Category-tagged item id of the instance in the slot.
    pub item_id: u32,
    /// The `SwordArtsParam` row it carries, or `None` when it reports none.
    pub arts_id: Option<u32>,
}

impl WornArmament {
    /// The upgrade level the player sees on this armament, read off the id's last two digits.
    ///
    /// This is the oracle for "+25 or +0", and it is deliberately not
    /// `GaitemLookupResult::GetReinforcement`: that field can read 25 on a weapon the player sees
    /// as +0, which is exactly the failure it hid once. The id is what the menu renders from --
    /// the character's own weapons come back as `12531125`, base `12530000` + Blood `1100` + `25`.
    #[must_use]
    pub fn level(self) -> u16 {
        (self.item_id % 100) as u16
    }

    /// The armament's identity without its upgrade level: base row plus affinity.
    ///
    /// The game's own normalisation (`EquipParamWeapon::GetEntry` looks up `(paramId / 100) * 100`)
    /// and the only sound way to ask "is this the armament the plan placed here", since the plan
    /// names an armament and the level is a separate dimension of it.
    #[must_use]
    pub fn armament_identity(self) -> u32 {
        self.item_id / 100 * 100
    }
}

/// Read the armament worn in `slot`, or `None` when the slot is empty.
///
/// Three native hops, none of them skippable: the slot names a gaitem handle, the handle names a
/// gaitem instance, and only the instance knows which gem is in the weapon. Deriving the arts id
/// from the weapon id instead (the menu path's `arts_id * 100` heuristic) misses every weapon whose
/// gem id is not derived that way.
///
/// `slot` is a `ChrAsmSlot` in exactly the numbering [`ARMAMENT_CHR_ASM_SLOTS`] and
/// `GetParamIdInSlot` use -- the handle getter bottoms out in a single
/// `chrAsm->equipmentGaItemHandles[slot]`, so the two read-backs are asking about the same slot.
///
/// # Safety
///
/// Game thread; `module_base` the loaded image base.
pub unsafe fn worn_armament(module_base: usize, slot: i32) -> Option<WornArmament> {
    let player = {
        // Safety: a fault-checked read of the singleton slot and one offset inside it.
        let world = unsafe {
            er_game_base::mem::safe_read_usize(er_game_base::mem::game_data_addr(
                module_base,
                WORLD_CHR_MAN_GLOBAL_RVA,
                "WORLD_CHR_MAN_GLOBAL_RVA",
            ))
        }?;
        if world == 0 {
            return None;
        }
        // Safety: as above.
        let player =
            unsafe { er_game_base::mem::safe_read_usize(world + WORLD_CHR_MAN_PLAYER_INS_OFFSET) }?;
        if player == 0 {
            return None;
        }
        player
    };
    // Safety: the caller's contract; the slot came from the caller's fixed table.
    let handle = unsafe { worn_weapon_handle(module_base, player, slot) }?;
    // Safety: as above. The record is the engine's own, at the engine's own length -- see
    // `crate::gaitem` for what a short one costs.
    let mut lookup = unsafe { GaitemLookupResult::from_handle(module_base, handle) }?;
    // Safety: as above.
    let arts_id = unsafe { lookup.sword_arts_id(module_base) };
    Some(WornArmament {
        item_id: lookup.item_id,
        arts_id,
    })
}

/// The `SwordArtsParam` row the armament in `slot` is actually holding, by way of its equipped gem.
///
/// # Safety
///
/// Game thread; `module_base` the loaded image base.
pub unsafe fn equipped_weapon_arts_id(module_base: usize, slot: i32) -> Option<u32> {
    // Safety: the caller's contract.
    unsafe { worn_armament(module_base, slot) }?.arts_id
}

/// Read the whole character.
///
/// Returns `None` only when the character is not in the world at all -- every finer-grained failure
/// leaves a field empty and is counted in [`CharacterRead::unnamed_slots`], because a build that is
/// missing one talisman is still worth sharing and a build that is silently missing one is not.
///
/// # Safety
///
/// Game task thread, params streamed, character in the world -- the same three preconditions the
/// importer's `tick` checks before it runs.
pub unsafe fn read_character(module_base: usize, msg: usize, egd: usize) -> Option<CharacterRead> {
    // Safety: the caller's contract.
    let pgd = unsafe { player_game_data() }?;
    // Resolved once, before anything is read. Every equipment slot below is answered by this one
    // getter, so on a build where it has no mapping the honest answer is that the character
    // cannot be read at all -- `None`, which the caller reports. Answering slot by slot would
    // produce a fully-formed export document of a character wearing nothing, and a wrong document
    // is worse than no document because it can be uploaded and shared.
    let param_in_slot = crate::native::resolve(
        module_base,
        GET_PARAM_ID_IN_SLOT,
        "CS::EquipGameData::GetParamIdInSlot",
    )?;
    // Safety: resolved for the running build immediately above.
    let param_in_slot: ParamInSlotFn = unsafe { core::mem::transmute(param_in_slot) };
    let mut out = CharacterRead::default();

    // Safety: plain reads of live save data at offsets bound to the upstream typed layout.
    unsafe {
        let read_i32 = |offset: usize| *((pgd + offset) as *const i32) as i64;
        out.stats = vec![
            ("rl", read_i32(pgd::LEVEL)),
            ("vig", read_i32(pgd::VIGOR)),
            ("mnd", read_i32(pgd::MIND)),
            // The planner calls Endurance "vit". Verified, not inferred.
            ("vit", read_i32(pgd::ENDURANCE)),
            ("str", read_i32(pgd::STRENGTH)),
            ("dex", read_i32(pgd::DEXTERITY)),
            ("int", read_i32(pgd::INTELLIGENCE)),
            ("fth", read_i32(pgd::FAITH)),
            ("arc", read_i32(pgd::ARCANE)),
        ];
        let archetype = *((pgd + pgd::ARCHETYPE) as *const u8);
        out.character_class =
            er_build_import_core::class::class_for_archetype(archetype).map(str::to_owned);
        let upgrade = *((pgd + pgd::MATCHING_WEAPON_LEVEL) as *const u8);
        out.weapon_upgrade = (upgrade <= MATCHING_WEAPON_LEVEL_MAX).then_some(u16::from(upgrade));
        out.flask_crimson = u32::from(*((pgd + pgd::MAX_HP_FLASK) as *const u8));
        out.flask_cerulean = u32::from(*((pgd + pgd::MAX_FP_FLASK) as *const u8));
    }
    // Safety: the name is read through the fault-checked reader, so an unmapped page yields a
    // short name rather than a fault.
    out.name = unsafe { read_character_name(pgd) };
    // Safety: as above -- a fault-checked read of a fixed-length field at a derived offset.
    out.face_data = unsafe { read_face_data(pgd) };

    let mut unnamed = 0usize;

    // Armaments, in the planner's index order rather than the engine's interleaved one.
    for (equip_index, slot) in ARMAMENT_CHR_ASM_SLOTS.into_iter().enumerate() {
        let equip_index = equip_index as u32;
        // Safety: the caller's contract.
        let read = unsafe {
            read_slot(
                param_in_slot,
                module_base,
                msg,
                egd,
                slot,
                Kind::Weapon,
                Some(equip_index),
            )
        };
        match read {
            SlotRead::Item(item) => out.armaments.push(item),
            SlotRead::Unnamed => unnamed += 1,
            SlotRead::Empty => {}
        }
    }
    // Armour: head, chest, hands, legs, consecutive from ProtectorHead.
    for (offset, part) in PROTECTOR_PARTS.into_iter().enumerate() {
        let slot = CHR_ASM_SLOT_PROTECTOR_HEAD + offset as i32;
        // Safety: as above.
        let read = unsafe {
            read_slot(
                param_in_slot,
                module_base,
                msg,
                egd,
                slot,
                Kind::Protector,
                Some(0),
            )
        };
        match read {
            SlotRead::Item(item) => out.protectors.push((part, item)),
            SlotRead::Unnamed => unnamed += 1,
            SlotRead::Empty => {}
        }
    }
    // Talismans.
    for index in 0..4i32 {
        let slot = CHR_ASM_SLOT_ACCESSORY_1 + index;
        // Safety: as above.
        let read = unsafe {
            read_slot(
                param_in_slot,
                module_base,
                msg,
                egd,
                slot,
                Kind::Talisman,
                Some(index as u32),
            )
        };
        match read {
            SlotRead::Item(item) => out.talismans.push(item),
            SlotRead::Unnamed => unnamed += 1,
            SlotRead::Empty => {}
        }
    }

    // Ammunition, through the same getter every other ChrAsm equipment slot goes through -- and
    // in the engine's order, `Arrow1 = 6, Bolt1 = 7, Arrow2 = 8, Bolt2 = 9`, which interleaves the
    // two kinds while the planner's UI groups them. Walking this loop in the planner's grouped
    // order would put every bolt in an arrow position, so the order is the shared table's.
    //
    // `Kind::Ammo` rather than `Kind::Weapon` on purpose: arrows are `EquipParamWeapon` rows, but
    // they carry no upgrade level in the last two digits and no ash of war, so the armament split
    // has nothing to do and the gem chain has nothing to find. The name getter is the same one.
    for index in 0..AMMO_SLOTS {
        let slot = CHR_ASM_SLOT_AMMO_1 + index as i32;
        // Safety: as above.
        let read = unsafe {
            read_slot(
                param_in_slot,
                module_base,
                msg,
                egd,
                slot,
                Kind::Ammo,
                // Ammo has no `equipIndex` in the payload at all -- the key is the position -- so
                // there is nothing for the read to carry here.
                None,
            )
        };
        out.ammo.push(match read {
            SlotRead::Item(item) => Some(item.name),
            SlotRead::Unnamed => {
                unnamed += 1;
                None
            }
            SlotRead::Empty => None,
        });
    }

    // Quickbar and pouch, read straight out of `EquipGameData::equipmentEntries`.
    // Safety: as above -- fault-checked reads at offsets the compiler derived from the upstream
    // struct and pinned to the engine's own getter, see `entries`.
    unsafe { read_quick_and_pouch(module_base, msg, egd, &mut out, &mut unnamed) };

    // Spells. The capacity is asked for rather than assumed: it accounts for Memory Stones and
    // talismans, and the engine clamps it to 14.
    // Safety: one pointer read at the verified EquipGameData offset.
    let emd = unsafe { *((egd + EQUIP_GAME_DATA_MAGIC_OFFSET) as *const usize) };
    // Resolved for the running build, and only once a character actually has magic data. A
    // refusal leaves `magic_capacity` at zero and the spell list empty, which is the same shape
    // as a character with no spells -- so it is said, once, by `crate::native`, rather than left
    // to be read off a silently short list.
    if emd != 0
        && let Ok([slots_count, magic_id]) = crate::native::resolve_all(
            module_base,
            [
                (
                    GET_MAGIC_SLOTS_COUNT,
                    "CS::EquipMagicData::GetMagicSlotsCount",
                ),
                (GET_EQUIP_MAGIC_ID, "GetEquipMagicId"),
            ],
        )
    {
        // Safety: both addresses were resolved for the running build immediately above.
        let slots_count: SlotsCountFn = unsafe { core::mem::transmute(slots_count) };
        // Safety: as above.
        let magic_id: MagicIdFn = unsafe { core::mem::transmute(magic_id) };
        // Safety: a null SpecialEffect means "derive it from the player".
        out.magic_capacity = unsafe { slots_count(emd, 0) };
        for slot in 0..out.magic_capacity.min(i32::MAX as u32) as i32 {
            // Safety: slot is within the capacity the engine just reported.
            let raw = unsafe { magic_id(emd, slot) };
            let Ok(row_id) = u32::try_from(raw) else {
                continue;
            };
            if row_id == 0 || row_id == u32::MAX {
                continue;
            }
            // Safety: the caller's contract.
            match unsafe { name_for(Kind::Spell, msg, module_base, row_id) } {
                Some(name) => out.spells.push(ReadSlot {
                    name,
                    infusion: None,
                    weapon_art: None,
                    // Memorisation order is the slot index, which the loop below carries in
                    // `equip_index`; a spell is not an inventory entry and has no acquisition key.
                    sort_id: 0,
                    upgrade: None,
                    row_with_affinity: None,
                    max_upgrade: None,
                    equip_index: Some(slot as u32),
                    gaitem_handle: None,
                }),
                None => unnamed += 1,
            }
        }
    }

    // Physick. The field holds category-tagged ids (a game-filled flask reads back as e.g.
    // `0x40001FC1`), so the nibble is masked off before the row is named.
    // Resolved for the running build; an out-parameter getter, which is why the destination is
    // ours. On a refusal the two tear slots are pushed as `None` -- the same value an empty flask
    // produces. That is a known imprecision and it is bounded by the fact that `crate::native`
    // has already logged the getter as unavailable for this session; there is no third state in
    // `Vec<Option<String>>` to put a refusal in without changing the exported document's shape.
    let get_tear = crate::native::resolve(
        module_base,
        GET_PHYSIC_TEAR_BY_SLOT,
        "CS::EquipGameData::GetPhysicTearBySlot",
    );
    for index in 0..2i32 {
        let tear = get_tear.and_then(|get_tear| {
            // Safety: resolved for the running build immediately above.
            let get_tear: OutParamGetterFn = unsafe { core::mem::transmute(get_tear) };
            let mut raw = -1i32;
            // Safety: our own slot, and the getter writes exactly one int through it.
            unsafe { get_tear(egd, &raw mut raw, index) };
            u32::try_from(raw)
                .ok()
                .filter(|id| *id != 0 && *id != u32::MAX)
                // Safety: the caller's contract.
                .and_then(|id| unsafe {
                    name_for(Kind::Tool, msg, module_base, id & ITEM_ID_ROW_MASK)
                })
        });
        out.crystal_tears.push(tear);
    }

    // Great rune. Three arguments: the outer wrapper never writes R8, so the slot passes straight
    // through to `GetEquippedGreatrune(EquipItemData*, int *out, int slot)`, whose body begins
    // `*out = -1; if (slot == 0 && ...)`. Calling it with two leaves R8 holding whatever the call
    // site had and it reports -1 no matter what is equipped.
    // Resolved for the running build; the out slot is ours and outlives the call.
    out.great_rune = crate::native::resolve(
        module_base,
        GET_EQUIPPED_GREATRUNE,
        "CS::EquipItemData::GetEquippedGreatrune",
    )
    .and_then(|get_rune| {
        // Safety: resolved for the running build immediately above.
        let get_rune: OutParamGetterFn = unsafe { core::mem::transmute(get_rune) };
        let mut rune = -1i32;
        // Safety: as above.
        unsafe { get_rune(egd, &raw mut rune, 0) };
        u32::try_from(rune)
            .ok()
            .filter(|id| *id != 0 && *id != u32::MAX)
            // Safety: the caller's contract.
            .and_then(|id| unsafe {
                name_for(Kind::GreatRune, msg, module_base, id & ITEM_ID_ROW_MASK)
            })
    });

    // Two-handing, from the same `ChrAsm::equipment.arm_style` the engine renders from.
    // Safety: a read of one enum-sized field at an offset bound to the upstream layout.
    out.two_handing = unsafe { read_arm_style(egd) }.is_some_and(|style| {
        style == ARM_STYLE_LEFT_BOTH_HANDS || style == ARM_STYLE_RIGHT_BOTH_HANDS
    });

    out.unnamed_slots = unnamed;
    // Safety: the caller's contract -- game task thread, params streamed, live `egd`/`msg`.
    unsafe { read_carried(module_base, msg, egd, &mut out) };
    Some(out)
}

/// `EquipGameData::equipmentEntries`, and the two runs of it the quickbar and the pouch occupy.
///
/// # A struct offset is the only failure here that says nothing
///
/// A wrong function address is refused by the resolver and logged; a wrong field offset returns
/// the neighbouring field, plausible and silent, forever. So these are derived three ways and all
/// three agree:
///
/// 1. **The upstream typed layout.** `offset_of!` computes them, so a `ChrAsmEquipEntries` or
///    `EquipGameData` change upstream breaks the build instead of the read.
/// 2. **The engine's own getter, on both images.** `CS::EquipGameData::GetItemIdByQuickSlotIndex`
///    (`0x140247ee0`) is `add rcx, 0x348` and a tail call to `0x14024ba10`, whose body is
///    `cmp r8d, 9 / ja / lea eax, [r8+0x16] / mov eax, [rcx+r8*4]` -- and the pouch reader two
///    instructions later is the same with `cmp r8d, 5 / lea eax, [r8+0x20]`. Disassembled out of
///    `eldenring-deobf.bin` (1.16.2) and `eldenring-deobf-1.17.bin` and byte-identical in both, so
///    the base is `0x348`, the quickbar run starts at entry `0x16` and the pouch run at `0x20` on
///    the installed build as well as the one the symbols came from.
/// 3. **The neighbour.** The struct's 38 `int`s end at `0x348 + 0x98 = 0x3e0`, and
///    `EquipGameData::physicTears` at `0x3e4` is already proven at runtime by
///    `equip_native::EQUIP_GAME_DATA_PHYSIC_TEARS`.
///
/// The `assert!`s below are that agreement made executable: (1) is what computes the values and
/// (2) is what they are checked against, so the two cannot drift apart quietly.
mod entries {
    use eldenring::cs::{ChrAsmEquipEntries, EquipGameData};
    use er_build_import_core::equip::{CHR_ASM_SLOT_QUICK_BASE, QUICKBAR_SLOTS};

    /// `EquipGameData::equipmentEntries`.
    pub const BASE: usize = core::mem::offset_of!(EquipGameData, equipment_entries);
    /// `ChrAsmEquipEntries::quickItem1`, i.e. `ChrAsmSlot` `0x16`.
    pub const QUICKBAR: usize = BASE + core::mem::offset_of!(ChrAsmEquipEntries, quick_tems);
    /// `ChrAsmEquipEntries::pouch1`, i.e. `ChrAsmSlot` `0x20`.
    pub const POUCH: usize = BASE + core::mem::offset_of!(ChrAsmEquipEntries, pouch);
    /// One entry: a category-tagged item id.
    pub const STRIDE: usize = core::mem::size_of::<u32>();

    /// `ChrAsmSlot` of quickbar position 0, as an array index.
    const QUICKBAR_ENTRY: usize = CHR_ASM_SLOT_QUICK_BASE as usize;

    // The literals here are the engine's, read off both flat images in the doc comment above; the
    // values they are compared against are the compiler's, from the upstream struct. Neither side
    // can move without the other noticing.
    const _: () = assert!(BASE == 0x348);
    const _: () = assert!(QUICKBAR_ENTRY == 0x16);
    const _: () = assert!(QUICKBAR == BASE + QUICKBAR_ENTRY * STRIDE);
    // The pouch follows the quickbar with no gap, which is what makes the planner's single
    // `equipIndex` axis (`0..10` quickbar, `10..16` pouch) a contiguous run of native slots.
    const _: () = assert!(POUCH == QUICKBAR + QUICKBAR_SLOTS * STRIDE);
    const _: () = assert!(POUCH == BASE + 0x20 * STRIDE);
}

/// The item-category nibble of a category-tagged item id, and the value that means goods.
///
/// `ItemCategory::Goods = 4`, in the top four bits. Every quickbar and pouch entry is a goods row
/// -- consumables, the flasks, the crafting kit -- so a category that is not goods is not
/// something the goods name getter can name, and asking it anyway would answer with whatever row
/// of `EquipParamGoods` happens to share the number.
const ITEM_ID_CATEGORY_MASK: u32 = 0xF000_0000;
const ITEM_ID_CATEGORY_GOODS: u32 = 0x4000_0000;

/// What one `ChrAsmEquipEntries` position held. The same three outcomes as [`SlotRead`], without
/// the payload a ChrAsm equipment entry carries: a quickbar position is a name and nothing else.
enum EntryRead {
    /// Nothing assigned here.
    Empty,
    /// Something is assigned, and it could not be named.
    Unnamed,
    /// A named item.
    Named(String),
}

/// Read one `ChrAsmEquipEntries` position and name it.
///
/// # Safety
///
/// Game thread, `egd` live, `msg` a live `MsgRepositoryImp*`.
unsafe fn read_entry(module_base: usize, msg: usize, address: usize) -> EntryRead {
    // Safety: fault-checked; an unmapped page answers `None` instead of taking the game down.
    let Some(raw) = (unsafe { er_game_base::mem::safe_read_i32(address) }) else {
        return EntryRead::Empty;
    };
    // REINTERPRETED, not converted. The field is an `OptionalItemId`, i.e. a `u32` whose top
    // nibble is the category, and the only 32-bit fault-checked reader available returns `i32`.
    // A `try_from` would reject every id with bit 31 set -- the Gem tag, `0x8` -- and report it
    // as an empty position, which is the one answer that is never worth reporting. The empty
    // sentinel is exactly `OptionalItemId::NONE`, and it is tested for by value below.
    let item_id = raw as u32;
    if item_id == 0 || item_id == u32::MAX {
        return EntryRead::Empty;
    }
    if item_id & ITEM_ID_CATEGORY_MASK != ITEM_ID_CATEGORY_GOODS {
        // Not empty and not nameable by this GETTER, which is a read worth counting rather than
        // one worth guessing at.
        return EntryRead::Unnamed;
    }
    // Safety: the caller's contract.
    match unsafe { name_for(Kind::Tool, msg, module_base, item_id & ITEM_ID_ROW_MASK) } {
        Some(name) => EntryRead::Named(name),
        None => EntryRead::Unnamed,
    }
}

/// Fill [`CharacterRead::quickbar`] and [`CharacterRead::pouch`], counting what could not be named.
///
/// Both lists come out at their full length with holes for the empty positions, because the
/// position is the whole meaning here -- an item's index in this vector is the `equipIndex` the
/// planner will be given, so a list that skipped the empties would shift every later item onto
/// somebody else's slot.
///
/// # Safety
///
/// Game thread, `egd` a live `EquipGameData*`, `msg` a live `MsgRepositoryImp*`.
unsafe fn read_quick_and_pouch(
    module_base: usize,
    msg: usize,
    egd: usize,
    out: &mut CharacterRead,
    unnamed: &mut usize,
) {
    for (base, count, into) in [
        (entries::QUICKBAR, QUICKBAR_SLOTS, &mut out.quickbar),
        (entries::POUCH, POUCH_SLOTS, &mut out.pouch),
    ] {
        for index in 0..count {
            // Safety: the caller's contract; the address is one entry inside a fixed-size array of
            // a live struct, at an offset the compiler derived and the asserts pinned.
            let read =
                unsafe { read_entry(module_base, msg, egd + base + index * entries::STRIDE) };
            into.push(match read {
                EntryRead::Named(name) => Some(name),
                EntryRead::Unnamed => {
                    *unnamed += 1;
                    None
                }
                EntryRead::Empty => None,
            });
        }
    }
}

/// Read the character's name out of `PlayerGameData`.
///
/// Stops at the first NUL and at the field's own bound, so neither a name that fills the array nor
/// one whose terminator was never written can run past it.
///
/// # Safety
///
/// `pgd` must be a live `PlayerGameData*`.
pub(crate) unsafe fn read_character_name(pgd: usize) -> String {
    let mut units = Vec::with_capacity(pgd::NAME_LEN_U16);
    for index in 0..pgd::NAME_LEN_U16 {
        // Safety: fault-checked; an unreadable address ends the name instead of faulting.
        let Some(unit) = (unsafe { er_game_base::mem::safe_read_u16(pgd + pgd::NAME + index * 2) })
        else {
            break;
        };
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    String::from_utf16(&units).unwrap_or_default()
}

/// Everything in the character's inventory, named -- the backpack, not just what is worn.
///
/// # Why this is a second pass rather than a replacement for the equipment read
///
/// The equipment slots say what is worn and in which position; the inventory says what is held.
/// Elden Ring keeps a worn item in the inventory too, so the two overlap, and the merge below
/// takes the inventory as the list and the equipment slots as the annotation. Written this way
/// round, a character whose inventory cannot be read still exports its loadout exactly as before
/// -- [`CharacterRead::read_whole_inventory`] says which of the two happened.
///
/// # What is left out, and counted instead
///
/// Goods -- consumables, crafting materials, key items -- are counted into
/// [`CharacterRead::carried_goods`] and not exported. A finished character carries several hundred
/// of them, the planner models only the few that sit in the quickbar, and the payload rides in a
/// URL. Gems (ashes of war) are skipped for the same reason: the ash that matters is the one
/// mounted on an armament, and that is exported with the armament.
///
/// # Safety
///
/// Game task thread, params streamed, `egd` a live `EquipGameData*` and `msg` a live
/// `MsgRepositoryImp*`.
unsafe fn read_carried(module_base: usize, msg: usize, egd: usize, out: &mut CharacterRead) {
    use eldenring::cs::{EquipGameData, EquipInventoryData, ItemCategory};

    /// `EquipGameData::equip_inventory_data`, from the upstream typed layout.
    const INVENTORY_OFFSET: usize = core::mem::offset_of!(EquipGameData, equip_inventory_data);

    // Safety: the caller's contract -- `egd` is the live `EquipGameData` and the inventory is a
    // by-value member of it, so this is a field address rather than a pointer to follow.
    let inventory = unsafe { &*((egd + INVENTORY_OFFSET) as *const EquipInventoryData) }
        .items_data
        .items();

    let parts = protector_parts();
    let facts = weapon_facts();
    let ashes = ash_of_war_arts_rows();
    let levels = ReinforceLevels::read();
    let mut maxima: std::collections::BTreeMap<u32, u16> = std::collections::BTreeMap::new();
    let mut armaments = Vec::new();
    let mut protectors: Vec<(&'static str, ReadSlot)> = Vec::new();
    let mut talismans = Vec::new();

    for entry in inventory {
        let item_id = entry.item_id;
        let row = item_id.param_id();
        match item_id.category() {
            ItemCategory::Weapon => {
                let split = split_armament_id(row);
                // Ammunition is not an armament. Arrows and bolts are `EquipParamWeapon` rows, so
                // they arrive in this list, and a quiver of them is most of a real inventory --
                // 68 of the 123 entries the first whole-inventory export produced. The planner
                // keeps ammo in its own `items.ammo` map, drops it from `inventory` on import,
                // and the only thing it does in a share link is make the URL longer.
                let known = facts.get(&split.row_with_affinity).copied();
                if known.is_some_and(|facts| is_ammunition(facts.wep_type)) {
                    out.carried_ammunition += 1;
                    continue;
                }
                // Safety: the caller's contract carries through.
                let Some(name) = (unsafe { name_for(Kind::Weapon, msg, module_base, split.row) })
                else {
                    continue;
                };
                if split.row == UNARMED_ARMAMENT_ROW {
                    continue;
                }
                // The raw 32 bits, read off the entry itself: `EquipInventoryDataListEntry` is
                // `#[repr(C)]` with the handle first, so this is the field, not a reconstruction.
                // Safety: `entry` is a live engine record and the read is of its first word.
                let handle = unsafe {
                    *(std::ptr::from_ref::<eldenring::cs::EquipInventoryDataListEntry>(entry)
                        .cast::<u32>())
                };
                // Safety: the handle came out of the engine's own inventory entry, and the whole
                // lookup chain is null-checked inside.
                let weapon_art = unsafe {
                    mounted_ash(
                        module_base,
                        msg,
                        handle,
                        known.map(|facts| facts.default_arts),
                        known.map(|facts| facts.wep_type),
                        &ashes,
                    )
                };
                let (copies, clamped) = copies_of(entry.quantity);
                if clamped {
                    crate::log_line(&format!(
                        "[build-export] {name:?} says it is held {} times; exporting {MAX_COPIES_PER_ENTRY}",
                        entry.quantity
                    ));
                }
                let slot = ReadSlot {
                    name,
                    infusion: split.infusion.map(str::to_owned),
                    weapon_art,
                    upgrade: Some(split.level),
                    sort_id: entry.sort_id,
                    row_with_affinity: Some(split.row_with_affinity),
                    max_upgrade: Some(max_upgrade(&levels, &mut maxima, split.row_with_affinity)),
                    equip_index: None,
                    gaitem_handle: Some(handle),
                };
                armaments.extend(std::iter::repeat_n(slot, copies as usize));
            }
            ItemCategory::Protector => {
                // Safety: as above.
                let Some(name) = (unsafe { name_for(Kind::Protector, msg, module_base, row) })
                else {
                    continue;
                };
                let Some(part) = parts.get(&row).copied() else {
                    continue;
                };
                let (copies, _) = copies_of(entry.quantity);
                let slot = (
                    part,
                    ReadSlot {
                        name,
                        infusion: None,
                        weapon_art: None,
                        upgrade: None,
                        sort_id: entry.sort_id,
                        row_with_affinity: None,
                        max_upgrade: None,
                        equip_index: None,
                        gaitem_handle: None,
                    },
                );
                protectors.extend(std::iter::repeat_n(slot, copies as usize));
            }
            ItemCategory::Accessory => {
                // Safety: as above.
                let Some(name) = (unsafe { name_for(Kind::Talisman, msg, module_base, row) })
                else {
                    continue;
                };
                let (copies, _) = copies_of(entry.quantity);
                let slot = ReadSlot {
                    name,
                    infusion: None,
                    weapon_art: None,
                    upgrade: None,
                    sort_id: entry.sort_id,
                    row_with_affinity: None,
                    max_upgrade: None,
                    equip_index: None,
                    gaitem_handle: None,
                };
                talismans.extend(std::iter::repeat_n(slot, copies as usize));
            }
            ItemCategory::Goods => out.carried_goods += 1,
            ItemCategory::Gem => {}
        }
    }

    // Acquisition order is the engine'S, not the array'S. The inventory array is in slot order --
    // pick two items up and discard the first and the next one lands in the hole -- while the game
    // sorts the equipment menu's "Order of Acquisition" by `sort_id`, a counter it hands out as
    // items arrive. The planner's own "Acquisition" sort is `(a.order || 0) - (b.order || 0)`,
    // i.e. it renders whatever order this export writes, so ordering by `sort_id` here is what
    // makes the two agree.
    armaments.sort_by_key(|item| item.sort_id);
    talismans.sort_by_key(|item| item.sort_id);
    protectors.sort_by_key(|(_, item)| item.sort_id);

    // Every copy is exported, duplicates included. Two identical armaments is a real build -- it
    // is how a character dual-wields one weapon -- and rows of the same armour piece are what the
    // player sees in their own inventory. Collapsing them made a shorter link that described a
    // different character, which is not a trade this export is allowed to make.
    crate::log_line(&format!(
        "[build-export] inventory: {} armament rows, {} talisman rows, {} armour rows (a stacked \
         entry is expanded into one row per copy, the way the equipment menu draws it)",
        armaments.len(),
        talismans.len(),
        protectors.len(),
    ));

    out.read_whole_inventory = true;
    out.armaments = merge_worn(
        core::mem::take(&mut out.armaments),
        armaments,
        |worn, held| worn.gaitem_handle.is_some() && worn.gaitem_handle == held.gaitem_handle,
    );
    out.talismans = merge_worn(
        core::mem::take(&mut out.talismans),
        talismans,
        |worn, held| worn.name == held.name,
    );

    let worn_protectors = core::mem::take(&mut out.protectors);
    for part in PROTECTOR_PARTS {
        let worn: Vec<ReadSlot> = worn_protectors
            .iter()
            .filter(|(worn_part, _)| *worn_part == part)
            .map(|(_, slot)| slot.clone())
            .collect();
        let held: Vec<ReadSlot> = protectors
            .iter()
            .filter(|(held_part, _)| *held_part == part)
            .map(|(_, slot)| slot.clone())
            .collect();
        for slot in merge_worn(worn, held, |worn, held| worn.name == held.name) {
            out.protectors.push((part, slot));
        }
    }
}

/// Every `SwordArtsParam` row that some `EquipParamGem` row grants -- i.e. every skill that is an
/// ash of war, as opposed to a skill an armament simply has.
///
/// # The technical shape of the bug this closes
///
/// `GetSwordArtsParamIdForWeapon` answers for every armament, gem or no gem: with a gem, that
/// gem's `swordArtsParamId`; with none, the armament's own. The two are indistinguishable at the
/// call, and the second is not an ash of war -- there is no gem item that grants it, so no ash
/// exists by that name in any catalogue built from the gem table. That is the class:
///
/// * armaments that refuse ashes entirely (`Ringed Finger` -> `Claw Flick`, `Rivers of Blood` ->
///   `Corpse Piler`, every boss and DLC unique);
/// * armaments that allow one but have none mounted, which report the same way.
///
/// Exporting either as `weaponArt` names something the planner cannot resolve, and its build page
/// throws while looking it up -- which is what made a build unsaveable. So the export asks the
/// gem table whether the skill is an ash at all, which is the same question the planner's own
/// (gem-derived) ash list answers, rather than testing one armament's default and hoping.
fn ash_of_war_arts_rows() -> std::collections::BTreeSet<u32> {
    use eldenring::cs::{EquipParamGem, SoloParamRepository};
    use fromsoftware_shared::FromStatic;

    let mut rows = std::collections::BTreeSet::new();
    // Safety: read-only enumeration behind the populated-singleton check.
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        return rows;
    };
    for (_, row) in repo.rows::<EquipParamGem>() {
        if let Ok(arts_id) = u32::try_from(row.sword_arts_param_id())
            && arts_id != 0
        {
            rows.insert(arts_id);
        }
    }
    rows
}

/// [`WeaponFacts`] for one armament row, for the worn read, which has no reason to build the
/// whole table the carried read needs.
fn weapon_facts_for(row_with_affinity: u32) -> Option<WeaponFacts> {
    use eldenring::cs::{EquipParamWeapon, SoloParamRepository};
    use fromsoftware_shared::FromStatic;

    // Safety: read-only row access behind the populated-singleton check.
    let repo = (unsafe { SoloParamRepository::instance() }).ok()?;
    repo.rows::<EquipParamWeapon>()
        .find(|(id, _)| *id == row_with_affinity)
        .map(|(_, row)| WeaponFacts {
            wep_type: row.wep_type(),
            default_arts: u32::try_from(row.sword_arts_param_id()).unwrap_or_default(),
        })
}

/// How many copies of an item one inventory entry stands for.
///
/// An entry carries a `quantity`, and the equipment menu draws that many rows: gear the player
/// holds several of can arrive as one entry saying five rather than as five entries. Exporting the
/// entry alone loses the other four, which is exactly "rows of duplicates missing" -- so the entry
/// is expanded here.
///
/// The bound is a sanity rail, not a policy: gear quantities are small, and a five-figure one
/// means the read is wrong rather than that the player is carrying a five-figure pile. A clamp is
/// logged by the caller rather than applied quietly.
const MAX_COPIES_PER_ENTRY: u32 = 999;

/// The number of rows one entry becomes, and whether that number had to be clamped.
fn copies_of(quantity: u32) -> (u32, bool) {
    let wanted = quantity.max(1);
    (
        wanted.min(MAX_COPIES_PER_ENTRY),
        wanted > MAX_COPIES_PER_ENTRY,
    )
}

/// `EquipParamWeapon::wepType` values that are ammunition rather than armaments.
///
/// Measured, not assumed. The field sits at row offset 422 and every one of the game's 3554 weapon
/// rows was tabulated against its own name (`scripts/regulation-params.py` +
/// `scripts/er-item-name.py`): 81 is `Arrow`, 83 `Great Arrow`, 85 `Bolt`, 86 `Ballista Bolt`, and
/// no other type carries any of those four. 33 is `Unarmed`, alone in its type.
const AMMUNITION_WEAPON_TYPES: [u16; 4] = [81, 83, 85, 86];

/// Whether a `wepType` is one of the four ammunition types.
///
/// Shared with [`crate::catalog::Quivers`], which classifies the same rows by `weaponCategory` --
/// the field the engine's quantity gate reads -- and compares its answer against this one. Two
/// independent offsets agreeing is what stands in for the compile-time offset pin that
/// `EQUIP_PARAM_WEAPON_ST`'s private fields make impossible.
pub(crate) fn is_ammunition(wep_type: u16) -> bool {
    AMMUNITION_WEAPON_TYPES.contains(&wep_type)
}

/// What the param table says about each armament row: its `wepType`, and the skill it carries
/// with no ash of war mounted.
#[derive(Clone, Copy)]
struct WeaponFacts {
    wep_type: u16,
    /// `swordArtsParamId` -- the armament's own skill. An armament reporting this row is an
    /// armament with no ash on it.
    default_arts: u32,
}

/// [`WeaponFacts`] for every `EquipParamWeapon` row, read once off the live table.
fn weapon_facts() -> std::collections::BTreeMap<u32, WeaponFacts> {
    use eldenring::cs::{EquipParamWeapon, SoloParamRepository};
    use fromsoftware_shared::FromStatic;

    let mut facts = std::collections::BTreeMap::new();
    // Safety: read-only row access behind the populated-singleton check.
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        return facts;
    };
    for (row_id, row) in repo.rows::<EquipParamWeapon>() {
        facts.insert(
            row_id,
            WeaponFacts {
                wep_type: row.wep_type(),
                default_arts: u32::try_from(row.sword_arts_param_id()).unwrap_or_default(),
            },
        );
    }
    facts
}

/// The highest level `row_with_affinity` can reach, cached: an inventory holds many copies of one
/// armament and the answer is a scan of the whole `EquipParamWeapon` table.
fn max_upgrade(
    levels: &ReinforceLevels,
    cache: &mut std::collections::BTreeMap<u32, u16>,
    row_with_affinity: u32,
) -> u16 {
    if let Some(known) = cache.get(&row_with_affinity) {
        return *known;
    }
    let max = levels.clamp(
        row_with_affinity,
        er_build_import_core::plan::MAX_REGULAR_LEVEL,
    );
    cache.insert(row_with_affinity, max);
    max
}

/// `EquipParamWeapon` row 0x1adb0 -- "Unarmed", the item an empty hand holds.
///
/// An empty weapon slot does not read as nothing: `GetParamIdInSlot` answers 110000, which is a
/// real row with a real name, so a character wielding one armament exported as six -- the armament
/// and five copies of "Unarmed". The planner deletes them on import (its catalogue has no such
/// item), which is why they were invisible on the site and present in every payload.
const UNARMED_ARMAMENT_ROW: u32 = 110_000;

/// The ash of war mounted on the instance a gaitem handle names, or `None` when it is carrying
/// nothing but its own skill.
///
/// # An armament's own skill is not an ash of war, and saying it is breaks the planner
///
/// `GetSwordArtsParamIdForWeapon` always answers: with a gem, the gem's row; without one, the
/// armament's `swordArtsParamId`. Exporting that second answer as `weaponArt` names a skill the
/// planner has no ash for -- Ringed Finger reports `Claw Flick`, which is a weapon skill and not
/// in its ashes-of-war table (`allow_ash_of_war: !1` on that armament) -- and the page throws
/// while looking it up. A build carrying one cannot be saved. So the default is filtered out
/// here, which is also what it MEANS: nothing was mounted.
///
/// # Safety
///
/// Game thread; `module_base` the loaded image base and `msg` a live `MsgRepositoryImp*`.
unsafe fn mounted_ash(
    module_base: usize,
    msg: usize,
    handle: u32,
    default_arts: Option<u32>,
    wep_type: Option<u16>,
    ashes: &std::collections::BTreeSet<u32>,
) -> Option<String> {
    // Safety: the caller's contract; the record is the engine's own and the chain is null-checked.
    let mut lookup = unsafe { GaitemLookupResult::from_handle(module_base, handle) }?;
    // Safety: as above.
    let arts_id = unsafe { lookup.sword_arts_id(module_base) }?;
    if !is_ash_of_war(arts_id, default_arts, ashes) {
        return None;
    }
    // And the game has to allow it on this armament. An ash reaches an armament as a gem, and
    // nothing in a grant path checks whether that gem may be mounted there -- a build asking for
    // a shield ash on a katana gets one, and the character then carries a pairing the game itself
    // would never let a player make. `CheckIfWepTypeCanEquipGem` is the rule, and it is applied to
    // the gem actually mounted rather than to the one a catalogue would have picked for the skill.
    if let Some(wep_type) = wep_type {
        // Safety: the caller's contract; the record is the engine's own.
        match unsafe { lookup.mounted_gem_row(module_base) } {
            Some(gem_row) if !crate::gem_mount::gem_can_mount(gem_row, wep_type) => return None,
            // No gem at all means the skill is the armament's own, which `is_ash_of_war` already
            // refused; anything else is a gem the game is happy with.
            _ => {}
        }
    }
    // Safety: as above.
    unsafe { name_for(Kind::AshOfWar, msg, module_base, arts_id) }
}

/// Whether a skill an armament reports is an ASH of war that was mounted on it.
///
/// Two tests, and the first is the class-wide one: a skill no `EquipParamGem` row grants is not an
/// ash, whatever armament is carrying it. The second catches the remaining case -- an armament
/// whose own skill happens to also exist as an ash (many do: `Square Off` is both the Longsword's
/// default and a purchasable ash) and which has nothing mounted, where reporting the ash would
/// claim an item the player does not have.
fn is_ash_of_war(
    arts_id: u32,
    default_arts: Option<u32>,
    ashes: &std::collections::BTreeSet<u32>,
) -> bool {
    ashes.contains(&arts_id) && default_arts != Some(arts_id)
}

/// Merge the worn list into the held one: every held item, with the worn ones carrying their
/// equip index.
///
/// A worn item that matched nothing held is kept rather than dropped -- it is on the character, so
/// leaving it out because the inventory did not list it would export a loadout the player is not
/// wearing.
fn merge_worn(
    worn: Vec<ReadSlot>,
    held: Vec<ReadSlot>,
    matches: impl Fn(&ReadSlot, &ReadSlot) -> bool,
) -> Vec<ReadSlot> {
    let mut claimed = vec![false; worn.len()];
    let mut out: Vec<ReadSlot> = held
        .into_iter()
        .map(|mut slot| {
            if let Some((index, worn_slot)) = worn
                .iter()
                .enumerate()
                .find(|(index, worn_slot)| !claimed[*index] && matches(worn_slot, &slot))
            {
                claimed[index] = true;
                slot.equip_index = worn_slot.equip_index;
                // The worn read knows the ash the instance carries; keep it when the held read
                // could not answer.
                if slot.weapon_art.is_none() {
                    slot.weapon_art = worn_slot.weapon_art.clone();
                }
            }
            slot
        })
        .collect();
    for (index, slot) in worn.into_iter().enumerate() {
        if !claimed[index] {
            out.push(slot);
        }
    }
    out
}

/// Which body part every `EquipParamProtector` row is worn on.
///
/// Read from the live param table rather than inferred from the id, because the id ranges are not
/// contiguous per part. Built once per export: the table is ~800 rows and a carried inventory can
/// hold a hundred pieces.
fn protector_parts() -> std::collections::BTreeMap<u32, &'static str> {
    use eldenring::cs::{EquipParamProtector, SoloParamRepository};
    use fromsoftware_shared::FromStatic;

    let mut parts = std::collections::BTreeMap::new();
    // Safety: read-only row access behind the populated-singleton check.
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        return parts;
    };
    for (row_id, row) in repo.rows::<EquipParamProtector>() {
        let part = if row.head_equip() {
            "head"
        } else if row.body_equip() {
            "body"
        } else if row.arm_equip() {
            "arms"
        } else if row.leg_equip() {
            "legs"
        } else {
            continue;
        };
        parts.insert(row_id, part);
    }
    parts
}

/// Read the character's appearance out of `PlayerGameData`.
///
/// Returns the buffer whole -- `FACE`, version, declared size, payload -- because that is the unit
/// the game serialises into a save slot and the unit every appearance-editing tool exchanges. A
/// read that does not begin with the magic is reported as no face data at all rather than as a
/// blob: the failure mode this guards against is publishing 288 bytes of unrelated heap into a
/// share link, which nothing downstream could tell from a real face.
///
/// # Safety
///
/// `pgd` must be a live `PlayerGameData*`.
unsafe fn read_face_data(pgd: usize) -> Option<Vec<u8>> {
    let mut buffer = vec![0u8; pgd::FACE_DATA_BUFFER_LEN];
    // Safety: fault-checked; an unmapped page answers false instead of taking the game down.
    if !unsafe { er_game_base::mem::read_bytes(pgd + pgd::FACE_DATA_BUFFER, &mut buffer) } {
        return None;
    }
    if buffer[..FACE_DATA_MAGIC.len()] != FACE_DATA_MAGIC {
        return None;
    }
    Some(buffer)
}

/// Low 28 bits of a category-tagged item id, i.e. the param row.
const ITEM_ID_ROW_MASK: u32 = 0x0FFF_FFFF;

/// `ChrAsmArmStyle::LeftBothHands` / `RightBothHands` -- the two values that mean two-handing.
const ARM_STYLE_LEFT_BOTH_HANDS: u32 = 2;
const ARM_STYLE_RIGHT_BOTH_HANDS: u32 = 3;

/// `EquipGameData::chr_asm.equipment.arm_style`, bound to the upstream typed layout.
const EQUIP_GAME_DATA_ARM_STYLE_OFFSET: usize = {
    use eldenring::cs::{ChrAsm, EquipGameData};
    core::mem::offset_of!(EquipGameData, chr_asm) + core::mem::offset_of!(ChrAsm, equipment)
};

/// Read `ChrAsm::equipment.arm_style`.
///
/// # Safety
///
/// Game thread, `egd` live.
unsafe fn read_arm_style(egd: usize) -> Option<u32> {
    // Safety: a fault-checked read of one field at an offset the compiler derived from the
    // upstream struct, so a layout change breaks the build rather than the read. Read as `i32`
    // because that is the width the fault-checked reader offers; the enum has four values.
    unsafe { er_game_base::mem::safe_read_i32(egd + EQUIP_GAME_DATA_ARM_STYLE_OFFSET) }
        .map(|value| value as u32)
}
