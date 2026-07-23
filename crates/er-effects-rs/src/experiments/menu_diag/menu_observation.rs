use std::{
    ffi::c_void,
    fmt::Write as _,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use std::os::windows::ffi::OsStrExt as _;

use crate::input_blocker::{InputBlocker, InputFlags};
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM},
        System::{
            LibraryLoader::{GetModuleHandleA, GetProcAddress},
            Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery},
            SystemServices::DLL_PROCESS_ATTACH,
            Threading::GetCurrentProcessId,
        },
        UI::WindowsAndMessaging::{
            ClipCursor, EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
            WM_KEYDOWN, WM_KEYUP,
        },
    },
    core::{BOOL, PCSTR},
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, ffi::*, hooks::*, telemetry::*};

use super::*;

/// Decode one x86-64 jmp-thunk hop. Matches either `add rcx,8 ; jmp rel32` (the MSVC
/// `std::function` `_Do_call` thunk family the FD4 menu-item action functor routes
/// through) or a bare `jmp rel32`, returning the absolute jump target. Returns `None`
/// when `addr` is not such a thunk (i.e. it is the real lambda body). Fault-tolerant:
/// reads via `safe_read_*`, never faults on unmapped code.
pub(crate) unsafe fn decode_thunk_hop(addr: usize) -> Option<usize> {
    // Low 5 bytes `48 83 C1 08 E9` = `add rcx,8 ; jmp` (little-endian in the qword).
    const ADDRCX8_JMP_PREFIX: usize = 0xE9_08C1_8348;
    const PREFIX_MASK_40: usize = 0xFF_FFFF_FFFF;
    const ADDRCX8_REL_OFF: usize = 5;
    const ADDRCX8_NEXT_OFF: i64 = 9;
    const JMP_OPCODE: usize = 0xE9;
    const JMP_OPCODE_MASK: usize = 0xFF;
    const JMP_REL_OFF: usize = 1;
    const JMP_NEXT_OFF: i64 = 5;
    let w0 = unsafe { safe_read_usize(addr) }?;
    if (w0 & PREFIX_MASK_40) == ADDRCX8_JMP_PREFIX {
        let rel = unsafe { safe_read_i32(addr + ADDRCX8_REL_OFF) }? as i64;
        Some((addr as i64 + ADDRCX8_NEXT_OFF + rel) as usize)
    } else if (w0 & JMP_OPCODE_MASK) == JMP_OPCODE {
        let rel = unsafe { safe_read_i32(addr + JMP_REL_OFF) }? as i64;
        Some((addr as i64 + JMP_NEXT_OFF + rel) as usize)
    } else {
        None
    }
}

