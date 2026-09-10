//! Building the item catalog from the running game.
//!
//! The planner payload names items and gives no ids, so importing a build needs a
//! name -> id map. The obvious source is the planner's own bundled database, but the
//! game already holds the same table -- in the player's language, at the player's patch
//! level, DLC included -- so this asks the game instead of shipping a copy of someone
//! else's dataset.
//!
//! # Two halves, both borrowed rather than invented
//!
//! *Rows* come from [`SoloParamRepository::rows`](eldenring::cs::SoloParamRepository::rows), which
//! yields `(row_id, &row)` for a param table. *Names* come from the game's own per-category
//! getters, each a one-call wrapper around `MsgRepositoryImp::LookupEntry` that walks its own
//! bundle chain.
//!
//! Calling those wrappers rather than reimplementing the chain is not fastidiousness.
//! `GetGoodsName` consults **four** bundles (`0x6f`, `0xa`, `0x13f`, `0x1a3`), while the
//! neighbouring categories look like a tidy base/dlc01 pair -- an importer that inferred
//! the pattern from its neighbours would silently lose two whole bundles of consumables.
//!
//! # The map runs the wrong way, on purpose
//!
//! The game answers *id -> name*; the importer needs *name -> id*. So the table is built
//! by enumerating every row, asking for its name, and inverting. That is a few thousand
//! calls once, not per lookup.

use std::collections::BTreeSet;

use er_build_import_core::catalog::{Entry, Kind, MapCatalog};

/// `MsgRepositoryImp::LookupEntry` returns a placeholder like `?GoodsName?` for a row that
/// has no name. Those are misses, not items, and a catalog that accepted them would
/// happily resolve `"?GoodsName?"` to whichever row asked last.
const PLACEHOLDER_PREFIX: char = '?';

/// The other placeholder, and the one that actually ships in the message files: FromSoftware names
/// its dummy and unused rows `[ERROR]`, `[ERROR]Type 1`, `[ERROR]type 10`, and so on. There are
/// hundreds of them -- they are why a catalog build reports around a hundred colliding names, all
/// of them this one string -- and not one is an item. Treating them as names lets a slot holding a
/// dummy row export as an item nothing can resolve, and lets a build asking for `"[error]"` import
/// whichever dummy row was enumerated last. Verified against the shipped message files:
/// `python3 scripts/er-item-name.py ProtectorName 1000` answers `'[ERROR]Type 1'`.
const ERROR_PLACEHOLDER_PREFIX: &str = "[error]";

/// RVAs of the game's per-category name getters, verified in the 1.16.2 dump.
///
/// Each takes `(MsgRepositoryImp*, u32 row_id)` and returns a NUL-terminated `wchar_t*`.
mod rva {
    /// `MsgRepositoryImp::GetWeaponName` -- base bundle `0x136`.
    pub const GET_WEAPON_NAME_RVA: usize = 0xd11370;
    /// `MsgRepositoryImp::GetProtectorName` -- base bundle `0x139`.
    pub const GET_PROTECTOR_NAME_RVA: usize = 0xd10d90;
    /// `MsgRepositoryImp::GetAccessoryName` -- base bundle `0x13c`, DLC `0x1a0`.
    pub const GET_ACCESSORY_NAME_RVA: usize = 0xd0fda0;
    /// `MsgRepositoryImp::GetGoodsName` -- bundles `0x6f`, `0xa`, `0x13f`, `0x1a3`.
    pub const GET_GOODS_NAME_RVA: usize = 0xd10600;
    /// `MsgRepositoryImp::GetGemName` -- base bundle `0x142`, DLC `0x1a6`.
    ///
    /// Names the *gem item* ("Ash of War: Lion's Claw"), which is not what a build's
    /// `weaponArt` says -- so it is not what the catalog keys on. It is used as the test for
    /// whether a gem row is a real, obtainable ash rather than a development placeholder,
    /// which is how the ash catalog picks between several gems carrying one skill.
    pub const GET_GEM_NAME_RVA: usize = 0xd103d0;
    /// `MsgRepositoryImp::GetArtsName` -- base bundle `0x14b`.
    ///
    /// Names the *skill* ("Lion's Claw"), keyed by `SwordArtsParam` row id -- which is
    /// exactly what a build's `weaponArt` carries and what `weaponSkill` encodes.
    pub const GET_ARTS_NAME_RVA: usize = 0xd0ff70;
}

/// The category tag the game ORs into the high nibble of an item id.
///
/// Confirmed by the game's own code rather than inferred from the planner's data:
/// `EquipGreatRune` rejects anything whose `id & 0xF0000000` is not `0x40000000` and takes
/// the row id as `id & 0x0FFFFFFF`.
/// `repr(u32)` because the gem category's discriminant sets bit 31, which does not fit a signed
/// pointer-width discriminant on a 32-bit target -- and the tags are u32 item ids, not indices.
#[derive(Clone, Copy)]
#[repr(u32)]
enum Tag {
    Weapon = 0x0000_0000,
    Protector = 0x1000_0000,
    Accessory = 0x2000_0000,
    Goods = 0x4000_0000,
    /// `EquipParamGem`, i.e. an ash of war as an item.
    ///
    /// Same evidence as the others, from the same switch: `GetGaitemHandleByItemId` sends
    /// `itemId >> 28 == 8` to `GetGaItemHandleGem`, and `GaitemLookupResult::GetSwordArtsParamId`
    /// takes an id apart as `0x8000_0000 | EquipParamGem row`.
    Gem = 0x8000_0000,
}

/// `(MsgRepositoryImp*, row_id) -> wchar_t*`
type NameGetter = unsafe extern "system" fn(usize, u32) -> *const u16;

/// One category's recipe: where its rows live, what to call them, and how to tag them.
struct Source {
    kind: Kind,
    tag: Tag,
    getter_rva: usize,
}

/// Every category the importer resolves.
///
/// Ashes of war are the one category whose name and ID come from different tables, which is
/// why they are not in this list and are built by [`insert_ashes_of_war`] instead.
///
/// A build's `weaponArt` names the *skill* ("Bloodhound's Step"), which is a `SwordArtsParam`
/// row and is named out of the arts bundle. But the thing the game can put on a weapon is the
/// *gem* ("Ash of War: Bloodhound's Step"), an `EquipParamGem` row -- `GetSwordArtsParamId`
/// reads `0x8000_0000 | gem row`, and `GetGaItemHandleGem` mints from a gem row. So the catalog
/// keys on the arts name and stores the gem id.
///
/// Resolving to the `SwordArtsParam` row instead is the bug this replaces, and it was invisible:
/// every ash in a build resolved, the importer reported `0 unresolved`, and not one weapon came
/// out carrying its ash. The planner's own ids were the tell all along -- 80100, 401000, 505000
/// are multiples of 100 in gem space (`gem = arts * 100` for most ashes), not arts rows.
///
/// Spells and tools are both `EquipParamGoods` rows sharing one name bundle -- a sorcery's
/// item id really is `0x4 << 28 | goodsRowId`. They are separated by whether the same row
/// id also exists in `MAGIC_PARAM_ST`, which is the game's own distinction, not a heuristic.
///
/// Armaments and ammunition are the second such pair: arrows and bolts are `EquipParamWeapon`
/// rows sharing the weapon name bundle and the weapon category nibble, and the game separates
/// them by `weaponCategory` -- see [`Quivers`]. They are split rather than left in one kind
/// because the planner does the same: its build document keeps ammo in `items.ammo`, keyed by
/// equip position, and its item database serves `ammo` from a store of its own beside `weapons`,
/// so a build's `inventory` never names an arrow and a `Kind::Weapon` lookup never needs to find
/// one.
const SOURCES: &[Source] = &[
    Source {
        kind: Kind::Weapon,
        tag: Tag::Weapon,
        getter_rva: rva::GET_WEAPON_NAME_RVA,
    },
    Source {
        kind: Kind::Protector,
        tag: Tag::Protector,
        getter_rva: rva::GET_PROTECTOR_NAME_RVA,
    },
    Source {
        kind: Kind::Talisman,
        tag: Tag::Accessory,
        getter_rva: rva::GET_ACCESSORY_NAME_RVA,
    },
    Source {
        kind: Kind::Spell,
        tag: Tag::Goods,
        getter_rva: rva::GET_GOODS_NAME_RVA,
    },
    Source {
        kind: Kind::Tool,
        tag: Tag::Goods,
        getter_rva: rva::GET_GOODS_NAME_RVA,
    },
    Source {
        kind: Kind::GreatRune,
        tag: Tag::Goods,
        getter_rva: rva::GET_GOODS_NAME_RVA,
    },
    Source {
        kind: Kind::Ammo,
        tag: Tag::Weapon,
        getter_rva: rva::GET_WEAPON_NAME_RVA,
    },
];

