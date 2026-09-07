//! Standalone "apply one SpEffect to every loaded enemy" toggle DLL.
//!
//! Press the configured hotkey in-game and every enemy currently loaded is put under the
//! configured `effect_id` (Charming Branch by default, but any SpEffectParam row works -- the
//! shipped config uses `[Incantation] Darkness`); while the toggle is on, any enemy that is not
//! under it -- one that just
//! spawned, or one whose 180 seconds ran out -- is put back under it on the next frame. Press it
//! again and the effect is stripped back off.
//!
//! Ships as its own `er_enemynpc_effects.dll`, listed as its own ME3 `[[natives]]` entry. It shares
//! no state with the product DLL and hooks nothing but DirectInput's keyboard read.

#[cfg(windows)]
mod charm;
// Ungated on purpose: pure text parsing over a config file, so its tests run on the host.
mod config;
#[cfg(windows)]
mod hotkey;
// Ungated on purpose: a scancode table and a hotkey parser, so their tests run on the host.
mod keys;
mod log;

#[cfg(windows)]
use std::sync::{
    Once,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[cfg(windows)]
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp},
    fd4::FD4TaskData,
};
#[cfg(windows)]
use fromsoftware_shared::{FromStatic, SharedTaskImpExt};
#[cfg(windows)]
use windows::Win32::{Foundation::HINSTANCE, System::SystemServices::DLL_PROCESS_ATTACH};

#[cfg(windows)]
use crate::{charm::SweepMode, log::charm_log};

const DLL_MAIN_SUCCESS: i32 = 1;

/// Frames between attempts to install the DirectInput hook. `dinput8.dll` is not loaded yet when
/// this DLL attaches, so the first attempts are expected to fail; the game frame is the retry
/// clock, so there is no sleeping thread waiting for it.
#[cfg(windows)]
const HOOK_INSTALL_RETRY_TICKS: usize = 60;

/// Frames between status lines. At 60fps this is roughly every ten seconds.
#[cfg(windows)]
const STATUS_LOG_TICKS: usize = 600;

/// Minimum frames between "applied to N enemies" lines. Without it, one character the game
/// declines to give the effect to would produce a line every frame forever: the sweep would find
/// it un-charmed, apply, find it un-charmed again next frame, and say so 60 times a second.
#[cfg(windows)]
const APPLY_LOG_MIN_TICKS: usize = 30;

#[cfg(windows)]
static START: Once = Once::new();
#[cfg(windows)]
static ENABLED: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static TICKS: AtomicUsize = AtomicUsize::new(0);
/// Set when the toggle goes off, cleared once the strip-everything sweep has run.
#[cfg(windows)]
static DISABLE_SWEEP_PENDING: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static TOTAL_APPLIED: AtomicUsize = AtomicUsize::new(0);
/// Tick of the last "applied" line, for [`APPLY_LOG_MIN_TICKS`].
#[cfg(windows)]
static LAST_APPLY_LOG_TICK: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static TOTAL_REMOVED: AtomicUsize = AtomicUsize::new(0);

#[cfg(windows)]
fn wait_for_task_instance() -> Option<&'static CSTaskImp> {
    // Bounded (2026-08-29). This was `loop { yield_now() }`. On 1.17 the singleton did not turn
    // up promptly and two such loops starved the wineserver: the game reached 104 CPU ticks in
    // three minutes while these threads burned 19,000 each, half of it system time. See
    // er_game_base::wait for the measurement.
    er_game_base::wait::poll_until(|| unsafe { CSTaskImp::instance() }.ok())
}

/// Put the toggle into force, from wherever it moved -- a keypress or the config file.
///
/// `remove_on_disable` is passed in rather than read here because both callers already hold, or
/// have just released, the config lock; taking it again inside would be one nested lock away from
/// a deadlock for no benefit.
#[cfg(windows)]
fn apply_enabled_state(enabled: bool, remove_on_disable: bool) {
    ENABLED.store(enabled, Ordering::SeqCst);
    // Turning it on cancels a strip sweep that has not run yet; turning it off schedules one, but
    // only if the player asked for the charm to be taken back off rather than left to lapse.
    DISABLE_SWEEP_PENDING.store(!enabled && remove_on_disable, Ordering::SeqCst);
}

