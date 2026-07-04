/// MODEL B orchestrator (gated by live_dialog_enabled(), OFF by default). At the rendered title
/// menu: (1) do the wall-clock-bounded active-screen scan to acquire the live TitleTopDialog* +
/// MenuWindow*, (2) call the dialog factory 0x14081ead0(rcx=title_dialog+0xa38, rdx=menu_window)
/// ONCE -- which builds + registers the LIVE ProfileLoadDialog into the active-screen set, then (3)
/// wait for that ProfileLoadDialog (vtable 0x142b229f8) to appear in the active-screen array, latch
/// it as OWN_STEPPER_DIALOG, and hand it to STAGE2 ACTIVATE (which fires load_activate -> native pump
/// mount -> guarded, char-fingerprint-gated continue_confirm). One-shot fire latch; bounded wait.
/// FAIL-CLOSED at every step (no acquisition -> stay; bad vtable -> no call; dialog not live yet ->
/// wait then DONE on timeout). The forge path is untouched.
pub(crate) unsafe fn own_stepper_live_dialog_fire(
    owner: usize,
    base: usize,
    waits: u64,
    timed_out: bool,
    elapsed_ms: u64,
) {
    // FIX 2 (probe-6): the factory 0x14081ead0 RETURNS the new dialog in rax. fire_live_loadgame_node
    // validates that return == ProfileLoadDialog (vt 0x142b229f8) and, on a match, stores it as
    // OWN_STEPPER_DIALOG + transitions own_stepper to STAGE2 ACTIVATE on THAT pointer. We no longer
    // scan the active-screen array 0x143d6d8d0 here (probe-2 proved it holds MODEL-RENDERERS, never
    // the PLD -> it would never confirm). Once fired+verified the orchestrator routes to STAGE2.
    if OWN_STEPPER_LIVE_FIRED.load(Ordering::SeqCst) == OWN_STEPPER_LIVE_FIRED_NO {
        let Some(ready) = (unsafe { title_live_dialog_fire_ready(owner, base) }) else {
            if timed_out {
                append_autoload_debug(format_args!(
                    "live-dialog: factory args never became semantically ready after {waits} polls/{elapsed_ms}ms -- STAY at menu (NO-WRITE), DONE"
                ));
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            return;
        };
        append_autoload_debug(format_args!(
            "live-dialog: factory args ready title_dialog=0x{:x} vt=0x{:x} capture_slot=0x{:x} capture=0x{:x} capture_vt=0x{:x} registry_vt=0x{:x} latch={} menu_window=0x{:x} menu_window_vt=0x{:x} -- firing live factory",
            ready.title_dialog,
            ready.title_dialog_vt,
            ready.capture_slot,
            ready.capture,
            ready.capture_vt,
            ready.registry_vt,
            ready.menu_opened_latch,
            ready.menu_window,
            ready.menu_window_vt
        ));
        // fire_live_loadgame_node returns true ONLY when the factory returned a verified
        // ProfileLoadDialog (it has already stored it + set STAGE2 ACTIVATE on success).
        if unsafe { fire_live_loadgame_node(ready.title_dialog, ready.menu_window, base, true) } {
            OWN_STEPPER_LIVE_FIRED.store(OWN_STEPPER_LIVE_FIRED_YES, Ordering::SeqCst);
        } else if timed_out {
            append_autoload_debug(format_args!(
                "live-dialog: factory returned non-PLD (or fail-closed) after {waits} polls/{elapsed_ms}ms -- STAY at menu (NO-WRITE), DONE"
            ));
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
        }
        return;
    }
    // Fired + verified: own_stepper is already in STAGE2 ACTIVATE driving the returned PLD. If we are
    // somehow still here (phase not advanced), bound the wait and stop without writing.
    if timed_out {
        append_autoload_debug(format_args!(
            "live-dialog: fired factory but STAGE2 did not advance after {waits} polls/{elapsed_ms}ms -- STAY (NO-WRITE), DONE"
        ));
        OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
    }
}
/// Fire a captured MenuWindowJob's `+0xa8` action std::function in-context, mirroring the
/// native leaf Update's functor-invoke at `0x1407ad2b9`:
///   rcx = `[item+0xa8]` (the std::function obj); rax = `[rcx]` (`_Func_impl_no_alloc`
///   vtable, no RTTI); rdx = `item+0x10` (the dialog ctx out-slot, the single arg);
///   call `[rax+0x10]` (`_Do_call`: `add rcx,8; jmp <lambda>`).
/// Returns the lambda result (e.g. the built dialog), which the native Update stores to
/// `[item+0x130]`. Guarded EXACTLY like the native BUILD path: only fires when
/// `[item+0xa8]!=0` AND `[item+0x10]==0`, so we never re-invoke an already-built item
/// (which would leak/overwrite `item+0x130`). This is the game's OWN menu-action functor
/// (NOT input synthesis) -- compliant with the zero-input standard. NOTE: this performs a
/// native call, so it is only used once the live item/owner are validated; it is NOT a
/// save-write by itself (the Load-entry/dialog functors build UI, not save state).
pub(crate) unsafe fn invoke_menu_item_functor(item: usize) -> Option<usize> {
    const ITEM_FUNCTOR_A8: usize = MENU_ITEM_FUNCTOR_A8_OFFSET;
    const ITEM_CTX_10: usize = 0x10;
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let functor = unsafe { safe_read_usize(item + ITEM_FUNCTOR_A8) }?;
    if functor == null {
        return None;
    }
    // BUILD-path precondition: the native Update fires the functor only when item+0x10==0.
    let ctx_slot = unsafe { safe_read_usize(item + ITEM_CTX_10) }?;
    if ctx_slot != null {
        return None;
    }
    let functor_vtable = unsafe { safe_read_usize(functor) }?;
    if functor_vtable == null {
        return None;
    }
    let do_call = unsafe { safe_read_usize(functor_vtable + DOCALL_VTABLE_SLOT_10) }?;
    if do_call == null {
        return None;
    }
    let f: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(do_call) };
    let ctx_out = item + ITEM_CTX_10;
    Some(unsafe { f(functor, ctx_out) })
}
/// Drive the NATIVE MenuWindowJob::Update 0x1407ad1c0(rcx=item, rdx=&out, r8=framectx) once to
/// BUILD the item's dialog the way the game does. Unlike a bare functor invoke, the native Update
/// WIRES the ctx (item+0x10) from the descriptor (item+0x58 -> resolved window item+0x68 via
/// 0x140d6a8e0 + window-mgr 0x143d83148) BEFORE firing the functor -- so it needs NO synthetic ctx
/// (the prior wall). It is idempotent (returns early if item+0x130 already holds a dialog) and the
/// Load-Game item only builds a ProfileLoadDialog -> BUILD-ONLY, no save write. Guarded by the
/// native BUILD precondition (mirrors 0x1407ad1ec/1fa/208): [item+0x130]==0 && [item+0xa8]!=0 &&
/// [item+0x10]==0. `framectx` is the live FD4Time passed to our idx10 step (the same ctx the native
/// pump feeds the leaf). Returns the built dialog at [item+0x130], if any.
pub(crate) unsafe fn drive_menu_item_update(
    item: usize,
    base: usize,
    framectx: usize,
) -> Option<usize> {
    const ITEM_FUNCTOR_A8: usize = MENU_ITEM_FUNCTOR_A8_OFFSET;
    const ITEM_CTX_10: usize = 0x10;
    const ITEM_RESULT_130: usize = 0x130;
    const OUT_ZERO: u64 = 0;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let functor = unsafe { safe_read_usize(item + ITEM_FUNCTOR_A8) }?;
    let ctx = unsafe { safe_read_usize(item + ITEM_CTX_10) }?;
    let pre130 = unsafe { safe_read_usize(item + ITEM_RESULT_130) }?;
    // Native BUILD precondition: dialog not yet built, functor present, ctx not yet wired.
    if functor == null || ctx != null || pre130 != null {
        return None;
    }
    let update: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(base + MENU_ITEM_UPDATE_RVA as usize) };
    // 16-byte writable StepResult out-slot ([0]=status, [4]=payload) the leaf Update writes.
    let mut out = [OUT_ZERO, OUT_ZERO];
    let _ = unsafe { update(item, out.as_mut_ptr() as usize, framectx) };
    let _ = &out;
    unsafe { safe_read_usize(item + ITEM_RESULT_130) }.filter(|&d| d != null)
}
/// Decode a single-child FD4 job decorator's forwarded-child offset from its Update fn
/// prologue. Every decorator in the owner+0x130 menu chain forwards Update to one wrapped
/// child via `mov rcx,[node+disp]; mov rax,[rcx]; call [rax+0x10]`, but the child offset
/// varies per type (0x48, 0x40, ...). Rather than tabulate each, we read the Update fn's
/// first bytes and return the disp of the FIRST `mov rcx,[rcx+disp]`:
///   `48 8b 49 <disp8>`              -> disp8
///   `48 8b 89 <disp32 le>`          -> disp32
/// Returns None if no such load appears in the scanned prologue (not a forwarding decorator).
/// Pure code read via `safe_read_usize`; never faults.
pub(crate) unsafe fn decorator_child_offset(update_fn: usize) -> Option<usize> {
    const SCAN_LEN: usize = 0x28;
    const REXW: usize = 0x48;
    const MOV_RM_OPCODE: usize = 0x8b;
    const MODRM_RCX_RCX_DISP8: usize = 0x49;
    const MODRM_RCX_RCX_DISP32: usize = 0x89;
    const BYTE_MASK: usize = 0xff;
    const B1_SHIFT: usize = 8;
    const B2_SHIFT: usize = 16;
    const B3_SHIFT: usize = 24;
    const DISP32_LEN: usize = 4;
    // bytes consumed by `48 8b 89` before the disp32 immediate begins.
    const DISP32_PREFIX_LEN: usize = 3;
    const SCAN_START: usize = 0;
    const SCAN_STEP: usize = 1;
    let mut i = SCAN_START;
    while i < SCAN_LEN {
        let word = unsafe { safe_read_usize(update_fn + i) }?;
        let b0 = word & BYTE_MASK;
        let b1 = (word >> B1_SHIFT) & BYTE_MASK;
        let b2 = (word >> B2_SHIFT) & BYTE_MASK;
        let b3 = (word >> B3_SHIFT) & BYTE_MASK;
        if b0 == REXW && b1 == MOV_RM_OPCODE {
            if b2 == MODRM_RCX_RCX_DISP8 {
                return Some(b3);
            }
            if b2 == MODRM_RCX_RCX_DISP32 {
                let mut disp = SCAN_START;
                let mut k = SCAN_START;
                while k < DISP32_LEN {
                    let byte = unsafe { safe_read_usize(update_fn + i + DISP32_PREFIX_LEN + k) }?
                        & BYTE_MASK;
                    disp |= byte << (k * B1_SHIFT);
                    k += SCAN_STEP;
                }
                return Some(disp);
            }
        }
        i += SCAN_STEP;
    }
    None
}
/// STAGE 1b (strictly NO-WRITE): recursive bounded walk of the title menu JOB tree rooted
/// at `[owner+0xe0]` (the FD4 multicast/job holder -- runtime proved the real menu lives
/// here, NOT the empty `owner+0x138`). Classifies each node by its Update slot
/// `[vtable+0x10]`: 0x1407aa1f0 = Sequence/IfElse container (children at `[node+0x18]` base,
/// count `[node+0x60]`, stride 8), 0x1407ad1c0 = MenuWindowJob leaf (action functor
/// `[node+0xa8]`). Logs the structure and returns the Load-Game leaf (functor -> dialog
/// factory). Both child-pointer interpretations (base-deref and inline) are enqueued; a
/// visited-set + node/depth caps bound it; fault-tolerant reads never AV. NO writes/calls.
pub(crate) unsafe fn diagnostic_job_tree_walk(
    owner: usize,
    module_base: usize,
    holder_offset: usize,
    tag: &str,
    verbose: bool,
) -> Option<usize> {
    const VTABLE_UPDATE_SLOT_10: usize = 0x10;
    const NODE_CHILDREN_BASE_18: usize = 0x18;
    const NODE_COUNT_60: usize = 0x60;
    const NODE_HOLDER_ROOT_18: usize = 0x18;
    const SEQ_UPDATE_RVA: usize = 0x07aa1f0;
    const LEAF_UPDATE_RVA: usize = 0x07ad1c0;
    // IfElseJob combiner (vt 0x142aa2c38). Its child jobs are NOT at the sequence
    // [+0x18]/[+0x60] layout; that mis-read is the "garbage count" the generic walk hit.
    // Decoded from selector 0x140793390: inline entry array at [node+0x18], stride 0x10,
    // each entry = {predicate@+0, child_job@+0x8}; entry count at [node+0xa0]; default/else
    // child at [node+0xa8]; runtime-active child at [node+0xb0]. Entry + default child jobs
    // are pre-built/retained at BUILD time, so reading them needs no pump.
    const IFELSE_UPDATE_RVA: usize = 0x07931e0;
    // Single-child wrapper (vt 0x142a93af8, update 0x140745510): `mov rcx,[node+0x48];
    // call [rcx]->vt[+0x10]` -- forwards Update to one wrapped child at [node+0x48]. The
    // IfElseJob entry child jobs are these wrappers, not MenuWindowJobs directly.
    const WRAP_UPDATE_RVA: usize = 0x0745510;
    const WRAP_CHILD_48: usize = 0x48;
    const IFELSE_ENTRY_STRIDE_10: usize = 0x10;
    const IFELSE_ENTRY_JOB_8: usize = 0x8;
    const IFELSE_COUNT_A0: usize = 0xa0;
    const IFELSE_DEFAULT_A8: usize = 0xa8;
    const IFELSE_ACTIVE_B0: usize = 0xb0;
    const ITEM_CTX_10: usize = 0x10;
    const ITEM_RESULT_130: usize = 0x130;
    const PTR_STRIDE: usize = core::mem::size_of::<usize>();
    const COUNT_MIN: usize = 1;
    const COUNT_MAX: usize = 32;
    const MAX_NODES: usize = 256;
    const MAX_DEPTH: usize = 8;
    const WALK_START: usize = 0;
    const WALK_STEP: usize = 1;
    // Generic decorator descent. The owner+0x130 menu tree threads d180 through a chain of
    // single-child FD4 job decorators (vt 0x142a93af8 child@+0x48, vt 0x142a93d18 child@+0x40,
    // ...) with per-type child offsets. Rather than decode each, for any node that is none of
    // the known container/leaf kinds we scan a bounded field window and enqueue every qword
    // that points at an in-module job object (its vtable AND that vtable's Update slot both
    // land inside the game image). Fault-tolerant reads; visited-set + node budget bound it.
    const GEN_SCAN_LO: usize = 0x10;
    const GEN_SCAN_HI: usize = 0xc0;
    // PE image bounds (for the in-module pointer test): SizeOfImage at NT+0x50, e_lfanew at
    // base+0x3c. Both are u32; mask the low dword off the qword read.
    const PE_E_LFANEW_OFFSET: usize = 0x3c;
    const PE_SIZE_OF_IMAGE_FROM_NT: usize = 0x50;
    const PE_U32_MASK: usize = 0xffffffff;
    const MODULE_SPAN_FALLBACK: usize = 0x3000000;
    const MODULE_MIN_OFFSET: usize = 0x1000;

    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let seq_update_abs = module_base + SEQ_UPDATE_RVA;
    let leaf_update_abs = module_base + LEAF_UPDATE_RVA;
    let ifelse_update_abs = module_base + IFELSE_UPDATE_RVA;
    let wrap_update_abs = module_base + WRAP_UPDATE_RVA;

    let e_lfanew = unsafe { safe_read_usize(module_base + PE_E_LFANEW_OFFSET) }
        .map(|v| v & PE_U32_MASK)
        .unwrap_or(null);
    let image_span = if e_lfanew != null {
        unsafe { safe_read_usize(module_base + e_lfanew + PE_SIZE_OF_IMAGE_FROM_NT) }
            .map(|v| v & PE_U32_MASK)
            .filter(|&s| s != null)
            .unwrap_or(MODULE_SPAN_FALLBACK)
    } else {
        MODULE_SPAN_FALLBACK
    };
    let module_lo = module_base + MODULE_MIN_OFFSET;
    let module_hi = module_base + image_span;
    let in_module = |p: usize| p >= module_lo && p < module_hi;

    let holder = unsafe { safe_read_usize(owner + holder_offset) }.unwrap_or(null);
    if verbose {
        append_autoload_debug(format_args!(
            "job-tree[{tag}]: owner=0x{owner:x} holder(owner+0x{holder_offset:x})=0x{holder:x} seq_update=0x{seq_update_abs:x} leaf_update=0x{leaf_update_abs:x}"
        ));
    }
    if holder == null {
        return None;
    }
    let root = unsafe { safe_read_usize(holder + NODE_HOLDER_ROOT_18) }.unwrap_or(null);

    let mut load_game: Option<usize> = None;
    let mut visited: Vec<usize> = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();
    stack.push((holder, WALK_START));
    if root != null {
        stack.push((root, WALK_START));
    }
    let mut node_budget = MAX_NODES;
    while let Some((node, depth)) = stack.pop() {
        if node_budget == WALK_START {
            break;
        }
        node_budget -= WALK_STEP;
        if node == null || visited.contains(&node) {
            continue;
        }
        visited.push(node);
        let vtable = unsafe { safe_read_usize(node) }.unwrap_or(null);
        let update = if vtable != null {
            unsafe { safe_read_usize(vtable + VTABLE_UPDATE_SLOT_10) }.unwrap_or(null)
        } else {
            null
        };
        let count = unsafe { safe_read_usize(node + NODE_COUNT_60) }.unwrap_or(null);
        let base = unsafe { safe_read_usize(node + NODE_CHILDREN_BASE_18) }.unwrap_or(null);
        let is_leaf = update == leaf_update_abs;
        let is_container = update == seq_update_abs;
        let is_ifelse = update == ifelse_update_abs;
        let is_wrap = update == wrap_update_abs;
        let wrap_child = unsafe { safe_read_usize(node + WRAP_CHILD_48) }.unwrap_or(null);
        let ife_count = unsafe { safe_read_usize(node + IFELSE_COUNT_A0) }.unwrap_or(null);
        let ife_default = unsafe { safe_read_usize(node + IFELSE_DEFAULT_A8) }.unwrap_or(null);
        let ife_active = unsafe { safe_read_usize(node + IFELSE_ACTIVE_B0) }.unwrap_or(null);
        let mut chain = String::new();
        let is_load_game = if update != null {
            unsafe { functor_chain_hits_factory(node, module_base, &mut chain) }
        } else {
            false
        };
        if is_load_game && load_game.is_none() {
            load_game = Some(node);
        }
        let ctx = unsafe { safe_read_usize(node + ITEM_CTX_10) }.unwrap_or(null);
        let result = unsafe { safe_read_usize(node + ITEM_RESULT_130) }.unwrap_or(null);
        if verbose {
            append_autoload_debug(format_args!(
                "job-tree[{tag}] d={depth} node=0x{node:x} vt=0x{vtable:x} update=0x{update:x} leaf={is_leaf} container={is_container} ifelse={is_ifelse} wrap={is_wrap} count=0x{count:x} base=0x{base:x} ife_count=0x{ife_count:x} ife_default=0x{ife_default:x} ife_active=0x{ife_active:x} wrap_child=0x{wrap_child:x} ctx=0x{ctx:x} result=0x{result:x} {chain} LOAD_GAME={is_load_game}"
            ));
        }
        if depth < MAX_DEPTH && is_wrap {
            // Single-child wrapper: descend into its one forwarded child.
            if wrap_child != null {
                stack.push((wrap_child, depth + WALK_STEP));
            }
        } else if depth < MAX_DEPTH && is_ifelse {
            // IfElseJob (selector 0x140793390): a case vector at [node+0x18], stride 0x10, each
            // case = {predicate@+0, child_job@+0x8}; the main-menu branch (holding d180) binds its
            // child to [node+0xb0] ONLY when its input-gated predicate flips (so headless d180 is
            // present-but-unbound). The case COUNT offset is ambiguous across memos (+0xa0 vs +0x88
            // = capacity vs size), so rather than trust a count we do a bounded LAYOUT-AGNOSTIC
            // scan of the case slots and enqueue every child_job (and predicate slot) that points
            // at an in-module job object -- this reaches d180's case child whether or not its
            // branch is bound, with no pump. Pure reads; visited-set + node budget bound it.
            let _ = (ife_count, IFELSE_COUNT_A0, COUNT_MIN, IFELSE_ENTRY_JOB_8);
            let mut i = WALK_START;
            while i < COUNT_MAX {
                let case = node + NODE_CHILDREN_BASE_18 + i * IFELSE_ENTRY_STRIDE_10;
                for slot in [WALK_START, IFELSE_ENTRY_JOB_8] {
                    let child = unsafe { safe_read_usize(case + slot) }.unwrap_or(null);
                    if child != null && child != node {
                        let cvt = unsafe { safe_read_usize(child) }.unwrap_or(null);
                        if in_module(cvt) {
                            stack.push((child, depth + WALK_STEP));
                        }
                    }
                }
                i += WALK_STEP;
            }
            if ife_default != null {
                stack.push((ife_default, depth + WALK_STEP));
            }
            if ife_active != null && ife_active != ife_default {
                stack.push((ife_active, depth + WALK_STEP));
            }
        } else if depth < MAX_DEPTH && is_container && (COUNT_MIN..=COUNT_MAX).contains(&count) {
            let mut i = WALK_START;
            while i < count {
                let child_b = if base != null {
                    unsafe { safe_read_usize(base + i * PTR_STRIDE) }.unwrap_or(null)
                } else {
                    null
                };
                let child_i =
                    unsafe { safe_read_usize(node + NODE_CHILDREN_BASE_18 + i * PTR_STRIDE) }
                        .unwrap_or(null);
                if child_b != null {
                    stack.push((child_b, depth + WALK_STEP));
                }
                if child_i != null && child_i != child_b {
                    stack.push((child_i, depth + WALK_STEP));
                }
                i += WALK_STEP;
            }
        } else if depth < MAX_DEPTH && !is_leaf && in_module(vtable) && in_module(update) {
            // Unknown FD4 decorator: decode the single forwarded-child offset from its Update
            // prologue (`mov rcx,[node+disp]`) and descend into [node+disp] ONLY -- a precise
            // single-child follow, never a field scan (which wandered into the GUI graph).
            if let Some(off) = unsafe { decorator_child_offset(update) } {
                if (GEN_SCAN_LO..=GEN_SCAN_HI).contains(&off) {
                    let child = unsafe { safe_read_usize(node + off) }.unwrap_or(null);
                    if child != null && child != node {
                        let cvt = unsafe { safe_read_usize(child) }.unwrap_or(null);
                        if in_module(cvt) {
                            stack.push((child, depth + WALK_STEP));
                        }
                    }
                }
            }
        }
    }
    if verbose {
        append_autoload_debug(format_args!(
            "job-tree[{tag}] summary: nodes_visited={} load_game=0x{:x}",
            visited.len(),
            load_game.unwrap_or(null)
        ));
    }
    load_game
}
/// STAGE 2 in-context load drive (see the lib.rs STAGE-2 const block). Runs each frame while
/// `OWN_STEPPER_PHASE` is one of the four S2 phases, sequencing:
///   INVOKE  -> hand-fire d180's `+0xa8` functor to build the ProfileLoadDialog
///   ACTIVATE-> write slot cursor `[dialog+0xb0c]=N`, call vtable-slot-20 `load_activate(dialog)`
///   MOUNT_POLL -> let the native pump tick the selector; detect the mount (`ac0==N` + io
///               request set->cleared); latch the real `c30`
///   CONFIRM -> guard (`ac0==N && c30==latched`) then `continue_confirm` -> SetState(5)
/// Every cross-into-game call is gated by read-only preconditions; the ONLY save-write risk is
/// the CONFIRM SetState(5), gated entirely by a verified real mount (fail-closed otherwise:
/// stay at the menu, NO SetState(5), NO save write).
pub(crate) unsafe fn own_stepper_stage2(
    owner: usize,
    base: usize,
    gm: usize,
    want_slot: i32,
    n: u64,
    framectx: usize,
) {
    const S2_LOG_INTERVAL: u64 = 30;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let phase = OWN_STEPPER_PHASE.load(Ordering::SeqCst);
    let waits = OWN_STEPPER_S2_WAITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
    let s2_elapsed_ms = own_stepper_s2_elapsed_ms();
    let s2_timed_out = own_stepper_s2_timed_out();
    let item = MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst);
    let pld_vt = base + PROFILE_LOAD_DIALOG_VTABLE_RVA;
    // 32-bit GameMan field read (low dword of the 8-byte safe read; little-endian).
    let ri32 = |addr: usize, dflt: i32| -> i32 {
        unsafe { safe_read_usize(addr) }
            .map(|v| v as u32 as i32)
            .unwrap_or(dflt)
    };
    let c30 = if gm != null {
        ri32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET, GAME_MAN_C30_UNSET)
    } else {
        GAME_MAN_C30_UNSET
    };
    let ac0 = if gm != null {
        ri32(
            gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET,
            OWN_STEPPER_SLOT_NONE,
        )
    } else {
        OWN_STEPPER_SLOT_NONE
    };
    let b80 = if gm != null {
        ri32(
            gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET,
            OWN_STEPPER_B80_IDLE,
        )
    } else {
        OWN_STEPPER_B80_IDLE
    };
    let iodev = unsafe { safe_read_usize(base + IODEV_GLOBAL_RVA) }.unwrap_or(null);
    let (io10, io18, io20) = if iodev != null {
        (
            unsafe { safe_read_usize(iodev + IODEV_INFLIGHT_10_OFFSET) }.unwrap_or(null),
            unsafe { safe_read_usize(iodev + IODEV_REQHANDLE_18_OFFSET) }.unwrap_or(null),
            unsafe { safe_read_usize(iodev + IODEV_REQHANDLE_20_OFFSET) }.unwrap_or(null),
        )
    } else {
        (null, null, null)
    };
    // A dialog candidate is valid iff its vtable == ProfileLoadDialog.
    let valid_dialog =
        |d: usize| -> bool { d != null && unsafe { safe_read_usize(d) }.unwrap_or(null) == pld_vt };

    if phase == OWN_STEPPER_PHASE_S2_INVOKE {
        if item == null {
            if s2_timed_out {
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE2-INVOKE-TIMEOUT no item after {waits} polls/{s2_elapsed_ms}ms -- STAGE2-NOWRITE-ABORT"
                ));
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            return;
        }
        let dlg130 =
            unsafe { safe_read_usize(item + MENU_ITEM_DIALOG_RESULT_130_OFFSET) }.unwrap_or(null);
        let ctx10 = unsafe { safe_read_usize(item + MENU_ITEM_CTX_10_OFFSET) }.unwrap_or(null);
        let functor =
            unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }.unwrap_or(null);
        // If the native pump already built the dialog (focused on Load), use it.
        let existing = if valid_dialog(dlg130) {
            dlg130
        } else if valid_dialog(ctx10) {
            ctx10
        } else {
            null
        };
        if existing != null {
            OWN_STEPPER_DIALOG.store(existing, Ordering::SeqCst);
            timeline_event(
                "T_dialog",
                n,
                format_args!("dialog=0x{existing:x} via=native"),
            );
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-INVOKE-OK (native-built) dialog=0x{existing:x} dvt=0x{pld_vt:x} item=0x{item:x}"
            ));
            own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_ACTIVATE);
            return;
        }
        // Drive d180's NATIVE Update once as soon as the item exists and its native build
        // preconditions are true. d180 lives at owner+0x130 under an input-gated IfElseJob branch
        // (its case child is never bound headless), so the native pump never ticks it -- but the
        // item is fully built, so calling its own MenuWindowJob::Update 0x1407ad1c0 (which wires
        // the ctx item+0x10 from the descriptor item+0x58 before firing the functor) builds the
        // ProfileLoadDialog with a NATIVE ctx (no synthesis) and zero input. Build-only;
        // idempotent; no save write.
        if OWN_STEPPER_INVOKED.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS as usize {
            let ret = unsafe { drive_menu_item_update(item, base, framectx) }.unwrap_or(null);
            OWN_STEPPER_INVOKED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            let dlg130b = unsafe { safe_read_usize(item + MENU_ITEM_DIALOG_RESULT_130_OFFSET) }
                .unwrap_or(null);
            let ctx10b = unsafe { safe_read_usize(item + MENU_ITEM_CTX_10_OFFSET) }.unwrap_or(null);
            let candidate = if valid_dialog(ret) {
                ret
            } else if valid_dialog(dlg130b) {
                dlg130b
            } else if valid_dialog(ctx10b) {
                ctx10b
            } else {
                null
            };
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-INVOKE hand-fired item=0x{item:x} functor=0x{functor:x} ret=0x{ret:x} dlg130(pre=0x{dlg130:x},post=0x{dlg130b:x}) ctx10(pre=0x{ctx10:x},post=0x{ctx10b:x}) candidate=0x{candidate:x}"
            ));
            if candidate != null {
                // Mirror native bookkeeping: stash the built dialog at item+0x130 if empty so a
                // later native leaf-Update does not re-build it.
                if dlg130b == null {
                    unsafe {
                        *((item + MENU_ITEM_DIALOG_RESULT_130_OFFSET) as *mut usize) = candidate;
                    }
                }
                OWN_STEPPER_DIALOG.store(candidate, Ordering::SeqCst);
                timeline_event(
                    "T_dialog",
                    n,
                    format_args!("dialog=0x{candidate:x} via=invoke"),
                );
                own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_ACTIVATE);
                return;
            }
        }
        if s2_timed_out {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-INVOKE-TIMEOUT dialog not built after {waits} polls/{s2_elapsed_ms}ms -- STAGE2-NOWRITE-ABORT"
            ));
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
        }
        return;
    }

    if phase == OWN_STEPPER_PHASE_S2_ACTIVATE {
        let dialog = OWN_STEPPER_DIALOG.load(Ordering::SeqCst);
        if gm == null {
            if waits % S2_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE2-ACTIVATE waiting for GameMan before load_activate dialog=0x{dialog:x}"
                ));
            }
            if s2_timed_out {
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            return;
        }
        let log_pending = waits % S2_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64;
        let Some(ready) =
            (unsafe { profile_load_dialog_ready(base, dialog, want_slot, log_pending) })
        else {
            if s2_timed_out {
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE2-ACTIVATE-TIMEOUT profile_load_dialog_ready stayed false after {waits} polls/{s2_elapsed_ms}ms -- STAGE2-NOWRITE-ABORT"
                ));
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            return;
        };
        let dialog = ready.dialog;
        let dvt = ready.dvt;
        let bound = ready.bound;
        let cursor_now = ready.cursor_now;
        let expected_slot = ready.expected_slot;
        let cursor_target = ready.cursor_target;
        let lav = ready.load_activate;
        // For a fixed slot, write the dialog row cursor (UI state, not a save write); for
        // most-recent, leave the dialog's own highlight untouched.
        if want_slot != OWN_STEPPER_SLOT_NONE {
            unsafe {
                *((dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) as *mut i32) = cursor_target;
            }
        }
        OWN_STEPPER_EXPECTED_SLOT.store(expected_slot, Ordering::SeqCst);
        if (live_dialog_enabled() || product_autoload_enabled())
            && expected_slot != OWN_STEPPER_SLOT_NONE
        {
            let set_save_slot: unsafe extern "system" fn(i32) =
                unsafe { std::mem::transmute(base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
            unsafe { set_save_slot(expected_slot) };
            let slot_after = unsafe { *((gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-ACTIVATE native-selector set_save_slot({expected_slot}) after profile_load_dialog_ready -> ac0={slot_after}"
            ));
        }
        OWN_STEPPER_SELECTOR_STEP.store(null, Ordering::SeqCst);
        OWN_STEPPER_SELECTOR_CTX.store(null, Ordering::SeqCst);
        let activate: unsafe extern "system" fn(usize) -> u8 = unsafe { std::mem::transmute(lav) };
        let r = unsafe { activate(dialog) };
        append_autoload_debug(format_args!(
            "own_stepper: STAGE2-ACTIVATE profile_load_dialog_ready opened want={want_slot} expected={expected_slot} cursor_target={cursor_target} cursor_now={cursor_now} bound={bound} dvt=0x{dvt:x} lav=0x{lav:x} ret={r} dialog=0x{dialog:x} ctx=0x{:x} ctx_vt=0x{:x} pgd=0x{:x} io18=0x{io18:x} io20=0x{io20:x} -- MOUNT via live selector tick plus direct submit+drain+deser",
            ready.load_job_ctx, ready.load_job_ctx_vt, ready.player_game_data
        ));
        // Reset the shared mount latches so the MOUNT phase's delegate (cold_char_mount_drive) and
        // the mount-done gate observe a clean slate for this drive.
        OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_NOT_FIRED, Ordering::SeqCst);
        OWN_STEPPER_MOUNT_C30.store(GAME_MAN_C30_UNSET, Ordering::SeqCst);
        OWN_STEPPER_IO_WAS_SET.store(OWN_STEPPER_IO_WAS_SET_NO, Ordering::SeqCst);
        own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_MOUNT_POLL);
        return;
    }

    if phase == OWN_STEPPER_PHASE_S2_MOUNT_POLL {
        let expected = OWN_STEPPER_EXPECTED_SLOT.load(Ordering::SeqCst);
        // Product/live-dialog path: once load_activate builds the real selector step, self-pump
        // that native selector instead of jumping straight to the cold full-read helper. This is the
        // proper Load-Game beginning: profile rows/record state -> load_activate -> selector tick ->
        // menu_deser/mount. The cold helper remains for the older non-selector diagnostic paths.
        let native_selector_path = live_dialog_enabled() || product_autoload_enabled();
        if native_selector_path {
            const SELECTOR_TICK_RVA: usize = PROFILE_LOAD_SELECTOR_TICK_RVA;
            #[repr(C)]
            struct SelectorTickResultLayout {
                qwords: [usize; 4],
            }
            const SELECTOR_RESULT_QWORDS: usize =
                core::mem::size_of::<SelectorTickResultLayout>() / core::mem::size_of::<usize>();
            let step = OWN_STEPPER_SELECTOR_STEP.load(Ordering::SeqCst);
            let selector_ctx = OWN_STEPPER_SELECTOR_CTX.load(Ordering::SeqCst);
            if step != null && selector_ctx != null {
                let tick: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
                    unsafe { std::mem::transmute(base + SELECTOR_TICK_RVA) };
                let mut result = [TITLE_OWNER_SCAN_START_ADDRESS; SELECTOR_RESULT_QWORDS];
                let result_ptr = result.as_mut_ptr() as usize;
                let tick_ret = unsafe { tick(step, selector_ctx, result_ptr, null) };
                if waits % S2_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                    append_autoload_debug(format_args!(
                        "own_stepper: native selector self-pump step=0x{step:x} ctx=0x{selector_ctx:x} result=0x{result_ptr:x} ret=0x{tick_ret:x}"
                    ));
                }
            } else if waits % S2_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                append_autoload_debug(format_args!(
                    "own_stepper: native selector self-pump waiting for selector step/ctx step=0x{step:x} ctx=0x{selector_ctx:x}"
                ));
            }
        } else {
            unsafe { cold_char_mount_drive(base, gm, want_slot, n) };
        }
        // io18/io20 both non-null => the request was started; latch it.
        if io18 != null && io20 != null {
            OWN_STEPPER_IO_WAS_SET.store(OWN_STEPPER_IO_WAS_SET_YES, Ordering::SeqCst);
        }
        let io_was_set =
            OWN_STEPPER_IO_WAS_SET.load(Ordering::SeqCst) == OWN_STEPPER_IO_WAS_SET_YES;
        let io_consumed = io18 == null && io20 == null;
        // Mount signal = the deserialize 0x67b290 SUCCEEDED (ret==1), which proves it wrote c30 from
        // the save header + applied the real char. c30 itself is ambiguous (the char's real early map
        // 0xa010000 collides with the new-game default), so the reliable signal is deser-success +
        // a SANE latched c30 (not the unset sentinel, not zero). (setstate5-is-save-safe-c30-from-save)
        const C30_ZERO: i32 = 0;
        let _ = (io_was_set, io_consumed);
        let mut latched_c30 = OWN_STEPPER_MOUNT_C30.load(Ordering::SeqCst);
        let mut deser_state = OWN_STEPPER_DESER_FIRED.load(Ordering::SeqCst);
        if native_selector_path
            && deser_state == OWN_STEPPER_DESER_NOT_FIRED
            && ac0 == expected
            && expected != OWN_STEPPER_SLOT_NONE
            && c30 != GAME_MAN_C30_UNSET
            && c30 != C30_ZERO
        {
            let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
            if fp_real {
                OWN_STEPPER_MOUNT_C30.store(c30, Ordering::SeqCst);
                OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_OK, Ordering::SeqCst);
                latched_c30 = c30;
                deser_state = OWN_STEPPER_DESER_FIRED_OK;
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE2-MOUNT-LATCH native-selector ac0={ac0} expected={expected} c30=0x{c30:x} fp_real={fp_real}(level={fp_level} name_len={fp_name_len})"
                ));
            }
        }
        let deser_ok = deser_state == OWN_STEPPER_DESER_FIRED_OK;
        let deser_done = deser_state != OWN_STEPPER_DESER_NOT_FIRED;
        let (fp_real_mount, _fp_level_mount, _fp_name_len_mount) =
            unsafe { char_fingerprint(base) };
        let c30_available = latched_c30 != GAME_MAN_C30_UNSET && latched_c30 != C30_ZERO;
        let c30_sane =
            c30_available && (latched_c30 != GAME_MAN_NEWGAME_DEFAULT_MAP || fp_real_mount);
        let mount_done =
            deser_ok && c30_sane && ac0 == expected && expected != OWN_STEPPER_SLOT_NONE;
        if waits % S2_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 || deser_done {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-MOUNT-POLL waits={waits} ac0={ac0} expected={expected} c30=0x{c30:x} latched=0x{latched_c30:x} deser_ok={deser_ok} c30_sane={c30_sane} b80={b80} io18=0x{io18:x} io20=0x{io20:x}"
            ));
        }
        // Default VERIFY-ONLY: stop at deserialize. With the explicit fullread commit gate enabled,
        // a verified mount advances to CONFIRM, whose independent guard re-checks deser_ok,
        // fp_real, expected slot, and c30 latch before continue_confirm/SetState5.
        if deser_done {
            let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
            timeline_event(
                "T_mount",
                n,
                format_args!("ac0={ac0} c30=0x{latched_c30:x} waits={waits}"),
            );
            let commit = native_fullread_commit_enabled();
            if mount_done && commit {
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE2-MOUNT-COMMIT deser_ok={deser_ok} mount_done={mount_done} ac0={ac0} expected={expected} c30=0x{c30:x} latched=0x{latched_c30:x} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) b80={b80} -- entering CONFIRM"
                ));
                own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_CONFIRM);
            } else {
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE2-MOUNT-VERIFY deser_ok={deser_ok} mount_done={mount_done} commit={commit} ac0={ac0} expected={expected} c30=0x{c30:x} latched=0x{latched_c30:x} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) b80={b80} -- VERIFY-ONLY (NO SetState5/NO save write) -> DONE"
                ));
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
        } else if s2_timed_out {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-MOUNT-POLL-TIMEOUT ac0={ac0} want={want_slot} c30=0x{c30:x} io_was_set={io_was_set} after {waits} polls/{s2_elapsed_ms}ms -- STAGE2-NOWRITE-ABORT (stay at menu)"
            ));
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
        }
        return;
    }

    if phase == OWN_STEPPER_PHASE_S2_CONFIRM {
        let latched = OWN_STEPPER_MOUNT_C30.load(Ordering::SeqCst);
        let expected = OWN_STEPPER_EXPECTED_SLOT.load(Ordering::SeqCst);
        // HARD save-write guard: only SetState(5) when the real char is still mounted. Require the
        // mount latch, c30 unchanged since the mount and present, the slot match, and the decisive
        // PlayerGameData character fingerprint. c30 may legitimately equal the m10_01 default for
        // saves parked there, and the UTF-16 name field can be empty/unknown, so neither is a hard
        // failure when the level/stat fingerprint is real.
        const DESER_FIRED_OK_CONFIRM: usize = 2;
        const C30_ZERO_CONFIRM: i32 = 0;
        let deser_ok = OWN_STEPPER_DESER_FIRED.load(Ordering::SeqCst) == DESER_FIRED_OK_CONFIRM;
        // CHAR-FINGERPRINT gate (MODEL B): SetState(5) ONLY when a REAL character is mounted in
        // PlayerGameData (level>=1). Runtime direct-build evidence showed the mounted target slot
        // has real stats/level while the name field remains empty/unknown, so name is diagnostic
        // only. The new-game default remains level 0, so level>=1 still fail-closes safely.
        let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
        let c30_available = c30 == latched && c30 != GAME_MAN_C30_UNSET && c30 != C30_ZERO_CONFIRM;
        let proceed = deser_ok
            && fp_real
            && ac0 == expected
            && expected != OWN_STEPPER_SLOT_NONE
            && c30_available;
        if !proceed {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-CONFIRM-GUARD-FAIL ac0={ac0} expected={expected} c30=0x{c30:x} latched=0x{latched:x} deser_ok={deser_ok} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) -- STAGE2-NOWRITE-ABORT"
            ));
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        if OWN_STEPPER_CONFIRMED.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS as usize {
            let shim = &raw mut OWN_STEPPER_SHIM;
            unsafe { (*shim)[OWN_STEPPER_SHIM_OWNER_IDX] = owner };
            let shim_ptr = shim as usize;
            let confirm: unsafe extern "system" fn(usize) =
                unsafe { std::mem::transmute(base + CONTINUE_CONFIRM_RVA) };
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-CONFIRM-GUARD-PASS ac0={ac0} c30=0x{c30:x} -> continue_confirm shim=0x{shim_ptr:x} owner=0x{owner:x}"
            ));
            timeline_event("T_playgame", n, format_args!("ac0={ac0} c30=0x{c30:x}"));
            unsafe { confirm(shim_ptr) };
            OWN_STEPPER_CONFIRMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-SETSTATE5 fired owner=0x{owner:x} -- native pump now streams the real world"
            ));
        }
        OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
    }
}
/// Patch the writable .data idx10 step-fn slot to our handler once the FE-host is at
/// committed state 10. Same thread as the dispatch (game-task), so no race.
pub(crate) unsafe fn own_stepper_patch_once(module_base: usize) {
    if OWN_STEPPER_PATCHED.load(Ordering::SeqCst) != OWN_STEPPER_PATCHED_NO {
        return;
    }
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        return;
    };
    let owner = owner as usize;
    if unsafe { *((owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) }
        != TITLE_STEP_MENU_JOB_WAIT
    {
        return;
    }
    // Optional slot override from the trigger file ("slot=N"); -1/absent => the game's
    // own most-recent selection.
    if let Some(dir) = game_directory_path() {
        if let Ok(content) = std::fs::read_to_string(dir.join("er-effects-own-stepper.txt")) {
            for line in content.lines() {
                if let Some(rest) = line.trim().strip_prefix("slot=") {
                    if let Ok(v) = rest.trim().parse::<i32>() {
                        OWN_STEPPER_SLOT.store(v, Ordering::SeqCst);
                    }
                }
            }
        }
    }
    let slot = module_base + TITLE_STEP_IDX10_SLOT_RVA;
    let orig = unsafe { *(slot as *const usize) };
    OWN_STEPPER_ORIG_IDX10.store(orig, Ordering::SeqCst);
    OWN_STEPPER_BASE.store(module_base, Ordering::SeqCst);
    // Own idx6 (STEP_GameStepWait) too, for the post-SetState(5) deserialize + re-target.
    let slot6 = module_base + TITLE_STEP_IDX6_SLOT_RVA;
    let orig6 = unsafe { *(slot6 as *const usize) };
    OWN_STEPPER_ORIG_IDX6.store(orig6, Ordering::SeqCst);
    unsafe { *(slot6 as *mut usize) = own_stepper_idx6 as usize };
    unsafe { *(slot as *mut usize) = own_stepper_idx10 as usize };
    OWN_STEPPER_PATCHED.store(OWN_STEPPER_PATCHED_YES, Ordering::SeqCst);
    let handler = own_stepper_idx10 as usize;
    let _ = TITLE_STEP_PLAY_GAME;
    append_autoload_debug(format_args!(
        "own_stepper: PATCHED idx10 slot=0x{slot:x} orig=0x{orig:x} -> handler=0x{handler:x} owner=0x{owner:x}"
    ));
}
