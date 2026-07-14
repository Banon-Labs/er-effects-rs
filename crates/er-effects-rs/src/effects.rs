use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use eldenring::cs::{ChrInsExt, PlayerIns};
use er_effects_data::{EffectCallSpec, EffectKindSpec};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, HC_ACTION, KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_SYSKEYDOWN,
};

use crate::{EffectsState, append_autoload_debug, command_path};

const EFFECT_HOTKEY_UP: usize = 1 << 0;
const EFFECT_HOTKEY_DOWN: usize = 1 << 1;
const EFFECT_HOTKEY_TOGGLE: usize = 1 << 2;

const VK_UP: u32 = 0x26;
const VK_DOWN: u32 = 0x28;
const VK_OEM_7: u32 = 0xde;
const LLKHF_ALTDOWN: u32 = 0x20;

static EFFECT_HOTKEY_PENDING_UP: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_PENDING_DOWN: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_PENDING_TOGGLE: AtomicUsize = AtomicUsize::new(0);
static EFFECT_HOTKEY_HOOK_STARTED: AtomicBool = AtomicBool::new(false);
static EFFECT_HOTKEY_HOOK_ACTIVE: AtomicBool = AtomicBool::new(false);
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

pub(crate) fn effect_setting_path() -> PathBuf {
    PathBuf::from(".effect-setting.txt")
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

pub(crate) fn call_kind_from_spec(kind: EffectKindSpec, id: i32) -> EffectCallKind {
    match kind {
        EffectKindSpec::SpEffect => EffectCallKind::SpEffect { id },
    }
}

pub(crate) fn named_call_from_spec(spec: EffectCallSpec) -> NamedEffectCall {
    let kind = call_kind_from_spec(spec.kind, spec.id);
    NamedEffectCall::new(spec.name, kind, spec.enabled)
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
        VK_UP => EFFECT_HOTKEY_UP,
        VK_DOWN => EFFECT_HOTKEY_DOWN,
        VK_OEM_7 if alt_down => EFFECT_HOTKEY_TOGGLE,
        _ => 0,
    }
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
            let action = effect_hotkey_action_for_key(kb.vkCode, alt_down);
            if action != 0 {
                EFFECT_HOTKEY_HOOK_HITS.fetch_add(1, Ordering::SeqCst);
                if action & EFFECT_HOTKEY_UP != 0 {
                    EFFECT_HOTKEY_PENDING_UP.fetch_add(1, Ordering::SeqCst);
                }
                if action & EFFECT_HOTKEY_DOWN != 0 {
                    EFFECT_HOTKEY_PENDING_DOWN.fetch_add(1, Ordering::SeqCst);
                }
                if action & EFFECT_HOTKEY_TOGGLE != 0 {
                    EFFECT_HOTKEY_PENDING_TOGGLE.fetch_add(1, Ordering::SeqCst);
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

pub(crate) fn consume_effect_hotkeys(player: &mut PlayerIns, state: &mut EffectsState) {
    let toggles = EFFECT_HOTKEY_PENDING_TOGGLE.swap(0, Ordering::SeqCst);
    let ups = EFFECT_HOTKEY_PENDING_UP.swap(0, Ordering::SeqCst);
    let downs = EFFECT_HOTKEY_PENDING_DOWN.swap(0, Ordering::SeqCst);
    let total = toggles + ups + downs;
    if total == 0 {
        return;
    }
    EFFECT_HOTKEY_APPLIED_ACTIONS.fetch_add(total, Ordering::SeqCst);
    for _ in 0..toggles {
        toggle_selected_effect(player, state);
    }
    for _ in 0..ups {
        step_selected_effect(player, state, -1);
    }
    for _ in 0..downs {
        step_selected_effect(player, state, 1);
    }
}

fn toggle_selected_effect(player: &mut PlayerIns, state: &mut EffectsState) {
    if state.effect_hotkeys_effects_on {
        disable_all_calls(player, state);
        state.effect_hotkeys_effects_on = false;
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

fn step_selected_effect(player: &mut PlayerIns, state: &mut EffectsState, delta: isize) {
    let len = state.calls.len();
    if len == 0 {
        state.last_driver_command = Some("effect-hotkey: no effects available".to_owned());
        return;
    }
    let current = state.selected_effect_index.filter(|index| *index < len);
    let next = match (current, delta.is_negative()) {
        (Some(index), false) => (index + 1) % len,
        (Some(index), true) => (index + len - 1) % len,
        (None, false) => 0,
        (None, true) => len - 1,
    };
    enable_only_call(player, state, next, true);
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
    if persist && let Some(call) = state.calls.get(index) {
        let EffectCallKind::SpEffect { id } = call.kind;
        let label = call.kind.label();
        let name = call.name.clone();
        persist_selected_effect(call);
        state.effect_setting_last_id = Some(id);
        state.effect_setting_last_modified = current_effect_setting_modified();
        state.last_driver_command = Some(format!("effect-hotkey: selected {label} ({name})"));
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

    enable_only_call(player, state, index, false);
    state.effect_setting_live_updates = state.effect_setting_live_updates.saturating_add(1);
    if let Some(call) = state.calls.get(index) {
        state.last_driver_command = Some(format!(
            "effect-setting: selected {} ({})",
            call.kind.label(),
            call.name
        ));
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
    let call = state
        .calls
        .get_mut(index)
        .ok_or_else(|| format!("call index {index} out of range"))?;

    call.enabled = enabled;
    if enabled {
        call.kind.apply(player, state.network_sync);
        call.active = call.kind.is_active(player);
        call.active_seen_since_enable = call.active;
        call.apply_failed = !call.active;
        state.selected_effect_index = Some(index);
        state.effect_hotkeys_effects_on = true;
        persist_selected_effect(call);
    } else {
        call.kind.remove(player);
        call.remove_requested = false;
        call.active_seen_since_enable = false;
        call.apply_failed = false;
        call.active = call.kind.is_active(player);
        if state.selected_effect_index == Some(index) {
            state.effect_hotkeys_effects_on = false;
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
    let network_sync = state.network_sync;
    for call in state.calls.iter_mut().filter(|call| call.enabled) {
        call.kind.apply(player, network_sync);
        // The game call reports nothing, so check the active list directly.
        call.active = call.kind.is_active(player);
        call.active_seen_since_enable |= call.active;
        call.apply_failed = !call.active;
    }
}

pub(crate) fn reapply_expired_enabled_calls(player: &mut PlayerIns, state: &mut EffectsState) {
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
