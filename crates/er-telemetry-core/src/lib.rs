//! er-telemetry-core: the telemetry subsystem lifted out of the product DLL.
//!
//! STATUS: skeleton + shared log/oracle scaffolding. The full body of the 8
//! telemetry source files (write_telemetry / write_game_module_oracles /
//! write_oracle / game_man_snapshot / bootstrap / save_policy_logs) is migrated
//! here file-group by file-group as the ~900-symbol ownership inversion described
//! in the extraction plan is completed. This crate depends only on er-game-base +
//! upstream game libs, never on er-quickload (product).
//!
//! Per-tick product data enters via [`TelemetryFrameInput`] rather than a direct
//! read of the product's `EffectsState` behind its `Arc<Mutex<>>` lock, so
//! telemetry never needs the product lock type.

pub mod counters;
pub mod load_count;
pub mod log_channels;
mod read;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// The handful of per-frame product-owned values telemetry actually reads,
/// built by the product before calling into telemetry (so telemetry never
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
/// from the product's `er-quickload-telemetry.json` so a combined run keeps both.
const STANDALONE_JSON: &str = "er-telemetry-timeseries.jsonl";

/// This run's timeseries: the launcher's redirect if it set one, else beside `eldenring.exe`.
///
/// Without the knob the file was single-slot in the game directory, so two launches lost the run
/// before last — and the tick stamps are per-run, which makes a stale timeseries worse than a
/// missing one. Resolution lives in `er_game_base::log`, shared with every other per-run artifact.
fn standalone_json_path() -> PathBuf {
    er_game_base::log::redirected_artifact_path("ER_QUICKLOAD_TIMESERIES_PATH", STANDALONE_JSON)
}

/// Read-side-only telemetry tick for the standalone `er-telemetry`.
///
/// Emits exactly the subset of oracle_* fields derivable from game RAM/PE alone
/// (no product hooks, no `EffectsState`): the game module base and the three
/// stable singleton pointers. As the real oracle bodies migrate here, this grows
/// to call `write_game_module_oracles` / `write_oracle_telemetry` with an absent
/// [`TelemetryFrameInput`] and default (product-unwritten) counters.
/// Wall-clock ms since boot (GetTickCount64), 0 off-windows. Same clock the input-harness stamps into
/// `er-input-harness-phases.jsonl` (`start_tick_ms`/`end_tick_ms`), so the oracle can align an fps sample
/// to the harness phase it falls inside and compute per-phase fps. bd oracle-dll-decides-reports-2026-07-22.
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