/// The only goods rows the game accepts as great runes.
///
/// `GetGreatruneEnumByGoodsId` (0x140d39ab0) switches on exactly `0xbf..=0xc4` -- Godrick,
/// Radahn, Morgott, Rykard, Mohg, Malenia -- and returns "none" for everything else. The goods
/// table holds other rows with the same display names, so restricting the catalog to this
/// range is what makes the name lookup land on a rune the engine will actually equip.
const GREAT_RUNE_ROWS: core::ops::RangeInclusive<u32> = 191..=196;

/// What a catalog build found, for the log.
#[derive(Debug, Default)]
pub struct BuildStats {
    /// Rows that produced a usable name.
    pub named: usize,
    /// Rows whose name was absent or a `?placeholder?`.
    pub unnamed: usize,
    /// How many goods rows were classified as spells.
    pub spell_rows: usize,
    /// Ashes whose only `EquipParamGem` row draws no icon, so the tile badge falls back to the
    /// literal `ICON` placeholder. Non-zero is not a crash and not a wrong ash -- the name and
    /// the skill are right -- but it is the exact defect a player reports as "the ash symbol
    /// says icon", so it is a number here rather than a discovery in-game.
    pub iconless_ashes: usize,
    /// Goods rows the game POT-caps, i.e. rows [`PotGroups`] classified as pot consumables.
    ///
    /// Reported because a zero here means the grant path's pot handling is inert for the session
    /// -- the param table was not readable when the catalog was built -- and that failure is
    /// otherwise indistinguishable from a character who simply has no pot conflicts.
    pub pot_capped_rows: usize,
    /// Goods rows that declared a `maxNum`, i.e. rows [`MaxHeld`] could read.
    ///
    /// Same reason as the field above, for the same table. A zero means every consumable in the
    /// build falls back to one copy each, which looks exactly like a build with no consumables
    /// in it unless the number is in the log.
    pub max_held_rows: usize,
    /// `EquipParamWeapon` rows [`Quivers`] classified as ammunition.
    ///
    /// Reported for a sharper reason than the two above: this number is a denominator for the
    /// armament catalog as well. Ammunition is subtracted from `Kind::Weapon` and added to
    /// `Kind::Ammo`, so a zero here does not merely mean "no arrows resolve" -- it means the
    /// split did not happen, `Kind::Weapon` still holds every arrow, and an ammo name in a build
    /// will come back unresolved while the same name sits in the armament catalog. The installed
    /// 1.17 table has 73; anything far from that is the param read, not the build.
    pub ammo_rows: usize,
    /// Rows the two ammunition classifications disagreed about. See [`Quivers::disagreements`].
    pub ammo_classification_disagreements: usize,
}

/// Read a NUL-terminated UTF-16 string, bounded so a bad pointer cannot run away.
///
/// # Safety
///
/// `ptr` must be null or point to a NUL-terminated UTF-16 string.
unsafe fn read_wide(ptr: *const u16) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // Item names are short; the bound exists so a corrupt pointer fails instead of scanning.
    const MAX_CHARS: usize = 256;
    let mut units = Vec::new();
    for offset in 0..MAX_CHARS {
        // Safety: bounded by MAX_CHARS and stopped at the first NUL.
        let unit = unsafe { *ptr.add(offset) };
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    let text = String::from_utf16(&units).ok()?;
    if text.is_empty() || text.starts_with(PLACEHOLDER_PREFIX) {
        return None;
    }
    // Case-insensitively, because the shipped files spell it both `[ERROR]Type 1` and
    // `[ERROR]type 10` -- a case-sensitive test would let half of them through. Sliced with `get`
    // rather than `[..7]`: item names are UTF-16 from the game and a byte-index slice through a
    // multi-byte character PANICS, which in this crate means taking the game down to avoid
    // exporting a placeholder.
    if text
        .get(..ERROR_PLACEHOLDER_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(ERROR_PLACEHOLDER_PREFIX))
    {
        return None;
    }
    Some(text)
}

/// Ask the game for one row's display name.
///
/// # Safety
///
/// `getter` must be the address of a live `(MsgRepositoryImp*, u32) -> wchar_t*` function
/// and `msg` a valid `MsgRepositoryImp*`.
unsafe fn name_of(getter: usize, msg: usize, row_id: u32) -> Option<String> {
    // Safety: the caller guarantees the address is one of the verified getters.
    let getter: NameGetter = unsafe { core::mem::transmute::<usize, NameGetter>(getter) };
    // Safety: the getter is a plain lookup; it does not retain the pointer it returns.
    unsafe { read_wide(getter(msg, row_id)) }
}

/// The name getter for one category, or `None` for a category the game does not name this way.
///
/// [`Kind::AshOfWar`] is answered explicitly rather than out of [`SOURCES`]: it is not in that
/// table (its rows come from the gem table, see [`insert_ashes_of_war`]), but it is named by
/// `GetArtsName` keyed on a `SwordArtsParam` row, which is exactly what the export direction has
/// in hand when it asks.
fn getter_rva_for(kind: Kind) -> Option<usize> {
    if kind == Kind::AshOfWar {
        return Some(rva::GET_ARTS_NAME_RVA);
    }
    SOURCES
        .iter()
        .find(|source| source.kind == kind)
        .map(|source| source.getter_rva)
}

