use std::{
    ffi::c_void,
    mem::ManuallyDrop,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, WAIT_OBJECT_0},
        Graphics::{
            Direct3D::{D3D_FEATURE_LEVEL_11_0, D3D_PRIMITIVE_TOPOLOGY_UNDEFINED},
            Direct3D12::{
                D3D12_BOX, D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC,
                D3D12_COMMAND_QUEUE_FLAG_NONE, D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
                D3D12_FENCE_FLAG_NONE, D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES,
                D3D12_HEAP_TYPE_UPLOAD, D3D12_MEMORY_POOL_UNKNOWN,
                D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_RANGE, D3D12_RESOURCE_BARRIER,
                D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                D3D12_RESOURCE_BARRIER_FLAGS, D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
                D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_BUFFER,
                D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_NONE,
                D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_GENERIC_READ,
                D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_TRANSITION_BARRIER,
                D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0,
                D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                D3D12_TEXTURE_LAYOUT_UNKNOWN, D3D12CreateDevice, ID3D12CommandAllocator,
                ID3D12CommandList, ID3D12CommandQueue, ID3D12Device, ID3D12Fence,
                ID3D12GraphicsCommandList, ID3D12PipelineState, ID3D12Resource,
            },
            Dxgi::{
                Common::{
                    DXGI_ALPHA_MODE_UNSPECIFIED, DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM,
                    DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_R8G8B8A8_UNORM,
                    DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_R10G10B10A2_UNORM,
                    DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
                },
                CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT_PARAMETERS,
                DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_DISCARD,
                DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory4, IDXGISwapChain1, IDXGISwapChain3,
            },
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::{CreateEventW, WaitForSingleObject},
        },
        UI::WindowsAndMessaging::{
            CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG,
            RegisterClassW, TranslateMessage, UnregisterClassW, WINDOW_EX_STYLE, WNDCLASSW,
            WS_OVERLAPPEDWINDOW,
        },
    },
    core::{Interface, w},
};

use crate::{effects::effect_selector_text, log::net_effects_log};

const PRESENT_VTABLE_INDEX: usize = 8;
const PRESENT1_VTABLE_INDEX: usize = 22;
const MAX_RT_DIM: u64 = 16_384;
const RGBA8_BPP: usize = 4;
const READBACK_FENCE_WAIT_MS: u32 = 100;

const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;
const GLYPH_ADV: usize = 6;
const TEXT_SCALE: usize = 2;
const VIEW_PAD_X: usize = 10;
const VIEW_PAD_Y: usize = 8;
const VIEW_LINE_GAP: usize = 5;
const VIEW_MARGIN_X: u32 = 18;
const VIEW_Y: u32 = 70;
const VIEW_MIN_W: u32 = 360;
const VIEW_MAX_W: u32 = 1120;
const VIEW_BG: [u8; 3] = [5, 5, 5];
const VIEW_BORDER: [u8; 3] = [72, 70, 64];
const VIEW_TEXT: [u8; 3] = [226, 223, 214];

static PRESENT_ORIG: AtomicUsize = AtomicUsize::new(0);
static PRESENT1_ORIG: AtomicUsize = AtomicUsize::new(0);
static HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static DRAW_STATE: AtomicUsize = AtomicUsize::new(0);
static DRAW_BUSY: AtomicUsize = AtomicUsize::new(0);
static DRAW_HITS: AtomicUsize = AtomicUsize::new(0);
static DRAW_FAILS: AtomicUsize = AtomicUsize::new(0);
static VIEW_ALLOCATOR: AtomicUsize = AtomicUsize::new(0);
static VIEW_LIST: AtomicUsize = AtomicUsize::new(0);
static VIEW_FENCE: AtomicUsize = AtomicUsize::new(0);
static VIEW_QUEUE: AtomicUsize = AtomicUsize::new(0);
static VIEW_UPLOAD: AtomicUsize = AtomicUsize::new(0);
static VIEW_UPLOAD_SIZE: AtomicU64 = AtomicU64::new(0);
static VIEW_HASH: AtomicUsize = AtomicUsize::new(0);
static VIEW_W: AtomicUsize = AtomicUsize::new(0);
static VIEW_H: AtomicUsize = AtomicUsize::new(0);
static FENCE_VAL: AtomicU64 = AtomicU64::new(1);

