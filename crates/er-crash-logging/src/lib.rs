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

mod hang;

/// Publish the live `CS::LoadingScreenData*` for the hang watchdog's loading-screen oracle.
///
/// Re-exported because `mod hang` is private: the function is documented as the entry point for a
/// host DLL that is not `er_effects_rs.dll`, and such a host is by definition another crate.
pub use hang::publish_loading_screen_data;

const DEFAULT_LOG_FILE: &str = "er-crash-log.txt";
const DEFAULT_LATEST_FILE: &str = "er-crash-latest.txt";
const DEFAULT_BREADCRUMB_FILE: &str = "er-crash-breadcrumb-latest.txt";
const DEFAULT_MODULES_FILE: &str = "er-crash-modules.txt";
const DEFAULT_MINIDUMP_FILE: &str = "er-crash-minidump.dmp";
const DEFAULT_HANG_REPORT_FILE: &str = "er-crash-hang-latest.txt";
const DEFAULT_HANG_MINIDUMP_FILE: &str = "er-crash-hang-minidump.dmp";
const DEFAULT_MODULE_LABEL: &str = "er-crash-logging";

/// Default main-thread stall window before a hang is reported.
///
/// A frozen frame counter for this long is already far outside anything the game does on purpose;
/// the margin exists so a slow synchronous asset load on a cold disk is not mistaken for a lockup.
const DEFAULT_HANG_STALL_SECONDS: u64 = 30;

#[derive(Clone, Copy, Debug)]
pub struct CrashLogConfig {
    pub log_file_name: &'static str,
    pub latest_file_name: &'static str,
    pub breadcrumb_file_name: &'static str,
    /// Full loaded-module inventory: what was in the process, where, and which build. Written at
    /// install and rewritten when the process dies, so an offset like `nvwgf2umx.dll+0xead0c0`
    /// can be tied back to an exact driver image later.
    pub modules_file_name: &'static str,
    /// Postmortem minidump, written only from the unhandled-exception path.
    pub minidump_file_name: &'static str,
    /// Snapshot of every thread taken when the main thread stops ticking frames.
    pub hang_report_file_name: &'static str,
    /// Minidump taken alongside that snapshot, while the process is still frozen and readable.
    pub hang_minidump_file_name: &'static str,
    /// Main-thread stall window, in seconds, before a hang is reported. 0 disables the watchdog.
    ///
    /// A hang raises no exception, so without this the crash log stays empty through the entire
    /// freeze and the only record is whatever the exit path crashes on afterwards.
    pub hang_stall_seconds: u64,
    pub module_label: &'static str,
}

