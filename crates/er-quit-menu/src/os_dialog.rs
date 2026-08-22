//! System>Quit OS common-file-dialog entrypoints.
//!
//! The reusable comdlg32 mechanism lives in `er-save-picker`; these entrypoints belong to
//! product (B), because they stage System>Quit rows/save-flow state and own the dim cover.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use er_save_picker::os_dialog::{OsPickAbort, os_pick_validated};
use er_save_picker::{PickerCover, PickerIntent};
use er_telemetry::counters::{
    SAVE_DEST_COMMIT_COUNT, SAVE_DEST_COMMIT_PENDING, SAVE_DEST_CONFIRM_PENDING,
    SAVE_DEST_OVERWRITE_UNCONFIRMABLE_COUNT, SAVE_DEST_PICKER_OPEN_COUNT,
    SAVE_DEST_TARGET_EXISTING_COUNT, SAVE_DEST_TARGET_NEW_COUNT, SAVE_PICKER_ACTION_OBJ,
    SAVE_PICKER_CANCEL_COUNT, SAVE_PICKER_OPEN_COUNT, SAVE_PICKER_OPEN_SLOTS_PENDING,
    SAVE_PICKER_PICK_COUNT, SAVE_PICKER_PICK_REJECT_COUNT, SAVE_PICKER_SYSTEM_DIALOG,
    SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT,
};

use crate::{
    SaveDestOrigin, append_autoload_debug, save_dest_set_target, save_dest_start_dir,
    save_flow_box_clear, save_flow_box_recipe_available, save_picker_seamless_mode_after_settle,
    save_picker_start_dir, system_dialog_from_action_obj, system_quit_env_save_path,
    system_quit_ingest_picked_save, system_quit_save_swap_arm_original,
    system_quit_save_swap_restore_profile_summary, system_quit_windows_path_for_log,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerOpenOutcome {
    Opened,
    Dismissed,
    NotOpened,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DestRoute {
    ConfirmOverwrite,
    CommitDirect,
}

fn save_dest_route_picked_target(target: &Path) -> DestRoute {
    if target.is_file() {
        DestRoute::ConfirmOverwrite
    } else {
        DestRoute::CommitDirect
    }
}

pub fn picker_dim_cover_factory(label: &str) -> Option<PickerCover> {
    let guard = crate::picker_dim_arm(label)?;
    let owner_hwnd = crate::picker_dim_armed_cover_hwnd().0 as usize;
    Some(PickerCover::new(owner_hwnd, Box::new(guard)))
}

/// OS-mode LOAD source: pick a save container, then hand it to the unchanged in-game ingest
/// pipeline and stage the slot view exactly as the in-game pick does.
///
/// `SAVE_PICKER_SYSTEM_DIALOG` is stored from the row's action object BEFORE the dialog opens, the
/// same way the in-game arm does it: the menu-pump resubmit reopens `05_010` through that dialog and
/// abandons the reopen if it is 0. `SYSTEM_QUIT_PROFILE_SELECT_WINDOW` is already 0 in OS mode
/// (there is no picker window), so the resubmit's precondition holds with no window to close.
///
/// # Safety
///
/// `action_obj` must be a live `CS::MenuJob` action object for the row press that is calling
/// this, and the call must be on the game thread that owns it: the body reads through it
/// (`system_dialog_from_action_obj`) and then blocks in comdlg32, so the object has to stay
/// alive for the read. Pass 0 rather than a stale pointer -- a 0 is checked for, a freed
/// object is not.
pub unsafe fn os_open_save_picker_load(action_obj: usize) -> PickerOpenOutcome {
    let save_path = match system_quit_env_save_path() {
        Ok(path) => path,
        Err(reason) => {
            SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!("save-picker-os: refused to open -- {reason}"));
            return PickerOpenOutcome::NotOpened;
        }
    };
    unsafe { system_quit_save_swap_restore_profile_summary("save-picker-os-reopen") };
    if !system_quit_save_swap_arm_original(&save_path) {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        return PickerOpenOutcome::NotOpened;
    }
    let Some(start_dir) = save_picker_start_dir().and_then(|dir| dir.to_str().map(str::to_owned))
    else {
        SYSTEM_QUIT_OPEN_SAVE_DIR_FAILURE_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "save-picker-os: refused to open -- no readable start directory (preferred/save-dir/default-root all unavailable)"
        ));
        return PickerOpenOutcome::NotOpened;
    };
    // Same flavor filter and the same source of it as the in-game picker and the ingest pipeline.
    let seamless = save_picker_seamless_mode_after_settle("system-quit-os-picker-open");
    let extensions: &[&str] = if seamless { &["co2", "sl2"] } else { &["sl2"] };
    // Store the owning System dialog BEFORE the blocking call: the menu-pump resubmit needs it, and
    // after the dialog returns the action object may no longer be the identity that survives.
    SAVE_PICKER_ACTION_OBJ.store(action_obj, Ordering::SeqCst);
    SAVE_PICKER_SYSTEM_DIALOG.store(
        unsafe { system_dialog_from_action_obj(action_obj) },
        Ordering::SeqCst,
    );
    SAVE_PICKER_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    // THE ONE COLLAPSE THIS ARM IS ALLOWED: the System>Quit load surface treats a user's Cancel and
    // an unusable comdlg32 identically, because both leave the System menu alone and the row press
    // that asked for the dialog is spent either way. It does NOT collapse either of them into
    // `NotOpened` -- that collapse is the reopen loop (bd `er-effects-rs-rsxi`).
    let staged = os_pick_validated(
        false,
        start_dir,
        "",
        extensions,
        &PickerIntent::LoadSource,
        picker_dim_cover_factory,
        |picked| {
            // The SECOND gate, unchanged: BND4 parse, SteamID normalization, ProfileSummary
            // preview, candidate staging, picked-dir memory. The predicate above only added the
            // listing gate the dialog bypassed; nothing here is weakened.
            if !unsafe { system_quit_ingest_picked_save(picked) } {
                SAVE_PICKER_PICK_REJECT_COUNT.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "save-picker-os: ingest refused '{}' after the listing predicate accepted it; nothing staged",
                    system_quit_windows_path_for_log(picked)
                ));
                return false;
            }
            SAVE_PICKER_PICK_COUNT.fetch_add(1, Ordering::SeqCst);
            // Hand off to the SAME menu-pump resubmit the in-game pick uses, which reopens `05_010`
            // as the normal slot view. Contract 5: the slot view is always ours.
            SAVE_PICKER_OPEN_SLOTS_PENDING.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "save-picker-os: accepted '{}'; slot view staged for the menu pump (dialog=0x{:x})",
                system_quit_windows_path_for_log(picked),
                SAVE_PICKER_SYSTEM_DIALOG.load(Ordering::SeqCst)
            ));
            true
        },
    );
    match staged {
        Ok(true) => PickerOpenOutcome::Opened,
        // Nothing staged, the System menu untouched. Restore the preview we armed above so the
        // user's real rows are what the System UI shows. There is no retry latch on this surface --
        // the row press IS the request -- so a cancel here simply leaves the user standing on the
        // System>Quit rows, which is the Back semantics the save-destination surface now matches.
        other => {
            unsafe { system_quit_save_swap_restore_profile_summary("save-picker-os-no-pick") };
            match other {
                // No dialog ever appeared, so this is NOT a user decision. It used to be counted as
                // a cancel (every `None` was); now that the two are distinguishable, counting a
                // refusal as a user's Cancel is just a telemetry lie.
                Err(OsPickAbort::NotOpened) => PickerOpenOutcome::NotOpened,
                // A dialog RAN and produced nothing. `SAVE_PICKER_CANCEL_COUNT` is the
                // surface-agnostic "a picker was abandoned" counter, and both halves belong in it;
                // WHICH half it was is already separated one layer down, by
                // `SAVE_PICKER_OS_CANCEL_COUNT` and `SAVE_PICKER_OS_ERROR_COUNT`. Nothing on this
                // surface acts on the difference -- only the boot arm does.
                Err(OsPickAbort::Cancelled | OsPickAbort::Failed) => {
                    SAVE_PICKER_CANCEL_COUNT.fetch_add(1, Ordering::SeqCst);
                    PickerOpenOutcome::Dismissed
                }
                // The ingest refused a path the listing predicate had accepted: a dialog RAN and
                // came back, so the request is discharged -- but that is OUR refusal, not the
                // user's, and `SAVE_PICKER_PICK_REJECT_COUNT` already counted it.
                Ok(_) => PickerOpenOutcome::Dismissed,
            }
        }
    }
}

