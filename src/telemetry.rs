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

#[repr(C)]
pub(crate) struct NowLoadingHelperLayout {
    pub(crate) unknown_000: [u8; 0xed],
    pub(crate) loading_flag: u8,
}

#[repr(C)]
pub(crate) struct GameManSaveSnapshotLayout {
    pub(crate) unknown_000: [u8; 0xdf0],
    pub(crate) deserialize_ready: usize,
}

#[repr(C)]
pub(crate) struct IoDeviceSnapshotLayout {
    pub(crate) unknown_000: [u8; 0x10],
    pub(crate) inflight: usize,
    pub(crate) unknown_18: [u8; 0x08],
    pub(crate) request_handle: usize,
}

const SEAMLESS_COOP_MODULE_NAME: &[u8] = b"ersc.dll\0";
const SEAMLESS_COOP_MARKER: &str = "ersc.dll";
const RUNTIME_MODE_SEAMLESS: &str = "seamless";
const RUNTIME_MODE_VANILLA_OR_UNKNOWN: &str = "vanilla_or_unknown";

pub(crate) fn seamless_coop_loaded() -> bool {
    matches!(
        unsafe { GetModuleHandleA(PCSTR(SEAMLESS_COOP_MODULE_NAME.as_ptr())) },
        Ok(module) if module.0 as usize != 0
    )
}

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

fn title_logo_gfx_alpha_for_frame(frame: i32) -> i32 {
    match frame {
        TITLE_LOGO_GFX_UNKNOWN_FRAME => TITLE_LOGO_GFX_UNKNOWN_ALPHA,
        // Disk correlation: `target/autoresearch/gfx-analysis/script-smoke/summary.json` for
        // `05_001_title_logo.gfx` shows root depth 3 is placed at frame 1 with no color transform,
        // then moved by FadeIn frames 2..60 using alphaMultTerm 0..256, remains full through
        // Title_TopMenu/FadeOut frame 113, and fades to 0 by frame 133. The in-memory oracle reads
        // the live Scaleform current frame through `FUN_140d82620`, so convert that frame back into
        // the on-disk alpha term instead of treating the entire ramp as a generic visible boolean.
        1 => TITLE_LOGO_GFX_FULL_ALPHA,
        2..=60 => ((frame - 2) * TITLE_LOGO_GFX_FULL_ALPHA + 29) / 58,
        61..=113 => TITLE_LOGO_GFX_FULL_ALPHA,
        114..=133 => ((133 - frame) * TITLE_LOGO_GFX_FULL_ALPHA + 10) / 20,
        _ => TITLE_LOGO_GFX_UNKNOWN_ALPHA,
    }
}

unsafe fn title_logo_gfx_current_frame(base: usize, title_logo_back_view_parts: usize) -> i32 {
    if title_logo_back_view_parts == TITLE_OWNER_SCAN_START_ADDRESS
        || title_logo_back_view_parts == 0
    {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    }
    let gfx_value = title_logo_back_view_parts + TITLE_LOGO_GFX_VALUE_88_OFFSET;
    let Some(handle) = (unsafe { crate::experiments::safe_read_usize(gfx_value) }) else {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    };
    if handle == 0 || handle == TITLE_OWNER_SCAN_START_ADDRESS {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    }
    let Some(vtable) = (unsafe { crate::experiments::safe_read_usize(handle) }) else {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    };
    if vtable == 0 || vtable == TITLE_OWNER_SCAN_START_ADDRESS {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    }
    let Some(resolve_value_addr) = (unsafe { crate::experiments::safe_read_usize(vtable + 0x8) })
    else {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    };
    if resolve_value_addr == 0 || resolve_value_addr == TITLE_OWNER_SCAN_START_ADDRESS {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    }
    // Mirrors native helpers at 0x140749980/0x1407499e0: load *(gfx_value) into rcx, call vtable+8,
    // then pass the resolved Scaleform value to FUN_140d82620 to read the current 1-based frame.
    let resolve_value: unsafe extern "system" fn(usize) -> usize =
        unsafe { std::mem::transmute(resolve_value_addr) };
    let value = unsafe { resolve_value(handle) };
    if value == 0 || value == TITLE_OWNER_SCAN_START_ADDRESS {
        return TITLE_LOGO_GFX_UNKNOWN_FRAME;
    }
    let current_frame: unsafe extern "system" fn(usize) -> i32 =
        unsafe { std::mem::transmute(base + TITLE_LOGO_GFX_CURRENT_FRAME_RVA) };
    unsafe { current_frame(value) }
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

    let player_seen =
        player_available || IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES;
    let path = telemetry_path();
    let mut body = String::new();
    let seamless_loaded = seamless_coop_loaded();
    let runtime_mode = if seamless_loaded {
        RUNTIME_MODE_SEAMLESS
    } else {
        RUNTIME_MODE_VANILLA_OR_UNKNOWN
    };
    body.push_str("{\n");
    body.push_str(&format!("  \"player_available\": {player_available},\n"));
    body.push_str(&format!("  \"player_seen\": {player_seen},\n"));
    body.push_str(&format!("  \"runtime_mode\": \"{runtime_mode}\",\n"));
    body.push_str(&format!("  \"seamless_coop_loaded\": {seamless_loaded},\n"));
    body.push_str(&format!(
        "  \"seamless_coop_marker\": {},\n",
        if seamless_loaded {
            format!("\"{}\"", json_escape(SEAMLESS_COOP_MARKER))
        } else {
            "null".to_owned()
        }
    ));
    body.push_str(&format!(
        "  \"current_animation_id\": {},\n",
        state
            .current_animation_id
            .map_or_else(|| "null".to_owned(), |id| id.to_string())
    ));
    body.push_str(&format!(
        "  \"expected_animation_seen\": {},\n",
        state.expected_animation_seen
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
        "  \"title_handoff_complete\": {},\n",
        TITLE_HANDOFF_COMPLETE.load(Ordering::SeqCst) != TITLE_HANDOFF_INCOMPLETE
    ));
    // Cold-char-mount progress as phase+1 (0 = never ran, 5 = PHASE_DONE = terminal/evidence
    // collected). The readiness watcher tears down on the terminal value instead of the cap.
    body.push_str(&format!(
        "  \"oracle_cold_char_mount_phase\": {},\n",
        crate::experiments::COLD_CHAR_MOUNT_PHASE_PUB.load(Ordering::SeqCst)
    ));
    // OWN-LOAD verify-only probe progress as phase+1 (0 = never ran, 2 = PHASE_DONE = terminal,
    // evidence collected). The readiness watcher tears down on the terminal value, not the cap.
    body.push_str(&format!(
        "  \"oracle_own_load_phase\": {},\n",
        crate::experiments::OWN_LOAD_PHASE_PUB.load(Ordering::SeqCst)
    ));
    // OWN-LOAD per-frame world-stream stall telemetry (own-load-reaches-loading-screen-2026-06-22 /
    // full-pipeline-traced-to-worldreswait-map-block-streaming). After own_load_continue fires
    // continue_confirm/SetState5 the engine reaches the real-char LOADING SCREEN but STALLS; these
    // mirror the deepest world-load pump values so the readiness watcher / agent can see whether ANY
    // advances (progress) or all are frozen (genuine stall). UNREAD sentinel -> JSON null (the chain
    // pointer was null / RPM faulted, distinct from a real 0). All hex except the count fields.
    let fmt_stream = |v: i64, hex: bool| -> String {
        if v == crate::experiments::OWN_LOAD_STREAM_FIELD_UNREAD {
            "null".to_owned()
        } else if hex {
            format!("\"{v:#x}\"")
        } else {
            v.to_string()
        }
    };
    body.push_str(&format!(
        "  \"oracle_own_load_stream_frames\": {},\n  \"oracle_own_load_stream_recur_frames\": {},\n  \"oracle_own_load_continue_fired\": {},\n  \"oracle_own_load_stream_owner_state\": {},\n  \"oracle_own_load_stream_owner_req_state\": {},\n  \"oracle_own_load_stream_mms_state\": {},\n  \"oracle_own_load_stream_block_count\": {},\n  \"oracle_own_load_stream_req_coord\": {},\n  \"oracle_own_load_stream_io_inflight\": {},\n  \"oracle_own_load_stream_io_reqhandle\": {},\n  \"oracle_own_load_stream_c30\": {},\n  \"oracle_own_load_stream_player_present\": {},\n  \"oracle_own_load_ingame_phase\": {},\n  \"oracle_own_load_req_blockid\": {},\n  \"oracle_own_load_target_block_present\": {},\n  \"oracle_own_load_wbr_update_calls\": {},\n  \"oracle_own_load_wbr_max_phase\": {},\n  \"oracle_own_load_wbr_any_gate_set\": {},\n  \"oracle_own_m28_dispatch_fired\": {},\n  \"oracle_own_load_install_job_fired\": {},\n  \"oracle_own_load_pump_fired\": {},\n  \"oracle_own_load_pump_state\": {},\n  \"oracle_own_load_pump_subcode\": {},\n  \"oracle_own_load_pump_done\": {},\n",
        crate::experiments::OWN_LOAD_STREAM_FRAMES.load(Ordering::SeqCst),
        crate::experiments::OWN_LOAD_STREAM_RECUR_FRAMES.load(Ordering::SeqCst),
        crate::experiments::OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_OWNER_STATE.load(Ordering::SeqCst),
            false
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_OWNER_REQ_STATE.load(Ordering::SeqCst),
            false
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_MMS_STATE.load(Ordering::SeqCst),
            false
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_BLOCK_COUNT.load(Ordering::SeqCst),
            false
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_REQ_COORD.load(Ordering::SeqCst),
            true
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_IO_INFLIGHT.load(Ordering::SeqCst),
            true
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_IO_REQHANDLE.load(Ordering::SeqCst),
            true
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_C30.load(Ordering::SeqCst),
            true
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_PLAYER_PRESENT.load(Ordering::SeqCst),
            false
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_INGAME_PHASE.load(Ordering::SeqCst),
            false
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_REQ_BLOCKID.load(Ordering::SeqCst),
            true
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_STREAM_TARGET_BLOCK_PRESENT.load(Ordering::SeqCst),
            false
        ),
        crate::experiments::OWN_LOAD_WBR_UPDATE_CALLS.load(Ordering::SeqCst),
        fmt_stream(
            crate::experiments::OWN_LOAD_WBR_MAX_PHASE.load(Ordering::SeqCst) as i64,
            true
        ),
        crate::experiments::OWN_LOAD_WBR_ANY_GATE_SET.load(Ordering::SeqCst),
        crate::experiments::OWN_LOAD_M28_DISPATCH_FIRED.load(Ordering::SeqCst),
        crate::experiments::OWN_LOAD_INSTALL_JOB_FIRED.load(Ordering::SeqCst),
        crate::experiments::OWN_LOAD_PUMP_FIRED.load(Ordering::SeqCst),
        fmt_stream(
            crate::experiments::OWN_LOAD_PUMP_STATE.load(Ordering::SeqCst),
            false
        ),
        fmt_stream(
            crate::experiments::OWN_LOAD_PUMP_SUBCODE.load(Ordering::SeqCst),
            false
        ),
        crate::experiments::OWN_LOAD_PUMP_DONE.load(Ordering::SeqCst),
    ));
    let product_core_blocker = PRODUCT_CORE_LAST_BLOCKER.load(Ordering::SeqCst);
    let format_scan_ptr = |value: usize| -> String {
        if value == TITLE_OWNER_SCAN_START_ADDRESS {
            "null".to_owned()
        } else {
            format!("\"0x{value:x}\"")
        }
    };
    let title_owner_state_bits = TITLE_OWNER_SCAN_LAST_STATE_BITS.load(Ordering::SeqCst);
    body.push_str(&format!(
        "  \"product_autoload_armed\": {},\n  \"product_core_autoload_ticks\": {},\n  \"product_core_ready_blocks\": {},\n  \"product_core_ready_successes\": {},\n  \"product_core_owner_ticks\": {},\n  \"product_core_last_owner\": {},\n  \"product_core_last_title_dialog\": {},\n  \"product_core_last_title_dialog_vt\": {},\n  \"product_core_last_title_in_loop\": {},\n  \"product_core_last_title_in_textfadeout\": {},\n  \"product_core_last_menu_opened_latch\": {},\n  \"product_core_last_press_start_proxy\": {},\n  \"product_core_last_press_start_vt\": {},\n  \"product_core_last_press_start_context\": {},\n  \"product_core_last_phase\": {},\n  \"product_core_ready_blocker\": \"{}\",\n  \"title_owner_scan_attempts\": {},\n  \"title_owner_scan_vtable_hits\": {},\n  \"title_owner_scan_table_rejects\": {},\n  \"title_owner_scan_state_rejects\": {},\n  \"title_owner_scan_cached_owner\": {},\n  \"title_owner_scan_last_candidate\": {},\n  \"title_owner_scan_last_table\": {},\n  \"title_owner_scan_last_state\": {},\n",
        product_autoload_enabled(),
        PRODUCT_CORE_AUTOLOAD_TICKS.load(Ordering::SeqCst),
        PRODUCT_CORE_READY_BLOCKS.load(Ordering::SeqCst),
        PRODUCT_CORE_READY_SUCCESSES.load(Ordering::SeqCst),
        PRODUCT_CORE_OWNER_TICKS.load(Ordering::SeqCst),
        format_scan_ptr(PRODUCT_CORE_LAST_OWNER.load(Ordering::SeqCst)),
        format_scan_ptr(PRODUCT_CORE_LAST_TITLE_DIALOG.load(Ordering::SeqCst)),
        format_scan_ptr(PRODUCT_CORE_LAST_TITLE_DIALOG_VT.load(Ordering::SeqCst)),
        PRODUCT_CORE_LAST_TITLE_IN_LOOP.load(Ordering::SeqCst) != 0,
        PRODUCT_CORE_LAST_TITLE_IN_TEXTFADEOUT.load(Ordering::SeqCst) != 0,
        PRODUCT_CORE_LAST_MENU_OPENED_LATCH.load(Ordering::SeqCst),
        format_scan_ptr(PRODUCT_CORE_LAST_PRESS_START_PROXY.load(Ordering::SeqCst)),
        format_scan_ptr(PRODUCT_CORE_LAST_PRESS_START_VT.load(Ordering::SeqCst)),
        format_scan_ptr(PRODUCT_CORE_LAST_PRESS_START_CONTEXT.load(Ordering::SeqCst)),
        PRODUCT_CORE_LAST_PHASE.load(Ordering::SeqCst),
        json_escape(product_core_ready_blocker_label(product_core_blocker)),
        TITLE_OWNER_SCAN_ATTEMPTS.load(Ordering::SeqCst),
        TITLE_OWNER_SCAN_VTABLE_HITS.load(Ordering::SeqCst),
        TITLE_OWNER_SCAN_TABLE_REJECTS.load(Ordering::SeqCst),
        TITLE_OWNER_SCAN_STATE_REJECTS.load(Ordering::SeqCst),
        format_scan_ptr(TITLE_OWNER_PTR.load(Ordering::SeqCst)),
        format_scan_ptr(TITLE_OWNER_SCAN_LAST_CANDIDATE.load(Ordering::SeqCst)),
        format_scan_ptr(TITLE_OWNER_SCAN_LAST_TABLE.load(Ordering::SeqCst)),
        if title_owner_state_bits == usize::MAX {
            "null".to_owned()
        } else {
            (title_owner_state_bits as u32 as i32).to_string()
        }
    ));
    body.push_str(&format!(
        "  \"autoload_attempts\": {},\n",
        state.autoload.attempts()
    ));
    body.push_str(&format!(
        "  \"game_task_ticks\": {},\n",
        state.game_task_ticks
    ));
    write_oracle_telemetry(&mut body);
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
    // `loadgame_build_ctx_ready`: the "engine filled enough to drive our own load" gate -- GameDataMan
    // -> menuSystemSaveLoad -> a PLAUSIBLE TitleFlowContext at mss+0xa38. This is the gate the bypass
    // arms on. It is DISTINCT from `game_man_instance_resolved` below, which only means the GameMan
    // pointer is non-null (true from BootPhase4, long before the LoadGame job can be built without an AV).
    // Computed independently of GameMan::instance() so it is always emitted (both branches below).
    let loadgame_build_ctx_ready = crate::experiments::game_module_base()
        .map(|base| unsafe { crate::experiments::loadgame_build_ctx_ready(base) })
        .unwrap_or(false);
    body.push_str(&format!(
        "  \"loadgame_build_ctx_ready\": {loadgame_build_ctx_ready},\n"
    ));

    let Ok(game_man) = (unsafe { GameMan::instance() }) else {
        body.push_str("  \"game_man_instance_resolved\": false,\n");
        return;
    };

    let telemetry = GameManTelemetry::from_game_man(game_man);
    body.push_str("  \"game_man_instance_resolved\": true,\n");
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

