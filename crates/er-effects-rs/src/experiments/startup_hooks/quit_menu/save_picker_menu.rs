use super::*;

// In-game save-file picker rendered through native `05_010_ProfileSelect` rows. Directory and drive
// navigation reserve exact owner-zero authority before staging a fresh owner. Row activation is
// intercepted at `CS::ProfileLoadDialog` vtable slot 20 (`0x9a4670`); cursor/back remain native.

/// Action object of the "Load Character from File" row; `system_quit_open_profile_load_dialog` derives
/// the System dialog (action+0x8), submit queue and window list from it on every (re)submit.
pub(crate) use er_telemetry::counters::SAVE_PICKER_ACTION_OBJ;
pub(crate) use er_telemetry::counters::SAVE_PICKER_CANCEL_COUNT;
/// 1 while the live picker is the save-DESTINATION chooser (save-game-flow WP3) instead of the
/// load-source browser: `[ new ]` is the initial selection (row 1 when drives occupy row 0), and
/// activation feeds the save flow.
pub(crate) use er_telemetry::counters::SAVE_PICKER_DEST_MODE;
/// 1 while the live `05_010_ProfileSelect` window is OUR file-picker (rows = directory listing).
/// 0 when it is the normal character-slot view.
pub(crate) use er_telemetry::counters::SAVE_PICKER_MODE_ACTIVE;
/// Diagnostics / telemetry oracles.
pub(crate) use er_telemetry::counters::SAVE_PICKER_OPEN_COUNT;
/// 1 = a file was ingested from the picker; the menu-pump Run hook must resubmit `05_010` as the
/// NORMAL slot view (picker mode already cleared) so the user picks a character slot next.
pub(crate) use er_telemetry::counters::SAVE_PICKER_OPEN_SLOTS_PENDING;
/// 1 while a modal OS file dialog is blocking the menu pump. Freeze predicate, re-entrancy claim
/// and stage-3 liveness term, all one word.
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_DIALOG_OPEN;
/// Game-task ticks whose save-flow deadline accrual was suppressed while a dialog was open.
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_TICKS_FROZEN;
pub(crate) use er_telemetry::counters::SAVE_PICKER_PICK_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_PICK_REJECT_COUNT;
/// Exact live owner whose latest picker model must be presented by a fresh native 05_010 window.
pub(crate) use er_telemetry::counters::SAVE_PICKER_REFRESH_PENDING_DIALOG;
/// 1 = the picker window is closing for a model/presentation change; the menu-pump Run hook must
/// stage only after owner-zero and then submit a fresh `05_010` instead of restoring the System UI.
pub(crate) use er_telemetry::counters::SAVE_PICKER_REOPEN_PENDING;
pub(crate) use er_telemetry::counters::SAVE_PICKER_REPOPULATE_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_RESUBMIT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_STAGED_ROW_COUNT;
pub(crate) static SAVE_PICKER_LAYOUT_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
const PICKED_FILE_CLOSE_IDLE: usize = 0;
const PICKED_FILE_CLOSE_ARMING: usize = 1;
const PICKED_FILE_CLOSE_DEFERRED: usize = 2;
const PICKED_FILE_CLOSE_FAILED: usize = 3;
/// Picked-file ingestion succeeded and slot-view resubmit is armed, but picker mode/model may only
/// clear after the owned native close sink runs or the exact ticket is safely cancelled.
static SAVE_PICKER_PICKED_FILE_CLOSE_STATE: AtomicUsize = AtomicUsize::new(PICKED_FILE_CLOSE_IDLE);
/// Which picker surface this session runs (0 = this in-game browser, 1 = the OS file dialog).
/// Latched once in `init_runtime_config`; exported as `oracle_save_picker_surface`.
pub(crate) use er_telemetry::counters::SAVE_PICKER_SURFACE;
/// System/Quit dialog the live picker window was submitted from; the menu-pump resubmit reopens
/// through it (the destination picker is opened by the save flow, which has no row action object).
/// Do not use this as the live `05_010_ProfileSelect` dialog: cursor/rebuild work uses
/// `SYSTEM_QUIT_PROFILE_SELECT_WINDOW`, which is populated from the `05_010` MenuWindowJob owner.
pub(crate) use er_telemetry::counters::SAVE_PICKER_SYSTEM_DIALOG;

/// Windows-form (`Z:\...`) string for a possibly Linux-form absolute path; drive-prefixed paths
/// pass through with separators normalized. String twin of `system_quit_path_for_windows`.
pub(crate) fn save_picker_windows_path_string(path: &str) -> String {
    let mut win = if path.starts_with('/') {
        format!("Z:{}", path.replace('/', "\\"))
    } else {
        path.replace('/', "\\")
    };
    while win.ends_with('\\') && win.len() > 3 {
        win.pop();
    }
    win
}

/// Starting directory for the picker: last picked dir (session, then er-effects.toml) when it
/// still exists, else the active save's directory, else the default save root.
pub(crate) fn save_picker_start_dir() -> Option<PathBuf> {
    if let Some(preferred) = crate::config::preferred_save_picker_dir_now() {
        if let Some(text) = preferred.to_str() {
            let windows = PathBuf::from(save_picker_windows_path_string(text));
            if windows.is_dir() {
                return Some(windows);
            }
        }
    }
    if let Ok(dir) = system_quit_env_save_dir() {
        let windows = PathBuf::from(save_picker_windows_path_string(&dir));
        if windows.is_dir() {
            return Some(windows);
        }
    }
    default_save_root()
        .and_then(|root| root.to_str().map(save_picker_windows_path_string))
        .map(PathBuf::from)
        .filter(|root| root.is_dir())
}

include!("save_picker_open_preflight.rs");
include!("save_picker_initial_open.rs");

/// Open the LOAD-source picker from the "Load Character from File" row action (menu thread). Which
/// surface that is -- this in-game browser or the OS file dialog -- is decided in one place,
/// [`open_picker_for_intent`]; the signature and the four call sites are unchanged.
pub(crate) unsafe fn system_quit_open_save_picker_menu(action_obj: usize) -> PickerOpenOutcome {
    unsafe { open_picker_for_intent(PickerOpenRequest::LoadSource { action_obj }) }
}

/// Open the IN-GAME file picker (menu thread). Mirrors the old OS-picker preflight (restore stale
/// preview, arm the active save snapshot), then stages the browse rows and submits the
/// `05_010_ProfileSelect` window.
pub(crate) unsafe fn system_quit_open_save_picker_menu_in_game(action_obj: usize) -> bool {
    let _open_guard = match unsafe { begin_picker_load_source_open_preflight(action_obj) } {
        PickerLoadSourceOpenPreflight::Initial(guard) => guard,
        PickerLoadSourceOpenPreflight::Coalesced(decision, _) => {
            append_autoload_debug(format_args!(
                "save-picker: duplicate load-source open coalesced before mutation action=0x{action_obj:x} decision={decision:?}"
            ));
            return true;
        }
        PickerLoadSourceOpenPreflight::Rejected(facts) => {
            // Name EVERY precondition. `classify_picker_load_source_open` folds six of them into one
            // `Rejected`, and twice now a wedge has been diagnosed by inference instead of evidence
            // because this line printed only the action pointer.
            append_autoload_debug(format_args!(
                "save-picker: load-source open REJECTED action=0x{action_obj:x} \
                 mode_active={} owner=0x{:x} owner_vtable=0x{:x} expected_vtable=0x{:x} \
                 live_owner_authorized={} owner_zero_pending={} activation_system=0x{:x} \
                 tracked_system=0x{:x} tracked_action=0x{:x} exact_parent_authority={} \
                 -- Initial needs !mode_active && owner==0 && !owner_zero_pending && tracked_system==0 && tracked_action==0 && exact_parent_authority",
                facts.mode_active,
                facts.profile_owner,
                facts.profile_vtable,
                facts.expected_profile_vtable,
                facts.live_owner_authorized,
                facts.owner_zero_resubmit_pending,
                facts.activation_system,
                facts.tracked_system,
                facts.tracked_action,
                facts.exact_parent_authority,
            ));
            return false;
        }
    };
    let save_path = match system_quit_env_save_path() {
        Ok(path) => path,
        Err(reason) => {
            SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!("save-picker: refused to open -- {reason}"));
            return false;
        }
    };
    unsafe { system_quit_save_swap_restore_profile_summary("save-picker-reopen") };
    clear_picker_presentation();
    let opened_dir = std::cell::RefCell::new(None::<String>);
    let opened = execute_picker_initial_open_sequence_with(
        || {
            system_quit_save_swap_arm_original_transaction(&save_path)
                .map(|arm| PickerInitialOpenAttemptGuard::new(arm, action_obj))
        },
        || {
            let start_dir = save_picker_start_dir();
            if start_dir.is_none() {
                append_autoload_debug(format_args!(
                    "save-picker: refused to open -- no readable start directory (preferred/save-dir/default-root all unavailable)"
                ));
            }
            start_dir
        },
        |start_dir| {
            *opened_dir.borrow_mut() = Some(start_dir.display().to_string());
            // Runtime-flavor extension filter: vanilla offers `.sl2`; Seamless offers both `.co2`
            // and `.sl2` so vanilla saves can be loaded/imported while ERSC owns the session.
            let seamless = save_picker_seamless_mode_after_settle("system-quit-picker-open");
            Some(if seamless {
                crate::experiments::save_picker::SavePickerModel::open_with_extensions(
                    &start_dir,
                    &["co2", "sl2"],
                )
            } else {
                crate::experiments::save_picker::SavePickerModel::open(&start_dir, "sl2")
            })
        },
        |model| unsafe { save_picker_stage_row_records(model) },
        |attempt, model| attempt.publish_model(model),
        || unsafe { system_quit_open_profile_load_dialog(action_obj) },
        |attempt| {
            let identity = attempt.commit();
            append_autoload_debug(format_args!(
                "save-picker: initial open transferred save-swap arm generation={} path='{}' to live preview ownership",
                identity.generation, identity.path
            ));
        },
    );
    if !opened {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        return false;
    }
    SAVE_PICKER_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-picker: opened in-game picker action=0x{action_obj:x} dir='{}' ext=.{}",
        opened_dir.borrow().as_deref().unwrap_or("<unknown>"),
        crate::experiments::save_picker::active_save_picker_lock()
            .as_ref()
            .map(|model| model.extension().to_owned())
            .unwrap_or_else(|| "<unset>".to_owned())
    ));
    true
}

/// Open the save-DESTINATION chooser for the Save Game flow (save-game-flow WP3). Menu-pump
/// owned: called from `system_quit_menu_window_run_post` after the tick stages
/// `SAVE_DEST_OPEN_PICKER_PENDING`. Which surface opens is decided in one place,
/// [`open_picker_for_intent`]; the signature and the call site are unchanged.
pub(crate) unsafe fn system_quit_open_save_dest_picker(system_dialog: usize) -> PickerOpenOutcome {
    unsafe { open_picker_for_intent(PickerOpenRequest::SaveDestination { system_dialog }) }
}

/// Open the IN-GAME `05_010` picker as the save-destination chooser -- the same submit context
/// the load picker's resubmit uses.
///
/// Differences from the load-source picker, all deliberate:
///   * start dir = the LOADED save's own directory, not the remembered preferred dir -- "save
///     next to the save you loaded" is the expected default and the remembered dir belongs to the
///     load flow. Since the Save Game row press opens this browser with nothing in front of it,
///     that folder is also the first thing the user sees, so it has to be the one where both
///     answers -- a fresh file, or the save they are playing -- are one press away;
///   * NO save-swap byte preview is armed: nothing foreign is previewed here, and the safety
///     snapshot of the live save is taken later, at the fire gate, by `save_dest_arm_redirect`;
///   * the model carries the loaded save's filename so the `[ new ]` row writes that leaf, and its
///     full path so that row is marked `[CURRENT]` in the listing.
pub(crate) unsafe fn system_quit_open_save_dest_picker_in_game(system_dialog: usize) -> bool {
    const HEAP_LO: usize = 0x10000;
    if system_dialog < HEAP_LO || system_dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        append_autoload_debug(format_args!(
            "save-dest-picker: refused to open -- System dialog=0x{system_dialog:x} is not heap-like"
        ));
        return false;
    }
    let Some(SaveDestOrigin {
        start_dir,
        loaded_file_name,
        loaded_path,
    }) = save_dest_start_dir()
    else {
        return false;
    };
    unsafe { system_quit_save_swap_restore_profile_summary("save-dest-picker-open") };
    clear_picker_presentation();
    // Same mode-locked filter as the load picker: the destination list shows the containers the
    // active runtime flavor understands.
    let seamless = save_picker_seamless_mode_after_settle("system-quit-save-dest-picker-open");
    let extensions: &[&str] = if seamless { &["co2", "sl2"] } else { &["sl2"] };
    let model = crate::experiments::save_picker::SavePickerModel::open_destination(
        &start_dir,
        extensions,
        &loaded_file_name,
        &loaded_path,
    );
    if !unsafe { save_picker_stage_row_records(&model) } {
        return false;
    }
    *crate::experiments::save_picker::active_save_picker_lock() = Some(model);
    SAVE_PICKER_MODE_ACTIVE.store(1, Ordering::SeqCst);
    SAVE_PICKER_DEST_MODE.store(1, Ordering::SeqCst);
    SAVE_PICKER_ACTION_OBJ.store(0, Ordering::SeqCst);
    let _ = save_picker_publish_system_dialog(system_dialog);
    save_picker_set_reopen_pending(0);
    clear_picker_refresh_request();
    clear_path_editor_return_reopen_request();
    clear_picker_pending_resubmit_transition();
    save_picker_set_open_slots_pending(0);
    if !unsafe { system_quit_open_profile_load_dialog_on(system_dialog) } {
        unsafe { system_quit_save_swap_restore_profile_summary("save-dest-picker-open-failed") };
        clear_picker_presentation();
        *crate::experiments::save_picker::active_save_picker_lock() = None;
        SAVE_PICKER_MODE_ACTIVE.store(0, Ordering::SeqCst);
        SAVE_PICKER_DEST_MODE.store(0, Ordering::SeqCst);
        let _ = save_picker_publish_system_dialog(0);
        append_autoload_debug(format_args!(
            "save-dest-picker: 05_010 submit FAILED for dialog=0x{system_dialog:x}"
        ));
        return false;
    }
    SAVE_DEST_PICKER_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_PICKER_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-dest-picker: opened destination browser dialog=0x{system_dialog:x} dir='{}' new_file='{loaded_file_name}' seamless={seamless}",
        start_dir.display()
    ));
    true
}

/// Atomic stage edge used by menu-thread destination-picker decisions.
///
/// The game task owns the save-flow stage machine, but a picked destination is delivered on the
/// menu thread. If the game task has already moved the flow out of the destination-browser stage
/// (for example a timeout/abort path), the picker must not resurrect it by blindly storing a new
/// stage. Compare-and-swap against the browser stage and reset ticks only on success.
pub(crate) fn save_flow_menu_stage_cas(
    stage_word: &std::sync::atomic::AtomicUsize,
    ticks_word: &std::sync::atomic::AtomicUsize,
    expected: usize,
    stage: usize,
) -> Result<usize, usize> {
    let previous =
        stage_word.compare_exchange(expected, stage, Ordering::SeqCst, Ordering::SeqCst)?;
    ticks_word.store(0, Ordering::SeqCst);
    Ok(previous)
}

