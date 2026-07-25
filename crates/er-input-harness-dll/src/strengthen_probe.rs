//! Harness-only telemetry for armament strengthen/reinforcement rows.
//!
//! `CurrentOpenMenu == 9` is the normal blacksmith `OpenEnhanceShop(0)` armament-upgrade menu;
//! `0x19` is the limited smithing-table variant. The previous `0x17` path is `OpenBuddyUpgradeMenu`,
//! the shared Spirit Tuning strengthen shell, and must not classify rows as armament upgrades. The
//! selected-row dialog path calls `FUN_140848fa0(selected_menu_gaitem, out)` before building the
//! confirm dialog, so this hook records the authoritative selected `MenuGaitem*` and its item
//! category. It is telemetry only; it does not alter native row/dialog behavior.

#[cfg(windows)]
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

#[cfg(windows)]
use er_hook::{MH_ApplyQueued, MH_STATUS, MhHook};

use crate::game_mem::current_open_menu_id;
use crate::log::harness_log;
use crate::win32::{read_u8, read_u32, read_usize};

const HEAP_LO: usize = 0x1_0000_0000;
/// Deobf/live function start for the selected-row after-forge helper called by dump
/// `FUN_14098df10(param_3, local_220)`. Initial content matching of dump `FUN_140848fa0` landed
/// inside this function at `0x140848eb0`; bounded parent-repo disassembly proves the clean prologue at
/// `0x140848dd0` (`48 89 5c 24 08 ... 55 48 8b ec`). Hook the prologue, never the interior block.
const FORGE_SELECTED_GAITEM_RVA: usize = 0x848dd0;
/// `OpenEnhanceShop(0)` constructs an inventory-backed MenuGaitem list through `FUN_14084d3f0`, which
/// calls this row builder for each `EquipInventoryData` index. Runtime trace
/// `weapon-upgrade-frida-index-source-nocap-20260724-184322` proved a seeded Dagger is inserted at
/// index 2252 and this builder returns `item_id=0xf4240`, `item_type=19`, `CurrentOpenMenu=9` for it.
/// This is the weapon-row source semaphore for armament upgrade availability.
const INVENTORY_INDEX_MENU_GAITEM_RVA: usize = 0x847a20;
/// `OpenEnhanceShop(0)` / `OpenEnhanceShop(1)` also install this per-row transform while constructing
/// a related upgrade/material stream (`FUN_140989620` and `FUN_140989180` store it as a std::function).
/// Runtime traces show it may only see goods/material rows such as `0x400399e2`, so it is supporting
/// telemetry but not sufficient by itself to prove a weapon row exists.
const ARMAMENT_ROW_TRANSFORM_RVA: usize = 0x98f850;

const MENU_GAITEM_ITEM_ID_OFFSET: usize = 0x4c;
const MENU_GAITEM_ITEM_TYPE_OFFSET: usize = 0x54;
const MENU_GAITEM_METADATA_PTR_OFFSET: usize = 0x58;
const MENU_GAITEM_METADATA_ASTRUCT96_OFFSET: usize = 0x8;
const ASTRUCT96_ITEM_CATEGORY_OFFSET: usize = 0x17;
const ARMAMENT_UPGRADE_OPEN_MENU_ID: u32 = 9;
const SMITHING_TABLE_UPGRADE_OPEN_MENU_ID: u32 = 0x19;
const ITEM_ID_CATEGORY_MASK: u32 = 0xf000_0000;
const ITEM_ID_WEAPON_CATEGORY: u32 = 0x0000_0000;
const ITEM_ID_GOODS_CATEGORY: u32 = 0x4000_0000;

#[cfg(windows)]
static ORIG_FORGE_SELECTED_GAITEM: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static ORIG_INVENTORY_INDEX_MENU_GAITEM: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static ORIG_ARMAMENT_ROW_TRANSFORM: AtomicUsize = AtomicUsize::new(0);
static ARMAMENT_ROW_OPEN_MENU_HINT: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_SERIAL: AtomicU64 = AtomicU64::new(0);
static LAST_SELECTED_PTR: AtomicUsize = AtomicUsize::new(0);
static LAST_ITEM_ID: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_ITEM_TYPE: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_ITEM_CATEGORY: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_ITEM_ID_CATEGORY: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_OPEN_MENU: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_WEAPON_ROW_SERIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StrengthenRowKind {
    None,
    Weapon,
    GoodsOrSpirit,
    Other,
}

