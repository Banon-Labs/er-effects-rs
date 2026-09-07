use std::{
    fs,
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use er_hotkey_config::{FileChange, HotFile};

use crate::{duration_filter::PermanentEffects, log::net_effects_log, stacked_config};

const CONFIG_FILE_NAME: &str = "er-net-effects.toml";
/// Everything in the shipped config file except the key section, which lives in
/// [`crate::bindings::SHIPPED_KEY_SECTION`] next to the table it documents -- this module is
/// windows-gated, so a test that the documented key names actually parse could never run here.
const DEFAULT_CONFIG_HEAD: &str = r#"# er-net-effects standalone DLL configuration.
# The DLL is optional; include er_net_effects.dll as its own ME3 native when
# you want keyboard-controlled network-synced SpEffect application.
network_sync = true
# Start with the selector overlay bar shown. Press Alt+Numpad0, Alt+0, or
# Alt+Insert to hide/show it while in-game -- or whatever you set
# `selector_show_hide_key` to at the bottom of this file.
#
# SHOWN IS NOT OPEN. The bar starts MINIMIZED to its [+] button; click that
# button to expand it. The DLL takes the arrow keys away from the game ONLY
# while the bar is expanded AND a character is loaded, so with the bar
# minimized -- or at the title screen -- every key is the game's. The keys that
# move the selector cursor (arrows, numpad +/-) likewise wait for the expanded
# bar; Alt+' and your own hotkeys from the hotkeys file keep firing regardless,
# which is how this DLL is meant to be played.
overlay_visible_on_start = true
hotkeys_file = ".er-net-effects-hotkeys.json"
selected_effect_file = ".er-net-effects-setting.txt"
selected_catalog_file = ".er-net-effects-catalog-setting.txt"
enabled_file = ".er-net-effects-enabled.txt"
command_file = "er-net-effects-command.txt"
telemetry_file = "er-net-effects-telemetry.json"
catalog_dir = "er-net-effect-catalogs"
master_catalog_file = "er-net-effect-master-catalog.json"
# Which effects the selector offers, by duration.
#
#   include  every effect (the default, and the behaviour before this setting existed)
#   exclude  hide effects whose SpEffectParam duration is -1
#   only     offer ONLY those
#
# A -1 duration never expires. That matters with network_sync on: the game broadcasts an effect
# when it is applied but has no message for removing one, so an effect you put on other players
# ends for them only when its duration runs out -- and a -1 never does, leaving it on them until
# they die or reload. 516 of the 842 entries in the shipped visuals-only catalog are -1.
permanent_effects = "include"
# Effects that stay applied no matter where the selector cursor is, so several can run at once.
# The stack-add key adds the highlighted effect to this list, stack-remove takes it back out; the
# DLL rewrites this line as you do, leaving the rest of the file alone.
stacked_effects = []
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
    /// Which effects the selector offers, by duration -- see [`PermanentEffects`].
    pub(crate) permanent_effects: PermanentEffects,
    /// Effects that stay applied regardless of the selector cursor, edited in-game with
    /// the stack keys and written back to this file.
    pub(crate) stacked_effects: Vec<i32>,
    /// Raw `(config key, value)` for every selector binding line the file carries.
    ///
    /// Kept as text rather than parsed here because the parse has to happen where the previous
    /// binding is known: a value that does not parse must keep the key that was working, and this
    /// type has no idea what that was. See [`crate::bindings`].
    pub(crate) key_bindings: Vec<(String, String)>,
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
            // Absent from the file means every effect stays selectable, so an existing config
            // keeps behaving exactly as it did.
            permanent_effects: PermanentEffects::Include,
            stacked_effects: Vec::new(),
            // Empty means "the file mentions no bindings", which leaves every key at its shipped
            // default -- see `bindings::SelectorBindings::default`.
            key_bindings: Vec::new(),
            load_error: None,
        }
    }
}

