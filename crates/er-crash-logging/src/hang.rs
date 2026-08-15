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

// ---------------------------------------------------------------------------
// CS::LoadingScreenData -- the oracle a frame counter cannot be
// ---------------------------------------------------------------------------
//
// The frame-counter watchdog above cannot see a stuck LOAD. Frames keep advancing through a
// loading screen -- the screen animates, the update runs at ~59/s -- so a load that will never
// finish looks identical to a healthy one. Measured on a live Seamless invasion-load softlock
// (2026-08-15): the game sat at 12% for eleven minutes with three threads RUNNING and the
// frame counter ticking the whole time. The watchdog never fired, correctly, because nothing
// it watches was stalled.
//
// What IS stalled is visible one level down, in `CS::LoadingScreenData` (class name read from
// live MSVC RTTI; vtable 0x142a9be10 in 1.16.2):
//
//     +0x0c  u32   already-closed latch
//     +0x11  u8    CLOSE GATE -- the update refuses to finish while this is 0
//     +0x14  i32   active; -1 means reset/inactive
//     +0x18  f32   lerp START
//     +0x1c  f32   lerp TARGET      <- what everyone calls "progress"
//     +0x20  f32   lerp DURATION (seconds)
//     +0x24  f32   ELAPSED (seconds), advances 1.0/s
//
// The subtlety that makes this oracle correct where a naive one is wrong: `+0x1c` is NOT a
// progress counter, it is the TARGET of an interpolation. The game's getter (0x140860d40)
// returns lerp(start, target, elapsed/duration), clamped to the target once elapsed exceeds
// duration. So a bar frozen at 12% does not mean an animation is stuck mid-flight -- it means
// the last animation COMPLETED and no producer ever supplied a new target. In the captured
// softlock the animation had finished 685 seconds earlier.
//
// Hence the stall condition: the TARGET is unchanged while the screen's own ELAPSED clock keeps
// advancing, and the screen has neither been finalized (+0x11) nor closed (+0x0c). A live clock
// beside a dead target is a frozen substep; both frozen together just means the process died,
// which the frame counter already catches.
pub mod loading_screen {
    /// Offsets into `CS::LoadingScreenData`, verified against live values on 1.16.2.
    pub const ALREADY_CLOSED_OFFSET: usize = 0x0c;
    pub const CLOSE_GATE_OFFSET: usize = 0x11;
    pub const ACTIVE_OFFSET: usize = 0x14;
    pub const LERP_START_OFFSET: usize = 0x18;
    pub const LERP_TARGET_OFFSET: usize = 0x1c;
    pub const LERP_DURATION_OFFSET: usize = 0x20;
    pub const ELAPSED_OFFSET: usize = 0x24;

    /// One read of the struct. Kept plain data so the verdict logic is testable without a game.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Sample {
        pub active: i32,
        pub lerp_target: f32,
        pub elapsed: f32,
        pub close_gate: u8,
        pub already_closed: u8,
    }

    impl Sample {
        /// A screen that is displaying and has not been finalized or closed.
        ///
        /// `active < 0` is the reset state written by the game's own teardown (0x140860d90), so
        /// an inactive screen is not a stall -- it is the absence of a loading screen.
        #[must_use]
        pub fn is_pending(&self) -> bool {
            self.active >= 0 && self.close_gate == 0 && self.already_closed == 0
        }
    }

    /// Tracks consecutive observations of the same target while the clock advances.
    #[derive(Debug, Default)]
    pub struct Witness {
        last: Option<Sample>,
        /// Seconds of the screen's OWN clock elapsed since the target last changed. Using the
        /// game's clock rather than wall time means a paused or slowed process cannot be
        /// misreported as a stall -- if its clock stops too, the frame-counter watchdog owns it.
        frozen_for: f32,
    }

    /// Why the witness fired, carried into the report so the log states the evidence.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Verdict {
        pub lerp_target: f32,
        pub frozen_seconds: f32,
        pub elapsed: f32,
    }

    impl Witness {
        /// Feed one sample. Returns a verdict the first time the stall threshold is crossed.
        ///
        /// Returns `None` while healthy, and re-arms on any target change, so a load that
        /// resumes cannot leave the witness latched.
        pub fn observe(&mut self, sample: Sample, threshold_seconds: f32) -> Option<Verdict> {
            let previous = self.last.replace(sample);

            if !sample.is_pending() {
                self.frozen_for = 0.0;
                return None;
            }
            let Some(previous) = previous else {
                return None;
            };
            // A different screen (or the same one restarted) resets the accounting: the target
            // moved, or the clock went backwards because the struct was reused.
            if previous.lerp_target != sample.lerp_target || sample.elapsed < previous.elapsed {
                self.frozen_for = 0.0;
                return None;
            }

            let advanced = sample.elapsed - previous.elapsed;
            if advanced <= 0.0 {
                // The screen's clock is not moving either. That is a whole-process stall, which
                // the frame counter already owns; claiming it here would double-report it.
                return None;
            }

            let was_below = self.frozen_for < threshold_seconds;
            self.frozen_for += advanced;
            if was_below && self.frozen_for >= threshold_seconds {
                return Some(Verdict {
                    lerp_target: sample.lerp_target,
                    frozen_seconds: self.frozen_for,
                    elapsed: sample.elapsed,
                });
            }
            None
        }
    }
}

