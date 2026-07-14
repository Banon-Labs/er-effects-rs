
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::{
    D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob, ID3DInclude,
};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D12::{
    D3D_ROOT_SIGNATURE_VERSION_1, D3D12_BLEND_DESC, D3D12_BLEND_INV_SRC_ALPHA,
    D3D12_BLEND_ONE, D3D12_BLEND_OP_ADD, D3D12_BLEND_SRC_ALPHA,
    D3D12_COLOR_WRITE_ENABLE_ALL, D3D12_COMPARISON_FUNC_ALWAYS,
    D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF, D3D12_CULL_MODE_NONE,
    D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING, D3D12_DEPTH_STENCIL_DESC,
    D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
    D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
    D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_DESCRIPTOR_RANGE, D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
    D3D12_GPU_DESCRIPTOR_HANDLE,
    D3D12_DESCRIPTOR_RANGE_TYPE_SRV, D3D12_FILL_MODE_SOLID,
    D3D12_FILTER_MIN_MAG_MIP_LINEAR, D3D12_GRAPHICS_PIPELINE_STATE_DESC,
    D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_DISABLED, D3D12_INPUT_LAYOUT_DESC,
    D3D12_PIPELINE_STATE_FLAG_NONE, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
    D3D12_RASTERIZER_DESC, D3D12_RENDER_TARGET_BLEND_DESC, D3D12_RENDER_TARGET_VIEW_DESC,
    D3D12_RENDER_TARGET_VIEW_DESC_0, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
    D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_ROOT_CONSTANTS, D3D12_ROOT_DESCRIPTOR_TABLE,
    D3D12_ROOT_PARAMETER, D3D12_ROOT_PARAMETER_0, D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
    D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE, D3D12_ROOT_SIGNATURE_DESC,
    D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
    D3D12_RTV_DIMENSION_TEXTURE2D, D3D12_SAMPLER_DESC, D3D12_SHADER_BYTECODE,
    D3D12_SHADER_RESOURCE_VIEW_DESC, D3D12_SHADER_RESOURCE_VIEW_DESC_0,
    D3D12_SHADER_VISIBILITY_PIXEL, D3D12_SRV_DIMENSION_TEXTURE2D,
    D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK, D3D12_STATIC_SAMPLER_DESC,
    D3D12_TEXTURE_ADDRESS_MODE_CLAMP, D3D12_TEX2D_RTV, D3D12_TEX2D_SRV, D3D12_VIEWPORT,
    D3D12SerializeRootSignature, ID3D12DescriptorHeap, ID3D12PipelineState, ID3D12RootSignature,
};
use windows::core::BOOL;

static OVERLAY_ROOT_SIGNATURE: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_PSO: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_SRV_HEAP: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_RTV_HEAP: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_GPU_TEXTURE: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_GPU_UPLOAD: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_GPU_UPLOAD_SIZE: AtomicU64 = AtomicU64::new(0);
static OVERLAY_GPU_TEX_W: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_GPU_TEX_H: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_GPU_TEX_STATE: AtomicUsize = AtomicUsize::new(0); // 0=COPY_DEST/unknown, 1=PIXEL_SHADER_RESOURCE
static OVERLAY_GPU_TEX_VERSION: AtomicUsize = AtomicUsize::new(usize::MAX);
static OVERLAY_TEXT_GPU_TEXTURE: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_TEXT_GPU_UPLOAD: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_TEXT_GPU_UPLOAD_SIZE: AtomicU64 = AtomicU64::new(0);
static OVERLAY_TEXT_GPU_TEX_W: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_TEXT_GPU_TEX_H: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_TEXT_GPU_TEX_STATE: AtomicUsize = AtomicUsize::new(0); // 0=COPY_DEST/unknown, 1=PIXEL_SHADER_RESOURCE
static OVERLAY_TEXT_GPU_TEX_VERSION: AtomicUsize = AtomicUsize::new(usize::MAX);
static OVERLAY_PSO_FORMAT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OVERLAY_GPU_FAIL_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OVERLAY_GPU_FAIL_CODE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OVERLAY_GPU_FAIL_VERSION: AtomicUsize = AtomicUsize::new(0);
/// Loading-screen build currently accepted by Present. When this changes, any already-published portrait
/// snapshot is stale/previous-window content (often a different source resolution), so Present holds until
/// a later live publish bumps `LOADING_BG_PORTRAIT_RGBA_VERSION` for this build.
static OVERLAY_LOADSCREEN_BUILD_SEEN: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_LOADSCREEN_BASELINE_VERSION: AtomicUsize = AtomicUsize::new(0);
/// Native loading-bar final hits already consumed by this overlay window. Reset on re-arm so a prior
/// load's terminal-frame latch cannot instantly affect the next loading portrait window.
static OVERLAY_NATIVE_BAR_FINAL_HITS_SEEN: AtomicUsize = AtomicUsize::new(0);
/// Native LoadingScreen close/result hits already consumed by this overlay window. This fires after the
/// post-100%-bar countdown and is the preferred stop for matching the visible loading-screen teardown.
static OVERLAY_NATIVE_CLOSE_HITS_SEEN: AtomicUsize = AtomicUsize::new(0);
/// Present counter for throttling the head pixel-oracle while the overlay is demoted.
static OVERLAY_HEAD_PROBE_TICK: AtomicUsize = AtomicUsize::new(0);

const OVERLAY_SHADER_HLSL: &[u8] = br#"
Texture2D portrait_tex : register(t0);
SamplerState portrait_sampler : register(s0);
cbuffer OverlayConstants : register(b0) {
    float4 uv_scale_bias;
};
struct VsOut {
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
};
VsOut vs_main(uint id : SV_VertexID) {
    float2 pos;
    if (id == 0) {
        pos = float2(-1.0, -1.0);
    } else if (id == 1) {
        pos = float2(-1.0, 3.0);
    } else {
        pos = float2(3.0, -1.0);
    }
    VsOut o;
    o.pos = float4(pos, 0.0, 1.0);
    o.uv = float2(pos.x * 0.5 + 0.5, 0.5 - pos.y * 0.5);
    return o;
}
float4 ps_main(VsOut input) : SV_Target {
    float2 uv = input.uv * uv_scale_bias.xy + uv_scale_bias.zw;
    return portrait_tex.Sample(portrait_sampler, uv);
}
"#;

