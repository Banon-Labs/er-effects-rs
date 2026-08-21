use std::{
    ffi::{OsString, c_void},
    fmt,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    sync::{
        Once,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use er_game_base::{
    log::{append_line, game_directory_path},
    mem::{game_module_base, safe_read_usize},
};
use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};

const CRASH_LOG_FILE_NAME: &str = "er-better-refills-crash-log.txt";
const FORCE_CRASH_MARKER_FILE_NAME: &str = "er_better_refills_force_crash.txt";

const VECTORED_FIRST_HANDLER: u32 = 1;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const EXCEPTION_ACCESS_VIOLATION_CODE: u32 = 0xC000_0005;
const MAX_EXCEPTION_LOG_LINES: u64 = 64;
const MAX_STACK_QWORDS: usize = 24;
const MAX_BACKTRACE_FRAMES: u32 = 32;

const CONTEXT_RAX_OFFSET: usize = 0x78;
const CONTEXT_RCX_OFFSET: usize = 0x80;
const CONTEXT_RDX_OFFSET: usize = 0x88;
const CONTEXT_RSP_OFFSET: usize = 0x98;
const CONTEXT_R8_OFFSET: usize = 0xb8;
const CONTEXT_R9_OFFSET: usize = 0xc0;
const CONTEXT_RIP_OFFSET: usize = 0xf8;

const PE_DOS_LFANEW_OFFSET: usize = 0x3c;
const PE_OPTIONAL_HEADER_FROM_NT: usize = 0x18;
const PE_SIZE_OF_IMAGE_IN_OPTIONAL: usize = 0x38;
const PE_U32_MASK: usize = 0xffff_ffff;

static CRASH_LOGGER_INSTALLED: Once = Once::new();
static CRASH_LOG_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static EXCEPTION_LOG_LINES: AtomicU64 = AtomicU64::new(0);
static SELF_DLL_BASE: AtomicUsize = AtomicUsize::new(0);
static SELF_DLL_SIZE: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL_EXIT_PROCESS: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL_TERMINATE_PROCESS: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL_RTL_EXIT_USER_PROCESS: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL_NT_TERMINATE_PROCESS: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
struct ExceptionRecordMin {
    exception_code: u32,
    exception_flags: u32,
    exception_record: *mut ExceptionRecordMin,
    exception_address: *mut c_void,
    number_parameters: u32,
    exception_information: [usize; 15],
}

#[repr(C)]
struct ExceptionPointersMin {
    exception_record: *mut ExceptionRecordMin,
    context_record: *mut c_void,
}

type VectoredHandler = unsafe extern "system" fn(*mut ExceptionPointersMin) -> i32;

unsafe extern "system" {
    fn AddVectoredExceptionHandler(first: u32, handler: VectoredHandler) -> *mut c_void;
    fn RtlCaptureStackBackTrace(
        frames_to_skip: u32,
        frames_to_capture: u32,
        backtrace: *mut *mut c_void,
        backtrace_hash: *mut u32,
    ) -> u16;
    fn GetModuleHandleA(module_name: *const u8) -> *mut c_void;
    fn GetModuleFileNameW(module: *mut c_void, filename: *mut u16, size: u32) -> u32;
    fn GetProcAddress(module: *mut c_void, proc_name: *const u8) -> *mut c_void;
}

pub(crate) fn install(self_module_base: usize) {
    record_self_module(self_module_base);
    CRASH_LOGGER_INSTALLED.call_once(|| {
        append_crash_log(format_args!(
            "install: better-refills crash logger self_base=0x{:x} self_size=0x{:x}",
            SELF_DLL_BASE.load(Ordering::SeqCst),
            SELF_DLL_SIZE.load(Ordering::SeqCst)
        ));
        unsafe { AddVectoredExceptionHandler(VECTORED_FIRST_HANDLER, crash_vectored_handler) };
        match unsafe { MH_Initialize() } {
            MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
            status => append_crash_log(format_args!(
                "install: crash logger MH_Initialize failed: {status:?}"
            )),
        }
        install_exit_hook(
            "ExitProcess",
            b"kernel32.dll\0",
            b"ExitProcess\0",
            exit_process_hook as *mut c_void,
            &ORIGINAL_EXIT_PROCESS,
        );
        install_exit_hook(
            "TerminateProcess",
            b"kernel32.dll\0",
            b"TerminateProcess\0",
            terminate_process_hook as *mut c_void,
            &ORIGINAL_TERMINATE_PROCESS,
        );
        install_exit_hook(
            "RtlExitUserProcess",
            b"ntdll.dll\0",
            b"RtlExitUserProcess\0",
            rtl_exit_user_process_hook as *mut c_void,
            &ORIGINAL_RTL_EXIT_USER_PROCESS,
        );
        install_exit_hook(
            "NtTerminateProcess",
            b"ntdll.dll\0",
            b"NtTerminateProcess\0",
            nt_terminate_process_hook as *mut c_void,
            &ORIGINAL_NT_TERMINATE_PROCESS,
        );
        match unsafe { MH_ApplyQueued() } {
            MH_STATUS::MH_OK => append_crash_log(format_args!("install: exit hooks ACTIVE")),
            status => append_crash_log(format_args!("install: MH_ApplyQueued failed: {status:?}")),
        }
    });
}

pub(crate) fn force_crash_requested() -> bool {
    self_module_path()
        .and_then(|path| {
            path.parent()
                .map(|dir| dir.join(FORCE_CRASH_MARKER_FILE_NAME))
        })
        .is_some_and(|marker| marker.is_file())
}

pub(crate) unsafe fn force_crash_for_smoke() {
    append_crash_log(format_args!(
        "force-crash: marker {FORCE_CRASH_MARKER_FILE_NAME} present next to DLL; deliberately reading unmapped address 0x1"
    ));
    let bad = 0x1usize as *const u8;
    let _ = unsafe { std::ptr::read_volatile(bad) };
}

fn record_self_module(base: usize) {
    if base < 0x10_000 {
        return;
    }
    SELF_DLL_BASE.store(base, Ordering::SeqCst);
    let size = unsafe { safe_read_usize(base + PE_DOS_LFANEW_OFFSET) }
        .map(|value| value & PE_U32_MASK)
        .and_then(|e_lfanew| {
            unsafe {
                safe_read_usize(
                    base + e_lfanew + PE_OPTIONAL_HEADER_FROM_NT + PE_SIZE_OF_IMAGE_IN_OPTIONAL,
                )
            }
            .map(|value| value & PE_U32_MASK)
        })
        .unwrap_or(0);
    SELF_DLL_SIZE.store(size, Ordering::SeqCst);
}

fn install_exit_hook(
    name: &str,
    module_name: &'static [u8],
    proc_name: &'static [u8],
    hook_impl: *mut c_void,
    original: &'static AtomicUsize,
) {
    let module = unsafe { GetModuleHandleA(module_name.as_ptr()) };
    if module.is_null() {
        append_crash_log(format_args!("install: {name} module unavailable"));
        return;
    }
    let target = unsafe { GetProcAddress(module, proc_name.as_ptr()) };
    if target.is_null() {
        append_crash_log(format_args!("install: {name} proc unavailable"));
        return;
    }
    match unsafe { MhHook::new(target, hook_impl) } {
        Ok(hook) => {
            original.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_crash_log(format_args!("install: queue {name} failed: {status:?}"));
            } else {
                append_crash_log(format_args!("install: queued exit hook {name}"));
            }
            std::mem::forget(hook);
        }
        Err(status) => append_crash_log(format_args!("install: hook {name} failed: {status:?}")),
    }
}