/// Per-core CPU + this-process CPU sampler, to test whether single-core contention (H-B) is a factor in
/// the load2 20fps (bd next-telemetry-capture-per-core-cpu). Returns (max_core_busy%, cores_over_85,
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
            for (cur, prev) in buf.iter().zip(g.cores.iter()).take(ncores) {
                let idle_d = (cur.idle - prev.0) as f64;
                let total = (cur.kernel - prev.1) as f64 + (cur.user - prev.2) as f64;
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
        for (prev, cur) in g.cores.iter_mut().zip(buf.iter()).take(ncores) {
            *prev = (cur.idle, cur.kernel, cur.user);
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
/// loaded into ER (as the first me3 native -- native Windows D3D12, not a Vulkan layer), fire
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

    fn resolve() -> usize {
        let h = unsafe { GetModuleHandleA(c"renderdoc.dll".as_ptr().cast::<u8>()) };
        if h == 0 {
            return 0;
        }
        let getapi = unsafe { GetProcAddress(h, c"RENDERDOC_GetAPI".as_ptr().cast::<u8>()) };
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
        if api == 0 {
            // RE-check each call until found (do not permanently cache "absent"): RenderDoc may be injected
            // after boot via `renderdoccmd inject --PID` (the native-Windows capture path -- injecting at
            // boot/device-creation stalls ER, bd STEP4-me3-native-renderdoc-dll-stalls-boot). trigger_capture
            // only runs on slow steady frames, so the GetModuleHandle re-check is negligible. Cache only the
            // positive result.
            api = resolve();
            if api != 0 {
                API_PTR.store(api, Ordering::SeqCst);
            }
            if api == 0 {
                return false;
            }
        }
        let api_ref = unsafe { &*(api as *const Api) };
        if PATH_SET
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            && let Ok(path) = std::env::var("ER_RENDERDOC_CAPFILE")
            && let Ok(c) = std::ffi::CString::new(path)
        {
            unsafe { (api_ref.set_capture_file_path_template)(c.as_ptr() as *const u8) };
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
/// catches the 20fps reload but not the ~30fps boot; set `ER_RENDERDOC_SLOW_MS=0` for the fast vanilla
/// reload so its playable frame is captured too.
fn renderdoc_slow_ms() -> f32 {
    use std::sync::atomic::AtomicU32;
    static CACHED: AtomicU32 = AtomicU32::new(u32::MAX);
    let c = CACHED.load(Ordering::SeqCst);
    if c != u32::MAX {
        return f32::from_bits(c);
    }
    // Prefer a game-DIR marker file (er-quickload-rdoc-slow-ms.txt): env does not propagate through
    // me3/Proton to the game process (bd correction-RenderDoc...), so a marker is the reliable way to set a
    // low threshold that captures the fast vanilla/mod reload (16-18ms) for the per-pass GPU A/B diff.
    let v = std::fs::read_to_string("er-quickload-rdoc-slow-ms.txt")
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok())
        .or_else(|| {
            std::env::var("ER_RENDERDOC_SLOW_MS")
                .ok()
                .and_then(|s| s.trim().parse::<f32>().ok())
        })
        .unwrap_or(40.0);
    CACHED.store(v.to_bits(), Ordering::SeqCst);
    v
}

/// Fire a RenderDoc capture once the world has been simulating (play_time rising) for a settled window
/// and the frame is slow enough (reload) -- throttled + capped. Returns the running capture count.
fn maybe_trigger_renderdoc(play_time_ms: i64, task_delta: f32, _tick_n: u64) -> u32 {
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32};
    static PREV_PT: AtomicI64 = AtomicI64::new(-1);
    static STREAK: AtomicU32 = AtomicU32::new(0);
    static FLAT: AtomicU32 = AtomicU32::new(0); // consecutive ticks play_time did not advance
    static ARMED: AtomicBool = AtomicBool::new(true); // eligible to capture once this in-world window
    static CAPS: AtomicU32 = AtomicU32::new(0);
    const MAX_CAPS: u32 = 6; // load1 + 2 reloads + headroom
    const SETTLE_TICKS: u32 = 8; // ~32 game frames of settled in-world play before a capture
    const LOADING_GAP_TICKS: u32 = 10; // play_time flat this long = a load boundary -> re-arm one capture

    let caps = CAPS.load(Ordering::SeqCst);
    if caps >= MAX_CAPS {
        return caps;
    }
    let prev = PREV_PT.swap(play_time_ms, Ordering::SeqCst);
    // One capture per in-world window (fixes "4x load1, 0x reload" -- MAX_CAPS was burned inside load1's
    // window before the quit->reload). play_time not advancing = a load/loading pause; a sustained flat
    // window (>= LOADING_GAP_TICKS) is a load boundary that RE-arms the next window's single capture, so
    // we get load1 and each reload (a single in-world hiccup does not re-arm).
    if play_time_ms <= 0 || !(prev >= 0 && play_time_ms > prev) {
        STREAK.store(0, Ordering::SeqCst);
        if FLAT.fetch_add(1, Ordering::SeqCst) + 1 >= LOADING_GAP_TICKS {
            ARMED.store(true, Ordering::SeqCst);
        }
        return caps;
    }
    FLAT.store(0, Ordering::SeqCst);
    let streak = STREAK.fetch_add(1, Ordering::SeqCst) + 1;
    let frame_ms = task_delta * 1000.0;
    if streak >= SETTLE_TICKS
        && frame_ms >= renderdoc_slow_ms()
        && ARMED.load(Ordering::SeqCst)
        && renderdoc::trigger_capture()
    {
        ARMED.store(false, Ordering::SeqCst); // one capture per in-world window
        return CAPS.fetch_add(1, Ordering::SeqCst) + 1;
    }
    caps
}

/// Game-thread sampling profiler (bd: reload 29ms is CPU-bound, present=0.2ms). A separate thread
/// suspends the game/main thread during slow frames (`task_delta` >= threshold) and records its RIP as an
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

    /// Called from `standalone_tick` (which runs on the game thread): record the thread id + latest frame
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
            if tid != 0
                && base != 0
                && td >= SLOW_TASK_DELTA
                && let Some(rip) = sample_rip(tid)
                && rip > base
                && rip - base < 0x8000_0000
            {
                let rva = (rip - base) & !(BUCKET - 1);
                if let Ok(mut g) = HIST.lock()
                    && let Some(h) = g.as_mut()
                {
                    *h.entry(rva).or_insert(0) += 1;
                }
                SAMPLES.fetch_add(1, Ordering::Relaxed);
            }
            iters += 1;
            if iters.is_multiple_of(DUMP_EVERY) {
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
        // 16-byte-aligned context buffer; we only set ContextFlags + read Rip.
        #[repr(align(16))]
        struct Ctx([u8; CTX_SIZE]);
        let mut ctx = Ctx([0u8; CTX_SIZE]);
        let p = ctx.0.as_mut_ptr();
        unsafe {
            *(p.add(CTX_FLAGS_OFF) as *mut u32) = CONTEXT_CONTROL_AMD64;
        }

        unsafe {
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
        }
    }

    fn dump() {
        let Ok(g) = HIST.lock() else { return };
        let Some(h) = g.as_ref() else { return };
        let total = SAMPLES.load(Ordering::Relaxed).max(1);
        let mut v: Vec<(usize, u32)> = h.iter().map(|(k, c)| (*k, *c)).collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.1));
        let mut s = format!(
            "er-cpu-profile: {total} samples of the game thread during slow frames (task_delta>={SLOW_TASK_DELTA}). Top RVAs (rva -> deobf VA = base+rva):\n"
        );
        for (rva, c) in v.iter().take(40) {
            s.push_str(&format!(
                "  0x{rva:08x}  {c:>6}  {:.1}%\n",
                100.0 * *c as f64 / total as f64
            ));
        }
        // Redirectable like every other per-run artifact, and it needs it more than most: this
        // one is a bare `fs::write`, so it keeps zero previous generations — the run before this
        // one is gone the instant this one dumps, with no `.prev` to fall back on.
        let path = er_game_base::log::redirected_artifact_path(
            "ER_QUICKLOAD_CPU_PROFILE_PATH",
            "er-cpu-profile.txt",
        );
        let _ = std::fs::write(path, s);
    }
}

