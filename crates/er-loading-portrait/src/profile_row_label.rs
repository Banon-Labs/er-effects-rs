//! The ProfileSelect row's merged header label: ONE text element replacing the three that used to
//! sit before the attribute block (`PlayerName`, the `Level` FMG caption, and the `Level` value).
//!
//! WHY MERGE (user request 2026-08-06/07). Three independently-placed fields have to be kept in
//! horizontal agreement by hand, in a schema, against a proportional font whose glyph advances we
//! do not model. Every name length change re-opens that alignment question. One field with markers
//! makes the spacing a property of the STRING -- the font lays it out, and there is nothing left to
//! align. It also frees the caption's width: `Level` is 5 glyphs, `RL` is 2, and the user asked for
//! the reclaimed space to carry a new `WL` (max weapon upgrade level) readout.
//!
//! WHERE IT RENDERS. The merged text goes into the row model's `PlayerName` `CS::MenuString`
//! (`rowModel + 0x50`) before the native row populate reads it, so the game's OWN writer
//! (`FUN_1408757e0` -> `FUN_140749ed0`) draws it. That is why this module emits PLAIN TEXT and not
//! Scaleform HTML: `ErCharStats` is pushed through our own SetText and may be marked-up, but
//! `PlayerName` is written by the native path, which does not enable HTML on that field. A `<` in a
//! character name therefore stays a literal `<`, exactly as it does today on the unmerged field.
//!
//! The `Level` caption and value are hidden per row rather than blanked, through the same
//! visibility choke point every other row field uses -- see `RowSlotFieldVisibility::NATIVE_MERGED`
//! in `title_stats_text`. They cannot be emptied through the row model (a zeroed level word still
//! formats to "0"), and the row clips are recycled, so hiding is the only content-free lever.

/// Marker for the character's name.
pub const MARKER_CHARACTER_NAME: &str = "%character_name";
/// Marker for the character's Rune Level.
pub const MARKER_RUNE_LEVEL: &str = "%rune_level";
/// Marker for the character's maximum weapon upgrade level.
pub const MARKER_WEAPON_LEVEL: &str = "%weapon_level";

/// The shipped row-header template.
///
/// `[ ... ]` is an OPTIONAL GROUP: it is emitted only when every marker inside it resolves. That is
/// what makes one template serve every row state without a second code path -- a slot whose level
/// could not be decoded renders the bare name instead of a dangling `RL `, and `WL` simply does not
/// appear until a weapon level is available. The leading space lives INSIDE each group so a dropped
/// group leaves no trailing whitespace behind.
/// The comma lives INSIDE the level group, so a row with no level renders `Maddened Bean` and not
/// `Maddened Bean,` — punctuation that only makes sense when something follows it must be dropped
/// with the thing it introduces.
pub const SHIPPED_ROW_HEADER_TEMPLATE: &str =
    "%character_name[, RL %rune_level][ WL %weapon_level]";

/// Rune Level bounds, matching the save decoder's own plausibility window
/// (`er_save_loader::stats`, which rejects anything outside `1..=713` before trusting a stat block).
/// A value outside the window means we decoded the wrong offset, and rendering it would put a
/// confident wrong number on screen -- so the marker fails to resolve and its group drops instead.
const RUNE_LEVEL_MIN: i32 = 1;
const RUNE_LEVEL_MAX: i32 = 713;

/// Weapon upgrade bounds. Standard weapons reinforce `+0..=+25`, somber weapons `+0..=+10`, so 25 is
/// the maximum any single armament can reach. `+0` is deliberately still renderable: "WL 0" is a
/// true statement about a fresh character, unlike a wrong number.
const WEAPON_LEVEL_MIN: i32 = 0;
const WEAPON_LEVEL_MAX: i32 = 25;

/// The values a row header can interpolate. Every field is optional because a row is allowed to know
/// less than the full picture: an unreadable save, an empty slot, or a not-yet-implemented source
/// must degrade to a shorter label, never to a fabricated one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowHeaderValues {
    /// The character name.
    pub character_name: Option<String>,
    /// Rune Level, dropped when outside the decoder's plausibility window.
    pub rune_level: Option<i32>,
    /// Highest weapon upgrade level on the character, dropped when outside `+0..=+25`.
    pub weapon_level: Option<i32>,
}

impl RowHeaderValues {
    /// A header carrying just the name -- what a row falls back to when nothing else decoded.
    pub fn from_name(name: impl Into<String>) -> Self {
        Self {
            character_name: Some(name.into()),
            rune_level: None,
            weapon_level: None,
        }
    }

