//! Main-thread hang watchdog.
//!
//! A crash logger only sees a process that faults. A process that *freezes* raises no exception at
//! all, so an in-game lockup leaves the crash log completely empty and the only record of it is
//! whatever the exit path happens to crash on afterwards -- evidence about shutdown, not about the
//! bug. This module closes that blind spot.
//!
//! The signal is the game's own per-frame counter: `MainUpdate` increments a single dword once per
//! main-loop tick, so that dword advancing *is* the definition of "the main thread is alive". When
//! it stops advancing for longer than the configured window, the watchdog snapshots every thread in
//! the process -- instruction pointer, stack pointer, and a raw stack scan resolved against the
//! loaded-module table -- and writes it out the same way a fault record is written.
//!
//! Two properties matter more than the detection itself:
//!
//! * **It proves its own address before trusting it.** The frame-counter RVA is version-pinned. The
//!   watchdog refuses to arm until it has actually watched the counter advance several times, so a
//!   game patch that moves the field disarms the watchdog and says so, instead of reporting a
//!   permanent fake hang.
//! * **It never allocates while a thread is suspended.** Suspending a thread that holds the heap
//!   lock and then allocating would deadlock the very process being diagnosed. Each thread is
//!   suspended, read into fixed storage, and resumed before any formatting happens.
//!
//! The watchdog is diagnostic only: it never kills, never faults, and never changes game state.

#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{CONTEXT_RIP_OFFSET, CONTEXT_RSP_OFFSET, module_tag};
#[cfg(windows)]
use crate::{
    MIN_VALID_PTR, append_log, config, loaded_modules, ms_since_install, path_for, safe_read_u32,
    safe_read_usize, utc_timestamp,
};

/// `eldenring.exe` 1.16.2: the dword `MainUpdate` (`0x140dea370`) increments once per main-loop
/// frame, at `0x140dea394` -- `INC dword ptr [0x143d8567c]`. Image base is `0x140000000`, so the
/// runtime address is the loaded exe base plus this RVA.
///
/// This is the one version-pinned constant in the module. `validate_frame_counter` is what keeps it
/// honest on any other build.
const GAME_FRAME_COUNTER_RVA: usize = 0x3d8_567c;

/// Executable this counter belongs to. Checked case-insensitively before the RVA is read at all, so
/// the watchdog stays inert if the crate is ever loaded into some other host process.
const GAME_MODULE_NAME: &str = "eldenring.exe";

/// Let the loader finish and the game reach its main loop before the first sample. Nothing in this
/// module may touch the game until `DllMain` has long returned.
const STARTUP_DELAY_MS: u32 = 5_000;

const SAMPLE_INTERVAL_MS: u32 = 1_000;

/// How many distinct increments must be observed before the watchdog trusts the address.
const ARM_ADVANCES_REQUIRED: u32 = 3;

/// How long to wait for those increments before giving up and disarming. Generous: the counter does
/// not move until the game is past its own boot.
const ARM_TIMEOUT_SAMPLES: u32 = 300;

/// One stall is one report. Bounded so a game left frozen overnight cannot fill the disk.
const MAX_HANG_REPORTS: usize = 3;

const MAX_THREADS_SAMPLED: usize = 128;
const THREAD_STACK_QWORDS: usize = 192;
const THREAD_STACK_MAX_FRAMES: usize = 40;

#[cfg(windows)]
const TH32CS_SNAPTHREAD: u32 = 0x0000_0004;
#[cfg(windows)]
const THREAD_SUSPEND_RESUME: u32 = 0x0002;
#[cfg(windows)]
const THREAD_GET_CONTEXT: u32 = 0x0008;
#[cfg(windows)]
const THREAD_QUERY_INFORMATION: u32 = 0x0040;
#[cfg(windows)]
const INVALID_HANDLE_VALUE: isize = -1;
#[cfg(windows)]
const SUSPEND_FAILED: u32 = u32::MAX;

