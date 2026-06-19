//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

#![allow(unused_imports)]

use std::{
    ffi::c_void,
    fmt::Write as _,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use debug::{InputBlocker, InputFlags};
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use hudhook::{
    ImguiRenderLoop, MessageFilter,
    hooks::dx12::ImguiDx12Hooks,
    imgui::{Condition, Context, Ui},
    mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook},
    windows::{
        Win32::{
            Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM},
            System::{
                LibraryLoader::{GetModuleHandleA, GetProcAddress},
                Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery},
                SystemServices::DLL_PROCESS_ATTACH,
                Threading::GetCurrentProcessId,
            },
            UI::WindowsAndMessaging::{
                ClipCursor, EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
                WM_KEYDOWN, WM_KEYUP,
            },
        },
        core::{BOOL, PCSTR},
    },
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, ffi::*, hooks::*, telemetry::*};

pub(crate) fn game_module_base() -> Result<usize, String> {
    let module = unsafe { GetModuleHandleA(PCSTR::null()) }
        .map_err(|error| format!("failed to resolve game module: {error}"))?;
    Ok(module.0 as usize)
}

pub(crate) fn game_rva(rva: u32) -> Result<usize, String> {
    Ok(game_module_base()? + rva as usize)
}

/// Kill-switch to skip installing the continue_trace hooks (bisecting a ~19s
/// title crash caused by our DLL). When set, the continue/load-flow hooks are
/// not installed even if autoload is configured.
/// Bisect kill-switch: when set, the recurring game task does nothing each
/// frame, so we can tell whether the per-frame task body or the DLL's mere
/// presence is what terminates the title ~19s in.
pub(crate) fn inert_mode() -> bool {
    matches!(std::env::var("ER_EFFECTS_INERT").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-inert.txt")
            .exists()
}

/// Bisect kill-switch: the recurring task does lock + tick only, with no
/// filesystem I/O. Lets us tell whether the per-frame file I/O (telemetry write)
/// is what stalls the title vs. any per-frame work at all.
pub(crate) fn lite_mode() -> bool {
    matches!(std::env::var("ER_EFFECTS_LITE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-lite.txt")
            .exists()
}

pub(crate) fn continue_trace_disabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_NO_CONTINUE_TRACE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-no-continue-trace.txt")
        .exists()
}

pub(crate) fn trace_continue_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TRACE_CONTINUE").as_deref(),
        Ok("1")
    ) || trace_continue_default_path().exists()
        || PathBuf::from("er-effects-trace-continue.txt").exists()
}

pub(crate) fn trace_menu_task_update_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TRACE_MENU_TASK_UPDATE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-trace-menu-task-update.txt")
        .exists()
}

pub(crate) fn native_title_job_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_AUTOLOAD_NATIVE_TITLE_JOB").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-native-title-job.txt")
        .exists()
}

pub(crate) fn force_play_game_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_AUTOLOAD_FORCE_PLAY_GAME").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-force-play-game.txt")
        .exists()
}

pub(crate) fn selectbot_probe_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_SELECTBOT_PROBE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-selectbot-probe.txt")
        .exists()
}

/// Read-only runtime validation for the SelectBot selection-injection lane.
///
/// Static RE (runs 300/301) decoded the pump's selection path but the SelectBot
/// registry is FromSoftware's internal test-automation channel, so it may be
/// empty/inactive in the retail build. Before reversing the registry write API
/// and attempting an injection, this samples the live state each frame: the
/// SimpleTitleStep owner state (+0x4c), title queue (+0x128), parsed selection
/// (+0x130), the registry root pointer ([0x143d87360]) and the load-active gate
/// byte ([0x143d856a0]). It never writes game memory. A non-null registry with
/// an idle pump (state stable, queue/selection empty, gate 0) confirms the
/// injection target is real and reachable; a null registry means the SelectBot
/// harness is not initialized and the lane needs a different entry.
pub(crate) unsafe fn selectbot_probe_once(module_base: usize, tick: u64) {
    if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL != TITLE_OWNER_SCAN_START_ADDRESS as u64 {
        return;
    }
    // Owner-independent module globals: sample these ALWAYS. After the latch
    // advances the inner TitleStep to Finish (state 11 -> -1) the inner owner is
    // torn down, but `pump_ran` (does the outer MenuLoop spin up?) and the latch
    // byte live in module globals, so we must still capture them post-cascade.
    let registry = unsafe { *((module_base + SELECTBOT_REGISTRY_GLOBAL_RVA) as *const usize) };
    let load_gate = unsafe { *((module_base + SELECTBOT_LOAD_GATE_RVA) as *const u8) };
    let input_manager =
        unsafe { *((module_base + SELECTBOT_INPUT_MANAGER_GLOBAL_RVA) as *const usize) };
    let pump_ran = if input_manager != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { *((input_manager + SELECTBOT_PUMP_RAN_FLAG_OFFSET) as *const u8) }
    } else {
        DIRECT_INPUT_FAILURE_HRESULT as u8
    };
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        append_autoload_debug(format_args!(
            "selectbot_probe: owner not resolved registry={registry:#x} load_gate={load_gate} input_mgr={input_manager:#x} pump_ran={pump_ran} tick={tick}"
        ));
        return;
    };
    let state = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    let queue128 = unsafe { *(owner.add(SELECTBOT_OWNER_TITLE_QUEUE_128_OFFSET) as *const usize) };
    let selection130 =
        unsafe { *(owner.add(SELECTBOT_OWNER_PARSED_SELECTION_130_OFFSET) as *const i32) };
    append_autoload_debug(format_args!(
        "selectbot_probe: state={state} queue128={queue128:#x} selection130={selection130} registry={registry:#x} load_gate={load_gate} input_mgr={input_manager:#x} pump_ran={pump_ran} tick={tick}"
    ));
    // Lever-1 title-accept experiment: set the proceed latch [0x143d856a0]=1 ONCE,
    // only while the inner owner is confirmed at MenuJobWait (state 10), so the
    // native MenuJobWait handler advances itself to state 11 (Finish) on its next
    // tick. Sampling continues above so the cascade (state, pump_ran, registry) is
    // observed after the write. Gated separately from the read-only probe.
    if title_proceed_gate_enabled()
        && state == TITLE_STEP_MENU_JOB_WAIT_STATE
        && !TITLE_PROCEED_GATE_FIRED.swap(true, Ordering::SeqCst)
    {
        unsafe {
            *((module_base + SELECTBOT_LOAD_GATE_RVA) as *mut u8) = TITLE_PROCEED_GATE_SET_VALUE;
        }
        let after = unsafe { *((module_base + SELECTBOT_LOAD_GATE_RVA) as *const u8) };
        append_autoload_debug(format_args!(
            "title_proceed_gate: set [0x143d856a0]={after} at state {state} tick={tick}"
        ));
    }
}

pub(crate) fn title_proceed_gate_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TITLE_PROCEED_GATE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-title-proceed-gate.txt")
        .exists()
}

pub(crate) fn ingamestep_pump_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_INGAMESTEP_PUMP").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-ingamestep-pump.txt")
        .exists()
}

/// Directly drives the orphaned InGameStep load to completion, called once per
/// game-thread frame from the recurring CSTask (NOT a hook — detouring the hot
/// step pump `0x140b0bd60` froze the title state machine, run 305).
///
/// `force_play_game` advances the inner TitleStep to GameStepWait (state 6) and
/// submits the load (`job+0xd8=1`), but the InGameStep step machine is a
/// parent-ticked child the title scheduler never routes to in the forced state,
/// so the load orphans. The InGameStep's own Execute pump is `0x140b0bd60`
/// (FD4StepTemplate::Execute, signature `execute(&mut self, &FD4TaskData)`), so
/// we call it directly on the InGameStep (`owner+0x2e8`) with the live
/// `FD4TaskData` the CSTask already supplies — the exact ctx the task system
/// would pass. The step handlers drain `job+0xd8` 1 -> 2 -> 0 and load the world.
pub(crate) unsafe fn ingamestep_pump_tick(module_base: usize, task_data: &FD4TaskData) {
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        return;
    };
    let inner_state = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    if inner_state != TITLE_STEP_GAME_STEP_WAIT {
        return;
    }
    let ingame = unsafe { *(owner.add(TITLE_OWNER_JOB_OFFSET) as *const *mut u8) };
    if ingame.is_null() {
        return;
    }
    // Sample the InGameStep step machine. step_state (+0x48) is the CURRENT step,
    // next (+0x4c) is where it wants to go: if next advances while cur lags, the
    // machine IS progressing (real wait is downstream). The override fields
    // (+0x69/+0xa8/+0xac) reveal whether the pump force-re-stamps the step index
    // each frame (which would pin it). Log on change of (next, d8) to trace it.
    let cur = unsafe { *(ingame.add(INGAMESTEP_STEP_STATE_OFFSET) as *const i32) };
    let next = unsafe { *(ingame.add(INGAMESTEP_NEXT_STATE_OFFSET) as *const i32) };
    let d8 = unsafe { *(ingame.add(TITLE_OWNER_JOB_PENDING_OFFSET) as *const i32) };
    let ov_trigger = unsafe { *(ingame.add(INGAMESTEP_OVERRIDE_TRIGGER_OFFSET)) };
    let ov_guard = unsafe { *(ingame.add(INGAMESTEP_OVERRIDE_GUARD_OFFSET)) };
    let ov_target = unsafe { *(ingame.add(INGAMESTEP_OVERRIDE_TARGET_OFFSET) as *const i32) };
    let last_next = INGAMESTEP_PUMP_LAST_NEXT.swap(next, Ordering::SeqCst);
    let last_d8 = INGAMESTEP_PUMP_LAST_D8.swap(d8, Ordering::SeqCst);
    if next != last_next || d8 != last_d8 {
        append_autoload_debug(format_args!(
            "ingamestep_pump: cur={cur} next={next} d8={d8} ov_trigger={ov_trigger} ov_guard={ov_guard} ov_target={ov_target} ingame={ingame:p}"
        ));
    }
    if cur == INGAMESTEP_FINISHED_SENTINEL || d8 == INGAMESTEP_LOAD_DONE {
        return;
    }
    // Gated, one-shot "unpin": if the force-state override is re-stamping the step
    // index (trigger set, target == current stalled step), clear the trigger so
    // the natural step advance sticks. Read-only by default; opt in via
    // ER_EFFECTS_INGAMESTEP_UNPIN once the log confirms the machine is pinned.
    if ingamestep_unpin_enabled()
        && ov_trigger != INGAMESTEP_OVERRIDE_TRIGGER_CLEAR
        && ov_target == cur
        && !INGAMESTEP_UNPIN_DONE.swap(true, Ordering::SeqCst)
    {
        unsafe {
            *(ingame.add(INGAMESTEP_OVERRIDE_TRIGGER_OFFSET)) = INGAMESTEP_OVERRIDE_TRIGGER_CLEAR;
        }
        append_autoload_debug(format_args!(
            "ingamestep_pump: cleared force-override trigger (was {ov_trigger}, target={ov_target}) cur={cur} ingame={ingame:p}"
        ));
    }
    let Ok(pump) = game_rva(STEP_PUMP_DRIVER_RVA) else {
        return;
    };
    let pump: unsafe extern "system" fn(*mut u8, *const FD4TaskData) -> usize =
        unsafe { std::mem::transmute(pump) };
    let _ = unsafe { pump(ingame, task_data as *const FD4TaskData) };
}

pub(crate) fn native_autoload_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_NATIVE_AUTOLOAD").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-native-autoload.txt")
        .exists()
}

/// Recipe A: arm the game's OWN built-in title autoload with zero input.
///
/// The save-manager per-frame update `0x14067f5d0` performs an autoload when the
/// save slot (`GameMan+0xac0`) is set AND the force flag `0x143d856a0` is non-zero
/// — it primes the world/streaming subsystems through the game's own state
/// machine (which `force_play_game` bypassed). So we set the slot via the native
/// setter `0x67a810` and raise the force flag ONCE, then let the engine load.
/// The earlier crash from raising that flag came from leaving the slot at -1 (a
/// Finish teardown with no load armed); arming the slot first is the fix.
pub(crate) unsafe fn native_autoload_once(module_base: usize, slot: i32, tick: u64) {
    if tick < TITLE_NATIVE_JOB_MIN_TICK {
        return;
    }
    let game_man =
        unsafe { *((module_base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) as *const usize) };
    if game_man == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let load_in_progress =
        unsafe { *((game_man + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const u8) };
    if NATIVE_AUTOLOAD_ARMED.load(Ordering::SeqCst) {
        // Observe the load cascade after arming.
        if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
            let slot_now =
                unsafe { *((game_man + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
            let load14 =
                unsafe { *((game_man + FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET) as *const i32) };
            let latch = unsafe { *((module_base + SELECTBOT_LOAD_GATE_RVA) as *const u8) };
            let b72 = unsafe { *((game_man + GAME_MAN_ARM_FLAG_B72_OFFSET) as *const u8) };
            let csfeman = unsafe { *((module_base + CSFEMAN_SINGLETON_RVA) as *const usize) };
            append_autoload_debug(format_args!(
                "native_autoload: observe slot={slot_now} b80={load_in_progress} load14={load14} latch={latch} b72={b72} csfeman=0x{csfeman:x} tick={tick}"
            ));
        }
        return;
    }
    if load_in_progress != TITLE_NATIVE_JOB_TASK_DATA_ZERO {
        append_autoload_debug(format_args!(
            "native_autoload: load already in progress (b80={load_in_progress}) before arm; skipping tick={tick}"
        ));
        return;
    }
    // CORRECTED recipe (native-continue-and-slotn-recipe-2026): the latch
    // 0x143d856a0 must stay CLEAR; the arm flag is [GameMan+0xb72]=1. (The old
    // code set the latch to 1, which the disasm proves aborts the load.)
    let latch_before = unsafe { *((module_base + SELECTBOT_LOAD_GATE_RVA) as *const u8) };
    let set_save_slot: unsafe extern "system" fn(i32) =
        unsafe { std::mem::transmute(module_base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
    unsafe { set_save_slot(slot) };
    let slot_after = unsafe { *((game_man + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
    unsafe {
        *((game_man + GAME_MAN_ARM_FLAG_B72_OFFSET) as *mut u8) = TITLE_PROCEED_GATE_SET_VALUE;
    }
    NATIVE_AUTOLOAD_ARMED.store(true, Ordering::SeqCst);
    let csfeman = unsafe { *((module_base + CSFEMAN_SINGLETON_RVA) as *const usize) };
    append_autoload_debug(format_args!(
        "native_autoload: armed slot={slot_after} b72=1 latch_left={latch_before} b80={load_in_progress} csfeman=0x{csfeman:x} tick={tick}"
    ));
}

pub(crate) fn observe_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_OBSERVE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-observe.txt")
            .exists()
}

pub(crate) fn own_stepper_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_OWN_STEPPER").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-own-stepper.txt")
            .exists()
}

/// OBSERVE-ONLY NATIVE-LOAD gate (corrected-autoload-design-observe-not-force-native-load-2026).
/// OFF by default; enable via env `ER_EFFECTS_NATIVE_LOAD=1` OR a GAME_DIR file
/// `er-effects-native-load.txt`. Mirrors `own_stepper_enabled` (env OR file). When ON, the idx10
/// handler installs the patch (so OUR handler runs each frame) but does NOT force the title state
/// machine: it lets OWN_STEPPER_ORIG_IDX10 pass-through advance the native boot naturally (the user
/// drives past press-any-button + modals in this hybrid test), and ONCE the live TitleTopDialog
/// menu is rendered + settled, it fires the native Load-Game MenuMemberFuncJob node's run
/// 0x1409aaba0 exactly once -- testing whether that loads the real char in a NATURAL (non-forced)
/// menu. NO SetState(2/3), NO beginlogo-gate clear, NO registrar self-fire, NO direct_build /
/// cold_char_mount. De-risks design step 4.
pub(crate) fn native_load_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_NATIVE_LOAD").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-native-load.txt")
            .exists()
}

/// OBSERVE-ONLY NATIVE FULL-SAVE-READ gate (native-full-save-read-slot-resolve-chain-observe-recipe-2026).
/// OFF by default; enable via env `ER_EFFECTS_NATIVE_FULLREAD=1` OR a GAME_DIR file
/// `er-effects-native-fullread.txt`. Mirrors `native_load_enabled` (env OR file). When ON, the idx10
/// handler installs the patch (so OUR handler runs each frame) but does NOT force the title state
/// machine: it lets OWN_STEPPER_ORIG_IDX10 pass-through advance the native boot naturally (the user
/// drives past press-any-button + modals in this hybrid test), and ONCE the live TitleTopDialog menu
/// is rendered + settled, it runs the native full-save-read load chain directly at the live menu --
/// where the FD4 IO worker pool is LIVE so the submit drains (SUBMIT -> DRAIN_POLL -> DESER -> GUARD
/// -> CONFIRM). NO SetState forcing for boot, NO selector-step pump (probe-12 crash). The sole save
/// write (continue_confirm 0x140b0e180 -> SetState5) is HARD-gated behind the step-6 guard AND the
/// separate commit sub-gate `native_fullread_commit_enabled` (default = VERIFY-ONLY).
pub(crate) fn native_fullread_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_NATIVE_FULLREAD").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-native-fullread.txt")
        .exists()
}

/// COMMIT sub-gate for the native full-save-read chain (REQUIRED to actually fire continue_confirm
/// 0x140b0e180 -> SetState5, the SOLE save write). OFF by default; enable via env
/// `ER_EFFECTS_FULLREAD_COMMIT=1` OR a GAME_DIR file `er-effects-fullread-commit.txt`. Without it the
/// chain stops at the step-6 GUARD (deserialize + guard + log only): save-safe, NO continue_confirm,
/// NO SetState5. This lets a first test run VERIFY-ONLY (default) before any save write.
pub(crate) fn native_fullread_commit_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_FULLREAD_COMMIT").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-fullread-commit.txt")
        .exists()
}

/// OPT-IN gate for the MenuWindow-latch diagnostic hook (SceneObjProxy ctor 0x14074a700).
/// OFF by default: a clean run installs NO MinHook / NO detour for this. Enable only when
/// the latch is needed, via env `ER_EFFECTS_MENU_WINDOW_LATCH=1` OR a GAME_DIR file
/// `er-effects-menu-window-latch.txt`. Mirrors `own_stepper_enabled` (env OR file).
/// Rationale: this hook was previously installed UNCONDITIONALLY at process-attach and was
/// NOT present in the prior working cold-mount run; gating it lets us isolate hook-induced
/// mount perturbation (see bd probe11 caveat).
pub(crate) fn menu_window_latch_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_MENU_WINDOW_LATCH").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-menu-window-latch.txt")
        .exists()
}

/// OPT-IN gate for the c30-writer diagnostic hook (hot deserialize-internal 0x67bd70).
/// OFF by default: a clean run installs NO MinHook / NO detour for this. Enable only when
/// the diagnostic is needed, via env `ER_EFFECTS_C30_DIAG=1` OR a GAME_DIR file
/// `er-effects-c30-diag.txt`. Mirrors `own_stepper_enabled` (env OR file).
/// Rationale: a trampoline on the HOT 0x67bd70 deserialize path may itself perturb the
/// mount (b80 stuck / crash); gating it lets us run without it to isolate (bd probe11).
pub(crate) fn c30_writer_diag_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_C30_DIAG").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-c30-diag.txt")
            .exists()
}

/// PASSIVE own-stepper: do NOT force the menu (no SetState(2)/self-fire) and do NOT block input.
/// The user navigates to Load Game once (the input that surfaces the input-gated d180); the
/// capture hooks grab d180; then STAGE 2 drives mount->confirm->load. This both PROVES the load
/// (correct + faster than manual slot-select) and lets the iterator log the menu-structure change
/// so the pump-switch can be replayed zero-input later. File: er-effects-passive.txt.
pub(crate) fn own_stepper_passive_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_PASSIVE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-passive.txt")
            .exists()
}

/// DETERMINISTIC MENU INPUT PROBE (er-effects-input-probe.txt / ER_EFFECTS_INPUT_PROBE). After the
/// menu opens, inject one Down tap then (after an observation window) one Confirm tap, at frames WE
/// choose -- so we know exactly the frame to break on. Decisive question: does the Load-Game leaf
/// d180 tick its leaf Update on HIGHLIGHT alone (Down, no Confirm yet), or only at Confirm? Targeted
/// input used purely as a MEASUREMENT oracle (NOT the zero-input deliverable).
pub(crate) fn input_probe_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_INPUT_PROBE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-input-probe.txt")
            .exists()
}

/// SELF-DRIVEN GAMEPAD NAV INJECTION (er-effects-inject-nav.txt / ER_EFFECTS_INJECT_NAV). When on,
/// the input block stays engaged PAST menu-open (user input fully suppressed) and the XInput hook
/// fabricates a D-pad Down nav schedule at the gamepad poll source, cycling the title-menu cursor
/// so the input/focus-gated row populate fires and the row-push/csmenu-ctor hooks capture its
/// trigger -- uncontaminated by user input. Capture-only (Down nav, never Confirm).
pub(crate) fn inject_nav_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_INJECT_NAV").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-inject-nav.txt")
            .exists()
}

/// DISPROVEN/LEGACY menu-drive escape hatch -- deliberately OFF by default and HARD to trigger.
///
/// The own_stepper "title-confirm" Load drive (fire_titletop_load_entry + the d180-locate walk) was
/// built on a MISIDENTIFIED function: RTTI on the dearxan-deobfuscated image proved 0x14078e1c0 is
/// `CommandSelectDialog::Update` (an in-game dialog), NOT the title menu's confirm router, so its
/// offsets (cursor [+0xb0c], rows [+0x1290]) do NOT apply to the TitleTopDialog at owner+0xe0
/// (RTTI vt 0x142b26468). See bd rtti-correction-0x14078e1c0-is-commandselectdialog-not-title-
/// confirm-2026. We keep the code (it still has diagnostic value) but it must NEVER be the default
/// path: a fresh session running plain own_stepper must not take this wrong route. The trigger name
/// is intentionally obscure so it cannot be stumbled into -- enable ONLY to revisit the dead path.
pub(crate) fn legacy_menu_drive_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_LEGACY_DISPROVEN_MENU_DRIVE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-legacy-disproven-menu-drive.txt")
        .exists()
}

/// WORLD-RES STREAMING-DRIVER COLD-BUILD PROBE gate (env ER_EFFECTS_WORLDRES_COLDBUILD /
/// er-effects-worldres-coldbuild.txt). OFF by default. When on, own_stepper runs a ONE-SHOT,
/// SAVE-SAFE probe at the parked title that cold-builds the CSEmkResManImp streaming driver
/// (0x143d7c088) + registers the stream worker (0x144842d40) via the CSResStep tick getter
/// 0x140cd6c50 with a stub `this` -- NO SetState, NO world load, zero save-write risk. See bd
/// emk-resman-streaming-driver-coldbuild-stub-lever-2026.
pub(crate) fn worldres_coldbuild_probe_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_WORLDRES_COLDBUILD").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-worldres-coldbuild.txt")
        .exists()
}

/// SAVE-SAFE one-shot cold-build probe of the world-resource streaming driver. Validates the lever
/// emk-resman-streaming-driver-coldbuild-stub-lever-2026 live, WITHOUT SetState / world load.
/// The CSResStep tick getter 0x140cd6c50's body is context-free (builds the EMK resman cluster via
/// global RIP-relative stores + boot allocators; `this`/rsi is touched ONLY at prologue/tail). The
/// tail registers the stream worker when [this+0x48] >= 6. So a zeroed stub with [+0x48]=6 builds
/// the driver 0x143d7c088 + worker 0x144842d40, cold. Pure build -> read-back; no save write.
unsafe fn worldres_coldbuild_probe(base: usize) {
    const CSRES_GETTER_RVA: usize = 0x00cd6c50;
    const EMK_RESMAN_DRIVER_RVA: usize = 0x03d7c088;
    const STREAM_WORKER_RVA: usize = 0x04842d40;
    const STUB_LEN: usize = 0x80;
    const STUB_FILL: u8 = 0;
    const STUB_STATE_OFFSET: usize = 0x48;
    const STUB_STATE_VALUE: i32 = 6;
    const PROBE_DONE: usize = 1;
    static COLDBUILD_DONE: AtomicUsize = AtomicUsize::new(0);
    if COLDBUILD_DONE.swap(PROBE_DONE, Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let driver_before = unsafe { *((base + EMK_RESMAN_DRIVER_RVA) as *const usize) };
    let worker_before = unsafe { *((base + STREAM_WORKER_RVA) as *const usize) };
    // Persistent zeroed stub `this`: the getter only touches [+0x48] (state) / [+0x4c] / [+0x50].
    let stub: &'static mut [u8; STUB_LEN] = Box::leak(Box::new([STUB_FILL; STUB_LEN]));
    let stub_ptr = stub.as_mut_ptr() as usize;
    unsafe { *((stub_ptr + STUB_STATE_OFFSET) as *mut i32) = STUB_STATE_VALUE };
    append_autoload_debug(format_args!(
        "worldres-coldbuild: BEFORE driver[0x{:x}]=0x{driver_before:x} worker[0x{:x}]=0x{worker_before:x} -- calling CSResStep getter 0x{:x}(stub=0x{stub_ptr:x})",
        base + EMK_RESMAN_DRIVER_RVA,
        base + STREAM_WORKER_RVA,
        base + CSRES_GETTER_RVA
    ));
    let getter: unsafe extern "system" fn(usize) -> usize =
        unsafe { std::mem::transmute(base + CSRES_GETTER_RVA) };
    let ret = unsafe { getter(stub_ptr) };
    let driver_after = unsafe { *((base + EMK_RESMAN_DRIVER_RVA) as *const usize) };
    let worker_after = unsafe { *((base + STREAM_WORKER_RVA) as *const usize) };
    append_autoload_debug(format_args!(
        "worldres-coldbuild: AFTER driver=0x{driver_after:x} worker=0x{worker_after:x} ret=0x{ret:x} (both non-null = lever VALIDATED, NO SetState/NO save write)"
    ));
}

/// COLD CHAR-MOUNT experiment gate (env ER_EFFECTS_COLD_CHAR_MOUNT / er-effects-cold-char-mount.txt,
/// OFF by default). The DECISIVE save-data experiment (save-io-infra-present-cold-char-mount-is-the-
/// decisive-untested-experiment-2026): with the stream worker REGISTERED, can the b80 save-IO read
/// drain to resident so 0x67b290 mounts the real char -- zero-input, SAVE-SAFE (reads the save,
/// applies char to memory; NO SetState, NO save write).
pub(crate) fn cold_char_mount_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_COLD_CHAR_MOUNT").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-cold-char-mount.txt")
        .exists()
}

/// Direct ProfileLoadDialog build mode (er-effects-direct-build.txt / ER_EFFECTS_DIRECT_BUILD).
/// OFF by default: a plain own_stepper run stays the safe read-only scan; the native dialog build
/// (which leads to a guarded SetState(5) save-write via STAGE 2) fires only when deliberately
/// enabled, so the first native-build run is a deliberate, save-backed experiment.
pub(crate) fn direct_build_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_DIRECT_BUILD").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-direct-build.txt")
            .exists()
}

/// MODEL B: LIVE-dialog Load-Game fire (er-effects-live-dialog.txt / ER_EFFECTS_LIVE_DIALOG).
/// OFF by default. SIBLING to direct_build (the forge). Instead of FORGING a ProfileLoadDialog
/// (factory 0x14081ead0 with a synthetic capture + no live MenuWindow -> a NON-LIVE dialog the
/// native menu group never pumps -> wrong-map/crash), this locates the REAL Load-Game registry
/// node (CS::MenuMemberFuncJob<TitleTopDialog>, vtable 0x142b265d0, member-fn chains to factory
/// 0x14081ead0) and invokes its native run 0x1409aaba0(rcx=node) -- so the ProfileLoadDialog is
/// born LIVE & registered in menu-group 0x143d87350, which the native pump drives. STAGE2 then
/// fires load_activate (vt+0xa0) + the guarded continue_confirm -> SetState(5). The forge path
/// (direct_build) is untouched; this is a deliberate, separately-gated experiment.
pub(crate) fn live_dialog_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_LIVE_DIALOG").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-live-dialog.txt")
            .exists()
}

/// 2026-06-18 BREAKTHROUGH build: construct a CS::ProfileLoadDialog DIRECTLY at the open menu,
/// bypassing the input-gated router_this/d180-on-confirm layer (runtime-PROVEN never to build
/// headless -- loadgame-fingerprint-scan-confirms-router-this-not-built-headless-2026). The
/// ProfileLoadDialog ctor 0x1409a3d90 is COLD-VIABLE (it builds router_this + the slot rows
/// inline, no session/PlayerGameData/input-focus deps). We call dialog_factory 0x14081ead0,
/// which does op-new(0x1cd0) via allocator [0x143d87350] + ctx-build + ctor, passing:
///   rcx = &cap  (cap[0] = owner+0x138 = the ctor r8 = *(capture+8); factory reads *(rcx));
///   rdx = &ctx  (zeroed incoming-ctx -> empty cosmetic label).
/// Returns the dialog* in rax. FULLY read-only-validated before the native call (owner-obj vtable
/// 0x142ac7f20 + a populated row-vector [+0xa58..+0xa60]); fail-closed on any mismatch (NO call /
/// NO further action / NO write). On success: store OWN_STEPPER_DIALOG + advance to S2_ACTIVATE,
/// which own_stepper_stage2 drives (load_activate -> menu_deser mount -> guarded continue_confirm).
/// One-shot (OWN_STEPPER_DIRECT_BUILT). The ONLY save-write risk is STAGE 2's guarded SetState(5).
unsafe fn own_stepper_direct_build(owner: usize, base: usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const FACTORY_RVA: usize = 0x81ead0;
    const OWNER_OBJ_138: usize = 0x138;
    const OWNER_OBJ_VTABLE_RVA: usize = 0x2ac7f20;
    const ROWVEC_BEGIN_A58: usize = 0xa58;
    const ROWVEC_END_A60: usize = 0xa60;
    const ROWVEC_MAX_SPAN: usize = 0x10000;
    const PTR_ALIGN_MASK: usize = 0x7;
    // CONVERGENCE (2026-06-18, cold-b80-drain-is-PREVIEW-metadata-lane + direct-build): ACTIVATE the
    // slot byte BEFORE building the dialog, so the ctor's list-builder 0x140875590 (which checks
    // 0x140261cd0 = [ProfileSummary+8+slot]) APPENDS the slot -> the dialog's save-rows populate
    // (bound>0) -> load_activate has a row to read. This wires the ACTIVATE-byte breakthrough into
    // the direct-built dialog. Save-safe (in-memory byte; the dialog build is no-write).
    const PROFILE_SLOT_ACTIVATE_RVA: usize = 0x262250;
    let want_slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    let gdm = unsafe { safe_read_usize(base + SLOT_MANAGER_RVA) }.unwrap_or(NULL);
    let profile_summary = if gdm != NULL {
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if profile_summary != NULL && want_slot >= OWN_STEPPER_SLOT_ZERO {
        let activate: unsafe extern "system" fn(usize, i32) =
            unsafe { std::mem::transmute(base + PROFILE_SLOT_ACTIVATE_RVA) };
        unsafe { activate(profile_summary, want_slot) };
        // Record-state: load_activate 0x1409a4670's gate is INVERTED (load_activate-gate-inverted-
        // live-mount-is-nonbuild-path) -- the LIVE mount takes the NON-build branch (which calls
        // builder 0x140826510 @0x9a4985) when [rec+0x295]>=1 && accessor 0x140e362c0([rec+0x44])==2.
        // So set those so load_activate BUILDS the selector step (then we self-pump it -- the cold
        // standalone dialog is not ticked by the MENU group). rec = profile + 0x18 + slot*0x2a0.
        const RECORD_BASE_18: usize = 0x18;
        const RECORD_STRIDE_2A0: usize = 0x2a0;
        const RECORD_VALID_295: usize = 0x295;
        const RECORD_STATE_44: usize = 0x44;
        const RECORD_VALID_SET: u8 = 1;
        const RECORD_STATE_LOADABLE: i32 = 2;
        let rec = profile_summary + RECORD_BASE_18 + (want_slot as usize) * RECORD_STRIDE_2A0;
        unsafe { *((rec + RECORD_VALID_295) as *mut u8) = RECORD_VALID_SET };
        unsafe { *((rec + RECORD_STATE_44) as *mut i32) = RECORD_STATE_LOADABLE };
        append_autoload_debug(format_args!(
            "own_stepper: DIRECT-BUILD ACTIVATE 0x{:x}(profile=0x{profile_summary:x}, slot={want_slot}) + record [rec=0x{rec:x}+0x295]=1 [+0x44]=2 (rows populate + load_activate reaches the selector builder)",
            base + PROFILE_SLOT_ACTIVATE_RVA
        ));
    }
    let owner_obj = owner + OWNER_OBJ_138;
    // Read-only re-validation of r8 (owner_obj) before the native build: expected vtable + a
    // populated row-vector (begin < end, sane span). Fail-closed (latch set so we don't spin).
    let ovt = unsafe { safe_read_usize(owner_obj) }.unwrap_or(NULL);
    let begin = unsafe { safe_read_usize(owner_obj + ROWVEC_BEGIN_A58) }.unwrap_or(NULL);
    let end = unsafe { safe_read_usize(owner_obj + ROWVEC_END_A60) }.unwrap_or(NULL);
    let span = end.wrapping_sub(begin);
    let rows_ok = ovt == base + OWNER_OBJ_VTABLE_RVA
        && begin != NULL
        && (begin & PTR_ALIGN_MASK) == NULL
        && end > begin
        && span <= ROWVEC_MAX_SPAN;
    if !rows_ok {
        append_autoload_debug(format_args!(
            "own_stepper: DIRECT-BUILD ABORT (fail-closed, NO native call) owner_obj=0x{owner_obj:x} vt=0x{ovt:x}(want 0x{:x}) rowvec=[0x{begin:x}..0x{end:x}] span=0x{span:x}",
            base + OWNER_OBJ_VTABLE_RVA
        ));
        OWN_STEPPER_DIRECT_BUILT.store(OWN_STEPPER_DIRECT_BUILT_YES, Ordering::SeqCst);
        return;
    }
    // Stage the persistent buffers: cap[0] = owner_obj (factory reads *(rcx) for the ctor r8);
    // ctx stays zeroed (factory reads it to build an empty label).
    let cap_ptr = (&raw mut DIRECT_BUILD_CAP) as *mut usize;
    unsafe { *cap_ptr = owner_obj };
    let cap_addr = cap_ptr as usize;
    let ctx_addr = (&raw mut DIRECT_BUILD_CTX) as *mut usize as usize;
    let factory: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(base + FACTORY_RVA) };
    append_autoload_debug(format_args!(
        "own_stepper: DIRECT-BUILD calling factory 0x{:x}(rcx=&cap[=0x{owner_obj:x}], rdx=&ctx) owner_obj vt=0x{ovt:x} rowvec=[0x{begin:x}..0x{end:x}]",
        base + FACTORY_RVA
    ));
    let dialog = unsafe { factory(cap_addr, ctx_addr) };
    let dvt = if dialog != NULL {
        unsafe { safe_read_usize(dialog) }.unwrap_or(NULL)
    } else {
        NULL
    };
    OWN_STEPPER_DIRECT_BUILT.store(OWN_STEPPER_DIRECT_BUILT_YES, Ordering::SeqCst);
    if dialog != NULL && dvt == base + PROFILE_LOAD_DIALOG_VTABLE_RVA {
        OWN_STEPPER_DIALOG.store(dialog, Ordering::SeqCst);
        OWN_STEPPER_S2_WAITS.store(NULL, Ordering::SeqCst);
        OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_S2_ACTIVATE, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "own_stepper: DIRECT-BUILD SUCCESS dialog=0x{dialog:x} vt=0x{dvt:x} (ProfileLoadDialog) -- entering STAGE2 ACTIVATE (slot={})",
            OWN_STEPPER_SLOT.load(Ordering::SeqCst)
        ));
    } else {
        append_autoload_debug(format_args!(
            "own_stepper: DIRECT-BUILD returned dialog=0x{dialog:x} vt=0x{dvt:x} != ProfileLoadDialog 0x{:x} -- fail-closed, STAY (NO STAGE2, NO write)",
            base + PROFILE_LOAD_DIALOG_VTABLE_RVA
        ));
    }
}

/// Multi-frame cold char-mount drive (gated, SAVE-SAFE). Sequence (worker registered): build+register
/// the FD4 stream worker (0xb0a980 stub) so the scheduler ticks it and drains the save-IO read; set
/// the slot; PREVIEW 0x67b4e0 (b80=1 + starts the iodev read); poll 0x679180 each frame until
/// GameMan+0xb80==3 (the make-or-break -- the registered+ticked worker draining the read); then
/// deserialize 0x67b290 (mounts GameMan+0xc30=real map + applies the char to PlayerGameData).
/// NO SetState / NO save write. dump_load_correctness verifies the mounted char.
unsafe fn cold_char_mount_drive(base: usize, gm: usize, want_slot: i32, n: u64) {
    const PHASE_INIT: usize = 0;
    const PHASE_LANE: usize = 1;
    const PHASE_POLL: usize = 2;
    const PHASE_DESER: usize = 3;
    const PHASE_DONE: usize = 4;
    const STUB_FILL: u8 = 0;
    const POLL_ARG: u8 = 0;
    const B80_RESIDENT: i32 = 3;
    const B80_IDLE: i32 = 0;
    const MOUNT_POLL_MAX: usize = 1200;
    const LOG_INTERVAL: usize = 30;
    const WAIT_INC: usize = 1;
    static MOUNT_PHASE: AtomicUsize = AtomicUsize::new(PHASE_INIT);
    static MOUNT_WAITS: AtomicUsize = AtomicUsize::new(0);
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if gm == null {
        return;
    }
    let read_i32 = |off: usize| unsafe { *((gm + off) as *const i32) };
    let iodev_summary = || -> (usize, usize, usize) {
        let iodev = unsafe { *((base + IODEV_GLOBAL_RVA) as *const usize) };
        if iodev == null {
            (null, null, null)
        } else {
            unsafe {
                (
                    *((iodev + IODEV_INFLIGHT_10_OFFSET) as *const usize),
                    *((iodev + IODEV_REQHANDLE_18_OFFSET) as *const usize),
                    *((iodev + IODEV_REQHANDLE_20_OFFSET) as *const usize),
                )
            }
        }
    };
    let phase = MOUNT_PHASE.load(Ordering::SeqCst);
    if phase == PHASE_INIT {
        const SLOT_MIN: i32 = 0;
        if want_slot < SLOT_MIN {
            append_autoload_debug(format_args!(
                "cold-char-mount: needs an EXPLICIT slot (slot={want_slot}); set slot=N in er-effects-own-stepper.txt -- ABORT (no-write)"
            ));
            MOUNT_PHASE.store(PHASE_DONE, Ordering::SeqCst);
            return;
        }
        // (-1) Set the save-file path/name on the container so the device read returns slot N's REAL
        // .sl2 bytes. The native Continue handler runs this slot-mgr peek 0x140678a50 FIRST (reads
        // [GameDataMan+0x8] container, sync-reads the save path token 0x47054, copies the name to
        // container+0x94, sets GameMan+0xe70=1) before the load. The prior cold attempt SKIPPED it,
        // so the device read an EMPTY buffer (deserialize gave c30=0xffffffff + garbage char).
        // Save-safe (sets a path + reads metadata; NO save write).
        const SLOT_MGR_PEEK_RVA: usize = 0x678a50;
        let peek: unsafe extern "system" fn() =
            unsafe { std::mem::transmute(base + SLOT_MGR_PEEK_RVA) };
        unsafe { peek() };
        append_autoload_debug(format_args!(
            "cold-char-mount: slot-mgr peek 0x{:x}() -> set save-file path before mount (GameMan+0xe70 ready)",
            base + SLOT_MGR_PEEK_RVA
        ));
        // (0) REFRAME (2026-06-18, REFRAME-io-subsystem-present-cold-blocker-is-just-the-active-byte):
        // the FD4 IO subsystem (pool/task/iodev) is ALREADY present + CLEAN cold (snapshot-proven).
        // 0x67b200 fails cold ONLY because its slot-check 0x140261cd0 reads [ProfileSummary+8+slot]==0
        // (the session/ProfileSummary IS present). Set that byte directly via ACTIVATE 0x140262250
        // (byte[profile+slot+8]=1) so 0x67b200 passes its slot-check and submits the read onto the
        // present subsystem. Save-safe (sets an in-memory flag; the deserialize only READS the .sl2).
        const PROFILE_SLOT_ACTIVATE_RVA: usize = 0x262250;
        const SLOT_ACTIVE_BYTE_BASE: usize = 0x8;
        let game_data_man = unsafe { *((base + SLOT_MANAGER_RVA) as *const usize) };
        let profile_summary = if game_data_man != null {
            unsafe { *((game_data_man + SLOT_MANAGER_CONTAINER_OFFSET) as *const usize) }
        } else {
            null
        };
        if profile_summary != null {
            let activate: unsafe extern "system" fn(usize, i32) =
                unsafe { std::mem::transmute(base + PROFILE_SLOT_ACTIVATE_RVA) };
            unsafe { activate(profile_summary, want_slot) };
            let abyte = unsafe {
                *((profile_summary + SLOT_ACTIVE_BYTE_BASE + want_slot as usize) as *const u8)
            };
            append_autoload_debug(format_args!(
                "cold-char-mount: ACTIVATE 0x{:x}(profile=0x{profile_summary:x}, slot={want_slot}) -> [profile+8+{want_slot}]={abyte} (so 0x67b200 slot-check 0x140261cd0 passes)",
                base + PROFILE_SLOT_ACTIVATE_RVA
            ));
        } else {
            append_autoload_debug(format_args!(
                "cold-char-mount: ProfileSummary null (gdm=0x{game_data_man:x}) -- cannot ACTIVATE; 0x67b200 will fail its slot-check"
            ));
        }
        // (1) build + register the FD4 stream worker so the scheduler ticks it (drains the read).
        let stub: &'static mut [u8; SYNTHETIC_STEP_THIS_SIZE] =
            Box::leak(Box::new([STUB_FILL; SYNTHETIC_STEP_THIS_SIZE]));
        let stub_ptr = stub.as_mut_ptr() as usize;
        unsafe {
            *((stub_ptr + SYNTHETIC_STEP_STATE_OFFSET) as *mut i32) = WORLD_WORKER_BUILD_STATE
        };
        let worker_build: unsafe extern "system" fn(usize) -> usize =
            unsafe { std::mem::transmute(base + WORLD_WORKER_BUILD_RVA) };
        unsafe { worker_build(stub_ptr) };
        let worker = unsafe { *((base + WORLD_STREAM_WORKER_RVA) as *const usize) };
        // (2) set the slot, then PREVIEW (b80=1 + start the iodev read). The preview 0x67b4e0 is
        // REQUIRED: it pre-warms a RESIDENT iodev request that 0x67b200 reuses (the no-preview run
        // showed 0x67b200 alone sets b80=2 but the poll immediately resets it -- the read never goes
        // resident). CAVEAT (char-apply lane mismatch, cold-b80-char-apply-is-the-async-lane-mismatch):
        // the preview reads only slot METADATA (0x60000), so the resident request 0x67b290 reads is
        // metadata, NOT slot N's full 0x280000 save -> header invalid -> char not applied. The
        // full-save read/deserialize lane does not drain cold; this preview lane DOES (b80->3).
        let set_save_slot: unsafe extern "system" fn(i32) =
            unsafe { std::mem::transmute(base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
        unsafe { set_save_slot(want_slot) };
        let preview: unsafe extern "system" fn(i32) -> i32 =
            unsafe { std::mem::transmute(base + LOAD_INITIATOR_RVA) };
        let pret = unsafe { preview(want_slot) };
        let (io10, io18, io20) = iodev_summary();
        append_autoload_debug(format_args!(
            "cold-char-mount: INIT slot={want_slot} worker=0x{worker:x} preview_ret={pret} b80={} io10=0x{io10:x} io18=0x{io18:x} io20=0x{io20:x} -> LANE",
            read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET)
        ));
        MOUNT_PHASE.store(PHASE_LANE, Ordering::SeqCst);
        return;
    }
    if phase == PHASE_LANE {
        // While b80==1, tick the b80==1 lane driver 0x679510 (IO tick) to drive the PREVIEW read to
        // resident. It keeps b80=1 while in-progress and resets b80=0 once the read completes (the
        // registered+ticked worker is what makes that completion happen). When b80==0, the iodev
        // request is resident; fire LoadSaveData 0x67b200 to re-enter the b80=2 lane (populates io18).
        let lane: unsafe extern "system" fn() -> i32 =
            unsafe { std::mem::transmute(base + B80_LANE1_DRIVER_RVA) };
        let _ = unsafe { lane() };
        let b80 = read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
        let w = MOUNT_WAITS.fetch_add(WAIT_INC, Ordering::SeqCst);
        if w % LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS {
            let (io10, io18, io20) = iodev_summary();
            append_autoload_debug(format_args!(
                "cold-char-mount: LANE waits={w} b80={b80} io10=0x{io10:x} io18=0x{io18:x} io20=0x{io20:x}"
            ));
        }
        if b80 == B80_IDLE {
            let loadsave: unsafe extern "system" fn(i32) -> i32 =
                unsafe { std::mem::transmute(base + B80_LOAD_SAVE_DATA_INITIATOR_RVA) };
            let lret = unsafe { loadsave(want_slot) };
            let (io10, io18, io20) = iodev_summary();
            append_autoload_debug(format_args!(
                "cold-char-mount: preview read RESIDENT (b80->0 after {w} lane ticks) -> LoadSaveData 0x67b200 ret={lret} b80={} io10=0x{io10:x} io18=0x{io18:x} io20=0x{io20:x} -> POLL",
                read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET)
            ));
            MOUNT_WAITS.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
            MOUNT_PHASE.store(PHASE_POLL, Ordering::SeqCst);
        } else if w >= MOUNT_POLL_MAX {
            append_autoload_debug(format_args!(
                "cold-char-mount: PREVIEW read never resident after {w} lane ticks (b80 stuck at {b80}, io18 never populated) -- the registered worker is NOT draining the read. TIMEOUT (no-write)"
            ));
            MOUNT_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        }
        return;
    }
    if phase == PHASE_POLL {
        let poll: unsafe extern "system" fn(u8, u8) -> i32 =
            unsafe { std::mem::transmute(base + B80_POLL_RVA) };
        let _ = unsafe { poll(POLL_ARG, POLL_ARG) };
        let b80 = read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
        let w = MOUNT_WAITS.fetch_add(WAIT_INC, Ordering::SeqCst);
        if w % LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS {
            let (io10, io18, io20) = iodev_summary();
            append_autoload_debug(format_args!(
                "cold-char-mount: POLL waits={w} b80={b80} io10=0x{io10:x} io18=0x{io18:x} io20=0x{io20:x}"
            ));
        }
        if b80 == B80_RESIDENT {
            append_autoload_debug(format_args!(
                "cold-char-mount: b80 reached RESIDENT(3) after {w} polls -- the registered worker DRAINED the read -> DESERIALIZE"
            ));
            MOUNT_PHASE.store(PHASE_DESER, Ordering::SeqCst);
        } else if w >= MOUNT_POLL_MAX {
            append_autoload_debug(format_args!(
                "cold-char-mount: b80 STUCK at {b80} after {w} polls (worker registered but read never resident) -- TIMEOUT (no-write)"
            ));
            MOUNT_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        }
        return;
    }
    if phase == PHASE_DESER {
        // DIAGNOSTIC (char-apply debug, COLD-B80-WALL-BROKEN-...): before the deserialize, read the
        // suspects for why c30/char did not apply: [mgr+0xdf0] (deserialize-ready -- if set, 0x67b100
        // takes the fast-path and does NOT read into 0x67b290's buffer = lane mismatch / empty parse);
        // [mgr+0x18] (the async load job 0x140e6eb80 queued); [0x143d68078] (the c30-write gate that
        // gates 0x67bd70 inside 0x67b290).
        const DF0_OFFSET: usize = 0xdf0;
        const ASYNC_JOB_18_OFFSET: usize = 0x18;
        const C30_WRITE_GATE_RVA: usize = 0x3d68078;
        let df0 = unsafe { *((gm + DF0_OFFSET) as *const usize) };
        let job18 = unsafe { *((gm + ASYNC_JOB_18_OFFSET) as *const usize) };
        let c30_gate = unsafe { *((base + C30_WRITE_GATE_RVA) as *const usize) };
        let deser: unsafe extern "system" fn(i32) -> i32 =
            unsafe { std::mem::transmute(base + DESERIALIZE_SLOT_RVA) };
        let dret = unsafe { deser(want_slot) };
        let c30 = read_i32(GAME_MAN_SAVED_MAP_C30_OFFSET);
        let ac0 = read_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
        append_autoload_debug(format_args!(
            "cold-char-mount: DESERIALIZE slot={want_slot} ret={dret} c30=0x{c30:x} ac0={ac0} | pre-deser df0(mgr+0xdf0)=0x{df0:x} async_job(mgr+0x18)=0x{job18:x} c30_gate(0x143d68078)=0x{c30_gate:x} (df0!=0 -> 0x67b100 fast-path skips the read = empty parse). NO SetState/NO save write:"
        ));
        unsafe { dump_load_correctness(base, n) };
        // Publish the result so a STAGE2 caller that delegates here can observe completion + the
        // c30/char result (deser ret==1 == real char + c30 written from the save). Mirrors STAGE2's
        // DESER_FIRED/MOUNT_C30 latches so the STAGE2 mount-done logging/gate works identically.
        if dret == OWN_STEPPER_DESER_SUCCESS_RET {
            OWN_STEPPER_MOUNT_C30.store(c30, Ordering::SeqCst);
            OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_OK, Ordering::SeqCst);
        } else {
            OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_FAIL, Ordering::SeqCst);
        }
        MOUNT_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        return;
    }
}

/// The D-pad Down button mask to inject for poll-frame `n` (counted from the first poll after
/// menu-open), per the INJECT_NAV schedule: settle, then `INJECT_NAV_MAX_CYCLES` tap+gap cycles
/// with Down asserted for the first `INJECT_NAV_TAP_LEN` frames of each cycle. Returns 0 (no
/// input) during settle, gaps, and after the cycles complete.
pub(crate) fn inject_nav_buttons(n: usize) -> u16 {
    const NONE: u16 = 0;
    if n < INJECT_NAV_SETTLE_FRAMES {
        return NONE;
    }
    let m = n - INJECT_NAV_SETTLE_FRAMES;
    if m >= INJECT_NAV_MAX_CYCLES * INJECT_NAV_CYCLE {
        return NONE;
    }
    if m % INJECT_NAV_CYCLE < INJECT_NAV_TAP_LEN {
        XINPUT_GAMEPAD_DPAD_DOWN
    } else {
        NONE
    }
}

/// AUTO-CONFIRM observe mode (er-effects-auto-confirm.txt): drive the game's OWN natural title
/// flow with Confirm input-taps so we can finally observe the view PAST the modal. No SetState
/// forcing, no input block, no custom dismiss -- just the press the game polls for.
pub(crate) fn auto_confirm_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_AUTO_CONFIRM").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-auto-confirm.txt")
            .exists()
}

/// Tap Confirm (inputmgr+0x90+0x3d, edge) to walk the NATURAL flow:
/// press-any-button -> [confirm] -> connection-error modal -> [confirm] -> MAIN MENU.
/// STOPS once the modal has been SEEN and is now GONE, so we never confirm a main-menu item
/// (Continue = load most-recent = SetState(5) save-write risk). Pure observation of the post-modal
/// view. Uses the builder capture hook only to know when the modal is up.
pub(crate) fn auto_confirm_tap() {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let Ok(base) = game_module_base() else {
        return;
    };
    install_auto_accept_hook();
    let modal_now = CONNECTION_ERROR_DIALOG.load(Ordering::SeqCst) != null;
    if modal_now {
        AUTO_CONFIRM_MODAL_SEEN.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    let seen = AUTO_CONFIRM_MODAL_SEEN.load(Ordering::SeqCst) != null;
    if seen && !modal_now {
        // Past the modal -> stop tapping (do NOT confirm Continue on the main menu).
        return;
    }
    let inputmgr =
        unsafe { safe_read_usize(base + SELECTBOT_INPUT_MANAGER_GLOBAL_RVA) }.unwrap_or(null);
    if inputmgr == null {
        return;
    }
    let frame = AUTO_CONFIRM_FRAME.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
    if frame % AUTO_CONFIRM_CYCLE_FRAMES < AUTO_CONFIRM_SET_FRAMES {
        unsafe {
            *((inputmgr + INPUTMGR_BITMAP_90_OFFSET + MENU_EVENT_CONFIRM_3D) as *mut u8) |=
                MENU_EVENT_PRESSED_BIT;
        }
    }
    if frame % AUTO_CONFIRM_LOG_INTERVAL == null as u64 {
        append_autoload_debug(format_args!(
            "auto-confirm: tap frame={frame} modal_now={modal_now} seen={seen} inputmgr=0x{inputmgr:x}"
        ));
    }
}

/// Whether STAGE 1d should SELF-FIRE the TitleTopDialog open-menu registrar (0x1409b24e0).
/// DEFAULT OFF (file-gated): with the connection-error modal now handled (clean headless boot),
/// the NATURAL Continue/Load main menu builds from SetState(2)=BeginLogo, and force-firing the
/// TitleTopDialog registrar opens a COMPETING dialog that prevents the natural menu's Load-Game
/// item d180 from ticking through the capture hooks. Off => let the natural menu surface d180.
pub(crate) fn own_stepper_selffire_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_SELFFIRE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-selffire.txt")
            .exists()
}

/// Decode one x86-64 jmp-thunk hop. Matches either `add rcx,8 ; jmp rel32` (the MSVC
/// `std::function` `_Do_call` thunk family the FD4 menu-item action functor routes
/// through) or a bare `jmp rel32`, returning the absolute jump target. Returns `None`
/// when `addr` is not such a thunk (i.e. it is the real lambda body). Fault-tolerant:
/// reads via `safe_read_*`, never faults on unmapped code.
unsafe fn decode_thunk_hop(addr: usize) -> Option<usize> {
    // Low 5 bytes `48 83 C1 08 E9` = `add rcx,8 ; jmp` (little-endian in the qword).
    const ADDRCX8_JMP_PREFIX: usize = 0xE9_08C1_8348;
    const PREFIX_MASK_40: usize = 0xFF_FFFF_FFFF;
    const ADDRCX8_REL_OFF: usize = 5;
    const ADDRCX8_NEXT_OFF: i64 = 9;
    const JMP_OPCODE: usize = 0xE9;
    const JMP_OPCODE_MASK: usize = 0xFF;
    const JMP_REL_OFF: usize = 1;
    const JMP_NEXT_OFF: i64 = 5;
    let w0 = unsafe { safe_read_usize(addr) }?;
    if (w0 & PREFIX_MASK_40) == ADDRCX8_JMP_PREFIX {
        let rel = unsafe { safe_read_i32(addr + ADDRCX8_REL_OFF) }? as i64;
        Some((addr as i64 + ADDRCX8_NEXT_OFF + rel) as usize)
    } else if (w0 & JMP_OPCODE_MASK) == JMP_OPCODE {
        let rel = unsafe { safe_read_i32(addr + JMP_REL_OFF) }? as i64;
        Some((addr as i64 + JMP_NEXT_OFF + rel) as usize)
    } else {
        None
    }
}

/// STAGE 1 (strictly NO-WRITE): walk the title menu-item container at `owner+0x138` and
/// log each item, so we can (a) confirm the live FD4 SBO pointer-vector layout matches
/// the static RE (the captured recipe pointers were suspiciously low, so VERIFY before
/// any call) and (b) identify the Load-Game leaf by its `+0xa8` action functor's
/// `_Do_call` jmp-chain resolving to `dialog_factory 0x14081ead0` (Continue's instead
/// routes to confirm `0x140b0e180`, no dialog). All reads go through fault-tolerant
/// ReadProcessMemory -- NO writes, NO native calls, NO SetState -> save-safe at the
/// parked title. Tries both container interpretations (inline SBO vs base-pointer at
/// `+0x18`) and reports which yields valid menu-item vtables. Runs once.
unsafe fn diagnostic_menu_walk(
    owner: usize,
    module_base: usize,
    tag: &str,
    verbose: bool,
) -> Option<usize> {
    const ITEM_CONTAINER_138: usize = 0x138;
    const CONT_CURSOR_10: usize = 0x10;
    const CONT_ELEM0_18: usize = 0x18;
    const CONT_COUNT_60: usize = 0x60;
    const MENU_JOB_HOLDER_E0: usize = 0xe0;
    const ITEM_VTABLE_RVA: usize = 0x02aa97e8;
    const ITEM_FUNCTOR_A8: usize = 0xa8;
    const ITEM_CTX_10: usize = 0x10;
    const ITEM_DESC_58: usize = 0x58;
    const ITEM_RESULT_130: usize = 0x130;
    const DIALOG_FACTORY_RVA: usize = 0x0081ead0;
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const COUNT_SANITY_MIN: i32 = 1;
    const COUNT_SANITY_MAX: i32 = 32;
    const PTR_STRIDE: usize = core::mem::size_of::<usize>();
    const WALK_START: usize = 0;
    const WALK_STEP: usize = 1;
    const JMP_CHAIN_MAX_HOPS: usize = 4;
    const INTERP_INLINE: usize = 0;
    const INTERP_BASE_PTR: usize = 1;
    const INTERP_COUNT: usize = 2;

    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let item_vtable_abs = module_base + ITEM_VTABLE_RVA;
    let dialog_factory_abs = module_base + DIALOG_FACTORY_RVA;
    let container = owner + ITEM_CONTAINER_138;

    let state = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    let cursor =
        unsafe { safe_read_i32(container + CONT_CURSOR_10) }.unwrap_or(TITLE_STATE_OWNER_GONE);
    let count =
        unsafe { safe_read_i32(container + CONT_COUNT_60) }.unwrap_or(TITLE_STATE_OWNER_GONE);
    let holder = unsafe { safe_read_usize(owner + MENU_JOB_HOLDER_E0) }.unwrap_or(null);
    let elem0_raw = unsafe { safe_read_usize(container + CONT_ELEM0_18) }.unwrap_or(null);
    if verbose {
        append_autoload_debug(format_args!(
            "menu-walk[{tag}]: owner=0x{owner:x} state={state} container=0x{container:x} cursor={cursor} count={count} holder=0x{holder:x} elem0_raw=0x{elem0_raw:x} item_vt=0x{item_vtable_abs:x} dialog_factory=0x{dialog_factory_abs:x}"
        ));
    }
    if !(COUNT_SANITY_MIN..=COUNT_SANITY_MAX).contains(&count) {
        if verbose {
            append_autoload_debug(format_args!(
                "menu-walk[{tag}]: count={count} out of sane range -- container layout unverified (NO-WRITE)"
            ));
        }
        return None;
    }
    let count_usize = count as usize;

    let mut load_game_item: Option<usize> = None;
    let mut interp = INTERP_INLINE;
    while interp < INTERP_COUNT {
        let label = if interp == INTERP_INLINE {
            "inline"
        } else {
            "baseptr"
        };
        let base_ptr = if interp == INTERP_BASE_PTR {
            elem0_raw
        } else {
            null
        };
        if interp == INTERP_BASE_PTR && base_ptr == null {
            interp += WALK_STEP;
            continue;
        }
        let mut menu_items_found = WALK_START;
        let mut i = WALK_START;
        while i < count_usize {
            let item = if interp == INTERP_INLINE {
                unsafe { safe_read_usize(container + CONT_ELEM0_18 + i * PTR_STRIDE) }
            } else {
                unsafe { safe_read_usize(base_ptr + i * PTR_STRIDE) }
            }
            .unwrap_or(null);
            if item == null {
                i += WALK_STEP;
                continue;
            }
            let vtable = unsafe { safe_read_usize(item) }.unwrap_or(null);
            let is_menu_item = vtable == item_vtable_abs;
            if is_menu_item {
                menu_items_found += WALK_STEP;
            }
            let functor = unsafe { safe_read_usize(item + ITEM_FUNCTOR_A8) }.unwrap_or(null);
            let ctx = unsafe { safe_read_usize(item + ITEM_CTX_10) }.unwrap_or(null);
            let result = unsafe { safe_read_usize(item + ITEM_RESULT_130) }.unwrap_or(null);
            let desc_lo = unsafe { safe_read_usize(item + ITEM_DESC_58) }.unwrap_or(null);
            let desc_hi =
                unsafe { safe_read_usize(item + ITEM_DESC_58 + PTR_STRIDE) }.unwrap_or(null);
            // Follow the action functor's _Do_call jmp-chain; if it reaches the dialog
            // factory this is the Load-Game item.
            let mut is_load_game = false;
            let mut chain = String::new();
            if functor != null {
                let functor_vtable = unsafe { safe_read_usize(functor) }.unwrap_or(null);
                let mut docall = if functor_vtable != null {
                    unsafe { safe_read_usize(functor_vtable + DOCALL_VTABLE_SLOT_10) }
                        .unwrap_or(null)
                } else {
                    null
                };
                chain.push_str(&format!("docall=0x{docall:x}"));
                let mut hop = WALK_START;
                while hop < JMP_CHAIN_MAX_HOPS && docall != null {
                    if docall == dialog_factory_abs {
                        is_load_game = true;
                        break;
                    }
                    match unsafe { decode_thunk_hop(docall) } {
                        Some(next) => {
                            chain.push_str(&format!("->0x{next:x}"));
                            docall = next;
                        }
                        None => break,
                    }
                    hop += WALK_STEP;
                }
                if docall == dialog_factory_abs {
                    is_load_game = true;
                }
            }
            if is_menu_item && is_load_game && load_game_item.is_none() {
                load_game_item = Some(item);
            }
            if verbose {
                append_autoload_debug(format_args!(
                    "menu-walk[{tag}/{label}] i={i} item=0x{item:x} vt=0x{vtable:x} menu_item={is_menu_item} functor=0x{functor:x} ctx=0x{ctx:x} result=0x{result:x} desc=0x{desc_hi:016x}{desc_lo:016x} {chain} LOAD_GAME={is_load_game}"
                ));
            }
            i += WALK_STEP;
        }
        if verbose {
            append_autoload_debug(format_args!(
                "menu-walk[{tag}/{label}] summary: menu_items_found={menu_items_found}/{count_usize}"
            ));
        }
        interp += WALK_STEP;
    }
    load_game_item
}

/// Does `item`'s action functor at `+0xa8` resolve (through its `_Do_call` jmp-chain) to
/// the dialog factory 0x14081ead0? That uniquely marks the Load-Game leaf (Continue's
/// functor instead routes to the c30->SetState(5) confirm 0x140b0e180). Appends the decoded
/// chain to `chain` for logging. Fault-tolerant reads; never faults.
/// Does a std::function `functor` (the pointer ITSELF, not item+offset) resolve through its
/// `_Do_call` jmp-chain to the dialog factory 0x14081ead0? Used for the TitleTopDialog ROW entries
/// whose action functor lives at `[entry+0xf8]` (vs the MenuWindowJob `[item+0xa8]`). Fault-tolerant.
unsafe fn functor_ptr_hits_factory(functor: usize, module_base: usize, chain: &mut String) -> bool {
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const DIALOG_FACTORY_RVA: usize = 0x0081ead0;
    const JMP_CHAIN_MAX_HOPS: usize = 4;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog_factory_abs = module_base + DIALOG_FACTORY_RVA;
    if functor == null {
        return false;
    }
    let functor_vtable = unsafe { safe_read_usize(functor) }.unwrap_or(null);
    if functor_vtable == null {
        return false;
    }
    let mut docall =
        unsafe { safe_read_usize(functor_vtable + DOCALL_VTABLE_SLOT_10) }.unwrap_or(null);
    chain.push_str(&format!("functor=0x{functor:x} docall=0x{docall:x}"));
    let mut hop = HOP_START;
    while hop < JMP_CHAIN_MAX_HOPS && docall != null {
        if docall == dialog_factory_abs {
            return true;
        }
        match unsafe { decode_thunk_hop(docall) } {
            Some(next) => {
                chain.push_str(&format!("->0x{next:x}"));
                docall = next;
            }
            None => break,
        }
        hop += HOP_STEP;
    }
    docall == dialog_factory_abs
}

unsafe fn functor_chain_hits_factory(item: usize, module_base: usize, chain: &mut String) -> bool {
    const ITEM_FUNCTOR_A8: usize = 0xa8;
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const DIALOG_FACTORY_RVA: usize = 0x0081ead0;
    const JMP_CHAIN_MAX_HOPS: usize = 4;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog_factory_abs = module_base + DIALOG_FACTORY_RVA;
    let functor = unsafe { safe_read_usize(item + ITEM_FUNCTOR_A8) }.unwrap_or(null);
    if functor == null {
        return false;
    }
    let functor_vtable = unsafe { safe_read_usize(functor) }.unwrap_or(null);
    if functor_vtable == null {
        return false;
    }
    let mut docall =
        unsafe { safe_read_usize(functor_vtable + DOCALL_VTABLE_SLOT_10) }.unwrap_or(null);
    chain.push_str(&format!("functor=0x{functor:x} docall=0x{docall:x}"));
    let mut hop = HOP_START;
    while hop < JMP_CHAIN_MAX_HOPS && docall != null {
        if docall == dialog_factory_abs {
            return true;
        }
        match unsafe { decode_thunk_hop(docall) } {
            Some(next) => {
                chain.push_str(&format!("->0x{next:x}"));
                docall = next;
            }
            None => break,
        }
        hop += HOP_STEP;
    }
    docall == dialog_factory_abs
}

/// READ-ONLY enumerator of the TitleTopDialog's REALIZED selectable-entry vector -- the actual
/// Continue/Load-Game/New-Game rows the user navigates. These are NOT FD4 MenuWindowJobs in the
/// Sequence tree (which is why every job-tree walk + the 0x1407ad1c0 Update hook miss them); they
/// live in the dialog's own CSMenu sub-object (menu = dialog+0xa38) as a vector
/// `[menu+0x1290]..[menu+0x1298]` stride 0x210, cursor `[dialog+0xb0c]`, bound `[dialog+0xb08]`
/// (mainmenu-items-are-titletopdialog-widgets-not-fd4-jobs-2026). The confirm router 0x14078e1c0
/// fires an entry via `rax=[entry]; call [rax+0x10]` when `[entry+0xf8]!=0`. For each entry this
/// logs the vtable, its action method `[vtable+0x10]`, the `+0xf8` action-functor + its decoded
/// `_Do_call` jmp-chain, and whether either resolves to dialog_factory 0x14081ead0 (Load-Game) or
/// continue_confirm 0x140b0e180 (Continue). Pure vector math + reads (no game call) -> save-safe.
/// Returns (load_game_entry, continue_entry, cursor) for STAGE 2 to drive.
/// ZERO-INPUT title-menu Load fire (STATIC-RE validated, NO input injection). Replicates the
/// confirm router 0x14078e1c0's entry-action call directly (decoded: resolver 0x14078fbd0 returns
/// entry=[dialog+0x1290]+idx*0x210; if [entry+0xf8]!=0 -> rcx=[entry+0xf8]; call [[rcx]+0x10]).
/// Scans the realized TitleTopDialog row vector for the entry whose action functor [entry+0xf8]
/// chains to dialog_factory 0x14081ead0 (= Load Game; found empirically, NOT assumed by index),
/// sets cursor [dialog+0xb0c], and fires its _Do_call(rcx=action) -> builds the ProfileLoadDialog.
/// SELF-VALIDATING + FAIL-CLOSED: asserts the dialog vtable, that the row vector is populated, and
/// that a Load-Game entry was found, BEFORE firing -- so a non-realized/contaminated state is
/// caught, not absorbed. Build-only; the sole save-write is downstream (gated continue_confirm).
/// Returns true iff it fired.
unsafe fn fire_titletop_load_entry(dialog: usize, base: usize) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const VEC_BEGIN_1290: usize = 0x1290;
    const VEC_END_1298: usize = 0x1298;
    const ENTRY_STRIDE_210: usize = 0x210;
    const ENTRY_ACTION_F8: usize = 0xf8;
    const CURSOR_B0C: usize = 0xb0c;
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const MAX_ENTRIES: usize = 16;
    const IDX_START: usize = 0;
    const IDX_STEP: usize = 1;
    // VALIDATE 1: dialog identity (runtime vtable 0x142b26468).
    let vt = unsafe { safe_read_usize(dialog) }.unwrap_or(NULL);
    if vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "titletop-fire: dialog=0x{dialog:x} vt=0x{vt:x} != TitleTopDialog 0x{:x} -- ABORT (no fire)",
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return false;
    }
    // VALIDATE 2: row vector realized/populated.
    let begin = unsafe { safe_read_usize(dialog + VEC_BEGIN_1290) }.unwrap_or(NULL);
    let end = unsafe { safe_read_usize(dialog + VEC_END_1298) }.unwrap_or(NULL);
    if begin == NULL || end <= begin {
        append_autoload_debug(format_args!(
            "titletop-fire: row vector EMPTY/unrealized vec=[0x{begin:x}..0x{end:x}] -- ABORT (rows not populated)"
        ));
        return false;
    }
    let count = (end - begin) / ENTRY_STRIDE_210;
    // VALIDATE 3: find Load-Game by action->dialog_factory (NOT assumed index).
    let mut found: Option<(usize, usize)> = None;
    let mut idx = IDX_START;
    while idx < count && idx < MAX_ENTRIES {
        let entry = begin + idx * ENTRY_STRIDE_210;
        let action = unsafe { safe_read_usize(entry + ENTRY_ACTION_F8) }.unwrap_or(NULL);
        if action != NULL {
            let mut chain = String::new();
            if unsafe { functor_ptr_hits_factory(action, base, &mut chain) } {
                found = Some((idx, action));
                append_autoload_debug(format_args!(
                    "titletop-fire: LOAD-GAME entry idx={idx} entry=0x{entry:x} action=0x{action:x} {chain}"
                ));
                break;
            }
        }
        idx += IDX_STEP;
    }
    let (load_idx, action) = match found {
        Some(v) => v,
        None => {
            append_autoload_debug(format_args!(
                "titletop-fire: NO Load-Game entry (action->dialog_factory) in {count} rows -- ABORT"
            ));
            return false;
        }
    };
    // All validated -> set cursor + fire the action's _Do_call(rcx=action) == the router's confirm.
    unsafe {
        *((dialog + CURSOR_B0C) as *mut i32) = load_idx as i32;
    }
    let vtable = unsafe { safe_read_usize(action) }.unwrap_or(NULL);
    let do_call = if vtable != NULL {
        unsafe { safe_read_usize(vtable + DOCALL_VTABLE_SLOT_10) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if do_call == NULL {
        append_autoload_debug(format_args!(
            "titletop-fire: action=0x{action:x} has no _Do_call -- ABORT"
        ));
        return false;
    }
    let f: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(do_call) };
    unsafe { f(action) };
    append_autoload_debug(format_args!(
        "titletop-fire: FIRED Load-Game idx={load_idx} do_call=0x{do_call:x} -- ProfileLoadDialog should now build at owner+0xe0"
    ));
    true
}

/// Baseline snapshot of the TitleTopDialog dword window, captured before the one deterministic
/// Down so the post-Down pass can diff against it and name the cursor field precisely.
static CURSOR_PROBE_BASELINE: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

/// CURSOR-OFFSET PROBE (read-only, save-safe). `baseline=true`: snapshot the live TitleTopDialog
/// (owner+0xe0) dword window (cursor=0=Continue). `baseline=false` (after exactly one deterministic
/// Down, cursor=1=Load Game): re-read and log every offset whose value CHANGED, flagging the
/// 0->1 transition = the cursor field. Also logs the unverified static candidate [dialog+0xb0c] to
/// confirm/refute it. Pure reads via safe_read_usize -> never AVs.
unsafe fn cursor_offset_probe(owner: usize, base: usize, baseline: bool) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const DIALOG_E0: usize = 0xe0;
    const DWORD_LO_MASK: usize = 0xffffffff;
    const DWORD_BYTES: usize = 4;
    const SCAN_START: usize = 0;
    const SCAN_STEP: usize = 1;
    const CURSOR_FROM: u32 = 0;
    const CURSOR_TO: u32 = 1;
    let tag = if baseline { "baseline" } else { "postdown" };
    let dialog = unsafe { safe_read_usize(owner + DIALOG_E0) }.unwrap_or(NULL);
    if dialog == NULL {
        return;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(NULL);
    let cand_b0c = unsafe { safe_read_usize(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }
        .map(|v| (v & DWORD_LO_MASK) as u32)
        .unwrap_or(u32::MAX);
    append_autoload_debug(format_args!(
        "cursor-probe[{tag}]: dialog=0x{dialog:x} vt=0x{dialog_vt:x}(want base+0x{:x}) candidate[+0xb0c]={cand_b0c}",
        TITLE_TOP_DIALOG_VTABLE_RVA
    ));
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return;
    }
    let read_dword = |off: usize| -> u32 {
        unsafe { safe_read_usize(dialog + off) }
            .map(|w| (w & DWORD_LO_MASK) as u32)
            .unwrap_or(u32::MAX)
    };
    if baseline {
        let mut snap = Vec::with_capacity(CURSOR_PROBE_SCAN_DWORDS);
        let mut i = SCAN_START;
        while i < CURSOR_PROBE_SCAN_DWORDS {
            snap.push(read_dword(i * DWORD_BYTES));
            i += SCAN_STEP;
        }
        if let Ok(mut b) = CURSOR_PROBE_BASELINE.lock() {
            *b = snap;
        }
        return;
    }
    let baseline_snap = match CURSOR_PROBE_BASELINE.lock() {
        Ok(b) if b.len() == CURSOR_PROBE_SCAN_DWORDS => b.clone(),
        _ => {
            append_autoload_debug(format_args!(
                "cursor-probe[postdown]: no baseline captured -- skip diff"
            ));
            return;
        }
    };
    let mut logged = SCAN_START;
    let mut i = SCAN_START;
    while i < CURSOR_PROBE_SCAN_DWORDS && logged < CURSOR_PROBE_LOG_CAP {
        let off = i * DWORD_BYTES;
        let old = baseline_snap[i];
        let new = read_dword(off);
        if old != new && new < CURSOR_PROBE_SMALL_MAX {
            let is_cursor = old == CURSOR_FROM && new == CURSOR_TO;
            append_autoload_debug(format_args!(
                "cursor-probe[postdown] CHANGED off=0x{off:x} {old}->{new}{}",
                if is_cursor { "  <== CURSOR (0->1)" } else { "" }
            ));
            logged += SCAN_STEP;
        }
        i += SCAN_STEP;
    }
    append_autoload_debug(format_args!(
        "cursor-probe[postdown]: diff complete ({logged} changed small dwords)"
    ));
}

unsafe fn dump_titletop_menu_entries(
    owner: usize,
    base: usize,
) -> (Option<usize>, Option<usize>, i32) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const DIALOG_E0: usize = 0xe0;
    const MENU_SUBOBJ_A38: usize = 0xa38;
    const ENTRY_VEC_BEGIN_1290: usize = 0x1290;
    const ENTRY_VEC_END_1298: usize = 0x1298;
    const ENTRY_STRIDE_210: usize = 0x210;
    const ENTRY_ACTION_VT_SLOT_10: usize = 0x10;
    const ENTRY_FUNCTOR_F8: usize = 0xf8;
    const ENTRY_RESULT_130: usize = 0x130;
    const DIALOG_FACTORY_RVA: usize = 0x0081ead0;
    const MAX_ENTRIES: usize = 16;
    const IDX_START: usize = 0;
    const IDX_STEP: usize = 1;
    const JMP_HOPS: usize = 5;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    const BAD_I32: i32 = -1;
    let ri32 = |addr: usize| -> i32 {
        unsafe { safe_read_usize(addr) }
            .map(|v| v as u32 as i32)
            .unwrap_or(BAD_I32)
    };
    let dialog = unsafe { safe_read_usize(owner + DIALOG_E0) }.unwrap_or(NULL);
    let dialog_vt = if dialog != NULL {
        unsafe { safe_read_usize(dialog) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let cursor = if dialog != NULL {
        ri32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET)
    } else {
        BAD_I32
    };
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "titletop-entries: owner+0xe0=0x{dialog:x} vt=0x{dialog_vt:x} (expect 0x{:x}) -- not the TitleTopDialog, skip",
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return (None, None, cursor);
    }
    // The selectable-row vector does NOT live on the TitleTopDialog -- [dialog+0x1290] is GFx
    // markup text (runtime read = ASCII). The rows live on a SEPARATE title CSMenu controller
    // ("router_this", runtime vtable base+0x2afa070, ctor 0x1409060d8): the select router
    // 0x14078e1c0 calls the resolver 0x14078fbd0 with rcx=router_this, reading [router_this+0x1290]
    // /[+0x1298] (stride 0x210); cursor [+0xb0c], bound [+0xb08]. Locate router_this by scanning
    // the TitleTopDialog's fields for a pointer to an object whose [0] == that vtable. Pure reads
    // (safe_read_usize tolerates bad derefs) -> save-safe.
    const ROUTER_VTABLE_RVA: usize = 0x02afa070;
    const ROUTER_SCAN_QWORDS: usize = 0x400;
    const PTR_ALIGN_MASK: usize = 0x7;
    const QW_START: usize = 0;
    const QW_STEP: usize = 1;
    const PTR_SZ: usize = 8;
    let router_vt = base + ROUTER_VTABLE_RVA;
    // Prefer the ctor-latched router_this (cap_csmenu_ctor_hook captures it at construction --
    // it is NOT field-linked from the TitleTopDialog). Fall back to a dialog-field scan.
    let mut router_this = MENU_ROUTER_THIS.load(Ordering::SeqCst);
    if router_this == NULL {
        let mut q = QW_START;
        while q < ROUTER_SCAN_QWORDS {
            let p = unsafe { safe_read_usize(dialog + q * PTR_SZ) }.unwrap_or(NULL);
            if p != NULL
                && (p & PTR_ALIGN_MASK) == QW_START
                && unsafe { safe_read_usize(p) }.unwrap_or(NULL) == router_vt
            {
                router_this = p;
                break;
            }
            q += QW_STEP;
        }
    }
    if router_this == NULL {
        append_autoload_debug(format_args!(
            "titletop-entries: dialog=0x{dialog:x} -- router_this (CSMenu vt=0x{router_vt:x}) NOT found in dialog fields; cursor={cursor} (rows unreachable via this path)"
        ));
        return (None, None, cursor);
    }
    let menu = router_this + MENU_SUBOBJ_A38;
    let cursor = ri32(router_this + DIALOG_SLOT_CURSOR_B0C_OFFSET);
    let vec_begin = unsafe { safe_read_usize(router_this + ENTRY_VEC_BEGIN_1290) }.unwrap_or(NULL);
    let vec_end = unsafe { safe_read_usize(router_this + ENTRY_VEC_END_1298) }.unwrap_or(NULL);
    let bound = ri32(router_this + DIALOG_SLOT_BOUND_B08_OFFSET);
    if vec_begin == NULL || vec_end <= vec_begin {
        append_autoload_debug(format_args!(
            "titletop-entries: router_this=0x{router_this:x} vec=[0x{vec_begin:x}..0x{vec_end:x}] EMPTY -- rows NOT populated headless; cursor={cursor} bound={bound}"
        ));
        return (None, None, cursor);
    }
    let count = (vec_end - vec_begin) / ENTRY_STRIDE_210;
    append_autoload_debug(format_args!(
        "titletop-entries: dialog=0x{dialog:x} menu=0x{menu:x} count={count} cursor={cursor} bound={bound} vec=[0x{vec_begin:x}..0x{vec_end:x}]"
    ));
    let factory_abs = base + DIALOG_FACTORY_RVA;
    let confirm_abs = base + CONTINUE_CONFIRM_RVA;
    // Decode a function/thunk address forward through up to JMP_HOPS jmp-thunks, reporting if it
    // reaches the Load-Game factory or the Continue confirm. (Full-function actions that only CALL
    // the factory internally won't chain-resolve -- the raw action address is logged regardless.)
    let classify = |start: usize, chain: &mut String| -> (bool, bool) {
        let mut tgt = start;
        let mut hop = HOP_START;
        while hop < JMP_HOPS && tgt != NULL {
            if tgt == factory_abs {
                return (true, false);
            }
            if tgt == confirm_abs {
                return (false, true);
            }
            match unsafe { decode_thunk_hop(tgt) } {
                Some(next) => {
                    chain.push_str(&format!("->0x{next:x}"));
                    tgt = next;
                }
                None => break,
            }
            hop += HOP_STEP;
        }
        (tgt == factory_abs, tgt == confirm_abs)
    };
    let mut load_game: Option<usize> = None;
    let mut continue_entry: Option<usize> = None;
    let mut idx = IDX_START;
    while idx < count && idx < MAX_ENTRIES {
        let entry = vec_begin + idx * ENTRY_STRIDE_210;
        let evt = unsafe { safe_read_usize(entry) }.unwrap_or(NULL);
        let action = if evt != NULL {
            unsafe { safe_read_usize(evt + ENTRY_ACTION_VT_SLOT_10) }.unwrap_or(NULL)
        } else {
            NULL
        };
        let functor = unsafe { safe_read_usize(entry + ENTRY_FUNCTOR_F8) }.unwrap_or(NULL);
        let result = unsafe { safe_read_usize(entry + ENTRY_RESULT_130) }.unwrap_or(NULL);
        // Classify the vtable action method, and (if present) the +0xf8 std::function's _Do_call.
        let mut action_chain = String::new();
        let (a_load, a_cont) = classify(action, &mut action_chain);
        let mut f_chain = String::new();
        let f_docall = if functor != NULL {
            let fvt = unsafe { safe_read_usize(functor) }.unwrap_or(NULL);
            if fvt != NULL {
                unsafe { safe_read_usize(fvt + ENTRY_ACTION_VT_SLOT_10) }.unwrap_or(NULL)
            } else {
                NULL
            }
        } else {
            NULL
        };
        let (f_load, f_cont) = if f_docall != NULL {
            classify(f_docall, &mut f_chain)
        } else {
            (false, false)
        };
        let is_load = a_load || f_load;
        let is_cont = a_cont || f_cont;
        append_autoload_debug(format_args!(
            "titletop-entry #{idx} entry=0x{entry:x} vt=0x{evt:x} action=0x{action:x}{action_chain} f8=0x{functor:x} f8_docall=0x{f_docall:x}{f_chain} result=0x{result:x} LOAD_GAME={is_load} CONTINUE={is_cont}"
        ));
        if is_load && load_game.is_none() {
            load_game = Some(entry);
        }
        if is_cont && continue_entry.is_none() {
            continue_entry = Some(entry);
        }
        idx += IDX_STEP;
    }
    (load_game, continue_entry, cursor)
}

/// SAVE-SAFE READ-ONLY structural scan of the OPEN TitleTopDialog for the Load-Game entry,
/// using the two RTTI fingerprints from the 2026-06-18 reconciliation
/// (bd title-load-is-profileloaddialog-NOT-movemapliststep-b78-dead-2026):
///   * d180 std::function `_Func_impl` vtable = `base+0x2ac3ea8` (its `_Do_call` 0x140820c60
///     `add rcx,8; jmp dialog_factory 0x14081ead0`), held at a MenuWindowJob's `+0xa8`;
///   * `CS::MenuMemberFuncJob<TitleTopDialog>` vtable = `base+0x2b265d0` (run 0x1409aaba0),
///     the entries the registrar 0x1409b24e0 registers into `[dialog+0xa48]`.
/// The prior d180-locate walked the FD4 MenuJobSequence tree (owner+0xe0/0x130/0x138) and never
/// surfaced the item, because the title rows are TitleTopDialog REGISTRY entries, not Sequence
/// children, AND `[dialog+0xa48]` is an opaque FD4 delegate registry (insert 0x1407a6c00, vcall
/// node-build -- not statically walkable). This instead does a BOUNDED flat scan of the dialog
/// object's own fields for any pointer to either fingerprint (and any object whose `+0xa8` holds
/// the d180 functor = a MenuWindowJob d180). Pure ReadProcessMemory (safe_read_usize tolerates bad
/// derefs) -> NO writes, NO native calls -> save-safe. RECON-ONLY: logs every hit and RETURNS
/// `(member_node, window_item)`: `member_node` = the first Load-Game CS::MenuMemberFuncJob node
/// (vt MEMBERFUNCJOB_VTABLE_RVA, member_fn reaches the dialog factory) -- this is the node the
/// native run 0x1409aaba0 is fired against; `window_item` = the first d180 MenuWindowJob item
/// (whose +0xa8 holds the d180 functor). It does NOT latch/advance (the caller decides) so a first
/// run stays NO-WRITE at the menu. (Extended 2026-06-18 to also return the MenuMemberFuncJob node
/// so native_load_enabled() can fire its run; previously it returned only the window item.)
unsafe fn scan_dialog_for_loadgame(owner: usize, base: usize) -> (Option<usize>, Option<usize>) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const DIALOG_E0: usize = 0xe0;
    const ENTRY_REGISTRY_A48: usize = 0xa48;
    const ENTRY_SOURCE_A38: usize = 0xa38;
    // d180 std::function _Func_impl vtable (user-capture-confirmed); MenuMemberFuncJob vtable.
    const FUNCTOR_VTABLE_RVA: usize = 0x02ac3ea8;
    const MEMBERFUNCJOB_VTABLE_RVA: usize = 0x02b265d0;
    const FACTORY_RVA: usize = 0x0081ead0;
    const ITEM_FUNCTOR_A8: usize = 0xa8;
    const MEMBER_FN_18: usize = 0x18;
    const MEMBER_DIALOG_10: usize = 0x10;
    const MEMBER_ADJ_20: usize = 0x20;
    const SCAN_QWORDS: usize = 0x500;
    const PTR_SZ: usize = core::mem::size_of::<usize>();
    const PTR_ALIGN_MASK: usize = 0x7;
    const HEAP_LO: usize = 0x10000;
    const QW_START: usize = 0;
    const QW_STEP: usize = 1;
    const JMP_HOPS: usize = 6;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    const HIT_CAP: usize = 24;
    const HIT_START: usize = 0;
    const HIT_STEP: usize = 1;

    let dialog = unsafe { safe_read_usize(owner + DIALOG_E0) }.unwrap_or(NULL);
    if dialog == NULL {
        return (None, None);
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(NULL);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "loadgame-scan: owner+0xe0=0x{dialog:x} vt=0x{dialog_vt:x} != TitleTopDialog 0x{:x} -- skip",
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return (None, None);
    }
    let functor_vt = base + FUNCTOR_VTABLE_RVA;
    let memberjob_vt = base + MEMBERFUNCJOB_VTABLE_RVA;
    let factory_abs = base + FACTORY_RVA;
    // Resolve a (member-)fn forward through up to JMP_HOPS jmp-thunks; true if it reaches the
    // Load-Game dialog_factory. (A full member fn that only CALLs the factory internally won't
    // chain-resolve; the raw fn VA is logged regardless for offline disasm.)
    let reaches_factory = |start: usize| -> bool {
        let mut tgt = start;
        let mut hop = HOP_START;
        while hop < JMP_HOPS && tgt != NULL {
            if tgt == factory_abs {
                return true;
            }
            match unsafe { decode_thunk_hop(tgt) } {
                Some(next) => tgt = next,
                None => break,
            }
            hop += HOP_STEP;
        }
        tgt == factory_abs
    };
    let registry = unsafe { safe_read_usize(dialog + ENTRY_REGISTRY_A48) }.unwrap_or(NULL);
    let source = unsafe { safe_read_usize(dialog + ENTRY_SOURCE_A38) }.unwrap_or(NULL);
    append_autoload_debug(format_args!(
        "loadgame-scan: dialog=0x{dialog:x} registry(0xa48)=0x{registry:x} source(0xa38)=0x{source:x} functor_vt=0x{functor_vt:x} memberjob_vt=0x{memberjob_vt:x} -- scanning {SCAN_QWORDS} qwords"
    ));
    // DIRECT-BUILD r8 (ctor owner-obj) candidate validation (2026-06-18 breakthrough: the
    // ProfileLoadDialog ctor 0x1409a3d90 is COLD-VIABLE -- it builds router_this + slot rows
    // inline, no session/PGD/input-focus deps). dialog_factory 0x14081ead0 passes the ctor
    // r8 = *(capture+8); the gold capture showed that = owner+0x138, and the ctor reads the
    // profile ROW-VECTOR COUNT at [r8+0xa60]. Validate READ-ONLY which candidate has a plausible
    // vtable [+0] + a small row count [+0xa60] BEFORE any native build call (look before acting).
    const OWNER_MENU_OBJ_138: usize = 0x138;
    const CTOR_ROW_COUNT_A60: usize = 0xa60;
    const CTOR_ROW_VEC_BEGIN_A58: usize = 0xa58;
    const R8_CAND_N: usize = 2;
    let cand_a = owner + OWNER_MENU_OBJ_138;
    let cand_b = unsafe { safe_read_usize(cand_a) }.unwrap_or(NULL);
    let cands: [(&str, usize); R8_CAND_N] = [("owner+0x138", cand_a), ("*(owner+0x138)", cand_b)];
    for (tag, c) in cands.iter() {
        if *c == NULL {
            continue;
        }
        let cvt = unsafe { safe_read_usize(*c) }.unwrap_or(NULL);
        let cnt = unsafe { safe_read_usize(*c + CTOR_ROW_COUNT_A60) }.unwrap_or(NULL);
        let vbeg = unsafe { safe_read_usize(*c + CTOR_ROW_VEC_BEGIN_A58) }.unwrap_or(NULL);
        append_autoload_debug(format_args!(
            "loadgame-scan: r8-cand[{tag}]=0x{c:x} vt=0x{cvt:x} rowvec_begin[+0xa58]=0x{vbeg:x} rowcount[+0xa60]=0x{cnt:x}"
        ));
    }
    let mut found_item: Option<usize> = None;
    let mut found_member_node: Option<usize> = None;
    let mut hits = HIT_START;
    let mut q = QW_START;
    while q < SCAN_QWORDS {
        let off = q * PTR_SZ;
        let p = unsafe { safe_read_usize(dialog + off) }.unwrap_or(NULL);
        if p != NULL && (p & PTR_ALIGN_MASK) == QW_START && p >= HEAP_LO {
            let vt = unsafe { safe_read_usize(p) }.unwrap_or(NULL);
            if vt == memberjob_vt {
                // (a) a MenuMemberFuncJob registry entry node.
                let mfn = unsafe { safe_read_usize(p + MEMBER_FN_18) }.unwrap_or(NULL);
                let mdlg = unsafe { safe_read_usize(p + MEMBER_DIALOG_10) }.unwrap_or(NULL);
                let madj = unsafe { safe_read_usize(p + MEMBER_ADJ_20) }.unwrap_or(NULL);
                let rf = reaches_factory(mfn);
                if hits < HIT_CAP {
                    append_autoload_debug(format_args!(
                        "loadgame-scan: dialog+0x{off:x} MenuMemberFuncJob node=0x{p:x} member_fn=0x{mfn:x} reaches_factory={rf} back=0x{mdlg:x} adj=0x{madj:x}"
                    ));
                }
                // The Load-Game run target: a MenuMemberFuncJob whose member_fn chains to the
                // dialog factory. Latch the FIRST such node (run 0x1409aaba0 fires against it).
                if rf && found_member_node.is_none() {
                    found_member_node = Some(p);
                }
                hits += HIT_STEP;
            } else if vt == functor_vt {
                // (b) the d180 functor object itself.
                if hits < HIT_CAP {
                    append_autoload_debug(format_args!(
                        "loadgame-scan: dialog+0x{off:x} -> d180 FUNCTOR object=0x{p:x} (vt 0x2ac3ea8)"
                    ));
                }
                hits += HIT_STEP;
            } else {
                // (c) a MenuWindowJob whose +0xa8 holds the d180 functor = the Load-Game item.
                let fa8 = unsafe { safe_read_usize(p + ITEM_FUNCTOR_A8) }.unwrap_or(NULL);
                if fa8 != NULL && (fa8 & PTR_ALIGN_MASK) == QW_START && fa8 >= HEAP_LO {
                    let fvt = unsafe { safe_read_usize(fa8) }.unwrap_or(NULL);
                    if fvt == functor_vt {
                        append_autoload_debug(format_args!(
                            "loadgame-scan: dialog+0x{off:x} -> d180 MenuWindowJob item=0x{p:x} item_vt=0x{vt:x} functor=0x{fa8:x} -- LOAD-GAME candidate"
                        ));
                        if found_item.is_none() {
                            found_item = Some(p);
                        }
                        hits += HIT_STEP;
                    }
                }
            }
        }
        q += QW_STEP;
    }
    append_autoload_debug(format_args!(
        "loadgame-scan: done hits={hits} found_member_node=0x{:x} found_item=0x{:x}",
        found_member_node.unwrap_or(NULL),
        found_item.unwrap_or(NULL)
    ));
    (found_member_node, found_item)
}

/// MODEL B (FACTORY-HOOK LATCH RECIPE 2026-06-18, bd
/// live-dialog-menuwindow-latch-via-factory-hook-0x14081e5e0-2026): READ-ONLY deterministic
/// acquisition of the two LIVE args the Load-Game dialog factory 0x14081ead0 needs -- the live
/// TitleTopDialog* (the factory rcx = its [+0xa38] TitleFlowContext capture) and the live host
/// MenuWindow* (the factory rdx). The MenuWindow is NOT persistently readable at the parked title
/// (probe-5 proved [td+0xa38] is a CS::TitleFlowContext, NOT a SceneObjProxy, and there is no
/// persistent SceneObjProxy to read the +0x20 back-ref from). Instead the host MenuWindow is
/// LATCHED at boot from rdx of the SceneObjProxy ctor 0x14074a700
/// (`scene_obj_proxy_ctor_hook` -> LATCHED_MENU_WINDOW; probe-6: the OLD TitleTopDialog-factory rdx
/// was a std::function delegate, NOT the MenuWindow).
///
/// CONVERGED recipe (all pure safe_read_usize / atomic load -> NO writes, NO native calls, never
/// AVs -> save-safe; fail-closed at every step, every step logged via append_autoload_debug):
///   1. td = *(owner+0xe0); require *(td) == base+TITLE_TOP_DIALOG_VTABLE_RVA (else fail-closed).
///   2. SELF-DIAGNOSTIC: read + LOG the TitleFlowContext capture *(td+0xa38) + its vtable (context
///      only; it is the factory rcx, never gates acquisition).
///   3. menu_window = LATCHED_MENU_WINDOW (SeqCst); fail-closed if 0 (factory not yet hit) or not a
///      canonical heap pointer. Read mwvt = *(menu_window); LOG menu_window + mwvt; if mwvt is
///      neither MenuWindow nor MenuWindowProxy LOG loudly but STILL return it (probe visibility).
///   4. Return (td, menu_window).
unsafe fn locate_live_loadgame_node(owner: usize, base: usize) -> Option<(usize, usize)> {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    const PTR_ALIGN_MASK: usize = 0x7;

    let title_vt = base + TITLE_TOP_DIALOG_VTABLE_RVA;
    let scene_proxy_vt = base + SCENE_OBJ_PROXY_VTABLE_RVA;
    let menu_vt = base + MENU_WINDOW_VTABLE_RVA;
    let menu_proxy_vt = base + MENU_WINDOW_PROXY_VTABLE_RVA;

    // (1) TitleTopDialog: owner+0xe0, vtable-gated (probe-2/3 runtime-confirmed).
    let td = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(NULL);
    let tdvt = if td != NULL {
        unsafe { safe_read_usize(td) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if tdvt != title_vt {
        append_autoload_debug(format_args!(
            "live-dialog: owner+0x{:x}=0x{td:x} vt=0x{tdvt:x} != TitleTopDialog 0x{title_vt:x} -- title not up, fail-closed",
            TITLE_OWNER_MENU_HOLDER_E0_OFFSET
        ));
        return None;
    }
    append_autoload_debug(format_args!(
        "live-dialog: TitleTopDialog acquired owner+0x{:x}=0x{td:x} (vt 0x{tdvt:x})",
        TITLE_OWNER_MENU_HOLDER_E0_OFFSET
    ));

    // (2) SELF-DIAGNOSTIC (context only): the TitleFlowContext capture at td+0xa38. Probe-5 proved
    // this is a CS::TitleFlowContext (vt 0x142ac7f20), NOT a persistent SceneObjProxy, so it does
    // NOT yield the MenuWindow -- but it IS the correct factory rcx (= td+0xa38). LOG it for
    // context; it never gates acquisition.
    let capture =
        unsafe { safe_read_usize(td + DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET) }.unwrap_or(NULL);
    let cvt = if capture != NULL {
        unsafe { safe_read_usize(capture) }.unwrap_or(NULL)
    } else {
        NULL
    };
    append_autoload_debug(format_args!(
        "live-dialog: capture *(td+0x{:x})=0x{capture:x} vt=0x{cvt:x} (TitleFlowContext; factory rcx) (probe scene_proxy_vt 0x{scene_proxy_vt:x})",
        DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET
    ));

    // (3) MenuWindow: READ the boot-latched host MenuWindow* (latched as rdx of the TitleTopDialog
    // ctor 0x14074a700 by `scene_obj_proxy_ctor_hook`). The MenuWindow is NOT persistently
    // readable at the parked title, so the latch is the only headless source. Fail-closed if 0.
    let menu_window = LATCHED_MENU_WINDOW.load(Ordering::SeqCst);
    if menu_window == NULL {
        append_autoload_debug(format_args!(
            "live-dialog: LATCHED_MENU_WINDOW is 0 (SceneObjProxy ctor 0x14074a700 not yet hit) -- fail-closed, no factory call"
        ));
        return None;
    }
    if menu_window < HEAP_LO || (menu_window & PTR_ALIGN_MASK) != NULL {
        append_autoload_debug(format_args!(
            "live-dialog: latched MenuWindow 0x{menu_window:x} is not a valid heap pointer -- fail-closed, no factory call"
        ));
        return None;
    }
    let mwvt = unsafe { safe_read_usize(menu_window) }.unwrap_or(NULL);
    append_autoload_debug(format_args!(
        "live-dialog: latched MenuWindow=0x{menu_window:x} vt=0x{mwvt:x} (want MenuWindow 0x{menu_vt:x} or MenuWindowProxy 0x{menu_proxy_vt:x})"
    ));
    if mwvt != menu_vt && mwvt != menu_proxy_vt {
        // Loud log but STILL return it (probe visibility) -- the pointer is heap-canonical above.
        append_autoload_debug(format_args!(
            "live-dialog: unexpected latched MenuWindow vtable 0x{mwvt:x} (neither 0x{menu_vt:x} nor 0x{menu_proxy_vt:x}) -- returning anyway for probe visibility"
        ));
    }
    append_autoload_debug(format_args!(
        "live-dialog: ACQUIRED title_dialog=0x{td:x} (vt 0x{title_vt:x}) menu_window=0x{menu_window:x} via boot factory-hook latch"
    ));
    Some((td, menu_window))
}

/// MODEL B (FINAL RECIPE 2026-06-18): build the LIVE registered ProfileLoadDialog by calling the
/// dialog factory 0x14081ead0 WITH THE LIVE CALL-FRAME ARGS -- the only way the dialog becomes
/// live + pumped (the parameterless node-run builds a NON-LIVE dialog and discards it). The factory
/// reads the SceneProxy from [rcx] (r8 = *(dialog+0xa38), the live SceneProxy* the TitleTopDialog
/// ctor stored there at 0x1409a8213) and takes the live MenuWindow* as rdx. So:
///   factory(rcx = title_dialog + 0xa38, rdx = menu_window) -> ProfileLoadDialog* in rax.
/// This builds + registers the dialog into the menu group 0x143d87350 + active-screen set
/// intrinsically (registration is folded into the factory invocation under live args), which the
/// native pump then drives. We FAIL-CLOSED: re-validate the title_dialog vtable (0x142b26468) and
/// that its SceneProxy capture [+0xa38] + the menu_window are non-null heap BEFORE the call; a
/// mismatch returns false with NO native call. Zero-input (the game's own factory, no synthesis).
/// Returns true if the factory was invoked.
unsafe fn fire_live_loadgame_node(title_dialog: usize, menu_window: usize, base: usize) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    if title_dialog == NULL || menu_window == NULL {
        return false;
    }
    let dvt = unsafe { safe_read_usize(title_dialog) }.unwrap_or(NULL);
    let capture_slot = title_dialog + DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET;
    let scene_proxy = unsafe { safe_read_usize(capture_slot) }.unwrap_or(NULL);
    if dvt != base + TITLE_TOP_DIALOG_VTABLE_RVA || scene_proxy < HEAP_LO || menu_window < HEAP_LO {
        append_autoload_debug(format_args!(
            "live-dialog: FIRE ABORT (fail-closed, NO native call) title_dialog=0x{title_dialog:x} vt=0x{dvt:x}(want 0x{:x}) scene_proxy([+0xa38])=0x{scene_proxy:x} menu_window=0x{menu_window:x}",
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return false;
    }
    let factory: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(base + LIVE_DIALOG_FACTORY_RVA) };
    append_autoload_debug(format_args!(
        "live-dialog: FIRE factory 0x{:x}(rcx=title_dialog+0xa38=0x{capture_slot:x} [SceneProxy=0x{scene_proxy:x}], rdx=menu_window=0x{menu_window:x}) -- building LIVE registered ProfileLoadDialog",
        base + LIVE_DIALOG_FACTORY_RVA
    ));
    let dialog = unsafe { factory(capture_slot, menu_window) };
    let pld_vt = base + PROFILE_LOAD_DIALOG_VTABLE_RVA;
    let dialog_vt = if dialog >= HEAP_LO {
        unsafe { safe_read_usize(dialog) }.unwrap_or(NULL)
    } else {
        NULL
    };
    append_autoload_debug(format_args!(
        "live-dialog: factory returned dialog=0x{dialog:x} vt=0x{dialog_vt:x} (want ProfileLoadDialog 0x{pld_vt:x})"
    ));
    // FIX 2 (probe-6): drive the RETURNED dialog directly -- do NOT scan the active-screen array
    // 0x143d6d8d0 (probe-2 proved it is MODEL-RENDERERS, never the PLD). If the returned vtable is
    // the ProfileLoadDialog, store it + transition own_stepper to STAGE2 ACTIVATE on THAT pointer.
    if dialog_vt != pld_vt {
        append_autoload_debug(format_args!(
            "live-dialog: returned dialog vtable 0x{dialog_vt:x} != ProfileLoadDialog 0x{pld_vt:x} -- fail-closed, STAY (NO-WRITE, no STAGE2)"
        ));
        return false;
    }
    OWN_STEPPER_DIALOG.store(dialog, Ordering::SeqCst);
    OWN_STEPPER_S2_WAITS.store(NULL, Ordering::SeqCst);
    OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_S2_ACTIVATE, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "live-dialog: LIVE ProfileLoadDialog=0x{dialog:x} (vt 0x{pld_vt:x}) from factory return -- entering STAGE2 ACTIVATE (slot={})",
        OWN_STEPPER_SLOT.load(Ordering::SeqCst)
    ));
    true
}

/// MODEL B orchestrator (gated by live_dialog_enabled(), OFF by default). At the rendered title
/// menu: (1) do the bounded active-screen scan to acquire the live TitleTopDialog* + MenuWindow*,
/// (2) call the dialog factory 0x14081ead0(rcx=title_dialog+0xa38, rdx=menu_window) ONCE -- which
/// builds + registers the LIVE ProfileLoadDialog into the active-screen set, then (3) wait
/// (bounded, per-frame) for that ProfileLoadDialog (vtable 0x142b229f8) to appear in the
/// active-screen array, latch it as OWN_STEPPER_DIALOG, and hand it to STAGE2 ACTIVATE (which fires
/// load_activate -> native pump mount -> guarded, char-fingerprint-gated continue_confirm).
/// One-shot fire latch; bounded wait. FAIL-CLOSED at every step (no acquisition -> stay; bad
/// vtable -> no call; dialog not live yet -> wait then DONE on timeout). The forge path is untouched.
unsafe fn own_stepper_live_dialog_fire(owner: usize, base: usize, waits: u64) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const FIRE_SETTLE_WAITS: u64 = 30;
    const FIRE_DIALOG_WAIT_MAX: u64 = 600;
    // FIX 2 (probe-6): the factory 0x14081ead0 RETURNS the new dialog in rax. fire_live_loadgame_node
    // validates that return == ProfileLoadDialog (vt 0x142b229f8) and, on a match, stores it as
    // OWN_STEPPER_DIALOG + transitions own_stepper to STAGE2 ACTIVATE on THAT pointer. We no longer
    // scan the active-screen array 0x143d6d8d0 here (probe-2 proved it holds MODEL-RENDERERS, never
    // the PLD -> it would never confirm). Once fired+verified the orchestrator routes to STAGE2.
    if OWN_STEPPER_LIVE_FIRED.load(Ordering::SeqCst) == OWN_STEPPER_LIVE_FIRED_NO {
        if waits < FIRE_SETTLE_WAITS {
            return;
        }
        match unsafe { locate_live_loadgame_node(owner, base) } {
            Some((title_dialog, menu_window)) => {
                // fire_live_loadgame_node returns true ONLY when the factory returned a verified
                // ProfileLoadDialog (it has already stored it + set STAGE2 ACTIVATE on success).
                if unsafe { fire_live_loadgame_node(title_dialog, menu_window, base) } {
                    OWN_STEPPER_LIVE_FIRED.store(OWN_STEPPER_LIVE_FIRED_YES, Ordering::SeqCst);
                } else if waits >= FIRE_DIALOG_WAIT_MAX {
                    append_autoload_debug(format_args!(
                        "live-dialog: factory returned non-PLD (or fail-closed) after {waits} waits -- STAY at menu (NO-WRITE), DONE"
                    ));
                    OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
                }
            }
            None => {
                if waits >= FIRE_DIALOG_WAIT_MAX {
                    append_autoload_debug(format_args!(
                        "live-dialog: could not acquire live args after {waits} waits -- STAY at menu (NO-WRITE), DONE"
                    ));
                    OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
                }
            }
        }
        return;
    }
    // Fired + verified: own_stepper is already in STAGE2 ACTIVATE driving the returned PLD. If we are
    // somehow still here (phase not advanced), bound the wait and stop without writing.
    if waits >= FIRE_DIALOG_WAIT_MAX {
        append_autoload_debug(format_args!(
            "live-dialog: fired factory but STAGE2 did not advance after {waits} waits -- STAY (NO-WRITE), DONE"
        ));
        OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
    }
}

/// Fire a captured MenuWindowJob's `+0xa8` action std::function in-context, mirroring the
/// native leaf Update's functor-invoke at `0x1407ad2b9`:
///   rcx = `[item+0xa8]` (the std::function obj); rax = `[rcx]` (`_Func_impl_no_alloc`
///   vtable, no RTTI); rdx = `item+0x10` (the dialog ctx out-slot, the single arg);
///   call `[rax+0x10]` (`_Do_call`: `add rcx,8; jmp <lambda>`).
/// Returns the lambda result (e.g. the built dialog), which the native Update stores to
/// `[item+0x130]`. Guarded EXACTLY like the native BUILD path: only fires when
/// `[item+0xa8]!=0` AND `[item+0x10]==0`, so we never re-invoke an already-built item
/// (which would leak/overwrite `item+0x130`). This is the game's OWN menu-action functor
/// (NOT input synthesis) -- compliant with the zero-input standard. NOTE: this performs a
/// native call, so it is only used once the live item/owner are validated; it is NOT a
/// save-write by itself (the Load-entry/dialog functors build UI, not save state).
unsafe fn invoke_menu_item_functor(item: usize) -> Option<usize> {
    const ITEM_FUNCTOR_A8: usize = 0xa8;
    const ITEM_CTX_10: usize = 0x10;
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let functor = unsafe { safe_read_usize(item + ITEM_FUNCTOR_A8) }?;
    if functor == null {
        return None;
    }
    // BUILD-path precondition: the native Update fires the functor only when item+0x10==0.
    let ctx_slot = unsafe { safe_read_usize(item + ITEM_CTX_10) }?;
    if ctx_slot != null {
        return None;
    }
    let functor_vtable = unsafe { safe_read_usize(functor) }?;
    if functor_vtable == null {
        return None;
    }
    let do_call = unsafe { safe_read_usize(functor_vtable + DOCALL_VTABLE_SLOT_10) }?;
    if do_call == null {
        return None;
    }
    let f: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(do_call) };
    let ctx_out = item + ITEM_CTX_10;
    Some(unsafe { f(functor, ctx_out) })
}

/// Drive the NATIVE MenuWindowJob::Update 0x1407ad1c0(rcx=item, rdx=&out, r8=framectx) once to
/// BUILD the item's dialog the way the game does. Unlike a bare functor invoke, the native Update
/// WIRES the ctx (item+0x10) from the descriptor (item+0x58 -> resolved window item+0x68 via
/// 0x140d6a8e0 + window-mgr 0x143d83148) BEFORE firing the functor -- so it needs NO synthetic ctx
/// (the prior wall). It is idempotent (returns early if item+0x130 already holds a dialog) and the
/// Load-Game item only builds a ProfileLoadDialog -> BUILD-ONLY, no save write. Guarded by the
/// native BUILD precondition (mirrors 0x1407ad1ec/1fa/208): [item+0x130]==0 && [item+0xa8]!=0 &&
/// [item+0x10]==0. `framectx` is the live FD4Time passed to our idx10 step (the same ctx the native
/// pump feeds the leaf). Returns the built dialog at [item+0x130], if any.
unsafe fn drive_menu_item_update(item: usize, base: usize, framectx: usize) -> Option<usize> {
    const ITEM_FUNCTOR_A8: usize = 0xa8;
    const ITEM_CTX_10: usize = 0x10;
    const ITEM_RESULT_130: usize = 0x130;
    const OUT_ZERO: u64 = 0;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let functor = unsafe { safe_read_usize(item + ITEM_FUNCTOR_A8) }?;
    let ctx = unsafe { safe_read_usize(item + ITEM_CTX_10) }?;
    let pre130 = unsafe { safe_read_usize(item + ITEM_RESULT_130) }?;
    // Native BUILD precondition: dialog not yet built, functor present, ctx not yet wired.
    if functor == null || ctx != null || pre130 != null {
        return None;
    }
    let update: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(base + MENU_ITEM_UPDATE_RVA as usize) };
    // 16-byte writable StepResult out-slot ([0]=status, [4]=payload) the leaf Update writes.
    let mut out = [OUT_ZERO, OUT_ZERO];
    let _ = unsafe { update(item, out.as_mut_ptr() as usize, framectx) };
    let _ = &out;
    unsafe { safe_read_usize(item + ITEM_RESULT_130) }.filter(|&d| d != null)
}

/// Decode a single-child FD4 job decorator's forwarded-child offset from its Update fn
/// prologue. Every decorator in the owner+0x130 menu chain forwards Update to one wrapped
/// child via `mov rcx,[node+disp]; mov rax,[rcx]; call [rax+0x10]`, but the child offset
/// varies per type (0x48, 0x40, ...). Rather than tabulate each, we read the Update fn's
/// first bytes and return the disp of the FIRST `mov rcx,[rcx+disp]`:
///   `48 8b 49 <disp8>`              -> disp8
///   `48 8b 89 <disp32 le>`          -> disp32
/// Returns None if no such load appears in the scanned prologue (not a forwarding decorator).
/// Pure code read via `safe_read_usize`; never faults.
unsafe fn decorator_child_offset(update_fn: usize) -> Option<usize> {
    const SCAN_LEN: usize = 0x28;
    const REXW: usize = 0x48;
    const MOV_RM_OPCODE: usize = 0x8b;
    const MODRM_RCX_RCX_DISP8: usize = 0x49;
    const MODRM_RCX_RCX_DISP32: usize = 0x89;
    const BYTE_MASK: usize = 0xff;
    const B1_SHIFT: usize = 8;
    const B2_SHIFT: usize = 16;
    const B3_SHIFT: usize = 24;
    const DISP32_LEN: usize = 4;
    // bytes consumed by `48 8b 89` before the disp32 immediate begins.
    const DISP32_PREFIX_LEN: usize = 3;
    const SCAN_START: usize = 0;
    const SCAN_STEP: usize = 1;
    let mut i = SCAN_START;
    while i < SCAN_LEN {
        let word = unsafe { safe_read_usize(update_fn + i) }?;
        let b0 = word & BYTE_MASK;
        let b1 = (word >> B1_SHIFT) & BYTE_MASK;
        let b2 = (word >> B2_SHIFT) & BYTE_MASK;
        let b3 = (word >> B3_SHIFT) & BYTE_MASK;
        if b0 == REXW && b1 == MOV_RM_OPCODE {
            if b2 == MODRM_RCX_RCX_DISP8 {
                return Some(b3);
            }
            if b2 == MODRM_RCX_RCX_DISP32 {
                let mut disp = SCAN_START;
                let mut k = SCAN_START;
                while k < DISP32_LEN {
                    let byte = unsafe { safe_read_usize(update_fn + i + DISP32_PREFIX_LEN + k) }?
                        & BYTE_MASK;
                    disp |= byte << (k * B1_SHIFT);
                    k += SCAN_STEP;
                }
                return Some(disp);
            }
        }
        i += SCAN_STEP;
    }
    None
}

/// STAGE 1b (strictly NO-WRITE): recursive bounded walk of the title menu JOB tree rooted
/// at `[owner+0xe0]` (the FD4 multicast/job holder -- runtime proved the real menu lives
/// here, NOT the empty `owner+0x138`). Classifies each node by its Update slot
/// `[vtable+0x10]`: 0x1407aa1f0 = Sequence/IfElse container (children at `[node+0x18]` base,
/// count `[node+0x60]`, stride 8), 0x1407ad1c0 = MenuWindowJob leaf (action functor
/// `[node+0xa8]`). Logs the structure and returns the Load-Game leaf (functor -> dialog
/// factory). Both child-pointer interpretations (base-deref and inline) are enqueued; a
/// visited-set + node/depth caps bound it; fault-tolerant reads never AV. NO writes/calls.
unsafe fn diagnostic_job_tree_walk(
    owner: usize,
    module_base: usize,
    holder_offset: usize,
    tag: &str,
    verbose: bool,
) -> Option<usize> {
    const VTABLE_UPDATE_SLOT_10: usize = 0x10;
    const NODE_CHILDREN_BASE_18: usize = 0x18;
    const NODE_COUNT_60: usize = 0x60;
    const NODE_HOLDER_ROOT_18: usize = 0x18;
    const SEQ_UPDATE_RVA: usize = 0x07aa1f0;
    const LEAF_UPDATE_RVA: usize = 0x07ad1c0;
    // IfElseJob combiner (vt 0x142aa2c38). Its child jobs are NOT at the sequence
    // [+0x18]/[+0x60] layout; that mis-read is the "garbage count" the generic walk hit.
    // Decoded from selector 0x140793390: inline entry array at [node+0x18], stride 0x10,
    // each entry = {predicate@+0, child_job@+0x8}; entry count at [node+0xa0]; default/else
    // child at [node+0xa8]; runtime-active child at [node+0xb0]. Entry + default child jobs
    // are pre-built/retained at BUILD time, so reading them needs no pump.
    const IFELSE_UPDATE_RVA: usize = 0x07931e0;
    // Single-child wrapper (vt 0x142a93af8, update 0x140745510): `mov rcx,[node+0x48];
    // call [rcx]->vt[+0x10]` -- forwards Update to one wrapped child at [node+0x48]. The
    // IfElseJob entry child jobs are these wrappers, not MenuWindowJobs directly.
    const WRAP_UPDATE_RVA: usize = 0x0745510;
    const WRAP_CHILD_48: usize = 0x48;
    const IFELSE_ENTRY_STRIDE_10: usize = 0x10;
    const IFELSE_ENTRY_JOB_8: usize = 0x8;
    const IFELSE_COUNT_A0: usize = 0xa0;
    const IFELSE_DEFAULT_A8: usize = 0xa8;
    const IFELSE_ACTIVE_B0: usize = 0xb0;
    const ITEM_CTX_10: usize = 0x10;
    const ITEM_RESULT_130: usize = 0x130;
    const PTR_STRIDE: usize = core::mem::size_of::<usize>();
    const COUNT_MIN: usize = 1;
    const COUNT_MAX: usize = 32;
    const MAX_NODES: usize = 256;
    const MAX_DEPTH: usize = 8;
    const WALK_START: usize = 0;
    const WALK_STEP: usize = 1;
    // Generic decorator descent. The owner+0x130 menu tree threads d180 through a chain of
    // single-child FD4 job decorators (vt 0x142a93af8 child@+0x48, vt 0x142a93d18 child@+0x40,
    // ...) with per-type child offsets. Rather than decode each, for any node that is none of
    // the known container/leaf kinds we scan a bounded field window and enqueue every qword
    // that points at an in-module job object (its vtable AND that vtable's Update slot both
    // land inside the game image). Fault-tolerant reads; visited-set + node budget bound it.
    const GEN_SCAN_LO: usize = 0x10;
    const GEN_SCAN_HI: usize = 0xc0;
    // PE image bounds (for the in-module pointer test): SizeOfImage at NT+0x50, e_lfanew at
    // base+0x3c. Both are u32; mask the low dword off the qword read.
    const PE_E_LFANEW_OFFSET: usize = 0x3c;
    const PE_SIZE_OF_IMAGE_FROM_NT: usize = 0x50;
    const PE_U32_MASK: usize = 0xffffffff;
    const MODULE_SPAN_FALLBACK: usize = 0x3000000;
    const MODULE_MIN_OFFSET: usize = 0x1000;

    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let seq_update_abs = module_base + SEQ_UPDATE_RVA;
    let leaf_update_abs = module_base + LEAF_UPDATE_RVA;
    let ifelse_update_abs = module_base + IFELSE_UPDATE_RVA;
    let wrap_update_abs = module_base + WRAP_UPDATE_RVA;

    let e_lfanew = unsafe { safe_read_usize(module_base + PE_E_LFANEW_OFFSET) }
        .map(|v| v & PE_U32_MASK)
        .unwrap_or(null);
    let image_span = if e_lfanew != null {
        unsafe { safe_read_usize(module_base + e_lfanew + PE_SIZE_OF_IMAGE_FROM_NT) }
            .map(|v| v & PE_U32_MASK)
            .filter(|&s| s != null)
            .unwrap_or(MODULE_SPAN_FALLBACK)
    } else {
        MODULE_SPAN_FALLBACK
    };
    let module_lo = module_base + MODULE_MIN_OFFSET;
    let module_hi = module_base + image_span;
    let in_module = |p: usize| p >= module_lo && p < module_hi;

    let holder = unsafe { safe_read_usize(owner + holder_offset) }.unwrap_or(null);
    if verbose {
        append_autoload_debug(format_args!(
            "job-tree[{tag}]: owner=0x{owner:x} holder(owner+0x{holder_offset:x})=0x{holder:x} seq_update=0x{seq_update_abs:x} leaf_update=0x{leaf_update_abs:x}"
        ));
    }
    if holder == null {
        return None;
    }
    let root = unsafe { safe_read_usize(holder + NODE_HOLDER_ROOT_18) }.unwrap_or(null);

    let mut load_game: Option<usize> = None;
    let mut visited: Vec<usize> = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();
    stack.push((holder, WALK_START));
    if root != null {
        stack.push((root, WALK_START));
    }
    let mut node_budget = MAX_NODES;
    while let Some((node, depth)) = stack.pop() {
        if node_budget == WALK_START {
            break;
        }
        node_budget -= WALK_STEP;
        if node == null || visited.contains(&node) {
            continue;
        }
        visited.push(node);
        let vtable = unsafe { safe_read_usize(node) }.unwrap_or(null);
        let update = if vtable != null {
            unsafe { safe_read_usize(vtable + VTABLE_UPDATE_SLOT_10) }.unwrap_or(null)
        } else {
            null
        };
        let count = unsafe { safe_read_usize(node + NODE_COUNT_60) }.unwrap_or(null);
        let base = unsafe { safe_read_usize(node + NODE_CHILDREN_BASE_18) }.unwrap_or(null);
        let is_leaf = update == leaf_update_abs;
        let is_container = update == seq_update_abs;
        let is_ifelse = update == ifelse_update_abs;
        let is_wrap = update == wrap_update_abs;
        let wrap_child = unsafe { safe_read_usize(node + WRAP_CHILD_48) }.unwrap_or(null);
        let ife_count = unsafe { safe_read_usize(node + IFELSE_COUNT_A0) }.unwrap_or(null);
        let ife_default = unsafe { safe_read_usize(node + IFELSE_DEFAULT_A8) }.unwrap_or(null);
        let ife_active = unsafe { safe_read_usize(node + IFELSE_ACTIVE_B0) }.unwrap_or(null);
        let mut chain = String::new();
        let is_load_game = if update != null {
            unsafe { functor_chain_hits_factory(node, module_base, &mut chain) }
        } else {
            false
        };
        if is_load_game && load_game.is_none() {
            load_game = Some(node);
        }
        let ctx = unsafe { safe_read_usize(node + ITEM_CTX_10) }.unwrap_or(null);
        let result = unsafe { safe_read_usize(node + ITEM_RESULT_130) }.unwrap_or(null);
        if verbose {
            append_autoload_debug(format_args!(
                "job-tree[{tag}] d={depth} node=0x{node:x} vt=0x{vtable:x} update=0x{update:x} leaf={is_leaf} container={is_container} ifelse={is_ifelse} wrap={is_wrap} count=0x{count:x} base=0x{base:x} ife_count=0x{ife_count:x} ife_default=0x{ife_default:x} ife_active=0x{ife_active:x} wrap_child=0x{wrap_child:x} ctx=0x{ctx:x} result=0x{result:x} {chain} LOAD_GAME={is_load_game}"
            ));
        }
        if depth < MAX_DEPTH && is_wrap {
            // Single-child wrapper: descend into its one forwarded child.
            if wrap_child != null {
                stack.push((wrap_child, depth + WALK_STEP));
            }
        } else if depth < MAX_DEPTH && is_ifelse {
            // IfElseJob (selector 0x140793390): a case vector at [node+0x18], stride 0x10, each
            // case = {predicate@+0, child_job@+0x8}; the main-menu branch (holding d180) binds its
            // child to [node+0xb0] ONLY when its input-gated predicate flips (so headless d180 is
            // present-but-unbound). The case COUNT offset is ambiguous across memos (+0xa0 vs +0x88
            // = capacity vs size), so rather than trust a count we do a bounded LAYOUT-AGNOSTIC
            // scan of the case slots and enqueue every child_job (and predicate slot) that points
            // at an in-module job object -- this reaches d180's case child whether or not its
            // branch is bound, with no pump. Pure reads; visited-set + node budget bound it.
            let _ = (ife_count, IFELSE_COUNT_A0, COUNT_MIN, IFELSE_ENTRY_JOB_8);
            let mut i = WALK_START;
            while i < COUNT_MAX {
                let case = node + NODE_CHILDREN_BASE_18 + i * IFELSE_ENTRY_STRIDE_10;
                for slot in [WALK_START, IFELSE_ENTRY_JOB_8] {
                    let child = unsafe { safe_read_usize(case + slot) }.unwrap_or(null);
                    if child != null && child != node {
                        let cvt = unsafe { safe_read_usize(child) }.unwrap_or(null);
                        if in_module(cvt) {
                            stack.push((child, depth + WALK_STEP));
                        }
                    }
                }
                i += WALK_STEP;
            }
            if ife_default != null {
                stack.push((ife_default, depth + WALK_STEP));
            }
            if ife_active != null && ife_active != ife_default {
                stack.push((ife_active, depth + WALK_STEP));
            }
        } else if depth < MAX_DEPTH && is_container && (COUNT_MIN..=COUNT_MAX).contains(&count) {
            let mut i = WALK_START;
            while i < count {
                let child_b = if base != null {
                    unsafe { safe_read_usize(base + i * PTR_STRIDE) }.unwrap_or(null)
                } else {
                    null
                };
                let child_i =
                    unsafe { safe_read_usize(node + NODE_CHILDREN_BASE_18 + i * PTR_STRIDE) }
                        .unwrap_or(null);
                if child_b != null {
                    stack.push((child_b, depth + WALK_STEP));
                }
                if child_i != null && child_i != child_b {
                    stack.push((child_i, depth + WALK_STEP));
                }
                i += WALK_STEP;
            }
        } else if depth < MAX_DEPTH && !is_leaf && in_module(vtable) && in_module(update) {
            // Unknown FD4 decorator: decode the single forwarded-child offset from its Update
            // prologue (`mov rcx,[node+disp]`) and descend into [node+disp] ONLY -- a precise
            // single-child follow, never a field scan (which wandered into the GUI graph).
            if let Some(off) = unsafe { decorator_child_offset(update) } {
                if (GEN_SCAN_LO..=GEN_SCAN_HI).contains(&off) {
                    let child = unsafe { safe_read_usize(node + off) }.unwrap_or(null);
                    if child != null && child != node {
                        let cvt = unsafe { safe_read_usize(child) }.unwrap_or(null);
                        if in_module(cvt) {
                            stack.push((child, depth + WALK_STEP));
                        }
                    }
                }
            }
        }
    }
    if verbose {
        append_autoload_debug(format_args!(
            "job-tree[{tag}] summary: nodes_visited={} load_game=0x{:x}",
            visited.len(),
            load_game.unwrap_or(null)
        ));
    }
    load_game
}

/// DETERMINISTIC MENU INPUT PROBE driver. Runs each frame (in PHASE_MENU_BUILD, after the menu is
/// open) when `input_probe_enabled()`. Schedule (probe-frame `f`, see lib.rs consts):
///   [0, DOWN_START)                 SETTLE   -- baseline, no input (rows empty headless?)
///   [DOWN_START, +DOWN_TAP_FRAMES)  DOWN     -- inject one Down (Continue->Load Game)
///   [DOWN_START, CONFIRM_START)     HIGHLIGHT-- NO input; watch MENU_D180_LEAF_TICKED grow?
///   [CONFIRM_START, +CONFIRM_TAP)   CONFIRM  -- inject Confirm; native load fires (captured)
/// The decisive signal is whether the genuine d180 leaf-Update tick count grows during HIGHLIGHT
/// (before Confirm). Pure reads + the two keystate-bit writes; no SetState here (the Confirm drives
/// the native load). `dump_titletop_menu_entries` logs the live router_this row vector each interval.
unsafe fn menu_input_probe(owner: usize, base: usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    INPUT_PROBE_ACTIVE.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    let inputmgr =
        unsafe { safe_read_usize(base + SELECTBOT_INPUT_MANAGER_GLOBAL_RVA) }.unwrap_or(NULL);
    let f = INPUT_PROBE_FRAME.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
    let item = MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst);
    let leaf_ticks = MENU_D180_LEAF_TICKED.load(Ordering::SeqCst);

    let in_down =
        f >= INPUT_PROBE_DOWN_START && f < INPUT_PROBE_DOWN_START + INPUT_PROBE_DOWN_TAP_FRAMES;
    let in_highlight = f >= INPUT_PROBE_DOWN_START && f < INPUT_PROBE_CONFIRM_START;
    let in_confirm = f >= INPUT_PROBE_CONFIRM_START
        && f < INPUT_PROBE_CONFIRM_START + INPUT_PROBE_CONFIRM_TAP_FRAMES;

    if inputmgr != NULL {
        if in_down {
            // Inject BOTH vertical-move events (one is Down, one Up; Up saturates at the top so
            // from Continue only Down moves -> lands on Load Game). Edge-triggered &1.
            unsafe {
                *((inputmgr + INPUTMGR_BITMAP_90_OFFSET + MENU_EVENT_MOVE_A_00) as *mut u8) |=
                    MENU_EVENT_PRESSED_BIT;
                *((inputmgr + INPUTMGR_BITMAP_90_OFFSET + MENU_EVENT_MOVE_B_45) as *mut u8) |=
                    MENU_EVENT_PRESSED_BIT;
            }
        }
        if in_confirm {
            unsafe {
                *((inputmgr + INPUTMGR_BITMAP_90_OFFSET + MENU_EVENT_CONFIRM_3D) as *mut u8) |=
                    MENU_EVENT_PRESSED_BIT;
            }
        }
    }

    // DECISIVE one-shot: d180's leaf Update ticked during the highlight window (after Down, before
    // Confirm). Snapshot taken at DOWN_START; any growth here means highlight ALONE ticks d180.
    if in_highlight
        && leaf_ticks > INPUT_PROBE_DOWN_LEAF_BASELINE.load(Ordering::SeqCst)
        && INPUT_PROBE_D180_PRECONFIRM.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst) == NULL
    {
        let (l, c, cur) = unsafe { dump_titletop_menu_entries(owner, base) };
        append_autoload_debug(format_args!(
            "INPUT-PROBE: *** d180 LEAF-TICKED during HIGHLIGHT (pre-confirm) f={f} ticks={leaf_ticks} item=0x{item:x} cursor={cur} load_entry=0x{:x} cont_entry=0x{:x} *** -> highlight ALONE ticks d180; zero-input functor-invoke route VIABLE",
            l.unwrap_or(NULL),
            c.unwrap_or(NULL)
        ));
    }

    if f == INPUT_PROBE_DOWN_START {
        // Latch the leaf-tick baseline at the moment Down begins, so HIGHLIGHT growth is measured
        // strictly from here (ignores any pre-Down ticks).
        INPUT_PROBE_DOWN_LEAF_BASELINE.store(leaf_ticks, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "INPUT-PROBE: DOWN inject f={f} inputmgr=0x{inputmgr:x} leaf_baseline={leaf_ticks} -- highlight window [{}..{}) before Confirm",
            INPUT_PROBE_DOWN_START, INPUT_PROBE_CONFIRM_START
        ));
    }
    if f == INPUT_PROBE_CONFIRM_START {
        let pre = INPUT_PROBE_D180_PRECONFIRM.load(Ordering::SeqCst) != NULL;
        append_autoload_debug(format_args!(
            "INPUT-PROBE: CONFIRM inject f={f} d180_leaf_ticked_on_highlight={pre} ticks_now={leaf_ticks} -- {} (load now fires via Confirm)",
            if pre {
                "highlight WAS sufficient"
            } else {
                "highlight did NOT tick d180 -> needs static walk / focus is required"
            }
        ));
    }
    if f % INPUT_PROBE_LOG_INTERVAL == NULL as u64 {
        let phase = if in_down {
            "DOWN"
        } else if in_confirm {
            "CONFIRM"
        } else if in_highlight {
            "HIGHLIGHT"
        } else if f < INPUT_PROBE_DOWN_START {
            "SETTLE"
        } else {
            "POST"
        };
        append_autoload_debug(format_args!(
            "INPUT-PROBE: f={f} phase={phase} d180_item=0x{item:x} leaf_ticks={leaf_ticks}"
        ));
        let _ = unsafe { dump_titletop_menu_entries(owner, base) };
    }
}

/// OBSERVE-ONLY NATIVE-LOAD tick (native_load_enabled(), gated OFF by default). Runs each frame
/// INSTEAD of the own_stepper forcing logic, then the caller pass-throughs to OWN_STEPPER_ORIG_IDX10
/// so the NATIVE title machine advances untouched (the user drives past press-any-button + modals).
/// KEEP vs the normal own_stepper: it does NOT SetState(owner,2/3), does NOT clear the beginlogo
/// gate, does NOT self-fire the registrar 0x1409b24e0, does NOT run direct_build / cold_char_mount.
/// It ONLY: (1) read-only checks whether the live TitleTopDialog menu is rendered (owner+0xe0 ->
/// dialog, *(dialog)==base+TITLE_TOP_DIALOG_VTABLE_RVA, [dialog+0xa48] row registry populated);
/// (2) once the menu first appears, waits NATIVE_LOAD_SETTLE_FRAMES; (3) ONE-SHOT: locates the
/// Load-Game CS::MenuMemberFuncJob node via scan_dialog_for_loadgame and fires its native run
/// MENU_MEMBER_FUNC_JOB_RUN_RVA (0x1409aaba0, rcx=node) -- which builds the LIVE registered
/// ProfileLoadDialog the native pump drives. After firing it observes (the caller keeps writing the
/// golden oracle as the native pump hopefully loads the char). Pure read-only until the single fire.
unsafe fn native_load_tick(owner: usize, base: usize, n: u64) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    const PTR_ALIGN_MASK: usize = 0x7;
    const NO_TICK: usize = 0;
    // Already fired: keep observing (oracle written by the caller's pass-through telemetry).
    if NATIVE_LOAD_FIRED.load(Ordering::SeqCst) != NATIVE_LOAD_FIRED_NO {
        if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            append_autoload_debug(format_args!(
                "native-load: FIRED -- observing native pump (#{n}); golden oracle written via telemetry"
            ));
        }
        return;
    }
    // (1) Is the live TitleTopDialog menu rendered? owner+0xe0 -> dialog, vtable-gated, with its
    // row registry [dialog+0xa48] populated. Pure reads -- fail-closed (just keep observing) if not.
    let dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(NULL);
    let dialog_vt = if dialog != NULL {
        unsafe { safe_read_usize(dialog) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let registry = if dialog != NULL {
        unsafe { safe_read_usize(dialog + DIALOG_ROW_REGISTRY_A48_OFFSET) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let menu_live = dialog_vt == base + TITLE_TOP_DIALOG_VTABLE_RVA
        && registry != NULL
        && registry >= HEAP_LO
        && (registry & PTR_ALIGN_MASK) == NULL;
    if !menu_live {
        if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            append_autoload_debug(format_args!(
                "native-load: waiting for live menu (#{n}) dialog=0x{dialog:x} vt=0x{dialog_vt:x}(want 0x{:x}) registry[0xa48]=0x{registry:x}",
                base + TITLE_TOP_DIALOG_VTABLE_RVA
            ));
        }
        return;
    }
    // (2) Settle gate: latch the frame the live+settled menu was FIRST seen; wait
    // NATIVE_LOAD_SETTLE_FRAMES so the registrar finishes populating the rows before we fire.
    let first_seen = NATIVE_LOAD_MENU_FIRST_SEEN.load(Ordering::SeqCst);
    if first_seen == NO_TICK {
        NATIVE_LOAD_MENU_FIRST_SEEN.store(n as usize, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "native-load: LIVE MENU first seen #{n} dialog=0x{dialog:x} registry=0x{registry:x} -- settling {NATIVE_LOAD_SETTLE_FRAMES} frames before firing native Load run"
        ));
        return;
    }
    if (n as usize).saturating_sub(first_seen) < NATIVE_LOAD_SETTLE_FRAMES {
        return;
    }
    // (3) ONE-SHOT fire. Locate the Load-Game MenuMemberFuncJob node (scan_dialog_for_loadgame
    // returns (member_node, window_item)); fire its native run rcx=node. Latch FIRST so a re-entry
    // (or a scan that finds nothing) never double-fires; if the node is missing, mark fired with a
    // loud log (stay observing -- NO blind retry) so we never spin firing.
    if NATIVE_LOAD_FIRED.swap(NATIVE_LOAD_FIRED_YES, Ordering::SeqCst) != NATIVE_LOAD_FIRED_NO {
        return;
    }
    let (member_node, window_item) = unsafe { scan_dialog_for_loadgame(owner, base) };
    let Some(node) = member_node else {
        append_autoload_debug(format_args!(
            "native-load: live menu settled (#{n}) but scan found NO Load-Game MenuMemberFuncJob node (window_item=0x{:x}) -- NOT firing, stay observing",
            window_item.unwrap_or(NULL)
        ));
        return;
    };
    // Validate the node vtable read-only before the native call (look before acting): it must be a
    // CS::MenuMemberFuncJob (vt MEMBERFUNCJOB_VTABLE_RVA). run computes rcx=[node+0x10]+[node+0x20]
    // and calls [node+0x18]; log those for the record.
    let node_vt = unsafe { safe_read_usize(node) }.unwrap_or(NULL);
    if node_vt != base + MEMBERFUNCJOB_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "native-load: FIRE ABORT node=0x{node:x} vt=0x{node_vt:x} != MenuMemberFuncJob 0x{:x} -- stay observing (NO native call)",
            base + MEMBERFUNCJOB_VTABLE_RVA
        ));
        return;
    }
    const MEMBER_DIALOG_10: usize = 0x10;
    const MEMBER_FN_18: usize = 0x18;
    const MEMBER_ADJ_20: usize = 0x20;
    let m_dlg = unsafe { safe_read_usize(node + MEMBER_DIALOG_10) }.unwrap_or(NULL);
    let m_fn = unsafe { safe_read_usize(node + MEMBER_FN_18) }.unwrap_or(NULL);
    let m_adj = unsafe { safe_read_usize(node + MEMBER_ADJ_20) }.unwrap_or(NULL);
    let run: unsafe extern "system" fn(usize) = unsafe {
        std::mem::transmute::<usize, unsafe extern "system" fn(usize)>(
            base + MENU_MEMBER_FUNC_JOB_RUN_RVA,
        )
    };
    append_autoload_debug(format_args!(
        "native-load: *** FIRING native Load-Game run 0x{:x}(rcx=node=0x{node:x}) vt=0x{node_vt:x} [+0x10]=0x{m_dlg:x} [+0x18]=0x{m_fn:x} [+0x20]=0x{m_adj:x} #{n} -- building LIVE ProfileLoadDialog in the NATURAL menu (zero forcing) ***",
        base + MENU_MEMBER_FUNC_JOB_RUN_RVA
    ));
    timeline_event(
        "T_native_load_fire",
        n,
        format_args!("node=0x{node:x} member_fn=0x{m_fn:x}"),
    );
    unsafe { run(node) };
    append_autoload_debug(format_args!(
        "native-load: native Load-Game run returned -- observing native pump for golden oracle (#{n})"
    ));
}

/// Resolve the full-read target slot: a configured OWN_STEPPER_SLOT (>=0, from the trigger-file
/// "slot=N"), else ER_EFFECTS_AUTOLOAD_SLOT (>=0), else FULLREAD_DEFAULT_SLOT (Banon = 0).
fn native_fullread_slot() -> i32 {
    let configured = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    if configured >= OWN_STEPPER_SLOT_ZERO {
        return configured;
    }
    if let Ok(v) = std::env::var("ER_EFFECTS_AUTOLOAD_SLOT") {
        if let Ok(slot) = v.trim().parse::<i32>() {
            if slot >= OWN_STEPPER_SLOT_ZERO {
                return slot;
            }
        }
    }
    FULLREAD_DEFAULT_SLOT
}

/// OBSERVE-ONLY NATIVE FULL-SAVE-READ tick (native_fullread_enabled(), gated OFF by default). Runs
/// each frame INSTEAD of the own_stepper forcing logic (no SetState forcing for boot); the caller
/// pass-throughs to OWN_STEPPER_ORIG_IDX10 so the NATIVE title machine advances untouched. Once the
/// live TitleTopDialog menu is rendered + settled (same detection as native_load_tick: owner+0xe0 ->
/// dialog, *(dialog)==TitleTopDialog vtable, [dialog+0xa48] row registry populated, then a
/// NATIVE_LOAD_SETTLE_FRAMES settle), it runs the full-save-read load chain as a per-frame phase
/// machine at the LIVE menu (where the FD4 IO worker pool 0x144853048 is live so the submit drains):
///   SUBMIT: set GameMan+0xb78=slot (step 1, NEW), set_save_slot 0x14067a810 (step 2 -> GameMan+0xac0),
///           submit full read 0x14067b1a0 (step 3, type-0xa).
///   DRAIN:  tick lane 0x140679510 + poll 0x140679180 each frame until GameMan+0xb80==3 (step 4).
///   DESER:  deserialize 0x14067b290(slot) ONCE at b80==3 (step 5 -> GameMan+0xc30 = real map).
///   GUARD:  c30 != 0xa010000 (m10 default) AND char fingerprint present (level>=10 + name) (step 6).
///   CONFIRM (step 7, the SOLE save write): ONLY if the guard passes AND native_fullread_commit_enabled():
///           continue_confirm 0x140b0e180(rcx=shim{[OWNER]=owner}) where owner=*(base+0x3d5df38+8);
///           it checks owner+0x284==0 -> sets owner+0xbc=c30 + SetState5 (AUTOSAVES). Without the
///           commit sub-gate, stops at GUARD (VERIFY-ONLY: log only, NO continue_confirm/NO SetState5).
/// Reuses cold_char_mount_drive's submit/lane/poll/deser CALLS (exact RVAs) but builds/pumps NO
/// selector step (probe-12 crash) and forces NO SetState for boot. Logs b80/c30/level each frame.
unsafe fn native_fullread_tick(owner: usize, base: usize, n: u64) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    const PTR_ALIGN_MASK: usize = 0x7;
    const NO_TICK: usize = 0;
    const WAIT_INC: usize = 1;
    let gm = unsafe { safe_read_usize(base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) }.unwrap_or(NULL);
    let phase = FULLREAD_PHASE.load(Ordering::SeqCst);
    // Already finished: keep observing (the golden oracle is written by the caller's telemetry once
    // the native pump streams the world).
    if phase == FULLREAD_PHASE_DONE {
        if n % FULLREAD_LOG_INTERVAL == NULL as u64 {
            let c30 = if gm != NULL {
                unsafe { *((gm + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) }
            } else {
                GAME_MAN_C30_UNSET
            };
            let (_fp_real, level, _name_len) = unsafe { char_fingerprint(base) };
            append_autoload_debug(format_args!(
                "native-fullread: DONE -- observing native pump (#{n}) c30=0x{c30:x} level={level}"
            ));
        }
        return;
    }
    // (A) Live-menu detection (same as native_load_tick): owner+0xe0 -> dialog, vtable-gated, with
    // its row registry [dialog+0xa48] populated. Pure reads -- fail-closed (keep observing) if not.
    let dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(NULL);
    let dialog_vt = if dialog != NULL {
        unsafe { safe_read_usize(dialog) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let registry = if dialog != NULL {
        unsafe { safe_read_usize(dialog + DIALOG_ROW_REGISTRY_A48_OFFSET) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let menu_live = dialog_vt == base + TITLE_TOP_DIALOG_VTABLE_RVA
        && registry != NULL
        && registry >= HEAP_LO
        && (registry & PTR_ALIGN_MASK) == NULL;
    if !menu_live || gm == NULL {
        if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            append_autoload_debug(format_args!(
                "native-fullread: waiting for live menu (#{n}) gm=0x{gm:x} dialog=0x{dialog:x} vt=0x{dialog_vt:x}(want 0x{:x}) registry[0xa48]=0x{registry:x}",
                base + TITLE_TOP_DIALOG_VTABLE_RVA
            ));
        }
        return;
    }
    // (B) Settle gate: latch the frame the live menu was FIRST seen; wait NATIVE_LOAD_SETTLE_FRAMES.
    let first_seen = FULLREAD_MENU_FIRST_SEEN.load(Ordering::SeqCst);
    if first_seen == NO_TICK {
        FULLREAD_MENU_FIRST_SEEN.store(n as usize, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "native-fullread: LIVE MENU first seen #{n} dialog=0x{dialog:x} registry=0x{registry:x} -- settling {NATIVE_LOAD_SETTLE_FRAMES} frames before the full-save-read chain"
        ));
        return;
    }
    if (n as usize).saturating_sub(first_seen) < NATIVE_LOAD_SETTLE_FRAMES {
        return;
    }
    let slot = native_fullread_slot();
    let read_i32 = |off: usize| unsafe { *((gm + off) as *const i32) };

    if phase == FULLREAD_PHASE_SUBMIT {
        // Step 1 (NEW): set the slot-resolve global GameMan+0xb78=slot (resolver 0x1406793c0 returns
        // *(u32*)(gm+0xb78)) so the native chain resolves OUR slot. Save-safe (an in-memory selector).
        unsafe { *((gm + GAME_MAN_SLOT_SELECT_B78_OFFSET) as *mut i32) = slot };
        // Step 2: set_save_slot 0x14067a810(slot) -> GameMan+0xac0=slot.
        let set_save_slot: unsafe extern "system" fn(i32) =
            unsafe { std::mem::transmute(base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
        unsafe { set_save_slot(slot) };
        // Step 3: submit the full read 0x14067b1a0(slot) (type-0xa; sets GameMan+0xb80=2, the
        // deserialize arm). At the LIVE menu the FD4 IO worker pool is live so this DRAINS.
        let submit: unsafe extern "system" fn(i32) -> i32 =
            unsafe { std::mem::transmute(base + B80_FULL_LOAD_INITIATOR_RVA) };
        let sret = unsafe { submit(slot) };
        let b80 = read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
        let ac0 = read_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
        let b78 = read_i32(GAME_MAN_SLOT_SELECT_B78_OFFSET);
        append_autoload_debug(format_args!(
            "native-fullread: SUBMIT slot={slot} b78={b78} (0x{:x} write) set_save_slot 0x{:x} ac0={ac0} submit 0x{:x} ret={sret} b80={b80} -> DRAIN",
            base,
            base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA,
            base + B80_FULL_LOAD_INITIATOR_RVA
        ));
        timeline_event(
            "T_fullread_submit",
            n,
            format_args!("slot={slot} b80={b80}"),
        );
        FULLREAD_DRAIN_WAITS.store(NULL, Ordering::SeqCst);
        FULLREAD_PHASE.store(FULLREAD_PHASE_DRAIN, Ordering::SeqCst);
        return;
    }

    if phase == FULLREAD_PHASE_DRAIN {
        // Step 4: tick lane 0x140679510 (b80==1/2 IO tick) + poll 0x140679180 each frame until
        // GameMan+0xb80==3 (RESIDENT, the 0x280000 buffer drained). Reuses cold_char_mount's calls.
        let lane: unsafe extern "system" fn() -> i32 =
            unsafe { std::mem::transmute(base + B80_LANE1_DRIVER_RVA) };
        let _ = unsafe { lane() };
        let poll: unsafe extern "system" fn(u8, u8) -> i32 =
            unsafe { std::mem::transmute(base + B80_POLL_RVA) };
        let _ = unsafe { poll(FULLREAD_POLL_ARG, FULLREAD_POLL_ARG) };
        let b80 = read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
        let c30 = read_i32(GAME_MAN_SAVED_MAP_C30_OFFSET);
        let w = FULLREAD_DRAIN_WAITS.fetch_add(WAIT_INC, Ordering::SeqCst) as u64;
        if w % FULLREAD_LOG_INTERVAL == NULL as u64 {
            let (_fp, level, _nl) = unsafe { char_fingerprint(base) };
            append_autoload_debug(format_args!(
                "native-fullread: DRAIN waits={w} b80={b80} c30=0x{c30:x} level={level}"
            ));
        }
        if b80 == FULLREAD_B80_RESIDENT {
            append_autoload_debug(format_args!(
                "native-fullread: b80 reached RESIDENT(3) after {w} drain ticks -- the LIVE worker pool DRAINED the full read -> DESER"
            ));
            FULLREAD_PHASE.store(FULLREAD_PHASE_DESER, Ordering::SeqCst);
        } else if w >= FULLREAD_DRAIN_MAX {
            append_autoload_debug(format_args!(
                "native-fullread: b80 STUCK at {b80} after {w} drain ticks (full read never resident) -- TIMEOUT (no write) -> DONE"
            ));
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
        }
        return;
    }

    if phase == FULLREAD_PHASE_DESER {
        // Step 5: deserialize 0x14067b290(slot) ONCE at b80==3 -> writes GameMan+0xc30 = real map.
        let deser: unsafe extern "system" fn(i32) -> i32 =
            unsafe { std::mem::transmute(base + DESERIALIZE_SLOT_RVA) };
        let dret = unsafe { deser(slot) };
        let c30 = read_i32(GAME_MAN_SAVED_MAP_C30_OFFSET);
        let ac0 = read_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
        let (_fp, level, _nl) = unsafe { char_fingerprint(base) };
        append_autoload_debug(format_args!(
            "native-fullread: DESER slot={slot} ret={dret} c30=0x{c30:x} ac0={ac0} level={level} -> GUARD"
        ));
        timeline_event(
            "T_fullread_deser",
            n,
            format_args!("c30=0x{c30:x} level={level}"),
        );
        FULLREAD_PHASE.store(FULLREAD_PHASE_GUARD, Ordering::SeqCst);
        return;
    }

    if phase == FULLREAD_PHASE_GUARD {
        // Step 6: GUARD. c30 != 0xa010000 (m10 default) AND char fingerprint present (level>=10 +
        // non-empty name). This is the HARD gate for the only save write.
        let c30 = read_i32(GAME_MAN_SAVED_MAP_C30_OFFSET);
        let (fp_real, level, name_len) = unsafe { char_fingerprint(base) };
        let c30_real = c30 != FULLREAD_C30_M10_DEFAULT && c30 != GAME_MAN_C30_UNSET;
        let level_real = level >= FULLREAD_MIN_REAL_LEVEL;
        let guard_pass = c30_real && fp_real && level_real;
        let commit = native_fullread_commit_enabled();
        append_autoload_debug(format_args!(
            "native-fullread: GUARD c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={level} level_real={level_real} name_len={name_len} -> guard_pass={guard_pass} commit_gate={commit}"
        ));
        if !guard_pass {
            append_autoload_debug(format_args!(
                "native-fullread: GUARD FAIL (c30=0x{c30:x} level={level}) -- NO continue_confirm, NO SetState5, NO save write -> DONE (save-safe)"
            ));
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        // Step 7 is HARD-gated behind BOTH the guard above AND the commit sub-gate (default off):
        // VERIFY-ONLY by default -- stop here (log only, NO continue_confirm/NO SetState5).
        if !commit {
            append_autoload_debug(format_args!(
                "native-fullread: GUARD PASS (c30=0x{c30:x} level={level}) but VERIFY-ONLY (commit sub-gate OFF) -- NO continue_confirm, NO SetState5 -> DONE (save-safe). Set ER_EFFECTS_FULLREAD_COMMIT=1 / er-effects-fullread-commit.txt to commit."
            ));
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        // COMMIT: continue_confirm 0x140b0e180(rcx=&shim{[OWNER]=owner}), owner=*(base+0x3d5df38+8).
        // It checks owner+0x284==0 -> sets owner+0xbc=c30 + SetState5 (AUTOSAVES). Look before acting:
        // resolve owner read-only + confirm owner+0x284==0 before the native call (fail-closed).
        let owner_obj = unsafe {
            safe_read_usize(base + PLAYER_GAME_DATA_SINGLETON_RVA + FULLREAD_OWNER_GDM_08_OFFSET)
        }
        .unwrap_or(NULL);
        if owner_obj == NULL {
            append_autoload_debug(format_args!(
                "native-fullread: COMMIT ABORT -- continue_confirm owner (*(base+0x{:x}+0x{:x})) is null -> DONE (no write)",
                PLAYER_GAME_DATA_SINGLETON_RVA, FULLREAD_OWNER_GDM_08_OFFSET
            ));
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        let new_game_flag =
            unsafe { *((owner_obj + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET) as *const u8) };
        if new_game_flag != FULLREAD_OWNER_NEW_GAME_OK {
            append_autoload_debug(format_args!(
                "native-fullread: COMMIT ABORT -- owner+0x284={new_game_flag} != 0 (continue_confirm requires the new-game flag clear) -> DONE (no write)"
            ));
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        let shim = &raw mut OWN_STEPPER_SHIM;
        unsafe { (*shim)[OWN_STEPPER_SHIM_OWNER_IDX] = owner_obj };
        let shim_ptr = shim as usize;
        let confirm: unsafe extern "system" fn(usize) =
            unsafe { std::mem::transmute(base + CONTINUE_CONFIRM_RVA) };
        append_autoload_debug(format_args!(
            "native-fullread: *** COMMIT continue_confirm 0x{:x}(shim=0x{shim_ptr:x} owner=0x{owner_obj:x}) c30=0x{c30:x} level={level} owner+0x284=0 -- SetState5 (AUTOSAVES) ***",
            base + CONTINUE_CONFIRM_RVA
        ));
        timeline_event(
            "T_fullread_confirm",
            n,
            format_args!("c30=0x{c30:x} level={level}"),
        );
        unsafe { confirm(shim_ptr) };
        append_autoload_debug(format_args!(
            "native-fullread: continue_confirm returned -- native pump now streams the real world (#{n}) -> DONE"
        ));
        FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
        return;
    }
}

/// OWN-THE-STEPPER step 2 (the load driver): runs IN-CONTEXT at idx10 (STEP_MenuJobWait,
/// rcx=owner, rdx=FD4Time) as a real FD4 step. After letting the boot settle to the
/// stable press-any-button state, it drives the game's OWN load: SetState(3=BeginTitle)
/// builds the Continue/Load menu + sets GameMan+0xc30 to the most-recent saved map, then
/// the native Continue confirm 0x140b0e180 (via a {[+8]=owner} shim) does slot-select +
/// child-request + SetState(5=PlayGame). The native pump then loads the world, SKIPPING
/// the entire variable UI -- no input, no menu traversal.
pub(crate) unsafe extern "system" fn own_stepper_idx10(owner: usize, framectx: usize) {
    let n = OWN_STEPPER_CALLS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
    let base = OWN_STEPPER_BASE.load(Ordering::SeqCst);
    let phase = OWN_STEPPER_PHASE.load(Ordering::SeqCst);
    let gm = unsafe { *((base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) as *const usize) };
    let read_gm = |off: usize| {
        if gm != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((gm + off) as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        }
    };
    let c30 = read_gm(GAME_MAN_SAVED_MAP_C30_OFFSET);
    let b80 = read_gm(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
    let pass_through = |force_log: bool| {
        if force_log || n % OWN_STEPPER_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
            append_autoload_debug(format_args!(
                "own_stepper: pass-through #{n} phase={phase} owner=0x{owner:x} c30=0x{c30:x} framectx=0x{framectx:x}"
            ));
        }
        let orig = OWN_STEPPER_ORIG_IDX10.load(Ordering::SeqCst);
        if orig != TITLE_OWNER_SCAN_START_ADDRESS {
            let f: unsafe extern "system" fn(usize, usize) = unsafe { std::mem::transmute(orig) };
            unsafe { f(owner, framectx) };
        }
    };
    let want_slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    // OBSERVE-ONLY NATIVE-LOAD mode (gated OFF by default). Takes precedence over ALL the
    // own_stepper forcing logic below: it does NOT force the title machine -- the native boot
    // advances naturally via pass-through, and once the live menu is rendered + settled we fire
    // the native Load-Game node's run exactly once, then keep observing so the golden oracle is
    // written as the native pump loads the char. Pure read-only until the one-shot fire.
    // OBSERVE-ONLY NATIVE FULL-SAVE-READ mode (gated OFF by default). Takes precedence over ALL the
    // own_stepper forcing logic below AND over native_load: it does NOT force the title machine --
    // the native boot advances naturally via pass-through, and once the live menu is rendered +
    // settled it runs the full-save-read load chain (SUBMIT -> DRAIN -> DESER -> GUARD -> CONFIRM)
    // at the LIVE menu (where the FD4 IO worker pool is live so the submit drains). The sole save
    // write (continue_confirm -> SetState5) is HARD-gated behind the step-6 guard AND the commit
    // sub-gate (default = VERIFY-ONLY). NO SetState forcing for boot, NO selector pump.
    if native_fullread_enabled() {
        unsafe { native_fullread_tick(owner, base, n) };
        pass_through(false);
        return;
    }
    if native_load_enabled() {
        unsafe { native_load_tick(owner, base, n) };
        pass_through(false);
        return;
    }
    let read_iodev = || {
        let iodev = unsafe { *((base + IODEV_GLOBAL_RVA) as *const usize) };
        if iodev != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe {
                (
                    *((iodev + IODEV_INFLIGHT_10_OFFSET) as *const usize),
                    *((iodev + IODEV_REQHANDLE_18_OFFSET) as *const usize),
                    *((iodev + IODEV_REQHANDLE_20_OFFSET) as *const usize),
                )
            }
        } else {
            (
                TITLE_OWNER_SCAN_START_ADDRESS,
                TITLE_OWNER_SCAN_START_ADDRESS,
                TITLE_OWNER_SCAN_START_ADDRESS,
            )
        }
    };
    // SAVE-SAFE world-res streaming-driver cold-build probe (gated OFF by default; one-shot).
    // Builds the CSEmkResManImp driver (0x143d7c088) + stream worker (0x144842d40) at the parked
    // title via the CSResStep getter with a stub `this` -- NO SetState, NO world load, NO save
    // write. Validates emk-resman-streaming-driver-coldbuild-stub-lever-2026 live. Additive: the
    // normal phase logic continues (default = stay at the open menu, save-safe).
    if worldres_coldbuild_probe_enabled() && n >= OWN_STEPPER_SETTLE_CALLS {
        unsafe { worldres_coldbuild_probe(base) };
    }
    // DECISIVE save-data experiment (gated OFF by default; SAVE-SAFE). Register the stream worker,
    // then drive the cold b80 save-IO mount (preview -> poll to b80==3 -> deserialize) so 0x67b290
    // mounts the real char to memory -- NO SetState, NO save write. Bypasses the menu drive while
    // active; pass-through keeps the title ticking so the scheduler ticks the registered worker.
    if cold_char_mount_enabled() && n >= OWN_STEPPER_SETTLE_CALLS {
        unsafe { cold_char_mount_drive(base, gm, want_slot, n) };
        pass_through(false);
        return;
    }
    if phase == OWN_STEPPER_PHASE_MENU {
        // Drive once the boot settles. want_slot == -1 is the "most-recent" intent (resolved
        // from the dialog's natural highlight at PHASE_S2_ACTIVATE), NOT a "do nothing" signal,
        // so we no longer gate on it -- the own-stepper trigger file is itself the drive intent.
        if n < OWN_STEPPER_SETTLE_CALLS {
            pass_through(false);
            return;
        }
        // Wait for any startup MessageBoxDialog (connection-error / EULA / warning) to be
        // dismissed BEFORE driving the menu -- otherwise we build the main menu underneath the
        // live modal (black screen / contention). The auto-accept capture clears this atomic
        // once the dialog is gone (vtable no longer matches), so the own-stepper proceeds then.
        if CONNECTION_ERROR_DIALOG.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS {
            if n % OWN_STEPPER_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                append_autoload_debug(format_args!(
                    "own_stepper: waiting for startup modal to clear (CONNECTION_ERROR_DIALOG=0x{:x}) before menu drive #{n}",
                    CONNECTION_ERROR_DIALOG.load(Ordering::SeqCst)
                ));
            }
            pass_through(false);
            return;
        }
        // NO-WRITE CHECKPOINT. Path A (b78-route) is RUNTIME-FALSIFIED
        // (pathA-b78-route-falsified-b80-stuck-latch-gate-2026): disp2 0x140afb880's b78-route
        // is gated by the title-accept latch [0x143d856a0] (SET by load time -> disp2 bails to
        // cleanup every frame), so GameMan+0xb80 never leaves 0 and the native PlayGame
        // defaults to a NEW-GAME null character (which autosaved over the live slot in the
        // Seamless run). Every hand-driven b80 lever (cold slot-int primitives, b72 lever,
        // b78-route) hits the SAME wall: b80 reaches 3 ONLY when the native MoveMapListStep
        // async job pumps the menu deserialize 0x14082c240; FD4 stream-worker registration
        // alone does NOT advance b80 (0x140af1b40 registers the same task 0x144842d40 under the
        // same key 0x59682f01 as the in-game 0x140b0a980 milestone lever-c already tried with
        // b80 still 0). So idx10 NO LONGER SetState(5)s -- it stays at the title (NO save
        // write) pending the Path B menu-drive (drive the selector-owner step 0x140826d50 /
        // native Load-Game menu entry so the native async job mounts c30=real before PlayGame).
        // STAGE 1 (NO-WRITE layout verification + zero-input main-menu build). The parked
        // press-any-button title is the FIRST state 10 and has NOT run BeginTitle, so
        // owner+0x138 holds only intro items, not Continue/Load. (1) Walk the bare tree and
        // log it to VERIFY the live FD4 SBO pointer-vector layout against the static RE
        // (the captured recipe pointers were suspiciously low -- verify before any invoke).
        // (2) Build the main menu zero-input via SetState(owner, 3=BeginTitle): BeginTitle
        // needs no session and writes NO save (it is a menu-UI build), so this is save-safe;
        // it is exactly what the native press does after BeginLogo. The next frames run
        // BeginTitle (populating Continue/Load into owner+0x138) then return to state 10,
        // where PHASE_MENU_BUILD walks + identifies the Load-Game leaf. Stage 2 (invoke its
        // +0xa8 functor -> drive the dialog -> native mount) follows once this confirms the
        // live layout + item. Every hand-driven b80 lever is dead (the menu async job is the
        // only thing that mounts c30 before PlayGame); this is the Path B menu-drive.
        // T0: the common timeline start -- the title is parked at state 10 and we begin the
        // DLL drive. The first timeline_event sets the wall-clock epoch (so all later ms= are
        // measured from here); a native-baseline observe run sets T0 the same way.
        timeline_event(
            "T0",
            n,
            format_args!("owner=0x{owner:x} state10 slot={want_slot} c30=0x{c30:x}"),
        );
        // PASSIVE mode: do NOT force the menu. Hand off to PHASE_MENU_BUILD which waits for the
        // user to navigate to Load Game (surfacing d180 via the capture hooks), then runs STAGE 2.
        if own_stepper_passive_enabled() {
            append_autoload_debug(format_args!(
                "own_stepper: PASSIVE -- not forcing the menu; waiting for the user to open Load Game so d180 is captured, then STAGE 2 drives the load (input UNBLOCKED) #{n}"
            ));
            OWN_STEPPER_MENU_BUILD_WAITS.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_MENU_BUILD, Ordering::SeqCst);
            pass_through(false);
            return;
        }
        let bare = unsafe { diagnostic_menu_walk(owner, base, "bare", true) };
        let bare_tree = unsafe {
            diagnostic_job_tree_walk(
                owner,
                base,
                TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
                "bare-tree",
                true,
            )
        };
        // STAGE 1c: build the FULL main menu by replicating the engine's OWN press path.
        // The parked press-any-button screen is the FIRST state 10; the native press handler
        // 0x140b0b6b0 issues SetState(owner,2)=BeginLogo, after which the native pump advances
        // 2->3->10 and builds the Continue / Load-Game(d180) / New-Game items into the CSMenu
        // registry at owner+0xe0. The registry update 0x1409aac10 then ticks EVERY registered
        // entry each frame, so our menu-item Update hook (functor_chain_hits_factory) will
        // capture d180. SetState(3)=BeginTitle ALONE (skipping BeginLogo) only built the
        // BackScreen (runtime: only c000 ticked), so we drive the full sequence. BeginLogo(2)
        // hard-asserts session singleton 0x144588e98 at entry -- read it live; SetState(2) only
        // when non-null, else fall back to SetState(3). Save-safe either way: BeginLogo/BeginTitle
        // are menu-UI builds with NO save write (only SetState(5)/PlayGame writes).
        let session = unsafe { safe_read_usize(base + SESSION_SINGLETON_144588E98_RVA) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let target_state = if session != TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_STEP_BEGIN_LOGO
        } else {
            TITLE_STEP_BEGIN_TITLE
        };
        // CRITICAL: STEP_BeginLogo builds the main-menu list (Continue/Load d180/...) into
        // owner+0xe0 via 0x14081f180 ONLY when [owner+0xb8]==0; if set it short-circuits to
        // SetState(3) and skips the build (bd mainmenu-item-builder-into-iterator-tree-2026) --
        // which is why our prior SetState(2) only produced the 3 title-composition items. Clear
        // the gate so BeginLogo runs the full build (zero-input, menu-UI only -> save-safe).
        let beginlogo_gate =
            unsafe { safe_read_usize(owner + TITLE_OWNER_BEGINLOGO_LIST_GATE_B8_OFFSET) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        if target_state == TITLE_STEP_BEGIN_LOGO {
            unsafe {
                *((owner + TITLE_OWNER_BEGINLOGO_LIST_GATE_B8_OFFSET) as *mut u32) =
                    TITLE_OWNER_BEGINLOGO_GATE_CLEAR;
            }
        }
        let set_state: unsafe extern "system" fn(usize, i32) =
            unsafe { std::mem::transmute(base + TITLE_SET_STATE_RVA) };
        unsafe { set_state(owner, target_state) };
        OWN_STEPPER_MENU_BUILD_WAITS.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
        OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_MENU_BUILD, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "own_stepper: STAGE1c bare-walk done (load_game_138=0x{:x} load_game_tree=0x{:x}) session(0x144588e98)=0x{session:x} beginlogo_gate(0xb8)=0x{beginlogo_gate:x} -> SetState({target_state}) [{}] to build the FULL main menu zero-input (#{n}) slot={want_slot} gm=0x{gm:x} c30=0x{c30:x} b80={b80}",
            bare.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
            bare_tree.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
            if target_state == TITLE_STEP_BEGIN_LOGO {
                "BeginLogo 2->3->10 full menu"
            } else {
                "BeginTitle fallback (session null)"
            }
        ));
        // Suppress unused warnings for consts/statics retained from the falsified cold
        // slot-int drive, synthetic-dispatcher, b78-route, and Continue-shim work.
        let _ = (
            invoke_menu_item_functor as usize,
            CONTINUE_CONFIRM_RVA,
            B80_FULL_LOAD_INITIATOR_RVA,
            OWN_STEPPER_PHASE_MOUNT,
            OWN_STEPPER_PHASE_DRIVE,
            OWN_STEPPER_PHASE_CONTINUE,
            B80_DISPATCHER1_RVA,
            B80_DISPATCHER2_RVA,
            SYNTH_MMS_SKIP_APPLY_12A_OFFSET,
            SYNTH_MMS_DESER_SLOT_12C_OFFSET,
            SYNTH_MMS_SKIP_APPLY_ON,
            OWN_STEPPER_DRIVE_MAX,
            OWN_STEPPER_SHIM_OWNER_IDX,
            OWN_STEPPER_MOUNT_POLL_MAX,
            OWN_STEPPER_B80_RESIDENT,
            OWN_STEPPER_B80_PREVIEW_LANE,
            OWN_STEPPER_B80_IDLE,
            B80_POLL_RVA,
            B80_POLL_ARG_ZERO,
            B80_LANE1_DRIVER_RVA,
            B80_LOAD_SAVE_DATA_INITIATOR_RVA,
            DESERIALIZE_SLOT_RVA,
            LOAD_INITIATOR_RVA,
            WORLD_WORKER_BUILD_RVA,
            WORLD_STREAM_WORKER_RVA,
            WORLD_WORKER_BUILD_STATE,
            SYNTHETIC_STEP_STATE_OFFSET,
            FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA,
            GAME_MAN_REQUESTED_SLOT_B78_OFFSET,
            GAME_MAN_ARM_FLAG_B72_OFFSET,
            TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET,
            TITLE_OWNER_PLAY_GAME_SLOT_OFFSET,
            DEFAULT_PLAY_GAME_MAP,
            TITLE_STEP_PLAY_GAME,
            &raw const OWN_STEPPER_SHIM,
            &raw const SYNTH_MMS_OWNER,
            &raw mut OWN_STEPPER_WORKER_THIS,
            &OWN_STEPPER_DRIVE_CALLS,
            &OWN_STEPPER_MOUNT_POLLS,
        );
        let _ = read_iodev;
        pass_through(false);
        return;
    }
    if phase == OWN_STEPPER_PHASE_MENU_BUILD {
        let waits =
            OWN_STEPPER_MENU_BUILD_WAITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
        // SetState(2)=BeginLogo re-enters the online check and pops the connection-error
        // MessageBoxDialog, and the online attempt RE-FIRES on every menu transition -> the modal
        // LOOPS (a fresh dialog each transition). force_dismiss_startup_dialog presses OK every
        // frame (OnDecide 0x140927ba0 with +0x25e0=OK -> proceeds, NOT the force-stop cancel that
        // reverted to press-any-button). Because the popup LOOPS, blocking here until
        // CONNECTION_ERROR_DIALOG is clear waited FOREVER and STAGE 1d never ran. So: only block
        // during the initial GRACE window (let the first boot/SetState(2) modal get dismissed),
        // then PROCEED to menu nav + d180 capture EVEN WITH a modal present -- OnDecide-OK clears
        // each one every frame, and the underlying main menu / capture hooks run concurrently.
        if waits < OWN_STEPPER_MODAL_GRACE {
            if waits % OWN_STEPPER_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                append_autoload_debug(format_args!(
                    "own_stepper: PHASE_MENU_BUILD grace {waits}/{OWN_STEPPER_MODAL_GRACE} (dialog=0x{:x}) before menu open -- OnDecide-OK dismisses each looping popup",
                    CONNECTION_ERROR_DIALOG.load(Ordering::SeqCst)
                ));
            }
            pass_through(false);
            return;
        }
        // ZERO-INPUT d180 LOCATE (replaces the old simulated-input cursor nav, which wrote the
        // keystate bitmap inputmgr+0x90 to move the cursor onto Load-Game -- that is synthesized
        // input and VIOLATES the No-Compromises zero-input standard). SetState(2)->3->10 builds the
        // main-menu job tree; the Load-Game item d180 (a MenuWindowJob whose +0xa8 functor's
        // _Do_call chains to dialog_factory 0x14081ead0) is constructed into the tree at BUILD time,
        // so a pure-read recursive walk can surface it WITHOUT the pump ticking it and WITHOUT any
        // input. A user-driven capture (2026-06-17) pinned d180's functor object = {_Func_impl
        // vtable 0x142ac3ea8, captured owner+0x138}; the factory reads [capture+8]=owner+0x138 as
        // the dialog owner. We walk the candidate holder roots and, on the first functor->factory
        // hit, latch the item into MENU_LOAD_GAME_ITEM so STAGE 2 drives the load. (The
        // cap_menu_item_update hook also sets it if d180 ever ticks; whichever fires first wins.)
        // Throttled; pure reads -> save-safe.
        const D180_ROOT_E0: usize = 0xe0;
        const D180_ROOT_130: usize = 0x130;
        const D180_ROOT_138: usize = 0x138;
        // d180's +0xa8 functor object = {_Func_impl vtable base+0x2ac3ea8, capture[+8]=owner+0x138}
        // (user-driven capture 2026-06-17) -- a strong fingerprint corroborating the functor->factory
        // classification.
        const MENU_ITEM_LOADGAME_FUNCTOR_VTABLE_RVA: usize = 0x02ac3ea8;
        if !own_stepper_passive_enabled()
            && !input_probe_enabled()
            && MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS
            && waits >= STAGE1D_SETTLE_WAITS
            && (waits - STAGE1D_SETTLE_WAITS) % STAGE1D_RETRY_INTERVAL
                == TITLE_OWNER_SCAN_START_ADDRESS as u64
        {
            // Walk the candidate roots; on the first functor->dialog_factory hit (= the Load-Game
            // item d180), validate its fingerprint and LATCH it into MENU_LOAD_GAME_ITEM. STAGE 2
            // then drives it via the NATIVE MenuWindowJob::Update 0x1407ad1c0 (which wires the ctx
            // item+0x10 from the descriptor item+0x58 before firing the functor -> NO synthetic
            // ctx, NO save write). The cap_menu_item_update hook also sets it if d180 ever ticks;
            // whichever fires first wins. Throttled; pure reads here (save-safe).
            const ITEM_FUNCTOR_A8: usize = 0xa8;
            const ITEM_CTX_10: usize = 0x10;
            const ITEM_RESULT_130: usize = 0x130;
            let verbose = OWN_STEPPER_TITLETOP_DUMPS
                .fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
                < OWN_STEPPER_TITLETOP_DUMP_CAP;
            let roots = [D180_ROOT_E0, D180_ROOT_130, D180_ROOT_138];
            for &root in roots.iter() {
                if let Some(item) =
                    unsafe { diagnostic_job_tree_walk(owner, base, root, "d180-locate", verbose) }
                {
                    let null = TITLE_OWNER_SCAN_START_ADDRESS;
                    let functor =
                        unsafe { safe_read_usize(item + ITEM_FUNCTOR_A8) }.unwrap_or(null);
                    let fvt = if functor != null {
                        unsafe { safe_read_usize(functor) }.unwrap_or(null)
                    } else {
                        null
                    };
                    let fcap = if functor != null {
                        unsafe { safe_read_usize(functor + core::mem::size_of::<usize>()) }
                            .unwrap_or(null)
                    } else {
                        null
                    };
                    let ctx10 = unsafe { safe_read_usize(item + ITEM_CTX_10) }.unwrap_or(null);
                    let res130 = unsafe { safe_read_usize(item + ITEM_RESULT_130) }.unwrap_or(null);
                    MENU_LOAD_GAME_ITEM.store(item, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "own_stepper: ZERO-INPUT d180 LOCATED item=0x{item:x} via owner+0x{root:x} functor=0x{functor:x} fvt=0x{fvt:x}(want base+0x{:x}) fcap=0x{fcap:x}(want owner+0x138=0x{:x}) ctx10=0x{ctx10:x} result130=0x{res130:x} -- latched, STAGE2 will native-Update it",
                        MENU_ITEM_LOADGAME_FUNCTOR_VTABLE_RVA,
                        owner.wrapping_add(D180_ROOT_138)
                    ));
                    break;
                }
            }
        }
        // STAGE 1d: open the main menu zero-input. SetState(2)->3->10 built the TitleTopDialog at
        // owner+0xe0 (vt 0x142b26468). The dialog's native update 0x1409aac10 (ticked every frame
        // by pass_through -> STEP_MenuJobWait) runs the intro FadeIn animation, transitions
        // FadeIn->Loop on anim-complete (NOT input), and on its NON-INPUT Loop-ready path
        // (0x1409aade8) calls the open-menu registrar 0x1409b24e0 ITSELF, which set_state's the
        // SM [dialog+0xa60] to "TextFadeOut" and registers Continue/Load(d180)/New-Game. So the
        // PRIMARY path is to do NOTHING and let the native update self-open the menu.
        //
        // The prior force-call was harmful (bd titletopdialog-loop-ready-gate-2026): firing the
        // registrar on bare flags>=2 fired from the FadeIn node (wrong state) AND set the latch
        // [dialog+0xa40]=1, which PERMANENTLY blocks the native non-input path (it needs latch==0).
        // So here we (a) READ-ONLY probe the live state by NAME via the game's own is_in_state
        // (FadeIn/Loop/TextFadeOut) + the latch, logging it; and (b) only as a FALLBACK self-fire
        // the registrar on the CORRECT gate -- is_in_state(Loop)==true && latch==0 -- which is
        // exactly the native path's own precondition (zero input, NO save write). If the native
        // path fires first (latch->1 in Loop) we simply observe the menu open.
        const MENU_JOB_HOLDER_E0: usize = 0xe0;
        if MENU_ENTRIES_SEEN.load(Ordering::SeqCst) == MENU_ENTRIES_SEEN_NO
            && waits >= STAGE1D_SETTLE_WAITS
        {
            let dialog = unsafe { safe_read_usize(owner + MENU_JOB_HOLDER_E0) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let dialog_vt = if dialog != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(dialog) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            };
            // Only call into the dialog's FD4 state machine once owner+0xe0 IS the TitleTopDialog.
            if dialog_vt == base + TITLE_TOP_DIALOG_VTABLE_RVA {
                // is_in_state receiver = the ADDRESS dialog+0xa60 (the embedded SM sub-object), per
                // the registrar's `add rcx,0xa60; call`. is_in_state(sm, desc) -> bool reads the
                // live state by name (no hand pointer-chase). Read-only / no side effects.
                let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
                let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
                    unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
                let in_fadein = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_FADEIN_RVA) }
                    != OWN_STEPPER_FALSE;
                let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) }
                    != OWN_STEPPER_FALSE;
                let in_textfadeout =
                    unsafe { is_in_state(sm, base + TITLE_STATE_DESC_TEXTFADEOUT_RVA) }
                        != OWN_STEPPER_FALSE;
                let latch =
                    unsafe { safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET) }
                        .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
                        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                if (waits - STAGE1D_SETTLE_WAITS) % STAGE1D_RETRY_INTERVAL
                    == TITLE_OWNER_SCAN_START_ADDRESS as u64
                {
                    append_autoload_debug(format_args!(
                        "own_stepper: STAGE1d probe dialog=0x{dialog:x} sm=0x{sm:x} fadein={in_fadein} loop={in_loop} textfadeout={in_textfadeout} latch={latch} waits={waits} (self-fire open-menu on Loop+latch-clear)"
                    ));
                }
                // SELF-FIRE the open-menu registrar on the CORRECT gate (the native path's own
                // precondition: settled in Loop + latch clear). RUNTIME-PROVEN NECESSARY
                // (headless-load 2026-06-17): with the modal suppressed (online-disable), the
                // TitleTopDialog SM sits in Loop forever -- the Loop-ready predicate needs the
                // accept byte (input), which never comes headless (latch=0 for 3000 waits). So the
                // "native self-opens" assumption is FALSE for a clean offline boot; we must fire
                // 0x1409b24e0 ourselves (the zero-input-menu-open milestone proved this opens the
                // menu). Default ON now (no flag) since headless cannot rely on a button press;
                // gated to the correct state (in_loop, NOT FadeIn) + once + latch-clear so it can
                // neither corrupt the SM (titletopdialog-fadein-gate) nor double-fire.
                if in_loop
                    && latch == TITLE_OWNER_SCAN_START_ADDRESS
                    && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) == OWN_STEPPER_MENU_OPENED_NO
                {
                    let open_menu: unsafe extern "system" fn(usize) =
                        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_OPEN_MENU_RVA) };
                    unsafe { open_menu(dialog) };
                    OWN_STEPPER_MENU_OPENED.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                    // Deterministic timing endpoint: the DLL has driven boot -> modal-skip ->
                    // past press-any-button -> a READY main menu with ZERO input. ms-from-T0 here
                    // is the headless boot-to-menu time (the part vanilla needs >=3 human inputs +
                    // an online-attempt timeout to reach).
                    timeline_event(
                        "T_menu_open",
                        n,
                        format_args!("dialog=0x{dialog:x} waits={waits}"),
                    );
                    append_autoload_debug(format_args!(
                        "own_stepper: STAGE1d self-fire open-menu 0x{:x}(dialog=0x{dialog:x}) -- in Loop + latch clear (correct gate, zero-input) waits={waits}",
                        base + TITLE_TOP_DIALOG_OPEN_MENU_RVA
                    ));
                }
            }
        }
        // DETERMINISTIC INPUT PROBE: once the menu is open, drive a frame-precise Down->Confirm
        // (targeted input as a MEASUREMENT oracle) and short-circuit the zero-input locate/STAGE2
        // path -- the injected Confirm drives the native load; idx6 watches it. Answers whether the
        // d180 leaf ticks on highlight alone (so the zero-input functor-invoke route is viable).
        if input_probe_enabled()
            && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO
        {
            unsafe { menu_input_probe(owner, base) };
            pass_through(false);
            return;
        }
        // INJECT-NAV instrument-capture: self-drive the cursor with synthesized menu-DOWN while
        // the user's input stays blocked. The menu is KEYBOARD-navigated under Proton (XInput is
        // not polled), so the primary vehicle is the DInput keyboard block, into which we stamp
        // DIK_DOWN on the schedule (InputBlocker::set_injected_key); the gamepad button state is
        // also published for the XInput hook in case a controller is present. This runs every
        // frame (unlike the XInput hook). Capture-only: DOWN nav, never Confirm -> no load/write.
        if inject_nav_enabled()
            && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO
        {
            let nf = INJECT_NAV_FRAME.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            let buttons = inject_nav_buttons(nf);
            INJECT_NAV_CUR_BUTTONS.store(buttons as usize, Ordering::SeqCst);
            let dik = if buttons != INJECT_NAV_NO_BUTTONS {
                DIK_DOWN
            } else {
                DIK_NONE
            };
            InputBlocker::get_instance().set_injected_key(dik);
            // Find the cursor offset by observing it across the ONE deterministic Down: snapshot
            // before (cursor=0), diff after it settles (cursor=1). The 0->1 dword IS the cursor.
            if nf as usize == CURSOR_PROBE_BASELINE_FRAME {
                unsafe { cursor_offset_probe(owner, base, true) };
            } else if nf as usize == CURSOR_PROBE_POSTDOWN_FRAME {
                unsafe { cursor_offset_probe(owner, base, false) };
            }
            if dik != DIK_NONE {
                let lc = INJECT_NAV_LOG_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                if lc < INJECT_NAV_LOG_FIRST {
                    append_autoload_debug(format_args!(
                        "inject-nav: frame={nf} menu-DOWN asserted (DIK=0x{dik:x} wButtons=0x{buttons:x})"
                    ));
                }
            }
            pass_through(false);
            return;
        }
        // 2026-06-18 MODEL B LIVE-DIALOG (gated, OFF by default). SIBLING to direct_build: instead
        // of FORGING a non-live dialog (which loads the wrong map + crashes), locate the REAL
        // Load-Game registry node and fire its NATIVE run 0x1409aaba0 -> a LIVE registered
        // ProfileLoadDialog the native menu group pumps. own_stepper_live_dialog_fire latches the
        // fire (one-shot), waits for the live dialog at owner+0xe0, then routes to STAGE2 ACTIVATE
        // (load_activate + char-fingerprint-gated continue_confirm). Fail-closed at every step.
        // Checked BEFORE direct_build so enabling live-dialog takes the live path, not the forge.
        if live_dialog_enabled()
            && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO
        {
            unsafe { own_stepper_live_dialog_fire(owner, base, waits) };
            pass_through(false);
            return;
        }
        // 2026-06-18 DIRECT BUILD (gated, OFF by default). Once the menu is open, build the
        // ProfileLoadDialog DIRECTLY (factory 0x14081ead0) -- bypassing the input-gated row
        // controller that never constructs headless -- then drive STAGE 2 (mount + guarded
        // continue_confirm). One-shot + fail-closed (validates r8 read-only before the native
        // call). A plain (un-gated) run skips this and stays the safe read-only scan below.
        if direct_build_enabled()
            && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO
            && OWN_STEPPER_DIRECT_BUILT.load(Ordering::SeqCst) == OWN_STEPPER_DIRECT_BUILT_NO
            && waits >= STAGE1D_SETTLE_WAITS
        {
            unsafe { own_stepper_direct_build(owner, base) };
            pass_through(false);
            return;
        }
        // SAFE DEFAULT (RTTI-corrected, 2026-06-17). The "title-confirm" menu-drive below was built
        // on a MISIDENTIFIED function: 0x14078e1c0 is CommandSelectDialog::Update (an in-game
        // dialog), NOT the TitleTopDialog (owner+0xe0, RTTI vt 0x142b26468) confirm router, so its
        // cursor [+0xb0c] / rows [+0x1290] offsets do not apply here (bd rtti-correction-...). It is
        // now DEMOTED behind legacy_menu_drive_enabled(). A plain own_stepper run must NOT take that
        // wrong route -- it reaches the open menu zero-input and STAYS there (no fire, no SetState,
        // save-safe). The real headless Load path is the own-the-stepper / session-activation route,
        // not driving these fake-menu steppers.
        if OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO
            && !own_stepper_passive_enabled()
            && !legacy_menu_drive_enabled()
            && !input_probe_enabled()
            && !inject_nav_enabled()
        {
            if OWN_STEPPER_TITLE_FIRED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
                == TITLE_OWNER_SCAN_START_ADDRESS
            {
                append_autoload_debug(format_args!(
                    "own_stepper: menu open zero-input; disproven title-confirm menu-drive is gated OFF (RTTI-corrected) -- STAY at open menu (NO-WRITE). Set er-effects-legacy-disproven-menu-drive.txt to revisit the dead path."
                ));
            }
            // 2026-06-18 RECON-ONLY fingerprint scan for the Load-Game entry, run HERE (the open-menu
            // park is where a plain own_stepper run actually lives -- the dump block further down is
            // unreachable behind this early return). Result discarded -> no latch into
            // MENU_LOAD_GAME_ITEM, no STAGE2 advance -> stays NO-WRITE. Dedicated cap/interval so it
            // logs a handful of times across the ~20s post-open window without spamming.
            if OWN_STEPPER_LOADGAME_SCANS.load(Ordering::SeqCst) < OWN_STEPPER_LOADGAME_SCAN_CAP
                && (waits % STAGE1D_RETRY_INTERVAL) == TITLE_OWNER_SCAN_START_ADDRESS as u64
            {
                OWN_STEPPER_LOADGAME_SCANS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                let _ = unsafe { scan_dialog_for_loadgame(owner, base) };
            }
            if waits >= OWN_STEPPER_MENU_BUILD_WAIT_MAX {
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            pass_through(false);
            return;
        }
        // LEGACY / DISPROVEN title-confirm Load -- gated behind legacy_menu_drive_enabled() (OFF by
        // default). Built on titletop-confirm-route-static-validated-no-input-needed-2026, which RTTI
        // later REFUTED (0x14078e1c0 = CommandSelectDialog::Update). fire_titletop_load_entry is
        // self-validating so it fail-closes on the wrong object, but it is the WRONG layer entirely;
        // kept only to revisit the dead path deliberately. Never the default.
        if OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO
            && !own_stepper_passive_enabled()
            && legacy_menu_drive_enabled()
        {
            let null = TITLE_OWNER_SCAN_START_ADDRESS;
            let dialog = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }
                .unwrap_or(null);
            let pld_vt = base + PROFILE_LOAD_DIALOG_VTABLE_RVA;
            let cur_vt = if dialog != null {
                unsafe { safe_read_usize(dialog) }.unwrap_or(null)
            } else {
                null
            };
            if cur_vt == pld_vt {
                // The fired Load-Game action already built the ProfileLoadDialog at owner+0xe0.
                OWN_STEPPER_DIALOG.store(dialog, Ordering::SeqCst);
                OWN_STEPPER_S2_WAITS.store(null, Ordering::SeqCst);
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_S2_ACTIVATE, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "own_stepper: title-confirm built ProfileLoadDialog=0x{dialog:x} at owner+0xe0 -- entering STAGE2 ACTIVATE (slot={want_slot})"
                ));
                pass_through(false);
                return;
            }
            if OWN_STEPPER_TITLE_FIRED.load(Ordering::SeqCst) == null {
                // Not yet fired: attempt the validated fire (fail-closed no-op + retry if the rows
                // are not realized yet -- never writes on a non-realized/contaminated state).
                if unsafe { fire_titletop_load_entry(dialog, base) } {
                    OWN_STEPPER_TITLE_FIRED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                }
                pass_through(false);
                return;
            }
            // Fired; waiting for the ProfileLoadDialog to appear at owner+0xe0. Bounded timeout.
            if waits >= OWN_STEPPER_MENU_BUILD_WAIT_MAX {
                append_autoload_debug(format_args!(
                    "own_stepper: title-confirm fired but ProfileLoadDialog not at owner+0xe0 after {waits} waits -- STAY (NO-WRITE)"
                ));
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            pass_through(false);
            return;
        }
        // Wait for the registered entries to tick: the menu-item Update hook + Sequence-iterator
        // hook capture the Load-Game leaf (functor->dialog_factory) as the native pump ticks
        // them. Fallback: our static tree walk. NO SetState here -> stays at the main menu,
        // save-safe. STAGE 2 (invoke the leaf functor) follows once the live item is confirmed.
        // (REFUTED d180-locate path, retained only for the input-probe/inject-nav diagnostic modes.)
        let hooked = MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst);
        // The real Continue/Load-Game rows are TitleTopDialog entries (NOT FD4 jobs). Once the
        // menu is open, sample the dialog's entry vector a few times as it realizes -- save-safe
        // read-only enumeration that identifies the Load-Game/Continue entries for STAGE 2.
        if OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO
            && OWN_STEPPER_TITLETOP_DUMPS.load(Ordering::SeqCst) < OWN_STEPPER_TITLETOP_DUMP_CAP
            && (waits % STAGE1D_RETRY_INTERVAL) == TITLE_OWNER_SCAN_START_ADDRESS as u64
        {
            OWN_STEPPER_TITLETOP_DUMPS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            let (tt_load, tt_cont, tt_cursor) = unsafe { dump_titletop_menu_entries(owner, base) };
            append_autoload_debug(format_args!(
                "own_stepper: STAGE1b titletop-entries load_game=0x{:x} continue=0x{:x} cursor={tt_cursor} (entries are dialog rows, not FD4 jobs)",
                tt_load.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
                tt_cont.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            ));
        }
        // Search BOTH the owner+0x130 BeginLogo commit target (where the main-menu list with d180
        // actually lands, per the commit fn 0x140b0e530) AND owner+0xe0 (the dialog holder).
        let found = if hooked != TITLE_OWNER_SCAN_START_ADDRESS {
            Some(hooked)
        } else {
            unsafe {
                diagnostic_job_tree_walk(
                    owner,
                    base,
                    TITLE_OWNER_MENU_LIST_130_OFFSET,
                    "list130",
                    false,
                )
            }
            .or_else(|| unsafe {
                diagnostic_job_tree_walk(
                    owner,
                    base,
                    TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
                    "built-tree",
                    false,
                )
            })
        };
        match found {
            Some(item) => {
                let _ = unsafe { diagnostic_menu_walk(owner, base, "built-138", true) };
                let _ = unsafe {
                    diagnostic_job_tree_walk(
                        owner,
                        base,
                        TITLE_OWNER_MENU_LIST_130_OFFSET,
                        "list130",
                        true,
                    )
                };
                let _ = unsafe {
                    diagnostic_job_tree_walk(
                        owner,
                        base,
                        TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
                        "built-tree",
                        true,
                    )
                };
                // Ensure MENU_LOAD_GAME_ITEM is set (the item may have come from the static
                // tree walk rather than the leaf/iterator hook) so STAGE 2 reads it.
                if MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS {
                    MENU_LOAD_GAME_ITEM.store(item, Ordering::SeqCst);
                }
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE1b LOAD-GAME item identified=0x{item:x} after {waits} waits -- entering STAGE 2 load drive (slot={want_slot}) c30=0x{c30:x} b80={b80}"
                ));
                timeline_event(
                    "T_menu_built",
                    n,
                    format_args!("item=0x{item:x} c30=0x{c30:x}"),
                );
                OWN_STEPPER_S2_WAITS.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_S2_INVOKE, Ordering::SeqCst);
            }
            None => {
                if waits >= OWN_STEPPER_MENU_BUILD_WAIT_MAX && !own_stepper_passive_enabled() {
                    let _ = unsafe { diagnostic_menu_walk(owner, base, "built138-timeout", true) };
                    let _ = unsafe {
                        diagnostic_job_tree_walk(
                            owner,
                            base,
                            TITLE_OWNER_MENU_LIST_130_OFFSET,
                            "list130-timeout",
                            true,
                        )
                    };
                    let _ = unsafe {
                        diagnostic_job_tree_walk(
                            owner,
                            base,
                            TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
                            "built-tree-timeout",
                            true,
                        )
                    };
                    append_autoload_debug(format_args!(
                        "own_stepper: STAGE1b menu-build TIMEOUT after {waits} waits -- Load-Game item not found; staying at title (NO-WRITE)"
                    ));
                    OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
                }
            }
        }
        pass_through(false);
        return;
    }
    if phase == OWN_STEPPER_PHASE_S2_INVOKE
        || phase == OWN_STEPPER_PHASE_S2_ACTIVATE
        || phase == OWN_STEPPER_PHASE_S2_MOUNT_POLL
        || phase == OWN_STEPPER_PHASE_S2_CONFIRM
    {
        // STAGE 2: drive the verified menu load (functor -> dialog -> load_activate -> native
        // pump mounts c30=real+ac0+char -> continue_confirm -> SetState(5)). Pass-through each
        // frame so STEP_MenuJobWait keeps the native menu task ticking the registered selector.
        unsafe { own_stepper_stage2(owner, base, gm, want_slot, n, framectx) };
        pass_through(false);
        return;
    }
    // phase DONE: idx6 watches the native load; idx10 just passes through if re-entered.
    pass_through(false);
}

/// STAGE 2 in-context load drive (see the lib.rs STAGE-2 const block). Runs each frame while
/// `OWN_STEPPER_PHASE` is one of the four S2 phases, sequencing:
///   INVOKE  -> hand-fire d180's `+0xa8` functor to build the ProfileLoadDialog
///   ACTIVATE-> write slot cursor `[dialog+0xb0c]=N`, call vtable-slot-20 `load_activate(dialog)`
///   MOUNT_POLL -> let the native pump tick the selector; detect the mount (`ac0==N` + io
///               request set->cleared); latch the real `c30`
///   CONFIRM -> guard (`ac0==N && c30==latched`) then `continue_confirm` -> SetState(5)
/// Every cross-into-game call is gated by read-only preconditions; the ONLY save-write risk is
/// the CONFIRM SetState(5), gated entirely by a verified real mount (fail-closed otherwise:
/// stay at the menu, NO SetState(5), NO save write).
unsafe fn own_stepper_stage2(
    owner: usize,
    base: usize,
    gm: usize,
    want_slot: i32,
    n: u64,
    framectx: usize,
) {
    const S2_LOG_INTERVAL: u64 = 30;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let phase = OWN_STEPPER_PHASE.load(Ordering::SeqCst);
    let waits = OWN_STEPPER_S2_WAITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
    let item = MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst);
    let pld_vt = base + PROFILE_LOAD_DIALOG_VTABLE_RVA;
    // 32-bit GameMan field read (low dword of the 8-byte safe read; little-endian).
    let ri32 = |addr: usize, dflt: i32| -> i32 {
        unsafe { safe_read_usize(addr) }
            .map(|v| v as u32 as i32)
            .unwrap_or(dflt)
    };
    let c30 = if gm != null {
        ri32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET, GAME_MAN_C30_UNSET)
    } else {
        GAME_MAN_C30_UNSET
    };
    let ac0 = if gm != null {
        ri32(
            gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET,
            OWN_STEPPER_SLOT_NONE,
        )
    } else {
        OWN_STEPPER_SLOT_NONE
    };
    let b80 = if gm != null {
        ri32(
            gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET,
            OWN_STEPPER_B80_IDLE,
        )
    } else {
        OWN_STEPPER_B80_IDLE
    };
    let iodev = unsafe { safe_read_usize(base + IODEV_GLOBAL_RVA) }.unwrap_or(null);
    let (io10, io18, io20) = if iodev != null {
        (
            unsafe { safe_read_usize(iodev + IODEV_INFLIGHT_10_OFFSET) }.unwrap_or(null),
            unsafe { safe_read_usize(iodev + IODEV_REQHANDLE_18_OFFSET) }.unwrap_or(null),
            unsafe { safe_read_usize(iodev + IODEV_REQHANDLE_20_OFFSET) }.unwrap_or(null),
        )
    } else {
        (null, null, null)
    };
    // A dialog candidate is valid iff its vtable == ProfileLoadDialog.
    let valid_dialog =
        |d: usize| -> bool { d != null && unsafe { safe_read_usize(d) }.unwrap_or(null) == pld_vt };

    if phase == OWN_STEPPER_PHASE_S2_INVOKE {
        if item == null {
            if waits >= OWN_STEPPER_S2_PHASE_MAX {
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE2-INVOKE-TIMEOUT no item after {waits} waits -- STAGE2-NOWRITE-ABORT"
                ));
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            return;
        }
        let dlg130 =
            unsafe { safe_read_usize(item + MENU_ITEM_DIALOG_RESULT_130_OFFSET) }.unwrap_or(null);
        let ctx10 = unsafe { safe_read_usize(item + MENU_ITEM_CTX_10_OFFSET) }.unwrap_or(null);
        let functor =
            unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }.unwrap_or(null);
        // If the native pump already built the dialog (focused on Load), use it.
        let existing = if valid_dialog(dlg130) {
            dlg130
        } else if valid_dialog(ctx10) {
            ctx10
        } else {
            null
        };
        if existing != null {
            OWN_STEPPER_DIALOG.store(existing, Ordering::SeqCst);
            timeline_event(
                "T_dialog",
                n,
                format_args!("dialog=0x{existing:x} via=native"),
            );
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-INVOKE-OK (native-built) dialog=0x{existing:x} dvt=0x{pld_vt:x} item=0x{item:x}"
            ));
            OWN_STEPPER_S2_WAITS.store(null, Ordering::SeqCst);
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_S2_ACTIVATE, Ordering::SeqCst);
            return;
        }
        // Let the opened menu settle, then drive d180's NATIVE Update once to build its dialog.
        // d180 lives at owner+0x130 under an input-gated IfElseJob branch (its case child is never
        // bound headless), so the native pump never ticks it -- but the item is fully built, so
        // calling its own MenuWindowJob::Update 0x1407ad1c0 (which wires the ctx item+0x10 from the
        // descriptor item+0x58 then fires the functor) builds the ProfileLoadDialog with a NATIVE
        // ctx (no synthesis) and zero input. Build-only; idempotent; no save write.
        if waits < OWN_STEPPER_S2_INVOKE_SETTLE {
            return;
        }
        if OWN_STEPPER_INVOKED.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS as usize {
            let ret = unsafe { drive_menu_item_update(item, base, framectx) }.unwrap_or(null);
            OWN_STEPPER_INVOKED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            let dlg130b = unsafe { safe_read_usize(item + MENU_ITEM_DIALOG_RESULT_130_OFFSET) }
                .unwrap_or(null);
            let ctx10b = unsafe { safe_read_usize(item + MENU_ITEM_CTX_10_OFFSET) }.unwrap_or(null);
            let candidate = if valid_dialog(ret) {
                ret
            } else if valid_dialog(dlg130b) {
                dlg130b
            } else if valid_dialog(ctx10b) {
                ctx10b
            } else {
                null
            };
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-INVOKE hand-fired item=0x{item:x} functor=0x{functor:x} ret=0x{ret:x} dlg130(pre=0x{dlg130:x},post=0x{dlg130b:x}) ctx10(pre=0x{ctx10:x},post=0x{ctx10b:x}) candidate=0x{candidate:x}"
            ));
            if candidate != null {
                // Mirror native bookkeeping: stash the built dialog at item+0x130 if empty so a
                // later native leaf-Update does not re-build it.
                if dlg130b == null {
                    unsafe {
                        *((item + MENU_ITEM_DIALOG_RESULT_130_OFFSET) as *mut usize) = candidate;
                    }
                }
                OWN_STEPPER_DIALOG.store(candidate, Ordering::SeqCst);
                timeline_event(
                    "T_dialog",
                    n,
                    format_args!("dialog=0x{candidate:x} via=invoke"),
                );
                OWN_STEPPER_S2_WAITS.store(null, Ordering::SeqCst);
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_S2_ACTIVATE, Ordering::SeqCst);
                return;
            }
        }
        if waits >= OWN_STEPPER_S2_PHASE_MAX {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-INVOKE-TIMEOUT dialog not built after {waits} waits -- STAGE2-NOWRITE-ABORT"
            ));
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
        }
        return;
    }

    if phase == OWN_STEPPER_PHASE_S2_ACTIVATE {
        let dialog = OWN_STEPPER_DIALOG.load(Ordering::SeqCst);
        if !valid_dialog(dialog) {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-ACTIVATE invalid dialog=0x{dialog:x} -- STAGE2-NOWRITE-ABORT"
            ));
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        // PlayerGameData must be non-null (load_activate asserts it).
        let pgd = unsafe { safe_read_usize(base + PLAYER_GAME_DATA_SINGLETON_RVA) }.unwrap_or(null);
        if pgd == null {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-ACTIVATE PlayerGameData null -- STAGE2-NOWRITE-ABORT"
            ));
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        let dvt = unsafe { safe_read_usize(dialog) }.unwrap_or(null);
        let bound = ri32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET, OWN_STEPPER_SLOT_NONE);
        let cursor_now = ri32(
            dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET,
            OWN_STEPPER_SLOT_NONE,
        );
        // Resolve the target slot: a configured slot=N (>=0), else (slot=-1 = "most-recent")
        // the dialog's NATURAL highlight cursor -- so we never need to know which slot holds a
        // character up front, and we never overwrite the user's most-recent highlight.
        let target = if want_slot == OWN_STEPPER_SLOT_NONE {
            cursor_now
        } else {
            want_slot
        };
        if target < OWN_STEPPER_SLOT_ZERO || (bound > OWN_STEPPER_SLOT_ZERO && target >= bound) {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-ACTIVATE invalid slot want={want_slot} target={target} cursor={cursor_now} bound={bound} dialog=0x{dialog:x} -- STAGE2-NOWRITE-ABORT (no chars / wrong profile?)"
            ));
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        // For a fixed slot, write the cursor (UI state, not a save write); for most-recent,
        // leave the dialog's own highlight untouched.
        if want_slot != OWN_STEPPER_SLOT_NONE {
            unsafe {
                *((dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) as *mut i32) = want_slot;
            }
        }
        OWN_STEPPER_EXPECTED_SLOT.store(target, Ordering::SeqCst);
        let lav =
            unsafe { safe_read_usize(dvt + DIALOG_LOAD_ACTIVATE_VTSLOT_A0_OFFSET) }.unwrap_or(null);
        if lav == null {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-ACTIVATE load_activate slot null dvt=0x{dvt:x} -- STAGE2-NOWRITE-ABORT"
            ));
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        let activate: unsafe extern "system" fn(usize) -> u8 = unsafe { std::mem::transmute(lav) };
        let r = unsafe { activate(dialog) };
        // load_activate builds + validates the selector (its job is to wire the dialog/ctx). We do
        // NOT build or self-pump the transient selector step 0x140826510/0x140826d50 anymore: that
        // step is a transient stack/queue object never stored back to the dialog, so pumping it out
        // of context CRASHES the game (~795 ticks, process_exited) and never drives GameMan+0xb80
        // (probe-12-isolation-selector-pump-crashes-regressed-from-direct-submit-drain). The MOUNT
        // phase now uses the PROVEN cold_char_mount_drive direct submit+drain+poll+deserialize
        // sequence (SAVE-DATA-HALF-SOLVED) instead. ctx is read here only for the log evidence.
        const DIALOG_LOADJOBCTX_1CC8: usize = 0x1cc8;
        let ctx = unsafe { safe_read_usize(dialog + DIALOG_LOADJOBCTX_1CC8) }.unwrap_or(null);
        let ctx_vt = if ctx != null {
            unsafe { safe_read_usize(ctx) }.unwrap_or(null)
        } else {
            null
        };
        OWN_STEPPER_SELECTOR_STEP.store(null, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "own_stepper: STAGE2-ACTIVATE want={want_slot} target={target} cursor_now={cursor_now} bound={bound} lav=0x{lav:x} ret={r} dialog=0x{dialog:x} ctx=0x{ctx:x} ctx_vt=0x{ctx_vt:x} io18=0x{io18:x} io20=0x{io20:x} -- selector self-pump DISABLED; MOUNT via direct submit+drain+deser"
        ));
        // Reset the shared mount latches so the MOUNT phase's delegate (cold_char_mount_drive) and
        // the mount-done gate observe a clean slate for this drive.
        OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_NOT_FIRED, Ordering::SeqCst);
        OWN_STEPPER_MOUNT_C30.store(GAME_MAN_C30_UNSET, Ordering::SeqCst);
        OWN_STEPPER_IO_WAS_SET.store(OWN_STEPPER_IO_WAS_SET_NO, Ordering::SeqCst);
        OWN_STEPPER_S2_WAITS.store(null, Ordering::SeqCst);
        OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_S2_MOUNT_POLL, Ordering::SeqCst);
        return;
    }

    if phase == OWN_STEPPER_PHASE_S2_MOUNT_POLL {
        // MOUNT via the PROVEN direct submit+drain+poll+deserialize sequence (SAVE-DATA-HALF-SOLVED:
        // b80->3, deser ret==1, real char loads cold). We DELEGATE to cold_char_mount_drive, which
        // runs the exact working calls -- slot-mgr peek 0x678a50, ACTIVATE-byte, build+register the
        // stream worker, set_save_slot 0x14067a810, PREVIEW 0x67b4e0 -> LANE-drive 0x679510 to b80==0
        // -> LoadSaveData 0x67b200 (b80=2) -> POLL 0x679180 to b80==3 (RESIDENT) -> deserialize
        // 0x67b290 (writes c30 from the save header + applies the real char). It publishes the result
        // via OWN_STEPPER_DESER_FIRED / OWN_STEPPER_MOUNT_C30. We do NOT self-pump the transient
        // selector step 0x140826d50 anymore -- that crashed the game and never drove b80
        // (probe-12-isolation-selector-pump-crashes-regressed-from-direct-submit-drain).
        unsafe { cold_char_mount_drive(base, gm, want_slot, n) };
        // io18/io20 both non-null => the request was started; latch it.
        if io18 != null && io20 != null {
            OWN_STEPPER_IO_WAS_SET.store(OWN_STEPPER_IO_WAS_SET_YES, Ordering::SeqCst);
        }
        let io_was_set =
            OWN_STEPPER_IO_WAS_SET.load(Ordering::SeqCst) == OWN_STEPPER_IO_WAS_SET_YES;
        let io_consumed = io18 == null && io20 == null;
        let expected = OWN_STEPPER_EXPECTED_SLOT.load(Ordering::SeqCst);
        // Mount signal = the deserialize 0x67b290 SUCCEEDED (ret==1), which proves it wrote c30 from
        // the save header + applied the real char. c30 itself is ambiguous (the char's real early map
        // 0xa010000 collides with the new-game default), so the reliable signal is deser-success +
        // a SANE latched c30 (not the unset sentinel, not zero). (setstate5-is-save-safe-c30-from-save)
        const C30_ZERO: i32 = 0;
        let _ = (io_was_set, io_consumed);
        let latched_c30 = OWN_STEPPER_MOUNT_C30.load(Ordering::SeqCst);
        let deser_state = OWN_STEPPER_DESER_FIRED.load(Ordering::SeqCst);
        let deser_ok = deser_state == OWN_STEPPER_DESER_FIRED_OK;
        let deser_done = deser_state != OWN_STEPPER_DESER_NOT_FIRED;
        let c30_sane = latched_c30 != GAME_MAN_C30_UNSET && latched_c30 != C30_ZERO;
        let mount_done =
            deser_ok && c30_sane && ac0 == expected && expected != OWN_STEPPER_SLOT_NONE;
        if waits % S2_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 || deser_done {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-MOUNT-POLL waits={waits} ac0={ac0} expected={expected} c30=0x{c30:x} latched=0x{latched_c30:x} deser_ok={deser_ok} c30_sane={c30_sane} b80={b80} io18=0x{io18:x} io20=0x{io20:x}"
            ));
        }
        // VERIFY-ONLY: stop at the deserialize. We log c30 + the char fingerprint and go to DONE --
        // NO continue_confirm / NO SetState5 / NO save write. (The CONFIRM phase below stays in the
        // code but is no longer reachable from this verify-only mount.)
        if deser_done {
            let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
            timeline_event(
                "T_mount",
                n,
                format_args!("ac0={ac0} c30=0x{latched_c30:x} waits={waits}"),
            );
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-MOUNT-VERIFY deser_ok={deser_ok} mount_done={mount_done} ac0={ac0} expected={expected} c30=0x{c30:x} latched=0x{latched_c30:x} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) b80={b80} -- VERIFY-ONLY (NO SetState5/NO save write) -> DONE"
            ));
            OWN_STEPPER_S2_WAITS.store(null, Ordering::SeqCst);
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
        } else if waits >= OWN_STEPPER_S2_PHASE_MAX {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-MOUNT-POLL-TIMEOUT ac0={ac0} want={want_slot} c30=0x{c30:x} io_was_set={io_was_set} after {waits} waits -- STAGE2-NOWRITE-ABORT (stay at menu)"
            ));
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
        }
        return;
    }

    if phase == OWN_STEPPER_PHASE_S2_CONFIRM {
        let latched = OWN_STEPPER_MOUNT_C30.load(Ordering::SeqCst);
        let expected = OWN_STEPPER_EXPECTED_SLOT.load(Ordering::SeqCst);
        // HARD save-write guard: only SetState(5) when the real char is still mounted. Require the
        // deserialize SUCCEEDED (ret==1 -> c30 written from save), c30 unchanged since the mount and
        // not the unset sentinel, and the slot matches. (setstate5-is-save-safe-c30-from-save)
        const DESER_FIRED_OK_CONFIRM: usize = 2;
        let deser_ok = OWN_STEPPER_DESER_FIRED.load(Ordering::SeqCst) == DESER_FIRED_OK_CONFIRM;
        // CHAR-FINGERPRINT gate (MODEL B): SetState(5) ONLY when a REAL character is mounted in
        // PlayerGameData (level>=1 AND a non-empty name) -- NOT on c30 (the ambiguous m10_01
        // collision the wrong-map crash rode in on). This is the decisive save-write guard: a
        // new-game default has level 0 / empty name, so it fail-closes (NO SetState5, NO write).
        let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
        let proceed = deser_ok
            && fp_real
            && ac0 == expected
            && expected != OWN_STEPPER_SLOT_NONE
            && c30 == latched
            && c30 != GAME_MAN_C30_UNSET;
        if !proceed {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-CONFIRM-GUARD-FAIL ac0={ac0} expected={expected} c30=0x{c30:x} latched=0x{latched:x} deser_ok={deser_ok} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) -- STAGE2-NOWRITE-ABORT"
            ));
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        if OWN_STEPPER_CONFIRMED.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS as usize {
            let shim = &raw mut OWN_STEPPER_SHIM;
            unsafe { (*shim)[OWN_STEPPER_SHIM_OWNER_IDX] = owner };
            let shim_ptr = shim as usize;
            let confirm: unsafe extern "system" fn(usize) =
                unsafe { std::mem::transmute(base + CONTINUE_CONFIRM_RVA) };
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-CONFIRM-GUARD-PASS ac0={ac0} c30=0x{c30:x} -> continue_confirm shim=0x{shim_ptr:x} owner=0x{owner:x}"
            ));
            timeline_event("T_playgame", n, format_args!("ac0={ac0} c30=0x{c30:x}"));
            unsafe { confirm(shim_ptr) };
            OWN_STEPPER_CONFIRMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-SETSTATE5 fired owner=0x{owner:x} -- native pump now streams the real world"
            ));
        }
        OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
    }
}

/// CHAR-FINGERPRINT save-write gate: returns (is_real, level, name_len) by reading the live
/// CS::PlayerGameData (GameDataMan `[base+0x3d5df38]` -> +0x08 -> PlayerGameData), the validated
/// reading (the same chain dump_load_correctness uses). A REAL mounted character has level>=1 AND
/// a non-empty 16-bit name; a new-game default has level 0 / empty name. Pure fault-tolerant
/// safe_read_usize -> never faults. Used to FAIL-CLOSED SetState(5): the c30 oracle is ambiguous
/// (m10_01 collision), so the character actually present in PlayerGameData is the decisive signal.
unsafe fn char_fingerprint(base: usize) -> (bool, u32, usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const ZERO_U16: u16 = 0;
    const ZERO_U32: u32 = 0;
    const U16_STRIDE: usize = 2;
    const IDX_START: usize = 0;
    const IDX_STEP: usize = 1;
    const MIN_REAL_LEVEL: u32 = 1;
    const NAME_LEN_NONE: usize = 0;
    let gdm = unsafe { safe_read_usize(base + PLAYER_GAME_DATA_SINGLETON_RVA) }.unwrap_or(NULL);
    let pgd = if gdm != NULL {
        unsafe { safe_read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if pgd == NULL {
        return (false, ZERO_U32, NAME_LEN_NONE);
    }
    let level = unsafe { safe_read_usize(pgd + PGD_LEVEL_68_OFFSET) }
        .map(|v| v as u32)
        .unwrap_or(ZERO_U32);
    let mut name_len = NAME_LEN_NONE;
    while name_len < PGD_NAME_LEN_U16 {
        let u = unsafe { safe_read_usize(pgd + PGD_NAME_9C_OFFSET + name_len * U16_STRIDE) }
            .map(|v| v as u16)
            .unwrap_or(ZERO_U16);
        if u == ZERO_U16 {
            break;
        }
        name_len += IDX_STEP;
    }
    let _ = IDX_START;
    let is_real = level >= MIN_REAL_LEVEL && name_len > NAME_LEN_NONE;
    (is_real, level, name_len)
}

/// Read the load-correctness invariants at the in-world transition and log a single greppable
/// `LOAD-CORRECTNESS` record: GameMan c30/ac0/name_is_empty + the CS::PlayerGameData
/// (`[base+0x4588268]`) character fingerprint (name, level, runes, rune-memory, chr_type,
/// 8-stat block). A native-menu load and a DLL-driven load produce comparable records;
/// correctness == field-for-field match (name non-empty, level/runes/stats equal). Pure reads,
/// fault-tolerant; safe to call once at the first in-world frame.
pub(crate) unsafe fn dump_load_correctness(base: usize, frame: u64) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const BAD_I32: i32 = -1;
    const ZERO_U16: u16 = 0;
    const ZERO_U32: u32 = 0;
    const NAME_UNKNOWN: u8 = 0xff;
    const U16_STRIDE: usize = 2;
    const U32_STRIDE: usize = 4;
    const IDX_START: usize = 0;
    const IDX_STEP: usize = 1;
    let gm = unsafe { safe_read_usize(base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) }.unwrap_or(NULL);
    let ri32 = |addr: usize| -> i32 {
        unsafe { safe_read_usize(addr) }
            .map(|v| v as u32 as i32)
            .unwrap_or(BAD_I32)
    };
    let ru32 = |addr: usize| -> u32 {
        unsafe { safe_read_usize(addr) }
            .map(|v| v as u32)
            .unwrap_or(ZERO_U32)
    };
    let (c30, ac0, name_empty) = if gm != NULL {
        (
            ri32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET),
            ri32(gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET),
            unsafe { safe_read_usize(gm + GAME_MAN_NAME_IS_EMPTY_E70_OFFSET) }
                .map(|v| v as u8)
                .unwrap_or(NAME_UNKNOWN),
        )
    } else {
        (BAD_I32, BAD_I32, NAME_UNKNOWN)
    };
    // [0x144588268] -> GameDataMan; PlayerGameData (the save data) = [GameDataMan + 0x08].
    let gdm = unsafe { safe_read_usize(base + PLAYER_GAME_DATA_SINGLETON_RVA) }.unwrap_or(NULL);
    let pgd = if gdm != NULL {
        unsafe { safe_read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if pgd == NULL {
        append_autoload_debug(format_args!(
            "LOAD-CORRECTNESS frame={frame} pgd=NULL gm_c30=0x{c30:x} gm_ac0={ac0} name_empty={name_empty}"
        ));
        return;
    }
    let level = ru32(pgd + PGD_LEVEL_68_OFFSET);
    let runes = ru32(pgd + PGD_RUNE_COUNT_6C_OFFSET);
    let rune_mem = ru32(pgd + PGD_RUNE_MEMORY_70_OFFSET);
    let chr_type = ru32(pgd + PGD_CHR_TYPE_98_OFFSET);
    // character_name: up to 17 UTF-16LE units, to the first NUL.
    let mut name_units = [ZERO_U16; PGD_NAME_LEN_U16];
    let mut i = IDX_START;
    while i < PGD_NAME_LEN_U16 {
        name_units[i] = unsafe { safe_read_usize(pgd + PGD_NAME_9C_OFFSET + i * U16_STRIDE) }
            .map(|v| v as u16)
            .unwrap_or(ZERO_U16);
        i += IDX_STEP;
    }
    let mut nlen = IDX_START;
    while nlen < PGD_NAME_LEN_U16 && name_units[nlen] != ZERO_U16 {
        nlen += IDX_STEP;
    }
    let name = String::from_utf16(&name_units[..nlen]).unwrap_or_default();
    let mut stats = [ZERO_U32; PGD_STAT_COUNT];
    let mut s = IDX_START;
    while s < PGD_STAT_COUNT {
        stats[s] = ru32(pgd + PGD_STAT_BASE_3C_OFFSET + s * U32_STRIDE);
        s += IDX_STEP;
    }
    append_autoload_debug(format_args!(
        "LOAD-CORRECTNESS frame={frame} gm_c30=0x{c30:x} gm_ac0={ac0} name_empty={name_empty} pgd=0x{pgd:x} chr_type={chr_type} name={name:?} level={level} runes={runes} rune_mem={rune_mem} stats={stats:?}"
    ));
}

/// OWN-THE-STEPPER idx6 (STEP_GameStepWait) handler: runs IN-CONTEXT after idx10's
/// placeholder SetState(5) builds the MoveMapStep, whose native update 0x140aff640 ticks
/// the b80 dispatchers (disp1 0x140afbad0 + disp2 0x140afb880). idx6 does NOT call the
/// deserialize itself -- it keeps the b78-route armed (re-plant GameMan+0xb78=slot, clear
/// b72, only while b80 is idle) so the NATIVE disp2 b78-route initiates and disp1
/// deserializes the real slot into GameMan+0xc30. When c30 turns real, idx6 re-targets
/// owner+0xbc to that map and SetState(5) ONCE so the load streams the character's real
/// world instead of the m60 placeholder. Pass-through (watch+log) otherwise.
pub(crate) unsafe extern "system" fn own_stepper_idx6(owner: usize, framectx: usize) {
    let base = OWN_STEPPER_BASE.load(Ordering::SeqCst);
    let phase = OWN_STEPPER_PHASE.load(Ordering::SeqCst);
    let gm = unsafe { *((base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) as *const usize) };
    let csfeman = unsafe { *((base + CSFEMAN_SINGLETON_RVA) as *const usize) };
    let read_gm = |off: usize| {
        if gm != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((gm + off) as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        }
    };
    let b80 = read_gm(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
    let c30 = read_gm(GAME_MAN_SAVED_MAP_C30_OFFSET);
    let n = OWN_STEPPER_IDX6_CALLS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
    let pass6 = || {
        let orig = OWN_STEPPER_ORIG_IDX6.load(Ordering::SeqCst);
        if orig != TITLE_OWNER_SCAN_START_ADDRESS {
            let f: unsafe extern "system" fn(usize, usize) = unsafe { std::mem::transmute(orig) };
            unsafe { f(owner, framectx) };
        }
    };
    let _ = phase;
    // NO-WRITE CHECKPOINT. The Path A re-target (re-plant b78 / re-SetState(5) on c30=real)
    // is REMOVED: it MISFIRED on the native new-game default c30=0xa010000 and reloaded an
    // m10 null character (pathA-b78-route-falsified-b80-stuck-latch-gate-2026). idx10 no
    // longer SetState(5)s, so this idx6 (state 6) is not reached in normal flow; it remains a
    // pure read-only watcher (no writes) for any future in-context load comparison.
    let _ = (
        &OWN_STEPPER_RETARGETED,
        OWN_STEPPER_RETARGET_NO,
        OWN_STEPPER_RETARGET_YES,
        OWN_STEPPER_SLOT_NONE,
        OWN_STEPPER_B80_IDLE,
        GAME_MAN_C30_UNSET,
        DEFAULT_PLAY_GAME_MAP,
        GAME_MAN_REQUESTED_SLOT_B78_OFFSET,
        GAME_MAN_ARM_FLAG_B72_OFFSET,
        TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET,
        TITLE_OWNER_PLAY_GAME_SLOT_OFFSET,
        TITLE_SET_STATE_RVA,
        TITLE_STEP_PLAY_GAME,
        &OWN_STEPPER_SLOT,
    );
    // WATCH the native load that the idx10 Continue confirm kicked off (state 6
    // GameStepWait). Mirrors the observe snapshot so the in-context load can be compared
    // directly to the real user-driven load: csfeman + MoveMapStep build, mms_state
    // advance (1 MsbLoad -> 2 MsbLoadWait -> 3 WorldResWait), b80 deserialize, c30 -> real
    // map, resmgr + b7c1 (the streaming-enable the real flow sets natively at mms_state=2).
    if n % OWN_STEPPER_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
        let ac0 = read_gm(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
        let ingame = unsafe { *((owner + TITLE_OWNER_JOB_OFFSET) as *const usize) };
        let mms = if ingame != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((ingame + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) as *const usize) }
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let mms_state = if mms != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((mms + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        };
        let wrm = if mms != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET) as *const usize) }
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let resmgr = if wrm != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((wrm + WORLDRES_RESMGR_10_OFFSET) as *const usize) }
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let b7c1 = if resmgr != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((resmgr + RESMGR_STREAM_ENABLE_B7C1_OFFSET) as *const u8) as i32 }
        } else {
            TITLE_STATE_OWNER_GONE
        };
        append_autoload_debug(format_args!(
            "own_stepper: idx6 watch #{n} csfeman=0x{csfeman:x} c30=0x{c30:x} ac0={ac0} b80={b80} mms=0x{mms:x} mms_state={mms_state} resmgr=0x{resmgr:x} b7c1={b7c1}"
        ));
    }
    pass6();
}

/// Patch the writable .data idx10 step-fn slot to our handler once the FE-host is at
/// committed state 10. Same thread as the dispatch (game-task), so no race.
pub(crate) unsafe fn own_stepper_patch_once(module_base: usize) {
    if OWN_STEPPER_PATCHED.load(Ordering::SeqCst) != OWN_STEPPER_PATCHED_NO {
        return;
    }
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        return;
    };
    let owner = owner as usize;
    if unsafe { *((owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) }
        != TITLE_STEP_MENU_JOB_WAIT
    {
        return;
    }
    // Optional slot override from the trigger file ("slot=N"); -1/absent => the game's
    // own most-recent selection.
    if let Some(dir) = game_directory_path() {
        if let Ok(content) = std::fs::read_to_string(dir.join("er-effects-own-stepper.txt")) {
            for line in content.lines() {
                if let Some(rest) = line.trim().strip_prefix("slot=") {
                    if let Ok(v) = rest.trim().parse::<i32>() {
                        OWN_STEPPER_SLOT.store(v, Ordering::SeqCst);
                    }
                }
            }
        }
    }
    let slot = module_base + TITLE_STEP_IDX10_SLOT_RVA;
    let orig = unsafe { *(slot as *const usize) };
    OWN_STEPPER_ORIG_IDX10.store(orig, Ordering::SeqCst);
    OWN_STEPPER_BASE.store(module_base, Ordering::SeqCst);
    // Own idx6 (STEP_GameStepWait) too, for the post-SetState(5) deserialize + re-target.
    let slot6 = module_base + TITLE_STEP_IDX6_SLOT_RVA;
    let orig6 = unsafe { *(slot6 as *const usize) };
    OWN_STEPPER_ORIG_IDX6.store(orig6, Ordering::SeqCst);
    unsafe { *(slot6 as *mut usize) = own_stepper_idx6 as usize };
    unsafe { *(slot as *mut usize) = own_stepper_idx10 as usize };
    OWN_STEPPER_PATCHED.store(OWN_STEPPER_PATCHED_YES, Ordering::SeqCst);
    let handler = own_stepper_idx10 as usize;
    let _ = TITLE_STEP_PLAY_GAME;
    append_autoload_debug(format_args!(
        "own_stepper: PATCHED idx10 slot=0x{slot:x} orig=0x{orig:x} -> handler=0x{handler:x} owner=0x{owner:x}"
    ));
}

/// Pure read-only observation (NO forcing, NO SetState) of the title -> menu -> load
/// transition. Logs a full snapshot every OBSERVE_INTERVAL ticks so we can capture
/// exactly what the REAL button press does: the title state sequence, when CSFeMan /
/// session build, when the save mounts (GameMan+0xc30 changes from the default), the
/// InGameStep/MoveMapStep appearance. Ground-truths the menu-build the static RE
/// kept mis-identifying.
pub(crate) unsafe fn title_observe_tick(module_base: usize, tick: u64) {
    let _ = OBSERVE_INTERVAL;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let owner = unsafe { title_owner(module_base) }.map(|p| p as usize);
    let state = match owner {
        Some(o) => unsafe { *((o + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) },
        None => TITLE_STATE_OWNER_GONE,
    };
    // Title->menu timing baseline (works for BOTH a true-vanilla user run and the DLL run):
    // T0 = first frame parked at the title (state 10); T_menu_open = when the TitleTopDialog SM
    // reaches TextFadeOut (menu open -- by the user's presses+modal-dismissals in vanilla). The
    // delta is the apples-to-apples title->ready-menu time to compare against the DLL's headless
    // 3.1s. Read-only (is_in_state is a pure state query).
    if state == TITLE_STEP_MENU_JOB_WAIT
        && owner.is_some()
        && OBSERVE_T0_EMITTED.swap(OBSERVE_MARKER_EMITTED, Ordering::SeqCst)
            == OBSERVE_MARKER_NOT_EMITTED
    {
        timeline_event("T0", tick, format_args!("state10 observe-baseline"));
    }
    if let Some(o) = owner {
        if OBSERVE_MENU_OPEN_EMITTED.load(Ordering::SeqCst) == OBSERVE_MARKER_NOT_EMITTED {
            let dialog =
                unsafe { safe_read_usize(o + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
            let dialog_vt = if dialog != null {
                unsafe { safe_read_usize(dialog) }.unwrap_or(null)
            } else {
                null
            };
            if dialog_vt == module_base + TITLE_TOP_DIALOG_VTABLE_RVA {
                let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
                let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
                    unsafe { std::mem::transmute(module_base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
                let textfadeout =
                    unsafe { is_in_state(sm, module_base + TITLE_STATE_DESC_TEXTFADEOUT_RVA) }
                        != OWN_STEPPER_FALSE;
                if textfadeout
                    && OBSERVE_MENU_OPEN_EMITTED.swap(OBSERVE_MARKER_EMITTED, Ordering::SeqCst)
                        == OBSERVE_MARKER_NOT_EMITTED
                {
                    timeline_event(
                        "T_menu_open",
                        tick,
                        format_args!("dialog=0x{dialog:x} observe-baseline"),
                    );
                }
            }
        }
    }
    let csfeman = unsafe { *((module_base + CSFEMAN_SINGLETON_RVA) as *const usize) };
    let session = unsafe { *((module_base + SESSION_SINGLETON_RVA) as *const usize) };
    let gm = unsafe { *((module_base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) as *const usize) };
    let read_gm = |off: usize| {
        if gm != null {
            unsafe { *((gm + off) as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        }
    };
    let c30 = read_gm(GAME_MAN_SAVED_MAP_C30_OFFSET);
    let ac0 = read_gm(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
    let b80 = read_gm(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
    let b78 = read_gm(GAME_MAN_REQUESTED_SLOT_B78_OFFSET);
    // Frame-level save-IO orchestration capture (menu-b80-mount-orchestration-sequence):
    // the iodev request handle pair [iodev+0x18]/[iodev+0x20] + [iodev+0x10] inflight.
    // Only 0x14067b4e0's preview read populates these; logging them across a real
    // load pins EXACTLY when the read goes in-flight/resident vs when b80 flips.
    let iodev = unsafe { *((module_base + IODEV_GLOBAL_RVA) as *const usize) };
    let read_iodev = |off: usize| {
        if iodev != null {
            unsafe { *((iodev + off) as *const usize) }
        } else {
            null
        }
    };
    let iodev10 = read_iodev(IODEV_INFLIGHT_10_OFFSET);
    let iodev18 = read_iodev(IODEV_REQHANDLE_18_OFFSET);
    let iodev20 = read_iodev(IODEV_REQHANDLE_20_OFFSET);
    let ingame = match owner {
        Some(o) => unsafe { *((o + TITLE_OWNER_JOB_OFFSET) as *const usize) },
        None => null,
    };
    let mms = if ingame != null {
        unsafe { *((ingame + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) as *const usize) }
    } else {
        null
    };
    let mms_state = if mms != null {
        unsafe { *((mms + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) }
    } else {
        TITLE_STATE_OWNER_GONE
    };
    let slotmgr = unsafe { *((module_base + SLOT_MANAGER_RVA) as *const usize) };
    // World-resource streaming enable-state (the WorldResWait resolution gate):
    // resmgr = deref(deref(MoveMapStep+0xf0)+0x10); b7c1 = its streaming-enable flag;
    // driver = the streaming/session driver singleton 0x143d7c088. Capture what the
    // REAL load has enabled during mms_state=3 that our forced load lacks.
    let wrm = if mms != null {
        unsafe { *((mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET) as *const usize) }
    } else {
        null
    };
    let resmgr = if wrm != null {
        unsafe { *((wrm + WORLDRES_RESMGR_10_OFFSET) as *const usize) }
    } else {
        null
    };
    let b7c1 = if resmgr != null {
        unsafe { *((resmgr + RESMGR_STREAM_ENABLE_B7C1_OFFSET) as *const u8) as i32 }
    } else {
        TITLE_STATE_OWNER_GONE
    };
    let driver = unsafe { *((module_base + STREAMING_DRIVER_SINGLETON_RVA) as *const usize) };
    // Change-detection: only log when the signature changes (full granularity, no
    // per-frame file I/O). Captures every transition incl. the mms_state 3 -> resolve.
    let csf_nz = (csfeman != null) as i64;
    let sess_nz = (session != null) as i64;
    let ingame_nz = (ingame != null) as i64;
    let driver_nz = (driver != null) as i64;
    let mut sig = state as i64;
    sig = sig
        .wrapping_mul(OBSERVE_SIG_MULT)
        .wrapping_add(mms_state as i64);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(csf_nz);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(sess_nz);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(ingame_nz);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(c30 as i64);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(b80 as i64);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(ac0 as i64);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(b7c1 as i64);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(driver_nz);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(b78 as i64);
    sig = sig
        .wrapping_mul(OBSERVE_SIG_MULT)
        .wrapping_add((iodev10 != null) as i64);
    sig = sig
        .wrapping_mul(OBSERVE_SIG_MULT)
        .wrapping_add((iodev18 != null) as i64);
    sig = sig
        .wrapping_mul(OBSERVE_SIG_MULT)
        .wrapping_add((iodev20 != null) as i64);
    if OBSERVE_LAST_SIG.swap(sig, Ordering::SeqCst) == sig {
        return;
    }
    append_autoload_debug(format_args!(
        "observe: state={state} csfeman=0x{csfeman:x} session=0x{session:x} c30=0x{c30:x} ac0={ac0} b80={b80} b78={b78} iodev=0x{iodev:x} io10=0x{iodev10:x} io18=0x{iodev18:x} io20=0x{iodev20:x} mms=0x{mms:x} mms_state={mms_state} resmgr=0x{resmgr:x} b7c1={b7c1} driver=0x{driver:x} slotmgr=0x{slotmgr:x} tick={tick}"
    ));
}

pub(crate) fn submit_play_game_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_SUBMIT_PLAY_GAME").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-submit-play-game.txt")
        .exists()
}

/// Corrected native play-game submit (play-game-submit-and-continue-load-recipe-2026).
/// On the live FE-host SimpleTitleStep (committed state 10), replicate the Continue/
/// Load handler 0x140b0e180's load branch WITHOUT forcing state: set the slot, clear
/// the new-game flag owner+0x284, write a packed map to owner+0xbc, and call the
/// game's own SetState 0x140b0d960(owner, 5=PlayGame). The existing per-frame pump
/// then runs PlayGame -> child MoveMap_Init -> builds CSFeMan -> loads. Zero input.
/// (force_play_game wrote owner+0x4c=5 raw + a raw slot in +0xbc -> orphaned.)
pub(crate) unsafe fn submit_play_game_once(
    module_base: usize,
    slot: i32,
    tick: u64,
    task_data: &FD4TaskData,
) -> bool {
    if tick < TITLE_NATIVE_JOB_MIN_TICK {
        return false;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let csfeman = unsafe { *((module_base + CSFEMAN_SINGLETON_RVA) as *const usize) };
    let gm = unsafe { *((module_base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) as *const usize) };
    let read_c30 = || {
        if gm != null {
            unsafe { *((gm + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        }
    };
    let set_state: unsafe extern "system" fn(usize, i32) =
        unsafe { std::mem::transmute(module_base + TITLE_SET_STATE_RVA) };
    match SUBMIT_PLAY_GAME_PHASE.load(Ordering::SeqCst) {
        SUBMIT_PHASE_INIT => {
            // Phase A: deserialize slot N (CSFeMan-less at the title) to set its map,
            // then SetState(5)=PlayGame so the pump builds CSFeMan + the MoveMapStep.
            let Some(owner) = (unsafe { title_owner(module_base) }) else {
                return false;
            };
            let owner = owner as usize;
            if unsafe { *((owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) }
                != TITLE_STEP_MENU_JOB_WAIT
            {
                return false;
            }
            let set_save_slot: unsafe extern "system" fn(i32) =
                unsafe { std::mem::transmute(module_base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
            unsafe { set_save_slot(slot) };
            let deserialize: unsafe extern "system" fn(i32) =
                unsafe { std::mem::transmute(module_base + DESERIALIZE_SLOT_RVA) };
            unsafe { deserialize(slot) };
            let c30 = read_c30();
            unsafe {
                *((owner + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET) as *mut u8) =
                    MOVIE_SKIP_FLAG_CLEAR;
                *((owner + TITLE_OWNER_PLAY_GAME_SLOT_OFFSET) as *mut i32) = c30;
            }
            unsafe { set_state(owner, TITLE_STEP_PLAY_GAME) };
            SUBMIT_PLAY_GAME_PHASE.store(SUBMIT_PHASE_BUILT, Ordering::SeqCst);
            let _ = TITLE_STEP_BEGIN_TITLE;
            append_autoload_debug(format_args!(
                "submit_play_game: phaseA deserialize+SetState(5) slot={slot} c30=0x{c30:x} tick={tick}"
            ));
        }
        SUBMIT_PHASE_DESER => {
            SUBMIT_PLAY_GAME_PHASE.store(SUBMIT_PHASE_BUILT, Ordering::SeqCst);
        }
        SUBMIT_PHASE_BUILT => {
            // Phase C: close the two world-streaming gaps (worldres-loadstate-creator-
            // and-streaming-enable-gate-2026). Gap 1: the spawner built its block-load
            // request from [InGameStep+0x100], which held the wrong coord, so slot 9's
            // m10 load-states were never created -- set the real coord + re-submit via
            // 0x140aed820 so the builder creates them. Gap 2: world-res streaming is
            // disabled ([resmgr+0xb7c1]==0) -- call the virtual enabler 0x14066e2e4 to
            // set it + build the session singletons + start the IO job machine.
            if csfeman == null {
                return true;
            }
            let Some(owner) = (unsafe { title_owner(module_base) }) else {
                return true;
            };
            let owner = owner as usize;
            let ingame = unsafe { *((owner + TITLE_OWNER_JOB_OFFSET) as *const usize) };
            if ingame == null {
                return true;
            }
            let coord = read_c30();
            unsafe {
                *((ingame + INGAMESTEP_TARGET_COORD_100_OFFSET) as *mut i32) = coord;
            }
            // CORRECT resmgr = deref(deref(MoveMapStep+0xf0)+0x10), vtable 0x142a7e030
            // (NOT InGameStep+0x250, which is the WorldRes-OWNER, vtable 0x142a7de60 --
            // passing that was the prior crash).
            let mms = unsafe { *((ingame + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) as *const usize) };
            let wrm = if mms != null {
                unsafe { *((mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET) as *const usize) }
            } else {
                null
            };
            let resmgr = if wrm != null {
                unsafe { *((wrm + WORLDRES_RESMGR_10_OFFSET) as *const usize) }
            } else {
                null
            };
            // TIMING FIX: the resmgr only exists once the MoveMapStep has spun up
            // (~mms_state 2 in the real load). WAIT for it -- our prior attempts ran
            // at phaseC with resmgr=0x0 and silently skipped the enable.
            if resmgr == null {
                return true;
            }
            let resmgr_vt = unsafe { *(resmgr as *const usize) };
            let b7c1_before =
                unsafe { *((resmgr + RESMGR_STREAM_ENABLE_B7C1_OFFSET) as *const u8) as i32 };
            // Defensive: build the streaming/session driver singleton if somehow null
            // (it is normally built from boot).
            let driver_before =
                unsafe { *((module_base + STREAMING_DRIVER_SINGLETON_RVA) as *const usize) };
            if driver_before == null {
                let build_driver: unsafe extern "system" fn() -> usize =
                    unsafe { std::mem::transmute(module_base + STREAMING_DRIVER_BUILDER_RVA) };
                let _ = unsafe { build_driver() };
            }
            // ENABLE streaming on the live heap resmgr (the one WorldResWait checks) if
            // not already enabled. The REAL load has b7c1=1 here; ours is missing only
            // this bit. 0x14066e2e4 sets +0xb7c1 + builds the 2 session singletons +
            // starts the IO jobs.
            let mut enabled = DIAG_COUNT_ZERO;
            if b7c1_before == DIAG_COUNT_ZERO {
                let enable: unsafe extern "system" fn(usize) =
                    unsafe { std::mem::transmute(module_base + STREAMING_ENABLE_RVA) };
                unsafe { enable(resmgr) };
                enabled = DIAG_COUNT_ONE;
            }
            // Re-submit so the builder (re)creates the block load-states.
            let submit_req: unsafe extern "system" fn(usize) =
                unsafe { std::mem::transmute(module_base + REQUEST_SUBMIT_RVA) };
            unsafe { submit_req(ingame) };
            let _ = (
                RESMGR_EXPECTED_VTABLE_RVA,
                INGAMESTEP_RESMGR_250_OFFSET,
                SESSION_SINGLETON_A_RVA,
                SESSION_SINGLETON_B_RVA,
                TITLE_PROCEED_GATE_SET_VALUE,
                LOAD_INITIATOR_RVA,
                WORLD_WORKER_BUILD_RVA,
                SYNTHETIC_STEP_THIS_SIZE,
                SYNTHETIC_STEP_STATE_OFFSET,
                WORLD_WORKER_BUILD_STATE,
                WORLD_STREAM_WORKER_RVA,
            );
            SUBMIT_PLAY_GAME_PHASE.store(SUBMIT_PHASE_DONE, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "submit_play_game: phaseC ENABLE resmgr=0x{resmgr:x} vt=0x{resmgr_vt:x} b7c1={b7c1_before} driver=0x{driver_before:x} enabled={enabled} coord=0x{coord:x} tick={tick}"
            ));
        }
        _ => {
            // Phase D (observe): the scheduler ticks CSTaskGroup 20 (MoveMapStep)
            // every frame, so after phaseC initiated the b80 load the game's own
            // b80 machine + MsbLoad drive the stream to resident natively. Watch
            // b80 advance, mms_state -> -1, and child+0xd8 drain 1->2->0. No pumping
            // (direct-pump of 0x140aff640 crashes: movemapstep-direct-pump-crashes).
            let _ = (
                task_data,
                MOVEMAPSTEP_UPDATE_RVA,
                INGAMESTEP_PENDING_D8_PENDING,
            );
            if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL != null as u64 {
                return true;
            }
            let Some(owner) = (unsafe { title_owner(module_base) }) else {
                return true;
            };
            let owner = owner as usize;
            let ingame = unsafe { *((owner + TITLE_OWNER_JOB_OFFSET) as *const usize) };
            if ingame == null {
                return true;
            }
            let d8 = unsafe { *((ingame + TITLE_OWNER_JOB_PENDING_OFFSET) as *const i32) };
            let movemapstep =
                unsafe { *((ingame + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) as *const usize) };
            let state = unsafe { *((owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) };
            let mms_state = if movemapstep != null {
                unsafe { *((movemapstep + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) }
            } else {
                TITLE_STATE_OWNER_GONE
            };
            let b80 = if gm != null {
                unsafe { *((gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const i32) }
            } else {
                TITLE_STATE_OWNER_GONE
            };
            let world_a = unsafe { *((module_base + WORLD_SINGLETON_A_RVA) as *const usize) };
            // STEP_WorldResWait inputs: the requested coord [[MoveMapStep+0xf0]+0x2c]
            // (byte3 = target area; 0x0a == m10 requested) and the resmgr loaded-block
            // count [[[MoveMapStep+0xf0]+0x10]+0xb3140].
            let wrm = if movemapstep != null {
                unsafe { *((movemapstep + MOVEMAPSTEP_WORLDRES_F0_OFFSET) as *const usize) }
            } else {
                null
            };
            let coord = if wrm != null {
                unsafe { *((wrm + WORLDRES_COORD_2C_OFFSET) as *const i32) }
            } else {
                DIAG_NULL_CHAIN
            };
            let resmgr = if wrm != null {
                unsafe { *((wrm + WORLDRES_RESMGR_10_OFFSET) as *const usize) }
            } else {
                null
            };
            let blocks = if resmgr != null {
                unsafe { *((resmgr + RESMGR_BLOCK_COUNT_B3140_OFFSET) as *const i32) }
            } else {
                DIAG_NULL_CHAIN
            };
            // Scan the block array for slot 9's target area 0x0a (m10): found10 says
            // whether the block is registered (streaming gap) vs absent (loader gap);
            // sample is the first few blocks' area bytes (likely the title's scene).
            let mut found10 = DIAG_COUNT_ZERO;
            let mut sample = DIAG_SAMPLE_ZERO;
            let mut m10phase = DIAG_PHASE_NONE;
            let mut m10flag = DIAG_PHASE_NONE;
            if resmgr != null && blocks > DIAG_COUNT_ZERO {
                let arr = resmgr + WORLDRES_BLOCK_ARRAY_B3030_OFFSET;
                let n = blocks.min(BLOCK_SCAN_MAX);
                for i in DIAG_COUNT_ZERO..n {
                    let entry =
                        unsafe { *((arr + (i as usize) * BLOCK_ENTRY_STRIDE) as *const usize) };
                    if entry == null {
                        continue;
                    }
                    let areaobj =
                        unsafe { *((entry + BLOCK_ENTRY_AREAOBJ_8_OFFSET) as *const usize) };
                    if areaobj == null {
                        continue;
                    }
                    let area = unsafe { *((areaobj + BLOCK_AREAOBJ_AREA_C_OFFSET) as *const i32) };
                    if area == TARGET_AREA_M10 {
                        found10 += DIAG_COUNT_ONE;
                        // load-state = entry->vtable[+0x10](entry); phase = [+0x35].
                        let vt = unsafe { *(entry as *const usize) };
                        if vt != null {
                            let getter: unsafe extern "system" fn(usize) -> usize = unsafe {
                                std::mem::transmute(
                                    *((vt + BLOCK_LOADSTATE_GETTER_VT_10_OFFSET) as *const usize),
                                )
                            };
                            let ls = unsafe { getter(entry) };
                            if ls != null {
                                m10flag = unsafe {
                                    *((ls + BLOCK_LOADSTATE_FLAG_2D_OFFSET) as *const u8) as i32
                                };
                                m10phase = unsafe {
                                    *((ls + BLOCK_LOADSTATE_PHASE_35_OFFSET) as *const u8) as i32
                                };
                            }
                        }
                    }
                    if (i as usize) < BLOCK_SAMPLE_COUNT {
                        sample |= ((area as u32) & BLOCK_AREA_BYTE_MASK)
                            << ((i as u32) * BLOCK_SAMPLE_SHIFT);
                    }
                }
            }
            append_autoload_debug(format_args!(
                "submit_play_game: phaseD state={state} mms_state={mms_state} blocks={blocks} found10={found10} m10phase={m10phase} m10flag={m10flag} sample=0x{sample:x} reqcoord=0x{coord:x} child_d8={d8} csfeman=0x{csfeman:x} tick={tick}"
            ));
            let _ = (world_a, b80);
        }
    }
    true
}

pub(crate) fn ingameinit_drive_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_INGAMEINIT_DRIVE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-ingameinit-drive.txt")
        .exists()
}

pub(crate) fn continue_drive_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_CONTINUE_DRIVE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-continue-drive.txt")
        .exists()
}

pub(crate) fn arm_probe_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_ARM_PROBE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-arm-probe.txt")
            .exists()
}

pub(crate) fn native_arm_loop_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_NATIVE_ARM_LOOP").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-native-arm-loop.txt")
        .exists()
}

pub(crate) fn title_accept_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_TITLE_ACCEPT").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-title-accept.txt")
            .exists()
}

pub(crate) fn title_accept_inject_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TITLE_ACCEPT_INJECT").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-title-accept-inject.txt")
        .exists()
}

pub(crate) fn splash_skip_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_SPLASH_SKIP").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-splash-skip.txt")
            .exists()
}

/// Force OFFLINE boot (no online login attempt -> no "Unable to start in online mode" modal),
/// so the headless autoload reaches the real title/main-menu directly. Auto-on whenever the
/// own-stepper drives the front-end (the autoload runs vanilla-OFFLINE), plus explicit overrides.
/// Gated (not always-on) so it never forces offline on a co-op/online launch that wants the
/// getter live.
pub(crate) fn online_disable_enabled() -> bool {
    own_stepper_enabled()
        || matches!(std::env::var("ER_EFFECTS_OFFLINE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-offline.txt")
            .exists()
}

/// Patch the `GameMan::IsOnlineMode` getter 0x14067a030 to `xor eax,eax; ret` so it always
/// reports OFFLINE. Validates the expected first opcode byte (aborts if the binary differs),
/// VirtualProtects the 3-byte stub region RWX, writes the stub, restores protection, and
/// flushes the instruction cache. Spawned early at DLL attach (timing-independent: it changes
/// what the function RETURNS, not a data field, so it works whether GameMan is constructed yet
/// or not). Mirrors `apply_splash_skip`. Equivalent to the player choosing "Play Offline" --
/// no save access, no struct mutation, no crash risk.
pub(crate) fn apply_online_disable() {
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!("online-disable: module base unavailable"));
        return;
    };
    // Patch the IsOnlineMode getter (consumers read offline). NOTE: the login-readiness predicate
    // patch (0x140cab230) was REVERTED -- it did not prevent the modal (the offline fork shows it
    // too) AND it broke the OnDecide OK-dispatch (the modal stuck instead of proceeding).
    apply_xor_ret_stub(base, ONLINE_DISABLE_RVA, "IsOnlineMode getter");
    let _ = ONLINE_PREDICATE_DISABLE_RVA;
}

/// Patch a 0x48-prologue function body to `xor eax,eax; ret` (return 0) at `base+rva`. Validates
/// the expected first byte, VirtualProtects RWX, writes the 3-byte stub, restores protection, and
/// flushes the icache. Used to force-offline the IsOnlineMode getter + login-readiness predicate.
fn apply_xor_ret_stub(base: usize, rva: usize, label: &str) {
    let target = (base + rva) as *mut u8;
    let existing = unsafe { *target };
    if existing != ONLINE_DISABLE_EXPECTED_FIRST {
        append_autoload_debug(format_args!(
            "online-disable: ABORT {label} -- byte at 0x{:x} is 0x{existing:x}, expected 0x{ONLINE_DISABLE_EXPECTED_FIRST:x}",
            base + rva
        ));
        return;
    }
    let mut old_protect = PAGE_PROTECT_UNSET;
    let protect_ok = unsafe {
        VirtualProtect(
            target as *mut c_void,
            ONLINE_DISABLE_PATCH_LEN,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if protect_ok == HOOK_FALSE_RETURN as i32 {
        append_autoload_debug(format_args!(
            "online-disable: VirtualProtect failed for {label}"
        ));
        return;
    }
    let mut i = TITLE_OWNER_SCAN_START_ADDRESS;
    while i < ONLINE_DISABLE_PATCH_LEN {
        unsafe { *target.add(i) = ONLINE_DISABLE_STUB[i] };
        i += ONLINE_DISABLE_BYTE_STEP;
    }
    let mut restored = PAGE_PROTECT_UNSET;
    unsafe {
        VirtualProtect(
            target as *mut c_void,
            ONLINE_DISABLE_PATCH_LEN,
            old_protect,
            &mut restored,
        )
    };
    unsafe {
        FlushInstructionCache(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            target as *const c_void,
            ONLINE_DISABLE_PATCH_LEN,
        )
    };
    append_autoload_debug(format_args!(
        "online-disable: patched {label} 0x{:x} -> xor eax,eax;ret (forces offline)",
        base + rva
    ));
}

// (The 0x1407b0cf0 "finished-poll" auto-accept hook was removed: RE showed 0x1407b0cf0 is a
// "has >= 2 buttons" layout query, not a finished-poll -- it is never called for the
// connection-error dialog, and writing +0x25e0/+0x25e8 corrupts the dialog (+0x25e8 is the
// button COUNT). The dismiss is force_dismiss_startup_dialog -> OnDecide 0x140927ba0.)

/// DIAGNOSTIC detour for the dialog builder 0x1409275b0 (4 register args rcx/rdx/r8/r9 -> dialog
/// in rax). Calls the original, then (pre-world, capped) logs the BUILT dialog's vtable/class +
/// the 4 args (the FMG message id is one of them) + caller, so we can identify the actual
/// connection-error dialog without guessing. Read-only; never mutates the dialog.
pub(crate) unsafe extern "system" fn msgbox_builder_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = MSGBOX_BUILDER_ORIG.load(Ordering::SeqCst);
    let ret = if orig != null {
        let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(a, b, c, d) }
    } else {
        null
    };
    if ret != null && IN_WORLD_REACHED.load(Ordering::SeqCst) != IN_WORLD_REACHED_YES {
        let base = {
            let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
            if own != null {
                own
            } else {
                game_module_base().unwrap_or(null)
            }
        };
        let vt = unsafe { safe_read_usize(ret) }.unwrap_or(null);
        // CAPTURE the MessageBoxDialog (the connection-error / startup popup) so the game task can
        // dismiss it via OnDecide each frame. Do NOT touch its fields here: +0x25e0 is the chosen
        // button (builder-defaulted to OK) and +0x25e8 is the BUTTON COUNT -- writing them corrupts
        // the dialog. The dismiss is force_dismiss_startup_dialog -> OnDecide 0x140927ba0.
        if vt == base + MSGBOX_DIALOG_VTABLE_RVA {
            CONNECTION_ERROR_DIALOG.store(ret, Ordering::SeqCst);
        }
        let n = MSGBOX_BUILDER_LOG.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        if n < MSGBOX_BUILDER_LOG_MAX {
            let vt_rva = vt.wrapping_sub(base);
            append_autoload_debug(format_args!(
                "msgbox-builder #{n}: dialog=0x{ret:x} vt=0x{vt:x} vt_rva=0x{vt_rva:x} captured={} args(rcx=0x{a:x} rdx=0x{b:x} r8=0x{c:x} r9=0x{d:x}) {}",
                vt == base + MSGBOX_DIALOG_VTABLE_RVA,
                trace_callers_summary()
            ));
        }
    }
    ret
}

/// Dismiss the captured startup MessageBoxDialog (connection-error / EULA / warning) by calling
/// its verified OnDecide/finalize 0x140927ba0(rcx=dialog) -- the genuine OK handler that
/// dispatches the chosen button (builder-defaulted to OK) and drives the dialog to emit "stop"
/// so the parent MenuWindowJob tears it down. Called each frame pre-in-world from the game task
/// (the menu/game thread, where OnDecide's input-registrar singleton access is valid) UNTIL the
/// closing latch [dialog+0x3b0]==1 or the dialog is freed/reused (vtable mismatch) -- both stop
/// the calls, avoiding re-dispatch / UAF. Fault-tolerant reads never AV.
pub(crate) fn force_dismiss_startup_dialog() {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog = CONNECTION_ERROR_DIALOG.load(Ordering::SeqCst);
    if dialog == null {
        return;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != null {
            own
        } else {
            game_module_base().unwrap_or(null)
        }
    };
    let vt = unsafe { safe_read_usize(dialog) }.unwrap_or(null);
    if base == null || vt != base + MSGBOX_DIALOG_VTABLE_RVA {
        // Dialog consumed/freed/reused -> stop (and let the builder hook re-capture a new one).
        CONNECTION_ERROR_DIALOG.store(null, Ordering::SeqCst);
        return;
    }
    // Stop once the dialog has begun teardown (EmitResult set the closing latch) -- calling
    // OnDecide again risks re-dispatch / UAF as the job frees it.
    let closing = unsafe { safe_read_usize(dialog + MSGBOX_CLOSING_LATCH_3B0_OFFSET) }
        .map(|v| v & MSGBOX_LATCH_BYTE_MASK)
        .unwrap_or(MSGBOX_CLOSING_YES);
    if closing == MSGBOX_CLOSING_YES {
        CONNECTION_ERROR_DIALOG.store(null, Ordering::SeqCst);
        let n = DISMISS_WRITE_LOG.load(Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "auto-accept: MessageBoxDialog 0x{dialog:x} closing (latch+0x3b0=1) after {n} OnDecide calls -- dismissed"
        ));
        return;
    }
    // PROPER OK (NOT force-stop): OnDecide 0x140927ba0 branches on the chosen button [dialog+0x25e0]
    // -- if == -1 it calls 0x14078dfd0 (the CANCEL/notify-closed path, which kicks the title flow
    // BACK to PRESS-ANY-BUTTON); if != -1 it DISPATCHES that button (= press OK -> proceed to the
    // main menu offline). The prior force-stop 0x14078dfd0 was exactly the cancel path, so the game
    // bounced back to press-any-button. Fix: set the chosen button to OK (index 0), then OnDecide.
    // Press OK EVERY FRAME (runtime-confirmed: one-shot only HIGHLIGHTS OK; the modal needs the
    // per-frame re-dispatch to progress its decide animation -> activate -> close -> proceed to
    // the main menu). [dialog+0x25e0]=0 selects OK so OnDecide takes the dispatch (NOT cancel) arm.
    // Call THE REAL OK-BUTTON HANDLER 0x14078e030(rcx=dialog) -- captured from a live OK-press.
    // It reads the dialog cursor, gets the OK callback, and COMMITS (0x14078ef20) which actually
    // CLOSES the dialog and emits its result so the title flow PROCEEDS. This is what a real OK
    // does; OnDecide/field-writes/input-injection all failed to close it. Runs each frame on every
    // captured MessageBoxDialog -> skips ALL of them (connection-error, starting-offline, ...).
    let ok_handler: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + MSGBOX_OK_HANDLER_RVA) };
    unsafe { ok_handler(dialog) };
    let n = DISMISS_WRITE_LOG.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n % AUTO_ACCEPT_LOG_INTERVAL == null {
        append_autoload_debug(format_args!(
            "auto-accept: OK-handler 0x{:x}(MessageBoxDialog 0x{dialog:x}) -- real OK-press to close + proceed #{n}",
            base + MSGBOX_OK_HANDLER_RVA
        ));
    }
    let _ = (
        &LAST_ONDECIDE_DIALOG,
        MSGBOX_RESULT_BUTTON_25E0_OFFSET,
        MSGBOX_OK_BUTTON,
        MSGBOX_CONFIRM_LATCH_1BC0_OFFSET,
        MSGBOX_CONFIRM_LATCH_SET,
        MSGBOX_ONDECIDE_RVA,
        INPUTMGR_BITMAP_90_OFFSET,
        MENU_EVENT_CONFIRM_3D,
        MENU_EVENT_PRESSED_BIT,
    );
}

/// Install the startup-popup capture hook once (minhook on the MessageBoxDialog builder
/// 0x1409275b0). The builder hook captures each created MessageBoxDialog into
/// CONNECTION_ERROR_DIALOG; `force_dismiss_startup_dialog` then dismisses it via OnDecide each
/// frame. Idempotent; safe to call every frame from the game task until it succeeds.
pub(crate) fn install_auto_accept_hook() {
    if AUTO_ACCEPT_INSTALLED.load(Ordering::SeqCst) != AUTO_ACCEPT_NOT_INSTALLED {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "auto-accept: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(builder_addr) = game_rva(MSGBOX_BUILDER_RVA) else {
        append_autoload_debug(format_args!("auto-accept: failed to resolve builder rva"));
        return;
    };
    match unsafe {
        MhHook::new(
            builder_addr as *mut c_void,
            msgbox_builder_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            MSGBOX_BUILDER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "auto-accept: queue_enable builder failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    AUTO_ACCEPT_INSTALLED.store(AUTO_ACCEPT_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "auto-accept: hooked MessageBoxDialog builder 0x{builder_addr:x} (capture -> OnDecide dismiss)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "auto-accept: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "auto-accept: MhHook::new builder failed: {status:?}"
        )),
    }
}

/// LATCH detour for the CS::SceneObjProxy ctor 0x14074a700 (rcx=proxy[this], rdx=MenuWindow*,
/// r8/r9 forwarded). Disasm-verified: the ctor does `mov %rdx,%rbx` (0x14074a720) then
/// `mov %rbx,0x20(%rsi)` (0x14074a735) -- so the incoming RDX is the engine-verified MenuWindow it
/// stores at proxy+0x20 (probe-6 proved the OLD TitleTopDialog-factory rdx was a std::function
/// delegate, NOT the MenuWindow). We VALIDATE *(rdx) == base+MenuWindow / MenuWindowProxy vtable and,
/// when valid, OVERWRITE LATCHED_MENU_WINDOW on EVERY valid call (most-recent live host window wins
/// -- the title's host window is latched by the time STAGE2 runs). Then pure passthrough: call the
/// original trampoline with ALL args preserved + return its result, never perturbing the build.
/// bd live-dialog-probe6-factory-fires-returns-dialog-rdx-not-menuwindow-2026.
pub(crate) unsafe extern "system" fn scene_obj_proxy_ctor_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let menu_window = rdx;
    // UNCONDITIONAL entry instrumentation (probe-8): log every ctor entry (rate-limited),
    // independent of the MenuWindow-vtable validation below, to settle definitively whether
    // 0x14074a700 fires at the parked title at all + what its rdx actually is.
    // bd live-dialog-ABI-DEFINITIVE-rcx-correct-rdx-menuwindow-required-2026.
    {
        const SCENE_OBJ_PROXY_CTOR_LOG_MAX: usize = 32;
        const SCENE_OBJ_PROXY_CTOR_HIT_INC: usize = 1;
        static SCENE_OBJ_PROXY_CTOR_HITS: AtomicUsize = AtomicUsize::new(0);
        let hit =
            SCENE_OBJ_PROXY_CTOR_HITS.fetch_add(SCENE_OBJ_PROXY_CTOR_HIT_INC, Ordering::SeqCst);
        if hit < SCENE_OBJ_PROXY_CTOR_LOG_MAX {
            let pvt = unsafe { safe_read_usize(rdx) }.unwrap_or(null);
            append_autoload_debug(format_args!(
                "menuwindow-latch: 0x14074a700 ENTRY #{hit} rdx=0x{rdx:x} *(rdx)=0x{pvt:x}"
            ));
        }
    }
    if menu_window != null {
        if let Ok(base) = game_module_base() {
            let mwvt = unsafe { safe_read_usize(menu_window) }.unwrap_or(null);
            let menu_vt = base + MENU_WINDOW_VTABLE_RVA;
            let menu_proxy_vt = base + MENU_WINDOW_PROXY_VTABLE_RVA;
            if mwvt == menu_vt || mwvt == menu_proxy_vt {
                LATCHED_MENU_WINDOW.store(menu_window, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "menuwindow-latch: 0x14074a700 MenuWindow=0x{menu_window:x} vt=0x{mwvt:x}"
                ));
            }
        }
    }
    let orig = SCENE_OBJ_PROXY_CTOR_ORIG.load(Ordering::SeqCst);
    if orig == null {
        return null;
    }
    let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { f(rcx, rdx, r8, r9) }
}

/// Install the MenuWindow-latch hook once (MinHook on the SceneObjProxy ctor 0x14074a700),
/// matching the auto-accept builder-hook precedent exactly (MhHook::new + queue_enable +
/// MH_ApplyQueued). Must run at process attach BEFORE the title builds during boot so the ctor's
/// rdx (the validated host MenuWindow*) is latched. Idempotent + harmless (latch + passthrough).
pub(crate) fn install_menu_window_latch_hook() {
    if MENU_WINDOW_LATCH_INSTALLED.load(Ordering::SeqCst) != MENU_WINDOW_LATCH_NOT_INSTALLED {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "menuwindow-latch: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(ctor_addr) = game_rva(SCENE_OBJ_PROXY_CTOR_RVA) else {
        append_autoload_debug(format_args!(
            "menuwindow-latch: failed to resolve SceneObjProxy ctor rva"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            ctor_addr as *mut c_void,
            scene_obj_proxy_ctor_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SCENE_OBJ_PROXY_CTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "menuwindow-latch: queue_enable ctor failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    MENU_WINDOW_LATCH_INSTALLED
                        .store(MENU_WINDOW_LATCH_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "menuwindow-latch: hooked SceneObjProxy ctor 0x{ctor_addr:x} (latch rdx=MenuWindow*)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "menuwindow-latch: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "menuwindow-latch: MhHook::new ctor failed: {status:?}"
        )),
    }
}

/// Install the SAVE-SAFE c30-writer diagnostic hook once (MinHook on the SOLE
/// GameMan+0xc30 writer 0x14067bd70), mirroring the MenuWindow-latch precedent exactly
/// (MH_Initialize + MhHook::new + queue_enable + MH_ApplyQueued). Installed
/// UNCONDITIONALLY at process attach. The hook (`c30_writer_hook`) is a pure
/// passthrough that forwards all args + returns the original's result; it only logs the
/// c30-write gate, c30 before/after, and a window of the resident save buffer so we can
/// diagnose why c30 stays default cold. NO SetState5, NO save write -- harmless.
pub(crate) fn install_c30_writer_hook() {
    if C30_WRITER_HOOK_INSTALLED.load(Ordering::SeqCst) != C30_WRITER_HOOK_NOT_INSTALLED {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!("c30-writer: MH_Initialize failed: {status:?}"));
            return;
        }
    }
    let Ok(writer_addr) = game_rva(C30_WRITER_RVA as u32) else {
        append_autoload_debug(format_args!("c30-writer: failed to resolve 0x67bd70 rva"));
        return;
    };
    match unsafe { MhHook::new(writer_addr as *mut c_void, c30_writer_hook as *mut c_void) } {
        Ok(hook) => {
            C30_WRITER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!("c30-writer: queue_enable failed: {status:?}"));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    C30_WRITER_HOOK_INSTALLED
                        .store(C30_WRITER_HOOK_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "c30-writer: hooked 0x{writer_addr:x} (SAVE-SAFE c30-write diagnostic; gate + c30 before/after + buffer window)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "c30-writer: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => {
            append_autoload_debug(format_args!("c30-writer: MhHook::new failed: {status:?}"))
        }
    }
}

/// Clean static splash-skip patch (flip je->jg in STEP_BeginLogo) so the game's
/// own flow advances past the logo via SetState instead of playing it. Validates
/// the expected opcode first (aborts if the binary differs), and restores page
/// protection after. Spawned early at DLL attach so it lands before state 2 runs.
pub(crate) fn apply_splash_skip() {
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!("splash-skip: module base unavailable"));
        return;
    };
    let target = (base + SPLASH_SKIP_RVA) as *mut u8;
    let existing = unsafe { *target };
    if existing != SPLASH_SKIP_EXPECTED_JE {
        append_autoload_debug(format_args!(
            "splash-skip: ABORT -- byte at 0x{:x} is 0x{existing:x}, expected 0x{SPLASH_SKIP_EXPECTED_JE:x}",
            base + SPLASH_SKIP_RVA
        ));
        return;
    }
    let mut old_protect = PAGE_PROTECT_UNSET;
    let protect_ok = unsafe {
        VirtualProtect(
            target as *mut c_void,
            SPLASH_PATCH_LEN,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if protect_ok == HOOK_FALSE_RETURN as i32 {
        append_autoload_debug(format_args!("splash-skip: VirtualProtect failed"));
        return;
    }
    unsafe { *target = SPLASH_SKIP_REPLACEMENT_JG };
    let mut restored = PAGE_PROTECT_UNSET;
    unsafe {
        VirtualProtect(
            target as *mut c_void,
            SPLASH_PATCH_LEN,
            old_protect,
            &mut restored,
        )
    };
    append_autoload_debug(format_args!(
        "splash-skip: patched 0x{:x} 0x{SPLASH_SKIP_EXPECTED_JE:x}->0x{SPLASH_SKIP_REPLACEMENT_JG:x}",
        base + SPLASH_SKIP_RVA
    ));
}

/// Render-thread liveness + bootstrap probe. Runs from the ImGui render loop (a
/// separate thread from the game-task scheduler), so it keeps reporting after the
/// title->menu phase transition stops the title CSTask. Distinguishes "the title
/// advanced (render alive + CSFeMan builds)" from "the game hung (render frozen)".
#[allow(dead_code)]
/// When set, ALL game input is hard-blocked at the API layer (see `enforce_input_block`):
/// DInput8 keyboard+mouse (state zeroed by the `debug::InputBlocker` hook) AND XInput
/// gamepad (this module's hook). Read by `xinput_get_state_hook` each poll so the block is
/// authoritative regardless of window focus.
pub(crate) static BLOCK_INPUT_ACTIVE: AtomicUsize = AtomicUsize::new(0);
const BLOCK_INPUT_ON: usize = 1;
/// Original `XInputGetState` (minhook trampoline). 0 until the hook installs.
pub(crate) static XINPUT_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);

/// STAY-ACTIVE gate (`ER_EFFECTS_STAY_ACTIVE=1` / `er-effects-stay-active.txt`). When set, keep ER's
/// input-accept flag `[DLUID+0x88d]` forced to 1 every tick so a virtual gamepad keeps driving the
/// menus while ER is UNFOCUSED -- letting the user work in another window during a golden capture.
/// Decoded: ER clears that flag each frame when it isn't `GetActiveWindow` (`0x141f292bd`); we re-set
/// it. Touches ONLY focus-input gating, never the sim/save/load.
pub(crate) fn stay_active_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_STAY_ACTIVE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-stay-active.txt")
            .exists()
}

/// True when the autoload/own-stepper probe must run UNCONTAMINATED -- no real keyboard,
/// mouse (move/click), or gamepad input may reach the game even if the user focuses the
/// window. Auto-on whenever the own-stepper drives the front-end (the whole point of that
/// probe is a zero-input load), plus an explicit env/file override for standalone use.
pub(crate) fn block_input_enabled() -> bool {
    // FORCE-BLOCK override (env/file): block UNCONDITIONALLY, even past menu-open. Used to
    // FALSIFY -- runtime-proven 2026-06-17 that blocking through menu-open lets the menu OPEN
    // (self-fire) but starves the post-open navigation, so the load never selects.
    if matches!(std::env::var("ER_EFFECTS_BLOCK_INPUT").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-block-input.txt")
            .exists()
    {
        return true;
    }
    // INJECT-NAV instrument-capture: keep the block ON past menu-open so the user's input is
    // suppressed while the XInput hook fabricates the cursor nav (so nothing pollutes the
    // capture). The fabricated Down is written INTO the otherwise-blocked gamepad state, so the
    // menu still gets a live (synthesized) input each frame -- it does not stall.
    if own_stepper_enabled() && !own_stepper_passive_enabled() && inject_nav_enabled() {
        return true;
    }
    // PASSIVE mode never blocks. Otherwise keep the block engaged through the ENTIRE headless
    // drive -- boot -> menu-open -> zero-input title-confirm Load fire -> mount -> confirm --
    // releasing ONLY once in-world (the user takes over) or on abort (phase DONE). The
    // title-confirm route drives the load with NO user input (direct field-write + functor call,
    // not the input pipeline), so there is no reason to release at menu-open; keeping it on makes
    // the run UNCONTAMINATABLE (the user cannot nudge it even deliberately). [Earlier design
    // released at menu-open for a user-driven Continue; that is obsolete now.]
    own_stepper_enabled()
        && !own_stepper_passive_enabled()
        && IN_WORLD_REACHED.load(Ordering::SeqCst) != IN_WORLD_REACHED_YES
        && OWN_STEPPER_PHASE.load(Ordering::SeqCst) != OWN_STEPPER_PHASE_DONE
}

/// Release the input block (DInput + XInput) once `block_input_enabled()` flips false mid-run.
/// The hooks stay installed but pass input through when `BLOCK_INPUT_ACTIVE` is clear; the
/// DInput blocker also needs its own flags cleared. Acts once on the ON->off transition.
pub(crate) fn release_input_block_now() {
    if BLOCK_INPUT_ACTIVE.swap(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst) == BLOCK_INPUT_ON {
        InputBlocker::get_instance().block_only(InputFlags::empty());
        // Release the cursor confinement (paired with the ClipCursor lockdown in enforce).
        let _ = unsafe { ClipCursor(None) };
        append_autoload_debug(format_args!(
            "input-block: RELEASED (in-world / abort) -- keyboard/mouse/gamepad + cursor live"
        ));
    }
}

/// XInput `XInputGetState(user_index, *mut XINPUT_STATE) -> DWORD` detour. Calls the real
/// function, then -- while the block is active -- zeroes the XINPUT_GAMEPAD sub-struct
/// (buttons + triggers + thumbsticks) so the game reads a connected-but-idle pad (no
/// "controller disconnected" popup, but zero input). Leaves the disconnected return code
/// untouched so a genuinely absent pad still reads absent.
pub(crate) unsafe extern "system" fn xinput_get_state_hook(user_index: u32, state: *mut u8) -> u32 {
    const XINPUT_SUCCESS: u32 = 0;
    const XINPUT_ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
    // XINPUT_STATE = { DWORD dwPacketNumber; XINPUT_GAMEPAD Gamepad; }; the gamepad sub-struct
    // (wButtons,bLeftTrigger,bRightTrigger,sThumbLX/LY/RX/RY) starts at +4 and is 12 bytes.
    const XINPUT_GAMEPAD_OFFSET: usize = 4;
    const XINPUT_GAMEPAD_SIZE: usize = 12;
    const ZERO_FILL_BYTE: u8 = 0;
    let orig = XINPUT_GET_STATE_ORIG.load(Ordering::SeqCst);
    let hr = if orig != TITLE_OWNER_SCAN_START_ADDRESS {
        let f: unsafe extern "system" fn(u32, *mut u8) -> u32 =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(user_index, state) }
    } else {
        XINPUT_ERROR_DEVICE_NOT_CONNECTED
    };
    const XINPUT_PACKET_OFFSET: usize = 0;
    const WBUTTONS_OFFSET_IN_GAMEPAD: usize = 0;
    if !state.is_null() && BLOCK_INPUT_ACTIVE.load(Ordering::SeqCst) == BLOCK_INPUT_ON {
        let inject = inject_nav_enabled()
            && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO;
        if inject {
            // Fabricate the gamepad state at the poll source from the schedule driven each frame
            // by own_stepper idx10 (this hook may never be polled if no controller, so the
            // schedule does NOT live here). Force SUCCESS + a fresh packet number so a live pad is
            // simulated; write the scheduled D-pad Down. Harmless if the game ignores XInput.
            let buttons = INJECT_NAV_CUR_BUTTONS.load(Ordering::SeqCst) as u16;
            let pkt = INJECT_NAV_FRAME.load(Ordering::SeqCst) as u32;
            unsafe {
                std::ptr::write_bytes(
                    state.add(XINPUT_GAMEPAD_OFFSET),
                    ZERO_FILL_BYTE,
                    XINPUT_GAMEPAD_SIZE,
                );
                *(state.add(XINPUT_PACKET_OFFSET) as *mut u32) = pkt;
                *(state.add(XINPUT_GAMEPAD_OFFSET + WBUTTONS_OFFSET_IN_GAMEPAD) as *mut u16) =
                    buttons;
            }
            let _ = user_index;
            return XINPUT_SUCCESS;
        }
        if hr == XINPUT_SUCCESS {
            unsafe {
                std::ptr::write_bytes(
                    state.add(XINPUT_GAMEPAD_OFFSET),
                    ZERO_FILL_BYTE,
                    XINPUT_GAMEPAD_SIZE,
                )
            };
        }
    }
    hr
}

/// Install the XInput gamepad block once. Hooks `XInputGetState` (and ordinal-100
/// `XInputGetStateEx`, used by Steam Input) in whichever xinput runtime DLL is loaded.
/// minhook-based, mirroring `create_continue_trace_hook`.
unsafe fn install_xinput_block() {
    const XINPUT_DLLS: [&[u8]; 5] = [
        b"xinput1_4.dll\0",
        b"xinput1_3.dll\0",
        b"xinput9_1_0.dll\0",
        b"xinput1_2.dll\0",
        b"xinput1_1.dll\0",
    ];
    const XINPUT_GET_STATE_EX_ORDINAL: usize = 100;
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "xinput-block: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let mut hooked_any = false;
    for name in XINPUT_DLLS {
        let hmod = match unsafe { GetModuleHandleA(PCSTR(name.as_ptr())) } {
            Ok(h) if !h.is_invalid() => h,
            _ => continue,
        };
        let proc = unsafe { GetProcAddress(hmod, PCSTR(b"XInputGetState\0".as_ptr())) };
        let Some(addr) = proc else { continue };
        let addr = addr as usize;
        match unsafe { MhHook::new(addr as *mut c_void, xinput_get_state_hook as *mut c_void) } {
            Ok(hook) => {
                XINPUT_GET_STATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                if let Err(status) = unsafe { hook.queue_enable() } {
                    append_autoload_debug(format_args!(
                        "xinput-block: queue_enable XInputGetState failed: {status:?}"
                    ));
                } else {
                    append_autoload_debug(format_args!(
                        "xinput-block: hooked XInputGetState at 0x{addr:x}"
                    ));
                    std::mem::forget(hook);
                    hooked_any = true;
                }
            }
            Err(status) => append_autoload_debug(format_args!(
                "xinput-block: MhHook::new XInputGetState failed: {status:?}"
            )),
        }
        // Steam Input routes the guide button through ordinal-100 XInputGetStateEx; neuter it
        // too so a focused pad cannot drive menus through that path. Same zeroing detour.
        let ex = unsafe { GetProcAddress(hmod, PCSTR(XINPUT_GET_STATE_EX_ORDINAL as *const u8)) };
        if let Some(ex_addr) = ex {
            let ex_addr = ex_addr as usize;
            if ex_addr != addr {
                if let Ok(hook) = unsafe {
                    MhHook::new(ex_addr as *mut c_void, xinput_get_state_hook as *mut c_void)
                } {
                    let _ = unsafe { hook.queue_enable() };
                    std::mem::forget(hook);
                    append_autoload_debug(format_args!(
                        "xinput-block: hooked XInputGetStateEx(ord 100) at 0x{ex_addr:x}"
                    ));
                }
            }
        }
        break;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {}
        status => append_autoload_debug(format_args!(
            "xinput-block: MH_ApplyQueued failed: {status:?}"
        )),
    }
    if !hooked_any {
        append_autoload_debug(format_args!(
            "xinput-block: no xinput DLL with XInputGetState found yet (will retry next frame)"
        ));
    }
}

/// Tracks whether the DInput keyboard+mouse `install_hooks` has run (once).
static DINPUT_BLOCK_INSTALLED: AtomicUsize = AtomicUsize::new(0);

/// Enforce the comprehensive input block for this frame. Self-contained (no args) so it can
/// run from EITHER the game task OR the render loop -- critical because under the offline
/// launcher the hudhook render loop does NOT execute at the title, so the render-loop call
/// alone never engaged the block (that was the contamination hole). Driven every frame from
/// the game task while `block_input_enabled()`:
///   1. ONCE: install the DInput8 keyboard+mouse `GetDeviceState` block (panics on probe
///      failure -> contained with catch_unwind so the FD4 task never unwinds into C++).
///   2. EVERY frame: assert the block-all flag (sticky, overriding any overlay want-capture
///      clear) and install/retry the XInput gamepad hook until the xinput DLL is present.
/// Genuinely zero-input: it only SUPPRESSES device reads -- it never synthesizes any input.
pub(crate) fn enforce_input_block_now() {
    let blocker = InputBlocker::get_instance();
    if DINPUT_BLOCK_INSTALLED.swap(BLOCK_INPUT_ON, Ordering::SeqCst)
        == TITLE_OWNER_SCAN_START_ADDRESS
    {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            blocker.install_hooks()
        }));
        match result {
            Ok(Ok(())) => {
                append_autoload_debug(format_args!(
                    "input-block: DInput keyboard+mouse GetDeviceState hook installed"
                ));
            }
            Ok(Err(status)) => append_autoload_debug(format_args!(
                "input-block: DInput install_hooks failed: {status:?} (XInput still hooks)"
            )),
            Err(_) => append_autoload_debug(format_args!(
                "input-block: DInput install_hooks panicked (contained; XInput still hooks)"
            )),
        }
    }
    BLOCK_INPUT_ACTIVE.store(BLOCK_INPUT_ON, Ordering::SeqCst);
    blocker.block_only(InputFlags::all());
    if XINPUT_GET_STATE_ORIG.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS {
        // Not yet hooked (xinput DLL may load late): retry each frame until it sticks.
        unsafe { install_xinput_block() };
    }
    // Lock down MOUSE MOVEMENT: the DInput GetDeviceState block zeroes keyboard + mouse buttons +
    // DInput mouse deltas, but ER moves the MENU cursor via the OS cursor position (GetCursorPos),
    // which DInput blocking does NOT cover -- so the user can still move the cursor. Confine the OS
    // cursor to a 1x1 rect every frame: it physically cannot move regardless of which API reads it,
    // making the run uncontaminatable by the mouse. Released (ClipCursor(None)) when the block lifts.
    const CLIP_ORIGIN: i32 = 0;
    const CLIP_EDGE: i32 = 1;
    let clip = RECT {
        left: CLIP_ORIGIN,
        top: CLIP_ORIGIN,
        right: CLIP_EDGE,
        bottom: CLIP_EDGE,
    };
    let _ = unsafe { ClipCursor(Some(&clip)) };
}

pub(crate) fn render_liveness_probe() {
    if !title_accept_enabled() {
        return;
    }
    let frame = RENDER_FRAME_COUNT.fetch_add(AV_LOG_LINE_INCREMENT, Ordering::SeqCst);
    if frame % RENDER_PROBE_INTERVAL != TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let Ok(base) = game_module_base() else {
        return;
    };
    let csfeman = unsafe { *((base + CSFEMAN_SINGLETON_RVA) as *const usize) };
    let latch = unsafe { *((base + TITLE_ACCEPT_LATCH_RVA) as *const u8) };
    append_autoload_debug(format_args!(
        "render_probe: frame={frame} csfeman=0x{csfeman:x} latch={latch}"
    ));
}

/// Boot-level title-accept (genuine zero input). The press-any-button wall is the
/// boot intro/movie thread parked in its movie-wait loop; the latch 0x143d856a0
/// (sole writer 0x140c8ff41) is set only AFTER that loop finishes, which is what
/// lets the inner MenuJobWait advance 10->11. The movie-dismiss gate 0x140e90820
/// has NO input check -- it finishes on decode completion or the skip-flag byte
/// 0x14458b8a5. So writing the skip-flag makes the intro thread complete its REAL
/// fade-out + teardown + latch LEGITIMATELY (proper bookkeeping, unlike the bare
/// latch poke that crashes), driving the native title-accept with zero input.
/// Watch CSFeMan 0x143d6b880 for the bootstrap.
pub(crate) unsafe fn title_accept_tick(module_base: usize, tick: u64, do_write: bool) {
    if tick < ARM_PROBE_MIN_TICK {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Module-base globals -- always safe committed reads. NO title_owner scan:
    // its full-memory VirtualQuery+deref walk raced the booting game (region freed
    // mid-scan -> AV, the boot-crash). The autoload needs none of it -- the movie
    // singleton and GameMan are fixed globals.
    let csfeman = unsafe { *((module_base + CSFEMAN_SINGLETON_RVA) as *const usize) };
    let latch = unsafe { *((module_base + TITLE_ACCEPT_LATCH_RVA) as *const u8) };
    let movie = unsafe { *((module_base + MOVIE_SINGLETON_RVA) as *const usize) };
    let skip = unsafe { *((module_base + MOVIE_SKIP_FLAG_RVA) as *const u8) };
    let gm = unsafe { *((module_base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) as *const usize) };
    let session = unsafe { *((module_base + SESSION_SINGLETON_RVA) as *const usize) };
    let log_now = (tick % ARM_PROBE_TICK_INTERVAL == null as u64)
        || (skip == MOVIE_SKIP_FLAG_SET && csfeman == null);
    // Scan-free native movie dismiss: gated on the movie singleton being present
    // with the expected vtable (= the title bg movie is up at press-any-button,
    // since splash-skip removed the logos) + a tick floor + skip-flag clear.
    if do_write && tick >= DISMISS_MIN_TICK && skip == MOVIE_SKIP_FLAG_CLEAR && movie != null {
        let movie_vtable = unsafe { *(movie as *const usize) };
        let hwnd = unsafe { *((movie + MOVIE_HWND_OFFSET) as *const usize) };
        if movie_vtable == module_base + MOVIE_VTABLE_RVA && hwnd != null {
            let hwnd_ptr = hwnd as *mut c_void;
            unsafe {
                let menu = GetSystemMenu(hwnd_ptr, WND_GET_SYSTEM_MENU_KEEP);
                if !menu.is_null() {
                    DeleteMenu(menu, WND_SC_CLOSE, WND_MF_BYCOMMAND);
                }
                ShowWindow(hwnd_ptr, WND_SW_HIDE);
                UpdateWindow(hwnd_ptr);
                *((module_base + MOVIE_SKIP_FLAG_RVA) as *mut u8) = MOVIE_SKIP_FLAG_SET;
            }
            append_autoload_debug(format_args!(
                "title_accept: native movie dismiss (movie=0x{movie:x} hwnd=0x{hwnd:x} latch={latch} tick={tick})"
            ));
        }
    }
    // Observability: GameMan load fields + session + csfeman, to see the post-
    // dismiss bootstrap/load trajectory (drives where to arm the load recipe).
    if log_now {
        let (cmd, force, slot, loading) = if gm != null {
            unsafe {
                (
                    *((gm + GAME_MAN_REQUESTED_SLOT_B78_OFFSET) as *const i32),
                    *((gm + GAME_MAN_ARM_FLAG_B72_OFFSET) as *const u8),
                    *((gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32),
                    *((gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const u8),
                )
            }
        } else {
            (
                TITLE_STATE_OWNER_GONE,
                MOVIE_SKIP_FLAG_CLEAR,
                TITLE_STATE_OWNER_GONE,
                MOVIE_SKIP_FLAG_CLEAR,
            )
        };
        append_autoload_debug(format_args!(
            "title_accept: skip={skip} movie=0x{movie:x} latch={latch} csfeman=0x{csfeman:x} session=0x{session:x} gm=0x{gm:x} cmd={cmd} force={force} slot={slot} loading={loading} tick={tick}"
        ));
    }
}

/// Per-frame native autoload arm. Recipe A set the slot once and the title reset
/// it to -1 before the save-mgr update could arm, so the latch fired Finish with
/// nothing armed -> null deref. This re-sets the slot EVERY frame (against the
/// title's reset) and sets the latch, giving the native update 0x14067f5d0 a
/// chance to arm GameMan+0xb72 before Finish. Observes b72 / b80 / CSFeMan to see
/// if the arm + bootstrap take. Crash logger should run alongside.
pub(crate) unsafe fn native_arm_loop_tick(module_base: usize, slot: i32, tick: u64) {
    if tick < ARM_PROBE_MIN_TICK {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let game_man =
        unsafe { *((module_base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) as *const usize) };
    if game_man == null {
        return;
    }
    let load_in_progress =
        unsafe { *((game_man + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const u8) };
    let armed = unsafe { *((game_man + GAME_MAN_ARM_FLAG_B72_OFFSET) as *const u8) };
    let csfeman = unsafe { *((module_base + CSFEMAN_SINGLETON_RVA) as *const usize) };
    if load_in_progress == TITLE_NATIVE_JOB_TASK_DATA_ZERO {
        // Re-arm each frame: persist the slot against the title's reset, set latch.
        let set_save_slot: unsafe extern "system" fn(i32) =
            unsafe { std::mem::transmute(module_base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
        unsafe { set_save_slot(slot) };
        unsafe {
            *((module_base + SELECTBOT_LOAD_GATE_RVA) as *mut u8) = TITLE_PROCEED_GATE_SET_VALUE;
        }
    }
    if tick % ARM_PROBE_TICK_INTERVAL == null as u64 {
        let ac0 = unsafe { *((game_man + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
        append_autoload_debug(format_args!(
            "native_arm_loop tick={tick} ac0={ac0} b72={armed} b80={load_in_progress} csfeman=0x{csfeman:x}"
        ));
    }
}

/// Read-only probe of the native autoload-arm preconditions at the title. The
/// decisive unknown is `[slotmgr+0x8]` (the loaded slot-record container): the
/// native save-mgr update arms autoload only when it is populated. Logs the
/// GameMan flow flags, slot manager + its data/container pointers, and whether
/// CSFeMan / the input manager exist yet. Touches no state.
pub(crate) unsafe fn arm_precondition_probe(module_base: usize, tick: u64) {
    if tick < ARM_PROBE_MIN_TICK
        || tick % ARM_PROBE_TICK_INTERVAL != TITLE_OWNER_SCAN_START_ADDRESS as u64
    {
        return;
    }
    let read_ptr = |rva: usize| unsafe { *((module_base + rva) as *const usize) };
    let game_man = read_ptr(FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA);
    let slot_mgr = read_ptr(SLOT_MANAGER_RVA);
    let csfeman = read_ptr(CSFEMAN_SINGLETON_RVA);
    let input_mgr = read_ptr(TITLE_INPUT_MANAGER_RVA);
    let latch = unsafe { *((module_base + SELECTBOT_LOAD_GATE_RVA) as *const u8) };
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let gm_byte = |off: usize| {
        if game_man != null {
            i64::from(unsafe { *((game_man + off) as *const u8) })
        } else {
            ARM_PROBE_FIELD_ABSENT
        }
    };
    let gm_i32 = |off: usize| {
        if game_man != null {
            i64::from(unsafe { *((game_man + off) as *const i32) })
        } else {
            ARM_PROBE_FIELD_ABSENT
        }
    };
    let (slot_data, slot_container) = if slot_mgr != null {
        (
            unsafe { *((slot_mgr + SLOT_MANAGER_DATA_OFFSET) as *const usize) },
            unsafe { *((slot_mgr + SLOT_MANAGER_CONTAINER_OFFSET) as *const usize) },
        )
    } else {
        (null, null)
    };
    append_autoload_debug(format_args!(
        "arm_probe tick={tick} gm=0x{game_man:x} slotmgr=0x{slot_mgr:x} slotmgr+8=0x{slot_data:x} slotmgr+78=0x{slot_container:x} csfeman=0x{csfeman:x} input_mgr=0x{input_mgr:x} latch={latch} b80={} ac0={} b72={} b73={} b75={} b78={} bc4={}",
        gm_byte(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET),
        gm_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET),
        gm_byte(GAME_MAN_ARM_FLAG_B72_OFFSET),
        gm_byte(GAME_MAN_FLAG_B73_PROBE_OFFSET),
        gm_byte(GAME_MAN_FLAG_B75_PROBE_OFFSET),
        gm_i32(GAME_MAN_REQUESTED_SLOT_B78_OFFSET),
        gm_byte(GAME_MAN_FLAG_BC4_OFFSET),
    ));
}

/// Recipe Option 1 (genuine offline continue, flagless): drive the MoveMapList
/// dispatcher 0x140afb880 each frame with GameMan b73 set so it begins
/// current_slot_load and deserializes the REAL slot character (sets
/// GameMan+0x10=1), also building the world singletons. owner is a synthetic
/// buffer with +0x12c = slot. Never writes the force flag 0x143d856a0.
pub(crate) unsafe fn continue_drive_tick(module_base: usize, slot: i32, tick: u64) {
    // Lower gate than the title-owner experiments: continue_drive only needs
    // GameMan (ready ~tick 82), not the inner TitleStep owner, and degraded
    // sessions sometimes exit ~tick 154, so start the drive earlier.
    if tick < CONTINUE_DRIVE_MIN_TICK {
        return;
    }
    let game_man =
        unsafe { *((module_base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) as *const usize) };
    if game_man == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let real_done = unsafe { *((game_man + GAME_MAN_REAL_LOAD_DONE_OFFSET) as *const i32) };
    let load_progress =
        unsafe { *((game_man + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const u8) };
    let map14 = unsafe { *((game_man + FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET) as *const i32) };
    if real_done == GAME_MAN_REAL_LOAD_DONE_VALUE {
        if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
            append_autoload_debug(format_args!(
                "continue_drive: REAL LOAD DONE gm+0x10=1 map14={map14} b80={load_progress} tick={tick}"
            ));
        }
        return;
    }
    // Synthetic MoveMapList owner: the offline-continue path reads owner+0x12c
    // (slot) and +0x12a. A persistent zeroed buffer suffices.
    let mut owner_ptr = CONTINUE_OWNER_PTR.load(Ordering::SeqCst);
    if owner_ptr == TITLE_OWNER_SCAN_START_ADDRESS {
        let buf = vec![SYNTHETIC_ZERO_QWORD; CONTINUE_OWNER_QWORDS].into_boxed_slice();
        owner_ptr = Box::leak(buf).as_mut_ptr() as usize;
        CONTINUE_OWNER_PTR.store(owner_ptr, Ordering::SeqCst);
    }
    let owner = owner_ptr as *mut u8;
    unsafe {
        *(owner.add(CONTINUE_OWNER_SLOT_OFFSET) as *mut i32) = slot;
        *(owner.add(CONTINUE_OWNER_FLAG_12A_OFFSET)) = CONTINUE_OWNER_FLAG_12A_VALUE;
    }
    // Until the async load has begun (b80 != 0), arm the slot + b73 so the
    // dispatcher selects current_slot_load and begins. The begin is gated on
    // b80==0, so re-arming after it starts cannot re-submit.
    if !CONTINUE_DRIVE_BEGUN.load(Ordering::SeqCst) {
        let set_save_slot: unsafe extern "system" fn(i32) =
            unsafe { std::mem::transmute(module_base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
        unsafe { set_save_slot(slot) };
        unsafe {
            *((game_man + GAME_MAN_B73_FLAG_OFFSET) as *mut u8) = GAME_MAN_B73_FLAG_SET;
        }
        if load_progress != TITLE_NATIVE_JOB_TASK_DATA_ZERO {
            CONTINUE_DRIVE_BEGUN.store(true, Ordering::SeqCst);
        }
    }
    let dispatcher: unsafe extern "system" fn(*mut u8) -> usize =
        unsafe { std::mem::transmute(module_base + MOVEMAP_DISPATCHER_RVA) };
    let _ = unsafe { dispatcher(owner) };
    if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
        let real_after = unsafe { *((game_man + GAME_MAN_REAL_LOAD_DONE_OFFSET) as *const i32) };
        let b80_after =
            unsafe { *((game_man + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const u8) };
        let map14_after =
            unsafe { *((game_man + FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET) as *const i32) };
        append_autoload_debug(format_args!(
            "continue_drive: drove dispatcher slot={slot} b80={b80_after} real_done={real_after} map14={map14_after} tick={tick}"
        ));
    }
}

/// Recipe B (flagless): drive the outer SimpleTitleStep IngameInit once to prime
/// the world subsystems and submit the load, then pump the InGameStep each frame
/// to completion. Never touches the force flag 0x143d856a0. Replaces
/// force_play_game (which double-submits). Locates the outer object via scan,
/// arms the staging slot the same frame (IngameInit's descriptor builder reads
/// GameMan+0xac0), calls IngameInit(outer, &FD4TaskData) once, then ticks the
/// InGameStep pump and observes the load cascade.
pub(crate) unsafe fn ingameinit_drive_tick(
    module_base: usize,
    slot: i32,
    tick: u64,
    task_data: &FD4TaskData,
) {
    if tick < TITLE_NATIVE_JOB_MIN_TICK {
        return;
    }
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        return;
    };
    let ingame = unsafe { *(owner.add(TITLE_OWNER_JOB_OFFSET) as *const usize) };
    let owner_state = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    if ingame == TITLE_OWNER_SCAN_START_ADDRESS {
        if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
            append_autoload_debug(format_args!(
                "ingameinit_drive: ingame(owner+0x2e8) is NULL, owner={owner:p} state={owner_state} tick={tick}"
            ));
        }
        return;
    }
    let _ = owner_state;
    if !INGAMEINIT_DRIVE_DONE.swap(true, Ordering::SeqCst) {
        // Arm the staging slot this frame (the descriptor builder 0x140aea590
        // reads GameMan+0xac0).
        let set_save_slot: unsafe extern "system" fn(i32) =
            unsafe { std::mem::transmute(module_base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
        unsafe { set_save_slot(slot) };
        // Compute a valid (non -1) map id so IngameInit takes the continue
        // variant (variant 2 / -1 is the new-game path). Parse the same default
        // map string the new-game path uses.
        let map_parser: unsafe extern "system" fn(*const c_void) -> i32 =
            unsafe { std::mem::transmute(module_base + INGAMEINIT_MAP_PARSER_RVA) };
        let map_id = unsafe { map_parser((module_base + DEFAULT_MAP_STRING_RVA) as *const c_void) };
        // The SimpleTitleStep container is never instantiated in this build, so we
        // call IngameInit with a SYNTHETIC `this`: it only reads +0xc0 (InGameStep)
        // and +0x130 (map), and its tail 0x140b0a980 inc's +0x4c (safe while
        // +0x48 <= 6). A persistent zeroed buffer satisfies all of that.
        let mut synth_ptr = SYNTHETIC_OUTER_PTR.load(Ordering::SeqCst);
        if synth_ptr == TITLE_OWNER_SCAN_START_ADDRESS {
            let buf = vec![SYNTHETIC_ZERO_QWORD; INGAMEINIT_SYNTHETIC_QWORDS].into_boxed_slice();
            synth_ptr = Box::leak(buf).as_mut_ptr() as usize;
            SYNTHETIC_OUTER_PTR.store(synth_ptr, Ordering::SeqCst);
        }
        let synth = synth_ptr as *mut u8;
        unsafe {
            *(synth.add(OUTER_STEP_INGAMESTEP_OFFSET) as *mut usize) = ingame;
            *(synth.add(OUTER_STEP_MAP_OVERRIDE_130_OFFSET) as *mut i32) = map_id;
        }
        let ingame_init: unsafe extern "system" fn(*mut u8, *const FD4TaskData) -> usize =
            unsafe { std::mem::transmute(module_base + INGAMEINIT_HANDLER_RVA) };
        append_autoload_debug(format_args!(
            "ingameinit_drive: calling IngameInit synth={synth:p} slot={slot} map_id={map_id} ingame={ingame:#x}"
        ));
        let _ = unsafe { ingame_init(synth, task_data as *const FD4TaskData) };
        let ingame_d8 = unsafe { *((ingame + TITLE_OWNER_JOB_PENDING_OFFSET) as *const i32) };
        let ingame_cur = unsafe { *((ingame + INGAMESTEP_STEP_STATE_OFFSET) as *const i32) };
        append_autoload_debug(format_args!(
            "ingameinit_drive: IngameInit returned ingame_d8={ingame_d8} ingame_cur={ingame_cur}"
        ));
        return;
    }
    // After priming+submit: pump the InGameStep each frame so step 7 observes the
    // (now primed) stream reach resident and sets d8=2 -> load completes.
    let ingame_ptr = ingame as *mut u8;
    let cur = unsafe { *(ingame_ptr.add(INGAMESTEP_STEP_STATE_OFFSET) as *const i32) };
    let d8 = unsafe { *(ingame_ptr.add(TITLE_OWNER_JOB_PENDING_OFFSET) as *const i32) };
    let last_next = INGAMESTEP_PUMP_LAST_NEXT.swap(cur, Ordering::SeqCst);
    let last_d8 = INGAMESTEP_PUMP_LAST_D8.swap(d8, Ordering::SeqCst);
    if cur != last_next || d8 != last_d8 {
        append_autoload_debug(format_args!(
            "ingameinit_drive: pump cur={cur} d8={d8} ingame={ingame:#x}"
        ));
    }
    if cur == INGAMESTEP_FINISHED_SENTINEL || d8 == INGAMESTEP_LOAD_DONE {
        return;
    }
    let Ok(pump) = game_rva(STEP_PUMP_DRIVER_RVA) else {
        return;
    };
    let pump: unsafe extern "system" fn(*mut u8, *const FD4TaskData) -> usize =
        unsafe { std::mem::transmute(pump) };
    let _ = unsafe { pump(ingame_ptr, task_data as *const FD4TaskData) };
}

pub(crate) fn ingamestep_unpin_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_INGAMESTEP_UNPIN").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-ingamestep-unpin.txt")
        .exists()
}

/// Drives the native TitleStep state machine to `STEP_PlayGame` once.
///
/// Live zero-input probes showed the game parks at `STEP_BeginTitle`
/// (PRESS ANY BUTTON) with GameMan ready but the MoveMapList load dispatcher
/// inactive, so directly setting the continue flags is a no-op. Static RE maps
/// the TitleStep handler table: index 5 (`STEP_PlayGame`, 0x140b0d5b0) reads the
/// selected save slot and submits the native load job. This selects slot `slot`
/// via the menu set-slot primitive and advances the owner's state field so the
/// game's own title task dispatches `STEP_PlayGame` on the next frame — no host
/// input and no synthetic load-primitive calls. We only act once the owner has
/// reached `STEP_BeginTitle`, which guarantees `STEP_InitMenu` already built the
/// menu object `STEP_PlayGame` depends on.
pub(crate) unsafe fn call_force_play_game_once(module_base: usize, slot: i32, tick: u64) -> bool {
    if tick < TITLE_NATIVE_JOB_MIN_TICK {
        return false;
    }
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        return false;
    };
    let state_before = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    // Log every TitleStep state transition so we can see whether the forced
    // STEP_PlayGame write sticks and advances (5 -> 6 GameStepWait -> load) or
    // gets reset by the title task / a different owner instance.
    let last_state = FORCE_PLAY_GAME_LAST_STATE.swap(state_before, Ordering::SeqCst);
    if state_before != last_state {
        // Read GameMan+0x14 (the load value pair writes) each transition: if it
        // becomes nonnegative when PlayGame runs (5 -> 6), the pair chain
        // succeeded and the gap is downstream (GameStepWait/job); if it stays -1,
        // submit/validate/pair never wrote it.
        let gm = unsafe { *((module_base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) as *const usize) };
        let load14 = if gm != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((gm + FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET) as *const i32) }
        } else {
            DIRECT_INPUT_FAILURE_HRESULT
        };
        append_autoload_debug(format_args!(
            "force_play_game: observed state {last_state}->{state_before} load14={load14} tick={tick}"
        ));
    }
    if FORCE_PLAY_GAME_CALLED.load(Ordering::SeqCst) != TITLE_NATIVE_JOB_NOT_CALLED {
        // Already drove the state once; keep observing transitions (logged above).
        // While parked in GameStepWait, periodically report the load job's pending
        // field so we can see whether anything drains it.
        if state_before == TITLE_STEP_GAME_STEP_WAIT {
            let job = unsafe { *(owner.add(TITLE_OWNER_JOB_OFFSET) as *const usize) };
            if job != TITLE_OWNER_SCAN_START_ADDRESS {
                let pending = unsafe { *((job + TITLE_OWNER_JOB_PENDING_OFFSET) as *const i32) };
                if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                    append_autoload_debug(format_args!(
                        "force_play_game: gamestepwait job={job:#x} job_d8={pending} tick={tick}"
                    ));
                }
                // NOTE: calling the menu-task update wrapper (0x82a0f0) directly on
                // this job crashed the game (autoload-live-playgame-v10) -- the job
                // is not the right `this` / reentrancy. Pumping must go through the
                // game's own task runner; do not force-orphan the job.
            }
        }
        return true;
    }
    // The live title idles at STEP_MenuJobWait (the input-wait state shown as
    // PRESS ANY BUTTON); STEP_BeginTitle is the alternate stable pre-load step.
    // Both run after STEP_InitMenu built the menu object PlayGame needs.
    if state_before != TITLE_STEP_BEGIN_TITLE && state_before != TITLE_STEP_MENU_JOB_WAIT {
        return false;
    }
    let set_save_slot: unsafe extern "system" fn(i32) =
        unsafe { std::mem::transmute(module_base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
    unsafe { set_save_slot(slot) };
    // Read-only diagnostic: log the PlayGame load-pair preconditions so we can
    // see which one blocks (pair skips writing GameMan+0x14 unless b28==0; the
    // validate step gates on 12d/12e).
    let game_man_ptr =
        unsafe { *((module_base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) as *const usize) };
    if game_man_ptr != TITLE_OWNER_SCAN_START_ADDRESS {
        let gm = game_man_ptr as *const u8;
        let ac0 = unsafe { *(gm.add(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
        let load14 = unsafe { *(gm.add(FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET) as *const i32) };
        let b28 = unsafe { *gm.add(FORCE_PLAY_GAME_GM_PAIR_GATE_B28_OFFSET) };
        let f12d = unsafe { *gm.add(FORCE_PLAY_GAME_GM_VALIDATE_12D_OFFSET) };
        let f12e = unsafe { *gm.add(FORCE_PLAY_GAME_GM_VALIDATE_12E_OFFSET) };
        append_autoload_debug(format_args!(
            "force_play_game: gm={game_man_ptr:#x} ac0={ac0} load14={load14} b28={b28} f12d={f12d} f12e={f12e}"
        ));
    }
    unsafe {
        *(owner.add(TITLE_OWNER_PLAY_GAME_REQUEST_FLAG_OFFSET) as *mut u8) =
            TITLE_OWNER_PLAY_GAME_REQUEST_FLAG_SET;
    }
    // Select the slot STEP_PlayGame loads: its handler reads owner+0xbc and the
    // pair step writes it to GameMan+0x14. Without this it stays -1 and pair bails.
    unsafe { *(owner.add(TITLE_OWNER_PLAY_GAME_SLOT_OFFSET) as *mut i32) = slot };
    unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *mut i32) = TITLE_STEP_PLAY_GAME };
    let state_after = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    FORCE_PLAY_GAME_CALLED.store(TITLE_NATIVE_JOB_CALLED_VALUE, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "force_play_game: set slot={slot} state {state_before}->{state_after} (STEP_PlayGame) tick={tick}"
    ));
    true
}

/// Pseudo-handle for the current process (GetCurrentProcess() is constant -1).
pub(crate) const CURRENT_PROCESS_PSEUDO_HANDLE: isize = -1;
/// Bytes read per ReadProcessMemory call when scanning a region for the title
/// vtable. One syscall per 64KB chunk (then an in-process buffer scan) keeps the
/// fault-tolerant scan fast -- a syscall per 8-byte cursor would stall the thread.
pub(crate) const SCAN_CHUNK_SIZE: usize = 0x10000;

/// Fault-tolerant pointer-sized read via ReadProcessMemory: returns None on
/// unmapped/freed memory instead of raising an access violation. Used by the
/// title-owner scan to survive the TOCTOU race against the booting game.
pub(crate) unsafe fn safe_read_usize(addr: usize) -> Option<usize> {
    let mut value: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let mut read: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut usize as *mut c_void,
            std::mem::size_of::<usize>(),
            &mut read,
        )
    };
    if ok != HOOK_FALSE_RETURN as i32 && read == std::mem::size_of::<usize>() {
        Some(value)
    } else {
        None
    }
}

/// Fault-tolerant i32 read via ReadProcessMemory (None on unmapped memory).
pub(crate) unsafe fn safe_read_i32(addr: usize) -> Option<i32> {
    let mut value: i32 = TITLE_OWNER_SCAN_START_ADDRESS as i32;
    let mut read: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut i32 as *mut c_void,
            std::mem::size_of::<i32>(),
            &mut read,
        )
    };
    if ok != HOOK_FALSE_RETURN as i32 && read == std::mem::size_of::<i32>() {
        Some(value)
    } else {
        None
    }
}

pub(crate) unsafe fn find_title_owner_by_vtable(module_base: usize) -> Option<*mut u8> {
    let target_vtable = module_base.checked_add(TITLE_OWNER_VTABLE_RVA)?;
    let mut scan_buf = vec![MOVIE_SKIP_FLAG_CLEAR; SCAN_CHUNK_SIZE];
    let mut address = TITLE_OWNER_SCAN_START_ADDRESS;
    while address < TITLE_OWNER_SCAN_MAX_ADDRESS {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let queried = unsafe {
            VirtualQuery(
                Some(address as *const c_void),
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if queried == TITLE_OWNER_QUERY_FAILED_BYTES {
            break;
        }

        let base = info.BaseAddress as usize;
        let size = info.RegionSize;
        let next = base.saturating_add(size);
        let state = info.State.0;
        let protect = info.Protect.0;
        if state == MEM_COMMIT_NUMERIC
            && protect & (PAGE_NOACCESS_NUMERIC | PAGE_GUARD_NUMERIC) == PAGE_PROTECTION_NO_FLAGS
            && size >= TITLE_OWNER_STATE_OFFSET + std::mem::size_of::<i32>()
        {
            // Read the region in chunks via ReadProcessMemory (a chunk freed by
            // the booting game returns FALSE instead of faulting), then scan each
            // buffer in-process. One syscall per 64KB keeps the scan fast.
            let mut region_off = TITLE_OWNER_SCAN_START_ADDRESS;
            while region_off < size {
                let chunk = (size - region_off).min(SCAN_CHUNK_SIZE);
                let chunk_base = base + region_off;
                let mut read: usize = TITLE_OWNER_SCAN_START_ADDRESS;
                let ok = unsafe {
                    ReadProcessMemory(
                        CURRENT_PROCESS_PSEUDO_HANDLE,
                        chunk_base as *const c_void,
                        scan_buf.as_mut_ptr() as *mut c_void,
                        chunk,
                        &mut read,
                    )
                };
                if ok != HOOK_FALSE_RETURN as i32 && read >= std::mem::size_of::<usize>() {
                    let mut i = TITLE_OWNER_SCAN_START_ADDRESS;
                    while i + std::mem::size_of::<usize>() <= read {
                        let vtable = usize::from_le_bytes(
                            scan_buf[i..i + std::mem::size_of::<usize>()]
                                .try_into()
                                .unwrap(),
                        );
                        if vtable == target_vtable {
                            let cursor = chunk_base + i;
                            // Validate the per-instance state-table pointer (rejects
                            // the stray .data match 0x1000ffc58); fault-tolerant.
                            let instance_table = unsafe {
                                safe_read_usize(cursor + TITLE_OWNER_INSTANCE_TABLE_OFFSET)
                            };
                            let state_value =
                                unsafe { safe_read_i32(cursor + TITLE_OWNER_STATE_OFFSET) };
                            if instance_table == Some(module_base + INNER_TITLE_STATE_TABLE_RVA)
                                && state_value.is_some_and(|s| {
                                    (TITLE_OWNER_MIN_STATE..=TITLE_OWNER_MAX_STATE).contains(&s)
                                })
                            {
                                return Some(cursor as *mut u8);
                            }
                        }
                        i += TITLE_OWNER_SCAN_ALIGNMENT;
                    }
                }
                region_off = region_off.saturating_add(chunk);
            }
        }

        if next <= address {
            break;
        }
        address = next;
    }
    None
}

pub(crate) unsafe fn title_owner(module_base: usize) -> Option<*mut u8> {
    let cached = TITLE_OWNER_PTR.load(Ordering::SeqCst) as *mut u8;
    if !cached.is_null() {
        return Some(cached);
    }
    // Throttle the full-memory scan: until the owner exists it would otherwise
    // run every frame and cripple FPS (observed ~2 task ticks/s).
    let countdown = TITLE_OWNER_SCAN_COUNTDOWN.load(Ordering::SeqCst);
    if countdown > TITLE_OWNER_SCAN_COUNTDOWN_READY {
        TITLE_OWNER_SCAN_COUNTDOWN.fetch_sub(TITLE_OWNER_SCAN_COUNTDOWN_STEP, Ordering::SeqCst);
        return None;
    }
    TITLE_OWNER_SCAN_COUNTDOWN.store(TITLE_OWNER_SCAN_CALL_INTERVAL, Ordering::SeqCst);
    let found = unsafe { find_title_owner_by_vtable(module_base) }?;
    TITLE_OWNER_PTR.store(found as usize, Ordering::SeqCst);
    let state_value = unsafe { *(found.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    append_autoload_debug(format_args!(
        "native_title_job: captured title owner={found:p} state={state_value}"
    ));
    Some(found)
}

pub(crate) unsafe fn call_native_title_job_once(module_base: usize, tick: u64) -> bool {
    if TITLE_NATIVE_JOB_CALLED.load(Ordering::SeqCst) != TITLE_NATIVE_JOB_NOT_CALLED {
        return true;
    }
    if tick < TITLE_NATIVE_JOB_MIN_TICK {
        let count = TITLE_OWNER_TRACE_COUNT
            .fetch_add(TITLE_TRACE_SEQUENCE_INCREMENT, Ordering::SeqCst)
            + TITLE_TRACE_SEQUENCE_INCREMENT;
        if count <= TITLE_OWNER_TRACE_LIMIT {
            append_autoload_debug(format_args!(
                "native_title_job: waiting for min tick tick={tick} target={TITLE_NATIVE_JOB_MIN_TICK}"
            ));
        }
        return false;
    }
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        let count = TITLE_OWNER_TRACE_COUNT
            .fetch_add(TITLE_TRACE_SEQUENCE_INCREMENT, Ordering::SeqCst)
            + TITLE_TRACE_SEQUENCE_INCREMENT;
        if count <= TITLE_OWNER_TRACE_LIMIT {
            append_autoload_debug(format_args!(
                "native_title_job: waiting for title owner at tick={tick}"
            ));
        }
        return false;
    };

    let state_before = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    let mut task_data = [TITLE_NATIVE_JOB_TASK_DATA_ZERO; TITLE_NATIVE_JOB_TASK_DATA_BYTES];
    let frame_delta = TITLE_NATIVE_JOB_FRAME_DELTA_NUMERATOR / TITLE_NATIVE_JOB_FRAME_RATE;
    task_data[TITLE_NATIVE_JOB_DELTA_OFFSET_START..TITLE_NATIVE_JOB_DELTA_OFFSET_END]
        .copy_from_slice(&frame_delta.to_le_bytes());
    let title_menu_job: unsafe extern "system" fn(*mut u8, *mut c_void) =
        unsafe { std::mem::transmute(module_base + TITLE_MENU_JOB_WAIT_RVA) };
    append_autoload_debug(format_args!(
        "native_title_job: ENTER owner={owner:p} state_before={state_before} tick={tick}"
    ));
    unsafe { title_menu_job(owner, task_data.as_mut_ptr().cast()) };
    let state_after = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    TITLE_NATIVE_JOB_CALLED.store(TITLE_NATIVE_JOB_CALLED_VALUE, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "native_title_job: LEAVE owner={owner:p} state_after={state_after} tick={tick}"
    ));
    true
}

#[derive(Clone, Copy)]
pub(crate) struct MenuTraceSnapshot {
    pub(crate) seq: usize,
    pub(crate) hook_rva: usize,
    pub(crate) table_rva: usize,
    pub(crate) this_ptr: usize,
    pub(crate) state_qword: usize,
    pub(crate) payload_ptr: usize,
}

impl MenuTraceSnapshot {
    pub(crate) fn advanced_from(self, previous: Self) -> bool {
        self.seq != previous.seq
            || self.hook_rva != previous.hook_rva
            || self.table_rva != previous.table_rva
            || self.this_ptr != previous.this_ptr
            || self.state_qword != previous.state_qword
            || self.payload_ptr != previous.payload_ptr
    }

    pub(crate) fn barrier_id(self) -> String {
        format!(
            "hook_0x{:x}/table_{}",
            self.hook_rva,
            trace_rva_label(self.table_rva)
        )
    }

    pub(crate) fn summary(self) -> String {
        format!(
            "last_menu_seq={} hook_rva=0x{:x} table_rva={} this=0x{:x} state_qword=0x{:x} payload_ptr=0x{:x}",
            self.seq,
            self.hook_rva,
            trace_rva_label(self.table_rva),
            self.this_ptr,
            self.state_qword,
            self.payload_ptr
        )
    }
}

pub(crate) fn menu_trace_snapshot() -> MenuTraceSnapshot {
    MenuTraceSnapshot {
        seq: MENU_TRACE_LAST_SEQ.load(Ordering::SeqCst),
        hook_rva: MENU_TRACE_LAST_HOOK_RVA.load(Ordering::SeqCst),
        table_rva: MENU_TRACE_LAST_TABLE_RVA.load(Ordering::SeqCst),
        this_ptr: MENU_TRACE_LAST_THIS.load(Ordering::SeqCst),
        state_qword: MENU_TRACE_LAST_STATE_QWORD.load(Ordering::SeqCst),
        payload_ptr: MENU_TRACE_LAST_PAYLOAD_PTR.load(Ordering::SeqCst),
    }
}

pub(crate) fn trace_rva_label(rva: usize) -> String {
    if rva == TRACE_UNKNOWN_TABLE_RVA as usize {
        "unknown".to_owned()
    } else {
        format!("0x{rva:x}")
    }
}

pub(crate) fn append_confirm_probe(
    phase: &str,
    pulse_seq: usize,
    tick: u64,
    snapshot: MenuTraceSnapshot,
    advanced_after_pulse: Option<bool>,
) {
    let advanced =
        advanced_after_pulse.map_or_else(|| "unknown".to_owned(), |value| value.to_string());
    let line = format!(
        "confirm_probe phase={phase} pulse={pulse_seq} tick={tick} menu_condition[unknown_confirmable_modal] barrier_id={} observed_after_pulse={advanced} confirm_active={} {} {}",
        snapshot.barrier_id(),
        SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst) > NO_SAFE_INPUT_CONFIRM_FRAMES,
        snapshot.summary(),
        game_man_trace_summary()
    );
    append_autoload_debug(format_args!("{line}"));
    append_continue_trace(format_args!("{line}"));
}

pub(crate) unsafe fn menu_task_state_summary(this: *mut c_void) -> (usize, usize, String) {
    if this.is_null() {
        return (
            MENU_TASK_NULL_STATE_QWORD,
            MENU_TASK_NULL_PAYLOAD_PTR,
            "task_state{null=true}".to_owned(),
        );
    }
    let base = this.cast::<u8>();
    let state_qword = unsafe { *(base.cast::<usize>()) };
    let state_code = unsafe { *(base.cast::<i32>()) };
    let state_payload = unsafe { *(base.add(MENU_TASK_STATE_PAYLOAD_CODE_OFFSET).cast::<i32>()) };
    let delay_bits = unsafe { *(base.add(MENU_TASK_STATE_DELAY_OFFSET).cast::<u32>()) };
    let payload_ptr = unsafe { *(base.add(MENU_TASK_STATE_PAYLOAD_PTR_OFFSET).cast::<usize>()) };
    (
        state_qword,
        payload_ptr,
        format!(
            "task_state{{qword=0x{state_qword:x},code={state_code},payload={state_payload},delay_bits=0x{delay_bits:x},payload_ptr=0x{payload_ptr:x}}}"
        ),
    )
}

pub(crate) fn record_menu_trace_snapshot(
    seq: usize,
    hook_rva: u32,
    table_rva: u32,
    this: *mut c_void,
    state_qword: usize,
    payload_ptr: usize,
) {
    MENU_TRACE_LAST_SEQ.store(seq, Ordering::SeqCst);
    MENU_TRACE_LAST_HOOK_RVA.store(hook_rva as usize, Ordering::SeqCst);
    MENU_TRACE_LAST_TABLE_RVA.store(table_rva as usize, Ordering::SeqCst);
    MENU_TRACE_LAST_THIS.store(this as usize, Ordering::SeqCst);
    MENU_TRACE_LAST_STATE_QWORD.store(state_qword, Ordering::SeqCst);
    MENU_TRACE_LAST_PAYLOAD_PTR.store(payload_ptr, Ordering::SeqCst);
}

pub(crate) unsafe fn append_menu_semaphore_trace(
    hook_name: &str,
    phase: &str,
    hook_rva: u32,
    table_rva: u32,
    this: *mut c_void,
) {
    let seq = MENU_TRACE_EVENT_SEQ.fetch_add(MENU_TRACE_EVENT_INCREMENT, Ordering::SeqCst)
        + MENU_TRACE_EVENT_INCREMENT;
    let (state_qword, payload_ptr, task_state) = unsafe { menu_task_state_summary(this) };
    record_menu_trace_snapshot(seq, hook_rva, table_rva, this, state_qword, payload_ptr);
    append_continue_trace(format_args!(
        "menu_semaphore seq={seq} phase={phase} hook={hook_name} hook_rva=0x{hook_rva:x} table_rva={} this={this:p} barrier_id=hook_0x{hook_rva:x}/table_{} confirm_active={} pulse={} {} {} {}",
        trace_rva_label(table_rva as usize),
        trace_rva_label(table_rva as usize),
        SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst) > NO_SAFE_INPUT_CONFIRM_FRAMES,
        SAFE_INPUT_CONFIRM_PULSE_SEQ.load(Ordering::SeqCst),
        task_state,
        trace_callers_summary(),
        game_man_trace_summary()
    ));
}

pub(crate) fn game_man_trace_summary() -> String {
    const GAME_MAN_GLOBAL_RVA: u32 = 0x03d69918;
    const GAME_MAN_SAVE_SLOT_OFFSET: usize = 0xac0;
    const GAME_MAN_REQUESTED_SAVE_SLOT_LOAD_INDEX_OFFSET: usize = 0xb78;
    const GAME_MAN_SAVE_STATE_OFFSET: usize = 0xb80;
    const GAME_MAN_FLAG_B72_OFFSET: usize = 0xb72;
    const GAME_MAN_FLAG_B73_OFFSET: usize = 0xb73;
    const GAME_MAN_FLAG_B74_OFFSET: usize = 0xb74;
    const GAME_MAN_FLAG_B75_OFFSET: usize = 0xb75;
    const GAME_MAN_FLAG_BB8_OFFSET: usize = 0xbb8;
    const GAME_MAN_FLAG_BC4_OFFSET: usize = 0xbc4;
    const GAME_MAN_FLAG_BBC_OFFSET: usize = 0xbbc;
    const GAME_MAN_FLAG_BC0_OFFSET: usize = 0xbc0;

    unsafe {
        let Ok(global) = game_rva(GAME_MAN_GLOBAL_RVA) else {
            return "gm_global_unresolved".to_owned();
        };
        let game_man = *(global as *const *const u8);
        if game_man.is_null() {
            return "gm=null".to_owned();
        }

        let read_i32 = |offset: usize| *(game_man.add(offset) as *const i32);
        let read_u8 = |offset: usize| *game_man.add(offset);
        let requested_slot_index = read_i32(GAME_MAN_REQUESTED_SAVE_SLOT_LOAD_INDEX_OFFSET);
        let save_state = read_i32(GAME_MAN_SAVE_STATE_OFFSET);
        format!(
            "gm={game_man:p} slot={} req_idx={} b78={} state={} b80={} flags{{b72={},b73={},b74={},b75={},bb8={}}} bbc={} bc0={} bc4={}",
            read_i32(GAME_MAN_SAVE_SLOT_OFFSET),
            requested_slot_index,
            requested_slot_index,
            save_state,
            save_state,
            read_u8(GAME_MAN_FLAG_B72_OFFSET),
            read_u8(GAME_MAN_FLAG_B73_OFFSET),
            read_u8(GAME_MAN_FLAG_B74_OFFSET),
            read_u8(GAME_MAN_FLAG_B75_OFFSET),
            read_u8(GAME_MAN_FLAG_BB8_OFFSET),
            read_i32(GAME_MAN_FLAG_BBC_OFFSET),
            read_i32(GAME_MAN_FLAG_BC0_OFFSET),
            read_i32(GAME_MAN_FLAG_BC4_OFFSET),
        )
    }
}

pub(crate) unsafe fn create_continue_trace_hook(
    hooks: &mut Vec<MhHook>,
    name: &str,
    rva: u32,
    hook_impl: *mut c_void,
    original: &AtomicUsize,
) {
    let Ok(addr) = game_rva(rva) else {
        append_continue_trace(format_args!("hook {name}: failed to resolve rva=0x{rva:x}"));
        return;
    };

    match unsafe { MhHook::new(addr as *mut c_void, hook_impl) } {
        Ok(hook) => {
            original.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_continue_trace(format_args!("hook {name}: queue_enable failed: {status:?}"));
            } else {
                append_continue_trace(format_args!(
                    "hook {name}: target=0x{addr:x} trampoline={:p}",
                    hook.trampoline()
                ));
                hooks.push(hook);
            }
        }
        Err(status) => append_continue_trace(format_args!(
            "hook {name}: create failed at 0x{addr:x}: {status:?}"
        )),
    }
}

pub(crate) fn install_continue_trace_hooks() {
    write_bootstrap_event(
        BOOTSTRAP_EVENT_CONTINUE_TRACE_STARTED,
        BOOTSTRAP_DETAIL_START,
    );
    // Local Proton executable RVAs. The shared Ghidra 1.16.1 function starts are
    // currently +0xf0 for these text symbols; these RVAs are verified against
    // /home/banon/.local/share/Steam/.../eldenring.exe sha256
    // 34102b1c08bb5f769a724427a6f70fe29b3b732c31cf73693f861c48d3492ddb.
    const MENU_CONTINUE_WRAPPER_RVA: u32 = 0x0082bac0;
    const MENU_NEW_OR_LOAD_WRAPPER_RVA: u32 = 0x0082ba80;
    const MENU_OTHER_LOAD_WRAPPER_RVA: u32 = 0x0082bb00;
    const SET_SAVE_SLOT_RVA: u32 = 0x0067a810;
    const SAVE_REQUEST_PROFILE_RVA: u32 = 0x0067a420;
    const REQUEST_SAVE_RVA: u32 = 0x0067a520;
    const CURRENT_SLOT_LOAD_RVA: u32 = 0x0067b570;
    const CONTINUE_LOAD_RVA: u32 = 0x0067b750;
    const COMBINED_LOAD_RVA: u32 = 0x0067b940;
    const MAP_LOAD_RVA: u32 = 0x0067bc10;
    const SAVE_LOAD_STATE_INIT_RVA: u32 = 0x0067b030;

    append_continue_trace(format_args!(
        "install_continue_trace_hooks begin {}",
        game_man_trace_summary()
    ));

    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_continue_trace(format_args!("MH_Initialize failed: {status:?}"));
            return;
        }
    }

    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "menu_continue_wrapper",
            MENU_CONTINUE_WRAPPER_RVA,
            menu_continue_wrapper_hook as *mut c_void,
            &MENU_CONTINUE_WRAPPER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "menu_new_or_load_wrapper",
            MENU_NEW_OR_LOAD_WRAPPER_RVA,
            menu_new_or_load_wrapper_hook as *mut c_void,
            &MENU_NEW_OR_LOAD_WRAPPER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "menu_other_load_wrapper",
            MENU_OTHER_LOAD_WRAPPER_RVA,
            menu_other_load_wrapper_hook as *mut c_void,
            &MENU_OTHER_LOAD_WRAPPER_ORIG,
        );
        if trace_menu_task_update_enabled() {
            create_continue_trace_hook(
                &mut hooks,
                "menu_task_update_wrapper",
                TRACE_MENU_TASK_UPDATE_WRAPPER_RVA,
                menu_task_update_wrapper_hook as *mut c_void,
                &MENU_TASK_UPDATE_WRAPPER_ORIG,
            );
        } else {
            append_continue_trace(format_args!(
                "menu_task_update_wrapper trace skipped by default; set ER_EFFECTS_TRACE_MENU_TASK_UPDATE=1 for invasive pump diagnostics"
            ));
        }
        create_continue_trace_hook(
            &mut hooks,
            "task_enqueue_7a7b60",
            TRACE_TASK_ENQUEUE_RVA,
            task_enqueue_hook as *mut c_void,
            &TASK_ENQUEUE_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "set_save_slot",
            SET_SAVE_SLOT_RVA,
            set_save_slot_hook as *mut c_void,
            &SET_SAVE_SLOT_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "save_request_profile",
            SAVE_REQUEST_PROFILE_RVA,
            save_request_profile_hook as *mut c_void,
            &SAVE_REQUEST_PROFILE_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "request_save",
            REQUEST_SAVE_RVA,
            request_save_hook as *mut c_void,
            &REQUEST_SAVE_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "current_slot_load_67b570",
            CURRENT_SLOT_LOAD_RVA,
            current_slot_load_hook as *mut c_void,
            &CURRENT_SLOT_LOAD_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "continue_load_67b750",
            CONTINUE_LOAD_RVA,
            continue_load_hook as *mut c_void,
            &CONTINUE_LOAD_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "combined_load_67b940",
            COMBINED_LOAD_RVA,
            combined_load_hook as *mut c_void,
            &COMBINED_LOAD_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "map_load_67bc10",
            MAP_LOAD_RVA,
            map_load_hook as *mut c_void,
            &MAP_LOAD_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "save_load_state_init_67b030",
            SAVE_LOAD_STATE_INIT_RVA,
            save_load_state_init_hook as *mut c_void,
            &SAVE_LOAD_STATE_INIT_ORIG,
        );
        // b80 save-mount capture: the 5 functions that drive the slot deserialize. A real
        // user-driven .co2 load through these pins the exact call order + args + which fn
        // populates io18/io20 + which transitions b80 + which applies the character, so we
        // can replicate it with slot-int primitives (no synthetic-owner save-write).
        create_continue_trace_hook(
            &mut hooks,
            "b80_preview_67b4e0",
            LOAD_INITIATOR_RVA as u32,
            b80_preview_initiator_hook as *mut c_void,
            &B80_PREVIEW_INITIATOR_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "b80_loadsavedata_67b200",
            B80_LOAD_SAVE_DATA_INITIATOR_RVA as u32,
            b80_loadsavedata_hook as *mut c_void,
            &B80_LOAD_SAVE_DATA_INITIATOR_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "b80_fullload_67b1a0",
            B80_FULL_LOAD_INITIATOR_RVA as u32,
            b80_fullload_hook as *mut c_void,
            &B80_FULL_LOAD_INITIATOR_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "b80_poll_679180",
            B80_POLL_RVA as u32,
            b80_poll_hook as *mut c_void,
            &B80_POLL_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "b80_deserialize_67b290",
            DESERIALIZE_SLOT_RVA as u32,
            b80_deserialize_hook as *mut c_void,
            &B80_DESERIALIZE_ORIG,
        );
        // NOTE: the c30_writer 0x67bd70 hook is NOT installed here. It is installed
        // UNCONDITIONALLY at process attach via install_c30_writer_hook (mirroring the
        // MenuWindow-latch precedent) so the SAVE-SAFE c30-write diagnostic is always
        // armed without requiring the continue-trace path. Installing it twice on the
        // same address would make the second MhHook::new fail, so it lives only there.
        // MENU-UI capture (Path B state-stepper). One real navigation through these pins the
        // this-pointers + construction order + call sequence for the 4 user interactions:
        // SetState (state machine), Continue confirm, ProfileLoadDialog activate (both
        // variants), the enter-Load-Game builder, the selector-step tick, and the mount.
        const CAP_SETSTATE_RVA: u32 = 0x00b0d960;
        const CAP_CONTINUE_CONFIRM_RVA: u32 = 0x00b0e180;
        const CAP_LOAD_ACTIVATE_RVA: u32 = 0x009a4670;
        const CAP_LOAD_ACTIVATE2_RVA: u32 = 0x009ac760;
        const CAP_BUILDER_RVA: u32 = 0x00826510;
        const CAP_SELECTOR_TICK_RVA: u32 = 0x00826d50;
        const CAP_MENU_DESER_RVA: u32 = 0x0082c240;
        const CAP_DIALOG_FACTORY_RVA: u32 = 0x0081ead0;
        create_continue_trace_hook(
            &mut hooks,
            "cap_setstate_b0d960",
            CAP_SETSTATE_RVA,
            cap_setstate_hook as *mut c_void,
            &CAP_SETSTATE_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_continue_confirm_b0e180",
            CAP_CONTINUE_CONFIRM_RVA,
            cap_continue_confirm_hook as *mut c_void,
            &CAP_CONTINUE_CONFIRM_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_load_activate_9a4670",
            CAP_LOAD_ACTIVATE_RVA,
            cap_load_activate_hook as *mut c_void,
            &CAP_LOAD_ACTIVATE_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_load_activate2_9ac760",
            CAP_LOAD_ACTIVATE2_RVA,
            cap_load_activate2_hook as *mut c_void,
            &CAP_LOAD_ACTIVATE2_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_builder_826510",
            CAP_BUILDER_RVA,
            cap_builder_hook as *mut c_void,
            &CAP_BUILDER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_selector_tick_826d50",
            CAP_SELECTOR_TICK_RVA,
            cap_selector_tick_hook as *mut c_void,
            &CAP_SELECTOR_TICK_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_menu_deser_82c240",
            CAP_MENU_DESER_RVA,
            cap_menu_deser_hook as *mut c_void,
            &CAP_MENU_DESER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_dialog_factory_81ead0",
            CAP_DIALOG_FACTORY_RVA,
            cap_dialog_factory_hook as *mut c_void,
            &CAP_DIALOG_FACTORY_ORIG,
        );
        // Menu-item Update 0x1407ad1c0: capture the live Load-Game item (functor ->
        // dialog_factory) by letting the native pump walk its own CSMenu tree.
        create_continue_trace_hook(
            &mut hooks,
            "cap_menu_item_update_7ad1c0",
            MENU_ITEM_UPDATE_RVA,
            cap_menu_item_update_hook as *mut c_void,
            &MENU_ITEM_UPDATE_ORIG,
        );
        // Sequence child-iterator 0x1407aa1f0: enumerate every Sequence's children to capture
        // the Load-Game leaf d180 even though it does not tick (only the focused entry ticks
        // the leaf Update above).
        create_continue_trace_hook(
            &mut hooks,
            "cap_sequence_iter_7aa1f0",
            SEQUENCE_ITER_RVA,
            cap_sequence_iter_hook as *mut c_void,
            &SEQUENCE_ITER_ORIG,
        );
        // CSMenu controller ctor 0x1409060d0: latch router_this (owns the selectable-row vector
        // at +0x1290) -- it is NOT field-linked from the TitleTopDialog, so capturing it at
        // construction is how the own-stepper reaches the Continue/Load rows zero-input.
        create_continue_trace_hook(
            &mut hooks,
            "cap_csmenu_ctor_9060d8",
            CSMENU_CTOR_RVA,
            cap_csmenu_ctor_hook as *mut c_void,
            &CAP_CSMENU_CTOR_ORIG,
        );
        // Row-push functions (reliable .text): if either fires headless the rows materialize
        // zero-input; if neither does, the interactive menu controller is input-instantiated.
        create_continue_trace_hook(
            &mut hooks,
            "cap_rebuild_rows_78d2c0",
            REBUILD_ROWS_RVA,
            cap_rebuild_rows_hook as *mut c_void,
            &CAP_REBUILD_ROWS_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_append_one_78eea0",
            APPEND_ONE_RVA,
            cap_append_one_hook as *mut c_void,
            &CAP_APPEND_ONE_ORIG,
        );
    }

    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            write_bootstrap_event(
                BOOTSTRAP_EVENT_CONTINUE_TRACE_APPLIED,
                BOOTSTRAP_DETAIL_DONE,
            );
            append_continue_trace(format_args!(
                "install_continue_trace_hooks applied count={} {}",
                hooks.len(),
                game_man_trace_summary()
            ));
        }
        status => {
            let detail = format!("MH_ApplyQueued failed: {status:?}");
            write_bootstrap_event(BOOTSTRAP_EVENT_CONTINUE_TRACE_APPLY_FAILED, &detail);
            append_continue_trace(format_args!("{detail}"));
        }
    }

    std::mem::forget(hooks);
}

pub(crate) unsafe fn call_wrapper_original(
    original: &AtomicUsize,
    this: *mut c_void,
) -> Option<*mut c_void> {
    let original = original.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let original: unsafe extern "system" fn(*mut c_void) -> *mut c_void =
        unsafe { std::mem::transmute(original) };
    Some(unsafe { original(this) })
}

pub(crate) unsafe fn call_bool3_original(
    original: &AtomicUsize,
    arg0: i32,
    arg1: u8,
    arg2: u8,
) -> Option<u8> {
    let original = original.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let original: unsafe extern "system" fn(i32, u8, u8) -> u8 =
        unsafe { std::mem::transmute(original) };
    Some(unsafe { original(arg0, arg1, arg2) })
}

pub(crate) unsafe fn call_task_enqueue_original(
    arg0: *mut c_void,
    arg1: *mut c_void,
) -> Option<*mut c_void> {
    let original = TASK_ENQUEUE_ORIG.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let original: unsafe extern "system" fn(*mut c_void, *mut c_void) -> *mut c_void =
        unsafe { std::mem::transmute(original) };
    Some(unsafe { original(arg0, arg1) })
}

/// Defensive default when a b80 trampoline is somehow unset (dead branch: if our hook
/// runs, MhHook installed and the trampoline is set).
const B80_HOOK_DEFAULT_RET: i32 = 0;

/// State snapshot for the b80 save-mount capture: the GameMan load-phase fields plus the
/// iodev request-handle pair the poll keys on. Logged at ENTER and LEAVE of each hooked
/// b80 function so a real user-driven load pins which fn populates io18/io20, transitions
/// b80 0->1/2->3, and writes c30/ac0 (the character-apply). io18 && io20 set == the
/// deserialize-ready signature (real-load-c30-mount-write-confirmed-seamless-2026).
pub(crate) fn b80_mount_trace_summary() -> String {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let Ok(base) = game_module_base() else {
        return "base_unresolved".to_owned();
    };
    let gm = unsafe { *((base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) as *const usize) };
    let read_gm = |off: usize| {
        if gm != null {
            unsafe { *((gm + off) as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        }
    };
    let b80 = read_gm(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
    let ac0 = read_gm(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
    let c30 = read_gm(GAME_MAN_SAVED_MAP_C30_OFFSET);
    let b78 = read_gm(GAME_MAN_REQUESTED_SLOT_B78_OFFSET);
    let iodev = unsafe { *((base + IODEV_GLOBAL_RVA) as *const usize) };
    let read_io = |off: usize| {
        if iodev != null {
            unsafe { *((iodev + off) as *const usize) }
        } else {
            null
        }
    };
    let io10 = read_io(IODEV_INFLIGHT_10_OFFSET);
    let io18 = read_io(IODEV_REQHANDLE_18_OFFSET);
    let io20 = read_io(IODEV_REQHANDLE_20_OFFSET);
    format!(
        "b80={b80} ac0={ac0} c30=0x{c30:x} b78={b78} io10=0x{io10:x} io18=0x{io18:x} io20=0x{io20:x}"
    )
}

/// Call an original slot-int b80 initiator/deserialize (fastcall, ecx=slot). Returns the
/// full eax the original produced so the game's caller sees the unmodified result.
unsafe fn call_b80_initiator_original(original: &AtomicUsize, slot: i32) -> i32 {
    let original = original.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return B80_HOOK_DEFAULT_RET;
    }
    let original: unsafe extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(original) };
    unsafe { original(slot) }
}

/// Call the original b80 poll 0x140679180(cl,dl). Returns its full eax (0 ready /
/// 1 in-progress / else error) so the dispatcher's switch is unchanged.
unsafe fn call_b80_poll_original(original: &AtomicUsize, arg0: u8, arg1: u8) -> i32 {
    let original = original.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return B80_HOOK_DEFAULT_RET;
    }
    let original: unsafe extern "system" fn(u8, u8) -> i32 =
        unsafe { std::mem::transmute(original) };
    unsafe { original(arg0, arg1) }
}

pub(crate) unsafe extern "system" fn b80_preview_initiator_hook(slot: i32) -> i32 {
    append_continue_trace(format_args!(
        "b80_preview_67b4e0 ENTER slot={slot} {}",
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_b80_initiator_original(&B80_PREVIEW_INITIATOR_ORIG, slot) };
    append_continue_trace(format_args!(
        "b80_preview_67b4e0 LEAVE slot={slot} ret={ret} {}",
        b80_mount_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn b80_loadsavedata_hook(slot: i32) -> i32 {
    append_continue_trace(format_args!(
        "b80_loadsavedata_67b200 ENTER slot={slot} {}",
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_b80_initiator_original(&B80_LOAD_SAVE_DATA_INITIATOR_ORIG, slot) };
    append_continue_trace(format_args!(
        "b80_loadsavedata_67b200 LEAVE slot={slot} ret={ret} {}",
        b80_mount_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn b80_fullload_hook(slot: i32) -> i32 {
    append_continue_trace(format_args!(
        "b80_fullload_67b1a0 ENTER slot={slot} {}",
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_b80_initiator_original(&B80_FULL_LOAD_INITIATOR_ORIG, slot) };
    append_continue_trace(format_args!(
        "b80_fullload_67b1a0 LEAVE slot={slot} ret={ret} {}",
        b80_mount_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn b80_poll_hook(arg0: u8, arg1: u8) -> i32 {
    append_continue_trace(format_args!(
        "b80_poll_679180 ENTER arg0={arg0} arg1={arg1} {}",
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_b80_poll_original(&B80_POLL_ORIG, arg0, arg1) };
    append_continue_trace(format_args!(
        "b80_poll_679180 LEAVE ret={ret} {}",
        b80_mount_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn b80_deserialize_hook(slot: i32) -> i32 {
    append_continue_trace(format_args!(
        "b80_deserialize_67b290 ENTER slot={slot} {}",
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_b80_initiator_original(&B80_DESERIALIZE_ORIG, slot) };
    append_continue_trace(format_args!(
        "b80_deserialize_67b290 LEAVE slot={slot} ret={ret} {}",
        b80_mount_trace_summary()
    ));
    ret
}

/// The SOLE GameMan+0xc30 writer 0x14067bd70(rcx=GameMan, rdx=buf, r8d=size). Logs the
/// CALLER STACK (which deserializer drove the c30 write -- the Wine-safe replacement
/// for the hardware watchpoint) + the mount state, then chains the original. If this
/// never fires during a Seamless .co2 load, ERSC writes c30 from its own module.
pub(crate) unsafe extern "system" fn c30_writer_hook(
    game_man: usize,
    buffer: usize,
    size: u32,
) -> usize {
    // SAVE-SAFE diagnostic (NO SetState5, NO save write): a pure passthrough that forwards
    // ALL args + returns the original's result. Rate-limited to the first few calls (the cold
    // deserialize drives a small bounded number of c30-writer entries). On ENTER we log the gate
    // [0x143d68078] (null -> writer returns without writing), c30 BEFORE, and a window of the
    // resident save buffer (rdx) so the REAL target map record can be spotted offline. On LEAVE
    // we log the return (al) + c30 AFTER, so we can see whether 0x67bd70 ran, whether it changed
    // c30, and to what. (coldmount-c30-is-the-single-key-write-conditions-and-recipe-2026)
    const C30_LOG_INC: usize = 1;
    const HEX_BYTES_PER_LINE: usize = 16;
    let log_n = C30_WRITER_LOG_COUNT.fetch_add(C30_LOG_INC, Ordering::SeqCst);
    let do_log = log_n < C30_WRITER_LOG_MAX;
    if do_log {
        let gate = game_module_base()
            .ok()
            .map(|base| unsafe { *((base + SAVE_DATA_SUBSYSTEM_GATE_RVA) as *const usize) })
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let c30_before = unsafe { *((game_man + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) };
        // Hex window of the resident 0x280000 save buffer header so the map record is visible.
        let mut hex = String::new();
        const BUFFER_DUMP_START: usize = 0;
        for i in BUFFER_DUMP_START..C30_WRITER_BUFFER_DUMP_BYTES {
            if i % HEX_BYTES_PER_LINE == TITLE_OWNER_SCAN_START_ADDRESS {
                hex.push(' ');
            }
            let byte = unsafe { *((buffer + i) as *const u8) };
            let _ = write!(hex, "{byte:02x}");
        }
        append_continue_trace(format_args!(
            "c30_writer_67bd70 ENTER#{log_n} game_man=0x{game_man:x} buf=0x{buffer:x} size=0x{size:x} gate(0x143d68078)=0x{gate:x} c30_before=0x{c30_before:x} buf[0..0x{:x}]={hex} {} {}",
            C30_WRITER_BUFFER_DUMP_BYTES,
            b80_mount_trace_summary(),
            trace_callers_summary()
        ));
    }
    let original = C30_WRITER_ORIG.load(Ordering::SeqCst);
    let ret = if original == HOOK_ORIGINAL_UNSET {
        B80_HOOK_DEFAULT_RET as usize
    } else {
        let original: unsafe extern "system" fn(usize, usize, u32) -> usize =
            unsafe { std::mem::transmute(original) };
        unsafe { original(game_man, buffer, size) }
    };
    if do_log {
        let c30_after = unsafe { *((game_man + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) };
        append_continue_trace(format_args!(
            "c30_writer_67bd70 LEAVE#{log_n} ret=0x{ret:x} c30_after=0x{c30_after:x} {}",
            b80_mount_trace_summary()
        ));
    }
    ret
}

pub(crate) unsafe extern "system" fn menu_continue_wrapper_hook(this: *mut c_void) -> *mut c_void {
    unsafe {
        append_menu_semaphore_trace(
            "menu_continue_wrapper",
            "ENTER",
            TRACE_MENU_CONTINUE_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            this,
        )
    };
    let result =
        unsafe { call_wrapper_original(&MENU_CONTINUE_WRAPPER_ORIG, this) }.unwrap_or(this);
    unsafe {
        append_menu_semaphore_trace(
            "menu_continue_wrapper",
            "LEAVE",
            TRACE_MENU_CONTINUE_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            result,
        )
    };
    result
}

pub(crate) unsafe extern "system" fn menu_new_or_load_wrapper_hook(
    this: *mut c_void,
) -> *mut c_void {
    unsafe {
        append_menu_semaphore_trace(
            "menu_new_or_load_wrapper",
            "ENTER",
            TRACE_MENU_NEW_OR_LOAD_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            this,
        )
    };
    let result =
        unsafe { call_wrapper_original(&MENU_NEW_OR_LOAD_WRAPPER_ORIG, this) }.unwrap_or(this);
    unsafe {
        append_menu_semaphore_trace(
            "menu_new_or_load_wrapper",
            "LEAVE",
            TRACE_MENU_NEW_OR_LOAD_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            result,
        )
    };
    result
}

pub(crate) unsafe extern "system" fn menu_other_load_wrapper_hook(
    this: *mut c_void,
) -> *mut c_void {
    unsafe {
        append_menu_semaphore_trace(
            "menu_other_load_wrapper",
            "ENTER",
            TRACE_MENU_OTHER_LOAD_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            this,
        )
    };
    let result =
        unsafe { call_wrapper_original(&MENU_OTHER_LOAD_WRAPPER_ORIG, this) }.unwrap_or(this);
    unsafe {
        append_menu_semaphore_trace(
            "menu_other_load_wrapper",
            "LEAVE",
            TRACE_MENU_OTHER_LOAD_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            result,
        )
    };
    result
}

/// Forward a captured menu-UI call through its trampoline. Uniform 4-arg fastcall: the
/// integer arg registers (rcx/rdx/r8/r9) pass through; callees taking fewer args ignore the
/// rest, and none of the captured targets take >4 integer args or float args. Returns rax.
unsafe fn call_cap_original(orig: &AtomicUsize, a: usize, b: usize, c: usize, d: usize) -> usize {
    let original = orig.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(original) };
    unsafe { f(a, b, c, d) }
}

/// Title CSMenu controller ctor 0x1409060d0 (real prologue entry; doc's 0x9060d8 was mid-
/// prologue): latches `router_this` (the object owning the
/// selectable Continue/Load/NewGame row vector at +0x1290) when its primary vtable
/// (runtime `base+0x2afa070`) is installed. router_this is NOT field-linked from the
/// TitleTopDialog, so this ctor capture is how the own-stepper obtains it. Pure observe +
/// pass-through; latches the first matching controller.
pub(crate) unsafe extern "system" fn cap_csmenu_ctor_hook(
    this: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const ROUTER_VEC_BEGIN_1290: usize = 0x1290;
    const ROUTER_VEC_END_1298: usize = 0x1298;
    let ret = unsafe { call_cap_original(&CAP_CSMENU_CTOR_ORIG, this, b, c, d) };
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != NULL {
            own
        } else {
            game_module_base().unwrap_or(NULL)
        }
    };
    if this != NULL && base != NULL {
        let vt = unsafe { safe_read_usize(this) }.unwrap_or(NULL);
        let vt_rva = vt.wrapping_sub(base);
        let matched = vt == base + ROUTER_THIS_VTABLE_RVA;
        if matched {
            MENU_ROUTER_THIS.store(this, Ordering::SeqCst);
        }
        // Log the first N constructions REGARDLESS of match: reveals whether this ctor fires
        // headless at all and the ACTUAL installed runtime vtable (vt_rva), so the inferred
        // ROUTER_THIS_VTABLE_RVA=0x2afa070 (derived via a +0xe00 dump skew, not measured) can be
        // corrected if wrong.
        let n = CAP_CSMENU_CTOR_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        if n < CAP_CSMENU_CTOR_LOG_FIRST {
            let vb = unsafe { safe_read_usize(this + ROUTER_VEC_BEGIN_1290) }.unwrap_or(NULL);
            let ve = unsafe { safe_read_usize(this + ROUTER_VEC_END_1298) }.unwrap_or(NULL);
            append_continue_trace(format_args!(
                "CAP csmenu_ctor #{n} this=0x{this:x} vt=0x{vt:x} vt_rva=0x{vt_rva:x} matched={matched} vec=[0x{vb:x}..0x{ve:x}] {}",
                trace_callers_summary()
            ));
        }
    }
    ret
}

/// Post-build scan of a row container (`rebuild_rows`/`append_one` rcx). The generic FD4 list
/// builder fires for EVERY menu list, so the title menu is identified by CONTENT: a row whose
/// action functor ([entry+0xf8] -> [+0] vtable -> [+0x10] _Do_call) chains to dialog_factory
/// 0x14081ead0 (Load-Game) or continue_confirm 0x140b0e180 (Continue). Captures the Load-Game /
/// Continue ROW ENTRIES (and router_this = container-0x1290) when found. Pure reads + classify
/// (the original already ran) -> save-safe. Called AFTER the original builds the rows.
unsafe fn inspect_row_container(tag: &str, container: usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const ENTRY_STRIDE_210: usize = 0x210;
    const ENTRY_ACTION_F8: usize = 0xf8;
    const ACTION_DOCALL_10: usize = 0x10;
    const ROW_VEC_OFFSET_1290: usize = 0x1290;
    const DIALOG_FACTORY_RVA: usize = 0x0081ead0;
    const PROBE_ENTRIES: usize = 8;
    const PROBE_START: usize = 0;
    const PROBE_STEP: usize = 1;
    const JMP_HOPS: usize = 5;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    if container == NULL {
        return;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != NULL {
            own
        } else {
            game_module_base().unwrap_or(NULL)
        }
    };
    if base == NULL {
        return;
    }
    let factory = base + DIALOG_FACTORY_RVA;
    let confirm = base + CONTINUE_CONFIRM_RVA;
    let begin = unsafe { safe_read_usize(container) }.unwrap_or(NULL);
    if begin == NULL {
        return;
    }
    let mut load_entry: usize = NULL;
    let mut cont_entry: usize = NULL;
    let mut i = PROBE_START;
    while i < PROBE_ENTRIES {
        let entry = begin + i * ENTRY_STRIDE_210;
        let action = unsafe { safe_read_usize(entry + ENTRY_ACTION_F8) }.unwrap_or(NULL);
        if action != NULL {
            let avt = unsafe { safe_read_usize(action) }.unwrap_or(NULL);
            if avt != NULL {
                let mut tgt = unsafe { safe_read_usize(avt + ACTION_DOCALL_10) }.unwrap_or(NULL);
                let mut hop = HOP_START;
                while hop < JMP_HOPS && tgt != NULL {
                    if tgt == factory {
                        load_entry = entry;
                        break;
                    }
                    if tgt == confirm {
                        cont_entry = entry;
                        break;
                    }
                    match unsafe { decode_thunk_hop(tgt) } {
                        Some(next) => tgt = next,
                        None => break,
                    }
                    hop += HOP_STEP;
                }
            }
        }
        i += PROBE_STEP;
    }
    if load_entry == NULL && cont_entry == NULL {
        return;
    }
    // This container IS the title menu row list. Latch the entries + a router_this candidate.
    if load_entry != NULL {
        MENU_LOADGAME_ROW_ENTRY.store(load_entry, Ordering::SeqCst);
    }
    if cont_entry != NULL {
        MENU_CONTINUE_ROW_ENTRY.store(cont_entry, Ordering::SeqCst);
    }
    let router_this = container.wrapping_sub(ROW_VEC_OFFSET_1290);
    MENU_ROUTER_THIS.store(router_this, Ordering::SeqCst);
    let n = CAP_ROW_PUSH_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n < CAP_ROW_PUSH_LOG_FIRST {
        let rvt = unsafe { safe_read_usize(router_this) }.unwrap_or(NULL);
        append_continue_trace(format_args!(
            "CAP row_push[{tag}] TITLE-MENU container=0x{container:x} begin=0x{begin:x} load_entry=0x{load_entry:x} cont_entry=0x{cont_entry:x} router_this?=0x{router_this:x} rvt=0x{rvt:x} {}",
            trace_callers_summary()
        ));
    }
}

/// rebuild_rows 0x14078d2c0(rcx=list-model container, rdx=src iterator pair): bulk-emplaces the
/// Continue/Load/NewGame rows. Firing headless proves the rows materialize zero-input; the
/// post-build scan isolates the title menu by row CONTENT.
pub(crate) unsafe extern "system" fn cap_rebuild_rows_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let ret = unsafe { call_cap_original(&CAP_REBUILD_ROWS_ORIG, a, b, c, d) };
    unsafe { log_row_push_caller("rebuild", a) };
    unsafe { inspect_row_container("rebuild", a) };
    ret
}

/// append_one 0x14078eea0(rcx=list-model, r8=&idx): single-row emplace.
pub(crate) unsafe extern "system" fn cap_append_one_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let ret = unsafe { call_cap_original(&CAP_APPEND_ONE_ORIG, a, b, c, d) };
    unsafe { log_row_push_caller("append", a) };
    unsafe { inspect_row_container("append", a) };
    ret
}

/// UNCONDITIONAL instrument-capture: log container + row-vector size + caller stack for the
/// first N rebuild_rows/append_one fires, regardless of content. This pins WHAT triggers the
/// TitleTopDialog CSMenu row populate (the input/focus-gated step confirmed missing zero-input).
/// Pure reads; the original already ran -> save-safe.
unsafe fn log_row_push_caller(tag: &str, container: usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const ROW_VEC_BEGIN_1290: usize = 0x1290;
    const ROW_VEC_END_1298: usize = 0x1298;
    let n = CAP_ROW_PUSH_ALLFIRE_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n >= CAP_ROW_PUSH_ALLFIRE_LOG_FIRST {
        return;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != NULL {
            own
        } else {
            game_module_base().unwrap_or(NULL)
        }
    };
    // container is the list-model; router_this back-ptr at [container+8], its row vector lives at
    // router_this+0x1290. Also probe the container itself in case it IS router_this.
    let backptr = unsafe { safe_read_usize(container + ROW_CONTAINER_BACKPTR_8) }.unwrap_or(NULL);
    let vb = unsafe { safe_read_usize(container + ROW_VEC_BEGIN_1290) }.unwrap_or(NULL);
    let ve = unsafe { safe_read_usize(container + ROW_VEC_END_1298) }.unwrap_or(NULL);
    let cvt = unsafe { safe_read_usize(container) }.unwrap_or(NULL);
    let cvt_rva = if base != NULL {
        cvt.wrapping_sub(base)
    } else {
        cvt
    };
    append_continue_trace(format_args!(
        "CAP row_push_ALL[{tag}] #{n} container=0x{container:x} cvt=0x{cvt:x}(rva 0x{cvt_rva:x}) backptr=0x{backptr:x} vec=[0x{vb:x}..0x{ve:x}] {}",
        trace_callers_summary()
    ));
}

/// SetState 0x140b0d960(this, state): the title state machine setter. Logging every call
/// reveals the press-any-key advance + Continue's SetState(5) sequence.
pub(crate) unsafe extern "system" fn cap_setstate_hook(
    this: usize,
    state: usize,
    c: usize,
    d: usize,
) -> usize {
    append_continue_trace(format_args!(
        "CAP setstate this=0x{this:x} state={} {} {}",
        state as i32,
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    unsafe { call_cap_original(&CAP_SETSTATE_ORIG, this, state, c, d) }
}

/// Continue confirm 0x140b0e180(this): reads GameMan+0xc30 into owner+0xbc then SetState(5).
pub(crate) unsafe extern "system" fn cap_continue_confirm_hook(
    this: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let owner = if this != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe {
            *((this + OWN_STEPPER_SHIM_OWNER_IDX * core::mem::size_of::<usize>()) as *const usize)
        }
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    append_continue_trace(format_args!(
        "CAP continue_confirm this=0x{this:x} owner=0x{owner:x} {} {}",
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    unsafe { call_cap_original(&CAP_CONTINUE_CONFIRM_ORIG, this, b, c, d) }
}

/// Load activate 0x1409a4670 = CS::ProfileLoadDialog vtable slot 20 (this = the dialog).
pub(crate) unsafe extern "system" fn cap_load_activate_hook(
    this: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    append_continue_trace(format_args!(
        "CAP load_activate(slot20) dialog_this=0x{this:x} a1=0x{b:x} a2=0x{c:x} a3=0x{d:x} {} {}",
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    unsafe { call_cap_original(&CAP_LOAD_ACTIVATE_ORIG, this, b, c, d) }
}

/// Load activate variant 0x1409ac760 (global-slot path).
pub(crate) unsafe extern "system" fn cap_load_activate2_hook(
    this: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    append_continue_trace(format_args!(
        "CAP load_activate2 this=0x{this:x} a1=0x{b:x} a2=0x{c:x} a3=0x{d:x} {} {}",
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    unsafe { call_cap_original(&CAP_LOAD_ACTIVATE2_ORIG, this, b, c, d) }
}

/// Enter-Load-Game builder 0x140826510(owner, rdx, r8d=slot, r9) -> selector step.
pub(crate) unsafe extern "system" fn cap_builder_hook(
    owner: usize,
    rdx: usize,
    slot: usize,
    r9: usize,
) -> usize {
    append_continue_trace(format_args!(
        "CAP builder owner=0x{owner:x} slot={} rdx=0x{rdx:x} r9=0x{r9:x} {} {}",
        slot as i32,
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    unsafe { call_cap_original(&CAP_BUILDER_ORIG, owner, rdx, slot, r9) }
}

/// Selector-owner step tick 0x140826d50(step, ctx, result). Rate-limited (it ticks every
/// frame). Logs the step this, its +0x68 install flag, and the slot at ctx[0].
pub(crate) unsafe extern "system" fn cap_selector_tick_hook(
    step: usize,
    ctx: usize,
    result: usize,
    d: usize,
) -> usize {
    let n = CAP_SELECTOR_TICK_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n < CAP_SELECTOR_TICK_LOG_FIRST
        || n % CAP_SELECTOR_TICK_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS
    {
        let installed = if step != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((step + SELECTOR_STEP_INSTALL_FLAG_68_OFFSET) as *const u8) as i32 }
        } else {
            TITLE_STATE_OWNER_GONE
        };
        let ctx_slot = if ctx != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *(ctx as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        };
        append_continue_trace(format_args!(
            "CAP selector_tick #{n} step=0x{step:x} ctx=0x{ctx:x} installed={installed} ctx_slot={ctx_slot} {}",
            b80_mount_trace_summary()
        ));
    }
    unsafe { call_cap_original(&CAP_SELECTOR_TICK_ORIG, step, ctx, result, d) }
}

/// ProfileLoadDialog factory 0x14081ead0(rcx=ctx, rdx): builds the Load-Game dialog when the
/// main-menu "Load Game" item is activated. The caller backtrace pins the navigation chain.
pub(crate) unsafe extern "system" fn cap_dialog_factory_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    // Capture ALL four register args (rcx/rdx/r8/r9) AND a window of the rcx capture object so the
    // headless PATH-3-direct replay can reconstruct the exact factory invocation. The native
    // _Do_call thunk 0x140820c60 does `add rcx,8` before jmping here, so rcx (=a) is the lambda
    // capture state past the _Func_impl header; the ctor reads the owner from a field of it. Pure
    // reads + pass-through -> save-safe.
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const CAP_START: usize = 0;
    const CAP_WINDOW: usize = 7;
    const CAP_STEP: usize = 1;
    const PTR_SIZE: usize = 8;
    let mut capdump = String::new();
    // Dump [a-8 .. a+0x30] (the _Func_impl vtable at a-8, then capture fields).
    let mut i: usize = CAP_START;
    while i < CAP_WINDOW {
        let off = i * PTR_SIZE;
        let addr = a.wrapping_sub(PTR_SIZE).wrapping_add(off);
        let v = unsafe { safe_read_usize(addr) }.unwrap_or(NULL);
        capdump.push_str(&format!(" [rcx-8+0x{off:x}]=0x{v:x}"));
        i += CAP_STEP;
    }
    let rdx0 = unsafe { safe_read_usize(b) }.unwrap_or(NULL);
    let rdx8 = unsafe { safe_read_usize(b.wrapping_add(PTR_SIZE)) }.unwrap_or(NULL);
    append_continue_trace(format_args!(
        "CAP dialog_factory ENTER rcx=0x{a:x} rdx=0x{b:x} r8=0x{c:x} r9=0x{d:x} [rdx]=0x{rdx0:x} [rdx+8]=0x{rdx8:x}{capdump} {} {}",
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_cap_original(&CAP_DIALOG_FACTORY_ORIG, a, b, c, d) };
    let ret_vt = if ret != NULL {
        unsafe { safe_read_usize(ret) }.unwrap_or(NULL)
    } else {
        NULL
    };
    append_continue_trace(format_args!(
        "CAP dialog_factory LEAVE dialog_this=0x{ret:x} dialog_vt=0x{ret_vt:x}"
    ));
    ret
}

/// Menu deserialize 0x14082c240(this, ctx): the real mount (writes GameMan+0xc30 + char).
pub(crate) unsafe extern "system" fn cap_menu_deser_hook(
    this: usize,
    ctx: usize,
    c: usize,
    d: usize,
) -> usize {
    let ctx_slot = if ctx != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { *(ctx as *const i32) }
    } else {
        TITLE_STATE_OWNER_GONE
    };
    append_continue_trace(format_args!(
        "CAP menu_deser ENTER this=0x{this:x} ctx=0x{ctx:x} ctx_slot={ctx_slot} {} {}",
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_cap_original(&CAP_MENU_DESER_ORIG, this, ctx, c, d) };
    append_continue_trace(format_args!(
        "CAP menu_deser LEAVE ret=0x{ret:x} {}",
        b80_mount_trace_summary()
    ));
    ret
}

/// MenuWindowJob::Update 0x1407ad1c0 hook: the native menu pump calls this with rcx = a
/// menu-item each tick. We let the game walk its own (CSMenu) tree and CAPTURE the item
/// whose +0xa8 action functor's _Do_call chain resolves to dialog_factory 0x14081ead0 (=
/// the Load-Game item) into MENU_LOAD_GAME_ITEM, so the own-stepper can drive it
/// zero-input without guessing the container layout. Pure observe + pass-through (no
/// behaviour change). Logs the first distinct items to map the live title menu.
pub(crate) unsafe extern "system" fn cap_menu_item_update_hook(
    item: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    // Module base independent of the own-stepper (so this hook also works during a
    // user-driven trace with the own-stepper off): own-stepper base if set, else resolve it.
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != TITLE_OWNER_SCAN_START_ADDRESS {
            own
        } else {
            game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        }
    };
    // While the deterministic input probe is active, count GENUINE d180 leaf-Update ticks (this
    // leaf fn 0x1407ad1c0 actually running for the Load-Game item) even after MENU_LOAD_GAME_ITEM
    // is already latched -- so the probe can tell "d180 leaf ticked" from "static walk found it".
    if INPUT_PROBE_ACTIVE.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS
        && item != TITLE_OWNER_SCAN_START_ADDRESS
        && base != TITLE_OWNER_SCAN_START_ADDRESS
        && MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS
    {
        let mut chain = String::new();
        if unsafe { functor_chain_hits_factory(item, base, &mut chain) } {
            MENU_D180_LEAF_TICKED.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        }
    }
    if item != TITLE_OWNER_SCAN_START_ADDRESS
        && base != TITLE_OWNER_SCAN_START_ADDRESS
        && MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS
    {
        let mut chain = String::new();
        let is_load_game = unsafe { functor_chain_hits_factory(item, base, &mut chain) };
        if is_load_game {
            MENU_LOAD_GAME_ITEM.store(item, Ordering::SeqCst);
            MENU_D180_LEAF_TICKED.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            append_continue_trace(format_args!(
                "MENU-ITEM-UPDATE captured LOAD-GAME item=0x{item:x} {chain} {}",
                trace_callers_summary()
            ));
        } else if MENU_ITEM_UPDATE_LAST.swap(item, Ordering::SeqCst) != item {
            // New distinct item ticked: log it once. CAPPED -- with a few items rotating
            // each frame this otherwise floods the size-capped trace and rolls the early
            // SEQ-ITER-CHILD enumeration off. The capture (MENU_LOAD_GAME_ITEM) is unaffected.
            let n =
                MENU_ITEM_UPDATE_CAPTURE_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            if n < MENU_ITEM_UPDATE_LOG_MAX {
                let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                append_continue_trace(format_args!(
                    "MENU-ITEM-UPDATE #{n} item=0x{item:x} vt=0x{vt:x} {chain} load_game=false {}",
                    trace_callers_summary()
                ));
            }
        }
    }
    unsafe { call_cap_original(&MENU_ITEM_UPDATE_ORIG, item, b, c, d) }
}

/// FD4 Sequence::Update / child-iterator 0x1407aa1f0 hook. The opened main-menu registers the
/// Load-Game leaf d180 but it does NOT tick (only the focused entry ticks the leaf Update, so
/// `cap_menu_item_update_hook` misses d180). This iterator runs on every Sequence node; we
/// walk its inline child array ([seq+0x18 + i*8], count [seq+0x60]) and classify each child by
/// the action-functor `_Do_call` chain (`functor_chain_hits_factory` -> dialog_factory
/// 0x14081ead0). The unique hit is d180 / Load-Game -- captured regardless of focus, then read
/// by own_stepper idx10 (MENU_LOAD_GAME_ITEM) for the Stage-2 functor invoke. Early-outs once
/// found (the iterator is hot); fault-tolerant reads never AV; pure read, NO writes/calls into
/// the game beyond the original.
pub(crate) unsafe extern "system" fn cap_sequence_iter_hook(
    seq: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    const PTR_STRIDE: usize = core::mem::size_of::<usize>();
    const WALK_START: usize = 0;
    const WALK_STEP: usize = 1;
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != TITLE_OWNER_SCAN_START_ADDRESS {
            own
        } else {
            game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        }
    };
    if seq != TITLE_OWNER_SCAN_START_ADDRESS
        && base != TITLE_OWNER_SCAN_START_ADDRESS
        && MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS
    {
        let count = unsafe { safe_read_usize(seq + SEQUENCE_COUNT_60_OFFSET) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        // Unconditional structural dump (first N calls): what does the iterator walk?
        let ndbg = SEQ_ITER_DEBUG_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        if ndbg < SEQ_ITER_DEBUG_MAX {
            let seq_vt = unsafe { safe_read_usize(seq) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let child0 = unsafe { safe_read_usize(seq + SEQUENCE_CHILDREN_BASE_18_OFFSET) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let child0_vt = if child0 != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(child0) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            };
            append_continue_trace(format_args!(
                "SEQ-ITER-DBG #{ndbg} seq=0x{seq:x} seqvt=0x{seq_vt:x} count={count} child0=0x{child0:x} child0vt=0x{child0_vt:x}"
            ));
        }
        if (SEQUENCE_CHILD_COUNT_MIN..=SEQUENCE_CHILD_COUNT_MAX).contains(&count) {
            let mut i = WALK_START;
            while i < count {
                let child = unsafe {
                    safe_read_usize(seq + SEQUENCE_CHILDREN_BASE_18_OFFSET + i * PTR_STRIDE)
                }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                if child != TITLE_OWNER_SCAN_START_ADDRESS {
                    let mut chain = String::new();
                    let child_vt =
                        unsafe { safe_read_usize(child) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                    if unsafe { functor_chain_hits_factory(child, base, &mut chain) } {
                        MENU_LOAD_GAME_ITEM.store(child, Ordering::SeqCst);
                        append_continue_trace(format_args!(
                            "SEQ-ITER captured LOAD-GAME child=0x{child:x} vt=0x{child_vt:x} seq=0x{seq:x} count={count} idx={i} {chain}"
                        ));
                        break;
                    }
                    // A MenuWindowJob child means the main menu actually opened (its entries
                    // are registered into a Sequence the iterator walks) -- signal the STAGE1d
                    // retry loop to stop. The title views tick via a different pump, so this
                    // fires ONLY on the real main-menu entries.
                    if child_vt == base + MENU_WINDOW_JOB_VTABLE_RVA {
                        MENU_ENTRIES_SEEN.store(MENU_ENTRIES_SEEN_YES, Ordering::SeqCst);
                    }
                    // Diagnostic: surface distinct MenuWindowJob children (the registered menu
                    // entries, ticking or not) with their docall chain so one run reveals the
                    // opened-menu structure (which entry is Load-Game). Capped to avoid flooding.
                    if child_vt == base + MENU_WINDOW_JOB_VTABLE_RVA
                        && SEQ_ITER_CHILD_LAST.swap(child, Ordering::SeqCst) != child
                    {
                        let nlog = SEQ_ITER_CHILD_LOG_COUNT
                            .fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                        if nlog < SEQ_ITER_CHILD_LOG_MAX {
                            append_continue_trace(format_args!(
                                "SEQ-ITER-CHILD #{nlog} child=0x{child:x} seq=0x{seq:x} count={count} idx={i} {chain}"
                            ));
                        }
                    }
                }
                i += WALK_STEP;
            }
        }
    }
    unsafe { call_cap_original(&SEQUENCE_ITER_ORIG, seq, b, c, d) }
}

pub(crate) unsafe extern "system" fn menu_task_update_wrapper_hook(
    this: *mut c_void,
) -> *mut c_void {
    unsafe {
        append_menu_semaphore_trace(
            "menu_task_update_wrapper",
            "ENTER",
            TRACE_MENU_TASK_UPDATE_WRAPPER_RVA,
            TRACE_MENU_TASK_UPDATE_TABLE_RVA,
            this,
        )
    };
    let result =
        unsafe { call_wrapper_original(&MENU_TASK_UPDATE_WRAPPER_ORIG, this) }.unwrap_or(this);
    unsafe {
        append_menu_semaphore_trace(
            "menu_task_update_wrapper",
            "LEAVE",
            TRACE_MENU_TASK_UPDATE_WRAPPER_RVA,
            TRACE_MENU_TASK_UPDATE_TABLE_RVA,
            result,
        )
    };
    result
}

pub(crate) unsafe extern "system" fn task_enqueue_hook(
    arg0: *mut c_void,
    arg1: *mut c_void,
) -> *mut c_void {
    let trace_index = TASK_ENQUEUE_TRACE_COUNT
        .fetch_add(TASK_ENQUEUE_TRACE_INCREMENT, Ordering::SeqCst)
        + TASK_ENQUEUE_TRACE_INCREMENT;
    let should_trace = trace_index <= TASK_ENQUEUE_TRACE_LIMIT
        || SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst)
            > NO_SAFE_INPUT_CONFIRM_FRAMES;
    if should_trace {
        append_continue_trace(format_args!(
            "menu_task_enqueue seq={trace_index} phase=ENTER hook_rva=0x{:x} list={arg0:p} node={arg1:p} node_{} confirm_active={} pulse={} {} {}",
            TRACE_TASK_ENQUEUE_RVA,
            unsafe { object_vtable_summary(arg1) },
            SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst)
                > NO_SAFE_INPUT_CONFIRM_FRAMES,
            SAFE_INPUT_CONFIRM_PULSE_SEQ.load(Ordering::SeqCst),
            trace_callers_summary(),
            game_man_trace_summary()
        ));
    }
    let result = unsafe { call_task_enqueue_original(arg0, arg1) }.unwrap_or(arg1);
    if should_trace {
        append_continue_trace(format_args!(
            "menu_task_enqueue seq={trace_index} phase=LEAVE ret={result:p} {}",
            game_man_trace_summary()
        ));
    }
    result
}

pub(crate) unsafe extern "system" fn set_save_slot_hook(slot: i32) {
    append_continue_trace(format_args!(
        "ENTER set_save_slot slot={slot} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = SET_SAVE_SLOT_ORIG.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(i32) = unsafe { std::mem::transmute(original) };
        unsafe { original(slot) };
    }
    append_continue_trace(format_args!(
        "LEAVE set_save_slot {}",
        game_man_trace_summary()
    ));
}

pub(crate) unsafe extern "system" fn save_request_profile_hook(enabled: u8) {
    append_continue_trace(format_args!(
        "ENTER save_request_profile enabled={enabled} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = SAVE_REQUEST_PROFILE_ORIG.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(u8) = unsafe { std::mem::transmute(original) };
        unsafe { original(enabled) };
    }
    append_continue_trace(format_args!(
        "LEAVE save_request_profile {}",
        game_man_trace_summary()
    ));
}

pub(crate) unsafe extern "system" fn request_save_hook(enabled: u8) {
    append_continue_trace(format_args!(
        "ENTER request_save enabled={enabled} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = REQUEST_SAVE_ORIG.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(u8) = unsafe { std::mem::transmute(original) };
        unsafe { original(enabled) };
    }
    append_continue_trace(format_args!(
        "LEAVE request_save {}",
        game_man_trace_summary()
    ));
}

pub(crate) unsafe extern "system" fn current_slot_load_hook(arg0: i32, arg1: u8, arg2: u8) -> u8 {
    append_continue_trace(format_args!(
        "ENTER current_slot_load_67b570 arg0={arg0} arg1={arg1} arg2={arg2} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let ret = unsafe { call_bool3_original(&CURRENT_SLOT_LOAD_ORIG, arg0, arg1, arg2) }
        .unwrap_or(HOOK_FALSE_RETURN);
    append_continue_trace(format_args!(
        "LEAVE current_slot_load_67b570 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn continue_load_hook(slot: i32, arg1: u8, arg2: u8) -> u8 {
    append_continue_trace(format_args!(
        "ENTER continue_load_67b750 slot={slot} arg1={arg1} arg2={arg2} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let ret = unsafe { call_bool3_original(&CONTINUE_LOAD_ORIG, slot, arg1, arg2) }
        .unwrap_or(HOOK_FALSE_RETURN);
    append_continue_trace(format_args!(
        "LEAVE continue_load_67b750 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn combined_load_hook(slot: i32, arg1: u8, arg2: u8) -> u8 {
    append_continue_trace(format_args!(
        "ENTER combined_load_67b940 slot={slot} arg1={arg1} arg2={arg2} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let ret = unsafe { call_bool3_original(&COMBINED_LOAD_ORIG, slot, arg1, arg2) }
        .unwrap_or(HOOK_FALSE_RETURN);
    append_continue_trace(format_args!(
        "LEAVE combined_load_67b940 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn map_load_hook() -> u8 {
    append_continue_trace(format_args!(
        "ENTER map_load_67bc10 {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = MAP_LOAD_ORIG.load(Ordering::SeqCst);
    let ret = if original == HOOK_ORIGINAL_UNSET {
        HOOK_FALSE_RETURN
    } else {
        let original: unsafe extern "system" fn() -> u8 = unsafe { std::mem::transmute(original) };
        unsafe { original() }
    };
    if ret != HOOK_FALSE_RETURN {
        TITLE_BOOTSTRAP_SEEN.store(TITLE_BOOTSTRAP_SEEN_VALUE, Ordering::SeqCst);
    }
    append_continue_trace(format_args!(
        "LEAVE map_load_67bc10 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn save_load_state_init_hook() -> u8 {
    append_continue_trace(format_args!(
        "ENTER save_load_state_init_67b030 {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = SAVE_LOAD_STATE_INIT_ORIG.load(Ordering::SeqCst);
    let ret = if original == HOOK_ORIGINAL_UNSET {
        HOOK_FALSE_RETURN
    } else {
        let original: unsafe extern "system" fn() -> u8 = unsafe { std::mem::transmute(original) };
        unsafe { original() }
    };
    append_continue_trace(format_args!(
        "LEAVE save_load_state_init_67b030 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}