    /// Attach a Rune Level, ignoring implausible values (see [`RUNE_LEVEL_MIN`]).
    #[must_use]
    pub fn with_rune_level(mut self, level: i32) -> Self {
        self.rune_level = (RUNE_LEVEL_MIN..=RUNE_LEVEL_MAX)
            .contains(&level)
            .then_some(level);
        self
    }

    /// Attach a maximum weapon upgrade level, ignoring implausible values.
    #[must_use]
    pub fn with_weapon_level(mut self, level: i32) -> Self {
        self.weapon_level = (WEAPON_LEVEL_MIN..=WEAPON_LEVEL_MAX)
            .contains(&level)
            .then_some(level);
        self
    }

    fn resolve(&self, marker: &str) -> Option<String> {
        match marker {
            MARKER_CHARACTER_NAME => self.character_name.clone(),
            MARKER_RUNE_LEVEL => self.rune_level.map(|v| v.to_string()),
            MARKER_WEAPON_LEVEL => self.weapon_level.map(|v| v.to_string()),
            _ => None,
        }
    }
}

/// Every marker this module knows, longest first so a prefix never shadows a longer name.
const MARKERS: [&str; 3] = [
    MARKER_CHARACTER_NAME,
    MARKER_RUNE_LEVEL,
    MARKER_WEAPON_LEVEL,
];

/// Expand `template` against `values`.
///
/// Outside a group, an unresolved marker expands to nothing (it is the caller's statement that the
/// value is optional in that position). Inside `[ ... ]`, ONE unresolved marker drops the WHOLE
/// group, which is what keeps ` RL ` off screen when the level is unknown. A group containing no
/// markers at all is emitted verbatim -- it is literal text the author chose to bracket.
///
/// Unbalanced brackets are not an error: a `[` with no `]` is treated as a group running to the end
/// of the template. This is a display string, and a template typo must degrade to slightly wrong
/// text, never to a panic on the game thread.
pub fn expand_row_header(template: &str, values: &RowHeaderValues) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let bytes = template.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => {
                let start = i + 1;
                let end = template[start..]
                    .find(']')
                    .map_or(template.len(), |off| start + off);
                let (body, complete) = (
                    &template[start..end],
                    expand_group(&template[start..end], values),
                );
                if let Some(text) = complete {
                    out.push_str(&text);
                }
                debug_assert!(body.len() + start <= template.len());
                i = if end < template.len() { end + 1 } else { end };
            }
            b']' => {
                // A stray closer with no opener is literal text.
                out.push(']');
                i += 1;
            }
            _ => {
                if let Some((marker, text)) = take_marker(template, i, values) {
                    out.push_str(&text);
                    i += marker.len();
                } else {
                    let ch_len = char_len_at(template, i);
                    out.push_str(&template[i..i + ch_len]);
                    i += ch_len;
                }
            }
        }
    }
    out
}

/// Expand one optional group, or `None` when a marker inside it did not resolve.
fn expand_group(body: &str, values: &RowHeaderValues) -> Option<String> {
    let mut out = String::with_capacity(body.len() + 8);
    let mut i = 0usize;
    while i < body.len() {
        if let Some(marker) = MARKERS.iter().find(|m| body[i..].starts_with(**m)) {
            // The group is all-or-nothing: bail on the FIRST unresolved marker.
            out.push_str(&values.resolve(marker)?);
            i += marker.len();
        } else {
            let ch_len = char_len_at(body, i);
            out.push_str(&body[i..i + ch_len]);
            i += ch_len;
        }
    }
    Some(out)
}

/// If a marker starts at `i`, return it and its expansion (empty when unresolved).
fn take_marker<'a>(text: &'a str, i: usize, values: &RowHeaderValues) -> Option<(&'a str, String)> {
    let marker = MARKERS.iter().find(|m| text[i..].starts_with(**m))?;
    Some((marker, values.resolve(marker).unwrap_or_default()))
}

/// Byte length of the UTF-8 character starting at `i`. Keeps slicing on char boundaries so a
/// non-ASCII character name (every localisation has them) cannot panic the game thread.
fn char_len_at(text: &str, i: usize) -> usize {
    text[i..].chars().next().map_or(1, char::len_utf8)
}

