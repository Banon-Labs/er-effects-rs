//! Sending the gear the build does not name back to the storage box.
//!
//! # The complaint this answers
//!
//! Reported 2026-09-10, twice: "loading a build from URL doesn't appear to send the previous
//! inventory back into storage -- at least not the weapons, or armor, talismans". It did not, and
//! that was a decision rather than an oversight: [`crate::reorder`] makes the build's items sort
//! above everything else, so nothing gets in the way of the order, and evicting a player's
//! belongings to tidy a list is a larger change to the character than the import itself.
//!
//! That reasoning holds for crafting materials and consumables. It does not hold for the three
//! categories the report names. A build is a statement about what the character wears and carries
//! to fight with, and a hundred armaments the build never mentions are not context -- they are the
//! previous build, still in the pockets. So those three categories are swept and nothing else is.
//!
//! # What is never touched
//!
//! Only the weapon, protector and accessory categories, decided by the item id's top nibble.
//! Goods keep their category `0x4`, so every consumable, crafting material, key item, spell and
//! remembrance is outside this pass by construction rather than by an exception list.
//!
//! # When the box is full, a third copy is destroyed
//!
//! An armament does not always have somewhere else to go. Measured on this character 2026-09-10:
//! the storage box was at `1920 of 1920` entries, so 68 pieces of gear were refused for no reason
//! to do with the gear at all, and a Frenzied Flame Seal and three shields stayed in the pockets.
//!
//! So, by user directive the same day: when the box will not take an entry **and** the character
//! would still own [`REDUNDANT_COPIES`] or more of the same item without it, the carried copy is
//! destroyed instead. That count is the shelf's holding kept live as this pass deposits into it,
//! plus the copies this pass keeps in the inventory -- see [`is_redundant`] for the two measured
//! cases that each half exists for. Same item means same item *by name* -- [`shelf_identity`] strips the affinity and the
//! upgrade level off an armament id, so an Occult Longsword +25 counts against two plain
//! Longswords on the shelf.
//!
//! The Ash of War comes off first, through the engine's own remove action
//! ([`er_game_base::rva::REMOVE_GEM_FROM_WEAPON_RVA`]), because an ash is an item consumed into
//! one specific instance and destroying the weapon with it mounted destroys the ash too. It is
//! returned to the inventory, and the entry count rising by one is what proves it.
//!
//! Nothing else here destroys anything. A refusal the box makes about the *item* -- it will not
//! take that kind at all -- leaves it exactly where it was, and so does an entry still on the
//! character.
//!
//! # The trap that would have deposited the build's own weapons
//!
//! An armament's upgrade level lives in the last two digits of its item id, so the id the plan
//! names (+0) and the id the character carries (+25) are different numbers. A keep-set built from
//! the plan alone therefore does not recognise the very armaments the grant just minted, and the
//! sweep would put them straight in the box. The keep-set is built from both: the planned ids and
//! the ids the grant reports actually landing, which is what [`GrantOutcome::armaments`] carries.
//!
//! [`GrantOutcome::armaments`]: crate::grant::GrantOutcome::armaments

use std::collections::{BTreeMap, BTreeSet};

use er_build_import_core::catalog::Kind;
use er_build_import_core::plan::{Grant, split_armament_id};

use crate::grant::ArmamentOutcome;

use crate::equip_native::SlotClearer;
use crate::storage::{InventoryEntry, Storage};

/// The three item categories this pass may move, as the top nibble of a category-tagged item id.
///
/// `0x0` weapons (ammunition included -- an arrow is an `EquipParamWeapon` row), `0x1` protectors,
/// `0x2` accessories. Goods are `0x4` and gems `0x8`, so both are excluded by not being listed.
const SWEPT_CATEGORIES: [u32; 3] = [0x0000_0000, 0x1000_0000, 0x2000_0000];
const ITEM_CATEGORY_MASK: u32 = 0xF000_0000;

/// The engine's own "this slot is empty" placeholders, which are gear-shaped and are not gear.
///
/// Clearing a hand does not leave it holding nothing -- `FUN_140247160` puts the Unarmed fist in
/// it -- and clearing a piece of armour puts that slot's empty-piece row in it. Both are ordinary
/// inventory entries in the weapon and protector categories, so a sweep that only looks at the
/// category nibble finds them, tries to deposit them, and is refused because they are worn.
///
/// Measured on the first live run of this pass, 2026-09-10: six of sixteen refusals were these,
/// five of them the fist alone. They are noise the vacate pass creates immediately before this
/// one runs, and reporting them as gear that could not be evicted buries the real refusals.
///
/// `0x1ADB0` is `GetDefaultUnarmedParamId`'s constant, category 0 so the tagged id is the param
/// row itself. The four protectors are `GetDefaultItemIdForEmptyProtectorSlot`'s constants,
/// already category-tagged as the game returns them.
const EMPTY_SLOT_PLACEHOLDERS: [u32; 5] = [
    0x0001_ADB0,
    0x1000_2710,
    0x1000_2774,
    0x1000_27D8,
    0x1000_283C,
];

