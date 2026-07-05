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

