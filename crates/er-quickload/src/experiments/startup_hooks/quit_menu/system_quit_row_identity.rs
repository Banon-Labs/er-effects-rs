use super::*;

// Positive row identity for the System -> Quit dialog.
//
// S7 moved the pure row-decision core to `er-quit-menu-core::rows`; this slice moved the live
// memory-capture and telemetry half to `er-quit-menu-core::row_identity`. What is left here is the
// product facade: the two reads whose offsets belong to `er_title_flow` (the dialog slot cursor and
// slot bound) are performed on this side and handed over as values, and the row-table reset stays
// because it also tears down the build-url editor, whose 02_990 field lives in the product.

pub(crate) use er_quit_menu_core::row_identity::{
    system_quit_controller_is_a_quit_row, system_quit_row_gate_instant_quit,
    system_quit_row_label_at, system_quit_row_table_index, system_quit_row_table_record_index,
};
pub(crate) use er_quit_menu_core::rows::{
    PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_STORAGE_OFFSET,
    QUIT_ROW_TABLE_ROWS as SYSTEM_QUIT_ROW_TABLE_ROWS, QuitRowVerdict,
    quit_controller_of_action_alias as system_quit_controller_of_action_alias,
    quit_row_verdict_text as system_quit_row_verdict_text,
};

/// Forget the captured row table. Called when the Quit tab starts building a dialog so a rebuilt
/// pane can never be resolved against another dialog's indices.
pub(crate) fn system_quit_row_table_reset(dialog: usize) {
    // The Quit tab is building a fresh dialog, so any link field latched against the previous one
    // is pointing at a dead `MenuJobQueue`. This is the only moment that is reliably true, which is
    // why the editor's reset hangs off the row table's rather than having a lifecycle of its own.
    reset_build_url_editor_state();
    set_build_url_row_help(er_build_import_core::BUILD_URL_ROW_HELP);
    // The export machine hangs off the same moment for the same reason: a rebuilt dialog means any
    // request latched against the previous one belongs to a row that no longer exists. Its worker,
    // if one is running, is unaffected -- it holds a document, not a dialog -- and will simply find
    // the phase already moved on.
    er_build_import_runtime::export::reset();
    set_generate_build_link_row_help(GENERATE_BUILD_LINK_ROW_HELP);
    SYSTEM_QUIT_ROW_TABLE_DIALOG.store(dialog, Ordering::SeqCst);
    SYSTEM_QUIT_ROW_INDEX_SAVE_GAME_PLUS1.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_ROW_INDEX_RETURN_DESKTOP_PLUS1.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_ROW_INDEX_LOAD_PROFILE_PLUS1.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_ROW_INDEX_LOAD_SAVE_PROFILES_PLUS1.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_ROW_INDEX_LOAD_BUILD_URL_PLUS1.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_ROW_INDEX_GENERATE_BUILD_LINK_PLUS1.store(0, Ordering::SeqCst);
}

/// Resolve which Quit row an activation belongs to, from live memory, and record the outcome.
///
/// The dialog's slot cursor is read HERE: `DIALOG_SLOT_CURSOR_B0C_OFFSET` is derived from
/// `er_title_flow`'s `ProfileLoadDialogLayout`, and handing the moved resolver the integer costs
/// less than giving the quit-menu crate a dependency on the whole title-flow crate.
pub(crate) unsafe fn system_quit_resolve_row_now(
    activation_dialog: usize,
    event: usize,
) -> QuitRowVerdict {
    let cursor = if activation_dialog >= 0x10000 {
        unsafe { safe_read_i32(activation_dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }.unwrap_or(-1)
    } else {
        -1
    };
    unsafe {
        er_quit_menu_core::row_identity::system_quit_resolve_row_now(
            activation_dialog,
            event,
            cursor,
        )
    }
}

/// Read the live `CS::GridControl` geometry of the patched Quit dialog into the navigability
/// oracles. The item count comes from `DIALOG_SLOT_BOUND_B08_OFFSET`, `er_title_flow`'s offset, so
/// it is read here for the same reason the cursor above is.
pub(crate) unsafe fn system_quit_record_grid_geometry(dialog: usize) {
    if dialog < 0x10000 {
        return;
    }
    let count = unsafe { safe_read_i32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET) }.unwrap_or(-1);
    unsafe { er_quit_menu_core::row_identity::system_quit_record_grid_geometry(dialog, count) };
}
