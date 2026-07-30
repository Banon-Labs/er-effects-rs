// THE one place that decides WHICH picker opens.
//
// There are exactly three "open a picker" entry points -- the System>Quit "Load Save Profiles"
// row, the System>Quit Save Game destination step, and the MISSING-SAVE BOOT -- and
// `er-effects.toml`'s `os_native_save_picker` governs ALL THREE from a single key. Routing them
// through one function is what makes that mechanical rather than a convention three call sites
// have to remember: the existing call sites keep calling the same public names, those names
// delegate here, and no other file learns the flag exists.
//
// That is not a hypothetical. The boot intent was ADDED here because it was the one entry point
// that never routed through this function: the boot arm called the in-game overlay directly, so a
// user with `os_native_save_picker = true` got the OS dialog at System>Quit and the in-game
// browser at boot, with no code anywhere reading the key on that path. A per-intent surface
// decision is exactly what the table test below now forbids.
//
// The mode is read ONCE PER OPEN, here. It cannot change mid-session anyway (`RUNTIME_CONFIG` is
// a parse-once `OnceLock`), but reading it in one place means a future caching change has one
// place to touch.
//
// WHAT THE SURFACE DECISION IS NOT. The surface is uniform across intents; the OUTCOME OF A CANCEL
// is not, and deliberately so. A cancelled System>Quit picker discharges its open request and
// returns to the System menu (the #107 fix). A cancelled BOOT picker QUITS THE GAME, because at a
// missing-save boot there is no menu to return to and world entry stays denied until a save is
// chosen -- "OK -> choose a save, Cancel -> exit", the contract `path_hooks.rs` has documented
// since the pre-in-game-picker era. Per-intent cancel semantics live in each intent's own arm; the
// discrimination is by INTENT (an enum variant, checked by the compiler) and never by a flag read
// inside the OS dialog code.
//
// The rest of the file is the decisions BOTH surfaces must share, for the same reason: where a
// destination browser starts (`save_dest_start_dir`) and what a chosen destination becomes
// (`save_dest_route_picked_target`). A copy of either in the OS arm is how the modes would drift.

/// Which surface is asking for a picker, and the native handle that surface owns.
///
/// `LoadSource` carries the row's action object (the load picker derives the System dialog, the
/// submit queue and the window list from it). `SaveDestination` carries the System dialog
/// directly, because the save flow -- not a row press -- opens that browser and there is no row
/// action object at all. `MissingSaveBoot` carries NOTHING, and the absence is the information:
/// at a no-save boot the game's menu assets are not built, there is no `05_010` window and no row
/// action object to derive anything from, which is why that intent's in-game arm is the DLL-drawn
/// overlay rather than a native menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PickerOpenRequest {
    LoadSource { action_obj: usize },
    SaveDestination { system_dialog: usize },
    MissingSaveBoot,
}

/// Which picker surface an open resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PickerSurface {
    /// The native `05_010_ProfileSelect` browse rows (default; the surface the build gate covers).
    InGame,
    /// The OS common file dialog (`os_native_save_picker = true`).
    OsNative,
}

/// Resolve the surface from the flag. Takes the bool as an ARGUMENT rather than reading the
/// config, so the invariant the contract cares about -- one key value yields the same surface for
/// BOTH intents -- is provable by a table test instead of by reviewer discipline.
fn picker_surface_for(os_enabled: bool) -> PickerSurface {
    if os_enabled {
        PickerSurface::OsNative
    } else {
        PickerSurface::InGame
    }
}

/// True when this session's picker surface is the OS dialog.
///
/// Reads the latch `init_runtime_config` set from `os_native_save_picker_enabled()`, so the config
/// is walked once at attach and every runtime decision is a single load. It also inherits the same
/// fail-safe direction: a session where the config never loaded leaves the latch at 0, the in-game
/// browser.
pub(crate) fn os_native_picker_active() -> bool {
    SAVE_PICKER_SURFACE.load(Ordering::SeqCst) != 0
}

/// Open the picker this request's surface calls for. Returns whatever the chosen surface returns:
/// true when a picker is up (in-game) or a path was accepted (OS), false when nothing was staged
/// and the caller must leave the System menu alone.
///
/// The boot arms return "this open was TAKEN OVER by a surface". A `false` there is not a failure
/// and not a cancel -- it is "nothing owns the pick yet, ask again on the next tick" -- which is
/// what lets the OS boot arm wait for the core `CreateFileW` detour to go live without either
/// spinning a dialog or stranding the boot.
pub(crate) unsafe fn open_picker_for_intent(request: PickerOpenRequest) -> bool {
    let surface = picker_surface_for(os_native_picker_active());
    match (surface, request) {
        (PickerSurface::InGame, PickerOpenRequest::LoadSource { action_obj }) => unsafe {
            system_quit_open_save_picker_menu_in_game(action_obj)
        },
        (PickerSurface::InGame, PickerOpenRequest::SaveDestination { system_dialog }) => unsafe {
            system_quit_open_save_dest_picker_in_game(system_dialog)
        },
        (PickerSurface::InGame, PickerOpenRequest::MissingSaveBoot) => {
            crate::experiments::boot_arm_missing_save_picker_in_game()
        }
        (PickerSurface::OsNative, PickerOpenRequest::LoadSource { action_obj }) => unsafe {
            os_open_save_picker_load(action_obj)
        },
        (PickerSurface::OsNative, PickerOpenRequest::SaveDestination { system_dialog }) => unsafe {
            os_open_save_dest_picker(system_dialog)
        },
        (PickerSurface::OsNative, PickerOpenRequest::MissingSaveBoot) => {
            boot_os_open_missing_save_picker()
        }
    }
}

