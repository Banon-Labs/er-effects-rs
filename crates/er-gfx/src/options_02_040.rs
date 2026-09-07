//! Runtime-derived 6-button Quit Game layout transform for `data0:/menu/win/02_040_optionsetting.gfx`.
//!
//! This does **not** ship a game-derived GFx file. The DLL reads the game's own
//! Scaleform MemoryFile, applies these content-addressed tag edits in memory, and
//! serves the derived movie for that process. The edit extends the native
//! `MENU_FL_QuitGame` sprite (id 138) from two button instances to six while
//! preserving the native GameEnd/portrait component and avoiding the multi-slot
//! component-index swap that poisons the shared OptionSetting GFx list.
//!
//! # The four added cells fill a 2x3 grid, and that is the whole navigation model
//!
//! `CS::GridControl` (the list widget embedded at `GenericListSelectDialog + 0xa38`)
//! does not take its geometry from the property list -- it measures it from the movie.
//! `GridControl::MeasureGridFromMovie` (vtable `+0x18`, 1.16.2 `FUN_140737c60`, called
//! once from `FUN_14077ef30` while the dialog is constructed) probes the child component
//! named `Item_<row>_<col>` (`"Item_%d_%d"` formatted `(row, col)` in `FUN_140736fc0`)
//! for row 0.., col 0.., takes `cols = max(col + 1)` and `rows = max(row + 1)`, and stops
//! at the first row whose column 0 is absent. Those two numbers then decide everything:
//!
//! * `GridControl::Update` (`FUN_1407392f0`) enables the vertical axis only when
//!   `rows >= 2`, and the horizontal axis only when `cols != 1 || rows < 2`.
//! * the mouse hit test (`FUN_140736c90`, reached from `FUN_14073a5c0`) walks exactly
//!   `cols * rows` cells, so a component outside the measured grid can never be hovered.
//! * the item index of a cell is `row * cols + col`.
//!
//! The native pair sits side by side -- `Item_0_0` at `tx = -3979` twips, `Item_0_1` at
//! `tx = +4780`, both `ty = 4500` -- so vanilla measures `cols = 2, rows = 1`: one
//! horizontal row, no vertical axis. Naming the added cells `Item_0_2`/`Item_0_3`
//! (a previous form of this edit) measured `cols = 4, rows = 1`: all four cells were
//! hoverable, but up/down was disabled outright and left/right had to walk the whole
//! strip. Naming them by row instead fills a rectangle: `Item_1_0`/`Item_1_1` at
//! `ty = 5600` and `Item_2_0`/`Item_2_1` at `ty = 6700`, each reusing one of the two
//! native columns (`tx = -3979` and `tx = +4780`) and each row one 1100-twip (55px) step
//! below the last. That measures `cols = 2, rows = 3`: both axes live, every cell inside
//! the hit test, and `row * cols + col` mapping them onto property indices 0..5 in the
//! order the rows are appended -- Save Game, Return to Desktop, Load Character, Load
//! Character from File, Load Build from URL, Generate Build Link. The native placement
//! matrices are untouched, so nothing that was already on screen moves.
//!
//! # six items fill the 2x3 grid exactly, which is why this is the easy case
//!
//! `cols * rows == 6` and `GridControl::SetItemCount` is given 6, so the two numbers
//! agree and every cell the engine probes for exists. There is no cell that can be
//! hovered but not selected, and none that can be selected but not hovered.
//!
//! This simplified the movie rather than complicating it. At five items the bottom row
//! was ragged, and three separate pieces of native behaviour had to be proven harmless:
//! the mouse hit test walked a sixth cell that was not there (safe only because
//! `FUN_14074b0d0` tests `(*(u32 *)(value + 0x20) & 0x8f) == 10` first and an absent
//! component answers `dataType == 0`); `SetItemCount` was given 5 while `cols * rows` was
//! 6, so the cursor was bounded by the smaller of two disagreeing numbers; and reaching
//! the bottom row by pad depended on `FUN_14073bae0` answering `2` for an out-of-range
//! index and `FUN_14073b0c0` walking back along the row a column at a time. All of that
//! reasoning was correct, and all of it is now moot. Nothing here relies on it.
//!
//! The measure loop still terminates the same way it always did -- by probing row 3
//! column 0, finding nothing, and destructing the invalid value it gets back. That absent
//! probe happens on every dialog including vanilla; it is the loop's exit condition, not a
//! thing this edit introduces.
//!
//! # The label field is 400px, and the labels were measured against it
//!
//! Each cell (`char 129`) shows its label through `Text_0` -> sprite 96 -> `DefineEditText`
//! char 95: bounds -40..7960 twips = **400px** wide, `MenuFont_01` at 480 twips = **24px**,
//! center-aligned, and crucially `wordwrap = false, multiline = false, autosize = false` --
//! so a label wider than the field clips rather than wrapping, losing its tail silently.
//! `scripts/gfx_text_width.py --height-px 24 --box-px 400` sums that font's own advance
//! table: "Save Game" 103.1px, "Return to Desktop" 172.8px, "Load Character" 144.5px,
//! "Load Character from File" 234.6px, "Load Build from URL" 203.7px, "Generate Build
//! Link" 189.9px. Any future relabel goes through that tool before it goes in; widening
//! the field or moving a matrix is not the answer, because these placements are what
//! makes the 2x3 measure work.

