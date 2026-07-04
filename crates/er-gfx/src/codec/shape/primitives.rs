/// A bit-packed `RECT` (Nbits-aware). Used by [`Tag::DefineScalingGrid`] and
/// reusable for shape bounds later. `nbits` is stored exactly (not recomputed)
/// so the source's bit width is reproduced even when non-minimal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Bit width of each of the four signed coordinates (the source's `Nbits`).
    pub nbits: u32,
    pub x_min: i32,
    pub x_max: i32,
    pub y_min: i32,
    pub y_max: i32,
}

impl Rect {
    const CTX: &'static str = "RECT";

    fn read(br: &mut BitReader) -> Result<Rect, GfxError> {
        let nbits = br.read_ubits(5, Self::CTX)?;
        let x_min = br.read_sbits(nbits, Self::CTX)?;
        let x_max = br.read_sbits(nbits, Self::CTX)?;
        let y_min = br.read_sbits(nbits, Self::CTX)?;
        let y_max = br.read_sbits(nbits, Self::CTX)?;
        br.byte_align(Self::CTX)?;
        Ok(Rect {
            nbits,
            x_min,
            x_max,
            y_min,
            y_max,
        })
    }

    fn write(&self, bw: &mut BitWriter) {
        bw.write_ubits(self.nbits, 5);
        bw.write_sbits(self.x_min, self.nbits);
        bw.write_sbits(self.x_max, self.nbits);
        bw.write_sbits(self.y_min, self.nbits);
        bw.write_sbits(self.y_max, self.nbits);
        bw.byte_align();
    }
}

/// A bit-packed `MATRIX` with each source bit width preserved exactly.
///
/// `scale_x/y` and `rotate_skew0/1` are 16.16 fixed-point; `translate_x/y` are
/// twips. All are stored as their raw signed integers. The `*_nbits` fields hold
/// the SOURCE's bit widths; they are reproduced verbatim because the exporter is
/// not minimal (see the primitive-layer module comment).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matrix {
    pub has_scale: bool,
    /// `NScaleBits` (only meaningful when `has_scale`).
    pub scale_nbits: u32,
    pub scale_x: i32,
    pub scale_y: i32,
    pub has_rotate: bool,
    /// `NRotateBits` (only meaningful when `has_rotate`).
    pub rotate_nbits: u32,
    pub rotate_skew0: i32,
    pub rotate_skew1: i32,
    /// `NTranslateBits`. Translate is always present.
    pub translate_nbits: u32,
    pub translate_x: i32,
    pub translate_y: i32,
}

impl Matrix {
    const CTX: &'static str = "MATRIX";

    fn read(br: &mut BitReader) -> Result<Matrix, GfxError> {
        let has_scale = br.read_ubits(1, Self::CTX)? != 0;
        let (scale_nbits, scale_x, scale_y) = if has_scale {
            let n = br.read_ubits(5, Self::CTX)?;
            (
                n,
                br.read_fbits(n, Self::CTX)?,
                br.read_fbits(n, Self::CTX)?,
            )
        } else {
            (0, 0, 0)
        };
        let has_rotate = br.read_ubits(1, Self::CTX)? != 0;
        let (rotate_nbits, rotate_skew0, rotate_skew1) = if has_rotate {
            let n = br.read_ubits(5, Self::CTX)?;
            (
                n,
                br.read_fbits(n, Self::CTX)?,
                br.read_fbits(n, Self::CTX)?,
            )
        } else {
            (0, 0, 0)
        };
        let translate_nbits = br.read_ubits(5, Self::CTX)?;
        let translate_x = br.read_sbits(translate_nbits, Self::CTX)?;
        let translate_y = br.read_sbits(translate_nbits, Self::CTX)?;
        br.byte_align(Self::CTX)?;
        Ok(Matrix {
            has_scale,
            scale_nbits,
            scale_x,
            scale_y,
            has_rotate,
            rotate_nbits,
            rotate_skew0,
            rotate_skew1,
            translate_nbits,
            translate_x,
            translate_y,
        })
    }

