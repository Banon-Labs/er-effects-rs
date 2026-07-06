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

/// Original `IDXGISwapChain::Present` / `Present1` trampolines; 0 until installed.
static PRESENT_ORIG: AtomicUsize = AtomicUsize::new(0);
static PRESENT1_ORIG: AtomicUsize = AtomicUsize::new(0);
static PRESENT_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Per-frame Present hit counter (RAM semaphore that the overlay hook is live + firing).
pub(crate) static PRESENT_HOOK_HITS: AtomicUsize = AtomicUsize::new(0);

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
/// Set once we've found the GAME's swapchain and hooked its REAL Present/Present1. (The earlier "dummy
/// swapchain vtable funcs differ under vkd3d-proton" theory was unsound -- under Proton all dxgi.dll
/// swapchains share one DXVK `CDXGISwapChain` vtable, so Present(8)/Present1(22) are the same function for
/// every swapchain. The real prior blocker was the FIND missing the object, so MinHook was never attempted
/// on a real swapchain; dinput8 MinHooks fire, so the hook path itself is sound.)
static GAME_PRESENT_HOOKED: AtomicUsize = AtomicUsize::new(0);
static GAME_SWAPCHAIN_FIND_TRIES: AtomicUsize = AtomicUsize::new(0);
/// The dummy swapchain's resolved `Present(8)` / `Present1(22)` addrs. Under vkd3d-proton EVERY dxgi
/// swapchain shares one DXVK `CDXGISwapChain` vtable, so the GAME swapchain's `vtable[8]`/`vtable[22]`
/// are byte-identical to these (runtime-proven: resolved 0x..209f0 == VMT-swapped Present8 0x..209f0).
/// This lets `swapchain_vtable_matches` confirm a candidate by READING + comparing these two slots --
/// never by dispatching `QueryInterface`, which faults on a half-constructed early-boot object whose
/// dxgi-ranged-but-bogus vtable can't be caught by `catch_unwind` (that AV killed the pump at +726ms).
static PRESENT_RESOLVED_ADDR: AtomicUsize = AtomicUsize::new(0);
static PRESENT1_RESOLVED_ADDR: AtomicUsize = AtomicUsize::new(0);
/// The found GAME swapchain pointer + game module base, latched in `try_install_game_present_hook`. The
/// Present detour composites the portrait only when `this` matches `GAME_SWAPCHAIN` -- the shared dxgi
/// vtable means the detour ALSO fires for our throwaway dummy swapchain, which we must never draw on.
static GAME_SWAPCHAIN: AtomicUsize = AtomicUsize::new(0);
static GAME_BASE: AtomicUsize = AtomicUsize::new(0);

/// Detour for `IDXGISwapChain::Present(this, SyncInterval, Flags)`. Phase 1: log-only, then tail-call the
/// original. `this` IS the game's swapchain (we never created it), so a real overlay draws onto its
/// current backbuffer here. Must never panic (runs on the game's render thread every frame).
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
            // NOTE: the offscreen RASTERIZE is NOT driven here. Present is the WRONG GX phase -- the frame's
            // GX recording is already closed, so the subcontext pool pop no-ops (black). The rasterize is
            // driven from profile_lookat_realtime_draw_tick (a DRAW-phase CSTaskImp task, live recording
            // frame). This hook only does the static composite of an already-captured RGBA.
            // The boot-progress view owns the pre-Continue black gap; once the portrait composite starts
            // drawing (loading window) it wins, and the boot view self-stops on the same signals.
            let drew_portrait = unsafe { composite_portrait_on_swapchain(base, this_u) };
            if !drew_portrait {
                let _ = unsafe { composite_boot_progress_on_swapchain(base, this_u) };
            }
        }
    }
    let orig = PRESENT_ORIG.load(Ordering::SeqCst);
    if orig != 0 {
        let f: PresentFn = unsafe { std::mem::transmute(orig) };
        unsafe { f(this, sync, flags) }
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
            let drew_portrait = unsafe { composite_portrait_on_swapchain(base, this_u) };
            if !drew_portrait {
                let _ = unsafe { composite_boot_progress_on_swapchain(base, this_u) };
            }
        }
    }
    let orig = PRESENT1_ORIG.load(Ordering::SeqCst);
    if orig != 0 {
        let f: Present1Fn = unsafe { std::mem::transmute(orig) };
        unsafe { f(this, sync, flags, params) }
    } else {
        0
    }
}

