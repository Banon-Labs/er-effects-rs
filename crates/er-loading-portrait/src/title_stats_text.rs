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
    let end = end.min(TITLE_STATS_LABELS.len());
    let mut s = String::from("<p align=\"left\">");
    for i in start..end {
        let v = attributes[i];
        if i > start {
            // A wider gap between pairs groups the attributes.
            s.push_str("  ");
        }
        s.push_str("<font size=\"");
        s.push_str(TITLE_STATS_HTML_SIZE);
        s.push_str("\" color=\"");
        s.push_str(TITLE_STATS_LABEL_COLOR);
        s.push_str("\">");
        s.push_str(TITLE_STATS_LABELS[i]);
        s.push_str("</font> <font size=\"");
        s.push_str(TITLE_STATS_HTML_SIZE);
        s.push_str("\" color=\"");
        s.push_str(TITLE_STATS_VALUE_COLORS[i]);
        s.push_str("\"><b>");
        s.push_str(&v.to_string());
        s.push_str("</b></font>");
    }
    s.push_str("</p>");
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Which of a ProfileSelect row's native per-slot info fields should be on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowSlotFieldVisibility {
    /// The `Level` FMG caption and the level value, which live and die together.
    pub level: bool,
    /// The `PlayTime` field.
    pub play_time: bool,
}

impl RowSlotFieldVisibility {
    /// What a row the picker does not own gets: exactly what the game drew.
    pub const NATIVE: Self = Self {
        level: true,
        play_time: true,
    };

    /// Browse/file rows are not profile slots; level is hidden, and play-time is
    /// shown only when the row has a replacement timestamp to stage.
    pub const fn browse_row(has_play_time: bool) -> Self {
        Self {
            level: false,
            play_time: has_play_time,
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
                play_time: true,
            }
        );
        assert_eq!(
            RowSlotFieldVisibility::browse_row(false),
            RowSlotFieldVisibility {
                level: false,
                play_time: false,
            }
        );
        assert_eq!(
            RowSlotFieldVisibility::NATIVE,
            RowSlotFieldVisibility {
                level: true,
                play_time: true,
            }
        );
    }
}
