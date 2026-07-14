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
    cs::{
        CSTaskGroupIndex, CSTaskImp, ChrInsExt, FaceData, FaceDataBuffer, GameDataMan, GameMan,
        PlayerGameData, PlayerIns,
    },
    dlkr::DLAllocator,
    fd4::FD4TaskData,
};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoader};
use fromsoftware_shared::{F32Vector4, FromStatic, InstanceError, SharedTaskImpExt};
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

mod config;
mod constants;
mod crashlog;
mod effects;
mod experiments;
mod ffi;
mod hooks;
mod input_blocker;
mod mh;
mod telemetry;

include!("lib_parts/dll_entry.rs");
include!("lib_parts/runtime_helpers.rs");