/// How many of an item the box must already hold before a copy the box will not take is
/// destroyed instead of carried.
///
/// Two, by user directive 2026-09-10. The player keeps a pair on the shelf; a third copy that the
/// box has no room for is redundant, and carrying it defeats the whole point of the sweep. One
/// would be too thin -- an item held once is the only one there is.
const REDUNDANT_COPIES: i64 = 2;

/// Whether a copy the box will not take can be destroyed, given how many the character keeps.
///
/// `owned_elsewhere` is the shelf's holding plus what this pass keeps in the inventory, because
/// the question is what the character still owns once this copy is gone -- and a copy on the
/// character counts exactly as much as one on the shelf. Both halves were learned from a single
/// measurement on 2026-09-10, and each spared an item the player wanted gone:
///
/// * the shelf count was a snapshot taken before the pass, so a character carrying two Spiralhorn
///   Shields deposited the first into the last free slot and then spared the second against a
///   count of one, while the box already held two;
/// * a Serpent Crest Shield in Magic that the build kept sat beside its Standard twin with one on
///   the shelf, and the twin survived on a count of one rather than the two it really came to.
fn is_redundant(owned_elsewhere: i64) -> bool {
    owned_elsewhere >= REDUNDANT_COPIES
}

/// Whether an item id is an armament, which is the only category with an ash of war on it.
fn is_armament(item_id: u32) -> bool {
    item_id & ITEM_CATEGORY_MASK == 0x0000_0000
}

/// The identity two copies share when they are the same item to a player reading the menu.
///
/// An armament's id carries three things: the base row, the affinity, and the upgrade level. A
/// Heavy Longsword +25 and a plain Longsword +0 are both a Longsword on the shelf, and the
/// directive this implements says so explicitly -- compare by name, ignoring the ash of war and
/// the infusion. [`split_armament_id`] is the arithmetic the exporter already runs for exactly
/// this, so the affinity table lives in one place.
///
/// Everything else is its own identity. Armour cannot be upgraded and a talisman's `+1` is a
/// different item with a different name, so folding those together would destroy something the
/// player does not have a second of.
fn shelf_identity(item_id: u32) -> u32 {
    if is_armament(item_id) {
        split_armament_id(item_id).row
    } else {
        item_id
    }
}

/// How many copies of one item the build asked for, and which ids count as that item.
///
/// One bucket per grant rather than a map keyed by id, because a name can resolve to more than
/// one row (`Grant::also_known_as`) and those rows have to draw on the same allowance. Keyed by
/// [`shelf_identity`] so the plan's `+0` and the character's `+25` land in the same bucket without
/// anyone having to join them up.
struct KeepBudget {
    ids: Vec<u32>,
    remaining: u32,
}

/// What the build entitles the character to keep, and the copies it already minted.
///
/// # Why a budget and not a set of ids
///
/// It was a set until 2026-09-10, and a set cannot express "one". A build naming one Serpent
/// Crest Shield kept every Serpent Crest Shield in the inventory, because they all share an item
/// id -- so the copy this import had just minted and the copy the previous build left behind both
/// survived, and the sweep reported 168 entries left alone for a build with 24 gear positions.
/// Five Crimson Seed Talismans were kept the same way.
pub struct Keep {
    budgets: Vec<KeepBudget>,
    /// `GaItemHandle`s the grant minted this import. These are the build's own copies and are
    /// never swept, whatever the budget says -- an ash lives on the instance, so the item id
    /// cannot tell this copy from the old one it is replacing.
    minted: BTreeSet<u32>,
}

impl Keep {
    /// Build the allowance from the plan and from what the grant actually minted.
    pub fn new(grants: &[Grant], armaments: &[ArmamentOutcome]) -> Self {
        let budgets = grants
            .iter()
            .map(|grant| KeepBudget {
                ids: std::iter::once(grant.item_id)
                    .chain(grant.also_known_as.iter().copied())
                    .map(shelf_identity)
                    .collect(),
                remaining: grant.quantity,
            })
            .collect();
        let minted = armaments
            .iter()
            .map(|arm| arm.handle)
            .filter(|handle| *handle != 0)
            .collect();
        Self { budgets, minted }
    }

    /// Whether this exact copy is one the grant minted.
    fn is_minted(&self, handle: u32) -> bool {
        handle != 0 && self.minted.contains(&handle)
    }

    /// Spend one of the build's allowance on this item, if any is left.
    fn take(&mut self, item_id: u32) -> bool {
        let identity = shelf_identity(item_id);
        for budget in &mut self.budgets {
            if budget.remaining > 0 && budget.ids.contains(&identity) {
                budget.remaining -= 1;
                return true;
            }
        }
        false
    }
}

/// Total quantity per [`shelf_identity`], from one walk of an inventory.
fn shelf_counts(entries: Vec<InventoryEntry>) -> BTreeMap<u32, i64> {
    let mut counts: BTreeMap<u32, i64> = BTreeMap::new();
    for entry in entries {
        if entry.quantity <= 0 {
            continue;
        }
        *counts.entry(shelf_identity(entry.item_id)).or_default() += i64::from(entry.quantity);
    }
    counts
}

