//! Tier-0/Tier-1 lossless codec for uncompressed Scaleform **GFX** movies (the
//! `.gfx` files shipping in Elden Ring's `menu/` tree, magic `b"GFX"`, version
//! `0x0b`).
//!
//! # Goal
//!
//! Read ANY such `.gfx` and re-serialize it **byte-for-byte identical**. We do
//! that by structurally modelling the file header, the `DefineSprite` (code 39)
//! nesting, the `End` (code 0) terminator, plus a growing set of **Tier-1**
//! "trivial" tags that carry no bitstream and re-encode losslessly from typed
//! fields (see [`Tag`]). Every other tag is still treated as opaque
//! [`Tag::Unknown`] whose body bytes are re-emitted verbatim. Tag *lengths* are
//! always recomputed by the writer (never copied from the source), so
//! structurally-derived fields (FileLength, every `RecordHeader`) are
//! regenerated rather than echoed.
//!
//! # Tier-1 typed tags
//!
//! The promoted tags below are re-encoded **field-by-field** (not from a stored
//! raw copy); each was proven byte-identical across the full 114-file corpus
//! before promotion. Each typed variant carries its own [`force_long`](Tag) bit
//! so the exact `RecordHeader` form is reproduced (the GFX exporter is not
//! length-deterministic; see below). String fields are stored decoded but are
//! always re-emitted with their terminating NUL, and variable tags assert they
//! consume their declared body exactly so any future structural divergence
//! fails loudly at parse rather than silently producing wrong bytes.
//!
//! # RecordHeader long/short form decision (load-bearing for byte-identity)
//!
//! A `RecordHeader` is a little-endian `u16` where `code = word >> 6` and
//! `len = word & 0x3f`. When `len == 0x3f`, a `u32` "long" length follows.
//! Short form can encode body lengths `0..=0x3e`; long form can encode any
//! length but is *mandatory* only for lengths `>= 0x3f`.
//!
//! Measured over the real corpus, the exporter is **not** length-deterministic:
//! 14,766 tags use the long form even though their body is `<= 0x3e` (e.g. tag
//! codes 26 `PlaceObject2` and 70 `PlaceObject3` appear in BOTH forms with the
//! same small length, so the choice is not even per-tag-code). To guarantee
//! byte-identity we therefore record a per-tag [`force_long`](Tag) bit at parse
//! time and reproduce the exact form on write. We never shorten a source's
//! needlessly-long header. This is option (a) from the task brief.
//!
//! The `End` tag (code 0) is always short (`0x0000`) across the entire corpus;
//! we encode it as such and reject a long-form End as malformed so a regression
//! would fail loudly rather than silently diverge.

use std::fmt;

pub mod announce_notice;
pub mod arts_badge;
pub mod edit;
pub mod options_02_040;
pub mod profile_05_010_layout;
pub mod profile_05_010_protocol;
pub mod raster;
pub mod text_input_02_990;
pub mod title_05_000;
pub mod title_05_010;
pub mod world_map_pin;

/// Twips per pixel. SWF/GFX stores every geometric quantity -- RECT bounds, matrix
/// translations, font heights, and a live `GFx::TextField` document's source/layout bounds -- in
/// twips, while this crate's layout schema and the editor UI speak pixels.
///
/// This is a single named constant because the conversion has gone missing in practice. The
/// live-apply path wrote a schema pixel width straight into the text document's twips source
/// bound, making `PlayerName`'s box 1200 twips (60 px) instead of 1200 px; the name then
/// word-wrapped and only its first line fit the field, which reads on screen as a truncated
/// character name. Prefer this over re-declaring a local `TW`.
pub const TWIPS_PER_PIXEL: i32 = 20;
/// [`TWIPS_PER_PIXEL`] for the float paths (live text-document bounds are `f32`).
pub const TWIPS_PER_PIXEL_F32: f32 = TWIPS_PER_PIXEL as f32;

// Tag code for `DefineSprite`. Its body is `spriteId: u16`, `frameCount: u16`,
// then a NESTED tag stream parsed with the same parser and terminated by its
// own `End(0)`. (A plain comment, not a doc comment: `include!` takes no docs.)
include!("codec/tag_codes.rs");
include!("codec/types.rs");
include!("codec/io.rs");
include!("codec/shape.rs");
include!("codec/text.rs");
include!("codec/movie.rs");
include!("codec/writer.rs");
include!("codec/tests.rs");
