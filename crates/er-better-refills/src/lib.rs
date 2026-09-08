//! Standalone storage-box autorefill improvement DLL.
//!
//! Vanilla Elden Ring only flips persistent autorefill state when the user toggles an item in the
//! storage box. The actual storage -> personal inventory transfer happens later through
//! `ReplanishItemsFromChest`. This DLL hooks the native toggle function and, when the new native
//! state is enabled, calls the game's own refill routine immediately. That preserves the game's
//! item eligibility, stack/capacity, storage removal, and unlimited-consumables gates.

// Host-build hygiene. This crate is a windows `cdylib`: on a non-windows host every item
// whose only consumer is `DllMain` or a hook reads as dead, and `[workspace.lints.rust]
// warnings = "deny"` promotes that to a hard compile ERROR -- so `cargo test -p er-better-refills`
// failed outright, and its unit tests had therefore never executed in any gate. Same fix,
// same reason, as er-save-suppress / er-seamless-bugfixes / er-armament-icons. The shipping
// target is unaffected: this allow does not exist there.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

#[cfg(windows)]
mod crashlog;

use std::{
    fmt,
    path::PathBuf,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use er_game_base::{
    log::{append_line, game_directory_path},
    mem::{game_module_base, safe_read_usize},
    rva::{GAME_DATA_MAN_GLOBAL_RVA, REPLANISH_ITEMS_FROM_CHEST_RVA, SHOULD_REPLENISH_ITEM_RVA},
};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;

const LOG_FILE_NAME: &str = "er-better-refills.log";

/// `SetItemReplenishState(int *itemId)`, called by the DepositoryDialog toggle handler.
/// Static RE / 1.16.2 MCP: `FUN_1408d87d0 -> SetItemReplenishState`.
const SET_ITEM_REPLENISH_STATE_RVA: usize = 0x786430;
/// `IsItemReplanishFromChestRequested()`: reads `GameMan->itemReplanishFromChestRequested`.
const IS_ITEM_REPLANISH_FROM_CHEST_REQUESTED_RVA: usize = 0x67a050;
/// `CS::MoveMapStep::UpdatePlayerInfo(this)`: native post-load player handoff. The function tail
/// already consumes the vanilla chest-refill request flag, so a post-hook here is the load-complete
/// point for deaths, map warps, continue/load, and other MoveMapStep-backed loading screens.
const MOVE_MAP_STEP_UPDATE_PLAYER_INFO_RVA: usize = 0xafc160;
/// `CS::CSLuaEventScriptImitation::OnEvent_BonfireFirstLvUp(this, proxy)`: first grace activation.
const BONFIRE_FIRST_LVUP_RVA: usize = 0x59c1e0;
/// `CS::EquipGameData::GetEquipInventoryData(equipGameData)` -> main `EquipInventoryData*`.
/// Declared once in `er-game-base`; a second crate now needs the same address.
use er_game_base::rva::GET_EQUIP_INVENTORY_DATA_RVA;
// The storage-box transfer set, moved to `er-game-base::rva` on 2026-08-31:
//
//   GetMainPlayerStorageBoxInventory()                                    -> box inventory
//   EquipInventoryData::GetItemInventoryIdx(inventory, int *itemId)
//   EquipInventoryData::GetQuantityByItemId(inventory, int *itemId)
//   EquipInventoryData::ChangeAmountInBox(box, int *itemId, int requested)
//   TransferItemBetweenInventoryDatas(itemIdx, src, dst, qty, reassignQuickSlot)
//   CS::EquipGameData::UpdateTrophyStats(equipGameData, int *itemId)
//
// This crate deposits into the box on first grace; `er-build-import-runtime` now pulls back out
// of it and deposits pot-group members to free capacity for an imported build. Two crates, one
// set of addresses, and a second literal copy of each is exactly the alias drift
// `check-rva-alias-drift.py` refuses. The 1.16.2 values stay pinned by this crate's own
// `rvas_match_er_1162_static_re` test, which is now the check that the shared declaration did not
// move under it.
use er_game_base::rva::{
    CHANGE_AMOUNT_IN_BOX_RVA, GET_ITEM_INVENTORY_IDX_RVA,
    GET_MAIN_PLAYER_STORAGE_BOX_INVENTORY_RVA, GET_QUANTITY_BY_ITEM_ID_RVA,
    TRANSFER_ITEM_BETWEEN_INVENTORY_DATAS_RVA, UPDATE_TROPHY_STATS_RVA,
};

/// `GameDataMan -> PlayerGameData` is shared in `er-game-base`; within `PlayerGameData`,
/// `equipGameData` is by-value at +0x2b0 and the native `ItemReplenishStateTracker*` used by
/// SetItemReplenishState sits at +0x5e8 (`equipGameData + 0x338`).
const PLAYER_GAME_DATA_EQUIP_GAME_DATA_OFFSET: usize = 0x2b0;
const PLAYER_GAME_DATA_ITEM_REPLENISH_TRACKER_OFFSET: usize = 0x5e8;
/// `EquipGameData.equipmentItemIdxList: int[22]`; native deposit UI leaves one behind when the
/// selected item index is equipped and storage could otherwise accept the full carried quantity.
const EQUIP_GAME_DATA_EQUIPMENT_ITEM_IDX_LIST_OFFSET: usize = 0x8;
const EQUIPMENT_ITEM_IDX_LIST_LEN: usize = 22;

const HOOK_INACTIVE: usize = 0;
const HOOK_ACTIVE: usize = 1;

static LOG_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static HOOK_STATE: AtomicUsize = AtomicUsize::new(HOOK_INACTIVE);
static GAME_BASE: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_ITEM_REPLENISH_STATE: AtomicUsize = AtomicUsize::new(0);
static ORIG_MOVE_MAP_STEP_UPDATE_PLAYER_INFO: AtomicUsize = AtomicUsize::new(0);
static ORIG_BONFIRE_FIRST_LVUP: AtomicUsize = AtomicUsize::new(0);
static TOGGLE_CALLS: AtomicU64 = AtomicU64::new(0);
static IMMEDIATE_REFILLS: AtomicU64 = AtomicU64::new(0);
static LOAD_COMPLETION_REFILLS: AtomicU64 = AtomicU64::new(0);
static LOAD_COMPLETION_VANILLA_PENDING: AtomicU64 = AtomicU64::new(0);
static FIRST_GRACE_REFILLS: AtomicU64 = AtomicU64::new(0);
static DISABLE_DEPOSITS: AtomicU64 = AtomicU64::new(0);
static DISABLE_DEPOSIT_SKIPS: AtomicU64 = AtomicU64::new(0);
static DISABLE_DEPOSIT_EQUIPPED_KEEP_ONE: AtomicU64 = AtomicU64::new(0);
static SKIPPED_DISABLED_AFTER_TOGGLE: AtomicU64 = AtomicU64::new(0);
static SKIPPED_NO_TRACKER: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
static START: std::sync::Once = std::sync::Once::new();

fn log_message(args: fmt::Arguments<'_>) {
    let path = game_directory_path()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(LOG_FILE_NAME);
    let seq = LOG_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1;
    append_line(&path, format_args!("[{seq:06}] {args}"));
}

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
///
/// Called by the Windows loader. Do not call directly.
pub unsafe extern "system" fn DllMain(
    module: *mut core::ffi::c_void,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // One sink for this DLL's hook + address lines. Without it a refused address is
        // silent here, because every cdylib links its own copy of er-hook/er-game-base.
        // A rust_panic in a cdylib loaded into the game is otherwise anonymous: the message goes to a
        // stderr nobody reads, and what survives is a 0xe06d7363 record naming the module and nothing
        // else. Two boots were lost to one before this existed. See er_game_base::panic_report.
        er_game_base::panic_report::report_panics_to("er-better-refills", log_message);
        er_hook::set_hook_logger(log_message);
        let module_base = module as usize;
        START.call_once(|| spawn_better_refills_task(module_base));
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_better_refills_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}

#[cfg(windows)]
fn spawn_better_refills_task(module_base: usize) {
    let _ = std::thread::Builder::new()
        .name("er-better-refills".to_owned())
        .spawn(move || {
            crashlog::install(module_base);
            if crashlog::force_crash_requested() {
                unsafe { crashlog::force_crash_for_smoke() };
            }
            let mut attempts = 0_u64;
            // Bounded (2026-08-29): an unbounded `loop { yield_now() }` in two other shells starved the
            // wineserver and hung a whole boot -- see er_game_base::wait. Same shape, same fix.
            let found = er_game_base::wait::poll_until(|| match game_module_base() {
                Ok(base) => Some(base),
                Err(err) => {
                    if attempts == 0 || attempts.is_multiple_of(4096) {
                        log_message(format_args!("install: waiting for game module base: {err}"));
                    }
                    attempts = attempts.saturating_add(1);
                    None
                }
            });
            let Some(base) = found else {
                log_message(format_args!(
                    "install: no game module base; nothing installed"
                ));
                return;
            };
            GAME_BASE.store(base, Ordering::SeqCst);
            install_better_refills_hooks(base);
        });
}

#[cfg(windows)]
fn install_better_refills_hooks(base: usize) {
    use std::ffi::c_void;

    use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};

    if HOOK_STATE.load(Ordering::SeqCst) == HOOK_ACTIVE {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            log_message(format_args!("install: MH_Initialize failed: {status:?}"));
            return;
        }
    }

    let targets = [
        (
            "SetItemReplenishState",
            base + SET_ITEM_REPLENISH_STATE_RVA,
            set_item_replenish_state_hook as *mut c_void,
            &ORIG_SET_ITEM_REPLENISH_STATE,
        ),
        (
            "MoveMapStep::UpdatePlayerInfo",
            base + MOVE_MAP_STEP_UPDATE_PLAYER_INFO_RVA,
            move_map_step_update_player_info_hook as *mut c_void,
            &ORIG_MOVE_MAP_STEP_UPDATE_PLAYER_INFO,
        ),
        (
            "OnEvent_BonfireFirstLvUp",
            base + BONFIRE_FIRST_LVUP_RVA,
            bonfire_first_lvup_hook as *mut c_void,
            &ORIG_BONFIRE_FIRST_LVUP,
        ),
    ];

    // One refused hook must not take the others down with it.
    //
    // This loop used to `return` on the first failure. That is much worse than it looks: the
    // failure happens at `MhHook::new`, before `MH_ApplyQueued`, so bailing there leaves the
    // hooks that did queue successfully queued and never applied. The mod does not degrade, it
    // goes completely inert -- and it says nothing, because the only log line is about the one
    // that failed. Measured on 1.17: `OnEvent_BonfireFirstLvUp` is third in this list and its
    // address was refused, so `SetItemReplenishState` and `MoveMapStep::UpdatePlayerInfo` were
    // both mapped, both queued, and both dead, with no `hooks ACTIVE` line to say so.
    //
    // A refused address is now survivable per-hook: the features behind the hooks that resolved
    // still work, and the ones that did not are named. Partial is the honest outcome of a partial
    // migration; silence is not.
    let mut installed: Vec<&str> = Vec::new();
    let mut refused: Vec<&str> = Vec::new();
    for (name, target, detour, orig_slot) in targets {
        let hook = match unsafe { MhHook::new(target as *mut c_void, detour) } {
            Ok(hook) => hook,
            Err(status) => {
                log_message(format_args!(
                    "install: MhHook::new({name} @0x{target:x}) failed: {status:?} -- skipping this hook, keeping the rest"
                ));
                refused.push(name);
                continue;
            }
        };
        orig_slot.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            log_message(format_args!(
                "install: queue_enable({name} @0x{target:x}) failed: {status:?} -- skipping this hook, keeping the rest"
            ));
            refused.push(name);
            continue;
        }
        installed.push(name);
    }

    if installed.is_empty() {
        log_message(format_args!(
            "install: NO hook queued ({} refused: {}); nothing to apply",
            refused.len(),
            refused.join(", ")
        ));
        return;
    }
    if !refused.is_empty() {
        log_message(format_args!(
            "install: PARTIAL -- queued [{}], refused [{}]; applying the queued ones",
            installed.join(", "),
            refused.join(", ")
        ));
    }

    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            HOOK_STATE.store(HOOK_ACTIVE, Ordering::SeqCst);
            log_message(format_args!(
                "install: hooks ACTIVE SetItemReplenishState @0x{:x}, MoveMapStep::UpdatePlayerInfo @0x{:x}, OnEvent_BonfireFirstLvUp @0x{:x}; native refill rva=0x{REPLANISH_ITEMS_FROM_CHEST_RVA:x}",
                er_game_base::mem::game_data_addr(
                    base,
                    SET_ITEM_REPLENISH_STATE_RVA,
                    "SET_ITEM_REPLENISH_STATE_RVA"
                ),
                er_game_base::mem::game_data_addr(
                    base,
                    MOVE_MAP_STEP_UPDATE_PLAYER_INFO_RVA,
                    "MOVE_MAP_STEP_UPDATE_PLAYER_INFO_RVA"
                ),
                er_game_base::mem::game_data_addr(
                    base,
                    BONFIRE_FIRST_LVUP_RVA,
                    "BONFIRE_FIRST_LVUP_RVA"
                ),
            ));
        }
        status => log_message(format_args!("install: MH_ApplyQueued failed: {status:?}")),
    }
}

