//! er-telemetry: the telemetry subsystem lifted out of the product DLL.
//!
//! STATUS: skeleton + shared log/oracle scaffolding. The full body of the 8
//! telemetry source files (write_telemetry / write_game_module_oracles /
//! write_oracle / game_man_snapshot / bootstrap / save_policy_logs) is migrated
//! here file-group by file-group as the ~900-symbol ownership inversion described
//! in the extraction plan is completed. This crate depends ONLY on er-game-base +
//! upstream game libs, never on er-effects-rs (product).
//!
//! Per-tick product data enters via [`TelemetryFrameInput`] rather than a direct
//! read of the product's `EffectsState` behind its `Arc<Mutex<>>` lock, so
//! telemetry never needs the product lock type.

pub mod counters;

use std::path::PathBuf;
use std::sync::atomic::Ordering;

/// The handful of per-frame product-owned values telemetry actually reads,
/// built by the product BEFORE calling into telemetry (so telemetry never
/// touches the product's `Arc<Mutex<EffectsState>>`). Extended as write_telemetry
/// migrates over.
#[derive(Clone, Copy, Debug, Default)]
pub struct TelemetryFrameInput {
    /// Whether the local player pointer resolved this frame (product-observed).
    pub player_available: bool,
    /// Monotonic per-frame game-task tick counter (product-owned).
    pub game_task_ticks: u64,
}

/// CWD-relative artifact written by the standalone telemetry-only DLL. Distinct
/// from the product's `er-effects-telemetry.json` so a combined run keeps both.
const STANDALONE_JSON: &str = "er-telemetry-timeseries.jsonl";

fn standalone_json_path() -> PathBuf {
    er_game_base::log::game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(STANDALONE_JSON)
}

/// Read-side-only telemetry tick for the standalone `er-telemetry-dll`.
///
/// Emits exactly the subset of oracle_* fields derivable from game RAM/PE alone
/// (no product hooks, no `EffectsState`): the game module base and the three
/// stable singleton pointers. As the real oracle bodies migrate here, this grows
/// to call `write_game_module_oracles` / `write_oracle_telemetry` with an absent
/// [`TelemetryFrameInput`] and default (product-unwritten) counters.
/// Wall-clock ms since boot (GetTickCount64), 0 off-windows. Same clock the input-harness stamps into
/// `er-input-harness-phases.jsonl` (`start_tick_ms`/`end_tick_ms`), so the ORACLE can align an fps sample
/// to the harness phase it falls inside and compute per-phase fps. bd ORACLE-dll-decides-reports-2026-07-22.
fn tick_ms() -> u64 {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetTickCount64() -> u64;
        }
        unsafe { GetTickCount64() }
    }
    #[cfg(not(windows))]
    {
        0
    }
}

/// Per-core CPU + this-process CPU sampler, to test whether single-core CONTENTION (H-B) is a factor in
/// the load2 20fps (bd NEXT-telemetry-capture-per-core-cpu). Returns (max_core_busy%, cores_over_85,
/// ncores, proc_cpu_core_equivalents). Delta-based vs the previous call. -1 until it has two samples.
#[cfg(windows)]
mod cpu {
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::sync::Mutex;

    const SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION: u32 = 8;
    const ALL_PROCESSOR_GROUPS: u16 = 0xffff;
    const MAX_CORES: usize = 64;
    const SATURATED_PCT: f32 = 85.0;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct SppInfo {
        idle: i64,
        kernel: i64, // includes idle
        user: i64,
        dpc: i64,
        interrupt: i64,
        interrupt_count: u32,
        _pad: u32,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetProcessTimes(h: isize, c: *mut i64, e: *mut i64, k: *mut i64, u: *mut i64) -> i32;
        fn GetActiveProcessorCount(group: u16) -> u32;
        fn GetTickCount64() -> u64;
    }

