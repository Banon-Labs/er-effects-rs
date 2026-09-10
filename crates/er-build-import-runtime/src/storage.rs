//! Moving items between the player's pockets and their storage box, through the game's own calls.
//!
//! # Why the importer needs the storage box at all
//!
//! Two reasons, and only the second one is about pots.
//!
//! **The build is a statement about the character, not about the backpack.** A grant reconciles
//! to a target -- "this character has three Fire Pots" -- by measuring what is held and adding the
//! shortfall. Until now "what is held" meant the carried inventory alone, so an item sitting in
//! the player's storage box was invisible and a second copy was minted beside it. Asking the box
//! first, and pulling from it when it has one, is the same reconcile with the whole inventory in
//! view.
//!
//! **The box has no pot cap, and the pockets do.** A consumable with an `EquipParamGoods`
//! `potGroupId >= 0` can be carried only up to the number of Cracked Pots sharing that group (see
//! [`crate::catalog::PotGroups`]). The box is exempt, and the exemption is a construction flag
//! rather than a special case in the transfer code:
//! `EquipInventoryData::EquipInventoryData(this, size, keySize, limitedPots, unlimitedConsumables)`
//! (1.16.2 `0x14024bbf0`) has exactly two callers -- the `EquipGameData` constructor at
//! `0x140245485` passes `unlimitedConsumables = 0`, the `PlayerGameData` constructor at
//! `0x14025d879` passes `1`. That flag makes `UpdatePotsStates` return immediately and routes
//! `GetMaxAmountForItem` to `GetMaxItemCountForUnlimitedConsumables` (`0x1406748c0`, 1.17
//! `0x140675710`), which answers `EquipParamGoods.maxRepositoryNum` instead of the group's
//! remaining headroom.
//!
//! So depositing a pot the build does not want DECREMENTS the carried `potItemsCount` for its
//! group and raises the ceiling for the one the build does want. Nothing is destroyed: the
//! displaced pot is in the box, where the player can take it back.
//!
//! # Resolve everything, then act
//!
//! Every native this module needs is resolved in [`Storage::open`], before a single one runs. A
//! transfer that gets half way -- source decremented, destination never credited -- is worse than
//! a transfer that never started, and "the fifth address had no mapping for this build" is
//! exactly the way that happens on a patched game.
//!
//! # The two ways this corrupts a save if it is written carelessly
//!
//! 1. **`reassignQuickSlot` is directional.** When set, the tail of
//!    `TransferItemBetweenInventoryDatas` writes the destination index into the main player's
//!    quick-slot table. Setting it while depositing points the player's quickbar at a
//!    storage-box index, and that dangling reference persists into the save. It is `false` for
//!    [`Storage::deposit`] and `true` for [`Storage::pull`], and the two are separate functions
//!    partly so the flag cannot be passed by a caller who has to remember which way it goes.
//! 2. **An index is only valid until the next transfer.** The call ends in `AdjustQuantityBy` and
//!    then `RemoveItem` once the stack empties, which reindexes the source inventory. Every
//!    method here re-resolves `GetItemInventoryIdx` immediately before its own transfer and never
//!    accepts an index from a caller.
//!
//! And one that does not corrupt a save but does lose an item's slot: an equipped entry is never
//! deposited. `EquipGameData.equipmentItemIdxList` (`+0x8`, `int[22]`) holds inventory indices,
//! so removing an entry a ChrAsm slot still names leaves that slot pointing at a shifted or freed
//! one. `Storage::is_equipped_index` is the same scan `er-better-refills` runs before its own
//! deposit.

use er_game_base::rva::{
    ADJUST_QUANTITY_BY_RVA, CHANGE_AMOUNT_IN_BOX_RVA, CS_MENU_MAN_GLOBAL_RVA,
    EQUIP_GAME_DATA_REMOVE_ITEM_RVA, GAME_DATA_MAN_GLOBAL_RVA, GET_ADD_OR_REMOVE_AMOUNT_RVA,
    GET_INVENTORY_ITEM_ENTRY_BY_INDEX_RVA, GET_ITEM_INVENTORY_IDX_RVA,
    GET_MAIN_PLAYER_STORAGE_BOX_INVENTORY_RVA, GET_QUANTITY_BY_ITEM_ID_RVA,
    INVENTORY_ITEM_ENTRY_SORT_ID_OFFSET, TRANSFER_ITEM_BETWEEN_INVENTORY_DATAS_RVA,
    UPDATE_TROPHY_STATS_RVA,
};