/// Post-hook on `SetItemReplenishState(int *itemId)`.
///
/// The original toggles the native state. After it returns, `ShouldReplenishItem` reports the new
/// effective state, including default handling. We only refill when that new state is enabled. The
/// depository handler refreshes the list immediately after this call, so the UI should observe the
/// native inventory changes from `ReplanishItemsFromChest`.
#[cfg(windows)]
unsafe extern "system" fn set_item_replenish_state_hook(item_id: *mut i32) {
    let call_index = TOGGLE_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    let orig = ORIG_SET_ITEM_REPLENISH_STATE.load(Ordering::SeqCst);
    if orig != 0 {
        let original: unsafe extern "system" fn(*mut i32) = unsafe { std::mem::transmute(orig) };
        unsafe { original(item_id) };
    }

    if item_id.is_null() {
        log_message(format_args!(
            "toggle#{call_index}: skipped immediate refill: null item pointer"
        ));
        return;
    }

    let Some(tracker) = (unsafe { resolve_item_replenish_state_tracker() }) else {
        let skipped = SKIPPED_NO_TRACKER.fetch_add(1, Ordering::SeqCst) + 1;
        log_message(format_args!(
            "toggle#{call_index}: skipped immediate refill: no ItemReplenishStateTracker (skipped_no_tracker={skipped})"
        ));
        return;
    };

    let base = GAME_BASE.load(Ordering::SeqCst);
    if base == 0 {
        log_message(format_args!(
            "toggle#{call_index}: skipped immediate refill: missing game base"
        ));
        return;
    }

    type ShouldReplenishItemFn = unsafe extern "system" fn(usize, *mut i32) -> bool;
    // Through the 1.17 gate. `base + rva` calls the 1.16.2 address on a build that moved the
    // function, and nothing refuses it -- the map knowing the new address is no help to a call
    // that never asks.
    let Ok(should_replenish_addr) = er_game_base::mem::game_rva_named(
        SHOULD_REPLENISH_ITEM_RVA as u32,
        "SHOULD_REPLENISH_ITEM_RVA",
    ) else {
        return;
    };
    let should_replenish: ShouldReplenishItemFn =
        unsafe { std::mem::transmute(should_replenish_addr) };
    if !unsafe { should_replenish(tracker, item_id) } {
        let skipped = SKIPPED_DISABLED_AFTER_TOGGLE.fetch_add(1, Ordering::SeqCst) + 1;
        let raw_item_id = unsafe { item_id.read_unaligned() };
        match deposit_inventory_item_to_storage(raw_item_id) {
            DepositBackResult::Deposited {
                moved,
                carried_before,
                source_item_idx,
                kept_one_equipped,
            } => {
                let deposits = DISABLE_DEPOSITS.fetch_add(1, Ordering::SeqCst) + 1;
                if kept_one_equipped {
                    DISABLE_DEPOSIT_EQUIPPED_KEEP_ONE.fetch_add(1, Ordering::SeqCst);
                }
                log_message(format_args!(
                    "toggle#{call_index}: item=0x{raw_item_id:08x} disabled after toggle; deposited {moved}/{carried_before} back to storage via native TransferItemBetweenInventoryDatas (source_item_idx={source_item_idx}, kept_one_equipped={kept_one_equipped}, disable_deposits={deposits}, skipped_disabled={skipped})"
                ));
            }
            DepositBackResult::Skipped { reason } => {
                let deposit_skips = DISABLE_DEPOSIT_SKIPS.fetch_add(1, Ordering::SeqCst) + 1;
                if skipped <= 8
                    || skipped.is_multiple_of(32)
                    || deposit_skips <= 8
                    || deposit_skips.is_multiple_of(32)
                {
                    log_message(format_args!(
                        "toggle#{call_index}: item=0x{raw_item_id:08x} disabled after toggle; no deposit: {reason} (deposit_skips={deposit_skips}, skipped_disabled={skipped})"
                    ));
                }
            }
        }
        return;
    }

    let raw_item_id = unsafe { item_id.read_unaligned() };
    if call_native_refill("storage-toggle-on") {
        let refills = IMMEDIATE_REFILLS.fetch_add(1, Ordering::SeqCst) + 1;
        log_message(format_args!(
            "toggle#{call_index}: item=0x{raw_item_id:08x} enabled after toggle; called native ReplanishItemsFromChest (immediate_refills={refills})"
        ));
    } else {
        log_message(format_args!(
            "toggle#{call_index}: item=0x{raw_item_id:08x} enabled after toggle; skipped native ReplanishItemsFromChest: missing runtime context"
        ));
    }
}

