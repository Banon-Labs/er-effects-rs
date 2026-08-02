//! Standalone ME3-loadable shell for product (A), the DLL-drawn boot save picker.
//!
//! SCAFFOLDING ONLY: the shell exists, logs its attach, and installs a standalone host
//! seam. The boot overlay now has an explicit `er_save_picker::overlay::arm_boot_picker()`
//! entrypoint, but this DLL still arms nothing until the standalone smoke/profile wiring
//! lands in the later DLL-realization slice.
//!
//! Deliberately separate from the product `er_effects_rs.dll`, same pattern as
//! `er-loading-bar-dll` / `er-loading-portrait-dll`: it proves the feature crate builds
//! and loads as its own native DLL without dragging product hooks, autoload or runtime
//! state along.
//!
//! It is ALSO designed to be co-loadable with every other DLL we ship, which those two
//! predecessors are not -- see this crate's Cargo.toml for the two rules that make that
//! true.

#![allow(non_snake_case)]

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;
const LOG_FILE_NAME: &str = "er-save-picker-dll.log";

#[cfg(windows)]
static START: std::sync::Once = std::sync::Once::new();

/// Where the standalone log lands: next to the executable, falling back to the CWD.
fn log_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn append_log(dir: &PathBuf, args: std::fmt::Arguments<'_>) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(LOG_FILE_NAME))
    {
        let _ = writeln!(file, "er-save-picker-dll: {args}");
    }
}

/// The standalone host seam: this DLL has no product behind it, so every product-owned
/// answer stays at its neutral default and only the log sink is real.
fn install_standalone_host() {
    let _ = er_save_picker::install_host(er_save_picker::SavePickerHost {
        append_autoload_debug: standalone_log,
        ..er_save_picker::SavePickerHost::defaults()
    });
}

fn standalone_log(args: std::fmt::Arguments<'_>) {
    append_log(&log_dir(), args);
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
        let module_base = module as usize;
        START.call_once(|| {
            install_standalone_host();
            append_log(
                &log_dir(),
                format_args!(
                    "loaded module_base=0x{module_base:x}; standalone boot-save-picker shell (scaffolding: arm_boot_picker not wired yet)"
                ),
            );
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
        er_save_picker::install_host(er_save_picker::SavePickerHost {
            append_autoload_debug: standalone_log,
            ..er_save_picker::SavePickerHost::defaults()
        })
    }
}