/// `GameDataMan::main_player_game_data`, read as a raw pointer rather than the typed `OwnedPtr`
/// upstream declares, because before a character is loaded the slot really is null.
///
/// Measured, not taken from that declaration. `CS::GameDataMan::GetMainPlayerGameData`
/// (`0x140e9fc30`) is a twelve-byte leaf whose whole body is
/// `mov rax,[rip+GLOBAL_GameDataMan] ; mov rax,[rax+0x8] ; ret` -- the field's identity in one
/// instruction. It has not moved: `CS::GameData::GameData(GameDataMan*)`, the constructor
/// (`0x140254680`, 1.17 `0x140254650`), aligns 351/351 instructions across the two de-Arxan'd
/// images with 45 `this`-relative offsets and zero moved, `0x8` among them; `~GameDataMan`
/// (`0x140254d40`, 1.17 `0x140254d10`) aligns 359/359 and agrees. Re-measured every run by
/// `scripts/check-object-field-offsets-1170.py`.
///
/// Same value and same reason as `grant::GAME_DATA_MAN_PLAYER_OFFSET`; it is repeated here rather
/// than shared because this module's use of it is a NULL check on the engine's behalf --
/// `GetMainPlayerStorageBoxInventory` dereferences the slot without checking it -- not a walk to
/// `EquipGameData`.
const GAME_DATA_MAN_PLAYER_OFFSET: usize = 0x08;

/// `EquipGameData.equipmentItemIdxList: int[22]` -- Inventory indices of the worn loadout.
///
/// Confirmed in the 1.16.2 dump's `EquipGameData` structure (`equipmentItemIdxList int[22]` at
/// offset 8, in a 0x4b0-byte object). `er-better-refills` has shipped the same constant since its
/// deposit-back path landed.
const EQUIPMENT_ITEM_IDX_LIST_OFFSET: usize = 0x8;
/// Length of the list above.
const EQUIPMENT_ITEM_IDX_LIST_LEN: usize = 22;

type GetQuantityFn = unsafe extern "system" fn(usize, *mut i32) -> i32;
type GetItemIdxFn = unsafe extern "system" fn(usize, *mut i32) -> i32;
type ChangeAmountInBoxFn = unsafe extern "system" fn(usize, *mut i32, i32) -> i32;
type TransferFn = unsafe extern "system" fn(i32, usize, usize, i32, bool) -> bool;
type UpdateTrophyStatsFn = unsafe extern "system" fn(usize, *mut i32);
type GetAddOrRemoveAmountFn = unsafe extern "system" fn(usize, *mut u32, i32) -> i32;
type GetStorageInventoryFn = unsafe extern "system" fn() -> usize;
type GetEntryFn = unsafe extern "system" fn(usize, u32) -> usize;
type RemoveItemFn = unsafe extern "system" fn(usize, i32, u32, bool) -> bool;
type AdjustQuantityFn = unsafe extern "system" fn(usize, u32, i32, *mut i32) -> u32;

/// What one [`Storage::recycle`] did, measured by reading the entry back.
///
/// Every field is a measurement rather than a count of calls: `deposited` and `retrieved` are
/// carried-quantity differences taken either side of a transfer, and the two sort ids are read
/// out of the inventory entry itself.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Recycled {
    /// How many were carried before any of this.
    pub held_before: i32,
    /// How many reached the box.
    pub deposited: i32,
    /// How many came back.
    pub retrieved: i32,
    /// The acquisition order before, when the entry could be read.
    pub sort_id_before: Option<i32>,
    /// The acquisition order after.
    pub sort_id_after: Option<i32>,
    /// Why nothing was moved, when that is the answer. `None` means the round trip was attempted.
    pub declined: Option<&'static str>,
}

impl Recycled {
    /// Whether the game re-acquired the item, proved by a strictly larger acquisition order.
    ///
    /// Not `deposited == retrieved`: an item can make the whole round trip and keep its old order
    /// when the deposit was partial, because the retrieve then merges into the stack still in the
    /// pockets through `AdjustQuantityBy`, which re-stamps nothing.
    pub fn restamped(&self) -> bool {
        matches!(
            (self.sort_id_before, self.sort_id_after),
            (Some(before), Some(after)) if after > before
        )
    }

    /// Whether the player ends up holding as many as they started with.
    ///
    /// The one property a reorder must never break. A false here is an item left in the box.
    pub fn count_preserved(&self) -> bool {
        self.declined.is_some() || self.deposited == self.retrieved
    }
}

/// The player's two inventories and the calls that move items between them.
///
/// Constructed only by [`Storage::open`], which is where the "resolve everything first" rule and
/// the singleton null checks live.
pub struct Storage {
    /// `EquipGameData*` -- the trophy update and the equipped-index scan both need it.
    egd: usize,
    /// The carried `EquipInventoryData*` (`egd + 0x158`, via `GetEquipInventoryData`).
    carried: usize,
    /// The storage box `EquipInventoryData*` (`PlayerGameData + 0x8d0`).
    box_inventory: usize,
    get_quantity: GetQuantityFn,
    get_item_idx: GetItemIdxFn,
    change_amount_in_box: ChangeAmountInBoxFn,
    transfer: TransferFn,
    update_trophy_stats: UpdateTrophyStatsFn,
    get_add_or_remove_amount: GetAddOrRemoveAmountFn,
    /// Reads the entry itself, so [`Storage::carried_sort_id`] can prove a re-acquisition landed.
    get_entry: GetEntryFn,
    /// The discard pair, resolved together and `None` unless both resolved.
    ///
    /// One `Option` for the two because a partial discard needs both -- decrement, and then remove
    /// the entry if it emptied -- and half a discard leaves a zero-quantity entry that the
    /// quickbar may still name. This is the only field in this module whose calls destroy
    /// something, which is why it is also the only one allowed to be absent without the whole
    /// `Storage` refusing to open: everything else here degrades to "the item was not moved",
    /// and losing the ability to destroy an item is not a degradation worth refusing over.
    discard: Option<DiscardNatives>,
}

