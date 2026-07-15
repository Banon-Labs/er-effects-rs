//! Native-Windows loading-experience overlay: a SEPARATE top-level window with our OWN D3D12 device and
//! swapchain, layered on top of the game to OWN what the user sees during boot + every loading screen.
//!
//! WHY (bd er-effects-rs-8jz, er-effects-rs-n4x). On the strict native AMD D3D12 driver, compositing on
//! the GAME's shared device (creating resources + submitting command lists that race the game's own async
//! rendering) crashes at every phase -- proven across 17 native-Windows runs. vkd3d/Proton isolates the
//! shared-device work; native Windows does not. So on native Windows we do NOT touch the game's device.
//! Instead we render the loading bar / static portrait / stats / save picker on our OWN device+swapchain,
//! in a topmost borderless window covering the game. Fully isolated -> nothing we do can corrupt the game.
//!
//! OWNERSHIP CYCLE (user 2026-07-15): this is NOT a one-shot boot cover. Because we can no longer paint
//! onto the game's native loading screen, the window must REPLACE it every time -- SHOW whenever a loading
//! sequence is active (boot + every subsequent native loading screen), HIDE during gameplay, re-own on the
//! next load. The window is persistent (created once, toggled), never destroyed per load.
//!
//! This file is the MINIMAL PROOF stage: create the window + device + swapchain, clear to a visible color
//! while shown, and honor the SHOW/HIDE flag. Content (bar/portrait/stats/picker) is layered on next.

#![allow(unused_imports)]

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_COMMAND_QUEUE_FLAG_NONE,
    D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
    D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_FENCE_FLAG_NONE, D3D12_RESOURCE_BARRIER,
    D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_FLAG_NONE,
    D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_STATE_PRESENT,
    D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATES, D3D12_RESOURCE_TRANSITION_BARRIER,
    D3D12CreateDevice, ID3D12CommandAllocator, ID3D12CommandQueue, ID3D12DescriptorHeap,
    ID3D12Device, ID3D12Fence, ID3D12GraphicsCommandList, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    IDXGIFactory4, IDXGISwapChain1, IDXGISwapChain3,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetSystemMetrics, MSG,
    PM_REMOVE, PeekMessageW, RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_SHOWNOACTIVATE,
    ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};
use windows::core::{Interface, w};

use super::LOADING_SCREEN_BAR_PROGRESS_PERMILLE;
use crate::telemetry::append_autoload_debug;

/// Visibility request set by the game task each frame from the loading state: 1 = SHOW (cover the game),
/// 0 = HIDE (release to gameplay). Starts SHOWN so the boot black gap is covered immediately.
pub(crate) static NATIVE_OVERLAY_SHOW: AtomicUsize = AtomicUsize::new(1);
/// One-shot install latch.
pub(crate) static NATIVE_OVERLAY_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Frames presented (RAM oracle: the overlay is live + presenting).
pub(crate) static NATIVE_OVERLAY_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Last init stage reached (RAM oracle for diagnosis): 1=thread, 2=class, 3=window, 4=factory, 5=device,
/// 6=queue, 7=swapchain, 8=rtv-heap, 9=cmd-objects, 10=render-loop-entered.
pub(crate) static NATIVE_OVERLAY_STAGE: AtomicUsize = AtomicUsize::new(0);

/// Install the native-Windows loading overlay (idempotent). Spawns a dedicated thread that owns the
/// window + our D3D12 device and runs the render loop. Safe to call unconditionally at attach; the caller
/// gates it to native Windows.
pub(crate) fn install_native_overlay() {
    if NATIVE_OVERLAY_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("er-effects-native-overlay".to_owned())
        .spawn(|| {
            let _ = std::panic::catch_unwind(|| unsafe { native_overlay_run() });
        });
}

/// Progress (0..=1000) for the overlay loading bar. During the game's native loading screen we mirror its
/// real progress; before that (boot / title), a gentle frame-driven creep (capped) reads as "working".
fn overlay_progress_permille() -> usize {
    let native = LOADING_SCREEN_BAR_PROGRESS_PERMILLE.load(Ordering::SeqCst);
    if native > 0 {
        return native.min(1000);
    }
    (NATIVE_OVERLAY_FRAMES.load(Ordering::SeqCst) / 8).min(350)
}

