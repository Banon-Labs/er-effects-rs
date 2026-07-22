use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use eldenring::cs::{ChrInsExt, PlayerIns};
use er_effects_data::{
    EffectKindSpec, parse_effect_hotkeys_json, parse_effect_id_catalog_json,
    parse_effect_master_catalog_json,
};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, HC_ACTION, KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::{config::runtime_config, crash_telemetry, input_suppression, log::net_effects_log};

const EFFECT_HOTKEY_UP: usize = 1 << 0;
const EFFECT_HOTKEY_DOWN: usize = 1 << 1;
const EFFECT_HOTKEY_LEFT: usize = 1 << 2;
const EFFECT_HOTKEY_RIGHT: usize = 1 << 3;
const EFFECT_HOTKEY_TOGGLE: usize = 1 << 4;
const EFFECT_HOTKEY_SELECTOR_TOGGLE: usize = 1 << 5;

const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;
const VK_INSERT: u32 = 0x2d;
const VK_0: u32 = 0x30;
const VK_NUMPAD0: u32 = 0x60;
const VK_NUMPAD9: u32 = 0x69;
const VK_MULTIPLY: u32 = 0x6a;
const VK_ADD: u32 = 0x6b;
const VK_SUBTRACT: u32 = 0x6d;
const VK_DECIMAL: u32 = 0x6e;
const VK_DIVIDE: u32 = 0x6f;
const VK_OEM_7: u32 = 0xde;
const LLKHF_ALTDOWN: u32 = 0x20;
const DEFAULT_EFFECT_TRIGGER_HOTKEYS_JSON: &str = r#"{
  "hotkeys": [
    {
      "name": "deathblight network test",
      "key": "numpad_multiply",
      "effect_id": 8355,
      "count": 1
    }
  ]
}
"#;
const EFFECT_TRIGGER_COUNT_MAX: u32 = 200;
const EFFECT_SELECTOR_NAME_CHARS: usize = 36;

static EFFECT_SELECTOR_TEXT: OnceLock<Mutex<String>> = OnceLock::new();
static EFFECT_TRIGGER_PENDING_KEYS: OnceLock<Mutex<Vec<EffectTriggerKeyPress>>> = OnceLock::new();
static EFFECT_HOTKEY_HOOK_STARTED: AtomicBool = AtomicBool::new(false);
static EFFECT_HOTKEY_HOOK_ACTIVE: AtomicBool = AtomicBool::new(false);
static EFFECT_SELECTOR_VISIBLE_FOR_HOOK: AtomicBool = AtomicBool::new(false);
static EFFECT_RUNTIME_READY_FOR_HOOK: AtomicBool = AtomicBool::new(false);
static EFFECT_HOTKEY_HOOK_HITS: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_APPLIED_ACTIONS: AtomicUsize = AtomicUsize::new(0);
static EFFECT_INPUT_SUPPRESSED_KEYS: AtomicUsize = AtomicUsize::new(0);
static EFFECT_INPUT_SUPPRESSED_ARROW_KEYS: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_PENDING_UP: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_PENDING_DOWN: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_PENDING_LEFT: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_PENDING_RIGHT: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_PENDING_TOGGLE: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_PENDING_SELECTOR_TOGGLE: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectCallKind {
    SpEffect { id: i32 },
}

impl EffectCallKind {
    fn label(self) -> String {
        match self {
            Self::SpEffect { id } => format!("SpEffect {id}"),
        }
    }

    fn apply(self, player: &mut PlayerIns, network_sync: bool) {
        match self {
            Self::SpEffect { id } => {
                let dont_sync = !network_sync;
                player.apply_speffect(id, dont_sync);
            }
        }
    }

    fn remove(self, player: &mut PlayerIns) {
        match self {
            Self::SpEffect { id } => player.chr_ins.remove_speffect(id),
        }
    }

    fn is_active(self, player: &PlayerIns) -> bool {
        match self {
            Self::SpEffect { id } => player
                .chr_ins
                .special_effect
                .entries()
                .any(|entry| entry.param_id == id),
        }
    }
}

pub(crate) struct NamedEffectCall {
    pub(crate) name: String,
    pub(crate) kind: EffectCallKind,
    pub(crate) enabled: bool,
    remove_requested: bool,
    pub(crate) active: bool,
    active_seen_since_enable: bool,
    apply_failed: bool,
}

impl NamedEffectCall {
    fn new(name: String, kind: EffectCallKind, enabled: bool) -> Self {
        Self {
            name,
            kind,
            enabled,
            remove_requested: false,
            active: false,
            active_seen_since_enable: false,
            apply_failed: false,
        }
    }
}

#[derive(Clone)]
pub(crate) struct EffectCatalog {
    source_key: String,
    pub(crate) file_name: String,
    pub(crate) name: String,
    pub(crate) call_indices: Vec<usize>,
    pub(crate) invalid_ids: usize,
}

#[derive(Clone, Copy)]
struct EffectTriggerKeyPress {
    vk: u32,
    alt: bool,
}

#[derive(Clone, Copy)]
struct EffectTriggerKey {
    vk: u32,
    alt: bool,
}

#[derive(Clone)]
pub(crate) struct EffectTriggerHotkey {
    pub(crate) name: String,
    pub(crate) key_name: String,
    key: EffectTriggerKey,
    effect_id: i32,
    count: u32,
}

pub(crate) struct NetEffectsState {
    pub(crate) calls: Vec<NamedEffectCall>,
    pub(crate) catalogs: Vec<EffectCatalog>,
    pub(crate) load_error: Option<String>,
    pub(crate) network_sync: bool,
    pub(crate) selected_effect_index: Option<usize>,
    pub(crate) selected_catalog_index: Option<usize>,
    effect_catalogs_signature: String,
    pub(crate) effect_catalog_live_updates: u64,
    pub(crate) effect_hotkeys_effects_on: bool,
    pub(crate) effect_selector_visible: bool,
    pub(crate) effect_trigger_hotkeys: Vec<EffectTriggerHotkey>,
    effect_trigger_hotkeys_modified: Option<SystemTime>,
    pub(crate) effect_trigger_hotkeys_load_error: Option<String>,
    pending_effect_triggers: Vec<EffectTriggerHotkey>,
    pub(crate) effect_trigger_fire_count: u64,
    pub(crate) effect_trigger_last_key: Option<String>,
    pub(crate) effect_trigger_last_id: Option<i32>,
    pub(crate) effect_trigger_last_count: u32,
    pub(crate) effect_setting_last_id: Option<i32>,
    effect_setting_last_modified: Option<SystemTime>,
    pub(crate) effect_setting_live_updates: u64,
    pub(crate) effect_reapply_count: u64,
    pub(crate) effect_reapply_last_index: Option<usize>,
    pub(crate) last_telemetry_write: Option<Instant>,
    pub(crate) last_driver_command: Option<String>,
    pub(crate) game_task_ticks: u64,
    pub(crate) runtime_ready: bool,
}

