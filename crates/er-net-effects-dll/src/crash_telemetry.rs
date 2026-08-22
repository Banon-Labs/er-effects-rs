use std::{
    fmt::Write as FmtWrite,
    fs,
    io::Write as IoWrite,
    sync::{
        OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Instant,
};

use windows::Win32::{
    Foundation::HINSTANCE,
    System::{
        Diagnostics::Debug::{
            AddVectoredExceptionHandler, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
        },
        Threading::GetCurrentThreadId,
    },
};

use crate::log::net_effects_log;

const CRASH_LATEST_PATH: &str = "er-net-effects-crash-telemetry-latest.txt";
const CRASH_LOG_PATH: &str = "er-net-effects-crash-telemetry.log";
const BREADCRUMB_PATH: &str = "er-net-effects-breadcrumb-latest.txt";
const SNAPSHOT_INTERVAL_MS: u64 = 250;
const EXCEPTION_ACCESS_VIOLATION: u32 = 0xc000_0005;
const EXCEPTION_ILLEGAL_INSTRUCTION: u32 = 0xc000_001d;
const EXCEPTION_STACK_BUFFER_OVERRUN: u32 = 0xc000_0409;
const EXCEPTION_STACK_OVERFLOW: u32 = 0xc000_00fd;

#[repr(u32)]
#[derive(Clone, Copy)]
pub(crate) enum Phase {
    DllAttach = 1,
    HandlerInstalled = 2,
    RuntimeSuspended = 10,
    RuntimeReady = 11,
    // Ids 20..32 were the hand-rolled Present/D3D12-draw overlay path, replaced by hudhook.
    // `phase_label` still decodes them so crash logs from those builds stay readable.
    HudhookApplyOk = 40,
    HudhookApplyFailed = 41,
    HudhookInitialize = 42,
    HudhookRenderEnter = 43,
    HudhookRenderVisible = 44,
    HudhookRenderExit = 45,
    ExceptionObserved = 90,
}

static MODULE_BASE: AtomicUsize = AtomicUsize::new(0);
static MODULE_SIZE: AtomicUsize = AtomicUsize::new(0);
static HANDLER: AtomicUsize = AtomicUsize::new(0);
static PHASE: AtomicUsize = AtomicUsize::new(0);
static LAST_PHASE_MS: AtomicU64 = AtomicU64::new(0);
static LAST_RUNTIME_READY_MS: AtomicU64 = AtomicU64::new(0);
static HUDHOOK_THREAD_ID: AtomicUsize = AtomicUsize::new(0);
static HUDHOOK_RENDER_DEPTH: AtomicUsize = AtomicUsize::new(0);
static HUDHOOK_RENDER_COUNT: AtomicU64 = AtomicU64::new(0);
static HUDHOOK_VISIBLE_RENDER_COUNT: AtomicU64 = AtomicU64::new(0);
static LAST_HUDHOOK_RENDER_BEGIN_MS: AtomicU64 = AtomicU64::new(0);
static LAST_HUDHOOK_RENDER_END_MS: AtomicU64 = AtomicU64::new(0);
static LAST_HUDHOOK_VISIBLE_RENDER_MS: AtomicU64 = AtomicU64::new(0);
static LAST_EXCEPTION_CODE: AtomicUsize = AtomicUsize::new(0);
static LAST_EXCEPTION_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static LAST_EXCEPTION_THREAD_ID: AtomicUsize = AtomicUsize::new(0);
static LAST_SNAPSHOT_MS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn set_module_base(module: HINSTANCE) {
    let base = module.0 as usize;
    MODULE_BASE.store(base, Ordering::SeqCst);
    let size = unsafe { pe_size_of_image(base) };
    MODULE_SIZE.store(size, Ordering::SeqCst);
    mark_phase(Phase::DllAttach);
}

pub(crate) fn install_handler() {
    if HANDLER.load(Ordering::SeqCst) != 0 {
        return;
    }
    let handle = unsafe { AddVectoredExceptionHandler(1, Some(exception_handler)) } as usize;
    if handle == 0 {
        net_effects_log(format_args!(
            "crash-telemetry: AddVectoredExceptionHandler failed"
        ));
        return;
    }
    HANDLER.store(handle, Ordering::SeqCst);
    mark_phase(Phase::HandlerInstalled);
    write_snapshot("handler-installed", true);
    net_effects_log(format_args!(
        "crash-telemetry: handler installed module=0x{:x}+0x{:x}",
        MODULE_BASE.load(Ordering::SeqCst),
        MODULE_SIZE.load(Ordering::SeqCst)
    ));
}

pub(crate) fn hudhook_apply_ok() {
    mark_phase(Phase::HudhookApplyOk);
    write_snapshot("hudhook-apply-ok", true);
}

pub(crate) fn hudhook_apply_failed() {
    mark_phase(Phase::HudhookApplyFailed);
    write_snapshot("hudhook-apply-failed", true);
}

pub(crate) fn hudhook_initialize() {
    mark_phase(Phase::HudhookInitialize);
    write_snapshot("hudhook-initialize", true);
}

pub(crate) fn hudhook_render_enter() {
    HUDHOOK_THREAD_ID.store(current_thread_id(), Ordering::SeqCst);
    HUDHOOK_RENDER_DEPTH.fetch_add(1, Ordering::SeqCst);
    HUDHOOK_RENDER_COUNT.fetch_add(1, Ordering::SeqCst);
    LAST_HUDHOOK_RENDER_BEGIN_MS.store(now_ms(), Ordering::SeqCst);
    mark_phase(Phase::HudhookRenderEnter);
}

pub(crate) fn hudhook_render_visible() {
    HUDHOOK_VISIBLE_RENDER_COUNT.fetch_add(1, Ordering::SeqCst);
    LAST_HUDHOOK_VISIBLE_RENDER_MS.store(now_ms(), Ordering::SeqCst);
    mark_phase(Phase::HudhookRenderVisible);
}

pub(crate) fn hudhook_render_exit() {
    LAST_HUDHOOK_RENDER_END_MS.store(now_ms(), Ordering::SeqCst);
    mark_phase(Phase::HudhookRenderExit);
    let depth = HUDHOOK_RENDER_DEPTH.load(Ordering::SeqCst);
    if depth <= 1 {
        HUDHOOK_RENDER_DEPTH.store(0, Ordering::SeqCst);
        HUDHOOK_THREAD_ID.store(0, Ordering::SeqCst);
    } else {
        HUDHOOK_RENDER_DEPTH.store(depth - 1, Ordering::SeqCst);
    }
}

pub(crate) fn runtime_ready(ready: bool) {
    if ready {
        LAST_RUNTIME_READY_MS.store(now_ms(), Ordering::SeqCst);
        mark_phase(Phase::RuntimeReady);
        write_snapshot("runtime-ready", true);
    } else {
        mark_phase(Phase::RuntimeSuspended);
        write_snapshot("runtime-suspended", true);
    }
}

pub(crate) fn telemetry_json_fields() -> String {
    let now = now_ms();
    format!(
        "  \"crash_telemetry_phase\": \"{}\",\n  \"crash_telemetry_phase_id\": {},\n  \"crash_telemetry_ms_since_phase\": {},\n  \"crash_telemetry_hudhook_render_depth\": {},\n  \"crash_telemetry_hudhook_thread_id\": {},\n  \"crash_telemetry_hudhook_render_count\": {},\n  \"crash_telemetry_hudhook_visible_render_count\": {},\n  \"crash_telemetry_ms_since_hudhook_render_begin\": {},\n  \"crash_telemetry_ms_since_hudhook_render_end\": {},\n  \"crash_telemetry_ms_since_hudhook_visible_render\": {},\n  \"crash_telemetry_last_exception_code\": \"0x{:08x}\",\n  \"crash_telemetry_last_exception_address\": \"0x{:x}\",\n",
        phase_label(PHASE.load(Ordering::SeqCst)),
        PHASE.load(Ordering::SeqCst),
        age_ms(now, LAST_PHASE_MS.load(Ordering::SeqCst)),
        HUDHOOK_RENDER_DEPTH.load(Ordering::SeqCst),
        HUDHOOK_THREAD_ID.load(Ordering::SeqCst),
        HUDHOOK_RENDER_COUNT.load(Ordering::SeqCst),
        HUDHOOK_VISIBLE_RENDER_COUNT.load(Ordering::SeqCst),
        age_ms(now, LAST_HUDHOOK_RENDER_BEGIN_MS.load(Ordering::SeqCst)),
        age_ms(now, LAST_HUDHOOK_RENDER_END_MS.load(Ordering::SeqCst)),
        age_ms(now, LAST_HUDHOOK_VISIBLE_RENDER_MS.load(Ordering::SeqCst)),
        LAST_EXCEPTION_CODE.load(Ordering::SeqCst) as u32,
        LAST_EXCEPTION_ADDRESS.load(Ordering::SeqCst),
    )
}

fn mark_phase(phase: Phase) {
    PHASE.store(phase as usize, Ordering::SeqCst);
    LAST_PHASE_MS.store(now_ms(), Ordering::SeqCst);
}

unsafe extern "system" fn exception_handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    let mut code = 0u32;
    let mut addr = 0usize;
    let mut access_kind = 0usize;
    let mut access_addr = 0usize;
    let mut rip = 0usize;
    let mut rsp = 0usize;
    let mut rbp = 0usize;
    if !info.is_null() {
        let pointers = unsafe { &*info };
        if !pointers.ExceptionRecord.is_null() {
            let record = unsafe { &*pointers.ExceptionRecord };
            code = record.ExceptionCode.0 as u32;
            addr = record.ExceptionAddress as usize;
            if record.NumberParameters > 0 {
                access_kind = record.ExceptionInformation[0];
            }
            if record.NumberParameters > 1 {
                access_addr = record.ExceptionInformation[1];
            }
        }
        if !pointers.ContextRecord.is_null() {
            let context = unsafe { &*pointers.ContextRecord };
            rip = context.Rip as usize;
            rsp = context.Rsp as usize;
            rbp = context.Rbp as usize;
        }
    }
    if !is_crash_like_exception(code) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let thread_id = current_thread_id();
    LAST_EXCEPTION_CODE.store(code as usize, Ordering::SeqCst);
    LAST_EXCEPTION_ADDRESS.store(addr, Ordering::SeqCst);
    LAST_EXCEPTION_THREAD_ID.store(thread_id, Ordering::SeqCst);
    mark_phase(Phase::ExceptionObserved);
    let report = exception_report(&ExceptionSite {
        code,
        addr,
        access_kind,
        access_addr,
        rip,
        rsp,
        rbp,
        thread_id,
    });
    let _ = fs::write(CRASH_LATEST_PATH, &report);
    // Fresh per process: this run's crashes, `---`-separated, and nothing from a build that no
    // longer exists. The previous run's file survives one generation as `.log.prev`.
    if let Some(mut file) =
        er_game_base::log::open_fresh_run_append(std::path::Path::new(CRASH_LOG_PATH))
    {
        let _ = file.write_all(report.as_bytes());
        let _ = file.write_all(b"\n---\n");
    }
    EXCEPTION_CONTINUE_SEARCH
}

