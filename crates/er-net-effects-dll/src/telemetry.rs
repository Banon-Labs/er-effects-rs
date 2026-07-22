use std::{fs, time::Duration};

use crate::{
    config::runtime_config,
    effects::{self, NetEffectsState},
};

pub(crate) fn write_telemetry_throttled(state: &mut NetEffectsState, player_available: bool) {
    const TELEMETRY_INTERVAL: Duration = Duration::from_millis(250);

    let now = std::time::Instant::now();
    if state
        .last_telemetry_write
        .is_some_and(|last_write| now.duration_since(last_write) < TELEMETRY_INTERVAL)
    {
        return;
    }
    state.last_telemetry_write = Some(now);
    write_telemetry(state, player_available);
}

fn write_telemetry(state: &NetEffectsState, player_available: bool) {
    let selected_catalog = state
        .selected_catalog_index
        .and_then(|index| state.catalogs.get(index));
    let selected_call = state
        .selected_effect_index
        .and_then(|index| state.calls.get(index));
    let config = runtime_config();
    let mut body = String::new();
    body.push_str("{\n");
    body.push_str(&format!("  \"player_available\": {player_available},\n"));
    body.push_str(&format!(
        "  \"game_task_ticks\": {},\n",
        state.game_task_ticks
    ));
    body.push_str(&format!("  \"network_sync\": {},\n", state.network_sync));
    body.push_str(&format!(
        "  \"config_path\": \"{}\",\n",
        json_escape(&config.config_path.display().to_string())
    ));
    body.push_str(&format!(
        "  \"config_load_error\": {},\n",
        config.load_error.as_ref().map_or_else(
            || "null".to_owned(),
            |error| format!("\"{}\"", json_escape(error))
        )
    ));
    body.push_str(&format!(
        "  \"effect_hotkey_hook_active\": {},\n  \"effect_hotkey_hook_hits\": {},\n  \"effect_hotkey_applied_actions\": {},\n  \"effect_input_suppressed_keys\": {},\n  \"effect_input_suppressed_arrow_keys\": {},\n  \"effect_dinput_kb_hook_fires\": {},\n  \"effect_dinput_mouse_hook_fires\": {},\n  \"effect_dinput_suppressed_arrow_keys\": {},\n",
        effects::effect_hotkey_hook_active(),
        effects::effect_hotkey_hook_hits(),
        effects::effect_hotkey_applied_actions(),
        effects::effect_input_suppressed_keys(),
        effects::effect_input_suppressed_arrow_keys(),
        effects::dinput_kb_hook_fires(),
        effects::dinput_mouse_hook_fires(),
        effects::dinput_suppressed_arrow_keys()
    ));
    body.push_str(&format!(
        "  \"effect_selector_visible\": {},\n  \"effect_selector_text\": \"{}\",\n",
        state.effect_selector_visible,
        json_escape(&effects::effect_selector_text())
    ));
    body.push_str(&format!(
        "  \"effect_hotkeys_effects_on\": {},\n  \"effect_trigger_hotkey_count\": {},\n  \"effect_trigger_hotkeys_load_error\": {},\n  \"effect_trigger_fire_count\": {},\n  \"effect_trigger_last_key\": {},\n  \"effect_trigger_last_id\": {},\n  \"effect_trigger_last_count\": {},\n",
        state.effect_hotkeys_effects_on,
        state.effect_trigger_hotkeys.len(),
        state.effect_trigger_hotkeys_load_error.as_ref().map_or_else(
            || "null".to_owned(),
            |error| format!("\"{}\"", json_escape(error))
        ),
        state.effect_trigger_fire_count,
        state.effect_trigger_last_key.as_ref().map_or_else(
            || "null".to_owned(),
            |key| format!("\"{}\"", json_escape(key))
        ),
        state
            .effect_trigger_last_id
            .map_or_else(|| "null".to_owned(), |id| id.to_string()),
        state.effect_trigger_last_count
    ));
    body.push_str(&format!(
        "  \"effect_catalog_count\": {},\n  \"effect_catalog_live_updates\": {},\n  \"selected_effect_catalog_index\": {},\n  \"selected_effect_catalog_file\": {},\n  \"selected_effect_catalog_name\": {},\n  \"selected_effect_catalog_size\": {},\n  \"selected_effect_catalog_position\": {},\n  \"selected_effect_index\": {},\n  \"selected_effect_id\": {},\n  \"selected_effect_name\": {},\n  \"selected_effect_status\": {},\n  \"effect_setting_last_id\": {},\n  \"effect_setting_live_updates\": {},\n  \"effect_reapply_count\": {},\n  \"effect_reapply_last_index\": {},\n  \"load_error\": {},\n  \"last_driver_command\": {}\n",
        state.catalogs.len(),
        state.effect_catalog_live_updates,
        state
            .selected_catalog_index
            .map_or_else(|| "null".to_owned(), |index| index.to_string()),
        selected_catalog.map_or_else(
            || "null".to_owned(),
            |catalog| format!("\"{}\"", json_escape(&catalog.file_name))
        ),
        selected_catalog.map_or_else(
            || "null".to_owned(),
            |catalog| format!("\"{}\"", json_escape(&catalog.name))
        ),
        selected_catalog.map_or_else(|| "null".to_owned(), |catalog| catalog.call_indices.len().to_string()),
        effects::selected_catalog_position(state)
            .map(|position| position.saturating_add(1))
            .map_or_else(|| "null".to_owned(), |position| position.to_string()),
        state
            .selected_effect_index
            .map_or_else(|| "null".to_owned(), |index| index.to_string()),
        effects::selected_effect_id(state).map_or_else(|| "null".to_owned(), |id| id.to_string()),
        selected_call.map_or_else(
            || "null".to_owned(),
            |call| format!("\"{}\"", json_escape(&call.name))
        ),
        selected_call.map_or_else(
            || "null".to_owned(),
            |call| format!("\"{}\"", effects::call_status_text(call))
        ),
        state
            .effect_setting_last_id
            .map_or_else(|| "null".to_owned(), |id| id.to_string()),
        state.effect_setting_live_updates,
        state.effect_reapply_count,
        state
            .effect_reapply_last_index
            .map_or_else(|| "null".to_owned(), |index| index.to_string()),
        state.load_error.as_ref().map_or_else(
            || "null".to_owned(),
            |error| format!("\"{}\"", json_escape(error))
        ),
        state.last_driver_command.as_ref().map_or_else(
            || "null".to_owned(),
            |command| format!("\"{}\"", json_escape(command))
        )
    ));
    body.push_str("}\n");

    let tmp_path = config.telemetry_file.with_extension("json.tmp");
    if fs::write(&tmp_path, body).is_ok() {
        let _ = fs::rename(tmp_path, &config.telemetry_file);
    }
}

fn json_escape(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}
