//! Tier-0 lossless codec for uncompressed Scaleform **GFX** movies (the `.gfx`
//! files shipping in Elden Ring's `menu/` tree, magic `b"GFX"`, version `0x0b`).
//!
//! # Goal
//!
//! Read ANY such `.gfx` and re-serialize it **byte-for-byte identical**. We do
//! that by only structurally modelling what Tier-0 actually needs -- the file
//! header, the `DefineSprite` (code 39) nesting, and the `End` (code 0)
//! terminator -- and treating every other tag as opaque [`Tag::Unknown`] whose
//! body bytes are re-emitted verbatim. Tag *lengths* are always recomputed by
//! the writer (never copied from the source), so structurally-derived fields
//! (FileLength, every `RecordHeader`) are regenerated rather than echoed.
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Any tag not otherwise modelled at Tier 0; body bytes re-emitted verbatim.
    Unknown {
        code: u16,
        raw: Vec<u8>,
        /// Whether the source encoded this tag's `RecordHeader` in long form.
        force_long: bool,
    },
}

/// A parsed GFX movie: header plus its top-level tag stream (the top-level
/// stream includes its terminating [`Tag::End`] as its last element).
#[derive(Clone, Debug, PartialEq, Eq)]
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

    fn write_u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    fn write_bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
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
            let raw = r.read_bytes(len)?;
            tags.push(Tag::Unknown {
                code,
                raw,
                force_long,
            });
        }
    }
    Ok(tags)
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
}
