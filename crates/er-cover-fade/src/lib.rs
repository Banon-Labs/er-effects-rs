#![cfg(windows)]

//! The loading cover's one-way release fade, at the D3D12 level: the command objects it submits on,
//! the root signature / PSO / texture slot it draws with, and the single alpha-blended full-screen
//! frame it composites onto the game's backbuffer.
//!
//! Extracted from `er-quickload`'s `experiments/gpu_readback/boot_progress.rs`, which had grown past
//! the repo's hard Rust file-size limit. This half came out because it holds no product decision:
//! When to fade, what alpha, whether a hold pauses it and what the frame should look like are all
//! still decided in the DLL. What is here is the GPU work those decisions turn into, and the
//! resource-state bookkeeping that work needs to be correct.
//!
//! The two seams are deliberate and narrow:
//!   * the frame's pixels arrive through a `rasterize(w, h) -> Vec<u8>` callback, so the strip
//!     geometry, the phase label and the progress reading stay with the code that owns them;
//!   * the draw-busy latch is taken by the caller, because the same latch also guards the opaque
//!     composite path, and a lock whose two users live in different crates is a lock nobody owns.
//!
//! Everything below is a verbatim move: the counters it writes are the same statics the oracle
//! emission already reads, and the submit order is unchanged.

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use er_loading_bar_core::RGBA8_BPP;
use er_loading_portrait_core::gpu_draw_shared::{
    create_overlay_pso, create_overlay_root_signature, ensure_overlay_gpu_texture_slot,
    execute_and_wait, srv_gpu_handle_at,
};
use er_loading_portrait_core::{MAX_RT_DIM, record_transition};
// Our own persistent command objects (leaked raw pointers, same pattern as the portrait overlay --
// windows-rs COM types are !Send). Deliberately separate from the OVERLAY_* objects so the boot view
// cannot interfere with the proven portrait composite path or thrash its cached buffers at handoff.
// They live in `er-telemetry-core` because the oracle emission reads them back; this crate writes
// them. `BOOT_VIEW_RTV_HEAP` is the 1-descriptor RTV heap for the self-present full-clear (the
// engine has never rendered the backbuffer before its first own present, so un-cleared regions
// would show garbage).
use er_telemetry_core::counters::{
    BOOT_VIEW_ALLOCATOR, BOOT_VIEW_DRAW_STATE, BOOT_VIEW_FENCE, BOOT_VIEW_LIST, BOOT_VIEW_QUEUE,
    BOOT_VIEW_RTV_HEAP,
};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_COMMAND_QUEUE_FLAG_NONE,
    D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
    D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
    D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_FENCE_FLAG_NONE, D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
    D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_TEXTURE_COPY_LOCATION,
    D3D12_TEXTURE_COPY_LOCATION_0, D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
    D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, D3D12_VIEWPORT, ID3D12CommandAllocator,
    ID3D12CommandQueue, ID3D12DescriptorHeap, ID3D12Device, ID3D12Fence, ID3D12GraphicsCommandList,
    ID3D12PipelineState, ID3D12Resource, ID3D12RootSignature,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::Win32::Graphics::Dxgi::IDXGISwapChain3;
use windows::core::Interface;

/// The fade's own D3D12 objects. Separate from the opaque cover's copy path on purpose, exactly as
/// they were when they lived beside it: a fade frame is a draw (root signature + PSO + SRV), not a
/// texture copy, and giving it its own slots keeps it from thrashing the proven copy path's state.
static BOOT_VIEW_FADE_ROOT_SIGNATURE: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_PSO: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_PSO_FORMAT: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_SRV_HEAP: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_TEXTURE: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_UPLOAD: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_UPLOAD_SIZE: AtomicU64 = AtomicU64::new(0);
static BOOT_VIEW_FADE_TEX_W: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_TEX_H: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_TEX_STATE: AtomicUsize = AtomicUsize::new(0);
static BOOT_VIEW_FADE_TEX_VERSION: AtomicUsize = AtomicUsize::new(usize::MAX);

