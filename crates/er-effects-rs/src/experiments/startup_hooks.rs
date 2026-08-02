//! Startup-hook glue for product loading flows, save-picker/quit-menu features, and
//! runtime diagnostics.
//!
//! This module is being converted from one flat `include!` namespace into ownership
//! modules. Compatibility `pub(crate) use` shims preserve the current private helper
//! visibility while each ownership cluster is moved behind a real Rust module.
//!
//! Ownership groups:
//! * `loading_cover/` -- title/loading-cover/product boot resources that stay with the
//!   product until the loading-cover extraction resumes.
//! * `save_picker/` -- product (A), the boot missing-save picker and its shared OS dialog
//!   mechanism.
//! * `quit_menu/` -- product (B), the customized System>Quit menu, Load Profile rows,
//!   Save Game flow, destination picker, dim cover, and ownership fixes.
//! * `diagnostics/` -- agent/runtime-probe traces and diagnostics that must not be
//!   dragged into standalone feature crates.
//!
//! The loading-screen portrait capture pipeline + stats producer
//! (dlstring_lookat_math, lookat_bone_hooks, lookat_stage_camera, stats_loading_text)
//! moved to the `er-loading-portrait` crate (portrait crate split); the glob shim below
//! re-exports it so every remaining flat-namespace reference keeps compiling unchanged.

#![allow(unused_imports)]

pub(crate) use er_loading_portrait::*;

// Product loading-cover / title resources.
include!("startup_hooks/loading_cover/title_scaleform_msgbox.rs");
include!("startup_hooks/loading_cover/startup_modals_menu_cover.rs");
include!("startup_hooks/loading_cover/loading_cover_save_slot.rs");
include!("startup_hooks/loading_cover/portrait_equip_oracle.rs");
include!("startup_hooks/loading_cover/profile_table_gfx_files.rs");
include!("startup_hooks/loading_cover/scaleform_descriptor_guard.rs");
include!("startup_hooks/loading_cover/title_resources_stats_text.rs");
include!("startup_hooks/loading_cover/window_reconfig_observer.rs");
include!("startup_hooks/loading_cover/dlc_roots_self_heal.rs");

// Product (B): customized System>Quit / profile-switch / save-flow menu.
include!("startup_hooks/quit_menu/profile_rows_system_quit_menu.rs");
include!("startup_hooks/quit_menu/system_quit_row_identity.rs");
include!("startup_hooks/quit_menu/system_quit_dialog_handlers.rs");
include!("startup_hooks/quit_menu/save_flow_boxes.rs");
include!("startup_hooks/quit_menu/save_dest_identity.rs");
include!("startup_hooks/quit_menu/save_dest_commit.rs");
include!("startup_hooks/quit_menu/save_picker_menu.rs");
mod quit_menu;
pub(crate) use quit_menu::*;
include!("startup_hooks/quit_menu/save_swap_profile_table.rs");
include!("startup_hooks/quit_menu/system_quit_ownership_repro.rs");
include!("startup_hooks/quit_menu/system_quit_repro_guards.rs");
include!("startup_hooks/quit_menu/system_quit_hooks.rs");

// Product (A): boot missing-save picker and shared OS-native picker mechanism.
include!("startup_hooks/save_picker/save_picker_os_dialog.rs");
include!("startup_hooks/save_picker/save_picker_boot.rs");
include!("startup_hooks/save_picker/save_picker_surface.rs");

// Runtime diagnostics / agent probes.
mod diagnostics;
pub(crate) use diagnostics::*;
include!("startup_hooks/diagnostics/layout_global_hooks.rs");