type PresentFn = unsafe extern "system" fn(*mut c_void, u32, u32) -> i32;
type Present1Fn =
    unsafe extern "system" fn(*mut c_void, u32, u32, *const DXGI_PRESENT_PARAMETERS) -> i32;

struct DrawBusyGuard;
impl Drop for DrawBusyGuard {
    fn drop(&mut self) {
        DRAW_BUSY.store(0, Ordering::SeqCst);
    }
}

pub(crate) fn install_present_overlay_hook() {
    if HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            net_effects_log(format_args!(
                "present-overlay: MH_Initialize failed: {status:?}"
            ));
            HOOK_INSTALLED.store(0, Ordering::SeqCst);
            return;
        }
    }

    let Some((present, present1)) = (unsafe { resolve_present_addrs() }) else {
        net_effects_log(format_args!(
            "present-overlay: failed to resolve Present addresses"
        ));
        HOOK_INSTALLED.store(0, Ordering::SeqCst);
        return;
    };
    let present_hook =
        match unsafe { MhHook::new(present as *mut c_void, present_hook as *mut c_void) } {
            Ok(hook) => hook,
            Err(status) => {
                net_effects_log(format_args!(
                    "present-overlay: Present hook create failed: {status:?}"
                ));
                HOOK_INSTALLED.store(0, Ordering::SeqCst);
                return;
            }
        };
    PRESENT_ORIG.store(present_hook.trampoline() as usize, Ordering::SeqCst);
    let present1_hook =
        match unsafe { MhHook::new(present1 as *mut c_void, present1_hook as *mut c_void) } {
            Ok(hook) => hook,
            Err(status) => {
                net_effects_log(format_args!(
                    "present-overlay: Present1 hook create failed: {status:?}"
                ));
                HOOK_INSTALLED.store(0, Ordering::SeqCst);
                return;
            }
        };
    PRESENT1_ORIG.store(present1_hook.trampoline() as usize, Ordering::SeqCst);

    if unsafe { present_hook.queue_enable() }.is_err()
        || unsafe { present1_hook.queue_enable() }.is_err()
        || unsafe { MH_ApplyQueued() }.ok().is_err()
    {
        net_effects_log(format_args!("present-overlay: hook enable/apply failed"));
        HOOK_INSTALLED.store(0, Ordering::SeqCst);
        return;
    }
    std::mem::forget(present_hook);
    std::mem::forget(present1_hook);
    net_effects_log(format_args!(
        "present-overlay: installed Present=0x{present:x} Present1=0x{present1:x}"
    ));
}

unsafe extern "system" fn present_hook(this: *mut c_void, sync: u32, flags: u32) -> i32 {
    maybe_draw(this);
    let orig = PRESENT_ORIG.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let f: PresentFn = unsafe { std::mem::transmute(orig) };
    unsafe { f(this, sync, flags) }
}

unsafe extern "system" fn present1_hook(
    this: *mut c_void,
    sync: u32,
    flags: u32,
    params: *const DXGI_PRESENT_PARAMETERS,
) -> i32 {
    maybe_draw(this);
    let orig = PRESENT1_ORIG.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let f: Present1Fn = unsafe { std::mem::transmute(orig) };
    unsafe { f(this, sync, flags, params) }
}

fn maybe_draw(swapchain: *mut c_void) {
    if swapchain.is_null() || effect_selector_text().trim().is_empty() {
        return;
    }
    let ok = unsafe { composite_effect_selector_on_swapchain(swapchain as usize) };
    if !ok {
        let failures = DRAW_FAILS.fetch_add(1, Ordering::SeqCst) + 1;
        if failures == 1 || failures % 300 == 0 {
            net_effects_log(format_args!(
                "present-overlay: draw failed count={failures}"
            ));
        }
    }
}

