//! Putting the granted items on the character, through the menu's own equip handler.
//!
//! # The handler, not a reimplementation
//!
//! `EquipItemToChrAsmSlot` is what the inventory UI calls. It resolves the gaitem handle,
//! clears whatever previously occupied the slot, writes the equipment entry and fires
//! `BroadCastEquipmentChange`. Doing those four things by hand would be reimplementing a
//! function the game already exports, so this builds the one argument it needs and calls it.
//!
//! # Two traps it hides
//!
//! * It takes an **inventory index**, not a param id. The index is only valid after the item is
//!   granted, and which index it is is the subject of the section below.
//! * Equipping an item into the slot it **already occupies toggles it off**. So every equip
//!   is preceded by `GetSlotIndexByItemIndex`, and skipped when the item is already there.
//!   Without that check a re-run strips the gear it just put on.
//!
//! # An item id cannot name one copy, so the equip does not ask by item id
//!
//! An ash of war lives on the **gaitem instance**, not in the item id. A build carrying four
//! Miséricordes that differ only by their ash gives three of them the identical item id
//! `0x000FB9C8`, and there is no id-shaped question whose answer is "the third one".
//! `EquipInventoryData::GetItemInventoryIdx` (`0x14024c560`) is exactly such a question -- its
//! whole body is `if (*itemId != -1) GetItemIndex(&inv->itemsData, itemId)` plus a null-handle
//! rejection -- and `InventoryItemsData::InsertItemIntoLookupMap` keeps the lowest index for a
//! repeated id. So it returns the same copy for all four positions, and `GetParamIdInSlot`, which
//! compares only the param id, then reports every one of them as verified.
//!
//! `plan.rs` grants the worn copy first to make that one answer the right one. That mitigates
//! exactly one position per id and cannot do more, which is why it is a mitigation and this is
//! the fix: the grant now hands each minted armament's `GaItemHandle` forward and the equip
//! resolves the inventory index from that, through
//! `GET_ITEM_INDEX_BY_GAITEM_HANDLE_RVA`. Where no handle is available the id lookup is still used
//! -- and [`EquipOutcome::by_item_id`] records that it was, because an unannounced fall back to
//! the ambiguous question is how this bug stayed invisible.

use er_build_import_core::equip::{
    CHR_ASM_SLOT_QUICK_BASE, EquipLedger, EquipRef, PlannedPosition, PositionKind, PositionResult,
};
use er_build_import_core::plan::ArmamentSkill;

/// `EquipItemToChrAsmSlot(ChrAsmSlot slot, MenuGaitem *item)`.
const EQUIP_ITEM_TO_CHR_ASM_SLOT_RVA: usize = 0x787c30;
// Both addresses are declared once in `er-game-base::rva`, which `er-better-refills` also
// reads them from.
use er_game_base::rva::{
    GET_EQUIP_INVENTORY_DATA_RVA as GET_EQUIP_INVENTORY_DATA,
    GET_EQUIPPED_GREATRUNE_RVA as GET_EQUIPPED_GREATRUNE,
    GET_ITEM_INVENTORY_IDX_RVA as GET_ITEM_INVENTORY_IDX,
    GET_PARAM_ID_IN_SLOT_RVA as GET_PARAM_ID_IN_SLOT,
    GET_PHYSIC_TEAR_BY_SLOT_RVA as GET_PHYSIC_TEAR_BY_SLOT,
};
/// `CS::EquipGameData::GetSlotIndexByItemIndex(egd, itemIdx) -> ChrAsmSlot`.
const GET_SLOT_INDEX_BY_ITEM_INDEX_RVA: usize = 0x248440;
/// `FUN_140788a90(ChrAsmSlot) -> bool` -- the permission gate `EquipItemToChrAsmSlot` consults
/// before doing anything. Probed only, never relied on: when it says no, that function returns
/// void having silently done nothing.
const EQUIP_PERMISSION_GATE_RVA: usize = 0x788a90;
/// `CS::EquipInventoryData::GetGaItemHandleByIndex(inv, uint *out, uint itemIdx) -> uint*`.
const GET_GAITEM_HANDLE_BY_INDEX_RVA: usize = 0x24c7b0;
/// `CS::EquipInventoryData::GetItemIndexByGaitemHandle(EquipInventoryData*, uint *gaitemHandle)
/// -> int` -- the inventory question that can tell two copies of one armament apart.
///
/// Verified against the 1.16.2 dump and byte-checked in `eldenring-deobf.bin` (shift zero;
/// `41 56 48 83 EC 40 48 C7 44 24 20 FE FF FF FF`). It resolves the handle into a
/// `GaitemLookupResult`, reads the instance's item id, and then splits:
///
/// * **stackable** ids -- consumables, which have no per-instance identity -- fall back to the
///   same `GetItemIndex` the id lookup uses, because there is nothing to distinguish;
/// * **everything else**, armaments included, is answered by walking `0..=itemEntriesCount` and
///   returning the index of the entry whose own `InventoryItemEntry::GetGaitemHandle` equals the
///   handle asked about. That is the exact-instance answer, and `-1` when the handle names
///   nothing in this inventory.
///
/// The engine uses it the same way: `FUN_140248670`, the ash-mounting path, feeds its result
/// straight to `EquipInventoryData::RemoveItem`. The first argument is
/// `&egd->equipInventoryData`, which is literally what `GetEquipInventoryData` returns, so it is
/// the pointer this module already holds.
const GET_ITEM_INDEX_BY_GAITEM_HANDLE_RVA: usize = 0x24c460;
/// `CS::EquipGameData::SetEquipmentEntries(egd, slot, uint *gaitemHandle, int itemIdx,
/// bool force, bool writeEntry, bool isArrowOrBolt)` -- the actual equipment writer.
///
/// # `isArrowOrBolt` is inert on this path, and the `false` below is not an ammunition bug
///
/// Ammunition goes through this call like every other `ChrAsm` position, so a reader adding arrows
/// will look for that flag. Its whole body is:
///
/// ```text
/// if (currentHandle != *gaitemHandle || equipmentItemIdxList[slot] != itemIdx || force) {
///     ...the real write...
/// } else if (isArrowOrBolt && writeEntry) {
///     WriteEquipSlot(&equipmentEntries, slot, GetItemIdByIndex(inv, itemIdx));
/// }
/// ```
///
/// The flag is read only in the `else`, i.e. only when the slot already holds this exact handle
/// and index -- a no-op write, which for a stackable arrow still needs the entry's item id
/// refreshed. Every call here passes `force = true`, which takes the first branch unconditionally
/// and does that refresh anyway, so the third flag can never be reached. Setting it for ammo would
/// change nothing; the note exists so nobody has to re-derive that.
const SET_EQUIPMENT_ENTRIES_RVA: usize = 0x249160;
/// `FUN_140249a90(egd)` -- the post-write refresh the menu path always calls.
const EQUIP_REFRESH_RVA: usize = 0x249a90;
/// `BroadCastEquipmentChange(PlayerIns*)` -- tells the rest of the engine the loadout moved.
const BROADCAST_EQUIPMENT_CHANGE_RVA: usize = 0x658c90;
/// `FUN_140249a50(egd, index, uint *gaitemHandle, uint itemIdx)` -- the single native entry for
/// the quickbar, the pouch and the great rune. It dispatches internally:
/// `index < 10` -> `EquipItemData::SetQuickSlotItem`, `10..16` -> the pouch writer, `16` ->
/// `SetGreatRune`. So all three of those live behind one call, and the index is exactly
/// `ChrAsmSlot - 0x16`, which is what `ConvertChrAsmSlotToQuickItemOrPouchSlot` computes.
const SET_QUICK_OR_POUCH_OR_RUNE_RVA: usize = 0x249a50;
/// `CS::EquipGameData::GetPhysicTearBySlot(egd, slot) -> int` -- the physick read-back.
/// `EquipGameData::physicTears`, `int[3]` (empty == `-1`). Confirmed by the getter's own
/// addressing, `MOV ECX,dword ptr [RCX + RAX*0x4 + 0x3e4]`. Recorded, not written: see
/// [`read_physick`] for why writing it directly produced error icons in the flask.
const EQUIP_GAME_DATA_PHYSIC_TEARS: usize = 0x3e4;
/// `CS::EquipGameData::GetItemIdByQuickSlotIndex(egd, int *out, uint index) -> int*` --
/// The QUICKBAR read-back, and another out-parameter getter. Its whole body is
/// `if (index < 10) *out = entries[index + 0x16]; else *out = -1;`, so it answers for the ten
/// quickbar positions and refuses the pouch. The value it hands back is the category-tagged item
/// id, not a bare param id.
const GET_ITEM_ID_BY_QUICK_SLOT_INDEX_RVA: usize = 0x247ee0;
/// `EquipGameData::equipmentEntries`, a `ChrAsmEquipEntries` -- 39 `int`s of category-tagged item
/// ids indexed by `ChrAsmSlot`, of which `0x16..0x1F` are `quickItem1..10` and `0x20..0x25` are
/// `pouch1..6`. This is the array the pouch writer `FUN_14024bb20` stores into
/// (`*(int *)(entries + (index + 0x20) * 4)`) and the array
/// [`GET_ITEM_ID_BY_QUICK_SLOT_INDEX_RVA`] reads, so a direct read of it is the same question the
/// game's own getter asks -- there just is no named getter that covers the pouch.
///
/// The offset is corroborated by its neighbour: `physicTears` sits at `840 + 156 = 996 = 0x3E4`,
/// which [`EQUIP_GAME_DATA_PHYSIC_TEARS`] already proves correct at runtime.
const EQUIP_GAME_DATA_EQUIPMENT_ENTRIES: usize = 0x348;

