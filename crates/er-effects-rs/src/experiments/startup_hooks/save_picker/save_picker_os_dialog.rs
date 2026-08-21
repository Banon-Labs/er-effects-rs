//! Compatibility shim: System>Quit OS-dialog entrypoints moved to `er-quit-menu`.

use super::*;

pub(crate) use er_save_picker::os_dialog::{OsPickAbort, no_picker_cover, os_pick_validated};
pub(crate) use er_telemetry::counters::{
    SAVE_PICKER_OS_CANCEL_COUNT, SAVE_PICKER_OS_CLOSED_WITH_PATH, SAVE_PICKER_OS_ERROR_COUNT,
    SAVE_PICKER_OS_LAST_ERROR, SAVE_PICKER_OS_LAST_REJECT_REASON, SAVE_PICKER_OS_OPEN_COUNT,
    SAVE_PICKER_OS_OWNER_HWND, SAVE_PICKER_OS_OWNER_IS_COVER, SAVE_PICKER_OS_REJECT_COUNT,
    SAVE_PICKER_OS_REOPEN_COUNT, SAVE_PICKER_OS_REOPEN_EXHAUSTED, SAVE_PICKER_OS_SAVELIKE_OPENS,
};

pub(crate) unsafe fn os_open_save_picker_load(action_obj: usize) -> PickerOpenOutcome {
    match unsafe { er_quit_menu::os_open_save_picker_load(action_obj) } {
        er_quit_menu::PickerOpenOutcome::Opened => PickerOpenOutcome::Opened,
        er_quit_menu::PickerOpenOutcome::Dismissed => PickerOpenOutcome::Dismissed,
        er_quit_menu::PickerOpenOutcome::NotOpened => PickerOpenOutcome::NotOpened,
    }
}

pub(crate) unsafe fn os_open_save_dest_picker(system_dialog: usize) -> PickerOpenOutcome {
    match unsafe { er_quit_menu::os_open_save_dest_picker(system_dialog) } {
        er_quit_menu::PickerOpenOutcome::Opened => PickerOpenOutcome::Opened,
        er_quit_menu::PickerOpenOutcome::Dismissed => PickerOpenOutcome::Dismissed,
        er_quit_menu::PickerOpenOutcome::NotOpened => PickerOpenOutcome::NotOpened,
    }
}
