//! D3D12 Present overlay -- draw the captured now-loading portrait directly onto the swapchain
//! backbuffer, bypassing the Scaleform/TexRepository pipeline entirely.
//!
//! The in-pipeline routes (forge bake, re-forge, CS-texture upload) cannot drive the DISPLAYED image:
//! the forge pre-binds before the portrait renders (timing race) and Scaleform samples its own decoded
//! GFx-renderer texture copy distinct from the CS-side texture we can reach (see bd
//! `postcontinue-portrait-EXHAUSTIVE-2026-06-30`). This is the sanctioned native D3D12/game-render-layer
//! path: hook `IDXGISwapChain::Present`, and when the now-loading screen is up, copy the captured portrait
//! over the backbuffer.
//!
//! Phase 1 (this commit): install the Present hook via the standard dummy-swapchain vtable technique and
//! log that it fires + the backbuffer format/dims. NO backbuffer writes yet (lowest crash risk) -- proves
//! the hook mechanism before adding the draw.

#![allow(unused_imports)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_COMMAND_QUEUE_FLAG_NONE,
    D3D12CreateDevice, ID3D12CommandQueue, ID3D12Device,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_UNSPECIFIED, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory4, IDXGISwapChain,
    IDXGISwapChain1,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW,
    UnregisterClassW, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{IUnknown, Interface, w};

use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};

use super::*;

/// Per-frame Present hit counter (RAM semaphore that the overlay hook is live + firing).
pub(crate) use er_telemetry::counters::PRESENT_HOOK_HITS;
pub(crate) use er_telemetry::counters::PRESENT_HOOK_INSTALLED;
/// Original `IDXGISwapChain::Present` / `Present1` trampolines; 0 until installed.
pub(crate) use er_telemetry::counters::PRESENT_ORIG;
pub(crate) use er_telemetry::counters::PRESENT1_ORIG;

/// `IDXGISwapChain::Present` vtable index: IUnknown(3) + IDXGIObject(4) + IDXGIDeviceSubObject(1) = slot 8.
const PRESENT_VTABLE_INDEX: usize = 8;
/// `IDXGISwapChain1::Present1` vtable index (Present(8)..GetLastPresentCount(17), GetDesc1(18)..Present1(22)).
const PRESENT1_VTABLE_INDEX: usize = 22;

type PresentFn = unsafe extern "system" fn(*mut c_void, u32, u32) -> i32;
type Present1Fn = unsafe extern "system" fn(*mut c_void, u32, u32, *const c_void) -> i32;

/// `g_GxDrawContext` (GXSR::GxDrawContext singleton) deobf RVA -- the RE-confirmed root of the live
/// swapchain. The deobf binary stores this singleton at 0x1419e637c (`mov [rip]=0x1447ef360`) right before
/// the `GxDrawContext::Initilize` fall-through (ground-truthed against the deobf binary, not a formula).
/// The 0x1010-byte GxDrawContext holds the per-window render-output vector at +0x120 (begin ptr at +0x128);
/// each inline 0x170-byte entry's first qword is the per-window output object, whose first qword IS the live
/// `IDXGISwapChain3*`. Chain: `*(base+RVA)` -> `+0x128` -> `*entry[0]` -> `*output` = swapchain. (Supersedes
/// the old `GLOBAL_CSGraphics` root, which never held the swapchain -- CSGraphics is unrelated to GX present.)
const G_GX_DRAW_CONTEXT_RVA: usize = 0x47ef360;
/// `GxDrawContext+0x128` = begin pointer of the per-window render-output vector (vector object at +0x120).
const GXDC_OUTPUT_VEC_BEGIN_OFFSET: usize = 0x128;
pub(crate) use er_telemetry::counters::GAME_BASE;
/// Set once we've found the GAME's swapchain and hooked its REAL Present/Present1. (The earlier "dummy
/// swapchain vtable funcs differ under vkd3d-proton" theory was unsound -- under Proton all dxgi.dll
/// swapchains share one DXVK `CDXGISwapChain` vtable, so Present(8)/Present1(22) are the same function for
/// every swapchain. The real prior blocker was the FIND missing the object, so MinHook was never attempted
/// on a real swapchain; dinput8 MinHooks fire, so the hook path itself is sound.)
pub(crate) use er_telemetry::counters::GAME_PRESENT_HOOKED;
/// The found GAME swapchain pointer + game module base, latched in `try_install_game_present_hook`. The
/// Present detour composites the portrait only when `this` matches `GAME_SWAPCHAIN` -- the shared dxgi
/// vtable means the detour ALSO fires for our throwaway dummy swapchain, which we must never draw on.
pub(crate) use er_telemetry::counters::GAME_SWAPCHAIN;
pub(crate) use er_telemetry::counters::GAME_SWAPCHAIN_FIND_TRIES;
/// The dummy swapchain's resolved `Present(8)` / `Present1(22)` addrs. Under vkd3d-proton EVERY dxgi
/// swapchain shares one DXVK `CDXGISwapChain` vtable, so the GAME swapchain's `vtable[8]`/`vtable[22]`
/// are byte-identical to these (runtime-proven: resolved 0x..209f0 == VMT-swapped Present8 0x..209f0).
/// This lets `swapchain_vtable_matches` confirm a candidate by READING + comparing these two slots --
/// never by dispatching `QueryInterface`, which faults on a half-constructed early-boot object whose
/// dxgi-ranged-but-bogus vtable can't be caught by `catch_unwind` (that AV killed the pump at +726ms).
pub(crate) use er_telemetry::counters::PRESENT_RESOLVED_ADDR;
pub(crate) use er_telemetry::counters::PRESENT1_RESOLVED_ADDR;

// === Swapchain-find reject attribution (RAM oracles) =============================================
// The 2026-07-15 native-Windows runs burned three probes on an opaque "chain miss": the walk gave no
// way to tell a null chain link from a REAL candidate rejected by the vtable-equality check. Every
// find attempt now stores its terminal stage + candidate facts so telemetry alone names the failing
// predicate (oracle_present_find_* in write_game_module_oracles).
/// How the game swapchain was accepted: 0=not yet, 1=exact vtable match against the dummy-resolved
/// Present/Present1 (the vkd3d shared-vtable fast path), 2=module-backed + stable + QI fallback (the
/// native-Windows / wrapped-swapchain path).
pub(crate) use er_telemetry::counters::PRESENT_ACCEPT_PATH;
/// Backbuffer DXGI_FORMAT.0 from the first Present's GetDesc1 (RAM oracle: makes the native-Windows
/// HDR/10-bit case, DXGI_FORMAT_R10G10B10A2_UNORM=24, directly attributable instead of inferred from
/// zero composite draw-hits). 0 until the first present.
pub(crate) use er_telemetry::counters::PRESENT_BACKBUFFER_FORMAT;
/// Last non-null chain candidate (`*output`), its vtable pointer, and its Present(8)/Present1(22).
pub(crate) use er_telemetry::counters::PRESENT_FIND_CANDIDATE;
pub(crate) use er_telemetry::counters::PRESENT_FIND_CANDIDATE_VT;
pub(crate) use er_telemetry::counters::PRESENT_FIND_GOT8;
pub(crate) use er_telemetry::counters::PRESENT_FIND_GOT22;
pub(crate) use er_telemetry::counters::PRESENT_FIND_LAST_CANDIDATE;
/// Last find stage (see `FIND_STAGE_*`): which link/predicate the most recent attempt ended on.
pub(crate) use er_telemetry::counters::PRESENT_FIND_STAGE;
/// Consecutive tries that yielded the SAME candidate pointer (the QI-fallback stability gate).
pub(crate) use er_telemetry::counters::PRESENT_FIND_STREAK;
/// Owning module of the candidate's vtable: 0=unknown/not-module-backed, 1=dxgi.dll, 2=the game exe
/// (mis-layout red flag), 3=another module (overlay/wrapper DLL -- name in the debug log).
pub(crate) use er_telemetry::counters::PRESENT_FIND_VT_MODULE_KIND;
/// Consecutive same-candidate observations required before the QI fallback may dispatch through the
/// candidate's vtable. A half-constructed transient does not survive consecutive frames in the game's
/// single live output slot; the real swapchain does.
const PRESENT_QI_STABILITY_TRIES: usize = 3;
/// `PRESENT_FIND_STAGE` values, in chain-walk order.
const FIND_STAGE_CTX_NULL: usize = 1;
const FIND_STAGE_VEC_NULL: usize = 2;
const FIND_STAGE_ENTRY_NULL: usize = 3;
const FIND_STAGE_CANDIDATE_NULL: usize = 4;
const FIND_STAGE_VT_UNREADABLE: usize = 5;
const FIND_STAGE_VT_NOT_MODULE: usize = 6;
const FIND_STAGE_VT_IN_GAME_EXE: usize = 7;
const FIND_STAGE_STABILITY_WAIT: usize = 8;
const FIND_STAGE_QI_REJECTED: usize = 9;
const FIND_STAGE_ACCEPTED_VTABLE: usize = 10;
const FIND_STAGE_ACCEPTED_QI: usize = 11;