/// Ask the game what one row is called -- the export direction.
///
/// # Why this exists next to [`build_from_game`] rather than inverting it
///
/// The importer needs name -> id, which the game cannot answer, so it enumerates every row and
/// inverts. The exporter needs id -> name, which is the direction the game answers natively: one
/// call, no table. Building the whole catalog to read it backwards would be a few thousand calls
/// to answer a question the getter answers directly -- and it would also be wrong in one case,
/// because the inverted map is keyed by folded name and two rows can fold together.
///
/// `Kind` still matters: the same row id means different items in different tables, so the caller
/// must say which table the id came from.
///
/// # Safety
///
/// `msg` must be a live `MsgRepositoryImp*` and `module_base` the loaded image base.
pub unsafe fn name_for(kind: Kind, msg: usize, module_base: usize, row_id: u32) -> Option<String> {
    // Resolved, not added. `name_of` transmutes this into a function pointer and calls it, and its
    // safety comment says the caller guarantees the address is one of the verified getters --
    // which was not true while this was bare addition against a 1.16.2 RVA. A refusal means the
    // row goes unnamed, which is an answer this function already has.
    let getter = getter_rva_for(kind)?;
    let getter = crate::native::resolve(module_base, getter, name_getter_of(getter))?;
    // Safety: `getter` was resolved for the running build immediately above and `msg` is the
    // caller's live repository pointer.
    unsafe { name_of(getter, msg, row_id) }
}

/// The engine's own name for one name getter, for the refusal log.
///
/// Keyed on the RVA rather than on [`Kind`] because that is what the getter is -- several kinds
/// share one getter (talismans and great runes both come out of the goods bundle), and a label
/// derived from the kind would report the same missing function under three different names.
/// A name per getter rather than one shared label, because a reader who sees a refusal wants to
/// know which table stopped naming rows.
fn name_getter_of(getter_rva: usize) -> &'static str {
    match getter_rva {
        rva::GET_WEAPON_NAME_RVA => "MsgRepositoryImp::GetWeaponName",
        rva::GET_PROTECTOR_NAME_RVA => "MsgRepositoryImp::GetProtectorName",
        rva::GET_ACCESSORY_NAME_RVA => "MsgRepositoryImp::GetAccessoryName",
        rva::GET_GOODS_NAME_RVA => "MsgRepositoryImp::GetGoodsName",
        rva::GET_GEM_NAME_RVA => "MsgRepositoryImp::GetGemName",
        rva::GET_ARTS_NAME_RVA => "MsgRepositoryImp::GetArtsName",
        // Unreachable through `SOURCES` and `getter_rva_for`, which between them cover every
        // constant above. A catch-all rather than a panic: this is a log label, and taking a
        // game-loaded DLL down to complain about one is not a trade worth making.
        _ => "an unrecognised MsgRepositoryImp name getter",
    }
}

/// Insert one resolved row.
///
/// `somber` is left false on purpose, and setting it would be a regression rather than a fix.
/// It only feeds [`er_build_import_core::plan::somber_remap`], a table this repository reproduces from
/// the planner without knowing its intent, and the importer no longer needs to guess how far an
/// armament upgrades: [`ReinforceLevels`] asks the game which `reinforceTypeId + level` rows
/// exist and clamps to the highest one the build can have. Reviving the flag would put a guessed
/// level back in front of a measured one.
fn insert(
    catalog: &mut MapCatalog,
    source: &Source,
    row_id: u32,
    name: &str,
    pots: &PotGroups,
    max_held: &MaxHeld,
    quivers: &Quivers,
) {
    catalog.insert(
        source.kind,
        name,
        Entry {
            full_item_id: (source.tag as u32) | row_id,
            // Goods only, because `maxNum` is a goods field and the engine does not consult it
            // for anything else: `GetMaxAmountForItem` sends every other category to
            // `GetMaxItemQuantity`, a different function with a different answer. Leaving those
            // `None` says "not asked", which the plan reads as one -- the right number for an
            // armament or a piece of armour anyway.
            max_stored: match (source.kind, source.tag) {
                // Ammunition has its own field in its own table, and the engine reads that one
                // rather than `maxNum` for it: `GetMaxAmountForItem` sends every non-goods
                // category to `GetMaxItemQuantity`, whose weapon branch answers
                // `maxArrowQuantity` for categories 13 and 14 and a bare 1 for every other
                // armament. See [`Quivers`].
                (Kind::Ammo, _) => quivers.of(row_id),
                (_, Tag::Goods) => max_held.of(row_id),
                _ => None,
            },
            somber: false,
            // Only a goods row can be pot-capped, and `group_of` answers `None` for every id it
            // did not enumerate -- so this is safe to ask for every category rather than gated on
            // one, and a weapon or protector row that happens to share a number with a pot row
            // cannot pick up its group.
            pot_group: match source.tag {
                Tag::Goods => pots.group_of(row_id),
                _ => None,
            },
        },
    );
}

/// Row ids for one category, read from the live param tables.
///
/// Returns an empty list rather than failing when the repository is not up yet; the caller
/// sees that as zero named rows, which is the honest report for "the game is not ready".
fn row_ids(kind: Kind, spells: &BTreeSet<u32>, quivers: &Quivers) -> Vec<u32> {
    use eldenring::cs::{
        EquipParamAccessory, EquipParamGoods, EquipParamProtector, EquipParamWeapon,
        SoloParamRepository,
    };
    use fromsoftware_shared::FromStatic;

    // Safety: `instance()` returns a reference only when the singleton is populated, and the
    // rows are read, never written.
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        return Vec::new();
    };
    match kind {
        // The weapon table carries both; `weaponCategory` decides which, exactly as the goods
        // table is split by membership in `MAGIC_PARAM_ST`.
        //
        // Membership, not quantity. `contains` and not `of(..).is_none()`: an ammunition row that
        // declares no `maxArrowQuantity` is still ammunition, and asking the quantity question
        // would file it back under `Kind::Weapon` -- putting an arrow in the armament catalog,
        // where a name lookup could equip it into a hand. The two questions are separate for
        // exactly this reason, and no row in the installed table declares zero, so the difference
        // would never have shown up in a test that used real data.
        Kind::Weapon => repo
            .rows::<EquipParamWeapon>()
            .map(|(id, _)| id)
            .filter(|id| !quivers.contains(*id))
            .collect(),
        Kind::Ammo => quivers.rows(),
        Kind::Protector => repo
            .rows::<EquipParamProtector>()
            .map(|(id, _)| id)
            .collect(),
        Kind::Talisman => repo
            .rows::<EquipParamAccessory>()
            .map(|(id, _)| id)
            .collect(),
        // Built from the gem table by `insert_ashes_of_war`, which needs both ids per row.
        Kind::AshOfWar => Vec::new(),
        // The goods table carries both; membership in MAGIC_PARAM_ST decides which.
        Kind::Spell => repo
            .rows::<EquipParamGoods>()
            .map(|(id, _)| id)
            .filter(|id| spells.contains(id))
            .collect(),
        Kind::Tool => repo
            .rows::<EquipParamGoods>()
            .map(|(id, _)| id)
            .filter(|id| !spells.contains(id))
            .collect(),
        Kind::GreatRune => GREAT_RUNE_ROWS.collect(),
    }
}

/// Every row id present in `MAGIC_PARAM_ST`, i.e. every goods row that is really a spell.
fn spell_row_ids() -> BTreeSet<u32> {
    use eldenring::cs::{Magic, SoloParamRepository};
    use fromsoftware_shared::FromStatic;

    // Safety: as above -- a read-only enumeration behind a populated-singleton check.
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        return BTreeSet::new();
    };
    repo.rows::<Magic>().map(|(id, _)| id).collect()
}