impl StrengthenRowKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Weapon => "weapon",
            Self::GoodsOrSpirit => "goods_or_spirit",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StrengthenRowSnapshot {
    pub serial: u64,
    pub selected_ptr: usize,
    pub item_id: u32,
    pub item_type: u32,
    pub item_category: u32,
    pub item_id_category: u32,
    pub open_menu: u32,
    pub kind: StrengthenRowKind,
}

impl StrengthenRowSnapshot {
    pub fn current() -> Self {
        let serial = LAST_SERIAL.load(Ordering::SeqCst);
        let selected_ptr = LAST_SELECTED_PTR.load(Ordering::SeqCst);
        let item_id = LAST_ITEM_ID.load(Ordering::SeqCst);
        let item_type = LAST_ITEM_TYPE.load(Ordering::SeqCst);
        let item_category = LAST_ITEM_CATEGORY.load(Ordering::SeqCst);
        let item_id_category = LAST_ITEM_ID_CATEGORY.load(Ordering::SeqCst);
        let open_menu = LAST_OPEN_MENU.load(Ordering::SeqCst);
        let kind = if serial == 0 {
            StrengthenRowKind::None
        } else {
            classify_row(open_menu, item_id_category)
        };
        Self {
            serial,
            selected_ptr,
            item_id,
            item_type,
            item_category,
            item_id_category,
            open_menu,
            kind,
        }
    }
}

pub fn last_row_snapshot() -> StrengthenRowSnapshot {
    StrengthenRowSnapshot::current()
}

pub fn last_selected_row_is_weapon() -> bool {
    StrengthenRowSnapshot::current().kind == StrengthenRowKind::Weapon
}

pub fn armament_weapon_row_seen() -> bool {
    LAST_WEAPON_ROW_SERIAL.load(Ordering::SeqCst) != 0
}

fn read_item_category(selected: usize) -> Option<u32> {
    let metadata = unsafe { read_usize(selected + MENU_GAITEM_METADATA_PTR_OFFSET) }?;
    if metadata < HEAP_LO {
        return None;
    }
    let astruct96 = unsafe { read_usize(metadata + MENU_GAITEM_METADATA_ASTRUCT96_OFFSET) }?;
    if astruct96 < HEAP_LO {
        return None;
    }
    unsafe { read_u8(astruct96 + ASTRUCT96_ITEM_CATEGORY_OFFSET) }.map(u32::from)
}

pub fn begin_armament_upgrade_row_build(open_menu: u32) {
    ARMAMENT_ROW_OPEN_MENU_HINT.store(open_menu, Ordering::SeqCst);
    LAST_WEAPON_ROW_SERIAL.store(0, Ordering::SeqCst);
}

pub fn end_armament_upgrade_row_build() {
    ARMAMENT_ROW_OPEN_MENU_HINT.store(u32::MAX, Ordering::SeqCst);
}

fn read_row_fields(
    selected: usize,
    fallback_open_menu: Option<u32>,
) -> Option<(u32, u32, u32, u32, u32)> {
    if selected < HEAP_LO {
        return None;
    }
    let item_id = unsafe { read_u32(selected + MENU_GAITEM_ITEM_ID_OFFSET) }.unwrap_or(u32::MAX);
    let item_type =
        unsafe { read_u32(selected + MENU_GAITEM_ITEM_TYPE_OFFSET) }.unwrap_or(u32::MAX);
    let item_category = read_item_category(selected).unwrap_or(u32::MAX);
    let item_id_category = item_id & ITEM_ID_CATEGORY_MASK;
    let open_menu = current_open_menu_id()
        .or(fallback_open_menu)
        .unwrap_or(u32::MAX);
    Some((
        item_id,
        item_type,
        item_category,
        item_id_category,
        open_menu,
    ))
}

fn classify_row(open_menu: u32, item_id_category: u32) -> StrengthenRowKind {
    if item_id_category == ITEM_ID_GOODS_CATEGORY {
        StrengthenRowKind::GoodsOrSpirit
    } else if matches!(
        open_menu,
        ARMAMENT_UPGRADE_OPEN_MENU_ID | SMITHING_TABLE_UPGRADE_OPEN_MENU_ID
    ) && item_id_category == ITEM_ID_WEAPON_CATEGORY
    {
        StrengthenRowKind::Weapon
    } else {
        StrengthenRowKind::Other
    }
}

