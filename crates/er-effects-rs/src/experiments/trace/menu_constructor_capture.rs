
/// Forward a captured menu-UI call through its trampoline. Uniform 4-arg fastcall: the
/// integer arg registers (rcx/rdx/r8/r9) pass through; callees taking fewer args ignore the
/// rest, and none of the captured targets take >4 integer args or float args. Returns rax.
unsafe fn call_cap_original(orig: &AtomicUsize, a: usize, b: usize, c: usize, d: usize) -> usize {
    let original = orig.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(original) };
    unsafe { f(a, b, c, d) }
}

/// Title CSMenu controller ctor 0x1409060d0 (real prologue entry; doc's 0x9060d8 was mid-
/// prologue): latches `router_this` (the object owning the
/// selectable Continue/Load/NewGame row vector at +0x1290) when its primary vtable
/// (runtime `base+0x2afa070`) is installed. router_this is NOT field-linked from the
/// TitleTopDialog, so this ctor capture is how the own-stepper obtains it. Pure observe +
/// pass-through; latches the first matching controller.
pub(crate) unsafe extern "system" fn cap_csmenu_ctor_hook(
    this: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const ROUTER_VEC_BEGIN_1290: usize = 0x1290;
    const ROUTER_VEC_END_1298: usize = 0x1298;
    let ret = unsafe { call_cap_original(&CAP_CSMENU_CTOR_ORIG, this, b, c, d) };
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != NULL {
            own
        } else {
            game_module_base().unwrap_or(NULL)
        }
    };
    if this != NULL && base != NULL {
        let vt = unsafe { safe_read_usize(this) }.unwrap_or(NULL);
        let vt_rva = vt.wrapping_sub(base);
        let matched = vt == base + ROUTER_THIS_VTABLE_RVA;
        if matched {
            MENU_ROUTER_THIS.store(this, Ordering::SeqCst);
        }
        // Log the first N constructions REGARDLESS of match: reveals whether this ctor fires
        // headless at all and the ACTUAL installed runtime vtable (vt_rva), so the inferred
        // ROUTER_THIS_VTABLE_RVA=0x2afa070 (derived via a +0xe00 dump skew, not measured) can be
        // corrected if wrong.
        let n = CAP_CSMENU_CTOR_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        if n < CAP_CSMENU_CTOR_LOG_FIRST {
            let vb = unsafe { safe_read_usize(this + ROUTER_VEC_BEGIN_1290) }.unwrap_or(NULL);
            let ve = unsafe { safe_read_usize(this + ROUTER_VEC_END_1298) }.unwrap_or(NULL);
            append_continue_trace(format_args!(
                "CAP csmenu_ctor #{n} this=0x{this:x} vt=0x{vt:x} vt_rva=0x{vt_rva:x} matched={matched} vec=[0x{vb:x}..0x{ve:x}] {}",
                trace_callers_summary()
            ));
        }
    }
    ret
}

/// Post-build scan of a row container (`rebuild_rows`/`append_one` rcx). The generic FD4 list
/// builder fires for EVERY menu list, so the title menu is identified by CONTENT: a row whose
/// action functor ([entry+0xf8] -> [+0] vtable -> [+0x10] _Do_call) chains to dialog_factory
/// 0x14081ead0 (Load-Game) or continue_confirm 0x140b0e180 (Continue). Captures the Load-Game /
/// Continue ROW ENTRIES (and router_this = container-0x1290) when found. Pure reads + classify
/// (the original already ran) -> save-safe. Called AFTER the original builds the rows.
unsafe fn inspect_row_container(tag: &str, container: usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const ENTRY_STRIDE_210: usize = 0x210;
    const ENTRY_ACTION_F8: usize = 0xf8;
    const ACTION_DOCALL_10: usize = 0x10;
    const ROW_VEC_OFFSET_1290: usize = 0x1290;
    const DIALOG_FACTORY_RVA: usize = LIVE_DIALOG_FACTORY_RVA;
    const PROBE_ENTRIES: usize = 8;
    const PROBE_START: usize = 0;
    const PROBE_STEP: usize = 1;
    const JMP_HOPS: usize = 5;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    if container == NULL {
        return;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != NULL {
            own
        } else {
            game_module_base().unwrap_or(NULL)
        }
    };
    if base == NULL {
        return;
    }
    let factory = base + DIALOG_FACTORY_RVA;
    let confirm = base + CONTINUE_CONFIRM_RVA;
    let begin = unsafe { safe_read_usize(container) }.unwrap_or(NULL);
    if begin == NULL {
        return;
    }
    let mut load_entry: usize = NULL;
    let mut cont_entry: usize = NULL;
    let mut i = PROBE_START;
    while i < PROBE_ENTRIES {
        let entry = begin + i * ENTRY_STRIDE_210;
        let action = unsafe { safe_read_usize(entry + ENTRY_ACTION_F8) }.unwrap_or(NULL);
        if action != NULL {
            let avt = unsafe { safe_read_usize(action) }.unwrap_or(NULL);
            if avt != NULL {
                let mut tgt = unsafe { safe_read_usize(avt + ACTION_DOCALL_10) }.unwrap_or(NULL);
                let mut hop = HOP_START;
                while hop < JMP_HOPS && tgt != NULL {
                    if tgt == factory {
                        load_entry = entry;
                        break;
                    }
                    if tgt == confirm {
                        cont_entry = entry;
                        break;
                    }
                    match unsafe { decode_thunk_hop(tgt) } {
                        Some(next) => tgt = next,
                        None => break,
                    }
                    hop += HOP_STEP;
                }
            }
        }
        i += PROBE_STEP;
    }
    if load_entry == NULL && cont_entry == NULL {
        return;
    }
    // This container IS the title menu row list. Latch the entries + a router_this candidate.
    if load_entry != NULL {
        MENU_LOADGAME_ROW_ENTRY.store(load_entry, Ordering::SeqCst);
    }
    if cont_entry != NULL {
        MENU_CONTINUE_ROW_ENTRY.store(cont_entry, Ordering::SeqCst);
    }
    let router_this = container.wrapping_sub(ROW_VEC_OFFSET_1290);
    MENU_ROUTER_THIS.store(router_this, Ordering::SeqCst);
    let n = CAP_ROW_PUSH_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n < CAP_ROW_PUSH_LOG_FIRST {
        let rvt = unsafe { safe_read_usize(router_this) }.unwrap_or(NULL);
        append_continue_trace(format_args!(
            "CAP row_push[{tag}] TITLE-MENU container=0x{container:x} begin=0x{begin:x} load_entry=0x{load_entry:x} cont_entry=0x{cont_entry:x} router_this?=0x{router_this:x} rvt=0x{rvt:x} {}",
            trace_callers_summary()
        ));
    }
}