#[cfg(not(windows))]
mod profiler {
    pub fn note_frame(_base: usize, _task_delta: f32) {}
}

/// Fields every sample carries even when unchanged: the row's identity and its time axis. A row
/// that could not be placed on the clock is not a sample of anything.
const ALWAYS_SAMPLED: [&str; 2] = ["oracle_standalone_ticks", "oracle_tick_ms"];

/// Render one JSONL record, omitting fields byte-identical to the previously written record.
///
/// The omission is keyed on the value, never on the field name: a field is dropped only when the
/// exact bytes this record would have written are the bytes the last record already carries, so a
/// reader that carries values forward reconstructs the full series losslessly, and a reader that
/// filters (all three in `scripts/`) sees exactly the transitions.
fn render_sample(fields: &[(&str, String)]) -> String {
    static PREVIOUS: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);
    let mut previous = PREVIOUS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    render_sample_against(fields, &mut previous)
}

/// The whole of [`render_sample`] except for where the previous record is kept. Split out so the
/// tests own their `previous` instead of racing each other through one process-wide static -- the
/// elision is a function of the last record, and a test that cannot choose the last record is
/// testing whichever test got there first.
fn render_sample_against(fields: &[(&str, String)], previous: &mut Option<Vec<String>>) -> String {
    // Length mismatch means the field list changed under a running process (it cannot, but a
    // stale vector would silently mis-pair names to values, which is worse than a fat record).
    let last = previous.as_ref().filter(|last| last.len() == fields.len());
    let mut body = String::from("{");
    for (index, (name, value)) in fields.iter().enumerate() {
        let unchanged = last.is_some_and(|last| last[index] == *value);
        if unchanged && !ALWAYS_SAMPLED.contains(name) {
            continue;
        }
        if body.len() > 1 {
            body.push(',');
        }
        body.push('"');
        body.push_str(name);
        body.push_str("\":");
        body.push_str(value);
    }
    body.push_str("}\n");
    *previous = Some(fields.iter().map(|(_, value)| value.clone()).collect());
    body
}