/// Consume the presses the hook queued. An even number is a press and a release of the toggle, so
/// only the parity matters.
#[cfg(windows)]
fn consume_hotkey_presses() {
    // An even count is either nothing at all or a pair that cancels out, and in both cases the
    // toggle ends up where it started.
    if hotkey::take_pending_toggles().is_multiple_of(2) {
        return;
    }
    let enabled = !ENABLED.fetch_xor(true, Ordering::SeqCst);

    // Write it back. Without this the toggle is a per-session thing and every launch starts off,
    // which for a feature you turn on and leave on is the same as not remembering it at all. The
    // write re-reads the file first, so it also picks up an edit made since the last poll.
    let outcome = config::persist_enabled(enabled);
    let config = config::config();
    apply_enabled_state(enabled, config.remove_on_disable);

    match &outcome.error {
        None => charm_log(format_args!(
            "hotkey: charm-all-enemies toggled {}; wrote enabled={enabled} to {}",
            if enabled { "ON" } else { "OFF" },
            config.config_path.display()
        )),
        Some(error) => charm_log(format_args!(
            "hotkey: charm-all-enemies toggled {}; FAILED to write {} ({error}) -- the toggle is \
             live but will not survive a relaunch",
            if enabled { "ON" } else { "OFF" },
            config.config_path.display()
        )),
    }

    // The read-back can carry an edit the player made in the last second. It is in force already;
    // this names it and resets the key edge state if the hotkey itself is what moved.
    config::log_update(&outcome.update);
    if outcome.update.hotkey_moved.is_some() {
        hotkey::rebind(config::live_hotkey());
    }
}

/// Re-read `er-enemynpc-effects.toml` and adopt anything that moved.
///
/// The reload itself is throttled inside `er_hotkey_config::HotFile`, so calling this every frame
/// costs one integer comparison in the steady state. A moved hotkey goes to [`hotkey::rebind`],
/// which is what clears the edge state -- without that, a key held at the instant of the swap
/// reads as a press the player never made.
#[cfg(windows)]
fn poll_config_reload() {
    let Some(update) = config::poll_reload() else {
        return;
    };
    config::log_update(&update);
    if update.hotkey_moved.is_some() {
        hotkey::rebind(config::live_hotkey());
    }
    // Editing `enabled` by hand is a second way to work the toggle, and it has to do everything
    // the keypress does -- including scheduling the strip sweep when it goes off.
    if let Some((_, enabled)) = update.enabled_moved {
        apply_enabled_state(enabled, config::config().remove_on_disable);
    }
}