unsafe fn resolve_present_addrs() -> Option<(usize, usize)> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let factory: IDXGIFactory4 = match CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) {
            Ok(f) => f,
            Err(e) => {
                net_effects_log(format_args!(
                    "present-overlay: CreateDXGIFactory2 failed: {e:?}"
                ));
                return None;
            }
        };
        let mut device_opt: Option<ID3D12Device> = None;
        if let Err(e) = D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut device_opt) {
            net_effects_log(format_args!(
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
                net_effects_log(format_args!(
                    "present-overlay: CreateCommandQueue failed: {e:?}"
                ));
                return None;
            }
        };
        let hinstance = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(e) => {
                net_effects_log(format_args!(
                    "present-overlay: GetModuleHandleW failed: {e:?}"
                ));
                return None;
            }
        };
        let class_name = w!("ErNetEffectsPresentDummyWnd");
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
            w!("er-net-effects-present-dummy"),
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
                net_effects_log(format_args!(
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
                net_effects_log(format_args!(
                    "present-overlay: CreateSwapChainForHwnd failed: {e:?}"
                ));
                return None;
            }
        };
        let obj = swapchain.as_raw() as *const *const usize;
        let vtable = *obj;
        let present_addr = *vtable.add(PRESENT_VTABLE_INDEX) as usize;
        let present1_addr = *vtable.add(PRESENT1_VTABLE_INDEX) as usize;
        net_effects_log(format_args!(
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

unsafe extern "system" fn dummy_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

unsafe fn composite_effect_selector_on_swapchain(swapchain_raw: usize) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        composite_effect_selector_inner(swapchain_raw)
    }))
    .unwrap_or(false)
}

unsafe fn effect_selector_view_init(backbuffer: &ID3D12Resource) -> bool {
    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { backbuffer.GetDevice(&mut device_opt) }.is_err() {
        return false;
    }
    let Some(device) = device_opt else {
        return false;
    };
    let Ok(allocator) = (unsafe {
        device.CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT)
    }) else {
        return false;
    };
    let Ok(list) = (unsafe {
        device.CreateCommandList::<_, _, ID3D12GraphicsCommandList>(
            0,
            D3D12_COMMAND_LIST_TYPE_DIRECT,
            &allocator,
            None::<&ID3D12PipelineState>,
        )
    }) else {
        return false;
    };
    let _ = unsafe { list.Close() };
    let Ok(fence) = (unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }) else {
        return false;
    };
    let queue_desc = D3D12_COMMAND_QUEUE_DESC {
        Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
        Priority: 0,
        Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
        NodeMask: 0,
    };
    let Ok(queue) = (unsafe { device.CreateCommandQueue::<ID3D12CommandQueue>(&queue_desc) })
    else {
        return false;
    };
    VIEW_ALLOCATOR.store(allocator.into_raw() as usize, Ordering::SeqCst);
    VIEW_LIST.store(list.into_raw() as usize, Ordering::SeqCst);
    VIEW_FENCE.store(fence.into_raw() as usize, Ordering::SeqCst);
    VIEW_QUEUE.store(queue.into_raw() as usize, Ordering::SeqCst);
    true
}

