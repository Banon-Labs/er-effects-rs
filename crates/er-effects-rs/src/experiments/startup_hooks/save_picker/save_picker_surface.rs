use super::*;

// Product-side surface router. The pure picker surface/outcome decisions live in `er-save-picker`;
// this root shim keeps the compatibility names and owns the runtime hook glue, root config latch,
// and System>Quit state staging.

pub(crate) use er_save_picker::{
    DestRoute, PickerOpenOutcome, PickerOpenRequest, PickerSurface, SaveDestOrigin,
    open_taken_over_outcome, picker_surface_for, save_dest_route_picked_target,
};

/// True when this session's picker surface is the OS dialog.
///
/// Reads the latch `init_runtime_config` set from `os_native_save_picker_from()`, so the config
/// is walked once at attach and every runtime decision is a single load. It also inherits the same
/// fail-safe direction: a session where the config never loaded leaves the latch at 0, the in-game
/// browser.
pub(crate) fn os_native_picker_active() -> bool {
    SAVE_PICKER_SURFACE.load(Ordering::SeqCst) != 0
}

/// Open the picker this request's surface calls for, and report what the request did.
///
/// The shape of the request/outcome is owned by `er-save-picker`; this function is deliberately the
/// root-owned glue that calls native menu hooks, root save-flow staging, and boot-thread OS dialog
/// code.
pub(crate) unsafe fn open_picker_for_intent(request: PickerOpenRequest) -> PickerOpenOutcome {
    let surface = picker_surface_for(os_native_picker_active());
    match (surface, request) {
        (PickerSurface::InGame, PickerOpenRequest::LoadSource { action_obj }) => {
            open_taken_over_outcome(unsafe {
                system_quit_open_save_picker_menu_in_game(action_obj)
            })
        }
        (PickerSurface::InGame, PickerOpenRequest::SaveDestination { system_dialog }) => {
            open_taken_over_outcome(unsafe {
                system_quit_open_save_dest_picker_in_game(system_dialog)
            })
        }
        (PickerSurface::InGame, PickerOpenRequest::MissingSaveBoot) => {
            open_taken_over_outcome(crate::experiments::boot_arm_missing_save_picker_in_game())
        }
        (PickerSurface::OsNative, PickerOpenRequest::LoadSource { action_obj }) => unsafe {
            os_open_save_picker_load(action_obj)
        },
        (PickerSurface::OsNative, PickerOpenRequest::SaveDestination { system_dialog }) => unsafe {
            os_open_save_dest_picker(system_dialog)
        },
        (PickerSurface::OsNative, PickerOpenRequest::MissingSaveBoot) => {
            open_taken_over_outcome(boot_os_open_missing_save_picker())
        }
    }
}

/// Where a save-DESTINATION browser starts, the leaf a new file there is given, and which file is
/// the loaded one. `None` (with a logged reason) when the loaded save cannot be resolved or no
/// readable folder exists.
///
/// BOTH surfaces call this, so they cannot drift. This remains root-owned because it reads the
/// active save path from the product runtime and logs through the product diagnostics.
pub(crate) fn save_dest_start_dir() -> Option<SaveDestOrigin> {
    let save_path = match system_quit_env_save_path() {
        Ok(path) => path,
        Err(reason) => {
            append_autoload_debug(format_args!(
                "save-dest-picker: refused to open -- {reason}"
            ));
            return None;
        }
    };
    let loaded_path = PathBuf::from(save_picker_windows_path_string(&save_path));
    let loaded_file_name = match Path::new(&save_path)
        .file_name()
        .and_then(|name| name.to_str())
    {
        Some(name) => name.to_owned(),
        None => {
            append_autoload_debug(format_args!(
                "save-dest-picker: refused to open -- loaded save '{save_path}' has no file name"
            ));
            return None;
        }
    };
    // Start where the loaded save lives; fall back to the default save root only if that directory
    // is gone.
    let start_dir = system_quit_env_save_dir()
        .ok()
        .map(|dir| PathBuf::from(save_picker_windows_path_string(&dir)))
        .filter(|dir| dir.is_dir())
        .or_else(|| {
            default_save_root()
                .and_then(|root| root.to_str().map(save_picker_windows_path_string))
                .map(PathBuf::from)
                .filter(|root| root.is_dir())
        });
    let Some(start_dir) = start_dir else {
        append_autoload_debug(format_args!(
            "save-dest-picker: refused to open -- neither the loaded save's directory nor the default save root is readable"
        ));
        return None;
    };
    Some(SaveDestOrigin {
        start_dir,
        loaded_file_name,
        loaded_path,
    })
}

#[cfg(test)]
mod save_picker_surface_tests {
    use super::*;

    /// The process latch starts at 0, so absent/failed root config stays on the in-game browser.
    #[test]
    fn the_uninitialized_runtime_latch_defaults_to_the_in_game_browser() {
        assert_eq!(picker_surface_for(false), PickerSurface::InGame);
        assert!(
            !os_native_picker_active(),
            "an uninitialized surface latch must read as the in-game browser"
        );
    }
}