/// Why one deposit was refused.
///
/// An enum rather than the string it prints, because the decision that follows -- destroy this
/// copy, or keep it and report -- turns on which of these it is, and matching on a sentence is
/// how the wrong one gets destroyed after somebody rewords a log line.
#[derive(Clone, Copy)]
enum Refusal {
    /// Still on the character, so removing its entry would leave a `ChrAsm` slot dangling.
    Worn,
    /// The box's ordinary item list has no free entry left.
    BoxHasNoRoom,
    /// The box holds this item already and the stack will take no more.
    StackAtMaximum,
    /// The box does not accept this kind of item at all.
    WrongKind,
}

impl Refusal {
    /// Whether the refusal is about the box being out of space rather than about the item.
    ///
    /// The two cases where destroying a redundant copy is the answer. `Worn` is not one of them:
    /// the pass could not take the item off, so it has no business destroying it. Neither is
    /// `WrongKind`, where the box would refuse the item however empty it was, and a copy the box
    /// would never hold is not redundant with anything.
    fn is_the_box_being_full(self) -> bool {
        matches!(self, Self::BoxHasNoRoom | Self::StackAtMaximum)
    }

    /// Add this refusal to its own counter.
    fn record(self, outcome: &mut EvictOutcome) {
        match self {
            Self::Worn => outcome.refused_worn += 1,
            Self::BoxHasNoRoom => outcome.refused_box_no_room += 1,
            Self::StackAtMaximum => outcome.refused_box_full += 1,
            Self::WrongKind => outcome.refused_kind += 1,
        }
    }

    /// The sentence for the log.
    fn explain(self) -> &'static str {
        match self {
            Self::Worn => {
                "still worn, and `UnequipItem` has no verified mapping for the running build"
            }
            Self::BoxHasNoRoom => {
                "the storage box is full -- its ordinary item list has no free entry left"
            }
            Self::StackAtMaximum => {
                "the storage box already holds this item at its maxRepositoryNum"
            }
            Self::WrongKind => "the storage box will not take this kind of item at all",
        }
    }
}

/// The row id under which the category masks off, for the name getters.
const ITEM_ID_ROW_MASK: u32 = 0x0FFF_FFFF;

/// What the game calls this item, with the raw id kept beside it.
///
/// A log line reading `item 0x100FDE80` cannot answer the only question anyone asks of this pass
/// -- which of my things went where -- so every line names the item. The id stays because it is
/// what a follow-up query needs.
///
/// An armament is named by [`shelf_identity`], the same base row the exporter names it by: the
/// upgrade level has no `EquipParamWeapon` row of its own, so a levelled id has no name at all,
/// and the affinity is a prefix the menu shows separately.
///
/// # Safety
///
/// Game thread, `msg` a live `MsgRepositoryImp*` when present.
unsafe fn label_for(msg: Option<usize>, module_base: usize, item_id: u32) -> String {
    let hex = format!("0x{item_id:08X}");
    let Some(msg) = msg else {
        return hex;
    };
    let (kind, row) = if is_armament(item_id) {
        (Kind::Weapon, shelf_identity(item_id))
    } else if item_id & ITEM_CATEGORY_MASK == 0x1000_0000 {
        (Kind::Protector, item_id & ITEM_ID_ROW_MASK)
    } else {
        (Kind::Talisman, item_id & ITEM_ID_ROW_MASK)
    };
    // Safety: delegated -- `name_for` resolves its getter for the running build and answers `None`
    // rather than faulting on a row the repository does not carry.
    match unsafe { crate::catalog::name_for(kind, msg, module_base, row) } {
        Some(name) => format!("{name} ({hex})"),
        None => hex,
    }
}

/// Which of the three swept categories an id belongs to, as an index.
fn category_of(item_id: u32) -> usize {
    match item_id & ITEM_CATEGORY_MASK {
        0x0000_0000 => 0,
        0x1000_0000 => 1,
        _ => 2,
    }
}

/// Whether an item id is one of the three categories this pass sweeps.
fn is_swept(item_id: u32) -> bool {
    SWEPT_CATEGORIES.contains(&(item_id & ITEM_CATEGORY_MASK))
        && !EMPTY_SLOT_PLACEHOLDERS.contains(&item_id)
}

