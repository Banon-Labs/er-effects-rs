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
\"oracle_proc_cpu_cores\":{proc_cpu_cores:.3}}}\n",
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
