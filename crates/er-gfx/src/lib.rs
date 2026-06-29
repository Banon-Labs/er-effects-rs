//! Tier-0/Tier-1 lossless codec for uncompressed Scaleform **GFX** movies (the
//! `.gfx` files shipping in Elden Ring's `menu/` tree, magic `b"GFX"`, version
//! `0x0b`).
//!
//! # Goal
//!
//! Read ANY such `.gfx` and re-serialize it **byte-for-byte identical**. We do
//! that by structurally modelling the file header, the `DefineSprite` (code 39)
//! nesting, the `End` (code 0) terminator, plus a growing set of **Tier-1**
//! "trivial" tags that carry no bitstream and re-encode losslessly from typed
//! fields (see [`Tag`]). Every other tag is still treated as opaque
//! [`Tag::Unknown`] whose body bytes are re-emitted verbatim. Tag *lengths* are
//! always recomputed by the writer (never copied from the source), so
//! structurally-derived fields (FileLength, every `RecordHeader`) are
//! regenerated rather than echoed.
//!
//! # Tier-1 typed tags
//!
//! The promoted tags below are re-encoded **field-by-field** (not from a stored
//! raw copy); each was proven byte-identical across the full 114-file corpus
//! before promotion. Each typed variant carries its own [`force_long`](Tag) bit
//! so the exact `RecordHeader` form is reproduced (the GFX exporter is not
//! length-deterministic; see below). String fields are stored decoded but are
//! always re-emitted with their terminating NUL, and variable tags assert they
//! consume their declared body exactly so any future structural divergence
//! fails loudly at parse rather than silently producing wrong bytes.
//!
//! # RecordHeader long/short form decision (load-bearing for byte-identity)
//!
//! A `RecordHeader` is a little-endian `u16` where `code = word >> 6` and
//! `len = word & 0x3f`. When `len == 0x3f`, a `u32` "long" length follows.
//! Short form can encode body lengths `0..=0x3e`; long form can encode any
//! length but is *mandatory* only for lengths `>= 0x3f`.
//!
//! Measured over the real corpus, the exporter is **not** length-deterministic:
//! 14,766 tags use the long form even though their body is `<= 0x3e` (e.g. tag
//! codes 26 `PlaceObject2` and 70 `PlaceObject3` appear in BOTH forms with the
//! same small length, so the choice is not even per-tag-code). To guarantee
//! byte-identity we therefore record a per-tag [`force_long`](Tag) bit at parse
//! time and reproduce the exact form on write. We never shorten a source's
//! needlessly-long header. This is option (a) from the task brief.
//!
//! The `End` tag (code 0) is always short (`0x0000`) across the entire corpus;
//! we encode it as such and reject a long-form End as malformed so a regression
//! would fail loudly rather than silently diverge.

use std::fmt;

/// Tag code for `DefineSprite`. Its body is `spriteId: u16`, `frameCount: u16`,
/// then a NESTED tag stream parsed with the same parser and terminated by its
/// own `End(0)`.
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
/// Short-form length sentinel: `len == 0x3f` means a `u32` long length follows.
const LONG_LEN_SENTINEL: u16 = 0x3f;
/// Maximum tag code representable in a `RecordHeader` (`u16 >> 6`).
const MAX_TAG_CODE: u16 = 0x3ff;
/// Expected file magic.
const MAGIC: [u8; 3] = *b"GFX";

/// File header preceding the tag stream.
///
/// The movie bounds `RECT` is bit-packed; to guarantee byte-identity without a
/// full bit-level RECT re-encoder, we capture its exact bytes verbatim in
/// [`Header::movie_rect_raw`] (we only parse its bit length to know how many
/// bytes to slice) and never re-encode it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// File format version (observed: `0x0b`).
    pub version: u8,
    /// The exact bit-packed movie `RECT` bytes, preserved verbatim.
    pub movie_rect_raw: Vec<u8>,
    /// Frame rate (8.8 fixed-point, stored as raw `u16`).
    pub frame_rate: u16,
    /// Declared frame count.
    pub frame_count: u16,
}

