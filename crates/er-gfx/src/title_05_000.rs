//! Product strip transform for `data0:/menu/05_000_title.gfx` (er-effects-rs-h7x).
//!
//! Derives the validated "native-ui-stripped v3" title movie (removes the
//! PRESS ANY BUTTON / Continue-menu / footer / progress placements and the
//! golden Cursor glow, and forces the movie background from gray to black, preserving the GFx shell +
//! AS3 bindability) from the **vanilla** movie by applying [`TITLE_05_000_STRIP_EDITS`]: 15 tag removals
//! and 4 tag replacements, content-addressed by exact serialized bytes. For
//! the known vanilla input, the output fingerprint is derived by applying the
//! edits to the vanilla corpus fixture (fixture-gated test in `tests/title_strip.rs`);
//! for an unknown input (game update, another mod's asset) it either applies cleanly
//! in full or fails all-or-nothing so the caller can fall back to serving the input untouched.
//!
//! The edit table is generated -- never hand-edited -- by:
//! `python3 scripts/gfx_tag_diff.py <vanilla> <stripped-v3> --emit-rust TITLE_05_000_STRIP_EDITS`

use crate::edit::{EditError, EditOp, TagEdit, apply_edits};
use crate::{GfxError, Movie, Tag};

include!("title_05_000_edits.rs");

/// Length of the known vanilla (1.16.1) `05_000_title.gfx`.
pub const VANILLA_LEN: usize = 12174;
/// [`fnv1a64`] of the known vanilla movie.
pub const VANILLA_FNV1A64: u64 = 0x3b97_2bcf_60d0_44ff;
/// Length of the stripped output for the known vanilla input (the validated
/// v3 strip: v2 text/menu removals plus black SetBackgroundColor).
pub const STRIPPED_LEN: usize = 11707;
/// FNV-1a 64-bit content fingerprint (telemetry/identity checks, not crypto).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// True iff `bytes` is the known vanilla movie the edit table was derived from
/// (and for which the output is proven byte-identical to the validated asset).
pub fn is_known_vanilla(bytes: &[u8]) -> bool {
    bytes.len() == VANILLA_LEN && fnv1a64(bytes) == VANILLA_FNV1A64
}

/// Why [`strip`] could not produce a stripped movie.
#[derive(Clone, Debug)]
pub enum StripError {
    /// The input did not parse as an uncompressed GFX movie.
    Parse(GfxError),
    /// The edit set did not apply cleanly (all-or-nothing; input untouched).
    Edit(EditError),
    /// Re-serialization failed after editing.
    Write(GfxError),
    /// The input was the known vanilla movie but the output did not satisfy the
    /// structural invariants for the stripped product movie -- codec or edit-table
    /// regression; do not serve the output.
    KnownInputBadOutput {
        out_len: usize,
        background_rgb: Option<[u8; 3]>,
    },
}

impl core::fmt::Display for StripError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StripError::Parse(e) => write!(f, "parse: {e}"),
            StripError::Edit(e) => write!(f, "edit: {e}"),
            StripError::Write(e) => write!(f, "write: {e}"),
            StripError::KnownInputBadOutput {
                out_len,
                background_rgb,
            } => write!(
                f,
                "known vanilla input but output len={out_len} background_rgb={background_rgb:?} did not satisfy stripped invariants (expected len={STRIPPED_LEN}, black background)"
            ),
        }
    }
}

impl std::error::Error for StripError {}

fn strip_unvalidated(vanilla: &[u8]) -> Result<Vec<u8>, StripError> {
    let mut movie = Movie::parse(vanilla).map_err(StripError::Parse)?;
    apply_edits(&mut movie, TITLE_05_000_STRIP_EDITS).map_err(StripError::Edit)?;
    movie.write().map_err(StripError::Write)
}

/// Derive the stripped-output fingerprint from an input movie instead of storing
/// a magic fingerprint constant. Tests pass the known vanilla corpus movie here;
/// runtime callers can log this value for provenance after [`strip`] succeeds.
pub fn stripped_fnv1a64(vanilla: &[u8]) -> Result<u64, StripError> {
    Ok(fnv1a64(&strip_unvalidated(vanilla)?))
}

/// Top-level movie background color, if the movie parses and carries a
/// `SetBackgroundColor` tag.
pub fn background_rgb(bytes: &[u8]) -> Result<Option<[u8; 3]>, GfxError> {
    let movie = Movie::parse(bytes)?;
    Ok(movie.tags.iter().find_map(|tag| match tag {
        Tag::SetBackgroundColor { r, g, b, .. } => Some([*r, *g, *b]),
        _ => None,
    }))
}

/// Structural invariant for the product stripped title movie. This replaces the
/// former hard-coded stripped FNV constant: the exact fingerprint is derived in
/// tests from the vanilla corpus, while runtime fail-closed validation still
/// proves the output has the expected length and black background.
pub fn stripped_output_is_valid(bytes: &[u8]) -> bool {
    bytes.len() == STRIPPED_LEN && matches!(background_rgb(bytes), Ok(Some([0, 0, 0])))
}

/// Parse `vanilla`, apply the strip edit set, re-serialize. All-or-nothing:
/// any failure returns an error and the caller should serve its input
/// untouched. When the input is [`is_known_vanilla`], the output is verified
/// against structural product invariants before being returned.
pub fn strip(vanilla: &[u8]) -> Result<Vec<u8>, StripError> {
    let out = strip_unvalidated(vanilla)?;
    if is_known_vanilla(vanilla) && !stripped_output_is_valid(&out) {
        return Err(StripError::KnownInputBadOutput {
            out_len: out.len(),
            background_rgb: background_rgb(&out).ok().flatten(),
        });
    }
    Ok(out)
}
