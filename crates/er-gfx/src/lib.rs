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

// --- Tier-3 typed tag codes (the DefineShape family + SHAPEWITHSTYLE). ---
/// `DefineShape` (code 2): shape version 1. RGB colors, LINESTYLE, no extended
/// fill/line counts, no StateNewStyles.
const TAG_DEFINE_SHAPE: u16 = 2;
/// `DefineShape2` (code 22): shape version 2. RGB colors, LINESTYLE, adds the
/// `0xFF`-extended u16 fill/line counts and StateNewStyles.
const TAG_DEFINE_SHAPE2: u16 = 22;
/// `DefineShape3` (code 32): shape version 3. RGBA colors, LINESTYLE.
const TAG_DEFINE_SHAPE3: u16 = 32;
/// `DefineShape4` (code 83): shape version 4. RGBA colors, LINESTYLE2, plus an
/// `edgeBounds` RECT and a flags byte before the SHAPEWITHSTYLE.
const TAG_DEFINE_SHAPE4: u16 = 83;

// --- Tier-4 typed tag codes (text/font tags reusing the RECT + SHAPE machinery). ---
/// `DefineEditText` (code 37): a dynamic/input text field. Body is a
/// `characterId`, a bounds `RECT` (byte-aligns), a 2-byte flag field, then a set
/// of per-flag optional fields and two trailing strings. See [`Tag::DefineEditText`].
const TAG_DEFINE_EDIT_TEXT: u16 = 37;
/// `DefineFont3` (code 75): a glyph font. Body is a `fontId`, a flags byte, a
/// language code, a length-prefixed font name, then the glyph offset table, the
/// glyph `SHAPE`s (reusing the edge bitstream), a code table, and an optional
/// layout block (advances, glyph bounds, kerning). See [`Tag::DefineFont3`].
const TAG_DEFINE_FONT3: u16 = 75;

// --- DefineEditText flag byte 1 (MSB-to-LSB, SWF bit order). The 8 bits are
// stored verbatim in `flags1`; the four that gate an optional field are branched
// on, the rest are documented but carry no extra body. ---
/// `HasText`: an `initialText` cstring follows (last field).
const ET_HAS_TEXT: u8 = 0x80;
/// `WordWrap`: word-wrap rendering hint. No extra field.
#[allow(dead_code)]
const ET_WORD_WRAP: u8 = 0x40;
/// `Multiline`: multi-line field hint. No extra field.
#[allow(dead_code)]
const ET_MULTILINE: u8 = 0x20;
/// `Password`: password field hint. No extra field.
#[allow(dead_code)]
const ET_PASSWORD: u8 = 0x10;
/// `ReadOnly`: read-only field hint. No extra field.
#[allow(dead_code)]
const ET_READONLY: u8 = 0x08;
/// `HasTextColor`: an `RGBA` text color follows.
const ET_HAS_TEXT_COLOR: u8 = 0x04;
/// `HasMaxLength`: a `u16` max length follows. Never set in the corpus, but
/// modelled (decode-then-verify keeps it safe either way).
const ET_HAS_MAX_LENGTH: u8 = 0x02;
/// `HasFont`: a `u16` `FontID` follows (and, jointly with `HasFontClass`, a
/// `FontHeight`).
const ET_HAS_FONT: u8 = 0x01;

// --- DefineEditText flag byte 2 (MSB-to-LSB, SWF bit order), stored in `flags2`. ---
/// `HasFontClass`: a `FontClass` cstring follows (and a `FontHeight`).
const ET2_HAS_FONT_CLASS: u8 = 0x80;
/// `AutoSize`: auto-size hint. No extra field.
#[allow(dead_code)]
const ET2_AUTOSIZE: u8 = 0x40;
/// `HasLayout`: a layout block (align + margins + indent + leading) follows.
const ET2_HAS_LAYOUT: u8 = 0x20;
/// `NoSelect`: non-selectable hint. No extra field.
#[allow(dead_code)]
const ET2_NOSELECT: u8 = 0x10;
/// `Border`: draw-border hint. No extra field.
#[allow(dead_code)]
const ET2_BORDER: u8 = 0x08;
/// `WasStatic`: was-static hint. No extra field.
#[allow(dead_code)]
const ET2_WAS_STATIC: u8 = 0x04;
/// `HTML`: the `initialText` is HTML. No extra field (the text is still a cstring).
#[allow(dead_code)]
const ET2_HTML: u8 = 0x02;
/// `UseOutlines`: render with font outlines. No extra field.
#[allow(dead_code)]
const ET2_USE_OUTLINES: u8 = 0x01;

// --- DefineFont3 flags byte (MSB-to-LSB, SWF bit order), stored verbatim. ---
/// `HasLayout`: the trailing layout block (ascent/descent/leading + advance,
/// bounds, and kerning tables) is present.
const F3_HAS_LAYOUT: u8 = 0x80;
/// `ShiftJIS`: codes are Shift-JIS. No structural effect here. Never set in the
/// corpus.
#[allow(dead_code)]
const F3_SHIFT_JIS: u8 = 0x40;
/// `SmallText`: small-text rendering hint. No structural effect.
#[allow(dead_code)]
const F3_SMALL_TEXT: u8 = 0x20;
/// `ANSI`: codes are ANSI. No structural effect here. Never set in the corpus.
#[allow(dead_code)]
const F3_ANSI: u8 = 0x10;
/// `WideOffsets`: the glyph offset table (and code-table offset) entries are
/// `u32` rather than `u16`.
const F3_WIDE_OFFSETS: u8 = 0x08;
/// `WideCodes`: code-table (and kerning-record code) entries are `u16` rather
/// than `u8`. Always set for DefineFont3 in the corpus.
const F3_WIDE_CODES: u8 = 0x04;
/// `Italic`: italic hint. No structural effect.
#[allow(dead_code)]
const F3_ITALIC: u8 = 0x02;
/// `Bold`: bold hint. No structural effect.
#[allow(dead_code)]
const F3_BOLD: u8 = 0x01;

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

    /// Read one whole byte at the current (byte-aligned) position. Used by the
    /// shape codec, whose FILLSTYLEARRAY/LINESTYLEARRAY/GRADRECORD fields are
    /// byte-structured even though they are embedded in the larger bitstream.
    fn read_u8_aligned(&mut self, context: &'static str) -> Result<u8, GfxError> {
        debug_assert_eq!(self.bitpos & 7, 0, "read_u8_aligned not byte aligned");
        let idx = self.bitpos >> 3;
        if idx >= self.data.len() {
            return Err(GfxError::BitstreamEof { context });
        }
        let v = self.data[idx];
        self.bitpos += 8;
        Ok(v)
    }

    /// Read a little-endian `u16` at the current (byte-aligned) position.
    fn read_u16_aligned(&mut self, context: &'static str) -> Result<u16, GfxError> {
        let lo = self.read_u8_aligned(context)?;
        let hi = self.read_u8_aligned(context)?;
        Ok(u16::from_le_bytes([lo, hi]))
    }

    /// Read `n` whole bytes at the current (byte-aligned) position.
    fn read_bytes_aligned(&mut self, n: usize, context: &'static str) -> Result<Vec<u8>, GfxError> {
        debug_assert_eq!(self.bitpos & 7, 0, "read_bytes_aligned not byte aligned");
        let idx = self.bitpos >> 3;
        if idx + n > self.data.len() {
            return Err(GfxError::BitstreamEof { context });
        }
        let s = self.data[idx..idx + n].to_vec();
        self.bitpos += n * 8;
        Ok(s)
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

    /// Write one whole byte at the current (byte-aligned) position. Counterpart
    /// to [`BitReader::read_u8_aligned`]; used by the shape codec.
    fn write_u8_aligned(&mut self, v: u8) {
        debug_assert_eq!(self.nbits, 0, "write_u8_aligned not byte aligned");
        self.buf.push(v);
    }

    /// Write a little-endian `u16` at the current (byte-aligned) position.
    fn write_u16_aligned(&mut self, v: u16) {
        self.write_u8_aligned((v & 0xff) as u8);
        self.write_u8_aligned((v >> 8) as u8);
    }

    /// Write whole bytes at the current (byte-aligned) position.
    fn write_bytes_aligned(&mut self, b: &[u8]) {
        debug_assert_eq!(self.nbits, 0, "write_bytes_aligned not byte aligned");
        self.buf.extend_from_slice(b);
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

// ===========================================================================
// Tier-3: the DefineShape family + SHAPEWITHSTYLE / SHAPERECORD bitstream.
// ===========================================================================
//
// A SHAPEWITHSTYLE is one continuous MSB-first bitstream that mixes byte-aligned
// sub-structures (FILLSTYLEARRAY, LINESTYLEARRAY, GRADRECORD, the embedded
// MATRIX/RECT primitives) with truly bit-packed SHAPERECORDs. Every source bit
// width is preserved verbatim because the Scaleform exporter is non-minimal: of
// the 16,902 edge records in the 114-file corpus, 1,133 use more delta bits than
// the minimal width (the SHAPEWITHSTYLE NumFillBits/NumLineBits happen to be
// minimal in this corpus, but are still stored, never recomputed). The decoder
// re-serializes every parsed shape and compares against the source body; any
// mismatch (or structural surprise) falls the whole tag back to [`Tag::Unknown`]
// so the raw body re-emits byte-identically -- see [`decode_define_shape`].

/// A shape color: 3-byte `RGB` (DefineShape/DefineShape2) or 4-byte `RGBA`
/// (DefineShape3/DefineShape4). The byte width is dictated by the shape version,
/// captured here so colors re-encode without re-deriving the width.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Color {
    /// 3-byte `[red, green, blue]`.
    Rgb([u8; 3]),
    /// 4-byte `[red, green, blue, alpha]`.
    Rgba([u8; 4]),
}

impl Color {
    fn read(br: &mut BitReader, rgba: bool, context: &'static str) -> Result<Color, GfxError> {
        if rgba {
            let b = br.read_bytes_aligned(4, context)?;
            Ok(Color::Rgba([b[0], b[1], b[2], b[3]]))
        } else {
            let b = br.read_bytes_aligned(3, context)?;
            Ok(Color::Rgb([b[0], b[1], b[2]]))
        }
    }

    fn write(&self, bw: &mut BitWriter) {
        match self {
            Color::Rgb(c) => bw.write_bytes_aligned(c),
            Color::Rgba(c) => bw.write_bytes_aligned(c),
        }
    }
}

/// One `GRADRECORD`: a ratio (`0..=255`) and a [`Color`] (RGB for Shape1/2,
/// RGBA for Shape3/4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GradRecord {
    pub ratio: u8,
    pub color: Color,
}

