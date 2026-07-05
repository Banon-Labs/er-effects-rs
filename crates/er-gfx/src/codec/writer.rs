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