/// A single tag in a (possibly nested) tag stream.
///
/// Every variant that carries a body also carries a [`force_long`](Tag) bit that
/// records whether the source used the long `RecordHeader` form so the writer
/// can reproduce it exactly (see module docs).
///
/// `Eq` is intentionally **not** derived because [`Tag::CsmTextSettings`] holds
/// `f32` fields; equality is still available via the derived [`PartialEq`].
#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    /// `DefineSprite` (code 39): a sprite id, frame count, and a nested tag
    /// stream (which includes its own terminating [`Tag::End`]).
    DefineSprite {
        id: u16,
        frame_count: u16,
        tags: Vec<Tag>,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `End` (code 0): terminates the enclosing tag stream. Always short form.
    End,

    // --- Tier-1 typed tags (re-encoded from fields; see module docs). ---
    /// `ShowFrame` (code 1): advance the timeline one frame. Empty body.
    ShowFrame {
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `SetBackgroundColor` (code 9): the movie background `RGB` triple.
    SetBackgroundColor {
        r: u8,
        g: u8,
        b: u8,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `RemoveObject2` (code 28): remove the object at `depth`.
    RemoveObject2 {
        depth: u16,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `FileAttributes` (code 69): a flags word. The bits are stored raw and not
    /// interpreted (preserving any reserved/unknown bits exactly).
    FileAttributes {
        flags: u32,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `Metadata` (code 77): a single NUL-terminated string (typically the XMP
    /// RDF packet). The terminating NUL is implicit and always re-emitted.
    Metadata {
        xml: String,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `FrameLabel` (code 43): a NUL-terminated label and an OPTIONAL trailing
    /// named-anchor byte. `named_anchor` preserves the byte's presence/absence
    /// and value exactly (none present in the Tier-1 corpus, but modelled).
    FrameLabel {
        label: String,
        named_anchor: Option<u8>,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `SymbolClass` (code 76): `(tag, name)` exports. Order is preserved.
    SymbolClass {
        symbols: Vec<(u16, String)>,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `ImportAssets2` (code 71): a source `url`, 2 reserved bytes (stored raw,
    /// commonly `[0x01, 0x00]`), and `(tag, name)` imports.
    ImportAssets2 {
        url: String,
        reserved: [u8; 2],
        symbols: Vec<(u16, String)>,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
    /// `CSMTextSettings` (code 74): fixed-width font-rendering settings. The two
    /// `f32` fields are reproduced bit-exactly via [`f32::to_bits`].
    CsmTextSettings {
        character_id: u16,
        flags: u8,
        thickness: f32,
        sharpness: f32,
        reserved: u8,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },

    /// Any tag not otherwise modelled; body bytes re-emitted verbatim.
    Unknown {
        code: u16,
        raw: Vec<u8>,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
}

/// A parsed GFX movie: header plus its top-level tag stream (the top-level
/// stream includes its terminating [`Tag::End`] as its last element).
///
/// `Eq` is not derived because [`Tag`] holds `f32` fields; [`PartialEq`] is.
#[derive(Clone, Debug, PartialEq)]
pub struct Movie {
    pub header: Header,
    pub tags: Vec<Tag>,
}

/// Errors produced while parsing a GFX movie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GfxError {
    /// Ran out of input while reading: needed `need` bytes at `pos`, had `have`.
    UnexpectedEof {
        pos: usize,
        need: usize,
        have: usize,
    },
    /// File did not start with the `b"GFX"` magic.
    BadMagic([u8; 3]),
    /// An `End` tag was encoded with a non-empty / long-form body. Tier-0 treats
    /// this as malformed rather than silently round-tripping it incorrectly.
    BadEndTag { force_long: bool, len: usize },
    /// A `DefineSprite` body was shorter than its mandatory 4-byte prelude.
    SpriteBodyTooShort { len: usize },
    /// A nested tag stream overran its declared body length.
    NestedOverrun { body_end: usize, pos: usize },
    /// Tag code does not fit in a `RecordHeader` (`> 0x3ff`); cannot serialize.
    CodeTooLarge(u16),
    /// A Tier-1 string field ran to the end of its tag body without a NUL.
    UnterminatedString { code: u16 },
    /// A Tier-1 string field was not valid UTF-8.
    InvalidUtf8 { code: u16 },
    /// A Tier-1 tag body had bytes left over after structured decode (the tag's
    /// real layout differs from what Tier-1 models); kept as a hard error so a
    /// silent byte-divergence is impossible.
    TrailingTagBytes { code: u16, remaining: usize },
    /// A fixed-width Tier-1 tag body was not its expected length.
    UnexpectedTagBodyLen {
        code: u16,
        expected: usize,
        got: usize,
    },
}

impl fmt::Display for GfxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GfxError::UnexpectedEof { pos, need, have } => write!(
                f,
                "unexpected EOF: needed {need} byte(s) at offset {pos}, only {have} available"
            ),
            GfxError::BadMagic(m) => {
                write!(f, "bad magic: expected GFX, got {m:02x?}")
            }
            GfxError::BadEndTag { force_long, len } => {
                write!(f, "malformed End tag (force_long={force_long}, len={len})")
            }
            GfxError::SpriteBodyTooShort { len } => {
                write!(f, "DefineSprite body too short: {len} byte(s)")
            }
            GfxError::NestedOverrun { body_end, pos } => {
                write!(
                    f,
                    "nested tag stream overran body: pos {pos} > end {body_end}"
                )
            }
            GfxError::CodeTooLarge(c) => write!(f, "tag code {c} exceeds RecordHeader max 0x3ff"),
            GfxError::UnterminatedString { code } => {
                write!(f, "unterminated string in tag code {code}")
            }
            GfxError::InvalidUtf8 { code } => {
                write!(f, "invalid UTF-8 string in tag code {code}")
            }
            GfxError::TrailingTagBytes { code, remaining } => write!(
                f,
                "tag code {code} had {remaining} unconsumed body byte(s) after typed decode"
            ),
            GfxError::UnexpectedTagBodyLen {
                code,
                expected,
                got,
            } => write!(
                f,
                "tag code {code} body length {got} != expected {expected}"
            ),
        }
    }
}

impl std::error::Error for GfxError {}

/// Minimal forward cursor over the input bytes with bounds-checked reads.
struct GfxReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> GfxReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        GfxReader { data, pos: 0 }
    }

    fn need(&self, n: usize) -> Result<(), GfxError> {
        if self.pos + n > self.data.len() {
            Err(GfxError::UnexpectedEof {
                pos: self.pos,
                need: n,
                have: self.data.len().saturating_sub(self.pos),
            })
        } else {
            Ok(())
        }
    }

    fn read_u8(&mut self) -> Result<u8, GfxError> {
        self.need(1)?;
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    fn read_u16(&mut self) -> Result<u16, GfxError> {
        self.need(2)?;
        let v = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    fn read_u32(&mut self) -> Result<u32, GfxError> {
        self.need(4)?;
        let v = u32::from_le_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(v)
    }

    fn read_bytes(&mut self, n: usize) -> Result<Vec<u8>, GfxError> {
        self.need(n)?;
        let s = self.data[self.pos..self.pos + n].to_vec();
        self.pos += n;
        Ok(s)
    }

    /// Read a NUL-terminated string, consuming the terminator. The returned
    /// `String` excludes the NUL (re-added by the writer). Errors loudly if the
    /// data ends before a NUL or the bytes are not valid UTF-8 -- `code` names
    /// the owning tag for diagnostics.
    fn read_cstring(&mut self, code: u16) -> Result<String, GfxError> {
        let start = self.pos;
        loop {
            if self.pos >= self.data.len() {
                return Err(GfxError::UnterminatedString { code });
            }
            let byte = self.data[self.pos];
            self.pos += 1;
            if byte == 0 {
                let bytes = self.data[start..self.pos - 1].to_vec();
                return String::from_utf8(bytes).map_err(|_| GfxError::InvalidUtf8 { code });
            }
        }
    }

    /// Read the bit-packed movie `RECT` as raw bytes.
    ///
    /// Layout: top 5 bits of the first byte are `Nbits`; the field then holds
    /// `4 * Nbits` more bits (signed xmin/xmax/ymin/ymax) for `5 + 4*Nbits`
    /// total bits, byte-aligned. We only compute the byte length and slice the
    /// bytes verbatim (no bit decode), preserving them exactly.
    fn read_rect_raw(&mut self) -> Result<Vec<u8>, GfxError> {
        self.need(1)?;
        let nbits = (self.data[self.pos] >> 3) as usize;
        let total_bits = 5 + 4 * nbits;
        let byte_len = total_bits.div_ceil(8);
        self.read_bytes(byte_len)
    }
}

/// Append-only byte sink with the small write helpers the writer needs.
struct GfxWriter {
    buf: Vec<u8>,
}

impl GfxWriter {
    fn new() -> Self {
        GfxWriter { buf: Vec::new() }
    }

    fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    fn write_u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    /// Write a string followed by its NUL terminator.
    fn write_cstring(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
    }

    /// Emit a `RecordHeader` for `code`/`body_len`, honoring `force_long`.
    fn write_record_header(
        &mut self,
        code: u16,
        body_len: usize,
        force_long: bool,
    ) -> Result<(), GfxError> {
        if code > MAX_TAG_CODE {
            return Err(GfxError::CodeTooLarge(code));
        }
        if force_long || body_len >= LONG_LEN_SENTINEL as usize {
            let word = (code << 6) | LONG_LEN_SENTINEL;
            self.write_u16(word);
            self.write_u32(body_len as u32);
        } else {
            let word = (code << 6) | (body_len as u16);
            self.write_u16(word);
        }
        Ok(())
    }
}

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
        _ => Ok(Tag::Unknown {
            code,
            raw: body,
            force_long,
        }),
    }
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
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Unknown tag code 70, long form, body 2 bytes.
        let word: u16 = (70u16 << 6) | 0x3f;
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
}
