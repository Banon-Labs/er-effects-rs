//! The build watermark: a faint list, top right, of every one of this workspace's DLLs in the
//! process and how each stands against `main`.
//!
//! # Why it exists
//!
//! On 2026-08-24 a tester reported a crash against an invasion-warp DLL three days older than the
//! fix for that exact crash, sitting beside two DLLs built the same afternoon. Nothing on screen
//! said so; the only way to establish it was parsing the module table out of a minidump. This is
//! that answer, permanently on screen, for anyone who screenshots anything.
//!
//! # Loud only when it must be
//!
//! Quiet states are drawn at **1%** -- present in a screenshot, effectively invisible while
//! playing. The single loud state, **25% red**, is a build that is an older published release
//! than `main`'s tip: somebody is running code we have already moved past, and a bug report
//! against it may describe something already fixed. A dirty local tree is not that, and is drawn
//! as quietly as the tip -- see `er_game_base::build_id::Standing`.
//!
//! # Why hudhook rather than the D3D12 compositor
//!
//! `er-d3d12-compositor` copies an RGBA frame onto the backbuffer; it never blends, so "1%
//! opacity" is not expressible through it at all -- the first cut of this watermark had to paint
//! an opaque dark panel and call it faint. Real alpha through that path would have meant either a
//! per-frame backbuffer readback (a fence stall inside `Present`) or a blend pipeline with its
//! own shaders. `er-net-effects` already runs a hudhook/imgui DX12 overlay in this same
//! process, alongside the product's own `Present` hook, and `DrawList::add_text` takes a float
//! alpha imgui blends for free. Reusing it costs one dependency and no stall.

pub mod layout;
pub mod releases;

#[cfg(windows)]
mod overlay;

/// One imgui host per process, and a way for every other module to draw on it.
#[cfg(windows)]
pub mod overlay_host;

#[cfg(windows)]
pub use overlay::{
    OverlayClaim, claim_overlay_ownership, claim_owner, draw_rows, install_if_owner, render_hits,
    visible_rows,
};

/// Host stub so callers compile on Linux; there is no swapchain to draw on.
#[cfg(not(windows))]
pub fn install_if_owner(_hmodule_raw: usize, _log: fn(std::fmt::Arguments<'_>)) -> bool {
    false
}
