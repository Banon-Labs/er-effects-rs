//! Harness-only telemetry for the shared strengthen/reinforcement menu.
//!
//! `CurrentOpenMenu == 0x17` only proves the shared strengthen shell opened. Weapon reinforcement,
//! Spirit Tuning, and goods-like rows share that shell. The selected-row dialog path calls
//! `FUN_140848fa0(selected_menu_gaitem, out)` before building the confirm dialog, so this hook records
//! the authoritative selected `MenuGaitem*` and its item category. It is telemetry only; it does not
//! alter native row/dialog behavior.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

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

const MENU_GAITEM_ITEM_ID_OFFSET: usize = 0x4c;
const MENU_GAITEM_ITEM_TYPE_OFFSET: usize = 0x54;
const MENU_GAITEM_METADATA_PTR_OFFSET: usize = 0x58;
const MENU_GAITEM_METADATA_ASTRUCT96_OFFSET: usize = 0x8;
const ASTRUCT96_ITEM_CATEGORY_OFFSET: usize = 0x17;
const STRENGTHEN_OPEN_MENU_ID: u32 = 0x17;
const ITEM_ID_CATEGORY_MASK: u32 = 0xf000_0000;
const ITEM_ID_WEAPON_CATEGORY: u32 = 0x0000_0000;
const ITEM_ID_GOODS_CATEGORY: u32 = 0x4000_0000;

static ORIG_FORGE_SELECTED_GAITEM: AtomicUsize = AtomicUsize::new(0);
static LAST_SERIAL: AtomicU64 = AtomicU64::new(0);
static LAST_SELECTED_PTR: AtomicUsize = AtomicUsize::new(0);
static LAST_ITEM_ID: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_ITEM_TYPE: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_ITEM_CATEGORY: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_ITEM_ID_CATEGORY: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_OPEN_MENU: AtomicU32 = AtomicU32::new(u32::MAX);

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
        } else if open_menu == STRENGTHEN_OPEN_MENU_ID
            && item_id_category == ITEM_ID_WEAPON_CATEGORY
        {
            StrengthenRowKind::Weapon
        } else if item_id_category == ITEM_ID_GOODS_CATEGORY {
            StrengthenRowKind::GoodsOrSpirit
        } else {
            StrengthenRowKind::Other
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

fn record_selected_row(selected: usize) {
    if selected < HEAP_LO {
        return;
    }
    let item_id = unsafe { read_u32(selected + MENU_GAITEM_ITEM_ID_OFFSET) }.unwrap_or(u32::MAX);
    let item_type =
        unsafe { read_u32(selected + MENU_GAITEM_ITEM_TYPE_OFFSET) }.unwrap_or(u32::MAX);
    let item_category = read_item_category(selected).unwrap_or(u32::MAX);
    let item_id_category = item_id & ITEM_ID_CATEGORY_MASK;
    let open_menu = current_open_menu_id().unwrap_or(u32::MAX);

    LAST_SELECTED_PTR.store(selected, Ordering::SeqCst);
    LAST_ITEM_ID.store(item_id, Ordering::SeqCst);
    LAST_ITEM_TYPE.store(item_type, Ordering::SeqCst);
    LAST_ITEM_CATEGORY.store(item_category, Ordering::SeqCst);
    LAST_ITEM_ID_CATEGORY.store(item_id_category, Ordering::SeqCst);
    LAST_OPEN_MENU.store(open_menu, Ordering::SeqCst);
    let serial = LAST_SERIAL.fetch_add(1, Ordering::SeqCst) + 1;
    let kind = StrengthenRowSnapshot::current().kind;
    harness_log!(
        "strengthen-row: serial={serial} kind={} selected=0x{selected:x} item_id=0x{item_id:x} item_type=0x{item_type:x} item_category=0x{item_category:x} item_id_category=0x{item_id_category:x} open_menu=0x{open_menu:x}",
        kind.as_str()
    );
}

unsafe extern "system" fn forge_selected_gaitem_hook(selected: usize, out: usize) -> usize {
    record_selected_row(selected);
    let orig = ORIG_FORGE_SELECTED_GAITEM.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { original(selected, out) }
}

pub fn install_strengthen_row_hook(base: usize) -> bool {
    let addr = (base + FORGE_SELECTED_GAITEM_RVA) as *mut c_void;
    match unsafe { MhHook::new(addr, forge_selected_gaitem_hook as *mut c_void) } {
        Ok(hook) => {
            ORIG_FORGE_SELECTED_GAITEM.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_ok() {
                std::mem::forget(hook);
                if matches!(unsafe { MH_ApplyQueued() }, MH_STATUS::MH_OK) {
                    harness_log!(
                        "strengthen-row: hooked selected MenuGaitem helper at 0x{:x}",
                        addr as usize
                    );
                    return true;
                }
                harness_log!("strengthen-row: MH_ApplyQueued failed");
            } else {
                harness_log!("strengthen-row: queue_enable failed");
            }
        }
        Err(status) => {
            harness_log!(
                "strengthen-row: hook create failed at 0x{:x}: {:?}",
                addr as usize,
                status
            );
        }
    }
    false
}