/// What one eviction pass did.
#[derive(Debug, Default)]
pub struct EvictOutcome {
    /// Distinct gear entries the build does not name.
    pub found: usize,
    /// Entries the box accepted, and how many items that came to.
    pub deposited_entries: usize,
    pub deposited_items: u32,
    /// Entries taken off the character so they could be deposited at all.
    ///
    /// The previous build's gear is worn in the positions the new build also names, and
    /// [`crate::equip_native::vacate_all`] does not touch those -- it clears only the positions
    /// the build wants bare. So this pass takes off what it is about to deposit, exactly as
    /// [`crate::reorder`] does, and leaves the equip that follows to dress the character.
    pub unequipped: usize,
    /// Entries left alone because the build names them -- the character keeps wearing these.
    ///
    /// Reported because "my shield survived" has two completely different causes and the log
    /// could not tell them apart: the build asked for it, or the box had no room. One is the
    /// pass working and the other is the pass stuck.
    pub kept: usize,
    /// Names of the kept entries, capped like the rest.
    pub kept_names: Vec<String>,
    /// `(item, why)` for gear the box would not take, sampled per category for the log.
    pub refused: Vec<(String, String)>,
    /// How many were refused in total, which is not the same as `refused.len()`.
    ///
    /// The list is capped so a character with a full box does not write a thousand lines, and
    /// that cap is exactly what hid the scale of the first live failure: the summary read
    /// `41 of 181 ... 16 refused` while 124 entries had quietly done nothing. A denominator that
    /// does not add up is the one thing a report of this shape must never print.
    pub refused_total: usize,
    /// Refusals split by reason, which the capped list cannot carry.
    ///
    /// The cap is what hid the failure this pass was rebuilt for: sixteen printed refusals were
    /// all ammunition the box was already full of, while the gear the report was about sat
    /// silently in the other hundred and fifteen. A reason that only appears in a list the log
    /// truncates is a reason nobody reads.
    pub refused_worn: usize,
    pub refused_box_full: usize,
    pub refused_kind: usize,
    /// Refused because the box itself had no free entry left, which is not about the item at all.
    pub refused_box_no_room: usize,
    /// Judged redundant, and the destroy did nothing anyway.
    ///
    /// Always zero in a healthy run. A number here means the discard was asked to destroy an item
    /// the inventory does not hold under that id, which is how a copy survives while every count
    /// says it should not.
    pub destroy_failed: usize,
    /// Entries destroyed because the box would not take them and the shelf already had a pair.
    pub discarded_entries: usize,
    /// How many items that came to.
    pub discarded_items: u32,
    /// Ashes of War taken off a doomed armament and given back, measured by the entry count.
    pub ashes_recovered: usize,
    /// `(item, how many, whether an ash came back)` for the log, capped like the refusals.
    pub discarded: Vec<(String, u32, bool)>,
    /// `(entries used, entries the box holds)` after the pass, when both could be read.
    pub box_slots: Option<(i32, i32)>,
    /// Why nothing was attempted, when that is the answer.
    pub unavailable: Option<&'static str>,
}

impl EvictOutcome {
    /// One line for the import log.
    pub fn summary(&self) -> String {
        match self.unavailable {
            Some(why) => format!("EVICT: nothing was moved -- {why}"),
            None => format!(
                "EVICT: of {} carried armament/armour/talisman entr(ies), {} are within what the \
                 build asks for and stay. Of the other {}, {} went to the storage box ({} items, \
                 {} taken off the character first); {} refused -- \
                 {} still worn, {} the box had no room for, {} the box already holds at its \
                 maximum stack, {} the box will not take{}{}. Consumables, materials and key \
                 items were not touched",
                self.found + self.kept,
                self.kept,
                self.found,
                self.deposited_entries,
                self.deposited_items,
                self.unequipped,
                self.refused_total,
                self.refused_worn,
                self.refused_box_no_room,
                self.refused_box_full,
                self.refused_kind,
                match self.box_slots {
                    Some((used, capacity)) =>
                        format!(". The storage box holds {used} of {capacity} entries"),
                    None => String::new(),
                },
                if self.discarded_entries == 0 && self.destroy_failed == 0 {
                    String::new()
                } else if self.destroy_failed > 0 {
                    format!(
                        ". {} entr(ies) ({} item(s)) were DESTROYED instead; {} more were judged \
                         redundant and the destroy did nothing at all, which should never happen \
                         and is named one by one above",
                        self.discarded_entries, self.discarded_items, self.destroy_failed
                    )
                } else {
                    format!(
                        ". {} entr(ies) ({} item(s)) were DESTROYED instead, because the box \
                         would not take them and already holds {REDUNDANT_COPIES} or more of the \
                         same item; {} Ash(es) of War were taken off first and returned to the \
                         inventory",
                        self.discarded_entries, self.discarded_items, self.ashes_recovered
                    )
                }
            ),
        }
    }
}