/// Detour for `IDXGISwapChain::Present(this, SyncInterval, Flags)`. Phase 1: log-only, then tail-call the
/// original. `this` IS the game's swapchain (we never created it), so a real overlay draws onto its
/// current backbuffer here. Must never panic (runs on the game's render thread every frame).
/// Cached QueryPerformanceCounter frequency (ticks/sec) for converting DXGI SyncQPCTime deltas to us.
fn qpc_frequency() -> u64 {
    static FREQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let f = FREQ.load(Ordering::Relaxed);
    if f != 0 {
        return f;
    }
    let mut v = 0i64;
    let _ = unsafe { windows::Win32::System::Performance::QueryPerformanceFrequency(&mut v) };
    let v = v.max(0) as u64;
    FREQ.store(v, Ordering::Relaxed);
    v
}

/// Record the present-cadence semaphores for the GAME swapchain (read-only; never panics): the game's
/// requested `SyncInterval`, plus the OBSERVED cadence from IDXGISwapChain::GetFrameStatistics
/// (display-refreshes/present x100 and present-to-present QPC spacing). Splits a deliberate 20fps
/// present throttle (SyncInterval=3) from GPU-can't-keep-up (SyncInterval=1 but 3 vblanks/present).
/// bd GPU-timestamp-semaphore-split-reload-20fps-residual-2026-07-22.
unsafe fn record_present_frame_stats(this: *mut c_void, sync: u32) {
    if this as usize != GAME_SWAPCHAIN.load(Ordering::SeqCst) {
        return;
    }
    er_telemetry::counters::PRESENT_SYNC_INTERVAL_LAST.store(sync as usize, Ordering::SeqCst);
    let Some(sc) = (unsafe { IDXGISwapChain::from_raw_borrowed(&this) }) else {
        return;
    };
    // GetFrameStatistics returns DXGI_ERROR_FRAME_STATISTICS_DISJOINT until a stable present series
    // exists; treat any error as "no sample this frame" and leave the prior oracle values in place.
    let mut stats = windows::Win32::Graphics::Dxgi::DXGI_FRAME_STATISTICS::default();
    if unsafe { sc.GetFrameStatistics(&mut stats) }.is_err() {
        return;
    }
    let pc = stats.PresentCount as usize;
    let sr = stats.SyncRefreshCount as usize;
    let qpc = stats.SyncQPCTime.max(0) as u64;
    let prev_pc =
        er_telemetry::counters::PRESENT_STATS_PREV_PRESENT_COUNT.swap(pc, Ordering::SeqCst);
    let prev_sr =
        er_telemetry::counters::PRESENT_STATS_PREV_SYNC_REFRESH.swap(sr, Ordering::SeqCst);
    let prev_qpc = er_telemetry::counters::PRESENT_STATS_PREV_QPC.swap(qpc, Ordering::SeqCst);
    let dpc = pc.wrapping_sub(prev_pc);
    let dsr = sr.wrapping_sub(prev_sr);
    if dpc > 0 && dpc < 1000 {
        er_telemetry::counters::PRESENT_REFRESH_PER_PRESENT_X100
            .store(dsr.wrapping_mul(100) / dpc, Ordering::SeqCst);
    }
    if prev_qpc != 0 && qpc > prev_qpc {
        let freq = qpc_frequency();
        if freq > 0 {
            let us = ((qpc - prev_qpc) as u128 * 1_000_000 / freq as u128) as usize;
            er_telemetry::counters::PRESENT_QPC_DELTA_US.store(us, Ordering::SeqCst);
        }
    }
}

unsafe extern "system" fn present_hook(this: *mut c_void, sync: u32, flags: u32) -> i32 {
    let hits = PRESENT_HOOK_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if hits == 1 {
        append_autoload_debug(format_args!(
            "present-overlay: PRESENT(8) hook FIRED first-hit this=0x{:x}",
            this as usize
        ));
        unsafe { log_backbuffer_desc(this) };
    }
    // Composite the captured portrait onto the backbuffer (gated; never panics). Only on the GAME's
    // swapchain -- the shared dxgi vtable means this detour also fires for our throwaway dummy swapchain.
    let this_u = this as usize;
    if this_u == GAME_SWAPCHAIN.load(Ordering::SeqCst) {
        let base = GAME_BASE.load(Ordering::SeqCst);
        if base != 0 {
            // Time the boot-view composite (the suspected per-frame WORK stall on reloads). Gated on the
            // overlay being a product feature this run: telemetry-only measurement records cadence (below)
            // but SKIPS the flow-modifying composite so the vanilla baseline stays flow-faithful.
            let tc = std::time::Instant::now();
            if portrait_overlay_enabled() {
                unsafe { composite_on_game_swapchain(base, this_u) };
            }
            er_telemetry::counters::COMPOSITE_LAST_US
                .store(tc.elapsed().as_micros() as usize, Ordering::SeqCst);
        }
    }
    let orig = PRESENT_ORIG.load(Ordering::SeqCst);
    if orig != 0 {
        let f: PresentFn = unsafe { std::mem::transmute(orig) };
        // Time the original Present to split present-BLOCK (compositor/vsync throttle) from a real
        // per-frame WORK stall. bd FOCUS-AB-falsifies-unfocused-throttle...next-present-duration.
        let t0 = std::time::Instant::now();
        let r = unsafe { f(this, sync, flags) };
        er_telemetry::counters::PRESENT_CALL_LAST_US
            .store(t0.elapsed().as_micros() as usize, Ordering::SeqCst);
        unsafe { record_present_frame_stats(this, sync) };
        r
    } else {
        0
    }
}