/// A `GRADIENT` / `FOCALGRADIENT`. The `(SpreadMode:2, InterpolationMode:2,
/// NumGradients:4)` byte is bit-packed (empirically the same layout for every
/// shape version in the corpus); `NumGradients` is derived from `records.len()`
/// on write. `focal_point` is the trailing `FIXED8` present only for focal
/// gradients (fill type `0x13`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gradient {
    /// `SpreadMode` (2 bits): 0 pad, 1 reflect, 2 repeat.
    pub spread_mode: u8,
    /// `InterpolationMode` (2 bits): 0 normal RGB, 1 linear RGB.
    pub interpolation_mode: u8,
    /// Gradient stops (`NumGradients` is `records.len()`, max 15).
    pub records: Vec<GradRecord>,
    /// Focal point `FIXED8` (raw little-endian `i16`), present iff focal-radial.
    pub focal_point: Option<i16>,
}

impl Gradient {
    fn read(
        br: &mut BitReader,
        rgba: bool,
        focal: bool,
        context: &'static str,
    ) -> Result<Gradient, GfxError> {
        let spread_mode = br.read_ubits(2, context)? as u8;
        let interpolation_mode = br.read_ubits(2, context)? as u8;
        let num = br.read_ubits(4, context)?;
        // 2 + 2 + 4 = 8 bits -> back to byte alignment for the GRADRECORDs.
        let mut records = Vec::with_capacity(num as usize);
        for _ in 0..num {
            let ratio = br.read_u8_aligned(context)?;
            let color = Color::read(br, rgba, context)?;
            records.push(GradRecord { ratio, color });
        }
        let focal_point = if focal {
            Some(br.read_u16_aligned(context)? as i16)
        } else {
            None
        };
        Ok(Gradient {
            spread_mode,
            interpolation_mode,
            records,
            focal_point,
        })
    }

    fn write(&self, bw: &mut BitWriter) {
        bw.write_ubits(self.spread_mode as u32, 2);
        bw.write_ubits(self.interpolation_mode as u32, 2);
        bw.write_ubits(self.records.len() as u32, 4);
        for r in &self.records {
            bw.write_u8_aligned(r.ratio);
            r.color.write(bw);
        }
        if let Some(fp) = self.focal_point {
            bw.write_u16_aligned(fp as u16);
        }
    }
}

/// A `FILLSTYLE`. The leading type byte is stored inside the `Gradient`/`Bitmap`
/// variants (`fill_type`) so the exact sub-kind (`0x10` linear / `0x12` radial /
/// `0x13` focal; `0x40`-`0x43` bitmap clip/tile flavors) is reproduced verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FillStyle {
    /// Solid color (type `0x00`): `RGB` (Shape1/2) or `RGBA` (Shape3/4).
    Solid(Color),
    /// Gradient (type `0x10` linear, `0x12` radial, `0x13` focal-radial): a
    /// `MATRIX` and a [`Gradient`].
    Gradient {
        /// Fill type byte (`0x10`/`0x12`/`0x13`), preserved verbatim.
        fill_type: u8,
        matrix: Matrix,
        gradient: Gradient,
    },
    /// Bitmap fill (types `0x40`-`0x43`): a `bitmapId` and a `MATRIX`.
    Bitmap {
        /// Fill type byte (`0x40`-`0x43`), preserved verbatim.
        fill_type: u8,
        bitmap_id: u16,
        matrix: Matrix,
    },
}

impl FillStyle {
    const CTX: &'static str = "FILLSTYLE";

    fn read(br: &mut BitReader, rgba: bool) -> Result<FillStyle, GfxError> {
        let t = br.read_u8_aligned(Self::CTX)?;
        match t {
            0x00 => Ok(FillStyle::Solid(Color::read(br, rgba, Self::CTX)?)),
            0x10 | 0x12 | 0x13 => {
                let matrix = Matrix::read(br)?;
                let gradient = Gradient::read(br, rgba, t == 0x13, Self::CTX)?;
                Ok(FillStyle::Gradient {
                    fill_type: t,
                    matrix,
                    gradient,
                })
            }
            0x40 | 0x41 | 0x42 | 0x43 => {
                let bitmap_id = br.read_u16_aligned(Self::CTX)?;
                let matrix = Matrix::read(br)?;
                Ok(FillStyle::Bitmap {
                    fill_type: t,
                    bitmap_id,
                    matrix,
                })
            }
            other => Err(GfxError::UnknownFillStyleType(other)),
        }
    }

    fn write(&self, bw: &mut BitWriter) {
        match self {
            FillStyle::Solid(color) => {
                bw.write_u8_aligned(0x00);
                color.write(bw);
            }
            FillStyle::Gradient {
                fill_type,
                matrix,
                gradient,
            } => {
                bw.write_u8_aligned(*fill_type);
                matrix.write(bw);
                gradient.write(bw);
            }
            FillStyle::Bitmap {
                fill_type,
                bitmap_id,
                matrix,
            } => {
                bw.write_u8_aligned(*fill_type);
                bw.write_u16_aligned(*bitmap_id);
                matrix.write(bw);
            }
        }
    }
}

/// A `FILLSTYLEARRAY`: a count (`u8`, or `0xFF`-extended `u16` for Shape2/3/4)
/// then the fill styles. `count_ext` records whether the extended form was used
/// so a non-minimal count encoding (none occur in the corpus) is preserved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillStyleArray {
    /// Whether the `0xFF`-extended `u16` count form was used.
    pub count_ext: bool,
    pub styles: Vec<FillStyle>,
}

impl FillStyleArray {
    const CTX: &'static str = "FILLSTYLEARRAY";

    fn read(br: &mut BitReader, version: u8, rgba: bool) -> Result<FillStyleArray, GfxError> {
        let first = br.read_u8_aligned(Self::CTX)?;
        let (count, count_ext) = if first == 0xFF && version >= 2 {
            (br.read_u16_aligned(Self::CTX)? as usize, true)
        } else {
            (first as usize, false)
        };
        let mut styles = Vec::with_capacity(count);
        for _ in 0..count {
            styles.push(FillStyle::read(br, rgba)?);
        }
        Ok(FillStyleArray { count_ext, styles })
    }

    fn write(&self, bw: &mut BitWriter) {
        if self.count_ext {
            bw.write_u8_aligned(0xFF);
            bw.write_u16_aligned(self.styles.len() as u16);
        } else {
            bw.write_u8_aligned(self.styles.len() as u8);
        }
        for fs in &self.styles {
            fs.write(bw);
        }
    }
}

/// The fill carried by a `LINESTYLE2` (DefineShape4): either an `RGBA` color or,
/// when its `HasFill` flag is set, a nested [`FillStyle`] (never set in the
/// corpus, but modelled).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineFill {
    /// `[red, green, blue, alpha]` line color (HasFill clear).
    Color([u8; 4]),
    /// A nested fill style (HasFill set).
    Fill(Box<FillStyle>),
}

/// A `LINESTYLE` (Shape1/2/3) or `LINESTYLE2` (Shape4). For `LINESTYLE2` the
/// 16-bit caps/join/flags word is stored verbatim and governs the optional
/// `miter_limit` (present iff JoinStyle == 2) and whether `fill` is a color or a
/// nested fill (HasFill bit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineStyle {
    /// `LINESTYLE`: width + `RGB` (Shape1/2) or `RGBA` (Shape3) color.
    Plain { width: u16, color: Color },
    /// `LINESTYLE2`: width + 16-bit flags + optional miter limit + fill.
    Style2 {
        width: u16,
        /// The 16-bit caps/join/HasFill/NoHScale/NoVScale/PixelHinting/NoClose/
        /// EndCap flags word, stored verbatim.
        flags: u16,
        /// `MiterLimitFactor` (`u16`), present iff JoinStyle (`flags` bits 12-13)
        /// is 2.
        miter_limit: Option<u16>,
        /// Color (HasFill clear) or nested fill (HasFill set, `flags` bit 11).
        fill: LineFill,
    },
}

impl LineStyle {
    const CTX: &'static str = "LINESTYLE";

    fn read(br: &mut BitReader, version: u8, rgba: bool) -> Result<LineStyle, GfxError> {
        let width = br.read_u16_aligned(Self::CTX)?;
        if version == 4 {
            let flags = br.read_u16_aligned(Self::CTX)?;
            let join = (flags >> 12) & 0x3;
            let has_fill = (flags >> 11) & 0x1 != 0;
            let miter_limit = if join == 2 {
                Some(br.read_u16_aligned(Self::CTX)?)
            } else {
                None
            };
            let fill = if has_fill {
                LineFill::Fill(Box::new(FillStyle::read(br, rgba)?))
            } else {
                let b = br.read_bytes_aligned(4, Self::CTX)?;
                LineFill::Color([b[0], b[1], b[2], b[3]])
            };
            Ok(LineStyle::Style2 {
                width,
                flags,
                miter_limit,
                fill,
            })
        } else {
            let color = Color::read(br, rgba, Self::CTX)?;
            Ok(LineStyle::Plain { width, color })
        }
    }

    fn write(&self, bw: &mut BitWriter) {
        match self {
            LineStyle::Plain { width, color } => {
                bw.write_u16_aligned(*width);
                color.write(bw);
            }
            LineStyle::Style2 {
                width,
                flags,
                miter_limit,
                fill,
            } => {
                bw.write_u16_aligned(*width);
                bw.write_u16_aligned(*flags);
                if let Some(m) = miter_limit {
                    bw.write_u16_aligned(*m);
                }
                match fill {
                    LineFill::Color(c) => bw.write_bytes_aligned(c),
                    LineFill::Fill(fs) => fs.write(bw),
                }
            }
        }
    }
}

/// A `LINESTYLEARRAY`: a count (`u8`, or `0xFF`-extended `u16` for Shape2/3/4)
/// then the line styles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineStyleArray {
    /// Whether the `0xFF`-extended `u16` count form was used.
    pub count_ext: bool,
    pub styles: Vec<LineStyle>,
}

impl LineStyleArray {
    const CTX: &'static str = "LINESTYLEARRAY";

