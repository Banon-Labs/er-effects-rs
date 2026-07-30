//! Boot/loading-screen frame rasterizer + save-picker overlay host (product side).
//!
//! The portrait capture pipeline (staged color+depth readback, depth-key worker,
//! portrait/stats CPU compositors, frame bridge) moved to the `er-loading-portrait`
//! crate (portrait crate split); the glob shim below re-exports it so every remaining
//! flat-namespace reference (BootViewFrame, portrait_onto, RGBA8_BPP, MAX_RT_DIM,
//! OVERLAY_FENCE_VAL, record_transition, ...) keeps compiling unchanged.

#![allow(unused_imports)]

pub(crate) use er_loading_portrait::*;

use super::*;

// The shared import block for the remaining include! files below (it used to live at the
// top of resource_readback.rs before that file moved to er-loading-portrait).
use std::mem::ManuallyDrop;

use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, WAIT_OBJECT_0};
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
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_R10G10B10A2_UNORM,
    DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain3;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICBitmapSource, IWICImagingFactory,
    WICConvertBitmapSource, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleA;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{IUnknown, Interface, PCSTR, PCWSTR};

include!("gpu_readback/gpu_draw_shared.rs");
include!("gpu_readback/boot_progress.rs");
include!("gpu_readback/save_picker_overlay.rs");