/// The faulting-thread facts pulled out of `EXCEPTION_POINTERS`, grouped so the report
/// builder takes one argument instead of eight.
struct ExceptionSite {
    code: u32,
    addr: usize,
    access_kind: usize,
    access_addr: usize,
    rip: usize,
    rsp: usize,
    rbp: usize,
    thread_id: usize,
}

fn exception_report(site: &ExceptionSite) -> String {
    let &ExceptionSite {
        code,
        addr,
        access_kind,
        access_addr,
        rip,
        rsp,
        rbp,
        thread_id,
    } = site;
    let now = now_ms();
    let module_base = MODULE_BASE.load(Ordering::SeqCst);
    let module_size = MODULE_SIZE.load(Ordering::SeqCst);
    let phase = PHASE.load(Ordering::SeqCst);
    let hudhook_thread = HUDHOOK_THREAD_ID.load(Ordering::SeqCst);
    let hudhook_depth = HUDHOOK_RENDER_DEPTH.load(Ordering::SeqCst);
    let in_hudhook_render_thread = hudhook_depth != 0 && hudhook_thread == thread_id;
    let exception_in_dll = module_contains(addr) || module_contains(rip);
    let mut out = String::new();
    let _ = writeln!(out, "reason=veh-crash-like-exception");
    let _ = writeln!(out, "exception_code=0x{code:08x}");
    let _ = writeln!(out, "exception_address=0x{addr:x}");
    let _ = writeln!(out, "exception_access_kind={access_kind}");
    let _ = writeln!(out, "exception_access_address=0x{access_addr:x}");
    let _ = writeln!(out, "context_rip=0x{rip:x}");
    let _ = writeln!(out, "context_rsp=0x{rsp:x}");
    let _ = writeln!(out, "context_rbp=0x{rbp:x}");
    let _ = writeln!(out, "thread_id={thread_id}");
    let _ = writeln!(out, "dll_module_base=0x{module_base:x}");
    let _ = writeln!(out, "dll_module_size=0x{module_size:x}");
    let _ = writeln!(out, "exception_ip_in_er_net_effects_dll={exception_in_dll}");
    let _ = writeln!(out, "phase_id={phase}");
    let _ = writeln!(out, "phase={}", phase_label(phase));
    let _ = writeln!(
        out,
        "ms_since_phase={}",
        age_ms(now, LAST_PHASE_MS.load(Ordering::SeqCst))
    );
    let _ = writeln!(
        out,
        "ms_since_runtime_ready={}",
        age_ms(now, LAST_RUNTIME_READY_MS.load(Ordering::SeqCst))
    );
    let _ = writeln!(out, "hudhook_render_depth={hudhook_depth}");
    let _ = writeln!(out, "hudhook_thread_id={hudhook_thread}");
    let _ = writeln!(
        out,
        "exception_on_hudhook_render_thread={in_hudhook_render_thread}"
    );
    let _ = writeln!(
        out,
        "hudhook_render_count={}",
        HUDHOOK_RENDER_COUNT.load(Ordering::SeqCst)
    );
    let _ = writeln!(
        out,
        "hudhook_visible_render_count={}",
        HUDHOOK_VISIBLE_RENDER_COUNT.load(Ordering::SeqCst)
    );
    let _ = writeln!(
        out,
        "ms_since_hudhook_render_begin={}",
        age_ms(now, LAST_HUDHOOK_RENDER_BEGIN_MS.load(Ordering::SeqCst))
    );
    let _ = writeln!(
        out,
        "ms_since_hudhook_render_end={}",
        age_ms(now, LAST_HUDHOOK_RENDER_END_MS.load(Ordering::SeqCst))
    );
    let _ = writeln!(
        out,
        "ms_since_hudhook_visible_render={}",
        age_ms(now, LAST_HUDHOOK_VISIBLE_RENDER_MS.load(Ordering::SeqCst))
    );
    out
}

