
// ===========================================================================
// DDS constants (Microsoft "DDS Programming Guide" / DXGI). Named so the test
// byte assertions read as spec citations.
// ===========================================================================

/// `DDS ` magic as a little-endian `u32` (bytes `b"DDS "` = `44 44 53 20`).
pub const DDS_MAGIC: u32 = 0x2053_4444;
/// `DDS_HEADER.dwSize` -- always 124 for a valid DDS header.
pub const DDS_HEADER_SIZE: u32 = 124;
/// `DDS_PIXELFORMAT.dwSize` -- always 32.
pub const DDS_PIXELFORMAT_SIZE: u32 = 32;
/// Size of the `DDS_HEADER_DXT10` extension (5 `u32`s).
pub const DDS_DXT10_HEADER_SIZE: usize = 20;

/// Byte offset of the first pixel: 4 (magic) + 124 (`DDS_HEADER`) + 20
/// (`DDS_HEADER_DXT10`).
pub const DDS_PIXEL_DATA_OFFSET: usize = 4 + DDS_HEADER_SIZE as usize + DDS_DXT10_HEADER_SIZE;

// --- DDS_HEADER.dwFlags bits. ---
/// `DDSD_CAPS`: `dwCaps` is valid (required).
pub const DDSD_CAPS: u32 = 0x0000_0001;
/// `DDSD_HEIGHT`: `dwHeight` is valid (required).
pub const DDSD_HEIGHT: u32 = 0x0000_0002;
/// `DDSD_WIDTH`: `dwWidth` is valid (required).
pub const DDSD_WIDTH: u32 = 0x0000_0004;
/// `DDSD_PITCH`: `dwPitchOrLinearSize` is a row pitch (uncompressed textures).
pub const DDSD_PITCH: u32 = 0x0000_0008;
/// `DDSD_PIXELFORMAT`: `ddspf` is valid (required).
pub const DDSD_PIXELFORMAT: u32 = 0x0000_1000;
/// `DDSD_MIPMAPCOUNT`: `dwMipMapCount` is valid (set only when `mips > 1`).
pub const DDSD_MIPMAPCOUNT: u32 = 0x0002_0000;
/// The always-required `dwFlags` set for an uncompressed single-mip texture:
/// `CAPS | HEIGHT | WIDTH | PITCH | PIXELFORMAT` = `0x0000_100F`.
pub const DDSD_REQUIRED: u32 = DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH | DDSD_PITCH | DDSD_PIXELFORMAT;

// --- DDS_PIXELFORMAT.dwFlags bits. ---
/// `DDPF_FOURCC`: `dwFourCC` is valid. Set so the `DX10` extension is present.
pub const DDPF_FOURCC: u32 = 0x0000_0004;
/// `DX10` four-CC as a little-endian `u32` (bytes `b"DX10"` = `44 58 31 30`).
/// Its presence signals that a `DDS_HEADER_DXT10` follows the `DDS_HEADER`.
pub const FOURCC_DX10: u32 = 0x3031_5844;

// --- DDS_HEADER.dwCaps bits. ---
/// `DDSCAPS_TEXTURE`: required on every DDS.
pub const DDSCAPS_TEXTURE: u32 = 0x0000_1000;

// --- DDS_HEADER_DXT10 values. ---
/// `DXGI_FORMAT_R8G8B8A8_UNORM` -- the uncompressed 32-bpp RGBA format Tier-0
/// emits.
pub const DXGI_FORMAT_R8G8B8A8_UNORM: u32 = 28;
/// `D3D10_RESOURCE_DIMENSION_TEXTURE2D`.
pub const D3D10_RESOURCE_DIMENSION_TEXTURE2D: u32 = 3;

// --- Legacy (non-DX10) DDS_PIXELFORMAT masked path. ---
// The classic R8G8B8A8 surface description: no `DX10` four-CC and no
// `DDS_HEADER_DXT10`, just the four channel bit masks. The engine's legacy DDS
// path maps this to DXGI 28, which sidesteps the strict DX10 format validator.

/// `DDPF_ALPHAPIXELS`: an alpha channel is present (`dwABitMask` is valid).
pub const DDPF_ALPHAPIXELS: u32 = 0x0000_0001;
/// `DDPF_RGB`: uncompressed RGB data is present (the four channel masks are
/// valid).
pub const DDPF_RGB: u32 = 0x0000_0040;
/// The legacy `DDS_PIXELFORMAT.dwFlags` for an RGBA8 surface:
/// `DDPF_RGB | DDPF_ALPHAPIXELS` = `0x41`.
pub const DDPF_RGBA: u32 = DDPF_RGB | DDPF_ALPHAPIXELS;