/// `WorldChrMan::mainPlayerIns`.
const WORLD_CHR_MAN_MAIN_PLAYER_INS: usize = 0x1e508;

/// The bare param row inside a category-tagged item id.
///
/// `GetParamIdInSlot` masks the category nibble off before answering, so every comparison against
/// it has to mask the same way. Declared here rather than reached for out of `grant.rs` because
/// this module's read-backs are the reason it matters here.
const ITEM_ROW_MASK: u32 = 0x0FFF_FFFF;

/// `MenuGaitem` is 128 bytes; the equip path reads only two of its fields.
const MENU_GAITEM_SIZE: usize = 128;
/// `MenuGaitem::itemIdx`.
const MENU_GAITEM_ITEM_IDX: usize = 0x48;
/// `MenuGaitem::itemId`.
const MENU_GAITEM_ITEM_ID: usize = 0x4c;

type EquipFn = unsafe extern "system" fn(i32, *const u8);
type GetInventoryFn = unsafe extern "system" fn(usize) -> usize;
type GetItemIdxFn = unsafe extern "system" fn(usize, *const i32) -> i32;
/// `GetItemIndexByGaitemHandle(EquipInventoryData*, uint *gaitemHandle) -> int`. The handle is
/// read through the pointer and never written, so it is `*const`.
type GetItemIdxByHandleFn = unsafe extern "system" fn(usize, *const u32) -> i32;
type GetSlotFn = unsafe extern "system" fn(usize, i32) -> i32;
type GetParamIdInSlotFn = unsafe extern "system" fn(usize, i32) -> i32;
type GateFn = unsafe extern "system" fn(i32) -> u8;
type GetHandleFn = unsafe extern "system" fn(usize, *mut u32, u32) -> *mut u32;
type SetEntriesFn = unsafe extern "system" fn(usize, i32, *const u32, i32, bool, bool, bool);
type RefreshFn = unsafe extern "system" fn(usize);
type BroadcastFn = unsafe extern "system" fn(usize);
type SetQuickFn = unsafe extern "system" fn(usize, u32, *const u32, u32);
/// `CS::EquipGameData::GetItemIdByQuickSlotIndex(egd, int *out, uint index) -> int*` --
/// out-parameter form, like every other `Get*` here.
type GetQuickIdFn = unsafe extern "system" fn(usize, *mut i32, u32) -> *mut i32;
/// `CS::EquipGameData::GetPhysicTearBySlot(egd, int *out, uint slot) -> int*`.
///
/// An out-parameter form, not a return-value getter: the body is literally
/// `*out = egd->physicTears[slot]`. Calling it as `(egd, slot)` puts the slot index in RDX and
/// the function writes through it -- a store to address 0, which took the game down once.
type GetTearFn = unsafe extern "system" fn(usize, *mut i32, u32) -> *mut i32;
/// `CS::EquipItemData::GetEquippedGreatrune(EquipItemData*, int *out, int slot)`.
///
/// Three arguments, and the third is not optional -- see [`equipped_great_rune`] for the run of
/// wrong answers that produced.
type GreatRuneFn = unsafe extern "system" fn(usize, *mut i32, i32) -> *mut i32;

/// Which minted armament instance belongs in which `ChrAsmSlot`.
///
/// # The join, and why it is on (item id, ash) rather than on order
///
/// The grant produces one [`ArmamentOutcome`](crate::grant::ArmamentOutcome) per armament, in plan
/// order; the equip walks positions in `ChrAsmSlot` order. Neither order is the other's, and
/// matching them by position would be an assumption about two lists built from different
/// traversals of the build document.
///
/// So the join is on the pair that actually *identifies* an armament to a player: its item id and
/// the ash mounted on it. That pair is complete -- two copies agreeing on both are genuinely
/// interchangeable instances, so it does not matter which of them a position takes -- and it is
/// the smallest thing that is. An entry is consumed once claimed, so two positions asking for the
/// same armament with the same ash get two different copies rather than the same one twice.
///
/// The ash side of the pair comes from [`er_build_import_core::plan::equipped_armament_skills`], the
/// same table the post-import read-back is adjudicated against, and the gem encoding is decoded
/// by the grant's own [`crate::grant::gem_row_of`] rather than re-implemented here.
pub struct WornInstances {
    /// Every armament the grant minted a usable handle for.
    minted: Vec<Minted>,
    /// `ChrAsmSlot` -> the gem row the build wants worn there. An absent slot is a position that
    /// is not an armament at all, which is a different answer from an armament with no ash.
    wanted: std::collections::BTreeMap<i32, Option<u32>>,
}

