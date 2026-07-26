//! Windows-proof bridge renderer: a child overlay inside Elden Ring's own window with its own D3D12 device/swapchain.
//!
//! The native loading/title GFx surfaces are game-owned and have disjoint startup/teardown lifetimes during
//! vanilla loading/autoload. This bridge owns the full launch/load cover without touching Elden Ring's D3D
//! device, swapchain, or Present path. It starts visible, renders on an isolated device, hides when the
//! loaded character render flags are ready, and shows again on later non-ready load/title phases.
//! GFx/MemoryFile remains an opportunistic asset seam, not the product cover surface.

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_BOX, D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC,
    D3D12_COMMAND_QUEUE_FLAG_NONE, D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
    D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_FLAG_NONE, D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
    D3D12_FENCE_FLAG_NONE, D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_READBACK,
    D3D12_HEAP_TYPE_UPLOAD, D3D12_MEMORY_POOL_UNKNOWN, D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    D3D12_RANGE, D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_0,
    D3D12_RESOURCE_BARRIER_FLAG_NONE, D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_DIMENSION_BUFFER, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_NONE,
    D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE,
    D3D12_RESOURCE_STATE_GENERIC_READ, D3D12_RESOURCE_STATE_PRESENT,
    D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATES, D3D12_RESOURCE_TRANSITION_BARRIER,
    D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0,
    D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
    D3D12_TEXTURE_LAYOUT_ROW_MAJOR, D3D12_TEXTURE_LAYOUT_UNKNOWN, ID3D12CommandAllocator,
    ID3D12CommandQueue, ID3D12DescriptorHeap, ID3D12Device, ID3D12Fence, ID3D12GraphicsCommandList,
    ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory4, IDXGISwapChain1,
    IDXGISwapChain3,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcessId, INFINITE, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, EnumWindows, GetClientRect, GetParent,
    GetWindowThreadProcessId, IsWindowVisible, MSG, MoveWindow, PM_REMOVE, PeekMessageW,
    RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow, TranslateMessage, WNDCLASSW, WS_CHILD,
    WS_EX_NOACTIVATE, WS_VISIBLE,
};
use windows::core::{BOOL, Error, HRESULT, Interface, PCSTR, w};

use super::gpu_readback::boot_view_render_frame;

use crate::constants::{
    LOADING_SCREEN_BAR_ENABLED, LOADING_SCREEN_BAR_MAX_FRAME, LOADING_SCREEN_CLOSE_SENT,
    LOADING_SCREEN_UPDATE_HITS,
};
use crate::telemetry::append_autoload_debug;