/// `oracle_flip_*` for "the CSFlipperImp global has no mapping on this build", as against `-1.0`
/// for "it resolved and the object is not there yet". See [`singleton_field`] for why the two are
/// worth separating.
const UNRESOLVED_F32: f32 = -2.0;
/// `oracle_play_time_ms` for "GameDataMan has no mapping on this build", as against `-1` for
/// "resolved, no character loaded". Both are non-positive, so every existing reader treats them
/// alike; only a human or a future gate reading the series can tell them apart, which is the point.
const UNRESOLVED_I64: i64 = -2;
/// Render a singleton pointer oracle: its address, or `"unmapped"` when the RVA has no mapping for
/// the running build.
///
/// # Why not just print `0x0`
///
/// Because that is what happened, and it hid for a whole run. `oracle_game_man_ptr` and
/// `oracle_cs_menu_man_ptr` were `"0x0"` in all 4,350 records of `br-20260831-160354-2513` -- not
/// because the game had no GameMan or CSMenuMan, but because the reads were aimed at 1.16.2
/// addresses that 1.17 leaves blank. `"0x0"` is also the honest answer for a global the game has
/// not built yet, so nothing downstream could tell a dead address from an early sample, and the
/// two fields sat there looking like ordinary boot noise for the length of the file.
///
/// A distinct token cannot be mistaken for either. It is a string, and every reader of these two
/// fields today is a human reading the series, so it costs no parser.
fn singleton_field(pointer: Option<usize>) -> String {
    match pointer {
        None => "\"unmapped\"".to_string(),
        Some(address) => format!("\"0x{address:x}\""),
    }
}

