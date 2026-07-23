
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
pub(crate) use er_telemetry::counters::PROFILE_LIVE_RT_RES;
/// PER-FRAME readback resource cache (created ONCE on the game device, reused every frame). The original
/// per-call readback created a fresh command queue + allocator + list + fence + buffer EACH frame; under
/// the loading screen that mostly failed at resource creation (command-queue creation is limited), so the
/// readback published only ~4x and the displayed head froze. Caching them lets the per-frame readback be a
/// cheap reset+copy+wait, so it publishes every frame and the overlay re-uploads the tracking head per
/// frame. Raw COM pointers owned by these statics (released only on dims change).
pub(crate) use er_telemetry::counters::RB_FAST_QUEUE;
pub(crate) use er_telemetry::counters::RB_FAST_ALLOC;
pub(crate) use er_telemetry::counters::RB_FAST_LIST;
pub(crate) use er_telemetry::counters::RB_FAST_FENCE;
pub(crate) use er_telemetry::counters::RB_FAST_BUFFER;
pub(crate) use er_telemetry::counters::RB_FAST_BUFSIZE;
pub(crate) use er_telemetry::counters::RB_FAST_FENCEVAL;
/// Counter so the deterministic-resolve diagnostic logs only the first few attempts.
pub(crate) use er_telemetry::counters::PROFILE_DET_RESOLVE_DIAG;

