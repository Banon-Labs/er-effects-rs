//! Startup-hook glue for product loading flows, save-picker/quit-menu features, and
//! runtime diagnostics.
//!
//! This module is being converted from one flat pasted namespace into ownership modules.
//! Compatibility `pub(crate) use` shims preserve the current private helper visibility while
//! each ownership cluster is moved behind a real Rust module.
//!
//! Ownership groups:
//! * `loading_cover/` -- title/loading-cover/product boot resources that stay with the
//!   product until the loading-cover extraction resumes.
//! * `save_picker/` -- product (A), the boot missing-save picker and its shared OS dialog
//!   mechanism.
//! * `quit_menu/` -- product (B), the customized System>Quit menu, Load Profile rows,
//!   Save Game flow, destination picker, dim cover, and ownership fixes.
//! * `diagnostics/` -- agent/runtime-probe traces and diagnostics that must not be
//!   dragged into standalone feature crates.
//!
//! The loading-screen portrait capture pipeline + stats producer
//! (dlstring_lookat_math, lookat_bone_hooks, lookat_stage_camera, stats_loading_text)
//! moved to the `er-loading-portrait` crate (portrait crate split); the glob shim below
//! re-exports it so every remaining flat-namespace reference keeps compiling unchanged.

#![allow(unused_imports)]

pub(crate) use er_loading_portrait::*;

