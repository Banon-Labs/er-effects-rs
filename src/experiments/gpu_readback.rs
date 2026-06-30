//! In-process D3D12 readback of the live profile-portrait offscreen render target.
//!
//! P1 already drives the live character model to render into a `CSGxTexture`'s GPU child (an
//! `ID3D12Resource`, possibly behind a `CSOffscreenGxTexture` wrapper). This module copies that
//! render target's subresource 0 into a CPU-visible READBACK buffer and de-swizzles it into a
//! tightly-packed `width*height*4` RGBA8 buffer, which the now-loading forge then feeds to the game's
//! in-memory TPF factory -- so the loading screen shows the REAL rendered head instead of the
//! magenta/yellow checker placeholder.
//!
//! Safety contract (see TASK):
//! * The game's `ID3D12Resource` is wrapped WITHOUT taking ownership (`from_raw_borrowed`), so we
//!   NEVER Release it.
//! * We create our OWN command queue/allocator/list/fence; we NEVER touch the game's queue.
//! * Every fallible COM call is `?`/`ok()?`-checked and the whole body is `catch_unwind`-wrapped:
//!   this runs on the game thread and must never panic or crash; on any failure it returns `None`.

#![allow(unused_imports)]

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::atomic::Ordering;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_COMMAND_QUEUE_FLAG_NONE,
    D3D12_CPU_PAGE_PROPERTY_UNKNOWN, D3D12_FENCE_FLAG_NONE, D3D12_HEAP_FLAG_NONE,
    D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_READBACK, D3D12_MEMORY_POOL_UNKNOWN,
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_RANGE, D3D12_RESOURCE_BARRIER,
    D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_FLAG_NONE,
    D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_BUFFER,
    D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COMMON,
    D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATES,
    D3D12_RESOURCE_TRANSITION_BARRIER, D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0,
    D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
    D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ID3D12CommandAllocator, ID3D12CommandList, ID3D12CommandQueue,
    ID3D12Device, ID3D12Fence, ID3D12GraphicsCommandList, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_UNKNOWN,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleA;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{IUnknown, Interface, PCSTR};

use super::*;

/// Bytes per RGBA8 texel.
const RGBA8_BPP: usize = 4;
/// Reject absurd render-target dimensions (corrupt/unexpected desc -> bail).
const MAX_RT_DIM: u32 = 16384;
/// Bounded fence wait: a small offscreen-RT copy completes in well under this, and a finite wait
/// guarantees we never hang the game thread if the GPU stalls (timeout -> `None`, no garbage read).
const READBACK_FENCE_WAIT_MS: u32 = 2000;
/// The fence value our single command-list submission signals.
const READBACK_FENCE_TARGET: u64 = 1;

/// PE image range `[base, base+SizeOfImage)` read from the in-memory PE headers at `base`.
unsafe fn pe_image_range(base: usize) -> Option<(usize, usize)> {
    if base == 0 {
        return None;
    }
    let e_lfanew = (unsafe { safe_read_usize(base + 0x3c) }? & 0xffff_ffff) as usize;
    let size = (unsafe { safe_read_usize(base + e_lfanew + 0x50) }? & 0xffff_ffff) as usize;
    if size == 0 || size > 0x1000_0000 {
        return None; // sanity: image < 256MB
    }
    Some((base, base + size))
}

/// `[base, base+SizeOfImage)` for a loaded module by null-terminated ASCII name, or `None`.
unsafe fn module_range(name: &[u8]) -> Option<(usize, usize)> {
    let h = unsafe { GetModuleHandleA(PCSTR(name.as_ptr())) }.ok()?;
    unsafe { pe_image_range(h.0 as usize) }
}

/// `QueryInterface` `ptr` for `ID3D12Resource` and accept it only if it is a non-trivial TEXTURE2D.
/// QI both validates the COM type (no blind vtable call on a non-resource) and returns an owned ref
/// (the QI AddRef is balanced by the returned value's `Drop`).
unsafe fn try_texture2d(ptr: usize) -> Option<(ID3D12Resource, u64)> {
    let raw = ptr as *mut c_void;
    let unk = unsafe { IUnknown::from_raw_borrowed(&raw) }?;
    let res: ID3D12Resource = match unk.cast() {
        Ok(r) => r,
        Err(_) => {
            append_autoload_debug(format_args!(
                "portrait-scan: cand 0x{ptr:x} QI(ID3D12Resource) failed (d3d obj but not a resource)"
            ));
            return None;
        }
    };
    let desc = unsafe { res.GetDesc() };
    append_autoload_debug(format_args!(
        "portrait-scan: cand 0x{ptr:x} IS resource dim={} w={} h={} fmt={}",
        desc.Dimension.0, desc.Width, desc.Height, desc.Format.0
    ));
    // COLOR ONLY: the offscreen has a color render target AND a same-size depth-stencil sibling
    // (observed: 256x256 fmt=28 color next to 256x256 fmt=19 R32G8X24 depth). Accept only the 8bpp
    // RGBA/BGRA formats our de-swizzle handles; reject depth/typeless-depth so "largest" can't pick
    // the depth buffer over the head color RT.
    let is_color = matches!(
        desc.Format,
        DXGI_FORMAT_R8G8B8A8_UNORM
            | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB
            | DXGI_FORMAT_B8G8R8A8_UNORM
            | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
    );
    if is_color
        && desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D
        && desc.Width >= 8
        && desc.Width <= MAX_RT_DIM as u64
        && desc.Height >= 8
        && desc.Height <= MAX_RT_DIM
    {
        Some((res, desc.Width * desc.Height as u64))
    } else {
        None
    }
}

