use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use er_save_loader::{SaveLoadMethod, SaveLoadRequest};
use windows::Win32::{
    Foundation::{HINSTANCE, HMODULE},
    System::LibraryLoader::GetModuleFileNameW,
};

use crate::telemetry::{append_autoload_debug, game_directory_path};

const CONFIG_FILE_NAME: &str = "er-effects.toml";
const SAVE_FILE_ENV: &str = "ER_EFFECTS_SAVE_FILE";
const SLOT_ENV: &str = "ER_EFFECTS_AUTOLOAD_SLOT";
const METHOD_ENV: &str = "ER_EFFECTS_AUTOLOAD_METHOD";
const MENU_SORT_ARMAMENTS_ENV: &str = "ER_EFFECTS_MENU_SORT_ARMAMENTS";
const MENU_SORT_ARMOR_ENV: &str = "ER_EFFECTS_MENU_SORT_ARMOR";
const MENU_SORT_TALISMANS_ENV: &str = "ER_EFFECTS_MENU_SORT_TALISMANS";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MenuSortDefault {
    /// Keep the game's vanilla boot value untouched for this category.
    Preserve,
    /// GR_MenuText 6105 / comparator id 0x5141.
    ItemType,
    /// GR_MenuText 6190 / comparator id 0x5140.
    OrderOfAcquisition,
}

