//! Reusable crash logging for ME3-loaded Elden Ring companion DLLs.
//!
//! This crate owns the general pieces that do not belong to a product feature:
//! a first-chance vectored exception handler, fresh-per-process crash log files,
//! fault-time register/stack/module snapshots, and low-frequency breadcrumbs.
//! Feature DLLs can depend on this crate and mark phases/breadcrumbs; the sibling
//! `er-crash-logging-dll` crate is a tiny standalone ME3-loadable shell.

use std::{fmt, path::PathBuf, sync::OnceLock};

#[cfg(windows)]
use std::{
    ffi::c_void,
    fs,
    sync::atomic::{AtomicUsize, Ordering},
};

const DEFAULT_LOG_FILE: &str = "er-crash-log.txt";
const DEFAULT_LATEST_FILE: &str = "er-crash-latest.txt";
const DEFAULT_BREADCRUMB_FILE: &str = "er-crash-breadcrumb-latest.txt";
const DEFAULT_MODULE_LABEL: &str = "er-crash-logging";

#[derive(Clone, Copy, Debug)]
pub struct CrashLogConfig {
    pub log_file_name: &'static str,
    pub latest_file_name: &'static str,
    pub breadcrumb_file_name: &'static str,
    pub module_label: &'static str,
}

impl Default for CrashLogConfig {
    fn default() -> Self {
        Self {
            log_file_name: DEFAULT_LOG_FILE,
            latest_file_name: DEFAULT_LATEST_FILE,
            breadcrumb_file_name: DEFAULT_BREADCRUMB_FILE,
            module_label: DEFAULT_MODULE_LABEL,
        }
    }
}

#[repr(usize)]
#[derive(Clone, Copy, Debug)]
pub enum Phase {
    Uninitialized = 0,
    DllAttach = 1,
    HandlerInstalled = 2,
    ClientReady = 10,
    ExceptionObserved = 90,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::Uninitialized => "uninitialized",
            Self::DllAttach => "dll-attach",
            Self::HandlerInstalled => "handler-installed",
            Self::ClientReady => "client-ready",
            Self::ExceptionObserved => "exception-observed",
        }
    }
}

static CONFIG: OnceLock<CrashLogConfig> = OnceLock::new();

fn config() -> CrashLogConfig {
    *CONFIG.get_or_init(CrashLogConfig::default)
}

fn path_for(name: &str) -> PathBuf {
    er_game_base::log::game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(name)
}

pub fn append_log(args: fmt::Arguments<'_>) {
    er_game_base::log::append_line(&path_for(config().log_file_name), args);
}

pub fn write_breadcrumb(reason: &str, args: fmt::Arguments<'_>) {
    let mut body = String::new();
    use fmt::Write as _;
    let _ = writeln!(body, "reason={reason}");
    let _ = writeln!(body, "module={}", config().module_label);
    write_common_fields(&mut body);
    let _ = writeln!(body, "details={args}");
    #[cfg(windows)]
    let _ = fs::write(path_for(config().breadcrumb_file_name), body);
    #[cfg(not(windows))]
    let _ = std::fs::write(path_for(config().breadcrumb_file_name), body);
}

fn write_common_fields(out: &mut String) {
    #[cfg(windows)]
    {
        use fmt::Write as _;
        let phase = PHASE.load(Ordering::SeqCst);
        let _ = writeln!(out, "phase_id={phase}");
        let _ = writeln!(out, "phase={}", phase_label(phase));
        let _ = writeln!(
            out,
            "self_module_base=0x{:x}",
            SELF_MODULE_BASE.load(Ordering::SeqCst)
        );
        let _ = writeln!(
            out,
            "self_module_size=0x{:x}",
            SELF_MODULE_SIZE.load(Ordering::SeqCst)
        );
    }
    #[cfg(not(windows))]
    {
        use fmt::Write as _;
        let _ = writeln!(out, "phase_id=0");
        let _ = writeln!(out, "phase=uninitialized");
        let _ = writeln!(out, "self_module_base=0x0");
        let _ = writeln!(out, "self_module_size=0x0");
    }
}

