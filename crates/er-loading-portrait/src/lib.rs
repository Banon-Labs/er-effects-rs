//! Loading-screen portrait + character-stats feature crate (er-effects-rs portrait crate
//! split, path B + capture pipeline).
//!
//! Contents, moved verbatim from the root DLL:
//! * capture pipeline: profile-table renderer drive, idle-anim bind, camera override,
//!   staged color+depth readback, depth-key worker, frame bridge
//!   (`lookat_bone_hooks`, `lookat_stage_camera`, `dlstring_lookat_math`,
//!   `resource_readback`, `cached_depth_readback`, `depth_mask_upload`,
//!   `portrait_worker`, `portrait_shared`);
//! * display half: portrait/stats CPU compositors (`portrait_overlay`, `stats_overlay`)
//!   and the native-Windows isolated overlay window host (`native_overlay`);
//! * stats producer: loading-screen stats lines + game-menu-font raster
//!   (`stats_loading_text`);
//! * the reverse-engineered constants/statics those modules own (`layout`, `pgd_layout`,
//!   `portrait_lookat`, `portrait_camera`, `portrait_semaphores`) and the published-frame
//!   bridge (`bridge`).
//!
//! Product state the feature reads (gates, slot resolution, logging, the shared boot-view
//! rasterizer) crosses the seam as injected function pointers: see [`host::install_host`].
//! This crate must not depend on the root crate.

mod prelude;

pub mod host;
pub use host::*;

pub mod bridge;
pub use bridge::*;

pub mod layout;
pub use layout::*;

#[cfg(windows)]
pub mod pgd_layout;
#[cfg(windows)]
pub use pgd_layout::*;

#[cfg(windows)]
pub mod player_identity;
#[cfg(windows)]
pub use player_identity::*;

#[cfg(windows)]
pub mod portrait_lookat;
#[cfg(windows)]
pub use portrait_lookat::*;

#[cfg(windows)]
pub mod portrait_camera;
#[cfg(windows)]
pub use portrait_camera::*;

#[cfg(windows)]
pub mod portrait_semaphores;
#[cfg(windows)]
pub use portrait_semaphores::*;

// Pure identity-decision logic for the portrait semaphores. Deliberately NOT windows-gated: the
// rules that decide "is the portrait showing the right character" are the ones a wrong-face bug
// hides in, so they must be reachable by a host `cargo test` rather than only by a game launch.
pub mod portrait_identity;
pub use portrait_identity::*;

#[cfg(windows)]
pub mod resource_readback;
#[cfg(windows)]
pub use resource_readback::*;

#[cfg(windows)]
pub mod cached_depth_readback;
#[cfg(windows)]
pub use cached_depth_readback::*;

#[cfg(windows)]
pub mod depth_mask_upload;
#[cfg(windows)]
pub use depth_mask_upload::*;

#[cfg(windows)]
pub mod portrait_shared;
#[cfg(windows)]
pub use portrait_shared::*;

#[cfg(windows)]
pub mod portrait_worker;
#[cfg(windows)]
pub use portrait_worker::*;

pub mod portrait_overlay;
pub use portrait_overlay::*;

#[cfg(windows)]
pub mod stats_overlay;
#[cfg(windows)]
pub use stats_overlay::*;

#[cfg(windows)]
pub mod lookat_bone_hooks;
#[cfg(windows)]
pub use lookat_bone_hooks::*;

#[cfg(windows)]
pub mod lookat_stage_camera;
#[cfg(windows)]
pub use lookat_stage_camera::*;

#[cfg(windows)]
pub mod dlstring_lookat_math;
#[cfg(windows)]
pub use dlstring_lookat_math::*;

pub mod stats_lines;
pub use stats_lines::*;

pub mod portrait_equip;

pub mod title_stats_text;
pub use title_stats_text::*;

pub mod profile_row_label;

#[cfg(windows)]
pub mod stats_loading_text;
#[cfg(windows)]
pub use stats_loading_text::*;

#[cfg(windows)]
pub mod native_overlay;
#[cfg(windows)]
pub use native_overlay::*;
