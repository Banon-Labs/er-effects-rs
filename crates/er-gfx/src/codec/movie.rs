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
