//! DIRECT in-process input injection -- the VERIFIED ER input lever (user-confirmed 2026-07-19; the
//! SendInput/XInput/window-focus path was a DEAD END). No OS input is synthesized: the game's own
//! input memory is written on the game thread each frame.
//!
//! Two writes, both ported verbatim from the product with their exact reverse-engineered addresses:
//!
//!  1. MENU EVENTS -- the front-end/menu reads a KEYSTATE BITMAP at `inputmgr+0x90+eventId`, edge-
//!     triggered (`&1`). `inputmgr = *(base + 0x3d6b7b0)` (CSMenuMan / SelectBot input manager). Tap
//!     an event by OR-ing bit0 into `inputmgr+0x90+eventId`; the bitmap is re-polled every frame, so
//!     assert for a couple frames then gap for a clean single edge (no auto-repeat). Verified event
//!     ids (RE 2026-06-17, `frontend-menu-input-injection-ids-2026`): vertical-move = 0x00 AND 0x45
//!     (inject both; only Down advances, Up saturates), Confirm/OK = 0x3d. Mirrors the product's
//!     `menu_input_probe` (crates/er-effects-rs/src/experiments/continue_load/product_continue.rs).
//!
//!  2. STAY-ACTIVE (unfocused input) -- ER clears `[DLUID+0x88d]` every frame it is not
//!     `GetActiveWindow`; re-setting it to 1 lets the injected input apply while the window is
//!     UNFOCUSED (bd `breakthrough-pad-boundary-injection-moves-char-needs-focus`). `DLUID =
//!     *(base + 0x485dc18)` (input-device manager). This is why the direct path needs no window focus.
//!
//! Both writes are guarded by a fault-safe readability probe first, so a not-yet-initialized singleton
//! pointer can never fault the game thread.

use crate::log::harness_log;
use crate::win32::{read_u8, read_u32, read_usize, write_u32};

/// `inputmgr`/CSMenuMan singleton RVA (`SELECTBOT_INPUT_MANAGER_GLOBAL_RVA` /
/// `GLOBAL_CSMENUMAN_RVA` in the product constant tree).
const INPUT_MANAGER_GLOBAL_RVA: usize = 0x3d6b7b0;
/// Boolean shop-category context slots consumed by `FUN_140784380` when filtering weapon
/// strengthen rows. Static RE found only reads in that filter and clear-writes in the shared Gaitem
/// constructors through `GLOBAL_CSMenuMan`, so the deterministic harness may seed them as probe
/// context; fresh selected-row telemetry remains the proof that the seed actually produced a weapon
/// row.
const CSMENU_REINFORCE_SHOP_CATEGORY_1_OFFSET: usize = 0x47c;
const CSMENU_REINFORCE_SHOP_CATEGORY_2_OFFSET: usize = 0x480;
const CSMENU_REINFORCE_SHOP_CATEGORY_3_OFFSET: usize = 0x484;
const CSMENU_REINFORCE_SHOP_CATEGORY_4_OFFSET: usize = 0x488;
const DETERMINISTIC_REINFORCE_SHOP_CATEGORY_ENABLED: u32 = 1;
/// Keystate bitmap base within the input manager (`INPUTMGR_BITMAP_90_OFFSET`).
const INPUTMGR_BITMAP_90_OFFSET: usize = 0x90;
/// Edge bit written per event (`MENU_EVENT_PRESSED_BIT`).
const MENU_EVENT_PRESSED_BIT: u8 = 1;

/// DLUID (input-device manager) singleton RVA (`RuntimeGlobalRva::DluidInputManager`).
const DLUID_SINGLETON_RVA: usize = 0x485dc18;
/// Input-active flag offset within DLUID (`DLUID_INPUT_ACTIVE_FLAG_OFFSET`).
const DLUID_INPUT_ACTIVE_FLAG_OFFSET: usize = 0x88d;

const HEAP_LO: usize = 0x10000;

