//! Boot-sequence CPU profiler: an INDEPENDENT sampler thread that records, over the whole boot,
//! per-thread CPU work (high-res cycles via `QueryThreadCycleTime` + absolute kernel/user time via
//! `GetThreadTimes`) and, optionally, the instruction pointer of each thread (`GetThreadContext`).
//!
//! Why a standalone thread (not the game task): the ~10s engine-init gap happens BEFORE
//! `CSTaskImp::instance()` resolves, i.e. before our recurring game task ticks. A separate sampler
//! observes every OS thread regardless of our task state, so it sees the engine's own init threads
//! during that gap. The per-thread cycle/CPU-time timeline is what reveals MISSED PARALLELISM: one
//! thread pegged while N-1 cores sit idle for seconds is a serialized bottleneck.
//!
//! Two layers, separately gated:
//!   * CPU-time sampling (DEFAULT when profiler on): NO thread suspension. Pure `QueryThreadCycleTime`
//!     + `GetThreadTimes` reads -> safe, cannot perturb the game. This answers "where does wall-clock
//!     go and is each phase CPU-bound or wait-bound, and is it parallelized".
//!   * RIP sampling (`ER_EFFECTS_PROFILE_RIP=1`, OFF by default): `SuspendThread`+`GetThreadContext`
//!     to capture each thread's Rip -> hot-function attribution (symbolized offline via the Ghidra
//!     dump). Suspension is heavier and could be noticed by anti-tamper, so it is opt-in.
//!
//! Output: one JSON object per sample, newline-delimited, to `ER_EFFECTS_PROFILE_PATH`
//! (default `<game_dir>/er-effects-profile.jsonl`). The offline renderer
//! (`scripts/boot-profile-render.py`) diffs consecutive samples per thread.

#![allow(clippy::too_many_lines)]

use std::{
    collections::HashMap,
    fmt::Write as _,
    fs,
    io::Write as _,
    path::PathBuf,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use windows::Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE},
    System::{
        Diagnostics::{
            Debug::GetThreadContext,
            ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
        },
        SystemInformation::{GetSystemInfo, SYSTEM_INFO},
        Threading::{
            GetCurrentProcessId, GetCurrentThreadId, GetThreadDescription, GetThreadTimes,
            OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION,
            THREAD_SUSPEND_RESUME,
        },
        WindowsProgramming::QueryThreadCycleTime,
    },
};
use windows::Win32::System::Diagnostics::Debug::{CONTEXT, CONTEXT_FLAGS};

use super::*;

/// AMD64 `CONTEXT_CONTROL` (the segment-regs/IP/SP subset). The `windows` crate only exposes the
/// generic `CONTEXT_CONTROL` for x86; on x86_64 the value is `0x0010_0001`. We only need `Rip`.
const CONTEXT_CONTROL_AMD64: u32 = 0x0010_0001;

/// Profiler master switch: env `ER_EFFECTS_PROFILE=1` or `<game_dir>/er-effects-profile.txt`.
pub(crate) fn profiler_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_PROFILE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-profile.txt")
            .exists()
}

/// RIP-sampling sub-switch (suspends threads). OFF unless `ER_EFFECTS_PROFILE_RIP=1` or the file.
pub(crate) fn profiler_rip_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_PROFILE_RIP").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-profile-rip.txt")
            .exists()
}

fn profile_path() -> PathBuf {
    std::env::var("ER_EFFECTS_PROFILE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            game_directory_path()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("er-effects-profile.jsonl")
        })
}

/// Sampling cadence (ms). `QueryThreadCycleTime` is high-resolution so ~25ms gives a smooth
/// utilization curve without flooding the file (whole boot ~40s -> ~1600 samples).
fn sample_interval_ms() -> u64 {
    std::env::var("ER_EFFECTS_PROFILE_INTERVAL_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&v| v >= 1)
        .unwrap_or(25)
}

/// Sample RIP every Nth CPU sample (suspension is heavier). Default: every 4th (~100ms at 25ms base).
fn rip_every_n() -> u64 {
    std::env::var("ER_EFFECTS_PROFILE_RIP_EVERY")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&v| v >= 1)
        .unwrap_or(4)
}

/// Hard stop for the sampler (s). Bounds the file even if teardown is missed. Default 120s.
fn max_runtime_s() -> u64 {
    std::env::var("ER_EFFECTS_PROFILE_MAX_S")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&v| v >= 1)
        .unwrap_or(120)
}

fn filetime_to_100ns(ft: FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | (ft.dwLowDateTime as u64)
}

/// Best-effort thread name via `GetThreadDescription` (FromSoft names several engine threads).
unsafe fn thread_name(handle: HANDLE) -> Option<String> {
    let pwstr = unsafe { GetThreadDescription(handle) }.ok()?;
    if pwstr.is_null() {
        return None;
    }
    // SAFETY: GetThreadDescription returns a LocalAlloc'd, NUL-terminated UTF-16 string.
    let s = unsafe { pwstr.to_string() }.ok().filter(|s| !s.is_empty());
    // The buffer must be freed with LocalFree; leaking a few short strings during a bounded boot
    // probe is acceptable and avoids a second FFI import. (Names are captured once and cached.)
    s
}

/// Enumerate this process's thread IDs via a ToolHelp snapshot (read-only; does not open threads).
unsafe fn enumerate_thread_ids(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let snapshot = match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) } {
        Ok(h) => h,
        Err(_) => return out,
    };
    let mut entry = THREADENTRY32 {
        dwSize: core::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    if unsafe { Thread32First(snapshot, &mut entry) }.is_ok() {
        loop {
            if entry.th32OwnerProcessID == pid {
                out.push(entry.th32ThreadID);
            }
            entry.dwSize = core::mem::size_of::<THREADENTRY32>() as u32;
            if unsafe { Thread32Next(snapshot, &mut entry) }.is_err() {
                break;
            }
        }
    }
    let _ = unsafe { CloseHandle(snapshot) };
    out
}

