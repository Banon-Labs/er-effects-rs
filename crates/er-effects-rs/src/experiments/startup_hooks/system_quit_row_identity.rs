// POSITIVE row identity for the four-row System -> Quit dialog.
//
// # Why the previous identity was wrong
//
// The Quit-tab routing keyed every decision on an "action object" pointer read from
// `controller + PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET` (`+0xa8`). That pointer is
// **not an object of its own** -- it is a fixed-offset ALIAS of the controller:
//
// `CS::PropertyNewButtonController` is a 0x300-byte heap object (`HeapAlloc(0x300, 8, ...)` in the
// 1.16.2 dump's `FUN_14086a950`) whose constructor `FUN_14086a2a0` copy-constructs the caller's
// action `std::function` into `this + 0x70` (`param_1 + 0xe`) and stores the resulting
// `_Getter()` pointer at `this + 0xa8` (`param_1[0x15]`). MSVC's `std::function` keeps that getter
// slot at `storage + 0x38` and, for a small (inline) callable, it points at the storage itself --
// so `*(controller + 0xa8) == controller + 0x70`, always. Every logged row in the measured run
// agrees on all four rows: `0x23517880+0x70 = 0x235178f0`, `0x23517580+0x70 = 0x235175f0`,
// `0x23518180+0x70 = 0x235181f0`, `0x23517b80+0x70 = 0x23517bf0`.
//
// Therefore `action_obj == captured_action` is exactly `controller == captured_controller` wearing
// a disguise, and it carries no row information whatsoever. Worse, the four visible buttons are
// dispatched through only TWO controllers: in the measured run only the two NATIVE row controllers
// ever reached `PropertyNewButtonController::Activate` (0x23517880 with index 0, 0x23517580 with
// index 1, twice per frame), and the two cloned rows' controllers never appeared at all. So a click
// on the fourth visible button ("Load Save Profiles") arrives carrying the second native row's
// controller -- and the old gate read that as "the user confirmed Return to Desktop" and called
// `ExitProcess(0)`.
//
// # The identity used instead
//
// Each `EditProperty` row carries its own LABEL, and the label is reachable live from the dialog:
// `PropertyEditDialog.properties.items` starts at `dialog + 0x1268`, rows are `0x88` apart, and
// `EditProperty.label` at `+0x8` is a `CS::MenuHelpLabelComponent` whose first field is the
// `MenuString`'s RAW UTF-16 pointer (`CS::MenuString::MenuString` stores the pointer it is given).
// The two cloned rows are built from this DLL's own process-lifetime label arrays, so they match by
// exact POINTER equality; all four rows also match by text. That is measured, not assumed: the same
// run reported `oracle_optionsetting_active_row_count = 4` with
// `oracle_optionsetting_active_row_quit_label_mask = 15`, i.e. all four rows' labels were readable
// and each matched one of the four known Quit labels, on the very dialog (`0x175842080`) the fatal
// click came from. `cloned_mask = 12` and `native_save_mask = 1` pin the order: row 0 Save Game,
// row 1 Return to Desktop, row 2 Load Profile, row 3 Load Save Profiles.
//
// Which row was ACTIVATED comes from ONE source for all three input kinds: the dialog's own list
// cursor, `dialog + 0xb0c` -- field `+0xd4` of the `CS::GridControl` embedded at `dialog + 0xa38`
// (`FUN_140739e20` returns it; the widget's item count is `+0xd0 == dialog + 0xb08`).
//
// # Why the cursor is authoritative for the mouse too
//
// `GridControl::Update` (vtable `+0x10`, 1.16.2 `FUN_1407392f0`) is the single writer of that field:
//
//   * MOUSE -- `GridControl::HandleMouse` (`FUN_14073a5c0`) ends by asking, when the cursor is live
//     (`FUN_140758050`: `CSMenuManImp::disableMouseCursor == false`) and the mouse is the active
//     pointer (`FUN_1407588a0`: `CSMouseMan + 0x30`) and no drag/wheel/direction input is in flight,
//     `FUN_140736c90(this, FUN_140757af0(pad))`. That hit-tests each of the `cols * rows` grid CELL
//     proxies via `FUN_14074b0d0`, converts the hit cell's `(row, col)` into an item index, and calls
//     `FUN_14073bc10(this, index)` -- which writes `+0xd4`. Hover moves the native cursor.
//   * PAD / KEYBOARD -- the direction branches of the same `Update` (`FUN_14073b0c0` /
//     `FUN_14073b4d0` / `FUN_14073a250`) land on the same `FUN_14073bc10`, and `Update` then diffs
//     `+0xd4` against its pre-update value to fire the selection-changed callbacks.
//
// There is no separate focus field. So once the two rows this DLL adds are real grid cells, the
// cursor identifies the row for mouse, keyboard and pad alike -- see `er_gfx::options_02_040` for the
// movie-side half (the cells are named `Item_1_0`/`Item_1_1` so the grid measures 2x2, which is what
// makes them hit-testable AND puts the vertical axis in play).
//
// The cursor's two halves are cross-checked and must agree: the captured build-time row TABLE
// (index -> row) and the LIVE label read at that index. A mismatch, an unreadable label, an
// out-of-range cursor or a stale dialog are all `Ambiguous`, and an ambiguous row NEVER quits and
// never runs anything.

