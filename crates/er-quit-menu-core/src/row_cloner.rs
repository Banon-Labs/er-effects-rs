//! The AddCancelButton row cloner, the row router it feeds, and the one entry point that arms them.
//!
//! Moved out of `er-quickload`'s `quit_menu/system_quit_dialog_handlers.rs`, which keeps the Save
//! Game flow and the ProfileLoad dialog opener -- the rows this file routes *to* rather than the
//! machinery that creates and identifies them. The split is deliberate: the two build rows need the
//! cloner and the router with no product DLL behind them, while the other four rows drive flows
//! only a product has.
//!
//! # How the other four rows still work
//!
//! Through [`QuitRowActions`], a table of function pointers the arm call supplies. The product
//! passes its real flows; a standalone shell passes none and arms only the rows it clones, so a
//! press that could reach a flow it does not have never happens -- the row is not on the tab.
//!
//! # Which rows exist
//!
//! [`RowSet`] decides, and it is what makes a shell-only profile legal. The grid the rows are cells
//! of is six cells (`er_gfx::options_02_040::quit6`), the native pair occupy the first two, and each
//! cloned row lands at the next free index. A shell arming only the two build rows puts them at
//! indices 2 and 3; the product arming all four puts them at 2, 3, 4 and 5. Either way the row
//! table records where each row actually landed, and identity is that index plus the live label --
//! never a pointer, which the engine aliases across rows.

use std::sync::atomic::{AtomicUsize, Ordering};

use er_game_base::game_build::seamless_coop_loaded;
use er_game_base::mem::{game_rva, game_rva_for_hook, safe_read_i32, safe_read_usize};
use er_game_base::stack::callstack_contains_game_rva;
use er_telemetry_core::counters::{
    OPTIONSETTING_ACTIVELY_SHOWN, OPTIONSETTING_CURRENT_DIALOG, OPTIONSETTING_CURRENT_TAB,
    PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_INSTALLED, SAVE_FLOW_STAGE, SAVE_PICKER_MODE_ACTIVE,
    SYSTEM_QUIT_DUPLICATE_COUNT, SYSTEM_QUIT_DUPLICATE_LAST_COUNT_AFTER,
    SYSTEM_QUIT_DUPLICATE_LAST_COUNT_BEFORE, SYSTEM_QUIT_GENERATE_BUILD_LINK_ACTION_LAST_OBJECT,
    SYSTEM_QUIT_GENERATE_BUILD_LINK_CONTROLLER_LAST_OBJECT,
    SYSTEM_QUIT_LOAD_BUILD_URL_ACTION_LAST_OBJECT,
    SYSTEM_QUIT_LOAD_BUILD_URL_CONTROLLER_LAST_OBJECT,
    SYSTEM_QUIT_LOAD_PROFILE_CONTROLLER_LAST_OBJECT,
    SYSTEM_QUIT_NATIVE_RETURN_DESKTOP_CONTROLLER_LAST_OBJECT,
    SYSTEM_QUIT_NATIVE_SAVE_GAME_ACTION_LAST_OBJECT,
    SYSTEM_QUIT_NATIVE_SAVE_GAME_CONTROLLER_LAST_OBJECT, SYSTEM_QUIT_NOOP_ACTION_INSTALLED,
    SYSTEM_QUIT_NOOP_ACTION_LAST_OBJECT, SYSTEM_QUIT_NOOP_SELECTION_COUNT,
    SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_COUNT, SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_LAST_OBJECT,
    SYSTEM_QUIT_OPEN_SAVE_DIR_CONTROLLER_LAST_OBJECT, SYSTEM_QUIT_PROFILE_LOAD_FLOW_ACTIVE,
    SYSTEM_QUIT_PROFILE_SELECT_WINDOW, SYSTEM_QUIT_QUICKLOAD_PHASE,
    SYSTEM_QUIT_QUIT_REFUSED_AMBIGUOUS_ROW_COUNT, SYSTEM_QUIT_RETURN_DESKTOP_ACTION_INSTALLED,
    SYSTEM_QUIT_ROW_INDEX_GENERATE_BUILD_LINK_PLUS1, SYSTEM_QUIT_ROW_INDEX_LOAD_BUILD_URL_PLUS1,
    SYSTEM_QUIT_ROW_INDEX_LOAD_PROFILE_PLUS1, SYSTEM_QUIT_ROW_INDEX_LOAD_SAVE_PROFILES_PLUS1,
    SYSTEM_QUIT_ROW_INDEX_RETURN_DESKTOP_PLUS1, SYSTEM_QUIT_ROW_INDEX_SAVE_GAME_PLUS1,
    SYSTEM_QUIT_ROW_TABLE_DIALOG, SYSTEM_QUIT_SAVE_GAME_ACTION_COUNT,
    SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG,
};
use er_title_flow::SYSTEM_QUIT_DUPLICATE_ORIG;
use er_title_flow::{
    PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_ORIG,
    SYSTEM_QUIT_NATIVE_RETURN_DESKTOP_ACTION_LAST_OBJECT, SYSTEM_QUIT_NOOP_ACTION_ORIG,
    SYSTEM_QUIT_RETURN_DESKTOP_ACTION_ORIG,
};

use crate::build_url_editor::reset_build_url_editor_state;
use crate::build_url_row::{system_quit_log_build_import_press, system_quit_start_build_import};
use crate::generate_build_link_row::{
    system_quit_log_build_export_press, system_quit_start_build_export,
};
use crate::host::{append_autoload_debug, release_input_block_now};
use crate::quit_dialog_layout::{
    DIALOG_GRID_CONTROL_A38_OFFSET, EDIT_PROPERTY_CONTROLLER_OFFSET, EDIT_PROPERTY_SIZE,
    PROPERTY_EDIT_DIALOG_PROPERTIES_1268_OFFSET, PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET,
};
use crate::row_identity::{
    system_quit_controller_is_a_quit_row, system_quit_row_gate_instant_quit,
    system_quit_row_label_at, system_quit_row_table_index, system_quit_row_table_record_index,
};
use crate::row_text::{
    GENERATE_BUILD_LINK_ROW_HELP, SYSTEM_QUIT_GENERATE_BUILD_LINK_LABEL_W,
    SYSTEM_QUIT_LOAD_BUILD_URL_LABEL_W, SYSTEM_QUIT_LOAD_PROFILE_HELP_W,
    SYSTEM_QUIT_LOAD_PROFILE_LABEL_W, SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_CO2_W,
    SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_W, SYSTEM_QUIT_LOAD_SAVE_PROFILES_LABEL_W,
    SYSTEM_QUIT_ROW_TEXT_CAPACITY, build_url_row_help_wide, generate_build_link_row_help_wide,
    set_build_url_row_help, set_generate_build_link_row_help,
};
use crate::rows::{
    NativeRowAction, PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_STORAGE_OFFSET,
    QUIT_ROW_TABLE_ROWS as SYSTEM_QUIT_ROW_TABLE_ROWS, QuitRow, QuitRowVerdict, native_row_action,
    quit_controller_of_action_alias as system_quit_controller_of_action_alias,
    quit_row_verdict_text as system_quit_row_verdict_text,
};

/// A trampoline slot that has never been written.
const HOOK_ORIGINAL_UNSET: usize = 0;

unsafe extern "system" {
    /// The instant quit. Called only after the row has been positively identified as
    /// Return to Desktop and the save-flow gate has cleared.
    fn ExitProcess(code: u32) -> !;
}

// Every address below is declared once, in `er-title-flow`, and derived here. Re-declaring one
// would be a second claim about what it is (`scripts/check-rva-alias-drift.py`).
const SYSTEM_QUIT_DUPLICATE_ADD_CANCEL_BUTTON_RVA: u32 =
    er_title_flow::SYSTEM_QUIT_DUPLICATE_ADD_CANCEL_BUTTON_RVA;
const SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES: usize =
    er_title_flow::SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES;
const SYSTEM_QUIT_RETURN_TITLE_ACTION_DO_CALL_RVA: u32 =
    er_title_flow::SYSTEM_QUIT_RETURN_TITLE_ACTION_DO_CALL_RVA;
const SYSTEM_QUIT_RETURN_DESKTOP_ACTION_DO_CALL_RVA: u32 =
    er_title_flow::SYSTEM_QUIT_RETURN_DESKTOP_ACTION_DO_CALL_RVA;
const PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_RVA: u32 =
    er_title_flow::PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_RVA;
const PROPERTY_NEW_BUTTON_CONTROLLER_SHOULD_INVOKE_RVA: u32 =
    er_title_flow::PROPERTY_NEW_BUTTON_CONTROLLER_SHOULD_INVOKE_RVA;
const PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET: usize =
    er_title_flow::PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET;
const SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET: usize =
    er_title_flow::SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET;
const DIALOG_SLOT_CURSOR_B0C_OFFSET: usize = er_title_flow::DIALOG_SLOT_CURSOR_B0C_OFFSET;
const DIALOG_SLOT_BOUND_B08_OFFSET: usize = er_title_flow::DIALOG_SLOT_BOUND_B08_OFFSET;
const MENU_HELP_LABEL_DTOR_RVA: u32 = er_title_flow::MENU_HELP_LABEL_DTOR_RVA;
const MENU_HELP_LABEL_HELP_OFFSET: usize = er_title_flow::MENU_HELP_LABEL_HELP_OFFSET;
const MENU_HELP_LABEL_SIZE: usize = er_title_flow::MENU_HELP_LABEL_SIZE;
const MENU_STRING_FROM_WIDE_RVA: u32 = er_title_flow::MENU_STRING_FROM_WIDE_RVA;
const GRID_CONTROL_SET_ITEM_COUNT_RVA: u32 = er_title_flow::GRID_CONTROL_SET_ITEM_COUNT_RVA;
const OPTIONSETTING_QUIT_TAB_INDEX: usize = er_title_flow::OPTIONSETTING_QUIT_TAB_INDEX;
const SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE: usize = er_title_flow::SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE;
const SAVE_FLOW_STAGE_IDLE: usize = er_title_flow::SAVE_FLOW_STAGE_IDLE;
const SYSTEM_QUIT_NOOP_ACTION_INSTALLED_YES: usize =
    er_title_flow::SYSTEM_QUIT_NOOP_ACTION_INSTALLED_YES;
const SYSTEM_QUIT_RETURN_DESKTOP_ACTION_INSTALLED_YES: usize =
    er_title_flow::SYSTEM_QUIT_RETURN_DESKTOP_ACTION_INSTALLED_YES;
const PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_INSTALLED_YES: usize =
    er_title_flow::PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_INSTALLED_YES;

/// Scratch for one `CS::MenuHelpLabelComponent`, built on the caller's stack for exactly as long as
/// the native `AddCancelButton` call needs it.
#[repr(C, align(8))]
struct SystemQuitMenuHelpLabelScratch {
    bytes: [u8; MENU_HELP_LABEL_SIZE],
}

/// Which cloned rows this load owns.
///
/// Not a preference: it is the difference between a product that has the flows behind all four rows
/// and a shell that has only the two build rows. A row that is not in the set is never cloned, so it
/// can never be pressed, so its absent flow can never be reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowSet {
    pub load_character: bool,
    pub load_character_from_file: bool,
    pub load_build_from_url: bool,
    pub generate_build_link: bool,
}

impl RowSet {
    /// No cloned rows at all -- the vanilla tab.
    pub const NONE: Self = Self {
        load_character: false,
        load_character_from_file: false,
        load_build_from_url: false,
        generate_build_link: false,
    };
    /// Every cloned row. What the product arms.
    pub const ALL: Self = Self {
        load_character: true,
        load_character_from_file: true,
        load_build_from_url: true,
        generate_build_link: true,
    };
    /// The two rows that need no product behind them. What a standalone shell arms.
    pub const BUILD_ROWS_ONLY: Self = Self {
        load_build_from_url: true,
        generate_build_link: true,
        ..Self::NONE
    };

    fn includes(&self, row: QuitRow) -> bool {
        match row {
            QuitRow::LoadProfile => self.load_character,
            QuitRow::LoadSaveProfiles => self.load_character_from_file,
            QuitRow::LoadBuildFromUrl => self.load_build_from_url,
            QuitRow::GenerateBuildLink => self.generate_build_link,
            // The native pair are the game's own rows. Nothing clones them.
            QuitRow::SaveGame | QuitRow::ReturnToDesktop => false,
        }
    }
}

/// What the four rows this crate does not own actually do when pressed.
///
/// Each is `None` in a load that does not have that flow, and a `None` here is not a fallback: the
/// matching [`RowSet`] entry is false in the same load, so the row is not on the tab to be pressed.
/// The Save Game pair are separate because the Return-to-Desktop arm calls the save request without
/// starting the flow.
#[derive(Clone, Copy, Default)]
pub struct QuitRowActions {
    /// Open the in-game ProfileSelect character switch. Argument: the row's action object.
    pub open_profile_load_dialog: Option<unsafe fn(usize) -> bool>,
    /// Open the in-game save-container browser. Argument: the row's action object.
    pub open_save_picker_menu: Option<unsafe fn(usize) -> bool>,
    /// Stage the Save Game destination list. Argument: the System dialog.
    pub save_game_start_flow: Option<unsafe fn(usize) -> bool>,
    /// Ask the game to persist the character without starting the confirm chain. Called on the
    /// irreversible quit, immediately before `ExitProcess(0)`.
    pub save_game_request_save_only: Option<unsafe fn()>,
    /// Forget the row table's dialog-scoped product state. Called when the tab rebuilds.
    pub row_table_reset: Option<fn(usize)>,
    /// Record a same-row drive-strip click while the save picker owns ProfileSelect.
    pub note_drive_strip_click_event: Option<unsafe fn(usize)>,
}

/// Presses forwarded without resolving a row while a row table was captured. Not an error on its
/// own -- the thunk vtable is shared -- but it is the only unexamined path to the native Return to
/// Desktop, so it is counted rather than invisible.
static SYSTEM_QUIT_FOREIGN_DIALOG_FORWARDS: AtomicUsize = AtomicUsize::new(0);
/// Log the first few and then only count. A shared thunk can fire every frame in some menus.
const FOREIGN_DIALOG_FORWARD_LOG_LIMIT: usize = 8;

static ROW_SET: AtomicUsize = AtomicUsize::new(0);
static ROW_ACTIONS: std::sync::OnceLock<QuitRowActions> = std::sync::OnceLock::new();

/// Bit positions of [`RowSet`] inside the published word. Four bits, so "armed" and "armed with no
/// rows" stay distinguishable from the `ARMED` bit below.
const ROW_BIT_LOAD_CHARACTER: usize = 1 << 0;
const ROW_BIT_LOAD_FROM_FILE: usize = 1 << 1;
const ROW_BIT_LOAD_BUILD_URL: usize = 1 << 2;
const ROW_BIT_GENERATE_LINK: usize = 1 << 3;
const ROW_BIT_ARMED: usize = 1 << 4;

fn publish_row_set(rows: RowSet) {
    let mut word = ROW_BIT_ARMED;
    if rows.load_character {
        word |= ROW_BIT_LOAD_CHARACTER;
    }
    if rows.load_character_from_file {
        word |= ROW_BIT_LOAD_FROM_FILE;
    }
    if rows.load_build_from_url {
        word |= ROW_BIT_LOAD_BUILD_URL;
    }
    if rows.generate_build_link {
        word |= ROW_BIT_GENERATE_LINK;
    }
    ROW_SET.store(word, Ordering::SeqCst);
}

/// The rows this load armed. `NONE` before [`arm`] runs, which is also what stops the cloner from
/// touching a dialog in a process that never armed it.
fn row_set() -> RowSet {
    let word = ROW_SET.load(Ordering::SeqCst);
    if word & ROW_BIT_ARMED == 0 {
        return RowSet::NONE;
    }
    RowSet {
        load_character: word & ROW_BIT_LOAD_CHARACTER != 0,
        load_character_from_file: word & ROW_BIT_LOAD_FROM_FILE != 0,
        load_build_from_url: word & ROW_BIT_LOAD_BUILD_URL != 0,
        generate_build_link: word & ROW_BIT_GENERATE_LINK != 0,
    }
}

fn row_actions() -> &'static QuitRowActions {
    static NONE: QuitRowActions = QuitRowActions {
        open_profile_load_dialog: None,
        open_save_picker_menu: None,
        save_game_start_flow: None,
        save_game_request_save_only: None,
        row_table_reset: None,
        note_drive_strip_click_event: None,
    };
    ROW_ACTIONS.get().unwrap_or(&NONE)
}

/// Forget the captured row table. Called when the Quit tab starts building a dialog so a rebuilt
/// pane can never be resolved against another dialog's indices.
pub fn system_quit_row_table_reset(dialog: usize) {
    // The Quit tab is building a fresh dialog, so any link field latched against the previous one
    // is pointing at a dead `MenuJobQueue`. This is the only moment that is reliably true, which is
    // why the field's reset hangs off the row table's rather than having a lifecycle of its own.
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
    if let Some(reset) = row_actions().row_table_reset {
        reset(dialog);
    }
}