unsafe fn composite_effect_selector_inner(swapchain_raw: usize) -> bool {
    let text = effect_selector_text();
    if text.trim().is_empty() || DRAW_STATE.load(Ordering::SeqCst) == 2 {
        return false;
    }
    if DRAW_BUSY.swap(1, Ordering::SeqCst) != 0 {
        return false;
    }
    let _busy = DrawBusyGuard;

    let sc_raw = swapchain_raw as *mut c_void;
    let Some(sc) = (unsafe { IDXGISwapChain3::from_raw_borrowed(&sc_raw) }) else {
        return false;
    };
    let idx = unsafe { sc.GetCurrentBackBufferIndex() };
    let Ok(backbuffer) = (unsafe { sc.GetBuffer::<ID3D12Resource>(idx) }) else {
        return false;
    };
    if DRAW_STATE.load(Ordering::SeqCst) == 0 {
        if unsafe { effect_selector_view_init(&backbuffer) } {
            DRAW_STATE.store(1, Ordering::SeqCst);
            net_effects_log(format_args!("present-overlay: draw state READY"));
        } else {
            DRAW_STATE.store(2, Ordering::SeqCst);
            net_effects_log(format_args!("present-overlay: draw init FAILED"));
            return false;
        }
    }

    let bb_desc = unsafe { backbuffer.GetDesc() };
    let bw = bb_desc.Width as u32;
    let bh = bb_desc.Height;
    if bw == 0 || bh == 0 || bw as u64 > MAX_RT_DIM || u64::from(bh) > MAX_RT_DIM {
        return false;
    }
    let Some(bb_encoding) = backbuffer_encoding(bb_desc.Format) else {
        return false;
    };
    let text_lines = overlay_lines(&text);
    let text_w = text_lines
        .iter()
        .map(|line| text_width(line, TEXT_SCALE) as u32)
        .max()
        .unwrap_or(0);
    let region_w = (text_w + (VIEW_PAD_X as u32 * 2))
        .max(VIEW_MIN_W)
        .min(VIEW_MAX_W)
        .min(bw.saturating_sub(VIEW_MARGIN_X).max(1));
    let line_h = GLYPH_H * TEXT_SCALE;
    let text_block_h =
        text_lines.len() * line_h + text_lines.len().saturating_sub(1) * VIEW_LINE_GAP;
    let region_h = (text_block_h + VIEW_PAD_Y * 2) as u32;
    if region_w == 0 || region_h == 0 || VIEW_Y + region_h > bh {
        return false;
    }
    let dst_x = bw.saturating_sub(region_w + VIEW_MARGIN_X);

    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { backbuffer.GetDevice(&mut device_opt) }.is_err() {
        return false;
    }
    let Some(device) = device_opt else {
        return false;
    };
    let region_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: region_w as u64,
        Height: region_h,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: bb_desc.Format,
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
        );
    }
    if total_bytes == 0 || footprint.Footprint.RowPitch == 0 {
        return false;
    }

    let mut upload_fresh = false;
    if VIEW_UPLOAD_SIZE.load(Ordering::SeqCst) != total_bytes {
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_UPLOAD,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
            MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
            CreationNodeMask: 1,
            VisibleNodeMask: 1,
        };
        let desc = D3D12_RESOURCE_DESC {
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
        let mut up_opt: Option<ID3D12Resource> = None;
        if unsafe {
            device.CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATE_GENERIC_READ,
                None,
                &mut up_opt,
            )
        }
        .is_err()
        {
            return false;
        }
        let Some(up) = up_opt else {
            return false;
        };
        let old = VIEW_UPLOAD.swap(up.into_raw() as usize, Ordering::SeqCst);
        if old != 0 {
            drop(unsafe { ID3D12Resource::from_raw(old as *mut c_void) });
        }
        VIEW_UPLOAD_SIZE.store(total_bytes, Ordering::SeqCst);
        upload_fresh = true;
    }
    let upload_raw = VIEW_UPLOAD.load(Ordering::SeqCst) as *mut c_void;
    let Some(upload) = (unsafe { ID3D12Resource::from_raw_borrowed(&upload_raw) }) else {
        return false;
    };

    let hash = text_hash(&text);
    let geom_changed = VIEW_W.swap(region_w as usize, Ordering::SeqCst) != region_w as usize
        || VIEW_H.swap(region_h as usize, Ordering::SeqCst) != region_h as usize;
    if upload_fresh || geom_changed || VIEW_HASH.load(Ordering::SeqCst) != hash {
        let mut tight = vec![0u8; region_w as usize * region_h as usize * RGBA8_BPP];
        fill_rect(
            &mut tight,
            region_w as usize,
            region_h as usize,
            0,
            0,
            region_w as usize,
            region_h as usize,
            VIEW_BG,
        );
        fill_rect(
            &mut tight,
            region_w as usize,
            region_h as usize,
            0,
            0,
            region_w as usize,
            1,
            VIEW_BORDER,
        );
        fill_rect(
            &mut tight,
            region_w as usize,
            region_h as usize,
            0,
            region_h as usize - 1,
            region_w as usize,
            1,
            VIEW_BORDER,
        );
        for (line_index, line) in text_lines.iter().enumerate() {
            let y = VIEW_PAD_Y + line_index * (GLYPH_H * TEXT_SCALE + VIEW_LINE_GAP);
            draw_text_rgb(
                &mut tight,
                region_w as usize,
                region_h as usize,
                VIEW_PAD_X,
                y,
                line,
                VIEW_TEXT,
                TEXT_SCALE,
            );
        }
        let row_pitch = footprint.Footprint.RowPitch as usize;
        let total = total_bytes as usize;
        let mut map: *mut c_void = std::ptr::null_mut();
        if unsafe { upload.Map(0, None, Some(&mut map)) }.is_err() || map.is_null() {
            return false;
        }
        {
            let dst = unsafe { std::slice::from_raw_parts_mut(map as *mut u8, total) };
            let src_row = region_w as usize * RGBA8_BPP;
            for y in 0..region_h as usize {
                let so = y * src_row;
                let dofs = y * row_pitch;
                if dofs + src_row > total || so + src_row > tight.len() {
                    break;
                }
                let srow = &tight[so..so + src_row];
                let drow = &mut dst[dofs..dofs + src_row];
                match bb_encoding {
                    BackbufferEncoding::Straight => drow.copy_from_slice(srow),
                    BackbufferEncoding::SwapRb => {
                        drow.copy_from_slice(srow);
                        for t in 0..region_w as usize {
                            drow.swap(t * RGBA8_BPP, t * RGBA8_BPP + 2);
                        }
                    }
                    BackbufferEncoding::Pack10 => {
                        for t in 0..region_w as usize {
                            let s = t * RGBA8_BPP;
                            let packed = pack_rgba8_to_r10g10b10a2(
                                srow[s],
                                srow[s + 1],
                                srow[s + 2],
                                srow[s + 3],
                            );
                            drow[s..s + 4].copy_from_slice(&packed.to_le_bytes());
                        }
                    }
                }
            }
        }
        unsafe { upload.Unmap(0, None) };
        VIEW_HASH.store(hash, Ordering::SeqCst);
    }

    let alloc_raw = VIEW_ALLOCATOR.load(Ordering::SeqCst) as *mut c_void;
    let list_raw = VIEW_LIST.load(Ordering::SeqCst) as *mut c_void;
    let fence_raw = VIEW_FENCE.load(Ordering::SeqCst) as *mut c_void;
    let queue_raw = VIEW_QUEUE.load(Ordering::SeqCst) as *mut c_void;
    let (Some(allocator), Some(list), Some(fence), Some(queue)) = (unsafe {
        (
            ID3D12CommandAllocator::from_raw_borrowed(&alloc_raw),
            ID3D12GraphicsCommandList::from_raw_borrowed(&list_raw),
            ID3D12Fence::from_raw_borrowed(&fence_raw),
            ID3D12CommandQueue::from_raw_borrowed(&queue_raw),
        )
    }) else {
        return false;
    };
    if unsafe { allocator.Reset() }.is_err() || unsafe { list.Reset(allocator, None) }.is_err() {
        return false;
    }
    unsafe {
        record_transition(
            list,
            &backbuffer,
            D3D12_RESOURCE_STATE_PRESENT,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )
    };
    let mut up_src = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(upload.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    let mut bb_dst = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(backbuffer.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    let up_box = D3D12_BOX {
        left: 0,
        top: 0,
        front: 0,
        right: region_w,
        bottom: region_h,
        back: 1,
    };
    unsafe { list.CopyTextureRegion(&bb_dst, dst_x, VIEW_Y, 0, &up_src, Some(&up_box)) };
    unsafe { ManuallyDrop::drop(&mut up_src.pResource) };
    unsafe { ManuallyDrop::drop(&mut bb_dst.pResource) };
    unsafe {
        record_transition(
            list,
            &backbuffer,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_PRESENT,
        )
    };
    if !unsafe { execute_and_wait(queue, list, fence) } {
        return false;
    }
    let hits = DRAW_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if hits == 1 {
        net_effects_log(format_args!(
            "present-overlay: first draw {region_w}x{region_h} at {dst_x},{VIEW_Y} text='{text}'"
        ));
    }
    true
}

unsafe fn record_transition(
    list: &ID3D12GraphicsCommandList,
    res: &ID3D12Resource,
    before: windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATES,
    after: windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATES,
) {
    let mut barrier = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAGS(0),
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: ManuallyDrop::new(Some(res.clone())),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    };
    unsafe { list.ResourceBarrier(std::slice::from_ref(&barrier)) };
    let transition = unsafe { &mut *barrier.Anonymous.Transition };
    unsafe { ManuallyDrop::drop(&mut transition.pResource) };
}

