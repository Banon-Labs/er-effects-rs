
fn format_optional_usize_hex(value: usize) -> String {
    if value == TITLE_OWNER_SCAN_START_ADDRESS {
        "null".to_owned()
    } else {
        format!("0x{value:x}")
    }
}

unsafe fn result_built_flag(result: usize) -> usize {
    const RESULT_BUILT_3B0_OFFSET: usize = 0x3b0;
    const U8_MASK: usize = 0xff;
    if result == TITLE_OWNER_SCAN_START_ADDRESS {
        TITLE_OWNER_SCAN_START_ADDRESS
    } else {
        unsafe { safe_read_usize(result + RESULT_BUILT_3B0_OFFSET) }
            .map_or(TITLE_OWNER_SCAN_START_ADDRESS, |value| value & U8_MASK)
    }
}

unsafe fn native_result_event_words(event: usize) -> (usize, usize) {
    const EVENT_WORD0_OFFSET: usize = 0;
    const EVENT_WORD1_OFFSET: usize = core::mem::size_of::<usize>();
    if event == TITLE_OWNER_SCAN_START_ADDRESS {
        return (
            TITLE_OWNER_SCAN_START_ADDRESS,
            TITLE_OWNER_SCAN_START_ADDRESS,
        );
    }
    let word0 = unsafe { safe_read_usize(event + EVENT_WORD0_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let word1 = unsafe { safe_read_usize(event + EVENT_WORD1_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    (word0, word1)
}

fn fd4_event_code_arg(raw_qword0: usize) -> (usize, usize) {
    const U32_MASK: usize = 0xffff_ffff;
    if raw_qword0 == TITLE_OWNER_SCAN_START_ADDRESS {
        return (
            TITLE_OWNER_SCAN_START_ADDRESS,
            TITLE_OWNER_SCAN_START_ADDRESS,
        );
    }
    (raw_qword0 & U32_MASK, (raw_qword0 >> 32) & U32_MASK)
}

pub(crate) unsafe extern "system" fn native_submit_hook(result: usize) {
    const TRACE_FIRST: usize = 16;
    let seq =
        NATIVE_SUBMIT_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) + OWN_STEPPER_CALL_INC;
    NATIVE_SUBMIT_LAST_RESULT.store(result, Ordering::SeqCst);
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "native_submit_7ac890 seq={seq} phase=ENTER result=0x{result:x} built={} {}",
            format_optional_usize_hex(unsafe { result_built_flag(result) }),
            trace_callers_summary()
        ));
    }
    let _ = unsafe { call_result_void1_original(&NATIVE_SUBMIT_ORIG, result) };
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "native_submit_7ac890 seq={seq} phase=LEAVE result=0x{result:x} built={} {}",
            format_optional_usize_hex(unsafe { result_built_flag(result) }),
            game_man_trace_summary()
        ));
    }
}

pub(crate) unsafe extern "system" fn result_event_handler_hook(result: usize, event: usize) {
    const TRACE_FIRST: usize = 16;
    let seq = RESULT_EVENT_HANDLER_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    RESULT_EVENT_LAST_RESULT.store(result, Ordering::SeqCst);
    RESULT_EVENT_LAST_EVENT.store(event, Ordering::SeqCst);
    let (event_raw_qword0, _) = unsafe { native_result_event_words(event) };
    let (fd4_code, fd4_arg) = fd4_event_code_arg(event_raw_qword0);
    RESULT_EVENT_LAST_RAW_QWORD0.store(event_raw_qword0, Ordering::SeqCst);
    RESULT_EVENT_LAST_FD4_CODE.store(fd4_code, Ordering::SeqCst);
    RESULT_EVENT_LAST_FD4_ARG.store(fd4_arg, Ordering::SeqCst);
    let built_before = unsafe { result_built_flag(result) };
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "result_event_handler_746e80 seq={seq} phase=ENTER result=0x{result:x} event=0x{event:x} event_raw_qword0={} fd4_code={} fd4_arg={} built_before={} {}",
            format_optional_usize_hex(event_raw_qword0),
            format_optional_usize_hex(fd4_code),
            format_optional_usize_hex(fd4_arg),
            format_optional_usize_hex(built_before),
            trace_callers_summary()
        ));
    }
    let _ = unsafe { call_result_void2_original(&RESULT_EVENT_HANDLER_ORIG, result, event) };
    let built_after = unsafe { result_built_flag(result) };
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "result_event_handler_746e80 seq={seq} phase=LEAVE result=0x{result:x} event=0x{event:x} built_after={} {}",
            format_optional_usize_hex(built_after),
            game_man_trace_summary()
        ));
    }
}