/// Live `CS::LoadingScreenData` pointer, published by whoever already has it.
///
/// This DLL deliberately does NOT hook the loading-screen update to obtain it. That prologue
/// (`LOADING_SCREEN_UPDATE_RVA` = 0x90a6b0) is already detoured by `er-loading-portrait`, which
/// the product links, and a second MinHook instance on one prologue is the documented
/// trampoline-corruption conflict this repo tracks in `scripts/me3-dll-conflicts.toml`. So the
/// witness consumes a pointer instead of competing for the hook: zero when nobody publishes one,
/// in which case the loading-screen oracle stays inert and the frame-counter watchdog is
/// unchanged.
#[cfg(windows)]
static LOADING_SCREEN_DATA: AtomicUsize = AtomicUsize::new(0);

/// Publish the current `CS::LoadingScreenData*`. Pass 0 when the screen goes away.
///
/// Only needed by a host that is not `er_effects_rs.dll`; when the product is present its
/// `er_effects_loading_screen_data` export is polled directly and nothing has to push.
#[cfg(windows)]
pub fn publish_loading_screen_data(pointer: usize) {
    LOADING_SCREEN_DATA.store(pointer, Ordering::Relaxed);
}

/// Name of the product export that hands over the live `CS::LoadingScreenData*`.
#[cfg(windows)]
const LOADING_SCREEN_DATA_EXPORT: &[u8] = b"er_effects_loading_screen_data\0";
#[cfg(windows)]
const PRODUCT_MODULE_NAME: &str = "er_effects_rs.dll";

/// Cached resolution of the product export: 0 = not looked up, 1 = looked up and absent.
#[cfg(windows)]
static LOADING_SCREEN_DATA_FN: AtomicUsize = AtomicUsize::new(0);

/// The current `CS::LoadingScreenData*` from whichever source exists.
///
/// Prefers a pushed pointer (a non-product host), else the product's export. Resolution is cached
/// including the negative result, so a standalone run does not pay a `GetProcAddress` every second
/// forever.
#[cfg(windows)]
fn current_loading_screen_data() -> usize {
    let pushed = LOADING_SCREEN_DATA.load(Ordering::Relaxed);
    if pushed >= MIN_VALID_PTR {
        return pushed;
    }
    let mut resolved = LOADING_SCREEN_DATA_FN.load(Ordering::Relaxed);
    if resolved == 0 {
        resolved = resolve_loading_screen_data_export();
        LOADING_SCREEN_DATA_FN.store(resolved, Ordering::Relaxed);
        append_log(format_args!(
            "hang watchdog loading-screen oracle {} (export {} in {PRODUCT_MODULE_NAME})",
            if resolved == 1 { "INERT" } else { "ARMED" },
            if resolved == 1 { "absent" } else { "resolved" },
        ));
    }
    if resolved <= 1 {
        return 0;
    }
    let getter: extern "system" fn() -> usize = unsafe { std::mem::transmute(resolved) };
    getter()
}

/// Look up the export, returning its address, or 1 for "absent" so the miss is cached too.
#[cfg(windows)]
fn resolve_loading_screen_data_export() -> usize {
    let wide: Vec<u16> = PRODUCT_MODULE_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let module = unsafe { GetModuleHandleW(wide.as_ptr()) };
    if module.is_null() {
        return 1;
    }
    let address = unsafe { GetProcAddress(module, LOADING_SCREEN_DATA_EXPORT.as_ptr()) };
    if address.is_null() {
        1
    } else {
        address as usize
    }
}

