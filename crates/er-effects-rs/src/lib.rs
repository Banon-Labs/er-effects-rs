#![allow(unused_imports)]

#[cfg(not(windows))]
pub fn host_diagnostic_stub() {}

#[cfg(windows)]
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

#[cfg(windows)]
use crate::input_blocker::InputBlocker;
#[cfg(windows)]
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
#[cfg(windows)]
use eldenring::{
    cs::{
        CSTaskGroupIndex, CSTaskImp, ChrInsExt, FaceData, FaceDataBuffer, GameDataMan, GameMan,
        PlayerGameData, PlayerIns,
    },
    dlkr::DLAllocator,
    fd4::FD4TaskData,
};
#[cfg(windows)]
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoader};
#[cfg(windows)]
use fromsoftware_shared::{F32Vector4, FromStatic, InstanceError, SharedTaskImpExt};
#[cfg(windows)]
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
        System::{
            LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA},
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

#[cfg(windows)]
mod config;
#[cfg(windows)]
mod constants;
#[cfg(windows)]
mod crashlog;
#[cfg(windows)]
mod experiments;
#[cfg(windows)]
mod ffi;
#[cfg(windows)]
mod hooks;
#[cfg(windows)]
mod input_blocker;
#[cfg(windows)]
mod mh;
#[cfg(windows)]
mod telemetry;

#[cfg(windows)]
include!("lib_parts/dll_entry.rs");
#[cfg(windows)]
include!("lib_parts/runtime_helpers.rs");
