//! Which mods in this profile are fighting over the same key -- and does the game already use it.
//!
//! # The bug this exists to have caught
//!
//! In a live fifteen-DLL profile, `er-invasion-warp` polled `VK_F7` every frame and a second shell
//! had defaulted its own toggle to `F7`. Pressing F7 warped the player instead of toggling the
//! other feature. Nothing crashed, nothing logged an error, and nothing anywhere warned -- the
//! collision was found by a human noticing that a key did the wrong thing. Every piece of
//! information needed to predict it was already in the process; nobody was looking.
//!
//! This DLL looks. It watches the input APIs, works out which module each call came from, and once
//! the profile has settled it prints one warning naming every input that more than one party
//! wants.
//!
//! # Why it observes the APIs instead of asking the mods
//!
//! It has to work against any author's DLL. A third-party mod exports nothing you can query, ships
//! no manifest of its bindings, and will never cooperate. Sitting on `GetAsyncKeyState` and
//! attributing the caller works on a binary you have never seen, which is the only property that
//! makes the tool worth having.
//!
//! # It changes nothing
//!
//! Every detour chains to the original and returns its value untouched; no input is swallowed,
//! altered or injected; no param row is patched (that breaks Seamless invasions); no game memory
//! is written; nothing is loaded into the process that was not already there. The two reads it
//! does perform -- the loaded-module list and the game's own key-configuration table -- are loads.
//!
//! # What it cannot see
//!
//! Written down here rather than left to be discovered, because a blind spot the reader does not
//! know about is worse than one they do:
//!
//! * **A binding read through DirectInput has no key.** `GetDeviceState` returns all 256 scancodes
//!   at once and the caller picks its own afterwards, in its own code. Those modules are reported
//!   as whole-keyboard readers, explicitly as not checked. The one thing that does recover a key
//!   is a mod BLANKING it -- see [`dik`].
//! * **A mod that reads `inputmgr+0x90+eventId`** -- the game's decoded per-action keystate bitmap
//!   -- calls no API at all. Catching it needs a data breakpoint, which is not passive.
//! * **Action names.** The game's binding table gives an action index; the names live in an FMG
//!   message table, not in the executable, so the report prints the index.

// Ungated on purpose: the census, the attribution rule, the scancode table, the settle gate and
// the report renderer are pure and are exercised by `cargo test` on the host. That is most of the
// crate, and it is where a wrong answer would come from.
mod attribution;
mod census;
mod dik;
mod game_bindings;
mod log;
mod report;
mod settle;

#[cfg(windows)]
mod hooks;
#[cfg(windows)]
mod modules;
#[cfg(windows)]
mod overlay;

#[cfg(windows)]
use std::sync::{
    Mutex, Once,
    atomic::{AtomicUsize, Ordering},
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
use crate::{
    census::{Census, InputId},
    dik::{ConsumptionWatch, VK_DOWN_MASK},
    game_bindings::GameBindings,
    log::{conflict_log, elapsed_seconds, reset_log_file},
    modules::ModuleMap,
    report::{ReportInput, overlay_line, render},
    settle::SettleGate,
};

const DLL_MAIN_SUCCESS: i32 = 1;

/// Frames between attempts at the hooks that need a module the game loads later -- `dinput8.dll`
/// and whichever XInput redistributable is in use. The game frame is the retry clock, so nothing
/// sleeps waiting for them.
#[cfg(windows)]
const LAZY_HOOK_RETRY_TICKS: u64 = 60;

/// Frames between loaded-module snapshots. Enumerating 120 modules and naming each one is a
/// hundred-odd loader calls, so it is done twice a second rather than every frame; the cached
/// signature is what the settle gate reads in between.
#[cfg(windows)]
const MODULE_SAMPLE_TICKS: u64 = 120;

/// Frames between physical-key scans for the consumption check. Twice a second, over the ~110 keys
/// that have a DirectInput scancode -- which with a two-sample streak means a key has to be held
/// about a second to be reported.
#[cfg(windows)]
const CONSUMPTION_SCAN_TICKS: u64 = 30;

/// Frames between status lines, and between re-folds looking for a late collision. Ten seconds.
#[cfg(windows)]
const STATUS_LOG_TICKS: u64 = 600;

/// Attribution chains printed under the report. Enough to prove the mechanism works -- distinct
/// modules resolving from distinct call sites -- without turning the log into a stack dump.
#[cfg(windows)]
const MAX_CHAIN_DIAGNOSTICS: usize = 60;

#[cfg(windows)]
static START: Once = Once::new();
#[cfg(windows)]
static TICKS: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static MODULE_SIGNATURE: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static MODULE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// The module map, refreshed on a slow clock and read when a report is folded.
#[cfg(windows)]
static MODULE_MAP: Mutex<Option<ModuleMap>> = Mutex::new(None);
#[cfg(windows)]
static GATE: Mutex<Option<SettleGate>> = Mutex::new(None);
#[cfg(windows)]
static CONSUMPTION: Mutex<Option<ConsumptionWatch>> = Mutex::new(None);
/// Collisions already named in the report or in an amendment, so a late-collision sweep says each
/// thing once.
#[cfg(windows)]
static ANNOUNCED: Mutex<Vec<InputId>> = Mutex::new(Vec::new());

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetAsyncKeyState(vkey: i32) -> i16;
    fn GetForegroundWindow() -> isize;
    fn GetWindowThreadProcessId(window: isize, process_id: *mut u32) -> u32;
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcessId() -> u32;
}

