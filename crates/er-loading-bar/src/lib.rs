//! Backend-neutral loading-bar primitives.
//!
//! This crate deliberately owns only the small pieces that do not need Elden
//! Ring, Win32, D3D12, hooks, save picking, portrait replacement, or product
//! autoload state: phase labels, the uppercase 5x7 font, text measurement, and
//! tight RGBA8 raster helpers. Runtime adapters decide when and where to draw.

#![forbid(unsafe_code)]

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

/// Identity of a single loading phase, independent of which epoch's sequence contains it.
///
/// A load "epoch" is one arm-to-teardown lifetime of the bar. The phases a process boot walks
/// through are NOT the phases a later character reload walks through: a reload cannot start the
/// engine, construct GameMan, or acquire the title's Scaleform resources a second time. Naming the
/// phase (rather than indexing a single global table) is what lets each epoch publish a sequence
/// containing only the phases that can actually occur in it, so the visible `N/M` denominator is
/// honest instead of padded with phases that already happened in a previous epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadPhase {
    /// Process boot only: our present path is coming up.
    StartingUp,
    /// Process boot only: the engine's core managers are constructing.
    GameSystems,
    /// Process boot only: the title menu is acquiring its resources.
    AcquiringAssets,
    /// Process boot only: title Scaleform movies are opening.
    OpeningMenuUi,
    /// Process boot only: title Scaleform movies are building.
    BuildingMenuUi,
    /// Reload only: the user picked a slot and the switch was committed.
    SwitchConfirmed,
    /// Reload only: the live world is being torn down back to the title owner.
    ReturningToTitle,
    /// The title is up and owns the frame.
    TitleReady,
    /// The load request is armed and the save is being prepared.
    PreparingSave,
    /// The save is being read / the load has been confirmed.
    LoadingSave,
    /// The native loading screen is up; the world build has begun.
    BuildingWorld,
    /// The native world-load gauge is streaming.
    StreamingWorld,
    /// The native world-load gauge is past its midpoint.
    FinalizingWorld,
    /// The world is handing off to gameplay.
    EnteringWorld,
}

impl LoadPhase {
    /// Left-aligned visible label. Uppercase A-Z + space only (embedded 5x7 font contract).
    pub const fn label(self) -> &'static str {
        match self {
            Self::StartingUp => "STARTING UP",
            Self::GameSystems => "GAME SYSTEMS",
            Self::AcquiringAssets => "ACQUIRING ASSETS",
            Self::OpeningMenuUi => "OPENING MENU UI",
            Self::BuildingMenuUi => "BUILDING MENU UI",
            Self::SwitchConfirmed => "SWITCHING SAVE",
            Self::ReturningToTitle => "RETURNING TO TITLE",
            Self::TitleReady => "TITLE READY",
            Self::PreparingSave => "PREPARING SAVE",
            Self::LoadingSave => "LOADING SAVE",
            Self::BuildingWorld => "BUILDING WORLD",
            Self::StreamingWorld => "STREAMING WORLD",
            Self::FinalizingWorld => "FINALIZING WORLD",
            Self::EnteringWorld => "ENTERING WORLD",
        }
    }
}

/// One epoch's ordered phase sequence plus that sequence's own fill targets.
///
/// The permille targets belong to the SET, not to the phase: the same phase occupies a different
/// slice of 0..1000 depending on how many phases share the bar with it in that epoch.
pub struct PhaseSet {
    phases: &'static [LoadPhase],
    permille: &'static [usize],
}

impl PhaseSet {
    pub const fn len(&self) -> usize {
        self.phases.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.phases.is_empty()
    }

    /// The largest main index this set can display; also the `M` in the visible `N/M`.
    pub const fn main_total(&self) -> usize {
        self.phases.len() - 1
    }

    pub fn phase(&self, idx: usize) -> LoadPhase {
        self.phases[idx.min(self.main_total())]
    }