impl NetEffectsState {
    pub(crate) fn new() -> Self {
        let effect_catalogs_signature = current_effect_catalog_signature();
        let (mut calls, catalogs, load_error) = build_effect_catalog_state();
        let (effect_trigger_hotkeys, effect_trigger_hotkeys_load_error) =
            match load_effect_trigger_hotkeys() {
                Ok(hotkeys) => (hotkeys, None),
                Err(error) => (Vec::new(), Some(error)),
            };
        let selected_effect_id = restore_selected_effect_id();
        let selected_effect_index =
            selected_effect_id.and_then(|id| find_call_index_by_id(&calls, id));
        let restored_catalog_key = restore_selected_catalog_key();
        let selected_catalog_index = restored_catalog_key
            .as_deref()
            .and_then(|key| {
                catalogs
                    .iter()
                    .position(|catalog| catalog.source_key == key)
            })
            .or_else(|| {
                selected_effect_index.and_then(|selected| {
                    catalogs.iter().position(|catalog| {
                        catalog
                            .call_indices
                            .iter()
                            .any(|call_index| *call_index == selected)
                    })
                })
            })
            .or_else(|| (!catalogs.is_empty()).then_some(0));
        let effect_hotkeys_effects_on =
            restore_effects_enabled() && selected_effect_index.is_some();
        if effect_hotkeys_effects_on
            && let Some(index) = selected_effect_index
            && let Some(call) = calls.get_mut(index)
        {
            call.enabled = true;
            call.remove_requested = false;
            call.active_seen_since_enable = false;
            call.apply_failed = false;
        }

        Self {
            calls,
            catalogs,
            load_error,
            network_sync: runtime_config().network_sync,
            selected_effect_index,
            selected_catalog_index,
            effect_catalogs_signature,
            effect_catalog_live_updates: 0,
            effect_hotkeys_effects_on,
            effect_selector_visible: runtime_config().overlay_visible_on_start,
            effect_trigger_hotkeys,
            effect_trigger_hotkeys_modified: current_effect_trigger_hotkeys_modified(),
            effect_trigger_hotkeys_load_error,
            pending_effect_triggers: Vec::new(),
            effect_trigger_fire_count: 0,
            effect_trigger_last_key: None,
            effect_trigger_last_id: None,
            effect_trigger_last_count: 0,
            effect_setting_last_id: selected_effect_id,
            effect_setting_last_modified: current_effect_setting_modified(),
            effect_setting_live_updates: 0,
            effect_reapply_count: 0,
            effect_reapply_last_index: None,
            last_telemetry_write: None,
            last_driver_command: None,
            game_task_ticks: 0,
            runtime_ready: false,
        }
    }
}

fn effect_setting_path() -> PathBuf {
    runtime_config().selected_effect_file.clone()
}

fn effect_catalog_setting_path() -> PathBuf {
    runtime_config().selected_catalog_file.clone()
}

fn effect_enabled_setting_path() -> PathBuf {
    runtime_config().enabled_file.clone()
}

fn effect_trigger_hotkeys_path() -> PathBuf {
    runtime_config().hotkeys_file.clone()
}

fn user_effect_catalog_dir() -> PathBuf {
    runtime_config().catalog_dir.clone()
}

fn effect_master_catalog_path() -> PathBuf {
    runtime_config().master_catalog_file.clone()
}

fn command_path() -> PathBuf {
    runtime_config().command_file.clone()
}

fn effect_file_signature(path: &Path) -> String {
    let Ok(metadata) = fs::metadata(path) else {
        return "missing".to_owned();
    };
    let len = metadata.len();
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{len}:{modified}")
}

fn current_effect_catalog_signature() -> String {
    let mut parts = Vec::new();
    parts.push(format!(
        "master:{}",
        effect_file_signature(&effect_master_catalog_path())
    ));
    if let Ok(entries) = fs::read_dir(user_effect_catalog_dir()) {
        let mut catalog_parts = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
            .filter_map(|path| {
                let file_name = path.file_name()?.to_str()?.to_owned();
                Some(format!("{file_name}:{}", effect_file_signature(&path)))
            })
            .collect::<Vec<_>>();
        catalog_parts.sort();
        parts.extend(catalog_parts);
    } else {
        parts.push("catalogs:missing".to_owned());
    }
    parts.join("|")
}