impl MenuSortDefault {
    pub(crate) fn label(self) -> &'static str {
        match self {
            MenuSortDefault::Preserve => "preserve",
            MenuSortDefault::ItemType => "item_type",
            MenuSortDefault::OrderOfAcquisition => "order_of_acquisition",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RuntimeConfig {
    pub path: PathBuf,
    pub save_file: Option<PathBuf>,
    pub slot: Option<i32>,
    pub method: Option<String>,
    pub boot_background_image: Option<PathBuf>,
    pub persist_boot_background_to_loading_screen: Option<bool>,
    pub menu_sort_armaments: Option<MenuSortDefault>,
    pub menu_sort_armor: Option<MenuSortDefault>,
    pub menu_sort_talismans: Option<MenuSortDefault>,
    pub preferred_save_picker_dir: Option<PathBuf>,
    pub autoupdate_preferred_picker_dir: Option<bool>,
}

static RUNTIME_CONFIG: OnceLock<Result<RuntimeConfig, String>> = OnceLock::new();

pub(crate) fn init_runtime_config(hmodule: HINSTANCE) {
    let _ = RUNTIME_CONFIG.set(load_runtime_config(hmodule));
    match RUNTIME_CONFIG.get() {
        Some(Ok(config)) => append_autoload_debug(format_args!(
            "runtime-config: loaded '{}' save_file={} slot={} method={} boot_background_image={} persist_boot_background_to_loading_screen={} menu_sort.armaments={} menu_sort.armor={} menu_sort.talismans={} preferred_save_picker_dir={} autoupdate_preferred_picker_dir={}",
            config.path.display(),
            config
                .save_file
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<unset>".to_owned()),
            config
                .slot
                .map(|v| v.to_string())
                .unwrap_or_else(|| "<unset>".to_owned()),
            config.method.as_deref().unwrap_or("<unset>"),
            config
                .boot_background_image
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<unset>".to_owned()),
            config
                .persist_boot_background_to_loading_screen
                .map(|v| v.to_string())
                .unwrap_or_else(|| "<default:true>".to_owned()),
            config
                .menu_sort_armaments
                .map(MenuSortDefault::label)
                .unwrap_or("<default>"),
            config
                .menu_sort_armor
                .map(MenuSortDefault::label)
                .unwrap_or("<default>"),
            config
                .menu_sort_talismans
                .map(MenuSortDefault::label)
                .unwrap_or("<default>"),
            config
                .preferred_save_picker_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<unset>".to_owned()),
            config
                .autoupdate_preferred_picker_dir
                .map(|v| v.to_string())
                .unwrap_or_else(|| "<default:true>".to_owned())
        )),
        Some(Err(err)) => append_autoload_debug(format_args!("runtime-config: {err}")),
        None => {}
    }
}

pub(crate) fn runtime_config_error() -> Option<String> {
    match RUNTIME_CONFIG.get() {
        Some(Err(err)) => Some(err.clone()),
        None => Some("runtime config was not initialized".to_owned()),
        Some(Ok(_)) => None,
    }
}

pub(crate) fn configured_save_file() -> Option<PathBuf> {
    configured_explicit_save_file()
}

pub(crate) fn configured_explicit_save_file() -> Option<PathBuf> {
    if let Ok(value) = std::env::var(SAVE_FILE_ENV) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    runtime_config().and_then(|config| config.save_file.clone())
}

pub(crate) fn configured_save_file_string() -> Option<String> {
    configured_save_file().map(|path| path.to_string_lossy().into_owned())
}

/// Optional boot background image override from `er-effects.toml`. This is intentionally TOML-only:
/// the production DLL can be configured without shipping a helper script or hard-coding Steam account IDs.
pub(crate) fn configured_boot_background_image() -> Option<PathBuf> {
    runtime_config().and_then(|config| config.boot_background_image.clone())
}

/// Whether the chosen boot background should persist into the game's native loading-screen GFX
/// background. Default-on; users can opt out in `er-effects.toml`.
pub(crate) fn persist_boot_background_to_loading_screen_enabled() -> bool {
    runtime_config()
        .and_then(|config| config.persist_boot_background_to_loading_screen)
        .unwrap_or(true)
}

/// Folder the missing-save picker opens in, from `er-effects.toml` only (no env form on purpose:
/// this is persisted UI state, not a probe gate).
pub(crate) fn configured_preferred_save_picker_dir() -> Option<PathBuf> {
    runtime_config().and_then(|config| config.preferred_save_picker_dir.clone())
}

/// Dir of the most recent validated pick THIS session. `RUNTIME_CONFIG` is parse-once, so
/// same-session reopens would otherwise keep starting at the attach-time value even after
/// `remember_preferred_save_picker_dir` rewrote the file.
static SESSION_PREFERRED_PICKER_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Preferred picker dir as of NOW: the last dir picked this session when there is one, else the
/// attach-time `preferred_save_picker_dir`. UI pickers open here so "remember last opened
/// location" holds within a session, not only across launches.
pub(crate) fn preferred_save_picker_dir_now() -> Option<PathBuf> {
    let session = SESSION_PREFERRED_PICKER_DIR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    session.or_else(configured_preferred_save_picker_dir)
}

/// Whether a validated missing-save pick rewrites `preferred_save_picker_dir` in the user's
/// `er-effects.toml`. Defaults to true when the key is absent.
pub(crate) fn autoupdate_preferred_picker_dir_enabled() -> bool {
    runtime_config()
        .and_then(|config| config.autoupdate_preferred_picker_dir)
        .unwrap_or(true)
}

const PREFERRED_PICKER_DIR_KEY: &str = "preferred_save_picker_dir";
const AUTOUPDATE_PICKER_DIR_KEY: &str = "autoupdate_preferred_picker_dir";

/// Persist the folder of the last validated missing-save pick into the game-directory
/// `er-effects.toml`: update the existing assignment in place, or create the file with commented
/// boilerplate when it does not exist. Skips (with a debug line) when the config failed to load at
/// attach, so a file the user must fix by hand is never clobbered. The in-memory `RuntimeConfig`
/// is intentionally left as loaded -- the new value matters on the NEXT attach.
pub(crate) fn remember_preferred_save_picker_dir(dir: &std::path::Path) {
    let Some(dir_str) = dir.to_str().filter(|dir| !dir.is_empty()) else {
        return;
    };
    *SESSION_PREFERRED_PICKER_DIR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(PathBuf::from(dir));
    let Some(config) = runtime_config() else {
        append_autoload_debug(format_args!(
            "runtime-config: not persisting {PREFERRED_PICKER_DIR_KEY} -- config was unreadable/invalid at attach; fix er-effects.toml first"
        ));
        return;
    };
    let path = config.path.clone();
    let assignment = format!(
        "{PREFERRED_PICKER_DIR_KEY} = {}",
        toml_path_literal(dir_str)
    );
    let new_contents = match std::fs::read_to_string(&path) {
        Ok(contents) => upsert_top_level_key(&contents, PREFERRED_PICKER_DIR_KEY, &assignment),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            boilerplate_config(Some(&assignment))
        }
        Err(err) => {
            append_autoload_debug(format_args!(
                "runtime-config: not persisting {PREFERRED_PICKER_DIR_KEY} -- '{}' unreadable: {err}",
                path.display()
            ));
            return;
        }
    };
    match std::fs::write(&path, new_contents) {
        Ok(()) => append_autoload_debug(format_args!(
            "runtime-config: persisted {PREFERRED_PICKER_DIR_KEY}='{dir_str}' to '{}'",
            path.display()
        )),
        Err(err) => append_autoload_debug(format_args!(
            "runtime-config: failed to persist {PREFERRED_PICKER_DIR_KEY} to '{}': {err}",
            path.display()
        )),
    }
}

/// Replace the top-level `key = ...` line, or insert `assignment` before the first `[section]`
/// header (end of file when none) so the key stays top-level in real TOML.
fn upsert_top_level_key(contents: &str, key: &str, assignment: &str) -> String {
    let mut lines: Vec<String> = contents.lines().map(str::to_owned).collect();
    let existing = lines.iter().position(|line| {
        strip_comment(line)
            .split_once('=')
            .is_some_and(|(k, _)| k.trim() == key)
    });
    match existing {
        Some(idx) => lines[idx] = assignment.to_owned(),
        None => {
            let insert_at = lines
                .iter()
                .position(|line| strip_comment(line).trim().starts_with('['))
                .unwrap_or(lines.len());
            lines.insert(insert_at, assignment.to_owned());
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn boilerplate_config(picker_assignment: Option<&str>) -> String {
    let picker_block = if let Some(assignment) = picker_assignment {
        format!(
            "# Folder the missing-save picker opens in. While {AUTOUPDATE_PICKER_DIR_KEY} is true,\n# it is rewritten to the folder of each successfully picked save.\n{assignment}\n{AUTOUPDATE_PICKER_DIR_KEY} = true"
        )
    } else {
        format!(
            "# Folder the missing-save picker opens in. While {AUTOUPDATE_PICKER_DIR_KEY} is true,\n# it is rewritten to the folder of each successfully picked save.\n# {PREFERRED_PICKER_DIR_KEY} = 'C:\\path\\to\\saves'\n{AUTOUPDATE_PICKER_DIR_KEY} = true"
        )
    };
    format!(
        "\
# er-effects-rs runtime config (auto-created next to the game executable).
# All keys are optional; uncomment and edit as needed.
#
# save_file = 'C:\\path\\to\\ER0000.sl2'  # explicit save to load (skips default-save detection and the picker)
# slot = 0                               # character slot the autoload selects
# method = \"...\"                         # autoload method override
# boot_background_image = 'C:\\path\\to\\background.png'
# persist_boot_background_to_loading_screen = true
# menu_sort.armaments = \"order_of_acquisition\"  # or \"item_type\" / \"preserve\"
# menu_sort.armor = \"order_of_acquisition\"
# menu_sort.talismans = \"order_of_acquisition\"

{picker_block}
"
    )
}

/// Quote a path for the TOML subset we parse: single-quoted literal when possible (keeps Windows
/// backslashes readable), else a basic string with escaped backslashes/quotes.
fn toml_path_literal(path: &str) -> String {
    if !path.contains('\'') {
        format!("'{path}'")
    } else {
        format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

pub(crate) fn configured_autoload_slot() -> Option<i32> {
    if let Ok(value) = std::env::var(SLOT_ENV) {
        if let Ok(slot) = value.trim().parse() {
            return Some(slot);
        }
    }
    runtime_config().and_then(|config| config.slot)
}

pub(crate) fn configured_menu_sort_armaments() -> MenuSortDefault {
    configured_menu_sort_default(
        MENU_SORT_ARMAMENTS_ENV,
        |config| config.menu_sort_armaments,
        "armaments",
    )
}

pub(crate) fn configured_menu_sort_armor() -> MenuSortDefault {
    configured_menu_sort_default(
        MENU_SORT_ARMOR_ENV,
        |config| config.menu_sort_armor,
        "armor",
    )
}

pub(crate) fn configured_menu_sort_talismans() -> MenuSortDefault {
    configured_menu_sort_default(
        MENU_SORT_TALISMANS_ENV,
        |config| config.menu_sort_talismans,
        "talismans",
    )
}

pub(crate) fn configured_save_load_request() -> SaveLoadRequest {
    let mut request = SaveLoadRequest::from_env();
    if std::env::var(SLOT_ENV).is_err()
        && let Some(slot) = runtime_config().and_then(|config| config.slot)
    {
        request.slot = Some(slot);
    }
    if std::env::var(METHOD_ENV).is_err()
        && let Some(method) = runtime_config().and_then(|config| config.method.clone())
    {
        request.method = SaveLoadMethod::from_label(method.trim());
    }
    request
}

fn configured_menu_sort_default(
    env_name: &str,
    from_config: impl FnOnce(&RuntimeConfig) -> Option<MenuSortDefault>,
    label: &str,
) -> MenuSortDefault {
    if let Ok(value) = std::env::var(env_name) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            match parse_menu_sort_default_label(trimmed) {
                Ok(choice) => return choice,
                Err(err) => append_autoload_debug(format_args!(
                    "runtime-config: ignoring invalid {env_name} for menu_sort.{label}: {err}"
                )),
            }
        }
    }
    runtime_config()
        .and_then(from_config)
        .unwrap_or(MenuSortDefault::OrderOfAcquisition)
}

fn runtime_config() -> Option<&'static RuntimeConfig> {
    match RUNTIME_CONFIG.get() {
        Some(Ok(config)) => Some(config),
        _ => None,
    }
}

fn load_runtime_config(hmodule: HINSTANCE) -> Result<RuntimeConfig, String> {
    let dll_path = dll_path(hmodule).map_err(|err| format!("failed to locate DLL path: {err}"))?;
    let Some(dll_dir) = dll_path.parent() else {
        return Err(format!("DLL path has no parent: '{}'", dll_path.display()));
    };
    let path = game_directory_path()
        .unwrap_or_else(|| dll_dir.to_path_buf())
        .join(CONFIG_FILE_NAME);
    let legacy_dll_path = dll_dir.join(CONFIG_FILE_NAME);
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::read_to_string(&legacy_dll_path) {
                Ok(contents) if legacy_dll_path != path => {
                    match std::fs::write(&path, &contents) {
                        Ok(()) => append_autoload_debug(format_args!(
                            "runtime-config: migrated legacy DLL-adjacent config '{}' to game-directory config '{}'",
                            legacy_dll_path.display(),
                            path.display()
                        )),
                        Err(write_err) => append_autoload_debug(format_args!(
                            "runtime-config: loaded legacy DLL-adjacent config '{}' because game-directory config '{}' could not be created: {write_err}",
                            legacy_dll_path.display(),
                            path.display()
                        )),
                    }
                    contents
                }
                Ok(contents) => contents,
                Err(legacy_err) if legacy_err.kind() == std::io::ErrorKind::NotFound => {
                    let contents = boilerplate_config(None);
                    match std::fs::write(&path, &contents) {
                        Ok(()) => append_autoload_debug(format_args!(
                            "runtime-config: auto-created default '{}' next to the game executable",
                            path.display()
                        )),
                        Err(write_err) => {
                            append_autoload_debug(format_args!(
                                "runtime-config: default config '{}' was missing and could not be auto-created: {write_err}; using defaults for this run",
                                path.display()
                            ));
                            return Ok(RuntimeConfig {
                                path,
                                ..RuntimeConfig::default()
                            });
                        }
                    }
                    contents
                }
                Err(legacy_err) => {
                    append_autoload_debug(format_args!(
                        "runtime-config: legacy DLL-adjacent config '{}' was unreadable: {legacy_err}; using game-directory default path '{}'",
                        legacy_dll_path.display(),
                        path.display()
                    ));
                    let contents = boilerplate_config(None);
                    match std::fs::write(&path, &contents) {
                        Ok(()) => contents,
                        Err(write_err) => {
                            append_autoload_debug(format_args!(
                                "runtime-config: default config '{}' was missing and could not be auto-created: {write_err}; using defaults for this run",
                                path.display()
                            ));
                            return Ok(RuntimeConfig {
                                path,
                                ..RuntimeConfig::default()
                            });
                        }
                    }
                }
            }
        }
        Err(err) => {
            return Err(format!("config '{}' is unreadable: {err}", path.display()));
        }
    };
    parse_runtime_config(path, &contents)
}

fn dll_path(hmodule: HINSTANCE) -> Result<PathBuf, String> {
    let mut buf = [0u16; 32768];
    let len = unsafe { GetModuleFileNameW(Some(HMODULE(hmodule.0)), &mut buf) } as usize;
    if len == 0 || len >= buf.len() {
        return Err(format!("GetModuleFileNameW returned {len}"));
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
}

fn parse_runtime_config(path: PathBuf, contents: &str) -> Result<RuntimeConfig, String> {
    let config_dir = path.parent().map(PathBuf::from).unwrap_or_default();
    let mut config = RuntimeConfig {
        path,
        ..RuntimeConfig::default()
    };
    for (line_no, raw_line) in contents.lines().enumerate() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() || (line.starts_with('[') && line.ends_with(']')) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("invalid TOML assignment on line {}", line_no + 1));
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "save_file" | "save.path" | "save_file_path" => {
                let raw = parse_toml_string(value)
                    .map_err(|err| format!("invalid save_file on line {}: {err}", line_no + 1))?;
                config.save_file = Some(configured_path_from_toml(&raw, &config_dir));
            }
            "slot" | "autoload.slot" => {
                config.slot = Some(
                    value
                        .parse::<i32>()
                        .map_err(|err| format!("invalid slot on line {}: {err}", line_no + 1))?,
                );
            }
            "method" | "autoload.method" => {
                config.method = Some(
                    parse_toml_string(value)
                        .map_err(|err| format!("invalid method on line {}: {err}", line_no + 1))?,
                );
            }
            "boot_background_image"
            | "background_image"
            | "boot.background_image"
            | "boot.background"
            | "background.image" => {
                let raw = parse_toml_string(value).map_err(|err| {
                    format!(
                        "invalid boot_background_image on line {}: {err}",
                        line_no + 1
                    )
                })?;
                config.boot_background_image = Some(configured_path_from_toml(&raw, &config_dir));
            }
            "persist_boot_background_to_loading_screen"
            | "boot.persist_background_to_loading_screen"
            | "loading_screen.persist_boot_background"
            | "loading_background.persist_boot_background" => {
                config.persist_boot_background_to_loading_screen =
                    Some(parse_toml_bool(value).map_err(|err| {
                        format!(
                            "invalid persist_boot_background_to_loading_screen on line {}: {err}",
                            line_no + 1
                        )
                    })?);
            }
            "menu_sort.armaments" | "sort.armaments" | "armaments_sort" => {
                config.menu_sort_armaments =
                    Some(parse_menu_sort_default_value(value).map_err(|err| {
                        format!("invalid menu_sort.armaments on line {}: {err}", line_no + 1)
                    })?);
            }
            "menu_sort.armor" | "sort.armor" | "armor_sort" => {
                config.menu_sort_armor =
                    Some(parse_menu_sort_default_value(value).map_err(|err| {
                        format!("invalid menu_sort.armor on line {}: {err}", line_no + 1)
                    })?);
            }
            "menu_sort.talismans" | "sort.talismans" | "talismans_sort" => {
                config.menu_sort_talismans =
                    Some(parse_menu_sort_default_value(value).map_err(|err| {
                        format!("invalid menu_sort.talismans on line {}: {err}", line_no + 1)
                    })?);
            }
            "preferred_save_picker_dir" => {
                let parsed = PathBuf::from(parse_toml_string(value).map_err(|err| {
                    format!(
                        "invalid preferred_save_picker_dir on line {}: {err}",
                        line_no + 1
                    )
                })?);
                config.preferred_save_picker_dir = Some(if parsed.is_absolute() {
                    parsed
                } else {
                    config_dir.join(parsed)
                });
            }
            "autoupdate_preferred_picker_dir" => {
                config.autoupdate_preferred_picker_dir =
                    Some(parse_toml_bool(value).map_err(|err| {
                        format!(
                            "invalid autoupdate_preferred_picker_dir on line {}: {err}",
                            line_no + 1
                        )
                    })?);
            }
            _ => {}
        }
    }
    Ok(config)
}