/// Detour for `IDXGISwapChain1::Present1(this, SyncInterval, Flags, *DXGI_PRESENT_PARAMETERS)`.
unsafe extern "system" fn present1_hook(
    this: *mut c_void,
    sync: u32,
    flags: u32,
    params: *const c_void,
) -> i32 {
    let hits = PRESENT_HOOK_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if hits == 1 {
        append_autoload_debug(format_args!(
            "present-overlay: PRESENT1(22) hook FIRED first-hit this=0x{:x}",
            this as usize
        ));
        unsafe { log_backbuffer_desc(this) };
    }
    // Composite the captured portrait onto the backbuffer (gated; never panics). Only on the GAME's
    // swapchain -- the shared dxgi vtable means this detour also fires for our throwaway dummy swapchain.
    let this_u = this as usize;
    if this_u == GAME_SWAPCHAIN.load(Ordering::SeqCst) {
        let base = GAME_BASE.load(Ordering::SeqCst);
        if base != 0 {
            // Composite gated on the overlay being a product feature this run; telemetry-only measurement
            // records cadence (below) but skips the flow-modifying composite (vanilla baseline stays
            // flow-faithful). See the Present(8) detour for the rationale.
            let tc = std::time::Instant::now();
            if portrait_overlay_enabled() {
                unsafe { composite_on_game_swapchain(base, this_u) };
            }
            er_telemetry::counters::COMPOSITE_LAST_US
                .store(tc.elapsed().as_micros() as usize, Ordering::SeqCst);
        }
    }
    let orig = PRESENT1_ORIG.load(Ordering::SeqCst);
    if orig != 0 {
        let f: Present1Fn = unsafe { std::mem::transmute(orig) };
        let t0 = std::time::Instant::now();
        let r = unsafe { f(this, sync, flags, params) };
        er_telemetry::counters::PRESENT_CALL_LAST_US
            .store(t0.elapsed().as_micros() as usize, Ordering::SeqCst);
        unsafe { record_present_frame_stats(this, sync) };
        r
    } else {
        0
    }
}

/// `+0x754/+0x755` on a CSMenuProfModelRend are the engine's build-request latches -- the kick raises
/// them and the engine clears them when the model+offscreen build lands (see kick_target_profile_slot's
/// "kick only when BOTH read 0 = not already in flight" parity check, loading_cover_save_slot.rs:301-306).
const PROFILE_RENDERER_REQ_754_OFFSET: usize = 0x754;
const PROFILE_RENDERER_REQ_755_OFFSET: usize = 0x755;
/// Presents skipped because a profile-model build was in flight (RAM oracle for the crash mitigation).
pub(crate) use er_telemetry::counters::PRESENT_COMPOSITE_BUILD_SKIPS;

/// True while the just-kicked profile renderer's build-request latches are still set -- i.e. the engine
/// is mid-build of the profile model + its offscreen RT. During this window the engine's OWN
/// ResMan-scheduled offscreen render can reach FUN_141e90290 with a half-seeded GX resource (rcx=0x20)
/// and access-violate; adding our concurrent composite GPU submit to the game device in the same window
/// is the ONLY behavioral delta between the crashing fix-run and the six clean control/guard runs
/// (native-Windows 2026-07-15, bd er-effects-rs-n4x). Bounded: the latches clear when the build lands.
fn profile_model_build_in_flight() -> bool {
    let r = PORTRAIT_KICK_RENDERER.load(Ordering::SeqCst);
    if r == 0 || r == TITLE_OWNER_SCAN_START_ADDRESS {
        return false;
    }
    let l754 = unsafe { safe_read_u8(r + PROFILE_RENDERER_REQ_754_OFFSET) }.unwrap_or(0);
    let l755 = unsafe { safe_read_u8(r + PROFILE_RENDERER_REQ_755_OFFSET) }.unwrap_or(0);
    l754 != 0 || l755 != 0
}

/// Presents skipped because the now-loading display window has not opened yet (RAM oracle).
pub(crate) use er_telemetry::counters::PRESENT_COMPOSITE_EARLY_SKIPS;

/// True on native Windows, where our overlay compositing must be fully suppressed. Runtime-proven across
/// 17 native-Windows runs (bd er-effects-rs-n4x, 2026-07-15): compositing on the GAME's shared D3D12
/// device -- creating resources + submitting command lists -- crashes the strict native AMD driver at
/// EVERY phase (early-boot D3D12 init: RIP-outside AV; the game's own now-loading screen: WRITE AV), while
/// composite-off is 60s clean and reaches gameplay. vkd3d/Proton isolates the shared-device work; native
/// Windows does not. Displaying our overlay on native Windows therefore needs a different architecture (a
/// SEPARATE overlay swapchain/window, or the game's own Scaleform/CSEzDraw render primitives), not a
/// when-gate. Until that redesign, native Windows runs stable with NO custom overlay visuals; the
/// swapchain-find/HDR/device-removed infra all stay in place for the eventual redesign.
fn composite_suppressed_on_native() -> bool {
    !running_under_wine()
}

/// Shared Present/Present1 composite body: draw the loading bar / save picker (and, where the render-drive
/// runs, the loading-screen portrait), then the in-world effect-selector HUD, then service overlay input.
/// Runs on the game render thread; never panics.
///
/// NATIVE-WINDOWS SPLIT (2026-07-15, bd er-effects-rs-n4x). The strict native D3D12 driver crashes on the
/// character-profile RENDER-DRIVE (proven: drive off => 60s stable + reaches gameplay), so on native
/// Windows the drive is gated off and the animated PORTRAIT cannot render. But the loading BAR + save
/// PICKER are pure DISPLAY (a CopyTextureRegion of our own CPU-rasterized pixels onto the backbuffer
/// inside the game's Present, when the backbuffer is guaranteed to be in PRESENT state) -- they need no
/// drive. So on native Windows: SKIP the portrait composite (it would only ever have a stale/absent head
/// with the drive off, and its readback path is heavier), and draw the boot-progress bar + picker
/// directly. On Wine/Proton (vkd3d), keep the full portrait-first path.
unsafe fn composite_on_game_swapchain(base: usize, this_u: usize) {
    // FPS PARITY (bd FPS-DELTA-CONFIRMED-load2-20fps-load1-45fps): once the CURRENT load epoch is
    // genuinely in-world (world-clock live for THIS fresh_deser epoch -- BOOT_VIEW_EPOCH_WORLD_LIVE was
    // set to it by the play_time_live oracle), every overlay here (portrait cover, loading bar, save
    // picker, effect selector) is a loading/menu surface with nothing to draw in gameplay. Skip ALL of
    // it so the Present hook is a pure passthrough in-world -- the reliable PER-EPOCH stop (not the stale
    // one-shot IN_WORLD_REACHED latch that never fires for load2). Isolates/kills the DLL per-frame
    // composite as a load2 FPS cost; during loading (epoch world not yet live) compositing still runs.
    {
        let cur =
            crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
        if crate::constants::BOOT_VIEW_EPOCH_WORLD_LIVE.load(Ordering::SeqCst) == cur {
            PRESENT_COMPOSITE_EARLY_SKIPS.fetch_add(1, Ordering::SeqCst);
            return;
        }
    }
    // NOTE: the offscreen RASTERIZE is NOT driven here. Present is the WRONG GX phase -- the frame's GX
    // recording is already closed, so the subcontext pool pop no-ops (black). The rasterize is driven from
    // profile_lookat_realtime_draw_tick (a DRAW-phase CSTaskImp task, live recording frame). This hook
    // only does the static composite of an already-captured RGBA.
    //
    // NATIVE-WINDOWS DISPLAY-WINDOW GATE (2026-07-15, bd er-effects-rs-n4x). Compositing during the fragile
    // early-boot D3D12 init / title crashes the strict driver even with the render-drive off (RIP-outside
    // AV ~+8s; composite-off is 60s clean). So on native Windows, do NO GPU work until the game's OWN
    // now-loading screen is live (LOADING_SCREEN_UPDATE_HITS > 0, set by the native CS::LoadingScreen hook
    // -- drive-independent, unlike PROFILE_LOADSCREEN_TABLE_BUILDS). Until then the detour is a pure
    // passthrough and the game boots exactly as it does overlay-less. (Wine/vkd3d composites throughout,
    // as before.)
    if composite_suppressed_on_native() {
        PRESENT_COMPOSITE_EARLY_SKIPS.fetch_add(1, Ordering::SeqCst);
        return;
    }
    let portrait_path = running_under_wine();
    let drew_portrait = if portrait_path && !profile_model_build_in_flight() {
        unsafe { composite_portrait_on_swapchain(base, this_u) }
    } else {
        if portrait_path {
            PRESENT_COMPOSITE_BUILD_SKIPS.fetch_add(1, Ordering::SeqCst);
        }
        false
    };
    if !drew_portrait {
        // Loading bar + save picker (the native-Windows display path; also the boot/black-gap path on Wine).
        let _ = unsafe { composite_boot_progress_on_swapchain(base, this_u) };
    }
    // Keyboard input runs on an event-driven WH_KEYBOARD_LL hook (spawned once) so every press registers
    // regardless of the ~4fps boot Present rate; the render-thread poll handles gamepad (and is the
    // keyboard fallback if the hook fails to install).
    let _ = std::panic::catch_unwind(ensure_save_picker_keyboard_hook);
    let _ = std::panic::catch_unwind(save_picker_overlay_input_tick);
}

