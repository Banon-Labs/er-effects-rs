use std::fs;

use eldenring::cs::{ChrInsExt, PlayerIns};
use er_effects_data::{EffectCallSpec, EffectKindSpec};

use crate::{command_path, EffectsState};

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
            apply_failed: false,
        }
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
        call.apply_failed = !call.active;
    } else {
        call.kind.remove(player);
        call.remove_requested = false;
        call.apply_failed = false;
        call.active = call.kind.is_active(player);
    }

    Ok(())
}

pub(crate) fn remove_requested_calls(player: &mut PlayerIns, state: &mut EffectsState) {
    if state.remove_all_requested {
        for call in &mut state.calls {
            call.kind.remove(player);
            call.remove_requested = false;
            call.apply_failed = false;
        }
        state.remove_all_requested = false;
        return;
    }

    for call in &mut state.calls {
        if call.remove_requested {
            call.kind.remove(player);
            call.remove_requested = false;
            call.apply_failed = false;
        }
    }
}

pub(crate) fn apply_selected_calls(player: &mut PlayerIns, state: &mut EffectsState) {
    let network_sync = state.network_sync;
    for call in state.calls.iter_mut().filter(|call| call.enabled) {
        call.kind.apply(player, network_sync);
        // The game call reports nothing, so check the active list directly.
        call.apply_failed = !call.kind.is_active(player);
    }
}

pub(crate) fn refresh_call_status(player: &PlayerIns, state: &mut EffectsState) {
    for call in &mut state.calls {
        call.active = call.kind.is_active(player);
        if call.active {
            call.apply_failed = false;
        }
    }
}
