use std::sync::atomic::Ordering;

use crate::{
    constants::{
        CONTINUE_CONFIRM_RVA, DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET, DIALOG_SLOT_BOUND_B08_OFFSET,
        DIALOG_SLOT_CURSOR_B0C_OFFSET, LIVE_DIALOG_FACTORY_RVA, MENU_ITEM_FUNCTOR_A8_OFFSET,
        MENU_ROUTER_THIS, ROUTER_THIS_VTABLE_RVA, TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
        TITLE_OWNER_MENU_LIST_130_OFFSET, TITLE_OWNER_SCAN_START_ADDRESS,
        TITLE_OWNER_STATE_COMMITTED_OFFSET, TITLE_STATE_OWNER_GONE, TITLE_TOP_DIALOG_VTABLE_RVA,
        TRACE_MENU_CONTINUE_WRAPPER_RVA,
    },
    experiments::{
        MENU_CONTINUE_DOCALL, MENU_CONTINUE_ENTRY, MENU_CONTINUE_FUNCTOR, MENU_CONTINUE_INDEX,
        MENU_CONTINUE_ROUTER,
    },
    telemetry::append_autoload_debug,
};
use er_game_base::mem::{safe_read_i32, safe_read_usize};

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
    let item_vtable_abs =
        er_game_base::mem::game_data_addr(module_base, ITEM_VTABLE_RVA, "ITEM_VTABLE_RVA");
    // RESOLVED, like the `ITEM_VTABLE_RVA` sibling one line up. `CS::LiveDialogFactory` moved on
    // 1.17 (0x81ead0 -> 0x81f950), so the raw form matched no functor and this walk classified
    // every row as "not the Load-Game leaf".
    let dialog_factory_abs = er_game_base::mem::game_data_addr(
        module_base,
        DIALOG_FACTORY_RVA,
        "LIVE_DIALOG_FACTORY_RVA",
    );
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
                // Post-loop, so `docall` may be 0 from an unreadable hop -- and a refused
                // `dialog_factory_abs` is 0 too. Screen it here; the in-loop test cannot reach 0.
                if dialog_factory_abs != null && docall == dialog_factory_abs {
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
    // RESOLVED, and a refusal must not answer `true`. `LIVE_DIALOG_FACTORY_RVA` moved on 1.17
    // (0x81ead0 -> 0x81f950), so this predicate -- the sole discriminator between the Load-Game
    // leaf and Continue -- returned `false` for every item. Refusing early matters as much:
    // `docall` is `unwrap_or(null)` = 0, which a refused RVA would match.
    let dialog_factory_abs = er_game_base::mem::game_data_addr(
        module_base,
        DIALOG_FACTORY_RVA,
        "LIVE_DIALOG_FACTORY_RVA",
    );
    if dialog_factory_abs == null {
        return false;
    }
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
    // Screened at the top, so a `docall` of 0 from an unreadable hop cannot answer `true`.
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
    if dialog_vt
        != er_game_base::mem::game_data_addr(
            base,
            TITLE_TOP_DIALOG_VTABLE_RVA,
            "TITLE_TOP_DIALOG_VTABLE_RVA",
        )
    {
        append_autoload_debug(format_args!(
            "titletop-entries: owner+0xe0=0x{dialog:x} vt=0x{dialog_vt:x} (expect 0x{:x}) -- not the TitleTopDialog, skip",
            er_game_base::mem::game_data_addr(
                base,
                TITLE_TOP_DIALOG_VTABLE_RVA,
                "TITLE_TOP_DIALOG_VTABLE_RVA"
            )
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
    const ROUTER_VTABLE_RVA: usize = ROUTER_THIS_VTABLE_RVA;
    const ROUTER_SCAN_QWORDS: usize = 0x400;
    const PTR_ALIGN_MASK: usize = 0x7;
    const QW_START: usize = 0;
    const QW_STEP: usize = 1;
    const PTR_SZ: usize = 8;
    // RESOLVED, and refused rather than used as a scan needle. The title CSMenu vtable moved on
    // 1.17 (0x2afa070 -> 0x2afd0f0), so the raw value matched nothing, `router_this` was never
    // found, and the selectable-row enumeration returned `(None, None, cursor)` every time. 0 as
    // a needle would match the first zeroed qword of the 0x400-qword sweep -- the same failure
    // `er-input-harness::title_scan` refuses to scan for.
    let router_vt =
        er_game_base::mem::game_data_addr(base, ROUTER_VTABLE_RVA, "ROUTER_THIS_VTABLE_RVA");
    if router_vt == NULL {
        append_autoload_debug(format_args!(
            "titletop-entries: dialog=0x{dialog:x} -- ROUTER_THIS_VTABLE_RVA has no mapping on this build; refusing to scan (0 as a needle matches every zeroed qword)"
        ));
        return (None, None, cursor);
    }
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
    // All three classifier targets RESOLVED. Two were raw and both moved on 1.17
    // (`LIVE_DIALOG_FACTORY_RVA` 0x81ead0 -> 0x81f950, `TRACE_MENU_CONTINUE_WRAPPER_RVA`
    // 0x82bac0 -> 0x82cab0), so `classify` could return only `(false, false)` and every row was
    // logged as neither Load-Game nor Continue.
    let factory_abs =
        er_game_base::mem::game_data_addr(base, DIALOG_FACTORY_RVA, "LIVE_DIALOG_FACTORY_RVA");
    let confirm_abs =
        er_game_base::mem::game_data_addr(base, CONTINUE_CONFIRM_RVA, "CONTINUE_CONFIRM_RVA");
    let continue_wrapper_abs = er_game_base::mem::game_data_addr(
        base,
        TRACE_MENU_CONTINUE_WRAPPER_RVA as usize,
        "TRACE_MENU_CONTINUE_WRAPPER_RVA",
    );
    // Decode a function/thunk address forward through up to JMP_HOPS jmp-thunks, reporting if it
    // reaches the Load-Game factory, Continue confirm, or native Continue wrapper. (Full-function
    // actions that only CALL the factory internally won't chain-resolve -- the raw action address is
    // logged regardless.)
    // A resolved 0 is a REFUSAL and the walk ends at 0 on any unreadable hop, so every comparison
    // requires the expected side to be real -- including the two after the loop, where `tgt` is
    // 0 most often.
    let is_factory = |tgt: usize| factory_abs != NULL && tgt == factory_abs;
    let is_continue = |tgt: usize| {
        (confirm_abs != NULL && tgt == confirm_abs)
            || (continue_wrapper_abs != NULL && tgt == continue_wrapper_abs)
    };
    let classify = |start: usize, chain: &mut String| -> (bool, bool) {
        let mut tgt = start;
        let mut hop = HOP_START;
        while hop < JMP_HOPS && tgt != NULL {
            if is_factory(tgt) {
                return (true, false);
            }
            if is_continue(tgt) {
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
        (is_factory(tgt), is_continue(tgt))
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
///
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
/// so a caller can fire its run; previously it returned only the window item.)
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
    // THE ROW VECTOR, and why the flat scan above could never see into it (2026-09-06).
    //
    // The title's rows are NOT direct pointer fields on the dialog. `CS::TitleTopDialog::
    // TitleTopDialog` (0x1409a8180, named in the 1.16.2 dump) adds each one through
    // `FUN_140744540(MenuWindow*, MenuString, action_fn, enabled_fn)`, whose last act is
    // `FUN_1407486c0(&param_1->field255_0x1f0, entry)` -- an append into a vector living in
    // `MenuWindow` itself. `FUN_1407486c0`'s own body gives the layout with no guessing: it reads
    // `+0x10` as END, `+0x08` as BEGIN, `+0x18` as CAPACITY, divides the offset by `0x140` and
    // bumps END by `0x140`, so relative to the dialog that is BEGIN=+0x1f8, END=+0x200,
    // CAP=+0x208, STRIDE=0x140. `MenuWindow`'s Ghidra layout agrees: `+0x1f0 DLAllocator*` then
    // three longlongs.
    //
    // The flat scan sees `dialog+0x1f8` as one candidate pointer, reads the FIRST ROW's first
    // qword as if it were a vtable, gets no match, and moves on -- it never strides. That is why
    // run br-20260906-195900-9b8f (PR #403) and br-20260906-195423-28aa (main) BOTH logged
    // `loadgame-scan: done hits=0` and then parked at `PREPARING SAVE 6/11 (COMPLETE 2/2)`
    // forever: the Continue MenuWindowJob latch is unsatisfiable by construction (bd
    // `continue-latch-is-unsatisfiable-docall-is-backscreen-accept-is-global-busy-2026-09-06`),
    // so this scan is the ONLY path to a fireable node, and it was looking one dereference short.
    //
    // Each row is walked with the SAME fingerprints and the SAME `reaches_factory` validation the
    // flat scan uses, so nothing new is trusted -- only a place that was never looked at.
    const ROW_VEC_BEGIN_1F8: usize = 0x1f8;
    const ROW_VEC_END_200: usize = 0x200;
    const ROW_STRIDE: usize = 0x140;
    const ROW_QWORDS: usize = ROW_STRIDE / PTR_SZ;
    const ROW_CAP: usize = 64;

    let dialog = unsafe { safe_read_usize(owner + DIALOG_E0) }.unwrap_or(NULL);
    if dialog == NULL {
        return (None, None);
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(NULL);
    if dialog_vt
        != er_game_base::mem::game_data_addr(
            base,
            TITLE_TOP_DIALOG_VTABLE_RVA,
            "TITLE_TOP_DIALOG_VTABLE_RVA",
        )
    {
        append_autoload_debug(format_args!(
            "loadgame-scan: owner+0xe0=0x{dialog:x} vt=0x{dialog_vt:x} != TitleTopDialog 0x{:x} -- skip",
            er_game_base::mem::game_data_addr(
                base,
                TITLE_TOP_DIALOG_VTABLE_RVA,
                "TITLE_TOP_DIALOG_VTABLE_RVA"
            )
        ));
        return (None, None);
    }
    let functor_vt =
        er_game_base::mem::game_data_addr(base, FUNCTOR_VTABLE_RVA, "FUNCTOR_VTABLE_RVA");
    let memberjob_vt = er_game_base::mem::game_data_addr(
        base,
        MEMBERFUNCJOB_VTABLE_RVA,
        "MEMBERFUNCJOB_VTABLE_RVA",
    );
    let factory_abs = er_game_base::mem::game_data_addr(base, FACTORY_RVA, "FACTORY_RVA");
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
    // ROW-VECTOR PASS. Only runs when the flat pass came up short, so a dialog whose rows the flat
    // scan already resolved keeps its existing behaviour byte for byte.
    let mut rows_walked = HIT_START;
    if found_member_node.is_none() || found_item.is_none() {
        let begin = unsafe { safe_read_usize(dialog + ROW_VEC_BEGIN_1F8) }.unwrap_or(NULL);
        let end = unsafe { safe_read_usize(dialog + ROW_VEC_END_200) }.unwrap_or(NULL);
        let sane = begin != NULL
            && end != NULL
            && end >= begin
            && begin >= HEAP_LO
            && (begin & PTR_ALIGN_MASK) == QW_START
            && (end - begin) % ROW_STRIDE == QW_START;
        let rows = if sane {
            (end - begin) / ROW_STRIDE
        } else {
            QW_START
        };
        append_autoload_debug(format_args!(
            "loadgame-scan: row vector begin(0x1f8)=0x{begin:x} end(0x200)=0x{end:x} stride=0x{ROW_STRIDE:x} rows={rows} sane={sane}"
        ));
        let mut row = QW_START;
        while sane && row < rows && row < ROW_CAP {
            let entry = begin + row * ROW_STRIDE;
            let mut q = QW_START;
            while q < ROW_QWORDS {
                let p = unsafe { safe_read_usize(entry + q * PTR_SZ) }.unwrap_or(NULL);
                if p != NULL && (p & PTR_ALIGN_MASK) == QW_START && p >= HEAP_LO {
                    let vt = unsafe { safe_read_usize(p) }.unwrap_or(NULL);
                    if vt == memberjob_vt {
                        let mfn = unsafe { safe_read_usize(p + MEMBER_FN_18) }.unwrap_or(NULL);
                        let rf = reaches_factory(mfn);
                        append_autoload_debug(format_args!(
                            "loadgame-scan: row {row} +0x{:x} MenuMemberFuncJob node=0x{p:x} member_fn=0x{mfn:x} reaches_factory={rf}",
                            q * PTR_SZ
                        ));
                        if rf && found_member_node.is_none() {
                            found_member_node = Some(p);
                        }
                        hits += HIT_STEP;
                    } else {
                        let fa8 = unsafe { safe_read_usize(p + ITEM_FUNCTOR_A8) }.unwrap_or(NULL);
                        if fa8 != NULL
                            && (fa8 & PTR_ALIGN_MASK) == QW_START
                            && fa8 >= HEAP_LO
                            && unsafe { safe_read_usize(fa8) }.unwrap_or(NULL) == functor_vt
                        {
                            append_autoload_debug(format_args!(
                                "loadgame-scan: row {row} +0x{:x} d180 MenuWindowJob item=0x{p:x} functor=0x{fa8:x} -- LOAD-GAME candidate",
                                q * PTR_SZ
                            ));
                            if found_item.is_none() {
                                found_item = Some(p);
                            }
                            hits += HIT_STEP;
                        }
                    }
                }
                q += QW_STEP;
            }
            rows_walked += HIT_STEP;
            row += QW_STEP;
        }
    }
    append_autoload_debug(format_args!(
        "loadgame-scan: done hits={hits} rows_walked={rows_walked} found_member_node=0x{:x} found_item=0x{:x}",
        found_member_node.unwrap_or(NULL),
        found_item.unwrap_or(NULL)
    ));
    (found_member_node, found_item)
}