/// Resolve which Quit row an activation belongs to, from live memory, and record the outcome.
///
/// # Safety
///
/// `activation_dialog` is read through the fault-safe readers; the caller must be on a menu thread.
pub unsafe fn system_quit_resolve_row_now(
    activation_dialog: usize,
    event: usize,
) -> QuitRowVerdict {
    let cursor = if activation_dialog >= 0x10000 {
        unsafe { safe_read_i32(activation_dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }.unwrap_or(-1)
    } else {
        -1
    };
    unsafe { crate::row_identity::system_quit_resolve_row_now(activation_dialog, event, cursor) }
}

/// Read the live `CS::GridControl` geometry of the patched Quit dialog into the navigability
/// oracles.
///
/// # Safety
///
/// `dialog` is read through the fault-safe readers.
pub unsafe fn system_quit_record_grid_geometry(dialog: usize) {
    if dialog < 0x10000 {
        return;
    }
    let count = unsafe { safe_read_i32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET) }.unwrap_or(-1);
    unsafe { crate::row_identity::system_quit_record_grid_geometry(dialog, count) };
}

/// Route one Quit-tab row action thunk (`FUN_140961640` for the first native row, `FUN_1409610d0`
/// for the second and every row cloned from it).
///
/// `action_obj` is the thunk's `this`, and it is only `controller + 0x70` -- the controller's own
/// inline `std::function` storage (see `system_quit_row_identity.rs`). It therefore cannot name a
/// row, so the row is resolved positively from the row table + the live label at the dialog's list
/// cursor. An unresolvable row is suppressed rather than forwarded, because the native action behind
/// the second row is the irreversible Return to Desktop.
///
/// # Safety
///
/// Quit-tab action-thunk context. `action_obj` is a live thunk `this`, and `orig` is either the
/// game trampoline for that thunk or the next handler on the union, both callable under the union
/// signature. Calling this off that path forwards an arbitrary pointer into a native action.
pub unsafe fn system_quit_route_button_action_or_forward(
    action_obj: usize,
    orig: usize,
    hook_name: &str,
) -> usize {
    if action_obj == 0 {
        return 0;
    }
    let controller = system_quit_controller_of_action_alias(action_obj);
    let dialog =
        unsafe { safe_read_usize(action_obj + SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET) }
            .unwrap_or(0);
    let cursor = if dialog >= 0x10000 {
        unsafe { safe_read_i32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }.unwrap_or(-1)
    } else {
        -1
    };
    // This `_Func_impl` thunk vtable (dump 0x142b125d0 slot +0x10) is shared by several construction
    // sites, so the hook also sees cancel/confirm rows of dialogs that are not the patched Quit tab.
    // Forward those untouched -- and before resolving, so foreign dialogs never pollute the row
    // oracles.
    let table_dialog = SYSTEM_QUIT_ROW_TABLE_DIALOG.load(Ordering::SeqCst);
    if dialog == 0 || table_dialog == 0 || dialog != table_dialog {
        // The one forward in this function that happens without resolving a row, and the action
        // behind the second Quit row's thunk is the irreversible Return to Desktop -- so a mismatch
        // here can quit the process with nothing in the log to say it did. A genuinely foreign
        // dialog (no table captured at all) stays silent, because this thunk vtable is shared with
        // menus that have nothing to do with the Quit tab and logging those would bury the run.
        // What is worth a line is a DISAGREEMENT: a table exists and this press belongs to some
        // other dialog, which is either a stale table or a second dialog wearing the same thunk.
        if table_dialog != 0 && dialog != 0 {
            let seen = SYSTEM_QUIT_FOREIGN_DIALOG_FORWARDS.fetch_add(1, Ordering::SeqCst) + 1;
            if seen <= FOREIGN_DIALOG_FORWARD_LOG_LIMIT {
                append_autoload_debug(format_args!(
                    "system-quit-dup: {hook_name} forwarding UNRESOLVED action_alias=0x{action_obj:x} controller=0x{controller:x} dialog=0x{dialog:x} table_dialog=0x{table_dialog:x} cursor={cursor}; this press belongs to a dialog the row table was not built for, and the native action behind this thunk may be Return to Desktop (line {seen} of {FOREIGN_DIALOG_FORWARD_LOG_LIMIT}; total in `SYSTEM_QUIT_FOREIGN_DIALOG_FORWARDS`)"
                ));
            }
        }
        if orig == HOOK_ORIGINAL_UNSET {
            append_autoload_debug(format_args!(
                "system-quit-save: {hook_name} action trampoline is unset for action_alias=0x{action_obj:x} dialog=0x{dialog:x} table_dialog=0x{table_dialog:x}; fail-open return 0"
            ));
            return 0;
        }
        // Called through the union signature on purpose: under the chain this slot holds either
        // the game trampoline (one argument, the extra registers harmlessly ignored) or the next
        // handler, which is four-argument. Calling a chained handler with the game's narrower
        // signature would leave its trailing registers undefined.
        let original: er_hook::UnionFn = unsafe { std::mem::transmute(orig) };
        return unsafe { original(action_obj, 0, 0, 0) };
    }
    // No event object reaches an action thunk, so the input kind is unclassifiable here -- which
    // costs nothing, because the list cursor names the row for every input kind alike.
    let verdict = unsafe { system_quit_resolve_row_now(dialog, 0) };
    let verdict_text = system_quit_row_verdict_text(verdict);
    match verdict.resolved_row() {
        Some(QuitRow::LoadProfile) => {
            let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
            if phase != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE {
                append_autoload_debug(format_args!(
                    "system-quit-dup: cloned quick-load action re-entry ignored action_alias=0x{action_obj:x} phase={phase}; native handoff already armed"
                ));
                return 0;
            }
            SYSTEM_QUIT_NOOP_SELECTION_COUNT.fetch_add(1, Ordering::SeqCst);
            let opened = match row_actions().open_profile_load_dialog {
                Some(open) => unsafe { open(action_obj) },
                None => false,
            };
            append_autoload_debug(format_args!(
                "system-quit-dup: Load Profile action selected {hook_name} action_alias=0x{action_obj:x} controller=0x{controller:x} cursor={cursor} {verdict_text} opened={opened}; suppressing native Quit Game row action until ProfileSelect confirms slot"
            ));
            0
        }
        Some(QuitRow::LoadSaveProfiles) => {
            SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_COUNT.fetch_add(1, Ordering::SeqCst);
            let opened = match row_actions().open_save_picker_menu {
                Some(open) => unsafe { open(action_obj) },
                None => false,
            };
            append_autoload_debug(format_args!(
                "system-quit-load-save-profiles: Load Save Profiles action selected {hook_name} action_alias=0x{action_obj:x} controller=0x{controller:x} cursor={cursor} {verdict_text} opened={opened:?} (in-game save picker); suppressing native Quit Game row action"
            ));
            0
        }
        // The one row on this tab that neither returns to the title nor touches a save container:
        // it queues a build import against the character already in the world. Nothing here blocks
        // or mutates game state -- the press spawns a fetch worker and the FrameBegin task applies
        // the result -- so it is the same two lines from either routing hook.
        Some(QuitRow::LoadBuildFromUrl) => {
            let press = system_quit_start_build_import(dialog);
            system_quit_log_build_import_press(hook_name, &press);
            append_autoload_debug(format_args!(
                "system-quit-build-url: action_alias=0x{action_obj:x} controller=0x{controller:x} cursor={cursor} {verdict_text}; suppressing the native Quit Game row action behind this thunk"
            ));
            0
        }
        // The only row that reads the character instead of writing to it, and the only one with no
        // dialog of any kind behind it: the press queues a read and returns, and the row's own help
        // line becomes the report.
        Some(QuitRow::GenerateBuildLink) => {
            let press = system_quit_start_build_export();
            system_quit_log_build_export_press(hook_name, &press);
            append_autoload_debug(format_args!(
                "system-quit-generate-link: action_alias=0x{action_obj:x} controller=0x{controller:x} cursor={cursor} {verdict_text}; suppressing the native Quit Game row action behind this thunk"
            ));
            0
        }
        Some(QuitRow::SaveGame) => {
            if dialog < 0x10000 {
                append_autoload_debug(format_args!(
                    "system-quit-save: Save Game row press IGNORED action_alias=0x{action_obj:x}; dialog=0x{dialog:x} is not heap-like"
                ));
                return 0;
            }
            // Save-flow re-entry guard (WP1): one commit at a time, and never while a profile
            // switch owns the quit machinery -- both flows drive the same close sequence and
            // GameMan save fields.
            let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
            let stage = SAVE_FLOW_STAGE.load(Ordering::SeqCst);
            if phase != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE || stage != SAVE_FLOW_STAGE_IDLE {
                append_autoload_debug(format_args!(
                    "system-quit-save: Save Game row press IGNORED action_alias=0x{action_obj:x} quickload_phase={phase} save_flow_stage={stage}; a switch or save commit is already in flight"
                ));
                return 0;
            }
            // A load with no Save Game flow must not swallow this row. The press is positively
            // the native first row, and its own thunk still holds the game's action, so forwarding
            // leaves the row exactly as vanilla built it -- label and behaviour together. Replacing
            // it with nothing is what a standalone shell used to do: measured 2026-09-11 on an
            // product-less profile arming this row alone, where the tab's first row read "Quit Game"
            // and did nothing at all, because this arm suppressed the native action unconditionally
            // and then had no flow to run in its place.
            //
            // Both halves of the identity have to agree before anything is forwarded. The cursor
            // naming this row is not enough on its own: the visible buttons dispatch through only
            // two controllers, so a press can arrive carrying the other native row's thunk, and the
            // action behind that one is the irreversible Return to Desktop. The captured first-row
            // controller is the second half, and without it this falls through to the suppression
            // it always did.
            let save_game_controller =
                SYSTEM_QUIT_NATIVE_SAVE_GAME_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst);
            match native_row_action(
                row_actions().save_game_start_flow.is_some(),
                orig != HOOK_ORIGINAL_UNSET,
                save_game_controller,
                controller,
            ) {
                NativeRowAction::RunFlow => {}
                NativeRowAction::ForwardNative => {
                    SYSTEM_QUIT_SAVE_GAME_ACTION_COUNT.fetch_add(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-save: native first Quit row FORWARDED at {hook_name} action_alias=0x{action_obj:x} controller=0x{controller:x} cursor={cursor} {verdict_text}; this load supplies no Save Game flow, so the row keeps the game's own action rather than becoming a row that does nothing"
                    ));
                    // Safety: the union slot holds either the game trampoline or the next handler,
                    // both callable under the union signature; the same call the foreign-dialog
                    // path above makes.
                    let original: er_hook::UnionFn = unsafe { std::mem::transmute(orig) };
                    return unsafe { original(action_obj, 0, 0, 0) };
                }
                NativeRowAction::Suppress => {
                    append_autoload_debug(format_args!(
                        "system-quit-save: native first Quit row SUPPRESSED at {hook_name} action_alias=0x{action_obj:x} controller=0x{controller:x} captured_save_game_controller=0x{save_game_controller:x} cursor={cursor} {verdict_text}; no Save Game flow and the dispatching controller is not the captured first row's, so the native action behind this thunk may be Return to Desktop"
                    ));
                    return 0;
                }
            }
            SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG.store(0, Ordering::SeqCst);
            SYSTEM_QUIT_SAVE_GAME_ACTION_COUNT.fetch_add(1, Ordering::SeqCst);
            let started = match row_actions().save_game_start_flow {
                Some(start) => unsafe { start(dialog) },
                None => false,
            };
            append_autoload_debug(format_args!(
                "system-quit-save: Save Game native row selected {hook_name} action_alias=0x{action_obj:x} dialog=0x{dialog:x} cursor={cursor} {verdict_text}; staged the destination list started={started} stage={}; suppressed native Quit Game/return-title action",
                SAVE_FLOW_STAGE.load(Ordering::SeqCst)
            ));
            0
        }
        Some(QuitRow::ReturnToDesktop) => {
            // The genuine Return to Desktop, positively identified. Make quit an instant ALT+F4:
            // persist the save, release the cursor clip, and ExitProcess(0) before the world
            // teardown renders a loading screen. The old clean-kill (system_quit_ownership_repro)
            // fired mid-teardown, so the loading cover was already visible.
            //
            // Two further refusals guard this irreversible step (save-safety hardening, kept):
            //   * the activated controller must not be the Save Game row's -- a Save Game press can
            //     never quit the game;
            //   * `save_flow_stage == IDLE` -- never exit mid save-flow, where a commit may be armed
            //     or in flight and a process exit would tear it in half.
            let save_flow_stage = SAVE_FLOW_STAGE.load(Ordering::SeqCst);
            let save_game_controller =
                SYSTEM_QUIT_NATIVE_SAVE_GAME_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst);
            if (save_game_controller != 0 && controller == save_game_controller)
                || save_flow_stage != SAVE_FLOW_STAGE_IDLE
            {
                SYSTEM_QUIT_QUIT_REFUSED_AMBIGUOUS_ROW_COUNT.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "quit-to-desktop: REFUSING the instant quit at {hook_name} for controller=0x{controller:x} cursor={cursor} -- save_game_controller=0x{save_game_controller:x} save_flow_stage={save_flow_stage}; a Save Game press must never quit the game and a save flow must never be torn in half"
                ));
                return 0;
            }
            if !system_quit_row_gate_instant_quit(verdict, hook_name) {
                return 0;
            }
            // The instant `ExitProcess(0)` is only safe because the product persists the character
            // first. A load that cannot do that must not take this path: it would terminate the
            // process on a character whose progress since the last autosave has never been written,
            // which is the one failure on this tab the player cannot undo. Forward the game's own
            // Return to Desktop instead -- slower, and it renders the teardown this path exists to
            // skip, but it is the vanilla quit and it saves. Same two-half identity as the first
            // row: the cursor must name this row and the press must carry the captured second-row
            // controller.
            let return_desktop_controller =
                SYSTEM_QUIT_NATIVE_RETURN_DESKTOP_CONTROLLER_LAST_OBJECT.load(Ordering::SeqCst);
            let request_save = match native_row_action(
                row_actions().save_game_request_save_only.is_some(),
                orig != HOOK_ORIGINAL_UNSET,
                return_desktop_controller,
                controller,
            ) {
                NativeRowAction::RunFlow => row_actions()
                    .save_game_request_save_only
                    .expect("RunFlow is only returned when the save request is present"),
                NativeRowAction::ForwardNative => {
                    append_autoload_debug(format_args!(
                        "quit-to-desktop: native Return to Desktop FORWARDED at {hook_name} controller=0x{controller:x} cursor={cursor} {verdict_text}; this load cannot request a save, so the instant ExitProcess(0) is refused and the game's own quit runs instead"
                    ));
                    // Safety: as above -- the union slot under the union signature.
                    let original: er_hook::UnionFn = unsafe { std::mem::transmute(orig) };
                    return unsafe { original(action_obj, 0, 0, 0) };
                }
                NativeRowAction::Suppress => {
                    SYSTEM_QUIT_QUIT_REFUSED_AMBIGUOUS_ROW_COUNT.fetch_add(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "quit-to-desktop: REFUSING the quit at {hook_name} controller=0x{controller:x} captured_return_desktop_controller=0x{return_desktop_controller:x} cursor={cursor} {verdict_text}; no save request available and the dispatching controller is not the captured second row's"
                    ));
                    return 0;
                }
            };
            unsafe { request_save() };
            release_input_block_now();
            append_autoload_debug(format_args!(
                "quit-to-desktop: Return-to-Desktop confirmed at {hook_name} controller=0x{controller:x} action_alias=0x{action_obj:x} cursor={cursor} {verdict_text}; requested save + released cursor clip; INSTANT ExitProcess(0) before world teardown (no loading screen)"
            ));
            unsafe { ExitProcess(0) }
        }
        None => {
            // The patched Quit dialog, row not identified. Suppress instead of forwarding:
            // forwarding this thunk runs the native action, and for the second Quit row that action
            // is Return to Desktop. A row press that does nothing is a nuisance; a row press that
            // terminates the process is not shippable.
            system_quit_row_gate_instant_quit(verdict, hook_name);
            append_autoload_debug(format_args!(
                "system-quit-save: {hook_name} row press SUPPRESSED action_alias=0x{action_obj:x} controller=0x{controller:x} dialog=0x{dialog:x} cursor={cursor} {verdict_text}; the native action behind this thunk is the irreversible Return to Desktop, so an unidentified row of the patched Quit tab must not reach it"
            ));
            0
        }
    }
}