/// The two calls a discard needs. See [`Storage::discard`].
#[derive(Clone, Copy)]
struct DiscardNatives {
    remove_item: RemoveItemFn,
    adjust_quantity: AdjustQuantityFn,
}

/// Resolve the discard pair, or `None` if either half has no mapping for the running build.
///
/// Separate from `Storage::open`'s all-or-nothing group on purpose. That group is all-or-nothing
/// because a half-finished transfer loses an item; this pair is optional because its absence
/// costs only the ability to destroy one, which is a rung the importer can simply not climb.
fn discard_natives(module_base: usize) -> Option<DiscardNatives> {
    let [remove_item, adjust_quantity] = crate::native::resolve_all(
        module_base,
        [
            (
                EQUIP_GAME_DATA_REMOVE_ITEM_RVA,
                "CS::EquipGameData::RemoveItem",
            ),
            (
                ADJUST_QUANTITY_BY_RVA,
                "CS::EquipInventoryData::AdjustQuantityBy",
            ),
        ],
    )
    .ok()?;
    // Safety: both addresses were resolved for the running build immediately above.
    Some(unsafe {
        DiscardNatives {
            remove_item: core::mem::transmute::<usize, RemoveItemFn>(remove_item),
            adjust_quantity: core::mem::transmute::<usize, AdjustQuantityFn>(adjust_quantity),
        }
    })
}

impl Storage {
    /// Whether this session can destroy an item at all. See [`Storage::discard`].
    pub fn can_discard(&self) -> bool {
        self.discard.is_some()
    }

    /// Resolve every native and both inventory pointers, or refuse.
    ///
    /// `None` means the storage rungs are inert for this session and the caller should say so
    /// rather than guessing -- it does not mean the grant cannot proceed.
    ///
    /// # Two DLPanics this rules out before calling, rather than after
    ///
    /// `GetMainPlayerStorageBoxInventory` reads `GLOBAL_CSMenuMan` and takes the FD4Singleton
    /// `DLPanic` path when it is null, which does not return. It then reads
    /// `GLOBAL_GameDataMan->mainPlayerGameData->storageInventory` with no null check on the
    /// middle pointer. Both are checked here. The importer runs on the game thread after a
    /// character is in the world, so both should hold -- "should" is why they are checked.
    ///
    /// # Safety
    ///
    /// Game thread, `egd` a live `EquipGameData*`, `carried` the live carried
    /// `EquipInventoryData*`, `module_base` the loaded image base.
    pub unsafe fn open(module_base: usize, egd: usize, carried: usize) -> Option<Self> {
        if egd == 0 || carried == 0 {
            return None;
        }

        // All seven before any of them runs. This module moves items between two inventories, and
        // a half-finished move is worse than none: the source has already been decremented.
        let resolved = crate::native::resolve_all(
            module_base,
            [
                (
                    GET_MAIN_PLAYER_STORAGE_BOX_INVENTORY_RVA,
                    "GetMainPlayerStorageBoxInventory",
                ),
                (GET_QUANTITY_BY_ITEM_ID_RVA, "GetQuantityByItemId"),
                (GET_ITEM_INVENTORY_IDX_RVA, "GetItemInventoryIdx"),
                (
                    CHANGE_AMOUNT_IN_BOX_RVA,
                    "EquipInventoryData::ChangeAmountInBox",
                ),
                (
                    TRANSFER_ITEM_BETWEEN_INVENTORY_DATAS_RVA,
                    "TransferItemBetweenInventoryDatas",
                ),
                (
                    UPDATE_TROPHY_STATS_RVA,
                    "CS::EquipGameData::UpdateTrophyStats",
                ),
                (
                    GET_ADD_OR_REMOVE_AMOUNT_RVA,
                    "EquipInventoryDat::GetAddOrRemoveAmount",
                ),
                (
                    GET_INVENTORY_ITEM_ENTRY_BY_INDEX_RVA,
                    "EquipInventoryData::GetInventoryItemEntryByIndex",
                ),
            ],
        );
        let Ok(
            [
                get_box,
                get_quantity,
                get_item_idx,
                change_amount_in_box,
                transfer,
                update_trophy_stats,
                get_add_or_remove_amount,
                get_entry,
            ],
        ) = resolved
        else {
            return None;
        };

        // Safety: a fault-checked read of one pointer-sized slot in the loaded image. A null here
        // is the DLPanic the getter would take, so it is a refusal rather than a call.
        let menu_man = er_game_base::mem::read_global_ptr(
            module_base,
            CS_MENU_MAN_GLOBAL_RVA,
            "CS_MENU_MAN_GLOBAL_RVA",
        );
        if menu_man == 0 {
            crate::log_line(
                "[build-import] storage box unavailable: CSMenuMan is null, and \
                 GetMainPlayerStorageBoxInventory DLPanics on that -- not calling it",
            );
            return None;
        }
        let game_data_man = er_game_base::mem::read_global_ptr(
            module_base,
            GAME_DATA_MAN_GLOBAL_RVA,
            "GAME_DATA_MAN_GLOBAL_RVA",
        );
        // Safety: one fault-checked pointer read at a verified offset inside a live singleton.
        let player_game_data = if game_data_man == 0 {
            0
        } else {
            unsafe {
                er_game_base::mem::safe_read_usize(game_data_man + GAME_DATA_MAN_PLAYER_OFFSET)
            }
            .unwrap_or(0)
        };
        if player_game_data == 0 {
            crate::log_line(
                "[build-import] storage box unavailable: no PlayerGameData, which \
                 GetMainPlayerStorageBoxInventory dereferences without checking",
            );
            return None;
        }

        // Safety: resolved for the running build immediately above, and both singletons the
        // function reads have just been proved non-null.
        let get_box: GetStorageInventoryFn = unsafe { core::mem::transmute(get_box) };
        // Safety: game thread, and the call reads only the two globals checked above.
        let box_inventory = unsafe { get_box() };
        if box_inventory == 0 {
            crate::log_line("[build-import] storage box unavailable: the box inventory is null");
            return None;
        }

        Some(Self {
            egd,
            carried,
            box_inventory,
            // Safety: every address below was resolved for the running build immediately above.
            get_quantity: unsafe { core::mem::transmute::<usize, GetQuantityFn>(get_quantity) },
            // Safety: as above.
            get_item_idx: unsafe { core::mem::transmute::<usize, GetItemIdxFn>(get_item_idx) },
            // Safety: as above.
            change_amount_in_box: unsafe {
                core::mem::transmute::<usize, ChangeAmountInBoxFn>(change_amount_in_box)
            },
            // Safety: as above.
            transfer: unsafe { core::mem::transmute::<usize, TransferFn>(transfer) },
            // Safety: as above.
            update_trophy_stats: unsafe {
                core::mem::transmute::<usize, UpdateTrophyStatsFn>(update_trophy_stats)
            },
            // Safety: as above.
            get_add_or_remove_amount: unsafe {
                core::mem::transmute::<usize, GetAddOrRemoveAmountFn>(get_add_or_remove_amount)
            },
            // Safety: as above.
            get_entry: unsafe { core::mem::transmute::<usize, GetEntryFn>(get_entry) },
            discard: discard_natives(module_base),
        })
    }