    fn read(br: &mut BitReader, version: u8, rgba: bool) -> Result<LineStyleArray, GfxError> {
        let first = br.read_u8_aligned(Self::CTX)?;
        let (count, count_ext) = if first == 0xFF && version >= 2 {
            (br.read_u16_aligned(Self::CTX)? as usize, true)
        } else {
            (first as usize, false)
        };
        let mut styles = Vec::with_capacity(count);
        for _ in 0..count {
            styles.push(LineStyle::read(br, version, rgba)?);
        }
        Ok(LineStyleArray { count_ext, styles })
    }

    fn write(&self, bw: &mut BitWriter) {
        if self.count_ext {
            bw.write_u8_aligned(0xFF);
            bw.write_u16_aligned(self.styles.len() as u16);
        } else {
            bw.write_u8_aligned(self.styles.len() as u8);
        }
        for ls in &self.styles {
            ls.write(bw);
        }
    }
}

/// A `MOVETO` sub-record of a STYLECHANGERECORD. `num_bits` (the source
/// `MoveBits`) is stored verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveTo {
    /// `MoveBits` (the signed delta width).
    pub num_bits: u32,
    pub dx: i32,
    pub dy: i32,
}

/// The geometry of a STRAIGHTEDGERECORD: a general line (both deltas), or a
/// horizontal/vertical line (one delta).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StraightEdge {
    /// `GeneralLineFlag` set: both deltas present.
    General { dx: i32, dy: i32 },
    /// Horizontal line (`GeneralLineFlag` clear, `VertLineFlag` clear).
    Horizontal { dx: i32 },
    /// Vertical line (`GeneralLineFlag` clear, `VertLineFlag` set).
    Vertical { dy: i32 },
}

/// A fresh fill/line style set introduced by a STYLECHANGERECORD's StateNewStyles
/// (Shape2/3/4 only). Reading byte-aligns before the new arrays and the trailing
/// `(NumFillBits:4, NumLineBits:4)` reset the bit widths for the records that
/// follow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewStyles {
    pub fill_styles: FillStyleArray,
    pub line_styles: LineStyleArray,
    /// New `NumFillBits` for subsequent fill-index reads.
    pub num_fill_bits: u32,
    /// New `NumLineBits` for subsequent line-index reads.
    pub num_line_bits: u32,
}

/// One `SHAPERECORD`. The shape stream is terminated by [`ShapeRecord::End`]
/// (the all-zero non-edge record). Edge records store the source `NumBits` field
/// verbatim (actual delta width is `num_bits + 2`); it is non-minimal in 1,133
/// corpus edges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShapeRecord {
    /// ENDSHAPERECORD: `TypeFlag=0` plus five zero state bits.
    End,
    /// STYLECHANGERECORD. The 5-bit `flags`
    /// (`StateNewStyles`/`StateLineStyle`/`StateFillStyle1`/`StateFillStyle0`/
    /// `StateMoveTo`, MSB-to-LSB) are the source of truth; each optional field is
    /// `Some` iff its state bit is set.
    StyleChange {
        flags: u8,
        move_to: Option<MoveTo>,
        /// `FillStyle0` index (`StateFillStyle0`), `NumFillBits` wide.
        fill_style0: Option<u32>,
        /// `FillStyle1` index (`StateFillStyle1`), `NumFillBits` wide.
        fill_style1: Option<u32>,
        /// `LineStyle` index (`StateLineStyle`), `NumLineBits` wide.
        line_style: Option<u32>,
        /// New style arrays (`StateNewStyles`, Shape2/3/4 only).
        new_styles: Option<NewStyles>,
    },
    /// STRAIGHTEDGERECORD. `num_bits` is the source field (delta width `+2`).
    StraightEdge { num_bits: u32, edge: StraightEdge },
    /// CURVEDEDGERECORD. `num_bits` is the source field (delta width `+2`).
    CurvedEdge {
        num_bits: u32,
        control_dx: i32,
        control_dy: i32,
        anchor_dx: i32,
        anchor_dy: i32,
    },
}

/// A `SHAPEWITHSTYLE`: the initial fill/line style arrays, the starting
/// `(NumFillBits:4, NumLineBits:4)`, and the SHAPERECORD stream (including its
/// terminating [`ShapeRecord::End`]). The initial `num_fill_bits`/
/// `num_line_bits` are stored verbatim, never recomputed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShapeWithStyle {
    pub fill_styles: FillStyleArray,
    pub line_styles: LineStyleArray,
    pub num_fill_bits: u32,
    pub num_line_bits: u32,
    pub records: Vec<ShapeRecord>,
}

impl ShapeWithStyle {
    const CTX: &'static str = "SHAPEWITHSTYLE";

    fn read(br: &mut BitReader, version: u8) -> Result<ShapeWithStyle, GfxError> {
        let rgba = version >= 3;
        let fill_styles = FillStyleArray::read(br, version, rgba)?;
        let line_styles = LineStyleArray::read(br, version, rgba)?;
        let num_fill_bits = br.read_ubits(4, Self::CTX)?;
        let num_line_bits = br.read_ubits(4, Self::CTX)?;
        let records = read_shape_records(br, version, rgba, num_fill_bits, num_line_bits)?;
        Ok(ShapeWithStyle {
            fill_styles,
            line_styles,
            num_fill_bits,
            num_line_bits,
            records,
        })
    }

    fn write(&self, bw: &mut BitWriter) {
        self.fill_styles.write(bw);
        self.line_styles.write(bw);
        bw.write_ubits(self.num_fill_bits, 4);
        bw.write_ubits(self.num_line_bits, 4);
        write_shape_records(bw, &self.records, self.num_fill_bits, self.num_line_bits);
    }
}

/// Read the SHAPERECORD stream up to and including the ENDSHAPERECORD. The fill/
/// line bit widths shadow the SHAPEWITHSTYLE defaults and are reset by any
/// StateNewStyles record.
fn read_shape_records(
    br: &mut BitReader,
    version: u8,
    rgba: bool,
    mut num_fill_bits: u32,
    mut num_line_bits: u32,
) -> Result<Vec<ShapeRecord>, GfxError> {
    const CTX: &str = "SHAPERECORD";
    let mut records = Vec::new();
    loop {
        let type_flag = br.read_ubits(1, CTX)?;
        if type_flag == 0 {
            let flags = br.read_ubits(5, CTX)? as u8;
            if flags == 0 {
                records.push(ShapeRecord::End);
                break;
            }
            let new_styles_flag = flags & 0x10 != 0;
            let state_line = flags & 0x08 != 0;
            let state_fill1 = flags & 0x04 != 0;
            let state_fill0 = flags & 0x02 != 0;
            let state_move = flags & 0x01 != 0;

            let move_to = if state_move {
                let mb = br.read_ubits(5, CTX)?;
                let dx = br.read_sbits(mb, CTX)?;
                let dy = br.read_sbits(mb, CTX)?;
                Some(MoveTo {
                    num_bits: mb,
                    dx,
                    dy,
                })
            } else {
                None
            };
            let fill_style0 = if state_fill0 {
                Some(br.read_ubits(num_fill_bits, CTX)?)
            } else {
                None
            };
            let fill_style1 = if state_fill1 {
                Some(br.read_ubits(num_fill_bits, CTX)?)
            } else {
                None
            };
            let line_style = if state_line {
                Some(br.read_ubits(num_line_bits, CTX)?)
            } else {
                None
            };
            let new_styles = if new_styles_flag {
                if version < 2 {
                    return Err(GfxError::ShapeNewStylesUnsupported);
                }
                // StateNewStyles byte-aligns before the new (byte-structured)
                // style arrays (corpus-proven over 24 records).
                br.byte_align(CTX)?;
                let fill_styles = FillStyleArray::read(br, version, rgba)?;
                let line_styles = LineStyleArray::read(br, version, rgba)?;
                let nf = br.read_ubits(4, CTX)?;
                let nl = br.read_ubits(4, CTX)?;
                num_fill_bits = nf;
                num_line_bits = nl;
                Some(NewStyles {
                    fill_styles,
                    line_styles,
                    num_fill_bits: nf,
                    num_line_bits: nl,
                })
            } else {
                None
            };
            records.push(ShapeRecord::StyleChange {
                flags,
                move_to,
                fill_style0,
                fill_style1,
                line_style,
                new_styles,
            });
        } else {
            let straight = br.read_ubits(1, CTX)? != 0;
            let num_bits = br.read_ubits(4, CTX)?;
            let bits = num_bits + 2;
            if straight {
                let general = br.read_ubits(1, CTX)? != 0;
                let edge = if general {
                    StraightEdge::General {
                        dx: br.read_sbits(bits, CTX)?,
                        dy: br.read_sbits(bits, CTX)?,
                    }
                } else {
                    let vert = br.read_ubits(1, CTX)? != 0;
                    if vert {
                        StraightEdge::Vertical {
                            dy: br.read_sbits(bits, CTX)?,
                        }
                    } else {
                        StraightEdge::Horizontal {
                            dx: br.read_sbits(bits, CTX)?,
                        }
                    }
                };
                records.push(ShapeRecord::StraightEdge { num_bits, edge });
            } else {
                records.push(ShapeRecord::CurvedEdge {
                    num_bits,
                    control_dx: br.read_sbits(bits, CTX)?,
                    control_dy: br.read_sbits(bits, CTX)?,
                    anchor_dx: br.read_sbits(bits, CTX)?,
                    anchor_dy: br.read_sbits(bits, CTX)?,
                });
            }
        }
    }
    Ok(records)
}

