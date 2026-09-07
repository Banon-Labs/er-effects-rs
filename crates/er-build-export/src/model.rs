//! The `?i=` document -- the planner's character object, written rather than read.
//!
//! [`crate::model`] is the mirror image of `er_build_import_core::model`: that one models the
//! *subset* a reader needs and ignores everything else, because an upstream field addition
//! must not fail a parse. A writer cannot afford the same laxity. The planner's
//! `importState` merges the incoming object into the live character with a helper that sets
//! any key **missing from the source** to `undefined`, so a key this crate omits is not
//! defaulted -- it is erased, and the planner then reads `undefined.slots` and dies. Every
//! key of the planner's own `makeDefault()` is therefore modelled here, defaulted here, and
//! always emitted.
//!
//! Two keys are omitted on purpose, and only these two: `computed` (the planner recomputes
//! it from scratch on its next save) and `activeEffects` (read with `?.`, so `undefined` is
//! a value it already handles). Both are large, both are derived, and neither survives a
//! round trip anyway.
//!
//! The defaults below are transcribed from the live bundle's
//! `static makeDefault(e=0,t=!1)`, not invented.

use serde::Serialize;

/// Planner schema version stamped into every document this crate writes.
///
/// The planner runs `migrateCharacter` over an imported document, and the migrations it
/// applies are chosen by this field. Stamping the version the site currently ships means
/// every migration is a no-op, which is the only state in which what we wrote is what the
/// planner ends up holding. Read out of the live bundle (`di = "3.12.2"`).
pub const PLANNER_VERSION: &str = "3.12.2";

/// Armament upgrade level `makeDefault` uses when it is given no rune level.
pub const DEFAULT_WEAPON_UPGRADE: u16 = 25;

/// Rune levels per armament upgrade level in `makeDefault`'s level-derived form,
/// `min(level / 5, 25)`. Not a game rule -- the planner's own guess at what a build of that
/// level would plausibly be carrying.
pub const RUNE_LEVELS_PER_UPGRADE: u16 = 5;

/// The physick flask has exactly two tear slots, and the planner always writes both
/// (`crystalTears: [null, null]`) rather than a shorter array.
pub const CRYSTAL_TEAR_SLOTS: usize = 2;

/// The name of the one set every category starts with.
pub const DEFAULT_SET_NAME: &str = "Default";

/// Tool `equipIndex` values below this are QUICKBAR positions; the rest are pouch positions.
///
/// The planner's own `QUICKBAR`, read out of the live bundle
/// (`e[e.QUICKBAR = 10] = "QUICKBAR", e[e.POUCH = 16] = "POUCH"`). It is a fact about the
/// document, which is why it is declared here rather than borrowed from the importer: the game
/// side has its own count -- the length of `ChrAsmEquipEntries::quickItem1..10` -- and the two
/// being equal is a claim worth testing rather than an identity worth assuming. See
/// `tests/round_trip.rs`, which asserts they agree.
pub const QUICKBAR_POSITIONS: usize = 10;

/// Tool `equipIndex` values from [`QUICKBAR_POSITIONS`] up to this are pouch positions.
///
/// The planner's `POUCH`, and it is a total rather than a count: its equip view builds
/// `times(POUCH)` entries and slices at `QUICKBAR`, so the pouch itself holds
/// `POUCH_POSITIONS_TOTAL - QUICKBAR_POSITIONS` = 6.
pub const POUCH_POSITIONS_TOTAL: usize = 16;