impl Default for CrashLogConfig {
    fn default() -> Self {
        Self {
            log_file_name: DEFAULT_LOG_FILE,
            latest_file_name: DEFAULT_LATEST_FILE,
            breadcrumb_file_name: DEFAULT_BREADCRUMB_FILE,
            modules_file_name: DEFAULT_MODULES_FILE,
            minidump_file_name: DEFAULT_MINIDUMP_FILE,
            hang_report_file_name: DEFAULT_HANG_REPORT_FILE,
            hang_minidump_file_name: DEFAULT_HANG_MINIDUMP_FILE,
            hang_stall_seconds: DEFAULT_HANG_STALL_SECONDS,
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

// `phase_label` is the only caller and it is windows-only; the test module pins the strings.
#[cfg(any(windows, test))]
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

pub(crate) fn config() -> CrashLogConfig {
    *CONFIG.get_or_init(CrashLogConfig::default)
}

pub(crate) fn path_for(name: &str) -> PathBuf {
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

// Exception codes and the record policy built on them are plain arithmetic, so they stay off
// `cfg(windows)`: the classification is the part most worth testing, and the tests run on the
// host toolchain.
const EXCEPTION_ACCESS_VIOLATION: u32 = 0xc000_0005;
const EXCEPTION_ILLEGAL_INSTRUCTION: u32 = 0xc000_001d;
const EXCEPTION_STACK_BUFFER_OVERRUN: u32 = 0xc000_0409;
const EXCEPTION_STACK_OVERFLOW: u32 = 0xc000_00fd;
const EXCEPTION_HEAP_CORRUPTION: u32 = 0xc000_0374;
const EXCEPTION_IN_PAGE_ERROR: u32 = 0xc000_0006;
const EXCEPTION_INT_DIVIDE_BY_ZERO: u32 = 0xc000_0094;
const EXCEPTION_PRIVILEGED_INSTRUCTION: u32 = 0xc000_0096;
const EXCEPTION_NONCONTINUABLE: u32 = 0xc000_0025;
const EXCEPTION_CPP_THROW: u32 = 0xe06d_7363;
const EXCEPTION_SEVERITY_MASK: u32 = 0xc000_0000;

/// Which budget an observed exception draws from.
///
/// A first-chance vectored handler sees every `0xe06d7363` the process raises, and D3D plus the
/// vendor user-mode drivers throw and catch C++ exceptions as ordinary control flow. Sharing one
/// budget between those and real faults means a noisy render thread can exhaust the log before the
/// access violation that actually killed the process ever gets written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RecordClass {
    /// `0xe06d7363` -- a C++/Rust throw. Usually caught by somebody; only interesting in bulk or
    /// when it turns out to be the one that went unhandled.
    Throw,
    /// A hard fault: access violation, heap corruption, fail-fast, and friends.
    Fault,
}

/// Throw records are capped low; they are noise until proven otherwise.
const MAX_THROW_RECORDS: usize = 8;
/// Faults get their own, larger budget that throws cannot touch.
const MAX_FAULT_RECORDS: usize = 24;
/// Distinct `(code, address)` sites tracked for repeat counting.
#[cfg(windows)]
const THROW_SITE_SLOTS: usize = 24;

pub fn record_class(code: u32) -> RecordClass {
    if code == EXCEPTION_CPP_THROW {
        RecordClass::Throw
    } else {
        RecordClass::Fault
    }
}

/// Whether a record of `class` may still be written, given how many of each class already were.
pub fn budget_allows(class: RecordClass, throws_written: usize, faults_written: usize) -> bool {
    match class {
        RecordClass::Throw => throws_written < MAX_THROW_RECORDS,
        RecordClass::Fault => faults_written < MAX_FAULT_RECORDS,
    }
}

/// Turn an MSVC RTTI `TypeDescriptor` name into something readable.
///
/// `.?AVbad_alloc@std@@` -> `std::bad_alloc`, `.?AV_com_error@@` -> `_com_error`. Anything that
/// does not match the expected decoration is returned unchanged rather than mangled further --
/// a raw name we cannot parse is still evidence.
pub fn format_thrown_type_name(raw: &str) -> String {
    let Some(body) = raw.strip_prefix('.') else {
        return raw.to_string();
    };
    let body = ["?AV", "?AU", "PEAV", "PEAU", "AV", "AU"]
        .iter()
        .find_map(|prefix| body.strip_prefix(prefix))
        .unwrap_or(body);
    let Some(body) = body.strip_suffix("@@") else {
        return raw.to_string();
    };
    if body.is_empty() {
        return raw.to_string();
    }
    // MSVC stores the qualified name innermost-first, so `bad_alloc@std` is `std::bad_alloc`.
    body.split('@').rev().collect::<Vec<_>>().join("::")
}

#[cfg(windows)]
const CONTEXT_RAX_OFFSET: usize = 0x78;
#[cfg(windows)]
const CONTEXT_RCX_OFFSET: usize = 0x80;
#[cfg(windows)]
const CONTEXT_RDX_OFFSET: usize = 0x88;
#[cfg(any(windows, test))]
pub(crate) const CONTEXT_RSP_OFFSET: usize = 0x98;
#[cfg(windows)]
const CONTEXT_R8_OFFSET: usize = 0xb8;
#[cfg(windows)]
const CONTEXT_R9_OFFSET: usize = 0xc0;
#[cfg(any(windows, test))]
pub(crate) const CONTEXT_RIP_OFFSET: usize = 0xf8;

// MSVC C++ EH payload, as raised by `_CxxThrowException`. `ExceptionInformation` is
// `[magic, thrown object, ThrowInfo, module base]`; on x64 every pointer inside `ThrowInfo` is a
// 32-bit RVA relative to that module base.
#[cfg(windows)]
const CPP_EH_PARAMS_KEPT: usize = 4;
#[cfg(windows)]
const CPP_EH_MAGIC: usize = 0x1993_0520;
#[cfg(windows)]
const CPP_EH_PARAM_OBJECT: usize = 1;
#[cfg(windows)]
const CPP_EH_PARAM_THROW_INFO: usize = 2;
#[cfg(windows)]
const CPP_EH_PARAM_MODULE_BASE: usize = 3;
#[cfg(windows)]
const THROW_INFO_CATCHABLE_ARRAY_OFFSET: usize = 0x0c;
#[cfg(windows)]
const CATCHABLE_ARRAY_FIRST_TYPE_OFFSET: usize = 0x04;
#[cfg(windows)]
const CATCHABLE_TYPE_DESCRIPTOR_OFFSET: usize = 0x04;
#[cfg(windows)]
const TYPE_DESCRIPTOR_NAME_OFFSET: usize = 0x10;
#[cfg(windows)]
const TYPE_DESCRIPTOR_NAME_MAX: usize = 96;
#[cfg(windows)]
const THROWN_OBJECT_PREVIEW_BYTES: usize = 32;

// Minidump: indirectly-referenced memory plus thread info keeps the dump small while still
// carrying the stacks and the objects those stacks point at.
#[cfg(windows)]
const MINIDUMP_NORMAL: u32 = 0x0000_0000;
#[cfg(windows)]
const MINIDUMP_WITH_UNLOADED_MODULES: u32 = 0x0000_0020;
#[cfg(windows)]
const MINIDUMP_WITH_INDIRECTLY_REFERENCED_MEMORY: u32 = 0x0000_0040;
#[cfg(windows)]
const MINIDUMP_WITH_PROCESS_THREAD_DATA: u32 = 0x0000_0100;
#[cfg(windows)]
const MINIDUMP_WITH_THREAD_INFO: u32 = 0x0000_1000;
#[cfg(windows)]
const GENERIC_WRITE: u32 = 0x4000_0000;
#[cfg(windows)]
const CREATE_ALWAYS: u32 = 2;
#[cfg(windows)]
const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
#[cfg(windows)]
const INVALID_HANDLE_VALUE: isize = -1;
#[cfg(windows)]
const PE_TIMEDATESTAMP_FROM_NT: usize = 0x08;

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
pub(crate) const MIN_VALID_PTR: usize = 0x10000;
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
static THROW_RECORDS_WRITTEN: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static FAULT_RECORDS_WRITTEN: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static RECORD_INDEX: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static PREVIOUS_UNHANDLED_FILTER: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static FATAL_REPORTED: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static INSTALL_INSTANT: OnceLock<std::time::Instant> = OnceLock::new();

/// One tracked `(code, address)` exception site and how often it has been seen.
///
/// The count is the difference between background noise and a smoking gun: a throw site at its
/// 400th occurrence is how the driver works, one at its 1st occurrence right before the process
/// dies is the lead.
#[cfg(windows)]
struct ThrowSite {
    code: AtomicUsize,
    address: AtomicUsize,
    count: AtomicUsize,
}

#[cfg(windows)]
static THROW_SITES: [ThrowSite; THROW_SITE_SLOTS] = [const {
    ThrowSite {
        code: AtomicUsize::new(0),
        address: AtomicUsize::new(0),
        count: AtomicUsize::new(0),
    }
}; THROW_SITE_SLOTS];

/// Count this `(code, address)` and return how many times it has now been seen (1 on first sight).
/// Returns 0 once the table is full, meaning "unknown, not tracked".
#[cfg(windows)]
fn note_exception_site(code: u32, address: usize) -> usize {
    let code = code as usize;
    for site in THROW_SITES.iter() {
        let claimed = site.address.load(Ordering::SeqCst);
        if claimed == address && site.code.load(Ordering::SeqCst) == code {
            return site.count.fetch_add(1, Ordering::SeqCst) + 1;
        }
        if claimed == 0
            && site
                .address
                .compare_exchange(0, address, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            site.code.store(code, Ordering::SeqCst);
            return site.count.fetch_add(1, Ordering::SeqCst) + 1;
        }
    }
    0
}

/// How many times this `(code, address)` has been seen, without counting this look.
///
/// The fatal filter re-observes the exception the vectored handler already counted; counting it
/// twice would report every fatal site as one occurrence more common than it really is.
#[cfg(windows)]
fn current_site_count(code: u32, address: usize) -> usize {
    let code = code as usize;
    THROW_SITES
        .iter()
        .find(|site| {
            site.address.load(Ordering::SeqCst) == address
                && site.code.load(Ordering::SeqCst) == code
        })
        .map(|site| site.count.load(Ordering::SeqCst))
        .unwrap_or(0)
}

/// Every tracked site with its repeat count, most-repeated first.
#[cfg(windows)]
fn exception_site_histogram(modules: &[(usize, usize, String)]) -> String {
    let mut sites: Vec<(usize, u32, usize)> = THROW_SITES
        .iter()
        .filter_map(|site| {
            let address = site.address.load(Ordering::SeqCst);
            (address != 0).then(|| {
                (
                    site.count.load(Ordering::SeqCst),
                    site.code.load(Ordering::SeqCst) as u32,
                    address,
                )
            })
        })
        .collect();
    sites.sort_by_key(|site| std::cmp::Reverse(site.0));
    let mut out = String::from("[");
    for (index, (count, code, address)) in sites.iter().enumerate() {
        if index != 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "0x{:08x}@0x{:x}{}x{}",
            code,
            address,
            module_tag(*address, modules),
            count
        ));
    }
    out.push(']');
    out
}

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
type UnhandledExceptionFilter = unsafe extern "system" fn(*mut ExceptionPointersMin) -> i32;

#[cfg(windows)]
#[repr(C)]
struct SystemTimeMin {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    milliseconds: u16,
}

#[cfg(windows)]
unsafe extern "system" {
    fn AddVectoredExceptionHandler(first: u32, handler: VectoredHandler) -> *mut c_void;
    fn SetUnhandledExceptionFilter(filter: UnhandledExceptionFilter) -> *mut c_void;
    fn GetSystemTime(out: *mut SystemTimeMin);
    fn LoadLibraryA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn GetCurrentProcess() -> isize;
    fn GetCurrentProcessId() -> u32;
    fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *mut c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: isize,
    ) -> isize;
    fn CloseHandle(handle: isize) -> i32;
    fn GetCurrentThreadId() -> u32;
    fn GetLastError() -> u32;
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
        let _ = INSTALL_INSTANT.set(std::time::Instant::now());
        mark_phase(Phase::DllAttach);
        let handle =
            unsafe { AddVectoredExceptionHandler(VECTORED_FIRST_HANDLER, crash_vectored_handler) }
                as usize;
        HANDLER_HANDLE.store(handle, Ordering::SeqCst);
        // The vectored handler only ever sees first-chance exceptions, so on its own it cannot say
        // whether anything it logged was fatal. The top-level filter is what closes that gap: it
        // runs only when nothing handled the exception. Chain whatever was installed before us --
        // Seamless Co-op's crashpad lives here too, and silently swallowing it would trade one
        // blind spot for another.
        let previous = unsafe { SetUnhandledExceptionFilter(unhandled_exception_filter) } as usize;
        PREVIOUS_UNHANDLED_FILTER.store(previous, Ordering::SeqCst);
        if handle != 0 {
            mark_phase(Phase::HandlerInstalled);
            append_log(format_args!(
                "crash logger installed module={} self=0x{:x}+0x{:x} previous_unhandled_filter=0x{:x}",
                config().module_label,
                SELF_MODULE_BASE.load(Ordering::SeqCst),
                SELF_MODULE_SIZE.load(Ordering::SeqCst),
                previous
            ));
            write_breadcrumb("handler-installed", format_args!("handler=0x{handle:x}"));
        } else {
            append_log(format_args!(
                "crash logger AddVectoredExceptionHandler failed"
            ));
        }
        write_module_inventory("install");
        // A hang raises no exception, so the handlers above are blind to it. The watchdog is the
        // only path that produces evidence when the game stops rather than crashes.
        hang::start(config().hang_stall_seconds);
    });
}