    fn write(&self, bw: &mut BitWriter) {
        bw.write_ubits(self.has_scale as u32, 1);
        if self.has_scale {
            bw.write_ubits(self.scale_nbits, 5);
            bw.write_fbits(self.scale_x, self.scale_nbits);
            bw.write_fbits(self.scale_y, self.scale_nbits);
        }
        bw.write_ubits(self.has_rotate as u32, 1);
        if self.has_rotate {
            bw.write_ubits(self.rotate_nbits, 5);
            bw.write_fbits(self.rotate_skew0, self.rotate_nbits);
            bw.write_fbits(self.rotate_skew1, self.rotate_nbits);
        }
        bw.write_ubits(self.translate_nbits, 5);
        bw.write_sbits(self.translate_x, self.translate_nbits);
        bw.write_sbits(self.translate_y, self.translate_nbits);
        bw.byte_align();
    }
}

/// A bit-packed `CXFORM` (no alpha): RGB multiply/add terms. `nbits` preserved
/// exactly. Not used by a typed tag yet (it is the `PlaceObject`/`DefineButton`
/// color transform) but provided as a reusable primitive with its own tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cxform {
    pub has_add: bool,
    pub has_mult: bool,
    pub nbits: u32,
    /// `[red, green, blue]` multiply terms, present iff `has_mult`.
    pub mult: Option<[i32; 3]>,
    /// `[red, green, blue]` add terms, present iff `has_add`.
    pub add: Option<[i32; 3]>,
}

impl Cxform {
    const CTX: &'static str = "CXFORM";

    fn read(br: &mut BitReader) -> Result<Cxform, GfxError> {
        let has_add = br.read_ubits(1, Self::CTX)? != 0;
        let has_mult = br.read_ubits(1, Self::CTX)? != 0;
        let nbits = br.read_ubits(4, Self::CTX)?;
        let mult = if has_mult {
            Some([
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
            ])
        } else {
            None
        };
        let add = if has_add {
            Some([
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
            ])
        } else {
            None
        };
        br.byte_align(Self::CTX)?;
        Ok(Cxform {
            has_add,
            has_mult,
            nbits,
            mult,
            add,
        })
    }

    fn write(&self, bw: &mut BitWriter) {
        bw.write_ubits(self.has_add as u32, 1);
        bw.write_ubits(self.has_mult as u32, 1);
        bw.write_ubits(self.nbits, 4);
        if let Some(m) = self.mult {
            for v in m {
                bw.write_sbits(v, self.nbits);
            }
        }
        if let Some(a) = self.add {
            for v in a {
                bw.write_sbits(v, self.nbits);
            }
        }
        bw.byte_align();
    }
}

/// A bit-packed `CXFORMWITHALPHA`: RGBA multiply/add terms. `nbits` preserved
/// exactly. This is the color transform carried by [`Tag::PlaceObject2`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CxformWithAlpha {
    pub has_add: bool,
    pub has_mult: bool,
    pub nbits: u32,
    /// `[red, green, blue, alpha]` multiply terms, present iff `has_mult`.
    pub mult: Option<[i32; 4]>,
    /// `[red, green, blue, alpha]` add terms, present iff `has_add`.
    pub add: Option<[i32; 4]>,
}

impl CxformWithAlpha {
    const CTX: &'static str = "CXFORM";

    fn read(br: &mut BitReader) -> Result<CxformWithAlpha, GfxError> {
        let has_add = br.read_ubits(1, Self::CTX)? != 0;
        let has_mult = br.read_ubits(1, Self::CTX)? != 0;
        let nbits = br.read_ubits(4, Self::CTX)?;
        let mult = if has_mult {
            Some([
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
            ])
        } else {
            None
        };
        let add = if has_add {
            Some([
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
                br.read_sbits(nbits, Self::CTX)?,
            ])
        } else {
            None
        };
        br.byte_align(Self::CTX)?;
        Ok(CxformWithAlpha {
            has_add,
            has_mult,
            nbits,
            mult,
            add,
        })
    }

