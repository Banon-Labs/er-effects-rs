use super::*;

// DLC VIRTUAL ROOT BLANK/REFILL TRACE -- why the reload's roots stay empty.
//
// The reload softlock is a blanked DLC virtual root: at the stall `mapstudio_dlc2` is `""` while the
// base-game `mapstudio` still resolves, so the m28 msb read returns 0 bytes (bd
// `PROVEN-reload-softlock-is-blanked-dlc-virtual-root-mapstudio-dlc2-empty-2026-07-30`).
//
// Two functions own that state, and this traces BOTH so one run says which ran and which did not:
//
//   BLANK   FUN_140e06490(CSDlcImp*, true) -- re-registers the 13 `*_dlc2` aliases with root L"" and
//           clears ~50 DLC ownership flags. Sole code caller: the title start-game flow FUN_1409b24e0.
//   REFILL  FUN_140e05fb0(CSDlcImp*, true) -- re-queries Steam DLC ownership and calls
//           CSDlcImp::AddVirtualFileRoots, restoring mapstudio_dlc2 -> "map_dlc2:/mapstudio".
//
// WHY THE REFILL ENTRY AND NOT ITS CALLERS: FUN_140e05fb0 has two callers --
// CS::MoveMapListStep::STEP_LoadListWait and the title-flow job body FUN_1408371e0 -- and a measured
// run has already shown STEP_LoadListWait executes ZERO times, even on a load that SUCCEEDS. So the
// live refill arrives via the title-flow job, and hooking the shared entry counts every refill
// attempt regardless of path, with no need to guess which. It also avoids FUN_1408371e0's
// rip-relative prologue (`mov rcx,[rip+...]` at +4).
//
// Both prologues are clean pushes/stores with no rip-relative operand in the patch window
// (`40 55 56 57 41 56 41 57 48 83 ec 70` and `48 89 74 24 10 57 48 83 ec 20`), so a 5-byte detour
// relocates safely.
//
// These are TRACES: they forward unconditionally and write only our own counters. Roots are sampled
// BEFORE and AFTER each call, so a single line shows the transition (`ok -> EMPTY` for the blank,
// `EMPTY -> ok` for a refill that worked, `EMPTY -> EMPTY` for one that ran but achieved nothing --
// which would move the blame to the DLC ownership re-query rather than to dispatch).

/// Installs the DLC virtual-root blank/refill traces. Idempotent.
pub(crate) fn install_dlc_roots_trace() {
    if DLC_ROOTS_TRACE_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_crash_log(format_args!(
                "dlc-roots-trace: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    install_one_dlc_roots_hook(
        DLC_ROOTS_BLANK_RVA,
        dlc_roots_blank_trace_hook as *mut c_void,
        &DLC_ROOTS_BLANK_ORIG,
        "blank",
    );
    install_one_dlc_roots_hook(
        DLC_ROOTS_REFILL_RVA,
        dlc_roots_refill_trace_hook as *mut c_void,
        &DLC_ROOTS_REFILL_ORIG,
        "refill",
    );
    install_one_dlc_roots_hook(
        DLC_ROOTS_JOB_RVA,
        dlc_roots_job_trace_hook as *mut c_void,
        &DLC_ROOTS_JOB_ORIG,
        "job",
    );
}

/// Shared install body for the two root traces -- same shape, different target.
pub(crate) fn install_one_dlc_roots_hook(
    rva: usize,
    detour: *mut c_void,
    orig: &'static AtomicUsize,
    label: &str,
) {
    let Ok(addr) = game_rva(rva as u32) else {
        append_crash_log(format_args!(
            "dlc-roots-trace: failed to resolve {label} rva 0x{rva:x}"
        ));
        return;
    };
    match unsafe { MhHook::new(addr as *mut c_void, detour) } {
        Ok(hook) => {
            orig.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_crash_log(format_args!(
                    "dlc-roots-trace: {label} queue_enable failed: {status:?}"
                ));
                return;
            }
            append_crash_log(format_args!(
                "dlc-roots-trace: hooked DLC-root {label} at 0x{addr:x}"
            ));
        }
        Err(status) => {
            append_crash_log(format_args!(
                "dlc-roots-trace: {label} MhHook::new failed: {status:?}"
            ));
        }
    }
}