/// Note that the host DLL is detaching.
///
/// A fatal record with no matching detach means the process died where it stood; a detach with no
/// fatal record means it shut down or was killed from outside. Neither is knowable from the crash
/// log alone.
#[cfg(windows)]
pub fn note_process_detach() {
    write_breadcrumb(
        "process-detach",
        format_args!(
            "throw_records={} fault_records={} fatal_reported={}",
            THROW_RECORDS_WRITTEN.load(Ordering::SeqCst),
            FAULT_RECORDS_WRITTEN.load(Ordering::SeqCst),
            FATAL_REPORTED.load(Ordering::SeqCst)
        ),
    );
}

#[cfg(not(windows))]
pub fn note_process_detach() {}

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
    // Count every occurrence, even ones too far over budget to write: the repeat count is what
    // separates "this driver throws constantly" from "this happened once, just before the end".
    let repeat = note_exception_site(snapshot.code, snapshot.address);
    let class = record_class(snapshot.code);
    let allowed = budget_allows(
        class,
        THROW_RECORDS_WRITTEN.load(Ordering::SeqCst),
        FAULT_RECORDS_WRITTEN.load(Ordering::SeqCst),
    );
    if !allowed {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    match class {
        RecordClass::Throw => THROW_RECORDS_WRITTEN.fetch_add(1, Ordering::SeqCst),
        RecordClass::Fault => FAULT_RECORDS_WRITTEN.fetch_add(1, Ordering::SeqCst),
    };
    mark_phase(Phase::ExceptionObserved);
    let report = exception_report(&snapshot, "veh-first-chance-exception", repeat);
    // A fatal record is the conclusion of the run; never let a later benign throw overwrite it.
    if FATAL_REPORTED.load(Ordering::SeqCst) == 0 {
        let _ = fs::write(path_for(config().latest_file_name), &report);
    }
    append_log(format_args!("{report}\n---"));
    EXCEPTION_CONTINUE_SEARCH
}