/// ORACLE reads for the proof bundle (per the goal): the LIVE in-world facts the harness asserts
/// on, independent of any agent narrative. Re-fetches the local player (the lib.rs player borrow
/// has ended before this runs). For a ZERO-INPUT run, `simulated_button_presses_total` MUST be 0;
/// `oracle_grounded` + a valid `oracle_block_id` + finite non-origin `oracle_havok_pos`
/// distinguish "in the playable world" from "frozen on a loading screen".
pub(crate) fn write_oracle_telemetry(body: &mut String) {
    const BLOCK_ID_NONE: i32 = -1;
    const GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET: usize = core::mem::offset_of!(GameMan, save_state);
    const GAME_MAN_SAVED_MAP_C30_OFFSET: usize =
        core::mem::offset_of!(GameMan, stay_in_multiplay_area_saved_rotation)
            + core::mem::size_of::<fromsoftware_shared::F32Vector4>()
            + core::mem::size_of::<fromsoftware_shared::F32Vector4>();
    const READ_FAIL_SENTINEL: i32 = -1;
    body.push_str(&format!(
        "  \"simulated_button_presses_total\": {},\n",
        crate::hooks::SIMULATED_INPUT_PRESSES_TOTAL.load(Ordering::SeqCst)
    ));
    let continue_task_node = MENU_CONTINUE_TASK_NODE.load(Ordering::SeqCst);
    let continue_member_node = MENU_CONTINUE_MEMBER_NODE.load(Ordering::SeqCst);
    let format_optional_ptr = |value: usize| -> String {
        if value == TITLE_OWNER_SCAN_START_ADDRESS {
            "null".to_owned()
        } else {
            format!("\"0x{value:x}\"")
        }
    };
    body.push_str(&format!(
        "  \"oracle_continue_task_node\": {},\n  \"oracle_continue_member_node\": {},\n  \"oracle_menu_window_ctor_hits\": {},\n  \"oracle_menu_window_ctor_semantic_hits\": {},\n  \"oracle_menu_window_ctor_last_item\": {},\n  \"oracle_menu_window_ctor_last_vt\": {},\n  \"oracle_menu_window_ctor_last_functor\": {},\n  \"oracle_menu_window_ctor_last_docall\": {},\n  \"oracle_menu_window_ctor_last_accept\": {},\n  \"oracle_menu_window_native_ctor_b_hits\": {},\n  \"oracle_menu_window_native_ctor_b_continue_hits\": {},\n  \"oracle_menu_window_native_ctor_b_last_caller_rva\": {},\n  \"oracle_menu_window_native_ctor_b_last_item\": {},\n  \"oracle_menu_window_native_ctor_b_last_out_slot\": {},\n  \"oracle_menu_window_native_ctor_b_last_vt\": {},\n  \"oracle_menu_window_native_ctor_b_last_functor\": {},\n  \"oracle_menu_window_native_ctor_b_last_docall\": {},\n  \"oracle_menu_window_native_ctor_b_last_accept\": {},\n  \"oracle_menu_window_idle_ctor_hits\": {},\n  \"oracle_menu_window_idle_ctor_continue_hits\": {},\n  \"oracle_menu_window_idle_ctor_continue_last_caller_rva\": {},\n  \"oracle_menu_window_idle_ctor_continue_last_item\": {},\n  \"oracle_menu_window_idle_ctor_continue_last_out_slot\": {},\n  \"oracle_menu_window_idle_ctor_continue_last_docall\": {},\n  \"oracle_menu_window_idle_ctor_continue_last_accept\": {},\n  \"oracle_menu_continue_idle_insert_hits\": {},\n  \"oracle_menu_continue_idle_insert_last_caller_rva\": {},\n  \"oracle_menu_continue_idle_insert_last_arg0\": {},\n  \"oracle_menu_continue_idle_insert_last_arg1\": {},\n  \"oracle_menu_continue_idle_insert_last_ret\": {},\n  \"oracle_menu_continue_idle_insert_last_arg1_update_rva\": {},\n  \"oracle_menu_continue_idle_insert_last_ret_update_rva\": {},\n  \"oracle_task_enqueue_generic_hits\": {},\n  \"oracle_task_enqueue_generic_last_caller_rva\": {},\n  \"oracle_task_enqueue_generic_last_arg0\": {},\n  \"oracle_task_enqueue_generic_last_arg0_pointee\": {},\n  \"oracle_task_enqueue_generic_last_arg1\": {},\n  \"oracle_task_enqueue_generic_last_ret\": {},\n  \"oracle_task_enqueue_generic_sample0_caller_rva\": {},\n  \"oracle_task_enqueue_generic_sample0_arg0\": {},\n  \"oracle_task_enqueue_generic_sample0_arg0_pointee\": {},\n  \"oracle_task_enqueue_generic_sample0_arg1\": {},\n  \"oracle_task_enqueue_generic_sample0_ret\": {},\n  \"oracle_task_enqueue_generic_sample1_caller_rva\": {},\n  \"oracle_task_enqueue_generic_sample1_arg0\": {},\n  \"oracle_task_enqueue_generic_sample1_arg0_pointee\": {},\n  \"oracle_task_enqueue_generic_sample1_arg1\": {},\n  \"oracle_task_enqueue_generic_sample1_ret\": {},\n  \"oracle_task_enqueue_generic_idle_item_match_hits\": {},\n  \"oracle_task_enqueue_generic_idle_item_last_match_kind\": {},\n  \"oracle_menu_window_idle_ctor_last_caller_rva\": {},\n  \"oracle_menu_window_idle_ctor_last_item\": {},\n  \"oracle_menu_window_idle_ctor_last_vt\": {},\n  \"oracle_menu_window_idle_ctor_last_functor\": {},\n  \"oracle_menu_window_idle_ctor_last_docall\": {},\n  \"oracle_menu_window_idle_ctor_last_accept\": {},\n  \"oracle_menu_item_update_hits\": {},\n  \"oracle_menu_item_update_semantic_hits\": {},\n  \"oracle_menu_item_update_last_item\": {},\n  \"oracle_menu_item_update_last_vt\": {},\n  \"oracle_menu_item_update_last_functor\": {},\n  \"oracle_menu_item_update_last_docall\": {},\n  \"oracle_menu_item_update_last_accept\": {},\n  \"oracle_menu_continue_candidate_item\": {},\n  \"oracle_menu_continue_candidate_hits\": {},\n  \"oracle_menu_continue_candidate_idle_accept_hits\": {},\n  \"oracle_menu_continue_candidate_native_accept_hits\": {},\n  \"oracle_menu_continue_candidate_other_accept_hits\": {},\n  \"oracle_menu_continue_candidate_accept_changes\": {},\n  \"oracle_menu_continue_candidate_last_accept\": {},\n  \"oracle_title_native_ready_hits\": {},\n  \"oracle_title_native_ready_last_caller_rva\": {},\n  \"oracle_title_native_ready_last_this\": {},\n  \"oracle_title_native_ready_last_vtable\": {},\n  \"oracle_title_native_ready_last_getter\": {},\n  \"oracle_title_native_ready_last_object\": {},\n  \"oracle_title_native_ready_last_flags\": {},\n  \"oracle_title_native_ready_last_masked\": {},\n  \"oracle_title_native_ready_last_ret\": {},\n  \"oracle_title_langselect_ready_last_object\": {},\n  \"oracle_title_langselect_ready_last_flags\": {},\n  \"oracle_title_langselect_ready_last_masked\": {},\n  \"oracle_title_langselect_ready_last_ret\": {},\n",
        format_optional_ptr(continue_task_node),
        format_optional_ptr(continue_member_node),
        MENU_WINDOW_JOB_CTOR_HITS.load(Ordering::SeqCst),
        MENU_WINDOW_JOB_CTOR_SEMANTIC_HITS.load(Ordering::SeqCst),
        format_optional_ptr(MENU_WINDOW_JOB_CTOR_LAST_ITEM.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_CTOR_LAST_VT.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_CTOR_LAST_FUNCTOR.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_CTOR_LAST_DOCALL.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_CTOR_LAST_ACCEPT.load(Ordering::SeqCst)),
        MENU_WINDOW_JOB_NATIVE_CTOR_B_HITS.load(Ordering::SeqCst),
        MENU_WINDOW_JOB_NATIVE_CTOR_B_CONTINUE_HITS.load(Ordering::SeqCst),
        format_optional_ptr(MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_ITEM.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_OUT_SLOT.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_VT.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_FUNCTOR.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_DOCALL.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_ACCEPT.load(Ordering::SeqCst)),
        MENU_WINDOW_JOB_IDLE_CTOR_HITS.load(Ordering::SeqCst),
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_HITS.load(Ordering::SeqCst),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_ITEM.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_OUT_SLOT.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_DOCALL.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_ACCEPT.load(Ordering::SeqCst)),
        MENU_CONTINUE_IDLE_INSERT_HITS.load(Ordering::SeqCst),
        format_optional_ptr(MENU_CONTINUE_IDLE_INSERT_LAST_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_CONTINUE_IDLE_INSERT_LAST_ARG0.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_CONTINUE_IDLE_INSERT_LAST_ARG1.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_CONTINUE_IDLE_INSERT_LAST_RET.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_CONTINUE_IDLE_INSERT_LAST_ARG1_UPDATE_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_CONTINUE_IDLE_INSERT_LAST_RET_UPDATE_RVA.load(Ordering::SeqCst)),
        TASK_ENQUEUE_GENERIC_HITS.load(Ordering::SeqCst),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_LAST_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_LAST_ARG0.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_LAST_ARG0_POINTEE.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_LAST_ARG1.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_LAST_RET.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE0_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE0_ARG0.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE0_ARG0_POINTEE.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE0_ARG1.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE0_RET.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE1_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE1_ARG0.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE1_ARG0_POINTEE.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE1_ARG1.load(Ordering::SeqCst)),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_SAMPLE1_RET.load(Ordering::SeqCst)),
        TASK_ENQUEUE_GENERIC_IDLE_ITEM_MATCH_HITS.load(Ordering::SeqCst),
        format_optional_ptr(TASK_ENQUEUE_GENERIC_IDLE_ITEM_LAST_MATCH_KIND.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_LAST_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_LAST_ITEM.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_LAST_VT.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_LAST_FUNCTOR.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_LAST_DOCALL.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_WINDOW_JOB_IDLE_CTOR_LAST_ACCEPT.load(Ordering::SeqCst)),
        MENU_ITEM_UPDATE_HITS.load(Ordering::SeqCst),
        MENU_ITEM_UPDATE_SEMANTIC_HITS.load(Ordering::SeqCst),
        format_optional_ptr(MENU_ITEM_UPDATE_LAST_ITEM.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_ITEM_UPDATE_LAST_VT.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_ITEM_UPDATE_LAST_FUNCTOR.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_ITEM_UPDATE_LAST_DOCALL.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_ITEM_UPDATE_LAST_ACCEPT.load(Ordering::SeqCst)),
        format_optional_ptr(MENU_CONTINUE_CANDIDATE_ITEM.load(Ordering::SeqCst)),
        MENU_CONTINUE_CANDIDATE_HITS.load(Ordering::SeqCst),
        MENU_CONTINUE_CANDIDATE_IDLE_ACCEPT_HITS.load(Ordering::SeqCst),
        MENU_CONTINUE_CANDIDATE_NATIVE_ACCEPT_HITS.load(Ordering::SeqCst),
        MENU_CONTINUE_CANDIDATE_OTHER_ACCEPT_HITS.load(Ordering::SeqCst),
        MENU_CONTINUE_CANDIDATE_ACCEPT_CHANGES.load(Ordering::SeqCst),
        format_optional_ptr(MENU_CONTINUE_CANDIDATE_LAST_ACCEPT.load(Ordering::SeqCst)),
        TITLE_NATIVE_READY_PREDICATE_HITS.load(Ordering::SeqCst),
        format_optional_ptr(TITLE_NATIVE_READY_PREDICATE_LAST_CALLER_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(TITLE_NATIVE_READY_PREDICATE_LAST_THIS.load(Ordering::SeqCst)),
        format_optional_ptr(TITLE_NATIVE_READY_PREDICATE_LAST_VTABLE.load(Ordering::SeqCst)),
        format_optional_ptr(TITLE_NATIVE_READY_PREDICATE_LAST_GETTER.load(Ordering::SeqCst)),
        format_optional_ptr(TITLE_NATIVE_READY_PREDICATE_LAST_OBJECT.load(Ordering::SeqCst)),
        TITLE_NATIVE_READY_PREDICATE_LAST_FLAGS.load(Ordering::SeqCst),
        TITLE_NATIVE_READY_PREDICATE_LAST_MASKED.load(Ordering::SeqCst),
        TITLE_NATIVE_READY_PREDICATE_LAST_RET.load(Ordering::SeqCst),
        format_optional_ptr(TITLE_NATIVE_READY_PREDICATE_LAST_OBJECT.load(Ordering::SeqCst)),
        TITLE_NATIVE_READY_PREDICATE_LAST_FLAGS.load(Ordering::SeqCst),
        TITLE_NATIVE_READY_PREDICATE_LAST_MASKED.load(Ordering::SeqCst),
        TITLE_NATIVE_READY_PREDICATE_LAST_RET.load(Ordering::SeqCst)
    ));
    body.push_str(&format!(
        "  \"oracle_native_submit_hits\": {},\n  \"oracle_native_submit_last_result\": {},\n  \"oracle_result_event_handler_hits\": {},\n  \"oracle_result_action_builder_hits\": {},\n  \"oracle_result_event_last_result\": {},\n  \"oracle_result_event_last_event\": {},\n  \"oracle_result_event_last_raw_qword0\": {},\n  \"oracle_result_event_last_fd4_code\": {},\n  \"oracle_result_event_last_fd4_arg\": {},\n  \"oracle_result_action_last_result\": {},\n  \"oracle_result_action_last_event\": {},\n  \"oracle_result_action_last_word0\": {},\n  \"oracle_result_action_last_word1\": {},\n  \"oracle_result_action_insert_hits\": {},\n  \"oracle_result_action_last_insert_arg0\": {},\n  \"oracle_result_action_last_insert_arg1\": {},\n  \"oracle_result_action_last_insert_ret\": {},\n  \"oracle_result_action_last_insert_arg1_update_rva\": {},\n  \"oracle_result_action_last_insert_ret_update_rva\": {},\n  \"oracle_result_action_wrapper_builder_hits\": {},\n  \"oracle_result_action_last_wrapper_builder_rcx\": {},\n  \"oracle_result_action_last_wrapper_builder_rdx\": {},\n  \"oracle_result_action_last_wrapper_builder_r8\": {},\n  \"oracle_result_action_last_wrapper_builder_ret\": {},\n  \"oracle_result_action_last_wrapper_builder_ret_update_rva\": {},\n",
        NATIVE_SUBMIT_HITS.load(Ordering::SeqCst),
        format_optional_ptr(NATIVE_SUBMIT_LAST_RESULT.load(Ordering::SeqCst)),
        RESULT_EVENT_HANDLER_HITS.load(Ordering::SeqCst),
        RESULT_ACTION_BUILDER_HITS.load(Ordering::SeqCst),
        format_optional_ptr(RESULT_EVENT_LAST_RESULT.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_EVENT_LAST_EVENT.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_EVENT_LAST_RAW_QWORD0.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_EVENT_LAST_FD4_CODE.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_EVENT_LAST_FD4_ARG.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_RESULT.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_EVENT.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_WORD0.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_WORD1.load(Ordering::SeqCst)),
        RESULT_ACTION_INSERT_HITS.load(Ordering::SeqCst),
        format_optional_ptr(RESULT_ACTION_LAST_INSERT_ARG0.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_INSERT_ARG1.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_INSERT_RET.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_INSERT_ARG1_UPDATE_RVA.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_INSERT_RET_UPDATE_RVA.load(Ordering::SeqCst)),
        RESULT_ACTION_WRAPPER_BUILDER_HITS.load(Ordering::SeqCst),
        format_optional_ptr(RESULT_ACTION_LAST_WRAPPER_BUILDER_RCX.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_WRAPPER_BUILDER_RDX.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_WRAPPER_BUILDER_R8.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_WRAPPER_BUILDER_RET.load(Ordering::SeqCst)),
        format_optional_ptr(RESULT_ACTION_LAST_WRAPPER_BUILDER_RET_UPDATE_RVA.load(Ordering::SeqCst))
    ));
    body.push_str(&format!(
        // NOTE: oracle_continue_deser_fired / oracle_continue_confirmed were REMOVED
        // (2026-06-24): they tracked OWN_STEPPER_DESER_FIRED/OWN_STEPPER_CONFIRMED -- the
        // own_stepper/native_continue confirm-FIRE chain -- NOT whether the character loaded.
        // The default zero-input autoload (pab-advance + title-accept-byte natural menu-open)
        // loads without that chain, so the fields read 0 on success and were repeatedly misread
        // as "load failed". The real load semaphore is world_loaded (player_present + world_stable
        // + saved_map_c30), already emitted below. The backing statics stay (they gate block_input
        // release + own_stepper STAGE2).
        "  \"oracle_continue_phase\": {},\n  \"oracle_continue_expected_slot\": {},\n  \"oracle_continue_mount_c30\": {},\n  \"oracle_continue_guard_waits\": {},\n",
        FULLREAD_PHASE.load(Ordering::SeqCst),
        OWN_STEPPER_EXPECTED_SLOT.load(Ordering::SeqCst),
        OWN_STEPPER_MOUNT_C30.load(Ordering::SeqCst),
        FULLREAD_DRAIN_WAITS.load(Ordering::SeqCst)
    ));
    // GameMan save-mgr signals: b80 (load-in-progress lane -- the golden-capture mash-stop signal,
    // nonzero once continue is confirmed and the deserialize kicks) + c30 (saved map id, oracle item 2).
    const NULL_PTR: usize = 0;
    if let Ok(base) = crate::experiments::game_module_base() {
        let gm = crate::game_man_ptr_or_null();
        let read_i32 = |addr: usize| -> i32 {
            unsafe { crate::experiments::safe_read_usize(addr) }
                .map_or(READ_FAIL_SENTINEL, |v| v as u32 as i32)
        };
        let (b80, c30) = if gm == NULL_PTR {
            (READ_FAIL_SENTINEL, READ_FAIL_SENTINEL)
        } else {
            (
                read_i32(gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET),
                read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET),
            )
        };
        body.push_str(&format!(
            "  \"oracle_load_in_progress_b80\": {b80},\n  \"oracle_saved_map_c30\": \"{c30:#x}\",\n"
        ));
        // IDENTITY oracle: loaded character values that should match the chosen save slot.
        // These mirror ER-Save-File-Readers' player_game_data models (health/fp today, broader
        // slot attributes as that reference grows) while reading the live GameDataMan path used by
        // dump_load_correctness: GameDataMan = [base + 0x3d5df38]; PlayerGameData = [GameDataMan+8].
        const LEVEL_READ_FAIL: i64 = -1;
        const ZERO_U16: u16 = 0;
        const ZERO_U32: u32 = 0;
        const U16_STRIDE: usize = 2;
        const U32_STRIDE: usize = 4;
        const IDX_START: usize = 0;
        const IDX_STEP: usize = 1;
        let gdm = crate::game_data_man_ptr_or_null();
        let pgd = if gdm == NULL_PTR {
            NULL_PTR
        } else {
            unsafe {
                crate::experiments::safe_read_usize(
                    gdm + crate::GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET,
                )
            }
            .unwrap_or(NULL_PTR)
        };
        const U8_MASK: usize = 0xff;
        let read_pgd_u32 = |offset: usize| -> u32 {
            if pgd == NULL_PTR {
                ZERO_U32
            } else {
                unsafe { crate::experiments::safe_read_usize(pgd + offset) }
                    .map_or(ZERO_U32, |value| value as u32)
            }
        };
        let read_pgd_u8 = |offset: usize| -> u8 {
            if pgd == NULL_PTR {
                ZERO_U32 as u8
            } else {
                unsafe { crate::experiments::safe_read_usize(pgd + offset) }
                    .map_or(ZERO_U32 as u8, |value| (value & U8_MASK) as u8)
            }
        };
        let level = if pgd == NULL_PTR {
            LEVEL_READ_FAIL
        } else {
            i64::from(read_pgd_u32(crate::PGD_LEVEL_68_OFFSET))
        };
        let current_hp = read_pgd_u32(crate::PGD_CURRENT_HP_10_OFFSET);
        let current_max_hp = read_pgd_u32(crate::PGD_CURRENT_MAX_HP_14_OFFSET);
        let base_max_hp = read_pgd_u32(crate::PGD_BASE_MAX_HP_18_OFFSET);
        let current_fp = read_pgd_u32(crate::PGD_CURRENT_FP_1C_OFFSET);
        let current_max_fp = read_pgd_u32(crate::PGD_CURRENT_MAX_FP_20_OFFSET);
        let base_max_fp = read_pgd_u32(crate::PGD_BASE_MAX_FP_24_OFFSET);
        let current_stamina = read_pgd_u32(crate::PGD_CURRENT_STAMINA_2C_OFFSET);
        let current_max_stamina = read_pgd_u32(crate::PGD_CURRENT_MAX_STAMINA_30_OFFSET);
        let base_max_stamina = read_pgd_u32(crate::PGD_BASE_MAX_STAMINA_34_OFFSET);
        let runes = read_pgd_u32(crate::PGD_RUNE_COUNT_6C_OFFSET);
        let rune_memory = read_pgd_u32(crate::PGD_RUNE_MEMORY_70_OFFSET);
        let chr_type = read_pgd_u32(crate::PGD_CHR_TYPE_98_OFFSET);
        let gender = read_pgd_u8(crate::PGD_GENDER_BE_OFFSET);
        let archetype = read_pgd_u8(crate::PGD_ARCHETYPE_BF_OFFSET);
        let voice_type = read_pgd_u8(crate::PGD_VOICE_TYPE_C2_OFFSET);
        let starting_gift = read_pgd_u8(crate::PGD_STARTING_GIFT_C3_OFFSET);
        let unlocked_talisman_slots = read_pgd_u8(crate::PGD_UNLOCKED_TALISMAN_SLOTS_C6_OFFSET);
        let spirit_ash_level = read_pgd_u8(crate::PGD_SPIRIT_ASH_LEVEL_C7_OFFSET);
        const ZERO_U8: u8 = 0;
        let max_crimson_flask_count = read_pgd_u8(crate::PGD_MAX_CRIMSON_FLASK_101_OFFSET);
        let max_cerulean_flask_count = read_pgd_u8(crate::PGD_MAX_CERULEAN_FLASK_102_OFFSET);
        let face_buffer_pgd_offset = crate::PGD_FACE_DATA_OFFSET + crate::FACE_DATA_BUFFER_OFFSET;
        let mut face_data_buffer = [ZERO_U8; crate::FACE_DATA_BUFFER_TOTAL_SIZE];
        let mut face_data_idx = IDX_START;
        while face_data_idx < crate::FACE_DATA_BUFFER_TOTAL_SIZE {
            face_data_buffer[face_data_idx] = read_pgd_u8(face_buffer_pgd_offset + face_data_idx);
            face_data_idx += IDX_STEP;
        }
        let face_data_magic =
            String::from_utf8(face_data_buffer[..crate::FACE_DATA_BUFFER_VERSION_OFFSET].to_vec())
                .unwrap_or_default();
        let face_data_version =
            read_pgd_u32(face_buffer_pgd_offset + crate::FACE_DATA_BUFFER_VERSION_OFFSET);
        let face_data_buffer_size =
            read_pgd_u32(face_buffer_pgd_offset + crate::FACE_DATA_BUFFER_SIZE_OFFSET);
        let mut face_data_buffer_hex = String::new();
        for byte in face_data_buffer {
            use std::fmt::Write as _;
            let _ = write!(&mut face_data_buffer_hex, "{byte:02x}");
        }
        let face_model =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_FACE_MODEL_OFFSET);
        let hair_model =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_HAIR_MODEL_OFFSET);
        let eyebrow_model =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_EYEBROW_MODEL_OFFSET);
        let beard_model =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_BEARD_MODEL_OFFSET);
        let eye_patch_model =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_EYE_PATCH_MODEL_OFFSET);
        let apparent_age =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_APPARENT_AGE_OFFSET);
        let facial_aesthetic =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_FACIAL_AESTHETIC_OFFSET);
        let form_emphasis =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_FORM_EMPHASIS_OFFSET);
        let head_size =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_HEAD_SIZE_OFFSET);
        let chest_size =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_CHEST_SIZE_OFFSET);
        let abdomen_size =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_ABDOMEN_SIZE_OFFSET);
        let arms_size =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_ARMS_SIZE_OFFSET);
        let legs_size =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_LEGS_SIZE_OFFSET);
        let skin_color_r =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_SKIN_COLOR_R_OFFSET);
        let skin_color_g =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_SKIN_COLOR_G_OFFSET);
        let skin_color_b =
            read_pgd_u8(face_buffer_pgd_offset + crate::FACE_BODY_FIELD_SKIN_COLOR_B_OFFSET);
        let face_body_fields = format!(
            "{{\"face_model\": {face_model}, \"hair_model\": {hair_model}, \"eyebrow_model\": {eyebrow_model}, \"beard_model\": {beard_model}, \"eye_patch_model\": {eye_patch_model}, \"apparent_age\": {apparent_age}, \"facial_aesthetic\": {facial_aesthetic}, \"form_emphasis\": {form_emphasis}, \"head_size\": {head_size}, \"chest_size\": {chest_size}, \"abdomen_size\": {abdomen_size}, \"arms_size\": {arms_size}, \"legs_size\": {legs_size}, \"skin_color_r\": {skin_color_r}, \"skin_color_g\": {skin_color_g}, \"skin_color_b\": {skin_color_b}}}"
        );
        let mut name_units = [ZERO_U16; crate::PGD_NAME_LEN_U16];
        let mut name_idx = IDX_START;
        while pgd != NULL_PTR && name_idx < crate::PGD_NAME_LEN_U16 {
            name_units[name_idx] = unsafe {
                crate::experiments::safe_read_usize(
                    pgd + crate::PGD_NAME_9C_OFFSET + name_idx * U16_STRIDE,
                )
            }
            .map_or(ZERO_U16, |value| value as u16);
            name_idx += IDX_STEP;
        }
        let mut name_len = IDX_START;
        while name_len < crate::PGD_NAME_LEN_U16 && name_units[name_len] != ZERO_U16 {
            name_len += IDX_STEP;
        }
        let name = String::from_utf16(&name_units[..name_len]).unwrap_or_default();
        let mut stats = [ZERO_U32; crate::PGD_STAT_COUNT];
        let mut stat_idx = IDX_START;
        while stat_idx < crate::PGD_STAT_COUNT {
            stats[stat_idx] = read_pgd_u32(crate::PGD_STAT_BASE_3C_OFFSET + stat_idx * U32_STRIDE);
            stat_idx += IDX_STEP;
        }
        let stat_values = stats.map(|value| value.to_string()).join(", ");
        body.push_str(&format!(
            "  \"oracle_char_current_hp\": {current_hp},\n  \"oracle_char_current_max_hp\": {current_max_hp},\n  \"oracle_char_base_max_hp\": {base_max_hp},\n  \"oracle_char_current_fp\": {current_fp},\n  \"oracle_char_current_max_fp\": {current_max_fp},\n  \"oracle_char_base_max_fp\": {base_max_fp},\n  \"oracle_char_current_stamina\": {current_stamina},\n  \"oracle_char_current_max_stamina\": {current_max_stamina},\n  \"oracle_char_base_max_stamina\": {base_max_stamina},\n  \"oracle_char_level\": {level},\n  \"oracle_char_runes\": {runes},\n  \"oracle_char_rune_memory\": {rune_memory},\n  \"oracle_char_chr_type\": {chr_type},\n  \"oracle_char_gender\": {gender},\n  \"oracle_char_archetype\": {archetype},\n  \"oracle_char_voice_type\": {voice_type},\n  \"oracle_char_starting_gift\": {starting_gift},\n  \"oracle_char_unlocked_talisman_slots\": {unlocked_talisman_slots},\n  \"oracle_char_spirit_ash_level\": {spirit_ash_level},\n  \"oracle_char_max_crimson_flask_count\": {max_crimson_flask_count},\n  \"oracle_char_max_cerulean_flask_count\": {max_cerulean_flask_count},\n  \"oracle_char_name\": \"{}\",\n  \"oracle_char_name_len\": {name_len},\n  \"oracle_char_stats\": [{stat_values}],\n  \"oracle_face_data_magic\": \"{}\",\n  \"oracle_face_data_version\": {face_data_version},\n  \"oracle_face_data_buffer_size\": {face_data_buffer_size},\n  \"oracle_face_data_buffer_hex\": \"{face_data_buffer_hex}\",\n  \"oracle_face_body_fields\": {face_body_fields},\n",
            json_escape(&name),
            json_escape(&face_data_magic)
        ));
        // WORLD-LIVE oracle: CSNowLoadingHelper "now loading" latch = *(u8*)([base+0x3d60ec8]+0xED).
        // 1 = loading screen ACTIVE; 0 = cleared / playable (latches when the MoveMapStep world-load
        // steps stop requesting the loading screen). This replaces the grounded check, which fires
        // DURING loading (player physics exist before the world renders).
        const NOW_LOADING_SINGLETON_RVA: usize = RuntimeGlobalRva::NowLoadingSingleton as usize;
        const NOW_LOADING_FLAG_OFFSET: usize =
            core::mem::offset_of!(NowLoadingHelperLayout, loading_flag);
        const NOW_LOADING_BYTE_MASK: usize = u8::MAX as usize;
        let now_loading = {
            let helper =
                unsafe { crate::experiments::safe_read_usize(base + NOW_LOADING_SINGLETON_RVA) }
                    .unwrap_or(NULL_PTR);
            if helper == NULL_PTR {
                READ_FAIL_SENTINEL
            } else {
                unsafe { crate::experiments::safe_read_usize(helper + NOW_LOADING_FLAG_OFFSET) }
                    .map_or(READ_FAIL_SENTINEL, |v| (v & NOW_LOADING_BYTE_MASK) as i32)
            }
        };
        body.push_str(&format!("  \"oracle_now_loading\": {now_loading},\n"));
        let msgbox_dialog = MSGBOX_LAST_DIALOG.load(Ordering::SeqCst);
        let msgbox_vtable = if msgbox_dialog == NULL_PTR {
            NULL_PTR
        } else {
            unsafe { crate::experiments::safe_read_usize(msgbox_dialog) }.unwrap_or(NULL_PTR)
        };
        let msgbox_closing_latch = if msgbox_vtable == base + MSGBOX_DIALOG_VTABLE_RVA {
            unsafe {
                crate::experiments::safe_read_usize(msgbox_dialog + MSGBOX_CLOSING_LATCH_3B0_OFFSET)
            }
            .map(|value| value & MSGBOX_LATCH_BYTE_MASK)
            .unwrap_or(MSGBOX_CLOSING_YES)
        } else {
            MSGBOX_CLOSING_YES
        };
        const NO_MSGBOX_BUILDS: usize = MENU_TRACE_UNSEEN_SEQ;
        let msgbox_total_builds = MSGBOX_TOTAL_BUILDS.load(Ordering::SeqCst);
        let msgbox_postload_builds = MSGBOX_POSTLOAD_BUILDS.load(Ordering::SeqCst);
        let msgbox_any_seen = msgbox_total_builds != NO_MSGBOX_BUILDS;
        let postload_modal_seen = msgbox_postload_builds != NO_MSGBOX_BUILDS;
        let blocking_modal_present = msgbox_vtable == base + MSGBOX_DIALOG_VTABLE_RVA
            && msgbox_closing_latch != MSGBOX_CLOSING_YES;
        let msgbox_arg_rcx = MSGBOX_LAST_ARG_RCX.load(Ordering::SeqCst);
        let msgbox_arg_rdx = MSGBOX_LAST_ARG_RDX.load(Ordering::SeqCst);
        let msgbox_arg_r8 = MSGBOX_LAST_ARG_R8.load(Ordering::SeqCst);
        let msgbox_arg_r9 = MSGBOX_LAST_ARG_R9.load(Ordering::SeqCst);
        let policy_total_builds = POLICY_TOS_TITLE_TOTAL_BUILDS.load(Ordering::SeqCst);
        let policy_any_seen = policy_total_builds != NO_MSGBOX_BUILDS;
        let policy_ptr = POLICY_TOS_TITLE_LAST_THIS.load(Ordering::SeqCst);
        let policy_vtable = POLICY_TOS_TITLE_LAST_VTABLE.load(Ordering::SeqCst);
        let policy_arg_rdx = POLICY_TOS_TITLE_LAST_ARG_RDX.load(Ordering::SeqCst);
        let policy_arg_r8 = POLICY_TOS_TITLE_LAST_ARG_R8.load(Ordering::SeqCst);
        let policy_arg_r9 = POLICY_TOS_TITLE_LAST_ARG_R9.load(Ordering::SeqCst);
        let policy_stack_arg0 = POLICY_TOS_TITLE_LAST_STACK_ARG0.load(Ordering::SeqCst);
        let policy_backing_flag_ptr = POLICY_TOS_TITLE_LAST_BACKING_FLAG_PTR.load(Ordering::SeqCst);
        let policy_stored_backing_flag_ptr =
            POLICY_TOS_TITLE_LAST_STORED_BACKING_FLAG_PTR.load(Ordering::SeqCst);
        let policy_backing_flag_value =
            POLICY_TOS_TITLE_LAST_BACKING_FLAG_VALUE.load(Ordering::SeqCst);
        let policy_requested_flag_value =
            POLICY_TOS_TITLE_LAST_REQUESTED_FLAG_VALUE.load(Ordering::SeqCst);
        let policy_caller_rva = POLICY_TOS_TITLE_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let policy_wrapper_hits = POLICY_TOS_TITLE_WRAPPER_HITS.load(Ordering::SeqCst);
        let policy_wrapper_record = POLICY_TOS_TITLE_WRAPPER_LAST_RECORD.load(Ordering::SeqCst);
        let policy_wrapper_original_this =
            POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_THIS.load(Ordering::SeqCst);
        let policy_wrapper_original_vtable =
            POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_VTABLE.load(Ordering::SeqCst);
        let policy_wrapper_record_id =
            POLICY_TOS_TITLE_WRAPPER_LAST_RECORD_ID.load(Ordering::SeqCst);
        let policy_wrapper_stack_arg0 =
            POLICY_TOS_TITLE_WRAPPER_LAST_STACK_ARG0.load(Ordering::SeqCst);
        let policy_wrapper_backing_flag_ptr =
            POLICY_TOS_TITLE_WRAPPER_LAST_BACKING_FLAG_PTR.load(Ordering::SeqCst);
        let policy_wrapper_ret = POLICY_TOS_TITLE_WRAPPER_LAST_RET.load(Ordering::SeqCst);
        let policy_wrapper_caller_rva =
            POLICY_TOS_TITLE_WRAPPER_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let policy_selector_hits = POLICY_TOS_SELECTOR_WRAPPER_HITS.load(Ordering::SeqCst);
        let policy_selector_record = POLICY_TOS_SELECTOR_WRAPPER_LAST_RECORD.load(Ordering::SeqCst);
        let policy_selector_original_this =
            POLICY_TOS_SELECTOR_WRAPPER_LAST_ORIGINAL_THIS.load(Ordering::SeqCst);
        let policy_selector_original_vtable =
            POLICY_TOS_SELECTOR_WRAPPER_LAST_ORIGINAL_VTABLE.load(Ordering::SeqCst);
        let policy_selector_owner = POLICY_TOS_SELECTOR_WRAPPER_LAST_OWNER.load(Ordering::SeqCst);
        let policy_selector_requested_flag =
            POLICY_TOS_SELECTOR_WRAPPER_LAST_REQUESTED_FLAG.load(Ordering::SeqCst);
        let policy_selector_arg =
            POLICY_TOS_SELECTOR_WRAPPER_LAST_SELECTOR_ARG.load(Ordering::SeqCst);
        let policy_selector_ret = POLICY_TOS_SELECTOR_WRAPPER_LAST_RET.load(Ordering::SeqCst);
        let policy_selector_caller_rva =
            POLICY_TOS_SELECTOR_WRAPPER_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let policy_selector_ctor_hits = POLICY_TOS_SELECTOR_CTOR_HITS.load(Ordering::SeqCst);
        let policy_selector_ctor_this = POLICY_TOS_SELECTOR_CTOR_LAST_THIS.load(Ordering::SeqCst);
        let policy_selector_ctor_vtable =
            POLICY_TOS_SELECTOR_CTOR_LAST_VTABLE.load(Ordering::SeqCst);
        let policy_selector_ctor_owner = POLICY_TOS_SELECTOR_CTOR_LAST_OWNER.load(Ordering::SeqCst);
        let policy_selector_ctor_requested_flag_ptr =
            POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_PTR.load(Ordering::SeqCst);
        let policy_selector_ctor_requested_flag_value =
            POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_VALUE.load(Ordering::SeqCst);
        let policy_selector_ctor_selector_arg =
            POLICY_TOS_SELECTOR_CTOR_LAST_SELECTOR_ARG.load(Ordering::SeqCst);
        let policy_selector_ctor_stored_selector_arg =
            POLICY_TOS_SELECTOR_CTOR_LAST_STORED_SELECTOR_ARG.load(Ordering::SeqCst);
        let policy_selector_ctor_stored_requested_flag_ptr =
            POLICY_TOS_SELECTOR_CTOR_LAST_STORED_REQUESTED_FLAG_PTR.load(Ordering::SeqCst);
        let policy_selector_ctor_ret = POLICY_TOS_SELECTOR_CTOR_LAST_RET.load(Ordering::SeqCst);
        let policy_selector_ctor_caller_rva =
            POLICY_TOS_SELECTOR_CTOR_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let policy_status_hits = POLICY_TOS_STATUS_HITS.load(Ordering::SeqCst);
        let policy_status_this = POLICY_TOS_STATUS_LAST_THIS.load(Ordering::SeqCst);
        let policy_status_owner = POLICY_TOS_STATUS_LAST_OWNER.load(Ordering::SeqCst);
        let policy_status_flag_ptr = POLICY_TOS_STATUS_LAST_FLAG_PTR.load(Ordering::SeqCst);
        let policy_status_flag_value = POLICY_TOS_STATUS_LAST_FLAG_VALUE.load(Ordering::SeqCst);
        let policy_status_ret = POLICY_TOS_STATUS_LAST_RET.load(Ordering::SeqCst);
        let policy_status_caller_rva = POLICY_TOS_STATUS_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let policy_flag_setter_hits = POLICY_TOS_FLAG_SETTER_HITS.load(Ordering::SeqCst);
        let policy_flag_setter_owner = POLICY_TOS_FLAG_SETTER_LAST_OWNER.load(Ordering::SeqCst);
        let policy_flag_setter_value = POLICY_TOS_FLAG_SETTER_LAST_VALUE.load(Ordering::SeqCst);
        let policy_flag_setter_force = POLICY_TOS_FLAG_SETTER_LAST_FORCE.load(Ordering::SeqCst);
        let policy_flag_setter_before = POLICY_TOS_FLAG_SETTER_LAST_BEFORE.load(Ordering::SeqCst);
        let policy_flag_setter_after = POLICY_TOS_FLAG_SETTER_LAST_AFTER.load(Ordering::SeqCst);
        let policy_flag_setter_caller_rva =
            POLICY_TOS_FLAG_SETTER_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let server_status_total_seen = SERVER_STATUS_TOTAL_SEEN.load(Ordering::SeqCst);
        let server_status_any_seen = server_status_total_seen != NO_MSGBOX_BUILDS;
        let server_status_state = SERVER_STATUS_LAST_STATE.load(Ordering::SeqCst);
        let server_status_text_id = SERVER_STATUS_LAST_TEXT_ID.load(Ordering::SeqCst);
        let title_visual_suppress_installed = TITLE_NATIVE_MENU_VISUAL_SUPPRESS_INSTALLED
            .load(Ordering::SeqCst)
            == TITLE_NATIVE_MENU_VISUAL_SUPPRESS_INSTALLED_YES;
        let title_visual_suppressed_builds =
            TITLE_NATIVE_MENU_VISUAL_SUPPRESSED_BUILDS.load(Ordering::SeqCst);
        let title_visual_last_out_slot =
            TITLE_NATIVE_MENU_VISUAL_LAST_OUT_SLOT.load(Ordering::SeqCst);
        let title_visual_last_prev_out =
            TITLE_NATIVE_MENU_VISUAL_LAST_PREV_OUT.load(Ordering::SeqCst);
        let title_visual_last_arg_rdx =
            TITLE_NATIVE_MENU_VISUAL_LAST_ARG_RDX.load(Ordering::SeqCst);
        let title_visual_last_arg_r8 = TITLE_NATIVE_MENU_VISUAL_LAST_ARG_R8.load(Ordering::SeqCst);
        let title_visual_last_caller_rva =
            TITLE_NATIVE_MENU_VISUAL_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let title_visual_native_job = TITLE_NATIVE_MENU_VISUAL_NATIVE_JOB.load(Ordering::SeqCst);
        let title_visual_native_window =
            TITLE_NATIVE_MENU_VISUAL_NATIVE_WINDOW.load(Ordering::SeqCst);
        let title_visual_render_suppress_installed =
            TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED.load(Ordering::SeqCst)
                == TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED_YES;
        let title_visual_render_suppressed_windows =
            TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESSED_WINDOWS.load(Ordering::SeqCst);
        let title_visual_render_last_window =
            TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_WINDOW.load(Ordering::SeqCst);
        let title_visual_render_last_flags_before =
            TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_FLAGS_BEFORE.load(Ordering::SeqCst);
        let title_visual_render_last_flags_after =
            TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_FLAGS_AFTER.load(Ordering::SeqCst);
        let title_visual_render_last_caller_rva =
            TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let title_visual_current_menu_id = if title_visual_native_window != NULL_PTR
            && title_visual_native_window != TITLE_OWNER_SCAN_START_ADDRESS
        {
            unsafe { crate::experiments::safe_read_u16(title_visual_native_window + 0x180) }
                .map_or(TITLE_OWNER_SCAN_START_ADDRESS, usize::from)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let title_visual_current_flags = if title_visual_current_menu_id < 0x47 {
            let cs_menu_man =
                unsafe { crate::experiments::safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }
                    .unwrap_or(NULL_PTR);
            if cs_menu_man != NULL_PTR {
                unsafe {
                    crate::experiments::safe_read_u8(
                        cs_menu_man + 0x90 + title_visual_current_menu_id,
                    )
                }
                .map_or(TITLE_OWNER_SCAN_START_ADDRESS, usize::from)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            }
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let title_visual_current_draw_bit_set = title_visual_current_flags
            != TITLE_OWNER_SCAN_START_ADDRESS
            && (title_visual_current_flags & TITLE_NATIVE_MENU_VISUAL_VISIBLE_FLAGS_MASK as usize)
                != 0;
        // Actual visible logo surface telemetry: `TitleBackViewParts` / `05_001_Title_Logo` is an
        // embedded object at TitleTopDialog+0xaa8, separate from the preserved `05_000_Title`
        // MenuWindowJob. A real portrait cover depends on post-SL2 profile_summary readiness and the
        // SYSTEX_Menu_Profile render pipeline, so expose both in RAM telemetry before any mutation.
        let title_logo_dialog = PRODUCT_CORE_LAST_TITLE_DIALOG.load(Ordering::SeqCst);
        let title_logo_back_view_parts = if title_logo_dialog != NULL_PTR
            && title_logo_dialog != TITLE_OWNER_SCAN_START_ADDRESS
        {
            title_logo_dialog + TITLE_LOGO_BACK_VIEW_PARTS_AA8_OFFSET
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let title_logo_back_view_parts_vtable =
            if title_logo_back_view_parts != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { crate::experiments::safe_read_usize(title_logo_back_view_parts) }
                    .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            };
        let title_logo_gfx_frame =
            if title_logo_back_view_parts_vtable != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { title_logo_gfx_current_frame(base, title_logo_back_view_parts) }
            } else {
                TITLE_LOGO_GFX_UNKNOWN_FRAME
            };
        let title_logo_gfx_alpha_mult_term = title_logo_gfx_alpha_for_frame(title_logo_gfx_frame);
        let title_logo_gfx_visibility = title_logo_gfx_alpha_mult_term > 0;
        let title_logo_gfx_hide_calls = TITLE_LOGO_GFX_HIDE_CALLS.load(Ordering::SeqCst);
        let title_logo_gfx_hide_last_dialog =
            TITLE_LOGO_GFX_HIDE_LAST_DIALOG.load(Ordering::SeqCst);
        let title_logo_gfx_hide_last_logo = TITLE_LOGO_GFX_HIDE_LAST_LOGO.load(Ordering::SeqCst);
        let title_logo_gfx_hide_last_caller_phase =
            TITLE_LOGO_GFX_HIDE_LAST_CALLER_PHASE.load(Ordering::SeqCst);
        let title_logo_gfx_hide_last_requested_visible =
            TITLE_LOGO_GFX_HIDE_LAST_REQUESTED_VISIBLE.load(Ordering::SeqCst);
        let title_press_start_gfx_hide_calls =
            TITLE_PRESS_START_GFX_HIDE_CALLS.load(Ordering::SeqCst);
        let title_press_start_gfx_hide_last_dialog =
            TITLE_PRESS_START_GFX_HIDE_LAST_DIALOG.load(Ordering::SeqCst);
        let title_press_start_gfx_hide_last_proxy =
            TITLE_PRESS_START_GFX_HIDE_LAST_PROXY.load(Ordering::SeqCst);
        let title_press_start_gfx_hide_last_context =
            TITLE_PRESS_START_GFX_HIDE_LAST_CONTEXT.load(Ordering::SeqCst);
        let title_press_start_gfx_hide_last_caller_phase =
            TITLE_PRESS_START_GFX_HIDE_LAST_CALLER_PHASE.load(Ordering::SeqCst);
        let title_press_start_gfx_value = TITLE_PRESS_START_GFX_VALUE.load(Ordering::SeqCst);
        let title_press_start_gfx_force_false_calls =
            TITLE_PRESS_START_GFX_FORCE_FALSE_CALLS.load(Ordering::SeqCst);
        let title_press_start_gfx_force_false_last_value =
            TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_VALUE.load(Ordering::SeqCst);
        let title_press_start_gfx_force_false_last_requested =
            TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_REQUESTED.load(Ordering::SeqCst);
        let title_press_start_bind_hits = TITLE_PRESS_START_BIND_HITS.load(Ordering::SeqCst);
        let title_press_start_bind_last_parent =
            TITLE_PRESS_START_BIND_LAST_PARENT.load(Ordering::SeqCst);
        let title_press_start_bind_last_out =
            TITLE_PRESS_START_BIND_LAST_OUT.load(Ordering::SeqCst);
        let title_press_start_bind_last_name =
            TITLE_PRESS_START_BIND_LAST_NAME.load(Ordering::SeqCst);
        let title_press_start_bind_last_context =
            TITLE_PRESS_START_BIND_LAST_CONTEXT.load(Ordering::SeqCst);
        let title_press_start_bind_hide_calls =
            TITLE_PRESS_START_BIND_HIDE_CALLS.load(Ordering::SeqCst);
        // Real false until a later mutation binds the post-SL2 profile/SYSTEX portrait to the
        // 05_001_Title_Logo root-depth-3 surface. `05_010_ProfileSelect` dummy faces are exported
        // bitmap classes only (0 timeline placements), so profile_summary readiness alone is not a
        // visible cover binding.
        let title_profile_cover_bound_to_logo_surface = false;
        let title_logo_profile_summary = {
            let game_data_man = crate::game_data_man_ptr_or_null();
            if game_data_man != NULL_PTR {
                unsafe {
                    crate::experiments::safe_read_usize(
                        game_data_man + SLOT_MANAGER_CONTAINER_OFFSET,
                    )
                }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            }
        };
        let title_logo_profile_summary_ready = title_logo_profile_summary
            != TITLE_OWNER_SCAN_START_ADDRESS
            && title_logo_profile_summary != NULL_PTR;
        let title_scaleform_bind_observer_hits =
            TITLE_SCALEFORM_BIND_OBSERVER_HITS.load(Ordering::SeqCst);
        let title_scaleform_bind_observer_systex_hits =
            TITLE_SCALEFORM_BIND_OBSERVER_SYSTEX_HITS.load(Ordering::SeqCst);
        let title_scaleform_bind_observer_last_owner =
            TITLE_SCALEFORM_BIND_OBSERVER_LAST_OWNER.load(Ordering::SeqCst);
        let title_scaleform_bind_observer_last_pair =
            TITLE_SCALEFORM_BIND_OBSERVER_LAST_PAIR.load(Ordering::SeqCst);
        let title_scaleform_bind_observer_last_symbol_ptr =
            TITLE_SCALEFORM_BIND_OBSERVER_LAST_SYMBOL_PTR.load(Ordering::SeqCst);
        let title_scaleform_bind_observer_last_target_ptr =
            TITLE_SCALEFORM_BIND_OBSERVER_LAST_TARGET_PTR.load(Ordering::SeqCst);
        let now_loading_helper_hooks_installed =
            NOW_LOADING_HELPER_HOOKS_INSTALLED.load(Ordering::SeqCst);
        let now_loading_helper_ctor_hits = NOW_LOADING_HELPER_CTOR_HITS.load(Ordering::SeqCst);
        let now_loading_helper_update_hits = NOW_LOADING_HELPER_UPDATE_HITS.load(Ordering::SeqCst);
        let now_loading_helper_last_this = NOW_LOADING_HELPER_LAST_THIS.load(Ordering::SeqCst);
        let now_loading_helper_last_menu_index =
            NOW_LOADING_HELPER_LAST_MENU_INDEX.load(Ordering::SeqCst);
        let now_loading_helper_last_replace_tex_info =
            NOW_LOADING_HELPER_LAST_REPLACE_TEX_INFO.load(Ordering::SeqCst);
        let now_loading_helper_last_requested_replace_tex_info =
            NOW_LOADING_HELPER_LAST_REQUESTED_REPLACE_TEX_INFO.load(Ordering::SeqCst);
        let now_loading_helper_last_flags = NOW_LOADING_HELPER_LAST_FLAGS.load(Ordering::SeqCst);
        let title_custom_cover_profile_render_refresh_calls =
            TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_CALLS.load(Ordering::SeqCst);
        let title_custom_cover_profile_render_refresh_last_profile_summary =
            TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_LAST_PROFILE_SUMMARY.load(Ordering::SeqCst);
        let title_custom_cover_profile_render_refresh_last_caller_phase =
            TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_LAST_CALLER_PHASE.load(Ordering::SeqCst);
        let title_custom_cover_profile_select_builds =
            TITLE_CUSTOM_COVER_PROFILE_SELECT_BUILDS.load(Ordering::SeqCst);
        let title_custom_cover_profile_select_last_ret =
            TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_RET.load(Ordering::SeqCst);
        let title_custom_cover_profile_select_last_job =
            TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_JOB.load(Ordering::SeqCst);
        let title_custom_cover_profile_select_last_caller_rva =
            TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_CALLER_RVA.load(Ordering::SeqCst);
        let title_custom_cover_run_calls = TITLE_CUSTOM_COVER_RUN_CALLS.load(Ordering::SeqCst);
        let title_custom_cover_run_last_native_job =
            TITLE_CUSTOM_COVER_RUN_LAST_NATIVE_JOB.load(Ordering::SeqCst);
        let title_custom_cover_run_last_cover_job =
            TITLE_CUSTOM_COVER_RUN_LAST_COVER_JOB.load(Ordering::SeqCst);
        let title_custom_cover_run_last_cover_window =
            TITLE_CUSTOM_COVER_RUN_LAST_COVER_WINDOW.load(Ordering::SeqCst);
        let title_custom_cover_run_last_ret =
            TITLE_CUSTOM_COVER_RUN_LAST_RET.load(Ordering::SeqCst);
        let title_pab_information_visual_builds =
            TITLE_PAB_INFORMATION_VISUAL_BUILDS.load(Ordering::SeqCst);
        let title_pab_information_visual_last_job =
            TITLE_PAB_INFORMATION_VISUAL_LAST_JOB.load(Ordering::SeqCst);
        let title_pab_information_visual_last_window =
            TITLE_PAB_INFORMATION_VISUAL_LAST_WINDOW.load(Ordering::SeqCst);
        let title_pab_information_visual_last_caller_rva =
            TITLE_PAB_INFORMATION_VISUAL_LAST_CALLER_RVA.load(Ordering::SeqCst);
        body.push_str(&format!(
            "  \"oracle_msgbox_total_builds\": {},\n  \"oracle_msgbox_any_seen\": {},\n  \"oracle_msgbox_postload_builds\": {},\n  \"oracle_postload_modal_seen\": {},\n  \"oracle_blocking_modal_present\": {},\n  \"oracle_blocking_modal_ptr\": {},\n  \"oracle_blocking_modal_vtable\": {},\n  \"oracle_blocking_modal_closing_latch\": {},\n  \"oracle_msgbox_builder_args\": [{}, {}, {}, {}],\n  \"oracle_policy_window_total_builds\": {},\n  \"oracle_policy_window_any_seen\": {},\n  \"oracle_policy_window_ptr\": {},\n  \"oracle_policy_window_vtable\": {},\n  \"oracle_policy_window_args\": [{}, {}, {}, {}, {}],\n  \"oracle_policy_window_stack_arg0\": {},\n  \"oracle_policy_window_backing_flag_ptr\": {},\n  \"oracle_policy_window_stored_backing_flag_ptr\": {},\n  \"oracle_policy_window_backing_flag_value\": {},\n  \"oracle_policy_window_requested_flag_value\": {},\n  \"oracle_policy_window_caller_rva\": {},\n  \"oracle_policy_ctor_wrapper_hits\": {},\n  \"oracle_policy_ctor_wrapper_record\": {},\n  \"oracle_policy_ctor_wrapper_original_this\": {},\n  \"oracle_policy_ctor_wrapper_original_vtable\": {},\n  \"oracle_policy_ctor_wrapper_record_id\": {},\n  \"oracle_policy_ctor_wrapper_stack_arg0\": {},\n  \"oracle_policy_ctor_wrapper_backing_flag_ptr\": {},\n  \"oracle_policy_ctor_wrapper_ret\": {},\n  \"oracle_policy_ctor_wrapper_caller_rva\": {},\n  \"oracle_policy_selector_wrapper_hits\": {},\n  \"oracle_policy_selector_wrapper_record\": {},\n  \"oracle_policy_selector_wrapper_original_this\": {},\n  \"oracle_policy_selector_wrapper_original_vtable\": {},\n  \"oracle_policy_selector_wrapper_owner\": {},\n  \"oracle_policy_selector_wrapper_requested_flag\": {},\n  \"oracle_policy_selector_wrapper_selector_arg\": {},\n  \"oracle_policy_selector_wrapper_ret\": {},\n  \"oracle_policy_selector_wrapper_caller_rva\": {},\n  \"oracle_policy_selector_ctor_hits\": {},\n  \"oracle_policy_selector_ctor_this\": {},\n  \"oracle_policy_selector_ctor_vtable\": {},\n  \"oracle_policy_selector_ctor_owner\": {},\n  \"oracle_policy_selector_ctor_requested_flag_ptr\": {},\n  \"oracle_policy_selector_ctor_requested_flag_value\": {},\n  \"oracle_policy_selector_ctor_selector_arg\": {},\n  \"oracle_policy_selector_ctor_stored_selector_arg\": {},\n  \"oracle_policy_selector_ctor_stored_requested_flag_ptr\": {},\n  \"oracle_policy_selector_ctor_ret\": {},\n  \"oracle_policy_selector_ctor_caller_rva\": {},\n  \"oracle_policy_status_predicate_hits\": {},\n  \"oracle_policy_status_predicate_this\": {},\n  \"oracle_policy_status_predicate_owner\": {},\n  \"oracle_policy_status_predicate_flag_ptr\": {},\n  \"oracle_policy_status_predicate_flag_value\": {},\n  \"oracle_policy_status_predicate_ret\": {},\n  \"oracle_policy_status_predicate_caller_rva\": {},\n  \"oracle_policy_flag_setter_hits\": {},\n  \"oracle_policy_flag_setter_owner\": {},\n  \"oracle_policy_flag_setter_value\": {},\n  \"oracle_policy_flag_setter_force\": {},\n  \"oracle_policy_flag_setter_before\": {},\n  \"oracle_policy_flag_setter_after\": {},\n  \"oracle_policy_flag_setter_caller_rva\": {},\n  \"oracle_server_status_total_seen\": {},\n  \"oracle_server_status_any_seen\": {},\n  \"oracle_server_status_state\": {},\n  \"oracle_server_status_text_id\": {},\n",
            msgbox_total_builds,
            msgbox_any_seen,
            msgbox_postload_builds,
            postload_modal_seen,
            blocking_modal_present,
            msgbox_dialog,
            msgbox_vtable,
            msgbox_closing_latch,
            msgbox_arg_rcx,
            msgbox_arg_rdx,
            msgbox_arg_r8,
            msgbox_arg_r9,
            policy_total_builds,
            policy_any_seen,
            policy_ptr,
            policy_vtable,
            policy_arg_rdx,
            policy_arg_r8,
            policy_arg_r9,
            policy_stack_arg0,
            policy_backing_flag_ptr,
            policy_stack_arg0,
            policy_backing_flag_ptr,
            policy_stored_backing_flag_ptr,
            policy_backing_flag_value,
            policy_requested_flag_value,
            policy_caller_rva,
            policy_wrapper_hits,
            policy_wrapper_record,
            policy_wrapper_original_this,
            policy_wrapper_original_vtable,
            policy_wrapper_record_id,
            policy_wrapper_stack_arg0,
            policy_wrapper_backing_flag_ptr,
            policy_wrapper_ret,
            policy_wrapper_caller_rva,
            policy_selector_hits,
            policy_selector_record,
            policy_selector_original_this,
            policy_selector_original_vtable,
            policy_selector_owner,
            policy_selector_requested_flag,
            policy_selector_arg,
            policy_selector_ret,
            policy_selector_caller_rva,
            policy_selector_ctor_hits,
            policy_selector_ctor_this,
            policy_selector_ctor_vtable,
            policy_selector_ctor_owner,
            policy_selector_ctor_requested_flag_ptr,
            policy_selector_ctor_requested_flag_value,
            policy_selector_ctor_selector_arg,
            policy_selector_ctor_stored_selector_arg,
            policy_selector_ctor_stored_requested_flag_ptr,
            policy_selector_ctor_ret,
            policy_selector_ctor_caller_rva,
            policy_status_hits,
            policy_status_this,
            policy_status_owner,
            policy_status_flag_ptr,
            policy_status_flag_value,
            policy_status_ret,
            policy_status_caller_rva,
            policy_flag_setter_hits,
            policy_flag_setter_owner,
            policy_flag_setter_value,
            policy_flag_setter_force,
            policy_flag_setter_before,
            policy_flag_setter_after,
            policy_flag_setter_caller_rva,
            server_status_total_seen,
            server_status_any_seen,
            server_status_state,
            server_status_text_id
        ));
        body.push_str(&format!(
            "  \"oracle_title_native_menu_visual_suppress_installed\": {},\n  \"oracle_title_native_menu_visual_suppressed_builds\": {},\n  \"oracle_title_native_menu_visual_any_suppressed\": {},\n  \"oracle_title_native_menu_visual_last_out_slot\": {},\n  \"oracle_title_native_menu_visual_last_prev_out\": {},\n  \"oracle_title_native_menu_visual_last_args\": [{}, {}],\n  \"oracle_title_native_menu_visual_last_caller_rva\": {},\n  \"oracle_title_native_menu_visual_native_job\": {},\n  \"oracle_title_native_menu_visual_native_window\": {},\n  \"oracle_title_native_menu_visual_current_menu_id\": {},\n  \"oracle_title_native_menu_visual_current_flags\": {},\n  \"oracle_title_native_menu_visual_current_draw_bit_set\": {},\n  \"oracle_title_native_menu_visual_render_suppress_installed\": {},\n  \"oracle_title_native_menu_visual_render_suppressed_windows\": {},\n  \"oracle_title_native_menu_visual_render_any_suppressed\": {},\n  \"oracle_title_native_menu_visual_render_last_window\": {},\n  \"oracle_title_native_menu_visual_render_last_flags_before\": {},\n  \"oracle_title_native_menu_visual_render_last_flags_after\": {},\n  \"oracle_title_native_menu_visual_render_last_caller_rva\": {},\n  \"oracle_title_logo_surface_name\": \"{}\",\n  \"oracle_title_logo_resource_name\": \"{}\",\n  \"oracle_title_logo_gfx_root_depth\": {},\n  \"oracle_title_logo_gfx_root_sprite_char\": {},\n  \"oracle_title_logo_gfx_main_asset_char\": {},\n  \"oracle_title_logo_gfx_main_asset_name\": \"{}\",\n  \"oracle_title_logo_back_view_parts\": {},\n  \"oracle_title_logo_back_view_parts_vtable\": {},\n  \"oracle_title_logo_gfx_frame\": {},\n  \"oracle_title_logo_gfx_alpha_mult_term\": {},\n  \"oracle_title_logo_gfx_visibility\": {},\n  \"oracle_title_logo_gfx_hide_calls\": {},\n  \"oracle_title_logo_gfx_any_hidden\": {},\n  \"oracle_title_logo_gfx_hide_last_dialog\": {},\n  \"oracle_title_logo_gfx_hide_last_logo\": {},\n  \"oracle_title_logo_gfx_hide_last_caller_phase\": {},\n  \"oracle_title_logo_gfx_hide_last_requested_visible\": {},\n  \"oracle_title_press_start_surface_name\": \"PressStart\",\n  \"oracle_title_press_start_text_name\": \"StaticSystemText_101000\",\n  \"oracle_title_press_start_text_initial\": \"PRESS BUTTON\",\n  \"oracle_title_press_start_gfx_hide_calls\": {},\n  \"oracle_title_press_start_gfx_any_hidden\": {},\n  \"oracle_title_press_start_gfx_hide_last_dialog\": {},\n  \"oracle_title_press_start_gfx_hide_last_proxy\": {},\n  \"oracle_title_press_start_gfx_hide_last_context\": {},\n  \"oracle_title_press_start_gfx_hide_last_caller_phase\": {},\n  \"oracle_title_press_start_gfx_value\": {},\n  \"oracle_title_press_start_gfx_force_false_calls\": {},\n  \"oracle_title_press_start_gfx_force_false_any\": {},\n  \"oracle_title_press_start_gfx_force_false_last_value\": {},\n  \"oracle_title_press_start_gfx_force_false_last_requested\": {},\n  \"oracle_title_press_start_bind_hits\": {},\n  \"oracle_title_press_start_bind_any\": {},\n  \"oracle_title_press_start_bind_last_parent\": {},\n  \"oracle_title_press_start_bind_last_out\": {},\n  \"oracle_title_press_start_bind_last_name\": {},\n  \"oracle_title_press_start_bind_last_context\": {},\n  \"oracle_title_press_start_bind_hide_calls\": {},\n  \"oracle_title_press_start_bind_any_hidden\": {},\n  \"oracle_title_profile_cover_bound_to_logo_surface\": {},\n  \"oracle_title_scaleform_bind_observer_hits\": {},\n  \"oracle_title_scaleform_bind_observer_systex_hits\": {},\n  \"oracle_title_scaleform_bind_observer_last_owner\": {},\n  \"oracle_title_scaleform_bind_observer_last_pair\": {},\n  \"oracle_title_scaleform_bind_observer_last_symbol_ptr\": {},\n  \"oracle_title_scaleform_bind_observer_last_target_ptr\": {},\n  \"oracle_title_now_loading_helper_hooks_installed\": {},\n  \"oracle_title_now_loading_helper_ctor_hits\": {},\n  \"oracle_title_now_loading_helper_update_hits\": {},\n  \"oracle_title_now_loading_helper_last_this\": {},\n  \"oracle_title_now_loading_helper_last_menu_index\": {},\n  \"oracle_title_now_loading_helper_last_replace_tex_info\": {},\n  \"oracle_title_now_loading_helper_last_requested_replace_tex_info\": {},\n  \"oracle_title_now_loading_helper_last_flags\": {},\n  \"oracle_title_logo_profile_summary\": {},\n  \"oracle_title_logo_profile_summary_ready\": {},\n  \"oracle_title_custom_cover_profile_render_refresh_calls\": {},\n  \"oracle_title_custom_cover_profile_render_refresh_last_profile_summary\": {},\n  \"oracle_title_custom_cover_profile_render_refresh_last_caller_phase\": {},\n  \"oracle_title_custom_cover_profile_select_builds\": {},\n  \"oracle_title_custom_cover_profile_select_any_built\": {},\n  \"oracle_title_custom_cover_profile_select_last_ret\": {},\n  \"oracle_title_custom_cover_profile_select_last_job\": {},\n  \"oracle_title_custom_cover_profile_select_last_caller_rva\": {},\n  \"oracle_title_custom_cover_run_calls\": {},\n  \"oracle_title_custom_cover_run_any\": {},\n  \"oracle_title_custom_cover_run_last_native_job\": {},\n  \"oracle_title_custom_cover_run_last_cover_job\": {},\n  \"oracle_title_custom_cover_run_last_cover_window\": {},\n  \"oracle_title_custom_cover_run_last_ret\": {},\n  \"oracle_title_pab_information_visual_name\": \"{}\",\n  \"oracle_title_pab_information_visual_builds\": {},\n  \"oracle_title_pab_information_visual_any_built\": {},\n  \"oracle_title_pab_information_visual_last_job\": {},\n  \"oracle_title_pab_information_visual_last_window\": {},\n  \"oracle_title_pab_information_visual_last_caller_rva\": {},\n",
            title_visual_suppress_installed,
            title_visual_suppressed_builds,
            title_visual_suppressed_builds != 0,
            title_visual_last_out_slot,
            title_visual_last_prev_out,
            title_visual_last_arg_rdx,
            title_visual_last_arg_r8,
            title_visual_last_caller_rva,
            title_visual_native_job,
            title_visual_native_window,
            title_visual_current_menu_id,
            title_visual_current_flags,
            title_visual_current_draw_bit_set,
            title_visual_render_suppress_installed,
            title_visual_render_suppressed_windows,
            title_visual_render_suppressed_windows != 0,
            title_visual_render_last_window,
            title_visual_render_last_flags_before,
            title_visual_render_last_flags_after,
            title_visual_render_last_caller_rva,
            TITLE_LOGO_BACK_VIEW_PARTS_NAME,
            TITLE_LOGO_RESOURCE_NAME,
            TITLE_LOGO_GFX_ROOT_DEPTH,
            TITLE_LOGO_GFX_ROOT_SPRITE_CHAR,
            TITLE_LOGO_GFX_MAIN_ASSET_CHAR,
            TITLE_LOGO_GFX_MAIN_ASSET_NAME,
            title_logo_back_view_parts,
            title_logo_back_view_parts_vtable,
            title_logo_gfx_frame,
            title_logo_gfx_alpha_mult_term,
            title_logo_gfx_visibility,
            title_logo_gfx_hide_calls,
            title_logo_gfx_hide_calls != 0,
            title_logo_gfx_hide_last_dialog,
            title_logo_gfx_hide_last_logo,
            title_logo_gfx_hide_last_caller_phase,
            title_logo_gfx_hide_last_requested_visible,
            title_press_start_gfx_hide_calls,
            title_press_start_gfx_hide_calls != 0,
            title_press_start_gfx_hide_last_dialog,
            title_press_start_gfx_hide_last_proxy,
            title_press_start_gfx_hide_last_context,
            title_press_start_gfx_hide_last_caller_phase,
            title_press_start_gfx_value,
            title_press_start_gfx_force_false_calls,
            title_press_start_gfx_force_false_calls != 0,
            title_press_start_gfx_force_false_last_value,
            title_press_start_gfx_force_false_last_requested,
            title_press_start_bind_hits,
            title_press_start_bind_hits != 0,
            title_press_start_bind_last_parent,
            title_press_start_bind_last_out,
            title_press_start_bind_last_name,
            title_press_start_bind_last_context,
            title_press_start_bind_hide_calls,
            title_press_start_bind_hide_calls != 0,
            title_profile_cover_bound_to_logo_surface,
            title_scaleform_bind_observer_hits,
            title_scaleform_bind_observer_systex_hits,
            title_scaleform_bind_observer_last_owner,
            title_scaleform_bind_observer_last_pair,
            title_scaleform_bind_observer_last_symbol_ptr,
            title_scaleform_bind_observer_last_target_ptr,
            now_loading_helper_hooks_installed,
            now_loading_helper_ctor_hits,
            now_loading_helper_update_hits,
            now_loading_helper_last_this,
            now_loading_helper_last_menu_index,
            now_loading_helper_last_replace_tex_info,
            now_loading_helper_last_requested_replace_tex_info,
            now_loading_helper_last_flags,
            title_logo_profile_summary,
            title_logo_profile_summary_ready,
            title_custom_cover_profile_render_refresh_calls,
            title_custom_cover_profile_render_refresh_last_profile_summary,
            title_custom_cover_profile_render_refresh_last_caller_phase,
            title_custom_cover_profile_select_builds,
            title_custom_cover_profile_select_builds != 0,
            title_custom_cover_profile_select_last_ret,
            title_custom_cover_profile_select_last_job,
            title_custom_cover_profile_select_last_caller_rva,
            title_custom_cover_run_calls,
            title_custom_cover_run_calls != 0,
            title_custom_cover_run_last_native_job,
            title_custom_cover_run_last_cover_job,
            title_custom_cover_run_last_cover_window,
            title_custom_cover_run_last_ret,
            TITLE_PAB_INFORMATION_VISUAL_NAME,
            title_pab_information_visual_builds,
            title_pab_information_visual_builds != 0,
            title_pab_information_visual_last_job,
            title_pab_information_visual_last_window,
            title_pab_information_visual_last_caller_rva
        ));
    }
    if let Ok(player) = unsafe { PlayerIns::local_player_mut() } {
        let pos = player.chr_ins.modules.physics.position;
        let grounded = player.chr_ins.modules.physics.standing_on_solid_ground;
        let block = player.current_block_id.0;
        let bp = player.block_position;
        let chr_model_ins_ptr = player.chr_ins.chr_model_ins.as_ptr() as usize;
        let chr_ctrl_ptr = player.chr_ins.chr_ctrl.as_ptr() as usize;
        let chr_draw_group_enabled = player.chr_ins.load_state.draw_group_enabled();
        let chr_render_group_enabled = player.chr_ins.chr_flags1c4.is_render_group_enabled();
        let chr_onscreen = player.chr_ins.chr_flags1c4.is_onscreen();
        let chr_enable_render = player.chr_ins.chr_flags1c5.enable_render();
        let player_render_ready = chr_model_ins_ptr != TITLE_OWNER_SCAN_START_ADDRESS
            && chr_ctrl_ptr != TITLE_OWNER_SCAN_START_ADDRESS
            && chr_draw_group_enabled
            && chr_render_group_enabled
            && chr_enable_render;
        body.push_str(&format!(
            "  \"oracle_player_present\": true,\n  \"oracle_havok_pos\": [{}, {}, {}],\n  \"oracle_grounded\": {},\n  \"oracle_block_id\": {},\n  \"oracle_block_id_valid\": {},\n  \"oracle_block_pos\": [{}, {}, {}],\n  \"oracle_chr_model_ins_present\": {},\n  \"oracle_chr_ctrl_present\": {},\n  \"oracle_chr_draw_group_enabled\": {},\n  \"oracle_chr_render_group_enabled\": {},\n  \"oracle_chr_onscreen\": {},\n  \"oracle_chr_enable_render\": {},\n  \"oracle_player_render_ready\": {},\n",
            pos.0,
            pos.1,
            pos.2,
            grounded,
            block,
            block != BLOCK_ID_NONE,
            bp.x,
            bp.y,
            bp.z,
            chr_model_ins_ptr != TITLE_OWNER_SCAN_START_ADDRESS,
            chr_ctrl_ptr != TITLE_OWNER_SCAN_START_ADDRESS,
            chr_draw_group_enabled,
            chr_render_group_enabled,
            chr_onscreen,
            chr_enable_render,
            player_render_ready
        ));
    } else {
        body.push_str("  \"oracle_player_present\": false,\n");
    }
}