/// Enter a save-flow stage from the menu thread only if the game task has not already left the
/// expected stage.
pub(crate) fn save_flow_menu_enter_stage(expected: usize, stage: usize, reason: &str) -> bool {
    match save_flow_menu_stage_cas(&SAVE_FLOW_STAGE, &SAVE_FLOW_STAGE_TICKS, expected, stage) {
        Ok(previous) => {
            append_autoload_debug(format_args!(
                "save-flow: menu stage {previous} -> {stage} ({reason})"
            ));
            true
        }
        Err(actual) => {
            append_autoload_debug(format_args!(
                "save-flow: menu stage transition REFUSED expected={expected} actual={actual} target={stage} ({reason}); the destination decision is stale and nothing will be written from it"
            ));
            false
        }
    }
}

#[cfg(test)]
#[path = "save_picker_initial_open_tests.rs"]
mod save_picker_initial_open_tests;
#[cfg(test)]
#[path = "save_picker_menu_stage_transition_tests.rs"]
mod save_picker_menu_stage_transition_tests;
#[cfg(test)]
#[path = "save_picker_open_preflight_tests.rs"]
mod save_picker_open_preflight_tests;
#[cfg(test)]
#[path = "save_swap_transaction_tests.rs"]
mod save_swap_transaction_tests;

/// Handle a destination-browser activation (menu thread, from `save_picker_handle_activation`).
/// `target` already exists -> the overwrite confirm; otherwise the commit is staged and the picker
/// closes so the save-flow tick can close the menus and fire.
///
/// THE ROUTE IS DECIDED BY THE TARGET, NOT BY WHICH ROW WAS PRESSED. `[ new ]` gets no exemption:
/// it resolves to the loaded save's own leaf in the browsed folder, and in the folder the browser
/// OPENS IN that leaf is the loaded save itself -- so pressing `[ new ]` there is an overwrite and
/// confirms like any other. The only rows that skip the question are the ones whose target does
/// not exist, where there is nothing to warn about.
pub(crate) unsafe fn save_dest_handle_picked_target(
    dialog: usize,
    target: PathBuf,
    source: &'static str,
) {
    match save_dest_route_picked_target(&target) {
        DestRoute::ConfirmOverwrite => {
            SAVE_DEST_TARGET_EXISTING_COUNT.fetch_add(1, Ordering::SeqCst);
            // NO CONFIRM MEANS NO OVERWRITE. On a build whose MessageBoxBuilder recipe failed its
            // prologue check the question cannot be asked, and the answer to "may I destroy this
            // file without asking" is no. The user stays in the browser and can still save to a
            // free name; the refusal is counted so a run can tell it from a decline.
            if !save_flow_box_recipe_available() {
                SAVE_DEST_OVERWRITE_UNCONFIRMABLE_COUNT.fetch_add(1, Ordering::SeqCst);
                save_picker_set_visible_status(er_save_picker::PickerStatusMessage::new(
                    "CANNOT CONFIRM OVERWRITE",
                    "This build cannot show the overwrite prompt; choose a new file instead.",
                ));
                append_autoload_debug(format_args!(
                    "save-dest: REFUSED to overwrite '{}' (source={source}) -- the overwrite confirm cannot be built on this build, and an unconfirmed overwrite is not something this flow performs. Staying in the destination list with visible reason; a new file name still saves",
                    target.display()
                ));
                return;
            }
            save_dest_set_target(target, source);
            // The confirm is hosted by the PICKER dialog (the game raises its own confirms over
            // 05_010 the same way), so it does not contend with the System dialog queue that owns
            // the picker window job. Submitted inline here (menu thread); a not-ready queue leaves
            // the pending latch for the next menu pump.
            save_flow_box_set_host_dialog(dialog);
            SAVE_FLOW_SUBMIT_BOX_PENDING.store(SAVE_FLOW_BOX_OVERWRITE_FILE, Ordering::SeqCst);
            if !save_flow_menu_enter_stage(
                SAVE_FLOW_STAGE_DEST_BROWSE,
                SAVE_FLOW_STAGE_OVERWRITE_CONFIRM,
                "picked existing destination -> overwrite confirm",
            ) {
                save_flow_box_clear();
                save_dest_clear_target("stale overwrite-confirm stage transition");
                return;
            }
            if unsafe { save_flow_submit_box(SAVE_FLOW_BOX_OVERWRITE_FILE) } {
                SAVE_FLOW_SUBMIT_BOX_PENDING.store(SAVE_FLOW_BOX_NONE, Ordering::SeqCst);
            }
        }
        DestRoute::CommitDirect => {
            SAVE_DEST_TARGET_NEW_COUNT.fetch_add(1, Ordering::SeqCst);
            save_dest_set_target(target, source);
            save_dest_stage_commit_and_close_picker(dialog, "new-file");
        }
    }
}

/// Stage the destination commit and close the browser for the save-flow tick.
pub(crate) unsafe fn save_dest_stage_commit_and_close_picker(dialog: usize, reason: &str) {
    if !save_flow_menu_enter_stage(
        SAVE_FLOW_STAGE_DEST_BROWSE,
        SAVE_FLOW_STAGE_DEST_BROWSE,
        "picked free destination -> commit",
    ) {
        SAVE_DEST_COMMIT_FAIL.fetch_add(1, Ordering::SeqCst);
        save_dest_clear_target("stale destination-commit stage transition");
        return;
    }
    SAVE_DEST_COMMIT_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_DEST_COMMIT_PENDING.store(1, Ordering::SeqCst);
    save_flow_box_clear();
    unsafe { save_picker_native_close(dialog, reason) };
    append_autoload_debug(format_args!(
        "save-dest: commit staged (reason={reason}) target='{}'; picker closing, the save-flow tick will close the menus and fire",
        save_dest_target()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unset>".to_owned())
    ));
}

/// Route one native-accepted picker activation; never forward ProfileLoad behavior.
pub(crate) unsafe fn save_picker_handle_activation(
    dialog: usize,
    cursor: i32,
    provenance: er_save_picker::DriveStripActivationProvenance,
) -> er_save_picker::PickerSourceDecision {
    use crate::experiments::save_picker::PickerActivation;
    let model_row = save_picker_model_row_from_native_cursor(cursor);
    let decision = {
        let mut guard = crate::experiments::save_picker::active_save_picker_lock();
        er_save_picker::route_picker_source_activation(
            true,
            true,
            guard.as_mut(),
            model_row,
            provenance,
        )
    };
    let reported = decision.clone();
    let activation = match decision {
        er_save_picker::PickerSourceDecision::ForwardNative => {
            SAVE_PICKER_ACTIVATION_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
            SAVE_PICKER_NAMED_REJECTIONS.fetch_add(1, Ordering::SeqCst);
            return er_save_picker::PickerSourceDecision::Rejected(
                er_save_picker::PickerSourceRejection::UnknownSource,
            );
        }
        er_save_picker::PickerSourceDecision::Rejected(reason) => {
            SAVE_PICKER_NAMED_REJECTIONS.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-activation: named rejection dialog=0x{dialog:x} cursor={cursor} model_row={model_row:?} source={provenance:?} reason={reason:?}"
            ));
            return reported;
        }
        er_save_picker::PickerSourceDecision::Effect(
            er_save_picker::PickerNativeActivationEffect::Model(activation),
        ) => {
            SAVE_PICKER_ORDINARY_EFFECTS.fetch_add(1, Ordering::SeqCst);
            activation
        }
        er_save_picker::PickerSourceDecision::Effect(
            er_save_picker::PickerNativeActivationEffect::RequestPathEditor,
        ) => {
            SAVE_PICKER_PATH_EDITOR_REQUESTS.fetch_add(1, Ordering::SeqCst);
            save_picker_request_path_editor(dialog);
            append_autoload_debug(format_args!(
                "save-picker-path: scoped activation requested editor dialog=0x{dialog:x} native_cursor={cursor} model_row={model_row:?}"
            ));
            SAVE_PICKER_COMMITTED_EFFECTS.fetch_add(1, Ordering::SeqCst);
            return reported;
        }
        er_save_picker::PickerSourceDecision::Effect(
            er_save_picker::PickerNativeActivationEffect::DriveSelected(cell),
        ) => {
            SAVE_PICKER_DRIVE_SELECTIONS.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker: scoped drive activation selected cell={cell} native_cursor={cursor} model_row={model_row:?}"
            ));
            PickerActivation::Repopulate
        }
        er_save_picker::PickerSourceDecision::Effect(
            er_save_picker::PickerNativeActivationEffect::Ignored,
        ) => {
            SAVE_PICKER_ACTIVATION_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
            SAVE_PICKER_NAMED_REJECTIONS.fetch_add(1, Ordering::SeqCst);
            return er_save_picker::PickerSourceDecision::Rejected(
                er_save_picker::PickerSourceRejection::ModelIgnored,
            );
        }
    };
    match activation {
        PickerActivation::Repopulate => {
            if save_picker_schedule_refresh_request(dialog, "activation-repopulate") {
                SAVE_PICKER_REPOPULATE_COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }
        PickerActivation::PickedFile(path) if SAVE_PICKER_DEST_MODE.load(Ordering::SeqCst) != 0 => {
            unsafe { save_dest_handle_picked_target(dialog, path, "picked-file") };
        }
        PickerActivation::PickedNewFile(path) => {
            unsafe { save_dest_handle_picked_target(dialog, path, "new-row") };
        }
        PickerActivation::PickedFile(path) => {
            let Some(path_str) = path.to_str() else {
                SAVE_PICKER_PICK_REJECT_COUNT.fetch_add(1, Ordering::SeqCst);
                save_picker_set_visible_status(
                    er_save_picker::PickRejection::PathNotUtf8.status_message("SL2"),
                );
                SAVE_PICKER_NAMED_REJECTIONS.fetch_add(1, Ordering::SeqCst);
                return er_save_picker::PickerSourceDecision::Rejected(
                    er_save_picker::PickerSourceRejection::ModelIgnored,
                );
            };
            if unsafe { system_quit_ingest_picked_save(path_str) } {
                SAVE_PICKER_PICK_COUNT.fetch_add(1, Ordering::SeqCst);
                let route = er_save_picker::run_picked_file_close_route(
                    || {
                        save_picker_set_open_slots_pending(1);
                        SAVE_PICKER_PICKED_FILE_CLOSE_STATE
                            .store(PICKED_FILE_CLOSE_ARMING, Ordering::SeqCst);
                    },
                    || unsafe { save_picker_native_close(dialog, "picked-file") },
                    save_picker_finish_picked_file_close,
                );
                if route == er_save_picker::PickedFileCloseRoute::Deferred {
                    if save_picker_path_editor_deferred_close_pending() {
                        SAVE_PICKER_PICKED_FILE_CLOSE_STATE
                            .store(PICKED_FILE_CLOSE_DEFERRED, Ordering::SeqCst);
                    } else {
                        // No owned ticket exists to finish this transition. Keep picker mode/model
                        // live and disarm slot resubmit rather than opening D2 over D1.
                        SAVE_PICKER_PICKED_FILE_CLOSE_STATE
                            .store(PICKED_FILE_CLOSE_IDLE, Ordering::SeqCst);
                        save_picker_set_open_slots_pending(0);
                    }
                }
            } else {
                SAVE_PICKER_PICK_REJECT_COUNT.fetch_add(1, Ordering::SeqCst);
                SAVE_PICKER_NAMED_REJECTIONS.fetch_add(1, Ordering::SeqCst);
                return er_save_picker::PickerSourceDecision::Rejected(
                    er_save_picker::PickerSourceRejection::ModelIgnored,
                );
            }
        }
        PickerActivation::Ignored => {
            SAVE_PICKER_ACTIVATION_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
            SAVE_PICKER_NAMED_REJECTIONS.fetch_add(1, Ordering::SeqCst);
            return er_save_picker::PickerSourceDecision::Rejected(
                er_save_picker::PickerSourceRejection::ModelIgnored,
            );
        }
    }
    SAVE_PICKER_COMMITTED_EFFECTS.fetch_add(1, Ordering::SeqCst);
    reported
}

fn save_picker_finish_picked_file_close() {
    *crate::experiments::save_picker::active_save_picker_lock() = None;
    SAVE_PICKER_MODE_ACTIVE.store(0, Ordering::SeqCst);
    SAVE_PICKER_PICKED_FILE_CLOSE_STATE.store(PICKED_FILE_CLOSE_IDLE, Ordering::SeqCst);
}

include!("save_picker_native_owner.rs");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickerRefreshCloseResolution {
    Retained,
    RetiredClearReopen,
    NotOwned,
}

