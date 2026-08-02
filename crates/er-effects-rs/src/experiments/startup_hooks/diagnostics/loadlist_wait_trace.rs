use super::*;

// STEP_LoadListWait GATE TRACE -- which of the three conditions blocks the DLC virtual-root refill.
//
// The reload softlock is PROVEN to be a blanked DLC virtual root: at the stall `mapstudio_dlc2` is
// `""` while the base-game `mapstudio` is still `data2:/map/mapstudio/`, so the m28 msb read resolves
// against nothing and returns 0 bytes (bd
// `PROVEN-reload-softlock-is-blanked-dlc-virtual-root-mapstudio-dlc2-empty-2026-07-30`).
//
// Those roots are refilled from exactly ONE live place -- inside `CS::MoveMapListStep::STEP_LoadListWait`:
//
//     if ((loadList == NULL || *(int*)loadList - 2u < 2) && *(this+0xb8) == 0)      // gates A and B
//         if (FUN_140e6e6c0(FUN_140e6e060()) != 1) {                                // gate C
//             ...; FUN_140e05fb0(GLOBAL_CSDlc, true); ...                           // <- the refill
//         }
//
// FOUR outcomes have to be told apart, and only three of them are "a gate":
//   0 calls on the reload  -> the STEP NEVER RAN; no internal gate is responsible
//   gate A fails           -> loadList is non-null and not in state 2/3
//   gate B fails           -> this+0xb8 is non-zero
//   A and B both pass, roots STILL empty -> gate C (the storage-status check) is the blocker
//
// Gate C is deliberately NOT evaluated here. `FUN_140e6e6c0` allocates (DLAllocator/GetBackAllocator)
// and is not a pure predicate, so calling it from a trace would perturb the very run being measured.
// It is inferred instead: the counter says the step reached it, and the existing root probe says
// whether the refill happened. That inference is sound and costs nothing.
//
// This is a TRACE: it forwards unconditionally and writes only our own counters. `STEP_LoadListWait`
// runs every frame, so logging is on VERDICT CHANGE (plus the first few entries for a load-1
// baseline) -- the same discipline that kept the msb-parse trace readable.

/// Name the first failing gate. Returns `(verdict, loadList, loadListState, gate_b8)` where verdict
/// is 0 = both readable gates pass (the step reaches the storage-status check), 1 = loadList state
/// gate, 2 = the `+0xb8` gate. Mirrors the native short-circuit order exactly.
pub(crate) unsafe fn loadlist_wait_verdict(this: usize) -> (usize, usize, i64, usize) {
    let load_list =
        unsafe { safe_read_usize(this + MOVEMAPLISTSTEP_LOADLIST_2C0_OFFSET) }.unwrap_or(0);
    // Native reads the state only when loadList is non-null; a NULL list PASSES gate A.
    let state = if load_list > 0x10000 {
        unsafe { safe_read_i32(load_list) }
            .map(i64::from)
            .unwrap_or(-1)
    } else {
        -1
    };
    let gate_b8 = unsafe { safe_read_usize(this + MOVEMAPLISTSTEP_GATE_B8_OFFSET) }.unwrap_or(0);

    // Gate A: `loadList == NULL || (*loadList - 2) <= 1`, i.e. state 2 or 3.
    let gate_a_passes = load_list <= 0x10000 || state == 2 || state == 3;
    if !gate_a_passes {
        return (1, load_list, state, gate_b8);
    }
    if gate_b8 != 0 {
        return (2, load_list, state, gate_b8);
    }
    (0, load_list, state, gate_b8)
}

/// Installs the `STEP_LoadListWait` gate trace. Idempotent.
pub(crate) fn install_loadlist_wait_trace() {
    if LOADLIST_WAIT_TRACE_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_crash_log(format_args!(
                "loadlist-wait-trace: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(STEP_LOADLIST_WAIT_RVA as u32) else {
        append_crash_log(format_args!(
            "loadlist-wait-trace: failed to resolve STEP_LoadListWait rva 0x{STEP_LOADLIST_WAIT_RVA:x}"
        ));
        return;
    };
    match unsafe { MhHook::new(addr as *mut c_void, loadlist_wait_trace_hook as *mut c_void) } {
        Ok(hook) => {
            LOADLIST_WAIT_TRACE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_crash_log(format_args!(
                    "loadlist-wait-trace: queue_enable failed: {status:?}"
                ));
                return;
            }
            append_crash_log(format_args!(
                "loadlist-wait-trace: hooked STEP_LoadListWait at 0x{addr:x} -- the ONLY live DLC virtual-root refill site; logs which of its gates blocks, and a flat call count means the step never ran at all"
            ));
        }
        Err(status) => {
            append_crash_log(format_args!(
                "loadlist-wait-trace: MhHook::new failed: {status:?}"
            ));
        }
    }
}

/// Trace detour for `CS::MoveMapListStep::STEP_LoadListWait`. Forwards unconditionally.
pub(crate) unsafe extern "system" fn loadlist_wait_trace_hook(this: usize, param_2: usize) {
    let n = LOADLIST_WAIT_TRACE_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    let (verdict, load_list, state, gate_b8) = if this > 0x10000 {
        unsafe { loadlist_wait_verdict(this) }
    } else {
        (usize::MAX, 0, -1, 0)
    };
    if verdict == 0 {
        LOADLIST_WAIT_TRACE_REACHED_STATUS_GATE.fetch_add(1, Ordering::SeqCst);
    }

    // Log on CHANGE (plus an opening window for the load-1 baseline): this runs every frame, and a
    // per-frame line would drown the reload in thousands of identical entries.
    let previous = LOADLIST_WAIT_TRACE_LAST_VERDICT.swap(verdict, Ordering::SeqCst);
    if previous != verdict || n <= LOADLIST_WAIT_TRACE_VERBOSE_CALLS {
        let label = match verdict {
            0 => "PASSES-A+B(reaches storage-status gate C)",
            1 => "BLOCKED-gate-A(loadList state not 2/3)",
            2 => "BLOCKED-gate-B(+0xb8 non-zero)",
            _ => "BAD-THIS",
        };
        // The roots ride along so a single line answers both halves at once: which gate held, and
        // whether the DLC aliases were populated at that instant.
        let roots = match game_module_base().ok().filter(|&b| b != 0) {
            Some(b) => unsafe { dlio_virtual_roots_summary(b) },
            None => String::from("<nobase>"),
        };
        append_crash_log(format_args!(
            "loadlist-wait #{n}: {label} this=0x{this:x} arg2=0x{param_2:x} loadList=0x{load_list:x} state={state} b8=0x{gate_b8:x} reachedC={} roots=[{roots}]",
            LOADLIST_WAIT_TRACE_REACHED_STATUS_GATE.load(Ordering::SeqCst)
        ));
    }

    let orig = LOADLIST_WAIT_TRACE_ORIG.load(Ordering::SeqCst);
    if orig != 0 {
        let f: unsafe extern "system" fn(usize, usize) = unsafe { core::mem::transmute(orig) };
        unsafe { f(this, param_2) };
    }
}