/// One-time setup for the per-frame composite: derive the device from the backbuffer, create the
/// persistent command objects, and build a tiny GPU alpha-composite pipeline. We do NOT submit on the
/// game's command queue -- doing so from the Present hook caused a vkd3d access violation; instead we
/// CPU-fence-wait our private queue before the original Present runs.
unsafe fn init_overlay_draw_state(backbuffer: &ID3D12Resource) -> bool {
    let mut device_opt: Option<ID3D12Device> = None;
    if unsafe { backbuffer.GetDevice(&mut device_opt) }.is_err() {
        return false;
    }
    let Some(device) = device_opt else {
        return false;
    };
    let bb_desc = unsafe { backbuffer.GetDesc() };

    let (pw, ph, pixels_len) = {
        let Ok(g) = LOADING_BG_PORTRAIT_RGBA.lock() else {
            return false;
        };
        match g.as_ref() {
            Some((w, h, px)) => (*w, *h, px.len()),
            None => return false,
        }
    };
    if pw == 0 || ph == 0 || pw > MAX_RT_DIM || ph > MAX_RT_DIM {
        return false;
    }
    if pixels_len < (pw as usize) * (ph as usize) * RGBA8_BPP {
        return false;
    }
    append_autoload_debug(format_args!(
        "present-overlay: GPU init step1 device + portrait ok ({pw}x{ph}, {pixels_len} bytes, bb_format={})",
        bb_desc.Format.0
    ));

    let Some(root_sig) = (unsafe { create_overlay_root_signature(&device) }) else {
        append_autoload_debug(format_args!("present-overlay: GPU init root signature failed"));
        return false;
    };
    let Some(pso) = (unsafe { create_overlay_pso(&device, &root_sig, bb_desc.Format) }) else {
        append_autoload_debug(format_args!("present-overlay: GPU init PSO failed"));
        return false;
    };
    let srv_heap_desc = D3D12_DESCRIPTOR_HEAP_DESC {
        Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
        // descriptor 0 = animated portrait, descriptor 1 = independently rasterized stats text.
        NumDescriptors: 2,
        Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
        NodeMask: 0,
    };
    let Ok(srv_heap) = (unsafe {
        device.CreateDescriptorHeap::<ID3D12DescriptorHeap>(&srv_heap_desc)
    }) else {
        return false;
    };
    let rtv_heap_desc = D3D12_DESCRIPTOR_HEAP_DESC {
        Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
        NumDescriptors: 1,
        Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
        NodeMask: 0,
    };
    let Ok(rtv_heap) = (unsafe {
        device.CreateDescriptorHeap::<ID3D12DescriptorHeap>(&rtv_heap_desc)
    }) else {
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
            Some(&pso),
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
    append_autoload_debug(format_args!(
        "present-overlay: GPU init step3 root/pso/descriptors/cmd objects + own queue ready"
    ));

    OVERLAY_ROOT_SIGNATURE.store(root_sig.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_PSO.store(pso.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_SRV_HEAP.store(srv_heap.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_RTV_HEAP.store(rtv_heap.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_ALLOCATOR.store(allocator.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_LIST.store(list.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_FENCE.store(fence.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_QUEUE.store(queue.into_raw() as usize, Ordering::SeqCst);
    OVERLAY_PORTRAIT_W.store(pw as usize, Ordering::SeqCst);
    OVERLAY_PORTRAIT_H.store(ph as usize, Ordering::SeqCst);
    OVERLAY_PSO_FORMAT.store(bb_desc.Format.0 as usize, Ordering::SeqCst);
    true
}

unsafe fn create_overlay_root_signature(device: &ID3D12Device) -> Option<ID3D12RootSignature> {
    let range = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 1,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
    };
    let table = D3D12_ROOT_DESCRIPTOR_TABLE {
        NumDescriptorRanges: 1,
        pDescriptorRanges: &range,
    };
    let params = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: table,
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    Num32BitValues: 4,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        },
    ];
    let sampler = D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D12_COMPARISON_FUNC_ALWAYS,
        BorderColor: D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
        MinLOD: 0.0,
        MaxLOD: f32::MAX,
        ShaderRegister: 0,
        RegisterSpace: 0,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    };
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: params.len() as u32,
        pParameters: params.as_ptr(),
        NumStaticSamplers: 1,
        pStaticSamplers: &sampler,
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
    };
    let mut blob: Option<ID3DBlob> = None;
    let mut err: Option<ID3DBlob> = None;
    if unsafe {
        D3D12SerializeRootSignature(
            &desc,
            D3D_ROOT_SIGNATURE_VERSION_1,
            &mut blob,
            Some(&mut err),
        )
    }
    .is_err()
    {
        log_shader_error("root-signature", err.as_ref());
        return None;
    }
    let blob = blob?;
    let bytes = unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
    };
    unsafe { device.CreateRootSignature::<ID3D12RootSignature>(0, bytes).ok() }
}

unsafe fn create_overlay_pso(
    device: &ID3D12Device,
    root_sig: &ID3D12RootSignature,
    bb_format: DXGI_FORMAT,
) -> Option<ID3D12PipelineState> {
    let vs = unsafe { compile_overlay_shader(b"vs_main\0", b"vs_5_0\0") }?;
    let ps = unsafe { compile_overlay_shader(b"ps_main\0", b"ps_5_0\0") }?;
    let mut blend = D3D12_BLEND_DESC::default();
    blend.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
        BlendEnable: BOOL(1),
        LogicOpEnable: BOOL(0),
        SrcBlend: D3D12_BLEND_SRC_ALPHA,
        DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_ONE,
        DestBlendAlpha: D3D12_BLEND_INV_SRC_ALPHA,
        BlendOpAlpha: D3D12_BLEND_OP_ADD,
        LogicOp: Default::default(),
        RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    let mut rtv_formats = [DXGI_FORMAT_UNKNOWN; 8];
    rtv_formats[0] = bb_format;
    let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        pRootSignature: ManuallyDrop::new(Some(root_sig.clone())),
        VS: D3D12_SHADER_BYTECODE {
            pShaderBytecode: unsafe { vs.GetBufferPointer() },
            BytecodeLength: unsafe { vs.GetBufferSize() },
        },
        PS: D3D12_SHADER_BYTECODE {
            pShaderBytecode: unsafe { ps.GetBufferPointer() },
            BytecodeLength: unsafe { ps.GetBufferSize() },
        },
        BlendState: blend,
        SampleMask: u32::MAX,
        RasterizerState: D3D12_RASTERIZER_DESC {
            FillMode: D3D12_FILL_MODE_SOLID,
            CullMode: D3D12_CULL_MODE_NONE,
            FrontCounterClockwise: BOOL(0),
            DepthBias: 0,
            DepthBiasClamp: 0.0,
            SlopeScaledDepthBias: 0.0,
            DepthClipEnable: BOOL(0),
            MultisampleEnable: BOOL(0),
            AntialiasedLineEnable: BOOL(0),
            ForcedSampleCount: 0,
            ConservativeRaster: D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
        },
        DepthStencilState: D3D12_DEPTH_STENCIL_DESC::default(),
        InputLayout: D3D12_INPUT_LAYOUT_DESC::default(),
        IBStripCutValue: D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_DISABLED,
        PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        NumRenderTargets: 1,
        RTVFormats: rtv_formats,
        DSVFormat: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Flags: D3D12_PIPELINE_STATE_FLAG_NONE,
        ..Default::default()
    };
    unsafe { device.CreateGraphicsPipelineState::<ID3D12PipelineState>(&desc).ok() }
}

unsafe fn compile_overlay_shader(entry: &'static [u8], target: &'static [u8]) -> Option<ID3DBlob> {
    let mut code: Option<ID3DBlob> = None;
    let mut err: Option<ID3DBlob> = None;
    if unsafe {
        D3DCompile(
            OVERLAY_SHADER_HLSL.as_ptr() as *const c_void,
            OVERLAY_SHADER_HLSL.len(),
            PCSTR::from_raw(b"er-effects-present-overlay\0".as_ptr()),
            None,
            None::<&ID3DInclude>,
            PCSTR::from_raw(entry.as_ptr()),
            PCSTR::from_raw(target.as_ptr()),
            0,
            0,
            &mut code,
            Some(&mut err),
        )
    }
    .is_err()
    {
        log_shader_error(core::str::from_utf8(entry).unwrap_or("shader"), err.as_ref());
        return None;
    }
    code
}

fn overlay_gpu_fail(code: usize, msg: &str, cur_ver: usize) -> bool {
    OVERLAY_GPU_FAIL_CODE.store(code, Ordering::SeqCst);
    OVERLAY_GPU_FAIL_VERSION.store(cur_ver, Ordering::SeqCst);
    let n = OVERLAY_GPU_FAIL_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    if n <= 16 || n % 128 == 0 {
        append_autoload_debug(format_args!(
            "present-overlay: GPU composite failed code={code} count={n} version={cur_ver} msg={msg}"
        ));
    }
    false
}

fn log_shader_error(stage: &str, err: Option<&ID3DBlob>) {
    if let Some(err) = err {
        let ptr = unsafe { err.GetBufferPointer() } as *const u8;
        let len = unsafe { err.GetBufferSize() };
        if !ptr.is_null() && len > 0 {
            let bytes = unsafe { std::slice::from_raw_parts(ptr, len.min(512)) };
            let msg = core::str::from_utf8(bytes).unwrap_or("<non-utf8 shader error>");
            append_autoload_debug(format_args!("present-overlay: {stage} compile error: {msg}"));
            return;
        }
    }
    append_autoload_debug(format_args!("present-overlay: {stage} compile/serialize failed"));
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
    // WORKER-OFFLOAD SWITCH SAFETY (2026-07-06). Bump the pipeline generation FIRST: any portrait consume
    // job still in flight on the worker thread snapshotted the PREVIOUS gen, so when it re-reads this before
    // it pins/publishes it will see the bump and DISCARD -- a head captured for the old window can never be
    // pinned/published into the new one.
    PORTRAIT_PIPELINE_GEN.fetch_add(1, Ordering::SeqCst);
    // Then bounded-drain the in-flight consume jobs (up to ~15ms, yielding) so late telemetry lands in the
    // right window and no worker is mid-publish while we clear the state below. This reset already runs off
    // the render thread (see the note further down), so a short spin here is safe.
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(15);
        while PORTRAIT_JOB_INFLIGHT.load(Ordering::SeqCst) != 0
            && std::time::Instant::now() < deadline
        {
            std::thread::yield_now();
        }
    }
    // CLEAR-ON-COMPLETE (user 2026-07-06, REVERSING the 2026-07-03 make-before-break KEEP): drop the
    // published head snapshot the moment the load completes (character in-world, native bar terminal).
    // The kept bridge was the stale-content reservoir behind the second-load wrong-head bug: the next
    // window's forge baked the PREVIOUS character's held frame into the now-loading background at decode
    // time (the bake is decode-once; maybe_update_gfx_loading_portrait is a no-op), so the old head
    // stayed on screen for the whole next load even while the readback/publish pipeline was proven
    // (pixel-diff vs same-character baseline, runs 2026-07-06) to produce the NEW character. With the
    // snapshot cleared here there is nothing stale to bake or bridge: the next window starts head-less
    // and shows the new character's first keyed frame. Costs a brief head-less loading screen
    // (~0.5s after the window's table build in both measured runs) -- preferred over a wrong head.
    if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
        *g = None;
    }
    PROFILE_HAVE_KEYED_FRAME.store(0, Ordering::SeqCst);
    PROFILE_BAKE_RGBA_CAPTURED.store(0, Ordering::SeqCst);
    PROFILE_LOADSCREEN_TABLE_OWNED.store(0, Ordering::SeqCst);
    PROFILE_LOADSCREEN_REPAIR_REBUILT.store(0, Ordering::SeqCst);
    // Candidate A (er-effects-rs-jsm): drop the demote credit + stash the cached CSTextureImage ref for
    // the game-thread updater to Release (this reset runs off the game thread; Scaleform frees must not).
    gfx_loading_portrait_window_reset();
    // Rebuild the stats text for the next load (a System-Quit character switch may load a different char).
    stats_text_window_reset();
    PROFILE_RT_PIN.store(0, Ordering::SeqCst);
    PROFILE_DEPTH_PIN.store(0, Ordering::SeqCst);
    // Fresh adaptive tear baseline for the next window's character (honest content scores differ
    // per character: speckled textures sit ~40, smooth skin ~3).
    PROFILE_TEAR_EMA.store(0, Ordering::SeqCst);
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
    // `PROFILE_RT_SRV_COPIES_WINDOW` is a true per-window counter used by the fast-fail gate. Reset it
    // here with the other per-window counters; leaving stale copies from an earlier window makes a later
    // no-copy failure report as cause=0/unknown instead of the actionable no-copy class.
    let copies = PROFILE_RT_SRV_COPIES_WINDOW.swap(0, Ordering::SeqCst);
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
    // Source provenance per window: cb/cs = color ticks resolved from the scene bundle vs the scan;
    // dc/db = depth via the deterministic chain vs the BFS; unpaired = real frames held back for
    // lacking bundle provenance (the green-face wrong-buffer class). A starved window (clean=0)
    // with cs/db dominant convicts a chain miss for that window's renderer.
    let cb = winof(
        &PROFILE_COLOR_FROM_BUNDLE,
        &PROFILE_COLOR_FROM_BUNDLE_WINDOW_MARK,
    );
    let cs = winof(
        &PROFILE_COLOR_FROM_SCAN,
        &PROFILE_COLOR_FROM_SCAN_WINDOW_MARK,
    );
    let dc = winof(
        &PROFILE_DEPTH_FROM_CHAIN,
        &PROFILE_DEPTH_FROM_CHAIN_WINDOW_MARK,
    );
    let db = winof(&PROFILE_DEPTH_FROM_BFS, &PROFILE_DEPTH_FROM_BFS_WINDOW_MARK);
    let unpaired = winof(
        &PROFILE_PUBLISH_SKIPPED_UNPAIRED,
        &PROFILE_PUBLISH_SKIPPED_UNPAIRED_WINDOW_MARK,
    );
    let lowmask = winof(
        &PROFILE_PUBLISH_SKIPPED_LOWMASK,
        &PROFILE_PUBLISH_SKIPPED_LOWMASK_WINDOW_MARK,
    );
    // First-keyed latency: display-frame index of this window's first publish ('-' = never
    // published; the whole window rode the bridge). Snapshot + re-arm for the next window.
    let first_keyed = PROFILE_WINDOW_FIRST_KEYED_DISPLAY.swap(usize::MAX, Ordering::SeqCst);
    PROFILE_WINDOW_FIRST_KEYED_DISPLAY_LAST.store(
        if first_keyed == usize::MAX {
            0
        } else {
            first_keyed
        },
        Ordering::SeqCst,
    );
    let first_keyed_s = if first_keyed == usize::MAX {
        "-".to_owned()
    } else {
        first_keyed.to_string()
    };
    // Floor-evidence: min transparent share among floor-passing frames vs max among lowmask-held
    // frames this window ('-' = no frame in that class). Sets PORTRAIT_MIN_TRANSPARENT_PCT from data.
    let share_min = PROFILE_PUBLISH_SHARE_MIN.swap(usize::MAX, Ordering::SeqCst);
    let share_min_s = if share_min == usize::MAX {
        "-".to_owned()
    } else {
        share_min.to_string()
    };
    let held_max = PROFILE_LOWMASK_SHARE_MAX.swap(0, Ordering::SeqCst);
    let checker = winof(
        &PROFILE_READBACK_CHECKER,
        &PROFILE_READBACK_CHECKER_WINDOW_MARK,
    );
    let badiou = winof(
        &PROFILE_PUBLISH_SKIPPED_BADIOU,
        &PROFILE_PUBLISH_SKIPPED_BADIOU_WINDOW_MARK,
    );
    // HARNESS-FAILURE semaphore (user directive 2026-07-06): a window that DROVE the model (produced
    // readback frames) yet published ZERO clean portraits is a broken feature for that character, not an
    // acceptable silent skip. The FAST-FAIL in the draw tick trips this mid-window (grace=0) so it fires
    // the frame the render misses, not here at window close. This is the BACKSTOP: it records the
    // precise per-window dominant cause for the log, and only increments the failure counter if the
    // fast-fail latch did not already count this window (defensive; ~never with grace=0). Guarded on
    // `drive > 0` -- a window that never got a model (build-side gap) is not a publish-gate fault.
    if published == 0 && drive > 0 {
        let cause = if torn >= unkeyed && torn >= badiou && torn >= lowmask && torn > 0 {
            1 // torn: usable frames the tear metric rejected
        } else if unkeyed >= badiou && unkeyed >= lowmask && unkeyed > 0 {
            2 // unkeyed: depth mask never cut background (opaque/black RT)
        } else if badiou >= lowmask && badiou > 0 {
            3
        } else if lowmask > 0 {
            4
        } else if checker > 0 {
            5
        } else if multi > 0 {
            6
        } else if unpaired > 0 {
            7
        } else if copies == 0 {
            8
        } else if cb + cs + dc + db == 0 {
            9
        } else {
            0
        };
        PORTRAIT_WINDOW_PUBLISH_FAIL_CAUSE.store(cause, Ordering::SeqCst);
        let already = PORTRAIT_WINDOW_PUBLISH_FAIL_LATCHED.load(Ordering::SeqCst) != 0;
        let n = if already {
            PORTRAIT_WINDOW_PUBLISH_FAILURES.load(Ordering::SeqCst)
        } else {
            PORTRAIT_WINDOW_PUBLISH_FAILURES.fetch_add(1, Ordering::SeqCst) + 1
        };
        append_autoload_debug(format_args!(
            "present-overlay: PORTRAIT PUBLISH FAILURE #{n}{} -- window drove {drive} frames but published 0 (dominant cause={} torn={torn} unkeyed={unkeyed} badiou={badiou} lowmask={lowmask} checker={checker} multi={multi} unpaired={unpaired} copies={copies} cb={cb} cs={cs} dc={dc} db={db}); HARNESS MUST FAIL until the root render is fixed",
            if already { " (already fast-failed)" } else { "" },
            match cause { 1 => "torn", 2 => "unkeyed", 3 => "badiou", 4 => "lowmask", 5 => "checker", 6 => "multi", 7 => "unpaired", 8 => "no-copy", 9 => "no-provenance", _ => "unknown" }
        ));
    }
    // Re-arm the per-window fast-fail state for the next window.
    PROFILE_PUBLISH_CLEAN_WINDOW.store(0, Ordering::SeqCst);
    PORTRAIT_WINDOW_PUBLISH_FAIL_LATCHED.store(0, Ordering::SeqCst);
    PORTRAIT_LAST_SKIP_CLASS.store(0, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "present-overlay: loading-portrait window reset ({reason}) -- animated {drive} / displayed {display} frames (drive<<display == froze early); publish[clean={published} torn={torn} unkeyed={unkeyed} lowmask={lowmask} badiou={badiou} checker={checker} multi={multi} pin_moves={pin_moves} fence_skips={fence_skips} unpaired={unpaired} copies={copies} first_keyed={first_keyed_s}] share[pass_min={share_min_s} held_max={held_max}] src[color bundle={cb}/scan={cs} depth chain={dc}/bfs={db}] (clean=0 with drive>0 == PUBLISH FAILURE, see the failure line above; the dominant skip class is the cause); pins/spare cleared for the next load"
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
        // Stopped: re-arm ONLY on evidence of a NEW loading window. For native terminal stops
        // (reason=4 legacy bar-final, reason=5 close/result), do NOT re-arm just because `loading` still
        // reads true: the same native loading window can linger during the menu result handoff. A fresh
        // post-Continue table build is the reliable new-window proof.
        let rebuilt = PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst)
            > OVERLAY_STOP_TABLE_BUILDS.load(Ordering::SeqCst);
        let stop_reason = OVERLAY_STOP_REASON.load(Ordering::SeqCst);
        let stopped_on_native_terminal = stop_reason == 4 || stop_reason == 5;
        if !rebuilt && (stopped_on_native_terminal || !loading) {
            return false;
        }
        OVERLAY_STOPPED.store(0, Ordering::SeqCst);
        OVERLAY_NOW_LOADING_SEEN.store(if loading { 1 } else { 0 }, Ordering::SeqCst);
        OVERLAY_BRIDGE_PRESENTS.store(0, Ordering::SeqCst);
        OVERLAY_WORLD_STOP_LOGGED.store(0, Ordering::SeqCst);
        OVERLAY_NATIVE_BAR_FINAL_HITS_SEEN.store(
            LOADING_SCREEN_BAR_FINAL_HITS.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        OVERLAY_NATIVE_CLOSE_HITS_SEEN.store(
            LOADING_SCREEN_CLOSE_SENT_HITS.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        append_autoload_debug(format_args!(
            "present-overlay: re-armed for a new loading window (loading={loading} rebuilt={rebuilt})"
        ));
    }
    // PRODUCT STOP: wait for the native LoadingScreen close/result handoff, not the earlier Gauge_3
    // terminal frame. Static RE (2026-07-13): `CS::LoadingScreen::Update` reaches Gauge_3 max first,
    // then after its own countdown calls the MenuWindow result callback, resets LoadingScreenData, and
    // latches a finish-sent byte. That later edge tracks the visible loading-screen teardown much more
    // closely, narrowing the user's "portrait disappears before bar/screen disappears" gap.
    let native_close_hits = LOADING_SCREEN_CLOSE_SENT_HITS.load(Ordering::SeqCst);
    let native_close_seen = OVERLAY_NATIVE_CLOSE_HITS_SEEN.load(Ordering::SeqCst);
    if native_close_hits > native_close_seen {
        OVERLAY_STOPPED.store(1, Ordering::SeqCst);
        OVERLAY_STOP_TABLE_BUILDS.store(
            PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst),
            Ordering::SeqCst,
        );
        OVERLAY_WINDOW_STOPS.fetch_add(1, Ordering::SeqCst);
        OVERLAY_STOP_REASON.store(5, Ordering::SeqCst);
        OVERLAY_NATIVE_CLOSE_HITS_SEEN.store(native_close_hits, Ordering::SeqCst);
        let frame = LOADING_SCREEN_BAR_CURRENT_FRAME.load(Ordering::SeqCst);
        let max = LOADING_SCREEN_BAR_MAX_FRAME.load(Ordering::SeqCst);
        let progress = LOADING_SCREEN_BAR_PROGRESS_PERMILLE.load(Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "present-overlay: native LoadingScreen close/result sent ({frame}/{max}, progress={progress}permille) -> stopped compositing loading portrait"
        ));
        loading_portrait_window_reset("native loading screen close/result");
        return false;
    }
    // Keep the bar-final edge as telemetry only. It is earlier than the native close/result handoff and
    // caused the visible early-pop gap the user reported, so it is no longer a product stop.
    OVERLAY_NATIVE_BAR_FINAL_HITS_SEEN.store(
        LOADING_SCREEN_BAR_FINAL_HITS.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    // FALLBACK STOP: `load_done && !fake_vis` fires before the user-visible loading surface has fully
    // handed off on some runs, so bridge it; the TimeAct stop above is preferred when it appears first.
    let post_load_bridge = loading_seen && !loading;
    if post_load_bridge {
        let n = OVERLAY_BRIDGE_PRESENTS.fetch_add(1, Ordering::SeqCst) + 1;
        if n < OVERLAY_LOAD_DONE_VISIBLE_BRIDGE_PRESENTS {
            if n == 1 {
                append_autoload_debug(format_args!(
                    "present-overlay: load_done+!fake_vis observed; bridging visible loading hand-off for up to {OVERLAY_LOAD_DONE_VISIBLE_BRIDGE_PRESENTS} presents"
                ));
            }
        } else {
            OVERLAY_STOPPED.store(1, Ordering::SeqCst);
            OVERLAY_STOP_TABLE_BUILDS.store(
                PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
            OVERLAY_WINDOW_STOPS.fetch_add(1, Ordering::SeqCst);
            OVERLAY_STOP_REASON.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "present-overlay: load completed bridge elapsed ({n} presents after load_done+!fake_vis) -> stopped compositing"
            ));
            loading_portrait_window_reset("load completed bridge elapsed");
            return false;
        }
    }
    // ANTI-RUNAWAY BACKSTOP: pathological case where the load reports done AND we're in-world but the
    // primary bridge/stop never engaged. Count in-world+load_done frames; force a stop past a huge budget
    // so the head can't composite forever. reason=3 flags the assumption broke.
    if !post_load_bridge && load_done && IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES {
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
    let _forge_committed = LOADING_BG_TEXTURE_REDIRECT_COMMITS.load(Ordering::SeqCst) > 0;
    // Current-source gate: do NOT composite during the loose pre-build `loading` interval. That interval can
    // still hold a previous/profile-select 256x256 snapshot while the current loading-cover renderer has not
    // been built yet; drawing it caused a visible resolution swap before the live 56x56 source took over.
    // The loading portrait belongs to the loadscreen renderer window, so require that window's table build.
    let load_builds = PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst);
    let loadscreen_active = PROFILE_LOADSCREEN_TABLE_OWNED.load(Ordering::SeqCst) != 0;
    if !loadscreen_active {
        return false;
    }
    let cur_ver = LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst);
    if loadscreen_active {
        let has_existing_portrait = LOADING_BG_PORTRAIT_RGBA
            .lock()
            .map(|g| {
                g.as_ref().is_some_and(|(w, h, px)| {
                    *w != 0 && *h != 0 && px.len() >= (*w as usize) * (*h as usize) * RGBA8_BPP
                })
            })
            .unwrap_or(false);
        let seen = OVERLAY_LOADSCREEN_BUILD_SEEN.load(Ordering::SeqCst);
        if seen != load_builds {
            OVERLAY_LOADSCREEN_BUILD_SEEN.store(load_builds, Ordering::SeqCst);
            OVERLAY_LOADSCREEN_BASELINE_VERSION.store(cur_ver, Ordering::SeqCst);
            if has_existing_portrait {
                append_autoload_debug(format_args!(
                    "present-overlay: loadscreen build {load_builds} started; drawing frozen existing portrait until source version advances past {cur_ver}"
                ));
            } else {
                append_autoload_debug(format_args!(
                    "present-overlay: loadscreen build {load_builds} started; holding blank until source version advances past {cur_ver} (no existing portrait snapshot)"
                ));
                return false;
            }
        }
        if cur_ver <= OVERLAY_LOADSCREEN_BASELINE_VERSION.load(Ordering::SeqCst)
            && !has_existing_portrait
        {
            return false;
        }
    }
    // CANDIDATE A HANDOFF (er-effects-rs-jsm): demote the overlay only when the native now-loading movie
    // has a LIVE per-frame head update path. A one-shot baked background is static by construction; using
    // the backbuffer head-match oracle alone made the overlay yield to that static image, which froze the
    // character exactly when the native loading screen appeared. Until a real in-movie update path bumps
    // `GFX_PORTRAIT_UPLOADS`, the overlay remains the animated product surface and keeps drawing the live
    // head as a bridge.
    if GFX_PORTRAIT_BAKED.load(Ordering::SeqCst) > 0
        && GFX_PORTRAIT_UPLOADS.load(Ordering::SeqCst) > 0
    {
        let tick = OVERLAY_HEAD_PROBE_TICK.fetch_add(1, Ordering::SeqCst);
        if tick % 30 == 0 {
            let sc_raw = swapchain_raw as *mut c_void;
            if let Some(sc) = unsafe { IDXGISwapChain3::from_raw_borrowed(&sc_raw) } {
                let idx = unsafe { sc.GetCurrentBackBufferIndex() };
                if let Ok(bb) = unsafe { sc.GetBuffer::<ID3D12Resource>(idx) } {
                    let d = unsafe { bb.GetDesc() };
                    unsafe { probe_head_on_screen(&bb, d.Format) };
                }
            }
        }
        if GFX_PORTRAIT_HEAD_ON_SCREEN.load(Ordering::SeqCst) == 1 {
            GFX_PORTRAIT_OVERLAY_YIELDS.fetch_add(1, Ordering::SeqCst);
            return true;
        }
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
    // Fill the whole viewable backbuffer. The portrait alpha/mask is scaled to the same full-screen
    // bounds, so the clip region is the entire visible loading-screen surface instead of a centered
    // source-sized rectangle that can leave uncovered borders at non-portrait resolutions.
    let cw = bw;
    let ch = bh;
    let dx = 0;
    let dy = 0;
    // Stats text is deliberately NOT baked into `spx`: keep the animated portrait at its current lower
    // render/readback resolution, then rasterize the text at actual backbuffer scale and draw it as a
    // second overlay texture so text sharpness no longer depends on portrait RT size.
    let stats_text = stats_text_screen_bitmap(bw.max(bh));

    // Alpha-honoring GPU composite: upload the latest CPU-published portrait RGBA into a tiny GPU texture
    // on version changes, then draw one full-screen triangle over the live loading-screen backbuffer with
    // standard src-alpha blending. This preserves transparency without reading/blending the 4K backbuffer
    // on the CPU. If stats text is available, a second alpha-blended draw overlays it at screen resolution.
    if !unsafe {
        gpu_composite_portrait_over_backbuffer(
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
            sh,
            &spx,
            cur_ver,
            stats_text.as_ref(),
        )
    } {
        return false;
    }

    // OVERLAY-LANDING PIXEL SEMAPHORE (user QA 2026-07-06: window 2 published the NEW character's
    // frames with 352 fresh uploads and zero GPU failures, yet the user saw the OLD character -- no
    // oracle ever proved OUR draw reaches the visible frame). Probe the backbuffer AFTER our composite
    // (throttled): the head-match here includes the overlay's own draw, so a high match proves the
    // overlay head lands on the presented frame; a low match alongside fresh uploads proves it does
    // not. Mutually exclusive with the candidate-A pre-draw probe (GFX_PORTRAIT_BAKED > 0), which owns
    // the demote decision and must not see post-draw frames.
    if GFX_PORTRAIT_BAKED.load(Ordering::SeqCst) == 0 {
        let tick = OVERLAY_HEAD_PROBE_TICK.fetch_add(1, Ordering::SeqCst);
        if tick % 30 == 0 {
            unsafe { probe_head_on_screen(&backbuffer, bb_desc.Format) };
        }
    }
    // Playback-rate semaphores: draw timing proves how often the overlay reached the swapchain; reupload
    // timing proves how often a distinct source portrait frame reached that overlay; stale-run proves visible
    // held frames when the same portrait source is reused across consecutive presents.
    let now_ms = overlay_timing_ms();
    let _ = OVERLAY_DRAW_FIRST_MS.compare_exchange(0, now_ms, Ordering::SeqCst, Ordering::SeqCst);
    OVERLAY_DRAW_LAST_MS.store(now_ms, Ordering::SeqCst);
    // Preserve the "displayed head updates per frame" oracle (oracle_overlay_reuploads): count a fresh
    // published version reaching the screen.
    let prev_ver = OVERLAY_PORTRAIT_VERSION.swap(cur_ver, Ordering::SeqCst);
    if cur_ver != prev_ver {
        OVERLAY_REUPLOADS.fetch_add(1, Ordering::SeqCst);
        let _ = OVERLAY_REUPLOAD_FIRST_MS.compare_exchange(0, now_ms, Ordering::SeqCst, Ordering::SeqCst);
        OVERLAY_REUPLOAD_LAST_MS.store(now_ms, Ordering::SeqCst);
        OVERLAY_STALE_PRESENT_RUN.store(0, Ordering::SeqCst);
    } else {
        let stale = OVERLAY_STALE_PRESENT_RUN.fetch_add(1, Ordering::SeqCst) + 1;
        update_atomic_max(&OVERLAY_STALE_PRESENT_MAX, stale);
    }
    let hits = OVERLAY_DRAW_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if hits == 1 {
        append_autoload_debug(format_args!(
            "present-overlay: portrait GPU alpha-composited onto full backbuffer {bw}x{bh} (source {sw}x{sh}, aspect-cover scale/crop, depth-alpha-keyed bg)"
        ));
    }
    true
}