/// A whole planner character, as `?i=` carries it.
///
/// Field order is `makeDefault`'s order so the two can be diffed by eye; JSON object order
/// is not significant to the planner.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildExportDoc {
    /// A link the author attached to the build. Not the share link -- free text.
    pub build_url: String,
    /// The author's notes.
    pub description: String,
    /// Server-side share id, i.e. the `?b=` value. Empty for a build that was never stored.
    pub id: String,
    /// Armaments. Named `inventory` upstream, and it is only ever weapons.
    pub inventory: SlotList,
    /// Talismans.
    pub talismans: SlotList,
    /// The build's display name.
    pub name: String,
    /// Rune level and the eight attributes.
    pub stats: Stats,
    /// Sorceries and incantations, in memorisation order.
    ///
    /// Deliberately [`SpellList`] and not [`SlotList`]: `makeDefault` gives spells
    /// `{slots: []}` with **no** `sorting` key, and emitting one would be inventing a field.
    pub spells: SpellList,
    /// Armour, one list per body part.
    pub protectors: Protectors,
    /// Consumables, ammunition, physick tears and flask allocation.
    pub items: Items,
    /// Starting class, or `null` when the author picked none.
    pub character_class: Option<String>,
    /// Equipped great rune, by name.
    ///
    /// The one key here that `makeDefault()` does **not** write, and it is still required.
    /// The stored payload carries it at the top level (verified on build `af97a9da874151`,
    /// where it is `null`), `er_build_import_core::model::BuildDoc` reads it, and the planner reads
    /// it as `this.character.greatRune && tools.get(...)` -- a guarded read, so absent is
    /// safe. Exporting without the key at all would silently drop the equipped rune on the
    /// way back into the game, which is why it is modelled; `null` would merely be noise,
    /// which is why it is skipped rather than written when unset.
    #[serde(rename = "greatRune", skip_serializing_if = "Option::is_none")]
    pub great_rune: Option<String>,
    /// The character's appearance, as an uppercase hex AOB of the game's own `FaceDataBuffer`.
    ///
    /// **Ours, not the planner's.** No key of `makeDefault()` carries an appearance and nothing in
    /// the planner reads one: a build there is stats and gear. It is written at the top level
    /// anyway because the `?i=` payload is the only place a shared build can carry it, and because
    /// the planner's merge (`K_`) copies unknown keys through or ignores them -- it never fails on
    /// one. So the site shows the build it always showed, and a reader that knows about this key
    /// (this repository's own decoder, or a player pasting the AOB into a save editor) gets the
    /// face back with it.
    ///
    /// The value is the whole buffer, magic first, so it is self-describing and matches what every
    /// appearance tool exchanges byte for byte. Absent rather than `null` when unknown.
    #[serde(rename = "faceData", skip_serializing_if = "Option::is_none")]
    pub face_data: Option<String>,
    /// Schema version; see [`PLANNER_VERSION`].
    pub version: String,
    /// Cloud account that stored the build, or `null` for a local one.
    pub author: Option<Author>,
    /// Upgrade level applied to any armament slot that does not override it.
    pub weapon_upgrade: u16,
    /// Whether the build two-hands its main armament.
    #[serde(rename = "is2h")]
    pub two_handing: bool,
    /// PvE mode, which changes which effects the planner considers active.
    #[serde(rename = "isPvE")]
    pub pve: bool,
    /// Which view each pane of the planner UI is showing.
    pub views: Views,
    /// Conditional-effect toggles.
    pub conditions: Conditions,
    /// Named equipment sets. Every category always has at least the active `Default`.
    pub sets: Sets,
    /// Free-text tags.
    pub tags: Vec<String>,
    /// User-authored effects. Shape is the planner's business, so it stays untyped.
    pub custom_effects: Vec<serde_json::Value>,
}

impl Default for BuildExportDoc {
    /// `makeDefault(0, false)`.
    fn default() -> Self {
        Self {
            build_url: String::new(),
            description: String::new(),
            id: String::new(),
            inventory: SlotList::default(),
            talismans: SlotList::default(),
            name: String::new(),
            stats: Stats::default(),
            spells: SpellList::default(),
            protectors: Protectors::default(),
            items: Items::default(),
            character_class: None,
            great_rune: None,
            face_data: None,
            version: PLANNER_VERSION.to_string(),
            author: None,
            weapon_upgrade: DEFAULT_WEAPON_UPGRADE,
            two_handing: false,
            pve: false,
            views: Views::default(),
            conditions: Conditions::default(),
            sets: Sets::default(),
            tags: Vec::new(),
            custom_effects: Vec::new(),
        }
    }
}

impl BuildExportDoc {
    /// `makeDefault(level, pve)` -- a default document for a character of a given rune level.
    ///
    /// Worth having as its own constructor rather than leaving callers to set the two fields:
    /// the level does not only land in `stats.rl`, it also *derives* `weaponUpgrade`, and a
    /// caller who set `rl` alone would silently ship every armament at +25.
    pub fn with_level(level: i64, pve: bool) -> Self {
        let mut doc = Self {
            weapon_upgrade: weapon_upgrade_for_level(level),
            pve,
            ..Self::default()
        };
        doc.stats.rune_level = level;
        doc
    }
}

/// The planner's `weaponUpgrade` derivation: `(level > 0 ? min(level / 5, 25) : 25) | 0`.
///
/// The `| 0` is a truncation, and JavaScript's `/` is real division, so this is a floor.
pub fn weapon_upgrade_for_level(level: i64) -> u16 {
    if level <= 0 {
        return DEFAULT_WEAPON_UPGRADE;
    }
    let derived = level / i64::from(RUNE_LEVELS_PER_UPGRADE);
    let capped = derived.min(i64::from(DEFAULT_WEAPON_UPGRADE));
    // `capped` is in `1..=DEFAULT_WEAPON_UPGRADE` here, so the narrowing cannot lose anything.
    capped as u16
}

