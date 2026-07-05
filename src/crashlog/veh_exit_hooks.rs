use std::{
    ffi::c_void,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use crate::input_blocker::InputBlocker;
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
        System::{
            LibraryLoader::{GetModuleHandleA, GetProcAddress},
            Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery},
            SystemServices::DLL_PROCESS_ATTACH,
            Threading::GetCurrentProcessId,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_KEYDOWN,
            WM_KEYUP,
        },
    },
    core::{BOOL, PCSTR},
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{experiments::*, ffi::*, hooks::*, telemetry::*};

pub(crate) const NO_PROCESS_HANDLE: usize = 0;

/// Opt-in: install the crash/exit logger. Off by default so production and
/// normal smoke runs are untouched; enabled for diagnostic runs.
pub(crate) fn crash_logger_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_CRASH_LOG").as_deref(), Ok("1"))
        // Redirecting the crash log to a consolidated per-run path (e.g. the probe's artifact dir)
        // implies the logger is wanted -- treat the redirect env as an enable so the sentinel file
        // is not also required. bd log-output-paths-consolidation.
        || std::env::var("ER_EFFECTS_CRASH_LOG_PATH").is_ok()
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-crash-log.txt")
            .exists()
}

/// Separate, explicit opt-in for deliberate proof-gate faults. Crash logging is diagnostic telemetry;
/// it must not turn semantic semaphore mismatches into crashes unless a run explicitly asks for
/// release/fail-fast behavior.
pub(crate) fn deliberate_fail_fast_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_FAIL_FAST").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-fail-fast.txt")
            .exists()
}

pub(crate) fn log_process_exit(api: &str, code: u32, handle: usize) {
    // Log only the first terminator -- the one that actually quits the game.
    if PROCESS_EXIT_LOGGED.swap(true, Ordering::SeqCst) {
        return;
    }
    append_crash_log(format_args!(
        "process-exit via {api} code=0x{code:x} handle=0x{handle:x} {}",
        trace_callers_summary()
    ));
}

pub(crate) unsafe extern "system" fn exit_process_hook(code: u32) {
    log_process_exit("ExitProcess", code, NO_PROCESS_HANDLE);
    let original = ORIGINAL_EXIT_PROCESS.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(original) };
        unsafe { original(code) };
    }
}

pub(crate) unsafe extern "system" fn terminate_process_hook(handle: *mut c_void, code: u32) -> i32 {
    log_process_exit("TerminateProcess", code, handle as usize);
    let original = ORIGINAL_TERMINATE_PROCESS.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(*mut c_void, u32) -> i32 =
            unsafe { std::mem::transmute(original) };
        return unsafe { original(handle, code) };
    }
    HOOK_FALSE_RETURN as i32
}

pub(crate) unsafe extern "system" fn rtl_exit_user_process_hook(code: u32) {
    log_process_exit("RtlExitUserProcess", code, NO_PROCESS_HANDLE);
    let original = ORIGINAL_RTL_EXIT_USER_PROCESS.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(original) };
        unsafe { original(code) };
    }
}

pub(crate) unsafe extern "system" fn nt_terminate_process_hook(
    handle: *mut c_void,
    status: i32,
) -> i32 {
    log_process_exit("NtTerminateProcess", status as u32, handle as usize);
    let original = ORIGINAL_NT_TERMINATE_PROCESS.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(*mut c_void, i32) -> i32 =
            unsafe { std::mem::transmute(original) };
        return unsafe { original(handle, status) };
    }
    HOOK_FALSE_RETURN as i32
}

/// When set, the assert-wrapper hook returns WITHOUT chaining the original, so a
/// failed FromSoft assertion does not crash -- the game continues past the check.
/// Diagnostic only (may continue in a degraded state); off by default.
pub(crate) fn assert_nonfatal() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_ASSERT_NONFATAL").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-assert-nonfatal.txt")
        .exists()
}