    struct Prev {
        cores: [(i64, i64, i64); MAX_CORES], // (idle, kernel, user)
        proc_k: i64,
        proc_u: i64,
        tick: u64,
        valid: bool,
    }
    impl Prev {
        const fn new() -> Self {
            Prev {
                cores: [(0, 0, 0); MAX_CORES],
                proc_k: 0,
                proc_u: 0,
                tick: 0,
                valid: false,
            }
        }
    }
    static PREV: Mutex<Prev> = Mutex::new(Prev::new());

    pub fn sample() -> (f32, u32, u32, f32) {
        let ncores =
            (unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) } as usize).clamp(1, MAX_CORES);
        let mut buf = [SppInfo::default(); MAX_CORES];
        let mut ret = 0u32;
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION,
                buf.as_mut_ptr() as *mut c_void,
                (ncores * size_of::<SppInfo>()) as u32,
                &mut ret,
            )
        };
        let now_tick = unsafe { GetTickCount64() };
        let (mut pk, mut pu, mut d0, mut d1) = (0i64, 0i64, 0i64, 0i64);
        unsafe { GetProcessTimes(GetCurrentProcess(), &mut d0, &mut d1, &mut pk, &mut pu) };

        let Ok(mut g) = PREV.lock() else {
            return (-1.0, 0, ncores as u32, -1.0);
        };
        let (mut max_busy, mut saturated, mut proc_cpu) = (-1.0f32, 0u32, -1.0f32);
        if status == 0 && g.valid {
            for i in 0..ncores {
                let idle_d = (buf[i].idle - g.cores[i].0) as f64;
                let total =
                    (buf[i].kernel - g.cores[i].1) as f64 + (buf[i].user - g.cores[i].2) as f64;
                if total > 0.0 {
                    let busy = ((total - idle_d) / total * 100.0) as f32;
                    if busy > max_busy {
                        max_busy = busy;
                    }
                    if busy > SATURATED_PCT {
                        saturated += 1;
                    }
                }
            }
            let wall_100ns = now_tick.saturating_sub(g.tick) as f64 * 10_000.0;
            if wall_100ns > 0.0 {
                proc_cpu = (((pk - g.proc_k) + (pu - g.proc_u)) as f64 / wall_100ns) as f32;
            }
        }
        for i in 0..ncores {
            g.cores[i] = (buf[i].idle, buf[i].kernel, buf[i].user);
        }
        g.proc_k = pk;
        g.proc_u = pu;
        g.tick = now_tick;
        g.valid = true;
        (max_busy, saturated, ncores as u32, proc_cpu)
    }
}

#[cfg(not(windows))]
mod cpu {
    pub fn sample() -> (f32, u32, u32, f32) {
        (-1.0, 0, 0, -1.0)
    }
}

/// Programmatic RenderDoc frame trigger (bd RENDERDOC-inject-via-me3-native). When `renderdoc.dll` is
/// loaded into ER (as the first me3 native -- native Windows D3D12, NOT a Vulkan layer), fire
/// `TriggerCapture` at the reload's playable window so we capture the 20fps product-reload frame -- and,
/// with `ER_RENDERDOC_SLOW_MS=0`, the fast vanilla-reload frame -- agent-driven, no F12 timing. No-op
/// when `renderdoc.dll` is absent (a normal run without RENDERDOC=1).
#[cfg(windows)]
mod renderdoc {
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// RENDERDOC_API_1_4_0 function-pointer table. Layout must match renderdoc_app.h exactly. We use
    /// `SetCaptureFilePathTemplate` (index 11 -- preceded by GetAPIVersion, Set/GetCaptureOption{U32,F32},
    /// SetFocusToggleKeys, SetCaptureKeys, Get/MaskOverlayBits, RemoveHooks, UnloadCrashHandler) and
    /// `TriggerCapture` (index 15 -- preceded by GetCaptureFilePathTemplate, GetNumCaptures, GetCapture).
    #[repr(C)]
    struct Api {
        before_set_path: [usize; 11],
        set_capture_file_path_template: unsafe extern "C" fn(*const u8),
        between: [usize; 3],
        trigger_capture: unsafe extern "C" fn(),
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleA(name: *const u8) -> usize;
        fn GetProcAddress(module: usize, name: *const u8) -> usize;
    }

