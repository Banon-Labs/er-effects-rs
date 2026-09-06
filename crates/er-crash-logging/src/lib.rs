//! Standalone ME3-loadable crash-logging DLL.
//!
//! Add this DLL as a `[[natives]]` entry beside `ersc.dll` to capture crash-like
//! first-chance exceptions into `er-crash-log.txt` / `er-crash-latest.txt` in the
//! game directory without loading the product DLL.

use std::sync::Once;

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_PROCESS_DETACH: u32 = 0;
const DLL_MAIN_SUCCESS: i32 = 1;

/// Main-thread stall window before the hang watchdog reports a freeze.
///
/// A lockup produces no exception at all, so without this the standalone DLL is silent through the
/// exact failure it was added beside `ersc.dll` to capture.
const HANG_STALL_SECONDS: u64 = 30;

static START: Once = Once::new();

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
        // vectored handler -- so no crash record is written at all and the process simply
        // vanishes. Enforced by `scripts/check-panic-reporter-installed.py`.
        er_game_base::panic_report::report_panics_to(
            "er-crash-logging",
            er_crash_logging_core::append_log,
        );

        START.call_once(|| {
            er_crash_logging_core::install(
                er_crash_logging_core::CrashLogConfig {
                    log_file_name: "er-crash-log.txt",
                    latest_file_name: "er-crash-latest.txt",
                    breadcrumb_file_name: "er-crash-breadcrumb-latest.txt",
                    modules_file_name: "er-crash-modules.txt",
                    minidump_file_name: "er-crash-minidump.dmp",
                    hang_report_file_name: "er-crash-hang-latest.txt",
                    hang_minidump_file_name: "er-crash-hang-minidump.dmp",
                    hang_stall_seconds: HANG_STALL_SECONDS,
                    module_label: "er-crash-logging",
                },
                module as usize,
            );
            er_crash_logging_core::write_breadcrumb(
                "dll-attach",
                format_args!("standalone loaded"),
            );
        });
    }
    if reason == DLL_PROCESS_DETACH {
        // Distinguishes an orderly shutdown from a process that died where it stood.
        er_crash_logging_core::note_process_detach();
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_crash_logging_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}
