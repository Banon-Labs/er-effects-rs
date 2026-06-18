//! telemetry module (split from lib.rs; pure code reorganization, no behavior change).

#![allow(unused_imports)]

use std::{
    ffi::c_void,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use debug::InputBlocker;
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use hudhook::{
    ImguiRenderLoop, MessageFilter,
    hooks::dx12::ImguiDx12Hooks,
    imgui::{Condition, Context, Ui},
    mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook},
    windows::{
        Win32::{
            Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
            System::{
                LibraryLoader::{GetModuleHandleA, GetProcAddress},
                Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery},
                SystemServices::DLL_PROCESS_ATTACH,
                Threading::GetCurrentProcessId,
            },
            UI::WindowsAndMessaging::{
                EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_KEYDOWN,
                WM_KEYUP,
            },
        },
        core::{BOOL, PCSTR},
    },
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, experiments::*, ffi::*, hooks::*};

pub(crate) fn bootstrap_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_BOOTSTRAP_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("er-effects-bootstrap.jsonl"))
}

pub(crate) fn bootstrap_state_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_BOOTSTRAP_STATE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("er-effects-bootstrap-state.json"))
}

pub(crate) fn write_bootstrap_event(stage: &str, detail: &str) {
    use std::io::Write;

    let event_path = bootstrap_path();
    let state_path = bootstrap_state_path();
    if let Some(parent) = event_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Some(parent) = state_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let payload = format!(
        "{{\"stage\":\"{}\",\"detail\":\"{}\"}}\n",
        json_escape(stage),
        json_escape(detail)
    );
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&event_path)
    {
        let _ = file.write_all(payload.as_bytes());
    }
    let _ = fs::write(state_path, payload);
}

pub(crate) fn write_telemetry_throttled(state: &mut EffectsState, player_available: bool) {
    const TELEMETRY_INTERVAL: Duration = Duration::from_millis(250);

    let now = Instant::now();
    if state
        .last_telemetry_write
        .is_some_and(|last_write| now.duration_since(last_write) < TELEMETRY_INTERVAL)
    {
        return;
    }

    state.last_telemetry_write = Some(now);
    write_telemetry(state, player_available);
}

pub(crate) fn write_telemetry(state: &EffectsState, player_available: bool) {
    if BOOTSTRAP_TELEMETRY_SEEN
        .compare_exchange(
            BOOTSTRAP_TELEMETRY_UNSEEN,
            BOOTSTRAP_TELEMETRY_SEEN_VALUE,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
    {
        write_bootstrap_event(
            BOOTSTRAP_EVENT_TELEMETRY_WRITE,
            if player_available {
                BOOTSTRAP_DETAIL_PLAYER_AVAILABLE
            } else {
                BOOTSTRAP_DETAIL_PLAYER_UNAVAILABLE
            },
        );
    }

    let path = telemetry_path();
    let mut body = String::new();
    body.push_str("{\n");
    body.push_str(&format!("  \"player_available\": {player_available},\n"));
    body.push_str(&format!(
        "  \"current_animation_id\": {},\n",
        state
            .current_animation_id
            .map_or_else(|| "null".to_owned(), |id| id.to_string())
    ));
    body.push_str(&format!("  \"network_sync\": {},\n", state.network_sync));
    body.push_str(&format!(
        "  \"autoload_save_extension\": {},\n",
        state.autoload.save_extension().map_or_else(
            || "null".to_owned(),
            |extension| format!("\"{}\"", json_escape(extension))
        )
    ));
    body.push_str(&format!(
        "  \"autoload_slot\": {},\n",
        state
            .autoload
            .slot()
            .map_or_else(|| "null".to_owned(), |slot| slot.to_string())
    ));
    body.push_str(&format!(
        "  \"autoload_method\": \"{}\",\n",
        state.autoload.method().label()
    ));
    body.push_str(&format!(
        "  \"autoload_require_title_bootstrap\": {},\n",
        state.autoload.requires_title_bootstrap()
    ));
    body.push_str(&format!(
        "  \"title_bootstrap_seen\": {},\n",
        TITLE_BOOTSTRAP_SEEN.load(Ordering::SeqCst) != TITLE_BOOTSTRAP_UNSEEN
    ));
    body.push_str(&format!(
        "  \"autoload_attempts\": {},\n",
        state.autoload.attempts()
    ));
    body.push_str(&format!(
        "  \"game_task_ticks\": {},\n",
        state.game_task_ticks
    ));
    body.push_str(&format!(
        "  \"safe_input_confirm_count\": {},\n",
        state.safe_input.confirm_count
    ));
    body.push_str(&format!(
        "  \"safe_input_pulses_sent\": {},\n",
        state.safe_input.pulses_sent
    ));
    body.push_str(&format!(
        "  \"safe_input_hooks_requested\": {},\n",
        state.safe_input.hooks_requested
    ));
    body.push_str(&format!(
        "  \"safe_input_hook_frames_remaining\": {},\n",
        SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst)
    ));
    body.push_str(&format!(
        "  \"safe_input_last_status\": {},\n",
        state.safe_input.last_status.as_ref().map_or_else(
            || "null".to_owned(),
            |status| format!("\"{}\"", json_escape(status))
        )
    ));
    body.push_str(&format!(
        "  \"autoload_last_status\": {},\n",
        state.autoload.last_status().map_or_else(
            || "null".to_owned(),
            |status| format!("\"{}\"", json_escape(status))
        )
    ));
    write_game_man_telemetry(&mut body);
    write_save_data_snapshot_telemetry(&mut body);
    body.push_str(&format!(
        "  \"last_driver_command\": {},\n",
        state.last_driver_command.as_ref().map_or_else(
            || "null".to_owned(),
            |command| format!("\"{}\"", json_escape(command))
        )
    ));
    body.push_str("  \"calls\": [\n");
    for (index, call) in state.calls.iter().enumerate() {
        let comma = if index + NEXT_INDEX_OFFSET == state.calls.len() {
            ""
        } else {
            ","
        };
        body.push_str(&format!(
            "    {{\"index\": {index}, \"name\": \"{}\", \"kind\": \"{}\", \"enabled\": {}, \"active\": {}, \"apply_failed\": {}}}{comma}\n",
            json_escape(&call.name),
            json_escape(&call.kind.label()),
            call.enabled,
            call.active,
            call.apply_failed,
        ));
    }
    body.push_str("  ]\n}\n");

    let tmp_path = path.with_extension("json.tmp");
    if fs::write(&tmp_path, body).is_ok() {
        let _ = fs::rename(tmp_path, path);
    }
}

