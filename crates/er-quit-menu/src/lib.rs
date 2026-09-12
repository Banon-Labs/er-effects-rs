//! Standalone ME3 shell for all four cloned System>Quit rows -- **Load Character**, **Load
//! Character from File**, **Load Build from URL** and **Generate Build Link** -- with no product
//! DLL in the profile.
//!
//! Everything the rows are made of lives in `er-quit-menu-core`, which the product DLL also links
//! and arms. The two loads now pass the same `RowSet::ALL`; what still differs is the flow table.
//! The product supplies its save-safe character switch, its Save Game flow and its picker; this
//! shell supplies the two openers the feature crate already owns and leaves the rest at `None`,
//! which the router reads as a press it must not carry out.
//!
//! # Why one shell arms all four rather than two shells arming halves
//!
//! Halves were tried on 2026-09-12 and measured the same day. Each cdylib links its own copy of
//! `er-quit-menu-core`, so `ROW_SET`, the row table and the arm latch are per DLL; both
//! `AddCancelButton` detours sat on the hook union and both fired, and each shell cloned its pair
//! into the same dialog and then again on the other's pass. The result was a six-cell grid holding
//! `item_count=10`, two row tables disagreeing about which index is Return to Desktop, and every
//! press resolving `row=AMBIGUOUS` -- so the safety gate suppressed Save Game and refused the
//! instant quit, and what the player saw was the native Return-to-Desktop confirm behind every row.
//! One arm in one process is the only shape that works without shared cross-DLL state.
//!
//! # What this shell has to install that the product already had
//!
//! Three things, and each fails differently, so each is reported separately:
//!
//! * the derived six-cell `02_040_optionsetting` grid the rows are cells of, and the `02_990`
//!   movie the link field opens, both served from the Scaleform file-open prologue;
//! * a `MenuWindowJob::Run` detour, which is the only context in which the link field's job can be
//!   submitted and its display objects resolved;
//! * a `FrameBegin` task, which is the only context in which an import may touch the inventory.
//!
//! # Never in the same profile as `er-quit-load-character`, and never beside a `quit-rows` product
//!
//! Any other host that arms rows is a second cloner, for the reason above, and both derive the same
//! movie -- `er_gfx::options_02_040::quit6` fail-closes when its input is not vanilla, so a second
//! deriver handed already-derived bytes correctly refuses. `scripts/me3-dll-conflicts.toml` records
//! those pairs and the profile generator refuses to emit a profile carrying both. A default
//! `er-quickload` is not one of them: it arms no rows and derives no grid since `quit-rows` came
//! off its default features.
//!
//! # What stays refused here
//!
//! Every product-owned answer in the host seam stays at its neutral default -- including the
//! save-write bypass, so a product-less load can never push a write past `er-save-suppress`.

// A cdylib whose every consumer is `DllMain` and the hooks it installs, all of them
// `#[cfg(windows)]`. On a host build the shell is compiled with its only callers cfg'd
// out, so `dead_code`/`unused_imports` there report the cfg, not real debt. The shipping
// target (x86_64-pc-windows-msvc) carries the full deny with no allows.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

use std::path::{Path, PathBuf};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;
const LOG_FILE_NAME: &str = "er-quit-menu.log";

#[cfg(windows)]
static START: std::sync::Once = std::sync::Once::new();

