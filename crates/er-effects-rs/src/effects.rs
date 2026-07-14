use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use eldenring::cs::{ChrInsExt, PlayerIns};
use er_effects_data::{
    BUILT_IN_EFFECT_CATALOGS, EffectCallSpec, EffectKindSpec, embedded_effect_master_catalog,
    embedded_effects, parse_effect_hotkeys_json, parse_effect_id_catalog_json,
};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, HC_ACTION, KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_SYSKEYDOWN,
};

use crate::{EffectsState, append_autoload_debug, command_path};

const EFFECT_HOTKEY_UP: usize = 1 << 0;
const EFFECT_HOTKEY_DOWN: usize = 1 << 1;
const EFFECT_HOTKEY_LEFT: usize = 1 << 2;
const EFFECT_HOTKEY_RIGHT: usize = 1 << 3;
const EFFECT_HOTKEY_TOGGLE: usize = 1 << 4;
const EFFECT_HOTKEY_OVERLAY_TOGGLE: usize = 1 << 5;

const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;
const VK_INSERT: u32 = 0x2d;
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
      "name": "deathblight self test",
      "key": "numpad_multiply",
      "effect_id": 8355,
      "count": 1
    }
  ]
}
"#;
const EFFECT_TRIGGER_COUNT_MAX: u32 = 200;

static EFFECT_HOTKEY_PENDING_UP: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_PENDING_DOWN: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_PENDING_LEFT: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_PENDING_RIGHT: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_PENDING_TOGGLE: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_PENDING_OVERLAY_TOGGLE: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_HOOK_STARTED: AtomicBool = AtomicBool::new(false);
static EFFECT_HOTKEY_HOOK_ACTIVE: AtomicBool = AtomicBool::new(false);
static EFFECT_SELECTOR_OVERLAY_TEXT: OnceLock<Mutex<String>> = OnceLock::new();
static EFFECT_TRIGGER_PENDING_KEYS: OnceLock<Mutex<Vec<EffectTriggerKeyPress>>> = OnceLock::new();
pub(crate) static EFFECT_HOTKEY_HOOK_HITS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static EFFECT_HOTKEY_APPLIED_ACTIONS: AtomicUsize = AtomicUsize::new(0);

/// A named runtime effect call the overlay can trigger.
///
/// Adding a new call kind (e.g. an SFX/FXR spawn once `fromsoftware-rs`
/// exposes a wrapper for it) takes three mechanical steps:
/// 1. add a variant to `er_effects_data::EffectKindSpec` (the
///    `data/effects.json` schema),
/// 2. add the matching variant here plus arms in `label`/`apply`/`remove`/
///    `is_active`,
/// 3. map it in `call_kind_from_spec`.
/// The overlay and the game task dispatch exclusively through those four
/// methods, so nothing else needs to change.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectCallKind {
    SpEffect { id: i32 },
}

impl EffectCallKind {
    pub(crate) fn label(self) -> String {
        match self {
            Self::SpEffect { id } => format!("SpEffect {id}"),
        }
    }

    pub(crate) fn apply(self, player: &mut PlayerIns, network_sync: bool) {
        match self {
            Self::SpEffect { id } => {
                let dont_sync = !network_sync;
                player.apply_speffect(id, dont_sync);
            }
        }
    }

    pub(crate) fn remove(self, player: &mut PlayerIns) {
        match self {
            Self::SpEffect { id } => player.chr_ins.remove_speffect(id),
        }
    }