/// Accepts `true`/`false` case-insensitively (so a hand-written `True` still parses) plus `1`/`0`.
fn parse_toml_bool(value: &str) -> Result<bool, &'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err("expected true or false"),
    }
}

fn parse_menu_sort_default_value(value: &str) -> Result<MenuSortDefault, &'static str> {
    let label = parse_toml_string(value)?;
    parse_menu_sort_default_label(&label)
}

fn configured_path_from_toml(raw: &str, config_dir: &std::path::Path) -> PathBuf {
    if let Some(wine_path) = wine_z_path_from_linux_absolute(raw) {
        return wine_path;
    }
    let parsed = PathBuf::from(raw);
    if parsed.is_absolute() {
        parsed
    } else {
        config_dir.join(parsed)
    }
}

fn wine_z_path_from_linux_absolute(raw: &str) -> Option<PathBuf> {
    if !raw.starts_with('/') || raw.starts_with("//") {
        return None;
    }
    let mut path = String::from("Z:");
    path.push_str(&raw.replace('/', "\\"));
    Some(PathBuf::from(path))
}

fn parse_menu_sort_default_label(value: &str) -> Result<MenuSortDefault, &'static str> {
    let normalized = value.trim().to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "preserve" | "disabled" | "disable" | "off" | "none" | "vanilla" => {
            Ok(MenuSortDefault::Preserve)
        }
        "item_type" | "type" => Ok(MenuSortDefault::ItemType),
        "order_of_acquisition" | "acquisition" | "order_acquisition" | "acquired" => {
            Ok(MenuSortDefault::OrderOfAcquisition)
        }
        _ => Err("expected order_of_acquisition, item_type, or preserve"),
    }
}

fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '#' if !in_string => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn parse_toml_string(value: &str) -> Result<String, &'static str> {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return Ok(value[1..value.len() - 1].to_owned());
    }
    if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
        return Err("expected a quoted TOML string");
    }
    let inner = &value[1..value.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let Some(next) = chars.next() else {
            return Err("trailing escape");
        };
        match next {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            _ => return Err("unsupported escape"),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_absolute_toml_paths_map_to_wine_z_drive() {
        let path = configured_path_from_toml(
            "/home/banon/Pictures/loading screen.png",
            std::path::Path::new("C:\\ignored"),
        );
        assert_eq!(
            path.to_string_lossy(),
            "Z:\\home\\banon\\Pictures\\loading screen.png"
        );
    }

    #[test]
    fn relative_toml_paths_resolve_against_config_dir() {
        let path = configured_path_from_toml(
            "backgrounds/load.png",
            std::path::Path::new("C:\\Games\\ELDEN RING\\Game"),
        );
        assert_eq!(
            path.to_string_lossy(),
            "C:\\Games\\ELDEN RING\\Game/backgrounds/load.png"
        );
    }
}