/// Top-level filter: reached only when no handler anywhere in the process claimed the exception.
///
/// This is the record that says "this one was fatal" -- the distinction a first-chance handler
/// structurally cannot make.
#[cfg(windows)]
unsafe extern "system" fn unhandled_exception_filter(info: *mut ExceptionPointersMin) -> i32 {
    // If reporting the fatal exception itself faults, do not recurse into reporting again -- chain
    // straight on so the next filter still gets its turn.
    let first_entry = FATAL_REPORTED
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    if !first_entry {
        return chain_previous_unhandled_filter(info);
    }
    if let Some(snapshot) = unsafe { ExceptionSnapshot::from_raw(info) } {
        let repeat = current_site_count(snapshot.code, snapshot.address);
        let report = exception_report(&snapshot, "unhandled-exception-fatal", repeat);
        let _ = fs::write(path_for(config().latest_file_name), &report);
        append_log(format_args!("{report}\n---"));
    }
    write_module_inventory("fatal");
    unsafe { write_minidump_named(config().minidump_file_name, info) };
    chain_previous_unhandled_filter(info)
}

/// Hand the exception on exactly as we found it.
///
/// Whatever was registered before us -- Seamless Co-op's crashpad, the game's own filter, Windows
/// Error Reporting -- still gets its turn. We observe; we do not consume.
#[cfg(windows)]
fn chain_previous_unhandled_filter(info: *mut ExceptionPointersMin) -> i32 {
    let previous = PREVIOUS_UNHANDLED_FILTER.load(Ordering::SeqCst);
    if previous != 0 {
        let chained: UnhandledExceptionFilter = unsafe { std::mem::transmute(previous) };
        return unsafe { chained(info) };
    }
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
    /// The leading `ExceptionInformation` entries, kept verbatim. For `0xe06d7363` these carry the
    /// C++ EH magic, the thrown object, the `ThrowInfo`, and the module base its RVAs resolve
    /// against -- everything needed to name the thrown type.
    params: [usize; CPP_EH_PARAMS_KEPT],
    param_count: usize,
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
            params: [0; CPP_EH_PARAMS_KEPT],
            param_count: record.number_parameters as usize,
        };
        for (index, slot) in out.params.iter_mut().enumerate() {
            if index < out.param_count {
                *slot = record.exception_information[index];
            }
        }
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
fn exception_report(snapshot: &ExceptionSnapshot, reason: &str, repeat: usize) -> String {
    let mut out = String::new();
    use fmt::Write as _;
    let modules = loaded_modules();
    let class = record_class(snapshot.code);
    let _ = writeln!(out, "reason={reason}");
    let _ = writeln!(out, "module={}", config().module_label);
    let _ = writeln!(
        out,
        "record_index={}",
        RECORD_INDEX.fetch_add(1, Ordering::SeqCst)
    );
    let _ = writeln!(out, "utc={}", utc_timestamp());
    let _ = writeln!(out, "ms_since_install={}", ms_since_install());
    let _ = writeln!(
        out,
        "record_class={}",
        match class {
            RecordClass::Throw => "throw",
            RecordClass::Fault => "fault",
        }
    );
    // The one question a first-chance record cannot answer about itself.
    let _ = writeln!(out, "fatal={}", reason == "unhandled-exception-fatal");
    let _ = writeln!(out, "site_repeat_count={repeat}");
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
    if snapshot.code == EXCEPTION_CPP_THROW {
        write_cpp_throw_fields(&mut out, snapshot, &modules);
    }
    if snapshot.code == EXCEPTION_STACK_OVERFLOW {
        // Everything below here formats into deeper stack; on an overflow there is none to spend.
        let _ = writeln!(
            out,
            "stack_note=stack-overflow; skipped stack scan/backtrace"
        );
        return out;
    }
    let _ = writeln!(out, "site_histogram={}", exception_site_histogram(&modules));
    let _ = writeln!(out, "callers={}", capture_callers(&modules));
    let _ = writeln!(
        out,
        "stack_modules={}",
        scan_stack_modules(snapshot.rsp, &modules)
    );
    let _ = writeln!(out, "stack_raw={}", scan_stack_raw(snapshot.rsp, &modules));
    out
}

