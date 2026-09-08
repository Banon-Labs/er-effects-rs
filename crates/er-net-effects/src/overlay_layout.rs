//! Where the net-effects bar sits on screen, and what a mouse click on it hits.
//!
//! Ungated on purpose: pure geometry with no Windows or imgui types crossing the boundary, so
//! the right-edge anchor, the equal padding and the collapse button's hit box are proven by
//! `cargo test` on the host instead of by a game launch and a screenshot.

// Windows-only in practice; kept portable so the geometry below is asserted on the host.
#![cfg_attr(not(windows), allow(dead_code))]

/// Distance from the panel to each screen edge it is anchored to.
///
/// One number for both the top and the right edge on purpose: the bar should read as pinned to
/// the corner, and two different numbers read as a mistake even when neither is wrong.
pub(crate) const SCREEN_MARGIN: f32 = 48.0;

/// Border-to-text padding, identical on all four sides.
pub(crate) const PANEL_PADDING: f32 = 14.0;

/// Blank space between two text rows.
///
/// Deliberately not folded into a row's height: the old layout advanced by a 26px row for 13px
/// text, so the leftover 13px landed under the last row and the bottom margin was twice the top.
pub(crate) const ROW_GAP: f32 = 8.0;

/// Corner radius of the panel and of the toggle button drawn inside it.
pub(crate) const PANEL_ROUNDING: f32 = 6.0;

/// `[x0, y0, x1, y1]` in screen pixels.
pub(crate) type Rect = [f32; 4];

/// Is `point` inside `rect`?
///
/// A non-finite coordinate answers `false`: imgui parks the pointer at `-FLT_MAX` when the window
/// has no mouse, and that must never count as hovering the button.
pub(crate) fn rect_contains(rect: Rect, point: [f32; 2]) -> bool {
    point[0].is_finite()
        && point[1].is_finite()
        && point[0] >= rect[0]
        && point[0] <= rect[2]
        && point[1] >= rect[1]
        && point[1] <= rect[3]
}

/// The resolved geometry of one frame of the bar.
pub(crate) struct PanelLayout {
    /// The full background panel.
    pub(crate) panel: Rect,
    /// The clickable region that minimizes/maximizes the bar. Collapsed, this is the whole
    /// panel -- the bar is the button. Expanded, it is the header row only, so a click near the
    /// effect list cannot collapse the thing the player is reading.
    pub(crate) toggle: Rect,
    /// Left edge of every text row.
    pub(crate) text_x: f32,
    /// Right edge available to text, where the `[+]`/`[-]` marker ends.
    pub(crate) inner_right: f32,
    /// Top of the first (header) row.
    pub(crate) first_row_y: f32,
    /// Distance from one row's top to the next row's top.
    pub(crate) row_advance: f32,
}