/// Build the catalog from the running game.
///
/// # Safety
///
/// `msg` must be a live `MsgRepositoryImp*`, `module_base` the loaded image base, and the
/// game must be far enough along that the message repository and param tables are populated.
pub unsafe fn build_from_game(msg: usize, module_base: usize) -> (MapCatalog, BuildStats) {
    let spells = spell_row_ids();
    let pots = PotGroups::read();
    let max_held = MaxHeld::read();
    let quivers = Quivers::read();
    let mut catalog = MapCatalog::new();
    let mut stats = BuildStats {
        spell_rows: spells.len(),
        pot_capped_rows: pots.len(),
        max_held_rows: max_held.len(),
        ammo_rows: quivers.len(),
        ammo_classification_disagreements: quivers.disagreements(),
        ..BuildStats::default()
    };

    for source in SOURCES {
        // Resolved for the running build, once per table rather than once per row: the loop below
        // is thousands of calls and a refusal is a property of the build, not of a row. A table
        // whose getter has no mapping contributes no names, and every row in it lands in
        // `stats.unnamed` -- which the caller already prints beside `stats.named`, so a catalog
        // that came up empty cannot be read as a catalog that found nothing to name.
        let Some(getter) = crate::native::resolve(
            module_base,
            source.getter_rva,
            name_getter_of(source.getter_rva),
        ) else {
            stats.unnamed += row_ids(source.kind, &spells, &quivers).len();
            continue;
        };
        for row_id in row_ids(source.kind, &spells, &quivers) {
            // Safety: `getter` was resolved for the running build immediately above and `msg` is
            // the caller's live repository pointer.
            match unsafe { name_of(getter, msg, row_id) } {
                Some(name) => {
                    insert(
                        &mut catalog,
                        source,
                        row_id,
                        &name,
                        &pots,
                        &max_held,
                        &quivers,
                    );
                    stats.named += 1;
                }
                None => stats.unnamed += 1,
            }
        }
    }

    // Safety: the caller's contract carries through unchanged.
    unsafe { insert_ashes_of_war(&mut catalog, &mut stats, msg, module_base) };

    (catalog, stats)
}

/// Add every ash of war, keyed by skill name and valued by gem id.
///
/// # Why this is not just another [`Source`]
///
/// Every other category answers one question with one id: enumerate the rows of table T, ask T's
/// name getter about each row, store that row. An ash needs two tables at once. The name the
/// planner writes is the skill's (`GetArtsName`, keyed on a `SwordArtsParam` row), while the id
/// the game can act on is the gem's (`EquipParamGem`, which is what `GetGaItemHandleGem` mints and
/// what `GetSwordArtsParamId` decodes). `EquipParamGem.swordArtsParamId` is the join.
///
/// # Which gem, when several carry the same skill
///
/// The gem table holds development and placeholder rows alongside the real ashes, and more than
/// one row can name the same skill. Rather than trust `arts * 100` -- a heuristic that lands on
/// an unrelated row for some ashes (arts 309 "Thops's Barrier" -> gem 30900 is "No Skill";
/// Igon's Drake Hunt is arts 4210 but gem 548000) -- this walks the whole table and keeps, per
/// skill, the first of: a gem the game gives a display name to, else the lowest row id. Both
/// tie-breaks are order-independent, so the catalog is the same on every run.
///
/// # Safety
///
/// `msg` must be a live `MsgRepositoryImp*` and `module_base` the loaded image base, with the
/// param tables streamed (see [`params_ready`]).
unsafe fn insert_ashes_of_war(
    catalog: &mut MapCatalog,
    stats: &mut BuildStats,
    msg: usize,
    module_base: usize,
) {
    use eldenring::cs::{EquipParamGem, SoloParamRepository};
    use fromsoftware_shared::FromStatic;
    use std::collections::BTreeMap;

    // Safety: read-only enumeration behind the populated-singleton check.
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        return;
    };
    // Resolved for the running build rather than added blind: `name_of` transmutes these into
    // function pointers and calls them, and its safety comment says the caller guarantees the
    // address is one of the verified getters -- which was not true while this was bare addition.
    // On a build that moved the code, an unresolvable getter means no ash names, not a call into
    // whatever now occupies the address.
    // Through `crate::native` rather than the resolver directly, so a refusal here is named in
    // this crate's own log exactly once for the life of the process, like every other native it
    // asks for -- and under the same name the two loops above would report.
    let Ok([arts_getter, gem_getter]) = crate::native::resolve_all(
        module_base,
        [
            (
                rva::GET_ARTS_NAME_RVA,
                name_getter_of(rva::GET_ARTS_NAME_RVA),
            ),
            (rva::GET_GEM_NAME_RVA, name_getter_of(rva::GET_GEM_NAME_RVA)),
        ],
    ) else {
        return;
    };

    // skill row -> (this gem is the canonical `arts * 100` row, it draws an icon, it has an
    // item name, gem row).
    //
    // The icon is the first key, and it is why this is not just "named, else lowest row".
    // Measured 2026-08-23: a build imported with every ash name correct and the ash badge on the
    // tile rendering the literal placeholder text `ICON`. The names were right because the label
    // comes from `EquipParamGem.swordArtsParamId` -> `GetArtsName`, which a development row
    // carries just as faithfully as a real one; the icon was missing because that row has none.
    // The old tiebreak fell back to the lowest row ID whenever no gem for a skill was named, and
    // the lowest rows are exactly the placeholder rows -- which is how ids like 103, 146, 185 and
    // 191 ended up mounted on the player's weapons next to real ashes at 401000 and 22800.
    //
    // `0` and `u16::MAX` are both treated as "no icon": the field is unsigned, so a row that
    // means "none" can only say so with one of the two ends, and neither names a real texture.
    let mut best: BTreeMap<u32, (bool, bool, bool, u32)> = BTreeMap::new();
    let mut iconless = 0usize;
    // arts row -> (how many gem rows carry it, the icon id of the one that won). Only consulted
    // for the ashes that end up iconless, but it has to be gathered during the walk.
    let mut gem_shape: BTreeMap<u32, (usize, u16)> = BTreeMap::new();
    for (gem_id, row) in repo.rows::<EquipParamGem>() {
        let Ok(arts_id) = u32::try_from(row.sword_arts_param_id()) else {
            continue;
        };
        // Arts 0 is `No Skill`, and it is a real Ash of War the player can buy and mount -- it
        // is how a weapon is deliberately stripped of its innate skill. Skipping it here left it
        // out of the catalog entirely, which is why `plan.rs` had to special-case the string and
        // treat "No Skill" as "mount nothing". Those are opposite outcomes: mounting nothing
        // leaves the innate skill in place. Measured 2026-09-10 on a Serpent Crest Shield the
        // build asked to carry `No Skill`, which came out of the import reporting `arts 10`.
        //
        // It goes through the same canonical/icon/named tiebreak as every other skill rather
        // than being hard-coded to a row, because the rule that picks the purchasable item over
        // a development placeholder is the same rule here as anywhere.
        _ = arts_id;
        // Safety: a verified getter RVA and the caller's live repository pointer.
        let named = unsafe { name_of(gem_getter, msg, gem_id) }.is_some();
        let icon = row.icon_id();
        let has_icon = icon != 0 && icon != u16::MAX;
        // The canonical row wins outright. `gem == arts * 100` is where the purchasable "Ash of
        // War: <skill>" item lives for the overwhelming majority of skills, and it is only
        // reached here after the walk has already confirmed this row carries this arts id -- so
        // the heuristic the module header warns about (arts 309 -> gem 30900 is "No Skill",
        // Igon's Drake Hunt is arts 4210 but gem 548000) cannot fire: a row that does not carry
        // the skill never becomes a candidate for it, and those skills fall through to the keys
        // below exactly as before.
        //
        // Without this, "lowest row id" decides -- and it is wrong, because a block of low
        // four-digit rows around 1000-1020 carries the same skills as the real ash items and is
        // named and iconned enough to beat nothing. Measured 2026-08-23: Flaming Strike (arts 214)
        // resolved to gem 1010 instead of 21400 and the tile drew the `ICON` placeholder, with the
        // ash name still correct because that comes from `swordArtsParamId`. Broadsword took 1002
        // over 10300, Star Fist 1013 over 50200, Rusted Anchor 1010 over 21400.
        let canonical = gem_id == arts_id.saturating_mul(100);
        let candidate = (canonical, has_icon, named, gem_id);
        gem_shape
            .entry(arts_id)
            .and_modify(|seen| seen.0 += 1)
            .or_insert((1, icon));
        best.entry(arts_id)
            .and_modify(|held| {
                // The canonical row beats everything; then an icon beats no icon; then a
                // named gem beats an unnamed one; among equals the lower row id wins.
                if (canonical, has_icon, named, core::cmp::Reverse(gem_id))
                    > (held.0, held.1, held.2, core::cmp::Reverse(held.3))
                {
                    *held = candidate;
                }
            })
            .or_insert(candidate);
    }

    for (arts_id, (_, has_icon, _, gem_id)) in best {
        // Counted, not silently accepted: an ash whose only gem row draws no icon will still
        // render `ICON` on the tile, and that has to be a number in the log rather than a
        // surprise on the player's weapon.
        if !has_icon {
            iconless += 1;
            // Named, not just counted. "5 ashes have no icon" cannot be acted on; "Flaming Strike
            // has no icon, its winning gem is row N with iconId 0, and it has M gem rows" says
            // straight away whether the row is a genuine placeholder or whether `0` is a real
            // icon id this filter is wrongly rejecting.
            let (rows, icon) = gem_shape.get(&arts_id).copied().unwrap_or((0, 0));
            // Safety: a verified getter RVA and the caller's live repository pointer.
            let label = unsafe { name_of(arts_getter, msg, arts_id) }
                .unwrap_or_else(|| format!("<unnamed arts {arts_id}>"));
            crate::log_line(&format!(
                "[build-import]   ICONLESS ASH {label:?} arts {arts_id} -> gem {gem_id} \
                 (iconId {icon}, {rows} gem row(s) carry this skill)"
            ));
        }
        // Safety: as above.
        let named = unsafe { name_of(arts_getter, msg, arts_id) };
        // `SwordArtsParam` row 0 is `No Skill`, and the message repository has no name for it --
        // it is the absence of a skill, so nothing in the arts bundle describes it. Without a
        // name it never entered the catalog, and the name `No Skill` was then claimed by gem
        // 30900, a placeholder row carrying arts 309 that the repository does name that way. A
        // build asking for `No Skill` got that row mounted instead, and the read-back on a
        // Serpent Crest Shield came back `arts 10` -- its own skill, untouched.
        //
        // The literal is the planner's own vocabulary: the payload writes `weaponArt: "No Skill"`
        // and `plan.rs` compared against exactly this string for as long as it special-cased it.
        // Row 0 is reached first (`best` is keyed by arts id and iterated ascending) and
        // `MapCatalog::insert` is first-wins, so gem 30900 becomes an alternate rather than
        // taking the name.
        let named = match (arts_id, named) {
            (0, None) => Some("No Skill".to_owned()),
            (_, named) => named,
        };
        if arts_id == 0 {
            crate::log_line(&format!(
                "[build-import]   NO SKILL ASH: arts 0 -> gem {gem_id}, catalogued as {:?}",
                named.as_deref().unwrap_or("<nothing>")
            ));
        }
        match named {
            Some(name) => {
                catalog.insert(
                    Kind::AshOfWar,
                    &name,
                    Entry {
                        full_item_id: (Tag::Gem as u32) | gem_id,
                        // An ash is an `EquipParamGem` row, not a goods row: neither `maxNum` nor
                        // `potGroupId` exists on that table to read. Nothing grants an ash as an
                        // item anyway -- it rides onto its armament as `Grant::weapon_skill`.
                        max_stored: None,
                        somber: false,
                        pot_group: None,
                    },
                );
                stats.named += 1;
            }
            None => stats.unnamed += 1,
        }
    }
    stats.iconless_ashes = iconless;
}