pub(crate) use er_telemetry::counters::FACTORY2_ORIG;
type Factory2Fn =
    unsafe extern "system" fn(u32, *const windows::core::GUID, *mut *mut c_void) -> i32;

/// Detour for the `dxgi.dll!CreateDXGIFactory2` EXPORT -- logs that the GAME created a DXGI factory AFTER
/// our hook installed (the timing precondition for catching its swapchain creation via the export chain).
unsafe extern "system" fn factory2_hook(
    flags: u32,
    riid: *const windows::core::GUID,
    out: *mut *mut c_void,
) -> i32 {
    let orig = FACTORY2_ORIG.load(Ordering::SeqCst);
    let hr = if orig != 0 {
        let f: Factory2Fn = unsafe { std::mem::transmute(orig) };
        unsafe { f(flags, riid, out) }
    } else {
        -1
    };
    let factory = if out.is_null() {
        0
    } else {
        (unsafe { *out }) as usize
    };
    append_autoload_debug(format_args!(
        "present-overlay: GAME called CreateDXGIFactory2 (export) -> hr={hr} factory=0x{factory:x} (export chain viable)"
    ));
    hr
}

/// Hook the `dxgi.dll!CreateDXGIFactory2` export (a fixed export address, reliable under Wine) to learn
/// whether the game creates its DXGI factory after our install -- the precondition for the export chain
/// (factory -> CreateSwapChainForHwnd -> swapchain -> Present) that catches the game's ACTUAL swapchain.
fn install_dxgi_factory_export_hook() {
    let dxgi = match unsafe { GetModuleHandleW(windows::core::w!("dxgi.dll")) } {
        Ok(h) => h,
        Err(_) => {
            append_autoload_debug(format_args!("present-overlay: dxgi.dll not loaded yet"));
            return;
        }
    };
    let proc = unsafe {
        windows::Win32::System::LibraryLoader::GetProcAddress(
            dxgi,
            windows::core::s!("CreateDXGIFactory2"),
        )
    };
    let Some(addr) = proc else {
        append_autoload_debug(format_args!(
            "present-overlay: GetProcAddress(CreateDXGIFactory2) failed"
        ));
        return;
    };
    match unsafe { MhHook::new(addr as *mut c_void, factory2_hook as *mut c_void) } {
        Ok(hook) => {
            FACTORY2_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            std::mem::forget(hook);
            append_autoload_debug(format_args!(
                "present-overlay: hooked dxgi.dll!CreateDXGIFactory2 export 0x{:x}",
                addr as usize
            ));
        }
        Err(e) => append_autoload_debug(format_args!(
            "present-overlay: hook CreateDXGIFactory2 export failed: {e:?}"
        )),
    }
}

/// Prep the Present overlay ONCE (early): init MinHook + build a throwaway dummy swapchain only to learn
/// the IDXGISwapChain vtable module (the same-module hint for the runtime swapchain scan). The dummy's own
/// vtable funcs are NOT hooked -- under vkd3d-proton the game's swapchain is a different object, so the
/// REAL Present hook is installed later by `try_install_game_present_hook` once the GX device is up.
pub(crate) fn install_present_overlay_hook() {
    // Under a RenderDoc capture, our throwaway dummy swapchain double-registers with RenderDoc's resource
    // tracker and trips its `ref>=0` assertion (bd RENDERDOC-assert-cause-is-product-dummy-swapchain). Skip
    // the overlay entirely -- it is not needed to CAPTURE the render state.
    if crate::experiments::renderdoc_active() {
        append_autoload_debug(format_args!(
            "present-overlay: SKIPPED -- renderdoc.dll loaded (dummy swapchain would trip RenderDoc resource assert)"
        ));
        return;
    }
    if PRESENT_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "present-overlay: MH_Initialize failed: {status:?}"
            ));
            PRESENT_HOOK_INSTALLED.store(0, Ordering::SeqCst);
            return;
        }
    }
    // Module hint only: the dummy's Present addr identifies which module implements IDXGISwapChain, so the
    // runtime BFS can filter swapchain candidates by vtable-in-that-module.
    if let Some((present_addr, present1_addr)) = unsafe { resolve_present_addrs() } {
        PRESENT_RESOLVED_ADDR.store(present_addr, Ordering::SeqCst);
        PRESENT1_RESOLVED_ADDR.store(present1_addr, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "present-overlay: prepared (module hint Present=0x{present_addr:x}); scanning for game swapchain per-frame"
        ));
    } else {
        append_autoload_debug(format_args!(
            "present-overlay: prepared (no module hint; will filter by Wine-module window)"
        ));
    }
    // EARLY-BOOT SELF-PRESENT PUMP (user 2026-07-05: show the boot bar sooner than the game's
    // first present). The game's swapchain exists long before its render loop first presents
    // (~+3.7s); this thread polls for it from here (~+0.4s), VMT-swaps the moment it appears,
    // then presents our own cleared strip frames through the ORIGINAL Present until the game's
    // first real present arrives (PRESENT_HOOK_HITS > 0), at which point it stops forever and
    // the Present-detour path owns the view. Bounded, one-way stop latches, never touches the
    // game's queue.
    std::thread::Builder::new()
        .name("er-effects-boot-present-pump".to_owned())
        .spawn(boot_present_pump)
        .ok();
}

/// True when running under Wine/Proton (vkd3d), detected by the `wine_get_version` export that Wine's
/// `ntdll` exposes and native Windows never does. Cached after the first probe. Used to gate the boot
/// self-present, which is only barrier-safe under vkd3d's tolerant translation layer.
fn running_under_wine() -> bool {
    static CACHED: AtomicUsize = AtomicUsize::new(0); // 0=unknown, 1=native, 2=wine
    match CACHED.load(Ordering::SeqCst) {
        1 => return false,
        2 => return true,
        _ => {}
    }
    let is_wine = unsafe { GetModuleHandleW(windows::core::w!("ntdll.dll")) }
        .ok()
        .map(|h| {
            unsafe {
                windows::Win32::System::LibraryLoader::GetProcAddress(
                    h,
                    windows::core::s!("wine_get_version"),
                )
            }
            .is_some()
        })
        .unwrap_or(false);
    CACHED.store(if is_wine { 2 } else { 1 }, Ordering::SeqCst);
    is_wine
}

/// Self-present budget: the game's first present lands ~+3.7s; if it has not arrived by this
/// pump-relative age, something unusual is happening -- stop pumping and let the detour path
/// (which needs no pump) carry the view whenever presents do start.
const BOOT_PUMP_MAX_MS: u128 = 20_000;
/// Poll cadence while waiting for the swapchain to exist.
const BOOT_PUMP_POLL_SLEEP_MS: u64 = 10;
/// Hold the FIRST self-present at most this long waiting for the winreconfig early final-geometry
/// apply to declare a result. The apply's MoveWindow/SetWindowPos BLOCK until the game's window
/// thread starts pumping messages (~+4s, measured run 200757: issued +795ms, flushed +4010ms), so
/// the result latch fires exactly when the geometry is truly final -- present before it and the
/// XWayland remap flashes 2 black frames over the already-visible cover (the run-200757 residual).
/// The cap must sit past that ~4s flush; the fallback still beats an invisible boot on any hang.
const BOOT_PUMP_EARLY_APPLY_WAIT_MAX_MS: u128 = 8_000;
/// Self-present cadence (~30 fps -- Present(sync=1) additionally paces on vsync).
const BOOT_PUMP_FRAME_SLEEP_MS: u64 = 16;

