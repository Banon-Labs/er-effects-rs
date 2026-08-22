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

use crate::input_blocker::InputBlocker;
use eldenring::{
    cs::{GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_save_loader::GameManTelemetry;
use fromsoftware_shared::FromStatic;
use windows::{
    Win32::{
        Foundation::RECT,
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

mod trace;
pub(crate) use trace::*;

mod startup_hooks;
pub(crate) use startup_hooks::*;

mod gpu_readback;
pub(crate) use gpu_readback::*;

mod present_overlay;
pub(crate) use present_overlay::*;

mod gpu_frame_timing;
pub(crate) use gpu_frame_timing::*;

// native_overlay moved to the er-loading-portrait crate (portrait crate split); the
// explicit re-exports keep the product call sites (lifecycle.rs, telemetry oracles)
// compiling unchanged. The rest of the crate's surface flows in through the glob shims
// at the top of gpu_readback.rs / startup_hooks.rs.
pub(crate) use er_loading_portrait::{NATIVE_OVERLAY_SHOW, install_native_overlay};

pub(crate) mod can_move_probe;
mod input_block;
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

#[path = "mod/own_stepper_idx6_memory.rs"]
mod own_stepper_idx6_memory;
pub(crate) use own_stepper_idx6_memory::*;