/// Every goods row the game POT-caps, and which group it is capped against.
///
/// # What a pot group is, and why an importer has to know
///
/// `EquipParamGoods.potGroupId` (壺グループID, `s8`, `-1..15`) ties a crafted consumable to the
/// vessel it needs. `EquipInventoryData::UpdatePotsStates` (1.16.2 `0x14024e930`, same address on
/// 1.17) walks the carried inventory once and builds two `int[16]` tables on the inventory
/// itself: `potItemsCapacity` at `+0xc8`, summed from the group's regenerative materials (the
/// Cracked Pots), and `potItemsCount` at `+0x88`, summed from its CONSUMABLES (the pots you
/// throw). `GetMaxAmountForItem` (`0x14024e570`) then answers `capacity[g] - count[g]`.
///
/// Both ways of acquiring an item clamp to that answer and say nothing:
/// `EquipInventoryData::InsertItem` (`0x14024cfd0`) and `UpdateQuantity` (`0x14024d760`) each do
/// `if (max < amount) amount = max;`. So "grant five Fire Pots" against a full group delivers
/// three, returns no error, and leaves `EquipGameData.lastItemAddResult` at zero. Crafting does
/// not escape it either -- every acquisition funnels through those two functions -- which is why
/// the grant path frees space in the group instead of trying to make more pots.
///
/// # Consumables only
///
/// The predicate is the engine's own `EquipParamGoodsLookupResult::IsPotConsumable`
/// (1.16.2 `0x140d3a190`, 1.17 `0x140d3b8e0`; the two are byte-for-byte identical, which is how
/// the field offsets below were confirmed to survive the patch):
///
/// ```text
/// CMP byte ptr [RDX + 0x3e], 0x0   ; goodsType == NORMAL_ITEM
/// MOVZX ECX, byte ptr [RDX + 0x2e] ; potGroupId
/// TEST CL, CL / JS                 ; ... >= 0
/// ```
///
/// The Cracked Pot itself is `goodsType == 0x0b` (REGENERATIVE_MATERIAL, per
/// `IsRegenerativeMaterial` at `0x140d3a1c0` / `0x140d3b910`) and is deliberately not in here.
/// It is what supplies the capacity, so a caller that deposited it to free space would be
/// removing the space. Measured against the installed regulation by
/// `scripts/regulation-potgroup-census.py`: 67 consumable rows across 4 groups, each group with
/// exactly one material (9500, 9510, 9501, 2009500).
///
/// # Read from the game, not from a list
///
/// The rows are enumerated live rather than hard-coded, for the same reason the name catalog is:
/// the table is the player's patch level with their DLC, and a shipped list of pot ids would be
/// a copy of it that goes stale silently.
pub struct PotGroups {
    /// Goods row id -> group, for pot consumables only.
    by_row: std::collections::BTreeMap<u32, u8>,
    /// Group -> every pot-consumable item id (goods-tagged) in it.
    members: [Vec<u32>; POT_GROUP_COUNT],
}

