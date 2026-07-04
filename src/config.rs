use std::{path::PathBuf, sync::OnceLock};

use er_save_loader::{SaveLoadMethod, SaveLoadRequest};
use windows::Win32::{
    Foundation::{HINSTANCE, HMODULE},
    System::LibraryLoader::GetModuleFileNameW,
};

use crate::telemetry::append_autoload_debug;

const CONFIG_FILE_NAME: &str = "er-effects.toml";
const SAVE_FILE_ENV: &str = "ER_EFFECTS_SAVE_FILE";
const SLOT_ENV: &str = "ER_EFFECTS_AUTOLOAD_SLOT";
const METHOD_ENV: &str = "ER_EFFECTS_AUTOLOAD_METHOD";

#[derive(Clone, Debug, Default)]
pub(crate) struct RuntimeConfig {
    pub path: PathBuf,
    pub save_file: Option<PathBuf>,
    pub slot: Option<i32>,
    pub method: Option<String>,
}

static RUNTIME_CONFIG: OnceLock<Result<RuntimeConfig, String>> = OnceLock::new();

pub(crate) fn init_runtime_config(hmodule: HINSTANCE) {
    let _ = RUNTIME_CONFIG.set(load_runtime_config(hmodule));
    match RUNTIME_CONFIG.get() {
        Some(Ok(config)) => append_autoload_debug(format_args!(
            "runtime-config: loaded '{}' save_file={} slot={} method={}",
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
            config.method.as_deref().unwrap_or("<unset>")
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

pub(crate) fn configured_autoload_slot() -> Option<i32> {
    if let Ok(value) = std::env::var(SLOT_ENV) {
        if let Ok(slot) = value.trim().parse() {
            return Some(slot);
        }
    }
    runtime_config().and_then(|config| config.slot)
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

fn runtime_config() -> Option<&'static RuntimeConfig> {
    match RUNTIME_CONFIG.get() {
        Some(Ok(config)) => Some(config),
        _ => None,
    }
}

fn load_runtime_config(hmodule: HINSTANCE) -> Result<RuntimeConfig, String> {
    let dll_path = dll_path(hmodule).map_err(|err| format!("failed to locate DLL path: {err}"))?;
    let Some(dir) = dll_path.parent() else {
        return Err(format!("DLL path has no parent: '{}'", dll_path.display()));
    };
    let path = dir.join(CONFIG_FILE_NAME);
    let contents = std::fs::read_to_string(&path).map_err(|err| {
        format!(
            "required config '{}' is missing or unreadable: {err}",
            path.display()
        )
    })?;
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
                let parsed =
                    PathBuf::from(parse_toml_string(value).map_err(|err| {
                        format!("invalid save_file on line {}: {err}", line_no + 1)
                    })?);
                config.save_file = Some(if parsed.is_absolute() {
                    parsed
                } else {
                    config_dir.join(parsed)
                });
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
            _ => {}
        }
    }
    if config.save_file.is_none() {
        return Err(format!(
            "required config '{}' must contain save_file = \"...\"",
            config.path.display()
        ));
    }
    Ok(config)
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
