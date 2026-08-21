//! Pure title/ProfileSelect stats-panel text and neutral-background decisions.
//!
//! The root DLL still owns the unsafe Scaleform hooks, row-model reads, and texture
//! registration. This module owns the host-testable formatting/layout constants those
//! hooks apply.

use crate::layout::STATS_ATTR_COUNT;
use er_gfx::title_05_010::DRIVE_CELL_CAPACITY;

/// Number of ProfileSelect save slots addressed by the stats-panel neutral backgrounds.
pub const STATS_PANEL_SLOT_COUNT: usize = 10;

/// Unique in-RAM SYSTEX keys, one per slot 00..09. Each is the TPF003 entry name
/// (== the GLOBAL_TexRepository GPU key the Scaleform bridge derives) and the
/// rewritten bind target. Kept short enough for the native target DLString.
pub const STATS_PANEL_SYSTEX_KEYS: [&str; STATS_PANEL_SLOT_COUNT] = [
    "SYSTEX_ErTpf_Prf00",
    "SYSTEX_ErTpf_Prf01",
    "SYSTEX_ErTpf_Prf02",
    "SYSTEX_ErTpf_Prf03",
    "SYSTEX_ErTpf_Prf04",
    "SYSTEX_ErTpf_Prf05",
    "SYSTEX_ErTpf_Prf06",
    "SYSTEX_ErTpf_Prf07",
    "SYSTEX_ErTpf_Prf08",
    "SYSTEX_ErTpf_Prf09",
];

/// Neutral-background texture side length (square RGBA8).
pub const STATS_PANEL_TEX_DIM: u32 = 256;
/// Neutral dark panel color (opaque).
pub const STATS_PANEL_BG_RGBA: [u8; 4] = [30, 28, 26, 255];

/// The SYSTEX key for `slot`, if `slot` is one of the native ProfileSelect rows.
pub fn stats_panel_systex_key(slot: usize) -> Option<&'static str> {
    STATS_PANEL_SYSTEX_KEYS.get(slot).copied()
}

/// The redirect key for `slot` only after its neutral texture is registered.
pub fn stats_panel_registered_systex_key(
    slot: usize,
    registered_mask: usize,
) -> Option<&'static str> {
    let bit = 1usize.checked_shl(slot as u32)?;
    if registered_mask & bit == 0 {
        return None;
    }
    stats_panel_systex_key(slot)
}

const TITLE_STATS_LABELS: [&str; STATS_ATTR_COUNT] =
    ["VIG", "MND", "END", "STR", "DEX", "INT", "FAI", "ARC"];
// One distinct, dark-row-legible color per attribute value.
const TITLE_STATS_VALUE_COLORS: [&str; STATS_ATTR_COUNT] = [
    "#e0736b", // VIG - red
    "#6fb4e0", // MND - blue
    "#7fc27a", // END - green
    "#e0973f", // STR - orange
    "#d7d06a", // DEX - yellow
    "#79cfe0", // INT - cyan
    "#e0c766", // FAI - gold
    "#c489c0", // ARC - violet
];

// Labels dimmer than the native #cccccc so they read as secondary.
const TITLE_STATS_LABEL_COLOR: &str = "#8f887a";
const TITLE_STATS_HTML_SIZE: &str = "19";

/// Build the ProfileSelect stats line for `attributes[start..end]` as a
/// NUL-terminated UTF-16 Scaleform-HTML string for native SetText.
pub fn build_title_stats_html_utf16(
    attributes: &[i32; STATS_ATTR_COUNT],
    start: usize,
    end: usize,
) -> Vec<u16> {
    build_title_stats_html_utf16_with(
        attributes,
        start,
        end,
        &TITLE_STATS_LABELS,
        Some(TITLE_STATS_HTML_SIZE),
        "  ",
    )
}

/// Build all eight ProfileSelect stats as one shorter NUL-terminated UTF-16
/// Scaleform-HTML line. This is for the compact one-row `05_010` layout: the
/// native row already has only one spare horizontal band. It intentionally omits
/// HTML `size=` overrides so Scaleform uses the row field's own embedded MenuFont
/// definition; the field itself is narrower/smaller, but the face matches the
/// surrounding native row text.
pub fn build_title_stats_compact_html_utf16(attributes: &[i32; STATS_ATTR_COUNT]) -> Vec<u16> {
    build_title_stats_html_utf16_with(
        attributes,
        0,
        STATS_ATTR_COUNT,
        &TITLE_STATS_LABELS,
        None,
        " ",
    )
}