fn current_effect_setting_modified() -> Option<SystemTime> {
    fs::metadata(effect_setting_path())
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn restore_selected_effect_id() -> Option<i32> {
    fs::read_to_string(effect_setting_path())
        .ok()?
        .trim()
        .parse::<i32>()
        .ok()
}

fn restore_selected_catalog_key() -> Option<String> {
    let value = fs::read_to_string(effect_catalog_setting_path()).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn restore_effects_enabled() -> bool {
    let Ok(value) = fs::read_to_string(effect_enabled_setting_path()) else {
        return false;
    };
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "on" | "true" | "1" | "enabled"
    )
}

fn persist_effects_enabled(enabled: bool) {
    let path = effect_enabled_setting_path();
    let tmp_path = path.with_extension("txt.tmp");
    let value = if enabled { "on\n" } else { "off\n" };
    if fs::write(&tmp_path, value).is_ok() {
        let _ = fs::rename(tmp_path, path);
    }
}

fn persist_selected_effect(call: &NamedEffectCall) {
    let EffectCallKind::SpEffect { id } = call.kind;
    let path = effect_setting_path();
    let tmp_path = path.with_extension("txt.tmp");
    if fs::write(&tmp_path, format!("{id}\n")).is_ok() {
        let _ = fs::rename(tmp_path, path);
    }
}

fn persist_selected_catalog(catalog: &EffectCatalog) {
    let path = effect_catalog_setting_path();
    let tmp_path = path.with_extension("txt.tmp");
    if fs::write(&tmp_path, format!("{}\n", catalog.source_key)).is_ok() {
        let _ = fs::rename(tmp_path, path);
    }
}

fn catalog_display_name(file_name: &str) -> String {
    Path::new(file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(file_name)
        .replace(['-', '_'], " ")
}

fn ensure_default_effect_trigger_hotkeys_file() {
    let path = effect_trigger_hotkeys_path();
    if path.exists() {
        return;
    }
    let tmp_path = path.with_extension("json.tmp");
    if fs::write(&tmp_path, DEFAULT_EFFECT_TRIGGER_HOTKEYS_JSON).is_ok() {
        let _ = fs::rename(tmp_path, path);
    }
}

fn current_effect_trigger_hotkeys_modified() -> Option<SystemTime> {
    fs::metadata(effect_trigger_hotkeys_path())
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn load_effect_trigger_hotkeys() -> Result<Vec<EffectTriggerHotkey>, String> {
    ensure_default_effect_trigger_hotkeys_file();
    let path = effect_trigger_hotkeys_path();
    let raw = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let parsed = parse_effect_hotkeys_json(&raw)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    parsed
        .hotkeys
        .into_iter()
        .enumerate()
        .map(|(index, spec)| {
            let key = parse_effect_trigger_key(&spec.key)
                .map_err(|error| format!("hotkeys[{index}] {}: {error}", spec.key))?;
            let count = spec.count.clamp(1, EFFECT_TRIGGER_COUNT_MAX);
            let name = spec
                .name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| format!("{} x{}", spec.effect_id, count));
            Ok(EffectTriggerHotkey {
                name,
                key_name: spec.key,
                key,
                effect_id: spec.effect_id,
                count,
            })
        })
        .collect()
}

fn parse_effect_trigger_key(raw: &str) -> Result<EffectTriggerKey, String> {
    let mut alt = false;
    let mut key_name = raw.trim().to_ascii_lowercase();
    loop {
        let Some((prefix, rest)) = key_name.split_once('+') else {
            break;
        };
        match prefix.trim() {
            "alt" => alt = true,
            "" => {}
            modifier => {
                return Err(format!(
                    "unsupported modifier {modifier:?}; only alt+ is supported"
                ));
            }
        }
        key_name = rest.trim().to_owned();
    }
    let vk = match key_name.as_str() {
        "numpad_multiply" | "numpad_*" | "num_*" | "*" => VK_MULTIPLY,
        "numpad_add" | "numpad_plus" | "numpad_+" | "+" => VK_ADD,
        "numpad_subtract" | "numpad_minus" | "numpad_-" | "-" => VK_SUBTRACT,
        "numpad_decimal" | "numpad_." => VK_DECIMAL,
        "numpad_divide" | "numpad_/" | "/" => VK_DIVIDE,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        "semicolon" | "quote" => VK_OEM_7,
        other if other.starts_with("numpad") && other.len() == "numpad0".len() => {
            let digit = other
                .chars()
                .last()
                .and_then(|c| c.to_digit(10))
                .ok_or_else(|| format!("unknown key {raw:?}"))?;
            VK_NUMPAD0 + digit
        }
        other => return Err(format!("unknown key {other:?}")),
    };
    Ok(EffectTriggerKey { vk, alt })
}

fn call_kind_from_spec(kind: EffectKindSpec, id: i32) -> EffectCallKind {
    match kind {
        EffectKindSpec::SpEffect => EffectCallKind::SpEffect { id },
    }
}

struct RawEffectCatalog {
    source_key: String,
    file_name: String,
    ids: Vec<i32>,
}

fn build_effect_catalog_state() -> (Vec<NamedEffectCall>, Vec<EffectCatalog>, Option<String>) {
    let mut load_errors = Vec::new();
    let master_names = match fs::read_to_string(effect_master_catalog_path()) {
        Ok(json) => match parse_effect_master_catalog_json(&json) {
            Ok(master) => Some(
                master
                    .effects
                    .into_iter()
                    .map(|effect| {
                        let name = if effect.name.is_empty() {
                            format!("SpEffect {}", effect.id)
                        } else {
                            effect.name
                        };
                        (effect.id, name)
                    })
                    .collect::<HashMap<_, _>>(),
            ),
            Err(error) => {
                load_errors.push(format!(
                    "{}: {error}",
                    effect_master_catalog_path().display()
                ));
                None
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            load_errors.push(format!(
                "{}: {error}",
                effect_master_catalog_path().display()
            ));
            None
        }
    };

    let mut raw_catalogs = Vec::new();
    if let Ok(entries) = fs::read_dir(user_effect_catalog_dir()) {
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            match fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|json| {
                    parse_effect_id_catalog_json(&json).map_err(|error| error.to_string())
                }) {
                Ok(ids) => raw_catalogs.push(RawEffectCatalog {
                    source_key: file_name.to_owned(),
                    file_name: file_name.to_owned(),
                    ids,
                }),
                Err(error) => load_errors.push(format!("{}: {error}", path.display())),
            }
        }
    }

    let mut calls = Vec::new();
    let mut call_index_by_id = HashMap::<i32, usize>::new();
    let mut catalogs = Vec::new();
    for raw in raw_catalogs {
        let mut seen = HashSet::new();
        let mut call_indices = Vec::new();
        let mut invalid_ids = 0usize;
        for id in raw.ids {
            if !seen.insert(id) {
                continue;
            }
            let name = if let Some(names) = master_names.as_ref() {
                let Some(name) = names.get(&id).cloned() else {
                    invalid_ids = invalid_ids.saturating_add(1);
                    continue;
                };
                name
            } else {
                format!("SpEffect {id}")
            };
            let index = *call_index_by_id.entry(id).or_insert_with(|| {
                let index = calls.len();
                calls.push(NamedEffectCall::new(
                    name,
                    call_kind_from_spec(EffectKindSpec::SpEffect, id),
                    false,
                ));
                index
            });
            call_indices.push(index);
        }
        if !call_indices.is_empty() {
            catalogs.push(EffectCatalog {
                source_key: raw.source_key,
                name: catalog_display_name(&raw.file_name),
                file_name: raw.file_name,
                call_indices,
                invalid_ids,
            });
        }
    }

    let load_error = (!load_errors.is_empty())
        .then(|| format!("effect catalog load errors: {}", load_errors.join("; ")));
    (calls, catalogs, load_error)
}

fn effect_hotkey_action_for_key(vk: u32, alt_down: bool) -> usize {
    match vk {
        VK_LEFT => EFFECT_HOTKEY_LEFT,
        VK_UP => EFFECT_HOTKEY_UP,
        VK_RIGHT => EFFECT_HOTKEY_RIGHT,
        VK_DOWN => EFFECT_HOTKEY_DOWN,
        VK_OEM_7 if alt_down => EFFECT_HOTKEY_TOGGLE,
        VK_0 | VK_NUMPAD0 | VK_INSERT if alt_down => EFFECT_HOTKEY_SELECTOR_TOGGLE,
        _ => 0,
    }
}

fn queue_effect_trigger_key(vk: u32, alt: bool) {
    if let Ok(mut pending) = EFFECT_TRIGGER_PENDING_KEYS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        && pending.len() < 64
    {
        pending.push(EffectTriggerKeyPress { vk, alt });
    }
}

fn drain_effect_trigger_keys() -> Vec<EffectTriggerKeyPress> {
    EFFECT_TRIGGER_PENDING_KEYS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|mut pending| std::mem::take(&mut *pending))
        .unwrap_or_default()
}

pub(crate) fn discard_pending_effect_trigger_keys() -> usize {
    drain_effect_trigger_keys().len()
}