/// STAGE 1 (strictly NO-WRITE): walk the title menu-item container at `owner+0x138` and
/// log each item, so we can (a) confirm the live FD4 SBO pointer-vector layout matches
/// the static RE (the captured recipe pointers were suspiciously low, so VERIFY before
/// any call) and (b) identify the Load-Game leaf by its `+0xa8` action functor's
/// `_Do_call` jmp-chain resolving to `dialog_factory 0x14081ead0` (Continue's instead
/// routes to confirm `0x140b0e180`, no dialog). All reads go through fault-tolerant
/// ReadProcessMemory -- NO writes, NO native calls, NO SetState -> save-safe at the
/// parked title. Tries both container interpretations (inline SBO vs base-pointer at
/// `+0x18`) and reports which yields valid menu-item vtables. Runs once.
pub(crate) unsafe fn diagnostic_menu_walk(
    owner: usize,
    module_base: usize,
    tag: &str,
    verbose: bool,
) -> Option<usize> {
    const ITEM_CONTAINER_138: usize = 0x138;
    const CONT_CURSOR_10: usize = 0x10;
    const CONT_ELEM0_18: usize = 0x18;
    const CONT_COUNT_60: usize = 0x60;
    const MENU_JOB_HOLDER_E0: usize = TITLE_OWNER_MENU_HOLDER_E0_OFFSET;
    const ITEM_VTABLE_RVA: usize = 0x02aa97e8;
    const ITEM_FUNCTOR_A8: usize = MENU_ITEM_FUNCTOR_A8_OFFSET;
    const ITEM_CTX_10: usize = 0x10;
    const ITEM_DESC_58: usize = 0x58;
    const ITEM_RESULT_130: usize = 0x130;
    const DIALOG_FACTORY_RVA: usize = LIVE_DIALOG_FACTORY_RVA;
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const COUNT_SANITY_MIN: i32 = 1;
    const COUNT_SANITY_MAX: i32 = 32;
    const PTR_STRIDE: usize = core::mem::size_of::<usize>();
    const WALK_START: usize = 0;
    const WALK_STEP: usize = 1;
    const JMP_CHAIN_MAX_HOPS: usize = 4;
    const INTERP_INLINE: usize = 0;
    const INTERP_BASE_PTR: usize = 1;
    const INTERP_COUNT: usize = 2;

    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let item_vtable_abs = module_base + ITEM_VTABLE_RVA;
    let dialog_factory_abs = module_base + DIALOG_FACTORY_RVA;
    let container = owner + ITEM_CONTAINER_138;

    let state = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    let cursor =
        unsafe { safe_read_i32(container + CONT_CURSOR_10) }.unwrap_or(TITLE_STATE_OWNER_GONE);
    let count =
        unsafe { safe_read_i32(container + CONT_COUNT_60) }.unwrap_or(TITLE_STATE_OWNER_GONE);
    let holder = unsafe { safe_read_usize(owner + MENU_JOB_HOLDER_E0) }.unwrap_or(null);
    let elem0_raw = unsafe { safe_read_usize(container + CONT_ELEM0_18) }.unwrap_or(null);
    if verbose {
        append_autoload_debug(format_args!(
            "menu-walk[{tag}]: owner=0x{owner:x} state={state} container=0x{container:x} cursor={cursor} count={count} holder=0x{holder:x} elem0_raw=0x{elem0_raw:x} item_vt=0x{item_vtable_abs:x} dialog_factory=0x{dialog_factory_abs:x}"
        ));
    }
    if !(COUNT_SANITY_MIN..=COUNT_SANITY_MAX).contains(&count) {
        if verbose {
            append_autoload_debug(format_args!(
                "menu-walk[{tag}]: count={count} out of sane range -- container layout unverified (NO-WRITE)"
            ));
        }
        return None;
    }
    let count_usize = count as usize;

    let mut load_game_item: Option<usize> = None;
    let mut interp = INTERP_INLINE;
    while interp < INTERP_COUNT {
        let label = if interp == INTERP_INLINE {
            "inline"
        } else {
            "baseptr"
        };
        let base_ptr = if interp == INTERP_BASE_PTR {
            elem0_raw
        } else {
            null
        };
        if interp == INTERP_BASE_PTR && base_ptr == null {
            interp += WALK_STEP;
            continue;
        }
        let mut menu_items_found = WALK_START;
        let mut i = WALK_START;
        while i < count_usize {
            let item = if interp == INTERP_INLINE {
                unsafe { safe_read_usize(container + CONT_ELEM0_18 + i * PTR_STRIDE) }
            } else {
                unsafe { safe_read_usize(base_ptr + i * PTR_STRIDE) }
            }
            .unwrap_or(null);
            if item == null {
                i += WALK_STEP;
                continue;
            }
            let vtable = unsafe { safe_read_usize(item) }.unwrap_or(null);
            let is_menu_item = vtable == item_vtable_abs;
            if is_menu_item {
                menu_items_found += WALK_STEP;
            }
            let functor = unsafe { safe_read_usize(item + ITEM_FUNCTOR_A8) }.unwrap_or(null);
            let ctx = unsafe { safe_read_usize(item + ITEM_CTX_10) }.unwrap_or(null);
            let result = unsafe { safe_read_usize(item + ITEM_RESULT_130) }.unwrap_or(null);
            let desc_lo = unsafe { safe_read_usize(item + ITEM_DESC_58) }.unwrap_or(null);
            let desc_hi =
                unsafe { safe_read_usize(item + ITEM_DESC_58 + PTR_STRIDE) }.unwrap_or(null);
            // Follow the action functor's _Do_call jmp-chain; if it reaches the dialog
            // factory this is the Load-Game item.
            let mut is_load_game = false;
            let mut chain = String::new();
            if functor != null {
                let functor_vtable = unsafe { safe_read_usize(functor) }.unwrap_or(null);
                let mut docall = if functor_vtable != null {
                    unsafe { safe_read_usize(functor_vtable + DOCALL_VTABLE_SLOT_10) }
                        .unwrap_or(null)
                } else {
                    null
                };
                chain.push_str(&format!("docall=0x{docall:x}"));
                let mut hop = WALK_START;
                while hop < JMP_CHAIN_MAX_HOPS && docall != null {
                    if docall == dialog_factory_abs {
                        is_load_game = true;
                        break;
                    }
                    match unsafe { decode_thunk_hop(docall) } {
                        Some(next) => {
                            chain.push_str(&format!("->0x{next:x}"));
                            docall = next;
                        }
                        None => break,
                    }
                    hop += WALK_STEP;
                }
                if docall == dialog_factory_abs {
                    is_load_game = true;
                }
            }
            if is_menu_item && is_load_game && load_game_item.is_none() {
                load_game_item = Some(item);
            }
            if verbose {
                append_autoload_debug(format_args!(
                    "menu-walk[{tag}/{label}] i={i} item=0x{item:x} vt=0x{vtable:x} menu_item={is_menu_item} functor=0x{functor:x} ctx=0x{ctx:x} result=0x{result:x} desc=0x{desc_hi:016x}{desc_lo:016x} {chain} LOAD_GAME={is_load_game}"
                ));
            }
            i += WALK_STEP;
        }
        if verbose {
            append_autoload_debug(format_args!(
                "menu-walk[{tag}/{label}] summary: menu_items_found={menu_items_found}/{count_usize}"
            ));
        }
        interp += WALK_STEP;
    }
    load_game_item
}