pub fn is_reportable_exception(code: u32) -> bool {
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

pub fn exception_code_label(code: u32) -> &'static str {
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

/// Name the thrown C++ type and preview the thrown object.
///
/// Without this a `0xe06d7363` record says only "somebody threw something", which is why the
/// August 2026 render-thread throw could not be attributed. With it the record names the type --
/// `_com_error`, `std::bad_alloc`, a driver-internal class -- and that name is the lead.
#[cfg(windows)]
fn write_cpp_throw_fields(
    out: &mut String,
    snapshot: &ExceptionSnapshot,
    modules: &[(usize, usize, String)],
) {
    use fmt::Write as _;
    if snapshot.param_count < CPP_EH_PARAMS_KEPT || snapshot.params[0] != CPP_EH_MAGIC {
        let _ = writeln!(
            out,
            "cpp_throw_note=not-a-decodable-msvc-throw params={} magic=0x{:x}",
            snapshot.param_count, snapshot.params[0]
        );
        return;
    }
    let object = snapshot.params[CPP_EH_PARAM_OBJECT];
    let throw_info = snapshot.params[CPP_EH_PARAM_THROW_INFO];
    let image_base = snapshot.params[CPP_EH_PARAM_MODULE_BASE];
    let _ = writeln!(
        out,
        "cpp_throw_object=0x{:x}{}",
        object,
        module_tag(object, modules)
    );
    let _ = writeln!(
        out,
        "cpp_throw_info=0x{:x}{}",
        throw_info,
        module_tag(throw_info, modules)
    );
    let _ = writeln!(
        out,
        "cpp_throw_image_base=0x{:x}{}",
        image_base,
        module_tag(image_base, modules)
    );
    match unsafe { read_thrown_type_name(throw_info, image_base) } {
        Some(raw) => {
            let _ = writeln!(out, "cpp_throw_type={}", format_thrown_type_name(&raw));
            let _ = writeln!(out, "cpp_throw_type_raw={raw}");
        }
        None => {
            let _ = writeln!(out, "cpp_throw_type=<unresolved>");
        }
    }
    let _ = writeln!(out, "cpp_throw_object_bytes={}", unsafe {
        read_hex_preview(object, THROWN_OBJECT_PREVIEW_BYTES)
    });
}

/// Walk `ThrowInfo -> CatchableTypeArray -> CatchableType -> TypeDescriptor` and read the name.
///
/// Only the first catchable type is read: for `throw Derived` that is `Derived` itself, the rest
/// being the base classes it could also be caught as.
#[cfg(windows)]
unsafe fn read_thrown_type_name(throw_info: usize, image_base: usize) -> Option<String> {
    if throw_info < MIN_VALID_PTR || image_base < MIN_VALID_PTR {
        return None;
    }
    let array_rva = unsafe { safe_read_u32(throw_info + THROW_INFO_CATCHABLE_ARRAY_OFFSET) }?;
    if array_rva == 0 {
        return None;
    }
    let array = image_base + array_rva as usize;
    let count = unsafe { safe_read_u32(array) }?;
    if count == 0 {
        return None;
    }
    let first_rva = unsafe { safe_read_u32(array + CATCHABLE_ARRAY_FIRST_TYPE_OFFSET) }?;
    if first_rva == 0 {
        return None;
    }
    let catchable = image_base + first_rva as usize;
    let descriptor_rva = unsafe { safe_read_u32(catchable + CATCHABLE_TYPE_DESCRIPTOR_OFFSET) }?;
    if descriptor_rva == 0 {
        return None;
    }
    let name_addr = image_base + descriptor_rva as usize + TYPE_DESCRIPTOR_NAME_OFFSET;
    let mut bytes = Vec::with_capacity(TYPE_DESCRIPTOR_NAME_MAX);
    for index in 0..TYPE_DESCRIPTOR_NAME_MAX {
        match unsafe { safe_read_u8(name_addr + index) } {
            Some(0) | None => break,
            Some(byte) => bytes.push(byte),
        }
    }
    if bytes.is_empty() {
        return None;
    }
    // RTTI type names are ASCII by construction; anything else means we walked off target and the
    // raw bytes are more honest than a substituted replacement character.
    String::from_utf8(bytes).ok()
}

#[cfg(windows)]
unsafe fn read_hex_preview(addr: usize, len: usize) -> String {
    if addr < MIN_VALID_PTR {
        return String::from("<null>");
    }
    let mut out = String::with_capacity(len * 2);
    for index in 0..len {
        match unsafe { safe_read_u8(addr + index) } {
            Some(byte) => {
                let _ = fmt::Write::write_fmt(&mut out, format_args!("{byte:02x}"));
            }
            None => {
                out.push_str("??");
                break;
            }
        }
    }
    out
}

#[cfg(windows)]
pub(crate) fn utc_timestamp() -> String {
    let mut now = SystemTimeMin {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        milliseconds: 0,
    };
    unsafe { GetSystemTime(&mut now) };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        now.year, now.month, now.day, now.hour, now.minute, now.second, now.milliseconds
    )
}

