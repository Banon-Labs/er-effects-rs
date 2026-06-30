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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_BOX, D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC,
    D3D12_COMMAND_QUEUE_FLAG_NONE, D3D12_CPU_PAGE_PROPERTY_UNKNOWN, D3D12_FENCE_FLAG_NONE,
    D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_READBACK,
    D3D12_HEAP_TYPE_UPLOAD, D3D12_MEMORY_POOL_UNKNOWN, D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    D3D12_RANGE, D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_0,
    D3D12_RESOURCE_BARRIER_FLAG_NONE, D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_DIMENSION_BUFFER, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_NONE,
    D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE,
    D3D12_RESOURCE_STATE_GENERIC_READ, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATES,
    D3D12_RESOURCE_TRANSITION_BARRIER, D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0,
    D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
    D3D12_TEXTURE_LAYOUT_ROW_MAJOR, D3D12_TEXTURE_LAYOUT_UNKNOWN, ID3D12CommandAllocator,
    ID3D12CommandList, ID3D12CommandQueue, ID3D12Device, ID3D12Fence, ID3D12GraphicsCommandList,
    ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_UNKNOWN,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain3;
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

/// GX swapchain command-queue global: deobf RVA of the qword holding the game's `ID3D12CommandQueue` (the
/// `pDevice` arg the GX backend passes to `IDXGIFactory::CreateSwapChain` -- for a D3D12 swapchain that arg
/// IS the command queue). Dump `0x1448012a8` (= `&DAT_1448012a0 + 8`, resolved from the swapchain creator
/// `FUN_141e9cc70` via `FUN_141e888c0`). RETAINED FOR REFERENCE ONLY: submitting our composite on the
/// game's queue from the Present hook caused a vkd3d access violation, so we now use our own private queue.
#[allow(dead_code)]
const GX_COMMAND_QUEUE_RVA: usize = 0x8012a8;

// Persistent portrait-overlay draw state. The COM objects are leaked (`into_raw`) for the process lifetime
// and re-borrowed (`from_raw_borrowed`) each Present -- storing raw `usize` keeps them `Send` across the
// `static` boundary (windows-rs COM types are `!Send`). State machine: 0=uninit, 1=ready, 2=failed/give-up.
static OVERLAY_DRAW_STATE: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_PORTRAIT_VERSION: AtomicUsize = AtomicUsize::new(usize::MAX); // last LOADING_BG_PORTRAIT_RGBA_VERSION uploaded into OVERLAY_PORTRAIT_TEX
static OVERLAY_PORTRAIT_TEX: AtomicUsize = AtomicUsize::new(0); // ID3D12Resource (DEFAULT heap, COPY_SOURCE)
static OVERLAY_ALLOCATOR: AtomicUsize = AtomicUsize::new(0); // ID3D12CommandAllocator (DIRECT)
static OVERLAY_LIST: AtomicUsize = AtomicUsize::new(0); // ID3D12GraphicsCommandList (DIRECT, kept closed)
static OVERLAY_FENCE: AtomicUsize = AtomicUsize::new(0); // ID3D12Fence
static OVERLAY_QUEUE: AtomicUsize = AtomicUsize::new(0); // our OWN private DIRECT ID3D12CommandQueue (leaked)
static OVERLAY_FENCE_VAL: AtomicU64 = AtomicU64::new(0); // monotonically incremented per submit
static OVERLAY_PORTRAIT_W: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_PORTRAIT_H: AtomicUsize = AtomicUsize::new(0);
/// Successful backbuffer composites submitted (RAM semaphore that the portrait is actually being drawn).
pub(crate) static OVERLAY_DRAW_HITS: AtomicUsize = AtomicUsize::new(0);
/// Latches once the in-world NowLoading streaming screen has been seen, so we can detect world-ready
/// (NowLoading seen, then both loading signals down) and stop compositing once the player is in the world.
static OVERLAY_NOW_LOADING_SEEN: AtomicUsize = AtomicUsize::new(0);

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
pub(crate) unsafe fn module_range(name: &[u8]) -> Option<(usize, usize)> {
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
    unsafe { find_d3d12_resource_ex(start, 0) }.map(|(r, _)| r)
}

/// Like `find_d3d12_resource` but (a) returns the candidate object pointer alongside the resource, and
/// (b) skips any candidate whose pointer == `exclude_v`. Lets the RT->SRV copy pick the SRV from its own
/// single-texture nest, then the LARGEST OTHER texture in the offscreen nest as the content source --
/// deterministic where plain "largest texture" is ambiguous between two same-size textures.
unsafe fn find_d3d12_resource_ex(
    start: usize,
    exclude_v: usize,
) -> Option<(ID3D12Resource, usize)> {
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
                            if v == exclude_v {
                                // Caller-excluded texture (e.g. the SRV) -- skip so we pick a different one.
                            } else if let Some((res, area)) = unsafe { try_texture2d(v) } {
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
        return Some((res, v));
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

/// Readback the largest TEXTURE2D in `start`'s nest EXCLUDING whichever texture is found from
/// `exclude_start` (e.g. read the content RT while excluding the SRV). For visual diagnosis of which
/// texture holds the portrait when several same-/different-size textures share the offscreen nest.
pub(crate) unsafe fn readback_excluding_rgba8(
    start: usize,
    exclude_start: usize,
) -> Option<(u32, u32, Vec<u8>)> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let exclude_v = find_d3d12_resource_ex(exclude_start, 0)
            .map(|(_, v)| v)
            .unwrap_or(0);
        let (resource, _) = find_d3d12_resource_ex(start, exclude_v)?;
        readback_resource_rgba8_inner(resource)
    }))
    .ok()
    .flatten()
}

