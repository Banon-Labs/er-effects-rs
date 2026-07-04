
/// Encode a texture name to its on-disk bytes for `encoding`. Shift-JIS/ASCII
/// (`0`/`2`) is one byte per char + a single `NUL`; UTF-16 (`1`) is little-endian
/// code units + a `u16` `NUL`.
fn encode_name(name: &str, encoding: u8) -> Result<Vec<u8>, TpfError> {
    match encoding {
        TPF_ENCODING_UTF16 => {
            let mut v = Vec::with_capacity(name.len() * 2 + 2);
            for unit in name.encode_utf16() {
                v.extend_from_slice(&unit.to_le_bytes());
            }
            v.extend_from_slice(&[0, 0]);
            Ok(v)
        }
        0 | TPF_ENCODING_SHIFT_JIS => {
            // The ASCII subset of Shift-JIS is identity-mapped; names here are
            // ASCII so the UTF-8 bytes are the Shift-JIS bytes.
            let mut v = name.as_bytes().to_vec();
            v.push(0);
            Ok(v)
        }
        other => Err(TpfError::UnknownEncoding(other)),
    }
}

/// Decode a texture name from `data` starting at absolute `offset`, per
/// `encoding`. Inverse of [`encode_name`].
fn decode_name(data: &[u8], offset: usize, encoding: u8) -> Result<String, TpfError> {
    match encoding {
        TPF_ENCODING_UTF16 => {
            let mut units = Vec::new();
            let mut p = offset;
            loop {
                if p + 2 > data.len() {
                    return Err(TpfError::UnterminatedName);
                }
                let unit = u16::from_le_bytes([data[p], data[p + 1]]);
                p += 2;
                if unit == 0 {
                    break;
                }
                units.push(unit);
            }
            String::from_utf16(&units).map_err(|_| TpfError::InvalidUtf16Name)
        }
        0 | TPF_ENCODING_SHIFT_JIS => {
            let mut end = offset;
            loop {
                if end >= data.len() {
                    return Err(TpfError::UnterminatedName);
                }
                if data[end] == 0 {
                    break;
                }
                end += 1;
            }
            String::from_utf8(data[offset..end].to_vec()).map_err(|_| TpfError::InvalidUtf8Name)
        }
        other => Err(TpfError::UnknownEncoding(other)),
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Read a little-endian `u32` at byte `off` (test-side spec citation).
    fn u32_at(b: &[u8], off: usize) -> u32 {
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }

    // --- Tier 0: DDS exact-byte assertions -------------------------------

    #[test]
    fn dds_exact_bytes_and_total_length() {
        let w = 4u32;
        let h = 2u32;
        let img = DdsImage::solid(w, h, [0x10, 0x20, 0x30, 0x40]);
        let dds = img.to_dds_bytes();

        // Magic 'DDS '.
        assert_eq!(&dds[0..4], b"DDS ");
        assert_eq!(u32_at(&dds, 0), DDS_MAGIC);
        // DDS_HEADER.dwSize == 124.
        assert_eq!(u32_at(&dds, 4), 124);
        // dwFlags == required set (single mip, no MIPMAPCOUNT bit).
        assert_eq!(u32_at(&dds, 8), 0x0000_100F);
        // dwHeight @ +12, dwWidth @ +16.
        assert_eq!(u32_at(&dds, 12), h);
        assert_eq!(u32_at(&dds, 16), w);
        // dwPitchOrLinearSize == width*4 (PITCH).
        assert_eq!(u32_at(&dds, 20), w * 4);
        // dwMipMapCount == 1 @ +28.
        assert_eq!(u32_at(&dds, 28), 1);
        // DDS_PIXELFORMAT.dwSize == 32 @ +76.
        assert_eq!(u32_at(&dds, 76), 32);
        // DDS_PIXELFORMAT.dwFlags == DDPF_FOURCC @ +80.
        assert_eq!(u32_at(&dds, 80), DDPF_FOURCC);
        // DDS_PIXELFORMAT.dwFourCC == 'DX10' @ +84.
        assert_eq!(&dds[84..88], b"DX10");
        assert_eq!(u32_at(&dds, 84), FOURCC_DX10);
        // dwCaps == DDSCAPS_TEXTURE @ +108.
        assert_eq!(u32_at(&dds, 108), DDSCAPS_TEXTURE);
        // DDS_HEADER_DXT10.dxgiFormat == 28 @ +128.
        assert_eq!(u32_at(&dds, 128), 28);
        // resourceDimension == 3 @ +132, arraySize == 1 @ +140.
        assert_eq!(u32_at(&dds, 132), 3);
        assert_eq!(u32_at(&dds, 140), 1);
        // Pixel data starts at +148.
        assert_eq!(DDS_PIXEL_DATA_OFFSET, 148);
        // Total length == 4 + 124 + 20 + w*h*4.
        assert_eq!(dds.len(), 4 + 124 + 20 + (w * h * 4) as usize);
    }

    #[test]
    fn dds_known_pixel_at_known_offset() {
        // 3x3 checker: texel (0,0) is `a`, texel (1,0) is `b`.
        let a = [0xAA, 0xBB, 0xCC, 0xDD];
        let b = [0x11, 0x22, 0x33, 0x44];
        let img = DdsImage::checker(3, 3, 1, a, b);
        let dds = img.to_dds_bytes();

        let texel = |x: usize, y: usize| {
            let off = DDS_PIXEL_DATA_OFFSET + (y * 3 + x) * 4;
            &dds[off..off + 4]
        };
        assert_eq!(texel(0, 0), &a);
        assert_eq!(texel(1, 0), &b);
        assert_eq!(texel(2, 0), &a);
        assert_eq!(texel(0, 1), &b); // (0+1) odd -> b
        assert_eq!(texel(1, 1), &a); // (1+1) even -> a
    }

    #[test]
    fn dds_self_roundtrip_solid() {
        let img = DdsImage::solid(8, 5, [1, 2, 3, 4]);
        let parsed = DdsImage::parse(&img.to_dds_bytes()).expect("parse DDS");
        assert_eq!(parsed, img);
    }

    #[test]
    fn dds_self_roundtrip_checker() {
        let img = DdsImage::checker(16, 16, 4, [9, 8, 7, 6], [1, 2, 3, 255]);
        let parsed = DdsImage::parse(&img.to_dds_bytes()).expect("parse DDS");
        assert_eq!(parsed, img);
    }

    #[test]
    fn dds_parse_rejects_bad_magic() {
        let mut dds = DdsImage::solid(2, 2, [0; 4]).to_dds_bytes();
        dds[0] = b'X';
        match DdsImage::parse(&dds) {
            Err(TpfError::BadDdsMagic(_)) => {}
            other => panic!("expected BadDdsMagic, got {other:?}"),
        }
    }

    #[test]
    fn dds_parse_rejects_wrong_dxgi_format() {
        let mut dds = DdsImage::solid(2, 2, [0; 4]).to_dds_bytes();
        // dxgiFormat lives at +128; flip 28 -> 71 (BC1_UNORM).
        dds[128..132].copy_from_slice(&71u32.to_le_bytes());
        match DdsImage::parse(&dds) {
            Err(TpfError::UnsupportedDxgiFormat(71)) => {}
            other => panic!("expected UnsupportedDxgiFormat(71), got {other:?}"),
        }
    }

    #[test]
    fn dds_parse_rejects_trailing_bytes() {
        let mut dds = DdsImage::solid(2, 2, [0; 4]).to_dds_bytes();
        dds.push(0xFF);
        match DdsImage::parse(&dds) {
            Err(TpfError::TrailingDdsBytes { remaining: 1 }) => {}
            other => panic!("expected TrailingDdsBytes, got {other:?}"),
        }
    }

    // --- Tier 0: legacy (non-DX10) RGBA8 DDS header ----------------------

    #[test]
    fn dds_legacy_exact_bytes_and_total_length() {
        let w = 4u32;
        let h = 2u32;
        let img = DdsImage::solid(w, h, [0x10, 0x20, 0x30, 0x40]);
        let dds = img.to_dds_bytes_with(DdsHeaderMode::LegacyRgba8);

        // Magic 'DDS '.
        assert_eq!(&dds[0..4], b"DDS ");
        assert_eq!(u32_at(&dds, 0), DDS_MAGIC);
        // DDS_HEADER.dwSize == 124.
        assert_eq!(u32_at(&dds, 4), 124);
        // dwFlags == required set (single mip, no MIPMAPCOUNT bit).
        assert_eq!(u32_at(&dds, 8), 0x0000_100F);
        // dwHeight @ +12, dwWidth @ +16.
        assert_eq!(u32_at(&dds, 12), h);
        assert_eq!(u32_at(&dds, 16), w);
        // dwPitchOrLinearSize == width*4 (PITCH).
        assert_eq!(u32_at(&dds, 20), w * 4);
        // DDS_PIXELFORMAT.dwSize == 32 @ +76.
        assert_eq!(u32_at(&dds, 76), 32);
        // DDS_PIXELFORMAT.dwFlags == DDPF_RGB|DDPF_ALPHAPIXELS == 0x41 @ +80.
        assert_eq!(u32_at(&dds, 80), 0x41);
        assert_eq!(u32_at(&dds, 80), DDPF_RGBA);
        // dwFourCC == 0 @ +84 (legacy: no DX10 extension).
        assert_eq!(u32_at(&dds, 84), 0);
        // dwRGBBitCount == 32 @ +88.
        assert_eq!(u32_at(&dds, 88), 32);
        // The four channel masks @ +92/+96/+100/+104.
        assert_eq!(u32_at(&dds, 92), 0x0000_00ff); // dwRBitMask
        assert_eq!(u32_at(&dds, 96), 0x0000_ff00); // dwGBitMask
        assert_eq!(u32_at(&dds, 100), 0x00ff_0000); // dwBBitMask
        assert_eq!(u32_at(&dds, 104), 0xff00_0000); // dwABitMask
        // dwCaps == DDSCAPS_TEXTURE @ +108.
        assert_eq!(u32_at(&dds, 108), DDSCAPS_TEXTURE);
        // Pixel data starts at +128 (no 20-byte DDS_HEADER_DXT10).
        assert_eq!(DDS_LEGACY_PIXEL_DATA_OFFSET, 128);
        assert_eq!(&dds[128..132], &[0x10, 0x20, 0x30, 0x40]);
        // Total length == 4 + 124 + w*h*4 (no DXT10 header).
        assert_eq!(dds.len(), 4 + 124 + (w * h * 4) as usize);
    }

    #[test]
    fn dds_legacy_self_roundtrip() {
        let img = DdsImage::checker(16, 16, 4, [9, 8, 7, 6], [1, 2, 3, 255]);
        let dds = img.to_dds_bytes_with(DdsHeaderMode::LegacyRgba8);
        let parsed = DdsImage::parse(&dds).expect("parse legacy DDS");
        assert_eq!(parsed, img);
    }

    #[test]
    fn dds_legacy_and_dx10_decode_to_same_image() {
        let img = DdsImage::checker(8, 4, 2, [10, 20, 30, 40], [200, 150, 100, 50]);
        let dx10_bytes = img.to_dds_bytes_with(DdsHeaderMode::Dx10);
        let legacy_bytes = img.to_dds_bytes_with(DdsHeaderMode::LegacyRgba8);

        // Both header forms describe the same pixels and parse to the same image.
        let dx10 = DdsImage::parse(&dx10_bytes).expect("parse dx10");
        let legacy = DdsImage::parse(&legacy_bytes).expect("parse legacy");
        assert_eq!(dx10, img);
        assert_eq!(legacy, img);
        assert_eq!(dx10, legacy);

        // The only size difference is the 20-byte DDS_HEADER_DXT10.
        assert_eq!(dx10_bytes.len() - legacy_bytes.len(), DDS_DXT10_HEADER_SIZE);
        // Default to_dds_bytes() is still the DX10 form (byte-identical).
        assert_eq!(img.to_dds_bytes(), dx10_bytes);
    }

    #[test]
    fn dds_legacy_parse_rejects_wrong_masks() {
        let mut dds = DdsImage::solid(2, 2, [0; 4]).to_dds_bytes_with(DdsHeaderMode::LegacyRgba8);
        // Corrupt dwBBitMask @ +100 (B8G8R8A8-style swap), which no longer
        // matches the R8G8B8A8 layout the legacy path accepts.
        dds[100..104].copy_from_slice(&0x0000_00ffu32.to_le_bytes());
        match DdsImage::parse(&dds) {
            Err(TpfError::UnsupportedLegacyPixelFormat { .. }) => {}
            other => panic!("expected UnsupportedLegacyPixelFormat, got {other:?}"),
        }
    }

    // --- Tier 1: TPF003 wrap + self round-trip ---------------------------

    #[test]
    fn tpf_header_and_entry_layout() {
        let img = DdsImage::solid(4, 4, [0x80, 0x80, 0x80, 0xFF]);
        let dds = img.to_dds_bytes();
        let dds_len = dds.len();
        let tpf = Tpf::single_pc("cover", dds, 1);
        let blob = tpf.build().expect("build TPF");

        // Header: magic, total size, fileCount, platform/flag2/encoding/reserved.
        assert_eq!(&blob[0..4], b"TPF\0");
        assert_eq!(u32_at(&blob, 4), dds_len as u32); // totalTextureDataSize
        assert_eq!(u32_at(&blob, 8), 1); // fileCount
        assert_eq!(blob[0x0C], TPF_PLATFORM_PC);
        assert_eq!(blob[0x0D], TPF_DEFAULT_FLAG2);
        assert_eq!(blob[0x0E], TPF_ENCODING_SHIFT_JIS);
        assert_eq!(blob[0x0F], 0); // extFlag bit0 CLEAR

        // Entry table begins at +0x10.
        let data_offset = u32_at(&blob, 0x10) as usize;
        let data_size = u32_at(&blob, 0x14) as usize;
        assert_eq!(data_size, dds_len);
        assert_eq!(blob[0x18], TPF_FORMAT_R8G8B8A8_UNORM); // format
        assert_eq!(blob[0x19], TEX_TYPE_TEXTURE); // type
        assert_eq!(blob[0x1A], 1); // mipCount
        assert_eq!(blob[0x1B], 0); // flags1
        let name_offset = u32_at(&blob, 0x1C) as usize;
        assert_eq!(u32_at(&blob, 0x20), 0); // hasFloatStruct == 0

        // Self-consistency: every referenced range is in-bounds and the DDS
        // bytes at dataOffset are the encoded DDS (magic check).
        assert!(data_offset + data_size <= blob.len());
        assert!(name_offset < blob.len());
        assert_eq!(&blob[data_offset..data_offset + 4], b"DDS ");
        // Name string at nameOffset is "cover\0".
        assert_eq!(&blob[name_offset..name_offset + 6], b"cover\0");
    }

    #[test]
    fn tpf_entry_name_settable_lands_at_offset_and_roundtrips() {
        // The game's in-memory TPF upload derives the GLOBAL_TexRepository
        // (SYSTEX) key from the entry's own texture-name string, so a
        // caller-set name must land verbatim at nameOffset (Shift-JIS/ASCII:
        // raw bytes + a single NUL) and survive the parse round-trip.
        let key = "SYSTEX_TitleCover_00";
        let dds = DdsImage::solid(2, 2, [1, 2, 3, 4]).to_dds_bytes();
        let tpf = Tpf::single_pc(key, dds, 1);
        let blob = tpf.build().expect("build");

        // nameOffset is the entry's 5th field @ +0x1C.
        let name_offset = u32_at(&blob, 0x1C) as usize;
        let mut expected = key.as_bytes().to_vec();
        expected.push(0); // NUL terminator
        assert_eq!(
            &blob[name_offset..name_offset + expected.len()],
            &expected[..]
        );

        // And it round-trips through the parser to exactly the set key.
        let parsed = Tpf::parse(&blob).expect("parse");
        assert_eq!(parsed.textures[0].name, key);
        assert_eq!(parsed, tpf);
    }

    #[test]
    fn tpf_self_roundtrip_single() {
        let img = DdsImage::checker(8, 8, 2, [255, 0, 0, 255], [0, 0, 255, 255]);
        let tpf = Tpf::single_pc("title_cover", img.to_dds_bytes(), 1);
        let parsed = Tpf::parse(&tpf.build().expect("build")).expect("parse");
        assert_eq!(parsed, tpf);
        // And the wrapped DDS still decodes back to the original image.
        let inner = DdsImage::parse(&parsed.textures[0].dds).expect("parse inner DDS");
        assert_eq!(inner, img);
    }

    #[test]
    fn tpf_self_roundtrip_multi_texture() {
        let a = DdsImage::solid(2, 2, [1, 1, 1, 1]).to_dds_bytes();
        let b = DdsImage::checker(4, 4, 1, [0; 4], [255; 4]).to_dds_bytes();
        let tpf = Tpf {
            platform: TPF_PLATFORM_PC,
            flag2: 1,
            encoding: TPF_ENCODING_SHIFT_JIS,
            textures: vec![
                TpfTexture {
                    name: "first".into(),
                    format: TPF_FORMAT_R8G8B8A8_UNORM,
                    tex_type: TEX_TYPE_TEXTURE,
                    mip_count: 1,
                    flags1: 0,
                    dds: a,
                },
                TpfTexture {
                    name: "second".into(),
                    format: TPF_FORMAT_R8G8B8A8_UNORM,
                    tex_type: TEX_TYPE_TEXTURE,
                    mip_count: 1,
                    flags1: 2,
                    dds: b,
                },
            ],
        };
        let parsed = Tpf::parse(&tpf.build().expect("build")).expect("parse");
        assert_eq!(parsed, tpf);
    }

    #[test]
    fn tpf_self_roundtrip_utf16_name() {
        let tpf = Tpf {
            platform: TPF_PLATFORM_PC,
            flag2: TPF_DEFAULT_FLAG2,
            encoding: TPF_ENCODING_UTF16,
            textures: vec![TpfTexture {
                name: "ｗｉｄｅ".into(),
                format: TPF_FORMAT_R8G8B8A8_UNORM,
                tex_type: TEX_TYPE_TEXTURE,
                mip_count: 1,
                flags1: 0,
                dds: DdsImage::solid(1, 1, [7, 7, 7, 7]).to_dds_bytes(),
            }],
        };
        let parsed = Tpf::parse(&tpf.build().expect("build")).expect("parse");
        assert_eq!(parsed, tpf);
    }

    #[test]
    fn tpf_total_texture_data_is_sum() {
        let a = DdsImage::solid(2, 2, [0; 4]).to_dds_bytes();
        let b = DdsImage::solid(3, 1, [0; 4]).to_dds_bytes();
        let sum = (a.len() + b.len()) as u32;
        let tpf = Tpf {
            platform: TPF_PLATFORM_PC,
            flag2: 0,
            encoding: TPF_ENCODING_SHIFT_JIS,
            textures: vec![
                TpfTexture {
                    name: "a".into(),
                    format: TPF_FORMAT_R8G8B8A8_UNORM,
                    tex_type: TEX_TYPE_TEXTURE,
                    mip_count: 1,
                    flags1: 0,
                    dds: a,
                },
                TpfTexture {
                    name: "b".into(),
                    format: TPF_FORMAT_R8G8B8A8_UNORM,
                    tex_type: TEX_TYPE_TEXTURE,
                    mip_count: 1,
                    flags1: 0,
                    dds: b,
                },
            ],
        };
        let blob = tpf.build().expect("build");
        assert_eq!(u32_at(&blob, 4), sum);
    }

    #[test]
    fn tpf_parse_rejects_bad_magic() {
        let mut blob = Tpf::single_pc("x", DdsImage::solid(1, 1, [0; 4]).to_dds_bytes(), 1)
            .build()
            .expect("build");
        blob[0] = b'Z';
        match Tpf::parse(&blob) {
            Err(TpfError::BadTpfMagic(_)) => {}
            other => panic!("expected BadTpfMagic, got {other:?}"),
        }
    }

    #[test]
    fn tpf_parse_rejects_non_pc_platform() {
        let mut blob = Tpf::single_pc("x", DdsImage::solid(1, 1, [0; 4]).to_dds_bytes(), 1)
            .build()
            .expect("build");
        blob[0x0C] = 4; // PS4
        match Tpf::parse(&blob) {
            Err(TpfError::UnsupportedPlatform(4)) => {}
            other => panic!("expected UnsupportedPlatform(4), got {other:?}"),
        }
    }

    #[test]
    fn tpf_parse_rejects_total_size_mismatch() {
        let mut blob = Tpf::single_pc("x", DdsImage::solid(1, 1, [0; 4]).to_dds_bytes(), 1)
            .build()
            .expect("build");
        // Corrupt totalTextureDataSize @ +4.
        let bad = (u32_at(&blob, 4) + 1).to_le_bytes();
        blob[4..8].copy_from_slice(&bad);
        match Tpf::parse(&blob) {
            Err(TpfError::TotalSizeMismatch { .. }) => {}
            other => panic!("expected TotalSizeMismatch, got {other:?}"),
        }
    }

    #[test]
    fn tpf_parse_rejects_out_of_range_data_offset() {
        let mut blob = Tpf::single_pc("x", DdsImage::solid(1, 1, [0; 4]).to_dds_bytes(), 1)
            .build()
            .expect("build");
        // Push dataOffset @ +0x10 past the end of the blob.
        blob[0x10..0x14].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        match Tpf::parse(&blob) {
            Err(TpfError::OffsetOutOfRange { .. }) => {}
            other => panic!("expected OffsetOutOfRange, got {other:?}"),
        }
    }
}