unsafe extern "system" fn crash_vectored_handler(info: *mut ExceptionPointersMin) -> i32 {
    if !info.is_null() {
        let record = unsafe { (*info).exception_record };
        let context = unsafe { (*info).context_record };
        if !record.is_null()
            && !context.is_null()
            && EXCEPTION_LOG_LINES.fetch_add(1, Ordering::SeqCst) < MAX_EXCEPTION_LOG_LINES
        {
            let code = unsafe { (*record).exception_code };
            if code != EXCEPTION_ACCESS_VIOLATION_CODE {
                return EXCEPTION_CONTINUE_SEARCH;
            }
            let address = unsafe { (*record).exception_address as usize };
            let fault_addr = unsafe { (*record).exception_information[1] };
            let access_kind = unsafe { (*record).exception_information[0] };
            let cbase = context as *mut u8;
            let read_reg = |off: usize| unsafe { *(cbase.add(off) as *const u64) } as usize;
            let rip = read_reg(CONTEXT_RIP_OFFSET);
            let rsp = read_reg(CONTEXT_RSP_OFFSET);
            let rcx = read_reg(CONTEXT_RCX_OFFSET);
            let rdx = read_reg(CONTEXT_RDX_OFFSET);
            let r8 = read_reg(CONTEXT_R8_OFFSET);
            let r9 = read_reg(CONTEXT_R9_OFFSET);
            let rax = read_reg(CONTEXT_RAX_OFFSET);
            let mut stack = String::new();
            for index in 0..MAX_STACK_QWORDS {
                let stack_addr = rsp + index * core::mem::size_of::<usize>();
                let value = unsafe { *(stack_addr as *const usize) };
                stack.push_str(&format!("{}(0x{value:x}),", address_tag(value)));
            }
            append_crash_log(format_args!(
                "exception code=0x{code:x} access-violation exception_addr={} rip={} access={access_kind:x} fault_addr={} rcx={} rdx={} r8={} r9={} rax={} rsp={} stack=[{stack}] captured_bt=[{}]",
                address_tag(address),
                address_tag(rip),
                address_tag(fault_addr),
                address_tag(rcx),
                address_tag(rdx),
                address_tag(r8),
                address_tag(r9),
                address_tag(rax),
                address_tag(rsp),
                captured_backtrace_summary()
            ));
        }
    }
    EXCEPTION_CONTINUE_SEARCH
}

