
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

/// `CS::MenuWindowJob::~MenuWindowJob` destructor hook (deobf 0x1407ac720). Prevents BOTH observed
/// return-to-title crashes (rva 0x7ada87 and 0x7adb28) at their common root: the finalize's whole
/// `if (owningMenuWindow != 0)` block runs on a DOOMED title window during return-to-title. See
/// `MENU_WINDOW_JOB_DTOR_RVA` for the full analysis (er-effects-rs-j74t). rcx = the job; the native
/// dtor passes rdx/r8/r9 to the finalize untouched, so we forward all four verbatim.
///
/// We reproduce the exact call the finalize makes -- `owningMenuWindow->vfptr[3](window, &scratch)` --
/// and inspect the descriptor's first i32 (the event-table index). If the vtable is not in the game
/// module (freed+reused), or the index is out of range (doomed unmapped window), we null
/// `owningMenuWindow` so the finalize skips the block entirely (and correctly does NOT unref a dead
/// window). Gated to `menu_id == 0xffff` (the unmapped state every crash was in and the precondition
/// of the finalize's second getter) so healthy mapped windows are byte-identical -- no extra call.
pub(crate) unsafe extern "system" fn menu_window_job_dtor_hook(
    job: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) {
    if job != 0 {
        if let Some(base) = game_module_base().ok().filter(|&b| b != 0) {
            let owning_addr = job + MENU_WINDOW_JOB_OWNING_WINDOW_OFFSET;
            if let Some(window) = unsafe { safe_read_usize(owning_addr) } {
                if window != 0 {
                    if let Some((doomed, index)) =
                        unsafe { menu_window_doomed_event_index(window, base) }
                    {
                        if doomed {
                            // The finalize would remove the window from its push-target vector, but
                            // it crashes at the getter first, leaving the window dangling in the
                            // title-step's active-window vector STEP_MenuJobWait walks (crash rva
                            // 0x733f80). Do that removal ourselves so no stale entry survives.
                            let removed =
                                unsafe { menu_window_remove_from_push_target(job, window, base) };
                            // Null owningMenuWindow so the finalize skips its own (now-crashing)
                            // window block entirely.
                            unsafe { (owning_addr as *mut usize).write_volatile(0) };
                            let n =
                                MENU_WINDOW_JOB_DTOR_DOOMED_GUARDS.fetch_add(1, Ordering::SeqCst) + 1;
                            MENU_WINDOW_JOB_DTOR_LAST_GUARDED_WINDOW.store(window, Ordering::SeqCst);
                            MENU_WINDOW_JOB_DTOR_LAST_GUARDED_INDEX
                                .store(index.map(|i| i as usize).unwrap_or(usize::MAX), Ordering::SeqCst);
                            if n <= 32 {
                                append_crash_log(format_args!(
                                    "menu-window-job-guard: DOOMED owningMenuWindow #{n} on ~MenuWindowJob job=0x{job:x} window=0x{window:x} event_index={index:?} list_removed={removed} -- removed from push-target vector + nulled job+0x130 so the finalize skips its window block (prevents the return-to-title AV at rva 0x7ada87/0x7adb28/0x733f80)"
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    let orig = MENU_WINDOW_JOB_DTOR_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET || orig == 0 {
        return;
    }
    let f: unsafe extern "system" fn(usize, usize, usize, usize) =
        unsafe { std::mem::transmute(orig) };
    unsafe { f(job, rdx, r8, r9) };
}

/// Reproduce the finalize's `owningMenuWindow->vfptr[3](window, &scratch)` and return
/// `(doomed, event_index)`, or `None` when the window is a healthy mapped window we must not touch
/// (menu_id != 0xffff) so the caller leaves it untouched. `doomed` is true when the window is
/// freed/reused (vtable or vfptr[3] not in the game module) or the descriptor's event index is out of
/// range -- exactly the states that make the finalize dereference wild memory. Only ever calls the
/// game's own getter method (which returned successfully in every observed run; the crash was always
/// the caller's later deref), and only for unmapped (0xffff) windows.
unsafe fn menu_window_doomed_event_index(window: usize, base: usize) -> Option<(bool, Option<i32>)> {
    let in_module = |p: usize| p >= base && p.wrapping_sub(base) < GAME_MODULE_VTABLE_SPAN;
    // Read the window's vtable. A freed+reused window's vtable is heap garbage (not in the module) ->
    // doomed; the finalize's virtual call would fault. Do NOT call through a non-module vtable.
    let Some(vtable) = (unsafe { safe_read_usize(window) }) else {
        return Some((true, None));
    };
    if !in_module(vtable) {
        return Some((true, None));
    }
    // Only unmapped windows reach the crashing states; leave healthy mapped windows byte-identical.
    let menu_id = unsafe { safe_read_u16(window + MENU_WINDOW_MENU_ID_OFFSET) };
    if menu_id != Some(MENU_WINDOW_MENU_ID_UNMAPPED_SENTINEL) {
        return None;
    }
    let Some(vf3) = (unsafe { safe_read_usize(vtable + MENU_WINDOW_INPUT_DESC_VTABLE_SLOT) }) else {
        return Some((true, None));
    };
    if !in_module(vf3) {
        return Some((true, None));
    }
    // Reproduce the finalize's call: fn(window, &scratch) -> descriptor pointer. The descriptor's
    // first i32 is the event-table index the getter would use.
    let mut scratch = [0u8; MENU_WINDOW_INPUT_DESC_SCRATCH_LEN];
    let get_desc: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(vf3) };
    let descriptor = unsafe { get_desc(window, scratch.as_mut_ptr() as usize) };
    let index = unsafe { safe_read_i32(descriptor) };
    let doomed = !matches!(index, Some(i) if (0..MENU_WINDOW_EVENT_INDEX_SANE_MAX).contains(&i));
    Some((doomed, index))
}

/// Remove `window` from the job's push-target `DLFixedVector` (`*(job+0x50)`) via the game's own
/// `FUN_140733e70`, replicating the cleanup the finalize can no longer reach. Returns true iff the
/// removal ran. Validated before calling: the push-target pointer must be readable and its count
/// (`vector+0x48`) sane, because the native search loop is not SEH-guarded and a corrupt vector
/// pointer would otherwise fault. The removal itself only touches vector slots -- never the window's
/// vtable -- so it is safe on a doomed window.
unsafe fn menu_window_remove_from_push_target(job: usize, window: usize, base: usize) -> bool {
    let Some(vector) = (unsafe { safe_read_usize(job + MENU_WINDOW_JOB_PUSH_TARGET_50_OFFSET) })
    else {
        return false;
    };
    if vector == 0 {
        return false;
    }
    let count = unsafe { safe_read_i32(vector + MENU_WINDOW_LIST_COUNT_48_OFFSET) };
    if !matches!(count, Some(c) if (1..=MENU_WINDOW_LIST_SANE_MAX_COUNT).contains(&c)) {
        return false;
    }
    let Ok(remove_addr) = game_rva(MENU_WINDOW_LIST_REMOVE_RVA as u32) else {
        return false;
    };
    let _ = base;
    let remove: unsafe extern "system" fn(usize, usize) = unsafe { std::mem::transmute(remove_addr) };
    unsafe { remove(vector, window) };
    MENU_WINDOW_JOB_DTOR_LIST_REMOVALS.fetch_add(1, Ordering::SeqCst);
    true
}

/// Install the ~MenuWindowJob doomed-window guard (er-effects-rs-j74t). Idempotent.
fn install_menu_window_job_dtor_guard() {
    if MENU_WINDOW_JOB_DTOR_TRACE_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "menu-window-job-guard: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(dtor_addr) = game_rva(MENU_WINDOW_JOB_DTOR_RVA as u32) else {
        append_autoload_debug(format_args!(
            "menu-window-job-guard: failed to resolve dtor rva 0x{MENU_WINDOW_JOB_DTOR_RVA:x}"
        ));
        return;
    };
    let mut ok = true;
    match unsafe {
        MhHook::new(
            dtor_addr as *mut c_void,
            menu_window_job_dtor_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            MENU_WINDOW_JOB_DTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            ok &= unsafe { hook.queue_enable() }.is_ok();
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "menu-window-job-guard: MhHook::new(dtor) failed: {status:?}"
            ));
            ok = false;
        }
    }
    if !ok {
        return;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            MENU_WINDOW_JOB_DTOR_TRACE_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "menu-window-job-guard: hooked ~MenuWindowJob 0x{dtor_addr:x}; doomed-window guard armed (nulls a doomed owningMenuWindow so the finalize skips its block; prevents the return-to-title AV at rva 0x7ada87/0x7adb28)"
            ));
        }
        status => append_autoload_debug(format_args!(
            "menu-window-job-guard: MH_ApplyQueued failed: {status:?}"
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

/// Is the `MenuOffscrRendParam` param table absent from SoloParamRepository? True only during a quit
/// teardown (the world unload drops it); it stays resident through loads. Reproduces the game's own
/// check (`GetParamResCap(repo, MenuOffscrRendParam, 0) == NULL`) read-only.
unsafe fn menu_offscr_rend_param_table_absent(base: usize) -> bool {
    let repo = unsafe { safe_read_usize(base + SOLO_PARAM_REPOSITORY_PTR_RVA) }.unwrap_or(0);
    if repo == 0 {
        return false; // repo itself not up yet -> not the quit-teardown condition; forward.
    }
    let Ok(getcap_addr) = game_rva(GET_PARAM_RESCAP_RVA as u32) else {
        return false;
    };
    let get_rescap: unsafe extern "system" fn(usize, u32, u32) -> usize =
        unsafe { std::mem::transmute(getcap_addr) };
    let rescap = unsafe { get_rescap(repo, MENU_OFFSCR_REND_PARAM_TYPE, 0) };
    rescap == 0
}

/// `LookupMenuOffscrRendParam` (inner, deobf 0x140d3ed90; rcx = out descriptor, edx = row id). See
/// `MENU_OFFSCR_REND_PARAM_LOOKUP_RVA` for the quit-to-desktop clean-kill rationale. When the param
/// table is absent (quit teardown), `ExitProcess(0)` for a fast clean exit instead of the game's
/// imminent DLPanic; otherwise forward unchanged.
pub(crate) unsafe extern "system" fn menu_offscr_rend_param_lookup_hook(out: usize, row: u32) {
    if let Some(base) = game_module_base().ok().filter(|&b| b != 0) {
        if unsafe { menu_offscr_rend_param_table_absent(base) } {
            let n = QUIT_TO_DESKTOP_CLEAN_KILLS.fetch_add(1, Ordering::SeqCst) + 1;
            append_crash_log(format_args!(
                "quit-to-desktop: MenuOffscrRendParam table absent (quit teardown) #{n} row={row} -- native save already issued; clean ExitProcess(0) instead of the MenuOffscrRendParam DLPanic crash"
            ));
            unsafe { ExitProcess(0) };
        }
    }
    let orig = MENU_OFFSCR_REND_PARAM_LOOKUP_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET || orig == 0 {
        return;
    }
    let f: unsafe extern "system" fn(usize, u32) = unsafe { std::mem::transmute(orig) };
    unsafe { f(out, row) };
}

/// Install the quit-to-desktop clean-kill guard (er-effects-rs-j74t follow-up). Idempotent.
fn install_quit_to_desktop_clean_kill_hook() {
    if MENU_OFFSCR_REND_PARAM_LOOKUP_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "quit-to-desktop: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(lookup_addr) = game_rva(MENU_OFFSCR_REND_PARAM_LOOKUP_RVA as u32) else {
        append_autoload_debug(format_args!(
            "quit-to-desktop: failed to resolve MenuOffscrRendParam lookup rva 0x{MENU_OFFSCR_REND_PARAM_LOOKUP_RVA:x}"
        ));
        return;
    };
    let mut ok = true;
    match unsafe {
        MhHook::new(
            lookup_addr as *mut c_void,
            menu_offscr_rend_param_lookup_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            MENU_OFFSCR_REND_PARAM_LOOKUP_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            ok &= unsafe { hook.queue_enable() }.is_ok();
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "quit-to-desktop: MhHook::new(lookup) failed: {status:?}"
            ));
            ok = false;
        }
    }
    if !ok {
        return;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            MENU_OFFSCR_REND_PARAM_LOOKUP_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "quit-to-desktop: hooked MenuOffscrRendParam lookup 0x{lookup_addr:x}; on quit the world teardown's absent param table triggers a clean ExitProcess(0) (save-then-kill) instead of the DLPanic crash"
            ));
        }
        status => append_autoload_debug(format_args!(
            "quit-to-desktop: MH_ApplyQueued failed: {status:?}"
        )),
    }
}