    fn write(&self, bw: &mut BitWriter) {
        bw.write_ubits(self.has_add as u32, 1);
        bw.write_ubits(self.has_mult as u32, 1);
        bw.write_ubits(self.nbits, 4);
        if let Some(m) = self.mult {
            for v in m {
                bw.write_sbits(v, self.nbits);
            }
        }
        if let Some(a) = self.add {
            for v in a {
                bw.write_sbits(v, self.nbits);
            }
        }
        bw.byte_align();
    }
}

/// One entry of a `PlaceObject3` SURFACEFILTERLIST.
///
/// Only the filter ids that actually occur in the Elden Ring menu corpus are
/// typed: [`Filter::DropShadow`] (id 0, 2,849 instances) and [`Filter::Glow`]
/// (id 2, 31 instances). The fixed-point fields (`FIXED` is 16.16, `FIXED8` is
/// 8.8) are stored as their raw little-endian signed integers so byte-identity
/// is exact without committing to a float representation; `flags` holds the
/// filter's trailing `InnerShadow`/`Knockout`/`CompositeSource`/`Passes`
/// sub-byte verbatim. Any other filter id forces the whole `PlaceObject3` back
/// to [`Tag::Unknown`] (none occur -- 0 of 2,880 filters).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filter {
    /// `DropShadowFilter` (id 0): `RGBA` color, BlurX/BlurY/Angle/Distance
    /// (`FIXED` 16.16), Strength (`FIXED8` 8.8), and a trailing flags byte.
    DropShadow {
        /// `[red, green, blue, alpha]` shadow color.
        color: [u8; 4],
        /// BlurX, raw 16.16 fixed (`FIXED`).
        blur_x: i32,
        /// BlurY, raw 16.16 fixed.
        blur_y: i32,
        /// Angle, raw 16.16 fixed (radians).
        angle: i32,
        /// Distance, raw 16.16 fixed.
        distance: i32,
        /// Strength, raw 8.8 fixed (`FIXED8`).
        strength: i16,
        /// `InnerShadow`(0x80) / `Knockout`(0x40) / `CompositeSource`(0x20) /
        /// `Passes`(low 5 bits), stored verbatim.
        flags: u8,
    },
    /// `GlowFilter` (id 2): `RGBA` color, BlurX/BlurY (`FIXED`), Strength
    /// (`FIXED8`), and a trailing flags byte.
    Glow {
        /// `[red, green, blue, alpha]` glow color.
        color: [u8; 4],
        /// BlurX, raw 16.16 fixed.
        blur_x: i32,
        /// BlurY, raw 16.16 fixed.
        blur_y: i32,
        /// Strength, raw 8.8 fixed.
        strength: i16,
        /// `InnerGlow`(0x80) / `Knockout`(0x40) / `CompositeSource`(0x20) /
        /// `Passes`(low 5 bits), stored verbatim.
        flags: u8,
    },
}

impl Filter {
    /// Read one filter: a `u8` id followed by its body. Returns `Ok(None)` for an
    /// unmodelled filter id so the caller can fall the whole `PlaceObject3` back
    /// to [`Tag::Unknown`] (the id byte is NOT consumed in that case, but the
    /// caller discards `r` anyway and re-emits the raw body).
    fn read(r: &mut GfxReader) -> Result<Option<Filter>, GfxError> {
        let id = r.read_u8()?;
        match id {
            FILTER_DROP_SHADOW => {
                let color = [r.read_u8()?, r.read_u8()?, r.read_u8()?, r.read_u8()?];
                let blur_x = r.read_u32()? as i32;
                let blur_y = r.read_u32()? as i32;
                let angle = r.read_u32()? as i32;
                let distance = r.read_u32()? as i32;
                let strength = r.read_u16()? as i16;
                let flags = r.read_u8()?;
                Ok(Some(Filter::DropShadow {
                    color,
                    blur_x,
                    blur_y,
                    angle,
                    distance,
                    strength,
                    flags,
                }))
            }
            FILTER_GLOW => {
                let color = [r.read_u8()?, r.read_u8()?, r.read_u8()?, r.read_u8()?];
                let blur_x = r.read_u32()? as i32;
                let blur_y = r.read_u32()? as i32;
                let strength = r.read_u16()? as i16;
                let flags = r.read_u8()?;
                Ok(Some(Filter::Glow {
                    color,
                    blur_x,
                    blur_y,
                    strength,
                    flags,
                }))
            }
            _ => Ok(None),
        }
    }