/// One-time command-object init (device derived from the backbuffer; own direct queue -- never the
/// game's). Mirrors the proven portrait-overlay init; separate objects on purpose.
///
/// # Safety
/// `backbuffer` must be a live swapchain buffer owned by the calling render thread.
pub unsafe fn ensure_cover_command_objects(backbuffer: &ID3D12Resource) -> bool {
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
            None,
        )
    }) else {
        return false;
    };
    if unsafe { list.Close() }.is_err() {
        return false;
    }
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
    let rtv_heap_desc = D3D12_DESCRIPTOR_HEAP_DESC {
        Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
        NumDescriptors: 1,
        Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
        NodeMask: 0,
    };
    let Ok(rtv_heap) =
        (unsafe { device.CreateDescriptorHeap::<ID3D12DescriptorHeap>(&rtv_heap_desc) })
    else {
        return false;
    };
    BOOT_VIEW_RTV_HEAP.store(rtv_heap.into_raw() as usize, Ordering::SeqCst);
    BOOT_VIEW_ALLOCATOR.store(allocator.into_raw() as usize, Ordering::SeqCst);
    BOOT_VIEW_LIST.store(list.into_raw() as usize, Ordering::SeqCst);
    BOOT_VIEW_FENCE.store(fence.into_raw() as usize, Ordering::SeqCst);
    BOOT_VIEW_QUEUE.store(queue.into_raw() as usize, Ordering::SeqCst);
    true
}

unsafe fn fade_init(device: &ID3D12Device, format: DXGI_FORMAT, w: u32, h: u32) -> bool {
    let fmt = format.0 as usize;
    if BOOT_VIEW_FADE_ROOT_SIGNATURE.load(Ordering::SeqCst) != 0
        && BOOT_VIEW_FADE_PSO.load(Ordering::SeqCst) != 0
        && BOOT_VIEW_FADE_SRV_HEAP.load(Ordering::SeqCst) != 0
        && BOOT_VIEW_FADE_PSO_FORMAT.load(Ordering::SeqCst) == fmt
        && BOOT_VIEW_FADE_TEX_W.load(Ordering::SeqCst) == w as usize
        && BOOT_VIEW_FADE_TEX_H.load(Ordering::SeqCst) == h as usize
    {
        return true;
    }
    let Some(root_sig) = (unsafe { create_overlay_root_signature(device) }) else {
        return false;
    };
    let Some(pso) = (unsafe { create_overlay_pso(device, &root_sig, format) }) else {
        return false;
    };
    let srv_desc = D3D12_DESCRIPTOR_HEAP_DESC {
        Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
        NumDescriptors: 1,
        Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
        NodeMask: 0,
    };
    let Ok(srv_heap) = (unsafe { device.CreateDescriptorHeap::<ID3D12DescriptorHeap>(&srv_desc) })
    else {
        return false;
    };
    if !unsafe {
        ensure_overlay_gpu_texture_slot(
            device,
            &srv_heap,
            w,
            h,
            0,
            &BOOT_VIEW_FADE_TEXTURE,
            &BOOT_VIEW_FADE_UPLOAD,
            &BOOT_VIEW_FADE_UPLOAD_SIZE,
            &BOOT_VIEW_FADE_TEX_W,
            &BOOT_VIEW_FADE_TEX_H,
            &BOOT_VIEW_FADE_TEX_STATE,
            &BOOT_VIEW_FADE_TEX_VERSION,
        )
    } {
        return false;
    }
    BOOT_VIEW_FADE_ROOT_SIGNATURE.store(root_sig.into_raw() as usize, Ordering::SeqCst);
    BOOT_VIEW_FADE_PSO.store(pso.into_raw() as usize, Ordering::SeqCst);
    BOOT_VIEW_FADE_SRV_HEAP.store(srv_heap.into_raw() as usize, Ordering::SeqCst);
    BOOT_VIEW_FADE_PSO_FORMAT.store(fmt, Ordering::SeqCst);
    true
}