// Shared imports preserved from the former flat startup-hook namespace.
// Child modules glob-import their parent while this refactor exposes real module boundaries.
pub(crate) use crate::input_blocker::{InputBlocker, InputFlags};
pub(crate) use crate::mh::{
    MH_ApplyQueued, MH_Initialize, MH_QueueDisableHook, MH_QueueEnableHook, MH_STATUS, MhHook,
};
pub(crate) use crate::*;
pub(crate) use crate::{crashlog::*, ffi::*, hooks::*, telemetry::*};
pub(crate) use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
pub(crate) use er_quit_menu::rows::{
    QuitInputKind, QuitRow, QuitRowAmbiguity, QuitRowDiscriminator, QuitRowFacts, QuitRowLabel,
    QuitRowVerdict, resolve_quit_row,
};
pub(crate) use er_quit_menu::save_dest_identity::*;
pub(crate) use er_quit_menu::save_flow_boxes::{
    SAVE_FLOW_BOX_NONE, SAVE_FLOW_BOX_OVERWRITE_FILE, SaveFlowButton, SaveFlowDecision,
    save_flow_box_add_order, save_flow_box_label, save_flow_box_yes_index,
};
pub(crate) use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
pub(crate) use er_save_picker::os_dialog::{OsPickAbort, no_picker_cover, os_pick_validated};
pub(crate) use er_save_picker::{PickerCover, PickerIntent};
pub(crate) use er_save_picker::{SaveSlotInfo, parse_save_character_slots};
pub(crate) use er_telemetry::counters::CORRUPTED_SAVE_SEEN_COUNT;
pub(crate) use er_telemetry::counters::GAME_TASK_TICKS_TOTAL;
pub(crate) use er_telemetry::counters::GR_SYSMSG_LOG_COUNT;
pub(crate) use er_telemetry::counters::GR_SYSMSG_LOG_INSTALLED;
pub(crate) use er_telemetry::counters::GR_SYSMSG_LOG_ORIG;
pub(crate) use er_telemetry::counters::NETWORK_CHECK_SHORTCIRCUIT_COUNT;
pub(crate) use er_telemetry::counters::NETWORK_CHECK_SHORTCIRCUIT_INSTALLED;
pub(crate) use er_telemetry::counters::OPTIONS_02_040_QUIT4_RUNTIME_FAILURES;
pub(crate) use er_telemetry::counters::OPTIONS_02_040_QUIT4_RUNTIME_SERVES;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_BAD_FRAMES;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_BAD_FRAMES_TOTAL;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_BAD_MASK;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_CAPTURE_EFFECTIVE_ID;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_CAPTURE_VERDICT;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_FIRST_EFFECTIVE_ID;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_FIRST_UNK0;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_FIRST_UNKD4;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_FIRST_UNKD8;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_ORACLE_SLOT;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_ORACLE_WINDOW;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_RECORD_PARAM_ID;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_SAMPLED_FRAMES;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_WINDOWS_BAD;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_WINDOWS_SAMPLED;
pub(crate) use er_telemetry::counters::PORTRAIT_FACE_IDENTITY_CHECKS;
pub(crate) use er_telemetry::counters::PORTRAIT_FACE_IDENTITY_MISMATCHES;
pub(crate) use er_telemetry::counters::PROFILE_STATS_PREVIEW_ROW_CURSOR;
pub(crate) use er_telemetry::counters::SAVE_DEST_CANCEL_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_COMMIT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_COMMIT_FAIL;
pub(crate) use er_telemetry::counters::SAVE_DEST_COMMIT_PENDING;
pub(crate) use er_telemetry::counters::SAVE_DEST_LIVE_BAK_MUTATED;
pub(crate) use er_telemetry::counters::SAVE_DEST_LIVE_FILE_MUTATED;
pub(crate) use er_telemetry::counters::SAVE_DEST_LIVE_OVERWRITE_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_OPEN_PICKER_PENDING;
pub(crate) use er_telemetry::counters::SAVE_DEST_PICKER_OPEN_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_PICKER_OPEN_RETRY_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_REDIRECT_ARMED;
pub(crate) use er_telemetry::counters::SAVE_DEST_REDIRECT_HITS;
pub(crate) use er_telemetry::counters::SAVE_DEST_SEED_FAIL_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_SEEDED_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_EXISTING_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_NEW_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_STRUCTURE_OK;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_WRITTEN_OK;
pub(crate) use er_telemetry::counters::SAVE_PICKER_ACTION_OBJ;
pub(crate) use er_telemetry::counters::SAVE_PICKER_BOOT_GAME_TICKS_AT_ANSWER;
pub(crate) use er_telemetry::counters::SAVE_PICKER_BOOT_GAME_TICKS_AT_OPEN;
pub(crate) use er_telemetry::counters::SAVE_PICKER_BOOT_TELEMETRY_FLUSHED;
pub(crate) use er_telemetry::counters::SAVE_PICKER_CANCEL_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_DEST_MODE;
pub(crate) use er_telemetry::counters::SAVE_PICKER_MODE_ACTIVE;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OPEN_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OPEN_SLOTS_PENDING;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_CANCEL_EXIT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_DEFER_TICKS;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_EXIT_PERFORMED;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_FALLBACK_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_OPEN_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_PICK_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_STATE;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_DIALOG_OPEN;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_TICKS_FROZEN;
pub(crate) use er_telemetry::counters::SAVE_PICKER_PICK_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_PICK_REJECT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_REBUILD_PENDING_DIALOG;
pub(crate) use er_telemetry::counters::SAVE_PICKER_REOPEN_PENDING;
pub(crate) use er_telemetry::counters::SAVE_PICKER_REPOPULATE_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_RESUBMIT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_STAGED_ROW_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_SURFACE;
pub(crate) use er_telemetry::counters::SAVE_PICKER_SYSTEM_DIALOG;
pub(crate) use er_telemetry::counters::SHOW_PROGRESS_SHORTCIRCUIT_COUNT;
pub(crate) use er_telemetry::counters::SHOW_PROGRESS_SHORTCIRCUIT_INSTALLED;
pub(crate) use er_telemetry::counters::SHOW_PROGRESS_TYPE_LOGGED;
pub(crate) use er_telemetry::counters::SQ_REPRO_PANE_BUILD_TRIED;
pub(crate) use er_telemetry::counters::SQ_REPRO_ROUTE_FIRED;
pub(crate) use er_telemetry::counters::SQ_REPRO_ROWNAV_BASE;
pub(crate) use er_telemetry::counters::SQ_REPRO_TAB_BASELINE;
pub(crate) use er_telemetry::counters::SQ_REPRO_TAB_DISCOVERED;
pub(crate) use er_telemetry::counters::SYSTEM_QUIT_SAVE_SWAP_POLL_TICK;
pub(crate) use er_telemetry::counters::TESTNET_FF_FIRED_EPOCH;
pub(crate) use er_telemetry::counters::TESTNET_FF_LAST_MMS;
pub(crate) use er_telemetry::counters::TESTNET_FF_STUCK_FRAMES;
pub(crate) use er_telemetry::counters::TITLE_OPEN_MENU_SUPPRESS_INSTALLED;
pub(crate) use er_telemetry::counters::TITLE_OPEN_MENU_SUPPRESSED_COUNT;
pub(crate) use er_telemetry::counters::WINRECONFIG_CHANGE_DISPLAY_CALLS;
pub(crate) use er_telemetry::counters::WINRECONFIG_CHANGE_DISPLAY_ORIG;
pub(crate) use er_telemetry::counters::WINRECONFIG_CREATE_WINDOW_CALLS;
pub(crate) use er_telemetry::counters::WINRECONFIG_CREATE_WINDOW_ORIG;
pub(crate) use er_telemetry::counters::WINRECONFIG_EARLY_APPLY_MS;
pub(crate) use er_telemetry::counters::WINRECONFIG_EARLY_APPLY_RECT;
pub(crate) use er_telemetry::counters::WINRECONFIG_EARLY_APPLY_RESULT;
pub(crate) use er_telemetry::counters::WINRECONFIG_LAST_SET_POS_SIZE;
pub(crate) use er_telemetry::counters::WINRECONFIG_MOVE_WINDOW_CALLS;
pub(crate) use er_telemetry::counters::WINRECONFIG_MOVE_WINDOW_ORIG;
pub(crate) use er_telemetry::counters::WINRECONFIG_SET_WINDOW_LONG_CALLS;
pub(crate) use er_telemetry::counters::WINRECONFIG_SET_WINDOW_LONG_ORIG;
pub(crate) use er_telemetry::counters::WINRECONFIG_SET_WINDOW_POS_CALLS;
pub(crate) use er_telemetry::counters::WINRECONFIG_SET_WINDOW_POS_ORIG;
pub(crate) use er_telemetry::counters::{
    SAVE_PICKER_OS_CANCEL_COUNT, SAVE_PICKER_OS_CLOSED_WITH_PATH, SAVE_PICKER_OS_ERROR_COUNT,
    SAVE_PICKER_OS_LAST_ERROR, SAVE_PICKER_OS_LAST_REJECT_REASON, SAVE_PICKER_OS_OPEN_COUNT,
    SAVE_PICKER_OS_OWNER_HWND, SAVE_PICKER_OS_OWNER_IS_COVER, SAVE_PICKER_OS_REJECT_COUNT,
    SAVE_PICKER_OS_REOPEN_COUNT, SAVE_PICKER_OS_REOPEN_EXHAUSTED, SAVE_PICKER_OS_SAVELIKE_OPENS,
};
pub(crate) use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
pub(crate) use std::os::windows::ffi::OsStrExt as _;
pub(crate) use std::{
    ffi::{CStr, c_void},
    fmt::Write as _,
    fs,
    path::Path,
    sync::{
        Arc, Mutex, Once, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, UNIX_EPOCH},
};
pub(crate) use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM},
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
    core::{BOOL, PCSTR, PCWSTR},
};
mod loading_cover;
pub(crate) use loading_cover::*;

mod quit_menu;
pub(crate) use quit_menu::*;

mod save_picker;
pub(crate) use save_picker::*;

mod diagnostics;
pub(crate) use diagnostics::*;