/// Verified front-end/menu event ids (see module doc). No reversed id exists for the OptionSetting
/// tab-switch -- that is mouse-only on native and is the known self-drive gap.
#[derive(Clone, Copy)]
pub enum MenuEvent {
    /// Vertical move DOWN (id 0x00) and UP (id 0x45) -- verified vertical-move ids. Injected singly now
    /// (directional), so nav can stop on a middle row instead of saturating an extreme.
    MoveDown,
    MoveUp,
    /// OptionSetting tab-switch: LEFT/prev tab (id 0x30) and RIGHT/next tab (id 0x31). RE 2026-07-22
    /// (bd MENU-GAPS-CLOSED): GridControl pager FUN_1407392f0 -> tab handler FUN_14093b760.
    TabLeft,
    TabRight,
    Confirm,
    /// Modal-dialog OK/accept (id 0x01). Consumed ONLY by the dialog builder FUN_140e9a920 while a modal
    /// CS dialog (connection-error / offline-notice / save-data / ToS popup) is up; a no-op otherwise.
    /// Tapped EVERY frame to generally accept the 0-N boot-flow popups that block Continue. bd
    /// HARNESS-must-tap-dialog-OK-0x01-every-frame-2026-07-22.
    PopupAccept,
}

impl MenuEvent {
    const fn id(self) -> usize {
        match self {
            MenuEvent::MoveDown => 0x00,
            MenuEvent::MoveUp => 0x45,
            MenuEvent::TabLeft => 0x30,
            MenuEvent::TabRight => 0x31,
            MenuEvent::Confirm => 0x3d,
            MenuEvent::PopupAccept => 0x01,
        }
    }
}

/// CSMenuManImp.popupMenu (+0x80) and its request-open-IngameTop flag (+0x121). RE 2026-07-22.
const CS_MENU_MAN_POPUP_MENU_80_OFFSET: usize = 0x80;
const POPUP_MENU_REQUEST_OPEN_INGAME_TOP_121_OFFSET: usize = 0x121;
/// The in-world menu-open guard id: opening IngameTop is only honored while `inputmgr+0x90+0x1c & 1 == 0`
/// (the raw-pad Options press is read elsewhere; this is the equivalent of the native open fn's guard).
const MENU_OPEN_GUARD_EVENT_ID: usize = 0x1c;

/// Request the in-world pause/System menu (02_000_IngameTop) to open, the equivalent one-shot effect of
/// the native open fn (deobf 0x7ede50): set `popupMenu+0x121 = 1`. Fault-safe; game-thread only. Returns
/// true once the request was written. Gated on the same guard the native fn uses so it is a no-op when a
/// menu is already up. bd MENU-GAPS-CLOSED-tabswitch...pausemenu-open-2026-07-22.
pub fn request_open_ingame_menu(input_manager_ptr: usize) -> bool {
    let guard = input_manager_ptr + INPUTMGR_BITMAP_90_OFFSET + MENU_OPEN_GUARD_EVENT_ID;
    if unsafe { read_u8(guard) }.is_none_or(|g| g & 1 != 0) {
        return false;
    }
    let Some(popup) = (unsafe { read_usize(input_manager_ptr + CS_MENU_MAN_POPUP_MENU_80_OFFSET) })
        .filter(|p| *p >= HEAP_LO)
    else {
        return false;
    };
    let req = popup + POPUP_MENU_REQUEST_OPEN_INGAME_TOP_121_OFFSET;
    if unsafe { read_u8(req) }.is_none() {
        return false;
    }
    // SAFETY: confirmed-readable byte in the live CSPopupMenu; +0x121 is the request-open-IngameTop
    // flag CSPopupMenu::Update consumes next frame (RE 2026-07-22).
    unsafe {
        *(req as *mut u8) = 1;
    }
    true
}

/// Resolve the dereferenced input-manager pointer, or `None` before it is initialized.
pub fn input_manager(base: usize) -> Option<usize> {
    unsafe { read_usize(base + INPUT_MANAGER_GLOBAL_RVA) }.filter(|p| *p >= HEAP_LO)
}