static RUNTIME_CONFIG: OnceLock<RuntimeConfig> = OnceLock::new();
/// The catalog-shaping settings, RE-read while the game runs.
///
/// [`RUNTIME_CONFIG`] is a `OnceLock` frozen at `DllMain`, which is right for paths and hook
/// wiring -- those cannot change under a running process. But `permanent_effects` and
/// `stacked_effects` describe what the selector should be offering right now, and requiring a
/// relaunch to see an edit take effect makes the file feel dead. These two are therefore kept
/// live, polled by the same signature that already rebuilds the catalogs when their files change.
static LIVE_PERMANENT_EFFECTS: AtomicUsize = AtomicUsize::new(usize::MAX);
static LIVE_STACKED_EFFECTS: Mutex<Vec<i32>> = Mutex::new(Vec::new());

fn encode_permanent(mode: PermanentEffects) -> usize {
    match mode {
        PermanentEffects::Include => 0,
        PermanentEffects::Exclude => 1,
        PermanentEffects::Only => 2,
    }
}

fn decode_permanent(raw: usize) -> PermanentEffects {
    match raw {
        1 => PermanentEffects::Exclude,
        2 => PermanentEffects::Only,
        _ => PermanentEffects::Include,
    }
}

/// The `permanent_effects` in force now, which is the file's value, not `DllMain`'s.
pub(crate) fn live_permanent_effects() -> PermanentEffects {
    match LIVE_PERMANENT_EFFECTS.load(Ordering::Relaxed) {
        usize::MAX => runtime_config().permanent_effects,
        raw => decode_permanent(raw),
    }
}