/// x86-64 `CONTEXT`: 0x4d0 bytes, 16-byte aligned, `ContextFlags` at offset 0x30.
const CONTEXT_SIZE: usize = 0x4d0;
const CONTEXT_FLAGS_OFFSET: usize = 0x30;
#[cfg(windows)]
const CONTEXT_AMD64_CONTROL_INTEGER: u32 = 0x0010_0000 | 0x0000_0001 | 0x0000_0002;

#[cfg(windows)]
static HANG_REPORTS_WRITTEN: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static WATCHDOG_STARTED: std::sync::Once = std::sync::Once::new();

#[cfg(windows)]
type ThreadStart = unsafe extern "system" fn(*mut c_void) -> u32;

#[repr(C)]
struct ThreadEntry32 {
    size: u32,
    usage: u32,
    thread_id: u32,
    owner_process_id: u32,
    base_priority: i32,
    delta_priority: i32,
    flags: u32,
}

#[repr(C, align(16))]
struct ThreadContext([u8; CONTEXT_SIZE]);

#[cfg(windows)]
unsafe extern "system" {
    fn CreateThread(
        attributes: *mut c_void,
        stack_size: usize,
        start: ThreadStart,
        parameter: *mut c_void,
        flags: u32,
        thread_id: *mut u32,
    ) -> isize;
    fn Sleep(milliseconds: u32);
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> isize;
    fn Thread32First(snapshot: isize, entry: *mut ThreadEntry32) -> i32;
    fn Thread32Next(snapshot: isize, entry: *mut ThreadEntry32) -> i32;
    fn OpenThread(access: u32, inherit: i32, thread_id: u32) -> isize;
    fn SuspendThread(thread: isize) -> u32;
    fn ResumeThread(thread: isize) -> u32;
    fn GetThreadContext(thread: isize, context: *mut c_void) -> i32;
    fn GetThreadTimes(
        thread: isize,
        creation: *mut u64,
        exit: *mut u64,
        kernel: *mut u64,
        user: *mut u64,
    ) -> i32;
    fn CloseHandle(handle: isize) -> i32;
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentThreadId() -> u32;
}

/// Start the watchdog thread.
///
/// Safe to call from `DllMain`: the thread is created and never joined, so the loader lock is not
/// held across anything the new thread does. A `hang_stall_seconds` of 0 disables the watchdog.
#[cfg(windows)]
pub(crate) fn start(stall_seconds: u64) {
    if stall_seconds == 0 {
        return;
    }
    WATCHDOG_STARTED.call_once(|| {
        let handle = unsafe {
            CreateThread(
                std::ptr::null_mut(),
                0,
                watchdog_thread,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
            )
        };
        if handle == 0 {
            append_log(format_args!("hang watchdog CreateThread failed"));
            return;
        }
        unsafe { CloseHandle(handle) };
    });
}

#[cfg(not(windows))]
pub(crate) fn start(_stall_seconds: u64) {}

#[cfg(windows)]
unsafe extern "system" fn watchdog_thread(_parameter: *mut c_void) -> u32 {
    unsafe { Sleep(STARTUP_DELAY_MS) };

    let Some(counter) = locate_frame_counter() else {
        append_log(format_args!(
            "hang watchdog disarmed reason=game-module-not-found expected={GAME_MODULE_NAME}"
        ));
        return 0;
    };

    let Some(mut last_value) = validate_frame_counter(counter) else {
        append_log(format_args!(
            "hang watchdog disarmed reason=frame-counter-never-advanced addr=0x{counter:x} \
             rva=0x{GAME_FRAME_COUNTER_RVA:x} note=game build likely moved the field"
        ));
        return 0;
    };

    let stall = config().hang_stall_seconds;
    append_log(format_args!(
        "hang watchdog armed addr=0x{counter:x} rva=0x{GAME_FRAME_COUNTER_RVA:x} \
         frame_counter={last_value} stall_seconds={stall}"
    ));

    let mut last_change = std::time::Instant::now();
    let mut reported_this_stall = false;

    loop {
        unsafe { Sleep(SAMPLE_INTERVAL_MS) };
        let Some(value) = (unsafe { safe_read_u32(counter) }) else {
            // The exe unmapping means the process is going away; nothing left to watch.
            return 0;
        };
        if value != last_value {
            last_value = value;
            last_change = std::time::Instant::now();
            reported_this_stall = false;
            continue;
        }
        if reported_this_stall {
            continue;
        }
        let stalled_for = last_change.elapsed();
        if stalled_for.as_secs() < stall {
            continue;
        }
        reported_this_stall = true;
        if HANG_REPORTS_WRITTEN.fetch_add(1, Ordering::SeqCst) >= MAX_HANG_REPORTS {
            append_log(format_args!(
                "hang watchdog report budget exhausted; further stalls will not be captured"
            ));
            return 0;
        }
        report_stall(counter, value, stalled_for.as_secs());
    }
}