    static API_PTR: AtomicUsize = AtomicUsize::new(0);
    const NOT_AVAILABLE: usize = usize::MAX;

    fn resolve() -> usize {
        let h = unsafe { GetModuleHandleA(b"renderdoc.dll\0".as_ptr()) };
        if h == 0 {
            return 0;
        }
        let getapi = unsafe { GetProcAddress(h, b"RENDERDOC_GetAPI\0".as_ptr()) };
        if getapi == 0 {
            return 0;
        }
        type GetApiFn = unsafe extern "C" fn(version: u32, out: *mut *mut Api) -> i32;
        let getapi: GetApiFn = unsafe { std::mem::transmute(getapi) };
        let mut out: *mut Api = std::ptr::null_mut();
        // eRENDERDOC_API_Version_1_4_0 = 10400
        let ok = unsafe { getapi(10400, &mut out) };
        if ok != 1 || out.is_null() {
            return 0;
        }
        out as usize
    }

    static PATH_SET: AtomicUsize = AtomicUsize::new(0);

    /// Fire a RenderDoc capture of the next present. Returns true if the API was available + triggered.
    /// On the first call, points the capture-file template at `ER_RENDERDOC_CAPFILE` (else %TEMP%).
    pub fn trigger_capture() -> bool {
        let mut api = API_PTR.load(Ordering::SeqCst);
        if api == NOT_AVAILABLE {
            return false;
        }
        if api == 0 {
            api = resolve();
            API_PTR.store(if api == 0 { NOT_AVAILABLE } else { api }, Ordering::SeqCst);
            if api == 0 {
                return false;
            }
        }
        let api_ref = unsafe { &*(api as *const Api) };
        if PATH_SET
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            if let Ok(path) = std::env::var("ER_RENDERDOC_CAPFILE") {
                if let Ok(c) = std::ffi::CString::new(path) {
                    unsafe { (api_ref.set_capture_file_path_template)(c.as_ptr() as *const u8) };
                }
            }
        }
        unsafe { (api_ref.trigger_capture)() };
        true
    }
}

#[cfg(not(windows))]
mod renderdoc {
    pub fn trigger_capture() -> bool {
        false
    }
}

/// Slow-frame threshold (ms) above which an in-world frame is a capture candidate. Default 40ms (~25fps)
/// catches the 20fps reload but NOT the ~30fps boot; set `ER_RENDERDOC_SLOW_MS=0` for the fast vanilla
/// reload so its playable frame is captured too.
fn renderdoc_slow_ms() -> f32 {
    use std::sync::atomic::AtomicU32;
    static CACHED: AtomicU32 = AtomicU32::new(u32::MAX);
    let c = CACHED.load(Ordering::SeqCst);
    if c != u32::MAX {
        return f32::from_bits(c);
    }
    let v = std::env::var("ER_RENDERDOC_SLOW_MS")
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok())
        .unwrap_or(40.0);
    CACHED.store(v.to_bits(), Ordering::SeqCst);
    v
}

