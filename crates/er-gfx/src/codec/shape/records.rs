
impl LineStyle {
    const CTX: &'static str = "LINESTYLE";

    fn read(br: &mut BitReader, version: u8, rgba: bool) -> Result<LineStyle, GfxError> {
        let width = br.read_u16_aligned(Self::CTX)?;
        if version == 4 {
            let flags = br.read_u16_aligned(Self::CTX)?;
            let join = (flags >> 12) & 0x3;
            let has_fill = (flags >> 11) & 0x1 != 0;
            let miter_limit = if join == 2 {
                Some(br.read_u16_aligned(Self::CTX)?)
            } else {
                None
            };
            let fill = if has_fill {
                LineFill::Fill(Box::new(FillStyle::read(br, rgba)?))
            } else {
                let b = br.read_bytes_aligned(4, Self::CTX)?;
                LineFill::Color([b[0], b[1], b[2], b[3]])
            };
            Ok(LineStyle::Style2 {
                width,
                flags,
                miter_limit,
                fill,
            })
        } else {
            let color = Color::read(br, rgba, Self::CTX)?;
            Ok(LineStyle::Plain { width, color })
        }
    }

    fn write(&self, bw: &mut BitWriter) {
        match self {
            LineStyle::Plain { width, color } => {
                bw.write_u16_aligned(*width);
                color.write(bw);
            }
            LineStyle::Style2 {
                width,
                flags,
                miter_limit,
                fill,
            } => {
                bw.write_u16_aligned(*width);
                bw.write_u16_aligned(*flags);
                if let Some(m) = miter_limit {
                    bw.write_u16_aligned(*m);
                }
                match fill {
                    LineFill::Color(c) => bw.write_bytes_aligned(c),
                    LineFill::Fill(fs) => fs.write(bw),
                }
            }
        }
    }
}

/// A `LINESTYLEARRAY`: a count (`u8`, or `0xFF`-extended `u16` for Shape2/3/4)
/// then the line styles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineStyleArray {
    /// Whether the `0xFF`-extended `u16` count form was used.
    pub count_ext: bool,
    pub styles: Vec<LineStyle>,
}

impl LineStyleArray {
    const CTX: &'static str = "LINESTYLEARRAY";

    fn read(br: &mut BitReader, version: u8, rgba: bool) -> Result<LineStyleArray, GfxError> {
        let first = br.read_u8_aligned(Self::CTX)?;
        let (count, count_ext) = if first == 0xFF && version >= 2 {
            (br.read_u16_aligned(Self::CTX)? as usize, true)
        } else {
            (first as usize, false)
        };
        let mut styles = Vec::with_capacity(count);
        for _ in 0..count {
            styles.push(LineStyle::read(br, version, rgba)?);
        }
        Ok(LineStyleArray { count_ext, styles })
    }

    fn write(&self, bw: &mut BitWriter) {
        if self.count_ext {
            bw.write_u8_aligned(0xFF);
            bw.write_u16_aligned(self.styles.len() as u16);
        } else {
            bw.write_u8_aligned(self.styles.len() as u8);
        }
        for ls in &self.styles {
            ls.write(bw);
        }
    }
}

/// A `MOVETO` sub-record of a STYLECHANGERECORD. `num_bits` (the source
/// `MoveBits`) is stored verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveTo {
    /// `MoveBits` (the signed delta width).
    pub num_bits: u32,
    pub dx: i32,
    pub dy: i32,
}

/// The geometry of a STRAIGHTEDGERECORD: a general line (both deltas), or a
/// horizontal/vertical line (one delta).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StraightEdge {
    /// `GeneralLineFlag` set: both deltas present.
    General { dx: i32, dy: i32 },
    /// Horizontal line (`GeneralLineFlag` clear, `VertLineFlag` clear).
    Horizontal { dx: i32 },
    /// Vertical line (`GeneralLineFlag` clear, `VertLineFlag` set).
    Vertical { dy: i32 },
}

