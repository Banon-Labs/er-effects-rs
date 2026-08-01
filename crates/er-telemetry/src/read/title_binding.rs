//! ORACLE-1 / SEMAPHORE-A: title-rebuild `dialog+0xb78` SceneObjProxy binding.
//!
//! The warm switch-reload title DEADLOCKS because the press-start SceneObjProxy at
//! `dialog+0xb78` never binds (own_load/loaders.rs:249-261), which is why the
//! menu-free own_load reload exists. This oracle reads -- passively -- the exact
//! binding window so the armed-vs-disarmed A/B can pin which product hook/hold
//! keeps the proxy unbound: BOUND iff the proxy vtable matches CS::SceneObjProxy
//! AND its embedded CSScaleformValue handle at `+0x28` is non-null.
//!
//! It resolves the TitleStep owner via its OWN vtable-gated address-space scan
//! cached in a module static (ported from `find_title_owner_by_vtable`
//! experiments/title/profile_select_flow.rs:717 and the already-decoupled port
//! er-input-harness-dll/src/title_scan.rs) -- it does NOT read the product's
//! `TITLE_OWNER_PTR`/`PRODUCT_CORE_LAST_TITLE_DIALOG`. All field reads are
//! fault-safe `safe_read_*`; NO vtable function is ever called.

use core::ffi::c_void;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

use er_game_base::mem::{safe_read_i32, safe_read_u8, safe_read_usize, vtable_in_game_image};

// --- RVAs / offsets off the game image base (0x140000000). Sources cited inline. ---
/// TitleStep owner vtable RVA -- `[owner+0x00] == base+this`. Source:
/// constants/anti_debug.rs `TitleSessionRva::TitleOwnerVtable` (== the value the
/// decoupled er-input-harness-dll/src/title_scan.rs:31 also pins).
const TITLE_OWNER_VTABLE_RVA: usize = 0x2b63bb0;
/// Per-instance state-dispatch table RVA -- `[owner+0x10] == base+this`; the
/// discriminator that rejects stray `.data` matches on the vtable value alone.
/// Source: er-input-harness-dll/src/title_scan.rs:34.
const INNER_TITLE_STATE_TABLE_RVA: usize = 0x3d71580;
/// Live title state (`owner+0x48`, i32): 10 = MenuJobWait (PAB + menu), 11 =
/// Finish, 6 = in-world. Source: er-input-harness-dll/src/title_scan.rs:36.
const TITLE_OWNER_STATE_48_OFFSET: usize = 0x48;
/// State-table pointer slot within the owner (`owner+0x10`). Source:
/// constants/anti_debug.rs `TITLE_OWNER_INSTANCE_TABLE_OFFSET`.
const TITLE_OWNER_INSTANCE_TABLE_10_OFFSET: usize = 0x10;
/// TitleTopDialog holder (`owner+0xe0` -> CS::TitleTopDialog). Source:
/// constants/stats_panel_background.rs `TITLE_OWNER_MENU_HOLDER_E0_OFFSET`.
const TITLE_OWNER_DIALOG_E0_OFFSET: usize = 0xe0;
/// CS::TitleTopDialog vtable RVA -- `[dialog+0x00] == base+this`. Source:
/// constants/stats_panel_background.rs `TitleDialogRva::Vtable`.
const TITLE_TOP_DIALOG_VTABLE_RVA: usize = er_game_base::rva::TITLE_TOP_DIALOG_VTABLE_RVA;
/// TitleTopDialog embedded press-start SceneObjProxy window (`dialog+0xb78`).
/// Source: constants/stats_panel_background.rs
/// `TITLE_PRESS_START_SCENE_PROXY_B78_OFFSET`.
const DIALOG_PRESS_START_PROXY_B78_OFFSET: usize = 0xb78;
/// CS::SceneObjProxy vtable RVA -- `[proxy+0x00] == base+this` when bound. Source:
/// constants/stats_panel_background.rs `SCENE_OBJ_PROXY_VTABLE_RVA`.
const SCENE_OBJ_PROXY_VTABLE_RVA: usize = 0x2a94a70;
/// SceneObjProxy component slot (`proxy+0x08`) -- proxy header window field.
const SCENE_OBJ_PROXY_COMPONENT_08_OFFSET: usize = 0x08;
/// SceneObjProxy context back-ref (`proxy+0x20`). Source:
/// constants/stats_panel_background.rs `SCENE_OBJ_PROXY_CONTEXT_20_OFFSET`.
const SCENE_OBJ_PROXY_CONTEXT_20_OFFSET: usize = 0x20;
/// SceneObjProxy embedded CSScaleformValue handle (`proxy+0x28`) -- BOUND iff this
/// is non-null (the named-child binder 0x74a2f0 fills it). Source:
/// constants/stats_panel_text.rs `SCENE_OBJ_PROXY_EMBEDDED_VALUE_OFFSET`.
const SCENE_OBJ_PROXY_EMBEDDED_VALUE_28_OFFSET: usize = 0x28;
/// CS::TitleFlowContext slot (`dialog+0xa38`). Source: constants/own_load_pump.rs
/// `DIALOG_OWNER_CTX_A38_OFFSET`.
const DIALOG_TFC_A38_OFFSET: usize = 0xa38;
/// CS::TitleFlowContext vtable RVA. Source:
/// experiments/title/profile_select_flow.rs:140 `TITLE_FLOW_CONTEXT_VTABLE_RVA`.
const TITLE_FLOW_CONTEXT_VTABLE_RVA: usize = 0x2ac7f20;
/// TFC dispatch-state (`tfc+0x14c`, i32: 0=idle 1=load 3/5=busy). Source:
/// constants/own_load_pump.rs `TFC_DISPATCH_STATE_14C_OFFSET`.
const TFC_DISPATCH_STATE_14C_OFFSET: usize = 0x14c;
/// TFC notReleaseFlag55 (`tfc+0x18c`, u8). Source: constants/own_load_pump.rs
/// `TFC_NOT_RELEASE_FLAG_18C_OFFSET`.
const TFC_NOT_RELEASE_FLAG_18C_OFFSET: usize = 0x18c;
/// MenuJob slot drained by STEP_MenuJobWait / ExecuteMenuJob (`owner+0x130`).
/// Source: constants/stats_panel_background.rs `TITLE_OWNER_MENU_LIST_130_OFFSET`
/// (own_load/loaders.rs:677).
const TITLE_OWNER_MENUJOB_130_OFFSET: usize = 0x130;

