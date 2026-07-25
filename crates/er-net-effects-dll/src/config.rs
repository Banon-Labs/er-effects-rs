use std::{fs, path::PathBuf, sync::OnceLock};

use crate::log::net_effects_log;

const CONFIG_FILE_NAME: &str = "er-net-effects.toml";
const DEFAULT_CONFIG_TOML: &str = r#"# er-net-effects standalone DLL configuration.
# The DLL is optional; include er_net_effects_dll.dll as its own ME3 native when
# you want keyboard-controlled network-synced SpEffect application.
network_sync = true
# Start with the visible selector overlay shown. Press Alt+Numpad0,
# Alt+0, or Alt+Insert to hide/show it while in-game.
overlay_visible_on_start = true
hotkeys_file = ".er-net-effects-hotkeys.json"
selected_effect_file = ".er-net-effects-setting.txt"
selected_catalog_file = ".er-net-effects-catalog-setting.txt"
enabled_file = ".er-net-effects-enabled.txt"
command_file = "er-net-effects-command.txt"
telemetry_file = "er-net-effects-telemetry.json"
catalog_dir = "er-net-effect-catalogs"
master_catalog_file = "er-net-effect-master-catalog.json"
"#;

#[derive(Clone, Debug)]
pub(crate) struct RuntimeConfig {
    pub(crate) config_path: PathBuf,
    pub(crate) network_sync: bool,
    pub(crate) overlay_visible_on_start: bool,
    pub(crate) hotkeys_file: PathBuf,
    pub(crate) selected_effect_file: PathBuf,
    pub(crate) selected_catalog_file: PathBuf,
    pub(crate) enabled_file: PathBuf,
    pub(crate) command_file: PathBuf,
    pub(crate) telemetry_file: PathBuf,
    pub(crate) catalog_dir: PathBuf,
    pub(crate) master_catalog_file: PathBuf,
    pub(crate) load_error: Option<String>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            config_path: PathBuf::from(CONFIG_FILE_NAME),
            // This standalone DLL is intentionally the network-effects package;
            // users can set `network_sync = false` to preserve local-only behavior.
            network_sync: true,
            // Default visible because the DLL is optional and the selector UI is the primary
            // confirmation that it loaded and is listening for keyboard control.
            overlay_visible_on_start: true,
            hotkeys_file: PathBuf::from(".er-net-effects-hotkeys.json"),
            selected_effect_file: PathBuf::from(".er-net-effects-setting.txt"),
            selected_catalog_file: PathBuf::from(".er-net-effects-catalog-setting.txt"),
            enabled_file: PathBuf::from(".er-net-effects-enabled.txt"),
            command_file: PathBuf::from("er-net-effects-command.txt"),
            telemetry_file: PathBuf::from("er-net-effects-telemetry.json"),
            catalog_dir: PathBuf::from("er-net-effect-catalogs"),
            master_catalog_file: PathBuf::from("er-net-effect-master-catalog.json"),
            load_error: None,
        }
    }
}

static RUNTIME_CONFIG: OnceLock<RuntimeConfig> = OnceLock::new();

pub(crate) fn init_runtime_config() {
    let _ = ensure_default_config_file();
    let config = load_runtime_config();
    if let Some(error) = &config.load_error {
        net_effects_log(format_args!("runtime-config: {error}"));
    } else {
        net_effects_log(format_args!(
            "runtime-config: loaded {} network_sync={} overlay_visible_on_start={} hotkeys={} catalogs={}",
            config.config_path.display(),
            config.network_sync,
            config.overlay_visible_on_start,
            config.hotkeys_file.display(),
            config.catalog_dir.display()
        ));
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

    let mut errors = Vec::new();
    for (line_index, line) in raw.lines().enumerate() {
        let line_number = line_index + 1;
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() || (line.starts_with('[') && line.ends_with(']')) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            errors.push(format!("line {line_number}: expected key = value"));
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "network_sync" => match parse_bool(value) {
                Some(value) => config.network_sync = value,
                None => errors.push(format!("line {line_number}: invalid bool for network_sync")),
            },
            "overlay_visible_on_start" => match parse_bool(value) {
                Some(value) => config.overlay_visible_on_start = value,
                None => errors.push(format!(
                    "line {line_number}: invalid bool for overlay_visible_on_start"
                )),
            },
            "hotkeys_file" => config.hotkeys_file = parse_path(value),
            "selected_effect_file" => config.selected_effect_file = parse_path(value),
            "selected_catalog_file" => config.selected_catalog_file = parse_path(value),
            "enabled_file" => config.enabled_file = parse_path(value),
            "command_file" => config.command_file = parse_path(value),
            "telemetry_file" => config.telemetry_file = parse_path(value),
            "catalog_dir" => config.catalog_dir = parse_path(value),
            "master_catalog_file" => config.master_catalog_file = parse_path(value),
            other => errors.push(format!("line {line_number}: unknown key {other:?}")),
        }
    }
    if !errors.is_empty() {
        config.load_error = Some(format!(
            "{} parse warnings: {}; using recognized values/defaults",
            config.config_path.display(),
            errors.join("; ")
        ));
    }
    config
}

fn parse_bool(raw: &str) -> Option<bool> {
    match unquote(raw).trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" | "enabled" => Some(true),
        "false" | "0" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
}

fn parse_path(raw: &str) -> PathBuf {
    PathBuf::from(unquote(raw))
}

fn unquote(raw: &str) -> String {
    let raw = raw.trim();
    if raw.len() >= 2
        && ((raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('\'') && raw.ends_with('\'')))
    {
        raw[1..raw.len() - 1].to_owned()
    } else {
        raw.to_owned()
    }
}