/// Seed the blacksmith-style weapon strengthen category gates for the deterministic `upgrade_det`
/// harness. The native shared Gaitem constructors clear these slots while opening the shell, so the
/// caller deliberately writes before and after the native opener. This is harness-only setup, not
/// product behavior: the selected-row hook must still observe a fresh weapon row after the shell opens
/// before any confirm dialog can advance.
pub fn seed_reinforce_shop_categories_for_probe(base: usize) -> bool {
    let Some(menu) = input_manager(base) else {
        return false;
    };
    let slots = [
        CSMENU_REINFORCE_SHOP_CATEGORY_1_OFFSET,
        CSMENU_REINFORCE_SHOP_CATEGORY_2_OFFSET,
        CSMENU_REINFORCE_SHOP_CATEGORY_3_OFFSET,
        CSMENU_REINFORCE_SHOP_CATEGORY_4_OFFSET,
    ];
    slots.into_iter().all(|offset| unsafe {
        write_u32(menu + offset, DETERMINISTIC_REINFORCE_SHOP_CATEGORY_ENABLED)
    })
}

// --- NATIVE EquipTop open (bd er-effects-rs-pe98, RE 2026-07-23) ---
// The pause list opens submenus exclusively through MenuJob FACTORIES + CSPopupMenu job submit;
// there is NO request byte for Equipment (the +0x121/+0x122 request family covers only
// IngameTop/WorldMap). Equipment row = st_pauseMenuClickHandlerInfoList[0], factory dump
// 0x140801cb0 (builds the 02_010_EquipTop union job from popup+0x10 alone); submit wrapper dump
// 0x1407ee2e0 is the SAME one the proven +0x121 IngameTop request path uses (core
// CSPopupMenu::StartTopMenuJob dump 0x1407f0c40 pushes the current top job to popup+0xD0 so Back
// pops natively, and bumps the job serial at popup+0x168). All deobf VAs ground-truthed
// content-unique via scripts/dump-deobf-shift.py.

/// `FUN_140801cb0` deobf: Equipment pause-row MenuJob factory
/// `(DLReferencePointer<CS::MenuJob>* out, ComponentStack* popup+0x10) -> out`.
const EQUIP_TOP_JOB_FACTORY_RVA: usize = 0x801bc0;
/// `InventoryUiLoad` deobf (dump 0x140801e40): Inventory pause-row MenuJob factory,
/// same `(out, ComponentStack*)` signature -- builds the 02_020_Inventory union job.
const INVENTORY_JOB_FACTORY_RVA: usize = 0x801d50;
/// Native weapon-reinforcement/upgrade open wrapper. Live disasm at `0x140e9da60` calls the
/// `FUN_14080ddd0` builder/submit path, writes `CurrentOpenMenu=0x17`, then calls
/// `IsOpenMenuJobCurrentTop`. Use this instead of direct factory submit so the semantic open-menu
/// semaphore is native.
const WEAPON_UPGRADE_OPEN_WRAPPER_RVA: usize = 0xe9da60;
/// `FUN_1407ee2e0` deobf: popup top-job submit wrapper `(popup, refptr* out, u64* serial_out,
/// refptr* job)` -- the exact call shape of the +0x121 IngameTop open path.
const POPUP_SUBMIT_TOP_JOB_RVA: usize = 0x7ee1f0;
/// CSPopupMenu.componentStack used by every pause-row factory.
const POPUP_COMPONENT_STACK_10_OFFSET: usize = 0x10;
/// CSPopupMenu top-job submit serial; increments per StartTopMenuJob -- a clean open semaphore.
const POPUP_JOB_SERIAL_168_OFFSET: usize = 0x168;

fn popup_menu(input_manager_ptr: usize) -> Option<usize> {
    unsafe { read_usize(input_manager_ptr + CS_MENU_MAN_POPUP_MENU_80_OFFSET) }
        .filter(|p| *p >= HEAP_LO)
}

/// Read the CSPopupMenu job-submit serial (popup+0x168), or 0 when unresolvable.
pub fn popup_job_serial(input_manager_ptr: usize) -> u64 {
    popup_menu(input_manager_ptr)
        .and_then(|popup| unsafe { read_usize(popup + POPUP_JOB_SERIAL_168_OFFSET) })
        .unwrap_or(0) as u64
}