fn overlay_timing_ms() -> usize {
    let mut guard = match OVERLAY_TIMING_EPOCH.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let epoch = guard.get_or_insert_with(std::time::Instant::now);
    epoch.elapsed().as_millis() as usize + 1
}

fn update_atomic_max(slot: &AtomicUsize, value: usize) {
    let mut cur = slot.load(Ordering::SeqCst);
    while value > cur {
        match slot.compare_exchange(cur, value, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => break,
            Err(next) => cur = next,
        }
    }
}

fn sample_portrait_rgba_cover(
    spx: &[u8],
    sw: usize,
    sh: usize,
    x: usize,
    y: usize,
    out_w: usize,
    out_h: usize,
) -> (u32, u32, u32, u32) {
    if sw == 0 || sh == 0 || out_w == 0 || out_h == 0 {
        return (0, 0, 0, 0);
    }
    // Aspect-cover, single-sample mapping: preserve the portrait aspect ratio, scale until the entire
    // destination is covered, and crop the excess in source space. This is deliberately NOT stretch and
    // deliberately NOT supersampling; the user wants the FPS/fit experiment without the 4x cost.
    let scale = (out_w as f32 / sw as f32).max(out_h as f32 / sh as f32);
    let visible_w = out_w as f32 / scale;
    let visible_h = out_h as f32 / scale;
    let src_x = ((x as f32 + 0.5) / out_w as f32) * visible_w + (sw as f32 - visible_w) * 0.5;
    let src_y = ((y as f32 + 0.5) / out_h as f32) * visible_h + (sh as f32 - visible_h) * 0.5;
    let sx = (src_x.floor() as usize).min(sw - 1);
    let sy = (src_y.floor() as usize).min(sh - 1);
    let so = (sy * sw + sx) * RGBA8_BPP;
    if so + 4 > spx.len() {
        return (0, 0, 0, 0);
    }
    (
        spx[so] as u32,
        spx[so + 1] as u32,
        spx[so + 2] as u32,
        spx[so + 3] as u32,
    )
}

