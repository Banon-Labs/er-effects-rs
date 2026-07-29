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
// Which row was ACTIVATED then comes from the dialog's own list cursor -- `dialog + 0xb0c`, the
// cursor of the `GenericListSelectDialog` item-list widget embedded at `dialog + 0xa38`
// (`FUN_140739e20` returns `widget + 0xd4`; the widget's count is `widget + 0xd0 == dialog + 0xb08`,
// the field the row-clone hook raises to 4) -- and, for a mouse click, from the OS pointer band the
// cloned visuals occupy. The pointer band may only name a CLONED row or veto a quit; it can never
// authorize one, because it is guessed screen geometry rather than the game's own hit test.
//
// Anything that does not resolve positively is `Ambiguous`, and an ambiguous row NEVER quits.

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

/// Which evidence resolved the row. Recorded per activation so a run can show WHY the gate decided
/// what it decided.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum QuitRowDiscriminator {
    /// The live label at the list cursor's row is one of OUR labels, and the captured table agrees
    /// that this row index owns that label. Strongest form: exact pointer/text identity.
    CursorRowOurLabel,
    /// The live label at the list cursor's row is foreign (native FMG) AND the cursor equals the
    /// captured index of a native row. The only discriminator that may authorize the quit.
    CursorRowNativeIndex,
    /// A mouse click landed in the band the two cloned visuals occupy. Names a cloned row only.
    PointerBand,
    /// The dispatched controller is a row's OWN captured controller and that row is not the
    /// irreversible one. Deliberately never applied to Return to Desktop, because the measured
    /// dispatch collapses the two cloned buttons onto exactly that controller.
    ActivatedRowController,
}

impl QuitRowDiscriminator {
    pub(crate) fn code(self) -> usize {
        match self {
            QuitRowDiscriminator::CursorRowOurLabel => 1,
            QuitRowDiscriminator::CursorRowNativeIndex => 2,
            QuitRowDiscriminator::PointerBand => 3,
            QuitRowDiscriminator::ActivatedRowController => 4,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            QuitRowDiscriminator::CursorRowOurLabel => "cursor-row-our-label",
            QuitRowDiscriminator::CursorRowNativeIndex => "cursor-row-native-index",
            QuitRowDiscriminator::PointerBand => "pointer-band",
            QuitRowDiscriminator::ActivatedRowController => "activated-row-controller",
        }
    }
}

/// Why the row could not be identified. Every one of these refuses the quit.
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
    /// captured table says -- the table and live memory disagree, so trust neither.
    CursorRowLabelMismatch,
    /// The cursor row's label pointer could not be read at all.
    CursorRowLabelUnreadable,
    /// The cursor row's label is foreign but the cursor matches neither captured native row index.
    CursorRowUnclaimed,
    /// The event was a mouse click but the OS pointer could not be resolved, so the position that
    /// the game's own hit test used is unknown.
    MouseClickWithoutPointer,
    /// Two independent discriminators resolved, and they named DIFFERENT rows. Two sources
    /// disagreeing is an ambiguity, never a tie to break by preference: pick neither. This is the
    /// exact shape of the measured mouse defect (the pointer band claimed a cloned row while the
    /// dispatched controller said Return to Desktop), and it subsumes the older
    /// "pointer on a cloned visual vetoes a quit" special case by naming the real condition.
    DiscriminatorDisagreement,
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
            QuitRowAmbiguity::MouseClickWithoutPointer => 7,
            // 8 was `pointer-band-vetoed-quit`, now generalised into 9.
            QuitRowAmbiguity::DiscriminatorDisagreement => 9,
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
            QuitRowAmbiguity::MouseClickWithoutPointer => "mouse-click-without-pointer",
            QuitRowAmbiguity::DiscriminatorDisagreement => "discriminator-disagreement",
        }
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
    /// The activated controller mapped back onto a captured row, when it is one of the four.
    /// Only ever used to REFUSE a quit; the measured dispatch collapses rows 2/3 onto row 1's
    /// controller, so it can never be used to authorize one.
    pub(crate) activated_row_by_controller: Option<QuitRow>,
    /// Live list cursor `dialog + 0xb0c`; `-1` when unreadable.
    pub(crate) cursor: i32,
    /// Number of rows in the table (always 4 once complete); the cursor must be inside it.
    pub(crate) row_count: i32,
    /// Label read live at the cursor row; `None` when the pointer was unreadable.
    pub(crate) cursor_row_label: Option<QuitRowLabel>,
    /// How the game classified the dispatched event.
    pub(crate) input_kind: QuitInputKind,
    /// Normalised OS pointer: window-centre origin, `+x` right, `+y` down.
    pub(crate) pointer: Option<(f32, f32)>,
}