/// Cached AddRef'd content-RT `ID3D12Resource` raw pointer (0 = not yet resolved). Set once by the first
/// `readback_cached_content_rgba8` scan; re-copied every frame after without re-scanning.
pub(crate) static PROFILE_LIVE_RT_RES: AtomicUsize = AtomicUsize::new(0);
/// Counter so the deterministic-resolve diagnostic logs only the first few attempts.
static PROFILE_DET_RESOLVE_DIAG: AtomicUsize = AtomicUsize::new(0);

/// DETERMINISTICALLY resolve the content RT's vkd3d `ID3D12Resource` from a CSGxTexture by following the
/// FIXED wrapper chain (bd live-portrait-d3d12-resource-buried-in-gx-wrapper-nest, RE'd from a live dump),
/// validating each hop's vtable so a layout change fails closed instead of dereferencing garbage. NO
/// memory scan / QI of arbitrary objects -> nothing to race the teardown free. Returns an AddRef'd ref.
unsafe fn resolve_content_resource_deterministic(srv_gx: usize) -> Option<ID3D12Resource> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null && p > 0x10000;
    let base = game_module_base().ok()?;
    if !valid(srv_gx) {
        return None;
    }
    let d3d: Vec<(usize, usize)> = [
        b"d3d12core.dll\0".as_slice(),
        b"d3d12.dll\0".as_slice(),
        b"dxgi.dll\0".as_slice(),
    ]
    .iter()
    .filter_map(|n| unsafe { module_range(n) })
    .collect();
    let in_d3d = |vt: usize| d3d.iter().any(|&(lo, hi)| lo <= vt && vt < hi);
    // RVA of a vtable (vt - base) for logging; usize::MAX if not in the game image.
    let rva = |vt: usize| if vt >= base { vt - base } else { usize::MAX };
    // DIAGNOSTIC (first few attempts, even on failure): dump the actual chain hops + vtable RVAs so the
    // exact offsets/vtables for the BUILD-OWN renderer (which differs from the menu ProfileSelect one the
    // chain was first RE'd from) can be read off the log and the constants corrected.
    let read = |p: usize| unsafe { safe_read_usize(p) }.unwrap_or(0);
    // DIAGNOSTIC: dump the CSOffscreenGxTexture (srv_gx+0x10) field layout -- each qword 0x08..0x60 with
    // its pointee's vtable RVA + whether that pointee is a d3d12 object -- so the path to the real
    // ID3D12Resource for the BUILD-OWN renderer can be read off the log. Only when h1 is the expected
    // CSOffscreenGxTexture, throttled to the first ~4 dumps.
    let h1d = read(srv_gx + 0x10);
    if h1d != 0
        && read(h1d) == base + PROFILE_GX_GPU_WRAPPER_VTABLE_RVA
        && PROFILE_DET_RESOLVE_DIAG.fetch_add(1, Ordering::SeqCst) < 4
    {
        for off in (0x08..=0x60usize).step_by(0x08) {
            let p = read(h1d + off);
            let vt = if p != 0 { read(p) } else { 0 };
            append_autoload_debug(format_args!(
                "det-resolve-dump: h1=0x{h1d:x} +0x{off:02x}=0x{p:x} vt_rva=0x{:x} in_d3d={}",
                rva(vt),
                in_d3d(vt)
            ));
        }
    }
    // hop 1: srv_gx +0x10 -> CSOffscreenGxTexture (validate vtable)
    let h1 = unsafe { safe_read_usize(srv_gx + GX_TEXTURE_GPU_RESOURCE_OFFSET) }?;
    if !valid(h1) || unsafe { safe_read_usize(h1) }? != base + PROFILE_GX_GPU_WRAPPER_VTABLE_RVA {
        return None;
    }
    // hop 2: +0x18 -> holder A
    let h2 = unsafe { safe_read_usize(h1 + GX_RES_CHAIN_HOLDER_A_OFFSET) }?;
    if !valid(h2) || unsafe { safe_read_usize(h2) }? != base + GX_RES_CHAIN_HOLDER_A_VTABLE_RVA {
        return None;
    }
    // hop 3: +0x40 -> holder B
    let h3 = unsafe { safe_read_usize(h2 + GX_RES_CHAIN_HOLDER_B_OFFSET) }?;
    if !valid(h3) || unsafe { safe_read_usize(h3) }? != base + GX_RES_CHAIN_HOLDER_B_VTABLE_RVA {
        return None;
    }
    // hop 4: +0x20 -> the ID3D12Resource; its vtable must land in a d3d12 module.
    let res_ptr = unsafe { safe_read_usize(h3 + GX_RES_CHAIN_RESOURCE_OFFSET) }?;
    if !valid(res_ptr) {
        return None;
    }
    let vt = unsafe { safe_read_usize(res_ptr) }?;
    if !in_d3d(vt) {
        return None;
    }
    let raw = res_ptr as *mut c_void;
    let res = unsafe { ID3D12Resource::from_raw_borrowed(&raw) }?;
    Some(res.clone()) // AddRef -- caller owns this ref
}

