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

    /// The `DefineShape` family (`DefineShape` 2, `DefineShape2` 22,
    /// `DefineShape3` 32, `DefineShape4` 83). All four are modelled by one
    /// variant discriminated by its `version` field (1..=4), which governs
    /// RGB-vs-RGBA colors, LINESTYLE-vs-LINESTYLE2, the `0xFF`-extended fill/line
    /// counts, and StateNewStyles availability.
    ///
    /// The body is decode-then-verified: the parser reproduces the whole
    /// SHAPEWITHSTYLE bitstream (every source bit width preserved verbatim --
    /// the edge `NumBits` are non-minimal in 1,133 of the 16,902 corpus edge
    /// records) and the decoder re-serializes it; if that does not reproduce the
    /// source body byte-for-byte (an unmodelled fill type, line-style sub-form,
    /// gradient layout, or any structural surprise), the tag falls back to
    /// [`Tag::Unknown`] so byte-identity can never be silently lost. All 366
    /// `DefineShape*` across the 114-file corpus decode to this typed variant.
    DefineShape {
        /// Shape version 1/2/3/4 (tag code 2/22/32/83).
        version: u8,
        shape_id: u16,
        /// The shape's bounding box `RECT`.
        shape_bounds: Rect,
        /// `edgeBounds` RECT (DefineShape4 only; `None` otherwise).
        edge_bounds: Option<Rect>,
        /// The DefineShape4 flags byte (`UsesFillWindingRule`/
        /// `UsesNonScalingStrokes`/`UsesScalingStrokes` in its low bits), stored
        /// verbatim. `None` for versions 1/2/3.
        flags_byte: Option<u8>,
        /// The `SHAPEWITHSTYLE` (fill/line style arrays + shape records).
        shapes: ShapeWithStyle,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },

    /// `DefineEditText` (code 37): a dynamic/input text field -- the dominant
    /// text mechanism in the corpus (1,479 instances across 102 files). Both
    /// flag bytes are stored verbatim and are the source of truth for which
    /// optional field is present (each is `Some` iff its flag bit is set).
    ///
    /// Field order (corpus-proven byte-identical over all instances): `bounds`
    /// `RECT` (byte-aligns), `flags1`+`flags2`, `font_id` (`flags1` `HasFont`),
    /// `font_class` (`flags2` `HasFontClass`), `font_height` (present iff
    /// `HasFont` OR `HasFontClass`), `text_color` (`flags1` `HasTextColor`,
    /// `RGBA`), `max_length` (`flags1` `HasMaxLength`), `layout` (`flags2`
    /// `HasLayout`), `variable_name` (always), `initial_text` (`flags1`
    /// `HasText`).
    ///
    /// Decode-then-verified: if the parsed fields do not re-serialize to the
    /// exact source body (e.g. a non-UTF-8 string, or any structural surprise),
    /// the tag falls back to [`Tag::Unknown`] so byte-identity is never lost.
    DefineEditText {
        character_id: u16,
        /// The field's bounding box `RECT`.
        bounds: Rect,
        /// First flags byte (`HasText`/`WordWrap`/`Multiline`/`Password`/
        /// `ReadOnly`/`HasTextColor`/`HasMaxLength`/`HasFont`, MSB-to-LSB).
        flags1: u8,
        /// Second flags byte (`HasFontClass`/`AutoSize`/`HasLayout`/`NoSelect`/
        /// `Border`/`WasStatic`/`HTML`/`UseOutlines`, MSB-to-LSB).
        flags2: u8,
        /// `FontID` (flag `flags1` `HasFont` `0x01`).
        font_id: Option<u16>,
        /// `FontClass` name (flag `flags2` `HasFontClass` `0x80`); NUL implicit.
        font_class: Option<String>,
        /// `FontHeight` in twips, present iff `HasFont` OR `HasFontClass`.
        font_height: Option<u16>,
        /// `[red, green, blue, alpha]` text color (flag `flags1` `HasTextColor`
        /// `0x04`).
        text_color: Option<[u8; 4]>,
        /// `MaxLength` (flag `flags1` `HasMaxLength` `0x02`). Never set in the
        /// corpus, but modelled.
        max_length: Option<u16>,
        /// Layout block (flag `flags2` `HasLayout` `0x20`).
        layout: Option<EditTextLayout>,
        /// `VariableName` (always present); NUL implicit. Empty across the corpus.
        variable_name: String,
        /// `InitialText` (flag `flags1` `HasText` `0x80`); NUL implicit.
        initial_text: Option<String>,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },

    /// `DefineFont3` (code 75): a glyph font (7 fonts across 3 corpus files).
    /// The `flags` byte is stored verbatim and is the source of truth for
    /// `WideOffsets` (offset table width), `WideCodes` (code/kerning width), and
    /// `HasLayout` (layout-block presence).
    ///
    /// The glyph offset table values are stored verbatim (never recomputed) so
    /// byte-identity holds even if the exporter's offsets were non-canonical;
    /// each glyph `SHAPE` reuses the Tier-3 edge bitstream (its own
    /// `NumFillBits`/`NumLineBits` header, no style arrays). Decode-then-verified:
    /// any byte mismatch falls the whole tag back to [`Tag::Unknown`].
    DefineFont3 {
        font_id: u16,
        /// Raw flags byte (`HasLayout`/`ShiftJIS`/`SmallText`/`ANSI`/
        /// `WideOffsets`/`WideCodes`/`Italic`/`Bold`, MSB-to-LSB).
        flags: u8,
        /// `LanguageCode` byte (stored raw).
        language_code: u8,
        /// `FontName`, length-prefixed (the `u8` length is `font_name.len()`).
        /// Stored as raw bytes because the exporter includes a trailing NUL
        /// *inside* the counted length and the bytes are not guaranteed UTF-8.
        font_name: Vec<u8>,
        /// The glyph offset table plus trailing code-table offset
        /// (`glyphs.len() + 1` values), stored verbatim. Emitted as `u32` iff
        /// `flags & WideOffsets`, else `u16`.
        offsets: Vec<u32>,
        /// The `numGlyphs` glyph `SHAPE`s.
        glyphs: Vec<GlyphShape>,
        /// The `numGlyphs` character codes. Emitted as `u16` iff `flags &
        /// WideCodes`, else `u8`.
        codes: Vec<u16>,
        /// Layout block (`flags & HasLayout`): ascent/descent/leading, the
        /// per-glyph advance + bounds tables, and the kerning table.
        layout: Option<Font3Layout>,
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
    /// A FILLSTYLE carried a type byte the Tier-3 shape codec does not model.
    /// Internally signals the owning `DefineShape*` to fall back to
    /// [`Tag::Unknown`] (the raw body still re-emits byte-identically).
    UnknownFillStyleType(u8),
    /// A SHAPERECORD set StateNewStyles in a `DefineShape` (version 1) body,
    /// where that feature does not exist. Signals a fall-back to
    /// [`Tag::Unknown`].
    ShapeNewStylesUnsupported,
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
            GfxError::UnknownFillStyleType(t) => {
                write!(f, "unmodelled FILLSTYLE type 0x{t:02x}")
            }
            GfxError::ShapeNewStylesUnsupported => {
                write!(f, "StateNewStyles in a DefineShape (version 1) body")
            }
        }
    }
}

impl std::error::Error for GfxError {}