/// Find the VKD3D `ID3D12Resource` (a TEXTURE2D whose vtable lives in d3d12core/d3d12/dxgi) by a
/// bounded BFS over the eldenring-wrapper object nest reachable from `start` (the CSGxTexture's GPU
/// child). The real resource is several wrappers deep and at run-varying offsets, so we scan by
/// vtable-module rather than hard-code a fragile offset chain (see bd
/// `live-portrait-d3d12-resource-buried-in-gx-wrapper-nest-2026-06-29`). Returns the validated,
/// QI-owned resource. Pure read-only pointer-walking until the QI on a confirmed-d3d12 candidate.
unsafe fn find_d3d12_resource(start: usize) -> Option<ID3D12Resource> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if start == 0 || start == null {
        return None;
    }
    let er = match unsafe { pe_image_range(game_module_base().ok()?) } {
        Some(r) => r,
        None => {
            append_autoload_debug(format_args!(
                "portrait-scan: pe_image_range(eldenring) failed -- cannot bound EXE"
            ));
            return None;
        }
    };
    let d3d: Vec<(usize, usize)> = [
        b"d3d12core.dll\0".as_slice(),
        b"d3d12.dll\0".as_slice(),
        b"dxgi.dll\0".as_slice(),
    ]
    .iter()
    .filter_map(|n| unsafe { module_range(n) })
    .collect();
    append_autoload_debug(format_args!(
        "portrait-scan: start=0x{start:x} er=[0x{:x},0x{:x}) d3d_modules={}",
        er.0,
        er.1,
        d3d.len()
    ));
    if d3d.is_empty() {
        append_autoload_debug(format_args!(
            "portrait-scan: NO d3d modules resolved (GetModuleHandleA failed for d3d12core/d3d12/dxgi)"
        ));
        return None;
    }
    let in_ranges = |v: usize, r: &[(usize, usize)]| r.iter().any(|&(lo, hi)| lo <= v && v < hi);

    let mut visited: Vec<usize> = Vec::new();
    let mut queue: Vec<(usize, u32)> = vec![(start, 0)];
    let mut budget = 0u32;
    let mut d3d_hits = 0u32; // pointers whose vtable is in a d3d module
    let mut qi_fails = 0u32; // d3d candidates that failed the ID3D12Resource TEXTURE2D QI
    // Collect the LARGEST TEXTURE2D in the nest -- the offscreen RT, not the 1x1 null/dummy textures
    // vkd3d leaves bound on unused descriptor slots (observed: the gx sub-nest is all 1x1).
    let mut best: Option<(ID3D12Resource, u64, usize)> = None;
    while let Some((obj, depth)) = queue.pop() {
        if budget >= 256 {
            break;
        }
        budget += 1;
        if obj == 0 || visited.contains(&obj) {
            continue;
        }
        visited.push(obj);
        let mut off = 0usize;
        while off < 0x60 {
            if let Some(v) = unsafe { safe_read_usize(obj + off) } {
                if v > 0x10000 && v < 0x8000_0000_0000 {
                    if let Some(vt) = unsafe { safe_read_usize(v) } {
                        if in_ranges(vt, &d3d) {
                            // Confirmed d3d12-module vtable -> safe to QI for ID3D12Resource.
                            d3d_hits += 1;
                            if let Some((res, area)) = unsafe { try_texture2d(v) } {
                                if best.as_ref().is_none_or(|&(_, a, _)| area > a) {
                                    best = Some((res, area, v));
                                }
                            } else {
                                qi_fails += 1;
                            }
                        } else if depth < 6 && in_ranges(vt, std::slice::from_ref(&er)) {
                            queue.push((v, depth + 1));
                        }
                    }
                }
            }
            off += 8;
        }
    }
    if let Some((res, area, v)) = best {
        append_autoload_debug(format_args!(
            "portrait-scan: FOUND largest TEXTURE2D at 0x{v:x} area={area} objs={budget} d3d_hits={d3d_hits} qi_fails={qi_fails}"
        ));
        return Some(res);
    }
    append_autoload_debug(format_args!(
        "portrait-scan: no TEXTURE2D RT found -- objs={budget} d3d_hits={d3d_hits} qi_fails={qi_fails} (all d3d textures were 1x1 dummies or non-resources => offscreen not composited)"
    ));
    None
}