unsafe fn execute_and_wait(
    queue: &ID3D12CommandQueue,
    list: &ID3D12GraphicsCommandList,
    fence: &ID3D12Fence,
) -> bool {
    if unsafe { list.Close() }.is_err() {
        return false;
    }
    let Ok(base_list) = list.cast::<ID3D12CommandList>() else {
        return false;
    };
    unsafe { queue.ExecuteCommandLists(&[Some(base_list)]) };
    let val = FENCE_VAL.fetch_add(1, Ordering::SeqCst) + 1;
    if unsafe { queue.Signal(fence, val) }.is_err() {
        return false;
    }
    if unsafe { fence.GetCompletedValue() } >= val {
        return true;
    }
    let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else {
        return false;
    };
    let ok = unsafe { fence.SetEventOnCompletion(val, event) }.is_ok()
        && unsafe { WaitForSingleObject(event, READBACK_FENCE_WAIT_MS) } == WAIT_OBJECT_0;
    let _ = unsafe { CloseHandle(event) };
    ok
}

#[derive(Clone, Copy)]
enum BackbufferEncoding {
    Straight,
    SwapRb,
    Pack10,
}

fn backbuffer_encoding(format: DXGI_FORMAT) -> Option<BackbufferEncoding> {
    match format {
        DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => {
            Some(BackbufferEncoding::SwapRb)
        }
        DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => {
            Some(BackbufferEncoding::Straight)
        }
        DXGI_FORMAT_R10G10B10A2_UNORM => Some(BackbufferEncoding::Pack10),
        _ => None,
    }
}

