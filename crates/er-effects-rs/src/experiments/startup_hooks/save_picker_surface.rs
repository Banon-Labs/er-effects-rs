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
//
// The rest of the file is the decisions BOTH surfaces must share, for the same reason: where a
// destination browser starts (`save_dest_start_dir`) and what a chosen destination becomes
// (`save_dest_route_picked_target`). A copy of either in the OS arm is how the modes would drift.

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

/// What an "open a picker" REQUEST did -- which is NOT the same question as "is a picker up now".
///
/// These three used to be two spellings of one `bool`, and that collapse IS the reopen loop the OS
/// dialog trapped users in (bd `er-effects-rs-rsxi`). The menu-pump consumer of
/// `SAVE_DEST_OPEN_PICKER_PENDING` reads a `false` as "the open never happened, retry on the next
/// pump" -- correct for a MenuJob submit the dialog's queue deferred, catastrophic for a user who
/// just pressed Cancel: the request stayed armed and comdlg32 reopened ~57 ms later, forever.
///
/// The distinction that fixes it is OWNERSHIP OF THE REQUEST, not user intent: a picker that RAN
/// has carried the request out whatever the user decided, and only a picker that never ran is still
/// owed one. `Dismissed` is therefore a first-class terminal answer, not a failure to ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PickerOpenOutcome {
    /// A picker is up (in-game) or a path was accepted and staged (OS).
    Opened,
    /// The picker RAN and produced no destination: the user cancelled, comdlg32 failed, the
    /// invalid-pick reopen bound gave up, or the ingest refused the pick. Nothing is staged.
    Dismissed,
    /// NO picker ran: a refusal (unresolvable directory, non-heap dialog, detour not live yet, a
    /// re-entrant open) or a submit the menu pump deferred. The request still stands.
    NotOpened,
}

impl PickerOpenOutcome {
    /// Whether the open request has been carried out and MUST NOT be re-armed.
    ///
    /// This is the single predicate the reopen loop got wrong. Re-arming on a `Dismissed` is the
    /// loop; re-arming on a `NotOpened` is the deferred-submit retry the in-game surface needs.
    pub(crate) fn request_discharged(self) -> bool {
        !matches!(self, PickerOpenOutcome::NotOpened)
    }
}

/// The in-game arms answer a strictly smaller question: a window is up, or the submit did not
/// happen and the caller may try again. Backing OUT of a live `05_010` browser is a LATER event
/// with its own path (`save_picker_reset`), never this return value -- which is exactly why the
/// in-game surface never looped and the OS surface did.
fn in_game_open_outcome(opened: bool) -> PickerOpenOutcome {
    if opened {
        PickerOpenOutcome::Opened
    } else {
        PickerOpenOutcome::NotOpened
    }
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

/// Open the picker this request's surface calls for, and report what the request DID -- see
/// [`PickerOpenOutcome`], whose three states are what keep a dismissal from being retried.
pub(crate) unsafe fn open_picker_for_intent(request: PickerOpenRequest) -> PickerOpenOutcome {
    let surface = picker_surface_for(os_native_picker_active());
    match (surface, request) {
        (PickerSurface::InGame, PickerOpenRequest::LoadSource { action_obj }) => {
            in_game_open_outcome(unsafe { system_quit_open_save_picker_menu_in_game(action_obj) })
        }
        (PickerSurface::InGame, PickerOpenRequest::SaveDestination { system_dialog }) => {
            in_game_open_outcome(unsafe {
                system_quit_open_save_dest_picker_in_game(system_dialog)
            })
        }
        (PickerSurface::OsNative, PickerOpenRequest::LoadSource { action_obj }) => unsafe {
            os_open_save_picker_load(action_obj)
        },
        (PickerSurface::OsNative, PickerOpenRequest::SaveDestination { system_dialog }) => unsafe {
            os_open_save_dest_picker(system_dialog)
        },
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

    /// THE REOPEN-LOOP REGRESSION (bd `er-effects-rs-rsxi`). A picker that RAN discharges the open
    /// request whatever the user decided; only a picker that never ran is still owed one. Collapsing
    /// `Dismissed` into `NotOpened` -- which a `bool` return has no way not to do -- is precisely
    /// what made the menu pump re-ask a question the user had just answered with Cancel, reopening
    /// comdlg32 every ~57 ms with no way out of the save flow.
    #[test]
    fn a_dismissed_picker_discharges_the_open_request_and_only_a_never_opened_one_retries() {
        assert!(
            PickerOpenOutcome::Dismissed.request_discharged(),
            "a user's Cancel is an ANSWER; re-arming the request re-asks it, which is the loop"
        );
        assert!(PickerOpenOutcome::Opened.request_discharged());
        assert!(
            !PickerOpenOutcome::NotOpened.request_discharged(),
            "a deferred MenuJob submit is the ONE case that must still retry"
        );
    }

    /// The in-game arms cannot express a dismissal, and that is not an oversight: backing out of a
    /// live `05_010` browser is a later event with its own path, so their `bool` only ever means
    /// "a window is up" or "the submit did not happen". Pinning it here keeps the mapping from
    /// drifting into the OS surface's three-state meaning.
    #[test]
    fn the_in_game_arms_map_only_to_opened_or_not_opened() {
        assert_eq!(in_game_open_outcome(true), PickerOpenOutcome::Opened);
        assert_eq!(in_game_open_outcome(false), PickerOpenOutcome::NotOpened);
        assert!(!in_game_open_outcome(false).request_discharged());
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
