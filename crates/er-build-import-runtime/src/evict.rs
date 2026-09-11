//! Sending the gear the build does not name back to the storage box.
//!
//! # The invariant this pass owes
//!
//! Every gear entry the character holds when this pass finishes is a copy the build's grant list
//! names, in no more items of it than that list asks for, and every copy that is not appears by
//! name in this pass's report with the reason it could not be moved.
//!
//! The first two thirds of that are decided by [`er_build_import_core::sweep`], which is host
//! testable and is held to the invariant over generated inventories rather than chosen ones. The
//! last third is this module's own job and is the part that used to be missing: the pass counted
//! what it had attempted and never asked the game what was still there afterwards.
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
//! previous build, still in the pockets.
//!
//! # What is never touched, and why that is a property rather than a list
//!
//! [`Category::is_gear`] answers it from the item id's top nibble, so every consumable, crafting
//! material, key item, spell and remembrance is outside this pass by construction. The engine's
//! own empty-slot rows -- the Unarmed fist in a bare hand, the empty-piece row in a bare armour
//! slot -- are outside it by [`er_build_import_core::sweep::is_engine_placeholder`], which is a
//! statement about what those ids mean rather than a list this module maintains. Both live in the
//! core crate, where the property
//! test can hold the classification to being total.
//!
//! # When the box is full, a surplus copy is destroyed
//!
//! An armament does not always have somewhere else to go. Measured 2026-09-10 and again
//! 2026-09-11: the storage box was at `1920 of 1920` entries, so the box refused gear for no
//! reason to do with the gear at all.
//!
//! So, by user directive: when the box will not take an entry **and** the character would still
//! own [`REDUNDANT_COPIES`] or more of the same item without it, the carried copy is destroyed
//! instead. That count is measured rather than accumulated -- see [`retained_after`] -- because
//! three separate live failures were all the same failure: a running total of the character's
//! holdings that the loop keeping it was itself mutating.
//!
//! The Ash of War comes off first, through the engine's own remove action
//! ([`er_game_base::rva::REMOVE_GEM_FROM_WEAPON_RVA`]), because an ash is an item consumed into
//! one specific instance and destroying the weapon with it mounted destroys the ash too.
//!
//! A refusal the box makes about the *item* -- it will not take that kind at all -- leaves it
//! exactly where it was, and so does an entry still on the character.
//!
//! # The trap that would have deposited the build's own weapons
//!
//! An armament's upgrade level lives in the last two digits of its item id, so the id the plan
//! names (+0) and the id the character carries (+25) are different numbers. An allowance built
//! from the plan alone therefore does not recognise the very armaments the grant just minted. It
//! is built from both: the planned ids and the `GaItemHandle`s the grant reports actually landing,
//! which is what [`GrantOutcome::armaments`] carries and what
//! [`er_build_import_core::sweep::Kept::Pinned`] is named for.
//!
//! [`GrantOutcome::armaments`]: crate::grant::GrantOutcome::armaments

use std::collections::BTreeMap;

use er_build_import_core::catalog::Kind;
use er_build_import_core::plan::Grant;
use er_build_import_core::sweep::{
    Allowance, Category, Disposition, Held, ITEM_ID_ROW_MASK, apply, held_by_identity, identity,
    plan_sweep, survivors,
};

use crate::grant::ArmamentOutcome;

use crate::equip_native::SlotClearer;
use crate::storage::{InventoryEntry, Storage};

/// How many of an item the character must still own before a copy the box will not take is
/// destroyed instead of carried.
///
/// Two, by user directive 2026-09-10. The player keeps a pair; a third copy that the box has no
/// room for is redundant, and carrying it defeats the whole point of the sweep. One would be too
/// thin -- an item held once is the only one there is, and the five armaments that survived the
/// 2026-09-11 import were each the only one of themselves. That is this threshold working, not
/// failing, and the report now says so on its own line instead of inside a sentence about
/// something else.
const REDUNDANT_COPIES: i64 = 2;