pub(crate) unsafe extern "system" fn result_event_wrapper_builder_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
) -> usize {
    const TRACE_FIRST: usize = 16;
    const RESULT_ACTION_BUILDER_TRACE_SIZE: usize = 0x360;
    let from_result_action_builder = callstack_contains_game_rva(
        RESULT_ACTION_BUILDER_RVA as usize,
        RESULT_ACTION_BUILDER_RVA as usize + RESULT_ACTION_BUILDER_TRACE_SIZE,
    );
    let result = unsafe { call_wrapper_builder_original(rcx, rdx, r8) }.unwrap_or(rcx);
    if from_result_action_builder {
        let seq = RESULT_ACTION_WRAPPER_BUILDER_HITS
            .fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
            + OWN_STEPPER_CALL_INC;
        let base = game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let ret_update_rva = if base == TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_OWNER_SCAN_START_ADDRESS
        } else {
            unsafe { task_node_update_rva(base, result) }
        };
        RESULT_ACTION_LAST_WRAPPER_BUILDER_RCX.store(rcx, Ordering::SeqCst);
        RESULT_ACTION_LAST_WRAPPER_BUILDER_RDX.store(rdx, Ordering::SeqCst);
        RESULT_ACTION_LAST_WRAPPER_BUILDER_R8.store(r8, Ordering::SeqCst);
        RESULT_ACTION_LAST_WRAPPER_BUILDER_RET.store(result, Ordering::SeqCst);
        RESULT_ACTION_LAST_WRAPPER_BUILDER_RET_UPDATE_RVA.store(ret_update_rva, Ordering::SeqCst);
        if seq <= TRACE_FIRST {
            append_continue_trace(format_args!(
                "result_event_wrapper_builder_744a60 seq={seq} rcx=0x{rcx:x} rdx=0x{rdx:x} r8=0x{r8:x} ret=0x{result:x} ret_update_rva={} -- passive wrapper-builder call from result action builder",
                format_optional_usize_hex(ret_update_rva)
            ));
        }
    }
    result
}

pub(crate) unsafe extern "system" fn result_action_builder_hook(result: usize, event: usize) {
    const TRACE_FIRST: usize = 16;
    let seq = RESULT_ACTION_BUILDER_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    RESULT_ACTION_LAST_RESULT.store(result, Ordering::SeqCst);
    RESULT_ACTION_LAST_EVENT.store(event, Ordering::SeqCst);
    let (event_word0, event_word1) = unsafe { native_result_event_words(event) };
    RESULT_ACTION_LAST_WORD0.store(event_word0, Ordering::SeqCst);
    RESULT_ACTION_LAST_WORD1.store(event_word1, Ordering::SeqCst);
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "result_action_builder_746a00 seq={seq} phase=ENTER result=0x{result:x} event=0x{event:x} event_word0={} event_word1={} built={} {}",
            format_optional_usize_hex(event_word0),
            format_optional_usize_hex(event_word1),
            format_optional_usize_hex(unsafe { result_built_flag(result) }),
            trace_callers_summary()
        ));
    }
    let _ = unsafe { call_result_void2_original(&RESULT_ACTION_BUILDER_ORIG, result, event) };
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "result_action_builder_746a00 seq={seq} phase=LEAVE result=0x{result:x} event=0x{event:x} built={} {}",
            format_optional_usize_hex(unsafe { result_built_flag(result) }),
            game_man_trace_summary()
        ));
    }
}

