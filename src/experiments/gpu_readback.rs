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
use std::sync::Mutex;
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
static OVERLAY_PORTRAIT_VERSION: AtomicUsize = AtomicUsize::new(usize::MAX); // last LOADING_BG_PORTRAIT_RGBA_VERSION composited to the backbuffer
static OVERLAY_ALLOCATOR: AtomicUsize = AtomicUsize::new(0); // ID3D12CommandAllocator (DIRECT)
static OVERLAY_LIST: AtomicUsize = AtomicUsize::new(0); // ID3D12GraphicsCommandList (DIRECT, kept closed)
static OVERLAY_FENCE: AtomicUsize = AtomicUsize::new(0); // ID3D12Fence
static OVERLAY_QUEUE: AtomicUsize = AtomicUsize::new(0); // our OWN private DIRECT ID3D12CommandQueue (leaked)
static OVERLAY_FENCE_VAL: AtomicU64 = AtomicU64::new(0); // monotonically incremented per submit
static OVERLAY_PORTRAIT_W: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_PORTRAIT_H: AtomicUsize = AtomicUsize::new(0);
/// Successful backbuffer composites submitted (RAM semaphore that the portrait is actually being drawn).
pub(crate) static OVERLAY_DRAW_HITS: AtomicUsize = AtomicUsize::new(0);
/// Count of LIVE RE-UPLOADS: each time the overlay source texture was rebuilt from a fresh
/// (version-bumped) `LOADING_BG_PORTRAIT_RGBA` -> proves the DISPLAYED head updated per-frame (followed
/// the cursor), not froze on the first captured frame. `oracle_overlay_reuploads`.
pub(crate) static OVERLAY_REUPLOADS: AtomicUsize = AtomicUsize::new(0);
/// Latches once the `now_loading` streaming screen (the tips+bar loading screen the portrait belongs on)
/// has been seen this window. The correct STOP is this-seen-then-gone: the bar appeared, filled, and the
/// game transitioned to gameplay. Reset per window on re-arm.
static OVERLAY_NOW_LOADING_SEEN: AtomicUsize = AtomicUsize::new(0);
/// One-shot diagnostic when the anti-runaway backstop disables the loading portrait overlay.
static OVERLAY_WORLD_STOP_LOGGED: AtomicUsize = AtomicUsize::new(0);

// LOADING-SCREEN WINDOW state machine. DECISIVE timeline (run portrait-swap-fix2-noteardown-20260702-213407):
// the tips+bar loading screen the loaded character sits on is the `now_loading` streaming flag, TRUE from
// +27.6s to +78s. IN_WORLD_REACHED (player becomes controllable) fires at +25.9s -- 1.7s BEFORE that screen
// even appears -- because PlayerIns is live throughout the loading screen (the documented false positive).
// So IN_WORLD is NOT a valid stop; using it (directly, or via the CSNowLoadingHelperImp update hook which
// read 0 hits) popped the portrait ~2s before the bar was even up. The overlay now composites from the
// moment we have a captured head, BRIDGES the pre-now_loading gap, then rides the now_loading window and
// stops only when it has been seen and then drops (the game's own transition). A generous present-counted
// backstop guards the (non-product) case where now_loading never appears, so we can't composite forever.
/// 1 = stopped (window over); stays stopped until a NEW loading window re-arms it.
static OVERLAY_STOPPED: AtomicUsize = AtomicUsize::new(0);
/// `PROFILE_LOADSCREEN_TABLE_BUILDS` at the moment of the stop -- a later build = a new window (re-arm).
static OVERLAY_STOP_TABLE_BUILDS: AtomicUsize = AtomicUsize::new(0);
/// Presents counted while IN_WORLD but now_loading NOT yet seen -- the pre-loading-screen bridge gap. Reset
/// to 0 the instant now_loading latches (then the seen-then-gone stop takes over) and on re-arm.
static OVERLAY_BRIDGE_PRESENTS: AtomicUsize = AtomicUsize::new(0);
/// Anti-runaway backstop: max bridge presents before we stop even though now_loading was never seen. The
/// real gap is ~1.7s; this is set FAR above any real present rate over that gap so it NEVER pre-empts a
/// genuine load (which always shows now_loading) -- it only bounds a deeply-wrong state. Biased huge so the
/// overlay errs toward holding the portrait (the product requirement) over popping early (the bug).
const OVERLAY_NOWLOAD_BRIDGE_MAX_PRESENTS: usize = 60000;
/// RAM oracle: number of overlay window stops (`oracle_overlay_window_stops`).
pub(crate) static OVERLAY_WINDOW_STOPS: AtomicUsize = AtomicUsize::new(0);
/// RAM oracle: last stop reason (`oracle_overlay_stop_reason`): 0=none yet, 1=now_loading seen-then-gone
/// (primary, the game's real loading screen finished -- the spec-correct pop), 3=anti-runaway backstop
/// (now_loading never appeared; a signal the assumption broke, not a normal stop).
pub(crate) static OVERLAY_STOP_REASON: AtomicUsize = AtomicUsize::new(0);

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
    unsafe { find_d3d12_resource_ex(start, 0, false, 0) }.map(|(r, _)| r)
}

// CANDIDATE PINS (fix for the cross-slot portrait swap, run strip-default-drive-20260702-194018): the
// offscreen nest BFS reaches EVERY profile slot's same-size 1024x1024 RT through shared GX structures, so
// "largest texture, first found" flips to a FOREIGN slot's RT the moment a late slot's model finishes
// building mid-load -> the displayed portrait swapped to another save character between two frames. Once a
// readback of candidate `v` publishes a confirmed non-checker head, `v` is PINNED and preferred by every
// subsequent scan while it remains reachable; only its disappearance (RT recreation/teardown) falls back to
// the largest-candidate heuristic. Pinning the CANDIDATE POINTER (re-QI'd each frame) -- not the resource
// handle -- avoids the stale-cache dangling-handle bug that killed `readback_cached_content_rgba8`.
/// Pinned content-RT candidate object pointer (0 = unpinned). `oracle_portrait_rt_pin`.
pub(crate) static PROFILE_RT_PIN: AtomicUsize = AtomicUsize::new(0);
/// Times the pin moved to a DIFFERENT candidate after first latch (`oracle_portrait_rt_pin_switches`).
/// >0 on a single load window means the content source was unstable -- the swap-bug tripwire.
pub(crate) static PROFILE_RT_PIN_SWITCHES: AtomicUsize = AtomicUsize::new(0);
/// Pinned depth-sibling candidate pointer (0 = unpinned); latched when a depth readback yields a mask
/// with clean bg/head separation, so the alpha cutout can't sample a foreign slot's depth buffer.
pub(crate) static PROFILE_DEPTH_PIN: AtomicUsize = AtomicUsize::new(0);

// Static-RE'd offscreen scene-target member chain (Ghidra dump decompiles, 2026-07-03 -- the
// black-background-on-reload root fix). The CSEzOffscreenRend stores its GXSgCompositeScene facade
// at +0x48 (FUN_140bb7440 wraps the GXSgSceneFactory product into `field9_0x48`); the facade's
// vt+0x18 getter (FUN_141b64640) is the trivial member read `*(facade+0x38)` = the 0xc4b0-byte
// GXSceneContext; the context's render-target bundle sits at +0x248 (FUN_140bb73a0 clears its RTV
// via the FUN_141a1adc0 accessor) with the color RTV view at bundle+0x30 and the DEPTH DSV view at
// bundle+0x40 -- proven by the engine's own target bind FUN_141a086c0:
// `FUN_1419ea4c0(cmdlist, 1, &*(bundle+0x30), *(bundle+0x40))` (count, RTV array, DSV).
// bundle+0x548 is the redirect-to-global-target flag the accessor checks; when set the local views
// are not authoritative, so the chain fails closed.
const OFFSCREEN_SCENE_FACADE_OFFSET: usize = 0x48;
const SCENE_FACADE_CONTEXT_OFFSET: usize = 0x38;
const SCENE_CONTEXT_TARGET_BUNDLE_OFFSET: usize = 0x248;
const TARGET_BUNDLE_REDIRECT_FLAG_OFFSET: usize = 0x548;
const TARGET_BUNDLE_DSV_VIEW_OFFSET: usize = 0x40;
/// The COLOR RTV view sits at bundle+0x30 (paired with the depth DSV at bundle+0x40 in the SAME bundle) --
/// resolving BOTH from one bundle guarantees the color and depth are the same render pass's siblings.
const TARGET_BUNDLE_RTV_VIEW_OFFSET: usize = 0x30;
/// One-shot diagnostic latch for the deterministic depth-view chain (first resolve + first miss).
static DEPTH_CHAIN_DIAG: AtomicUsize = AtomicUsize::new(0);

/// Find the offscreen scene's DEPTH-STENCIL resource (same-size sibling of the color RT, observed
/// format 19 = R32G8X24_TYPELESS). Used for the depth-key transparent background: background =
/// pixels the character geometry never wrote (cleared/far depth).
///
/// Resolution order (run anim-bind21 root cause): the whole-nest heuristic BFS only reached the
/// depth buffer when the GX allocator happened to place it within the bounded scan window -- true
/// for boot-era renderers (load 1 keyed fine), FALSE for every renderer built mid-load, which made
/// window 2+ fail open to the opaque black background (dk find-fails climbed 1:1 with no-mask
/// frames). So: first walk the static-RE'd member chain straight to OUR scene's DSV view object
/// and QI the resource out of that tiny nest (deterministic, slot-local by construction); only if
/// a chain link is null/redirected fall back to the historical BFS from the offscreen object.
pub(crate) unsafe fn find_depth_resource(start: usize) -> Option<(ID3D12Resource, usize)> {
    // Deterministic bundle-paired DSV FIRST, walked with NO pin (prefer=0): the color RTV (bundle+0x30)
    // and depth DSV (bundle+0x40) are the SAME render-target bundle, so this view already points at the
    // scene's OWN depth sibling -- letting the drifting DEPTH_PIN win here would override that correct
    // pointer with a foreign larger buffer (user 2026-07-03: share the paired pointer, don't heuristically
    // re-pin). The pin is kept ONLY in the BFS fallback below, for mid-load renderers whose DSV chain is
    // null/redirected.
    if let Some(dsv_view) = unsafe { offscreen_depth_view(start) } {
        if let Some(found) = unsafe { find_d3d12_resource_ex(dsv_view, 0, true, 0) } {
            if DEPTH_CHAIN_DIAG.fetch_or(1, Ordering::SeqCst) & 1 == 0 {
                append_autoload_debug(format_args!(
                    "depth-chain: resolved DSV view 0x{dsv_view:x} from off=0x{start:x} -> depth resource cand 0x{:x}",
                    found.1
                ));
            }
            return Some(found);
        }
    }
    let prefer = PROFILE_DEPTH_PIN.load(Ordering::SeqCst);
    if DEPTH_CHAIN_DIAG.fetch_or(2, Ordering::SeqCst) & 2 == 0 {
        append_autoload_debug(format_args!(
            "depth-chain: MISS (facade/ctx/dsv null, redirected, or view nest yielded no depth texture) off=0x{start:x} -- falling back to heuristic nest BFS"
        ));
    }
    unsafe { find_d3d12_resource_ex(start, 0, true, prefer) }
}

/// Resolve the offscreen scene's DSV view object via the static-RE'd member chain above. All reads
/// fault-guarded; `None` when any link is null/implausible or the bundle is redirected to the
/// global target (fail closed -- the local view would not be what the scene renders into).
unsafe fn offscreen_depth_view(off: usize) -> Option<usize> {
    let bundle = unsafe { offscreen_target_bundle(off) }?;
    let dsv = unsafe { safe_read_usize(bundle + TARGET_BUNDLE_DSV_VIEW_OFFSET) }?;
    (dsv > 0x10000 && dsv < 0x8000_0000_0000).then_some(dsv)
}