/// Legacy R8G8B8A8 `dwRGBBitCount` (32 bits per pixel).
pub const RGBA8_BIT_COUNT: u32 = 32;
/// Legacy `dwRBitMask` for R8G8B8A8 (red is the low byte, matching the in-memory
/// `R,G,B,A` byte order).
pub const RGBA8_R_MASK: u32 = 0x0000_00ff;
/// Legacy `dwGBitMask` for R8G8B8A8.
pub const RGBA8_G_MASK: u32 = 0x0000_ff00;
/// Legacy `dwBBitMask` for R8G8B8A8.
pub const RGBA8_B_MASK: u32 = 0x00ff_0000;
/// Legacy `dwABitMask` for R8G8B8A8 (alpha is the high byte).
pub const RGBA8_A_MASK: u32 = 0xff00_0000;

/// Byte offset of the first pixel in a legacy (non-DX10) DDS: 4 (magic) + 124
/// (`DDS_HEADER`), with **no** `DDS_HEADER_DXT10`.
pub const DDS_LEGACY_PIXEL_DATA_OFFSET: usize = 4 + DDS_HEADER_SIZE as usize;

/// Bytes per `R8G8B8A8_UNORM` pixel.
const RGBA8_BYTES_PER_PIXEL: usize = 4;

// ===========================================================================
// TPF003 (PC) constants (SoulsFormats `TPF`). The PC entry layout is documented
// inline at the read/build sites.
// ===========================================================================

/// `TPF\0` container magic.
pub const TPF_MAGIC: [u8; 4] = *b"TPF\0";
/// Fixed TPF header size; the texture-entry table begins at this offset (the
/// `extFlag` byte at +0x0F is kept clear so no extended header is present).
pub const TPF_HEADER_SIZE: usize = 0x10;
/// Size of one PC (`TPFPlatform.PC`) texture entry: `dataOffset(u32)` +
/// `dataSize(u32)` + `format(u8)` + `type(u8)` + `mipCount(u8)` + `flags1(u8)` +
/// `nameOffset(u32)` + `hasFloatStruct(u32)`.
pub const TPF_PC_ENTRY_SIZE: usize = 0x14;

/// `TPFPlatform.PC` (little-endian, no per-texture platform header). The only
/// platform this crate builds or parses.
pub const TPF_PLATFORM_PC: u8 = 0;

/// Default `Flag2` byte. SoulsFormats asserts `Flag2 in {0,1,2,3}`; its exact
/// semantics are not pinned down here. It is round-tripped verbatim and does not
/// affect Tier-1's self-consistency gate. Documented-uncertain.
pub const TPF_DEFAULT_FLAG2: u8 = 3;

/// `Encoding` = Shift-JIS (the ASCII subset is one byte per char + a `NUL`
/// terminator). SoulsFormats treats `0` and `2` as Shift-JIS and `1` as UTF-16.
pub const TPF_ENCODING_SHIFT_JIS: u8 = 2;
/// `Encoding` = UTF-16LE (two bytes per code unit + a `u16` `NUL` terminator).
pub const TPF_ENCODING_UTF16: u8 = 1;

/// `TexType.Texture` (a plain 2D texture). Cubemap/Volume are 1/2.
pub const TEX_TYPE_TEXTURE: u8 = 0;

/// TPF `format` byte for `R8G8B8A8_UNORM`, per the FromSoftware TPF format table
/// (the DSMapStudio `TexUtil` mapping: `9` = `B8G8R8A8`, `10` = `R8G8B8A8`).
/// **Documented-uncertain**: the authoritative raster description is the
/// `DDS_HEADER_DXT10.dxgiFormat` (= `28`); this byte is a loader hint and its
/// exact game acceptance is a later runtime tier. It is round-tripped verbatim
/// and does not affect the self-consistency gate.
pub const TPF_FORMAT_R8G8B8A8_UNORM: u8 = 10;

// ===========================================================================
// Errors
// ===========================================================================