/// The Save Game row's action thunk, reached as the first native Quit row.
///
/// # Safety
///
/// Installed by `er-hook` on `FUN_140961640`; the game calls it on the menu thread with a live
/// thunk `this`.
pub unsafe extern "system" fn system_quit_noop_desktop_action_hook(action_obj: usize) -> usize {
    let orig = SYSTEM_QUIT_NOOP_ACTION_ORIG.load(Ordering::SeqCst);
    unsafe { system_quit_route_button_action_or_forward(action_obj, orig, "save-game/first-row") }
}

/// The second native Quit row's action thunk, shared by every row cloned from it.
///
/// # Safety
///
/// Installed by `er-hook` on `FUN_1409610d0`; the game calls it on the menu thread with a live
/// thunk `this`.
pub unsafe extern "system" fn system_quit_return_desktop_action_hook(action_obj: usize) -> usize {
    let orig = SYSTEM_QUIT_RETURN_DESKTOP_ACTION_ORIG.load(Ordering::SeqCst);
    unsafe {
        system_quit_route_button_action_or_forward(action_obj, orig, "return-desktop/second-row")
    }
}

/// Hand one controller activation back to whoever owns the address below us.
///
/// # Safety
///
/// `PropertyNewButtonController::Activate` context. `controller` and both event arguments must be
/// the live ones the game passed, and the published trampoline must still be installed.
pub unsafe fn system_quit_forward_button_controller_activation(
    controller: usize,
    event_kind: u32,
    event_a: usize,
    event_b: usize,
) {
    let orig = PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: PropertyNewButtonController activation trampoline unset for controller=0x{controller:x}; fail-closed return"
        ));
        return;
    }
    // Union signature, same reason as the action thunks: the slot holds either the game
    // trampoline or the next four-argument handler. The game's own `event_kind` is a `u32` in
    // `edx`, so widening it back to the register the union passes is lossless.
    let original: er_hook::UnionFn = unsafe { std::mem::transmute(orig) };
    unsafe { original(controller, event_kind as usize, event_a, event_b) };
}

