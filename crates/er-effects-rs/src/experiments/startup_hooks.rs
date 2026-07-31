//! experiments module (split from lib.rs; pure code reorganization, no behavior change).
//!
//! The loading-screen portrait capture pipeline + stats producer
//! (dlstring_lookat_math, lookat_bone_hooks, lookat_stage_camera, stats_loading_text)
//! moved to the `er-loading-portrait` crate (portrait crate split); the glob shim below
//! re-exports it so every remaining flat-namespace reference keeps compiling unchanged.

#![allow(unused_imports)]

pub(crate) use er_loading_portrait::*;

include!("startup_hooks/title_scaleform_msgbox.rs");
include!("startup_hooks/startup_modals_menu_cover.rs");
include!("startup_hooks/loading_cover_save_slot.rs");
include!("startup_hooks/save_swap_profile_table.rs");
include!("startup_hooks/profile_table_gfx_files.rs");
include!("startup_hooks/scaleform_descriptor_guard.rs");
include!("startup_hooks/title_resources_stats_text.rs");
include!("startup_hooks/profile_rows_system_quit_menu.rs");
include!("startup_hooks/system_quit_row_identity.rs");
include!("startup_hooks/system_quit_dialog_handlers.rs");
include!("startup_hooks/save_flow_boxes.rs");
include!("startup_hooks/save_dest_identity.rs");
include!("startup_hooks/save_dest_commit.rs");
include!("startup_hooks/save_picker_menu.rs");
include!("startup_hooks/save_picker_dim_overlay.rs");
include!("startup_hooks/save_picker_os_dialog.rs");
include!("startup_hooks/save_picker_boot.rs");
include!("startup_hooks/save_picker_surface.rs");
include!("startup_hooks/system_quit_ownership_repro.rs");
include!("startup_hooks/msb_parse_trace.rs");
include!("startup_hooks/loadlist_wait_trace.rs");
include!("startup_hooks/dlc_roots_trace.rs");
include!("startup_hooks/dlc_roots_self_heal.rs");
include!("startup_hooks/system_quit_repro_guards.rs");
include!("startup_hooks/system_quit_hooks.rs");
include!("startup_hooks/layout_global_hooks.rs");
include!("startup_hooks/window_reconfig_observer.rs");