    /// How many of `item_id` the carried inventory would actually accept right now.
    ///
    /// This is the question a pot cap answers `0` to while every other signal says the add will
    /// work. `EquipInventoryDat::GetAddOrRemoveAmount` is a pure query -- it reads the entry and
    /// asks `HasSpaceForItem` / `GetMaxAmountForItem` / `GetMaxQuantityForItemEntry` -- so asking
    /// it costs nothing and is the only cheap way to see the clamp coming.
    ///
    /// # Safety
    ///
    /// Game thread.
    pub unsafe fn carried_headroom(&self, item_id: u32, wanted: i32) -> i32 {
        let mut id = item_id;
        // Safety: engine-owned inventory pointer, read only; the id outlives the call.
        unsafe { (self.get_add_or_remove_amount)(self.carried, &raw mut id, wanted) }
    }

    /// How many of `item_id` the carried inventory holds. Negative answers mean "cannot say".
    ///
    /// # Safety
    ///
    /// Game thread.
    pub unsafe fn carried_quantity(&self, item_id: u32) -> i32 {
        let mut id = item_id as i32;
        // Safety: engine-owned inventory pointer, read only.
        unsafe { (self.get_quantity)(self.carried, &raw mut id) }.max(0)
    }

    /// How many of `item_id` the storage box holds.
    ///
    /// The same `GetQuantityByItemId` the carried inventory uses: it takes an
    /// `EquipInventoryData*` and does not care which one.
    ///
    /// # Safety
    ///
    /// Game thread.
    pub unsafe fn stored_quantity(&self, item_id: u32) -> i32 {
        let mut id = item_id as i32;
        // Safety: engine-owned inventory pointer, read only.
        unsafe { (self.get_quantity)(self.box_inventory, &raw mut id) }.max(0)
    }

