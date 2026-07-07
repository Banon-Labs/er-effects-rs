
/// Read CSDelayDeleteMan's pending count (+0x40) and high-water (+0x44) via the singleton pointer
/// at DELAY_DELETE_MAN_SINGLETON_PTR_RVA. Returns `(pending, highwater)` or None if the singleton is
/// null/unresolved or the read is implausible (a wrong RVA/layout -> the count fails the sane bound).
/// This is the repeated-switch overflow oracle: pending climbing ~+10/switch means the delay-delete
/// pump is not draining the torn-down profile renderers, whose still-registered draw tasks then keep
/// filling the GX command queue.
pub(crate) unsafe fn delay_delete_pending() -> Option<(usize, usize)> {
    let base = game_rva(0).ok()?;
    let man = unsafe { safe_read_usize(base + DELAY_DELETE_MAN_SINGLETON_PTR_RVA) }?;
    if man < 0x10000 {
        return None;
    }
    let pending = unsafe { safe_read_i32(man + DELAY_DELETE_MAN_PENDING_COUNT_OFFSET) }?;
    let highwater = unsafe { safe_read_i32(man + DELAY_DELETE_MAN_PENDING_HIGHWATER_OFFSET) }?;
    if !(0..=DELAY_DELETE_MAN_PENDING_SANE_MAX as i32).contains(&pending) {
        return None;
    }
    Some((pending as usize, highwater.max(0) as usize))
}

/// OWNERSHIP LEDGER -- record that we took manual ownership of a native object (we are now
/// responsible for releasing it). Pair EVERY `ownership_take` with exactly one `ownership_release`
/// on the discharge path; a bare `store(0)`/overwrite that drops the pointer without a release is
/// the leak this ledger exists to catch.
pub(crate) fn ownership_take(class: OwnedClass) {
    let i = class as usize;
    let taken = OWNED_TAKEN[i].fetch_add(1, Ordering::SeqCst) + 1;
    let released = OWNED_RELEASED[i].load(Ordering::SeqCst);
    OWNED_MAX_OUTSTANDING[i].fetch_max(taken.saturating_sub(released), Ordering::SeqCst);
}

/// OWNERSHIP LEDGER -- record that we handed a native-owned object back to its native lifecycle
/// (e.g. delete-enqueued it). Only call on the REAL discharge path, never on an incidental pointer
/// clear, so the ledger stays an honest leak detector.
pub(crate) fn ownership_release(class: OwnedClass) {
    OWNED_RELEASED[class as usize].fetch_add(1, Ordering::SeqCst);
}

/// Current taken-but-not-released count for a class.
pub(crate) fn ownership_outstanding(class: OwnedClass) -> usize {
    let i = class as usize;
    OWNED_TAKEN[i]
        .load(Ordering::SeqCst)
        .saturating_sub(OWNED_RELEASED[i].load(Ordering::SeqCst))
}

/// OWNERSHIP LEDGER -- assert every class stays within its bound; on breach, latch the violation
/// oracle and log loudly. Called at each switch boundary (cheap enough to call per-frame). Returns
/// true iff all classes are within bound. A breach means a native-owned object was taken without a
/// paired release (the spared-renderer leak class) -- caught at the FIRST offending switch, not at a
/// downstream crash.
pub(crate) fn ownership_ledger_check(context: &str) -> bool {
    let mut ok = true;
    for i in 0..OWNED_CLASS_COUNT {
        let taken = OWNED_TAKEN[i].load(Ordering::SeqCst);
        let released = OWNED_RELEASED[i].load(Ordering::SeqCst);
        let outstanding = taken.saturating_sub(released);
        if outstanding > OWNED_CLASS_BOUND[i] {
            ok = false;
            OWNED_LEDGER_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "OWNERSHIP-LEDGER VIOLATION ({context}): class '{}' outstanding={outstanding} > bound={} (taken={taken} released={released}) -- a native-owned object was taken without a paired release (the spared-renderer leak class)",
                OWNED_CLASS_NAMES[i], OWNED_CLASS_BOUND[i]
            ));
        }
    }
    ok
}

