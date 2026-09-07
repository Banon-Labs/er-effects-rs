//! Where the watermark's lines sit, and what colour each is drawn in.
//!
//! Deliberately free of hudhook, imgui and Windows types so every decision that has a right
//! answer -- the right-edge anchor, the row advance, the opacity per standing -- is asserted by
//! `cargo test` on the host instead of by launching a game and looking at a corner.

#![cfg_attr(not(windows), allow(dead_code))]

/// Distance from the panel to the top and right screen edges.
///
/// One number for both, borrowed from `er-net-effects`' overlay: the block should read as pinned
/// to the corner, and two different margins read as a mistake even when neither is wrong.
pub const SCREEN_MARGIN: f32 = 48.0;

/// Blank space between rows, on top of the font's own height.
pub const ROW_GAP: f32 = 4.0;

/// Font size for the watermark, in pixels.
///
/// "Relatively big" was an explicit requirement, so it is a named constant with a test rather
/// than a number chosen by eye: imgui's default face is 13px, and this is a little over double.
pub const FONT_SIZE_PX: f32 = 27.0;

/// "Relatively big" was a requirement, not a preference, so it is checked at compile time rather
/// than by a test: both sides are constants, and a runtime assertion over two constants is a test
/// that can only ever pass or fail to build. Shrinking the font below twice imgui's 13px default
/// stops the build.
const _: () = assert!(FONT_SIZE_PX >= 13.0 * 2.0);

/// Colour of a line whose build is at `main`, on a local branch, or unknown.
///
/// Near-white so that what little of it survives 1% alpha is legible against the game's mostly
/// dark, warm palette.
pub const QUIET_RGB: [f32; 3] = [0.92, 0.92, 0.95];

/// Colour of a line whose build is an older published release than `main`'s tip.
pub const BEHIND_RGB: [f32; 3] = [1.0, 0.27, 0.27];

/// One rendered row: the text, and the RGBA it is drawn in.
#[derive(Clone, Debug, PartialEq)]
pub struct WatermarkRow {
    /// `NAME  SHA`, already formatted.
    pub text: String,
    /// Straight to `DrawList::add_text`, alpha included.
    pub rgba: [f32; 4],
}

/// Colour + opacity for a standing.
///
/// The opacity comes from [`er_game_base::build_id::Standing::opacity_percent`] rather than being
/// restated here, so the number that decides how loud the watermark is lives in exactly one
/// place: the same enum the comparison produces.
pub fn row_rgba(standing: er_game_base::build_id::Standing) -> [f32; 4] {
    let rgb = if standing.is_behind() {
        BEHIND_RGB
    } else {
        QUIET_RGB
    };
    let alpha = f32::from(standing.opacity_percent()) / 100.0;
    [rgb[0], rgb[1], rgb[2], alpha]
}

/// Build the rows for a roster.
///
/// Sorted so a behind line is never buried in the middle of a quiet list: at 1% the rest of the
/// block is nearly invisible anyway, and the one line anybody needs to see belongs where the eye
/// lands first.
pub fn rows(
    identities: &[er_game_base::build_id::ModIdentity],
    published: &[String],
) -> Vec<WatermarkRow> {
    let mut rows: Vec<(bool, WatermarkRow)> = identities
        .iter()
        .map(|identity| {
            let standing = er_game_base::build_id::standing_against_main(&identity.sha, published);
            (
                standing.is_behind(),
                WatermarkRow {
                    text: format!("{}  {}", identity.module, identity.sha),
                    rgba: row_rgba(standing),
                },
            )
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.0));
    rows.into_iter().map(|(_, row)| row).collect()
}

/// Top-left position of row `index`, given the screen size and the width of that row's text.
///
/// Right-aligned rather than left-aligned at a fixed x: the rows differ in length, and a ragged
/// right edge against the screen edge it is anchored to reads as broken.
pub fn row_position(screen_width: f32, index: usize, row_height: f32, text_width: f32) -> [f32; 2] {
    let x = (screen_width - SCREEN_MARGIN - text_width).max(0.0);
    let y = SCREEN_MARGIN + index as f32 * (row_height + ROW_GAP);
    [x, y]
}

#[cfg(test)]
mod tests {
    use super::*;
    use er_game_base::build_id::{ModIdentity, Standing};

    fn identity(module: &str, sha: &str) -> ModIdentity {
        ModIdentity {
            module: module.to_string(),
            sha: sha.to_string(),
            pe_timestamp: 0,
        }
    }

    fn published() -> Vec<String> {
        [
            "ba46f81cc9306253958013affa1c916f980e1162",
            "c1adc6c89a49107160897c9e3dd7e1eff1415419",
        ]
        .iter()
        .map(|sha| sha.to_string())
        .collect()
    }

    /// The two numbers the user asked for, asserted as pixels-on-screen rather than as an enum.
    #[test]
    fn quiet_rows_are_one_percent_and_behind_rows_are_twenty_five() {
        assert_eq!(row_rgba(Standing::AtMain)[3], 0.01);
        assert_eq!(row_rgba(Standing::Local)[3], 0.01);
        assert_eq!(row_rgba(Standing::Unknown)[3], 0.01);
        assert_eq!(row_rgba(Standing::BehindMain)[3], 0.25);
    }

    /// Red is reserved. A dirty local build is the developer's ordinary state and must not be
    /// drawn in the colour that means "stop and update".
    #[test]
    fn only_a_behind_row_is_red() {
        assert_eq!(row_rgba(Standing::BehindMain)[..3], BEHIND_RGB);
        for quiet in [Standing::AtMain, Standing::Local, Standing::Unknown] {
            assert_eq!(row_rgba(quiet)[..3], QUIET_RGB, "{quiet:?} was drawn red");
        }
    }

    /// The one line that matters goes to the top, whatever order the loader produced.
    #[test]
    fn a_behind_row_is_hoisted_above_the_quiet_ones() {
        let identities = [
            identity("er_quickload.dll", "ba46f81cc930"),
            identity("er_invasion_warp.dll", "c1adc6c89a49"),
            identity("er_quit_menu.dll", "ba46f81cc930+dirty"),
        ];
        let rows = rows(&identities, &published());
        assert!(
            rows[0].text.starts_with("er_invasion_warp.dll"),
            "the behind row was not hoisted: {:?}",
            rows[0].text
        );
        assert_eq!(rows[0].rgba[3], 0.25);
        assert!(rows[1..].iter().all(|row| row.rgba[3] == 0.01));
    }

    /// Rows are right-aligned to a single edge, and advance downward by a fixed step.
    #[test]
    fn rows_right_align_and_stack_downward() {
        let first = row_position(3840.0, 0, 27.0, 400.0);
        let second = row_position(3840.0, 1, 27.0, 300.0);
        assert_eq!(first, [3840.0 - 48.0 - 400.0, 48.0]);
        // Shorter text starts further right -- that is what right alignment means.
        assert!(second[0] > first[0]);
        assert_eq!(second[1] - first[1], 27.0 + ROW_GAP);
    }

    /// A row wider than the screen clamps to the left edge instead of going off it.
    #[test]
    fn an_overlong_row_stays_on_screen() {
        assert_eq!(row_position(800.0, 0, 27.0, 5_000.0)[0], 0.0);
    }
}