/// Errors produced while building or parsing a DDS blob or TPF container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TpfError {
    /// Ran out of input: needed `need` bytes at `pos`, only `have` available.
    UnexpectedEof {
        pos: usize,
        need: usize,
        have: usize,
    },
    /// TPF did not start with the `TPF\0` magic.
    BadTpfMagic([u8; 4]),
    /// DDS did not start with the `DDS ` magic.
    BadDdsMagic([u8; 4]),
    /// `DDS_HEADER.dwSize` was not 124.
    BadDdsHeaderSize(u32),
    /// `DDS_PIXELFORMAT.dwSize` was not 32.
    BadPixelFormatSize(u32),
    /// The pixel format did not advertise a `DX10` four-CC, so no
    /// `DDS_HEADER_DXT10` is present (this Tier-0 encoder always emits one).
    MissingDxt10Header,
    /// `DDS_HEADER_DXT10.dxgiFormat` was not `R8G8B8A8_UNORM` (28).
    UnsupportedDxgiFormat(u32),
    /// A legacy (non-DX10) `DDS_PIXELFORMAT` advertised RGB bit masks that do not
    /// match the `R8G8B8A8_UNORM` layout this crate's legacy path supports.
    UnsupportedLegacyPixelFormat {
        rgb_bits: u32,
        r_mask: u32,
        g_mask: u32,
        b_mask: u32,
        a_mask: u32,
    },
    /// A single-mip DDS carried more pixel bytes than `width * height * 4`.
    TrailingDdsBytes { remaining: usize },
    /// The parser only supports `TPFPlatform.PC` (0).
    UnsupportedPlatform(u8),
    /// A PC texture entry set `hasFloatStruct != 0`; the optional `FloatStruct`
    /// trailer is not modelled by this Tier-1 builder.
    FloatStructUnsupported(u32),
    /// A texture entry's `dataOffset + dataSize` or `nameOffset` fell outside the
    /// blob. `context` names the field.
    OffsetOutOfRange {
        context: &'static str,
        offset: usize,
        size: usize,
        blob_len: usize,
    },
    /// `totalTextureDataSize` did not equal the sum of the per-texture
    /// `dataSize`s.
    TotalSizeMismatch { declared: u32, computed: u32 },
    /// A texture name ran to the end of the blob without a `NUL` terminator.
    UnterminatedName,
    /// A Shift-JIS/ASCII texture name was not valid UTF-8.
    InvalidUtf8Name,
    /// A UTF-16 texture name had an odd byte length (not a whole number of code
    /// units).
    OddUtf16NameLength,
    /// A UTF-16 texture name was not valid UTF-16.
    InvalidUtf16Name,
    /// An unknown TPF `Encoding` byte was encountered while decoding a name.
    UnknownEncoding(u8),
}

impl fmt::Display for TpfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TpfError::UnexpectedEof { pos, need, have } => write!(
                f,
                "unexpected EOF: needed {need} byte(s) at offset {pos}, only {have} available"
            ),
            TpfError::BadTpfMagic(m) => write!(f, "bad TPF magic: expected TPF\\0, got {m:02x?}"),
            TpfError::BadDdsMagic(m) => write!(f, "bad DDS magic: expected 'DDS ', got {m:02x?}"),
            TpfError::BadDdsHeaderSize(s) => write!(f, "DDS_HEADER.dwSize {s} != 124"),
            TpfError::BadPixelFormatSize(s) => write!(f, "DDS_PIXELFORMAT.dwSize {s} != 32"),
            TpfError::MissingDxt10Header => {
                write!(
                    f,
                    "DDS pixel format is not 'DX10'; no DDS_HEADER_DXT10 present"
                )
            }
            TpfError::UnsupportedDxgiFormat(fmt) => {
                write!(
                    f,
                    "unsupported dxgiFormat {fmt} (expected 28 R8G8B8A8_UNORM)"
                )
            }
            TpfError::UnsupportedLegacyPixelFormat {
                rgb_bits,
                r_mask,
                g_mask,
                b_mask,
                a_mask,
            } => write!(
                f,
                "unsupported legacy DDS_PIXELFORMAT (rgbBitCount {rgb_bits}, masks \
                 R={r_mask:#010x} G={g_mask:#010x} B={b_mask:#010x} A={a_mask:#010x}); \
                 expected R8G8B8A8_UNORM"
            ),
            TpfError::TrailingDdsBytes { remaining } => {
                write!(
                    f,
                    "{remaining} trailing DDS byte(s) past a single mip level"
                )
            }
            TpfError::UnsupportedPlatform(p) => {
                write!(f, "unsupported TPF platform {p} (only PC=0 is supported)")
            }
            TpfError::FloatStructUnsupported(v) => {
                write!(
                    f,
                    "texture entry hasFloatStruct={v} (FloatStruct not modelled)"
                )
            }
            TpfError::OffsetOutOfRange {
                context,
                offset,
                size,
                blob_len,
            } => write!(
                f,
                "{context} range {offset}+{size} exceeds blob length {blob_len}"
            ),
            TpfError::TotalSizeMismatch { declared, computed } => write!(
                f,
                "totalTextureDataSize {declared} != sum of texture sizes {computed}"
            ),
            TpfError::UnterminatedName => write!(f, "unterminated texture name"),
            TpfError::InvalidUtf8Name => write!(f, "texture name was not valid UTF-8"),
            TpfError::OddUtf16NameLength => write!(f, "UTF-16 texture name had an odd byte length"),
            TpfError::InvalidUtf16Name => write!(f, "texture name was not valid UTF-16"),
            TpfError::UnknownEncoding(e) => write!(f, "unknown TPF encoding byte {e}"),
        }
    }
}