/// A fresh fill/line style set introduced by a STYLECHANGERECORD's StateNewStyles
/// (Shape2/3/4 only). Reading byte-aligns before the new arrays and the trailing
/// `(NumFillBits:4, NumLineBits:4)` reset the bit widths for the records that
/// follow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewStyles {
    pub fill_styles: FillStyleArray,
    pub line_styles: LineStyleArray,
    /// New `NumFillBits` for subsequent fill-index reads.
    pub num_fill_bits: u32,
    /// New `NumLineBits` for subsequent line-index reads.
    pub num_line_bits: u32,
}

/// One `SHAPERECORD`. The shape stream is terminated by [`ShapeRecord::End`]
/// (the all-zero non-edge record). Edge records store the source `NumBits` field
/// verbatim (actual delta width is `num_bits + 2`); it is non-minimal in 1,133
/// corpus edges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShapeRecord {
    /// ENDSHAPERECORD: `TypeFlag=0` plus five zero state bits.
    End,
    /// STYLECHANGERECORD. The 5-bit `flags`
    /// (`StateNewStyles`/`StateLineStyle`/`StateFillStyle1`/`StateFillStyle0`/
    /// `StateMoveTo`, MSB-to-LSB) are the source of truth; each optional field is
    /// `Some` iff its state bit is set.
    StyleChange {
        flags: u8,
        move_to: Option<MoveTo>,
        /// `FillStyle0` index (`StateFillStyle0`), `NumFillBits` wide.
        fill_style0: Option<u32>,
        /// `FillStyle1` index (`StateFillStyle1`), `NumFillBits` wide.
        fill_style1: Option<u32>,
        /// `LineStyle` index (`StateLineStyle`), `NumLineBits` wide.
        line_style: Option<u32>,
        /// New style arrays (`StateNewStyles`, Shape2/3/4 only).
        new_styles: Option<NewStyles>,
    },
    /// STRAIGHTEDGERECORD. `num_bits` is the source field (delta width `+2`).
    StraightEdge { num_bits: u32, edge: StraightEdge },
    /// CURVEDEDGERECORD. `num_bits` is the source field (delta width `+2`).
    CurvedEdge {
        num_bits: u32,
        control_dx: i32,
        control_dy: i32,
        anchor_dx: i32,
        anchor_dy: i32,
    },
}

/// A `SHAPEWITHSTYLE`: the initial fill/line style arrays, the starting
/// `(NumFillBits:4, NumLineBits:4)`, and the SHAPERECORD stream (including its
/// terminating [`ShapeRecord::End`]). The initial `num_fill_bits`/
/// `num_line_bits` are stored verbatim, never recomputed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShapeWithStyle {
    pub fill_styles: FillStyleArray,
    pub line_styles: LineStyleArray,
    pub num_fill_bits: u32,
    pub num_line_bits: u32,
    pub records: Vec<ShapeRecord>,
}

impl ShapeWithStyle {
    const CTX: &'static str = "SHAPEWITHSTYLE";

    fn read(br: &mut BitReader, version: u8) -> Result<ShapeWithStyle, GfxError> {
        let rgba = version >= 3;
        let fill_styles = FillStyleArray::read(br, version, rgba)?;
        let line_styles = LineStyleArray::read(br, version, rgba)?;
        let num_fill_bits = br.read_ubits(4, Self::CTX)?;
        let num_line_bits = br.read_ubits(4, Self::CTX)?;
        let records = read_shape_records(br, version, rgba, num_fill_bits, num_line_bits)?;
        Ok(ShapeWithStyle {
            fill_styles,
            line_styles,
            num_fill_bits,
            num_line_bits,
            records,
        })
    }

    fn write(&self, bw: &mut BitWriter) {
        self.fill_styles.write(bw);
        self.line_styles.write(bw);
        bw.write_ubits(self.num_fill_bits, 4);
        bw.write_ubits(self.num_line_bits, 4);
        write_shape_records(bw, &self.records, self.num_fill_bits, self.num_line_bits);
    }
}