/// Destroy a previously-spared portrait renderer via CSDelayDeleteMan -- the exact native path the
/// profile-renderer teardown (`FUN_1409b2f00`) uses for the other 9 renderers each teardown (marks
/// the object's +0x756 byte, enqueues it, freed on the delete pump when the GPU is done). Vtable-
/// guarded so a stale/freed/garbage pointer is never enqueued. MUST run on the game/menu thread (the
/// same thread the native teardown runs on -- the manager's list is mutated without locks). Returns
/// true if the object was enqueued for deletion.
pub(crate) unsafe fn delay_delete_enqueue_renderer(renderer: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if renderer == 0 || renderer == null {
        return false;
    }
    let Ok(base) = game_module_base() else {
        return false;
    };
    // Only a LIVE profile renderer (correct vtable) -- never a freed/garbage pointer.
    if unsafe { safe_read_usize(renderer) }.unwrap_or(0)
        != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
    {
        return false;
    }
    let man = unsafe { safe_read_usize(base + DELAY_DELETE_MAN_SINGLETON_PTR_RVA) }.unwrap_or(0);
    if man < 0x10000 {
        return false;
    }
    let Ok(enqueue) = game_rva(DELAY_DELETE_ENQUEUE_RVA as u32) else {
        return false;
    };
    let f: unsafe extern "system" fn(usize, usize) -> u8 = unsafe { std::mem::transmute(enqueue) };
    unsafe { f(man, renderer) };
    PROFILE_SPARE_ORPHANS_DELETED.fetch_add(1, Ordering::SeqCst);
    true
}

/// Format an `AtomicUsize` low-water value: `usize::MAX` is the never-sampled sentinel.
fn fmt_lowwater(v: usize) -> String {
    if v == usize::MAX {
        "unsampled".to_string()
    } else {
        v.to_string()
    }
}

/// Bump the GX command-queue producer histogram for `key` (lock-free open addressing; a full table
/// counts drops instead of evicting so the hot producers stay attributed).
fn gx_cmd_queue_hist_bump(key: usize) {
    if key == 0 {
        return;
    }
    let mut idx = (key >> 4) % GX_CMD_QUEUE_HIST_SLOTS;
    for _ in 0..GX_CMD_QUEUE_HIST_SLOTS {
        let cur = GX_CMD_QUEUE_HIST_KEYS[idx].load(Ordering::Relaxed);
        if cur == key {
            GX_CMD_QUEUE_HIST_COUNTS[idx].fetch_add(1, Ordering::Relaxed);
            return;
        }
        if cur == 0 {
            match GX_CMD_QUEUE_HIST_KEYS[idx].compare_exchange(
                0,
                key,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    GX_CMD_QUEUE_HIST_COUNTS[idx].fetch_add(1, Ordering::Relaxed);
                    return;
                }
                Err(actual) if actual == key => {
                    GX_CMD_QUEUE_HIST_COUNTS[idx].fetch_add(1, Ordering::Relaxed);
                    return;
                }
                Err(_) => {}
            }
        }
        idx = (idx + 1) % GX_CMD_QUEUE_HIST_SLOTS;
    }
    GX_CMD_QUEUE_HIST_DROPPED.fetch_add(1, Ordering::Relaxed);
}