/// Resolve the frame counter's runtime address, refusing any host that is not the expected exe.
#[cfg(windows)]
fn locate_frame_counter() -> Option<usize> {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    if base < MIN_VALID_PTR {
        return None;
    }
    let is_expected_host = loaded_modules().iter().any(|(module_base, _, name)| {
        *module_base == base && name.eq_ignore_ascii_case(GAME_MODULE_NAME)
    });
    if !is_expected_host {
        return None;
    }
    Some(base + GAME_FRAME_COUNTER_RVA)
}

/// Watch the counter until it has demonstrably advanced, and only then let the watchdog arm.
///
/// This is what makes a version-pinned RVA safe to ship: on a build where the field has moved, the
/// address either reads as unmapped or sits still, and the watchdog disarms instead of reporting a
/// hang that is not happening.
#[cfg(windows)]
fn validate_frame_counter(addr: usize) -> Option<u32> {
    let mut previous = unsafe { safe_read_u32(addr) }?;
    let mut advances = 0u32;
    for _ in 0..ARM_TIMEOUT_SAMPLES {
        unsafe { Sleep(SAMPLE_INTERVAL_MS) };
        let current = unsafe { safe_read_u32(addr) }?;
        if current != previous {
            advances += 1;
            previous = current;
            if advances >= ARM_ADVANCES_REQUIRED {
                return Some(current);
            }
        }
    }
    None
}

#[cfg(windows)]
fn report_stall(counter_addr: usize, frame_counter: u32, stalled_seconds: u64) {
    use std::fmt::Write as _;

    let modules = loaded_modules();
    let mut out = String::new();
    let _ = writeln!(out, "reason=main-thread-stall");
    let _ = writeln!(out, "module={}", config().module_label);
    let _ = writeln!(out, "utc={}", utc_timestamp());
    let _ = writeln!(out, "ms_since_install={}", ms_since_install());
    let _ = writeln!(out, "stalled_seconds={stalled_seconds}");
    let _ = writeln!(
        out,
        "stall_threshold_seconds={}",
        config().hang_stall_seconds
    );
    let _ = writeln!(out, "frame_counter={frame_counter}");
    let _ = writeln!(out, "frame_counter_addr=0x{counter_addr:x}");
    let _ = writeln!(
        out,
        "frame_counter_rva=0x{GAME_FRAME_COUNTER_RVA:x} host={GAME_MODULE_NAME}"
    );
    let _ = writeln!(
        out,
        "report_index={}",
        HANG_REPORTS_WRITTEN
            .load(Ordering::SeqCst)
            .saturating_sub(1)
    );

    let threads = enumerate_threads();
    let _ = writeln!(out, "thread_count={}", threads.len());

    // The main thread is the process's first thread, so it is the one with the earliest creation
    // time. That is what identifies the stalled stack among all the workers.
    let mut earliest_creation = u64::MAX;
    let mut main_thread_id = 0u32;
    let mut samples = Vec::with_capacity(threads.len());
    for thread_id in threads {
        if let Some(sample) = sample_thread(thread_id) {
            if sample.creation_time != 0 && sample.creation_time < earliest_creation {
                earliest_creation = sample.creation_time;
                main_thread_id = sample.thread_id;
            }
            samples.push(sample);
        }
    }

    for sample in &samples {
        let is_main = sample.thread_id == main_thread_id;
        let _ = writeln!(
            out,
            "thread tid={} main_thread={} rip=0x{:x}{} rsp=0x{:x} stack={}",
            sample.thread_id,
            is_main,
            sample.rip,
            module_tag(sample.rip, &modules),
            sample.rsp,
            sample.format_stack(&modules)
        );
    }

    let _ = std::fs::write(path_for(config().hang_report_file_name), &out);
    append_log(format_args!("{out}---"));

    // The text snapshot is a raw stack scan; the dump carries the real unwind for every thread.
    unsafe { crate::write_minidump_named(config().hang_minidump_file_name, std::ptr::null_mut()) };
}

