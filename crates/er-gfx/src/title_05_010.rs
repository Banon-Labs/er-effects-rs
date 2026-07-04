//! Stats-panel layout transform for `data0:/menu/05_010_profileselect.gfx`.
//!
//! Derives the "stats-panel v1" ProfileSelect movie from the **vanilla** movie
//! by applying [`TITLE_05_010_STATS_EDITS`]: removes the 128x128 face box
//! (`Icon_0`) from the row template (user direction 2026-07-04), repurposes
//! the icon-frame deco (char 67, placed nowhere else) as a new 630x40
//! left-aligned 22px `MenuFont_01` `DefineEditText` placed as
//! [`STATS_FIELD_NAME`] on the row's second text line, and shifts PlayerName /
//! the Level FMG caption / the Level value field left into the freed strip.
//! Location and PlayTime keep their native placements; the DLL pushes the
//! attribute line onto the new field natively (SetText) at row-populate time.
//!
//! The edit table is generated -- never hand-edited -- by
//! `cargo run -p er-gfx --example make_05_010_stats -- <vanilla> <edited>` then
//! `python3 scripts/gfx_tag_diff.py <vanilla> <edited> --emit-rust TITLE_05_010_STATS_EDITS`.
//!
//! All-or-nothing exactly like [`crate::title_05_000`]: for the known vanilla
//! input the output is verified against the edited-asset fingerprint; for an
//! unknown input (game update, another mod's asset) the edits either apply
//! cleanly in full or the caller serves its input untouched.

use crate::edit::{EditError, TagEdit, apply_edits};
use crate::title_05_000::fnv1a64;
use crate::{GfxError, Movie};

include!("title_05_010_edits.rs");

/// Instance name of the injected per-row stats text field. Single source of
/// truth: the DLL resolves the row child by this name for its native SetText
/// push, and the generator example bakes it into the placement tag. It must
/// keep matching NO engine populate prefix (`StaticText_*`/`StaticRegionText_*`/
/// `StaticLineHelp_*`/`StaticSystemText_*`/`StaticDialogText_*`/
/// `StaticKeyGuide_*`/`Dynamic`+`KeyIcon_`) so only the DLL ever writes it.
pub const STATS_FIELD_NAME: &str = "ErStats";

/// Length of the known vanilla (1.16.1) `05_010_profileselect.gfx`.
pub const VANILLA_LEN: usize = 14388;
/// [`fnv1a64`] of the known vanilla movie.
pub const VANILLA_FNV1A64: u64 = 0xfc22_4f43_7a73_13f3;
/// Length of the stats-panel output for the known vanilla input.
pub const EDITED_LEN: usize = 14389;
/// [`fnv1a64`] of the stats-panel output for the known vanilla input.
pub const EDITED_FNV1A64: u64 = 0xf6ae_e75a_54e0_eccf;

/// True iff `bytes` is the known vanilla movie the edit table was derived from
/// (and for which the output is proven byte-identical to the generated asset).
pub fn is_known_vanilla(bytes: &[u8]) -> bool {
    bytes.len() == VANILLA_LEN && fnv1a64(bytes) == VANILLA_FNV1A64
}

/// Why [`stats_panel`] could not produce an edited movie.
#[derive(Clone, Debug)]
pub enum StatsPanelError {
    /// The input did not parse as an uncompressed GFX movie.
    Parse(GfxError),
    /// The edit set did not apply cleanly (all-or-nothing; input untouched).
    Edit(EditError),
    /// Re-serialization failed after editing.
    Write(GfxError),
    /// The input was the known vanilla movie but the output did not match the
    /// generated-asset fingerprint -- codec or edit-table regression; do not
    /// serve the output.
    KnownInputBadOutput { out_len: usize, out_fnv1a64: u64 },
}

impl core::fmt::Display for StatsPanelError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StatsPanelError::Parse(e) => write!(f, "parse: {e}"),
            StatsPanelError::Edit(e) => write!(f, "edit: {e}"),
            StatsPanelError::Write(e) => write!(f, "write: {e}"),
            StatsPanelError::KnownInputBadOutput {
                out_len,
                out_fnv1a64,
            } => write!(
                f,
                "known vanilla input but output len={out_len} fnv=0x{out_fnv1a64:016x} != expected len={EDITED_LEN} fnv=0x{EDITED_FNV1A64:016x}"
            ),
        }
    }
}

impl std::error::Error for StatsPanelError {}

/// Parse `vanilla`, apply the stats-panel edit set, re-serialize.
/// All-or-nothing: any failure returns an error and the caller should serve
/// its input untouched. When the input is [`is_known_vanilla`], the output is
/// verified against the generated-asset fingerprint before being returned.
pub fn stats_panel(vanilla: &[u8]) -> Result<Vec<u8>, StatsPanelError> {
    let mut movie = Movie::parse(vanilla).map_err(StatsPanelError::Parse)?;
    apply_edits(&mut movie, TITLE_05_010_STATS_EDITS).map_err(StatsPanelError::Edit)?;
    let out = movie.write().map_err(StatsPanelError::Write)?;
    if is_known_vanilla(vanilla) && (out.len() != EDITED_LEN || fnv1a64(&out) != EDITED_FNV1A64) {
        return Err(StatsPanelError::KnownInputBadOutput {
            out_len: out.len(),
            out_fnv1a64: fnv1a64(&out),
        });
    }
    Ok(out)
}