    fn write(&self, w: &mut GfxWriter) {
        match self {
            Filter::DropShadow {
                color,
                blur_x,
                blur_y,
                angle,
                distance,
                strength,
                flags,
            } => {
                w.write_u8(FILTER_DROP_SHADOW);
                w.write_bytes(color);
                w.write_u32(*blur_x as u32);
                w.write_u32(*blur_y as u32);
                w.write_u32(*angle as u32);
                w.write_u32(*distance as u32);
                w.write_u16(*strength as u16);
                w.write_u8(*flags);
            }
            Filter::Glow {
                color,
                blur_x,
                blur_y,
                strength,
                flags,
            } => {
                w.write_u8(FILTER_GLOW);
                w.write_bytes(color);
                w.write_u32(*blur_x as u32);
                w.write_u32(*blur_y as u32);
                w.write_u16(*strength as u16);
                w.write_u8(*flags);
            }
        }
    }
}

// ===========================================================================
// Tier-3: the DefineShape family + SHAPEWITHSTYLE / SHAPERECORD bitstream.
// ===========================================================================
//
// A SHAPEWITHSTYLE is one continuous MSB-first bitstream that mixes byte-aligned
// sub-structures (FILLSTYLEARRAY, LINESTYLEARRAY, GRADRECORD, the embedded
// MATRIX/RECT primitives) with truly bit-packed SHAPERECORDs. Every source bit
// width is preserved verbatim because the Scaleform exporter is non-minimal: of
// the 16,902 edge records in the 114-file corpus, 1,133 use more delta bits than
// the minimal width (the SHAPEWITHSTYLE NumFillBits/NumLineBits happen to be
// minimal in this corpus, but are still stored, never recomputed). The decoder
// re-serializes every parsed shape and compares against the source body; any
// mismatch (or structural surprise) falls the whole tag back to [`Tag::Unknown`]
// so the raw body re-emits byte-identically -- see [`decode_define_shape`].

/// A shape color: 3-byte `RGB` (DefineShape/DefineShape2) or 4-byte `RGBA`
/// (DefineShape3/DefineShape4). The byte width is dictated by the shape version,
/// captured here so colors re-encode without re-deriving the width.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Color {
    /// 3-byte `[red, green, blue]`.
    Rgb([u8; 3]),
    /// 4-byte `[red, green, blue, alpha]`.
    Rgba([u8; 4]),
}

impl Color {
    fn read(br: &mut BitReader, rgba: bool, context: &'static str) -> Result<Color, GfxError> {
        if rgba {
            let b = br.read_bytes_aligned(4, context)?;
            Ok(Color::Rgba([b[0], b[1], b[2], b[3]]))
        } else {
            let b = br.read_bytes_aligned(3, context)?;
            Ok(Color::Rgb([b[0], b[1], b[2]]))
        }
    }

    fn write(&self, bw: &mut BitWriter) {
        match self {
            Color::Rgb(c) => bw.write_bytes_aligned(c),
            Color::Rgba(c) => bw.write_bytes_aligned(c),
        }
    }
}

/// One `GRADRECORD`: a ratio (`0..=255`) and a [`Color`] (RGB for Shape1/2,
/// RGBA for Shape3/4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GradRecord {
    pub ratio: u8,
    pub color: Color,
}