/// Where a save-DESTINATION browser starts, and the leaf a new file there is given. `None` (with a
/// logged reason) when the loaded save cannot be resolved or no readable folder exists.
///
/// BOTH surfaces call this, so they cannot drift. Contract 8 read per surface: a destination starts
/// at the LOADED SAVE'S OWN folder, deliberately NOT at the remembered `preferred_save_picker_dir`
/// -- "save next to the save you loaded" is the expected default and the remembered dir belongs to
/// the LOAD flow, which is where `save_picker_start_dir` consults it. If that reading is ever
/// changed, it changes here, for both modes at once.
fn save_dest_start_dir() -> Option<(PathBuf, String)> {
    let save_path = match system_quit_env_save_path() {
        Ok(path) => path,
        Err(reason) => {
            append_autoload_debug(format_args!(
                "save-dest-picker: refused to open -- {reason}"
            ));
            return None;
        }
    };
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
    Some((start_dir, loaded_file_name))
}

/// What a chosen destination becomes. Mode-free on purpose: a `[ new ]` row, a picked existing file
/// in the in-game browser and an OS Save-As all route through the same decision, so the overwrite
/// gate cannot differ between surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DestRoute {
    /// The file is already there: Box3 decides, and Box3 is the SINGLE overwrite gate (which is why
    /// the OS Save-As does not set `OFN_OVERWRITEPROMPT`).
    ConfirmOverwrite,
    /// A name nobody is using: stage the commit.
    CommitDirect,
}

/// Route a chosen destination. Known and unfixed (bd `er-effects-rs-8tq4` item 16): this existence
/// check runs on the menu thread while the seed runs frames later on the game thread, and Save-As
/// WIDENS that window because the user may sit in the dialog for a minute. The fix belongs to that
/// issue -- a re-check at arm time in `save_dest_arm_redirect`.
pub(crate) fn save_dest_route_picked_target(target: &Path) -> DestRoute {
    if target.is_file() {
        DestRoute::ConfirmOverwrite
    } else {
        DestRoute::CommitDirect
    }
}

#[cfg(test)]
mod save_picker_surface_tests {
    use super::*;

    /// CONTRACT 2, mechanically. One key, EVERY surface: for a given flag value every intent
    /// resolves to the SAME surface. A future per-intent special case has to break this table
    /// before it can reach a user.
    ///
    /// `MissingSaveBoot` is in this table because it is the intent that was MISSING one: the boot
    /// arm bypassed `open_picker_for_intent` entirely and always drew the in-game overlay, so
    /// `os_native_save_picker = true` was silently ignored at a missing-save boot. Listing it here
    /// is what makes that regression impossible to reintroduce quietly.
    #[test]
    fn one_key_value_resolves_every_intent_to_the_same_surface() {
        let requests = [
            PickerOpenRequest::LoadSource {
                action_obj: 0x1234_5678,
            },
            PickerOpenRequest::SaveDestination {
                system_dialog: 0x8765_4321,
            },
            PickerOpenRequest::MissingSaveBoot,
        ];
        for (os_enabled, expected) in [(false, PickerSurface::InGame), (true, PickerSurface::OsNative)]
        {
            for request in requests {
                assert_eq!(
                    picker_surface_for(os_enabled),
                    expected,
                    "os_native_save_picker={os_enabled} must resolve {request:?} to {expected:?}"
                );
            }
        }
    }

    /// THE DEFAULT MUST NOT MOVE: absent/failed config resolves to `false` (pinned in
    /// `config::tests`), and `false` is the in-game browser. `os_native_picker_active` reads the
    /// latch, which is 0 until `init_runtime_config` says otherwise -- so a process where the
    /// config never loaded also lands here.
    #[test]
    fn the_default_surface_is_the_in_game_browser() {
        assert_eq!(picker_surface_for(false), PickerSurface::InGame);
        assert!(
            !os_native_picker_active(),
            "an uninitialized surface latch must read as the in-game browser"
        );
    }

    /// The overwrite gate is a property of the TARGET, not of the surface that chose it. `[ new ]`,
    /// a picked existing file and an OS Save-As all ask this one question, which is what keeps Box3
    /// the single overwrite gate in both modes.
    #[test]
    fn an_existing_target_confirms_and_a_free_name_commits() {
        let dir = std::env::temp_dir().join("er-save-dest-route");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
        let existing = dir.join("ER0000.sl2");
        std::fs::write(&existing, b"already here").expect("temp file must be writable");
        assert_eq!(
            save_dest_route_picked_target(&existing),
            DestRoute::ConfirmOverwrite
        );
        assert_eq!(
            save_dest_route_picked_target(&dir.join("brand-new.sl2")),
            DestRoute::CommitDirect
        );
        assert_eq!(
            save_dest_route_picked_target(&dir),
            DestRoute::CommitDirect,
            "a directory is not a file, so it never routes to the overwrite confirm"
        );
    }
}
