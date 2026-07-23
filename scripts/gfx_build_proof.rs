//! Standalone, dependency-free proof that the Elden Ring title-cover magenta GFX
//! asset (`TITLE_MINIMAL_MAGENTA_GFX` in `crates/er-effects-rs/src/constants.rs`) can be reconstructed
//! from typed Rust structs instead of an opaque byte literal.
//!
//! The GFX container is an uncompressed Scaleform file: an SWF-derived on-disk
//! format. Header = `'G' 'F' 'X' version(=0x0b) FileLength(u32 LE)`, followed by a
//! movie body: bit-packed RECT bounds, frameRate (fixed8.8), frameCount (u16), then
//! a flat list of tags `(code<<6)|len` with a `len>=0x3f` u32 long-form escape.
//!
//! Build:  rustc -O scripts/gfx_build_proof.rs -o <out_binary>
//! Run:    <out_binary> <path-to-original-magenta.gfx> <path-to-write-built.gfx>
//!
//! Byte-identity with FFDEC's output is NOT the goal (FFDEC force-encodes some tag
//! headers in long form, an encoder quirk). The goal is a VALID, functionally
//! equivalent GFX with the same tag tree the game's Scaleform parser would read.

use std::env;
use std::fs;

// ----------------------------------------------------------------------------
// Bit-level writer for RECT and SHAPE records (MSB-first, like SWF/GFX).
// ----------------------------------------------------------------------------
struct BitWriter {
    bytes: Vec<u8>,
    bit_buf: u8,
    bit_count: u8, // number of bits currently filled in bit_buf (0..8)
}

impl BitWriter {
    fn new() -> Self {
        BitWriter { bytes: Vec::new(), bit_buf: 0, bit_count: 0 }
    }

    fn write_bit(&mut self, b: u32) {
        self.bit_buf |= ((b & 1) as u8) << (7 - self.bit_count);
        self.bit_count += 1;
        if self.bit_count == 8 {
            self.bytes.push(self.bit_buf);
            self.bit_buf = 0;
            self.bit_count = 0;
        }
    }

    fn write_ubits(&mut self, value: u32, nbits: u32) {
        for i in (0..nbits).rev() {
            self.write_bit((value >> i) & 1);
        }
    }

    fn write_sbits(&mut self, value: i32, nbits: u32) {
        let mask: u32 = if nbits >= 32 { 0xFFFF_FFFF } else { (1u32 << nbits) - 1 };
        self.write_ubits((value as u32) & mask, nbits);
    }

    /// Flush any partial byte (zero-padded), as SWF/GFX does between byte-aligned fields.
    fn align(&mut self) {
        if self.bit_count > 0 {
            self.bytes.push(self.bit_buf);
            self.bit_buf = 0;
            self.bit_count = 0;
        }
    }

    fn into_bytes(mut self) -> Vec<u8> {
        self.align();
        self.bytes
    }
}

/// Minimum bits to hold `v` as a signed two's-complement value (>=1, includes sign bit).
fn sbits_needed(v: i32) -> u32 {
    let mut n: u32 = 1;
    loop {
        let lo: i64 = -(1i64 << (n - 1));
        let hi: i64 = (1i64 << (n - 1)) - 1;
        if (v as i64) >= lo && (v as i64) <= hi {
            return n;
        }
        n += 1;
        if n >= 32 {
            return 32;
        }
    }
}

// ----------------------------------------------------------------------------
// Typed structs for the GFX content.
// ----------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

/// SWF 8.8 fixed-point frame rate, serialized LE as [fraction, integer].
#[derive(Clone, Copy)]
struct Fixed8 {
    integer: u8,
    fraction: u8,
}
impl Fixed8 {
    fn from_fps(fps: u8) -> Self {
        Fixed8 { integer: fps, fraction: 0 }
    }
    fn to_le_bytes(self) -> [u8; 2] {
        [self.fraction, self.integer]
    }
}