// --- owner-scan tuning (ported from er-input-harness-dll/src/title_scan.rs) ---
/// One `ReadProcessMemory` per 64 KiB keeps the address-space walk fast.
const SCAN_CHUNK: usize = 0x10000;
/// Upper scan bound (top of the 47-bit user address space).
const SCAN_MAX: usize = 1usize << 47;
/// Lowest plausible heap/image pointer -- filters null + small sentinels.
const HEAP_LO: usize = 0x10000;
/// `find_title_owner` calls to skip between full scans while not yet found.
const SCAN_THROTTLE: u64 = 120;

const MARKER: &str = "er-oracle-title-binding.on";
const OUT_FILE: &str = "er-oracle-title-binding.jsonl";

const CURRENT_PROCESS: isize = -1;
const MEM_COMMIT: u32 = 0x1000;
const PAGE_NOACCESS: u32 = 0x01;
const PAGE_GUARD: u32 = 0x100;

static CACHED_OWNER: AtomicUsize = AtomicUsize::new(0);
static SCAN_COUNTDOWN: AtomicU64 = AtomicU64::new(0);

/// Windows `MEMORY_BASIC_INFORMATION` (x64). Only `base_address`, `region_size`,
/// `state`, `protect` are read; declared with the documented layout so `repr(C)`
/// pads `region_size` to +0x18 exactly like the OS struct.
#[repr(C)]
#[derive(Clone, Copy)]
struct MemoryBasicInformation {
    base_address: usize,
    allocation_base: usize,
    allocation_protect: u32,
    partition_id: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    mem_type: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualQuery(addr: *const c_void, buf: *mut MemoryBasicInformation, len: usize) -> usize;
    fn ReadProcessMemory(
        process: isize,
        base_address: *const c_void,
        buffer: *mut c_void,
        size: usize,
        bytes_read: *mut usize,
    ) -> i32;
}

/// Marker gate, checked once and cached (0 = unknown, 1 = off, 2 = on).
fn enabled() -> bool {
    static CACHED: AtomicU8 = AtomicU8::new(0);
    match CACHED.load(Ordering::SeqCst) {
        1 => false,
        2 => true,
        _ => {
            let on = super::marker_exists(MARKER);
            CACHED.store(if on { 2 } else { 1 }, Ordering::SeqCst);
            on
        }
    }
}