/// `EquipInventoryData::potItemsCount` / `potItemsCapacity` are `int[16]`, and `UpdatePotsStates`
/// discards any group id at or above that -- `if (uVar5 < 0x10)` guards both accumulations.
const POT_GROUP_COUNT: usize = 16;

/// `EquipParamGoods.goodsType` value for an ordinary consumable, the one half of
/// `IsPotConsumable`'s test that is not the group id.
const GOODS_TYPE_NORMAL_ITEM: u8 = 0x00;

impl PotGroups {
    /// Read the goods table. Empty when the repository is not up, which makes every pot-aware
    /// path inert rather than wrong -- the grant still runs, it just cannot free space first.
    pub fn read() -> Self {
        use eldenring::cs::{EquipParamGoods, SoloParamRepository};
        use fromsoftware_shared::FromStatic;

        let mut by_row = std::collections::BTreeMap::new();
        let mut members: [Vec<u32>; POT_GROUP_COUNT] = Default::default();
        // Safety: read-only enumeration behind the populated-singleton check.
        let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
            return Self { by_row, members };
        };
        for (row_id, row) in repo.rows::<EquipParamGoods>() {
            if row.goods_type() != GOODS_TYPE_NORMAL_ITEM {
                continue;
            }
            let group = row.pot_group_id();
            if group < 0 || usize::from(group.unsigned_abs()) >= POT_GROUP_COUNT {
                continue;
            }
            let group = group.unsigned_abs();
            by_row.insert(row_id, group);
            members[usize::from(group)].push((Tag::Goods as u32) | row_id);
        }
        Self { by_row, members }
    }

    /// The group a goods row id is capped against, or `None` when the game does not cap it.
    pub fn group_of(&self, row_id: u32) -> Option<u8> {
        self.by_row.get(&row_id).copied()
    }

    /// Every pot-consumable item id in `group`, goods category nibble included.
    ///
    /// These are the only items whose deposit raises the ceiling for another member of the group.
    pub fn members(&self, group: u8) -> &[u32] {
        self.members
            .get(usize::from(group))
            .map_or(&[], Vec::as_slice)
    }

    /// How many rows the game pot-caps at all. Zero means this table could not be read.
    pub fn len(&self) -> usize {
        self.by_row.len()
    }

    /// Whether nothing was read.
    pub fn is_empty(&self) -> bool {
        self.by_row.is_empty()
    }
}

/// Which `EquipParamWeapon` rows are ammunition, and how many of each the game lets a player hold.
///
/// # Ammunition is a weapon, and `maxNum` says nothing about it
///
/// Arrows and bolts are `EquipParamWeapon` rows -- which is why they equip into dedicated
/// `ChrAsmSlot` positions rather than the quickbar -- so [`MaxHeld`], which reads
/// `EquipParamGoods.maxNum`, cannot answer for them and the engine never asks it to.
/// `CS::EquipInventoryData::GetMaxAmountForItem` (1.16.2 and 1.17 both `0x14024e570`) handles the
/// goods category inline and tail-jumps every other category to `::GetMaxItemQuantity`
/// (1.16.2 `0x140674680`, 1.17 `0x1406754d0`), whose weapon branch is the whole of the answer:
///
/// ```text
/// EquipParamWeapon::GetEntry(&lookup, itemId & 0x0FFFFFFF);
/// if (lookup.row && (row->weaponCategory == 13 || row->weaponCategory == 14))
///     return row->maxArrowQuantity;
/// return 1;                                   // every other weapon: one armament
/// ```
///
/// So the same two fields answer both questions this type exists for -- "is this row ammunition"
/// and "how many of it may be held" -- and they answer them the way the engine does rather than
/// the way a classification of our own would.
///
/// # The two offsets, confirmed on both images
///
/// A wrong struct offset is the one failure mode with no symptom: no refusal, no fault, no log
/// line, just a number read from the wrong place. Both are therefore established twice over and
/// then pinned to the compiler:
///
/// * The 1.16.2 dump's own `_EQUIP_PARAM_WEAPON_ST` (struct size 664) names `weaponCategory` at
///   offset 230 (`0xE6`, `u8`) and `maxArrowQuantity` at 565 (`0x235`, `u8`).
/// * 1.17 confirms them by reading the function that consumes them. The instruction bytes are
///   identical between builds --
///   `0f b6 91 e6 00 00 00 / 80 fa 0d / 74 09 / 80 fa 0e / 0f 85 .. / 0f b6 81 35 02 00 00` --
///   at `0x140674887` in `eldenring-deobf.bin` and `0x1406756d7` in `eldenring-deobf-1.17.bin`.
/// * The installed 1.17 `regulation.bin` corroborates by value
///   (`scripts/regulation-ammo-census.py`): it derives the same 664-byte stride, finds 73 rows in
///   categories 13 and 14, and reads `maxArrowQuantity` `{1: 2, 20: 5, 30: 8, 99: 58}` -- the 20s
///   the five Ballista Bolts, the 30s the eight Great Arrows, the 99s every ordinary arrow and
///   bolt. Those are the quiver limits the game enforces.
///
/// The `const` assertions below are the part that cannot rot: they ask the compiler where the
/// fields are, so a layout change in `../fromsoftware-rs` becomes a build error rather than a
/// wrong read at runtime.
///
/// # Two classifications of ammunition, measured to agree
///
/// [`crate::read_character`] classifies ammunition by `wepType` (81 Arrow, 83 Great Arrow,
/// 85 Bolt, 86 Ballista Bolt) because that is what the export side has in hand. This side uses
/// `weaponCategory` because that is the field the engine's own quantity gate reads. The two select
/// the identical 73 rows of the installed table, with zero rows on either side of the difference,
/// and `scripts/regulation-ammo-census.py` exits non-zero if they ever stop agreeing -- so the
/// duplication is checked rather than merely asserted.
pub struct Quivers {
    /// Ammunition row id -> `maxArrowQuantity`, whatever it declares.
    by_row: std::collections::BTreeMap<u32, u32>,
    /// Rows the `weaponCategory` and `wepType` classifications disagreed about. See
    /// [`Quivers::disagreements`].
    disagreements: usize,
}

impl Quivers {
    /// `EquipParamWeapon.weaponCategory` for arrows.
    const CATEGORY_ARROW: u8 = 13;
    /// `EquipParamWeapon.weaponCategory` for bolts.
    const CATEGORY_BOLT: u8 = 14;

