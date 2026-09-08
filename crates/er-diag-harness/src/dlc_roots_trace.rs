// DLC virtual root BLANK/REFILL trace -- why the reload's roots stay empty.
//
// Moved verbatim out of the product DLL's
// `crates/er-quickload/src/experiments/startup_hooks/diagnostics/dlc_roots_trace.rs` on
// 2026-08-25. There all three detours were installed unconditionally at process attach for a log
// nothing in the product read back. The bodies, the sampling order and every log string are
// unchanged; only the sink moved and the counters are now this crate's own.
//
// One soft coupling survives the move, and it is deliberate. `er-title-flow`'s DLC-root self-heal
// (`crates/er-title-flow/src/dlc_roots_self_heal.rs`) prefers `DLC_ROOTS_REFILL_ORIG` -- the
// trampoline this trace used to store -- over resolving the refill's RVA, so that the heal does not
// re-enter our own detour. Rust statics are per-DLL, so with the trace in a second image the
// product's copy stays 0 and the heal takes its existing fallback: `game_rva(DLC_ROOTS_REFILL_RVA)`.
// In a product-only profile that is the un-detoured native and the behaviour is identical. In a
// product + harness profile the heal enters this detour, which samples the roots and forwards to
// its own trampoline -- one extra log line, no recursion, and the trace now also sees the heal's
// refill, which is strictly more informative than the old arrangement.
//
// The reload softlock is a blanked DLC virtual root: at the stall `mapstudio_dlc2` is `""` while the
// base-game `mapstudio` still resolves, so the m28 msb read returns 0 bytes (bd
// `PROVEN-reload-softlock-is-blanked-dlc-virtual-root-mapstudio-dlc2-empty-2026-07-30`).
//
// Two functions own that state, and this traces both so one run says which ran and which did not:
//
//   Blank   FUN_140e06490(CSDlcImp*, true) -- re-registers the 13 `*_dlc2` aliases with root L"" and
//           clears ~50 DLC ownership flags. Sole code caller: the title start-game flow FUN_1409b24e0.
//   Refill  FUN_140e05fb0(CSDlcImp*, true) -- re-queries Steam DLC ownership and calls
//           CSDlcImp::AddVirtualFileRoots, restoring mapstudio_dlc2 -> "map_dlc2:/mapstudio".
//
// Why the refill entry and not its CALLERS: FUN_140e05fb0 has two callers --
// CS::MoveMapListStep::STEP_LoadListWait and the title-flow job body FUN_1408371e0 -- and a measured
// run has already shown STEP_LoadListWait executes zero times, even on a load that succeeds. So the
// live refill arrives via the title-flow job, and hooking the shared entry counts every refill
// attempt regardless of path, with no need to guess which. It also avoids FUN_1408371e0's
// rip-relative prologue (`mov rcx,[rip+...]` at +4).
//
// Both prologues are clean pushes/stores with no rip-relative operand in the patch window
// (`40 55 56 57 41 56 41 57 48 83 ec 70` and `48 89 74 24 10 57 48 83 ec 20`), so a 5-byte detour
// relocates safely.
//
// These are TRACES: they forward unconditionally and write only our own counters. Roots are sampled
// before and after each call, so a single line shows the transition (`ok -> EMPTY` for the blank,
// `EMPTY -> ok` for a refill that worked, `EMPTY -> EMPTY` for one that ran but achieved nothing --
// which would move the blame to the DLC ownership re-query rather than to dispatch).

use std::{
    ffi::c_void,
    sync::atomic::{AtomicUsize, Ordering},
};

use er_game_base::{
    filecap::dlio_virtual_roots_summary,
    mem::{game_module_base, game_rva_for_hook},
};
use er_hook::{MH_Initialize, MH_STATUS, MhHook};

use crate::{
    log::diag_log,
    rva::{DLC_ROOTS_BLANK_RVA, DLC_ROOTS_JOB_RVA, DLC_ROOTS_REFILL_RVA, HOOK_ORIGINAL_UNSET},
};