/// Normalized (0..1 of the 1920x1080 stage == the 16:9 backbuffer) tip-text + loading-bar rectangles,
/// from the VERIFIED asset geometry (er-effects-rs-jsm). The head probe EXCLUDES these so the native tips
/// (drawn over the head) can't be mistaken for "head absent".
fn in_tip_or_bar_rect(u: f32, v: f32) -> bool {
    let r = |u0: f32, u1: f32, v0: f32, v1: f32| u >= u0 && u <= u1 && v >= v0 && v <= v1;
    r(0.0583, 0.6365, 0.6444, 0.6889) // tip title  (112..1222, 696..744)
        || r(0.0583, 0.6365, 0.6935, 1.0) // tip body   (112..1222, 749..1159 clamped)
        || r(0.0583, 0.6573, 0.9324, 0.9685) // key guide  (112..1262, 1007..1046)
        || r(0.7339, 0.9401, 0.9269, 0.9648) // gauge bar  (1409..1805, 1001..1042)
}

/// PIXEL ORACLE (er-effects-rs-jsm): read back the game's composited backbuffer and vote, over the head's
/// opaque pixels outside the tip/bar rects, whether each sampled pixel is closer to the captured head or
/// to the loading background. Sets `GFX_PORTRAIT_HEAD_MATCH_PCT` / `GFX_PORTRAIT_HEAD_ON_SCREEN`. This
/// checks the pixels the user actually sees, so it is a resource-agnostic regression guard: it can't be
/// fooled by a "the re-forge ran" counter. Throttled by the caller; never panics.
unsafe fn probe_head_on_screen(backbuffer: &ID3D12Resource, bb_format: DXGI_FORMAT) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let Some((hw, hh, hpx)) = LOADING_BG_PORTRAIT_RGBA.lock().ok().and_then(|g| g.clone()) else {
            return;
        };
        if hw == 0 || hh == 0 {
            return;
        }
        let bg = boot_bg_image_rgba_clone(); // Option<(usize,usize,Vec<u8>)>
        let desc = backbuffer.GetDesc();
        let bw = desc.Width as u32;
        let bh = desc.Height;
        if bw == 0 || bh == 0 {
            return;
        }
        let mut dev: Option<ID3D12Device> = None;
        if backbuffer.GetDevice(&mut dev).is_err() {
            return;
        }
        let Some(device) = dev else {
            return;
        };
        let mut fp = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut total: u64 = 0;
        device.GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut fp), None, None, Some(&mut total));
        if total == 0 || fp.Footprint.RowPitch == 0 {
            return;
        }
        // (Re)create the cached readback buffer on size change.
        if SEM_BB_BUFSIZE.load(Ordering::SeqCst) != total {
            let heap = D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_READBACK,
                CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
                MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
                CreationNodeMask: 1,
                VisibleNodeMask: 1,
            };
            let buf = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Alignment: 0,
                Width: total,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: DXGI_FORMAT_UNKNOWN,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                Flags: D3D12_RESOURCE_FLAG_NONE,
            };
            let mut rb: Option<ID3D12Resource> = None;
            if device
                .CreateCommittedResource(
                    &heap,
                    D3D12_HEAP_FLAG_NONE,
                    &buf,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    None,
                    &mut rb,
                )
                .is_err()
            {
                return;
            }
            let Some(rb) = rb else { return };
            let old = SEM_BB_READBACK.swap(rb.into_raw() as usize, Ordering::SeqCst);
            if old != 0 {
                drop(ID3D12Resource::from_raw(old as *mut c_void));
            }
            SEM_BB_BUFSIZE.store(total, Ordering::SeqCst);
        }
        let rb_raw = SEM_BB_READBACK.load(Ordering::SeqCst) as *mut c_void;
        let Some(readback) = ID3D12Resource::from_raw_borrowed(&rb_raw) else {
            return;
        };
        // One-shot command objects (throttled caller -> infrequent).
        let qd = D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            Priority: 0,
            Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
            NodeMask: 0,
        };
        let Ok(queue) = device.CreateCommandQueue::<ID3D12CommandQueue>(&qd) else {
            return;
        };
        let Ok(alloc) =
            device.CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT)
        else {
            return;
        };
        let Ok(list) = device.CreateCommandList::<_, _, ID3D12GraphicsCommandList>(
            0,
            D3D12_COMMAND_LIST_TYPE_DIRECT,
            &alloc,
            None,
        ) else {
            return;
        };
        record_transition(
            &list,
            backbuffer,
            D3D12_RESOURCE_STATE_PRESENT,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        );
        let mut rb_dst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(readback.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: fp },
        };
        let mut bb_src = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(backbuffer.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        list.CopyTextureRegion(&rb_dst, 0, 0, 0, &bb_src, None);
        ManuallyDrop::drop(&mut rb_dst.pResource);
        ManuallyDrop::drop(&mut bb_src.pResource);
        record_transition(
            &list,
            backbuffer,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_RESOURCE_STATE_PRESENT,
        );
        let Ok(fence) = device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) else {
            return;
        };
        if !execute_and_wait(&queue, &list, &fence) {
            return;
        }
        let row_pitch = fp.Footprint.RowPitch as usize;
        let total_us = total as usize;
        let swap = matches!(
            bb_format,
            DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
        );
        let range = D3D12_RANGE {
            Begin: 0,
            End: total_us,
        };
        let mut rmap: *mut c_void = std::ptr::null_mut();
        if readback.Map(0, Some(&range), Some(&mut rmap)).is_err() || rmap.is_null() {
            return;
        }
        let rb_bytes = std::slice::from_raw_parts(rmap as *const u8, total_us);
        let (gw, gh) = (48u32, 27u32); // sample grid (16:9)
        let mut head_votes = 0usize;
        let mut votes = 0usize;
        for gy in 0..gh {
            for gx in 0..gw {
                let x = (gx * bw / gw).min(bw - 1);
                let y = (gy * bh / gh).min(bh - 1);
                let (u, v) = (x as f32 / bw as f32, y as f32 / bh as f32);
                if in_tip_or_bar_rect(u, v) {
                    continue;
                }
                let (hr, hg, hb, a) = sample_portrait_rgba_cover(
                    &hpx, hw as usize, hh as usize, x as usize, y as usize, bw as usize, bh as usize,
                );
                if a < 200 {
                    continue; // only where the head is opaque
                }
                let (br, bgc, bbc) = match &bg {
                    Some((bgw, bgh, bpx)) => {
                        let (r, g, b, _) = sample_portrait_rgba_cover(
                            bpx, *bgw, *bgh, x as usize, y as usize, bw as usize, bh as usize,
                        );
                        (r, g, b)
                    }
                    None => (0, 0, 0),
                };
                let o = y as usize * row_pitch + x as usize * 4;
                if o + 4 > total_us {
                    continue;
                }
                let (d0, d1, d2) = (rb_bytes[o] as u32, rb_bytes[o + 1] as u32, rb_bytes[o + 2] as u32);
                let (dr, db) = if swap { (d2, d0) } else { (d0, d2) };
                let dg = d1;
                let diff = |a: u32, b: u32| a.abs_diff(b);
                let diff_head = diff(hr, dr) + diff(hg, dg) + diff(hb, db);
                let diff_bg = diff(br, dr) + diff(bgc, dg) + diff(bbc, db);
                if diff_head < diff_bg {
                    head_votes += 1;
                }
                votes += 1;
            }
        }
        let empty = D3D12_RANGE { Begin: 0, End: 0 };
        readback.Unmap(0, Some(&empty));
        if votes == 0 {
            return;
        }
        let pct = head_votes * 100 / votes;
        GFX_PORTRAIT_HEAD_MATCH_PCT.store(pct, Ordering::SeqCst);
        let n = GFX_PORTRAIT_HEAD_PROBE_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        // Track the CURRENT state (not a latch): the overlay demotes only while the movie actually shows
        // the head, and resumes bridging if a bg-only artwork rotates in.
        GFX_PORTRAIT_HEAD_ON_SCREEN.store(
            (pct >= GFX_PORTRAIT_HEAD_MATCH_THRESHOLD) as usize,
            Ordering::SeqCst,
        );
        if pct >= GFX_PORTRAIT_HEAD_MATCH_THRESHOLD {
            GFX_PORTRAIT_HEAD_EVER.store(1, Ordering::SeqCst);
        }
        if n <= 3 || n % 32 == 0 {
            append_autoload_debug(format_args!(
                "gfx-head-probe #{n}: backbuffer head-match={pct}% (votes={votes}, threshold={GFX_PORTRAIT_HEAD_MATCH_THRESHOLD}) -> head_on_screen={}",
                GFX_PORTRAIT_HEAD_ON_SCREEN.load(Ordering::SeqCst)
            ));
        }
    }));
}