/// Body of the `er-effects-boot-present-pump` thread. See the spawn comment for the contract.
fn boot_present_pump() {
    // Same gate as the install: the boot view + its swapchain hook are the portrait-path feature. Also
    // run under telemetry-only so the pump installs the present detour for CADENCE MEASUREMENT (the
    // composite is gated off separately; the boot self-presents only pace the pre-game-present boot phase
    // and do not touch the in-world steady-state cadence being measured).
    if !portrait_overlay_enabled() && !crate::experiments::save_override_telemetry_only() {
        return;
    }
    let start = std::time::Instant::now();
    // Pacing primitive (matches the boot profiler): a held-but-never-sent channel; `recv_timeout`
    // is the sanctioned bounded wait (plain `thread::sleep` is banned by check-no-timeouts). The
    // per-iteration terminal checks below are the real stop conditions; this only paces the poll.
    let (_tick_tx, tick_rx) = std::sync::mpsc::channel::<()>();
    let poll = std::time::Duration::from_millis(BOOT_PUMP_POLL_SLEEP_MS);
    let frame = std::time::Duration::from_millis(BOOT_PUMP_FRAME_SLEEP_MS);
    loop {
        // The game presenting is the SUCCESS terminal state: the detour path draws from here on.
        if PRESENT_HOOK_HITS.load(Ordering::SeqCst) > 0 {
            BOOT_VIEW_PUMP_STOP_REASON.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "boot-pump: game presenting -- handed over after {} self-presents",
                BOOT_VIEW_SELF_PRESENTS.load(Ordering::SeqCst)
            ));
            return;
        }
        if start.elapsed().as_millis() > BOOT_PUMP_MAX_MS {
            BOOT_VIEW_PUMP_STOP_REASON.store(2, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "boot-pump: budget exhausted with no game present -- stopping (self_presents={})",
                BOOT_VIEW_SELF_PRESENTS.load(Ordering::SeqCst)
            ));
            return;
        }
        if GAME_PRESENT_HOOKED.load(Ordering::SeqCst) == 0 {
            if let Ok(base) = game_module_base() {
                unsafe { try_install_game_present_hook(base) };
            }
            if GAME_PRESENT_HOOKED.load(Ordering::SeqCst) == 0 {
                let _ = tick_rx.recv_timeout(poll);
                continue;
            }
            let found_ms = start.elapsed().as_millis().min(usize::MAX as u128) as usize;
            BOOT_VIEW_SWAPCHAIN_FOUND_MS.store(found_ms, Ordering::SeqCst);
            // NATIVE-WINDOWS SAFETY (2026-07-15, bd er-effects-rs-n4x). The boot pump self-presents by
            // drawing on the game swapchain's backbuffer with our own PRESENT<->COPY_DEST transitions at an
            // ARBITRARY time (our thread, not synchronized with the game's frame). That barrier assumes the
            // backbuffer is in PRESENT state; on a real D3D12 driver it usually is NOT (the game may be
            // mid-render), so the wrong-state transition removes the device (Present hr=0x887a0005) and the
            // game crashes in its device-removed handler (rva=0x1e8ad57). vkd3d/Proton TOLERATES the wrong
            // barrier (translation layer), which is the only reason this ever worked. So the self-present is
            // safe ONLY under Wine/Proton; on native Windows (with OR without a co-resident overlay like
            // Special K) skip it entirely and let the Present DETOUR -- which draws INSIDE the game's Present
            // call, when D3D12 guarantees the backbuffer is in PRESENT state -- carry the boot bar + portrait.
            let self_present_safe =
                running_under_wine() && PRESENT_ACCEPT_PATH.load(Ordering::SeqCst) != 2;
            if !self_present_safe {
                BOOT_VIEW_PUMP_STOP_REASON.store(4, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "boot-pump: swapchain hooked at pump+{found_ms}ms -- native/wrapped (wine={}, accept_path={}); handing off to the Present detour without self-presenting",
                    running_under_wine(),
                    PRESENT_ACCEPT_PATH.load(Ordering::SeqCst)
                ));
                return;
            }
            append_autoload_debug(format_args!(
                "boot-pump: swapchain hooked at pump+{found_ms}ms -- self-presenting until the game's first frame"
            ));
        }
        let sc = GAME_SWAPCHAIN.load(Ordering::SeqCst);
        let orig = PRESENT_ORIG.load(Ordering::SeqCst);
        if sc == 0 || orig == 0 {
            let _ = tick_rx.recv_timeout(poll);
            continue;
        }
        // Hold the FIRST pixel off the screen until the startup window geometry is final (the
        // winreconfig early-apply latched a result): presenting before the early MoveWindow lands
        // would re-introduce a visible black flash when XWayland services the resize
        // (bd er-effects-rs-rzow). Bounded: after BOOT_PUMP_EARLY_APPLY_WAIT_MAX_MS the pump
        // presents anyway (a late flash beats an invisible boot).
        if BOOT_VIEW_SELF_PRESENTS.load(Ordering::SeqCst) == 0
            && WINRECONFIG_EARLY_APPLY_RESULT.load(Ordering::SeqCst) == 0
            && start.elapsed().as_millis() <= BOOT_PUMP_EARLY_APPLY_WAIT_MAX_MS
        {
            let _ = tick_rx.recv_timeout(poll);
            continue;
        }
        // Draw first, then re-check the game has still not presented before submitting our own
        // Present (narrows the two-thread Present race to the in-flight window; the busy-latch in
        // the composite already serializes the draw objects themselves).
        let drew = unsafe { composite_boot_progress_self_frame(sc) };
        if drew && PRESENT_HOOK_HITS.load(Ordering::SeqCst) == 0 {
            let f: PresentFn = unsafe { std::mem::transmute(orig) };
            let hr = unsafe { f(sc as *mut c_void, 1, 0) };
            if hr < 0 {
                BOOT_VIEW_PUMP_STOP_REASON.store(3, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "boot-pump: Present failed hr=0x{hr:x} -- stopping (self_presents={})",
                    BOOT_VIEW_SELF_PRESENTS.load(Ordering::SeqCst)
                ));
                return;
            }
            let n = BOOT_VIEW_SELF_PRESENTS.fetch_add(1, Ordering::SeqCst) + 1;
            if n == 1 {
                append_autoload_debug(format_args!(
                    "boot-pump: FIRST self-present on game swapchain 0x{sc:x} (pump+{}ms)",
                    start.elapsed().as_millis()
                ));
            }
        }
        let _ = tick_rx.recv_timeout(frame);
    }
}

