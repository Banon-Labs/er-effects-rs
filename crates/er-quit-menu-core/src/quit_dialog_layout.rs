//! RAM layout of the `PropertyEditDialog` the System>Quit tab builds, plus the two native input
//! predicates its rows are classified with.
//!
//! Moved out of `er-quickload`'s `constants::autoload_state` with the row-identity code that is
//! their only reader; that module now re-exports them from here, so each address and offset still
//! has exactly one declaration (`scripts/check-rva-alias-drift.py`).

use er_game_base::mem::safe_read_u16;

/// A null pointer, named.
///
/// Same value as the product's `er_title_flow::TITLE_OWNER_SCAN_START_ADDRESS` (`usize::MIN`).
/// It is not an address, so there is no reverse-engineering fact here that could drift between
/// the two spellings -- which is why this crate names its own rather than taking a dependency on
/// the title-flow crate for a zero.
const NULL_POINTER: usize = usize::MIN;

/// `PropertyEditDialog.properties.items`: 0x1260 + BasicViewItemList.items(+8).
pub const PROPERTY_EDIT_DIALOG_PROPERTIES_1268_OFFSET: usize = 0x1268;
/// `PropertyEditDialog.properties.items.count`: 0x1260 + BasicViewItemList.items(+8) +
/// DLFixedVector<EditProperty>.count(+0x888). Pure diagnostic read only.
pub const PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET: usize = 0x1af0;
pub const EDIT_PROPERTY_SIZE: usize = 0x88;
pub const EDIT_PROPERTY_CONTROLLER_OFFSET: usize = 0x78;
/// `CS::EditProperty.label` (a `CS::MenuHelpLabelComponent`, 0x70 bytes) whose first field is the
/// `MenuString`'s raw UTF-16 pointer -- `CS::MenuString::MenuString` stores the pointer it is handed,
/// so a row built from this DLL's static label arrays is identifiable by pointer equality, and every
/// row is identifiable by its text. This is the only per-row identity in the Quit dialog that the
/// engine does not alias or share (1.16.2 `EditProperty`: super_MenuViewItem +0, label +8,
/// propertyController +0x78, size 0x88).
pub const EDIT_PROPERTY_LABEL_OFFSET: usize = 0x8;
/// `CS::CSEzMenuViewerPad` predicates that `PropertyNewButtonController`'s should-invoke predicate
/// (`FUN_140974b00`, deobf 0x974b00) itself calls to classify the dispatched event. The first
/// short-circuits the predicate with no positional test (pad/keyboard confirm); the second is the one
/// whose result the native code then hit-tests against the row's display object (mouse click).
pub const MENU_VIEWER_PAD_CONFIRM_PRESSED_RVA: u32 =
    er_game_base::rva::MENU_VIEWER_PAD_CONFIRM_PRESSED_RVA;
pub const MENU_VIEWER_PAD_MOUSE_CLICKED_RVA: u32 =
    er_game_base::rva::MENU_VIEWER_PAD_MOUSE_CLICKED_RVA;
/// The `CS::GridControl` (0x7c8 bytes, vtable dump `0x142a913b8`) embedded in every
/// `GenericListSelectDialog` at `+0xa38`. Its geometry fields, measured once at dialog construction
/// by `GridControl::MeasureGridFromMovie` (vtable `+0x18`, `FUN_140737c60`) from which
/// `Item_<row>_<col>` components the movie actually contains:
///   `+0xd0` item count, `+0xd4` cursor, `+0xd8` columns, `+0xdc` rows.
/// `GridControl::Update` (`FUN_1407392f0`) enables up/down only at `rows >= 2` and left/right only at
/// `cols != 1 || rows < 2`, and the mouse hit test (`FUN_140736c90`) walks exactly `cols * rows`
/// cells -- so these two numbers are the whole navigation and hover model of the dialog.
pub const DIALOG_GRID_CONTROL_A38_OFFSET: usize = 0xa38;
pub const GRID_CONTROL_COLS_D8_OFFSET: usize = 0xd8;
pub const GRID_CONTROL_ROWS_DC_OFFSET: usize = 0xdc;

/// Does the UTF-16 string at `ptr` begin with `ascii`?
///
/// Reads one code unit at a time through the fault-safe reader, so an unmapped or garbage pointer
/// answers `false` instead of faulting. A row label is the only per-row identity the engine does
/// not alias, so this is a routing decision, not a diagnostic.
pub fn wide_ptr_starts_with_ascii(ptr: usize, ascii: &[u8]) -> bool {
    if ptr == NULL_POINTER || ascii.is_empty() {
        return false;
    }
    for (idx, &want) in ascii.iter().enumerate() {
        let Some(unit) = (unsafe { safe_read_u16(ptr + idx * 2) }) else {
            return false;
        };
        if unit != want as u16 {
            return false;
        }
    }
    true
}
