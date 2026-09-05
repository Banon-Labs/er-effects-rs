//! er-focus-input -- ELDEN RING accepts input while its window is UNFOCUSED.
//!
//! A standalone `er_focus_input.dll`, loaded as its own `[[natives]]` entry. Its mere PRESENCE
//! enables it: no env var, no marker file, no config. Omit it from the profile to get vanilla
//! focus behaviour back.
//!
//! # What it is for
//!
//! Agent-driven runtime validation cannot require the game to hold OS focus -- the user needs
//! their focus elsewhere, and a run that silently depends on it produces evidence for a state
//! nobody arranged. Measured 2026-09-05 (run `br-20260905-031000-5ac5`): the product's move probe
//! wrote the forward stick into the live `FD4PadDevice` for 150 frames
//! (`oracle_supplied_movement_input_frames = 150`), the character moved on 5
//! (`oracle_did_move_frames = 5`), `oracle_can_move` stayed false, and the ER window was mapped and
//! visible but not focused. `crates/er-focus-input/src/predicate.rs` decodes why, instruction by
//! instruction, and this shell forces the one byte that ends it.
//!
//! # Mechanism, in one line
//!
//! `CS::CSPadStep::STEP_Update` skips the whole input update on an unfocused frame UNLESS
//! `Game.Debug.IsEnableControlOnDisactiveWindow()` is true. That accessor is a single
//! `movzx eax, byte ptr [<global>]`, and this shell writes that global from the game thread each
//! frame. Read `predicate.rs` before changing anything here -- it carries the addresses, the two
//! independent 1.17 corroborations, and the three candidate mechanisms that were eliminated.
//!
//! # Why it installs no detour
//!
//! The intervention is a byte store into a `.data` global. There is no prologue to detour, so
//! there is nothing for a second MinHook instance to overwrite and nothing for `er-hook`'s union
//! to arbitrate -- which is what makes this shell co-loadable with the product and with every
//! other shell in the workspace unconditionally. See `scripts/me3-dll-conflicts.toml`.

// A cdylib whose only consumers are `DllMain` and the `#[cfg(windows)]` game task it registers. On
// a host build those callers are cfg'd out, so `dead_code` there reports the cfg rather than real
// debt; the SHIPPING target carries the full workspace deny. Same shape as er-input-harness.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

mod log;
mod predicate;

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

#[cfg(windows)]
use std::sync::Once;

#[cfg(windows)]
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp},
    fd4::FD4TaskData,
};
#[cfg(windows)]
use fromsoftware_shared::{FromStatic, SharedTaskImpExt};
#[cfg(windows)]
use windows::Win32::{Foundation::HINSTANCE, System::SystemServices::DLL_PROCESS_ATTACH};

use crate::log::{focus_log, reset_log_file};

const DLL_MAIN_SUCCESS: i32 = 1;

/// How many successful stores to let by before the log goes quiet. The first few frames are the
/// evidence that the address resolved and the store landed; frame 5,000 saying the same thing is
/// 5,000 lines of nothing (see `er_game_base::repeat` for what that costs).
const FORCED_FRAMES_LOGGED: u64 = 3;

/// Cached game image base, resolved once the game image is mapped.
static GAME_BASE: AtomicUsize = AtomicUsize::new(0);

/// Frames on which the store succeeded. Also the log throttle.
static FORCED_FRAMES: AtomicU64 = AtomicU64::new(0);

/// Frames on which the store was REFUSED -- an unresolved address on an unrecognised build. Counted
/// separately so a run can tell "the shell did nothing" apart from "the shell was not loaded", and
/// so the refusal is reported once rather than every frame.
static REFUSED_FRAMES: AtomicU64 = AtomicU64::new(0);

/// Frames on which the secondary DLUID latch was held. Reported separately from
/// [`FORCED_FRAMES`] precisely because it is expected to be inert on a stock build -- if the
/// unfocused input ever starts working only when this counter moves, the default-false reading of
/// `Ext.UserInput.CooperativeLevel.SetForeGround.*` in `predicate.rs` is wrong and should be
/// re-measured rather than believed.
static DLUID_HELD_FRAMES: AtomicU64 = AtomicU64::new(0);