/// Is the game the foreground window?
///
/// The consumption check depends on it. DirectInput at a foreground cooperative level returns an
/// empty buffer whenever the game does not have focus, while `GetAsyncKeyState` keeps answering --
/// so alt-tabbed, every held key would look like a key some mod had taken.
#[cfg(windows)]
fn game_has_focus() -> bool {
    // SAFETY: two argument-free Win32 queries and one with a live out-param.
    unsafe {
        let window = GetForegroundWindow();
        if window == 0 {
            return false;
        }
        let mut process_id: u32 = 0;
        GetWindowThreadProcessId(window, &mut process_id);
        process_id != 0 && process_id == GetCurrentProcessId()
    }
}

/// Virtual keys currently held, restricted to the ones that have a DirectInput scancode.
///
/// Wrapped in [`hooks::without_observing`] because this DLL's own detour sits on the very function
/// being called: without it, the census would report this module as a mod that polls every key on
/// the keyboard, which is both false and the single most confusing thing the report could say.
#[cfg(windows)]
fn physically_held_keys() -> Vec<u16> {
    hooks::without_observing(|| {
        dik::comparable_virtual_keys()
            .into_iter()
            // SAFETY: a Win32 call taking a virtual-key code and returning a bitfield.
            .filter(|vk| (unsafe { GetAsyncKeyState(i32::from(*vk)) } & VK_DOWN_MASK) != 0)
            .collect()
    })
}

#[cfg(windows)]
fn own_module_name() -> String {
    er_game_base::build_id::own_module()
        .map(|(_, name)| name)
        .unwrap_or_default()
}

/// Fold everything observed so far into a census, using the current module map.
#[cfg(windows)]
fn fold_census() -> (Census, Vec<String>) {
    let raw = hooks::snapshot_tally();
    let own = own_module_name();
    let guard = MODULE_MAP.lock();
    let resolve = |address: usize| -> Option<String> {
        guard
            .as_ref()
            .ok()
            .and_then(|slot| slot.as_ref())
            .and_then(|map| map.resolve(address))
            .map(str::to_string)
    };
    let game_module = modules::executable_name();
    attribution::fold(
        &raw,
        &own,
        hooks::union_host_module(),
        &game_module,
        resolve,
    )
}

