//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

#![allow(unused_imports)]

use std::{
    ffi::c_void,
    fmt::Write as _,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use std::os::windows::ffi::OsStrExt as _;

use crate::input_blocker::{InputBlocker, InputFlags};
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
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
            ClipCursor, EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
            WM_KEYDOWN, WM_KEYUP,
        },
    },
    core::{BOOL, PCSTR},
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

mod input_block;
pub(crate) use input_block::*;

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

mod submit;
pub(crate) use submit::*;

mod profiler;
pub(crate) use profiler::*;

mod lifecycle;
pub(crate) use lifecycle::*;

#[path = "mod/product_core_own_stepper.rs"]
mod product_core_own_stepper;
pub(crate) use product_core_own_stepper::*;

#[path = "mod/own_stepper_idx6_memory.rs"]
mod own_stepper_idx6_memory;
pub(crate) use own_stepper_idx6_memory::*;
