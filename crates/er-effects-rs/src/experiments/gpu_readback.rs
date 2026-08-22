//! Boot/loading-screen frame rasterizer + save-picker overlay host (product side).
//!
//! The portrait capture pipeline (staged color+depth readback, depth-key worker,
//! portrait/stats CPU compositors, frame bridge) moved to the `er-loading-portrait`
//! crate (portrait crate split). A `pub(crate) use er_loading_portrait::*` shim used to sit here
//! so every remaining flat-namespace reference (BootViewFrame, portrait_onto, RGBA8_BPP,
//! MAX_RT_DIM, OVERLAY_FENCE_VAL, record_transition, ...) kept compiling unchanged. Those
//! references are gone -- the 2026-08-21 lint-parity sweep pruned the last of them -- so the shim
//! resolved nothing and rustc 1.98 flagged it. What this module still needs it names directly.

use super::*;

// The shared import block for the remaining modules below (it used to live at the
// top of resource_readback.rs before that file moved to er-loading-portrait).
use std::mem::ManuallyDrop;

use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_COMMAND_QUEUE_FLAG_NONE,
    D3D12_CPU_PAGE_PROPERTY_UNKNOWN, D3D12_FENCE_FLAG_NONE, D3D12_HEAP_FLAG_NONE,
    D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_UPLOAD,
    D3D12_MEMORY_POOL_UNKNOWN, D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_DIMENSION_BUFFER, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_NONE,
    D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_GENERIC_READ,
    D3D12_RESOURCE_STATE_PRESENT, D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0,
    D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
    D3D12_TEXTURE_LAYOUT_ROW_MAJOR, D3D12_TEXTURE_LAYOUT_UNKNOWN, ID3D12CommandAllocator,
    ID3D12CommandList, ID3D12CommandQueue, ID3D12DescriptorHeap, ID3D12Device, ID3D12Fence,
    ID3D12GraphicsCommandList, ID3D12PipelineState, ID3D12Resource, ID3D12RootSignature,
};
use windows::Win32::Graphics::Direct3D12::{
    D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
    D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
    D3D12_DESCRIPTOR_HEAP_TYPE_RTV, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
    D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_VIEWPORT,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain3;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICBitmapSource, IWICImagingFactory,
    WICConvertBitmapSource, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{IUnknown, Interface, PCSTR, PCWSTR};

mod gpu_draw_shared;
// Private glob on purpose: nothing in this submodule is `pub(crate)`, so a `pub(crate) use`
// re-exports nothing and rustc reports it as an unused import. Children still reach these
// through their own `use super::*`.
use gpu_draw_shared::*;

mod boot_progress;
pub(crate) use boot_progress::*;

mod save_picker_overlay;
pub(crate) use save_picker_overlay::*;