unsafe fn fill_fade_upload(
    upload: &ID3D12Resource,
    alpha: u8,
    row_pitch: usize,
    w: usize,
    h: usize,
    rasterize: &mut dyn FnMut(usize, usize) -> Vec<u8>,
) -> bool {
    let total = BOOT_VIEW_FADE_UPLOAD_SIZE.load(Ordering::SeqCst) as usize;
    if total < row_pitch.saturating_mul(h) || row_pitch < w.saturating_mul(RGBA8_BPP) {
        return false;
    }
    // The host owns the frame's content: strip geometry, the phase label and the progress reading
    // are its business, and the alpha ramp below is the only thing the fade adds to them.
    let mut tight = rasterize(w, h);
    for px in tight.as_chunks_mut::<RGBA8_BPP>().0 {
        px[3] = alpha;
    }
    let mut map: *mut c_void = std::ptr::null_mut();
    if unsafe { upload.Map(0, None, Some(&mut map)) }.is_err() || map.is_null() {
        return false;
    }
    let dst = unsafe { std::slice::from_raw_parts_mut(map as *mut u8, total) };
    dst.fill(0);
    let src_row = w * RGBA8_BPP;
    for y in 0..h {
        let so = y * src_row;
        let dofs = y * row_pitch;
        if so + src_row > tight.len() || dofs + src_row > dst.len() {
            break;
        }
        dst[dofs..dofs + src_row].copy_from_slice(&tight[so..so + src_row]);
    }
    unsafe { upload.Unmap(0, None) };
    true
}

