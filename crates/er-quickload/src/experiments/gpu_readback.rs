//! Boot/loading-screen frame rasterizer + save-picker overlay host (product side).
//!
//! The portrait capture pipeline (staged color+depth readback, depth-key worker,
//! portrait/stats CPU compositors, frame bridge) moved to the `er-loading-portrait-core`
//! crate (portrait crate split). A `pub(crate) use er_loading_portrait_core::*` shim used to sit here
//! so every remaining flat-namespace reference (BootViewFrame, portrait_onto, RGBA8_BPP,
//! MAX_RT_DIM, OVERLAY_FENCE_VAL, record_transition, ...) kept compiling unchanged. Those
//! references are gone -- the 2026-08-21 lint-parity sweep pruned the last of them -- so the shim
//! resolved nothing and rustc 1.98 flagged it. What this module still needs it names directly.

use super::*;

// The shared import block for the remaining modules below (it used to live at the
// top of resource_readback.rs before that file moved to er-loading-portrait-core).
//
// It used to be much longer. The release fade was the only thing here that built D3D12 objects --
// command allocator/list/queue/fence, descriptor heaps, PSOs, copy footprints, viewports -- and it
// moved to `er-cover-fade`, taking every one of those imports with it. What is left is what the
// modules below still touch directly: the swapchain they composite onto and the backbuffer they get
// from it. The shared draw plumbing glob went the same way, for the same reason: the only caller of
// `gpu_draw_shared` under this module was the fade.
use windows::Win32::Graphics::Direct3D12::ID3D12Resource;
use windows::Win32::Graphics::Dxgi::IDXGISwapChain3;
use windows::core::Interface;

mod boot_progress;
pub(crate) use boot_progress::*;

mod save_picker_overlay;
pub(crate) use save_picker_overlay::*;