impl std::error::Error for TpfError {}

// ===========================================================================
// Little-endian byte helpers
// ===========================================================================

/// Append-only little-endian byte sink.
struct LeWriter {
    buf: Vec<u8>,
}

impl LeWriter {
    fn new() -> Self {
        LeWriter { buf: Vec::new() }
    }

    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    /// Append `n` zero bytes (reserved/padding fields).
    fn zeros(&mut self, n: usize) {
        self.buf.resize(self.buf.len() + n, 0);
    }

    fn pos(&self) -> usize {
        self.buf.len()
    }
}

/// Forward cursor over input bytes with bounds-checked reads. Also exposes
/// absolute-offset slicing for the offset-referenced TPF name/data regions.
struct LeReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> LeReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        LeReader { data, pos: 0 }
    }

    fn need(&self, n: usize) -> Result<(), TpfError> {
        if self.pos + n > self.data.len() {
            Err(TpfError::UnexpectedEof {
                pos: self.pos,
                need: n,
                have: self.data.len().saturating_sub(self.pos),
            })
        } else {
            Ok(())
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], TpfError> {
        self.need(n)?;
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn array4(&mut self) -> Result<[u8; 4], TpfError> {
        let s = self.take(4)?;
        Ok([s[0], s[1], s[2], s[3]])
    }

    fn u8(&mut self) -> Result<u8, TpfError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, TpfError> {
        Ok(u32::from_le_bytes(self.array4()?))
    }

    fn skip(&mut self, n: usize) -> Result<(), TpfError> {
        self.take(n)?;
        Ok(())
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
}

// ===========================================================================
// Tier 0 -- DDS encoder (uncompressed R8G8B8A8_UNORM, single mip)
// ===========================================================================

/// Selects which DDS header form [`DdsImage::to_dds_bytes_with`] emits. Both
/// describe the same uncompressed 32-bpp `R8G8B8A8_UNORM` pixels; they differ
/// only in the header the engine's loader reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DdsHeaderMode {
    /// `DX10` four-CC + a `DDS_HEADER_DXT10` carrying `dxgiFormat = 28`. The
    /// strict modern form; the pixel data begins at [`DDS_PIXEL_DATA_OFFSET`]
    /// (148). This is the default ([`DdsImage::to_dds_bytes`]).
    #[default]
    Dx10,
    /// Legacy `DDS_PIXELFORMAT` masks (`DDPF_RGB | DDPF_ALPHAPIXELS`, 32-bpp with
    /// the R8G8B8A8 channel masks) and **no** `DDS_HEADER_DXT10`, so the pixel
    /// data begins at [`DDS_LEGACY_PIXEL_DATA_OFFSET`] (128). Maps to DXGI `28`
    /// via the engine's legacy path and bypasses the DX10 format validator --
    /// the safest first-proof form.
    LegacyRgba8,
}

/// An uncompressed `R8G8B8A8_UNORM` image: a `width x height` row-major RGBA8
/// pixel buffer. `pixels.len()` is always `width * height * 4`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DdsImage {
    pub width: u32,
    pub height: u32,
    /// Row-major RGBA8 pixels: 4 bytes (`R`, `G`, `B`, `A`) per texel.
    pub pixels: Vec<u8>,
}