/// The four rows of the patched System -> Quit dialog, in property-list order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum QuitRow {
    /// Native first row, relabelled "Save Game" by the `MsgRepository::GetAndFormat` hook.
    SaveGame,
    /// Native second row: the real Return to Desktop. The only row allowed to quit.
    ReturnToDesktop,
    /// Cloned row: opens the native `05_010_ProfileSelect` character picker.
    LoadProfile,
    /// Cloned row: opens the in-game save-container picker.
    LoadSaveProfiles,
}

impl QuitRow {
    /// Telemetry code (`0` is reserved for "no row").
    pub(crate) fn code(self) -> usize {
        match self {
            QuitRow::SaveGame => 1,
            QuitRow::ReturnToDesktop => 2,
            QuitRow::LoadProfile => 3,
            QuitRow::LoadSaveProfiles => 4,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            QuitRow::SaveGame => "Save Game",
            QuitRow::ReturnToDesktop => "Return to Desktop",
            QuitRow::LoadProfile => "Load Profile",
            QuitRow::LoadSaveProfiles => "Load Save Profiles",
        }
    }
}

/// What the label at a given row index turned out to be.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum QuitRowLabel {
    /// One of the three labels this DLL owns (pointer- or text-matched).
    Ours(QuitRow),
    /// A readable label that is none of ours -- i.e. a native FMG string. Locale independent: we
    /// never require the English "Return to Desktop" text to authorize a quit.
    Foreign,
}

/// How the activation arrived, as classified by the game's OWN predicates on the dispatched event
/// (`FUN_140758a10` = pad/keyboard confirm, `FUN_140758a70` = mouse click; both are the tests
/// `PropertyNewButtonController`'s should-invoke predicate `FUN_140974b00` itself runs).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum QuitInputKind {
    PadConfirm,
    MouseClick,
    /// Neither predicate answered (unresolvable RVA, or an event the game classifies as neither).
    Unknown,
}

impl QuitInputKind {
    pub(crate) fn code(self) -> usize {
        match self {
            QuitInputKind::Unknown => 0,
            QuitInputKind::PadConfirm => 1,
            QuitInputKind::MouseClick => 2,
        }
    }
}

/// Which evidence resolved the row. Exactly ONE variant on purpose: the dialog's own list cursor is
/// the single row identity for mouse, keyboard and pad, so every resolution reports the same
/// discriminator regardless of input kind (`oracle_system_quit_row_last_discriminator == 1`).
/// Adding a second variant means a second identity source was reintroduced -- which is the defect
/// this type exists to prevent, not an extension point.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum QuitRowDiscriminator {
    /// The row at the live list cursor, cross-checked between the captured build-time row table and
    /// the label read live at that index.
    CursorRow,
}

