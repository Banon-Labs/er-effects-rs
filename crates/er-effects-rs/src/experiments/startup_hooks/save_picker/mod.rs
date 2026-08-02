// Product (A) boot missing-save picker modules.
#![allow(unused_imports)]

// Shared imports preserved from the former flat startup-hook namespace for child modules.
use crate::input_blocker::{InputBlocker, InputFlags};
use crate::mh::{
    MH_ApplyQueued, MH_Initialize, MH_QueueDisableHook, MH_QueueEnableHook, MH_STATUS, MhHook,
};
use crate::*;
use crate::{crashlog::*, ffi::*, hooks::*, telemetry::*};
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_quit_menu::rows::{
    QuitInputKind, QuitRow, QuitRowAmbiguity, QuitRowDiscriminator, QuitRowFacts, QuitRowLabel,
    QuitRowVerdict, resolve_quit_row,
};
use er_quit_menu::save_dest_identity::*;
use er_quit_menu::save_flow_boxes::{
    SAVE_FLOW_BOX_NONE, SAVE_FLOW_BOX_OVERWRITE_FILE, SaveFlowButton, SaveFlowDecision,
    save_flow_box_add_order, save_flow_box_label, save_flow_box_yes_index,
};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use er_save_picker::os_dialog::{OsPickAbort, no_picker_cover, os_pick_validated};
use er_save_picker::{PickerCover, PickerIntent};
use er_save_picker::{SaveSlotInfo, parse_save_character_slots};
use er_telemetry::counters::CORRUPTED_SAVE_SEEN_COUNT;
use er_telemetry::counters::GAME_TASK_TICKS_TOTAL;
use er_telemetry::counters::GR_SYSMSG_LOG_COUNT;
use er_telemetry::counters::GR_SYSMSG_LOG_INSTALLED;
use er_telemetry::counters::GR_SYSMSG_LOG_ORIG;
use er_telemetry::counters::NETWORK_CHECK_SHORTCIRCUIT_COUNT;
use er_telemetry::counters::NETWORK_CHECK_SHORTCIRCUIT_INSTALLED;
use er_telemetry::counters::OPTIONS_02_040_QUIT4_RUNTIME_FAILURES;
use er_telemetry::counters::OPTIONS_02_040_QUIT4_RUNTIME_SERVES;
use er_telemetry::counters::PORTRAIT_EQUIP_BAD_FRAMES;
use er_telemetry::counters::PORTRAIT_EQUIP_BAD_FRAMES_TOTAL;
use er_telemetry::counters::PORTRAIT_EQUIP_BAD_MASK;
use er_telemetry::counters::PORTRAIT_EQUIP_CAPTURE_EFFECTIVE_ID;
use er_telemetry::counters::PORTRAIT_EQUIP_CAPTURE_VERDICT;
use er_telemetry::counters::PORTRAIT_EQUIP_FIRST_EFFECTIVE_ID;
use er_telemetry::counters::PORTRAIT_EQUIP_FIRST_UNK0;
use er_telemetry::counters::PORTRAIT_EQUIP_FIRST_UNKD4;
use er_telemetry::counters::PORTRAIT_EQUIP_FIRST_UNKD8;
use er_telemetry::counters::PORTRAIT_EQUIP_ORACLE_SLOT;
use er_telemetry::counters::PORTRAIT_EQUIP_ORACLE_WINDOW;
use er_telemetry::counters::PORTRAIT_EQUIP_RECORD_PARAM_ID;
use er_telemetry::counters::PORTRAIT_EQUIP_SAMPLED_FRAMES;
use er_telemetry::counters::PORTRAIT_EQUIP_WINDOWS_BAD;
use er_telemetry::counters::PORTRAIT_EQUIP_WINDOWS_SAMPLED;
use er_telemetry::counters::PORTRAIT_FACE_IDENTITY_CHECKS;
use er_telemetry::counters::PORTRAIT_FACE_IDENTITY_MISMATCHES;
use er_telemetry::counters::PROFILE_STATS_PREVIEW_ROW_CURSOR;
use er_telemetry::counters::SAVE_DEST_CANCEL_COUNT;
use er_telemetry::counters::SAVE_DEST_COMMIT_COUNT;
use er_telemetry::counters::SAVE_DEST_COMMIT_FAIL;
use er_telemetry::counters::SAVE_DEST_COMMIT_PENDING;
use er_telemetry::counters::SAVE_DEST_LIVE_BAK_MUTATED;
use er_telemetry::counters::SAVE_DEST_LIVE_FILE_MUTATED;
use er_telemetry::counters::SAVE_DEST_LIVE_OVERWRITE_COUNT;
use er_telemetry::counters::SAVE_DEST_OPEN_PICKER_PENDING;
use er_telemetry::counters::SAVE_DEST_PICKER_OPEN_COUNT;
use er_telemetry::counters::SAVE_DEST_PICKER_OPEN_RETRY_COUNT;
use er_telemetry::counters::SAVE_DEST_REDIRECT_ARMED;
use er_telemetry::counters::SAVE_DEST_REDIRECT_HITS;
use er_telemetry::counters::SAVE_DEST_SEED_FAIL_COUNT;
use er_telemetry::counters::SAVE_DEST_SEEDED_COUNT;
use er_telemetry::counters::SAVE_DEST_TARGET_EXISTING_COUNT;
use er_telemetry::counters::SAVE_DEST_TARGET_NEW_COUNT;
use er_telemetry::counters::SAVE_DEST_TARGET_STRUCTURE_OK;
use er_telemetry::counters::SAVE_DEST_TARGET_WRITTEN_OK;
use er_telemetry::counters::SAVE_PICKER_ACTION_OBJ;
use er_telemetry::counters::SAVE_PICKER_BOOT_GAME_TICKS_AT_ANSWER;
use er_telemetry::counters::SAVE_PICKER_BOOT_GAME_TICKS_AT_OPEN;
use er_telemetry::counters::SAVE_PICKER_BOOT_TELEMETRY_FLUSHED;
use er_telemetry::counters::SAVE_PICKER_CANCEL_COUNT;
use er_telemetry::counters::SAVE_PICKER_DEST_MODE;
use er_telemetry::counters::SAVE_PICKER_MODE_ACTIVE;
use er_telemetry::counters::SAVE_PICKER_OPEN_COUNT;
use er_telemetry::counters::SAVE_PICKER_OPEN_SLOTS_PENDING;
use er_telemetry::counters::SAVE_PICKER_OS_BOOT_CANCEL_EXIT_COUNT;
use er_telemetry::counters::SAVE_PICKER_OS_BOOT_DEFER_TICKS;
use er_telemetry::counters::SAVE_PICKER_OS_BOOT_EXIT_PERFORMED;
use er_telemetry::counters::SAVE_PICKER_OS_BOOT_FALLBACK_COUNT;
use er_telemetry::counters::SAVE_PICKER_OS_BOOT_OPEN_COUNT;
use er_telemetry::counters::SAVE_PICKER_OS_BOOT_PICK_COUNT;
use er_telemetry::counters::SAVE_PICKER_OS_BOOT_STATE;
use er_telemetry::counters::SAVE_PICKER_OS_DIALOG_OPEN;
use er_telemetry::counters::SAVE_PICKER_OS_TICKS_FROZEN;
use er_telemetry::counters::SAVE_PICKER_PICK_COUNT;
use er_telemetry::counters::SAVE_PICKER_PICK_REJECT_COUNT;
use er_telemetry::counters::SAVE_PICKER_REBUILD_PENDING_DIALOG;
use er_telemetry::counters::SAVE_PICKER_REOPEN_PENDING;
use er_telemetry::counters::SAVE_PICKER_REPOPULATE_COUNT;
use er_telemetry::counters::SAVE_PICKER_RESUBMIT_COUNT;
use er_telemetry::counters::SAVE_PICKER_STAGED_ROW_COUNT;
use er_telemetry::counters::SAVE_PICKER_SURFACE;
use er_telemetry::counters::SAVE_PICKER_SYSTEM_DIALOG;
use er_telemetry::counters::SHOW_PROGRESS_SHORTCIRCUIT_COUNT;
use er_telemetry::counters::SHOW_PROGRESS_SHORTCIRCUIT_INSTALLED;
use er_telemetry::counters::SHOW_PROGRESS_TYPE_LOGGED;
use er_telemetry::counters::SQ_REPRO_PANE_BUILD_TRIED;
use er_telemetry::counters::SQ_REPRO_ROUTE_FIRED;
use er_telemetry::counters::SQ_REPRO_ROWNAV_BASE;
use er_telemetry::counters::SQ_REPRO_TAB_BASELINE;
use er_telemetry::counters::SQ_REPRO_TAB_DISCOVERED;
use er_telemetry::counters::SYSTEM_QUIT_SAVE_SWAP_POLL_TICK;
use er_telemetry::counters::TESTNET_FF_FIRED_EPOCH;
use er_telemetry::counters::TESTNET_FF_LAST_MMS;
use er_telemetry::counters::TESTNET_FF_STUCK_FRAMES;
use er_telemetry::counters::TITLE_OPEN_MENU_SUPPRESS_INSTALLED;
use er_telemetry::counters::TITLE_OPEN_MENU_SUPPRESSED_COUNT;
use er_telemetry::counters::WINRECONFIG_CHANGE_DISPLAY_CALLS;
use er_telemetry::counters::WINRECONFIG_CHANGE_DISPLAY_ORIG;
use er_telemetry::counters::WINRECONFIG_CREATE_WINDOW_CALLS;
use er_telemetry::counters::WINRECONFIG_CREATE_WINDOW_ORIG;
use er_telemetry::counters::WINRECONFIG_EARLY_APPLY_MS;
use er_telemetry::counters::WINRECONFIG_EARLY_APPLY_RECT;
use er_telemetry::counters::WINRECONFIG_EARLY_APPLY_RESULT;
use er_telemetry::counters::WINRECONFIG_LAST_SET_POS_SIZE;
use er_telemetry::counters::WINRECONFIG_MOVE_WINDOW_CALLS;
use er_telemetry::counters::WINRECONFIG_MOVE_WINDOW_ORIG;
use er_telemetry::counters::WINRECONFIG_SET_WINDOW_LONG_CALLS;
use er_telemetry::counters::WINRECONFIG_SET_WINDOW_LONG_ORIG;
use er_telemetry::counters::WINRECONFIG_SET_WINDOW_POS_CALLS;
use er_telemetry::counters::WINRECONFIG_SET_WINDOW_POS_ORIG;
use er_telemetry::counters::{
    SAVE_PICKER_OS_CANCEL_COUNT, SAVE_PICKER_OS_CLOSED_WITH_PATH, SAVE_PICKER_OS_ERROR_COUNT,
    SAVE_PICKER_OS_LAST_ERROR, SAVE_PICKER_OS_LAST_REJECT_REASON, SAVE_PICKER_OS_OPEN_COUNT,
    SAVE_PICKER_OS_OWNER_HWND, SAVE_PICKER_OS_OWNER_IS_COVER, SAVE_PICKER_OS_REJECT_COUNT,
    SAVE_PICKER_OS_REOPEN_COUNT, SAVE_PICKER_OS_REOPEN_EXHAUSTED, SAVE_PICKER_OS_SAVELIKE_OPENS,
};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use std::os::windows::ffi::OsStrExt as _;
use std::{
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
use windows::{
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

pub(crate) mod save_picker_os_dialog;
pub(crate) use save_picker_os_dialog::*;

pub(crate) mod save_picker_boot;
pub(crate) use save_picker_boot::*;

pub(crate) mod save_picker_surface;
pub(crate) use save_picker_surface::*;