/// Where the standalone log lands: next to the executable, falling back to the CWD.
fn log_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Fresh per process: the first line of a run truncates the file (rotating the previous
/// run's aside as `.log.prev`), later lines append. No log in this repo accumulates across
/// runs -- mixing evidence from builds that no longer exist is how a count over one file
/// gets read as one run's behaviour.
fn append_log(dir: &Path, args: std::fmt::Arguments<'_>) {
    er_game_base::log::append_line(
        &dir.join(LOG_FILE_NAME),
        format_args!("er-quit-menu: {args}"),
    );
}

/// The standalone host seam: this DLL has no product behind it, so every product-owned
/// answer stays at its neutral default -- including the save-write bypass, which must stay
/// refused so a product-less load can never push a write past `er-save-suppress`.
fn install_standalone_host() {
    let _ = er_quit_menu_core::install_host(er_quit_menu_core::QuitMenuHost {
        append_autoload_debug: standalone_log,
        append_crash_log: standalone_log,
        ..er_quit_menu_core::QuitMenuHost::defaults()
    });
}

fn standalone_log(args: std::fmt::Arguments<'_>) {
    append_log(&log_dir(), args);
}

/// Open the browse picker for a **Load Character from File** press.
///
/// The row router answers in booleans; the picker answers in outcomes, and the two disagree about
/// one case on purpose. `Dismissed` means a picker ran and the user backed out of it -- the press
/// was carried out, so the row must not re-arm -- while the router only needs to know whether the
/// press was taken.
///
/// # Safety
///
/// Menu-thread press context, with `action_obj` the row's live action object.
#[cfg(windows)]
unsafe fn open_save_picker_for_row(action_obj: usize) -> bool {
    unsafe { er_quit_menu_core::save_picker_menu::system_quit_open_save_picker_menu(action_obj) }
        .request_discharged()
}

/// Arm all four cloned rows. Runs on its own thread because the game-task registration waits for
/// the game's task manager to exist, and waiting inside the loader lock deadlocks the process.
#[cfg(windows)]
fn arm_build_rows() {
    // Safety: a bootstrap thread, once per process (`START` gates the spawn), before the Quit tab
    // has built a dialog.
    let arm = unsafe {
        er_quit_menu_core::arm::arm_standalone(
            // All four cloned rows, from one shell (2026-09-12).
            //
            // Two shells arming halves of the set was tried and measured the same day, and it is
            // not a way to ship both halves: each cdylib links its own copy of
            // `er-quit-menu-core`, so `ROW_SET`, the row table and the arm latch are per DLL. Both
            // `AddCancelButton` detours sat on the hook union and both fired, so each shell cloned
            // its own pair into the same dialog and then again on the other's pass -- a six-cell
            // grid carrying `item_count=10`, two row tables that disagreed about which index is
            // Return to Desktop, and therefore every press resolving to `row=AMBIGUOUS`. The safety
            // gate did its job: Save Game was suppressed and the instant quit refused, so what the
            // player saw was the native Return-to-Desktop confirm behind every row.
            //
            // One arm in one process is the fix, and it costs nothing to reach from here: both
            // flows the character rows need already live in `er-quit-menu-core`.
            er_quit_menu_core::row_cloner::RowSet::ALL,
            er_quit_menu_core::row_cloner::QuitRowActions {
                open_profile_load_dialog: Some(
                    er_quit_menu_core::profile_load_dialog::system_quit_open_profile_load_dialog,
                ),
                open_save_picker_menu: Some(open_save_picker_for_row),
                ..er_quit_menu_core::row_cloner::QuitRowActions::default()
            },
        )
    };
    if !arm.is_complete() {
        append_log(
            &log_dir(),
            format_args!(
                "some of the rows' machinery did not install: {arm:?}; the rows may be absent or inert"
            ),
        );
    }
}

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
///
/// Called by the Windows loader. Do not call directly.
pub unsafe extern "system" fn DllMain(
    module: *mut core::ffi::c_void,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // First, before anything that can panic. A panic in a cdylib crosses an
        // `extern "system"` boundary and becomes an abort, which does not dispatch to a
        // vectored handler -- so `er_crash_logging` writes no record at all and the process
        // just vanishes. This hook is what turns that silence into a file:line. The hook is
        // per-DLL: every cdylib links its own `er-game-base`, so another shell installing it
        // does nothing here. Enforced by `scripts/check-panic-reporter-installed.py`.
        er_game_base::panic_report::report_panics_to("er-quit-menu", standalone_log);

        let module_base = module as usize;
        START.call_once(|| {
            // Before the thread, so no moved code can run against an un-installed seam.
            install_standalone_host();
            append_log(
                &log_dir(),
                format_args!(
                    "loaded module_base=0x{module_base:x}; arming all four cloned System>Quit rows"
                ),
            );
            std::thread::spawn(arm_build_rows);
        });
    }
    DLL_MAIN_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standalone_host_installs_exactly_once() {
        assert!(install_host_once());
        assert!(!install_host_once());
    }

    fn install_host_once() -> bool {
        er_quit_menu_core::install_host(er_quit_menu_core::QuitMenuHost {
            append_autoload_debug: standalone_log,
            append_crash_log: standalone_log,
            ..er_quit_menu_core::QuitMenuHost::defaults()
        })
    }
}
