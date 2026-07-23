//! Pure-CPU raster compositing onto a [`DdsImage`] RGBA8 buffer: solid rectangle
//! fills and 8-bit **coverage** blits (the output form a glyph rasterizer
//! produces). This is deliberately renderer-agnostic and game-free -- it only
//! reads/writes the `width * height * 4` pixel buffer -- so the stats-panel
//! composition (neutral background + text) is fully host-testable offline before
//! any of it is registered into the live texture repositories.
//!
//! Coverage is the natural boundary between a font rasterizer (which knows glyph
//! geometry but not color) and the panel builder (which knows color but not
//! geometry): [`DdsImage::blit_coverage`] takes an 8-bit alpha-coverage bitmap
//! plus a solid RGBA color and alpha-composites `color` over the destination,
//! scaling `color`'s own alpha by the per-texel coverage. Straight-alpha
//! `src-over` blending, matching what a Scaleform text field composites.

use super::{DdsImage, RGBA8_BYTES_PER_PIXEL};

impl DdsImage {
    /// Alpha-composite `color` over the destination texel at `(x, y)` using
    /// straight-alpha `src-over`. `alpha` is the *effective* source alpha in
    /// `0..=255` (already the product of the color alpha and any coverage). Out
    /// of bounds or `alpha == 0` is a no-op. `alpha == 255` is an exact
    /// overwrite (no rounding drift).
    pub fn blend_px(&mut self, x: i32, y: i32, color: [u8; 4], alpha: u8) {
        if alpha == 0 || x < 0 || y < 0 || x as u32 >= self.width || y as u32 >= self.height {
            return;
        }
        let idx = ((y as u32 * self.width + x as u32) as usize) * RGBA8_BYTES_PER_PIXEL;
        if alpha == 255 {
            self.pixels[idx] = color[0];
            self.pixels[idx + 1] = color[1];
            self.pixels[idx + 2] = color[2];
            self.pixels[idx + 3] = 255;
            return;
        }
        let a = alpha as u32;
        let inv = 255 - a;
        // Round-to-nearest integer src-over on each RGB channel; the result
        // alpha is the standard a + dst_a*(1-a) so blitting onto a transparent
        // background accumulates opacity correctly.
        for c in 0..3 {
            let src = color[c] as u32;
            let dst = self.pixels[idx + c] as u32;
            self.pixels[idx + c] = ((src * a + dst * inv + 127) / 255) as u8;
        }
        let dst_a = self.pixels[idx + 3] as u32;
        self.pixels[idx + 3] = (a + (dst_a * inv + 127) / 255) as u8;
    }