/// One minted armament, and whether a position has already claimed it.
struct Minted {
    /// Category-tagged armament id, affinity included.
    item_id: u32,
    /// `EquipParamGem` row the plan asked to mount, or `None` for "no ash".
    gem: Option<u32>,
    /// The `GaItemHandle` the mint produced. Never zero: unusable ones are not kept.
    handle: u32,
    /// Claimed by a position already, so no second position may take it.
    taken: bool,
}

/// How a position's inventory index is going to be found.
enum Resolved {
    /// The handle of the exact instance minted for this position.
    Handle(u32),
    /// No handle; fall back to the ambiguous item-id lookup, for this stated reason.
    ById(&'static str),
}

impl WornInstances {
    /// Build the join from what the grant minted and what each armament slot should wear.
    ///
    /// # When this can pick the wrong ash, and why that is safe
    ///
    /// `wants` may name one slot more than once, because a planner payload can have several rows
    /// claiming one position. The last entry wins here, which is payload order;
    /// `equip::settle` breaks the same tie by the rows' `order` field, and
    /// `equipped_armament_skills` selects rows by the bare `equipIndex` while `equip_plan` selects
    /// them by the active set. Those agree on an ordinary payload and can disagree on a
    /// self-contradicting one -- which the import already reports as `CONTESTED`.
    ///
    /// When they do disagree this looks up an ash the plan did not place, finds no minted copy
    /// carrying it, and returns `Resolved::ById` with a reason. So the disagreement degrades to
    /// the old id lookup with a log line saying so, never to a confidently-wrong handle.
    pub fn new(armaments: &[crate::grant::ArmamentOutcome], wants: &[ArmamentSkill]) -> Self {
        Self {
            minted: armaments
                .iter()
                .filter(|arm| arm.handle != 0)
                .map(|arm| Minted {
                    item_id: arm.item_id,
                    gem: arm.wanted_gem,
                    handle: arm.handle,
                    taken: false,
                })
                .collect(),
            wanted: wants
                .iter()
                .map(|want| (want.slot, crate::grant::gem_row_of(want.weapon_skill)))
                .collect(),
        }
    }

    /// How many minted armaments are available to be claimed.
    pub fn available(&self) -> usize {
        self.minted.len()
    }

    /// Claim the instance that belongs in `slot`, or say why there is none.
    fn claim(&mut self, slot: i32, item_id: u32) -> Resolved {
        let Some(gem) = self.wanted.get(&slot).copied() else {
            return Resolved::ById(
                "this position is not an armament, so the grant minted no instance for it",
            );
        };
        // Matched without the upgrade level, which is the last two digits of an armament's item
        // id. The grant mints at the level the build asked for (`60500125`) while the equip plan
        // names the armament (`60500100`) -- it has no business deciding a level, and could not
        // anyway, since the clamp against this armament's real `ReinforceParamWeapon` rows is a
        // runtime question. Comparing the raw ids made every armament fall to the id lookup, and
        // then miss there too because the unlevelled id names nothing in the inventory: measured
        // 2026-08-23 as `0 position(s) found by minted gaitem handle` and 10/10 gear -> 8/10.
        let identity = armament_identity(item_id);
        let Some(found) = self
            .minted
            .iter_mut()
            .find(|arm| !arm.taken && armament_identity(arm.item_id) == identity && arm.gem == gem)
        else {
            return Resolved::ById(
                "no unclaimed minted armament matches this position's item id and ash",
            );
        };
        found.taken = true;
        Resolved::Handle(found.handle)
    }

