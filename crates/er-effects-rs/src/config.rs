use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock, atomic::Ordering},
};

use er_save_loader::{SaveLoadMethod, SaveLoadRequest};
use er_save_picker::{
    AUTOUPDATE_PICKER_DIR_KEY, OS_NATIVE_SAVE_PICKER_KEY, PREFERRED_PICKER_DIR_KEY,
    SavePickerRuntimeConfig, boilerplate_picker_block, os_native_save_picker_from,
};
use er_telemetry::counters::SAVE_PICKER_SURFACE;
use windows::Win32::{
    Foundation::{HINSTANCE, HMODULE},
    System::LibraryLoader::GetModuleFileNameW,
};

use crate::telemetry::{append_autoload_debug, game_directory_path};

const CONFIG_FILE_NAME: &str = "er-effects.toml";
const METHOD_ENV: &str = "ER_EFFECTS_AUTOLOAD_METHOD";

// REMOVED, NOT DEPRECATED: `ER_EFFECTS_SAVE_FILE` and `ER_EFFECTS_AUTOLOAD_SLOT`.
//
// They were a second way to name the autoload save source, sitting in front of the config
// file. `save_redirect/path_hooks.rs` treated the env form and the file form as one class
// ("an explicit loose save source"), so the two routes could disagree while the debug log
// reported only one of them -- and env state is invisible to anyone reading the config
// afterwards, which makes a run impossible to reconstruct.
//
// The config file is now the only way to name a save source. A per-run override goes in the
// DLL-adjacent sidecar (see `sidecar_config_path`), which is a FILE, so it can be read back.
const SAVE_SUPPRESSION_ENABLED_KEY: &str = "save_suppression_enabled";
#[derive(Clone, Debug, Default)]
pub(crate) struct RuntimeConfig {
    pub path: PathBuf,
    /// The DLL-adjacent per-run overlay, when one was found and parsed. Recorded so the
    /// `runtime-config: loaded` line names it: that line is what a launcher reads back to
    /// prove THIS build loaded ITS config, and a run that cannot name its overlay cannot
    /// prove that.
    pub sidecar: Option<PathBuf>,
    pub save_file: Option<PathBuf>,
    /// Sidecar-only: `true` clears any inherited `save_file`, selecting the active Steam
    /// user's default container. See the `save_file_default` arm in `parse_runtime_config`.
    pub save_file_default: Option<bool>,
    pub slot: Option<i32>,
    pub method: Option<String>,
    pub boot_background_image: Option<PathBuf>,
    pub save_suppression_enabled: Option<bool>,
    pub save_picker: SavePickerRuntimeConfig,
}

static RUNTIME_CONFIG: OnceLock<Result<RuntimeConfig, String>> = OnceLock::new();