/// Hook on the FromSoft assert wrapper: log the failing assertion's args as RVAs
/// (the expr/message/file wide strings live in .rdata, so they are read offline
/// with recon_strings -- no risky in-process deref) plus the caller, then either
/// chain the original (crashes in the default mode) or, if assert_nonfatal, skip.
pub(crate) unsafe extern "system" fn assert_wrapper_hook(
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
) {
    if ASSERT_LOG_LINES_WRITTEN.fetch_add(AV_LOG_LINE_INCREMENT, Ordering::SeqCst)
        < MAX_ASSERT_LOG_LINES
    {
        let base = game_module_base().unwrap_or(NULL_MODULE_BASE);
        let rva = |pointer: usize| {
            if base != NULL_MODULE_BASE && pointer >= base {
                pointer - base
            } else {
                pointer
            }
        };
        append_crash_log(format_args!(
            "ASSERT a0_rva=0x{:x} a1_rva=0x{:x} a2_rva=0x{:x} a3=0x{arg3:x} {}",
            rva(arg0),
            rva(arg1),
            rva(arg2),
            trace_callers_summary()
        ));
    }
    if assert_nonfatal() {
        return;
    }
    let original = ORIGINAL_ASSERT_WRAPPER.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(usize, usize, usize, usize) =
            unsafe { std::mem::transmute(original) };
        unsafe { original(arg0, arg1, arg2, arg3) };
    }
}

/// Upper bound on a plausible game-module `.text` RVA. The DLL's own anti-antidebug
/// pass logs the scanned code range as `0x140001000..0x1429a2c00`, so a return address
/// into game code has an RVA below ~0x29a2c00. Used to filter a raw stack scan down to
/// game-side return addresses.
const AV_GAME_TEXT_RVA_MAX: usize = 0x2a0_0000;
const AV_GAME_TEXT_RVA_MIN: usize = 0x1000;
/// Number of 8-byte stack slots scanned upward from RSP at an access violation.
const AV_STACK_SCAN_SLOTS: usize = 256;
/// Max game-side return addresses recorded from the stack scan.
const AV_STACK_MAX_RETURNS: usize = 8;
/// Raw stack qwords dumped from RSP regardless of value (a stack smash may leave no
/// game `.text` return address at all — the raw window still shows the smashed frame).
const AV_STACK_RAW_QWORDS: usize = 8;

/// Scan the crashing thread's stack (from `rsp` upward) for values inside the game
/// module's `.text` (return addresses of the game-side frames) AND dump the raw head of
/// the frame. The recorded `callers=[...]` trail only holds our own instrumentation trail
/// (under wine it surfaces ntdll addresses), so this is what actually names the game
/// function at the fault. Reads are `ReadProcessMemory`-guarded so an unmapped slot yields
/// `None` instead of re-faulting into this handler. `.text` hits are emitted as live/deobf
/// RVAs (`addr - base`); map to the Ghidra dump with `scripts/dump-deobf-shift.py`.
fn av_stack_game_returns(rsp: usize, base: usize) -> String {
    if rsp < 0x10000 {
        return String::from("stk=[] self_stk=[] raw=[]");
    }
    let self_base = SELF_DLL_BASE.load(Ordering::SeqCst);
    let self_size = SELF_DLL_SIZE.load(Ordering::SeqCst);
    let mut game = String::from("stk=[");
    let mut selfret = String::from("self_stk=[");
    let mut game_found = 0usize;
    let mut self_found = 0usize;
    let mut slot = 0usize;
    while slot < AV_STACK_SCAN_SLOTS
        && (game_found < AV_STACK_MAX_RETURNS || self_found < AV_STACK_MAX_RETURNS)
    {
        let addr = rsp + slot * std::mem::size_of::<usize>();
        if let Some(val) = unsafe { safe_read_usize(addr) } {
            if base != NULL_MODULE_BASE {
                if let Some(rva) = val.checked_sub(base) {
                    if (AV_GAME_TEXT_RVA_MIN..AV_GAME_TEXT_RVA_MAX).contains(&rva)
                        && game_found < AV_STACK_MAX_RETURNS
                    {
                        if game_found != 0 {
                            game.push(',');
                        }
                        game.push_str(&format!("0x{rva:x}"));
                        game_found += 1;
                    }
                }
            }
            if self_base != NULL_MODULE_BASE {
                if let Some(rva) = val.checked_sub(self_base) {
                    if rva < self_size && self_found < AV_STACK_MAX_RETURNS {
                        if self_found != 0 {
                            selfret.push(',');
                        }
                        selfret.push_str(&format!("0x{rva:x}"));
                        self_found += 1;
                    }
                }
            }
        }
        slot += 1;
    }
    game.push_str("] ");
    game.push_str(&selfret);
    game.push_str("] raw=[");
    for i in 0..AV_STACK_RAW_QWORDS {
        if i != 0 {
            game.push(',');
        }
        match unsafe { safe_read_usize(rsp + i * std::mem::size_of::<usize>()) } {
            Some(v) => {
                let tag = annotate_addr(v, base);
                game.push_str(&format!("0x{v:x}{tag}"));
            }
            None => game.push_str("??"),
        }
    }
    game.push(']');
    game
}