/// Write the SHAPERECORD stream. The fill/line bit widths shadow the
/// SHAPEWITHSTYLE defaults and are reset by any StateNewStyles record, exactly
/// as on read.
fn write_shape_records(
    bw: &mut BitWriter,
    records: &[ShapeRecord],
    mut num_fill_bits: u32,
    mut num_line_bits: u32,
) {
    for rec in records {
        match rec {
            ShapeRecord::End => {
                bw.write_ubits(0, 1); // TypeFlag = 0
                bw.write_ubits(0, 5); // all state bits clear
            }
            ShapeRecord::StyleChange {
                flags,
                move_to,
                fill_style0,
                fill_style1,
                line_style,
                new_styles,
            } => {
                bw.write_ubits(0, 1); // TypeFlag = 0
                bw.write_ubits(*flags as u32, 5);
                if let Some(m) = move_to {
                    bw.write_ubits(m.num_bits, 5);
                    bw.write_sbits(m.dx, m.num_bits);
                    bw.write_sbits(m.dy, m.num_bits);
                }
                if let Some(f0) = fill_style0 {
                    bw.write_ubits(*f0, num_fill_bits);
                }
                if let Some(f1) = fill_style1 {
                    bw.write_ubits(*f1, num_fill_bits);
                }
                if let Some(l) = line_style {
                    bw.write_ubits(*l, num_line_bits);
                }
                if let Some(ns) = new_styles {
                    bw.byte_align();
                    ns.fill_styles.write(bw);
                    ns.line_styles.write(bw);
                    bw.write_ubits(ns.num_fill_bits, 4);
                    bw.write_ubits(ns.num_line_bits, 4);
                    num_fill_bits = ns.num_fill_bits;
                    num_line_bits = ns.num_line_bits;
                }
            }
            ShapeRecord::StraightEdge { num_bits, edge } => {
                bw.write_ubits(1, 1); // TypeFlag = 1
                bw.write_ubits(1, 1); // StraightFlag = 1
                bw.write_ubits(*num_bits, 4);
                let bits = num_bits + 2;
                match edge {
                    StraightEdge::General { dx, dy } => {
                        bw.write_ubits(1, 1); // GeneralLineFlag = 1
                        bw.write_sbits(*dx, bits);
                        bw.write_sbits(*dy, bits);
                    }
                    StraightEdge::Vertical { dy } => {
                        bw.write_ubits(0, 1); // GeneralLineFlag = 0
                        bw.write_ubits(1, 1); // VertLineFlag = 1
                        bw.write_sbits(*dy, bits);
                    }
                    StraightEdge::Horizontal { dx } => {
                        bw.write_ubits(0, 1); // GeneralLineFlag = 0
                        bw.write_ubits(0, 1); // VertLineFlag = 0
                        bw.write_sbits(*dx, bits);
                    }
                }
            }
            ShapeRecord::CurvedEdge {
                num_bits,
                control_dx,
                control_dy,
                anchor_dx,
                anchor_dy,
            } => {
                bw.write_ubits(1, 1); // TypeFlag = 1
                bw.write_ubits(0, 1); // StraightFlag = 0
                bw.write_ubits(*num_bits, 4);
                let bits = num_bits + 2;
                bw.write_sbits(*control_dx, bits);
                bw.write_sbits(*control_dy, bits);
                bw.write_sbits(*anchor_dx, bits);
                bw.write_sbits(*anchor_dy, bits);
            }
        }
    }
}

/// Map a shape `version` (1..=4) to its tag code.
fn shape_version_to_code(version: u8) -> u16 {
    match version {
        1 => TAG_DEFINE_SHAPE,
        2 => TAG_DEFINE_SHAPE2,
        3 => TAG_DEFINE_SHAPE3,
        _ => TAG_DEFINE_SHAPE4,
    }
}

/// Parse a `DefineShape*` body into its typed parts (the bitstream model). Used
/// by [`decode_define_shape`], which re-serializes and verifies the result.
struct DefineShapeParts {
    shape_id: u16,
    shape_bounds: Rect,
    edge_bounds: Option<Rect>,
    flags_byte: Option<u8>,
    shapes: ShapeWithStyle,
}

fn parse_define_shape(body: &[u8], version: u8) -> Result<DefineShapeParts, GfxError> {
    const CTX: &str = "DefineShape";
    let mut br = BitReader::new_at_byte(body, 0);
    let shape_id = br.read_u16_aligned(CTX)?;
    let shape_bounds = Rect::read(&mut br)?;
    let (edge_bounds, flags_byte) = if version == 4 {
        let eb = Rect::read(&mut br)?;
        let fb = br.read_u8_aligned(CTX)?;
        (Some(eb), Some(fb))
    } else {
        (None, None)
    };
    let shapes = ShapeWithStyle::read(&mut br, version)?;
    br.byte_align(CTX)?;
    // Trailing bytes after the byte-aligned shape end are a structural surprise;
    // the decode-then-verify byte comparison would also catch it, but fail here
    // so the caller falls back without re-serializing.
    if br.byte_pos() != body.len() {
        return Err(GfxError::TrailingTagBytes {
            code: shape_version_to_code(version),
            remaining: body.len() - br.byte_pos(),
        });
    }
    Ok(DefineShapeParts {
        shape_id,
        shape_bounds,
        edge_bounds,
        flags_byte,
        shapes,
    })
}

/// Serialize a `DefineShape*` body from its typed parts.
fn serialize_shape_body(
    version: u8,
    shape_id: u16,
    shape_bounds: &Rect,
    edge_bounds: Option<&Rect>,
    flags_byte: Option<u8>,
    shapes: &ShapeWithStyle,
) -> Vec<u8> {
    let mut bw = BitWriter::new();
    bw.write_u16_aligned(shape_id);
    shape_bounds.write(&mut bw);
    if version == 4 {
        edge_bounds
            .expect("DefineShape4 without edge_bounds")
            .write(&mut bw);
        bw.write_u8_aligned(flags_byte.expect("DefineShape4 without flags_byte"));
    }
    shapes.write(&mut bw);
    bw.byte_align();
    bw.into_bytes()
}

/// Decode a `DefineShape*` (code 2/22/32/83) body. The body is decode-then-
/// verified: it is fully parsed, re-serialized, and compared against the source;
/// on any structural surprise or byte mismatch it falls back to [`Tag::Unknown`]
/// so byte-identity is never silently lost. Always returns `Ok` -- the fallback
/// is data, not an error.
fn decode_define_shape(code: u16, body: Vec<u8>, force_long: bool) -> Tag {
    let version = match code {
        TAG_DEFINE_SHAPE => 1u8,
        TAG_DEFINE_SHAPE2 => 2,
        TAG_DEFINE_SHAPE3 => 3,
        _ => 4,
    };
    match parse_define_shape(&body, version) {
        Ok(parts) => {
            let reencoded = serialize_shape_body(
                version,
                parts.shape_id,
                &parts.shape_bounds,
                parts.edge_bounds.as_ref(),
                parts.flags_byte,
                &parts.shapes,
            );
            if reencoded == body {
                Tag::DefineShape {
                    version,
                    shape_id: parts.shape_id,
                    shape_bounds: parts.shape_bounds,
                    edge_bounds: parts.edge_bounds,
                    flags_byte: parts.flags_byte,
                    shapes: parts.shapes,
                    force_long,
                }
            } else {
                Tag::Unknown {
                    code,
                    raw: body,
                    force_long,
                }
            }
        }
        Err(_) => Tag::Unknown {
            code,
            raw: body,
            force_long,
        },
    }
}

// ===========================================================================
// Tier-4: DefineEditText (37) + DefineFont3 (75) -- text/font tags that reuse
// the RECT primitive and the SHAPERECORD edge bitstream.
// ===========================================================================
//
// Both tags are decode-then-verified exactly like the DefineShape family: the
// body is parsed into typed fields, re-serialized, and byte-compared against the
// source; any structural surprise or byte mismatch falls the whole tag back to
// [`Tag::Unknown`] so byte-identity can never be silently lost. Across the
// 114-file corpus all 1,479 DefineEditText and all 7 DefineFont3 decode to their
// typed variants byte-cleanly (python ground-truth verifier).

/// The `DefineEditText` layout block (present iff `flags2` `HasLayout`).
/// `leading` is signed; the rest are unsigned twips/indices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditTextLayout {
    /// Paragraph alignment (0 left, 1 right, 2 center, 3 justify).
    pub align: u8,
    pub left_margin: u16,
    pub right_margin: u16,
    pub indent: u16,
    /// `Leading` (signed twips between lines).
    pub leading: i16,
}

/// One glyph `SHAPE` inside a [`Tag::DefineFont3`]. Unlike a `SHAPEWITHSTYLE` it
/// carries no fill/line style arrays -- just its own starting `NumFillBits`/
/// `NumLineBits` (stored verbatim) and the SHAPERECORD stream (terminated by its
/// [`ShapeRecord::End`]), reusing the Tier-3 edge machinery. A glyph SHAPE never
/// carries a StateNewStyles record (it has no style arrays); one would fall the
/// owning font back to [`Tag::Unknown`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphShape {
    pub num_fill_bits: u32,
    pub num_line_bits: u32,
    pub records: Vec<ShapeRecord>,
}

/// One `KERNINGRECORD` in a [`Font3Layout`] kerning table. The two character
/// codes are `u16` iff the font's `WideCodes` flag is set (else `u8`); the
/// adjustment is a signed twip delta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KerningRecord {
    pub code1: u16,
    pub code2: u16,
    pub adjustment: i16,
}

/// The `DefineFont3` layout block (present iff `flags` `HasLayout`). The advance
/// and bounds tables have one entry per glyph; the kerning table is `u16`-counted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Font3Layout {
    pub ascent: i16,
    pub descent: i16,
    pub leading: i16,
    /// Per-glyph advance widths (`numGlyphs` entries).
    pub advance: Vec<i16>,
    /// Per-glyph bounding boxes (`numGlyphs` `RECT`s, each byte-aligned).
    pub bounds: Vec<Rect>,
    /// Kerning pairs (`count` is `kernings.len()` on write).
    pub kernings: Vec<KerningRecord>,
}

/// Parsed `DefineEditText` fields (the intermediate of [`decode_define_edit_text`],
/// which re-serializes and verifies them).
struct EditTextParts {
    character_id: u16,
    bounds: Rect,
    flags1: u8,
    flags2: u8,
    font_id: Option<u16>,
    font_class: Option<String>,
    font_height: Option<u16>,
    text_color: Option<[u8; 4]>,
    max_length: Option<u16>,
    layout: Option<EditTextLayout>,
    variable_name: String,
    initial_text: Option<String>,
}