/// Resolve (and cache) the game image base; 0 until the image is mapped.
fn resolve_base() -> usize {
    let cached = GAME_BASE.load(Ordering::SeqCst);
    if cached != 0 {
        return cached;
    }
    let base = er_game_base::mem::game_module_base().unwrap_or(0);
    if base != 0 {
        GAME_BASE.store(base, Ordering::SeqCst);
    }
    base
}

/// One frame's work: assert the debug byte, and report the first few outcomes.
fn on_frame() {
    let base = resolve_base();
    if base == 0 {
        return;
    }
    // Secondary latch first, and unconditionally: it depends on a singleton that appears later
    // than the game image, so tying it to the primary store's success would hide its own progress.
    if predicate::hold_dluid_input_active(base) {
        let held = DLUID_HELD_FRAMES.fetch_add(1, Ordering::Relaxed);
        if held == 0 {
            focus_log!(
                "unfocused-input: holding DLUID+0x88d = 1 as well (secondary latch; expected inert \
                 on a stock build -- see predicate::hold_dluid_input_active)"
            );
        }
    }
    if predicate::force_control_on_disactive_window(base) {
        let forced = FORCED_FRAMES.fetch_add(1, Ordering::Relaxed);
        if forced < FORCED_FRAMES_LOGGED {
            focus_log!(
                "unfocused-input: forced Game.Debug.IsEnableControlOnDisactiveWindow byte \
                 (base=0x{base:x} rva=0x{:x}) -- reads back {} after store (frame {})",
                predicate::GAME_DEBUG_ENABLE_CONTROL_ON_DISACTIVE_WINDOW_RVA,
                predicate::control_on_disactive_window(base),
                forced + 1,
            );
        }
        return;
    }
    // Refused: the running build has no verified translation for this global, so nothing was
    // written. Say so ONCE -- a per-frame refusal line is the 339,764-line failure mode
    // `er_game_base::game_build` exists to avoid.
    if REFUSED_FRAMES.fetch_add(1, Ordering::Relaxed) == 0 {
        focus_log!(
            "unfocused-input: REFUSED -- no verified address for \
             GAME_DEBUG_ENABLE_CONTROL_ON_DISACTIVE_WINDOW_RVA (0x{:x}) on this build ({}). \
             The window-focus requirement is UNCHANGED; this shell is inert.",
            predicate::GAME_DEBUG_ENABLE_CONTROL_ON_DISACTIVE_WINDOW_RVA,
            er_game_base::game_build::describe_build(),
        );
    }
}

#[cfg(windows)]
static START: Once = Once::new();

#[cfg(windows)]
fn install() {
    reset_log_file();
    focus_log!(
        "er-focus-input attach: forcing Game.Debug.IsEnableControlOnDisactiveWindow from a \
         CSTaskImp FrameBegin task -- no detour, no OS input, one byte store per frame ({})",
        er_game_base::game_build::describe_build(),
    );
    // BOUNDED wait for the task manager: the unbounded yield loop every shell used to open with
    // starved the wineserver and hung a boot (see er_game_base::wait).
    let Some(task) = er_game_base::wait::poll_until(|| unsafe { CSTaskImp::instance() }.ok())
    else {
        focus_log!(
            "er-focus-input: CSTaskImp never appeared within the bounded wait -- staying inert \
             rather than spinning. The window-focus requirement is UNCHANGED."
        );
        return;
    };
    task.run_recurring(
        |_data: &FD4TaskData| on_frame(),
        CSTaskGroupIndex::FrameBegin,
    );
    focus_log!("er-focus-input: FrameBegin task registered");
}

/// # Safety
///
/// Standard `DllMain` contract; called by the Windows loader, never directly. On attach it only
/// spawns a thread -- no loader-lock work.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    _module: HINSTANCE,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        // FIRST, before anything that can panic. A panic in a cdylib crossing an `extern "system"`
        // boundary becomes an ABORT, and what survives is an anonymous 0xe06d7363 record naming
        // the module and nothing else. Enforced by scripts/check-panic-reporter-installed.py.
        er_game_base::panic_report::report_panics_to("er-focus-input", crate::log::log_line);
        START.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-focus-input-install".to_owned())
                .spawn(install);
        });
    }
    DLL_MAIN_SUCCESS
}

// Non-windows: keep the crate buildable for host tooling / workspace resolution.
#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_focus_input_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}
