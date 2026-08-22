//! In-DLL D3D12 GPU-timestamp oracle -- the goal-doc §3.3 `gpu_frame_us` semaphore
//! (`docs/goals/switch-reload-framerate-parity-acceptance.md`; bd er-effects-rs-03ma).
//!
//! WHAT IT MEASURES. Per-frame GPU-busy time in microseconds, obtained by injecting two D3D12
//! TIMESTAMP queries onto the GAME's own render `ID3D12CommandQueue`: a START stamp on the FIRST
//! `ExecuteCommandLists` after a present, and an END stamp on every ECL (last-executed wins). The
//! span START..END brackets the frame's real GPU command execution (first-ECL to last-ECL) and
//! EXCLUDES the vsync/flip present-wait (that happens INSIDE the original `Present`, after the last
//! ECL). So:
//!   - `gpu_frame_us` large (~50ms on the reload) => the frame is genuinely RENDER-BOUND;
//!   - `gpu_frame_us` small while `oracle_present_qpc_delta_us` stays ~50ms => a present/vblank throttle.
//!
//! This is exactly the split issue 03ma asks for -- attributing the reload-20fps residual to GPU
//! render cost vs present-wait -- and the localization tool the parity goal's §4 delta protocol uses.
//!
//! WHY SHARED-VTABLE HOOK + RUNTIME LATCH (no RVA). The game's render queue pointer is NOT read from a
//! hardcoded RVA (a prior `GX_COMMAND_QUEUE_RVA=0x8012a8` was stale/unpopulated at runtime -> the
//! oracle never left state 0). Instead we ride the already-RE'd present path: the game device comes
//! RVA-free from the found swapchain's backbuffer (`GetBuffer(0).GetDevice()`), a probe queue on that
//! device gives the shared `ID3D12CommandQueue` vtable, we swap its `ExecuteCommandLists` slot (all
//! queues from one D3D12 runtime module share one vtable -- vkd3d under Wine, d3d12core.dll on native
//! Windows -- like the shared dxgi swapchain vtable the present hook relies on), and
//! the detour LATCHES the real render queue at runtime = the first DIRECT queue that submits.
//!
//! WHY PIGGYBACK (not a separate submit). The DLL previously AV'd submitting its own command lists on
//! the game queue from the Present hook (see `gpu_readback/resource_readback.rs` `GX_COMMAND_QUEUE_RVA`
//! note). Timestamps MUST share the game queue's timeline to bracket the game's GPU work, so this
//! oracle never issues its OWN `ExecuteCommandLists`/`Signal` on the game queue: it AUGMENTS the game's
//! own ECL call by prepending/appending pre-recorded timestamp lists to the list array the game already
//! submits -- the game's own submit, on the game's own render thread, in-order.
//!
//! SAFETY. The platform is not assumed (no Wine/native gate; whatever the machine is at runtime).
//! Every entry point is `catch_unwind`-guarded and fail-closed: any D3D12
//! error disables the oracle permanently (never a retry-crash), and `oracle_gpu_frame_state` /
//! `oracle_gpu_frame_samples` make a `gpu_frame_us == 0` attributable. The readback is a lagged,
//! cache-coherent CPU read of the previous frame's resolved pair (no fence on the game queue); a rare
//! torn/stale value is clamped out and washed out by the steady-state median.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_COMMAND_QUEUE_FLAG_NONE,
    D3D12_CPU_PAGE_PROPERTY_UNKNOWN, D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES,
    D3D12_HEAP_TYPE_READBACK, D3D12_MEMORY_POOL_UNKNOWN, D3D12_QUERY_HEAP_DESC,
    D3D12_QUERY_HEAP_TYPE_TIMESTAMP, D3D12_QUERY_TYPE_TIMESTAMP, D3D12_RANGE, D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_DIMENSION_BUFFER, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST,
    D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ID3D12CommandAllocator, ID3D12CommandList, ID3D12CommandQueue,
    ID3D12Device, ID3D12GraphicsCommandList, ID3D12QueryHeap, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain;
use windows::core::Interface;

use super::*;

/// Emitted RAM oracles (see `er-telemetry/src/counters.rs` for the doc contract).
pub(crate) use er_telemetry::counters::GPU_FRAME_ORACLE_SAMPLES;
pub(crate) use er_telemetry::counters::GPU_FRAME_ORACLE_STATE;
pub(crate) use er_telemetry::counters::GPU_FRAME_US_LAST;