/// Post-hook on `CS::MoveMapStep::UpdatePlayerInfo(this)`.
///
/// The original function is the native post-load player-info handoff and already consumes the
/// vanilla `GameMan->itemReplanishFromChestRequested` flag at its tail. If that flag was pending at
/// entry, the original performs the native refill and clears it; otherwise this hook adds the desired
/// refill-on-load-completion behavior after the native player update returns.
#[cfg(windows)]
unsafe extern "system" fn move_map_step_update_player_info_hook(this: *mut core::ffi::c_void) {
    let base = GAME_BASE.load(Ordering::SeqCst);
    let vanilla_request_pending = if base == 0 {
        false
    } else {
        type IsItemReplanishFromChestRequestedFn = unsafe extern "system" fn() -> bool;
        match er_game_base::mem::game_rva_named(
            IS_ITEM_REPLANISH_FROM_CHEST_REQUESTED_RVA as u32,
            "IS_ITEM_REPLANISH_FROM_CHEST_REQUESTED_RVA",
        ) {
            Ok(address) => {
                let is_requested: IsItemReplanishFromChestRequestedFn =
                    unsafe { std::mem::transmute(address) };
                unsafe { is_requested() }
            }
            // No verified address means we cannot know; "no vanilla request pending" is the
            // conservative answer, and it is what an unhooked build reports anyway.
            Err(_) => false,
        }
    };

    let orig = ORIG_MOVE_MAP_STEP_UPDATE_PLAYER_INFO.load(Ordering::SeqCst);
    if orig != 0 {
        let original: unsafe extern "system" fn(*mut core::ffi::c_void) =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(this) };
    }

    if vanilla_request_pending {
        let count = LOAD_COMPLETION_VANILLA_PENDING.fetch_add(1, Ordering::SeqCst) + 1;
        if count <= 8 || count.is_multiple_of(32) {
            log_message(format_args!(
                "load-complete#{count}: vanilla itemReplanishFromChestRequested was pending; original UpdatePlayerInfo consumed native refill"
            ));
        }
        return;
    }

    if call_native_refill("load-complete") {
        let count = LOAD_COMPLETION_REFILLS.fetch_add(1, Ordering::SeqCst) + 1;
        if count <= 8 || count.is_multiple_of(32) {
            log_message(format_args!(
                "load-complete#{count}: called native ReplanishItemsFromChest after MoveMapStep::UpdatePlayerInfo"
            ));
        }
    }
}