impl QuitRowDiscriminator {
    pub(crate) fn code(self) -> usize {
        match self {
            QuitRowDiscriminator::CursorRow => 1,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            QuitRowDiscriminator::CursorRow => "cursor-row",
        }
    }
}

/// Why the row could not be identified. Every one of these refuses the quit AND runs nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum QuitRowAmbiguity {
    /// One or more of the four row indices was never captured, or two captured the same index.
    RowTableIncomplete,
    /// The activation's dialog is not the dialog the table was captured from (a rebuilt Quit pane,
    /// or a heap address reused after the old dialog died).
    DialogMismatch,
    /// `dialog + 0xb0c` was unreadable or outside the row table.
    CursorOutOfRange,
    /// The label read live at the cursor row is one of ours but sits at a different index than the
    /// captured table says -- the table and live memory DISAGREE, so trust neither.
    CursorRowLabelMismatch,
    /// The cursor row's label pointer could not be read at all.
    CursorRowLabelUnreadable,
    /// The cursor row's label is foreign but the cursor matches neither captured native row index --
    /// again the table and the live label DISAGREE about what sits at that index.
    CursorRowUnclaimed,
}

impl QuitRowAmbiguity {
    pub(crate) fn code(self) -> usize {
        match self {
            QuitRowAmbiguity::RowTableIncomplete => 1,
            QuitRowAmbiguity::DialogMismatch => 2,
            QuitRowAmbiguity::CursorOutOfRange => 3,
            QuitRowAmbiguity::CursorRowLabelMismatch => 4,
            QuitRowAmbiguity::CursorRowLabelUnreadable => 5,
            QuitRowAmbiguity::CursorRowUnclaimed => 6,
            // 7 was `mouse-click-without-pointer` and 8 `pointer-band-vetoed-quit`; both belonged to
            // the deleted per-input-kind discriminators. 9 was the generic disagreement, which is now
            // codes 4/6 (the only two sources left are the cursor's own halves).
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            QuitRowAmbiguity::RowTableIncomplete => "row-table-incomplete",
            QuitRowAmbiguity::DialogMismatch => "dialog-mismatch",
            QuitRowAmbiguity::CursorOutOfRange => "cursor-out-of-range",
            QuitRowAmbiguity::CursorRowLabelMismatch => "cursor-row-label-mismatch",
            QuitRowAmbiguity::CursorRowLabelUnreadable => "cursor-row-label-unreadable",
            QuitRowAmbiguity::CursorRowUnclaimed => "cursor-row-unclaimed",
        }
    }

