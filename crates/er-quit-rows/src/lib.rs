#[cfg(not(windows))]
pub fn host_diagnostic_stub() {}

// Deliberately outside the `#[cfg(windows)]` block below: a pure install decision whose tests run
// on the host, where every other gate in this crate is only ever type-checked by the cross-compile.
pub mod menu_window_run_install;
// Same reason, same shape: the `05_010_ProfileSelect` chrome decision, host-testable.
pub mod profile_select_chrome_gate;

#[cfg(windows)]
use std::{
    ffi::c_void,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

#[cfg(windows)]
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
#[cfg(windows)]
use er_save_loader::{SaveLoadContext, SaveLoader};
#[cfg(windows)]
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
#[cfg(windows)]
use windows::{
    Win32::{
        Foundation::HINSTANCE,
        System::{
            LibraryLoader::{GetProcAddress, LoadLibraryA},
            SystemServices::DLL_PROCESS_ATTACH,
        },
    },
    core::PCSTR,
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
