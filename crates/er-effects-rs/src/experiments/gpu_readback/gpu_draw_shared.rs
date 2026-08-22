use super::*;
// Shared D3D12 GPU-draw plumbing used by the boot-view fade and effect-selector overlay
// draws in boot_progress.rs: HLSL compile, root-signature/PSO builders, the generic
// texture+upload+SRV slot creator, SRV handle math, and the close/execute/fence-wait
// submit helper. Relocated out of the deleted path-A overlay composite
// (bd er-effects-rs-f9mq); the imports below feed the whole flat gpu_readback namespace.

use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{ID3DBlob, ID3DInclude};
use windows::Win32::Graphics::Direct3D12::{
    D3D_ROOT_SIGNATURE_VERSION_1, D3D12_BLEND_DESC, D3D12_BLEND_INV_SRC_ALPHA, D3D12_BLEND_ONE,
    D3D12_BLEND_OP_ADD, D3D12_BLEND_SRC_ALPHA, D3D12_COLOR_WRITE_ENABLE_ALL,
    D3D12_COMPARISON_FUNC_ALWAYS, D3D12_CONSERVATIVE_RASTERIZATION_MODE_OFF,
    D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_CULL_MODE_NONE, D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
    D3D12_DEPTH_STENCIL_DESC, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, D3D12_DESCRIPTOR_RANGE,
    D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND, D3D12_DESCRIPTOR_RANGE_TYPE_SRV, D3D12_FILL_MODE_SOLID,
    D3D12_FILTER_MIN_MAG_MIP_LINEAR, D3D12_GPU_DESCRIPTOR_HANDLE,
    D3D12_GRAPHICS_PIPELINE_STATE_DESC, D3D12_INDEX_BUFFER_STRIP_CUT_VALUE_DISABLED,
    D3D12_INPUT_LAYOUT_DESC, D3D12_PIPELINE_STATE_FLAG_NONE,
    D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, D3D12_RASTERIZER_DESC, D3D12_RENDER_TARGET_BLEND_DESC,
    D3D12_ROOT_CONSTANTS, D3D12_ROOT_DESCRIPTOR_TABLE, D3D12_ROOT_PARAMETER,
    D3D12_ROOT_PARAMETER_0, D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
    D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE, D3D12_ROOT_SIGNATURE_DESC,
    D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT, D3D12_SHADER_BYTECODE,
    D3D12_SHADER_RESOURCE_VIEW_DESC, D3D12_SHADER_RESOURCE_VIEW_DESC_0,
    D3D12_SHADER_VISIBILITY_PIXEL, D3D12_SRV_DIMENSION_TEXTURE2D,
    D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK, D3D12_STATIC_SAMPLER_DESC, D3D12_TEX2D_SRV,
    D3D12_TEXTURE_ADDRESS_MODE_CLAMP, D3D12SerializeRootSignature, ID3D12DescriptorHeap,
    ID3D12PipelineState, ID3D12RootSignature,
};
use windows::core::{BOOL, s};

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

pub(super) unsafe fn create_overlay_root_signature(
    device: &ID3D12Device,
) -> Option<ID3D12RootSignature> {
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
    unsafe {
        device
            .CreateRootSignature::<ID3D12RootSignature>(0, bytes)
            .ok()
    }
}

pub(super) unsafe fn create_overlay_pso(
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
    unsafe {
        device
            .CreateGraphicsPipelineState::<ID3D12PipelineState>(&desc)
            .ok()
    }
}

unsafe fn compile_overlay_shader(entry: &'static [u8], target: &'static [u8]) -> Option<ID3DBlob> {
    let mut code: Option<ID3DBlob> = None;
    let mut err: Option<ID3DBlob> = None;
    if unsafe {
        D3DCompile(
            OVERLAY_SHADER_HLSL.as_ptr() as *const c_void,
            OVERLAY_SHADER_HLSL.len(),
            s!("er-effects-present-overlay"),
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
        log_shader_error(
            core::str::from_utf8(entry).unwrap_or("shader"),
            err.as_ref(),
        );
        return None;
    }
    code
}

fn log_shader_error(stage: &str, err: Option<&ID3DBlob>) {
    if let Some(err) = err {
        let ptr = unsafe { err.GetBufferPointer() } as *const u8;
        let len = unsafe { err.GetBufferSize() };
        if !ptr.is_null() && len > 0 {
            let bytes = unsafe { std::slice::from_raw_parts(ptr, len.min(512)) };
            let msg = core::str::from_utf8(bytes).unwrap_or("<non-utf8 shader error>");
            append_autoload_debug(format_args!(
                "present-overlay: {stage} compile error: {msg}"
            ));
            return;
        }
    }
    append_autoload_debug(format_args!(
        "present-overlay: {stage} compile/serialize failed"
    ));
}

#[allow(clippy::too_many_arguments)]
pub(super) unsafe fn ensure_overlay_gpu_texture_slot(
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
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
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
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
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
    let inc =
        unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) }
            as usize;
    handle.ptr += inc * index as usize;
    handle
}

pub(super) fn srv_gpu_handle_at(
    device: &ID3D12Device,
    heap: &ID3D12DescriptorHeap,
    index: u32,
) -> D3D12_GPU_DESCRIPTOR_HANDLE {
    let mut handle = unsafe { heap.GetGPUDescriptorHandleForHeapStart() };
    let inc =
        unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) }
            as u64;
    handle.ptr += inc * index as u64;
    handle
}

/// Close `list`, execute it on `queue`, signal `fence` with a fresh monotonic value, and CPU-wait (bounded)
/// for GPU completion. `false` on any failure. Shared by the two-submit CPU-blend composite.
pub(super) unsafe fn execute_and_wait(
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