    /// Fill the axis-aligned rectangle `[x, x+w) x [y, y+h)` with solid opaque
    /// `rgba` (an exact overwrite, alpha included). The rectangle is clipped to
    /// the image bounds; a rectangle fully outside is a no-op. Negative `x`/`y`
    /// are supported (the visible sub-rect is filled).
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, rgba: [u8; 4]) {
        let x0 = x.max(0) as u32;
        let y0 = y.max(0) as u32;
        let x1 = ((x as i64 + w as i64).clamp(0, self.width as i64)) as u32;
        let y1 = ((y as i64 + h as i64).clamp(0, self.height as i64)) as u32;
        for py in y0..y1 {
            let row = (py * self.width) as usize * RGBA8_BYTES_PER_PIXEL;
            for px in x0..x1 {
                let idx = row + px as usize * RGBA8_BYTES_PER_PIXEL;
                self.pixels[idx] = rgba[0];
                self.pixels[idx + 1] = rgba[1];
                self.pixels[idx + 2] = rgba[2];
                self.pixels[idx + 3] = rgba[3];
            }
        }
    }

    /// Composite an 8-bit **coverage** bitmap (`cov_w * cov_h`, row-major, one
    /// byte per texel in `0..=255`) at destination origin `(x, y)` using solid
    /// `color`. Each destination texel is `src-over`-blended with an effective
    /// alpha of `coverage * color_alpha / 255`, so full coverage paints the
    /// color at its own alpha and zero coverage is untouched. This is the blit a
    /// font rasterizer feeds: it produces coverage (glyph shape), the caller
    /// picks the color. Clipped to image bounds; a `coverage` shorter than
    /// `cov_w * cov_h` stops early (defensive, never panics).
    pub fn blit_coverage(
        &mut self,
        x: i32,
        y: i32,
        cov_w: u32,
        cov_h: u32,
        coverage: &[u8],
        color: [u8; 4],
    ) {
        let color_a = color[3] as u32;
        for gy in 0..cov_h {
            for gx in 0..cov_w {
                let ci = (gy * cov_w + gx) as usize;
                let Some(&cov) = coverage.get(ci) else {
                    return;
                };
                if cov == 0 {
                    continue;
                }
                let eff = ((cov as u32 * color_a + 127) / 255) as u8;
                self.blend_px(x + gx as i32, y + gy as i32, color, eff);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_rect_solid_overwrites_exact_bytes() {
        let mut img = DdsImage::solid(4, 4, [0, 0, 0, 0]);
        img.fill_rect(1, 1, 2, 2, [10, 20, 30, 255]);
        // Corner (0,0) untouched.
        assert_eq!(&img.pixels[0..4], &[0, 0, 0, 0]);
        // (1,1) filled.
        let idx = ((1 * 4 + 1) * 4) as usize;
        assert_eq!(&img.pixels[idx..idx + 4], &[10, 20, 30, 255]);
        // (3,3) outside the 1..3 range, untouched.
        let idx = ((3 * 4 + 3) * 4) as usize;
        assert_eq!(&img.pixels[idx..idx + 4], &[0, 0, 0, 0]);
    }

    #[test]
    fn fill_rect_clips_to_bounds_without_panic() {
        let mut img = DdsImage::solid(3, 3, [1, 1, 1, 1]);
        // Overlapping the top-left corner with a negative origin.
        img.fill_rect(-2, -2, 4, 4, [9, 9, 9, 255]);
        // (0,0)..(1,1) covered; (2,2) not.
        assert_eq!(&img.pixels[0..4], &[9, 9, 9, 255]);
        let idx = ((1 * 3 + 1) * 4) as usize;
        assert_eq!(&img.pixels[idx..idx + 4], &[9, 9, 9, 255]);
        let idx = ((2 * 3 + 2) * 4) as usize;
        assert_eq!(&img.pixels[idx..idx + 4], &[1, 1, 1, 1]);
        // Fully outside is a no-op.
        img.fill_rect(100, 100, 5, 5, [7, 7, 7, 255]);
    }

    #[test]
    fn blend_px_full_alpha_is_exact_overwrite() {
        let mut img = DdsImage::solid(1, 1, [0, 0, 0, 0]);
        img.blend_px(0, 0, [200, 100, 50, 255], 255);
        assert_eq!(&img.pixels[0..4], &[200, 100, 50, 255]);
    }

    #[test]
    fn blend_px_half_alpha_mixes_toward_source() {
        // 50% white over black -> ~128 grey, alpha accumulates to ~128.
        let mut img = DdsImage::solid(1, 1, [0, 0, 0, 0]);
        img.blend_px(0, 0, [255, 255, 255, 255], 128);
        let p = &img.pixels[0..4];
        assert!((p[0] as i32 - 128).abs() <= 1, "r={}", p[0]);
        assert!((p[3] as i32 - 128).abs() <= 1, "a={}", p[3]);
    }

    #[test]
    fn blend_px_out_of_bounds_is_noop() {
        let mut img = DdsImage::solid(2, 2, [5, 5, 5, 5]);
        img.blend_px(-1, 0, [9, 9, 9, 255], 255);
        img.blend_px(0, 2, [9, 9, 9, 255], 255);
        img.blend_px(9, 9, [9, 9, 9, 255], 255);
        assert!(img.pixels.iter().all(|&b| b == 5));
    }

    #[test]
    fn blit_coverage_paints_shape_with_color() {
        let mut img = DdsImage::solid(3, 1, [0, 0, 0, 255]);
        // Coverage: left texel full, middle half, right zero.
        let cov = [255u8, 128, 0];
        img.blit_coverage(0, 0, 3, 1, &cov, [255, 0, 0, 255]);
        // Left fully red.
        assert_eq!(&img.pixels[0..4], &[255, 0, 0, 255]);
        // Middle ~half red over black.
        let mid = &img.pixels[4..8];
        assert!((mid[0] as i32 - 128).abs() <= 2, "mid r={}", mid[0]);
        // Right untouched (still black).
        assert_eq!(&img.pixels[8..12], &[0, 0, 0, 255]);
    }

    #[test]
    fn blit_coverage_respects_color_alpha() {
        // Full coverage but a 50%-alpha color -> half blend.
        let mut img = DdsImage::solid(1, 1, [0, 0, 0, 255]);
        img.blit_coverage(0, 0, 1, 1, &[255], [255, 255, 255, 128]);
        let p = &img.pixels[0..4];
        assert!((p[0] as i32 - 128).abs() <= 2, "r={}", p[0]);
    }

    #[test]
    fn blit_coverage_short_buffer_stops_early() {
        let mut img = DdsImage::solid(2, 2, [0, 0, 0, 255]);
        // Only one coverage byte for a 2x2 request: paints (0,0), then returns.
        img.blit_coverage(0, 0, 2, 2, &[255], [1, 2, 3, 255]);
        assert_eq!(&img.pixels[0..4], &[1, 2, 3, 255]);
        assert_eq!(&img.pixels[4..8], &[0, 0, 0, 255]);
    }
}