#[allow(clippy::too_many_arguments)]
unsafe fn gpu_composite_portrait_over_backbuffer(
    device: &ID3D12Device,
    queue: &ID3D12CommandQueue,
    allocator: &ID3D12CommandAllocator,
    list: &ID3D12GraphicsCommandList,
    fence: &ID3D12Fence,
    backbuffer: &ID3D12Resource,
    bb_format: DXGI_FORMAT,
    _dx: u32,
    _dy: u32,
    cw: u32,
    ch: u32,
    sw: u32,
    sh: u32,
    spx: &[u8],
    cur_ver: usize,
    stats_text: Option<&(u32, u32, Vec<u8>, usize)>,
) -> bool {
    if bb_format.0 as usize != OVERLAY_PSO_FORMAT.load(Ordering::SeqCst) {
        return overlay_gpu_fail(10, "backbuffer format changed", cur_ver);
    }
    let root_raw = OVERLAY_ROOT_SIGNATURE.load(Ordering::SeqCst) as *mut c_void;
    let pso_raw = OVERLAY_PSO.load(Ordering::SeqCst) as *mut c_void;
    let srv_heap_raw = OVERLAY_SRV_HEAP.load(Ordering::SeqCst) as *mut c_void;
    let rtv_heap_raw = OVERLAY_RTV_HEAP.load(Ordering::SeqCst) as *mut c_void;
    let (Some(root_sig), Some(pso), Some(srv_heap), Some(rtv_heap)) = (unsafe {
        (
            ID3D12RootSignature::from_raw_borrowed(&root_raw),
            ID3D12PipelineState::from_raw_borrowed(&pso_raw),
            ID3D12DescriptorHeap::from_raw_borrowed(&srv_heap_raw),
            ID3D12DescriptorHeap::from_raw_borrowed(&rtv_heap_raw),
        )
    }) else {
        return overlay_gpu_fail(11, "missing root/pso/descriptor heap", cur_ver);
    };

    let upload_needed = OVERLAY_GPU_TEX_VERSION.load(Ordering::SeqCst) != cur_ver
        || OVERLAY_GPU_TEX_W.load(Ordering::SeqCst) != sw as usize
        || OVERLAY_GPU_TEX_H.load(Ordering::SeqCst) != sh as usize;
    if upload_needed && !unsafe { ensure_overlay_gpu_texture(device, srv_heap, sw, sh) } {
        return overlay_gpu_fail(12, "ensure texture/upload failed", cur_ver);
    }
    let tex_raw = OVERLAY_GPU_TEXTURE.load(Ordering::SeqCst) as *mut c_void;
    let upload_raw = OVERLAY_GPU_UPLOAD.load(Ordering::SeqCst) as *mut c_void;
    let (Some(texture), Some(upload)) = (unsafe {
        (
            ID3D12Resource::from_raw_borrowed(&tex_raw),
            ID3D12Resource::from_raw_borrowed(&upload_raw),
        )
    }) else {
        return overlay_gpu_fail(13, "missing texture/upload resource", cur_ver);
    };

    if upload_needed && !unsafe { fill_overlay_upload_buffer(upload, sw, sh, spx) } {
        // Fill is pure CPU/map work; keep it BEFORE command-list Reset so a transient map/size failure
        // cannot leave the list open and poison every later frame's Reset.
        return overlay_gpu_fail(15, "fill upload buffer failed", cur_ver);
    }

    let mut text_resources: Option<(ID3D12Resource, ID3D12Resource, u32, u32, usize)> = None;
    let text_upload_needed = if let Some((tw, th, tpx, text_ver)) = stats_text {
        let needed = OVERLAY_TEXT_GPU_TEX_VERSION.load(Ordering::SeqCst) != *text_ver
            || OVERLAY_TEXT_GPU_TEX_W.load(Ordering::SeqCst) != *tw as usize
            || OVERLAY_TEXT_GPU_TEX_H.load(Ordering::SeqCst) != *th as usize;
        if needed && !unsafe { ensure_overlay_text_gpu_texture(device, srv_heap, *tw, *th) } {
            return overlay_gpu_fail(17, "ensure text texture/upload failed", *text_ver);
        }
        let text_tex_raw = OVERLAY_TEXT_GPU_TEXTURE.load(Ordering::SeqCst) as *mut c_void;
        let text_upload_raw = OVERLAY_TEXT_GPU_UPLOAD.load(Ordering::SeqCst) as *mut c_void;
        let (Some(text_tex), Some(text_upload)) = (unsafe {
            (
                ID3D12Resource::from_raw_borrowed(&text_tex_raw),
                ID3D12Resource::from_raw_borrowed(&text_upload_raw),
            )
        }) else {
            return overlay_gpu_fail(18, "missing text texture/upload resource", *text_ver);
        };
        if needed && !unsafe { fill_overlay_text_upload_buffer(text_upload, *tw, *th, tpx) } {
            return overlay_gpu_fail(19, "fill text upload buffer failed", *text_ver);
        }
        text_resources = Some((text_tex.clone(), text_upload.clone(), *tw, *th, *text_ver));
        needed
    } else {
        false
    };

    if unsafe { allocator.Reset() }.is_err() || unsafe { list.Reset(allocator, Some(pso)) }.is_err() {
        return overlay_gpu_fail(14, "allocator/list reset failed", cur_ver);
    }

    let submit_start = std::time::Instant::now();
    if upload_needed {
        if OVERLAY_GPU_TEX_STATE.load(Ordering::SeqCst) == 1 {
            unsafe {
                record_transition(
                    list,
                    texture,
                    D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                )
            };
        }
        let desc = unsafe { texture.GetDesc() };
        let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        unsafe {
            device.GetCopyableFootprints(
                &desc,
                0,
                1,
                0,
                Some(&mut footprint),
                None,
                None,
                None,
            )
        };
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
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
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
        OVERLAY_GPU_TEX_STATE.store(1, Ordering::SeqCst);
    }
    if text_upload_needed {
        let Some((text_texture, text_upload, _tw, _th, _text_ver)) = text_resources.as_ref() else {
            return overlay_gpu_fail(20, "text upload requested without resources", cur_ver);
        };
        if OVERLAY_TEXT_GPU_TEX_STATE.load(Ordering::SeqCst) == 1 {
            unsafe {
                record_transition(
                    list,
                    text_texture,
                    D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                )
            };
        }
        let desc = unsafe { text_texture.GetDesc() };
        let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        unsafe {
            device.GetCopyableFootprints(
                &desc,
                0,
                1,
                0,
                Some(&mut footprint),
                None,
                None,
                None,
            )
        };
        let mut src = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(text_upload.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: footprint,
            },
        };
        let mut dst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: ManuallyDrop::new(Some(text_texture.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        unsafe { list.CopyTextureRegion(&dst, 0, 0, 0, &src, None) };
        unsafe { ManuallyDrop::drop(&mut src.pResource) };
        unsafe { ManuallyDrop::drop(&mut dst.pResource) };
        unsafe {
            record_transition(
                list,
                text_texture,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            )
        };
        OVERLAY_TEXT_GPU_TEX_STATE.store(1, Ordering::SeqCst);
    }

    let rtv_cpu = unsafe { rtv_heap.GetCPUDescriptorHandleForHeapStart() };
    let rtv_desc = D3D12_RENDER_TARGET_VIEW_DESC {
        Format: bb_format,
        ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
        Anonymous: D3D12_RENDER_TARGET_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_RTV {
                MipSlice: 0,
                PlaneSlice: 0,
            },
        },
    };
    unsafe { device.CreateRenderTargetView(backbuffer, Some(&rtv_desc), rtv_cpu) };
    unsafe { record_transition(list, backbuffer, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET) };

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
    let scale = (cw as f32 / sw as f32).max(ch as f32 / sh as f32);
    let uv_scale_x = cw as f32 / (scale * sw as f32);
    let uv_scale_y = ch as f32 / (scale * sh as f32);
    let constants = [
        uv_scale_x.to_bits(),
        uv_scale_y.to_bits(),
        ((1.0 - uv_scale_x) * 0.5).to_bits(),
        ((1.0 - uv_scale_y) * 0.5).to_bits(),
    ];

    unsafe {
        list.SetGraphicsRootSignature(root_sig);
        list.SetPipelineState(pso);
        list.SetDescriptorHeaps(&[Some(srv_heap.clone())]);
        list.SetGraphicsRootDescriptorTable(0, srv_gpu_handle_at(device, srv_heap, 0));
        list.SetGraphicsRoot32BitConstants(1, constants.len() as u32, constants.as_ptr() as *const c_void, 0);
        list.RSSetViewports(std::slice::from_ref(&viewport));
        list.RSSetScissorRects(std::slice::from_ref(&scissor));
        list.OMSetRenderTargets(1, Some(&rtv_cpu), true, None);
        list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        list.DrawInstanced(3, 1, 0, 0);

        if let Some((_text_texture, _text_upload, tw, th, _text_ver)) = text_resources.as_ref() {
            let (tx, ty) = stats_text_screen_position(cw, ch, sw, sh);
            let right = (tx + *tw as i32).min(cw as i32);
            let bottom = (ty + *th as i32).min(ch as i32);
            if tx < cw as i32 && ty < ch as i32 && right > tx.max(0) && bottom > ty.max(0) {
                let text_viewport = D3D12_VIEWPORT {
                    TopLeftX: tx as f32,
                    TopLeftY: ty as f32,
                    Width: *tw as f32,
                    Height: *th as f32,
                    MinDepth: 0.0,
                    MaxDepth: 1.0,
                };
                let text_scissor = RECT {
                    left: tx.max(0),
                    top: ty.max(0),
                    right,
                    bottom,
                };
                let text_constants = [1.0f32.to_bits(), 1.0f32.to_bits(), 0.0f32.to_bits(), 0.0f32.to_bits()];
                list.SetGraphicsRootDescriptorTable(0, srv_gpu_handle_at(device, srv_heap, 1));
                list.SetGraphicsRoot32BitConstants(
                    1,
                    text_constants.len() as u32,
                    text_constants.as_ptr() as *const c_void,
                    0,
                );
                list.RSSetViewports(std::slice::from_ref(&text_viewport));
                list.RSSetScissorRects(std::slice::from_ref(&text_scissor));
                list.DrawInstanced(3, 1, 0, 0);
            }
        }

        record_transition(list, backbuffer, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PRESENT);
    }

    let ok = unsafe { execute_and_wait(queue, list, fence) };
    if !ok {
        return overlay_gpu_fail(16, "execute/wait failed", cur_ver);
    }
    if upload_needed {
        OVERLAY_GPU_TEX_VERSION.store(cur_ver, Ordering::SeqCst);
    }
    if text_upload_needed {
        if let Some((_text_texture, _text_upload, _tw, _th, text_ver)) = text_resources.as_ref() {
            OVERLAY_TEXT_GPU_TEX_VERSION.store(*text_ver, Ordering::SeqCst);
        }
    }
    record_overlay_stage_ms(
        &OVERLAY_STAGE_BLEND_COUNT,
        &OVERLAY_STAGE_BLEND_MS_SUM,
        &OVERLAY_STAGE_BLEND_MS_MAX,
        submit_start.elapsed().as_millis() as usize,
    );
    true
}