/// `ID3D12CommandQueue::ExecuteCommandLists` vtable index: IUnknown(3) + ID3D12Object(4) +
/// ID3D12DeviceChild(1) + ID3D12Pageable(0) = 8; then UpdateTileMappings(8), CopyTileMappings(9),
/// ExecuteCommandLists(10).
const ECL_VTABLE_INDEX: usize = 10;
/// Query-heap slots: 0 = frame START (first ECL after a present), 1 = frame END (last ECL).
const TS_START_IDX: u32 = 0;
const TS_END_IDX: u32 = 1;
const QUERY_COUNT: u32 = 2;
/// Two `u64` timestamps resolved into the readback buffer.
const READBACK_BYTES: u64 = 16;
/// Max command lists we augment on the stack (no per-ECL heap alloc on the render thread). An ECL with
/// more lists than this is forwarded UNCHANGED that frame (instrumentation skipped, never a truncation).
const MAX_AUG_LISTS: usize = 128;
/// Clamp for a sane per-frame GPU span (drop torn/stale readbacks): 1us..1s.
const GPU_US_SANE_MIN: usize = 1;
const GPU_US_SANE_MAX: usize = 1_000_000;

/// The game's render queue pointer (== `this` we augment), latched at runtime from the first DIRECT
/// queue that submits; 0 until latched.
static GAME_CMD_QUEUE: AtomicUsize = AtomicUsize::new(0);
/// The probe queue we created on the game device to read the shared vtable -- excluded from latching.
static PROBE_QUEUE_PTR: AtomicUsize = AtomicUsize::new(0);
/// Original `ExecuteCommandLists` fn (the detour tail-calls it); 0 until hooked.
static ECL_ORIG: AtomicUsize = AtomicUsize::new(0);
/// Leaked (process-lifetime) COM objects, kept alive as raw pointers exactly like the `RB_COH_*`
/// readback statics. The query heap + readback are referenced by the recorded lists; the two lists are
/// what we splice into the game's ECL array each frame.
static START_LIST_PTR: AtomicUsize = AtomicUsize::new(0);
static END_LIST_PTR: AtomicUsize = AtomicUsize::new(0);
static READBACK_RES_PTR: AtomicUsize = AtomicUsize::new(0);
/// GPU timestamp frequency (ticks/sec) from `ID3D12CommandQueue::GetTimestampFrequency`.
static TS_FREQ: AtomicU64 = AtomicU64::new(0);
/// False at each present (via `gpu_frame_oracle_on_present`); the first ECL that finds it false is the
/// frame's first ECL (prepend START + read the previous frame's resolved pair).
static FRAME_STARTED: AtomicBool = AtomicBool::new(false);
/// One-shot install latch (set once we commit to creating D3D12 objects; prevents a retry-crash).
static INSTALLED: AtomicBool = AtomicBool::new(false);

type EclFn = unsafe extern "system" fn(*mut c_void, u32, *const *mut c_void);

/// Diagnostic OPT-IN for the GPU-frame oracle -- DEFAULT OFF, and the primary safety gate. The
/// ECL-piggyback (appending an EndQuery+ResolveQueryData list to EVERY game `ExecuteCommandLists`)
/// device-removed the game ~28s in on the native code path (bd
/// `gpu-frame-us-ecl-piggyback-oracle-crashes-native-path-must-be-opt-in-2026-07-23`), so it must NEVER
/// run on a product/user launch. A measurement run that accepts the current crash risk (pending the
/// one-resolve-per-frame redesign) opts in by dropping `er-effects-gpu-frame-oracle.txt` next to
/// `eldenring.exe` (the game CWD). This is a DIAGNOSTIC-only gate, not a product feature gate, so a
/// marker file is the right mechanism (release default is unchanged: oracle absent).
fn gpu_frame_oracle_opt_in() -> bool {
    std::path::Path::new("er-effects-gpu-frame-oracle.txt").exists()
}

/// Enabled only when explicitly opted in (above) AND the present-cadence telemetry is being measured
/// (mirrors `try_install_game_present_hook`'s gate). The platform (Wine/Proton vs native Windows) is
/// NOT assumed or gated on here -- that is decided at runtime by whatever the machine actually is, and
/// the D3D12 setup in `install_inner` fail-closes if the runtime cannot support the query-heap/piggyback.
/// (An earlier `&& running_under_wine()` gate baked in a stale "this box is Wine" assumption and left
/// the oracle permanently at state 0.) RenderDoc runs stand the overlay down, so skip there.
fn gpu_frame_oracle_enabled() -> bool {
    gpu_frame_oracle_opt_in()
        && (portrait_overlay_enabled()
            || crate::experiments::save_override_telemetry_only()
            || crate::experiments::measure_no_composite())
        && !crate::experiments::renderdoc_active()
}

