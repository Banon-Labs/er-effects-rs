//! Runtime-derived 4-button Quit Game layout transform for `data0:/menu/win/02_040_optionsetting.gfx`.
//!
//! This does **not** ship a game-derived GFx file. The DLL reads the game's own
//! Scaleform MemoryFile, applies these content-addressed tag edits in memory, and
//! serves the derived movie for that process. The edit extends the native
//! `MENU_FL_QuitGame` sprite (id 138) from two button instances
//! (`Item_0_0`/`Item_0_1`) to four (`Item_0_0`..`Item_0_3`) while preserving the
//! native GameEnd/portrait component and avoiding the multi-slot component-index
//! swap that poisons the shared OptionSetting GFx list.

use crate::edit::{EditError, EditOp, TagEdit, apply_edits};
use crate::title_05_000::fnv1a64;
use crate::{GfxError, Movie};

include!("options_02_040_quit4_edits.rs");

pub const VANILLA_WIN_LEN: usize = 44007;
pub const VANILLA_WIN_FNV1A64: u64 = 0x570d_8549_2c03_72a0;
pub const QUIT4_WIN_LEN: usize = 44057;
pub const QUIT4_WIN_FNV1A64: u64 = 0xd66f_c0d3_1b17_ef5e;

pub fn is_known_vanilla_win(bytes: &[u8]) -> bool {
    bytes.len() == VANILLA_WIN_LEN && fnv1a64(bytes) == VANILLA_WIN_FNV1A64
}

#[derive(Clone, Debug)]
pub enum Quit4Error {
    Parse(GfxError),
    Edit(EditError),
    Write(GfxError),
    KnownInputBadOutput { out_len: usize, out_fnv1a64: u64 },
}

impl core::fmt::Display for Quit4Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Quit4Error::Parse(e) => write!(f, "parse: {e}"),
            Quit4Error::Edit(e) => write!(f, "edit: {e}"),
            Quit4Error::Write(e) => write!(f, "write: {e}"),
            Quit4Error::KnownInputBadOutput {
                out_len,
                out_fnv1a64,
            } => write!(
                f,
                "known vanilla input but output len={out_len} fnv=0x{out_fnv1a64:016x} != expected len={QUIT4_WIN_LEN} fnv=0x{QUIT4_WIN_FNV1A64:016x}"
            ),
        }
    }
}

impl std::error::Error for Quit4Error {}

pub fn quit4(vanilla: &[u8]) -> Result<Vec<u8>, Quit4Error> {
    let mut movie = Movie::parse(vanilla).map_err(Quit4Error::Parse)?;
    apply_edits(&mut movie, OPTIONS_02_040_QUIT4_EDITS).map_err(Quit4Error::Edit)?;
    let out = movie.write().map_err(Quit4Error::Write)?;
    if is_known_vanilla_win(vanilla)
        && (out.len() != QUIT4_WIN_LEN || fnv1a64(&out) != QUIT4_WIN_FNV1A64)
    {
        return Err(Quit4Error::KnownInputBadOutput {
            out_len: out.len(),
            out_fnv1a64: fnv1a64(&out),
        });
    }
    Ok(out)
}