/// Post-hook on first grace activation (`OnEvent_BonfireFirstLvUp`).
#[cfg(windows)]
unsafe extern "system" fn bonfire_first_lvup_hook(
    this: *mut core::ffi::c_void,
    proxy: *mut core::ffi::c_void,
) {
    let orig = ORIG_BONFIRE_FIRST_LVUP.load(Ordering::SeqCst);
    if orig != 0 {
        let original: unsafe extern "system" fn(*mut core::ffi::c_void, *mut core::ffi::c_void) =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(this, proxy) };
    }

    if call_native_refill("first-grace-activation") {
        let count = FIRST_GRACE_REFILLS.fetch_add(1, Ordering::SeqCst) + 1;
        if count <= 8 || count.is_multiple_of(32) {
            log_message(format_args!(
                "first-grace#{count}: called native ReplanishItemsFromChest after OnEvent_BonfireFirstLvUp"
            ));
        }
    }
}

#[cfg(windows)]
enum DepositBackResult {
    Deposited {
        moved: i32,
        carried_before: i32,
        source_item_idx: i32,
        kept_one_equipped: bool,
    },
    Skipped {
        reason: &'static str,
    },
}

#[cfg(windows)]
fn deposit_inventory_item_to_storage(raw_item_id: i32) -> DepositBackResult {
    let base = GAME_BASE.load(Ordering::SeqCst);
    if base == 0 {
        return DepositBackResult::Skipped {
            reason: "missing game base",
        };
    }
    let Some(player_game_data) = (unsafe { resolve_player_game_data() }) else {
        return DepositBackResult::Skipped {
            reason: "missing PlayerGameData",
        };
    };

    let equip_game_data = player_game_data + PLAYER_GAME_DATA_EQUIP_GAME_DATA_OFFSET;
    let mut item_id = raw_item_id;

    type GetEquipInventoryDataFn = unsafe extern "system" fn(usize) -> usize;
    type GetMainPlayerStorageBoxInventoryFn = unsafe extern "system" fn() -> usize;
    type GetQuantityByItemIdFn = unsafe extern "system" fn(usize, *mut i32) -> i32;
    type GetItemInventoryIdxFn = unsafe extern "system" fn(usize, *mut i32) -> i32;
    type ChangeAmountInBoxFn = unsafe extern "system" fn(usize, *mut i32, i32) -> i32;
    type TransferItemBetweenInventoryDatasFn =
        unsafe extern "system" fn(i32, usize, usize, i32, bool);
    type UpdateTrophyStatsFn = unsafe extern "system" fn(usize, *mut i32);

    // All seven through the 1.17 gate, resolved together and before any of them runs: this
    // function moves items between inventories, so half a deposit is worse than none.
    let (
        Ok(get_main_inventory_addr),
        Ok(get_storage_inventory_addr),
        Ok(get_quantity_addr),
        Ok(get_item_idx_addr),
        Ok(change_amount_addr),
        Ok(transfer_item_addr),
        Ok(update_trophy_addr),
    ) = (
        er_game_base::mem::game_rva_named(
            GET_EQUIP_INVENTORY_DATA_RVA as u32,
            "GET_EQUIP_INVENTORY_DATA_RVA",
        ),
        er_game_base::mem::game_rva_named(
            GET_MAIN_PLAYER_STORAGE_BOX_INVENTORY_RVA as u32,
            "GET_MAIN_PLAYER_STORAGE_BOX_INVENTORY_RVA",
        ),
        er_game_base::mem::game_rva_named(
            GET_QUANTITY_BY_ITEM_ID_RVA as u32,
            "GET_QUANTITY_BY_ITEM_ID_RVA",
        ),
        er_game_base::mem::game_rva_named(
            GET_ITEM_INVENTORY_IDX_RVA as u32,
            "GET_ITEM_INVENTORY_IDX_RVA",
        ),
        er_game_base::mem::game_rva_named(
            CHANGE_AMOUNT_IN_BOX_RVA as u32,
            "CHANGE_AMOUNT_IN_BOX_RVA",
        ),
        er_game_base::mem::game_rva_named(
            TRANSFER_ITEM_BETWEEN_INVENTORY_DATAS_RVA as u32,
            "TRANSFER_ITEM_BETWEEN_INVENTORY_DATAS_RVA",
        ),
        er_game_base::mem::game_rva_named(
            UPDATE_TROPHY_STATS_RVA as u32,
            "UPDATE_TROPHY_STATS_RVA",
        ),
    )
    else {
        return DepositBackResult::Skipped {
            reason: "inventory transfer functions have no verified address for this build",
        };
    };
    let get_main_inventory: GetEquipInventoryDataFn =
        unsafe { std::mem::transmute(get_main_inventory_addr) };
    let get_storage_inventory: GetMainPlayerStorageBoxInventoryFn =
        unsafe { std::mem::transmute(get_storage_inventory_addr) };
    let get_quantity: GetQuantityByItemIdFn = unsafe { std::mem::transmute(get_quantity_addr) };
    let get_item_idx: GetItemInventoryIdxFn = unsafe { std::mem::transmute(get_item_idx_addr) };
    let change_amount_in_box: ChangeAmountInBoxFn =
        unsafe { std::mem::transmute(change_amount_addr) };
    let transfer_item: TransferItemBetweenInventoryDatasFn =
        unsafe { std::mem::transmute(transfer_item_addr) };
    let update_trophy_stats: UpdateTrophyStatsFn =
        unsafe { std::mem::transmute(update_trophy_addr) };

    let main_inventory = unsafe { get_main_inventory(equip_game_data) };
    if main_inventory == 0 {
        return DepositBackResult::Skipped {
            reason: "missing main EquipInventoryData",
        };
    }
    let storage_inventory = unsafe { get_storage_inventory() };
    if storage_inventory == 0 {
        return DepositBackResult::Skipped {
            reason: "missing storage EquipInventoryData",
        };
    }

    let carried_before = unsafe { get_quantity(main_inventory, &mut item_id) };
    if carried_before <= 0 {
        return DepositBackResult::Skipped {
            reason: "item not carried",
        };
    }
    let source_item_idx = unsafe { get_item_idx(main_inventory, &mut item_id) };
    if source_item_idx < 0 {
        return DepositBackResult::Skipped {
            reason: "source item index not found",
        };
    }

    let mut moved =
        unsafe { change_amount_in_box(storage_inventory, &mut item_id, carried_before) };
    if moved <= 0 {
        return DepositBackResult::Skipped {
            reason: "storage cannot accept item",
        };
    }

    let source_item_equipped = unsafe { is_equipped_item_idx(equip_game_data, source_item_idx) };
    let mut kept_one_equipped = false;
    if source_item_equipped && moved >= carried_before {
        moved = carried_before.saturating_sub(1);
        kept_one_equipped = true;
    }
    if moved <= 0 {
        return DepositBackResult::Skipped {
            reason: "equipped item must keep one carried",
        };
    }

    unsafe {
        transfer_item(
            source_item_idx,
            main_inventory,
            storage_inventory,
            moved,
            false,
        )
    };
    unsafe { update_trophy_stats(equip_game_data, &mut item_id) };

    DepositBackResult::Deposited {
        moved,
        carried_before,
        source_item_idx,
        kept_one_equipped,
    }
}

