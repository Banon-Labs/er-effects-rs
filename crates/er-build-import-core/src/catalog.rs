//! The item catalog: name -> item id, scoped by category.
//!
//! # Why lookup must be category-scoped
//!
//! Elden Ring reuses item names across categories, so a flat name->id map grants
//! the wrong item. Measured against the planner's own database (2063 entries),
//! seven names are ambiguous:
//!
//! | name | spell | ash of war | other |
//! |---|---|---|---|
//! | `Golden Vow` | 6600 | 60300 | consumable 2003170 |
//! | `Beast Claw` | 6820 | -- | weapon 68500000 |
//! | `Glintstone Pebble` | 4000 | 20300 | -- |
//! | `Glintblade Phalanx` | 4300 | 20000 | -- |
//! | `Carian Greatsword` | 4430 | 21900 | -- |
//! | `Carian Retaliation` | 4640 | 30500 | -- |
//! | `Thops's Barrier` | 4630 | 31000 | -- |
//!
//! A build that lists `Golden Vow` under `spells` and a weapon whose `weaponArt`
//! is `Golden Vow` means two different item ids from one string, so [`Catalog`]
//! takes the [`Kind`] as part of the key rather than as a hint.

use std::collections::HashMap;

use crate::name::fold;

/// Which of the planner's seven item databases a name should be looked up in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// Armaments, including staves, seals and shields.
    Weapon,
    /// Head / body / arms / legs armour.
    Protector,
    /// Talismans.
    Talisman,
    /// Sorceries and incantations.
    Spell,
    /// Consumables, crafting items and pots.
    Tool,
    /// Ashes of war, referenced by a weapon slot's `weaponArt`.
    AshOfWar,
    /// Arrows and bolts.
    Ammo,
    /// Great runes.
    ///
    /// A separate kind rather than a subset of [`Kind::Tool`] because the goods table contains
    /// more than one row named e.g. "Godrick's Great Rune", and only rows 191..=196 are
    /// accepted by the game: `GetGreatruneEnumByGoodsId` switches on exactly those six and
    /// returns "none" for anything else. A plain name lookup picked row 8148 and the rune
    /// silently failed to equip.
    GreatRune,
}

impl Kind {
    /// Human-readable label, used in unresolved-item reports.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Weapon => "weapon",
            Kind::Protector => "armour",
            Kind::Talisman => "talisman",
            Kind::Spell => "spell",
            Kind::Tool => "tool",
            Kind::AshOfWar => "ash of war",
            Kind::Ammo => "ammo",
            Kind::GreatRune => "great rune",
        }
    }
}

/// One catalog row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// The category-tagged item id: `(category_nibble << 28) | param_id`.
    ///
    /// Verified across all 2063 catalog entries: the low 28 bits are exactly the
    /// param row id, and the nibble is 0 for weapons/ammo, 1 for protectors,
    /// 2 for talismans and ashes of war, 4 for goods (spells and tools).
    pub full_item_id: u32,
    /// How many the game itself lets the player hold.
    ///
    /// `None` means the question was not asked or was not answered -- a category the engine has
    /// no limit for, or a row that declares none -- and the grant path reads that as "just one".
    ///
    /// # Whose number this is
    ///
    /// Not a comfortable stack size picked here. `CS::EquipInventoryData::GetMaxAmountForItem`
    /// (1.16.2 `0x14024e570`, same address on 1.17) is the engine's entire answer to "how many
    /// more of this may be added", and for a goods item that is not pot-capped its body reduces
    /// to `paramRow->maxNum > 0 ? paramRow->maxNum : 99`. So this is the ceiling the engine will
    /// enforce whatever the importer asks for.
    ///
    /// # two fields in two tables, and which one this holds depends on the [`Kind`]
    ///
    /// `maxNum` is a goods field, and the engine reads it only for the goods category: every
    /// other category is tail-jumped to `::GetMaxItemQuantity` (1.16.2 `0x140674680`, 1.17
    /// `0x1406754d0`), which answers `1` for an armament, a protector and a talisman -- and, for
    /// a weapon row whose `weaponCategory` is 13 or 14, `EquipParamWeapon.maxArrowQuantity`.
    ///
    /// So for [`Kind::Ammo`] this field is `maxArrowQuantity`, not `maxNum`; ammunition is an
    /// `EquipParamWeapon` row and has no `maxNum` to read. Same meaning -- the engine's own
    /// ceiling for this item -- from the table the item actually lives in.
    ///
    /// It is also what the planner's own item database records under this name. Measured against
    /// the installed 1.17 `regulation.bin`, every tool, great rune and tear row the test fixture
    /// carries matches `maxNum` exactly -- Clarifying Boluses 99, Flask of Cerulean Tears 20,
    /// Opaline Pickled Liver 5, Blessing of Marika 1, Mohg's Great Rune 1. The planner's spell
    /// rows are the one place the two disagree (it records `maxRepositoryNum`, 600, where the
    /// game's `maxNum` is 99), and a spell is granted one copy either way, so nothing reads this
    /// for them.
    ///
    /// # Not a delivery promise
    ///
    /// A pot-group item is capped far below `maxNum` by the mechanism [`Entry::pot_group`]
    /// documents, and both acquisition paths clamp to it in silence. This says what the item's
    /// own row permits, not what an add will deliver.
    pub max_stored: Option<u32>,
    /// Whether the armament upgrades with Somber Smithing Stones.
    pub somber: bool,
    /// `EquipParamGoods.potGroupId` when this item is one the game POT-caps, else `None`.
    ///
    /// # The one inventory limit an importer cannot see coming
    ///
    /// A consumable in a pot group can be held only up to the number of Cracked Pots (or Ritual
    /// Pots, or Perfume Bottles) sharing that group -- `EquipInventoryData::UpdatePotsStates`
    /// (0x14024e930) sums the group's *materials* into `potItemsCapacity[16]` and its
    /// *consumables* into `potItemsCount[16]`, and `GetMaxAmountForItem` (0x14024e570) hands out
    /// the difference. Both acquisition paths then clamp to it with a silent
    /// `if (max < amount) amount = max;` -- `InsertItem` (0x14024cfd0) and `UpdateQuantity`
    /// (0x14024d760) -- so a grant of five Fire Pots against a full group delivers three, returns
    /// no error, and sets no result code.
    ///
    /// # Consumables only, never the material
    ///
    /// This mirrors `EquipParamGoodsLookupResult::IsPotConsumable` exactly
    /// (1.16.2 `0x140d3a190`, 1.17 `0x140d3b8e0`, byte-identical): `goodsType == 0` (NORMAL_ITEM)
    /// **and** `potGroupId >= 0`. The Cracked Pot itself is `goodsType == 0x0b`
    /// (REGENERATIVE_MATERIAL) and is deliberately excluded, because it is what supplies the
    /// capacity -- a caller freeing pot space by depositing group members would otherwise deposit
    /// the pots that create the space and make the problem worse.
    ///
    /// Measured against the installed regulation by
    /// `scripts/regulation-potgroup-census.py`: 67 rows across 4 groups, each with exactly one
    /// material (9500 / 9510 / 9501 / 2009500).
    pub pot_group: Option<u8>,
}