/// NATIVE top-menu open: build a pause-row MenuJob with the game's own factory (Equipment or
/// Inventory -- identical `(out, ComponentStack*)` signature) and submit it through the native
/// CSPopupMenu top-job path (native enqueue + native pump ownership; no Scaleform input). Faithful
/// nesting requires the pause menu (IngameTop) to already be the top job -- call only after
/// `pause_menu_open()`. Game thread only. Returns true once the job was built and submitted.
fn native_open_top_menu(base: usize, input_manager_ptr: usize, factory_rva: usize) -> bool {
    type JobFactoryFn = unsafe extern "system" fn(*mut [usize; 2], usize) -> *mut [usize; 2];
    type SubmitTopJobFn =
        unsafe extern "system" fn(usize, *mut [usize; 2], *mut u64, *mut [usize; 2]);

    let Some(popup) = popup_menu(input_manager_ptr) else {
        return false;
    };
    let factory: JobFactoryFn = unsafe { std::mem::transmute(base + factory_rva) };
    let submit: SubmitTopJobFn = unsafe { std::mem::transmute(base + POPUP_SUBMIT_TOP_JOB_RVA) };

    let mut job: [usize; 2] = [0; 2];
    // SAFETY: the factory constructs a DLReferencePointer<MenuJob> into raw 16-byte out storage
    // from popup+0x10, exactly as every native pause-row confirm does (RE 2026-07-23).
    unsafe { factory(&mut job, popup + POPUP_COMPONENT_STACK_10_OFFSET) };
    if job[0] < HEAP_LO {
        return false;
    }
    let mut out: [usize; 2] = [0; 2];
    let mut serial: u64 = 0;
    // SAFETY: same call shape as the native +0x121 IngameTop open (popup, &out, &serial, &job);
    // the core pushes the current top job to popup+0xD0 so Back pops natively. The job refptr's
    // one retained reference is intentionally left alive (the menu owns the job's lifetime).
    unsafe { submit(popup, &mut out, &mut serial, &mut job) };
    true
}

/// Native open of the EquipTop menu (equipped-slot summary + slot-selection grids).
pub fn native_open_equip_menu(base: usize, input_manager_ptr: usize) -> bool {
    native_open_top_menu(base, input_manager_ptr, EQUIP_TOP_JOB_FACTORY_RVA)
}

/// Native open of the Inventory menu (02_020_Inventory -- the Melee/Ranged/Shields tabs whose item
/// cells carry the bottom-left ArtsIcon child, bd er-effects-rs-pe98 GFX geometry).
pub fn native_open_inventory_menu(base: usize, input_manager_ptr: usize) -> bool {
    native_open_top_menu(base, input_manager_ptr, INVENTORY_JOB_FACTORY_RVA)
}

/// Native open of the weapon-reinforcement/upgrade menu through the same wrapper the invoke-case path
/// uses, so `CurrentOpenMenu` is set by native code.
pub fn native_open_weapon_upgrade_menu(base: usize, input_manager_ptr: usize) -> bool {
    type WeaponUpgradeOpenFn = unsafe extern "system" fn(*mut [usize; 3]);

    if popup_menu(input_manager_ptr).is_none() {
        return false;
    }
    let open_weapon_upgrade: WeaponUpgradeOpenFn =
        unsafe { std::mem::transmute(base + WEAPON_UPGRADE_OPEN_WRAPPER_RVA) };
    let mut open_menu_job: [usize; 3] = [0; 3];
    // SAFETY: live disassembly shows the wrapper writes an OpenMenuJob-like 24-byte out struct at RCX
    // and performs the same build/submit + CurrentOpenMenu update as the event invoke case.
    unsafe { open_weapon_upgrade(&mut open_menu_job) };
    open_menu_job[0] >= HEAP_LO
}

/// CT `ER_TGA_v1.16.1.CT` ItemGive target (`AOB 8B 02 83 F8 0A - 0x52`), Ghidra-named
/// `CS::MapItemManImpl::GiveItems(MapItemManImpl*, ItemLotSpawnDataList*, ItemLotSpawnDataStatus*,
/// DL_BOOL)`. This is a harness-only deterministic setup lever, not product behavior.
const GIVE_ITEMS_RVA: usize = 0x5605b0;
/// CT MapItemMan singleton AOB (`48 8B 0D ?? ?? ?? ?? C7 44 24 50 FF FF FF FF`).
const MAP_ITEM_MAN_GLOBAL_RVA: usize = 0x3d67a50;
/// CT AddSoul AOB (`44 8B ?? ?? 45 33 ?? 44 89 5C 24`), Ghidra-named `AddRunes`.
const ADD_RUNES_RVA: usize = 0x25e100;
/// Repo-canonical `PlayerGameData::rune_count` offset (see `player_correctness.rs`). Read this before
/// calling `AddRunes` so a bad `PlayerGameData*` fails closed instead of crashing at the function's
/// first dereference.
const PLAYER_GAME_DATA_RUNE_COUNT_OFFSET: usize = 0x6c;

