//! Standalone ELDEN RING save-disable DLL.
//!
//! Deliberately decoupled from the product `er-effects-rs` cdylib: separate crate,
//! separate ME3 `[[natives]]` entry, separate log and telemetry files, no shared
//! state. The product already manipulates save-adjacent state during System->Quit
//! (it clears `CSMenuMan->disableSaveMenu` at +0x13c); keeping this DLL independent
//! means the two can be run separately and their effects told apart.
//!
//! # What it does
//!
//! Two cooperating layers, and it matters that they are separate:
//!
//! **Suppression** (`suppress`) stops the game ever enqueueing a save-write job, at the
//! single choke point every save funnels through, and answers the game's own completion
//! poll with the code that means success. No save byte is written, no `.bak` is copied
//! or deleted, and every native observer sees the state a real successful save leaves.
//! Loads are untouched, so Continue and Load Game still read the real file.
//!
//! **Census** (`hooks` + `witness`) hooks the Win32 file APIs *below* every FromSoft
//! abstraction and records the game-module RVA of any call site that still reaches save
//! data on disk. It was built to discover the write paths from the bottom up; with
//! suppression armed it inverts into the completeness oracle. `escaped_write_sites`
//! must be empty, and any entry in it names a save path the suppression missed.
//!
//! Keeping the census when suppression works is the whole point: the static call graph
//! proves the SL submit is the only *known* write path, and the census is what would
//! catch an unknown one.

#![allow(non_snake_case)]

mod config;
#[cfg(windows)]
mod hooks;
mod redirect;
mod suppress;
mod telemetry;
mod witness;

use std::{
    fmt,
    path::PathBuf,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use er_game_base::log::{append_line, game_directory_path};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;

const LOG_FILE_NAME: &str = "er-save-disable.log";

/// Surfaced in telemetry so a harness can never mistake an observation-only run for
/// a run in which saving was actually suppressed. The value is derived from what
/// actually installed at runtime, not from a compile-time claim -- which is why this
/// replaced the old `PHASE` constant: a build that intended to suppress but failed to
/// arm must not report itself as suppressing.
pub(crate) fn phase() -> &'static str {
    if suppress::is_armed() {
        "suppress+census"
    } else {
        "census-only"
    }
}

static LOG_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static HOOKS_INSTALLED: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn hooks_installed() -> usize {
    HOOKS_INSTALLED.load(Ordering::SeqCst)
}

pub(crate) fn log_message(args: fmt::Arguments<'_>) {
    let path = game_directory_path()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(LOG_FILE_NAME);
    let seq = LOG_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1;
    append_line(&path, format_args!("[{seq:06}] {args}"));
}

#[cfg(windows)]
static START: std::sync::Once = std::sync::Once::new();

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
///
/// Called by the Windows loader. Do not call directly.
pub unsafe extern "system" fn DllMain(
    _module: *mut core::ffi::c_void,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        START.call_once(spawn_census_task);
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_save_disable_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}

#[cfg(windows)]
fn spawn_census_task() {
    // Off the loader lock: MinHook and the module walk must not run inside DllMain.
    let _ = std::thread::Builder::new()
        .name("er-save-disable".to_owned())
        .spawn(|| {
            let mut attempts = 0_u64;
            let base = loop {
                match er_game_base::mem::game_module_base() {
                    Ok(base) => break base,
                    Err(err) => {
                        if attempts == 0 || attempts % 4096 == 0 {
                            log_message(format_args!(
                                "install: waiting for game module base: {err}"
                            ));
                        }
                        attempts = attempts.saturating_add(1);
                        std::thread::yield_now();
                    }
                }
            };
            witness::set_game_base(base);
            // Both layers, in this order. Redirect's destination directory must exist
            // before any write is diverted into it, and the census must be watching
            // before suppression arms, so a write that escapes during the arming window
            // is still recorded.
            config::init_runtime_config();
            redirect::ensure_destination_directory();
            let installed = hooks::install();
            HOOKS_INSTALLED.store(installed, Ordering::SeqCst);
            let suppressing = suppress::install();
            log_message(format_args!(
                "install: base=0x{base:x}, census hooks={installed}/{}, \
                 suppression hooks={suppressing}/2, phase={}",
                hooks::EXPECTED_HOOKS,
                phase()
            ));
            // Publish immediately so a harness can distinguish "no saves happened"
            // from "the DLL never installed" -- an absent telemetry file means the
            // latter, and treating those as the same would let a dead DLL read as a
            // clean run.
            telemetry::write_snapshot();
        });
}
