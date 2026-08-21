use super::*;

// POSITIVE row identity for the four-row System -> Quit dialog.
//
// S7 moved the pure row-decision core to `er-quit-menu::rows`. This product-side shim keeps the
// live memory capture/telemetry functions in the root DLL until the hooked surfaces move in S8.

pub(crate) use er_quit_menu::rows::{
    PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_STORAGE_OFFSET,
    QUIT_ROW_TABLE_ROWS as SYSTEM_QUIT_ROW_TABLE_ROWS, QuitInputKind, QuitRow, QuitRowFacts,
    QuitRowLabel, QuitRowTable, QuitRowVerdict,
    quit_controller_of_action_alias as system_quit_controller_of_action_alias, quit_row_facts_text,
    quit_row_index_from_plus1, quit_row_is_false_quit_claim,
    quit_row_verdict_text as system_quit_row_verdict_text, resolve_quit_row,
};

// Live side: capture the row table at build time, read the facts at activation time, and record
// what happened. Everything below reads game memory; the decision itself stays in the pure
// resolver above.
// ---------------------------------------------------------------------------------------------

/// Forget the captured row table. Called when the Quit tab starts building a dialog so a rebuilt
/// pane can never be resolved against another dialog's indices.
pub(crate) fn system_quit_row_table_reset(dialog: usize) {
    SYSTEM_QUIT_ROW_TABLE_DIALOG.store(dialog, Ordering::SeqCst);
    SYSTEM_QUIT_ROW_INDEX_SAVE_GAME_PLUS1.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_ROW_INDEX_RETURN_DESKTOP_PLUS1.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_ROW_INDEX_LOAD_PROFILE_PLUS1.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_ROW_INDEX_LOAD_SAVE_PROFILES_PLUS1.store(0, Ordering::SeqCst);
}

/// Record the property-list index a row landed at. `index` is the row's slot in
/// `PropertyEditDialog.properties.items`, i.e. `count - 1` right after the row was pushed.
pub(crate) fn system_quit_row_table_record_index(row: QuitRow, index: usize) {
    let plus1 = index.saturating_add(1);
    match row {
        QuitRow::SaveGame => SYSTEM_QUIT_ROW_INDEX_SAVE_GAME_PLUS1.store(plus1, Ordering::SeqCst),
        QuitRow::ReturnToDesktop => {
            SYSTEM_QUIT_ROW_INDEX_RETURN_DESKTOP_PLUS1.store(plus1, Ordering::SeqCst)
        }
        QuitRow::LoadProfile => {
            SYSTEM_QUIT_ROW_INDEX_LOAD_PROFILE_PLUS1.store(plus1, Ordering::SeqCst)
        }
        QuitRow::LoadSaveProfiles => {
            SYSTEM_QUIT_ROW_INDEX_LOAD_SAVE_PROFILES_PLUS1.store(plus1, Ordering::SeqCst)
        }
    }
}

pub(crate) fn system_quit_row_table_index(row: QuitRow) -> i32 {
    let plus1 = match row {
        QuitRow::SaveGame => SYSTEM_QUIT_ROW_INDEX_SAVE_GAME_PLUS1.load(Ordering::SeqCst),
        QuitRow::ReturnToDesktop => {
            SYSTEM_QUIT_ROW_INDEX_RETURN_DESKTOP_PLUS1.load(Ordering::SeqCst)
        }
        QuitRow::LoadProfile => SYSTEM_QUIT_ROW_INDEX_LOAD_PROFILE_PLUS1.load(Ordering::SeqCst),
        QuitRow::LoadSaveProfiles => {
            SYSTEM_QUIT_ROW_INDEX_LOAD_SAVE_PROFILES_PLUS1.load(Ordering::SeqCst)
        }
    };
    quit_row_index_from_plus1(plus1)
}

