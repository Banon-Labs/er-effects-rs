//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

#![allow(unused_imports)]

include!("startup_hooks/title_scaleform_msgbox.rs");
include!("startup_hooks/startup_modals_menu_cover.rs");
include!("startup_hooks/dlstring_lookat_math.rs");
include!("startup_hooks/lookat_bone_hooks.rs");
include!("startup_hooks/lookat_stage_camera.rs");
include!("startup_hooks/loading_cover_save_slot.rs");
include!("startup_hooks/save_swap_profile_table.rs");
include!("startup_hooks/profile_table_gfx_files.rs");
include!("startup_hooks/title_resources_stats_text.rs");
include!("startup_hooks/profile_rows_system_quit_menu.rs");
include!("startup_hooks/system_quit_dialog_handlers.rs");
include!("startup_hooks/system_quit_ownership_repro.rs");
include!("startup_hooks/system_quit_repro_guards.rs");
include!("startup_hooks/system_quit_hooks.rs");
include!("startup_hooks/layout_global_hooks.rs");