/// Resolve the offscreen scene's render-target BUNDLE via the static-RE'd member chain (facade -> ctx ->
/// bundle), redirect-checked so a bundle pointing at the global target fails closed. The color RTV view is
/// at `bundle+0x30` and the depth DSV at `bundle+0x40` -- resolving BOTH from this one bundle is what makes
/// the coherent readback's color and depth the SAME render pass's paired siblings (no cross-bundle drift,
/// the 2nd-character desync). All reads fault-guarded.
unsafe fn offscreen_target_bundle(off: usize) -> Option<usize> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let plausible = |v: usize| v > 0x10000 && v < 0x8000_0000_0000 && v != null;
    if !plausible(off) {
        return None;
    }
    let facade = unsafe { safe_read_usize(off + OFFSCREEN_SCENE_FACADE_OFFSET) }?;
    if !plausible(facade) {
        return None;
    }
    let ctx = unsafe { safe_read_usize(facade + SCENE_FACADE_CONTEXT_OFFSET) }?;
    if !plausible(ctx) {
        return None;
    }
    let bundle = ctx + SCENE_CONTEXT_TARGET_BUNDLE_OFFSET;
    if unsafe { safe_read_usize(bundle + TARGET_BUNDLE_REDIRECT_FLAG_OFFSET) }? & 0xff != 0 {
        return None;
    }
    Some(bundle)
}

/// The offscreen scene's COLOR RTV view object (`bundle+0x30`) -- the paired sibling of
/// `offscreen_depth_view`'s DSV. Resolving the coherent readback's color from THIS (same bundle as the
/// depth) guarantees they are the same render pass, so the depth-derived mask matches the color head.
unsafe fn offscreen_color_view(off: usize) -> Option<usize> {
    let bundle = unsafe { offscreen_target_bundle(off) }?;
    let rtv = unsafe { safe_read_usize(bundle + TARGET_BUNDLE_RTV_VIEW_OFFSET) }?;
    (rtv > 0x10000 && rtv < 0x8000_0000_0000).then_some(rtv)
}

/// Depth-stencil TEXTURE2D acceptor (mirror of `try_texture2d` for depth formats): accept the common
/// depth/typeless-depth formats so the nest scan can pick the depth buffer instead of the color RT.
unsafe fn try_depth_texture2d(ptr: usize) -> Option<(ID3D12Resource, u64)> {
    let raw = ptr as *mut c_void;
    let unk = unsafe { IUnknown::from_raw_borrowed(&raw) }?;
    let res: ID3D12Resource = unk.cast().ok()?;
    let desc = unsafe { res.GetDesc() };
    // Depth formats: R32G8X24_TYPELESS(19), D32_FLOAT_S8X24(20), R32_FLOAT_X8X24(21), R24G8_TYPELESS(44),
    // D24_UNORM_S8(45), R32_TYPELESS(39), D32_FLOAT(40), R16_TYPELESS(53), D16_UNORM(55).
    let f = desc.Format.0;
    let is_depth = matches!(f, 19 | 20 | 21 | 44 | 45 | 39 | 40 | 53 | 55);
    if is_depth
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

/// Like `find_d3d12_resource` but (a) returns the candidate object pointer alongside the resource, and
/// (b) skips any candidate whose pointer == `exclude_v`. Lets the RT->SRV copy pick the SRV from its own
/// single-texture nest, then the LARGEST OTHER texture in the offscreen nest as the content source --
/// deterministic where plain "largest texture" is ambiguous between two same-size textures.
/// `prefer_v` (0 = none): a previously PINNED candidate -- if the scan reaches it and it still QIs as a
/// valid texture, it wins immediately over the largest-candidate heuristic, so the resolved content source
/// cannot flip between same-size RTs frame-to-frame (the cross-slot portrait-swap bug).
unsafe fn find_d3d12_resource_ex(
    start: usize,
    exclude_v: usize,
    want_depth: bool,
    prefer_v: usize,
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
                            } else if let Some((res, area)) = unsafe {
                                if want_depth {
                                    try_depth_texture2d(v)
                                } else {
                                    try_texture2d(v)
                                }
                            } {
                                if prefer_v != 0 && v == prefer_v {
                                    // Pinned candidate still reachable + valid: it wins outright.
                                    return Some((res, v));
                                }
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

/// Per-frame DISPLAY readback: re-resolve the offscreen nest's content RT FRESH each frame -- but PREFER
/// the pinned candidate (`PROFILE_RT_PIN`) when it is still reachable, so the resolved source cannot flip
/// to another profile slot's same-size RT mid-load (the cross-slot swap bug). Falls back to the largest-
/// texture heuristic only while unpinned or after the pinned RT was recreated/torn down. Copies with the
/// CACHED `RB_FAST_*` objects so it succeeds every frame (vs the per-call object creation that only
/// published ~4x). Returns the candidate pointer alongside the pixels; the caller pins it once the frame
/// is confirmed to be a real (non-checker) head. Fault-guarded; caller must gate on a live renderer/model
/// so the scan can't race a teardown free.
pub(crate) unsafe fn readback_offscreen_fast(
    gpu_child: usize,
) -> Option<(u32, u32, Vec<u8>, usize)> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let prefer = PROFILE_RT_PIN.load(Ordering::SeqCst);
        let (resource, cand) = find_d3d12_resource_ex(gpu_child, 0, false, prefer)?;
        readback_resource_cached_fast(resource).map(|(w, h, px)| (w, h, px, cand))
    }))
    .ok()
    .flatten()
}

/// Consume the coherently-captured depth for this tick (clears it so it is used exactly once and never
/// carried to a later frame). `None` -> the caller reads depth fresh (the legacy separate path).
fn take_coherent_depth() -> Option<(u32, u32, Vec<f32>, usize)> {
    COHERENT_DEPTH.lock().ok().and_then(|mut g| g.take())
}

/// Color+depth read COHERENTLY (bug #3 fix). Same `(w, h, rgba, cand)` shape as `readback_offscreen_fast`
/// so the drive is unchanged, but it ALSO stashes the depth captured on the same fence into
/// `COHERENT_DEPTH` for this tick's `apply_depth_alpha_key` to consume -- so the mask is derived from the
/// SAME frame as the color. On ANY failure the stash is CLEARED and we fall back to the separate color
/// path (the mask then reads depth fresh); never a stale depth, never a crash.
pub(crate) unsafe fn readback_offscreen_fast_coherent(
    gpu_child: usize,
) -> Option<(u32, u32, Vec<u8>, usize)> {
    if let Some((cw, ch, color, ccand, dw, dh, depth, dcand)) =
        unsafe { readback_offscreen_color_depth_coherent(gpu_child) }
    {
        COHERENT_READ_OK.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut g) = COHERENT_DEPTH.lock() {
            *g = Some((dw, dh, depth, dcand));
        }
        return Some((cw, ch, color, ccand));
    }
    // Coherent read failed -> clear the stash so no stale depth leaks into this frame's mask.
    COHERENT_READ_FALLBACK.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut g) = COHERENT_DEPTH.lock() {
        *g = None;
    }
    unsafe { readback_offscreen_fast(gpu_child) }
}

