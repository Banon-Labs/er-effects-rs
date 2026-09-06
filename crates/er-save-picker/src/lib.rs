//! Standalone ME3-loadable shell for product (A), the DLL-drawn boot save picker.
//!
//! This DLL is deliberately separate from the product `er_quickload.dll`, following the
//! `er-loading-bar` / `er-loading-portrait` shape: the feature crate owns the picker
//! logic, this thin shell installs a standalone host seam and arms the boot picker when loaded
//! by ME3.
//!
//! Co-loading stays conservative when the product DLL is already present: this standalone shell
//! does not install its host or arm, so the product remains the owner of the boot flow. S6 does not
//! claim a standalone-first co-load proof; when loaded by itself this DLL owns a standalone pending
//! latch, opens the picker model, starts the low-level keyboard hook, and records selected paths in
//! its own log. It does not install product save-redirect hooks; a standalone pick is validated and
//! planned through `er-save-redirect`, then closes the standalone latch instead of pretending to
//! install hooks or load the game save.

// A cdylib whose every consumer is `DllMain` and the hooks it installs, all of them
// `#[cfg(windows)]`. On a host build the shell is compiled with its only callers cfg'd
// out, so `dead_code`/`unused_imports` there report the cfg, not real debt. The SHIPPING
// target (x86_64-pc-windows-msvc) carries the full deny with no allows.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(windows)]
use windows::Win32::System::LibraryLoader::GetModuleHandleA;
#[cfg(windows)]
use windows::core::PCSTR;

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;
const LOG_FILE_NAME: &str = "er-save-picker.log";

#[cfg(windows)]
static START: std::sync::Once = std::sync::Once::new();

static STANDALONE_MISSING_SAVE_PENDING: AtomicBool = AtomicBool::new(true);

