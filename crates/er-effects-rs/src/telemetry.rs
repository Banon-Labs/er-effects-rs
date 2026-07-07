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

use crate::input_blocker::InputBlocker;
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
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, experiments::*, ffi::*, hooks::*};

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

include!("telemetry/runtime_oracles.rs");
include!("telemetry/save_policy_logs.rs");
