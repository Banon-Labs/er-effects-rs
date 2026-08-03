//! Standalone ME3-loadable shell for the world-map invasion-spawn warp feature.
//!
//! Loading this DLL through a me3 `[[natives]]` entry is what turns the feature on: the
//! shell installs a standalone host seam whose gate answers YES, so profile inclusion IS the
//! toggle and no env var or marker file is involved.
//!
//! SCAFFOLDING (bd er-effects-rs-5es): the shell attaches, arms the seam and logs. It installs
//! no detours yet, because the world-map interception is still a design backed by decompiles
//! (docs/plans/world-map-invasion-warp.md) rather than landed code. Nothing it does can start,
//! fake or spoof invasion/multiplayer/session state -- the feature reads one coordinate table.
//!
//! Unlike `er-loading-portrait-dll` this DLL is safe to load ALONGSIDE `er_effects_rs.dll`:
//! it owns no Present detour and no MinHook instance. That stays true only while it installs
//! nothing; the first detour it adds must go through the `er-hook` union.

#![allow(non_snake_case)]

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;
const LOG_FILE_NAME: &str = "er-invasion-warp-dll.log";

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
        let _ = writeln!(file, "er-invasion-warp-dll: {args}");
    }
}

fn standalone_log(args: std::fmt::Arguments<'_>) {
    append_log(&log_dir(), args);
}

/// This shell exists to offer the surface, so its gate is on. Profile inclusion is the
/// toggle; there is deliberately no env var or marker file behind this.
fn gate_on() -> bool {
    true
}

/// Install the standalone host seam: log sink -> this DLL's own log file, feature gate ON.
fn install_standalone_host() -> bool {
    er_invasion_warp::install_host(er_invasion_warp::InvasionWarpHost {
        append_autoload_debug: standalone_log,
        invasion_warp_enabled: gate_on,
    })
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
            let installed = install_standalone_host();
            append_log(
                &log_dir(),
                format_args!(
                    "loaded module_base=0x{module_base:x}; standalone invasion-warp shell \
                     (host_installed={installed}; scaffolding: no detours armed yet)"
                ),
            );
        });
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_invasion_warp_dll_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standalone_host_installs_exactly_once_and_turns_the_gate_on() {
        // Before install the crate is inert; after install this shell's gate answers yes.
        assert!(!er_invasion_warp::invasion_warp_enabled());
        assert!(install_standalone_host());
        assert!(er_invasion_warp::invasion_warp_enabled());
        // A second install must change nothing -- DllMain's Once is belt, this is braces.
        assert!(!install_standalone_host());
        assert!(er_invasion_warp::invasion_warp_enabled());
    }
}