pub fn standalone_tick() {
    let n = counters::STANDALONE_TICKS.fetch_add(1, Ordering::SeqCst) + 1;

    // Throttle disk writes so the series stays dense enough to sample the game frame time across a
    // ~3s vanilla-reload playable window -- ~0.2s between writes -- but no denser.
    //
    // The floor is in time, not in ticks. This used to be `every 4th tick`, a frame-count proxy for
    // that 0.2s which tightens exactly when the game is healthy: 0.2s at 20fps, 0.067s at 60fps. So
    // the file grew three times denser than its own stated requirement precisely when nothing
    // interesting was happening. Measured on run `br-20260831-160354-2513`: 4,350 records at a
    // 115ms median -- already denser than the design target -- for 4.43 MB across 514s (31 MB/h),
    // plus a 29 MB `.prev` beside it. A wall-clock floor delivers the documented spacing at any
    // framerate.
    const MIN_SAMPLE_SPACING_MS: u64 = 200;
    let tick_ms = tick_ms();
    {
        static LAST_SAMPLE_MS: AtomicU64 = AtomicU64::new(0);
        let last = LAST_SAMPLE_MS.load(Ordering::SeqCst);
        // `0` is the never-sampled sentinel, so the first tick always writes and establishes the
        // epoch. `saturating_sub` keeps a backwards clock from wedging the series shut forever.
        if last != 0 && tick_ms.saturating_sub(last) < MIN_SAMPLE_SPACING_MS {
            return;
        }
        LAST_SAMPLE_MS.store(tick_ms.max(1), Ordering::SeqCst);
    }

    let base = er_game_base::mem::game_module_base().unwrap_or(0);
    // Every singleton read asks where the global lives on this build. This closure was
    // `safe_read_usize(base + rva)` until 2026-08-31, and that one line is why four of the fields
    // below were byte-identical in all 4,350 records of run `br-20260831-160354-2513`.
    //
    // A raw `base + rva` is a read of the 1.16.2 slot, and every `.data` global moved on 1.17:
    // GameDataMan 0x3d5df38 -> 0x3d61f98, GameMan 0x3d69918 -> 0x3d6d988, CSMenuMan
    // 0x3d6b7b0 -> 0x3d6f820, CSFlipperImp 0x4589ad8 -> 0x458db58. The reads did not FAIL -- they
    // succeeded against whatever now occupies the stale address, and the series recorded exactly
    // what that was: `oracle_game_data_man_ptr` was `0x6e614d6e6f697463` in every single record,
    // which is little-endian ASCII `"ctionMan"` -- eight bytes out of the middle of the RTTI type
    // name `.?AVNWSteamConnectionManager@DLNW3@@`, which 1.17 parks where GameDataMan used to be.
    // The other three stale slots landed in still-blank `.data` and read `0x0`, which is
    // indistinguishable from a global the game has not created yet. That is the whole hazard: a
    // wrong pointer oracle does not go quiet, it goes constant, and a constant is invisible.
    //
    // `game_data_addr` translates through the verified 1.16.2 -> 1.17 map and answers `0` for an
    // address with no mapping, which `safe_read_usize` then fails on -- so the next stale RVA
    // reaches the UNMAPPED sentinels below instead of a plausible number.
    let read_singleton = |rva: usize, what: &'static str| -> Option<usize> {
        if base == 0 {
            return None;
        }
        let address = er_game_base::mem::game_data_addr(base, rva, what);
        if address == 0 {
            return None;
        }
        unsafe { er_game_base::mem::safe_read_usize(address) }
    };
    // `None` = the module base or the RVA could not be resolved; `Some(0)` = resolved, and the
    // game has genuinely not populated the global yet. Keeping those apart is the point.
    let game_data_man = read_singleton(
        er_game_base::rva::GAME_DATA_MAN_GLOBAL_RVA,
        "GAME_DATA_MAN_GLOBAL_RVA",
    );
    let game_man = read_singleton(
        er_game_base::rva::GAME_MAN_SINGLETON_RVA,
        "GAME_MAN_SINGLETON_RVA",
    );
    let cs_menu_man = read_singleton(
        er_game_base::rva::CS_MENU_MAN_GLOBAL_RVA,
        "CS_MENU_MAN_GLOBAL_RVA",
    );

    // Vanilla-reload FPS comparison (2026-07-22): read the game's own frame timer. CSFlipperImp
    // singleton at 1.16.2 base+0x4589ad8; task_delta (+0x268) = the game loop frame time
    // (1/task_delta = fps), fixed_spf (+0x1c) = the flip target (0.0167=60). play_time
    // (GameDataMan+0xa0, u32 ms) rises only while the world simulates -> the in-world/playable
    // gate. Lets a telemetry-only run measure a user-driven native reload's playable fps to
    // compare against our reload path. bd user-chose-vanilla-reload-comparison-2026-07-22.
    //
    // Both OFFSETS are 1.17-confirmed and neither moved: `GetPlayTime` (1.16.2 `0x1402565d0`,
    // 1.17 `0x1402565a0`) is `mov rax,[GameDataMan]; ...; mov eax,[rax+0xa0]` in both images,
    // byte-identical apart from the rip displacement. Only the global moved.
    const CS_FLIPPER_SINGLETON_RVA: usize = 0x4589ad8;
    const GAME_DATA_MAN_PLAY_TIME_A0_OFFSET: usize = 0xa0;
    let flipper = read_singleton(CS_FLIPPER_SINGLETON_RVA, "CS_FLIPPER_SINGLETON_RVA");
    // `-2.0` = the CSFlipperImp global could not be resolved for this build; `-1.0` = it resolved
    // and the object is not there (or the field read faulted). Every reader of these fields
    // (`analyze-core-contention.py`, `report-harness-phases.py`, `analyze-vanilla-reload-fps.py`)
    // filters on a positive value, so the second sentinel costs them nothing and buys a run's
    // telemetry the ability to say which of the two happened.
    let read_f32 = |ptr: Option<usize>, off: usize| -> f32 {
        match ptr {
            None => UNRESOLVED_F32,
            Some(0) => -1.0,
            Some(p) => unsafe { er_game_base::mem::safe_read_usize(p + off) }
                .map_or(-1.0, |v| f32::from_bits((v & 0xffff_ffff) as u32)),
        }
    };
    let flip_task_delta = read_f32(flipper, 0x268);
    // Feed the game-thread CPU sampler: this tick runs on the game thread, so record its id + frame time.
    profiler::note_frame(base, flip_task_delta);
    let flip_fixed_spf = read_f32(flipper, 0x1c);
    let play_time_ms: i64 = match game_data_man {
        None => UNRESOLVED_I64,
        Some(0) => -1,
        Some(gdm) => {
            unsafe { er_game_base::mem::safe_read_usize(gdm + GAME_DATA_MAN_PLAY_TIME_A0_OFFSET) }
                .map_or(-1, |v| i64::from((v & 0xffff_ffff) as u32))
        }
    };

    // Per-core + this-process CPU, to test whether single-core contention (H-B) drives the load2 20fps.
    let (core_max_busy, cores_saturated, ncores, proc_cpu_cores) = cpu::sample();
    // RenderDoc: capture the reload's playable frame when running under the capture layer (no-op else).
    let renderdoc_captures = maybe_trigger_renderdoc(play_time_ms, flip_task_delta, n);
    let winreconfig_create_window_calls =
        counters::WINRECONFIG_CREATE_WINDOW_CALLS.load(Ordering::SeqCst);
    let winreconfig_set_window_pos_calls =
        counters::WINRECONFIG_SET_WINDOW_POS_CALLS.load(Ordering::SeqCst);
    let winreconfig_set_window_long_calls =
        counters::WINRECONFIG_SET_WINDOW_LONG_CALLS.load(Ordering::SeqCst);
    let winreconfig_move_window_calls =
        counters::WINRECONFIG_MOVE_WINDOW_CALLS.load(Ordering::SeqCst);
    let winreconfig_change_display_calls =
        counters::WINRECONFIG_CHANGE_DISPLAY_CALLS.load(Ordering::SeqCst);
    let winreconfig_last_set_pos_size =
        counters::WINRECONFIG_LAST_SET_POS_SIZE.load(Ordering::SeqCst);
    let winreconfig_last_set_pos_flags =
        counters::WINRECONFIG_LAST_SET_POS_FLAGS.load(Ordering::SeqCst);
    let winreconfig_last_move_size = counters::WINRECONFIG_LAST_MOVE_SIZE.load(Ordering::SeqCst);
    let winreconfig_last_change_display_size =
        counters::WINRECONFIG_LAST_CHANGE_DISPLAY_SIZE.load(Ordering::SeqCst);
    let winreconfig_last_change_display_flags =
        counters::WINRECONFIG_LAST_CHANGE_DISPLAY_FLAGS.load(Ordering::SeqCst);
    let winreconfig_early_apply_result =
        counters::WINRECONFIG_EARLY_APPLY_RESULT.load(Ordering::SeqCst);
    let winreconfig_early_apply_ms = counters::WINRECONFIG_EARLY_APPLY_MS.load(Ordering::SeqCst);
    let winreconfig_early_apply_rect =
        counters::WINRECONFIG_EARLY_APPLY_RECT.load(Ordering::SeqCst);
    // One field per row, so a field that did not move costs nothing. `render_sample` omits any
    // field whose rendered value is byte-identical to the previously written record. Measured on
    // run `br-20260831-160354-2513`: of 27 fields across 4,350 records, 18 never changed once and
    // the 13 `oracle_winreconfig_*` counters alone were 58% of every line's bytes while changing
    // at most twice all run. Every reader in the repo reaches these through `dict.get(...)` with a
    // filter (`scripts/analyze-core-contention.py`, `analyze-vanilla-reload-fps.py`,
    // `report-harness-phases.py`), so an absent field reads as "no new sample", which is exactly
    // what it means. A field that genuinely varies per sample is never elided, so this tightens
    // itself now that the singleton reads below go through the resolver: six of those 18 frozen
    // fields were frozen because the reads were unresolved, not because nothing was happening.
    let fields = [
        ("oracle_standalone_ticks", n.to_string()),
        ("oracle_game_module_base", format!("\"0x{base:x}\"")),
        ("oracle_game_data_man_ptr", singleton_field(game_data_man)),
        ("oracle_game_man_ptr", singleton_field(game_man)),
        ("oracle_cs_menu_man_ptr", singleton_field(cs_menu_man)),
        ("oracle_flip_task_delta", format!("{flip_task_delta:.6}")),
        ("oracle_flip_fixed_spf", format!("{flip_fixed_spf:.6}")),
        ("oracle_play_time_ms", play_time_ms.to_string()),
        ("oracle_tick_ms", tick_ms.to_string()),
        ("oracle_core_max_busy", format!("{core_max_busy:.1}")),
        ("oracle_cores_saturated", cores_saturated.to_string()),
        ("oracle_ncores", ncores.to_string()),
        ("oracle_proc_cpu_cores", format!("{proc_cpu_cores:.3}")),
        ("oracle_renderdoc_captures", renderdoc_captures.to_string()),
        (
            "oracle_winreconfig_create_window_calls",
            winreconfig_create_window_calls.to_string(),
        ),
        (
            "oracle_winreconfig_set_window_pos_calls",
            winreconfig_set_window_pos_calls.to_string(),
        ),
        (
            "oracle_winreconfig_set_window_long_calls",
            winreconfig_set_window_long_calls.to_string(),
        ),
        (
            "oracle_winreconfig_move_window_calls",
            winreconfig_move_window_calls.to_string(),
        ),
        (
            "oracle_winreconfig_change_display_calls",
            winreconfig_change_display_calls.to_string(),
        ),
        (
            "oracle_winreconfig_last_set_pos_size",
            winreconfig_last_set_pos_size.to_string(),
        ),
        (
            "oracle_winreconfig_last_set_pos_flags",
            winreconfig_last_set_pos_flags.to_string(),
        ),
        (
            "oracle_winreconfig_last_move_size",
            winreconfig_last_move_size.to_string(),
        ),
        (
            "oracle_winreconfig_last_change_display_size",
            winreconfig_last_change_display_size.to_string(),
        ),
        (
            "oracle_winreconfig_last_change_display_flags",
            winreconfig_last_change_display_flags.to_string(),
        ),
        (
            "oracle_winreconfig_early_apply_result",
            winreconfig_early_apply_result.to_string(),
        ),
        (
            "oracle_winreconfig_early_apply_ms",
            winreconfig_early_apply_ms.to_string(),
        ),
        (
            "oracle_winreconfig_early_apply_rect",
            winreconfig_early_apply_rect.to_string(),
        ),
    ];
    let body = render_sample(&fields);
    // APPEND one JSON line per write -> a timeseries jsonl the agent reads after the run (no polling,
    // no sleep). body already ends in '\n'.
    //
    // The timeseries is per run, so the file is truncated by this process's first sample (previous
    // run kept one generation as `.prev`). A file spanning launches would make the tick stamps jump
    // backwards mid-file and every "how long did phase X take" read off it wrong.
    use std::io::Write as _;
    if let Some(mut f) = er_game_base::log::open_fresh_run_append(&standalone_json_path()) {
        let _ = f.write_all(body.as_bytes());
    }

    // Independently-marker-gated, passive read-side oracles (title-binding +
    // stream-overlap). Each no-ops unless its own game-dir marker is present, so a
    // plain run carries zero extra cost and one A/B enables exactly what it needs.
    read::tick(base, play_time_ms, flip_task_delta);
}