/// Top-N GX producer histogram entries as `0x<rva>[+self] x<count>`, count-descending. `+self`
/// marks submissions whose call chain passed through our DLL (our pipeline caused them).
pub(crate) fn gx_cmd_queue_hist_top(n: usize) -> String {
    let mut entries: Vec<(usize, usize)> = (0..GX_CMD_QUEUE_HIST_SLOTS)
        .filter_map(|i| {
            let key = GX_CMD_QUEUE_HIST_KEYS[i].load(Ordering::Relaxed);
            let count = GX_CMD_QUEUE_HIST_COUNTS[i].load(Ordering::Relaxed);
            (key != 0 && count != 0).then_some((key, count))
        })
        .collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries
        .iter()
        .take(n)
        .map(|(key, count)| {
            let rva = key & !GX_CMD_QUEUE_SELF_TAG;
            let self_tag = if key & GX_CMD_QUEUE_SELF_TAG != 0 {
                "+self"
            } else {
                ""
            };
            format!("0x{rva:x}{self_tag} x{count}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Thin entry hook on the GX drain pump `FUN_141b3bdc0` (deobf 0x1b3bda0): latch its context
/// (param_1, the object holding the 109-bucket per-frame slot-range table) and forward. The bucket
/// table is what `gx_cmd_queue_bucket_summary` reads; the pump itself is untouched.
pub(crate) unsafe extern "system" fn gx_cmd_pump_hook(
    ctx: usize,
    param2: usize,
    param3: i32,
    param4: u32,
) {
    GX_CMD_PUMP_CTX.store(ctx, Ordering::Relaxed);
    let orig = GX_CMD_PUMP_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET || orig == 0 {
        return;
    }
    let f: unsafe extern "system" fn(usize, usize, i32, u32) = unsafe { std::mem::transmute(orig) };
    unsafe { f(ctx, param2, param3, param4) }
}

/// Nonzero per-bucket widths from the pump context's 109-bucket slot-range table as
/// `idx:width, ...` (begin at ctx+0x30+idx*0x18, end at +0x34). The bucket whose width GROWS
/// across switches is the retained-producer class behind the 0x1aeaf05 overflow. Empty string
/// until the pump context has been latched.
pub(crate) fn gx_cmd_queue_bucket_summary() -> String {
    let ctx = GX_CMD_PUMP_CTX.load(Ordering::Relaxed);
    if ctx == 0 {
        return String::new();
    }
    let mut parts = Vec::new();
    for idx in 0..GX_CMD_QUEUE_BUCKET_COUNT {
        let begin = unsafe {
            safe_read_i32(ctx + GX_CMD_QUEUE_BUCKET_BEGIN_OFFSET + idx * GX_CMD_QUEUE_BUCKET_STRIDE)
        }
        .unwrap_or(0);
        let end = unsafe {
            safe_read_i32(ctx + GX_CMD_QUEUE_BUCKET_END_OFFSET + idx * GX_CMD_QUEUE_BUCKET_STRIDE)
        }
        .unwrap_or(0);
        let width = end.saturating_sub(begin);
        // Widths above the slot capacity are torn/stale reads (this walker races the render
        // thread; run 10e's post-crash telemetry read showed multi-million "widths") -- skip them.
        if width > 0 && width <= GX_CMD_QUEUE_BUCKET_WIDTH_SANE_MAX {
            parts.push(format!("{idx}:{width}"));
        }
    }
    parts.join(", ")
}

/// Sample the command-byte arena's remaining space (arena at queue+0x40; remaining =
/// limit@+0x20 - align4(cursor_lo@+0x28), per the FUN_141c48e80 decompile) and fold it into the
/// cumulative + per-switch low-water. Returns the sampled remaining for the caller's own logging,
/// or None on unreadable fields.
unsafe fn gx_cmd_arena_sample_remaining(queue: usize) -> Option<i64> {
    let arena = queue + GX_CMD_QUEUE_ARENA_OFFSET;
    let limit = unsafe { safe_read_i32(arena + GX_CMD_ARENA_LIMIT_OFFSET) }?;
    let cursor_lo = unsafe { safe_read_i32(arena + GX_CMD_ARENA_CURSOR_OFFSET) }?;
    let aligned = (cursor_lo.wrapping_add(3)) & !3;
    let remaining = i64::from(limit) - i64::from(aligned);
    let clamped = remaining.max(0) as usize;
    GX_CMD_ARENA_MIN_REMAINING.fetch_min(clamped, Ordering::Relaxed);
    GX_CMD_ARENA_SWITCH_MIN_REMAINING.fetch_min(clamped, Ordering::Relaxed);
    Some(remaining)
}

/// Telemetry-only wrapper for `reserve_command_queue_slot` (deobf 0x141aeae60): the fixed 192-slot
/// GX command queue whose full-queue null-slot write is the repeated-switch crash at rva 0x1aeaf05
/// (reproduced at switch #4, run autostep10c-directarm-20260703-145348). Tracks occupancy
/// high-water (cumulative + per-switch), total reserves, and a producer histogram keyed by the
/// first game-.text caller outside the enqueue-wrapper band (self-tagged when our DLL is in the
/// chain), and dumps the top producers as the queue nears the edge -- so the overflow run NAMES the
/// accumulating producer. ALWAYS forwards unchanged: the 5ae3965 drop-on-overflow guard corrupted
/// the render (c2794d9) and must not return.
pub(crate) unsafe extern "system" fn gx_reserve_cmd_queue_slot_hook(
    queue: usize,
    param2: usize,
    param3: i32,
    param4: u32,
    param5: u32,
) -> usize {
    let count = unsafe { safe_read_i32(queue + GX_CMD_QUEUE_COUNT_OFFSET) }.unwrap_or(-1);
    let cap = unsafe { safe_read_i32(queue + GX_CMD_QUEUE_CAP_OFFSET) }.unwrap_or(-1);
    if count >= 0 {
        GX_CMD_QUEUE_MAX_FILL.fetch_max(count as usize, Ordering::Relaxed);
        GX_CMD_QUEUE_SWITCH_MAX_FILL.fetch_max(count as usize, Ordering::Relaxed);
    }
    if cap > 0 {
        GX_CMD_QUEUE_CAP_SEEN.store(cap as usize, Ordering::Relaxed);
    }
    GX_CMD_QUEUE_SUBMITS.fetch_add(1, Ordering::Relaxed);
    let (producer, self_in_stack) =
        stack_producer_rva(GX_CMD_QUEUE_WRAPPER_RVA_MIN..GX_CMD_QUEUE_WRAPPER_RVA_MAX);
    let key = if self_in_stack {
        producer | GX_CMD_QUEUE_SELF_TAG
    } else {
        producer
    };
    gx_cmd_queue_hist_bump(key);
    let arena_remaining = unsafe { gx_cmd_arena_sample_remaining(queue) };
    // Peak-frame bucket snapshot: the growth only materializes in teardown/reload frames (run 10e),
    // so capture the bucket composition as the per-switch high-water climbs, not just near cap.
    if count >= 0 {
        let count_us = count as usize;
        let last = GX_CMD_QUEUE_PEAK_LAST_LOGGED.load(Ordering::Relaxed);
        if count_us >= GX_CMD_QUEUE_PEAK_LOG_MIN
            && count_us >= last + GX_CMD_QUEUE_PEAK_LOG_STEP
            && GX_CMD_QUEUE_PEAK_LAST_LOGGED
                .compare_exchange(last, count_us, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        {
            append_autoload_debug(format_args!(
                "gx-cmdqueue: PEAK count={count}/{cap} arena_remaining={} buckets: {}",
                arena_remaining.unwrap_or(-1),
                gx_cmd_queue_bucket_summary()
            ));
        }
    }
    if cap > 0 && count >= 0 && count as usize >= (cap as usize) - GX_CMD_QUEUE_NEARFULL_MARGIN {
        let hits = GX_CMD_QUEUE_NEARFULL_HITS.fetch_add(1, Ordering::Relaxed);
        if hits % GX_CMD_QUEUE_NEARFULL_LOG_EVERY == 0 {
            append_autoload_debug(format_args!(
                "gx-cmdqueue: NEAR-FULL count={count}/{cap} (hit #{hits}) queue=0x{queue:x} top producers: {} | buckets: {}",
                gx_cmd_queue_hist_top(8),
                gx_cmd_queue_bucket_summary()
            ));
        }
    }
    let orig = GX_RESERVE_CMD_QUEUE_SLOT_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET || orig == 0 {
        // Fail-open is impossible here (the caller needs a real slot buffer); this branch can only
        // be reached if MinHook called the detour before the trampoline store, which queue_enable
        // ordering prevents. Keep a loud log so an impossible state is visible, not silent.
        append_autoload_debug(format_args!(
            "gx-cmdqueue: trampoline unset in detour (queue=0x{queue:x}) -- forwarding impossible"
        ));
        return 0;
    }
    let f: unsafe extern "system" fn(usize, usize, i32, u32, u32) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { f(queue, param2, param3, param4, param5) }
}

/// Install the GX command-queue producer telemetry hooks (never alter queue behavior): the
/// reserve-slot occupancy/histogram wrapper plus the thin pump-context latch for the bucket table.
fn install_gx_cmd_queue_telemetry() {
    if GX_RESERVE_CMD_QUEUE_SLOT_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "gx-cmdqueue: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let (Ok(addr), Ok(pump_addr)) = (
        game_rva(GX_RESERVE_CMD_QUEUE_SLOT_RVA as u32),
        game_rva(GX_CMD_PUMP_RVA as u32),
    ) else {
        append_autoload_debug(format_args!(
            "gx-cmdqueue: failed to resolve rvas 0x{GX_RESERVE_CMD_QUEUE_SLOT_RVA:x}/0x{GX_CMD_PUMP_RVA:x}"
        ));
        return;
    };
    let mut ok = true;
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            gx_reserve_cmd_queue_slot_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            GX_RESERVE_CMD_QUEUE_SLOT_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            ok &= unsafe { hook.queue_enable() }.is_ok();
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "gx-cmdqueue: MhHook::new(reserve) failed: {status:?}"
            ));
            ok = false;
        }
    }
    match unsafe { MhHook::new(pump_addr as *mut c_void, gx_cmd_pump_hook as *mut c_void) } {
        Ok(hook) => {
            GX_CMD_PUMP_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            ok &= unsafe { hook.queue_enable() }.is_ok();
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "gx-cmdqueue: MhHook::new(pump) failed: {status:?}"
            ));
            ok = false;
        }
    }
    if ok && matches!(unsafe { MH_ApplyQueued() }, MH_STATUS::MH_OK) {
        GX_RESERVE_CMD_QUEUE_SLOT_INSTALLED.store(1, Ordering::SeqCst);
        GX_CMD_PUMP_INSTALLED.store(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "gx-cmdqueue: producer telemetry hooked reserve_command_queue_slot 0x{addr:x} + pump 0x{pump_addr:x} (occupancy high-water + caller histogram + bucket table; forwards always)"
        ));
    } else {
        append_autoload_debug(format_args!(
            "gx-cmdqueue: queue_enable/MH_ApplyQueued failed (reserve 0x{addr:x}, pump 0x{pump_addr:x})"
        ));
    }
}