/// Build a throwaway HWND swapchain (hidden dummy window + `CreateSwapChainForHwnd` -- composition
/// swapchains are "Not implemented" under vkd3d/Wine) and read its `Present`/`Present1` vtable entries.
/// All resources are local and dropped at scope end; only the function pointers are kept. NOTE: on
/// native Windows these are a REFERENCE, not ground truth -- a co-resident overlay may wrap the game's
/// swapchain (or ours) so the game's vtable slots legitimately differ; `find_game_swapchain` treats a
/// slot mismatch as a fallback path, not a rejection.
unsafe fn resolve_present_addrs() -> Option<(usize, usize)> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let factory: IDXGIFactory4 = match CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) {
            Ok(f) => f,
            Err(e) => {
                append_autoload_debug(format_args!(
                    "present-overlay: CreateDXGIFactory2 failed: {e:?}"
                ));
                return None;
            }
        };
        let mut device_opt: Option<ID3D12Device> = None;
        if let Err(e) = D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut device_opt) {
            append_autoload_debug(format_args!(
                "present-overlay: D3D12CreateDevice failed: {e:?}"
            ));
            return None;
        }
        let device = device_opt?;
        let queue_desc = D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            Priority: 0,
            Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
            NodeMask: 0,
        };
        let queue: ID3D12CommandQueue = match device.CreateCommandQueue(&queue_desc) {
            Ok(q) => q,
            Err(e) => {
                append_autoload_debug(format_args!(
                    "present-overlay: CreateCommandQueue failed: {e:?}"
                ));
                return None;
            }
        };
        // Hidden dummy window (Wine/vkd3d has no DirectComposition, so we need a real HWND).
        let hinstance = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(e) => {
                append_autoload_debug(format_args!(
                    "present-overlay: GetModuleHandleW failed: {e:?}"
                ));
                return None;
            }
        };
        let class_name = w!("ErEffectsOverlayDummyWnd");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(dummy_wndproc),
            hInstance: hinstance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _atom = RegisterClassW(&wc);
        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("er-effects-overlay"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            64,
            64,
            None,
            None,
            Some(hinstance.into()),
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                append_autoload_debug(format_args!(
                    "present-overlay: CreateWindowExW failed: {e:?}"
                ));
                let _ = UnregisterClassW(class_name, Some(hinstance.into()));
                return None;
            }
        };
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: 64,
            Height: 64,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_UNSPECIFIED,
            Flags: 0,
        };
        let swapchain_res = factory.CreateSwapChainForHwnd(&queue, hwnd, &desc, None, None);
        let _ = DestroyWindow(hwnd);
        let _ = UnregisterClassW(class_name, Some(hinstance.into()));
        let swapchain: IDXGISwapChain1 = match swapchain_res {
            Ok(s) => s,
            Err(e) => {
                append_autoload_debug(format_args!(
                    "present-overlay: CreateSwapChainForHwnd failed: {e:?}"
                ));
                return None;
            }
        };
        // The COM object's first qword is the vtable pointer; read Present(8) + Present1(22).
        let obj = swapchain.as_raw() as *const *const usize;
        let vtable = *obj;
        let present_addr = *vtable.add(PRESENT_VTABLE_INDEX) as usize;
        let present1_addr = *vtable.add(PRESENT1_VTABLE_INDEX) as usize;
        append_autoload_debug(format_args!(
            "present-overlay: resolved Present=0x{present_addr:x} Present1=0x{present1_addr:x}"
        ));
        if present_addr > 0x10000 && present1_addr > 0x10000 {
            Some((present_addr, present1_addr))
        } else {
            None
        }
    }))
    .ok()
    .flatten()
}

/// Best-effort log of the swapchain's backbuffer dims/format (separate from the fired-log so a GetDesc1
/// failure doesn't hide that the hook fired).
unsafe fn log_backbuffer_desc(this: *mut c_void) {
    if this as usize <= 0x10000 {
        return;
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Some(sc) = unsafe { IDXGISwapChain1::from_raw_borrowed(&this) } {
            if let Ok(desc) = unsafe { sc.GetDesc1() } {
                PRESENT_BACKBUFFER_FORMAT.store(desc.Format.0 as usize, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "present-overlay: backbuffer {}x{} format={} buffers={}",
                    desc.Width, desc.Height, desc.Format.0, desc.BufferCount
                ));
            }
        }
    }));
}

/// True if the pointer `v` is a plausible dxgi.dll vtable address: within the loaded dxgi.dll module
/// range if we can resolve it, else in the same high Wine-module window as the resolved Present addr
/// (Wine modules live at 0x6fff_xxxx_xxxx). Gate the QI on this so we never dispatch QueryInterface
/// through a garbage/half-constructed object's vtable during early boot -- that fault is a hardware
/// access violation `catch_unwind` cannot catch, and it killed the game at +673ms when the early
/// self-present pump QI-walked objects before they were fully constructed.
fn dxgi_vtable_ok(v: usize) -> bool {
    if let Some((lo, hi)) = unsafe { module_range(b"dxgi.dll\0") } {
        lo <= v && v < hi
    } else {
        let hint = PRESENT_RESOLVED_ADDR.load(Ordering::SeqCst);
        hint != 0 && (v >> 24) == (hint >> 24)
    }
}

/// Crash-proof swapchain check: `obj`'s `vtable[8]`/`vtable[22]` (Present/Present1) exactly match the
/// resolved shared DXVK addresses. PURE READS (`safe_read_usize`) + compares -- never dispatches a
/// virtual call, so a half-constructed early-boot object with a bogus vtable is rejected, not faulted.
/// Requires the resolved addrs to be known (dummy swapchain built at attach); returns false otherwise
/// so callers fall back to the QI path (only reached late, when the object is fully constructed).
fn swapchain_vtable_matches(obj: usize) -> bool {
    if obj < 0x10000 {
        return false;
    }
    let want8 = PRESENT_RESOLVED_ADDR.load(Ordering::SeqCst);
    let want22 = PRESENT1_RESOLVED_ADDR.load(Ordering::SeqCst);
    if want8 == 0 || want22 == 0 {
        return false;
    }
    let Some(vt) = (unsafe { safe_read_usize(obj) }) else {
        return false;
    };
    if !dxgi_vtable_ok(vt) {
        return false;
    }
    let got8 = unsafe { safe_read_usize(vt + PRESENT_VTABLE_INDEX * 8) };
    let got22 = unsafe { safe_read_usize(vt + PRESENT1_VTABLE_INDEX * 8) };
    got8 == Some(want8) && got22 == Some(want22)
}

/// The loaded module image containing `addr` (VirtualQuery `AllocationBase` of a `MEM_IMAGE` region),
/// or `None` when `addr` is heap/unmapped/garbage. This is the crash-safety predicate that lets the QI
/// fallback run on NON-dxgi vtables: a module-backed vtable belongs to SOME real class (dxgi proper or
/// an overlay wrapper like SpecialK/GameOverlayRenderer), so a virtual dispatch executes real code
/// instead of the uncatchable SEH fault a garbage vtable produces.
fn image_module_base(addr: usize) -> Option<usize> {
    use windows::Win32::System::Memory::{MEM_IMAGE, MEMORY_BASIC_INFORMATION, VirtualQuery};
    if addr < 0x10000 {
        return None;
    }
    let mut mbi = MEMORY_BASIC_INFORMATION::default();
    let got = unsafe {
        VirtualQuery(
            Some(addr as *const c_void),
            &mut mbi,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if got == 0 || mbi.Type != MEM_IMAGE {
        return None;
    }
    Some(mbi.AllocationBase as usize)
}

/// Best-effort module path for a module base (diagnostic log only).
fn module_name_of_base(module_base: usize) -> String {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::GetModuleFileNameA;
    let mut buf = [0u8; 260];
    let len =
        unsafe { GetModuleFileNameA(Some(HMODULE(module_base as *mut c_void)), &mut buf) } as usize;
    if len == 0 || len > buf.len() {
        return format!("module@0x{module_base:x}");
    }
    // UTF-8 Lossy: diagnostic-only module path; replacement chars still identify the module.
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

/// Classify the candidate vtable's owning module for `PRESENT_FIND_VT_MODULE_KIND`.
fn vt_module_kind(vt_module: usize, game_base: usize) -> usize {
    if vt_module == game_base {
        return 2;
    }
    if let Some((lo, hi)) = unsafe { module_range(b"dxgi.dll\0") } {
        if lo <= vt_module && vt_module < hi {
            return 1;
        }
    }
    3
}

/// QI `obj` as `IDXGISwapChain` WITHOUT the dxgi-module vtable gate. Borrow-wraps (no AddRef on
/// `obj`); the QI result is owned + dropped (its Release balances the QI AddRef), net 0 on the game
/// object. Callers MUST have pre-validated the vtable as module-backed (`image_module_base`), not the
/// game exe's, AND the candidate as pointer-stable across `PRESENT_QI_STABILITY_TRIES` consecutive
/// frames -- together those exclude the half-constructed/garbage-vtable object whose QI dispatch
/// hard-faults (SEH, uncatchable), which is the early-boot hazard the old dxgi-only gate guarded.
/// The relaxation exists because on native Windows the game's swapchain is routinely WRAPPED by
/// co-resident overlays (Special K observed in-process on the 2026-07-15 runs; Steam overlay/RTSS in
/// the wild) whose vtables live outside dxgi.dll -- the dxgi-only gate rejected the real swapchain
/// forever, which is exactly why no Windows user ever saw the boot bar or the loading portrait.
unsafe fn qi_confirms_swapchain(obj: usize) -> bool {
    let raw = obj as *mut c_void;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        match unsafe { IUnknown::from_raw_borrowed(&raw) } {
            Some(unk) => unk.cast::<IDXGISwapChain>().is_ok(),
            None => false,
        }
    }))
    .unwrap_or(false)
}