    /// Destroy up to `wanted` of `item_id`, and answer with how many really went.
    ///
    /// # The only thing in this module that cannot be undone
    ///
    /// Every other rung moves an item somewhere the player can walk to. This one deletes it, so
    /// the caller owes three things before it may be reached, and `grant.rs` is where they are
    /// checked rather than here: the item must be one the build does not ask for, the storage box
    /// must have already refused it, and it must be a pot-group member whose presence is the thing
    /// stopping the build's own pot from being carried. Outside that shape a discard is destroying
    /// a player's belongings to satisfy a preference.
    ///
    /// # Why the caller does not have to unequip first
    ///
    /// `CS::EquipGameData::RemoveItem` starts by asking `GetSlotIndexByItemIndex` which slot names
    /// the entry and calling the engine's own unequip on it, then clears the quickbar and pouch
    /// references, then removes the entry. That is what the inventory menu is expressing when it
    /// offers to discard ten of eleven Hefty Rock Pots: the eleventh is on the quickbar, and
    /// discarding it is a two-step the menu makes the player do and the native does for itself.
    ///
    /// # Whole entry versus part of one
    ///
    /// `RemoveItem`'s third argument is a flag, not a count -- it reaches
    /// `EquipInventoryData::RemoveItem`, which only tests it against zero -- so it always destroys
    /// the entire entry. A partial discard is therefore `AdjustQuantityBy(-n)`, which clamps at
    /// zero and answers with the new quantity; an entry that reaches zero is then removed properly
    /// so no quickbar slot is left naming a zero-quantity entry.
    ///
    /// # Safety
    ///
    /// Game thread.
    pub unsafe fn discard(&self, item_id: u32, wanted: i32) -> i32 {
        let Some(discard) = self.discard else {
            return 0;
        };
        if wanted <= 0 {
            return 0;
        }
        // Safety: game thread, read only.
        let before = unsafe { self.carried_quantity(item_id) };
        let wanted = wanted.min(before);
        if wanted <= 0 {
            return 0;
        }
        // Re-resolved immediately before the destructive call, for the same reason every other
        // method here re-resolves: an index is only valid until the next thing that reindexes.
        // Safety: game thread, read only.
        let index = unsafe { self.carried_index(item_id) };
        if index < 0 {
            return 0;
        }

        if wanted >= before {
            // The whole entry, through the call that also takes it off the character and off the
            // quickbar. `true` is the equip refresh the menu's own discard passes.
            // Safety: game thread, `egd` live, index resolved on the line above.
            unsafe { (discard.remove_item)(self.egd, index, 1, true) };
        } else {
            let mut clamped = 0i32;
            // Safety: game thread, engine-owned inventory, index resolved above; the out-parameter
            // is ours and outlives the call.
            let left = unsafe {
                (discard.adjust_quantity)(self.carried, index as u32, -wanted, &raw mut clamped)
            };
            if left == 0 {
                // A zero-quantity entry still exists and the quickbar may still name it. Finish
                // the job the way `TransferItemBetweenInventoryDatas` does when a move empties a
                // stack -- through the `EquipGameData` call, so the references go too.
                // Safety: as above; the index has not been invalidated by `AdjustQuantityBy`,
                // which changes a quantity and does not reindex.
                unsafe { (discard.remove_item)(self.egd, index, 1, true) };
            }
        }

        let mut id = item_id as i32;
        // Safety: keeps achievement state in step with what the inventory now holds, as every
        // other mutation here does.
        unsafe { (self.update_trophy_stats)(self.egd, &raw mut id) };
        // Measured, not assumed: the answer is the difference the inventory reports.
        // Safety: game thread, read only.
        (before - unsafe { self.carried_quantity(item_id) }).max(0)
    }

    /// Take up to `wanted` of `item_id` out of the box. Returns how many actually arrived,
    /// measured from the carried quantity before and after rather than from the call's return.
    ///
    /// `reassignQuickSlot` is `true` here: the item is moving into the pockets, so pointing the
    /// player's quickbar at its new carried index is the correct thing for the engine to do (and
    /// is what it skips anyway when the destination already holds a stack of the same id).
    ///
    /// # Safety
    ///
    /// Game thread.
    pub unsafe fn pull(&self, item_id: u32, wanted: i32) -> i32 {
        if wanted <= 0 {
            return 0;
        }
        // Safety: game thread; all three are reads.
        let available = unsafe { self.stored_quantity(item_id) };
        let headroom = unsafe { self.carried_headroom(item_id, wanted) };
        let before = unsafe { self.carried_quantity(item_id) };
        let take = wanted.min(available).min(headroom.max(0));
        if take <= 0 {
            return 0;
        }
        let mut id = item_id as i32;
        // RE-resolved here, not earlier. Any transfer since the last lookup could have reindexed
        // the box: the call ends in `AdjustQuantityBy` and then `RemoveItem`.
        // Safety: engine-owned inventory pointer, read only.
        let index = unsafe { (self.get_item_idx)(self.box_inventory, &raw mut id) };
        if index < 0 {
            return 0;
        }
        // Safety: game thread, both inventories engine-owned, index resolved on the line above.
        unsafe { (self.transfer)(index, self.box_inventory, self.carried, take, true) };
        // Safety: what the engine's own acquisition path calls once an item has landed.
        unsafe { (self.update_trophy_stats)(self.egd, &raw mut id) };
        // Safety: game thread, read only.
        (unsafe { self.carried_quantity(item_id) } - before).max(0)
    }