/// Does `item`'s action functor at `+0xa8` resolve (through its `_Do_call` jmp-chain) to
/// the dialog factory 0x14081ead0? That uniquely marks the Load-Game leaf (Continue's
/// functor instead routes to the c30->SetState(5) confirm 0x140b0e180). Appends the decoded
/// chain to `chain` for logging. Fault-tolerant reads; never faults.
/// Does a std::function `functor` (the pointer ITSELF, not item+offset) resolve through its
/// `_Do_call` jmp-chain to the dialog factory 0x14081ead0? Used for the TitleTopDialog ROW entries
/// whose action functor lives at `[entry+0xf8]` (vs the MenuWindowJob `[item+0xa8]`). Fault-tolerant.
pub(crate) unsafe fn functor_ptr_hits_factory(
    functor: usize,
    module_base: usize,
    chain: &mut String,
) -> bool {
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const DIALOG_FACTORY_RVA: usize = LIVE_DIALOG_FACTORY_RVA;
    const JMP_CHAIN_MAX_HOPS: usize = 4;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog_factory_abs = module_base + DIALOG_FACTORY_RVA;
    if functor == null {
        return false;
    }
    let functor_vtable = unsafe { safe_read_usize(functor) }.unwrap_or(null);
    if functor_vtable == null {
        return false;
    }
    let mut docall =
        unsafe { safe_read_usize(functor_vtable + DOCALL_VTABLE_SLOT_10) }.unwrap_or(null);
    chain.push_str(&format!("functor=0x{functor:x} docall=0x{docall:x}"));
    let mut hop = HOP_START;
    while hop < JMP_CHAIN_MAX_HOPS && docall != null {
        if docall == dialog_factory_abs {
            return true;
        }
        match unsafe { decode_thunk_hop(docall) } {
            Some(next) => {
                chain.push_str(&format!("->0x{next:x}"));
                docall = next;
            }
            None => break,
        }
        hop += HOP_STEP;
    }
    docall == dialog_factory_abs
}

pub(crate) unsafe fn functor_chain_hits_factory(
    item: usize,
    module_base: usize,
    chain: &mut String,
) -> bool {
    const ITEM_FUNCTOR_A8: usize = MENU_ITEM_FUNCTOR_A8_OFFSET;
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const DIALOG_FACTORY_RVA: usize = LIVE_DIALOG_FACTORY_RVA;
    const JMP_CHAIN_MAX_HOPS: usize = 4;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog_factory_abs = module_base + DIALOG_FACTORY_RVA;
    let functor = unsafe { safe_read_usize(item + ITEM_FUNCTOR_A8) }.unwrap_or(null);
    if functor == null {
        return false;
    }
    let functor_vtable = unsafe { safe_read_usize(functor) }.unwrap_or(null);
    if functor_vtable == null {
        return false;
    }
    let mut docall =
        unsafe { safe_read_usize(functor_vtable + DOCALL_VTABLE_SLOT_10) }.unwrap_or(null);
    chain.push_str(&format!("functor=0x{functor:x} docall=0x{docall:x}"));
    let mut hop = HOP_START;
    while hop < JMP_CHAIN_MAX_HOPS && docall != null {
        if docall == dialog_factory_abs {
            return true;
        }
        match unsafe { decode_thunk_hop(docall) } {
            Some(next) => {
                chain.push_str(&format!("->0x{next:x}"));
                docall = next;
            }
            None => break,
        }
        hop += HOP_STEP;
    }
    docall == dialog_factory_abs
}

