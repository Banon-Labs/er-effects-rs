//! Rasterize a [`Tag::DefineFont3`] glyph outline into an 8-bit **coverage**
//! bitmap. This is the piece that lets us draw the game's *own* menu font: when
//! the Scaleform file-open hook hands us the decompressed bytes of a menu movie
//! that carries a `DefineFont3`, we build a [`RasterFont`] from it and rasterize
//! whatever glyphs the stats panel needs -- so the baked text matches the native
//! font's letterforms rather than a bundled bitmap font. Nothing game-derived is
//! embedded or committed: the outlines come from the game at runtime, and this
//! module only turns them into pixels.
//!
//! # Geometry
//!
//! A glyph `SHAPE` is a pen walk in **font units** (SWF y-axis points *down*; the
//! baseline is `y == 0` and ascenders sit at negative `y`). A `StyleChange` with
//! a `MoveTo` sets an absolute pen position (starting a new contour); a
//! `StraightEdge` is a line delta; a `CurvedEdge` is a quadratic Bézier delta
//! (control then anchor). Each contour auto-closes back to its start.
//!
//! # Fill
//!
//! Contours are flattened to line segments (curves subdivided by chord length)
//! and filled with a **non-zero winding** scanline rule -- correct for glyphs
//! whose counters (the hole in `o`, `e`, `a`) wind opposite to the outer
//! contour, and robust to overlapping same-direction contours. Anti-aliasing is
//! `SUBSAMPLES_Y` sub-scanlines per row with *analytic* horizontal coverage
//! (each inside span contributes its exact overlap with each pixel column), so
//! edges are smooth without a full NxN supersample.

use crate::{GlyphShape, MoveTo, ShapeRecord, StraightEdge, Tag};

/// Vertical sub-scanlines per output row for anti-aliasing. Horizontal coverage
/// is analytic (exact span/column overlap), so 4 vertical samples already give
/// smooth edges at menu text sizes.
const SUBSAMPLES_Y: u32 = 4;

/// A rasterizable view of one `DefineFont3`: the glyph outlines, their character
/// codes (parallel to the outlines), per-glyph advances, and the ascent/descent
/// metrics used to convert a desired pixel height into a font-unit scale. Build
/// once from a captured menu movie, then [`rasterize`](RasterFont::rasterize) any
/// character at any scale.
#[derive(Clone, Debug)]
pub struct RasterFont {
    pub font_id: u16,
    /// Glyph outlines, index-parallel to [`codes`](Self::codes).
    glyphs: Vec<GlyphShape>,
    /// Character code per glyph (index-parallel to [`glyphs`](Self::glyphs)).
    codes: Vec<u16>,
    /// Per-glyph advance width in font units (empty if the font had no layout
    /// block; then advance falls back to the inked width).
    advances: Vec<i16>,
    /// Font ascent in font units (positive, above baseline). `None` if the font
    /// carried no layout block, in which case scale is derived from the glyph
    /// extent instead.
    ascent: Option<i32>,
    descent: Option<i32>,
}

/// A rasterized glyph: an 8-bit coverage bitmap plus the placement metrics a text
/// layout needs. Composite it at destination `(pen_x + left, baseline_y + top)`.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphBitmap {
    pub width: u32,
    pub height: u32,
    /// X offset (px) from the pen origin to the bitmap's left edge (left side
    /// bearing; may be negative).
    pub left: i32,
    /// Y offset (px) from the baseline to the bitmap's top edge. Negative for the
    /// usual case where ink rises above the baseline.
    pub top: i32,
    /// Pen advance in px (how far to move the pen after drawing this glyph).
    pub advance: f32,
    /// `width * height` coverage values, row-major, `0..=255`.
    pub coverage: Vec<u8>,
}

impl RasterFont {
    /// Build from a `DefineFont3` tag. Returns `None` for any other tag. The
    /// glyph/code/advance vectors are cloned so the font outlives the parsed
    /// movie (the captured bytes can be dropped immediately).
    pub fn from_define_font3(tag: &Tag) -> Option<RasterFont> {
        let Tag::DefineFont3 {
            font_id,
            glyphs,
            codes,
            layout,
            ..
        } = tag
        else {
            return None;
        };
        // codes must be index-parallel to glyphs; a malformed font that violates
        // that is unusable for character lookup.
        if codes.len() != glyphs.len() {
            return None;
        }
        let (advances, ascent, descent) = match layout {
            Some(l) => (
                l.advance.clone(),
                Some(l.ascent as i32),
                Some(l.descent as i32),
            ),
            None => (Vec::new(), None, None),
        };
        Some(RasterFont {
            font_id: *font_id,
            glyphs: glyphs.clone(),
            codes: codes.clone(),
            advances,
            ascent,
            descent,
        })
    }