/// LIVE-TRACKING readback: resolve the built renderer's content RT ONCE via the DETERMINISTIC GX wrapper
/// chain (no scan), cache it AddRef'd, then re-copy the cached resource every frame. The crash that killed
/// per-frame tracking was the old `find_d3d12_resource_ex` SCAN QI'ing the D3D12 object list every readback
/// -- during the menu->world teardown it QIs a freed object -> uncatchable AV. With the deterministic chain
/// there is no scan, and the cached resource is our built renderer's RT (our lifetime), so per-frame
/// re-copy is safe through the whole loading screen -> the head tracks the look-at without crashing.
/// `srv_gx` is the renderer offscreen's CSGxTexture; `start` is the offscreen nest (`renderer+0xa8`)
/// that `readback_offscreen_rgba8` reads the real head from.
pub(crate) unsafe fn readback_cached_content_rgba8(
    start: usize,
    srv_gx: usize,
) -> Option<(u32, u32, Vec<u8>)> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let cached = PROFILE_LIVE_RT_RES.load(Ordering::SeqCst);
        if cached != 0 {
            // Re-copy the cached resource -- NO scan, NO chain walk. Borrow then clone (AddRef) to pass by
            // value into the readback (which drops that clone normally).
            let raw = cached as *mut c_void;
            let res = ID3D12Resource::from_raw_borrowed(&raw)?;
            return readback_resource_rgba8_inner(res.clone());
        }
        // First resolve: deterministic chain (no scan); else a ONE-TIME scan of the OFFSCREEN nest, then
        // cache an AddRef'd ref for all future frames. The build-own (post-Continue) renderer's GX wrapper
        // layout differs from the menu renderer the chain was RE'd from -- the det-resolve-dump shows
        // h1+0x18 is null there, so hop 2 fails closed; AND scanning `srv_gx` itself finds nothing. The
        // proven head path is `find_d3d12_resource(start)` over the OFFSCREEN nest (renderer+0xa8) -- the
        // exact resolve `readback_offscreen_rgba8(off)` uses to read back the real head (dumped as slot
        // 100). The teardown-race AV was a PER-FRAME scan; a one-time scan while the renderer is alive
        // mid-loading, cached here, never re-scans -> no race.
        let (resource, how) = match resolve_content_resource_deterministic(srv_gx) {
            Some(r) => (r, "deterministic chain"),
            None => (find_d3d12_resource(start)?, "one-time offscreen nest scan"),
        };
        PROFILE_LIVE_RT_RES.store(resource.clone().into_raw() as usize, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "live-feed: content RT resolved from start=0x{start:x} srv_gx=0x{srv_gx:x} via {how} -> cached resource -- per-frame tracking now safe"
        ));
        readback_resource_rgba8_inner(resource)
    }))
    .ok()
    .flatten()
}

unsafe fn readback_offscreen_rgba8_inner(gpu_child: usize) -> Option<(u32, u32, Vec<u8>)> {
    // Scan the wrapper nest for the real VKD3D ID3D12Resource (validated TEXTURE2D, QI-owned ref;
    // its Drop balances the QI AddRef, so the game's object is left net-untouched).
    let resource = unsafe { find_d3d12_resource(gpu_child) }?;
    unsafe { readback_resource_rgba8_inner(resource) }
}

unsafe fn readback_resource_rgba8_inner(resource: ID3D12Resource) -> Option<(u32, u32, Vec<u8>)> {
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

/// Force the offscreen RT->SRV resolve in D3D12: CopyResource the render-target texture behind
/// `src_gpu_child` (the renderer's offscreen RT, which holds the rendered head) into the sampleable SRV
/// texture behind `dst_gpu_child` (offscreen+0x10's CSGxTexture, what the loading-screen forge binds and
/// GFx samples). The engine's own per-frame resolve almost never fires post-Continue (RT has content, SRV
/// stays black), so we do the copy ourselves every render-thread frame. Returns true on a completed copy
/// (or when src==dst so no copy is needed). Same safety contract as the readback: our OWN
/// queue/allocator/list/fence, game resources borrowed (never Released), never panics/crashes.
pub(crate) unsafe fn copy_offscreen_rt_to_srv(src_gpu_child: usize, dst_gpu_child: usize) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        copy_offscreen_rt_to_srv_inner(src_gpu_child, dst_gpu_child)
    }))
    .unwrap_or(false)
}