unsafe extern "system" fn exit_process_hook(code: u32) {
    log_process_exit("ExitProcess", code, 0);
    let original = ORIGINAL_EXIT_PROCESS.load(Ordering::SeqCst);
    if original != 0 {
        let original: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(original) };
        unsafe { original(code) };
    }
}

unsafe extern "system" fn terminate_process_hook(handle: *mut c_void, code: u32) -> i32 {
    log_process_exit("TerminateProcess", code, handle as usize);
    let original = ORIGINAL_TERMINATE_PROCESS.load(Ordering::SeqCst);
    if original != 0 {
        let original: unsafe extern "system" fn(*mut c_void, u32) -> i32 =
            unsafe { std::mem::transmute(original) };
        return unsafe { original(handle, code) };
    }
    0
}

unsafe extern "system" fn rtl_exit_user_process_hook(code: u32) {
    log_process_exit("RtlExitUserProcess", code, 0);
    let original = ORIGINAL_RTL_EXIT_USER_PROCESS.load(Ordering::SeqCst);
    if original != 0 {
        let original: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(original) };
        unsafe { original(code) };
    }
}

unsafe extern "system" fn nt_terminate_process_hook(handle: *mut c_void, code: u32) -> i32 {
    log_process_exit("NtTerminateProcess", code, handle as usize);
    let original = ORIGINAL_NT_TERMINATE_PROCESS.load(Ordering::SeqCst);
    if original != 0 {
        let original: unsafe extern "system" fn(*mut c_void, u32) -> i32 =
            unsafe { std::mem::transmute(original) };
        return unsafe { original(handle, code) };
    }
    0
}

fn log_process_exit(name: &str, code: u32, handle: usize) {
    append_crash_log(format_args!(
        "process-exit {name} code=0x{code:x} handle=0x{handle:x} captured_bt=[{}]",
        captured_backtrace_summary()
    ));
}

fn captured_backtrace_summary() -> String {
    let mut frames = [std::ptr::null_mut::<c_void>(); MAX_BACKTRACE_FRAMES as usize];
    let captured = unsafe {
        RtlCaptureStackBackTrace(
            0,
            MAX_BACKTRACE_FRAMES,
            frames.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    } as usize;
    let mut out = String::new();
    for frame in frames.iter().take(captured) {
        out.push_str(&format!("{};", address_tag(*frame as usize)));
    }
    out
}

fn append_crash_log(args: fmt::Arguments<'_>) {
    let path = crash_log_path();
    let seq = CRASH_LOG_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1;
    append_line(&path, format_args!("[{seq:06}] {args}"));
}

fn crash_log_path() -> PathBuf {
    game_directory_path()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(CRASH_LOG_FILE_NAME)
}

fn self_module_path() -> Option<PathBuf> {
    let base = SELF_DLL_BASE.load(Ordering::SeqCst);
    if base == 0 {
        return None;
    }
    let mut buf = [0u16; 1024];
    let len = unsafe { GetModuleFileNameW(base as *mut c_void, buf.as_mut_ptr(), buf.len() as u32) }
        as usize;
    if len == 0 || len >= buf.len() {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&buf[..len])))
}

fn address_tag(address: usize) -> String {
    if address == 0 {
        return "0x0".to_owned();
    }
    let self_base = SELF_DLL_BASE.load(Ordering::SeqCst);
    let self_size = SELF_DLL_SIZE.load(Ordering::SeqCst);
    if self_base != 0 && self_size != 0 && address >= self_base && address < self_base + self_size {
        return format!("self+0x{:x}", address - self_base);
    }
    if let Ok(game_base) = game_module_base() {
        if address >= game_base && address < game_base + 0x1000_0000 {
            return format!("game+0x{:x}", address - game_base);
        }
    }
    format!("0x{address:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_crash_marker_name_is_dll_scoped() {
        assert_eq!(
            FORCE_CRASH_MARKER_FILE_NAME,
            "er_better_refills_force_crash.txt"
        );
    }

    #[test]
    fn context_offsets_match_win64_layout_used_by_product_logger() {
        assert_eq!(CONTEXT_RCX_OFFSET, 0x80);
        assert_eq!(CONTEXT_RDX_OFFSET, 0x88);
        assert_eq!(CONTEXT_RSP_OFFSET, 0x98);
        assert_eq!(CONTEXT_RIP_OFFSET, 0xf8);
    }
}