use crate::edit::{EditError, EditOp, TagEdit, apply_edits};
use crate::{GfxError, Movie};
use er_game_base::fnv1a::fnv1a64;

include!("options_02_040_quit6_edits.rs");

pub const VANILLA_WIN_LEN: usize = 44007;
pub const VANILLA_WIN_FNV1A64: u64 = 0x570d_8549_2c03_72a0;
pub const QUIT6_WIN_LEN: usize = 44107;
pub const QUIT6_WIN_FNV1A64: u64 = 0x4c54_7513_773a_fd59;

/// The six grid cell names the derived movie must expose in sprite 138, in item-index order
/// (`row * cols + col` with the measured `cols = 2`). Asserted by the er-gfx integration test: the
/// whole navigation/hover model of the patched Quit tab is these six strings.
pub const QUIT6_GRID_CELL_NAMES: [&str; 6] = [
    "Item_0_0", "Item_0_1", "Item_1_0", "Item_1_1", "Item_2_0", "Item_2_1",
];

pub fn is_known_vanilla_win(bytes: &[u8]) -> bool {
    bytes.len() == VANILLA_WIN_LEN && fnv1a64(bytes) == VANILLA_WIN_FNV1A64
}

/// Column cap of the native measure loop (`iVar15 < 0x20`).
pub const GRID_MAX_COLS: u32 = 32;
/// Row cap of the native measure loop (`0x3f < iVar17` ends it).
pub const GRID_MAX_ROWS: u32 = 64;

/// `CS::GridControl::MeasureGridFromMovie` (1.16.2 `FUN_140737c60`), as pure arithmetic over "does
/// the movie contain a child component named `Item_<row>_<col>`".
///
/// The native loop starts from the constructor's `cols = rows = 1`, walks columns of a row until one
/// is missing, raises `cols`/`rows` to the highest index reached `+ 1`, then advances to the next row
/// -- and stops as soon as a row has no column 0 at all. `has_cell` must answer for the movie the
/// dialog was built against.
pub fn measure_grid(has_cell: impl Fn(u32, u32) -> bool) -> (u32, u32) {
    let mut cols = 1;
    let mut rows = 1;
    let mut row = 0;
    loop {
        let mut col = 0;
        while col < GRID_MAX_COLS && has_cell(row, col) {
            col += 1;
            cols = cols.max(col);
            rows = rows.max(row + 1);
        }
        if col == 0 {
            return (cols, rows);
        }
        row += 1;
        if row >= GRID_MAX_ROWS {
            return (cols, rows);
        }
    }
}

/// Item index of the cell at `(row, col)` -- the same `row * cols + col` the native hit test
/// (`FUN_140736c90`) and cell lookup (`FUN_140736e30`) use once `rows != 1`.
pub fn grid_item_index(row: u32, col: u32, cols: u32) -> u32 {
    row * cols + col
}

/// Whether `GridControl::Update` (`FUN_1407392f0`) will act on an up/down input. The vertical branch
/// is reached only when the measured grid has at least two rows.
pub fn grid_vertical_axis_enabled(_cols: u32, rows: u32) -> bool {
    rows >= 2
}

/// Whether `GridControl::Update` will act on a left/right input: `cols != 1 || rows < 2`, i.e. a
/// single-column grid of two or more rows is vertical-only.
pub fn grid_horizontal_axis_enabled(cols: u32, rows: u32) -> bool {
    cols != 1 || rows < 2
}

#[derive(Clone, Debug)]
pub enum Quit6Error {
    Parse(GfxError),
    Edit(EditError),
    Write(GfxError),
    KnownInputBadOutput { out_len: usize, out_fnv1a64: u64 },
}

impl core::fmt::Display for Quit6Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Quit6Error::Parse(e) => write!(f, "parse: {e}"),
            Quit6Error::Edit(e) => write!(f, "edit: {e}"),
            Quit6Error::Write(e) => write!(f, "write: {e}"),
            Quit6Error::KnownInputBadOutput {
                out_len,
                out_fnv1a64,
            } => write!(
                f,
                "known vanilla input but output len={out_len} fnv=0x{out_fnv1a64:016x} != expected len={QUIT6_WIN_LEN} fnv=0x{QUIT6_WIN_FNV1A64:016x}"
            ),
        }
    }
}

impl std::error::Error for Quit6Error {}

pub fn quit6(vanilla: &[u8]) -> Result<Vec<u8>, Quit6Error> {
    let mut movie = Movie::parse(vanilla).map_err(Quit6Error::Parse)?;
    apply_edits(&mut movie, OPTIONS_02_040_QUIT6_EDITS).map_err(Quit6Error::Edit)?;
    let out = movie.write().map_err(Quit6Error::Write)?;
    if is_known_vanilla_win(vanilla)
        && (out.len() != QUIT6_WIN_LEN || fnv1a64(&out) != QUIT6_WIN_FNV1A64)
    {
        return Err(Quit6Error::KnownInputBadOutput {
            out_len: out.len(),
            out_fnv1a64: fnv1a64(&out),
        });
    }
    Ok(out)
}