/// Per-frame game-task tick (called right after `try_install_game_present_hook`). One-shot; needs the
/// present hook already installed (so `GAME_SWAPCHAIN` is known and the present detour resets the frame
/// boundary). Cheap when latched.
pub(crate) unsafe fn try_install_gpu_frame_oracle(_base: usize) {
    if INSTALLED.load(Ordering::SeqCst) || !gpu_frame_oracle_enabled() {
        return;
    }
    // The present hook owns the frame-boundary reset AND provides the swapchain -> game device.
    if er_telemetry::counters::GAME_PRESENT_HOOKED.load(Ordering::SeqCst) == 0 {
        return;
    }
    let sc = er_telemetry::counters::GAME_SWAPCHAIN.load(Ordering::SeqCst);
    if sc <= 0x10000 {
        return;
    }
    // Commit: latch BEFORE the D3D12 setup so a partial failure never retries (and never crashes).
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        install_inner(sc)
    }))
    .unwrap_or(None);
    if ok.is_none() {
        append_autoload_debug(format_args!(
            "gpu-frame-oracle: install FAILED (state={}) -- disabled for this run",
            GPU_FRAME_ORACLE_STATE.load(Ordering::SeqCst)
        ));
    }
}

/// D3D12 setup on the GAME device (obtained RVA-free from the swapchain backbuffer): query heap +
/// readback buffer + the two pre-recorded timestamp lists, then the shared-vtable ECL hook. Returns
/// `None` on any failure (state records how far it got).
unsafe fn install_inner(sc_ptr: usize) -> Option<()> {
    let sc_raw = sc_ptr as *mut c_void;
    let swapchain = unsafe { IDXGISwapChain::from_raw_borrowed(&sc_raw) }?;
    // Game device, RVA-free: the swapchain's backbuffer is an ID3D12Resource on the game device.
    let backbuffer: ID3D12Resource = unsafe { swapchain.GetBuffer(0) }.ok()?;
    let mut device_opt: Option<ID3D12Device> = None;
    unsafe { backbuffer.GetDevice(&mut device_opt) }.ok()?;
    let device = device_opt?;

    // Probe queue on the GAME device: its vtable IS the game render queue's vtable (same device), and it
    // also answers GetTimestampFrequency. It never submits, so it is excluded from render-queue latching.
    let queue_desc = D3D12_COMMAND_QUEUE_DESC {
        Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
        Priority: 0,
        Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
        NodeMask: 0,
    };
    let probe_queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&queue_desc) }.ok()?;
    let freq = unsafe { probe_queue.GetTimestampFrequency() }.ok()?;
    if freq == 0 {
        return None;
    }
    TS_FREQ.store(freq, Ordering::SeqCst);

    let heap_desc = D3D12_QUERY_HEAP_DESC {
        Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
        Count: QUERY_COUNT,
        NodeMask: 0,
    };
    let mut heap_opt: Option<ID3D12QueryHeap> = None;
    unsafe { device.CreateQueryHeap(&heap_desc, &mut heap_opt) }.ok()?;
    let heap = heap_opt?;

    // READBACK-heap buffer (CPU-coherent, always COPY_DEST) for the resolved START/END pair.
    let heap_props = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_READBACK,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    };
    let buf_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: READBACK_BYTES,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    };
    let mut rb: Option<ID3D12Resource> = None;
    unsafe {
        device.CreateCommittedResource(
            &heap_props,
            D3D12_HEAP_FLAG_NONE,
            &buf_desc,
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut rb,
        )
    }
    .ok()?;
    let readback = rb?;

    // One allocator backs both tiny lists (recorded sequentially, never Reset -- so re-executing them
    // every ECL forever is valid). START list: one timestamp write. END list: timestamp write + a
    // resolve of the START..END pair into the readback buffer (in-list order = write-then-resolve).
    let allocator: ID3D12CommandAllocator =
        unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }.ok()?;
    let start_list: ID3D12GraphicsCommandList =
        unsafe { device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None) }
            .ok()?;
    unsafe { start_list.EndQuery(&heap, D3D12_QUERY_TYPE_TIMESTAMP, TS_START_IDX) };
    unsafe { start_list.Close() }.ok()?;
    let end_list: ID3D12GraphicsCommandList =
        unsafe { device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None) }
            .ok()?;
    unsafe { end_list.EndQuery(&heap, D3D12_QUERY_TYPE_TIMESTAMP, TS_END_IDX) };
    unsafe {
        end_list.ResolveQueryData(
            &heap,
            D3D12_QUERY_TYPE_TIMESTAMP,
            TS_START_IDX,
            QUERY_COUNT,
            &readback,
            0,
        )
    };
    unsafe { end_list.Close() }.ok()?;

    // Leak everything to process lifetime (COM is !Send; the raw pointers are the owners), like RB_COH_*.
    let start_base: ID3D12CommandList = start_list.cast().ok()?;
    let end_base: ID3D12CommandList = end_list.cast().ok()?;
    START_LIST_PTR.store(start_base.into_raw() as usize, Ordering::SeqCst);
    END_LIST_PTR.store(end_base.into_raw() as usize, Ordering::SeqCst);
    READBACK_RES_PTR.store(readback.into_raw() as usize, Ordering::SeqCst);
    let _ = heap.into_raw(); // referenced by the recorded lists; keep alive.
    std::mem::forget(allocator); // backs the recorded lists; must not drop.
    std::mem::forget(device); // owns all of the above.
    GPU_FRAME_ORACLE_STATE.store(1, Ordering::SeqCst); // objects created

    // Hook ExecuteCommandLists via a vtable-slot swap on the SHARED vkd3d queue vtable (read from the
    // probe queue; NOT MinHook -- Wine's d3d12core.dll refuses the code-page patch, same as dxgi.dll).
    let probe_raw = probe_queue.as_raw() as usize;
    let vt = unsafe { safe_read_usize(probe_raw) }.filter(|v| *v > 0x10000)?;
    let slot = vt + ECL_VTABLE_INDEX * 8;
    let orig = unsafe { safe_read_usize(slot) }.filter(|o| *o > 0x10000)?;
    ECL_ORIG.store(orig, Ordering::SeqCst);
    PROBE_QUEUE_PTR.store(probe_raw, Ordering::SeqCst);
    let _ = probe_queue.into_raw(); // keep the probe queue alive (its vtable is what we patched).
    unsafe { vtable_swap_slot(slot, execute_command_lists_hook as *const () as usize) }?;
    GPU_FRAME_ORACLE_STATE.store(2, Ordering::SeqCst); // hooked; render queue latched at first ECL
    append_autoload_debug(format_args!(
        "gpu-frame-oracle: hook installed (game device via swapchain 0x{sc_ptr:x}, probe queue 0x{probe_raw:x}, ECL slot 0x{slot:x} orig=0x{orig:x}, ts_freq={freq}); awaiting first DIRECT-queue ECL to latch the render queue"
    ));
    Some(())
}