#[cfg(windows)]
unsafe fn is_equipped_item_idx(equip_game_data: usize, item_idx: i32) -> bool {
    for slot in 0..EQUIPMENT_ITEM_IDX_LIST_LEN {
        let addr = equip_game_data + EQUIP_GAME_DATA_EQUIPMENT_ITEM_IDX_LIST_OFFSET + slot * 4;
        let equipped_idx = unsafe { (addr as *const i32).read_unaligned() };
        if equipped_idx == item_idx {
            return true;
        }
    }
    false
}

#[cfg(windows)]
fn call_native_refill(source: &str) -> bool {
    let base = GAME_BASE.load(Ordering::SeqCst);
    if base == 0 {
        log_message(format_args!(
            "{source}: skipped native ReplanishItemsFromChest: missing game base"
        ));
        return false;
    }
    if unsafe { resolve_item_replenish_state_tracker() }.is_none() {
        let skipped = SKIPPED_NO_TRACKER.fetch_add(1, Ordering::SeqCst) + 1;
        if skipped <= 8 || skipped.is_multiple_of(32) {
            log_message(format_args!(
                "{source}: skipped native ReplanishItemsFromChest: no ItemReplenishStateTracker (skipped_no_tracker={skipped})"
            ));
        }
        return false;
    }

    type ReplanishItemsFromChestFn = unsafe extern "system" fn();
    let Ok(refill_addr) = er_game_base::mem::game_rva_named(
        REPLANISH_ITEMS_FROM_CHEST_RVA as u32,
        "REPLANISH_ITEMS_FROM_CHEST_RVA",
    ) else {
        return false;
    };
    let refill: ReplanishItemsFromChestFn = unsafe { std::mem::transmute(refill_addr) };
    unsafe { refill() };
    true
}