/// One-shot install guard for the DLC virtual-root blank/refill traces.
static DLC_ROOTS_TRACE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Trampoline for the DLC-root blank (`FUN_140e06490`). 0 = not hooked.
static DLC_ROOTS_BLANK_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// Trampoline for the DLC-root refill (`FUN_140e05fb0`). 0 = not hooked.
static DLC_ROOTS_REFILL_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// Trampoline for the DLC-root refill job body (`FUN_140836f30`). 0 = not hooked.
static DLC_ROOTS_JOB_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// Times the DLC virtual roots were blanked to `L""`. Read from the `dlc-roots-BLANK` log lines.
static DLC_ROOTS_BLANK_CALLS: AtomicUsize = AtomicUsize::new(0);
/// Times the DLC virtual-root refill ran. If this trails the blank count across a reload, the roots
/// were emptied and never restored -- which is the softlock.
static DLC_ROOTS_REFILL_CALLS: AtomicUsize = AtomicUsize::new(0);
/// Times the refill job body ran. This is the FORK: the job body sits one level above the refill
/// (body -> FUN_14082e230 -> FUN_14082eb60 -> FUN_14082dbf0 -> FUN_14082faf0 -> ... -> the refill).
/// If this fires on a reload whose roots stay empty, the job runs and diverges inside, so a native
/// fix exists. If it stays flat, the job was never enqueued -- and its creator is a dynamically
/// built `std::function` with no static registration, so there is no call site to patch.
static DLC_ROOTS_JOB_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Queues the DLC virtual-root blank/refill traces. Idempotent. The caller applies the MinHook queue
/// once for every trace in this shell.
pub(crate) fn install_dlc_roots_trace() {
    if DLC_ROOTS_TRACE_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            diag_log!("dlc-roots-trace: MH_Initialize failed: {status:?}");
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
    install_shared_dlc_roots_job_hook();
}

/// Register the job-body trace (`0x836f30`) on the process's single MinHook instance.
///
/// Why this one is not `install_one_dlc_roots_hook`. `er-reload-trace` detours the same prologue
/// (`HookSpec { name: "map_request_do_836f30", rva: 0x836f30, .. }`, its `lib.rs`), and both shells
/// are co-loaded by hand-written profiles -- `~/Elden/group-1170.me3` carries `er_quickload`,
/// `er_diag_harness`, `er_reload_trace` and `er_armament_icons` together. A bare `MhHook` here is a
/// second MinHook instance on that prologue: each instance is a per-DLL static, so neither can see
/// the other's `MH_CreateHook`, and the second writes its five-byte JMP over the first's. The loser
/// reports installed, never fires, and its counter reads 0 forever -- indistinguishable from a
/// feature that was never enabled. That is the measured 2026-08-23 failure (product +
/// `er-armament-icons` on `TITLE_SCALEFORM_FILE_OPEN_RVA`), re-created on a different address.
///
/// `register_shared_hook` is the repo's answer: it resolves `er_quickload.dll`'s
/// `er_effects_union_register` export and chains this handler onto the product's one instance,
/// falling back to this DLL's own union when the product is absent (a standalone
/// `sweep-er-diag-harness.me3` run is therefore unchanged). Install order stops deciding who runs.
///
/// The address is handed over UNTRANSLATED on purpose. `game_rva` resolves 1.16.2 -> the running
/// build, and so does `register_shared_hook`; passing an already-resolved address would translate
/// twice, and a 1.17 destination can itself be another row's 1.16.2 source (see
/// `er_hook::register_shared_hook_with_budget`'s own note). So this passes `base + rva` and lets the
/// registrar own the single resolve -- exactly what `er-reload-trace`'s `install_one` does.
fn install_shared_dlc_roots_job_hook() {
    let Ok(base) = game_module_base() else {
        diag_log!("dlc-roots-trace: failed to resolve module base for the job trace");
        return;
    };
    match unsafe {
        er_hook::register_shared_hook(
            base + DLC_ROOTS_JOB_RVA,
            dlc_roots_job_trace_hook,
            &DLC_ROOTS_JOB_ORIG,
        )
    } {
        Ok(route) => diag_log!(
            "dlc-roots-trace: hooked DLC-root job rva 0x{DLC_ROOTS_JOB_RVA:x} via {route:?}"
        ),
        Err(status) => diag_log!(
            "dlc-roots-trace: job register_shared_hook failed: {status:?} -- nothing was written at this address"
        ),
    }
}

/// Shared install body for the two root traces -- same shape, different target.
fn install_one_dlc_roots_hook(
    rva: usize,
    detour: *mut c_void,
    orig: &'static AtomicUsize,
    label: &str,
) {
    let Ok(addr) = game_rva_for_hook(rva as u32) else {
        diag_log!("dlc-roots-trace: failed to resolve {label} rva 0x{rva:x}");
        return;
    };
    match unsafe { MhHook::new(addr as *mut c_void, detour) } {
        Ok(hook) => {
            orig.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                diag_log!("dlc-roots-trace: {label} queue_enable failed: {status:?}");
                return;
            }
            diag_log!("dlc-roots-trace: hooked DLC-root {label} at 0x{addr:x}");
        }
        Err(status) => {
            diag_log!("dlc-roots-trace: {label} MhHook::new failed: {status:?}");
        }
    }
}

/// Sample the DLC roots, run `body`, sample again, and log the transition.
fn trace_dlc_roots_transition(kind: &str, n: usize, arg: u8, body: impl FnOnce()) {
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
    diag_log!("dlc-roots-{kind} #{n}: enable={arg} before=[{before}] after=[{after}]");
}