#[cfg(windows)]
fn enumerate_threads() -> Vec<u32> {
    let mut ids = Vec::new();
    let process_id = unsafe { GetCurrentProcessId() };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE || snapshot == 0 {
        return ids;
    }
    let mut entry = new_thread_entry();
    let mut ok = unsafe { Thread32First(snapshot, &mut entry) };
    while ok != 0 && ids.len() < MAX_THREADS_SAMPLED {
        if entry.owner_process_id == process_id {
            ids.push(entry.thread_id);
        }
        entry = new_thread_entry();
        ok = unsafe { Thread32Next(snapshot, &mut entry) };
    }
    unsafe { CloseHandle(snapshot) };
    ids
}

fn new_thread_entry() -> ThreadEntry32 {
    ThreadEntry32 {
        size: std::mem::size_of::<ThreadEntry32>() as u32,
        usage: 0,
        thread_id: 0,
        owner_process_id: 0,
        base_priority: 0,
        delta_priority: 0,
        flags: 0,
    }
}

struct ThreadSample {
    thread_id: u32,
    rip: usize,
    rsp: usize,
    creation_time: u64,
    stack: [usize; THREAD_STACK_QWORDS],
    stack_len: usize,
}

impl ThreadSample {
    fn format_stack(&self, modules: &[(usize, usize, String)]) -> String {
        let mut out = String::from("[");
        let mut emitted = 0usize;
        let mut last = String::new();
        for value in self.stack.iter().take(self.stack_len) {
            if emitted >= THREAD_STACK_MAX_FRAMES {
                break;
            }
            let tag = module_tag(*value, modules);
            if tag.is_empty() || tag == last {
                continue;
            }
            if emitted != 0 {
                out.push(',');
            }
            // `module_tag` wraps in braces for inline use; unwrap it for the list form.
            out.push_str(tag.trim_matches(['{', '}']));
            emitted += 1;
            last = tag;
        }
        out.push(']');
        out
    }
}

/// Snapshot one thread.
///
/// Everything between `SuspendThread` and `ResumeThread` writes into fixed-size storage only. A
/// heap allocation there could block on a lock the suspended thread owns and freeze the process for
/// real, which would be a diagnostic tool causing the failure it exists to observe.
#[cfg(windows)]
fn sample_thread(thread_id: u32) -> Option<ThreadSample> {
    if thread_id == unsafe { GetCurrentThreadId() } {
        return None;
    }
    let access = THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION;
    let thread = unsafe { OpenThread(access, 0, thread_id) };
    if thread == 0 {
        return None;
    }

    let mut sample = ThreadSample {
        thread_id,
        rip: 0,
        rsp: 0,
        creation_time: 0,
        stack: [0; THREAD_STACK_QWORDS],
        stack_len: 0,
    };

    if unsafe { SuspendThread(thread) } != SUSPEND_FAILED {
        let mut context = ThreadContext([0u8; CONTEXT_SIZE]);
        context.0[CONTEXT_FLAGS_OFFSET..CONTEXT_FLAGS_OFFSET + 4]
            .copy_from_slice(&CONTEXT_AMD64_CONTROL_INTEGER.to_le_bytes());
        let got_context =
            unsafe { GetThreadContext(thread, context.0.as_mut_ptr().cast::<c_void>()) };
        if got_context != 0 {
            sample.rip = read_context_field(&context, CONTEXT_RIP_OFFSET);
            sample.rsp = read_context_field(&context, CONTEXT_RSP_OFFSET);
            if sample.rsp >= MIN_VALID_PTR {
                for slot in 0..THREAD_STACK_QWORDS {
                    let addr = sample.rsp + slot * std::mem::size_of::<usize>();
                    match unsafe { safe_read_usize(addr) } {
                        Some(value) => {
                            sample.stack[slot] = value;
                            sample.stack_len = slot + 1;
                        }
                        None => break,
                    }
                }
            }
        }
        unsafe { ResumeThread(thread) };
    }

    let mut creation = 0u64;
    let mut exit = 0u64;
    let mut kernel = 0u64;
    let mut user = 0u64;
    if unsafe { GetThreadTimes(thread, &mut creation, &mut exit, &mut kernel, &mut user) } != 0 {
        sample.creation_time = creation;
    }
    unsafe { CloseHandle(thread) };
    Some(sample)
}

