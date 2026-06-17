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
    tag: &str,
    verbose: bool,
) -> Option<usize> {
    const MENU_JOB_HOLDER_E0: usize = 0xe0;
    const VTABLE_UPDATE_SLOT_10: usize = 0x10;
    const NODE_CHILDREN_BASE_18: usize = 0x18;
    const NODE_COUNT_60: usize = 0x60;
    const NODE_HOLDER_ROOT_18: usize = 0x18;
    const SEQ_UPDATE_RVA: usize = 0x07aa1f0;
    const LEAF_UPDATE_RVA: usize = 0x07ad1c0;
    const ITEM_CTX_10: usize = 0x10;
    const ITEM_RESULT_130: usize = 0x130;
    const PTR_STRIDE: usize = core::mem::size_of::<usize>();
    const COUNT_MIN: usize = 1;
    const COUNT_MAX: usize = 32;
    const MAX_NODES: usize = 128;
    const MAX_DEPTH: usize = 8;
    const WALK_START: usize = 0;
    const WALK_STEP: usize = 1;

    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let seq_update_abs = module_base + SEQ_UPDATE_RVA;
    let leaf_update_abs = module_base + LEAF_UPDATE_RVA;

    let holder = unsafe { safe_read_usize(owner + MENU_JOB_HOLDER_E0) }.unwrap_or(null);
    if verbose {
        append_autoload_debug(format_args!(
            "job-tree[{tag}]: owner=0x{owner:x} holder(owner+0xe0)=0x{holder:x} seq_update=0x{seq_update_abs:x} leaf_update=0x{leaf_update_abs:x}"
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
                "job-tree[{tag}] d={depth} node=0x{node:x} vt=0x{vtable:x} update=0x{update:x} leaf={is_leaf} container={is_container} count=0x{count:x} base=0x{base:x} ctx=0x{ctx:x} result=0x{result:x} {chain} LOAD_GAME={is_load_game}"
            ));
        }
        if depth < MAX_DEPTH && (COUNT_MIN..=COUNT_MAX).contains(&count) {
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
    if phase == OWN_STEPPER_PHASE_MENU {
        if n < OWN_STEPPER_SETTLE_CALLS || want_slot == OWN_STEPPER_SLOT_NONE {
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
        let bare = unsafe { diagnostic_menu_walk(owner, base, "bare", true) };
        let bare_tree = unsafe { diagnostic_job_tree_walk(owner, base, "bare-tree", true) };
        let set_state: unsafe extern "system" fn(usize, i32) =
            unsafe { std::mem::transmute(base + TITLE_SET_STATE_RVA) };
        unsafe { set_state(owner, TITLE_STEP_BEGIN_TITLE) };
        OWN_STEPPER_MENU_BUILD_WAITS.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
        OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_MENU_BUILD, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "own_stepper: STAGE1 bare-walk done (load_game_138=0x{:x} load_game_tree=0x{:x}) -> SetState(3) BeginTitle to build the main menu zero-input (#{n}) slot={want_slot} gm=0x{gm:x} c30=0x{c30:x} b80={b80}",
            bare.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
            bare_tree.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        ));
        // Suppress unused warnings for consts/statics retained from the falsified cold
        // slot-int drive, synthetic-dispatcher, b78-route, and Continue-shim work.
        let _ = (
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
        // Zero-input main-menu built by the idx10 SetState(3). Wait for BeginTitle to
        // populate Continue/Load into owner+0x138, polling each frame (quiet); log the full
        // tree once when the Load-Game leaf is found (or on timeout). NO SetState here ->
        // stays at the main menu, save-safe. STAGE 2 (invoke the leaf functor) is gated on
        // this confirming the live item + layout first.
        let waits =
            OWN_STEPPER_MENU_BUILD_WAITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
        // Primary: the menu-item Update hook captures the Load-Game item as the native pump
        // ticks the CSMenu tree (robust, no layout guess). Fallback: our static tree walk.
        let hooked = MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst);
        let found = if hooked != TITLE_OWNER_SCAN_START_ADDRESS {
            Some(hooked)
        } else {
            unsafe { diagnostic_job_tree_walk(owner, base, "built-tree", false) }
        };
        match found {
            Some(item) => {
                let _ = unsafe { diagnostic_menu_walk(owner, base, "built-138", true) };
                let _ = unsafe { diagnostic_job_tree_walk(owner, base, "built-tree", true) };
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE1b LOAD-GAME item identified=0x{item:x} after {waits} waits -- STAGE 2 (functor invoke) PENDING; staying at main menu (NO-WRITE) c30=0x{c30:x} b80={b80}"
                ));
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            None => {
                if waits >= OWN_STEPPER_MENU_BUILD_WAIT_MAX {
                    let _ = unsafe { diagnostic_menu_walk(owner, base, "built138-timeout", true) };
                    let _ = unsafe {
                        diagnostic_job_tree_walk(owner, base, "built-tree-timeout", true)
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
    // phase DONE: idx6 watches the native load; idx10 just passes through if re-entered.
    pass_through(false);
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
        create_continue_trace_hook(
            &mut hooks,
            "c30_writer_67bd70",
            C30_WRITER_RVA as u32,
            c30_writer_hook as *mut c_void,
            &C30_WRITER_ORIG,
        );
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
    append_continue_trace(format_args!(
        "c30_writer_67bd70 ENTER game_man=0x{game_man:x} buf=0x{buffer:x} size=0x{size:x} {} {}",
        b80_mount_trace_summary(),
        trace_callers_summary()
    ));
    let original = C30_WRITER_ORIG.load(Ordering::SeqCst);
    let ret = if original == HOOK_ORIGINAL_UNSET {
        B80_HOOK_DEFAULT_RET as usize
    } else {
        let original: unsafe extern "system" fn(usize, usize, u32) -> usize =
            unsafe { std::mem::transmute(original) };
        unsafe { original(game_man, buffer, size) }
    };
    append_continue_trace(format_args!(
        "c30_writer_67bd70 LEAVE ret=0x{ret:x} {}",
        b80_mount_trace_summary()
    ));
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
    append_continue_trace(format_args!(
        "CAP dialog_factory ENTER rcx=0x{a:x} rdx=0x{b:x} {} {}",
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_cap_original(&CAP_DIALOG_FACTORY_ORIG, a, b, c, d) };
    append_continue_trace(format_args!(
        "CAP dialog_factory LEAVE dialog_this=0x{ret:x}"
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
    if item != TITLE_OWNER_SCAN_START_ADDRESS
        && base != TITLE_OWNER_SCAN_START_ADDRESS
        && MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS
    {
        let mut chain = String::new();
        let is_load_game = unsafe { functor_chain_hits_factory(item, base, &mut chain) };
        if is_load_game {
            MENU_LOAD_GAME_ITEM.store(item, Ordering::SeqCst);
            append_continue_trace(format_args!(
                "MENU-ITEM-UPDATE captured LOAD-GAME item=0x{item:x} {chain} {}",
                trace_callers_summary()
            ));
        } else if MENU_ITEM_UPDATE_LAST.swap(item, Ordering::SeqCst) != item {
            // New distinct item ticked (user navigated to it): log it once.
            let n =
                MENU_ITEM_UPDATE_CAPTURE_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            append_continue_trace(format_args!(
                "MENU-ITEM-UPDATE #{n} item=0x{item:x} vt=0x{vt:x} {chain} load_game=false {}",
                trace_callers_summary()
            ));
        }
    }
    unsafe { call_cap_original(&MENU_ITEM_UPDATE_ORIG, item, b, c, d) }
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