/// Rune level and the eight attributes.
///
/// Every key is renamed because the planner's are abbreviations, and one of them is a trap:
/// see [`Stats::endurance`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Stats {
    /// Arcane.
    #[serde(rename = "arc")]
    pub arcane: i64,
    /// Dexterity.
    #[serde(rename = "dex")]
    pub dexterity: i64,
    /// Faith.
    #[serde(rename = "fth")]
    pub faith: i64,
    /// Intelligence.
    #[serde(rename = "int")]
    pub intelligence: i64,
    /// Mind.
    #[serde(rename = "mnd")]
    pub mind: i64,
    /// Strength.
    #[serde(rename = "str")]
    pub strength: i64,
    /// Vigour.
    #[serde(rename = "vig")]
    pub vigor: i64,
    /// **Endurance**, despite the key.
    ///
    /// `vit` reads as Vitality -- a stat Elden Ring does not have, and a name Dark Souls used
    /// for the health stat, i.e. for what Elden Ring calls Vigour. So the intuitive misreading
    /// puts this value in the wrong slot *and* leaves endurance unset. The planner's key set is
    /// `{arc, dex, fth, int, mnd, str, vig, vit}`: eight keys for eight attributes, with no
    /// `end`, which only balances if `vit` is endurance. Verified against the game side in
    /// `er-build-import-runtime` (`stats[STAT_ENDURANCE] = want("vit")`) and recorded in bd
    /// `planner-vit-key-IS-endurance-verified-2026-08-22`.
    #[serde(rename = "vit")]
    pub endurance: i64,
    /// Rune level.
    #[serde(rename = "rl")]
    pub rune_level: i64,
}

/// A category's slot list plus how the planner is sorting it on screen.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct SlotList {
    /// The slots themselves.
    pub slots: Vec<Slot>,
    /// Display sort order. Cosmetic, but part of the key set, so it is always written.
    pub sorting: Sorting,
}

impl SlotList {
    /// A list holding exactly these slots, sorted the default way.
    pub fn new(slots: Vec<Slot>) -> Self {
        Self {
            slots,
            sorting: Sorting::default(),
        }
    }
}

/// Spells, which alone among the categories carry no `sorting` key.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct SpellList {
    /// Memorised spells, in memorisation order.
    pub slots: Vec<Slot>,
}

impl SpellList {
    /// A spell list holding exactly these slots.
    pub fn new(slots: Vec<Slot>) -> Self {
        Self { slots }
    }
}

/// How a category is sorted on screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sorting {
    /// `"asc"` or `"desc"`.
    pub direction: String,
    /// `"acquisition"` and friends.
    pub method: String,
}

/// `makeDefault`'s sort direction.
pub const DEFAULT_SORT_DIRECTION: &str = "asc";
/// `makeDefault`'s sort method.
pub const DEFAULT_SORT_METHOD: &str = "acquisition";

impl Default for Sorting {
    fn default() -> Self {
        Self {
            direction: DEFAULT_SORT_DIRECTION.to_string(),
            method: DEFAULT_SORT_METHOD.to_string(),
        }
    }
}

/// One item in a build.
///
/// The optional fields are skipped when `None` rather than written as `null`, because the
/// planner distinguishes them: a *carried but unequipped* armament has no `equipIndex` key at
/// all, so `Some(i)` means equipped and `None` means carried. Writing `equipIndex: null`
/// would be a third state neither the planner nor `er-build-import-core` reads.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Slot {
    /// The item's display name -- the only identifier the payload carries.
    pub name: String,
    /// Position within its list.
    pub order: i64,
    /// Per-slot upgrade level overriding [`BuildExportDoc::weapon_upgrade`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upgrade: Option<u16>,
    /// Affinity, e.g. `"Occult"`. Absent means `"Standard"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub infusion: Option<String>,
    /// Ash of war on this armament, if any.
    #[serde(rename = "weaponArt", skip_serializing_if = "Option::is_none")]
    pub weapon_art: Option<String>,
    /// Indices of the named sets this slot is equipped in. Observed as `[1]`, `[2]`, ...
    #[serde(rename = "equipSet", skip_serializing_if = "Option::is_none")]
    pub equip_set: Option<Vec<u32>>,
    /// Present when the slot is actually equipped, giving the equip position.
    #[serde(rename = "equipIndex", skip_serializing_if = "Option::is_none")]
    pub equip_index: Option<u32>,
}

impl Slot {
    /// A carried, unequipped item.
    pub fn carried(name: impl Into<String>, order: i64) -> Self {
        Self {
            name: name.into(),
            order,
            ..Self::default()
        }
    }