impl DdsImage {
    /// Build a solid-color image (every texel `rgba`).
    pub fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let count = (width as usize) * (height as usize);
        let mut pixels = Vec::with_capacity(count * RGBA8_BYTES_PER_PIXEL);
        for _ in 0..count {
            pixels.extend_from_slice(&rgba);
        }
        DdsImage {
            width,
            height,
            pixels,
        }
    }

    /// Build a 2-color checker. `cell` is the square size in texels (clamped to
    /// at least 1); a texel is `a` when `(x/cell + y/cell)` is even, else `b`.
    pub fn checker(width: u32, height: u32, cell: u32, a: [u8; 4], b: [u8; 4]) -> Self {
        let cell = cell.max(1);
        let mut pixels =
            Vec::with_capacity((width as usize) * (height as usize) * RGBA8_BYTES_PER_PIXEL);
        for y in 0..height {
            for x in 0..width {
                let even = ((x / cell) + (y / cell)) & 1 == 0;
                pixels.extend_from_slice(if even { &a } else { &b });
            }
        }
        DdsImage {
            width,
            height,
            pixels,
        }
    }

    /// The single-mip row pitch in bytes (`DDS_HEADER.dwPitchOrLinearSize` for an
    /// uncompressed texture): `width * 4`.
    pub fn pitch(&self) -> u32 {
        self.width * RGBA8_BYTES_PER_PIXEL as u32
    }

    /// Encode the Tier-0 DDS blob using the default [`DdsHeaderMode::Dx10`]
    /// header: `DDS ` magic + 124-byte `DDS_HEADER` + 20-byte `DDS_HEADER_DXT10`
    /// + raw RGBA pixel bytes (single mip).
    pub fn to_dds_bytes(&self) -> Vec<u8> {
        self.to_dds_bytes_with(DdsHeaderMode::Dx10)
    }

    /// Encode the Tier-0 DDS blob with a caller-chosen header form.
    ///
    /// Both forms emit the same `DDS ` magic, 124-byte `DDS_HEADER`, and raw
    /// single-mip RGBA pixels. They differ only in `DDS_PIXELFORMAT` and the
    /// presence of `DDS_HEADER_DXT10`:
    ///
    /// * [`DdsHeaderMode::Dx10`] -- `DDPF_FOURCC` + `DX10` four-CC, then a
    ///   20-byte `DDS_HEADER_DXT10` with `dxgiFormat = 28`. Pixels at
    ///   [`DDS_PIXEL_DATA_OFFSET`] (148).
    /// * [`DdsHeaderMode::LegacyRgba8`] -- `DDPF_RGB | DDPF_ALPHAPIXELS`, the
    ///   32-bpp R8G8B8A8 channel masks, four-CC `0`, and **no**
    ///   `DDS_HEADER_DXT10`. Pixels at [`DDS_LEGACY_PIXEL_DATA_OFFSET`] (128).
    pub fn to_dds_bytes_with(&self, mode: DdsHeaderMode) -> Vec<u8> {
        debug_assert_eq!(
            self.pixels.len(),
            (self.width as usize) * (self.height as usize) * RGBA8_BYTES_PER_PIXEL,
            "DdsImage pixel buffer length must be width*height*4"
        );

        // Single mip for Tier 0. The MIPMAPCOUNT flag is set only when mips > 1
        // (documented here even though Tier 0 never takes that branch).
        let mips: u32 = 1;
        let mut flags = DDSD_REQUIRED;
        if mips > 1 {
            flags |= DDSD_MIPMAPCOUNT;
        }

        let mut w = LeWriter::new();
        // Magic.
        w.u32(DDS_MAGIC);

        // --- DDS_HEADER (124 bytes) ---
        w.u32(DDS_HEADER_SIZE); // dwSize = 124
        w.u32(flags); // dwFlags
        w.u32(self.height); // dwHeight
        w.u32(self.width); // dwWidth
        w.u32(self.pitch()); // dwPitchOrLinearSize = width*4 (PITCH)
        w.u32(0); // dwDepth
        w.u32(mips); // dwMipMapCount
        w.zeros(44); // dwReserved1[11]
        // DDS_PIXELFORMAT (32 bytes) -- the only header region that differs by
        // mode.
        w.u32(DDS_PIXELFORMAT_SIZE); // dwSize = 32
        match mode {
            DdsHeaderMode::Dx10 => {
                w.u32(DDPF_FOURCC); // dwFlags = DDPF_FOURCC
                w.u32(FOURCC_DX10); // dwFourCC = 'DX10' (DXT10 header follows)
                w.u32(0); // dwRGBBitCount (unused with DX10)
                w.u32(0); // dwRBitMask
                w.u32(0); // dwGBitMask
                w.u32(0); // dwBBitMask
                w.u32(0); // dwABitMask
            }
            DdsHeaderMode::LegacyRgba8 => {
                w.u32(DDPF_RGBA); // dwFlags = DDPF_RGB | DDPF_ALPHAPIXELS (0x41)
                w.u32(0); // dwFourCC = 0 (no DX10 extension)
                w.u32(RGBA8_BIT_COUNT); // dwRGBBitCount = 32
                w.u32(RGBA8_R_MASK); // dwRBitMask = 0x000000ff
                w.u32(RGBA8_G_MASK); // dwGBitMask = 0x0000ff00
                w.u32(RGBA8_B_MASK); // dwBBitMask = 0x00ff0000
                w.u32(RGBA8_A_MASK); // dwABitMask = 0xff000000
            }
        }
        // caps
        w.u32(DDSCAPS_TEXTURE); // dwCaps
        w.u32(0); // dwCaps2
        w.u32(0); // dwCaps3
        w.u32(0); // dwCaps4
        w.u32(0); // dwReserved2

        match mode {
            DdsHeaderMode::Dx10 => {
                // --- DDS_HEADER_DXT10 (20 bytes) ---
                w.u32(DXGI_FORMAT_R8G8B8A8_UNORM); // dxgiFormat = 28
                w.u32(D3D10_RESOURCE_DIMENSION_TEXTURE2D); // resourceDimension = 3
                w.u32(0); // miscFlag
                w.u32(1); // arraySize
                w.u32(0); // miscFlags2
                debug_assert_eq!(w.pos(), DDS_PIXEL_DATA_OFFSET, "DDS header layout drift");
            }
            DdsHeaderMode::LegacyRgba8 => {
                // No DDS_HEADER_DXT10: pixels follow the 124-byte header.
                debug_assert_eq!(
                    w.pos(),
                    DDS_LEGACY_PIXEL_DATA_OFFSET,
                    "legacy DDS header layout drift"
                );
            }
        }

        // --- pixel data (single mip) ---
        w.bytes(&self.pixels);
        w.buf
    }

    /// Parse a Tier-0 DDS blob back into a [`DdsImage`]. Accepts **both** header
    /// forms emitted by [`DdsImage::to_dds_bytes_with`]:
    ///
    /// * [`DdsHeaderMode::Dx10`]: requires the `DX10` four-CC and a
    ///   `DDS_HEADER_DXT10` with `dxgiFormat == 28`.
    /// * [`DdsHeaderMode::LegacyRgba8`]: requires `DDPF_RGB` with the exact
    ///   32-bpp R8G8B8A8 channel masks and no `DDS_HEADER_DXT10`.
    ///
    /// Either way it then slices exactly `width * height * 4` pixel bytes
    /// (single mip).
    pub fn parse(data: &[u8]) -> Result<DdsImage, TpfError> {
        let mut r = LeReader::new(data);

        let magic = r.array4()?;
        if u32::from_le_bytes(magic) != DDS_MAGIC {
            return Err(TpfError::BadDdsMagic(magic));
        }

        // DDS_HEADER
        let dw_size = r.u32()?;
        if dw_size != DDS_HEADER_SIZE {
            return Err(TpfError::BadDdsHeaderSize(dw_size));
        }
        let _dw_flags = r.u32()?;
        let height = r.u32()?;
        let width = r.u32()?;
        let _pitch = r.u32()?;
        let _depth = r.u32()?;
        let _mips = r.u32()?;
        r.skip(44)?; // dwReserved1[11]
        // DDS_PIXELFORMAT
        let pf_size = r.u32()?;
        if pf_size != DDS_PIXELFORMAT_SIZE {
            return Err(TpfError::BadPixelFormatSize(pf_size));
        }
        let pf_flags = r.u32()?;
        let fourcc = r.u32()?;
        let rgb_bits = r.u32()?;
        let r_mask = r.u32()?;
        let g_mask = r.u32()?;
        let b_mask = r.u32()?;
        let a_mask = r.u32()?;
        let _caps = r.u32()?;
        r.skip(16)?; // dwCaps2/3/4 + dwReserved2

        if pf_flags & DDPF_FOURCC != 0 {
            // DX10 form: a DDS_HEADER_DXT10 follows the DDS_HEADER.
            if fourcc != FOURCC_DX10 {
                return Err(TpfError::MissingDxt10Header);
            }
            let dxgi = r.u32()?;
            let _dim = r.u32()?;
            let _misc = r.u32()?;
            let _array = r.u32()?;
            let _misc2 = r.u32()?;
            if dxgi != DXGI_FORMAT_R8G8B8A8_UNORM {
                return Err(TpfError::UnsupportedDxgiFormat(dxgi));
            }
        } else if pf_flags & DDPF_RGB != 0 {
            // Legacy form: the channel masks must describe exactly R8G8B8A8, and
            // pixel data follows the 124-byte header with no DXT10 extension.
            if rgb_bits != RGBA8_BIT_COUNT
                || r_mask != RGBA8_R_MASK
                || g_mask != RGBA8_G_MASK
                || b_mask != RGBA8_B_MASK
                || a_mask != RGBA8_A_MASK
            {
                return Err(TpfError::UnsupportedLegacyPixelFormat {
                    rgb_bits,
                    r_mask,
                    g_mask,
                    b_mask,
                    a_mask,
                });
            }
        } else {
            // Neither a DX10 four-CC nor an RGB-masked legacy surface.
            return Err(TpfError::MissingDxt10Header);
        }

        let expected = (width as usize) * (height as usize) * RGBA8_BYTES_PER_PIXEL;
        let pixels = r.take(expected)?.to_vec();
        if r.remaining() != 0 {
            return Err(TpfError::TrailingDdsBytes {
                remaining: r.remaining(),
            });
        }

        Ok(DdsImage {
            width,
            height,
            pixels,
        })
    }
}

