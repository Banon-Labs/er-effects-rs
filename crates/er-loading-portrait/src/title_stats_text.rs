//! Pure title/ProfileSelect stats-panel text and neutral-background decisions.
//!
//! The root DLL still owns the unsafe Scaleform hooks, row-model reads, and texture
//! registration. This module owns the host-testable formatting/layout constants those
//! hooks apply.

use crate::layout::STATS_ATTR_COUNT;

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
    pub er_stats: bool,
    /// `ErCharStats` -- our attribute line, "VIG 50 MND 10 ...". Character rows only.
    pub char_stats: bool,
    /// `DriveCell_0..2` -- the drive-letter cells, which live and die together. Available on every
    /// picker-owned row (the picker blanks the cells per row), denied on character rows.
    pub drive_cells: bool,
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
        drive_cells: false,
    };

    /// Browse/file rows are not profile slots; level is hidden, the top-right
    /// location field is shown only when it carries a staged timestamp, and the
    /// bottom play-time row is always hidden so file rows collapse to one line.
    ///
    /// `char_stats` is false: the attribute line describes a character, and a file row has none.
    ///
    /// `drive_cells` is TRUE for every picker-owned row, including plain file and directory rows.
    /// That is deliberate: the picker already writes all three cells on every row it owns, blanking
    /// them where there is no drive strip, so their CONTENT is decided per row by that pass and the
    /// visibility statement only has to keep them available. Denying them here would hide the drive
    /// strip on the drive row itself. Character rows deny them, which is what stops the strip
    /// surviving onto a recycled character-slot clip.
    pub const fn browse_row(has_timestamp: bool) -> Self {
        Self {
            level: false,
            location: has_timestamp,
            play_time: false,
            er_stats: true,
            char_stats: false,
            drive_cells: true,
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
            RowSlotFieldVisibility::browse_row(true),
            RowSlotFieldVisibility {
                level: false,
                location: true,
                play_time: false,
                er_stats: true,
                char_stats: false,
                drive_cells: true,
            }
        );
        assert_eq!(
            RowSlotFieldVisibility::browse_row(false),
            RowSlotFieldVisibility {
                level: false,
                location: false,
                play_time: false,
                er_stats: true,
                char_stats: false,
                drive_cells: true,
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
                drive_cells: false,
            }
        );
    }

    /// Every field must be claimed by exactly one row kind and denied by the others, or a recycled
    /// clip keeps the previous kind's text. This is the invariant the leak violated: `char_stats`
    /// was shown on character rows and never denied on browse rows, `drive_cells` shown on drive
    /// rows and never denied on character rows.
    #[test]
    fn each_row_kind_states_an_answer_for_every_field_and_they_disagree() {
        let native = RowSlotFieldVisibility::NATIVE;
        let browse = RowSlotFieldVisibility::browse_row(true);

        // The character-only field is denied by the picker kind.
        assert!(native.char_stats && !browse.char_stats);
        // The picker-only fields are denied by the character kind. drive_cells stays available on
        // every picker row because the picker's own per-row pass decides their content.
        assert!(!native.er_stats && browse.er_stats);
        assert!(!native.drive_cells && browse.drive_cells);
        // The native per-slot fields belong to character rows.
        assert!(native.level && !browse.level);
        assert!(native.play_time && !browse.play_time);

        // The two kinds differ, so `!= NATIVE` remains a meaningful "this row is ours" test.
        assert_ne!(native, browse);
        assert_ne!(browse, RowSlotFieldVisibility::browse_row(false));
    }
}
