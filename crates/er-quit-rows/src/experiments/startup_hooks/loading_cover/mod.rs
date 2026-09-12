// Product loading-cover / title resource modules.
// Shared imports preserved from the former flat startup-hook namespace for child modules.
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use crate::*;
use crate::{crashlog::*, telemetry::*};
use eldenring::cs::PlayerIns;
use er_quit_menu_core::save_flow_boxes::{SAVE_FLOW_BOX_NONE, save_flow_box_label};
use er_telemetry_core::counters::OPTIONS_02_040_QUIT6_RUNTIME_FAILURES;
use er_telemetry_core::counters::OPTIONS_02_040_QUIT6_RUNTIME_SERVES;
use fromsoftware_shared::FromStatic;
use std::{
    ffi::c_void,
    fs,
    sync::{
        Mutex, Once, OnceLock,
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

// Moved to er-loading-portrait-core; the file that remains is a documentation-only facade and
// exports nothing, so it carries no `pub(crate) use` (which rustc would flag as unused).
pub(crate) mod portrait_equip_oracle;

pub(crate) mod profile_table_gfx_files;
pub(crate) use profile_table_gfx_files::*;

pub(crate) mod title_resources_stats_text;
pub(crate) use title_resources_stats_text::*;

pub(crate) mod scaleform_descriptor_guard;
pub(crate) use scaleform_descriptor_guard::*;

pub(crate) mod window_reconfig_observer;
pub(crate) use window_reconfig_observer::install_window_reconfig_observer_hooks;

/// Install the `er-loading-portrait-core` loading-cover seam, exactly once.
///
/// The portrait seam (`PortraitHost`) is installed from `DllMain`. This one cannot be: adding
/// fields to that struct literal would edit `lib_parts/dll_entry_parts/bootstrap.rs`, which is the
/// spine several parallel crate extractions hang off. Instead every facade entry point into the
/// moved loading-cover code calls this first, so the seam is always installed before any moved
/// code can read through it -- the moved modules are unreachable from the root by any other path.
///
/// `OnceLock::set` is the idempotence, and `Once` keeps the steady-state cost to one relaxed load,
/// which is why a per-frame facade wrapper can call it unconditionally.
pub(crate) fn ensure_loading_cover_host() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        er_loading_portrait_core::install_loading_cover_host(
            er_loading_portrait_core::LoadingCoverHost {
                resolve_module_proc: crate::hooks::safe_input_proc,
                game_main_window: crate::hooks::own_window,
                create_absolute_hook: crate::hooks::create_absolute_hook,
                push_json_usize: crate::telemetry::push_json_usize,
                process_log_elapsed_ms: crate::telemetry::process_log_elapsed_ms,
                boot_view_epoch_ms_if_anchored: crate::experiments::boot_view_epoch_ms_if_anchored,
                fake_loading_screen_visible: crate::experiments::fake_loading_screen_visible,
            },
        );
    });
}
