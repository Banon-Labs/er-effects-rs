use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use crate::{
    log::Log,
    menu_sort::{MenuSortDefault, MenuSortPreferenceOverrides, MenuSortPreferences},
};

const ENV_ARMAMENTS: &str = "ER_EFFECTS_MENU_SORT_ARMAMENTS";
const ENV_ARMOR: &str = "ER_EFFECTS_MENU_SORT_ARMOR";
const ENV_TALISMANS: &str = "ER_EFFECTS_MENU_SORT_TALISMANS";
const CONFIG_FILE_NAMES: [&str; 2] = ["er-menu-sort.toml", "er-effects.toml"];

static RUNTIME_CONFIG: OnceLock<RuntimeConfig> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeConfig {
    preferences: MenuSortPreferences,
}

impl RuntimeConfig {
    pub(crate) fn install() {
        let config = RuntimeConfigLoader::load().unwrap_or_else(|err| {
            Log::write(format_args!("menu-sort-dll: config load warning: {err}"));
            Self::default()
        });
        let _ = RUNTIME_CONFIG.set(config);
    }

    pub(crate) fn active_preferences() -> MenuSortPreferences {
        RUNTIME_CONFIG
            .get()
            .copied()
            .unwrap_or_default()
            .preferences
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            preferences: MenuSortPreferences::default(),
        }
    }
}

struct RuntimeConfigLoader;

impl RuntimeConfigLoader {
    fn load() -> Result<RuntimeConfig, String> {
        let file_overrides = RuntimeConfigFile::find()
            .map(RuntimeConfigFile::parse)
            .transpose()?
            .unwrap_or_default();
        Ok(RuntimeConfig {
            preferences: EnvOverrides::read(file_overrides).resolve(),
        })
    }
}

struct RuntimeConfigFile {
    path: PathBuf,
    contents: String,
}

impl RuntimeConfigFile {
    fn find() -> Option<Self> {
        let config_dir = game_directory_path()?;
        CONFIG_FILE_NAMES
            .into_iter()
            .map(|name| config_dir.join(name))
            .find(|path| path.exists())
            .and_then(|path| {
                fs::read_to_string(&path)
                    .map(|contents| Self { path, contents })
                    .ok()
            })
    }

    fn parse(self) -> Result<MenuSortPreferenceOverrides, String> {
        parse_runtime_config(&self.contents, &self.path)
    }
}

struct EnvOverrides;

impl EnvOverrides {
    fn read(mut overrides: MenuSortPreferenceOverrides) -> MenuSortPreferenceOverrides {
        overrides.armaments = env_default(ENV_ARMAMENTS, "armaments").or(overrides.armaments);
        overrides.armor = env_default(ENV_ARMOR, "armor").or(overrides.armor);
        overrides.talismans = env_default(ENV_TALISMANS, "talismans").or(overrides.talismans);
        overrides
    }
}

fn env_default(env_name: &str, label: &str) -> Option<MenuSortDefault> {
    let value = std::env::var(env_name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    match MenuSortDefault::parse_label(trimmed) {
        Ok(choice) => Some(choice),
        Err(err) => {
            Log::write(format_args!(
                "menu-sort-dll: ignoring invalid {env_name} for menu_sort.{label}: {err}"
            ));
            None
        }
    }
}

fn parse_runtime_config(
    contents: &str,
    path: &Path,
) -> Result<MenuSortPreferenceOverrides, String> {
    let mut overrides = MenuSortPreferenceOverrides::default();
    for (line_no, raw_line) in contents.lines().enumerate() {
        let line = ConfigLine::new(raw_line);
        let Some((key, value)) = line.assignment() else {
            continue;
        };
        match key {
            ConfigKey::Armaments => {
                overrides.armaments =
                    Some(parse_value(value, path, line_no, "menu_sort.armaments")?);
            }
            ConfigKey::Armor => {
                overrides.armor = Some(parse_value(value, path, line_no, "menu_sort.armor")?);
            }
            ConfigKey::Talismans => {
                overrides.talismans =
                    Some(parse_value(value, path, line_no, "menu_sort.talismans")?);
            }
            ConfigKey::Other => {}
        }
    }
    Ok(overrides)
}

fn parse_value(
    value: &str,
    path: &Path,
    line_no: usize,
    key_name: &str,
) -> Result<MenuSortDefault, String> {
    MenuSortDefault::parse_toml_value(value).map_err(|err| {
        format!(
            "invalid {key_name} in '{}' on line {}: {err}",
            path.display(),
            line_no + 1
        )
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfigKey {
    Armaments,
    Armor,
    Talismans,
    Other,
}

impl ConfigKey {
    fn parse(key: &str) -> Self {
        match key.trim() {
            "menu_sort.armaments" | "sort.armaments" | "armaments_sort" => Self::Armaments,
            "menu_sort.armor"
            | "sort.armor"
            | "armor_sort"
            | "menu_sort.protectors"
            | "sort.protectors"
            | "protectors_sort" => Self::Armor,
            "menu_sort.talismans" | "sort.talismans" | "talismans_sort" => Self::Talismans,
            _ => Self::Other,
        }
    }
}

struct ConfigLine<'a> {
    raw: &'a str,
}

impl<'a> ConfigLine<'a> {
    const fn new(raw: &'a str) -> Self {
        Self { raw }
    }

    fn assignment(self) -> Option<(ConfigKey, &'a str)> {
        let line = self.without_comment().trim();
        if line.is_empty() || line.starts_with('[') {
            return None;
        }
        let (key, value) = line.split_once('=')?;
        Some((ConfigKey::parse(key), value))
    }

    fn without_comment(self) -> &'a str {
        let mut in_string = false;
        let mut escaped = false;
        for (idx, ch) in self.raw.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' if in_string => escaped = true,
                '"' => in_string = !in_string,
                '#' if !in_string => return &self.raw[..idx],
                _ => {}
            }
        }
        self.raw
    }
}

fn game_directory_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_runtime_config_subset() {
        let config = parse_runtime_config(
            r#"
            menu_sort.armaments = "order_of_acquisition"
            menu_sort.protectors = "item_type"
            menu_sort.talismans = "preserve"
            "#,
            Path::new("test.toml"),
        )
        .expect("config parses")
        .resolve();

        assert_eq!(config.armaments, MenuSortDefault::OrderOfAcquisition);
        assert_eq!(config.armor, MenuSortDefault::ItemType);
        assert_eq!(config.talismans, MenuSortDefault::Preserve);
    }
}