/// Whether a copy the box will not take can be destroyed, given how many the character keeps.
///
/// `owned_elsewhere` is what the character still owns once this copy is gone: the box's holding,
/// plus what this pass has already deposited, plus every copy the plan keeps. A copy on the
/// character counts exactly as much as one on the shelf.
fn is_redundant(owned_elsewhere: i64) -> bool {
    owned_elsewhere >= REDUNDANT_COPIES
}

/// Whether an item id is an armament, which is the only category with an ash of war on it.
fn is_armament(item_id: u32) -> bool {
    Category::of(item_id) == Category::Armament
}

/// Build the allowance from the plan's grants and the instances the grant pass produced.
///
/// The handles matter as much as the ids. An ash lives on the gaitem instance, so the item id
/// cannot tell the copy this import just made from the older one it replaces, and a sweep working
/// from ids alone will pick whichever the inventory filed lowest.
pub fn allowance_for(grants: &[Grant], armaments: &[ArmamentOutcome]) -> Allowance {
    Allowance::new(grants, armaments.iter().map(|arm| arm.handle))
}

/// One inventory entry, as the classifier wants it.
fn as_held(entry: &InventoryEntry) -> Held {
    Held {
        handle: entry.handle,
        item_id: entry.item_id,
        quantity: entry.quantity,
    }
}

/// How many items of each identity the character will still hold once this pass has done what the
/// plan says.
///
/// Computed once, from the plan, before anything moves. That is the whole correction: the count
/// this feeds -- whether a copy the box refuses is redundant -- was previously accumulated inside
/// the loop that was changing the thing being counted, and every one of the three fixes made to
/// it on 2026-09-10 was a place the accumulation had missed an update. A total taken from the
/// plan cannot miss one, because it is not accumulated at all.
fn retained_after(
    entries: &[Held],
    plan: &er_build_import_core::sweep::SweepPlan,
) -> BTreeMap<u32, i64> {
    held_by_identity(&apply(entries, plan))
}