fn build_title_stats_html_utf16_with(
    attributes: &[i32; STATS_ATTR_COUNT],
    start: usize,
    end: usize,
    labels: &[&str; STATS_ATTR_COUNT],
    size: Option<&str>,
    separator: &str,
) -> Vec<u16> {
    let end = end.min(labels.len());
    let mut s = String::from("<p align=\"left\">");
    for i in start..end {
        let v = attributes[i];
        if i > start {
            s.push_str(separator);
        }
        s.push_str("<font");
        if let Some(size) = size {
            s.push_str(" size=\"");
            s.push_str(size);
            s.push('"');
        }
        s.push_str(" color=\"");
        s.push_str(TITLE_STATS_LABEL_COLOR);
        s.push_str("\">");
        s.push_str(labels[i]);
        s.push_str("</font> <font");
        if let Some(size) = size {
            s.push_str(" size=\"");
            s.push_str(size);
            s.push('"');
        }
        s.push_str(" color=\"");
        s.push_str(TITLE_STATS_VALUE_COLORS[i]);
        s.push_str("\"><b>");
        s.push_str(&v.to_string());
        s.push_str("</b></font>");
    }
    s.push_str("</p>");
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Which of a ProfileSelect row's fields should be on screen.
///
/// This must name EVERY field any row kind writes, not just the native per-slot ones. The row clips
/// are recycled between the character-slot list, the file-browse list and the drive list, so a field
/// that one kind writes and another never mentions keeps the first kind's text. That is not
/// hypothetical: the attribute line (`ErCharStats`) written on character rows reappeared over the
/// browse rows, and the drive-letter cells written on drive rows reappeared over the character rows,
/// both surviving a character load, because this struct covered only `level`/`location`/`play_time`
/// while five other fields were left unstated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowSlotFieldVisibility {
    /// The `Level` FMG caption and the level value, which live and die together.
    pub level: bool,
    /// The top-right `Location` field.
    pub location: bool,
    /// The bottom-right `PlayTime` field.
    pub play_time: bool,
    /// `ErStats` -- our merged line: the browse rows' "FOLDER / ..." / "N CHAR / ..." text.
    /// Hidden on a drive row, which has no stats copy of its own, so a recycled parent-row field
    /// cannot keep showing "PARENT FOLDER / ..." beside the drive cells.
    pub er_stats: bool,
    /// `ErCharStats` -- our attribute line, "VIG 50 MND 10 ...". Character rows only.
    pub char_stats: bool,
    /// `DriveCell_0..25` and their matching button frames, one visibility bit per possible Windows
    /// drive letter. Visible only for populated cells on the picker's drive-cycle row, denied
    /// everywhere else.
    pub drive_cells: [bool; DRIVE_CELL_CAPACITY],
    /// Full current-directory text/button pair, visible only beside the populated drive strip.
    pub current_path: bool,
    /// Full-width native `Backing`. The drive row hides it because its independent drive buttons
    /// are the interaction chrome. Native `Cursor` visibility is deliberately not represented here:
    /// the game's list-selection code owns it, while runtime only changes its drive-row geometry.
    pub backing: bool,
}

impl RowSlotFieldVisibility {
    /// What a row the picker does not own gets: the game's own per-slot fields, plus our attribute
    /// line, and explicitly NOT the browse/drive text.
    ///
    /// `er_stats` and `drive_cells` are false here on purpose. They are the fields the file and
    /// drive lists write, and stating them false is the only thing that stops those lists' text
    /// surviving onto a character row.
    pub const NATIVE: Self = Self {
        level: true,
        location: true,
        play_time: true,
        er_stats: false,
        char_stats: true,
        drive_cells: [false; DRIVE_CELL_CAPACITY],
        current_path: false,
        backing: true,
    };

    /// A character row whose header was MERGED: the name, Rune Level and weapon level are one
    /// string in `PlayerName` (see `profile_row_label`), so the separate `Level` FMG caption and
    /// level value must go.
    ///
    /// This is a DISTINCT constant rather than a change to [`Self::NATIVE`] on purpose. The row
    /// pass only applies visibility when the wanted state differs from `NATIVE` (or when a row was
    /// previously hidden), so flipping `NATIVE.level` to false would make the wanted state equal
    /// `NATIVE` again and skip the very pass that has to hide the fields -- the caption would
    /// render under the merged label until some other row happened to trigger the pass. Keeping
    /// them separate means the guard fires on the first merged row, with no change to the guard and
    /// no visibility work on rows we do not merge.
    ///
    /// Every other field matches `NATIVE`: this is a character row, so it still denies the browse
    /// and drive text that would otherwise survive onto a recycled clip.
    /// `play_time` is false here for a LAYOUT reason, not a data one (user 2026-08-07): the row's
    /// bottom-right play-time box overlaps the top-right `Location` box, so as long as it is drawn,
    /// `Location` cannot be widened past it and long place names clip. Dropping the play-time frees
    /// that whole band for the location, which is the field the user actually reads. A row that
    /// fails to merge falls back to `NATIVE` and keeps its play-time, so the vanilla view is intact.
    pub const NATIVE_MERGED: Self = Self {
        level: false,
        play_time: false,
        ..Self::NATIVE
    };

