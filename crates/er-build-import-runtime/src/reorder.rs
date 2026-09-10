//! Making the character's inventory come out in the build's order, for items they already own.
//!
//! # The case this exists for
//!
//! A build imported twice is usually not a build being installed twice. It is the same items with
//! something moved, or with one thing swapped out -- the player edits the page and re-imports.
//! Everything the character already holds is left alone by the grant, which is right: reconciling
//! to a target rather than adding to a pile is what stopped the importer minting a second Flask of
//! Wondrous Physick. But it means nothing about those items changes, and the one thing the player
//! looks at to check the import worked -- the inventory list, next to the planner page -- keeps
//! whatever order it had before.
//!
//! `plan::plan` already emits its grants in the build's own order, and the comment there says
//! plainly that this is "the order the player sees in their inventory and the only order they can
//! check against the planner page". That is true only for items being acquired for the first time.
//! For an item already held, the acquisition order was decided whenever the player picked it up,
//! and no amount of granting in the right order changes it.
//!
//! # What changes it
//!
//! `EquipInventoryData.nextSortId` is a counter, and `CS::EquipInventoryData::InsertItem` stamps
//! `entry.sortId` from it and increments on every insert. So the order is the order things
//! arrived, and an item arrives when it enters the inventory -- including when it comes back out
//! of the storage box.
//!
//! The obvious way to earn a new one is to make the game acquire the item again: deposit the
//! stack into the storage box and take it straight back. That is what this pass did until
//! 2026-09-10, and it fails completely on a full box -- a deposit needs a free entry, and
//! `GetAddOrRemoveAmount` answers zero for a non-stackable when there is none. Measured on a box
//! at `1920 of 1920`: 6 items of 137 re-acquired, 131 declined with "the box would not take it",
//! and the inventory kept whatever order it had.
//!
//! So the counter is used directly. Walking the build's items in the build's order and stamping
//! each entry from `nextSortId` is exactly what `InsertItem` does, minus the two transfers -- and
//! it cannot strand an item in the box, because no item moves. The counter is left past the last
//! value written, so anything the player picks up afterwards still sorts above the build.
//!
//! # Why nothing else has to be moved out of the way
//!
//! The obvious alternative is to evict the character's other items so the build's items are not
//! interleaved with them. This does not need that, and deliberately does not do it: after the
//! walk, every item the build names sits above every item it does not, so there is nothing left to
//! interleave. A player's crafting materials, keys and souvenirs stay in their pockets, which is
//! where they want them -- exiling them to the box to tidy a sort order would be a much larger
//! change to the character than the import itself.
//!
//! # What it will not do
//!
//! An entry whose store fails is reported by name rather than counted, and nothing else can go
//! wrong: there is no transfer to be half-completed and no copy to be confused with another.

use er_build_import_core::plan::Grant;
use er_game_base::rva::GET_EQUIP_INVENTORY_DATA_RVA;

use crate::storage::Storage;

/// `CS::EquipGameData::GetEquipInventoryData(egd) -> EquipInventoryData*`.
type GetInventoryFn = unsafe extern "system" fn(usize) -> usize;

