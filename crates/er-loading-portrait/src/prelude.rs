//! Crate-internal flat-namespace prelude.
//!
//! The moved modules were `include!`-concatenated flat namespaces inside er-effects-rs
//! (experiments/gpu_readback.rs, experiments/startup_hooks.rs, constants.rs), where every
//! file saw every other file's items plus the module's shared import block. Each moved
//! module now starts with `use crate::prelude::*;`, and this prelude re-creates that flat
//! namespace: every crate module's items plus the shared std/windows/game imports the old
//! include! parents provided. Deliberately liberal -- unused imports are allowed
//! (`#![allow(unused_imports)]` at the crate root).

// --- every crate module, glob (cyclic glob re-imports are legal) --------------------
pub(crate) use crate::bridge::*;
#[cfg(windows)]
pub(crate) use crate::cached_depth_readback::*;
#[cfg(windows)]
pub(crate) use crate::depth_mask_upload::*;
#[cfg(windows)]
pub(crate) use crate::dlstring_lookat_math::*;
pub(crate) use crate::host::*;
pub(crate) use crate::layout::*;
#[cfg(windows)]
pub(crate) use crate::lookat_bone_hooks::*;
#[cfg(windows)]
pub(crate) use crate::lookat_stage_camera::*;
#[cfg(windows)]
pub(crate) use crate::native_overlay::*;
#[cfg(windows)]
pub(crate) use crate::pgd_layout::*;
#[cfg(windows)]
pub(crate) use crate::player_identity::*;
#[cfg(windows)]
pub(crate) use crate::portrait_camera::*;
#[cfg(windows)]
pub(crate) use crate::portrait_lookat::*;
pub(crate) use crate::portrait_overlay::*;
#[cfg(windows)]
pub(crate) use crate::portrait_semaphores::*;
#[cfg(windows)]
pub(crate) use crate::portrait_shared::*;
#[cfg(windows)]
pub(crate) use crate::portrait_worker::*;
#[cfg(windows)]
pub(crate) use crate::resource_readback::*;
pub(crate) use crate::stats_lines::*;
#[cfg(windows)]
pub(crate) use crate::stats_loading_text::*;
#[cfg(windows)]
pub(crate) use crate::stats_overlay::*;
pub(crate) use crate::title_stats_text::*;

// --- telemetry counters whose canonical product re-export stays in er-effects-rs ----
pub(crate) use er_telemetry::counters::{
    BOOT_VIEW_EPOCH_WORLD_LIVE, PROFILE_SPARE_ORPHAN,
    SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT, SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT,
};

// --- private sentinels (values copied EXACTLY from the product originals; the product
// --- keeps its own copies, so these deliberately never leak out of this crate) ------
/// er-effects-rs `constants/anti_debug.rs`: `TITLE_OWNER_SCAN_START_ADDRESS = usize::MIN`.
pub(crate) const TITLE_OWNER_SCAN_START_ADDRESS: usize = usize::MIN;
/// er-effects-rs `constants.rs`: `HOOK_ORIGINAL_UNSET = 0`.
pub(crate) const HOOK_ORIGINAL_UNSET: usize = 0;
/// er-effects-rs `constants/own_load_pump.rs`: `OWN_STEPPER_CALL_INC = true as usize`.
pub(crate) const OWN_STEPPER_CALL_INC: usize = true as usize;

// --- shared std imports (union of the old include! parents' import blocks) ----------
pub(crate) use std::{
    ffi::c_void,
    fmt::Write as _,
    fs,
    mem::ManuallyDrop,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once, OnceLock,
        atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

#[cfg(windows)]
pub(crate) use std::os::windows::ffi::OsStrExt as _;

// --- shared game/hook/mem imports ----------------------------------------------------
#[cfg(windows)]
pub(crate) use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
#[cfg(windows)]
pub(crate) use er_game_base::mem::*;
#[cfg(windows)]
pub(crate) use er_game_base::pgd::{
    read_utf16_name_units, utf16_name_empty_like, utf16_names_equal,
};
#[cfg(windows)]
pub(crate) use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
#[cfg(windows)]
pub(crate) use fromsoftware_shared::{F32Vector4, FromStatic, InstanceError, SharedTaskImpExt};

// --- shared windows imports (union of experiments/mod.rs + the gpu_readback flat
// --- namespace header that used to live in resource_readback.rs) --------------------
#[cfg(windows)]
pub(crate) use windows::{
    Win32::{
        Foundation::{
            CloseHandle, GENERIC_READ, HANDLE, HINSTANCE, HWND, LPARAM, RECT, WAIT_OBJECT_0, WPARAM,
        },
        Graphics::{
            Direct3D::D3D_FEATURE_LEVEL_11_0,
            Direct3D12::{
                D3D12_BOX, D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC,
                D3D12_COMMAND_QUEUE_FLAG_NONE, D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
                D3D12_FENCE_FLAG_NONE, D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES,
                D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_READBACK, D3D12_HEAP_TYPE_UPLOAD,
                D3D12_MEMORY_POOL_UNKNOWN, D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_RANGE,
                D3D12_RESOURCE_BARRIER, D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_FLAG_NONE,
                D3D12_RESOURCE_BARRIER_TYPE_TRANSITION, D3D12_RESOURCE_DESC,
                D3D12_RESOURCE_DIMENSION_BUFFER, D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COMMON,
                D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_GENERIC_READ, D3D12_RESOURCE_STATE_PRESENT,
                D3D12_RESOURCE_STATES, D3D12_RESOURCE_TRANSITION_BARRIER,
                D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0,
                D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                D3D12_TEXTURE_LAYOUT_UNKNOWN, ID3D12CommandAllocator, ID3D12CommandList,
                ID3D12CommandQueue, ID3D12Device, ID3D12Fence, ID3D12GraphicsCommandList,
                ID3D12Resource,
            },
            Dxgi::{
                Common::{
                    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
                    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
                    DXGI_FORMAT_R10G10B10A2_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
                },
                IDXGISwapChain3,
            },
            Imaging::{
                CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICBitmapSource,
                IWICImagingFactory, WICConvertBitmapSource, WICDecodeMetadataCacheOnDemand,
            },
        },
        System::{
            Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx},
            LibraryLoader::{GetModuleHandleA, GetProcAddress},
            Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery},
            SystemServices::DLL_PROCESS_ATTACH,
            Threading::{CreateEventW, GetCurrentProcessId, WaitForSingleObject},
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_KEYDOWN,
            WM_KEYUP,
        },
    },
    core::{BOOL, IUnknown, Interface, PCSTR, PCWSTR},
};