/// rebuild_rows 0x14078d2c0(rcx=list-model container, rdx=src iterator pair): bulk-emplaces the
/// Continue/Load/NewGame rows. Firing headless proves the rows materialize zero-input; the
/// post-build scan isolates the title menu by row CONTENT.
pub(crate) unsafe extern "system" fn cap_rebuild_rows_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let ret = unsafe { call_cap_original(&CAP_REBUILD_ROWS_ORIG, a, b, c, d) };
    unsafe { log_row_push_caller("rebuild", a) };
    unsafe { inspect_row_container("rebuild", a) };
    ret
}

/// append_one 0x14078eea0(rcx=list-model, r8=&idx): single-row emplace.
pub(crate) unsafe extern "system" fn cap_append_one_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let ret = unsafe { call_cap_original(&CAP_APPEND_ONE_ORIG, a, b, c, d) };
    unsafe { log_row_push_caller("append", a) };
    unsafe { inspect_row_container("append", a) };
    ret
}

/// UNCONDITIONAL instrument-capture: log container + row-vector size + caller stack for the
/// first N rebuild_rows/append_one fires, regardless of content. This pins WHAT triggers the
/// TitleTopDialog CSMenu row populate (the input/focus-gated step confirmed missing zero-input).
/// Pure reads; the original already ran -> save-safe.
unsafe fn log_row_push_caller(tag: &str, container: usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const ROW_VEC_BEGIN_1290: usize = 0x1290;
    const ROW_VEC_END_1298: usize = 0x1298;
    let n = CAP_ROW_PUSH_ALLFIRE_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n >= CAP_ROW_PUSH_ALLFIRE_LOG_FIRST {
        return;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != NULL {
            own
        } else {
            game_module_base().unwrap_or(NULL)
        }
    };
    // container is the list-model; router_this back-ptr at [container+8], its row vector lives at
    // router_this+0x1290. Also probe the container itself in case it IS router_this.
    let backptr = unsafe { safe_read_usize(container + ROW_CONTAINER_BACKPTR_8) }.unwrap_or(NULL);
    let vb = unsafe { safe_read_usize(container + ROW_VEC_BEGIN_1290) }.unwrap_or(NULL);
    let ve = unsafe { safe_read_usize(container + ROW_VEC_END_1298) }.unwrap_or(NULL);
    let cvt = unsafe { safe_read_usize(container) }.unwrap_or(NULL);
    let cvt_rva = if base != NULL {
        cvt.wrapping_sub(base)
    } else {
        cvt
    };
    append_continue_trace(format_args!(
        "CAP row_push_ALL[{tag}] #{n} container=0x{container:x} cvt=0x{cvt:x}(rva 0x{cvt_rva:x}) backptr=0x{backptr:x} vec=[0x{vb:x}..0x{ve:x}] {}",
        trace_callers_summary()
    ));
}