/// Parse a `DefineEditText` (code 37) body into its typed fields. The `bounds`
/// RECT is bit-packed and byte-aligns; everything after the two flag bytes is
/// byte-structured. The two flag bytes are the source of truth for optional-field
/// presence.
fn parse_define_edit_text(body: &[u8]) -> Result<EditTextParts, GfxError> {
    let code = TAG_DEFINE_EDIT_TEXT;
    let mut r = GfxReader::new(body);
    let character_id = r.read_u16()?;
    let mut bits = BitReader::new_at_byte(body, r.pos);
    let bounds = Rect::read(&mut bits)?;
    r.pos = bits.byte_pos();
    let flags1 = r.read_u8()?;
    let flags2 = r.read_u8()?;

    let has_font = flags1 & ET_HAS_FONT != 0;
    let has_font_class = flags2 & ET2_HAS_FONT_CLASS != 0;
    let font_id = if has_font { Some(r.read_u16()?) } else { None };
    let font_class = if has_font_class {
        Some(r.read_cstring(code)?)
    } else {
        None
    };
    let font_height = if has_font || has_font_class {
        Some(r.read_u16()?)
    } else {
        None
    };
    let text_color = if flags1 & ET_HAS_TEXT_COLOR != 0 {
        Some([r.read_u8()?, r.read_u8()?, r.read_u8()?, r.read_u8()?])
    } else {
        None
    };
    let max_length = if flags1 & ET_HAS_MAX_LENGTH != 0 {
        Some(r.read_u16()?)
    } else {
        None
    };
    let layout = if flags2 & ET2_HAS_LAYOUT != 0 {
        Some(EditTextLayout {
            align: r.read_u8()?,
            left_margin: r.read_u16()?,
            right_margin: r.read_u16()?,
            indent: r.read_u16()?,
            leading: r.read_u16()? as i16,
        })
    } else {
        None
    };
    let variable_name = r.read_cstring(code)?;
    let initial_text = if flags1 & ET_HAS_TEXT != 0 {
        Some(r.read_cstring(code)?)
    } else {
        None
    };
    ensure_consumed(code, r.pos, body.len())?;
    Ok(EditTextParts {
        character_id,
        bounds,
        flags1,
        flags2,
        font_id,
        font_class,
        font_height,
        text_color,
        max_length,
        layout,
        variable_name,
        initial_text,
    })
}

/// Serialize a `DefineEditText` body from typed fields (used by both the
/// decode-then-verify check and the writer). Each optional field is emitted iff
/// its flag bit is set; the flag bytes are the source of truth.
#[allow(clippy::too_many_arguments)]
fn serialize_edit_text_body(
    character_id: u16,
    bounds: &Rect,
    flags1: u8,
    flags2: u8,
    font_id: Option<u16>,
    font_class: Option<&str>,
    font_height: Option<u16>,
    text_color: Option<&[u8; 4]>,
    max_length: Option<u16>,
    layout: Option<&EditTextLayout>,
    variable_name: &str,
    initial_text: Option<&str>,
) -> Vec<u8> {
    let mut w = GfxWriter::new();
    w.write_u16(character_id);
    let mut bw = BitWriter::new();
    bounds.write(&mut bw);
    w.write_bytes(&bw.into_bytes());
    w.write_u8(flags1);
    w.write_u8(flags2);
    if flags1 & ET_HAS_FONT != 0 {
        w.write_u16(font_id.expect("HasFont set without font_id"));
    }
    if flags2 & ET2_HAS_FONT_CLASS != 0 {
        w.write_cstring(font_class.expect("HasFontClass set without font_class"));
    }
    if flags1 & ET_HAS_FONT != 0 || flags2 & ET2_HAS_FONT_CLASS != 0 {
        w.write_u16(font_height.expect("font present without font_height"));
    }
    if flags1 & ET_HAS_TEXT_COLOR != 0 {
        w.write_bytes(text_color.expect("HasTextColor set without text_color"));
    }
    if flags1 & ET_HAS_MAX_LENGTH != 0 {
        w.write_u16(max_length.expect("HasMaxLength set without max_length"));
    }
    if flags2 & ET2_HAS_LAYOUT != 0 {
        let l = layout.expect("HasLayout set without layout");
        w.write_u8(l.align);
        w.write_u16(l.left_margin);
        w.write_u16(l.right_margin);
        w.write_u16(l.indent);
        w.write_u16(l.leading as u16);
    }
    w.write_cstring(variable_name);
    if flags1 & ET_HAS_TEXT != 0 {
        w.write_cstring(initial_text.expect("HasText set without initial_text"));
    }
    w.buf
}

/// Decode a `DefineEditText` (code 37) body, decode-then-verified: parse, then
/// re-serialize and byte-compare; on any mismatch or structural surprise fall
/// back to [`Tag::Unknown`] so byte-identity is never silently lost. Always
/// returns `Ok` (the fallback is data, not an error).
fn decode_define_edit_text(body: Vec<u8>, force_long: bool) -> Tag {
    match parse_define_edit_text(&body) {
        Ok(p) => {
            let reencoded = serialize_edit_text_body(
                p.character_id,
                &p.bounds,
                p.flags1,
                p.flags2,
                p.font_id,
                p.font_class.as_deref(),
                p.font_height,
                p.text_color.as_ref(),
                p.max_length,
                p.layout.as_ref(),
                &p.variable_name,
                p.initial_text.as_deref(),
            );
            if reencoded == body {
                Tag::DefineEditText {
                    character_id: p.character_id,
                    bounds: p.bounds,
                    flags1: p.flags1,
                    flags2: p.flags2,
                    font_id: p.font_id,
                    font_class: p.font_class,
                    font_height: p.font_height,
                    text_color: p.text_color,
                    max_length: p.max_length,
                    layout: p.layout,
                    variable_name: p.variable_name,
                    initial_text: p.initial_text,
                    force_long,
                }
            } else {
                Tag::Unknown {
                    code: TAG_DEFINE_EDIT_TEXT,
                    raw: body,
                    force_long,
                }
            }
        }
        Err(_) => Tag::Unknown {
            code: TAG_DEFINE_EDIT_TEXT,
            raw: body,
            force_long,
        },
    }
}

/// Read one glyph `SHAPE` (a `SHAPE`, not `SHAPEWITHSTYLE`): a `NumFillBits`/
/// `NumLineBits` header then the SHAPERECORD stream, byte-aligning at its end (the
/// font offset table packs glyphs on byte boundaries). Passes `version = 1` so a
/// StateNewStyles record (invalid in a styleless glyph SHAPE) errors out and
/// falls the font back to [`Tag::Unknown`]; `rgba` is irrelevant without styles.
fn read_glyph_shape(br: &mut BitReader) -> Result<GlyphShape, GfxError> {
    const CTX: &str = "GLYPH";
    let num_fill_bits = br.read_ubits(4, CTX)?;
    let num_line_bits = br.read_ubits(4, CTX)?;
    let records = read_shape_records(br, 1, false, num_fill_bits, num_line_bits)?;
    br.byte_align(CTX)?;
    Ok(GlyphShape {
        num_fill_bits,
        num_line_bits,
        records,
    })
}

/// Write one glyph `SHAPE`, mirroring [`read_glyph_shape`].
fn write_glyph_shape(bw: &mut BitWriter, g: &GlyphShape) {
    bw.write_ubits(g.num_fill_bits, 4);
    bw.write_ubits(g.num_line_bits, 4);
    write_shape_records(bw, &g.records, g.num_fill_bits, g.num_line_bits);
    bw.byte_align();
}

/// Parsed `DefineFont3` fields (the intermediate of [`decode_define_font3`]).
struct Font3Parts {
    font_id: u16,
    flags: u8,
    language_code: u8,
    font_name: Vec<u8>,
    offsets: Vec<u32>,
    glyphs: Vec<GlyphShape>,
    codes: Vec<u16>,
    layout: Option<Font3Layout>,
}

/// Parse a `DefineFont3` (code 75) body into its typed fields. The offset table
/// values are read verbatim (never recomputed); each glyph `SHAPE` reuses the
/// edge bitstream and byte-aligns. `WideOffsets`/`WideCodes`/`HasLayout` come from
/// the `flags` byte.
fn parse_define_font3(body: &[u8]) -> Result<Font3Parts, GfxError> {
    let code = TAG_DEFINE_FONT3;
    let mut r = GfxReader::new(body);
    let font_id = r.read_u16()?;
    let flags = r.read_u8()?;
    let language_code = r.read_u8()?;
    let name_len = r.read_u8()? as usize;
    let font_name = r.read_bytes(name_len)?;
    let num_glyphs = r.read_u16()? as usize;
    let wide_offsets = flags & F3_WIDE_OFFSETS != 0;
    let wide_codes = flags & F3_WIDE_CODES != 0;

    // OffsetTable: numGlyphs glyph offsets + 1 CodeTableOffset (same width).
    let mut offsets = Vec::with_capacity(num_glyphs + 1);
    for _ in 0..num_glyphs + 1 {
        offsets.push(if wide_offsets {
            r.read_u32()?
        } else {
            r.read_u16()? as u32
        });
    }
    // GlyphShapeTable: numGlyphs SHAPEs (byte-aligned via the offset table).
    let mut glyphs = Vec::with_capacity(num_glyphs);
    for _ in 0..num_glyphs {
        let mut bits = BitReader::new_at_byte(body, r.pos);
        let g = read_glyph_shape(&mut bits)?;
        r.pos = bits.byte_pos();
        glyphs.push(g);
    }
    // CodeTable: numGlyphs codes.
    let mut codes = Vec::with_capacity(num_glyphs);
    for _ in 0..num_glyphs {
        codes.push(if wide_codes {
            r.read_u16()?
        } else {
            r.read_u8()? as u16
        });
    }

    let layout = if flags & F3_HAS_LAYOUT != 0 {
        let ascent = r.read_u16()? as i16;
        let descent = r.read_u16()? as i16;
        let leading = r.read_u16()? as i16;
        let mut advance = Vec::with_capacity(num_glyphs);
        for _ in 0..num_glyphs {
            advance.push(r.read_u16()? as i16);
        }
        let mut bounds = Vec::with_capacity(num_glyphs);
        for _ in 0..num_glyphs {
            let mut bits = BitReader::new_at_byte(body, r.pos);
            let rect = Rect::read(&mut bits)?;
            r.pos = bits.byte_pos();
            bounds.push(rect);
        }
        let kerning_count = r.read_u16()? as usize;
        let mut kernings = Vec::with_capacity(kerning_count);
        for _ in 0..kerning_count {
            let code1 = if wide_codes {
                r.read_u16()?
            } else {
                r.read_u8()? as u16
            };
            let code2 = if wide_codes {
                r.read_u16()?
            } else {
                r.read_u8()? as u16
            };
            let adjustment = r.read_u16()? as i16;
            kernings.push(KerningRecord {
                code1,
                code2,
                adjustment,
            });
        }
        Some(Font3Layout {
            ascent,
            descent,
            leading,
            advance,
            bounds,
            kernings,
        })
    } else {
        None
    };

    ensure_consumed(code, r.pos, body.len())?;
    Ok(Font3Parts {
        font_id,
        flags,
        language_code,
        font_name,
        offsets,
        glyphs,
        codes,
        layout,
    })
}