// ===========================================================================
// Tier 1 -- TPF003 (PC) container wrap
// ===========================================================================

/// One texture entry inside a [`Tpf`]: a name plus the raw (uncompressed) DDS
/// payload and the PC entry's small format/type/mip/flags bytes. The DDS bytes
/// are stored opaquely so the TPF round-trip preserves them exactly (decode them
/// with [`DdsImage::parse`] if the typed image is wanted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TpfTexture {
    /// Texture name (no path/extension; the DDS payload is implicit). This is a
    /// caller-set, per-entry field: the game's in-memory TPF upload derives the
    /// `GLOBAL_TexRepository` (SYSTEX) key from this very string, so it is the
    /// repository key the runtime binds to. Set it to the key the engine should
    /// resolve. Written verbatim at the entry's `nameOffset` (see
    /// [`Tpf::single_pc`] for the one-entry convenience constructor).
    pub name: String,
    /// The TPF `format` byte (see [`TPF_FORMAT_R8G8B8A8_UNORM`]).
    pub format: u8,
    /// `TexType` byte (see [`TEX_TYPE_TEXTURE`]).
    pub tex_type: u8,
    /// Mip count declared in the entry. For a Tier-0 single-mip DDS this is 1.
    pub mip_count: u8,
    /// The entry `flags1` byte (SoulsFormats asserts `0..=3`). Stored verbatim.
    pub flags1: u8,
    /// The raw, uncompressed DDS blob (typically [`DdsImage::to_dds_bytes`]).
    pub dds: Vec<u8>,
}