/// Probe a candidate object pointer: read its first qword (a C++ vtable pointer for a
/// polymorphic object) and, when that vtable lands in the game module, emit its RVA so the
/// crashing object's class can be named from the Ghidra dump. Guarded reads; `??`/`-` on
/// unmapped memory. Format: `obj@0x..=[vt=0x.. vtrva=0x..]`.
fn av_object_probe(label: &str, ptr: usize, base: usize) -> String {
    if ptr < 0x10000 {
        return format!("{label}=0x{ptr:x}[unmapped]");
    }
    match unsafe { safe_read_usize(ptr) } {
        Some(vt) => {
            let vtrva = vt.checked_sub(base).filter(|r| {
                base != NULL_MODULE_BASE && (AV_GAME_TEXT_RVA_MIN..0x4000000).contains(r)
            });
            match vtrva {
                Some(r) => format!("{label}=0x{ptr:x}[vt=0x{vt:x} vtrva=0x{r:x}]"),
                None => format!("{label}=0x{ptr:x}[vt=0x{vt:x}]"),
            }
        }
        None => format!("{label}=0x{ptr:x}[unreadable]"),
    }
}

/// PE optional-header offsets (PE32+). `e_lfanew` (DOS header) points at the NT headers;
/// the optional header starts 24 bytes past that (4-byte signature + 20-byte file header),
/// and `SizeOfImage` sits at optional-header +0x38.
const PE_E_LFANEW_OFFSET: usize = 0x3c;
const PE_OPTIONAL_HEADER_FROM_NT: usize = 24;
const PE_SIZE_OF_IMAGE_IN_OPTIONAL: usize = 0x38;
/// Fallback extent used when the DLL's `SizeOfImage` cannot be read (generous upper bound for
/// this cdylib; only used to bound-check self-frame attribution, never for anything semantic).
const SELF_DLL_SIZE_FALLBACK: usize = 0x0400_0000;

/// Record this DLL's load base + image size (called once from `DllMain`). Pure guarded PE-header
/// reads — no APIs, no loader lock — safe to run at `DLL_PROCESS_ATTACH`. Enables `self+0xRVA`
/// annotation of faults in our relocated code (see [`SELF_DLL_BASE`]).
pub(crate) fn record_self_dll_base(base: usize) {
    if base < 0x10000 {
        return;
    }
    SELF_DLL_BASE.store(base, Ordering::SeqCst);
    let size = unsafe { safe_read_usize(base + PE_E_LFANEW_OFFSET) }
        .map(|v| v & 0xffff_ffff)
        .and_then(|e_lfanew| {
            unsafe {
                safe_read_usize(
                    base + e_lfanew + PE_OPTIONAL_HEADER_FROM_NT + PE_SIZE_OF_IMAGE_IN_OPTIONAL,
                )
            }
            .map(|v| v & 0xffff_ffff)
        })
        .filter(|&s| s != 0)
        .unwrap_or(SELF_DLL_SIZE_FALLBACK);
    SELF_DLL_SIZE.store(size, Ordering::SeqCst);
}

/// Annotate a code address with the module + RVA it lands in, for a crash line. Resolves against
/// the game module (`.text`) and this injected DLL (relocated far away under Wine). Returns a
/// compact `{game+0x..}` / `{self+0x..}` tag, or an empty string when the address is in neither
/// (a Wine system DLL, the heap, or a smashed value) — the raw hex is already printed alongside.
fn annotate_addr(addr: usize, game_base: usize) -> String {
    if game_base != NULL_MODULE_BASE {
        if let Some(rva) = addr.checked_sub(game_base) {
            if (AV_GAME_TEXT_RVA_MIN..AV_GAME_TEXT_RVA_MAX).contains(&rva) {
                return format!("{{game+0x{rva:x}}}");
            }
        }
    }
    let self_base = SELF_DLL_BASE.load(Ordering::SeqCst);
    if self_base != NULL_MODULE_BASE {
        if let Some(rva) = addr.checked_sub(self_base) {
            if rva < SELF_DLL_SIZE.load(Ordering::SeqCst) {
                return format!("{{self+0x{rva:x}}}");
            }
        }
    }
    String::new()
}