unsafe extern "system" fn effect_hotkey_ll_keyboard_proc(
    ncode: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if ncode == HC_ACTION as i32 && lparam.0 != 0 {
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let msg = wparam.0 as u32;
        let key_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let key_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
        let runtime_ready = EFFECT_RUNTIME_READY_FOR_HOOK.load(Ordering::SeqCst);
        let suppress_arrow = runtime_ready
            && (key_down || key_up)
            && is_arrow_key(kb.vkCode)
            && EFFECT_SELECTOR_VISIBLE_FOR_HOOK.load(Ordering::SeqCst);
        if key_down && runtime_ready {
            let alt_down = msg == WM_SYSKEYDOWN || (kb.flags.0 & LLKHF_ALTDOWN) != 0;
            queue_effect_trigger_key(kb.vkCode, alt_down);
            let action = effect_hotkey_action_for_key(kb.vkCode, alt_down);
            if action != 0 {
                EFFECT_HOTKEY_HOOK_HITS.fetch_add(1, Ordering::SeqCst);
                if action & EFFECT_HOTKEY_UP != 0 {
                    EFFECT_HOTKEY_PENDING_UP.fetch_add(1, Ordering::SeqCst);
                }
                if action & EFFECT_HOTKEY_DOWN != 0 {
                    EFFECT_HOTKEY_PENDING_DOWN.fetch_add(1, Ordering::SeqCst);
                }
                if action & EFFECT_HOTKEY_LEFT != 0 {
                    EFFECT_HOTKEY_PENDING_LEFT.fetch_add(1, Ordering::SeqCst);
                }
                if action & EFFECT_HOTKEY_RIGHT != 0 {
                    EFFECT_HOTKEY_PENDING_RIGHT.fetch_add(1, Ordering::SeqCst);
                }
                if action & EFFECT_HOTKEY_TOGGLE != 0 {
                    EFFECT_HOTKEY_PENDING_TOGGLE.fetch_add(1, Ordering::SeqCst);
                }
                if action & EFFECT_HOTKEY_SELECTOR_TOGGLE != 0 {
                    EFFECT_HOTKEY_PENDING_SELECTOR_TOGGLE.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
        if suppress_arrow {
            record_suppressed_arrow_keys(1);
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, ncode, wparam, lparam) }
}

pub(crate) fn ensure_effect_hotkey_hook() {
    if EFFECT_HOTKEY_HOOK_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("er-net-effects-hotkeys".to_owned())
        .spawn(|| {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::{
                DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
                PeekMessageW, QS_ALLINPUT, SetWindowsHookExW, TranslateMessage,
                UnhookWindowsHookEx, WH_KEYBOARD_LL,
            };

            let Ok(hook) = (unsafe {
                SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(effect_hotkey_ll_keyboard_proc),
                    None,
                    0,
                )
            }) else {
                EFFECT_HOTKEY_HOOK_STARTED.store(false, Ordering::SeqCst);
                net_effects_log(format_args!("effect-hotkeys: hook install failed"));
                return;
            };
            EFFECT_HOTKEY_HOOK_ACTIVE.store(true, Ordering::SeqCst);
            net_effects_log(format_args!("effect-hotkeys: hook installed"));
            let mut msg = MSG::default();
            loop {
                let _ = unsafe {
                    MsgWaitForMultipleObjectsEx(None, 50, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
                };
                while unsafe {
                    PeekMessageW(&mut msg, Some(HWND(std::ptr::null_mut())), 0, 0, PM_REMOVE)
                }
                .as_bool()
                {
                    unsafe {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            }
            #[allow(unreachable_code)]
            unsafe {
                let _ = UnhookWindowsHookEx(hook);
                EFFECT_HOTKEY_HOOK_ACTIVE.store(false, Ordering::SeqCst);
                EFFECT_HOTKEY_HOOK_STARTED.store(false, Ordering::SeqCst);
            }
        });
    if spawned.is_err() {
        EFFECT_HOTKEY_HOOK_STARTED.store(false, Ordering::SeqCst);
    }
}

pub(crate) fn effect_hotkey_hook_active() -> bool {
    EFFECT_HOTKEY_HOOK_ACTIVE.load(Ordering::SeqCst)
}

pub(crate) fn effect_hotkey_hook_hits() -> usize {
    EFFECT_HOTKEY_HOOK_HITS.load(Ordering::SeqCst)
}

pub(crate) fn effect_hotkey_applied_actions() -> usize {
    EFFECT_HOTKEY_APPLIED_ACTIONS.load(Ordering::SeqCst)
}

pub(crate) fn effect_input_suppressed_keys() -> usize {
    EFFECT_INPUT_SUPPRESSED_KEYS.load(Ordering::SeqCst)
}

pub(crate) fn effect_input_suppressed_arrow_keys() -> usize {
    EFFECT_INPUT_SUPPRESSED_ARROW_KEYS.load(Ordering::SeqCst)
}

pub(crate) fn record_suppressed_arrow_keys(count: usize) {
    EFFECT_INPUT_SUPPRESSED_KEYS.fetch_add(count, Ordering::SeqCst);
    EFFECT_INPUT_SUPPRESSED_ARROW_KEYS.fetch_add(count, Ordering::SeqCst);
}

pub(crate) fn effect_runtime_ready() -> bool {
    EFFECT_RUNTIME_READY_FOR_HOOK.load(Ordering::SeqCst)
}

fn clear_effect_selector_text() {
    if let Ok(mut slot) = EFFECT_SELECTOR_TEXT
        .get_or_init(|| Mutex::new(String::new()))
        .lock()
    {
        slot.clear();
    }
}

fn discard_pending_effect_selector_inputs() -> usize {
    EFFECT_HOTKEY_PENDING_UP.swap(0, Ordering::SeqCst)
        + EFFECT_HOTKEY_PENDING_DOWN.swap(0, Ordering::SeqCst)
        + EFFECT_HOTKEY_PENDING_LEFT.swap(0, Ordering::SeqCst)
        + EFFECT_HOTKEY_PENDING_RIGHT.swap(0, Ordering::SeqCst)
        + EFFECT_HOTKEY_PENDING_TOGGLE.swap(0, Ordering::SeqCst)
        + EFFECT_HOTKEY_PENDING_SELECTOR_TOGGLE.swap(0, Ordering::SeqCst)
        + discard_pending_effect_trigger_keys()
}

pub(crate) fn set_runtime_ready(state: &mut NetEffectsState, ready: bool) {
    if state.runtime_ready == ready {
        return;
    }
    state.runtime_ready = ready;
    EFFECT_RUNTIME_READY_FOR_HOOK.store(ready, Ordering::SeqCst);
    crash_telemetry::runtime_ready(ready);
    if ready {
        sync_effect_selector_input_suppression(state.effect_selector_visible);
        state.last_driver_command = Some("runtime-gate: character/map ready".to_owned());
        net_effects_log(format_args!(
            "runtime-gate: character/map ready; enabling net-effects processing"
        ));
    } else {
        let discarded = discard_pending_effect_selector_inputs();
        sync_effect_selector_input_suppression(false);
        clear_effect_selector_text();
        state.last_driver_command = Some(format!(
            "runtime-gate: character/map absent; suspended net-effects processing; discarded={discarded}"
        ));
        net_effects_log(format_args!(
            "runtime-gate: character/map absent; suspended net-effects processing; discarded={discarded}"
        ));
    }
}

pub(crate) fn effect_selector_text() -> String {
    EFFECT_SELECTOR_TEXT
        .get_or_init(|| Mutex::new(String::new()))
        .lock()
        .map(|text| text.clone())
        .unwrap_or_default()
}

fn truncated_effect_name(call: &NamedEffectCall) -> Option<String> {
    let EffectCallKind::SpEffect { id } = call.kind;
    let fallback = format!("SpEffect {id}");
    let name = call.name.trim();
    if name.is_empty() || name == fallback {
        return None;
    }
    let mut out = String::new();
    let mut truncated = false;
    for (index, ch) in name.chars().enumerate() {
        if index >= EFFECT_SELECTOR_NAME_CHARS {
            truncated = true;
            break;
        }
        out.push(ch);
    }
    if truncated {
        out.push('>');
    }
    Some(out)
}

fn sync_effect_selector_input_suppression(visible: bool) {
    EFFECT_SELECTOR_VISIBLE_FOR_HOOK.store(visible, Ordering::SeqCst);
    input_suppression::set_arrow_key_suppression(visible);
}

pub(crate) fn publish_effect_selector_text(state: &mut NetEffectsState) {
    if !state.runtime_ready {
        sync_effect_selector_input_suppression(false);
        clear_effect_selector_text();
        return;
    }
    let selector_toggles = EFFECT_HOTKEY_PENDING_SELECTOR_TOGGLE.swap(0, Ordering::SeqCst);
    if selector_toggles != 0 {
        if selector_toggles % 2 == 1 {
            state.effect_selector_visible = !state.effect_selector_visible;
        }
        state.last_driver_command = Some(format!(
            "effect-selector: {}",
            if state.effect_selector_visible {
                "shown"
            } else {
                "hidden"
            }
        ));
        EFFECT_HOTKEY_APPLIED_ACTIONS.fetch_add(selector_toggles, Ordering::SeqCst);
    }
    sync_effect_selector_input_suppression(state.effect_selector_visible);
    if !state.effect_selector_visible {
        clear_effect_selector_text();
        return;
    }
    let catalog = state
        .selected_catalog_index
        .and_then(|index| state.catalogs.get(index));
    let catalog_count = state.catalogs.len();
    let catalog_index = state.selected_catalog_index.unwrap_or(0);
    let catalog_name = catalog
        .map(|catalog| catalog.name.as_str())
        .unwrap_or("NO CATALOG");
    let catalog_size = catalog.map_or(0, |catalog| catalog.call_indices.len());
    let trigger_text = state
        .effect_trigger_last_key
        .as_ref()
        .and_then(|key| {
            state.effect_trigger_last_id.map(|id| {
                format!(
                    " | TRIG {} ID {} X{}",
                    key, id, state.effect_trigger_last_count
                )
            })
        })
        .unwrap_or_default();
    let position = state.selected_effect_index.and_then(|selected| {
        catalog.and_then(|catalog| {
            catalog
                .call_indices
                .iter()
                .position(|call_index| *call_index == selected)
        })
    });
    let selected_call = state
        .selected_effect_index
        .and_then(|index| state.calls.get(index));
    let effect_id = selected_call.map(|call| match call.kind {
        EffectCallKind::SpEffect { id } => id,
    });
    let effect_label = selected_call
        .and_then(truncated_effect_name)
        .map_or_else(String::new, |name| format!(" {name}"));
    let catalog_display_index = if catalog_count == 0 {
        0
    } else {
        catalog_index.saturating_add(1)
    };
    let mut text = format!(
        "CAT {}/{} {} | ID {}{} | {}/{} | {} | NET {}{}",
        catalog_display_index,
        catalog_count,
        catalog_name,
        effect_id.map_or_else(|| "NONE".to_owned(), |id| id.to_string()),
        effect_label,
        position.map_or(0, |position| position.saturating_add(1)),
        catalog_size,
        if state.effect_hotkeys_effects_on {
            "ON"
        } else {
            "OFF"
        },
        if state.network_sync { "ON" } else { "OFF" },
        trigger_text
    );
    text = text
        .chars()
        .map(|c| {
            let c = c.to_ascii_uppercase();
            if c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    ' ' | '-'
                        | '_'
                        | '/'
                        | ':'
                        | '['
                        | ']'
                        | '('
                        | ')'
                        | '>'
                        | '?'
                        | '!'
                        | '%'
                        | '.'
                )
            {
                c
            } else {
                ' '
            }
        })
        .collect();
    if text.len() > 128 {
        text.truncate(128);
    }
    if let Ok(mut slot) = EFFECT_SELECTOR_TEXT
        .get_or_init(|| Mutex::new(String::new()))
        .lock()
    {
        *slot = text;
    }
}

pub(crate) fn consume_effect_hotkeys(player: &mut PlayerIns, state: &mut NetEffectsState) {
    poll_effect_trigger_hotkeys(state);
    consume_effect_trigger_hotkeys(player, state);
    let toggles = EFFECT_HOTKEY_PENDING_TOGGLE.swap(0, Ordering::SeqCst);
    let ups = EFFECT_HOTKEY_PENDING_UP.swap(0, Ordering::SeqCst);
    let downs = EFFECT_HOTKEY_PENDING_DOWN.swap(0, Ordering::SeqCst);
    let lefts = EFFECT_HOTKEY_PENDING_LEFT.swap(0, Ordering::SeqCst);
    let rights = EFFECT_HOTKEY_PENDING_RIGHT.swap(0, Ordering::SeqCst);
    let arrow_total = ups + downs + lefts + rights;
    let arrows_allowed = state.effect_selector_visible;
    let applied_total = toggles + if arrows_allowed { arrow_total } else { 0 };
    if !arrows_allowed && arrow_total != 0 {
        state.last_driver_command = Some(format!(
            "effect-hotkey: ignored {arrow_total} arrow keypresses because selector is hidden"
        ));
    }
    if applied_total == 0 {
        return;
    }
    EFFECT_HOTKEY_APPLIED_ACTIONS.fetch_add(applied_total, Ordering::SeqCst);
    for _ in 0..toggles {
        toggle_selected_effect(player, state);
    }
    if !arrows_allowed {
        return;
    }
    for _ in 0..lefts {
        step_selected_catalog(player, state, -1);
    }
    for _ in 0..rights {
        step_selected_catalog(player, state, 1);
    }
    for _ in 0..ups {
        step_selected_effect(player, state, -1);
    }
    for _ in 0..downs {
        step_selected_effect(player, state, 1);
    }
}

fn poll_effect_trigger_hotkeys(state: &mut NetEffectsState) {
    let modified = current_effect_trigger_hotkeys_modified();
    if state.effect_trigger_hotkeys_modified == modified {
        return;
    }
    state.effect_trigger_hotkeys_modified = modified;
    match load_effect_trigger_hotkeys() {
        Ok(hotkeys) => {
            let count = hotkeys.len();
            state.effect_trigger_hotkeys = hotkeys;
            state.effect_trigger_hotkeys_load_error = None;
            state.last_driver_command = Some(format!("effect-trigger: loaded {count} hotkeys"));
        }
        Err(error) => {
            state.effect_trigger_hotkeys.clear();
            state.effect_trigger_hotkeys_load_error = Some(error.clone());
            state.last_driver_command = Some(format!("effect-trigger: {error}"));
        }
    }
}

fn consume_effect_trigger_hotkeys(player: &mut PlayerIns, state: &mut NetEffectsState) {
    let pending = drain_effect_trigger_keys();
    if pending.is_empty() || state.effect_trigger_hotkeys.is_empty() {
        return;
    }
    let mut hidden_arrow_ignores = 0usize;
    for keypress in pending {
        if !state.effect_selector_visible && is_arrow_key(keypress.vk) {
            hidden_arrow_ignores = hidden_arrow_ignores.saturating_add(1);
            continue;
        }
        let matched = state
            .effect_trigger_hotkeys
            .iter()
            .find(|hotkey| hotkey.key.vk == keypress.vk && hotkey.key.alt == keypress.alt)
            .cloned();
        let Some(hotkey) = matched else {
            continue;
        };
        trigger_effect_hotkey(player, state, &hotkey);
    }
    if hidden_arrow_ignores != 0 {
        state.last_driver_command = Some(format!(
            "effect-trigger: ignored {hidden_arrow_ignores} arrow keypresses because selector is hidden"
        ));
    }
}

fn is_arrow_key(vk: u32) -> bool {
    matches!(vk, VK_LEFT | VK_UP | VK_RIGHT | VK_DOWN)
}

fn trigger_effect_hotkey(
    player: &mut PlayerIns,
    state: &mut NetEffectsState,
    hotkey: &EffectTriggerHotkey,
) {
    let count = hotkey.count.clamp(1, EFFECT_TRIGGER_COUNT_MAX);
    state.effect_trigger_last_key = Some(hotkey.key_name.clone());
    state.effect_trigger_last_id = Some(hotkey.effect_id);
    state.effect_trigger_last_count = count;
    apply_effect_trigger_now(player, state, hotkey);
}

fn apply_effect_trigger_now(
    player: &mut PlayerIns,
    state: &mut NetEffectsState,
    hotkey: &EffectTriggerHotkey,
) {
    let count = hotkey.count.clamp(1, EFFECT_TRIGGER_COUNT_MAX);
    for _ in 0..count {
        EffectCallKind::SpEffect {
            id: hotkey.effect_id,
        }
        .apply(player, state.network_sync);
    }
    let active = player
        .chr_ins
        .special_effect
        .entries()
        .any(|entry| entry.param_id == hotkey.effect_id);
    state.effect_trigger_fire_count = state.effect_trigger_fire_count.saturating_add(1);
    state.effect_setting_last_id = Some(hotkey.effect_id);
    if let Some(index) = find_call_index_by_id(&state.calls, hotkey.effect_id) {
        state.selected_effect_index = Some(index);
        if let Some(catalog_index) = state.catalogs.iter().position(|catalog| {
            catalog
                .call_indices
                .iter()
                .any(|call_index| *call_index == index)
        }) {
            state.selected_catalog_index = Some(catalog_index);
        }
    }
    state.last_driver_command = Some(format!(
        "effect-trigger: {} fired {} x{} ({}, network_sync={})",
        hotkey.key_name,
        hotkey.effect_id,
        count,
        if active { "active" } else { "not active" },
        state.network_sync
    ));
}

pub(crate) fn apply_pending_effect_work(player: &mut PlayerIns, state: &mut NetEffectsState) {
    let pending_triggers = std::mem::take(&mut state.pending_effect_triggers);
    for hotkey in pending_triggers {
        state.effect_trigger_last_key = Some(hotkey.key_name.clone());
        state.effect_trigger_last_id = Some(hotkey.effect_id);
        state.effect_trigger_last_count = hotkey.count.clamp(1, EFFECT_TRIGGER_COUNT_MAX);
        apply_effect_trigger_now(player, state, &hotkey);
    }
    apply_pending_enabled_calls(player, state);
}

fn toggle_selected_effect(player: &mut PlayerIns, state: &mut NetEffectsState) {
    if state.effect_hotkeys_effects_on {
        disable_all_calls(player, state);
        state.effect_hotkeys_effects_on = false;
        persist_effects_enabled(false);
        state.last_driver_command = Some("effect-hotkey: toggled effects off".to_owned());
        return;
    }

    let Some(index) = state
        .selected_effect_index
        .filter(|index| *index < state.calls.len())
        .or_else(|| (!state.calls.is_empty()).then_some(0))
    else {
        state.last_driver_command = Some("effect-hotkey: no effects available".to_owned());
        return;
    };
    enable_only_call(player, state, index, true);
}

fn selected_catalog_indices(state: &NetEffectsState) -> Vec<usize> {
    state
        .selected_catalog_index
        .and_then(|catalog_index| state.catalogs.get(catalog_index))
        .map(|catalog| catalog.call_indices.clone())
        .unwrap_or_else(|| (0..state.calls.len()).collect())
}

fn step_selected_effect(player: &mut PlayerIns, state: &mut NetEffectsState, delta: isize) {
    let catalog_indices = selected_catalog_indices(state);
    let len = catalog_indices.len();
    if len == 0 {
        state.last_driver_command = Some("effect-hotkey: no effects available".to_owned());
        return;
    }
    let current_position = state
        .selected_effect_index
        .and_then(|selected| catalog_indices.iter().position(|index| *index == selected));
    let next_position = match (current_position, delta.is_negative()) {
        (Some(index), false) => (index + 1) % len,
        (Some(index), true) => (index + len - 1) % len,
        (None, false) => 0,
        (None, true) => len - 1,
    };
    enable_only_call(player, state, catalog_indices[next_position], true);
}

fn step_selected_catalog(player: &mut PlayerIns, state: &mut NetEffectsState, delta: isize) {
    let len = state.catalogs.len();
    if len == 0 {
        state.last_driver_command = Some("effect-hotkey: no catalogs available".to_owned());
        return;
    }
    let current = state.selected_catalog_index.filter(|index| *index < len);
    let next = match (current, delta.is_negative()) {
        (Some(index), false) => (index + 1) % len,
        (Some(index), true) => (index + len - 1) % len,
        (None, false) => 0,
        (None, true) => len - 1,
    };
    select_catalog(player, state, next, true);
}

fn select_catalog(
    player: &mut PlayerIns,
    state: &mut NetEffectsState,
    catalog_index: usize,
    persist: bool,
) {
    let Some(catalog) = state.catalogs.get(catalog_index) else {
        return;
    };
    state.selected_catalog_index = Some(catalog_index);
    let selected_in_catalog = state
        .selected_effect_index
        .filter(|selected| catalog.call_indices.iter().any(|index| index == selected));
    let next_effect_index = selected_in_catalog.or_else(|| catalog.call_indices.first().copied());
    let catalog_name = catalog.name.clone();
    let catalog_file = catalog.file_name.clone();
    if persist {
        persist_selected_catalog(catalog);
    }
    if let Some(index) = next_effect_index {
        state.selected_effect_index = Some(index);
        if persist && let Some(call) = state.calls.get(index) {
            persist_selected_effect(call);
            state.effect_setting_last_modified = current_effect_setting_modified();
        }
        if state.effect_hotkeys_effects_on {
            enable_only_call(player, state, index, persist);
            return;
        }
    }
    state.last_driver_command = Some(format!(
        "effect-hotkey: catalog {} ({})",
        catalog_name, catalog_file
    ));
}

fn disable_all_calls(player: &mut PlayerIns, state: &mut NetEffectsState) {
    for call in &mut state.calls {
        call.kind.remove(player);
        call.enabled = false;
        call.remove_requested = false;
        call.active_seen_since_enable = false;
        call.apply_failed = false;
        call.active = call.kind.is_active(player);
    }
}

fn enable_only_call(
    player: &mut PlayerIns,
    state: &mut NetEffectsState,
    index: usize,
    persist: bool,
) {
    let network_sync = state.network_sync;
    for (call_index, call) in state.calls.iter_mut().enumerate() {
        if call_index == index {
            call.enabled = true;
            call.remove_requested = false;
            call.kind.apply(player, network_sync);
            call.active = call.kind.is_active(player);
            call.active_seen_since_enable = call.active;
            call.apply_failed = !call.active;
        } else {
            call.kind.remove(player);
            call.enabled = false;
            call.remove_requested = false;
            call.active_seen_since_enable = false;
            call.apply_failed = false;
            call.active = call.kind.is_active(player);
        }
    }
    state.selected_effect_index = Some(index);
    state.effect_hotkeys_effects_on = true;
    if persist {
        persist_effects_enabled(true);
    }
    if persist && let Some(call) = state.calls.get(index) {
        let EffectCallKind::SpEffect { id } = call.kind;
        let label = call.kind.label();
        let name = call.name.clone();
        persist_selected_effect(call);
        if let Some(catalog_index) = state.selected_catalog_index
            && let Some(catalog) = state.catalogs.get(catalog_index)
        {
            persist_selected_catalog(catalog);
        }
        state.effect_setting_last_id = Some(id);
        state.effect_setting_last_modified = current_effect_setting_modified();
        state.last_driver_command = Some(format!(
            "effect-hotkey: selected {label} ({name}); network_sync={}",
            state.network_sync
        ));
    }
}

pub(crate) fn poll_live_effect_catalogs(player: &mut PlayerIns, state: &mut NetEffectsState) {
    let signature = current_effect_catalog_signature();
    if state.effect_catalogs_signature == signature {
        return;
    }

    let selected_id = state
        .effect_setting_last_id
        .or_else(restore_selected_effect_id);
    let selected_catalog_key = state
        .selected_catalog_index
        .and_then(|index| state.catalogs.get(index))
        .map(|catalog| catalog.source_key.clone())
        .or_else(restore_selected_catalog_key);
    let effects_were_on = state.effect_hotkeys_effects_on;

    disable_all_calls(player, state);
    let (calls, catalogs, load_error) = build_effect_catalog_state();
    state.calls = calls;
    state.catalogs = catalogs;
    state.load_error = load_error;
    state.effect_catalogs_signature = signature;
    state.effect_catalog_live_updates = state.effect_catalog_live_updates.saturating_add(1);

    state.selected_effect_index =
        selected_id.and_then(|id| find_call_index_by_id(&state.calls, id));
    state.selected_catalog_index = selected_catalog_key
        .as_deref()
        .and_then(|key| {
            state
                .catalogs
                .iter()
                .position(|catalog| catalog.source_key == key)
        })
        .or_else(|| {
            state.selected_effect_index.and_then(|selected| {
                state.catalogs.iter().position(|catalog| {
                    catalog
                        .call_indices
                        .iter()
                        .any(|call_index| *call_index == selected)
                })
            })
        })
        .or_else(|| (!state.catalogs.is_empty()).then_some(0));

    if effects_were_on && let Some(index) = state.selected_effect_index {
        enable_only_call(player, state, index, false);
    }
    state.last_driver_command = Some(format!(
        "effect-catalog: reloaded {} catalogs ({} calls)",
        state.catalogs.len(),
        state.calls.len()
    ));
}

pub(crate) fn poll_live_effect_setting(player: &mut PlayerIns, state: &mut NetEffectsState) {
    let modified = current_effect_setting_modified();
    if state.effect_setting_last_modified == modified {
        return;
    }
    state.effect_setting_last_modified = modified;
    let Ok(raw_id) = fs::read_to_string(effect_setting_path()) else {
        state.last_driver_command = Some("effect-setting: failed to read live setting".to_owned());
        return;
    };
    let trimmed = raw_id.trim();
    let Ok(id) = trimmed.parse::<i32>() else {
        state.last_driver_command = Some(format!("effect-setting: invalid id {trimmed:?}"));
        return;
    };
    state.effect_setting_last_id = Some(id);

    let Some(index) = find_call_index_by_id(&state.calls, id) else {
        state.last_driver_command = Some(format!("effect-setting: id {id} is not in catalog"));
        return;
    };
    let current_catalog_contains_id = state
        .selected_catalog_index
        .and_then(|catalog_index| state.catalogs.get(catalog_index))
        .is_some_and(|catalog| {
            catalog
                .call_indices
                .iter()
                .any(|call_index| *call_index == index)
        });
    if !current_catalog_contains_id
        && let Some(catalog_index) = state.catalogs.iter().position(|catalog| {
            catalog
                .call_indices
                .iter()
                .any(|call_index| *call_index == index)
        })
    {
        state.selected_catalog_index = Some(catalog_index);
    }
    if let Some(catalog_index) = state.selected_catalog_index
        && let Some(catalog) = state.catalogs.get(catalog_index)
    {
        persist_selected_catalog(catalog);
    }

    enable_only_call(player, state, index, false);
    persist_effects_enabled(true);
    state.effect_setting_live_updates = state.effect_setting_live_updates.saturating_add(1);
    if let Some(call) = state.calls.get(index) {
        state.last_driver_command = Some(format!(
            "effect-setting: selected {} ({}); network_sync={}",
            call.kind.label(),
            call.name,
            state.network_sync
        ));
    }
}

pub(crate) fn process_driver_command(player: &mut PlayerIns, state: &mut NetEffectsState) {
    let path = command_path();
    let Ok(raw_command) = fs::read_to_string(&path) else {
        return;
    };
    let _ = fs::remove_file(path);
    execute_and_record_driver_command(player, state, raw_command.trim());
}

fn execute_and_record_driver_command(
    player: &mut PlayerIns,
    state: &mut NetEffectsState,
    command: &str,
) {
    if command.is_empty() {
        return;
    }
    state.last_driver_command = Some(match execute_driver_command(player, state, command) {
        Ok(()) => format!("ok: {command}"),
        Err(error) => format!("error: {command}: {error}"),
    });
}

fn execute_driver_command(
    player: &mut PlayerIns,
    state: &mut NetEffectsState,
    command: &str,
) -> Result<(), String> {
    let parts: Vec<_> = command.split_whitespace().collect();
    match parts.as_slice() {
        ["selector", "on"] | ["overlay", "on"] => {
            state.effect_selector_visible = true;
            sync_effect_selector_input_suppression(true);
            Ok(())
        }
        ["selector", "off"] | ["overlay", "off"] => {
            state.effect_selector_visible = false;
            sync_effect_selector_input_suppression(false);
            Ok(())
        }
        ["selector", "toggle"] | ["overlay", "toggle"] => {
            state.effect_selector_visible = !state.effect_selector_visible;
            sync_effect_selector_input_suppression(state.effect_selector_visible);
            Ok(())
        }
        ["network", "on"] => {
            state.network_sync = true;
            Ok(())
        }
        ["network", "off"] => {
            state.network_sync = false;
            Ok(())
        }
        ["network", "toggle"] => {
            state.network_sync = !state.network_sync;
            Ok(())
        }
        ["apply_all"] => {
            apply_selected_calls(player, state);
            refresh_call_status(player, state);
            Ok(())
        }
        ["remove_all"] => {
            for call in &mut state.calls {
                call.kind.remove(player);
                call.enabled = false;
                call.remove_requested = false;
                call.active_seen_since_enable = false;
                call.apply_failed = false;
            }
            state.effect_hotkeys_effects_on = false;
            persist_effects_enabled(false);
            refresh_call_status(player, state);
            Ok(())
        }
        ["apply", index] => set_call_enabled(player, state, parse_call_index(index)?, true),
        ["remove", index] => set_call_enabled(player, state, parse_call_index(index)?, false),
        ["set", index, "on"] => set_call_enabled(player, state, parse_call_index(index)?, true),
        ["set", index, "off"] => set_call_enabled(player, state, parse_call_index(index)?, false),
        ["toggle", index] => {
            let index = parse_call_index(index)?;
            let enabled = !state
                .calls
                .get(index)
                .ok_or_else(|| format!("call index {index} out of range"))?
                .enabled;
            set_call_enabled(player, state, index, enabled)
        }
        _ => Err("expected selector/overlay on|off|toggle, network on|off|toggle, apply_all, remove_all, apply <index>, remove <index>, set <index> on|off, or toggle <index>".to_owned()),
    }
}

fn parse_call_index(index: &str) -> Result<usize, String> {
    index
        .parse()
        .map_err(|error| format!("invalid call index {index:?}: {error}"))
}

fn set_call_enabled(
    player: &mut PlayerIns,
    state: &mut NetEffectsState,
    index: usize,
    enabled: bool,
) -> Result<(), String> {
    let network_sync = state.network_sync;
    let call = state
        .calls
        .get_mut(index)
        .ok_or_else(|| format!("call index {index} out of range"))?;

    call.enabled = enabled;
    if enabled {
        call.kind.apply(player, network_sync);
        call.active = call.kind.is_active(player);
        call.active_seen_since_enable = call.active;
        call.apply_failed = !call.active;
        state.selected_effect_index = Some(index);
        state.effect_hotkeys_effects_on = true;
        persist_effects_enabled(true);
        persist_selected_effect(call);
        state.effect_setting_last_modified = current_effect_setting_modified();
        if let Some(catalog_index) = state.selected_catalog_index
            && let Some(catalog) = state.catalogs.get(catalog_index)
        {
            persist_selected_catalog(catalog);
        }
    } else {
        call.kind.remove(player);
        call.remove_requested = false;
        call.active_seen_since_enable = false;
        call.apply_failed = false;
        call.active = call.kind.is_active(player);
        if state.selected_effect_index == Some(index) {
            state.effect_hotkeys_effects_on = false;
            persist_effects_enabled(false);
        }
    }

    Ok(())
}

pub(crate) fn remove_requested_calls(player: &mut PlayerIns, state: &mut NetEffectsState) {
    for call in &mut state.calls {
        if call.remove_requested {
            call.kind.remove(player);
            call.remove_requested = false;
            call.active_seen_since_enable = false;
            call.apply_failed = false;
        }
    }
}

fn apply_selected_calls(player: &mut PlayerIns, state: &mut NetEffectsState) {
    let network_sync = state.network_sync;
    for call in state.calls.iter_mut().filter(|call| call.enabled) {
        call.kind.apply(player, network_sync);
        call.active = call.kind.is_active(player);
        call.active_seen_since_enable |= call.active;
        call.apply_failed = !call.active;
    }
}

fn apply_pending_enabled_calls(player: &mut PlayerIns, state: &mut NetEffectsState) {
    let network_sync = state.network_sync;
    for call in state
        .calls
        .iter_mut()
        .filter(|call| call.enabled && !call.active_seen_since_enable && !call.apply_failed)
    {
        call.kind.apply(player, network_sync);
        call.active = call.kind.is_active(player);
        call.active_seen_since_enable |= call.active;
        call.apply_failed = !call.active;
    }
}

pub(crate) fn reapply_expired_enabled_calls(player: &mut PlayerIns, state: &mut NetEffectsState) {
    let network_sync = state.network_sync;
    for (index, call) in state
        .calls
        .iter_mut()
        .enumerate()
        .filter(|(_, call)| call.enabled && !call.active && call.active_seen_since_enable)
    {
        call.kind.apply(player, network_sync);
        call.active = call.kind.is_active(player);
        call.active_seen_since_enable |= call.active;
        call.apply_failed = !call.active;
        state.effect_reapply_count = state.effect_reapply_count.saturating_add(1);
        state.effect_reapply_last_index = Some(index);
    }
}

pub(crate) fn refresh_call_status(player: &PlayerIns, state: &mut NetEffectsState) {
    for call in &mut state.calls {
        call.active = call.kind.is_active(player);
        if call.active {
            call.active_seen_since_enable = true;
            call.apply_failed = false;
        }
    }
}

fn find_call_index_by_id(calls: &[NamedEffectCall], id: i32) -> Option<usize> {
    calls.iter().position(|call| match call.kind {
        EffectCallKind::SpEffect { id: call_id } => call_id == id,
    })
}

pub(crate) fn selected_catalog_position(state: &NetEffectsState) -> Option<usize> {
    let selected = state.selected_effect_index?;
    state
        .selected_catalog_index
        .and_then(|catalog_index| state.catalogs.get(catalog_index))
        .and_then(|catalog| {
            catalog
                .call_indices
                .iter()
                .position(|call_index| *call_index == selected)
        })
}

pub(crate) fn selected_effect_id(state: &NetEffectsState) -> Option<i32> {
    state
        .selected_effect_index
        .and_then(|index| state.calls.get(index))
        .map(|call| match call.kind {
            EffectCallKind::SpEffect { id } => id,
        })
}

pub(crate) fn call_status_text(call: &NamedEffectCall) -> &'static str {
    if call.active {
        "active"
    } else if call.apply_failed {
        "apply_failed"
    } else {
        "inactive"
    }
}

pub(crate) fn dinput_kb_hook_fires() -> usize {
    input_suppression::dinput_kb_hook_fires()
}

pub(crate) fn dinput_mouse_hook_fires() -> usize {
    input_suppression::dinput_mouse_hook_fires()
}

pub(crate) fn dinput_suppressed_arrow_keys() -> usize {
    input_suppression::dinput_suppressed_arrow_keys()
}