/// Install the Scaleform handler ctor/dtor lifecycle guard (repeated-switch ProfileSelect UAF).
fn install_scaleform_handler_lifecycle_guard() {
    if SCALEFORM_HANDLER_TRACE_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "scaleform-handler-guard: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let (Ok(ctor_addr), Ok(dtor_addr)) = (
        game_rva(SCALEFORM_HANDLER_CTOR_RVA as u32),
        game_rva(SCALEFORM_HANDLER_DTOR_RVA as u32),
    ) else {
        append_autoload_debug(format_args!(
            "scaleform-handler-guard: failed to resolve ctor/dtor rvas 0x{SCALEFORM_HANDLER_CTOR_RVA:x}/0x{SCALEFORM_HANDLER_DTOR_RVA:x}"
        ));
        return;
    };
    let mut ok = true;
    match unsafe {
        MhHook::new(
            ctor_addr as *mut c_void,
            scaleform_handler_ctor_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SCALEFORM_HANDLER_CTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            ok &= unsafe { hook.queue_enable() }.is_ok();
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "scaleform-handler-guard: MhHook::new(ctor) failed: {status:?}"
            ));
            ok = false;
        }
    }
    match unsafe {
        MhHook::new(
            dtor_addr as *mut c_void,
            scaleform_handler_dtor_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SCALEFORM_HANDLER_DTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            ok &= unsafe { hook.queue_enable() }.is_ok();
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "scaleform-handler-guard: MhHook::new(dtor) failed: {status:?}"
            ));
            ok = false;
        }
    }
    if !ok {
        return;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            SCALEFORM_HANDLER_TRACE_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "scaleform-handler-guard: hooked ctor 0x{ctor_addr:x} + inner dtor 0x{dtor_addr:x}; live-set double-free guard armed (skips freed-object destructs)"
            ));
        }
        status => append_autoload_debug(format_args!(
            "scaleform-handler-guard: MH_ApplyQueued failed: {status:?}"
        )),
    }
}