/// Vectored handler: log access violations (faulting RVA + caller stack) so an
/// in-process crash points straight at the instruction. Rate-limited; never
/// changes behavior (returns EXCEPTION_CONTINUE_SEARCH).
pub(crate) unsafe extern "system" fn crash_vectored_handler(
    info: *mut ExceptionPointersMin,
) -> i32 {
    if !info.is_null() {
        let record = unsafe { (*info).exception_record };
        let context = unsafe { (*info).context_record };
        // Software (INT3) breakpoint: on #BP at one of our armed addresses, log the full
        // register/stack context, restore the original byte, back RIP up to it, and set the
        // trap flag so the next single-step re-arms the INT3 (persistent breakpoint).
        if !record.is_null()
            && !context.is_null()
            && unsafe { (*record).exception_code } == EXCEPTION_BREAKPOINT_CODE
        {
            let cbase = context as *mut u8;
            let rip = unsafe { *(cbase.add(CONTEXT_RIP_OFFSET) as *const u64) } as usize;
            // Windows leaves the saved Rip PAST the INT3 (bp = Rip-1); wine/Proton may leave it
            // AT the INT3 (bp = Rip). Accept either so the lookup is robust across both.
            let cand_past = rip.wrapping_sub(INT3_RIP_BACKUP);
            let cand_at = rip;
            let mut slot = SW_BP_EMPTY;
            let mut found = false;
            let mut bp_addr = cand_past;
            while slot < SW_BP_MAX {
                let armed = SW_BP_ADDR[slot].load(Ordering::SeqCst);
                if armed != SW_BP_EMPTY && (armed == cand_past || armed == cand_at) {
                    found = true;
                    bp_addr = armed;
                    break;
                }
                slot += SW_BP_SLOT_STEP;
            }
            if found {
                let hits = SW_BP_HITS[slot].fetch_add(SW_BP_HIT_INCREMENT, Ordering::SeqCst);
                if hits < SW_BP_MAX_LOGS_PER_BP {
                    let base = game_module_base().unwrap_or(NULL_MODULE_BASE);
                    let read_reg = |off: usize| unsafe { *(cbase.add(off) as *const u64) } as usize;
                    let rva = |pointer: usize| {
                        if base != NULL_MODULE_BASE && pointer >= base {
                            pointer - base
                        } else {
                            pointer
                        }
                    };
                    let rcx = read_reg(CONTEXT_RCX_OFFSET);
                    let rdx = read_reg(CONTEXT_RDX_OFFSET);
                    let r8 = read_reg(CONTEXT_R8_OFFSET);
                    let r9 = read_reg(CONTEXT_R9_OFFSET);
                    let rax = read_reg(CONTEXT_RAX_OFFSET);
                    let rsp = read_reg(CONTEXT_RSP_OFFSET);
                    // RAW stack qwords (NOT rva'd): in-image game return addresses show as full
                    // 0x140xxxxxxx (subtract base for the RVA), our DLL frames as 0x6ffe..., stack/heap
                    // as 0x7ffe..., locals as small values -- so the caller chain up from the BP'd
                    // function is identifiable. Deepened to capture the map-load orchestrator frames.
                    let mut stack = String::new();
                    let mut q = SW_BP_EMPTY;
                    while q < SW_BP_STACK_DUMP_QWORDS {
                        let v =
                            unsafe { *((rsp + q * core::mem::size_of::<usize>()) as *const usize) };
                        stack.push_str(&format!("0x{:x},", v));
                        q += SW_BP_SLOT_STEP;
                    }
                    append_crash_log(format_args!(
                        "sw-bp #{slot} rva=0x{:x} hit={hits} rcx=0x{rcx:x} rdx=0x{rdx:x} r8=0x{r8:x} r9=0x{r9:x} rax=0x{rax:x} rsp=0x{rsp:x} stack=[{stack}] {}",
                        rva(bp_addr),
                        trace_callers_summary()
                    ));
                }
                // (Reverted: an OVERFLOW-GUARD here that reset [rcx+0x48] on the 0x7ad53b push was
                // based on a WRONG premise -- that field is a POINTER (~0x7fff...), not a small count,
                // so dialog+0x50 is NOT a valid DLFixedVector in our context; zeroing it corrupted the
                // dialog -> a new AV. The real issue is the load job's mis-contextualized push target,
                // not an 8-full vector. bd dialog-plus0x50-NOT-a-vector-built-job-miscontextualized.)
                let orig = (SW_BP_ORIG[slot].load(Ordering::SeqCst) & SW_BP_ORIG_BYTE_MASK) as u8;
                unsafe { write_code_byte(bp_addr, orig) };
                unsafe {
                    *(cbase.add(CONTEXT_RIP_OFFSET) as *mut u64) = bp_addr as u64;
                    let eflags = *(cbase.add(CONTEXT_EFLAGS_OFFSET) as *const u32);
                    *(cbase.add(CONTEXT_EFLAGS_OFFSET) as *mut u32) = eflags | TRAP_FLAG_MASK;
                }
                SW_BP_REARM_PENDING.store(bp_addr, Ordering::SeqCst);
                return EXCEPTION_CONTINUE_EXECUTION;
            }
            // #BP not at one of our armed addresses. Log it once (diagnostic: confirms the VEH
            // IS invoked for #BP under wine; the rip tells us if it is ours with a different
            // Rip convention or a foreign breakpoint).
            let seen = SW_BP_UNMATCHED_LOGGED.fetch_add(SW_BP_HIT_INCREMENT, Ordering::SeqCst);
            if seen < SW_BP_MAX_UNMATCHED_LOGS {
                let base = game_module_base().unwrap_or(NULL_MODULE_BASE);
                let rva = if base != NULL_MODULE_BASE && rip >= base {
                    rip - base
                } else {
                    rip
                };
                append_crash_log(format_args!(
                    "sw-bp UNMATCHED #BP rip_rva=0x{rva:x} rip=0x{rip:x} {}",
                    trace_callers_summary()
                ));
            }
            return EXCEPTION_CONTINUE_SEARCH;
        }
        // Hardware watchpoint (DR0) on GameMan+0xc30: a data-write trap surfaces as a
        // single-step exception with DR6 bit0 set. Log the writing instruction's RIP +
        // call stack -- this pins the EXACT function that mounts the save (vanilla
        // 0x67b290-class OR Seamless/ERSC), no guessing -- then one-shot disarm DR7 in
        // the CONTEXT that gets restored and resume execution.
        if !record.is_null()
            && !context.is_null()
            && unsafe { (*record).exception_code } == EXCEPTION_SINGLE_STEP_CODE
        {
            let cbase = context as *mut u8;
            let dr6 = unsafe { *(cbase.add(CONTEXT_DR6_OFFSET) as *const u64) };
            if (dr6 & DR6_DR0_HIT_MASK) == DR6_DR0_HIT_MASK {
                if C30_WATCH_HITS.fetch_add(C30_WATCH_HIT_INCREMENT, Ordering::SeqCst)
                    < MAX_C30_WATCH_HITS
                {
                    let rip = unsafe { *(cbase.add(CONTEXT_RIP_OFFSET) as *const u64) } as usize;
                    let base = game_module_base().unwrap_or(NULL_MODULE_BASE);
                    match rip.checked_sub(base) {
                        Some(rva) if base != NULL_MODULE_BASE => append_crash_log(format_args!(
                            "c30-write rip_rva=0x{rva:x} rip=0x{rip:x} {} {}",
                            trace_callers_summary(),
                            b80_mount_trace_summary()
                        )),
                        _ => append_crash_log(format_args!(
                            "c30-write rip=0x{rip:x} (module unresolved) {} {}",
                            trace_callers_summary(),
                            b80_mount_trace_summary()
                        )),
                    }
                }
                unsafe {
                    *(cbase.add(CONTEXT_DR6_OFFSET) as *mut u64) = DR6_CLEAR;
                    *(cbase.add(CONTEXT_DR7_OFFSET) as *mut u64) = DR7_DISARM;
                }
                return EXCEPTION_CONTINUE_EXECUTION;
            }
            // Software-breakpoint re-arm: this single-step is the one we requested after
            // restoring + stepping over the original instruction. Re-write the INT3 and clear
            // the trap flag so the breakpoint fires again next time.
            let pending = SW_BP_REARM_PENDING.swap(SW_BP_REARM_NONE, Ordering::SeqCst);
            if pending != SW_BP_REARM_NONE {
                unsafe { write_code_byte(pending, INT3_OPCODE) };
                unsafe {
                    let eflags = *(cbase.add(CONTEXT_EFLAGS_OFFSET) as *const u32);
                    *(cbase.add(CONTEXT_EFLAGS_OFFSET) as *mut u32) = eflags & !TRAP_FLAG_MASK;
                }
                return EXCEPTION_CONTINUE_EXECUTION;
            }
        }
        if !record.is_null()
            && unsafe { (*record).exception_code } == EXCEPTION_ACCESS_VIOLATION_CODE
            && AV_LOG_LINES_WRITTEN.fetch_add(AV_LOG_LINE_INCREMENT, Ordering::SeqCst)
                < MAX_AV_LOG_LINES
        {
            let address = unsafe { (*record).exception_address } as usize;
            // For an access violation ExceptionInformation[0] is the access kind
            // (0=read, 1=write, 8=execute) and [1] is the faulting DATA address --
            // the pointer that was actually dereferenced. That plus the accessor
            // registers (RCX/RDX/R8) distinguishes a bad `this` pointer from a wild
            // index without decompilation guesswork.
            let (access_kind, fault_addr) = unsafe {
                if (*record).number_parameters >= 2 {
                    (
                        (*record).exception_information[0],
                        (*record).exception_information[1],
                    )
                } else {
                    (usize::MAX, 0)
                }
            };
            let (rcx, rdx, r8, rsp) = if !context.is_null() {
                let cbase = context as *const u8;
                unsafe {
                    (
                        *(cbase.add(CONTEXT_RCX_OFFSET) as *const u64) as usize,
                        *(cbase.add(CONTEXT_RDX_OFFSET) as *const u64) as usize,
                        *(cbase.add(CONTEXT_R8_OFFSET) as *const u64) as usize,
                        *(cbase.add(CONTEXT_RSP_OFFSET) as *const u64) as usize,
                    )
                }
            } else {
                (0, 0, 0, 0)
            };
            let base = game_module_base().unwrap_or(NULL_MODULE_BASE);
            let stack = av_stack_game_returns(rsp, base);
            let rcx_probe = av_object_probe("rcx", rcx, base);
            // For a hijacked control transfer (access=8, RIP jumped to non-code), the value
            // at [rsp] is the smashed/popped return candidate; probe it as an object too.
            let ret0 = unsafe { safe_read_usize(rsp) }.unwrap_or(0);
            let ret0_probe = av_object_probe("ret0", ret0, base);
            // Code-address annotations: name the faulting RIP and the return-at-[rsp] as
            // game/self module + RVA when they land in known code (a heap-executing RIP under
            // Wine otherwise prints as an undecodable raw value). self_base is emitted so any
            // remaining raw frame can be resolved by hand against the DLL's symbols.
            let rip_tag = annotate_addr(address, base);
            let ret0_tag = annotate_addr(ret0, base);
            let self_base = SELF_DLL_BASE.load(Ordering::SeqCst);
            // Only treat the fault instruction as an in-module RVA when it actually lands in
            // `.text`; an execute-fault RIP in the heap (access=8) is NOT a game RVA and a
            // blind `addr - base` there prints a misleading value.
            let rva = address.checked_sub(base).filter(|r| {
                base != NULL_MODULE_BASE && (AV_GAME_TEXT_RVA_MIN..AV_GAME_TEXT_RVA_MAX).contains(r)
            });
            match rva {
                Some(rva) => append_crash_log(format_args!(
                    "access-violation rva=0x{rva:x} addr=0x{address:x}{rip_tag} access={access_kind:x} fault_addr=0x{fault_addr:x} rcx=0x{rcx:x} rdx=0x{rdx:x} r8=0x{r8:x} rsp=0x{rsp:x} self_base=0x{self_base:x} {rcx_probe} {ret0_probe} ret0_code=0x{ret0:x}{ret0_tag} {stack} {}",
                    trace_callers_summary()
                )),
                None => append_crash_log(format_args!(
                    "access-violation addr=0x{address:x}{rip_tag} (RIP outside .text) access={access_kind:x} fault_addr=0x{fault_addr:x} rcx=0x{rcx:x} rdx=0x{rdx:x} r8=0x{r8:x} rsp=0x{rsp:x} self_base=0x{self_base:x} {rcx_probe} {ret0_probe} ret0_code=0x{ret0:x}{ret0_tag} {stack} {}",
                    trace_callers_summary()
                )),
            }
        }
    }
    EXCEPTION_CONTINUE_SEARCH
}