fn pack_rgba8_to_r10g10b10a2(r: u8, g: u8, b: u8, a: u8) -> u32 {
    let r10 = ((r as u32 * 1023 + 127) / 255) & 0x3ff;
    let g10 = ((g as u32 * 1023 + 127) / 255) & 0x3ff;
    let b10 = ((b as u32 * 1023 + 127) / 255) & 0x3ff;
    let a2 = ((a as u32 * 3 + 127) / 255) & 0x3;
    r10 | (g10 << 10) | (b10 << 20) | (a2 << 30)
}

fn overlay_lines(text: &str) -> Vec<String> {
    let parts = text.split(" | ").collect::<Vec<_>>();
    if parts.len() >= 4 {
        let mut lines = vec![
            parts[0].to_owned(),
            format!("{} {} {}", parts[1], parts[2], parts[3]),
        ];
        if parts.len() > 4 {
            lines.push(parts[4..].join(" "));
        }
        lines
    } else {
        vec![text.to_owned()]
    }
}

fn text_hash(text: &str) -> usize {
    let mut hash = 0xcbf29ce484222325usize;
    for byte in text.as_bytes() {
        hash ^= *byte as usize;
        hash = hash.wrapping_mul(0x100000001b3usize);
    }
    hash
}

fn text_width(text: &str, scale: usize) -> usize {
    text.chars().count() * GLYPH_ADV * scale
}

fn draw_text_rgb(
    buf: &mut [u8],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    text: &str,
    rgb: [u8; 3],
    scale: usize,
) {
    let mut cx = x;
    for c in text.chars() {
        let rows = glyph_5x7(c);
        for (gy, row) in rows.iter().enumerate() {
            for gx in 0..GLYPH_W {
                if row & (1 << (GLYPH_W - 1 - gx)) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = cx + gx * scale + sx;
                        let py = y + gy * scale + sy;
                        if px < w && py < h {
                            let o = (py * w + px) * RGBA8_BPP;
                            buf[o] = rgb[0];
                            buf[o + 1] = rgb[1];
                            buf[o + 2] = rgb[2];
                            buf[o + 3] = 255;
                        }
                    }
                }
            }
        }
        cx += GLYPH_ADV * scale;
    }
}

fn fill_rect(
    buf: &mut [u8],
    w: usize,
    h: usize,
    x0: usize,
    y0: usize,
    rw: usize,
    rh: usize,
    rgb: [u8; 3],
) {
    for y in y0..(y0 + rh).min(h) {
        for x in x0..(x0 + rw).min(w) {
            let o = (y * w + x) * RGBA8_BPP;
            buf[o] = rgb[0];
            buf[o + 1] = rgb[1];
            buf[o + 2] = rgb[2];
            buf[o + 3] = 255;
        }
    }
}

fn glyph_5x7(c: char) -> [u8; 7] {
    match c {
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e],
        'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        'G' => [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0e],
        'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'I' => [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        'J' => [0x01, 0x01, 0x01, 0x01, 0x11, 0x11, 0x0e],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d],
        'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1b, 0x11],
        'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f],
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1f],
        '3' => [0x0e, 0x11, 0x01, 0x06, 0x01, 0x11, 0x0e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        '5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        '6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c],
        '-' => [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1f],
        '/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
        ':' => [0x00, 0x0c, 0x0c, 0x00, 0x0c, 0x0c, 0x00],
        '[' => [0x0e, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0e],
        ']' => [0x0e, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0e],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '>' => [0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08],
        '?' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
        '!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],
        '%' => [0x19, 0x19, 0x02, 0x04, 0x08, 0x13, 0x13],
        ' ' => [0; 7],
        _ => [0; 7],
    }
}