/// Bit-packed twips rectangle. Nbits is auto-chosen from the widest field.
#[derive(Clone, Copy)]
struct Rect {
    x_min: i32,
    x_max: i32,
    y_min: i32,
    y_max: i32,
}
impl Rect {
    fn write(&self, bw: &mut BitWriter) {
        let nbits = [self.x_min, self.x_max, self.y_min, self.y_max]
            .iter()
            .map(|&v| sbits_needed(v))
            .max()
            .unwrap();
        bw.write_ubits(nbits, 5);
        bw.write_sbits(self.x_min, nbits);
        bw.write_sbits(self.x_max, nbits);
        bw.write_sbits(self.y_min, nbits);
        bw.write_sbits(self.y_max, nbits);
        bw.align(); // RECT is byte-aligned; next field starts on a byte boundary.
    }
}

#[derive(Clone, Copy)]
enum FillStyle {
    /// type 0x00 = solid fill; DefineShape (tag 2) uses RGB (no alpha).
    Solid(Rgb),
}
impl FillStyle {
    fn write(&self, out: &mut Vec<u8>) {
        match self {
            FillStyle::Solid(c) => {
                out.push(0x00);
                out.extend_from_slice(&[c.r, c.g, c.b]);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct LineStyle; // none used here; kept for completeness.

#[derive(Clone, Copy)]
enum ShapeRecord {
    /// STYLECHANGERECORD: optional absolute move + optional style selections.
    StyleChange {
        move_to: Option<(i32, i32)>,
        fill_style0: Option<u32>,
        fill_style1: Option<u32>,
        line_style: Option<u32>,
    },
    /// STRAIGHTEDGERECORD. One delta zero => axis-aligned (GeneralLine=0); else general.
    StraightEdge { dx: i32, dy: i32 },
    /// ENDSHAPERECORD (6 zero bits).
    End,
}

impl ShapeRecord {
    fn write(&self, bw: &mut BitWriter, num_fill_bits: u32, num_line_bits: u32) {
        match *self {
            ShapeRecord::StyleChange { move_to, fill_style0, fill_style1, line_style } => {
                bw.write_bit(0); // TypeFlag = 0 -> non-edge
                let state_new_styles = 0u32;
                let state_line_style = line_style.is_some() as u32;
                let state_fill1 = fill_style1.is_some() as u32;
                let state_fill0 = fill_style0.is_some() as u32;
                let state_move = move_to.is_some() as u32;
                bw.write_bit(state_new_styles);
                bw.write_bit(state_line_style);
                bw.write_bit(state_fill1);
                bw.write_bit(state_fill0);
                bw.write_bit(state_move);
                if let Some((dx, dy)) = move_to {
                    let move_bits = sbits_needed(dx).max(sbits_needed(dy));
                    bw.write_ubits(move_bits, 5);
                    bw.write_sbits(dx, move_bits);
                    bw.write_sbits(dy, move_bits);
                }
                // SWF order: FillStyle0, FillStyle1, LineStyle.
                if let Some(f0) = fill_style0 {
                    bw.write_ubits(f0, num_fill_bits);
                }
                if let Some(f1) = fill_style1 {
                    bw.write_ubits(f1, num_fill_bits);
                }
                if let Some(ls) = line_style {
                    bw.write_ubits(ls, num_line_bits);
                }
            }
            ShapeRecord::StraightEdge { dx, dy } => {
                bw.write_bit(1); // TypeFlag = 1 -> edge
                bw.write_bit(1); // StraightFlag = 1
                if dx != 0 && dy != 0 {
                    let nb = sbits_needed(dx).max(sbits_needed(dy)).max(2);
                    bw.write_ubits(nb - 2, 4);
                    bw.write_bit(1); // GeneralLineFlag
                    bw.write_sbits(dx, nb);
                    bw.write_sbits(dy, nb);
                } else if dy == 0 {
                    let nb = sbits_needed(dx).max(2);
                    bw.write_ubits(nb - 2, 4);
                    bw.write_bit(0); // GeneralLineFlag = 0
                    bw.write_bit(0); // VertLineFlag = 0 -> horizontal (DeltaX)
                    bw.write_sbits(dx, nb);
                } else {
                    let nb = sbits_needed(dy).max(2);
                    bw.write_ubits(nb - 2, 4);
                    bw.write_bit(0); // GeneralLineFlag = 0
                    bw.write_bit(1); // VertLineFlag = 1 -> vertical (DeltaY)
                    bw.write_sbits(dy, nb);
                }
            }
            ShapeRecord::End => {
                // TypeFlag(0) + 5 state flags all 0 = 6 zero bits.
                for _ in 0..6 {
                    bw.write_bit(0);
                }
            }
        }
        // num_fill_bits/num_line_bits unused for some records; silence dead reads.
        let _ = (num_fill_bits, num_line_bits);
    }
}

enum Tag {
    /// GFX tag 1000: exporter metadata + the movie ("swf") name.
    ExporterInfo {
        version: u16,
        flags: u32,
        field_a: u16,
        field_b: u8,
        swf_name: String,
    },
    /// SWF tag 69.
    FileAttributes(u32),
    /// SWF tag 9.
    SetBackgroundColor(Rgb),
    /// SWF tag 2.
    DefineShape {
        id: u16,
        bounds: Rect,
        fills: Vec<FillStyle>,
        lines: Vec<LineStyle>,
        records: Vec<ShapeRecord>,
    },
    /// SWF tag 26.
    PlaceObject2 {
        depth: u16,
        character_id: u16,
        matrix: Matrix,
    },
    /// SWF tag 1.
    ShowFrame,
    /// SWF tag 0.
    End,
}

/// 2x3 affine matrix. Only the identity case is exercised here (1 byte 0x00).
#[derive(Clone, Copy)]
struct Matrix {
    identity: bool,
}
impl Matrix {
    fn identity() -> Self {
        Matrix { identity: true }
    }
    fn write(&self, out: &mut Vec<u8>) {
        if self.identity {
            // HasScale=0, HasRotate=0, NTranslateBits=0 -> 7 zero bits -> 1 byte.
            out.push(0x00);
        } else {
            unimplemented!("only identity matrix is needed for this proof");
        }
    }
}

impl Tag {
    fn code(&self) -> u16 {
        match self {
            Tag::ExporterInfo { .. } => 1000,
            Tag::FileAttributes(_) => 69,
            Tag::SetBackgroundColor(_) => 9,
            Tag::DefineShape { .. } => 2,
            Tag::PlaceObject2 { .. } => 26,
            Tag::ShowFrame => 1,
            Tag::End => 0,
        }
    }

    /// Serialize just the tag body (no header).
    fn body(&self) -> Vec<u8> {
        let mut b = Vec::new();
        match self {
            Tag::ExporterInfo { version, flags, field_a, field_b, swf_name } => {
                b.extend_from_slice(&version.to_le_bytes());
                b.extend_from_slice(&flags.to_le_bytes());
                b.extend_from_slice(&field_a.to_le_bytes());
                b.push(*field_b);
                let name = swf_name.as_bytes();
                b.push(name.len() as u8); // u8 length prefix
                b.extend_from_slice(name);
                b.push(0x00); // string null terminator
                b.push(0x00); // trailing field
            }
            Tag::FileAttributes(flags) => {
                b.extend_from_slice(&flags.to_le_bytes());
            }
            Tag::SetBackgroundColor(c) => {
                b.extend_from_slice(&[c.r, c.g, c.b]);
            }
            Tag::DefineShape { id, bounds, fills, lines, records } => {
                b.extend_from_slice(&id.to_le_bytes());
                // SHAPE bounds RECT (byte aligned).
                let mut bw = BitWriter::new();
                bounds.write(&mut bw);
                b.extend_from_slice(&bw.into_bytes());
                // Fill style array.
                write_style_count(&mut b, fills.len());
                for f in fills {
                    f.write(&mut b);
                }
                // Line style array.
                write_style_count(&mut b, lines.len());
                // (no LineStyle bodies; lines is empty for this proof)
                // NumFillBits / NumLineBits.
                let num_fill_bits = bits_for_count(fills.len());
                let num_line_bits = bits_for_count(lines.len());
                b.push(((num_fill_bits as u8) << 4) | (num_line_bits as u8));
                // Shape records (bit-packed, then byte-aligned).
                let mut sbw = BitWriter::new();
                for r in records {
                    r.write(&mut sbw, num_fill_bits, num_line_bits);
                }
                b.extend_from_slice(&sbw.into_bytes());
            }
            Tag::PlaceObject2 { depth, character_id, matrix } => {
                // PlaceFlags: HasCharacter(bit1) | HasMatrix(bit2).
                let flags: u8 = 0b0000_0110;
                b.push(flags);
                b.extend_from_slice(&depth.to_le_bytes());
                b.extend_from_slice(&character_id.to_le_bytes());
                matrix.write(&mut b);
            }
            Tag::ShowFrame | Tag::End => {}
        }
        b
    }

    /// Serialize header + body. Header is `(code<<6)|len`; if len>=0x3f, emit
    /// the 0x3f escape followed by the real length as u32 LE.
    fn write(&self, out: &mut Vec<u8>) {
        let body = self.body();
        write_tag_header(out, self.code(), body.len());
        out.extend_from_slice(&body);
    }
}

/// SWF style-array count: a single byte unless >=0xFF, then 0xFF + u16 LE extended count.
fn write_style_count(out: &mut Vec<u8>, n: usize) {
    if n >= 0xFF {
        out.push(0xFF);
        out.extend_from_slice(&(n as u16).to_le_bytes());
    } else {
        out.push(n as u8);
    }
}

/// Bits needed to index `count` styles (indices are 1-based; index `count` must fit).
fn bits_for_count(count: usize) -> u32 {
    if count == 0 {
        return 0;
    }
    let mut bits = 0u32;
    let mut max = count as u32; // highest 1-based index
    while max > 0 {
        bits += 1;
        max >>= 1;
    }
    bits
}

fn write_tag_header(out: &mut Vec<u8>, code: u16, len: usize) {
    if len >= 0x3f {
        let hdr: u16 = (code << 6) | 0x3f;
        out.extend_from_slice(&hdr.to_le_bytes());
        out.extend_from_slice(&(len as u32).to_le_bytes());
    } else {
        let hdr: u16 = (code << 6) | (len as u16);
        out.extend_from_slice(&hdr.to_le_bytes());
    }
}

struct GfxFile {
    version: u8,
    bounds: Rect,
    frame_rate: Fixed8,
    frame_count: u16,
    tags: Vec<Tag>,
}

impl GfxFile {
    fn serialize(&self) -> Vec<u8> {
        // Movie body: RECT + frameRate + frameCount + tags.
        let mut body = Vec::new();
        let mut bw = BitWriter::new();
        self.bounds.write(&mut bw);
        body.extend_from_slice(&bw.into_bytes());
        body.extend_from_slice(&self.frame_rate.to_le_bytes());
        body.extend_from_slice(&self.frame_count.to_le_bytes());
        for t in &self.tags {
            t.write(&mut body);
        }

        // Header: 'G' 'F' 'X' version FileLength(u32 LE). FileLength = whole file.
        let mut out = Vec::new();
        out.extend_from_slice(b"GFX");
        out.push(self.version);
        out.extend_from_slice(&0u32.to_le_bytes()); // FileLength placeholder
        out.extend_from_slice(&body);

        // Back-patch FileLength == total file length.
        let total = out.len() as u32;
        out[4..8].copy_from_slice(&total.to_le_bytes());
        out
    }
}

fn build_magenta() -> GfxFile {
    // 1920x1080 in twips (x20).
    let full = Rect { x_min: 0, x_max: 38400, y_min: 0, y_max: 21600 };
    let magenta = Rgb { r: 0xFF, g: 0x00, b: 0xFF };

    // Full-screen rectangle traced from the bottom-right corner, fill style 1.
    // (Same path FFDEC chose: BR -> BL -> TL -> TR -> BR.)
    let shape_records = vec![
        ShapeRecord::StyleChange {
            move_to: Some((38400, 21600)),
            fill_style0: None,
            fill_style1: Some(1),
            line_style: None,
        },
        ShapeRecord::StraightEdge { dx: -38400, dy: 0 }, // bottom edge, right->left
        ShapeRecord::StraightEdge { dx: 0, dy: -21600 }, // left edge, bottom->top
        ShapeRecord::StraightEdge { dx: 38400, dy: 0 },  // top edge, left->right
        ShapeRecord::StraightEdge { dx: 0, dy: 21600 },  // right edge, top->bottom (close)
        ShapeRecord::End,
    ];

    GfxFile {
        version: 0x0b,
        bounds: full,
        frame_rate: Fixed8::from_fps(30),
        frame_count: 1,
        tags: vec![
            Tag::ExporterInfo {
                version: 0x0401,
                flags: 0,
                field_a: 0x000d,
                field_b: 0x00,
                swf_name: "er_effects_title_cover".to_string(),
            },
            Tag::FileAttributes(0x08),
            Tag::SetBackgroundColor(magenta),
            Tag::DefineShape {
                id: 1,
                bounds: full,
                fills: vec![FillStyle::Solid(magenta)],
                lines: vec![],
                records: shape_records,
            },
            Tag::PlaceObject2 {
                depth: 1,
                character_id: 1,
                matrix: Matrix::identity(),
            },
            Tag::ShowFrame,
            Tag::End,
        ],
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let original_path = args.get(1).cloned().unwrap_or_default();
    let out_path = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "minimal_magenta.built.gfx".to_string());

    let gfx = build_magenta();
    let bytes = gfx.serialize();

    fs::write(&out_path, &bytes).expect("write built gfx");

    // Structural sanity checks.
    let magic_ok = &bytes[0..3] == b"GFX";
    let version_ok = bytes[3] == 0x0b;
    let filelen_field = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let filelen_ok = filelen_field as usize == bytes.len();
    let ends_with_end = bytes[bytes.len() - 2..] == [0x00, 0x00];

    println!("emitted_path {}", out_path);
    println!("emitted_len {}", bytes.len());
    println!("magic_GFX {}", magic_ok);
    println!("version_0x0b {}", version_ok);
    println!("FileLength_field {}", filelen_field);
    println!("FileLength_matches_len {}", filelen_ok);
    println!("ends_with_End_tag {}", ends_with_end);

    if !original_path.is_empty() {
        match fs::read(&original_path) {
            Ok(orig) => {
                println!("original_len {}", orig.len());
                let identical = orig == bytes;
                println!("byte_identical {}", identical);
                if !identical {
                    // Report first divergence for the record.
                    let n = orig.len().min(bytes.len());
                    let mut first = None;
                    for i in 0..n {
                        if orig[i] != bytes[i] {
                            first = Some(i);
                            break;
                        }
                    }
                    match first {
                        Some(i) => println!(
                            "first_divergence_offset {} orig=0x{:02x} built=0x{:02x}",
                            i, orig[i], bytes[i]
                        ),
                        None => println!(
                            "first_divergence_offset {} (length-only difference)",
                            n
                        ),
                    }
                }
            }
            Err(e) => println!("could not read original {}: {}", original_path, e),
        }
    }

    assert!(magic_ok, "magic must be GFX");
    assert!(version_ok, "version must be 0x0b");
    assert!(filelen_ok, "FileLength must equal actual length");
    assert!(ends_with_end, "file must end with End tag (0x00 0x00)");
    println!("ALL_STRUCTURAL_CHECKS_PASSED");
}
