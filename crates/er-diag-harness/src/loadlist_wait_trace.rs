// STEP_LoadListWait gate trace -- which of the three conditions blocks the DLC virtual-root refill.
//
// Moved verbatim out of the product DLL's
// `crates/er-quickload/src/experiments/startup_hooks/diagnostics/loadlist_wait_trace.rs` on
// 2026-08-25. There it was installed unconditionally at process attach, on a step that runs every
// frame, for a log nothing in the product read back. The verdict logic, the short-circuit order and
// every log string are unchanged; only the sink moved and the counters are now this crate's own.
//
// The reload softlock is PROVEN to be a blanked DLC virtual root: at the stall `mapstudio_dlc2` is
// `""` while the base-game `mapstudio` is still `data2:/map/mapstudio/`, so the m28 msb read resolves
// against nothing and returns 0 bytes (bd
// `PROVEN-reload-softlock-is-blanked-dlc-virtual-root-mapstudio-dlc2-empty-2026-07-30`).
//
// Those roots are refilled from exactly one live place -- inside `CS::MoveMapListStep::STEP_LoadListWait`:
//
//     if ((loadList == NULL || *(int*)loadList - 2u < 2) && *(this+0xb8) == 0)      // gates A and B
//         if (FUN_140e6e6c0(FUN_140e6e060()) != 1) {                                // gate C
//             ...; FUN_140e05fb0(GLOBAL_CSDlc, true); ...                           // <- the refill
//         }
//
// Four outcomes have to be told apart, and only three of them are "a gate":
//   0 calls on the reload  -> the step never ran; no internal gate is responsible
//   gate A fails           -> loadList is non-null and not in state 2/3
//   gate B fails           -> this+0xb8 is non-zero
//   A and B both pass, roots still empty -> gate C (the storage-status check) is the blocker
//
// Gate C is deliberately not evaluated here. `FUN_140e6e6c0` allocates (DLAllocator/GetBackAllocator)
// and is not a pure predicate, so calling it from a trace would perturb the very run being measured.
// It is inferred instead: the counter says the step reached it, and the existing root probe says
// whether the refill happened. That inference is sound and costs nothing.
//
// This is a TRACE: it forwards unconditionally and writes only our own counters. `STEP_LoadListWait`
// runs every frame, so logging is on verdict change (plus the first few entries for a load-1
// baseline) -- the same discipline that kept the msb-parse trace readable.

use std::{
    ffi::c_void,
    sync::atomic::{AtomicUsize, Ordering},
};

use er_game_base::{
    filecap::dlio_virtual_roots_summary,
    mem::{game_module_base, game_rva_for_hook, safe_read_i32, safe_read_usize},
};
use er_hook::{MH_Initialize, MH_STATUS, MhHook};

use crate::{
    log::diag_log,
    rva::{
        HOOK_ORIGINAL_UNSET, LOADLIST_WAIT_TRACE_VERBOSE_CALLS, MOVEMAPLISTSTEP_GATE_B8_OFFSET,
        MOVEMAPLISTSTEP_LOADLIST_2C0_OFFSET, PTR_SANITY_MIN, STEP_LOADLIST_WAIT_RVA,
    },
};

/// One-shot install guard for the `STEP_LoadListWait` gate trace (deobf 0x140af1800).
static LOADLIST_WAIT_TRACE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Trampoline for the `STEP_LoadListWait` gate trace. 0 = not hooked.
static LOADLIST_WAIT_TRACE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// Total `STEP_LoadListWait` entries observed. The zero case is the POINT: the DLC virtual roots are
/// refilled only from inside this step, so if this stays flat across a profile-switch reload the
/// blocker is "the step never ran", which is not any of its three internal gates.
static LOADLIST_WAIT_TRACE_CALLS: AtomicUsize = AtomicUsize::new(0);
/// Last gate verdict seen, so the trace can log on change instead of every frame. Encoding matches
/// `loadlist_wait_verdict`: 0 = both readable gates pass, 1 = loadList state gate, 2 = the `+0xb8`
/// gate. `usize::MAX` = nothing observed yet.
static LOADLIST_WAIT_TRACE_LAST_VERDICT: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Entries where both readable gates passed, i.e. the step reached the storage-status check. If this
/// is non-zero on a reload whose roots stayed empty, the blocker is that third check -- the one the
/// trace deliberately does not evaluate itself, because it allocates and would perturb the run.
static LOADLIST_WAIT_TRACE_REACHED_STATUS_GATE: AtomicUsize = AtomicUsize::new(0);

