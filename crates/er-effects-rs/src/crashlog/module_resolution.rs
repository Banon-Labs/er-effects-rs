
/// Locate the live module's .text section [start, len) by parsing the PE headers at `base`.
unsafe fn find_text_section(base: usize) -> Option<(usize, usize)> {
    let e_lfanew = unsafe { safe_read_usize(base + PE_DOS_LFANEW_OFFSET) }? & PE_U32_MASK;
    let nt = base + e_lfanew;
    let num_sections = unsafe { safe_read_usize(nt + PE_FILE_NUM_SECTIONS_OFFSET) }? & PE_U16_MASK;
    let size_opt = unsafe { safe_read_usize(nt + PE_FILE_SIZE_OPT_HEADER_OFFSET) }? & PE_U16_MASK;
    let sections = nt + PE_OPT_HEADER_OFFSET + size_opt;
    let mut index = PE_SECTION_SCAN_START;
    while index < num_sections {
        let header = sections + index * PE_SECTION_HEADER_SIZE;
        let name = unsafe { safe_read_usize(header) }.unwrap_or(NULL_MODULE_BASE);
        if name.to_le_bytes().starts_with(PE_TEXT_SECTION_NAME) {
            let vsize = unsafe { safe_read_usize(header + PE_SECTION_VSIZE_OFFSET) }? & PE_U32_MASK;
            let vaddr = unsafe { safe_read_usize(header + PE_SECTION_VADDR_OFFSET) }? & PE_U32_MASK;
            return Some((base + vaddr, vsize));
        }
        index += ANTI_ANTIDEBUG_STEP;
    }
    None
}

/// Port of ProDebug's patchDbgChecks, corrected for ER 1.16.1: scan THIS module's .text (resolved
/// from the real game_module_base, not GetModuleHandle(NULL) which ProDebug got wrong under the
/// LazyLoader) for the timed anti-debug patterns and neutralize them, so debug exceptions reach
/// our VEH. Patches are tiny (branch-offset edits) per ANTI_ANTIDEBUG_CHECKS. Runs once.
pub(crate) unsafe fn apply_anti_antidebug_once(base: usize) {
    if ANTI_ANTIDEBUG_APPLIED.swap(ANTI_ANTIDEBUG_STEP, Ordering::SeqCst)
        != ANTI_ANTIDEBUG_NOT_APPLIED
    {
        return;
    }
    let Some((start, len)) = (unsafe { find_text_section(base) }) else {
        append_crash_log(format_args!(
            "anti-antidebug: .text not found at base=0x{base:x}"
        ));
        return;
    };
    let text = unsafe { std::slice::from_raw_parts(start as *const u8, len) };
    for (find_spec, patch_spec) in ANTI_ANTIDEBUG_CHECKS {
        let find = parse_byte_pattern(find_spec);
        let patch = parse_byte_pattern(patch_spec);
        let plen = find.len();
        let Some(Some(first)) = find.first().copied() else {
            continue;
        };
        if plen == ANTI_ANTIDEBUG_COUNT_INIT || plen > len {
            continue;
        }
        let mut count = ANTI_ANTIDEBUG_COUNT_INIT;
        let mut i = ANTI_ANTIDEBUG_COUNT_INIT;
        while i + plen <= len {
            if text[i] == first {
                let matched = find
                    .iter()
                    .enumerate()
                    .all(|(j, pat)| pat.is_none_or(|b| text[i + j] == b));
                if matched {
                    let match_addr = start + i;
                    for (j, pat) in patch.iter().enumerate() {
                        if let Some(b) = pat {
                            unsafe { write_code_byte(match_addr + j, *b) };
                        }
                    }
                    count += ANTI_ANTIDEBUG_STEP;
                }
            }
            i += ANTI_ANTIDEBUG_STEP;
        }
        append_crash_log(format_args!(
            "anti-antidebug: patched {count} site(s) for pattern 0x{first:x} (len {plen})"
        ));
    }
    unsafe {
        FlushInstructionCache(
            ER_CURRENT_PROCESS_PSEUDO_HANDLE,
            std::ptr::null(),
            FLUSH_WHOLE_PROCESS_SIZE,
        )
    };
    append_crash_log(format_args!(
        "anti-antidebug: done over .text 0x{start:x}..0x{:x}",
        start + len
    ));
}