/// Visibility requested by the game task: 1 = cover title/loading, 0 = release to gameplay.
pub(crate) static NATIVE_OVERLAY_SHOW: AtomicUsize = AtomicUsize::new(1);
/// One-shot install latch.
pub(crate) static NATIVE_OVERLAY_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Frames presented by the isolated overlay renderer.
pub(crate) static NATIVE_OVERLAY_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Last init stage reached: 1=thread, 2=class, 3=window, 4=factory, 5=device, 6=queue,
/// 7=swapchain, 8=rtv-heap, 9=cmd-objects, 10=render-loop-entered.
pub(crate) static NATIVE_OVERLAY_STAGE: AtomicUsize = AtomicUsize::new(0);
/// Render frames cleared/drawn on our isolated backbuffer before Present.
pub(crate) static NATIVE_OVERLAY_DRAW_HITS: AtomicUsize = AtomicUsize::new(0);
/// Last HRESULT-ish stage failure code (0 = none; values are local diagnostic buckets).
pub(crate) static NATIVE_OVERLAY_FAILURE: AtomicUsize = AtomicUsize::new(0);
/// 1 while the game's native loading GFx/bar surface is live. Telemetry only: GFx is not the product cover.
pub(crate) static NATIVE_OVERLAY_HANDOFF_READY: AtomicUsize = AtomicUsize::new(0);
/// Counts frames where the bridge saw the native loading GFx/bar surface while remaining owner of cover.
pub(crate) static NATIVE_OVERLAY_HANDOFF_READY_HITS: AtomicUsize = AtomicUsize::new(0);
/// Counts frames where the game loading UI existed, the player was absent, and the bridge stayed visible.
pub(crate) static NATIVE_OVERLAY_COVERING_LOADING_HITS: AtomicUsize = AtomicUsize::new(0);
/// Counts frames where the bridge rendered real full-frame/content coverage, not just the 1px marker.
pub(crate) static NATIVE_OVERLAY_CONTENT_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// One-shot objective pixel readback attempts against the bridge backbuffer.
pub(crate) static NATIVE_OVERLAY_PIXEL_PROBE_HITS: AtomicUsize = AtomicUsize::new(0);
/// One-shot bridge pixel readback matches against the unique clear color.
pub(crate) static NATIVE_OVERLAY_PIXEL_PROBE_MATCHES: AtomicUsize = AtomicUsize::new(0);
/// Last readback pixel as packed RGBA8.
pub(crate) static NATIVE_OVERLAY_PIXEL_PROBE_RGBA: AtomicUsize = AtomicUsize::new(0);
/// Nonzero parent game HWND used for the bridge child window.
pub(crate) static NATIVE_OVERLAY_PARENT_HWND: AtomicUsize = AtomicUsize::new(0);
/// 1 when the bridge HWND was created as a child of the game's own visible window.
pub(crate) static NATIVE_OVERLAY_CHILD_WINDOW: AtomicUsize = AtomicUsize::new(0);
/// 1 when GetParent(child) still equals the game HWND.
pub(crate) static NATIVE_OVERLAY_CHILD_PARENT_MATCH: AtomicUsize = AtomicUsize::new(0);
/// 1 when the child overlay client size matches the parent game client size.
pub(crate) static NATIVE_OVERLAY_CHILD_CLIENT_MATCH: AtomicUsize = AtomicUsize::new(0);
/// Counts frames where parent/child relation or client-size match failed.
pub(crate) static NATIVE_OVERLAY_CHILD_GEOMETRY_MISMATCH_HITS: AtomicUsize = AtomicUsize::new(0);
/// Counts child-window resizes performed to follow game-window client/fullscreen reconfiguration.
pub(crate) static NATIVE_OVERLAY_CHILD_RESIZE_HITS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static NATIVE_OVERLAY_PARENT_CLIENT_W: AtomicUsize = AtomicUsize::new(0);
pub(crate) static NATIVE_OVERLAY_PARENT_CLIENT_H: AtomicUsize = AtomicUsize::new(0);
pub(crate) static NATIVE_OVERLAY_CHILD_CLIENT_W: AtomicUsize = AtomicUsize::new(0);
pub(crate) static NATIVE_OVERLAY_CHILD_CLIENT_H: AtomicUsize = AtomicUsize::new(0);

const FAILURE_DYNAMIC_FACTORY: usize = 1;
const FAILURE_DYNAMIC_DEVICE: usize = 2;
const FAILURE_WINDOW: usize = 3;
const FAILURE_PARENT_WINDOW: usize = 4;

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

pub(crate) fn native_overlay_release_tick(release_ready: bool) {
    let handoff_ready = native_loading_surface_handoff_ready();
    NATIVE_OVERLAY_HANDOFF_READY.store(usize::from(handoff_ready), Ordering::SeqCst);
    if handoff_ready {
        NATIVE_OVERLAY_HANDOFF_READY_HITS.fetch_add(1, Ordering::SeqCst);
        if !release_ready {
            NATIVE_OVERLAY_COVERING_LOADING_HITS.fetch_add(1, Ordering::SeqCst);
        }
    }
    NATIVE_OVERLAY_SHOW.store(usize::from(!release_ready), Ordering::SeqCst);
}

fn native_loading_surface_handoff_ready() -> bool {
    LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst) != 0
        && LOADING_SCREEN_BAR_ENABLED.load(Ordering::SeqCst) != 0
        && LOADING_SCREEN_BAR_MAX_FRAME.load(Ordering::SeqCst) != 0
        && LOADING_SCREEN_CLOSE_SENT.load(Ordering::SeqCst) == 0
}