fn install_system_quit_menu_window_job_run_hook() {
    if SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-dup: MH_Initialize for MenuWindowJob::Run hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(MENU_WINDOW_JOB_RUN_RVA as u32) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve MenuWindowJob::Run rva 0x{MENU_WINDOW_JOB_RUN_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_menu_window_job_run_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_ORIG
                .store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-dup: queue_enable MenuWindowJob::Run hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_INSTALLED.store(
                        SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_INSTALLED_YES,
                        Ordering::SeqCst,
                    );
                    append_autoload_debug(format_args!(
                        "system-quit-dup: hooked MenuWindowJob::Run 0x{addr:x}; will map System/ProfileSelect resources to real MenuWindow pointers"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-dup: MH_ApplyQueued MenuWindowJob::Run hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-dup: MhHook::new MenuWindowJob::Run hook failed: {status:?}"
        )),
    }
}

fn install_system_quit_window_list_push_hook() {
    if SYSTEM_QUIT_WINDOW_LIST_PUSH_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_WINDOW_LIST_PUSH_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-dup: MH_Initialize for MenuWindow list push hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(MENU_WINDOW_LIST_PUSH_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve MenuWindow list push rva 0x{MENU_WINDOW_LIST_PUSH_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_menu_window_list_push_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_WINDOW_LIST_PUSH_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-dup: queue_enable MenuWindow list push hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_WINDOW_LIST_PUSH_INSTALLED
                        .store(SYSTEM_QUIT_WINDOW_LIST_PUSH_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-dup: hooked MenuWindow list push 0x{addr:x}; will record ProfileSelect append/list for Back/removal restore state"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-dup: MH_ApplyQueued MenuWindow list push hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-dup: MhHook::new MenuWindow list push hook failed: {status:?}"
        )),
    }
}