unsafe fn ensure_overlay_gpu_texture(
    device: &ID3D12Device,
    srv_heap: &ID3D12DescriptorHeap,
    sw: u32,
    sh: u32,
) -> bool {
    unsafe {
        ensure_overlay_gpu_texture_slot(
            device,
            srv_heap,
            sw,
            sh,
            0,
            &OVERLAY_GPU_TEXTURE,
            &OVERLAY_GPU_UPLOAD,
            &OVERLAY_GPU_UPLOAD_SIZE,
            &OVERLAY_GPU_TEX_W,
            &OVERLAY_GPU_TEX_H,
            &OVERLAY_GPU_TEX_STATE,
            &OVERLAY_GPU_TEX_VERSION,
        )
    }
}

unsafe fn ensure_overlay_text_gpu_texture(
    device: &ID3D12Device,
    srv_heap: &ID3D12DescriptorHeap,
    sw: u32,
    sh: u32,
) -> bool {
    unsafe {
        ensure_overlay_gpu_texture_slot(
            device,
            srv_heap,
            sw,
            sh,
            1,
            &OVERLAY_TEXT_GPU_TEXTURE,
            &OVERLAY_TEXT_GPU_UPLOAD,
            &OVERLAY_TEXT_GPU_UPLOAD_SIZE,
            &OVERLAY_TEXT_GPU_TEX_W,
            &OVERLAY_TEXT_GPU_TEX_H,
            &OVERLAY_TEXT_GPU_TEX_STATE,
            &OVERLAY_TEXT_GPU_TEX_VERSION,
        )
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn ensure_overlay_gpu_texture_slot(
    device: &ID3D12Device,
    srv_heap: &ID3D12DescriptorHeap,
    sw: u32,
    sh: u32,
    srv_index: u32,
    texture_slot: &AtomicUsize,
    upload_slot: &AtomicUsize,
    upload_size_slot: &AtomicU64,
    tex_w_slot: &AtomicUsize,
    tex_h_slot: &AtomicUsize,
    tex_state_slot: &AtomicUsize,
    tex_version_slot: &AtomicUsize,
) -> bool {
    if sw == 0 || sh == 0 || sw > MAX_RT_DIM || sh > MAX_RT_DIM {
        return false;
    }
    if texture_slot.load(Ordering::SeqCst) != 0
        && upload_slot.load(Ordering::SeqCst) != 0
        && tex_w_slot.load(Ordering::SeqCst) == sw as usize
        && tex_h_slot.load(Ordering::SeqCst) == sh as usize
    {
        return true;
    }
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Alignment: 0,
        Width: sw as u64,
        Height: sh,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    };
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    };
    let mut tex_opt: Option<ID3D12Resource> = None;
    if unsafe {
        device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut tex_opt,
        )
    }
    .is_err()
    {
        return false;
    }
    let Some(texture) = tex_opt else { return false };

    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total_bytes = 0u64;
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
        return false;
    }
    let upload_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: total_bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    };
    let upload_heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_UPLOAD,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    };
    let mut up_opt: Option<ID3D12Resource> = None;
    if unsafe {
        device.CreateCommittedResource(
            &upload_heap,
            D3D12_HEAP_FLAG_NONE,
            &upload_desc,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut up_opt,
        )
    }
    .is_err()
    {
        return false;
    }
    let Some(upload) = up_opt else { return false };

    let srv_desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_SRV {
                MostDetailedMip: 0,
                MipLevels: 1,
                PlaneSlice: 0,
                ResourceMinLODClamp: 0.0,
            },
        },
    };
    unsafe {
        device.CreateShaderResourceView(
            &texture,
            Some(&srv_desc),
            srv_cpu_handle_at(device, srv_heap, srv_index),
        )
    };
    texture_slot.store(texture.into_raw() as usize, Ordering::SeqCst);
    upload_slot.store(upload.into_raw() as usize, Ordering::SeqCst);
    upload_size_slot.store(total_bytes, Ordering::SeqCst);
    tex_w_slot.store(sw as usize, Ordering::SeqCst);
    tex_h_slot.store(sh as usize, Ordering::SeqCst);
    tex_state_slot.store(0, Ordering::SeqCst);
    tex_version_slot.store(usize::MAX, Ordering::SeqCst);
    true
}