fn write_snapshot(reason: &str, force: bool) {
    let now = now_ms();
    if !force {
        let last = LAST_SNAPSHOT_MS.load(Ordering::SeqCst);
        if last != 0 && now.saturating_sub(last) < SNAPSHOT_INTERVAL_MS {
            return;
        }
        if LAST_SNAPSHOT_MS
            .compare_exchange(last, now, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
    } else {
        LAST_SNAPSHOT_MS.store(now, Ordering::SeqCst);
    }
    let mut out = String::new();
    let _ = writeln!(out, "reason={reason}");
    let _ = writeln!(out, "phase={}", phase_label(PHASE.load(Ordering::SeqCst)));
    let _ = writeln!(out, "phase_id={}", PHASE.load(Ordering::SeqCst));
    let _ = writeln!(
        out,
        "hudhook_render_depth={}",
        HUDHOOK_RENDER_DEPTH.load(Ordering::SeqCst)
    );
    let _ = writeln!(
        out,
        "hudhook_thread_id={}",
        HUDHOOK_THREAD_ID.load(Ordering::SeqCst)
    );
    let _ = writeln!(
        out,
        "hudhook_render_count={}",
        HUDHOOK_RENDER_COUNT.load(Ordering::SeqCst)
    );
    let _ = writeln!(
        out,
        "hudhook_visible_render_count={}",
        HUDHOOK_VISIBLE_RENDER_COUNT.load(Ordering::SeqCst)
    );
    let _ = writeln!(
        out,
        "ms_since_hudhook_render_begin={}",
        age_ms(now, LAST_HUDHOOK_RENDER_BEGIN_MS.load(Ordering::SeqCst))
    );
    let _ = writeln!(
        out,
        "ms_since_hudhook_render_end={}",
        age_ms(now, LAST_HUDHOOK_RENDER_END_MS.load(Ordering::SeqCst))
    );
    let _ = fs::write(BREADCRUMB_PATH, out);
}

fn is_crash_like_exception(code: u32) -> bool {
    matches!(
        code,
        EXCEPTION_ACCESS_VIOLATION
            | EXCEPTION_ILLEGAL_INSTRUCTION
            | EXCEPTION_STACK_BUFFER_OVERRUN
            | EXCEPTION_STACK_OVERFLOW
    )
}

fn module_contains(addr: usize) -> bool {
    let base = MODULE_BASE.load(Ordering::SeqCst);
    let size = MODULE_SIZE.load(Ordering::SeqCst);
    base != 0 && size != 0 && addr >= base && addr < base.saturating_add(size)
}

fn phase_label(phase: usize) -> &'static str {
    match phase {
        1 => "dll-attach",
        2 => "handler-installed",
        10 => "runtime-suspended",
        11 => "runtime-ready",
        20 => "present-enter",
        21 => "draw-begin",
        22 => "draw-target",
        23 => "draw-barrier-to-copy",
        24 => "draw-copy",
        25 => "draw-barrier-to-present",
        26 => "draw-execute-wait",
        27 => "draw-end-ok",
        28 => "draw-end-fail",
        29 => "draw-failure-recorded",
        30 => "present-failure-recorded",
        31 => "original-present",
        32 => "present-exit",
        40 => "hudhook-apply-ok",
        41 => "hudhook-apply-failed",
        42 => "hudhook-initialize",
        43 => "hudhook-render-enter",
        44 => "hudhook-render-visible",
        45 => "hudhook-render-exit",
        90 => "exception-observed",
        _ => "uninitialized",
    }
}

fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn age_ms(now: u64, then: u64) -> u64 {
    if then == 0 {
        0
    } else {
        now.saturating_sub(then)
    }
}

fn current_thread_id() -> usize {
    unsafe { GetCurrentThreadId() as usize }
}

unsafe fn pe_size_of_image(base: usize) -> usize {
    if base == 0 {
        return 0;
    }
    let mz = unsafe { *(base as *const u16) };
    if mz != 0x5a4d {
        return 0;
    }
    let e_lfanew = unsafe { *((base + 0x3c) as *const i32) };
    if !(0..=0x1000).contains(&e_lfanew) {
        return 0;
    }
    let nt = base + e_lfanew as usize;
    let sig = unsafe { *(nt as *const u32) };
    if sig != 0x0000_4550 {
        return 0;
    }
    let optional = nt + 0x18;
    unsafe { *((optional + 0x38) as *const u32) as usize }
}
