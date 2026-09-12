// Product (B) System>Quit modules.
// Shared imports preserved from the former flat startup-hook namespace for child modules.
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use crate::*;
use crate::{crashlog::*, telemetry::*};
use eldenring::cs::PlayerIns;
use er_telemetry_core::counters::PROFILE_STATS_PREVIEW_ROW_CURSOR;
use er_telemetry_core::counters::SAVE_DEST_OPEN_PICKER_PENDING;
use er_telemetry_core::counters::SAVE_DEST_PICKER_OPEN_RETRY_COUNT;
use er_telemetry_core::counters::SAVE_PICKER_MODE_ACTIVE;
use er_telemetry_core::counters::SYSTEM_QUIT_SAVE_SWAP_POLL_TICK;
use fromsoftware_shared::FromStatic;
use std::{
    ffi::c_void,
    fs,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

pub(crate) mod profile_rows_system_quit_menu;
pub(crate) use profile_rows_system_quit_menu::*;

pub(crate) mod build_url_row;
pub(crate) use build_url_row::*;

pub(crate) mod generate_build_link_row;
pub(crate) use generate_build_link_row::*;

pub(crate) mod build_url_editor;
pub(crate) use build_url_editor::*;

pub(crate) mod build_url_backdrop;
pub(crate) use build_url_backdrop::*;

pub(crate) mod system_quit_dialog_handlers;
pub(crate) use system_quit_dialog_handlers::*;

pub(crate) mod save_swap_profile_table;
pub(crate) use save_swap_profile_table::*;

pub(crate) mod system_quit_repro_guards;
pub(crate) use system_quit_repro_guards::*;

pub(crate) mod system_quit_hooks;
pub(crate) use system_quit_hooks::*;