unsafe extern "system" fn overlay_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

unsafe extern "system" fn enum_current_process_visible_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == unsafe { GetCurrentProcessId() } && unsafe { IsWindowVisible(hwnd) }.as_bool() {
        let out = lparam.0 as *mut HWND;
        if !out.is_null() {
            unsafe { *out = hwnd };
            return BOOL(0);
        }
    }
    BOOL(1)
}

fn find_current_process_visible_window() -> Option<HWND> {
    let mut hwnd = HWND(std::ptr::null_mut());
    unsafe {
        let _ = EnumWindows(
            Some(enum_current_process_visible_window),
            LPARAM((&mut hwnd as *mut HWND) as isize),
        );
    }
    if !hwnd.0.is_null() { Some(hwnd) } else { None }
}

fn wait_for_parent_game_window() -> Option<(HWND, i32, i32)> {
    let (_tick_tx, tick_rx) = std::sync::mpsc::channel::<()>();
    for _ in 0..600 {
        if let Some(parent) = find_current_process_visible_window() {
            let mut rect = RECT::default();
            if unsafe { GetClientRect(parent, &mut rect) }.is_ok() {
                let w = (rect.right - rect.left).max(1);
                let h = (rect.bottom - rect.top).max(1);
                return Some((parent, w, h));
            }
        }
        let _ = tick_rx.recv_timeout(std::time::Duration::from_millis(16));
    }
    None
}

fn rect_size(rect: RECT) -> (usize, usize) {
    (
        (rect.right - rect.left).max(0) as usize,
        (rect.bottom - rect.top).max(0) as usize,
    )
}

fn sync_child_to_parent_client(parent: HWND, child: HWND) {
    let mut parent_rect = RECT::default();
    let mut child_rect = RECT::default();
    if unsafe { GetClientRect(parent, &mut parent_rect) }.is_err()
        || unsafe { GetClientRect(child, &mut child_rect) }.is_err()
    {
        return;
    }
    let (parent_w, parent_h) = rect_size(parent_rect);
    let (child_w, child_h) = rect_size(child_rect);
    if parent_w == 0 || parent_h == 0 || (parent_w == child_w && parent_h == child_h) {
        return;
    }
    if unsafe { MoveWindow(child, 0, 0, parent_w as i32, parent_h as i32, true) }.is_ok() {
        NATIVE_OVERLAY_CHILD_RESIZE_HITS.fetch_add(1, Ordering::SeqCst);
    }
}

fn record_child_geometry_oracle(parent: HWND, child: HWND) {
    let parent_match = unsafe { GetParent(child) }.is_ok_and(|actual| actual == parent);
    NATIVE_OVERLAY_CHILD_PARENT_MATCH.store(usize::from(parent_match), Ordering::SeqCst);

    let mut parent_rect = RECT::default();
    let mut child_rect = RECT::default();
    let parent_ok = unsafe { GetClientRect(parent, &mut parent_rect) }.is_ok();
    let child_ok = unsafe { GetClientRect(child, &mut child_rect) }.is_ok();
    let (parent_w, parent_h) = if parent_ok {
        rect_size(parent_rect)
    } else {
        (0, 0)
    };
    let (child_w, child_h) = if child_ok {
        rect_size(child_rect)
    } else {
        (0, 0)
    };
    NATIVE_OVERLAY_PARENT_CLIENT_W.store(parent_w, Ordering::SeqCst);
    NATIVE_OVERLAY_PARENT_CLIENT_H.store(parent_h, Ordering::SeqCst);
    NATIVE_OVERLAY_CHILD_CLIENT_W.store(child_w, Ordering::SeqCst);
    NATIVE_OVERLAY_CHILD_CLIENT_H.store(child_h, Ordering::SeqCst);

    let client_match = parent_ok && child_ok && parent_w == child_w && parent_h == child_h;
    NATIVE_OVERLAY_CHILD_CLIENT_MATCH.store(usize::from(client_match), Ordering::SeqCst);
    if !parent_match || !client_match {
        NATIVE_OVERLAY_CHILD_GEOMETRY_MISMATCH_HITS.fetch_add(1, Ordering::SeqCst);
    }
}