/// Find the GAME's live `IDXGISwapChain3*` via the RE-confirmed deref chain rooted at `g_GxDrawContext`:
/// `*(base+RVA)` (GxDrawContext) -> `+0x128` (output-vector begin) -> `*entry[0]` (per-window output) ->
/// `*output` (the swapchain). This is the ONLY accepted source: it reads the game's own live pointer to
/// its active render output, so a non-null vtable-matching hit IS the real swapchain. When the chain is
/// not yet populated (early boot) or a hit fails the vtable match, we return `None` and the caller simply
/// retries next frame -- never a crash, never a wrong hook.
///
/// SEAMLESS COMPAT (2026-07-05): the previous BFS fallback (scan any reachable dxgi-vtable'd object) was
/// REMOVED. `swapchain_vtable_matches` only compares the vtable pointer, which a RELEASED/dummy DXVK
/// swapchain retains -- so BFS could latch a swapchain-shaped-but-dead object. Under Seamless Co-op (ERSC
/// does its own DXGI work) such an object was reachable from the GxDrawContext root before the real
/// swapchain existed; BFS latched it (`FOUND ... via BFS after 12897 objs`), the self-present pump called
/// a COM method on it, and DXVK faulted with a null internal `this` (rcx=0). The precise chain never has
/// this failure mode because it walks to the game's single live output slot, not arbitrary heap pointers.
unsafe fn find_game_swapchain(base: usize) -> Option<usize> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let read_nn = |a: usize| -> Option<usize> {
        let v = unsafe { safe_read_usize(a) }?;
        if v < 0x10000 || v == null || v >= 0x8000_0000_0000 {
            None
        } else {
            Some(v)
        }
    };
    // Every miss names its terminal stage (RAM oracle + throttled log) -- see the FIND_STAGE block.
    let miss = |stage: usize| -> Option<usize> {
        PRESENT_FIND_STAGE.store(stage, Ordering::SeqCst);
        log_find_miss(stage);
        None
    };
    let Some(ctx) = read_nn(base + G_GX_DRAW_CONTEXT_RVA) else {
        return miss(FIND_STAGE_CTX_NULL);
    };
    // Precise chain: output-vector begin -> entry[0] output object -> output[0] swapchain.
    let Some(vec_begin) = read_nn(ctx + GXDC_OUTPUT_VEC_BEGIN_OFFSET) else {
        return miss(FIND_STAGE_VEC_NULL);
    };
    let Some(entry0) = read_nn(vec_begin) else {
        return miss(FIND_STAGE_ENTRY_NULL);
    };
    let Some(sc) = read_nn(entry0) else {
        return miss(FIND_STAGE_CANDIDATE_NULL);
    };
    PRESENT_FIND_CANDIDATE.store(sc, Ordering::SeqCst);
    // Same-candidate streak: the QI fallback's stability gate.
    let streak = if PRESENT_FIND_LAST_CANDIDATE.swap(sc, Ordering::SeqCst) == sc {
        PRESENT_FIND_STREAK.fetch_add(1, Ordering::SeqCst) + 1
    } else {
        PRESENT_FIND_STREAK.store(1, Ordering::SeqCst);
        1
    };
    let Some(vt) = (unsafe { safe_read_usize(sc) }) else {
        return miss(FIND_STAGE_VT_UNREADABLE);
    };
    PRESENT_FIND_CANDIDATE_VT.store(vt, Ordering::SeqCst);
    PRESENT_FIND_GOT8.store(
        unsafe { safe_read_usize(vt + PRESENT_VTABLE_INDEX * 8) }.unwrap_or(0),
        Ordering::SeqCst,
    );
    PRESENT_FIND_GOT22.store(
        unsafe { safe_read_usize(vt + PRESENT1_VTABLE_INDEX * 8) }.unwrap_or(0),
        Ordering::SeqCst,
    );
    // FAST PATH: crash-proof exact vtable match (read-only compares against the dummy-resolved
    // Present/Present1). Sufficient under vkd3d-proton, where every dxgi swapchain shares one DXVK
    // CDXGISwapChain vtable.
    if swapchain_vtable_matches(sc) {
        PRESENT_FIND_STAGE.store(FIND_STAGE_ACCEPTED_VTABLE, Ordering::SeqCst);
        if PRESENT_ACCEPT_PATH.swap(1, Ordering::SeqCst) == 0 {
            append_autoload_debug(format_args!(
                "present-overlay: FOUND game swapchain 0x{sc:x} via g_GxDrawContext chain (ctx=0x{ctx:x}, exact vtable match)"
            ));
        }
        return Some(sc);
    }
    // NATIVE-WINDOWS FALLBACK: on real dxgi the exact match can fail legitimately -- co-resident
    // overlays (Special K, Steam GameOverlayRenderer, RTSS) wrap the game's swapchain in their own
    // object whose vtable lives in THEIR module, and native dxgi may use distinct concrete vtables
    // where DXVK uses one. The chain still points at the game's single live output (never BFS, so
    // the Seamless dead-swapchain hazard from 2026-07-05 stays excluded); accept it when the vtable
    // is module-backed (real class, QI dispatch executes real code), NOT the game exe's (a game-exe
    // vtable means the chain layout is wrong -- swapchain impls never live there), the candidate has
    // been stable for PRESENT_QI_STABILITY_TRIES frames, and QI confirms IDXGISwapChain.
    let Some(vt_module) = image_module_base(vt) else {
        PRESENT_FIND_VT_MODULE_KIND.store(0, Ordering::SeqCst);
        return miss(FIND_STAGE_VT_NOT_MODULE);
    };
    let kind = vt_module_kind(vt_module, base);
    PRESENT_FIND_VT_MODULE_KIND.store(kind, Ordering::SeqCst);
    if kind == 2 {
        return miss(FIND_STAGE_VT_IN_GAME_EXE);
    }
    if streak < PRESENT_QI_STABILITY_TRIES {
        return miss(FIND_STAGE_STABILITY_WAIT);
    }
    if unsafe { qi_confirms_swapchain(sc) } {
        PRESENT_FIND_STAGE.store(FIND_STAGE_ACCEPTED_QI, Ordering::SeqCst);
        if PRESENT_ACCEPT_PATH.swap(2, Ordering::SeqCst) == 0 {
            append_autoload_debug(format_args!(
                "present-overlay: FOUND game swapchain 0x{sc:x} via g_GxDrawContext chain + QI fallback (vt=0x{vt:x} in {}, streak={streak})",
                module_name_of_base(vt_module)
            ));
        }
        return Some(sc);
    }
    miss(FIND_STAGE_QI_REJECTED)
}

