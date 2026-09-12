//! The in-game save-file picker, now owned by `er-quit-menu-core`.
//!
//! The 3,178 lines that stood here moved to `er_quit_menu_core::save_picker_menu` so the
//! System>Quit **Load Character from File** row can ship in a standalone shell. This crate keeps
//! every call site it had, through the re-export below, and supplies the steps only a host with a
//! save-swap ledger, a save-flow and a live-layout editor can perform.

use super::*;

pub(crate) use er_quit_menu_core::save_picker_menu::*;

/// The passive device read the picker's browse pumps need, backed by this DLL's `InputBlocker`.
///
/// A standalone shell installs none of this and the picker still browses -- it simply never sees a
/// held direction, so the edge-scroll and the drive strip answer only to activations.
fn install_nav_input_hooks() {
    crate::experiments::ensure_save_picker_user_nav_input_hooks_installed();
}

fn take_nav_edges_for(mask: usize) -> usize {
    crate::experiments::save_picker_take_user_nav_edges_for(mask)
}

fn nav_held() -> usize {
    crate::experiments::save_picker_user_nav_held()
}

/// The live editor's font/align hot-reload, wrapped around a row text push.
///
/// `push_stats_text_on_row` hands the text it was given when this is not installed, which is what a
/// shell with no editor wants.
fn live_text_for_field(field_name_nul: &str, text: &[u16]) -> Vec<u16> {
    crate::experiments::startup_hooks::loading_cover::profile_editor_live_text_for_field(
        field_name_nul,
        text,
    )
    .into_owned()
}

fn remember_field_target(
    field_name_nul: &str,
    component: usize,
    utf16: &[u16],
    active_surface: &'static str,
) {
    crate::experiments::startup_hooks::remember_profile_editor_field_target(
        field_name_nul,
        component,
        utf16,
        active_surface,
    );
}

/// The product's own steps around a pick. Installed once, from the quit-menu arm.
pub(crate) fn install_product_save_picker_hooks() {
    er_quit_menu_core::save_picker_menu::install_hooks(
        er_quit_menu_core::save_picker_menu::SavePickerMenuHooks {
            preferred_save_picker_dir_now: Some(crate::config::preferred_save_picker_dir_now),
            restore_staged_row_records: Some(save_picker_restore_staged_row_records),
            save_flow_box_set_host_dialog: Some(save_flow_box_set_host_dialog),
            save_flow_submit_box: Some(save_flow_submit_box),
            profile_editor_field_font_height: Some(profile_editor_field_font_height),
            open_picker_for_intent: Some(open_picker_for_intent),
            ensure_nav_input_hooks: Some(install_nav_input_hooks),
            take_nav_edges_for: Some(take_nav_edges_for),
            nav_held: Some(nav_held),
        },
    );
    er_quit_menu_core::scaleform_proxy::install_row_text_hooks(
        er_quit_menu_core::scaleform_proxy::RowTextHooks {
            live_text_for_field: Some(live_text_for_field),
            remember_field_target: Some(remember_field_target),
        },
    );
}
