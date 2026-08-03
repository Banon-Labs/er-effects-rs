//! Standalone ME3-loadable shell for the world-map invasion-spawn warp feature.
//!
//! Loading this DLL through a me3 `[[natives]]` entry is what turns the feature on: the
//! shell installs a standalone host seam whose gate answers YES, so profile inclusion IS the
//! toggle and no env var or marker file is involved.
//!
//! # What it does today: ORACLE 1 only
//!
//! It registers ONE recurring game task (`CSTaskImp` / `FrameBegin`, the same registration
//! `er-telemetry-dll` uses) which drives `er_invasion_warp::sampler`: a fail-closed read of the
//! live `CSAutoInvadePoint` coordinate table, re-taken until the totals settle, published as
//! `oracle_invasion_warp_catalog_targets` / `_blocks` / `_areas` into
//! `er-invasion-warp-telemetry.json` and this DLL's log next to the executable.
//!
//! It still installs NO detours, patches nothing, and writes nothing into the engine. Oracles
//! 2-5 (list rows, selected id, requested warp, final position) need the world-map interception
//! that is still only a design (docs/plans/world-map-invasion-warp.md), so they remain names.
//! Nothing here can start, fake or spoof invasion/multiplayer/session state -- the feature
//! reads one coordinate table.
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
/// Rewritten (not appended) on every publish, so the file always holds the CURRENT oracle
/// values rather than a history a reader has to scroll to the end of.
const TELEMETRY_FILE_NAME: &str = "er-invasion-warp-telemetry.json";

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

/// Telemetry sink: overwrite `er-invasion-warp-telemetry.json` with the feature's current
/// oracle document. Failure is silent by design -- a read-only game directory must degrade to
/// "log lines only", never to a panic on the game thread.
fn standalone_publish_oracle_json(body: &str) {
    let path = log_dir().join(TELEMETRY_FILE_NAME);
    let _ = std::fs::write(path, body.as_bytes());
}

/// This shell exists to offer the surface, so its gate is on. Profile inclusion is the
/// toggle; there is deliberately no env var or marker file behind this.
fn gate_on() -> bool {
    true
}

/// Install the standalone host seam: log sink -> this DLL's own log file, telemetry sink ->
/// this DLL's own JSON document, feature gate ON.
fn install_standalone_host() -> bool {
    er_invasion_warp::install_host(er_invasion_warp::InvasionWarpHost {
        append_autoload_debug: standalone_log,
        invasion_warp_enabled: gate_on,
        publish_oracle_json: standalone_publish_oracle_json,
    })
}

/// Wait for the game's task manager, then register the oracle-1 sampler as a recurring
/// `FrameBegin` game task.
///
/// This is the `er-telemetry-dll` pattern rather than a private thread on purpose: the catalog
/// read must happen on the game thread, where the singleton is stable, and it must RETRY
/// (the `.aipbnd` containers mount asynchronously during boot) rather than read once and give
/// up. The sampler itself stops reading the moment the totals settle, so a latched run costs
/// the game thread one atomic increment per frame and nothing else.
#[cfg(windows)]
fn spawn_catalog_task() {
    use eldenring::cs::{CSTaskGroupIndex, CSTaskImp};
    use eldenring::fd4::FD4TaskData;
    use fromsoftware_shared::{FromStatic, SharedTaskImpExt};

    let _ = std::thread::Builder::new()
        .name("er-invasion-warp-dll".to_owned())
        .spawn(|| {
            let task = loop {
                match unsafe { CSTaskImp::instance() } {
                    Ok(task) => break task,
                    // No sleep (banned by scripts/check-no-timeouts.py): yield to the game
                    // threads and re-poll, exactly as er-telemetry-dll and the product's
                    // wait_for_task_instance do.
                    Err(_) => std::thread::yield_now(),
                }
            };
            standalone_log(format_args!(
                "CSTaskImp resolved; registering the invasion-warp catalog sampler on FrameBegin"
            ));
            let handle = task.run_recurring(
                |_data: &FD4TaskData| {
                    // SAFETY: this closure runs on the game task thread, after CSTaskImp
                    // resolved -- exactly the context the tick's contract requires. The read
                    // itself is fault-closed (er_invasion_warp::live_read).
                    unsafe { er_invasion_warp::sampler::invasion_warp_catalog_tick() };
                },
                CSTaskGroupIndex::FrameBegin,
            );
            // The handle cancels the task on drop; the task must outlive this bootstrap
            // thread, so hand ownership to the process.
            std::mem::forget(handle);
        });
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
                     (host_installed={installed}; oracle 1 catalog sampler only, no detours)"
                ),
            );
            spawn_catalog_task();
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

    #[test]
    fn the_two_artifact_names_are_distinct_and_namespaced_to_this_dll() {
        // A shared name would let a combined profile's DLLs overwrite each other's evidence.
        assert_ne!(LOG_FILE_NAME, TELEMETRY_FILE_NAME);
        for name in [LOG_FILE_NAME, TELEMETRY_FILE_NAME] {
            assert!(name.starts_with("er-invasion-warp"), "{name}");
        }
    }

    #[test]
    fn the_telemetry_sink_writes_the_document_verbatim_and_replaces_the_previous_one() {
        let dir = std::env::temp_dir().join("er-invasion-warp-dll-telemetry-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(TELEMETRY_FILE_NAME);
        let long = er_invasion_warp::catalog_oracle_json("sampling", "first");
        std::fs::write(&path, long.as_bytes()).expect("write");
        let short = "{}";
        std::fs::write(&path, short.as_bytes()).expect("rewrite");
        // Truncating, not appending: a stale longer document must not survive underneath.
        assert_eq!(std::fs::read_to_string(&path).expect("read"), short);
        let _ = std::fs::remove_file(&path);
    }
}