/// Read the SHAPERECORD stream up to and including the ENDSHAPERECORD. The fill/
/// line bit widths shadow the SHAPEWITHSTYLE defaults and are reset by any
/// StateNewStyles record.
fn read_shape_records(
    br: &mut BitReader,
    version: u8,
    rgba: bool,
    mut num_fill_bits: u32,
    mut num_line_bits: u32,
) -> Result<Vec<ShapeRecord>, GfxError> {
    const CTX: &str = "SHAPERECORD";
    let mut records = Vec::new();
    loop {
        let type_flag = br.read_ubits(1, CTX)?;
        if type_flag == 0 {
            let flags = br.read_ubits(5, CTX)? as u8;
            if flags == 0 {
                records.push(ShapeRecord::End);
                break;
            }
            let new_styles_flag = flags & 0x10 != 0;
            let state_line = flags & 0x08 != 0;
            let state_fill1 = flags & 0x04 != 0;
            let state_fill0 = flags & 0x02 != 0;
            let state_move = flags & 0x01 != 0;

            let move_to = if state_move {
                let mb = br.read_ubits(5, CTX)?;
                let dx = br.read_sbits(mb, CTX)?;
                let dy = br.read_sbits(mb, CTX)?;
                Some(MoveTo {
                    num_bits: mb,
                    dx,
                    dy,
                })
            } else {
                None
            };
            let fill_style0 = if state_fill0 {
                Some(br.read_ubits(num_fill_bits, CTX)?)
            } else {
                None
            };
            let fill_style1 = if state_fill1 {
                Some(br.read_ubits(num_fill_bits, CTX)?)
            } else {
                None
            };
            let line_style = if state_line {
                Some(br.read_ubits(num_line_bits, CTX)?)
            } else {
                None
            };
            let new_styles = if new_styles_flag {
                if version < 2 {
                    return Err(GfxError::ShapeNewStylesUnsupported);
                }
                // StateNewStyles byte-aligns before the new (byte-structured)
                // style arrays (corpus-proven over 24 records).
                br.byte_align(CTX)?;
                let fill_styles = FillStyleArray::read(br, version, rgba)?;
                let line_styles = LineStyleArray::read(br, version, rgba)?;
                let nf = br.read_ubits(4, CTX)?;
                let nl = br.read_ubits(4, CTX)?;
                num_fill_bits = nf;
                num_line_bits = nl;
                Some(NewStyles {
                    fill_styles,
                    line_styles,
                    num_fill_bits: nf,
                    num_line_bits: nl,
                })
            } else {
                None
            };
            records.push(ShapeRecord::StyleChange {
                flags,
                move_to,
                fill_style0,
                fill_style1,
                line_style,
                new_styles,
            });
        } else {
            let straight = br.read_ubits(1, CTX)? != 0;
            let num_bits = br.read_ubits(4, CTX)?;
            let bits = num_bits + 2;
            if straight {
                let general = br.read_ubits(1, CTX)? != 0;
                let edge = if general {
                    StraightEdge::General {
                        dx: br.read_sbits(bits, CTX)?,
                        dy: br.read_sbits(bits, CTX)?,
                    }
                } else {
                    let vert = br.read_ubits(1, CTX)? != 0;
                    if vert {
                        StraightEdge::Vertical {
                            dy: br.read_sbits(bits, CTX)?,
                        }
                    } else {
                        StraightEdge::Horizontal {
                            dx: br.read_sbits(bits, CTX)?,
                        }
                    }
                };
                records.push(ShapeRecord::StraightEdge { num_bits, edge });
            } else {
                records.push(ShapeRecord::CurvedEdge {
                    num_bits,
                    control_dx: br.read_sbits(bits, CTX)?,
                    control_dy: br.read_sbits(bits, CTX)?,
                    anchor_dx: br.read_sbits(bits, CTX)?,
                    anchor_dy: br.read_sbits(bits, CTX)?,
                });
            }
        }
    }
    Ok(records)
}