/// Total quantity per identity, from one walk of an inventory.
fn shelf_counts(entries: Vec<InventoryEntry>) -> BTreeMap<u32, i64> {
    let held: Vec<Held> = entries.iter().map(as_held).collect();
    held_by_identity(&held)
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

/// What the game calls this item, with the raw id kept beside it.
///
/// A log line reading `item 0x100FDE80` cannot answer the only question anyone asks of this pass
/// -- which of my things went where -- so every line names the item. The id stays because it is
/// what a follow-up query needs.
///
/// An armament is named by its base row, the same row the exporter names it by: the upgrade level
/// has no `EquipParamWeapon` row of its own, so a levelled id has no name at all, and the
/// affinity is a prefix the menu shows separately.
///
/// # Safety
///
/// Game thread, `msg` a live `MsgRepositoryImp*` when present.
unsafe fn label_for(msg: Option<usize>, module_base: usize, item_id: u32) -> String {
    let hex = format!("0x{item_id:08X}");
    let Some(msg) = msg else {
        return hex;
    };
    let (kind, row) = match Category::of(item_id) {
        Category::Armament => (Kind::Weapon, identity(item_id)),
        Category::Protector => (Kind::Protector, item_id & ITEM_ID_ROW_MASK),
        _ => (Kind::Talisman, item_id & ITEM_ID_ROW_MASK),
    };
    // Safety: delegated -- `name_for` resolves its getter for the running build and answers `None`
    // rather than faulting on a row the repository does not carry.
    match unsafe { crate::catalog::name_for(kind, msg, module_base, row) } {
        Some(name) => format!("{name} ({hex})"),
        None => hex,
    }
}

/// Which of the three swept categories an id belongs to, as an index into the per-category log
/// caps.
fn category_of(item_id: u32) -> usize {
    match Category::of(item_id) {
        Category::Armament => 0,
        Category::Protector => 1,
        _ => 2,
    }
}

/// What one eviction pass did.
#[derive(Debug, Default)]
pub struct EvictOutcome {
    /// Gear entries the build does not entitle the character to, in whole or in part.
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
    /// Entries left alone because the build names them -- the character keeps these.
    pub kept: usize,
    /// Entries the pass has no business with at all: not gear, or an engine placeholder, or an
    /// entry holding nothing.
    ///
    /// Counted separately from [`Self::kept`] because they are not a decision. Folding them in
    /// inflated the numerator of a ratio that is supposed to say how much of the character the
    /// build accounts for, and the five empty-slot rows the vacate pass creates immediately
    /// before this one runs were being counted as five things the build asked for.
    pub untouchable: usize,
    /// Names of the kept entries, capped like the rest.
    pub kept_names: Vec<String>,
    /// `(item, why)` for gear the box would not take, sampled per category for the log.
    pub refused: Vec<(String, String)>,
    /// How many were refused in total, which is not the same as `refused.len()`.
    pub refused_total: usize,
    /// Refusals split by reason, which the capped list cannot carry.
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
    /// Entries destroyed because the box would not take them and the character still owns a pair.
    pub discarded_entries: usize,
    /// How many items that came to.
    pub discarded_items: u32,
    /// Ashes of War taken off a doomed armament and given back.
    pub ashes_recovered: usize,
    /// `(item, how many, whether an ash came back)` for the log, capped like the refusals.
    pub discarded: Vec<(String, u32, bool)>,
    /// Gear the character still holds that the build does not entitle it to, measured by reading
    /// the inventory back after the pass rather than by counting what the pass attempted.
    ///
    /// The number this pass exists to drive to zero, and the number it could not previously
    /// report at all. A pass that says what it tried is a pass that cannot be wrong about the
    /// result; this one says what is there.
    pub left_behind: usize,
    /// How many items those entries come to.
    pub left_behind_items: i64,
    /// Of those, the ones this pass has no recorded reason for.
    ///
    /// Every survivor should be a refusal this pass decided and logged. One that is not means a
    /// deposit or a discard reported a number the inventory does not agree with -- which is
    /// exactly what a sweep acting on "whichever copy has this item id" can do when several
    /// copies share one -- and it is the failure that has no other symptom.
    pub left_behind_unexplained: usize,
    /// `(item, why)` for every survivor. Uncapped on purpose: there should be none, and when
    /// there are, they are the whole content of the report.
    pub left_behind_names: Vec<(String, String)>,
    /// Instances this import produced that are no longer in the inventory when the pass ends.
    ///
    /// The over-removal direction, and the expensive one: this is the sweep having deposited or
    /// destroyed the build's own copy. It happens because the natives that move an entry resolve
    /// it by item id and `carried_index` names the lowest-indexed copy rather than the one the
    /// decision was about, so two copies of one id are not distinguishable to the call that moves
    /// them. Nothing else in this pass can see it: the counts all say the surplus copy left.
    pub pinned_lost: usize,
    /// Names of those instances. Uncapped, for the same reason the survivors are.
    pub pinned_lost_names: Vec<String>,
    /// `(entries used, entries the box holds)` after the pass, when both could be read.
    pub box_slots: Option<(i32, i32)>,
    /// Why nothing was attempted, when that is the answer.
    pub unavailable: Option<&'static str>,
}

impl EvictOutcome {
    /// Whether the character ended up holding no more than the build asks for.
    ///
    /// The pass's own verdict on itself, and the one a caller should score. `deposited` and
    /// `discarded` are work done; this is work finished.
    pub fn reconciles(&self) -> bool {
        self.unavailable.is_none()
            && self.left_behind == 0
            && self.destroy_failed == 0
            && self.pinned_lost == 0
    }

    /// One line for the import log, leading with what is still on the character.
    ///
    /// The order is deliberate. The previous line opened with how many entries were examined and
    /// buried the survivors between a refusal breakdown and a destroyed count in the thousands,
    /// so five weapons the pass had failed to move read as a footnote. What the pass failed to do
    /// goes first.
    pub fn summary(&self) -> String {
        match self.unavailable {
            Some(why) => format!("EVICT: nothing was moved -- {why}"),
            None => format!(
                "EVICT: {} gear entr(ies) ({} item(s)) the build does not ask for are STILL ON \
                 THE CHARACTER after this pass{}. Of {} gear entr(ies) examined, {} are within \
                 what the build asks for and stay and {} were surplus; {} went to the storage \
                 box ({} items, {} taken off the character first), {} entr(ies) ({} item(s)) \
                 were destroyed because the box would not take them and the character still owns \
                 {REDUNDANT_COPIES} or more ({} Ash(es) of War recovered first), {} refused -- \
                 {} still worn, {} the box had no room for, {} the box already holds at its \
                 maximum stack, {} the box will not take{}{}{}. {} entr(ies) were outside this \
                 pass entirely: consumables, materials, key items and the engine's own \
                 empty-slot rows",
                self.left_behind,
                self.left_behind_items,
                if self.left_behind_unexplained == 0 {
                    String::new()
                } else {
                    format!(
                        ", {} of them with no refusal this pass recorded, which should never \
                         happen and is named one by one below",
                        self.left_behind_unexplained
                    )
                },
                self.found + self.kept,
                self.kept,
                self.found,
                self.deposited_entries,
                self.deposited_items,
                self.unequipped,
                self.discarded_entries,
                self.discarded_items,
                self.ashes_recovered,
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
                if self.destroy_failed == 0 {
                    String::new()
                } else {
                    format!(
                        ". {} more were judged redundant and the destroy did nothing at all, \
                         which should never happen",
                        self.destroy_failed
                    )
                },
                if self.pinned_lost == 0 {
                    String::new()
                } else {
                    format!(
                        ". {} copy(ies) this import had just made are GONE from the inventory, \
                         which means a deposit or a discard moved the build's own item instead of \
                         the surplus one it was asked about",
                        self.pinned_lost
                    )
                },
                self.untouchable,
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
/// (it resolves inventory indices, which every deposit here shifts).
///
/// # It reads the inventory back before it reports
///
/// The last thing it does is walk the carried inventory again and run the same classification
/// over what it finds. Anything still surplus is a survivor, is named, and carries either the
/// refusal this pass recorded for it or the fact that this pass has no reason for it at all.
/// Without that, the report is a list of intentions.
///
/// # Safety
///
/// Game thread, character in the world, grants and equips already applied.
pub unsafe fn unlisted_gear(module_base: usize, egd: usize, allowance: &Allowance) -> EvictOutcome {
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

    // Snapshot first, decide second, move third. Every deposit reindexes the inventory, so walking
    // and depositing in one pass would read entries that have shifted under it.
    // Safety: game thread, read only.
    let entries = unsafe { storage.carried_entries() };
    let held: Vec<Held> = entries.iter().map(as_held).collect();
    let plan = plan_sweep(&held, allowance);
    let counts = plan.counts();
    outcome.untouchable = counts.untouchable;
    outcome.kept = counts.kept();
    outcome.found = counts.surplus;

    // What the storage box already holds, keyed by item rather than by exact id, and read once.
    // The box on a well-played character holds close to two thousand entries and each one costs a
    // call, so this is the one walk it gets.
    // Safety: game thread, read only.
    let mut shelf = shelf_counts(unsafe { storage.box_entries() });
    // What the character will still hold when the plan has been carried out, taken from the plan
    // rather than accumulated while carrying it out. See `retained_after`.
    let retained = retained_after(&held, &plan);

    // Resolved once. Absent only before the params stream in, which cannot be the case here --
    // the pass runs on a character in the world -- so the hex fallback is a belt, not a plan.
    let msg = crate::catalog::msg_repository();

    // Per category, not per pass. A flat cap is what hid the gear: sixteen lines filled up with
    // ammunition and armour before a single weapon reached the log, so the pass looked like it
    // had never touched one.
    let mut refused_shown = [0usize; 3];
    let mut destroyed_shown = [0usize; 3];

    // Why each surplus entry did not leave, keyed by the handle of the copy it was decided about.
    // The verify pass reads this to say whether a survivor is one this pass knew about.
    let mut reasons: BTreeMap<u32, String> = BTreeMap::new();

    for (index_in_plan, entry) in entries.iter().enumerate() {
        let disposition = plan.disposition(index_in_plan);
        if outcome.kept_names.len() < KEPT_NAMED
            && let Some(Disposition::Kept(why)) = disposition
        {
            // Safety: game thread, `msg` live.
            let label = unsafe { label_for(msg, module_base, entry.item_id) };
            // The reason, not just the name. A kept item and a stuck item look identical from the
            // outside, and so do "the build named this" and "this import made this copy" -- which
            // are different answers to the only question a reader asks of this list.
            outcome
                .kept_names
                .push(format!("{label}: {}", why.explain()));
        }
        let Some(Disposition::Surplus { shed, .. }) = disposition else {
            continue;
        };
        let InventoryEntry {
            handle, item_id, ..
        } = *entry;

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
        // The plan's own number, never `carried_quantity`. For a non-stackable that helper counts
        // the matching entries rather than reading a quantity -- eleven Longswords answer `11`
        // while each entry holds one -- and `TransferItemBetweenInventoryDatas` rejects a move of
        // eleven against an entry of one through its own `quantity <= entry quantity` guard. The
        // result is a deposit that silently does nothing, once per copy.
        //
        // Safety: `deposit` asks the box what it will take, refuses a worn entry, re-resolves the
        // index immediately before the transfer and measures what actually moved.
        let moved = unsafe { storage.deposit(item_id, shed) }.max(0) as u32;
        if moved > 0 {
            outcome.deposited_entries += 1;
            outcome.deposited_items += moved;
            // The shelf now holds one more, and the next copy of this item has to be judged
            // against that rather than against the count taken before the pass began.
            *shelf.entry(identity(item_id)).or_default() += i64::from(moved);
            if i32::try_from(moved).unwrap_or(i32::MAX) >= shed {
                continue;
            }
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

        // The box would not take it. If the character would still own two or more of the same
        // item without it -- the same item by name, so a different ash or infusion is still the
        // same item -- this copy is redundant and is destroyed instead of being carried around
        // forever. See the module header for why the threshold is two and why the ash comes off
        // first.
        let id = identity(item_id);
        let owned_elsewhere =
            shelf.get(&id).copied().unwrap_or(0) + retained.get(&id).copied().unwrap_or(0);
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
            // The index survives the strip; the id does not.
            // Safety: game thread, read only.
            let doomed = unsafe { storage.carried_item_id_at(index) }.unwrap_or(item_id);
            // Safety: game thread; `discard` re-resolves the index immediately before the
            // destructive call and refuses when the pair has no mapping for this build.
            let destroyed = unsafe { storage.discard(doomed, shed) }.max(0) as u32;
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
        // How many the character would still own, spelled out. A refusal that says only "the box
        // is full" does not say whether the copy was spared by the threshold or was never
        // eligible, and those are the two different things a reader has to tell apart before
        // deciding whether the threshold is the thing to change.
        let held = owned_elsewhere;
        let sentence = match why {
            _ if destroy_failed_here => format!(
                "it was judged redundant ({held} owned elsewhere) and the destroy did nothing -- \
                 the inventory holds no entry under that item id"
            ),
            Refusal::BoxHasNoRoom | Refusal::StackAtMaximum if held < REDUNDANT_COPIES => format!(
                "{}, and the character would still own only {held} of this item -- fewer than \
                 the {REDUNDANT_COPIES} needed before a copy is destroyed instead",
                why.explain()
            ),
            why => why.explain().to_owned(),
        };
        reasons.insert(handle, sentence.clone());
        let category = category_of(item_id);
        if refused_shown[category] < LINES_PER_CATEGORY {
            refused_shown[category] += 1;
            // Safety: game thread, `msg` live.
            let label = unsafe { label_for(msg, module_base, item_id) };
            outcome.refused.push((label, sentence));
        }
    }

    // The pass is over. Ask the game what is actually there.
    //
    // Everything above is a record of what this pass attempted; this is the only part that knows
    // what it achieved. The same classification, over the inventory as it now stands: anything
    // still surplus was left behind, whatever the counts above say.
    // Safety: game thread, read only.
    let after = unsafe { storage.carried_entries() };
    let held_after: Vec<Held> = after.iter().map(as_held).collect();
    for index in survivors(&held_after, allowance) {
        let entry = &after[index];
        outcome.left_behind += 1;
        outcome.left_behind_items += i64::from(entry.quantity.max(0));
        // Safety: game thread, `msg` live.
        let label = unsafe { label_for(msg, module_base, entry.item_id) };
        match reasons.get(&entry.handle) {
            Some(sentence) => outcome.left_behind_names.push((label, sentence.clone())),
            None => {
                outcome.left_behind_unexplained += 1;
                outcome.left_behind_names.push((
                    label,
                    "this pass recorded no refusal for this copy, so a deposit or a discard \
                     reported a number the inventory does not agree with"
                        .to_owned(),
                ));
            }
        }
    }

    // The other direction, and the one with no other symptom at all.
    //
    // `Storage::deposit` and `Storage::discard` both resolve the entry they act on by item id, and
    // `carried_index` names the lowest-indexed copy rather than the copy the decision was made
    // about. Two entries sharing an id -- the copy this import just minted and the older one it
    // replaces -- are therefore not distinguishable to the call, so a surplus entry can be
    // deposited by moving the kept one instead. The surplus copy then shows up above as a survivor
    // with no recorded reason; this is the same event seen from the side that costs the player
    // something, and it is the only thing here that says the build's own item went into the box.
    let handles_now: std::collections::BTreeSet<u32> =
        after.iter().map(|entry| entry.handle).collect();
    for entry in &entries {
        if allowance.is_pinned(entry.handle)
            && entry.quantity > 0
            && !handles_now.contains(&entry.handle)
        {
            outcome.pinned_lost += 1;
            // Safety: game thread, `msg` live.
            let label = unsafe { label_for(msg, module_base, entry.item_id) };
            outcome.pinned_lost_names.push(label);
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
    use er_build_import_core::plan::NO_SKILL;
    use er_build_import_core::sweep::{EMPTY_PROTECTOR_ITEM_IDS, Kept, UNARMED_ITEM_ID};

    fn grant_of(item_id: u32, quantity: u32, also: &[u32]) -> Grant {
        Grant {
            item_id,
            also_known_as: also.to_vec(),
            quantity,
            reinforce_lv: 0,
            upgrade_is_character_default: true,
            weapon_skill: NO_SKILL,
            label: format!("item 0x{item_id:08X}"),
            pot_group: None,
            armament: false,
        }
    }

    fn entry(index: i32, handle: u32, item_id: u32, quantity: i32) -> InventoryEntry {
        InventoryEntry {
            index,
            handle,
            item_id,
            quantity,
        }
    }

    /// A survivor is the headline, and the summary says so before it says anything else.
    #[test]
    fn the_summary_leads_with_what_is_still_on_the_character() {
        let outcome = EvictOutcome {
            left_behind: 5,
            left_behind_items: 5,
            left_behind_unexplained: 1,
            found: 94,
            kept: 204,
            ..EvictOutcome::default()
        };
        let summary = outcome.summary();
        let headline = summary.find("STILL ON").expect("the survivors are named");
        let examined = summary.find("examined").expect("the denominator is there");
        assert!(
            headline < examined,
            "the survivors have to come before the work: {summary}"
        );
        assert!(summary.contains("no refusal this pass recorded"));
    }

    /// A pass that moved a great deal and still left something behind has not reconciled, and
    /// neither has one that moved the build's own copy by mistake.
    #[test]
    fn work_done_is_not_work_finished() {
        assert!(EvictOutcome::default().reconciles());
        assert!(
            !EvictOutcome {
                deposited_entries: 88,
                left_behind: 5,
                ..EvictOutcome::default()
            }
            .reconciles()
        );
        assert!(
            !EvictOutcome {
                destroy_failed: 1,
                ..EvictOutcome::default()
            }
            .reconciles()
        );
        assert!(
            !EvictOutcome {
                deposited_entries: 94,
                pinned_lost: 1,
                ..EvictOutcome::default()
            }
            .reconciles(),
            "a pass that evicted the build's own item has not finished, however much it moved"
        );
    }

    /// The threshold counts everything the character still owns, not just the shelf.
    #[test]
    fn a_copy_is_redundant_only_when_two_survive_it() {
        assert!(is_redundant(2));
        assert!(!is_redundant(1));
        assert!(!is_redundant(0));
    }

    /// The count the threshold reads is taken from the plan, over every entry the plan keeps, so
    /// it cannot miss an update the way an accumulated total can.
    #[test]
    fn the_retained_count_comes_from_the_plan_not_from_the_loop() {
        let shield = 0x2000_0BB8;
        let held = [
            Held::one(1, shield),
            Held::one(2, shield),
            Held::one(3, shield),
        ];
        let allowance = Allowance::new(&[grant_of(shield, 2, &[])], []);
        let plan = plan_sweep(&held, &allowance);
        // Two are kept, so the third is judged against a count of two before anything moves --
        // which is the answer the accumulated version only reached after the fact, or not at all.
        assert_eq!(retained_after(&held, &plan).get(&shield), Some(&2));
    }

    /// The engine's empty-slot rows are not gear the pass keeps, and are not gear it moves.
    #[test]
    fn the_empty_slot_rows_are_outside_the_pass_rather_than_kept() {
        let mut entries = vec![entry(0, 1, UNARMED_ITEM_ID, 1)];
        for (n, id) in EMPTY_PROTECTOR_ITEM_IDS.iter().enumerate() {
            entries.push(entry(n as i32 + 1, n as u32 + 2, *id, 1));
        }
        let held: Vec<Held> = entries.iter().map(as_held).collect();
        let plan = plan_sweep(&held, &Allowance::default());
        assert_eq!(plan.counts().untouchable, 5);
        assert_eq!(plan.counts().kept(), 0);
        assert_eq!(plan.counts().surplus, 0);
    }

    /// Goods, gems and spells never reach this pass.
    #[test]
    fn goods_gems_and_spells_are_outside_this_pass() {
        let entries = [
            entry(0, 1, 0x4000_0084, 99),
            entry(1, 2, 0x8000_2AF8, 1),
            entry(2, 3, 0x4000_00C8, 3),
        ];
        let held: Vec<Held> = entries.iter().map(as_held).collect();
        assert!(survivors(&held, &Allowance::default()).is_empty());
    }

    /// A build naming one shield keeps one shield, whatever the upgrade level or infusion.
    #[test]
    fn an_upgraded_armament_spends_the_allowance_the_plan_wrote_at_plus_zero() {
        let bare = 1_010_000;
        let entries = [entry(0, 9, bare + 1_200 + 25, 1), entry(1, 0, bare, 1)];
        let held: Vec<Held> = entries.iter().map(as_held).collect();
        let allowance = Allowance::new(&[grant_of(bare, 1, &[])], [9]);
        let plan = plan_sweep(&held, &allowance);
        assert_eq!(plan.disposition(0), Some(Disposition::Kept(Kept::Pinned)));
        assert!(plan.disposition(1).expect("classified").is_surplus());
    }

    /// The shelf walk sums quantities per identity and ignores a freed entry.
    #[test]
    fn the_shelf_sums_quantities_per_item() {
        let counts = shelf_counts(vec![
            entry(0, 1, 0x0300_20A0, 10),
            entry(1, 2, 0x0300_20A0, 20),
            entry(2, 3, 0x2000_0BB8, 0),
        ]);
        assert_eq!(counts.get(&identity(0x0300_20A0)), Some(&30));
        assert_eq!(counts.get(&0x2000_0BB8), None);
    }

    /// Only a box that is out of space justifies destroying a copy.
    #[test]
    fn a_refusal_about_the_item_never_destroys_it() {
        assert!(Refusal::BoxHasNoRoom.is_the_box_being_full());
        assert!(Refusal::StackAtMaximum.is_the_box_being_full());
        assert!(!Refusal::Worn.is_the_box_being_full());
        assert!(!Refusal::WrongKind.is_the_box_being_full());
    }
}