struct ThreadSample {
    tid: u32,
    cycles: u64,
    kernel_100ns: u64,
    user_100ns: u64,
    rip: Option<u64>,
}

/// Public entry: spawn the sampler daemon thread. Idempotent via the `Once` in the caller.
pub(crate) fn spawn_boot_profiler() {
    let _ = std::thread::Builder::new()
        .name("er-effects-profiler".to_owned())
        .spawn(profiler_main);
}

fn profiler_main() {
    let pid = unsafe { GetCurrentProcessId() };
    let self_tid = unsafe { GetCurrentThreadId() };
    let rip_on = profiler_rip_enabled();
    let interval = Duration::from_millis(sample_interval_ms());
    let rip_n = rip_every_n();
    let max = Duration::from_secs(max_runtime_s());

    let ncpu = {
        let mut si = SYSTEM_INFO::default();
        unsafe { GetSystemInfo(&mut si) };
        si.dwNumberOfProcessors
    };

    let path = profile_path();
    let mut file = match fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            append_autoload_debug(format_args!("profiler: cannot open {path:?}: {e}"));
            return;
        }
    };
    // Header line documents the run for the offline renderer. `module_base` lets RIP samples be made
    // eldenring.exe-relative offline (0 if not yet resolvable at profiler start -- the renderer then
    // falls back to the readiness-result runtime_module_base).
    let module_base = game_module_base().unwrap_or(0);
    let _ = writeln!(
        file,
        "{{\"kind\":\"header\",\"ncpu\":{ncpu},\"interval_ms\":{},\"rip\":{},\"pid\":{pid},\"module_base\":{module_base}}}",
        interval.as_millis(),
        rip_on
    );
    append_autoload_debug(format_args!(
        "profiler: started ncpu={ncpu} interval_ms={} rip={rip_on} -> {path:?}",
        interval.as_millis()
    ));

    // Cache thread names so we resolve each only once (the description rarely changes).
    let mut names: HashMap<u32, String> = HashMap::new();
    let epoch = Instant::now();
    let mut iter: u64 = 0;

    while epoch.elapsed() < max {
        let ms = epoch.elapsed().as_millis();
        let do_rip = rip_on && (iter % rip_n == 0);
        let tids = unsafe { enumerate_thread_ids(pid) };
        let mut samples: Vec<ThreadSample> = Vec::with_capacity(tids.len());

        for tid in tids {
            if tid == self_tid {
                continue; // never sample/suspend ourselves
            }
            let mut desired = THREAD_QUERY_INFORMATION;
            if do_rip {
                desired |= THREAD_GET_CONTEXT | THREAD_SUSPEND_RESUME;
            }
            let Ok(handle) = (unsafe { OpenThread(desired, false, tid) }) else {
                continue;
            };

            let mut cycles: u64 = 0;
            let _ = unsafe { QueryThreadCycleTime(handle, &mut cycles) };
            let (mut k, mut u) = (FILETIME::default(), FILETIME::default());
            let (mut c0, mut e0) = (FILETIME::default(), FILETIME::default());
            let _ = unsafe { GetThreadTimes(handle, &mut c0, &mut e0, &mut k, &mut u) };

            let mut rip: Option<u64> = None;
            if do_rip {
                // Suspend, read Rip, resume immediately. Skip if suspend fails (terminating thread).
                let prev = unsafe { SuspendThread(handle) };
                if prev != u32::MAX {
                    let mut ctx = CONTEXT {
                        ContextFlags: CONTEXT_FLAGS(CONTEXT_CONTROL_AMD64),
                        ..Default::default()
                    };
                    if unsafe { GetThreadContext(handle, &mut ctx) }.is_ok() {
                        rip = Some(ctx.Rip);
                    }
                    let _ = unsafe { ResumeThread(handle) };
                }
            }

            if let std::collections::hash_map::Entry::Vacant(slot) = names.entry(tid) {
                if let Some(n) = unsafe { thread_name(handle) } {
                    slot.insert(n);
                }
            }

            samples.push(ThreadSample {
                tid,
                cycles,
                kernel_100ns: filetime_to_100ns(k),
                user_100ns: filetime_to_100ns(u),
                rip,
            });
            let _ = unsafe { CloseHandle(handle) };
        }

        // Emit one compact JSON line for this sample.
        let mut line = String::with_capacity(64 + samples.len() * 48);
        let _ = write!(line, "{{\"ms\":{ms},\"t\":[");
        for (i, s) in samples.iter().enumerate() {
            if i > 0 {
                line.push(',');
            }
            let _ = write!(
                line,
                "{{\"id\":{},\"cy\":{},\"k\":{},\"u\":{}",
                s.tid, s.cycles, s.kernel_100ns, s.user_100ns
            );
            if let Some(rip) = s.rip {
                let _ = write!(line, ",\"rip\":{rip}");
            }
            if let Some(name) = names.get(&s.tid) {
                let _ = write!(line, ",\"n\":\"{}\"", json_escape(name));
            }
            line.push('}');
        }
        line.push_str("]}");
        let _ = writeln!(file, "{line}");

        iter = iter.wrapping_add(1);
        std::thread::sleep(interval);
    }

    let _ = file.flush();
    append_autoload_debug(format_args!(
        "profiler: stopped after {}ms ({} samples)",
        epoch.elapsed().as_millis(),
        iter
    ));
}

/// Minimal JSON string escaping for thread names (ASCII engine names in practice).
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}