    /// `true` when the refusal is the two halves of the cursor identity CONTRADICTING each other --
    /// the captured build-time row table versus the label read live at that index. This is the
    /// refusal-on-disagreement backstop; the other reasons are simply absence of evidence.
    pub(crate) fn is_disagreement(self) -> bool {
        matches!(
            self,
            QuitRowAmbiguity::CursorRowLabelMismatch | QuitRowAmbiguity::CursorRowUnclaimed
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum QuitRowVerdict {
    Resolved {
        row: QuitRow,
        by: QuitRowDiscriminator,
    },
    Ambiguous(QuitRowAmbiguity),
}

impl QuitRowVerdict {
    pub(crate) fn resolved_row(self) -> Option<QuitRow> {
        match self {
            QuitRowVerdict::Resolved { row, .. } => Some(row),
            QuitRowVerdict::Ambiguous(_) => None,
        }
    }

    /// `true` only for a POSITIVELY identified Return-to-Desktop row. Everything else -- including
    /// every ambiguity -- is false, so the irreversible instant `ExitProcess(0)` can never run on
    /// absence of evidence.
    pub(crate) fn authorizes_quit(self) -> bool {
        matches!(
            self,
            QuitRowVerdict::Resolved {
                row: QuitRow::ReturnToDesktop,
                ..
            }
        )
    }
}

/// Everything the resolver needs, as plain data. No memory reads happen in here, which is what
/// makes the decision unit-testable on the host/wine target.
#[derive(Clone, Copy, Debug)]
pub(crate) struct QuitRowFacts {
    /// Captured property-list index per row; `-1` means "never captured".
    pub(crate) save_game_index: i32,
    pub(crate) return_desktop_index: i32,
    pub(crate) load_profile_index: i32,
    pub(crate) load_save_profiles_index: i32,
    /// The dialog the table above was captured from, and the dialog this activation belongs to.
    pub(crate) table_dialog: usize,
    pub(crate) activation_dialog: usize,
    /// Live list cursor `dialog + 0xb0c` (`GridControl + 0xd4`); `-1` when unreadable. THE row
    /// identity: the native grid writes it from mouse hover, keyboard and pad alike.
    pub(crate) cursor: i32,
    /// Number of rows in the table (always 4 once complete); the cursor must be inside it.
    pub(crate) row_count: i32,
    /// Label read live at the cursor row; `None` when the pointer was unreadable.
    pub(crate) cursor_row_label: Option<QuitRowLabel>,
    /// How the game classified the dispatched event. RECORDED, never branched on -- it is the
    /// evidence that one discriminator serves every input kind, not an input to the decision.
    pub(crate) input_kind: QuitInputKind,
}

impl QuitRowFacts {
    fn index_of(&self, row: QuitRow) -> i32 {
        match row {
            QuitRow::SaveGame => self.save_game_index,
            QuitRow::ReturnToDesktop => self.return_desktop_index,
            QuitRow::LoadProfile => self.load_profile_index,
            QuitRow::LoadSaveProfiles => self.load_save_profiles_index,
        }
    }

    fn table_complete_and_distinct(&self) -> bool {
        let idx = [
            self.save_game_index,
            self.return_desktop_index,
            self.load_profile_index,
            self.load_save_profiles_index,
        ];
        if idx.iter().any(|i| *i < 0 || *i >= self.row_count) {
            return false;
        }
        for (a, first) in idx.iter().enumerate() {
            for second in idx.iter().skip(a + 1) {
                if first == second {
                    return false;
                }
            }
        }
        true
    }

    /// The row the list cursor is sitting on. Both halves of the identity must agree: the captured
    /// build-time row TABLE (index -> row) and the LABEL read live at that index.
    fn cursor_candidate(&self) -> Result<QuitRow, QuitRowAmbiguity> {
        if self.cursor < 0 || self.cursor >= self.row_count {
            return Err(QuitRowAmbiguity::CursorOutOfRange);
        }
        match self.cursor_row_label {
            None => Err(QuitRowAmbiguity::CursorRowLabelUnreadable),
            Some(QuitRowLabel::Ours(row)) => {
                if self.index_of(row) == self.cursor {
                    Ok(row)
                } else {
                    Err(QuitRowAmbiguity::CursorRowLabelMismatch)
                }
            }
            // A native FMG label -- the table's native indices are the only thing that can claim it.
            // Locale independent: no English text is ever required to authorize the quit.
            Some(QuitRowLabel::Foreign) => {
                if self.cursor == self.return_desktop_index {
                    Ok(QuitRow::ReturnToDesktop)
                } else if self.cursor == self.save_game_index {
                    Ok(QuitRow::SaveGame)
                } else {
                    Err(QuitRowAmbiguity::CursorRowUnclaimed)
                }
            }
        }
    }
}

/// Resolve which System -> Quit row an activation belongs to, from the ONE identity the native grid
/// maintains for every input kind: its list cursor.
///
/// There is deliberately no per-input-kind branch, no screen-geometry rectangle and no controller /
/// action-object comparison. Those were three parallel interpretations beside the game's own model,
/// and preferring one over another is how a mouse click on Return to Desktop came to open the save
/// picker. The cursor's two halves (captured row table, live label) must still agree; anything else
/// is `Ambiguous`, which runs nothing and never quits.
pub(crate) fn resolve_quit_row(facts: &QuitRowFacts) -> QuitRowVerdict {
    if !facts.table_complete_and_distinct() {
        return QuitRowVerdict::Ambiguous(QuitRowAmbiguity::RowTableIncomplete);
    }
    if facts.table_dialog == 0
        || facts.activation_dialog == 0
        || facts.table_dialog != facts.activation_dialog
    {
        return QuitRowVerdict::Ambiguous(QuitRowAmbiguity::DialogMismatch);
    }
    match facts.cursor_candidate() {
        Ok(row) => QuitRowVerdict::Resolved {
            row,
            by: QuitRowDiscriminator::CursorRow,
        },
        Err(reason) => QuitRowVerdict::Ambiguous(reason),
    }
}

// ---------------------------------------------------------------------------------------------
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

fn system_quit_row_table_index(row: QuitRow) -> i32 {
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
    if plus1 == 0 || plus1 > i32::MAX as usize {
        -1
    } else {
        (plus1 - 1) as i32
    }
}

/// The captured `PropertyNewButtonController` of a row, or 0 when it was never captured.
fn system_quit_row_controller(row: QuitRow) -> usize {
    match row {
        QuitRow::SaveGame => SYSTEM_QUIT_NATIVE_SAVE_GAME_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst),
        QuitRow::ReturnToDesktop => {
            SYSTEM_QUIT_NATIVE_RETURN_DESKTOP_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst)
        }
        QuitRow::LoadProfile => SYSTEM_QUIT_LOAD_PROFILE_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst),
        QuitRow::LoadSaveProfiles => {
            SYSTEM_QUIT_OPEN_SAVE_DIR_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst)
        }
    }
}