fn dynamic_graphics_proc(module: windows::core::PCWSTR, proc: PCSTR) -> Option<*mut c_void> {
    let handle = unsafe {
        GetModuleHandleW(module)
            .or_else(|_| LoadLibraryW(module))
            .ok()?
    };
    unsafe { GetProcAddress(handle, proc).map(|addr| addr as *mut c_void) }
}

unsafe fn create_dxgi_factory2_dynamic<T: Interface>(
    flags: DXGI_CREATE_FACTORY_FLAGS,
) -> windows::core::Result<T> {
    type CreateDXGIFactory2Raw = unsafe extern "system" fn(
        DXGI_CREATE_FACTORY_FLAGS,
        *const windows::core::GUID,
        *mut *mut c_void,
    ) -> HRESULT;
    let Some(proc) = dynamic_graphics_proc(w!("dxgi.dll"), windows::core::s!("CreateDXGIFactory2"))
    else {
        return Err(Error::from_hresult(HRESULT(0x80004005u32 as i32)));
    };
    let raw: CreateDXGIFactory2Raw = unsafe { std::mem::transmute(proc) };
    let mut result = std::ptr::null_mut();
    unsafe { raw(flags, &T::IID, &mut result).ok()? };
    if result.is_null() {
        return Err(Error::from_hresult(HRESULT(0x80004003u32 as i32)));
    }
    Ok(unsafe { T::from_raw(result) })
}

unsafe fn d3d12_create_device_dynamic() -> windows::core::Result<ID3D12Device> {
    type D3D12CreateDeviceRaw = unsafe extern "system" fn(
        *mut c_void,
        windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL,
        *const windows::core::GUID,
        *mut *mut c_void,
    ) -> HRESULT;
    let Some(proc) = dynamic_graphics_proc(w!("d3d12.dll"), windows::core::s!("D3D12CreateDevice"))
    else {
        return Err(Error::from_hresult(HRESULT(0x80004005u32 as i32)));
    };
    let raw: D3D12CreateDeviceRaw = unsafe { std::mem::transmute(proc) };
    let mut result = std::ptr::null_mut();
    unsafe {
        raw(
            std::ptr::null_mut(),
            D3D_FEATURE_LEVEL_11_0,
            &ID3D12Device::IID,
            &mut result,
        )
        .ok()?
    };
    if result.is_null() {
        return Err(Error::from_hresult(HRESULT(0x80004003u32 as i32)));
    }
    Ok(unsafe { ID3D12Device::from_raw(result) })
}

fn bridge_clear_color() -> [f32; 4] {
    [0.015, 0.010, 0.030, 1.0]
}

fn bridge_marker_color() -> [f32; 4] {
    [0.18, 0.72, 0.93, 1.0]
}

fn bridge_marker_rect() -> RECT {
    RECT {
        left: 32,
        top: 32,
        right: 33,
        bottom: 33,
    }
}

fn bridge_marker_color_expected_rgba() -> [u8; 4] {
    [46, 184, 237, 255]
}

fn bridge_pixel_matches_expected(rgba: [u8; 4]) -> bool {
    let exp = bridge_marker_color_expected_rgba();
    rgba.iter()
        .zip(exp.iter())
        .all(|(a, b)| a.abs_diff(*b) <= 2)
}

unsafe fn create_bridge_readback(
    device: &ID3D12Device,
    backbuffer: &ID3D12Resource,
) -> Option<(ID3D12Resource, D3D12_PLACED_SUBRESOURCE_FOOTPRINT, u64)> {
    let desc = unsafe { backbuffer.GetDesc() };
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total_bytes: u64 = 0;
    unsafe {
        device.GetCopyableFootprints(
            &desc,
            0,
            1,
            0,
            Some(&mut footprint),
            None,
            None,
            Some(&mut total_bytes),
        )
    };
    if total_bytes == 0 || footprint.Footprint.RowPitch < 4 {
        return None;
    }
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_READBACK,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    };
    let buf_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: total_bytes,
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
    let mut out = None;
    if unsafe {
        device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &buf_desc,
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut out,
        )
    }
    .is_err()
    {
        return None;
    }
    Some((out?, footprint, total_bytes))
}