/// Trace detour for `FUN_140e06490` -- the DLC-root blank. Forwards unconditionally.
///
/// # Safety
/// Called by the game; the trampoline is invoked with the arguments unchanged.
pub(crate) unsafe extern "system" fn dlc_roots_blank_trace_hook(csdlc: usize, enable: u8) {
    let n = DLC_ROOTS_BLANK_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    trace_dlc_roots_transition("BLANK", n, enable, || {
        let orig = DLC_ROOTS_BLANK_ORIG.load(Ordering::SeqCst);
        if orig != HOOK_ORIGINAL_UNSET {
            let f: unsafe extern "system" fn(usize, u8) = unsafe { core::mem::transmute(orig) };
            unsafe { f(csdlc, enable) };
        }
    });
}

/// Trace detour for `FUN_140836f30` -- the job body that ultimately reaches the refill. Forwards
/// unconditionally.
///
/// This is the fork in the road. The chain below it is
/// `FUN_140836f30 -> FUN_14082e230 -> FUN_14082eb60 -> FUN_14082dbf0 -> FUN_14082faf0` (which builds
/// the functor whose `_Do_call` is `FUN_1408371e0`, the refill wrapper). A reload that fires this but
/// never reaches `FUN_140e05fb0` means the job runs and diverges inside -- a fixable native path. A
/// reload that never fires it means the job was never enqueued, and its creator is a dynamically
/// built `std::function` with no static registration, so no call site can be patched.
///
/// Four arguments, and all four are forwarded -- which is a bug fix, not only a mechanism change.
///
/// This used to be declared `(this, arg2) -> usize` and forward `f(this, arg2)`, on the reasoning
/// that "the native is `(this, rdx)`". The 1.16.2 decompile says otherwise:
///
/// ```text
/// ulonglong FUN_140836f30(longlong p1, ulonglong p2, undefined8 p3, undefined8 p4)
/// { FUN_14082e230(p2, p2 & 0xff, p1 + 8, p4, 0, 0xfffffffffffffffe); return p2; }
/// ```
///
/// `p4` (`r9`) is a live argument, passed straight through to `FUN_14082e230`. A two-argument
/// `extern "system"` detour only sets `rcx`/`rdx` before calling the trampoline, so whatever this
/// function body last left in `r8`/`r9` was handed to the native instead -- silently, on every
/// call. The four-argument shape carries them verbatim.
///
/// It is also exactly [`er_hook::UnionFn`], which is what lets this handler chain rather than own a
/// second MinHook instance. `er-hook`'s constraint is "<=4 integer/pointer args, no floats", and
/// this target is four integer args returning an integer -- inside it with nothing to spare.
///
/// # Safety
/// Called by the game (or by the previous handler in the union chain). `orig` may hold the next
/// handler rather than the game trampoline, so it is called through the same four-argument
/// signature -- never through the native's narrower one -- and its result is returned verbatim.
pub(crate) unsafe extern "system" fn dlc_roots_job_trace_hook(
    this: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
) -> usize {
    let n = DLC_ROOTS_JOB_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    let roots = match game_module_base().ok().filter(|&b| b != 0) {
        Some(b) => unsafe { dlio_virtual_roots_summary(b) },
        None => String::from("<nobase>"),
    };
    diag_log!(
        "dlc-roots-JOB #{n}: this=0x{this:x} arg2=0x{arg2:x} arg3=0x{arg3:x} arg4=0x{arg4:x} refills_so_far={} roots=[{roots}]",
        DLC_ROOTS_REFILL_CALLS.load(Ordering::SeqCst)
    );
    let orig = DLC_ROOTS_JOB_ORIG.load(Ordering::SeqCst);
    if orig != HOOK_ORIGINAL_UNSET {
        let f: er_hook::UnionFn = unsafe { core::mem::transmute(orig) };
        return unsafe { f(this, arg2, arg3, arg4) };
    }
    0
}

/// Trace detour for `FUN_140e05fb0` -- the DLC-root refill. Forwards unconditionally.
///
/// # Safety
/// Called by the game, and (in a product + harness profile) by `er-title-flow`'s DLC-root self-heal
/// through the raw RVA -- see the module header. The trampoline is invoked with the arguments
/// unchanged in both cases.
pub(crate) unsafe extern "system" fn dlc_roots_refill_trace_hook(csdlc: usize, enable: u8) {
    let n = DLC_ROOTS_REFILL_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    trace_dlc_roots_transition("REFILL", n, enable, || {
        let orig = DLC_ROOTS_REFILL_ORIG.load(Ordering::SeqCst);
        if orig != HOOK_ORIGINAL_UNSET {
            let f: unsafe extern "system" fn(usize, u8) = unsafe { core::mem::transmute(orig) };
            unsafe { f(csdlc, enable) };
        }
    });
}