/// COHERENT single-fence readback of the offscreen COLOR RT and its DEPTH sibling: both copies are
/// recorded into ONE command list and gated by ONE fence, so they capture the SAME GPU state. This is the
/// root fix for the wrong-shaped mask (bug #3): the separate RB_FAST_*/RB_DEPTH_* paths each fence their
/// own copy, and the game's async render can advance the RT between them (color=frameN, depth=frameN+1),
/// so the depth-derived cutout no longer matches the head. Resolves color via the RT pin and depth via
/// `find_depth_resource` (same sources as the twins). Returns
/// `(cw, ch, rgba, color_cand, dw, dh, depth_f32, depth_cand)`. `None` on any failure (caller falls back
/// to the separate path). Never touches the game's queues; catch_unwind; fault-guarded like the twins.
pub(crate) unsafe fn readback_offscreen_color_depth_coherent(
    gpu_child: usize,
) -> Option<(u32, u32, Vec<u8>, usize, u32, u32, Vec<f32>, usize)> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        // Resolve the COLOR from the SAME render-target bundle as the depth (bundle+0x30 RTV), so the two
        // are the same render pass's paired siblings -- the fix for the 2nd-character desync, where the
        // pinned color and the bundle depth came from DIFFERENT bundles (temporally coherent via the one
        // fence, but not IDENTITY-coherent). Fall back to the RT-pin scan only if the bundle RTV view is
        // unavailable (mid-load renderers whose scene chain is null/redirected).
        let bundle_color = offscreen_color_view(gpu_child)
            .and_then(|rtv| unsafe { find_d3d12_resource_ex(rtv, 0, false, 0) });
        let (color_res, color_cand) = match bundle_color {
            Some(c) => c,
            None => {
                let prefer_c = PROFILE_RT_PIN.load(Ordering::SeqCst);
                find_d3d12_resource_ex(gpu_child, 0, false, prefer_c)?
            }
        };

        let mut device_opt: Option<ID3D12Device> = None;
        color_res.GetDevice(&mut device_opt).ok()?;
        let device = device_opt?;

        // Color footprint (subresource 0).
        let cdesc: D3D12_RESOURCE_DESC = color_res.GetDesc();
        let cw = cdesc.Width as u32;
        let ch = cdesc.Height;
        let cformat = cdesc.Format;
        if cw == 0 || ch == 0 || cw > MAX_RT_DIM || ch > MAX_RT_DIM {
            return None;
        }
        LOADING_BG_PORTRAIT_FORMAT.store(cformat.0 as usize, Ordering::SeqCst);

        // DEPTH sibling via the deterministic bundle-paired chain (find_depth_resource now walks the
        // scene's own DSV with no pin), so it is THIS scene's depth, not a drifted foreign buffer.
        let (depth_res, depth_cand) = find_depth_resource(gpu_child)?;
        let mut cfoot = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut ctotal: u64 = 0;
        device.GetCopyableFootprints(
            &cdesc,
            0,
            1,
            0,
            Some(&mut cfoot),
            None,
            None,
            Some(&mut ctotal),
        );
        if ctotal == 0 || cfoot.Footprint.RowPitch == 0 {
            return None;
        }

        // Depth footprint (plane 0 = R32 float).
        let ddesc: D3D12_RESOURCE_DESC = depth_res.GetDesc();
        let dw = ddesc.Width as u32;
        let dh = ddesc.Height;
        if dw == 0 || dh == 0 || dw > MAX_RT_DIM || dh > MAX_RT_DIM {
            return None;
        }
        // COHERENCE GUARD: color and its depth sibling must share dimensions -- a mismatch means the
        // depth is NOT this color's pair (the drift bug), so reject the frame rather than copy a bad pair
        // (the caller falls back). This is a sanity check on the deterministic pointer, not a size-based
        // resolution heuristic.
        if dw != cw || dh != ch {
            return None;
        }
        let mut dfoot = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut dtotal: u64 = 0;
        device.GetCopyableFootprints(
            &ddesc,
            0,
            1,
            0,
            Some(&mut dfoot),
            None,
            None,
            Some(&mut dtotal),
        );
        if dtotal == 0 || dfoot.Footprint.RowPitch == 0 {
            return None;
        }

        // Create the shared queue/allocator/list/fence ONCE (list left Closed so the first Reset works).
        if RB_COH_QUEUE.load(Ordering::SeqCst) == 0 {
            let queue_desc = D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                Priority: 0,
                Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
                NodeMask: 0,
            };
            let queue: ID3D12CommandQueue = device.CreateCommandQueue(&queue_desc).ok()?;
            let allocator: ID3D12CommandAllocator = device
                .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                .ok()?;
            let list: ID3D12GraphicsCommandList = device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
                .ok()?;
            list.Close().ok()?;
            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).ok()?;
            RB_COH_QUEUE.store(queue.into_raw() as usize, Ordering::SeqCst);
            RB_COH_ALLOC.store(allocator.into_raw() as usize, Ordering::SeqCst);
            RB_COH_LIST.store(list.into_raw() as usize, Ordering::SeqCst);
            RB_COH_FENCE.store(fence.into_raw() as usize, Ordering::SeqCst);
        }
        // (Re)create the readback buffers on footprint-size change (won't change for a fixed RT/depth).
        let heap_props = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_READBACK,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
            MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
            CreationNodeMask: 1,
            VisibleNodeMask: 1,
        };
        let buffer_desc = |bytes: u64| D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Alignment: 0,
            Width: bytes,
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
        if RB_COH_CBUFSIZE.load(Ordering::SeqCst) != ctotal {
            let bd = buffer_desc(ctotal);
            let mut b: Option<ID3D12Resource> = None;
            device
                .CreateCommittedResource(
                    &heap_props,
                    D3D12_HEAP_FLAG_NONE,
                    &bd,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    None,
                    &mut b,
                )
                .ok()?;
            let old = RB_COH_CBUF.swap(b?.into_raw() as usize, Ordering::SeqCst);
            if old != 0 {
                drop(ID3D12Resource::from_raw(old as *mut c_void));
            }
            RB_COH_CBUFSIZE.store(ctotal, Ordering::SeqCst);
        }
        if RB_COH_DBUFSIZE.load(Ordering::SeqCst) != dtotal {
            let bd = buffer_desc(dtotal);
            let mut b: Option<ID3D12Resource> = None;
            device
                .CreateCommittedResource(
                    &heap_props,
                    D3D12_HEAP_FLAG_NONE,
                    &bd,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    None,
                    &mut b,
                )
                .ok()?;
            let old = RB_COH_DBUF.swap(b?.into_raw() as usize, Ordering::SeqCst);
            if old != 0 {
                drop(ID3D12Resource::from_raw(old as *mut c_void));
            }
            RB_COH_DBUFSIZE.store(dtotal, Ordering::SeqCst);
        }

        // Borrow the cached COM objects (no refcount change; the statics own them).
        let q_raw = RB_COH_QUEUE.load(Ordering::SeqCst) as *mut c_void;
        let a_raw = RB_COH_ALLOC.load(Ordering::SeqCst) as *mut c_void;
        let l_raw = RB_COH_LIST.load(Ordering::SeqCst) as *mut c_void;
        let f_raw = RB_COH_FENCE.load(Ordering::SeqCst) as *mut c_void;
        let cb_raw = RB_COH_CBUF.load(Ordering::SeqCst) as *mut c_void;
        let db_raw = RB_COH_DBUF.load(Ordering::SeqCst) as *mut c_void;
        let (Some(queue), Some(allocator), Some(list), Some(fence), Some(cbuf), Some(dbuf)) = (
            ID3D12CommandQueue::from_raw_borrowed(&q_raw),
            ID3D12CommandAllocator::from_raw_borrowed(&a_raw),
            ID3D12GraphicsCommandList::from_raw_borrowed(&l_raw),
            ID3D12Fence::from_raw_borrowed(&f_raw),
            ID3D12Resource::from_raw_borrowed(&cb_raw),
            ID3D12Resource::from_raw_borrowed(&db_raw),
        ) else {
            return None;
        };
        // Previous frame's fence wait guarantees the GPU is idle, so resetting the allocator is safe.
        if allocator.Reset().is_err() || list.Reset(allocator, None).is_err() {
            return None;
        }

        // COLOR copy: COMMON -> COPY_SOURCE, copy subresource 0, back to COMMON.
        record_transition(
            list,
            &color_res,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        );
        let mut csrc = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(color_res.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: 0,
            },
        };
        let mut cdst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(cbuf.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: cfoot,
            },
        };
        list.CopyTextureRegion(&cdst, 0, 0, 0, &csrc, None);
        ManuallyDrop::drop(&mut csrc.pResource);
        ManuallyDrop::drop(&mut cdst.pResource);
        record_transition(
            list,
            &color_res,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_RESOURCE_STATE_COMMON,
        );

        // DEPTH copy: plane 0, same list -> same fence -> same GPU moment as the color copy.
        record_transition(
            list,
            &depth_res,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        );
        let mut dsrc = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(depth_res.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: 0,
            },
        };
        let mut ddst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(dbuf.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: dfoot,
            },
        };
        list.CopyTextureRegion(&ddst, 0, 0, 0, &dsrc, None);
        ManuallyDrop::drop(&mut dsrc.pResource);
        ManuallyDrop::drop(&mut ddst.pResource);
        record_transition(
            list,
            &depth_res,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_RESOURCE_STATE_COMMON,
        );

        if list.Close().is_err() {
            return None;
        }
        let base_list: ID3D12CommandList = list.cast().ok()?;
        queue.ExecuteCommandLists(&[Some(base_list)]);
        let val = RB_COH_FENCEVAL.fetch_add(1, Ordering::SeqCst) + 1;
        queue.Signal(fence, val).ok()?;
        if fence.GetCompletedValue() < val {
            let event: HANDLE = CreateEventW(None, false, false, None).ok()?;
            fence.SetEventOnCompletion(val, event).ok()?;
            let wait = WaitForSingleObject(event, READBACK_FENCE_WAIT_MS);
            let _ = CloseHandle(event);
            if wait != WAIT_OBJECT_0 {
                return None;
            }
        }

        // Map + de-swizzle COLOR (RGBA8, swap R/B for BGRA formats).
        let color = {
            let read_range = D3D12_RANGE {
                Begin: 0,
                End: ctotal as usize,
            };
            let mut mapped: *mut c_void = std::ptr::null_mut();
            cbuf.Map(0, Some(&read_range), Some(&mut mapped)).ok()?;
            if mapped.is_null() {
                return None;
            }
            let w = cw as usize;
            let h = ch as usize;
            let row_pitch = cfoot.Footprint.RowPitch as usize;
            let out_row = w * RGBA8_BPP;
            let total = ctotal as usize;
            let src = mapped as *const u8;
            let swap_rb = matches!(
                cformat,
                DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
            );
            let mut out = vec![0u8; w * h * RGBA8_BPP];
            for y in 0..h {
                let row_off = y * row_pitch;
                if row_off >= total {
                    break;
                }
                let avail = total - row_off;
                let copy_bytes = out_row.min(row_pitch).min(avail);
                let src_row = src.add(row_off);
                let dst_row = &mut out[y * out_row..y * out_row + copy_bytes];
                std::ptr::copy_nonoverlapping(src_row, dst_row.as_mut_ptr(), copy_bytes);
                if swap_rb {
                    let texels = copy_bytes / RGBA8_BPP;
                    for t in 0..texels {
                        dst_row.swap(t * RGBA8_BPP, t * RGBA8_BPP + 2);
                    }
                }
            }
            let write_range = D3D12_RANGE { Begin: 0, End: 0 };
            cbuf.Unmap(0, Some(&write_range));
            out
        };

        // Map + reinterpret DEPTH (plane 0, each 4-byte texel as f32).
        let depth = {
            let read_range = D3D12_RANGE {
                Begin: 0,
                End: dtotal as usize,
            };
            let mut mapped: *mut c_void = std::ptr::null_mut();
            dbuf.Map(0, Some(&read_range), Some(&mut mapped)).ok()?;
            if mapped.is_null() {
                return None;
            }
            let w = dw as usize;
            let h = dh as usize;
            let row_pitch = dfoot.Footprint.RowPitch as usize;
            let total = dtotal as usize;
            let src = mapped as *const u8;
            let mut out = vec![0f32; w * h];
            for y in 0..h {
                let row_off = y * row_pitch;
                if row_off + w * 4 > total {
                    break;
                }
                for x in 0..w {
                    let b = std::slice::from_raw_parts(src.add(row_off + x * 4), 4);
                    out[y * w + x] = f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                }
            }
            let write_range = D3D12_RANGE { Begin: 0, End: 0 };
            dbuf.Unmap(0, Some(&write_range));
            out
        };

        Some((cw, ch, color, color_cand, dw, dh, depth, depth_cand))
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
        let exclude_v = find_d3d12_resource_ex(exclude_start, 0, false, 0)
            .map(|(_, v)| v)
            .unwrap_or(0);
        let (resource, _) = find_d3d12_resource_ex(start, exclude_v, false, 0)?;
        readback_resource_rgba8_inner(resource)
    }))
    .ok()
    .flatten()
}

/// Cached AddRef'd content-RT `ID3D12Resource` raw pointer (0 = not yet resolved). Set once by the first
/// `readback_cached_content_rgba8` scan; re-copied every frame after without re-scanning.
pub(crate) static PROFILE_LIVE_RT_RES: AtomicUsize = AtomicUsize::new(0);
/// PER-FRAME readback resource cache (created ONCE on the game device, reused every frame). The original
/// per-call readback created a fresh command queue + allocator + list + fence + buffer EACH frame; under
/// the loading screen that mostly failed at resource creation (command-queue creation is limited), so the
/// readback published only ~4x and the displayed head froze. Caching them lets the per-frame readback be a
/// cheap reset+copy+wait, so it publishes every frame and the overlay re-uploads the tracking head per
/// frame. Raw COM pointers owned by these statics (released only on dims change).
static RB_FAST_QUEUE: AtomicUsize = AtomicUsize::new(0);
static RB_FAST_ALLOC: AtomicUsize = AtomicUsize::new(0);
static RB_FAST_LIST: AtomicUsize = AtomicUsize::new(0);
static RB_FAST_FENCE: AtomicUsize = AtomicUsize::new(0);
static RB_FAST_BUFFER: AtomicUsize = AtomicUsize::new(0);
static RB_FAST_BUFSIZE: AtomicU64 = AtomicU64::new(0);
static RB_FAST_FENCEVAL: AtomicU64 = AtomicU64::new(0);
/// Counter so the deterministic-resolve diagnostic logs only the first few attempts.
static PROFILE_DET_RESOLVE_DIAG: AtomicUsize = AtomicUsize::new(0);