/// Deposit every carried armament, piece of armour and talisman the build does not name.
///
/// # Why it takes gear off the character itself
///
/// A worn entry is named by `EquipGameData.equipmentItemIdxList` and `Storage::deposit` refuses
/// it, because removing it from the inventory would leave that slot naming whatever entry slid
/// into its place. So gear can only be deposited once it is off the character.
///
/// [`crate::equip_native::vacate_all`] runs first but takes off only part of it: the positions
/// the build leaves empty. The previous build's weapon in a hand the new build also names stays
/// on, because the pass that replaces it -- the equip -- has not run yet, and cannot run first
/// (it resolves inventory indices, which every deposit here shifts). Reported 2026-09-10:
/// "Bloodfiend's Blood Arm was still not evicted among some others", and it was worn.
///
/// So each entry is taken off immediately before its deposit, the same way [`crate::reorder`]
/// does it, and left off. The equip that follows is what dresses the character, and it writes
/// every position the build names anyway.
///
/// # Safety
///
/// Game thread, character in the world, grants and equips already applied.
pub unsafe fn unlisted_gear(module_base: usize, egd: usize, keep: &mut Keep) -> EvictOutcome {
    let mut outcome = EvictOutcome::default();

    let Some(get_inventory) = crate::native::resolve(
        module_base,
        er_game_base::rva::GET_EQUIP_INVENTORY_DATA_RVA,
        "CS::EquipGameData::GetEquipInventoryData",
    ) else {
        outcome.unavailable =
            Some("`GetEquipInventoryData` has no verified mapping for the running build");
        return outcome;
    };
    // Safety: resolved for the running build on the line above.
    let get_inventory: unsafe extern "system" fn(usize) -> usize =
        unsafe { core::mem::transmute(get_inventory) };
    // Safety: game thread, `egd` live; the getter reads one field.
    let carried = unsafe { get_inventory(egd) };
    if carried == 0 {
        outcome.unavailable = Some("the carried inventory is null");
        return outcome;
    }
    // Safety: delegated -- `open` null-checks its singletons and resolves every native first.
    let Some(storage) = (unsafe { Storage::open(module_base, egd, carried) }) else {
        outcome.unavailable = Some("the storage box is unreachable this session");
        return outcome;
    };
    // Optional on purpose, and the cost of it being absent is exactly the failure this pass was
    // rebuilt for: worn gear is refused rather than moved, and says so.
    let clearer = SlotClearer::open(module_base);

    // Snapshot first, move second. Every deposit reindexes the inventory, so walking and
    // depositing in one pass would read entries that have shifted under it.
    // Safety: game thread, read only.
    let mut entries: Vec<InventoryEntry> = unsafe { storage.carried_entries() }
        .into_iter()
        .filter(|entry| is_swept(entry.item_id))
        .collect();
    // Copies the grant minted go first, so they spend the build's allowance before an older twin
    // sharing their item id can. A stable sort keeps inventory order inside each group, which is
    // the order the equip pass resolves a repeated id in.
    entries.sort_by_key(|entry| !keep.is_minted(entry.handle));

    // What the storage box already holds, keyed by item rather than by exact id, and read once.
    // Deposits during the loop only ever add to it, so a stale count under-reports -- which errs
    // toward keeping an item rather than destroying one, the right direction for the only thing
    // in this pass that cannot be undone.
    // Safety: game thread, read only.
    let mut shelf = shelf_counts(unsafe { storage.box_entries() });
    // What this pass keeps in the inventory, per item. It counts toward the same total as the
    // shelf: the question the threshold asks is how many of this item the character still owns
    // once the surplus copy is gone, and a copy kept on the character is owned exactly as much
    // as one on the shelf. Measured 2026-09-10: a Serpent Crest Shield the build kept in Magic
    // sat beside its Standard twin, the box held one, and the twin survived on a count of 1.
    let mut kept_here: BTreeMap<u32, i64> = BTreeMap::new();

    // Resolved once. Absent only before the params stream in, which cannot be the case here --
    // the pass runs on a character in the world -- so the hex fallback is a belt, not a plan.
    let msg = crate::catalog::msg_repository();

    // Per category, not per pass. A flat cap is what hid the gear: sixteen lines filled up with
    // ammunition and armour before a single weapon reached the log, so the pass looked like it
    // had never touched one.
    let mut refused_shown = [0usize; 3];
    let mut destroyed_shown = [0usize; 3];

    for entry in entries {
        let InventoryEntry {
            handle,
            item_id,
            quantity,
            ..
        } = entry;
        // A minted copy is kept whatever the allowance says -- it is the item this import just
        // made, and its ash lives on this instance and no other. It still spends a slot, so the
        // old copy it replaces is not also kept.
        let minted = keep.is_minted(handle);
        let allowed = keep.take(item_id);
        if minted || allowed {
            outcome.kept += 1;
            *kept_here.entry(shelf_identity(item_id)).or_default() += i64::from(quantity.max(1));
            if outcome.kept_names.len() < KEPT_NAMED {
                // Safety: game thread, `msg` live.
                outcome
                    .kept_names
                    .push(unsafe { label_for(msg, module_base, item_id) });
            }
            continue;
        }
        outcome.found += 1;
        if quantity <= 0 {
            continue;
        }
        // Take it off first. `deposit` re-resolves the entry by item id and refuses a worn one,
        // so the index asked about here is the one it will act on -- and for several copies of an
        // id, the lowest-index copy being worn is what blocks every other copy from moving too.
        // Safety: game thread, read only.
        let index = unsafe { storage.carried_index(item_id) };
        // Safety: a bounded read of 22 ints inside a live `EquipGameData`.
        if let Some(slot) = unsafe { storage.equipped_slot_of_index(index) }
            && let Some(clearer) = clearer.as_ref()
        {
            // Safety: game thread, player in the world (the caller's contract), and the item
            // stays in the inventory, which is what makes it depositable on the next line.
            unsafe { clearer.clear(slot) };
            outcome.unequipped += 1;
        }
        // The entry's own quantity, never `carried_quantity`. For a non-stackable that helper
        // counts the matching entries rather than reading a quantity -- eleven Longswords answer
        // `11` while each entry holds one -- and `TransferItemBetweenInventoryDatas` rejects a
        // move of eleven against an entry of one through its own `quantity <= entry quantity`
        // guard. The result is a deposit that silently does nothing, once per copy.
        //
        // Measured on the first live run, 2026-09-10: `EVICT: 41 of 181 ... 16 refused` left 124
        // entries in neither column, and they were all this. Walking entries and using each
        // entry's own quantity is right for gear (one per entry) and for the one stackable family
        // in these categories, ammunition (the stack size, which is what the box takes).
        //
        // Safety: `deposit` asks the box what it will take, refuses a worn entry, re-resolves the
        // index immediately before the transfer and measures what actually moved.
        let moved = unsafe { storage.deposit(item_id, quantity) }.max(0) as u32;
        if moved > 0 {
            outcome.deposited_entries += 1;
            outcome.deposited_items += moved;
            // The shelf now holds one more, and the next copy of this item has to be judged
            // against that rather than against the count taken before the pass began. Measured
            // 2026-09-10: a character carrying two Spiralhorn Shields deposited the first into
            // the last free slot and then spared the second on a stale count of one, while the
            // box it was being compared against already held two.
            *shelf.entry(shelf_identity(item_id)).or_default() += i64::from(moved);
            continue;
        }
        // Safety: a bounded read of 22 ints inside a live `EquipGameData`.
        let index = unsafe { storage.carried_index(item_id) };
        // Three different facts, and folding them together is how a full box reads as a broken
        // sweep. `ChangeAmountInBox` answers 0 both for an item the box refuses and for one it
        // has no room for, so the two are told apart by asking what the box already holds.
        // Safety: game thread, read only.
        let why = if unsafe { storage.equipped_slot_of_index(index) }.is_some() {
            Refusal::Worn
        } else if unsafe { storage.box_free_slots() }
            .is_some_and(|(used, capacity)| used >= capacity)
        {
            // First of the three box answers, because it is the one that has nothing to do with
            // the item. A non-stackable being added never consults the box's holdings of its id:
            // `GetAddOrRemoveAmount` answers the single boolean `count < capacity`, so a full box
            // refuses every piece of gear identically and would otherwise be reported as the box
            // refusing that kind of item.
            Refusal::BoxHasNoRoom
        } else if unsafe { storage.stored_quantity(item_id) } > 0 {
            Refusal::StackAtMaximum
        } else {
            Refusal::WrongKind
        };

        // The box would not take it. If the player already has two or more of the same item on
        // the shelf -- the same item by name, so a different ash or infusion is still the same
        // item -- this copy is redundant and is destroyed instead of being carried around
        // forever. See the module header for why the threshold is two and why the ash comes off
        // first.
        let identity = shelf_identity(item_id);
        let owned_elsewhere = shelf.get(&identity).copied().unwrap_or(0)
            + kept_here.get(&identity).copied().unwrap_or(0);
        let mut destroy_failed_here = false;
        if why.is_the_box_being_full() && is_redundant(owned_elsewhere) && storage.can_discard() {
            // The ash first, and on the index `discard` will actually take -- `carried_index`
            // names the lowest copy of the id and both calls ask it the same question, so they
            // agree about which copy is being destroyed.
            // Safety: game thread, player in the world, `index` a live carried entry.
            let ash_recovered = is_armament(item_id) && unsafe { storage.strip_ash(index) };
            // The id again, because the strip may have changed it. Taking an ash off resets the
            // armament's affinity and the affinity is part of the item id, so a Magic Spiralhorn
            // Shield comes back as the Standard one -- and `discard`, which resolves by id, then
            // looks up a row the inventory no longer holds, destroys nothing and reports nothing.
            // Measured 2026-09-10: `NOT EVICTED Spiralhorn Shield (0x01CCACE9)` in the log, with
            // `0x01CCA9C9` sitting in the inventory afterwards. The index survives the strip; the
            // id does not.
            // Safety: game thread, read only.
            let doomed = unsafe { storage.carried_item_id_at(index) }.unwrap_or(item_id);
            // Safety: game thread; `discard` re-resolves the index immediately before the
            // destructive call and refuses when the pair has no mapping for this build.
            let destroyed = unsafe { storage.discard(doomed, quantity) }.max(0) as u32;
            if destroyed == 0 {
                // Judged redundant, and nothing happened. That is a different failure from the
                // box being full and it must not print as one: it read as an ordinary box-full
                // refusal for a whole round trip while the real cause was `strip_ash` changing
                // the item id out from under `discard`.
                outcome.destroy_failed += 1;
                destroy_failed_here = true;
            }
            if destroyed > 0 {
                outcome.discarded_entries += 1;
                outcome.discarded_items += destroyed;
                if ash_recovered {
                    outcome.ashes_recovered += 1;
                }
                let category = category_of(item_id);
                if destroyed_shown[category] < LINES_PER_CATEGORY {
                    destroyed_shown[category] += 1;
                    // Safety: game thread, `msg` live.
                    let label = unsafe { label_for(msg, module_base, item_id) };
                    outcome.discarded.push((label, destroyed, ash_recovered));
                }
                continue;
            }
        }

        // Counted here rather than where `why` is decided, so an entry that goes on to be
        // destroyed is not also counted as one the box refused. It is one or the other.
        why.record(&mut outcome);
        outcome.refused_total += 1;
        let category = category_of(item_id);
        if refused_shown[category] < LINES_PER_CATEGORY {
            refused_shown[category] += 1;
            // Safety: game thread, `msg` live.
            let label = unsafe { label_for(msg, module_base, item_id) };
            // How many the shelf holds, spelled out. A refusal that says only "the box is full"
            // does not say whether the copy was spared by the threshold or was never eligible,
            // and those are the two different things a reader has to tell apart before deciding
            // whether the threshold is the thing to change.
            let held = owned_elsewhere;
            let why = match why {
                _ if destroy_failed_here => format!(
                    "it was judged redundant ({held} owned elsewhere) and the destroy did \
                     nothing -- the inventory holds no entry under that item id"
                ),
                Refusal::BoxHasNoRoom | Refusal::StackAtMaximum if held < REDUNDANT_COPIES => {
                    format!(
                        "{}, and the character would still own only {held} of this item -- \
                         fewer than the {REDUNDANT_COPIES} needed before a copy is destroyed \
                         instead",
                        why.explain()
                    )
                }
                why => why.explain().to_owned(),
            };
            outcome.refused.push((label, why));
        }
    }

    // Safety: game thread, read only.
    outcome.box_slots = unsafe { storage.box_free_slots() };
    outcome
}

