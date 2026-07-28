//! Backend-neutral loading-bar primitives.
//!
//! This crate deliberately owns only the small pieces that do not need Elden
//! Ring, Win32, D3D12, hooks, save picking, portrait replacement, or product
//! autoload state: phase labels, the uppercase 5x7 font, text measurement, and
//! tight RGBA8 raster helpers. Runtime adapters decide when and where to draw.

#![cfg_attr(not(windows), forbid(unsafe_code))]

#[cfg(windows)]
pub mod d3d12_compositor;

/// Tight RGBA8 byte stride used by this crate's CPU-side frame helpers.
pub const RGBA8_BPP: usize = 4;
/// Embedded glyph width in pixels before scaling.
pub const GLYPH_W: usize = 5;
/// Embedded glyph height in pixels before scaling.
pub const GLYPH_H: usize = 7;
/// Character advance in pixels before scaling: 5px glyph plus 1px gap.
pub const GLYPH_ADV: usize = 6;

/// Number of top-level loading phases in the standalone loading-bar model.
pub const PHASE_COUNT: usize = 12;

/// Left-aligned phase labels for the product loading bar.
pub const PHASE_LABELS: [&str; PHASE_COUNT] = [
    "STARTING UP",
    "GAME SYSTEMS",
    "ACQUIRING ASSETS",
    "OPENING MENU UI",
    "BUILDING MENU UI",
    "TITLE READY",
    "PREPARING SAVE",
    "LOADING SAVE",
    "BUILDING WORLD",
    "STREAMING WORLD",
    "FINALIZING WORLD",
    "ENTERING WORLD",
];

/// Phase fill targets in permille. Runtime adapters may force a higher fill
/// when a stronger native gauge or handoff semaphore is available, but they
/// should not move a displayed phase backwards.
pub const PHASE_PERMILLE: [usize; PHASE_COUNT] =
    [30, 80, 150, 220, 290, 360, 440, 520, 610, 730, 860, 950];

/// A single phase/subphase label suitable for the visible shape
/// `<main> N/M (<sub> X/Y)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadingLabel {
    pub main_label: &'static str,
    pub main_index: usize,
    pub main_total: usize,
    pub sub_label: &'static str,
    pub sub_index: usize,
    pub sub_total: usize,
}

impl LoadingLabel {
    pub fn new(
        main_label: &'static str,
        main_index: usize,
        main_total: usize,
        sub_label: &'static str,
        sub_index: usize,
        sub_total: usize,
    ) -> Self {
        Self {
            main_label,
            main_index,
            main_total,
            sub_label,
            sub_index,
            sub_total,
        }
    }

    /// Build the label text shown above the bar.
    pub fn write_text(self, out: &mut String) {
        self.write_text_with_sub_suffix(out, "");
    }

    /// Build the label text and append an already-formatted suffix inside the
    /// subphase parentheses. Runtime adapters use this for transient native FSM
    /// detail such as ` - SAVE RESIDENT` without changing the base shape.
    pub fn write_text_with_sub_suffix(self, out: &mut String, sub_suffix: &str) {
        use core::fmt::Write as _;
        let _ = write!(
            out,
            "{} {}/{} ({} {}/{}{})",
            self.main_label,
            self.main_index,
            self.main_total,
            self.sub_label,
            self.sub_index,
            self.sub_total,
            sub_suffix
        );
    }
}

/// Phase table lookup clamped to the final phase.
pub fn phase_label(idx: usize) -> &'static str {
    PHASE_LABELS[idx.min(PHASE_COUNT.saturating_sub(1))]
}

/// Phase target lookup clamped to the final phase.
pub fn phase_permille(idx: usize) -> usize {
    PHASE_PERMILLE[idx.min(PHASE_COUNT.saturating_sub(1))]
}

/// True when every glyph in `text` is represented by the embedded 5x7 font.
pub fn is_supported_text(text: &str) -> bool {
    text.chars()
        .all(|c| glyph_5x7(c) != [0; GLYPH_H] || c == ' ')
}

/// Width in pixels for the embedded font at `scale`.
pub fn text_width(text: &str, scale: usize) -> usize {
    text.chars().count() * GLYPH_ADV * scale
}