/// Throttled, stage-attributed miss line: early tries (1/300/1200), then once every 3600 (~once a
/// minute at frame cadence) so a session-length retry stays visible without flooding the log.
fn log_find_miss(stage: usize) {
    let t = GAME_SWAPCHAIN_FIND_TRIES.load(Ordering::SeqCst);
    if !(t == 1 || t == 300 || t == 1200 || (t > 0 && t % 3600 == 0)) {
        return;
    }
    let candidate = PRESENT_FIND_CANDIDATE.load(Ordering::SeqCst);
    let vt = PRESENT_FIND_CANDIDATE_VT.load(Ordering::SeqCst);
    let got8 = PRESENT_FIND_GOT8.load(Ordering::SeqCst);
    let got22 = PRESENT_FIND_GOT22.load(Ordering::SeqCst);
    let want8 = PRESENT_RESOLVED_ADDR.load(Ordering::SeqCst);
    let want22 = PRESENT1_RESOLVED_ADDR.load(Ordering::SeqCst);
    let streak = PRESENT_FIND_STREAK.load(Ordering::SeqCst);
    let vt_module = image_module_base(vt);
    let module = vt_module.map_or_else(|| "<none>".to_owned(), module_name_of_base);
    append_autoload_debug(format_args!(
        "present-overlay: find miss try={t} stage={stage} candidate=0x{candidate:x} vt=0x{vt:x} vt_module={module} got8=0x{got8:x}/want8=0x{want8:x} got22=0x{got22:x}/want22=0x{want22:x} streak={streak}"
    ));
}

/// Overwrite the COM `vtable[index]` slot at `slot_addr` so it points at `new_fn`, returning the previous
/// function pointer (for call-through), or `None` if the page could not be made writable. Patches a DATA
/// pointer in the dxgi vtable -- NOT the function body -- so it sidesteps the W^X code-page patch that
/// MinHook cannot apply on Wine's dxgi.dll (it reports MH_OK yet the detour never fires). `VirtualProtect`
/// the 8-byte slot to RW, swap the pointer, then restore the original page protection.
unsafe fn vtable_swap_slot(slot_addr: usize, new_fn: usize) -> Option<usize> {
    use windows::Win32::System::Memory::{PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualProtect};
    let slot = slot_addr as *mut usize;
    let mut old_prot = PAGE_PROTECTION_FLAGS(0);
    if unsafe {
        VirtualProtect(
            slot_addr as *const c_void,
            std::mem::size_of::<usize>(),
            PAGE_READWRITE,
            &mut old_prot,
        )
    }
    .is_err()
    {
        return None;
    }
    let old = unsafe { core::ptr::read_volatile(slot) };
    unsafe { core::ptr::write_volatile(slot, new_fn) };
    let mut restored = PAGE_PROTECTION_FLAGS(0);
    let _ = unsafe {
        VirtualProtect(
            slot_addr as *const c_void,
            std::mem::size_of::<usize>(),
            old_prot,
            &mut restored,
        )
    };
    Some(old)
}

/// Per-frame (from a recurring game task): once the GX device is up, find the GAME's swapchain and redirect
/// its REAL Present(8)/Present1(22) via a vtable-slot swap (NOT a MinHook code patch -- that reports MH_OK
/// but never fires on Wine's dxgi.dll). One-shot (latched on success); bounded retries.
pub(crate) unsafe fn try_install_game_present_hook(base: usize) {
    // Install the present detour for the overlay composite OR for telemetry-only CADENCE MEASUREMENT.
    // The detour records present cadence (record_present_frame_stats) read-only every frame; the
    // flow-modifying composite call is separately gated on portrait_overlay_enabled() below. So a
    // flow-faithful telemetry-only vanilla baseline (overlay off) still gets the present-cadence +
    // GetFrameStatistics + GX semaphores WITHOUT the overlay composite -- decoupling the instrumentation
    // from the feature it measures (bd present-cadence-gx-instrumentation-coupled-to-overlay-install-gate).
    if (!portrait_overlay_enabled() && !crate::experiments::save_override_telemetry_only())
        || crate::experiments::renderdoc_active()
        || PRESENT_HOOK_INSTALLED.load(Ordering::SeqCst) == 0
        || GAME_PRESENT_HOOKED.load(Ordering::SeqCst) != 0
    {
        return;
    }
    // Count attempts for telemetry/log throttling only. The old 3600-try lifetime cap dated from the
    // removed budgeted-BFS era; today's find is four bounded safe-reads, cheap at frame cadence, and
    // a lifetime cap would permanently disarm the hook when the chain populates late (the corrected
    // 2026-07-15 native-Windows analysis showed the finder must keep retrying through the title era).
    let tries = GAME_SWAPCHAIN_FIND_TRIES.fetch_add(1, Ordering::SeqCst) + 1;
    let sc = match unsafe { find_game_swapchain(base) } {
        Some(s) => s,
        None => return,
    };
    let vt = match unsafe { safe_read_usize(sc) } {
        Some(v) => v,
        None => return,
    };
    let present8 = unsafe { safe_read_usize(vt + PRESENT_VTABLE_INDEX * 8) }.unwrap_or(0);
    let present22 = unsafe { safe_read_usize(vt + PRESENT1_VTABLE_INDEX * 8) }.unwrap_or(0);
    if present8 <= 0x10000 || present22 <= 0x10000 {
        return;
    }
    // Latch BEFORE swapping so a retry can't double-install.
    if GAME_PRESENT_HOOKED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    // Latch the found swapchain + base so the detours can gate the composite to the GAME's swapchain.
    GAME_SWAPCHAIN.store(sc, Ordering::SeqCst);
    GAME_BASE.store(base, Ordering::SeqCst);
    // Save the originals FIRST (the detours tail-call them), then VMT-swap the swapchain's vtable slots.
    // We patch the vtable DATA pointers, NOT the function bodies: MinHook's code-page byte-patch reports
    // MH_OK on Wine's dxgi.dll yet never intercepts (HOOKED-but-never-fired, a W^X code-page refusal),
    // whereas a vtable-slot swap redirects the game's `swapchain->vtable[8]/[22]` calls deterministically.
    // The slot is 8-byte aligned, so the render thread reading it concurrently sees old-or-new, never torn.
    PRESENT_ORIG.store(present8, Ordering::SeqCst);
    PRESENT1_ORIG.store(present22, Ordering::SeqCst);
    let slot8 = vt + PRESENT_VTABLE_INDEX * 8;
    let slot22 = vt + PRESENT1_VTABLE_INDEX * 8;
    let swap8 = unsafe { vtable_swap_slot(slot8, present_hook as usize) }.is_some();
    let swap22 = unsafe { vtable_swap_slot(slot22, present1_hook as usize) }.is_some();
    // Read the slots back so a failed patch is visible in the log (self-validating: a later FIRED line plus
    // readback=true proves the redirect took; readback=true with no FIRED means the game presents elsewhere).
    let now8 = unsafe { safe_read_usize(slot8) }.unwrap_or(0);
    let now22 = unsafe { safe_read_usize(slot22) }.unwrap_or(0);
    append_autoload_debug(format_args!(
        "present-overlay: VMT-swap game swapchain 0x{sc:x} (tries={tries}) Present8=0x{present8:x} Present1_22=0x{present22:x} slot8@0x{slot8:x} swap={swap8} readback={} slot22@0x{slot22:x} swap={swap22} readback={}",
        now8 == present_hook as usize,
        now22 == present1_hook as usize,
    ));
}

unsafe extern "system" fn dummy_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