/// Ask the controller's own predicate whether this event is a real confirm, rather than deciding
/// from the event kind -- the game collapses several kinds onto one dispatch.
///
/// # Safety
///
/// `PropertyNewButtonController::Activate` context, `controller` live; this calls through the
/// controller's vtable.
pub unsafe fn system_quit_controller_should_invoke_action(
    controller: usize,
    event_a: usize,
) -> bool {
    let Ok(predicate_addr) = game_rva(PROPERTY_NEW_BUTTON_CONTROLLER_SHOULD_INVOKE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve PropertyNewButtonController action predicate rva 0x{PROPERTY_NEW_BUTTON_CONTROLLER_SHOULD_INVOKE_RVA:x}; forwarding native activation"
        ));
        return false;
    };
    let predicate: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(predicate_addr) };
    unsafe { predicate(controller, event_a) != 0 }
}

/// `PropertyNewButtonController::Activate` (dump `FUN_1409749f0`, vtable slot 2). Called once per
/// frame per dispatched row with the live event; the controller's own should-invoke predicate decides
/// whether that event is a real confirm.
///
/// The controller is used only to scope the hook to the patched Quit tab. It cannot name a row: the
/// dispatch collapses cloned buttons onto the native Return-to-Desktop controller, and the pointer at
/// `controller + 0xa8` is merely `controller + 0x70`. Row identity comes from
/// `system_quit_resolve_row_now`, i.e. the dialog's own list cursor.
///
/// # Safety
///
/// Installed by `er-hook` on vtable slot 2; the game calls it on the menu thread with a live
/// controller and its live event.
pub unsafe extern "system" fn property_new_button_controller_activate_hook(
    controller: usize,
    event_kind: u32,
    event_a: usize,
    event_b: usize,
) {
    if !system_quit_controller_is_a_quit_row(controller) {
        // Not a row of the patched Quit tab: vanilla behaviour, untouched. While the save picker owns
        // ProfileSelect, use the same event to capture a same-row drive-strip click before forwarding;
        // the normal ProfileLoad activation hook will consume the pending cell.
        if SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0
            && unsafe { system_quit_controller_should_invoke_action(controller, event_a) }
            && let Some(note_click) = row_actions().note_drive_strip_click_event
        {
            unsafe { note_click(event_a) };
        }
        unsafe {
            system_quit_forward_button_controller_activation(
                controller, event_kind, event_a, event_b,
            )
        };
        return;
    }
    // Focus / per-frame update rather than a confirm: never routes, never quits.
    if !unsafe { system_quit_controller_should_invoke_action(controller, event_a) } {
        unsafe {
            system_quit_forward_button_controller_activation(
                controller, event_kind, event_a, event_b,
            )
        };
        return;
    }
    let action_alias =
        controller.saturating_add(PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_STORAGE_OFFSET);
    let dialog =
        unsafe { safe_read_usize(action_alias + SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET) }
            .unwrap_or(0);
    let verdict = unsafe { system_quit_resolve_row_now(dialog, event_a) };
    let verdict_text = system_quit_row_verdict_text(verdict);
    match verdict.resolved_row() {
        Some(QuitRow::LoadProfile) => {
            let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
            if phase != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE {
                append_autoload_debug(format_args!(
                    "system-quit-dup: controller quick-load activation ignored controller=0x{controller:x} phase={phase}; native handoff already armed"
                ));
                return;
            }
            SYSTEM_QUIT_NOOP_SELECTION_COUNT.fetch_add(1, Ordering::SeqCst);
            let opened = match row_actions().open_profile_load_dialog {
                Some(open) => unsafe { open(action_alias) },
                None => false,
            };
            append_autoload_debug(format_args!(
                "system-quit-dup: Load Profile controller selected controller=0x{controller:x} {verdict_text} event_kind={event_kind} opened={opened}; suppressing native button activation"
            ));
        }
        Some(QuitRow::LoadSaveProfiles) => {
            SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_COUNT.fetch_add(1, Ordering::SeqCst);
            let opened = match row_actions().open_save_picker_menu {
                Some(open) => unsafe { open(action_alias) },
                None => false,
            };
            append_autoload_debug(format_args!(
                "system-quit-load-save-profiles: Load Save Profiles controller selected controller=0x{controller:x} {verdict_text} event_kind={event_kind} opened={opened:?} (in-game save picker); suppressing native button activation"
            ));
        }
        Some(QuitRow::LoadBuildFromUrl) => {
            let press = system_quit_start_build_import(dialog);
            system_quit_log_build_import_press("build-url/controller", &press);
            append_autoload_debug(format_args!(
                "system-quit-build-url: controller=0x{controller:x} {verdict_text} event_kind={event_kind}; suppressing native button activation"
            ));
        }
        Some(QuitRow::GenerateBuildLink) => {
            let press = system_quit_start_build_export();
            system_quit_log_build_export_press("generate-link/controller", &press);
            append_autoload_debug(format_args!(
                "system-quit-generate-link: controller=0x{controller:x} {verdict_text} event_kind={event_kind}; suppressing native button activation"
            ));
        }
        Some(QuitRow::ReturnToDesktop) => {
            // Positively the genuine Return to Desktop. Make quit an instant ALT+F4: persist the
            // save, release the cursor, and ExitProcess(0) before the world teardown renders any
            // loading screen / our isolated overlay.
            //
            // Require that no profile switch is in flight: the native return-desktop controller is
            // dispatched again from ProfileSelect (observed: 12 activations carried it during one
            // switch), so without this gate a switch's activation would ExitProcess mid-switch. And
            // never exit mid save-flow, where a commit may be armed or in flight.
            let switch_in_flight = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
                != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE
                || SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) != 0
                || SYSTEM_QUIT_PROFILE_LOAD_FLOW_ACTIVE.load(Ordering::SeqCst) != 0;
            let save_flow_in_flight =
                SAVE_FLOW_STAGE.load(Ordering::SeqCst) != SAVE_FLOW_STAGE_IDLE;
            if switch_in_flight || save_flow_in_flight {
                SYSTEM_QUIT_QUIT_REFUSED_AMBIGUOUS_ROW_COUNT.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "quit-to-desktop: REFUSING the instant quit at controller=0x{controller:x} -- switch_in_flight={switch_in_flight} save_flow_in_flight={save_flow_in_flight}; forwarding the native activation instead"
                ));
                unsafe {
                    system_quit_forward_button_controller_activation(
                        controller, event_kind, event_a, event_b,
                    )
                };
                return;
            }
            if !system_quit_row_gate_instant_quit(verdict, "return-desktop/controller") {
                // Unreachable while `resolved_row()` says Return to Desktop, but the gate stays the
                // single authority over the irreversible step.
                unsafe {
                    system_quit_forward_button_controller_activation(
                        controller, event_kind, event_a, event_b,
                    )
                };
                return;
            }
            if let Some(request_save) = row_actions().save_game_request_save_only {
                unsafe { request_save() };
            }
            release_input_block_now();
            append_autoload_debug(format_args!(
                "quit-to-desktop: Return-to-Desktop controller CLICK controller=0x{controller:x} action_alias=0x{action_alias:x} {verdict_text} event_kind={event_kind}; requested save + released cursor; INSTANT ExitProcess(0) (no teardown/loading screen)"
            ));
            unsafe { ExitProcess(0) };
        }
        // Save Game keeps flowing through its own native action thunk, which the action-route hook
        // owns (that is where the confirm chain and its re-entry guards live).
        Some(QuitRow::SaveGame) | None => {
            if verdict.resolved_row().is_none() {
                append_autoload_debug(format_args!(
                    "quit-to-desktop: controller confirm NOT routed controller=0x{controller:x} dialog=0x{dialog:x} {verdict_text} event_kind={event_kind}; forwarding the native activation, which the action-route hook gates again"
                ));
            }
            unsafe {
                system_quit_forward_button_controller_activation(
                    controller, event_kind, event_a, event_b,
                )
            };
        }
    }
}

