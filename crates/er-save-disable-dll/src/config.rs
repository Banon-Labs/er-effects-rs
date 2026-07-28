//! User-facing configuration for where diverted saves land.
//!
//! Generated on first run if absent, in the same line-oriented style as the other
//! standalone DLLs in this repo (`#` comments, `key = value`, no TOML dependency).

use std::{fs, path::PathBuf, sync::OnceLock};

use crate::log_message;

const CONFIG_FILE_NAME: &str = "er-save-disable.toml";

/// Appended to the WHOLE original filename, extension included, so the original
/// extension stays visible and nothing else claims the name:
/// `ER0000.sl2` -> `ER0000.sl2.save-disabled`, `ER0000.sl2.bak` -> `ER0000.sl2.bak.save-disabled`.
pub(crate) const DEFAULT_SUFFIX: &str = ".save-disabled";

const DEFAULT_CONFIG_TOML: &str = r#"# er-save-disable standalone DLL configuration.
#
# This DLL does not block saving. The game still performs every save in full -- it
# serializes, writes, truncates, backs up, and runs all of its own bookkeeping, and the
# write genuinely succeeds. Only the DESTINATION changes. Nothing is faked, so the game
# never behaves as though a save failed.
#
# Reads are never redirected. The game keeps loading your real save file, which is why
# your character still appears in the load menu with the playtime from your last genuine
# save.
#
# DEFAULT (what happens with no setting below): each diverted save is written next to the
# file the game read it from, with ".save-disabled" appended to the whole filename:
#
#     ER0000.sl2      ->  ER0000.sl2.save-disabled
#     ER0000.sl2.bak  ->  ER0000.sl2.bak.save-disabled
#
# Uncomment and set this to send diverted saves somewhere else instead. The directory is
# created if it does not exist. Filenames keep their original name plus the suffix.
# save_directory = "C:/Users/you/Documents/er-disabled-saves"

# Suffix appended to the original filename. Cannot be empty -- an empty suffix with no
# save_directory would point straight back at your real save.
suffix = ".save-disabled"
"#;

#[derive(Clone, Debug)]
pub(crate) struct RuntimeConfig {
    pub(crate) config_path: PathBuf,
    /// `None` = write beside the original file (the documented default).
    pub(crate) save_directory: Option<PathBuf>,
    pub(crate) suffix: String,
    pub(crate) load_error: Option<String>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            config_path: PathBuf::from(CONFIG_FILE_NAME),
            save_directory: None,
            suffix: DEFAULT_SUFFIX.to_owned(),
            load_error: None,
        }
    }
}

static RUNTIME_CONFIG: OnceLock<RuntimeConfig> = OnceLock::new();

pub(crate) fn init_runtime_config() {
    if let Err(error) = ensure_default_config_file() {
        log_message(format_args!(
            "config: could not write default {CONFIG_FILE_NAME}: {error}"
        ));
    }
    let config = load_runtime_config();
    match &config.load_error {
        Some(error) => log_message(format_args!("config: {error}")),
        None => log_message(format_args!(
            "config: loaded {} save_directory={} suffix={}",
            config.config_path.display(),
            config.save_directory.as_ref().map_or_else(
                || "<beside the original>".to_owned(),
                |d| d.display().to_string()
            ),
            config.suffix
        )),
    }
    let _ = RUNTIME_CONFIG.set(config);
}

pub(crate) fn runtime_config() -> &'static RuntimeConfig {
    RUNTIME_CONFIG.get_or_init(load_runtime_config)
}

fn ensure_default_config_file() -> std::io::Result<()> {
    let path = PathBuf::from(CONFIG_FILE_NAME);
    if path.exists() {
        return Ok(());
    }
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, DEFAULT_CONFIG_TOML)?;
    fs::rename(tmp, path)
}

fn load_runtime_config() -> RuntimeConfig {
    let mut config = RuntimeConfig::default();
    let raw = match fs::read_to_string(&config.config_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return config,
        Err(error) => {
            config.load_error = Some(format!(
                "failed to read {}: {error}; using defaults",
                config.config_path.display()
            ));
            return config;
        }
    };
    parse_into(&raw, &mut config);
    config
}

fn parse_into(raw: &str, config: &mut RuntimeConfig) {
    let mut errors = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        let number = index + 1;
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() || (line.starts_with('[') && line.ends_with(']')) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            errors.push(format!("line {number}: expected key = value"));
            continue;
        };
        let value = unquote(value.trim());
        match key.trim() {
            "save_directory" => {
                if value.is_empty() {
                    errors.push(format!("line {number}: save_directory is empty; ignoring"));
                } else {
                    config.save_directory = Some(PathBuf::from(value));
                }
            }
            "suffix" => {
                // An empty suffix combined with the default (beside-the-original)
                // destination would resolve to the real save path and overwrite the
                // user's save -- the one outcome this DLL exists to prevent.
                if value.is_empty() {
                    errors.push(format!(
                        "line {number}: suffix must not be empty; keeping {DEFAULT_SUFFIX}"
                    ));
                } else {
                    config.suffix = value.to_owned();
                }
            }
            other => errors.push(format!("line {number}: unknown key {other}")),
        }
    }
    if !errors.is_empty() {
        config.load_error = Some(errors.join("; "));
    }
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> RuntimeConfig {
        let mut config = RuntimeConfig::default();
        parse_into(raw, &mut config);
        config
    }

    #[test]
    fn the_generated_default_parses_cleanly_and_keeps_the_documented_default() {
        // The shipped file must round-trip through our own parser without complaint,
        // and its commented-out save_directory must stay inert.
        let config = parse(DEFAULT_CONFIG_TOML);
        assert_eq!(config.load_error, None, "generated config must parse clean");
        assert_eq!(
            config.save_directory, None,
            "the documented default is commented out"
        );
        assert_eq!(config.suffix, DEFAULT_SUFFIX);
    }

    #[test]
    fn an_override_directory_is_taken_and_unquoted() {
        let config = parse("save_directory = \"D:/elsewhere\"\nsuffix = \".x\"\n");
        assert_eq!(config.save_directory, Some(PathBuf::from("D:/elsewhere")));
        assert_eq!(config.suffix, ".x");
    }

    #[test]
    fn an_empty_suffix_is_refused_rather_than_pointing_at_the_real_save() {
        let config = parse("suffix = \"\"\n");
        assert_eq!(config.suffix, DEFAULT_SUFFIX);
        assert!(config.load_error.is_some(), "the refusal must be reported");
    }
}
