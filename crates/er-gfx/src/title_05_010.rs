//! Stats-panel layout transform for `data0:/menu/05_010_profileselect.gfx`.
//!
//! Derives the "stats-panel v1" ProfileSelect movie from the **vanilla** movie
//! by applying [`TITLE_05_010_STATS_EDITS`]: HIDES the 128x128 face box
//! (`Icon_0`) via an alpha-0 color transform -- kept PLACED so the native
//! row-populate can still resolve/release it (unplacing it crashes,
//! er-effects-rs-7e7) -- repurposes the icon-frame deco (char 67, placed
//! nowhere else) as a left-aligned `MenuFont_01` `DefineEditText`, PLACED TWICE
//! so the eight attributes split across the row's two text lines
//! ([`STATS_FIELD_NAME_TOP`] at y=-48, [`STATS_FIELD_NAME_BOTTOM`] at y=15), and
//! shifts PlayerName / the Level FMG caption / the Level value field left into
//! the freed strip.
//! Location and PlayTime keep their native placements; the DLL pushes the
//! attribute line onto the new field natively (SetText) at row-populate time,
//! as Scaleform HTML so each label is dimmed and each value gets a distinct
//! color (the SetText core dispatches with `bHTML=1`).
//!
//! The edit table is generated -- never hand-edited -- by
//! `cargo run -p er-gfx --example make_05_010_stats -- <vanilla> <edited>` then
//! `python3 scripts/gfx_tag_diff.py <vanilla> <edited> --emit-rust TITLE_05_010_STATS_EDITS`.
//!
//! All-or-nothing exactly like [`crate::title_05_000`]: for the known vanilla
//! input the output is verified against the edited-asset fingerprint; for an
//! unknown input (game update, another mod's asset) the edits either apply
//! cleanly in full or the caller serves its input untouched.

use crate::edit::{EditError, EditOp, TagEdit, apply_edits};
use crate::title_05_000::fnv1a64;
use crate::{GfxError, Movie};

include!("title_05_010_edits.rs");

/// Instance names of the two injected per-row stats text fields (the eight
/// attributes are split across the row's two text lines: the first four on the
/// TOP line, the last four on the BOTTOM line). Single source of truth: the DLL
/// resolves each row child by these names for its native SetText push, and the
/// generator example bakes them into the placement tags. They must keep matching
/// NO engine populate prefix (`StaticText_*`/`StaticRegionText_*`/
/// `StaticLineHelp_*`/`StaticSystemText_*`/`StaticDialogText_*`/`StaticKeyGuide_*`/
/// `Dynamic`+`KeyIcon_`) so only the DLL ever writes them.
pub const STATS_FIELD_NAME_TOP: &str = "ErStatsTop";
pub const STATS_FIELD_NAME_BOTTOM: &str = "ErStatsBottom";

/// Length of the known vanilla (1.16.1) `05_010_profileselect.gfx`.
pub const VANILLA_LEN: usize = 14388;
/// [`fnv1a64`] of the known vanilla movie.
pub const VANILLA_FNV1A64: u64 = 0xfc22_4f43_7a73_13f3;
/// Length of the stats-panel output for the known vanilla input.
pub const EDITED_LEN: usize = 14447;
/// [`fnv1a64`] of the stats-panel output for the known vanilla input.
pub const EDITED_FNV1A64: u64 = 0x9349_9cf1_c6a2_db89;

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