/// Composite one alpha-blended full-screen fade frame onto the swapchain's current backbuffer.
///
/// `rasterize(w, h)` supplies the tight RGBA the frame is built from; this applies `alpha` to every
/// pixel of it, uploads it and draws it. Returns whether the frame reached the backbuffer.
///
/// The caller must already hold the draw-busy latch: the same latch guards the opaque cover path,
/// which lives in the host, and the two paths share the command allocator and list.
///
/// # Safety
/// `swapchain_raw` must be a live `IDXGISwapChain3` and the caller must be on the thread that owns
/// it (the Present detour or the self-present pump), holding the draw-busy latch.
pub unsafe fn composite_release_fade_frame(
    swapchain_raw: usize,
    alpha: u8,
    rasterize: &mut dyn FnMut(usize, usize) -> Vec<u8>,
) -> bool {
    let sc_raw = swapchain_raw as *mut c_void;
    let Some(sc) = (unsafe { IDXGISwapChain3::from_raw_borrowed(&sc_raw) }) else {
        return false;
    };
    let idx = unsafe { sc.GetCurrentBackBufferIndex() };
    let Ok(backbuffer) = (unsafe { sc.GetBuffer::<ID3D12Resource>(idx) }) else {
        return false;
    };
    if BOOT_VIEW_DRAW_STATE.load(Ordering::SeqCst) == 0 {
        if unsafe { ensure_cover_command_objects(&backbuffer) } {
            BOOT_VIEW_DRAW_STATE.store(1, Ordering::SeqCst);
        } else {
            BOOT_VIEW_DRAW_STATE.store(2, Ordering::SeqCst);
            return false;
        }
    }
    if BOOT_VIEW_DRAW_STATE.load(Ordering::SeqCst) == 2 {
        return false;
    }
    let bb_desc = unsafe { backbuffer.GetDesc() };
    let cw = bb_desc.Width as u32;
    let ch = bb_desc.Height;
    if cw == 0 || ch == 0 || cw > MAX_RT_DIM || ch > MAX_RT_DIM {
        return false;
    }
    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { backbuffer.GetDevice(&mut device_opt) }.is_err() {
        return false;
    }
    let Some(device) = device_opt else {
        return false;
    };
    if !unsafe { fade_init(&device, bb_desc.Format, cw, ch) } {
        return false;
    }

    let tex_raw = BOOT_VIEW_FADE_TEXTURE.load(Ordering::SeqCst) as *mut c_void;
    let upload_raw = BOOT_VIEW_FADE_UPLOAD.load(Ordering::SeqCst) as *mut c_void;
    let root_raw = BOOT_VIEW_FADE_ROOT_SIGNATURE.load(Ordering::SeqCst) as *mut c_void;
    let pso_raw = BOOT_VIEW_FADE_PSO.load(Ordering::SeqCst) as *mut c_void;
    let srv_heap_raw = BOOT_VIEW_FADE_SRV_HEAP.load(Ordering::SeqCst) as *mut c_void;
    let alloc_raw = BOOT_VIEW_ALLOCATOR.load(Ordering::SeqCst) as *mut c_void;
    let list_raw = BOOT_VIEW_LIST.load(Ordering::SeqCst) as *mut c_void;
    let fence_raw = BOOT_VIEW_FENCE.load(Ordering::SeqCst) as *mut c_void;
    let queue_raw = BOOT_VIEW_QUEUE.load(Ordering::SeqCst) as *mut c_void;
    let rtv_heap_raw = BOOT_VIEW_RTV_HEAP.load(Ordering::SeqCst) as *mut c_void;
    let (
        Some(texture),
        Some(upload),
        Some(root_sig),
        Some(pso),
        Some(srv_heap),
        Some(allocator),
        Some(list),
        Some(fence),
        Some(queue),
        Some(rtv_heap),
    ) = (unsafe {
        (
            ID3D12Resource::from_raw_borrowed(&tex_raw),
            ID3D12Resource::from_raw_borrowed(&upload_raw),
            ID3D12RootSignature::from_raw_borrowed(&root_raw),
            ID3D12PipelineState::from_raw_borrowed(&pso_raw),
            ID3D12DescriptorHeap::from_raw_borrowed(&srv_heap_raw),
            ID3D12CommandAllocator::from_raw_borrowed(&alloc_raw),
            ID3D12GraphicsCommandList::from_raw_borrowed(&list_raw),
            ID3D12Fence::from_raw_borrowed(&fence_raw),
            ID3D12CommandQueue::from_raw_borrowed(&queue_raw),
            ID3D12DescriptorHeap::from_raw_borrowed(&rtv_heap_raw),
        )
    })
    else {
        return false;
    };

    let desc = unsafe { texture.GetDesc() };
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    unsafe { device.GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut footprint), None, None, None) };
    if !unsafe {
        fill_fade_upload(
            upload,
            alpha,
            footprint.Footprint.RowPitch as usize,
            cw as usize,
            ch as usize,
            rasterize,
        )
    } {
        return false;
    }
    let rtv_cpu = unsafe { rtv_heap.GetCPUDescriptorHandleForHeapStart() };
    unsafe { device.CreateRenderTargetView(&backbuffer, None, rtv_cpu) };
    if unsafe { allocator.Reset() }.is_err() || unsafe { list.Reset(allocator, None) }.is_err() {
        return false;
    }
    if BOOT_VIEW_FADE_TEX_STATE.load(Ordering::SeqCst) == 1 {
        unsafe {
            record_transition(
                list,
                texture,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )
        };
        BOOT_VIEW_FADE_TEX_STATE.store(0, Ordering::SeqCst);
    }
    let mut src = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(upload.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    let mut dst = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(texture.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    unsafe { list.CopyTextureRegion(&dst, 0, 0, 0, &src, None) };
    unsafe { ManuallyDrop::drop(&mut src.pResource) };
    unsafe { ManuallyDrop::drop(&mut dst.pResource) };
    unsafe {
        record_transition(
            list,
            texture,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        )
    };
    BOOT_VIEW_FADE_TEX_STATE.store(1, Ordering::SeqCst);
    unsafe {
        record_transition(
            list,
            &backbuffer,
            D3D12_RESOURCE_STATE_PRESENT,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
        )
    };
    let viewport = D3D12_VIEWPORT {
        TopLeftX: 0.0,
        TopLeftY: 0.0,
        Width: cw as f32,
        Height: ch as f32,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    };
    let scissor = RECT {
        left: 0,
        top: 0,
        right: cw as i32,
        bottom: ch as i32,
    };
    let constants = [
        1.0f32.to_bits(),
        1.0f32.to_bits(),
        0.0f32.to_bits(),
        0.0f32.to_bits(),
    ];
    unsafe {
        list.SetGraphicsRootSignature(root_sig);
        list.SetPipelineState(pso);
        list.SetDescriptorHeaps(&[Some(srv_heap.clone())]);
        list.SetGraphicsRootDescriptorTable(0, srv_gpu_handle_at(&device, srv_heap, 0));
        list.SetGraphicsRoot32BitConstants(
            1,
            constants.len() as u32,
            constants.as_ptr() as *const c_void,
            0,
        );
        list.RSSetViewports(std::slice::from_ref(&viewport));
        list.RSSetScissorRects(std::slice::from_ref(&scissor));
        list.OMSetRenderTargets(1, Some(&rtv_cpu), true, None);
        list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        list.DrawInstanced(3, 1, 0, 0);
        record_transition(
            list,
            &backbuffer,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            D3D12_RESOURCE_STATE_PRESENT,
        );
    }
    if !unsafe { execute_and_wait(queue, list, fence) } {
        return false;
    }
    // The post-stop draw detector and the fade-hit tally are the host's bookkeeping about its own
    // cover window, so they stay at the call site -- still on success only, still in this order,
    // still inside the draw-busy latch the caller holds.
    true
}