/// Read-only, save-safe save-data snapshot for the parked-title disambiguation
/// (goal step 2): confirm GameDataMan (`game_data_man_ptr_or_null()`) and its `CS::ProfileSummary`
/// container (`+SLOT_MANAGER_CONTAINER_OFFSET`) are built cold, read the per-slot
/// active bytes the char-mount gate (`0x67b200`) checks via `byte[profile+slot+8]`,
/// and read the save-mgr deserialize-ready handle (`[mgr+0xdf0]`, the gate fast-path).
/// Every access is a fault-tolerant `ReadProcessMemory` -- no game-state mutation.
pub(crate) fn write_save_data_snapshot_telemetry(body: &mut String) {
    /// Null pointer sentinel for the chased singleton reads.
    const NULL_POINTER_VALUE: usize = 0;
    /// ProfileSummary per-slot active-byte array base (getter reads `byte[profile+slot+8]`).
    const PROFILE_SLOT_ACTIVE_ARRAY_OFFSET: usize = core::mem::size_of::<usize>();
    /// Save-mgr deserialize-ready handle (gate `0x67b200` fast-path `[mgr+0xdf0]`).
    const GAME_MAN_DESERIALIZE_READY_DF0_OFFSET: usize =
        core::mem::offset_of!(GameManSaveSnapshotLayout, deserialize_ready);

    let Ok(base) = crate::experiments::game_module_base() else {
        body.push_str("  \"save_snapshot_available\": false,\n");
        return;
    };

    let game_data_man = crate::game_data_man_ptr_or_null();
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
    let save_mgr = crate::game_man_ptr_or_null();
    let deserialize_ready = if save_mgr == NULL_POINTER_VALUE {
        None
    } else {
        unsafe {
            crate::experiments::safe_read_usize(save_mgr + GAME_MAN_DESERIALIZE_READY_DF0_OFFSET)
        }
    };

    // FD4 async-IO DRAIN subsystem (B step-3 lever check, read-only). The cold save-IO read
    // never drains because the queue-processing worker threads live in the global thread POOL
    // [0x144853048], NOT in the worker MANAGER. If the pool is NULL cold, cold-building it
    // (0x14240afe0) is the untested save-safe lever; if non-null cold, the read fails elsewhere.
    // CORRECTION (autoresearch 2026-06-18): the "stream task" read is actually
    // upstream's `runtime_heap_allocator` (DLAllocator) -- always non-null, so the
    // `fd4_stream_task_present` signal is meaningless. Resolve it through fromsoftware-rs.
    const FD4_IO_POOL_RVA: usize = RuntimeGlobalRva::Fd4IoPool as usize;
    const FD4_IO_WORKER_MANAGER_RVA: usize = RuntimeGlobalRva::Fd4IoWorkerManager as usize;
    const IO_DEVICE_SINGLETON_RVA: usize = RuntimeGlobalRva::IoDeviceSingleton as usize;
    const IO_DEVICE_INFLIGHT_10_OFFSET: usize =
        core::mem::offset_of!(IoDeviceSnapshotLayout, inflight);
    const IO_DEVICE_REQHANDLE_20_OFFSET: usize =
        core::mem::offset_of!(IoDeviceSnapshotLayout, request_handle);
    let io_pool = unsafe { crate::experiments::safe_read_usize(base + FD4_IO_POOL_RVA) }
        .unwrap_or(NULL_POINTER_VALUE);
    let io_worker_manager =
        unsafe { crate::experiments::safe_read_usize(base + FD4_IO_WORKER_MANAGER_RVA) }
            .unwrap_or(NULL_POINTER_VALUE);
    let stream_task = crate::runtime_heap_allocator_ptr_or_null();
    let io_device = unsafe { crate::experiments::safe_read_usize(base + IO_DEVICE_SINGLETON_RVA) }
        .unwrap_or(NULL_POINTER_VALUE);
    let io_inflight = if io_device == NULL_POINTER_VALUE {
        None
    } else {
        unsafe { crate::experiments::safe_read_usize(io_device + IO_DEVICE_INFLIGHT_10_OFFSET) }
    };
    let io_reqhandle = if io_device == NULL_POINTER_VALUE {
        None
    } else {
        unsafe { crate::experiments::safe_read_usize(io_device + IO_DEVICE_REQHANDLE_20_OFFSET) }
    };

    body.push_str("  \"save_snapshot_available\": true,\n");
    body.push_str(&format!(
        "  \"fd4_io_pool_present\": {},\n",
        io_pool != NULL_POINTER_VALUE
    ));
    body.push_str(&format!(
        "  \"fd4_io_worker_manager_present\": {},\n",
        io_worker_manager != NULL_POINTER_VALUE
    ));
    body.push_str(&format!(
        "  \"fd4_stream_task_present\": {},\n",
        stream_task != NULL_POINTER_VALUE
    ));
    body.push_str(&format!(
        "  \"io_device_present\": {},\n",
        io_device != NULL_POINTER_VALUE
    ));
    body.push_str(&format!(
        "  \"io_device_inflight_10\": {},\n",
        io_inflight.map_or_else(|| "null".to_owned(), |value| format!("\"{value:#x}\""))
    ));
    body.push_str(&format!(
        "  \"io_device_reqhandle_20\": {},\n",
        io_reqhandle.map_or_else(|| "null".to_owned(), |value| format!("\"{value:#x}\""))
    ));
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
    // Corrupted-save SEMAPHORE: the GR_System_Message id (0 = none) the game fetched for a "save data
    // is corrupted" dialog -- our RAM-read detector for that popup (the gold save was read but rejected
    // on validate/write). See CORRUPTED_SAVE_MSG_IDS.
    body.push_str(&format!(
        "  \"oracle_corrupted_save_seen_id\": {},\n",
        crate::experiments::CORRUPTED_SAVE_SEEN_ID.load(Ordering::SeqCst)
    ));
    // PRIVACY-POLICY SEMAPHORE (privacy-policy-gated-on-character-presence-CONFIRMED-2026-06-23):
    // the Bandai-Namco PRIVACY POLICY boot screen is gated SOLELY on character presence -- it appears
    // iff the active profile summary is loaded but reports ZERO active slots (no character). This is
    // 1:1 with the on-screen privacy policy. When a gold load is EXPECTED (not telemetry-only), a true
    // value is the BAD blocker: the gold did NOT load, so the main menu / Continue is never reached
    // (the privacy-policy gate sits in front of it). On a real load this is false (char present ->
    // policy skipped). This is the in-process detector that was MISSING when the screen blocked runs.
    let privacy_policy_gate = profile_summary != NULL_POINTER_VALUE
        && slot_active_bytes == Some(0)
        && !crate::experiments::save_override_telemetry_only();
    body.push_str(&format!(
        "  \"oracle_privacy_policy_gate\": {privacy_policy_gate},\n"
    ));
    // SPLASH-SKIP SEMAPHORE (splash-skip-correctness): the only failure mode of the BeginLogo logo
    // skip is the je->jg branch flip at base+SPLASH_SKIP_RVA not being live (never applied, or
    // reverted by Arxan / another mod). So read that .text byte directly each telemetry frame:
    //   jg (0x7f) = patch LIVE -> STEP_BeginLogo falls through past the ESRB/illegal-copy logo build
    //               (the logos are skipped, the title advances SetState(2)->(3) without them);
    //   je (0x74) = UNPATCHED -> splash will play;
    //   anything else = corrupted/reverted -> splash-skip is BROKEN.
    // apply_splash_skip runs at DLL attach (before the title runs state 2), so by the time telemetry
    // writes (at the title/menu) a live jg means the skip already executed this boot. This is the
    // in-process detector that was MISSING for "are we correctly skipping the splash screens".
    if let Ok(base) = crate::experiments::game_module_base() {
        let splash_byte =
            unsafe { crate::experiments::safe_read_u8(base + crate::SPLASH_SKIP_RVA) }.unwrap_or(0);
        body.push_str(&format!(
            "  \"oracle_splash_skip_armed\": {},\n  \"oracle_splash_skip_patch_byte\": \"{:#x}\",\n",
            splash_byte == crate::SPLASH_SKIP_REPLACEMENT_JG,
            splash_byte
        ));
    }
    // oracle_continue_ready_stage / _scan_node_hits / _dialog_vt REMOVED 2026-06-24: they were the
    // diagnostic for the native_continue Continue-node scan (CONTINUE_READY_STAGE/SCAN_NODE_HITS/
    // DIALOG_VT_SEEN), which was ripped out as dead code -- the scan never found the node and the
    // zero-input load fires via pab-advance + title-accept-byte instead.
}