    /// Mark this slot equipped at `index`, in the set at the same index.
    ///
    /// Both keys are set together because the planner writes them together: `equipIndex`
    /// without a matching `equipSet` describes an item equipped in no set at all.
    pub fn equipped_at(mut self, index: u32) -> Self {
        self.equip_set = Some(vec![index]);
        self.equip_index = Some(index);
        self
    }

    /// Mark this slot equipped at `index` and claim no named set -- the shape a tool has.
    ///
    /// Not a laxer [`Slot::equipped_at`]: it is the planner's own distinction. `setSlotEquipIndex`
    /// assigns `equipIndex` unconditionally and only touches `equipSet` `if (category)`, and the
    /// tool surface never passes a category -- there is no `sets.tools`, only `sets.weapons`,
    /// `sets.talismans` and `sets.protectors`. The captured payload agrees: build
    /// `82086df03c4b8e` carries five equipped tools and the key union across all seven of its
    /// `items.tools.slots` rows is exactly `{name, order, equipIndex}`.
    ///
    /// So writing `equipSet` here would put a key on a row the planner never puts one on, and
    /// reading it back through `equip_index_in_set` -- which prefers `equipSet` when present --
    /// would then answer from a field nothing else maintains.
    pub fn equipped_without_set(mut self, index: u32) -> Self {
        self.equip_index = Some(index);
        self
    }

    /// Set the affinity.
    pub fn with_infusion(mut self, infusion: impl Into<String>) -> Self {
        self.infusion = Some(infusion.into());
        self
    }

    /// Set the ash of war.
    pub fn with_weapon_art(mut self, art: impl Into<String>) -> Self {
        self.weapon_art = Some(art.into());
        self
    }

    /// Override this slot's upgrade level.
    pub fn with_upgrade(mut self, upgrade: u16) -> Self {
        self.upgrade = Some(upgrade);
        self
    }

    /// Whether this slot is equipped rather than merely carried.
    pub fn is_equipped(&self) -> bool {
        self.equip_index.is_some()
    }
}

/// Armour, one list per body part.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Protectors {
    /// Gauntlets.
    pub arms: SlotList,
    /// Chest armour.
    pub body: SlotList,
    /// Helms.
    pub head: SlotList,
    /// Greaves.
    pub legs: SlotList,
}

/// Consumables, ammunition, physick tears and flask allocation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Items {
    /// Arrows and bolts, keyed by equip position. See [`Ammo`].
    pub ammo: Ammo,
    /// Consumable and crafting items -- **and the quickbar and the pouch**, neither of which has
    /// a key of its own anywhere in the document.
    ///
    /// The planner keeps one list here and addresses both assignable surfaces out of it through
    /// `equipIndex`: `0..10` is a quickbar position, `10..16` a pouch one. Its `ToolEquipSlots`
    /// view is literally `times(POUCH).map(() => null)` folded over `items.tools.slots` by
    /// `equipIndex`, then `slice(0, QUICKBAR)` for the quickbar and `slice(QUICKBAR, POUCH)` for
    /// the pouch, with `QUICKBAR = 10` and `POUCH = 16`. So a document that leaves this list
    /// empty ships a character whose quickbar and pouch are both empty -- one omission, two
    /// missing categories, which is exactly what "only the physick came through" was.
    ///
    /// A row here carries `equipIndex` and no `equipSet`: the planner's `setSlotEquipIndex`
    /// only touches `equipSet` when it is given a category, and it is never given one for tools.
    /// [`Slot::equipped_without_set`] is the constructor that respects that.
    pub tools: SlotList,
    /// Physick tears; entries are `null` when a slot is left empty. Always
    /// [`CRYSTAL_TEAR_SLOTS`] long.
    #[serde(rename = "crystalTears")]
    pub crystal_tears: Vec<Option<String>>,
    /// Flask allocation.
    pub flasks: Flasks,
}

impl Default for Items {
    fn default() -> Self {
        Self {
            ammo: Ammo::default(),
            tools: SlotList::default(),
            crystal_tears: vec![None; CRYSTAL_TEAR_SLOTS],
            flasks: Flasks::default(),
        }
    }
}