/// Lay out a right-anchored panel that fits `content_width` of text across `rows` rows.
///
/// The panel is sized to its content rather than to a fixed width, which is what makes the left
/// and right padding equal instead of leaving a ragged gap on whichever side the text stops short.
pub(crate) fn panel_layout(
    display: [f32; 2],
    content_width: f32,
    row_height: f32,
    rows: usize,
) -> PanelLayout {
    let rows = rows.max(1);
    let display_width = display[0].max(1.0);
    // Never let the anchor push the panel off the left edge on a narrow display: the widest the
    // panel may get still leaves SCREEN_MARGIN on both sides.
    let max_width = (display_width - SCREEN_MARGIN * 2.0).max(PANEL_PADDING * 2.0);
    let width = (content_width.max(0.0) + PANEL_PADDING * 2.0).min(max_width);
    let row_height = row_height.max(1.0);
    let content_height = row_height * rows as f32 + ROW_GAP * (rows - 1) as f32;
    let height = content_height + PANEL_PADDING * 2.0;

    let x1 = display_width - SCREEN_MARGIN;
    let x0 = x1 - width;
    let y0 = SCREEN_MARGIN;
    let y1 = y0 + height;

    let toggle_bottom = if rows == 1 {
        y1
    } else {
        // Stop halfway down the gap under the header: the button covers its own row and no part
        // of the next one.
        y0 + PANEL_PADDING + row_height + ROW_GAP * 0.5
    };

    PanelLayout {
        panel: [x0, y0, x1, y1],
        toggle: [x0, y0, x1, toggle_bottom],
        text_x: x0 + PANEL_PADDING,
        inner_right: x1 - PANEL_PADDING,
        first_row_y: y0 + PANEL_PADDING,
        row_advance: row_height + ROW_GAP,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISPLAY: [f32; 2] = [1920.0, 1080.0];
    const ROW_HEIGHT: f32 = 16.25;

    fn assert_close(actual: f32, expected: f32, what: &str) {
        assert!(
            (actual - expected).abs() < 0.001,
            "{what}: expected {expected}, got {actual}"
        );
    }

    #[test]
    fn the_panel_is_anchored_to_the_right_edge() {
        let layout = panel_layout(DISPLAY, 400.0, ROW_HEIGHT, 4);
        assert_close(
            DISPLAY[0] - layout.panel[2],
            SCREEN_MARGIN,
            "right screen margin",
        );
        assert!(
            layout.panel[0] > DISPLAY[0] * 0.5,
            "a right-anchored panel starts in the right half of the screen, not at x={}",
            layout.panel[0]
        );
    }

    #[test]
    fn the_top_and_right_screen_margins_are_the_same() {
        let layout = panel_layout(DISPLAY, 400.0, ROW_HEIGHT, 4);
        assert_close(layout.panel[1], SCREEN_MARGIN, "top screen margin");
        assert_close(
            DISPLAY[0] - layout.panel[2],
            SCREEN_MARGIN,
            "right screen margin",
        );
    }

    #[test]
    fn all_four_paddings_are_equal() {
        let content_width = 400.0;
        let rows = 4;
        let layout = panel_layout(DISPLAY, content_width, ROW_HEIGHT, rows);

        assert_close(layout.text_x - layout.panel[0], PANEL_PADDING, "left pad");
        assert_close(
            layout.panel[2] - (layout.text_x + content_width),
            PANEL_PADDING,
            "right pad",
        );
        assert_close(
            layout.first_row_y - layout.panel[1],
            PANEL_PADDING,
            "top pad",
        );

        let last_row_bottom =
            layout.first_row_y + layout.row_advance * (rows - 1) as f32 + ROW_HEIGHT;
        assert_close(
            layout.panel[3] - last_row_bottom,
            PANEL_PADDING,
            "bottom pad",
        );
    }

    #[test]
    fn the_bottom_pad_stays_equal_at_every_row_count() {
        for rows in 1..8 {
            let layout = panel_layout(DISPLAY, 400.0, ROW_HEIGHT, rows);
            let last_row_bottom =
                layout.first_row_y + layout.row_advance * (rows - 1) as f32 + ROW_HEIGHT;
            assert_close(
                layout.panel[3] - last_row_bottom,
                PANEL_PADDING,
                &format!("bottom pad at {rows} rows"),
            );
        }
    }

    #[test]
    fn collapsed_the_whole_panel_is_the_button() {
        let layout = panel_layout(DISPLAY, 160.0, ROW_HEIGHT, 1);
        assert_eq!(
            layout.toggle, layout.panel,
            "a one-row bar IS the button, so its hit box must be the panel"
        );
    }

    #[test]
    fn expanded_the_button_covers_the_header_row_only() {
        let rows = 4;
        let layout = panel_layout(DISPLAY, 400.0, ROW_HEIGHT, rows);
        let header_centre = [
            (layout.panel[0] + layout.panel[2]) * 0.5,
            layout.first_row_y + ROW_HEIGHT * 0.5,
        ];
        let second_row_centre = [header_centre[0], layout.first_row_y + layout.row_advance];

        assert!(
            rect_contains(layout.toggle, header_centre),
            "the header row must be clickable"
        );
        assert!(
            !rect_contains(layout.toggle, second_row_centre),
            "clicking the effect list must not collapse the bar"
        );
        assert!(
            layout.toggle[3] < layout.panel[3],
            "the expanded button must be shorter than the panel"
        );
    }

    #[test]
    fn a_narrow_display_keeps_the_panel_on_screen() {
        let display = [640.0, 480.0];
        let layout = panel_layout(display, 5000.0, ROW_HEIGHT, 4);
        assert!(
            layout.panel[0] >= SCREEN_MARGIN - 0.001,
            "the panel ran off the left edge: x0={}",
            layout.panel[0]
        );
        assert_close(
            display[0] - layout.panel[2],
            SCREEN_MARGIN,
            "right screen margin on a narrow display",
        );
    }

    #[test]
    fn a_pointer_with_no_position_never_hovers_the_button() {
        let layout = panel_layout(DISPLAY, 160.0, ROW_HEIGHT, 1);
        // What imgui reports when the window has no mouse.
        assert!(!rect_contains(layout.toggle, [-f32::MAX, -f32::MAX]));
        assert!(!rect_contains(layout.toggle, [f32::NAN, f32::NAN]));
    }

    #[test]
    fn a_pointer_outside_the_button_does_not_hover_it() {
        let layout = panel_layout(DISPLAY, 400.0, ROW_HEIGHT, 4);
        assert!(!rect_contains(
            layout.toggle,
            [layout.panel[0] - 1.0, layout.first_row_y]
        ));
        assert!(!rect_contains(
            layout.toggle,
            [layout.panel[2] + 1.0, layout.first_row_y]
        ));
        assert!(!rect_contains(
            layout.toggle,
            [layout.text_x, layout.panel[1] - 1.0]
        ));
    }
}