unsafe fn copy_offscreen_rt_to_srv_inner(src_gpu_child: usize, dst_gpu_child: usize) -> bool {
    // Resolve the SRV (dst) first from its OWN single-texture nest -> deterministic, plus its candidate
    // pointer. Then resolve the source as the largest texture in the offscreen nest EXCLUDING that SRV,
    // so we never pick the (black) SRV as the source and self-skip.
    let Some((dst, dst_v)) = (unsafe { find_d3d12_resource_ex(dst_gpu_child, 0) }) else {
        return false;
    };
    let Some((src, _src_v)) = (unsafe { find_d3d12_resource_ex(src_gpu_child, dst_v) }) else {
        return false;
    };
    // CopyResource requires identical dimensions + format.
    let sd: D3D12_RESOURCE_DESC = unsafe { src.GetDesc() };
    let dd: D3D12_RESOURCE_DESC = unsafe { dst.GetDesc() };
    // One-shot diagnostic: are src (RT) and dst (SRV) distinct resources, and do their descs match? If
    // find_d3d12_resource returns the SAME resource for both starts (RT==SRV by BFS), the copy is a
    // self-skip and can never populate the SRV -- the signature of the BFS "largest texture" ambiguity.
    if PROFILE_RT_SRV_COPY_DIAGGED.fetch_add(1, Ordering::SeqCst) < 6 {
        append_autoload_debug(format_args!(
            "rt-srv-copy-diag: src=0x{:x} dst=0x{:x} same={} src_dims={}x{} fmt={} dst_dims={}x{} fmt={}",
            src.as_raw() as usize,
            dst.as_raw() as usize,
            (src.as_raw() == dst.as_raw()) as u8,
            sd.Width,
            sd.Height,
            sd.Format.0,
            dd.Width,
            dd.Height,
            dd.Format.0,
        ));
    }
    // Same physical resource (already resolved / single texture) -> nothing to copy.
    if src.as_raw() == dst.as_raw() {
        return true;
    }
    if sd.Width != dd.Width || sd.Height != dd.Height || sd.Format != dd.Format {
        return false;
    }
    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { src.GetDevice(&mut device_opt) }.is_err() {
        return false;
    }
    let Some(device) = device_opt else {
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
    // src (RT): COMMON -> COPY_SOURCE; dst (SRV): COMMON -> COPY_DEST; copy; both back to COMMON.
    unsafe {
        record_transition(
            &list,
            &src,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )
    };
    unsafe {
        record_transition(
            &list,
            &dst,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )
    };
    unsafe { list.CopyResource(&dst, &src) };
    unsafe {
        record_transition(
            &list,
            &src,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_RESOURCE_STATE_COMMON,
        )
    };
    unsafe {
        record_transition(
            &list,
            &dst,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_COMMON,
        )
    };
    if unsafe { list.Close() }.is_err() {
        return false;
    }
    let Ok(base_list) = list.cast::<ID3D12CommandList>() else {
        return false;
    };
    unsafe { queue.ExecuteCommandLists(&[Some(base_list)]) };
    let Ok(fence) = (unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }) else {
        return false;
    };
    if unsafe { queue.Signal(&fence, READBACK_FENCE_TARGET) }.is_err() {
        return false;
    }
    if unsafe { fence.GetCompletedValue() } < READBACK_FENCE_TARGET {
        let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else {
            return false;
        };
        if unsafe { fence.SetEventOnCompletion(READBACK_FENCE_TARGET, event) }.is_err() {
            let _ = unsafe { CloseHandle(event) };
            return false;
        }
        let wait = unsafe { WaitForSingleObject(event, READBACK_FENCE_WAIT_MS) };
        let _ = unsafe { CloseHandle(event) };
        if wait != WAIT_OBJECT_0 {
            return false;
        }
    }
    true
}

/// Upload tightly-packed RGBA8 `pixels` (`w`x`h`) into the TEXTURE2D found in `dst_gpu_child`'s nest --
/// overwriting the displayed now-loading background texture's pixels in place, so the Scaleform sprite
/// (which already registered that texture by name on the first bind) composites the real portrait without
/// any re-registration. Dims/format must match the destination (R8G8B8A8_UNORM, same w/h). Our own
/// upload heap + queue/list/fence; the game's resource is borrowed (never Released). Never panics.
pub(crate) unsafe fn upload_rgba_to_texture(
    dst_gpu_child: usize,
    w: u32,
    h: u32,
    pixels: &[u8],
) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        upload_rgba_to_texture_inner(dst_gpu_child, w, h, pixels)
    }))
    .unwrap_or(false)
}