/// Record a one-subresource state transition into `list` for `res`, balancing the AddRef the
/// `ManuallyDrop<Option<ID3D12Resource>>` field requires (clone + explicit drop = net zero on `res`).
unsafe fn record_transition(
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
    // Release the clone we put into the barrier so `res` (the borrowed game resource) is untouched.
    // Explicit deref of the ManuallyDrop union field (`Transition`) is required; only `pResource`
    // owns a COM ref, so dropping it alone fully balances the clone (other fields are Copy).
    unsafe { ManuallyDrop::drop(&mut (*barrier.Anonymous.Transition).pResource) };
}

/// D3D12 readback of the offscreen render target behind `gpu_child` into tightly-packed RGBA8.
///
/// Returns `(width, height, rgba8)` on success, `None` on ANY failure. Never panics, never crashes,
/// never Releases the game's resource, never touches the game's command queue.
pub(crate) unsafe fn readback_offscreen_rgba8(gpu_child: usize) -> Option<(u32, u32, Vec<u8>)> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        readback_offscreen_rgba8_inner(gpu_child)
    }))
    .ok()
    .flatten()
}

unsafe fn readback_offscreen_rgba8_inner(gpu_child: usize) -> Option<(u32, u32, Vec<u8>)> {
    // Scan the wrapper nest for the real VKD3D ID3D12Resource (validated TEXTURE2D, QI-owned ref;
    // its Drop balances the QI AddRef, so the game's object is left net-untouched).
    let resource = unsafe { find_d3d12_resource(gpu_child) }?;

    // GetDevice AddRefs the device; that ref is ours, dropped normally at scope end.
    let mut device_opt: Option<ID3D12Device> = None;
    unsafe { resource.GetDevice(&mut device_opt) }.ok()?;
    let device = device_opt?;

    let desc: D3D12_RESOURCE_DESC = unsafe { resource.GetDesc() };
    let width = desc.Width as u32;
    let height = desc.Height;
    let format = desc.Format;
    // Record the source render-target format for telemetry (best-effort even for unhandled formats).
    LOADING_BG_PORTRAIT_FORMAT.store(format.0 as usize, Ordering::SeqCst);
    if width == 0 || height == 0 || width > MAX_RT_DIM || height > MAX_RT_DIM {
        append_autoload_debug(format_args!(
            "portrait-readback: rejected dims {width}x{height} format={}",
            format.0
        ));
        return None;
    }

    // Copyable footprint of subresource 0 (row pitch is 256-aligned, total bytes for the buffer).
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut num_rows: u32 = 0;
    let mut row_size_bytes: u64 = 0;
    let mut total_bytes: u64 = 0;
    unsafe {
        device.GetCopyableFootprints(
            &desc,
            0,
            1,
            0,
            Some(&mut footprint),
            Some(&mut num_rows),
            Some(&mut row_size_bytes),
            Some(&mut total_bytes),
        )
    };
    if total_bytes == 0 || footprint.Footprint.RowPitch == 0 {
        return None;
    }

    // READBACK buffer sized to the footprint, created on the GAME's device so CopyTextureRegion is
    // valid cross-resource.
    let heap_props = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_READBACK,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    };
    let buffer_desc = D3D12_RESOURCE_DESC {
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
    let mut readback_opt: Option<ID3D12Resource> = None;
    unsafe {
        device.CreateCommittedResource(
            &heap_props,
            D3D12_HEAP_FLAG_NONE,
            &buffer_desc,
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut readback_opt,
        )
    }
    .ok()?;
    let readback = readback_opt?;

    // OUR OWN DIRECT queue/allocator/list/fence -- never the game's.
    let queue_desc = D3D12_COMMAND_QUEUE_DESC {
        Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
        Priority: 0,
        Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
        NodeMask: 0,
    };
    let queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&queue_desc) }.ok()?;
    let allocator: ID3D12CommandAllocator =
        unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }.ok()?;
    let list: ID3D12GraphicsCommandList =
        unsafe { device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None) }
            .ok()?;

    // Barrier source COMMON -> COPY_SOURCE, copy subresource 0 into the readback footprint, barrier
    // back COPY_SOURCE -> COMMON.
    unsafe {
        record_transition(
            &list,
            &resource,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )
    };

    let mut src_location = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(resource.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    let mut dst_location = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(readback.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    unsafe { list.CopyTextureRegion(&dst_location, 0, 0, 0, &src_location, None) };
    // Release the clones we put into the copy locations (the command list holds its own refs).
    unsafe { ManuallyDrop::drop(&mut src_location.pResource) };
    unsafe { ManuallyDrop::drop(&mut dst_location.pResource) };

    unsafe {
        record_transition(
            &list,
            &resource,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_RESOURCE_STATE_COMMON,
        )
    };

    unsafe { list.Close() }.ok()?;
    let base_list: ID3D12CommandList = list.cast().ok()?;
    unsafe { queue.ExecuteCommandLists(&[Some(base_list)]) };

    // Fence + bounded wait for GPU completion.
    let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.ok()?;
    unsafe { queue.Signal(&fence, READBACK_FENCE_TARGET) }.ok()?;
    if unsafe { fence.GetCompletedValue() } < READBACK_FENCE_TARGET {
        let event: HANDLE = unsafe { CreateEventW(None, false, false, None) }.ok()?;
        unsafe { fence.SetEventOnCompletion(READBACK_FENCE_TARGET, event) }.ok()?;
        let wait = unsafe { WaitForSingleObject(event, READBACK_FENCE_WAIT_MS) };
        let _ = unsafe { CloseHandle(event) };
        if wait != WAIT_OBJECT_0 {
            append_autoload_debug(format_args!(
                "portrait-readback: fence wait did not signal (wait={:#x})",
                wait.0
            ));
            return None;
        }
    }

    // Map the readback buffer and de-swizzle each (256-aligned) row into a tightly packed RGBA8 Vec.
    let read_range = D3D12_RANGE {
        Begin: 0,
        End: total_bytes as usize,
    };
    let mut mapped: *mut c_void = std::ptr::null_mut();
    unsafe { readback.Map(0, Some(&read_range), Some(&mut mapped)) }.ok()?;
    if mapped.is_null() {
        return None;
    }

    let w = width as usize;
    let h = height as usize;
    let row_pitch = footprint.Footprint.RowPitch as usize;
    let out_row = w * RGBA8_BPP;
    let mut out = vec![0u8; w * h * RGBA8_BPP];
    let total = total_bytes as usize;
    let src = mapped as *const u8;

    let swap_rb = matches!(
        format,
        DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
    );
    for y in 0..h {
        let row_off = y * row_pitch;
        // Never read past the mapped buffer (bound by the footprint total).
        if row_off >= total {
            break;
        }
        let avail = total - row_off;
        let copy_bytes = out_row.min(row_pitch).min(avail);
        let src_row = unsafe { src.add(row_off) };
        let dst_row = &mut out[y * out_row..y * out_row + copy_bytes];
        unsafe { std::ptr::copy_nonoverlapping(src_row, dst_row.as_mut_ptr(), copy_bytes) };
        if swap_rb {
            // BGRA -> RGBA: swap byte 0 and 2 of each whole texel that landed in this row.
            let texels = copy_bytes / RGBA8_BPP;
            for t in 0..texels {
                dst_row.swap(t * RGBA8_BPP, t * RGBA8_BPP + 2);
            }
        }
    }

    // Unmap with an empty written-range (we wrote nothing back to the buffer).
    let write_range = D3D12_RANGE { Begin: 0, End: 0 };
    unsafe { readback.Unmap(0, Some(&write_range)) };

    Some((width, height, out))
}

/// True if the read-back RGBA8 image has any non-black texel (`max(R,G,B) > 24`) inside a center
/// 64x64 region. Used to set `LOADING_BG_PORTRAIT_NONBLACK` -- a quick "did we capture a real head
/// vs a blank/black offscreen" oracle.
pub(crate) fn portrait_center_nonblack(width: u32, height: u32, pixels: &[u8]) -> bool {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || pixels.len() < w * h * RGBA8_BPP {
        return false;
    }
    const REGION: usize = 64;
    let half = REGION / 2;
    let cx = w / 2;
    let cy = h / 2;
    let x0 = cx.saturating_sub(half);
    let x1 = (cx + half).min(w);
    let y0 = cy.saturating_sub(half);
    let y1 = (cy + half).min(h);
    for y in y0..y1 {
        for x in x0..x1 {
            let idx = (y * w + x) * RGBA8_BPP;
            let r = pixels[idx];
            let g = pixels[idx + 1];
            let b = pixels[idx + 2];
            if r.max(g).max(b) > 24 {
                return true;
            }
        }
    }
    false
}
