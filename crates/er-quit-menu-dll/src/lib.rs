//! Standalone ME3-loadable shell for product (B), the customized System>Quit menu.
//!
//! SCAFFOLDING ONLY (phase 1 of docs/plans/save-picker-crate-extraction.md): the shell
//! exists, logs its attach, and installs a standalone host seam. It arms nothing yet,
//! because `er-quit-menu` still has no moved code to arm.
//!
//! Unlike `er-loading-portrait-dll`, this DLL is meant to be loaded ALONGSIDE the product
//! and the other companions. It contends on game addresses the product also hooks, so its
//! detours go through the `er-hook` union -- never a bare `MhHook::new` -- and the union
//! owner is elected at load time rather than assumed to be `er_effects_rs.dll`.

#![allow(non_snake_case)]

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;
const LOG_FILE_NAME: &str = "er-quit-menu-dll.log";

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
        let _ = writeln!(file, "er-quit-menu-dll: {args}");
    }
}

/// The standalone host seam: this DLL has no product behind it, so every product-owned
/// answer stays at its neutral default -- including the save-write bypass, which must stay
/// refused so a product-less load can never push a write past `er-save-suppress`.
fn install_standalone_host() {
    let _ = er_quit_menu::install_host(er_quit_menu::QuitMenuHost {
        append_autoload_debug: standalone_log,
        append_crash_log: standalone_log,
        ..er_quit_menu::QuitMenuHost::defaults()
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
                    "loaded module_base=0x{module_base:x}; standalone quit-menu shell (scaffolding: nothing armed yet)"
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
        er_quit_menu::install_host(er_quit_menu::QuitMenuHost {
            append_autoload_debug: standalone_log,
            append_crash_log: standalone_log,
            ..er_quit_menu::QuitMenuHost::defaults()
        })
    }
}