/// True if `queue_ptr` borrows as an `ID3D12CommandQueue` whose desc Type is DIRECT.
unsafe fn is_direct_queue(queue_ptr: *mut c_void) -> bool {
    let Some(q) = (unsafe { ID3D12CommandQueue::from_raw_borrowed(&queue_ptr) }) else {
        return false;
    };
    unsafe { q.GetDesc() }.Type == D3D12_COMMAND_LIST_TYPE_DIRECT
}

/// Detour for `ID3D12CommandQueue::ExecuteCommandLists(this, num, ppLists)`. Latches the game render
/// queue (first DIRECT submitter that is not the probe queue), then splices the timestamp lists into
/// that queue's own submits; every other queue is forwarded byte-for-byte. Runs on the game render
/// thread; must never panic and must ALWAYS forward exactly once.
unsafe extern "system" fn execute_command_lists_hook(
    this: *mut c_void,
    num: u32,
    pp: *const *mut c_void,
) {
    let orig = ECL_ORIG.load(Ordering::SeqCst);
    if orig == 0 {
        return;
    }
    let f: EclFn = unsafe { std::mem::transmute::<usize, EclFn>(orig) };
    // `augment_and_forward` calls `f` exactly once as its final act and returns true; a panic can only
    // occur BEFORE that call, so on unwind we forward plain (never double-forward).
    let forwarded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        augment_and_forward(f, this, num, pp)
    }))
    .unwrap_or(false);
    if !forwarded {
        unsafe { f(this, num, pp) };
    }
}