pub(crate) unsafe extern "system" fn menu_task_update_wrapper_hook(
    this: *mut c_void,
) -> *mut c_void {
    unsafe {
        append_menu_semaphore_trace(
            "menu_task_update_wrapper",
            "ENTER",
            TRACE_MENU_TASK_UPDATE_WRAPPER_RVA,
            TRACE_MENU_TASK_UPDATE_TABLE_RVA,
            this,
        )
    };
    let result =
        unsafe { call_wrapper_original(&MENU_TASK_UPDATE_WRAPPER_ORIG, this) }.unwrap_or(this);
    unsafe {
        append_menu_semaphore_trace(
            "menu_task_update_wrapper",
            "LEAVE",
            TRACE_MENU_TASK_UPDATE_WRAPPER_RVA,
            TRACE_MENU_TASK_UPDATE_TABLE_RVA,
            result,
        )
    };
    result
}

unsafe fn text_section_bounds(base: usize) -> Option<(usize, usize)> {
    let e_lfanew = unsafe { safe_read_usize(base + PE_DOS_LFANEW_OFFSET) }? & PE_U32_MASK;
    let nt = base + e_lfanew;
    let num_sections = unsafe { safe_read_usize(nt + PE_FILE_NUM_SECTIONS_OFFSET) }? & PE_U16_MASK;
    let size_opt = unsafe { safe_read_usize(nt + PE_FILE_SIZE_OPT_HEADER_OFFSET) }? & PE_U16_MASK;
    let sections = nt + PE_OPT_HEADER_OFFSET + size_opt;
    let mut index = PE_SECTION_SCAN_START;
    while index < num_sections {
        let header = sections + index * PE_SECTION_HEADER_SIZE;
        let name = unsafe { safe_read_usize(header) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        if name.to_le_bytes().starts_with(PE_TEXT_SECTION_NAME) {
            let vsize = unsafe { safe_read_usize(header + PE_SECTION_VSIZE_OFFSET) }? & PE_U32_MASK;
            let vaddr = unsafe { safe_read_usize(header + PE_SECTION_VADDR_OFFSET) }? & PE_U32_MASK;
            return Some((base + vaddr, vsize));
        }
        index += OWN_STEPPER_CALL_INC;
    }
    None
}

unsafe fn update_target_in_text(base: usize, update: usize) -> bool {
    if update < base {
        return false;
    }
    let Some((text_start, text_len)) = (unsafe { text_section_bounds(base) }) else {
        return false;
    };
    update >= text_start && update < text_start.saturating_add(text_len)
}

unsafe fn raw_task_node_update_rva(base: usize, node: usize) -> usize {
    const TASK_NODE_UPDATE_VTABLE_SLOT: usize = 0x10;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let Some(vtable) = (unsafe { safe_read_usize(node) }) else {
        return null;
    };
    let Some(update) = (unsafe { safe_read_usize(vtable + TASK_NODE_UPDATE_VTABLE_SLOT) }) else {
        return null;
    };
    if unsafe { update_target_in_text(base, update) } {
        update - base
    } else {
        null
    }
}

pub(crate) unsafe fn task_node_update_rva(base: usize, node: usize) -> usize {
    let direct = unsafe { raw_task_node_update_rva(base, node) };
    if direct != TITLE_OWNER_SCAN_START_ADDRESS {
        return direct;
    }
    let Some(shared_pointee) = (unsafe { safe_read_usize(node) }) else {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    };
    unsafe { raw_task_node_update_rva(base, shared_pointee) }
}

unsafe fn qword_window_summary(ptr: usize) -> String {
    const QWORDS: usize = 6;
    const START: usize = 0;
    const STEP: usize = 1;
    const STRIDE: usize = core::mem::size_of::<usize>();
    let mut out = String::new();
    let mut i = START;
    while i < QWORDS {
        let off = i * STRIDE;
        let value = unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let _ = core::fmt::write(&mut out, format_args!(" +0x{off:x}=0x{value:x}"));
        i += STEP;
    }
    out
}

unsafe fn menu_item_action_summary(ptr: usize) -> String {
    const OFFSETS: [usize; 14] = [
        0x0, 0x8, 0x10, 0x40, 0x50, 0x68, 0xa8, 0xb0, 0xe8, 0xf0, 0xf8, 0x100, 0x130, 0x138,
    ];
    let mut out = String::new();
    for off in OFFSETS {
        let value = unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let _ = core::fmt::write(&mut out, format_args!(" +0x{off:x}=0x{value:x}"));
        if value != TITLE_OWNER_SCAN_START_ADDRESS {
            let _ = core::fmt::write(
                &mut out,
                format_args!(" ->{{{}}}", unsafe { qword_window_summary(value) }),
            );
        }
    }
    out
}

unsafe fn task_node_raw_summary(ptr: usize) -> String {
    const QWORDS: usize = 8;
    const START: usize = 0;
    const STEP: usize = 1;
    const STRIDE: usize = core::mem::size_of::<usize>();
    let mut out = String::new();
    let mut first = TITLE_OWNER_SCAN_START_ADDRESS;
    let mut second = TITLE_OWNER_SCAN_START_ADDRESS;
    let mut i = START;
    while i < QWORDS {
        let off = i * STRIDE;
        let value = unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        if i == START {
            first = value;
        } else if i == STEP {
            second = value;
        }
        let _ = core::fmt::write(&mut out, format_args!(" +0x{off:x}=0x{value:x}"));
        i += STEP;
    }
    if first != TITLE_OWNER_SCAN_START_ADDRESS {
        let _ = core::fmt::write(
            &mut out,
            format_args!(" | *q0{{{}}}", unsafe { qword_window_summary(first) }),
        );
    }
    if second != TITLE_OWNER_SCAN_START_ADDRESS {
        let _ = core::fmt::write(
            &mut out,
            format_args!(" | *q8{{{}}}", unsafe { qword_window_summary(second) }),
        );
    }
    out
}

unsafe fn capture_continue_task_node_candidate(base: usize, candidate: usize, label: &str) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if candidate == null {
        return;
    }
    let update_rva = unsafe { task_node_update_rva(base, candidate) };
    if update_rva != TRACE_MENU_CONTINUE_WRAPPER_RVA as usize {
        return;
    }
    if MENU_CONTINUE_TASK_NODE
        .compare_exchange(null, candidate, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        append_continue_trace(format_args!(
            "CAP continue_task_node {label}=0x{candidate:x} update_rva=0x{update_rva:x} -- captured native Continue menu task wrapper"
        ));
        append_autoload_debug(format_args!(
            "product-core-autoload: captured native Continue task node from {label}=0x{candidate:x} update_rva=0x{update_rva:x}"
        ));
    }
}