/// A `GRADIENT` / `FOCALGRADIENT`. The `(SpreadMode:2, InterpolationMode:2,
/// NumGradients:4)` byte is bit-packed (empirically the same layout for every
/// shape version in the corpus); `NumGradients` is derived from `records.len()`
/// on write. `focal_point` is the trailing `FIXED8` present only for focal
/// gradients (fill type `0x13`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gradient {
    /// `SpreadMode` (2 bits): 0 pad, 1 reflect, 2 repeat.
    pub spread_mode: u8,
    /// `InterpolationMode` (2 bits): 0 normal RGB, 1 linear RGB.
    pub interpolation_mode: u8,
    /// Gradient stops (`NumGradients` is `records.len()`, max 15).
    pub records: Vec<GradRecord>,
    /// Focal point `FIXED8` (raw little-endian `i16`), present iff focal-radial.
    pub focal_point: Option<i16>,
}

impl Gradient {
    fn read(
        br: &mut BitReader,
        rgba: bool,
        focal: bool,
        context: &'static str,
    ) -> Result<Gradient, GfxError> {
        let spread_mode = br.read_ubits(2, context)? as u8;
        let interpolation_mode = br.read_ubits(2, context)? as u8;
        let num = br.read_ubits(4, context)?;
        // 2 + 2 + 4 = 8 bits -> back to byte alignment for the GRADRECORDs.
        let mut records = Vec::with_capacity(num as usize);
        for _ in 0..num {
            let ratio = br.read_u8_aligned(context)?;
            let color = Color::read(br, rgba, context)?;
            records.push(GradRecord { ratio, color });
        }
        let focal_point = if focal {
            Some(br.read_u16_aligned(context)? as i16)
        } else {
            None
        };
        Ok(Gradient {
            spread_mode,
            interpolation_mode,
            records,
            focal_point,
        })
    }

    fn write(&self, bw: &mut BitWriter) {
        bw.write_ubits(self.spread_mode as u32, 2);
        bw.write_ubits(self.interpolation_mode as u32, 2);
        bw.write_ubits(self.records.len() as u32, 4);
        for r in &self.records {
            bw.write_u8_aligned(r.ratio);
            r.color.write(bw);
        }
        if let Some(fp) = self.focal_point {
            bw.write_u16_aligned(fp as u16);
        }
    }
}

/// A `FILLSTYLE`. The leading type byte is stored inside the `Gradient`/`Bitmap`
/// variants (`fill_type`) so the exact sub-kind (`0x10` linear / `0x12` radial /
/// `0x13` focal; `0x40`-`0x43` bitmap clip/tile flavors) is reproduced verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FillStyle {
    /// Solid color (type `0x00`): `RGB` (Shape1/2) or `RGBA` (Shape3/4).
    Solid(Color),
    /// Gradient (type `0x10` linear, `0x12` radial, `0x13` focal-radial): a
    /// `MATRIX` and a [`Gradient`].
    Gradient {
        /// Fill type byte (`0x10`/`0x12`/`0x13`), preserved verbatim.
        fill_type: u8,
        matrix: Matrix,
        gradient: Gradient,
    },
    /// Bitmap fill (types `0x40`-`0x43`): a `bitmapId` and a `MATRIX`.
    Bitmap {
        /// Fill type byte (`0x40`-`0x43`), preserved verbatim.
        fill_type: u8,
        bitmap_id: u16,
        matrix: Matrix,
    },
}

impl FillStyle {
    const CTX: &'static str = "FILLSTYLE";

    fn read(br: &mut BitReader, rgba: bool) -> Result<FillStyle, GfxError> {
        let t = br.read_u8_aligned(Self::CTX)?;
        match t {
            0x00 => Ok(FillStyle::Solid(Color::read(br, rgba, Self::CTX)?)),
            0x10 | 0x12 | 0x13 => {
                let matrix = Matrix::read(br)?;
                let gradient = Gradient::read(br, rgba, t == 0x13, Self::CTX)?;
                Ok(FillStyle::Gradient {
                    fill_type: t,
                    matrix,
                    gradient,
                })
            }
            0x40 | 0x41 | 0x42 | 0x43 => {
                let bitmap_id = br.read_u16_aligned(Self::CTX)?;
                let matrix = Matrix::read(br)?;
                Ok(FillStyle::Bitmap {
                    fill_type: t,
                    bitmap_id,
                    matrix,
                })
            }
            other => Err(GfxError::UnknownFillStyleType(other)),
        }
    }