/// # Safety
///
/// `out` must point at uninitialised storage the size of the game's menu-string type, and `text`
/// must stay alive for the process -- the constructor keeps the pointer rather than copying.
pub unsafe fn system_quit_init_menu_string_from_static_wide(
    out: usize,
    text: &'static [u16],
) -> bool {
    let Ok(ctor_addr) = game_rva(MENU_STRING_FROM_WIDE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve MenuString ctor rva 0x{MENU_STRING_FROM_WIDE_RVA:x}; cannot build static label"
        ));
        return false;
    };
    let ctor: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(ctor_addr) };
    unsafe { ctor(out, text.as_ptr() as usize) };
    true
}

/// # Safety
///
/// `out` must point at uninitialised storage for the label component, and both slices must be
/// process-lifetime: the built component borrows them instead of copying.
pub unsafe fn system_quit_build_static_label_component(
    out: usize,
    label: &'static [u16],
    help: &'static [u16],
) -> bool {
    unsafe { std::ptr::write_bytes(out as *mut u8, 0, MENU_HELP_LABEL_SIZE) };
    let label_ok = unsafe { system_quit_init_menu_string_from_static_wide(out, label) };
    let help_ok = unsafe {
        system_quit_init_menu_string_from_static_wide(out + MENU_HELP_LABEL_HELP_OFFSET, help)
    };
    label_ok && help_ok
}

/// Say once, per process, that a 1.16.2-only comparison has declined on this build.
///
/// Once and not per-call: these sit on hook paths that run at frame rate, and the failure mode
/// this whole change exists to fix is a silent one -- the cure for silence is a line a reader can
/// find, not 300,000 of them. (One session logged 339,764 copies of a single refusal; see
/// `scripts/check-no-rva-zero.py`.)
fn note_unsupported_build_comparison(what: &str) {
    use std::sync::atomic::AtomicUsize;
    static SAID: AtomicUsize = AtomicUsize::new(0);
    const SAY_AT_MOST: usize = 8;
    if SAID.fetch_add(1, Ordering::SeqCst) >= SAY_AT_MOST {
        return;
    }
    append_autoload_debug(format_args!(
        "callsite-gate: {what} compares a live return address against a 1.16.2 RVA that has no \
         verified address for this build -- the comparison is DECLINED rather than run against \
         the wrong bytes, so the feature behind it is inert on this build"
    ));
}

/// The System>Quit row detection could not be placed on this build. Separate wording from the
/// band above because the fix is different: this one needs a map row for `FUN_140958910`, and the
/// band needs someone to re-derive what it was ever watching.
fn system_quit_row_returns_unavailable_once() {
    note_unsupported_build_comparison(
        "System>Quit row cloning (SYSTEM_QUIT_QUIT_TAB_BUILDER_RVA 0x958910)",
    );
}