/// The four ammunition positions, each holding an item name.
///
/// # The one category that is not a slot list, and the one whose key is the position
///
/// Every other category is `{slots: [...]}` of [`Slot`] objects. Ammo is a flat object keyed by
/// equip position whose value is the bare name string --
/// `{"arrow1": "Bone Arrow", "bolt2": "Lightning Bolt"}` -- so there is no `order`, no
/// `equipIndex`, no `equipSet` and no quantity. Taken from the planner's own code, not inferred:
/// its picker writes `character.items.ammo[slot] = ammo.name`, its equip view reads
/// `e.arrow1 ? {name: e.arrow1} : null` for each of the four, and unequipping runs
/// `delete items.ammo[slot]` followed by deleting the whole object once nothing is left.
///
/// That last detail is why every field is skipped rather than written as `null`: an unequipped
/// position on the planner's own documents is an absent key, and `{}` -- what this serialises to
/// when the character carries no ammunition -- is exactly `makeDefault`'s value.
///
/// # There was an older shape and it is not a variant to support
///
/// The planner's migration runs `if (items.ammo && 'slots' in items.ammo) delete items.ammo`, so
/// writing a `{"slots": [...]}` ammo object is writing a document the planner has already decided
/// is unreadable. `er_build_import_core::model::Ammo` carries the same note from the read side.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Ammo {
    /// First arrow position, `ChrAsmSlot::Arrow1`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arrow1: Option<String>,
    /// Second arrow position, `ChrAsmSlot::Arrow2`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arrow2: Option<String>,
    /// First bolt position, `ChrAsmSlot::Bolt1`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bolt1: Option<String>,
    /// Second bolt position, `ChrAsmSlot::Bolt2`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bolt2: Option<String>,
}

impl Ammo {
    /// Put `name` in the position the planner calls `key`, answering whether the key was one of
    /// the four.
    ///
    /// The answer is the point. A caller here holds a key it read from a shared table, and the
    /// failure this guards is the one the whole document design fears: a key the planner does not
    /// know is not an error anywhere downstream -- it rides through the encoder, the URL and
    /// `JSON.parse`, and is simply never looked at, so the ammunition silently does not arrive.
    /// Refusing here makes that a value a caller can log instead of a discovery on the website.
    pub fn set(&mut self, key: &str, name: impl Into<String>) -> bool {
        let slot = match key {
            "arrow1" => &mut self.arrow1,
            "arrow2" => &mut self.arrow2,
            "bolt1" => &mut self.bolt1,
            "bolt2" => &mut self.bolt2,
            _ => return false,
        };
        *slot = Some(name.into());
        true
    }

    /// How many of the four positions are filled.
    pub fn filled(&self) -> usize {
        [&self.arrow1, &self.arrow2, &self.bolt1, &self.bolt2]
            .into_iter()
            .filter(|slot| slot.is_some())
            .count()
    }
}

/// Flask allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Flasks {
    /// Flasks allocated to Crimson Tears.
    pub crimson: u32,
    /// Flasks allocated to Cerulean Tears.
    pub cerulean: u32,
    /// Total flasks, i.e. Golden Seeds spent.
    pub total: u32,
    /// Flask potency, i.e. Sacred Tears drunk.
    pub level: u32,
}

impl Default for Flasks {
    /// `makeDefault`'s allocation: ten crimson, four cerulean, twelve sacred tears.
    fn default() -> Self {
        Self {
            crimson: 10,
            cerulean: 4,
            total: 14,
            level: 12,
        }
    }
}

/// Which view each pane of the planner UI is showing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Views {
    /// The overview pane. `makeDefault` picks between `"build-planner"` and `"swappa"`
    /// behind a feature flag; the former is the current default.
    pub overview: String,
}

/// `makeDefault`'s overview view.
pub const DEFAULT_OVERVIEW_VIEW: &str = "build-planner";

impl Default for Views {
    fn default() -> Self {
        Self {
            overview: DEFAULT_OVERVIEW_VIEW.to_string(),
        }
    }
}

/// Conditional-effect toggles.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Conditions {
    /// Effects the author has forced on. Shape is the planner's business.
    #[serde(rename = "forEffects")]
    pub for_effects: Vec<serde_json::Value>,
    /// Whether physick tear effects count as active.
    #[serde(rename = "crystalTears")]
    pub crystal_tears: bool,
}

/// Named equipment sets, one list per category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sets {
    /// Armament sets.
    pub weapons: Vec<SetEntry>,
    /// Talisman sets.
    pub talismans: Vec<SetEntry>,
    /// Armour sets.
    pub protectors: Vec<SetEntry>,
}

impl Default for Sets {
    fn default() -> Self {
        Self {
            weapons: vec![SetEntry::default()],
            talismans: vec![SetEntry::default()],
            protectors: vec![SetEntry::default()],
        }
    }
}

/// One named equipment set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SetEntry {
    /// The set's display name.
    pub name: String,
    /// Whether this is the set currently being edited.
    pub active: bool,
}