/// Name the first failing gate. Returns `(verdict, loadList, loadListState, gate_b8)` where verdict
/// is 0 = both readable gates pass (the step reaches the storage-status check), 1 = loadList state
/// gate, 2 = the `+0xb8` gate. Mirrors the native short-circuit order exactly.
///
/// # Safety
/// `this` is the `MoveMapListStep` pointer the game passed; every dereference is fault-tolerant.
unsafe fn loadlist_wait_verdict(this: usize) -> (usize, usize, i64, usize) {
    let load_list =
        unsafe { safe_read_usize(this + MOVEMAPLISTSTEP_LOADLIST_2C0_OFFSET) }.unwrap_or(0);
    // Native reads the state only when loadList is non-null; a NULL list passes gate A.
    let state = if load_list > PTR_SANITY_MIN {
        unsafe { safe_read_i32(load_list) }
            .map(i64::from)
            .unwrap_or(-1)
    } else {
        -1
    };
    let gate_b8 = unsafe { safe_read_usize(this + MOVEMAPLISTSTEP_GATE_B8_OFFSET) }.unwrap_or(0);

    // Gate A: `loadList == NULL || (*loadList - 2) <= 1`, i.e. state 2 or 3.
    let gate_a_passes = load_list <= PTR_SANITY_MIN || state == 2 || state == 3;
    if !gate_a_passes {
        return (1, load_list, state, gate_b8);
    }
    if gate_b8 != 0 {
        return (2, load_list, state, gate_b8);
    }
    (0, load_list, state, gate_b8)
}

/// Queues the `STEP_LoadListWait` gate trace. Idempotent. The caller applies the MinHook queue once
/// for every trace in this shell.
pub(crate) fn install_loadlist_wait_trace() {
    if LOADLIST_WAIT_TRACE_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            diag_log!("loadlist-wait-trace: MH_Initialize failed: {status:?}");
            return;
        }
    }
    let Ok(addr) = game_rva_for_hook(STEP_LOADLIST_WAIT_RVA as u32) else {
        diag_log!(
            "loadlist-wait-trace: failed to resolve STEP_LoadListWait rva 0x{STEP_LOADLIST_WAIT_RVA:x}"
        );
        return;
    };
    match unsafe { MhHook::new(addr as *mut c_void, loadlist_wait_trace_hook as *mut c_void) } {
        Ok(hook) => {
            LOADLIST_WAIT_TRACE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                diag_log!("loadlist-wait-trace: queue_enable failed: {status:?}");
                return;
            }
            diag_log!(
                "loadlist-wait-trace: hooked STEP_LoadListWait at 0x{addr:x} -- the ONLY live DLC virtual-root refill site; logs which of its gates blocks, and a flat call count means the step never ran at all"
            );
        }
        Err(status) => {
            diag_log!("loadlist-wait-trace: MhHook::new failed: {status:?}");
        }
    }
}

/// Trace detour for `CS::MoveMapListStep::STEP_LoadListWait`. Forwards unconditionally.
///
/// # Safety
/// Called by the game with the step's this-pointer in `rcx`; every dereference is fault-tolerant and
/// the trampoline is invoked with the arguments unchanged.
pub(crate) unsafe extern "system" fn loadlist_wait_trace_hook(this: usize, param_2: usize) {
    let n = LOADLIST_WAIT_TRACE_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    let (verdict, load_list, state, gate_b8) = if this > PTR_SANITY_MIN {
        unsafe { loadlist_wait_verdict(this) }
    } else {
        (usize::MAX, 0, -1, 0)
    };
    if verdict == 0 {
        LOADLIST_WAIT_TRACE_REACHED_STATUS_GATE.fetch_add(1, Ordering::SeqCst);
    }

    // Log on change (plus an opening window for the load-1 baseline): this runs every frame, and a
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
        diag_log!(
            "loadlist-wait #{n}: {label} this=0x{this:x} arg2=0x{param_2:x} loadList=0x{load_list:x} state={state} b8=0x{gate_b8:x} reachedC={} roots=[{roots}]",
            LOADLIST_WAIT_TRACE_REACHED_STATUS_GATE.load(Ordering::SeqCst)
        );
    }

    let orig = LOADLIST_WAIT_TRACE_ORIG.load(Ordering::SeqCst);
    if orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, usize) = unsafe { core::mem::transmute(orig) };
        unsafe { f(this, param_2) };
    }
}