fn apply_picker_refresh_close_with(
    request: PickerRefreshRequest,
    close: PickerRefreshCloseDisposition,
    mut retire: impl FnMut(PickerRefreshRequest, bool) -> bool,
) -> PickerRefreshCloseResolution {
    match close {
        PickerRefreshCloseDisposition::Closed => PickerRefreshCloseResolution::Retained,
        PickerRefreshCloseDisposition::Deferred(_)
        | PickerRefreshCloseDisposition::ResetInProgress
        | PickerRefreshCloseDisposition::PreflightRejected => {
            PickerRefreshCloseResolution::Retained
        }
        PickerRefreshCloseDisposition::Rejected
        | PickerRefreshCloseDisposition::Cancelled(_)
        | PickerRefreshCloseDisposition::ResolveFailed => {
            if retire(request, false) {
                PickerRefreshCloseResolution::RetiredClearReopen
            } else {
                PickerRefreshCloseResolution::NotOwned
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickerRefreshRetryOutcome {
    Deferred,
    DrainedClosed { dialog: usize },
    DrainedFailed { dialog: usize },
    Cancelled { dialog: usize },
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickerRefreshRetryResolution {
    NotOwned,
    Retained,
    RetiredClearReopen,
}

fn apply_picker_refresh_retry_with(
    request: Option<PickerRefreshRequest>,
    outcome: PickerRefreshRetryOutcome,
    mut retire: impl FnMut(PickerRefreshRequest, bool) -> bool,
) -> PickerRefreshRetryResolution {
    let Some(request) = request else {
        return PickerRefreshRetryResolution::NotOwned;
    };
    match outcome {
        PickerRefreshRetryOutcome::Deferred | PickerRefreshRetryOutcome::None => {
            PickerRefreshRetryResolution::Retained
        }
        PickerRefreshRetryOutcome::DrainedClosed { dialog } if dialog == request.dialog => {
            PickerRefreshRetryResolution::Retained
        }
        PickerRefreshRetryOutcome::DrainedFailed { dialog }
        | PickerRefreshRetryOutcome::Cancelled { dialog }
            if dialog == request.dialog =>
        {
            if retire(request, false) {
                PickerRefreshRetryResolution::RetiredClearReopen
            } else {
                PickerRefreshRetryResolution::NotOwned
            }
        }
        _ => PickerRefreshRetryResolution::NotOwned,
    }
}

pub(crate) unsafe fn save_picker_retry_deferred_native_close(
    observation: PickerProfileRunObservation,
) -> er_save_picker::PathEditorCloseRetryGate {
    let live_token = observation.live_token();
    if let Some(ticket) = save_picker_path_editor_deferred_close_ticket()
        && (!picker_deferred_close_token_allows(observation, ticket.dialog)
            || !observation
                .live_token()
                .is_some_and(save_picker_profile_token_still_current))
    {
        return er_save_picker::PathEditorCloseRetryGate::Deferred;
    }
    let refresh_request = load_picker_refresh_request();
    match save_picker_path_editor_retry_deferred_close(|dialog| {
        live_token.is_some_and(|token| {
            token.dialog == dialog
                && unsafe {
                    save_picker_invoke_native_close(token, "deferred-path-editor-lease").is_closed()
                }
        })
    }) {
        er_save_picker::PathEditorDeferredCloseDisposition::None => {
            let _ = apply_picker_refresh_retry_with(
                refresh_request,
                PickerRefreshRetryOutcome::None,
                retire_picker_refresh_request,
            );
            if save_picker_path_editor_reset_active() {
                return er_save_picker::PathEditorCloseRetryGate::Deferred;
            }
            match SAVE_PICKER_PICKED_FILE_CLOSE_STATE.load(Ordering::SeqCst) {
                PICKED_FILE_CLOSE_DEFERRED => {
                    // The exact ticket was cancelled by an owner-identity observation before retry.
                    // Cancellation is a safe close resolution: clear mode now, then abort this tick so
                    // D2 cannot open until a later no-ticket tick.
                    save_picker_finish_picked_file_close();
                    er_save_picker::PathEditorCloseRetryGate::Drained
                }
                PICKED_FILE_CLOSE_FAILED => er_save_picker::PathEditorCloseRetryGate::Deferred,
                _ => er_save_picker::PathEditorCloseRetryGate::NoTicket,
            }
        }
        er_save_picker::PathEditorDeferredCloseDisposition::Deferred(_) => {
            let _ = apply_picker_refresh_retry_with(
                refresh_request,
                PickerRefreshRetryOutcome::Deferred,
                retire_picker_refresh_request,
            );
            er_save_picker::PathEditorCloseRetryGate::Deferred
        }
        er_save_picker::PathEditorDeferredCloseDisposition::Drained { ticket, closed } => {
            if closed
                && let Some(request) =
                    refresh_request.filter(|request| request.dialog == ticket.dialog)
            {
                let _latch_guard = resubmit_latch_lock();
                if !any_resubmit_reserved() {
                    SAVE_PICKER_REFRESH_CLOSE_REQUESTED_GENERATION
                        .store(request.generation, Ordering::SeqCst);
                    let _ = arm_picker_pending_resubmit_transition(
                        request.dialog,
                        0,
                        request.generation,
                    );
                }
            }
            let refresh_resolution = apply_picker_refresh_retry_with(
                refresh_request,
                if closed {
                    PickerRefreshRetryOutcome::DrainedClosed {
                        dialog: ticket.dialog,
                    }
                } else {
                    PickerRefreshRetryOutcome::DrainedFailed {
                        dialog: ticket.dialog,
                    }
                },
                retire_picker_refresh_request,
            );
            if closed {
                SAVE_PICKER_PATH_EDITOR_DEFERRED_CLOSE_DRAINS.fetch_add(1, Ordering::SeqCst);
                if SAVE_PICKER_PICKED_FILE_CLOSE_STATE.load(Ordering::SeqCst)
                    == PICKED_FILE_CLOSE_DEFERRED
                {
                    save_picker_finish_picked_file_close();
                }
            } else if SAVE_PICKER_PICKED_FILE_CLOSE_STATE.load(Ordering::SeqCst)
                == PICKED_FILE_CLOSE_DEFERRED
            {
                SAVE_PICKER_PICKED_FILE_CLOSE_STATE
                    .store(PICKED_FILE_CLOSE_FAILED, Ordering::SeqCst);
            }
            append_autoload_debug(format_args!(
                "save-picker: owned deferred close drained dialog=0x{:x} generation={} closed={closed} refresh_resolution={refresh_resolution:?}",
                ticket.dialog, ticket.generation
            ));
            if closed || refresh_resolution == PickerRefreshRetryResolution::RetiredClearReopen {
                er_save_picker::PathEditorCloseRetryGate::Drained
            } else {
                er_save_picker::PathEditorCloseRetryGate::Deferred
            }
        }
        er_save_picker::PathEditorDeferredCloseDisposition::Cancelled(ticket) => {
            SAVE_PICKER_PATH_EDITOR_DEFERRED_CLOSE_CANCELS.fetch_add(1, Ordering::SeqCst);
            let refresh_resolution = apply_picker_refresh_retry_with(
                refresh_request,
                PickerRefreshRetryOutcome::Cancelled {
                    dialog: ticket.dialog,
                },
                retire_picker_refresh_request,
            );
            if SAVE_PICKER_PICKED_FILE_CLOSE_STATE.load(Ordering::SeqCst)
                == PICKED_FILE_CLOSE_DEFERRED
            {
                save_picker_finish_picked_file_close();
            }
            append_autoload_debug(format_args!(
                "save-picker: stale deferred close cancelled without native dereference dialog=0x{:x} generation={} refresh_resolution={refresh_resolution:?}",
                ticket.dialog, ticket.generation
            ));
            er_save_picker::PathEditorCloseRetryGate::Drained
        }
    }
}

fn picker_resubmit_pending_with(refresh_reopen: usize, open_slots: usize) -> bool {
    refresh_reopen != 0 || open_slots != 0
}

/// True while a picker-driven close must NOT run the normal restore path (a resubmit is queued).
pub(crate) fn save_picker_resubmit_pending() -> bool {
    picker_resubmit_pending_with(
        SAVE_PICKER_REOPEN_PENDING.load(Ordering::SeqCst),
        SAVE_PICKER_OPEN_SLOTS_PENDING.load(Ordering::SeqCst),
    )
}

const PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET: usize = 0xa38;
const GRID_CONTROL_SCROLLBAR_OFFSET: usize = 0x1a8;
const PROFILE_LOAD_DIALOG_SCROLLBAR_OFFSET: usize =
    PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET + GRID_CONTROL_SCROLLBAR_OFFSET;
const MENU_ITEM_LIST_CURSOR_GETTER_RVA: usize = 0x739e20;
/// Native GridControl cursor router (`FUN_140738d40`, ER 1.16.2). Static proof: it calls the
/// validated index setter `FUN_14073bc10`, refreshes the Cursor display through `FUN_1407396f0`,
/// and emits the old/new selection callback through `FUN_14073b3f0`. This is the same route used by
/// ProfileLoadDialog and other native list owners; do not write `grid+0xd4` directly.
const MENU_ITEM_LIST_CURSOR_SETTER_RVA: usize = 0x738d40;
const SCROLLBAR_CONTROL_SET_TOTAL_RVA: u32 = 0x74dad0;
const SCROLLBAR_CONTROL_SET_POSITION_RVA: u32 = 0x74db60;
/// `ScrollBarV` begins with `CSMenuVisibleComponent`; its `SceneObjProxy` starts at +8.
/// `FUN_140733340` loads `*(scrollbar+8)` and calls slot +8 from that table.
const SCROLLBAR_VISIBLE_PROXY_OFFSET: usize = 0x08;
const SCROLLBAR_VISIBLE_PROXY_DISPATCH_SLOT: usize = 0x08;
static SAVE_PICKER_SCROLLBAR_LAST_SYNC: AtomicUsize = AtomicUsize::new(usize::MAX);
/// `FUN_140757af0` returns a client-local point, not movie-stage or OS-screen coordinates.
/// 1.16.2 static proof: it calls `FUN_14075d6e0`, which reads the native event payload's integer
/// x/y and subtracts `g_GxDrawContext->field_0x128->{0x110,0x114}` (the render-window/client
/// origin), then converts those client pixels to floats. Its menu callers feed that result to
/// client-space hit-test helpers. Runtime proof agrees: raw `(1808,639)` in the validated
/// 3840x2160 ER client maps to movie stage `(-56.0,-220.5)`, matching the independently converted
/// live pointer `(-56.3,-220.2)`; interpreting the raw pair as movie stage cannot hit any control.
const MENU_VIEWER_EVENT_POINT_RVA: usize = 0x757af0;
const PROFILE_SELECT_MOVIE_WIDTH_PX: f32 = 1920.0;
const PROFILE_SELECT_MOVIE_HEIGHT_PX: f32 = 1080.0;
/// `ProfileList` is placed at root movie x=960 and every nested ItemList/row placement has identity
/// x; `save_picker_client_point_to_movie_stage` subtracts that same half-width. Drive/path row-local
/// x is therefore already the mouse stage x. Hit geometry comes from the shipped editor schema, the
/// same source the generator uses for text and button chrome.
fn drive_strip_hit_geometry() -> (f32, f32, f32) {
    let layout = er_gfx::profile_05_010_layout::shipped();
    let first = layout.field(er_gfx::title_05_010::DRIVE_CELL_FIELD_NAMES[0]);
    let second = layout.field(er_gfx::title_05_010::DRIVE_CELL_FIELD_NAMES[1]);
    (
        first.x + layout.row_chrome.drive_button.x,
        second.x - first.x,
        first.width as f32,
    )
}
const _: () =
    assert!(er_save_picker::DRIVE_STRIP_MAX_CELLS <= er_gfx::title_05_010::DRIVE_CELL_CAPACITY);
/// Live `05_010_ProfileSelect` cursor values are already staged model-row indices. The old +2
/// observation came from reading the parent System/Quit dialog, not the live ProfileSelect dialog.
const PROFILE_SELECT_NATIVE_ROW_MODEL_OFFSET: i32 = 0;
const SAVE_PICKER_DRIVE_STRIP_LEFT_MASK: usize = 1 << 0;
const SAVE_PICKER_DRIVE_STRIP_RIGHT_MASK: usize = 1 << 1;
const SAVE_PICKER_ACTIVATION_RING_CAPACITY: usize = 64;
pub(crate) type ProfileLoadMenuWindowUpdateFn = unsafe extern "system" fn(usize, f32, *const u8);
pub(crate) type ProfileLoadActivateFn = unsafe extern "system" fn(usize);
pub(crate) type PickerActivationEffectSink = unsafe fn(
    usize,
    i32,
    er_save_picker::DriveStripActivationProvenance,
) -> er_save_picker::PickerSourceDecision;
pub(crate) type PickerActivationTelemetrySink = fn(PickerActivationContext);
static SAVE_PICKER_ACTIVATION_SEQ: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Default)]
struct PickerPhysicalClickDiagnostic {
    raw_event_client: Option<(f32, f32)>,
    event_stage: Option<(f32, f32)>,
    live_stage: Option<(f32, f32)>,
    event_target: Option<er_save_picker::DriveStripFocus>,
    live_target: Option<er_save_picker::DriveStripFocus>,
}

#[derive(Clone, Copy, Debug)]
struct PickerPhysicalActivationClassification {
    provenance: er_save_picker::DriveStripActivationProvenance,
    diagnostic: Option<PickerPhysicalClickDiagnostic>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PickerActivationContext {
    seq: u64,
    source: &'static str,
    dialog: usize,
    row_input_gate: usize,
    cursor: i32,
    model_row: Option<usize>,
    layout_generation: u64,
    layout_hash: u64,
    provenance: er_save_picker::DriveStripActivationProvenance,
    physical_click: Option<PickerPhysicalClickDiagnostic>,
    callback_count: usize,
    route_count: usize,
    effect_count: usize,
    update_forward_count: usize,
    profile_load_original_count: usize,
    terminal_count: usize,
    route: &'static str,
    effect: &'static str,
    terminal: &'static str,
}

const PICKER_LIFECYCLE_CONTEXT_MISSING: u32 = 1 << 0;
const PICKER_LIFECYCLE_CALLBACK_DUPLICATED: u32 = 1 << 1;
const PICKER_LIFECYCLE_EFFECT_INVALID: u32 = 1 << 2;
const PICKER_LIFECYCLE_NATIVE_ORIGINAL_IN_PICKER: u32 = 1 << 3;
const PICKER_LIFECYCLE_NO_CALLBACK_TERMINAL_MISSING: u32 = 1 << 4;
const PICKER_LIFECYCLE_LATE_LABEL_INVALID: u32 = 1 << 5;
const PICKER_LIFECYCLE_FOREIGN_SUPPRESSED: u32 = 1 << 6;
const PICKER_LIFECYCLE_TELEMETRY_INVALID: u32 = 1 << 7;
const PICKER_LIFECYCLE_ROUTE_INVALID: u32 = 1 << 8;

#[derive(Clone, Copy, Debug)]
struct PickerLifecycleInvariantObservation {
    identity_exact: bool,
    context_present_at_callback: bool,
    telemetry_count: usize,
    context: PickerActivationContext,
}

/// Production lifecycle invariant validator. The adapter calls this immediately before every
/// telemetry delivery; mutation tests exercise this same function rather than a source-only copy.
fn validate_picker_lifecycle_invariants(observation: PickerLifecycleInvariantObservation) -> u32 {
    let context = observation.context;
    let mut violations = 0;
    if context.callback_count != 0 && !observation.context_present_at_callback {
        violations |= PICKER_LIFECYCLE_CONTEXT_MISSING;
    }
    if context.callback_count > 1 {
        violations |= PICKER_LIFECYCLE_CALLBACK_DUPLICATED;
    }
    if context.effect_count > 1
        || ((context.terminal == "committed" || context.terminal == "route-committed")
            && context.effect_count != 1)
    {
        violations |= PICKER_LIFECYCLE_EFFECT_INVALID;
    }
    if observation.identity_exact && context.profile_load_original_count != 0 {
        violations |= PICKER_LIFECYCLE_NATIVE_ORIGINAL_IN_PICKER;
    }
    if !observation.identity_exact && context.profile_load_original_count == 0 {
        violations |= PICKER_LIFECYCLE_FOREIGN_SUPPRESSED;
    }
    let known_no_callback = context.callback_count == 0
        && context.provenance
            != er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation;
    if known_no_callback
        && (context.terminal_count != 1
            || context.terminal != "native-matcher-no-callback"
            || context.effect != "native-matcher-no-callback")
    {
        violations |= PICKER_LIFECYCLE_NO_CALLBACK_TERMINAL_MISSING;
    }
    if context.source == "profile-load-late"
        && (context.terminal_count != 1
            || context.terminal != "late"
            || context.effect != "late"
            || context.callback_count != 1
            || context.effect_count != 0)
    {
        violations |= PICKER_LIFECYCLE_LATE_LABEL_INVALID;
    }
    if observation.telemetry_count != 1 {
        violations |= PICKER_LIFECYCLE_TELEMETRY_INVALID;
    }
    if context.route_count != context.callback_count.max(1)
        || context.terminal_count != 1
        || context.update_forward_count > 1
    {
        violations |= PICKER_LIFECYCLE_ROUTE_INVALID;
    }
    violations
}

fn deliver_picker_activation_telemetry(
    context: PickerActivationContext,
    telemetry_sink: PickerActivationTelemetrySink,
) {
    let violations = validate_picker_lifecycle_invariants(PickerLifecycleInvariantObservation {
        identity_exact: true,
        context_present_at_callback: true,
        telemetry_count: 1,
        context,
    });
    if violations != 0 {
        SAVE_PICKER_ACTIVATION_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker-activation: lifecycle invariant violations=0x{violations:x} seq={}",
            context.seq
        ));
    }
    telemetry_sink(context);
}

#[derive(Clone, Copy, Debug)]
struct PickerActivationRingRecord {
    context: PickerActivationContext,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PickerDialogIdentity {
    pub(crate) picker_mode_active: bool,
    pub(crate) dialog: usize,
    pub(crate) active_dialog: usize,
    pub(crate) expected_vtable: Option<usize>,
    pub(crate) actual_vtable: Option<usize>,
}

impl PickerDialogIdentity {
    pub(crate) fn is_exact_active_picker(self) -> bool {
        self.picker_mode_active
            && self.dialog != 0
            && self.dialog == self.active_dialog
            && self.expected_vtable.is_some()
            && self.actual_vtable == self.expected_vtable
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PickerProfileLoadDispatch {
    OriginalForwarded,
    PickerSuppressed,
}

/// Production lifecycle seam for the two native functions that bracket one picker activation.
/// The ABI-sensitive originals and the observable effect/telemetry sinks are explicit inputs, so
/// tests execute the same context-before-update, synchronous-callback, and finalize-after-update
/// ordering as the installed hooks.
#[derive(Clone, Copy)]
pub(crate) struct PickerNativeLifecycleAdapter {
    pub(crate) update_original: Option<ProfileLoadMenuWindowUpdateFn>,
    pub(crate) profile_load_original: Option<ProfileLoadActivateFn>,
    pub(crate) effect_sink: PickerActivationEffectSink,
    pub(crate) telemetry_sink: PickerActivationTelemetrySink,
}

static SAVE_PICKER_ACTIVATION_RING: OnceLock<
    Mutex<std::collections::VecDeque<PickerActivationRingRecord>>,
> = OnceLock::new();

std::thread_local! {
    static SAVE_PICKER_SCOPED_ACTIVATION: std::cell::RefCell<Option<PickerActivationContext>> =
        const { std::cell::RefCell::new(None) };
}

struct PickerActivationScope {
    previous: Option<PickerActivationContext>,
    finished: bool,
}

impl PickerActivationScope {
    fn enter(context: PickerActivationContext) -> Self {
        let previous = SAVE_PICKER_SCOPED_ACTIVATION.with(|slot| slot.replace(Some(context)));
        Self {
            previous,
            finished: false,
        }
    }

    fn finish(mut self) -> Option<PickerActivationContext> {
        let current = SAVE_PICKER_SCOPED_ACTIVATION.with(|slot| slot.replace(self.previous.take()));
        self.finished = true;
        current
    }
}

impl Drop for PickerActivationScope {
    fn drop(&mut self) {
        if !self.finished {
            SAVE_PICKER_SCOPED_ACTIVATION.with(|slot| {
                slot.replace(self.previous.take());
            });
        }
    }
}

fn save_picker_layout_hash() -> u64 {
    let (first_left, pitch, width) = drive_strip_hit_geometry();
    [first_left.to_bits(), pitch.to_bits(), width.to_bits()]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        })
}

fn save_picker_push_activation_record(context: PickerActivationContext) {
    let ring =
        SAVE_PICKER_ACTIVATION_RING.get_or_init(|| Mutex::new(std::collections::VecDeque::new()));
    if let Ok(mut ring) = ring.lock() {
        if ring.len() == SAVE_PICKER_ACTIVATION_RING_CAPACITY {
            ring.pop_front();
        }
        ring.push_back(PickerActivationRingRecord { context });
    } else {
        SAVE_PICKER_ACTIVATION_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
    }
}

fn save_picker_point_json(point: Option<(f32, f32)>) -> String {
    point.map_or_else(|| "null".to_owned(), |(x, y)| format!("[{x:.3},{y:.3}]"))
}

fn save_picker_target_json(target: Option<er_save_picker::DriveStripFocus>) -> String {
    target.map_or_else(|| "null".to_owned(), |target| format!("\"{target:?}\""))
}

pub(crate) fn save_picker_activation_ring_json() -> String {
    let Some(ring) = SAVE_PICKER_ACTIVATION_RING.get() else {
        return "[]".to_owned();
    };
    let Ok(ring) = ring.lock() else {
        return "[]".to_owned();
    };
    let mut output = String::from("[");
    for (index, record) in ring.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        let c = record.context;
        let physical = c.physical_click.unwrap_or_default();
        output.push_str(&format!(
            "{{\"seq\":{},\"source\":\"{}\",\"dialog\":{},\"gate\":{},\"cursor\":{},\"model_row\":{},\"layout_generation\":{},\"layout_hash\":{},\"classification\":\"{:?}\",\"raw_event_client\":{},\"event_stage\":{},\"live_stage\":{},\"event_target\":{},\"live_target\":{},\"route\":\"{}\",\"effect\":\"{}\",\"update_forward\":{},\"profile_load_original\":{},\"callbacks\":{},\"terminal\":\"{}\"}}",
            c.seq,
            c.source,
            c.dialog,
            c.row_input_gate,
            c.cursor,
            c.model_row.map_or(-1, |row| row as isize),
            c.layout_generation,
            c.layout_hash,
            c.provenance,
            save_picker_point_json(physical.raw_event_client),
            save_picker_point_json(physical.event_stage),
            save_picker_point_json(physical.live_stage),
            save_picker_target_json(physical.event_target),
            save_picker_target_json(physical.live_target),
            c.route,
            c.effect,
            c.update_forward_count,
            c.profile_load_original_count,
            c.callback_count,
            c.terminal,
        ));
    }
    output.push(']');
    output
}

unsafe fn save_picker_event_client_point(row_input_gate: *const u8) -> Option<(f32, f32)> {
    if row_input_gate.is_null() {
        return None;
    }
    let Ok(base) = game_module_base() else {
        return None;
    };
    let point_fn: unsafe extern "system" fn(*const u8, *mut u64) -> *mut u64 =
        unsafe { std::mem::transmute(base + MENU_VIEWER_EVENT_POINT_RVA) };
    let mut packed = 0_u64;
    unsafe { point_fn(row_input_gate, &mut packed as *mut u64) };
    let x = f32::from_bits(packed as u32);
    let y = f32::from_bits((packed >> 32) as u32);
    (x.is_finite() && y.is_finite()).then_some((x, y))
}

/// Bind a native logical activation source to picker provenance. The physical resolver is called
/// only for the event-bound primary-pointer branch, so later pointer coordinates can accept or
/// reject a physical target but cannot turn keyboard/pad Accept into physical input.
fn save_picker_provenance_from_native_source(
    source: QuitInputKind,
    classify_physical: impl FnOnce() -> er_save_picker::DriveStripActivationProvenance,
) -> er_save_picker::DriveStripActivationProvenance {
    match source {
        QuitInputKind::Confirm => {
            er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept
        }
        QuitInputKind::MouseClick => classify_physical(),
        QuitInputKind::Unknown => {
            er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation
        }
    }
}

/// Production composition boundary for one native picker row dispatch. The hook supplies the two
/// statically verified event-bound predicates and the physical target classifier directly; global
/// button state is not an input to this seam. Accept keeps native precedence over PrimaryPointerPress,
/// and physical target classification runs only for the latter.
pub(crate) fn save_picker_compose_activation_provenance_with(
    row_input_gate: *const u8,
    accept_pressed: impl FnOnce(*const u8) -> bool,
    primary_pointer_pressed: impl FnOnce(*const u8) -> bool,
    classify_physical: impl FnOnce(*const u8) -> er_save_picker::DriveStripActivationProvenance,
) -> er_save_picker::DriveStripActivationProvenance {
    let source = system_quit_classify_activation_input_with(
        row_input_gate,
        accept_pressed,
        primary_pointer_pressed,
    );
    save_picker_provenance_from_native_source(source, || classify_physical(row_input_gate))
}

unsafe fn save_picker_physical_activation_provenance(
    dialog: usize,
    cursor: i32,
    row_input_gate: *const u8,
) -> PickerPhysicalActivationClassification {
    let rejected = |diagnostic| PickerPhysicalActivationClassification {
        provenance: er_save_picker::DriveStripActivationProvenance::RejectedPhysicalClick,
        diagnostic,
    };
    let Some(live_cursor) = (unsafe { save_picker_native_cursor_for_event(dialog) }) else {
        return rejected(None);
    };
    if live_cursor != cursor {
        append_autoload_debug(format_args!(
            "save-picker: physical activation rejected cursor disagreement source={cursor} live={live_cursor} dialog=0x{dialog:x}"
        ));
        return rejected(None);
    }
    let cursor = live_cursor;
    let Some(model_row) = save_picker_model_row_from_native_cursor(cursor) else {
        return rejected(None);
    };
    let (drive_row, cell_count, controls_visible) = {
        let guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_ref() else {
            return rejected(None);
        };
        (
            model.drive_row(),
            model.drive_strip_cell_count(),
            model.status_message().is_none(),
        )
    };
    if let Some(provenance) = er_save_picker::classify_picker_physical_row(model_row, drive_row) {
        return PickerPhysicalActivationClassification {
            provenance,
            diagnostic: None,
        };
    }
    let Some(drive_row) = drive_row else {
        return PickerPhysicalActivationClassification {
            provenance:
                er_save_picker::DriveStripActivationProvenance::OrdinaryRowPhysicalActivation,
            diagnostic: None,
        };
    };

    let mut diagnostic = PickerPhysicalClickDiagnostic::default();
    let Some((event_client_x, event_client_y)) =
        (unsafe { save_picker_event_client_point(row_input_gate) })
    else {
        return rejected(Some(diagnostic));
    };
    diagnostic.raw_event_client = Some((event_client_x, event_client_y));
    let Some(geometry) = (unsafe { save_picker_validated_game_window_geometry() }) else {
        return rejected(Some(diagnostic));
    };
    let Some((event_stage_x, event_stage_y)) = geometry
        .viewport
        .client_point_to_movie_stage(event_client_x, event_client_y)
    else {
        return rejected(Some(diagnostic));
    };
    diagnostic.event_stage = Some((event_stage_x, event_stage_y));
    let Some(live_pointer) = (unsafe { save_picker_validated_game_pointer_from(geometry) }) else {
        return rejected(Some(diagnostic));
    };
    diagnostic.live_stage = Some((live_pointer.stage_x, live_pointer.stage_y));

    let bounds = save_picker_drive_strip_pointer_bounds(drive_row);
    let event_target = er_save_picker::route_drive_strip_native_click(
        geometry.window,
        model_row,
        drive_row,
        controls_visible,
        cell_count,
        event_stage_x,
        event_stage_y,
        bounds,
    );
    let live_target = er_save_picker::route_drive_strip_native_click(
        live_pointer.window,
        model_row,
        drive_row,
        controls_visible,
        cell_count,
        live_pointer.stage_x,
        live_pointer.stage_y,
        bounds,
    );
    diagnostic.event_target = event_target;
    diagnostic.live_target = live_target;
    let Some(target) = er_save_picker::agree_drive_strip_click_targets(event_target, live_target)
    else {
        append_autoload_debug(format_args!(
            "save-picker: physical click rejected raw_event_client=({event_client_x:.1},{event_client_y:.1}) event_stage=({event_stage_x:.1},{event_stage_y:.1}) live_stage=({:.1},{:.1}) event_target={event_target:?} live_target={live_target:?} native_cursor={cursor} drive_row={drive_row} cells={cell_count}",
            live_pointer.stage_x, live_pointer.stage_y
        ));
        return rejected(Some(diagnostic));
    };
    append_autoload_debug(format_args!(
        "save-picker: native physical-click transaction resolved native_cursor={cursor} drive_row={drive_row} raw_event_client=({event_client_x:.1},{event_client_y:.1}) event_stage=({event_stage_x:.1},{event_stage_y:.1}) live_stage=({:.1},{:.1}) event_target={event_target:?} live_target={live_target:?} cells={cell_count} target={target:?}",
        live_pointer.stage_x, live_pointer.stage_y
    ));
    PickerPhysicalActivationClassification {
        provenance: er_save_picker::DriveStripActivationProvenance::AcceptedPhysicalClick(target),
        diagnostic: Some(diagnostic),
    }
}

fn save_picker_decision_labels(
    decision: &er_save_picker::PickerSourceDecision,
) -> (&'static str, &'static str, &'static str, usize) {
    match decision {
        er_save_picker::PickerSourceDecision::ForwardNative => {
            ("forward-native", "none", "forwarded", 0)
        }
        er_save_picker::PickerSourceDecision::Rejected(reason) => match reason {
            er_save_picker::PickerSourceRejection::MissingModel => {
                ("reject", "missing-model", "rejected", 0)
            }
            er_save_picker::PickerSourceRejection::InvalidModelRow => {
                ("reject", "invalid-row", "rejected", 0)
            }
            er_save_picker::PickerSourceRejection::UnknownSource => {
                ("reject", "unknown-source", "rejected", 0)
            }
            er_save_picker::PickerSourceRejection::RejectedPhysicalClick => {
                ("reject", "physical-rejected", "rejected", 0)
            }
            er_save_picker::PickerSourceRejection::CrossRowProvenance => {
                ("reject", "cross-row", "rejected", 0)
            }
            er_save_picker::PickerSourceRejection::StatusOwnedRow => {
                ("reject", "status-owned", "rejected", 0)
            }
            er_save_picker::PickerSourceRejection::DuplicateCallback => {
                ("reject", "duplicate", "duplicate", 0)
            }
            er_save_picker::PickerSourceRejection::LateCallback => ("reject", "late", "late", 0),
            er_save_picker::PickerSourceRejection::ModelIgnored => {
                ("reject", "model-ignored", "rejected", 0)
            }
        },
        er_save_picker::PickerSourceDecision::Effect(effect) => match effect {
            er_save_picker::PickerNativeActivationEffect::Model(_) => {
                ("ordinary", "model", "committed", 1)
            }
            er_save_picker::PickerNativeActivationEffect::DriveSelected(_) => {
                ("drive", "drive-selected", "committed", 1)
            }
            er_save_picker::PickerNativeActivationEffect::RequestPathEditor => {
                ("path-editor", "path-editor-requested", "route-committed", 1)
            }
            er_save_picker::PickerNativeActivationEffect::Ignored => {
                ("reject", "ignored", "rejected", 0)
            }
        },
    }
}

unsafe fn save_picker_route_scoped_profile_load_callback_with(
    dialog: usize,
    cursor: i32,
    effect_sink: PickerActivationEffectSink,
) -> bool {
    let context = SAVE_PICKER_SCOPED_ACTIVATION.with(|slot| {
        let mut slot = slot.borrow_mut();
        let context = slot.as_mut()?;
        if context.dialog != dialog {
            return None;
        }
        context.callback_count += 1;
        Some(*context)
    });
    let Some(context) = context else {
        return false;
    };
    SAVE_PICKER_SOURCE_ACCEPTED_EVENTS.fetch_add(1, Ordering::SeqCst);
    if context.callback_count != 1 {
        SAVE_PICKER_NAMED_REJECTIONS.fetch_add(1, Ordering::SeqCst);
        SAVE_PICKER_ACTIVATION_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
        SAVE_PICKER_SCOPED_ACTIVATION.with(|slot| {
            if let Some(current) = slot.borrow_mut().as_mut() {
                current.route_count += 1;
                current.route = "reject";
                current.effect = "duplicate";
                current.terminal = "duplicate-callback";
            }
        });
        return true;
    }
    let decision = unsafe { effect_sink(dialog, cursor, context.provenance) };
    let (route, effect, terminal, effect_count) = save_picker_decision_labels(&decision);
    SAVE_PICKER_SCOPED_ACTIVATION.with(|slot| {
        if let Some(current) = slot.borrow_mut().as_mut() {
            current.route_count += 1;
            current.effect_count += effect_count;
            current.terminal_count += 1;
            current.route = route;
            current.effect = effect;
            current.terminal = terminal;
        }
    });
    true
}

pub(crate) fn save_picker_commit_activation_context(context: PickerActivationContext) {
    SAVE_PICKER_ACTIVATION_TERMINALS.fetch_add(context.terminal_count, Ordering::SeqCst);
    save_picker_push_activation_record(context);
    append_autoload_debug(format_args!(
        "save-picker-activation: seq={} source={} class={:?} route={} effect={} update_forward={} profile_load_original={} terminal={} callbacks={} dialog=0x{:x} cursor={} model_row={:?} layout={}:0x{:016x}",
        context.seq,
        context.source,
        context.provenance,
        context.route,
        context.effect,
        context.update_forward_count,
        context.profile_load_original_count,
        context.terminal,
        context.callback_count,
        context.dialog,
        context.cursor,
        context.model_row,
        context.layout_generation,
        context.layout_hash,
    ));
}

fn save_picker_record_unexpected_late_activation_with(
    dialog: usize,
    cursor: i32,
    telemetry_sink: PickerActivationTelemetrySink,
) {
    let seq = SAVE_PICKER_ACTIVATION_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    SAVE_PICKER_UNEXPECTED_LATE_ACTIVATIONS.fetch_add(1, Ordering::SeqCst);
    SAVE_PICKER_SOURCE_ACCEPTED_EVENTS.fetch_add(1, Ordering::SeqCst);
    SAVE_PICKER_NAMED_REJECTIONS.fetch_add(1, Ordering::SeqCst);
    let context = PickerActivationContext {
        seq,
        source: "profile-load-late",
        dialog,
        row_input_gate: 0,
        cursor,
        model_row: save_picker_model_row_from_native_cursor(cursor),
        layout_generation: SAVE_PICKER_LAYOUT_GENERATION.load(Ordering::SeqCst),
        layout_hash: save_picker_layout_hash(),
        provenance: er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation,
        physical_click: None,
        callback_count: 1,
        route_count: 1,
        effect_count: 0,
        update_forward_count: 0,
        profile_load_original_count: 0,
        terminal_count: 1,
        route: "reject",
        effect: "late",
        terminal: "late",
    };
    deliver_picker_activation_telemetry(context, telemetry_sink);
}

impl PickerNativeLifecycleAdapter {
    unsafe fn run_update_with(
        self,
        identity: PickerDialogIdentity,
        update_scalar: f32,
        row_input_gate: *const u8,
        make_context: impl FnOnce() -> PickerActivationContext,
    ) {
        let Some(original) = self.update_original else {
            SAVE_PICKER_ACTIVATION_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
            return;
        };
        if !identity.is_exact_active_picker() {
            unsafe { original(identity.dialog, update_scalar, row_input_gate) };
            return;
        }

        let scope = PickerActivationScope::enter(make_context());
        unsafe { original(identity.dialog, update_scalar, row_input_gate) };
        SAVE_PICKER_UPDATE_FORWARDS.fetch_add(1, Ordering::SeqCst);
        let Some(mut context) = scope.finish() else {
            SAVE_PICKER_ACTIVATION_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
            return;
        };
        context.update_forward_count = 1;
        if context.callback_count == 0 {
            if context.provenance
                == er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation
            {
                return;
            }
            SAVE_PICKER_SOURCE_ACCEPTED_EVENTS.fetch_add(1, Ordering::SeqCst);
            SAVE_PICKER_NAMED_REJECTIONS.fetch_add(1, Ordering::SeqCst);
            SAVE_PICKER_ACTIVATION_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
            context.route_count = 1;
            context.terminal_count = 1;
            context.route = "reject";
            context.effect = "native-matcher-no-callback";
            context.terminal = "native-matcher-no-callback";
        }
        deliver_picker_activation_telemetry(context, self.telemetry_sink);
    }

    pub(crate) unsafe fn dispatch_profile_load(
        self,
        identity: PickerDialogIdentity,
        cursor: i32,
    ) -> PickerProfileLoadDispatch {
        if !identity.is_exact_active_picker() {
            if let Some(original) = self.profile_load_original {
                unsafe { original(identity.dialog) };
            } else {
                SAVE_PICKER_ACTIVATION_INVARIANT_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
            }
            return PickerProfileLoadDispatch::OriginalForwarded;
        }

        SAVE_PICKER_SUPPRESSED_PROFILE_LOAD_ORIGINALS.fetch_add(1, Ordering::SeqCst);
        if !unsafe {
            save_picker_route_scoped_profile_load_callback_with(
                identity.dialog,
                cursor,
                self.effect_sink,
            )
        } {
            save_picker_record_unexpected_late_activation_with(
                identity.dialog,
                cursor,
                self.telemetry_sink,
            );
        }
        PickerProfileLoadDispatch::PickerSuppressed
    }
}

pub(crate) fn save_picker_dialog_identity(dialog: usize) -> PickerDialogIdentity {
    let expected_vtable = game_module_base()
        .ok()
        .and_then(|base| base.checked_add(PROFILE_LOAD_DIALOG_VTABLE_RVA));
    PickerDialogIdentity {
        picker_mode_active: SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0,
        dialog,
        active_dialog: SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst),
        expected_vtable,
        actual_vtable: (dialog != 0)
            .then(|| unsafe { safe_read_usize(dialog) })
            .flatten(),
    }
}

fn save_picker_profile_load_original() -> Option<ProfileLoadActivateFn> {
    let original = SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_ORIG.load(Ordering::SeqCst);
    (original != HOOK_ORIGINAL_UNSET)
        .then(|| unsafe { std::mem::transmute::<usize, ProfileLoadActivateFn>(original) })
}

pub(crate) unsafe extern "system" fn profile_load_menu_window_update_hook(
    dialog: usize,
    update_scalar: f32,
    row_input_gate: *const u8,
) {
    let original = PROFILE_LOAD_MENU_WINDOW_UPDATE_ORIG.load(Ordering::SeqCst);
    let update_original = (original != HOOK_ORIGINAL_UNSET)
        .then(|| unsafe { std::mem::transmute::<usize, ProfileLoadMenuWindowUpdateFn>(original) });
    let adapter = PickerNativeLifecycleAdapter {
        update_original,
        profile_load_original: save_picker_profile_load_original(),
        effect_sink: save_picker_handle_activation,
        telemetry_sink: save_picker_commit_activation_context,
    };
    let identity = save_picker_dialog_identity(dialog);
    unsafe {
        adapter.run_update_with(identity, update_scalar, row_input_gate, || {
            let cursor = unsafe { save_picker_native_cursor_for_event(dialog) }.unwrap_or(-1);
            let mut physical_click = None;
            let provenance = save_picker_compose_activation_provenance_with(
                row_input_gate,
                |gate| unsafe { system_quit_native_accept_pressed(gate) },
                |gate| unsafe { system_quit_native_primary_pointer_pressed(gate) },
                |gate| {
                    let classification =
                        unsafe { save_picker_physical_activation_provenance(dialog, cursor, gate) };
                    physical_click = classification.diagnostic;
                    classification.provenance
                },
            );
            PickerActivationContext {
                seq: SAVE_PICKER_ACTIVATION_SEQ.fetch_add(1, Ordering::SeqCst) + 1,
                source: "menu-window-update",
                dialog,
                row_input_gate: row_input_gate as usize,
                cursor,
                model_row: save_picker_model_row_from_native_cursor(cursor),
                layout_generation: SAVE_PICKER_LAYOUT_GENERATION.load(Ordering::SeqCst),
                layout_hash: save_picker_layout_hash(),
                provenance,
                physical_click,
                callback_count: 0,
                route_count: 0,
                effect_count: 0,
                update_forward_count: 0,
                profile_load_original_count: 0,
                terminal_count: 0,
                route: "none",
                effect: "none",
                terminal: "none",
            }
        });
    }
}

const _: ProfileLoadMenuWindowUpdateFn = profile_load_menu_window_update_hook;

#[cfg(test)]
#[path = "picker_native_lifecycle_adapter_tests.rs"]
mod picker_native_lifecycle_adapter_tests;
#[cfg(test)]
#[path = "save_picker_scrollbar_tests.rs"]
mod save_picker_scrollbar_tests;

// Legacy latch encoding remains only as a pure unit-test fixture. Production ownership is the
// thread-local scoped context above; no process-global pending atomic exists.
#[cfg(test)]
const SAVE_PICKER_DRIVE_STRIP_NO_PENDING_CELL: usize = usize::MAX;
#[cfg(test)]
const SAVE_PICKER_DRIVE_STRIP_PATH_EDITOR_PENDING: usize = usize::MAX - 1;
#[cfg(test)]
const SAVE_PICKER_DRIVE_STRIP_REJECTED_PHYSICAL_PENDING: usize = usize::MAX - 2;
#[cfg(test)]
const SAVE_PICKER_DRIVE_STRIP_ORDINARY_PHYSICAL_PENDING: usize = usize::MAX - 3;
#[cfg(test)]
const SAVE_PICKER_DRIVE_STRIP_CONSUMED_ACTIVATION_PENDING: usize = usize::MAX - 4;
#[cfg(test)]
const SAVE_PICKER_DRIVE_STRIP_KEYBOARD_ACCEPT_PENDING: usize = usize::MAX - 5;
#[cfg(test)]
const SAVE_PICKER_DRIVE_STRIP_UNKNOWN_ACTIVATION_PENDING: usize = usize::MAX - 6;
#[cfg(test)]
std::thread_local! {
    static TEST_PENDING_ACTIVATION: std::cell::Cell<usize> =
        const { std::cell::Cell::new(SAVE_PICKER_DRIVE_STRIP_NO_PENDING_CELL) };
}
#[cfg(test)]
fn save_picker_encode_pending_drive_strip_target(target: er_save_picker::DriveStripFocus) -> usize {
    match target {
        er_save_picker::DriveStripFocus::Cell(cell) => cell,
        er_save_picker::DriveStripFocus::CurrentPath => SAVE_PICKER_DRIVE_STRIP_PATH_EDITOR_PENDING,
    }
}
#[cfg(test)]
fn save_picker_decode_pending_drive_strip_target(
    pending: usize,
    cell_count: usize,
) -> Option<er_save_picker::DriveStripFocus> {
    if pending == SAVE_PICKER_DRIVE_STRIP_PATH_EDITOR_PENDING {
        Some(er_save_picker::DriveStripFocus::CurrentPath)
    } else {
        (pending < cell_count).then_some(er_save_picker::DriveStripFocus::Cell(pending))
    }
}
#[cfg(test)]
fn save_picker_decode_drive_strip_activation_provenance(
    pending: usize,
    cell_count: usize,
) -> er_save_picker::DriveStripActivationProvenance {
    match pending {
        SAVE_PICKER_DRIVE_STRIP_KEYBOARD_ACCEPT_PENDING => {
            er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept
        }
        SAVE_PICKER_DRIVE_STRIP_ORDINARY_PHYSICAL_PENDING => {
            er_save_picker::DriveStripActivationProvenance::OrdinaryRowPhysicalActivation
        }
        SAVE_PICKER_DRIVE_STRIP_REJECTED_PHYSICAL_PENDING => {
            er_save_picker::DriveStripActivationProvenance::RejectedPhysicalClick
        }
        SAVE_PICKER_DRIVE_STRIP_NO_PENDING_CELL
        | SAVE_PICKER_DRIVE_STRIP_CONSUMED_ACTIVATION_PENDING
        | SAVE_PICKER_DRIVE_STRIP_UNKNOWN_ACTIVATION_PENDING => {
            er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation
        }
        pending => er_save_picker::DriveStripActivationProvenance::physical_click(
            save_picker_decode_pending_drive_strip_target(pending, cell_count),
        ),
    }
}
#[cfg(test)]
fn save_picker_arm_drive_strip_activation_provenance(
    provenance: er_save_picker::DriveStripActivationProvenance,
) {
    let pending = match provenance {
        er_save_picker::DriveStripActivationProvenance::AcceptedPhysicalClick(target) => {
            save_picker_encode_pending_drive_strip_target(target)
        }
        er_save_picker::DriveStripActivationProvenance::RejectedPhysicalClick => {
            SAVE_PICKER_DRIVE_STRIP_REJECTED_PHYSICAL_PENDING
        }
        er_save_picker::DriveStripActivationProvenance::OrdinaryRowPhysicalActivation => {
            SAVE_PICKER_DRIVE_STRIP_ORDINARY_PHYSICAL_PENDING
        }
        er_save_picker::DriveStripActivationProvenance::KeyboardOrPadAccept => {
            SAVE_PICKER_DRIVE_STRIP_KEYBOARD_ACCEPT_PENDING
        }
        er_save_picker::DriveStripActivationProvenance::UnknownNativeActivation => {
            SAVE_PICKER_DRIVE_STRIP_UNKNOWN_ACTIVATION_PENDING
        }
    };
    TEST_PENDING_ACTIVATION.with(|cell| cell.set(pending));
}
#[cfg(test)]
fn save_picker_clear_pending_drive_strip_target() {
    TEST_PENDING_ACTIVATION.with(|cell| cell.set(SAVE_PICKER_DRIVE_STRIP_NO_PENDING_CELL));
}
#[cfg(test)]
fn save_picker_take_drive_strip_activation_provenance(
    cell_count: usize,
) -> er_save_picker::DriveStripActivationProvenance {
    TEST_PENDING_ACTIVATION.with(|cell| {
        let pending = cell.replace(SAVE_PICKER_DRIVE_STRIP_CONSUMED_ACTIVATION_PENDING);
        save_picker_decode_drive_strip_activation_provenance(pending, cell_count)
    })
}

fn save_picker_model_row_from_native_cursor(cursor: i32) -> Option<usize> {
    let row = cursor.checked_sub(PROFILE_SELECT_NATIVE_ROW_MODEL_OFFSET)?;
    (row >= 0 && (row as usize) < crate::experiments::save_picker::PICKER_ROW_COUNT)
        .then_some(row as usize)
}

pub(crate) fn save_picker_live_profile_dialog() -> usize {
    SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst)
}