pub(crate) fn telemetry_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_TELEMETRY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("er-effects-telemetry.json"))
}

pub(crate) fn write_policy_oracle_snapshot(reason: &str) {
    let path = telemetry_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let seamless_loaded = seamless_coop_loaded();
    let policy_total_builds = POLICY_TOS_TITLE_TOTAL_BUILDS.load(Ordering::SeqCst);
    let policy_any_seen = policy_total_builds != MENU_TRACE_UNSEEN_SEQ;
    let msgbox_total_builds = MSGBOX_TOTAL_BUILDS.load(Ordering::SeqCst);
    let msgbox_any_seen = msgbox_total_builds != MENU_TRACE_UNSEEN_SEQ;
    let server_status_total_seen = SERVER_STATUS_TOTAL_SEEN.load(Ordering::SeqCst);
    let server_status_any_seen = server_status_total_seen != MENU_TRACE_UNSEEN_SEQ;
    let body = format!(
        "{{\n  \"player_available\": false,\n  \"player_seen\": false,\n  \"runtime_mode\": \"{}\",\n  \"seamless_coop_loaded\": {},\n  \"telemetry_source\": \"policy_oracle_snapshot\",\n  \"telemetry_snapshot_reason\": \"{}\",\n  \"simulated_button_presses_total\": 0,\n  \"oracle_msgbox_total_builds\": {},\n  \"oracle_msgbox_any_seen\": {},\n  \"oracle_msgbox_builder_args\": [{}, {}, {}, {}],\n  \"oracle_policy_window_total_builds\": {},\n  \"oracle_policy_window_any_seen\": {},\n  \"oracle_policy_window_ptr\": {},\n  \"oracle_policy_window_vtable\": {},\n  \"oracle_policy_window_stack_arg0\": {},\n  \"oracle_policy_window_backing_flag_ptr\": {},\n  \"oracle_policy_window_stored_backing_flag_ptr\": {},\n  \"oracle_policy_window_backing_flag_value\": {},\n  \"oracle_policy_window_requested_flag_value\": {},\n  \"oracle_policy_window_caller_rva\": {},\n  \"oracle_policy_ctor_wrapper_hits\": {},\n  \"oracle_policy_ctor_wrapper_caller_rva\": {},\n  \"oracle_policy_selector_wrapper_hits\": {},\n  \"oracle_policy_selector_wrapper_caller_rva\": {},\n  \"oracle_policy_selector_ctor_hits\": {},\n  \"oracle_policy_selector_ctor_requested_flag_value\": {},\n  \"oracle_policy_selector_ctor_caller_rva\": {},\n  \"oracle_policy_status_predicate_hits\": {},\n  \"oracle_policy_status_predicate_caller_rva\": {},\n  \"oracle_policy_flag_setter_hits\": {},\n  \"oracle_policy_flag_setter_caller_rva\": {},\n  \"oracle_server_status_total_seen\": {},\n  \"oracle_server_status_any_seen\": {},\n  \"oracle_server_status_state\": {},\n  \"oracle_server_status_text_id\": {}\n}}\n",
        if seamless_loaded {
            RUNTIME_MODE_SEAMLESS
        } else {
            RUNTIME_MODE_VANILLA_OR_UNKNOWN
        },
        seamless_loaded,
        json_escape(reason),
        msgbox_total_builds,
        msgbox_any_seen,
        MSGBOX_LAST_ARG_RCX.load(Ordering::SeqCst),
        MSGBOX_LAST_ARG_RDX.load(Ordering::SeqCst),
        MSGBOX_LAST_ARG_R8.load(Ordering::SeqCst),
        MSGBOX_LAST_ARG_R9.load(Ordering::SeqCst),
        policy_total_builds,
        policy_any_seen,
        POLICY_TOS_TITLE_LAST_THIS.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_LAST_VTABLE.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_LAST_STACK_ARG0.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_LAST_BACKING_FLAG_PTR.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_LAST_STORED_BACKING_FLAG_PTR.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_LAST_BACKING_FLAG_VALUE.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_LAST_REQUESTED_FLAG_VALUE.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_LAST_CALLER_RVA.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_WRAPPER_HITS.load(Ordering::SeqCst),
        POLICY_TOS_TITLE_WRAPPER_LAST_CALLER_RVA.load(Ordering::SeqCst),
        POLICY_TOS_SELECTOR_WRAPPER_HITS.load(Ordering::SeqCst),
        POLICY_TOS_SELECTOR_WRAPPER_LAST_CALLER_RVA.load(Ordering::SeqCst),
        POLICY_TOS_SELECTOR_CTOR_HITS.load(Ordering::SeqCst),
        POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_VALUE.load(Ordering::SeqCst),
        POLICY_TOS_SELECTOR_CTOR_LAST_CALLER_RVA.load(Ordering::SeqCst),
        POLICY_TOS_STATUS_HITS.load(Ordering::SeqCst),
        POLICY_TOS_STATUS_LAST_CALLER_RVA.load(Ordering::SeqCst),
        POLICY_TOS_FLAG_SETTER_HITS.load(Ordering::SeqCst),
        POLICY_TOS_FLAG_SETTER_LAST_CALLER_RVA.load(Ordering::SeqCst),
        server_status_total_seen,
        server_status_any_seen,
        SERVER_STATUS_LAST_STATE.load(Ordering::SeqCst),
        SERVER_STATUS_LAST_TEXT_ID.load(Ordering::SeqCst)
    );
    let tmp_path = path.with_extension("json.tmp");
    if fs::write(&tmp_path, body).is_ok() {
        let _ = fs::rename(tmp_path, path);
    }
    write_bootstrap_event(BOOTSTRAP_EVENT_POLICY_TELEMETRY_SNAPSHOT, reason);
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
            character if character.is_control() => format!("\\u{:04x}", character as u32)
                .chars()
                .collect::<Vec<_>>(),
            character => vec![character],
        })
        .collect()
}

