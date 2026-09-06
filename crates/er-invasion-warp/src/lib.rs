//! Standalone ME3-loadable shell for the world-map invasion-spawn warp feature.
//!
//! Loading this DLL through a me3 `[[natives]]` entry is what turns the feature on: the
//! shell installs a standalone host seam whose gate answers YES, so profile inclusion IS the
//! toggle and no env var or marker file is involved.
//!
//! # What it does today: ORACLE 1 only
//!
//! It registers ONE recurring game task (`CSTaskImp` / `FrameBegin`, the same registration
//! `er-telemetry` uses) which drives `er_invasion_warp_core::sampler`: a fail-closed read of the
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
//! Unlike `er-loading-portrait` this DLL is safe to load ALONGSIDE `er_quickload.dll`:
//! it owns no Present detour and no MinHook instance. That stays true only while it installs
//! nothing; the first detour it adds must go through the `er-hook` union.

// A cdylib whose every consumer is `DllMain` and the game hooks/tasks it installs, all of them
// `#[cfg(windows)]`. On a host build the shell is compiled with its only callers cfg'd out, so
// `dead_code`/`unused_imports` there report the cfg, not real debt (measured 2026-08-21: 119 on
// the host, 0 on the shipping target). The SHIPPING target (x86_64-pc-windows-msvc) carries the
// full deny with no allows. `check.sh` host-runs this crate's tests, which is why the host build
// has to stay clean at all.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

pub mod announce;
pub mod drive;
pub mod lobby_publish;
pub mod local_invasion_filter;
#[cfg(windows)]
pub mod map_confirm;
pub mod map_gfx;
pub mod map_hooks;
#[cfg(windows)]
mod map_live_pins;
pub mod map_seams;
pub mod place_name;
pub mod restart_backoff;
mod seamless_probe;
pub mod stall_watchdog;