/// Returns true iff it forwarded to `f`. Never records anything after the `f` call, so a panic implies
/// `f` was not reached.
unsafe fn augment_and_forward(
    f: EclFn,
    this: *mut c_void,
    num: u32,
    pp: *const *mut c_void,
) -> bool {
    let mut game_q = GAME_CMD_QUEUE.load(Ordering::SeqCst);
    if game_q == 0 {
        // Latch the game render queue = the first DIRECT queue that submits, excluding our probe queue.
        if this as usize != PROBE_QUEUE_PTR.load(Ordering::SeqCst)
            && num > 0
            && unsafe { is_direct_queue(this) }
        {
            GAME_CMD_QUEUE.store(this as usize, Ordering::SeqCst);
            game_q = this as usize;
            append_autoload_debug(format_args!(
                "gpu-frame-oracle: latched game render queue 0x{:x} (first DIRECT ECL)",
                this as usize
            ));
        } else {
            unsafe { f(this, num, pp) };
            return true;
        }
    }
    if this as usize != game_q || num == 0 || pp.is_null() {
        unsafe { f(this, num, pp) };
        return true;
    }
    let start = START_LIST_PTR.load(Ordering::SeqCst) as *mut c_void;
    let end = END_LIST_PTR.load(Ordering::SeqCst) as *mut c_void;
    if start.is_null() || end.is_null() {
        unsafe { f(this, num, pp) };
        return true;
    }
    // First ECL of the frame? (present reset the flag). If so, prepend START and read the PREVIOUS
    // frame's resolved pair (the GPU had all of last frame to finish that resolve).
    let first = !FRAME_STARTED.swap(true, Ordering::SeqCst);
    if first {
        unsafe { read_prev_frame_gpu_us() };
    }
    let n = num as usize;
    let extra = if first { 2 } else { 1 };
    if n + extra > MAX_AUG_LISTS {
        // Too many lists to splice on the stack -- forward unchanged (skip instrumentation this ECL).
        unsafe { f(this, num, pp) };
        return true;
    }
    let mut buf: [*mut c_void; MAX_AUG_LISTS] = [std::ptr::null_mut(); MAX_AUG_LISTS];
    let mut k = 0usize;
    if first {
        buf[k] = start; // START before the game's work
        k += 1;
    }
    for i in 0..n {
        buf[k] = unsafe { *pp.add(i) };
        k += 1;
    }
    buf[k] = end; // END + resolve after the game's work (last ECL of the frame wins)
    k += 1;
    unsafe { f(this, k as u32, buf.as_ptr()) };
    true
}

/// CPU-coherent read of the readback buffer -> `oracle_gpu_frame_us`. Lagged/unfenced: it reads the
/// last resolve the GPU completed. A torn/stale value is clamped out; the steady-state median over the
/// 0.5s telemetry polls absorbs the rest.
unsafe fn read_prev_frame_gpu_us() {
    let rb_raw = READBACK_RES_PTR.load(Ordering::SeqCst) as *mut c_void;
    if rb_raw.is_null() {
        return;
    }
    let Some(readback) = (unsafe { ID3D12Resource::from_raw_borrowed(&rb_raw) }) else {
        return;
    };
    let read_range = D3D12_RANGE {
        Begin: 0,
        End: READBACK_BYTES as usize,
    };
    let mut mapped: *mut c_void = std::ptr::null_mut();
    if unsafe { readback.Map(0, Some(&read_range), Some(&mut mapped)) }.is_err() || mapped.is_null()
    {
        return;
    }
    let ts = unsafe { std::slice::from_raw_parts(mapped as *const u64, 2) };
    let (start_tick, end_tick) = (ts[0], ts[1]);
    // Wrote nothing back.
    let write_range = D3D12_RANGE { Begin: 0, End: 0 };
    unsafe { readback.Unmap(0, Some(&write_range)) };
    let freq = TS_FREQ.load(Ordering::SeqCst);
    if freq == 0 || end_tick <= start_tick {
        return;
    }
    let us = ((end_tick - start_tick) as u128 * 1_000_000 / freq as u128) as usize;
    if (GPU_US_SANE_MIN..GPU_US_SANE_MAX).contains(&us) {
        GPU_FRAME_US_LAST.store(us, Ordering::SeqCst);
        GPU_FRAME_ORACLE_SAMPLES.fetch_add(1, Ordering::SeqCst);
        GPU_FRAME_ORACLE_STATE.store(3, Ordering::SeqCst); // producing
    }
}

/// Called once per present from the Present detour: the next ECL on the game queue is the new frame's
/// first ECL. Cheap; safe before the oracle is installed (just flips an atomic).
pub(crate) fn gpu_frame_oracle_on_present() {
    FRAME_STARTED.store(false, Ordering::SeqCst);
}
