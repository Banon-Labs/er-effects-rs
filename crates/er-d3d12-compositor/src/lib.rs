#![cfg(windows)]

//! Minimal D3D12 swapchain compositor for loading-bar validation.
//!
//! This is the intentionally narrow first slice: install a Present/Present1 hook,
//! render a crate-owned loading-bar frame on the CPU, upload it, and copy the
//! opaque strip onto the current swapchain backbuffer immediately before the
//! original Present. Product DLL state and Elden Ring semaphores enter only through
//! a caller-supplied frame provider; the standalone wrapper can still prove the
//! Present path without pulling in er-effects-rs.

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicUsize, Ordering};

use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_COMMAND_QUEUE_FLAG_NONE,
    D3D12_CPU_PAGE_PROPERTY_UNKNOWN, D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
    D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_FENCE_FLAG_NONE, D3D12_HEAP_FLAG_NONE,
    D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_UPLOAD, D3D12_MEMORY_POOL_UNKNOWN,
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_0,
    D3D12_RESOURCE_BARRIER_FLAG_NONE, D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_DIMENSION_BUFFER, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_NONE,
    D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_GENERIC_READ,
    D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATES,
    D3D12_RESOURCE_TRANSITION_BARRIER, D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0,
    D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
    D3D12_TEXTURE_LAYOUT_ROW_MAJOR, D3D12_TEXTURE_LAYOUT_UNKNOWN, D3D12CreateDevice,
    ID3D12CommandAllocator, ID3D12CommandQueue, ID3D12DescriptorHeap, ID3D12Device, ID3D12Fence,
    ID3D12GraphicsCommandList, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_UNSPECIFIED, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory4, IDXGISwapChain,
    IDXGISwapChain1, IDXGISwapChain3,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW,
    UnregisterClassW, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{Interface, w};

use er_loading_bar::{BarStyle, LoadingLabel, PHASE_COUNT, RgbaFrame, render_label_bar_frame};

/// Logging sink used by the wrapper DLL/product caller. Optional; default is silent.
pub type LogFn = fn(std::fmt::Arguments<'_>);

/// CPU-rendered frame plus destination coordinates in the current D3D12 backbuffer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompositorFrame {
    pub rgba: RgbaFrame,
    pub dst_x: usize,
    pub dst_y: usize,
}

impl CompositorFrame {
    /// Explicit no-draw frame. Providers use this before their semantic
    /// on-screen gate opens; it is not a compositor failure.
    pub fn hidden() -> Self {
        Self {
            rgba: RgbaFrame {
                width: 0,
                height: 0,
                pixels: Vec::new(),
            },
            dst_x: 0,
            dst_y: 0,
        }
    }

    fn is_hidden(&self) -> bool {
        self.rgba.width == 0 || self.rgba.height == 0 || self.rgba.pixels.is_empty()
    }
}

/// Render a frame for the current backbuffer.
///
/// Arguments are `(backbuffer_width, backbuffer_height, present_frame_index)`.
/// Providers must return a frame fully contained in the supplied backbuffer bounds.
pub type FrameProviderFn = fn(usize, usize, usize) -> CompositorFrame;

const PRESENT_VTABLE_INDEX: usize = 8;
const PRESENT1_VTABLE_INDEX: usize = 22;
const DXGI_OK: i32 = 0;
const MAX_BACKBUFFER_DIM: u32 = 16_384;

type PresentFn = unsafe extern "system" fn(*mut c_void, u32, u32) -> i32;
type Present1Fn = unsafe extern "system" fn(*mut c_void, u32, u32, *const c_void) -> i32;

static LOG_SINK: AtomicUsize = AtomicUsize::new(0);
static FRAME_PROVIDER: AtomicUsize = AtomicUsize::new(0);
static INSTALLED: AtomicUsize = AtomicUsize::new(0);
static PRESENT_ORIG: AtomicUsize = AtomicUsize::new(0);
static PRESENT1_ORIG: AtomicUsize = AtomicUsize::new(0);
static PRESENT_HITS: AtomicUsize = AtomicUsize::new(0);
static DRAW_HITS: AtomicUsize = AtomicUsize::new(0);
static DRAW_FAILS: AtomicUsize = AtomicUsize::new(0);
static LAST_FAIL_CODE: AtomicUsize = AtomicUsize::new(0);
static LAST_BACKBUFFER_W: AtomicUsize = AtomicUsize::new(0);
static LAST_BACKBUFFER_H: AtomicUsize = AtomicUsize::new(0);
static RESIZE_LOGS: AtomicUsize = AtomicUsize::new(0);