fn srv_cpu_handle_at(
    device: &ID3D12Device,
    heap: &ID3D12DescriptorHeap,
    index: u32,
) -> D3D12_CPU_DESCRIPTOR_HANDLE {
    let mut handle = unsafe { heap.GetCPUDescriptorHandleForHeapStart() };
    let inc = unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) } as usize;
    handle.ptr += inc * index as usize;
    handle
}

fn srv_gpu_handle_at(
    device: &ID3D12Device,
    heap: &ID3D12DescriptorHeap,
    index: u32,
) -> D3D12_GPU_DESCRIPTOR_HANDLE {
    let mut handle = unsafe { heap.GetGPUDescriptorHandleForHeapStart() };
    let inc = unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) } as u64;
    handle.ptr += inc * index as u64;
    handle
}

fn stats_text_screen_position(cw: u32, ch: u32, sw: u32, sh: u32) -> (i32, i32) {
    if cw == 0 || ch == 0 || sw == 0 || sh == 0 {
        return (0, 0);
    }
    // Match the old placement that baked the text into the portrait texture at (5%, 60%) BEFORE the
    // portrait was aspect-cover scaled/cropped to the backbuffer. The text is now a separate screen-space
    // draw, but preserving this mapping keeps its visual position stable while improving sharpness.
    let scale = (cw as f32 / sw as f32).max(ch as f32 / sh as f32);
    let visible_w = cw as f32 / scale;
    let visible_h = ch as f32 / scale;
    let src_x = sw as f32 * 0.05;
    let src_y = sh as f32 * 0.60;
    let crop_x = (sw as f32 - visible_w) * 0.5;
    let crop_y = (sh as f32 - visible_h) * 0.5;
    (((src_x - crop_x) * scale).round() as i32, ((src_y - crop_y) * scale).round() as i32)
}