/// Resolves an item name, within a category, to its id.
///
/// A trait rather than a concrete table so the id source is swappable: an
/// offline table for tests, and (later) a resolver that reads the game's own
/// param/FMG data at runtime instead of shipping a copy of someone else's
/// database.
pub trait Catalog {
    /// Look up `name` (unfolded; the implementation folds it) within `kind`.
    fn lookup(&self, kind: Kind, name: &str) -> Option<Entry>;

    /// Ids other than [`Self::lookup`]'s answer that the same name also resolves to.
    ///
    /// Elden Ring gives each upgrade level of a flask its own goods row, and every row carries the
    /// same name -- so "Flask of Crimson Tears" is a dozen ids, not one. A caller that treats
    /// `lookup`'s single answer as "the" id will conclude the player does not hold an item they
    /// are visibly carrying, because they hold a different row of it.
    ///
    /// Defaults to empty so an offline test table needs no collision machinery.
    fn alternates(&self, _kind: Kind, _name: &str) -> Vec<u32> {
        Vec::new()
    }

    /// Every id that is this item at some upgrade level, the primary included.
    ///
    /// [`Self::alternates`] is not enough for the question "does the player hold this". It
    /// answers with the rows carrying the identical name, and Elden Ring does not name an
    /// upgraded flask identically: the live catalog enumerates `flask of crimson tears` at goods
    /// 1000/1001 and `flask of crimson tears +9` at 1018/1019, four distinct rows of one belt
    /// item under two distinct names. A character who has drunk a single Sacred Tear holds none
    /// of the ids `alternates` returns, which is why two imports in a row reported the Crimson
    /// and Cerulean flasks `NOT-IN-INVENTORY` while they sat visibly in the pouch -- run
    /// 2026-08-25 missed 0x400003E9/0x4000041B and run 2026-08-31 missed 0x400003E8/0x4000041A,
    /// i.e. Both rows of the unupgraded name.
    ///
    /// The suffix is the game's own: the upgraded row's `GoodsName` is the base name plus
    /// ` +N`. Talismans are the same shape (`Erdtree's Favor +2`), so this is not a flask
    /// special case. Armaments are not -- their level lives in the last two digits of the id and
    /// is handled arithmetically -- so a weapon name simply has no `+N` siblings and this
    /// returns the primary alone.
    ///
    /// Defaults to `alternates` plus nothing, so an offline test table needs no name scan.
    fn upgrade_variants(&self, kind: Kind, name: &str) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .lookup(kind, name)
            .map(|entry| entry.full_item_id)
            .into_iter()
            .collect();
        ids.extend(self.alternates(kind, name));
        ids
    }
}