fn install_system_quit_noop_action_hook() {
    let first_installed = SYSTEM_QUIT_NOOP_ACTION_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_NOOP_ACTION_NOT_INSTALLED;
    let second_installed = SYSTEM_QUIT_RETURN_DESKTOP_ACTION_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_RETURN_DESKTOP_ACTION_NOT_INSTALLED;
    let controller_installed = PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_INSTALLED.load(Ordering::SeqCst)
        != PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_NOT_INSTALLED;
    if first_installed && second_installed && controller_installed {
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
    if !first_installed {
        let Ok(addr) = game_rva(SYSTEM_QUIT_RETURN_TITLE_ACTION_DO_CALL_RVA) else {
            append_autoload_debug(format_args!(
                "system-quit-dup: failed to resolve Save Game/Quit action invoke rva 0x{SYSTEM_QUIT_RETURN_TITLE_ACTION_DO_CALL_RVA:x}"
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
                        "system-quit-dup: queue_enable first-row action hook failed: {status:?}"
                    ));
                    return;
                }
                match unsafe { MH_ApplyQueued() } {
                    MH_STATUS::MH_OK => {
                        std::mem::forget(hook);
                        SYSTEM_QUIT_NOOP_ACTION_INSTALLED
                            .store(SYSTEM_QUIT_NOOP_ACTION_INSTALLED_YES, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "system-quit-dup: hooked first Quit-tab action invoke 0x{addr:x}; native first row routes to Save Game"
                        ));
                    }
                    status => append_autoload_debug(format_args!(
                        "system-quit-dup: MH_ApplyQueued first-row action hook failed: {status:?}"
                    )),
                }
            }
            Err(status) => append_autoload_debug(format_args!(
                "system-quit-dup: MhHook::new first-row action hook failed: {status:?}"
            )),
        }
    }
    if !second_installed {
        let Ok(addr) = game_rva(SYSTEM_QUIT_RETURN_DESKTOP_ACTION_DO_CALL_RVA) else {
            append_autoload_debug(format_args!(
                "system-quit-dup: failed to resolve Return-to-Desktop action invoke rva 0x{SYSTEM_QUIT_RETURN_DESKTOP_ACTION_DO_CALL_RVA:x}"
            ));
            return;
        };
        match unsafe {
            MhHook::new(
                addr as *mut c_void,
                system_quit_return_desktop_action_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                SYSTEM_QUIT_RETURN_DESKTOP_ACTION_ORIG
                    .store(hook.trampoline() as usize, Ordering::SeqCst);
                if let Err(status) = unsafe { hook.queue_enable() } {
                    append_autoload_debug(format_args!(
                        "system-quit-dup: queue_enable second-row action hook failed: {status:?}"
                    ));
                    return;
                }
                match unsafe { MH_ApplyQueued() } {
                    MH_STATUS::MH_OK => {
                        std::mem::forget(hook);
                        SYSTEM_QUIT_RETURN_DESKTOP_ACTION_INSTALLED
                            .store(SYSTEM_QUIT_RETURN_DESKTOP_ACTION_INSTALLED_YES, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "system-quit-dup: hooked second Quit-tab action invoke 0x{addr:x}; cloned Load Profile/Load Save Profiles rows route before native Return-to-Desktop confirmation"
                        ));
                    }
                    status => append_autoload_debug(format_args!(
                        "system-quit-dup: MH_ApplyQueued second-row action hook failed: {status:?}"
                    )),
                }
            }
            Err(status) => append_autoload_debug(format_args!(
                "system-quit-dup: MhHook::new second-row action hook failed: {status:?}"
            )),
        }
    }
    if !controller_installed {
        let Ok(addr) = game_rva(PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_RVA) else {
            append_autoload_debug(format_args!(
                "system-quit-dup: failed to resolve PropertyNewButtonController activation rva 0x{PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_RVA:x}"
            ));
            return;
        };
        match unsafe {
            MhHook::new(
                addr as *mut c_void,
                property_new_button_controller_activate_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_ORIG
                    .store(hook.trampoline() as usize, Ordering::SeqCst);
                if let Err(status) = unsafe { hook.queue_enable() } {
                    append_autoload_debug(format_args!(
                        "system-quit-dup: queue_enable PropertyNewButtonController activation hook failed: {status:?}"
                    ));
                    return;
                }
                match unsafe { MH_ApplyQueued() } {
                    MH_STATUS::MH_OK => {
                        std::mem::forget(hook);
                        PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_INSTALLED
                            .store(PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_INSTALLED_YES, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "system-quit-dup: hooked PropertyNewButtonController activation 0x{addr:x}; custom Quit rows route by controller before native confirmation"
                        ));
                    }
                    status => append_autoload_debug(format_args!(
                        "system-quit-dup: MH_ApplyQueued PropertyNewButtonController activation hook failed: {status:?}"
                    )),
                }
            }
            Err(status) => append_autoload_debug(format_args!(
                "system-quit-dup: MhHook::new PropertyNewButtonController activation hook failed: {status:?}"
            )),
        }
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
                        "system-quit-save: hooked MsgRepository::GetAndFormat 0x{addr:x}; replacing native Quit rows GRMT/GRHK {SYSTEM_QUIT_FIRST_ROW_MENU_TEXT_ID}/{SYSTEM_QUIT_FIRST_ROW_LINEHELP_ID} and {SYSTEM_QUIT_SECOND_ROW_MENU_TEXT_ID}/{SYSTEM_QUIT_SECOND_ROW_LINEHELP_ID}; GRD:{SYSTEM_QUIT_SAVE_GAME_DIALOG_ID}"
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
                SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_FIRED.store(1, Ordering::SeqCst);
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
