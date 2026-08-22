use crate::profile_05_010_layout::{Profile05_010Layout, TextAlign};
use std::collections::BTreeMap;
use std::fmt;

pub const PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_PROFILE_EDITOR_DIR: &str = "target/pi-local/profile-editor";
pub const CONTROL_FILE_NAME: &str = "control.txt";
pub const STATUS_FILE_NAME: &str = "status.txt";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    MissingKey(&'static str),
    UnknownKey(String),
    InvalidValue(String),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingKey(key) => write!(f, "missing key {key}"),
            Self::UnknownKey(key) => write!(f, "unknown key {key}"),
            Self::InvalidValue(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RenderMode {
    OfflineApproximate,
    LiveRuntime,
}

impl RenderMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OfflineApproximate => "offline_approximate",
            Self::LiveRuntime => "live_runtime",
        }
    }

    fn parse(value: &str) -> Result<Self, ProtocolError> {
        match value {
            "offline_approximate" => Ok(Self::OfflineApproximate),
            "live_runtime" => Ok(Self::LiveRuntime),
            other => Err(ProtocolError::InvalidValue(format!(
                "invalid render_mode {other:?}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelectedKind {
    Field,
    PathEditor,
    Chrome,
    List,
}

impl SelectedKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Field => "field",
            Self::PathEditor => "path_editor",
            Self::Chrome => "chrome",
            Self::List => "list",
        }
    }

    fn parse(value: &str) -> Result<Self, ProtocolError> {
        match value {
            "field" => Ok(Self::Field),
            "path_editor" => Ok(Self::PathEditor),
            "chrome" => Ok(Self::Chrome),
            "list" => Ok(Self::List),
            other => Err(ProtocolError::InvalidValue(format!(
                "invalid selected_kind {other:?}"
            ))),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProfileEditorCommand {
    pub version: u32,
    pub sequence: u64,
    pub render_mode: RenderMode,
    pub selected_kind: SelectedKind,
    pub selected_name: String,
    pub text_probe: bool,
    pub layout: Profile05_010Layout,
}

impl ProfileEditorCommand {
    pub fn from_layout(
        sequence: u64,
        render_mode: RenderMode,
        selected_kind: SelectedKind,
        selected_name: impl Into<String>,
        layout: Profile05_010Layout,
    ) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            sequence,
            render_mode,
            selected_kind,
            selected_name: selected_name.into(),
            text_probe: false,
            layout,
        }
    }

    pub fn serialize(&self) -> String {
        let mut out = String::new();
        push_kv(
            &mut out,
            "profile_editor_protocol",
            &self.version.to_string(),
        );
        push_kv(&mut out, "sequence", &self.sequence.to_string());
        push_kv(&mut out, "render_mode", self.render_mode.as_str());
        push_kv(&mut out, "selected_kind", self.selected_kind.as_str());
        push_kv(&mut out, "selected_name", &self.selected_name);
        push_kv(
            &mut out,
            "text_probe",
            if self.text_probe { "true" } else { "false" },
        );
        for (name, field) in &self.layout.fields {
            push_kv(&mut out, &format!("field.{name}.x"), &field.x.to_string());
            push_kv(&mut out, &format!("field.{name}.y"), &field.y.to_string());
            push_kv(
                &mut out,
                &format!("field.{name}.width"),
                &field.width.to_string(),
            );
            push_kv(
                &mut out,
                &format!("field.{name}.clip_height"),
                &field.clip_height.to_string(),
            );
            push_kv(
                &mut out,
                &format!("field.{name}.font_height"),
                &field.font_height.to_string(),
            );
            push_kv(
                &mut out,
                &format!("field.{name}.align"),
                field.align.as_str(),
            );
        }
        let editor = &self.layout.path_editor;
        push_kv(&mut out, "path_editor.x", &editor.x.to_string());
        push_kv(&mut out, "path_editor.y", &editor.y.to_string());
        push_kv(&mut out, "path_editor.width", &editor.width.to_string());
        push_kv(
            &mut out,
            "path_editor.clip_height",
            &editor.clip_height.to_string(),
        );
        push_kv(
            &mut out,
            "path_editor.font_height",
            &editor.font_height.to_string(),
        );
        push_kv(&mut out, "path_editor.align", editor.align.as_str());
        for (name, transform) in self.layout.row_chrome.sections() {
            push_kv(
                &mut out,
                &format!("row_chrome.{name}.x"),
                &fmt_f32(transform.x),
            );
            push_kv(
                &mut out,
                &format!("row_chrome.{name}.y"),
                &fmt_f32(transform.y),
            );
            push_kv(
                &mut out,
                &format!("row_chrome.{name}.scale_x"),
                &fmt_f32(transform.scale_x),
            );
            push_kv(
                &mut out,
                &format!("row_chrome.{name}.scale_y"),
                &fmt_f32(transform.scale_y),
            );
            push_kv(
                &mut out,
                &format!("row_chrome.{name}.opacity"),
                &fmt_f32(transform.opacity),
            );
            push_kv(
                &mut out,
                &format!("row_chrome.{name}.editable"),
                if transform.editable { "true" } else { "false" },
            );
        }
        for (key, value) in [
            ("row_pitch", self.layout.list.row_pitch),
            ("visible_rows", self.layout.list.visible_rows),
            ("top_recycle_rows", self.layout.list.top_recycle_rows),
            ("bottom_recycle_rows", self.layout.list.bottom_recycle_rows),
            ("mask_height", self.layout.list.mask_height),
            ("scrollbar_x", self.layout.list.scrollbar_x),
            ("scrollbar_top_y", self.layout.list.scrollbar_top_y),
            (
                "scrollbar_track_height",
                self.layout.list.scrollbar_track_height,
            ),
        ] {
            push_kv(&mut out, &format!("list.{key}"), &value.to_string());
        }
        out
    }

    pub fn parse(text: &str) -> Result<Self, ProtocolError> {
        let map = parse_map(text)?;
        let version = required_u32(&map, "profile_editor_protocol")?;
        if version != PROTOCOL_VERSION {
            return Err(ProtocolError::InvalidValue(format!(
                "unsupported profile_editor_protocol {version}"
            )));
        }
        let mut layout = Profile05_010Layout::default();
        for (key, value) in &map {
            if matches!(
                key.as_str(),
                "profile_editor_protocol"
                    | "sequence"
                    | "render_mode"
                    | "selected_kind"
                    | "selected_name"
                    | "text_probe"
            ) {
                continue;
            }
            if let Some(rest) = key.strip_prefix("field.") {
                let (name, prop) = rest
                    .rsplit_once('.')
                    .ok_or_else(|| ProtocolError::UnknownKey(key.clone()))?;
                let field = layout
                    .field_mut(name)
                    .map_err(|_| ProtocolError::UnknownKey(key.clone()))?;
                match prop {
                    "x" => field.x = parse_f32(value, key)?,
                    "y" => field.y = parse_f32(value, key)?,
                    "width" => field.width = parse_i32(value, key)?,
                    "clip_height" => field.clip_height = parse_i32(value, key)?,
                    "font_height" => field.font_height = parse_i32(value, key)?,
                    "align" => {
                        field.align = match value.as_str() {
                            "left" => TextAlign::Left,
                            "right" => TextAlign::Right,
                            "center" => TextAlign::Center,
                            _ => {
                                return Err(ProtocolError::InvalidValue(format!(
                                    "invalid {key}={value:?}"
                                )));
                            }
                        }
                    }
                    _ => return Err(ProtocolError::UnknownKey(key.clone())),
                }
            } else if let Some(prop) = key.strip_prefix("path_editor.") {
                let field = &mut layout.path_editor;
                match prop {
                    "x" => field.x = parse_f32(value, key)?,
                    "y" => field.y = parse_f32(value, key)?,
                    "width" => field.width = parse_i32(value, key)?,
                    "clip_height" => field.clip_height = parse_i32(value, key)?,
                    "font_height" => field.font_height = parse_i32(value, key)?,
                    "align" => {
                        field.align = match value.as_str() {
                            "left" => TextAlign::Left,
                            "right" => TextAlign::Right,
                            "center" => TextAlign::Center,
                            _ => {
                                return Err(ProtocolError::InvalidValue(format!(
                                    "invalid {key}={value:?}"
                                )));
                            }
                        }
                    }
                    _ => return Err(ProtocolError::UnknownKey(key.clone())),
                }
            } else if let Some(rest) = key.strip_prefix("row_chrome.") {
                let (name, prop) = rest
                    .rsplit_once('.')
                    .ok_or_else(|| ProtocolError::UnknownKey(key.clone()))?;
                let Some(transform) = layout.row_chrome.section_mut(name) else {
                    return Err(ProtocolError::UnknownKey(key.clone()));
                };
                match prop {
                    "x" => transform.x = parse_f32(value, key)?,
                    "y" => transform.y = parse_f32(value, key)?,
                    "scale_x" => transform.scale_x = parse_f32(value, key)?,
                    "scale_y" => transform.scale_y = parse_f32(value, key)?,
                    "opacity" => transform.opacity = parse_f32(value, key)?,
                    "editable" => transform.editable = parse_bool(value, key)?,
                    _ => return Err(ProtocolError::UnknownKey(key.clone())),
                }
            } else if let Some(prop) = key.strip_prefix("list.") {
                match prop {
                    "row_pitch" => layout.list.row_pitch = parse_i32(value, key)?,
                    "visible_rows" => layout.list.visible_rows = parse_i32(value, key)?,
                    "top_recycle_rows" => layout.list.top_recycle_rows = parse_i32(value, key)?,
                    "bottom_recycle_rows" => {
                        layout.list.bottom_recycle_rows = parse_i32(value, key)?
                    }
                    "mask_height" => layout.list.mask_height = parse_i32(value, key)?,
                    "scrollbar_x" => layout.list.scrollbar_x = parse_i32(value, key)?,
                    "scrollbar_top_y" => layout.list.scrollbar_top_y = parse_i32(value, key)?,
                    "scrollbar_track_height" => {
                        layout.list.scrollbar_track_height = parse_i32(value, key)?
                    }
                    _ => return Err(ProtocolError::UnknownKey(key.clone())),
                }
            } else {
                return Err(ProtocolError::UnknownKey(key.clone()));
            }
        }
        layout
            .validate()
            .map_err(|e| ProtocolError::InvalidValue(e.to_string()))?;
        Ok(Self {
            version,
            sequence: required_u64(&map, "sequence")?,
            render_mode: RenderMode::parse(required(&map, "render_mode")?)?,
            selected_kind: SelectedKind::parse(required(&map, "selected_kind")?)?,
            selected_name: required(&map, "selected_name")?.to_owned(),
            text_probe: map
                .get("text_probe")
                .map(|value| parse_bool(value, "text_probe"))
                .transpose()?
                .unwrap_or(false),
            layout,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileEditorStatus {
    pub version: u32,
    pub ack_sequence: u64,
    pub connected: bool,
    pub status: String,
    pub active_surface: String,
    pub selected_kind: String,
    pub selected_name: String,
    pub applied_count: u32,
    pub unsupported_count: u32,
    pub error: String,
}

impl ProfileEditorStatus {
    pub fn disconnected() -> Self {
        Self {
            version: PROTOCOL_VERSION,
            ack_sequence: 0,
            connected: false,
            status: "disconnected".to_owned(),
            active_surface: "none".to_owned(),
            selected_kind: String::new(),
            selected_name: String::new(),
            applied_count: 0,
            unsupported_count: 0,
            error: String::new(),
        }
    }

    pub fn serialize(&self) -> String {
        let mut out = String::new();
        push_kv(
            &mut out,
            "profile_editor_protocol",
            &self.version.to_string(),
        );
        push_kv(&mut out, "ack_sequence", &self.ack_sequence.to_string());
        push_kv(
            &mut out,
            "connected",
            if self.connected { "true" } else { "false" },
        );
        push_kv(&mut out, "status", &self.status);
        push_kv(&mut out, "active_surface", &self.active_surface);
        push_kv(&mut out, "selected_kind", &self.selected_kind);
        push_kv(&mut out, "selected_name", &self.selected_name);
        push_kv(&mut out, "applied_count", &self.applied_count.to_string());
        push_kv(
            &mut out,
            "unsupported_count",
            &self.unsupported_count.to_string(),
        );
        push_kv(&mut out, "error", &self.error);
        out
    }

    pub fn parse(text: &str) -> Result<Self, ProtocolError> {
        let map = parse_map(text)?;
        let version = required_u32(&map, "profile_editor_protocol")?;
        if version != PROTOCOL_VERSION {
            return Err(ProtocolError::InvalidValue(format!(
                "unsupported profile_editor_protocol {version}"
            )));
        }
        Ok(Self {
            version,
            ack_sequence: required_u64(&map, "ack_sequence")?,
            connected: parse_bool(required(&map, "connected")?, "connected")?,
            status: required(&map, "status")?.to_owned(),
            active_surface: required(&map, "active_surface")?.to_owned(),
            selected_kind: required(&map, "selected_kind")?.to_owned(),
            selected_name: required(&map, "selected_name")?.to_owned(),
            applied_count: required(&map, "applied_count")?
                .parse()
                .map_err(|e| ProtocolError::InvalidValue(format!("invalid applied_count: {e}")))?,
            unsupported_count: required(&map, "unsupported_count")?.parse().map_err(|e| {
                ProtocolError::InvalidValue(format!("invalid unsupported_count: {e}"))
            })?,
            error: required(&map, "error")?.to_owned(),
        })
    }
}

fn push_kv(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(" = ");
    out.push_str(&quote(value));
    out.push('\n');
}

fn quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
        && !value.is_empty()
    {
        value.to_owned()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn fmt_f32(value: f32) -> String {
    let mut s = format!("{value:.3}");
    while s.contains('.') && s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.push('0');
    }
    s
}

fn parse_map(text: &str) -> Result<BTreeMap<String, String>, ProtocolError> {
    let mut map = BTreeMap::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| {
            ProtocolError::InvalidValue(format!("expected key = value: {line:?}"))
        })?;
        map.insert(key.trim().to_owned(), unquote(value.trim()));
    }
    Ok(map)
}

fn unquote(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
}

fn required<'a>(
    map: &'a BTreeMap<String, String>,
    key: &'static str,
) -> Result<&'a str, ProtocolError> {
    map.get(key)
        .map(String::as_str)
        .ok_or(ProtocolError::MissingKey(key))
}

fn required_u32(map: &BTreeMap<String, String>, key: &'static str) -> Result<u32, ProtocolError> {
    required(map, key)?
        .parse()
        .map_err(|e| ProtocolError::InvalidValue(format!("invalid {key}: {e}")))
}

fn required_u64(map: &BTreeMap<String, String>, key: &'static str) -> Result<u64, ProtocolError> {
    required(map, key)?
        .parse()
        .map_err(|e| ProtocolError::InvalidValue(format!("invalid {key}: {e}")))
}

fn parse_i32(value: &str, key: &str) -> Result<i32, ProtocolError> {
    value
        .parse()
        .map_err(|e| ProtocolError::InvalidValue(format!("invalid {key}: {e}")))
}

fn parse_f32(value: &str, key: &str) -> Result<f32, ProtocolError> {
    value
        .parse()
        .map_err(|e| ProtocolError::InvalidValue(format!("invalid {key}: {e}")))
}

fn parse_bool(value: &str, key: &str) -> Result<bool, ProtocolError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(ProtocolError::InvalidValue(format!(
            "invalid {key}: {value:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the tests name this type; a crate-level `use` would be dead in the lib build.
    use crate::profile_05_010_layout::RowChromeLayout;

    #[test]
    fn command_round_trips_layout_and_selected_object() {
        let mut layout = Profile05_010Layout::default();
        layout.field_mut("PlayerName").unwrap().x = -512.25;
        layout.row_chrome.cursor.scale_y = 0.375;
        let command = ProfileEditorCommand::from_layout(
            7,
            RenderMode::LiveRuntime,
            SelectedKind::Chrome,
            "cursor",
            layout,
        );
        let parsed = ProfileEditorCommand::parse(&command.serialize()).expect("command parses");
        assert_eq!(parsed.sequence, 7);
        assert_eq!(parsed.render_mode, RenderMode::LiveRuntime);
        assert_eq!(parsed.selected_kind, SelectedKind::Chrome);
        assert_eq!(parsed.selected_name, "cursor");
        assert_eq!(parsed.layout.field("PlayerName").x, -512.25);
        assert_eq!(parsed.layout.row_chrome.cursor.scale_y, 0.375);
    }

    #[test]
    fn active_path_editor_is_not_aliased_to_the_read_only_current_path_field() {
        let command = ProfileEditorCommand::from_layout(
            8,
            RenderMode::LiveRuntime,
            SelectedKind::PathEditor,
            "ActivePathEditor",
            Profile05_010Layout::default(),
        );
        let parsed = ProfileEditorCommand::parse(&command.serialize()).expect("command parses");
        assert_eq!(parsed.selected_kind, SelectedKind::PathEditor);
        assert_eq!(parsed.selected_name, "ActivePathEditor");
    }

    /// EVERY chrome section the schema knows must survive a live-control round trip.
    ///
    /// The channel fails closed on one unknown key, so a section present in the schema but missing
    /// from the protocol rejects the WHOLE control file. That happened when `hit_area` was added
    /// (2026-08-12): the game ran fine, the editor showed no ack, and the only evidence was an
    /// `unknown key row_chrome.hit_area.editable` line inside `status.txt`.
    #[test]
    fn every_schema_chrome_section_survives_a_live_control_round_trip() {
        let mut layout = Profile05_010Layout::default();
        // A distinct, non-default x per section proves the value lands in the right one rather
        // than every name resolving onto a single shared transform.
        for (index, name) in RowChromeLayout::SECTION_NAMES.iter().enumerate() {
            layout
                .row_chrome
                .section_mut(name)
                .unwrap_or_else(|| panic!("{name} must resolve"))
                .x = -100.0 - index as f32;
        }
        let command = ProfileEditorCommand::from_layout(
            9,
            RenderMode::LiveRuntime,
            SelectedKind::Chrome,
            "hit_area",
            layout,
        );
        let text = command.serialize();
        let mut parsed = ProfileEditorCommand::parse(&text)
            .unwrap_or_else(|e| panic!("every chrome section must parse: {e}\n{text}"));
        for (index, name) in RowChromeLayout::SECTION_NAMES.iter().enumerate() {
            assert_eq!(
                parsed.layout.row_chrome.section_mut(name).unwrap().x,
                -100.0 - index as f32,
                "{name} did not round trip"
            );
        }
    }

    #[test]
    fn command_unknown_keys_fail_closed() {
        let text = "profile_editor_protocol = 1\nsequence = 1\nrender_mode = live_runtime\nselected_kind = field\nselected_name = PlayerName\nbanana = 1\n";
        let err = ProfileEditorCommand::parse(text).expect_err("unknown protocol key must fail");
        assert!(err.to_string().contains("unknown key"), "{err}");
    }

    #[test]
    fn status_round_trips_runtime_ack() {
        let status = ProfileEditorStatus {
            version: PROTOCOL_VERSION,
            ack_sequence: 12,
            connected: true,
            status: "unsupported".to_owned(),
            active_surface: "ProfileSelect row_proxy=0x1234".to_owned(),
            selected_kind: "field".to_owned(),
            selected_name: "PlayerName".to_owned(),
            applied_count: 0,
            unsupported_count: 1,
            error: "live transform application stubbed".to_owned(),
        };
        assert_eq!(
            ProfileEditorStatus::parse(&status.serialize()).unwrap(),
            status
        );
    }
}