pub(crate) fn init_runtime_config(hmodule: HINSTANCE) {
    let _ = RUNTIME_CONFIG.set(load_runtime_config(hmodule));
    // Latched, not read lazily, so the surface is exported even in a session where no picker ever
    // opens -- and so the first debug line of every session states which picker the user is on.
    SAVE_PICKER_SURFACE.store(
        usize::from(os_native_save_picker_from(
            runtime_config().map(|config| &config.save_picker),
        )),
        Ordering::SeqCst,
    );
    match RUNTIME_CONFIG.get() {
        Some(Ok(config)) => append_autoload_debug(format_args!(
            "runtime-config: loaded '{}' sidecar={} save_file={} slot={} method={} boot_background_image={} {SAVE_SUPPRESSION_ENABLED_KEY}={} preferred_save_picker_dir={} autoupdate_preferred_picker_dir={} {OS_NATIVE_SAVE_PICKER_KEY}={}",
            config.path.display(),
            config
                .sidecar
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<none>".to_owned()),
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
                .save_suppression_enabled
                .map(|v| v.to_string())
                .unwrap_or_else(|| "<default:false>".to_owned()),
            config
                .save_picker
                .preferred_save_picker_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<unset>".to_owned()),
            config
                .save_picker
                .autoupdate_preferred_picker_dir
                .map(|v| v.to_string())
                .unwrap_or_else(|| "<default:true>".to_owned()),
            config
                .save_picker
                .os_native_save_picker
                .map(|v| v.to_string())
                .unwrap_or_else(|| "<default:false>".to_owned())
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
    runtime_config().and_then(|config| config.save_file.clone())
}

/// Optional boot background image override from `er-effects.toml`. This is intentionally TOML-only:
/// the production DLL can be configured without shipping a helper script or hard-coding Steam account IDs.
pub(crate) fn configured_boot_background_image() -> Option<PathBuf> {
    runtime_config().and_then(|config| config.boot_background_image.clone())
}

/// Folder the missing-save picker opens in, from `er-effects.toml` only (no env form on purpose:
/// this is persisted UI state, not a probe gate).
pub(crate) fn configured_preferred_save_picker_dir() -> Option<PathBuf> {
    runtime_config().and_then(|config| config.save_picker.preferred_save_picker_dir.clone())
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
        .map(|config| config.save_picker.autoupdate_preferred_picker_dir_enabled())
        .unwrap_or(true)
}

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
    let picker_block = boilerplate_picker_block(picker_assignment);
    format!(
        "\
# er-effects-rs runtime config (auto-created next to the game executable).
# All keys are optional; uncomment and edit as needed.
#
# With no save_file, autoload uses the active Steam user's valid default container:
# %APPDATA%\\EldenRing\\<SteamID64>\\ER0000.sl2 (vanilla) or the configured Seamless container.
# save_file = 'C:\\path\\to\\ER0000.sl2'  # explicit read-only source; staged privately, never written in place
# slot = 0                               # character slot the autoload selects (default: 0 when unset)
# A slot only names a target inside a resolved save; it does not prove that source or character exists.
# The picker opens only when neither source is valid. There is no require_save_picker key yet.
# os_native_save_picker = false          # when a picker opens: false=in-game browser, true=OS file dialog
# method = \"...\"                         # autoload method override
# boot_background_image = 'C:\\path\\to\\background.png'
# save_suppression_enabled = false        # opt-in only: suppresses native saves except the Save Game one-shot bypass
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

pub(crate) fn save_suppression_enabled() -> bool {
    runtime_config()
        .and_then(|config| config.save_suppression_enabled)
        .unwrap_or(false)
}

pub(crate) fn configured_autoload_slot() -> Option<i32> {
    runtime_config().and_then(|config| config.slot)
}

pub(crate) fn configured_save_load_request() -> SaveLoadRequest {
    let mut request = SaveLoadRequest::from_env();
    if let Some(slot) = runtime_config().and_then(|config| config.slot) {
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
                    // Read it, do NOT copy it into the game directory. The migration write
                    // that used to live here turned a per-build config into permanent shared
                    // state: a DLL-adjacent file staged for one run silently became the
                    // game-wide default for every later launch, including the user's own.
                    // Sixteen such stale files exist under target/ and .worktrees/ in this
                    // tree alone -- one of them pins a PR #116 repro save and an OS-picker
                    // flag from 2026-08-01.
                    append_autoload_debug(format_args!(
                        "runtime-config: read legacy DLL-adjacent config '{}' because game-directory config '{}' does not exist; NOT copying it there",
                        legacy_dll_path.display(),
                        path.display()
                    ));
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
    let base = parse_runtime_config(path, &contents)?;
    Ok(apply_sidecar_overlay(base, &dll_path))
}

/// Per-run config overlay: `<dll-file-stem>.toml` beside the loaded module.
///
/// A launcher that stages one build of this DLL for one run needs to say which save that run
/// loads, without writing the game directory -- which is shared, hand-edited, and outlives
/// every run. The module's own path is the natural per-run channel: me3 already decides which
/// DLL to load, so a file next to it is scoped to exactly that build, needs no environment
/// variable, and can be read back afterwards to reconstruct what the run was configured with.
///
/// The name is derived from the DLL (`er_effects_rs.dll` -> `er_effects_rs.toml`) rather than
/// being `er-effects.toml`, so it cannot collide with the legacy DLL-adjacent config above.
/// That matters: stale `er-effects.toml` files are strewn through `target/` and worktree build
/// dirs, and a scheme that gave them new authority would silently arm them.
fn sidecar_config_path(dll_path: &std::path::Path) -> Option<PathBuf> {
    let stem = dll_path.file_stem()?;
    Some(dll_path.with_file_name(stem).with_extension("toml"))
}

/// Overlay `sidecar` onto `base` KEY BY KEY -- not wholesale replacement.
///
/// The game-directory config holds settings that belong to the user rather than to the run:
/// `os_native_save_picker`, `preferred_save_picker_dir`, `boot_background_image`. Replacing it
/// entirely would mean every per-run launch silently dropped them. So only keys the sidecar
/// actually sets take effect, and each override is named in the debug log -- with two files
/// feeding one config, a run that cannot say where a value came from is not diagnosable.
fn apply_sidecar_overlay(mut base: RuntimeConfig, dll_path: &std::path::Path) -> RuntimeConfig {
    let Some(sidecar_path) = sidecar_config_path(dll_path) else {
        append_autoload_debug(format_args!(
            "runtime-config: no sidecar candidate -- module path '{}' has no file stem",
            dll_path.display()
        ));
        return base;
    };
    let contents = match std::fs::read_to_string(&sidecar_path) {
        Ok(contents) => contents,
        // Logged rather than silent. A launcher stages this file and then waits for the DLL to
        // confirm it read it; when that confirmation never comes, the ONLY question worth
        // answering is "which path did the DLL actually look at" -- and the module path under
        // Wine is not something to infer from the Linux side.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            append_autoload_debug(format_args!(
                "runtime-config: no sidecar at '{}' (module '{}') -- game-directory config used alone",
                sidecar_path.display(),
                dll_path.display()
            ));
            return base;
        }
        Err(err) => {
            append_autoload_debug(format_args!(
                "runtime-config: sidecar '{}' is unreadable: {err}; using the game-directory config alone",
                sidecar_path.display()
            ));
            return base;
        }
    };

    let overlay = match parse_runtime_config(sidecar_path.clone(), &contents) {
        Ok(overlay) => overlay,
        Err(err) => {
            // A malformed sidecar must not silently fall back to the game-directory save: the
            // run would load a DIFFERENT character than the launcher reported, which is worse
            // than not running at all. Say so loudly and leave the base config in place; the
            // launcher's identity check is what turns this into a visible failure.
            append_autoload_debug(format_args!(
                "runtime-config: sidecar '{}' is INVALID ({err}); ignoring it -- this run does NOT use the per-run overlay",
                sidecar_path.display()
            ));
            return base;
        }
    };

    let mut overridden: Vec<&str> = Vec::new();
    // Checked before `save_file` so a sidecar carrying both is unambiguous: an explicit
    // request for the default container wins over any inherited or co-listed path.
    if overlay.save_file_default == Some(true) {
        base.save_file = None;
        base.slot = None;
        overridden.push("save_file_default (cleared save_file + slot)");
    } else if overlay.save_file.is_some() {
        base.save_file = overlay.save_file;
        overridden.push("save_file");
    }
    if overlay.slot.is_some() {
        base.slot = overlay.slot;
        overridden.push("slot");
    }
    if overlay.method.is_some() {
        base.method = overlay.method;
        overridden.push("method");
    }
    if overlay.boot_background_image.is_some() {
        base.boot_background_image = overlay.boot_background_image;
        overridden.push("boot_background_image");
    }
    if overlay.save_suppression_enabled.is_some() {
        base.save_suppression_enabled = overlay.save_suppression_enabled;
        overridden.push(SAVE_SUPPRESSION_ENABLED_KEY);
    }
    if overlay.save_picker.preferred_save_picker_dir.is_some() {
        base.save_picker.preferred_save_picker_dir = overlay.save_picker.preferred_save_picker_dir;
        overridden.push(PREFERRED_PICKER_DIR_KEY);
    }
    if overlay
        .save_picker
        .autoupdate_preferred_picker_dir
        .is_some()
    {
        base.save_picker.autoupdate_preferred_picker_dir =
            overlay.save_picker.autoupdate_preferred_picker_dir;
        overridden.push(AUTOUPDATE_PICKER_DIR_KEY);
    }
    if overlay.save_picker.os_native_save_picker.is_some() {
        base.save_picker.os_native_save_picker = overlay.save_picker.os_native_save_picker;
        overridden.push(OS_NATIVE_SAVE_PICKER_KEY);
    }

    append_autoload_debug(format_args!(
        "runtime-config: sidecar '{}' overlaid {} key(s): {}",
        sidecar_path.display(),
        overridden.len(),
        if overridden.is_empty() {
            "<none>".to_owned()
        } else {
            overridden.join(", ")
        }
    ));
    base.sidecar = Some(sidecar_path);
    base
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
            // Overlay-only: an overlay can SET a key but has no way to UNSET one, so a per-run
            // sidecar could never ask for "the active Steam default save" once the game-directory
            // config named a `save_file`. This key says that explicitly. It is meaningless in the
            // base config (leave `save_file` out instead) and is ignored there.
            "save_file_default" => {
                config.save_file_default = Some(parse_toml_bool(value).map_err(|err| {
                    format!("invalid save_file_default on line {}: {err}", line_no + 1)
                })?);
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
            SAVE_SUPPRESSION_ENABLED_KEY => {
                config.save_suppression_enabled = Some(parse_toml_bool(value).map_err(|err| {
                    format!(
                        "invalid {SAVE_SUPPRESSION_ENABLED_KEY} on line {}: {err}",
                        line_no + 1
                    )
                })?);
            }
            "preferred_save_picker_dir" => {
                let raw = parse_toml_string(value).map_err(|err| {
                    format!(
                        "invalid preferred_save_picker_dir on line {}: {err}",
                        line_no + 1
                    )
                })?;
                // The DLL is a Windows target under Wine: PathBuf::from("/home/...") becomes
                // the current drive's `S:\\home\\...`, which does not name the Linux directory.
                // Share the save/background path bridge so a TOML Linux absolute becomes `Z:\\home\\...`.
                config.save_picker.preferred_save_picker_dir =
                    Some(configured_path_from_toml(&raw, &config_dir));
            }
            "autoupdate_preferred_picker_dir" => {
                config.save_picker.autoupdate_preferred_picker_dir =
                    Some(parse_toml_bool(value).map_err(|err| {
                        format!(
                            "invalid autoupdate_preferred_picker_dir on line {}: {err}",
                            line_no + 1
                        )
                    })?);
            }
            // ONE key for BOTH picker surfaces (load source and save destination). Two keys would
            // let the modes drift apart, and nothing about the OS dialog is per-surface.
            "os_native_save_picker" | "use_os_file_picker" | "save_picker.os_native" => {
                config.save_picker.os_native_save_picker =
                    Some(parse_toml_bool(value).map_err(|err| {
                        format!(
                            "invalid {OS_NATIVE_SAVE_PICKER_KEY} on line {}: {err}",
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

    fn parse(contents: &str) -> Result<RuntimeConfig, String> {
        parse_runtime_config(PathBuf::from("C:\\Game\\er-effects.toml"), contents)
    }

    #[test]
    fn linux_absolute_picker_directory_maps_to_wine_z_drive() {
        let config =
            parse("preferred_save_picker_dir = '/home/banon/projects/er-effects-rs/save-files'\n")
                .expect("Linux absolute picker directory must parse");
        assert_eq!(
            config
                .save_picker
                .preferred_save_picker_dir
                .expect("configured picker directory")
                .to_string_lossy(),
            "Z:\\home\\banon\\projects\\er-effects-rs\\save-files"
        );
    }

    #[test]
    fn every_accepted_spelling_of_the_picker_surface_key_parses() {
        for (line, expected) in [
            ("os_native_save_picker = true", true),
            ("os_native_save_picker = false", false),
            ("os_native_save_picker = 1", true),
            ("os_native_save_picker = 0", false),
            ("os_native_save_picker = True", true),
            ("use_os_file_picker = true", true),
            ("save_picker.os_native = true", true),
        ] {
            let config = parse(line).unwrap_or_else(|err| panic!("'{line}' must parse: {err}"));
            assert_eq!(
                config.save_picker.os_native_save_picker,
                Some(expected),
                "'{line}' parsed to the wrong surface"
            );
            assert_eq!(
                os_native_save_picker_from(Some(&config.save_picker)),
                expected,
                "'{line}' resolved to the wrong surface"
            );
        }
    }

    #[test]
    fn an_unparseable_picker_surface_value_names_its_line() {
        let err = parse("# comment\n\nos_native_save_picker = maybe\n")
            .expect_err("a non-boolean value must be rejected");
        assert!(
            err.contains("os_native_save_picker") && err.contains("line 3"),
            "the error must name the key and the line: {err}"
        );
    }

    /// THE DEFAULT MUST NOT MOVE. Both of the ways "the config did not say" can happen -- the key
    /// absent from a config that loaded, and no config at all because it failed to load -- resolve
    /// to the in-game picker, the only surface the build gate covers.
    #[test]
    fn a_config_that_does_not_say_leaves_the_user_on_the_in_game_picker() {
        let config = parse("slot = 0\n").expect("a config without the key must still parse");
        assert_eq!(config.save_picker.os_native_save_picker, None);
        assert!(!os_native_save_picker_from(Some(&config.save_picker)));
        assert!(
            !os_native_save_picker_from(None),
            "a config that failed to load must not move the user to the OS dialog"
        );
    }

    #[test]
    fn save_suppression_config_is_explicit_opt_in() {
        let config = parse("slot = 0\n").expect("a config without the key must still parse");
        assert_eq!(config.save_suppression_enabled, None);
        assert!(
            !config.save_suppression_enabled.unwrap_or(false),
            "save suppression must default off when the key is absent"
        );

        let enabled =
            parse("save_suppression_enabled = true\n").expect("an explicit true value must parse");
        assert_eq!(enabled.save_suppression_enabled, Some(true));

        let disabled = parse("save_suppression_enabled = false\n")
            .expect("an explicit false value must parse");
        assert_eq!(disabled.save_suppression_enabled, Some(false));

        let err = parse("save_suppression_enabled = maybe\n")
            .expect_err("a non-boolean value must be rejected");
        assert!(
            err.contains(SAVE_SUPPRESSION_ENABLED_KEY) && err.contains("line 1"),
            "the error must name the key and the line: {err}"
        );
    }

    #[test]
    fn both_boilerplate_branches_document_the_picker_surface_key() {
        for (label, generated) in [
            ("auto-created", boilerplate_config(None)),
            (
                "picked-dir",
                boilerplate_config(Some("preferred_save_picker_dir = 'C:\\saves'")),
            ),
        ] {
            assert!(
                generated.contains(OS_NATIVE_SAVE_PICKER_KEY),
                "the {label} boilerplate must document {OS_NATIVE_SAVE_PICKER_KEY}"
            );
            assert!(
                generated.contains(SAVE_SUPPRESSION_ENABLED_KEY),
                "the {label} boilerplate must document {SAVE_SUPPRESSION_ENABLED_KEY}"
            );
            assert!(
                generated.contains("active Steam user's valid default container"),
                "the {label} boilerplate must document default-save resolution"
            );
            assert!(
                generated.contains("There is no require_save_picker key yet."),
                "the {label} boilerplate must not imply that a picker can be forced"
            );
            assert_eq!(
                parse(&generated)
                    .expect("generated boilerplate must parse")
                    .save_suppression_enabled,
                None,
                "the {label} boilerplate must leave save suppression unset/default-off"
            );
            // Documented as a COMMENT: writing the key live would opt the user in.
            assert_eq!(
                parse(&generated)
                    .expect("generated boilerplate must parse")
                    .save_picker
                    .os_native_save_picker,
                None,
                "the {label} boilerplate must leave the surface unset"
            );
        }
    }

    #[test]
    fn relative_toml_paths_resolve_against_config_dir() {
        let path = configured_path_from_toml(
            "backgrounds/load.png",
            std::path::Path::new("C:\\Games\\ELDEN RING\\Game"),
        );
        // The join separator is the TARGET's, and this crate only ever builds for Windows -- the
        // old literal `/` expectation was written against a host build and fails on the real
        // target (verified by running the test exe). Separators INSIDE the configured relative
        // path are deliberately left alone; Windows accepts both.
        assert_eq!(
            path.to_string_lossy(),
            format!(
                "C:\\Games\\ELDEN RING\\Game{}backgrounds/load.png",
                std::path::MAIN_SEPARATOR
            )
        );
    }
}