/// Return the 5x7 row bitmap for a supported glyph.
pub fn glyph_5x7(c: char) -> [u8; GLYPH_H] {
    match c {
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e],
        'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        'G' => [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0e],
        'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'I' => [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        'J' => [0x01, 0x01, 0x01, 0x01, 0x11, 0x11, 0x0e],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d],
        'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1b, 0x11],
        'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c],
        '-' => [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1f],
        '/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
        '\\' => [0x10, 0x08, 0x08, 0x04, 0x02, 0x02, 0x01],
        ':' => [0x00, 0x0c, 0x0c, 0x00, 0x0c, 0x0c, 0x00],
        '[' => [0x0e, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0e],
        ']' => [0x0e, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0e],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '>' => [0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08],
        '?' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
        '!' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1f],
        '3' => [0x0e, 0x11, 0x01, 0x06, 0x01, 0x11, 0x0e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        '5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        '6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        '%' => [0x19, 0x19, 0x02, 0x04, 0x08, 0x13, 0x13],
        ' ' => [0; GLYPH_H],
        _ => [0; GLYPH_H],
    }
}

/// Blit `text` into a tight RGBA buffer at `(x, y)`, scaled by `scale`.
pub fn draw_text_rgb(
    buf: &mut [u8],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    text: &str,
    rgb: [u8; 3],
    scale: usize,
) {
    let mut cx = x;
    for c in text.chars() {
        let rows = glyph_5x7(c);
        for (gy, row) in rows.iter().enumerate() {
            for gx in 0..GLYPH_W {
                if row & (1 << (GLYPH_W - 1 - gx)) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = cx + gx * scale + sx;
                        let py = y + gy * scale + sy;
                        if px < w && py < h {
                            let o = (py * w + px) * RGBA8_BPP;
                            if o + RGBA8_BPP <= buf.len() {
                                buf[o] = rgb[0];
                                buf[o + 1] = rgb[1];
                                buf[o + 2] = rgb[2];
                                buf[o + 3] = 255;
                            }
                        }
                    }
                }
            }
        }
        cx += GLYPH_ADV * scale;
    }
}

/// Axis-aligned opaque fill into a tight RGBA buffer. Coordinates are clamped.
pub fn fill_rect_rgb(
    buf: &mut [u8],
    w: usize,
    h: usize,
    x0: usize,
    y0: usize,
    rw: usize,
    rh: usize,
    rgb: [u8; 3],
) {
    for y in y0..(y0 + rh).min(h) {
        for x in x0..(x0 + rw).min(w) {
            let o = (y * w + x) * RGBA8_BPP;
            if o + RGBA8_BPP <= buf.len() {
                buf[o] = rgb[0];
                buf[o + 1] = rgb[1];
                buf[o + 2] = rgb[2];
                buf[o + 3] = 255;
            }
        }
    }
}

/// Backend-neutral visual style for a plain label-plus-bar frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BarStyle {
    pub background_rgb: [u8; 3],
    pub track_rgb: [u8; 3],
    pub fill_rgb: [u8; 3],
    pub text_rgb: [u8; 3],
    pub bar_height: usize,
    pub text_bar_gap: usize,
    pub pad_bottom: usize,
}

impl Default for BarStyle {
    fn default() -> Self {
        Self {
            background_rgb: [0, 0, 0],
            track_rgb: [26, 26, 26],
            fill_rgb: [226, 223, 214],
            text_rgb: [150, 147, 138],
            bar_height: 3,
            text_bar_gap: 5,
            pad_bottom: 3,
        }
    }
}

/// A tight RGBA8 CPU frame. Runtime adapters upload or composite this however
/// their backend requires.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RgbaFrame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// Height needed for a single label row and bar at `text_scale`.
pub fn label_bar_frame_height(text_scale: usize, style: BarStyle) -> usize {
    GLYPH_H * text_scale + style.text_bar_gap + style.bar_height + style.pad_bottom
}