#[cfg(windows)]
fn tick() {
    poll_config_reload();
    let config = config::config();
    let ticks = TICKS.fetch_add(1, Ordering::Relaxed);

    if !hotkey::hook_installed() && ticks.is_multiple_of(HOOK_INSTALL_RETRY_TICKS) {
        // A successful install logs its own line, naming the MinHook instance that took it.
        if let Err(status) = hotkey::install_hotkey_hook(config.hotkey())
            && ticks == 0
        {
            charm_log(format_args!(
                "hotkey: DirectInput hook not available yet ({status:?}); retrying every {HOOK_INSTALL_RETRY_TICKS} frames"
            ));
        }
    }

    consume_hotkey_presses();

    let enabled = ENABLED.load(Ordering::SeqCst);
    if enabled {
        let counts = charm::sweep(config.effect_id, SweepMode::Apply);
        if counts.applied > 0 {
            TOTAL_APPLIED.fetch_add(counts.applied, Ordering::Relaxed);
            let last = LAST_APPLY_LOG_TICK.load(Ordering::Relaxed);
            if ticks == 0 || ticks.saturating_sub(last) >= APPLY_LOG_MIN_TICKS {
                LAST_APPLY_LOG_TICK.store(ticks, Ordering::Relaxed);
                charm_log(format_args!(
                    "charm: applied SpEffect {} to {} of {} loaded enemies ({} already charmed, {} charmable, {} refused, {} speffect rows held across them)",
                    config.effect_id,
                    counts.applied,
                    counts.enemies,
                    counts.already_charmed,
                    counts.charm_eligible,
                    counts.apply_refused,
                    counts.existing_entries
                ));
            }
        }
    } else if DISABLE_SWEEP_PENDING.swap(false, Ordering::SeqCst) {
        let counts = charm::sweep(config.effect_id, SweepMode::Remove);
        TOTAL_REMOVED.fetch_add(counts.removed, Ordering::Relaxed);
        charm_log(format_args!(
            "charm: removed SpEffect {} from {} of {} loaded enemies",
            config.effect_id, counts.removed, counts.enemies
        ));
    }

    if ticks.is_multiple_of(STATUS_LOG_TICKS) {
        // Count even when the toggle is off, so the log shows whether the enemy walk is finding
        // anything without needing the feature turned on to find out.
        let counts = charm::sweep(config.effect_id, SweepMode::Count);
        let (persist_writes, persist_failures) = config::persist_tallies();
        charm_log(format_args!(
            "status: enabled={} persisted_enabled={} persist_writes={} persist_failures={} hotkey={} hook={} loaded_enemies={} charmed_now={} charmable={} speffect_rows={} keyboard_reads={} non_keyboard_reads={} suppressed_trigger_reads={} applied_total={} removed_total={}",
            enabled,
            config.enabled,
            persist_writes,
            persist_failures,
            er_hotkey_config::chord_name(config.hotkey()),
            hotkey::hook_installed(),
            counts.enemies,
            counts.already_charmed,
            counts.charm_eligible,
            counts.existing_entries,
            hotkey::keyboard_reads(),
            hotkey::non_keyboard_reads(),
            hotkey::suppressed_trigger_reads(),
            TOTAL_APPLIED.load(Ordering::Relaxed),
            TOTAL_REMOVED.load(Ordering::Relaxed)
        ));
    }
}

#[cfg(windows)]
fn spawn_game_task() {
    let _ = std::thread::Builder::new()
        .name("er-enemynpc-effects-task".to_owned())
        .spawn(move || {
            charm_log(format_args!("game task thread waiting for CSTaskImp"));
            let Some(task) = wait_for_task_instance() else {
                charm_log(format_args!(
                    "CSTaskImp never appeared; this shell stays inert rather than spinning"
                ));
                return;
            };
            charm_log(format_args!("game task registering FrameBegin tick"));
            task.run_recurring(
                move |_data: &FD4TaskData| tick(),
                CSTaskGroupIndex::FrameBegin,
            );
        });
}

#[cfg(windows)]
fn install() {
    log::reset_log_file();
    let config = config::init_config();
    // Restore. `enabled` is written back on every toggle, so whatever the player left it as is
    // what the file says now; starting from the built-in `false` regardless would make the
    // write-back pointless.
    //
    // The store is direct rather than through `apply_enabled_state` because that one also
    // schedules the strip sweep when the state is off, and at attach there is nothing charmed to
    // strip -- it would spend the first frame walking the world to remove an effect from nobody
    // and say so in the log.
    ENABLED.store(config.enabled, Ordering::SeqCst);
    charm_log(format_args!(
        "er-enemynpc-effects attach: enabled={} (restored from the config) hotkey={:?} ({}) \
         effect_id={} remove_on_disable={} config={} \
         -- edits to that file take effect while the game runs, no restart",
        config.enabled,
        config.hotkey_text,
        er_hotkey_config::chord_name(config.hotkey()),
        config.effect_id,
        config.remove_on_disable,
        config.config_path.display()
    ));
    spawn_game_task();
}

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
/// Standard Windows `DllMain`; on attach it only starts an installer thread.
pub unsafe extern "system" fn DllMain(
    _module: HINSTANCE,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // One sink for this DLL's hook + address lines. Without it a refused address is
        // silent here, because every cdylib links its own copy of er-hook/er-game-base.
        // A rust_panic in a cdylib loaded into the game is otherwise anonymous: the message goes to a
        // stderr nobody reads, and what survives is a 0xe06d7363 record naming the module and nothing
        // else. Two boots were lost to one before this existed. See er_game_base::panic_report.
        er_game_base::panic_report::report_panics_to("er-enemynpc-effects", crate::charm_log);
        er_hook::set_hook_logger(crate::charm_log);
        START.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-enemynpc-effects-install".to_owned())
                .spawn(install);
        });
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_enemynpc_effects_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}
