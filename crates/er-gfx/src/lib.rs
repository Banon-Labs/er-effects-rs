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

use bitflags::bitflags;
use std::fmt;

pub mod edit;
pub mod equip_02_011;
pub mod options_02_040;
pub mod raster;
pub mod title_05_000;
pub mod title_05_010;

/// Tag code for `DefineSprite`. Its body is `spriteId: u16`, `frameCount: u16`,
/// then a NESTED tag stream parsed with the same parser and terminated by its
/// own `End(0)`.

include!("codec/tag_codes.rs");
include!("codec/types.rs");
include!("codec/io.rs");
include!("codec/shape.rs");
include!("codec/text.rs");
include!("codec/movie.rs");
include!("codec/writer.rs");
include!("codec/tests.rs");
