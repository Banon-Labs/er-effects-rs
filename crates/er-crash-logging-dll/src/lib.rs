//! Standalone ME3-loadable crash-logging DLL.
//!
//! Add this DLL as a `[[natives]]` entry beside `ersc.dll` to capture crash-like
//! first-chance exceptions into `er-crash-log.txt` / `er-crash-latest.txt` in the
//! game directory without loading the product DLL.

#![allow(non_snake_case)]

use std::sync::Once;

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;

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
        START.call_once(|| {
            er_crash_logging::install(
                er_crash_logging::CrashLogConfig {
                    log_file_name: "er-crash-log.txt",
                    latest_file_name: "er-crash-latest.txt",
                    breadcrumb_file_name: "er-crash-breadcrumb-latest.txt",
                    module_label: "er-crash-logging-dll",
                },
                module as usize,
            );
            er_crash_logging::write_breadcrumb("dll-attach", format_args!("standalone active"));
        });
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_crash_logging_dll_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}
