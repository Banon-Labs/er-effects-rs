// Product (B) System>Quit modules.
// Shared imports preserved from the former flat startup-hook namespace for child modules.
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use crate::*;
use crate::{crashlog::*, ffi::*, telemetry::*};
use eldenring::cs::PlayerIns;
use er_telemetry_core::counters::PROFILE_STATS_PREVIEW_ROW_CURSOR;
use er_telemetry_core::counters::SAVE_DEST_OPEN_PICKER_PENDING;
use er_telemetry_core::counters::SAVE_DEST_PICKER_OPEN_RETRY_COUNT;
use er_telemetry_core::counters::SAVE_PICKER_MODE_ACTIVE;
use er_telemetry_core::counters::SAVE_PICKER_REBUILD_PENDING_DIALOG;
use er_telemetry_core::counters::SYSTEM_QUIT_SAVE_SWAP_POLL_TICK;
use fromsoftware_shared::FromStatic;
use std::{
    ffi::c_void,
    fs,
    path::Path,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

pub(crate) mod profile_05_010_editor_runtime;
pub(crate) use profile_05_010_editor_runtime::*;

pub(crate) mod profile_rows_system_quit_menu;
pub(crate) use profile_rows_system_quit_menu::*;

// The four modules behind the two build rows, compiled only when this build asks for them. The
// files are always here; `build-rows` decides whether they are in the DLL.
#[cfg(feature = "build-rows")]
pub(crate) mod build_url_row;
#[cfg(feature = "build-rows")]
pub(crate) use build_url_row::*;

#[cfg(feature = "build-rows")]
pub(crate) mod generate_build_link_row;
#[cfg(feature = "build-rows")]
pub(crate) use generate_build_link_row::*;

#[cfg(feature = "build-rows")]
pub(crate) mod build_url_editor;
#[cfg(feature = "build-rows")]
pub(crate) use build_url_editor::*;

#[cfg(feature = "build-rows")]
pub(crate) mod build_url_backdrop;
#[cfg(feature = "build-rows")]
pub(crate) use build_url_backdrop::*;

pub(crate) mod system_quit_dialog_handlers;
pub(crate) use system_quit_dialog_handlers::*;

pub(crate) mod save_flow_boxes;
pub(crate) use save_flow_boxes::*;

pub(crate) mod save_dest_commit;
pub(crate) use save_dest_commit::*;

pub(crate) mod save_picker_menu;
pub(crate) use save_picker_menu::*;

pub(crate) mod save_picker_path_editor;
pub(crate) use save_picker_path_editor::*;

pub(crate) mod save_swap_profile_table;
pub(crate) use save_swap_profile_table::*;

pub(crate) mod system_quit_ownership_repro;
pub(crate) use system_quit_ownership_repro::*;

pub(crate) mod system_quit_repro_guards;
pub(crate) use system_quit_repro_guards::*;

pub(crate) mod system_quit_hooks;
pub(crate) use system_quit_hooks::*;
