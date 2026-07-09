//! Product transform for `data0:/menu/05_001_title_logo.gfx`.
//!
//! The remaining offline-title flash/glare is an animated title-logo effect, not
//! a background-color seam. The vanilla movie places a top-level sprite at depth
//! 3 and then drives it through an alpha ramp (startup -> peak -> wind-down)
//! across the movie timeline. That sprite contains the title-logo/glare artwork;
//! removing the depth-3 display-list entries leaves the neutral full-stage base
//! movie but suppresses the animated flash effect.

use crate::title_05_000::fnv1a64;
use crate::{GfxError, Movie, Tag};

/// Length of the known vanilla (1.16.1) `05_001_title_logo.gfx`.
pub const VANILLA_LEN: usize = 2862;
/// [`fnv1a64`] of the known vanilla movie.
pub const VANILLA_FNV1A64: u64 = 0x43e4_fb3b_f4c6_2e8c;

/// True iff `bytes` is the known vanilla movie this transform was derived from.
pub fn is_known_vanilla(bytes: &[u8]) -> bool {
    bytes.len() == VANILLA_LEN && fnv1a64(bytes) == VANILLA_FNV1A64
}

/// Why [`suppress_title_logo_effect`] could not produce an edited movie.
#[derive(Clone, Debug)]
pub enum TitleLogoEffectError {
    /// The input did not parse as an uncompressed GFX movie.
    Parse(GfxError),
    /// The source movie did not contain the expected animated top-level depth-3 effect.
    MissingAnimatedEffect,
    /// Re-serialization failed after editing.
    Write(GfxError),
    /// The input was known vanilla but the output still contains the animated depth-3 effect.
    KnownInputBadOutput,
}

impl core::fmt::Display for TitleLogoEffectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            TitleLogoEffectError::Parse(e) => write!(f, "parse: {e}"),
            TitleLogoEffectError::MissingAnimatedEffect => {
                write!(f, "missing animated top-level depth-3 title-logo effect")
            }
            TitleLogoEffectError::Write(e) => write!(f, "write: {e}"),
            TitleLogoEffectError::KnownInputBadOutput => write!(
                f,
                "known vanilla input but output still contains animated depth-3 title-logo effect"
            ),
        }
    }
}

impl std::error::Error for TitleLogoEffectError {}

fn is_top_level_title_logo_effect_tag(tag: &Tag) -> bool {
    match tag {
        Tag::PlaceObject2 { depth, .. } | Tag::PlaceObject3 { depth, .. } => *depth == 3,
        Tag::RemoveObject2 { depth, .. } => *depth == 3,
        _ => false,
    }
}

fn animated_effect_tag_count(movie: &Movie) -> usize {
    movie
        .tags
        .iter()
        .filter(|tag| is_top_level_title_logo_effect_tag(tag))
        .count()
}

/// True iff `bytes` parses and no longer contains the animated top-level depth-3
/// title-logo/glare effect.
pub fn title_logo_effect_is_suppressed(bytes: &[u8]) -> bool {
    let Ok(movie) = Movie::parse(bytes) else {
        return false;
    };
    animated_effect_tag_count(&movie) == 0
}

/// Remove the animated title-logo/glare effect from `05_001_title_logo.gfx`.
///
/// This intentionally does not rewrite `SetBackgroundColor`: the user-visible
/// failure is the depth-3 animated effect ramp, not the movie background color.
pub fn suppress_title_logo_effect(vanilla: &[u8]) -> Result<Vec<u8>, TitleLogoEffectError> {
    let mut movie = Movie::parse(vanilla).map_err(TitleLogoEffectError::Parse)?;
    let removed = animated_effect_tag_count(&movie);
    if removed == 0 {
        return Err(TitleLogoEffectError::MissingAnimatedEffect);
    }
    movie
        .tags
        .retain(|tag| !is_top_level_title_logo_effect_tag(tag));
    let out = movie.write().map_err(TitleLogoEffectError::Write)?;
    if is_known_vanilla(vanilla) && !title_logo_effect_is_suppressed(&out) {
        return Err(TitleLogoEffectError::KnownInputBadOutput);
    }
    Ok(out)
}