#[cfg(windows)]
pub fn mark_phase(phase: Phase) {
    PHASE.store(phase as usize, Ordering::SeqCst);
}

#[cfg(not(windows))]
pub fn mark_phase(_phase: Phase) {}

#[cfg(windows)]
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
#[cfg(windows)]
const VECTORED_FIRST_HANDLER: u32 = 1;
#[cfg(windows)]
const EXCEPTION_ACCESS_VIOLATION: u32 = 0xc000_0005;
#[cfg(windows)]
const EXCEPTION_ILLEGAL_INSTRUCTION: u32 = 0xc000_001d;
#[cfg(windows)]
const EXCEPTION_STACK_BUFFER_OVERRUN: u32 = 0xc000_0409;
#[cfg(windows)]
const EXCEPTION_STACK_OVERFLOW: u32 = 0xc000_00fd;
#[cfg(windows)]
const EXCEPTION_HEAP_CORRUPTION: u32 = 0xc000_0374;
#[cfg(windows)]
const EXCEPTION_IN_PAGE_ERROR: u32 = 0xc000_0006;
#[cfg(windows)]
const EXCEPTION_INT_DIVIDE_BY_ZERO: u32 = 0xc000_0094;
#[cfg(windows)]
const EXCEPTION_PRIVILEGED_INSTRUCTION: u32 = 0xc000_0096;
#[cfg(windows)]
const EXCEPTION_NONCONTINUABLE: u32 = 0xc000_0025;
#[cfg(windows)]
const EXCEPTION_CPP_THROW: u32 = 0xe06d_7363;
#[cfg(windows)]
const EXCEPTION_SEVERITY_MASK: u32 = 0xc000_0000;

#[cfg(windows)]
const CONTEXT_RAX_OFFSET: usize = 0x78;
#[cfg(windows)]
const CONTEXT_RCX_OFFSET: usize = 0x80;
#[cfg(windows)]
const CONTEXT_RDX_OFFSET: usize = 0x88;
#[cfg(windows)]
const CONTEXT_RSP_OFFSET: usize = 0x98;
#[cfg(windows)]
const CONTEXT_R8_OFFSET: usize = 0xb8;
#[cfg(windows)]
const CONTEXT_R9_OFFSET: usize = 0xc0;
#[cfg(windows)]
const CONTEXT_RIP_OFFSET: usize = 0xf8;

#[cfg(windows)]
const STACK_TRACE_FRAMES_TO_SKIP: u32 = 2;
#[cfg(windows)]
const STACK_TRACE_FRAME_COUNT: usize = 24;
#[cfg(windows)]
const STACK_SCAN_SLOTS: usize = 256;
#[cfg(windows)]
const STACK_SCAN_MAX_FRAMES: usize = 32;
#[cfg(windows)]
const STACK_RAW_QWORDS: usize = 8;
#[cfg(windows)]
const NULL_MODULE_BASE: usize = 0;
#[cfg(windows)]
const MIN_VALID_PTR: usize = 0x10000;
#[cfg(windows)]
const PE_E_LFANEW_OFFSET: usize = 0x3c;
#[cfg(windows)]
const PE_OPTIONAL_HEADER_FROM_NT: usize = 24;
#[cfg(windows)]
const PE_SIZE_OF_IMAGE_IN_OPTIONAL: usize = 0x38;
#[cfg(windows)]
const SELF_DLL_SIZE_FALLBACK: usize = 0x0400_0000;
#[cfg(windows)]
const CURRENT_PROCESS: isize = -1;

#[cfg(windows)]
static INSTALLED: std::sync::Once = std::sync::Once::new();
#[cfg(windows)]
static SELF_MODULE_BASE: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static SELF_MODULE_SIZE: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static HANDLER_HANDLE: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static PHASE: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static EXCEPTION_LINES_WRITTEN: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
const MAX_EXCEPTION_LINES: usize = 16;