include!("save_picker_refresh_request.rs");

fn save_picker_client_point_to_movie_stage(
    client_x: f32,
    client_y: f32,
    client_width: f32,
    client_height: f32,
) -> Option<(f32, f32)> {
    er_save_picker::DriveStripMovieViewport {
        client_origin_screen_x: 0.0,
        client_origin_screen_y: 0.0,
        client_width,
        client_height,
        movie_width: PROFILE_SELECT_MOVIE_WIDTH_PX,
        movie_height: PROFILE_SELECT_MOVIE_HEIGHT_PX,
    }
    .client_point_to_movie_stage(client_x, client_y)
}

fn save_picker_drive_strip_pointer_bounds(
    drive_row: usize,
) -> er_save_picker::DriveStripPointerBounds {
    let layout = er_gfx::profile_05_010_layout::shipped();
    let drive = layout.field(er_gfx::title_05_010::DRIVE_CELL_FIELD_NAMES[0]);
    let path = layout.field(er_gfx::title_05_010::CURRENT_PATH_FIELD_NAME);
    let (first_cell_left, cell_pitch, cell_width) = drive_strip_hit_geometry();
    let row_pitch = er_gfx::title_05_010::COMPACT_ROW_PITCH_PX as f32;
    let row_center_y = drive_row as f32 * row_pitch
        - (er_gfx::title_05_010::COMPACT_VISIBLE_ROW_COUNT as f32 * row_pitch) * 0.5
        + row_pitch * 0.5;
    er_save_picker::DriveStripPointerBounds {
        first_cell_left,
        cell_pitch,
        cell_width,
        path_left: path.x + layout.row_chrome.drive_button.x,
        path_width: path.width as f32,
        row_top: row_center_y + drive.y - 2.0 + layout.row_chrome.drive_button.y,
        row_height: drive.clip_height as f32,
    }
}