fn install_system_quit_noop_action_hook() {
    if SYSTEM_QUIT_NOOP_ACTION_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_NOOP_ACTION_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-dup: MH_Initialize for no-op action hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_RETURN_TITLE_ACTION_DO_CALL_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve Quit Game action invoke rva 0x{SYSTEM_QUIT_RETURN_TITLE_ACTION_DO_CALL_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_noop_desktop_action_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_NOOP_ACTION_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-dup: queue_enable no-op action hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_NOOP_ACTION_INSTALLED
                        .store(SYSTEM_QUIT_NOOP_ACTION_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-dup: hooked Quit Game action invoke 0x{addr:x}; cloned quick-load actions route to ProfileSelect; original row arms Save Game confirmation"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-dup: MH_ApplyQueued no-op action hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-dup: MhHook::new no-op action hook failed: {status:?}"
        )),
    }
}

fn install_system_quit_save_game_text_hook() {
    if SYSTEM_QUIT_SAVE_GAME_TEXT_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_SAVE_GAME_TEXT_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-save: MH_Initialize for text hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(MSG_REPOSITORY_GET_AND_FORMAT_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-save: failed to resolve MsgRepository::GetAndFormat rva 0x{MSG_REPOSITORY_GET_AND_FORMAT_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_save_game_get_and_format_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_SAVE_GAME_GET_AND_FORMAT_ORIG
                .store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-save: queue_enable text hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_SAVE_GAME_TEXT_INSTALLED
                        .store(SYSTEM_QUIT_SAVE_GAME_TEXT_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-save: hooked MsgRepository::GetAndFormat 0x{addr:x}; replacing GRMT:{SYSTEM_QUIT_SAVE_GAME_MENU_TEXT_ID}, GRHK:{SYSTEM_QUIT_SAVE_GAME_LINEHELP_ID}, GRD:{SYSTEM_QUIT_SAVE_GAME_DIALOG_ID}"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-save: MH_ApplyQueued text hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-save: MhHook::new text hook failed: {status:?}"
        )),
    }
}