/// Print the one warning.
#[cfg(windows)]
fn emit_report(reason: settle::Settled) {
    hooks::finalise_missed();
    let (census, chains) = fold_census();
    let bindings = game_bindings::read_from_game();
    let game_module = modules::executable_name();
    let consumed: Vec<u16> = CONSUMPTION
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(|watch| watch.reported().to_vec()))
        .unwrap_or_default();
    let armed = hooks::armed_surfaces();
    let missed = hooks::missed_surfaces();

    conflict_log(format_args!(
        "settled ({reason:?}): rendering the report. Attribution chains follow the banner."
    ));
    for line in render(&ReportInput {
        census: &census,
        game_module: &game_module,
        bindings: &bindings,
        consumed_keys: &consumed,
        observed_seconds: elapsed_seconds(),
        loaded_modules: MODULE_COUNT.load(Ordering::Relaxed),
        calls_seen: hooks::calls_seen(),
        surfaces_hooked: &armed,
        surfaces_missed: &missed,
    }) {
        conflict_log(format_args!("{line}"));
    }

    // The evidence for the report, not decoration. Attribution is the one claim here that can be
    // confidently wrong, and the way it fails is by resolving every call to one module. Printing
    // the resolved chains means that failure is visible in the log instead of showing up as a
    // warning naming an innocent DLL.
    conflict_log(format_args!(
        "attribution: {} distinct call site(s); each line is a resolved stack, innermost first, \
         with the fixed chain prefix that was stripped",
        chains.len()
    ));
    for line in chains.iter().take(MAX_CHAIN_DIAGNOSTICS) {
        conflict_log(format_args!("{line}"));
    }
    if chains.len() > MAX_CHAIN_DIAGNOSTICS {
        conflict_log(format_args!(
            "  ... {} more call site(s) not printed",
            chains.len() - MAX_CHAIN_DIAGNOSTICS
        ));
    }
    if let GameBindings::Table(table) = &bindings {
        conflict_log(format_args!(
            "game bindings: read {} bound keyboard action(s) from CSPcKeyConfig+0x440",
            table.len()
        ));
    }

    overlay::publish(overlay_line(&census, &game_module, &bindings));
    overlay::log_absent_host();
    if let Ok(mut announced) = ANNOUNCED.lock() {
        announced.extend(
            census
                .key_collisions(&game_module)
                .into_iter()
                .map(|collision| collision.input),
        );
    }
    hooks::mark_reported();
}

/// After the report, look for a collision that only appears later -- a hotkey a mod polls for the
/// first time inside a menu, an hour in.
#[cfg(windows)]
fn sweep_for_late_collisions() {
    let (census, _) = fold_census();
    let game_module = modules::executable_name();
    let Ok(mut announced) = ANNOUNCED.lock() else {
        return;
    };
    for collision in census.key_collisions(&game_module) {
        if announced.contains(&collision.input) {
            continue;
        }
        announced.push(collision.input);
        conflict_log(format_args!(
            "{}",
            report::amendment(collision.input, &collision.modules)
        ));
    }
    let bindings = game_bindings::read_from_game();
    overlay::publish(overlay_line(&census, &game_module, &bindings));
}

#[cfg(windows)]
fn sample_consumption() {
    if !game_has_focus() {
        return;
    }
    let Some(buffer) = hooks::keyboard_snapshot() else {
        return;
    };
    let held = physically_held_keys();
    if held.is_empty() {
        return;
    }
    let Ok(mut slot) = CONSUMPTION.lock() else {
        return;
    };
    let watch = slot.get_or_insert_with(ConsumptionWatch::default);
    for vk in watch.sample(&held, &buffer) {
        conflict_log(format_args!(
            "consumption: {} is held but the DirectInput buffer the game receives says it is up -- \
             a co-loaded mod is blanking it",
            InputId::Key(vk).describe()
        ));
    }
}

#[cfg(windows)]
fn tick() {
    let ticks = TICKS.fetch_add(1, Ordering::Relaxed) as u64;

    if ticks.is_multiple_of(LAZY_HOOK_RETRY_TICKS) {
        hooks::try_install_dinput();
        hooks::try_install_xinput();
        overlay::try_register();
    }

    if ticks.is_multiple_of(MODULE_SAMPLE_TICKS) {
        let map = ModuleMap::capture();
        // An empty capture is an enumeration failure, not a process with no modules in it.
        // Publishing it would blank every attribution and reset the settle gate's stability clock
        // on a transient, so the previous good map is kept instead.
        if !map.is_empty() {
            MODULE_SIGNATURE.store(map.signature() as usize, Ordering::Relaxed);
            MODULE_COUNT.store(map.len(), Ordering::Relaxed);
            if let Ok(mut slot) = MODULE_MAP.lock() {
                *slot = Some(map);
            }
        }
    }

    if ticks.is_multiple_of(CONSUMPTION_SCAN_TICKS) {
        sample_consumption();
    }

    let settled = {
        let Ok(mut slot) = GATE.lock() else {
            return;
        };
        let gate = slot.get_or_insert_with(SettleGate::default);
        gate.observe(
            MODULE_SIGNATURE.load(Ordering::Relaxed) as u64,
            hooks::calls_seen(),
        )
    };
    if let Some(reason) = settled {
        emit_report(reason);
        return;
    }

    if ticks > 0 && ticks.is_multiple_of(STATUS_LOG_TICKS) {
        let reported = GATE
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().map(SettleGate::fired))
            .unwrap_or(false);
        if reported {
            sweep_for_late_collisions();
        }
        let tally = hooks::snapshot_tally();
        conflict_log(format_args!(
            "status: reported={reported} calls={} recorded={} call_sites={} dropped={} modules={} \
             armed=[{}] overlay_guest={} overlay_draws={}",
            hooks::calls_seen(),
            tally.call_count(),
            tally.row_count(),
            tally.dropped(),
            MODULE_COUNT.load(Ordering::Relaxed),
            hooks::armed_surfaces().join(", "),
            overlay::registered(),
            overlay::draws()
        ));
    }
}

