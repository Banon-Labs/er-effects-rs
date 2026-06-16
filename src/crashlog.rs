//! crashlog module (split from lib.rs; pure code reorganization, no behavior change).

#![allow(unused_imports)]

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

use debug::InputBlocker;
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use hudhook::{
    ImguiRenderLoop, MessageFilter,
    hooks::dx12::ImguiDx12Hooks,
    imgui::{Condition, Context, Ui},
    mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook},
    windows::{
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
    },
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
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-crash-log.txt")
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

/// Vectored handler: log access violations (faulting RVA + caller stack) so an
/// in-process crash points straight at the instruction. Rate-limited; never
/// changes behavior (returns EXCEPTION_CONTINUE_SEARCH).
pub(crate) unsafe extern "system" fn crash_vectored_handler(
    info: *mut ExceptionPointersMin,
) -> i32 {
    if !info.is_null() {
        let record = unsafe { (*info).exception_record };
        let context = unsafe { (*info).context_record };
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
        }
        if !record.is_null()
            && unsafe { (*record).exception_code } == EXCEPTION_ACCESS_VIOLATION_CODE
            && AV_LOG_LINES_WRITTEN.fetch_add(AV_LOG_LINE_INCREMENT, Ordering::SeqCst)
                < MAX_AV_LOG_LINES
        {
            let address = unsafe { (*record).exception_address } as usize;
            let rva = game_module_base()
                .ok()
                .and_then(|base| address.checked_sub(base));
            match rva {
                Some(rva) => append_crash_log(format_args!(
                    "access-violation rva=0x{rva:x} addr=0x{address:x} {}",
                    trace_callers_summary()
                )),
                None => append_crash_log(format_args!(
                    "access-violation addr=0x{address:x} (outside game module) {}",
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
    let game_man =
        unsafe { *((module_base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) as *const usize) };
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

/// Install the crash/exit logger: a vectored handler for access violations plus
/// MinHooks on the process-exit paths. The exit hooks catch a CLEAN watchdog
/// termination (ExitProcess) that no exception debugger can observe, and record
/// which game code requested the exit.
pub(crate) fn install_crash_logger() {
    CRASH_LOGGER_INSTALLED.call_once(|| {
        unsafe { AddVectoredExceptionHandler(VECTORED_FIRST_HANDLER, crash_vectored_handler) };
        match unsafe { MH_Initialize() } {
            MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
            status => append_crash_log(format_args!(
                "crash-logger MH_Initialize failed: {status:?}"
            )),
        }
        let targets: [(&str, &[u8], &[u8], *mut c_void, &AtomicUsize); CRASH_EXIT_TARGET_COUNT] = [
            (
                "ExitProcess",
                b"kernel32.dll\0",
                b"ExitProcess\0",
                exit_process_hook as *mut c_void,
                &ORIGINAL_EXIT_PROCESS,
            ),
            (
                "TerminateProcess",
                b"kernel32.dll\0",
                b"TerminateProcess\0",
                terminate_process_hook as *mut c_void,
                &ORIGINAL_TERMINATE_PROCESS,
            ),
            (
                "RtlExitUserProcess",
                b"ntdll.dll\0",
                b"RtlExitUserProcess\0",
                rtl_exit_user_process_hook as *mut c_void,
                &ORIGINAL_RTL_EXIT_USER_PROCESS,
            ),
            (
                "NtTerminateProcess",
                b"ntdll.dll\0",
                b"NtTerminateProcess\0",
                nt_terminate_process_hook as *mut c_void,
                &ORIGINAL_NT_TERMINATE_PROCESS,
            ),
        ];
        for (name, module, proc, hook_impl, original) in targets {
            match safe_input_proc(module, proc) {
                Ok(target) => unsafe {
                    create_and_apply_single_hook(name, target, hook_impl, original)
                },
                Err(error) => {
                    append_crash_log(format_args!("crash-logger resolve {name} failed: {error}"))
                }
            }
        }
        // Hook the assert wrapper by absolute address (not an export) to capture
        // the failing assertion before its deliberate crash.
        match game_module_base() {
            Ok(base) => unsafe {
                create_and_apply_single_hook(
                    "AssertWrapper",
                    (base + ASSERT_WRAPPER_RVA) as *mut c_void,
                    assert_wrapper_hook as *mut c_void,
                    &ORIGINAL_ASSERT_WRAPPER,
                )
            },
            Err(error) => append_crash_log(format_args!(
                "crash-logger assert-wrapper base failed: {error}"
            )),
        }
        append_crash_log(format_args!(
            "crash logger installed (VEH + exit-path hooks + assert wrapper)"
        ));
    });
}

pub(crate) unsafe fn object_vtable_summary(ptr: *mut c_void) -> String {
    if ptr.is_null() {
        return "vtable_rva=null".to_owned();
    }
    let vtable = unsafe { *(ptr as *const usize) };
    let rva = game_module_base()
        .ok()
        .and_then(|module_base| vtable.checked_sub(module_base));
    rva.map_or_else(
        || format!("vtable=0x{vtable:x} vtable_rva=unknown"),
        |value| format!("vtable=0x{vtable:x} vtable_rva=0x{value:x}"),
    )
}

#[cfg(windows)]
pub(crate) fn trace_callers_summary() -> String {
    let mut frames = [std::ptr::null_mut::<c_void>(); STACK_TRACE_FRAME_COUNT];
    let captured = unsafe {
        RtlCaptureStackBackTrace(
            STACK_TRACE_FRAMES_TO_SKIP,
            frames.len() as u32,
            frames.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    } as usize;
    let module_base = unsafe { GetModuleHandleA(PCSTR::null()) }
        .ok()
        .map(|module| module.0 as usize)
        .unwrap_or(NULL_MODULE_BASE);

    let callers = frames
        .iter()
        .take(captured)
        .enumerate()
        .map(|(index, frame)| {
            let address = *frame as usize;
            if module_base != NULL_MODULE_BASE && address >= module_base {
                format!("#{index}=0x{:x}", address - module_base)
            } else {
                format!("#{index}=0x{address:x}")
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("callers=[{callers}]")
}

#[cfg(not(windows))]
pub(crate) fn trace_callers_summary() -> String {
    "callers=[]".to_owned()
}