#[derive(Clone, Copy, Debug)]
struct ValidatedGameWindowGeometry {
    hwnd: windows::Win32::Foundation::HWND,
    window: er_save_picker::DriveStripWindowFacts,
    viewport: er_save_picker::DriveStripMovieViewport,
}

#[derive(Clone, Copy, Debug)]
struct ValidatedGamePointer {
    window: er_save_picker::DriveStripWindowFacts,
    stage_x: f32,
    stage_y: f32,
    packed_position: u64,
}

/// Capture one immutable client origin/rect/viewport from the exact ER HWND. Both event-local and
/// live screen coordinates must use this value; no second window lookup or geometry sample may
/// enter one physical-click classification.
unsafe fn save_picker_validated_game_window_geometry() -> Option<ValidatedGameWindowGeometry> {
    use windows::Win32::Foundation::{POINT, RECT};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClientRect, GetForegroundWindow, GetWindowThreadProcessId,
    };

    let hwnd = crate::experiments::game_main_window();
    if hwnd.0.is_null() || unsafe { GetForegroundWindow() } != hwnd {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 || pid != unsafe { GetCurrentProcessId() } {
        return None;
    }
    let mut rect = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rect) }.is_err() {
        return None;
    }
    let width = rect.right.checked_sub(rect.left)?;
    let height = rect.bottom.checked_sub(rect.top)?;
    if width <= 0 || height <= 0 {
        return None;
    }
    let mut client_origin = POINT::default();
    if !unsafe { ClientToScreen(hwnd, &mut client_origin) }.as_bool() {
        return None;
    }
    let viewport = er_save_picker::DriveStripMovieViewport {
        client_origin_screen_x: client_origin.x as f32,
        client_origin_screen_y: client_origin.y as f32,
        client_width: width as f32,
        client_height: height as f32,
        movie_width: PROFILE_SELECT_MOVIE_WIDTH_PX,
        movie_height: PROFILE_SELECT_MOVIE_HEIGHT_PX,
    };
    // Exercise the production transform now so an invalid origin/client/movie viewport cannot be
    // represented as validated geometry even when the live pointer is sampled later.
    viewport.client_point_to_movie_stage(0.0, 0.0).or_else(|| {
        viewport.client_point_to_movie_stage(width as f32 * 0.5, height as f32 * 0.5)
    })?;
    Some(ValidatedGameWindowGeometry {
        hwnd,
        window: er_save_picker::DriveStripWindowFacts {
            hwnd_present: true,
            foreground_matches: true,
            same_process: true,
            client_geometry_valid: true,
            pointer_in_client: true,
        },
        viewport,
    })
}