/// The `stacked_effects` in force now.
pub(crate) fn live_stacked_effects() -> Vec<i32> {
    match LIVE_STACKED_EFFECTS.lock() {
        Ok(guard) if !guard.is_empty() => guard.clone(),
        Ok(_) => {
            // Empty could mean "the file says empty" or "never polled". Only the second is worth
            // falling back for, and re-reading the file is the cheap way to tell them apart.
            if CONFIG_POLLED.load(Ordering::Relaxed) {
                Vec::new()
            } else {
                runtime_config().stacked_effects.clone()
            }
        }
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

static CONFIG_POLLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The last signature published, returned unchanged while the file's text has not moved.
static LIVE_CONFIG_SIGNATURE: Mutex<String> = Mutex::new(String::new());
/// The file watcher. Compares the file's text and reads at most about once a second.
static LIVE_CONFIG_FILE: Mutex<Option<HotFile>> = Mutex::new(None);
/// Reloads that changed at least one binding. A telemetry number rather than a log-only fact:
/// "the config never reloaded" and "the poller never ran" are otherwise the same silence.
static BINDING_RELOADS: AtomicUsize = AtomicUsize::new(0);

/// How many reloads have moved a selector binding.
pub(crate) fn binding_reloads() -> usize {
    BINDING_RELOADS.load(Ordering::Relaxed)
}

/// Signature of the live settings, re-parsing the file only when its text changed.
///
/// # Why text, and why throttled
///
/// This used to `stat` the file every frame and compare the mtime. Two problems, and the second is
/// the one that made key rebinding unreliable:
///
/// * mtime has one-second resolution on several filesystems, including the kind a Wine prefix sits
///   on, so two saves inside one second lose the second one -- which reads as the edit not working.
/// * a `touch`, a re-save with no changes, and this DLL's own write-back of `stacked_effects` all
///   move mtime without changing anything. Every one of those would now report a reload, and a
///   reload RESETS the key edge mask, so a key held at that instant fires again.
///
/// `er_hotkey_config::HotFile` compares the text instead and reads at most about once a second, so
/// the steady-state cost per frame is one integer comparison rather than a syscall.
pub(crate) fn poll_live_config() -> String {
    let unchanged = || {
        LIVE_CONFIG_SIGNATURE.lock().map_or_else(
            |poisoned| poisoned.into_inner().clone(),
            |cached| cached.clone(),
        )
    };

    let change = {
        let mut guard = match LIVE_CONFIG_FILE.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let watcher =
            guard.get_or_insert_with(|| HotFile::new(runtime_config().config_path.clone()));
        watcher.poll()
    };
    let Some(change) = change else {
        return unchanged();
    };
    // A deleted config leaves the settings that were working in force. Re-reading a file that is
    // not there would swap the player's whole configuration for the defaults mid-session, which is
    // a far bigger consequence than the edit they were making.
    if matches!(change, FileChange::Missing) {
        net_effects_log(format_args!(
            "runtime-config: {} disappeared; keeping the settings already in force",
            runtime_config().config_path.display()
        ));
        return unchanged();
    }

    let reparsed = load_runtime_config();
    LIVE_PERMANENT_EFFECTS.store(
        encode_permanent(reparsed.permanent_effects),
        Ordering::Relaxed,
    );
    if let Ok(mut guard) = LIVE_STACKED_EFFECTS.lock() {
        *guard = reparsed.stacked_effects.clone();
    }
    apply_live_key_bindings(&reparsed);
    CONFIG_POLLED.store(true, Ordering::Relaxed);
    let signature = format!(
        "config:{}:{}:{:?}",
        BINDING_RELOADS.load(Ordering::Relaxed),
        reparsed.permanent_effects.as_str(),
        reparsed.stacked_effects
    );
    if let Ok(mut cached) = LIVE_CONFIG_SIGNATURE.lock() {
        *cached = signature.clone();
    }
    signature
}

/// Push the file's binding lines at the live bindings, and clear the key edge state if any moved.
///
/// The edge reset is the half that is easy to leave out and impossible to notice in review. The
/// DirectInput poll remembers which selector keys were down as a positional bitmask; carry it
/// across a rebind and a key held at that instant either swallows its own press or manufactures
/// one. A rejected value deliberately does not count as a move -- otherwise a config with a
/// permanent typo in it would produce one phantom press per reload, forever.
fn apply_live_key_bindings(config: &RuntimeConfig) {
    let update = crate::bindings::refresh_live(|name| {
        config
            .key_bindings
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    });
    for message in &update.messages {
        net_effects_log(format_args!("runtime-config: {message}"));
    }
    if update.moved {
        BINDING_RELOADS.fetch_add(1, Ordering::Relaxed);
        crate::input_suppression::forget_key_edges_after_rebind();
    }
}

pub(crate) fn init_runtime_config() {
    let _ = ensure_default_config_file();
    let config = load_runtime_config();
    if let Some(error) = &config.load_error {
        net_effects_log(format_args!("runtime-config: {error}"));
    } else {
        net_effects_log(format_args!(
            "runtime-config: loaded {} network_sync={} overlay_visible_on_start={} hotkeys={} catalogs={} permanent_effects={} stacked_effects={:?}",
            config.config_path.display(),
            config.network_sync,
            config.overlay_visible_on_start,
            config.hotkeys_file.display(),
            config.catalog_dir.display(),
            config.permanent_effects.as_str(),
            config.stacked_effects
        ));
    }
    let _ = RUNTIME_CONFIG.set(config);
}

pub(crate) fn runtime_config() -> &'static RuntimeConfig {
    RUNTIME_CONFIG.get_or_init(load_runtime_config)
}

/// The whole shipped config file: the settings above plus the key section the bindings module
/// owns.
fn default_config_toml() -> String {
    format!(
        "{DEFAULT_CONFIG_HEAD}{}",
        crate::bindings::SHIPPED_KEY_SECTION
    )
}

fn ensure_default_config_file() -> std::io::Result<()> {
    let path = PathBuf::from(CONFIG_FILE_NAME);
    if path.exists() {
        return Ok(());
    }
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, default_config_toml())?;
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
            "stacked_effects" => config.stacked_effects = stacked_config::parse_id_list(value),
            "permanent_effects" => match PermanentEffects::parse(&unquote(value)) {
                Some(mode) => config.permanent_effects = mode,
                None => errors.push(format!(
                    "line {line_number}: invalid permanent_effects {value:?}; \
                     expected include, exclude or only"
                )),
            },
            // Binding lines are carried through as text. They are parsed by `crate::bindings`,
            // which knows what the previous key was and can therefore keep it when a value does
            // not read.
            other if crate::bindings::is_binding_key(other) => {
                config.key_bindings.push((other.to_owned(), unquote(value)));
            }
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