    /// [`Self::NATIVE_MERGED`], but able to say the row has no location to show.
    ///
    /// The `Location` string is formatted from the `PlaceName` id in that slot's `ProfileSummary`
    /// record, and in a save assembled outside the game the record can belong to a different
    /// character than the body in that slot (see `er_save_loader::profile_summary`). The id exists
    /// nowhere else in the save and the game cannot recompute it from a map, so a row in that state
    /// has no location available -- and printing the record's anyway is how six of ten rows came to
    /// claim "Midra's Manse" for characters standing in the Academy (user-reported 2026-08-07).
    /// Blank is a visible, diagnosable absence; another character's place name is not.
    pub const fn native_merged(location: bool) -> Self {
        Self {
            location,
            ..Self::NATIVE_MERGED
        }
    }

    /// Picker-owned rows are not profile slots; level is hidden, the top-right location field is
    /// shown only when it carries a staged timestamp, and the bottom play-time row is always hidden
    /// so file rows collapse to one line.
    ///
    /// `char_stats` is false: the attribute line describes a character, and a picker row has none.
    /// `er_stats` and `drive_cells` are row-kind decisions supplied by the save-picker model. They
    /// must not be blanket picker-wide `true`: recycled row clips retain fields that the next row
    /// never writes, which is how the drive strip appeared on every row and the parent-folder copy
    /// survived onto the drive row.
    pub fn browse_row(has_timestamp: bool, er_stats: bool, drive_cell_count: usize) -> Self {
        Self {
            level: false,
            location: has_timestamp,
            play_time: false,
            er_stats,
            char_stats: false,
            drive_cells: std::array::from_fn(|index| index < drive_cell_count),
            current_path: drive_cell_count > 0,
            backing: drive_cell_count == 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16_to_string(v: &[u16]) -> String {
        assert_eq!(v.last(), Some(&0), "native string must be NUL terminated");
        String::from_utf16(&v[..v.len() - 1]).expect("valid utf16")
    }

    #[test]
    fn title_stats_html_keeps_expected_labels_colors_and_nul() {
        let attrs = [15, 10, 11, 14, 13, 9, 9, 7];
        let top = utf16_to_string(&build_title_stats_html_utf16(&attrs, 0, 4));
        assert!(top.starts_with("<p align=\"left\">"));
        assert!(top.ends_with("</p>"));
        assert!(top.contains("color=\"#8f887a\">VIG</font>"));
        assert!(top.contains("color=\"#e0736b\"><b>15</b></font>"));
        assert!(top.contains("color=\"#e0973f\"><b>14</b></font>"));
        assert!(!top.contains("DEX"), "end bound limits the line");
    }

    #[test]
    fn title_stats_compact_html_merges_all_attributes_into_one_short_line() {
        let attrs = [15, 10, 11, 14, 13, 9, 9, 7];
        let compact = utf16_to_string(&build_title_stats_compact_html_utf16(&attrs));
        assert!(compact.starts_with("<p align=\"left\">"));
        assert!(compact.ends_with("</p>"));
        assert!(compact.contains("color=\"#8f887a\">VIG</font> <font"));
        assert!(compact.contains("color=\"#e0736b\"><b>15</b></font>"));
        assert!(compact.contains("color=\"#c489c0\"><b>7</b></font>"));
        assert!(
            !compact.contains("size=\""),
            "compact line inherits the field font/height instead of forcing a Scaleform HTML font size"
        );
        assert!(
            compact.contains(">ARC</font>"),
            "last attribute is included"
        );
    }

    #[test]
    fn title_stats_html_second_line_keeps_global_color_indices() {
        let attrs = [15, 10, 11, 14, 13, 9, 9, 7];
        let bottom = utf16_to_string(&build_title_stats_html_utf16(&attrs, 4, STATS_ATTR_COUNT));
        assert!(bottom.contains("DEX"));
        assert!(bottom.contains("color=\"#d7d06a\"><b>13</b></font>"));
        assert!(bottom.contains("color=\"#c489c0\"><b>7</b></font>"));
        assert!(!bottom.contains("VIG"), "start bound limits the line");
    }

    #[test]
    fn registered_key_decision_requires_slot_and_mask() {
        assert_eq!(stats_panel_systex_key(0), Some("SYSTEX_ErTpf_Prf00"));
        assert_eq!(stats_panel_systex_key(9), Some("SYSTEX_ErTpf_Prf09"));
        assert_eq!(stats_panel_systex_key(10), None);
        assert_eq!(stats_panel_registered_systex_key(2, 0), None);
        assert_eq!(
            stats_panel_registered_systex_key(2, 1 << 2),
            Some("SYSTEX_ErTpf_Prf02")
        );
        assert_eq!(stats_panel_registered_systex_key(10, usize::MAX), None);
    }

    #[test]
    fn row_visibility_decisions_match_picker_contract() {
        assert_eq!(
            RowSlotFieldVisibility::browse_row(true, true, 0),
            RowSlotFieldVisibility {
                level: false,
                location: true,
                play_time: false,
                er_stats: true,
                char_stats: false,
                drive_cells: [false; DRIVE_CELL_CAPACITY],
                current_path: false,
                backing: true,
            }
        );
        assert_eq!(
            RowSlotFieldVisibility::browse_row(false, false, 3),
            RowSlotFieldVisibility {
                level: false,
                location: false,
                play_time: false,
                er_stats: false,
                char_stats: false,
                drive_cells: std::array::from_fn(|index| index < 3),
                current_path: true,
                backing: false,
            }
        );
        assert_eq!(
            RowSlotFieldVisibility::NATIVE,
            RowSlotFieldVisibility {
                level: true,
                location: true,
                play_time: true,
                er_stats: false,
                char_stats: true,
                drive_cells: [false; DRIVE_CELL_CAPACITY],
                current_path: false,
                backing: true,
            }
        );
    }

    #[test]
    fn drive_visibility_has_one_bit_per_real_button() {
        let cells = RowSlotFieldVisibility::browse_row(false, false, 2).drive_cells;
        assert!(cells[..2].iter().all(|visible| *visible));
        assert!(cells[2..].iter().all(|visible| !*visible));
    }

    /// Dropping a row's unsourceable location must drop ONLY that field. A merged row still owes the
    /// same statement about every other field, or the browse/drive text it denies starts surviving
    /// onto it -- the recycled-clip leak this struct exists to prevent.
    #[test]
    fn a_merged_row_without_a_location_still_states_every_other_field() {
        let with = RowSlotFieldVisibility::native_merged(true);
        let without = RowSlotFieldVisibility::native_merged(false);
        assert_eq!(with, RowSlotFieldVisibility::NATIVE_MERGED);
        assert!(!without.location);
        assert_eq!(
            RowSlotFieldVisibility {
                location: true,
                ..without
            },
            with
        );
        // And it is still distinguishable from NATIVE, so the visibility pass fires for it.
        assert_ne!(without, RowSlotFieldVisibility::NATIVE);
    }

    /// Every field must be claimed by exactly one row kind and denied by the others, or a recycled
    /// clip keeps the previous kind's text. The parent/file metadata field and the drive cells are
    /// separate row kinds as well as being picker-only fields: making both visible picker-wide is
    /// what put the drive letters on every row and stale parent copy on the drive row.
    #[test]
    fn each_row_kind_states_an_answer_for_every_field_and_they_disagree() {
        let native = RowSlotFieldVisibility::NATIVE;
        let browse = RowSlotFieldVisibility::browse_row(true, true, 0);
        let drive = RowSlotFieldVisibility::browse_row(false, false, 3);

        // The character-only field is denied by both picker kinds.
        assert!(native.char_stats && !browse.char_stats && !drive.char_stats);
        // The metadata and drive fields are mutually exclusive and both denied by character rows.
        assert!(!native.er_stats && browse.er_stats && !drive.er_stats);
        assert_eq!(native.drive_cells, [false; DRIVE_CELL_CAPACITY]);
        assert_eq!(browse.drive_cells, [false; DRIVE_CELL_CAPACITY]);
        assert_eq!(drive.drive_cells, std::array::from_fn(|index| index < 3));
        assert!(!native.current_path && !browse.current_path && drive.current_path);
        assert!(native.backing && browse.backing && !drive.backing);
        // The native per-slot fields belong to character rows.
        assert!(native.level && !browse.level && !drive.level);
        assert!(native.play_time && !browse.play_time && !drive.play_time);

        // Every picker kind differs from NATIVE, so the visibility pass fires on its first row.
        assert_ne!(native, browse);
        assert_ne!(native, drive);
        assert_ne!(browse, drive);
        assert_ne!(browse, RowSlotFieldVisibility::browse_row(false, true, 0));
    }
}