/// PER-FRAME DEPTH readback cache (separate from RB_FAST_* so the color and depth readbacks never share a
/// command list / fence across the two calls in one draw tick). Same create-once + reset+copy+wait pattern.
/// The depth sibling is `R32G8X24_TYPELESS` (fmt 19); we copy PLANE 0 (the R32 float depth) and reinterpret
/// each 4-byte texel as `f32`. Raw COM pointers owned by these statics (released only on footprint change).
static RB_DEPTH_QUEUE: AtomicUsize = AtomicUsize::new(0);
static RB_DEPTH_ALLOC: AtomicUsize = AtomicUsize::new(0);
static RB_DEPTH_LIST: AtomicUsize = AtomicUsize::new(0);
static RB_DEPTH_FENCE: AtomicUsize = AtomicUsize::new(0);
static RB_DEPTH_BUFFER: AtomicUsize = AtomicUsize::new(0);
static RB_DEPTH_BUFSIZE: AtomicU64 = AtomicU64::new(0);
static RB_DEPTH_FENCEVAL: AtomicU64 = AtomicU64::new(0);
/// One-shot `depth-key` diagnostic latch (logs corner/center/min/max depth + masked fraction once).
static DEPTH_KEY_DIAG_LOGGED: AtomicUsize = AtomicUsize::new(0);
/// RAM oracle: number of published frames where the depth key ACTUALLY cut out a background (i.e. the depth
/// buffer read back with clean bg/head separation and `>0` pixels were set to alpha 0). `oracle_depth_key_
/// applied` -- a pixel/native semaphore that the transparent-background cutout is live (not a screenshot).
pub(crate) static DEPTH_KEY_APPLIED: AtomicUsize = AtomicUsize::new(0);
/// RAM oracle: last frame's background-masked fraction, in whole percent (0..=100). `oracle_depth_key_bg_pct`.
/// A plausible portrait cutout is a large minority/majority of the frame (bg dominates a centered head).
pub(crate) static DEPTH_KEY_BG_PCT: AtomicUsize = AtomicUsize::new(0);
/// RAM oracle: number of frames the mask was RECALCULATED fresh from a valid depth buffer (vs reused from
/// cache). `oracle_depth_key_fresh`; `applied - fresh` = cached reuses. Proves the recalc-and-cache loop.
pub(crate) static DEPTH_KEY_FRESH: AtomicUsize = AtomicUsize::new(0);
/// One-shot latch for the no-gap / dims-mismatch `depth-key` skip diagnostic (separate from the success
/// latch so both a good frame and a skipped frame are each visible once in the log).
static DEPTH_KEY_NOGAP_LOGGED: AtomicUsize = AtomicUsize::new(0);
/// Last RECALCULATED depth-key mask (w, h, per-pixel: 1 = background/cut, 0 = keep). The offscreen depth
/// buffer only carries real content on genuine re-render frames; on the many frames it reads back cleared,
/// we re-apply this cached mask so the cutout stays stable. It is RECALCULATED whenever fresh depth is
/// available (tracking a real re-render) and only cached for the dead frames in between -- never frozen.
static LAST_DEPTH_MASK: Mutex<Option<(usize, usize, Vec<u8>)>> = Mutex::new(None);
/// Current portrait character incarnation (drive slot + 1; 0 = unset), set by the per-frame drive so the
/// mask cache can be tagged with the character it was computed for. A depth mask REUSED across a change
/// of this value means the PREVIOUS character's silhouette is being applied to the NEW character's head
/// -- the 2nd-character depth-mask desync (user 2026-07-03). See `PROFILE_MASK_STALE_REUSE`.
pub(crate) static PROFILE_PORTRAIT_INCARNATION: AtomicUsize = AtomicUsize::new(0);
/// Incarnation the currently-cached `LAST_DEPTH_MASK` was computed for (0 = none / cleared).
static LAST_DEPTH_MASK_INCARNATION: AtomicUsize = AtomicUsize::new(0);
/// FAIL-FAST desync semaphore: count of frames that REUSED the cached depth mask while the live portrait
/// incarnation differs from the one the cache was computed for -- a prior character's mask on the new
/// head. It trips early + deterministically (the 2nd character of a switch chain), so a run can stop in
/// ~40s instead of six minutes. Exposed as `oracle_portrait_mask_stale_reuse`.
pub(crate) static PROFILE_MASK_STALE_REUSE: AtomicUsize = AtomicUsize::new(0);
static PROFILE_MASK_STALE_REUSE_LOGGED: AtomicUsize = AtomicUsize::new(0);
/// FAIL-FAST mask/head coherence semaphore (the 2nd-character desync is a FRESH-but-WRONG mask: masks are
/// recomputed every frame, so it is not a cache reuse). Per published frame, IoU of the KEPT cutout region
/// (mask==0) vs the colour's OWN head (pixels far from the background colour). A correct mask keeps the
/// head -> high IoU; a fresh mask of a WRONG depth silhouette (stale depth content on the new character)
/// keeps a region that does not match this head -> low IoU. `_last` is an oracle; `_total` counts gross
/// mismatches; a SUSTAINED gross mismatch (STREAK) abort()s during the repro so the run stops fast.
pub(crate) const MASK_HEAD_IOU_MIN: usize = 25;
const MASK_HEAD_ABORT_STREAK: usize = 20;
pub(crate) static PROFILE_MASK_HEAD_IOU_LAST: AtomicUsize = AtomicUsize::new(100);
static PROFILE_MASK_HEAD_MISMATCH_STREAK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_MASK_HEAD_MISMATCH_TOTAL: AtomicUsize = AtomicUsize::new(0);

/// COHERENT color+depth readback cache (bug #3 fix). ONE queue/allocator/list/fence records BOTH the
/// color and depth copies, so they are captured at the SAME GPU submission -- unlike the separate
/// RB_FAST_* (color) and RB_DEPTH_* (depth) paths, between whose independent fences the game's async
/// render can advance the RT (color=frameN, depth=frameN+1 -> the mask shape mismatches the head).
/// Separate readback buffers for color and depth (resized on footprint change). Raw COM owned here.
static RB_COH_QUEUE: AtomicUsize = AtomicUsize::new(0);
static RB_COH_ALLOC: AtomicUsize = AtomicUsize::new(0);
static RB_COH_LIST: AtomicUsize = AtomicUsize::new(0);
static RB_COH_FENCE: AtomicUsize = AtomicUsize::new(0);
static RB_COH_CBUF: AtomicUsize = AtomicUsize::new(0);
static RB_COH_CBUFSIZE: AtomicU64 = AtomicU64::new(0);
static RB_COH_DBUF: AtomicUsize = AtomicUsize::new(0);
static RB_COH_DBUFSIZE: AtomicU64 = AtomicU64::new(0);
static RB_COH_FENCEVAL: AtomicU64 = AtomicU64::new(0);
/// Depth captured COHERENTLY with the current color frame `(dw, dh, depth, depth_cand)`, stashed by
/// `readback_offscreen_fast_coherent` for the SAME draw tick's `apply_depth_alpha_key` to consume via
/// `take_coherent_depth`. Single render-thread producer/consumer within one tick; the producer always
/// sets it (coherent success) or clears it (fallback) each frame, so a later frame never reads a stale
/// depth. `None` -> the mask path reads depth fresh (the legacy separate read).
static COHERENT_DEPTH: Mutex<Option<(u32, u32, Vec<f32>, usize)>> = Mutex::new(None);
/// Instrumentation the first coherent pass lacked: how many draw ticks the COHERENT color+depth readback
/// SUCCEEDED (`_OK`) vs fell back to the separate color+depth path (`_FALLBACK`). Exposed as oracles so a
/// run PROVES whether the single-fence path is actually engaging (not silently degrading).
pub(crate) static COHERENT_READ_OK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static COHERENT_READ_FALLBACK: AtomicUsize = AtomicUsize::new(0);

/// Cached backbuffer READBACK + UPLOAD buffers for the alpha-honoring CPU-blend composite (sized to the
/// centered portrait region's copyable footprint in the backbuffer's format). The composite reads the live
/// backbuffer region, blends the portrait over it honoring per-pixel alpha (bg alpha 0 => loading screen
/// shows through), and writes the blended region back -- all with the existing COPY primitives, so NO new
/// PSO/shader/RTV pipeline is needed. Owned raw COM pointers (released only on footprint change).
static OVERLAY_BB_READBACK: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_BB_UPLOAD: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_BB_BUFSIZE: AtomicU64 = AtomicU64::new(0);

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
            // Cached-resource readback: reuse RB_FAST_* objects so this succeeds EVERY frame (the per-call
            // version created a new queue each frame and mostly failed -> published only ~4x).
            return readback_resource_cached_fast(res.clone());
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
        readback_resource_cached_fast(resource)
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

/// Per-frame readback of `resource` reusing the CACHED `RB_FAST_*` queue/allocator/list/fence/buffer
/// (created once on the game device). Identical copy+wait+de-swizzle to `readback_resource_rgba8_inner`
/// but it does NOT create new D3D12 objects each call -- which is why the per-call version published only
/// ~4x under loading (command-queue creation kept failing). With the cache the readback succeeds every
/// frame so the displayed head follows the cursor. Returns `None` on any failure (draws the last frame).
unsafe fn readback_resource_cached_fast(resource: ID3D12Resource) -> Option<(u32, u32, Vec<u8>)> {
    let mut device_opt: Option<ID3D12Device> = None;
    unsafe { resource.GetDevice(&mut device_opt) }.ok()?;
    let device = device_opt?;
    let desc: D3D12_RESOURCE_DESC = unsafe { resource.GetDesc() };
    let width = desc.Width as u32;
    let height = desc.Height;
    let format = desc.Format;
    LOADING_BG_PORTRAIT_FORMAT.store(format.0 as usize, Ordering::SeqCst);
    if width == 0 || height == 0 || width > MAX_RT_DIM || height > MAX_RT_DIM {
        return None;
    }
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
    // Create the cached queue/allocator/list/fence ONCE (the list is left Closed so the first Reset works).
    if RB_FAST_QUEUE.load(Ordering::SeqCst) == 0 {
        let queue_desc = D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            Priority: 0,
            Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
            NodeMask: 0,
        };
        let queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&queue_desc) }.ok()?;
        let allocator: ID3D12CommandAllocator =
            unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }.ok()?;
        let list: ID3D12GraphicsCommandList = unsafe {
            device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
        }
        .ok()?;
        unsafe { list.Close() }.ok()?;
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.ok()?;
        RB_FAST_QUEUE.store(queue.into_raw() as usize, Ordering::SeqCst);
        RB_FAST_ALLOC.store(allocator.into_raw() as usize, Ordering::SeqCst);
        RB_FAST_LIST.store(list.into_raw() as usize, Ordering::SeqCst);
        RB_FAST_FENCE.store(fence.into_raw() as usize, Ordering::SeqCst);
    }
    // (Re)create the cached readback buffer if the footprint size changed (it won't for a fixed RT).
    if RB_FAST_BUFSIZE.load(Ordering::SeqCst) != total_bytes {
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
        let buf = readback_opt?;
        let old = RB_FAST_BUFFER.swap(buf.into_raw() as usize, Ordering::SeqCst);
        if old != 0 {
            drop(unsafe { ID3D12Resource::from_raw(old as *mut c_void) });
        }
        RB_FAST_BUFSIZE.store(total_bytes, Ordering::SeqCst);
    }
    // Borrow the cached COM objects (no refcount change; the statics own them).
    let q_raw = RB_FAST_QUEUE.load(Ordering::SeqCst) as *mut c_void;
    let a_raw = RB_FAST_ALLOC.load(Ordering::SeqCst) as *mut c_void;
    let l_raw = RB_FAST_LIST.load(Ordering::SeqCst) as *mut c_void;
    let f_raw = RB_FAST_FENCE.load(Ordering::SeqCst) as *mut c_void;
    let b_raw = RB_FAST_BUFFER.load(Ordering::SeqCst) as *mut c_void;
    let (Some(queue), Some(allocator), Some(list), Some(fence), Some(readback)) = (unsafe {
        (
            ID3D12CommandQueue::from_raw_borrowed(&q_raw),
            ID3D12CommandAllocator::from_raw_borrowed(&a_raw),
            ID3D12GraphicsCommandList::from_raw_borrowed(&l_raw),
            ID3D12Fence::from_raw_borrowed(&f_raw),
            ID3D12Resource::from_raw_borrowed(&b_raw),
        )
    }) else {
        return None;
    };
    // The previous frame's fence wait guarantees the GPU is done, so resetting the allocator is safe.
    if unsafe { allocator.Reset() }.is_err() {
        return None;
    }
    if unsafe { list.Reset(allocator, None) }.is_err() {
        return None;
    }
    unsafe {
        record_transition(
            list,
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
    unsafe { ManuallyDrop::drop(&mut src_location.pResource) };
    unsafe { ManuallyDrop::drop(&mut dst_location.pResource) };
    unsafe {
        record_transition(
            list,
            &resource,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_RESOURCE_STATE_COMMON,
        )
    };
    if unsafe { list.Close() }.is_err() {
        return None;
    }
    let base_list: ID3D12CommandList = list.cast().ok()?;
    unsafe { queue.ExecuteCommandLists(&[Some(base_list)]) };
    let val = RB_FAST_FENCEVAL.fetch_add(1, Ordering::SeqCst) + 1;
    unsafe { queue.Signal(fence, val) }.ok()?;
    if unsafe { fence.GetCompletedValue() } < val {
        let event: HANDLE = unsafe { CreateEventW(None, false, false, None) }.ok()?;
        unsafe { fence.SetEventOnCompletion(val, event) }.ok()?;
        let wait = unsafe { WaitForSingleObject(event, READBACK_FENCE_WAIT_MS) };
        let _ = unsafe { CloseHandle(event) };
        if wait != WAIT_OBJECT_0 {
            return None;
        }
    }
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
        if row_off >= total {
            break;
        }
        let avail = total - row_off;
        let copy_bytes = out_row.min(row_pitch).min(avail);
        let src_row = unsafe { src.add(row_off) };
        let dst_row = &mut out[y * out_row..y * out_row + copy_bytes];
        unsafe { std::ptr::copy_nonoverlapping(src_row, dst_row.as_mut_ptr(), copy_bytes) };
        if swap_rb {
            let texels = copy_bytes / RGBA8_BPP;
            for t in 0..texels {
                dst_row.swap(t * RGBA8_BPP, t * RGBA8_BPP + 2);
            }
        }
    }
    let write_range = D3D12_RANGE { Begin: 0, End: 0 };
    unsafe { readback.Unmap(0, Some(&write_range)) };
    Some((width, height, out))
}