/// Install a process-global log sink for compositor diagnostics.
pub fn set_log_sink(sink: LogFn) {
    LOG_SINK.store(sink as usize, Ordering::Release);
}

/// Install a process-global frame provider.
///
/// The default provider is a deterministic smoke frame. Product callers should
/// install a provider that renders from real loading semaphores before installing
/// the Present compositor.
pub fn set_frame_provider(provider: FrameProviderFn) {
    FRAME_PROVIDER.store(provider as usize, Ordering::Release);
}

/// One-shot install of the loading-bar Present compositor.
///
/// The compositor hooks the DXGI swapchain vtable function resolved from a
/// hidden dummy swapchain. Under vkd3d/Proton this catches the game swapchain's
/// Present path because DXVK swapchains share the same vtable implementation.
/// Native wrapped-swapchain fallback remains for the later extraction pass.
pub fn install_loading_bar_present_compositor() {
    if INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("er-loading-bar-present".to_owned())
        .spawn(|| unsafe {
            if !install_hooks() {
                INSTALLED.store(0, Ordering::SeqCst);
            }
        });
}

/// Copy a CPU-rendered RGBA frame onto the current backbuffer of an existing swapchain.
///
/// Product callers use this when they already own the Present hook / stop gates but want the same
/// upload + copy implementation as the standalone loading-bar compositor. When `clear_first` is set,
/// the helper clears the full backbuffer to opaque black before copying the frame.
///
/// # Safety
///
/// `swapchain_raw` must be a live `IDXGISwapChain` pointer whose current backbuffer is in PRESENT
/// state, and the caller must ensure no other command list is simultaneously transitioning that
/// backbuffer.
pub unsafe fn copy_rgba_frame_to_swapchain(
    swapchain_raw: usize,
    frame: &RgbaFrame,
    dst_x: usize,
    dst_y: usize,
    clear_first: bool,
) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let this = swapchain_raw as *mut c_void;
        let Some(swapchain) = IDXGISwapChain::from_raw_borrowed(&this) else {
            LAST_FAIL_CODE.store(1, Ordering::SeqCst);
            return false;
        };
        let idx = swapchain
            .cast::<IDXGISwapChain3>()
            .ok()
            .map(|sc3| sc3.GetCurrentBackBufferIndex())
            .unwrap_or(0);
        let Ok(backbuffer) = swapchain.GetBuffer::<ID3D12Resource>(idx) else {
            LAST_FAIL_CODE.store(5, Ordering::SeqCst);
            return false;
        };
        let bb_desc = backbuffer.GetDesc();
        let bb_w = bb_desc.Width.min(u64::from(MAX_BACKBUFFER_DIM)) as usize;
        let bb_h = bb_desc.Height.min(MAX_BACKBUFFER_DIM) as usize;
        if bb_w == 0
            || bb_h == 0
            || bb_desc.Width > u64::from(MAX_BACKBUFFER_DIM)
            || bb_desc.Height > MAX_BACKBUFFER_DIM
        {
            LAST_FAIL_CODE.store(6, Ordering::SeqCst);
            return false;
        }
        if dst_x > bb_w
            || dst_y > bb_h
            || frame.width > bb_w.saturating_sub(dst_x)
            || frame.height > bb_h.saturating_sub(dst_y)
        {
            LAST_FAIL_CODE.store(7, Ordering::SeqCst);
            return false;
        }
        copy_frame_to_backbuffer(&backbuffer, frame, dst_x, dst_y, clear_first)
    }))
    .unwrap_or(false)
}