/// Opt-in: arm a hardware write-watchpoint on GameMan+0xc30 (the save-mount map
/// write) so the exact writing instruction traps into the VEH. Requires the crash
/// logger (the VEH) to be installed.
pub(crate) fn c30_watch_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_C30_WATCH").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-c30-watch.txt")
            .exists()
}

/// Set DR0 = target_addr and DR7 = 4-byte data-write breakpoint on every game thread
/// (except ours) via Suspend/Get/Set/ResumeThread. Returns how many threads were armed.
/// Deadlock-safe: the CONTEXT buffer is stack-only and no heap alloc happens while a
/// thread is suspended (one thread suspended at a time).
pub(crate) unsafe fn arm_c30_watchpoint(target_addr: usize) -> i32 {
    let process_id = unsafe { GetCurrentProcessId() };
    let my_thread_id = unsafe { GetCurrentThreadId() };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, TOOLHELP_ALL_PROCESSES) };
    if snapshot == TOOLHELP_INVALID_SNAPSHOT {
        return C30_WATCH_ARM_COUNT_NONE;
    }
    let mut armed = C30_WATCH_ARM_COUNT_NONE;
    let mut entry: ThreadEntry32 = unsafe { std::mem::zeroed() };
    entry.dw_size = std::mem::size_of::<ThreadEntry32>() as u32;
    if unsafe { Thread32First(snapshot, &mut entry) } == TOOLHELP_ITER_OK {
        loop {
            if entry.th32_owner_process_id == process_id && entry.th32_thread_id != my_thread_id {
                let handle = unsafe {
                    OpenThread(
                        THREAD_WATCH_ACCESS,
                        INHERIT_HANDLE_FALSE,
                        entry.th32_thread_id,
                    )
                };
                if handle != INVALID_THREAD_HANDLE {
                    unsafe { SuspendThread(handle) };
                    // 16-byte-aligned stack CONTEXT (over-allocate + round the ptr up).
                    let mut raw = [CONTEXT_ZERO_FILL; CONTEXT_AMD64_SIZE + CONTEXT_ALIGN];
                    let aligned =
                        (raw.as_mut_ptr() as usize + CONTEXT_ALIGN_MASK) & !CONTEXT_ALIGN_MASK;
                    let cbase = aligned as *mut u8;
                    unsafe {
                        *(cbase.add(CONTEXT_FLAGS_OFFSET) as *mut u32) =
                            CONTEXT_DEBUG_REGISTERS_FLAG;
                    }
                    if unsafe { GetThreadContext(handle, cbase as *mut c_void) }
                        == SET_THREAD_CONTEXT_OK
                    {
                        unsafe {
                            *(cbase.add(CONTEXT_FLAGS_OFFSET) as *mut u32) =
                                CONTEXT_DEBUG_REGISTERS_FLAG;
                            *(cbase.add(CONTEXT_DR0_OFFSET) as *mut u64) = target_addr as u64;
                            *(cbase.add(CONTEXT_DR6_OFFSET) as *mut u64) = DR6_CLEAR;
                            *(cbase.add(CONTEXT_DR7_OFFSET) as *mut u64) = DR7_C30_WRITE_WATCH;
                        }
                        if unsafe { SetThreadContext(handle, cbase as *const c_void) }
                            == SET_THREAD_CONTEXT_OK
                        {
                            armed += C30_WATCH_ARM_INCREMENT;
                        }
                    }
                    unsafe { ResumeThread(handle) };
                    unsafe { CloseHandle(handle) };
                }
            }
            if unsafe { Thread32Next(snapshot, &mut entry) } != TOOLHELP_ITER_OK {
                break;
            }
        }
    }
    unsafe { CloseHandle(snapshot) };
    armed
}

