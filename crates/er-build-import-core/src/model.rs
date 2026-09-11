//! The subset of a planner build document this crate reads.
//!
//! The real payload carries far more (computed AR, absorption, resistances, view
//! state, tags, author). Everything not needed to grant and equip items is
//! ignored rather than modelled, so an upstream field addition cannot fail the
//! parse.

use serde::Deserialize;
use std::collections::BTreeMap;

/// A build as served by `GET /inventories/{id}`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BuildDoc {
    /// The share id, i.e. the `?b=` value.
    #[serde(default)]
    pub id: String,
    /// The build's display name.
    #[serde(default)]
    pub name: String,
    /// Starting class, when the author picked one.
    #[serde(default, rename = "characterClass")]
    pub character_class: Option<String>,
    /// Default armament upgrade level, applied to any weapon slot that does not
    /// override it.
    #[serde(default, rename = "weaponUpgrade")]
    pub weapon_upgrade: u16,
    /// Requested level and attributes (`rl`, `vig`, `str`, ...).
    #[serde(default)]
    pub stats: BTreeMap<String, i64>,
    /// The named loadout sets, one list per equip category. Absent on builds
    /// authored before the planner grew sets.
    #[serde(default)]
    pub sets: Sets,
    /// Armaments. Named `inventory` upstream.
    #[serde(default)]
    pub inventory: SlotList,
    /// Sorceries and incantations, in memorisation order.
    #[serde(default)]
    pub spells: SlotList,
    /// Talismans.
    #[serde(default)]
    pub talismans: SlotList,
    /// Armour, keyed by body part (`head`, `body`, `arms`, `legs`).
    #[serde(default)]
    pub protectors: BTreeMap<String, SlotList>,
    /// Consumables, flask and physick configuration.
    #[serde(default)]
    pub items: Items,
    /// Whether the build two-hands its main armament.
    #[serde(default, rename = "is2h")]
    pub two_handing: bool,
    /// Equipped great rune, when the author chose one.
    #[serde(default, rename = "greatRune")]
    pub great_rune: Option<String>,
    /// The character's appearance, as the planner's Cosmetics tab carries it.
    ///
    /// Absent on a build whose author never opened that tab, which is most of them, and on every
    /// build authored before the planner grew it in v2.19. The importer leaves the character's own
    /// face alone in that case rather than applying a default one -- see
    /// [`crate::sliders::SlidersRejection::Absent`].
    #[serde(default)]
    pub sliders: Option<crate::sliders::SlidersDoc>,
}

impl BuildDoc {
    /// The appearance this build asks for, or why there is none to apply.
    ///
    /// A `sliders` object carrying no slider values is [`SlidersRejection::Absent`] and not an
    /// empty appearance: the planner writes exactly that shape when its Cosmetics tab is opened
    /// and nothing is touched, and applying it would flatten every slider on the character to
    /// zero -- a face, but not one anybody chose.
    ///
    /// ```
    /// use er_build_import_core::model;
    /// let none = model::parse("{}").expect("parses");
    /// assert!(none.appearance().is_err());
    ///
    /// let empty = model::parse(r#"{"sliders":{"sliders":{}}}"#).expect("parses");
    /// assert!(empty.appearance().is_err());
    ///
    /// let some = model::parse(r#"{"sliders":{"sliders":{"age":3}}}"#).expect("parses");
    /// assert_eq!(some.appearance().expect("present").sliders["age"], 3);
    /// ```
    ///
    /// # Errors
    ///
    /// [`SlidersRejection::Absent`] when the build names no appearance.
    pub fn appearance(&self) -> Result<&crate::sliders::SlidersDoc, crate::SlidersRejection> {
        self.sliders
            .as_ref()
            .filter(|found| !found.sliders.is_empty())
            .ok_or(crate::SlidersRejection::Absent)
    }
}

/// A category's slot list.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SlotList {
    /// The slots themselves.
    #[serde(default)]
    pub slots: Vec<Slot>,
}

/// One item in a build.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Slot {
    /// The item's display name -- the only identifier the payload carries.
    #[serde(default)]
    pub name: String,
    /// Position within its list.
    #[serde(default)]
    pub order: i64,
    /// Per-slot upgrade level overriding [`BuildDoc::weapon_upgrade`].
    #[serde(default)]
    pub upgrade: Option<u16>,
    /// Affinity, e.g. `Occult`. Absent means `Standard`.
    #[serde(default)]
    pub infusion: Option<String>,
    /// Ash of war on this armament, if any.
    #[serde(default, rename = "weaponArt")]
    pub weapon_art: Option<String>,
    /// The equip position this slot holds **in the active set**.
    ///
    /// A cache of `equipSet[active]`, not an independent fact -- see
    /// [`Slot::equip_index_in_set`], which is what the importer reads.
    #[serde(default, rename = "equipIndex")]
    pub equip_index: Option<u32>,
    /// The equip position this slot holds in *each* set, indexed by set.
    ///
    /// Holes are sets the item is not equipped in, and the array is only as long
    /// as the highest set that uses the item. Absent on pre-sets builds.
    #[serde(default, rename = "equipSet")]
    pub equip_set: Option<Vec<Option<u32>>>,
}