fn record_selected_row(
    selected: usize,
    source: &str,
    fallback_open_menu: Option<u32>,
    counts_as_armament_menu_row: bool,
) {
    let Some((item_id, item_type, item_category, item_id_category, open_menu)) =
        read_row_fields(selected, fallback_open_menu)
    else {
        return;
    };

    LAST_SELECTED_PTR.store(selected, Ordering::SeqCst);
    LAST_ITEM_ID.store(item_id, Ordering::SeqCst);
    LAST_ITEM_TYPE.store(item_type, Ordering::SeqCst);
    LAST_ITEM_CATEGORY.store(item_category, Ordering::SeqCst);
    LAST_ITEM_ID_CATEGORY.store(item_id_category, Ordering::SeqCst);
    LAST_OPEN_MENU.store(open_menu, Ordering::SeqCst);
    let serial = LAST_SERIAL.fetch_add(1, Ordering::SeqCst) + 1;
    let kind = classify_row(open_menu, item_id_category);
    if counts_as_armament_menu_row && kind == StrengthenRowKind::Weapon {
        LAST_WEAPON_ROW_SERIAL.store(serial, Ordering::SeqCst);
    }
    harness_log!(
        "strengthen-row: source={source} serial={serial} armament_menu_row={} kind={} selected=0x{selected:x} item_id=0x{item_id:x} item_type=0x{item_type:x} item_category=0x{item_category:x} item_id_category=0x{item_id_category:x} open_menu=0x{open_menu:x}",
        counts_as_armament_menu_row as u8,
        kind.as_str()
    );
}

#[cfg(windows)]
fn log_row_fields(selected: usize, source: &str, fallback_open_menu: Option<u32>) {
    let Some((item_id, item_type, item_category, item_id_category, open_menu)) =
        read_row_fields(selected, fallback_open_menu)
    else {
        return;
    };
    let kind = classify_row(open_menu, item_id_category);
    harness_log!(
        "strengthen-row: source={source} diagnostic kind={} selected=0x{selected:x} item_id=0x{item_id:x} item_type=0x{item_type:x} item_category=0x{item_category:x} item_id_category=0x{item_id_category:x} open_menu=0x{open_menu:x}",
        kind.as_str()
    );
}

#[cfg(windows)]
unsafe extern "system" fn forge_selected_gaitem_hook(selected: usize, out: usize) -> usize {
    record_selected_row(selected, "selected-helper", None, false);
    let orig = ORIG_FORGE_SELECTED_GAITEM.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { original(selected, out) }
}

#[cfg(windows)]
unsafe extern "system" fn inventory_index_menu_gaitem_hook(out: usize, index: u32) -> usize {
    let orig = ORIG_INVENTORY_INDEX_MENU_GAITEM.load(Ordering::SeqCst);
    let built = if orig == 0 {
        out
    } else {
        let original: unsafe extern "system" fn(usize, u32) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(out, index) }
    };
    let hint = ARMAMENT_ROW_OPEN_MENU_HINT.load(Ordering::SeqCst);
    let fallback_open_menu = (hint != u32::MAX).then_some(hint);
    record_selected_row(
        built,
        "inventory-index-menu-gaitem",
        fallback_open_menu,
        true,
    );
    built
}

#[cfg(windows)]
unsafe extern "system" fn armament_row_transform_hook(row: usize) -> usize {
    let hint = ARMAMENT_ROW_OPEN_MENU_HINT.load(Ordering::SeqCst);
    let fallback_open_menu = (hint != u32::MAX).then_some(hint);
    record_selected_row(row, "armament-row-transform-pre", fallback_open_menu, true);

    let orig = ORIG_ARMAMENT_ROW_TRANSFORM.load(Ordering::SeqCst);
    let transformed = if orig == 0 {
        row
    } else {
        let original: unsafe extern "system" fn(usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(row) }
    };
    log_row_fields(
        transformed,
        "armament-row-transform-post",
        fallback_open_menu,
    );
    transformed
}