unsafe fn fill_overlay_upload_buffer(upload: &ID3D12Resource, sw: u32, sh: u32, spx: &[u8]) -> bool {
    let expected = sw as usize * sh as usize * RGBA8_BPP;
    if spx.len() < expected {
        append_autoload_debug(format_args!(
            "present-overlay: upload fill rejected short source len={} expected={} dims={}x{}",
            spx.len(), expected, sw, sh
        ));
        return false;
    }
    let row_size = sw as usize * RGBA8_BPP;
    let row_pitch = ((row_size + 255) & !255).max(row_size);
    let total = OVERLAY_GPU_UPLOAD_SIZE.load(Ordering::SeqCst) as usize;
    // D3D12 GetCopyableFootprints total size does not require padding after the final row:
    // total == row_pitch * (height - 1) + row_size. Do not reject valid small textures like 56x56
    // where row_pitch=256, row_size=224, total=14304 (not 14336).
    let needed = row_pitch * (sh as usize).saturating_sub(1) + row_size;
    if total < needed {
        append_autoload_debug(format_args!(
            "present-overlay: upload fill rejected short upload total={total} need={needed} row_pitch={row_pitch} row_size={row_size} dims={}x{}",
            sw,
            sh
        ));
        return false;
    }
    let mut map: *mut c_void = std::ptr::null_mut();
    if unsafe { upload.Map(0, None, Some(&mut map)) }.is_err() || map.is_null() {
        append_autoload_debug(format_args!(
            "present-overlay: upload fill Map failed dims={}x{} total={total}",
            sw, sh
        ));
        return false;
    }
    {
        let dst = unsafe { std::slice::from_raw_parts_mut(map as *mut u8, total) };
        let src_row = row_size;
        for y in 0..sh as usize {
            let src = &spx[y * src_row..y * src_row + src_row];
            let dst_off = y * row_pitch;
            dst[dst_off..dst_off + src_row].copy_from_slice(src);
        }
    }
    unsafe { upload.Unmap(0, None) };
    true
}

unsafe fn fill_overlay_text_upload_buffer(upload: &ID3D12Resource, sw: u32, sh: u32, spx: &[u8]) -> bool {
    let expected = sw as usize * sh as usize * RGBA8_BPP;
    if spx.len() < expected {
        append_autoload_debug(format_args!(
            "present-overlay: text upload fill rejected short source len={} expected={} dims={}x{}",
            spx.len(), expected, sw, sh
        ));
        return false;
    }
    let row_size = sw as usize * RGBA8_BPP;
    let row_pitch = ((row_size + 255) & !255).max(row_size);
    let total = OVERLAY_TEXT_GPU_UPLOAD_SIZE.load(Ordering::SeqCst) as usize;
    let needed = row_pitch * (sh as usize).saturating_sub(1) + row_size;
    if total < needed {
        append_autoload_debug(format_args!(
            "present-overlay: text upload fill rejected short upload total={total} need={needed} row_pitch={row_pitch} row_size={row_size} dims={}x{}",
            sw,
            sh
        ));
        return false;
    }
    let mut map: *mut c_void = std::ptr::null_mut();
    if unsafe { upload.Map(0, None, Some(&mut map)) }.is_err() || map.is_null() {
        append_autoload_debug(format_args!(
            "present-overlay: text upload fill Map failed dims={}x{} total={total}",
            sw, sh
        ));
        return false;
    }
    {
        let dst = unsafe { std::slice::from_raw_parts_mut(map as *mut u8, total) };
        let src_row = row_size;
        for y in 0..sh as usize {
            let src = &spx[y * src_row..y * src_row + src_row];
            let dst_off = y * row_pitch;
            dst[dst_off..dst_off + src_row].copy_from_slice(src);
        }
    }
    unsafe { upload.Unmap(0, None) };
    true
}

/// Alpha-honoring CPU composite: copy the live backbuffer region `[dx,dy .. dx+cw,dy+ch]` to a readback
/// buffer, blend the portrait (`spx`, `sw` x `sh`, RGBA8 with per-pixel alpha) OVER it (`src.a`/`1-src.a`; a
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
    sh: u32,
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
    let readback_wait_start = std::time::Instant::now();
    if !unsafe { execute_and_wait(queue, list, fence) } {
        return false;
    }
    record_overlay_stage_ms(
        &OVERLAY_STAGE_READBACK_WAIT_COUNT,
        &OVERLAY_STAGE_READBACK_WAIT_MS_SUM,
        &OVERLAY_STAGE_READBACK_WAIT_MS_MAX,
        readback_wait_start.elapsed().as_millis() as usize,
    );

    // ---- CPU BLEND: readback (backbuffer pixels) OVER-composited with the portrait, into the upload buffer.
    let blend_start = std::time::Instant::now();
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
        let sh = sh as usize;
        let cw = cw as usize;
        let ch = ch as usize;
        for y in 0..ch {
            let ro = y * row_pitch;
            for x in 0..cw {
                let o = ro + x * 4;
                if o + 4 > total {
                    break;
                }
                let (pr, pg, pb, a) = sample_portrait_rgba_cover(spx, sw, sh, x, y, cw, ch);
                let ia = 255 - a;
                // Portrait is RGBA; place each portrait channel at the backbuffer's channel position.
                let (p0, p2) = if swap { (pb, pr) } else { (pr, pb) };
                let blend = |p: u32, d: u32| ((p * a + d * ia + 127) / 255) as u8;
                up_bytes[o] = blend(p0, rb_bytes[o] as u32);
                up_bytes[o + 1] = blend(pg, rb_bytes[o + 1] as u32);
                up_bytes[o + 2] = blend(p2, rb_bytes[o + 2] as u32);
                up_bytes[o + 3] = 255;
            }
        }
    }
    let empty = D3D12_RANGE { Begin: 0, End: 0 };
    unsafe { readback.Unmap(0, Some(&empty)) };
    unsafe { upload.Unmap(0, None) };
    record_overlay_stage_ms(
        &OVERLAY_STAGE_BLEND_COUNT,
        &OVERLAY_STAGE_BLEND_MS_SUM,
        &OVERLAY_STAGE_BLEND_MS_MAX,
        blend_start.elapsed().as_millis() as usize,
    );

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
    let upload_wait_start = std::time::Instant::now();
    let ok = unsafe { execute_and_wait(queue, list, fence) };
    if ok {
        record_overlay_stage_ms(
            &OVERLAY_STAGE_UPLOAD_WAIT_COUNT,
            &OVERLAY_STAGE_UPLOAD_WAIT_MS_SUM,
            &OVERLAY_STAGE_UPLOAD_WAIT_MS_MAX,
            upload_wait_start.elapsed().as_millis() as usize,
        );
    }
    ok
}

fn record_overlay_stage_ms(count: &AtomicUsize, sum: &AtomicUsize, max: &AtomicUsize, elapsed_ms: usize) {
    count.fetch_add(1, Ordering::SeqCst);
    sum.fetch_add(elapsed_ms, Ordering::SeqCst);
    update_atomic_max(max, elapsed_ms);
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
