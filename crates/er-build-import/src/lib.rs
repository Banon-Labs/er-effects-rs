//! ME3 shell that runs the `er-quickload.toml` build once, at character load -- import, export, or
//! both.
//!
//! All of the work lives in `er-build-import-runtime`; this crate is the standalone trigger for it.
//! The product DLL (`er-quickload`) drives the SAME runtime from its System>Quit "Load Build from
//! URL" row instead, so the two must never share a profile -- see `scripts/me3-dll-conflicts.toml`.
//!
//! Nothing here sleeps: `DllMain` spawns two threads and returns, the fetch blocks in WinHTTP, and
//! the game task re-checks its preconditions once a frame.
//!
//! # The export side is a harness, not a feature
//!
//! `export_build_link_on_load = true` makes this shell generate ONE share link as soon as the
//! character is in the world and write it to `er-build-import.log`, with no clipboard and no
//! browser. That exists so the CONTENT of a link can be checked -- `scripts/decode-build-link.py
//! --log <file> --summary` -- without a human driving the System>Quit menu, which is the only way
//! a player reaches the product's own export. The product row is unchanged and is still the thing
//! players press.

#![cfg(windows)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows::Win32::Foundation::{HINSTANCE, TRUE};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

/// This DLL's own module handle, captured at attach so the sidecar can be found beside it.
static MODULE: AtomicUsize = AtomicUsize::new(0);

/// DLL entry point. Spawns the request and the task registrar, and returns.
///
/// # Safety
///
/// Called by the loader. Nothing slow or reentrant runs under the loader lock.
#[unsafe(no_mangle)]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut ()) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // FIRST, before anything that can panic. A panic in a cdylib crosses an
        // `extern "system"` boundary and becomes an ABORT, which does not dispatch to a
        // vectored handler -- so no crash record is written at all and the process simply
        // vanishes. Enforced by `scripts/check-panic-reporter-installed.py`.
        er_game_base::panic_report::report_panics_to("er-build-import", panic_log_sink);

        MODULE.store(module.0 as usize, Ordering::SeqCst);
        std::thread::spawn(start_configured_work);
        std::thread::spawn(register_task);
    }
    TRUE.0
}

/// `report_panics_to`'s sink, which takes `fmt::Arguments` where this crate logs `&str`.
///
/// Small and duplicated per shell on purpose: the hook is installed PER DLL because every cdylib
/// statically links its own `er-game-base`, so there is no shared place this could live and still
/// be the thing that runs in this module.
fn panic_log_sink(args: core::fmt::Arguments<'_>) {
    er_game_base::log::append_line(
        &er_game_base::log::game_directory_path()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("er-build-import.log"),
        args,
    );
}

/// This DLL's own path, for finding the sidecar beside it.
fn module_path() -> Option<PathBuf> {
    let module = MODULE.load(Ordering::SeqCst);
    if module == 0 {
        return None;
    }
    let mut buffer = [0u16; 1024];
    // Safety: our own module handle and our own buffer; the call writes at most `buffer.len()`.
    let written = unsafe { GetModuleFileNameW(Some(HINSTANCE(module as _).into()), &mut buffer) };
    if written == 0 || written as usize >= buffer.len() {
        return None;
    }
    // Decoded strictly, not lossily: a path that is not valid UTF-16 is a path this cannot open
    // anyway, and a replacement character would turn that into a sidecar silently read from the
    // wrong file name.
    String::from_utf16(&buffer[..written as usize])
        .ok()
        .map(PathBuf::from)
}

/// The DLL-adjacent per-run overlay: `<this dll's stem>.toml`.
///
/// A file the caller who staged this run owns, rather than the game-directory `er-quickload.toml`,
/// which belongs to the player. A harness that rewrote the player's config to arm itself would be
/// a harness nobody could safely run twice.
fn sidecar_path() -> Option<PathBuf> {
    let path = module_path()?;
    let stem = path.file_stem()?.to_owned();
    Some(path.with_file_name(stem).with_extension("toml"))
}

/// What the sidecar arms, if anything: `(export-only, round-trip)`.
fn sidecar_arms() -> (bool, bool) {
    let Some(path) = sidecar_path() else {
        return (false, false);
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return (false, false);
    };
    let export =
        er_build_import_core::config_flag(&contents, er_build_import_core::EXPORT_ON_LOAD_KEY);
    let round_trip =
        er_build_import_core::config_flag(&contents, er_build_import_core::ROUND_TRIP_ON_LOAD_KEY);
    er_build_import_runtime::log_line(&format!(
        "[build-export] sidecar '{}' read; {} = {export}, {} = {round_trip}",
        path.display(),
        er_build_import_core::EXPORT_ON_LOAD_KEY,
        er_build_import_core::ROUND_TRIP_ON_LOAD_KEY,
    ));
    (export, round_trip)
}