/// Serialize a `DefineFont3` body from typed fields (used by both the
/// decode-then-verify check and the writer). `numGlyphs` is derived from
/// `glyphs.len()`; the offset table is emitted verbatim. Widths come from `flags`.
#[allow(clippy::too_many_arguments)]
fn serialize_font3_body(
    font_id: u16,
    flags: u8,
    language_code: u8,
    font_name: &[u8],
    offsets: &[u32],
    glyphs: &[GlyphShape],
    codes: &[u16],
    layout: Option<&Font3Layout>,
) -> Vec<u8> {
    let wide_offsets = flags & F3_WIDE_OFFSETS != 0;
    let wide_codes = flags & F3_WIDE_CODES != 0;
    let mut w = GfxWriter::new();
    w.write_u16(font_id);
    w.write_u8(flags);
    w.write_u8(language_code);
    w.write_u8(font_name.len() as u8);
    w.write_bytes(font_name);
    w.write_u16(glyphs.len() as u16);
    for &off in offsets {
        if wide_offsets {
            w.write_u32(off);
        } else {
            w.write_u16(off as u16);
        }
    }
    for g in glyphs {
        let mut bw = BitWriter::new();
        write_glyph_shape(&mut bw, g);
        w.write_bytes(&bw.into_bytes());
    }
    for &c in codes {
        if wide_codes {
            w.write_u16(c);
        } else {
            w.write_u8(c as u8);
        }
    }
    if let Some(l) = layout {
        w.write_u16(l.ascent as u16);
        w.write_u16(l.descent as u16);
        w.write_u16(l.leading as u16);
        for &a in &l.advance {
            w.write_u16(a as u16);
        }
        for rect in &l.bounds {
            let mut bw = BitWriter::new();
            rect.write(&mut bw);
            w.write_bytes(&bw.into_bytes());
        }
        w.write_u16(l.kernings.len() as u16);
        for k in &l.kernings {
            if wide_codes {
                w.write_u16(k.code1);
                w.write_u16(k.code2);
            } else {
                w.write_u8(k.code1 as u8);
                w.write_u8(k.code2 as u8);
            }
            w.write_u16(k.adjustment as u16);
        }
    }
    w.buf
}