unsafe fn save_picker_validated_game_pointer_from(
    geometry: ValidatedGameWindowGeometry,
) -> Option<ValidatedGamePointer> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow};

    if unsafe { GetForegroundWindow() } != geometry.hwnd {
        return None;
    }
    let mut screen_point = POINT::default();
    if unsafe { GetCursorPos(&mut screen_point) }.is_err() {
        return None;
    }
    let (stage_x, stage_y) = geometry
        .viewport
        .screen_point_to_movie_stage(screen_point.x as f32, screen_point.y as f32)?;
    let (client_x, client_y) = geometry
        .viewport
        .screen_point_to_client(screen_point.x as f32, screen_point.y as f32)?;
    Some(ValidatedGamePointer {
        window: geometry.window,
        stage_x,
        stage_y,
        packed_position: u64::from(client_x) | (u64::from(client_y) << 32),
    })
}

/// Read the pointer only through one validated snapshot of the actual ER client HWND.
unsafe fn save_picker_validated_game_pointer() -> Option<ValidatedGamePointer> {
    let geometry = unsafe { save_picker_validated_game_window_geometry() }?;
    unsafe { save_picker_validated_game_pointer_from(geometry) }
}

/// Event cursor read with a last-moment published-owner/vtable lease.
unsafe fn save_picker_native_cursor_for_event(dialog: usize) -> Option<i32> {
    if dialog == 0 {
        return None;
    }
    let Ok(base) = game_module_base() else {
        return None;
    };
    let token = save_picker_current_live_token(dialog)?;
    let getter: unsafe extern "system" fn(usize) -> i32 =
        unsafe { std::mem::transmute(base + MENU_ITEM_LIST_CURSOR_GETTER_RVA) };
    execute_picker_live_token_call_with(
        token,
        save_picker_live_profile_dialog,
        |owner| unsafe { safe_read_usize(owner) },
        |owner| unsafe { getter(owner + PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET) },
    )
    .ok()
    .map(|(_, _, cursor)| cursor)
}

/// Menu-pump maintenance cursor read. The unforgeable argument is produced only by the current
/// exact `05_010_ProfileSelect` Run post after owner identity and vtable validation.
unsafe fn save_picker_native_cursor(token: PickerProfileRunToken) -> Option<i32> {
    let Ok(base) = game_module_base() else {
        return None;
    };
    let getter: unsafe extern "system" fn(usize) -> i32 =
        unsafe { std::mem::transmute(base + MENU_ITEM_LIST_CURSOR_GETTER_RVA) };
    execute_picker_live_token_call_with(
        token,
        save_picker_live_profile_dialog,
        |dialog| unsafe { safe_read_usize(dialog) },
        |dialog| unsafe { getter(dialog + PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET) },
    )
    .ok()
    .map(|(_, _, cursor)| cursor)
}

/// Focus a native ProfileSelect row through GridControl's own validated cursor route.
unsafe fn save_picker_set_native_row_focus(token: PickerProfileRunToken, row: usize) -> bool {
    if row >= crate::experiments::save_picker::PICKER_ROW_COUNT {
        return false;
    }
    let Ok(base) = game_module_base() else {
        return false;
    };
    let setter: unsafe extern "system" fn(usize, i32, u8) -> u8 =
        unsafe { std::mem::transmute(base + MENU_ITEM_LIST_CURSOR_SETTER_RVA) };
    let accepted = execute_picker_live_token_call_with(
        token,
        save_picker_live_profile_dialog,
        |dialog| unsafe { safe_read_usize(dialog) },
        |dialog| unsafe {
            setter(dialog + PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET, row as i32, 0) != 0
        },
    )
    .ok()
    .is_some_and(|(_, _, accepted)| accepted);
    let observed = unsafe { save_picker_native_cursor(token) } == Some(row as i32);
    accepted && observed
}

include!("save_picker_drive_strip_pump.rs");

/// Menu-pump scrollbar maintenance; each setter takes a fresh owner/vtable lease.
pub(crate) unsafe fn save_picker_menu_pump_native_scrollbar(token: PickerProfileRunToken) {
    let window = token.dialog;
    if !save_picker_profile_token_still_current(token)
        || SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0
        || save_picker_path_editor_blocks_profile_refresh()
        || save_picker_resubmit_pending()
    {
        SAVE_PICKER_SCROLLBAR_LAST_SYNC.store(usize::MAX, Ordering::SeqCst);
        return;
    }

    let (current, page, total) = {
        let guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_ref() else {
            return;
        };
        let page = model.entries_per_page().max(1);
        let total = model.entry_count().max(page);
        (
            model.scroll_offset().min(total.saturating_sub(page)),
            page,
            total,
        )
    };

    let Ok(base) = game_module_base() else {
        return;
    };
    let scrollbar = window + PROFILE_LOAD_DIALOG_SCROLLBAR_OFFSET;
    let set_total: unsafe extern "system" fn(usize, i32) =
        unsafe { std::mem::transmute(base + SCROLLBAR_CONTROL_SET_TOTAL_RVA as usize) };
    let set_position: unsafe extern "system" fn(usize, i32) =
        unsafe { std::mem::transmute(base + SCROLLBAR_CONTROL_SET_POSITION_RVA as usize) };
    let applied = save_picker_apply_native_scrollbar_with(
        base,
        scrollbar,
        total.min(i32::MAX as usize) as i32,
        current.min(i32::MAX as usize) as i32,
        |address| unsafe { safe_read_usize(address) },
        |owner, value| {
            execute_picker_live_token_call_with(
                token,
                save_picker_live_profile_dialog,
                |dialog| unsafe { safe_read_usize(dialog) },
                |_| unsafe { set_total(owner, value) },
            )
            .is_ok()
        },
        |owner, value| {
            execute_picker_live_token_call_with(
                token,
                save_picker_live_profile_dialog,
                |dialog| unsafe { safe_read_usize(dialog) },
                |_| unsafe { set_position(owner, value) },
            )
            .is_ok()
        },
    );
    let target = match applied {
        Ok(Some(target)) => {
            er_telemetry::counters::SAVE_PICKER_SCROLLBAR_DISPATCH_CALLS
                .fetch_add(1, Ordering::SeqCst);
            target
        }
        Ok(None) => {
            er_telemetry::counters::SAVE_PICKER_SCROLLBAR_DISPATCH_SKIPS
                .fetch_add(1, Ordering::SeqCst);
            return;
        }
        Err(rejection) => {
            let skips = er_telemetry::counters::SAVE_PICKER_SCROLLBAR_DISPATCH_SKIPS
                .fetch_add(1, Ordering::SeqCst)
                + 1;
            er_telemetry::counters::SAVE_PICKER_SCROLLBAR_LAST_REJECT_REASON
                .store(rejection.reason as usize, Ordering::SeqCst);
            er_telemetry::counters::SAVE_PICKER_SCROLLBAR_LAST_VTABLE
                .store(rejection.vtable, Ordering::SeqCst);
            er_telemetry::counters::SAVE_PICKER_SCROLLBAR_LAST_TARGET
                .store(rejection.target, Ordering::SeqCst);
            if skips <= 8 || skips.is_power_of_two() {
                append_autoload_debug(format_args!(
                    "save-picker: native scrollbar sync SKIPPED fail-closed reason={:?} scrollbar=0x{scrollbar:x} vtable=0x{:x} target=0x{:x} skips={skips}",
                    rejection.reason, rejection.vtable, rejection.target
                ));
            }
            return;
        }
    };

    let packed = save_picker_scrollbar_packed_state(current, page, total);
    if SAVE_PICKER_SCROLLBAR_LAST_SYNC.swap(packed, Ordering::SeqCst) != packed {
        append_autoload_debug(format_args!(
            "save-picker: native scrollbar sync current={current} page={page} total={total} scrollbar=0x{scrollbar:x} target=0x{target:x}"
        ));
    }
}

/// Edge-scroll maintenance over the ten-row native window.
pub(crate) unsafe fn save_picker_menu_pump_edge_scroll(token: PickerProfileRunToken) {
    let dialog = token.dialog;
    if !save_picker_profile_token_still_current(token)
        || SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0
        || save_picker_path_editor_blocks_profile_refresh()
        || save_picker_resubmit_pending()
    {
        return;
    }
    let Some(cursor) = (unsafe { save_picker_native_cursor(token) }) else {
        return;
    };
    let Some(model_row) = save_picker_model_row_from_native_cursor(cursor) else {
        return;
    };
    let scrolled = {
        let mut guard = crate::experiments::save_picker::active_save_picker_lock();
        let Some(model) = guard.as_mut() else {
            return;
        };
        model.edge_scroll_from_native_cursor_tick(model_row)
    };
    if !scrolled {
        return;
    }
    if save_picker_schedule_refresh_request(dialog, "edge-scroll") {
        append_autoload_debug(format_args!(
            "save-picker: edge-scroll queued fresh-owner presentation at native_cursor={cursor} model_row={model_row}"
        ));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickerRefreshConsumeDisposition {
    NativeRemovalHandoff,
    Deferred,
    AwaitingOwnerDisappearance,
    AwaitingLiveToken,
    OwnerAlreadyCleared,
    StaleIdentity,
    AlreadyClosing,
    CloseRequested(PickerRefreshCloseDisposition),
}

fn consume_picker_refresh_with_native_removal(
    native_removal_owns_request: bool,
    request: PickerRefreshRequest,
    current_dialog: usize,
    picker_active: bool,
    modal_blocked: bool,
    other_close_pending: bool,
    matching_path_return_pending: bool,
    observation: PickerProfileRunObservation,
    arm_reopen: impl FnOnce(),
    close: impl FnOnce(PickerProfileRunToken) -> PickerRefreshCloseDisposition,
) -> PickerRefreshConsumeDisposition {
    if native_removal_owns_request {
        return PickerRefreshConsumeDisposition::NativeRemovalHandoff;
    }
    if !picker_active || request.dialog == 0 {
        return PickerRefreshConsumeDisposition::StaleIdentity;
    }
    if current_dialog == 0 {
        if other_close_pending {
            return PickerRefreshConsumeDisposition::AlreadyClosing;
        }
        arm_reopen();
        return PickerRefreshConsumeDisposition::OwnerAlreadyCleared;
    }
    // Identity is first: an old dialog may never hide behind an unrelated AlreadyClosing state.
    if request.dialog != current_dialog {
        return PickerRefreshConsumeDisposition::StaleIdentity;
    }
    // SoftwareKeyboard owns the parent finish. A matching exact-generation no-close return always
    // outranks content refresh: wait for native disappearance, then stage the changed model at zero.
    if matching_path_return_pending {
        return PickerRefreshConsumeDisposition::AwaitingOwnerDisappearance;
    }
    if modal_blocked {
        return PickerRefreshConsumeDisposition::Deferred;
    }
    if other_close_pending {
        return PickerRefreshConsumeDisposition::AlreadyClosing;
    }
    let Some(token) = observation
        .live_token()
        .filter(|token| token.dialog == request.dialog && token.dialog == current_dialog)
    else {
        return PickerRefreshConsumeDisposition::AwaitingLiveToken;
    };
    arm_reopen();
    PickerRefreshConsumeDisposition::CloseRequested(close(token))
}

fn consume_picker_refresh_with(
    request: PickerRefreshRequest,
    current_dialog: usize,
    picker_active: bool,
    modal_blocked: bool,
    other_close_pending: bool,
    matching_path_return_pending: bool,
    observation: PickerProfileRunObservation,
    arm_reopen: impl FnOnce(),
    close: impl FnOnce(PickerProfileRunToken) -> PickerRefreshCloseDisposition,
) -> PickerRefreshConsumeDisposition {
    consume_picker_refresh_with_native_removal(
        false,
        request,
        current_dialog,
        picker_active,
        modal_blocked,
        other_close_pending,
        matching_path_return_pending,
        observation,
        arm_reopen,
        close,
    )
}

include!("save_picker_owner_zero_recovery.rs");

/// Consume one exact refresh request in menu-pump ownership. A native close is possible only when
/// this exact post carries the current live `05_010_ProfileSelect` owner token. Generic MenuWindow
/// posts retain the request without dereferencing the published pointer.
pub(crate) unsafe fn save_picker_menu_pump_refresh(observation: PickerProfileRunObservation) {
    let Some(request) = load_picker_refresh_request() else {
        return;
    };
    // Exact native removal transfers this refresh's latches to the one-shot picker-submit ticket.
    // Do not re-enter OwnerAlreadyCleared on every generic MenuWindow post while stage/submit retries.
    let native_removal_owns_request = save_picker_native_removal_owns_refresh(request);
    let current = save_picker_live_profile_dialog();
    let disposition = consume_picker_refresh_with_native_removal(
        native_removal_owns_request,
        request,
        current,
        SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0,
        save_picker_path_editor_blocks_profile_refresh(),
        SAVE_PICKER_OPEN_SLOTS_PENDING.load(Ordering::SeqCst) != 0,
        save_picker_path_editor_return_pending_for(request.dialog)
            || SAVE_PICKER_REFRESH_CLOSE_REQUESTED_GENERATION.load(Ordering::SeqCst)
                == request.generation,
        observation,
        || {
            let _latch_guard = resubmit_latch_lock();
            if !any_resubmit_reserved() {
                SAVE_PICKER_REOPEN_PENDING.store(1, Ordering::SeqCst);
                let _ =
                    arm_picker_pending_resubmit_transition(request.dialog, 0, request.generation);
            }
        },
        |token| unsafe { save_picker_refresh_native_close(token, "fresh-owner-refresh") },
    );
    match disposition {
        PickerRefreshConsumeDisposition::NativeRemovalHandoff => {}
        PickerRefreshConsumeDisposition::Deferred => {}
        PickerRefreshConsumeDisposition::AwaitingOwnerDisappearance => {
            er_telemetry::counters::SAVE_PICKER_REFRESH_NO_LIVE_TOKEN_DEFERS
                .fetch_add(1, Ordering::SeqCst);
        }
        PickerRefreshConsumeDisposition::AwaitingLiveToken => {
            er_telemetry::counters::SAVE_PICKER_REFRESH_NO_LIVE_TOKEN_DEFERS
                .fetch_add(1, Ordering::SeqCst);
        }
        PickerRefreshConsumeDisposition::OwnerAlreadyCleared => {
            if save_picker_native_removal_authority().is_some() {
                er_telemetry::counters::SAVE_PICKER_OWNER_ZERO_LOOP_GUARD_VIOLATIONS
                    .fetch_add(1, Ordering::SeqCst);
            }
            let retired = false;
            er_telemetry::counters::SAVE_PICKER_REFRESH_OWNER_ZERO_NO_CLOSES
                .fetch_add(1, Ordering::SeqCst);
            let spins = save_picker_owner_zero_spin_tick(request.generation);
            if spins == 1 {
                // One line per generation naming EACH conjunct. The 2026-08-11 run produced 36,501
                // identical "resubmit remains armed" lines that proved the ticket was rejected but
                // could not say which predicate rejected it.
                save_picker_log_owner_zero_ticket_rejection(request);
            }
            if spins <= 2 || spins == SAVE_PICKER_OWNER_ZERO_SPIN_LIMIT {
                append_autoload_debug(format_args!(
                    "save-picker: refresh generation={} reached owner-zero without native close old_owner=0x{:x} retired={retired} spins={spins}; fresh-owner resubmit remains armed",
                    request.generation, request.dialog
                ));
            }
            if spins >= SAVE_PICKER_OWNER_ZERO_SPIN_LIMIT {
                // The picker window is gone and the resubmit will not land. Holding
                // SAVE_PICKER_MODE_ACTIVE here keeps suppressing every underlying System row, so
                // the user is left in a quit menu whose buttons all do nothing. Releasing picker
                // ownership is strictly better than an invisible menu that eats input forever.
                unsafe { save_picker_release_wedged_owner_zero_refresh(request) };
            }
        }
        PickerRefreshConsumeDisposition::StaleIdentity => {
            let retired = retire_picker_refresh_request(request, false);
            append_autoload_debug(format_args!(
                "save-picker: dropped stale refresh without dereference requested=0x{:x} generation={} current=0x{current:x} retired={retired}",
                request.dialog, request.generation
            ));
        }
        PickerRefreshConsumeDisposition::AlreadyClosing => {
            let retired = retire_picker_refresh_request(request, false);
            er_telemetry::counters::SAVE_PICKER_REFRESH_COALESCES.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker: refresh generation={} retired without close because another picker close owns the transition retired={retired}",
                request.generation
            ));
        }
        PickerRefreshConsumeDisposition::CloseRequested(close) => {
            if close == PickerRefreshCloseDisposition::Closed {
                let _latch_guard = resubmit_latch_lock();
                if !any_resubmit_reserved() {
                    SAVE_PICKER_REFRESH_CLOSE_REQUESTED_GENERATION
                        .store(request.generation, Ordering::SeqCst);
                    let _ = arm_picker_pending_resubmit_transition(
                        request.dialog,
                        0,
                        request.generation,
                    );
                }
            }
            let resolution =
                apply_picker_refresh_close_with(request, close, retire_picker_refresh_request);
            append_autoload_debug(format_args!(
                "save-picker: fresh-owner close disposition={close:?} old_owner=0x{:x} generation={} resolution={resolution:?} reopen={}",
                request.dialog,
                request.generation,
                SAVE_PICKER_REOPEN_PENDING.load(Ordering::SeqCst)
            ));
        }
    }
}