unsafe fn upload_rgba_to_texture_inner(
    dst_gpu_child: usize,
    w: u32,
    h: u32,
    pixels: &[u8],
) -> bool {
    if pixels.len() < (w as usize) * (h as usize) * RGBA8_BPP {
        return false;
    }
    let Some(dst) = (unsafe { find_d3d12_resource(dst_gpu_child) }) else {
        return false;
    };
    let desc: D3D12_RESOURCE_DESC = unsafe { dst.GetDesc() };
    if desc.Width as u32 != w || desc.Height != h {
        return false; // dim mismatch -- caller must match (e.g. 1024x1024 checker <- 1024 portrait)
    }
    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { dst.GetDevice(&mut device_opt) }.is_err() {
        return false;
    }
    let Some(device) = device_opt else {
        return false;
    };
    // Copyable footprint of subresource 0 (256-aligned row pitch).
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
        return false;
    }
    // UPLOAD-heap buffer sized to the footprint; fill it with the RGBA rows at the 256-aligned pitch.
    let heap_props = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_UPLOAD,
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
    let mut upload_opt: Option<ID3D12Resource> = None;
    if unsafe {
        device.CreateCommittedResource(
            &heap_props,
            D3D12_HEAP_FLAG_NONE,
            &buffer_desc,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut upload_opt,
        )
    }
    .is_err()
    {
        return false;
    }
    let Some(upload) = upload_opt else {
        return false;
    };
    let row_pitch = footprint.Footprint.RowPitch as usize;
    let src_row = (w as usize) * RGBA8_BPP;
    let mut mapped: *mut c_void = std::ptr::null_mut();
    if unsafe { upload.Map(0, None, Some(&mut mapped)) }.is_err() || mapped.is_null() {
        return false;
    }
    let dstp = mapped as *mut u8;
    for y in 0..h as usize {
        let so = y * src_row;
        let d_o = y * row_pitch;
        if so + src_row > pixels.len() || (d_o + src_row) as u64 > total_bytes {
            break;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(pixels.as_ptr().add(so), dstp.add(d_o), src_row);
        }
    }
    unsafe { upload.Unmap(0, None) };
    // Record list: dst COMMON -> COPY_DEST, CopyTextureRegion(upload -> dst sub 0), dst back to COMMON.
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
    unsafe {
        record_transition(
            &list,
            &dst,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )
    };
    let mut src_loc = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(upload.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    let mut dst_loc = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(dst.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    unsafe { list.CopyTextureRegion(&dst_loc, 0, 0, 0, &src_loc, None) };
    unsafe { ManuallyDrop::drop(&mut src_loc.pResource) };
    unsafe { ManuallyDrop::drop(&mut dst_loc.pResource) };
    unsafe {
        record_transition(
            &list,
            &dst,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_COMMON,
        )
    };
    if unsafe { list.Close() }.is_err() {
        return false;
    }
    let Ok(base_list) = list.cast::<ID3D12CommandList>() else {
        return false;
    };
    unsafe { queue.ExecuteCommandLists(&[Some(base_list)]) };
    let Ok(fence) = (unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }) else {
        return false;
    };
    if unsafe { queue.Signal(&fence, READBACK_FENCE_TARGET) }.is_err() {
        return false;
    }
    if unsafe { fence.GetCompletedValue() } < READBACK_FENCE_TARGET {
        let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else {
            return false;
        };
        if unsafe { fence.SetEventOnCompletion(READBACK_FENCE_TARGET, event) }.is_err() {
            let _ = unsafe { CloseHandle(event) };
            return false;
        }
        let wait = unsafe { WaitForSingleObject(event, READBACK_FENCE_WAIT_MS) };
        let _ = unsafe { CloseHandle(event) };
        if wait != WAIT_OBJECT_0 {
            return false;
        }
    }
    true
}

/// Create a persistent DEFAULT-heap R8G8B8A8 TEXTURE2D, upload `pixels` into it via a one-shot private
/// queue, and leave it in `COPY_SOURCE` so the per-frame composite can use it as a `CopyTextureRegion`
/// source. Returns the texture (the temp queue/allocator/list/upload-buffer are released here). `None` on
/// any failure -- never panics.
unsafe fn create_portrait_source_texture(
    device: &ID3D12Device,
    pw: u32,
    ph: u32,
    pixels: &[u8],
) -> Option<ID3D12Resource> {
    let tex_heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    };
    let tex_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: pw as u64,
        Height: ph,
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
    let mut tex_opt: Option<ID3D12Resource> = None;
    if unsafe {
        device.CreateCommittedResource(
            &tex_heap,
            D3D12_HEAP_FLAG_NONE,
            &tex_desc,
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut tex_opt,
        )
    }
    .is_err()
    {
        return None;
    }
    let tex = tex_opt?;

    // Copyable footprint of subresource 0 (256-aligned row pitch).
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total_bytes: u64 = 0;
    unsafe {
        device.GetCopyableFootprints(
            &tex_desc,
            0,
            1,
            0,
            Some(&mut footprint),
            None,
            None,
            Some(&mut total_bytes),
        )
    };
    if total_bytes == 0 || footprint.Footprint.RowPitch == 0 {
        return None;
    }
    let buf_heap = D3D12_HEAP_PROPERTIES {
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
    let mut upload_opt: Option<ID3D12Resource> = None;
    if unsafe {
        device.CreateCommittedResource(
            &buf_heap,
            D3D12_HEAP_FLAG_NONE,
            &buf_desc,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut upload_opt,
        )
    }
    .is_err()
    {
        return None;
    }
    let upload = upload_opt?;
    let row_pitch = footprint.Footprint.RowPitch as usize;
    let src_row = (pw as usize) * RGBA8_BPP;
    let mut mapped: *mut c_void = std::ptr::null_mut();
    if unsafe { upload.Map(0, None, Some(&mut mapped)) }.is_err() || mapped.is_null() {
        return None;
    }
    let dstp = mapped as *mut u8;
    for y in 0..ph as usize {
        let so = y * src_row;
        let d_o = y * row_pitch;
        if so + src_row > pixels.len() || (d_o + src_row) as u64 > total_bytes {
            break;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(pixels.as_ptr().add(so), dstp.add(d_o), src_row);
        }
    }
    unsafe { upload.Unmap(0, None) };

    let queue_desc = D3D12_COMMAND_QUEUE_DESC {
        Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
        Priority: 0,
        Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
        NodeMask: 0,
    };
    let queue = unsafe { device.CreateCommandQueue::<ID3D12CommandQueue>(&queue_desc) }.ok()?;
    let allocator = unsafe {
        device.CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT)
    }
    .ok()?;
    let list = unsafe {
        device.CreateCommandList::<_, _, ID3D12GraphicsCommandList>(
            0,
            D3D12_COMMAND_LIST_TYPE_DIRECT,
            &allocator,
            None,
        )
    }
    .ok()?;
    // tex was created in COPY_DEST -- copy upload -> tex sub 0, then transition tex to COPY_SOURCE.
    let mut src_loc = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(upload.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    let mut dst_loc = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(tex.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    unsafe { list.CopyTextureRegion(&dst_loc, 0, 0, 0, &src_loc, None) };
    unsafe { ManuallyDrop::drop(&mut src_loc.pResource) };
    unsafe { ManuallyDrop::drop(&mut dst_loc.pResource) };
    unsafe {
        record_transition(
            &list,
            &tex,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )
    };
    if unsafe { list.Close() }.is_err() {
        return None;
    }
    let base_list = list.cast::<ID3D12CommandList>().ok()?;
    unsafe { queue.ExecuteCommandLists(&[Some(base_list)]) };
    let fence = unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }.ok()?;
    if unsafe { queue.Signal(&fence, READBACK_FENCE_TARGET) }.is_err() {
        return None;
    }
    if unsafe { fence.GetCompletedValue() } < READBACK_FENCE_TARGET {
        let event = unsafe { CreateEventW(None, false, false, None) }.ok()?;
        let _ = unsafe { fence.SetEventOnCompletion(READBACK_FENCE_TARGET, event) };
        let wait = unsafe { WaitForSingleObject(event, READBACK_FENCE_WAIT_MS) };
        let _ = unsafe { CloseHandle(event) };
        if wait != WAIT_OBJECT_0 {
            return None;
        }
    }
    Some(tex)
}

