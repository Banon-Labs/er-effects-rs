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
    CHANGE_AMOUNT_IN_BOX_RVA, CS_MENU_MAN_GLOBAL_RVA, GAME_DATA_MAN_GLOBAL_RVA,
    GET_ADD_OR_REMOVE_AMOUNT_RVA, GET_ITEM_INVENTORY_IDX_RVA,
    GET_MAIN_PLAYER_STORAGE_BOX_INVENTORY_RVA, GET_QUANTITY_BY_ITEM_ID_RVA,
    TRANSFER_ITEM_BETWEEN_INVENTORY_DATAS_RVA, UPDATE_TROPHY_STATS_RVA,
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
}

impl Storage {
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

    /// Whether a ChrAsm slot names this inventory index.
    ///
    /// The same scan `er-better-refills::is_equipped_item_idx` runs, and for the same reason: the
    /// list holds indices, not item ids, so an entry removed from under one leaves the slot
    /// naming whatever slid into its place.
    ///
    /// # Safety
    ///
    /// Game thread, `self.egd` a live `EquipGameData*`.
    unsafe fn is_equipped_index(&self, index: i32) -> bool {
        (0..EQUIPMENT_ITEM_IDX_LIST_LEN).any(|slot| {
            let addr = self.egd + EQUIPMENT_ITEM_IDX_LIST_OFFSET + slot * size_of::<i32>();
            // Safety: a fault-checked read inside a live object; a failed read is not a match.
            let equipped = unsafe { er_game_base::mem::safe_read_i32(addr) };
            equipped == Some(index)
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
    }

    /// `EquipGameData` field offsets, from the 1.16.2 dump's structure (0x4b0 bytes).
    #[test]
    fn equip_game_data_offsets_match_the_1162_structure() {
        assert_eq!(EQUIPMENT_ITEM_IDX_LIST_OFFSET, 0x8);
        assert_eq!(EQUIPMENT_ITEM_IDX_LIST_LEN, 22);
        assert_eq!(GAME_DATA_MAN_PLAYER_OFFSET, 0x08);
    }
}