#[cfg(windows)]
unsafe fn resolve_player_game_data() -> Option<usize> {
    let base = GAME_BASE.load(Ordering::SeqCst);
    let game_data_man = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            GAME_DATA_MAN_GLOBAL_RVA,
            "GAME_DATA_MAN_GLOBAL_RVA",
        ))
    }?;
    unsafe { safe_read_usize(game_data_man + 0x8) }
        .filter(|&player_game_data| player_game_data != 0)
}

#[cfg(windows)]
unsafe fn resolve_item_replenish_state_tracker() -> Option<usize> {
    let player_game_data = unsafe { resolve_player_game_data() }?;
    unsafe { safe_read_usize(player_game_data + PLAYER_GAME_DATA_ITEM_REPLENISH_TRACKER_OFFSET) }
        .filter(|&tracker| tracker != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rvas_match_er_1162_static_re() {
        assert_eq!(SET_ITEM_REPLENISH_STATE_RVA, 0x786430);
        assert_eq!(REPLANISH_ITEMS_FROM_CHEST_RVA, 0x24dff0);
        assert_eq!(SHOULD_REPLENISH_ITEM_RVA, 0x23d990);
        assert_eq!(IS_ITEM_REPLANISH_FROM_CHEST_REQUESTED_RVA, 0x67a050);
        assert_eq!(MOVE_MAP_STEP_UPDATE_PLAYER_INFO_RVA, 0xafc160);
        assert_eq!(BONFIRE_FIRST_LVUP_RVA, 0x59c1e0);
        assert_eq!(GET_EQUIP_INVENTORY_DATA_RVA, 0x247b30);
        assert_eq!(GET_MAIN_PLAYER_STORAGE_BOX_INVENTORY_RVA, 0x786810);
        assert_eq!(GET_QUANTITY_BY_ITEM_ID_RVA, 0x24c1b0);
        assert_eq!(GET_ITEM_INVENTORY_IDX_RVA, 0x24c560);
        assert_eq!(CHANGE_AMOUNT_IN_BOX_RVA, 0x24e3d0);
        assert_eq!(TRANSFER_ITEM_BETWEEN_INVENTORY_DATAS_RVA, 0x24db90);
        assert_eq!(UPDATE_TROPHY_STATS_RVA, 0x24a1a0);
        assert_eq!(PLAYER_GAME_DATA_EQUIP_GAME_DATA_OFFSET, 0x2b0);
        assert_eq!(PLAYER_GAME_DATA_ITEM_REPLENISH_TRACKER_OFFSET, 0x5e8);
        assert_eq!(EQUIP_GAME_DATA_EQUIPMENT_ITEM_IDX_LIST_OFFSET, 0x8);
        assert_eq!(EQUIPMENT_ITEM_IDX_LIST_LEN, 22);
    }
}
