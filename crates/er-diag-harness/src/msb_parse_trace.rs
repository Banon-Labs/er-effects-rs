// MSB PARSE trace -- the one measurement that collapses the phase-2 reload freeze.
//
// Moved verbatim out of the product DLL's
// `crates/er-quickload/src/experiments/startup_hooks/diagnostics/msb_parse_trace.rs` on
// 2026-08-25. There it was installed unconditionally at process attach, so every player carried a
// detour on this callback for the sake of a log nothing in the product read back. The body, the
// sampling order and every log string are unchanged; only the sink moved (`er-diag-harness.log`
// instead of the product's crash log) and the counters are now this crate's own.
//
// `MsbFileCap::msbResCap` (+0x90) has exactly one writer on 1.16.2: the load-complete callback at
// RVA 0x21bbf0, which does
//     content = FD4FileCap::AcquireContent(cap);
//     if (content != 0 && header_ok) { msbResCap = MsbRepository::GetOrCreate(name, content, size); }
//     FD4FileCap::ReleaseContent(cap);
// and returns normally when `content` is null -- nothing errors, nothing retries, and `loadState` is
// already 4. WorldBlockRes case 2 then waits on `msbResCap != 0` forever with no timeout.
//
// The cap-identity capture (bd `cap-identity-capture-rc1-lp0-name-resolved-2026-07-30`) narrowed the
// cause to two possibilities that no passive read can separate, because both leave identical state
// behind (`st=4, bytes=0x0, lp=0x0, ct=0x0`):
//   A. the callback fires for the m28 msb with a null content -> the read came back empty
//   B. the callback never fires for it -> `AddFileCap` cache-hit, no `PushFileCap`, no load process
// Only watching the writer itself tells them apart, so watch the writer itself.
//
// This is a trace, not a guard: it forwards to the trampoline unconditionally and changes no game
// state. It reads `msbResCap` before and after the real call, so the log line states outright
// whether that invocation produced a resource. Rate-limited, because on a cold boot this fires for
// every msb in the world.

use std::{
    ffi::c_void,
    sync::atomic::{AtomicUsize, Ordering},
};

use er_game_base::{
    filecap::{
        FD4_FILECAP_BYTES_90_OFFSET, FD4_FILECAP_LOADPROCESS_78_OFFSET,
        FD4_FILECAP_STATUS_88_OFFSET, dlio_virtual_roots_summary, fd4_filecap_content_state,
        fd4_filecap_name,
    },
    mem::{game_module_base, game_rva_for_hook, safe_read_u8, safe_read_usize},
};
use er_hook::{MH_Initialize, MH_STATUS, MhHook};

use crate::{
    log::diag_log,
    rva::{
        HOOK_ORIGINAL_UNSET, MSB_FILECAP_PARSE_CALLBACK_RVA, MSB_PARSE_TRACE_ROOTS_ON_NULL_RESULTS,
        MSB_PARSE_TRACE_VERBOSE_CALLS, PTR_SANITY_MIN,
    },
};

/// One-shot install guard for the msb-parse trace (the sole `msbResCap` writer, deobf 0x14021bbf0).
static MSB_PARSE_TRACE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Trampoline for the msb-parse trace. 0 = not hooked.
static MSB_PARSE_TRACE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// Total msb load-complete callbacks observed. Read from the `msb-parse #N` log lines.
static MSB_PARSE_TRACE_CALLS: AtomicUsize = AtomicUsize::new(0);
/// Callbacks that returned with `msbResCap` still null -- i.e. the content was null and the parse
/// silently short-circuited. Every one of these is a cap that will wedge `WorldBlockRes` case 2 if a
/// block ever waits on it, so a non-zero value here is the freeze precursor.
static MSB_PARSE_TRACE_NULL_RESULTS: AtomicUsize = AtomicUsize::new(0);