    /// Number of glyphs.
    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    /// Whether a layout block (ascent/descent/advances) was present.
    pub fn has_layout(&self) -> bool {
        self.ascent.is_some()
    }

    /// Glyph index for `ch` via the font code table, or `None` if unmapped.
    pub fn glyph_index(&self, ch: char) -> Option<usize> {
        let code = ch as u32;
        if code > u16::MAX as u32 {
            return None;
        }
        let code = code as u16;
        self.codes.iter().position(|&c| c == code)
    }

    /// Font units per em, from ascent+descent when a layout block is present,
    /// else the vertical extent of the glyph outlines (a coarse fallback).
    pub fn units_per_em(&self) -> f32 {
        if let (Some(a), Some(d)) = (self.ascent, self.descent) {
            let em = (a + d).unsigned_abs() as f32;
            if em > 0.0 {
                return em;
            }
        }
        // Fallback: measure the tallest glyph extent across the font.
        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        for g in &self.glyphs {
            if let Some((_, y0, _, y1)) = glyph_bbox_units(g) {
                min_y = min_y.min(y0);
                max_y = max_y.max(y1);
            }
        }
        if max_y > min_y {
            (max_y - min_y) as f32
        } else {
            1024.0 // last-resort nominal em; better than dividing by zero
        }
    }

    /// px-per-font-unit scale so the font's em box is `em_px` pixels tall.
    pub fn scale_for_em_px(&self, em_px: f32) -> f32 {
        em_px / self.units_per_em()
    }

    /// px-per-font-unit scale so the ascent is `ascent_px` pixels (falls back to
    /// an em-relative estimate if the font had no layout).
    pub fn scale_for_ascent_px(&self, ascent_px: f32) -> f32 {
        match self.ascent {
            Some(a) if a > 0 => ascent_px / a as f32,
            // ~80% of the em is a typical ascent fraction.
            _ => self.scale_for_em_px(ascent_px / 0.8),
        }
    }

    /// Ascent in px at `scale`.
    pub fn ascent_px(&self, scale: f32) -> f32 {
        match self.ascent {
            Some(a) => a as f32 * scale,
            None => 0.8 * self.units_per_em() * scale,
        }
    }

    /// A reasonable line height in px at `scale` (`(ascent + descent)` when known,
    /// else the em).
    pub fn line_height_px(&self, scale: f32) -> f32 {
        match (self.ascent, self.descent) {
            (Some(a), Some(d)) => (a + d).unsigned_abs() as f32 * scale,
            _ => self.units_per_em() * scale,
        }
    }

    /// Pen advance in px for `ch` at `scale`. Uses the layout advance table when
    /// present, else the inked width plus a small gap.
    pub fn advance_px(&self, ch: char, scale: f32) -> f32 {
        let Some(gi) = self.glyph_index(ch) else {
            // Unmapped (e.g. a stray space not in the code table): advance a
            // quarter-em so text does not collapse.
            return 0.25 * self.units_per_em() * scale;
        };
        if let Some(adv) = self.advances.get(gi) {
            return *adv as f32 * scale;
        }
        match glyph_bbox_units(&self.glyphs[gi]) {
            Some((x0, _, x1, _)) => (x1 - x0) as f32 * scale + 0.15 * self.units_per_em() * scale,
            None => 0.25 * self.units_per_em() * scale,
        }
    }

    /// Rasterize `ch` at `scale` (px per font unit). Returns `None` if the
    /// character is unmapped or its outline is empty (e.g. a space).
    pub fn rasterize(&self, ch: char, scale: f32) -> Option<GlyphBitmap> {
        let gi = self.glyph_index(ch)?;
        let advance = self.advance_px(ch, scale);
        rasterize_shape(&self.glyphs[gi], scale, advance)
    }
}

/// A flattened contour edge in bitmap-local px space (y increasing downward),
/// carrying its winding direction (`+1` downward, `-1` upward).
struct Edge {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    dir: i32,
}