impl Slot {
    /// The equip position this slot holds in set `set_index`, if any.
    ///
    /// `equipSet` is authoritative when present; [`Slot::equip_index`] is only
    /// its active-set entry, kept in step by the planner. When `equipSet` is
    /// absent the build predates sets, which is the same thing as being equipped
    /// in set 0 and nowhere else -- exactly what the planner's own migration
    /// (`equipSet = [equipIndex]`) makes of such a row the moment sets appear.
    ///
    /// ```
    /// use er_build_import_core::model;
    /// let doc = model::parse(
    ///     r#"{"inventory":{"slots":[{"name":"Shamshir","equipSet":[null,2]}]}}"#,
    /// )
    /// .expect("parses");
    /// let slot = &doc.inventory.slots[0];
    /// assert_eq!(slot.equip_index_in_set(0), None);
    /// assert_eq!(slot.equip_index_in_set(1), Some(2));
    /// ```
    pub fn equip_index_in_set(&self, set_index: usize) -> Option<u32> {
        match self.equip_set.as_ref() {
            Some(sets) => sets.get(set_index).copied().flatten(),
            None if set_index == 0 => self.equip_index,
            None => None,
        }
    }
}

/// The loadout sets a build carries, one independent list per equip category.
///
/// The planner lets an author keep several loadouts side by side -- a bow set, a
/// two-handing set -- and exactly one per category is active. Each category
/// switches independently, so there is no single "active set" for a build.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Sets {
    /// Armament sets.
    #[serde(default)]
    pub weapons: Vec<EquipSet>,
    /// Talisman sets.
    #[serde(default)]
    pub talismans: Vec<EquipSet>,
    /// Armour sets, shared by all four body parts.
    #[serde(default)]
    pub protectors: Vec<EquipSet>,
}

impl Sets {
    /// The active armament set.
    pub fn active_weapons(&self) -> usize {
        active_index(&self.weapons)
    }

    /// The active talisman set.
    pub fn active_talismans(&self) -> usize {
        active_index(&self.talismans)
    }

    /// The active armour set.
    pub fn active_protectors(&self) -> usize {
        active_index(&self.protectors)
    }
}

/// One named loadout set.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EquipSet {
    /// The author's label for it, e.g. `Bows`.
    #[serde(default)]
    pub name: String,
    /// Whether this is the set the planner is currently showing.
    #[serde(default)]
    pub active: bool,
}

/// Which set of `list` is active, defaulting to the first.
///
/// A build with no `sets` key at all, or one where nothing carries `active`,
/// still has to import *something*; set 0 is what the planner renders in both
/// cases, and it is the set a pre-sets `equipIndex` belongs to.
fn active_index(list: &[EquipSet]) -> usize {
    list.iter().position(|set| set.active).unwrap_or(0)
}

/// Consumables and flasks.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Items {
    /// Consumable/crafting items.
    #[serde(default)]
    pub tools: SlotList,
    /// Physick tears; entries are `null` when a slot is left empty.
    #[serde(default, rename = "crystalTears")]
    pub crystal_tears: Vec<Option<String>>,
    /// Flask allocation: `level` (Sacred Tears), `total`, and the crimson/cerulean split.
    #[serde(default)]
    pub flasks: Flasks,
    /// Equipped arrows and bolts.
    #[serde(default)]
    pub ammo: Ammo,
}

