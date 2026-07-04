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