/// Write the SHAPERECORD stream. The fill/line bit widths shadow the
/// SHAPEWITHSTYLE defaults and are reset by any StateNewStyles record, exactly
/// as on read.
fn write_shape_records(
    bw: &mut BitWriter,
    records: &[ShapeRecord],
    mut num_fill_bits: u32,
    mut num_line_bits: u32,
) {
    for rec in records {
        match rec {
            ShapeRecord::End => {
                bw.write_ubits(0, 1); // TypeFlag = 0
                bw.write_ubits(0, 5); // all state bits clear
            }
            ShapeRecord::StyleChange {
                flags,
                move_to,
                fill_style0,
                fill_style1,
                line_style,
                new_styles,
            } => {
                bw.write_ubits(0, 1); // TypeFlag = 0
                bw.write_ubits(*flags as u32, 5);
                if let Some(m) = move_to {
                    bw.write_ubits(m.num_bits, 5);
                    bw.write_sbits(m.dx, m.num_bits);
                    bw.write_sbits(m.dy, m.num_bits);
                }
                if let Some(f0) = fill_style0 {
                    bw.write_ubits(*f0, num_fill_bits);
                }
                if let Some(f1) = fill_style1 {
                    bw.write_ubits(*f1, num_fill_bits);
                }
                if let Some(l) = line_style {
                    bw.write_ubits(*l, num_line_bits);
                }
                if let Some(ns) = new_styles {
                    bw.byte_align();
                    ns.fill_styles.write(bw);
                    ns.line_styles.write(bw);
                    bw.write_ubits(ns.num_fill_bits, 4);
                    bw.write_ubits(ns.num_line_bits, 4);
                    num_fill_bits = ns.num_fill_bits;
                    num_line_bits = ns.num_line_bits;
                }
            }
            ShapeRecord::StraightEdge { num_bits, edge } => {
                bw.write_ubits(1, 1); // TypeFlag = 1
                bw.write_ubits(1, 1); // StraightFlag = 1
                bw.write_ubits(*num_bits, 4);
                let bits = num_bits + 2;
                match edge {
                    StraightEdge::General { dx, dy } => {
                        bw.write_ubits(1, 1); // GeneralLineFlag = 1
                        bw.write_sbits(*dx, bits);
                        bw.write_sbits(*dy, bits);
                    }
                    StraightEdge::Vertical { dy } => {
                        bw.write_ubits(0, 1); // GeneralLineFlag = 0
                        bw.write_ubits(1, 1); // VertLineFlag = 1
                        bw.write_sbits(*dy, bits);
                    }
                    StraightEdge::Horizontal { dx } => {
                        bw.write_ubits(0, 1); // GeneralLineFlag = 0
                        bw.write_ubits(0, 1); // VertLineFlag = 0
                        bw.write_sbits(*dx, bits);
                    }
                }
            }
            ShapeRecord::CurvedEdge {
                num_bits,
                control_dx,
                control_dy,
                anchor_dx,
                anchor_dy,
            } => {
                bw.write_ubits(1, 1); // TypeFlag = 1
                bw.write_ubits(0, 1); // StraightFlag = 0
                bw.write_ubits(*num_bits, 4);
                let bits = num_bits + 2;
                bw.write_sbits(*control_dx, bits);
                bw.write_sbits(*control_dy, bits);
                bw.write_sbits(*anchor_dx, bits);
                bw.write_sbits(*anchor_dy, bits);
            }
        }
    }
}

/// Map a shape `version` (1..=4) to its tag code.
fn shape_version_to_code(version: u8) -> u16 {
    match version {
        1 => TAG_DEFINE_SHAPE,
        2 => TAG_DEFINE_SHAPE2,
        3 => TAG_DEFINE_SHAPE3,
        _ => TAG_DEFINE_SHAPE4,
    }
}