/// Install the crash/exit logger: a vectored handler for access violations plus
/// MinHooks on the process-exit paths. The exit hooks catch a CLEAN watchdog
/// termination (ExitProcess) that no exception debugger can observe, and record
/// which game code requested the exit.
pub(crate) fn install_crash_logger() {
    CRASH_LOGGER_INSTALLED.call_once(|| {
        unsafe { AddVectoredExceptionHandler(VECTORED_FIRST_HANDLER, crash_vectored_handler) };
        match unsafe { MH_Initialize() } {
            MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
            status => append_crash_log(format_args!(
                "crash-logger MH_Initialize failed: {status:?}"
            )),
        }
        let targets: [(&str, &[u8], &[u8], *mut c_void, &AtomicUsize); CRASH_EXIT_TARGET_COUNT] = [
            (
                "ExitProcess",
                b"kernel32.dll\0",
                b"ExitProcess\0",
                exit_process_hook as *mut c_void,
                &ORIGINAL_EXIT_PROCESS,
            ),
            (
                "TerminateProcess",
                b"kernel32.dll\0",
                b"TerminateProcess\0",
                terminate_process_hook as *mut c_void,
                &ORIGINAL_TERMINATE_PROCESS,
            ),
            (
                "RtlExitUserProcess",
                b"ntdll.dll\0",
                b"RtlExitUserProcess\0",
                rtl_exit_user_process_hook as *mut c_void,
                &ORIGINAL_RTL_EXIT_USER_PROCESS,
            ),
            (
                "NtTerminateProcess",
                b"ntdll.dll\0",
                b"NtTerminateProcess\0",
                nt_terminate_process_hook as *mut c_void,
                &ORIGINAL_NT_TERMINATE_PROCESS,
            ),
        ];
        for (name, module, proc, hook_impl, original) in targets {
            match safe_input_proc(module, proc) {
                Ok(target) => unsafe {
                    create_and_apply_single_hook(name, target, hook_impl, original)
                },
                Err(error) => {
                    append_crash_log(format_args!("crash-logger resolve {name} failed: {error}"))
                }
            }
        }
        // Hook the assert wrapper by absolute address (not an export) to capture
        // the failing assertion before its deliberate crash.
        match game_module_base() {
            Ok(base) => unsafe {
                create_and_apply_single_hook(
                    "AssertWrapper",
                    (base + ASSERT_WRAPPER_RVA) as *mut c_void,
                    assert_wrapper_hook as *mut c_void,
                    &ORIGINAL_ASSERT_WRAPPER,
                )
            },
            Err(error) => append_crash_log(format_args!(
                "crash-logger assert-wrapper base failed: {error}"
            )),
        }
        append_crash_log(format_args!(
            "crash logger installed (VEH + exit-path hooks + assert wrapper)"
        ));
    });
}

pub(crate) unsafe fn object_vtable_summary(ptr: *mut c_void) -> String {
    if ptr.is_null() {
        return "vtable_rva=null".to_owned();
    }
    let vtable = unsafe { *(ptr as *const usize) };
    let rva = game_module_base()
        .ok()
        .and_then(|module_base| vtable.checked_sub(module_base));
    rva.map_or_else(
        || format!("vtable=0x{vtable:x} vtable_rva=unknown"),
        |value| format!("vtable=0x{vtable:x} vtable_rva=0x{value:x}"),
    )
}

