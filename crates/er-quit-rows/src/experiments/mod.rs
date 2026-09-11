//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

use std::{
    ffi::c_void,
    fs,
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Instant,
};

// `crate::input_blocker::InputBlocker` was imported here for the inject-NAV branch's
// `set_injected_key` stamp in product_core_own_stepper/fallback_drives.rs. That branch was
// unreachable -- its `inject_nav_enabled()` gate could only return `false` -- and was deleted with
// the other abandoned load-mechanism experiments, taking the last use of the type in this module
// with it. The gate itself is gone too (2026-08-26), along with the rest of inject-NAV.
use eldenring::{
    cs::{GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_save_loader::GameManTelemetry;
use fromsoftware_shared::FromStatic;
use windows::{
    Win32::{
        System::{
            LibraryLoader::{GetModuleHandleA, GetProcAddress},
            Threading::GetCurrentProcessId,
        },
        UI::WindowsAndMessaging::GetWindowThreadProcessId,
    },
    core::PCSTR,
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, ffi::*, hooks::*, telemetry::*};

mod save_redirect;
pub(crate) use save_redirect::*;

// Diagnostics, and the first subsystem behind a feature (2026-09-11). Two external references in
// 4,300 lines, so it is the cheapest proof that this crate can be compiled as a subset at all.
#[cfg(feature = "menu-trace")]
mod trace;
#[cfg(feature = "menu-trace")]
pub(crate) use trace::*;

mod startup_hooks;
pub(crate) use startup_hooks::*;

// Always compiled: the clock the whole DLL stamps against, split out of the cover module so
// `loading-cover` is a feature about drawing rather than about telling the time.
mod boot_view_clock;
pub(crate) use boot_view_clock::{boot_view_epoch_ms, boot_view_epoch_ms_if_anchored};

// The boot-view counters, re-exported here rather than from the cover module that used to own them.
// They live in `er-telemetry-core`, which is compiled either way, and they are read by the telemetry
// oracles, the Present compositor and the System>Quit switch guards -- none of which draw a cover.
// Re-exporting them from behind `loading-cover` made turning the cover off a compile error in
// thirty places that only wanted to read a number.
// Which of these a given feature set actually reads varies -- the cover reads most, a
// cover-less build reads the handful the switch guards and the Present path stamp -- so the block
// is allowed to carry the rest rather than being split into per-feature lists that drift.
#[allow(unused_imports)]
pub(crate) use er_telemetry_core::counters::{
    BOOT_VIEW_CONTINUE_ALLOW_BASELINE, BOOT_VIEW_COVER_WINDOW_MS_LAST, BOOT_VIEW_DARK_GAP_FAILURES,
    BOOT_VIEW_DARK_GAP_LAST_HELD_MS, BOOT_VIEW_DARK_GAP_LAST_NATIVE_HITS,
    BOOT_VIEW_DECISION_LOG_MS, BOOT_VIEW_DRAW_BUSY, BOOT_VIEW_DRAW_HITS, BOOT_VIEW_DRAW_STATE,
    BOOT_VIEW_DRAWN_BG_ACTIVE, BOOT_VIEW_DRAWN_IDX, BOOT_VIEW_DRAWN_PERMILLE, BOOT_VIEW_EPOCH_KIND,
    BOOT_VIEW_EPOCH_SEQ, BOOT_VIEW_FADE_COMPLETE_MS, BOOT_VIEW_FADE_FAILURES,
    BOOT_VIEW_FADE_HELD_MS, BOOT_VIEW_FADE_HITS, BOOT_VIEW_FADE_HOLD_HONORED,
    BOOT_VIEW_FADE_HOLD_REASSERT_RUN, BOOT_VIEW_FADE_HOLD_REASSERTS,
    BOOT_VIEW_FADE_HOLD_REASSERTS_FIRST_MS, BOOT_VIEW_FADE_HOLD_REFUSED,
    BOOT_VIEW_FADE_HOLD_TICK_MS, BOOT_VIEW_FADE_LAST_ALPHA, BOOT_VIEW_FADE_START_LS_UPDATE_HITS,
    BOOT_VIEW_FADE_START_MS, BOOT_VIEW_FPS_BAIL_PUBLISH_VERSION, BOOT_VIEW_FPS_BAIL_RESUMED,
    BOOT_VIEW_FPS_BAIL_RESUMES, BOOT_VIEW_FPS_BAIL_SLOT_KEY, BOOT_VIEW_FRESH_DESER_BASELINE,
    BOOT_VIEW_HANDOFF_NATIVE_HITS_BASELINE, BOOT_VIEW_HANDOFF_SEEN_MS, BOOT_VIEW_IDX_CHANGED_MS,
    BOOT_VIEW_LAST_LABEL_HASH, BOOT_VIEW_LAST_PERMILLE, BOOT_VIEW_LOADSCREEN_TABLE_BASELINE,
    BOOT_VIEW_MILESTONE_IDX, BOOT_VIEW_MONO_EPOCH, BOOT_VIEW_MONO_LABEL_LEN,
    BOOT_VIEW_MONO_LABEL_PTR, BOOT_VIEW_MONO_ORD, BOOT_VIEW_NATIVE_GFX_FADE_HOLD_COMPLETE_MS,
    BOOT_VIEW_NATIVE_GFX_FADE_HOLD_HITS, BOOT_VIEW_NONFADE_DRAW_DURING_FADE,
    BOOT_VIEW_NONFADE_DRAW_DURING_FADE_FIRST_MS, BOOT_VIEW_OWN_MENU_LOAD_ACTIVE,
    BOOT_VIEW_PORTRAIT_SPARED_BASELINE, BOOT_VIEW_PRE_WORLD_STOP_FAILURES,
    BOOT_VIEW_PRESENT_COVER_FAILURES, BOOT_VIEW_PRESENT_FULL_CLEAR_HITS, BOOT_VIEW_PUMP_STOP_MS,
    BOOT_VIEW_PUMP_STOP_REASON, BOOT_VIEW_REACHED_MASK, BOOT_VIEW_SELF_FULL_CLEAR_HITS,
    BOOT_VIEW_SELF_PRESENTS, BOOT_VIEW_STOP_NATIVE_HITS, BOOT_VIEW_STOP_REASON, BOOT_VIEW_STOPPED,
    BOOT_VIEW_STRIP_H, BOOT_VIEW_STRIP_W, BOOT_VIEW_SWAPCHAIN_FOUND_MS,
    BOOT_VIEW_TELEMETRY_HANDOFF_STAMPS, BOOT_VIEW_TFC_CONTINUE_BASELINE, BOOT_VIEW_WINDOW_ARM_MS,
};

mod gpu_readback;
pub(crate) use gpu_readback::*;

mod present_overlay;
pub(crate) use present_overlay::*;

mod gpu_frame_timing;
pub(crate) use gpu_frame_timing::*;

// native_overlay moved to the er-loading-portrait-core crate (portrait crate split); the
// explicit re-exports keep the product call sites (lifecycle.rs, telemetry oracles)
// compiling unchanged. The rest of the crate's surface flows in through the glob shims
// at the top of gpu_readback.rs / startup_hooks.rs.
pub(crate) use er_loading_portrait_core::{NATIVE_OVERLAY_SHOW, install_native_overlay};

pub(crate) mod can_move_probe;
pub(crate) mod input_block;
pub(crate) use input_block::*;

mod input_trace;
pub(crate) use input_trace::*;

mod own_load;
pub(crate) use own_load::*;

mod menu_diag;
pub(crate) use menu_diag::*;

mod mem;
pub(crate) use mem::*;

mod gating;
pub(crate) use gating::*;

mod own_stepper;
pub(crate) use own_stepper::*;

mod title;
pub(crate) use title::*;

mod continue_load;
pub(crate) use continue_load::*;

mod save_picker;

mod lifecycle;
pub(crate) use lifecycle::*;

#[path = "mod/product_core_own_stepper.rs"]
mod product_core_own_stepper;
pub(crate) use product_core_own_stepper::*;

// own_stepper_idx6_memory.rs moved to the er-title-flow crate (autoload/title-flow slice); the
// `title` glob above already re-exports it, so the module keeps its declaration site without a
// second re-export of its own.
#[path = "mod/own_stepper_idx6_memory.rs"]
mod own_stepper_idx6_memory;