/// The captured `PropertyNewButtonController` of a row, or 0 when it was never captured.
pub(crate) fn system_quit_row_controller(row: QuitRow) -> usize {
    match row {
        QuitRow::SaveGame => {
            SYSTEM_QUIT_NATIVE_SAVE_GAME_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst)
        }
        QuitRow::ReturnToDesktop => {
            SYSTEM_QUIT_NATIVE_RETURN_DESKTOP_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst)
        }
        QuitRow::LoadProfile => {
            SYSTEM_QUIT_LOAD_PROFILE_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst)
        }
        QuitRow::LoadSaveProfiles => {
            SYSTEM_QUIT_OPEN_SAVE_DIR_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst)
        }
    }
}

/// Is this dispatched controller one of the patched Quit tab's four? A pure SCOPE test: the
/// activation hook shares its `_Func_impl` thunk vtable and `Activate` slot with other dialogs, so it
/// must forward foreign controllers untouched.
///
/// It deliberately returns a bool rather than a row. The dispatch collapses the cloned buttons onto
/// the native Return-to-Desktop controller, so a controller cannot name a row -- and returning
/// `Option<QuitRow>` invited exactly that misuse.
pub(crate) fn system_quit_controller_is_a_quit_row(controller: usize) -> bool {
    controller != 0
        && SYSTEM_QUIT_ROW_TABLE_ROWS
            .into_iter()
            .any(|row| system_quit_row_controller(row) == controller)
}

/// Read the label of one property row, live from the dialog. `EditProperty.label`
/// (`row + 0x8`) is a `CS::MenuHelpLabelComponent` whose first field is the `MenuString`'s raw
/// UTF-16 pointer, so the two cloned rows match this DLL's own static arrays by POINTER, and all
/// four rows also match by text.
pub(crate) unsafe fn system_quit_row_label_at(dialog: usize, index: i32) -> Option<QuitRowLabel> {
    const HEAP_LO: usize = 0x10000;
    if dialog < HEAP_LO || index < 0 {
        return None;
    }
    let count =
        unsafe { safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET) }?;
    if count == 0 || index as usize >= count.min(16) {
        return None;
    }
    let row = dialog
        + PROPERTY_EDIT_DIALOG_PROPERTIES_1268_OFFSET
        + EDIT_PROPERTY_SIZE.saturating_mul(index as usize);
    let label_ptr = unsafe { safe_read_usize(row + EDIT_PROPERTY_LABEL_OFFSET) }?;
    if label_ptr < HEAP_LO {
        return None;
    }
    if label_ptr == SYSTEM_QUIT_LOAD_SAVE_PROFILES_LABEL_W.as_ptr() as usize {
        return Some(QuitRowLabel::Ours(QuitRow::LoadSaveProfiles));
    }
    if label_ptr == SYSTEM_QUIT_LOAD_PROFILE_LABEL_W.as_ptr() as usize {
        return Some(QuitRowLabel::Ours(QuitRow::LoadProfile));
    }
    if label_ptr == SYSTEM_QUIT_SAVE_GAME_LABEL_W.as_ptr() as usize {
        return Some(QuitRowLabel::Ours(QuitRow::SaveGame));
    }
    // Confirm the pointer is a readable UTF-16 string before classifying it as foreign, so an
    // unmapped/garbage pointer reports `None` (ambiguous) rather than "native label".
    unsafe { safe_read_u16(label_ptr) }?;
    // LONGEST LABEL FIRST, AND SINCE 2026-07-31 THAT IS LOAD-BEARING RATHER THAN TIDY.
    // "Load Character from File" STARTS WITH "Load Character", so a prefix test in the other order
    // classifies the file-browse row as the character row -- and this function's answer decides
    // which row a click ran. The old pair ("Load Save Profiles" / "Load Profile") did not overlap,
    // so the ordering was free then and is not now. Any future label must be checked against this.
    if wide_ptr_starts_with_ascii(label_ptr, b"Load Character from File") {
        return Some(QuitRowLabel::Ours(QuitRow::LoadSaveProfiles));
    }
    if wide_ptr_starts_with_ascii(label_ptr, b"Load Character") {
        return Some(QuitRowLabel::Ours(QuitRow::LoadProfile));
    }
    if wide_ptr_starts_with_ascii(label_ptr, b"Save Game") {
        return Some(QuitRowLabel::Ours(QuitRow::SaveGame));
    }
    Some(QuitRowLabel::Foreign)
}