    /// Read the weapon table. Empty when the repository is not up -- which leaves the ammunition
    /// catalog empty and every arrow in the build unresolved, rather than silently reclassifying
    /// arrows as armaments and equipping them into a hand.
    pub fn read() -> Self {
        use eldenring::cs::{EquipParamWeapon, SoloParamRepository};
        use fromsoftware_shared::FromStatic;

        // What the compiler can be asked, and what it cannot. `EQUIP_PARAM_WEAPON_ST`'s fields
        // are private in `../fromsoftware-rs`, so `offset_of!` cannot name them and the
        // field-level pin used for `PlayerGameData` is simply not available here. Its size is,
        // and 664 is verified twice over -- the 1.16.2 dump's struct is 664 bytes, and the
        // installed 1.17 `regulation.bin` derives a 664-byte stride from the gaps between its own
        // row offsets. That is a bracket, not a proof: a compensating insert-and-remove inside
        // the struct leaves the size unmoved. It is pinned because it is measured, and the
        // disagreement count below is what covers what it cannot.
        const _: () =
            assert!(core::mem::size_of::<eldenring::param::EQUIP_PARAM_WEAPON_ST>() == 664);

        let mut by_row = std::collections::BTreeMap::new();
        let mut disagreements = 0usize;
        // Safety: read-only enumeration behind the populated-singleton check.
        let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
            return Self {
                by_row,
                disagreements,
            };
        };
        for (row_id, row) in repo.rows::<EquipParamWeapon>() {
            let category = row.weapon_category();
            let by_category = category == Self::CATEGORY_ARROW || category == Self::CATEGORY_BOLT;
            // The cross-check that stands in for the pin the compiler would not take. `wepType`
            // is a second field, at a different offset (`+0x1A6`, `u16`), whose ammunition values
            // the exporter measured independently -- and on the installed table the two select
            // the identical 73 rows. A layout shift that moved one of them would move it out from
            // under only one of these two reads, so a non-zero count here is a wrong-offset alarm
            // for a failure that is otherwise perfectly silent.
            if by_category != crate::read_character::is_ammunition(row.wep_type()) {
                disagreements += 1;
            }
            if !by_category {
                continue;
            }
            // A row in an ammunition category with no declared quantity is still ammunition, so
            // it belongs in the row set either way; it is the quantity that is absent, and the
            // plan reads that as one. Recorded as such rather than dropped, because dropping it
            // would send an arrow back into `Kind::Weapon` and into a hand slot.
            by_row.insert(row_id, u32::from(row.max_arrow_quantity()));
        }
        Self {
            by_row,
            disagreements,
        }
    }

    /// Rows the two ammunition classifications disagreed about.
    ///
    /// Zero on every build this has been measured against. Non-zero means `weaponCategory` and
    /// `wepType` no longer describe the same set, which in practice means one of the two offsets
    /// is being read out of the wrong place -- the one defect in this module that produces no
    /// fault, no refusal and no log line of its own.
    pub fn disagreements(&self) -> usize {
        self.disagreements
    }

    /// Every ammunition row id.
    pub fn rows(&self) -> Vec<u32> {
        self.by_row.keys().copied().collect()
    }

    /// The `maxArrowQuantity` an ammunition row declares, or `None` for a row that is not
    /// ammunition at all.
    ///
    /// A declared zero comes back as `None` for the same reason [`MaxHeld`] leaves a
    /// `maxNum <= 0` row absent: the plan reads absence as one copy, and one is the right answer
    /// for an item the table has no opinion about.
    pub fn of(&self, row_id: u32) -> Option<u32> {
        self.by_row.get(&row_id).copied().filter(|max| *max > 0)
    }

    /// Whether `row_id` is an ammunition row, whatever quantity it declares.
    pub fn contains(&self, row_id: u32) -> bool {
        self.by_row.contains_key(&row_id)
    }

    /// How many ammunition rows were found. Zero means this table could not be read.
    pub fn len(&self) -> usize {
        self.by_row.len()
    }

    /// Whether nothing was read.
    pub fn is_empty(&self) -> bool {
        self.by_row.is_empty()
    }
}

/// How many of each goods row the game lets a player hold: `EquipParamGoods.maxNum`.
///
/// # The engine's own answer, not a stack size chosen here
///
/// `CS::EquipInventoryData::GetMaxAmountForItem` (1.16.2 `0x14024e570`, same address on 1.17) is
/// the whole of the engine's opinion on "how many more of this may be added". For an item id in
/// the goods category its body is:
///
/// ```text
/// GetEntry(&lookup, itemId & 0x0FFFFFFF);
/// if (GetPotGroupId(&lookup) >= 0)  return limitedPots ? capacity[g] - count[g] : 99;
/// if (lookup.paramRow && 0 < lookup.paramRow->maxNum) return lookup.paramRow->maxNum;
/// return 99;
/// ```
///
/// So `maxNum` is the ceiling the add will be clamped to anyway, and the pot branch above it is
/// the reason [`PotGroups`] exists next to this.
///
/// # The offset, on both images
///
/// `maxNum` is `+0x3A` in a 176-byte `EQUIP_PARAM_GOODS_ST`, between the `potGroupId` (`+0x2E`)
/// and `goodsType` (`+0x3E`) that [`PotGroups`] already pins. That is what the 1.16.2 dump's own
/// `_EQUIP_PARAM_GOODS_ST` says (field 21, offset 58, struct size 176), and it holds on 1.17:
/// the installed 1.17 `regulation.bin` derives the same 176-byte stride, and the `s16` at `+0x3A`
/// reads 1 for every great rune, 1 for every crystal tear, 20 for the Flask of Cerulean Tears, 99
/// for both boluses and 10 for every pot-group consumable -- the caps the game actually enforces,
/// and the same numbers the planner's own item database records. A wrong offset here would be
/// silent, which is why it is corroborated by value and not only by neighbour.
///
/// # Read from the game, not from a list
///
/// Same reason as the name catalog and the pot groups: the table is the player's patch level with
/// their DLC, and the actual read goes through `EquipParamGoods::max_num()`, so the offset above
/// is documentation of what that accessor owns rather than a second copy of it.
pub struct MaxHeld {
    /// Goods row id -> `maxNum`, for rows that declare a positive one.
    by_row: std::collections::BTreeMap<u32, u32>,
}

impl MaxHeld {
    /// Read the goods table. Empty when the repository is not up, which makes every consumable
    /// fall back to a single copy rather than to a wrong number.
    pub fn read() -> Self {
        use eldenring::cs::{EquipParamGoods, SoloParamRepository};
        use fromsoftware_shared::FromStatic;

        let mut by_row = std::collections::BTreeMap::new();
        // Safety: read-only enumeration behind the populated-singleton check.
        let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
            return Self { by_row };
        };
        for (row_id, row) in repo.rows::<EquipParamGoods>() {
            let max_num = row.max_num();
            // `0 < maxNum` is the engine'S own test, kept rather than paraphrased. A row that
            // declares nothing is left absent, so the plan grants one -- the engine would hand
            // out 99 for such a row, and handing a player 99 of something the game itself has no
            // opinion about is the over-grant this whole path is trying not to be. Two rows in
            // the installed 1.17 table are in that state.
            if max_num > 0 {
                by_row.insert(row_id, max_num.unsigned_abs().into());
            }
        }
        Self { by_row }
    }

    /// The `maxNum` a goods row declares, or `None` when it declares none.
    pub fn of(&self, row_id: u32) -> Option<u32> {
        self.by_row.get(&row_id).copied()
    }

    /// How many rows declared one. Zero means this table could not be read.
    pub fn len(&self) -> usize {
        self.by_row.len()
    }

    /// Whether nothing was read.
    pub fn is_empty(&self) -> bool {
        self.by_row.is_empty()
    }
}

/// Whether a param table has been streamed in.
///
/// `rows()` cannot answer this: `SoloParamRepository::get_param_file` does
/// `holder.get_res_cap(0).expect(...)`, so asking a not-yet-loaded table for its rows
/// PANICS rather than yielding nothing. That is what killed the first two catalog runs.
/// The res cap itself is an `Option`, so checking it is the non-destructive question.
fn holder_ready<P: eldenring::cs::SoloParam>(repo: &eldenring::cs::SoloParamRepository) -> bool {
    repo.solo_param_holders
        .get(P::INDEX as usize)
        .and_then(|holder| holder.get_res_cap(0))
        .is_some()
}