/// Per-frame DEPTH readback of the offscreen scene's depth-stencil sibling (the `R32G8X24_TYPELESS`
/// buffer next to the color RT in the same wrapper nest). Returns `(width, height, depth_f32)` where each
/// element is the plane-0 R32 float depth. `None` on any failure (the caller then leaves the color buffer
/// fully opaque -- fail-open, no cutout). Same catch_unwind + never-touch-the-game contract as the color
/// readback. Used by `apply_depth_alpha_key` to derive the transparent-background alpha mask.
pub(crate) unsafe fn readback_depth_fast(gpu_child: usize) -> Option<(u32, u32, Vec<f32>, usize)> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let (resource, cand) = find_depth_resource(gpu_child)?;
        readback_depth_resource_cached(resource).map(|(w, h, d)| (w, h, d, cand))
    }))
    .ok()
    .flatten()
}

/// Depth twin of `readback_resource_cached_fast`: reset+copy+wait using the dedicated `RB_DEPTH_*` cached
/// objects, but it copies PLANE 0 (subresource 0 = the R32 float depth of the `R32G8X24_TYPELESS` buffer)
/// and reinterprets each 4-byte texel as `f32`. No RB swap / no LOADING_BG_PORTRAIT_FORMAT write (that is
/// the color format telemetry). The `GetCopyableFootprints(&desc, 0, 1, ..)` call yields the plane-0
/// footprint directly, so the copy is plane-correct without hand-computing plane sizes.
unsafe fn readback_depth_resource_cached(resource: ID3D12Resource) -> Option<(u32, u32, Vec<f32>)> {
    let mut device_opt: Option<ID3D12Device> = None;
    unsafe { resource.GetDevice(&mut device_opt) }.ok()?;
    let device = device_opt?;
    let desc: D3D12_RESOURCE_DESC = unsafe { resource.GetDesc() };
    let width = desc.Width as u32;
    let height = desc.Height;
    if width == 0 || height == 0 || width > MAX_RT_DIM || height > MAX_RT_DIM {
        return None;
    }
    // Plane 0 (subresource 0) copyable footprint -- the R32 depth plane, 4 bytes/texel, 256-aligned rows.
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
    if total_bytes == 0 || footprint.Footprint.RowPitch == 0 {
        return None;
    }
    if RB_DEPTH_QUEUE.load(Ordering::SeqCst) == 0 {
        let queue_desc = D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            Priority: 0,
            Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
            NodeMask: 0,
        };
        let queue: ID3D12CommandQueue = unsafe { device.CreateCommandQueue(&queue_desc) }.ok()?;
        let allocator: ID3D12CommandAllocator =
            unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }.ok()?;
        let list: ID3D12GraphicsCommandList = unsafe {
            device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
        }
        .ok()?;
        unsafe { list.Close() }.ok()?;
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.ok()?;
        RB_DEPTH_QUEUE.store(queue.into_raw() as usize, Ordering::SeqCst);
        RB_DEPTH_ALLOC.store(allocator.into_raw() as usize, Ordering::SeqCst);
        RB_DEPTH_LIST.store(list.into_raw() as usize, Ordering::SeqCst);
        RB_DEPTH_FENCE.store(fence.into_raw() as usize, Ordering::SeqCst);
    }
    if RB_DEPTH_BUFSIZE.load(Ordering::SeqCst) != total_bytes {
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
        let buf = readback_opt?;
        let old = RB_DEPTH_BUFFER.swap(buf.into_raw() as usize, Ordering::SeqCst);
        if old != 0 {
            drop(unsafe { ID3D12Resource::from_raw(old as *mut c_void) });
        }
        RB_DEPTH_BUFSIZE.store(total_bytes, Ordering::SeqCst);
    }
    let q_raw = RB_DEPTH_QUEUE.load(Ordering::SeqCst) as *mut c_void;
    let a_raw = RB_DEPTH_ALLOC.load(Ordering::SeqCst) as *mut c_void;
    let l_raw = RB_DEPTH_LIST.load(Ordering::SeqCst) as *mut c_void;
    let f_raw = RB_DEPTH_FENCE.load(Ordering::SeqCst) as *mut c_void;
    let b_raw = RB_DEPTH_BUFFER.load(Ordering::SeqCst) as *mut c_void;
    let (Some(queue), Some(allocator), Some(list), Some(fence), Some(readback)) = (unsafe {
        (
            ID3D12CommandQueue::from_raw_borrowed(&q_raw),
            ID3D12CommandAllocator::from_raw_borrowed(&a_raw),
            ID3D12GraphicsCommandList::from_raw_borrowed(&l_raw),
            ID3D12Fence::from_raw_borrowed(&f_raw),
            ID3D12Resource::from_raw_borrowed(&b_raw),
        )
    }) else {
        return None;
    };
    if unsafe { allocator.Reset() }.is_err() {
        return None;
    }
    if unsafe { list.Reset(allocator, None) }.is_err() {
        return None;
    }
    unsafe {
        record_transition(
            list,
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
    unsafe { ManuallyDrop::drop(&mut src_location.pResource) };
    unsafe { ManuallyDrop::drop(&mut dst_location.pResource) };
    unsafe {
        record_transition(
            list,
            &resource,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_RESOURCE_STATE_COMMON,
        )
    };
    if unsafe { list.Close() }.is_err() {
        return None;
    }
    let base_list: ID3D12CommandList = list.cast().ok()?;
    unsafe { queue.ExecuteCommandLists(&[Some(base_list)]) };
    let val = RB_DEPTH_FENCEVAL.fetch_add(1, Ordering::SeqCst) + 1;
    unsafe { queue.Signal(fence, val) }.ok()?;
    if unsafe { fence.GetCompletedValue() } < val {
        let event: HANDLE = unsafe { CreateEventW(None, false, false, None) }.ok()?;
        unsafe { fence.SetEventOnCompletion(val, event) }.ok()?;
        let wait = unsafe { WaitForSingleObject(event, READBACK_FENCE_WAIT_MS) };
        let _ = unsafe { CloseHandle(event) };
        if wait != WAIT_OBJECT_0 {
            return None;
        }
    }
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
    let total = total_bytes as usize;
    let src = mapped as *const u8;
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        let row_off = y * row_pitch;
        if row_off + w * 4 > total {
            break;
        }
        for x in 0..w {
            let b = unsafe { std::slice::from_raw_parts(src.add(row_off + x * 4), 4) };
            out[y * w + x] = f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        }
    }
    let write_range = D3D12_RANGE { Begin: 0, End: 0 };
    unsafe { readback.Unmap(0, Some(&write_range)) };
    Some((width, height, out))
}

/// DEPTH-KEYED TRANSPARENT BACKGROUND: read back the offscreen scene's depth plane and set the color
/// buffer's per-pixel alpha to 0 for every BACKGROUND pixel. The portrait depth is BIMODAL -- the model
/// (head+shoulders) forms one cluster and the dark IBL surround another, separated by an empty depth GAP
/// (the "air" between the model and the backdrop). We histogram the depth, put the threshold in the WIDEST
/// empty gap, and KEEP whichever cluster the centered head sits in, cutting the other. Keying on the head's
/// own cluster (not an assumed clear value) makes this robust to the engine's Z direction and to the fact
/// that the corners are NOT all background (a portrait's shoulders fill the bottom corners). FAIL-OPEN on
/// every uncertainty (no depth buffer, dims mismatch, no separable gap) -> the color buffer is left fully
/// opaque, exactly the pre-existing display, so this can only ADD the cutout, never regress the head. Emits
/// a one-shot `depth-key` diagnostic and drives the `oracle_depth_key_*` RAM semaphores. `cpx` is the
/// tightly-packed RGBA8 the caller is about to publish (mutated in place).
pub(crate) unsafe fn apply_depth_alpha_key(gpu_child: usize, w: u32, h: u32, cpx: &mut [u8]) {
    let w = w as usize;
    let h = h as usize;
    if cpx.len() < w * h * 4 {
        return;
    }
    // (1) RECALCULATE the mask from the CURRENT depth buffer whenever it carries real content.
    let fresh = unsafe { compute_depth_mask(gpu_child, w, h) };
    // (2) On a fresh mask, CACHE it; on a dead frame (depth read back cleared -> no gap), REUSE the last
    //     cached mask so the cutout stays stable. Recalculated whenever fresh depth is available (tracks a
    //     genuine re-render), cached only for the frames in between -- never a frozen one-shot.
    let mask = if let Some(m) = fresh {
        DEPTH_KEY_FRESH.fetch_add(1, Ordering::SeqCst);
        // Tag the cache with the character it was computed for, so a later cross-character reuse is
        // detectable (the stale-reuse desync semaphore below).
        LAST_DEPTH_MASK_INCARNATION.store(
            PROFILE_PORTRAIT_INCARNATION.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        if let Ok(mut g) = LAST_DEPTH_MASK.lock() {
            *g = Some((w, h, m.clone()));
        }
        Some(m)
    } else {
        let reused = LAST_DEPTH_MASK.lock().ok().and_then(|g| match g.as_ref() {
            Some((cw, ch, m)) if *cw == w && *ch == h => Some(m.clone()),
            _ => None,
        });
        if reused.is_some() {
            // FAIL-FAST desync semaphore: this depth was dead (no fresh gap) so we are reusing the cached
            // mask -- but if it was computed for a DIFFERENT character incarnation than the one rendering
            // now, its silhouette will not match this head (the 2nd-character depth-mask desync). Detect
            // it as a run-stopping RAM oracle rather than only seeing it on screen.
            let cur = PROFILE_PORTRAIT_INCARNATION.load(Ordering::SeqCst);
            let cached = LAST_DEPTH_MASK_INCARNATION.load(Ordering::SeqCst);
            if cur != 0 && cached != 0 && cur != cached {
                let n = PROFILE_MASK_STALE_REUSE.fetch_add(1, Ordering::SeqCst);
                if PROFILE_MASK_STALE_REUSE_LOGGED.swap(1, Ordering::SeqCst) == 0 {
                    append_autoload_debug(format_args!(
                        "MASK-STALE-REUSE-DESYNC: reusing depth mask from portrait incarnation {cached} on incarnation {cur} (prior character's silhouette on the new head) -- #{}",
                        n + 1
                    ));
                    // HARD FAIL-FAST during the diagnostic repro ONLY: abort the process the instant the
                    // desync is detected so the run stops in ~40s instead of six minutes. A fast crash ==
                    // the semaphore caught the stale-reuse; no crash == the desync is NOT stale-reuse (a
                    // different mechanism) and the hypothesis is refuted. Never fires in product (gated on
                    // the System-Quit repro being active), so it cannot crash a real player session.
                    if system_quit_repro_enabled() {
                        append_autoload_debug(format_args!(
                            "MASK-STALE-REUSE-DESYNC: FAIL-FAST abort() (repro diagnostic stop)"
                        ));
                        std::process::abort();
                    }
                }
            }
        }
        reused
    };
    // No fresh gap and no cache yet -> fail open (opaque), no regression.
    let Some(mask) = mask else {
        return;
    };
    let mut masked = 0usize;
    for i in 0..(w * h) {
        if mask[i] != 0 {
            cpx[i * 4 + 3] = 0;
            masked += 1;
        }
    }
    if masked > 0 {
        DEPTH_KEY_APPLIED.fetch_add(1, Ordering::SeqCst);
        DEPTH_KEY_BG_PCT.store(masked * 100 / (w * h), Ordering::SeqCst);
        // FAIL-FAST mask/head coherence (2nd-character desync): does the KEPT cutout match THIS head?
        let iou = mask_head_iou(&mask, cpx, w, h);
        PROFILE_MASK_HEAD_IOU_LAST.store(iou, Ordering::SeqCst);
        if iou < MASK_HEAD_IOU_MIN {
            let streak = PROFILE_MASK_HEAD_MISMATCH_STREAK.fetch_add(1, Ordering::SeqCst) + 1;
            PROFILE_MASK_HEAD_MISMATCH_TOTAL.fetch_add(1, Ordering::SeqCst);
            if streak == 1 || streak % 16 == 0 {
                append_autoload_debug(format_args!(
                    "MASK-HEAD-MISMATCH: IoU={iou}% < {MASK_HEAD_IOU_MIN}% (kept cutout vs colour head) streak={streak} -- fresh-but-wrong depth silhouette on this head"
                ));
            }
            // Abort only on a SUSTAINED gross mismatch (a whole loading screen desyncs; a transient
            // build glitch is a few frames), and only during the repro (never a real player session).
            if streak >= MASK_HEAD_ABORT_STREAK && system_quit_repro_enabled() {
                append_autoload_debug(format_args!(
                    "MASK-HEAD-MISMATCH: FAIL-FAST abort() after {streak} sustained mismatch frames (IoU={iou}%) -- repro diagnostic stop"
                ));
                std::process::abort();
            }
        } else {
            PROFILE_MASK_HEAD_MISMATCH_STREAK.store(0, Ordering::SeqCst);
        }
    }
}

/// IoU (0..100) of the depth cutout's KEPT region (mask==0) vs the colour's OWN head (pixels whose colour
/// is clearly far from the corner background). ~high when the mask matches the head; low when a fresh mask
/// of the WRONG depth silhouette is applied to this head (the 2nd-character desync). Subsampled by 2 for
/// cost; 100 (perfect) on any degenerate input so it never false-trips.
fn mask_head_iou(mask: &[u8], cpx: &[u8], w: usize, h: usize) -> usize {
    if w < 16 || h < 16 || cpx.len() < w * h * 4 || mask.len() < w * h {
        return 100;
    }
    let (mut br, mut bgc, mut bb, mut bn) = (0u64, 0u64, 0u64, 0u64);
    for &(ox, oy) in &[(0usize, 0usize), (w - 8, 0), (0, h - 8), (w - 8, h - 8)] {
        for yy in oy..oy + 8 {
            for xx in ox..ox + 8 {
                let p = (yy * w + xx) * 4;
                br += cpx[p] as u64;
                bgc += cpx[p + 1] as u64;
                bb += cpx[p + 2] as u64;
                bn += 1;
            }
        }
    }
    if bn == 0 {
        return 100;
    }
    let (br, bgc, bb) = ((br / bn) as i32, (bgc / bn) as i32, (bb / bn) as i32);
    // Foreground = clearly far from the background colour (sum-abs channel distance, 0..765).
    const FG_DIST: i32 = 90;
    let (mut inter, mut union) = (0u64, 0u64);
    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let i = y * w + x;
            let p = i * 4;
            let kept = mask[i] == 0; // mask==1 => background/cut; ==0 => keep (the cutout foreground)
            let dist = (cpx[p] as i32 - br).abs()
                + (cpx[p + 1] as i32 - bgc).abs()
                + (cpx[p + 2] as i32 - bb).abs();
            let head = dist > FG_DIST;
            if kept || head {
                union += 1;
                if kept && head {
                    inter += 1;
                }
            }
            x += 2;
        }
        y += 1;
    }
    if union == 0 {
        100
    } else {
        (inter * 100 / union) as usize
    }
}