/// OS-mode SAVE DESTINATION: the Save-As dialog IS the destination browser.
///
/// Menu-pump owned, exactly like the in-game destination open: called from
/// `system_quit_menu_window_run_post` after the save-flow tick stages
/// `SAVE_DEST_OPEN_PICKER_PENDING`.
///
/// Three things this deliberately does NOT do, each of which would be a bug:
///
///  * it never calls `save_picker_native_close`. There is no picker window in OS mode, and handing
///    the System dialog to that helper would dispatch the MenuWindow cancel-close vfunc on a
///    `PropertyEditDialog` -- the exact mistake `system_quit_dialog_handlers` warns about. It stages
///    `SAVE_DEST_COMMIT_PENDING` and stops; stage 3 then sees no picker window and proceeds.
///  * it never writes `SAVE_FLOW_STAGE`. The in-game arm does, from the menu thread, bypassing
///    `save_flow_enter_stage` -- a filed defect (bd `er-effects-rs-8tq4` item 15). Adding a second
///    instance of a known defect is not "keeping the modes symmetric". It sets
///    `SAVE_DEST_CONFIRM_PENDING` and the TICK performs the transition.
///  * it stages nothing at all on cancel. Dropping the dialog claim with no latch set is precisely
///    what stage 3 reads as "the user abandoned the save". That was TRUE of the latches and FALSE
///    of the request: the menu pump's `SAVE_DEST_OPEN_PICKER_PENDING` stayed armed through the
///    cancel and reopened the dialog on the next pump, ~57 ms later, forever (bd
///    `er-effects-rs-rsxi`). The cancel path still needs no latch of its own -- what it needs is to
///    say `Dismissed` rather than "the open failed", which is what discharges that request.
///
/// # Safety
///
/// `system_dialog` must be a live `CS::SystemDialog` owned by the game thread this runs on;
/// the staging path reads through it to build the confirm. The heap-likeness test at the top
/// rejects an obviously bogus value, but it cannot tell a freed dialog from a live one.
pub unsafe fn os_open_save_dest_picker(system_dialog: usize) -> PickerOpenOutcome {
    const HEAP_LO: usize = 0x10000;
    if system_dialog < HEAP_LO || system_dialog == usize::MIN {
        append_autoload_debug(format_args!(
            "save-picker-os: refused save-as -- System dialog=0x{system_dialog:x} is not heap-like"
        ));
        return PickerOpenOutcome::NotOpened;
    }
    // The SAME start-dir/leaf resolution the in-game destination browser uses, so the two modes
    // cannot open in different places.
    let Some(SaveDestOrigin {
        start_dir,
        loaded_file_name,
        loaded_path,
    }) = save_dest_start_dir()
    else {
        return PickerOpenOutcome::NotOpened;
    };
    let Some(start_dir) = start_dir.to_str().map(str::to_owned) else {
        append_autoload_debug(format_args!(
            "save-picker-os: refused save-as -- the loaded save's folder is not representable as text"
        ));
        return PickerOpenOutcome::NotOpened;
    };
    let seamless = save_picker_seamless_mode_after_settle("system-quit-os-save-dest-picker-open");
    let extensions: &[&str] = if seamless { &["co2", "sl2"] } else { &["sl2"] };
    let intent = PickerIntent::SaveDestination {
        loaded_file_name: loaded_file_name.clone(),
        loaded_path: loaded_path.clone(),
    };
    SAVE_DEST_PICKER_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_PICKER_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    // Identical reasoning to the load arm: a cancelled and an unusable destination browser both
    // mean "nothing staged", which is what the save-flow tick already reads as the user abandoning
    // the save. `NotOpened` stays separate, because the menu pump's `SAVE_DEST_OPEN_PICKER_PENDING`
    // is a real retry latch and that is the one outcome allowed to keep it armed.
    let staged = os_pick_validated(
        true,
        start_dir,
        &loaded_file_name,
        extensions,
        &intent,
        picker_dim_cover_factory,
        |picked| {
            let target = PathBuf::from(picked);
            // The SAME mode-free routing decision the in-game browser's activation makes, so the
            // overwrite gate cannot differ between surfaces.
            match save_dest_route_picked_target(&target) {
                DestRoute::ConfirmOverwrite => {
                    SAVE_DEST_TARGET_EXISTING_COUNT.fetch_add(1, Ordering::SeqCst);
                    // Same refusal as the in-game arm: with no buildable confirm there is no
                    // overwrite. Nothing is staged, so stage 3 reads this as an abandoned browse
                    // and ends the flow without writing.
                    if !save_flow_box_recipe_available() {
                        SAVE_DEST_OVERWRITE_UNCONFIRMABLE_COUNT.fetch_add(1, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "save-picker-os: REFUSED to overwrite '{}' -- the overwrite confirm cannot be built on this build, and an unconfirmed overwrite is not something this flow performs. Nothing staged",
                            system_quit_windows_path_for_log(picked)
                        ));
                        return;
                    }
                    save_dest_set_target(target, "os-save-as");
                    SAVE_DEST_CONFIRM_PENDING.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "save-picker-os: save-as chose the existing '{}'; the overwrite confirm is owed to the tick",
                        system_quit_windows_path_for_log(picked)
                    ));
                }
                DestRoute::CommitDirect => {
                    SAVE_DEST_TARGET_NEW_COUNT.fetch_add(1, Ordering::SeqCst);
                    save_dest_set_target(target, "os-save-as");
                    SAVE_DEST_COMMIT_COUNT.fetch_add(1, Ordering::SeqCst);
                    save_flow_box_clear();
                    SAVE_DEST_COMMIT_PENDING.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "save-picker-os: save-as named the new '{}'; commit staged, no window to close",
                        system_quit_windows_path_for_log(picked)
                    ));
                }
            }
        },
    );
    match staged {
        Ok(()) => PickerOpenOutcome::Opened,
        Err(abort @ (OsPickAbort::Cancelled | OsPickAbort::Failed)) => {
            append_autoload_debug(format_args!(
                "save-picker-os: save-as closed without choosing ({abort:?}); nothing staged and the menu pump's open request is DISCHARGED (no reopen) -- the save-flow tick will end the flow with nothing written"
            ));
            PickerOpenOutcome::Dismissed
        }
        Err(OsPickAbort::NotOpened) => PickerOpenOutcome::NotOpened,
    }
}