/// Snapshot of the compositor's process-local counters.
pub fn stats() -> PresentCompositorStats {
    PresentCompositorStats {
        installed: INSTALLED.load(Ordering::SeqCst) != 0,
        present_hits: PRESENT_HITS.load(Ordering::SeqCst),
        draw_hits: DRAW_HITS.load(Ordering::SeqCst),
        draw_fails: DRAW_FAILS.load(Ordering::SeqCst),
        last_fail_code: LAST_FAIL_CODE.load(Ordering::SeqCst),
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PresentCompositorStats {
    pub installed: bool,
    pub present_hits: usize,
    pub draw_hits: usize,
    pub draw_fails: usize,
    pub last_fail_code: usize,
}

fn log(args: std::fmt::Arguments<'_>) {
    let raw = LOG_SINK.load(Ordering::Acquire);
    if raw != 0 {
        // SAFETY: only `set_log_sink` stores a `LogFn` value in this atomic.
        let sink: LogFn = unsafe { std::mem::transmute(raw) };
        sink(args);
    }
}

unsafe fn install_hooks() -> bool {
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            log(format_args!(
                "loading-bar-present: MH_Initialize failed: {status:?}"
            ));
            return false;
        }
    }

    let Some((present_addr, present1_addr)) = resolve_present_addrs() else {
        log(format_args!(
            "loading-bar-present: could not resolve dummy swapchain Present addrs"
        ));
        return false;
    };

    let present_hook = match unsafe {
        MhHook::new(present_addr as *mut c_void, present_hook as *mut c_void)
    } {
        Ok(hook) => hook,
        Err(status) => {
            log(format_args!(
                "loading-bar-present: Present hook create failed: {status:?} addr=0x{present_addr:x}"
            ));
            return false;
        }
    };
    PRESENT_ORIG.store(present_hook.trampoline() as usize, Ordering::SeqCst);
    if let Err(status) = unsafe { present_hook.queue_enable() } {
        log(format_args!(
            "loading-bar-present: Present queue_enable failed: {status:?}"
        ));
        return false;
    }
    let _leaked_present = Box::leak(Box::new(present_hook));

    if present1_addr != present_addr {
        match unsafe { MhHook::new(present1_addr as *mut c_void, present1_hook as *mut c_void) } {
            Ok(hook) => {
                PRESENT1_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                if let Err(status) = unsafe { hook.queue_enable() } {
                    log(format_args!(
                        "loading-bar-present: Present1 queue_enable failed: {status:?}"
                    ));
                } else {
                    let _leaked_present1 = Box::leak(Box::new(hook));
                }
            }
            Err(status) => log(format_args!(
                "loading-bar-present: Present1 hook create skipped/failed: {status:?} addr=0x{present1_addr:x}"
            )),
        }
    }

    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            log(format_args!(
                "loading-bar-present: installed Present=0x{present_addr:x} Present1=0x{present1_addr:x}"
            ));
            true
        }
        status => {
            log(format_args!(
                "loading-bar-present: MH_ApplyQueued failed: {status:?}"
            ));
            false
        }
    }
}

unsafe extern "system" fn present_hook(this: *mut c_void, sync: u32, flags: u32) -> i32 {
    let hit = PRESENT_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if hit == 1 {
        log(format_args!(
            "loading-bar-present: first Present hit this=0x{:x}",
            this as usize
        ));
    }
    unsafe { draw_loading_bar_on_swapchain(this, hit) };
    let orig = PRESENT_ORIG.load(Ordering::SeqCst);
    if orig != 0 {
        let f: PresentFn = unsafe { std::mem::transmute(orig) };
        unsafe { f(this, sync, flags) }
    } else {
        DXGI_OK
    }
}

unsafe extern "system" fn present1_hook(
    this: *mut c_void,
    sync: u32,
    flags: u32,
    params: *const c_void,
) -> i32 {
    let hit = PRESENT_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if hit == 1 {
        log(format_args!(
            "loading-bar-present: first Present1 hit this=0x{:x}",
            this as usize
        ));
    }
    unsafe { draw_loading_bar_on_swapchain(this, hit) };
    let orig = PRESENT1_ORIG.load(Ordering::SeqCst);
    if orig != 0 {
        let f: Present1Fn = unsafe { std::mem::transmute(orig) };
        unsafe { f(this, sync, flags, params) }
    } else {
        DXGI_OK
    }
}

