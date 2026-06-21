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
        "  \"product_autoload_armed\": {},\n  \"product_core_autoload_ticks\": {},\n  \"product_core_ready_blocks\": {},\n  \"product_core_ready_successes\": {},\n  \"product_core_last_phase\": {},\n  \"product_core_ready_blocker\": \"{}\",\n  \"title_owner_scan_attempts\": {},\n  \"title_owner_scan_vtable_hits\": {},\n  \"title_owner_scan_table_rejects\": {},\n  \"title_owner_scan_state_rejects\": {},\n  \"title_owner_scan_cached_owner\": {},\n  \"title_owner_scan_last_candidate\": {},\n  \"title_owner_scan_last_table\": {},\n  \"title_owner_scan_last_state\": {},\n",
        product_autoload_enabled(),
        PRODUCT_CORE_AUTOLOAD_TICKS.load(Ordering::SeqCst),
        PRODUCT_CORE_READY_BLOCKS.load(Ordering::SeqCst),
        PRODUCT_CORE_READY_SUCCESSES.load(Ordering::SeqCst),
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
        "  \"oracle_continue_task_node\": {},\n  \"oracle_continue_member_node\": {},\n",
        format_optional_ptr(continue_task_node),
        format_optional_ptr(continue_member_node)
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
        "  \"oracle_continue_phase\": {},\n  \"oracle_continue_expected_slot\": {},\n  \"oracle_continue_deser_fired\": {},\n  \"oracle_continue_confirmed\": {},\n  \"oracle_continue_mount_c30\": {},\n  \"oracle_continue_guard_waits\": {},\n",
        FULLREAD_PHASE.load(Ordering::SeqCst),
        OWN_STEPPER_EXPECTED_SLOT.load(Ordering::SeqCst),
        OWN_STEPPER_DESER_FIRED.load(Ordering::SeqCst),
        OWN_STEPPER_CONFIRMED.load(Ordering::SeqCst),
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
