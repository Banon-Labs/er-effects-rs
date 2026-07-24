//! Standalone storage-box autorefill improvement DLL.
//!
//! Vanilla Elden Ring only flips persistent autorefill state when the user toggles an item in the
//! storage box. The actual storage -> personal inventory transfer happens later through
//! `ReplanishItemsFromChest`. This DLL hooks the native toggle function and, when the new native
//! state is enabled, calls the game's own refill routine immediately. That preserves the game's
//! item eligibility, stack/capacity, storage removal, and unlimited-consumables gates.

#![allow(non_snake_case)]

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
    rva::GAME_DATA_MAN_GLOBAL_RVA,
};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;

const LOG_FILE_NAME: &str = "er-better-refills.log";

/// `SetItemReplenishState(int *itemId)`, called by the DepositoryDialog toggle handler.
/// Static RE / 1.16.2 MCP: `FUN_1408d87d0 -> SetItemReplenishState`.
const SET_ITEM_REPLENISH_STATE_RVA: usize = 0x786430;
/// `ReplanishItemsFromChest()`: native storage -> personal inventory refill loop.
const REPLANISH_ITEMS_FROM_CHEST_RVA: usize = 0x24dff0;
/// `ItemReplenishStateTracker::ShouldReplenishItem(tracker, int *itemId)`.
const SHOULD_REPLENISH_ITEM_RVA: usize = 0x23d990;
/// `IsItemReplanishFromChestRequested()`: reads `GameMan->itemReplanishFromChestRequested`.
const IS_ITEM_REPLANISH_FROM_CHEST_REQUESTED_RVA: usize = 0x67a050;
/// `CS::MoveMapStep::UpdatePlayerInfo(this)`: native post-load player handoff. The function tail
/// already consumes the vanilla chest-refill request flag, so a post-hook here is the load-complete
/// point for deaths, map warps, continue/load, and other MoveMapStep-backed loading screens.
const MOVE_MAP_STEP_UPDATE_PLAYER_INFO_RVA: usize = 0xafc160;
/// `CS::CSLuaEventScriptImitation::OnEvent_BonfireFirstLvUp(this, proxy)`: first grace activation.
const BONFIRE_FIRST_LVUP_RVA: usize = 0x59c1e0;

/// `GameDataMan -> PlayerGameData` is shared in `er-game-base`; within `PlayerGameData`,
/// the native `ItemReplenishStateTracker*` used by SetItemReplenishState sits at +0x5e8.
const PLAYER_GAME_DATA_ITEM_REPLENISH_TRACKER_OFFSET: usize = 0x5e8;

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
            loop {
                match game_module_base() {
                    Ok(base) => {
                        GAME_BASE.store(base, Ordering::SeqCst);
                        install_better_refills_hooks(base);
                        break;
                    }
                    Err(err) => {
                        if attempts == 0 || attempts % 4096 == 0 {
                            log_message(format_args!(
                                "install: waiting for game module base: {err}"
                            ));
                        }
                        attempts = attempts.saturating_add(1);
                        std::thread::yield_now();
                    }
                }
            }
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

    for (name, target, detour, orig_slot) in targets {
        let hook = match unsafe { MhHook::new(target as *mut c_void, detour) } {
            Ok(hook) => hook,
            Err(status) => {
                log_message(format_args!(
                    "install: MhHook::new({name} @0x{target:x}) failed: {status:?}"
                ));
                return;
            }
        };
        orig_slot.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            log_message(format_args!(
                "install: queue_enable({name} @0x{target:x}) failed: {status:?}"
            ));
            return;
        }
    }

    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            HOOK_STATE.store(HOOK_ACTIVE, Ordering::SeqCst);
            log_message(format_args!(
                "install: hooks ACTIVE SetItemReplenishState @0x{:x}, MoveMapStep::UpdatePlayerInfo @0x{:x}, OnEvent_BonfireFirstLvUp @0x{:x}; native refill rva=0x{REPLANISH_ITEMS_FROM_CHEST_RVA:x}",
                base + SET_ITEM_REPLENISH_STATE_RVA,
                base + MOVE_MAP_STEP_UPDATE_PLAYER_INFO_RVA,
                base + BONFIRE_FIRST_LVUP_RVA,
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
    let should_replenish: ShouldReplenishItemFn =
        unsafe { std::mem::transmute(base + SHOULD_REPLENISH_ITEM_RVA) };
    if !unsafe { should_replenish(tracker, item_id) } {
        let skipped = SKIPPED_DISABLED_AFTER_TOGGLE.fetch_add(1, Ordering::SeqCst) + 1;
        if skipped <= 8 || skipped % 32 == 0 {
            let raw_item_id = unsafe { item_id.read_unaligned() };
            log_message(format_args!(
                "toggle#{call_index}: item=0x{raw_item_id:08x} disabled after toggle; no refill (skipped_disabled={skipped})"
            ));
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
        let is_requested: IsItemReplanishFromChestRequestedFn =
            unsafe { std::mem::transmute(base + IS_ITEM_REPLANISH_FROM_CHEST_REQUESTED_RVA) };
        unsafe { is_requested() }
    };

    let orig = ORIG_MOVE_MAP_STEP_UPDATE_PLAYER_INFO.load(Ordering::SeqCst);
    if orig != 0 {
        let original: unsafe extern "system" fn(*mut core::ffi::c_void) =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(this) };
    }

    if vanilla_request_pending {
        let count = LOAD_COMPLETION_VANILLA_PENDING.fetch_add(1, Ordering::SeqCst) + 1;
        if count <= 8 || count % 32 == 0 {
            log_message(format_args!(
                "load-complete#{count}: vanilla itemReplanishFromChestRequested was pending; original UpdatePlayerInfo consumed native refill"
            ));
        }
        return;
    }

    if call_native_refill("load-complete") {
        let count = LOAD_COMPLETION_REFILLS.fetch_add(1, Ordering::SeqCst) + 1;
        if count <= 8 || count % 32 == 0 {
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
        if count <= 8 || count % 32 == 0 {
            log_message(format_args!(
                "first-grace#{count}: called native ReplanishItemsFromChest after OnEvent_BonfireFirstLvUp"
            ));
        }
    }
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
        if skipped <= 8 || skipped % 32 == 0 {
            log_message(format_args!(
                "{source}: skipped native ReplanishItemsFromChest: no ItemReplenishStateTracker (skipped_no_tracker={skipped})"
            ));
        }
        return false;
    }

    type ReplanishItemsFromChestFn = unsafe extern "system" fn();
    let refill: ReplanishItemsFromChestFn =
        unsafe { std::mem::transmute(base + REPLANISH_ITEMS_FROM_CHEST_RVA) };
    unsafe { refill() };
    true
}

#[cfg(windows)]
unsafe fn resolve_item_replenish_state_tracker() -> Option<usize> {
    let base = GAME_BASE.load(Ordering::SeqCst);
    let game_data_man = unsafe { safe_read_usize(base + GAME_DATA_MAN_GLOBAL_RVA) }?;
    let player_game_data = unsafe { safe_read_usize(game_data_man + 0x8) }?;
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
        assert_eq!(PLAYER_GAME_DATA_ITEM_REPLENISH_TRACKER_OFFSET, 0x5e8);
    }
}
