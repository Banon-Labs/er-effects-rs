//! Tier-0/Tier-1 **in-memory texture-payload builder** for Elden Ring's raster
//! pipeline. This is the raster analog of `er-gfx`'s Scaleform MemoryFile codec:
//! it emits the **uncompressed, post-Oodle-decompress** bytes the game's own
//! in-memory TPF path consumes, and it builds **bytes only** -- it never calls
//! the game, never touches disk, and never constructs a C struct.
//!
//! # Two tiers
//!
//! * **Tier 0 -- DDS encoder** ([`DdsImage`]). Encodes a `width x height` RGBA8
//!   pixel buffer into an uncompressed `R8G8B8A8_UNORM` (DXGI format `28`) DDS
//!   blob: the `DDS ` magic, the 124-byte `DDS_HEADER`, a `DDS_HEADER_DXT10`,
//!   then the raw pixel bytes (single mip). Layout follows the Microsoft DDS
//!   programming guide exactly so the byte assertions in the tests are spec
//!   citations, not guesses.
//! * **Tier 1 -- TPF003 wrap** ([`Tpf`]). Wraps one (or more) Tier-0 DDS blobs
//!   in an uncompressed TPF version-3 / PC (`TPFPlatform.PC`) container, mirroring
//!   the documented SoulsFormats `TPF` layout. The wrap is **never** Kraken/DCX
//!   compressed -- this crate emits only the decompressed in-memory form.
//!
//! # NEVER compressed
//!
//! This crate emits Kraken/DCX/Oodle data **nowhere**. The whole point is the
//! post-decompress blob; compression is a transport concern handled elsewhere.
//!
//! # Discipline (mirrors `er-gfx`)
//!
//! A small error enum ([`TpfError`]), a byte-builder plus a parser for each
//! tier, and **self round-trip tests** that assert `parse(build(x)) == x` over
//! the typed fields. Tier-0 additionally asserts exact bytes at known offsets.
//! Exact *game acceptance* of the TPF is a later runtime tier; Tier-1's gate
//! here is **self-consistency** (every offset in range, `dataOffset + dataSize`
//! within the blob, `totalTextureDataSize == sum of texture sizes`) plus the
//! typed round-trip -- not game validation.

use std::fmt;

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

    /// Encode the Tier-0 DDS blob: `DDS ` magic + 124-byte `DDS_HEADER` +
    /// 20-byte `DDS_HEADER_DXT10` + raw RGBA pixel bytes (single mip).
    pub fn to_dds_bytes(&self) -> Vec<u8> {
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
        // DDS_PIXELFORMAT (32 bytes)
        w.u32(DDS_PIXELFORMAT_SIZE); // dwSize = 32
        w.u32(DDPF_FOURCC); // dwFlags = DDPF_FOURCC
        w.u32(FOURCC_DX10); // dwFourCC = 'DX10' (DXT10 header follows)
        w.u32(0); // dwRGBBitCount (unused with DX10)
        w.u32(0); // dwRBitMask
        w.u32(0); // dwGBitMask
        w.u32(0); // dwBBitMask
        w.u32(0); // dwABitMask
        // caps
        w.u32(DDSCAPS_TEXTURE); // dwCaps
        w.u32(0); // dwCaps2
        w.u32(0); // dwCaps3
        w.u32(0); // dwCaps4
        w.u32(0); // dwReserved2

        // --- DDS_HEADER_DXT10 (20 bytes) ---
        w.u32(DXGI_FORMAT_R8G8B8A8_UNORM); // dxgiFormat = 28
        w.u32(D3D10_RESOURCE_DIMENSION_TEXTURE2D); // resourceDimension = 3
        w.u32(0); // miscFlag
        w.u32(1); // arraySize
        w.u32(0); // miscFlags2

        debug_assert_eq!(w.pos(), DDS_PIXEL_DATA_OFFSET, "DDS header layout drift");

        // --- pixel data (single mip) ---
        w.bytes(&self.pixels);
        w.buf
    }

    /// Parse a Tier-0 DDS blob back into a [`DdsImage`]. Validates the magic, the
    /// header sizes, the `DX10` four-CC, and that `dxgiFormat == 28`, then slices
    /// exactly `width * height * 4` pixel bytes (single mip).
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
        let _rgb_bits = r.u32()?;
        r.skip(16)?; // R/G/B/A bit masks
        let _caps = r.u32()?;
        r.skip(16)?; // dwCaps2/3/4 + dwReserved2

        if pf_flags & DDPF_FOURCC == 0 || fourcc != FOURCC_DX10 {
            return Err(TpfError::MissingDxt10Header);
        }

        // DDS_HEADER_DXT10
        let dxgi = r.u32()?;
        let _dim = r.u32()?;
        let _misc = r.u32()?;
        let _array = r.u32()?;
        let _misc2 = r.u32()?;
        if dxgi != DXGI_FORMAT_R8G8B8A8_UNORM {
            return Err(TpfError::UnsupportedDxgiFormat(dxgi));
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
    /// Texture name (no path/extension; the DDS payload is implicit).
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
