pub(crate) unsafe fn title_owner(module_base: usize) -> Option<*mut u8> {
    let cached = TITLE_OWNER_PTR.load(Ordering::SeqCst) as *mut u8;
    if !cached.is_null() {
        return Some(cached);
    }
    let found = title_step_ptr_or_null();
    if found == NULL_MODULE_BASE {
        return None;
    }
    let owner = found as *mut u8;
    let vtable = unsafe { *(found as *const usize) };
    let instance_table = unsafe { safe_read_usize(found + TITLE_OWNER_INSTANCE_TABLE_OFFSET) };
    let state_value = unsafe { safe_read_i32(found + TITLE_OWNER_STATE_OFFSET) };
    let vtable_ok = vtable == module_base + title_owner_vtable_rva();
    let table_ok = instance_table == Some(module_base + title_step_state_table_rva());
    let state_ok = state_value.is_some_and(|s| (TITLE_OWNER_MIN_STATE..=TITLE_OWNER_MAX_STATE).contains(&s));
    if !(vtable_ok && table_ok && state_ok) {
        append_autoload_debug(format_args!(
            "native_title_job: title owner path rejected owner=0x{found:x} vt=0x{vtable:x} table={:?} state={:?} expected_vt=0x{:x} expected_table=0x{:x}",
            instance_table,
            state_value,
            module_base + title_owner_vtable_rva(),
            module_base + title_step_state_table_rva()
        ));
        return None;
    }
    TITLE_OWNER_PTR.store(found, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "native_title_job: captured title owner={owner:p} state={}",
        state_value.unwrap_or_default()
    ));
    Some(owner)
}
pub(crate) unsafe fn call_native_title_job_once(module_base: usize, tick: u64) -> bool {
    if TITLE_NATIVE_JOB_CALLED.load(Ordering::SeqCst) != TITLE_NATIVE_JOB_NOT_CALLED {
        return true;
    }
    if tick < TITLE_NATIVE_JOB_MIN_TICK {
        let count = TITLE_OWNER_TRACE_COUNT
            .fetch_add(TITLE_TRACE_SEQUENCE_INCREMENT, Ordering::SeqCst)
            + TITLE_TRACE_SEQUENCE_INCREMENT;
        if count <= TITLE_OWNER_TRACE_LIMIT {
            append_autoload_debug(format_args!(
                "native_title_job: waiting for min tick tick={tick} target={TITLE_NATIVE_JOB_MIN_TICK}"
            ));
        }
        return false;
    }
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        let count = TITLE_OWNER_TRACE_COUNT
            .fetch_add(TITLE_TRACE_SEQUENCE_INCREMENT, Ordering::SeqCst)
            + TITLE_TRACE_SEQUENCE_INCREMENT;
        if count <= TITLE_OWNER_TRACE_LIMIT {
            append_autoload_debug(format_args!(
                "native_title_job: waiting for title owner at tick={tick}"
            ));
        }
        return false;
    };

    let state_before = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    let mut task_data = [TITLE_NATIVE_JOB_TASK_DATA_ZERO; TITLE_NATIVE_JOB_TASK_DATA_BYTES];
    let frame_delta = TITLE_NATIVE_JOB_FRAME_DELTA_NUMERATOR / TITLE_NATIVE_JOB_FRAME_RATE;
    task_data[TITLE_NATIVE_JOB_DELTA_OFFSET_START..TITLE_NATIVE_JOB_DELTA_OFFSET_END]
        .copy_from_slice(&frame_delta.to_le_bytes());
    let title_menu_job: unsafe extern "system" fn(*mut u8, *mut c_void) =
        unsafe { std::mem::transmute(module_base + TITLE_MENU_JOB_WAIT_RVA) };
    append_autoload_debug(format_args!(
        "native_title_job: ENTER owner={owner:p} state_before={state_before} tick={tick}"
    ));
    unsafe { title_menu_job(owner, task_data.as_mut_ptr().cast()) };
    let state_after = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    TITLE_NATIVE_JOB_CALLED.store(TITLE_NATIVE_JOB_CALLED_VALUE, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "native_title_job: LEAVE owner={owner:p} state_after={state_after} tick={tick}"
    ));
    true
}