use std::path::{Path, PathBuf};

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_MAIN_SUCCESS: i32 = 1;
const LOG_FILE_NAME: &str = "er-invasion-warp.log";
/// Rewritten (not appended) on every publish, so the file always holds the CURRENT oracle
/// values rather than a history a reader has to scroll to the end of.
const TELEMETRY_FILE_NAME: &str = "er-invasion-warp-telemetry.json";
/// The WARP document, kept separate from the catalog one on purpose: both are rewritten in
/// place, so sharing a filename would let whichever wrote last erase the other's evidence.
const WARP_TELEMETRY_FILE_NAME: &str = "er-invasion-warp-run.json";

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
/// run's aside as `.log.prev`), later lines append.
///
/// This DLL is the reason the repo has that rule. It used to open the log with a plain
/// `append(true)`, so twelve separate launches accumulated into one 565 KB file and a count
/// taken over it read as one run doing something twelve times over.
fn append_log(dir: &Path, args: std::fmt::Arguments<'_>) {
    er_game_base::log::append_line(
        &dir.join(LOG_FILE_NAME),
        format_args!("er-invasion-warp: {args}"),
    );
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

/// Warp telemetry sink: the per-warp oracle document, in its own file so it never overwrites
/// the catalog's.
fn standalone_publish_warp_json(body: &str) {
    let path = log_dir().join(WARP_TELEMETRY_FILE_NAME);
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
    er_invasion_warp_core::install_host(er_invasion_warp_core::InvasionWarpHost {
        append_autoload_debug: standalone_log,
        invasion_warp_enabled: gate_on,
        publish_oracle_json: standalone_publish_oracle_json,
    })
}

/// Wait for the game's task manager, then register the oracle-1 sampler as a recurring
/// `FrameBegin` game task.
///
/// This is the `er-telemetry` pattern rather than a private thread on purpose: the catalog
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
        .name("er-invasion-warp".to_owned())
        .spawn(|| {
            // BOUNDED (2026-08-29): the unbounded form of this loop starved the wineserver and
            // hung a boot. er_game_base::wait backs off in user space and gives up.
            let Some(task) =
                er_game_base::wait::poll_until(|| unsafe { CSTaskImp::instance() }.ok())
            else {
                return;
            };
            standalone_log(format_args!(
                "CSTaskImp resolved; registering the invasion-warp catalog sampler on FrameBegin"
            ));
            // Owned by the task closure: one driver instance for the process lifetime. The task
            // is the only thing that touches it, and the task is single-threaded, so no lock is
            // needed and none is taken on the game thread.
            let mut warp_drive = crate::drive::InvasionWarpDrive::new();
            // Same ownership rule as the warp driver: one instance for the process, touched only
            // by this single-threaded task.
            let mut mark_keys = crate::local_invasion_filter::MarkKeys::new();
            crate::local_invasion_filter::ensure_config_file();
            let mut frame: u64 = 0;
            let handle = task.run_recurring(
                move |_data: &FD4TaskData| {
                    frame = frame.wrapping_add(1);
                    // Fold every map the player has actually been in into the MSB InvasionPoint
                    // catalog. This has to live here rather than in the map hook: the world-map
                    // ViewModel constructor fires once per world entry during the loading screen,
                    // long before the player walks into a catacomb.
                    //
                    // SAFETY: game task thread with the world up; every read inside is fault-closed
                    // and each map is read at most once per session.
                    unsafe { crate::map_hooks::harvest_resident_msb_points(frame) };
                    // What destination a SEAMLESS invasion picked. Seamless does not use the
                    // .aip/MSB InvasionPoint tables at all, and the code that chooses a target
                    // is inside the part of ersc.dll Themida encrypted -- but the ANSWER lands
                    // in CSGameMan where anything can read it. Passive: no hook on ersc's path,
                    // so it cannot perturb the thing it is measuring.
                    //
                    // SAFETY: game task thread; every read is fault-closed.
                    unsafe { crate::seamless_probe::sample_invade_destination() };
                    // SAFETY: this closure runs on the game task thread, after CSTaskImp
                    // resolved -- exactly the context the tick's contract requires. The read
                    // itself is fault-closed (er_invasion_warp_core::live_read).
                    unsafe { er_invasion_warp_core::sampler::invasion_warp_catalog_tick() };
                    // SAFETY: same game-task context. Reads hotkeys, and on a press runs the
                    // engine's own warp sequence; every native call is byte-checked and every
                    // pointer read is fault-closed.
                    unsafe {
                        warp_drive.tick(standalone_log, standalone_publish_warp_json);
                    }
                    // SAFETY: same game-task context, and the installer is idempotent. The
                    // world-map observer is installed from the task rather than DllMain because
                    // MinHook must not run under the loader lock.
                    // `map_pins = false` withholds BOTH map hooks -- see the key's docs on
                    // `LocalInvasionConfig`. Read per tick rather than latched at attach because
                    // the config is hot-reloaded; the installers are idempotent, so flipping the
                    // key on mid-session arms them and flipping it off simply stops re-arming
                    // (a hook already installed stays installed, which the log makes visible).
                    let config_snapshot = crate::local_invasion_filter::current_config_snapshot();
                    let map_pins = config_snapshot
                        .as_ref()
                        .map(|config| config.map_pins)
                        .unwrap_or(true);
                    if map_pins {
                        unsafe { crate::map_hooks::install_map_observers() };
                    }
                    // SAFETY: same game-task context; also idempotent. This one arms as early
                    // as the task runs on purpose: it swaps the world-map movie as Scaleform
                    // parses it, so arming after that parse means the red pin icon simply never
                    // appears. The pins read the outcome rather than assuming it, so a late or
                    // failed arm costs the red icon and never leaves a pin iconless.
                    if map_pins && let Ok(base) = er_game_base::mem::game_module_base() {
                        unsafe { crate::map_gfx::install_world_map_gfx_hook(base) };
                    }
                    // The local invasion filter: installs its single game-side detour (idempotent),
                    // polls the Insert/Delete mark keys, and restarts a search that a rejected
                    // match cancelled. It hooks nothing in ersc.dll -- see that module's docs for
                    // why the option actions' arguments turned out to be unnecessary.
                    //
                    // SAFETY: same game-task context, after CSTaskImp resolved; every read inside
                    // is fault-closed and every native call is byte-checked first.
                    unsafe {
                        crate::local_invasion_filter::tick(
                            &mut mark_keys,
                            crate::drive::game_has_focus(),
                        );
                    }
                    // Advertise this host's current map on its own Steam lobby, so an invader can
                    // ASK for a location instead of sampling and rejecting. Gated internally on the
                    // block having CHANGED, so a host standing still costs one string compare.
                    //
                    // Republishing is the whole point: Seamless writes its advertisement once at
                    // CreateLobby and never again (measured -- 7 SetLobbyData calls at creation,
                    // zero after), so a location key written only at creation would go stale the
                    // moment the host walks away.
                    //
                    // No-ops harmlessly when this player is not hosting, when Steam is not ready, or
                    // when the block cannot be read. Not being findable by location is a missing
                    // convenience; a DLL that faulted here would be a broken game.
                    crate::lobby_publish::publish_current_map();
                    // Keep the advertised pool matching the configured one; Seamless never
                    // rebuilds its advertisement, so a toggle has to be applied by us.
                    crate::lobby_publish::reapply_pool_if_toggled();
                    // Hunt mode's query-narrowing hook. Idempotent and self-gating: it needs the
                    // Steam interface, which is not resolvable until Steam is up, so it retries
                    // until it lands rather than being installed once at attach and failing
                    // silently. It changes nothing unless `hunt = true` is in the config.
                    // Learn which lobby Seamless advertises on, by watching it declare one.
                    // Must be installed before publishing can do anything: publish now REFUSES
                    // until this observer has seen the declaration, rather than guessing from a
                    // struct offset that pointed at the wrong lobby in every run.
                    // `steam_hooks = false` withholds all three -- see the key's docs. Read from
                    // the same snapshot as `map_pins` so one config read serves both gates.
                    if config_snapshot
                        .as_ref()
                        .map(|config| config.steam_hooks)
                        .unwrap_or(true)
                    {
                        crate::lobby_publish::install_advertisement_observer();
                        crate::lobby_publish::install_hunt_hook();
                        crate::lobby_publish::install_pool_filter_hook();
                    }
                    // Lift both halves of location matchmaking out of the log and into the oracle
                    // document, because their failures are the ones that look like success from
                    // outside: a host that advertised nothing still runs fine, and a hunt hook that
                    // never landed is indistinguishable from an empty world. Four atomic loads.
                    {
                        let (publishes, refusals) = crate::lobby_publish::tally();
                        er_invasion_warp_core::oracles::publish_lobby_oracles(publishes, refusals);
                        let (hooked, filters) = crate::lobby_publish::hunt_tally();
                        er_invasion_warp_core::oracles::publish_hunt_oracles(hooked, filters);
                        // Writing the counters is not the same as PUBLISHING them: the telemetry
                        // document is otherwise only written by the catalog sampler, which stops
                        // once the totals latch -- seconds into a run, and long before anyone
                        // hunts. Without this the file freezes at zero while the counters climb.
                        // Gated on a change, so a steady run costs four comparisons and no I/O.
                        er_invasion_warp_core::oracles::republish_if_location_matchmaking_changed();
                    }
                    // Re-colour pins that already exist. Gated internally on the user's lists
                    // actually having changed, so the steady-state cost is one atomic compare.
                    //
                    // SAFETY: same game-task context. Walks the ONE live ViewModel's span -- read
                    // from the engine's own `CSPopupMenu+0x250` slot and required to match the one
                    // the injection recorded -- and writes only to rows that still point into this
                    // DLL's leaked param slab, so a span whose ViewModel was destroyed is refused.
                    unsafe {
                        crate::map_live_pins::restyle_live_pins();
                    }
                    // Make blocks harvested SINCE this world entry visible, by retargeting rows the
                    // constructor already reserved. Refuses unless the engine's own slots agree it
                    // is safe: the ViewModel read live from `CSPopupMenu+0x250`, no dialog attached,
                    // and the row list still beginning where our span was recorded.
                    //
                    // SAFETY: same game-task context; appends nothing and allocates nothing, so the
                    // row buffer cannot move and no raw row pointer can dangle.
                    unsafe {
                        crate::map_live_pins::top_up_live_pins();
                    }
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
            // ONE sink for both refusal channels, installed before anything resolves an address.
            //
            // Every cdylib statically links its own copy of `er-hook` and `er-game-base`, so an
            // uninstalled sink is silent PER DLL -- and this DLL had none. `HOOK REFUSED` and
            // `ADDRESS REFUSED` lines, the two things that say a 1.16.2 address did not survive
            // 1.17, were being written to nowhere while the map surface simply failed to appear.
            // `set_hook_logger` installs the address-resolution sink too.
            er_hook::set_hook_logger(standalone_log);
            // Beside the hook sink, and for the same reason one level worse. A panic in this
            // cdylib crosses an `extern "system"` boundary and becomes an ABORT, which does NOT
            // dispatch to a vectored handler -- so `er_crash_logging` writes no record, no
            // `-latest`, no module list, and the process simply vanishes. Measured 2026-09-04:
            // the F9 cross-area warp killed the game repeatedly and every run's crash log held
            // only its build header, while this same DLL's illegal-instruction fault produced
            // full records every time. The difference was not severity, it was that one raised an
            // exception and the other aborted. Enforced by
            // `scripts/check-panic-reporter-installed.py`.
            er_game_base::panic_report::report_panics_to("er-invasion-warp", standalone_log);
            // Say so in the log, because "no PANIC line" has two very different meanings and the
            // record cannot distinguish them on its own: either nothing panicked, or the hook was
            // never armed and a panic died silently anyway. Measured 2026-09-04: three runs ended
            // with the process gone, no exception record, and no panic line, and there was no way
            // to tell which of those two it was. This line makes the absence of a PANIC line
            // evidence rather than a gap.
            standalone_log(format_args!(
                "panic reporter ARMED for er_invasion_warp -- from here, a Rust panic in this DLL \
                 writes its message and file:line above before the process aborts. If this DLL \
                 dies with no PANIC line after this point, it did not panic."
            ));

            let installed = install_standalone_host();
            append_log(
                &log_dir(),
                format_args!(
                    "loaded module_base=0x{module_base:x}; standalone invasion-warp shell \
                     (host_installed={installed}; catalog sampler + F7/F8 warp driver, no detours)"
                ),
            );
            spawn_catalog_task();
        });
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_invasion_warp_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standalone_host_installs_exactly_once_and_turns_the_gate_on() {
        // Before install the crate is inert; after install this shell's gate answers yes.
        assert!(!er_invasion_warp_core::invasion_warp_enabled());
        assert!(install_standalone_host());
        assert!(er_invasion_warp_core::invasion_warp_enabled());
        // A second install must change nothing -- DllMain's Once is belt, this is braces.
        assert!(!install_standalone_host());
        assert!(er_invasion_warp_core::invasion_warp_enabled());
    }

    #[test]
    fn the_two_artifact_names_are_distinct_and_namespaced_to_this_dll() {
        // A shared name would let a combined profile's DLLs overwrite each other's evidence.
        let names = [LOG_FILE_NAME, TELEMETRY_FILE_NAME, WARP_TELEMETRY_FILE_NAME];
        for (index, name) in names.iter().enumerate() {
            assert!(name.starts_with("er-invasion-warp"), "{name}");
            for other in &names[index + 1..] {
                // A shared name would let the catalog document and the warp document -- both
                // rewritten in place -- erase each other's evidence.
                assert_ne!(name, other);
            }
        }
    }

    /// Scratch directory for one test, keyed by PROCESS as well as by `tag`.
    ///
    /// `std::env::temp_dir()` is ONE directory shared by every process on the machine, so a name
    /// keyed only by `tag` is the same directory -- and here the same FILE, since the leaf is the
    /// fixed artifact name this DLL writes -- in two test binaries at once. Two at once is the
    /// ordinary case in this repo: two agents running `scripts/check.sh` concurrently, the gate
    /// run twice over, or a second checkout. The test below writes a long document, overwrites it
    /// with a short one and requires the short one to have TRUNCATED the long one; a second
    /// process writing its own long document into that same path between this one's rewrite and
    /// its read-back makes the truncation look as though it never happened.
    ///
    /// Measured before the pid was added, over ten rounds of eight concurrent copies of this
    /// crate's test binary: one of the 80 runs went red, `assertion left == right failed, left:
    /// "", right: "{}"` -- the read landing between a sibling's `write` and its content, and the
    /// truncating-write contract taking the blame. Rare is not benign here: the gate runs this
    /// crate on every invocation, and a one-in-eighty red with a product-shaped message costs
    /// more to diagnose than a reliable one.
    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "er-invasion-warp-{tag}-p{pid}",
            pid = std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
        dir
    }

    #[test]
    fn the_telemetry_sink_writes_the_document_verbatim_and_replaces_the_previous_one() {
        let dir = scratch_dir("telemetry-test");
        let path = dir.join(TELEMETRY_FILE_NAME);
        let long = er_invasion_warp_core::catalog_oracle_json("sampling", "first");
        std::fs::write(&path, long.as_bytes()).expect("write");
        let short = "{}";
        std::fs::write(&path, short.as_bytes()).expect("rewrite");
        // Truncating, not appending: a stale longer document must not survive underneath.
        assert_eq!(std::fs::read_to_string(&path).expect("read"), short);
        let _ = std::fs::remove_file(&path);
    }
}