/// Resolve GameMan+0xc30 live and (re-)arm the watchpoint until the first hit. Re-arms
/// every C30_WATCH_REARM_INTERVAL frames to cover load threads spawned after the first
/// arm. No-op once a write has been caught.
pub(crate) unsafe fn maybe_arm_c30_watch(module_base: usize, tick: u64) {
    if C30_WATCH_HITS.load(Ordering::SeqCst) > C30_WATCH_NEVER_ARMED {
        return;
    }
    let now = tick as usize + C30_WATCH_TICK_BIAS;
    let last = C30_WATCH_LAST_ARM_TICK.load(Ordering::SeqCst);
    if last != C30_WATCH_NEVER_ARMED && now.saturating_sub(last) < C30_WATCH_REARM_INTERVAL {
        return;
    }
    let game_man = game_man_ptr_or_null();
    if game_man == NULL_MODULE_BASE {
        return;
    }
    let target = game_man + GAME_MAN_SAVED_MAP_C30_OFFSET;
    let armed = unsafe { arm_c30_watchpoint(target) };
    C30_WATCH_LAST_ARM_TICK.store(now, Ordering::SeqCst);
    append_crash_log(format_args!(
        "c30-watch (re)armed on {armed} threads target=0x{target:x} game_man=0x{game_man:x} tick={tick}"
    ));
}