/// Scan one already-`ReadProcessMemory`-loaded 64 KiB chunk for the title owner:
/// a pointer slot equal to the owner vtable whose `+0x10` slot equals the state
/// table. Fault-safe: the bulk read never faults, and the `+0x10` cross-check uses
/// `safe_read_usize`.
fn scan_chunk(
    want_vtable: usize,
    want_table: usize,
    addr: usize,
    len: usize,
    buf: &mut [u8],
) -> Option<usize> {
    let mut read: usize = 0;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS,
            addr as *const c_void,
            buf.as_mut_ptr() as *mut c_void,
            len,
            &mut read,
        )
    };
    if ok == 0 {
        return None;
    }
    let usable = read.min(len) & !(core::mem::size_of::<usize>() - 1);
    let mut i = 0usize;
    while i + core::mem::size_of::<usize>() <= usable {
        let val = usize::from_ne_bytes(buf[i..i + core::mem::size_of::<usize>()].try_into().ok()?);
        if val == want_vtable {
            let candidate = addr + i;
            if unsafe { safe_read_usize(candidate + TITLE_OWNER_INSTANCE_TABLE_10_OFFSET) }
                == Some(want_table)
            {
                return Some(candidate);
            }
        }
        i += core::mem::size_of::<usize>();
    }
    None
}

/// Bounded, fault-safe full address-space walk for the title owner. Only called on
/// the throttle boundary while the owner is not yet cached.
fn scan_for_owner(base: usize) -> Option<usize> {
    let want_vtable = base.checked_add(TITLE_OWNER_VTABLE_RVA)?;
    let want_table = base.checked_add(INNER_TITLE_STATE_TABLE_RVA)?;
    let mut buf = vec![0u8; SCAN_CHUNK];
    let mut address: usize = 0;
    while address < SCAN_MAX {
        let mut info: MemoryBasicInformation = unsafe { core::mem::zeroed() };
        let queried = unsafe {
            VirtualQuery(
                address as *const c_void,
                &mut info,
                core::mem::size_of::<MemoryBasicInformation>(),
            )
        };
        if queried == 0 {
            break;
        }
        let region_base = info.base_address;
        let size = info.region_size;
        let next = region_base.saturating_add(size);
        let readable = info.state == MEM_COMMIT && info.protect & (PAGE_NOACCESS | PAGE_GUARD) == 0;
        if readable && size >= TITLE_OWNER_STATE_48_OFFSET + core::mem::size_of::<i32>() {
            let mut region_off = 0usize;
            while region_off < size {
                let chunk = (size - region_off).min(SCAN_CHUNK);
                let chunk_base = region_base + region_off;
                if let Some(hit) = scan_chunk(want_vtable, want_table, chunk_base, chunk, &mut buf)
                {
                    return Some(hit);
                }
                region_off = region_off.saturating_add(chunk);
            }
        }
        if next <= address {
            break;
        }
        address = next;
    }
    None
}

/// Resolve the title owner: return the cached pointer if `[ptr+0] == base+vtable`
/// still holds, else rescan (THROTTLED to once per `SCAN_THROTTLE` calls while not
/// found so a not-found state does not scan every write-tick).
fn find_title_owner(base: usize) -> Option<usize> {
    let cached = CACHED_OWNER.load(Ordering::SeqCst);
    if cached != 0 {
        if unsafe { safe_read_usize(cached) } == Some(base + TITLE_OWNER_VTABLE_RVA) {
            return Some(cached);
        }
        CACHED_OWNER.store(0, Ordering::SeqCst);
    }
    let countdown = SCAN_COUNTDOWN.load(Ordering::SeqCst);
    if countdown > 0 {
        SCAN_COUNTDOWN.store(countdown - 1, Ordering::SeqCst);
        return None;
    }
    SCAN_COUNTDOWN.store(SCAN_THROTTLE, Ordering::SeqCst);
    let owner = scan_for_owner(base)?;
    CACHED_OWNER.store(owner, Ordering::SeqCst);
    Some(owner)
}

