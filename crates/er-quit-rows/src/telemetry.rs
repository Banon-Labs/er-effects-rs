//! telemetry module (split from lib.rs; pure code reorganization, no behavior change).

use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use eldenring::cs::{GameMan, PlayerIns};
use er_save_loader::GameManTelemetry;
use fromsoftware_shared::FromStatic;
use windows::{Win32::System::LibraryLoader::GetModuleHandleA, core::PCSTR};

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
include!("telemetry/portrait_load_windows.rs");
include!("telemetry/native_ls_exposure.rs");
include!("telemetry/cover_after_release.rs");