/// Read back the offscreen depth plane and compute the per-pixel background mask (1 = background/cut, 0 =
/// keep) via the bimodal-gap threshold. Returns `None` when the depth buffer has no content this frame or
/// no separable gap (the caller then reuses the cached mask). Emits the one-shot `depth-key` diagnostic
/// (success) and a separate one-shot skip diagnostic (dims mismatch / no gap) so both are visible once.
unsafe fn compute_depth_mask(gpu_child: usize, w: usize, h: usize) -> Option<Vec<u8>> {
    // Prefer the depth captured COHERENTLY with this tick's color (same single fence -- bug #3 fix);
    // fall back to a fresh, separately-fenced depth read when the coherent path was unavailable.
    let (dw, dh, depth, depth_cand) = match take_coherent_depth() {
        Some(d) => d,
        None => unsafe { readback_depth_fast(gpu_child) }?,
    };
    if dw as usize != w || dh as usize != h || depth.len() < w * h {
        // At higher-res (1024) the depth sibling MUST match the color RT or the mask can't align.
        if DEPTH_KEY_NOGAP_LOGGED.swap(1, Ordering::SeqCst) == 0 {
            append_autoload_debug(format_args!(
                "depth-key: SKIP dims-mismatch color={w}x{h} depth={dw}x{dh} depthlen={}",
                depth.len()
            ));
        }
        return None;
    }
    let idx = |x: usize, y: usize| y * w + x;
    // Frame depth extent over finite values.
    let mut dmin = f32::INFINITY;
    let mut dmax = f32::NEG_INFINITY;
    for &d in depth.iter().take(w * h) {
        if d.is_finite() {
            dmin = dmin.min(d);
            dmax = dmax.max(d);
        }
    }
    let range = dmax - dmin;

    // FOREGROUND reference = the CENTERED head's depth: sample a small central patch and take its median.
    // We KEEP the cluster this belongs to and CUT the other -- so the cut is robust to the engine's Z
    // direction (we never assume near/far == 0). The portrait's head+shoulders are the high-count
    // foreground cluster; the dark IBL surround is the other cluster, separated by an empty depth GAP
    // (the "air" between the model and the backdrop).
    let cr = (w.min(h) / 16).max(2);
    let (cx, cy) = (w / 2, h / 2);
    let mut cpatch: Vec<f32> = Vec::new();
    let mut yy = cy.saturating_sub(cr);
    while yy <= (cy + cr).min(h.saturating_sub(1)) {
        let mut xx = cx.saturating_sub(cr);
        while xx <= (cx + cr).min(w.saturating_sub(1)) {
            let d = depth[idx(xx, yy)];
            if d.is_finite() {
                cpatch.push(d);
            }
            xx += 1;
        }
        yy += 1;
    }
    cpatch.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let center = if cpatch.is_empty() {
        f32::NAN
    } else {
        cpatch[cpatch.len() / 2]
    };

    // Histogram over [dmin,dmax]; the threshold is the midpoint of the WIDEST run of near-empty bins --
    // i.e. the gap between the background cluster and the foreground cluster.
    const NB: usize = 128;
    let mut hist = [0u32; NB];
    if range > 0.0 {
        for &d in depth.iter().take(w * h) {
            if d.is_finite() {
                let bi = ((((d - dmin) / range) * NB as f32) as isize).clamp(0, NB as isize - 1);
                hist[bi as usize] += 1;
            }
        }
    }
    // A bin is "empty" if it holds < ~0.05% of the frame (tolerates a few depth-edge stragglers).
    let empty_thresh = ((w * h) / 2000).max(1) as u32;
    let (mut best_lo, mut best_len) = (0usize, 0usize);
    let (mut cur_lo, mut cur_len) = (0usize, 0usize);
    for b in 0..NB {
        if hist[b] <= empty_thresh {
            if cur_len == 0 {
                cur_lo = b;
            }
            cur_len += 1;
            if cur_len > best_len {
                best_len = cur_len;
                best_lo = cur_lo;
            }
        } else {
            cur_len = 0;
        }
    }
    let gap_mid_bin = best_lo as f32 + best_len as f32 * 0.5;
    let threshold = dmin + (gap_mid_bin / NB as f32) * range;
    // Require a REAL gap (>= ~4% of the range empty) and a valid center, else no separable background
    // this frame -> return None (caller reuses the cached mask; a unimodal depth would slice the head).
    let have_gap = range > 0.0 && (best_len as f32 / NB as f32) >= 0.04 && center.is_finite();
    // Keep the side of the gap the centered head sits on; the other side is the background.
    let keep_high = center > threshold;
    let mut mask = vec![0u8; w * h];
    let mut masked = 0usize;
    if have_gap {
        for i in 0..(w * h) {
            let d = depth[i];
            let is_bg = if keep_high {
                d < threshold
            } else {
                d > threshold
            };
            if is_bg {
                mask[i] = 1;
                masked += 1;
            }
        }
    }
    if DEPTH_KEY_DIAG_LOGGED.swap(1, Ordering::SeqCst) == 0 {
        let inset = (w.min(h) / 32).max(2);
        let (tl, tr, bl, br) = (
            depth[idx(inset, inset)],
            depth[idx(w - 1 - inset, inset)],
            depth[idx(inset, h - 1 - inset)],
            depth[idx(w - 1 - inset, h - 1 - inset)],
        );
        append_autoload_debug(format_args!(
            "depth-key: {w}x{h} min={dmin} max={dmax} center(head)={center} corners[tl={tl} tr={tr} bl={bl} br={br}] gap[bins {best_lo}..+{best_len}/{NB}] thr={threshold} keep_high={keep_high} have_gap={have_gap} masked={masked}/{} ({}%)",
            w * h,
            masked * 100 / (w * h).max(1)
        ));
    }
    if have_gap && masked > 0 {
        // A clean bimodal bg/head separation confirms this depth buffer belongs to OUR portrait scene --
        // pin its candidate so later scans can't drift to another slot's same-size depth sibling.
        PROFILE_DEPTH_PIN.store(depth_cand, Ordering::SeqCst);
        Some(mask)
    } else {
        None
    }
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
    let Some((dst, dst_v)) = (unsafe { find_d3d12_resource_ex(dst_gpu_child, 0, false, 0) }) else {
        return false;
    };
    // Prefer the pinned content RT for the source too, so the SRV the native forge samples is fed from
    // the same confirmed-ours texture the display publishes (never a foreign slot's RT).
    let rt_pin = PROFILE_RT_PIN.load(Ordering::SeqCst);
    let Some((src, _src_v)) =
        (unsafe { find_d3d12_resource_ex(src_gpu_child, dst_v, false, rt_pin) })
    else {
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
///
/// RETAINED FOR REFERENCE: the alpha-honoring composite now blends the portrait onto the backbuffer on the
/// CPU (see `blend_portrait_over_backbuffer`), so this GPU upload path is currently unused. Kept as the
/// proven RGBA->DEFAULT-heap upload for a future GPU-draw composite.
#[allow(dead_code)]
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

    // NOTE: no GPU source texture is created here anymore -- the composite blends the portrait onto the
    // backbuffer on the CPU (readback region -> alpha-blend -> writeback), sourcing the pixels directly
    // from `LOADING_BG_PORTRAIT_RGBA` each frame. That is what lets the transparent (alpha-0) background
    // show the loading screen through; a raw CopyTextureRegion could not honor per-pixel alpha.
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

/// Close the loading-portrait window: clear the published snapshot + the "have a head" gate so a later
/// window cannot flash the PREVIOUS character, drop the RT/depth candidate pins (the next window's
/// renderers are new objects), and clear the teardown-spared renderer so the NEXT load's teardown re-spares
/// the new character (LOADING_BG_PORTRAIT_SPARED_RENDERER is gated `== 0` and was otherwise never reset --
/// it stayed pinned to the first character's now-stale renderer, and driving that leaked renderer risks a
/// use-after-free). Called from the overlay stop at load completion; idempotent.
pub(crate) fn loading_portrait_window_reset(reason: &str) {
    // Make-before-break bridge (user 2026-07-03): KEEP the last published keyed frame + its
    // display-available flag so the just-loaded character stays on screen as the bridge when the NEXT
    // load window opens -- it is replaced the instant that window's newly-selected character produces
    // its own keyed frame (the drive re-engages because the freeze latch below is cleared). Previously
    // this nulled the snapshot to avoid flashing the previous character; that flash IS now the desired
    // behavior (old head held until the new masked head is ready), bounded by the keyed-publish gate.
    // Only the per-window drive-freeze latch is cleared here so the next window re-renders.
    PROFILE_BAKE_RGBA_CAPTURED.store(0, Ordering::SeqCst);
    PROFILE_RT_PIN.store(0, Ordering::SeqCst);
    PROFILE_DEPTH_PIN.store(0, Ordering::SeqCst);
    OVERLAY_NOW_LOADING_SEEN.store(0, Ordering::SeqCst);
    // Do NOT drop the spared renderer -- that leaked one live CSMenuProfModelRend per switch (it was
    // excluded from the native delete and its offscreen draw task kept filling the 192-slot GX
    // command queue -> 0x1aeaf05 overflow ~switch #4). MOVE it to the orphan slot; the game-thread
    // teardown-spare hook delete-enqueues it via CSDelayDeleteMan at the next teardown (this reset
    // runs off the game thread, so it stashes rather than deleting in place).
    let prev_spared = LOADING_BG_PORTRAIT_SPARED_RENDERER.swap(0, Ordering::SeqCst);
    if prev_spared != 0 {
        PROFILE_SPARE_ORPHAN.store(prev_spared, Ordering::SeqCst);
    }
    PROFILE_SPARE_CANDIDATE.store(0, Ordering::SeqCst);
    // Re-arm the idle-anim bind + drop the motion-metric history so the NEXT load window binds its
    // own renderer and starts a fresh inter-frame diff (cumulative attempt/max oracles are kept).
    PORTRAIT_ANIM_BIND_STATE.store(0, Ordering::SeqCst);
    PORTRAIT_ANIM_BOUND_RENDERER.store(0, Ordering::SeqCst);
    PORTRAIT_ANIM_BOUND_LOC.store(0, Ordering::SeqCst);
    PORTRAIT_KICK_SLOT_KEY.store(0, Ordering::SeqCst);
    PORTRAIT_KICK_RENDERER.store(0, Ordering::SeqCst);
    if let Ok(mut g) = PORTRAIT_MOTION_PREV_PLANES.lock() {
        *g = None;
    }
    if let Ok(mut g) = LAST_DEPTH_MASK.lock() {
        *g = None;
    }
    // Cache cleared -> forget which character it was for (a fresh compute re-tags it).
    LAST_DEPTH_MASK_INCARNATION.store(0, Ordering::SeqCst);
    // Animation-stall semaphore: snapshot this window's animated-vs-displayed frame counts, then zero
    // for the next window. drive << display == the head froze early (freeze-after-capture); the
    // user's "stopped animating / frozen the whole loading screen" symptom shows here as a low ratio.
    let drive = PROFILE_DRIVE_FRAMES_WINDOW.swap(0, Ordering::SeqCst);
    let display = PROFILE_DISPLAY_FRAMES_WINDOW.swap(0, Ordering::SeqCst);
    PROFILE_DRIVE_FRAMES_WINDOW_LAST.store(drive, Ordering::SeqCst);
    PROFILE_DISPLAY_FRAMES_WINDOW_LAST.store(display, Ordering::SeqCst);
    // PUBLISH-STARVATION ATTRIBUTION (2026-07-03 soak: windows froze on the PRIOR character with the
    // drive running ~1:1, so the starving class is publish-side and the cumulative oracles cannot say
    // WHICH window starved or WHY). Snapshot each publish/skip class per window (delta vs the previous
    // reset) so a frozen window names its own cause: published==0 with a dominant torn/unkeyed/multi
    // count is the starvation signature; pin_moves counts content-RT recreations inside the window.
    let winof = |cum: &AtomicUsize, last: &AtomicUsize| -> usize {
        let c = cum.load(Ordering::SeqCst);
        c.saturating_sub(last.swap(c, Ordering::SeqCst))
    };
    let published = winof(&PROFILE_PUBLISH_CLEAN, &PROFILE_PUBLISH_CLEAN_WINDOW_MARK);
    let torn = winof(
        &PROFILE_PUBLISH_SKIPPED_TORN,
        &PROFILE_PUBLISH_SKIPPED_TORN_WINDOW_MARK,
    );
    let unkeyed = winof(
        &PROFILE_PUBLISH_SKIPPED_UNKEYED,
        &PROFILE_PUBLISH_SKIPPED_UNKEYED_WINDOW_MARK,
    );
    let multi = winof(
        &PROFILE_MULTI_MODEL_PUBLISH_SKIPS,
        &PROFILE_MULTI_MODEL_PUBLISH_SKIPS_WINDOW_MARK,
    );
    let pin_moves = winof(
        &PROFILE_RT_PIN_SWITCHES,
        &PROFILE_RT_PIN_SWITCHES_WINDOW_MARK,
    );
    let fence_skips = winof(
        &PROFILE_DRIVE_FENCE_SKIPS,
        &PROFILE_DRIVE_FENCE_SKIPS_WINDOW_MARK,
    );
    append_autoload_debug(format_args!(
        "present-overlay: loading-portrait window reset ({reason}) -- animated {drive} / displayed {display} frames (drive<<display == froze early); publish[clean={published} torn={torn} unkeyed={unkeyed} multi={multi} pin_moves={pin_moves} fence_skips={fence_skips}] (clean=0 == frozen on prior character; the dominant skip class is the cause); pins/spare cleared for the next load"
    ));
}

/// Invalidate the depth-key MASKING PLANE for a NEW model: drop the cached mask and the pinned depth
/// candidate so the next `apply_depth_alpha_key` RECOMPUTES the silhouette from the new model's own depth
/// buffer instead of reusing the previous character's cached mask. Without this, a System Quit -> Load
/// Profile character switch would cut the OLD character's silhouette out of the NEW head until fresh depth
/// happened to land. Fail-open in the gap (leaves the head opaque) -- never a stale wrong-shape cutout.
pub(crate) fn invalidate_portrait_depth_mask() {
    PROFILE_DEPTH_PIN.store(0, Ordering::SeqCst);
    if let Ok(mut g) = LAST_DEPTH_MASK.lock() {
        *g = None;
    }
    // Cache cleared -> forget which character it was for (a fresh compute re-tags it).
    LAST_DEPTH_MASK_INCARNATION.store(0, Ordering::SeqCst);
}

unsafe fn composite_portrait_inner(base: usize, swapchain_raw: usize) -> bool {
    // LOADING-SCREEN WINDOW gate. The head composites while the map is LOADING (`!load_done`, the corrected
    // signal) and pops the instant the load COMPLETES (`load_done` false->true). IN_WORLD_REACHED is never a
    // stop -- it latches while the loading screen is still up (PlayerIns lives through it), the premature-pop
    // bug. The captured-head snapshot (PROFILE_BAKE_RGBA_CAPTURED, cleared only at the stop) persists even
    // after the profile renderers tear down, so the head stays on screen (frozen if the renderers are gone,
    // tracking while alive) for the whole load -- never blanks mid-load, never lingers into gameplay.
    // CORRECTED SIGNAL (RE 2026-07-02, CSNowLoadingHelperImp::Update decompile): `now_loading_active`
    // reads `load_done` -- a load-COMPLETE latch that is FALSE while the map loads and TRUE once it finishes
    // (and lingers into gameplay). So "still on the loading screen" is `!load_done`. The head must show
    // while loading and pop the instant the load COMPLETES (load_done false->true), NOT when load_done later
    // drops (that only happens on the NEXT load -> the head-persists-into-gameplay bug). `fake_vis` (the
    // CSFakeLoadingScreenImp black plate) is a secondary "still covered" signal that also means loading.
    let fake_vis = unsafe { fake_loading_screen_visible(base) };
    let load_done = unsafe { now_loading_active(base) };
    let loading = !load_done || fake_vis;
    let loading_seen = if loading {
        OVERLAY_BRIDGE_PRESENTS.store(0, Ordering::SeqCst);
        OVERLAY_NOW_LOADING_SEEN.store(1, Ordering::SeqCst);
        true
    } else {
        OVERLAY_NOW_LOADING_SEEN.load(Ordering::SeqCst) != 0
    };
    if OVERLAY_STOPPED.load(Ordering::SeqCst) != 0 {
        // Stopped: re-arm ONLY on evidence of a NEW loading window -- loading reasserting, or a fresh
        // post-Continue table build (the System Quit character-switch reload path). Reset the seen latch so
        // the previous window can't instant-stop this one.
        let rebuilt = PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst)
            > OVERLAY_STOP_TABLE_BUILDS.load(Ordering::SeqCst);
        if !(loading || rebuilt) {
            return false;
        }
        OVERLAY_STOPPED.store(0, Ordering::SeqCst);
        OVERLAY_NOW_LOADING_SEEN.store(if loading { 1 } else { 0 }, Ordering::SeqCst);
        OVERLAY_BRIDGE_PRESENTS.store(0, Ordering::SeqCst);
        OVERLAY_WORLD_STOP_LOGGED.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "present-overlay: re-armed for a new loading window (loading={loading} rebuilt={rebuilt})"
        ));
    }
    // PRIMARY STOP: we were on the loading screen and the load has now COMPLETED (load_done true AND the
    // black plate gone). This is the game's own transition to gameplay -- the spec-correct pop moment (the
    // instant the bar fills). IN_WORLD_REACHED is deliberately never consulted: it latches while the loading
    // screen is still up (PlayerIns lives through it), which was the premature-pop bug.
    if loading_seen && !loading {
        OVERLAY_STOPPED.store(1, Ordering::SeqCst);
        OVERLAY_STOP_TABLE_BUILDS.store(
            PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        OVERLAY_WINDOW_STOPS.fetch_add(1, Ordering::SeqCst);
        OVERLAY_STOP_REASON.store(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "present-overlay: load completed (load_done latched, cover plate gone) -> stopped compositing at the game's transition to gameplay"
        ));
        loading_portrait_window_reset("load completed");
        return false;
    }
    // ANTI-RUNAWAY BACKSTOP: pathological case where the load reports done AND we're in-world but the
    // primary stop can't fire (e.g. the black plate's `visible` stuck at 1). Count in-world+load_done
    // frames; force a stop past a huge budget so the head can't composite over gameplay forever. Never
    // fires on a normal load (load_done + !fake_vis stops immediately). reason=3 flags the assumption broke.
    if load_done && IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES {
        let n = OVERLAY_BRIDGE_PRESENTS.fetch_add(1, Ordering::SeqCst) + 1;
        if n >= OVERLAY_NOWLOAD_BRIDGE_MAX_PRESENTS {
            OVERLAY_STOPPED.store(1, Ordering::SeqCst);
            OVERLAY_STOP_TABLE_BUILDS.store(
                PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
            OVERLAY_WINDOW_STOPS.fetch_add(1, Ordering::SeqCst);
            OVERLAY_STOP_REASON.store(3, Ordering::SeqCst);
            if OVERLAY_WORLD_STOP_LOGGED.swap(1, Ordering::SeqCst) == 0 {
                append_autoload_debug(format_args!(
                    "present-overlay: BACKSTOP stop -- load_done + in-world for {n} presents but primary stop never fired (cover plate stuck?); forcing stop"
                ));
            }
            loading_portrait_window_reset("load-done backstop");
            return false;
        }
    }
    // DISPLAY-AVAILABILITY gate, decoupled from the drive-freeze latch (make-before-break): show
    // whenever we have EVER published a keyed (masked) frame (PROFILE_HAVE_KEYED_FRAME, persistent) or
    // the diagnostic bake path latched one. This is what lets the prior masked head keep displaying
    // after a confirm clears the drive-freeze (PROFILE_BAKE_RGBA_CAPTURED) to re-render the new
    // character -- the composite keeps showing LOADING_BG_PORTRAIT_RGBA until the new model's first
    // keyed frame replaces it. Before ANY keyed frame exists, bail (no opaque/blank flash).
    if PROFILE_HAVE_KEYED_FRAME.load(Ordering::SeqCst) == 0
        && PROFILE_BAKE_RGBA_CAPTURED.load(Ordering::SeqCst) == 0
    {
        return false;
    }
    // NOTE: this used to bail when render-drive was on, back when "render-drive" meant the Present hook
    // itself drove the offscreen rasterize (so compositing here would have fought it). The rasterize now
    // runs in the draw-phase task (profile_lookat_realtime_draw_tick -> drive(r)), which re-renders the
    // posed model and the readback republishes LOADING_BG_PORTRAIT_RGBA (version bump) EVERY frame. So the
    // Present hook is free to composite -- and MUST, to push that per-frame tracking head to the screen for
    // the whole loading screen (the forge redirect only commits ~twice -> a frozen displayed head). The
    // live-re-upload block below rebuilds the overlay texture on each version bump, so the displayed head
    // follows the cursor until loading completes.
    let forge_committed = LOADING_BG_TEXTURE_REDIRECT_COMMITS.load(Ordering::SeqCst) > 0;
    // Forge-INDEPENDENT loading-window latch: PROFILE_LOADSCREEN_TABLE_BUILDS goes >0 when we (re)build our
    // profile renderers post-Continue -- i.e. we are on the loading cover, past the menu, before the world.
    // The overlay used to lean solely on `forge_committed` as its "cover is up" signal (now_loading_active /
    // fake_loading_screen_visible both read false in the direct-menu-load path), so disabling the native
    // forge silently killed the overlay too. This latch keeps the overlay live when the forge is off
    // (overlay-only mode) without depending on it. Same monotonic behaviour (never decrements) as forge.
    let loadscreen_active = PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst) > 0;
    if !(forge_committed || loadscreen_active || loading) {
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

    // LIVE PORTRAIT SNAPSHOT: the draw-phase task republishes LOADING_BG_PORTRAIT_RGBA (version bump) each
    // frame with the freshly rendered, DEPTH-ALPHA-KEYED head (background alpha 0), so we blend the CURRENT
    // snapshot every frame -- the displayed head follows the cursor and its background stays transparent. On
    // any snapshot failure we skip this frame (leave the last presented content).
    let cur_ver = LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst);
    let Some((sw, sh, spx)) = LOADING_BG_PORTRAIT_RGBA.lock().ok().and_then(|g| g.clone()) else {
        return false;
    };
    if sw == 0 || sh == 0 || spx.len() < (sw as usize) * (sh as usize) * RGBA8_BPP {
        return false;
    }
    // Animation-stall semaphore: a portrait frame is being displayed this present. Paired with the
    // per-drive-frame counter, a low drive/display ratio means the head froze early in the window.
    PROFILE_DISPLAY_FRAMES_WINDOW.fetch_add(1, Ordering::SeqCst);

    let alloc_raw = OVERLAY_ALLOCATOR.load(Ordering::SeqCst) as *mut c_void;
    let list_raw = OVERLAY_LIST.load(Ordering::SeqCst) as *mut c_void;
    let fence_raw = OVERLAY_FENCE.load(Ordering::SeqCst) as *mut c_void;
    let queue_raw = OVERLAY_QUEUE.load(Ordering::SeqCst) as *mut c_void;
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
    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { backbuffer.GetDevice(&mut device_opt) }.is_err() {
        return false;
    }
    let Some(device) = device_opt else {
        return false;
    };

    let bb_desc = unsafe { backbuffer.GetDesc() };
    let bw = bb_desc.Width as u32;
    let bh = bb_desc.Height;
    if bw == 0 || bh == 0 {
        return false;
    }
    // Center the portrait region on the backbuffer (no scaling; region = portrait size clamped to the bb).
    let cw = sw.min(bw);
    let ch = sh.min(bh);
    let dx = (bw - cw) / 2;
    let dy = (bh - ch) / 2;

    // Alpha-honoring composite: read the live backbuffer region, blend the portrait OVER it (bg alpha 0 =>
    // the loading screen shows through), write the blended region back. All via COPY primitives -- no PSO.
    if !unsafe {
        blend_portrait_over_backbuffer(
            &device,
            queue,
            allocator,
            list,
            fence,
            &backbuffer,
            bb_desc.Format,
            dx,
            dy,
            cw,
            ch,
            sw,
            &spx,
        )
    } {
        return false;
    }

    // Preserve the "displayed head updates per frame" oracle (oracle_overlay_reuploads): count a fresh
    // published version reaching the screen.
    if cur_ver != OVERLAY_PORTRAIT_VERSION.swap(cur_ver, Ordering::SeqCst) {
        OVERLAY_REUPLOADS.fetch_add(1, Ordering::SeqCst);
    }
    let hits = OVERLAY_DRAW_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if hits == 1 {
        append_autoload_debug(format_args!(
            "present-overlay: portrait CPU-blended onto backbuffer {bw}x{bh} (portrait {sw}x{sh} at {dx},{dy}, depth-alpha-keyed bg)"
        ));
    }
    true
}