/// What this run of the shell is for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Import the configured build and stop -- the shell's original job.
    Import,
    /// Export the character as it stands, touching nothing.
    Export,
    /// Import the configured build and then export what actually landed.
    ///
    /// The round trip, and the only mode whose answer is known in advance: everything the export
    /// writes should be the equipped subset of what the import was told to place, with the same
    /// names, affinities, ashes and levels. A difference is a bug in one of the two directions
    /// rather than a matter of opinion about the player's build.
    ImportThenExport,
}

/// What this run is armed for.
///
/// EXPORT-ONLY IS EXCLUSIVE, and deliberately so: an export measures the character as it stands,
/// while an import REWRITES that character and grants it items. Asking for both is the round trip,
/// and it has to be asked for by name.
fn mode() -> Mode {
    let (export, round_trip) = sidecar_arms();
    if round_trip {
        return Mode::ImportThenExport;
    }
    if export || er_build_import_runtime::configured_export_on_load() {
        return Mode::Export;
    }
    Mode::Import
}

/// Start whichever half runs first. Off the loader thread because both paths read files.
fn start_configured_work() {
    match mode() {
        Mode::Import | Mode::ImportThenExport => start_configured_import(),
        // Nothing to import: the export can be armed immediately, and the game task will pick it
        // up as soon as the character is in the world.
        Mode::Export => start_configured_export(),
    }
}

/// Set once the round trip's export has been asked for, so it is asked for exactly once.
static EXPORT_REQUESTED: AtomicUsize = AtomicUsize::new(0);

/// In [`Mode::ImportThenExport`], arm the export the moment the import reports it is finished.
///
/// Polled from the game task rather than from a thread of its own: the import's phase is the only
/// signal that the character now holds what the build asked for, and exporting before that would
/// measure the character the import started from.
fn arm_export_after_import() {
    use er_build_import_runtime::Phase;

    if EXPORT_REQUESTED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match er_build_import_runtime::phase() {
        Phase::Done => {
            EXPORT_REQUESTED.store(1, Ordering::SeqCst);
            er_build_import_runtime::log_line(
                "[build-export] the import finished; exporting the character it produced",
            );
            start_configured_export();
        }
        // A FAILED import leaves the character in a state nobody asked for, so the round trip is
        // abandoned rather than reported on. Said once, then never again.
        Phase::Failed => {
            EXPORT_REQUESTED.store(1, Ordering::SeqCst);
            er_build_import_runtime::log_line(
                "[build-export] the import FAILED, so no export was taken -- the character is not \
                 the one the build describes",
            );
        }
        _ => {}
    }
}

/// Ask the runtime for the configured build. Off the loader thread because reading the config file
/// touches the filesystem.
fn start_configured_import() {
    match er_build_import_runtime::request_configured() {
        Ok(true) => {}
        // No `build_url` set: the runtime already logged that, and doing nothing is correct.
        Ok(false) => {}
        Err(err) => er_build_import_runtime::log_line(&format!(
            "[build-import] configured import refused: {err}"
        )),
    }
}

/// Ask the runtime for one exported link, when the config says to. Off the loader thread for the
/// same reason the import is: reading the config touches the filesystem.
///
/// The request only LATCHES here; the read itself happens on the game task below, once the params
/// are streamed and the character is actually in the world.
fn start_configured_export() {
    if !er_build_import_runtime::configured_export_on_load() {
        return;
    }
    match er_build_import_runtime::export::request() {
        Ok(()) => er_build_import_runtime::log_line(
            "[build-export] configured export armed; the link lands in this log once the \
             character is in the world",
        ),
        Err(err) => er_build_import_runtime::log_line(&format!(
            "[build-export] configured export refused: {err}"
        )),
    }
}

/// Register the FrameBegin task that owns every game-touching step.
fn register_task() {
    use eldenring::cs::{CSTaskGroupIndex, CSTaskImp};
    use eldenring::fd4::FD4TaskData;
    use fromsoftware_shared::{FromStatic, SharedTaskImpExt};

    // BOUNDED (2026-08-29): see er_game_base::wait -- the unbounded form of this loop starved the
    // wineserver and hung a boot.
    let Some(task) = er_game_base::wait::poll_until(|| unsafe { CSTaskImp::instance() }.ok())
    else {
        return;
    };
    er_build_import_runtime::log_line(
        "[build-import] CSTaskImp resolved; registering FrameBegin import task",
    );

    let round_trip = mode() == Mode::ImportThenExport;
    let handle = task.run_recurring(
        move |_data: &FD4TaskData| {
            // Safety: this closure runs on the game task thread, which is the context the runtime's
            // tick requires; each step inside it is individually precondition-checked.
            let _ = unsafe { er_build_import_runtime::tick() };
            if round_trip {
                arm_export_after_import();
            }
            // Safety: same game-task context; `export::tick` returns immediately unless the
            // request above latched one, and re-checks every precondition itself.
            let _ = unsafe {
                er_build_import_runtime::export::tick(
                    er_build_import_runtime::export::Sinks::log_only(),
                )
            };
        },
        CSTaskGroupIndex::FrameBegin,
    );
    // The handle cancels the task on drop, and the task must outlive this bootstrap thread.
    std::mem::forget(handle);
}
