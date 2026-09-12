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

use std::collections::BTreeMap;

use er_build_import_core::plan::Grant;
use er_build_import_core::sweep::identity;
use er_game_base::rva::GET_EQUIP_INVENTORY_DATA_RVA;

use crate::storage::Storage;

/// `CS::EquipGameData::GetEquipInventoryData(egd) -> EquipInventoryData*`.
type GetInventoryFn = unsafe extern "system" fn(usize) -> usize;

/// The inventory index holding each item the build names, keyed by [`identity`].
///
/// # Why this is not `Storage::carried_index(grant.item_id)`
///
/// Because that question is asked in the wrong vocabulary, and the wrong answer is silent.
/// `Grant::item_id` for an armament is the base row plus its affinity and nothing else: the
/// upgrade level is not in it, because the grant pass computes the level separately and mints at
/// `armament_item_id(grant.item_id, level)`. So a character holding a Bone Bow at +25 holds item
/// `40500025` while the grant that names it says `40500000`, and an inventory lookup for the
/// grant's id finds nothing at all.
///
/// The old loop answered that with `continue` -- not counted as attempted, not counted as
/// declined, and skipped again by the order check, so the pass reported `160/160 ... in the
/// build's order` over the subset that happened to resolve. Reported 2026-09-11: of 45 armaments
/// in one build, 44 came out in the build's order and the Bone Bow sat at the very front of the
/// inventory, below items the pass had never touched. It was the one the lookup missed.
///
/// [`identity`] is the rule the sweep already uses for "these two are the same item to a player",
/// so folding the affinity and the level away here makes both passes agree about what they are
/// looking at. One walk of the inventory also replaces one native call per item.
fn entries_by_identity(storage: &Storage) -> BTreeMap<u32, i32> {
    let mut out: BTreeMap<u32, i32> = BTreeMap::new();
    // Safety: game thread, read only -- the caller's contract covers the walk.
    for entry in unsafe { storage.carried_entries() } {
        if entry.quantity <= 0 {
            continue;
        }
        // The lowest index wins, which is the copy `GetItemInventoryIdx` would have named, so a
        // build naming one of several copies still reorders the one every other pass acts on.
        out.entry(identity(entry.item_id))
            .and_modify(|index| *index = (*index).min(entry.index))
            .or_insert(entry.index);
    }
    out
}

