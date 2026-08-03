//! Shared save-picker surface and destination routing decisions.
//!
//! This module owns the pure/product-owned parts of "which picker opens" and "what a picked
//! destination means". Runtime hook glue still lives in the host product crate: opening an in-game
//! native menu, staging a System>Quit save flow, and reading root process config are host actions.

use std::path::{Path, PathBuf};

/// Which surface is asking for a picker, and the native handle that surface owns.
///
/// `LoadSource` carries the row's action object. `SaveDestination` carries the System dialog
/// directly, because the save flow -- not a row press -- opens that browser. `MissingSaveBoot`
/// carries nothing: at a no-save boot the game's menu assets are not built, so the in-game arm is
/// the DLL-drawn overlay rather than a native menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerOpenRequest {
    LoadSource { action_obj: usize },
    SaveDestination { system_dialog: usize },
    MissingSaveBoot,
}

/// Which picker surface an open resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerSurface {
    /// The native/in-game browser surface (the default; the surface the build gate covers).
    InGame,
    /// The OS common file dialog (`os_native_save_picker = true`).
    OsNative,
}

/// What an "open a picker" request did -- distinct from "is a picker up now".
///
/// `Dismissed` is terminal because a picker ran and produced no usable destination. Only
/// `NotOpened` leaves the request owed and therefore retryable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerOpenOutcome {
    /// A picker is up (in-game) or a path was accepted and staged (OS).
    Opened,
    /// The picker ran and produced no destination: the user cancelled, the dialog failed, the
    /// invalid-pick reopen bound gave up, or the ingest refused the pick. Nothing is staged.
    Dismissed,
    /// No picker ran: a refusal or deferred submit. The request still stands.
    NotOpened,
}

impl PickerOpenOutcome {
    /// Whether the open request has been carried out and must not be re-armed.
    pub fn request_discharged(self) -> bool {
        !matches!(self, PickerOpenOutcome::NotOpened)
    }
}

/// Lift a bool-returning arm where all it can report is whether a surface took ownership.
pub fn open_taken_over_outcome(taken: bool) -> PickerOpenOutcome {
    if taken {
        PickerOpenOutcome::Opened
    } else {
        PickerOpenOutcome::NotOpened
    }
}

/// Resolve the surface from the config flag.
pub fn picker_surface_for(os_enabled: bool) -> PickerSurface {
    if os_enabled {
        PickerSurface::OsNative
    } else {
        PickerSurface::InGame
    }
}

/// Where a save-destination browser opens, and what it needs to know about the save already loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveDestOrigin {
    /// Folder the browser opens in.
    pub start_dir: PathBuf,
    /// Leaf the `[ new ]` row and the OS Save-As filename field are pre-filled with.
    pub loaded_file_name: String,
    /// Full path of the save currently loaded, for the `[CURRENT]` row marker.
    pub loaded_path: PathBuf,
}

/// What a chosen destination becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestRoute {
    /// The file is already there: the overwrite confirm decides.
    ConfirmOverwrite,
    /// A name nobody is using: stage the commit directly.
    CommitDirect,
}

/// Route a chosen destination. Directories are not files, so they never ask the overwrite confirm.
pub fn save_dest_route_picked_target(target: &Path) -> DestRoute {
    if target.is_file() {
        DestRoute::ConfirmOverwrite
    } else {
        DestRoute::CommitDirect
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for (os_enabled, expected) in [
            (false, PickerSurface::InGame),
            (true, PickerSurface::OsNative),
        ] {
            for request in requests {
                assert_eq!(
                    picker_surface_for(os_enabled),
                    expected,
                    "os_native_save_picker={os_enabled} must resolve {request:?} to {expected:?}"
                );
            }
        }
    }

    #[test]
    fn a_dismissed_picker_discharges_the_open_request_and_only_a_never_opened_one_retries() {
        assert!(PickerOpenOutcome::Dismissed.request_discharged());
        assert!(PickerOpenOutcome::Opened.request_discharged());
        assert!(!PickerOpenOutcome::NotOpened.request_discharged());
    }

    #[test]
    fn the_bool_returning_arms_map_only_to_opened_or_not_opened() {
        assert_eq!(open_taken_over_outcome(true), PickerOpenOutcome::Opened);
        assert_eq!(open_taken_over_outcome(false), PickerOpenOutcome::NotOpened);
        assert!(open_taken_over_outcome(true).request_discharged());
        assert!(!open_taken_over_outcome(false).request_discharged());
    }

    #[test]
    fn absent_surface_key_defaults_to_in_game_picker() {
        assert_eq!(picker_surface_for(false), PickerSurface::InGame);
    }

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
