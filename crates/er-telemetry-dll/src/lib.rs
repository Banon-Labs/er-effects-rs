//! er-telemetry-dll: a thin standalone telemetry cdylib.
//!
//! Modeled on er-reload-trace-dll's shape: a `DllMain` that, on
//! `DLL_PROCESS_ATTACH`, spawns an install thread which waits for the game's
//! task manager and registers a game-thread `FrameBegin` recurring tick. The
//! tick runs ONLY er-telemetry's read-side oracles (game-RAM/PE reads that need
//! no product hooks) and writes `er-telemetry-standalone.json`.
//!
//! Runnable alone (telemetry-only me3 profile) or alongside the product DLL as an
//! additional `[[natives]]` entry. All reusable logic lives in the er-telemetry
//! LIB; this crate is only the DllMain + task-registration shell.
#![allow(non_snake_case)]

#[cfg(windows)]
use std::sync::Once;

#[cfg(windows)]
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp},
    fd4::FD4TaskData,
};
#[cfg(windows)]
use fromsoftware_shared::{FromStatic, SharedTaskImpExt};
#[cfg(windows)]
use windows::Win32::{Foundation::HINSTANCE, System::SystemServices::DLL_PROCESS_ATTACH};

const DLL_MAIN_SUCCESS: i32 = 1;

#[cfg(windows)]
static START: Once = Once::new();

#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    _module: HINSTANCE,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        START.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-telemetry-standalone".into())
                .spawn(|| {
                    // Wait for the game's task manager, then register a game-thread
                    // per-frame tick (same pattern as the product's wait_for_task_instance).
                    let task = loop {
                        match unsafe { CSTaskImp::instance() } {
                            Ok(t) => break t,
                            // No sleep (banned by scripts/check-no-timeouts.py): yield to the
                            // game threads and re-poll -- the exact wait the product uses in
                            // wait_for_task_instance (bootstrap.rs).
                            Err(_) => std::thread::yield_now(),
                        }
                    };
                    task.run_recurring(
                        |_data: &FD4TaskData| {
                            // READ-SIDE oracles only: no product hooks, no EffectsState.
                            er_telemetry::standalone_tick();
                        },
                        CSTaskGroupIndex::FrameBegin,
                    );
                });
        });
    }
    DLL_MAIN_SUCCESS
}

// Non-windows: keep the crate buildable for host tooling / workspace resolution.
// A cdylib with no DllMain is valid; the game entry only exists on windows.
#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_telemetry_dll_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}