#[cfg(windows)]
fn spawn_game_task() {
    let _ = std::thread::Builder::new()
        .name("er-hotkey-conflicts-task".to_owned())
        .spawn(move || {
            conflict_log(format_args!("game task thread waiting for CSTaskImp"));
            // Bounded, and spinning in user space rather than yielding per attempt. A bare
            // `yield_now()` loop is one wineserver round trip per attempt, and the wineserver is
            // shared and serialising: measured 2026-08-29, two such threads starved the game to
            // 104 CPU ticks in three minutes with no window and no crash. See
            // `er_game_base::wait`, whose docs open with exactly the loop this replaced.
            let Some(task) =
                er_game_base::wait::poll_until(|| unsafe { CSTaskImp::instance() }.ok())
            else {
                // A shell that cannot find the task manager stays inert rather than hanging the
                // game. The user32 hooks are already installed and keep observing; only the
                // FrameBegin tick -- which is what prints the settled report -- is lost.
                conflict_log(format_args!(
                    "game task NOT registered: CSTaskImp never appeared within \
                     er_game_base::wait::MAX_YIELDS rounds. The input hooks stay armed and keep \
                     observing, but the settled conflict report will not print."
                ));
                return;
            };
            conflict_log(format_args!("game task registering FrameBegin tick"));
            task.run_recurring(
                move |_data: &FD4TaskData| tick(),
                CSTaskGroupIndex::FrameBegin,
            );
        });
}

#[cfg(windows)]
fn install() {
    reset_log_file();
    conflict_log(format_args!("{}", er_game_base::build_id::identity_line()));
    conflict_log(format_args!(
        "er-hotkey-conflicts attach: passive observer. Every detour chains to the original and \
         returns its value untouched; no input is swallowed, altered or injected, and no game \
         memory is written."
    ));
    hooks::install_user32();
    spawn_game_task();
}

/// Sink for `er_hook` address/detour lines and for panic reports, routed into this DLL's own log.
///
/// Thin on purpose: `conflict_log` already owns the file, the timestamps and the single-slot
/// rotation, so this only adapts the `fmt::Arguments` signature `er_hook` and `panic_report`
/// expect.
#[cfg(windows)]
fn log_sink(args: core::fmt::Arguments<'_>) {
    crate::log::conflict_log(args);
}

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
/// Standard Windows `DllMain`; on attach it only starts an installer thread. Nothing that takes a
/// lock runs under the loader lock.
pub unsafe extern "system" fn DllMain(
    _module: HINSTANCE,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // One sink for this DLL's hook + address lines. Without it a refused address is silent
        // here: every cdylib links its own copy of er-hook, so the default no-op sink is this
        // module's own, and `scripts/check-hook-log-sink.py` fails the build precisely because a
        // DLL that installs detours with no sink cannot tell you which address it declined.
        // `report_panics_to` rides along for the same reason -- a `rust_panic` inside a cdylib
        // loaded into the game is otherwise anonymous, leaving a 0xe06d7363 record that names the
        // module and nothing else.
        er_game_base::panic_report::report_panics_to("er-hotkey-conflicts", log_sink);
        er_hook::set_hook_logger(log_sink);
        START.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-hotkey-conflicts-install".to_owned())
                .spawn(install);
        });
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_hotkey_conflicts_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}

// This module never hosts the overlay (see `overlay`), but it links the host crate, and every
// shell that links it must define the export or it becomes a host no guest can find.
#[cfg(windows)]
er_build_watermark_core::export_overlay_host!();