/// One-time setup for the per-frame composite: derive the device from the backbuffer, read the captured
/// portrait, build the persistent source texture + command allocator/list/fence + our OWN private DIRECT
/// queue. We do NOT submit on the game's command queue -- doing so from the Present hook caused a vkd3d
/// access violation; instead we CPU-fence-wait our copy to completion before the original Present runs.
/// Stores every object as a leaked raw pointer. `false` on any failure. The step logs localize a hardware
/// fault (catch_unwind cannot catch an access violation inside a D3D12 call).
unsafe fn init_overlay_draw_state(backbuffer: &ID3D12Resource) -> bool {
    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { backbuffer.GetDevice(&mut device_opt) }.is_err() {
        return false;
    }
    let Some(device) = device_opt else {
        return false;
    };

    let (pw, ph, pixels) = {
        let Ok(g) = LOADING_BG_PORTRAIT_RGBA.lock() else {
            return false;
        };
        match g.as_ref() {
            Some((w, h, px)) => (*w, *h, px.clone()),
            None => return false,
        }
    };
    if pw == 0 || ph == 0 || pw > MAX_RT_DIM || ph > MAX_RT_DIM {
        return false;
    }
    if pixels.len() < (pw as usize) * (ph as usize) * RGBA8_BPP {
        return false;
    }
    append_autoload_debug(format_args!(
        "present-overlay: draw init step1 device + portrait ok ({pw}x{ph}, {} bytes)",
        pixels.len()
    ));

    let Some(tex) = (unsafe { create_portrait_source_texture(&device, pw, ph, &pixels) }) else {
        return false;
    };
    append_autoload_debug(format_args!(
        "present-overlay: draw init step2 portrait source texture ready"
    ));
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
    // CreateCommandList returns the list OPEN; close it so the first per-frame `Reset` is valid.
    if unsafe { list.Close() }.is_err() {
        return false;
    }
    let Ok(fence) = (unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }) else {
        return false;
    };

    // Our OWN persistent DIRECT queue (the proven readback/upload pattern). We never touch the game's queue.
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
    append_autoload_debug(format_args!(
        "present-overlay: draw init step3 cmd objects + own queue ready"
    ));

    OVERLAY_PORTRAIT_TEX.store(tex.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_ALLOCATOR.store(allocator.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_LIST.store(list.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_FENCE.store(fence.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_QUEUE.store(queue.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_PORTRAIT_W.store(pw as usize, Ordering::SeqCst);
    OVERLAY_PORTRAIT_H.store(ph as usize, Ordering::SeqCst);
    true
}

/// Composite the captured portrait onto the swapchain backbuffer. Called from the Present detour every
/// frame while the now-loading screen is up. `catch_unwind` + every COM call checked -> never panics or
/// crashes on the game's render thread; on any failure it draws nothing and returns `false`.
pub(crate) unsafe fn composite_portrait_on_swapchain(base: usize, swapchain_raw: usize) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        composite_portrait_inner(base, swapchain_raw)
    }))
    .unwrap_or(false)
}