unsafe fn create_bridge_upload(
    device: &ID3D12Device,
    width: usize,
    height: usize,
) -> Option<ID3D12Resource> {
    if width == 0 || height == 0 {
        return None;
    }
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: width as u64,
        Height: height as u32,
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
    let mut total_bytes: u64 = 0;
    unsafe {
        device.GetCopyableFootprints(&desc, 0, 1, 0, None, None, None, Some(&mut total_bytes))
    };
    if total_bytes == 0 {
        return None;
    }
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_UPLOAD,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    };
    let buf_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: total_bytes,
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
    let mut out = None;
    if unsafe {
        device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &buf_desc,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut out,
        )
    }
    .is_err()
    {
        return None;
    }
    out
}

unsafe fn copy_bridge_content_frame(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    backbuffer: &ID3D12Resource,
    upload: &ID3D12Resource,
    backbuffer_width: usize,
    backbuffer_height: usize,
) -> bool {
    let frame = boot_view_render_frame(backbuffer_width, backbuffer_height);
    if frame.w == 0
        || frame.h == 0
        || frame.rgba.len() < frame.w.saturating_mul(frame.h).saturating_mul(4)
    {
        return false;
    }
    let region_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: frame.w as u64,
        Height: frame.h as u32,
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
    let mut total_bytes: u64 = 0;
    unsafe {
        device.GetCopyableFootprints(
            &region_desc,
            0,
            1,
            0,
            Some(&mut footprint),
            None,
            None,
            Some(&mut total_bytes),
        )
    };
    let row_pitch = footprint.Footprint.RowPitch as usize;
    let src_row = frame.w * 4;
    if total_bytes == 0 || row_pitch < src_row {
        return false;
    }
    let mut mapped: *mut c_void = std::ptr::null_mut();
    if unsafe { upload.Map(0, None, Some(&mut mapped)) }.is_err() || mapped.is_null() {
        return false;
    }
    {
        let dst =
            unsafe { std::slice::from_raw_parts_mut(mapped as *mut u8, total_bytes as usize) };
        for y in 0..frame.h {
            let so = y * src_row;
            let dofs = y * row_pitch;
            if dofs + src_row > dst.len() || so + src_row > frame.rgba.len() {
                break;
            }
            dst[dofs..dofs + src_row].copy_from_slice(&frame.rgba[so..so + src_row]);
        }
    }
    unsafe { upload.Unmap(0, None) };

    unsafe {
        overlay_transition(
            list,
            backbuffer,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )
    };
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
        list.CopyTextureRegion(
            &dst_loc,
            frame.dx as u32,
            frame.dy as u32,
            0,
            &src_loc,
            None,
        )
    };
    unsafe { ManuallyDrop::drop(&mut dst_loc.pResource) };
    unsafe { ManuallyDrop::drop(&mut src_loc.pResource) };
    unsafe {
        overlay_transition(
            list,
            backbuffer,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
        )
    };
    true
}