    /// Put up to `wanted` of `item_id` into the box. Returns how many actually moved, measured
    /// from the carried quantity before and after.
    ///
    /// Three refusals, in the order they matter:
    ///
    /// * the box is asked first, with `ChangeAmountInBox` -- a pure query that applies both the
    ///   `CanDepositItemToStorageBox` eligibility gate and the box's own `maxRepositoryNum`
    ///   capacity, and answers with a number that may be smaller than `wanted`. That number is
    ///   honoured exactly; transferring more than the box said it would take is how an item goes
    ///   missing;
    /// * an equipped entry is left alone entirely. Its index is named by
    ///   `EquipGameData.equipmentItemIdxList`, and removing it leaves that slot pointing at a
    ///   shifted or freed entry;
    /// * `reassignQuickSlot` is `false`. Setting it here would write the box's index into the
    ///   player's quickbar, and that dangling reference survives into the save.
    ///
    /// # Safety
    ///
    /// Game thread.
    pub unsafe fn deposit(&self, item_id: u32, wanted: i32) -> i32 {
        if wanted <= 0 {
            return 0;
        }
        // Safety: game thread, read only.
        let before = unsafe { self.carried_quantity(item_id) };
        let wanted = wanted.min(before);
        if wanted <= 0 {
            return 0;
        }
        let mut id = item_id as i32;
        // Safety: pure query -- `ChangeAmountInBox` moves nothing, it only answers.
        let accepted =
            unsafe { (self.change_amount_in_box)(self.box_inventory, &raw mut id, wanted) };
        if accepted <= 0 {
            return 0;
        }
        // RE-resolved immediately before the transfer, for the same reason as in `pull`.
        // Safety: engine-owned inventory pointer, read only.
        let index = unsafe { (self.get_item_idx)(self.carried, &raw mut id) };
        if index < 0 {
            return 0;
        }
        // Safety: a bounded read of 22 ints inside a live `EquipGameData`.
        if unsafe { self.is_equipped_index(index) } {
            return 0;
        }
        // Safety: game thread, both inventories engine-owned, index resolved two lines above,
        // and `false` because the destination is the box (see the doc comment).
        unsafe { (self.transfer)(index, self.carried, self.box_inventory, accepted, false) };
        // Safety: keeps achievement state in step with what the inventory now holds, exactly as
        // `er-better-refills` does after its own deposit.
        unsafe { (self.update_trophy_stats)(self.egd, &raw mut id) };
        // Safety: game thread, read only.
        (before - unsafe { self.carried_quantity(item_id) }).max(0)
    }

    /// The inventory index of the carried `item_id`, or a negative number when it is not held.
    ///
    /// The same ambiguity `EquipInventoryData::GetItemInventoryIdx` always has: several copies of
    /// one armament differing only by their ash share an item id, and
    /// `InsertItemIntoLookupMap` keeps the lowest index for a repeated one, so this names a copy
    /// rather than the copy.
    ///
    /// # Safety
    ///
    /// Game thread.
    pub unsafe fn carried_index(&self, item_id: u32) -> i32 {
        let mut id = item_id as i32;
        // Safety: engine-owned inventory pointer, read only.
        unsafe { (self.get_item_idx)(self.carried, &raw mut id) }
    }

    /// The acquisition order stamped on the carried `item_id`, or `None` when it is not held.
    ///
    /// This is the number the equipment menu's `Order of Acquisition` sort reads, and the only
    /// evidence that a [`Storage::recycle`] actually re-acquired something rather than merely
    /// moving it twice. `InsertItem` stamps it from the inventory's own `nextSortId` counter and
    /// increments, so a re-acquired entry comes back with a strictly larger one -- which is what
    /// [`Recycled::restamped`] checks. Counting the two transfers would prove neither.
    ///
    /// # Safety
    ///
    /// Game thread.
    pub unsafe fn carried_sort_id(&self, item_id: u32) -> Option<i32> {
        // Safety: game thread, read only.
        let index = unsafe { self.carried_index(item_id) };
        let index = u32::try_from(index).ok()?;
        // Safety: engine-owned inventory pointer and an index it bounds-checks itself; it answers
        // null rather than faulting when nothing is filed there.
        let entry = unsafe { (self.get_entry)(self.carried, index) };
        if entry == 0 {
            return None;
        }
        // Safety: a fault-checked read of one int at a confirmed offset inside a live entry.
        unsafe { er_game_base::mem::safe_read_i32(entry + INVENTORY_ITEM_ENTRY_SORT_ID_OFFSET) }
    }