impl Default for SetEntry {
    fn default() -> Self {
        Self {
            name: DEFAULT_SET_NAME.to_string(),
            active: true,
        }
    }
}

/// The cloud account a stored build belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Author {
    /// Account uuid.
    pub id: String,
    /// Display name.
    pub name: String,
}

/// What a finished document actually carries, per category.
///
/// # Counted off the document, never off whatever produced it
///
/// This is the whole point of the type, and it lives beside the document rather than beside the
/// game-side reader for exactly that reason. The thing that was read and the thing that gets
/// encoded are two different objects, and the gap between them is the defect this exists to
/// expose: `items.tools` was never assigned, so a report derived from the read would have said
/// "five quickbar items" about a link that carried none, and the only place the truth appeared
/// was the planner's own screen. A number taken from the encoded document cannot say that.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WrittenCategories {
    /// Armaments the document holds -- carried, worn ones included.
    pub armaments: usize,
    /// Armour pieces across all four body parts.
    pub protectors: usize,
    /// Talismans.
    pub talismans: usize,
    /// Memorised spells.
    pub spells: usize,
    /// Tool rows on a quickbar position (`equipIndex < QUICKBAR_POSITIONS`).
    pub quickbar: usize,
    /// Tool rows on a pouch position (`QUICKBAR_POSITIONS..POUCH_POSITIONS_TOTAL`).
    pub pouch: usize,
    /// Tool rows carried with no equip position at all.
    pub tools_unassigned: usize,
    /// Filled ammunition positions, of four.
    pub ammo: usize,
    /// Physick tears, of [`CRYSTAL_TEAR_SLOTS`].
    pub physick: usize,
    /// Whether the document names an equipped great rune.
    pub great_rune: bool,
    /// Whether it carries the character's appearance.
    pub face_data: bool,
}