#[cfg(windows)]
pub(crate) fn trace_callers_summary() -> String {
    let mut frames = [std::ptr::null_mut::<c_void>(); STACK_TRACE_FRAME_COUNT];
    let captured = unsafe {
        RtlCaptureStackBackTrace(
            STACK_TRACE_FRAMES_TO_SKIP,
            frames.len() as u32,
            frames.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    } as usize;
    // Resolve against the real game module base (not GetModuleHandleA(NULL), which under Wine can
    // return the EXE or fail) and annotate frames that fall in our relocated DLL as `self+RVA`.
    let game_base = game_module_base().unwrap_or(NULL_MODULE_BASE);
    let callers = frames
        .iter()
        .take(captured)
        .enumerate()
        .map(|(index, frame)| {
            let address = *frame as usize;
            let tag = annotate_addr(address, game_base);
            format!("#{index}=0x{address:x}{tag}")
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("callers=[{callers}]")
}

#[cfg(windows)]
pub(crate) fn callstack_contains_game_rva(start_rva: usize, end_rva: usize) -> bool {
    let mut frames = [std::ptr::null_mut::<c_void>(); STACK_TRACE_FRAME_COUNT];
    let captured = unsafe {
        RtlCaptureStackBackTrace(
            STACK_TRACE_FRAMES_TO_SKIP,
            frames.len() as u32,
            frames.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    } as usize;
    let module_base = unsafe { GetModuleHandleA(PCSTR::null()) }
        .ok()
        .map(|module| module.0 as usize)
        .unwrap_or(NULL_MODULE_BASE);
    if module_base == NULL_MODULE_BASE {
        return false;
    }
    frames.iter().take(captured).any(|frame| {
        let address = *frame as usize;
        address >= module_base
            && address.wrapping_sub(module_base) >= start_rva
            && address.wrapping_sub(module_base) < end_rva
    })
}

/// GX command-queue producer attribution (`gx_reserve_cmd_queue_slot_hook`): walk the captured
/// stack and return `(producer_rva, self_in_stack)` -- the first game-.text return address (as an
/// RVA) that falls OUTSIDE `wrapper_rvas` (the reserve/enqueue transport band), plus whether any
/// frame BELOW the game code lies inside our own DLL image (submissions our pipeline caused vs
/// pure-native ones). The stack's leading frames are our own instrumentation (this helper + the
/// MinHook detour), so self frames only count AFTER a non-self frame has appeared -- counting the
/// prefix tagged every reserve as +self (observed run autostep10d: 8/8 producers false-tagged).
/// `producer_rva` is 0 when no qualifying game frame was captured.
#[cfg(windows)]
pub(crate) fn stack_producer_rva(wrapper_rvas: std::ops::Range<usize>) -> (usize, bool) {
    let mut frames = [std::ptr::null_mut::<c_void>(); STACK_TRACE_FRAME_COUNT];
    let captured = unsafe {
        RtlCaptureStackBackTrace(
            STACK_TRACE_FRAMES_TO_SKIP,
            frames.len() as u32,
            frames.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    } as usize;
    let game_base = game_module_base().unwrap_or(NULL_MODULE_BASE);
    let self_base = SELF_DLL_BASE.load(Ordering::SeqCst);
    let self_size = SELF_DLL_SIZE.load(Ordering::SeqCst);
    let mut producer = 0usize;
    let mut self_in_stack = false;
    let mut past_own_prefix = false;
    for frame in frames.iter().take(captured) {
        let address = *frame as usize;
        if self_base != NULL_MODULE_BASE && address.wrapping_sub(self_base) < self_size {
            if past_own_prefix {
                self_in_stack = true;
            }
            continue;
        }
        past_own_prefix = true;
        if game_base == NULL_MODULE_BASE {
            continue;
        }
        let Some(rva) = address.checked_sub(game_base) else {
            continue;
        };
        if !(AV_GAME_TEXT_RVA_MIN..AV_GAME_TEXT_RVA_MAX).contains(&rva)
            || wrapper_rvas.contains(&rva)
        {
            continue;
        }
        if producer == 0 {
            producer = rva;
        }
    }
    (producer, self_in_stack)
}

#[cfg(windows)]
pub(crate) fn trace_first_game_caller_rva() -> usize {
    const GAME_TEXT_RVA_LIMIT: usize = 0x0400_0000;
    let mut frames = [std::ptr::null_mut::<c_void>(); STACK_TRACE_FRAME_COUNT];
    let captured = unsafe {
        RtlCaptureStackBackTrace(
            STACK_TRACE_FRAMES_TO_SKIP,
            frames.len() as u32,
            frames.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    } as usize;
    let module_base = unsafe { GetModuleHandleA(PCSTR::null()) }
        .ok()
        .map(|module| module.0 as usize)
        .unwrap_or(NULL_MODULE_BASE);
    if module_base == NULL_MODULE_BASE {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    frames
        .iter()
        .take(captured)
        .filter_map(|frame| {
            let address = *frame as usize;
            if address >= module_base {
                let rva = address.wrapping_sub(module_base);
                if rva < GAME_TEXT_RVA_LIMIT {
                    return Some(rva);
                }
            }
            None
        })
        .next()
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
}

#[cfg(not(windows))]
pub(crate) fn callstack_contains_game_rva(_start_rva: usize, _end_rva: usize) -> bool {
    false
}

#[cfg(not(windows))]
pub(crate) fn trace_first_game_caller_rva() -> usize {
    TITLE_OWNER_SCAN_START_ADDRESS
}

#[cfg(not(windows))]
pub(crate) fn trace_callers_summary() -> String {
    "callers=[]".to_owned()
}