/// Opt-in: install software (INT3) breakpoints. Reads er-effects-breakpoints.txt (one
/// hex RVA per line) from the game dir. Requires the crash logger (the VEH) installed.
pub(crate) fn sw_breakpoints_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_SW_BP").as_deref(), Ok("1"))
        || sw_breakpoints_file().is_some()
}

fn sw_breakpoints_file() -> Option<PathBuf> {
    let path = game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-breakpoints.txt");
    if path.exists() { Some(path) } else { None }
}

/// Patch a single executable byte (VirtualProtect RWX -> write -> restore protection).
/// Used to arm/restore/re-arm an INT3. Returns true on success.
pub(crate) unsafe fn write_code_byte(addr: usize, byte: u8) -> bool {
    let mut old: u32 = PROTECT_OLD_INIT;
    let ok = unsafe {
        VirtualProtect(
            addr as *mut c_void,
            INT3_PATCH_SIZE,
            PAGE_EXECUTE_READWRITE,
            &mut old,
        )
    };
    if ok == SET_THREAD_CONTEXT_OK {
        unsafe { *(addr as *mut u8) = byte };
        let mut restored: u32 = PROTECT_OLD_INIT;
        unsafe {
            VirtualProtect(addr as *mut c_void, INT3_PATCH_SIZE, old, &mut restored);
        }
        true
    } else {
        false
    }
}