/// Menu/FD4 insertion helper 0x1407a7b60(rcx=registry/builder, rdx=descriptor): passive capture of
/// the exact objects TitleTopDialog::open_menu inserts. This is intentionally generic: log the
/// original return plus a few qwords around rcx/rdx so the next static/runtime step can identify the
/// registry storage without guessing dialog fields or generic Sequence trees.
unsafe fn log_menu_insert_details(a: usize, b: usize, c: usize, d: usize, ret: usize) {
    let n = CAP_MENU_INSERT_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n < CAP_MENU_INSERT_LOG_FIRST {
        let q = |addr: usize, off: usize| -> usize {
            if addr == TITLE_OWNER_SCAN_START_ADDRESS {
                TITLE_OWNER_SCAN_START_ADDRESS
            } else {
                unsafe { safe_read_usize(addr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            }
        };
        let base = {
            let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
            if own != TITLE_OWNER_SCAN_START_ADDRESS {
                own
            } else {
                game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            }
        };
        let avt = q(a, CAP_MENU_INSERT_VTABLE_OFFSET);
        let bvt = q(b, CAP_MENU_INSERT_VTABLE_OFFSET);
        let rvt = q(ret, CAP_MENU_INSERT_VTABLE_OFFSET);
        let arva = if base != TITLE_OWNER_SCAN_START_ADDRESS {
            avt.wrapping_sub(base)
        } else {
            avt
        };
        let brva = if base != TITLE_OWNER_SCAN_START_ADDRESS {
            bvt.wrapping_sub(base)
        } else {
            bvt
        };
        let rrva = if base != TITLE_OWNER_SCAN_START_ADDRESS {
            rvt.wrapping_sub(base)
        } else {
            rvt
        };
        append_continue_trace(format_args!(
            "CAP menu_insert #{} rcx=0x{a:x} vt=0x{avt:x}(rva 0x{arva:x}) a8=0x{:x} a10=0x{:x} a18=0x{:x} a38=0x{:x} a50=0x{:x} rdx=0x{b:x} vt=0x{bvt:x}(rva 0x{brva:x}) b8=0x{:x} b10=0x{:x} b18=0x{:x} b38=0x{:x} r8=0x{c:x} r9=0x{d:x} ret=0x{ret:x} ret_vt=0x{rvt:x}(rva 0x{rrva:x}) ret8=0x{:x} ret10=0x{:x} ret18=0x{:x} {}",
            n,
            q(a, CAP_MENU_INSERT_QWORD_8_OFFSET),
            q(a, CAP_MENU_INSERT_QWORD_10_OFFSET),
            q(a, CAP_MENU_INSERT_QWORD_18_OFFSET),
            q(a, CAP_MENU_INSERT_QWORD_38_OFFSET),
            q(a, CAP_MENU_INSERT_QWORD_50_OFFSET),
            q(b, CAP_MENU_INSERT_QWORD_8_OFFSET),
            q(b, CAP_MENU_INSERT_QWORD_10_OFFSET),
            q(b, CAP_MENU_INSERT_QWORD_18_OFFSET),
            q(b, CAP_MENU_INSERT_QWORD_38_OFFSET),
            q(ret, CAP_MENU_INSERT_QWORD_8_OFFSET),
            q(ret, CAP_MENU_INSERT_QWORD_10_OFFSET),
            q(ret, CAP_MENU_INSERT_QWORD_18_OFFSET),
            trace_callers_summary()
        ));
    }
}

/// SetState 0x140b0d960(this, state): the title state machine setter. Logging every call
/// reveals the press-any-key advance + Continue's SetState(5) sequence.
pub(crate) unsafe extern "system" fn cap_setstate_hook(
    this: usize,
    state: usize,
    c: usize,
    d: usize,
) -> usize {
    append_continue_trace(format_args!(
        "CAP setstate this=0x{this:x} state={} {} {}",
        state as i32,
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    unsafe { call_cap_original(&CAP_SETSTATE_ORIG, this, state, c, d) }
}

// (cap_continue_confirm_hook was folded into system_quit_continue_confirm_hook in
// startup_hooks.rs -- the 0x140b0e180 detour is now installed unconditionally at attach and
// carries both the trace logging and the System->Quit fresh-deserialize guard.)

/// Load activate 0x1409a4670 = CS::ProfileLoadDialog vtable slot 20 (this = the dialog).
pub(crate) unsafe extern "system" fn cap_load_activate_hook(
    this: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    append_continue_trace(format_args!(
        "CAP load_activate(slot20) dialog_this=0x{this:x} a1=0x{b:x} a2=0x{c:x} a3=0x{d:x} {} {}",
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    unsafe { call_cap_original(&CAP_LOAD_ACTIVATE_ORIG, this, b, c, d) }
}

/// Load activate variant 0x1409ac760 (global-slot path).
pub(crate) unsafe extern "system" fn cap_load_activate2_hook(
    this: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    const ARG_Q0_OFFSET: usize = 0x0;
    const ARG_Q8_OFFSET: usize = 0x8;
    const ARG_Q10_OFFSET: usize = 0x10;
    const ARG_Q18_OFFSET: usize = 0x18;
    let q = |ptr: usize, off: usize| -> usize {
        if ptr != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        }
    };
    append_continue_trace(format_args!(
        "CAP load_activate2 this=0x{this:x}[0x{:x},0x{:x},0x{:x},0x{:x}] a1=0x{b:x}[0x{:x},0x{:x}] a2=0x{c:x}[0x{:x},0x{:x},0x{:x},0x{:x}] a3=0x{d:x}[0x{:x},0x{:x}] {} {}",
        q(this, ARG_Q0_OFFSET),
        q(this, ARG_Q8_OFFSET),
        q(this, ARG_Q10_OFFSET),
        q(this, ARG_Q18_OFFSET),
        q(b, ARG_Q0_OFFSET),
        q(b, ARG_Q8_OFFSET),
        q(c, ARG_Q0_OFFSET),
        q(c, ARG_Q8_OFFSET),
        q(c, ARG_Q10_OFFSET),
        q(c, ARG_Q18_OFFSET),
        q(d, ARG_Q0_OFFSET),
        q(d, ARG_Q8_OFFSET),
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    unsafe { call_cap_original(&CAP_LOAD_ACTIVATE2_ORIG, this, b, c, d) }
}

/// Enter-Load-Game builder 0x140826510(owner, rdx, r8d=slot, r9) -> selector step.
pub(crate) unsafe extern "system" fn cap_builder_hook(
    owner: usize,
    rdx: usize,
    slot: usize,
    r9: usize,
) -> usize {
    let slot_i32 = slot as i32;
    let expected_slot = OWN_STEPPER_EXPECTED_SLOT.load(Ordering::SeqCst);
    let effective_slot = slot;
    append_continue_trace(format_args!(
        "CAP builder owner=0x{owner:x} slot={} effective_slot={} rdx=0x{rdx:x} r9=0x{r9:x} {} {}",
        slot_i32,
        effective_slot as i32,
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_cap_original(&CAP_BUILDER_ORIG, owner, rdx, effective_slot, r9) };
    // PHASE A: the selector self-pump feed is diagnostic-only (live_dialog gate, hardcoded off);
    // product runs must not feed own_stepper's self-pump inputs.
    if live_dialog_enabled() && ret != TITLE_OWNER_SCAN_START_ADDRESS {
        #[repr(C)]
        struct SelectorBuilderOwnerLayout {
            unknown_000: [u8; 0xf8],
            selector_ctx: usize,
        }
        const SELECTOR_CTX_OFFSET_F8: usize =
            core::mem::offset_of!(SelectorBuilderOwnerLayout, selector_ctx);
        const SELECTOR_STEP_VTABLE_RVA: usize = ProfileLoadMenuRva::SelectorStepVtable as usize;
        let step = unsafe { safe_read_usize(ret) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let step_vt = if step != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_usize(step) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let ctx = ret + SELECTOR_CTX_OFFSET_F8;
        if game_module_base()
            .ok()
            .is_some_and(|base| step_vt == base + SELECTOR_STEP_VTABLE_RVA)
        {
            OWN_STEPPER_SELECTOR_STEP.store(step, Ordering::SeqCst);
            OWN_STEPPER_SELECTOR_CTX.store(ctx, Ordering::SeqCst);
        }
        append_autoload_debug(format_args!(
            "own_stepper: builder ret(owner)=0x{ret:x} step=[owner]=0x{step:x} step_vt=0x{step_vt:x} ctx(owner+0xf8)=0x{ctx:x} slot={} effective_slot={} for native selector self-pump",
            slot_i32, effective_slot as i32
        ));
    }
    ret
}

/// Selector-owner step tick 0x140826d50(step, ctx, result). Rate-limited (it ticks every
/// frame). Logs the step this, its +0x68 install flag, and the slot at ctx[0].
pub(crate) unsafe extern "system" fn cap_selector_tick_hook(
    step: usize,
    ctx: usize,
    result: usize,
    d: usize,
) -> usize {
    let n = CAP_SELECTOR_TICK_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n < CAP_SELECTOR_TICK_LOG_FIRST
        || n % CAP_SELECTOR_TICK_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS
    {
        let installed = if step != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((step + SELECTOR_STEP_INSTALL_FLAG_68_OFFSET) as *const u8) as i32 }
        } else {
            TITLE_STATE_OWNER_GONE
        };
        let ctx_slot = if ctx != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *(ctx as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        };
        const SELECTOR_STEP_Q10_OFFSET: usize =
            core::mem::size_of::<usize>() + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q18_OFFSET: usize =
            SELECTOR_STEP_Q10_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q20_OFFSET: usize =
            SELECTOR_STEP_Q18_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q28_OFFSET: usize =
            SELECTOR_STEP_Q20_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q30_OFFSET: usize =
            SELECTOR_STEP_Q28_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q38_OFFSET: usize =
            SELECTOR_STEP_Q30_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q50_OFFSET: usize = SELECTOR_STEP_Q38_OFFSET
            + core::mem::size_of::<usize>()
            + core::mem::size_of::<usize>()
            + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q58_OFFSET: usize =
            SELECTOR_STEP_Q50_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q60_OFFSET: usize =
            SELECTOR_STEP_Q58_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_TASK_OFFSET: usize = SELECTOR_STEP_Q60_OFFSET
            + core::mem::size_of::<usize>()
            + core::mem::size_of::<usize>();
        let step_q = |off: usize| -> usize {
            if step != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(step + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            }
        };
        let step_task = step_q(SELECTOR_STEP_TASK_OFFSET);
        let step_task_vt = if step_task != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_usize(step_task) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        const PTR_Q0_OFFSET: usize = 0x0;
        const PTR_Q8_OFFSET: usize = 0x8;
        const PTR_Q10_OFFSET: usize = 0x10;
        const PTR_Q18_OFFSET: usize = 0x18;
        let ptr_q = |ptr: usize, off: usize| -> usize {
            if ptr != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            }
        };
        let step_q10 = step_q(SELECTOR_STEP_Q10_OFFSET);
        let step_q18 = step_q(SELECTOR_STEP_Q18_OFFSET);
        let step_q20 = step_q(SELECTOR_STEP_Q20_OFFSET);
        let step_q28 = step_q(SELECTOR_STEP_Q28_OFFSET);
        let step_q30 = step_q(SELECTOR_STEP_Q30_OFFSET);
        let step_q38 = step_q(SELECTOR_STEP_Q38_OFFSET);
        let step_q50 = step_q(SELECTOR_STEP_Q50_OFFSET);
        let step_q58 = step_q(SELECTOR_STEP_Q58_OFFSET);
        let step_q60 = step_q(SELECTOR_STEP_Q60_OFFSET);
        append_continue_trace(format_args!(
            "CAP selector_tick #{n} step=0x{step:x} ctx=0x{ctx:x} installed={installed} ctx_slot={ctx_slot} task=0x{step_task:x} task_vt=0x{step_task_vt:x} step_q=[0x{step_q10:x},0x{step_q18:x},0x{step_q20:x},0x{step_q28:x},0x{step_q30:x},0x{step_q38:x},0x{step_q50:x},0x{step_q58:x},0x{step_q60:x}] q50_obj=[0x{:x},0x{:x},0x{:x},0x{:x}] q60_obj=[0x{:x},0x{:x},0x{:x},0x{:x}] {}",
            ptr_q(step_q50, PTR_Q0_OFFSET),
            ptr_q(step_q50, PTR_Q8_OFFSET),
            ptr_q(step_q50, PTR_Q10_OFFSET),
            ptr_q(step_q50, PTR_Q18_OFFSET),
            ptr_q(step_q60, PTR_Q0_OFFSET),
            ptr_q(step_q60, PTR_Q8_OFFSET),
            ptr_q(step_q60, PTR_Q10_OFFSET),
            ptr_q(step_q60, PTR_Q18_OFFSET),
            b80_mount_trace_summary()
        ));
    }
    unsafe { call_cap_original(&CAP_SELECTOR_TICK_ORIG, step, ctx, result, d) }
}

/// ProfileLoadDialog factory 0x14081ead0(rcx=ctx, rdx): builds the Load-Game dialog when the
/// main-menu "Load Game" item is activated. The caller backtrace pins the navigation chain.
pub(crate) unsafe extern "system" fn cap_dialog_factory_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    // Capture ALL four register args (rcx/rdx/r8/r9) AND a window of the rcx capture object so the
    // headless PATH-3-direct replay can reconstruct the exact factory invocation. The native
    // _Do_call thunk 0x140820c60 does `add rcx,8` before jmping here, so rcx (=a) is the lambda
    // capture state past the _Func_impl header; the ctor reads the owner from a field of it. Pure
    // reads + pass-through -> save-safe.
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const CAP_START: usize = 0;
    const CAP_WINDOW: usize = 7;
    const CAP_STEP: usize = 1;
    const PTR_SIZE: usize = 8;
    let mut capdump = String::new();
    // Dump [a-8 .. a+0x30] (the _Func_impl vtable at a-8, then capture fields).
    let mut i: usize = CAP_START;
    while i < CAP_WINDOW {
        let off = i * PTR_SIZE;
        let addr = a.wrapping_sub(PTR_SIZE).wrapping_add(off);
        let v = unsafe { safe_read_usize(addr) }.unwrap_or(NULL);
        capdump.push_str(&format!(" [rcx-8+0x{off:x}]=0x{v:x}"));
        i += CAP_STEP;
    }
    let rdx0 = unsafe { safe_read_usize(b) }.unwrap_or(NULL);
    let rdx8 = unsafe { safe_read_usize(b.wrapping_add(PTR_SIZE)) }.unwrap_or(NULL);
    append_continue_trace(format_args!(
        "CAP dialog_factory ENTER rcx=0x{a:x} rdx=0x{b:x} r8=0x{c:x} r9=0x{d:x} [rdx]=0x{rdx0:x} [rdx+8]=0x{rdx8:x}{capdump} {} {}",
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_cap_original(&CAP_DIALOG_FACTORY_ORIG, a, b, c, d) };
    let ret_vt = if ret != NULL {
        unsafe { safe_read_usize(ret) }.unwrap_or(NULL)
    } else {
        NULL
    };
    append_continue_trace(format_args!(
        "CAP dialog_factory LEAVE dialog_this=0x{ret:x} dialog_vt=0x{ret_vt:x}"
    ));
    let base = game_module_base().unwrap_or(NULL);
    // PHASE A: gate the S2 auto-entry on the ProfileSelect flow's own gate instead of
    // product_autoload_enabled(). NOTE: OWN_STEPPER_TITLE_FIRED is NOT set only by the
    // compile-time-off ProfileSelect flow -- it is also set by the open-menu park latch
    // (product_core_own_stepper.rs:1191, reachable in product) and the legacy title-confirm
    // fire (:1247, behind hardcoded-false legacy_menu_drive_enabled()). The regate is still
    // behavior-identical in product because none of those setters coincides with a
    // ProfileLoadDialog build there: the shipping flow drives the native Continue row and
    // never constructs a ProfileLoadDialog, so this branch's vtable check never passes on a
    // product run. This removes the last product-gated own_stepper entry.
    if profile_select_load_flow_enabled()
        && base != NULL
        && OWN_STEPPER_TITLE_FIRED.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS
        && OWN_STEPPER_PHASE.load(Ordering::SeqCst) == OWN_STEPPER_PHASE_MENU
        && ret_vt == base + PROFILE_LOAD_DIALOG_VTABLE_RVA
    {
        OWN_STEPPER_DIALOG.store(ret, Ordering::SeqCst);
        // DEFAULT (gate OFF): latch the live ProfileLoadDialog and immediately enter STAGE2 ACTIVATE
        // -- byte-identical to before (this branch is only reached when OWN_STEPPER_TITLE_FIRED is set,
        // which the proven native Continue commit never does, so the default char-load is untouched).
        // ProfileSelect load flow (gate ON): keep the dialog latched but HOLD at PHASE_MENU so the
        // flow can render+capture the portrait first; it drives the STAGE2 transition itself.
        if !profile_select_load_flow_enabled() {
            own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_ACTIVATE);
        }
        append_autoload_debug(format_args!(
            "product-core-autoload: native TitleTopDialog Load-Game factory returned ProfileLoadDialog=0x{ret:x} vt=0x{ret_vt:x}; captured by factory hook (profile_select_flow={} -> {})",
            profile_select_load_flow_enabled(),
            if profile_select_load_flow_enabled() {
                "HOLD at MENU for portrait"
            } else {
                "STAGE2 ACTIVATE"
            }
        ));
    }
    ret
}

/// Menu deserialize 0x14082c240(this, ctx): the real mount (writes GameMan+0xc30 + char).
pub(crate) unsafe extern "system" fn cap_menu_deser_hook(
    this: usize,
    ctx: usize,
    c: usize,
    d: usize,
) -> usize {
    let ctx_slot = if ctx != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { *(ctx as *const i32) }
    } else {
        TITLE_STATE_OWNER_GONE
    };
    append_continue_trace(format_args!(
        "CAP menu_deser ENTER this=0x{this:x} ctx=0x{ctx:x} ctx_slot={ctx_slot} {} {}",
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    {
        const Q0: usize = 0x0;
        const Q1: usize = 0x8;
        const Q2: usize = 0x10;
        const Q3: usize = 0x18;
        let q = |ptr: usize, off: usize| -> usize {
            if ptr != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            }
        };
        let io = game_module_base()
            .ok()
            .map(|base| unsafe { *((base + IODEV_GLOBAL_RVA) as *const usize) })
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let io18 = q(io, IODEV_REQHANDLE_18_OFFSET);
        let io20 = q(io, IODEV_REQHANDLE_20_OFFSET);
        append_continue_trace(format_args!(
            "CAP menu_deser RAW this=[0x{:x},0x{:x},0x{:x},0x{:x}] ctx=[0x{:x},0x{:x},0x{:x},0x{:x}] io18=0x{io18:x}[0x{:x},0x{:x},0x{:x},0x{:x}] io20=0x{io20:x}[0x{:x},0x{:x},0x{:x},0x{:x}]",
            q(this, Q0),
            q(this, Q1),
            q(this, Q2),
            q(this, Q3),
            q(ctx, Q0),
            q(ctx, Q1),
            q(ctx, Q2),
            q(ctx, Q3),
            q(io18, Q0),
            q(io18, Q1),
            q(io18, Q2),
            q(io18, Q3),
            q(io20, Q0),
            q(io20, Q1),
            q(io20, Q2),
            q(io20, Q3),
        ));
    }
    let ret = unsafe { call_cap_original(&CAP_MENU_DESER_ORIG, this, ctx, c, d) };
    append_continue_trace(format_args!(
        "CAP menu_deser LEAVE ret=0x{ret:x} {}",
        b80_mount_trace_summary()
    ));
    ret
}

/// Title native-ready predicate 0x140733150 hook. Static RE shows the original body is:
/// `state = this->vtable[0](this); return (state->flags_20 & 0x8f) != 0`. Re-implement that tiny
/// body exactly so the hook can record the returned state object/flags without making a second
/// native getter call or changing success semantics.
pub(crate) unsafe extern "system" fn title_native_ready_predicate_hook(this: usize) -> usize {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const STATE_FLAGS_20_OFFSET: usize = 0x20;
    const READY_MASK_8F: usize = 0x8f;
    type StateGetter = unsafe extern "system" fn(usize) -> usize;

    let caller_rva = trace_first_game_caller_rva();
    let vtable = unsafe { safe_read_usize(this) }.unwrap_or(NULL);
    let getter = if vtable != NULL {
        unsafe { safe_read_usize(vtable) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let state = if getter != NULL {
        let f: StateGetter = unsafe { std::mem::transmute(getter) };
        unsafe { f(this) }
    } else {
        NULL
    };
    let flags = if state != NULL {
        unsafe { safe_read_usize(state + STATE_FLAGS_20_OFFSET) }.unwrap_or(0) & 0xff
    } else {
        0
    };
    let masked = flags & READY_MASK_8F;
    let ret = if masked != 0 { 1 } else { 0 };

    TITLE_NATIVE_READY_PREDICATE_HITS.fetch_add(1, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_THIS.store(this, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_VTABLE.store(vtable, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_GETTER.store(getter, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_OBJECT.store(state, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_FLAGS.store(flags, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_MASKED.store(masked, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_RET.store(ret, Ordering::SeqCst);

    ret
}

/// MenuWindowJob ctor 0x1407ac8c0 hook: observe constructed menu jobs and latch the semantic
/// Continue item only when both the Continue action and native accept predicate are installed.
/// This avoids poisoning MENU_CONTINUE_ITEM with the first updated title input leaf, whose
/// accept predicate is the constant-false 0x1407add70 diagnostic dead end.
pub(crate) unsafe extern "system" fn menu_window_job_ctor_hook(
    out_slot: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let ret = unsafe { call_cap_original(&MENU_WINDOW_JOB_CTOR_ORIG, out_slot, b, c, d) };
    if !product_autoload_enabled() || out_slot == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != TITLE_OWNER_SCAN_START_ADDRESS {
            own
        } else {
            game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        }
    };
    if base == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET: usize = 0xf8;
    const MENU_ITEM_ACCEPT_NATIVE_RVA: usize = 0x007ad810;
    let item = unsafe { safe_read_usize(out_slot) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    if item == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor = unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor_vt = if functor != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let do_call = if functor_vt != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor_vt + DOCALL_VTABLE_SLOT_10) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let accept_predicate = unsafe { safe_read_usize(item + MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    MENU_WINDOW_JOB_CTOR_HITS.fetch_add(1, Ordering::SeqCst);
    MENU_WINDOW_JOB_CTOR_LAST_ITEM.store(item, Ordering::SeqCst);
    MENU_WINDOW_JOB_CTOR_LAST_VT.store(vt, Ordering::SeqCst);
    MENU_WINDOW_JOB_CTOR_LAST_FUNCTOR.store(functor, Ordering::SeqCst);
    MENU_WINDOW_JOB_CTOR_LAST_DOCALL.store(do_call, Ordering::SeqCst);
    MENU_WINDOW_JOB_CTOR_LAST_ACCEPT.store(accept_predicate, Ordering::SeqCst);
    let continue_candidate =
        vt == base + MENU_WINDOW_JOB_VTABLE_RVA && do_call == base + MENU_TITLE_CONTINUE_DOCALL_RVA;
    if continue_candidate {
        record_continue_candidate(item, accept_predicate, base);
    }
    let semantic_continue_item =
        continue_candidate && accept_predicate == base + MENU_ITEM_ACCEPT_NATIVE_RVA;
    if semantic_continue_item {
        MENU_WINDOW_JOB_CTOR_SEMANTIC_HITS.fetch_add(1, Ordering::SeqCst);
    }
    if semantic_continue_item
        && MENU_CONTINUE_ITEM
            .compare_exchange(
                TITLE_OWNER_SCAN_START_ADDRESS,
                item,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
    {
        append_continue_trace(format_args!(
            "MENU-WINDOW-CTOR captured semantic native Continue item=0x{item:x} out=0x{out_slot:x} vt=0x{vt:x} functor=0x{functor:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x} item_fields{{{}}} {}",
            unsafe { menu_item_action_summary(item) },
            trace_callers_summary()
        ));
        append_autoload_debug(format_args!(
            "product-core-autoload: constructor captured semantic native Continue MenuWindowJob item=0x{item:x} vt=0x{vt:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x}"
        ));
    }
    ret
}

/// MenuWindowJob native-accept ctor variant 0x1407acb00 hook: observe constructed menu jobs from
/// the sibling constructor that static RE shows installs the native accept predicate 0x1407ad810.
/// This is passive except for the same semantic pointer latch used by the existing 0x1407ac8c0
/// constructor hook: if the item is a Continue row with native accept, record its pointer so the
/// product path can later submit through native semantics.
pub(crate) unsafe extern "system" fn menu_window_job_native_ctor_b_hook(
    out_slot: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let caller_rva = trace_first_game_caller_rva();
    let ret = unsafe { call_cap_original(&MENU_WINDOW_JOB_NATIVE_CTOR_B_ORIG, out_slot, b, c, d) };
    if !product_autoload_enabled() || out_slot == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != TITLE_OWNER_SCAN_START_ADDRESS {
            own
        } else {
            game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        }
    };
    if base == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET: usize = 0xf8;
    const MENU_ITEM_ACCEPT_NATIVE_RVA: usize = 0x007ad810;
    let item = unsafe { safe_read_usize(out_slot) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    if item == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor = unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor_vt = if functor != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let do_call = if functor_vt != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor_vt + DOCALL_VTABLE_SLOT_10) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let accept_predicate = unsafe { safe_read_usize(item + MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_HITS.fetch_add(1, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_ITEM.store(item, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_OUT_SLOT.store(out_slot, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_VT.store(vt, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_FUNCTOR.store(functor, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_DOCALL.store(do_call, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_ACCEPT.store(accept_predicate, Ordering::SeqCst);
    let semantic_continue_item = vt == base + MENU_WINDOW_JOB_VTABLE_RVA
        && do_call == base + MENU_TITLE_CONTINUE_DOCALL_RVA
        && accept_predicate == base + MENU_ITEM_ACCEPT_NATIVE_RVA;
    if semantic_continue_item {
        MENU_WINDOW_JOB_NATIVE_CTOR_B_CONTINUE_HITS.fetch_add(1, Ordering::SeqCst);
        record_continue_candidate(item, accept_predicate, base);
        if MENU_CONTINUE_ITEM
            .compare_exchange(
                TITLE_OWNER_SCAN_START_ADDRESS,
                item,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            append_continue_trace(format_args!(
                "MENU-WINDOW-NATIVE-CTOR-B captured semantic native Continue item=0x{item:x} caller_rva=0x{caller_rva:x} out=0x{out_slot:x} vt=0x{vt:x} functor=0x{functor:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x} item_fields{{{}}} {}",
                unsafe { menu_item_action_summary(item) },
                trace_callers_summary()
            ));
            append_autoload_debug(format_args!(
                "product-core-autoload: native ctor B captured semantic native Continue MenuWindowJob item=0x{item:x} caller_rva=0x{caller_rva:x} accept_predicate=0x{accept_predicate:x}"
            ));
        }
    }
    ret
}

/// MenuWindowJob disabled/idle ctor 0x1407acf80 hook: observe constructed menu jobs whose accept
/// functor is the constant-false 0x1407add70 variant. Static RE of the constructor shows it builds
/// the same MenuWindowJob vtable but installs the idle predicate into item+0xf0/+0xf8; this hook
/// attributes Continue-looking candidates to that disabled native path without promoting or
/// submitting them.
pub(crate) unsafe extern "system" fn menu_window_job_idle_ctor_hook(
    out_slot: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let caller_rva = trace_first_game_caller_rva();
    let ret = unsafe { call_cap_original(&MENU_WINDOW_JOB_IDLE_CTOR_ORIG, out_slot, b, c, d) };
    if !product_autoload_enabled() || out_slot == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != TITLE_OWNER_SCAN_START_ADDRESS {
            own
        } else {
            game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        }
    };
    if base == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET: usize = 0xf8;
    let item = unsafe { safe_read_usize(out_slot) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    if item == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor = unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor_vt = if functor != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let do_call = if functor_vt != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor_vt + DOCALL_VTABLE_SLOT_10) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let accept_predicate = unsafe { safe_read_usize(item + MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    MENU_WINDOW_JOB_IDLE_CTOR_HITS.fetch_add(1, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_ITEM.store(item, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_VT.store(vt, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_FUNCTOR.store(functor, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_DOCALL.store(do_call, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_ACCEPT.store(accept_predicate, Ordering::SeqCst);
    let continue_candidate =
        vt == base + MENU_WINDOW_JOB_VTABLE_RVA && do_call == base + MENU_TITLE_CONTINUE_DOCALL_RVA;
    if continue_candidate {
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_HITS.fetch_add(1, Ordering::SeqCst);
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_ITEM.store(item, Ordering::SeqCst);
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_OUT_SLOT.store(out_slot, Ordering::SeqCst);
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_DOCALL.store(do_call, Ordering::SeqCst);
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_ACCEPT.store(accept_predicate, Ordering::SeqCst);
        record_continue_candidate(item, accept_predicate, base);
        append_continue_trace(format_args!(
            "MENU-WINDOW-IDLE-CTOR observed Continue-looking disabled item=0x{item:x} caller_rva=0x{caller_rva:x} vt=0x{vt:x} functor=0x{functor:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x} item_fields{{{}}} {}",
            unsafe { menu_item_action_summary(item) },
            trace_callers_summary()
        ));
    }
    ret
}

/// MenuWindowJob::Update 0x1407ad1c0 hook: the native menu pump calls this with rcx = a
/// menu-item each tick. We let the game walk its own (CSMenu) tree and CAPTURE the item
/// whose +0xa8 action functor's _Do_call chain resolves to dialog_factory 0x14081ead0 (=
/// the Load-Game item) into MENU_LOAD_GAME_ITEM, so the own-stepper can drive it
/// zero-input without guessing the container layout. Pure observe + pass-through (no
/// behaviour change). Logs the first distinct items to map the live title menu.
pub(crate) unsafe extern "system" fn cap_menu_item_update_hook(
    item: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    // Module base independent of the own-stepper (so this hook also works during a
    // user-driven trace with the own-stepper off): own-stepper base if set, else resolve it.
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != TITLE_OWNER_SCAN_START_ADDRESS {
            own
        } else {
            game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        }
    };
    if product_autoload_enabled()
        && item != TITLE_OWNER_SCAN_START_ADDRESS
        && base != TITLE_OWNER_SCAN_START_ADDRESS
    {
        const DOCALL_VTABLE_SLOT_10: usize = 0x10;
        const MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET: usize = 0xf8;
        const MENU_ITEM_ACCEPT_NATIVE_RVA: usize = 0x007ad810;
        let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let functor = unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let functor_vt = if functor != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_usize(functor) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let do_call = if functor_vt != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_usize(functor_vt + DOCALL_VTABLE_SLOT_10) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let accept_predicate =
            unsafe { safe_read_usize(item + MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        MENU_ITEM_UPDATE_HITS.fetch_add(1, Ordering::SeqCst);
        MENU_ITEM_UPDATE_LAST_ITEM.store(item, Ordering::SeqCst);
        MENU_ITEM_UPDATE_LAST_VT.store(vt, Ordering::SeqCst);
        MENU_ITEM_UPDATE_LAST_FUNCTOR.store(functor, Ordering::SeqCst);
        MENU_ITEM_UPDATE_LAST_DOCALL.store(do_call, Ordering::SeqCst);
        MENU_ITEM_UPDATE_LAST_ACCEPT.store(accept_predicate, Ordering::SeqCst);
        let continue_candidate = vt == base + MENU_WINDOW_JOB_VTABLE_RVA
            && do_call == base + MENU_TITLE_CONTINUE_DOCALL_RVA;
        if continue_candidate {
            record_continue_candidate(item, accept_predicate, base);
        }
        let semantic_continue_item =
            continue_candidate && accept_predicate == base + MENU_ITEM_ACCEPT_NATIVE_RVA;
        if semantic_continue_item {
            MENU_ITEM_UPDATE_SEMANTIC_HITS.fetch_add(1, Ordering::SeqCst);
        }
        if semantic_continue_item
            && MENU_CONTINUE_ITEM
                .compare_exchange(
                    TITLE_OWNER_SCAN_START_ADDRESS,
                    item,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
        {
            append_continue_trace(format_args!(
                "MENU-ITEM-UPDATE captured semantic native Continue item=0x{item:x} vt=0x{vt:x} functor=0x{functor:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x} item_fields{{{}}} {}",
                unsafe { menu_item_action_summary(item) },
                trace_callers_summary()
            ));
            append_autoload_debug(format_args!(
                "product-core-autoload: captured semantic native Continue MenuWindowJob item=0x{item:x} vt=0x{vt:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x}"
            ));
        }
    }
    if product_autoload_enabled()
        && item != TITLE_OWNER_SCAN_START_ADDRESS
        && item == MENU_CONTINUE_ITEM.load(Ordering::SeqCst)
    {
        let n =
            MENU_CONTINUE_ITEM_FIELD_LOG_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        const FIELD_LOG_0: usize = 0;
        const FIELD_LOG_8: usize = 8;
        const FIELD_LOG_30: usize = 30;
        const FIELD_LOG_60: usize = 60;
        const FIELD_LOG_120: usize = 120;
        if n == FIELD_LOG_0
            || n == FIELD_LOG_8
            || n == FIELD_LOG_30
            || n == FIELD_LOG_60
            || n == FIELD_LOG_120
        {
            append_continue_trace(format_args!(
                "MENU-ITEM-UPDATE Continue candidate fields tick_count={n} item=0x{item:x} item_fields{{{}}} {}",
                unsafe { menu_item_action_summary(item) },
                trace_callers_summary()
            ));
        }
    }
    // While the deterministic input probe is active, count GENUINE d180 leaf-Update ticks (this
    // leaf fn 0x1407ad1c0 actually running for the Load-Game item) even after MENU_LOAD_GAME_ITEM
    // is already latched -- so the probe can tell "d180 leaf ticked" from "static walk found it".
    if INPUT_PROBE_ACTIVE.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS
        && item != TITLE_OWNER_SCAN_START_ADDRESS
        && base != TITLE_OWNER_SCAN_START_ADDRESS
        && MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS
    {
        let mut chain = String::new();
        if unsafe { functor_chain_hits_factory(item, base, &mut chain) } {
            MENU_D180_LEAF_TICKED.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        }
    }
    if item != TITLE_OWNER_SCAN_START_ADDRESS
        && base != TITLE_OWNER_SCAN_START_ADDRESS
        && MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS
    {
        let mut chain = String::new();
        let is_load_game = unsafe { functor_chain_hits_factory(item, base, &mut chain) };
        if is_load_game {
            MENU_LOAD_GAME_ITEM.store(item, Ordering::SeqCst);
            MENU_D180_LEAF_TICKED.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            append_continue_trace(format_args!(
                "MENU-ITEM-UPDATE captured LOAD-GAME item=0x{item:x} {chain} {}",
                trace_callers_summary()
            ));
        } else if MENU_ITEM_UPDATE_LAST.swap(item, Ordering::SeqCst) != item {
            // New distinct item ticked: log it once. CAPPED -- with a few items rotating
            // each frame this otherwise floods the size-capped trace and rolls the early
            // SEQ-ITER-CHILD enumeration off. The capture (MENU_LOAD_GAME_ITEM) is unaffected.
            let n =
                MENU_ITEM_UPDATE_CAPTURE_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            if n < MENU_ITEM_UPDATE_LOG_MAX {
                let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                if product_autoload_enabled() {
                    append_continue_trace(format_args!(
                        "MENU-ITEM-UPDATE #{n} item=0x{item:x} vt=0x{vt:x} item_fields{{{}}} {chain} load_game=false {}",
                        unsafe { menu_item_action_summary(item) },
                        trace_callers_summary()
                    ));
                } else {
                    append_continue_trace(format_args!(
                        "MENU-ITEM-UPDATE #{n} item=0x{item:x} vt=0x{vt:x} {chain} load_game=false {}",
                        trace_callers_summary()
                    ));
                }
            }
        }
    }
    unsafe { call_cap_original(&MENU_ITEM_UPDATE_ORIG, item, b, c, d) }
}

/// FD4 Sequence::Update / child-iterator 0x1407aa1f0 hook. The opened main-menu registers the
/// Load-Game leaf d180 but it does NOT tick (only the focused entry ticks the leaf Update, so
/// `cap_menu_item_update_hook` misses d180). This iterator runs on every Sequence node; we
/// walk its inline child array ([seq+0x18 + i*8], count [seq+0x60]) and classify each child by
/// the action-functor `_Do_call` chain (`functor_chain_hits_factory` -> dialog_factory
/// 0x14081ead0). The unique hit is d180 / Load-Game -- captured regardless of focus, then read
/// by own_stepper idx10 (MENU_LOAD_GAME_ITEM) for the Stage-2 functor invoke. Early-outs once
/// found (the iterator is hot); fault-tolerant reads never AV; pure read, NO writes/calls into
/// the game beyond the original.
pub(crate) unsafe extern "system" fn cap_sequence_iter_hook(
    seq: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    const PTR_STRIDE: usize = core::mem::size_of::<usize>();
    const WALK_START: usize = 0;
    const WALK_STEP: usize = 1;
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != TITLE_OWNER_SCAN_START_ADDRESS {
            own
        } else {
            game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        }
    };
    if seq != TITLE_OWNER_SCAN_START_ADDRESS
        && base != TITLE_OWNER_SCAN_START_ADDRESS
        && MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS
    {
        let count = unsafe { safe_read_usize(seq + SEQUENCE_COUNT_60_OFFSET) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        // Unconditional structural dump (first N calls): what does the iterator walk?
        let ndbg = SEQ_ITER_DEBUG_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        if ndbg < SEQ_ITER_DEBUG_MAX {
            let seq_vt = unsafe { safe_read_usize(seq) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let child0 = unsafe { safe_read_usize(seq + SEQUENCE_CHILDREN_BASE_18_OFFSET) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let child0_vt = if child0 != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(child0) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            };
            append_continue_trace(format_args!(
                "SEQ-ITER-DBG #{ndbg} seq=0x{seq:x} seqvt=0x{seq_vt:x} count={count} child0=0x{child0:x} child0vt=0x{child0_vt:x}"
            ));
        }
        if (SEQUENCE_CHILD_COUNT_MIN..=SEQUENCE_CHILD_COUNT_MAX).contains(&count) {
            let mut i = WALK_START;
            while i < count {
                let child = unsafe {
                    safe_read_usize(seq + SEQUENCE_CHILDREN_BASE_18_OFFSET + i * PTR_STRIDE)
                }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                if child != TITLE_OWNER_SCAN_START_ADDRESS {
                    let mut chain = String::new();
                    let child_vt =
                        unsafe { safe_read_usize(child) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                    if unsafe { functor_chain_hits_factory(child, base, &mut chain) } {
                        MENU_LOAD_GAME_ITEM.store(child, Ordering::SeqCst);
                        append_continue_trace(format_args!(
                            "SEQ-ITER captured LOAD-GAME child=0x{child:x} vt=0x{child_vt:x} seq=0x{seq:x} count={count} idx={i} {chain}"
                        ));
                        break;
                    }
                    // A MenuWindowJob child means the main menu actually opened (its entries
                    // are registered into a Sequence the iterator walks) -- signal the STAGE1d
                    // retry loop to stop. The title views tick via a different pump, so this
                    // fires ONLY on the real main-menu entries.
                    if child_vt == base + MENU_WINDOW_JOB_VTABLE_RVA {
                        MENU_ENTRIES_SEEN.store(MENU_ENTRIES_SEEN_YES, Ordering::SeqCst);
                    }
                    // Diagnostic: surface distinct MenuWindowJob children (the registered menu
                    // entries, ticking or not) with their docall chain so one run reveals the
                    // opened-menu structure (which entry is Load-Game). Capped to avoid flooding.
                    if child_vt == base + MENU_WINDOW_JOB_VTABLE_RVA
                        && SEQ_ITER_CHILD_LAST.swap(child, Ordering::SeqCst) != child
                    {
                        let nlog = SEQ_ITER_CHILD_LOG_COUNT
                            .fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                        if nlog < SEQ_ITER_CHILD_LOG_MAX {
                            append_continue_trace(format_args!(
                                "SEQ-ITER-CHILD #{nlog} child=0x{child:x} seq=0x{seq:x} count={count} idx={i} {chain}"
                            ));
                        }
                    }
                }
                i += WALK_STEP;
            }
        }
    }
    unsafe { call_cap_original(&SEQUENCE_ITER_ORIG, seq, b, c, d) }
}
