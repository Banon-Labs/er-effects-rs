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

pub(crate) use er_loading_portrait::*;

// The former flat startup-hook namespace used to be re-exported here wholesale
// (`crate::*` plus `crashlog/ffi/hooks/telemetry::*`) so child modules could glob-import their
// parent and keep compiling unqualified. Both shims are gone: after the 2026-08-21 lint-parity
// sweep pruned the dead imports, nothing resolves through them any more, and rustc 1.98 says so.
// The named re-exports below are the ones that actually carry references.
pub(crate) use er_quit_menu::save_dest_identity::*;
pub(crate) use er_quit_menu::save_flow_boxes::{
    SAVE_FLOW_BOX_OVERWRITE_FILE, SaveFlowDecision, save_flow_box_label,
};
pub(crate) use er_telemetry::counters::CORRUPTED_SAVE_SEEN_COUNT;
pub(crate) use er_telemetry::counters::NETWORK_CHECK_SHORTCIRCUIT_COUNT;
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
pub(crate) use er_telemetry::counters::SAVE_DEST_CANCEL_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_COMMIT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_COMMIT_FAIL;
pub(crate) use er_telemetry::counters::SAVE_DEST_COMMIT_PENDING;
pub(crate) use er_telemetry::counters::SAVE_DEST_LIVE_BAK_MUTATED;
pub(crate) use er_telemetry::counters::SAVE_DEST_LIVE_FILE_MUTATED;
pub(crate) use er_telemetry::counters::SAVE_DEST_LIVE_OVERWRITE_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_OPEN_PICKER_PENDING;
pub(crate) use er_telemetry::counters::SAVE_DEST_PICKER_OPEN_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_REDIRECT_ARMED;
pub(crate) use er_telemetry::counters::SAVE_DEST_REDIRECT_HITS;
pub(crate) use er_telemetry::counters::SAVE_DEST_SEED_FAIL_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_SEEDED_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_EXISTING_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_NEW_COUNT;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_STRUCTURE_OK;
pub(crate) use er_telemetry::counters::SAVE_DEST_TARGET_WRITTEN_OK;
pub(crate) use er_telemetry::counters::SAVE_PICKER_CANCEL_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_DEST_MODE;
pub(crate) use er_telemetry::counters::SAVE_PICKER_MODE_ACTIVE;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OPEN_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_DIALOG_OPEN;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_TICKS_FROZEN;
pub(crate) use er_telemetry::counters::SAVE_PICKER_PICK_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_PICK_REJECT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_REPOPULATE_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_RESUBMIT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_STAGED_ROW_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_SURFACE;
pub(crate) use er_telemetry::counters::TESTNET_FF_FIRED_EPOCH;
pub(crate) use er_telemetry::counters::TESTNET_FF_LAST_MMS;
pub(crate) use er_telemetry::counters::TESTNET_FF_STUCK_FRAMES;
pub(crate) use er_telemetry::counters::TITLE_OPEN_MENU_SUPPRESSED_COUNT;
pub(crate) use er_telemetry::counters::WINRECONFIG_CHANGE_DISPLAY_CALLS;
pub(crate) use er_telemetry::counters::WINRECONFIG_CREATE_WINDOW_CALLS;
pub(crate) use er_telemetry::counters::WINRECONFIG_EARLY_APPLY_MS;
pub(crate) use er_telemetry::counters::WINRECONFIG_EARLY_APPLY_RECT;
pub(crate) use er_telemetry::counters::WINRECONFIG_EARLY_APPLY_RESULT;
pub(crate) use er_telemetry::counters::WINRECONFIG_MOVE_WINDOW_CALLS;
pub(crate) use er_telemetry::counters::WINRECONFIG_SET_WINDOW_LONG_CALLS;
pub(crate) use er_telemetry::counters::WINRECONFIG_SET_WINDOW_POS_CALLS;
mod loading_cover;
pub(crate) use loading_cover::*;

mod quit_menu;
pub(crate) use quit_menu::*;

mod save_picker;
pub(crate) use save_picker::*;

mod diagnostics;
pub(crate) use diagnostics::*;
