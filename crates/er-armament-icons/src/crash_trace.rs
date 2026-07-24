//! In-DLL crash tracer (bd directive-dll-owns-crash-logging-deep-traces-veh, 2026-07-23).
//!
//! Installs a Vectored Exception Handler (fires FIRST, before arxan/OS handlers) plus an
//! unhandled-exception filter. On an access violation it writes a DEEP trace to the DLL
//! log: exception code, faulting instruction address + game-module RVA, faulting data
//! address, access kind, `CONTEXT` Rip/Rsp, and a scanned return-address backtrace (stack
//! qwords that land inside the game image). Rate-limited so arxan's routine AVs don't spam
//! the log. This replaces "no log line after N, so the crash is before X" guesswork with
//! the exact faulting RVA -- feed it to `scripts/dump-deobf-shift.py --reverse` / disasm.

#![cfg(windows)]

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use er_game_base::mem::{game_module_base, safe_read_usize};

use crate::log_message;

/// `EXCEPTION_POINTERS`: pointers to the `EXCEPTION_RECORD` and `CONTEXT`.
#[repr(C)]
struct ExceptionPointers {
    exception_record: *mut u8,
    context_record: *mut u8,
}

/// Let the OS/arxan continue searching for a handler after we log (we never swallow).
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const ACCESS_VIOLATION: u32 = 0xC000_0005;
/// Cap logged crashes so routine/handled AVs can't flood the log.
const MAX_CRASH_LOGS: u64 = 12;
/// Game image span used to recognize code/return addresses.
const MODULE_SPAN: usize = 0x0800_0000;

static CRASH_LOGS: AtomicU64 = AtomicU64::new(0);
static INSTALLED: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" {
    fn AddVectoredExceptionHandler(first: u32, handler: usize) -> usize;
    fn SetUnhandledExceptionFilter(filter: usize) -> usize;
}

/// Emit the deep trace for one exception. `tag` distinguishes the VEH vs unhandled path.
unsafe fn trace_exception(tag: &str, info: *mut ExceptionPointers) {
    if info.is_null() {
        return;
    }
    let rec = unsafe { (*info).exception_record };
    if rec.is_null() {
        return;
    }
    let base = game_module_base().unwrap_or(0);
    let in_mod = |a: usize| base != 0 && a >= base && a < base + MODULE_SPAN;

    // EXCEPTION_RECORD (x64): +0x00 Code, +0x10 ExceptionAddress, +0x20 Info[0] (rw),
    // +0x28 Info[1] (faulting data address).
    let code = unsafe { *(rec as *const u32) };
    let fault_rip = unsafe { *(rec.add(0x10) as *const usize) };
    let acc = unsafe { *(rec.add(0x20) as *const usize) };
    let data = unsafe { *(rec.add(0x28) as *const usize) };
    log_message(format_args!(
        "CRASH {tag}: code=0x{code:x} rip=0x{fault_rip:x} (game+0x{:x}) access={} fault_addr=0x{data:x} base=0x{base:x}",
        if in_mod(fault_rip) {
            fault_rip - base
        } else {
            0
        },
        match acc {
            0 => "read",
            1 => "write",
            8 => "exec",
            _ => "?",
        },
    ));

    // CONTEXT (x64): Rsp @ +0x98, Rip @ +0xF8. Scan the stack for return-address-looking
    // values inside the game image as a poor-man's backtrace.
    let ctx = unsafe { (*info).context_record };
    if ctx.is_null() {
        return;
    }
    let rsp = unsafe { *(ctx.add(0x98) as *const usize) };
    let rip = unsafe { *(ctx.add(0xF8) as *const usize) };
    log_message(format_args!(
        "CRASH {tag} ctx: rip=0x{rip:x} (game+0x{:x}) rsp=0x{rsp:x}",
        if in_mod(rip) { rip - base } else { 0 }
    ));
    let mut logged = 0u32;
    let mut off = 0usize;
    while off < 0x400 && logged < 12 {
        if let Some(v) = unsafe { safe_read_usize(rsp.wrapping_add(off)) } {
            if in_mod(v) {
                log_message(format_args!(
                    "CRASH {tag} bt: [rsp+0x{off:x}] -> game+0x{:x}",
                    v - base
                ));
                logged += 1;
            }
        }
        off += 8;
    }
}

/// VEH: fires for every exception. We log only access violations, rate-limited.
unsafe extern "system" fn veh(info: *mut ExceptionPointers) -> i32 {
    if info.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let rec = unsafe { (*info).exception_record };
    if rec.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let code = unsafe { *(rec as *const u32) };
    if code == ACCESS_VIOLATION && CRASH_LOGS.fetch_add(1, Ordering::SeqCst) < MAX_CRASH_LOGS {
        unsafe { trace_exception("VEH", info) };
    }
    EXCEPTION_CONTINUE_SEARCH
}

/// Unhandled filter: fires only for the actual crash (no handler caught it). Always logs.
unsafe extern "system" fn ueh(info: *mut ExceptionPointers) -> i32 {
    unsafe { trace_exception("UEH", info) };
    EXCEPTION_CONTINUE_SEARCH
}

/// Install the crash tracer. Idempotent; call before other hooks.
pub(crate) fn install() {
    if INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let h = unsafe { AddVectoredExceptionHandler(1, veh as usize) };
    let prev = unsafe { SetUnhandledExceptionFilter(ueh as usize) };
    log_message(format_args!(
        "crash-trace: installed (veh=0x{h:x} prev_ueh=0x{prev:x})"
    ));
}
