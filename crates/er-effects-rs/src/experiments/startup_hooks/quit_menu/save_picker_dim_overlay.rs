//! Compatibility shim: the System>Quit OS-dialog dim overlay moved to `er-quit-menu`.

#[cfg(windows)]
pub(crate) use er_quit_menu::{
    PickerDimGuard, install_picker_dim_overlay, picker_dim_arm, picker_dim_armed_cover_hwnd,
};