unsafe extern "system" fn dummy_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Resolve `IDXGISwapChain::Present` and `IDXGISwapChain1::Present1` from a temporary D3D12
/// swapchain. This is the process-wide deep owner of the dummy window, device, queue, swapchain
/// descriptor, and vtable walk used by both the standalone compositor and product callers.
///
/// The temporary window and class are released before this returns. Only the two function addresses
/// escape the helper.
pub fn resolve_present_addrs() -> Option<(usize, usize)> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).ok()?;
        let mut device_opt: Option<ID3D12Device> = None;
        D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut device_opt).ok()?;
        let device = device_opt?;
        let queue: ID3D12CommandQueue = device
            .CreateCommandQueue::<ID3D12CommandQueue>(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                Priority: 0,
                Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
                NodeMask: 0,
            })
            .ok()?;
        let hinstance = GetModuleHandleW(None).ok()?;
        let class_name = w!("ErLoadingBarDummySwapchain");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(dummy_wndproc),
            hInstance: hinstance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("er-loading-bar-dummy"),
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
            Ok(hwnd) => hwnd,
            Err(_) => {
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
        let swapchain_result = factory.CreateSwapChainForHwnd(&queue, hwnd, &desc, None, None);
        let _ = DestroyWindow(hwnd);
        let _ = UnregisterClassW(class_name, Some(hinstance.into()));
        let swapchain: IDXGISwapChain1 = swapchain_result.ok()?;
        let obj = swapchain.as_raw() as *const *const usize;
        let vtable = *obj;
        let present_addr = *vtable.add(PRESENT_VTABLE_INDEX) as usize;
        let present1_addr = *vtable.add(PRESENT1_VTABLE_INDEX) as usize;
        if present_addr > 0x10000 && present1_addr > 0x10000 {
            Some((present_addr, present1_addr))
        } else {
            None
        }
    }))
    .ok()
    .flatten()
}

unsafe fn draw_loading_bar_on_swapchain(this: *mut c_void, frame_index: usize) {
    if this as usize <= 0x10000 {
        return;
    }
    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        draw_loading_bar_on_swapchain_inner(this, frame_index)
    }))
    .unwrap_or(false);
    if ok {
        let n = DRAW_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        if n == 1 {
            log(format_args!(
                "loading-bar-present: first backbuffer copy succeeded"
            ));
        }
    } else {
        let code = LAST_FAIL_CODE.load(Ordering::SeqCst);
        if code != 0 {
            let n = DRAW_FAILS.fetch_add(1, Ordering::SeqCst) + 1;
            if n <= 8 || n.is_power_of_two() {
                log(format_args!(
                    "loading-bar-present: draw failed count={n} code={code}"
                ));
            }
        }
    }
}

unsafe fn draw_loading_bar_on_swapchain_inner(this: *mut c_void, frame_index: usize) -> bool {
    let Some(swapchain) = (unsafe { IDXGISwapChain::from_raw_borrowed(&this) }) else {
        LAST_FAIL_CODE.store(1, Ordering::SeqCst);
        return false;
    };
    let idx = swapchain
        .cast::<IDXGISwapChain3>()
        .ok()
        .map(|sc3| unsafe { sc3.GetCurrentBackBufferIndex() })
        .unwrap_or(0);
    let Ok(backbuffer) = (unsafe { swapchain.GetBuffer::<ID3D12Resource>(idx) }) else {
        LAST_FAIL_CODE.store(5, Ordering::SeqCst);
        return false;
    };
    let bb_desc = unsafe { backbuffer.GetDesc() };
    let bb_w = bb_desc.Width.min(u64::from(MAX_BACKBUFFER_DIM)) as usize;
    let bb_h = bb_desc.Height.min(MAX_BACKBUFFER_DIM) as usize;
    if bb_w == 0
        || bb_h == 0
        || bb_desc.Width > u64::from(MAX_BACKBUFFER_DIM)
        || bb_desc.Height > MAX_BACKBUFFER_DIM
    {
        LAST_FAIL_CODE.store(6, Ordering::SeqCst);
        return false;
    }
    // Our hidden dummy swapchain is 64x64. The global vtable hook also catches it;
    // never draw validation content there.
    if bb_w == 64 && bb_h == 64 {
        LAST_FAIL_CODE.store(4, Ordering::SeqCst);
        return false;
    }
    note_backbuffer_size(bb_w, bb_h);
    let frame = provider_frame(bb_w, bb_h, frame_index);
    if frame.is_hidden() {
        LAST_FAIL_CODE.store(0, Ordering::SeqCst);
        return false;
    }
    if frame.dst_x > bb_w
        || frame.dst_y > bb_h
        || frame.rgba.width > bb_w.saturating_sub(frame.dst_x)
        || frame.rgba.height > bb_h.saturating_sub(frame.dst_y)
    {
        LAST_FAIL_CODE.store(7, Ordering::SeqCst);
        return false;
    }
    unsafe { copy_frame_to_backbuffer(&backbuffer, &frame.rgba, frame.dst_x, frame.dst_y, false) }
}