unsafe fn composite_portrait_inner(base: usize, swapchain_raw: usize) -> bool {
    if PROFILE_BAKE_RGBA_CAPTURED.load(Ordering::SeqCst) == 0 {
        return false;
    }
    if portrait_render_drive_enabled() {
        return false;
    }
    let fake_vis = unsafe { fake_loading_screen_visible(base) };
    let now_load = unsafe { now_loading_active(base) };
    if now_load {
        OVERLAY_NOW_LOADING_SEEN.store(1, Ordering::SeqCst);
    }
    let world_ready =
        OVERLAY_NOW_LOADING_SEEN.load(Ordering::SeqCst) != 0 && !now_load && !fake_vis;
    let forge_committed = LOADING_BG_TEXTURE_REDIRECT_COMMITS.load(Ordering::SeqCst) > 0;
    if world_ready || !(forge_committed || fake_vis || now_load) {
        return false;
    }
    if OVERLAY_DRAW_STATE.load(Ordering::SeqCst) == 2 {
        return false;
    }

    let sc_raw = swapchain_raw as *mut c_void;
    let Some(sc) = (unsafe { IDXGISwapChain3::from_raw_borrowed(&sc_raw) }) else {
        return false;
    };
    let idx = unsafe { sc.GetCurrentBackBufferIndex() };
    let Ok(backbuffer) = (unsafe { sc.GetBuffer::<ID3D12Resource>(idx) }) else {
        return false;
    };

    if OVERLAY_DRAW_STATE.load(Ordering::SeqCst) == 0 {
        if unsafe { init_overlay_draw_state(&backbuffer) } {
            OVERLAY_DRAW_STATE.store(1, Ordering::SeqCst);
            OVERLAY_PORTRAIT_VERSION.store(
                LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
            append_autoload_debug(format_args!(
                "present-overlay: draw state READY (portrait {}x{})",
                OVERLAY_PORTRAIT_W.load(Ordering::SeqCst),
                OVERLAY_PORTRAIT_H.load(Ordering::SeqCst)
            ));
        } else {
            OVERLAY_DRAW_STATE.store(2, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "present-overlay: draw init FAILED -- giving up"
            ));
            return false;
        }
    }

    // LIVE RE-UPLOAD: when the draw-phase task has published a newer portrait (version bumped from the
    // throttled built-RT readback), rebuild the overlay source texture from the fresh RGBA so the displayed
    // head UPDATES (look-at follows) instead of freezing on the first captured frame. Same-dims only; on any
    // failure keep the previous texture (draw the last good frame). Our own queue/device, no game state.
    let cur_ver = LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst);
    if cur_ver != OVERLAY_PORTRAIT_VERSION.load(Ordering::SeqCst) {
        let snap = LOADING_BG_PORTRAIT_RGBA.lock().ok().and_then(|g| g.clone());
        if let Some((nw, nh, npx)) = snap {
            if nw as usize == OVERLAY_PORTRAIT_W.load(Ordering::SeqCst)
                && nh as usize == OVERLAY_PORTRAIT_H.load(Ordering::SeqCst)
                && npx.len() >= (nw as usize) * (nh as usize) * RGBA8_BPP
            {
                let mut device_opt: Option<ID3D12Device> = None;
                if unsafe { backbuffer.GetDevice(&mut device_opt) }.is_ok() {
                    if let Some(device) = device_opt {
                        if let Some(newtex) =
                            unsafe { create_portrait_source_texture(&device, nw, nh, &npx) }
                        {
                            let old = OVERLAY_PORTRAIT_TEX
                                .swap(newtex.into_raw() as usize, Ordering::SeqCst);
                            if old != 0 {
                                // Release the previous source texture (reclaim its raw COM ref).
                                drop(unsafe { ID3D12Resource::from_raw(old as *mut c_void) });
                            }
                            OVERLAY_PORTRAIT_VERSION.store(cur_ver, Ordering::SeqCst);
                        }
                    }
                }
            } else {
                // Dims changed (shouldn't for a fixed RT) -- accept the version so we don't spin.
                OVERLAY_PORTRAIT_VERSION.store(cur_ver, Ordering::SeqCst);
            }
        }
    }

    let tex_raw = OVERLAY_PORTRAIT_TEX.load(Ordering::SeqCst) as *mut c_void;
    let alloc_raw = OVERLAY_ALLOCATOR.load(Ordering::SeqCst) as *mut c_void;
    let list_raw = OVERLAY_LIST.load(Ordering::SeqCst) as *mut c_void;
    let fence_raw = OVERLAY_FENCE.load(Ordering::SeqCst) as *mut c_void;
    let queue_raw = OVERLAY_QUEUE.load(Ordering::SeqCst) as *mut c_void;
    let (Some(tex), Some(allocator), Some(list), Some(fence), Some(queue)) = (unsafe {
        (
            ID3D12Resource::from_raw_borrowed(&tex_raw),
            ID3D12CommandAllocator::from_raw_borrowed(&alloc_raw),
            ID3D12GraphicsCommandList::from_raw_borrowed(&list_raw),
            ID3D12Fence::from_raw_borrowed(&fence_raw),
            ID3D12CommandQueue::from_raw_borrowed(&queue_raw),
        )
    }) else {
        return false;
    };

    let bb_desc = unsafe { backbuffer.GetDesc() };
    let bw = bb_desc.Width as u32;
    let bh = bb_desc.Height;
    let pw = OVERLAY_PORTRAIT_W.load(Ordering::SeqCst) as u32;
    let ph = OVERLAY_PORTRAIT_H.load(Ordering::SeqCst) as u32;
    if bw == 0 || bh == 0 || pw == 0 || ph == 0 {
        return false;
    }
    // Clamp the copy to the backbuffer and center it (CopyTextureRegion does not scale).
    let cw = pw.min(bw);
    let ch = ph.min(bh);
    let dx = (bw - cw) / 2;
    let dy = (bh - ch) / 2;

    if unsafe { allocator.Reset() }.is_err() {
        return false;
    }
    if unsafe { list.Reset(allocator, None) }.is_err() {
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
    let src_box = D3D12_BOX {
        left: 0,
        top: 0,
        front: 0,
        right: cw,
        bottom: ch,
        back: 1,
    };
    let mut src_loc = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(tex.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    let mut dst_loc = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(backbuffer.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    unsafe { list.CopyTextureRegion(&dst_loc, dx, dy, 0, &src_loc, Some(&src_box)) };
    unsafe { ManuallyDrop::drop(&mut src_loc.pResource) };
    unsafe { ManuallyDrop::drop(&mut dst_loc.pResource) };
    unsafe {
        record_transition(
            list,
            &backbuffer,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_PRESENT,
        )
    };
    if unsafe { list.Close() }.is_err() {
        return false;
    }
    let Ok(base_list) = list.cast::<ID3D12CommandList>() else {
        return false;
    };
    unsafe { queue.ExecuteCommandLists(&[Some(base_list)]) };
    let val = OVERLAY_FENCE_VAL.fetch_add(1, Ordering::SeqCst) + 1;
    if unsafe { queue.Signal(fence, val) }.is_err() {
        return false;
    }
    if unsafe { fence.GetCompletedValue() } < val {
        let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else {
            return false;
        };
        if unsafe { fence.SetEventOnCompletion(val, event) }.is_err() {
            let _ = unsafe { CloseHandle(event) };
            return false;
        }
        let wait = unsafe { WaitForSingleObject(event, READBACK_FENCE_WAIT_MS) };
        let _ = unsafe { CloseHandle(event) };
        if wait != WAIT_OBJECT_0 {
            return false;
        }
    }
    let hits = OVERLAY_DRAW_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if hits == 1 {
        append_autoload_debug(format_args!(
            "present-overlay: portrait COMPOSITED onto backbuffer {bw}x{bh} (portrait {pw}x{ph} at {dx},{dy})"
        ));
    }
    true
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

/// True if the read-back RGBA8 image looks like a SOLID-COLOR-CHECKER PLACEHOLDER (our magenta/white or
/// magenta/yellow er-tpf cover, or an unrendered RT clear pattern) rather than a real 3D head render.
///
/// WHY: `portrait_center_nonblack` only proves "not all black" -- a bright magenta checker (255,0,255)
/// trivially passes it, so `oracle_loading_bg_portrait_gx_nonblack` was a FALSE POSITIVE for the autoload
/// path (run postcontinue-lookat-smoke 2026-06-30: nonblack=True but the captured bytes were a magenta/
/// white checker, because the model builds but is never rendered into the offscreen RT once the menu's
/// render driver dies post-Continue). A real character render has many shaded colors and few fully-
/// saturated "pure" texels; a checker is ~2 colors, each with channels pinned to 0/255. Heuristic over the
/// center region: sample texels, quantize to 5 bits/channel, and call it a checker if (a) the 2 most-common
/// quantized colors cover >= 85% of samples AND (b) >= 70% of samples are "pure" (every channel <16 or >239).
pub(crate) fn portrait_looks_like_checker(width: u32, height: u32, pixels: &[u8]) -> bool {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || pixels.len() < w * h * RGBA8_BPP {
        return false;
    }
    const REGION: usize = 128;
    let half = REGION / 2;
    let (cx, cy) = (w / 2, h / 2);
    let x0 = cx.saturating_sub(half);
    let x1 = (cx + half).min(w);
    let y0 = cy.saturating_sub(half);
    let y1 = (cy + half).min(h);
    let mut counts: std::collections::HashMap<u16, u32> = std::collections::HashMap::new();
    let mut total = 0u32;
    let mut pure = 0u32;
    for y in y0..y1 {
        for x in x0..x1 {
            let idx = (y * w + x) * RGBA8_BPP;
            let (r, g, b) = (pixels[idx], pixels[idx + 1], pixels[idx + 2]);
            // pure = every channel near an extreme (0/255) -> checker/placeholder hallmark
            let is_pure = |c: u8| c < 16 || c > 239;
            if is_pure(r) && is_pure(g) && is_pure(b) {
                pure += 1;
            }
            let key = (((r >> 3) as u16) << 10) | (((g >> 3) as u16) << 5) | ((b >> 3) as u16);
            *counts.entry(key).or_insert(0) += 1;
            total += 1;
        }
    }
    if total == 0 {
        return false;
    }
    let mut vals: Vec<u32> = counts.values().copied().collect();
    vals.sort_unstable_by(|a, b| b.cmp(a));
    let top2: u32 = vals.iter().take(2).sum();
    let top2_frac = top2 as f32 / total as f32;
    let pure_frac = pure as f32 / total as f32;
    top2_frac >= 0.85 && pure_frac >= 0.70
}
