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
// COLOR/DEPTH SOURCE PROVENANCE (green-face wrong-buffer fix, 2026-07-03). The offscreen nest holds
// same-size same-format non-final render targets (material/G-buffer: flat-green face, saturated
// orange emissive -- user screenshot), and the whole-nest "largest texture" scan can pick one when
// the deterministic scene-bundle chain misses; keyed+tear gates cannot tell buffers apart. Track
// where each tick's color/depth came from; the strict publish gate displays ONLY bundle-provenance
// color (identity-proven by construction), and scan-resolved frames hold the bridge instead.
/// Per-tick color provenance: 1 = scene-bundle RTV (identity-proven), 0 = whole-nest scan fallback.
/// Written by the readback, consumed immediately by the same-thread draw tick.
pub(crate) static PROFILE_COLOR_SRC_BUNDLE_LAST: AtomicUsize = AtomicUsize::new(0);
/// Cumulative ticks whose color resolved from the scene bundle vs the scan fallback.
pub(crate) static PROFILE_COLOR_FROM_BUNDLE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_COLOR_FROM_SCAN: AtomicUsize = AtomicUsize::new(0);
/// Cumulative depth resolutions via the deterministic bundle chain vs the heuristic BFS fallback.
pub(crate) static PROFILE_DEPTH_FROM_CHAIN: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_DEPTH_FROM_BFS: AtomicUsize = AtomicUsize::new(0);
/// Keyed+clean frames NOT displayed because their color was scan-resolved (no bundle provenance).
pub(crate) static PROFILE_PUBLISH_SKIPPED_UNPAIRED: AtomicUsize = AtomicUsize::new(0);

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
            PROFILE_DEPTH_FROM_CHAIN.fetch_add(1, Ordering::SeqCst);
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
    let r = unsafe { find_d3d12_resource_ex(start, 0, true, prefer) };
    if r.is_some() {
        PROFILE_DEPTH_FROM_BFS.fetch_add(1, Ordering::SeqCst);
    }
    r
}

/// NATIVE ALPHA-0 CLEAR of the offscreen scene's color RT (scene-alpha keying, strategy pivot
/// 2026-07-03). Replicates the engine's own per-slot offscreen clear (dump FUN_140bb73a0) with
/// clear color {0,0,0,0} instead of the shared opaque-black constant: pop a GX frame context,
/// ClearRTV the scene bundle's own RTV through the frame's subcontext, release the frame context.
/// Called by the pump on the render thread immediately before the model update+draw, so each
/// frame's RT is subject-only with alpha == model coverage (the backdrop box was never redrawn --
/// it was stale pixels the old skip-the-clear behavior preserved). Fail-closed on every link
/// (missing bundle/RTV/frame -> no clear, the frame simply keeps its previous content); the
/// redirect-to-global-target case fails closed inside `offscreen_target_bundle` so the swapchain
/// can never be cleared by mistake.
pub(crate) unsafe fn portrait_alpha0_clear(base: usize, off: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let Some(bundle) = (unsafe { offscreen_target_bundle(off) }) else {
        return false;
    };
    let rtv = unsafe { safe_read_usize(bundle + TARGET_BUNDLE_RTV_VIEW_OFFSET) }.unwrap_or(0);
    if !valid(rtv) {
        return false;
    }
    let gx = unsafe { safe_read_usize(base + GX_DRAW_CONTEXT_RVA) }.unwrap_or(0);
    if !valid(gx) {
        return false;
    }
    let pop: unsafe extern "system" fn(usize) -> usize =
        unsafe { core::mem::transmute(base + GX_FRAME_CTX_POP_RVA) };
    let frame = unsafe { pop(gx) };
    if !valid(frame) {
        return false;
    }
    let sub = unsafe { safe_read_usize(frame + GX_FRAME_SUBCTX_OFFSET) }.unwrap_or(0);
    let ok = if valid(sub) {
        let color: [f32; 4] = [0.0, 0.0, 0.0, 0.0];
        let clear: unsafe extern "system" fn(usize, usize, *const f32) =
            unsafe { core::mem::transmute(base + GX_CLEAR_RTV_WRAPPER_RVA) };
        unsafe { clear(sub, rtv, color.as_ptr()) };
        true
    } else {
        false
    };
    // Release the popped frame context even when the subcontext was missing -- the pop/release
    // pair must balance exactly like the engine's own body.
    let release: unsafe extern "system" fn(usize, usize) =
        unsafe { core::mem::transmute(base + GX_FRAME_CTX_RELEASE_RVA) };
    unsafe { release(gx, frame) };
    ok
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
    // VTABLE HARDENING (crash root fix 2026-07-03, runs +154662ms/+170480ms, previously
    // misattributed to er-effects-rs-az9). A real COM vtable lies PAST the PE header page of its
    // module (.rdata), and its slot 0 (QueryInterface) points back into a d3d module's code. The
    // plain `in_ranges(vt, &d3d)` check also accepted vt == module BASE: a freed heap chunk reused
    // to store a d3d module HANDLE (a base address) passed it, and the QI vcall then executed the
    // PE header as code -- the crash RIP 0x300905a4d is literally the 'MZ\x90' signature bytes.
    // Require both the vtable and its QI slot to sit >= 0x1000 into a d3d module before any vcall.
    let d3d_vtable_ok = |vt: usize, r: &[(usize, usize)]| {
        r.iter().any(|&(lo, hi)| lo + 0x1000 <= vt && vt < hi)
            && unsafe { safe_read_usize(vt) }
                .is_some_and(|qi| r.iter().any(|&(lo, hi)| lo + 0x1000 <= qi && qi < hi))
    };

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
                        if d3d_vtable_ok(vt, &d3d) {
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
    let r = unsafe { readback_offscreen_fast(gpu_child) };
    if r.is_some() {
        // Separate-path color is scan-resolved: no bundle provenance this tick.
        PROFILE_COLOR_SRC_BUNDLE_LAST.store(0, Ordering::SeqCst);
        PROFILE_COLOR_FROM_SCAN.fetch_add(1, Ordering::SeqCst);
    }
    r
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
        let (color_res, color_cand, color_from_bundle) = match bundle_color {
            Some((r, c)) => (r, c, true),
            None => {
                let prefer_c = PROFILE_RT_PIN.load(Ordering::SeqCst);
                let (r, c) = find_d3d12_resource_ex(gpu_child, 0, false, prefer_c)?;
                (r, c, false)
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

        // Success is certain here -- record this tick's color provenance for the strict publish
        // gate (a later-failing attempt never reaches this store; the fallback path stores 0).
        PROFILE_COLOR_SRC_BUNDLE_LAST.store(color_from_bundle as usize, Ordering::SeqCst);
        if color_from_bundle {
            PROFILE_COLOR_FROM_BUNDLE.fetch_add(1, Ordering::SeqCst);
        } else {
            PROFILE_COLOR_FROM_SCAN.fetch_add(1, Ordering::SeqCst);
        }
        Some((cw, ch, color, color_cand, dw, dh, depth, depth_cand))
    }))
    .ok()
    .flatten()
}