/// READ-ONLY enumerator of the TitleTopDialog's REALIZED selectable-entry vector -- the actual
/// Continue/Load-Game/New-Game rows the user navigates. These are NOT FD4 MenuWindowJobs in the
/// Sequence tree (which is why every job-tree walk + the 0x1407ad1c0 Update hook miss them); they
/// live in the dialog's own CSMenu sub-object (menu = dialog+0xa38) as a vector
/// `[menu+0x1290]..[menu+0x1298]` stride 0x210, cursor `[dialog+0xb0c]`, bound `[dialog+0xb08]`
/// (mainmenu-items-are-titletopdialog-widgets-not-fd4-jobs-2026). The confirm router 0x14078e1c0
/// fires an entry via `rax=[entry]; call [rax+0x10]` when `[entry+0xf8]!=0`. For each entry this
/// logs the vtable, its action method `[vtable+0x10]`, the `+0xf8` action-functor + its decoded
/// `_Do_call` jmp-chain, and whether either resolves to dialog_factory 0x14081ead0 (Load-Game) or
/// continue_confirm 0x140b0e180 (Continue). Pure vector math + reads (no game call) -> save-safe.
/// Returns (load_game_entry, continue_entry, cursor) for STAGE 2 to drive.
/// ZERO-INPUT title-menu Load fire (STATIC-RE validated, NO input injection). Replicates the
/// confirm router 0x14078e1c0's entry-action call directly (decoded: resolver 0x14078fbd0 returns
/// entry=[dialog+0x1290]+idx*0x210; if [entry+0xf8]!=0 -> rcx=[entry+0xf8]; call [[rcx]+0x10]).
/// Scans the realized TitleTopDialog row vector for the entry whose action functor [entry+0xf8]
/// chains to dialog_factory 0x14081ead0 (= Load Game; found empirically, NOT assumed by index),
/// sets cursor [dialog+0xb0c], and fires its _Do_call(rcx=action) -> builds the ProfileLoadDialog.
/// SELF-VALIDATING + FAIL-CLOSED: asserts the dialog vtable, that the row vector is populated, and
/// that a Load-Game entry was found, BEFORE firing -- so a non-realized/contaminated state is
/// caught, not absorbed. Build-only; the sole save-write is downstream (gated continue_confirm).
/// Returns true iff it fired.
pub(crate) unsafe fn fire_titletop_load_entry(dialog: usize, base: usize) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const VEC_BEGIN_1290: usize = 0x1290;
    const VEC_END_1298: usize = 0x1298;
    const ENTRY_STRIDE_210: usize = 0x210;
    const ENTRY_ACTION_F8: usize = 0xf8;
    const CURSOR_B0C: usize = 0xb0c;
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const MAX_ENTRIES: usize = 16;
    const IDX_START: usize = 0;
    const IDX_STEP: usize = 1;
    // VALIDATE 1: dialog identity (runtime vtable 0x142b26468).
    let vt = unsafe { safe_read_usize(dialog) }.unwrap_or(NULL);
    if vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "titletop-fire: dialog=0x{dialog:x} vt=0x{vt:x} != TitleTopDialog 0x{:x} -- ABORT (no fire)",
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return false;
    }
    // VALIDATE 2: row vector realized/populated.
    let begin = unsafe { safe_read_usize(dialog + VEC_BEGIN_1290) }.unwrap_or(NULL);
    let end = unsafe { safe_read_usize(dialog + VEC_END_1298) }.unwrap_or(NULL);
    if begin == NULL || end <= begin {
        append_autoload_debug(format_args!(
            "titletop-fire: row vector EMPTY/unrealized vec=[0x{begin:x}..0x{end:x}] -- ABORT (rows not populated)"
        ));
        return false;
    }
    let count = (end - begin) / ENTRY_STRIDE_210;
    // VALIDATE 3: find Load-Game by action->dialog_factory (NOT assumed index).
    let mut found: Option<(usize, usize)> = None;
    let mut idx = IDX_START;
    while idx < count && idx < MAX_ENTRIES {
        let entry = begin + idx * ENTRY_STRIDE_210;
        let action = unsafe { safe_read_usize(entry + ENTRY_ACTION_F8) }.unwrap_or(NULL);
        if action != NULL {
            let mut chain = String::new();
            if unsafe { functor_ptr_hits_factory(action, base, &mut chain) } {
                found = Some((idx, action));
                append_autoload_debug(format_args!(
                    "titletop-fire: LOAD-GAME entry idx={idx} entry=0x{entry:x} action=0x{action:x} {chain}"
                ));
                break;
            }
        }
        idx += IDX_STEP;
    }
    let (load_idx, action) = match found {
        Some(v) => v,
        None => {
            append_autoload_debug(format_args!(
                "titletop-fire: NO Load-Game entry (action->dialog_factory) in {count} rows -- ABORT"
            ));
            return false;
        }
    };
    // All validated -> set cursor + fire the action's _Do_call(rcx=action) == the router's confirm.
    unsafe {
        *((dialog + CURSOR_B0C) as *mut i32) = load_idx as i32;
    }
    let vtable = unsafe { safe_read_usize(action) }.unwrap_or(NULL);
    let do_call = if vtable != NULL {
        unsafe { safe_read_usize(vtable + DOCALL_VTABLE_SLOT_10) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if do_call == NULL {
        append_autoload_debug(format_args!(
            "titletop-fire: action=0x{action:x} has no _Do_call -- ABORT"
        ));
        return false;
    }
    let f: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(do_call) };
    unsafe { f(action) };
    append_autoload_debug(format_args!(
        "titletop-fire: FIRED Load-Game idx={load_idx} do_call=0x{do_call:x} -- ProfileLoadDialog should now build at owner+0xe0"
    ));
    true
}

/// Baseline snapshot of the TitleTopDialog dword window, captured before the one deterministic
/// Down so the post-Down pass can diff against it and name the cursor field precisely.
static CURSOR_PROBE_BASELINE: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

/// CURSOR-OFFSET PROBE (read-only, save-safe). `baseline=true`: snapshot the live TitleTopDialog
/// (owner+0xe0) dword window (cursor=0=Continue). `baseline=false` (after exactly one deterministic
/// Down, cursor=1=Load Game): re-read and log every offset whose value CHANGED, flagging the
/// 0->1 transition = the cursor field. Also logs the unverified static candidate [dialog+0xb0c] to
/// confirm/refute it. Pure reads via safe_read_usize -> never AVs.
pub(crate) unsafe fn cursor_offset_probe(owner: usize, base: usize, baseline: bool) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const DIALOG_E0: usize = TITLE_OWNER_MENU_HOLDER_E0_OFFSET;
    const DWORD_LO_MASK: usize = 0xffffffff;
    const DWORD_BYTES: usize = 4;
    const SCAN_START: usize = 0;
    const SCAN_STEP: usize = 1;
    const CURSOR_FROM: u32 = 0;
    const CURSOR_TO: u32 = 1;
    let tag = if baseline { "baseline" } else { "postdown" };
    let dialog = unsafe { safe_read_usize(owner + DIALOG_E0) }.unwrap_or(NULL);
    if dialog == NULL {
        return;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(NULL);
    let cand_b0c = unsafe { safe_read_usize(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }
        .map(|v| (v & DWORD_LO_MASK) as u32)
        .unwrap_or(u32::MAX);
    append_autoload_debug(format_args!(
        "cursor-probe[{tag}]: dialog=0x{dialog:x} vt=0x{dialog_vt:x}(want base+0x{:x}) candidate[+0xb0c]={cand_b0c}",
        TITLE_TOP_DIALOG_VTABLE_RVA
    ));
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return;
    }
    let read_dword = |off: usize| -> u32 {
        unsafe { safe_read_usize(dialog + off) }
            .map(|w| (w & DWORD_LO_MASK) as u32)
            .unwrap_or(u32::MAX)
    };
    if baseline {
        let mut snap = Vec::with_capacity(CURSOR_PROBE_SCAN_DWORDS);
        let mut i = SCAN_START;
        while i < CURSOR_PROBE_SCAN_DWORDS {
            snap.push(read_dword(i * DWORD_BYTES));
            i += SCAN_STEP;
        }
        if let Ok(mut b) = CURSOR_PROBE_BASELINE.lock() {
            *b = snap;
        }
        return;
    }
    let baseline_snap = match CURSOR_PROBE_BASELINE.lock() {
        Ok(b) if b.len() == CURSOR_PROBE_SCAN_DWORDS => b.clone(),
        _ => {
            append_autoload_debug(format_args!(
                "cursor-probe[postdown]: no baseline captured -- skip diff"
            ));
            return;
        }
    };
    let mut logged = SCAN_START;
    let mut i = SCAN_START;
    while i < CURSOR_PROBE_SCAN_DWORDS && logged < CURSOR_PROBE_LOG_CAP {
        let off = i * DWORD_BYTES;
        let old = baseline_snap[i];
        let new = read_dword(off);
        if old != new && new < CURSOR_PROBE_SMALL_MAX {
            let is_cursor = old == CURSOR_FROM && new == CURSOR_TO;
            append_autoload_debug(format_args!(
                "cursor-probe[postdown] CHANGED off=0x{off:x} {old}->{new}{}",
                if is_cursor { "  <== CURSOR (0->1)" } else { "" }
            ));
            logged += SCAN_STEP;
        }
        i += SCAN_STEP;
    }
    append_autoload_debug(format_args!(
        "cursor-probe[postdown]: diff complete ({logged} changed small dwords)"
    ));
}