const DETERMINISTIC_DAGGER_ID: i32 = 0x000f4240;
const SMITHING_STONE_1_RAW_GOODS_ID: i32 = 0x2774;
const NO_GEM_ID: i32 = -1;

#[repr(C)]
#[derive(Clone, Copy)]
struct ItemLotSpawnDataItemEntry {
    item_id: i32,
    quantity: i32,
    upgrade: i32,
    gem_id: i32,
}

#[repr(C)]
struct ItemLotSpawnDataList {
    count: u32,
    items: [ItemLotSpawnDataItemEntry; 10],
}

#[repr(C)]
#[derive(Default)]
struct ItemLotSpawnDataStatus {
    fields: [u32; 3],
}

/// Deterministically seed a +0 Dagger, Smithing Stone [1] material, and enough runes for a first
/// strengthen attempt. The item table layout comes from Ghidra `ItemLotSpawnDataList` and the IDs from
/// the CT dropdown / `ItemGive` helper, which strips the goods category base before calling `GiveItems`.
/// Rune setup uses the repo-canonical `GameDataMan` global, not the CT helper's stale singleton slot.
pub fn grant_deterministic_strengthen_seed(base: usize) -> bool {
    type GiveItemsFn = unsafe extern "system" fn(
        *mut core::ffi::c_void,
        *mut ItemLotSpawnDataList,
        *mut ItemLotSpawnDataStatus,
        u32,
    );
    type AddRunesFn = unsafe extern "system" fn(*mut core::ffi::c_void, i32) -> i32;
    let Some(map_item_man) = unsafe { read_usize(base + MAP_ITEM_MAN_GLOBAL_RVA) }
        .filter(|p| *p >= HEAP_LO)
        .map(|p| p as *mut core::ffi::c_void)
    else {
        return false;
    };
    let Some(game_data_man) =
        (unsafe { read_usize(base + er_game_base::rva::GAME_DATA_MAN_GLOBAL_RVA) })
            .filter(|p| *p >= HEAP_LO)
    else {
        return false;
    };
    let Some(player_game_data) = (unsafe {
        read_usize(game_data_man + er_game_base::rva::GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET)
    })
    .filter(|p| *p >= HEAP_LO) else {
        return false;
    };
    let Some(runes_before) =
        (unsafe { read_u32(player_game_data + PLAYER_GAME_DATA_RUNE_COUNT_OFFSET) })
    else {
        harness_log!(
            "deterministic-strengthen: refusing AddRunes because PlayerGameData rune_count unreadable pgd=0x{player_game_data:x}"
        );
        return false;
    };

    let mut list = ItemLotSpawnDataList {
        count: 2,
        items: [ItemLotSpawnDataItemEntry {
            item_id: 0,
            quantity: 0,
            upgrade: -1,
            gem_id: NO_GEM_ID,
        }; 10],
    };
    list.items[0] = ItemLotSpawnDataItemEntry {
        item_id: DETERMINISTIC_DAGGER_ID,
        quantity: 1,
        upgrade: 0,
        gem_id: NO_GEM_ID,
    };
    list.items[1] = ItemLotSpawnDataItemEntry {
        item_id: SMITHING_STONE_1_RAW_GOODS_ID,
        quantity: 10,
        upgrade: -1,
        gem_id: NO_GEM_ID,
    };
    let mut status = ItemLotSpawnDataStatus::default();
    let give_items: GiveItemsFn = unsafe { std::mem::transmute(base + GIVE_ITEMS_RVA) };
    let add_runes: AddRunesFn = unsafe { std::mem::transmute(base + ADD_RUNES_RVA) };

    // SAFETY: The callees and struct layout are resolved from CT + Ghidra decompile. The live crash on
    // 2026-07-24 was caused by using the CT helper's stale GameDataMan global for the first argument to
    // `AddRunes`; this now uses the repo-canonical singleton and a fault-safe rune-count readability
    // guard before calling into the native function.
    let runes_added = unsafe {
        give_items(map_item_man, &mut list, &mut status, 0);
        add_runes(player_game_data as *mut core::ffi::c_void, 10_000)
    };
    harness_log!(
        "deterministic-strengthen: seeded dagger=0x{DETERMINISTIC_DAGGER_ID:x} upgrade=0 smithing_stone_1=0x{SMITHING_STONE_1_RAW_GOODS_ID:x}x10 runes_before={runes_before} runes_added={runes_added} status={:x?}",
        status.fields
    );
    true
}