/// # Safety
///
/// Installed by `er-hook` on `AddCancelButton`; the game calls it on the menu thread while it
/// builds the Quit tab, with a live dialog and a live label component.
pub unsafe extern "system" fn system_quit_duplicate_add_cancel_button_hook(
    dialog: usize,
    label: usize,
    action_fn: usize,
    enabled_fn: usize,
    keyguide_fn: usize,
) -> usize {
    let orig = SYSTEM_QUIT_DUPLICATE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: original AddCancelButton trampoline is unset -- fail-open return 0"
        ));
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    // Both return addresses come from one resolution of the containing function
    // (`FUN_140958910`), because they are two offsets into it. Reached raw, these were 1.16.2
    // RVAs compared against live stack frames: on 1.17 neither ever matched, nothing was hooked
    // or resolved so nothing was logged, and all three cloned rows silently vanished from the
    // System>Quit tab. A refusal here is loud, once, and names the constant.
    let Some((first_row_return, second_row_return)) = er_title_flow::system_quit_row_return_rvas()
    else {
        system_quit_row_returns_unavailable_once();
        return unsafe { original(dialog, label, action_fn, enabled_fn, keyguide_fn) };
    };
    let first_row_call = callstack_contains_game_rva(
        first_row_return.saturating_sub(SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES),
        first_row_return + SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES,
    );
    let second_row_call = callstack_contains_game_rva(
        second_row_return.saturating_sub(SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES),
        second_row_return + SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES,
    );
    let before =
        unsafe { safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET) }
            .unwrap_or(0);
    let ret = unsafe { original(dialog, label, action_fn, enabled_fn, keyguide_fn) };
    if !(first_row_call || second_row_call) {
        return ret;
    }

    // OptionSetting constructs/lazily rebuilds hidden tab panes while another tab is visible. Mutate
    // only the Quit tab's own dialog; never write rows into the active non-Quit pane.
    let active_tab = OPTIONSETTING_CURRENT_TAB.load(Ordering::SeqCst);
    let active_dialog = OPTIONSETTING_CURRENT_DIALOG.load(Ordering::SeqCst);
    let actively_shown = OPTIONSETTING_ACTIVELY_SHOWN.load(Ordering::SeqCst) != 0;
    if actively_shown && active_tab != OPTIONSETTING_QUIT_TAB_INDEX && active_dialog == dialog {
        let skip_n = SYSTEM_QUIT_DUPLICATE_COUNT.load(Ordering::SeqCst);
        if skip_n < 16 {
            append_autoload_debug(format_args!(
                "system-quit-dup: matched Quit Game AddCancelButton but target is active non-Quit tab={active_tab} dialog=0x{dialog:x}; skipping row routing so active tab stays vanilla"
            ));
        }
        return ret;
    }

    let after_native =
        unsafe { safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET) }
            .unwrap_or(0);
    let properties = dialog + PROPERTY_EDIT_DIALOG_PROPERTIES_1268_OFFSET;
    let aligned_properties = (properties + 0x7) & !0x7;
    let mut after_final = after_native;
    if after_native > before {
        let native_row_index = after_native.saturating_sub(1);
        let native_row = aligned_properties + EDIT_PROPERTY_SIZE.saturating_mul(native_row_index);
        let native_controller =
            unsafe { safe_read_usize(native_row + EDIT_PROPERTY_CONTROLLER_OFFSET) }.unwrap_or(0);
        let native_action = if native_controller != 0 {
            unsafe {
                safe_read_usize(
                    native_controller + PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET,
                )
            }
            .unwrap_or(0)
        } else {
            0
        };
        if native_action != 0 && first_row_call {
            // The first native row starts a fresh row table: this is the Quit tab building its
            // dialog, and every index/controller from an earlier build is now stale (a heap address
            // may even have been reused by this build).
            system_quit_row_table_reset(dialog);
            SYSTEM_QUIT_NATIVE_SAVE_GAME_ACTION_LAST_OBJECT.store(native_action, Ordering::SeqCst);
            SYSTEM_QUIT_NATIVE_SAVE_GAME_CONTROLLER_LAST_OBJECT
                .store(native_controller, Ordering::SeqCst);
            system_quit_row_table_record_index(QuitRow::SaveGame, native_row_index);
            append_autoload_debug(format_args!(
                "system-quit-dup: captured native first Quit row index={native_row_index} controller=0x{native_controller:x} action_alias=0x{native_action:x} (== controller+0x{PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_STORAGE_OFFSET:x}); routing this in-place button to Save Game"
            ));
        } else if native_action != 0 && second_row_call {
            SYSTEM_QUIT_NATIVE_RETURN_DESKTOP_ACTION_LAST_OBJECT
                .store(native_action, Ordering::SeqCst);
            SYSTEM_QUIT_NATIVE_RETURN_DESKTOP_CONTROLLER_LAST_OBJECT
                .store(native_controller, Ordering::SeqCst);
            system_quit_row_table_record_index(QuitRow::ReturnToDesktop, native_row_index);
            append_autoload_debug(format_args!(
                "system-quit-dup: captured native second Quit row index={native_row_index} controller=0x{native_controller:x} action_alias=0x{native_action:x}; Return to Desktop is now identified by ROW (index + live label), never by this pointer"
            ));
        }
    }

    if second_row_call {
        let Ok(label_dtor_addr) = game_rva(MENU_HELP_LABEL_DTOR_RVA) else {
            append_autoload_debug(format_args!(
                "system-quit-dup: failed to resolve MenuHelpLabelComponent dtor rva 0x{MENU_HELP_LABEL_DTOR_RVA:x}; cannot add the cloned Quit rows"
            ));
            SYSTEM_QUIT_DUPLICATE_LAST_COUNT_BEFORE.store(before, Ordering::SeqCst);
            SYSTEM_QUIT_DUPLICATE_LAST_COUNT_AFTER.store(after_native, Ordering::SeqCst);
            return ret;
        };
        let label_dtor: unsafe extern "system" fn(usize) =
            unsafe { std::mem::transmute(label_dtor_addr) };
        // The cloned rows, as data. Each is the same five steps -- build a `MenuHelpLabelComponent`
        // over this DLL's own process-lifetime label/help arrays, call the native AddCancelButton
        // with it, destruct the component, read back the row the call appended, and record that
        // row's controller + property index. They were two hand-expanded copies of those steps
        // until the third row arrived; a table walked once is what stops the copies drifting.
        //
        // Order is the product contract, not a detail: the property index a row lands at is its
        // grid cell (`row * cols + col`), so this order is what puts Load Character at `Item_1_0`,
        // Load Character from File at `Item_1_1`, Load Build from URL at `Item_2_0` and Generate
        // Build Link at `Item_2_1`. It must match `er_gfx::options_02_040::QUIT6_GRID_CELL_NAMES`.
        struct ClonedRow {
            row: QuitRow,
            label: &'static [u16; SYSTEM_QUIT_ROW_TEXT_CAPACITY],
            help: &'static [u16],
            /// Where to record the row's action alias and its `PropertyNewButtonController`. Both
            /// are telemetry: the row identity is the list cursor, never these pointers.
            action_slot: &'static AtomicUsize,
            controller_slot: &'static AtomicUsize,
        }
        let cloned_rows = [
            ClonedRow {
                row: QuitRow::LoadProfile,
                label: &SYSTEM_QUIT_LOAD_PROFILE_LABEL_W,
                help: SYSTEM_QUIT_LOAD_PROFILE_HELP_W.as_slice(),
                action_slot: &SYSTEM_QUIT_NOOP_ACTION_LAST_OBJECT,
                controller_slot: &SYSTEM_QUIT_LOAD_PROFILE_CONTROLLER_LAST_OBJECT,
            },
            ClonedRow {
                row: QuitRow::LoadSaveProfiles,
                label: &SYSTEM_QUIT_LOAD_SAVE_PROFILES_LABEL_W,
                // Mode-locked at row-build time so the row never advertises the save flavor the
                // active mode ignores (user directive 2026-07-06).
                help: if seamless_coop_loaded() {
                    SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_CO2_W.as_slice()
                } else {
                    SYSTEM_QUIT_LOAD_SAVE_PROFILES_HELP_W.as_slice()
                },
                action_slot: &SYSTEM_QUIT_OPEN_SAVE_DIR_ACTION_LAST_OBJECT,
                controller_slot: &SYSTEM_QUIT_OPEN_SAVE_DIR_CONTROLLER_LAST_OBJECT,
            },
            ClonedRow {
                row: QuitRow::LoadBuildFromUrl,
                label: &SYSTEM_QUIT_LOAD_BUILD_URL_LABEL_W,
                // The live buffer, not a constant: the link field rewrites it to say why an
                // accept was refused, and the row behind the field shows that.
                help: build_url_row_help_wide(),
                action_slot: &SYSTEM_QUIT_LOAD_BUILD_URL_ACTION_LAST_OBJECT,
                controller_slot: &SYSTEM_QUIT_LOAD_BUILD_URL_CONTROLLER_LAST_OBJECT,
            },
            ClonedRow {
                row: QuitRow::GenerateBuildLink,
                label: &SYSTEM_QUIT_GENERATE_BUILD_LINK_LABEL_W,
                // Also a live buffer, for a different reason: this row opens no field, so when its
                // export finishes there is no other surface to report on. The row reports on itself.
                help: generate_build_link_row_help_wide(),
                action_slot: &SYSTEM_QUIT_GENERATE_BUILD_LINK_ACTION_LAST_OBJECT,
                controller_slot: &SYSTEM_QUIT_GENERATE_BUILD_LINK_CONTROLLER_LAST_OBJECT,
            },
        ];

        let mut any_row_added = false;
        let mut row_log = String::new();
        // Only the rows this load armed. A row left out is not cloned at all, so the flow behind it
        // -- which a standalone shell does not have -- can never be reached by a press.
        let armed = row_set();
        for cloned in cloned_rows
            .iter()
            .filter(|cloned| armed.includes(cloned.row))
        {
            // The scratch component lives on this stack frame for exactly as long as the native
            // call needs it: `CS::MenuString` keeps the label pointer (which is why the arrays are
            // `const`/process-lifetime), while the component wrapper itself is destructed the
            // instant AddCancelButton returns.
            let mut label_storage =
                std::mem::MaybeUninit::<SystemQuitMenuHelpLabelScratch>::uninit();
            let label = label_storage.as_mut_ptr() as usize;
            let built = unsafe {
                system_quit_build_static_label_component(label, cloned.label, cloned.help)
            };
            if !built {
                row_log.push_str(&format!(" {}=LABEL-BUILD-FAILED", cloned.row.label()));
                continue;
            }
            let row_ret = unsafe { original(dialog, label, action_fn, enabled_fn, keyguide_fn) };
            unsafe { label_dtor(label) };
            after_final = unsafe {
                safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET)
            }
            .unwrap_or(after_final);
            let row_index = after_final.saturating_sub(1);
            let row_addr = aligned_properties + EDIT_PROPERTY_SIZE.saturating_mul(row_index);
            let controller =
                unsafe { safe_read_usize(row_addr + EDIT_PROPERTY_CONTROLLER_OFFSET) }.unwrap_or(0);
            let action = if controller != 0 {
                unsafe {
                    safe_read_usize(
                        controller + PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET,
                    )
                }
                .unwrap_or(0)
            } else {
                0
            };
            if action != 0 {
                cloned.action_slot.store(action, Ordering::SeqCst);
            }
            // The property index is only recorded once a controller was actually read back. A row
            // whose controller is 0 was not really appended, and recording its index would complete
            // the row table with a lie -- which the resolver would then trust.
            if controller != 0 {
                cloned.controller_slot.store(controller, Ordering::SeqCst);
                system_quit_row_table_record_index(cloned.row, row_index);
                any_row_added = true;
            }
            row_log.push_str(&format!(
                " {}=#{row_index}:ret=0x{row_ret:x}:row=0x{row_addr:x}:controller=0x{controller:x}:action_alias=0x{action:x}",
                cloned.row.label()
            ));
        }
        if any_row_added {
            SYSTEM_QUIT_DUPLICATE_COUNT.fetch_add(1, Ordering::SeqCst);
        }
        // Raise the list widget's item count through the native setter, not by poking the field.
        // `GridControl::SetItemCount` writes `+0xd0` and recomputes the scroll/page row count on the
        // embedded scroll control at `+0x1a8` -- exactly what the native rebuild `FUN_140975040`
        // calls. A raw field write left that scroll control still describing the two-row list, which
        // is the state the vertical movement clamp reads.
        let prior_bound =
            unsafe { safe_read_i32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET) }.unwrap_or(-1);
        let new_bound = (after_final.min(i32::MAX as usize)) as i32;
        let set_item_count = game_rva(GRID_CONTROL_SET_ITEM_COUNT_RVA).ok();
        if new_bound > prior_bound {
            match set_item_count {
                Some(addr) => {
                    let set_count: unsafe extern "system" fn(usize, u32) =
                        unsafe { std::mem::transmute(addr) };
                    unsafe { set_count(dialog + DIALOG_GRID_CONTROL_A38_OFFSET, new_bound as u32) };
                }
                None => append_autoload_debug(format_args!(
                    "system-quit-dup: failed to resolve GridControl::SetItemCount rva 0x{GRID_CONTROL_SET_ITEM_COUNT_RVA:x}; the added rows stay outside the list cursor's range"
                )),
            }
        }
        let bound_after =
            unsafe { safe_read_i32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET) }.unwrap_or(-1);
        unsafe { system_quit_record_grid_geometry(dialog) };
        // The row table is the identity from here on: index + live label per row. Log it, and log the
        // label actually readable at each captured index so a run shows the table agreeing with
        // memory rather than being trusted.
        let table = SYSTEM_QUIT_ROW_TABLE_ROWS
            .map(|row| {
                let index = system_quit_row_table_index(row);
                let label = unsafe { system_quit_row_label_at(dialog, index) };
                format!("{}=#{index}:{label:?}", row.label())
            })
            .join(" ");
        append_autoload_debug(format_args!(
            "system-quit-dup: cloned Quit rows dialog=0x{dialog:x} count {before}->{after_native}->{after_final} cursor_bound {prior_bound}->{bound_after} ret=0x{ret:x} added={any_row_added};{row_log}; row table [{table}]"
        ));
    }

    SYSTEM_QUIT_DUPLICATE_LAST_COUNT_BEFORE.store(before, Ordering::SeqCst);
    SYSTEM_QUIT_DUPLICATE_LAST_COUNT_AFTER.store(after_final, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: routed native Quit rows dialog=0x{dialog:x} first_row_call={first_row_call} second_row_call={second_row_call} count {before}->{after_native}->{after_final}; native GameEnd GFx component preserved"
    ));
    ret
}