/// The four ammunition positions, each holding an item name.
///
/// # This category has a shape of its own, and it is not a [`SlotList`]
///
/// Every other category is a list of [`Slot`] objects. Ammo is a flat object keyed by position,
/// whose value is the bare name string -- `{"arrow1": "Bone Arrow", "bolt2": "Lightning Bolt"}` --
/// so there is no `order`, no `equipIndex`, no `equipSet` and no upgrade: the key is the equip
/// position, and an ammo entry that is not equipped simply does not exist. Taken from the
/// planner's own code rather than inferred: its picker writes
/// `character.items.ammo[slot] = ammo.name` for `slot` drawn from `['arrow1','arrow2']` and
/// `['bolt1','bolt2']`, its equip view reads `e.arrow1 ? {name: e.arrow1} : null` for each of the
/// four, and its "already equipped" test is `n.arrow1 === name || n.arrow2 === name || ...`.
///
/// # Absent, empty, and the shape that was deleted
///
/// All three of the captured fixtures carry no ammo -- two spell it `"ammo": {}` (which is the
/// planner's own default for a new character) and the third, authored at planner version 3.7.7,
/// has no `ammo` key at all, because the feature shipped in 3.9 (2025-07-25, "*New Feature: Ammo
/// inventory*"). Unequipping the last one deletes the object again, so an absent key and an empty
/// one mean the same thing and both have to parse.
///
/// There was an older shape, and modelling it would be modelling something the planner destroys:
/// its migration runs `if (items.ammo && 'slots' in items.ammo) delete items.ammo`, so a
/// `{"slots": [...]}` ammo object is not a variant to support -- it is a document the planner has
/// already decided is unreadable.
///
/// # No quantity, again
///
/// Like every other category, nothing here says how many. `plan::ammo_quantity` documents where
/// the number comes from instead.
///
/// # The keys, and why they are a shared table
///
/// [`AMMO_POSITION_KEYS`] holds the four spellings in `ChrAsmSlot` order, and both directions
/// read it: this module to name the position it parsed, and `er-build-import-runtime`'s exporter
/// to name the position it is writing. Two hand-written lists is two chances to interleave them
/// differently, and the export direction has no read-back to catch it -- a bolt written under an
/// arrow key produces a valid document describing a different character.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Ammo {
    /// First arrow position, `ChrAsmSlot::Arrow1`.
    #[serde(default)]
    pub arrow1: Option<String>,
    /// Second arrow position, `ChrAsmSlot::Arrow2`.
    #[serde(default)]
    pub arrow2: Option<String>,
    /// First bolt position, `ChrAsmSlot::Bolt1`.
    #[serde(default)]
    pub bolt1: Option<String>,
    /// Second bolt position, `ChrAsmSlot::Bolt2`.
    #[serde(default)]
    pub bolt2: Option<String>,
}

/// The planner's four ammunition keys, **in `ChrAsmSlot` order** -- `Arrow1 = 6, Bolt1 = 7,
/// Arrow2 = 8, Bolt2 = 9`.
///
/// The order is the engine's and the SPELLINGS are the planner's, and neither half is free to
/// change: the engine interleaves the two kinds while the planner's UI groups them
/// (`['arrow1','arrow2']`, then `['bolt1','bolt2']`), so a table written in the planner's grouping
/// and added to [`crate::equip::CHR_ASM_SLOT_AMMO_1`] puts every bolt in an arrow slot. One table,
/// read by [`Ammo::positions`] on the way in and by the exporter on the way out.
pub const AMMO_POSITION_KEYS: [&str; 4] = ["arrow1", "bolt1", "arrow2", "bolt2"];

impl Ammo {
    /// The four positions **in `ChrAsmSlot` order**, each with the planner key that names it.
    ///
    /// The order is load-bearing and is the engine's, not the planner's: `ChrAsmSlot` runs
    /// `Arrow1 = 6, Bolt1 = 7, Arrow2 = 8, Bolt2 = 9`, interleaving the two kinds, while the
    /// planner's own UI groups them (`['arrow1','arrow2']` then `['bolt1','bolt2']`). A caller
    /// that indexed this array and added it to the base slot while using the planner's grouping
    /// would put bolts in the arrow slots -- so the interleave lives here, once.
    ///
    /// ```
    /// use er_build_import_core::model;
    /// let doc = model::parse(r#"{"items":{"ammo":{"bolt1":"Bolt","arrow1":"Bone Arrow"}}}"#)
    ///     .expect("parses");
    /// let positions = doc.items.ammo.positions();
    /// assert_eq!(positions[0], ("arrow1", Some("Bone Arrow")));
    /// assert_eq!(positions[1], ("bolt1", Some("Bolt")));
    /// assert_eq!(positions[2], ("arrow2", None));
    /// assert_eq!(positions[3], ("bolt2", None));
    /// ```
    pub fn positions(&self) -> [(&'static str, Option<&str>); AMMO_POSITION_KEYS.len()] {
        let names = [
            self.arrow1.as_deref(),
            self.bolt1.as_deref(),
            self.arrow2.as_deref(),
            self.bolt2.as_deref(),
        ];
        core::array::from_fn(|index| (AMMO_POSITION_KEYS[index], names[index]))
    }

    /// Whether the build equips no ammunition at all.
    ///
    /// ```
    /// use er_build_import_core::model;
    /// assert!(model::parse(r#"{"items":{"ammo":{}}}"#).expect("parses").items.ammo.is_empty());
    /// assert!(model::parse(r#"{"items":{}}"#).expect("parses").items.ammo.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.positions().iter().all(|(_, name)| name.is_none())
    }
}

/// Flask allocation.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct Flasks {
    /// Flask potency, i.e. Sacred Tears drunk.
    #[serde(default)]
    pub level: u32,
    /// Total flasks, i.e. Golden Seeds spent.
    #[serde(default)]
    pub total: u32,
    /// Flasks allocated to Crimson Tears.
    #[serde(default)]
    pub crimson: u32,
    /// Flasks allocated to Cerulean Tears.
    #[serde(default)]
    pub cerulean: u32,
}

/// Parse a planner build document.
///
/// # Errors
///
/// Returns the underlying `serde_json` error when the payload is not a build
/// document.
pub fn parse(json: &str) -> Result<BuildDoc, serde_json::Error> {
    serde_json::from_str(json)
}
