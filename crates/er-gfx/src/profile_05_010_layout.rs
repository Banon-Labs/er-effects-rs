use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

pub const DEFAULT_PROFILE_05_010_LAYOUT_PATH: &str = "crates/er-gfx/profile_05_010_layout.toml";
pub const FIELD_NAMES: [&str; 10] = [
    "PlayerName",
    "StaticText_110502",
    "Level",
    "Location",
    "PlayTime",
    "ErStats",
    "ErCharStats",
    "DriveCell_0",
    "DriveCell_1",
    "DriveCell_2",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextAlign {
    Left,
    Right,
    Center,
}

impl TextAlign {
    pub fn swf_code(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Right => 1,
            Self::Center => 2,
        }
    }

    fn parse(value: &str) -> Result<Self, LayoutError> {
        match value.trim().trim_matches('"') {
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            "center" => Ok(Self::Center),
            other => Err(LayoutError::InvalidValue(format!(
                "align must be left/right/center, got {other:?}"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Center => "center",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldLayout {
    pub x: f32,
    pub y: f32,
    pub width: i32,
    pub clip_height: i32,
    pub font_height: i32,
    pub align: TextAlign,
    pub sample_load_character: String,
    pub sample_save_picker: String,
    pub sample_drive_row: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransformLayout {
    pub x: f32,
    pub y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub editable: bool,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowChromeLayout {
    pub backing: TransformLayout,
    pub cursor: TransformLayout,
    pub cursor_body: TransformLayout,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListGeometryLayout {
    pub row_pitch: i32,
    pub visible_rows: i32,
    pub top_recycle_rows: i32,
    pub bottom_recycle_rows: i32,
    pub mask_height: i32,
    pub scrollbar_x: i32,
    pub scrollbar_top_y: i32,
    pub scrollbar_track_height: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Profile05_010Layout {
    pub fields: BTreeMap<String, FieldLayout>,
    pub row_chrome: RowChromeLayout,
    pub list: ListGeometryLayout,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutError {
    Io(String),
    Parse { line: usize, message: String },
    UnknownSection(String),
    UnknownKey(String),
    MissingField(String),
    InvalidValue(String),
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) => write!(f, "{message}"),
            Self::Parse { line, message } => write!(f, "line {line}: {message}"),
            Self::UnknownSection(section) => write!(f, "unknown section [{section}]"),
            Self::UnknownKey(key) => write!(f, "unknown key {key}"),
            Self::MissingField(field) => write!(f, "missing layout field {field}"),
            Self::InvalidValue(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for LayoutError {}

/// The schema file, embedded at compile time. Nothing reads the TOML from disk at runtime: these
/// bytes are the shipped layout, baked into the DLL.
const SHIPPED_LAYOUT_TOML: &str = include_str!("../profile_05_010_layout.toml");

impl Default for Profile05_010Layout {
    /// The values this crate ships, which are the checked-in TOML and nothing else.
    ///
    /// This used to be a second hard-coded copy of every number, and it drifted: seven of the ten
    /// fields disagreed with the TOML (`PlayerName` was still `x -529 w252` against the schema's
    /// `x -520 w1200`). That mattered because `default()` is live at runtime -- it is the base
    /// `profile_05_010_protocol` layers an incoming control file onto, so any field a control file
    /// omitted silently inherited a stale number. One source of truth now; [`bootstrap`] exists
    /// only to give the parser a struct to fill and can never be observed with the schema present.
    ///
    /// [`bootstrap`]: Profile05_010Layout::bootstrap
    fn default() -> Self {
        static SHIPPED: OnceLock<Profile05_010Layout> = OnceLock::new();
        SHIPPED
            .get_or_init(|| {
                Profile05_010Layout::parse_from(
                    Profile05_010Layout::bootstrap(),
                    SHIPPED_LAYOUT_TOML,
                )
                .expect("checked-in profile_05_010_layout.toml must parse and validate")
            })
            .clone()
    }
}

impl Profile05_010Layout {
    /// Structural seed for the parser only -- every value here is overwritten by
    /// [`SHIPPED_LAYOUT_TOML`], which specifies all ten fields in full. Do not treat these numbers
    /// as meaningful and do not tune them; edit the TOML.
    fn bootstrap() -> Self {
        let mut fields = BTreeMap::new();
        for (name, field) in [
            (
                "PlayerName",
                field(
                    -529,
                    -14,
                    252,
                    24,
                    TextAlign::Left,
                    "Maddened Bean",
                    "[..] save-files",
                    "Save Root",
                ),
            ),
            (
                "StaticText_110502",
                field(-352, -14, 149, 24, TextAlign::Left, "Level", "", ""),
            ),
            (
                "Level",
                field(-282, -14, 52, 24, TextAlign::Right, "125", "", ""),
            ),
            (
                "Location",
                field(252, -14, 152, 24, TextAlign::Right, "Midra's Manse", "", ""),
            ),
            (
                "PlayTime",
                field(
                    326,
                    -14,
                    200,
                    24,
                    TextAlign::Right,
                    "107:49:34",
                    "2026-07-07",
                    "2026-07-07",
                ),
            ),
            (
                "ErStats",
                field(
                    -324,
                    -14,
                    587,
                    24,
                    TextAlign::Left,
                    "",
                    "PARENT FOLDER / Go to save-files",
                    "45-Slots / ER0000.sl2",
                ),
            ),
            (
                "ErCharStats",
                field(
                    -161,
                    -6,
                    425,
                    16,
                    TextAlign::Center,
                    "VIG 50 MND 10 END 50 STR 21 DEX 21 INT 10 FAI 35 ARC 7",
                    "",
                    "",
                ),
            ),
            (
                "DriveCell_0",
                field(-325, -16, 64, 24, TextAlign::Center, "", "[C:]", "[C:]"),
            ),
            (
                "DriveCell_1",
                field(-247, -16, 64, 24, TextAlign::Center, "", "[S:]", "[S:]"),
            ),
            (
                "DriveCell_2",
                field(-166, -16, 64, 24, TextAlign::Center, "", ">Z:<", ">Z:<"),
            ),
        ] {
            fields.insert(name.to_owned(), field);
        }
        Self {
            fields,
            row_chrome: RowChromeLayout {
                backing: transform(
                    -20.0,
                    0.0,
                    20.0,
                    1.0,
                    true,
                    "normal non-highlighted row backing: Backing/char54 contains MENU_FL_SelectWaku, the same 56x54 Save Game-style button frame art",
                ),
                cursor: transform(
                    -20.0,
                    0.0,
                    1.0,
                    1.0,
                    true,
                    "outer highlighted-row cursor wrapper: row sprite 76 named Cursor / char 75; keep identity so CursorBody locks to Backing dimensions",
                ),
                cursor_body: transform(
                    0.0,
                    0.0,
                    20.0,
                    1.0,
                    true,
                    "inner highlighted-row cursor body: Cursor/char75 -> CursorBody/char73 -> MENU_FL_Select_Cursor, the same 56x54 art used by Save Game / Return to Desktop style buttons",
                ),
            },
            list: ListGeometryLayout {
                row_pitch: 48,
                visible_rows: 10,
                top_recycle_rows: 1,
                bottom_recycle_rows: 1,
                mask_height: 480,
                scrollbar_x: 567,
                scrollbar_top_y: -240,
                scrollbar_track_height: 480,
            },
        }
    }
}

/// Menu font vertical metrics, read out of the game's own `data0:/font/eu_std/font.gfx`
/// `DefineFont3` id=1 ("Agmena W1G For Bandai", 910 glyphs) layout block.
///
/// These are why a text field has a floor on its box height. Scaleform lays one line out as
/// `(ascent + descent) / em_square * font_height` pixels; `GFx::TextField` reflow
/// (`FUN_14114bf10`) then insets the usable area by [`TEXT_DOC_INSET_PX_PER_EDGE`] on every
/// edge (`+0x80 = [+0xb0] + 40.0f` … `+0x8c = [+0xbc] - 40.0f`, twips). A box below that sum
/// cannot hold its own text, which is exactly how `PlayerName` shipped at `clip_height = 30`
/// against a 38.383 px requirement and rendered short. Vanilla's own `PlayerName` rect is
/// 40 px -- the tightest round number that clears it.
pub const MENU_FONT_ASCENT: i32 = 20800;
pub const MENU_FONT_DESCENT: i32 = 8540;
/// Fixed by the SWF spec for `DefineFont3`: glyph coordinates are 1/20 twip, i.e. 1024 * 20.
pub const MENU_FONT_EM_SQUARE: i32 = 20480;
/// 40 twips per edge in the reflow routine = 2 px, top and bottom both.
pub const TEXT_DOC_INSET_PX_PER_EDGE: i32 = 2;

/// Height in px of one laid-out line of the menu font at `font_height`.
pub fn line_box_px(font_height: i32) -> f32 {
    (MENU_FONT_ASCENT + MENU_FONT_DESCENT) as f32 / MENU_FONT_EM_SQUARE as f32 * font_height as f32
}

/// Smallest `clip_height` that can render one line at `font_height`, inset included.
///
/// Rounds up: a fractional shortfall still clips, so `38.383` demands `39`.
pub fn min_clip_height_px(font_height: i32) -> i32 {
    (line_box_px(font_height) + (2 * TEXT_DOC_INSET_PX_PER_EDGE) as f32).ceil() as i32
}

/// Inverse of [`min_clip_height_px`]: the largest `font_height` a box of `clip_height` can render
/// one line of. Zero when the box cannot hold any line at all.
///
/// This is the ceiling the LIVE path needs. `font_height` hot-reloads instantly through the
/// `<font size>` wrap, but `clip_height` is baked into the movie and is never applied live -- so a
/// font raised past this bound overflows a box that will not grow until the asset is rebuilt and
/// the screen reopened.
pub fn max_font_height_px(clip_height: i32) -> i32 {
    let usable = clip_height - 2 * TEXT_DOC_INSET_PX_PER_EDGE;
    if usable <= 0 {
        return 0;
    }
    (usable as f32 * MENU_FONT_EM_SQUARE as f32 / (MENU_FONT_ASCENT + MENU_FONT_DESCENT) as f32)
        .floor() as i32
}

/// The shipped layout, parsed once. Borrow this instead of cloning through [`Default`] on a hot
/// path such as a per-SetText clamp.
pub fn shipped() -> &'static Profile05_010Layout {
    static SHIPPED_REF: OnceLock<Profile05_010Layout> = OnceLock::new();
    SHIPPED_REF.get_or_init(Profile05_010Layout::default)
}

fn field(
    x: i32,
    y: i32,
    width: i32,
    font_height: i32,
    align: TextAlign,
    sample_load_character: &str,
    sample_save_picker: &str,
    sample_drive_row: &str,
) -> FieldLayout {
    FieldLayout {
        x: x as f32,
        y: y as f32,
        width,
        clip_height: 40,
        font_height,
        align,
        sample_load_character: sample_load_character.to_owned(),
        sample_save_picker: sample_save_picker.to_owned(),
        sample_drive_row: sample_drive_row.to_owned(),
    }
}

fn transform(
    x: f32,
    y: f32,
    scale_x: f32,
    scale_y: f32,
    editable: bool,
    source: &str,
) -> TransformLayout {
    TransformLayout {
        x,
        y,
        scale_x,
        scale_y,
        editable,
        source: source.to_owned(),
    }
}

impl Profile05_010Layout {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, LayoutError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path)
            .map_err(|e| LayoutError::Io(format!("read {}: {e}", path.display())))?;
        Self::parse(&text)
    }

    /// Parse `text` as overrides on top of the shipped defaults, so a partial file (a control file
    /// carrying one field, say) inherits the values this crate actually ships rather than a second,
    /// drifting set.
    pub fn parse(text: &str) -> Result<Self, LayoutError> {
        Self::parse_from(Self::default(), text)
    }

    fn parse_from(base: Self, text: &str) -> Result<Self, LayoutError> {
        let mut layout = base;
        let mut section = String::new();
        for (idx, raw) in text.lines().enumerate() {
            let line_no = idx + 1;
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                validate_section(inner)?;
                section.clear();
                section.push_str(inner);
                continue;
            }
            let (key, value) = line.split_once('=').ok_or_else(|| LayoutError::Parse {
                line: line_no,
                message: "expected key = value".to_owned(),
            })?;
            layout
                .set(&section, key.trim(), value.trim())
                .map_err(|e| match e {
                    LayoutError::Parse { .. } => e,
                    other => LayoutError::Parse {
                        line: line_no,
                        message: other.to_string(),
                    },
                })?;
        }
        layout.validate()?;
        Ok(layout)
    }

    pub fn field(&self, name: &str) -> &FieldLayout {
        self.fields
            .get(name)
            .unwrap_or_else(|| panic!("missing ProfileSelect field layout {name}"))
    }

    pub fn field_mut(&mut self, name: &str) -> Result<&mut FieldLayout, LayoutError> {
        self.fields
            .get_mut(name)
            .ok_or_else(|| LayoutError::MissingField(name.to_owned()))
    }

    pub fn validate(&self) -> Result<(), LayoutError> {
        for name in FIELD_NAMES {
            let field = self
                .fields
                .get(name)
                .ok_or_else(|| LayoutError::MissingField(name.to_owned()))?;
            if field.width <= 0 {
                return Err(LayoutError::InvalidValue(format!(
                    "field.{name}.width must be > 0"
                )));
            }
            if field.clip_height <= 0 {
                return Err(LayoutError::InvalidValue(format!(
                    "field.{name}.clip_height must be > 0"
                )));
            }
            if field.font_height <= 0 {
                return Err(LayoutError::InvalidValue(format!(
                    "field.{name}.font_height must be > 0"
                )));
            }
            let minimum = min_clip_height_px(field.font_height);
            if field.clip_height < minimum {
                return Err(LayoutError::InvalidValue(format!(
                    "field.{name}.clip_height {} cannot render one line of its own font: \
                     font_height {} needs a {:.3} px line box ({MENU_FONT_ASCENT} ascent + \
                     {MENU_FONT_DESCENT} descent over {MENU_FONT_EM_SQUARE} em) plus the \
                     {TEXT_DOC_INSET_PX_PER_EDGE} px/edge reflow inset, so clip_height must be >= {minimum}",
                    field.clip_height,
                    field.font_height,
                    line_box_px(field.font_height),
                )));
            }
        }
        if self.list.visible_rows != 10 {
            return Err(LayoutError::InvalidValue(
                "list.visible_rows must stay 10; increasing native grid item count breaks picker scrollbar/native rows".to_owned(),
            ));
        }
        if self.list.row_pitch <= 0
            || self.list.mask_height <= 0
            || self.list.scrollbar_track_height <= 0
        {
            return Err(LayoutError::InvalidValue(
                "list row_pitch/mask_height/scrollbar_track_height must be positive".to_owned(),
            ));
        }
        for (name, t) in [
            ("row_chrome.backing", &self.row_chrome.backing),
            ("row_chrome.cursor", &self.row_chrome.cursor),
            ("row_chrome.cursor_body", &self.row_chrome.cursor_body),
        ] {
            if t.scale_x == 0.0 || t.scale_y == 0.0 {
                return Err(LayoutError::InvalidValue(format!(
                    "{name} scale_x/scale_y must be nonzero"
                )));
            }
        }
        Ok(())
    }

    fn set(&mut self, section: &str, key: &str, raw_value: &str) -> Result<(), LayoutError> {
        if let Some(name) = section.strip_prefix("field.") {
            let field = self.field_mut(name)?;
            match key {
                "x" => field.x = parse_f32(raw_value)?,
                "y" => field.y = parse_f32(raw_value)?,
                "width" => field.width = parse_i32(raw_value)?,
                "clip_height" => field.clip_height = parse_i32(raw_value)?,
                "font_height" => field.font_height = parse_i32(raw_value)?,
                "align" => field.align = TextAlign::parse(raw_value)?,
                "sample_load_character" => field.sample_load_character = parse_string(raw_value),
                "sample_save_picker" => field.sample_save_picker = parse_string(raw_value),
                "sample_drive_row" => field.sample_drive_row = parse_string(raw_value),
                _ => return Err(LayoutError::UnknownKey(format!("[{section}].{key}"))),
            }
            return Ok(());
        }
        match section {
            "row_chrome.backing" => {
                set_transform(&mut self.row_chrome.backing, section, key, raw_value)?
            }
            "row_chrome.cursor" => {
                set_transform(&mut self.row_chrome.cursor, section, key, raw_value)?
            }
            "row_chrome.cursor_body" => {
                set_transform(&mut self.row_chrome.cursor_body, section, key, raw_value)?
            }
            "list" => match key {
                "row_pitch" => self.list.row_pitch = parse_i32(raw_value)?,
                "visible_rows" => self.list.visible_rows = parse_i32(raw_value)?,
                "top_recycle_rows" => self.list.top_recycle_rows = parse_i32(raw_value)?,
                "bottom_recycle_rows" => self.list.bottom_recycle_rows = parse_i32(raw_value)?,
                "mask_height" => self.list.mask_height = parse_i32(raw_value)?,
                "scrollbar_x" => self.list.scrollbar_x = parse_i32(raw_value)?,
                "scrollbar_top_y" => self.list.scrollbar_top_y = parse_i32(raw_value)?,
                "scrollbar_track_height" => {
                    self.list.scrollbar_track_height = parse_i32(raw_value)?
                }
                _ => return Err(LayoutError::UnknownKey(format!("[{section}].{key}"))),
            },
            "" => return Err(LayoutError::UnknownKey(key.to_owned())),
            _ => return Err(LayoutError::UnknownSection(section.to_owned())),
        }
        Ok(())
    }
}

fn set_transform(
    transform: &mut TransformLayout,
    section: &str,
    key: &str,
    raw_value: &str,
) -> Result<(), LayoutError> {
    match key {
        "x" => transform.x = parse_f32(raw_value)?,
        "y" => transform.y = parse_f32(raw_value)?,
        "scale_x" => transform.scale_x = parse_f32(raw_value)?,
        "scale_y" => transform.scale_y = parse_f32(raw_value)?,
        "editable" => transform.editable = parse_bool(raw_value)?,
        "source" => transform.source = parse_string(raw_value),
        _ => return Err(LayoutError::UnknownKey(format!("[{section}].{key}"))),
    }
    Ok(())
}

fn validate_section(section: &str) -> Result<(), LayoutError> {
    if let Some(name) = section.strip_prefix("field.") {
        if FIELD_NAMES.contains(&name) {
            return Ok(());
        }
        return Err(LayoutError::UnknownSection(section.to_owned()));
    }
    match section {
        "row_chrome.backing" | "row_chrome.cursor" | "row_chrome.cursor_body" | "list" => Ok(()),
        _ => Err(LayoutError::UnknownSection(section.to_owned())),
    }
}

fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut prev_escape = false;
    for (idx, ch) in line.char_indices() {
        if ch == '"' && !prev_escape {
            in_string = !in_string;
        }
        if ch == '#' && !in_string {
            return &line[..idx];
        }
        prev_escape = ch == '\\' && !prev_escape;
        if ch != '\\' {
            prev_escape = false;
        }
    }
    line
}

fn parse_i32(value: &str) -> Result<i32, LayoutError> {
    value
        .parse::<i32>()
        .map_err(|e| LayoutError::InvalidValue(format!("invalid integer {value:?}: {e}")))
}

fn parse_f32(value: &str) -> Result<f32, LayoutError> {
    value
        .parse::<f32>()
        .map_err(|e| LayoutError::InvalidValue(format!("invalid float {value:?}: {e}")))
}

fn parse_bool(value: &str) -> Result<bool, LayoutError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(LayoutError::InvalidValue(format!("invalid bool {value:?}"))),
    }
}

fn parse_string(value: &str) -> String {
    let value = value.trim();
    let Some(inner) = value.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return value.to_owned();
    };
    inner
        .replace("\\\"", "\"")
        .replace("\\n", "\n")
        .replace("\\t", "\t")
        .replace("\\\\", "\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_schema_parses_and_contains_required_controls() {
        let layout = Profile05_010Layout::parse(include_str!("../profile_05_010_layout.toml"))
            .expect("checked-in ProfileSelect layout schema parses");
        for name in FIELD_NAMES {
            let field = layout.field(name);
            assert!(field.width > 0, "{name} width exposed");
            assert!(field.clip_height > 0, "{name} clip height exposed");
            assert!(field.font_height > 0, "{name} font height exposed");
        }
        assert!(layout.row_chrome.backing.editable);
        assert!(layout.row_chrome.cursor.editable);
        assert!(layout.row_chrome.cursor_body.editable);
        assert_eq!(layout.list.visible_rows, 10);
        assert_eq!(layout.list.row_pitch, 48);
    }

    #[test]
    fn field_positions_accept_fractional_pixels() {
        let layout = Profile05_010Layout::parse("[field.StaticText_110502]\ny = -17.25\n")
            .expect("fractional ProfileSelect field y parses");
        assert_eq!(layout.field("StaticText_110502").y, -17.25);
    }

    #[test]
    fn unknown_keys_fail_closed() {
        let err = Profile05_010Layout::parse("[field.PlayerName]\nbanana = 7\n")
            .expect_err("unknown field key must fail");
        assert!(err.to_string().contains("unknown key"), "{err}");
        let err = Profile05_010Layout::parse("[field.Nope]\nx = 1\n")
            .expect_err("unknown field section must fail");
        assert!(err.to_string().contains("unknown section"), "{err}");
    }

    #[test]
    fn visible_rows_cannot_exceed_native_picker_transport() {
        let err = Profile05_010Layout::parse("[list]\nvisible_rows = 11\n")
            .expect_err("visible rows above native ten-slot transport must fail");
        assert!(
            err.to_string().contains("visible_rows must stay 10"),
            "{err}"
        );
    }

    /// The regression this exists to prevent: `PlayerName` shipped at `clip_height = 30` with
    /// `font_height = 24`, which is 8.383 px short of one line box plus the reflow inset, and the
    /// character name rendered short in game. The old rule was `font_height <= clip_height`, which
    /// 24-in-30 satisfies, so nothing caught it.
    #[test]
    fn clip_height_below_the_font_line_box_fails_closed() {
        let err =
            Profile05_010Layout::parse("[field.PlayerName]\nclip_height = 30\nfont_height = 24\n")
                .expect_err("a box too short for one line of its own font must fail");
        let message = err.to_string();
        assert!(message.contains("cannot render one line"), "{err}");
        assert!(message.contains(">= 39"), "{err}");

        // The exact boundary, so nobody "fixes" this by loosening the rounding: 24 px of Agmena
        // is a 34.383 px line box, +4 px inset = 38.383, so 38 must fail and 39 must pass.
        assert_eq!(min_clip_height_px(24), 39);
        assert!(
            Profile05_010Layout::parse("[field.PlayerName]\nclip_height = 38\nfont_height = 24\n")
                .is_err(),
            "38 px still clips a 38.383 px requirement"
        );
        assert!(
            Profile05_010Layout::parse("[field.PlayerName]\nclip_height = 39\nfont_height = 24\n")
                .is_ok(),
            "39 px clears the requirement"
        );
    }

    /// The live font ceiling and the schema floor must be exact inverses, or one of them lies.
    ///
    /// `max_font_height_px` is what the DLL clamps a live `font_height` to, because `clip_height`
    /// is baked into the movie and never applied live. If it ever disagreed with
    /// `min_clip_height_px`, the clamp would either overflow the box or shrink text that fit.
    #[test]
    fn the_live_font_ceiling_is_the_exact_inverse_of_the_clip_height_floor() {
        for clip in 8..=120 {
            let ceiling = max_font_height_px(clip);
            if ceiling <= 0 {
                continue;
            }
            assert!(
                min_clip_height_px(ceiling) <= clip,
                "clip {clip} says font {ceiling} fits, but the floor for {ceiling} is {}",
                min_clip_height_px(ceiling)
            );
            assert!(
                min_clip_height_px(ceiling + 1) > clip,
                "clip {clip} rejects font {} but the floor says it fits",
                ceiling + 1
            );
        }
        // The two shipped box heights, spelled out.
        assert_eq!(max_font_height_px(40), 25);
        assert_eq!(max_font_height_px(42), 26);
        // A box too small for any line yields no ceiling rather than a negative one.
        assert_eq!(max_font_height_px(4), 0);
    }

    /// `default()` must BE the shipped schema, and a partial parse must inherit it.
    ///
    /// The regression: `default()` was a second hard-coded copy of every number and had drifted from
    /// the TOML in seven of ten fields. `profile_05_010_protocol` layers an incoming control file
    /// onto `default()`, so any field the control file omitted resolved to a stale value at runtime
    /// -- `PlayerName` would have come back `x -529 w252` instead of the shipped `x -520 w1200`.
    #[test]
    fn defaults_are_the_shipped_schema_and_partial_parses_inherit_them() {
        let shipped = Profile05_010Layout::parse(SHIPPED_LAYOUT_TOML).expect("schema parses");
        let default = Profile05_010Layout::default();
        for name in FIELD_NAMES {
            assert_eq!(
                default.field(name),
                shipped.field(name),
                "field.{name} default drifted from profile_05_010_layout.toml"
            );
        }
        assert_eq!(default.list, shipped.list);
        assert_eq!(default.row_chrome, shipped.row_chrome);

        // A control file that mentions only `list` must leave every field at the shipped value.
        let partial = Profile05_010Layout::parse("[list]\nrow_pitch = 48\n")
            .expect("a partial layout parses as overrides on the shipped defaults");
        for name in FIELD_NAMES {
            assert_eq!(
                partial.field(name),
                shipped.field(name),
                "field.{name} did not inherit the shipped value through a partial parse"
            );
        }
    }

    /// Every field in the checked-in schema must clear its own font, not just `PlayerName`.
    #[test]
    fn checked_in_schema_fields_all_clear_their_font_line_box() {
        let layout = Profile05_010Layout::parse(include_str!("../profile_05_010_layout.toml"))
            .expect("checked-in schema parses");
        for name in FIELD_NAMES {
            let field = layout.field(name);
            let minimum = min_clip_height_px(field.font_height);
            assert!(
                field.clip_height >= minimum,
                "field.{name}: clip_height {} < {minimum} required for font_height {} \
                 (line box {:.3} px + {} px/edge inset)",
                field.clip_height,
                field.font_height,
                line_box_px(field.font_height),
                TEXT_DOC_INSET_PX_PER_EDGE,
            );
        }
    }
}