/// An uncompressed TPF003 / PC container holding one or more [`TpfTexture`]s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tpf {
    /// `TPFPlatform` byte. Only [`TPF_PLATFORM_PC`] is built/parsed here.
    pub platform: u8,
    /// `Flag2` byte (documented-uncertain; round-tripped verbatim).
    pub flag2: u8,
    /// `Encoding` byte (Shift-JIS `0`/`2`, or UTF-16 `1`).
    pub encoding: u8,
    pub textures: Vec<TpfTexture>,
}

impl Tpf {
    /// Convenience constructor: a one-texture PC container with the default
    /// flag2/encoding and a `R8G8B8A8` format byte.
    pub fn single_pc(name: impl Into<String>, dds: Vec<u8>, mip_count: u8) -> Self {
        Tpf {
            platform: TPF_PLATFORM_PC,
            flag2: TPF_DEFAULT_FLAG2,
            encoding: TPF_ENCODING_SHIFT_JIS,
            textures: vec![TpfTexture {
                name: name.into(),
                format: TPF_FORMAT_R8G8B8A8_UNORM,
                tex_type: TEX_TYPE_TEXTURE,
                mip_count,
                flags1: 0,
                dds,
            }],
        }
    }

    /// Serialize the uncompressed TPF003 (PC) blob.
    ///
    /// Layout: a 16-byte header, then a `TPF_PC_ENTRY_SIZE`-byte entry per
    /// texture (the entry table begins at `+0x10` because the `extFlag` byte at
    /// `+0x0F` is kept clear), then -- referenced by the absolute offsets stored
    /// in each entry -- the per-texture name string followed by its DDS payload.
    pub fn build(&self) -> Result<Vec<u8>, TpfError> {
        let entry_table_end = TPF_HEADER_SIZE + self.textures.len() * TPF_PC_ENTRY_SIZE;

        // First pass: resolve each texture's name and data absolute offsets.
        let mut cursor = entry_table_end;
        let mut name_bytes: Vec<Vec<u8>> = Vec::with_capacity(self.textures.len());
        let mut name_offsets: Vec<usize> = Vec::with_capacity(self.textures.len());
        let mut data_offsets: Vec<usize> = Vec::with_capacity(self.textures.len());
        for tex in &self.textures {
            let nb = encode_name(&tex.name, self.encoding)?;
            name_offsets.push(cursor);
            cursor += nb.len();
            name_bytes.push(nb);
            data_offsets.push(cursor);
            cursor += tex.dds.len();
        }

        let total_texture_data: u32 = self.textures.iter().map(|t| t.dds.len() as u32).sum();

        let mut w = LeWriter::new();
        // --- TPF header (16 bytes) ---
        w.bytes(&TPF_MAGIC); // "TPF\0"
        w.u32(total_texture_data); // totalTextureDataSize
        w.u32(self.textures.len() as u32); // fileCount
        w.u8(self.platform); // platform (PC = 0)
        w.u8(self.flag2); // flag2
        w.u8(self.encoding); // encoding
        w.u8(0); // reserved / extFlag -- bit0 CLEAR

        // --- texture entries (PC layout, TPF_PC_ENTRY_SIZE each) ---
        for (i, tex) in self.textures.iter().enumerate() {
            w.u32(data_offsets[i] as u32); // dataOffset
            w.u32(tex.dds.len() as u32); // dataSize
            w.u8(tex.format); // format
            w.u8(tex.tex_type); // type
            w.u8(tex.mip_count); // mipCount
            w.u8(tex.flags1); // flags1
            w.u32(name_offsets[i] as u32); // nameOffset
            w.u32(0); // hasFloatStruct = 0 (no FloatStruct trailer)
        }
        debug_assert_eq!(w.pos(), entry_table_end, "TPF entry table layout drift");

        // --- per-texture name string then DDS payload ---
        for (i, tex) in self.textures.iter().enumerate() {
            w.bytes(&name_bytes[i]);
            w.bytes(&tex.dds);
        }
        debug_assert_eq!(w.pos(), cursor, "TPF body layout drift");

        Ok(w.buf)
    }

