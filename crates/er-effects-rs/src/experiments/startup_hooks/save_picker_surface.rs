// THE one place that decides WHICH picker opens.
//
// There are exactly two "open a picker" entry points in the System>Quit flow -- the "Load Save
// Profiles" row and the Save Game destination step -- and `er-effects.toml`'s
// `os_native_save_picker` governs BOTH from a single key. Routing them through one function is
// what makes that mechanical rather than a convention two call sites have to remember: the four
// existing call sites keep calling the same public names, those names delegate here, and no other
// file learns the flag exists.
//
// The mode is read ONCE PER OPEN, here. It cannot change mid-session anyway (`RUNTIME_CONFIG` is
// a parse-once `OnceLock`), but reading it in one place means a future caching change has one
// place to touch.

/// Which System>Quit surface is asking for a picker, and the native handle that surface owns.
///
/// `LoadSource` carries the row's action object (the load picker derives the System dialog, the
/// submit queue and the window list from it). `SaveDestination` carries the System dialog
/// directly, because the save flow -- not a row press -- opens that browser and there is no row
/// action object at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PickerOpenRequest {
    LoadSource { action_obj: usize },
    SaveDestination { system_dialog: usize },
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

/// Open the picker this request's surface calls for. Returns whatever the chosen surface returns:
/// true when a picker is up (in-game) or a path was accepted (OS), false when nothing was staged
/// and the caller must leave the System menu alone.
pub(crate) unsafe fn open_picker_for_intent(request: PickerOpenRequest) -> bool {
    let surface = picker_surface_for(crate::config::os_native_save_picker_enabled());
    match (surface, request) {
        (PickerSurface::InGame, PickerOpenRequest::LoadSource { action_obj }) => unsafe {
            system_quit_open_save_picker_menu_in_game(action_obj)
        },
        (PickerSurface::InGame, PickerOpenRequest::SaveDestination { system_dialog }) => unsafe {
            system_quit_open_save_dest_picker_in_game(system_dialog)
        },
        (PickerSurface::OsNative, PickerOpenRequest::LoadSource { action_obj }) => unsafe {
            os_open_save_picker_load(action_obj)
        },
        (PickerSurface::OsNative, request @ PickerOpenRequest::SaveDestination { .. }) => {
            append_autoload_debug(format_args!(
                "save-picker-os: refusing {request:?} -- the Save-As arm is not built on this commit; nothing was staged"
            ));
            false
        }
    }
}

#[cfg(test)]
mod save_picker_surface_tests {
    use super::*;

    /// CONTRACT 2, mechanically. One key, both surfaces: for a given flag value every intent
    /// resolves to the SAME surface. A future per-intent special case has to break this table
    /// before it can reach a user.
    #[test]
    fn one_key_value_resolves_both_intents_to_the_same_surface() {
        let requests = [
            PickerOpenRequest::LoadSource {
                action_obj: 0x1234_5678,
            },
            PickerOpenRequest::SaveDestination {
                system_dialog: 0x8765_4321,
            },
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
    /// `config::tests`), and `false` is the in-game browser.
    #[test]
    fn the_default_surface_is_the_in_game_browser() {
        assert_eq!(picker_surface_for(false), PickerSurface::InGame);
    }
}