    fn write(&self, bw: &mut BitWriter) {
        match self {
            FillStyle::Solid(color) => {
                bw.write_u8_aligned(0x00);
                color.write(bw);
            }
            FillStyle::Gradient {
                fill_type,
                matrix,
                gradient,
            } => {
                bw.write_u8_aligned(*fill_type);
                matrix.write(bw);
                gradient.write(bw);
            }
            FillStyle::Bitmap {
                fill_type,
                bitmap_id,
                matrix,
            } => {
                bw.write_u8_aligned(*fill_type);
                bw.write_u16_aligned(*bitmap_id);
                matrix.write(bw);
            }
        }
    }
}

/// A `FILLSTYLEARRAY`: a count (`u8`, or `0xFF`-extended `u16` for Shape2/3/4)
/// then the fill styles. `count_ext` records whether the extended form was used
/// so a non-minimal count encoding (none occur in the corpus) is preserved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillStyleArray {
    /// Whether the `0xFF`-extended `u16` count form was used.
    pub count_ext: bool,
    pub styles: Vec<FillStyle>,
}

impl FillStyleArray {
    const CTX: &'static str = "FILLSTYLEARRAY";

    fn read(br: &mut BitReader, version: u8, rgba: bool) -> Result<FillStyleArray, GfxError> {
        let first = br.read_u8_aligned(Self::CTX)?;
        let (count, count_ext) = if first == 0xFF && version >= 2 {
            (br.read_u16_aligned(Self::CTX)? as usize, true)
        } else {
            (first as usize, false)
        };
        let mut styles = Vec::with_capacity(count);
        for _ in 0..count {
            styles.push(FillStyle::read(br, rgba)?);
        }
        Ok(FillStyleArray { count_ext, styles })
    }

    fn write(&self, bw: &mut BitWriter) {
        if self.count_ext {
            bw.write_u8_aligned(0xFF);
            bw.write_u16_aligned(self.styles.len() as u16);
        } else {
            bw.write_u8_aligned(self.styles.len() as u8);
        }
        for fs in &self.styles {
            fs.write(bw);
        }
    }
}

/// The fill carried by a `LINESTYLE2` (DefineShape4): either an `RGBA` color or,
/// when its `HasFill` flag is set, a nested [`FillStyle`] (never set in the
/// corpus, but modelled).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineFill {
    /// `[red, green, blue, alpha]` line color (HasFill clear).
    Color([u8; 4]),
    /// A nested fill style (HasFill set).
    Fill(Box<FillStyle>),
}

/// A `LINESTYLE` (Shape1/2/3) or `LINESTYLE2` (Shape4). For `LINESTYLE2` the
/// 16-bit caps/join/flags word is stored verbatim and governs the optional
/// `miter_limit` (present iff JoinStyle == 2) and whether `fill` is a color or a
/// nested fill (HasFill bit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineStyle {
    /// `LINESTYLE`: width + `RGB` (Shape1/2) or `RGBA` (Shape3) color.
    Plain { width: u16, color: Color },
    /// `LINESTYLE2`: width + 16-bit flags + optional miter limit + fill.
    Style2 {
        width: u16,
        /// The 16-bit caps/join/HasFill/NoHScale/NoVScale/PixelHinting/NoClose/
        /// EndCap flags word, stored verbatim.
        flags: u16,
        /// `MiterLimitFactor` (`u16`), present iff JoinStyle (`flags` bits 12-13)
        /// is 2.
        miter_limit: Option<u16>,
        /// Color (HasFill clear) or nested fill (HasFill set, `flags` bit 11).
        fill: LineFill,
    },
}