/// Classify a dispatched activation event with the game's own predicates -- the same two tests
/// `PropertyNewButtonController`'s should-invoke predicate (`FUN_140974b00`) runs. The pad predicate
/// short-circuits with no positional test; the mouse predicate is the one whose result the native
/// code then hit-tests against a display object.
pub(crate) unsafe fn system_quit_classify_activation_input(event: usize) -> QuitInputKind {
    if event == 0 {
        return QuitInputKind::Unknown;
    }
    let pad = game_rva(MENU_VIEWER_PAD_CONFIRM_PRESSED_RVA).ok();
    let mouse = game_rva(MENU_VIEWER_PAD_MOUSE_CLICKED_RVA).ok();
    if let Some(addr) = pad {
        let predicate: unsafe extern "system" fn(usize) -> u8 =
            unsafe { std::mem::transmute(addr) };
        if unsafe { predicate(event) } != 0 {
            return QuitInputKind::Confirm;
        }
    }
    if let Some(addr) = mouse {
        let predicate: unsafe extern "system" fn(usize) -> u8 =
            unsafe { std::mem::transmute(addr) };
        if unsafe { predicate(event) } != 0 {
            return QuitInputKind::MouseClick;
        }
    }
    QuitInputKind::Unknown
}

/// Resolve which Quit row an activation belongs to, from live memory, and record the outcome.
///
/// `activation_dialog` is the dialog the activation reached us with (`action_obj + 0x8`, i.e. the
/// dialog captured inside the action lambda) and `event` the native event object (0 to skip input
/// classification, which is telemetry only).
pub(crate) unsafe fn system_quit_resolve_row_now(
    activation_dialog: usize,
    event: usize,
) -> QuitRowVerdict {
    let cursor = if activation_dialog >= 0x10000 {
        unsafe { safe_read_i32(activation_dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }.unwrap_or(-1)
    } else {
        -1
    };
    let table = QuitRowTable {
        save_game_index: system_quit_row_table_index(QuitRow::SaveGame),
        return_desktop_index: system_quit_row_table_index(QuitRow::ReturnToDesktop),
        load_profile_index: system_quit_row_table_index(QuitRow::LoadProfile),
        load_save_profiles_index: system_quit_row_table_index(QuitRow::LoadSaveProfiles),
    };
    let facts = QuitRowFacts::from_table(
        table,
        SYSTEM_QUIT_ROW_TABLE_DIALOG.load(Ordering::SeqCst),
        activation_dialog,
        cursor,
        unsafe { system_quit_row_label_at(activation_dialog, cursor) },
        unsafe { system_quit_classify_activation_input(event) },
    );
    let verdict = resolve_quit_row(&facts);
    system_quit_row_record_resolution(&facts, verdict);
    verdict
}

/// Read the live `CS::GridControl` geometry of the patched Quit dialog into the navigability oracles.
/// Called right after the rows are appended, so a run can show whether all four rows are reachable
/// without inspecting the movie: `NAVIGABLE_CELLS = cols * rows` is the exact bound of the native
/// mouse hit-test loop, `ROWS >= 2` is what enables up/down, and `ITEM_COUNT` is the cursor bound.
pub(crate) unsafe fn system_quit_record_grid_geometry(dialog: usize) {
    if dialog < 0x10000 {
        return;
    }
    let grid = dialog + DIALOG_GRID_CONTROL_A38_OFFSET;
    let cols = unsafe { safe_read_i32(grid + GRID_CONTROL_COLS_D8_OFFSET) }.unwrap_or(-1);
    let rows = unsafe { safe_read_i32(grid + GRID_CONTROL_ROWS_DC_OFFSET) }.unwrap_or(-1);
    let count = unsafe { safe_read_i32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET) }.unwrap_or(-1);
    let nonneg = |v: i32| if v < 0 { 0 } else { v as usize };
    SYSTEM_QUIT_GRID_COLS.store(nonneg(cols), Ordering::SeqCst);
    SYSTEM_QUIT_GRID_ROWS.store(nonneg(rows), Ordering::SeqCst);
    SYSTEM_QUIT_GRID_NAVIGABLE_CELLS.store(nonneg(cols) * nonneg(rows), Ordering::SeqCst);
    SYSTEM_QUIT_GRID_ITEM_COUNT.store(nonneg(count), Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: Quit grid geometry dialog=0x{dialog:x} cols={cols} rows={rows} navigable_cells={} item_count={count}; up/down needs rows>=2, the mouse hit test walks cols*rows cells",
        nonneg(cols) * nonneg(rows)
    ));
}