/// The inventory index of the entry satisfying `grant`, under any id the game files it as.
///
/// The grant's own id first, then its alternates, because a name resolving to several rows is the
/// other way the character can hold the item under a number the plan does not mention.
fn index_for(held: &BTreeMap<u32, i32>, grant: &Grant) -> Option<i32> {
    std::iter::once(grant.item_id)
        .chain(grant.also_known_as.iter().copied())
        .find_map(|id| held.get(&identity(id)).copied())
}

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
    /// Items the build names that no inventory entry could be found for, by label.
    ///
    /// Distinct from [`Self::declined`], which is "tried and failed". This is "never tried", and
    /// it used to be a bare `continue`: not attempted, not declined, and skipped again by the
    /// order check, so the pass scored itself over the items it happened to resolve and printed
    /// `160/160 ... in the build's order` while one of them sat at the front of the inventory.
    pub not_held: Vec<String>,
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
             first, {} declined, {} stranded in the box, {} the character does not hold under any \
             id{}); the {} item(s) that could be read back are {}",
            self.restamped,
            self.attempted,
            self.unequipped,
            self.declined.len(),
            self.stranded.len(),
            self.not_held.len(),
            if self.not_held.is_empty() {
                String::new()
            } else {
                format!(
                    " -- those are NOT in the count above and keep whatever order they had: {}",
                    self.not_held.join(", ")
                )
            },
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

    // One walk, and every lookup below comes out of it. See `entries_by_identity` for why asking
    // the inventory about `grant.item_id` is the wrong question.
    let held = entries_by_identity(&storage);

    for grant in &ordered {
        let Some(index) = index_for(&held, grant) else {
            // The build names it and the character does not hold it, under any id it could be
            // filed as. Counted and named rather than skipped: an item that leaves the loop
            // without an outcome is one the order check will skip too, and the pass then prints a
            // perfect score over the items it happened to find.
            outcome.not_held.push(grant.label.clone());
            continue;
        };
        outcome.attempted += 1;

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
    let held = entries_by_identity(storage);
    let mut previous: Option<i32> = None;
    let mut checked = 0usize;
    let mut in_order = true;
    for grant in ordered {
        // The same resolution the stamping loop uses. When these two disagreed, the loop skipped
        // an item and so did this, and the pass reported the order of what was left as the order
        // of the whole build.
        let Some(index) = index_for(&held, grant) else {
            continue;
        };
        // Safety: game thread, read only.
        let Some(sort_id) = (unsafe { storage.sort_id_at(index) }) else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use er_build_import_core::plan::NO_SKILL;

    fn grant_of(item_id: u32, label: &str, also: &[u32]) -> Grant {
        Grant {
            item_id,
            also_known_as: also.to_vec(),
            quantity: 1,
            reinforce_lv: 0,
            upgrade_is_character_default: true,
            weapon_skill: NO_SKILL,
            label: label.to_owned(),
            pot_group: None,
            armament: true,
        }
    }

    /// The Bone Bow, measured 2026-09-11.
    ///
    /// The grant names `40500000` -- base row, no affinity, no level, because the grant pass
    /// computes the level separately. The character held `40500025`. An inventory lookup for the
    /// grant's own id finds nothing, and the pass skipped it in silence while reporting every
    /// other armament in the build as correctly ordered.
    #[test]
    fn an_armament_is_found_at_the_level_the_character_holds_it() {
        let bone_bow_base = 40_500_000;
        let held: BTreeMap<u32, i32> = [(identity(40_500_025), 2256)].into_iter().collect();
        let grant = grant_of(bone_bow_base, "Bone Bow", &[]);
        assert_eq!(index_for(&held, &grant), Some(2256));
        // Why the old pass missed it. It asked the game, whose `GetItemIndex` matches an exact
        // item id, and the two ids are not equal -- the character's entry is filed under
        // `40500025` and the grant says `40500000`. Folding them through `identity` is the whole
        // of the fix, and these two lines are the before and after of that fold.
        assert_ne!(40_500_025, bone_bow_base);
        assert_eq!(identity(40_500_025), bone_bow_base);
    }

    /// An affinity is folded away too, for the same reason and by the same rule.
    #[test]
    fn an_infused_armament_is_the_same_item_as_the_plain_one() {
        let longsword = 1_010_000;
        let occult_25 = longsword + 1_200 + 25;
        let held: BTreeMap<u32, i32> = [(identity(occult_25), 7)].into_iter().collect();
        assert_eq!(
            index_for(&held, &grant_of(longsword, "Longsword", &[])),
            Some(7)
        );
    }

    /// A name that resolves to several rows is the other way the character holds an item under a
    /// number the plan does not name.
    #[test]
    fn an_alternate_row_satisfies_the_grant_that_names_it() {
        let named = 0x2000_0BB8;
        let alternate = 0x2000_0BB9;
        let held: BTreeMap<u32, i32> = [(identity(alternate), 11)].into_iter().collect();
        assert_eq!(
            index_for(&held, &grant_of(named, "Talisman", &[alternate])),
            Some(11)
        );
        assert_eq!(index_for(&held, &grant_of(named, "Talisman", &[])), None);
    }

    /// An item the character genuinely does not hold is reported, not skipped.
    #[test]
    fn an_item_the_character_does_not_hold_has_no_index() {
        let held: BTreeMap<u32, i32> = BTreeMap::new();
        assert_eq!(
            index_for(&held, &grant_of(1_010_000, "Longsword", &[])),
            None
        );
    }

    /// The summary names what it could not find, and says those are outside its own score.
    #[test]
    fn the_summary_names_what_it_never_tried() {
        let outcome = ReorderOutcome {
            attempted: 159,
            restamped: 159,
            order_checked: 159,
            in_order: true,
            not_held: vec!["Bone Bow".to_owned()],
            ..ReorderOutcome::default()
        };
        let summary = outcome.summary();
        assert!(summary.contains("Bone Bow"), "{summary}");
        assert!(summary.contains("NOT in the count above"), "{summary}");
    }
}
