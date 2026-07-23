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
