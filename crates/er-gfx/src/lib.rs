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

/// Tag code for `DefineSprite`. Its body is `spriteId: u16`, `frameCount: u16`,
/// then a NESTED tag stream parsed with the same parser and terminated by its
/// own `End(0)`.
const TAG_DEFINE_SPRITE: u16 = 39;
/// Tag code for `End` (terminates a tag stream).
const TAG_END: u16 = 0;

// --- Tier-1 "trivial" tag codes (no bitstream; re-encode from typed fields). ---
/// `ShowFrame`: advance the timeline one frame. Empty body.
const TAG_SHOW_FRAME: u16 = 1;
/// `SetBackgroundColor`: 3-byte RGB.
const TAG_SET_BACKGROUND_COLOR: u16 = 9;
/// `RemoveObject2`: a single `depth: u16`.
const TAG_REMOVE_OBJECT2: u16 = 28;
/// `FrameLabel`: NUL-terminated label + optional named-anchor byte.
const TAG_FRAME_LABEL: u16 = 43;
/// `FileAttributes`: a `u32` flags word (stored raw; bits not interpreted).
const TAG_FILE_ATTRIBUTES: u16 = 69;
/// `ImportAssets2`: URL string + 2 reserved bytes + `u16` count + entries.
const TAG_IMPORT_ASSETS2: u16 = 71;
/// `CSMTextSettings`: fixed-width font-rendering settings.
const TAG_CSM_TEXT_SETTINGS: u16 = 74;
/// `SymbolClass`: `u16` count + that many `(u16 tag, NUL-terminated name)`.
const TAG_SYMBOL_CLASS: u16 = 76;
/// `Metadata`: a single NUL-terminated string (typically XMP RDF).
const TAG_METADATA: u16 = 77;

// --- Tier-2 typed tag codes (carry bit-packed primitives). ---
/// `PlaceObject2` (code 26): the dominant display-list tag. Body is a `u8` flags
/// byte, a `u16` depth, then per-flag optional `characterId`, `MATRIX`,
/// `CXFORMWITHALPHA`, `ratio`, `name`, `clipDepth`, and (unmodelled) clipActions.
const TAG_PLACE_OBJECT2: u16 = 26;
/// `DefineScalingGrid` (code 78): a `u16` characterId followed by a `RECT` (the
/// nine-slice scaling grid).
const TAG_DEFINE_SCALING_GRID: u16 = 78;
/// `PlaceObject3` (code 70): like `PlaceObject2` plus a second flags byte that
/// adds an image bit, class name, bitmap cache, blend mode, a SURFACEFILTERLIST,
/// and a visible/background-color pair. See [`Tag::PlaceObject3`].
const TAG_PLACE_OBJECT3: u16 = 70;

// --- PlaceObject2 flag bits (MSB-to-LSB within the flags byte; SWF order). ---
/// `PlaceFlagMove`: this tag moves an existing object at `depth`. Stored only in
/// the raw `flags` byte (it gates no optional field), so it is not branched on;
/// retained as a named documented bit.
#[allow(dead_code)]
const PO2_MOVE: u8 = 0x01;
/// `PlaceFlagHasCharacter`: a `u16` characterId follows.
const PO2_HAS_CHARACTER: u8 = 0x02;
/// `PlaceFlagHasMatrix`: a `MATRIX` follows.
const PO2_HAS_MATRIX: u8 = 0x04;
/// `PlaceFlagHasColorTransform`: a `CXFORMWITHALPHA` follows.
const PO2_HAS_CXFORM: u8 = 0x08;
/// `PlaceFlagHasRatio`: a `u16` morph ratio follows.
const PO2_HAS_RATIO: u8 = 0x10;
/// `PlaceFlagHasName`: a NUL-terminated instance name follows.
const PO2_HAS_NAME: u8 = 0x20;
/// `PlaceFlagHasClipDepth`: a `u16` clip depth follows.
const PO2_HAS_CLIPDEPTH: u8 = 0x40;
/// `PlaceFlagHasClipActions`: a CLIPACTIONS block follows (unmodelled; see
/// [`Tag`] -- such a PlaceObject2 is kept as [`Tag::Unknown`]).
const PO2_HAS_CLIPACTIONS: u8 = 0x80;

// --- PlaceObject3 second flags byte (MSB-first SWF v10+ layout). Empirically,
// the Elden Ring corpus only ever sets HasFilterList/HasBlendMode/
// HasCacheAsBitmap/HasImage (HasImage bears no extra field); HasClassName,
// HasVisible, and the two reserved high bits never occur. We still model the
// className / visible+background fields for completeness, and treat the two
// reserved bits as a fall-back-to-[`Tag::Unknown`] signal (their semantics are
// unverifiable since the corpus never exercises them). ---
/// `PlaceFlagHasFilterList`: a SURFACEFILTERLIST follows.
const PO3_HAS_FILTERLIST: u8 = 0x01;
/// `PlaceFlagHasBlendMode`: a `u8` blend mode follows.
const PO3_HAS_BLENDMODE: u8 = 0x02;
/// `PlaceFlagHasCacheAsBitmap`: a `u8` bitmap-cache flag follows.
const PO3_HAS_CACHE_AS_BITMAP: u8 = 0x04;
/// `PlaceFlagHasClassName`: a NUL-terminated class name follows (right after
/// `depth`). Never set in the corpus, but modelled. Note: the SWF spec's
/// alternative "(HasImage AND HasCharacter)" class-name trigger is NOT honored
/// by this Scaleform exporter -- those bodies carry `characterId`+`MATRIX`
/// directly after `depth` with no class name (corpus-proven), so class name is
/// gated SOLELY by this bit.
const PO3_HAS_CLASSNAME: u8 = 0x08;
/// `PlaceFlagHasImage`: marks an image/bitmap placement. Bears NO extra field
/// (corpus-proven); the bit is preserved verbatim in `flags2`.
#[allow(dead_code)]
const PO3_HAS_IMAGE: u8 = 0x10;
/// `PlaceFlagHasVisible`: a `u8` visible flag + an `RGBA` background color
/// follow. Never set in the corpus, but modelled.
const PO3_HAS_VISIBLE: u8 = 0x20;
/// The two high bits of `flags2` (`OpaqueBackground` + a reserved bit). Never
/// set in the corpus; if either is set we fall back to [`Tag::Unknown`] rather
/// than guess an unverifiable layout.
const PO3_RESERVED_MASK: u8 = 0xc0;

// --- SURFACEFILTERLIST filter ids (only those present in the corpus are typed;
// any other id forces the whole PlaceObject3 back to [`Tag::Unknown`]). ---
/// `DropShadowFilter` filter id.
const FILTER_DROP_SHADOW: u8 = 0;
/// `GlowFilter` filter id.
const FILTER_GLOW: u8 = 2;

/// Short-form length sentinel: `len == 0x3f` means a `u32` long length follows.
const LONG_LEN_SENTINEL: u16 = 0x3f;
/// Maximum tag code representable in a `RecordHeader` (`u16 >> 6`).
const MAX_TAG_CODE: u16 = 0x3ff;
/// Expected file magic.
const MAGIC: [u8; 3] = *b"GFX";

/// File header preceding the tag stream.
///
/// The movie bounds `RECT` is bit-packed; to guarantee byte-identity without a
/// full bit-level RECT re-encoder, we capture its exact bytes verbatim in
/// [`Header::movie_rect_raw`] (we only parse its bit length to know how many
/// bytes to slice) and never re-encode it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// File format version (observed: `0x0b`).
    pub version: u8,
    /// The exact bit-packed movie `RECT` bytes, preserved verbatim.
    pub movie_rect_raw: Vec<u8>,
    /// Frame rate (8.8 fixed-point, stored as raw `u16`).
    pub frame_rate: u16,
    /// Declared frame count.
    pub frame_count: u16,
}