    /// The levelled item id the grant actually minted for `slot`, if it minted one.
    ///
    /// The fall-back id lookup needs this: the plan's own id is unlevelled, so asking the
    /// inventory about it reports the armament missing when it is right there at +25.
    pub fn minted_id_for(&self, slot: i32, item_id: u32) -> Option<u32> {
        let gem = self.wanted.get(&slot).copied()?;
        let identity = armament_identity(item_id);
        self.minted
            .iter()
            .find(|arm| armament_identity(arm.item_id) == identity && arm.gem == gem)
            .map(|arm| arm.item_id)
    }
}

/// An armament's identity without its upgrade level: base row plus affinity.
///
/// The game's own normalisation -- `EquipParamWeapon::GetEntry` looks up `(paramId / 100) * 100`
/// precisely because the last two digits are the level.
fn armament_identity(item_id: u32) -> u32 {
    item_id / 100 * 100
}

/// What the equip pass observed while filling the plan.
///
/// # Where the counts are not
///
/// Deliberately: this struct holds evidence, never a score. The score lives in the
/// [`EquipLedger`] the caller opened over the plan, so a position the pass never visits stays
/// in the denominator instead of leaving with it. The field this replaced -- `quick_written`,
/// incremented once per dispatcher call and read back by nothing -- was both halves of that
/// mistake at once: a call count presented as a result, over a denominator that had already
/// dropped the positions it counted.
#[derive(Debug, Default)]
pub struct EquipOutcome {
    /// What the menu path's permission gate said, for the first few slots.
    pub gate: Vec<(i32, u8)>,
    /// Every quick/pouch/rune dispatch, as `(slot, wanted item id, inventory index, read back)`.
    /// The last element is the read-back, so a line of this trace is self-adjudicating.
    pub dispatch: Vec<(i32, u32, i32, i32)>,
    /// Item ids the inventory could not locate, i.e. the grant did not land.
    pub not_in_inventory: Vec<u32>,
    /// `(slot, the id wanted, what the position holds)` for a quick/pouch/rune position whose
    /// item no row of could be found in the inventory.
    ///
    /// The id the character owns is the one fact this failure is missing, and the position
    /// itself is holding it. Reading it costs one getter call on a path that was about to give
    /// up anyway, and it turns "not in the inventory" -- which for two flasks was wrong twice in
    /// a row and unfalsifiable from the log -- into a line naming the row that is actually
    /// there. When that row is one the build's name resolves to, the position is already correct
    /// and is recorded as such instead of as a casualty.
    pub position_holds_instead: Vec<(i32, u32, i32)>,
    /// Positions whose inventory index came from the exact instance the grant minted.
    pub by_handle: usize,
    /// `(kind, slot, item id, why)` for every position that fell back to the item-id lookup.
    ///
    /// Kept in full rather than counted, and carrying its kind, because the two cases are not
    /// equally serious. A talisman or a quickbar consumable has no per-instance identity to lose,
    /// so the id lookup is the right question for it and the entry is bookkeeping. An armament
    /// falling back is the importer admitting it may have equipped an arbitrary twin, and that
    /// admission is worthless if the log buries it among the harmless ones.
    pub by_item_id: Vec<(PositionKind, i32, u32, &'static str)>,
    /// `(kind, slot, the id the build named, the id the inventory actually held)` for every
    /// position that was found under one of [`EquipRef::also_known_as`].
    ///
    /// Not bookkeeping. The build names an item; the character owns a different row of that same
    /// item because they have upgraded it, and the two are different ids. Every entry here is a
    /// position that would previously have been recorded `NotInInventory` and left empty, so the
    /// log has to name the substitution rather than let a silent id swap look like a plain
    /// success.
    pub by_upgrade_variant: Vec<(PositionKind, i32, u32, u32)>,
    /// `(slot, expected, actual)` for armament positions holding the right armament at a
    /// different upgrade level.
    ///
    /// Counted as the position being filled -- the equip's job is which armament is in which
    /// hand, and the level is the grant's job, reported on its own `ARMAMENT` line. Kept
    /// separately so "the plan asked for +25 and the hand holds +10" is still visible instead of
    /// being folded into a bare `Verified`.
    pub level_differences: Vec<(i32, i32, i32)>,
    /// `(slot, item id, inventory index, the slot that already claimed it)` for every position
    /// whose inventory index was already spoken for by an earlier position in the same pass.
    ///
    /// Equipping one entry twice strips the first slot. `EquipItemToChrAsmSlot` (`0x140787c30`)
    /// calls `FUN_140247160(egd, oldSlot, true)` -- unequip it from where it already sits --
    /// before writing the new slot, so a later position naming an entry an earlier position is
    /// already wearing tears the earlier one back off. The per-position read-back cannot see it:
    /// it runs before the position that will undo it. Observed 2026-08-23 on build
    /// 94252a868b4f2a, where the ledger recorded `12 planned = 12 verified` and both hands were
    /// empty by the end of the same import.
    pub index_collisions: Vec<(i32, u32, i32, i32)>,
    /// `(slot, expected, actual)` for the first few positions that read back wrong.
    pub mismatches: Vec<(i32, i32, i32)>,
    /// `(slot, expected, actual)` for positions that passed their own read-back and were wrong
    /// again by the end of the pass.
    ///
    /// The per-position read-back proves a write landed; only the final sweep proves it survived
    /// the rest of the pass. A position that passes the first and fails the second was taken back
    /// off by something later, which is a different defect from one that never took -- so only
    /// that case lands here. A position that was already wrong at its own read-back is not
    /// evidence of a stripper and must not be reported as one: the sweep re-reads it, finds it
    /// still wrong, and leaves the failure it already had. Saying "something took them back off"
    /// about a position that never held the item is a claim the sweep has not established, and
    /// for three whole runs it was the only thing the log said about them.
    pub stripped_after_verifying: Vec<(i32, i32, i32)>,
    /// The equip game data pointer was unusable, so nothing at all was attempted.
    pub no_inventory: bool,
    /// Game functions with no verified mapping for the running build, so the positions that
    /// needed them were not attempted.
    ///
    /// Kept as names rather than a bool because the useful next action is a `docs/recon` row for
    /// each one, and a bool names none of them. Empty is the normal state.
    pub unresolved_natives: Vec<&'static str>,
}

/// The game functions [`equip_all`] calls, resolved for the build that is actually running.
///
/// # Why this is a struct and not thirteen `let`s
///
/// It used to be thirteen `transmute(module_base + <the RVA>)` lines under one shared comment
/// claiming "verified 1.16.2 RVAs within the loaded image". Every one was a direct call into game
/// code at an address from a previous patch, and the shared comment made them look like one
/// decision that had been checked once. Grouping them makes the resolution a single decision that
/// really is checked once -- and makes the two optional families below visible as families rather
/// than as thirteen equally-load-bearing addresses.
struct EquipNatives {
    equip: EquipFn,
    get_inventory: GetInventoryFn,
    get_item_idx: GetItemIdxFn,
    get_item_idx_by_handle: GetItemIdxByHandleFn,
    get_slot: GetSlotFn,
    param_in_slot: GetParamIdInSlotFn,
    get_handle: GetHandleFn,
    set_entries: SetEntriesFn,
    refresh: RefreshFn,
    broadcast: BroadcastFn,
    /// The menu path's permission gate. Probe only -- its answer is recorded and then used to
    /// choose between two write paths that are both available. `None` means it could not be
    /// resolved, which is treated as "not permitted": that is the branch that writes through the
    /// engine's own setters, i.e. the one that does not need the gate's opinion at all.
    gate: Option<GateFn>,
    /// The quick/pouch/rune family: the single dispatcher that writes those positions, and the
    /// getter that reads the quickbar back.
    ///
    /// One `Option` for the pair because a write with no read-back would put an unproven position
    /// in the ledger, and this module's whole discipline is that no number is derived from a call
    /// having been made. `None` means those positions are not attempted at all.
    quick: Option<QuickNatives>,
}

/// The quick/pouch/rune writer and its read-back, resolved together. See [`EquipNatives::quick`].
#[derive(Clone, Copy)]
struct QuickNatives {
    set_quick: SetQuickFn,
    get_quick_id: GetQuickIdFn,
    /// The great rune is not in the quickbar array at all -- index 16 writes `equipItemData+0x88`
    /// -- so its read-back is a third function rather than the same getter with another index.
    get_great_rune: GreatRuneFn,
}

impl EquipNatives {
    /// Resolve every function the equip pass calls, or say which ones have no mapping.
    ///
    /// The required ten are the ones both write paths need; missing any of them means the pass
    /// cannot run and `Err` names all of them, because fixing them one rebuild at a time is how a
    /// morning disappears.
    fn resolve(module_base: usize) -> Result<Self, Vec<&'static str>> {
        let [
            equip,
            get_inventory,
            get_item_idx,
            get_item_idx_by_handle,
            get_slot,
            param_in_slot,
            get_handle,
            set_entries,
            refresh,
            broadcast,
        ] = crate::native::resolve_all(
            module_base,
            [
                (EQUIP_ITEM_TO_CHR_ASM_SLOT_RVA, "EquipItemToChrAsmSlot"),
                (
                    GET_EQUIP_INVENTORY_DATA,
                    "CS::EquipGameData::GetEquipInventoryData",
                ),
                (
                    GET_ITEM_INVENTORY_IDX,
                    "CS::EquipInventoryData::GetItemInventoryIdx",
                ),
                (
                    GET_ITEM_INDEX_BY_GAITEM_HANDLE_RVA,
                    "CS::EquipInventoryData::GetItemIndexByGaitemHandle",
                ),
                (
                    GET_SLOT_INDEX_BY_ITEM_INDEX_RVA,
                    "CS::EquipGameData::GetSlotIndexByItemIndex",
                ),
                (GET_PARAM_ID_IN_SLOT, "CS::EquipGameData::GetParamIdInSlot"),
                (
                    GET_GAITEM_HANDLE_BY_INDEX_RVA,
                    "CS::EquipInventoryData::GetGaItemHandleByIndex",
                ),
                (
                    SET_EQUIPMENT_ENTRIES_RVA,
                    "CS::EquipGameData::SetEquipmentEntries",
                ),
                (
                    EQUIP_REFRESH_RVA,
                    "EquipGameData equip refresh (FUN_140249a90)",
                ),
                (BROADCAST_EQUIPMENT_CHANGE_RVA, "BroadCastEquipmentChange"),
            ],
        )?;
        let quick = match crate::native::resolve_all(
            module_base,
            [
                (
                    SET_QUICK_OR_POUCH_OR_RUNE_RVA,
                    "quick/pouch/rune dispatcher (FUN_140249a50)",
                ),
                (
                    GET_ITEM_ID_BY_QUICK_SLOT_INDEX_RVA,
                    "CS::EquipGameData::GetItemIdByQuickSlotIndex",
                ),
                (
                    GET_EQUIPPED_GREATRUNE,
                    "CS::EquipItemData::GetEquippedGreatrune",
                ),
            ],
        ) {
            // Safety: all three addresses were resolved for the running build immediately above.
            Ok([set_quick, get_quick_id, get_great_rune]) => Some(unsafe {
                QuickNatives {
                    set_quick: core::mem::transmute::<usize, SetQuickFn>(set_quick),
                    get_quick_id: core::mem::transmute::<usize, GetQuickIdFn>(get_quick_id),
                    get_great_rune: core::mem::transmute::<usize, GreatRuneFn>(get_great_rune),
                }
            }),
            Err(_) => None,
        };
        // The gate is asked for separately because it is the one function whose absence changes
        // nothing: its `false` answer and its absence lead to the same write path.
        let gate = crate::native::resolve(
            module_base,
            EQUIP_PERMISSION_GATE_RVA,
            "equip permission gate (FUN_140788a90)",
        )
        // Safety: resolved for the running build immediately above.
        .map(|address| unsafe { core::mem::transmute::<usize, GateFn>(address) });
        // Safety: every address below was resolved for the running build by `resolve_all`.
        Ok(unsafe {
            Self {
                equip: core::mem::transmute::<usize, EquipFn>(equip),
                get_inventory: core::mem::transmute::<usize, GetInventoryFn>(get_inventory),
                get_item_idx: core::mem::transmute::<usize, GetItemIdxFn>(get_item_idx),
                get_item_idx_by_handle: core::mem::transmute::<usize, GetItemIdxByHandleFn>(
                    get_item_idx_by_handle,
                ),
                get_slot: core::mem::transmute::<usize, GetSlotFn>(get_slot),
                param_in_slot: core::mem::transmute::<usize, GetParamIdInSlotFn>(param_in_slot),
                get_handle: core::mem::transmute::<usize, GetHandleFn>(get_handle),
                set_entries: core::mem::transmute::<usize, SetEntriesFn>(set_entries),
                refresh: core::mem::transmute::<usize, RefreshFn>(refresh),
                broadcast: core::mem::transmute::<usize, BroadcastFn>(broadcast),
                gate,
                quick,
            }
        })
    }
}

/// Read what a quick/pouch/rune position currently holds. `-1` means empty.
///
/// Returns the category-tagged item id, which is what all three of these positions store --
/// so the comparison is against [`EquipRef::item_id`], never `param_id`. Getting that wrong is
/// how the physick "verified 2/2" while showing error icons: a read-back only proves a value
/// round-tripped, so it has to be compared against the value the game would have written.
///
/// # Safety
///
/// Game thread, `egd` live, `index` in `0..=16`. `quick` must be the natives resolved for the
/// running build -- which is what having a [`QuickNatives`] at all means.
unsafe fn read_quick_position(
    quick: QuickNatives,
    egd: usize,
    kind: PositionKind,
    index: u32,
) -> i32 {
    match kind {
        // The native getter, which covers exactly the ten quickbar positions.
        PositionKind::Quickbar => {
            let mut out = -1i32;
            // Safety: out-parameter getter, resolved for the running build; the slot is ours and
            // outlives the call.
            unsafe { (quick.get_quick_id)(egd, &raw mut out, index) };
            out
        }
        // No named getter reaches the pouch, so read the array the pouch writer stores into.
        PositionKind::Pouch => {
            let entry = egd
                + EQUIP_GAME_DATA_EQUIPMENT_ENTRIES
                + (CHR_ASM_SLOT_QUICK_BASE as usize + index as usize) * 4;
            // Safety: one int inside a fixed-size array of a live struct, at a verified offset.
            unsafe { *(entry as *const i32) }
        }
        // The rune is not in that array at all: the dispatcher's `index == 16` branch writes
        // `equipItemData + 0x88`, which is what GetEquippedGreatrune reads.
        PositionKind::GreatRune => {
            let mut out = -1i32;
            // Safety: the three-argument out-parameter form, resolved for the running build; the
            // out slot is ours and outlives the call.
            unsafe { (quick.get_great_rune)(egd, &raw mut out, 0) };
            out
        }
        // Every other kind is a ChrAsm equipment entry, answered by GetParamIdInSlot instead.
        _ => -1,
    }
}

/// Equip everything the ledger's plan asks for, recording each position's read-back into it.
///
/// `instances` carries the gaitem handles the grant minted; each armament position claims its own
/// so that copies of one armament differing only by ash reach the right hands. It is consumed as
/// the pass walks, which is why it is taken by mutable reference.
///
/// # Safety
///
/// Game thread, character in the world, items already granted.
pub unsafe fn equip_all(
    module_base: usize,
    egd: usize,
    ledger: &mut EquipLedger,
    instances: &mut WornInstances,
) -> EquipOutcome {
    let mut outcome = EquipOutcome::default();
    // `(slot, inventory index)` for every position this pass has already equipped, so a later
    // position cannot name an entry an earlier one is wearing. See the guard below for why that
    // is destructive rather than merely redundant.
    let mut claimed: Vec<(i32, i32)> = Vec::new();
    // What each position was left holding, and whether its own read-back agreed, indexed the same
    // way as `planned`. The final sweep needs both: the id to re-compare against (which is not the
    // one the plan names, once an upgrade level is in play) and whether this position was ever
    // right, because only a position that was right and then went wrong was stripped by something
    // later. `None` is a position the pass never reached.
    let mut placements: Vec<Option<Placement>> = vec![None; ledger.planned().len()];
    // Cloned so the ledger stays writable while the pass walks it. The list is at most ~20
    // entries, and holding a borrow of the thing being recorded into is not worth the saving.
    let planned: Vec<PlannedPosition> = ledger.planned().to_vec();

    // Resolved for the running build, not added blind. Every address below is a 1.16.2 RVA and
    // the installed game is 1.17: a call through an unresolved one lands wherever that code moved
    // to, which is a control transfer into the middle of an unrelated function.
    //
    // A refusal here costs the equip pass and nothing else -- the grants, the stats and the
    // spells are separate steps and still run. Every planned position is recorded as not
    // attempted, so the ledger's denominator keeps them rather than quietly shrinking.
    let natives = match EquipNatives::resolve(module_base) {
        Ok(natives) => natives,
        Err(missing) => {
            outcome.unresolved_natives = missing;
            for (at, position) in planned.iter().enumerate() {
                if position.kind == PositionKind::Physick {
                    continue;
                }
                ledger.record(
                    at,
                    PositionResult::NotAttempted(
                        "the equip natives have no verified mapping for the running build",
                    ),
                );
            }
            return outcome;
        }
    };
    let EquipNatives {
        equip,
        get_inventory,
        get_item_idx,
        get_item_idx_by_handle,
        get_slot,
        param_in_slot,
        get_handle,
        set_entries,
        refresh,
        broadcast,
        gate,
        quick,
    } = natives;

    // Resolved once: BroadCastEquipmentChange wants the live PlayerIns.
    let main_player = {
        use eldenring::cs::WorldChrMan;
        use fromsoftware_shared::FromStatic;
        match WorldChrMan::instance_ptr() {
            Ok(wcm) if !wcm.is_null() => {
                // Safety: one pointer read at a verified offset in a live singleton.
                unsafe { *((wcm as usize + WORLD_CHR_MAN_MAIN_PLAYER_INS) as *const usize) }
            }
            _ => 0,
        }
    };

    // Safety: engine-owned pointer, read only.
    let inventory = unsafe { get_inventory(egd) };
    if inventory == 0 {
        outcome.no_inventory = true;
        for (at, position) in planned.iter().enumerate() {
            if position.kind == PositionKind::Physick {
                continue;
            }
            outcome.not_in_inventory.push(position.item.item_id);
            ledger.record(
                at,
                PositionResult::NotAttempted("the inventory pointer was null"),
            );
        }
        return outcome;
    }

    for (at, position) in planned.iter().enumerate() {
        // The physick is not a ChrAsmSlot and is written by `fill_physick`. It stays UNACCOUNTED
        // here on purpose: whoever writes it records it, and if nobody does, the ledger says so.
        if position.kind == PositionKind::Physick {
            continue;
        }
        let Some(slot) = position.slot else {
            ledger.record(
                at,
                PositionResult::NotAttempted("the plan gave this position no native slot"),
            );
            continue;
        };

        // The ID to ask the inventory about, which for an armament is not the one the plan names:
        // the plan names the armament, the grant minted it at a level, and the level is part of
        // the id. Asking about the unlevelled id reports a +25 weapon as not in the inventory.
        let lookup_id = instances
            .minted_id_for(slot, position.item.item_id)
            .unwrap_or(position.item.item_id);
        let id = lookup_id as i32;

        // Which copy. Asking by item id cannot answer that -- see the module header -- so the
        // handle the grant minted for this exact position is tried first, and the fall back to
        // the ambiguous question is recorded rather than taken quietly.
        let mut fell_back_to_id = None;
        let mut item_idx = match instances.claim(slot, position.item.item_id) {
            Resolved::Handle(handle) => {
                // Safety: the handle outlives the call and the inventory pointer is live; the
                // native reads the handle through the pointer and never writes it.
                let found = unsafe { get_item_idx_by_handle(inventory, &raw const handle) };
                if found < 0 {
                    // The instance was minted but this inventory does not hold it. That is a real
                    // finding, not a reason to silently equip a twin -- but the id lookup is
                    // still the better of the two remaining answers, so it is used and said.
                    fell_back_to_id =
                        Some("the minted handle names no entry in this inventory any more");
                }
                found
            }
            Resolved::ById(why) => {
                fell_back_to_id = Some(why);
                -1
            }
        };
        if item_idx < 0 {
            // Safety: `id` outlives the call; the inventory pointer is live.
            item_idx = unsafe { get_item_idx(inventory, &raw const id) };
        } else {
            outcome.by_handle += 1;
        }
        if let Some(why) = fell_back_to_id {
            outcome
                .by_item_id
                .push((position.kind, slot, lookup_id, why));
        }

        // The row this character actually owns. An id is not a stable name for an item the
        // player can upgrade: an upgraded flask or talisman is a different `EquipParamGoods`
        // row from the unupgraded one, under a name the game suffixes ` +N`. So a character who
        // has drunk one Sacred Tear holds none of the ids the build's name resolved to, and
        // asking only about those reports an item that is visibly in the pouch as missing --
        // which is what `NOT-IN-INVENTORY 0x400003E8` / `0x4000041A` were, on both the Crimson
        // and the Cerulean flask, in every import run there is a log of.
        //
        // Only reached when the named id is genuinely absent, so a character holding the exact
        // row the build named is never sent down this path.
        let mut placed_id = lookup_id;
        if item_idx < 0 {
            for candidate in &position.item.also_known_as {
                let candidate_id = *candidate as i32;
                // Safety: `candidate_id` outlives the call; the inventory pointer is live.
                let found = unsafe { get_item_idx(inventory, &raw const candidate_id) };
                if found >= 0 {
                    item_idx = found;
                    placed_id = *candidate;
                    outcome
                        .by_upgrade_variant
                        .push((position.kind, slot, lookup_id, placed_id));
                    break;
                }
            }
        }
        let placed = placed_id as i32;

        if item_idx < 0 {
            // Before giving up, ask the position. See `position_holds_instead`.
            let mut holds = None;
            if position.kind.is_quick_dispatch()
                && let Some(quick) = quick
                && let Ok(index) = u32::try_from(slot - CHR_ASM_SLOT_QUICK_BASE)
                && index <= QUICK_DISPATCH_MAX_INDEX
            {
                // Safety: resolved natives, bounded index, live `egd`.
                let actual = unsafe { read_quick_position(quick, egd, position.kind, index) };
                outcome
                    .position_holds_instead
                    .push((slot, lookup_id, actual));
                holds = (actual >= 0).then_some(actual);
            }
            let wanted_row = holds.is_some_and(|actual| {
                actual == placed || position.item.also_known_as.contains(&actual.unsigned_abs())
            });
            if wanted_row {
                let actual = holds.unwrap_or(placed);
                placements[at] = Some(Placement {
                    expected: actual,
                    verified: true,
                });
                ledger.record(at, PositionResult::Already);
            } else {
                outcome.not_in_inventory.push(lookup_id);
                ledger.record(at, PositionResult::NotInInventory);
            }
            continue;
        }

        // Quickbar, pouch and great-rune positions are not ChrAsm equipment entries at all -- the
        // engine routes them to a different writer, so SetEquipmentEntries would be the wrong
        // call even when the menu gate allows it. Unlike the equipment path this writer is a
        // plain assignment rather than a toggle, so there is nothing to check beforehand.
        if position.kind.is_quick_dispatch() {
            // Bounded before it is used: the pouch read-back below indexes a fixed-size array
            // with it, so an index the plan should never produce must not become a wild read.
            let index = match u32::try_from(slot - CHR_ASM_SLOT_QUICK_BASE) {
                Ok(index) if index <= QUICK_DISPATCH_MAX_INDEX => index,
                _ => {
                    ledger.record(
                        at,
                        PositionResult::NotAttempted(
                            "the slot is outside the quick/pouch/rune dispatcher's range",
                        ),
                    );
                    continue;
                }
            };
            // The write and its read-back resolved together or not at all: writing a position
            // this pass cannot then read back would put an unproven entry in the ledger, and the
            // module header says no number here is derived from a call having been made.
            let Some(quick) = quick else {
                ledger.record(
                    at,
                    PositionResult::NotAttempted(
                        "the quick/pouch/rune natives have no verified mapping for the running \
                         build",
                    ),
                );
                continue;
            };
            let mut handle = [0u32; 2];
            // Safety: engine-owned inventory, validated index, our own handle buffer.
            unsafe { get_handle(inventory, handle.as_mut_ptr(), item_idx as u32) };
            // Safety: the native dispatcher for quick/pouch/rune, resolved for this build.
            unsafe { (quick.set_quick)(egd, index, handle.as_ptr(), item_idx as u32) };
            // Safety: same context; a read of the position that was just written.
            let actual = unsafe { read_quick_position(quick, egd, position.kind, index) };
            outcome.dispatch.push((slot, placed_id, item_idx, actual));
            let result = verdict(&mut outcome, position.kind, slot, placed, actual);
            placements[at] = Some(Placement {
                expected: placed,
                verified: result.is_success(),
            });
            ledger.record(at, result);
            continue;
        }

        // One inventory entry, one slot. Equipping an entry that an earlier position in this very
        // pass is already wearing does not add a second copy -- `EquipItemToChrAsmSlot` unequips it
        // from the earlier slot first (`FUN_140247160(egd, oldSlot, true)`), so the later write
        // silently strips the earlier one and the earlier position's read-back, taken before this
        // one ran, still says Verified. Refusing here keeps a wrong answer out of the character
        // and out of the log; the position is recorded as a failure, which is what it is.
        if let Some((held_by, _)) = claimed
            .iter()
            .copied()
            .find(|(_, claimed_idx)| *claimed_idx == item_idx)
        {
            outcome
                .index_collisions
                .push((slot, position.item.item_id, item_idx, held_by));
            ledger.record(
                at,
                PositionResult::NotAttempted(
                    "another position in this pass is already wearing this exact inventory entry; \
                     equipping it here would strip that slot instead of filling this one",
                ),
            );
            continue;
        }
        claimed.push((slot, item_idx));

        // Safety: same context. Asking where the item currently sits.
        let current = unsafe { get_slot(egd, item_idx) };
        if current == slot {
            // Calling the handler here would toggle the item off.
            placements[at] = Some(Placement {
                expected: (placed_id & ITEM_ROW_MASK) as i32,
                verified: true,
            });
            ledger.record(at, PositionResult::Already);
            continue;
        }

        // Ask the menu path's gate what it thinks, purely to record it. An unresolved gate reads
        // as "not permitted", which is not a guess: that answer selects the branch that writes
        // through the engine's own setters, i.e. the one that never needed the gate's opinion.
        // Safety: a pure predicate over engine singletons, already known non-null here.
        let permitted = match gate {
            Some(gate) => unsafe { gate(slot) },
            None => 0,
        };
        if outcome.gate.len() < GATE_ANSWERS_KEPT {
            outcome.gate.push((slot, permitted));
        }

        if permitted != 0 {
            let mut gaitem = [0u8; MENU_GAITEM_SIZE];
            gaitem[MENU_GAITEM_ITEM_IDX..MENU_GAITEM_ITEM_IDX + 4]
                .copy_from_slice(&item_idx.to_le_bytes());
            gaitem[MENU_GAITEM_ITEM_ID..MENU_GAITEM_ITEM_ID + 4]
                .copy_from_slice(&placed.to_le_bytes());
            // Safety: the handler reads only itemIdx and itemId on this path.
            unsafe { equip(slot, gaitem.as_ptr()) };
        } else {
            // The menu handler refuses outside its own context, so write through the layer it
            // itself uses once its gate passes. These are the engine's own setters -- the only
            // thing skipped is the menu's permission question.
            let mut handle = [0u32; 2];
            // Safety: engine-owned inventory, validated index, our own handle buffer.
            unsafe { get_handle(inventory, handle.as_mut_ptr(), item_idx as u32) };
            // Safety: the flags are the ones the menu path passes.
            unsafe { set_entries(egd, slot, handle.as_ptr(), item_idx, true, true, false) };
            // Safety: the refresh and broadcast the menu path always runs after a write.
            unsafe { refresh(egd) };
            if main_player != 0 {
                unsafe { broadcast(main_player) };
            }
        }

        // Read the slot back. The handler returns void and declines silently, so this is the
        // only thing that distinguishes "equipped" from "asked politely and was ignored".
        // The ChrAsm getter masks the category nibble off, so the comparison here is against
        // the bare param id -- the opposite of the quick/pouch/rune read-back above.
        //
        // Against the row that was equipped, not the row the plan named. For an armament those
        // are different numbers by construction: the plan names the armament (`7100000`) and the
        // grant mints it at the level the build asked for (`7100010`), so comparing against
        // `position.item.param_id` asks whether the hand holds a +0 Eclipse Shotel and reports
        // the +10 one that was just correctly equipped as a failure. That is what
        // `SLOT 1 expected 7100000 but holds 7100010` was -- in the same run whose ash read-back
        // called the same slot OK -- and it cost three armaments out of three in every import
        // this branch has a log for.
        // Safety: same context; a plain read of the slot's current param id.
        let actual = unsafe { param_in_slot(egd, slot) };
        let expected = (placed_id & ITEM_ROW_MASK) as i32;
        let result = verdict(&mut outcome, position.kind, slot, expected, actual);
        placements[at] = Some(Placement {
            expected,
            verified: result.is_success(),
        });
        ledger.record(at, result);
    }

    // The sweep that actually proves it. Every read-back above happened before the positions after
    // it were written, so each one proves only that its own write landed -- not that it was still
    // there at the end. A position stripped by a later equip passes the first check and fails this
    // one, and that difference is the whole reason the two are recorded separately.
    //
    // It is a detector and it only gets to accuse what it can see. A position that was already
    // wrong at its own read-back is not evidence that anything stripped it, so re-reading it and
    // finding it still wrong adds no fact: it keeps the failure it already had, and stays out of
    // `stripped_after_verifying`. Only a position that was verified and is now wrong supports the
    // sentence "something later in the pass took it back off".
    for (at, position) in planned.iter().enumerate() {
        // Same exclusions the pass itself uses: the physick is not a ChrAsmSlot, and the
        // quick/pouch/rune positions read back through their own dispatcher rather than ChrAsm.
        if position.kind == PositionKind::Physick || position.kind.is_quick_dispatch() {
            continue;
        }
        let Some(slot) = position.slot else {
            continue;
        };
        // A position the pass never placed has no expectation to re-check, and its failure is
        // already recorded.
        let Some(placement) = placements[at] else {
            continue;
        };
        // Safety: same context; a plain read of the slot's current param id.
        let actual = unsafe { param_in_slot(egd, slot) };
        if holds_expected(position.kind, placement.expected, actual) {
            continue;
        }
        if !placement.verified {
            continue;
        }
        if outcome.stripped_after_verifying.len() < MISMATCHES_KEPT {
            outcome
                .stripped_after_verifying
                .push((slot, placement.expected, actual));
        }
        ledger.record(
            at,
            PositionResult::Mismatch {
                expected: placement.expected,
                actual,
            },
        );
    }

    outcome
}

/// The dispatcher's highest index: 0..9 quickbar, 10..15 pouch, 16 great rune. `FUN_140249a50`
/// itself returns without doing anything past this, and the pouch read-back indexes a fixed-size
/// array, so anything higher is refused rather than passed on.
const QUICK_DISPATCH_MAX_INDEX: u32 = 16;
/// How many gate answers to keep for the log.
const GATE_ANSWERS_KEPT: usize = 8;
/// How many read-back mismatches to keep for the log.
const MISMATCHES_KEPT: usize = 8;

/// What one position was left holding, and whether its own read-back agreed.
#[derive(Clone, Copy)]
struct Placement {
    /// The id the position should read back as -- the row that was actually equipped, bare for a
    /// `ChrAsm` slot and category-tagged for a quick/pouch/rune one, matching each read-back's
    /// own convention.
    expected: i32,
    /// Whether the read-back taken immediately after the write agreed.
    verified: bool,
}

/// Whether `actual` is the item the pass placed.
///
/// Exact for everything except an armament, whose upgrade level is the last two digits of its
/// param id and is not part of which armament it is. The game normalises the same way --
/// `EquipParamWeapon::GetEntry` looks up `(paramId / 100) * 100` for exactly this reason -- and
/// `lib.rs`'s ash read-back already adjudicates the worn armament on the same rule. The level is
/// not dropped: it is reported on its own by the grant's `ARMAMENT ... +N -> +N` line, and a
/// position that matches only after normalising is recorded in
/// [`EquipOutcome::level_differences`] so the difference stays in the log.
fn holds_expected(kind: PositionKind, expected: i32, actual: i32) -> bool {
    if expected == actual {
        return true;
    }
    kind == PositionKind::Armament
        && expected > 0
        && actual > 0
        && armament_identity(expected.unsigned_abs()) == armament_identity(actual.unsigned_abs())
}

/// Turn one read-back into a verdict, recording the failing ones for the log.
fn verdict(
    outcome: &mut EquipOutcome,
    kind: PositionKind,
    slot: i32,
    expected: i32,
    actual: i32,
) -> PositionResult {
    if actual == expected {
        return PositionResult::Verified;
    }
    if holds_expected(kind, expected, actual) {
        if outcome.level_differences.len() < MISMATCHES_KEPT {
            outcome.level_differences.push((slot, expected, actual));
        }
        return PositionResult::Verified;
    }
    if outcome.mismatches.len() < MISMATCHES_KEPT {
        outcome.mismatches.push((slot, expected, actual));
    }
    PositionResult::Mismatch { expected, actual }
}

/// Fill the Flask of Wondrous Physick.
///
/// `EquipGameData::physicTears` is `int[3]` at `+0x3e4` (empty == `-1`), confirmed by the
/// getter's own addressing `MOV ECX,[RCX + RAX*4 + 0x3e4]`. No native setter for it exists in
/// the dump under any searchable name, so this writes the field -- but it now writes the right
/// kind of value, which the first attempt did not.
///
/// The first attempt stored the tear's bare param id and produced error icons in game. The
/// field actually holds the **category-tagged** id: a flask populated by the game itself reads
/// back as `0x40001FC1`, i.e. nibble 4 (goods) in the high nibble. Writing `11003` where
/// `0x40002AFB` belongs gives the UI an id it cannot resolve, which is exactly an error icon.
///
/// That first attempt also "verified 2/2", because it compared the read-back against the same
/// wrong value it had written. A read-back only proves a value round-tripped; it cannot prove
/// the value means what you think. The comparison here is against the tagged id for that reason.
///
/// # Safety
///
/// Game thread, `egd` live.
pub unsafe fn fill_physick(
    module_base: usize,
    egd: usize,
    tears: &[Option<EquipRef>],
) -> Option<usize> {
    // The write is a STRUCT field; Only the read-back is a game function. The store below goes to
    // `egd + 0x3e4`, a field offset -- a different kind of 1.17 risk, owned by
    // `scripts/detect-struct-field-drift.py`, not by the address gate. The getter is code, and
    // code moved. So an unresolvable getter means the tears are still written and nothing is
    // verified, which is what `None` says: `0` would be indistinguishable from "the game read the
    // flask back and it held something else", and that is the exact confusion the first version
    // of this function shipped (it "verified 2/2" against the wrong value it had itself written).
    let get_tear = crate::native::resolve(
        module_base,
        GET_PHYSIC_TEAR_BY_SLOT,
        "CS::EquipGameData::GetPhysicTearBySlot",
    );
    let mut verified = 0;
    for (index, tear) in tears.iter().enumerate().take(2) {
        let Some(tear) = tear else { continue };
        // The tagged id, not param_id.
        let wanted = tear.item_id as i32;
        // Safety: one int of an int[3] at a verified offset in live save data.
        unsafe { *((egd + EQUIP_GAME_DATA_PHYSIC_TEARS + index * 4) as *mut i32) = wanted };
        let Some(get_tear) = get_tear else { continue };
        // Safety: resolved for the running build immediately above.
        let get_tear: GetTearFn = unsafe { core::mem::transmute(get_tear) };
        let mut got = -1i32;
        // Safety: the native out-parameter getter.
        unsafe { get_tear(egd, &raw mut got, index as u32) };
        if got == wanted {
            verified += 1;
        }
    }
    get_tear.map(|_| verified)
}

/// Read what the flask currently holds, for the log.
///
/// # Safety
///
/// Game thread, `egd` live.
pub unsafe fn read_physick(module_base: usize, egd: usize) -> Option<[i32; 2]> {
    // `None`, not `[-1, -1]`: in this field `-1` means empty, so a refusal returned as `-1`
    // would be reported to the reader as an empty flask on a character that may be carrying two
    // tears. A refusal has to be unrepresentable as a value.
    let get_tear = crate::native::resolve(
        module_base,
        GET_PHYSIC_TEAR_BY_SLOT,
        "CS::EquipGameData::GetPhysicTearBySlot",
    )?;
    // Safety: resolved for the running build immediately above.
    let get_tear: GetTearFn = unsafe { core::mem::transmute(get_tear) };
    let mut out = [-1i32; 2];
    for (index, slot) in out.iter_mut().enumerate() {
        let mut got = -1i32;
        // Safety: the native out-parameter getter.
        unsafe { get_tear(egd, &raw mut got, index as u32) };
        *slot = got;
    }
    Some(out)
}

/// Read back the equipped great rune. `-1` means none.
///
/// Another out-parameter getter, like `GetPhysicTearBySlot`: the disassembly is
/// `ADD RCX,0x288 / MOV RBX,RDX / CALL ... / MOV RAX,RBX`, i.e. RDX is a pointer the callee
/// writes through and RAX is just that same pointer handed back. Treating it as a scalar
/// getter passes the caller's second argument as a destination address and stores through it.
///
/// # Safety
///
/// Game thread, `egd` live.
pub unsafe fn equipped_great_rune(module_base: usize, egd: usize) -> Option<i32> {
    // Three arguments. The outer wrapper at 0x140247900 is only
    //     add RCX,0x288 / MOV RBX,RDX / call 0x14024f390 / MOV RAX,RBX
    // -- it never writes R8, so the slot argument passes straight through from the caller into
    // `CS::EquipItemData::GetEquippedGreatrune(EquipItemData*, int *out, int slot)`, whose body
    // begins `*out = -1; if (slot == 0 && ...)`. Calling it with two arguments leaves R8 holding
    // whatever the call site happened to have, the `slot == 0` test fails, and it reports -1 no
    // matter what is equipped. That produced three runs of "the rune will not equip" when the
    // rune was fine and the question was malformed.
    //
    // `None` for a refusal for the same reason the malformed call was worth fixing: `-1` is this
    // getter's own answer for "no rune equipped", so returning it for "could not ask" reports a
    // fact the code never established.
    let get = crate::native::resolve(
        module_base,
        GET_EQUIPPED_GREATRUNE,
        "CS::EquipItemData::GetEquippedGreatrune",
    )?;
    // Safety: resolved for the running build; the out slot is ours and outlives the call.
    let get: GreatRuneFn = unsafe { core::mem::transmute(get) };
    let mut out = -1i32;
    // Safety: as above.
    unsafe { get(egd, &raw mut out, 0) };
    Some(out)
}