/// Sample the DLC roots, run `body`, sample again, and log the transition.
pub(crate) fn trace_dlc_roots_transition(kind: &str, n: usize, arg: u8, body: impl FnOnce()) {
    let base = game_module_base().ok().filter(|&b| b != 0);
    let before = match base {
        Some(b) => unsafe { dlio_virtual_roots_summary(b) },
        None => String::from("<nobase>"),
    };
    body();
    let after = match base {
        Some(b) => unsafe { dlio_virtual_roots_summary(b) },
        None => String::from("<nobase>"),
    };
    append_crash_log(format_args!(
        "dlc-roots-{kind} #{n}: enable={arg} before=[{before}] after=[{after}]"
    ));
}

/// Trace detour for `FUN_140e06490` -- the DLC-root BLANK. Forwards unconditionally.
pub(crate) unsafe extern "system" fn dlc_roots_blank_trace_hook(csdlc: usize, enable: u8) {
    let n = DLC_ROOTS_BLANK_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    trace_dlc_roots_transition("BLANK", n, enable, || {
        let orig = DLC_ROOTS_BLANK_ORIG.load(Ordering::SeqCst);
        if orig != 0 {
            let f: unsafe extern "system" fn(usize, u8) = unsafe { core::mem::transmute(orig) };
            unsafe { f(csdlc, enable) };
        }
    });
}

/// Trace detour for `FUN_140836f30` -- the JOB BODY that ultimately reaches the refill. Forwards
/// unconditionally.
///
/// This is the fork in the road. The chain below it is
/// `FUN_140836f30 -> FUN_14082e230 -> FUN_14082eb60 -> FUN_14082dbf0 -> FUN_14082faf0` (which builds
/// the functor whose `_Do_call` is `FUN_1408371e0`, the refill wrapper). A reload that fires this but
/// never reaches `FUN_140e05fb0` means the job RUNS and diverges inside -- a fixable native path. A
/// reload that never fires it means the job was never enqueued, and its creator is a dynamically
/// built `std::function` with no static registration, so no call site can be patched.
///
/// Takes two args because the native is `(this, rdx)` with `rdx` stored at `[rsp+0x10]` on entry.
pub(crate) unsafe extern "system" fn dlc_roots_job_trace_hook(this: usize, arg2: usize) -> usize {
    let n = DLC_ROOTS_JOB_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    let roots = match game_module_base().ok().filter(|&b| b != 0) {
        Some(b) => unsafe { dlio_virtual_roots_summary(b) },
        None => String::from("<nobase>"),
    };
    append_crash_log(format_args!(
        "dlc-roots-JOB #{n}: this=0x{this:x} arg2=0x{arg2:x} refills_so_far={} roots=[{roots}]",
        DLC_ROOTS_REFILL_CALLS.load(Ordering::SeqCst)
    ));
    let orig = DLC_ROOTS_JOB_ORIG.load(Ordering::SeqCst);
    if orig != 0 {
        let f: unsafe extern "system" fn(usize, usize) -> usize =
            unsafe { core::mem::transmute(orig) };
        return unsafe { f(this, arg2) };
    }
    0
}

/// Trace detour for `FUN_140e05fb0` -- the DLC-root REFILL. Forwards unconditionally.
pub(crate) unsafe extern "system" fn dlc_roots_refill_trace_hook(csdlc: usize, enable: u8) {
    let n = DLC_ROOTS_REFILL_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    trace_dlc_roots_transition("REFILL", n, enable, || {
        let orig = DLC_ROOTS_REFILL_ORIG.load(Ordering::SeqCst);
        if orig != 0 {
            let f: unsafe extern "system" fn(usize, u8) = unsafe { core::mem::transmute(orig) };
            unsafe { f(csdlc, enable) };
        }
    });
}