/// Install the INT3 breakpoints listed (as hex RVAs) in er-effects-breakpoints.txt, once.
/// Each is patched with 0xCC; the VEH (crash_vectored_handler) logs every hit's full
/// register/stack context and re-arms it (persistent breakpoint).
pub(crate) unsafe fn install_sw_breakpoints_once(module_base: usize) {
    if SW_BP_INSTALLED.swap(SW_BP_HIT_INCREMENT, Ordering::SeqCst) != SW_BP_REARM_NONE {
        return;
    }
    let Some(path) = sw_breakpoints_file() else {
        // env-enabled but no file: nothing to install.
        return;
    };
    let Ok(contents) = fs::read_to_string(&path) else {
        return;
    };
    let mut slot = SW_BP_EMPTY;
    for line in contents.lines() {
        let trimmed = line
            .trim()
            .trim_start_matches("0x")
            .trim_start_matches("0X");
        if trimmed.is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let Ok(rva) = usize::from_str_radix(trimmed, RVA_HEX_RADIX) else {
            continue;
        };
        if slot >= SW_BP_MAX {
            append_crash_log(format_args!("sw-bp: table full, skipped rva=0x{rva:x}"));
            break;
        }
        let addr = module_base + rva;
        let orig = unsafe { *(addr as *const u8) };
        SW_BP_ADDR[slot].store(addr, Ordering::SeqCst);
        SW_BP_ORIG[slot].store(orig as usize, Ordering::SeqCst);
        let armed = unsafe { write_code_byte(addr, INT3_OPCODE) };
        append_crash_log(format_args!(
            "sw-bp #{slot} armed rva=0x{rva:x} addr=0x{addr:x} orig=0x{orig:x} ok={armed}"
        ));
        slot += SW_BP_SLOT_STEP;
    }
}

/// Opt-in: apply the anti-anti-debug patches (so debug exceptions / our INT3 breakpoints reach
/// our VEH). Auto-enabled whenever software breakpoints are enabled (they require it).
pub(crate) fn anti_antidebug_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_ANTI_ANTIDEBUG").as_deref(),
        Ok("1")
    ) || sw_breakpoints_enabled()
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-anti-antidebug.txt")
            .exists()
}

/// Parse a "7A ?? 75" hex/wildcard pattern into per-byte Option<u8> (None = wildcard).
fn parse_byte_pattern(spec: &str) -> Vec<Option<u8>> {
    spec.split_whitespace()
        .map(|token| {
            if token == PATTERN_WILDCARD {
                None
            } else {
                u8::from_str_radix(token, RVA_HEX_RADIX).ok()
            }
        })
        .collect()
}