/// A single tag in a (possibly nested) tag stream.
///
/// Every variant that carries a body also carries a [`force_long`](Tag) bit that
/// records whether the source used the long `RecordHeader` form so the writer
/// can reproduce it exactly (see module docs).
///
/// `Eq` is intentionally **not** derived because [`Tag::CsmTextSettings`] holds
/// `f32` fields; equality is still available via the derived [`PartialEq`].
#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    /// `DefineSprite` (code 39): a sprite id, frame count, and a nested tag
    /// stream (which includes its own terminating [`Tag::End`]).
    DefineSprite {
        id: u16,
        frame_count: u16,
        tags: Vec<Tag>,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `End` (code 0): terminates the enclosing tag stream. Always short form.
    End,

    // --- Tier-1 typed tags (re-encoded from fields; see module docs). ---
    /// `ShowFrame` (code 1): advance the timeline one frame. Empty body.
    ShowFrame {
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `SetBackgroundColor` (code 9): the movie background `RGB` triple.
    SetBackgroundColor {
        r: u8,
        g: u8,
        b: u8,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `RemoveObject2` (code 28): remove the object at `depth`.
    RemoveObject2 {
        depth: u16,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `FileAttributes` (code 69): a flags word. The bits are stored raw and not
    /// interpreted (preserving any reserved/unknown bits exactly).
    FileAttributes {
        flags: u32,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `Metadata` (code 77): a single NUL-terminated string (typically the XMP
    /// RDF packet). The terminating NUL is implicit and always re-emitted.
    Metadata {
        xml: String,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `FrameLabel` (code 43): a NUL-terminated label and an OPTIONAL trailing
    /// named-anchor byte. `named_anchor` preserves the byte's presence/absence
    /// and value exactly (none present in the Tier-1 corpus, but modelled).
    FrameLabel {
        label: String,
        named_anchor: Option<u8>,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `SymbolClass` (code 76): `(tag, name)` exports. Order is preserved.
    SymbolClass {
        symbols: Vec<(u16, String)>,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `ImportAssets2` (code 71): a source `url`, 2 reserved bytes (stored raw,
    /// commonly `[0x01, 0x00]`), and `(tag, name)` imports.
    ImportAssets2 {
        url: String,
        reserved: [u8; 2],
        symbols: Vec<(u16, String)>,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `CSMTextSettings` (code 74): fixed-width font-rendering settings. The two
    /// `f32` fields are reproduced bit-exactly via [`f32::to_bits`].
    CsmTextSettings {
        character_id: u16,
        flags: u8,
        thickness: f32,
        sharpness: f32,
        reserved: u8,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },

    // --- Tier-2 typed tags (carry bit-packed primitives; see module docs). ---
    /// `PlaceObject2` (code 26): the dominant display-list tag. The `flags` byte
    /// is stored verbatim and governs which optional fields are present; each
    /// optional field below is `Some` iff its flag bit is set in `flags`.
    ///
    /// `clipActions` (flag `0x80`) is NOT modelled; a PlaceObject2 carrying it is
    /// kept as [`Tag::Unknown`] (none occur in the corpus -- 0 of 171,728).
    PlaceObject2 {
        /// Raw flags byte (`Move`/`HasCharacter`/`HasMatrix`/`HasColorTransform`/
        /// `HasRatio`/`HasName`/`HasClipDepth`/`HasClipActions`, LSB-to-MSB).
        flags: u8,
        depth: u16,
        /// `characterId` (flag `HasCharacter` `0x02`).
        character_id: Option<u16>,
        /// Placement `MATRIX` (flag `HasMatrix` `0x04`).
        matrix: Option<Matrix>,
        /// `CXFORMWITHALPHA` color transform (flag `HasColorTransform` `0x08`).
        color_transform: Option<CxformWithAlpha>,
        /// Morph `ratio` (flag `HasRatio` `0x10`).
        ratio: Option<u16>,
        /// Instance `name` (flag `HasName` `0x20`); NUL terminator implicit.
        name: Option<String>,
        /// `clipDepth` (flag `HasClipDepth` `0x40`).
        clip_depth: Option<u16>,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `PlaceObject3` (code 70): `PlaceObject2` plus a second flags byte. Both
    /// flag bytes are stored verbatim and govern which optional fields are
    /// present; each optional field below is `Some` iff its flag bit is set.
    ///
    /// Field order (corpus-proven byte-identical over all 5,360 instances):
    /// `depth`, `class_name` (`flags2` `HasClassName`), `character_id`
    /// (`flags1` `HasCharacter`), `matrix`, `color_transform`, `ratio`, `name`,
    /// `clip_depth`, `filters` (`flags2` `HasFilterList`), `blend_mode`
    /// (`HasBlendMode`), `bitmap_cache` (`HasCacheAsBitmap`), `visible`
    /// (`HasVisible`: a `u8` flag + `RGBA` background).
    ///
    /// A PlaceObject3 carrying `clipActions` (`flags1` `0x80`), a reserved
    /// `flags2` bit (`0xc0`), or an unmodelled SURFACEFILTERLIST filter id is
    /// kept as [`Tag::Unknown`] (none occur in the corpus).
    PlaceObject3 {
        /// First flags byte (same bit layout as [`Tag::PlaceObject2`]'s flags).
        flags1: u8,
        /// Second flags byte (`HasImage`/`HasClassName`/`HasCacheAsBitmap`/
        /// `HasBlendMode`/`HasFilterList`/`HasVisible`, plus reserved).
        flags2: u8,
        depth: u16,
        /// Class name (flag `flags2` `HasClassName` `0x08`); NUL implicit.
        class_name: Option<String>,
        /// `characterId` (flag `flags1` `HasCharacter` `0x02`).
        character_id: Option<u16>,
        /// Placement `MATRIX` (flag `flags1` `HasMatrix` `0x04`).
        matrix: Option<Matrix>,
        /// `CXFORMWITHALPHA` (flag `flags1` `HasColorTransform` `0x08`).
        color_transform: Option<CxformWithAlpha>,
        /// Morph `ratio` (flag `flags1` `HasRatio` `0x10`).
        ratio: Option<u16>,
        /// Instance `name` (flag `flags1` `HasName` `0x20`); NUL implicit.
        name: Option<String>,
        /// `clipDepth` (flag `flags1` `HasClipDepth` `0x40`).
        clip_depth: Option<u16>,
        /// SURFACEFILTERLIST (flag `flags2` `HasFilterList` `0x01`). The `u8`
        /// count is derived from the vector length on write.
        filters: Option<Vec<Filter>>,
        /// Blend mode (flag `flags2` `HasBlendMode` `0x02`).
        blend_mode: Option<u8>,
        /// Bitmap-cache flag (flag `flags2` `HasCacheAsBitmap` `0x04`).
        bitmap_cache: Option<u8>,
        /// `(visible, [r, g, b, a] background)` (flag `flags2` `HasVisible`
        /// `0x20`).
        visible: Option<(u8, [u8; 4])>,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `DefineScalingGrid` (code 78): a `characterId` and a nine-slice `RECT`.
    DefineScalingGrid {
        character_id: u16,
        grid: Rect,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },

    /// Any tag not otherwise modelled; body bytes re-emitted verbatim.
    Unknown {
        code: u16,
        raw: Vec<u8>,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
}

/// A parsed GFX movie: header plus its top-level tag stream (the top-level
/// stream includes its terminating [`Tag::End`] as its last element).
///
/// `Eq` is not derived because [`Tag`] holds `f32` fields; [`PartialEq`] is.
#[derive(Clone, Debug, PartialEq)]
pub struct Movie {
    pub header: Header,
    pub tags: Vec<Tag>,
}

/// Errors produced while parsing a GFX movie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GfxError {
    /// Ran out of input while reading: needed `need` bytes at `pos`, had `have`.
    UnexpectedEof {
        pos: usize,
        need: usize,
        have: usize,
    },
    /// File did not start with the `b"GFX"` magic.
    BadMagic([u8; 3]),
    /// An `End` tag was encoded with a non-empty / long-form body. Tier-0 treats
    /// this as malformed rather than silently round-tripping it incorrectly.
    BadEndTag { force_long: bool, len: usize },
    /// A `DefineSprite` body was shorter than its mandatory 4-byte prelude.
    SpriteBodyTooShort { len: usize },
    /// A nested tag stream overran its declared body length.
    NestedOverrun { body_end: usize, pos: usize },
    /// Tag code does not fit in a `RecordHeader` (`> 0x3ff`); cannot serialize.
    CodeTooLarge(u16),
    /// A Tier-1 string field ran to the end of its tag body without a NUL.
    UnterminatedString { code: u16 },
    /// A Tier-1 string field was not valid UTF-8.
    InvalidUtf8 { code: u16 },
    /// A Tier-1 tag body had bytes left over after structured decode (the tag's
    /// real layout differs from what Tier-1 models); kept as a hard error so a
    /// silent byte-divergence is impossible.
    TrailingTagBytes { code: u16, remaining: usize },
    /// A fixed-width Tier-1 tag body was not its expected length.
    UnexpectedTagBodyLen {
        code: u16,
        expected: usize,
        got: usize,
    },
    /// A bit-packed primitive read past the end of its tag body. `context` names
    /// the primitive (`"MATRIX"`, `"CXFORM"`, `"RECT"`).
    BitstreamEof { context: &'static str },
    /// A bit-packed primitive's byte-alignment padding bits were non-zero. The
    /// whole corpus pads with zero bits; a non-zero pad would make a stored-field
    /// re-encode silently diverge, so we reject it loudly instead. `context`
    /// names the primitive.
    NonZeroBitPadding { context: &'static str },
}

impl fmt::Display for GfxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GfxError::UnexpectedEof { pos, need, have } => write!(
                f,
                "unexpected EOF: needed {need} byte(s) at offset {pos}, only {have} available"
            ),
            GfxError::BadMagic(m) => {
                write!(f, "bad magic: expected GFX, got {m:02x?}")
            }
            GfxError::BadEndTag { force_long, len } => {
                write!(f, "malformed End tag (force_long={force_long}, len={len})")
            }
            GfxError::SpriteBodyTooShort { len } => {
                write!(f, "DefineSprite body too short: {len} byte(s)")
            }
            GfxError::NestedOverrun { body_end, pos } => {
                write!(
                    f,
                    "nested tag stream overran body: pos {pos} > end {body_end}"
                )
            }
            GfxError::CodeTooLarge(c) => write!(f, "tag code {c} exceeds RecordHeader max 0x3ff"),
            GfxError::UnterminatedString { code } => {
                write!(f, "unterminated string in tag code {code}")
            }
            GfxError::InvalidUtf8 { code } => {
                write!(f, "invalid UTF-8 string in tag code {code}")
            }
            GfxError::TrailingTagBytes { code, remaining } => write!(
                f,
                "tag code {code} had {remaining} unconsumed body byte(s) after typed decode"
            ),
            GfxError::UnexpectedTagBodyLen {
                code,
                expected,
                got,
            } => write!(
                f,
                "tag code {code} body length {got} != expected {expected}"
            ),
            GfxError::BitstreamEof { context } => {
                write!(f, "bitstream EOF while reading {context}")
            }
            GfxError::NonZeroBitPadding { context } => {
                write!(f, "non-zero byte-alignment padding in {context}")
            }
        }
    }
}

impl std::error::Error for GfxError {}

/// Minimal forward cursor over the input bytes with bounds-checked reads.
struct GfxReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> GfxReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        GfxReader { data, pos: 0 }
    }

    fn need(&self, n: usize) -> Result<(), GfxError> {
        if self.pos + n > self.data.len() {
            Err(GfxError::UnexpectedEof {
                pos: self.pos,
                need: n,
                have: self.data.len().saturating_sub(self.pos),
            })
        } else {
            Ok(())
        }
    }

    fn read_u8(&mut self) -> Result<u8, GfxError> {
        self.need(1)?;
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    fn read_u16(&mut self) -> Result<u16, GfxError> {
        self.need(2)?;
        let v = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    fn read_u32(&mut self) -> Result<u32, GfxError> {
        self.need(4)?;
        let v = u32::from_le_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(v)
    }

    fn read_bytes(&mut self, n: usize) -> Result<Vec<u8>, GfxError> {
        self.need(n)?;
        let s = self.data[self.pos..self.pos + n].to_vec();
        self.pos += n;
        Ok(s)
    }

    /// Read a NUL-terminated string, consuming the terminator. The returned
    /// `String` excludes the NUL (re-added by the writer). Errors loudly if the
    /// data ends before a NUL or the bytes are not valid UTF-8 -- `code` names
    /// the owning tag for diagnostics.
    fn read_cstring(&mut self, code: u16) -> Result<String, GfxError> {
        let start = self.pos;
        loop {
            if self.pos >= self.data.len() {
                return Err(GfxError::UnterminatedString { code });
            }
            let byte = self.data[self.pos];
            self.pos += 1;
            if byte == 0 {
                let bytes = self.data[start..self.pos - 1].to_vec();
                return String::from_utf8(bytes).map_err(|_| GfxError::InvalidUtf8 { code });
            }
        }
    }

    /// Read the bit-packed movie `RECT` as raw bytes.
    ///
    /// Layout: top 5 bits of the first byte are `Nbits`; the field then holds
    /// `4 * Nbits` more bits (signed xmin/xmax/ymin/ymax) for `5 + 4*Nbits`
    /// total bits, byte-aligned. We only compute the byte length and slice the
    /// bytes verbatim (no bit decode), preserving them exactly.
    fn read_rect_raw(&mut self) -> Result<Vec<u8>, GfxError> {
        self.need(1)?;
        let nbits = (self.data[self.pos] >> 3) as usize;
        let total_bits = 5 + 4 * nbits;
        let byte_len = total_bits.div_ceil(8);
        self.read_bytes(byte_len)
    }
}

/// Append-only byte sink with the small write helpers the writer needs.
struct GfxWriter {
    buf: Vec<u8>,
}

impl GfxWriter {
    fn new() -> Self {
        GfxWriter { buf: Vec::new() }
    }

    fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn write_u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    /// Write a string followed by its NUL terminator.
    fn write_cstring(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }

    /// Emit a `RecordHeader` for `code`/`body_len`, honoring `force_long`.
    fn write_record_header(
        &mut self,
        code: u16,
        body_len: usize,
        force_long: bool,
    ) -> Result<(), GfxError> {
        if code > MAX_TAG_CODE {
            return Err(GfxError::CodeTooLarge(code));
        }
        if force_long || body_len >= LONG_LEN_SENTINEL as usize {
            let word = (code << 6) | LONG_LEN_SENTINEL;
            self.write_u16(word);
            self.write_u32(body_len as u32);
        } else {
            let word = (code << 6) | (body_len as u16);
            self.write_u16(word);
        }
        Ok(())
    }
}

// ===========================================================================
// Tier-2 bit-packed primitive layer (MSB-first bit order -- the SWF convention)
// ===========================================================================
//
// SWF/GFX bit-packed structures (RECT, MATRIX, CXFORM[WITHALPHA]) read bits
// most-significant-first and byte-align at the end of each structure. The fields
// `Nbits` (RECT), `NScaleBits`/`NRotateBits`/`NTranslateBits` (MATRIX), and
// `Nbits` (CXFORM) are NOT guaranteed minimal: the Scaleform exporter is
// confirmed non-minimal (2,413 MATRIX instances in the 114-file corpus use more
// translate bits than the minimal width; 21 scale, 14 rotate). Byte-identity
// therefore REQUIRES storing each source nbits verbatim and re-encoding with it,
// never recomputing a minimal width. Byte-alignment padding is always zero
// across the corpus (0 non-zero pads over 171,728 MATRIX, 20,157 CXFORM, 124
// RECT); the reader rejects a non-zero pad ([`GfxError::NonZeroBitPadding`])
// rather than silently dropping bits, and the writer zero-fills.

/// MSB-first bit cursor over a byte slice (typically a single tag body).
struct BitReader<'a> {
    data: &'a [u8],
    /// Absolute bit position from the start of `data`.
    bitpos: usize,
}

impl<'a> BitReader<'a> {
    /// Construct a reader positioned at byte `byte_off` (bit-aligned). Bit-packed
    /// SWF/GFX structures always begin on a byte boundary.
    fn new_at_byte(data: &'a [u8], byte_off: usize) -> Self {
        BitReader {
            data,
            bitpos: byte_off * 8,
        }
    }

    /// Read `n` (<= 32) unsigned bits, MSB-first.
    fn read_ubits(&mut self, n: u32, context: &'static str) -> Result<u32, GfxError> {
        let mut acc: u64 = 0;
        for _ in 0..n {
            let byte_idx = self.bitpos >> 3;
            if byte_idx >= self.data.len() {
                return Err(GfxError::BitstreamEof { context });
            }
            let bit = (self.data[byte_idx] >> (7 - (self.bitpos & 7))) & 1;
            acc = (acc << 1) | bit as u64;
            self.bitpos += 1;
        }
        Ok(acc as u32)
    }

    /// Read `n` (<= 32) bits and sign-extend (two's complement).
    fn read_sbits(&mut self, n: u32, context: &'static str) -> Result<i32, GfxError> {
        if n == 0 {
            return Ok(0);
        }
        let u = self.read_ubits(n, context)?;
        let v = if n < 32 && (u & (1u32 << (n - 1))) != 0 {
            // Sign bit set: extend the high bits.
            (u | !((1u32 << n) - 1)) as i32
        } else {
            u as i32
        };
        Ok(v)
    }

    /// Read an `FB` fixed-point value as its raw signed integer (16.16 fixed is a
    /// signed integer at the bit level; callers interpret the scaling). Identical
    /// bit handling to [`read_sbits`](Self::read_sbits).
    fn read_fbits(&mut self, n: u32, context: &'static str) -> Result<i32, GfxError> {
        self.read_sbits(n, context)
    }

    /// Consume padding bits up to the next byte boundary. The padding must be
    /// zero (corpus-proven) or this errors loudly.
    fn byte_align(&mut self, context: &'static str) -> Result<(), GfxError> {
        let rem = (8 - (self.bitpos & 7)) & 7;
        let pad = self.read_ubits(rem as u32, context)?;
        if pad != 0 {
            return Err(GfxError::NonZeroBitPadding { context });
        }
        Ok(())
    }

    /// Current byte offset (must be byte-aligned, e.g. just after `byte_align`).
    fn byte_pos(&self) -> usize {
        debug_assert_eq!(self.bitpos & 7, 0, "BitReader not byte aligned");
        self.bitpos >> 3
    }
}

/// MSB-first bit sink. Bits accumulate into a current byte (MSB-first) and flush
/// on each completed byte; [`byte_align`](Self::byte_align) zero-fills.
struct BitWriter {
    buf: Vec<u8>,
    cur: u8,
    nbits: u8,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter {
            buf: Vec::new(),
            cur: 0,
            nbits: 0,
        }
    }

    fn write_bit(&mut self, bit: u32) {
        self.cur = (self.cur << 1) | (bit as u8 & 1);
        self.nbits += 1;
        if self.nbits == 8 {
            self.buf.push(self.cur);
            self.cur = 0;
            self.nbits = 0;
        }
    }

    fn write_ubits(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            self.write_bit((value >> i) & 1);
        }
    }

    fn write_sbits(&mut self, value: i32, n: u32) {
        if n == 0 {
            return;
        }
        let mask = if n >= 32 { u32::MAX } else { (1u32 << n) - 1 };
        self.write_ubits((value as u32) & mask, n);
    }

    fn write_fbits(&mut self, value: i32, n: u32) {
        self.write_sbits(value, n);
    }

    /// Zero-fill to the next byte boundary.
    fn byte_align(&mut self) {
        while self.nbits != 0 {
            self.write_bit(0);
        }
    }

    /// Finish, asserting byte alignment, and return the bytes.
    fn into_bytes(self) -> Vec<u8> {
        debug_assert_eq!(self.nbits, 0, "BitWriter not byte aligned");
        self.buf
    }
}

/// A bit-packed `RECT` (Nbits-aware). Used by [`Tag::DefineScalingGrid`] and
/// reusable for shape bounds later. `nbits` is stored exactly (not recomputed)
/// so the source's bit width is reproduced even when non-minimal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Bit width of each of the four signed coordinates (the source's `Nbits`).
    pub nbits: u32,
    pub x_min: i32,
    pub x_max: i32,
    pub y_min: i32,
    pub y_max: i32,
}

impl Rect {
    const CTX: &'static str = "RECT";

    fn read(br: &mut BitReader) -> Result<Rect, GfxError> {
        let nbits = br.read_ubits(5, Self::CTX)?;
        let x_min = br.read_sbits(nbits, Self::CTX)?;
        let x_max = br.read_sbits(nbits, Self::CTX)?;
        let y_min = br.read_sbits(nbits, Self::CTX)?;
        let y_max = br.read_sbits(nbits, Self::CTX)?;
        br.byte_align(Self::CTX)?;
        Ok(Rect {
            nbits,
            x_min,
            x_max,
            y_min,
            y_max,
        })
    }

    fn write(&self, bw: &mut BitWriter) {
        bw.write_ubits(self.nbits, 5);
        bw.write_sbits(self.x_min, self.nbits);
        bw.write_sbits(self.x_max, self.nbits);
        bw.write_sbits(self.y_min, self.nbits);
        bw.write_sbits(self.y_max, self.nbits);
        bw.byte_align();
    }
}

/// A bit-packed `MATRIX` with each source bit width preserved exactly.
///
/// `scale_x/y` and `rotate_skew0/1` are 16.16 fixed-point; `translate_x/y` are
/// twips. All are stored as their raw signed integers. The `*_nbits` fields hold
/// the SOURCE's bit widths; they are reproduced verbatim because the exporter is
/// not minimal (see the primitive-layer module comment).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matrix {
    pub has_scale: bool,
    /// `NScaleBits` (only meaningful when `has_scale`).
    pub scale_nbits: u32,
    pub scale_x: i32,
    pub scale_y: i32,
    pub has_rotate: bool,
    /// `NRotateBits` (only meaningful when `has_rotate`).
    pub rotate_nbits: u32,
    pub rotate_skew0: i32,
    pub rotate_skew1: i32,
    /// `NTranslateBits`. Translate is always present.
    pub translate_nbits: u32,
    pub translate_x: i32,
    pub translate_y: i32,
}

impl Matrix {
    const CTX: &'static str = "MATRIX";

    fn read(br: &mut BitReader) -> Result<Matrix, GfxError> {
        let has_scale = br.read_ubits(1, Self::CTX)? != 0;
        let (scale_nbits, scale_x, scale_y) = if has_scale {
            let n = br.read_ubits(5, Self::CTX)?;
            (
                n,
                br.read_fbits(n, Self::CTX)?,
                br.read_fbits(n, Self::CTX)?,
            )
        } else {
            (0, 0, 0)
        };
        let has_rotate = br.read_ubits(1, Self::CTX)? != 0;
        let (rotate_nbits, rotate_skew0, rotate_skew1) = if has_rotate {
            let n = br.read_ubits(5, Self::CTX)?;
            (
                n,
                br.read_fbits(n, Self::CTX)?,
                br.read_fbits(n, Self::CTX)?,
            )
        } else {
            (0, 0, 0)
        };
        let translate_nbits = br.read_ubits(5, Self::CTX)?;
        let translate_x = br.read_sbits(translate_nbits, Self::CTX)?;
        let translate_y = br.read_sbits(translate_nbits, Self::CTX)?;
        br.byte_align(Self::CTX)?;
        Ok(Matrix {
            has_scale,
            scale_nbits,
            scale_x,
            scale_y,
            has_rotate,
            rotate_nbits,
            rotate_skew0,
            rotate_skew1,
            translate_nbits,
            translate_x,
            translate_y,
        })
    }

    fn write(&self, bw: &mut BitWriter) {
        bw.write_ubits(self.has_scale as u32, 1);
        if self.has_scale {
            bw.write_ubits(self.scale_nbits, 5);
            bw.write_fbits(self.scale_x, self.scale_nbits);
            bw.write_fbits(self.scale_y, self.scale_nbits);
        }
        bw.write_ubits(self.has_rotate as u32, 1);
        if self.has_rotate {
            bw.write_ubits(self.rotate_nbits, 5);
            bw.write_fbits(self.rotate_skew0, self.rotate_nbits);
            bw.write_fbits(self.rotate_skew1, self.rotate_nbits);
        }
        bw.write_ubits(self.translate_nbits, 5);
        bw.write_sbits(self.translate_x, self.translate_nbits);
        bw.write_sbits(self.translate_y, self.translate_nbits);
        bw.byte_align();
    }
}

/// A bit-packed `CXFORM` (no alpha): RGB multiply/add terms. `nbits` preserved
/// exactly. Not used by a typed tag yet (it is the `PlaceObject`/`DefineButton`
/// color transform) but provided as a reusable primitive with its own tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cxform {
    pub has_add: bool,
    pub has_mult: bool,
    pub nbits: u32,
    /// `[red, green, blue]` multiply terms, present iff `has_mult`.
    pub mult: Option<[i32; 3]>,
    /// `[red, green, blue]` add terms, present iff `has_add`.
    pub add: Option<[i32; 3]>,
}

impl Cxform {
    const CTX: &'static str = "CXFORM";

    fn read(br: &mut BitReader) -> Result<Cxform, GfxError> {
        let has_add = br.read_ubits(1, Self::CTX)? != 0;
        let has_mult = br.read_ubits(1, Self::CTX)? != 0;
        let nbits = br.read_ubits(4, Self::CTX)?;
        let mult = if has_mult {
            Some([
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
            ])
        } else {
            None
        };
        let add = if has_add {
            Some([
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
            ])
        } else {
            None
        };
        br.byte_align(Self::CTX)?;
        Ok(Cxform {
            has_add,
            has_mult,
            nbits,
            mult,
            add,
        })
    }

    fn write(&self, bw: &mut BitWriter) {
        bw.write_ubits(self.has_add as u32, 1);
        bw.write_ubits(self.has_mult as u32, 1);
        bw.write_ubits(self.nbits, 4);
        if let Some(m) = self.mult {
            for v in m {
                bw.write_sbits(v, self.nbits);
            }
        }
        if let Some(a) = self.add {
            for v in a {
                bw.write_sbits(v, self.nbits);
            }
        }
        bw.byte_align();
    }
}

/// A bit-packed `CXFORMWITHALPHA`: RGBA multiply/add terms. `nbits` preserved
/// exactly. This is the color transform carried by [`Tag::PlaceObject2`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CxformWithAlpha {
    pub has_add: bool,
    pub has_mult: bool,
    pub nbits: u32,
    /// `[red, green, blue, alpha]` multiply terms, present iff `has_mult`.
    pub mult: Option<[i32; 4]>,
    /// `[red, green, blue, alpha]` add terms, present iff `has_add`.
    pub add: Option<[i32; 4]>,
}

impl CxformWithAlpha {
    const CTX: &'static str = "CXFORM";

    fn read(br: &mut BitReader) -> Result<CxformWithAlpha, GfxError> {
        let has_add = br.read_ubits(1, Self::CTX)? != 0;
        let has_mult = br.read_ubits(1, Self::CTX)? != 0;
        let nbits = br.read_ubits(4, Self::CTX)?;
        let mult = if has_mult {
            Some([
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
            ])
        } else {
            None
        };
        let add = if has_add {
            Some([
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
            ])
        } else {
            None
        };
        br.byte_align(Self::CTX)?;
        Ok(CxformWithAlpha {
            has_add,
            has_mult,
            nbits,
            mult,
            add,
        })
    }

    fn write(&self, bw: &mut BitWriter) {
        bw.write_ubits(self.has_add as u32, 1);
        bw.write_ubits(self.has_mult as u32, 1);
        bw.write_ubits(self.nbits, 4);
        if let Some(m) = self.mult {
            for v in m {
                bw.write_sbits(v, self.nbits);
            }
        }
        if let Some(a) = self.add {
            for v in a {
                bw.write_sbits(v, self.nbits);
            }
        }
        bw.byte_align();
    }
}

/// One entry of a `PlaceObject3` SURFACEFILTERLIST.
///
/// Only the filter ids that actually occur in the Elden Ring menu corpus are
/// typed: [`Filter::DropShadow`] (id 0, 2,849 instances) and [`Filter::Glow`]
/// (id 2, 31 instances). The fixed-point fields (`FIXED` is 16.16, `FIXED8` is
/// 8.8) are stored as their raw little-endian signed integers so byte-identity
/// is exact without committing to a float representation; `flags` holds the
/// filter's trailing `InnerShadow`/`Knockout`/`CompositeSource`/`Passes`
/// sub-byte verbatim. Any other filter id forces the whole `PlaceObject3` back
/// to [`Tag::Unknown`] (none occur -- 0 of 2,880 filters).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filter {
    /// `DropShadowFilter` (id 0): `RGBA` color, BlurX/BlurY/Angle/Distance
    /// (`FIXED` 16.16), Strength (`FIXED8` 8.8), and a trailing flags byte.
    DropShadow {
        /// `[red, green, blue, alpha]` shadow color.
        color: [u8; 4],
        /// BlurX, raw 16.16 fixed (`FIXED`).
        blur_x: i32,
        /// BlurY, raw 16.16 fixed.
        blur_y: i32,
        /// Angle, raw 16.16 fixed (radians).
        angle: i32,
        /// Distance, raw 16.16 fixed.
        distance: i32,
        /// Strength, raw 8.8 fixed (`FIXED8`).
        strength: i16,
        /// `InnerShadow`(0x80) / `Knockout`(0x40) / `CompositeSource`(0x20) /
        /// `Passes`(low 5 bits), stored verbatim.
        flags: u8,
    },
    /// `GlowFilter` (id 2): `RGBA` color, BlurX/BlurY (`FIXED`), Strength
    /// (`FIXED8`), and a trailing flags byte.
    Glow {
        /// `[red, green, blue, alpha]` glow color.
        color: [u8; 4],
        /// BlurX, raw 16.16 fixed.
        blur_x: i32,
        /// BlurY, raw 16.16 fixed.
        blur_y: i32,
        /// Strength, raw 8.8 fixed.
        strength: i16,
        /// `InnerGlow`(0x80) / `Knockout`(0x40) / `CompositeSource`(0x20) /
        /// `Passes`(low 5 bits), stored verbatim.
        flags: u8,
    },
}

impl Filter {
    /// Read one filter: a `u8` id followed by its body. Returns `Ok(None)` for an
    /// unmodelled filter id so the caller can fall the whole `PlaceObject3` back
    /// to [`Tag::Unknown`] (the id byte is NOT consumed in that case, but the
    /// caller discards `r` anyway and re-emits the raw body).
    fn read(r: &mut GfxReader) -> Result<Option<Filter>, GfxError> {
        let id = r.read_u8()?;
        match id {
            FILTER_DROP_SHADOW => {
                let color = [r.read_u8()?, r.read_u8()?, r.read_u8()?, r.read_u8()?];
                let blur_x = r.read_u32()? as i32;
                let blur_y = r.read_u32()? as i32;
                let angle = r.read_u32()? as i32;
                let distance = r.read_u32()? as i32;
                let strength = r.read_u16()? as i16;
                let flags = r.read_u8()?;
                Ok(Some(Filter::DropShadow {
                    color,
                    blur_x,
                    blur_y,
                    angle,
                    distance,
                    strength,
                    flags,
                }))
            }
            FILTER_GLOW => {
                let color = [r.read_u8()?, r.read_u8()?, r.read_u8()?, r.read_u8()?];
                let blur_x = r.read_u32()? as i32;
                let blur_y = r.read_u32()? as i32;
                let strength = r.read_u16()? as i16;
                let flags = r.read_u8()?;
                Ok(Some(Filter::Glow {
                    color,
                    blur_x,
                    blur_y,
                    strength,
                    flags,
                }))
            }
            _ => Ok(None),
        }
    }

    fn write(&self, w: &mut GfxWriter) {
        match self {
            Filter::DropShadow {
                color,
                blur_x,
                blur_y,
                angle,
                distance,
                strength,
                flags,
            } => {
                w.write_u8(FILTER_DROP_SHADOW);
                w.write_bytes(color);
                w.write_u32(*blur_x as u32);
                w.write_u32(*blur_y as u32);
                w.write_u32(*angle as u32);
                w.write_u32(*distance as u32);
                w.write_u16(*strength as u16);
                w.write_u8(*flags);
            }
            Filter::Glow {
                color,
                blur_x,
                blur_y,
                strength,
                flags,
            } => {
                w.write_u8(FILTER_GLOW);
                w.write_bytes(color);
                w.write_u32(*blur_x as u32);
                w.write_u32(*blur_y as u32);
                w.write_u16(*strength as u16);
                w.write_u8(*flags);
            }
        }
    }
}

impl Movie {
    /// Parse a complete uncompressed GFX movie from `data`.
    pub fn parse(data: &[u8]) -> Result<Movie, GfxError> {
        let mut r = GfxReader::new(data);

        let magic = [r.read_u8()?, r.read_u8()?, r.read_u8()?];
        if magic != MAGIC {
            return Err(GfxError::BadMagic(magic));
        }
        let version = r.read_u8()?;
        // FileLength is structurally derived; read past it (it is recomputed on
        // write, never echoed).
        let _file_length = r.read_u32()?;
        let movie_rect_raw = r.read_rect_raw()?;
        let frame_rate = r.read_u16()?;
        let frame_count = r.read_u16()?;

        let header = Header {
            version,
            movie_rect_raw,
            frame_rate,
            frame_count,
        };

        // Top-level stream runs until its End tag (no enclosing length bound).
        let tags = parse_tag_stream(&mut r, None)?;

        Ok(Movie { header, tags })
    }

    /// Serialize this movie back to bytes. Byte-identical to the source for any
    /// movie produced by [`Movie::parse`] over the Tier-0 corpus.
    pub fn write(&self) -> Result<Vec<u8>, GfxError> {
        let mut w = GfxWriter::new();
        w.write_bytes(&MAGIC);
        w.buf.push(self.header.version);
        // FileLength placeholder; back-patched to the final total length.
        let file_length_offset = w.buf.len();
        w.write_u32(0);
        w.write_bytes(&self.header.movie_rect_raw);
        w.write_u16(self.header.frame_rate);
        w.write_u16(self.header.frame_count);

        for tag in &self.tags {
            write_tag(&mut w, tag)?;
        }

        let total = w.buf.len() as u32;
        w.buf[file_length_offset..file_length_offset + 4].copy_from_slice(&total.to_le_bytes());
        Ok(w.buf)
    }
}

/// Parse a tag stream. If `body_end` is `Some`, the stream is bounded (a nested
/// `DefineSprite` body); otherwise it runs to the top-level `End`.
///
/// The returned vector includes the terminating [`Tag::End`] as its last element
/// when one is present, so re-serialization reproduces it.
fn parse_tag_stream(r: &mut GfxReader, body_end: Option<usize>) -> Result<Vec<Tag>, GfxError> {
    let mut tags = Vec::new();
    loop {
        if let Some(end) = body_end {
            if r.pos >= end {
                // Bounded stream filled without an explicit End. Unusual, but
                // re-serialization stays byte-identical (length recomputed).
                if r.pos > end {
                    return Err(GfxError::NestedOverrun {
                        body_end: end,
                        pos: r.pos,
                    });
                }
                break;
            }
        }

        let word = r.read_u16()?;
        let code = word >> 6;
        let short_len = word & 0x3f;
        let (len, force_long) = if short_len == LONG_LEN_SENTINEL {
            (r.read_u32()? as usize, true)
        } else {
            (short_len as usize, false)
        };

        if code == TAG_END {
            if force_long || len != 0 {
                return Err(GfxError::BadEndTag { force_long, len });
            }
            tags.push(Tag::End);
            break;
        }

        if code == TAG_DEFINE_SPRITE {
            if len < 4 {
                return Err(GfxError::SpriteBodyTooShort { len });
            }
            let id = r.read_u16()?;
            let frame_count = r.read_u16()?;
            let inner_end = r.pos + (len - 4);
            let inner = parse_tag_stream(r, Some(inner_end))?;
            if r.pos != inner_end {
                return Err(GfxError::NestedOverrun {
                    body_end: inner_end,
                    pos: r.pos,
                });
            }
            tags.push(Tag::DefineSprite {
                id,
                frame_count,
                tags: inner,
                force_long,
            });
        } else {
            let body = r.read_bytes(len)?;
            tags.push(decode_tag_body(code, body, force_long)?);
        }
    }
    Ok(tags)
}

/// Decode a (already-sliced) tag `body` into a typed [`Tag`], falling back to
/// [`Tag::Unknown`] for codes Tier-1 does not model. Each typed branch is
/// proven byte-identical over the corpus; structural surprises (wrong length,
/// leftover bytes, missing NUL, non-UTF-8) are hard errors so a divergence can
/// never be silently re-emitted.
fn decode_tag_body(code: u16, body: Vec<u8>, force_long: bool) -> Result<Tag, GfxError> {
    match code {
        TAG_SHOW_FRAME => {
            ensure_body_len(code, &body, 0)?;
            Ok(Tag::ShowFrame { force_long })
        }
        TAG_SET_BACKGROUND_COLOR => {
            ensure_body_len(code, &body, 3)?;
            Ok(Tag::SetBackgroundColor {
                r: body[0],
                g: body[1],
                b: body[2],
                force_long,
            })
        }
        TAG_REMOVE_OBJECT2 => {
            ensure_body_len(code, &body, 2)?;
            Ok(Tag::RemoveObject2 {
                depth: u16::from_le_bytes([body[0], body[1]]),
                force_long,
            })
        }
        TAG_FILE_ATTRIBUTES => {
            ensure_body_len(code, &body, 4)?;
            Ok(Tag::FileAttributes {
                flags: u32::from_le_bytes([body[0], body[1], body[2], body[3]]),
                force_long,
            })
        }
        TAG_METADATA => {
            let mut br = GfxReader::new(&body);
            let xml = br.read_cstring(code)?;
            ensure_consumed(code, br.pos, body.len())?;
            Ok(Tag::Metadata { xml, force_long })
        }
        TAG_FRAME_LABEL => {
            let mut br = GfxReader::new(&body);
            let label = br.read_cstring(code)?;
            let remaining = body.len() - br.pos;
            let named_anchor = match remaining {
                0 => None,
                1 => Some(body[br.pos]),
                n => return Err(GfxError::TrailingTagBytes { code, remaining: n }),
            };
            Ok(Tag::FrameLabel {
                label,
                named_anchor,
                force_long,
            })
        }
        TAG_SYMBOL_CLASS => {
            let mut br = GfxReader::new(&body);
            let count = br.read_u16()?;
            let mut symbols = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let tag = br.read_u16()?;
                let name = br.read_cstring(code)?;
                symbols.push((tag, name));
            }
            ensure_consumed(code, br.pos, body.len())?;
            Ok(Tag::SymbolClass {
                symbols,
                force_long,
            })
        }
        TAG_IMPORT_ASSETS2 => {
            let mut br = GfxReader::new(&body);
            let url = br.read_cstring(code)?;
            let reserved = [br.read_u8()?, br.read_u8()?];
            let count = br.read_u16()?;
            let mut symbols = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let tag = br.read_u16()?;
                let name = br.read_cstring(code)?;
                symbols.push((tag, name));
            }
            ensure_consumed(code, br.pos, body.len())?;
            Ok(Tag::ImportAssets2 {
                url,
                reserved,
                symbols,
                force_long,
            })
        }
        TAG_CSM_TEXT_SETTINGS => {
            ensure_body_len(code, &body, 12)?;
            let mut br = GfxReader::new(&body);
            let character_id = br.read_u16()?;
            let flags = br.read_u8()?;
            let thickness = f32::from_bits(br.read_u32()?);
            let sharpness = f32::from_bits(br.read_u32()?);
            let reserved = br.read_u8()?;
            Ok(Tag::CsmTextSettings {
                character_id,
                flags,
                thickness,
                sharpness,
                reserved,
                force_long,
            })
        }
        TAG_PLACE_OBJECT2 => decode_place_object2(body, force_long),
        TAG_PLACE_OBJECT3 => decode_place_object3(body, force_long),
        TAG_DEFINE_SCALING_GRID => {
            let mut br = GfxReader::new(&body);
            let character_id = br.read_u16()?;
            let mut bits = BitReader::new_at_byte(&body, br.pos);
            let grid = Rect::read(&mut bits)?;
            ensure_consumed(code, bits.byte_pos(), body.len())?;
            Ok(Tag::DefineScalingGrid {
                character_id,
                grid,
                force_long,
            })
        }
        _ => Ok(Tag::Unknown {
            code,
            raw: body,
            force_long,
        }),
    }
}

/// Decode a `PlaceObject2` (code 26) body. The `flags` byte governs which
/// optional fields follow (MATRIX/CXFORMWITHALPHA are bit-packed and byte-align
/// at their end). A PlaceObject2 carrying clipActions (flag `0x80`) is kept as
/// [`Tag::Unknown`] -- that field is unmodelled and never occurs in the corpus;
/// keeping it opaque preserves byte-identity without guessing its structure.
fn decode_place_object2(body: Vec<u8>, force_long: bool) -> Result<Tag, GfxError> {
    let code = TAG_PLACE_OBJECT2;
    if body.len() < 3 {
        return Err(GfxError::UnexpectedTagBodyLen {
            code,
            expected: 3,
            got: body.len(),
        });
    }
    let flags = body[0];
    // clipActions is unmodelled: fall back to opaque Unknown (see doc comment).
    if flags & PO2_HAS_CLIPACTIONS != 0 {
        return Ok(Tag::Unknown {
            code,
            raw: body,
            force_long,
        });
    }
    let depth = u16::from_le_bytes([body[1], body[2]]);
    let mut pos = 3usize;

    let mut read_u16_at = |pos: &mut usize| -> Result<u16, GfxError> {
        if *pos + 2 > body.len() {
            return Err(GfxError::UnexpectedEof {
                pos: *pos,
                need: 2,
                have: body.len().saturating_sub(*pos),
            });
        }
        let v = u16::from_le_bytes([body[*pos], body[*pos + 1]]);
        *pos += 2;
        Ok(v)
    };

    let character_id = if flags & PO2_HAS_CHARACTER != 0 {
        Some(read_u16_at(&mut pos)?)
    } else {
        None
    };
    let matrix = if flags & PO2_HAS_MATRIX != 0 {
        let mut bits = BitReader::new_at_byte(&body, pos);
        let m = Matrix::read(&mut bits)?;
        pos = bits.byte_pos();
        Some(m)
    } else {
        None
    };
    let color_transform = if flags & PO2_HAS_CXFORM != 0 {
        let mut bits = BitReader::new_at_byte(&body, pos);
        let c = CxformWithAlpha::read(&mut bits)?;
        pos = bits.byte_pos();
        Some(c)
    } else {
        None
    };
    let ratio = if flags & PO2_HAS_RATIO != 0 {
        Some(read_u16_at(&mut pos)?)
    } else {
        None
    };
    let name = if flags & PO2_HAS_NAME != 0 {
        let mut nr = GfxReader::new(&body);
        nr.pos = pos;
        let s = nr.read_cstring(code)?;
        pos = nr.pos;
        Some(s)
    } else {
        None
    };
    let clip_depth = if flags & PO2_HAS_CLIPDEPTH != 0 {
        Some(read_u16_at(&mut pos)?)
    } else {
        None
    };

    ensure_consumed(code, pos, body.len())?;
    Ok(Tag::PlaceObject2 {
        flags,
        depth,
        character_id,
        matrix,
        color_transform,
        ratio,
        name,
        clip_depth,
        force_long,
    })
}

/// Decode a `PlaceObject3` (code 70) body. Two flags bytes govern the optional
/// fields (the first reuses the `PlaceObject2` bit layout; the second adds the
/// PO3 extras). MATRIX/CXFORMWITHALPHA are bit-packed and byte-align at their
/// end. A PlaceObject3 that sets `clipActions`, a reserved `flags2` bit, or
/// carries an unmodelled filter id is kept as [`Tag::Unknown`] -- those paths
/// never occur in the corpus, so keeping them opaque preserves byte-identity
/// without guessing their structure.
fn decode_place_object3(body: Vec<u8>, force_long: bool) -> Result<Tag, GfxError> {
    let code = TAG_PLACE_OBJECT3;
    // Need at least flags1 + flags2 + depth.
    if body.len() < 4 {
        return Ok(Tag::Unknown {
            code,
            raw: body,
            force_long,
        });
    }
    let flags1 = body[0];
    let flags2 = body[1];
    // Unmodelled (never-occurring) layouts -> opaque Unknown.
    if flags1 & PO2_HAS_CLIPACTIONS != 0 || flags2 & PO3_RESERVED_MASK != 0 {
        return Ok(Tag::Unknown {
            code,
            raw: body,
            force_long,
        });
    }

    let mut r = GfxReader::new(&body);
    r.pos = 2;
    let depth = r.read_u16()?;

    let class_name = if flags2 & PO3_HAS_CLASSNAME != 0 {
        Some(r.read_cstring(code)?)
    } else {
        None
    };
    let character_id = if flags1 & PO2_HAS_CHARACTER != 0 {
        Some(r.read_u16()?)
    } else {
        None
    };
    let matrix = if flags1 & PO2_HAS_MATRIX != 0 {
        let mut bits = BitReader::new_at_byte(&body, r.pos);
        let m = Matrix::read(&mut bits)?;
        r.pos = bits.byte_pos();
        Some(m)
    } else {
        None
    };
    let color_transform = if flags1 & PO2_HAS_CXFORM != 0 {
        let mut bits = BitReader::new_at_byte(&body, r.pos);
        let c = CxformWithAlpha::read(&mut bits)?;
        r.pos = bits.byte_pos();
        Some(c)
    } else {
        None
    };
    let ratio = if flags1 & PO2_HAS_RATIO != 0 {
        Some(r.read_u16()?)
    } else {
        None
    };
    let name = if flags1 & PO2_HAS_NAME != 0 {
        Some(r.read_cstring(code)?)
    } else {
        None
    };
    let clip_depth = if flags1 & PO2_HAS_CLIPDEPTH != 0 {
        Some(r.read_u16()?)
    } else {
        None
    };
    let filters = if flags2 & PO3_HAS_FILTERLIST != 0 {
        let count = r.read_u8()?;
        let mut v = Vec::with_capacity(count as usize);
        for _ in 0..count {
            match Filter::read(&mut r)? {
                Some(f) => v.push(f),
                // Unmodelled filter id: discard the partial decode and keep the
                // whole tag opaque (the raw body re-emits byte-identically).
                None => {
                    return Ok(Tag::Unknown {
                        code,
                        raw: body.clone(),
                        force_long,
                    });
                }
            }
        }
        Some(v)
    } else {
        None
    };
    let blend_mode = if flags2 & PO3_HAS_BLENDMODE != 0 {
        Some(r.read_u8()?)
    } else {
        None
    };
    let bitmap_cache = if flags2 & PO3_HAS_CACHE_AS_BITMAP != 0 {
        Some(r.read_u8()?)
    } else {
        None
    };
    let visible = if flags2 & PO3_HAS_VISIBLE != 0 {
        let v = r.read_u8()?;
        let bg = [r.read_u8()?, r.read_u8()?, r.read_u8()?, r.read_u8()?];
        Some((v, bg))
    } else {
        None
    };

    ensure_consumed(code, r.pos, body.len())?;
    Ok(Tag::PlaceObject3 {
        flags1,
        flags2,
        depth,
        class_name,
        character_id,
        matrix,
        color_transform,
        ratio,
        name,
        clip_depth,
        filters,
        blend_mode,
        bitmap_cache,
        visible,
        force_long,
    })
}

/// Assert a fixed-width Tier-1 tag body is exactly `expected` bytes.
fn ensure_body_len(code: u16, body: &[u8], expected: usize) -> Result<(), GfxError> {
    if body.len() != expected {
        Err(GfxError::UnexpectedTagBodyLen {
            code,
            expected,
            got: body.len(),
        })
    } else {
        Ok(())
    }
}

/// Assert a variable Tier-1 tag consumed its whole body (no leftover bytes).
fn ensure_consumed(code: u16, pos: usize, body_len: usize) -> Result<(), GfxError> {
    if pos != body_len {
        Err(GfxError::TrailingTagBytes {
            code,
            remaining: body_len - pos,
        })
    } else {
        Ok(())
    }
}

/// Serialize a single tag (recursing into `DefineSprite`). The body is built in
/// a scratch buffer first so the `RecordHeader` length is always derived.
fn write_tag(w: &mut GfxWriter, tag: &Tag) -> Result<(), GfxError> {
    match tag {
        Tag::End => {
            // Always short form: code 0, len 0 -> word 0x0000.
            w.write_record_header(TAG_END, 0, false)?;
        }
        Tag::Unknown {
            code,
            raw,
            force_long,
        } => {
            w.write_record_header(*code, raw.len(), *force_long)?;
            w.write_bytes(raw);
        }
        Tag::DefineSprite {
            id,
            frame_count,
            tags,
            force_long,
        } => {
            let mut body = GfxWriter::new();
            body.write_u16(*id);
            body.write_u16(*frame_count);
            for t in tags {
                write_tag(&mut body, t)?;
            }
            w.write_record_header(TAG_DEFINE_SPRITE, body.buf.len(), *force_long)?;
            w.write_bytes(&body.buf);
        }
        Tag::ShowFrame { force_long } => {
            // Empty body.
            w.write_record_header(TAG_SHOW_FRAME, 0, *force_long)?;
        }
        Tag::SetBackgroundColor {
            r,
            g,
            b,
            force_long,
        } => {
            let mut body = GfxWriter::new();
            body.write_u8(*r);
            body.write_u8(*g);
            body.write_u8(*b);
            w.write_record_header(TAG_SET_BACKGROUND_COLOR, body.buf.len(), *force_long)?;
            w.write_bytes(&body.buf);
        }
        Tag::RemoveObject2 { depth, force_long } => {
            let mut body = GfxWriter::new();
            body.write_u16(*depth);
            w.write_record_header(TAG_REMOVE_OBJECT2, body.buf.len(), *force_long)?;
            w.write_bytes(&body.buf);
        }
        Tag::FileAttributes { flags, force_long } => {
            let mut body = GfxWriter::new();
            body.write_u32(*flags);
            w.write_record_header(TAG_FILE_ATTRIBUTES, body.buf.len(), *force_long)?;
            w.write_bytes(&body.buf);
        }
        Tag::Metadata { xml, force_long } => {
            let mut body = GfxWriter::new();
            body.write_cstring(xml);
            w.write_record_header(TAG_METADATA, body.buf.len(), *force_long)?;
            w.write_bytes(&body.buf);
        }
        Tag::FrameLabel {
            label,
            named_anchor,
            force_long,
        } => {
            let mut body = GfxWriter::new();
            body.write_cstring(label);
            if let Some(anchor) = named_anchor {
                body.write_u8(*anchor);
            }
            w.write_record_header(TAG_FRAME_LABEL, body.buf.len(), *force_long)?;
            w.write_bytes(&body.buf);
        }
        Tag::SymbolClass {
            symbols,
            force_long,
        } => {
            let mut body = GfxWriter::new();
            body.write_u16(symbols.len() as u16);
            for (tag, name) in symbols {
                body.write_u16(*tag);
                body.write_cstring(name);
            }
            w.write_record_header(TAG_SYMBOL_CLASS, body.buf.len(), *force_long)?;
            w.write_bytes(&body.buf);
        }
        Tag::ImportAssets2 {
            url,
            reserved,
            symbols,
            force_long,
        } => {
            let mut body = GfxWriter::new();
            body.write_cstring(url);
            body.write_bytes(reserved);
            body.write_u16(symbols.len() as u16);
            for (tag, name) in symbols {
                body.write_u16(*tag);
                body.write_cstring(name);
            }
            w.write_record_header(TAG_IMPORT_ASSETS2, body.buf.len(), *force_long)?;
            w.write_bytes(&body.buf);
        }
        Tag::CsmTextSettings {
            character_id,
            flags,
            thickness,
            sharpness,
            reserved,
            force_long,
        } => {
            let mut body = GfxWriter::new();
            body.write_u16(*character_id);
            body.write_u8(*flags);
            body.write_u32(thickness.to_bits());
            body.write_u32(sharpness.to_bits());
            body.write_u8(*reserved);
            w.write_record_header(TAG_CSM_TEXT_SETTINGS, body.buf.len(), *force_long)?;
            w.write_bytes(&body.buf);
        }
        Tag::PlaceObject2 {
            flags,
            depth,
            character_id,
            matrix,
            color_transform,
            ratio,
            name,
            clip_depth,
            force_long,
        } => {
            let mut body = GfxWriter::new();
            // The flags byte is the source of truth; each optional field is
            // present iff its flag bit is set (guaranteed consistent by decode).
            body.write_u8(*flags);
            body.write_u16(*depth);
            if flags & PO2_HAS_CHARACTER != 0 {
                body.write_u16(character_id.expect("HasCharacter set without character_id"));
            }
            if flags & PO2_HAS_MATRIX != 0 {
                let mut bw = BitWriter::new();
                matrix
                    .as_ref()
                    .expect("HasMatrix set without matrix")
                    .write(&mut bw);
                body.write_bytes(&bw.into_bytes());
            }
            if flags & PO2_HAS_CXFORM != 0 {
                let mut bw = BitWriter::new();
                color_transform
                    .as_ref()
                    .expect("HasColorTransform set without color_transform")
                    .write(&mut bw);
                body.write_bytes(&bw.into_bytes());
            }
            if flags & PO2_HAS_RATIO != 0 {
                body.write_u16(ratio.expect("HasRatio set without ratio"));
            }
            if flags & PO2_HAS_NAME != 0 {
                body.write_cstring(name.as_deref().expect("HasName set without name"));
            }
            if flags & PO2_HAS_CLIPDEPTH != 0 {
                body.write_u16(clip_depth.expect("HasClipDepth set without clip_depth"));
            }
            // PO2_HAS_CLIPACTIONS never reaches here: such tags stay Tag::Unknown.
            w.write_record_header(TAG_PLACE_OBJECT2, body.buf.len(), *force_long)?;
            w.write_bytes(&body.buf);
        }
        Tag::PlaceObject3 {
            flags1,
            flags2,
            depth,
            class_name,
            character_id,
            matrix,
            color_transform,
            ratio,
            name,
            clip_depth,
            filters,
            blend_mode,
            bitmap_cache,
            visible,
            force_long,
        } => {
            let mut body = GfxWriter::new();
            // Both flags bytes are the source of truth; each optional field is
            // present iff its flag bit is set (guaranteed consistent by decode).
            body.write_u8(*flags1);
            body.write_u8(*flags2);
            body.write_u16(*depth);
            if flags2 & PO3_HAS_CLASSNAME != 0 {
                body.write_cstring(
                    class_name
                        .as_deref()
                        .expect("HasClassName without class_name"),
                );
            }
            if flags1 & PO2_HAS_CHARACTER != 0 {
                body.write_u16(character_id.expect("HasCharacter set without character_id"));
            }
            if flags1 & PO2_HAS_MATRIX != 0 {
                let mut bw = BitWriter::new();
                matrix
                    .as_ref()
                    .expect("HasMatrix set without matrix")
                    .write(&mut bw);
                body.write_bytes(&bw.into_bytes());
            }
            if flags1 & PO2_HAS_CXFORM != 0 {
                let mut bw = BitWriter::new();
                color_transform
                    .as_ref()
                    .expect("HasColorTransform set without color_transform")
                    .write(&mut bw);
                body.write_bytes(&bw.into_bytes());
            }
            if flags1 & PO2_HAS_RATIO != 0 {
                body.write_u16(ratio.expect("HasRatio set without ratio"));
            }
            if flags1 & PO2_HAS_NAME != 0 {
                body.write_cstring(name.as_deref().expect("HasName set without name"));
            }
            if flags1 & PO2_HAS_CLIPDEPTH != 0 {
                body.write_u16(clip_depth.expect("HasClipDepth set without clip_depth"));
            }
            if flags2 & PO3_HAS_FILTERLIST != 0 {
                let fs = filters.as_ref().expect("HasFilterList set without filters");
                body.write_u8(fs.len() as u8);
                for f in fs {
                    f.write(&mut body);
                }
            }
            if flags2 & PO3_HAS_BLENDMODE != 0 {
                body.write_u8(blend_mode.expect("HasBlendMode set without blend_mode"));
            }
            if flags2 & PO3_HAS_CACHE_AS_BITMAP != 0 {
                body.write_u8(bitmap_cache.expect("HasCacheAsBitmap set without bitmap_cache"));
            }
            if flags2 & PO3_HAS_VISIBLE != 0 {
                let (v, bg) = visible.as_ref().expect("HasVisible set without visible");
                body.write_u8(*v);
                body.write_bytes(bg);
            }
            // clipActions / reserved flags2 bits never reach here: such tags
            // stay Tag::Unknown.
            w.write_record_header(TAG_PLACE_OBJECT3, body.buf.len(), *force_long)?;
            w.write_bytes(&body.buf);
        }
        Tag::DefineScalingGrid {
            character_id,
            grid,
            force_long,
        } => {
            let mut body = GfxWriter::new();
            body.write_u16(*character_id);
            let mut bw = BitWriter::new();
            grid.write(&mut bw);
            body.write_bytes(&bw.into_bytes());
            w.write_record_header(TAG_DEFINE_SCALING_GRID, body.buf.len(), *force_long)?;
            w.write_bytes(&body.buf);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_minimal_synthetic_movie() {
        // magic + version + FileLength(placeholder) + rect(nbits=0 -> 5 bits ->1 byte)
        // + frameRate + frameCount + one Unknown tag + End.
        let mut d = Vec::new();
        d.extend_from_slice(b"GFX");
        d.push(0x0b);
        let len_off = d.len();
        d.extend_from_slice(&[0, 0, 0, 0]); // placeholder FileLength
        d.push(0x00); // RECT: nbits=0 -> 5 bits total -> 1 byte
        d.extend_from_slice(&30u16.to_le_bytes()); // frame_rate
        d.extend_from_slice(&1u16.to_le_bytes()); // frame_count
        // Unknown tag code 26, body 3 bytes, short form.
        let word: u16 = (26u16 << 6) | 3;
        d.extend_from_slice(&word.to_le_bytes());
        d.extend_from_slice(&[0xaa, 0xbb, 0xcc]);
        // End.
        d.extend_from_slice(&0u16.to_le_bytes());
        // patch FileLength
        let total = d.len() as u32;
        d[len_off..len_off + 4].copy_from_slice(&total.to_le_bytes());

        let m = Movie::parse(&d).expect("parse");
        let out = m.write().expect("write");
        assert_eq!(out, d);
    }

    #[test]
    fn preserves_overlong_record_header_form() {
        // Same as above but the Unknown tag uses the long form despite a small
        // body. Byte-identity requires the force_long bit to survive.
        let mut d = Vec::new();
        d.extend_from_slice(b"GFX");
        d.push(0x0b);
        let len_off = d.len();
        d.extend_from_slice(&[0, 0, 0, 0]);
        d.push(0x00);
        d.extend_from_slice(&30u16.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        // Unknown tag code 2 (DefineShape -- still unmodelled), long form, body
        // 2 bytes. (Code 70 is now the typed PlaceObject3, so this generic
        // "overlong header on an unknown code" test uses a code that stays
        // Tag::Unknown.)
        let word: u16 = (2u16 << 6) | 0x3f;
        d.extend_from_slice(&word.to_le_bytes());
        d.extend_from_slice(&2u32.to_le_bytes());
        d.extend_from_slice(&[0x11, 0x22]);
        d.extend_from_slice(&0u16.to_le_bytes()); // End
        let total = d.len() as u32;
        d[len_off..len_off + 4].copy_from_slice(&total.to_le_bytes());

        let m = Movie::parse(&d).expect("parse");
        match &m.tags[0] {
            Tag::Unknown { force_long, .. } => assert!(*force_long),
            other => panic!("expected Unknown, got {other:?}"),
        }
        assert_eq!(m.write().expect("write"), d);
    }

    // --- Tier-1 helpers ------------------------------------------------------

    /// Encode a single `RecordHeader` + body, honoring `force_long`.
    fn rec(code: u16, body: &[u8], force_long: bool) -> Vec<u8> {
        let mut v = Vec::new();
        if force_long || body.len() >= LONG_LEN_SENTINEL as usize {
            v.extend_from_slice(&((code << 6) | LONG_LEN_SENTINEL).to_le_bytes());
            v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        } else {
            v.extend_from_slice(&((code << 6) | body.len() as u16).to_le_bytes());
        }
        v.extend_from_slice(body);
        v
    }

    /// Wrap a raw tag-stream slice in a minimal valid movie (with a trailing
    /// `End`), patching the FileLength. The first parsed tag is the caller's.
    fn wrap(tag_stream: &[u8]) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"GFX");
        d.push(0x0b);
        let len_off = d.len();
        d.extend_from_slice(&[0, 0, 0, 0]);
        d.push(0x00); // RECT nbits=0 -> 1 byte
        d.extend_from_slice(&30u16.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(tag_stream);
        d.extend_from_slice(&0u16.to_le_bytes()); // End
        let total = d.len() as u32;
        d[len_off..len_off + 4].copy_from_slice(&total.to_le_bytes());
        d
    }

    /// Parse `wrap(stream)`, assert byte-identical round-trip, return first tag.
    fn parse_first(tag_stream: &[u8]) -> Tag {
        let d = wrap(tag_stream);
        let m = Movie::parse(&d).expect("parse");
        assert_eq!(
            m.write().expect("write"),
            d,
            "round-trip not byte-identical"
        );
        m.tags.into_iter().next().expect("at least one tag")
    }

    // --- Tier-1 field-level tests -------------------------------------------

    #[test]
    fn show_frame_empty_body() {
        match parse_first(&rec(TAG_SHOW_FRAME, &[], false)) {
            Tag::ShowFrame { force_long } => assert!(!force_long),
            other => panic!("expected ShowFrame, got {other:?}"),
        }
    }

    #[test]
    fn set_background_color_rgb_fields() {
        match parse_first(&rec(TAG_SET_BACKGROUND_COLOR, &[0x12, 0x34, 0x56], false)) {
            Tag::SetBackgroundColor { r, g, b, .. } => {
                assert_eq!((r, g, b), (0x12, 0x34, 0x56));
            }
            other => panic!("expected SetBackgroundColor, got {other:?}"),
        }
    }

    #[test]
    fn remove_object2_depth_field() {
        // bytes [0x02, 0x01] -> little-endian u16 0x0102.
        match parse_first(&rec(TAG_REMOVE_OBJECT2, &[0x02, 0x01], false)) {
            Tag::RemoveObject2 { depth, .. } => assert_eq!(depth, 0x0102),
            other => panic!("expected RemoveObject2, got {other:?}"),
        }
    }

    #[test]
    fn file_attributes_flags_stored_raw() {
        match parse_first(&rec(TAG_FILE_ATTRIBUTES, &0x18u32.to_le_bytes(), false)) {
            Tag::FileAttributes { flags, .. } => assert_eq!(flags, 0x18),
            other => panic!("expected FileAttributes, got {other:?}"),
        }
    }

    #[test]
    fn metadata_string_and_trailing_nul() {
        match parse_first(&rec(TAG_METADATA, b"<rdf/>\0", true)) {
            Tag::Metadata { xml, force_long } => {
                assert_eq!(xml, "<rdf/>");
                assert!(force_long);
            }
            other => panic!("expected Metadata, got {other:?}"),
        }
    }

    #[test]
    fn frame_label_without_anchor() {
        match parse_first(&rec(TAG_FRAME_LABEL, b"Loop\0", true)) {
            Tag::FrameLabel {
                label,
                named_anchor,
                ..
            } => {
                assert_eq!(label, "Loop");
                assert_eq!(named_anchor, None);
            }
            other => panic!("expected FrameLabel, got {other:?}"),
        }
    }

    #[test]
    fn frame_label_with_named_anchor() {
        // NUL-terminated label + a trailing anchor byte (not seen in the Tier-1
        // corpus, but the model must preserve it byte-identically).
        let mut body = b"Loop\0".to_vec();
        body.push(0x01);
        match parse_first(&rec(TAG_FRAME_LABEL, &body, true)) {
            Tag::FrameLabel {
                label,
                named_anchor,
                ..
            } => {
                assert_eq!(label, "Loop");
                assert_eq!(named_anchor, Some(0x01));
            }
            other => panic!("expected FrameLabel, got {other:?}"),
        }
    }

    #[test]
    fn symbol_class_tag_name_pairs() {
        let mut body = Vec::new();
        body.extend_from_slice(&2u16.to_le_bytes()); // count
        body.extend_from_slice(&5u16.to_le_bytes());
        body.extend_from_slice(b"A\0");
        body.extend_from_slice(&7u16.to_le_bytes());
        body.extend_from_slice(b"BB\0");
        match parse_first(&rec(TAG_SYMBOL_CLASS, &body, true)) {
            Tag::SymbolClass { symbols, .. } => {
                assert_eq!(
                    symbols,
                    vec![(5u16, "A".to_string()), (7u16, "BB".to_string())]
                );
            }
            other => panic!("expected SymbolClass, got {other:?}"),
        }
    }

    #[test]
    fn import_assets2_with_entries() {
        // Exercises the count>0 entries path (absent in the Tier-1 corpus).
        let mut body = Vec::new();
        body.extend_from_slice(b"font.swf\0");
        body.extend_from_slice(&[0x01, 0x00]); // reserved
        body.extend_from_slice(&1u16.to_le_bytes()); // count
        body.extend_from_slice(&9u16.to_le_bytes());
        body.extend_from_slice(b"Sym\0");
        match parse_first(&rec(TAG_IMPORT_ASSETS2, &body, true)) {
            Tag::ImportAssets2 {
                url,
                reserved,
                symbols,
                ..
            } => {
                assert_eq!(url, "font.swf");
                assert_eq!(reserved, [0x01, 0x00]);
                assert_eq!(symbols, vec![(9u16, "Sym".to_string())]);
            }
            other => panic!("expected ImportAssets2, got {other:?}"),
        }
    }

    #[test]
    fn csm_text_settings_fields_and_float_bits() {
        // characterId=0x027c, flags=0x50, thickness=1.5, sharpness=-0.25,
        // reserved=0. Non-zero floats prove the to_bits/from_bits round-trip.
        let mut body = Vec::new();
        body.extend_from_slice(&0x027cu16.to_le_bytes());
        body.push(0x50);
        body.extend_from_slice(&1.5f32.to_bits().to_le_bytes());
        body.extend_from_slice(&(-0.25f32).to_bits().to_le_bytes());
        body.push(0x00);
        match parse_first(&rec(TAG_CSM_TEXT_SETTINGS, &body, true)) {
            Tag::CsmTextSettings {
                character_id,
                flags,
                thickness,
                sharpness,
                reserved,
                ..
            } => {
                assert_eq!(character_id, 0x027c);
                assert_eq!(flags, 0x50);
                assert_eq!(thickness, 1.5);
                assert_eq!(sharpness, -0.25);
                assert_eq!(reserved, 0x00);
            }
            other => panic!("expected CsmTextSettings, got {other:?}"),
        }
    }

    #[test]
    fn unterminated_string_is_an_error() {
        // A Metadata body with no NUL must error, not silently round-trip wrong.
        let d = wrap(&rec(TAG_METADATA, b"no-nul-here", true));
        match Movie::parse(&d) {
            Err(GfxError::UnterminatedString { code }) => assert_eq!(code, TAG_METADATA),
            other => panic!("expected UnterminatedString, got {other:?}"),
        }
    }

    /// Field-level assertions against a known real corpus file. Skips (does not
    /// fail) when assets are absent.
    #[test]
    fn corpus_file_decoded_fields() {
        let path = "/home/banon/er-extract/nuxe-menu-20260619-170932/menu/01_000_fe.gfx";
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => {
                eprintln!("SKIP: {path} not present");
                return;
            }
        };
        let m = Movie::parse(&bytes).expect("parse corpus file");

        // First SetBackgroundColor is the known FromSoft grey 0x666666.
        let bg = m
            .tags
            .iter()
            .find_map(|t| match t {
                Tag::SetBackgroundColor { r, g, b, .. } => Some((*r, *g, *b)),
                _ => None,
            })
            .expect("a SetBackgroundColor");
        assert_eq!(bg, (0x66, 0x66, 0x66));

        // FileAttributes flags for this file are 0x08.
        let attrs = m
            .tags
            .iter()
            .find_map(|t| match t {
                Tag::FileAttributes { flags, .. } => Some(*flags),
                _ => None,
            })
            .expect("a FileAttributes");
        assert_eq!(attrs, 0x08);

        // The top-level SymbolClass exports 387 symbols (ffdec-confirmed count).
        let sym_count = m
            .tags
            .iter()
            .find_map(|t| match t {
                Tag::SymbolClass { symbols, .. } => Some(symbols.len()),
                _ => None,
            })
            .expect("a SymbolClass");
        assert_eq!(sym_count, 387);

        // ImportAssets2 imports "font.swf" with no entries in this file.
        let import_url = m
            .tags
            .iter()
            .find_map(|t| match t {
                Tag::ImportAssets2 { url, symbols, .. } => Some((url.clone(), symbols.len())),
                _ => None,
            })
            .expect("an ImportAssets2");
        assert_eq!(import_url, ("font.swf".to_string(), 0));

        // And it still re-serializes byte-identically end to end.
        assert_eq!(m.write().expect("write"), bytes);
    }

    // --- Tier-2 primitive + tag tests ---------------------------------------

    /// Decode a hex string like "0d8000" into bytes.
    fn hx(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn bit_reader_writer_msb_first_roundtrip() {
        // Mixed unsigned/signed fields, then byte-align.
        let mut bw = BitWriter::new();
        bw.write_ubits(0b101, 3);
        bw.write_sbits(-3, 5); // 11101
        bw.write_ubits(0xab, 8);
        bw.byte_align();
        let bytes = bw.into_bytes();

        let mut br = BitReader::new_at_byte(&bytes, 0);
        assert_eq!(br.read_ubits(3, "T").unwrap(), 0b101);
        assert_eq!(br.read_sbits(5, "T").unwrap(), -3);
        assert_eq!(br.read_ubits(8, "T").unwrap(), 0xab);
        br.byte_align("T").unwrap();
        assert_eq!(br.byte_pos(), bytes.len());
    }

    #[test]
    fn rect_primitive_non_minimal_nbits_roundtrip() {
        // Deliberately over-wide: nbits=12 holds tiny values (minimal would be 2).
        // The primitive must preserve the source width, not recompute a minimal.
        let r = Rect {
            nbits: 12,
            x_min: -1,
            x_max: 1,
            y_min: 0,
            y_max: 2,
        };
        let mut bw = BitWriter::new();
        r.write(&mut bw);
        let bytes = bw.into_bytes();
        let mut br = BitReader::new_at_byte(&bytes, 0);
        let r2 = Rect::read(&mut br).unwrap();
        assert_eq!(r2, r);
        assert_eq!(r2.nbits, 12, "non-minimal nbits must be preserved");
        assert_eq!(br.byte_pos(), bytes.len());
    }

    #[test]
    fn matrix_primitive_non_minimal_translate_bits_exact_bytes() {
        // From corpus 01_000_fe.gfx PlaceObject2 body `0501000d8000`: the MATRIX
        // body is `0d8000`. translate uses 6 bits though the values (-16, 0) only
        // need 5 -- the exporter is non-minimal, so storing translate_nbits=6 is
        // mandatory for byte-identity.
        let m = Matrix {
            has_scale: false,
            scale_nbits: 0,
            scale_x: 0,
            scale_y: 0,
            has_rotate: false,
            rotate_nbits: 0,
            rotate_skew0: 0,
            rotate_skew1: 0,
            translate_nbits: 6,
            translate_x: -16,
            translate_y: 0,
        };
        let mut bw = BitWriter::new();
        m.write(&mut bw);
        assert_eq!(bw.into_bytes(), hx("0d8000"), "exact corpus matrix bytes");

        let bytes = hx("0d8000");
        let mut br = BitReader::new_at_byte(&bytes, 0);
        let decoded = Matrix::read(&mut br).unwrap();
        assert_eq!(decoded, m);
        assert_eq!(
            decoded.translate_nbits, 6,
            "non-minimal translate nbits preserved (minimal would be 5)"
        );
        assert_eq!(br.byte_pos(), bytes.len());
    }

    #[test]
    fn matrix_primitive_with_scale_and_rotate_roundtrip() {
        // Exercise the has_scale/has_rotate paths (rare-ish in corpus) with
        // 16.16-fixed scale and rotate skews; over-wide nbits on purpose.
        let m = Matrix {
            has_scale: true,
            scale_nbits: 20,
            scale_x: 0x1_0000, // 1.0 in 16.16
            scale_y: -0x8000,  // -0.5
            has_rotate: true,
            rotate_nbits: 18,
            rotate_skew0: 0x2000,
            rotate_skew1: -0x100,
            translate_nbits: 14,
            translate_x: 3000, // must fit in 14 signed bits (|v| <= 8191)
            translate_y: -1920,
        };
        let mut bw = BitWriter::new();
        m.write(&mut bw);
        let bytes = bw.into_bytes();
        let mut br = BitReader::new_at_byte(&bytes, 0);
        let m2 = Matrix::read(&mut br).unwrap();
        assert_eq!(m2, m);
        assert_eq!(br.byte_pos(), bytes.len());
    }

    #[test]
    fn cxform_with_alpha_non_minimal_roundtrip() {
        // nbits=10 holding values that fit in fewer bits -- preserve the width.
        let c = CxformWithAlpha {
            has_add: false,
            has_mult: true,
            nbits: 10,
            mult: Some([256, 256, 256, 102]),
            add: None,
        };
        let mut bw = BitWriter::new();
        c.write(&mut bw);
        let bytes = bw.into_bytes();
        let mut br = BitReader::new_at_byte(&bytes, 0);
        let c2 = CxformWithAlpha::read(&mut br).unwrap();
        assert_eq!(c2, c);
        assert_eq!(c2.nbits, 10);
        assert_eq!(br.byte_pos(), bytes.len());
    }

    #[test]
    fn cxform_no_alpha_roundtrip_with_both_terms() {
        // The non-alpha CXFORM primitive (no typed tag uses it yet) with both
        // add and mult terms, including negatives.
        let c = Cxform {
            has_add: true,
            has_mult: true,
            nbits: 9,
            // Values must fit in 9 signed bits (-256..=255).
            mult: Some([255, 128, -64]),
            add: Some([-1, 0, 1]),
        };
        let mut bw = BitWriter::new();
        c.write(&mut bw);
        let bytes = bw.into_bytes();
        let mut br = BitReader::new_at_byte(&bytes, 0);
        let c2 = Cxform::read(&mut br).unwrap();
        assert_eq!(c2, c);
        assert_eq!(br.byte_pos(), bytes.len());
    }

    #[test]
    fn place_object2_fade_cxform_instance() {
        // Corpus 01_002_fe_saveicon.gfx: a fade frame -- Move + HasColorTransform
        // only, depth 1, CXFORMWITHALPHA alpha mult 244 (ffdec-confirmed).
        let body = hx("0901006900401003d0");
        match parse_first(&rec(TAG_PLACE_OBJECT2, &body, false)) {
            Tag::PlaceObject2 {
                flags,
                depth,
                character_id,
                matrix,
                color_transform,
                ratio,
                name,
                clip_depth,
                ..
            } => {
                assert_eq!(flags, 0x09);
                assert_eq!(flags & PO2_MOVE, PO2_MOVE);
                assert_eq!(depth, 1);
                assert_eq!(character_id, None);
                assert_eq!(matrix, None);
                assert_eq!(ratio, None);
                assert_eq!(name, None);
                assert_eq!(clip_depth, None);
                let c = color_transform.expect("a CXFORMWITHALPHA");
                assert!(c.has_mult && !c.has_add);
                assert_eq!(c.nbits, 10);
                assert_eq!(c.mult, Some([256, 256, 256, 244]));
                assert_eq!(c.add, None);
            }
            other => panic!("expected PlaceObject2, got {other:?}"),
        }
    }

    #[test]
    fn place_object2_autosave_icon_full_instance() {
        // Corpus 01_002_fe_saveicon.gfx, force_long: characterId 5, matrix
        // (translate-only, 17 bits, 36000/1920), CXFORMWITHALPHA alpha 102,
        // name "AutoSaveIcon" (all ffdec-confirmed).
        let body = hx("2e01000500228ca003c0006900401001984175746f5361766549636f6e00");
        match parse_first(&rec(TAG_PLACE_OBJECT2, &body, true)) {
            Tag::PlaceObject2 {
                flags,
                depth,
                character_id,
                matrix,
                color_transform,
                ratio,
                name,
                clip_depth,
                force_long,
            } => {
                assert_eq!(flags, 0x2e); // HasChar|HasMatrix|HasCxform|HasName
                assert!(force_long);
                assert_eq!(depth, 1);
                assert_eq!(character_id, Some(5));
                assert_eq!(ratio, None);
                assert_eq!(clip_depth, None);
                assert_eq!(name.as_deref(), Some("AutoSaveIcon"));
                let m = matrix.expect("a MATRIX");
                assert!(!m.has_scale && !m.has_rotate);
                assert_eq!(m.translate_nbits, 17);
                assert_eq!(m.translate_x, 36000);
                assert_eq!(m.translate_y, 1920);
                let c = color_transform.expect("a CXFORMWITHALPHA");
                assert_eq!(c.nbits, 10);
                assert_eq!(c.mult, Some([256, 256, 256, 102]));
            }
            other => panic!("expected PlaceObject2, got {other:?}"),
        }
    }

    #[test]
    fn place_object2_non_minimal_matrix_instance() {
        // Corpus 01_000_fe.gfx: Move + HasMatrix, translate_nbits=6 while the
        // values need only 5 -- proves byte-identity requires the stored width.
        let body = hx("0501000d8000");
        match parse_first(&rec(TAG_PLACE_OBJECT2, &body, false)) {
            Tag::PlaceObject2 {
                flags,
                depth,
                matrix,
                ..
            } => {
                assert_eq!(flags, 0x05);
                assert_eq!(depth, 1);
                let m = matrix.expect("a MATRIX");
                assert_eq!(m.translate_nbits, 6);
                assert_eq!(m.translate_x, -16);
                assert_eq!(m.translate_y, 0);
            }
            other => panic!("expected PlaceObject2, got {other:?}"),
        }
    }

    #[test]
    fn place_object2_with_clip_actions_falls_back_to_unknown() {
        // HasClipActions (0x80) is unmodelled; such a tag must stay Tag::Unknown
        // and still round-trip byte-identically via the opaque body.
        let mut body = vec![0x80u8]; // flags: only HasClipActions
        body.extend_from_slice(&7u16.to_le_bytes()); // depth
        body.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]); // opaque clipActions tail
        match parse_first(&rec(TAG_PLACE_OBJECT2, &body, false)) {
            Tag::Unknown { code, raw, .. } => {
                assert_eq!(code, TAG_PLACE_OBJECT2);
                assert_eq!(raw, body);
            }
            other => panic!("expected Unknown fallback, got {other:?}"),
        }
    }

    #[test]
    fn define_scaling_grid_instance() {
        // Corpus 01_010_messagebox.gfx: characterId 14, RECT nbits=9,
        // (-172,178,-183,184) -- ffdec-confirmed.
        let body = hx("0e004d5165495c00");
        match parse_first(&rec(TAG_DEFINE_SCALING_GRID, &body, false)) {
            Tag::DefineScalingGrid {
                character_id, grid, ..
            } => {
                assert_eq!(character_id, 14);
                assert_eq!(grid.nbits, 9);
                assert_eq!(grid.x_min, -172);
                assert_eq!(grid.x_max, 178);
                assert_eq!(grid.y_min, -183);
                assert_eq!(grid.y_max, 184);
            }
            other => panic!("expected DefineScalingGrid, got {other:?}"),
        }
    }

    // --- Tier-2b: PlaceObject3 (code 70) tests --------------------------------

    #[test]
    fn place_object3_dropshadow_instance() {
        // Corpus 01_000_fe.gfx: Move|HasMatrix|HasName (flags1 0x26), flags2 0x01
        // (HasFilterList), depth 1, characterId 103, name "Text", matrix
        // translate-only (40,40), one DropShadowFilter. Field values are
        // ffdec(-swf2xml)-confirmed: blurX=blurY=4.0, angle=0.7853851 (=45deg),
        // distance=3.0, strength=1.0, color black/alpha 255, compositeSource +
        // 3 passes (flags byte 0x23).
        let body =
            hx("2601010067000ea14054657874000100000000ff00000400000004000fc9000000000300000123");
        match parse_first(&rec(TAG_PLACE_OBJECT3, &body, false)) {
            Tag::PlaceObject3 {
                flags1,
                flags2,
                depth,
                character_id,
                name,
                matrix,
                filters,
                blend_mode,
                bitmap_cache,
                visible,
                class_name,
                color_transform,
                ratio,
                clip_depth,
                force_long,
            } => {
                assert_eq!(flags1, 0x26);
                assert_eq!(flags2, 0x01);
                assert!(!force_long);
                assert_eq!(depth, 1);
                assert_eq!(character_id, Some(103));
                assert_eq!(name.as_deref(), Some("Text"));
                assert_eq!(class_name, None);
                assert_eq!(color_transform, None);
                assert_eq!(ratio, None);
                assert_eq!(clip_depth, None);
                assert_eq!(blend_mode, None);
                assert_eq!(bitmap_cache, None);
                assert_eq!(visible, None);
                let m = matrix.expect("a MATRIX");
                assert!(!m.has_scale && !m.has_rotate);
                assert_eq!(m.translate_nbits, 7);
                assert_eq!((m.translate_x, m.translate_y), (40, 40));
                let fs = filters.expect("a filter list");
                assert_eq!(fs.len(), 1);
                match &fs[0] {
                    Filter::DropShadow {
                        color,
                        blur_x,
                        blur_y,
                        angle,
                        distance,
                        strength,
                        flags,
                    } => {
                        assert_eq!(*color, [0x00, 0x00, 0x00, 0xff]); // black, alpha 255
                        assert_eq!(*blur_x, 0x0004_0000); // 4.0 in 16.16
                        assert_eq!(*blur_y, 0x0004_0000);
                        assert_eq!(*angle, 0x0000_c90f); // 0.7853851 rad (45 deg)
                        assert_eq!(*distance, 0x0003_0000); // 3.0
                        assert_eq!(*strength, 0x0100); // 1.0 in 8.8
                        assert_eq!(*flags, 0x23); // CompositeSource + 3 passes
                    }
                    other => panic!("expected DropShadow, got {other:?}"),
                }
            }
            other => panic!("expected PlaceObject3, got {other:?}"),
        }
    }

    #[test]
    fn place_object3_glow_filter_and_blend_mode_instance() {
        // Corpus 01_000_fe.gfx: flags1 0x1e (HasChar|HasMatrix|HasCxform|HasRatio),
        // flags2 0x03 (HasFilterList|HasBlendMode), depth 3, characterId 107,
        // ratio 18, blendMode 8 (=add), one GlowFilter (ffdec-confirmed: red
        // glow ff0000/alpha 255, blur 0, strength 0, compositeSource + 1 pass =
        // flags 0x21).
        let body = hx("1e0303006b0017cde91869004010000012000102ff0000ff000000000000000000002108");
        match parse_first(&rec(TAG_PLACE_OBJECT3, &body, false)) {
            Tag::PlaceObject3 {
                flags1,
                flags2,
                depth,
                character_id,
                ratio,
                blend_mode,
                filters,
                color_transform,
                ..
            } => {
                assert_eq!(flags1, 0x1e);
                assert_eq!(flags2, 0x03);
                assert_eq!(depth, 3);
                assert_eq!(character_id, Some(107));
                assert_eq!(ratio, Some(18));
                assert_eq!(blend_mode, Some(8)); // SWF blend mode 8 = add
                assert!(color_transform.is_some());
                let fs = filters.expect("a filter list");
                assert_eq!(fs.len(), 1);
                match &fs[0] {
                    Filter::Glow {
                        color,
                        blur_x,
                        blur_y,
                        strength,
                        flags,
                    } => {
                        assert_eq!(*color, [0xff, 0x00, 0x00, 0xff]); // red, alpha 255
                        assert_eq!(*blur_x, 0);
                        assert_eq!(*blur_y, 0);
                        assert_eq!(*strength, 0);
                        assert_eq!(*flags, 0x21); // CompositeSource + 1 pass
                    }
                    other => panic!("expected Glow, got {other:?}"),
                }
            }
            other => panic!("expected PlaceObject3, got {other:?}"),
        }
    }

    #[test]
    fn filter_dropshadow_roundtrip() {
        // A DropShadowFilter primitive must re-encode to the exact corpus bytes.
        let raw = hx("00000000ff00000400000004000fc9000000000300000123");
        let mut r = GfxReader::new(&raw);
        let f = Filter::read(&mut r).unwrap().expect("a modelled filter");
        assert_eq!(r.pos, raw.len(), "filter consumed its whole body");
        match &f {
            Filter::DropShadow {
                angle, strength, ..
            } => {
                assert_eq!(*angle, 0x0000_c90f);
                assert_eq!(*strength, 0x0100);
            }
            other => panic!("expected DropShadow, got {other:?}"),
        }
        let mut w = GfxWriter::new();
        f.write(&mut w);
        assert_eq!(w.buf, raw, "DropShadow re-encode byte-identical");
    }

    #[test]
    fn filter_unknown_id_falls_back_to_unknown() {
        // A filter id we do not model (e.g. 3 = BevelFilter) must force the whole
        // PlaceObject3 back to Tag::Unknown, re-emitting the raw body verbatim.
        let mut body = vec![0x00u8, 0x01]; // flags1=0 (no PO2 fields), flags2=HasFilterList
        body.extend_from_slice(&9u16.to_le_bytes()); // depth
        body.push(0x01); // filter count = 1
        body.push(0x03); // filter id 3 (BevelFilter -- unmodelled)
        body.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]); // opaque filter tail
        match parse_first(&rec(TAG_PLACE_OBJECT3, &body, false)) {
            Tag::Unknown { code, raw, .. } => {
                assert_eq!(code, TAG_PLACE_OBJECT3);
                assert_eq!(raw, body);
            }
            other => panic!("expected Unknown fallback, got {other:?}"),
        }
    }

    #[test]
    fn place_object3_clip_actions_falls_back_to_unknown() {
        // clipActions (flags1 0x80) is unmodelled; such a tag stays Tag::Unknown
        // and round-trips via the opaque body. (None occur in the corpus.)
        let mut body = vec![0x80u8, 0x00]; // flags1 HasClipActions, flags2 none
        body.extend_from_slice(&7u16.to_le_bytes()); // depth
        body.extend_from_slice(&[0xca, 0xfe, 0xba, 0xbe]); // opaque clipActions tail
        match parse_first(&rec(TAG_PLACE_OBJECT3, &body, false)) {
            Tag::Unknown { code, raw, .. } => {
                assert_eq!(code, TAG_PLACE_OBJECT3);
                assert_eq!(raw, body);
            }
            other => panic!("expected Unknown fallback, got {other:?}"),
        }
    }

    #[test]
    fn place_object3_reserved_flag_falls_back_to_unknown() {
        // A reserved flags2 bit (0xc0 mask) has unverifiable semantics (never set
        // in the corpus); setting one must fall the tag back to Tag::Unknown.
        let mut body = vec![0x00u8, 0x40]; // flags2 = OpaqueBackground (reserved here)
        body.extend_from_slice(&3u16.to_le_bytes()); // depth
        body.extend_from_slice(&[0x12, 0x34]); // opaque tail
        match parse_first(&rec(TAG_PLACE_OBJECT3, &body, false)) {
            Tag::Unknown { code, raw, .. } => {
                assert_eq!(code, TAG_PLACE_OBJECT3);
                assert_eq!(raw, body);
            }
            other => panic!("expected Unknown fallback, got {other:?}"),
        }
    }

    #[test]
    fn place_object3_visible_and_classname_synthetic_roundtrip() {
        // HasClassName (0x08) and HasVisible (0x20) never occur in the corpus but
        // are modelled. This synthetic instance exercises both the class-name
        // string and the visible+background fields so the encoder/decoder stay
        // mutually consistent (byte-identity self-check).
        let mut body = Vec::new();
        body.push(0x00u8); // flags1: no PO2 optional fields
        body.push(0x08 | 0x20); // flags2: HasClassName | HasVisible
        body.extend_from_slice(&5u16.to_le_bytes()); // depth
        body.extend_from_slice(b"my.Class\0"); // class name
        body.push(0x01); // visible = 1
        body.extend_from_slice(&[0x10, 0x20, 0x30, 0x40]); // background RGBA
        match parse_first(&rec(TAG_PLACE_OBJECT3, &body, false)) {
            Tag::PlaceObject3 {
                flags2,
                depth,
                class_name,
                visible,
                ..
            } => {
                assert_eq!(flags2, 0x28);
                assert_eq!(depth, 5);
                assert_eq!(class_name.as_deref(), Some("my.Class"));
                assert_eq!(visible, Some((0x01, [0x10, 0x20, 0x30, 0x40])));
            }
            other => panic!("expected PlaceObject3, got {other:?}"),
        }
    }

    #[test]
    fn non_zero_bit_padding_is_rejected() {
        // A RECT whose alignment padding bits are non-zero must fail loudly
        // rather than silently dropping them. nbits=0 -> 5 bits used, 3 pad bits;
        // set a pad bit. Byte 0b00000_001 = 0x01.
        let body = vec![0x01u8];
        let mut br = BitReader::new_at_byte(&body, 0);
        match Rect::read(&mut br) {
            Err(GfxError::NonZeroBitPadding { context }) => assert_eq!(context, "RECT"),
            other => panic!("expected NonZeroBitPadding, got {other:?}"),
        }
    }
}