#[cfg(windows)]
#[repr(C)]
struct ExceptionRecordMin {
    exception_code: u32,
    exception_flags: u32,
    next_record: *mut ExceptionRecordMin,
    exception_address: *mut c_void,
    number_parameters: u32,
    exception_information: [usize; 15],
}

#[cfg(windows)]
#[repr(C)]
struct ExceptionPointersMin {
    exception_record: *mut ExceptionRecordMin,
    context_record: *mut c_void,
}

#[cfg(windows)]
type VectoredHandler = unsafe extern "system" fn(*mut ExceptionPointersMin) -> i32;

#[cfg(windows)]
unsafe extern "system" {
    fn AddVectoredExceptionHandler(first: u32, handler: VectoredHandler) -> *mut c_void;
    fn GetCurrentThreadId() -> u32;
    fn RtlCaptureStackBackTrace(
        frames_to_skip: u32,
        frames_to_capture: u32,
        backtrace: *mut *mut c_void,
        backtrace_hash: *mut u32,
    ) -> u16;
    fn ReadProcessMemory(
        process: isize,
        base: *const c_void,
        buffer: *mut c_void,
        size: usize,
        read: *mut usize,
    ) -> i32;
}

#[cfg(windows)]
pub fn install(new_config: CrashLogConfig, self_module_base: usize) {
    let _ = CONFIG.set(new_config);
    record_self_module(self_module_base);
    INSTALLED.call_once(|| {
        mark_phase(Phase::DllAttach);
        let handle =
            unsafe { AddVectoredExceptionHandler(VECTORED_FIRST_HANDLER, crash_vectored_handler) }
                as usize;
        HANDLER_HANDLE.store(handle, Ordering::SeqCst);
        if handle != 0 {
            mark_phase(Phase::HandlerInstalled);
            append_log(format_args!(
                "crash logger installed module={} self=0x{:x}+0x{:x}",
                config().module_label,
                SELF_MODULE_BASE.load(Ordering::SeqCst),
                SELF_MODULE_SIZE.load(Ordering::SeqCst)
            ));
            write_breadcrumb("handler-installed", format_args!("handler=0x{handle:x}"));
        } else {
            append_log(format_args!(
                "crash logger AddVectoredExceptionHandler failed"
            ));
        }
    });
}

#[cfg(not(windows))]
pub fn install(config: CrashLogConfig, _self_module_base: usize) {
    let _ = CONFIG.set(config);
}

#[cfg(windows)]
fn record_self_module(base: usize) {
    if base < MIN_VALID_PTR {
        return;
    }
    SELF_MODULE_BASE.store(base, Ordering::SeqCst);
    let size = unsafe { pe_size_of_image(base) }.unwrap_or(SELF_DLL_SIZE_FALLBACK);
    SELF_MODULE_SIZE.store(size, Ordering::SeqCst);
}

#[cfg(windows)]
unsafe fn pe_size_of_image(base: usize) -> Option<usize> {
    if unsafe { safe_read_u16(base) }? != 0x5a4d {
        return None;
    }
    let e_lfanew = unsafe { safe_read_usize(base + PE_E_LFANEW_OFFSET) }? & 0xffff_ffff;
    if e_lfanew > 0x1000 {
        return None;
    }
    let nt = base + e_lfanew;
    if unsafe { safe_read_usize(nt) }? & 0xffff_ffff != 0x0000_4550 {
        return None;
    }
    let optional = nt + PE_OPTIONAL_HEADER_FROM_NT;
    unsafe { safe_read_usize(optional + PE_SIZE_OF_IMAGE_IN_OPTIONAL) }.map(|v| v & 0xffff_ffff)
}