/// Render a plain black-backed loading-bar strip. It intentionally has no
/// portrait, background screenshot, D3D12 object, hook state, or runtime
/// semaphore dependency.
pub fn render_label_bar_frame(
    width: usize,
    text_scale: usize,
    label: &str,
    progress_permille: usize,
    style: BarStyle,
) -> RgbaFrame {
    let height = label_bar_frame_height(text_scale, style);
    let mut pixels = vec![0; width.saturating_mul(height).saturating_mul(RGBA8_BPP)];
    fill_rect_rgb(
        &mut pixels,
        width,
        height,
        0,
        0,
        width,
        height,
        style.background_rgb,
    );
    draw_text_rgb(
        &mut pixels,
        width,
        height,
        0,
        0,
        label,
        style.text_rgb,
        text_scale,
    );
    let bar_y = GLYPH_H * text_scale + style.text_bar_gap;
    fill_rect_rgb(
        &mut pixels,
        width,
        height,
        0,
        bar_y,
        width,
        style.bar_height,
        style.track_rgb,
    );
    let fill_w = width.saturating_mul(progress_permille.min(1000)) / 1000;
    fill_rect_rgb(
        &mut pixels,
        width,
        height,
        0,
        bar_y,
        fill_w,
        style.bar_height,
        style.fill_rgb,
    );
    RgbaFrame {
        width,
        height,
        pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_tables_are_aligned_and_monotonic() {
        assert_eq!(PHASE_LABELS.len(), PHASE_COUNT);
        assert_eq!(PHASE_PERMILLE.len(), PHASE_COUNT);
        assert!(PHASE_PERMILLE.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(PHASE_PERMILLE[PHASE_COUNT - 1] < 1000);
    }

    #[test]
    fn phase_labels_fit_the_embedded_font_contract() {
        for label in PHASE_LABELS {
            assert!(is_supported_text(label), "unsupported phase label: {label}");
            assert_eq!(label, label.to_ascii_uppercase());
        }
    }

    #[test]
    fn label_text_uses_required_main_and_sub_shape() {
        let label = LoadingLabel::new("LOADING SAVE", 8, 12, "STEP INIT", 1, 4);
        let mut text = String::new();
        label.write_text(&mut text);
        assert_eq!(text, "LOADING SAVE 8/12 (STEP INIT 1/4)");
        assert!(is_supported_text(&text));
        text.clear();
        label.write_text_with_sub_suffix(&mut text, " - SAVE RESIDENT");
        assert_eq!(text, "LOADING SAVE 8/12 (STEP INIT 1/4 - SAVE RESIDENT)");
    }

    #[test]
    fn text_width_uses_fixed_advance() {
        assert_eq!(text_width("ABC", 1), 18);
        assert_eq!(text_width("ABC", 2), 36);
    }

    #[test]
    fn draw_text_sets_only_clamped_pixels() {
        let mut buf = vec![0; 8 * 8 * RGBA8_BPP];
        draw_text_rgb(&mut buf, 8, 8, 0, 0, "A", [1, 2, 3], 2);
        assert!(buf.chunks_exact(RGBA8_BPP).any(|px| px == [1, 2, 3, 255]));
        assert_eq!(buf.len(), 8 * 8 * RGBA8_BPP);
    }

    #[test]
    fn fill_rect_is_clamped() {
        let mut buf = vec![0; 3 * 3 * RGBA8_BPP];
        fill_rect_rgb(&mut buf, 3, 3, 2, 2, 8, 8, [9, 8, 7]);
        let lit = buf
            .chunks_exact(RGBA8_BPP)
            .filter(|px| *px == [9, 8, 7, 255])
            .count();
        assert_eq!(lit, 1);
    }

    #[test]
    fn label_bar_frame_has_expected_geometry_and_fill() {
        let style = BarStyle::default();
        let frame = render_label_bar_frame(100, 2, "LOADING SAVE", 250, style);
        assert_eq!(frame.width, 100);
        assert_eq!(frame.height, label_bar_frame_height(2, style));
        assert_eq!(frame.pixels.len(), frame.width * frame.height * RGBA8_BPP);

        let bar_y = GLYPH_H * 2 + style.text_bar_gap;
        let row = &frame.pixels
            [(bar_y * frame.width * RGBA8_BPP)..((bar_y + 1) * frame.width * RGBA8_BPP)];
        let fill_pixels = row
            .chunks_exact(RGBA8_BPP)
            .filter(|px| *px == [226, 223, 214, 255])
            .count();
        let track_pixels = row
            .chunks_exact(RGBA8_BPP)
            .filter(|px| *px == [26, 26, 26, 255])
            .count();
        assert_eq!(fill_pixels, 25);
        assert_eq!(track_pixels, 75);
    }
}