pub(crate) unsafe fn dump_titletop_menu_entries(
    owner: usize,
    base: usize,
) -> (Option<usize>, Option<usize>, i32) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const DIALOG_E0: usize = TITLE_OWNER_MENU_HOLDER_E0_OFFSET;
    const MENU_SUBOBJ_A38: usize = DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET;
    const ENTRY_VEC_BEGIN_1290: usize = 0x1290;
    const ENTRY_VEC_END_1298: usize = 0x1298;
    const ENTRY_STRIDE_210: usize = 0x210;
    const ENTRY_ACTION_VT_SLOT_10: usize = 0x10;
    const ENTRY_FUNCTOR_F8: usize = 0xf8;
    const ENTRY_RESULT_130: usize = 0x130;
    const DIALOG_FACTORY_RVA: usize = LIVE_DIALOG_FACTORY_RVA;
    const MAX_ENTRIES: usize = 16;
    const IDX_START: usize = 0;
    const IDX_STEP: usize = 1;
    const JMP_HOPS: usize = 5;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    const BAD_I32: i32 = -1;
    let ri32 = |addr: usize| -> i32 {
        unsafe { safe_read_usize(addr) }
            .map(|v| v as u32 as i32)
            .unwrap_or(BAD_I32)
    };
    let dialog = unsafe { safe_read_usize(owner + DIALOG_E0) }.unwrap_or(NULL);
    let dialog_vt = if dialog != NULL {
        unsafe { safe_read_usize(dialog) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let cursor = if dialog != NULL {
        ri32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET)
    } else {
        BAD_I32
    };
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "titletop-entries: owner+0xe0=0x{dialog:x} vt=0x{dialog_vt:x} (expect 0x{:x}) -- not the TitleTopDialog, skip",
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return (None, None, cursor);
    }
    // The selectable-row vector does NOT live on the TitleTopDialog -- [dialog+0x1290] is GFx
    // markup text (runtime read = ASCII). The rows live on a SEPARATE title CSMenu controller
    // ("router_this", runtime vtable base+0x2afa070, ctor 0x1409060d8): the select router
    // 0x14078e1c0 calls the resolver 0x14078fbd0 with rcx=router_this, reading [router_this+0x1290]
    // /[+0x1298] (stride 0x210); cursor [+0xb0c], bound [+0xb08]. Locate router_this by scanning
    // the TitleTopDialog's fields for a pointer to an object whose [0] == that vtable. Pure reads
    // (safe_read_usize tolerates bad derefs) -> save-safe.
    const ROUTER_VTABLE_RVA: usize = 0x02afa070;
    const ROUTER_SCAN_QWORDS: usize = 0x400;
    const PTR_ALIGN_MASK: usize = 0x7;
    const QW_START: usize = 0;
    const QW_STEP: usize = 1;
    const PTR_SZ: usize = 8;
    let router_vt = base + ROUTER_VTABLE_RVA;
    // Prefer the ctor-latched router_this (cap_csmenu_ctor_hook captures it at construction --
    // it is NOT field-linked from the TitleTopDialog). Fall back to a dialog-field scan.
    let mut router_this = MENU_ROUTER_THIS.load(Ordering::SeqCst);
    if router_this == NULL {
        let mut q = QW_START;
        while q < ROUTER_SCAN_QWORDS {
            let p = unsafe { safe_read_usize(dialog + q * PTR_SZ) }.unwrap_or(NULL);
            if p != NULL
                && (p & PTR_ALIGN_MASK) == QW_START
                && unsafe { safe_read_usize(p) }.unwrap_or(NULL) == router_vt
            {
                router_this = p;
                break;
            }
            q += QW_STEP;
        }
    }
    if router_this == NULL {
        append_autoload_debug(format_args!(
            "titletop-entries: dialog=0x{dialog:x} -- router_this (CSMenu vt=0x{router_vt:x}) NOT found in dialog fields; cursor={cursor} (rows unreachable via this path)"
        ));
        return (None, None, cursor);
    }
    let menu = router_this + MENU_SUBOBJ_A38;
    let cursor = ri32(router_this + DIALOG_SLOT_CURSOR_B0C_OFFSET);
    let vec_begin = unsafe { safe_read_usize(router_this + ENTRY_VEC_BEGIN_1290) }.unwrap_or(NULL);
    let vec_end = unsafe { safe_read_usize(router_this + ENTRY_VEC_END_1298) }.unwrap_or(NULL);
    let bound = ri32(router_this + DIALOG_SLOT_BOUND_B08_OFFSET);
    if vec_begin == NULL || vec_end <= vec_begin {
        append_autoload_debug(format_args!(
            "titletop-entries: router_this=0x{router_this:x} vec=[0x{vec_begin:x}..0x{vec_end:x}] EMPTY -- rows NOT populated headless; cursor={cursor} bound={bound}"
        ));
        return (None, None, cursor);
    }
    let count = (vec_end - vec_begin) / ENTRY_STRIDE_210;
    append_autoload_debug(format_args!(
        "titletop-entries: dialog=0x{dialog:x} menu=0x{menu:x} count={count} cursor={cursor} bound={bound} vec=[0x{vec_begin:x}..0x{vec_end:x}]"
    ));
    let factory_abs = base + DIALOG_FACTORY_RVA;
    let confirm_abs = base + CONTINUE_CONFIRM_RVA;
    let continue_wrapper_abs = base + TRACE_MENU_CONTINUE_WRAPPER_RVA as usize;
    // Decode a function/thunk address forward through up to JMP_HOPS jmp-thunks, reporting if it
    // reaches the Load-Game factory, Continue confirm, or native Continue wrapper. (Full-function
    // actions that only CALL the factory internally won't chain-resolve -- the raw action address is
    // logged regardless.)
    let classify = |start: usize, chain: &mut String| -> (bool, bool) {
        let mut tgt = start;
        let mut hop = HOP_START;
        while hop < JMP_HOPS && tgt != NULL {
            if tgt == factory_abs {
                return (true, false);
            }
            if tgt == confirm_abs || tgt == continue_wrapper_abs {
                return (false, true);
            }
            match unsafe { decode_thunk_hop(tgt) } {
                Some(next) => {
                    chain.push_str(&format!("->0x{next:x}"));
                    tgt = next;
                }
                None => break,
            }
            hop += HOP_STEP;
        }
        (
            tgt == factory_abs,
            tgt == confirm_abs || tgt == continue_wrapper_abs,
        )
    };
    let mut load_game: Option<usize> = None;
    let mut continue_entry: Option<usize> = None;
    let mut idx = IDX_START;
    while idx < count && idx < MAX_ENTRIES {
        let entry = vec_begin + idx * ENTRY_STRIDE_210;
        let evt = unsafe { safe_read_usize(entry) }.unwrap_or(NULL);
        let action = if evt != NULL {
            unsafe { safe_read_usize(evt + ENTRY_ACTION_VT_SLOT_10) }.unwrap_or(NULL)
        } else {
            NULL
        };
        let functor = unsafe { safe_read_usize(entry + ENTRY_FUNCTOR_F8) }.unwrap_or(NULL);
        let result = unsafe { safe_read_usize(entry + ENTRY_RESULT_130) }.unwrap_or(NULL);
        // Classify the vtable action method, and (if present) the +0xf8 std::function's _Do_call.
        let mut action_chain = String::new();
        let (a_load, a_cont) = classify(action, &mut action_chain);
        let mut f_chain = String::new();
        let f_docall = if functor != NULL {
            let fvt = unsafe { safe_read_usize(functor) }.unwrap_or(NULL);
            if fvt != NULL {
                unsafe { safe_read_usize(fvt + ENTRY_ACTION_VT_SLOT_10) }.unwrap_or(NULL)
            } else {
                NULL
            }
        } else {
            NULL
        };
        let (f_load, f_cont) = if f_docall != NULL {
            classify(f_docall, &mut f_chain)
        } else {
            (false, false)
        };
        let is_load = a_load || f_load;
        let is_cont = a_cont || f_cont;
        append_autoload_debug(format_args!(
            "titletop-entry #{idx} entry=0x{entry:x} vt=0x{evt:x} action=0x{action:x}{action_chain} f8=0x{functor:x} f8_docall=0x{f_docall:x}{f_chain} result=0x{result:x} LOAD_GAME={is_load} CONTINUE={is_cont}"
        ));
        if is_load && load_game.is_none() {
            load_game = Some(entry);
        }
        if is_cont && continue_entry.is_none() {
            let receiver = if f_cont { functor } else { entry };
            let do_call = if f_cont { f_docall } else { action };
            continue_entry = Some(entry);
            MENU_CONTINUE_ENTRY.store(entry, Ordering::SeqCst);
            MENU_CONTINUE_FUNCTOR.store(receiver, Ordering::SeqCst);
            MENU_CONTINUE_DOCALL.store(do_call, Ordering::SeqCst);
            MENU_CONTINUE_ROUTER.store(router_this, Ordering::SeqCst);
            MENU_CONTINUE_INDEX.store(idx, Ordering::SeqCst);
        }
        idx += IDX_STEP;
    }
    (load_game, continue_entry, cursor)
}