/// The shipped row header for a character row: name, plus the level and weapon-level groups when
/// their values are known. This is the one the DLL calls.
pub fn row_header_label(values: &RowHeaderValues) -> String {
    expand_row_header(SHIPPED_ROW_HEADER_TEMPLATE, values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_template_renders_name_level_and_weapon_level() {
        let v = RowHeaderValues::from_name("Maddened Bean")
            .with_rune_level(125)
            .with_weapon_level(25);
        assert_eq!(row_header_label(&v), "Maddened Bean, RL 125 WL 25");
    }

    #[test]
    fn an_unknown_weapon_level_drops_its_group_without_trailing_space() {
        let v = RowHeaderValues::from_name("Maddened Bean").with_rune_level(125);
        assert_eq!(row_header_label(&v), "Maddened Bean, RL 125");
    }

    #[test]
    fn an_unknown_level_leaves_the_bare_name() {
        let v = RowHeaderValues::from_name("Maddened Bean");
        assert_eq!(row_header_label(&v), "Maddened Bean");
    }

    /// The group is all-or-nothing. Were it not, an undecoded level would render "Bean RL " and the
    /// dangling caption would read as a rendering bug rather than as missing data.
    #[test]
    fn a_group_drops_entirely_when_any_marker_inside_it_is_unresolved() {
        let v = RowHeaderValues::from_name("Bean");
        assert_eq!(
            expand_row_header("%character_name[ RL %rune_level]", &v),
            "Bean"
        );
        assert_eq!(
            expand_row_header(
                "[%rune_level and %weapon_level]",
                &RowHeaderValues::default()
            ),
            ""
        );
    }

    /// Weapon level is not yet sourced; the shipped template must already be correct for the day it
    /// is, and correct TODAY with the value absent. Both directions are pinned here so wiring the
    /// source in later cannot change the no-source rendering.
    #[test]
    fn weapon_level_zero_renders_but_absent_weapon_level_does_not() {
        let base = RowHeaderValues::from_name("Bean").with_rune_level(1);
        assert_eq!(row_header_label(&base), "Bean, RL 1");
        assert_eq!(
            row_header_label(&base.clone().with_weapon_level(0)),
            "Bean, RL 1 WL 0"
        );
    }

    /// A wrong number on screen is worse than a missing one: an out-of-range value means we decoded
    /// the wrong offset, so it must not reach the label at all.
    #[test]
    fn implausible_values_are_refused_rather_than_rendered() {
        let v = RowHeaderValues::from_name("Bean")
            .with_rune_level(0)
            .with_weapon_level(26);
        assert_eq!(v.rune_level, None);
        assert_eq!(v.weapon_level, None);
        assert_eq!(row_header_label(&v), "Bean");

        let hi = RowHeaderValues::from_name("Bean")
            .with_rune_level(714)
            .with_weapon_level(-1);
        assert_eq!(hi.rune_level, None);
        assert_eq!(hi.weapon_level, None);

        let edges = RowHeaderValues::from_name("Bean")
            .with_rune_level(713)
            .with_weapon_level(25);
        assert_eq!(edges.rune_level, Some(713));
        assert_eq!(edges.weapon_level, Some(25));
    }

    /// Character names are user-authored and localised. Slicing them by byte would panic on the
    /// game thread, which is a crash, not a cosmetic defect.
    #[test]
    fn multibyte_names_and_templates_survive_expansion() {
        let v = RowHeaderValues::from_name("褪せ人・ツリー").with_rune_level(150);
        assert_eq!(row_header_label(&v), "褪せ人・ツリー, RL 150");
        assert_eq!(expand_row_header("★[ ☆%rune_level ☆]★", &v), "★ ☆150 ☆★");
    }

    /// A template typo must degrade the text, never panic a row populate.
    #[test]
    fn malformed_templates_degrade_instead_of_panicking() {
        let v = RowHeaderValues::from_name("Bean").with_rune_level(9);
        assert_eq!(
            expand_row_header("%character_name[ RL %rune_level", &v),
            "Bean RL 9"
        );
        assert_eq!(expand_row_header("%character_name]", &v), "Bean]");
        assert_eq!(expand_row_header("", &v), "");
        assert_eq!(expand_row_header("[]", &v), "");
        assert_eq!(expand_row_header("[literal]", &v), "literal");
    }

    /// Outside a group an unresolved marker is the author saying "optional here", so it expands to
    /// nothing rather than leaking the marker text onto a live row.
    #[test]
    fn an_unresolved_marker_never_renders_its_own_name() {
        let out = expand_row_header("%character_name %rune_level", &RowHeaderValues::default());
        assert!(!out.contains('%'), "marker text leaked to the row: {out:?}");
        assert_eq!(out, " ");
    }

    /// `%rune_level` must not be shadowed by a shorter marker sharing its prefix. There is no such
    /// pair today; this pins the ordering rule before someone adds `%rune`.
    #[test]
    fn markers_are_matched_longest_first() {
        let v = RowHeaderValues::from_name("Bean")
            .with_rune_level(7)
            .with_weapon_level(3);
        assert_eq!(
            expand_row_header("%rune_level/%weapon_level/%character_name", &v),
            "7/3/Bean"
        );
    }
}