/// Alpha-honoring CPU composite: copy the live backbuffer region `[dx,dy .. dx+cw,dy+ch]` to a readback
/// buffer, blend the portrait (`spx`, `sw` wide, RGBA8 with per-pixel alpha) OVER it (`src.a`/`1-src.a`; a
/// background pixel with alpha 0 leaves the backbuffer untouched so the loading screen shows through), then
/// write the blended region back. Two submits on our OWN queue with a CPU fence wait between them (the blend
/// needs the readback mapped). Reuses the cached `OVERLAY_BB_*` buffers; leaves the backbuffer in PRESENT.
/// `false` on any failure (frame skipped). Never touches the game's queue.
#[allow(clippy::too_many_arguments)]
unsafe fn blend_portrait_over_backbuffer(
    device: &ID3D12Device,
    queue: &ID3D12CommandQueue,
    allocator: &ID3D12CommandAllocator,
    list: &ID3D12GraphicsCommandList,
    fence: &ID3D12Fence,
    backbuffer: &ID3D12Resource,
    bb_format: DXGI_FORMAT,
    dx: u32,
    dy: u32,
    cw: u32,
    ch: u32,
    sw: u32,
    spx: &[u8],
) -> bool {
    // Copyable footprint of a cw x ch region in the backbuffer's format (256-aligned rows).
    let region_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: cw as u64,
        Height: ch,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: bb_format,
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
    if total_bytes == 0 || footprint.Footprint.RowPitch == 0 {
        return false;
    }
    // (Re)create the cached readback + upload buffers on footprint change (fixed once for a fixed bb size).
    if OVERLAY_BB_BUFSIZE.load(Ordering::SeqCst) != total_bytes {
        let rb_heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_READBACK,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
            MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
            CreationNodeMask: 1,
            VisibleNodeMask: 1,
        };
        let up_heap = D3D12_HEAP_PROPERTIES {
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
        let mut rb_opt: Option<ID3D12Resource> = None;
        if unsafe {
            device.CreateCommittedResource(
                &rb_heap,
                D3D12_HEAP_FLAG_NONE,
                &buf_desc,
                D3D12_RESOURCE_STATE_COPY_DEST,
                None,
                &mut rb_opt,
            )
        }
        .is_err()
        {
            return false;
        }
        let mut up_opt: Option<ID3D12Resource> = None;
        if unsafe {
            device.CreateCommittedResource(
                &up_heap,
                D3D12_HEAP_FLAG_NONE,
                &buf_desc,
                D3D12_RESOURCE_STATE_GENERIC_READ,
                None,
                &mut up_opt,
            )
        }
        .is_err()
        {
            return false;
        }
        let (Some(rb), Some(up)) = (rb_opt, up_opt) else {
            return false;
        };
        let old_rb = OVERLAY_BB_READBACK.swap(rb.into_raw() as usize, Ordering::SeqCst);
        if old_rb != 0 {
            drop(unsafe { ID3D12Resource::from_raw(old_rb as *mut c_void) });
        }
        let old_up = OVERLAY_BB_UPLOAD.swap(up.into_raw() as usize, Ordering::SeqCst);
        if old_up != 0 {
            drop(unsafe { ID3D12Resource::from_raw(old_up as *mut c_void) });
        }
        OVERLAY_BB_BUFSIZE.store(total_bytes, Ordering::SeqCst);
    }
    let rb_raw = OVERLAY_BB_READBACK.load(Ordering::SeqCst) as *mut c_void;
    let up_raw = OVERLAY_BB_UPLOAD.load(Ordering::SeqCst) as *mut c_void;
    let (Some(readback), Some(upload)) = (unsafe {
        (
            ID3D12Resource::from_raw_borrowed(&rb_raw),
            ID3D12Resource::from_raw_borrowed(&up_raw),
        )
    }) else {
        return false;
    };

    // ---- SUBMIT #1: backbuffer region -> readback buffer (leaves the backbuffer in COPY_SOURCE) ----
    if unsafe { allocator.Reset() }.is_err() || unsafe { list.Reset(allocator, None) }.is_err() {
        return false;
    }
    unsafe {
        record_transition(
            list,
            backbuffer,
            D3D12_RESOURCE_STATE_PRESENT,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )
    };
    let mut rb_dst = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(readback.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    let mut bb_src = D3D12_TEXTURE_COPY_LOCATION {
        pResource: ManuallyDrop::new(Some(backbuffer.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    let read_box = D3D12_BOX {
        left: dx,
        top: dy,
        front: 0,
        right: dx + cw,
        bottom: dy + ch,
        back: 1,
    };
    unsafe { list.CopyTextureRegion(&rb_dst, 0, 0, 0, &bb_src, Some(&read_box)) };
    unsafe { ManuallyDrop::drop(&mut rb_dst.pResource) };
    unsafe { ManuallyDrop::drop(&mut bb_src.pResource) };
    if !unsafe { execute_and_wait(queue, list, fence) } {
        return false;
    }

    // ---- CPU BLEND: readback (backbuffer pixels) OVER-composited with the portrait, into the upload buffer.
    let row_pitch = footprint.Footprint.RowPitch as usize;
    let total = total_bytes as usize;
    let swap = matches!(
        bb_format,
        DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
    );
    let read_range = D3D12_RANGE {
        Begin: 0,
        End: total,
    };
    let mut rmap: *mut c_void = std::ptr::null_mut();
    if unsafe { readback.Map(0, Some(&read_range), Some(&mut rmap)) }.is_err() || rmap.is_null() {
        return false;
    }
    let mut umap: *mut c_void = std::ptr::null_mut();
    if unsafe { upload.Map(0, None, Some(&mut umap)) }.is_err() || umap.is_null() {
        let empty = D3D12_RANGE { Begin: 0, End: 0 };
        unsafe { readback.Unmap(0, Some(&empty)) };
        return false;
    }
    {
        let rb_bytes = unsafe { std::slice::from_raw_parts(rmap as *const u8, total) };
        let up_bytes = unsafe { std::slice::from_raw_parts_mut(umap as *mut u8, total) };
        let sw = sw as usize;
        let cw = cw as usize;
        let ch = ch as usize;
        for y in 0..ch {
            let ro = y * row_pitch;
            for x in 0..cw {
                let o = ro + x * 4;
                let so = (y * sw + x) * 4;
                if o + 4 > total || so + 4 > spx.len() {
                    break;
                }
                let a = spx[so + 3] as u32;
                let ia = 255 - a;
                // Portrait is RGBA; place each portrait channel at the backbuffer's channel position.
                let (p0, p2) = if swap {
                    (spx[so + 2] as u32, spx[so] as u32) // bb pos0=B, pos2=R
                } else {
                    (spx[so] as u32, spx[so + 2] as u32) // bb pos0=R, pos2=B
                };
                let p1 = spx[so + 1] as u32;
                let blend = |p: u32, d: u32| ((p * a + d * ia + 127) / 255) as u8;
                up_bytes[o] = blend(p0, rb_bytes[o] as u32);
                up_bytes[o + 1] = blend(p1, rb_bytes[o + 1] as u32);
                up_bytes[o + 2] = blend(p2, rb_bytes[o + 2] as u32);
                up_bytes[o + 3] = 255;
            }
        }
    }
    let empty = D3D12_RANGE { Begin: 0, End: 0 };
    unsafe { readback.Unmap(0, Some(&empty)) };
    unsafe { upload.Unmap(0, None) };

    // ---- SUBMIT #2: upload buffer -> backbuffer region (COPY_SOURCE -> COPY_DEST -> PRESENT) ----
    if unsafe { allocator.Reset() }.is_err() || unsafe { list.Reset(allocator, None) }.is_err() {
        return false;
    }
    unsafe {
        record_transition(
            list,
            backbuffer,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
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
        right: cw,
        bottom: ch,
        back: 1,
    };
    unsafe { list.CopyTextureRegion(&bb_dst, dx, dy, 0, &up_src, Some(&up_box)) };
    unsafe { ManuallyDrop::drop(&mut up_src.pResource) };
    unsafe { ManuallyDrop::drop(&mut bb_dst.pResource) };
    unsafe {
        record_transition(
            list,
            backbuffer,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_PRESENT,
        )
    };
    unsafe { execute_and_wait(queue, list, fence) }
}

/// Close `list`, execute it on `queue`, signal `fence` with a fresh monotonic value, and CPU-wait (bounded)
/// for GPU completion. `false` on any failure. Shared by the two-submit CPU-blend composite.
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