/// Walk a glyph's shape records, returning closed contours as point lists in
/// font units (each `Vec` is one contour; the closing segment back to the start
/// is implicit).
fn glyph_contours(glyph: &GlyphShape) -> Vec<Vec<(f32, f32)>> {
    let mut contours: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let mut pen_x: i32 = 0;
    let mut pen_y: i32 = 0;
    for rec in &glyph.records {
        match rec {
            ShapeRecord::StyleChange {
                move_to: Some(MoveTo { dx, dy, .. }),
                ..
            } => {
                // Absolute move: start a new contour.
                if cur.len() >= 2 {
                    contours.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
                pen_x = *dx;
                pen_y = *dy;
                cur.push((pen_x as f32, pen_y as f32));
            }
            ShapeRecord::StyleChange { .. } => {
                // Style-only change (fill/line index); geometry unaffected.
            }
            ShapeRecord::StraightEdge { edge, .. } => {
                let (dx, dy) = match edge {
                    StraightEdge::General { dx, dy } => (*dx, *dy),
                    StraightEdge::Horizontal { dx } => (*dx, 0),
                    StraightEdge::Vertical { dy } => (0, *dy),
                };
                pen_x += dx;
                pen_y += dy;
                cur.push((pen_x as f32, pen_y as f32));
            }
            ShapeRecord::CurvedEdge {
                control_dx,
                control_dy,
                anchor_dx,
                anchor_dy,
                ..
            } => {
                let p0 = (pen_x as f32, pen_y as f32);
                let cx = pen_x + control_dx;
                let cy = pen_y + control_dy;
                let ax = cx + anchor_dx;
                let ay = cy + anchor_dy;
                let ctrl = (cx as f32, cy as f32);
                let anchor = (ax as f32, ay as f32);
                flatten_quadratic(p0, ctrl, anchor, &mut cur);
                pen_x = ax;
                pen_y = ay;
            }
            ShapeRecord::End => break,
        }
    }
    if cur.len() >= 2 {
        contours.push(cur);
    }
    contours
}

/// Append points approximating the quadratic Bézier `p0 -> ctrl -> anchor` to
/// `out` (excluding `p0`, which is already the pen position). Subdivision count
/// scales with the control-polygon length so large curves stay smooth.
fn flatten_quadratic(p0: (f32, f32), ctrl: (f32, f32), anchor: (f32, f32), out: &mut Vec<(f32, f32)>) {
    let len = dist(p0, ctrl) + dist(ctrl, anchor);
    // One segment per ~40 font units of control-polygon length, clamped.
    let n = ((len / 40.0).ceil() as u32).clamp(2, 32);
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let mt = 1.0 - t;
        let x = mt * mt * p0.0 + 2.0 * mt * t * ctrl.0 + t * t * anchor.0;
        let y = mt * mt * p0.1 + 2.0 * mt * t * ctrl.1 + t * t * anchor.1;
        out.push((x, y));
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    (dx * dx + dy * dy).sqrt()
}

/// Bounding box of a glyph's outline in font units: `(min_x, min_y, max_x,
/// max_y)`, or `None` if the glyph has no geometry.
fn glyph_bbox_units(glyph: &GlyphShape) -> Option<(i32, i32, i32, i32)> {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for contour in glyph_contours(glyph) {
        for (x, y) in contour {
            min_x = min_x.min(x.floor() as i32);
            min_y = min_y.min(y.floor() as i32);
            max_x = max_x.max(x.ceil() as i32);
            max_y = max_y.max(y.ceil() as i32);
        }
    }
    if max_x >= min_x && max_y >= min_y {
        Some((min_x, min_y, max_x, max_y))
    } else {
        None
    }
}

/// Rasterize a single glyph shape at `scale`, using `advance` (already in px) for
/// the returned metrics. `None` if the glyph has no fillable geometry.
fn rasterize_shape(glyph: &GlyphShape, scale: f32, advance: f32) -> Option<GlyphBitmap> {
    let contours = glyph_contours(glyph);
    if contours.is_empty() {
        return None;
    }

    // Scale to px and find the ink bbox.
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for contour in &contours {
        for &(x, y) in contour {
            let px = x * scale;
            let py = y * scale;
            min_x = min_x.min(px);
            min_y = min_y.min(py);
            max_x = max_x.max(px);
            max_y = max_y.max(py);
        }
    }
    if !(min_x.is_finite() && max_x > min_x && max_y > min_y) {
        return None;
    }

    let left = min_x.floor() as i32;
    let top = min_y.floor() as i32;
    let width = (max_x.ceil() as i32 - left).max(1) as u32;
    let height = (max_y.ceil() as i32 - top).max(1) as u32;
    let ox = left as f32;
    let oy = top as f32;

    // Build bitmap-local edges (skip horizontal segments; they never cross a
    // scanline). Contours auto-close.
    let mut edges: Vec<Edge> = Vec::new();
    for contour in &contours {
        let n = contour.len();
        for i in 0..n {
            let (ax, ay) = contour[i];
            let (bx, by) = contour[(i + 1) % n];
            let x0 = ax * scale - ox;
            let y0 = ay * scale - oy;
            let x1 = bx * scale - ox;
            let y1 = by * scale - oy;
            if (y0 - y1).abs() < f32::EPSILON {
                continue;
            }
            let dir = if y1 > y0 { 1 } else { -1 };
            edges.push(Edge { x0, y0, x1, y1, dir });
        }
    }
    if edges.is_empty() {
        return None;
    }

    // Coverage accumulation buffer (f32 in 0..=1), then quantized to u8.
    let mut acc = vec![0.0f32; (width * height) as usize];
    let sub_w = 1.0 / SUBSAMPLES_Y as f32;
    let mut crossings: Vec<(f32, i32)> = Vec::new();

    for row in 0..height {
        for s in 0..SUBSAMPLES_Y {
            let sy = row as f32 + (s as f32 + 0.5) * sub_w;
            crossings.clear();
            for e in &edges {
                let (lo, hi) = if e.y0 < e.y1 {
                    (e.y0, e.y1)
                } else {
                    (e.y1, e.y0)
                };
                if sy < lo || sy >= hi {
                    continue;
                }
                let t = (sy - e.y0) / (e.y1 - e.y0);
                let x = e.x0 + t * (e.x1 - e.x0);
                crossings.push((x, e.dir));
            }
            if crossings.len() < 2 {
                continue;
            }
            crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut winding = 0;
            for k in 0..crossings.len() - 1 {
                winding += crossings[k].1;
                if winding != 0 {
                    add_span_coverage(
                        &mut acc,
                        width,
                        row,
                        crossings[k].0,
                        crossings[k + 1].0,
                        sub_w,
                    );
                }
            }
        }
    }

    let coverage: Vec<u8> = acc
        .iter()
        .map(|&c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
        .collect();

    Some(GlyphBitmap {
        width,
        height,
        left,
        top,
        advance,
        coverage,
    })
}

/// Add the coverage of an inside span `[xa, xb)` on `row` to `acc`, weighted by
/// `weight` (the sub-scanline's share). Each pixel column gets its exact overlap
/// with the span.
fn add_span_coverage(acc: &mut [f32], width: u32, row: u32, xa: f32, xb: f32, weight: f32) {
    let (xa, xb) = if xa <= xb { (xa, xb) } else { (xb, xa) };
    let xa = xa.max(0.0);
    let xb = xb.min(width as f32);
    if xb <= xa {
        return;
    }
    let first = xa.floor() as u32;
    let last = (xb.ceil() as u32).min(width);
    let base = (row * width) as usize;
    for col in first..last {
        let cell_lo = col as f32;
        let cell_hi = cell_lo + 1.0;
        let overlap = (xb.min(cell_hi) - xa.max(cell_lo)).max(0.0);
        if overlap > 0.0 {
            acc[base + col as usize] += overlap * weight;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Font3Layout;
    use crate::Rect;

    /// A filled box glyph from `(0,0)` to `(w,h)` in font units, wound clockwise
    /// (SWF y-down): down the left edge, right along the bottom, up the right,
    /// left along the top. One `DefineFont3` with a single glyph mapped to `ch`.
    fn box_font(ch: char, w: i32, h: i32) -> Tag {
        // Contour: moveTo(0,0) -> (0,h) -> (w,h) -> (w,0) -> close.
        let records = vec![
            ShapeRecord::StyleChange {
                flags: 0,
                move_to: Some(MoveTo { num_bits: 0, dx: 0, dy: 0 }),
                fill_style0: None,
                fill_style1: Some(1),
                line_style: None,
                new_styles: None,
            },
            ShapeRecord::StraightEdge { num_bits: 0, edge: StraightEdge::Vertical { dy: h } },
            ShapeRecord::StraightEdge { num_bits: 0, edge: StraightEdge::Horizontal { dx: w } },
            ShapeRecord::StraightEdge { num_bits: 0, edge: StraightEdge::Vertical { dy: -h } },
            ShapeRecord::End,
        ];
        let glyph = GlyphShape { num_fill_bits: 1, num_line_bits: 0, records };
        Tag::DefineFont3 {
            font_id: 7,
            flags: 0,
            language_code: 0,
            font_name: b"Test\0".to_vec(),
            offsets: vec![0, 0],
            glyphs: vec![glyph],
            codes: vec![ch as u16],
            layout: Some(Font3Layout {
                ascent: h as i16,
                descent: 0,
                leading: 0,
                advance: vec![(w as f32 * 1.2) as i16],
                bounds: vec![Rect { nbits: 16, x_min: 0, x_max: w, y_min: 0, y_max: h }],
                kernings: Vec::new(),
            }),
            force_long: false,
        }
    }

    #[test]
    fn builds_only_from_define_font3() {
        assert!(RasterFont::from_define_font3(&Tag::End).is_none());
        let font = RasterFont::from_define_font3(&box_font('A', 500, 700)).unwrap();
        assert_eq!(font.font_id, 7);
        assert_eq!(font.glyph_count(), 1);
        assert!(font.has_layout());
    }

    #[test]
    fn glyph_index_maps_char() {
        let font = RasterFont::from_define_font3(&box_font('Q', 500, 700)).unwrap();
        assert_eq!(font.glyph_index('Q'), Some(0));
        assert_eq!(font.glyph_index('Z'), None);
    }

    #[test]
    fn box_glyph_is_fully_covered_interior() {
        let font = RasterFont::from_define_font3(&box_font('A', 400, 800)).unwrap();
        // Scale the 800-unit em to ~20px.
        let scale = font.scale_for_em_px(20.0);
        let g = font.rasterize('A', scale).unwrap();
        assert!(g.width >= 8 && g.height >= 16, "size {}x{}", g.width, g.height);
        // Interior texel is fully covered.
        let cx = g.width / 2;
        let cy = g.height / 2;
        let center = g.coverage[(cy * g.width + cx) as usize];
        assert_eq!(center, 255, "interior should be solid, got {center}");
        // The bitmap is a solid box: nearly every texel covered.
        let covered = g.coverage.iter().filter(|&&c| c > 200).count();
        let total = g.coverage.len();
        assert!(
            covered * 100 / total >= 80,
            "expected mostly-covered box, {covered}/{total}"
        );
    }

    #[test]
    fn glyph_top_is_above_baseline() {
        // The box spans y=0 (baseline) up to y=h... in SWF y-down the box we drew
        // goes from 0 to +h, i.e. below baseline. Draw an ascender box from -h..0.
        let records = vec![
            ShapeRecord::StyleChange {
                flags: 0,
                move_to: Some(MoveTo { num_bits: 0, dx: 0, dy: -700 }),
                fill_style0: None,
                fill_style1: Some(1),
                line_style: None,
                new_styles: None,
            },
            ShapeRecord::StraightEdge { num_bits: 0, edge: StraightEdge::Vertical { dy: 700 } },
            ShapeRecord::StraightEdge { num_bits: 0, edge: StraightEdge::Horizontal { dx: 400 } },
            ShapeRecord::StraightEdge { num_bits: 0, edge: StraightEdge::Vertical { dy: -700 } },
            ShapeRecord::End,
        ];
        let glyph = GlyphShape { num_fill_bits: 1, num_line_bits: 0, records };
        let tag = Tag::DefineFont3 {
            font_id: 7,
            flags: 0,
            language_code: 0,
            font_name: b"T\0".to_vec(),
            offsets: vec![0, 0],
            glyphs: vec![glyph],
            codes: vec!['I' as u16],
            layout: Some(Font3Layout {
                ascent: 700,
                descent: 0,
                leading: 0,
                advance: vec![480],
                bounds: vec![Rect { nbits: 16, x_min: 0, x_max: 400, y_min: -700, y_max: 0 }],
                kernings: Vec::new(),
            }),
            force_long: false,
        };
        let font = RasterFont::from_define_font3(&tag).unwrap();
        let scale = font.scale_for_em_px(24.0);
        let g = font.rasterize('I', scale).unwrap();
        // Ink is above the baseline -> top is negative.
        assert!(g.top < 0, "top should be above baseline, got {}", g.top);
        assert!(g.advance > 0.0);
    }

    #[test]
    fn space_like_unmapped_char_has_advance_but_no_bitmap() {
        let font = RasterFont::from_define_font3(&box_font('A', 400, 800)).unwrap();
        let scale = font.scale_for_em_px(20.0);
        assert!(font.rasterize(' ', scale).is_none());
        assert!(font.advance_px(' ', scale) > 0.0);
    }
}