/// Parse a `DefineShape*` body into its typed parts (the bitstream model). Used
/// by [`decode_define_shape`], which re-serializes and verifies the result.
struct DefineShapeParts {
    shape_id: u16,
    shape_bounds: Rect,
    edge_bounds: Option<Rect>,
    flags_byte: Option<u8>,
    shapes: ShapeWithStyle,
}

fn parse_define_shape(body: &[u8], version: u8) -> Result<DefineShapeParts, GfxError> {
    const CTX: &str = "DefineShape";
    let mut br = BitReader::new_at_byte(body, 0);
    let shape_id = br.read_u16_aligned(CTX)?;
    let shape_bounds = Rect::read(&mut br)?;
    let (edge_bounds, flags_byte) = if version == 4 {
        let eb = Rect::read(&mut br)?;
        let fb = br.read_u8_aligned(CTX)?;
        (Some(eb), Some(fb))
    } else {
        (None, None)
    };
    let shapes = ShapeWithStyle::read(&mut br, version)?;
    br.byte_align(CTX)?;
    // Trailing bytes after the byte-aligned shape end are a structural surprise;
    // the decode-then-verify byte comparison would also catch it, but fail here
    // so the caller falls back without re-serializing.
    if br.byte_pos() != body.len() {
        return Err(GfxError::TrailingTagBytes {
            code: shape_version_to_code(version),
            remaining: body.len() - br.byte_pos(),
        });
    }
    Ok(DefineShapeParts {
        shape_id,
        shape_bounds,
        edge_bounds,
        flags_byte,
        shapes,
    })
}

/// Serialize a `DefineShape*` body from its typed parts.
fn serialize_shape_body(
    version: u8,
    shape_id: u16,
    shape_bounds: &Rect,
    edge_bounds: Option<&Rect>,
    flags_byte: Option<u8>,
    shapes: &ShapeWithStyle,
) -> Vec<u8> {
    let mut bw = BitWriter::new();
    bw.write_u16_aligned(shape_id);
    shape_bounds.write(&mut bw);
    if version == 4 {
        edge_bounds
            .expect("DefineShape4 without edge_bounds")
            .write(&mut bw);
        bw.write_u8_aligned(flags_byte.expect("DefineShape4 without flags_byte"));
    }
    shapes.write(&mut bw);
    bw.byte_align();
    bw.into_bytes()
}

/// Decode a `DefineShape*` (code 2/22/32/83) body. The body is decode-then-
/// verified: it is fully parsed, re-serialized, and compared against the source;
/// on any structural surprise or byte mismatch it falls back to [`Tag::Unknown`]
/// so byte-identity is never silently lost. Always returns `Ok` -- the fallback
/// is data, not an error.
fn decode_define_shape(code: u16, body: Vec<u8>, force_long: bool) -> Tag {
    let version = match code {
        TAG_DEFINE_SHAPE => 1u8,
        TAG_DEFINE_SHAPE2 => 2,
        TAG_DEFINE_SHAPE3 => 3,
        _ => 4,
    };
    match parse_define_shape(&body, version) {
        Ok(parts) => {
            let reencoded = serialize_shape_body(
                version,
                parts.shape_id,
                &parts.shape_bounds,
                parts.edge_bounds.as_ref(),
                parts.flags_byte,
                &parts.shapes,
            );
            if reencoded == body {
                Tag::DefineShape {
                    version,
                    shape_id: parts.shape_id,
                    shape_bounds: parts.shape_bounds,
                    edge_bounds: parts.edge_bounds,
                    flags_byte: parts.flags_byte,
                    shapes: parts.shapes,
                    force_long,
                }
            } else {
                Tag::Unknown {
                    code,
                    raw: body,
                    force_long,
                }
            }
        }
        Err(_) => Tag::Unknown {
            code,
            raw: body,
            force_long,
        },
    }
}