/// PER-FRAME DEPTH readback cache (separate from RB_FAST_* so the color and depth readbacks never share a
/// command list / fence across the two calls in one draw tick). Same create-once + reset+copy+wait pattern.
/// The depth sibling is `R32G8X24_TYPELESS` (fmt 19); we copy PLANE 0 (the R32 float depth) and reinterpret
/// each 4-byte texel as `f32`. Raw COM pointers owned by these statics (released only on footprint change).
pub(crate) use er_telemetry::counters::RB_DEPTH_QUEUE;
pub(crate) use er_telemetry::counters::RB_DEPTH_ALLOC;
pub(crate) use er_telemetry::counters::RB_DEPTH_LIST;
pub(crate) use er_telemetry::counters::RB_DEPTH_FENCE;
pub(crate) use er_telemetry::counters::RB_DEPTH_BUFFER;
pub(crate) use er_telemetry::counters::RB_DEPTH_BUFSIZE;
pub(crate) use er_telemetry::counters::RB_DEPTH_FENCEVAL;
/// One-shot `depth-key` diagnostic latch (logs corner/center/min/max depth + masked fraction once).
pub(crate) use er_telemetry::counters::DEPTH_KEY_DIAG_LOGGED;
/// RAM oracle: number of published frames where the depth key ACTUALLY cut out a background (i.e. the depth
/// buffer read back with clean bg/head separation and `>0` pixels were set to alpha 0). `oracle_depth_key_
/// applied` -- a pixel/native semaphore that the transparent-background cutout is live (not a screenshot).
pub(crate) use er_telemetry::counters::DEPTH_KEY_APPLIED;
/// RAM oracle: last frame's background-masked fraction, in whole percent (0..=100). `oracle_depth_key_bg_pct`.
/// A plausible portrait cutout is a large minority/majority of the frame (bg dominates a centered head).
pub(crate) use er_telemetry::counters::DEPTH_KEY_BG_PCT;
/// RAM oracle: number of frames the mask was RECALCULATED fresh from a valid depth buffer (vs reused from
/// cache). `oracle_depth_key_fresh`; `applied - fresh` = cached reuses. Proves the recalc-and-cache loop.
pub(crate) use er_telemetry::counters::DEPTH_KEY_FRESH;
/// One-shot latch for the no-gap / dims-mismatch `depth-key` skip diagnostic (separate from the success
/// latch so both a good frame and a skipped frame are each visible once in the log).
pub(crate) use er_telemetry::counters::DEPTH_KEY_NOGAP_LOGGED;
/// Frames whose fresh depth was DEGENERATE for masking (no histogram gap, or a mask cutting under the
/// publish floor). Throttles the recurring depth diagnostic (er-effects-rs-hi2: a whole window sat in
/// the lowmask band and the one-shot diag from boot left it invisible). `oracle_depth_key_degenerate`.
pub(crate) use er_telemetry::counters::DEPTH_KEY_DEGENERATE;
/// Degenerate frames RECOVERED by the second-pass histogram (clear-plane extremes excluded) --
/// the backdrop-geometry windows' masks. `oracle_depth_key_second_pass`.
pub(crate) use er_telemetry::counters::DEPTH_KEY_SECOND_PASS;
/// Keyed+clean frames HELD because their mask/head coherence (IoU) was below MASK_HEAD_IOU_MIN --
/// the "cut the wrong 34%" frames (user 2026-07-03: displayed heads whose backdrop was not keyed
/// out right; the share floor checks how MUCH is cut, IoU checks WHERE). Plus its window mark.
pub(crate) use er_telemetry::counters::PROFILE_PUBLISH_SKIPPED_BADIOU;
pub(crate) use er_telemetry::counters::PROFILE_PUBLISH_SKIPPED_BADIOU_WINDOW_MARK;
/// One-shot latch for the interior-histogram ground-truth dump when even the valley pass fails.
pub(crate) use er_telemetry::counters::DEPTH_KEY_HIST_DUMPED;
/// Per-window MIN transparent share (percent) among PUBLISHED frames (usize::MAX = none published) and
/// MAX share among lowmask-held frames -- the two sides of the floor, for setting it from evidence.
pub(crate) use er_telemetry::counters::PROFILE_PUBLISH_SHARE_MIN;
pub(crate) use er_telemetry::counters::PROFILE_LOWMASK_SHARE_MAX;
/// Last RECALCULATED depth-key mask (w, h, per-pixel: 1 = background/cut, 0 = keep). The offscreen depth
/// buffer only carries real content on genuine re-render frames; on the many frames it reads back cleared,
/// we re-apply this cached mask so the cutout stays stable. It is RECALCULATED whenever fresh depth is
/// available (tracking a real re-render) and only cached for the dead frames in between -- never frozen.
static LAST_DEPTH_MASK: Mutex<Option<(usize, usize, Vec<u8>)>> = Mutex::new(None);
/// Current portrait character incarnation (drive slot + 1; 0 = unset), set by the per-frame drive so the
/// mask cache can be tagged with the character it was computed for. A depth mask REUSED across a change
/// of this value means the PREVIOUS character's silhouette is being applied to the NEW character's head
/// -- the 2nd-character depth-mask desync (user 2026-07-03). See `PROFILE_MASK_STALE_REUSE`.
pub(crate) use er_telemetry::counters::PROFILE_PORTRAIT_INCARNATION;
/// Incarnation the currently-cached `LAST_DEPTH_MASK` was computed for (0 = none / cleared).
pub(crate) use er_telemetry::counters::LAST_DEPTH_MASK_INCARNATION;
/// FAIL-FAST desync semaphore: count of frames that REUSED the cached depth mask while the live portrait
/// incarnation differs from the one the cache was computed for -- a prior character's mask on the new
/// head. It trips early + deterministically (the 2nd character of a switch chain), so a run can stop in
/// ~40s instead of six minutes. Exposed as `oracle_portrait_mask_stale_reuse`.
pub(crate) use er_telemetry::counters::PROFILE_MASK_STALE_REUSE;
pub(crate) use er_telemetry::counters::PROFILE_MASK_STALE_REUSE_LOGGED;
/// FAIL-FAST mask/head coherence semaphore (the 2nd-character desync is a FRESH-but-WRONG mask: masks are
/// recomputed every frame, so it is not a cache reuse). Per published frame, IoU of the KEPT cutout region
/// (mask==0) vs the colour's OWN head (pixels far from the background colour). A correct mask keeps the
/// head -> high IoU; a fresh mask of a WRONG depth silhouette (stale depth content on the new character)
/// keeps a region that does not match this head -> low IoU. For DARK characters whose colour-head is a
/// sliver (Sacred Bean, er-effects-rs-y134) the score is head-COVERAGE instead of symmetric IoU -- see
/// `mask_head_iou`. `_last` is an oracle; `_total` counts gross
/// mismatches; a SUSTAINED gross mismatch (STREAK) abort()s during the repro so the run stops fast.
pub(crate) const MASK_HEAD_IOU_MIN: usize = 25;
const MASK_HEAD_ABORT_STREAK: usize = 20;
pub(crate) static PROFILE_MASK_HEAD_IOU_LAST: AtomicUsize = AtomicUsize::new(100);
pub(crate) use er_telemetry::counters::PROFILE_MASK_HEAD_MISMATCH_STREAK;
pub(crate) use er_telemetry::counters::PROFILE_MASK_HEAD_MISMATCH_TOTAL;

/// COHERENT color+depth readback cache (bug #3 fix). ONE queue/allocator/list/fence records BOTH the
/// color and depth copies, so they are captured at the SAME GPU submission -- unlike the separate
/// RB_FAST_* (color) and RB_DEPTH_* (depth) paths, between whose independent fences the game's async
/// render can advance the RT (color=frameN, depth=frameN+1 -> the mask shape mismatches the head).
/// Separate readback buffers for color and depth (resized on footprint change). Raw COM owned here.
// The queue/allocator/list/fence stay SINGLE and shared: the render thread WAITS on the fence each frame,
// so the GPU is idle before the next reuse and `allocator.Reset()` remains safe with one allocator. Only
// the readback STAGING BUFFERS ring (Step 2 worker offload), because the worker maps + de-swizzles a slot
// AFTER the wait while the render thread copies the NEXT frame into a DIFFERENT slot's buffers.
pub(crate) use er_telemetry::counters::RB_COH_QUEUE;
pub(crate) use er_telemetry::counters::RB_COH_ALLOC;
pub(crate) use er_telemetry::counters::RB_COH_LIST;
pub(crate) use er_telemetry::counters::RB_COH_FENCE;
pub(crate) use er_telemetry::counters::RB_COH_FENCEVAL;
/// STAGING-BUFFER RING size (Step 2). 3 slots: one being copied-into by the render thread, one (or more)
/// being de-swizzled/consumed by the worker, and headroom so the render thread rarely has to drop.
pub(crate) const RB_COH_RING: usize = 3;
/// Ring slot lifecycle state. FREE = available for the render thread to claim; BUSY = the render thread
/// copied into it (or the worker is consuming it). The render thread claims FREE->BUSY with a CAS; the
/// worker sets it back to FREE after it has finished de-swizzling + publishing (even on panic).
pub(crate) const RB_SLOT_FREE: usize = 0;
pub(crate) const RB_SLOT_BUSY: usize = 1;
pub(crate) static RB_COH_SLOT_STATE: [AtomicUsize; RB_COH_RING] =
    [const { AtomicUsize::new(RB_SLOT_FREE) }; RB_COH_RING];