impl BuildExportDoc {
    /// Count what this document carries, per category.
    ///
    /// A tool row whose `equipIndex` is past [`POUCH_POSITIONS_TOTAL`] is counted as
    /// **unassigned** rather than as a pouch item: the planner's equip view builds an array that
    /// long and folds by index, so such a row lands nowhere on screen, and calling it a pouch item
    /// would put a number in the log for something the reader will never see.
    pub fn written_categories(&self) -> WrittenCategories {
        let mut quickbar = 0;
        let mut pouch = 0;
        let mut unassigned = 0;
        for slot in &self.items.tools.slots {
            match slot.equip_index.map(|index| index as usize) {
                Some(index) if index < QUICKBAR_POSITIONS => quickbar += 1,
                Some(index) if index < POUCH_POSITIONS_TOTAL => pouch += 1,
                _ => unassigned += 1,
            }
        }
        WrittenCategories {
            armaments: self.inventory.slots.len(),
            protectors: [
                &self.protectors.head,
                &self.protectors.body,
                &self.protectors.arms,
                &self.protectors.legs,
            ]
            .into_iter()
            .map(|list| list.slots.len())
            .sum(),
            talismans: self.talismans.slots.len(),
            spells: self.spells.slots.len(),
            quickbar,
            pouch,
            tools_unassigned: unassigned,
            ammo: self.items.ammo.filled(),
            physick: self.items.crystal_tears.iter().flatten().count(),
            great_rune: self.great_rune.is_some(),
            face_data: self.face_data.is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key `makeDefault` writes, transcribed from the live bundle. `computed` and
    /// `activeEffects` are absent from that list because `makeDefault` does not write them
    /// either -- they are populated later, by `populateComputedValues`. Nor is `greatRune`,
    /// which the planner only writes once a rune is equipped; see
    /// [`super::BuildExportDoc::great_rune`].
    const MAKE_DEFAULT_KEYS: &[&str] = &[
        "buildUrl",
        "description",
        "id",
        "inventory",
        "talismans",
        "name",
        "stats",
        "spells",
        "protectors",
        "items",
        "characterClass",
        "version",
        "author",
        "weaponUpgrade",
        "is2h",
        "isPvE",
        "views",
        "conditions",
        "sets",
        "tags",
        "customEffects",
    ];

    fn as_object(doc: &BuildExportDoc) -> serde_json::Map<String, serde_json::Value> {
        match serde_json::to_value(doc).expect("a plain data document always serialises") {
            serde_json::Value::Object(map) => map,
            other => panic!("document serialised to {other:?}, not an object"),
        }
    }

    #[test]
    fn default_document_carries_exactly_the_planner_default_key_set() {
        let map = as_object(&BuildExportDoc::default());
        let mut got: Vec<&str> = map.keys().map(String::as_str).collect();
        let mut want = MAKE_DEFAULT_KEYS.to_vec();
        got.sort_unstable();
        want.sort_unstable();
        assert_eq!(got, want);
    }

    #[test]
    fn great_rune_is_absent_by_default_and_present_once_set() {
        // Absent, not null: the planner's read is guarded, and `makeDefault` omits the key.
        assert!(!as_object(&BuildExportDoc::default()).contains_key("greatRune"));

        let doc = BuildExportDoc {
            great_rune: Some("Great Rune of the Unborn".to_string()),
            ..BuildExportDoc::default()
        };
        assert_eq!(as_object(&doc)["greatRune"], "Great Rune of the Unborn");
    }

    #[test]
    fn face_data_is_absent_by_default_and_present_once_set() {
        // Absent, not null: the planner has no such key, so writing `null` would put a field on
        // the document that neither side reads.
        assert!(!as_object(&BuildExportDoc::default()).contains_key("faceData"));

        let doc = BuildExportDoc {
            face_data: Some("46414345".to_string()),
            ..BuildExportDoc::default()
        };
        assert_eq!(as_object(&doc)["faceData"], "46414345");
    }

    #[test]
    fn derived_values_are_never_written() {
        let map = as_object(&BuildExportDoc::default());
        assert!(!map.contains_key("computed"));
        assert!(!map.contains_key("activeEffects"));
    }

    #[test]
    fn spells_carry_no_sorting_key() {
        let map = as_object(&BuildExportDoc::default());
        let spells = map["spells"].as_object().expect("spells is an object");
        assert_eq!(spells.keys().collect::<Vec<_>>(), vec!["slots"]);
    }

    #[test]
    fn every_other_category_carries_sorting() {
        let map = as_object(&BuildExportDoc::default());
        for key in ["inventory", "talismans"] {
            assert!(map[key].get("sorting").is_some(), "{key} lost its sorting");
        }
        for part in ["arms", "body", "head", "legs"] {
            assert!(map["protectors"][part].get("sorting").is_some());
        }
        assert!(map["items"]["tools"].get("sorting").is_some());
    }

    #[test]
    fn stat_keys_are_the_planner_abbreviations() {
        let map = as_object(&BuildExportDoc::default());
        let mut keys: Vec<&str> = map["stats"]
            .as_object()
            .expect("stats is an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["arc", "dex", "fth", "int", "mnd", "rl", "str", "vig", "vit"]
        );
    }

    #[test]
    fn endurance_is_written_to_vit() {
        let doc = BuildExportDoc {
            stats: Stats {
                endurance: 40,
                vigor: 60,
                ..Stats::default()
            },
            ..BuildExportDoc::default()
        };
        let map = as_object(&doc);
        assert_eq!(map["stats"]["vit"], 40);
        assert_eq!(map["stats"]["vig"], 60);
    }

    #[test]
    fn unset_slot_options_are_absent_rather_than_null() {
        let doc = BuildExportDoc {
            inventory: SlotList::new(vec![Slot::carried("Dryleaf Arts", 0)]),
            ..BuildExportDoc::default()
        };
        let map = as_object(&doc);
        let slot = map["inventory"]["slots"][0]
            .as_object()
            .expect("a slot is an object");
        assert_eq!(slot.keys().collect::<Vec<_>>(), vec!["name", "order"]);
    }

    #[test]
    fn equipping_a_slot_writes_both_keys() {
        let slot = Slot::carried("Silver Tear Mask", 1).equipped_at(1);
        assert!(slot.is_equipped());
        let value = serde_json::to_value(&slot).expect("a slot always serialises");
        assert_eq!(value["equipIndex"], 1);
        assert_eq!(value["equipSet"], serde_json::json!([1]));
    }

    #[test]
    fn weapon_upgrade_follows_the_planner_derivation() {
        assert_eq!(weapon_upgrade_for_level(0), DEFAULT_WEAPON_UPGRADE);
        assert_eq!(weapon_upgrade_for_level(-5), DEFAULT_WEAPON_UPGRADE);
        assert_eq!(weapon_upgrade_for_level(1), 0);
        assert_eq!(weapon_upgrade_for_level(5), 1);
        assert_eq!(weapon_upgrade_for_level(9), 1);
        assert_eq!(weapon_upgrade_for_level(60), 12);
        assert_eq!(weapon_upgrade_for_level(125), 25);
        assert_eq!(weapon_upgrade_for_level(713), DEFAULT_WEAPON_UPGRADE);
    }

    #[test]
    fn with_level_sets_the_level_and_the_derived_upgrade() {
        let doc = BuildExportDoc::with_level(60, true);
        assert_eq!(doc.stats.rune_level, 60);
        assert_eq!(doc.weapon_upgrade, 12);
        assert!(doc.pve);
    }

    #[test]
    fn ammo_defaults_to_the_planners_own_empty_object() {
        // `makeDefault` writes `ammo: {}`, and unequipping the last position deletes every key
        // again -- so an empty ammo object, not a missing one and not four nulls.
        let map = as_object(&BuildExportDoc::default());
        assert_eq!(map["items"]["ammo"], serde_json::json!({}));
    }

    #[test]
    fn ammo_writes_only_the_four_planner_keys_and_bare_names() {
        let mut ammo = Ammo::default();
        assert!(ammo.set("arrow1", "Bone Arrow"));
        assert!(ammo.set("bolt2", "Ballista Bolt"));
        // A key the planner does not know is refused rather than written: it would survive the
        // whole pipeline and simply never be read.
        assert!(!ammo.set("arrow3", "Great Arrow"));
        assert!(!ammo.set("slots", "Great Arrow"));

        let doc = BuildExportDoc {
            items: Items {
                ammo,
                ..Items::default()
            },
            ..BuildExportDoc::default()
        };
        // Bare strings, and only the filled positions -- an empty one is an absent key.
        assert_eq!(
            as_object(&doc)["items"]["ammo"],
            serde_json::json!({"arrow1": "Bone Arrow", "bolt2": "Ballista Bolt"})
        );
    }

    #[test]
    fn ammo_key_set_is_exactly_the_planners() {
        let mut ammo = Ammo::default();
        for key in ["arrow1", "arrow2", "bolt1", "bolt2"] {
            assert!(ammo.set(key, "x"), "{key} is a planner ammo position");
        }
        assert_eq!(ammo.filled(), 4);
        let value = serde_json::to_value(&ammo).expect("plain data always serialises");
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("ammo is an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["arrow1", "arrow2", "bolt1", "bolt2"]);
    }

    #[test]
    fn an_equipped_tool_carries_equip_index_and_no_equip_set() {
        // The whole key set of a tool row, as the captured payload spells it.
        let slot = Slot::carried("Blessing of Marika", 4).equipped_without_set(10);
        assert!(slot.is_equipped());
        let value = serde_json::to_value(&slot).expect("a slot always serialises");
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("a slot is an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["equipIndex", "name", "order"]);
        assert_eq!(value["equipIndex"], 10);
    }

    #[test]
    fn an_empty_document_counts_every_category_at_zero() {
        // The number a report has to be able to print. Before the categories were written at all,
        // nothing in the exporter could produce a zero for the quickbar -- it produced no number.
        assert_eq!(
            BuildExportDoc::default().written_categories(),
            WrittenCategories {
                physick: 0,
                ..WrittenCategories::default()
            }
        );
    }

    #[test]
    fn tool_rows_are_counted_by_which_side_of_the_quickbar_split_they_fall() {
        let doc = BuildExportDoc {
            items: Items {
                tools: SlotList::new(vec![
                    Slot::carried("a", 0).equipped_without_set(0),
                    Slot::carried("b", 1).equipped_without_set(QUICKBAR_POSITIONS as u32 - 1),
                    Slot::carried("c", 2).equipped_without_set(QUICKBAR_POSITIONS as u32),
                    Slot::carried("d", 3).equipped_without_set(POUCH_POSITIONS_TOTAL as u32 - 1),
                    // Carried, on no position.
                    Slot::carried("e", 4),
                    // Past the planner's own array: it lands nowhere on screen, so it is not a
                    // pouch item and must not be reported as one.
                    Slot::carried("f", 5).equipped_without_set(POUCH_POSITIONS_TOTAL as u32),
                ]),
                ..Items::default()
            },
            ..BuildExportDoc::default()
        };
        let counts = doc.written_categories();
        assert_eq!(counts.quickbar, 2);
        assert_eq!(counts.pouch, 2);
        assert_eq!(counts.tools_unassigned, 2);
    }

    #[test]
    fn the_counts_come_from_the_document_and_not_from_its_construction() {
        // A document whose tool list was never assigned reports zero, which is the line the defect
        // would have shown had anything been counting.
        let doc = BuildExportDoc {
            items: Items {
                ammo: Ammo::default(),
                ..Items::default()
            },
            ..BuildExportDoc::default()
        };
        let counts = doc.written_categories();
        assert_eq!(counts.quickbar, 0);
        assert_eq!(counts.pouch, 0);
        assert_eq!(counts.ammo, 0);
    }

    #[test]
    fn physick_always_has_two_tear_slots() {
        let map = as_object(&BuildExportDoc::default());
        assert_eq!(
            map["items"]["crystalTears"],
            serde_json::json!([null, null]),
        );
    }
}