unsafe fn capture_continue_member_node_candidate(base: usize, candidate: usize, label: &str) {
    const MEMBER_DIALOG_10: usize = core::mem::size_of::<usize>() + core::mem::size_of::<usize>();
    const MEMBER_FN_18: usize = 0x18;
    const MEMBER_ADJ_20: usize = 0x20;
    const JMP_HOPS: usize = 6;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if candidate == null {
        return;
    }
    let node_vt = unsafe { safe_read_usize(candidate) }.unwrap_or(null);
    if node_vt != base + MEMBERFUNCJOB_VTABLE_RVA {
        return;
    }
    let member_fn = unsafe { safe_read_usize(candidate + MEMBER_FN_18) }.unwrap_or(null);
    if member_fn == null {
        return;
    }
    let continue_wrapper = base + TRACE_MENU_CONTINUE_WRAPPER_RVA as usize;
    let mut target = member_fn;
    let mut hop = 0;
    while hop < JMP_HOPS && target != null {
        if target == continue_wrapper {
            let member_dialog =
                unsafe { safe_read_usize(candidate + MEMBER_DIALOG_10) }.unwrap_or(null);
            let member_adjust =
                unsafe { safe_read_usize(candidate + MEMBER_ADJ_20) }.unwrap_or(null);
            if MENU_CONTINUE_MEMBER_NODE
                .compare_exchange(null, candidate, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                append_continue_trace(format_args!(
                    "CAP continue_member_node {label}=0x{candidate:x} node_vt=0x{node_vt:x} member_dialog=0x{member_dialog:x} member_fn=0x{member_fn:x} member_adjust=0x{member_adjust:x} -- captured registered TitleTopDialog Continue MenuMemberFuncJob"
                ));
                append_autoload_debug(format_args!(
                    "product-core-autoload: captured registered TitleTopDialog Continue MenuMemberFuncJob from {label}=0x{candidate:x} member_fn=0x{member_fn:x}"
                ));
            }
            return;
        }
        match unsafe { decode_thunk_hop(target) } {
            Some(next) => target = next,
            None => break,
        }
        hop += 1;
    }
}