    /// Put the whole carried stack of `item_id` in the box and take it straight back out, so the
    /// game re-acquires it and stamps it at the top of the acquisition order.
    ///
    /// # Why a round trip rather than writing the field
    ///
    /// `InventoryItemEntry.sortId` is four bytes at a known offset and could be assigned directly.
    /// It is not, because what an assignment would be trying to imitate is an acquisition, and an
    /// acquisition is more than that field: `InsertItem` also advances `nextSortId`, updates the
    /// lookup map, pushes the recent-item index, and runs `SpecialItemUpdate`. Doing the thing
    /// the player does at a storage box gets all of that for free and stays correct even where
    /// this module's reading of the sort is wrong.
    ///
    /// # The three ways it declines, all of them reported rather than silent
    ///
    /// * `available` is false when the entry is worn. `EquipGameData.equipmentItemIdxList` holds
    ///   inventory indices, so removing an entry a `ChrAsm` slot still names leaves that slot
    ///   pointing at a freed one. The caller unequips first or does not reorder that item;
    /// * `deposited < held_before` means the box would not take the whole stack -- it was full,
    ///   or `CanDepositItemToStorageBox` refused the item. What is left behind stays in the
    ///   pockets, so the retrieve merges into it through `AdjustQuantityBy` and no re-stamp
    ///   happens. The count is restored either way, and [`Recycled::restamped`] answers false;
    /// * `distinct_instances` is the armament case, and it declines twice. Copies of one armament
    ///   that differ only by their ash share an item id, and every question this module can ask --
    ///   `GetItemInventoryIdx`, the retrieve path, `GetQuantityByItemId` -- is asked by id. So
    ///   retrieving from a box that already holds that id can hand back the box's copy and strand
    ///   the player's, and a character holding more than one copy cannot say which copy is being
    ///   moved. The second case would otherwise arrive disguised: `GetQuantityByItemId` counts
    ///   matching entries for a non-stackable rather than reading a quantity, so four Misericordes
    ///   answer `4` while each entry holds one, and a deposit of four against an entry of one is
    ///   refused by `TransferItemBetweenInventoryDatas`'s own `quantity <= entry quantity` guard --
    ///   reported as "the box would not take it" rather than as the ambiguity it really is.
    ///
    /// # Safety
    ///
    /// Game thread.
    pub unsafe fn recycle(&self, item_id: u32, distinct_instances: bool) -> Recycled {
        // Safety: game thread, all reads.
        let held_before = unsafe { self.carried_quantity(item_id) };
        let mut out = Recycled {
            held_before,
            // Safety: game thread, read only.
            sort_id_before: unsafe { self.carried_sort_id(item_id) },
            ..Recycled::default()
        };
        if held_before <= 0 {
            out.declined = Some("not carried");
            return out;
        }
        // Safety: game thread, read only.
        let index = unsafe { self.carried_index(item_id) };
        if index < 0 {
            out.declined = Some("no inventory entry");
            return out;
        }
        // Safety: a bounded read of 22 ints inside a live `EquipGameData`.
        if unsafe { self.is_equipped_index(index) } {
            out.declined = Some("worn, and an entry a ChrAsm slot names must not be removed");
            return out;
        }
        // Safety: game thread, read only.
        if distinct_instances {
            if held_before > 1 {
                out.declined = Some(
                    "the character holds more than one copy of this armament, and the item id \
                     cannot name one of them",
                );
                return out;
            }
            // Safety: game thread, read only.
            if unsafe { self.stored_quantity(item_id) } > 0 {
                out.declined = Some(
                    "the box already holds this id, and copies of an armament cannot be told \
                     apart by id",
                );
                return out;
            }
        }

        // Safety: game thread; deposits the whole stack, so the entry is removed rather than
        // decremented and the retrieve below is a fresh insert.
        out.deposited = unsafe { self.deposit(item_id, held_before) };
        if out.deposited <= 0 {
            out.declined = Some("the box would not take it");
            return out;
        }
        // Safety: game thread; takes back exactly what went in, never more.
        out.retrieved = unsafe { self.pull(item_id, out.deposited) };
        // Safety: game thread, read only.
        out.sort_id_after = unsafe { self.carried_sort_id(item_id) };
        out
    }

    /// Whether a ChrAsm slot names this inventory index.
    ///
    /// The same scan `er-better-refills::is_equipped_item_idx` runs, and for the same reason: the
    /// list holds indices, not item ids, so an entry removed from under one leaves the slot
    /// naming whatever slid into its place.
    ///
    /// # Safety
    ///
    /// Game thread, `self.egd` a live `EquipGameData*`.
    pub unsafe fn is_equipped_index(&self, index: i32) -> bool {
        // Safety: delegated to the scan that answers the same question with the slot number.
        unsafe { self.equipped_slot_of_index(index) }.is_some()
    }

