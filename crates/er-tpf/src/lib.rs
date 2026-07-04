//! Tier-0/Tier-1 **in-memory texture-payload builder** for Elden Ring's raster
//! pipeline. This is the raster analog of `er-gfx`'s Scaleform MemoryFile codec:
//! it emits the **uncompressed, post-Oodle-decompress** bytes the game's own
//! in-memory TPF path consumes, and it builds **bytes only** -- it never calls
//! the game, never touches disk, and never constructs a C struct.
//!
//! # Two tiers
//!
//! * **Tier 0 -- DDS encoder** ([`DdsImage`]). Encodes a `width x height` RGBA8
//!   pixel buffer into an uncompressed `R8G8B8A8_UNORM` (DXGI format `28`) DDS
//!   blob: the `DDS ` magic, the 124-byte `DDS_HEADER`, a `DDS_HEADER_DXT10`,
//!   then the raw pixel bytes (single mip). Layout follows the Microsoft DDS
//!   programming guide exactly so the byte assertions in the tests are spec
//!   citations, not guesses. Two header forms are available via
//!   [`DdsHeaderMode`]: the strict [`DdsHeaderMode::Dx10`] form (a
//!   `DDS_HEADER_DXT10` with `dxgiFormat = 28`, the default), and a legacy
//!   [`DdsHeaderMode::LegacyRgba8`] form (classic `DDS_PIXELFORMAT` RGBA bit
//!   masks, **no** `DDS_HEADER_DXT10`) that maps to the same DXGI `28` through
//!   the engine's legacy path and bypasses the DX10 format validator.
//! * **Tier 1 -- TPF003 wrap** ([`Tpf`]). Wraps one (or more) Tier-0 DDS blobs
//!   in an uncompressed TPF version-3 / PC (`TPFPlatform.PC`) container, mirroring
//!   the documented SoulsFormats `TPF` layout. The wrap is **never** Kraken/DCX
//!   compressed -- this crate emits only the decompressed in-memory form.
//!
//! # NEVER compressed
//!
//! This crate emits Kraken/DCX/Oodle data **nowhere**. The whole point is the
//! post-decompress blob; compression is a transport concern handled elsewhere.
//!
//! # Discipline (mirrors `er-gfx`)
//!
//! A small error enum ([`TpfError`]), a byte-builder plus a parser for each
//! tier, and **self round-trip tests** that assert `parse(build(x)) == x` over
//! the typed fields. Tier-0 additionally asserts exact bytes at known offsets.
//! Exact *game acceptance* of the TPF is a later runtime tier; Tier-1's gate
//! here is **self-consistency** (every offset in range, `dataOffset + dataSize`
//! within the blob, `totalTextureDataSize == sum of texture sizes`) plus the
//! typed round-trip -- not game validation.

use std::fmt;

mod draw;

include!("lib_parts/chunk_01.rs");
include!("lib_parts/chunk_02.rs");