fn note_backbuffer_size(width: usize, height: usize) {
    let old_w = LAST_BACKBUFFER_W.swap(width, Ordering::SeqCst);
    let old_h = LAST_BACKBUFFER_H.swap(height, Ordering::SeqCst);
    if (old_w != width || old_h != height) && RESIZE_LOGS.fetch_add(1, Ordering::SeqCst) < 16 {
        log(format_args!(
            "loading-bar-present: backbuffer size {width}x{height}"
        ));
    }
}

fn provider_frame(width: usize, height: usize, frame_index: usize) -> CompositorFrame {
    let raw = FRAME_PROVIDER.load(Ordering::Acquire);
    if raw != 0 {
        // SAFETY: only `set_frame_provider` stores a `FrameProviderFn` value.
        let provider: FrameProviderFn = unsafe { std::mem::transmute(raw) };
        provider(width, height, frame_index)
    } else {
        smoke_frame(width, height, frame_index)
    }
}

fn smoke_frame(width: usize, height: usize, frame_index: usize) -> CompositorFrame {
    // Standalone validation has no Elden Ring load semaphore provider, so this is deliberately a
    // slow, looping, non-terminal demo. A fast clamp to 1000 makes a human-observed startup look
    // indistinguishable from the stale-full product bug this DLL exists to diagnose.
    const PHASE_FRAMES: usize = 180;
    const LOOP_FRAMES: usize = 1_800;
    const SMOKE_MIN_PERMILLE: usize = 40;
    const SMOKE_MAX_PERMILLE: usize = 900;

    let phase = (frame_index / PHASE_FRAMES) % PHASE_COUNT;
    let loop_frame = frame_index % LOOP_FRAMES;
    let progress =
        SMOKE_MIN_PERMILLE + loop_frame * (SMOKE_MAX_PERMILLE - SMOKE_MIN_PERMILLE) / LOOP_FRAMES;
    let mut text = String::new();
    LoadingLabel::new(
        er_loading_bar::phase_label(phase),
        phase + 1,
        PHASE_COUNT,
        "STANDALONE D3D12 SMOKE",
        loop_frame + 1,
        LOOP_FRAMES,
    )
    .write_text(&mut text);
    let frame_width = (width.saturating_mul(9) / 10).max(1);
    let rgba = render_label_bar_frame(frame_width, 2, &text, progress, BarStyle::default());
    let bottom_margin = (height / 24).clamp(12, 48);
    let dst_x = width.saturating_sub(rgba.width) / 2;
    let dst_y = height.saturating_sub(rgba.height.saturating_add(bottom_margin));
    CompositorFrame { rgba, dst_x, dst_y }
}