pub(crate) fn crash_log_path() -> PathBuf {
    std::env::var("ER_EFFECTS_CRASH_LOG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            // CANONICAL name `er-effects-crash-log.txt` -- the SAME file the crash-logger enable
            // sentinel (crash_logger_enabled) and the probe's per-run truncation use. The prior
            // default `er-effects-crash.log` silently diverged from those, so the probe never
            // cleared the real crash log (it accumulated across runs) and readers checked the wrong
            // file (observed 2026-06-22, cost a debug cycle). bd log-output-paths-consolidation.
            game_directory_path()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("er-effects-crash-log.txt")
        })
}

/// Monotonic process-attach epoch for self-describing DLL logs. Lazily set on the FIRST log call
/// (close to DLL_PROCESS_ATTACH in practice), so every emitted line carries `[+<elapsed_ms>ms] `
/// measured from that common start -- making ordering and gaps obvious in raw logs without needing
/// the bash launch T0. Mirrors the `TIMELINE_EPOCH` pattern; `Instant` is QPC-backed and works under
/// wine. Kept lock-light: one short lock that returns a u128, never held across the file write.
static PROCESS_LOG_EPOCH: Mutex<Option<Instant>> = Mutex::new(None);

/// Elapsed milliseconds since the process-log epoch (lazily anchored on first call). Cheap: a single
/// short-lived lock, poison-tolerant, no file IO under the lock.
fn process_log_elapsed_ms() -> u128 {
    let mut guard = match PROCESS_LOG_EPOCH.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let epoch = guard.get_or_insert_with(Instant::now);
    epoch.elapsed().as_millis()
}

pub(crate) fn append_crash_log(args: std::fmt::Arguments<'_>) {
    use std::io::Write;
    let ms = process_log_elapsed_ms();
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(crash_log_path())
    {
        let _ = writeln!(file, "[+{ms}ms] {args}");
    }
}

pub(crate) fn append_autoload_debug(args: std::fmt::Arguments<'_>) {
    use std::io::Write;

    let ms = process_log_elapsed_ms();
    let path = std::env::var("ER_EFFECTS_AUTOLOAD_DEBUG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("er-effects-autoload-debug.log"));
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "[+{ms}ms] {args}");
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