/// An in-memory catalog keyed by `(kind, folded name)`.
///
/// # Why a name can map to more than one id
///
/// The catalog is built by enumerating the game's param rows, asking each for its name, and
/// inverting. Names are not unique: several rows can share one, and a plain `insert` therefore
/// let the last row enumerated silently win. That is not a cosmetic loss -- the importer asks
/// "does the player already hold this?" about the id it resolved, so picking the other row means
/// asking about an item nobody has, concluding the player has none, and granting a duplicate. It
/// is exactly how importing a build produced a second Flask of Wondrous Physick beside the one
/// already in the inventory.
///
/// So every id is kept. `lookup` still answers with one, because callers want one, but the
/// alternates are available to a caller that can ask a better question -- see
/// [`MapCatalog::alternates`], which the grant path uses to check every candidate for an already
/// held copy before it adds anything.
#[derive(Debug, Clone, Default)]
pub struct MapCatalog {
    entries: HashMap<(Kind, String), Entry>,
    /// Every entry that lost a name collision, in insertion order under its key.
    alternates: HashMap<(Kind, String), Vec<Entry>>,
}

impl MapCatalog {
    /// An empty catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one row. Returns `self` so tables can be built in one expression.
    pub fn with(mut self, kind: Kind, name: &str, entry: Entry) -> Self {
        self.insert(kind, name, entry);
        self
    }

    /// Add one row.
    ///
    /// A second row under a name already present does not replace the first: the first stays the
    /// primary answer and the newcomer joins [`Self::alternates`]. Keeping the first is what makes
    /// the resolution stable -- param enumeration order is the game's, not ours, and a catalog
    /// whose answers shuffle between patches is worse than one that answers consistently.
    pub fn insert(&mut self, kind: Kind, name: &str, entry: Entry) {
        let key = (kind, fold(name));
        match self.entries.get(&key) {
            Some(existing) if *existing == entry => {}
            Some(_) => self.alternates.entry(key).or_default().push(entry),
            None => {
                self.entries.insert(key, entry);
            }
        }
    }

    /// Other rows that share `name` within `kind`, in the order the game enumerated them.
    ///
    /// Empty for the overwhelming majority of items; non-empty is the signal that a bare id
    /// cannot identify what the player is holding.
    pub fn alternates(&self, kind: Kind, name: &str) -> &[Entry] {
        self.alternates
            .get(&(kind, fold(name)))
            .map_or(&[], Vec::as_slice)
    }

    /// Every id under `name` or under `name +N`, within `kind`, sorted and deduplicated.
    ///
    /// See [`Catalog::upgrade_variants`] for why the `+N` rows have to be in the answer. The
    /// match is on the folded name, and [`fold`] preserves `+`, so `"Erdtree's Favor +2"` folds
    /// to `"erdtree's favor +2"` and is found by the prefix `"erdtree's favor +"`. The trailing
    /// space in that prefix is load-bearing: without it `"flask of crimson tears"` would also
    /// claim a hypothetical `"flask of crimson tears of something else"`.
    pub fn upgrade_variants(&self, kind: Kind, name: &str) -> Vec<u32> {
        let base = fold(name);
        let prefix = format!("{base} +");
        let is_family =
            |key: &(Kind, String)| key.0 == kind && (key.1 == base || key.1.starts_with(&prefix));
        let mut ids: Vec<u32> = self
            .entries
            .iter()
            .filter(|(key, _)| is_family(key))
            .map(|(_, entry)| entry.full_item_id)
            .chain(
                self.alternates
                    .iter()
                    .filter(|(key, _)| is_family(key))
                    .flat_map(|(_, extra)| extra.iter().map(|entry| entry.full_item_id)),
            )
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Every `(kind, folded name)` that resolved to more than one id, with all of its ids.
    ///
    /// Reported at import time rather than discovered from a duplicated item later.
    pub fn collisions(&self) -> Vec<(Kind, String, Vec<u32>)> {
        let mut found: Vec<(Kind, String, Vec<u32>)> = self
            .alternates
            .iter()
            .map(|((kind, name), extra)| {
                let mut ids: Vec<u32> = self
                    .entries
                    .get(&(*kind, name.clone()))
                    .map(|primary| primary.full_item_id)
                    .into_iter()
                    .collect();
                ids.extend(extra.iter().map(|entry| entry.full_item_id));
                (*kind, name.clone(), ids)
            })
            .collect();
        found.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        found
    }

    /// Number of rows.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the catalog holds no rows.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Catalog for MapCatalog {
    fn lookup(&self, kind: Kind, name: &str) -> Option<Entry> {
        self.entries.get(&(kind, fold(name))).copied()
    }

    fn alternates(&self, kind: Kind, name: &str) -> Vec<u32> {
        MapCatalog::alternates(self, kind, name)
            .iter()
            .map(|entry| entry.full_item_id)
            .collect()
    }

    fn upgrade_variants(&self, kind: Kind, name: &str) -> Vec<u32> {
        MapCatalog::upgrade_variants(self, kind, name)
    }
}

/// Build a simple entry with no known hold limit and standard upgrade material.
///
/// `max_stored: None` means a caller building a table by hand gets one of the item, which is the
/// right default for the armaments and armour this helper exists for. A consumable wants the
/// game's own number; see [`Entry::max_stored`].
pub fn entry(full_item_id: u32) -> Entry {
    Entry {
        full_item_id,
        max_stored: None,
        somber: false,
        pot_group: None,
    }
}