unsafe extern "system" fn overlay_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Transition a backbuffer between resource states on our own list (net-zero ref on `res`).
unsafe fn overlay_transition(
    list: &ID3D12GraphicsCommandList,
    res: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) {
    let mut barrier = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: ManuallyDrop::new(Some(res.clone())),
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

unsafe fn native_overlay_run() {
    NATIVE_OVERLAY_STAGE.store(1, Ordering::SeqCst);

    // --- 1. window (topmost, borderless, non-activating so it never steals input from the game) ---
    let hinstance = match unsafe { GetModuleHandleW(None) } {
        Ok(h) => h,
        Err(e) => {
            append_autoload_debug(format_args!(
                "native-overlay: GetModuleHandleW failed: {e:?}"
            ));
            return;
        }
    };
    let class_name = w!("ErEffectsLoadingOverlay");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(overlay_wndproc),
        hInstance: hinstance.into(),
        lpszClassName: class_name,
        ..Default::default()
    };
    let _atom = unsafe { RegisterClassW(&wc) };
    NATIVE_OVERLAY_STAGE.store(2, Ordering::SeqCst);

    let sw = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let sh = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let (win_w, win_h) = if sw > 0 && sh > 0 {
        (sw, sh)
    } else {
        (1920, 1080)
    };

    let hwnd = match unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class_name,
            w!("er-effects loading"),
            WS_POPUP,
            0,
            0,
            win_w,
            win_h,
            None,
            None,
            Some(hinstance.into()),
            None,
        )
    } {
        Ok(h) => h,
        Err(e) => {
            append_autoload_debug(format_args!(
                "native-overlay: CreateWindowExW failed: {e:?}"
            ));
            return;
        }
    };
    NATIVE_OVERLAY_STAGE.store(3, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "native-overlay: window created hwnd=0x{:x} {win_w}x{win_h}",
        hwnd.0 as usize
    ));

    // --- 2. our OWN D3D12 device + swapchain (isolated from the game's device) ---
    let factory: IDXGIFactory4 = match unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) } {
        Ok(f) => f,
        Err(e) => {
            append_autoload_debug(format_args!(
                "native-overlay: CreateDXGIFactory2 failed: {e:?}"
            ));
            return;
        }
    };
    NATIVE_OVERLAY_STAGE.store(4, Ordering::SeqCst);

    let mut device_opt: Option<ID3D12Device> = None;
    if let Err(e) = unsafe { D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut device_opt) } {
        append_autoload_debug(format_args!(
            "native-overlay: D3D12CreateDevice failed: {e:?}"
        ));
        return;
    }
    let Some(device) = device_opt else {
        append_autoload_debug(format_args!("native-overlay: device is None"));
        return;
    };
    NATIVE_OVERLAY_STAGE.store(5, Ordering::SeqCst);

    let queue: ID3D12CommandQueue = match unsafe {
        device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            Priority: 0,
            Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
            NodeMask: 0,
        })
    } {
        Ok(q) => q,
        Err(e) => {
            append_autoload_debug(format_args!(
                "native-overlay: CreateCommandQueue failed: {e:?}"
            ));
            return;
        }
    };
    NATIVE_OVERLAY_STAGE.store(6, Ordering::SeqCst);

    const BUFFERS: u32 = 2;
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: win_w as u32,
        Height: win_h as u32,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        Stereo: false.into(),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: BUFFERS,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        AlphaMode: DXGI_ALPHA_MODE_IGNORE,
        Flags: 0,
    };
    let swapchain1: IDXGISwapChain1 =
        match unsafe { factory.CreateSwapChainForHwnd(&queue, hwnd, &desc, None, None) } {
            Ok(s) => s,
            Err(e) => {
                append_autoload_debug(format_args!(
                    "native-overlay: CreateSwapChainForHwnd failed: {e:?}"
                ));
                return;
            }
        };
    let swapchain: IDXGISwapChain3 = match swapchain1.cast() {
        Ok(s) => s,
        Err(e) => {
            append_autoload_debug(format_args!("native-overlay: swapchain cast failed: {e:?}"));
            return;
        }
    };
    NATIVE_OVERLAY_STAGE.store(7, Ordering::SeqCst);

    // --- RTV heap + backbuffer views ---
    let rtv_heap: ID3D12DescriptorHeap = match unsafe {
        device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
            NumDescriptors: BUFFERS,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
            NodeMask: 0,
        })
    } {
        Ok(h) => h,
        Err(e) => {
            append_autoload_debug(format_args!(
                "native-overlay: CreateDescriptorHeap failed: {e:?}"
            ));
            return;
        }
    };
    let rtv_size =
        unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) } as usize;
    let rtv_base = unsafe { rtv_heap.GetCPUDescriptorHandleForHeapStart() };
    let mut backbuffers: Vec<ID3D12Resource> = Vec::with_capacity(BUFFERS as usize);
    for i in 0..BUFFERS {
        let bb: ID3D12Resource = match unsafe { swapchain.GetBuffer(i) } {
            Ok(b) => b,
            Err(e) => {
                append_autoload_debug(format_args!("native-overlay: GetBuffer({i}) failed: {e:?}"));
                return;
            }
        };
        let handle = D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: rtv_base.ptr + i as usize * rtv_size,
        };
        unsafe { device.CreateRenderTargetView(&bb, None, handle) };
        backbuffers.push(bb);
    }
    NATIVE_OVERLAY_STAGE.store(8, Ordering::SeqCst);

    // --- command objects + fence ---
    let allocator: ID3D12CommandAllocator =
        match unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) } {
            Ok(a) => a,
            Err(e) => {
                append_autoload_debug(format_args!(
                    "native-overlay: CreateCommandAllocator failed: {e:?}"
                ));
                return;
            }
        };
    let list: ID3D12GraphicsCommandList = match unsafe {
        device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
    } {
        Ok(l) => l,
        Err(e) => {
            append_autoload_debug(format_args!(
                "native-overlay: CreateCommandList failed: {e:?}"
            ));
            return;
        }
    };
    let _ = unsafe { list.Close() };
    let fence: ID3D12Fence = match unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) } {
        Ok(f) => f,
        Err(e) => {
            append_autoload_debug(format_args!("native-overlay: CreateFence failed: {e:?}"));
            return;
        }
    };
    let fence_event = match unsafe { CreateEventW(None, false, false, None) } {
        Ok(e) => e,
        Err(e) => {
            append_autoload_debug(format_args!("native-overlay: CreateEventW failed: {e:?}"));
            return;
        }
    };
    let mut fence_val: u64 = 0;
    NATIVE_OVERLAY_STAGE.store(9, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "native-overlay: D3D12 ready (own device); entering render loop"
    ));

    // Pacing primitive (thread::sleep is banned; a held-but-never-sent channel + recv_timeout is the
    // sanctioned bounded wait used elsewhere). Present(sync=1) vsync-paces while shown; this paces hidden.
    let (_tick_tx, tick_rx) = std::sync::mpsc::channel::<()>();
    let hidden_poll = std::time::Duration::from_millis(16);
    let mut shown = false;
    NATIVE_OVERLAY_STAGE.store(10, Ordering::SeqCst);

    loop {
        // Pump our window's messages (non-blocking).
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            let _ = unsafe { TranslateMessage(&msg) };
            unsafe { DispatchMessageW(&msg) };
        }

        let want_show = NATIVE_OVERLAY_SHOW.load(Ordering::SeqCst) != 0;
        if want_show != shown {
            let _ = unsafe {
                ShowWindow(
                    hwnd,
                    if want_show {
                        SW_SHOWNOACTIVATE
                    } else {
                        SW_HIDE
                    },
                )
            };
            shown = want_show;
        }
        if !shown {
            // Released to gameplay: no GPU work, no present. Pace and re-check.
            let _ = tick_rx.recv_timeout(hidden_poll);
            continue;
        }

        // --- render a frame on OUR device (minimal proof: clear to a visible color) ---
        let idx = unsafe { swapchain.GetCurrentBackBufferIndex() } as usize;
        let bb = &backbuffers[idx];
        if unsafe { allocator.Reset() }.is_err() || unsafe { list.Reset(&allocator, None) }.is_err()
        {
            let _ = tick_rx.recv_timeout(hidden_poll);
            continue;
        }
        unsafe {
            overlay_transition(
                &list,
                bb,
                D3D12_RESOURCE_STATE_PRESENT,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
            )
        };
        let handle = D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: rtv_base.ptr + idx * rtv_size,
        };
        // Charcoal cover (owns the screen), then a horizontal loading bar: a dim track with an Elden-Ring
        // gold fill sized to the progress. Drawn with ClearRenderTargetView sub-rects -- no upload needed.
        unsafe { list.ClearRenderTargetView(handle, &[0.02, 0.02, 0.03, 1.0], None) };
        let permille = overlay_progress_permille();
        let bar_w = win_w * 11 / 20; // ~55% width
        let bar_h = (win_h / 240).max(6); // ~9px at 2160p
        let bar_x = (win_w - bar_w) / 2;
        let bar_y = win_h * 82 / 100;
        let track = RECT {
            left: bar_x,
            top: bar_y,
            right: bar_x + bar_w,
            bottom: bar_y + bar_h,
        };
        unsafe { list.ClearRenderTargetView(handle, &[0.12, 0.10, 0.08, 1.0], Some(&[track])) };
        let fill_w = bar_w * permille as i32 / 1000;
        if fill_w > 0 {
            let fill = RECT {
                left: bar_x,
                top: bar_y,
                right: bar_x + fill_w,
                bottom: bar_y + bar_h,
            };
            unsafe { list.ClearRenderTargetView(handle, &[0.80, 0.66, 0.36, 1.0], Some(&[fill])) };
        }
        unsafe {
            overlay_transition(
                &list,
                bb,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                D3D12_RESOURCE_STATE_PRESENT,
            )
        };
        if unsafe { list.Close() }.is_err() {
            let _ = tick_rx.recv_timeout(hidden_poll);
            continue;
        }
        let list_any = list.cast().ok();
        unsafe { queue.ExecuteCommandLists(&[list_any]) };

        if unsafe { swapchain.Present(1, DXGI_PRESENT(0)) }.is_ok() {
            NATIVE_OVERLAY_FRAMES.fetch_add(1, Ordering::SeqCst);
        }

        // GPU sync so we don't reset the allocator while the frame is in flight.
        fence_val += 1;
        if unsafe { queue.Signal(&fence, fence_val) }.is_ok()
            && unsafe { fence.GetCompletedValue() } < fence_val
            && unsafe { fence.SetEventOnCompletion(fence_val, fence_event) }.is_ok()
        {
            unsafe { WaitForSingleObject(fence_event, INFINITE) };
        }
    }
}