    /// Parse an uncompressed TPF003 (PC) blob into typed fields. Enforces the
    /// Tier-1 self-consistency gate: only PC platform, no `FloatStruct` trailer,
    /// every `dataOffset + dataSize` / `nameOffset` in range, and
    /// `totalTextureDataSize == sum of dataSize`.
    pub fn parse(data: &[u8]) -> Result<Tpf, TpfError> {
        let mut r = LeReader::new(data);

        let magic = r.array4()?;
        if magic != TPF_MAGIC {
            return Err(TpfError::BadTpfMagic(magic));
        }
        let total_texture_data = r.u32()?;
        let file_count = r.u32()? as usize;
        let platform = r.u8()?;
        let flag2 = r.u8()?;
        let encoding = r.u8()?;
        let _reserved = r.u8()?;

        if platform != TPF_PLATFORM_PC {
            return Err(TpfError::UnsupportedPlatform(platform));
        }

        let mut textures = Vec::with_capacity(file_count);
        let mut computed_total: u32 = 0;
        for _ in 0..file_count {
            let data_offset = r.u32()? as usize;
            let data_size = r.u32()? as usize;
            let format = r.u8()?;
            let tex_type = r.u8()?;
            let mip_count = r.u8()?;
            let flags1 = r.u8()?;
            let name_offset = r.u32()? as usize;
            let has_float = r.u32()?;
            if has_float != 0 {
                return Err(TpfError::FloatStructUnsupported(has_float));
            }

            if data_offset + data_size > data.len() {
                return Err(TpfError::OffsetOutOfRange {
                    context: "texture data",
                    offset: data_offset,
                    size: data_size,
                    blob_len: data.len(),
                });
            }
            if name_offset >= data.len() {
                return Err(TpfError::OffsetOutOfRange {
                    context: "texture name",
                    offset: name_offset,
                    size: 0,
                    blob_len: data.len(),
                });
            }

            let dds = data[data_offset..data_offset + data_size].to_vec();
            let name = decode_name(data, name_offset, encoding)?;
            computed_total = computed_total.wrapping_add(data_size as u32);

            textures.push(TpfTexture {
                name,
                format,
                tex_type,
                mip_count,
                flags1,
                dds,
            });
        }

        if computed_total != total_texture_data {
            return Err(TpfError::TotalSizeMismatch {
                declared: total_texture_data,
                computed: computed_total,
            });
        }

        Ok(Tpf {
            platform,
            flag2,
            encoding,
            textures,
        })
    }
}