static FACTORY2_ORIG: AtomicUsize = AtomicUsize::new(0);
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

/// Self-present budget: the game's first present lands ~+3.7s; if it has not arrived by this
/// pump-relative age, something unusual is happening -- stop pumping and let the detour path
/// (which needs no pump) carry the view whenever presents do start.
const BOOT_PUMP_MAX_MS: u128 = 20_000;
/// Poll cadence while waiting for the swapchain to exist.
const BOOT_PUMP_POLL_SLEEP_MS: u64 = 10;
/// Self-present cadence (~30 fps -- Present(sync=1) additionally paces on vsync).
const BOOT_PUMP_FRAME_SLEEP_MS: u64 = 16;

/// Body of the `er-effects-boot-present-pump` thread. See the spawn comment for the contract.
fn boot_present_pump() {
    // Same gate as the install: the boot view + its swapchain hook are the portrait-path feature.
    if !portrait_lookat_enabled() {
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

/// Build a throwaway COMPOSITION swapchain (no HWND/window needed) and read its `Present` vtable entry.
/// All resources are local and dropped at scope end; only the function pointer (shared across all
/// IDXGISwapChain instances) is kept.
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

/// True if `obj` is a live COM object that QIs as `IDXGISwapChain`. Borrow-wraps (no AddRef on `obj`);
/// the QI result is owned + dropped (its Release balances the QI AddRef), leaving the game object net 0.
/// The QI is dispatched through `obj`'s vtable, so we FIRST require that vtable to be a dxgi.dll address
/// (`dxgi_vtable_ok`) -- QI'ing a not-yet-constructed object with a garbage vtable hard-faults (SEH,
/// uncatchable), which is exactly the early-boot hazard the self-present pump exposed.
unsafe fn is_idxgi_swapchain(obj: usize) -> bool {
    if obj < 0x10000 {
        return false;
    }
    let vt = match unsafe { safe_read_usize(obj) } {
        Some(v) => v,
        None => return false,
    };
    if !dxgi_vtable_ok(vt) {
        return false;
    }
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
    let ctx = read_nn(base + G_GX_DRAW_CONTEXT_RVA)?; // GxDrawContext*
    // Precise chain: output-vector begin -> entry[0] output object -> output[0] swapchain.
    if let Some(sc) = read_nn(ctx + GXDC_OUTPUT_VEC_BEGIN_OFFSET)
        .and_then(read_nn)
        .and_then(read_nn)
    {
        // Crash-proof vtable-match first (read-only, safe during early boot); QI only as a fallback
        // when the resolved addrs are somehow unknown (never reached in practice, and only late).
        if swapchain_vtable_matches(sc)
            || (PRESENT_RESOLVED_ADDR.load(Ordering::SeqCst) == 0
                && unsafe { is_idxgi_swapchain(sc) })
        {
            let t = GAME_SWAPCHAIN_FIND_TRIES.load(Ordering::SeqCst);
            if t <= 1 {
                append_autoload_debug(format_args!(
                    "present-overlay: FOUND game swapchain 0x{sc:x} via g_GxDrawContext chain (ctx=0x{ctx:x})"
                ));
            }
            return Some(sc);
        }
    }
    // Chain not yet populated (early frame) or a candidate failed the vtable match. Do NOT scan the heap
    // for a fallback -- a vtable-matching object may be a dead swapchain (see SEAMLESS COMPAT above).
    // Return None so the caller retries next frame; throttle a diagnostic so a persistent miss is visible.
    let t = GAME_SWAPCHAIN_FIND_TRIES.load(Ordering::SeqCst);
    if t == 1 || t == 300 || t == 1200 {
        append_autoload_debug(format_args!(
            "present-overlay: chain miss try={t} ctx=0x{ctx:x} (real swapchain not yet in g_GxDrawContext output vector)"
        ));
    }
    None
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
    if !portrait_lookat_enabled()
        || PRESENT_HOOK_INSTALLED.load(Ordering::SeqCst) == 0
        || GAME_PRESENT_HOOKED.load(Ordering::SeqCst) != 0
    {
        return;
    }
    // Bound the find attempts (each is a budgeted BFS) so a never-appearing swapchain can't spin forever.
    let tries = GAME_SWAPCHAIN_FIND_TRIES.fetch_add(1, Ordering::SeqCst) + 1;
    if tries > 3600 {
        return;
    }
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