    pub fn label(&self, idx: usize) -> &'static str {
        self.phase(idx).label()
    }

    /// Fill target reached when this phase becomes the active one.
    pub fn base_permille(&self, idx: usize) -> usize {
        self.permille[idx.min(self.main_total())]
    }

    /// Fill target of the NEXT phase, i.e. the ceiling the active phase fills toward.
    /// The final phase fills toward a full bar.
    pub fn next_permille(&self, idx: usize) -> usize {
        let i = idx.min(self.main_total());
        if i + 1 < self.permille.len() {
            self.permille[i + 1]
        } else {
            1000
        }
    }

    pub fn index_of(&self, phase: LoadPhase) -> Option<usize> {
        let mut i = 0;
        while i < self.phases.len() {
            if self.phases[i] as usize == phase as usize {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    /// `base_permille` of `phase` when this set contains it, else 0.
    pub fn base_permille_of(&self, phase: LoadPhase) -> usize {
        match self.index_of(phase) {
            Some(i) => self.permille[i],
            None => 0,
        }
    }
}

/// Process-boot epoch: engine bring-up all the way through to the first world.
pub static BOOT_PHASE_SET: PhaseSet = PhaseSet {
    phases: &[
        LoadPhase::StartingUp,
        LoadPhase::GameSystems,
        LoadPhase::AcquiringAssets,
        LoadPhase::OpeningMenuUi,
        LoadPhase::BuildingMenuUi,
        LoadPhase::TitleReady,
        LoadPhase::PreparingSave,
        LoadPhase::LoadingSave,
        LoadPhase::BuildingWorld,
        LoadPhase::StreamingWorld,
        LoadPhase::FinalizingWorld,
        LoadPhase::EnteringWorld,
    ],
    permille: &PHASE_PERMILLE,
};

/// Character-reload epoch (System->Quit->Load Profile and the programmatic switch that mirrors it).
///
/// Deliberately excludes every boot-only phase: the engine is already up, GameMan already exists,
/// and the title's Scaleform resources are already resident, so those phases cannot recur and must
/// not sit in the denominator inflating a reload's reported progress.
pub static RELOAD_PHASE_SET: PhaseSet = PhaseSet {
    phases: &[
        LoadPhase::SwitchConfirmed,
        LoadPhase::ReturningToTitle,
        LoadPhase::TitleReady,
        LoadPhase::PreparingSave,
        LoadPhase::LoadingSave,
        LoadPhase::BuildingWorld,
        LoadPhase::StreamingWorld,
        LoadPhase::FinalizingWorld,
        LoadPhase::EnteringWorld,
    ],
    permille: &RELOAD_PHASE_PERMILLE,
};

/// Reload fill targets, weighted by where a reload actually spends its time rather than spread evenly
/// across the phase count. Measured over two reload epochs (run samechar-3x-threedll-20260730-083446,
/// epochs 1 and 2, rearm -> full bar in ~22s each): the five pre-world phases complete inside the first
/// ~0.8s (~4% of the load) while the world tail owns the remaining ~21s. An even spread therefore put
/// 45% of the bar behind the first second and left the long streaming stretch crawling through the top
/// half -- technically monotonic, but not paced. These targets track the measured wall-clock split:
/// BUILDING WORLD ~3%, STREAMING ~22%, FINALIZING ~48%, ENTERING WORLD ~91%.
pub const RELOAD_PHASE_PERMILLE: [usize; 9] = [15, 25, 35, 45, 55, 70, 240, 480, 900];

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
#[allow(
    clippy::too_many_arguments,
    reason = "raster primitive: destination buffer + its dimensions + position + payload + style are all irreducible"
)]
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
#[allow(
    clippy::too_many_arguments,
    reason = "raster primitive: destination buffer + its dimensions + rect + color are all irreducible"
)]
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

    const ALL_PHASES: [LoadPhase; 14] = [
        LoadPhase::StartingUp,
        LoadPhase::GameSystems,
        LoadPhase::AcquiringAssets,
        LoadPhase::OpeningMenuUi,
        LoadPhase::BuildingMenuUi,
        LoadPhase::SwitchConfirmed,
        LoadPhase::ReturningToTitle,
        LoadPhase::TitleReady,
        LoadPhase::PreparingSave,
        LoadPhase::LoadingSave,
        LoadPhase::BuildingWorld,
        LoadPhase::StreamingWorld,
        LoadPhase::FinalizingWorld,
        LoadPhase::EnteringWorld,
    ];

    #[test]
    fn every_phase_label_fits_the_embedded_font_contract() {
        for phase in ALL_PHASES {
            let label = phase.label();
            assert!(is_supported_text(label), "unsupported phase label: {label}");
            assert_eq!(label, label.to_ascii_uppercase());
        }
    }

    #[test]
    fn boot_phase_set_matches_the_legacy_flat_tables() {
        assert_eq!(BOOT_PHASE_SET.len(), PHASE_COUNT);
        for i in 0..PHASE_COUNT {
            assert_eq!(BOOT_PHASE_SET.label(i), PHASE_LABELS[i]);
            assert_eq!(BOOT_PHASE_SET.base_permille(i), PHASE_PERMILLE[i]);
        }
    }

    #[test]
    fn phase_sets_are_ordered_and_span_the_whole_bar() {
        for set in [&BOOT_PHASE_SET, &RELOAD_PHASE_SET] {
            assert!(!set.is_empty());
            assert_eq!(set.main_total(), set.len() - 1);
            for i in 0..set.len() {
                assert!(
                    set.base_permille(i) < set.next_permille(i),
                    "phase {i} ({}) has a non-advancing span",
                    set.label(i)
                );
            }
            // The last phase always fills toward a full bar.
            assert_eq!(set.next_permille(set.main_total()), 1000);
        }
    }

    /// The whole point of the reload set: a character reload cannot replay engine bring-up, so those
    /// phases must not appear in its sequence (and therefore not in its `N/M` denominator).
    #[test]
    fn reload_phase_set_excludes_boot_only_phases() {
        for boot_only in [
            LoadPhase::StartingUp,
            LoadPhase::GameSystems,
            LoadPhase::AcquiringAssets,
            LoadPhase::OpeningMenuUi,
            LoadPhase::BuildingMenuUi,
        ] {
            assert!(
                RELOAD_PHASE_SET.index_of(boot_only).is_none(),
                "{boot_only:?} cannot recur on a character reload"
            );
            assert!(BOOT_PHASE_SET.index_of(boot_only).is_some());
        }
        for reload_only in [LoadPhase::SwitchConfirmed, LoadPhase::ReturningToTitle] {
            assert!(RELOAD_PHASE_SET.index_of(reload_only).is_some());
            assert!(
                BOOT_PHASE_SET.index_of(reload_only).is_none(),
                "{reload_only:?} cannot occur during a process boot"
            );
        }
        // Both epochs share the world tail, and the reload denominator is strictly smaller.
        assert!(RELOAD_PHASE_SET.len() < BOOT_PHASE_SET.len());
        for shared in [
            LoadPhase::LoadingSave,
            LoadPhase::BuildingWorld,
            LoadPhase::StreamingWorld,
            LoadPhase::FinalizingWorld,
            LoadPhase::EnteringWorld,
        ] {
            assert!(RELOAD_PHASE_SET.index_of(shared).is_some());
            assert!(BOOT_PHASE_SET.index_of(shared).is_some());
        }
    }

    #[test]
    fn base_permille_of_resolves_the_world_tail_floor_per_epoch() {
        assert_eq!(
            BOOT_PHASE_SET.base_permille_of(LoadPhase::BuildingWorld),
            PHASE_PERMILLE[8]
        );
        assert_eq!(
            RELOAD_PHASE_SET.base_permille_of(LoadPhase::BuildingWorld),
            RELOAD_PHASE_PERMILLE[5]
        );
        // A phase the set does not contain has no floor to contribute.
        assert_eq!(RELOAD_PHASE_SET.base_permille_of(LoadPhase::StartingUp), 0);
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
        assert!(buf.as_chunks::<RGBA8_BPP>().0.contains(&[1, 2, 3, 255]));
        assert_eq!(buf.len(), 8 * 8 * RGBA8_BPP);
    }

    #[test]
    fn fill_rect_is_clamped() {
        let mut buf = vec![0; 3 * 3 * RGBA8_BPP];
        fill_rect_rgb(&mut buf, 3, 3, 2, 2, 8, 8, [9, 8, 7]);
        let lit = buf
            .as_chunks::<RGBA8_BPP>()
            .0
            .iter()
            .filter(|px| **px == [9, 8, 7, 255])
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
            .as_chunks::<RGBA8_BPP>()
            .0
            .iter()
            .filter(|px| **px == [226, 223, 214, 255])
            .count();
        let track_pixels = row
            .as_chunks::<RGBA8_BPP>()
            .0
            .iter()
            .filter(|px| **px == [26, 26, 26, 255])
            .count();
        assert_eq!(fill_pixels, 25);
        assert_eq!(track_pixels, 75);
    }
}
