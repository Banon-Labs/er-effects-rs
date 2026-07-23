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