    /// Whether the call is currently in force on the player. The game's
    /// apply/remove calls return nothing, so the active-SpEffect list is the
    /// ground truth for surfacing success and failure in the overlay.
    pub(crate) fn is_active(self, player: &PlayerIns) -> bool {
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
    pub(crate) remove_requested: bool,
    /// Live status, recomputed every game tick from the player's SpEffect
    /// list.
    pub(crate) active: bool,
    /// Latched after this enabled call has appeared in the player's active
    /// SpEffect list. Once latched, the game task can reapply the call after a
    /// finite-duration effect naturally expires without hammering IDs that
    /// never took in the first place.
    pub(crate) active_seen_since_enable: bool,
    /// Set when an apply attempt did not take (e.g. the ID has no
    /// `SpEffectParam` row); cleared as soon as the effect shows up active.
    pub(crate) apply_failed: bool,
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
    pub(crate) source_key: String,
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
pub(crate) struct EffectTriggerKey {
    vk: u32,
    alt: bool,
}

#[derive(Clone)]
pub(crate) struct EffectTriggerHotkey {
    pub(crate) name: String,
    pub(crate) key_name: String,
    key: EffectTriggerKey,
    pub(crate) effect_id: i32,
    pub(crate) count: u32,
}

pub(crate) fn effect_setting_path() -> PathBuf {
    PathBuf::from(".effect-setting.txt")
}

pub(crate) fn effect_catalog_setting_path() -> PathBuf {
    PathBuf::from(".effect-catalog-setting.txt")
}

pub(crate) fn effect_enabled_setting_path() -> PathBuf {
    PathBuf::from(".effect-enabled-setting.txt")
}

pub(crate) fn effect_trigger_hotkeys_path() -> PathBuf {
    PathBuf::from(".effect-hotkeys.json")
}

pub(crate) fn user_effect_catalog_dir() -> PathBuf {
    PathBuf::from("effect-catalogs")
}

pub(crate) fn current_effect_setting_modified() -> Option<std::time::SystemTime> {
    fs::metadata(effect_setting_path())
        .and_then(|metadata| metadata.modified())
        .ok()
}

pub(crate) fn restore_selected_effect_id() -> Option<i32> {
    let path = effect_setting_path();
    fs::read_to_string(path).ok()?.trim().parse::<i32>().ok()
}

pub(crate) fn restore_selected_catalog_key() -> Option<String> {
    let value = fs::read_to_string(effect_catalog_setting_path()).ok()?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

pub(crate) fn restore_effects_enabled() -> bool {
    let Ok(value) = fs::read_to_string(effect_enabled_setting_path()) else {
        return false;
    };
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "on" | "true" | "1" | "enabled"
    )
}

pub(crate) fn persist_effects_enabled(enabled: bool) {
    let path = effect_enabled_setting_path();
    let tmp_path = path.with_extension("txt.tmp");
    let value = if enabled { "on\n" } else { "off\n" };
    if fs::write(&tmp_path, value).is_ok() {
        let _ = fs::rename(tmp_path, path);
    }
}

pub(crate) fn restore_selected_effect_index(calls: &[NamedEffectCall]) -> Option<usize> {
    let id = restore_selected_effect_id()?;
    find_call_index_by_id(calls, id)
}

fn find_call_index_by_id(calls: &[NamedEffectCall], id: i32) -> Option<usize> {
    calls.iter().position(|call| match call.kind {
        EffectCallKind::SpEffect { id: call_id } => call_id == id,
    })
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

pub(crate) fn current_effect_trigger_hotkeys_modified() -> Option<std::time::SystemTime> {
    fs::metadata(effect_trigger_hotkeys_path())
        .and_then(|metadata| metadata.modified())
        .ok()
}

pub(crate) fn load_effect_trigger_hotkeys() -> Result<Vec<EffectTriggerHotkey>, String> {
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
    if !(VK_NUMPAD0..=VK_NUMPAD9).contains(&vk)
        && !matches!(
            vk,
            VK_MULTIPLY
                | VK_ADD
                | VK_SUBTRACT
                | VK_DECIMAL
                | VK_DIVIDE
                | VK_UP
                | VK_DOWN
                | VK_LEFT
                | VK_RIGHT
                | VK_OEM_7
        )
    {
        return Err(format!("unsupported key {raw:?}"));
    }
    Ok(EffectTriggerKey { vk, alt })
}

pub(crate) fn call_kind_from_spec(kind: EffectKindSpec, id: i32) -> EffectCallKind {
    match kind {
        EffectKindSpec::SpEffect => EffectCallKind::SpEffect { id },
    }
}

pub(crate) fn named_call_from_spec(spec: EffectCallSpec) -> NamedEffectCall {
    let kind = call_kind_from_spec(spec.kind, spec.id);
    NamedEffectCall::new(spec.name, kind, spec.enabled)
}

struct RawEffectCatalog {
    source_key: String,
    file_name: String,
    ids: Vec<i32>,
}

pub(crate) fn build_effect_catalog_state()
-> (Vec<NamedEffectCall>, Vec<EffectCatalog>, Option<String>) {
    let master = match embedded_effect_master_catalog() {
        Ok(master) => master,
        Err(error) => {
            let fallback = embedded_effects()
                .map(|effects| {
                    effects
                        .calls
                        .into_iter()
                        .map(named_call_from_spec)
                        .collect()
                })
                .unwrap_or_default();
            return (
                fallback,
                Vec::new(),
                Some(format!(
                    "failed to parse embedded effect master catalog: {error}"
                )),
            );
        }
    };
    let master_names = master
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
        .collect::<HashMap<_, _>>();

    let mut raw_catalogs = Vec::new();
    let mut load_errors = Vec::new();
    for catalog in BUILT_IN_EFFECT_CATALOGS {
        match parse_effect_id_catalog_json(catalog.json) {
            Ok(ids) => raw_catalogs.push(RawEffectCatalog {
                source_key: format!("builtin/{}", catalog.file_name),
                file_name: catalog.file_name.to_owned(),
                ids,
            }),
            Err(error) => load_errors.push(format!("{}: {error}", catalog.file_name)),
        }
    }
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
                    source_key: format!("user/{file_name}"),
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
            let Some(name) = master_names.get(&id).cloned() else {
                invalid_ids = invalid_ids.saturating_add(1);
                continue;
            };
            let index = *call_index_by_id.entry(id).or_insert_with(|| {
                let index = calls.len();
                calls.push(NamedEffectCall::new(
                    name,
                    EffectCallKind::SpEffect { id },
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

pub(crate) fn call_status_text(call: &NamedEffectCall) -> &'static str {
    if call.active {
        "[active]"
    } else if call.apply_failed {
        "[apply failed]"
    } else {
        "[inactive]"
    }
}

pub(crate) fn effect_application_allowed(_state: &EffectsState) -> bool {
    // All callers already hold a live PlayerIns reference. Requiring the sampled TimeAct
    // animation id here makes standing-idle characters look unavailable until movement
    // changes the sampled slot, even though SpEffect application is already valid.
    true
}

fn effect_application_block_reason(state: &EffectsState) -> &'static str {
    if effect_application_allowed(state) {
        "ready"
    } else {
        "player is not loaded"
    }
}

pub(crate) fn add_custom_call(state: &mut EffectsState) {
    let id = state.custom_call_id;
    let kind = EffectCallKind::SpEffect { id };
    if state.calls.iter().any(|call| call.kind == kind) {
        return;
    }
    state
        .calls
        .push(NamedEffectCall::new(format!("Custom {id}"), kind, true));
}

fn effect_hotkey_action_for_key(vk: u32, alt_down: bool) -> usize {
    match vk {
        VK_LEFT => EFFECT_HOTKEY_LEFT,
        VK_UP => EFFECT_HOTKEY_UP,
        VK_RIGHT => EFFECT_HOTKEY_RIGHT,
        VK_DOWN => EFFECT_HOTKEY_DOWN,
        VK_OEM_7 if alt_down => EFFECT_HOTKEY_TOGGLE,
        VK_NUMPAD0 | VK_INSERT if alt_down => EFFECT_HOTKEY_OVERLAY_TOGGLE,
        _ => 0,
    }
}

fn queue_effect_trigger_key(vk: u32, alt: bool) {
    if let Ok(mut pending) = EFFECT_TRIGGER_PENDING_KEYS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
    {
        if pending.len() < 64 {
            pending.push(EffectTriggerKeyPress { vk, alt });
        }
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
        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
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
                if action & EFFECT_HOTKEY_OVERLAY_TOGGLE != 0 {
                    EFFECT_HOTKEY_PENDING_OVERLAY_TOGGLE.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, ncode, wparam, lparam) }
}

pub(crate) fn ensure_effect_hotkey_hook() {
    if EFFECT_HOTKEY_HOOK_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("er-effects-hotkeys".to_owned())
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
                append_autoload_debug(format_args!("effect-hotkeys: hook install failed"));
                return;
            };
            EFFECT_HOTKEY_HOOK_ACTIVE.store(true, Ordering::SeqCst);
            append_autoload_debug(format_args!("effect-hotkeys: hook installed"));
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

pub(crate) fn effect_selector_overlay_text() -> String {
    EFFECT_SELECTOR_OVERLAY_TEXT
        .get_or_init(|| Mutex::new(String::new()))
        .lock()
        .map(|text| text.clone())
        .unwrap_or_default()
}

pub(crate) fn publish_effect_selector_overlay_text(state: &mut EffectsState) {
    let overlay_toggles = EFFECT_HOTKEY_PENDING_OVERLAY_TOGGLE.swap(0, Ordering::SeqCst);
    if overlay_toggles != 0 {
        if overlay_toggles % 2 == 1 {
            state.effect_selector_overlay_visible = !state.effect_selector_overlay_visible;
        }
        state.last_driver_command = Some(format!(
            "effect-overlay: {}",
            if state.effect_selector_overlay_visible {
                "shown"
            } else {
                "hidden"
            }
        ));
        EFFECT_HOTKEY_APPLIED_ACTIONS.fetch_add(overlay_toggles, Ordering::SeqCst);
    }
    if !state.effect_selector_overlay_visible {
        if let Ok(mut slot) = EFFECT_SELECTOR_OVERLAY_TEXT
            .get_or_init(|| Mutex::new(String::new()))
            .lock()
        {
            slot.clear();
        }
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
    let effect_id = state
        .selected_effect_index
        .and_then(|index| state.calls.get(index))
        .map(|call| match call.kind {
            EffectCallKind::SpEffect { id } => id,
        });
    let mut text = format!(
        "CAT {}/{} {} | ID {} | {}/{} | {}{}",
        catalog_index.saturating_add(1),
        catalog_count,
        catalog_name,
        effect_id.map_or_else(|| "NONE".to_owned(), |id| id.to_string()),
        position.map_or(0, |position| position.saturating_add(1)),
        catalog_size,
        if state.effect_hotkeys_effects_on {
            "ON"
        } else {
            "OFF"
        },
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
    if let Ok(mut slot) = EFFECT_SELECTOR_OVERLAY_TEXT
        .get_or_init(|| Mutex::new(String::new()))
        .lock()
    {
        *slot = text;
    }
}

pub(crate) fn consume_effect_hotkeys(player: &mut PlayerIns, state: &mut EffectsState) {
    poll_effect_trigger_hotkeys(state);
    consume_effect_trigger_hotkeys(player, state);
    let toggles = EFFECT_HOTKEY_PENDING_TOGGLE.swap(0, Ordering::SeqCst);
    let ups = EFFECT_HOTKEY_PENDING_UP.swap(0, Ordering::SeqCst);
    let downs = EFFECT_HOTKEY_PENDING_DOWN.swap(0, Ordering::SeqCst);
    let lefts = EFFECT_HOTKEY_PENDING_LEFT.swap(0, Ordering::SeqCst);
    let rights = EFFECT_HOTKEY_PENDING_RIGHT.swap(0, Ordering::SeqCst);
    let arrow_total = ups + downs + lefts + rights;
    let arrows_allowed = state.effect_selector_overlay_visible;
    let applied_total = toggles + if arrows_allowed { arrow_total } else { 0 };
    if !arrows_allowed && arrow_total != 0 {
        state.last_driver_command = Some(format!(
            "effect-hotkey: ignored {arrow_total} arrow keypresses because debug menu is hidden"
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

fn poll_effect_trigger_hotkeys(state: &mut EffectsState) {
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

fn consume_effect_trigger_hotkeys(player: &mut PlayerIns, state: &mut EffectsState) {
    let pending = drain_effect_trigger_keys();
    if pending.is_empty() || state.effect_trigger_hotkeys.is_empty() {
        return;
    }
    let mut hidden_arrow_ignores = 0usize;
    for keypress in pending {
        if !state.effect_selector_overlay_visible && is_arrow_key(keypress.vk) {
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
            "effect-trigger: ignored {hidden_arrow_ignores} arrow keypresses because debug menu is hidden"
        ));
    }
}

fn is_arrow_key(vk: u32) -> bool {
    matches!(vk, VK_LEFT | VK_UP | VK_RIGHT | VK_DOWN)
}

fn trigger_effect_hotkey(
    player: &mut PlayerIns,
    state: &mut EffectsState,
    hotkey: &EffectTriggerHotkey,
) {
    let count = hotkey.count.clamp(1, EFFECT_TRIGGER_COUNT_MAX);
    state.effect_trigger_last_key = Some(hotkey.key_name.clone());
    state.effect_trigger_last_id = Some(hotkey.effect_id);
    state.effect_trigger_last_count = count;
    if !effect_application_allowed(state) {
        if state.pending_effect_triggers.len() < 16 {
            state.pending_effect_triggers.push(hotkey.clone());
        }
        state.last_driver_command = Some(format!(
            "effect-trigger: {} armed {} x{} until next animation ({})",
            hotkey.key_name,
            hotkey.effect_id,
            count,
            effect_application_block_reason(state)
        ));
        return;
    }
    apply_effect_trigger_now(player, state, hotkey);
}

fn apply_effect_trigger_now(
    player: &mut PlayerIns,
    state: &mut EffectsState,
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
        "effect-trigger: {} fired {} x{} ({})",
        hotkey.key_name,
        hotkey.effect_id,
        count,
        if active { "active" } else { "not active" }
    ));
}

pub(crate) fn apply_pending_effect_work(player: &mut PlayerIns, state: &mut EffectsState) {
    if !effect_application_allowed(state) {
        return;
    }
    let pending_triggers = std::mem::take(&mut state.pending_effect_triggers);
    for hotkey in pending_triggers {
        state.effect_trigger_last_key = Some(hotkey.key_name.clone());
        state.effect_trigger_last_id = Some(hotkey.effect_id);
        state.effect_trigger_last_count = hotkey.count.clamp(1, EFFECT_TRIGGER_COUNT_MAX);
        apply_effect_trigger_now(player, state, &hotkey);
    }
    apply_pending_enabled_calls(player, state);
}

fn toggle_selected_effect(player: &mut PlayerIns, state: &mut EffectsState) {
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

fn selected_catalog_indices(state: &EffectsState) -> Vec<usize> {
    state
        .selected_catalog_index
        .and_then(|catalog_index| state.catalogs.get(catalog_index))
        .map(|catalog| catalog.call_indices.clone())
        .unwrap_or_else(|| (0..state.calls.len()).collect())
}

fn selected_catalog_position(state: &EffectsState) -> Option<usize> {
    let selected = state.selected_effect_index?;
    selected_catalog_indices(state)
        .iter()
        .position(|index| *index == selected)
}

fn step_selected_effect(player: &mut PlayerIns, state: &mut EffectsState, delta: isize) {
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

fn step_selected_catalog(player: &mut PlayerIns, state: &mut EffectsState, delta: isize) {
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
    state: &mut EffectsState,
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

fn disable_all_calls(player: &mut PlayerIns, state: &mut EffectsState) {
    for call in &mut state.calls {
        call.kind.remove(player);
        call.enabled = false;
        call.remove_requested = false;
        call.active_seen_since_enable = false;
        call.apply_failed = false;
        call.active = call.kind.is_active(player);
    }
}

fn enable_only_call(player: &mut PlayerIns, state: &mut EffectsState, index: usize, persist: bool) {
    let network_sync = state.network_sync;
    let can_apply = effect_application_allowed(state);
    for (call_index, call) in state.calls.iter_mut().enumerate() {
        if call_index == index {
            call.enabled = true;
            call.remove_requested = false;
            if can_apply {
                call.kind.apply(player, network_sync);
                call.active = call.kind.is_active(player);
                call.active_seen_since_enable = call.active;
                call.apply_failed = !call.active;
            } else {
                call.active = call.kind.is_active(player);
                call.active_seen_since_enable = false;
                call.apply_failed = false;
            }
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
        state.last_driver_command = Some(if can_apply {
            format!("effect-hotkey: selected {label} ({name})")
        } else {
            format!(
                "effect-hotkey: armed {label} ({name}); not applied because {}",
                effect_application_block_reason(state)
            )
        });
    }
}

pub(crate) fn poll_live_effect_setting(player: &mut PlayerIns, state: &mut EffectsState) {
    let path = effect_setting_path();
    let Ok(metadata) = fs::metadata(&path) else {
        state.effect_setting_last_modified = None;
        return;
    };
    let Ok(modified) = metadata.modified() else {
        return;
    };
    if state.effect_setting_last_modified == Some(modified) {
        return;
    }
    state.effect_setting_last_modified = Some(modified);

    let Ok(raw_id) = fs::read_to_string(&path) else {
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

    let can_apply = effect_application_allowed(state);
    enable_only_call(player, state, index, false);
    persist_effects_enabled(true);
    state.effect_setting_live_updates = state.effect_setting_live_updates.saturating_add(1);
    if let Some(call) = state.calls.get(index) {
        state.last_driver_command = Some(if can_apply {
            format!(
                "effect-setting: selected {} ({})",
                call.kind.label(),
                call.name
            )
        } else {
            format!(
                "effect-setting: armed {} ({}); not applied because {}",
                call.kind.label(),
                call.name,
                effect_application_block_reason(state)
            )
        });
    }
}

pub(crate) fn process_driver_command(player: &mut PlayerIns, state: &mut EffectsState) {
    let path = command_path();
    let Ok(raw_command) = fs::read_to_string(&path) else {
        return;
    };
    let _ = fs::remove_file(path);

    execute_and_record_driver_command(player, state, raw_command.trim());
}

pub(crate) fn execute_and_record_driver_command(
    player: &mut PlayerIns,
    state: &mut EffectsState,
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

pub(crate) fn execute_driver_command(
    player: &mut PlayerIns,
    state: &mut EffectsState,
    command: &str,
) -> Result<(), String> {
    let parts: Vec<_> = command.split_whitespace().collect();
    match parts.as_slice() {
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
        _ => Err("expected apply_all, remove_all, apply <index>, remove <index>, set <index> on|off, toggle <index>, or load_slot <index> before player load".to_owned()),
    }
}

pub(crate) fn parse_call_index(index: &str) -> Result<usize, String> {
    index
        .parse()
        .map_err(|error| format!("invalid call index {index:?}: {error}"))
}

pub(crate) fn set_call_enabled(
    player: &mut PlayerIns,
    state: &mut EffectsState,
    index: usize,
    enabled: bool,
) -> Result<(), String> {
    let can_apply = effect_application_allowed(state);
    let call = state
        .calls
        .get_mut(index)
        .ok_or_else(|| format!("call index {index} out of range"))?;

    call.enabled = enabled;
    if enabled {
        if can_apply {
            call.kind.apply(player, state.network_sync);
            call.active = call.kind.is_active(player);
            call.active_seen_since_enable = call.active;
            call.apply_failed = !call.active;
        } else {
            call.active = call.kind.is_active(player);
            call.active_seen_since_enable = false;
            call.apply_failed = false;
        }
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

pub(crate) fn remove_requested_calls(player: &mut PlayerIns, state: &mut EffectsState) {
    if state.remove_all_requested {
        for call in &mut state.calls {
            call.kind.remove(player);
            call.remove_requested = false;
            call.active_seen_since_enable = false;
            call.apply_failed = false;
        }
        state.remove_all_requested = false;
        return;
    }

    for call in &mut state.calls {
        if call.remove_requested {
            call.kind.remove(player);
            call.remove_requested = false;
            call.active_seen_since_enable = false;
            call.apply_failed = false;
        }
    }
}

pub(crate) fn apply_selected_calls(player: &mut PlayerIns, state: &mut EffectsState) {
    if !effect_application_allowed(state) {
        return;
    }
    let network_sync = state.network_sync;
    for call in state.calls.iter_mut().filter(|call| call.enabled) {
        call.kind.apply(player, network_sync);
        // The game call reports nothing, so check the active list directly.
        call.active = call.kind.is_active(player);
        call.active_seen_since_enable |= call.active;
        call.apply_failed = !call.active;
    }
}

fn apply_pending_enabled_calls(player: &mut PlayerIns, state: &mut EffectsState) {
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

pub(crate) fn reapply_expired_enabled_calls(player: &mut PlayerIns, state: &mut EffectsState) {
    if !effect_application_allowed(state) {
        return;
    }
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

pub(crate) fn refresh_call_status(player: &PlayerIns, state: &mut EffectsState) {
    for call in &mut state.calls {
        call.active = call.kind.is_active(player);
        if call.active {
            call.active_seen_since_enable = true;
            call.apply_failed = false;
        }
    }
}