/// Whether every table the catalog reads is loaded.
///
/// All of them, not just one: a partially-streamed repository would let the first category
/// through and then panic on a later one, which is the same failure with a longer fuse.
pub fn params_ready() -> bool {
    use eldenring::cs::{
        EquipParamAccessory, EquipParamGem, EquipParamGoods, EquipParamProtector, EquipParamWeapon,
        Magic, ReinforceParamWeapon, SoloParamRepository, SwordArtsParam,
    };
    use fromsoftware_shared::FromStatic;

    // Safety: read-only field access behind the populated-singleton check.
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        return false;
    };
    holder_ready::<EquipParamWeapon>(repo)
        && holder_ready::<EquipParamProtector>(repo)
        && holder_ready::<EquipParamAccessory>(repo)
        && holder_ready::<EquipParamGoods>(repo)
        && holder_ready::<SwordArtsParam>(repo)
        // The ash catalog joins the gem table to the arts table; asking either one for rows
        // before it is streamed PANICS rather than yielding nothing. `ReinforceLevels` reads the
        // third table for the same reason, and has the same failure mode.
        && holder_ready::<EquipParamGem>(repo)
        && holder_ready::<ReinforceParamWeapon>(repo)
        && holder_ready::<Magic>(repo)
}

/// The live `CS::MsgRepositoryImp*`, or `None` before it exists.
///
/// Read out of the game's own global rather than through a typed upstream singleton. There is a
/// `MsgRepositoryImp` in `fromsoftware-rs` -- but only in a local fork, not at the revision CI
/// pins, so importing it compiles on one machine and fails everywhere else. The address lives in
/// `er-game-base::rva` beside the other cross-crate ones.
///
/// A zero is "not constructed yet", not an error: the whole point of this being an `Option` is
/// that the caller re-asks next frame instead of dereferencing a null the engine would DLPanic on.
pub fn msg_repository() -> Option<usize> {
    let base = er_game_base::mem::game_module_base().ok()?;
    // Safety: a fault-checked read of one pointer-sized slot in the loaded image.
    let repository = unsafe {
        er_game_base::mem::safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            er_game_base::rva::MSG_REPOSITORY_GLOBAL_RVA,
            "MSG_REPOSITORY_GLOBAL_RVA",
        ))
    }?;
    (repository != 0).then_some(repository)
}

/// The `SwordArtsParam` row an `EquipParamGem` row carries, straight off the live table.
///
/// The importer needs this to check its own work: the build names a gem, the worn weapon reports
/// an arts row, and only `EquipParamGem.swordArtsParamId` says whether those are the same ash.
/// Comparing display names instead would turn a wrong-row bug into a string-matching bug.
pub fn arts_row_for_gem(gem_row: u32) -> Option<u32> {
    use eldenring::cs::{EquipParamGem, SoloParamRepository};
    use fromsoftware_shared::FromStatic;

    // Safety: read-only row access behind the populated-singleton check.
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        return None;
    };
    repo.rows::<EquipParamGem>()
        .find(|(id, _)| *id == gem_row)
        .and_then(|(_, row)| u32::try_from(row.sword_arts_param_id()).ok())
}

/// Which upgrade levels the game actually has rows for, so a requested level cannot invent one.
///
/// # Why a level has to be clamped at all
///
/// `EquipParamWeapon::GetEntry(paramId)` splits its argument: the armament row is
/// `(paramId / 100) * 100`, and the level is `paramId % 100`, which it turns into
/// `ReinforceParamWeapon::GetEntry(row.reinforceTypeId + level)`. Somber armaments stop at +10,
/// so a level of 25 asks for a reinforce row that does not exist and the lookup comes back with a
/// NULL row for everything downstream to read.
///
/// The importer cannot tell somber from standard on its own: the build's `weaponUpgrade` is one
/// number for the whole character, the per-slot override is usually absent, and the planner's
/// somber remap is a table this repository reproduces without knowing its intent. So it does not
/// guess -- it asks which `reinforceTypeId + level` rows exist and takes the highest one at or
/// below what the build wanted.
pub struct ReinforceLevels {
    /// Every `ReinforceParamWeapon` row id present in the live table.
    rows: std::collections::BTreeSet<u32>,
}

impl ReinforceLevels {
    /// Read the table. Empty when the repository is not up, which clamps everything to +0 rather
    /// than writing a level nothing can back.
    pub fn read() -> Self {
        use eldenring::cs::{ReinforceParamWeapon, SoloParamRepository};
        use fromsoftware_shared::FromStatic;

        // Safety: read-only enumeration behind the populated-singleton check.
        let rows = match unsafe { SoloParamRepository::instance() } {
            Ok(repo) => repo
                .rows::<ReinforceParamWeapon>()
                .map(|(id, _)| id)
                .collect(),
            Err(_) => std::collections::BTreeSet::new(),
        };
        Self { rows }
    }

    /// The game level to store for an armament the build wants at `requested`.
    ///
    /// Two numbers reach this function on different scales, and `is_character_default` says which
    /// one this is (see `er_build_import_core::plan::Grant::upgrade_is_character_default`):
    ///
    /// * a per-slot `upgrade` is already the game's level, so it is only clamped;
    /// * the character-wide `weaponUpgrade` is in regular smithing-stone levels for every
    ///   armament, so for a somber one it is mapped down first -- `weaponUpgrade: 17` means the
    ///   game's +7 there, and clamping 17 instead would silently hand over a maxed +10.
    ///
    /// Somber is measured rather than flagged: an armament whose highest existing
    /// `reinforceTypeId + level` row is [`er_build_import_core::plan::MAX_SOMBER_LEVEL`] is one,
    /// which is the same question [`Self::clamp`] already answers.
    pub fn game_level_for(
        &self,
        weapon_param_id: u32,
        requested: u16,
        is_character_default: bool,
    ) -> u16 {
        use er_build_import_core::plan::{
            MAX_REGULAR_LEVEL, MAX_SOMBER_LEVEL, somber_level_for_regular,
        };

        if !is_character_default {
            return self.clamp(weapon_param_id, requested);
        }
        let max = self.clamp(weapon_param_id, MAX_REGULAR_LEVEL);
        let wanted = if max == MAX_SOMBER_LEVEL {
            somber_level_for_regular(requested)
        } else {
            requested
        };
        self.clamp(weapon_param_id, wanted)
    }

    /// The highest level at or below `requested` that `weapon_param_id` actually has a row for.
    ///
    /// Returns 0 when the armament row cannot be read, which is the level the game would have
    /// stored anyway before this code existed.
    pub fn clamp(&self, weapon_param_id: u32, requested: u16) -> u16 {
        use eldenring::cs::{EquipParamWeapon, SoloParamRepository};
        use fromsoftware_shared::FromStatic;

        if self.rows.is_empty() {
            return 0;
        }
        // Safety: read-only row access behind the populated-singleton check.
        let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
            return 0;
        };
        // The armament's own row is the +0 row of its affinity, which is what the plan builds.
        let Some(reinforce_type) = repo
            .rows::<EquipParamWeapon>()
            .find(|(id, _)| *id == weapon_param_id)
            .map(|(_, row)| i64::from(row.reinforce_type_id()))
        else {
            return 0;
        };
        (0..=requested)
            .rev()
            .find(|level| {
                u32::try_from(reinforce_type + i64::from(*level))
                    .is_ok_and(|id| self.rows.contains(&id))
            })
            .unwrap_or(0)
    }
}