unsafe fn copy_frame_to_backbuffer(
    backbuffer: &ID3D12Resource,
    frame: &RgbaFrame,
    dst_x: usize,
    dst_y: usize,
    clear_first: bool,
) -> bool {
    if frame.width == 0 || frame.height == 0 || frame.pixels.is_empty() {
        LAST_FAIL_CODE.store(10, Ordering::SeqCst);
        return false;
    }
    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { backbuffer.GetDevice(&mut device_opt) }.is_err() {
        LAST_FAIL_CODE.store(11, Ordering::SeqCst);
        return false;
    }
    let Some(device) = device_opt else {
        LAST_FAIL_CODE.store(12, Ordering::SeqCst);
        return false;
    };
    let Ok(queue) = (unsafe {
        device.CreateCommandQueue::<ID3D12CommandQueue>(&D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            Priority: 0,
            Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
            NodeMask: 0,
        })
    }) else {
        LAST_FAIL_CODE.store(13, Ordering::SeqCst);
        return false;
    };
    let Ok(allocator) = (unsafe {
        device.CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT)
    }) else {
        LAST_FAIL_CODE.store(14, Ordering::SeqCst);
        return false;
    };
    let Ok(list) = (unsafe {
        device.CreateCommandList::<_, _, ID3D12GraphicsCommandList>(
            0,
            D3D12_COMMAND_LIST_TYPE_DIRECT,
            &allocator,
            None,
        )
    }) else {
        LAST_FAIL_CODE.store(15, Ordering::SeqCst);
        return false;
    };

    let tex_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: frame.width as u64,
        Height: frame.height as u32,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    };
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total = 0u64;
    unsafe {
        device.GetCopyableFootprints(
            &tex_desc,
            0,
            1,
            0,
            Some(&mut footprint),
            None,
            None,
            Some(&mut total),
        )
    };
    if total == 0 {
        LAST_FAIL_CODE.store(16, Ordering::SeqCst);
        return false;
    }
    let upload_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: total,
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
    let up_heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_UPLOAD,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    };
    let mut upload_opt: Option<ID3D12Resource> = None;
    if unsafe {
        device.CreateCommittedResource(
            &up_heap,
            D3D12_HEAP_FLAG_NONE,
            &upload_desc,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut upload_opt,
        )
    }
    .is_err()
    {
        LAST_FAIL_CODE.store(17, Ordering::SeqCst);
        return false;
    }
    let Some(upload) = upload_opt else {
        LAST_FAIL_CODE.store(18, Ordering::SeqCst);
        return false;
    };

    let row_pitch = footprint.Footprint.RowPitch as usize;
    let src_row = frame.width.saturating_mul(er_loading_bar::RGBA8_BPP);
    let mut mapped: *mut c_void = std::ptr::null_mut();
    if unsafe { upload.Map(0, None, Some(&mut mapped)) }.is_err() || mapped.is_null() {
        LAST_FAIL_CODE.store(19, Ordering::SeqCst);
        return false;
    }
    let dst = unsafe { std::slice::from_raw_parts_mut(mapped as *mut u8, total as usize) };
    for y in 0..frame.height {
        let so = y.saturating_mul(src_row);
        let doff = y.saturating_mul(row_pitch);
        if so + src_row <= frame.pixels.len() && doff + src_row <= dst.len() {
            dst[doff..doff + src_row].copy_from_slice(&frame.pixels[so..so + src_row]);
        }
    }
    unsafe { upload.Unmap(0, None) };

    if clear_first {
        let Ok(rtv_heap) = (unsafe {
            device.CreateDescriptorHeap::<ID3D12DescriptorHeap>(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                NumDescriptors: 1,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                NodeMask: 0,
            })
        }) else {
            LAST_FAIL_CODE.store(23, Ordering::SeqCst);
            return false;
        };
        let rtv = unsafe { rtv_heap.GetCPUDescriptorHandleForHeapStart() };
        unsafe { device.CreateRenderTargetView(backbuffer, None, rtv) };
        unsafe {
            transition(
                &list,
                backbuffer,
                D3D12_RESOURCE_STATE_PRESENT,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
            )
        };
        unsafe { list.ClearRenderTargetView(rtv, &[0.0, 0.0, 0.0, 1.0], None) };
        unsafe {
            transition(
                &list,
                backbuffer,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )
        };
    } else {
        unsafe {
            transition(
                &list,
                backbuffer,
                D3D12_RESOURCE_STATE_PRESENT,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )
        };
    }
    let mut dst_loc = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(backbuffer.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    let mut src_loc = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(upload.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    unsafe {
        list.CopyTextureRegion(&dst_loc, dst_x as u32, dst_y as u32, 0, &src_loc, None);
        ManuallyDrop::drop(&mut dst_loc.pResource);
        ManuallyDrop::drop(&mut src_loc.pResource);
        transition(
            &list,
            backbuffer,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_PRESENT,
        );
    }
    if unsafe { list.Close() }.is_err() {
        LAST_FAIL_CODE.store(20, Ordering::SeqCst);
        return false;
    }
    let list_any = list.cast().ok();
    unsafe { queue.ExecuteCommandLists(&[list_any]) };

    let Ok(fence) = (unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }) else {
        LAST_FAIL_CODE.store(21, Ordering::SeqCst);
        return false;
    };
    let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else {
        LAST_FAIL_CODE.store(22, Ordering::SeqCst);
        return false;
    };
    if unsafe { queue.Signal(&fence, 1) }.is_ok()
        && unsafe { fence.GetCompletedValue() } < 1
        && unsafe { fence.SetEventOnCompletion(1, event) }.is_ok()
    {
        unsafe { WaitForSingleObject(event, INFINITE) };
    }
    LAST_FAIL_CODE.store(0, Ordering::SeqCst);
    true
}

unsafe fn transition(
    list: &ID3D12GraphicsCommandList,
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) {
    let barrier = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: ManuallyDrop::new(Some(resource.clone())),
                Subresource: 0,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    };
    unsafe { list.ResourceBarrier(std::slice::from_ref(&barrier)) };
    unsafe {
        ManuallyDrop::drop(&mut ManuallyDrop::into_inner(barrier.Anonymous.Transition).pResource)
    };
}