/// How many lines each of the three categories may contribute to the log.
///
/// Per category rather than per pass, so a hundred refused arrows cannot crowd out the one
/// refused shield -- which is exactly what happened on 2026-09-10, when all sixteen printed
/// refusals were talismans and armour and the weapons went unmentioned.
const LINES_PER_CATEGORY: usize = 8;
/// How many kept entries are named. A build names two dozen things at most.
const KEPT_NAMED: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every refusal reaches the summary line, whatever the printed list is capped at.
    ///
    /// The regression this guards is not a wrong number, it is an invisible one: the run that
    /// left worn gear behind printed sixteen refusals, all of them ammunition, and the fourteen
    /// worn armaments underneath never appeared anywhere in the log.
    #[test]
    fn the_summary_carries_each_refusal_reason() {
        let outcome = EvictOutcome {
            found: 172,
            deposited_entries: 41,
            deposited_items: 41,
            unequipped: 14,
            refused_total: 131,
            refused_worn: 3,
            refused_box_no_room: 100,
            refused_box_full: 15,
            refused_kind: 13,
            box_slots: Some((2000, 2000)),
            discarded_entries: 0,
            discarded_items: 0,
            ashes_recovered: 0,
            discarded: Vec::new(),
            destroy_failed: 0,
            kept: 4,
            kept_names: Vec::new(),
            refused: Vec::new(),
            unavailable: None,
        };
        let summary = outcome.summary();
        assert!(
            summary.contains("14 taken off the character first"),
            "{summary}"
        );
        // The denominator has to account for every swept entry, kept ones included. Reporting
        // "41 of 172" beside "168 left alone" invites the reader to add them to 340 or to assume
        // the 168 came from somewhere else; neither is what happened.
        assert!(
            summary.contains("of 176 carried armament/armour/talisman entr(ies), 4 are within"),
            "{summary}"
        );
        assert!(summary.contains("Of the other 172,"), "{summary}");
        assert!(summary.contains("3 still worn"), "{summary}");
        assert!(summary.contains("100 the box had no room for"), "{summary}");
        assert!(summary.contains("15 the box already holds"), "{summary}");
        assert!(summary.contains("13 the box will not take"), "{summary}");
        assert!(summary.contains("holds 2000 of 2000 entries"), "{summary}");
        assert_eq!(
            outcome.refused_worn
                + outcome.refused_box_no_room
                + outcome.refused_box_full
                + outcome.refused_kind,
            outcome.refused_total,
            "the four reasons are the whole of the refusals, so a reader can check the \
             denominator without the truncated list"
        );
    }

    /// A grant for one item, with everything else at a value the allowance does not read.
    fn grant_of(item_id: u32, quantity: u32, also: &[u32]) -> Grant {
        Grant {
            item_id,
            also_known_as: also.to_vec(),
            quantity,
            reinforce_lv: 0,
            upgrade_is_character_default: false,
            weapon_skill: er_build_import_core::plan::NO_SKILL,
            label: String::new(),
            pot_group: None,
            armament: false,
        }
    }

    /// A build asking for five keeps five, and the sixth is an extra.
    ///
    /// The case that made the allowance necessary: a set of ids kept every copy of a named id, so
    /// a build wanting five Crimson Seed Talismans and a character holding ten kept all ten.
    #[test]
    fn the_allowance_is_a_count_not_a_set() {
        let talisman = 0x2000_1BE4u32;
        let mut keep = Keep::new(&[grant_of(talisman, 5, &[])], &[]);
        for copy in 1..=5 {
            assert!(
                keep.take(talisman),
                "copy {copy} is within the build's five"
            );
        }
        assert!(!keep.take(talisman), "the sixth copy is an extra");
    }

    /// The plan names an armament at `+0` and the character carries it at `+25`.
    #[test]
    fn the_allowance_ignores_the_upgrade_level_and_the_infusion() {
        // Serpent Crest Shield: the plan's row, and the id the character actually holds.
        let planned = 0x01E0_F500u32;
        let carried = 0x01E0_F519u32;
        let mut keep = Keep::new(&[grant_of(planned, 1, &[])], &[]);
        assert!(
            keep.take(carried),
            "the +25 copy is the item the build named"
        );
        assert!(!keep.take(carried), "and the build named exactly one");
    }

    /// Rows sharing a name draw on one allowance rather than one each.
    #[test]
    fn alternates_share_the_build_s_allowance() {
        let row = 0x4000_2AFFu32;
        let other_row = 0x4000_2B03u32;
        let mut keep = Keep::new(&[grant_of(row, 1, &[other_row])], &[]);
        assert!(keep.take(other_row), "the copy the character holds counts");
        assert!(
            !keep.take(row),
            "and having counted it, the allowance is spent -- this is the duplicate flask"
        );
    }

    /// Two copies of one armament are the same item however they are infused or upgraded.
    #[test]
    fn the_shelf_ignores_the_ash_and_the_infusion() {
        // Misericorde: plain +0, Occult +0, Occult +9. One item on the shelf.
        let plain = 1_070_000u32;
        let occult = 1_071_200u32;
        let occult_nine = 1_071_209u32;
        assert_eq!(shelf_identity(plain), plain);
        assert_eq!(shelf_identity(occult), plain);
        assert_eq!(shelf_identity(occult_nine), plain);
        // A somber armament has no affinity block, so its own row is the identity.
        assert_eq!(shelf_identity(1_010_007), 1_010_000);
    }

    /// Armour and talismans are folded together with nothing, because their `+1` is another item.
    #[test]
    fn armour_and_talismans_keep_their_own_identity() {
        let head = 0x1001_86A0u32;
        assert_eq!(shelf_identity(head), head);
        let talisman = 0x2000_0FA0u32;
        assert_eq!(shelf_identity(talisman), talisman);
        assert_ne!(shelf_identity(talisman), shelf_identity(talisman + 1));
    }

    /// The shelf counts quantities, so one stack of thirty is thirty and not one.
    #[test]
    fn the_shelf_sums_quantities_per_item() {
        let entry = |index: i32, item_id: u32, quantity: i32| InventoryEntry {
            index,
            handle: 0,
            item_id,
            quantity,
        };
        let counts = shelf_counts(vec![
            entry(0, 1_071_200, 1),
            entry(1, 1_070_000, 1),
            entry(2, 0x1001_86A0, 1),
            entry(3, 0x0300_20A0, 30),
            // A freed entry contributes nothing.
            entry(4, 1_070_000, 0),
        ]);
        assert_eq!(counts.get(&1_070_000), Some(&2));
        assert_eq!(counts.get(&0x1001_86A0), Some(&1));
        assert_eq!(counts.get(&0x0300_20A0), Some(&30));
    }

    /// The threshold counts everything the character still owns, not just the shelf.
    #[test]
    fn a_copy_is_redundant_only_when_two_survive_it() {
        // The Spiralhorn case: one on the shelf at the start of the pass, one deposited into the
        // last free slot during it. The live count is two and the third copy goes.
        assert!(is_redundant(1 + 1));
        // The Serpent Crest case: one on the shelf, one kept on the character because the build
        // names it. Also two.
        assert!(is_redundant(2));
        // One anywhere is not a spare, and neither is none.
        assert!(!is_redundant(1));
        assert!(!is_redundant(0));
    }

    /// Only a box that is out of space justifies destroying a copy.
    #[test]
    fn a_refusal_about_the_item_never_destroys_it() {
        assert!(Refusal::BoxHasNoRoom.is_the_box_being_full());
        assert!(Refusal::StackAtMaximum.is_the_box_being_full());
        // The pass could not take it off, so it has no business destroying it.
        assert!(!Refusal::Worn.is_the_box_being_full());
        // A copy the box would never hold is not redundant with anything.
        assert!(!Refusal::WrongKind.is_the_box_being_full());
    }

    /// The vacate pass's own placeholders are not gear and are never swept.
    #[test]
    fn the_empty_slot_placeholders_are_skipped() {
        // The Unarmed fist, which a cleared hand holds.
        assert!(!is_swept(0x0001_ADB0));
        // The four empty-armour rows, which cleared protector slots hold.
        for empty in [0x1000_2710u32, 0x1000_2774, 0x1000_27D8, 0x1000_283C] {
            assert!(!is_swept(empty), "0x{empty:08X}");
        }
        // A real weapon whose id happens to sit near the fist's is still swept.
        assert!(is_swept(0x0001_ADB1));
    }

    /// Only three categories are swept, and goods are not one of them.
    #[test]
    fn goods_gems_and_spells_are_outside_this_pass() {
        assert!(is_swept(0x0000_0000));
        assert!(is_swept(0x002F_0409));
        // A real piece of armour, deliberately not `0x10002710` -- that is the empty head row and
        // is skipped as a placeholder, and picking it here is what made this test contradict
        // `the_empty_slot_placeholders_are_skipped`.
        assert!(is_swept(0x1001_86A0));
        assert!(is_swept(0x2000_0000));
        // Goods: consumables, crafting materials, key items, spells, remembrances.
        assert!(!is_swept(0x4000_0384));
        // Gems, i.e. ashes of war.
        assert!(!is_swept(0x8000_0000));
    }

    /// The upgrade level rides in the id, which is what the keep-set has to account for.
    #[test]
    fn an_upgraded_armament_is_a_different_id_from_the_one_the_plan_names() {
        let planned = 0x002F_0400u32;
        let carried = planned + 25;
        let keep: BTreeSet<u32> = [planned].into_iter().collect();
        assert!(is_swept(carried));
        assert!(
            !keep.contains(&carried),
            "a keep-set of planned ids alone does not recognise the granted armament, which is \
             why `unlisted_gear` is given the grant's own ids too"
        );
    }
}