/// Normalised `y` below which the pointer is still on the top (native) button row. The two cloned
/// visuals were inserted below the native pair (`Item_0_2` / `Item_0_3`, depths 16/17 of sprite 138
/// in the runtime `02_040_optionsetting` edit), so a pointer past this line is on a cloned button.
pub(crate) const SYSTEM_QUIT_CLONE_BAND_MIN_NY: f32 = 0.12;

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

    /// The row the list cursor is sitting on, with the evidence that named it.
    fn cursor_candidate(&self) -> Result<(QuitRow, QuitRowDiscriminator), QuitRowAmbiguity> {
        if self.cursor < 0 || self.cursor >= self.row_count {
            return Err(QuitRowAmbiguity::CursorOutOfRange);
        }
        match self.cursor_row_label {
            None => Err(QuitRowAmbiguity::CursorRowLabelUnreadable),
            Some(QuitRowLabel::Ours(row)) => {
                if self.index_of(row) == self.cursor {
                    Ok((row, QuitRowDiscriminator::CursorRowOurLabel))
                } else {
                    Err(QuitRowAmbiguity::CursorRowLabelMismatch)
                }
            }
            Some(QuitRowLabel::Foreign) => {
                if self.cursor == self.return_desktop_index {
                    Ok((
                        QuitRow::ReturnToDesktop,
                        QuitRowDiscriminator::CursorRowNativeIndex,
                    ))
                } else if self.cursor == self.save_game_index {
                    Ok((QuitRow::SaveGame, QuitRowDiscriminator::CursorRowNativeIndex))
                } else {
                    Err(QuitRowAmbiguity::CursorRowUnclaimed)
                }
            }
        }
    }

    /// Why NO discriminator could name the row. The cursor's own reason, except for a mouse click
    /// whose OS pointer could not be read at all -- then the position the game's hit test used is
    /// unknown, which is the more precise fact.
    fn unresolved_reason(&self) -> QuitRowAmbiguity {
        if self.input_kind == QuitInputKind::MouseClick && self.pointer.is_none() {
            return QuitRowAmbiguity::MouseClickWithoutPointer;
        }
        self.cursor_candidate()
            .err()
            .unwrap_or(QuitRowAmbiguity::CursorOutOfRange)
    }

    /// The cloned row the OS pointer is over, if it is in the cloned band at all. Deliberately
    /// cannot return a native row: screen geometry is not the game's hit test and must never be
    /// allowed to authorize the irreversible quit.
    fn pointer_clone_candidate(&self) -> Option<QuitRow> {
        let (nx, ny) = self.pointer?;
        if ny <= SYSTEM_QUIT_CLONE_BAND_MIN_NY {
            return None;
        }
        Some(if nx < 0.0 {
            QuitRow::LoadProfile
        } else {
            QuitRow::LoadSaveProfiles
        })
    }
}

/// One independently-derived candidate row plus the evidence that named it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct QuitRowCandidate {
    pub(crate) row: QuitRow,
    pub(crate) by: QuitRowDiscriminator,
}