#[cfg(test)]
mod sample_rendering_tests {
    use super::*;

    /// One rendering context: its own `previous`, so tests cannot contaminate each other.
    #[derive(Default)]
    struct Series(Option<Vec<String>>);

    impl Series {
        fn push(&mut self, ticks: &str, tick_ms: &str, busy: &str, winreconfig: &str) -> String {
            render_sample_against(&fields(ticks, tick_ms, busy, winreconfig), &mut self.0)
        }
    }

    fn fields(
        ticks: &str,
        tick_ms: &str,
        busy: &str,
        winreconfig: &str,
    ) -> Vec<(&'static str, String)> {
        vec![
            ("oracle_standalone_ticks", ticks.to_owned()),
            ("oracle_tick_ms", tick_ms.to_owned()),
            ("oracle_core_max_busy", busy.to_owned()),
            (
                "oracle_winreconfig_create_window_calls",
                winreconfig.to_owned(),
            ),
        ]
    }

    /// The bound: a field that did not move is not written again. This is the whole 58% of the
    /// record that the winreconfig counters used to cost while changing at most twice per run.
    #[test]
    fn a_field_that_did_not_change_is_omitted() {
        let mut series = Series::default();
        let first = series.push("4", "1000", "36.4", "1");
        assert!(first.contains("oracle_winreconfig_create_window_calls"));
        let second = series.push("8", "1200", "37.5", "1");
        assert!(
            !second.contains("oracle_winreconfig_create_window_calls"),
            "an unchanged counter was written again: {second}"
        );
        assert!(
            second.contains("\"oracle_core_max_busy\":37.5"),
            "a field that DID change must always be written: {second}"
        );
    }

