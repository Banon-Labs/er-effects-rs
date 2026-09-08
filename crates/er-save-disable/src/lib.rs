//! Standalone ELDEN RING save-disable DLL.
//!
//! Deliberately decoupled from the product `er-quickload` cdylib: separate crate,
//! separate ME3 `[[natives]]` entry, separate log and telemetry files, no shared
//! state. The product already manipulates save-adjacent state during System->Quit
//! (it clears `CSMenuMan->disableSaveMenu` at +0x13c); keeping this DLL independent
//! means the two can be run separately and their effects told apart.
//!
//! # What it does
//!
//! It blocks saving. No byte of the player's save is written while it is loaded.
//!
//! Two cooperating layers, and it matters that they are separate:
//!
//! **Suppression** (the shared `er-save-suppress` crate; it lived here as `suppress.rs`
//! until save-game-flow WP1 moved it out so the product DLL integrates the identical
//! hooks) stops the game ever enqueueing a save-write job, at the single choke point
//! every save funnels through, and answers the game's own completion poll with the code
//! that means success. No save byte is written, no `.bak` is copied or deleted, and
//! every native observer sees the state a real successful save leaves. Loads are
//! untouched, so Continue and Load Game still read the real file.
//!
//! **never load this DLL together with `er_quickload.dll` in one me3 profile.** The
//! product DLL now installs the same `er-save-suppress` hooks itself; each DLL carries
//! its own MinHook instance, so loading both would double-detour `0x140e6fb50` /
//! `0x140e6e430` and corrupt each other's trampolines. The census probe profile stays
//! product-DLL-free (`scripts/build-save-census-profile.sh`).
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

// Host-build hygiene. This crate is a windows `cdylib`: on a non-windows host every item
// whose only consumer is `DllMain` or a hook reads as dead, and `[workspace.lints.rust]
// warnings = "deny"` promotes that to a hard compile ERROR -- so `cargo test -p er-save-disable`
// failed outright, and its unit tests had therefore never executed in any gate. Same fix,
// same reason, as er-save-suppress / er-seamless-bugfixes / er-armament-icons. The shipping
// target is unaffected: this allow does not exist there.
// scripts/check-save-disable-warnings.py still holds the windows build to zero warnings.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

#[cfg(windows)]
mod hooks;
mod telemetry;
mod witness;

use std::{
    fmt,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use er_game_base::log::{append_line, redirected_artifact_path};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;

const LOG_FILE_NAME: &str = "er-save-disable.log";

/// What the run is actually doing, derived from what installed at runtime rather than
/// from a compile-time claim -- a build that intended to suppress but failed to arm must
/// not report itself as suppressing.
pub(crate) fn phase() -> &'static str {
    if er_save_suppress::is_armed() {
        "suppress+census"
    } else {
        "census-only"
    }
}

/// Diagnostic disarm for the mandatory positive control: the identical build with
/// interception off, to show that the detectors do fire and the save file does change
/// when nothing is suppressed.
///
/// Suppression is now the only layer that can hide a write, so disarming it leaves a
/// genuinely pure census. That used not to be true: a path-diversion layer stayed armed
/// through the control, sent the write to a decoy, and left the census with nothing to
/// observe -- an oracle whose negative control cannot fire is not falsifiable, and an
/// unfalsifiable oracle is not evidence. Deleting that layer makes the failure
/// structurally impossible rather than merely fixed.
pub(crate) const CENSUS_ONLY_ENV: &str = "ER_SAVE_DISABLE_CENSUS_ONLY";

pub(crate) fn census_only_requested() -> bool {
    matches!(
        std::env::var(CENSUS_ONLY_ENV).ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE")
    )
}

static LOG_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static HOOKS_INSTALLED: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn hooks_installed() -> usize {
    HOOKS_INSTALLED.load(Ordering::SeqCst)
}