pub(crate) fn write_game_man_telemetry(body: &mut String) {
    let Ok(game_man) = (unsafe { GameMan::instance() }) else {
        body.push_str("  \"game_man_available\": false,\n");
        return;
    };

    let telemetry = GameManTelemetry::from_game_man(game_man);
    body.push_str("  \"game_man_available\": true,\n");
    body.push_str(&format!("  \"game_save_slot\": {},\n", telemetry.save_slot));
    body.push_str(&format!(
        "  \"game_requested_save_slot_load_index\": {},\n",
        telemetry.requested_save_slot_load_index
    ));
    body.push_str(&format!(
        "  \"game_save_state\": {},\n",
        telemetry.save_state
    ));
    body.push_str(&format!(
        "  \"game_save_requested\": {},\n",
        telemetry.save_requested
    ));
}

/// Read-only, save-safe save-data snapshot for the parked-title disambiguation
/// (goal step 2): confirm GameDataMan (`SLOT_MANAGER_RVA`) and its `CS::ProfileSummary`
/// container (`+SLOT_MANAGER_CONTAINER_OFFSET`) are built cold, read the per-slot
/// active bytes the char-mount gate (`0x67b200`) checks via `byte[profile+slot+8]`,
/// and read the save-mgr deserialize-ready handle (`[mgr+0xdf0]`, the gate fast-path).
/// Every access is a fault-tolerant `ReadProcessMemory` -- no game-state mutation.
pub(crate) fn write_save_data_snapshot_telemetry(body: &mut String) {
    /// Null pointer sentinel for the chased singleton reads.
    const NULL_POINTER_VALUE: usize = 0;
    /// ProfileSummary per-slot active-byte array base (getter reads `byte[profile+slot+8]`).
    const PROFILE_SLOT_ACTIVE_ARRAY_OFFSET: usize = 0x8;
    /// Save-mgr deserialize-ready handle (gate `0x67b200` fast-path `[mgr+0xdf0]`).
    const GAME_MAN_DESERIALIZE_READY_DF0_OFFSET: usize = 0xdf0;

    let Ok(base) = crate::experiments::game_module_base() else {
        body.push_str("  \"save_snapshot_available\": false,\n");
        return;
    };

    let game_data_man =
        unsafe { crate::experiments::safe_read_usize(base + crate::SLOT_MANAGER_RVA) }
            .unwrap_or(NULL_POINTER_VALUE);
    let profile_summary = if game_data_man == NULL_POINTER_VALUE {
        NULL_POINTER_VALUE
    } else {
        unsafe {
            crate::experiments::safe_read_usize(
                game_data_man + crate::SLOT_MANAGER_CONTAINER_OFFSET,
            )
        }
        .unwrap_or(NULL_POINTER_VALUE)
    };
    let slot_active_bytes = if profile_summary == NULL_POINTER_VALUE {
        None
    } else {
        unsafe {
            crate::experiments::safe_read_usize(profile_summary + PROFILE_SLOT_ACTIVE_ARRAY_OFFSET)
        }
    };
    let save_mgr = unsafe {
        crate::experiments::safe_read_usize(base + crate::FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA)
    }
    .unwrap_or(NULL_POINTER_VALUE);
    let deserialize_ready = if save_mgr == NULL_POINTER_VALUE {
        None
    } else {
        unsafe {
            crate::experiments::safe_read_usize(save_mgr + GAME_MAN_DESERIALIZE_READY_DF0_OFFSET)
        }
    };

    body.push_str("  \"save_snapshot_available\": true,\n");
    body.push_str(&format!(
        "  \"game_data_man_present\": {},\n",
        game_data_man != NULL_POINTER_VALUE
    ));
    body.push_str(&format!(
        "  \"profile_summary_present\": {},\n",
        profile_summary != NULL_POINTER_VALUE
    ));
    body.push_str(&format!(
        "  \"profile_slot_active_bytes_qword\": {},\n",
        slot_active_bytes.map_or_else(|| "null".to_owned(), |value| format!("\"{value:#x}\""))
    ));
    body.push_str(&format!(
        "  \"game_save_deserialize_ready_df0\": {},\n",
        deserialize_ready.map_or_else(|| "null".to_owned(), |value| format!("\"{value:#x}\""))
    ));
}