fn install_system_quit_save_game_confirm_hook() {
    if SYSTEM_QUIT_SAVE_GAME_CONFIRM_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_SAVE_GAME_CONFIRM_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-save: MH_Initialize for confirm hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_RETURN_TITLE_REQUEST_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-save: failed to resolve return-title request rva 0x{SYSTEM_QUIT_RETURN_TITLE_REQUEST_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_save_game_return_title_request_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_SAVE_GAME_RETURN_TITLE_REQUEST_ORIG
                .store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-save: queue_enable confirm hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_SAVE_GAME_CONFIRM_INSTALLED.store(
                        SYSTEM_QUIT_SAVE_GAME_CONFIRM_INSTALLED_YES,
                        Ordering::SeqCst,
                    );
                    append_autoload_debug(format_args!(
                        "system-quit-save: hooked native return-title request 0x{addr:x}; armed System Save Game confirmations become save-only + menu close"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-save: MH_ApplyQueued confirm hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-save: MhHook::new confirm hook failed: {status:?}"
        )),
    }
}

pub(crate) unsafe extern "system" fn system_quit_profile_load_activate_hook(
    dialog: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let orig = SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: ProfileLoadDialog activation trampoline unset for dialog=0x{dialog:x} -- fail-closed return 0"
        ));
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    let base = game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let vt = unsafe { safe_read_usize(dialog) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let expected_vt = if base != TITLE_OWNER_SCAN_START_ADDRESS {
        base + PROFILE_LOAD_DIALOG_VTABLE_RVA
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let hidden = SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) != 0;
    let profile_window = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);

    // SAVE-FILE PICKER: while the live 05_010 window is our directory browser (in-game System
    // menu picker OR the startup title picker), every slot activation is a browse action (up /
    // enter dir / page / pick file) -- never a character load. Routed before ALL other logic:
    // at the title the in-game predicate below is false (nothing hidden), but the picker still
    // owns the dialog. Never forwards the native activation (which would arm a world load).
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0 && vt == expected_vt {
        let cursor =
            unsafe { safe_read_i32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }.unwrap_or(-1);
        SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_DIALOG.store(dialog, Ordering::SeqCst);
        SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_CURSOR.store(cursor as usize, Ordering::SeqCst);
        SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_COUNT.fetch_add(1, Ordering::SeqCst);
        return unsafe { save_picker_handle_activation(dialog, cursor) };
    }

    let system_quit_profile_active = hidden && profile_window != 0 && vt == expected_vt;
    if !system_quit_profile_active {
        return unsafe { original(dialog, b, c, d) };
    }

    let cursor = unsafe { safe_read_i32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }.unwrap_or(-1);
    let bound = unsafe { safe_read_i32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET) }.unwrap_or(-1);
    SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_DIALOG.store(dialog, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_CURSOR.store(cursor as usize, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_BOUND.store(bound as usize, Ordering::SeqCst);

    SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_COUNT.fetch_add(1, Ordering::SeqCst);

    // PRODUCT PATH (human-driven pick): the slot activation IS the load confirmation. A human's A on
    // a slot must load that character; the old flow instead forwarded into the native confirm ->
    // MessageBox -> OK -> load-job chain, but the product msgbox path SUPPRESSES that "load this
    // profile?" MessageBox before it renders, so a human never gets an OK to press and every A just
    // re-opens+re-suppresses the confirm -- the pick stalls, no load-job Run, no arm (observed
    // 2026-07-02: 24 activations, zero loads). Arm the save-safe switch DIRECTLY here and natively
    // cancel-close ProfileSelect, satisfying the confirm's only semantic side effect (user chose to
    // load this profile) with ZERO MessageBox and zero extra input. Repeatable: the continue_confirm
    // hook returns the phase to IDLE after each reload, so the next pick re-arms cleanly.
    //
    // The repro autopilot takes this SAME direct-arm path as a human pick. Its old scripted
    // double-A confirm chain (A pick -> confirm MessageBox -> A OK -> load-job Run -> arm) is
    // unreachable after the FIRST completed switch: that switch's arm latches PRODUCT_AUTOLOAD_ARMED,
    // whose msgbox suppression then eats the confirm box the second A needs, so every later pick
    // stalled (observed autostep10b 2026-07-03: switch #1 confirmed via the OK chain, switch #2
    // suppressed msgbox-skip #2/#3 and held 20 min). It also no longer matched the human flow this
    // autopilot exists to reproduce. Remaining gates: skip on the native-forward opt-in, when a
    // switch is already in flight (phase != IDLE), for an out-of-range cursor, or for an EMPTY slot
    // (arming an empty slot would tear down to a clean title then fail the deserialize).
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if !system_quit_profile_load_activation_allowed()
        && phase == SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE
        && (0..bound).contains(&cursor)
    {
        if !unsafe { profile_slot_has_character(cursor) } {
            append_autoload_debug(format_args!(
                "system-quit-dup: ProfileSelect slot activation IGNORED dialog=0x{dialog:x} cursor={cursor} bound={bound} -- slot holds no character; not arming a switch (would strand the game at a blank title)"
            ));
            return unsafe { original(dialog, b, c, d) };
        }
        let foreign_save_committed =
            match unsafe { system_quit_save_swap_prepare_selected_slot(cursor) } {
                Ok(committed) => committed,
                Err(()) => return 0,
            };
        unsafe { system_quit_arm_quickload_autoload(cursor, "ProfileSelectSlotActivate") };
        // The arm only takes when the preserved System dialog is present; on success it advances the
        // phase past IDLE. If it took, cancel-close ProfileSelect ourselves (no confirm-lambda runs on
        // this direct path) so the menu-pump return-title chain tears the world down + reloads the
        // picked slot at a clean title. If it did NOT take, fall through to the native activation so
        // the pick is not silently dropped.
        if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst) != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE {
            if let Ok(close_addr) = game_rva(SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA) {
                let close_fn: unsafe extern "system" fn(usize) =
                    unsafe { std::mem::transmute(close_addr) };
                unsafe { close_fn(dialog) };
                SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_COUNT.fetch_add(1, Ordering::SeqCst);
            }
            append_autoload_debug(format_args!(
                "system-quit-dup: ProfileSelect slot activation ARMED save-safe switch dialog=0x{dialog:x} cursor={cursor} bound={bound} foreign_save_committed={foreign_save_committed}; cancel-closed ProfileSelect -> return-title + clean-title fresh-deserialize of slot {cursor} (zero MessageBox)"
            ));
            return 0;
        }
        append_autoload_debug(format_args!(
            "system-quit-dup: ProfileSelect slot activation direct-arm did NOT take (no preserved System dialog) dialog=0x{dialog:x} cursor={cursor}; forwarding native activation"
        ));
    }

    append_autoload_debug(format_args!(
        "system-quit-dup: ProfileSelect slot activation dialog ALLOWED dialog=0x{dialog:x} cursor={cursor} bound={bound} profile_window=0x{profile_window:x} phase={phase}; forwarding native (load-job Run remains guarded)"
    ));
    unsafe { original(dialog, b, c, d) }
}

/// Advance the System->Quit repro autopilot to `next`, resetting the phase-local tick and the
/// waiting-log latch.
fn sq_repro_transition(next: usize) {
    SQ_REPRO_STATE.store(next, Ordering::SeqCst);
    SQ_REPRO_STATE_TICK.store(0, Ordering::SeqCst);
    SQ_REPRO_STATE_TAPS.store(0, Ordering::SeqCst);
}