/// Where the standalone log lands: next to the executable, falling back to the CWD.
fn log_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Fresh per process: the first line of a run truncates the file (rotating the previous
/// run's aside as `.log.prev`), later lines append. No log in this repo accumulates across
/// runs -- which save path a run picked must be readable without splitting a file by hand.
fn append_log(dir: &Path, args: std::fmt::Arguments<'_>) {
    er_game_base::log::append_line(
        &dir.join(LOG_FILE_NAME),
        format_args!("er-save-picker: {args}"),
    );
}

/// The standalone host seam. There is no product save hook owner behind this DLL, so the
/// completion callback validates/plans the pick and releases this DLL's own picker latch instead of
/// claiming it activated autoload.
fn install_standalone_host() -> bool {
    er_save_picker_core::install_host(er_save_picker_core::SavePickerHost {
        append_autoload_debug: standalone_log,
        missing_save_selection_pending: standalone_missing_save_selection_pending,
        complete_missing_save_selection_from_picker: standalone_complete_missing_save_selection,
        picker_start_dir: standalone_picker_start_dir,
        remember_picker_dir: standalone_remember_picker_dir,
        ..er_save_picker_core::SavePickerHost::defaults()
    })
}

fn standalone_log(args: std::fmt::Arguments<'_>) {
    append_log(&log_dir(), args);
}

fn standalone_missing_save_selection_pending() -> bool {
    STANDALONE_MISSING_SAVE_PENDING.load(Ordering::SeqCst)
}

fn standalone_complete_missing_save_selection(
    path: &Path,
) -> er_save_picker_core::MissingSaveSelectionOutcome {
    use er_save_picker_core::MissingSaveSelectionOutcome;
    let validated = match er_save_redirect::validate_save_file_path(path.to_path_buf()) {
        Ok(validated) => validated,
        Err(err) => {
            let message = standalone_rejection_message(err);
            standalone_log(format_args!(
                "standalone pick rejected by shared save-source validation: '{}' -- {err:?} visible='{}: {}'",
                path.display(),
                message.headline(),
                message.detail()
            ));
            return MissingSaveSelectionOutcome::Rejected(message);
        }
    };
    let plan = er_save_redirect::plan_validated_save_source(validated.clone(), false);
    standalone_log(format_args!(
        "standalone pick accepted for surface proof: '{}' plan={plan:?} (no save hook owner installed)",
        validated.display()
    ));
    STANDALONE_MISSING_SAVE_PENDING.store(false, Ordering::SeqCst);
    MissingSaveSelectionOutcome::Completed
}

fn standalone_rejection_message(
    err: er_save_redirect::SaveSourceRejection,
) -> er_save_picker_core::PickerStatusMessage {
    // Wording lives on the rejection itself; both picker surfaces had their own copy of this
    // match, and two copies of a user-facing explanation is two chances to describe the same
    // failure differently.
    let (title, message) = err.picker_status();
    er_save_picker_core::PickerStatusMessage::new(title, message)
}

fn standalone_picker_start_dir() -> PathBuf {
    // ME3 launches with the game directory as CWD on the approved path. Starting there is more
    // useful than an empty model and does not invent a user-specific save path.
    std::env::current_dir()
        .ok()
        .filter(|path| path.exists())
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(PathBuf::from))
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

fn standalone_remember_picker_dir(dir: &Path) {
    standalone_log(format_args!(
        "standalone picker remembered directory for this run only: '{}'",
        dir.display()
    ));
}

#[cfg(windows)]
fn product_dll_present() -> bool {
    unsafe { GetModuleHandleA(PCSTR(c"er_quickload.dll".as_ptr().cast::<u8>())).is_ok() }
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
        // FIRST, before anything that can panic. A panic in a cdylib crosses an
        // `extern "system"` boundary and becomes an ABORT, which does not dispatch to a
        // vectored handler -- so `er_crash_logging` writes no record at all and the process
        // just vanishes. This hook is what turns that silence into a file:line. The hook is
        // per-DLL: every cdylib links its own `er-game-base`, so another shell installing it
        // does nothing here. Enforced by `scripts/check-panic-reporter-installed.py`.
        er_game_base::panic_report::report_panics_to("er-save-picker", standalone_log);

        let module_base = module as usize;
        START.call_once(|| {
            if product_dll_present() {
                STANDALONE_MISSING_SAVE_PENDING.store(false, Ordering::SeqCst);
                append_log(
                    &log_dir(),
                    format_args!(
                        "loaded module_base=0x{module_base:x}; product DLL already present; standalone boot-save-picker stood down before host install"
                    ),
                );
                return;
            }

            let host_installed = install_standalone_host();
            STANDALONE_MISSING_SAVE_PENDING.store(true, Ordering::SeqCst);
            let armed = er_save_picker_core::overlay::arm_boot_picker();
            er_save_picker_core::overlay::ensure_save_picker_keyboard_hook();
            append_log(
                &log_dir(),
                format_args!(
                    "loaded module_base=0x{module_base:x}; standalone boot-save-picker armed={armed}; host_installed={host_installed}; standalone-first co-load is not S6 proof; start_dir='{}'",
                    standalone_picker_start_dir().display()
                ),
            );
        });
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_save_picker_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_host_installs_once_and_refuses_invalid_picks_without_releasing_the_latch() {
        STANDALONE_MISSING_SAVE_PENDING.store(true, Ordering::SeqCst);
        assert!(install_standalone_host());
        assert!(!install_standalone_host());
        assert!(standalone_missing_save_selection_pending());
        assert!(matches!(
            standalone_complete_missing_save_selection(Path::new("Z:\\saves\\ER0000.sl2")),
            er_save_picker_core::MissingSaveSelectionOutcome::Rejected(_)
        ));
        assert!(standalone_missing_save_selection_pending());
    }

    #[test]
    fn standalone_start_dir_is_non_empty() {
        assert!(!standalone_picker_start_dir().as_os_str().is_empty());
    }
}