pub(crate) fn telemetry_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_TELEMETRY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("er-effects-telemetry.json"))
}

pub(crate) fn command_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_COMMAND_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("er-effects-command.txt"))
}

pub(crate) fn json_escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\t' => "\\t".chars().collect::<Vec<_>>(),
            character => vec![character],
        })
        .collect()
}

pub(crate) fn crash_log_path() -> PathBuf {
    std::env::var("ER_EFFECTS_CRASH_LOG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            game_directory_path()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("er-effects-crash.log")
        })
}

pub(crate) fn append_crash_log(args: std::fmt::Arguments<'_>) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(crash_log_path())
    {
        let _ = writeln!(file, "{args}");
    }
}

pub(crate) fn append_autoload_debug(args: std::fmt::Arguments<'_>) {
    use std::io::Write;

    let path = std::env::var("ER_EFFECTS_AUTOLOAD_DEBUG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("er-effects-autoload-debug.log"));
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{args}");
    }
}

/// Wall-clock epoch for the load-timeline markers. Lazily set on the FIRST `timeline_event`
/// call (which is T0 by construction -- the first frame the title is parked at state 10),
/// so every subsequent `ms=` is measured from that common start. `Instant` is QPC-backed on
/// the windows target and works under wine, so no new FFI is needed.
static TIMELINE_EPOCH: Mutex<Option<Instant>> = Mutex::new(None);

/// Emit a frame-stamped load-timeline marker so one parser handles BOTH a native-menu load
/// (observe mode) and a DLL-driven load (own-stepper). Format (greppable, single regex):
///   `EVENT <name> frame=<n> ms=<elapsed-from-T0> <fields>`
/// `frame` is the monotonic per-frame `game_task_ticks`; `ms` is wall-clock from the first
/// event. Edge-triggering (fire each marker once) is the caller's responsibility.
pub(crate) fn timeline_event(name: &str, frame: u64, fields: std::fmt::Arguments<'_>) {
    let ms = {
        let mut guard = match TIMELINE_EPOCH.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let epoch = guard.get_or_insert_with(Instant::now);
        epoch.elapsed().as_millis()
    };
    append_autoload_debug(format_args!("EVENT {name} frame={frame} ms={ms} {fields}"));
}

pub(crate) fn trace_continue_default_path() -> PathBuf {
    game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-trace-continue.txt")
}

pub(crate) fn continue_trace_log_path() -> PathBuf {
    std::env::var("ER_EFFECTS_TRACE_CONTINUE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            game_directory_path()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("er-effects-continue-trace.log")
        })
}

pub(crate) fn game_directory_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
}

pub(crate) fn append_continue_trace(args: std::fmt::Arguments<'_>) {
    use std::io::Write;

    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(continue_trace_log_path())
    {
        let _ = writeln!(file, "{args}");
    }
}