    /// Which `ChrAsmSlot` names this inventory index, if any.
    ///
    /// The scan, rather than `CS::EquipGameData::GetSlotIndexByItemIndex` (`0x140248440`), which
    /// answers the same question and cannot express one of its answers: it returns `ChrAsmSlot`,
    /// its not-found value is `None`, and `None` and `WeaponLeft1` are both zero -- so an item in
    /// the left hand's first slot is indistinguishable from an item that is not worn at all. That
    /// slot is `armament_slot(3)`, an ordinary off-hand, so the ambiguity would land on a common
    /// case and read as "not worn", which is the answer that lets a worn entry be deposited.
    ///
    /// Reading `equipmentItemIdxList` directly has no such gap: the index is the slot.
    ///
    /// # Safety
    ///
    /// Game thread, `self.egd` a live `EquipGameData*`.
    pub unsafe fn equipped_slot_of_index(&self, index: i32) -> Option<i32> {
        (0..EQUIPMENT_ITEM_IDX_LIST_LEN).find_map(|slot| {
            let addr = self.egd + EQUIPMENT_ITEM_IDX_LIST_OFFSET + slot * size_of::<i32>();
            // Safety: a fault-checked read inside a live object; a failed read is not a match.
            let equipped = unsafe { er_game_base::mem::safe_read_i32(addr) };
            (equipped == Some(index)).then(|| i32::try_from(slot).unwrap_or(-1))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The addresses this module calls, pinned to the 1.16.2 static RE that established them.
    ///
    /// They live in `er-game-base::rva` now, shared with `er-better-refills`, so this is the check
    /// that the shared declaration did not move under this crate.
    #[test]
    fn rvas_match_er_1162_static_re() {
        assert_eq!(GET_MAIN_PLAYER_STORAGE_BOX_INVENTORY_RVA, 0x786810);
        assert_eq!(GET_QUANTITY_BY_ITEM_ID_RVA, 0x24c1b0);
        assert_eq!(GET_ITEM_INVENTORY_IDX_RVA, 0x24c560);
        assert_eq!(CHANGE_AMOUNT_IN_BOX_RVA, 0x24e3d0);
        assert_eq!(TRANSFER_ITEM_BETWEEN_INVENTORY_DATAS_RVA, 0x24db90);
        assert_eq!(UPDATE_TROPHY_STATS_RVA, 0x24a1a0);
        assert_eq!(GET_ADD_OR_REMOVE_AMOUNT_RVA, 0x24c630);
        assert_eq!(GET_INVENTORY_ITEM_ENTRY_BY_INDEX_RVA, 0x24e770);
        assert_eq!(EQUIP_GAME_DATA_REMOVE_ITEM_RVA, 0x248ad0);
        assert_eq!(ADJUST_QUANTITY_BY_RVA, 0x24bfe0);
    }

    /// The discard pair is the only optional group here, and the two halves move together.
    ///
    /// Half a discard is a decrement with no removal, which leaves a zero-quantity entry that a
    /// quickbar slot may still name -- so `discard_natives` returns `None` unless both resolved,
    /// and `Storage::discard` returns 0 rather than doing half the job.
    #[test]
    fn the_discard_pair_is_all_or_nothing() {
        // `resolve_all` is what enforces it, and its contract is `Err` naming every missing name
        // rather than a partial array. The `.ok()?` in `discard_natives` turns that into `None`.
        // Asserted here as the shape the two addresses above are wired through, so a future edit
        // that resolves them separately fails a test instead of shipping a half discard.
        let source = include_str!("storage.rs");
        assert!(
            source.contains("fn discard_natives(module_base: usize) -> Option<DiscardNatives>"),
            "the discard pair must resolve as one Option"
        );
        assert!(
            source.contains("let Some(discard) = self.discard else {"),
            "`discard` must refuse outright when the pair is absent"
        );
    }

    /// `InventoryItemEntry` is 24 bytes and `sortId` is the fourth int in it.
    #[test]
    fn sort_id_sits_where_the_1162_structure_puts_it() {
        assert_eq!(INVENTORY_ITEM_ENTRY_SORT_ID_OFFSET, 0xc);
    }

    /// A round trip proves itself with the entry's own acquisition order, never with its own
    /// return value.
    #[test]
    fn a_recycle_is_restamped_only_when_the_order_actually_moved() {
        let moved = Recycled {
            held_before: 3,
            deposited: 3,
            retrieved: 3,
            sort_id_before: Some(12),
            sort_id_after: Some(140),
            declined: None,
        };
        assert!(moved.restamped());
        assert!(moved.count_preserved());

        // The partial deposit. Everything came back, the counts balance, and the retrieve merged
        // into the stack still in the pockets through `AdjustQuantityBy` -- which re-stamps
        // nothing, so the item is exactly where it was in the order.
        let merged = Recycled {
            sort_id_after: Some(12),
            ..moved
        };
        assert!(!merged.restamped());
        assert!(merged.count_preserved());
    }

    /// The one failure that costs the player an item rather than a sort order.
    #[test]
    fn an_item_left_in_the_box_is_not_a_preserved_count() {
        let stranded = Recycled {
            held_before: 5,
            deposited: 5,
            retrieved: 2,
            sort_id_before: Some(4),
            sort_id_after: Some(90),
            declined: None,
        };
        assert!(!stranded.count_preserved());

        // A decline moved nothing at all, so the count is trivially intact.
        let declined = Recycled {
            held_before: 5,
            declined: Some("the box would not take it"),
            ..Recycled::default()
        };
        assert!(declined.count_preserved());
        assert!(!declined.restamped());
    }

    /// An entry that could not be read is not evidence either way.
    #[test]
    fn an_unreadable_sort_id_never_counts_as_a_restamp() {
        let unread = Recycled {
            held_before: 1,
            deposited: 1,
            retrieved: 1,
            sort_id_before: None,
            sort_id_after: Some(90),
            declined: None,
        };
        assert!(!unread.restamped());
        let unread_after = Recycled {
            sort_id_before: Some(1),
            sort_id_after: None,
            ..unread
        };
        assert!(!unread_after.restamped());
    }

    /// `EquipGameData` field offsets, from the 1.16.2 dump's structure (0x4b0 bytes).
    #[test]
    fn equip_game_data_offsets_match_the_1162_structure() {
        assert_eq!(EQUIPMENT_ITEM_IDX_LIST_OFFSET, 0x8);
        assert_eq!(EQUIPMENT_ITEM_IDX_LIST_LEN, 22);
        assert_eq!(GAME_DATA_MAN_PLAYER_OFFSET, 0x08);
    }
}