/// Append one census line, into this run's directory when the launcher named one.
///
/// The game-directory copy of this log is single-SLOT: `er_game_base::log::begin_fresh_run`
/// keeps exactly one previous generation, so two launches lose the run before last, and
/// several sessions launch concurrently in this repo. Worse, `run-save-census-probe.sh` used
/// to `rm -f` the live file before launching, and the rotation removes a stale `.prev`
/// unconditionally when the live file is absent -- so that swept away two runs' census
/// evidence at once, neither of them the deleting run's. The redirect is what lets a launcher
/// hand each run its own directory; the game-directory fallback stays because the env has to
/// survive `launch.sh` -> me3 -> Proton and a log written nowhere reads as "the DLL never
/// loaded", which is the exact false negative this census exists to rule out.
pub(crate) fn log_message(args: fmt::Arguments<'_>) {
    let path = redirected_artifact_path("ER_QUICKLOAD_SAVE_DISABLE_LOG_PATH", LOG_FILE_NAME);
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
        // One sink for this DLL's hook + address lines. Without it a refused address is
        // silent here, because every cdylib links its own copy of er-hook/er-game-base.
        // A rust_panic in a cdylib loaded into the game is otherwise anonymous: the message goes to a
        // stderr nobody reads, and what survives is a 0xe06d7363 record naming the module and nothing
        // else. Two boots were lost to one before this existed. See er_game_base::panic_report.
        er_game_base::panic_report::report_panics_to("er-save-disable", log_message);
        er_hook::set_hook_logger(log_message);
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
            // Bounded (2026-08-29): an unbounded `loop { yield_now() }` in two other shells starved the
            // wineserver and hung a whole boot -- see er_game_base::wait. Same shape, same fix.
            let found =
                er_game_base::wait::poll_until(|| match er_game_base::mem::game_module_base() {
                    Ok(base) => Some(base),
                    Err(err) => {
                        if attempts == 0 || attempts.is_multiple_of(4096) {
                            log_message(format_args!(
                                "install: waiting for game module base: {err}"
                            ));
                        }
                        attempts = attempts.saturating_add(1);
                        None
                    }
                });
            let Some(base) = found else {
                log_message(format_args!(
                    "install: no game module base; nothing installed"
                ));
                return;
            };
            witness::set_game_base(base);
            // Wire the shared suppression core's seams to this DLL's surfaces before
            // anything can install: human lines to er-save-disable.log, publishes to the
            // census telemetry snapshot through the witness reentrancy guard (the
            // suppression hooks are not observation paths, so they enter with the guard
            // clear; taking it in the sink keeps `telemetry::write_snapshot`'s documented
            // invariant true for every caller).
            er_save_suppress::set_log_sink(log_message);
            er_save_suppress::set_publish_sink(|| {
                let _ = witness::with_guard(telemetry::write_snapshot);
            });
            // Census first: it must be watching before suppression arms, so a write
            // that escapes suppression during the arming window is still recorded.
            let installed = hooks::install();
            HOOKS_INSTALLED.store(installed, Ordering::SeqCst);
            // The census-only env check moved out of the core (`install` takes a plain
            // bool): only this standalone diagnostic DLL consults the env var, so no
            // env var can alter the product DLL's behavior.
            let census_only = census_only_requested();
            if census_only {
                log_message(format_args!(
                    "install: {CENSUS_ONLY_ENV} requested census-only positive control; \
                     suppression will be disarmed and saves observed for real"
                ));
            }
            let suppressing = er_save_suppress::install(census_only);
            log_message(format_args!(
                "install: base=0x{base:x}, census hooks={installed}/{}, \
                 suppression hooks={suppressing}/{}, phase={}",
                hooks::EXPECTED_HOOKS,
                er_save_suppress::SUPPRESSOR_HOOKS,
                phase()
            ));
            // Publish immediately so a harness can distinguish "no saves happened"
            // from "the DLL never installed" -- an absent telemetry file means the
            // latter, and treating those as the same would let a dead DLL read as a
            // clean run.
            //
            // Through the guard: `hooks::install()` armed the detours above, so this is
            // the DLL's first re-entry into its own hooks. Without it the save-path
            // filter is the only thing keeping the census from observing its own output.
            let _ = witness::with_guard(telemetry::write_snapshot);
        });
}