/// Every candidate this activation produced, strongest evidence first: the list cursor, then the
/// dispatched controller, then the OS pointer band. `None` where that source could not name a row.
pub(crate) fn quit_row_candidates(facts: &QuitRowFacts) -> [Option<QuitRowCandidate>; 3] {
    [
        facts
            .cursor_candidate()
            .ok()
            .map(|(row, by)| QuitRowCandidate { row, by }),
        facts
            .activated_row_by_controller
            .map(|row| QuitRowCandidate {
                row,
                by: QuitRowDiscriminator::ActivatedRowController,
            }),
        facts
            .pointer_clone_candidate()
            .map(|row| QuitRowCandidate {
                row,
                by: QuitRowDiscriminator::PointerBand,
            }),
    ]
}

/// Resolve which System -> Quit row an activation belongs to, using only positive evidence.
///
/// Every discriminator that can name a row is consulted, and they must AGREE. A disagreement is an
/// ambiguity -- the activation runs nothing at all -- because there is no principled way to pick a
/// winner between two sources that each claim to identify the row, and the cost of guessing wrong is
/// a menu row that performs another row's action (measured: a mouse click on Return to Desktop
/// opened the save picker because the pointer band overrode an already-correct dispatch).
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

    let candidates = quit_row_candidates(facts);
    let mut resolved = candidates.iter().flatten();
    let Some(first) = resolved.next().copied() else {
        return QuitRowVerdict::Ambiguous(facts.unresolved_reason());
    };
    if resolved.any(|other| other.row != first.row) {
        return QuitRowVerdict::Ambiguous(QuitRowAmbiguity::DiscriminatorDisagreement);
    }

    // The dispatched controller may never be the ONLY evidence for Return to Desktop: the measured
    // dispatch collapses the cloned buttons onto that controller, so on its own it cannot separate a
    // confirm on the quit row from a click on a cloned one. Everything else it names is fine.
    if first.by == QuitRowDiscriminator::ActivatedRowController
        && first.row == QuitRow::ReturnToDesktop
    {
        return QuitRowVerdict::Ambiguous(facts.unresolved_reason());
    }
    QuitRowVerdict::Resolved {
        row: first.row,
        by: first.by,
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

/// Map a dispatched controller back onto a captured row. Never used to authorize a quit: the
/// measured dispatch collapses the two cloned rows onto the second native row's controller.
pub(crate) fn system_quit_row_by_controller(controller: usize) -> Option<QuitRow> {
    if controller == 0 {
        return None;
    }
    SYSTEM_QUIT_ROW_TABLE_ROWS
        .into_iter()
        .find(|row| system_quit_row_controller(*row) == controller)
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
/// dialog captured inside the action lambda), `controller` the dispatched controller (0 when the
/// caller only has the action alias), and `event` the native event object (0 to skip input
/// classification).
pub(crate) unsafe fn system_quit_resolve_row_now(
    activation_dialog: usize,
    controller: usize,
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
        activated_row_by_controller: system_quit_row_by_controller(controller),
        cursor,
        row_count: SYSTEM_QUIT_ROW_TABLE_ROWS.len() as i32,
        cursor_row_label: unsafe { system_quit_row_label_at(activation_dialog, cursor) },
        input_kind: unsafe { system_quit_classify_activation_input(event) },
        pointer: read_cursor_normalized(),
    };
    let verdict = resolve_quit_row(&facts);
    system_quit_row_record_resolution(&facts, verdict);
    verdict
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
            match by {
                QuitRowDiscriminator::CursorRowOurLabel => {
                    SYSTEM_QUIT_ROW_RESOLVED_BY_CURSOR_OUR_LABEL_COUNT.fetch_add(1, Ordering::SeqCst)
                }
                QuitRowDiscriminator::CursorRowNativeIndex => {
                    SYSTEM_QUIT_ROW_RESOLVED_BY_CURSOR_NATIVE_INDEX_COUNT
                        .fetch_add(1, Ordering::SeqCst)
                }
                QuitRowDiscriminator::PointerBand => {
                    SYSTEM_QUIT_ROW_RESOLVED_BY_POINTER_BAND_COUNT.fetch_add(1, Ordering::SeqCst)
                }
                QuitRowDiscriminator::ActivatedRowController => {
                    SYSTEM_QUIT_ROW_RESOLVED_BY_ACTIVATED_CONTROLLER_COUNT
                        .fetch_add(1, Ordering::SeqCst)
                }
            };
        }
        QuitRowVerdict::Ambiguous(reason) => {
            SYSTEM_QUIT_ROW_LAST_RESOLVED_ROW.store(0, Ordering::SeqCst);
            SYSTEM_QUIT_ROW_LAST_DISCRIMINATOR.store(0, Ordering::SeqCst);
            SYSTEM_QUIT_ROW_LAST_AMBIGUITY.store(reason.code(), Ordering::SeqCst);
            SYSTEM_QUIT_ROW_AMBIGUOUS_COUNT.fetch_add(1, Ordering::SeqCst);
            if reason == QuitRowAmbiguity::DiscriminatorDisagreement {
                SYSTEM_QUIT_ROW_REFUSED_DISAGREEMENT_COUNT.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-row: REFUSED -- discriminators disagree, so the activation runs nothing: {}",
                    quit_row_candidates_text(facts)
                ));
            }
        }
    }
}

