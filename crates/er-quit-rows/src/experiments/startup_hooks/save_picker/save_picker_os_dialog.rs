//! Compatibility shim: System>Quit OS-dialog entrypoints moved to `er-quit-menu-core`.

use super::*;

pub(crate) unsafe fn os_open_save_picker_load(action_obj: usize) -> PickerOpenOutcome {
    match unsafe { er_quit_menu_core::os_open_save_picker_load(action_obj) } {
        er_quit_menu_core::PickerOpenOutcome::Opened => PickerOpenOutcome::Opened,
        er_quit_menu_core::PickerOpenOutcome::Dismissed => PickerOpenOutcome::Dismissed,
        er_quit_menu_core::PickerOpenOutcome::NotOpened => PickerOpenOutcome::NotOpened,
    }
}

pub(crate) unsafe fn os_open_save_dest_picker(system_dialog: usize) -> PickerOpenOutcome {
    match unsafe { er_quit_menu_core::os_open_save_dest_picker(system_dialog) } {
        er_quit_menu_core::PickerOpenOutcome::Opened => PickerOpenOutcome::Opened,
        er_quit_menu_core::PickerOpenOutcome::Dismissed => PickerOpenOutcome::Dismissed,
        er_quit_menu_core::PickerOpenOutcome::NotOpened => PickerOpenOutcome::NotOpened,
    }
}