/// What one reorder pass did.
#[derive(Debug, Default)]
pub struct ReorderOutcome {
    /// Distinct items the build names that the character holds.
    pub attempted: usize,
    /// Items the game re-acquired, proved by a strictly larger `sortId`.
    pub restamped: usize,
    /// Items that were worn and had to be taken off first, so they could be moved at all.
    pub unequipped: usize,
    /// `(label, why)` for each item that was not moved.
    pub declined: Vec<(String, &'static str)>,
    /// `(label, deposited, retrieved)` for any item that did not come all the way back.
    ///
    /// Always empty in a healthy run, and the one failure here that costs the player something
    /// rather than merely leaving a list in the wrong order.
    pub stranded: Vec<(String, i32, i32)>,
    /// Whether the final read-back found the build's items in the build's order.
    pub in_order: bool,
    /// How many items that verdict covers.
    pub order_checked: usize,
    /// Why nothing was attempted, when that is the answer.
    pub unavailable: Option<&'static str>,
    /// The inventory was already in the build's order, so nothing was moved.
    ///
    /// The common case for a first import onto a character who owned none of it: the grant walks
    /// the build in order, so the items arrive in order and there is nothing to correct. Reported
    /// rather than folded into a zero, because "nothing needed doing" and "nothing could be done"
    /// are the same numbers and opposite facts.
    pub already_in_order: bool,
}

impl ReorderOutcome {
    /// One line for the import log.
    pub fn summary(&self) -> String {
        if let Some(why) = self.unavailable {
            return format!(
                "REORDER: the inventory keeps whatever order it had -- {why}. Items the character \
                 already owned will not sit where the build lists them"
            );
        }
        if self.already_in_order {
            return format!(
                "REORDER: nothing to do -- the {} item(s) the build names are already in the \
                 build's order",
                self.order_checked
            );
        }
        format!(
            "REORDER: {}/{} item(s) re-acquired into the build's order ({} had to be taken off \
             first, {} declined, {} stranded in the box); the {} item(s) that could be read back \
             are {}",
            self.restamped,
            self.attempted,
            self.unequipped,
            self.declined.len(),
            self.stranded.len(),
            self.order_checked,
            if self.in_order {
                "in the build's order"
            } else {
                "NOT in the build's order"
            }
        )
    }
}

/// Re-acquire everything the build names, in the build's order.
///
/// Runs between the grant and the equip, and both sides of that are load-bearing:
///
/// * after the grant, because an item that is not held yet cannot be reordered. Items the grant
///   has just minted are moved along with the rest and not skipped: the grant walks the build in
///   order, so those are in order among themselves, but every item the character already owned
///   carries an older `sortId` and would sort ahead of all of them regardless of where the build
///   lists it. Only moving everything puts the two groups on one scale;
/// * before the equip, because a worn entry cannot be deposited. The pass takes items off to move
///   them and does not put them back; the equip pass, which is about to write every position the
///   build names anyway, is what dresses the character afterwards.
///
/// # Safety
///
/// Game thread, character in the world, grants already applied, `egd` a live `EquipGameData*`.
/// The carried inventory is resolved from it here rather than passed in, so no caller can hand
/// this pass an `EquipInventoryData*` belonging to a different `EquipGameData`.
pub unsafe fn apply_build_order(
    module_base: usize,
    egd: usize,
    grants: &[Grant],
) -> ReorderOutcome {
    let mut outcome = ReorderOutcome::default();

    let Some(get_inventory) = crate::native::resolve(
        module_base,
        GET_EQUIP_INVENTORY_DATA_RVA,
        "CS::EquipGameData::GetEquipInventoryData",
    ) else {
        outcome.unavailable =
            Some("`GetEquipInventoryData` has no verified mapping for the running build");
        return outcome;
    };
    // Safety: resolved for the running build on the line above.
    let get_inventory: GetInventoryFn = unsafe { core::mem::transmute(get_inventory) };
    // Safety: game thread, `egd` live; the getter reads one field.
    let carried = unsafe { get_inventory(egd) };
    if carried == 0 {
        outcome.unavailable = Some("the carried inventory is null");
        return outcome;
    }

    // Safety: delegated -- `open` does its own singleton null checks and resolves every native it
    // needs before calling any of them.
    let Some(storage) = (unsafe { Storage::open(module_base, egd, carried) }) else {
        outcome.unavailable = Some("the storage box is unreachable this session");
        return outcome;
    };

    // One entry per distinct item id, in the order the build lists it. The build can name the same
    // id twice -- two copies of an armament that differ only by their ash, a consumable that
    // appears in both the tools list and the quickbar -- and recycling it twice would move it out
    // of the order its first appearance earned.
    let mut ordered: Vec<&Grant> = Vec::with_capacity(grants.len());
    for grant in grants {
        if !ordered.iter().any(|seen| seen.item_id == grant.item_id) {
            ordered.push(grant);
        }
    }

    // Ask before moving anything. A first import onto a character who owned none of the build
    // leaves it already in order -- the grant walks the build in order and the items arrive in
    // that order -- and this pass would otherwise put every one of them through the storage box
    // to arrive at the arrangement they were already in. Two transfers per item, for nothing.
    // Safety: game thread, read only.
    let (already, checked) = unsafe { verify_order(&storage, &ordered) };
    if already {
        outcome.already_in_order = true;
        outcome.in_order = true;
        outcome.order_checked = checked;
        return outcome;
    }

    // The counter the game itself stamps from, so everything this pass writes sits above every
    // acquisition the character already had and below everything they pick up afterwards.
    // Safety: game thread, read only.
    let Some(mut next) = (unsafe { storage.next_sort_id() }) else {
        outcome.unavailable = Some("the inventory's acquisition counter could not be read");
        return outcome;
    };

    for grant in &ordered {
        // Safety: game thread, read only.
        if unsafe { storage.carried_quantity(grant.item_id) } <= 0 {
            continue;
        }
        outcome.attempted += 1;

        // Safety: game thread, read only.
        let index = unsafe { storage.carried_index(grant.item_id) };
        // Safety: game thread; a store of one int into a live entry, fault-checked.
        if unsafe { storage.restamp(index, next) } {
            outcome.restamped += 1;
            next = next.saturating_add(1);
        } else {
            outcome.declined.push((
                grant.label.clone(),
                "its inventory entry could not be stamped with a new acquisition order",
            ));
        }
    }

    // Safety: game thread; a store of one int into a live inventory, fault-checked.
    unsafe { storage.set_next_sort_id(next) };

    // Safety: game thread, read only.
    let (in_order, checked) = unsafe { verify_order(&storage, &ordered) };
    outcome.in_order = in_order;
    outcome.order_checked = checked;
    outcome
}

/// Whether the build's items now read back in the build's order, and over how many of them.
///
/// The measurement the pass is for. Counting successful round trips would prove that items moved;
/// this proves what moving them was supposed to achieve, which is a different claim and the only
/// one worth printing. An item that could not be read is left out of both numbers rather than
/// counted as agreeing.
///
/// # Safety
///
/// Game thread.
unsafe fn verify_order(storage: &Storage, ordered: &[&Grant]) -> (bool, usize) {
    let mut previous: Option<i32> = None;
    let mut checked = 0usize;
    let mut in_order = true;
    for grant in ordered {
        // Safety: game thread, read only.
        let Some(sort_id) = (unsafe { storage.carried_sort_id(grant.item_id) }) else {
            continue;
        };
        checked += 1;
        if let Some(previous) = previous
            && sort_id <= previous
        {
            in_order = false;
        }
        previous = Some(sort_id);
    }
    (in_order, checked)
}