/// Per-slot color/depth readback staging buffers + their footprint sizes (resized once per slot on the
/// first frame; the RT size is fixed per run). Raw COM owned here (process-lifetime statics).
pub(crate) static RB_COH_CBUF: [AtomicUsize; RB_COH_RING] =
    [const { AtomicUsize::new(0) }; RB_COH_RING];
static RB_COH_CBUFSIZE: [AtomicU64; RB_COH_RING] = [const { AtomicU64::new(0) }; RB_COH_RING];
pub(crate) static RB_COH_DBUF: [AtomicUsize; RB_COH_RING] =
    [const { AtomicUsize::new(0) }; RB_COH_RING];
static RB_COH_DBUFSIZE: [AtomicU64; RB_COH_RING] = [const { AtomicU64::new(0) }; RB_COH_RING];
/// Round-robin frame counter for choosing the next ring slot.
pub(crate) use er_telemetry::counters::RB_COH_FRAME;
/// Frames whose readback was DROPPED because the chosen ring slot was still BUSY (the worker had not
/// finished consuming it). Intended backpressure -- the render thread never blocks. Telemetry.
pub(crate) use er_telemetry::counters::RB_COH_SLOT_BUSY_DROPS;
/// Depth captured COHERENTLY with the current color frame `(dw, dh, depth, depth_cand)`, stashed by
/// `readback_offscreen_fast_coherent` for the SAME draw tick's `apply_depth_alpha_key` to consume via
/// `take_coherent_depth`. Single render-thread producer/consumer within one tick; the producer always
/// sets it (coherent success) or clears it (fallback) each frame, so a later frame never reads a stale
/// depth. `None` -> the mask path reads depth fresh (the legacy separate read).
/// (Step 2: bypassed -- the worker reads depth from the staging slot; left per the design note.)
#[allow(dead_code)]
static COHERENT_DEPTH: Mutex<Option<(u32, u32, Vec<f32>, usize)>> = Mutex::new(None);
/// Instrumentation the first coherent pass lacked: how many draw ticks the COHERENT color+depth readback
/// SUCCEEDED (`_OK`) vs fell back to the separate color+depth path (`_FALLBACK`). Exposed as oracles so a
/// run PROVES whether the single-fence path is actually engaging (not silently degrading).
pub(crate) use er_telemetry::counters::COHERENT_READ_OK;
pub(crate) use er_telemetry::counters::COHERENT_READ_FALLBACK;

/// Cached backbuffer READBACK + UPLOAD buffers for the alpha-honoring CPU-blend composite (sized to the
/// centered portrait region's copyable footprint in the backbuffer's format). The composite reads the live
/// backbuffer region, blends the portrait over it honoring per-pixel alpha (bg alpha 0 => loading screen
/// shows through), and writes the blended region back -- all with the existing COPY primitives, so NO new
/// PSO/shader/RTV pipeline is needed. Owned raw COM pointers (released only on footprint change).
pub(crate) use er_telemetry::counters::OVERLAY_BB_READBACK;
pub(crate) use er_telemetry::counters::OVERLAY_BB_UPLOAD;
pub(crate) use er_telemetry::counters::OVERLAY_BB_BUFSIZE;

/// DETERMINISTICALLY resolve the content RT's vkd3d `ID3D12Resource` from a CSGxTexture by following the
/// FIXED wrapper chain (bd live-portrait-d3d12-resource-buried-in-gx-wrapper-nest, RE'd from a live dump),
/// validating each hop's vtable so a layout change fails closed instead of dereferencing garbage. NO
/// memory scan / QI of arbitrary objects -> nothing to race the teardown free. Returns an AddRef'd ref.
pub(crate) unsafe fn resolve_content_resource_deterministic(
    srv_gx: usize,
) -> Option<ID3D12Resource> {
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
///
/// NOTE (worker offload, 2026-07-06): no longer on the live path -- the coherent readback captures depth
/// with the color on one fence and the render thread hands it to the consume worker, so the separate-fence
/// depth read is unused. Retained as the proven standalone depth-readback for reference.
#[allow(dead_code)]
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