/// Queues the msb-parse trace detour. Idempotent. The caller applies the MinHook queue once for
/// every trace in this shell.
///
/// 0x21bbf0 carries no other detour: the product never hooked it for any purpose but this trace,
/// and MinHook allows exactly one per address.
pub(crate) fn install_msb_parse_trace() {
    if MSB_PARSE_TRACE_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            diag_log!("msb-parse-trace: MH_Initialize failed: {status:?}");
            return;
        }
    }
    let Ok(addr) = game_rva_for_hook(MSB_FILECAP_PARSE_CALLBACK_RVA as u32) else {
        diag_log!(
            "msb-parse-trace: failed to resolve parse-callback rva 0x{MSB_FILECAP_PARSE_CALLBACK_RVA:x}"
        );
        return;
    };
    match unsafe { MhHook::new(addr as *mut c_void, msb_parse_trace_hook as *mut c_void) } {
        Ok(hook) => {
            MSB_PARSE_TRACE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                diag_log!("msb-parse-trace: queue_enable failed: {status:?}");
                return;
            }
            diag_log!(
                "msb-parse-trace: hooked the SOLE msbResCap writer at 0x{addr:x} -- logs name/content/result per parse so a null-content short-circuit is visible"
            );
        }
        Err(status) => {
            diag_log!("msb-parse-trace: MhHook::new failed: {status:?}");
        }
    }
}

/// Trace detour for the msb load-complete callback. Forwards unconditionally; the only writes are to
/// our own counters.
///
/// # Safety
/// Called by the game with the `FD4FileCap*` in `rcx`. Every dereference is fault-tolerant, and the
/// trampoline is invoked with the arguments unchanged.
pub(crate) unsafe extern "system" fn msb_parse_trace_hook(cap: usize) {
    let before = if cap > PTR_SANITY_MIN {
        unsafe { safe_read_usize(cap + FD4_FILECAP_BYTES_90_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    // Sampled before the call: the callback's own `ReleaseContent` can drop the buffer and null the
    // load process on the way out, so reading these afterwards would describe the cleanup rather
    // than the inputs the parse actually saw.
    let (load_process, load_state) = if cap > PTR_SANITY_MIN {
        (
            unsafe { safe_read_usize(cap + FD4_FILECAP_LOADPROCESS_78_OFFSET) }.unwrap_or(0),
            unsafe { safe_read_u8(cap + FD4_FILECAP_STATUS_88_OFFSET) }
                .map(|v| v as i32)
                .unwrap_or(-1),
        )
    } else {
        (0, -1)
    };
    let (_, content, csize, _) = unsafe { fd4_filecap_content_state(load_process) };

    let orig = MSB_PARSE_TRACE_ORIG.load(Ordering::SeqCst);
    if orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize) = unsafe { core::mem::transmute(orig) };
        unsafe { f(cap) };
    }

    let after = if cap > PTR_SANITY_MIN {
        unsafe { safe_read_usize(cap + FD4_FILECAP_BYTES_90_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    let n = MSB_PARSE_TRACE_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    if after == 0 {
        MSB_PARSE_TRACE_NULL_RESULTS.fetch_add(1, Ordering::SeqCst);
    }

    // Log every call that produced nothing (the interesting case -- these are the caps that will
    // wedge case 2), but rate-limit the successful ones, which number in the hundreds on a cold
    // boot and would otherwise bury the failures.
    let interesting = after == 0;
    if interesting || n <= MSB_PARSE_TRACE_VERBOSE_CALLS {
        let name = if cap > PTR_SANITY_MIN {
            unsafe { fd4_filecap_name(cap) }
        } else {
            String::from("<badcap>")
        };
        // Within-run control for the DLC-virtual-root theory. These caps are named
        // `mapstudio_dlc2:/m28_*.msb`, and `mapstudio_dlc2` is a DLIO virtual-root alias that the
        // title start-game flow registers empty and only `STEP_LoadListWait` fills in. Dumping the
        // alias here -- where load 1 demonstrably succeeds -- is what makes a later empty reading
        // mean something: without a known-good baseline, "everything reads empty" is
        // indistinguishable from a broken vector walk. Bounded to the first few calls of each
        // outcome because the null path fires ~13x/second during the stall and the walk is not free.
        let roots = if n <= MSB_PARSE_TRACE_VERBOSE_CALLS
            || MSB_PARSE_TRACE_NULL_RESULTS.load(Ordering::SeqCst)
                <= MSB_PARSE_TRACE_ROOTS_ON_NULL_RESULTS
        {
            match game_module_base().ok().filter(|&b| b != 0) {
                Some(b) => unsafe { dlio_virtual_roots_summary(b) },
                None => String::from("<nobase>"),
            }
        } else {
            String::from("<skipped>")
        };
        diag_log!(
            "msb-parse{} #{n}: cap=0x{cap:x} name='{name}' loadState={load_state} loadProcess=0x{load_process:x} content=0x{content:x} size=0x{csize:x} msbResCap 0x{before:x}->0x{after:x} roots=[{roots}]",
            if interesting { "-NULL-RESULT" } else { "" }
        );
    }
}