/// Emit one title-binding line for this write-tick (only while the title owner is
/// resolvable, so an in-world session does not flood the file). No-op when the
/// marker is absent.
pub fn tick(base: usize, epoch: u64, play_time_ms: i64) {
    if base == 0 || !enabled() {
        return;
    }
    let Some(owner) = find_title_owner(base) else {
        return;
    };

    let title_state = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_48_OFFSET) };
    let menujob = unsafe { safe_read_usize(owner + TITLE_OWNER_MENUJOB_130_OFFSET) };

    // dialog (owner+0xe0), validated by its vtable.
    let dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_DIALOG_E0_OFFSET) }.filter(|p| *p >= HEAP_LO);
    let dialog_vtable = dialog.and_then(|d| unsafe { safe_read_usize(d) });
    let dialog_vtable_ok =
        matches!(dialog_vtable, Some(vt) if vt == base + TITLE_TOP_DIALOG_VTABLE_RVA);
    let valid_dialog = if dialog_vtable_ok { dialog } else { None };

    // press-start SceneObjProxy window (dialog+0xb78): the deadlock hinges here.
    let proxy = valid_dialog
        .and_then(|d| unsafe { safe_read_usize(d + DIALOG_PRESS_START_PROXY_B78_OFFSET) })
        .filter(|p| *p >= HEAP_LO);
    let proxy_vtable = proxy.and_then(|p| unsafe { safe_read_usize(p) });
    let proxy_vtable_ok = matches!(
        proxy_vtable,
        Some(vt) if vt == base + SCENE_OBJ_PROXY_VTABLE_RVA && vtable_in_game_image(vt, base)
    );
    let proxy_component =
        proxy.and_then(|p| unsafe { safe_read_usize(p + SCENE_OBJ_PROXY_COMPONENT_08_OFFSET) });
    let proxy_context =
        proxy.and_then(|p| unsafe { safe_read_usize(p + SCENE_OBJ_PROXY_CONTEXT_20_OFFSET) });
    let proxy_handle = proxy
        .and_then(|p| unsafe { safe_read_usize(p + SCENE_OBJ_PROXY_EMBEDDED_VALUE_28_OFFSET) });
    let proxy_handle_nonnull = matches!(proxy_handle, Some(h) if h != 0);
    let proxy_bound = proxy_vtable_ok && proxy_handle_nonnull;

    // TitleFlowContext (dialog+0xa38), validated by its vtable.
    let tfc = valid_dialog
        .and_then(|d| unsafe { safe_read_usize(d + DIALOG_TFC_A38_OFFSET) })
        .filter(|p| *p >= HEAP_LO);
    let tfc_vtable = tfc.and_then(|t| unsafe { safe_read_usize(t) });
    let tfc_vtable_ok =
        matches!(tfc_vtable, Some(vt) if vt == base + TITLE_FLOW_CONTEXT_VTABLE_RVA);
    let valid_tfc = if tfc_vtable_ok { tfc } else { None };
    let tfc_14c =
        valid_tfc.and_then(|t| unsafe { safe_read_i32(t + TFC_DISPATCH_STATE_14C_OFFSET) });
    let tfc_18c =
        valid_tfc.and_then(|t| unsafe { safe_read_u8(t + TFC_NOT_RELEASE_FLAG_18C_OFFSET) });

    let line = format!(
        "{{\"epoch\":{epoch},\
\"play_time_ms\":{play_time_ms},\
\"owner_ptr\":\"0x{owner:x}\",\
\"title_state\":{title_state},\
\"dialog_ptr\":{dialog},\
\"dialog_vtable_ok\":{dialog_vtable_ok},\
\"proxy_ptr\":{proxy},\
\"proxy_vtable\":{proxy_vtable},\
\"proxy_vtable_ok\":{proxy_vtable_ok},\
\"proxy_component_08\":{proxy_component},\
\"proxy_context_20\":{proxy_context},\
\"proxy_handle_28\":{proxy_handle},\
\"proxy_handle_nonnull\":{proxy_handle_nonnull},\
\"proxy_bound\":{proxy_bound},\
\"tfc_ptr\":{tfc},\
\"tfc_vtable_ok\":{tfc_vtable_ok},\
\"tfc_14c\":{tfc_14c},\
\"tfc_18c\":{tfc_18c},\
\"menujob_ptr\":{menujob}}}\n",
        title_state = super::json_i32_opt(title_state),
        dialog = super::json_hex_opt(dialog),
        proxy = super::json_hex_opt(proxy),
        proxy_vtable = super::json_hex_opt(proxy_vtable),
        proxy_component = super::json_hex_opt(proxy_component),
        proxy_context = super::json_hex_opt(proxy_context),
        proxy_handle = super::json_hex_opt(proxy_handle),
        tfc = super::json_hex_opt(tfc),
        tfc_14c = super::json_i32_opt(tfc_14c),
        tfc_18c = super::json_u8_opt(tfc_18c),
        menujob = super::json_hex_opt(menujob),
    );
    super::append_line(OUT_FILE, &line);
}
