//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

#![allow(unused_imports)]

use std::{
    ffi::c_void,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use debug::InputBlocker;
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
            Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
            System::{
                LibraryLoader::{GetModuleHandleA, GetProcAddress},
                Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery},
                SystemServices::DLL_PROCESS_ATTACH,
                Threading::GetCurrentProcessId,
            },
            UI::WindowsAndMessaging::{
                EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_KEYDOWN,
                WM_KEYUP,
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
            // Phase A: deserialize slot N FIRST. It is CSFeMan-less, so it works at
            // the title (state 10) and lands slot N's REAL saved map in GameMan+0xc30
            // (+ applies the character) BEFORE any MoveMap occupies the child with a
            // placeholder map. (The m60-first order occupied the child + the re-target
            // at state 6 was ignored -> child+0xd8 stuck on m60.)
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
            SUBMIT_PLAY_GAME_PHASE.store(SUBMIT_PHASE_DESER, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "submit_play_game: phaseA deserialize(slot {slot}) c30=0x{:x} csfeman=0x{csfeman:x} tick={tick}",
                read_c30()
            ));
        }
        SUBMIT_PHASE_DESER => {
            // Phase B: SetState(5) with the REAL saved map (GameMan+0xc30) so the
            // pump builds CSFeMan + the MoveMapStep targeting slot N's actual map
            // (no m60 placeholder to occupy the child).
            let Some(owner) = (unsafe { title_owner(module_base) }) else {
                return true;
            };
            let owner = owner as usize;
            let c30 = read_c30();
            unsafe {
                *((owner + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET) as *mut u8) =
                    MOVIE_SKIP_FLAG_CLEAR;
                *((owner + TITLE_OWNER_PLAY_GAME_SLOT_OFFSET) as *mut i32) = c30;
            }
            unsafe { set_state(owner, TITLE_STEP_PLAY_GAME) };
            SUBMIT_PLAY_GAME_PHASE.store(SUBMIT_PHASE_BUILT, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "submit_play_game: phaseB SetState(5) map=0x{c30:x} owner=0x{owner:x} tick={tick}"
            ));
        }
        SUBMIT_PHASE_BUILT => {
            // Phase C: once SetState(5) has built CSFeMan + the MoveMapStep (group 20,
            // scheduler-ticked every frame), initiate the NATIVE b80 load machine for
            // slot N: 0x14067b4e0(slot) begins the async slot-IO + sets GameMan+0xb80=1.
            // The scheduler-ticked MoveMapStep then runs the b80 deserialize/apply +
            // MsbLoad, PRIMING the world-stream (the piece the direct deserialize
            // skipped) -> resident -> child+0xd8 drains. No manual pumping.
            if csfeman == null {
                return true;
            }
            let initiate: unsafe extern "system" fn(i32) =
                unsafe { std::mem::transmute(module_base + LOAD_INITIATOR_RVA) };
            unsafe { initiate(slot) };
            // Build + register the world-stream worker 0x144842d40 (IngameInit's
            // +0x48>=7 tail 0x140b0a980) via a synthetic step `this` (zeroed buffer
            // with +0x48=7) -- the arm uses only globals/stack after the +0x48 check,
            // so this constructs + scheduler-registers the worker so the engine pool
            // services the queued slot reads -> MsbLoad -> world singletons -> resident.
            let mut synth = [0u8; SYNTHETIC_STEP_THIS_SIZE];
            unsafe {
                *(synth.as_mut_ptr().add(SYNTHETIC_STEP_STATE_OFFSET) as *mut i32) =
                    WORLD_WORKER_BUILD_STATE;
            }
            let build_worker: unsafe extern "system" fn(*mut u8) =
                unsafe { std::mem::transmute(module_base + WORLD_WORKER_BUILD_RVA) };
            unsafe { build_worker(synth.as_mut_ptr()) };
            let worker = unsafe { *((module_base + WORLD_STREAM_WORKER_RVA) as *const usize) };
            let b80 = if gm != null {
                unsafe { *((gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const i32) }
            } else {
                TITLE_STATE_OWNER_GONE
            };
            SUBMIT_PLAY_GAME_PHASE.store(SUBMIT_PHASE_DONE, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "submit_play_game: phaseC b80-load(slot {slot}) b80={b80} worker=0x{worker:x} c30=0x{:x} csfeman=0x{csfeman:x} tick={tick}",
                read_c30()
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
            let world_b = unsafe { *((module_base + WORLD_SINGLETON_B_RVA) as *const usize) };
            append_autoload_debug(format_args!(
                "submit_play_game: phaseD state={state} child_d8={d8} b80={b80} mms_state={mms_state} worldA=0x{world_a:x} worldB=0x{world_b:x} c30=0x{:x} csfeman=0x{csfeman:x} tick={tick}",
                read_c30()
            ));
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