/// Fire a RenderDoc capture once the world has been simulating (play_time rising) for a settled window
/// AND the frame is slow enough (reload) -- throttled + capped. Returns the running capture count.
fn maybe_trigger_renderdoc(play_time_ms: i64, task_delta: f32, tick_n: u64) -> u32 {
    use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64};
    static PREV_PT: AtomicI64 = AtomicI64::new(-1);
    static STREAK: AtomicU32 = AtomicU32::new(0);
    static LAST_CAP: AtomicU64 = AtomicU64::new(0);
    static CAPS: AtomicU32 = AtomicU32::new(0);
    const MAX_CAPS: u32 = 4;
    const SETTLE_TICKS: u32 = 8; // ~32 game frames of settled in-world play before a capture
    const COOLDOWN_TICKS: u64 = 30; // ~120 game frames between captures (one per in-world window)

    let caps = CAPS.load(Ordering::SeqCst);
    if play_time_ms <= 0 {
        STREAK.store(0, Ordering::SeqCst);
        PREV_PT.store(play_time_ms, Ordering::SeqCst);
        return caps;
    }
    if caps >= MAX_CAPS {
        return caps;
    }
    let prev = PREV_PT.swap(play_time_ms, Ordering::SeqCst);
    let streak = if prev >= 0 && play_time_ms > prev {
        STREAK.fetch_add(1, Ordering::SeqCst) + 1
    } else {
        STREAK.store(0, Ordering::SeqCst);
        0
    };
    let frame_ms = task_delta * 1000.0;
    if streak >= SETTLE_TICKS
        && frame_ms >= renderdoc_slow_ms()
        && tick_n.saturating_sub(LAST_CAP.load(Ordering::SeqCst)) >= COOLDOWN_TICKS
        && renderdoc::trigger_capture()
    {
        LAST_CAP.store(tick_n, Ordering::SeqCst);
        return CAPS.fetch_add(1, Ordering::SeqCst) + 1;
    }
    caps
}

