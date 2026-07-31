// THE one place that decides WHICH picker opens.
//
// There are exactly three "open a picker" entry points -- the System>Quit "Load Character from File"
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
///
/// THREE HERE, FOUR ONE LAYER DOWN, AND THE COLLAPSE IS DELIBERATE. `os_pick_validated` reports
/// FOUR states, because it splits `Dismissed` into a user's Cancel and an unusable dialog
/// (`OsPickAbort::Cancelled` vs `OsPickAbort::Failed`). No consumer of THIS type discriminates
/// them: every System>Quit caller asks exactly one question -- may I re-arm the request? -- and the
/// answer is "no" for both, because both mean a dialog ran and came back with nothing. The ONE
/// caller for which the difference decides whether a user's game is terminated is the missing-save
/// boot arm, and it consumes `os_pick_validated` directly rather than through this router. Adding a
/// fourth variant here would only manufacture a state no arm can produce and no caller would read.
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

/// Lift the `bool` an arm returns when all it can report is "did a surface TAKE THIS OVER".
///
/// Four of the router's six arms are like that, and for the same structural reason: they hand the
/// request to a surface and return before the user answers, so a dismissal is not theirs to report.
///
///  * the two IN-GAME System>Quit arms answer a strictly smaller question -- a window is up, or the
///    submit did not happen and the caller may try again. Backing OUT of a live `05_010` browser is
///    a LATER event with its own path (`save_picker_reset`), never this return value, which is
///    exactly why the in-game surface never looped and the OS surface did.
///  * the two BOOT arms return "this open was TAKEN OVER by a surface". A `false` there is not a
///    failure and not a cancel -- it is "nothing owns the pick yet, ask again on the next tick" --
///    which is what lets the OS boot arm wait for the core `CreateFileW` detour to go live without
///    either spinning a dialog or stranding the boot.
///
/// So `true` is `Opened` and `false` is `NotOpened`, and NEITHER can be `Dismissed`: re-arming on a
/// `false` here is the deferred-submit retry both surfaces need, not the reopen loop.
fn open_taken_over_outcome(taken: bool) -> PickerOpenOutcome {
    if taken {
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
///
/// ONLY THE TWO OS System>Quit ARMS CAN SAY `Dismissed`, because they are the only arms that are
/// still on the stack when the user answers: they block inside comdlg32 and return afterwards. The
/// other four hand the request to a surface and return immediately, so all they can report is
/// `Opened` / `NotOpened` -- see [`open_taken_over_outcome`].
///
/// A BOOT CANCEL THEREFORE NEVER COMES BACK THROUGH HERE, and that is the point rather than a gap.
/// The boot OS arm runs its dialog on a thread of its own; by the time this function returns, the
/// dialog has not been answered (usually not even opened), and the cancel is handled where it lands
/// -- `save_picker_boot.rs` quits the game from that thread. Per-intent cancel semantics live in
/// each intent's own arm, discriminated by an enum variant the compiler checks, and never by a flag
/// read inside shared picker code.
pub(crate) unsafe fn open_picker_for_intent(request: PickerOpenRequest) -> PickerOpenOutcome {
    let surface = picker_surface_for(os_native_picker_active());
    match (surface, request) {
        (PickerSurface::InGame, PickerOpenRequest::LoadSource { action_obj }) => {
            open_taken_over_outcome(unsafe { system_quit_open_save_picker_menu_in_game(action_obj) })
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

/// Where a save-DESTINATION browser opens, and what it needs to know about the save already
/// loaded. Resolved once per open by [`save_dest_start_dir`].
pub(crate) struct SaveDestOrigin {
    /// Folder the browser opens in.
    pub(crate) start_dir: PathBuf,
    /// Leaf the `[ new ]` row (and the OS Save-As filename field) is pre-filled with -- always the
    /// loaded save's own filename, so a destination keeps its save flavor.
    pub(crate) loaded_file_name: String,
    /// Full path of the save currently loaded, for the `[CURRENT]` row marker.
    ///
    /// DISPLAY ONLY. Marking a row is a string comparison; deciding that a chosen destination IS
    /// the loaded save is a FILESYSTEM-IDENTITY check (volume serial + file index, runtime-proven
    /// 2026-07-29) performed at commit time in `save_dest_commit.rs`. Two paths that name one file
    /// through different mounts, cases or links must never be told apart by this field, and the
    /// commit never asks it.
    pub(crate) loaded_path: PathBuf,
}

/// Where a save-DESTINATION browser starts, the leaf a new file there is given, and which file is
/// the loaded one. `None` (with a logged reason) when the loaded save cannot be resolved or no
/// readable folder exists.
///
/// BOTH surfaces call this, so they cannot drift. Contract 8 read per surface: a destination starts
/// at the LOADED SAVE'S OWN folder, deliberately NOT at the remembered `preferred_save_picker_dir`
/// -- "save next to the save you loaded" is the expected default and the remembered dir belongs to
/// the LOAD flow, which is where `save_picker_start_dir` consults it. If that reading is ever
/// changed, it changes here, for both modes at once.
///
/// STARTING THERE IS NOW LOAD-BEARING, not just convenient. The Save Game row opens this browser
/// with no question in front of it, so the first screen has to be the screen where "save over what
/// I had" is one press away: the loaded save's own folder, with the loaded save in the list and
/// `[ new ]` resolving to its own leaf.
fn save_dest_start_dir() -> Option<SaveDestOrigin> {
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

/// What a chosen destination becomes. Mode-free on purpose: a `[ new ]` row, a picked existing file
/// in the in-game browser and an OS Save-As all route through the same decision, so the overwrite
/// gate cannot differ between surfaces.
///
/// This is also the ONLY gate left in the Save Game flow. The two up-front confirms it used to sit
/// behind are gone, so "does this destination already exist" is now the entire question of whether
/// the user is asked anything at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DestRoute {
    /// The file is already there: the overwrite confirm decides, and it is the SINGLE overwrite
    /// gate in either surface -- which is why the OS Save-As deliberately does not set
    /// `OFN_OVERWRITEPROMPT` (verified 2026-07-31; its absence is load-bearing, see
    /// `save_picker_os_dialog.rs`). Setting it would ask the same question twice, once in a
    /// Windows dialog whose answer we do not receive and once in ours, which is the one that
    /// decides.
    ConfirmOverwrite,
    /// A name nobody is using: stage the commit. No question, because nothing is at risk.
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

    /// The `bool`-returning arms cannot express a dismissal, and that is not an oversight. The
    /// in-game ones return before the user answers, because backing out of a live `05_010` browser
    /// is a later event with its own path; the BOOT ones return before the user answers because
    /// their `bool` says only whether a surface has taken the pick over. So `false` must stay a
    /// retry on both, and neither may drift into the OS surface's terminal meaning -- a boot arm
    /// that reported `Dismissed` would strand a missing-save boot with no picker and no way out.
    #[test]
    fn the_bool_returning_arms_map_only_to_opened_or_not_opened() {
        assert_eq!(open_taken_over_outcome(true), PickerOpenOutcome::Opened);
        assert_eq!(open_taken_over_outcome(false), PickerOpenOutcome::NotOpened);
        assert!(open_taken_over_outcome(true).request_discharged());
        assert!(
            !open_taken_over_outcome(false).request_discharged(),
            "'no surface owns the pick yet' must stay askable on the next tick, on every arm that \
             reports it -- the in-game deferred submit AND the boot arm waiting for the core \
             CreateFileW detour"
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