#[cfg(windows)]
unsafe extern "system" fn crash_vectored_handler(info: *mut ExceptionPointersMin) -> i32 {
    let Some(snapshot) = (unsafe { ExceptionSnapshot::from_raw(info) }) else {
        return EXCEPTION_CONTINUE_SEARCH;
    };
    if !is_reportable_exception(snapshot.code) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    if EXCEPTION_LINES_WRITTEN.fetch_add(1, Ordering::SeqCst) >= MAX_EXCEPTION_LINES {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    mark_phase(Phase::ExceptionObserved);
    let report = exception_report(&snapshot);
    let _ = fs::write(path_for(config().latest_file_name), &report);
    append_log(format_args!("{report}\n---"));
    EXCEPTION_CONTINUE_SEARCH
}

#[cfg(windows)]
struct ExceptionSnapshot {
    code: u32,
    address: usize,
    access_kind: usize,
    access_address: usize,
    rip: usize,
    rsp: usize,
    rbp: usize,
    rax: usize,
    rcx: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
    thread_id: usize,
}

#[cfg(windows)]
impl ExceptionSnapshot {
    unsafe fn from_raw(info: *mut ExceptionPointersMin) -> Option<Self> {
        if info.is_null() {
            return None;
        }
        let pointers = unsafe { &*info };
        if pointers.exception_record.is_null() {
            return None;
        }
        let record = unsafe { &*pointers.exception_record };
        let mut out = Self {
            code: record.exception_code,
            address: record.exception_address as usize,
            access_kind: 0,
            access_address: 0,
            rip: 0,
            rsp: 0,
            rbp: 0,
            rax: 0,
            rcx: 0,
            rdx: 0,
            r8: 0,
            r9: 0,
            thread_id: unsafe { GetCurrentThreadId() as usize },
        };
        if record.number_parameters > 0 {
            out.access_kind = record.exception_information[0];
        }
        if record.number_parameters > 1 {
            out.access_address = record.exception_information[1];
        }
        if !pointers.context_record.is_null() {
            let base = pointers.context_record as *const u8;
            let read_reg = |off: usize| unsafe { *(base.add(off) as *const u64) as usize };
            out.rax = read_reg(CONTEXT_RAX_OFFSET);
            out.rcx = read_reg(CONTEXT_RCX_OFFSET);
            out.rdx = read_reg(CONTEXT_RDX_OFFSET);
            out.rsp = read_reg(CONTEXT_RSP_OFFSET);
            out.r8 = read_reg(CONTEXT_R8_OFFSET);
            out.r9 = read_reg(CONTEXT_R9_OFFSET);
            out.rip = read_reg(CONTEXT_RIP_OFFSET);
            out.rbp = 0;
        }
        Some(out)
    }
}

#[cfg(windows)]
fn exception_report(snapshot: &ExceptionSnapshot) -> String {
    let mut out = String::new();
    use fmt::Write as _;
    let modules = loaded_modules();
    let _ = writeln!(out, "reason=veh-crash-like-exception");
    let _ = writeln!(out, "module={}", config().module_label);
    let _ = writeln!(out, "exception_code=0x{:08x}", snapshot.code);
    let _ = writeln!(
        out,
        "exception_label={}",
        exception_code_label(snapshot.code)
    );
    let _ = writeln!(
        out,
        "exception_address=0x{:x}{}",
        snapshot.address,
        module_tag(snapshot.address, &modules)
    );
    let _ = writeln!(out, "exception_access_kind={}", snapshot.access_kind);
    let _ = writeln!(
        out,
        "exception_access_address=0x{:x}",
        snapshot.access_address
    );
    let _ = writeln!(
        out,
        "context_rip=0x{:x}{}",
        snapshot.rip,
        module_tag(snapshot.rip, &modules)
    );
    let _ = writeln!(out, "context_rsp=0x{:x}", snapshot.rsp);
    let _ = writeln!(out, "context_rbp=0x{:x}", snapshot.rbp);
    let _ = writeln!(out, "context_rax=0x{:x}", snapshot.rax);
    let _ = writeln!(out, "context_rcx=0x{:x}", snapshot.rcx);
    let _ = writeln!(out, "context_rdx=0x{:x}", snapshot.rdx);
    let _ = writeln!(out, "context_r8=0x{:x}", snapshot.r8);
    let _ = writeln!(out, "context_r9=0x{:x}", snapshot.r9);
    let _ = writeln!(out, "thread_id={}", snapshot.thread_id);
    write_common_fields(&mut out);
    if snapshot.code == EXCEPTION_STACK_OVERFLOW {
        let _ = writeln!(
            out,
            "stack_note=stack-overflow; skipped stack scan/backtrace"
        );
        return out;
    }
    let _ = writeln!(out, "callers={}", capture_callers(&modules));
    let _ = writeln!(
        out,
        "stack_modules={}",
        scan_stack_modules(snapshot.rsp, &modules)
    );
    let _ = writeln!(out, "stack_raw={}", scan_stack_raw(snapshot.rsp, &modules));
    out
}

#[cfg(windows)]
fn is_reportable_exception(code: u32) -> bool {
    matches!(
        code,
        EXCEPTION_ACCESS_VIOLATION
            | EXCEPTION_ILLEGAL_INSTRUCTION
            | EXCEPTION_STACK_BUFFER_OVERRUN
            | EXCEPTION_STACK_OVERFLOW
            | EXCEPTION_HEAP_CORRUPTION
            | EXCEPTION_IN_PAGE_ERROR
            | EXCEPTION_INT_DIVIDE_BY_ZERO
            | EXCEPTION_PRIVILEGED_INSTRUCTION
            | EXCEPTION_NONCONTINUABLE
            | EXCEPTION_CPP_THROW
    ) || (code & EXCEPTION_SEVERITY_MASK) == EXCEPTION_SEVERITY_MASK
}

#[cfg(windows)]
fn exception_code_label(code: u32) -> &'static str {
    match code {
        EXCEPTION_ACCESS_VIOLATION => "STATUS_ACCESS_VIOLATION",
        EXCEPTION_ILLEGAL_INSTRUCTION => "STATUS_ILLEGAL_INSTRUCTION",
        EXCEPTION_STACK_BUFFER_OVERRUN => "STATUS_STACK_BUFFER_OVERRUN / fail-fast",
        EXCEPTION_STACK_OVERFLOW => "STATUS_STACK_OVERFLOW",
        EXCEPTION_HEAP_CORRUPTION => "STATUS_HEAP_CORRUPTION",
        EXCEPTION_IN_PAGE_ERROR => "STATUS_IN_PAGE_ERROR",
        EXCEPTION_INT_DIVIDE_BY_ZERO => "STATUS_INTEGER_DIVIDE_BY_ZERO",
        EXCEPTION_PRIVILEGED_INSTRUCTION => "STATUS_PRIVILEGED_INSTRUCTION",
        EXCEPTION_NONCONTINUABLE => "STATUS_NONCONTINUABLE_EXCEPTION",
        EXCEPTION_CPP_THROW => "C++/Rust throw",
        _ => "unclassified-error-severity-exception",
    }
}

