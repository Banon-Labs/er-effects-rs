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