    /// Non-VACUITY guard against eliding by NAME: the same field must come back the moment its
    /// value moves, or the series silently freezes at whatever it read first.
    #[test]
    fn a_field_that_changes_after_a_flat_stretch_reappears() {
        let mut series = Series::default();
        series.push("4", "1000", "1.0", "1");
        for step in 0..50 {
            let flat = series.push("8", "1200", "1.0", "1");
            assert!(!flat.contains("winreconfig"), "step {step}: {flat}");
        }
        let moved = series.push("12", "1400", "1.0", "2");
        assert!(
            moved.contains("\"oracle_winreconfig_create_window_calls\":2"),
            "a changed field stayed elided after a flat stretch: {moved}"
        );
    }

    /// The identity and time columns are what let a reader place a row at all, so they survive
    /// even when they repeat (they cannot in practice; the assertion is what keeps it that way).
    #[test]
    fn the_identity_and_time_columns_are_never_elided() {
        let mut series = Series::default();
        series.push("4", "1000", "1.0", "1");
        let repeat = series.push("4", "1000", "1.0", "1");
        assert!(repeat.contains("oracle_standalone_ticks"), "{repeat}");
        assert!(repeat.contains("oracle_tick_ms"), "{repeat}");
    }

    /// Every emitted row must still parse as one JSON object -- the elision must never leave a
    /// dangling comma or an empty `{,}`.
    #[test]
    fn every_rendered_row_is_wellformed_json() {
        let mut series = Series::default();
        for row in [
            series.push("4", "1000", "1.0", "1"),
            series.push("4", "1000", "1.0", "1"),
            series.push("8", "2000", "9.5", "3"),
        ] {
            let body = row.trim().trim_start_matches('{').trim_end_matches('}');
            assert!(!body.starts_with(','), "leading comma: {row}");
            assert!(!body.ends_with(','), "trailing comma: {row}");
            assert!(!body.contains(",,"), "empty field: {row}");
            assert_eq!(
                body.split(',').count(),
                body.matches("\":").count(),
                "field count does not match separator count: {row}"
            );
        }
    }
}