#[cfg(windows)]
pub(crate) fn ms_since_install() -> u64 {
    INSTALL_INSTANT
        .get()
        .map(|start| start.elapsed().as_millis() as u64)
        .unwrap_or(0)
}

/// Write the full loaded-module table: name, base, size, and PE timestamp.
///
/// The PE `TimeDateStamp` identifies the exact build of each image, so a third-party offset in a
/// crash record stays resolvable even after the driver or overlay updates.
#[cfg(windows)]
fn write_module_inventory(reason: &str) {
    use fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "reason=module-inventory-{reason}");
    let _ = writeln!(out, "module={}", config().module_label);
    let _ = writeln!(out, "utc={}", utc_timestamp());
    let _ = writeln!(out, "ms_since_install={}", ms_since_install());
    let modules = loaded_modules();
    let _ = writeln!(out, "module_count={}", modules.len());
    for (base, size, name) in &modules {
        let stamp = unsafe { pe_timedatestamp(*base) }.unwrap_or(0);
        let _ = writeln!(
            out,
            "module_entry base=0x{base:x} size=0x{size:x} pe_timestamp=0x{stamp:08x} name={name}"
        );
    }
    let _ = fs::write(path_for(config().modules_file_name), out);
}

#[cfg(windows)]
unsafe fn pe_timedatestamp(base: usize) -> Option<u32> {
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
    unsafe { safe_read_u32(nt + PE_TIMEDATESTAMP_FROM_NT) }
}