fn read_context_field(context: &ThreadContext, offset: usize) -> usize {
    let mut bytes = [0u8; std::mem::size_of::<usize>()];
    bytes.copy_from_slice(&context.0[offset..offset + std::mem::size_of::<usize>()]);
    usize::from_le_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_counter_rva_matches_main_update_increment() {
        // `MainUpdate` @ 0x140dea370 contains `INC dword ptr [0x143d8567c]` at 0x140dea394.
        // Image base 0x140000000, so the counter's RVA is 0x3d8567c.
        assert_eq!(0x1_4000_0000_usize + GAME_FRAME_COUNTER_RVA, 0x1_43d8_567c);
    }

    #[test]
    fn context_layout_matches_windows_amd64() {
        // ContextFlags must sit inside the buffer, and RIP/RSP must be readable from it.
        assert!(CONTEXT_FLAGS_OFFSET + 4 <= CONTEXT_SIZE);
        assert!(CONTEXT_RIP_OFFSET + std::mem::size_of::<usize>() <= CONTEXT_SIZE);
        assert!(CONTEXT_RSP_OFFSET + std::mem::size_of::<usize>() <= CONTEXT_SIZE);
        assert_eq!(std::mem::align_of::<ThreadContext>(), 16);
    }

    #[test]
    fn thread_entry_size_matches_toolhelp_layout() {
        assert_eq!(std::mem::size_of::<ThreadEntry32>(), 28);
        assert_eq!(new_thread_entry().size as usize, 28);
    }

    #[test]
    fn arming_requires_more_than_one_observed_advance() {
        // A single advance could be noise in an unrelated dword; the point of the gate is that a
        // wrong address cannot arm the watchdog.
        assert!(ARM_ADVANCES_REQUIRED > 1);
        assert!(ARM_TIMEOUT_SAMPLES > ARM_ADVANCES_REQUIRED);
    }

    #[test]
    fn context_field_reads_little_endian_qword() {
        let mut context = ThreadContext([0u8; CONTEXT_SIZE]);
        context.0[CONTEXT_RIP_OFFSET..CONTEXT_RIP_OFFSET + 8]
            .copy_from_slice(&0x7ff6_1597_9999_usize.to_le_bytes());
        assert_eq!(
            read_context_field(&context, CONTEXT_RIP_OFFSET),
            0x7ff615979999
        );
    }

    #[test]
    fn stack_formatting_dedupes_and_drops_unresolved_addresses() {
        let modules = vec![(0x1000_0000usize, 0x1000usize, String::from("eldenring.exe"))];
        let mut sample = ThreadSample {
            thread_id: 1,
            rip: 0,
            rsp: 0,
            creation_time: 0,
            stack: [0; THREAD_STACK_QWORDS],
            stack_len: 4,
        };
        sample.stack[0] = 0x1000_0010;
        sample.stack[1] = 0x1000_0010; // repeat: collapsed
        sample.stack[2] = 0xdead_beef; // outside every module: dropped
        sample.stack[3] = 0x1000_0020;
        assert_eq!(
            sample.format_stack(&modules),
            "[eldenring.exe+0x10,eldenring.exe+0x20]"
        );
    }
}
