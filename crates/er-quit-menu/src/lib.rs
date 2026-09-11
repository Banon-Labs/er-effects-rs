//! Standalone ME3 shell for the System>Quit build rows: **Load Build from URL** and
//! **Generate Build Link**, with no product DLL in the profile.
//!
//! Everything the rows are made of lives in `er-quit-menu-core`, which the product DLL also links
//! and arms. The difference between the two loads is one argument: the product passes
//! `RowSet::ALL` and the four flows this crate does not own, while this shell passes
//! `RowSet::BUILD_ROWS_ONLY` and none. A row that is not in the set is never cloned, so a press
//! that could reach a flow this shell does not have never happens -- the row is not on the tab.
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
//! # Never in the same profile as the product
//!
//! Both offer the same two rows and both derive the same movie, and `er_gfx::options_02_040::quit6`
//! fail-closes when its input is not vanilla -- so a second deriver handed already-derived bytes
//! correctly refuses. `scripts/me3-dll-conflicts.toml` records the pair as a duplicate owner, and
//! the profile generator refuses to emit a profile carrying both.
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

/// Arm the two build rows. Runs on its own thread because the game-task registration waits for the
/// game's task manager to exist, and waiting inside the loader lock deadlocks the process.
#[cfg(windows)]
fn arm_build_rows() {
    // Safety: a bootstrap thread, once per process (`START` gates the spawn), before the Quit tab
    // has built a dialog.
    let arm = unsafe {
        er_quit_menu_core::arm::arm_standalone(
            er_quit_menu_core::row_cloner::RowSet::BUILD_ROWS_ONLY,
            // Both build rows are driven from inside the feature crate, so this shell supplies no
            // flow of its own. The four entries that stay `None` belong to rows this set leaves
            // off the tab.
            er_quit_menu_core::row_cloner::QuitRowActions::default(),
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
                    "loaded module_base=0x{module_base:x}; arming the System>Quit build rows"
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