#[cfg(not(windows))]
pub fn publish_loading_screen_data(_pointer: usize) {}

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
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
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
    let mut loading_witness = loading_screen::Witness::default();
    let mut loading_reported = false;

    loop {
        unsafe { Sleep(SAMPLE_INTERVAL_MS) };

        // Checked every tick, INDEPENDENTLY of the frame counter. The whole point is that this
        // fires while frames are advancing normally -- gating it on the frame-counter stall
        // would reproduce the blind spot it exists to cover.
        if !loading_reported
            && let Some(sample) = read_loading_screen_sample()
            && let Some(verdict) = loading_witness.observe(sample, stall as f32)
        {
            loading_reported = true;
            if HANG_REPORTS_WRITTEN.fetch_add(1, Ordering::SeqCst) < MAX_HANG_REPORTS {
                report_loading_stall(verdict, sample);
            }
        }

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

/// Read `CS::LoadingScreenData` if someone published a pointer to it.
///
/// Every field is read through the same guarded readers the rest of this module uses, so a
/// pointer that has been freed or a screen torn down mid-read yields `None` rather than a fault
/// inside a watchdog whose entire job is to survive a sick process.
#[cfg(windows)]
fn read_loading_screen_sample() -> Option<loading_screen::Sample> {
    let base = current_loading_screen_data();
    if base < MIN_VALID_PTR {
        return None;
    }
    let word = |offset: usize| unsafe { safe_read_u32(base + offset) };
    let bits_to_f32 = |bits: u32| f32::from_bits(bits);

    let active = word(loading_screen::ACTIVE_OFFSET)? as i32;
    let lerp_target = bits_to_f32(word(loading_screen::LERP_TARGET_OFFSET)?);
    let elapsed = bits_to_f32(word(loading_screen::ELAPSED_OFFSET)?);
    let already_closed = word(loading_screen::ALREADY_CLOSED_OFFSET)? as u8;
    // +0x10 and +0x11 are adjacent bytes the game writes as one u16; read the word and split it
    // rather than issuing a byte read the guarded reader does not offer.
    let gate_word = word(loading_screen::CLOSE_GATE_OFFSET & !3)?;
    let close_gate = ((gate_word >> 8) & 0xff) as u8;

    Some(loading_screen::Sample {
        active,
        lerp_target,
        elapsed,
        close_gate,
        already_closed,
    })
}

#[cfg(not(windows))]
fn read_loading_screen_sample() -> Option<loading_screen::Sample> {
    None
}

/// Report a load that stopped progressing while its own clock kept running.
#[cfg(windows)]
fn report_loading_stall(verdict: loading_screen::Verdict, sample: loading_screen::Sample) {
    append_log(format_args!(
        "hang watchdog LOADING STALL -- progress target {:.4} unchanged for {:.1}s of the loading \
         screen's own clock (elapsed={:.1}s, close_gate={}, already_closed={}, active={}). Frames \
         are still advancing, so this is a stuck LOAD, not a frozen process.",
        verdict.lerp_target,
        verdict.frozen_seconds,
        verdict.elapsed,
        sample.close_gate,
        sample.already_closed,
        sample.active,
    ));
    // Same thread capture as a frame stall: the interesting stacks are whatever is (not) feeding
    // the load, and they are only observable while it is still wedged.
    for thread_id in enumerate_threads() {
        if let Some(sampled) = sample_thread(thread_id) {
            append_log(format_args!(
                "  loading-stall thread {thread_id}: {}",
                sampled.format_stack(&loaded_modules())
            ));
        }
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
    /// The captured softlock, replayed: target pinned at 0.12 while the screen's clock runs.
    #[test]
    fn loading_witness_fires_when_target_freezes_but_the_clock_runs() {
        use super::loading_screen::{Sample, Witness};
        let mut witness = Witness::default();
        let at = |elapsed: f32| Sample {
            active: 0,
            lerp_target: 0.12,
            elapsed,
            close_gate: 0,
            already_closed: 0,
        };
        // Live values from the 2026-08-15 invasion-load softlock: elapsed climbed 1.0/s while
        // the target never moved off 0.12.
        assert_eq!(
            witness.observe(at(688.0), 30.0),
            None,
            "first sample has no baseline"
        );
        for second in 1..30 {
            assert_eq!(
                witness.observe(at(688.0 + second as f32), 30.0),
                None,
                "must not fire before the threshold (second {second})"
            );
        }
        let verdict = witness
            .observe(at(718.0), 30.0)
            .expect("30s of frozen target with a live clock must fire");
        assert_eq!(verdict.lerp_target, 0.12);
        assert!(verdict.frozen_seconds >= 30.0);
        assert_eq!(
            witness.observe(at(719.0), 30.0),
            None,
            "must fire once per stall, not every sample afterwards"
        );
    }

    /// A healthy load moves its target; the witness must stay quiet and re-arm.
    #[test]
    fn loading_witness_stays_quiet_while_the_target_advances() {
        use super::loading_screen::{Sample, Witness};
        let mut witness = Witness::default();
        for step in 0..120 {
            let sample = Sample {
                active: 0,
                lerp_target: 0.05 * step as f32,
                elapsed: step as f32,
                close_gate: 0,
                already_closed: 0,
            };
            assert_eq!(
                witness.observe(sample, 30.0),
                None,
                "advancing progress is not a stall (step {step})"
            );
        }
    }

    /// A frozen clock is a whole-process stall the frame counter already owns. Reporting it here
    /// too would double-count one event as two independent findings.
    #[test]
    fn loading_witness_defers_to_the_frame_counter_when_the_clock_also_stops() {
        use super::loading_screen::{Sample, Witness};
        let mut witness = Witness::default();
        let frozen = Sample {
            active: 0,
            lerp_target: 0.12,
            elapsed: 500.0,
            close_gate: 0,
            already_closed: 0,
        };
        witness.observe(frozen, 1.0);
        for _ in 0..500 {
            assert_eq!(witness.observe(frozen, 1.0), None);
        }
    }

    /// Finalized, closed, or reset screens are not stalls -- they are the normal end of a load.
    #[test]
    fn loading_witness_ignores_screens_that_are_not_pending() {
        use super::loading_screen::Sample;
        let pending = Sample {
            active: 0,
            lerp_target: 0.12,
            elapsed: 10.0,
            close_gate: 0,
            already_closed: 0,
        };
        assert!(pending.is_pending());
        assert!(
            !Sample {
                close_gate: 1,
                ..pending
            }
            .is_pending(),
            "close gate set means the update is free to finish"
        );
        assert!(
            !Sample {
                already_closed: 1,
                ..pending
            }
            .is_pending(),
            "already closed is a completed load"
        );
        assert!(
            !Sample {
                active: -1,
                ..pending
            }
            .is_pending(),
            "active = -1 is the game's own reset state (0x140860d90), not a loading screen"
        );
    }

    /// A screen restarting reuses the struct; the clock going backwards must re-arm, not fire.
    #[test]
    fn loading_witness_rearms_when_the_screen_restarts() {
        use super::loading_screen::{Sample, Witness};
        let mut witness = Witness::default();
        let at = |elapsed: f32| Sample {
            active: 0,
            lerp_target: 0.12,
            elapsed,
            close_gate: 0,
            already_closed: 0,
        };
        witness.observe(at(100.0), 10.0);
        witness.observe(at(105.0), 10.0);
        assert_eq!(witness.observe(at(0.5), 10.0), None, "clock reset re-arms");
        assert_eq!(
            witness.observe(at(5.0), 10.0),
            None,
            "and starts counting afresh"
        );
    }

    /// The offsets are the load-bearing part of this oracle; pin them to what was measured.
    #[test]
    fn loading_screen_offsets_match_the_measured_layout() {
        use super::loading_screen::*;
        assert_eq!(ALREADY_CLOSED_OFFSET, 0x0c);
        assert_eq!(CLOSE_GATE_OFFSET, 0x11);
        assert_eq!(ACTIVE_OFFSET, 0x14);
        assert_eq!(LERP_START_OFFSET, 0x18);
        assert_eq!(LERP_TARGET_OFFSET, 0x1c);
        assert_eq!(LERP_DURATION_OFFSET, 0x20);
        assert_eq!(ELAPSED_OFFSET, 0x24);
        // The close gate is the high byte of the u16 the constructor zeroes at +0x10, which is
        // how the reader extracts it.
        assert_eq!(CLOSE_GATE_OFFSET & !3, 0x10);
        assert_eq!(CLOSE_GATE_OFFSET - (CLOSE_GATE_OFFSET & !3), 1);
    }

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