/// Decode a `DefineFont3` (code 75) body, decode-then-verified like the other
/// Tier-3/4 typed tags. Always returns `Ok`.
fn decode_define_font3(body: Vec<u8>, force_long: bool) -> Tag {
    match parse_define_font3(&body) {
        Ok(p) => {
            let reencoded = serialize_font3_body(
                p.font_id,
                p.flags,
                p.language_code,
                &p.font_name,
                &p.offsets,
                &p.glyphs,
                &p.codes,
                p.layout.as_ref(),
            );
            if reencoded == body {
                Tag::DefineFont3 {
                    font_id: p.font_id,
                    flags: p.flags,
                    language_code: p.language_code,
                    font_name: p.font_name,
                    offsets: p.offsets,
                    glyphs: p.glyphs,
                    codes: p.codes,
                    layout: p.layout,
                    force_long,
                }
            } else {
                Tag::Unknown {
                    code: TAG_DEFINE_FONT3,
                    raw: body,
                    force_long,
                }
            }
        }
        Err(_) => Tag::Unknown {
            code: TAG_DEFINE_FONT3,
            raw: body,
            force_long,
        },
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
        TAG_DEFINE_SHAPE | TAG_DEFINE_SHAPE2 | TAG_DEFINE_SHAPE3 | TAG_DEFINE_SHAPE4 => {
            Ok(decode_define_shape(code, body, force_long))
        }
        TAG_DEFINE_EDIT_TEXT => Ok(decode_define_edit_text(body, force_long)),
        TAG_DEFINE_FONT3 => Ok(decode_define_font3(body, force_long)),
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
        Tag::DefineShape {
            version,
            shape_id,
            shape_bounds,
            edge_bounds,
            flags_byte,
            shapes,
            force_long,
        } => {
            let body = serialize_shape_body(
                *version,
                *shape_id,
                shape_bounds,
                edge_bounds.as_ref(),
                *flags_byte,
                shapes,
            );
            w.write_record_header(shape_version_to_code(*version), body.len(), *force_long)?;
            w.write_bytes(&body);
        }
        Tag::DefineEditText {
            character_id,
            bounds,
            flags1,
            flags2,
            font_id,
            font_class,
            font_height,
            text_color,
            max_length,
            layout,
            variable_name,
            initial_text,
            force_long,
        } => {
            let body = serialize_edit_text_body(
                *character_id,
                bounds,
                *flags1,
                *flags2,
                *font_id,
                font_class.as_deref(),
                *font_height,
                text_color.as_ref(),
                *max_length,
                layout.as_ref(),
                variable_name,
                initial_text.as_deref(),
            );
            w.write_record_header(TAG_DEFINE_EDIT_TEXT, body.len(), *force_long)?;
            w.write_bytes(&body);
        }
        Tag::DefineFont3 {
            font_id,
            flags,
            language_code,
            font_name,
            offsets,
            glyphs,
            codes,
            layout,
            force_long,
        } => {
            let body = serialize_font3_body(
                *font_id,
                *flags,
                *language_code,
                font_name,
                offsets,
                glyphs,
                codes,
                layout.as_ref(),
            );
            w.write_record_header(TAG_DEFINE_FONT3, body.len(), *force_long)?;
            w.write_bytes(&body);
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

    /// Every `DefineShape*` across the whole corpus must decode to the typed
    /// [`Tag::DefineShape`] variant -- NOT silently fall back to [`Tag::Unknown`].
    /// The `tests/roundtrip.rs` byte-identity gate passes either way (an opaque
    /// body also round-trips), so this gate separately proves the Tier-3 typed
    /// shape codec actually handles all 366 corpus shapes byte-cleanly rather
    /// than punting them to the opaque path. Skips when assets are absent.
    #[test]
    fn corpus_define_shapes_all_typed() {
        // Total DefineShape* characters across the 114-file corpus (276 Shape1 +
        // 8 Shape2 + 69 Shape3 + 13 Shape4), proven by the python ground-truth
        // verifier before this codec was written.
        const EXPECTED_TYPED_SHAPES: usize = 366;
        let root = "/home/banon/er-extract/nuxe-menu-20260619-170932/menu";
        if !std::path::Path::new(root).exists() {
            eprintln!("SKIP: corpus root {root} not present; shape-typing test skipped");
            return;
        }

        fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    collect(&p, out);
                } else if p.extension().and_then(|s| s.to_str()) == Some("gfx") {
                    out.push(p);
                }
            }
        }

        fn count(tags: &[Tag], typed: &mut usize, fell_back: &mut usize) {
            for t in tags {
                match t {
                    Tag::DefineShape { .. } => *typed += 1,
                    Tag::Unknown { code, .. }
                        if matches!(
                            *code,
                            TAG_DEFINE_SHAPE
                                | TAG_DEFINE_SHAPE2
                                | TAG_DEFINE_SHAPE3
                                | TAG_DEFINE_SHAPE4
                        ) =>
                    {
                        *fell_back += 1
                    }
                    Tag::DefineSprite { tags, .. } => count(tags, typed, fell_back),
                    _ => {}
                }
            }
        }

        let mut files = Vec::new();
        collect(std::path::Path::new(root), &mut files);
        files.sort();

        let mut typed = 0usize;
        let mut fell_back = 0usize;
        for path in &files {
            let bytes = std::fs::read(path).expect("read corpus file");
            let movie = Movie::parse(&bytes).expect("parse corpus file");
            count(&movie.tags, &mut typed, &mut fell_back);
        }

        assert_eq!(
            fell_back, 0,
            "{fell_back} DefineShape* tag(s) fell back to Tag::Unknown instead of typing"
        );
        assert_eq!(
            typed, EXPECTED_TYPED_SHAPES,
            "expected {EXPECTED_TYPED_SHAPES} typed DefineShape* tags, found {typed}"
        );
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

    // --- Tier-3: DefineShape family + SHAPEWITHSTYLE tests --------------------

    #[test]
    fn shape_solid_fill_rectangle_instance() {
        // Corpus 01_000_fe.gfx DefineShape (tag 2, version 1): a 30x210 solid
        // red rectangle. fill[0] = solid RGB 0x5b0000; records: a StyleChange
        // (moveTo + fillStyle1) then four straight edges + End. Field values are
        // verifier-confirmed against the raw bitstream.
        let body = hx("49014008fc932001005b000000101503f25dd696845bb2ed0780");
        match parse_first(&rec(TAG_DEFINE_SHAPE, &body, false)) {
            Tag::DefineShape {
                version,
                shape_id,
                shape_bounds,
                edge_bounds,
                flags_byte,
                shapes,
                ..
            } => {
                assert_eq!(version, 1);
                assert_eq!(shape_id, 329);
                assert_eq!(edge_bounds, None);
                assert_eq!(flags_byte, None);
                assert_eq!(shape_bounds.nbits, 8);
                assert_eq!(
                    (
                        shape_bounds.x_min,
                        shape_bounds.x_max,
                        shape_bounds.y_min,
                        shape_bounds.y_max
                    ),
                    (1, 31, -110, 100)
                );
                assert!(shapes.line_styles.styles.is_empty());
                assert_eq!(shapes.fill_styles.styles.len(), 1);
                match &shapes.fill_styles.styles[0] {
                    FillStyle::Solid(Color::Rgb(c)) => assert_eq!(*c, [0x5b, 0x00, 0x00]),
                    other => panic!("expected solid RGB fill, got {other:?}"),
                }
                // First record: StyleChange with moveTo (8 bits) to (31,-110)
                // selecting fillStyle1 = 1.
                match &shapes.records[0] {
                    ShapeRecord::StyleChange {
                        move_to,
                        fill_style1,
                        ..
                    } => {
                        let m = move_to.as_ref().expect("a moveTo");
                        assert_eq!(m.num_bits, 8);
                        assert_eq!((m.dx, m.dy), (31, -110));
                        assert_eq!(*fill_style1, Some(1));
                    }
                    other => panic!("expected StyleChange, got {other:?}"),
                }
                // Second record: a vertical straight edge dy=210, stored 7-bit
                // NumBits field (delta width 9).
                match &shapes.records[1] {
                    ShapeRecord::StraightEdge {
                        num_bits,
                        edge: StraightEdge::Vertical { dy },
                    } => {
                        assert_eq!(*num_bits, 7);
                        assert_eq!(*dy, 210);
                    }
                    other => panic!("expected vertical edge, got {other:?}"),
                }
                // Third record: a horizontal straight edge dx=-30.
                match &shapes.records[2] {
                    ShapeRecord::StraightEdge {
                        num_bits,
                        edge: StraightEdge::Horizontal { dx },
                    } => {
                        assert_eq!(*num_bits, 4);
                        assert_eq!(*dx, -30);
                    }
                    other => panic!("expected horizontal edge, got {other:?}"),
                }
                assert!(matches!(shapes.records.last(), Some(ShapeRecord::End)));
            }
            other => panic!("expected DefineShape, got {other:?}"),
        }
    }

    #[test]
    fn shape_bitmap_fill_instance() {
        // Corpus 02_020_inventory.gfx DefineShape (tag 2): a 520x520
        // bitmap-filled square. fill[0] = bitmap type 0x40, bitmapId 6, with a
        // scale-only MATRIX (NScaleBits=20, scale 266252).
        let body = hx("bf005800410001040001400600d104031040300000101568200079504725f8e5bf1c882000");
        match parse_first(&rec(TAG_DEFINE_SHAPE, &body, false)) {
            Tag::DefineShape {
                shape_id, shapes, ..
            } => {
                assert_eq!(shape_id, 191);
                match &shapes.fill_styles.styles[0] {
                    FillStyle::Bitmap {
                        fill_type,
                        bitmap_id,
                        matrix,
                    } => {
                        assert_eq!(*fill_type, 0x40);
                        assert_eq!(*bitmap_id, 6);
                        assert!(matrix.has_scale);
                        assert_eq!(matrix.scale_nbits, 20);
                        assert_eq!(matrix.scale_x, 266252);
                        assert_eq!(matrix.scale_y, 266252);
                        assert_eq!((matrix.translate_x, matrix.translate_y), (0, 0));
                    }
                    other => panic!("expected bitmap fill, got {other:?}"),
                }
            }
            other => panic!("expected DefineShape, got {other:?}"),
        }
    }

    #[test]
    fn shape_gradient_fill_instance() {
        // Corpus 05_000_title.gfx DefineShape3 (tag 32): the title-screen
        // darkening gradient -- a linear gradient (fill type 0x10) fading
        // transparent black (ratio 0, RGBA 00000000) to opaque black (ratio 255,
        // RGBA 000000ff). RGBA colors confirm the version-3 4-byte color path.
        let body = hx(
            "1000758f09c478d00000011084c3e3413881400468020000000000ff000000ff001015c9c478d1e5731e963c396347a2710000",
        );
        match parse_first(&rec(TAG_DEFINE_SHAPE3, &body, false)) {
            Tag::DefineShape {
                version,
                shape_id,
                shapes,
                ..
            } => {
                assert_eq!(version, 3);
                assert_eq!(shape_id, 16);
                match &shapes.fill_styles.styles[0] {
                    FillStyle::Gradient {
                        fill_type,
                        gradient,
                        matrix,
                    } => {
                        assert_eq!(*fill_type, 0x10); // linear
                        assert_eq!(gradient.spread_mode, 0);
                        assert_eq!(gradient.interpolation_mode, 0);
                        assert_eq!(gradient.focal_point, None);
                        assert_eq!(gradient.records.len(), 2);
                        assert_eq!(gradient.records[0].ratio, 0);
                        assert_eq!(gradient.records[0].color, Color::Rgba([0, 0, 0, 0]));
                        assert_eq!(gradient.records[1].ratio, 255);
                        assert_eq!(gradient.records[1].color, Color::Rgba([0, 0, 0, 0xff]));
                        assert!(matrix.has_scale);
                    }
                    other => panic!("expected gradient fill, got {other:?}"),
                }
            }
            other => panic!("expected DefineShape, got {other:?}"),
        }
    }

    #[test]
    fn shape_records_preserve_non_minimal_edge_nbits() {
        // A straight edge whose stored NumBits field (10 -> delta width 12) is
        // wider than the minimal width for dx=3 (which needs 3 bits). Byte
        // identity REQUIRES preserving the source width, not recomputing a
        // minimal one -- the corpus has 1,133 such non-minimal edges.
        let recs = vec![
            ShapeRecord::StraightEdge {
                num_bits: 10,
                edge: StraightEdge::Horizontal { dx: 3 },
            },
            ShapeRecord::End,
        ];
        let mut bw = BitWriter::new();
        write_shape_records(&mut bw, &recs, 0, 0);
        bw.byte_align();
        let bytes = bw.into_bytes();
        let mut br = BitReader::new_at_byte(&bytes, 0);
        let back = read_shape_records(&mut br, 1, false, 0, 0).unwrap();
        assert_eq!(back, recs);
        match &back[0] {
            ShapeRecord::StraightEdge { num_bits, .. } => {
                assert_eq!(*num_bits, 10, "non-minimal edge NumBits preserved");
            }
            other => panic!("expected straight edge, got {other:?}"),
        }
    }

    #[test]
    fn shape_unknown_fill_type_falls_back_to_unknown() {
        // A DefineShape whose FILLSTYLE carries an unmodelled type byte (0xAA)
        // must fall back to Tag::Unknown and re-emit its raw body verbatim, so
        // byte-identity is preserved even for shapes the typed codec can't model.
        // body: shapeId=1, RECT nbits=0 (1 byte), fill count=1, fill type 0xAA, tail.
        let body = vec![0x01, 0x00, 0x00, 0x01, 0xAA, 0xBB];
        match parse_first(&rec(TAG_DEFINE_SHAPE, &body, false)) {
            Tag::Unknown { code, raw, .. } => {
                assert_eq!(code, TAG_DEFINE_SHAPE);
                assert_eq!(raw, body);
            }
            other => panic!("expected Unknown fallback, got {other:?}"),
        }
    }

    // --- Tier-4: DefineEditText (37) tests -----------------------------------

    #[test]
    fn define_edit_text_corpus_instance() {
        // Corpus 01_010_messagebox.gfx characterId 20: bounds RECT nbits=14
        // (-40,7960,-40,680), flags1=0x8c (HasText|ReadOnly|HasTextColor),
        // flags2=0xb1 (HasFontClass|HasLayout|NoSelect|UseOutlines), FontClass
        // "MenuFont_01", FontHeight 480, text color cccccc/alpha 255, layout
        // (align=center,0,0,0,0), empty variableName, initialText "初期化".
        // Verifier-confirmed against the raw body (python ground truth).
        let body = hx(
            "140077fb0f8c7fb015408cb14d656e75466f6e745f303100e001ccccccff02000000000000000000e5889de69c9fe58c9600",
        );
        match parse_first(&rec(TAG_DEFINE_EDIT_TEXT, &body, false)) {
            Tag::DefineEditText {
                character_id,
                bounds,
                flags1,
                flags2,
                font_id,
                font_class,
                font_height,
                text_color,
                max_length,
                layout,
                variable_name,
                initial_text,
                force_long,
            } => {
                assert_eq!(character_id, 20);
                assert!(!force_long);
                assert_eq!(flags1, 0x8c);
                assert_eq!(flags2, 0xb1);
                assert_eq!(bounds.nbits, 14);
                assert_eq!(
                    (bounds.x_min, bounds.x_max, bounds.y_min, bounds.y_max),
                    (-40, 7960, -40, 680)
                );
                assert_eq!(font_id, None);
                assert_eq!(font_class.as_deref(), Some("MenuFont_01"));
                assert_eq!(font_height, Some(480));
                assert_eq!(text_color, Some([0xcc, 0xcc, 0xcc, 0xff]));
                assert_eq!(max_length, None);
                let l = layout.expect("a layout block");
                assert_eq!(l.align, 2); // center
                assert_eq!(
                    (l.left_margin, l.right_margin, l.indent, l.leading),
                    (0, 0, 0, 0)
                );
                assert_eq!(variable_name, "");
                assert_eq!(initial_text.as_deref(), Some("初期化"));
            }
            other => panic!("expected DefineEditText, got {other:?}"),
        }
    }

    #[test]
    fn define_edit_text_with_font_id_and_max_length_roundtrip() {
        // Synthetic instance exercising the HasFont (FontID) and HasMaxLength
        // paths -- HasMaxLength never occurs in the corpus, so this keeps the
        // encoder/decoder mutually consistent for it. flags1 = HasText|
        // HasTextColor|HasMaxLength|HasFont (0x87), flags2 = 0 (no layout/class).
        let mut body = Vec::new();
        body.extend_from_slice(&7u16.to_le_bytes()); // characterId
        body.push(0x00); // RECT nbits=0 -> 1 byte, byte-aligned
        let f1 = ET_HAS_TEXT | ET_HAS_TEXT_COLOR | ET_HAS_MAX_LENGTH | ET_HAS_FONT;
        body.push(f1);
        body.push(0x00); // flags2
        body.extend_from_slice(&42u16.to_le_bytes()); // FontID
        body.extend_from_slice(&240u16.to_le_bytes()); // FontHeight
        body.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]); // text color RGBA
        body.extend_from_slice(&100u16.to_le_bytes()); // MaxLength
        body.extend_from_slice(b"var\0"); // variableName
        body.extend_from_slice(b"hello\0"); // initialText
        match parse_first(&rec(TAG_DEFINE_EDIT_TEXT, &body, true)) {
            Tag::DefineEditText {
                font_id,
                font_class,
                font_height,
                max_length,
                layout,
                variable_name,
                initial_text,
                force_long,
                ..
            } => {
                assert!(force_long);
                assert_eq!(font_id, Some(42));
                assert_eq!(font_class, None);
                assert_eq!(font_height, Some(240));
                assert_eq!(max_length, Some(100));
                assert_eq!(layout, None);
                assert_eq!(variable_name, "var");
                assert_eq!(initial_text.as_deref(), Some("hello"));
            }
            other => panic!("expected DefineEditText, got {other:?}"),
        }
    }

    #[test]
    fn define_edit_text_non_utf8_falls_back_to_unknown() {
        // A non-UTF-8 variableName cannot be modelled as a Rust String; the tag
        // must fall back to Tag::Unknown and re-emit its raw body verbatim so
        // byte-identity is preserved even for un-modellable text.
        let mut body = Vec::new();
        body.extend_from_slice(&1u16.to_le_bytes()); // characterId
        body.push(0x00); // RECT nbits=0
        body.push(0x00); // flags1: no optional fields, no HasText
        body.push(0x00); // flags2
        body.push(0xff); // non-UTF-8 byte in variableName
        body.push(0x00); // NUL terminator
        match parse_first(&rec(TAG_DEFINE_EDIT_TEXT, &body, false)) {
            Tag::Unknown { code, raw, .. } => {
                assert_eq!(code, TAG_DEFINE_EDIT_TEXT);
                assert_eq!(raw, body);
            }
            other => panic!("expected Unknown fallback, got {other:?}"),
        }
    }

    /// Every `DefineEditText` across the whole corpus must decode to the typed
    /// [`Tag::DefineEditText`] variant -- not silently fall back to
    /// [`Tag::Unknown`]. The `tests/roundtrip.rs` byte-identity gate passes
    /// either way, so this separately proves the Tier-4 text codec handles all
    /// corpus instances byte-cleanly. Skips when assets are absent.
    #[test]
    fn corpus_define_edit_texts_all_typed() {
        // 1,479 DefineEditText across the 114-file corpus (python verifier).
        const EXPECTED_TYPED: usize = 1479;
        let root = "/home/banon/er-extract/nuxe-menu-20260619-170932/menu";
        if !std::path::Path::new(root).exists() {
            eprintln!("SKIP: corpus root {root} not present; edit-text-typing test skipped");
            return;
        }

        fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    collect(&p, out);
                } else if p.extension().and_then(|s| s.to_str()) == Some("gfx") {
                    out.push(p);
                }
            }
        }

        fn count(tags: &[Tag], typed: &mut usize, fell_back: &mut usize) {
            for t in tags {
                match t {
                    Tag::DefineEditText { .. } => *typed += 1,
                    Tag::Unknown { code, .. } if *code == TAG_DEFINE_EDIT_TEXT => *fell_back += 1,
                    Tag::DefineSprite { tags, .. } => count(tags, typed, fell_back),
                    _ => {}
                }
            }
        }

        let mut files = Vec::new();
        collect(std::path::Path::new(root), &mut files);
        files.sort();
        let mut typed = 0usize;
        let mut fell_back = 0usize;
        for path in &files {
            let bytes = std::fs::read(path).expect("read corpus file");
            let movie = Movie::parse(&bytes).expect("parse corpus file");
            count(&movie.tags, &mut typed, &mut fell_back);
        }
        assert_eq!(
            fell_back, 0,
            "{fell_back} DefineEditText tag(s) fell back to Tag::Unknown instead of typing"
        );
        assert_eq!(
            typed, EXPECTED_TYPED,
            "expected {EXPECTED_TYPED} typed DefineEditText tags, found {typed}"
        );
    }

    // --- Tier-4: DefineFont3 (75) tests --------------------------------------

    #[test]
    fn define_font3_minimal_roundtrip() {
        // Synthetic single-glyph font (no layout) exercising the offset table,
        // a styleless glyph SHAPE (End-only), and the code table. flags =
        // WideCodes only (so codes are u16, offsets u16).
        let mut body = Vec::new();
        body.extend_from_slice(&1u16.to_le_bytes()); // fontId
        body.push(F3_WIDE_CODES); // flags
        body.push(0x00); // languageCode
        body.push(0x00); // fontNameLen = 0 (empty name)
        body.extend_from_slice(&1u16.to_le_bytes()); // numGlyphs
        // OffsetTable: 2 u16 values. offset_table_start is here; the 2-entry
        // table is 4 bytes, glyph0 (End-only) is 2 bytes.
        body.extend_from_slice(&4u16.to_le_bytes()); // glyph0 offset
        body.extend_from_slice(&6u16.to_le_bytes()); // codeTableOffset
        body.extend_from_slice(&[0x00, 0x00]); // glyph0: nfb=0,nlb=0,End,align
        body.extend_from_slice(&65u16.to_le_bytes()); // code 'A'
        match parse_first(&rec(TAG_DEFINE_FONT3, &body, false)) {
            Tag::DefineFont3 {
                font_id,
                font_name,
                offsets,
                glyphs,
                codes,
                layout,
                ..
            } => {
                assert_eq!(font_id, 1);
                assert!(font_name.is_empty());
                assert_eq!(offsets, vec![4, 6]);
                assert_eq!(glyphs.len(), 1);
                assert_eq!(glyphs[0].records, vec![ShapeRecord::End]);
                assert_eq!(codes, vec![65]);
                assert_eq!(layout, None);
            }
            other => panic!("expected DefineFont3, got {other:?}"),
        }
    }

    #[test]
    fn define_font3_layout_roundtrip() {
        // Synthetic single-glyph font WITH a layout block (ascent/descent/leading
        // + advance + bounds + empty kerning table), exercising the HasLayout path
        // even when the corpus is absent.
        let mut body = Vec::new();
        body.extend_from_slice(&1u16.to_le_bytes()); // fontId
        body.push(F3_HAS_LAYOUT | F3_WIDE_CODES); // flags
        body.push(0x00); // languageCode
        body.push(0x00); // fontNameLen = 0
        body.extend_from_slice(&1u16.to_le_bytes()); // numGlyphs
        body.extend_from_slice(&4u16.to_le_bytes()); // glyph0 offset
        body.extend_from_slice(&6u16.to_le_bytes()); // codeTableOffset
        body.extend_from_slice(&[0x00, 0x00]); // glyph0 End-only
        body.extend_from_slice(&65u16.to_le_bytes()); // code 'A'
        body.extend_from_slice(&100i16.to_le_bytes()); // ascent
        body.extend_from_slice(&20i16.to_le_bytes()); // descent
        body.extend_from_slice(&10i16.to_le_bytes()); // leading
        body.extend_from_slice(&50i16.to_le_bytes()); // advance[0]
        body.push(0x00); // bounds[0] RECT nbits=0 -> 1 byte
        body.extend_from_slice(&0u16.to_le_bytes()); // kerning count = 0
        match parse_first(&rec(TAG_DEFINE_FONT3, &body, true)) {
            Tag::DefineFont3 { layout, .. } => {
                let l = layout.expect("a layout block");
                assert_eq!((l.ascent, l.descent, l.leading), (100, 20, 10));
                assert_eq!(l.advance, vec![50]);
                assert_eq!(l.bounds.len(), 1);
                assert_eq!(l.bounds[0].nbits, 0);
                assert!(l.kernings.is_empty());
            }
            other => panic!("expected DefineFont3, got {other:?}"),
        }
    }

    /// Field-level assertions against a real corpus DefineFont3, plus an
    /// end-to-end byte-identity check. Skips when assets are absent.
    #[test]
    fn define_font3_corpus_instance() {
        let path =
            "/home/banon/er-extract/nuxe-menu-20260619-170932/menu/02_123_worldmap_commandlist.gfx";
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => {
                eprintln!("SKIP: {path} not present");
                return;
            }
        };
        let m = Movie::parse(&bytes).expect("parse corpus file");
        let font = m
            .tags
            .iter()
            .find_map(|t| match t {
                Tag::DefineFont3 {
                    font_id,
                    font_name,
                    glyphs,
                    codes,
                    layout,
                    ..
                } => Some((*font_id, font_name, glyphs, codes, layout)),
                _ => None,
            })
            .expect("a DefineFont3");
        let (font_id, font_name, glyphs, codes, layout) = font;
        assert_eq!(font_id, 24);
        // The length-prefixed name includes a trailing NUL inside its count.
        assert_eq!(font_name.as_slice(), b"FOT-Cezanne ProN DB\0");
        assert_eq!(glyphs.len(), 8);
        assert_eq!(codes[0], 78);
        assert!(layout.is_some());
        // Glyph 0's first EDGE record is a vertical straight edge dy=-14870, with
        // a stored NumBits field of 13 (verifier-confirmed).
        let first_edge = glyphs[0]
            .records
            .iter()
            .find(|r| {
                matches!(
                    r,
                    ShapeRecord::StraightEdge { .. } | ShapeRecord::CurvedEdge { .. }
                )
            })
            .expect("a glyph edge record");
        match first_edge {
            ShapeRecord::StraightEdge {
                num_bits,
                edge: StraightEdge::Vertical { dy },
            } => {
                assert_eq!(*num_bits, 13);
                assert_eq!(*dy, -14870);
            }
            other => panic!("expected vertical straight edge, got {other:?}"),
        }
        // And the whole file still re-serializes byte-identically.
        assert_eq!(m.write().expect("write"), bytes);
    }

    /// Every `DefineFont3` across the whole corpus must decode to the typed
    /// [`Tag::DefineFont3`] variant rather than fall back to [`Tag::Unknown`].
    /// Skips when assets are absent.
    #[test]
    fn corpus_define_font3_all_typed() {
        // 7 DefineFont3 across the 114-file corpus (python verifier).
        const EXPECTED_TYPED: usize = 7;
        let root = "/home/banon/er-extract/nuxe-menu-20260619-170932/menu";
        if !std::path::Path::new(root).exists() {
            eprintln!("SKIP: corpus root {root} not present; font3-typing test skipped");
            return;
        }

        fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    collect(&p, out);
                } else if p.extension().and_then(|s| s.to_str()) == Some("gfx") {
                    out.push(p);
                }
            }
        }

        fn count(tags: &[Tag], typed: &mut usize, fell_back: &mut usize) {
            for t in tags {
                match t {
                    Tag::DefineFont3 { .. } => *typed += 1,
                    Tag::Unknown { code, .. } if *code == TAG_DEFINE_FONT3 => *fell_back += 1,
                    Tag::DefineSprite { tags, .. } => count(tags, typed, fell_back),
                    _ => {}
                }
            }
        }

        let mut files = Vec::new();
        collect(std::path::Path::new(root), &mut files);
        files.sort();
        let mut typed = 0usize;
        let mut fell_back = 0usize;
        for path in &files {
            let bytes = std::fs::read(path).expect("read corpus file");
            let movie = Movie::parse(&bytes).expect("parse corpus file");
            count(&movie.tags, &mut typed, &mut fell_back);
        }
        assert_eq!(
            fell_back, 0,
            "{fell_back} DefineFont3 tag(s) fell back to Tag::Unknown instead of typing"
        );
        assert_eq!(
            typed, EXPECTED_TYPED,
            "expected {EXPECTED_TYPED} typed DefineFont3 tags, found {typed}"
        );
    }
}
