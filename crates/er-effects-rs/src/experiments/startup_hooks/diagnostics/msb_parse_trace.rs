use super::*;

// MSB PARSE TRACE -- the one measurement that collapses the phase-2 reload freeze.
//
// `MsbFileCap::msbResCap` (+0x90) has EXACTLY ONE writer on 1.16.2: the load-complete callback at
// RVA 0x21bbf0, which does
//     content = FD4FileCap::AcquireContent(cap);
//     if (content != 0 && header_ok) { msbResCap = MsbRepository::GetOrCreate(name, content, size); }
//     FD4FileCap::ReleaseContent(cap);
// and returns NORMALLY when `content` is null -- nothing errors, nothing retries, and `loadState` is
// already 4. WorldBlockRes case 2 then waits on `msbResCap != 0` forever with no timeout.
//
// The cap-identity capture (bd `cap-identity-capture-rc1-lp0-name-resolved-2026-07-30`) narrowed the
// cause to two possibilities that NO passive read can separate, because both leave identical state
// behind (`st=4, bytes=0x0, lp=0x0, ct=0x0`):
//   A. the callback FIRES for the m28 msb with a null content -> the READ came back empty
//   B. the callback NEVER FIRES for it -> `AddFileCap` cache-hit, no `PushFileCap`, no load process
// Only watching the writer itself tells them apart, so watch the writer itself.
//
// This is a TRACE, not a guard: it forwards to the trampoline unconditionally and changes no game
// state. It reads `msbResCap` before and after the real call, so the log line states outright
// whether that invocation produced a resource. Rate-limited, because on a cold boot this fires for
// every msb in the world.

/// Installs the msb-parse trace. Idempotent. 0x21bbf0 carries no other detour (MinHook allows one
/// per address).
pub(crate) fn install_msb_parse_trace() {
    if MSB_PARSE_TRACE_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_crash_log(format_args!(
                "msb-parse-trace: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(MSB_FILECAP_PARSE_CALLBACK_RVA as u32) else {
        append_crash_log(format_args!(
            "msb-parse-trace: failed to resolve parse-callback rva 0x{MSB_FILECAP_PARSE_CALLBACK_RVA:x}"
        ));
        return;
    };
    match unsafe { MhHook::new(addr as *mut c_void, msb_parse_trace_hook as *mut c_void) } {
        Ok(hook) => {
            MSB_PARSE_TRACE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_crash_log(format_args!(
                    "msb-parse-trace: queue_enable failed: {status:?}"
                ));
                return;
            }
            append_crash_log(format_args!(
                "msb-parse-trace: hooked the SOLE msbResCap writer at 0x{addr:x} -- logs name/content/result per parse so a null-content short-circuit is visible"
            ));
        }
        Err(status) => {
            append_crash_log(format_args!(
                "msb-parse-trace: MhHook::new failed: {status:?}"
            ));
        }
    }
}

/// Trace detour for the msb load-complete callback. Forwards unconditionally; the only writes are to
/// our own counters.
pub(crate) unsafe extern "system" fn msb_parse_trace_hook(cap: usize) {
    let before = if cap > 0x10000 {
        unsafe { safe_read_usize(cap + FD4_FILECAP_BYTES_90_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    // Sampled BEFORE the call: the callback's own `ReleaseContent` can drop the buffer and null the
    // load process on the way out, so reading these afterwards would describe the cleanup rather
    // than the inputs the parse actually saw.
    let (load_process, load_state) = if cap > 0x10000 {
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
    if orig != 0 {
        let f: unsafe extern "system" fn(usize) = unsafe { core::mem::transmute(orig) };
        unsafe { f(cap) };
    }

    let after = if cap > 0x10000 {
        unsafe { safe_read_usize(cap + FD4_FILECAP_BYTES_90_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    let n = MSB_PARSE_TRACE_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    if after == 0 {
        MSB_PARSE_TRACE_NULL_RESULTS.fetch_add(1, Ordering::SeqCst);
    }

    // Log every call that produced NOTHING (the interesting case -- these are the caps that will
    // wedge case 2), but rate-limit the successful ones, which number in the hundreds on a cold
    // boot and would otherwise bury the failures.
    let interesting = after == 0;
    if interesting || n <= MSB_PARSE_TRACE_VERBOSE_CALLS {
        let name = if cap > 0x10000 {
            unsafe { fd4_filecap_name(cap) }
        } else {
            String::from("<badcap>")
        };
        // WITHIN-RUN CONTROL for the DLC-virtual-root theory. These caps are named
        // `mapstudio_dlc2:/m28_*.msb`, and `mapstudio_dlc2` is a DLIO virtual-root alias that the
        // title start-game flow registers EMPTY and only `STEP_LoadListWait` fills in. Dumping the
        // alias HERE -- where load 1 demonstrably succeeds -- is what makes a later EMPTY reading
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
        append_crash_log(format_args!(
            "msb-parse{} #{n}: cap=0x{cap:x} name='{name}' loadState={load_state} loadProcess=0x{load_process:x} content=0x{content:x} size=0x{csize:x} msbResCap 0x{before:x}->0x{after:x} roots=[{roots}]",
            if interesting { "-NULL-RESULT" } else { "" }
        ));
    }
}