pub(crate) unsafe extern "system" fn task_enqueue_hook(
    arg0: *mut c_void,
    arg1: *mut c_void,
) -> *mut c_void {
    let caller_rva = trace_first_game_caller_rva();
    let trace_index = TASK_ENQUEUE_TRACE_COUNT
        .fetch_add(TASK_ENQUEUE_TRACE_INCREMENT, Ordering::SeqCst)
        + TASK_ENQUEUE_TRACE_INCREMENT;
    let should_trace = trace_index <= TASK_ENQUEUE_TRACE_LIMIT
        || SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst)
            > NO_SAFE_INPUT_CONFIRM_FRAMES;
    if should_trace {
        append_continue_trace(format_args!(
            "menu_task_enqueue seq={trace_index} phase=ENTER hook_rva=0x{:x} list={arg0:p} node={arg1:p} node_{} raw{{{}}} confirm_active={} pulse={} {} {}",
            TRACE_TASK_ENQUEUE_RVA,
            unsafe { object_vtable_summary(arg1) },
            unsafe { task_node_raw_summary(arg1 as usize) },
            SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst)
                > NO_SAFE_INPUT_CONFIRM_FRAMES,
            SAFE_INPUT_CONFIRM_PULSE_SEQ.load(Ordering::SeqCst),
            trace_callers_summary(),
            game_man_trace_summary()
        ));
    }
    let result = unsafe { call_task_enqueue_original(arg0, arg1) }.unwrap_or(arg1);
    let arg0_pointee = if arg0 as usize != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(arg0 as usize) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let generic_hit = TASK_ENQUEUE_GENERIC_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    TASK_ENQUEUE_GENERIC_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    TASK_ENQUEUE_GENERIC_LAST_ARG0.store(arg0 as usize, Ordering::SeqCst);
    TASK_ENQUEUE_GENERIC_LAST_ARG0_POINTEE.store(arg0_pointee, Ordering::SeqCst);
    TASK_ENQUEUE_GENERIC_LAST_ARG1.store(arg1 as usize, Ordering::SeqCst);
    TASK_ENQUEUE_GENERIC_LAST_RET.store(result as usize, Ordering::SeqCst);
    match generic_hit {
        1 => {
            TASK_ENQUEUE_GENERIC_SAMPLE0_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE0_ARG0.store(arg0 as usize, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE0_ARG0_POINTEE.store(arg0_pointee, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE0_ARG1.store(arg1 as usize, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE0_RET.store(result as usize, Ordering::SeqCst);
        }
        2 => {
            TASK_ENQUEUE_GENERIC_SAMPLE1_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE1_ARG0.store(arg0 as usize, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE1_ARG0_POINTEE.store(arg0_pointee, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE1_ARG1.store(arg1 as usize, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE1_RET.store(result as usize, Ordering::SeqCst);
        }
        _ => {}
    }
    const MENU_CONTINUE_IDLE_INSERT_CALLER_RVA: usize = 0x0076432c;
    const MENU_CONTINUE_IDLE_INSERT_CALLER_START_RVA: usize = 0x007642b0;
    const MENU_CONTINUE_IDLE_INSERT_CALLER_END_RVA: usize = 0x007643c0;
    let idle_ctor_out_slot =
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_OUT_SLOT.load(Ordering::SeqCst);
    let idle_ctor_item = MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_ITEM.load(Ordering::SeqCst);
    let arg0_points_to_idle_item = arg0_pointee == idle_ctor_item;
    const TASK_ENQUEUE_IDLE_MATCH_CALLER_EXACT: usize = 1;
    const TASK_ENQUEUE_IDLE_MATCH_CALLER_RANGE: usize = 2;
    const TASK_ENQUEUE_IDLE_MATCH_ARG0_OUT_SLOT: usize = 3;
    const TASK_ENQUEUE_IDLE_MATCH_ARG0_POINTEE: usize = 4;
    const TASK_ENQUEUE_IDLE_MATCH_ARG1_ITEM: usize = 5;
    let stack_contains_idle_caller = callstack_contains_game_rva(
        MENU_CONTINUE_IDLE_INSERT_CALLER_START_RVA,
        MENU_CONTINUE_IDLE_INSERT_CALLER_END_RVA,
    );
    let idle_match_kind = if caller_rva == MENU_CONTINUE_IDLE_INSERT_CALLER_RVA {
        TASK_ENQUEUE_IDLE_MATCH_CALLER_EXACT
    } else if stack_contains_idle_caller {
        TASK_ENQUEUE_IDLE_MATCH_CALLER_RANGE
    } else if idle_ctor_out_slot != TITLE_OWNER_SCAN_START_ADDRESS
        && arg0 as usize == idle_ctor_out_slot
    {
        TASK_ENQUEUE_IDLE_MATCH_ARG0_OUT_SLOT
    } else if idle_ctor_item != TITLE_OWNER_SCAN_START_ADDRESS && arg0_points_to_idle_item {
        TASK_ENQUEUE_IDLE_MATCH_ARG0_POINTEE
    } else if idle_ctor_item != TITLE_OWNER_SCAN_START_ADDRESS && arg1 as usize == idle_ctor_item {
        TASK_ENQUEUE_IDLE_MATCH_ARG1_ITEM
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let idle_continue_insert_match = idle_match_kind != TITLE_OWNER_SCAN_START_ADDRESS;
    if idle_continue_insert_match {
        TASK_ENQUEUE_GENERIC_IDLE_ITEM_MATCH_HITS.fetch_add(1, Ordering::SeqCst);
        TASK_ENQUEUE_GENERIC_IDLE_ITEM_LAST_MATCH_KIND.store(idle_match_kind, Ordering::SeqCst);
    }
    if idle_continue_insert_match {
        let hit = MENU_CONTINUE_IDLE_INSERT_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        let base = game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let arg1_update_rva = if base == TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_OWNER_SCAN_START_ADDRESS
        } else {
            unsafe { task_node_update_rva(base, arg1 as usize) }
        };
        let ret_update_rva = if base == TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_OWNER_SCAN_START_ADDRESS
        } else {
            unsafe { task_node_update_rva(base, result as usize) }
        };
        MENU_CONTINUE_IDLE_INSERT_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
        MENU_CONTINUE_IDLE_INSERT_LAST_ARG0.store(arg0 as usize, Ordering::SeqCst);
        MENU_CONTINUE_IDLE_INSERT_LAST_ARG1.store(arg1 as usize, Ordering::SeqCst);
        MENU_CONTINUE_IDLE_INSERT_LAST_RET.store(result as usize, Ordering::SeqCst);
        MENU_CONTINUE_IDLE_INSERT_LAST_ARG1_UPDATE_RVA.store(arg1_update_rva, Ordering::SeqCst);
        MENU_CONTINUE_IDLE_INSERT_LAST_RET_UPDATE_RVA.store(ret_update_rva, Ordering::SeqCst);
        if hit <= CAP_MENU_INSERT_LOG_FIRST as u64 {
            append_continue_trace(format_args!(
                "MENU-CONTINUE-IDLE-INSERT seq={hit} caller_rva=0x{caller_rva:x} arg0={arg0:p} arg1={arg1:p} arg1_update_rva={} ret={result:p} ret_update_rva={} -- passive disabled Continue insert edge via 0x{:x}",
                format_optional_usize_hex(arg1_update_rva),
                format_optional_usize_hex(ret_update_rva),
                TRACE_TASK_ENQUEUE_RVA
            ));
        }
    }
    const RESULT_ACTION_BUILDER_TRACE_SIZE: usize = 0x360;
    if callstack_contains_game_rva(
        RESULT_ACTION_BUILDER_RVA as usize,
        RESULT_ACTION_BUILDER_RVA as usize + RESULT_ACTION_BUILDER_TRACE_SIZE,
    ) {
        let hit = RESULT_ACTION_INSERT_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
            + OWN_STEPPER_CALL_INC;
        RESULT_ACTION_LAST_INSERT_ARG0.store(arg0 as usize, Ordering::SeqCst);
        RESULT_ACTION_LAST_INSERT_ARG1.store(arg1 as usize, Ordering::SeqCst);
        RESULT_ACTION_LAST_INSERT_RET.store(result as usize, Ordering::SeqCst);
        let base = game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let arg1_update_rva = if base == TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_OWNER_SCAN_START_ADDRESS
        } else {
            unsafe { task_node_update_rva(base, arg1 as usize) }
        };
        let ret_update_rva = if base == TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_OWNER_SCAN_START_ADDRESS
        } else {
            unsafe { task_node_update_rva(base, result as usize) }
        };
        RESULT_ACTION_LAST_INSERT_ARG1_UPDATE_RVA.store(arg1_update_rva, Ordering::SeqCst);
        RESULT_ACTION_LAST_INSERT_RET_UPDATE_RVA.store(ret_update_rva, Ordering::SeqCst);
        if hit <= CAP_MENU_INSERT_LOG_FIRST {
            append_continue_trace(format_args!(
                "result_action_builder_insert seq={hit} arg0={arg0:p} arg1={arg1:p} arg1_update_rva={} ret={result:p} ret_update_rva={} -- passive downstream action node insert via 0x{:x}",
                format_optional_usize_hex(arg1_update_rva),
                format_optional_usize_hex(ret_update_rva),
                TRACE_TASK_ENQUEUE_RVA
            ));
        }
    }
    if let Ok(base) = game_module_base() {
        unsafe { capture_continue_task_node_candidate(base, arg1 as usize, "arg1") };
        unsafe { capture_continue_task_node_candidate(base, result as usize, "ret") };
        unsafe { capture_continue_member_node_candidate(base, arg1 as usize, "arg1") };
        unsafe { capture_continue_member_node_candidate(base, result as usize, "ret") };
    }
    unsafe {
        log_menu_insert_details(
            arg0 as usize,
            arg1 as usize,
            TITLE_OWNER_SCAN_START_ADDRESS,
            TITLE_OWNER_SCAN_START_ADDRESS,
            result as usize,
        );
    }
    if should_trace {
        append_continue_trace(format_args!(
            "menu_task_enqueue seq={trace_index} phase=LEAVE ret={result:p} ret_{} raw{{{}}} {}",
            unsafe { object_vtable_summary(result) },
            unsafe { task_node_raw_summary(result as usize) },
            game_man_trace_summary()
        ));
    }
    result
}

pub(crate) unsafe extern "system" fn set_save_slot_hook(slot: i32) {
    append_continue_trace(format_args!(
        "ENTER set_save_slot slot={slot} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = SET_SAVE_SLOT_ORIG.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(i32) = unsafe { std::mem::transmute(original) };
        unsafe { original(slot) };
    }
    append_continue_trace(format_args!(
        "LEAVE set_save_slot {}",
        game_man_trace_summary()
    ));
}

pub(crate) unsafe extern "system" fn save_request_profile_hook(enabled: u8) {
    append_continue_trace(format_args!(
        "ENTER save_request_profile enabled={enabled} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = SAVE_REQUEST_PROFILE_ORIG.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(u8) = unsafe { std::mem::transmute(original) };
        unsafe { original(enabled) };
    }
    append_continue_trace(format_args!(
        "LEAVE save_request_profile {}",
        game_man_trace_summary()
    ));
}

pub(crate) unsafe extern "system" fn request_save_hook(enabled: u8) {
    append_continue_trace(format_args!(
        "ENTER request_save enabled={enabled} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = REQUEST_SAVE_ORIG.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(u8) = unsafe { std::mem::transmute(original) };
        unsafe { original(enabled) };
    }
    append_continue_trace(format_args!(
        "LEAVE request_save {}",
        game_man_trace_summary()
    ));
}

pub(crate) unsafe extern "system" fn current_slot_load_hook(arg0: i32, arg1: u8, arg2: u8) -> u8 {
    append_continue_trace(format_args!(
        "ENTER current_slot_load_67b570 arg0={arg0} arg1={arg1} arg2={arg2} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let ret = unsafe { call_bool3_original(&CURRENT_SLOT_LOAD_ORIG, arg0, arg1, arg2) }
        .unwrap_or(HOOK_FALSE_RETURN);
    append_continue_trace(format_args!(
        "LEAVE current_slot_load_67b570 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn continue_load_hook(slot: i32, arg1: u8, arg2: u8) -> u8 {
    append_continue_trace(format_args!(
        "ENTER continue_load_67b750 slot={slot} arg1={arg1} arg2={arg2} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let ret = unsafe { call_bool3_original(&CONTINUE_LOAD_ORIG, slot, arg1, arg2) }
        .unwrap_or(HOOK_FALSE_RETURN);
    append_continue_trace(format_args!(
        "LEAVE continue_load_67b750 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn combined_load_hook(slot: i32, arg1: u8, arg2: u8) -> u8 {
    append_continue_trace(format_args!(
        "ENTER combined_load_67b940 slot={slot} arg1={arg1} arg2={arg2} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let ret = unsafe { call_bool3_original(&COMBINED_LOAD_ORIG, slot, arg1, arg2) }
        .unwrap_or(HOOK_FALSE_RETURN);
    append_continue_trace(format_args!(
        "LEAVE combined_load_67b940 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn map_load_hook() -> u8 {
    append_continue_trace(format_args!(
        "ENTER map_load_67bc10 {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = MAP_LOAD_ORIG.load(Ordering::SeqCst);
    let ret = if original == HOOK_ORIGINAL_UNSET {
        HOOK_FALSE_RETURN
    } else {
        let original: unsafe extern "system" fn() -> u8 = unsafe { std::mem::transmute(original) };
        unsafe { original() }
    };
    if ret != HOOK_FALSE_RETURN {
        TITLE_HANDOFF_COMPLETE.store(TITLE_HANDOFF_COMPLETE_VALUE, Ordering::SeqCst);
    }
    append_continue_trace(format_args!(
        "LEAVE map_load_67bc10 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn save_load_state_init_hook() -> u8 {
    append_continue_trace(format_args!(
        "ENTER save_load_state_init_67b030 {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = SAVE_LOAD_STATE_INIT_ORIG.load(Ordering::SeqCst);
    let ret = if original == HOOK_ORIGINAL_UNSET {
        HOOK_FALSE_RETURN
    } else {
        let original: unsafe extern "system" fn() -> u8 = unsafe { std::mem::transmute(original) };
        unsafe { original() }
    };
    append_continue_trace(format_args!(
        "LEAVE save_load_state_init_67b030 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}