/// SAVE-SAFE READ-ONLY structural scan of the OPEN TitleTopDialog for the Load-Game entry,
/// using the two RTTI fingerprints from the 2026-06-18 reconciliation
/// (bd title-load-is-profileloaddialog-NOT-movemapliststep-b78-dead-2026):
///   * d180 std::function `_Func_impl` vtable = `base+0x2ac3ea8` (its `_Do_call` 0x140820c60
///     `add rcx,8; jmp dialog_factory 0x14081ead0`), held at a MenuWindowJob's `+0xa8`;
///   * `CS::MenuMemberFuncJob<TitleTopDialog>` vtable = `base+0x2b265d0` (run 0x1409aaba0),
///     the entries the registrar 0x1409b24e0 registers into `[dialog+0xa48]`.
/// The prior d180-locate walked the FD4 MenuJobSequence tree (owner+0xe0/0x130/0x138) and never
/// surfaced the item, because the title rows are TitleTopDialog REGISTRY entries, not Sequence
/// children, AND `[dialog+0xa48]` is an opaque FD4 delegate registry (insert 0x1407a6c00, vcall
/// node-build -- not statically walkable). This instead does a BOUNDED flat scan of the dialog
/// object's own fields for any pointer to either fingerprint (and any object whose `+0xa8` holds
/// the d180 functor = a MenuWindowJob d180). Pure ReadProcessMemory (safe_read_usize tolerates bad
/// derefs) -> NO writes, NO native calls -> save-safe. RECON-ONLY: logs every hit and RETURNS
/// `(member_node, window_item)`: `member_node` = the first Load-Game CS::MenuMemberFuncJob node
/// (vt MEMBERFUNCJOB_VTABLE_RVA, member_fn reaches the dialog factory) -- this is the node the
/// native run 0x1409aaba0 is fired against; `window_item` = the first d180 MenuWindowJob item
/// (whose +0xa8 holds the d180 functor). It does NOT latch/advance (the caller decides) so a first
/// run stays NO-WRITE at the menu. (Extended 2026-06-18 to also return the MenuMemberFuncJob node
/// so native_load_enabled() can fire its run; previously it returned only the window item.)
pub(crate) unsafe fn scan_dialog_for_loadgame(
    owner: usize,
    base: usize,
) -> (Option<usize>, Option<usize>) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const DIALOG_E0: usize = TITLE_OWNER_MENU_HOLDER_E0_OFFSET;
    const ENTRY_REGISTRY_A48: usize = 0xa48;
    const ENTRY_SOURCE_A38: usize = DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET;
    // d180 std::function _Func_impl vtable (user-capture-confirmed); MenuMemberFuncJob vtable.
    const FUNCTOR_VTABLE_RVA: usize = 0x02ac3ea8;
    const MEMBERFUNCJOB_VTABLE_RVA: usize = 0x02b265d0;
    const FACTORY_RVA: usize = 0x0081ead0;
    const ITEM_FUNCTOR_A8: usize = MENU_ITEM_FUNCTOR_A8_OFFSET;
    const MEMBER_FN_18: usize = 0x18;
    const MEMBER_DIALOG_10: usize = core::mem::size_of::<usize>() + core::mem::size_of::<usize>();
    const MEMBER_ADJ_20: usize = 0x20;
    const SCAN_QWORDS: usize = 0x500;
    const PTR_SZ: usize = core::mem::size_of::<usize>();
    const PTR_ALIGN_MASK: usize = 0x7;
    const HEAP_LO: usize = 0x10000;
    const QW_START: usize = 0;
    const QW_STEP: usize = 1;
    const JMP_HOPS: usize = 6;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    const HIT_CAP: usize = 24;
    const HIT_START: usize = 0;
    const HIT_STEP: usize = 1;

    let dialog = unsafe { safe_read_usize(owner + DIALOG_E0) }.unwrap_or(NULL);
    if dialog == NULL {
        return (None, None);
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(NULL);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "loadgame-scan: owner+0xe0=0x{dialog:x} vt=0x{dialog_vt:x} != TitleTopDialog 0x{:x} -- skip",
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return (None, None);
    }
    let functor_vt = base + FUNCTOR_VTABLE_RVA;
    let memberjob_vt = base + MEMBERFUNCJOB_VTABLE_RVA;
    let factory_abs = base + FACTORY_RVA;
    // Resolve a (member-)fn forward through up to JMP_HOPS jmp-thunks; true if it reaches the
    // Load-Game dialog_factory. (A full member fn that only CALLs the factory internally won't
    // chain-resolve; the raw fn VA is logged regardless for offline disasm.)
    let reaches_factory = |start: usize| -> bool {
        let mut tgt = start;
        let mut hop = HOP_START;
        while hop < JMP_HOPS && tgt != NULL {
            if tgt == factory_abs {
                return true;
            }
            match unsafe { decode_thunk_hop(tgt) } {
                Some(next) => tgt = next,
                None => break,
            }
            hop += HOP_STEP;
        }
        tgt == factory_abs
    };
    let registry = unsafe { safe_read_usize(dialog + ENTRY_REGISTRY_A48) }.unwrap_or(NULL);
    let source = unsafe { safe_read_usize(dialog + ENTRY_SOURCE_A38) }.unwrap_or(NULL);
    append_autoload_debug(format_args!(
        "loadgame-scan: dialog=0x{dialog:x} registry(0xa48)=0x{registry:x} source(0xa38)=0x{source:x} functor_vt=0x{functor_vt:x} memberjob_vt=0x{memberjob_vt:x} -- scanning {SCAN_QWORDS} qwords"
    ));
    // DIRECT-BUILD r8 (ctor owner-obj) candidate validation (2026-06-18 breakthrough: the
    // ProfileLoadDialog ctor 0x1409a3d90 is COLD-VIABLE -- it builds router_this + slot rows
    // inline, no session/PGD/input-focus deps). dialog_factory 0x14081ead0 passes the ctor
    // r8 = *(capture+8); the gold capture showed that = owner+0x138, and the ctor reads the
    // profile ROW-VECTOR COUNT at [r8+0xa60]. Validate READ-ONLY which candidate has a plausible
    // vtable [+0] + a small row count [+0xa60] BEFORE any native build call (look before acting).
    const OWNER_MENU_OBJ_138: usize =
        TITLE_OWNER_MENU_LIST_130_OFFSET + core::mem::size_of::<usize>();
    const CTOR_ROW_COUNT_A60: usize = 0xa60;
    const CTOR_ROW_VEC_BEGIN_A58: usize = 0xa58;
    const R8_CAND_N: usize = 2;
    let cand_a = owner + OWNER_MENU_OBJ_138;
    let cand_b = unsafe { safe_read_usize(cand_a) }.unwrap_or(NULL);
    let cands: [(&str, usize); R8_CAND_N] = [("owner+0x138", cand_a), ("*(owner+0x138)", cand_b)];
    for (tag, c) in cands.iter() {
        if *c == NULL {
            continue;
        }
        let cvt = unsafe { safe_read_usize(*c) }.unwrap_or(NULL);
        let cnt = unsafe { safe_read_usize(*c + CTOR_ROW_COUNT_A60) }.unwrap_or(NULL);
        let vbeg = unsafe { safe_read_usize(*c + CTOR_ROW_VEC_BEGIN_A58) }.unwrap_or(NULL);
        append_autoload_debug(format_args!(
            "loadgame-scan: r8-cand[{tag}]=0x{c:x} vt=0x{cvt:x} rowvec_begin[+0xa58]=0x{vbeg:x} rowcount[+0xa60]=0x{cnt:x}"
        ));
    }
    let mut found_item: Option<usize> = None;
    let mut found_member_node: Option<usize> = None;
    let mut hits = HIT_START;
    let mut q = QW_START;
    while q < SCAN_QWORDS {
        let off = q * PTR_SZ;
        let p = unsafe { safe_read_usize(dialog + off) }.unwrap_or(NULL);
        if p != NULL && (p & PTR_ALIGN_MASK) == QW_START && p >= HEAP_LO {
            let vt = unsafe { safe_read_usize(p) }.unwrap_or(NULL);
            if vt == memberjob_vt {
                // (a) a MenuMemberFuncJob registry entry node.
                let mfn = unsafe { safe_read_usize(p + MEMBER_FN_18) }.unwrap_or(NULL);
                let mdlg = unsafe { safe_read_usize(p + MEMBER_DIALOG_10) }.unwrap_or(NULL);
                let madj = unsafe { safe_read_usize(p + MEMBER_ADJ_20) }.unwrap_or(NULL);
                let rf = reaches_factory(mfn);
                if hits < HIT_CAP {
                    append_autoload_debug(format_args!(
                        "loadgame-scan: dialog+0x{off:x} MenuMemberFuncJob node=0x{p:x} member_fn=0x{mfn:x} reaches_factory={rf} back=0x{mdlg:x} adj=0x{madj:x}"
                    ));
                }
                // The Load-Game run target: a MenuMemberFuncJob whose member_fn chains to the
                // dialog factory. Latch the FIRST such node (run 0x1409aaba0 fires against it).
                if rf && found_member_node.is_none() {
                    found_member_node = Some(p);
                }
                hits += HIT_STEP;
            } else if vt == functor_vt {
                // (b) the d180 functor object itself.
                if hits < HIT_CAP {
                    append_autoload_debug(format_args!(
                        "loadgame-scan: dialog+0x{off:x} -> d180 FUNCTOR object=0x{p:x} (vt 0x2ac3ea8)"
                    ));
                }
                hits += HIT_STEP;
            } else {
                // (c) a MenuWindowJob whose +0xa8 holds the d180 functor = the Load-Game item.
                let fa8 = unsafe { safe_read_usize(p + ITEM_FUNCTOR_A8) }.unwrap_or(NULL);
                if fa8 != NULL && (fa8 & PTR_ALIGN_MASK) == QW_START && fa8 >= HEAP_LO {
                    let fvt = unsafe { safe_read_usize(fa8) }.unwrap_or(NULL);
                    if fvt == functor_vt {
                        append_autoload_debug(format_args!(
                            "loadgame-scan: dialog+0x{off:x} -> d180 MenuWindowJob item=0x{p:x} item_vt=0x{vt:x} functor=0x{fa8:x} -- LOAD-GAME candidate"
                        ));
                        if found_item.is_none() {
                            found_item = Some(p);
                        }
                        hits += HIT_STEP;
                    }
                }
            }
        }
        q += QW_STEP;
    }
    append_autoload_debug(format_args!(
        "loadgame-scan: done hits={hits} found_member_node=0x{:x} found_item=0x{:x}",
        found_member_node.unwrap_or(NULL),
        found_item.unwrap_or(NULL)
    ));
    (found_member_node, found_item)
}
