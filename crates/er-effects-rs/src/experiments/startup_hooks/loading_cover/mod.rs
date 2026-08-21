// Product loading-cover / title resource modules.
// Shared imports preserved from the former flat startup-hook namespace for child modules.
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use crate::*;
use crate::{crashlog::*, hooks::*, telemetry::*};
use eldenring::cs::PlayerIns;
use er_quit_menu::save_flow_boxes::{SAVE_FLOW_BOX_NONE, save_flow_box_label};
use er_telemetry::counters::OPTIONS_02_040_QUIT4_RUNTIME_FAILURES;
use er_telemetry::counters::OPTIONS_02_040_QUIT4_RUNTIME_SERVES;
use fromsoftware_shared::FromStatic;
use std::{
    ffi::{CStr, c_void},
    fs,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::UNIX_EPOCH,
};

pub(crate) mod title_scaleform_msgbox;
pub(crate) use title_scaleform_msgbox::*;

pub(crate) mod startup_modals_menu_cover;
pub(crate) use startup_modals_menu_cover::*;

pub(crate) mod loading_cover_save_slot;
pub(crate) use loading_cover_save_slot::*;

pub(crate) mod portrait_equip_oracle;
pub(crate) use portrait_equip_oracle::*;

pub(crate) mod profile_table_gfx_files;
pub(crate) use profile_table_gfx_files::*;

pub(crate) mod title_resources_stats_text;
pub(crate) use title_resources_stats_text::*;

pub(crate) mod scaleform_descriptor_guard;
pub(crate) use scaleform_descriptor_guard::*;

pub(crate) mod window_reconfig_observer;
pub(crate) use window_reconfig_observer::*;