/// Write a postmortem minidump next to the text log.
///
/// The text record is a summary of one thread; the dump carries every thread's stack, the full
/// module list with versions, and the memory those stacks reference. `dbghelp` is resolved lazily
/// so nothing is linked or loaded until the process is already dying.
#[cfg(windows)]
pub(crate) unsafe fn write_minidump_named(file_name: &str, info: *mut ExceptionPointersMin) {
    #[repr(C)]
    struct MinidumpExceptionInformation {
        thread_id: u32,
        exception_pointers: *mut ExceptionPointersMin,
        client_pointers: i32,
    }
    type MiniDumpWriteDumpFn = unsafe extern "system" fn(
        process: isize,
        process_id: u32,
        file: isize,
        dump_type: u32,
        exception_param: *const MinidumpExceptionInformation,
        user_stream_param: *const c_void,
        callback_param: *const c_void,
    ) -> i32;

    let dbghelp = unsafe { LoadLibraryA(c"dbghelp.dll".as_ptr().cast()) };
    if dbghelp.is_null() {
        return;
    }
    let entry = unsafe { GetProcAddress(dbghelp, c"MiniDumpWriteDump".as_ptr().cast()) };
    if entry.is_null() {
        return;
    }
    let write_dump: MiniDumpWriteDumpFn = unsafe { std::mem::transmute(entry) };

    let path = path_for(file_name);
    let mut wide: Vec<u16> = path.to_string_lossy().encode_utf16().collect();
    wide.push(0);
    let file = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            0,
            std::ptr::null_mut(),
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            0,
        )
    };
    if file == INVALID_HANDLE_VALUE {
        append_log(format_args!(
            "minidump create failed last_error={} path={}",
            unsafe { GetLastError() },
            path.display()
        ));
        return;
    }
    // A null `info` means nobody faulted -- the hang path takes a dump of a live, frozen process.
    // `MiniDumpWriteDump` accepts a null exception parameter and captures every thread anyway.
    let exception = MinidumpExceptionInformation {
        thread_id: unsafe { GetCurrentThreadId() },
        exception_pointers: info,
        client_pointers: 0,
    };
    let exception_param = if info.is_null() {
        std::ptr::null()
    } else {
        &exception as *const MinidumpExceptionInformation
    };
    let rich_type = MINIDUMP_WITH_UNLOADED_MODULES
        | MINIDUMP_WITH_INDIRECTLY_REFERENCED_MEMORY
        | MINIDUMP_WITH_PROCESS_THREAD_DATA
        | MINIDUMP_WITH_THREAD_INFO;
    let mut dump_type = rich_type;
    let mut ok = unsafe {
        write_dump(
            GetCurrentProcess(),
            GetCurrentProcessId(),
            file,
            dump_type,
            exception_param,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    let rich_error = unsafe { GetLastError() };
    if ok == 0 {
        // The rich flags walk far more of the process than the minimal dump does, and any one of
        // them can be refused -- by a partial `dbghelp` implementation, or by a process too far
        // gone to enumerate. A bare thread-and-module dump is worth vastly more than no dump, so
        // never let the expensive attempt be the only one.
        dump_type = MINIDUMP_NORMAL;
        ok = unsafe {
            write_dump(
                GetCurrentProcess(),
                GetCurrentProcessId(),
                file,
                dump_type,
                exception_param,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
    }
    let final_error = unsafe { GetLastError() };
    unsafe { CloseHandle(file) };
    append_log(format_args!(
        "minidump write ok={} type=0x{:x} rich_last_error={} last_error={} path={}",
        ok != 0,
        dump_type,
        rich_error,
        final_error,
        path.display()
    ));
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

#[cfg(any(windows, test))]
pub(crate) fn module_tag(addr: usize, modules: &[(usize, usize, String)]) -> String {
    module_for_addr(addr, modules)
        .map(|(name, offset)| format!("{{{name}+0x{offset:x}}}"))
        .unwrap_or_default()
}

#[cfg(any(windows, test))]
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
pub(crate) fn loaded_modules() -> Vec<(usize, usize, String)> {
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

#[cfg(all(windows, not(target_arch = "x86_64")))]
pub(crate) fn loaded_modules() -> Vec<(usize, usize, String)> {
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
pub(crate) unsafe fn safe_read_usize(addr: usize) -> Option<usize> {
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
pub(crate) unsafe fn safe_read_u32(addr: usize) -> Option<u32> {
    let mut out = 0u32;
    let mut read = 0usize;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS,
            addr as *const c_void,
            (&mut out as *mut u32).cast(),
            std::mem::size_of::<u32>(),
            &mut read,
        )
    };
    (ok != 0 && read == std::mem::size_of::<u32>()).then_some(out)
}

#[cfg(windows)]
unsafe fn safe_read_u8(addr: usize) -> Option<u8> {
    let mut out = 0u8;
    let mut read = 0usize;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS,
            addr as *const c_void,
            (&mut out as *mut u8).cast(),
            std::mem::size_of::<u8>(),
            &mut read,
        )
    };
    (ok != 0 && read == std::mem::size_of::<u8>()).then_some(out)
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
        assert_eq!(cfg.modules_file_name, DEFAULT_MODULES_FILE);
        assert_eq!(cfg.minidump_file_name, DEFAULT_MINIDUMP_FILE);
        assert_eq!(cfg.module_label, DEFAULT_MODULE_LABEL);
    }

    #[test]
    fn phase_labels_are_stable() {
        assert_eq!(Phase::DllAttach.label(), "dll-attach");
        assert_eq!(Phase::ExceptionObserved.label(), "exception-observed");
    }

    #[test]
    fn cpp_throws_are_classified_apart_from_faults() {
        assert_eq!(record_class(EXCEPTION_CPP_THROW), RecordClass::Throw);
        assert_eq!(record_class(EXCEPTION_ACCESS_VIOLATION), RecordClass::Fault);
        assert_eq!(record_class(EXCEPTION_HEAP_CORRUPTION), RecordClass::Fault);
    }

    /// The failure this whole change exists to prevent: a render thread throwing C++ exceptions as
    /// routine control flow must not be able to consume the records reserved for a real fault.
    #[test]
    fn exhausted_throw_budget_leaves_fault_budget_intact() {
        let throws = MAX_THROW_RECORDS * 100;
        assert!(!budget_allows(RecordClass::Throw, throws, 0));
        assert!(budget_allows(RecordClass::Fault, throws, 0));
        assert!(budget_allows(
            RecordClass::Fault,
            throws,
            MAX_FAULT_RECORDS - 1
        ));
        assert!(!budget_allows(
            RecordClass::Fault,
            throws,
            MAX_FAULT_RECORDS
        ));
    }

    #[test]
    fn thrown_type_names_are_readable() {
        assert_eq!(format_thrown_type_name(".?AV_com_error@@"), "_com_error");
        assert_eq!(
            format_thrown_type_name(".?AVbad_alloc@std@@"),
            "std::bad_alloc"
        );
        assert_eq!(
            format_thrown_type_name(".PEAVDeviceRemoved@nv@@"),
            "nv::DeviceRemoved"
        );
    }

    /// An unparseable name is still evidence; never mangle it further.
    #[test]
    fn unrecognized_type_names_pass_through_untouched() {
        assert_eq!(format_thrown_type_name("garbage"), "garbage");
        assert_eq!(
            format_thrown_type_name(".?AVunterminated"),
            ".?AVunterminated"
        );
        assert_eq!(format_thrown_type_name(".?AV@@"), ".?AV@@");
    }

    #[test]
    fn reportable_set_covers_cpp_throw_and_error_severity() {
        assert!(is_reportable_exception(EXCEPTION_CPP_THROW));
        assert!(is_reportable_exception(EXCEPTION_ACCESS_VIOLATION));
        // Error-severity codes we have not enumerated still report.
        assert!(is_reportable_exception(0xc000_0fff));
        // Informational/warning severities do not.
        assert!(!is_reportable_exception(0x4000_0001));
    }

    #[test]
    fn cpp_throw_has_a_distinct_label() {
        assert_eq!(exception_code_label(EXCEPTION_CPP_THROW), "C++/Rust throw");
        assert_eq!(
            exception_code_label(EXCEPTION_ACCESS_VIOLATION),
            "STATUS_ACCESS_VIOLATION"
        );
    }
}