// ---- arming ----------------------------------------------------------------------------------

/// Why an arm call could not install the rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArmError {
    /// [`arm`] was already called in this process.
    AlreadyArmed,
    /// `AddCancelButton` could not be resolved on the running build, so nothing would be cloned.
    ClonerUnresolved,
    /// The union refused the row cloner's prologue.
    ClonerRefused,
}

static ARM_ONCE: AtomicUsize = AtomicUsize::new(0);

/// Install the row cloner and the row router, and publish which rows this load owns.
///
/// The single entry point for both hosts: the product calls it with [`RowSet::ALL`] and its real
/// flows, a standalone shell with [`RowSet::BUILD_ROWS_ONLY`] and none. That is what makes the
/// shell's path the same code the product runs rather than a second implementation of it.
///
/// Every detour goes through the `er-hook` union. `AddCancelButton` takes five arguments, so it
/// registers on the five-argument union; the three routing entry points take four or fewer and
/// register on the ordinary one.
///
/// # Safety
///
/// Process attach or startup-hook context, before the Quit tab has built a dialog.
pub unsafe fn arm(rows: RowSet, actions: QuitRowActions) -> Result<(), ArmError> {
    if ARM_ONCE
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(ArmError::AlreadyArmed);
    }
    let _ = ROW_ACTIONS.set(actions);
    publish_row_set(rows);

    // Unresolved on purpose: `register_union_hook5` owns the single 1.16.2 -> 1.17 resolve, so
    // there is no second translation for `scripts/check-double-resolved-hook-targets.py` to find.
    let Ok(cloner) = game_rva_for_hook(SYSTEM_QUIT_DUPLICATE_ADD_CANCEL_BUTTON_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve AddCancelButton rva 0x{SYSTEM_QUIT_DUPLICATE_ADD_CANCEL_BUTTON_RVA:x}"
        ));
        // Release the latch so a later call retries, which is what the product's install site did
        // before this moved: the flag it gates on is only raised on success, and an address that
        // would not resolve on one attempt can resolve on the next once the image is fully mapped.
        ARM_ONCE.store(0, Ordering::SeqCst);
        return Err(ArmError::ClonerUnresolved);
    };
    if let Err(status) = unsafe {
        er_hook::register_union_hook5(
            cloner,
            system_quit_duplicate_add_cancel_button_hook,
            &SYSTEM_QUIT_DUPLICATE_ORIG,
        )
    } {
        append_autoload_debug(format_args!(
            "system-quit-dup: register_union_hook5 AddCancelButton failed: {status:?} -- no rows will be cloned"
        ));
        ARM_ONCE.store(0, Ordering::SeqCst);
        return Err(ArmError::ClonerRefused);
    }
    append_autoload_debug(format_args!(
        "system-quit-dup: registered AddCancelButton 0x{cloner:x} on the 5-argument union; rows={rows:?}; will clone at caller rva {}",
        match er_title_flow::system_quit_row_return_rvas() {
            Some((first, second)) => format!("0x{first:x} (second row 0x{second:x})"),
            None => "UNRESOLVED on this build -- no rows will be cloned".to_owned(),
        }
    ));

    // The three routing entry points. Each installs or fails inside its own block: they used to
    // sit in one function that returned on the first refusal, so a single unresolved address took
    // the other two with it -- including the guard in front of the native Return-to-Desktop
    // confirm, which is the irreversible one. On 1.17 that was not hypothetical.
    'first: {
        let Ok(addr) = game_rva_for_hook(SYSTEM_QUIT_RETURN_TITLE_ACTION_DO_CALL_RVA) else {
            report_unresolved("first Quit-tab action invoke");
            break 'first;
        };
        report_routing_detour(
            "first Quit-tab action invoke",
            addr,
            unsafe {
                er_hook::register_union_hook(
                    addr,
                    union_first_row_action_hook,
                    &SYSTEM_QUIT_NOOP_ACTION_ORIG,
                )
            },
            &SYSTEM_QUIT_NOOP_ACTION_INSTALLED,
            SYSTEM_QUIT_NOOP_ACTION_INSTALLED_YES,
        );
    }
    'second: {
        let Ok(addr) = game_rva_for_hook(SYSTEM_QUIT_RETURN_DESKTOP_ACTION_DO_CALL_RVA) else {
            report_unresolved("second Quit-tab action invoke");
            break 'second;
        };
        report_routing_detour(
            "second Quit-tab action invoke",
            addr,
            unsafe {
                er_hook::register_union_hook(
                    addr,
                    union_second_row_action_hook,
                    &SYSTEM_QUIT_RETURN_DESKTOP_ACTION_ORIG,
                )
            },
            &SYSTEM_QUIT_RETURN_DESKTOP_ACTION_INSTALLED,
            SYSTEM_QUIT_RETURN_DESKTOP_ACTION_INSTALLED_YES,
        );
    }
    'activate: {
        let Ok(addr) = game_rva_for_hook(PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_RVA) else {
            report_unresolved("PropertyNewButtonController::Activate");
            break 'activate;
        };
        report_routing_detour(
            "PropertyNewButtonController::Activate",
            addr,
            unsafe {
                er_hook::register_union_hook(
                    addr,
                    union_controller_activate_hook,
                    &PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_ORIG,
                )
            },
            &PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_INSTALLED,
            PROPERTY_NEW_BUTTON_CONTROLLER_ACTIVATE_INSTALLED_YES,
        );
    }
    Ok(())
}

/// An address that would not resolve on the running build: that row stays vanilla, loudly.
fn report_unresolved(what: &str) {
    append_autoload_debug(format_args!(
        "system-quit-dup: failed to resolve {what}; that row stays vanilla"
    ));
}

/// Report one routing registration, and raise its installed oracle only on success.
///
/// The flag is what the telemetry writer reads back to say the row is routed at all, so it must be
/// raised here rather than assumed from the arm call returning: two of these three addresses are
/// the only thing standing between a cloned row's press and the irreversible native
/// Return-to-Desktop action, and a run has to be able to prove each one is live.
fn report_routing_detour(
    what: &str,
    addr: usize,
    outcome: Result<(), er_hook::MH_STATUS>,
    installed: &'static AtomicUsize,
    installed_yes: usize,
) {
    match outcome {
        Ok(()) => {
            installed.store(installed_yes, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "system-quit-dup: registered {what} 0x{addr:x} on the union"
            ))
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-dup: register_union_hook {what} failed: {status:?}; that row stays vanilla"
        )),
    }
}

// ---- union-shaped entry points ---------------------------------------------------------------
//
// The union dispatcher is four `usize` arguments returning `usize`, and the three routing targets
// are not: two take one argument, and the third takes four but declares its second as the `u32`
// the game really passes and returns nothing. Each therefore gets a wrapper of the dispatcher's
// exact shape, which is what `scripts/check-union-hook-abi.py` reads at the registration. The
// trailing registers a narrower target does not use are ignored, and a `usize` return on a `void`
// target leaves whatever was in `rax`, which the caller already discards.

/// # Safety
///
/// Installed by `er-hook`; the game calls it with one argument.
unsafe extern "system" fn union_first_row_action_hook(
    action_obj: usize,
    _b: usize,
    _c: usize,
    _d: usize,
) -> usize {
    unsafe { system_quit_noop_desktop_action_hook(action_obj) }
}

/// # Safety
///
/// Installed by `er-hook`; the game calls it with one argument.
unsafe extern "system" fn union_second_row_action_hook(
    action_obj: usize,
    _b: usize,
    _c: usize,
    _d: usize,
) -> usize {
    unsafe { system_quit_return_desktop_action_hook(action_obj) }
}

/// # Safety
///
/// Installed by `er-hook`; the game calls it with four arguments, the second a `u32` in `edx`.
unsafe extern "system" fn union_controller_activate_hook(
    controller: usize,
    event_kind: usize,
    event_a: usize,
    event_b: usize,
) -> usize {
    unsafe {
        property_new_button_controller_activate_hook(
            controller,
            event_kind as u32,
            event_a,
            event_b,
        )
    };
    0
}