#[cfg(windows)]
fn phase_label(phase: usize) -> &'static str {
    match phase {
        0 => Phase::Uninitialized.label(),
        1 => Phase::DllAttach.label(),
        2 => Phase::HandlerInstalled.label(),
        10 => Phase::ClientReady.label(),
        90 => Phase::ExceptionObserved.label(),
        _ => "unknown",
    }
}

#[cfg(windows)]
fn capture_callers(modules: &[(usize, usize, String)]) -> String {
    let mut frames = [std::ptr::null_mut::<c_void>(); STACK_TRACE_FRAME_COUNT];
    let captured = unsafe {
        RtlCaptureStackBackTrace(
            STACK_TRACE_FRAMES_TO_SKIP,
            frames.len() as u32,
            frames.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    } as usize;
    let mut out = String::from("[");
    for (index, frame) in frames.iter().take(captured).enumerate() {
        if index != 0 {
            out.push(',');
        }
        let addr = *frame as usize;
        out.push_str(&format!(
            "#{}=0x{:x}{}",
            index,
            addr,
            module_tag(addr, modules)
        ));
    }
    out.push(']');
    out
}

#[cfg(windows)]
fn scan_stack_modules(rsp: usize, modules: &[(usize, usize, String)]) -> String {
    let mut out = String::from("[");
    if rsp < MIN_VALID_PTR {
        out.push(']');
        return out;
    }
    let mut emitted = 0usize;
    let mut last = String::new();
    for slot in 0..STACK_SCAN_SLOTS {
        if emitted >= STACK_SCAN_MAX_FRAMES {
            break;
        }
        let Some(value) = (unsafe { safe_read_usize(rsp + slot * std::mem::size_of::<usize>()) })
        else {
            continue;
        };
        if let Some((name, offset)) = module_for_addr(value, modules) {
            let frame = format!("{name}+0x{offset:x}");
            if frame != last {
                if emitted != 0 {
                    out.push(',');
                }
                out.push_str(&frame);
                emitted += 1;
                last = frame;
            }
        }
    }
    out.push(']');
    out
}

#[cfg(windows)]
fn scan_stack_raw(rsp: usize, modules: &[(usize, usize, String)]) -> String {
    let mut out = String::from("[");
    for i in 0..STACK_RAW_QWORDS {
        if i != 0 {
            out.push(',');
        }
        match unsafe { safe_read_usize(rsp + i * std::mem::size_of::<usize>()) } {
            Some(value) => out.push_str(&format!("0x{:x}{}", value, module_tag(value, modules))),
            None => out.push_str("??"),
        }
    }
    out.push(']');
    out
}

#[cfg(windows)]
fn module_tag(addr: usize, modules: &[(usize, usize, String)]) -> String {
    module_for_addr(addr, modules)
        .map(|(name, offset)| format!("{{{name}+0x{offset:x}}}"))
        .unwrap_or_default()
}

#[cfg(windows)]
fn module_for_addr(addr: usize, modules: &[(usize, usize, String)]) -> Option<(&str, usize)> {
    modules.iter().find_map(|(base, size, name)| {
        addr.checked_sub(*base)
            .filter(|offset| *offset < *size)
            .map(|offset| (name.as_str(), offset))
    })
}

#[cfg(windows)]
const PEB_LDR_OFFSET: usize = 0x18;
#[cfg(windows)]
const PEB_LDR_IN_MEMORY_ORDER_LIST_OFFSET: usize = 0x20;
#[cfg(windows)]
const LDR_ENTRY_IN_MEMORY_ORDER_LINKS_OFFSET: usize = 0x10;
#[cfg(windows)]
const LDR_ENTRY_DLL_BASE_OFFSET: usize = 0x30;
#[cfg(windows)]
const LDR_ENTRY_SIZE_OF_IMAGE_OFFSET: usize = 0x40;
#[cfg(windows)]
const LDR_ENTRY_BASE_DLL_NAME_OFFSET: usize = 0x58;
#[cfg(windows)]
const UNICODE_STRING_BUFFER_OFFSET: usize = 0x8;
#[cfg(windows)]
const MODULE_WALK_MAX: usize = 512;
#[cfg(windows)]
const MODULE_NAME_MAX_WCHARS: usize = 260;

#[cfg(all(windows, target_arch = "x86_64"))]
#[inline(always)]
fn current_peb() -> usize {
    let peb: usize;
    unsafe {
        core::arch::asm!(
            "mov {peb}, gs:[0x60]",
            peb = out(reg) peb,
            options(nostack, preserves_flags),
        );
    }
    peb
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn loaded_modules() -> Vec<(usize, usize, String)> {
    let mut modules = Vec::new();
    let peb = current_peb();
    if peb < MIN_VALID_PTR {
        return modules;
    }
    let Some(ldr) = (unsafe { safe_read_usize(peb + PEB_LDR_OFFSET) }) else {
        return modules;
    };
    let list_head = ldr + PEB_LDR_IN_MEMORY_ORDER_LIST_OFFSET;
    let Some(mut node) = (unsafe { safe_read_usize(list_head) }) else {
        return modules;
    };
    let mut count = 0usize;
    while node != list_head && node >= MIN_VALID_PTR && count < MODULE_WALK_MAX {
        let entry = node - LDR_ENTRY_IN_MEMORY_ORDER_LINKS_OFFSET;
        let base = unsafe { safe_read_usize(entry + LDR_ENTRY_DLL_BASE_OFFSET) }.unwrap_or(0);
        let size = unsafe { safe_read_usize(entry + LDR_ENTRY_SIZE_OF_IMAGE_OFFSET) }
            .map(|v| v & 0xffff_ffff)
            .unwrap_or(0);
        if base != 0 && size != 0 {
            modules.push((base, size, read_module_base_name(entry)));
        }
        let Some(next) = (unsafe { safe_read_usize(node) }) else {
            break;
        };
        if next == node {
            break;
        }
        node = next;
        count += 1;
    }
    modules
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
fn loaded_modules() -> Vec<(usize, usize, String)> {
    Vec::new()
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn read_module_base_name(entry: usize) -> String {
    let name_field = entry + LDR_ENTRY_BASE_DLL_NAME_OFFSET;
    let len_bytes = unsafe { safe_read_u16(name_field) }.unwrap_or(0) as usize;
    let Some(buffer) = (unsafe { safe_read_usize(name_field + UNICODE_STRING_BUFFER_OFFSET) })
    else {
        return String::new();
    };
    if buffer < MIN_VALID_PTR || len_bytes == 0 {
        return String::new();
    }
    let wchars = (len_bytes / std::mem::size_of::<u16>()).min(MODULE_NAME_MAX_WCHARS);
    let mut units = Vec::with_capacity(wchars);
    for index in 0..wchars {
        match unsafe { safe_read_u16(buffer + index * std::mem::size_of::<u16>()) } {
            Some(unit) => units.push(unit),
            None => break,
        }
    }
    String::from_utf16_lossy(&units)
}

#[cfg(windows)]
unsafe fn safe_read_usize(addr: usize) -> Option<usize> {
    let mut out = 0usize;
    let mut read = 0usize;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS,
            addr as *const c_void,
            (&mut out as *mut usize).cast(),
            std::mem::size_of::<usize>(),
            &mut read,
        )
    };
    (ok != 0 && read == std::mem::size_of::<usize>()).then_some(out)
}

#[cfg(windows)]
unsafe fn safe_read_u16(addr: usize) -> Option<u16> {
    let mut out = 0u16;
    let mut read = 0usize;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS,
            addr as *const c_void,
            (&mut out as *mut u16).cast(),
            std::mem::size_of::<u16>(),
            &mut read,
        )
    };
    (ok != 0 && read == std::mem::size_of::<u16>()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_standalone_ship_names() {
        let cfg = CrashLogConfig::default();
        assert_eq!(cfg.log_file_name, DEFAULT_LOG_FILE);
        assert_eq!(cfg.latest_file_name, DEFAULT_LATEST_FILE);
        assert_eq!(cfg.breadcrumb_file_name, DEFAULT_BREADCRUMB_FILE);
        assert_eq!(cfg.module_label, DEFAULT_MODULE_LABEL);
    }

    #[test]
    fn phase_labels_are_stable() {
        assert_eq!(Phase::DllAttach.label(), "dll-attach");
        assert_eq!(Phase::ExceptionObserved.label(), "exception-observed");
    }
}