#[cfg(windows)]
fn install_one_hook(
    label: &str,
    addr: *mut c_void,
    detour: *mut c_void,
    original_slot: &AtomicUsize,
) -> bool {
    match unsafe { MhHook::new(addr, detour) } {
        Ok(hook) => {
            original_slot.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_ok() {
                std::mem::forget(hook);
                if matches!(unsafe { MH_ApplyQueued() }, MH_STATUS::MH_OK) {
                    harness_log!("strengthen-row: hooked {label} at 0x{:x}", addr as usize);
                    return true;
                }
                harness_log!("strengthen-row: MH_ApplyQueued failed for {label}");
            } else {
                harness_log!("strengthen-row: queue_enable failed for {label}");
            }
        }
        Err(status) => {
            harness_log!(
                "strengthen-row: hook create failed for {label} at 0x{:x}: {:?}",
                addr as usize,
                status
            );
        }
    }
    false
}

#[cfg(windows)]
pub fn install_strengthen_row_hook(base: usize) -> bool {
    let selected_ok = install_one_hook(
        "selected MenuGaitem helper",
        (base + FORGE_SELECTED_GAITEM_RVA) as *mut c_void,
        forge_selected_gaitem_hook as *mut c_void,
        &ORIG_FORGE_SELECTED_GAITEM,
    );
    let inventory_index_ok = install_one_hook(
        "inventory index MenuGaitem builder",
        (base + INVENTORY_INDEX_MENU_GAITEM_RVA) as *mut c_void,
        inventory_index_menu_gaitem_hook as *mut c_void,
        &ORIG_INVENTORY_INDEX_MENU_GAITEM,
    );
    let armament_ok = install_one_hook(
        "armament row transform",
        (base + ARMAMENT_ROW_TRANSFORM_RVA) as *mut c_void,
        armament_row_transform_hook as *mut c_void,
        &ORIG_ARMAMENT_ROW_TRANSFORM,
    );
    selected_ok || inventory_index_ok || armament_ok
}

#[cfg(not(windows))]
pub fn install_strengthen_row_hook(_base: usize) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn set_row(open_menu: u32, item_id_category: u32) {
        LAST_SERIAL.store(1, Ordering::SeqCst);
        LAST_SELECTED_PTR.store(HEAP_LO, Ordering::SeqCst);
        LAST_ITEM_ID.store(item_id_category, Ordering::SeqCst);
        LAST_ITEM_TYPE.store(0, Ordering::SeqCst);
        LAST_ITEM_CATEGORY.store(0, Ordering::SeqCst);
        LAST_ITEM_ID_CATEGORY.store(item_id_category, Ordering::SeqCst);
        LAST_OPEN_MENU.store(open_menu, Ordering::SeqCst);
        LAST_WEAPON_ROW_SERIAL.store(
            if classify_row(open_menu, item_id_category) == StrengthenRowKind::Weapon {
                1
            } else {
                0
            },
            Ordering::SeqCst,
        );
    }

    #[test]
    fn armament_upgrade_rows_require_open_enhance_shop_menu() {
        let _guard = TEST_LOCK.lock().unwrap();
        set_row(ARMAMENT_UPGRADE_OPEN_MENU_ID, ITEM_ID_WEAPON_CATEGORY);
        assert_eq!(
            StrengthenRowSnapshot::current().kind,
            StrengthenRowKind::Weapon
        );

        set_row(SMITHING_TABLE_UPGRADE_OPEN_MENU_ID, ITEM_ID_WEAPON_CATEGORY);
        assert_eq!(
            StrengthenRowSnapshot::current().kind,
            StrengthenRowKind::Weapon
        );

        set_row(0x17, ITEM_ID_WEAPON_CATEGORY);
        assert_eq!(
            StrengthenRowSnapshot::current().kind,
            StrengthenRowKind::Other
        );
        assert!(!armament_weapon_row_seen());
    }

    #[test]
    fn goods_or_spirit_rows_do_not_become_weapons_in_armament_menu() {
        let _guard = TEST_LOCK.lock().unwrap();
        set_row(ARMAMENT_UPGRADE_OPEN_MENU_ID, ITEM_ID_GOODS_CATEGORY);
        assert_eq!(
            StrengthenRowSnapshot::current().kind,
            StrengthenRowKind::GoodsOrSpirit
        );
    }
}