pub(crate) const SYSTEM_QUIT_ROW_TABLE_ROWS: [QuitRow; 4] = [
    QuitRow::SaveGame,
    QuitRow::ReturnToDesktop,
    QuitRow::LoadProfile,
    QuitRow::LoadSaveProfiles,
];

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

/// The `std::function` storage inside a controller that the action thunks receive as their `this`.
/// `*(controller + 0xa8) == controller + 0x70` for a small callable, so this is the SAME value the
/// old `*_ACTION_LAST_OBJECT` latches held -- named for what it is.
pub(crate) const PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_STORAGE_OFFSET: usize = 0x70;

/// Recover the controller an action thunk's `this` pointer aliases. Pure pointer arithmetic: the
/// action "object" is `controller + 0x70`, never an independent allocation.
pub(crate) fn system_quit_controller_of_action_alias(action_obj: usize) -> usize {
    action_obj.saturating_sub(PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_STORAGE_OFFSET)
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
    let count = unsafe { safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET) }?;
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
    // Longest label first: "Load Save Profiles" starts with neither of the others, but "Load
    // Profile" would also prefix-match a hypothetical longer string, so keep the order explicit.
    if wide_ptr_starts_with_ascii(label_ptr, b"Load Save Profiles") {
        return Some(QuitRowLabel::Ours(QuitRow::LoadSaveProfiles));
    }
    if wide_ptr_starts_with_ascii(label_ptr, b"Load Profile") {
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
unsafe fn system_quit_classify_activation_input(event: usize) -> QuitInputKind {
    if event == 0 {
        return QuitInputKind::Unknown;
    }
    let pad = game_rva(MENU_VIEWER_PAD_CONFIRM_PRESSED_RVA).ok();
    let mouse = game_rva(MENU_VIEWER_PAD_MOUSE_CLICKED_RVA).ok();
    if let Some(addr) = pad {
        let predicate: unsafe extern "system" fn(usize) -> u8 =
            unsafe { std::mem::transmute(addr) };
        if unsafe { predicate(event) } != 0 {
            return QuitInputKind::PadConfirm;
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
    let facts = QuitRowFacts {
        save_game_index: system_quit_row_table_index(QuitRow::SaveGame),
        return_desktop_index: system_quit_row_table_index(QuitRow::ReturnToDesktop),
        load_profile_index: system_quit_row_table_index(QuitRow::LoadProfile),
        load_save_profiles_index: system_quit_row_table_index(QuitRow::LoadSaveProfiles),
        table_dialog: SYSTEM_QUIT_ROW_TABLE_DIALOG.load(Ordering::SeqCst),
        activation_dialog,
        cursor,
        row_count: SYSTEM_QUIT_ROW_TABLE_ROWS.len() as i32,
        cursor_row_label: unsafe { system_quit_row_label_at(activation_dialog, cursor) },
        input_kind: unsafe { system_quit_classify_activation_input(event) },
    };
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

fn system_quit_row_record_resolution(facts: &QuitRowFacts, verdict: QuitRowVerdict) {
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

/// Both halves of the cursor identity, for the debug log: what the captured table says sits at each
/// index, and what the label read live at the cursor actually is.
pub(crate) fn quit_row_facts_text(facts: &QuitRowFacts) -> String {
    format!(
        "cursor={} table=[save_game=#{} return_desktop=#{} load_profile=#{} load_save_profiles=#{}] live_label={:?} input_kind={:?}",
        facts.cursor,
        facts.save_game_index,
        facts.return_desktop_index,
        facts.load_profile_index,
        facts.load_save_profiles_index,
        facts.cursor_row_label,
        facts.input_kind,
    )
}

/// One-line description of a verdict for the debug log.
pub(crate) fn system_quit_row_verdict_text(verdict: QuitRowVerdict) -> String {
    match verdict {
        QuitRowVerdict::Resolved { row, by } => {
            format!("row='{}' by={}", row.label(), by.label())
        }
        QuitRowVerdict::Ambiguous(reason) => format!("row=AMBIGUOUS reason={}", reason.label()),
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
    if matches!(
        verdict.resolved_row(),
        Some(QuitRow::LoadProfile) | Some(QuitRow::LoadSaveProfiles)
    ) {
        SYSTEM_QUIT_ACTION_ALIAS_FALSE_QUIT_CLAIMS.fetch_add(1, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "quit-to-desktop: REFUSED the instant ExitProcess at {site} -- {}; the action object is only controller+0x{PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_STORAGE_OFFSET:x}, so it cannot name a row, and no positive Return-to-Desktop evidence was found",
        system_quit_row_verdict_text(verdict)
    ));
    false
}

#[cfg(test)]
mod system_quit_row_identity_tests {
    use super::*;

    /// The measured table from the fatal run: dialog 0x175842080, rows 0..3 =
    /// Save Game / Return to Desktop / Load Profile / Load Save Profiles, cursor parked on row 1.
    fn facts() -> QuitRowFacts {
        QuitRowFacts {
            save_game_index: 0,
            return_desktop_index: 1,
            load_profile_index: 2,
            load_save_profiles_index: 3,
            table_dialog: 0x1758_4208_0,
            activation_dialog: 0x1758_4208_0,
            cursor: 1,
            row_count: 4,
            cursor_row_label: Some(QuitRowLabel::Foreign),
            input_kind: QuitInputKind::PadConfirm,
        }
    }

    #[test]
    fn the_cursor_on_the_native_quit_row_authorizes_the_quit() {
        let verdict = resolve_quit_row(&facts());
        assert_eq!(
            verdict,
            QuitRowVerdict::Resolved {
                row: QuitRow::ReturnToDesktop,
                by: QuitRowDiscriminator::CursorRow,
            }
        );
        assert!(verdict.authorizes_quit());
    }

    /// The acceptance property: the SAME cursor resolves the SAME row with the SAME discriminator no
    /// matter which input kind the game classified the event as. Nothing branches on input kind, so a
    /// mouse click on the row under the pointer and a pad confirm on the focused row are one path.
    #[test]
    fn every_input_kind_resolves_the_same_row_by_the_same_discriminator() {
        for kind in [
            QuitInputKind::Unknown,
            QuitInputKind::PadConfirm,
            QuitInputKind::MouseClick,
        ] {
            for (cursor, row, label) in [
                (0, QuitRow::SaveGame, Some(QuitRowLabel::Foreign)),
                (1, QuitRow::ReturnToDesktop, Some(QuitRowLabel::Foreign)),
                (
                    2,
                    QuitRow::LoadProfile,
                    Some(QuitRowLabel::Ours(QuitRow::LoadProfile)),
                ),
                (
                    3,
                    QuitRow::LoadSaveProfiles,
                    Some(QuitRowLabel::Ours(QuitRow::LoadSaveProfiles)),
                ),
            ] {
                let mut f = facts();
                f.input_kind = kind;
                f.cursor = cursor;
                f.cursor_row_label = label;
                assert_eq!(
                    resolve_quit_row(&f),
                    QuitRowVerdict::Resolved {
                        row,
                        by: QuitRowDiscriminator::CursorRow,
                    },
                    "{kind:?} cursor={cursor}"
                );
            }
        }
    }

    /// Only the Return-to-Desktop row may quit; the other three are resolved and harmless.
    #[test]
    fn no_row_but_return_to_desktop_can_quit() {
        for (cursor, label) in [
            (0, Some(QuitRowLabel::Foreign)),
            (2, Some(QuitRowLabel::Ours(QuitRow::LoadProfile))),
            (3, Some(QuitRowLabel::Ours(QuitRow::LoadSaveProfiles))),
        ] {
            let mut f = facts();
            f.cursor = cursor;
            f.cursor_row_label = label;
            let verdict = resolve_quit_row(&f);
            assert!(verdict.resolved_row().is_some(), "cursor={cursor}");
            assert!(!verdict.authorizes_quit(), "cursor={cursor} authorized a quit");
        }
    }

    #[test]
    fn an_out_of_range_cursor_runs_nothing() {
        for cursor in [-1, 4, 99] {
            let mut f = facts();
            f.cursor = cursor;
            assert_eq!(
                resolve_quit_row(&f),
                QuitRowVerdict::Ambiguous(QuitRowAmbiguity::CursorOutOfRange),
                "cursor={cursor}"
            );
        }
    }

    #[test]
    fn a_stale_row_table_from_another_dialog_never_quits() {
        let mut f = facts();
        f.activation_dialog = 0x1758_4308_0;
        assert_eq!(
            resolve_quit_row(&f),
            QuitRowVerdict::Ambiguous(QuitRowAmbiguity::DialogMismatch)
        );

        let mut f = facts();
        f.table_dialog = 0;
        assert_eq!(
            resolve_quit_row(&f),
            QuitRowVerdict::Ambiguous(QuitRowAmbiguity::DialogMismatch)
        );
    }

    #[test]
    fn an_incomplete_or_colliding_row_table_never_quits() {
        let mut f = facts();
        f.load_save_profiles_index = -1;
        assert_eq!(
            resolve_quit_row(&f),
            QuitRowVerdict::Ambiguous(QuitRowAmbiguity::RowTableIncomplete)
        );

        let mut f = facts();
        f.load_profile_index = 1;
        assert_eq!(
            resolve_quit_row(&f),
            QuitRowVerdict::Ambiguous(QuitRowAmbiguity::RowTableIncomplete)
        );

        let mut f = facts();
        f.return_desktop_index = 9;
        assert_eq!(
            resolve_quit_row(&f),
            QuitRowVerdict::Ambiguous(QuitRowAmbiguity::RowTableIncomplete)
        );
    }

    /// The surviving refusal-on-disagreement backstop: the captured table and the label read live at
    /// the cursor contradict each other, so neither is trusted.
    #[test]
    fn the_table_and_the_live_label_disagreeing_runs_nothing() {
        let mut f = facts();
        f.cursor = 1;
        f.cursor_row_label = Some(QuitRowLabel::Ours(QuitRow::LoadSaveProfiles));
        let verdict = resolve_quit_row(&f);
        assert_eq!(
            verdict,
            QuitRowVerdict::Ambiguous(QuitRowAmbiguity::CursorRowLabelMismatch)
        );
        assert!(!verdict.authorizes_quit());

        // A native FMG label at an index the table says is one of ours: the same contradiction seen
        // from the other side.
        let mut f = facts();
        f.cursor = 3;
        f.cursor_row_label = Some(QuitRowLabel::Foreign);
        assert_eq!(
            resolve_quit_row(&f),
            QuitRowVerdict::Ambiguous(QuitRowAmbiguity::CursorRowUnclaimed)
        );
    }

    #[test]
    fn both_disagreement_reasons_are_counted_as_disagreements() {
        assert!(QuitRowAmbiguity::CursorRowLabelMismatch.is_disagreement());
        assert!(QuitRowAmbiguity::CursorRowUnclaimed.is_disagreement());
        for absence in [
            QuitRowAmbiguity::RowTableIncomplete,
            QuitRowAmbiguity::DialogMismatch,
            QuitRowAmbiguity::CursorOutOfRange,
            QuitRowAmbiguity::CursorRowLabelUnreadable,
        ] {
            assert!(!absence.is_disagreement(), "{absence:?}");
        }
    }

    #[test]
    fn an_unreadable_cursor_row_label_never_quits() {
        let mut f = facts();
        f.cursor_row_label = None;
        assert_eq!(
            resolve_quit_row(&f),
            QuitRowVerdict::Ambiguous(QuitRowAmbiguity::CursorRowLabelUnreadable)
        );
    }

    #[test]
    fn the_save_game_row_is_reachable_by_both_label_forms() {
        let mut f = facts();
        f.cursor = 0;
        f.cursor_row_label = Some(QuitRowLabel::Ours(QuitRow::SaveGame));
        assert_eq!(resolve_quit_row(&f).resolved_row(), Some(QuitRow::SaveGame));

        // The Save Game label goes through MsgRepository::Format, so its MenuString may hold an
        // engine buffer rather than our pointer; the captured native index still names the row.
        f.cursor_row_label = Some(QuitRowLabel::Foreign);
        assert_eq!(resolve_quit_row(&f).resolved_row(), Some(QuitRow::SaveGame));
        assert!(!resolve_quit_row(&f).authorizes_quit());
    }

    #[test]
    fn telemetry_codes_are_distinct_and_nonzero() {
        let rows = [
            QuitRow::SaveGame,
            QuitRow::ReturnToDesktop,
            QuitRow::LoadProfile,
            QuitRow::LoadSaveProfiles,
        ];
        for (a, first) in rows.iter().enumerate() {
            assert_ne!(first.code(), 0);
            for second in rows.iter().skip(a + 1) {
                assert_ne!(first.code(), second.code());
            }
        }
        let reasons = [
            QuitRowAmbiguity::RowTableIncomplete,
            QuitRowAmbiguity::DialogMismatch,
            QuitRowAmbiguity::CursorOutOfRange,
            QuitRowAmbiguity::CursorRowLabelMismatch,
            QuitRowAmbiguity::CursorRowLabelUnreadable,
            QuitRowAmbiguity::CursorRowUnclaimed,
        ];
        for (a, first) in reasons.iter().enumerate() {
            assert_ne!(first.code(), 0);
            for second in reasons.iter().skip(a + 1) {
                assert_ne!(first.code(), second.code());
            }
        }
        // A resolved verdict must be distinguishable from an ambiguous one in the oracle, and there
        // is exactly ONE discriminator: `oracle_system_quit_row_last_discriminator` reads 1 for every
        // resolution, whatever the input kind.
        assert_eq!(QuitRowDiscriminator::CursorRow.code(), 1);
    }
}