/// Tap one menu event into the keystate bitmap (edge OR). Fault-safe: only writes once the target
/// byte is confirmed readable. Must be called on the game thread (from the per-frame drive hook) so
/// the write lands in the same frame the game re-polls the bitmap.
pub fn tap_menu_event(input_manager_ptr: usize, event: MenuEvent) {
    let addr = input_manager_ptr + INPUTMGR_BITMAP_90_OFFSET + event.id();
    if unsafe { read_u8(addr) }.is_none() {
        return;
    }
    // SAFETY: `addr` is a confirmed-readable byte inside the live input manager; OR-ing the edge bit
    // is exactly what the native input producer does at 0x1407ad509.
    unsafe {
        *(addr as *mut u8) |= MENU_EVENT_PRESSED_BIT;
    }
}

/// Re-set `[DLUID+0x88d] = 1` so injected input applies while the ER window is UNFOCUSED. Fault-safe;
/// call every frame from the drive hook (ER clears it each unfocused frame). Returns true once the
/// flag was written at least once (for logging).
pub fn keep_input_active(base: usize) -> bool {
    let Some(dluid) = (unsafe { read_usize(base + DLUID_SINGLETON_RVA) }).filter(|p| *p >= HEAP_LO)
    else {
        return false;
    };
    let flag = dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET;
    if unsafe { read_u8(flag) }.is_none() {
        return false;
    }
    // SAFETY: confirmed-readable flag byte inside the live DLUID singleton.
    unsafe {
        *(flag as *mut u8) = 1;
    }
    true
}

/// Title global accept byte RVA (`TITLE_GLOBAL_ACCEPT_BYTE_RVA` in the product constant tree). PRESS
/// ANY BUTTON is read on the raw-pad layer, NOT the keystate bitmap; the game's own
/// `TitleTopDialog::update` accept-gate advances the parked press-any-button title when this byte is 1
/// (bd title-global-accept-byte-144589bdc-zeroinput-advance). This is the decoded accept flag, not an
/// OS input event -- the harness sets it to blow through PRESS ANY BUTTON and open the title menu.
const TITLE_GLOBAL_ACCEPT_BYTE_RVA: usize = 0x4589bdc;

/// Set the title global accept byte = 1 to advance the parked PRESS ANY BUTTON title into its menu.
/// Fault-safe; game-thread only. Returns true once written. A no-op effect once past the title.
pub fn advance_press_any_button(base: usize) -> bool {
    let addr = base + TITLE_GLOBAL_ACCEPT_BYTE_RVA;
    if unsafe { read_u8(addr) }.is_none() {
        return false;
    }
    // SAFETY: confirmed-readable byte in the mapped game image; this is the product's own accept-byte
    // write (product_autoload_gates.rs: `*(base + TITLE_GLOBAL_ACCEPT_BYTE_RVA) = 1`).
    unsafe {
        *(addr as *mut u8) = 1;
    }
    true
}

/// Log the resolved singletons once, for the evidence trail.
pub fn log_resolution(base: usize) {
    harness_log!(
        "input-inject: base=0x{base:x} input_manager=0x{:x} dluid_present={} (direct keystate-bitmap + DLUID stay-active channel; no SendInput/XInput)",
        input_manager(base).unwrap_or(0),
        (unsafe { read_usize(base + DLUID_SINGLETON_RVA) })
            .filter(|p| *p >= HEAP_LO)
            .is_some() as u8
    );
}