/// Owner-zero menu-pump resubmit without dereferencing the closed dialog.
pub(crate) unsafe fn save_picker_menu_pump_resubmit(authority: PickerOuterPostAuthority) -> bool {
    if !save_picker_resubmit_pending() {
        return false;
    }
    let old_owner = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    if old_owner != 0 {
        return true;
    }
    // Reopen through the System dialog the window was submitted from. The destination browser has
    // no row action object at all (the save flow opens it), so the dialog -- not the dead 05_010
    // owner -- is the identity that survives both paths.
    let system_identity = save_picker_system_dialog_identity();
    let system_dialog = system_identity.map_or(0, |identity| identity.dialog);
    let expected = match authority {
        PickerOuterPostAuthority::NativeRemoval(authority) => Some(authority.pending),
        PickerOuterPostAuthority::Profile(PickerProfileRunObservation::OwnerCleared(cleared)) => {
            Some(cleared.pending)
        }
        _ => save_picker_pending_resubmit_transition(),
    };
    if let Some(abandoned) = abandon_lost_system_dialog_resubmit_with(
        system_dialog,
        expected,
        abandon_picker_pending_resubmit_for_system_dialog_loss,
    ) {
        append_autoload_debug(format_args!(
            "save-picker: owning System dialog lost; atomic pending-resubmit abandonment={abandoned}"
        ));
        return false;
    }
    if let Some(expected) = expected {
        let bound_system = PickerSystemDialogIdentity {
            dialog: expected.system_dialog,
            generation: expected.system_dialog_generation,
        };
        if system_identity != Some(bound_system) {
            let abandoned = abandon_picker_pending_resubmit_for_system_dialog_loss(Some(expected));
            append_autoload_debug(format_args!(
                "save-picker: System-dialog identity changed before resubmit; exact old transition abandonment={abandoned}"
            ));
            return false;
        }
    }
    let reopen_as_picker = SAVE_PICKER_REOPEN_PENDING.load(Ordering::SeqCst) != 0;
    if reopen_as_picker {
        if let Some(request) = load_path_editor_return_reopen_request()
            && er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_LAST_OWNER_CLEARED_GENERATION
                .swap(request.generation, Ordering::SeqCst)
                != request.generation
        {
            er_telemetry::counters::SAVE_PICKER_PATH_EDITOR_RETURN_OWNER_CLEARED
                .fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-path: observed parent owner zero for no-close return dialog=0x{:x} generation={}",
                request.dialog, request.generation
            ));
        }
        let generation =
            er_telemetry::counters::SAVE_PICKER_REFRESH_GENERATION.load(Ordering::SeqCst);
        if er_telemetry::counters::SAVE_PICKER_REFRESH_LAST_OWNER_CLEARED_GENERATION
            .swap(generation, Ordering::SeqCst)
            != generation
        {
            er_telemetry::counters::SAVE_PICKER_REFRESH_OWNER_CLEARED
                .fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker: observed old owner cleared for refresh generation={generation}"
            ));
        }
    }
    let stage_latest_model = || {
        er_telemetry::counters::SAVE_PICKER_RESUBMIT_STAGE_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
        let snapshot = {
            let guard = crate::experiments::save_picker::active_save_picker_lock();
            guard.as_ref().map(PickerRowStageSnapshot::from_model)
        };
        let staged = snapshot
            .as_ref()
            .is_some_and(|snapshot| unsafe { save_picker_stage_row_snapshot(snapshot) });
        if staged {
            er_telemetry::counters::SAVE_PICKER_RESUBMIT_STAGE_SUCCESSES
                .fetch_add(1, Ordering::SeqCst);
        } else {
            er_telemetry::counters::SAVE_PICKER_RESUBMIT_STAGE_FAILURES
                .fetch_add(1, Ordering::SeqCst);
        }
        staged
    };
    let disposition = match authority {
        PickerOuterPostAuthority::NativeRemoval(removal_authority) if reopen_as_picker => {
            execute_picker_native_removal_resubmit(
                removal_authority,
                stage_latest_model,
                || {
                    let _ = unsafe { rollback_picker_staged_presentation() };
                },
                || {
                    let _ = commit_picker_staged_presentation();
                },
                || {
                    er_telemetry::counters::SAVE_PICKER_RESUBMIT_SUBMIT_ATTEMPTS
                        .fetch_add(1, Ordering::SeqCst);
                    unsafe { system_quit_open_profile_load_dialog_on(system_dialog) }
                },
            )
        }
        PickerOuterPostAuthority::Profile(PickerProfileRunObservation::OwnerCleared(
            cleared_authority,
        )) if reopen_as_picker => execute_picker_owner_cleared_resubmit(
            PickerOwnerClearedResubmitAuthority {
                picker: cleared_authority,
                system: PickerSystemDialogIdentity {
                    dialog: cleared_authority.pending.system_dialog,
                    generation: cleared_authority.pending.system_dialog_generation,
                },
            },
            stage_latest_model,
            || {
                let _ = unsafe { rollback_picker_staged_presentation() };
            },
            || {
                let _ = commit_picker_staged_presentation();
            },
            || {
                er_telemetry::counters::SAVE_PICKER_RESUBMIT_SUBMIT_ATTEMPTS
                    .fetch_add(1, Ordering::SeqCst);
                unsafe { system_quit_open_profile_load_dialog_on(system_dialog) }
            },
        ),
        PickerOuterPostAuthority::DestinationParent(_) if !reopen_as_picker => {
            execute_picker_destination_resubmit(
                old_owner,
                system_identity.expect("nonzero dialog has synchronized identity"),
                || picker_outer_authority_still_current(authority),
                || {
                    er_telemetry::counters::SAVE_PICKER_RESUBMIT_SUBMIT_ATTEMPTS
                        .fetch_add(1, Ordering::SeqCst);
                },
                || unsafe { system_quit_open_profile_load_dialog_on(system_dialog) },
            )
        }
        _ => PickerResubmitDisposition::AuthorizationLost,
    };
    commit_picker_native_removal_after_resubmit(authority, disposition);
    apply_picker_resubmit_model_lifetime_with(disposition, reopen_as_picker, || {
        *crate::experiments::save_picker::active_save_picker_lock() = None;
        SAVE_PICKER_MODE_ACTIVE.store(0, Ordering::SeqCst);
    });
    let PickerResubmitDisposition::Submitted { opened } = disposition else {
        if disposition == PickerResubmitDisposition::StageFailed {
            append_autoload_debug(format_args!(
                "save-picker: owner-cleared row staging failed; keeping refresh pending and submitting nothing"
            ));
        }
        return true;
    };
    if opened {
        SAVE_PICKER_RESUBMIT_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker: menu-pump resubmitted 05_010 window as {} (dialog=0x{system_dialog:x})",
            if reopen_as_picker {
                "picker page"
            } else {
                "slot view"
            }
        ));
        return true;
    }
    append_autoload_debug(format_args!(
        "save-picker: menu-pump native resubmit returned false (dialog=0x{system_dialog:x}); exact picker/destination transition remains armed for retry"
    ));
    true
}

/// Escape text for the Scaleform-HTML SetText path (the `ErStats` row fields parse with bHTML=1,
/// so a character/file name containing `&`, `<` or `>` must not be interpreted as markup).
pub(crate) fn save_picker_html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// One dim Scaleform-HTML line for the browse rows' `ErStats` fields (same size/color language as
/// the stats panel's attribute lines), NUL-terminated UTF-16 for the native SetText wrapper. An
/// empty `text` yields a bare NUL so the field renders blank.
pub(crate) fn save_picker_browse_html_utf16(text: &str) -> Vec<u16> {
    save_picker_browse_html_utf16_color(text, "#8f887a")
}

pub(crate) fn save_picker_error_html_utf16(text: &str) -> Vec<u16> {
    save_picker_browse_html_utf16_color(text, "#d8a052")
}