unsafe fn record_bridge_pixel_copy(
    list: &ID3D12GraphicsCommandList,
    backbuffer: &ID3D12Resource,
    readback: &ID3D12Resource,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
) {
    let mut dst = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(readback.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    let mut src = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(backbuffer.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    let read_box = D3D12_BOX {
        left: 32,
        top: 32,
        front: 0,
        right: 33,
        bottom: 33,
        back: 1,
    };
    unsafe { list.CopyTextureRegion(&dst, 0, 0, 0, &src, Some(&read_box)) };
    unsafe { ManuallyDrop::drop(&mut dst.pResource) };
    unsafe { ManuallyDrop::drop(&mut src.pResource) };
}

unsafe fn sample_bridge_pixel_probe(readback: &ID3D12Resource, total_bytes: u64) {
    if NATIVE_OVERLAY_PIXEL_PROBE_HITS.load(Ordering::SeqCst) != 0 {
        return;
    }
    NATIVE_OVERLAY_PIXEL_PROBE_HITS.store(1, Ordering::SeqCst);
    let range = D3D12_RANGE {
        Begin: 0,
        End: total_bytes.min(4) as usize,
    };
    let mut ptr: *mut c_void = std::ptr::null_mut();
    if unsafe { readback.Map(0, Some(&range), Some(&mut ptr)) }.is_err() || ptr.is_null() {
        return;
    }
    let b = unsafe { std::slice::from_raw_parts(ptr as *const u8, 4) };
    let rgba = [b[0], b[1], b[2], b[3]];
    let packed = ((rgba[0] as usize) << 24)
        | ((rgba[1] as usize) << 16)
        | ((rgba[2] as usize) << 8)
        | rgba[3] as usize;
    NATIVE_OVERLAY_PIXEL_PROBE_RGBA.store(packed, Ordering::SeqCst);
    if bridge_pixel_matches_expected(rgba) {
        NATIVE_OVERLAY_PIXEL_PROBE_MATCHES.store(1, Ordering::SeqCst);
    }
    let empty = D3D12_RANGE { Begin: 0, End: 0 };
    unsafe { readback.Unmap(0, Some(&empty)) };
}

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

    let hinstance = match unsafe { GetModuleHandleW(None) } {
        Ok(h) => h,
        Err(e) => {
            append_autoload_debug(format_args!(
                "native-overlay: GetModuleHandleW failed: {e:?}"
            ));
            return;
        }
    };
    let class_name = w!("ErEffectsWindowsProofBridgeOverlay");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(overlay_wndproc),
        hInstance: hinstance.into(),
        lpszClassName: class_name,
        ..Default::default()
    };
    let _atom = unsafe { RegisterClassW(&wc) };
    NATIVE_OVERLAY_STAGE.store(2, Ordering::SeqCst);

    let Some((parent_hwnd, win_w, win_h)) = wait_for_parent_game_window() else {
        NATIVE_OVERLAY_FAILURE.store(FAILURE_PARENT_WINDOW, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "native-overlay: no visible current-process parent game window found"
        ));
        return;
    };

    let hwnd = match unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE,
            class_name,
            w!("er-effects bridge cover"),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            win_w,
            win_h,
            Some(parent_hwnd),
            None,
            Some(hinstance.into()),
            None,
        )
    } {
        Ok(h) => h,
        Err(e) => {
            NATIVE_OVERLAY_FAILURE.store(FAILURE_WINDOW, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "native-overlay: CreateWindowExW failed: {e:?}"
            ));
            return;
        }
    };
    NATIVE_OVERLAY_PARENT_HWND.store(parent_hwnd.0 as usize, Ordering::SeqCst);
    NATIVE_OVERLAY_CHILD_WINDOW.store(1, Ordering::SeqCst);
    record_child_geometry_oracle(parent_hwnd, hwnd);
    NATIVE_OVERLAY_STAGE.store(3, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "native-overlay: child bridge window created parent=0x{:x} hwnd=0x{:x} {win_w}x{win_h}",
        parent_hwnd.0 as usize, hwnd.0 as usize
    ));

    let factory: IDXGIFactory4 =
        match unsafe { create_dxgi_factory2_dynamic(DXGI_CREATE_FACTORY_FLAGS(0)) } {
            Ok(f) => f,
            Err(e) => {
                NATIVE_OVERLAY_FAILURE.store(FAILURE_DYNAMIC_FACTORY, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "native-overlay: dynamic DXGI factory creation failed: {e:?}"
                ));
                return;
            }
        };
    NATIVE_OVERLAY_STAGE.store(4, Ordering::SeqCst);

    let device = match unsafe { d3d12_create_device_dynamic() } {
        Ok(d) => d,
        Err(e) => {
            NATIVE_OVERLAY_FAILURE.store(FAILURE_DYNAMIC_DEVICE, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "native-overlay: dynamic D3D12 device creation failed: {e:?}"
            ));
            return;
        }
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

    let bridge_pixel_readback = unsafe { create_bridge_readback(&device, &backbuffers[0]) };
    let bridge_upload = unsafe { create_bridge_upload(&device, win_w as usize, win_h as usize) };

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
        "native-overlay: isolated D3D12 bridge ready; entering render loop (content_upload={})",
        bridge_upload.is_some()
    ));

    let (_tick_tx, tick_rx) = std::sync::mpsc::channel::<()>();
    let hidden_poll = std::time::Duration::from_millis(16);
    let mut shown = false;
    NATIVE_OVERLAY_STAGE.store(10, Ordering::SeqCst);

    loop {
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            let _ = unsafe { TranslateMessage(&msg) };
            unsafe { DispatchMessageW(&msg) };
        }

        sync_child_to_parent_client(parent_hwnd, hwnd);
        record_child_geometry_oracle(parent_hwnd, hwnd);

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
            let _ = tick_rx.recv_timeout(hidden_poll);
            continue;
        }

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
        unsafe { list.ClearRenderTargetView(handle, &bridge_clear_color(), None) };
        if let Some(upload) = bridge_upload.as_ref()
            && unsafe {
                copy_bridge_content_frame(
                    &device,
                    &list,
                    bb,
                    upload,
                    win_w as usize,
                    win_h as usize,
                )
            }
        {
            NATIVE_OVERLAY_CONTENT_FRAMES.fetch_add(1, Ordering::SeqCst);
        }
        let marker_rect = bridge_marker_rect();
        unsafe { list.ClearRenderTargetView(handle, &bridge_marker_color(), Some(&[marker_rect])) };
        NATIVE_OVERLAY_DRAW_HITS.fetch_add(1, Ordering::SeqCst);
        if NATIVE_OVERLAY_PIXEL_PROBE_HITS.load(Ordering::SeqCst) == 0 {
            if let Some((readback, footprint, _total_bytes)) = bridge_pixel_readback.as_ref() {
                unsafe {
                    overlay_transition(
                        &list,
                        bb,
                        D3D12_RESOURCE_STATE_RENDER_TARGET,
                        D3D12_RESOURCE_STATE_COPY_SOURCE,
                    )
                };
                unsafe { record_bridge_pixel_copy(&list, bb, readback, *footprint) };
                unsafe {
                    overlay_transition(
                        &list,
                        bb,
                        D3D12_RESOURCE_STATE_COPY_SOURCE,
                        D3D12_RESOURCE_STATE_PRESENT,
                    )
                };
            } else {
                unsafe {
                    overlay_transition(
                        &list,
                        bb,
                        D3D12_RESOURCE_STATE_RENDER_TARGET,
                        D3D12_RESOURCE_STATE_PRESENT,
                    )
                };
            }
        } else {
            unsafe {
                overlay_transition(
                    &list,
                    bb,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                    D3D12_RESOURCE_STATE_PRESENT,
                )
            };
        }
        if unsafe { list.Close() }.is_err() {
            let _ = tick_rx.recv_timeout(hidden_poll);
            continue;
        }
        let list_any = list.cast().ok();
        unsafe { queue.ExecuteCommandLists(&[list_any]) };

        if unsafe { swapchain.Present(1, DXGI_PRESENT(0)) }.is_ok() {
            NATIVE_OVERLAY_FRAMES.fetch_add(1, Ordering::SeqCst);
        }

        fence_val += 1;
        let signaled = unsafe { queue.Signal(&fence, fence_val) }.is_ok();
        if signaled
            && unsafe { fence.GetCompletedValue() } < fence_val
            && unsafe { fence.SetEventOnCompletion(fence_val, fence_event) }.is_ok()
        {
            unsafe { WaitForSingleObject(fence_event, INFINITE) };
        }
        if signaled
            && NATIVE_OVERLAY_PIXEL_PROBE_HITS.load(Ordering::SeqCst) == 0
            && let Some((readback, _footprint, total_bytes)) = bridge_pixel_readback.as_ref()
        {
            unsafe { sample_bridge_pixel_probe(readback, *total_bytes) };
        }
    }
}
