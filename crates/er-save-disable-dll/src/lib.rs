//! Standalone ELDEN RING save-disable DLL.
//!
//! Deliberately decoupled from the product `er-effects-rs` cdylib: separate crate,
//! separate ME3 `[[natives]]` entry, separate log and telemetry files, no shared
//! state. The product already manipulates save-adjacent state during System->Quit
//! (it clears `CSMenuMan->disableSaveMenu` at +0x13c); keeping this DLL independent
//! means the two can be run separately and their effects told apart.
//!
//! # Where this is in its lifecycle
//!
//! Phase 1 (current): **census only**. It suppresses nothing. It hooks the Win32
//! file APIs, filters to save containers, and records the game-module RVAs of every
//! call site that reaches save data on disk. That establishes, from the bottom up
//! and by measurement rather than by reading disassembly, the complete set of paths
//! by which ELDEN RING actually writes a save.
//!
//! Phase 2: game-side interception plus the fake-success contract, informed by the
//! static reverse engineering of the save orchestrator. When that lands, the census
//! stays -- it inverts into the completeness oracle, because any call site still
//! appearing in it is a save path the interception missed.
//!
//! Being explicit about the phase matters: a DLL named "save disable" that currently
//! disables nothing would otherwise be easy to mistake for a working feature.

#![allow(non_snake_case)]

mod config;
#[cfg(windows)]
mod hooks;
mod redirect;
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

/// Surfaced in telemetry so a harness knows what the run was actually doing.
///
/// `redirect` means save WRITES were diverted to a harmless path while reads were left
/// alone. The game's own save machinery runs unmodified and genuinely succeeds, so no
/// state is forged and no waiter can deadlock -- see `redirect.rs` for why that matters.
pub(crate) const PHASE: &str = "redirect";

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
            config::init_runtime_config();
            redirect::ensure_destination_directory();
            let installed = hooks::install();
            HOOKS_INSTALLED.store(installed, Ordering::SeqCst);
            let config = config::runtime_config();
            log_message(format_args!(
                "install: active (base=0x{base:x}, hooks={installed}/{}); phase={PHASE} -- save \
                 WRITES divert to {} with suffix {}; reads are untouched so the real save still loads",
                hooks::EXPECTED_HOOKS,
                config
                    .save_directory
                    .as_ref()
                    .map_or_else(|| "the original directory".to_owned(), |d| d.display().to_string()),
                config.suffix,
            ));
            // Publish immediately so a harness can distinguish "no saves happened"
            // from "the DLL never installed" -- an absent telemetry file means the
            // latter, and treating those as the same would let a dead DLL read as a
            // clean run.
            telemetry::write_snapshot();
        });
}