/// Game-thread sampling profiler (bd: reload 29ms is CPU-bound, present=0.2ms). A separate thread
/// suspends the game/main thread during SLOW frames (`task_delta` >= threshold) and records its RIP as an
/// RVA (rip - game_base); the histogram's top RVAs name the native function eating the reload's per-frame
/// cost. No RenderDoc / no admin needed. Dumps `er-cpu-profile.txt` to the game dir.
#[cfg(windows)]
mod profiler {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn OpenThread(access: u32, inherit: i32, tid: u32) -> isize;
        fn SuspendThread(h: isize) -> u32;
        fn ResumeThread(h: isize) -> u32;
        fn GetThreadContext(h: isize, ctx: *mut u8) -> i32;
        fn CloseHandle(h: isize) -> i32;
        fn Sleep(ms: u32);
    }
    const THREAD_GET_CONTEXT: u32 = 0x0008;
    const THREAD_SUSPEND_RESUME: u32 = 0x0002;
    const CONTEXT_CONTROL_AMD64: u32 = 0x0010_0001;
    const CTX_SIZE: usize = 0x4d0;
    const CTX_FLAGS_OFF: usize = 0x30;
    const CTX_RIP_OFF: usize = 0xf8;
    const SLOW_TASK_DELTA: f32 = 0.033; // >= ~30ms/frame (<=30fps): the reload/loading slow window
    const BUCKET: usize = 0x10; // RVA bucket granularity (instruction-ish)
    const DUMP_EVERY: u64 = 2000; // sampler iterations between dumps

    static GAME_TID: AtomicU32 = AtomicU32::new(0);
    static LAST_TD_BITS: AtomicU32 = AtomicU32::new(0);
    static GAME_BASE: AtomicUsize = AtomicUsize::new(0);
    static STARTED: AtomicUsize = AtomicUsize::new(0);
    static HIST: Mutex<Option<HashMap<usize, u32>>> = Mutex::new(None);
    static SAMPLES: AtomicUsize = AtomicUsize::new(0);

    /// Called from `standalone_tick` (which runs ON the game thread): record the thread id + latest frame
    /// time + base, and start the sampler once.
    pub fn note_frame(base: usize, task_delta: f32) {
        GAME_TID.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
        LAST_TD_BITS.store(task_delta.to_bits(), Ordering::Relaxed);
        if base != 0 {
            GAME_BASE.store(base, Ordering::Relaxed);
        }
        if STARTED.swap(1, Ordering::SeqCst) == 0 {
            *HIST.lock().unwrap() = Some(HashMap::new());
            let _ = std::thread::Builder::new()
                .name("er-cpu-sampler".into())
                .spawn(sampler_loop);
        }
    }

    fn sampler_loop() {
        let mut iters: u64 = 0;
        loop {
            let tid = GAME_TID.load(Ordering::Relaxed);
            let td = f32::from_bits(LAST_TD_BITS.load(Ordering::Relaxed));
            let base = GAME_BASE.load(Ordering::Relaxed);
            if tid != 0 && base != 0 && td >= SLOW_TASK_DELTA {
                if let Some(rip) = sample_rip(tid) {
                    if rip > base && rip - base < 0x8000_0000 {
                        let rva = (rip - base) & !(BUCKET - 1);
                        if let Ok(mut g) = HIST.lock() {
                            if let Some(h) = g.as_mut() {
                                *h.entry(rva).or_insert(0) += 1;
                            }
                        }
                        SAMPLES.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            iters += 1;
            if iters % DUMP_EVERY == 0 {
                dump();
            }
            unsafe { Sleep(1) };
        }
    }

    fn sample_rip(tid: u32) -> Option<usize> {
        let h = unsafe { OpenThread(THREAD_GET_CONTEXT | THREAD_SUSPEND_RESUME, 0, tid) };
        if h == 0 {
            return None;
        }
        // 16-byte-aligned CONTEXT buffer; we only set ContextFlags + read Rip.
        #[repr(align(16))]
        struct Ctx([u8; CTX_SIZE]);
        let mut ctx = Ctx([0u8; CTX_SIZE]);
        let p = ctx.0.as_mut_ptr();
        unsafe {
            *(p.add(CTX_FLAGS_OFF) as *mut u32) = CONTEXT_CONTROL_AMD64;
        }
        let rip = unsafe {
            if SuspendThread(h) == u32::MAX {
                CloseHandle(h);
                return None;
            }
            let ok = GetThreadContext(h, p);
            ResumeThread(h);
            let r = if ok != 0 {
                Some(*(p.add(CTX_RIP_OFF) as *const usize))
            } else {
                None
            };
            CloseHandle(h);
            r
        };
        rip
    }

    fn dump() {
        let Ok(g) = HIST.lock() else { return };
        let Some(h) = g.as_ref() else { return };
        let total = SAMPLES.load(Ordering::Relaxed).max(1);
        let mut v: Vec<(usize, u32)> = h.iter().map(|(k, c)| (*k, *c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        let mut s = format!(
            "er-cpu-profile: {total} samples of the game thread during slow frames (task_delta>={SLOW_TASK_DELTA}). Top RVAs (rva -> deobf VA = base+rva):\n"
        );
        for (rva, c) in v.iter().take(40) {
            s.push_str(&format!(
                "  0x{rva:08x}  {c:>6}  {:.1}%\n",
                100.0 * *c as f64 / total as f64
            ));
        }
        let path = er_game_base::log::game_directory_path()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("er-cpu-profile.txt");
        let _ = std::fs::write(path, s);
    }
}

#[cfg(not(windows))]
mod profiler {
    pub fn note_frame(_base: usize, _task_delta: f32) {}
}

pub fn standalone_tick() {
    let n = counters::STANDALONE_TICKS.fetch_add(1, Ordering::SeqCst) + 1;

    // Throttle disk writes to every 4th tick -- dense enough to sample the game frame time across a
    // ~3s vanilla-reload playable window (at 20fps that is ~0.2s between writes), still a snapshot.
    const WRITE_EVERY: u64 = 4;
    if n % WRITE_EVERY != 0 {
        return;
    }

    let base = er_game_base::mem::game_module_base().unwrap_or(0);
    let read_singleton = |rva: usize| -> usize {
        if base == 0 {
            return 0;
        }
        unsafe { er_game_base::mem::safe_read_usize(base + rva) }.unwrap_or(0)
    };
    let game_data_man = read_singleton(er_game_base::rva::GAME_DATA_MAN_GLOBAL_RVA);
    let game_man = read_singleton(er_game_base::rva::GAME_MAN_SINGLETON_RVA);
    let cs_menu_man = read_singleton(er_game_base::rva::CS_MENU_MAN_GLOBAL_RVA);

    // VANILLA-RELOAD FPS COMPARISON (2026-07-22): read the game's own frame timer. CSFlipperImp
    // singleton at base+0x4589ad8; task_delta (+0x268) = the game loop frame time (1/task_delta = fps),
    // fixed_spf (+0x1c) = the flip target (0.0167=60). play_time (GameDataMan+0xa0, u32 ms) rises only
    // while the world simulates -> the in-world/playable gate. Lets a telemetry-only run measure a
    // user-driven native reload's playable fps to compare against our reload path. bd
    // USER-chose-vanilla-reload-comparison-2026-07-22.
    const CS_FLIPPER_SINGLETON_RVA: usize = 0x4589ad8;
    const GAME_DATA_MAN_PLAY_TIME_A0_OFFSET: usize = 0xa0;
    let flipper = read_singleton(CS_FLIPPER_SINGLETON_RVA);
    let read_f32 = |ptr: usize, off: usize| -> f32 {
        if ptr == 0 {
            return -1.0;
        }
        unsafe { er_game_base::mem::safe_read_usize(ptr + off) }
            .map_or(-1.0, |v| f32::from_bits((v & 0xffff_ffff) as u32))
    };
    let flip_task_delta = read_f32(flipper, 0x268);
    // Feed the game-thread CPU sampler: this tick runs ON the game thread, so record its id + frame time.
    profiler::note_frame(base, flip_task_delta);
    let flip_fixed_spf = read_f32(flipper, 0x1c);
    let play_time_ms: i64 = if game_data_man == 0 {
        -1
    } else {
        unsafe {
            er_game_base::mem::safe_read_usize(game_data_man + GAME_DATA_MAN_PLAY_TIME_A0_OFFSET)
        }
        .map_or(-1, |v| i64::from((v & 0xffff_ffff) as u32))
    };

    // Per-core + this-process CPU, to test whether single-core contention (H-B) drives the load2 20fps.
    let (core_max_busy, cores_saturated, ncores, proc_cpu_cores) = cpu::sample();
    // RenderDoc: capture the reload's playable frame when running under the capture layer (no-op else).
    let renderdoc_captures = maybe_trigger_renderdoc(play_time_ms, flip_task_delta, n);
    let body = format!(
        "{{\"oracle_standalone_ticks\":{n},\
\"oracle_game_module_base\":\"0x{base:x}\",\
\"oracle_game_data_man_ptr\":\"0x{game_data_man:x}\",\
\"oracle_game_man_ptr\":\"0x{game_man:x}\",\
\"oracle_cs_menu_man_ptr\":\"0x{cs_menu_man:x}\",\
\"oracle_flip_task_delta\":{flip_task_delta:.6},\
\"oracle_flip_fixed_spf\":{flip_fixed_spf:.6},\
\"oracle_play_time_ms\":{play_time_ms},\
\"oracle_tick_ms\":{tick_ms},\
\"oracle_core_max_busy\":{core_max_busy:.1},\
\"oracle_cores_saturated\":{cores_saturated},\
\"oracle_ncores\":{ncores},\
\"oracle_proc_cpu_cores\":{proc_cpu_cores:.3},\
\"oracle_renderdoc_captures\":{renderdoc_captures}}}\n",
        tick_ms = tick_ms()
    );
    // APPEND one JSON line per write -> a timeseries jsonl the agent reads AFTER the run (no polling,
    // no sleep). body already ends in '\n'.
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(standalone_json_path())
    {
        let _ = f.write_all(body.as_bytes());
    }
}