/// Every candidate this activation produced, for the debug log. Named per source so a disagreement
/// line shows both values rather than only the one that would have won.
pub(crate) fn quit_row_candidates_text(facts: &QuitRowFacts) -> String {
    let named = |candidate: Option<QuitRowCandidate>| match candidate {
        Some(c) => c.row.label(),
        None => "-",
    };
    let candidates = quit_row_candidates(facts);
    format!(
        "cursor={} (index={} label={:?}) controller={} pointer-band={} input_kind={:?}",
        named(candidates[0]),
        facts.cursor,
        facts.cursor_row_label,
        named(candidates[1]),
        named(candidates[2]),
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
    /// Save Game / Return to Desktop / Load Profile / Load Save Profiles.
    fn facts() -> QuitRowFacts {
        QuitRowFacts {
            save_game_index: 0,
            return_desktop_index: 1,
            load_profile_index: 2,
            load_save_profiles_index: 3,
            table_dialog: 0x1758_4208_0,
            activation_dialog: 0x1758_4208_0,
            activated_row_by_controller: Some(QuitRow::ReturnToDesktop),
            cursor: 1,
            row_count: 4,
            cursor_row_label: Some(QuitRowLabel::Foreign),
            input_kind: QuitInputKind::PadConfirm,
            pointer: None,
        }
    }

    #[test]
    fn pad_confirm_on_the_native_quit_row_authorizes_the_quit() {
        let verdict = resolve_quit_row(&facts());
        assert_eq!(
            verdict,
            QuitRowVerdict::Resolved {
                row: QuitRow::ReturnToDesktop,
                by: QuitRowDiscriminator::CursorRowNativeIndex,
            }
        );
        assert!(verdict.authorizes_quit());
    }

    #[test]
    fn pad_confirm_on_a_cloned_row_resolves_that_row_and_never_quits() {
        for (cursor, row) in [(2, QuitRow::LoadProfile), (3, QuitRow::LoadSaveProfiles)] {
            let mut f = facts();
            f.cursor = cursor;
            f.cursor_row_label = Some(QuitRowLabel::Ours(row));
            f.activated_row_by_controller = Some(row);
            let verdict = resolve_quit_row(&f);
            assert_eq!(
                verdict,
                QuitRowVerdict::Resolved {
                    row,
                    by: QuitRowDiscriminator::CursorRowOurLabel,
                }
            );
            assert!(!verdict.authorizes_quit());
        }
    }

    /// The dispatch collapse the old build measured -- a cloned row's press arriving on the native
    /// Return-to-Desktop controller -- now contradicts the cursor, so it runs nothing rather than
    /// letting either side win.
    #[test]
    fn a_collapsed_controller_that_contradicts_the_cursor_runs_nothing() {
        for (cursor, row) in [(2, QuitRow::LoadProfile), (3, QuitRow::LoadSaveProfiles)] {
            let mut f = facts();
            f.cursor = cursor;
            f.cursor_row_label = Some(QuitRowLabel::Ours(row));
            f.activated_row_by_controller = Some(QuitRow::ReturnToDesktop);
            let verdict = resolve_quit_row(&f);
            assert_eq!(
                verdict,
                QuitRowVerdict::Ambiguous(QuitRowAmbiguity::DiscriminatorDisagreement)
            );
            assert!(!verdict.authorizes_quit());
        }
    }

    /// The measured mouse defect: the dispatch carried the native Return-to-Desktop controller AND
    /// the cursor named that row, while the pointer band claimed the bottom-right cloned row. The old
    /// resolver preferred the band and opened the save picker; now the two sources disagree, so the
    /// activation runs NOTHING -- neither the picker nor the quit.
    #[test]
    fn a_pointer_band_that_contradicts_the_cursor_and_controller_runs_nothing() {
        let mut f = facts();
        f.input_kind = QuitInputKind::MouseClick;
        f.pointer = Some((0.42, 0.31));
        f.cursor = 1;
        f.cursor_row_label = Some(QuitRowLabel::Foreign);
        let verdict = resolve_quit_row(&f);
        assert_eq!(
            verdict,
            QuitRowVerdict::Ambiguous(QuitRowAmbiguity::DiscriminatorDisagreement)
        );
        assert!(!verdict.authorizes_quit());
        assert_eq!(verdict.resolved_row(), None);
    }

    /// Agreement is what resolves: the same click with the CURSOR also on the cloned row opens that
    /// row, and names the cursor (not the band) as the discriminator.
    #[test]
    fn a_mouse_click_whose_cursor_and_band_agree_resolves_by_the_cursor() {
        let mut f = facts();
        f.input_kind = QuitInputKind::MouseClick;
        f.pointer = Some((0.42, 0.31));
        f.cursor = 3;
        f.cursor_row_label = Some(QuitRowLabel::Ours(QuitRow::LoadSaveProfiles));
        f.activated_row_by_controller = Some(QuitRow::LoadSaveProfiles);
        assert_eq!(
            resolve_quit_row(&f),
            QuitRowVerdict::Resolved {
                row: QuitRow::LoadSaveProfiles,
                by: QuitRowDiscriminator::CursorRowOurLabel,
            }
        );
    }

    #[test]
    fn the_pointer_band_alone_still_names_a_cloned_row() {
        let mut f = facts();
        f.input_kind = QuitInputKind::MouseClick;
        f.pointer = Some((-0.42, 0.31));
        f.cursor = -1;
        f.activated_row_by_controller = None;
        let verdict = resolve_quit_row(&f);
        assert_eq!(verdict.resolved_row(), Some(QuitRow::LoadProfile));
        assert!(!verdict.authorizes_quit());
    }

    #[test]
    fn mouse_click_on_the_top_band_still_needs_the_cursor_to_name_the_row() {
        let mut f = facts();
        f.input_kind = QuitInputKind::MouseClick;
        f.pointer = Some((0.42, -0.30));
        assert!(resolve_quit_row(&f).authorizes_quit());

        f.cursor = -1;
        assert_eq!(
            resolve_quit_row(&f),
            QuitRowVerdict::Ambiguous(QuitRowAmbiguity::CursorOutOfRange)
        );
    }

    #[test]
    fn mouse_click_without_a_readable_pointer_is_ambiguous() {
        let mut f = facts();
        f.input_kind = QuitInputKind::MouseClick;
        f.pointer = None;
        f.cursor = -1;
        assert_eq!(
            resolve_quit_row(&f),
            QuitRowVerdict::Ambiguous(QuitRowAmbiguity::MouseClickWithoutPointer)
        );
    }

    /// The refusal holds for EVERY input kind, so an inverted input classification still cannot let a
    /// click on a cloned button reach `ExitProcess`.
    #[test]
    fn a_contradicting_pointer_refuses_the_quit_for_every_input_kind() {
        for kind in [
            QuitInputKind::Unknown,
            QuitInputKind::PadConfirm,
            QuitInputKind::MouseClick,
        ] {
            let mut f = facts();
            f.input_kind = kind;
            f.pointer = Some((0.42, 0.31));
            let verdict = resolve_quit_row(&f);
            assert!(!verdict.authorizes_quit(), "{kind:?} authorized a quit");
            assert_eq!(
                verdict,
                QuitRowVerdict::Ambiguous(QuitRowAmbiguity::DiscriminatorDisagreement),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn unknown_input_kind_with_the_pointer_on_the_top_band_uses_the_cursor() {
        let mut f = facts();
        f.input_kind = QuitInputKind::Unknown;
        f.pointer = Some((0.42, -0.31));
        assert!(resolve_quit_row(&f).authorizes_quit());
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

    #[test]
    fn a_label_that_contradicts_the_captured_index_never_quits() {
        let mut f = facts();
        f.cursor = 1;
        f.cursor_row_label = Some(QuitRowLabel::Ours(QuitRow::LoadSaveProfiles));
        assert_eq!(
            resolve_quit_row(&f),
            QuitRowVerdict::Ambiguous(QuitRowAmbiguity::CursorRowLabelMismatch)
        );
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
    fn a_foreign_label_on_a_cloned_index_never_quits() {
        let mut f = facts();
        f.cursor = 3;
        f.cursor_row_label = Some(QuitRowLabel::Foreign);
        assert_eq!(
            resolve_quit_row(&f),
            QuitRowVerdict::Ambiguous(QuitRowAmbiguity::CursorRowUnclaimed)
        );
    }

    /// A cloned row's OWN controller positively names that row when nothing else can, and can never
    /// authorize the quit.
    #[test]
    fn a_cloned_controller_names_its_own_row_and_never_quits() {
        for row in [QuitRow::LoadProfile, QuitRow::LoadSaveProfiles] {
            let mut f = facts();
            f.activated_row_by_controller = Some(row);
            f.cursor = -1;
            let verdict = resolve_quit_row(&f);
            assert_eq!(
                verdict,
                QuitRowVerdict::Resolved {
                    row,
                    by: QuitRowDiscriminator::ActivatedRowController,
                }
            );
            assert!(!verdict.authorizes_quit());
        }
    }

    /// The Return-to-Desktop controller is explicitly NOT allowed to name its row, because the
    /// measured dispatch routes clicks on both cloned buttons through it.
    #[test]
    fn the_return_desktop_controller_alone_does_not_name_the_row() {
        let mut f = facts();
        f.activated_row_by_controller = Some(QuitRow::ReturnToDesktop);
        f.cursor = -1;
        assert_eq!(
            resolve_quit_row(&f),
            QuitRowVerdict::Ambiguous(QuitRowAmbiguity::CursorOutOfRange)
        );
    }

    #[test]
    fn an_unmapped_controller_does_not_block_a_positively_identified_quit() {
        let mut f = facts();
        f.activated_row_by_controller = None;
        assert!(resolve_quit_row(&f).authorizes_quit());
    }

    #[test]
    fn the_save_game_row_is_reachable_by_both_label_forms() {
        let mut f = facts();
        f.cursor = 0;
        f.activated_row_by_controller = Some(QuitRow::SaveGame);
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
            QuitRowAmbiguity::MouseClickWithoutPointer,
            QuitRowAmbiguity::DiscriminatorDisagreement,
        ];
        for (a, first) in reasons.iter().enumerate() {
            assert_ne!(first.code(), 0);
            for second in reasons.iter().skip(a + 1) {
                assert_ne!(first.code(), second.code());
            }
        }
    }
}