pub(crate) fn system_quit_row_record_resolution(facts: &QuitRowFacts, verdict: QuitRowVerdict) {
    SYSTEM_QUIT_ROW_RESOLVE_COUNT.fetch_add(1, Ordering::SeqCst);
    SYSTEM_QUIT_ROW_LAST_INPUT_KIND.store(facts.input_kind.code(), Ordering::SeqCst);
    SYSTEM_QUIT_ROW_LAST_CURSOR_PLUS1.store(
        if facts.cursor < 0 {
            0
        } else {
            facts.cursor as usize + 1
        },
        Ordering::SeqCst,
    );
    SYSTEM_QUIT_ROW_LAST_CURSOR_LABEL_KIND.store(
        match facts.cursor_row_label {
            None => 0,
            Some(QuitRowLabel::Foreign) => 5,
            Some(QuitRowLabel::Ours(row)) => row.code(),
        },
        Ordering::SeqCst,
    );
    match verdict {
        QuitRowVerdict::Resolved { row, by } => {
            SYSTEM_QUIT_ROW_LAST_RESOLVED_ROW.store(row.code(), Ordering::SeqCst);
            SYSTEM_QUIT_ROW_LAST_DISCRIMINATOR.store(by.code(), Ordering::SeqCst);
            SYSTEM_QUIT_ROW_LAST_AMBIGUITY.store(0, Ordering::SeqCst);
            SYSTEM_QUIT_ROW_RESOLVED_BY_CURSOR_ROW_COUNT.fetch_add(1, Ordering::SeqCst);
        }
        QuitRowVerdict::Ambiguous(reason) => {
            SYSTEM_QUIT_ROW_LAST_RESOLVED_ROW.store(0, Ordering::SeqCst);
            SYSTEM_QUIT_ROW_LAST_DISCRIMINATOR.store(0, Ordering::SeqCst);
            SYSTEM_QUIT_ROW_LAST_AMBIGUITY.store(reason.code(), Ordering::SeqCst);
            SYSTEM_QUIT_ROW_AMBIGUOUS_COUNT.fetch_add(1, Ordering::SeqCst);
            if reason.is_disagreement() {
                SYSTEM_QUIT_ROW_REFUSED_DISAGREEMENT_COUNT.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-row: REFUSED -- the row table and the live label disagree, so the activation runs nothing: {}",
                    quit_row_facts_text(facts)
                ));
            }
        }
    }
}

/// The single gate for the irreversible instant `ExitProcess(0)`. Returns `true` only on POSITIVE
/// evidence that the activated row is the Return-to-Desktop row; every refusal is counted so a run
/// shows the gate working instead of merely not crashing. Takes an already-resolved verdict so one
/// activation produces exactly one resolution in the oracles.
pub(crate) fn system_quit_row_gate_instant_quit(verdict: QuitRowVerdict, site: &str) -> bool {
    if verdict.authorizes_quit() {
        SYSTEM_QUIT_QUIT_AUTHORIZED_COUNT.fetch_add(1, Ordering::SeqCst);
        return true;
    }
    SYSTEM_QUIT_QUIT_REFUSED_AMBIGUOUS_ROW_COUNT.fetch_add(1, Ordering::SeqCst);
    if quit_row_is_false_quit_claim(verdict) {
        SYSTEM_QUIT_ACTION_ALIAS_FALSE_QUIT_CLAIMS.fetch_add(1, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "quit-to-desktop: REFUSED the instant ExitProcess at {site} -- {}; the action object is only controller+0x{PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_STORAGE_OFFSET:x}, so it cannot name a row, and no positive Return-to-Desktop evidence was found",
        system_quit_row_verdict_text(verdict)
    ));
    false
}