pub(crate) fn save_picker_browse_html_utf16_color(text: &str, color: &str) -> Vec<u16> {
    // Match the native ProfileSelect filename/timestamp fields; the asset gives ErStats a native-height box.
    const SIZE: &str = "24";
    if text.is_empty() {
        return vec![0];
    }
    let mut s = String::from("<p align=\"left\"><font size=\"");
    s.push_str(SIZE);
    s.push_str("\" color=\"");
    s.push_str(color);
    s.push_str("\">");
    s.push_str(&save_picker_html_escape(text));
    s.push_str("</font></p>");
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

pub(crate) fn save_picker_set_visible_status(message: er_save_picker::PickerStatusMessage) {
    if let Some(model) = crate::experiments::save_picker::active_save_picker_lock().as_mut() {
        model.set_status_message(message);
    }
}

/// Character budget for the per-file character list fragment. This text is merged onto the single
/// inline `ErStats` row field beside the filename and timestamp, so it must stay short enough to read
/// as row detail instead of a wrapped second line.
pub(crate) const SAVE_PICKER_BROWSE_LINE_CHAR_BUDGET: usize = 34;

pub(crate) fn save_picker_drive_cell_html_utf16(text: &str) -> Vec<u16> {
    // The button frame already supplies the visual boundary. Keep the model's `>C:<` / `[S:]`
    // wrappers for the boot overlay and selection semantics, but do not render that punctuation
    // inside the compact native button -- it clips before the drive letter does.
    let (selected, display) = if let Some(inner) = text
        .strip_prefix('>')
        .and_then(|inner| inner.strip_suffix('<'))
    {
        (true, inner)
    } else if let Some(inner) = text
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
    {
        (false, inner)
    } else {
        (false, text)
    };
    let color = if selected { "#d8a052" } else { "#8f887a" };
    save_picker_browse_html_utf16_color(display, color)
}

/// Resting colour of the CurrentPath control -- the same parchment tone the surrounding native
/// ProfileSelect text uses.
const SAVE_PICKER_PATH_NORMAL_COLOR: &str = "#b8b1a2";
/// Colour for a path the user submitted that failed validation. Yellow rather than red: the entry
/// is correctable, not an error state, and the folder the picker is showing has not changed.
const SAVE_PICKER_PATH_INVALID_COLOR: &str = "#e8c34a";

pub(crate) fn save_picker_current_path_text(row: usize) -> Option<Vec<u16>> {
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 && !missing_save_selection_pending() {
        return None;
    }
    let guard = crate::experiments::save_picker::active_save_picker_lock();
    let model = guard.as_ref()?;
    if model.drive_row() != Some(row) {
        return Some(vec![0]);
    }
    // A rejected submission renders in place of the current directory, in the invalid colour, so
    // the user sees what they typed and can correct it. `clear_status_message` (which every
    // successful navigation calls) drops it, returning the control to the normal colour.
    let (text, color) = match model.rejected_path_text() {
        Some(rejected) => (rejected, SAVE_PICKER_PATH_INVALID_COLOR),
        None => (model.current_dir().to_str()?, SAVE_PICKER_PATH_NORMAL_COLOR),
    };
    let escaped = save_picker_html_escape(text);
    let html =
        format!("<p align=\"left\"><font size=\"16\" color=\"{color}\">{escaped}</font></p>");
    Some(html.encode_utf16().chain(core::iter::once(0)).collect())
}

pub(crate) fn save_picker_drive_cell_text(row: usize, cell: usize) -> Option<Vec<u16>> {
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 && !missing_save_selection_pending() {
        return None;
    }
    let guard = crate::experiments::save_picker::active_save_picker_lock();
    let model = guard.as_ref()?;
    let text = model.drive_row_cell_label(row, cell).unwrap_or_default();
    Some(save_picker_drive_cell_html_utf16(&text))
}

/// The `ErStats` fragments for ProfileSelect row `row` while the browse picker owns the window.
/// The row-populate hook merges the two fragments into ONE inline field: file rows show active-slot
/// count plus character names/levels beside `ER0000.sl2`, while navigation/status rows show their
/// auxiliary copy beside the row label. Empty rows get blank fragments so neither leftover row text
/// nor per-slot attribute stats render as junk there. `None` when the picker does not own the rows
/// (the normal character-slot view keeps the attribute stats panel).
pub(crate) fn save_picker_browse_stats_lines(row: usize) -> Option<(Vec<u16>, Vec<u16>)> {
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 && !missing_save_selection_pending() {
        return None;
    }
    let guard = crate::experiments::save_picker::active_save_picker_lock();
    let model = guard.as_ref()?;
    let status_row = model.status_message().is_some() && row == 0;
    if let Some((top, bottom)) = model.row_auxiliary_lines(row) {
        if status_row {
            return Some((
                save_picker_error_html_utf16(&top),
                save_picker_error_html_utf16(&bottom),
            ));
        }
        return Some((
            save_picker_browse_html_utf16(&top),
            save_picker_browse_html_utf16(&bottom),
        ));
    }
    let is_current = model.row_is_loaded_save(row);
    let Some(chars) = model.row_file_characters(row) else {
        // Empty row: blank the injected stats field so no per-slot attribute stats render as junk.
        return Some((vec![0], vec![0]));
    };
    let count = if chars.len() == 1 {
        "1 CHAR".to_owned()
    } else {
        format!("{} CHAR", chars.len())
    };
    let top = if is_current {
        format!("* {count}")
    } else {
        count
    };
    let mut bottom = String::new();
    let mut shown = 0usize;
    for info in chars {
        let seg = format!("{} L{}", info.name, info.level);
        let sep = if bottom.is_empty() { "" } else { " / " };
        if !bottom.is_empty()
            && bottom.chars().count() + sep.chars().count() + seg.chars().count()
                > SAVE_PICKER_BROWSE_LINE_CHAR_BUDGET
        {
            break;
        }
        bottom.push_str(sep);
        bottom.push_str(&seg);
        shown += 1;
    }
    if shown < chars.len() {
        bottom.push_str(&format!(" +{}", chars.len() - shown));
    }
    Some((
        save_picker_browse_html_utf16(&top),
        save_picker_browse_html_utf16(&bottom),
    ))
}

/// What a picker-owned row does with every optional ProfileSelect field family.
///
/// The `Level` caption/value and bottom `PlayTime` are hidden for every picker row. The remaining
/// fields are row-kind-specific: a save-file row can stage its timestamp into top-right `Location`,
/// metadata rows own `ErStats`, and only the drive-cycle row owns populated `DriveCell_0..25` cells.
pub(crate) struct RowSlotInfo {
    /// Replacement text for the `Location` field (when the file was last written), or `None` to hide
    /// the field -- which is what every non-file row gets, and what a file whose timestamp is
    /// unreadable gets rather than a fabricated date.
    pub(crate) location: Option<String>,
    /// Whether this row has real `ErStats` copy. False on the drive row unless a visible status
    /// message temporarily owns it, so stale parent-folder copy cannot survive row-clip reuse.
    pub(crate) er_stats: bool,
    /// Number of populated drive-strip cells on this row. Zero outside the drive row and while a
    /// visible status message temporarily owns its field band.
    pub(crate) drive_cell_count: usize,
    /// Explicit drive/path subtarget whose geometry the row's one native animated Cursor follows.
    pub(crate) drive_strip_focus: Option<er_save_picker::DriveStripFocus>,
}

/// What the browse picker wants done with ProfileSelect row `row`'s per-slot info fields.
///
/// `None` when the picker does NOT own the rows. That is the load-bearing half of the scope: the
/// vanilla character-slot views, the title-screen Load Game list first among them, render from the
/// game's own records and must be left exactly as the game draws them. Same ownership gate as
/// [`save_picker_browse_stats_lines`], so the two cannot disagree about who owns a row.
pub(crate) fn save_picker_row_slot_info(row: usize) -> Option<RowSlotInfo> {
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) == 0 && !missing_save_selection_pending() {
        return None;
    }
    let (last_saved, er_stats, drive_cell_count, drive_strip_focus) = {
        let guard = crate::experiments::save_picker::active_save_picker_lock();
        let model = guard.as_ref()?;
        let has_auxiliary_lines = model.row_auxiliary_lines(row).is_some();
        let drive_cell_count = if model.drive_row() == Some(row) && !has_auxiliary_lines {
            model.drive_strip_cell_count()
        } else {
            0
        };
        let drive_strip_focus = (drive_cell_count > 0)
            .then(|| model.drive_strip_presented_focus())
            .flatten();
        (
            model.row_last_saved(row),
            has_auxiliary_lines || model.row_file_characters(row).is_some(),
            drive_cell_count,
            drive_strip_focus,
        )
    };
    Some(RowSlotInfo {
        location: last_saved.and_then(save_picker_last_saved_text),
        er_stats,
        drive_cell_count,
        drive_strip_focus,
    })
}

/// Render one file's modification time as the row's last-saved text, in local time.
/// `None` when the stamp predates the epoch or the OS cannot give a local offset for it -- the row
/// then hides the field rather than showing a date we cannot stand behind.
pub(crate) fn save_picker_last_saved_text(modified: std::time::SystemTime) -> Option<String> {
    let secs = modified
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let secs = i64::try_from(secs).ok()?;
    crate::experiments::save_picker::format_last_saved(secs, unsafe {
        local_utc_offset_seconds(secs)
    }?)
}

/// The local zone's offset from UTC at the instant `utc_secs`, in seconds.
///
/// Asks WINDOWS rather than assuming, and asks about THAT INSTANT rather than about now:
/// `SystemTimeToTzSpecificLocalTime` applies the zone's DST rules for the given date, so a save
/// written on the other side of a DST boundary still renders the wall-clock time it was written at.
/// (Comparing `GetLocalTime` to `GetSystemTime` would give only the CURRENT offset and misdate every
/// file from the other side of the boundary by an hour.) The offset comes back as a number, which is
/// all the pure formatter needs -- that is what keeps the rendering unit-testable.
pub(crate) unsafe fn local_utc_offset_seconds(utc_secs: i64) -> Option<i64> {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{
        FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    };

    /// 100ns ticks per second, and the seconds between the FILETIME (1601) and Unix (1970) epochs.
    const TICKS_PER_SECOND: i64 = 10_000_000;
    const FILETIME_EPOCH_TO_UNIX_SECONDS: i64 = 11_644_473_600;

    fn to_filetime(secs: i64) -> Option<FILETIME> {
        let ticks = secs
            .checked_add(FILETIME_EPOCH_TO_UNIX_SECONDS)?
            .checked_mul(TICKS_PER_SECOND)
            .and_then(|t| u64::try_from(t).ok())?;
        Some(FILETIME {
            dwLowDateTime: ticks as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        })
    }

    let utc_ft = to_filetime(utc_secs)?;
    let mut utc_st = SYSTEMTIME::default();
    unsafe { FileTimeToSystemTime(&utc_ft, &mut utc_st) }.ok()?;
    let mut local_st = SYSTEMTIME::default();
    unsafe { SystemTimeToTzSpecificLocalTime(None, &utc_st, &mut local_st) }.ok()?;
    // Reading the local wall clock back as if it were UTC turns it into "unix seconds shifted by the
    // offset", so the difference IS the offset the zone applied at that instant.
    let mut local_ft = FILETIME::default();
    unsafe { SystemTimeToFileTime(&local_st, &mut local_ft) }.ok()?;
    let local_ticks =
        (u64::from(local_ft.dwHighDateTime) << 32) | u64::from(local_ft.dwLowDateTime);
    let local_secs =
        i64::try_from(local_ticks / TICKS_PER_SECOND as u64).ok()? - FILETIME_EPOCH_TO_UNIX_SECONDS;
    Some(local_secs - utc_secs)
}

#[cfg(test)]
mod save_picker_row_slot_info_tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// SCOPE PROOF for the row Level/PlayTime rework: with no picker owning the rows -- the state
    /// the vanilla character-slot views run in, the title-screen Load Game list among them -- the
    /// gate answers `None` for every row, and `None` is the only answer the populate hook treats as
    /// "leave this row exactly as the game drew it". A regression that made the suppression or the
    /// last-saved text global would have to make this return `Some` here first.
    #[test]
    fn no_picker_means_no_row_is_ever_classified() {
        assert_eq!(
            SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst),
            0,
            "no picker session may be active in a unit test"
        );
        for row in 0..crate::experiments::save_picker::PICKER_ROW_COUNT {
            assert!(
                save_picker_row_slot_info(row).is_none(),
                "row {row} was classified without a picker owning the rows"
            );
            assert!(
                save_picker_browse_stats_lines(row).is_none(),
                "row {row} got browse stats without a picker owning the rows"
            );
        }
    }
}

/// Entry hook on the native ProfileSelect item-list builder (`PROFILE_SELECT_LIST_BUILDER_RVA`,
/// FUN_140875590): while the browse picker owns the `05_010` rows, RE-STAGE the browse-row records
/// immediately before the native builder turns ProfileSummary records into visible list rows.
///
/// Root cause of the stray current-character row (er-effects-rs-xlqh): the ProfileSummary records
/// are GAME-OWNED and volatile in-world. Every save the game performs runs the save-write path
/// `FUN_14067b940`, which calls `CS::ProfileSummary::MarkProfileIndexAsUsed(summary, saveSlot)`
/// and then `FUN_140262270(summary, saveSlot)` -- and `FUN_140262270` rewrites the ACTIVE slot's
/// record from the LIVE `mainPlayerGameData` (`wcsncpy(record.name, pgd.name, 0x10)` + level +
/// playtime + rune memory + map + face data; static RE, 1.16.2 dump). A save landing between our
/// row staging and the builder's record read left that slot's record holding the LOADED character,
/// which then rendered as a stray browse row (user report: `[ up .. ]`, <current character name>,
/// <save file name>). Rewriting the records here, on the same menu thread that immediately reads
/// them, closes that window for EVERY supported build site with one seam -- fresh dialog
/// construction and native-owned delete chains call this builder. Picker refresh code never calls
/// those delete callbacks or partial list-bind workers directly.
pub(crate) unsafe extern "system" fn save_picker_profile_list_builder_hook(
    out_list: usize,
) -> usize {
    if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0 || missing_save_selection_pending() {
        let summary = unsafe { system_quit_profile_summary_ptr() };
        if summary != TITLE_OWNER_SCAN_START_ADDRESS {
            let staged = {
                let guard = crate::experiments::save_picker::active_save_picker_lock();
                guard
                    .as_ref()
                    .map(|model| unsafe { save_picker_write_row_records(model, summary) })
            };
            if let Some(staged) = staged {
                let n = SAVE_PICKER_LIST_BUILDER_RESTAGE_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                if n <= 8 || n.is_power_of_two() {
                    append_autoload_debug(format_args!(
                        "save-picker: re-staged {staged} browse rows at native list build #{n} (game-save record-stomp guard)"
                    ));
                }
            }
        }
    }
    let orig = SAVE_PICKER_LIST_BUILDER_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        // Unreachable in practice (the trampoline is stored before the hook is enabled); mirror
        // the native return (the out-list pointer) rather than crash.
        return out_list;
    }
    let f: unsafe extern "system" fn(usize) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { f(out_list) }
}

/// Install the list-builder re-stage hook (idempotent; mirrors the row-populate install idiom).
pub(crate) fn install_save_picker_list_builder_hook() {
    if SAVE_PICKER_LIST_BUILDER_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "save-picker: list-builder MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(PROFILE_SELECT_LIST_BUILDER_RVA) else {
        append_autoload_debug(format_args!(
            "save-picker: failed to resolve list-builder rva 0x{PROFILE_SELECT_LIST_BUILDER_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            save_picker_profile_list_builder_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SAVE_PICKER_LIST_BUILDER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "save-picker: queue_enable list-builder failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SAVE_PICKER_LIST_BUILDER_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "save-picker: hooked ProfileSelect list builder FUN_140875590 0x{addr:x}; browse rows re-stage at every native list build"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "save-picker: list-builder MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "save-picker: MhHook::new list-builder failed: {status:?}"
        )),
    }
}

fn save_picker_reset_state_now(source: &str, publish_system_clear: bool) {
    let was_active = SAVE_PICKER_MODE_ACTIVE.swap(0, Ordering::SeqCst) != 0;
    let had_model = crate::experiments::save_picker::active_save_picker_lock()
        .take()
        .is_some();
    save_picker_set_reopen_pending(0);
    clear_picker_refresh_request();
    clear_path_editor_return_reopen_request();
    clear_picker_pending_resubmit_transition();
    save_picker_set_open_slots_pending(0);
    SAVE_PICKER_ACTION_OBJ.store(0, Ordering::SeqCst);
    if publish_system_clear {
        let _ = save_picker_publish_system_dialog(0);
    }
    clear_picker_presentation();
    let was_destination = SAVE_PICKER_DEST_MODE.swap(0, Ordering::SeqCst) != 0;
    if was_active || had_model {
        SAVE_PICKER_CANCEL_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker: reset (source={source}, was_active={was_active}, had_model={had_model}, destination={was_destination})"
        ));
    }
}

pub(crate) fn save_picker_apply_deferred_resubmit_reset() {
    let Some((reset_guard, action)) = claim_deferred_picker_reset_transaction() else {
        return;
    };
    unsafe {
        match action {
            PickerDeferredResetAction::PickerState { source } => {
                system_quit_apply_claimed_profile_reset(&source, &reset_guard);
            }
            PickerDeferredResetAction::RestoreRealWindows { base, source } => {
                system_quit_apply_claimed_restore_real_system_windows(base, &source, &reset_guard);
            }
        }
    }
}

pub(crate) fn save_picker_reset_source_is_applicable(source: &str) -> bool {
    if missing_save_selection_pending() {
        // STARTUP (title) picker: the model and the staged browse rows outlive any single window.
        // Backing out of the dialog returns to the no-save title menu with the rows still staged,
        // so the native Load Game row re-opens the SAME picker (and the SetState deny keeps every
        // world-entry path closed). State only clears when a save is picked.
        append_autoload_debug(format_args!(
            "save-picker: reset skipped while missing-save selection pending (source={source}); picker stays armed for native Load Game reopen"
        ));
        return false;
    }
    true
}

pub(crate) fn save_picker_reset_under_claimed_transaction(
    source: &str,
    _reset_lease: &PathEditorResetLeaseGuard,
    _reset_guard: &PickerResetTransactionGuard<'_>,
) {
    save_picker_reset_state_now(source, true);
}

// ===========================================================================
// STARTUP (TITLE) MISSING-SAVE PICKER
// ===========================================================================
//
// When the DLL attaches with no configured save and no readable default, the title boots to its
// NATIVE no-save menu (the save-data job passes through and completes empty; the SetState detour
// denies only world-entry states 4/5). Once the title main menu is interactive, this flow stages
// the browse rows into the (empty, boot-allocated) ProfileSummary and fires the native Load Game
// row -- the title's own 05_010 ProfileLoadDialog opens showing the file browser. Selection is
// routed by the SAME activate hook as the in-game picker; picking a valid save installs the
// save redirect (complete_missing_save_selection_from_picker), restores the summary, and fires
// the native return-to-title reload so the game re-reads the now-redirected save.

/// Start dir for the STARTUP overlay picker: remembered dir when valid, else the default save
/// root (`%APPDATA%\EldenRing`), else the Wine system drive root. Windows-form paths.
pub(crate) fn save_picker_title_start_dir() -> PathBuf {
    if let Some(preferred) = crate::config::preferred_save_picker_dir_now() {
        if let Some(text) = preferred.to_str() {
            let windows = PathBuf::from(save_picker_windows_path_string(text));
            if windows.is_dir() {
                return windows;
            }
        }
    }
    if let Some(root) = default_save_root()
        && let Some(text) = root.to_str()
    {
        let windows = PathBuf::from(save_picker_windows_path_string(text));
        if windows.is_dir() {
            return windows;
        }
    }
    PathBuf::from("Z:\\")
}
