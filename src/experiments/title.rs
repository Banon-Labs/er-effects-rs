//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

#![allow(unused_imports)]

use std::{
    ffi::c_void,
    fmt::Write as _,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use std::os::windows::ffi::OsStrExt as _;

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

use super::*;

pub(crate) fn arm_product_autoload_from_request(request: &SaveLoader) {
    // Arm the menu-free path flags from the reliable autoload-file channel, independent of slot
    // and method, so own_stepper_enabled()/cold_char_mount_enabled() do not depend on env-var
    // propagation through Proton or game_directory_path() trigger-file resolution.
    if request.own_stepper() {
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.cold_char_mount() {
        COLD_CHAR_MOUNT_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.own_load() {
        // own_load drives through the idx10 detour (own_stepper_idx10), so arm the own_stepper file
        // flag too -- that is what makes own_stepper_patch_once install the detour so OUR handler
        // runs each frame. own_load takes precedence inside the handler (like cold_char_mount).
        OWN_LOAD_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.own_load_continue() {
        // The final guarded world-stream step rides on the SAME own_load probe (own_load_drive runs
        // the proven verify-only parse, then fires the guarded continue). Arm own_load too so the
        // probe actually runs even if only own_load_continue was set in the autoload file.
        OWN_LOAD_CONTINUE_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_LOAD_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.own_dispatch() {
        // The m28 direct-enqueue lever rides the SAME OWN-LOAD path: it only fires AFTER our
        // continue_confirm sets OWN_LOAD_CONTINUE_FIRED. Arm own_load + own_load_continue too so the
        // path that sets that flag actually runs when only own_dispatch was set in the autoload file.
        OWN_DISPATCH_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_LOAD_CONTINUE_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_LOAD_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.own_load_install_job() {
        // The LoadGame-JOB INSTALL lever rides the SAME OWN-LOAD path: it runs INSTEAD of the
        // continue_confirm/SetState5 step at the END of own_load_drive. Arm own_load (+ own_stepper,
        // which installs the idx10 detour that runs own_load_drive) so the probe actually runs even if
        // only own_load_install_job was set in the autoload file. Deliberately does NOT arm
        // own_load_continue (the save-writing SetState5 lever): this is the non-SetState5 alternative.
        OWN_LOAD_INSTALL_JOB_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_LOAD_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.own_load_pump() {
        // PATH B PRIVATE-PUMP lever ("own the load"): builds the LoadGame job with REAL mss-derived ctx
        // then ticks its Run privately each frame to completion + drives the transition on Success. Rides
        // the SAME OWN-LOAD path: it runs INSTEAD of the install/continue step at the END of
        // own_load_drive. Arm own_load (+ own_stepper, which installs the idx10 detour that runs
        // own_load_drive) so the probe actually runs even if only own_load_pump was set in the autoload
        // file. Does NOT arm own_load_continue here -- the pump fires the guarded SetState5 transition
        // itself only after the pumped job reaches Success.
        OWN_LOAD_PUMP_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_LOAD_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    let Some(slot) = request.slot() else {
        return;
    };

    if slot < OWN_STEPPER_SLOT_ZERO {
        return;
    }

    // OWN_STEPPER_SLOT is the shared target slot for the menu-free own_stepper /
    // native_fullread / cold_char_mount / native-continue paths AND the experimental menu-driven
    // product_core path. Set it whenever a valid slot is configured, regardless of method, so the
    // known-good zero-input smoke path does not depend on a fragile env-method side effect.
    OWN_STEPPER_SLOT.store(slot, Ordering::SeqCst);
    if request.method() == SaveLoadMethod::DirectMenuLoad && experimental_direct_menu_load_enabled()
    {
        PRODUCT_AUTOLOAD_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
}
pub(crate) fn product_core_ready_blocker_label(blocker: usize) -> &'static str {
    match blocker {
        PRODUCT_CORE_BLOCKER_UNSEEN => "unseen",
        PRODUCT_CORE_BLOCKER_READY => "ready",
        PRODUCT_CORE_BLOCKER_NO_TITLE_OWNER => "no_title_owner",
        PRODUCT_CORE_BLOCKER_TITLE_OWNER_STATE => "title_owner_state",
        PRODUCT_CORE_BLOCKER_TITLE_TABLE => "title_table",
        PRODUCT_CORE_BLOCKER_SESSION => "session",
        PRODUCT_CORE_BLOCKER_GAME_DATA_MAN => "game_data_man",
        PRODUCT_CORE_BLOCKER_PROFILE_SUMMARY => "profile_summary",
        PRODUCT_CORE_BLOCKER_IODEV => "iodev",
        PRODUCT_CORE_BLOCKER_HEAP_ALLOCATOR => "heap_allocator",
        PRODUCT_CORE_BLOCKER_TITLE_DIALOG => "title_dialog",
        PRODUCT_CORE_BLOCKER_PRESS_START => "press_start",
        PRODUCT_CORE_BLOCKER_TITLE_STATE => "title_state",
        _ => "unknown",
    }
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
    // Lever-2 (option c): satisfy the global menu-accept side-effect zero-input. At the parked
    // press-any-button title (state 10), set the global accept byte 0x144589bdc=1 ONCE so the
    // native TitleTopDialog::update runs the open-menu registrar on its own next tick -- the
    // NATURAL advance (builds Continue/Load + transfers focus -> select-layer/router_this), which
    // a direct registrar self-fire could not do without spawning a competing dialog that reverted.
    // Not an input event (this is the decoded accept flag, like the ToS-accepted flag). Gated OFF
    // by default. Sampling above continues so the cascade (menu_opened, router_this) is observed.
    if title_accept_byte_gate_enabled()
        && state == TITLE_STEP_MENU_JOB_WAIT_STATE
        && !TITLE_ACCEPT_BYTE_GATE_FIRED.swap(true, Ordering::SeqCst)
    {
        unsafe {
            *((module_base + TITLE_GLOBAL_ACCEPT_BYTE_RVA) as *mut u8) =
                TITLE_PROCEED_GATE_SET_VALUE;
        }
        let after = unsafe { *((module_base + TITLE_GLOBAL_ACCEPT_BYTE_RVA) as *const u8) };
        append_autoload_debug(format_args!(
            "title_accept_byte_gate: set [0x144589bdc]={after} at state {state} tick={tick} -- zero-input natural menu-open"
        ));
    }
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
    let game_man = game_man_ptr_or_null();
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
pub(crate) unsafe fn cleanup_title_dialog_after_world_once(module_base: usize, frame: u64) {
    static TITLE_DIALOG_CLEANUP_DONE: AtomicUsize =
        AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
    if !cleanup_title_dialog_after_world_enabled()
        || TITLE_DIALOG_CLEANUP_DONE.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
            != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    let owner = unsafe { title_owner(module_base) };
    let Some(owner_ptr) = owner else {
        append_autoload_debug(format_args!(
            "title-dialog-cleanup: skipped frame={frame} no title owner"
        ));
        return;
    };
    let owner_addr = owner_ptr as usize;
    let dialog = unsafe { safe_read_usize(owner_addr + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let dialog_vt = if dialog != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(dialog) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    if dialog_vt != module_base + TITLE_TOP_DIALOG_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "title-dialog-cleanup: skipped frame={frame} dialog=0x{dialog:x} vt=0x{dialog_vt:x} expected=0x{:x}",
            module_base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return;
    }
    let cleanup: unsafe extern "system" fn(usize) -> usize =
        unsafe { std::mem::transmute(module_base + TITLE_TOP_DIALOG_CLEANUP_RVA) };
    let ret = unsafe { cleanup(dialog) };
    let mut remaining_slots = TITLE_OWNER_SCAN_START_ADDRESS;
    let mut idx = ACTIVE_SCREEN_SLOT_START;
    while idx < ACTIVE_SCREEN_ARRAY_SLOTS {
        let slot = module_base + ACTIVE_SCREEN_ARRAY_RVA + idx * ACTIVE_SCREEN_ARRAY_STRIDE;
        let ptr = unsafe { safe_read_usize(slot) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        if ptr != TITLE_OWNER_SCAN_START_ADDRESS {
            remaining_slots += ACTIVE_SCREEN_SLOT_STEP;
        }
        idx += ACTIVE_SCREEN_SLOT_STEP;
    }
    append_autoload_debug(format_args!(
        "title-dialog-cleanup: called 0x{:x} frame={frame} owner=0x{owner_addr:x} dialog=0x{dialog:x} ret=0x{ret:x} remaining_active_slots={remaining_slots}",
        module_base + TITLE_TOP_DIALOG_CLEANUP_RVA
    ));
}
/// AUTONOMOUS press-any-button -> open-menu (zero-input): drive the title to the open main menu
/// OURSELVES so a run needs no real button press. When the live TitleTopDialog (owner+0xe0) is settled
/// in the FD4 `Loop` state with the menu-opened latch (dialog+0xa40) still 0, call the native open-menu
/// registrar `0x1409b24e0(rcx=dialog)` -- the exact action a button press triggers -- to open the menu
/// (sets a40=1). Requires online-disable (`er-effects-offline.txt`) so the connection modal is skipped
/// and the SM reaches Loop. One-shot. Then `maybe_fire_tfc_continue` (gated a40==1) fires Continue. No
/// input. (Same self-fire the own_stepper STAGE1d uses, extracted for the tfc flow.)
pub(crate) unsafe fn maybe_auto_open_menu(base: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if TFC_AUTO_MENU_OPENED.load(Ordering::SeqCst) != 0 {
        return;
    }
    let Some(owner_ptr) = (unsafe { title_owner(base) }) else {
        return;
    };
    let owner = owner_ptr as usize;
    let dialog = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(0);
    let dialog_vt = if dialog != 0 {
        unsafe { safe_read_usize(dialog) }.unwrap_or(0)
    } else {
        0
    };
    if dialog == 0 || dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return;
    }
    let a40 = unsafe { safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET) }
        .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
        .unwrap_or(1);
    if a40 != OWN_STEPPER_MENU_OPENED_NO {
        // Menu already open (a real press or a prior call) -> nothing to do.
        TFC_AUTO_MENU_OPENED.store(1, Ordering::SeqCst);
        return;
    }
    // Require the dialog SETTLED in Loop: the registrar internally set_state(TextFadeOut) re-checks
    // node flags&0x8f>=2 and bails if not settled (FadeIn would no-op / corrupt). Read-only probe.
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
    let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) } != OWN_STEPPER_FALSE;
    if !in_loop {
        return;
    }
    // ROUTE THE REGISTRAR IN-PLACE (zero-input): the native open-menu call sites write a "mode" byte at
    // [*(base+TITLE_MENU_TRANSITION_SINGLETON_RVA)]+0 BEFORE jumping to the registrar -- press-accept
    // 0x1409b1260 sets it =1 (open main menu IN PLACE), pump/back paths set it =0. A bare open_menu with
    // the byte left STALE may route the registrar into an error-modal branch. Replicate the press-accept
    // set (subagent-C static RE: product native-open with this byte set reached the menu with 0 msgbox).
    // Null-/readability-guarded; no save write, no input. bd er-effects-rs-0ye + title-accept-to-registrar-narrow-path-143d5dea8.
    let transition_singleton =
        unsafe { safe_read_usize(base + TITLE_MENU_TRANSITION_SINGLETON_RVA) }.unwrap_or(null);
    if transition_singleton != null && unsafe { safe_read_usize(transition_singleton) }.is_some() {
        unsafe { *(transition_singleton as *mut u8) = TITLE_MENU_TRANSITION_FLAG_SET_VALUE };
        append_autoload_debug(format_args!(
            "tfc-auto-open: set menu-transition mode byte [*(0x{:x})]+0=1 before open-menu (route registrar in-place)",
            base + TITLE_MENU_TRANSITION_SINGLETON_RVA
        ));
    }
    let open_menu: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_OPEN_MENU_RVA) };
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        open_menu(dialog)
    }));
    TFC_AUTO_MENU_OPENED.store(1, Ordering::SeqCst);
    let _ = null;
    append_autoload_debug(format_args!(
        "tfc-auto-open: fired open-menu registrar 0x{:x}(dialog=0x{dialog:x}) on Loop+a40==0 (panicked={}) -- autonomous press-any-button equivalent, NO input",
        base + TITLE_TOP_DIALOG_OPEN_MENU_RVA,
        r.is_err()
    ));
}
/// Zero-input NATURAL menu-open (the row-building path). At the parked press-any-button title
/// (TitleTopDialog settled in "Loop", menu not yet open a40==0), set the decoded global menu-accept
/// byte 0x144589bdc=1 ONCE so the game's OWN `TitleTopDialog::update` accept-gate runs the open-menu
/// registrar in its NATIVE frame -- which POSTS the Continue/Load/NewGame MenuJob chain AND drains it
/// (MenuWindow::Update 0x140745520) in the same native flow, so the rows actually BUILD. A direct
/// registrar self-fire (`maybe_auto_open_menu`) only POSTS the chain; the native update does not drain
/// a chain it did not open itself, so the rows never build (continue-scan = 0 nodes; bd
/// rowbuild-mechanism-incontext-openmenu-2026-06-23 + title-global-accept-byte-144589bdc). This is the
/// decoded accept FLAG the input pipeline sets on press -- NOT a synthesized DInput/keystate/XInput
/// event -> still `simulated_button_presses_total == 0`. Save-safe (menu-UI build, no save write). The
/// ToS/language over-trigger this byte caused in 2026-06 is now neutralized by the offline-mode +
/// Menu_IsEnableOnlineMode patches, so it should reach the main menu cleanly; the msgbox/policy oracles
/// will catch any regression. One-shot via TITLE_ACCEPT_BYTE_GATE_FIRED, latched only after the gating
/// passes so a not-yet-settled title does not consume the shot.
pub(crate) unsafe fn maybe_set_title_accept_byte(base: usize) {
    if TITLE_ACCEPT_BYTE_GATE_FIRED.load(Ordering::SeqCst) {
        return;
    }
    let Some(owner_ptr) = (unsafe { title_owner(base) }) else {
        return;
    };
    let owner = owner_ptr as usize;
    let dialog = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(0);
    let dialog_vt = if dialog != 0 {
        unsafe { safe_read_usize(dialog) }.unwrap_or(0)
    } else {
        0
    };
    if dialog == 0 || dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return;
    }
    // Only at the parked press-any-button (menu not yet open): a40 latch == 0.
    let a40 = unsafe { safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET) }
        .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
        .unwrap_or(1);
    if a40 != OWN_STEPPER_MENU_OPENED_NO {
        TITLE_ACCEPT_BYTE_GATE_FIRED.store(true, Ordering::SeqCst); // already open -> nothing to do
        return;
    }
    // Require the dialog SETTLED in Loop so the native update's accept-gate consumes our byte on its
    // next tick (read-only probe of the live state by name, no side effects).
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
    let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) } != OWN_STEPPER_FALSE;
    if !in_loop {
        return;
    }
    if TITLE_ACCEPT_BYTE_GATE_FIRED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        *((base + TITLE_GLOBAL_ACCEPT_BYTE_RVA) as *mut u8) = TITLE_PROCEED_GATE_SET_VALUE;
    }
    append_autoload_debug(format_args!(
        "title-accept-byte: set [0x{:x}]=1 on settled TitleTopDialog (Loop, a40==0) -- zero-input NATURAL menu-open (registrar runs in native update frame -> Continue/Load/NewGame rows build + drain)",
        base + TITLE_GLOBAL_ACCEPT_BYTE_RVA
    ));
}
/// Connection-state OFFLINE lever (zero-input, save-safe) -- the milestone-3 fix. The title's
/// network/session event handlers (`CSLuaEventScriptImitation::On{LanCutError,DisconnectGameServer,
/// FailedGetBlockNum,NpServerSignOut,DisconnectEOSServer,...}`) build the "Cannot connect to network /
/// connection lost / network error" `GR_System_Message` MessageBoxDialogs that our offline pab boot
/// raises at menu-open. Each handler is guarded by `if (IsInOnlineMode()) { if
/// (IsServerConnectionEnabled() && ...) { build popup } }`, which reduces to two `GameMan` bytes:
/// `isInOnlineMode = [GameMan+0xBC8]`, `serverConnectionEnabled = [GameMan+0xBC9]`
/// (`GameMan = *(base+GAME_SAVE_SLOT_SINGLETON_RVA)`; getter `0x14067a030` is `mov rax,[0x143d69918];
/// movzx eax,[rax+0xBC8]; ret` -- VERIFIED by deobf disasm). NOTE the existing online-disable patches
/// that getter's RETURN value, but the handlers consult the BYTES (directly / via getters our patch
/// does not cover), so the patch alone does not gate them. Forcing both bytes to 0 each title frame
/// short-circuits the whole connection-loss family at the source (the guard fails -> no popup is ever
/// enqueued -- not suppressed, not dismissed). Pure offline state, no save write, no input. Readable-
/// guarded so a not-yet-initialized GameMan can never fault the game thread. bd er-effects-rs-0ye
/// (subagent-D GR_System_Message gate, subagent-B premise: modals are network notices not SaveRetry).
pub(crate) unsafe fn force_offline_connection_bytes(base: usize) {
    const IS_IN_ONLINE_MODE_BC8_OFFSET: usize = 0xBC8;
    const SERVER_CONNECTION_ENABLED_BC9_OFFSET: usize = 0xBC9;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let game_man = unsafe { safe_read_usize(base + GAME_SAVE_SLOT_SINGLETON_RVA) }.unwrap_or(null);
    if game_man == null {
        return;
    }
    let (Some(online), Some(server)) = (
        unsafe { safe_read_u8(game_man + IS_IN_ONLINE_MODE_BC8_OFFSET) },
        unsafe { safe_read_u8(game_man + SERVER_CONNECTION_ENABLED_BC9_OFFSET) },
    ) else {
        return;
    };
    if online == 0 && server == 0 {
        return;
    }
    unsafe {
        *((game_man + IS_IN_ONLINE_MODE_BC8_OFFSET) as *mut u8) = 0;
        *((game_man + SERVER_CONNECTION_ENABLED_BC9_OFFSET) as *mut u8) = 0;
    }
    if FORCE_OFFLINE_BYTES_CLEARED.fetch_add(1, Ordering::SeqCst) == 0 {
        append_autoload_debug(format_args!(
            "force-offline: cleared GameMan+0xBC8 (isInOnlineMode {online}->0) +0xBC9 (serverConnectionEnabled {server}->0) gm=0x{game_man:x} -- gate connection-loss GR_System_Message popups at source"
        ));
    }
}
/// See `fire_tfc_continue_enabled`. Runs from the recurring game task; self-gates and fires ONCE.
/// Pure in-process field writes (NO input, NO native call) -- the native menu pump's selector
/// (`0x1409a8eb0`) picks up `tfc+0x14c==1` on its next tick and dispatches the load through the
/// engine's own job pump (the proven user-Continue path, which avoids the FixOrderJobSequence
/// overflow that killed the factory-direct `own_load_pump`). Logs before/after so a probe sees the
/// exact write.
pub(crate) unsafe fn maybe_fire_tfc_continue(base: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if !fire_tfc_continue_enabled() {
        return;
    }
    if TFC_CONTINUE_FIRED.load(Ordering::SeqCst) != 0 {
        return;
    }
    // Resolve+cache the SimpleTitleStep owner (throttled full scan); bail until it exists.
    let Some(owner_ptr) = (unsafe { title_owner(base) }) else {
        return;
    };
    let owner = owner_ptr as usize;
    // Require the SETTLED main-menu state (STEP_MenuJobWait), i.e. press-any-button -> BeginLogo done.
    let committed = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    if committed != TITLE_STEP_MENU_JOB_WAIT {
        return;
    }
    // Require "the rest of GameMan is set up": the GetSaveSlot singleton (*(base+0x3d69918)) non-null.
    let gm_singleton = unsafe { safe_read_usize(base + GAME_SAVE_SLOT_SINGLETON_RVA) }.unwrap_or(0);
    if gm_singleton == null || gm_singleton == 0 {
        return;
    }
    // Live TitleTopDialog (owner+0xe0, vtable-gated) -> CS::TitleFlowContext at +0xa38.
    let dialog = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(0);
    let dialog_vt = if dialog != 0 {
        unsafe { safe_read_usize(dialog) }.unwrap_or(0)
    } else {
        0
    };
    if dialog == 0 || dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return;
    }
    // Require the MAIN MENU to be OPEN, not the bare press-any-button screen. State 10
    // (MenuJobWait) occurs at BOTH; the open-menu registrar 0x1409b24e0 sets the menu-opened latch
    // [dialog+0xa40]=1. Firing the bit at the closed press-any-button screen is dormant (the selector
    // that consumes tfc+0x14c is a Continue-item funclet not pumped until the menu is open) -- bd
    // tfc-14c-bit-dormant-without-menu-open-or-selector-invoke-2026-06-22.
    let menu_opened = unsafe {
        safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET)
            .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
            .unwrap_or(0)
    };
    if menu_opened != OWN_STEPPER_CALL_INC {
        return;
    }
    let tfc = unsafe { safe_read_usize(dialog + DIALOG_OWNER_CTX_A38_OFFSET) }.unwrap_or(0);
    if !(tfc > OWNER_CTX_MIN_PLAUSIBLE_PTR && tfc < OWNER_CTX_MAX_PLAUSIBLE_PTR) {
        return;
    }
    // READINESS GATE on dialog+0x50 (the load MenuWindowJob's push target -- selector sets r8=dialog+
    // 0x50). A run showed its count field (dialog+0x50+0x48 = dialog+0x98) can hold GARBAGE (a pointer
    // ~0x7fff..., not a small count) -> dialog+0x50 is not yet a valid/ready vector in our self-opened
    // flow, and firing then crashes (insert reads garbage count -> 'out of memory'). Fire ONLY when the
    // count is a plausible small value WITH ROOM (< 8); else WAIT (do NOT consume the one-shot) and
    // retry next frame. If it never becomes valid we simply never fire (no crash). bd
    // dialog-plus0x50-NOT-a-vector-built-job-miscontextualized-2026-06-23.
    let load_vec_count = unsafe {
        safe_read_usize(dialog + DIALOG_MENUWINDOW_VEC_50_OFFSET + DLFIXEDVECTOR_COUNT_48_OFFSET)
    }
    .unwrap_or(usize::MAX);
    if load_vec_count >= 8 {
        let waits = TFC_LOAD_VEC_WAIT_TICKS.fetch_add(1, Ordering::SeqCst);
        if waits % 120 == 0 {
            append_autoload_debug(format_args!(
                "fire-tfc-continue: WAIT -- dialog+0x50 load vector not ready (count@dialog+0x98=0x{load_vec_count:x} >= 8, likely uninitialized/garbage) dialog=0x{dialog:x} waits={waits}; not firing"
            ));
        }
        return;
    }
    append_autoload_debug(format_args!(
        "fire-tfc-continue: dialog+0x50 load vector READY (count={load_vec_count} < 8) -- proceeding to fire (dialog=0x{dialog:x})"
    ));
    let before = unsafe { safe_read_i32(tfc + TFC_DISPATCH_STATE_14C_OFFSET) }.unwrap_or(-1);
    // Set the save slot on mss FIRST (builder reads mss+0x1200 as the factory r8), then the dispatch
    // bit -- mirroring the native confirm handler 0x1409a9250's two key writes.
    let want_slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    let mss = unsafe { resolve_menu_system_save_load(base) };
    if let Some(mss) = mss {
        unsafe { *((mss + MSS_SAVE_SLOT_1200_OFFSET) as *mut i32) = want_slot };
    }
    unsafe { *((tfc + TFC_DISPATCH_STATE_14C_OFFSET) as *mut i32) = TFC_DISPATCH_STATE_LOAD };
    // Force the dispatcher's BUILD branch: clear tfc+0x18c (IsNotReleaseFlag55 0x14082cd60 `cmpb
    // $0,0x18c(rcx)`). The open-menu path sets this nonzero AFTER press-any-button, which makes the
    // load dispatcher 0x1409b3070 take its ABORT branch (empty job, no load -- the builder 0x9ac760
    // never fired). Clearing it guarantees the real LoadGame build. bd dispatcher-abort-branch-force-
    // tfc-18c-zero-2026-06-23.
    let nrf_before = unsafe { safe_read_usize(tfc + TFC_NOT_RELEASE_FLAG_18C_OFFSET) }
        .map(|v| (v & 0xff) as u8)
        .unwrap_or(0xff);
    unsafe { *((tfc + TFC_NOT_RELEASE_FLAG_18C_OFFSET) as *mut u8) = TFC_NOT_RELEASE_FLAG_CLEAR };
    TFC_CONTINUE_FIRED.store(1, Ordering::SeqCst);
    // Let the recurring world-stream observer log THROUGH the loading screen.
    OWN_LOAD_CONTINUE_FIRED.store(true, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "fire-tfc-continue: SET *(tfc+0x{:x})=1 (was {before}) + mss+0x{:x}=slot {want_slot} (tfc=0x{tfc:x} dialog=0x{dialog:x} owner=0x{owner:x} mss={mss:?} gm_singleton=0x{gm_singleton:x}) -- now INVOKING selector 0x{:x} (NO input)",
        TFC_DISPATCH_STATE_14C_OFFSET,
        MSS_SAVE_SLOT_1200_OFFSET,
        base + TITLE_CONTINUE_SELECTOR_RVA
    ));
    // INVOKE the Continue-item selector that consumes tfc+0x14c (it is NOT pumped from the idle menu).
    // Selector 0x1409a8eb0(rcx = &dialog_slot = owner+0xe0, rdx = out MenuJobResult*): reads
    // *(rcx)->dialog, *(dialog+0xa38)->tfc, *(tfc+0x14c)==1 -> LOAD branch -> sets r8=dialog+0x50 +
    // calls the load dispatcher 0x1409b3070 (proper ChainMenuJobs enqueue). Wrapped in catch_unwind
    // (a Rust panic is caught; a hardware AV is not). Keeps simulated_button_presses_total = 0.
    let dialog_slot = owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET;
    let mut out_job: [usize; 4] = [0; 4];
    let out_ptr = out_job.as_mut_ptr() as usize;
    let selector: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(base + TITLE_CONTINUE_SELECTOR_RVA) };
    let sel_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        selector(dialog_slot, out_ptr)
    }));
    if sel_ret.is_err() {
        append_autoload_debug(format_args!(
            "fire-tfc-continue: selector call PANICKED (caught) rcx=owner+0xe0=0x{dialog_slot:x} -- no dispatch (investigate ABI)"
        ));
        return;
    }
    append_autoload_debug(format_args!(
        "fire-tfc-continue: selector returned 0x{:x} out=[0x{:x},0x{:x},0x{:x},0x{:x}] -- LOAD branch dispatched 0x{:x}; now POSTING the built job",
        sel_ret.unwrap_or(0),
        out_job[0],
        out_job[1],
        out_job[2],
        out_job[3],
        base + 0x9b3070usize
    ));
    // INSTALL the built job as currentTopMenuJob (CSPopupMenu+0xB0) via CS::MenuJob::Assign, so the
    // NATIVE per-frame menu pump runs its Run IN CONTEXT -- the fix for the self-pump menu-jumping (our
    // ExecuteMenuJob/drain-wrapper attempts ran the job out of context and never deserialized). The
    // selector/dispatcher only BUILD + return the job (out_job[0]); the native flow normally installs
    // it into a pump-drained slot. We replicate that install. bd menu-job-install-mechanism-2026-06-23
    // + inject-job-into-native-pump-slots-recipe-2026-06-23. NO input.
    let job = out_job[0];
    if !(job > OWNER_CTX_MIN_PLAUSIBLE_PTR && job < OWNER_CTX_MAX_PLAUSIBLE_PTR) {
        append_autoload_debug(format_args!(
            "fire-tfc-continue: selector out[0]=0x{job:x} is not a plausible built MenuJob -> nothing to install (dispatcher took the abort/noop branch?)"
        ));
        return;
    }
    let _ = MENU_PUMP_KICK_PTR_RVA;
    let _ = TITLE_OWNER_MENU_LIST_130_OFFSET;
    let _ = DIALOG_MENU_QUEUE_10_OFFSET;
    let _ = MENUJOB_PUSHBACK_RVA;
    let _ = MENU_DRAIN_WRAPPER_RVA;
    let _ = EXECUTE_MENU_JOB_RVA;
    // (REMOVED the dialog+0x50 count-reset hack: a live TitleTopDialog's +0x98 count is provably
    // always 0..8 -- the garbage we saw means we read a NON-LIVE/transient object, so zeroing it just
    // masks the real lifecycle problem and would CORRUPT a valid dialog's window list. The readiness
    // gate above (count<8) + the vtable/a40 gates are the correct fail-closed guard. bd
    // forge-breaks-lifecycle-native-confirm-is-correct-context-2026-06-23.)
    let _ = DIALOG_MENUWINDOW_VEC_50_OFFSET;
    let _ = DLFIXEDVECTOR_COUNT_48_OFFSET;
    // TARGET = owner+0x130, the title flow's ACTIVE MenuJob slot that STEP_MenuJobWait runs
    // ExecuteMenuJob(&owner+0x130) on EVERY frame (the title's own per-frame pump, definitely live at
    // the title menu -- unlike currentTopMenuJob+0xB0 which a run showed is EMPTY/unused by the title).
    // owner+0x130 is a MenuJob* slot (PushBackJob AV'd there because it is NOT a FixOrderJobSequence;
    // Assign -- a slot replace -- is the right primitive). bd currenttopjob-B0-empty-not-drained.
    let _ = GLOBAL_CSMENUMAN_RVA;
    let _ = CSMENUMAN_POPUP_80_OFFSET;
    let _ = CSPOPUP_TOP_JOB_B0_OFFSET;
    let dest = owner + TITLE_OWNER_MENU_LIST_130_OFFSET;
    let old_top = unsafe { safe_read_usize(dest) }.unwrap_or(0);
    // Pre-bump the job refcount (+0x8) so it survives the Assign regardless of the wrap's count.
    if let Some(rc) = unsafe { safe_read_usize(job + MENU_JOB_REFCOUNT_8_OFFSET) } {
        unsafe { *((job + MENU_JOB_REFCOUNT_8_OFFSET) as *mut usize) = rc.wrapping_add(1) };
    }
    // Assign(rcx = dest=&owner+0x130 active slot, rdx = &scratch, r8 = &src): unref old, install ours.
    let mut scratch: usize = 0;
    let mut src: usize = job;
    let assign: unsafe extern "system" fn(usize, usize, usize) =
        unsafe { std::mem::transmute(base + MENU_JOB_ASSIGN3_RVA) };
    let scratch_ptr = (&raw mut scratch) as usize;
    let src_ptr = (&raw mut src) as usize;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        assign(dest, scratch_ptr, src_ptr)
    }));
    let new_top = unsafe { safe_read_usize(dest) }.unwrap_or(0);
    append_autoload_debug(format_args!(
        "fire-tfc-continue: *** INSTALLED job=0x{job:x} into owner+0x130 (STEP_MenuJobWait active slot) via Assign 0x{:x} (tfc+0x18c was {nrf_before}->0; owner=0x{owner:x} dest=0x{dest:x} old_top=0x{old_top:x} new_top=0x{new_top:x} panicked={}) -- STEP_MenuJobWait should pump it IN CONTEXT. Watch oracle: c30 real, player present, now_loading ***",
        base + MENU_JOB_ASSIGN3_RVA,
        r.is_err()
    ));
}
/// Install the TitleTopDialog::update hook ONCE so the Continue build runs in the pump's live frame.
/// minhook on 0x1409aac10, mirroring install_continue_trace_hooks (queue_enable + MH_ApplyQueued +
/// mem::forget to keep the hook alive). Gated by `fire_tfc_continue_enabled` at the call site.
pub(crate) unsafe fn install_title_update_hook(base: usize) {
    if TITLE_UPDATE_HOOK_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-update-hook: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "titletopdialog_update_9aac10",
            TITLE_TOP_DIALOG_UPDATE_RVA as u32,
            title_update_detour as *mut c_void,
            &TITLE_UPDATE_ORIG,
        );
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "title-update-hook: INSTALLED on TitleTopDialog::update 0x{:x} -- in-context Continue build armed",
            base + TITLE_TOP_DIALOG_UPDATE_RVA
        )),
        status => append_autoload_debug(format_args!(
            "title-update-hook: MH_ApplyQueued failed: {status:?}"
        )),
    }
    std::mem::forget(hooks);
}
/// Gated, fail-closed, one-shot readiness advance past press-any-button. Reads the built job at
/// `[step+0x130]`; once it is a valid in-image job (we are at press-any-button) and has settled, sets
/// `[job+0x1e8]=2` so the job's own predicate (0x1407a9200) completes it through the native path. Logs
/// the job struct on first sighting so the run self-confirms the offsets. ZERO input.
pub(crate) unsafe fn pab_advance_try(step: usize) {
    if !pab_advance_enabled() || PAB_ADVANCE_FIRED.load(Ordering::SeqCst) != 0 {
        return;
    }
    if step <= PAB_MIN_HEAP_PTR {
        return;
    }
    let Ok(base) = game_module_base() else {
        return;
    };
    // The press-any-button job the native node-update builds/holds.
    let job = unsafe { safe_read_usize(step + PAB_JOB_SLOT_130_OFFSET) }.unwrap_or(0);
    if job <= PAB_MIN_HEAP_PTR || (job & (core::mem::size_of::<usize>() - 1)) != 0 {
        return; // job not built yet (pre-press-any-button) -> wait
    }
    // Identity: a valid in-image vtable (fail closed -> never write a wrong/garbage object).
    let vt = unsafe { safe_read_usize(job) }.unwrap_or(0);
    if !vtable_in_game_image(vt, base) {
        return;
    }
    let count = unsafe { safe_read_i32(job + PAB_JOB_PRESS_COUNT_1E8_OFFSET) }.unwrap_or(-1) as u32;
    let keycode = unsafe { safe_read_i32(job + PAB_JOB_KEYCODE_180_OFFSET) }.unwrap_or(-1) as u32;
    let settle = PAB_ADVANCE_SETTLE.fetch_add(1, Ordering::SeqCst) + 1;
    if settle == 1 {
        append_autoload_debug(format_args!(
            "pab-advance: press-any-button job READY step=0x{step:x} job=0x{job:x} vt=0x{vt:x} [+0x1e8]count={count} [+0x180]keycode=0x{keycode:x} -- settling {PAB_ADVANCE_SETTLE_FRAMES} frames"
        ));
    }
    if settle < PAB_ADVANCE_SETTLE_FRAMES {
        return;
    }
    if count > PAB_COUNT_SANITY_MAX {
        return; // unreadable/garbage press-count -> do NOT write or latch; keep waiting
    }
    if count >= PAB_PRESS_COUNT_SATISFIED {
        // Already satisfied (a real press or prior advance) -> latch, nothing to do.
        PAB_ADVANCE_FIRED.store(1, Ordering::SeqCst);
        return;
    }
    // READINESS ADVANCE (zero-input): satisfy the job's own completion predicate.
    unsafe {
        *((job + PAB_JOB_PRESS_COUNT_1E8_OFFSET) as *mut u32) = PAB_PRESS_COUNT_SATISFIED;
    }
    PAB_ADVANCE_FIRED.store(1, Ordering::SeqCst);
    let after = unsafe { safe_read_i32(job + PAB_JOB_PRESS_COUNT_1E8_OFFSET) }.unwrap_or(-1) as u32;
    append_autoload_debug(format_args!(
        "pab-advance: *** SET [job+0x1e8]={PAB_PRESS_COUNT_SATISFIED} (was {count}, now {after}) job=0x{job:x} keycode=0x{keycode:x} settle={settle} -- readiness-gated press-any-button advance, ZERO input ***"
    ));
}
/// Install the press-any-button node-update hook ONCE (minhook, mirroring `install_title_update_hook`).
/// Gated by `pab_advance_enabled` at the call site; the detour self-gates too (pass-through until armed).
pub(crate) unsafe fn install_pab_advance_hook(base: usize) {
    if PAB_ADVANCE_HOOK_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "pab-advance-hook: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "pab_node_update_7ad1c0",
            PAB_NODE_UPDATE_RVA,
            pab_node_update_detour as *mut c_void,
            &PAB_ADVANCE_ORIG,
        );
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "pab-advance-hook: INSTALLED on PAB node-update 0x{:x} -- readiness press-any-button advance armed (zero-input)",
            base + PAB_NODE_UPDATE_RVA as usize
        )),
        status => append_autoload_debug(format_args!(
            "pab-advance-hook: MH_ApplyQueued failed: {status:?}"
        )),
    }
    std::mem::forget(hooks);
}
/// Read the TitleTopDialog FD4 state machine by NAME (is_in_state) given the title `owner` (rcx of
/// STEP_MenuJobWait). Returns `(dialog_ptr, in_fadein, in_loop, in_textfadeout, menu_opened_latch)` or
/// `None` if the dialog isn't the TitleTopDialog yet. Read-only / no side effects. Mirrors STAGE1d.
unsafe fn title_dialog_sm_state(
    owner: usize,
    base: usize,
) -> Option<(usize, bool, bool, bool, usize)> {
    if owner == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let dialog = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(0);
    if dialog == 0 {
        return None;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(0);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return None;
    }
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
    let in_fadein =
        unsafe { is_in_state(sm, base + TITLE_STATE_DESC_FADEIN_RVA) } != OWN_STEPPER_FALSE;
    let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) } != OWN_STEPPER_FALSE;
    let in_textfadeout =
        unsafe { is_in_state(sm, base + TITLE_STATE_DESC_TEXTFADEOUT_RVA) } != OWN_STEPPER_FALSE;
    let latch = unsafe { safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET) }
        .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
        .unwrap_or(0);
    Some((dialog, in_fadein, in_loop, in_textfadeout, latch))
}

/// Skip the title FadeIn ONCE: the first frame the dialog SM is settled in FadeIn (menu-open latch
/// clear), drive the FD4 state machine FadeIn->Loop by calling the game's OWN transition `SetState`
/// (deobf 0x1407499e0) with `(sm = dialog+0xa60, desc = Loop 0x142a8f9e8)`. This is EXACTLY the call
/// `CS::TitleTopDialog::update`'s input-skip branch makes on a confirm/cancel press (Ghidra: bd
/// fadein-* RE), so it is save-safe and routes through the SM's own vtable[0x150] request path (no
/// struct stomp) -- but ZERO input. `SetState` internally no-ops unless the current node is settled
/// (`[node+0x20]&0x8f >= 2`), so an early call before the node is eligible cannot corrupt the SM.
/// One-shot via `TITLE_FADEIN_SKIP_FIRED`; the dt-scale / frame-burst / anim-complete-predicate levers
/// were all runtime-falsified (bd title-anim-framedelta / pab-to-menuopen-real-breakdown / fadein-
/// predicate-75cea0). The FadeIn IS frame-paced animation -- it is just skipped by the state transition,
/// not by pacing.
unsafe fn title_anim_fadein_skip(owner: usize) {
    if TITLE_FADEIN_SKIP_FIRED.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS {
        return; // one-shot: already transitioned
    }
    if IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES {
        return;
    }
    if !(title_anim_speedup_factor() > TITLE_ANIM_SPEEDUP_MIN) {
        return; // lever off / forced to 1.0
    }
    let Ok(base) = game_module_base() else {
        return;
    };
    let st = unsafe { title_dialog_sm_state(owner, base) };
    // Light diagnostic so the SM timeline stays visible across boots.
    let n = TITLE_ANIM_DIAG_CALLS.fetch_add(1, Ordering::SeqCst);
    if n % TITLE_ANIM_DIAG_INTERVAL == 0 {
        append_autoload_debug(format_args!(
            "title-anim-diag: detour#{n} sm(dialog,fadein,loop,tfo,latch)={st:?}"
        ));
    }
    let Some((dialog, true, _, _, latch)) = st else {
        return; // not the TitleTopDialog, or not in FadeIn yet
    };
    if latch != TITLE_OWNER_SCAN_START_ADDRESS {
        return; // menu already opening -> leave the SM alone
    }
    // Fire the game's own FadeIn->Loop transition once (zero-input).
    if TITLE_FADEIN_SKIP_FIRED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return; // lost the one-shot race
    }
    let set_state: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(base + TITLE_FD4_SETSTATE_RVA) };
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    unsafe { set_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) };
    append_autoload_debug(format_args!(
        "title-anim-skip: *** SetState(sm=0x{sm:x}, Loop) via 0x{:x} -- zero-input FadeIn->Loop transition (game's own input-skip path, save-safe), skipping the title fade ***",
        base + TITLE_FD4_SETSTATE_RVA
    ));
}

/// Detour for STEP_MenuJobWait (0x140b0d400, `__fastcall(rcx=owner, rdx=task_data, ...)`). Drives the
/// one-shot FadeIn->Loop skip from the live SM state, then passes through to the original unchanged.
pub(crate) unsafe extern "system" fn title_menujob_speed_detour(
    owner: usize,
    task_data: usize,
    r8: usize,
    r9: usize,
) -> usize {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        title_anim_fadein_skip(owner)
    }));
    let orig_addr = TITLE_ANIM_SPEED_ORIG.load(Ordering::SeqCst);
    if orig_addr == TITLE_OWNER_SCAN_START_ADDRESS {
        return 0;
    }
    let orig: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig_addr) };
    unsafe { orig(owner, task_data, r8, r9) }
}

/// Install the title-anim speedup hook ONCE (MinHook, mirroring `install_pab_advance_hook`). Gated by
/// `title_anim_speedup_enabled` at the call site; the detour self-gates per frame too.
pub(crate) unsafe fn install_title_anim_speed_hook(base: usize) {
    if TITLE_ANIM_SPEED_HOOK_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-anim-speed-hook: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "title_menujob_speed_b0d400",
            TITLE_MENU_JOB_WAIT_RVA as u32,
            title_menujob_speed_detour as *mut c_void,
            &TITLE_ANIM_SPEED_ORIG,
        );
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "title-anim-speed-hook: INSTALLED on STEP_MenuJobWait 0x{:x} -- one-shot FadeIn->Loop skip armed (zero-input, save-safe)",
            base + TITLE_MENU_JOB_WAIT_RVA,
        )),
        status => append_autoload_debug(format_args!(
            "title-anim-speed-hook: MH_ApplyQueued failed: {status:?}"
        )),
    }
    std::mem::forget(hooks);
}
/// READ-ONLY trace detour for the title step-setter `SetState(owner, int state)` (deobf 0x140b0d960).
/// Logs every native state transition with a timestamp + the current owner+0xe0 (TitleTopDialog
/// holder) liveness, then calls the original UNCHANGED. Pure observation -- this is the
/// "look before acting" instrument for the menu-build-overlap lever: it reveals the exact wall-clock
/// at which BeginTitle(3) fires natively (and the full state sequence during boot), so we can decide
/// whether the 05_000_Title build has any headroom to be started earlier (overlap with init) before
/// risking a forced SetState (which has NO double-build guard). bd menu-build-overlap-lever-2026-06-24.
pub(crate) unsafe extern "system" fn title_setstate_trace_detour(owner: usize, state: i32) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let dialog = if owner > PAB_MIN_HEAP_PTR {
            unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(0)
        } else {
            0
        };
        let committed = if owner > PAB_MIN_HEAP_PTR {
            unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }.unwrap_or(-999)
        } else {
            -999
        };
        let b8 = if owner > PAB_MIN_HEAP_PTR {
            unsafe { safe_read_usize(owner + TITLE_OWNER_BEGINLOGO_LIST_GATE_B8_OFFSET) }
                .unwrap_or(0)
        } else {
            0
        };
        append_autoload_debug(format_args!(
            "title-setstate-trace: SetState(owner=0x{owner:x}, state={state}) committed_was={committed} owner+0xe0(dialog)=0x{dialog:x} owner+0xb8(gate)=0x{b8:x}"
        ));
    }));
    let orig = TITLE_SETSTATE_TRACE_ORIG.load(Ordering::SeqCst);
    if orig == TITLE_OWNER_SCAN_START_ADDRESS || orig == 0 {
        return;
    }
    let f: unsafe extern "system" fn(usize, i32) = unsafe { std::mem::transmute(orig) };
    unsafe { f(owner, state) };
}
/// Install the READ-ONLY title step-setter trace hook ONCE. Mirrors `install_pab_advance_hook`.
/// Save-safe: the detour only logs + passes through. bd menu-build-overlap-lever-2026-06-24.
pub(crate) unsafe fn install_title_setstate_trace_hook(base: usize) {
    if TITLE_SETSTATE_TRACE_HOOK_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-setstate-trace-hook: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "title_setstate_b0d960",
            TITLE_SET_STATE_RVA as u32,
            title_setstate_trace_detour as *mut c_void,
            &TITLE_SETSTATE_TRACE_ORIG,
        );
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "title-setstate-trace-hook: INSTALLED on SetState(owner,int) 0x{:x} -- read-only native state-transition timeline armed",
            base + TITLE_SET_STATE_RVA,
        )),
        status => append_autoload_debug(format_args!(
            "title-setstate-trace-hook: MH_ApplyQueued failed: {status:?}"
        )),
    }
    std::mem::forget(hooks);
}
/// Per-frame PUMP for the built LoadGame job (bd drain-dialog-plus8-not-menujob-pump-our-job-directly).
/// Runs from the recurring game task once `maybe_fire_tfc_continue` armed `TFC_DRAIN_JOB`. Calls
/// `ExecuteMenuJob(rcx = &job_slot, rdx = &FD4Time)` DIRECTLY on our built job -- it invokes the job's
/// own `vtable[2]` (the LoadGame chain's Execute), advancing deser/world-stream, and zeroes the slot
/// when done (`ShouldContinue==false`). We pump OUR job (not the dialog's `+0x8` slot, which is not a
/// MenuJob and AV'd the queue-drain wrapper). Pure native call (no input). Stops on completion (slot
/// cleared), in-world, panic, or the tick cap. Every call is `catch_unwind`-guarded.
pub(crate) unsafe fn tfc_continue_drain_tick(base: usize, frame_delta: f32) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let job = TFC_DRAIN_JOB.load(Ordering::SeqCst);
    if job == 0 || job == null {
        return;
    }
    if IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES {
        TFC_DRAIN_JOB.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "tfc-drain: in-world reached -> stop pumping (load complete)"
        ));
        return;
    }
    let ticks = TFC_DRAIN_TICKS.fetch_add(1, Ordering::SeqCst) + 1;
    if ticks > TFC_DRAIN_TICK_CAP {
        TFC_DRAIN_JOB.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "tfc-drain: tick cap {TFC_DRAIN_TICK_CAP} hit -> stop pumping (job never completed)"
        ));
        return;
    }
    // FD4Time: ExecuteMenuJob reads only +0x8 (f32 delta). Pass a 16-byte buffer with the frame delta.
    let mut time: [u8; FD4_TIME_SIZE] = [0u8; FD4_TIME_SIZE];
    time[FD4_TIME_DELTA_8_OFFSET..FD4_TIME_DELTA_8_OFFSET + core::mem::size_of::<f32>()]
        .copy_from_slice(&frame_delta.to_le_bytes());
    let time_ptr = time.as_mut_ptr() as usize;
    // ExecuteMenuJob(rcx = &job_slot, rdx = &FD4Time): cur=*rcx; AtomicInc(cur+8); cur->vtable[2](...);
    // if done -> *rcx=0. Pass a local slot (job ptr persists in TFC_DRAIN_JOB across frames).
    let mut job_slot: usize = job;
    let slot_ptr = (&raw mut job_slot) as usize;
    let exec: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(base + EXECUTE_MENU_JOB_RVA) };
    let exec_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        exec(slot_ptr, time_ptr)
    }));
    if exec_ret.is_err() {
        TFC_DRAIN_JOB.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "tfc-drain: ExecuteMenuJob 0x{:x}(rcx=&job=0x{job:x}) PANICKED (caught) at tick {ticks} -> stop pumping",
            base + EXECUTE_MENU_JOB_RVA
        ));
        return;
    }
    if job_slot == 0 {
        TFC_DRAIN_JOB.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "tfc-drain: job 0x{job:x} COMPLETED (slot cleared by ExecuteMenuJob) at tick {ticks} -> done pumping"
        ));
        return;
    }
    if ticks == 1 || ticks % (OWN_LOAD_STREAM_LOG_INTERVAL as usize) == 0 {
        append_autoload_debug(format_args!(
            "tfc-drain: tick {ticks} ExecuteMenuJob(job=0x{job:x}) delta={frame_delta} (pumping)"
        ));
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
pub(crate) unsafe fn title_press_button_component_ready(
    dialog: usize,
    base: usize,
) -> Option<TitlePressButtonComponent> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let proxy = dialog + TITLE_PRESS_START_SCENE_PROXY_B78_OFFSET;
    let proxy_vt = unsafe { safe_read_usize(proxy) }.unwrap_or(null);
    if proxy_vt != base + SCENE_OBJ_PROXY_VTABLE_RVA {
        return None;
    }
    let context =
        unsafe { safe_read_usize(proxy + SCENE_OBJ_PROXY_CONTEXT_20_OFFSET) }.unwrap_or(null);
    if context == null {
        return None;
    }
    Some(TitlePressButtonComponent { proxy, context })
}
pub(crate) unsafe fn title_dialog_state(dialog: usize, base: usize) -> TitleDialogState {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
    let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) } != OWN_STEPPER_FALSE;
    let in_textfadeout =
        unsafe { is_in_state(sm, base + TITLE_STATE_DESC_TEXTFADEOUT_RVA) } != OWN_STEPPER_FALSE;
    let menu_opened_latch =
        unsafe { safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET) }
            .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
            .unwrap_or(null);
    TitleDialogState {
        in_loop,
        in_textfadeout,
        menu_opened_latch,
    }
}
pub(crate) unsafe fn title_boot_ready(owner: usize, base: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let committed = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    let requested = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    let table =
        unsafe { safe_read_usize(owner + TITLE_OWNER_INSTANCE_TABLE_OFFSET) }.unwrap_or(null);
    let session =
        unsafe { safe_read_usize(base + SESSION_SINGLETON_144588E98_RVA) }.unwrap_or(null);
    let dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
    let dialog_vt = if dialog != null {
        unsafe { safe_read_usize(dialog) }.unwrap_or(null)
    } else {
        null
    };
    if committed != TITLE_STEP_MENU_JOB_WAIT
        || requested != TITLE_STEP_MENU_JOB_WAIT
        || table != base + INNER_TITLE_STATE_TABLE_RVA
        || session == null
        || dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA
        || unsafe { title_press_button_component_ready(dialog, base) }.is_none()
    {
        return false;
    }
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
    let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) } != OWN_STEPPER_FALSE;
    let in_textfadeout =
        unsafe { is_in_state(sm, base + TITLE_STATE_DESC_TEXTFADEOUT_RVA) } != OWN_STEPPER_FALSE;
    in_loop || in_textfadeout
}
pub(crate) unsafe fn title_scheduler_ready(owner: usize, base: usize) -> bool {
    unsafe { title_boot_ready(owner, base) }
}
pub(crate) unsafe fn product_core_autoload_ready(
    owner: usize,
    base: usize,
    gm: usize,
    slot: i32,
) -> Option<ProductCoreAutoloadReady> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if slot < OWN_STEPPER_SLOT_ZERO || gm == null {
        return None;
    }
    let committed = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    let requested = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    let table =
        unsafe { safe_read_usize(owner + TITLE_OWNER_INSTANCE_TABLE_OFFSET) }.unwrap_or(null);
    let session =
        unsafe { safe_read_usize(base + SESSION_SINGLETON_144588E98_RVA) }.unwrap_or(null);
    let game_data_man = game_data_man_ptr_or_null();
    let profile_summary = if game_data_man != null {
        unsafe { safe_read_usize(game_data_man + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(null)
    } else {
        null
    };
    let iodev = unsafe { safe_read_usize(base + IODEV_GLOBAL_RVA) }.unwrap_or(null);
    let heap_allocator = crate::runtime_heap_allocator_ptr_or_null();
    let dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
    let dialog_vt = if dialog != null {
        unsafe { safe_read_usize(dialog) }.unwrap_or(null)
    } else {
        null
    };
    let press_start = if dialog_vt == base + TITLE_TOP_DIALOG_VTABLE_RVA {
        unsafe { title_press_button_component_ready(dialog, base) }
    } else {
        None
    };
    let title_state = if dialog_vt == base + TITLE_TOP_DIALOG_VTABLE_RVA {
        Some(unsafe { title_dialog_state(dialog, base) })
    } else {
        None
    };
    if committed != TITLE_STEP_MENU_JOB_WAIT
        || requested != TITLE_STEP_MENU_JOB_WAIT
        || table != base + INNER_TITLE_STATE_TABLE_RVA
        || session == null
        || game_data_man == null
        || profile_summary == null
        || iodev == null
        || heap_allocator == null
        || press_start.is_none()
        || title_state.is_none()
    {
        return None;
    }
    let press_start = press_start?;
    let title_state = title_state?;
    Some(ProductCoreAutoloadReady {
        committed,
        requested,
        table,
        session,
        game_data_man,
        profile_summary,
        iodev,
        heap_allocator,
        title_dialog: dialog,
        title_in_loop: title_state.in_loop,
        title_in_textfadeout: title_state.in_textfadeout,
        menu_opened_latch: title_state.menu_opened_latch,
        press_start_proxy: press_start.proxy,
        press_start_context: press_start.context,
    })
}
pub(crate) unsafe fn product_core_autoload_tick(module_base: usize, slot: i32, tick: u64) -> bool {
    if !product_autoload_enabled() {
        return false;
    }
    PRODUCT_CORE_AUTOLOAD_TICKS.fetch_add(1, Ordering::SeqCst);
    let phase = OWN_STEPPER_PHASE.load(Ordering::SeqCst);
    PRODUCT_CORE_LAST_PHASE.store(phase, Ordering::SeqCst);
    if phase == OWN_STEPPER_PHASE_DONE {
        return true;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let Some(owner_ptr) = (unsafe { title_owner(module_base) }) else {
        PRODUCT_CORE_READY_BLOCKS.fetch_add(1, Ordering::SeqCst);
        PRODUCT_CORE_LAST_BLOCKER.store(PRODUCT_CORE_BLOCKER_NO_TITLE_OWNER, Ordering::SeqCst);
        if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
            append_autoload_debug(format_args!(
                "product-core-autoload: waiting for title owner before native save-load core tick={tick}"
            ));
        }
        return true;
    };
    let owner = owner_ptr as usize;
    PRODUCT_CORE_OWNER_TICKS.fetch_add(1, Ordering::SeqCst);
    PRODUCT_CORE_LAST_OWNER.store(owner, Ordering::SeqCst);
    let gm = game_man_ptr_or_null();
    if phase == OWN_STEPPER_PHASE_S2_INVOKE
        || phase == OWN_STEPPER_PHASE_S2_ACTIVATE
        || phase == OWN_STEPPER_PHASE_S2_MOUNT_POLL
        || phase == OWN_STEPPER_PHASE_S2_CONFIRM
    {
        unsafe { own_stepper_stage2(owner, module_base, gm, slot, tick, null) };
        return true;
    }
    if phase == OWN_STEPPER_PHASE_MENU
        && FULLREAD_PHASE.load(Ordering::SeqCst) == FULLREAD_PHASE_GUARD
    {
        // Native Continue can reset title-menu visual latches while its modal-confirm branch waits.
        // The product intent is to disable that confirm wait after the native load has produced
        // loaded-slot evidence, so keep the post-submit guard running instead of re-gating on title
        // visuals that are no longer authoritative.
        let guard_ready = ProductCoreAutoloadReady {
            committed: TITLE_STATE_OWNER_GONE,
            requested: TITLE_STATE_OWNER_GONE,
            table: null,
            session: null,
            game_data_man: null,
            profile_summary: null,
            iodev: null,
            heap_allocator: null,
            title_dialog: null,
            title_in_loop: false,
            title_in_textfadeout: false,
            menu_opened_latch: null,
            press_start_proxy: null,
            press_start_context: null,
        };
        unsafe { product_continue_autoload_tick(owner, module_base, gm, slot, tick, &guard_ready) };
        return true;
    }
    let Some(ready) = (unsafe { product_core_autoload_ready(owner, module_base, gm, slot) }) else {
        let committed = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
            .unwrap_or(TITLE_STATE_OWNER_GONE);
        let requested = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_OFFSET) }
            .unwrap_or(TITLE_STATE_OWNER_GONE);
        let table =
            unsafe { safe_read_usize(owner + TITLE_OWNER_INSTANCE_TABLE_OFFSET) }.unwrap_or(null);
        let session = unsafe { safe_read_usize(module_base + SESSION_SINGLETON_144588E98_RVA) }
            .unwrap_or(null);
        let game_data_man = game_data_man_ptr_or_null();
        let profile_summary = if game_data_man != null {
            unsafe { safe_read_usize(game_data_man + SLOT_MANAGER_CONTAINER_OFFSET) }
                .unwrap_or(null)
        } else {
            null
        };
        let iodev = unsafe { safe_read_usize(module_base + IODEV_GLOBAL_RVA) }.unwrap_or(null);
        let heap_allocator = crate::runtime_heap_allocator_ptr_or_null();
        let dialog =
            unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
        let dialog_vt = if dialog != null {
            unsafe { safe_read_usize(dialog) }.unwrap_or(null)
        } else {
            null
        };
        let press_start_proxy = dialog + TITLE_PRESS_START_SCENE_PROXY_B78_OFFSET;
        let press_start_vt = if dialog != null {
            unsafe { safe_read_usize(press_start_proxy) }.unwrap_or(null)
        } else {
            null
        };
        let press_start_context = if press_start_vt == module_base + SCENE_OBJ_PROXY_VTABLE_RVA {
            unsafe { safe_read_usize(press_start_proxy + SCENE_OBJ_PROXY_CONTEXT_20_OFFSET) }
                .unwrap_or(null)
        } else {
            null
        };
        let (title_loop, title_textfadeout, menu_opened_latch) =
            if dialog_vt == module_base + TITLE_TOP_DIALOG_VTABLE_RVA {
                let state = unsafe { title_dialog_state(dialog, module_base) };
                (state.in_loop, state.in_textfadeout, state.menu_opened_latch)
            } else {
                (false, false, null)
            };
        PRODUCT_CORE_LAST_TITLE_DIALOG.store(dialog, Ordering::SeqCst);
        PRODUCT_CORE_LAST_TITLE_DIALOG_VT.store(dialog_vt, Ordering::SeqCst);
        PRODUCT_CORE_LAST_TITLE_IN_LOOP.store(title_loop as usize, Ordering::SeqCst);
        PRODUCT_CORE_LAST_TITLE_IN_TEXTFADEOUT.store(title_textfadeout as usize, Ordering::SeqCst);
        PRODUCT_CORE_LAST_MENU_OPENED_LATCH.store(menu_opened_latch, Ordering::SeqCst);
        PRODUCT_CORE_LAST_PRESS_START_PROXY.store(press_start_proxy, Ordering::SeqCst);
        PRODUCT_CORE_LAST_PRESS_START_VT.store(press_start_vt, Ordering::SeqCst);
        PRODUCT_CORE_LAST_PRESS_START_CONTEXT.store(press_start_context, Ordering::SeqCst);
        let blocker =
            if committed != TITLE_STEP_MENU_JOB_WAIT || requested != TITLE_STEP_MENU_JOB_WAIT {
                PRODUCT_CORE_BLOCKER_TITLE_OWNER_STATE
            } else if table != module_base + INNER_TITLE_STATE_TABLE_RVA {
                PRODUCT_CORE_BLOCKER_TITLE_TABLE
            } else if session == null {
                PRODUCT_CORE_BLOCKER_SESSION
            } else if game_data_man == null {
                PRODUCT_CORE_BLOCKER_GAME_DATA_MAN
            } else if profile_summary == null {
                PRODUCT_CORE_BLOCKER_PROFILE_SUMMARY
            } else if iodev == null {
                PRODUCT_CORE_BLOCKER_IODEV
            } else if heap_allocator == null {
                PRODUCT_CORE_BLOCKER_HEAP_ALLOCATOR
            } else if dialog_vt != module_base + TITLE_TOP_DIALOG_VTABLE_RVA {
                PRODUCT_CORE_BLOCKER_TITLE_DIALOG
            } else if press_start_vt != module_base + SCENE_OBJ_PROXY_VTABLE_RVA
                || press_start_context == null
            {
                PRODUCT_CORE_BLOCKER_PRESS_START
            } else if !title_loop
                && !title_textfadeout
                && menu_opened_latch == OWN_STEPPER_MENU_OPENED_NO
            {
                PRODUCT_CORE_BLOCKER_TITLE_STATE
            } else {
                PRODUCT_CORE_BLOCKER_UNKNOWN
            };
        PRODUCT_CORE_READY_BLOCKS.fetch_add(1, Ordering::SeqCst);
        PRODUCT_CORE_LAST_BLOCKER.store(blocker, Ordering::SeqCst);
        if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
            append_autoload_debug(format_args!(
                "product-core-autoload: waiting for core readiness owner=0x{owner:x} state={committed}/{requested} table=0x{table:x} session=0x{session:x} gm=0x{gm:x} gdm=0x{game_data_man:x} profile=0x{profile_summary:x} iodev=0x{iodev:x} heap=0x{heap_allocator:x} title_loop={title_loop} title_textfadeout={title_textfadeout} menu_latch={menu_opened_latch} press_start_proxy=0x{press_start_proxy:x} press_start_vt=0x{press_start_vt:x} press_start_ctx=0x{press_start_context:x} slot={slot} tick={tick}"
            ));
        }
        return true;
    };
    PRODUCT_CORE_LAST_TITLE_DIALOG.store(ready.title_dialog, Ordering::SeqCst);
    PRODUCT_CORE_LAST_TITLE_DIALOG_VT.store(
        unsafe { safe_read_usize(ready.title_dialog) }.unwrap_or(null),
        Ordering::SeqCst,
    );
    PRODUCT_CORE_LAST_TITLE_IN_LOOP.store(ready.title_in_loop as usize, Ordering::SeqCst);
    PRODUCT_CORE_LAST_TITLE_IN_TEXTFADEOUT
        .store(ready.title_in_textfadeout as usize, Ordering::SeqCst);
    PRODUCT_CORE_LAST_MENU_OPENED_LATCH.store(ready.menu_opened_latch, Ordering::SeqCst);
    PRODUCT_CORE_LAST_PRESS_START_PROXY.store(ready.press_start_proxy, Ordering::SeqCst);
    PRODUCT_CORE_LAST_PRESS_START_VT.store(
        unsafe { safe_read_usize(ready.press_start_proxy) }.unwrap_or(null),
        Ordering::SeqCst,
    );
    PRODUCT_CORE_LAST_PRESS_START_CONTEXT.store(ready.press_start_context, Ordering::SeqCst);
    PRODUCT_CORE_READY_SUCCESSES.fetch_add(1, Ordering::SeqCst);
    PRODUCT_CORE_LAST_BLOCKER.store(PRODUCT_CORE_BLOCKER_READY, Ordering::SeqCst);
    if phase == OWN_STEPPER_PHASE_MENU {
        if ready.menu_opened_latch == OWN_STEPPER_MENU_OPENED_NO
            && OWN_STEPPER_MENU_OPENED
                .compare_exchange(
                    OWN_STEPPER_MENU_OPENED_NO,
                    OWN_STEPPER_CALL_INC,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
        {
            // Lever-3 (narrow registrar advance): the native title press-accept handler 0x1409b1260
            // sets the menu-system singleton's +0 byte to 1 BEFORE tail-jumping to this same
            // registrar -- the missing piece that makes it open the menu IN PLACE rather than
            // spawning the competing dialog a bare self-fire produced (and the route that reaches
            // the main menu without the language/ToS the broad global accept byte over-triggers).
            // Replicate that flag set, gated, just before the (already vtable-validated) open_menu.
            // Zero-input, no save write.
            if title_registrar_advance_gate_enabled() {
                let singleton = unsafe {
                    *((module_base + TITLE_MENU_TRANSITION_SINGLETON_RVA) as *const usize)
                };
                if singleton != TITLE_OWNER_SCAN_START_ADDRESS && singleton != null {
                    unsafe { *(singleton as *mut u8) = TITLE_MENU_TRANSITION_FLAG_SET_VALUE };
                    append_autoload_debug(format_args!(
                        "title_registrar_advance: set menu-transition singleton [0x{:x}]->+0=1 before open-menu",
                        module_base + TITLE_MENU_TRANSITION_SINGLETON_RVA
                    ));
                }
            }
            let open_menu: unsafe extern "system" fn(usize) =
                unsafe { std::mem::transmute(module_base + TITLE_TOP_DIALOG_OPEN_MENU_RVA) };
            unsafe { open_menu(ready.title_dialog) };
            timeline_event(
                "T_menu_open",
                tick,
                format_args!(
                    "product-core dialog=0x{:x} press_start_proxy=0x{:x}",
                    ready.title_dialog, ready.press_start_proxy
                ),
            );
            append_autoload_debug(format_args!(
                "product-core-autoload: PRESS BUTTON component ready; self-fire native open-menu 0x{:x}(dialog=0x{:x}) on validated title dialog + latch-clear before native save-load core; TitleTopDialog::open_menu writes latch and does not require Loop/TextFadeout state",
                module_base + TITLE_TOP_DIALOG_OPEN_MENU_RVA,
                ready.title_dialog
            ));
            return true;
        }
        if !ready.title_in_textfadeout && ready.menu_opened_latch == OWN_STEPPER_MENU_OPENED_NO {
            if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: waiting for title open-menu semantic confirmation dialog=0x{:x} loop={} textfadeout={} latch={} press_start_proxy=0x{:x} slot={slot} tick={tick}",
                    ready.title_dialog,
                    ready.title_in_loop,
                    ready.title_in_textfadeout,
                    ready.menu_opened_latch,
                    ready.press_start_proxy
                ));
            }
            return true;
        }
        if !unsafe { product_continue_action_ready(&ready, module_base, gm, slot) } {
            if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: waiting for native Continue action readiness owner=0x{owner:x} state={}/{} dialog=0x{:x} menu_latch={} press_start_proxy=0x{:x} slot={slot} -- no direct_build/input fallback",
                    ready.committed,
                    ready.requested,
                    ready.title_dialog,
                    ready.menu_opened_latch,
                    ready.press_start_proxy
                ));
            }
            return true;
        }
        unsafe { product_continue_autoload_tick(owner, module_base, gm, slot, tick, &ready) };
    }
    let phase_now = OWN_STEPPER_PHASE.load(Ordering::SeqCst);
    if phase_now == OWN_STEPPER_PHASE_S2_INVOKE
        || phase_now == OWN_STEPPER_PHASE_S2_ACTIVATE
        || phase_now == OWN_STEPPER_PHASE_S2_MOUNT_POLL
        || phase_now == OWN_STEPPER_PHASE_S2_CONFIRM
    {
        unsafe { own_stepper_stage2(owner, module_base, gm, slot, tick, null) };
    }
    true
}
pub(crate) unsafe fn title_menu_action_ready(owner: usize, base: usize) -> Option<MenuActionNode> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
    if dialog == null {
        return None;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(null);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return None;
    }
    let registry =
        unsafe { safe_read_usize(dialog + DIALOG_ROW_REGISTRY_A48_OFFSET) }.unwrap_or(null);
    if !vtable_in_game_image(registry, base) {
        return None;
    }
    let (member_node, window_item) = unsafe { scan_dialog_for_loadgame(owner, base) };
    let node = member_node?;
    let node_vt = unsafe { safe_read_usize(node) }.unwrap_or(null);
    if node_vt != base + MEMBERFUNCJOB_VTABLE_RVA {
        return None;
    }
    const MEMBER_DIALOG_10: usize = core::mem::size_of::<usize>() + core::mem::size_of::<usize>();
    const MEMBER_FN_18: usize = 0x18;
    const MEMBER_ADJ_20: usize = 0x20;
    const JMP_HOPS: usize = 6;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    let member_dialog = unsafe { safe_read_usize(node + MEMBER_DIALOG_10) }.unwrap_or(null);
    let member_fn = unsafe { safe_read_usize(node + MEMBER_FN_18) }.unwrap_or(null);
    let member_adjust = unsafe { safe_read_usize(node + MEMBER_ADJ_20) }.unwrap_or(null);
    if member_fn == null {
        return None;
    }
    let factory_abs = base + LIVE_DIALOG_FACTORY_RVA;
    let mut target = member_fn;
    let mut hop = HOP_START;
    while hop < JMP_HOPS && target != null {
        if target == factory_abs {
            return Some(MenuActionNode {
                node,
                node_vt,
                registry,
                member_dialog,
                member_fn,
                member_adjust,
                window_item: window_item.unwrap_or(null),
            });
        }
        match unsafe { decode_thunk_hop(target) } {
            Some(next) => target = next,
            None => break,
        }
        hop += HOP_STEP;
    }
    None
}
pub(crate) unsafe fn title_live_dialog_fire_ready(
    owner: usize,
    base: usize,
) -> Option<LiveDialogFireReady> {
    const TITLE_FLOW_CONTEXT_VTABLE_RVA: usize = 0x2ac7f20;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if !unsafe { title_scheduler_ready(owner, base) } {
        return None;
    }
    let title_dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
    if title_dialog == null {
        return None;
    }
    let title_dialog_vt = unsafe { safe_read_usize(title_dialog) }.unwrap_or(null);
    if title_dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return None;
    }
    let menu_opened_latch = unsafe {
        safe_read_usize(title_dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET)
            .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
            .unwrap_or(null)
    };
    if menu_opened_latch == OWN_STEPPER_MENU_OPENED_NO {
        return None;
    }
    let registry_vt =
        unsafe { safe_read_usize(title_dialog + DIALOG_ROW_REGISTRY_A48_OFFSET) }.unwrap_or(null);
    if registry_vt != base + SCENE_OBJ_PROXY_VTABLE_RVA {
        return None;
    }
    let capture_slot = title_dialog + DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET;
    let capture = unsafe { safe_read_usize(capture_slot) }.unwrap_or(null);
    if !unsafe { is_heap_aligned_ptr(capture) } {
        return None;
    }
    let capture_vt = unsafe { safe_read_usize(capture) }.unwrap_or(null);
    if capture_vt != base + TITLE_FLOW_CONTEXT_VTABLE_RVA {
        return None;
    }
    let menu_window = LATCHED_MENU_WINDOW.load(Ordering::SeqCst);
    if !unsafe { is_heap_aligned_ptr(menu_window) } {
        return None;
    }
    let menu_window_vt = unsafe { safe_read_usize(menu_window) }.unwrap_or(null);
    Some(LiveDialogFireReady {
        title_dialog,
        title_dialog_vt,
        capture_slot,
        capture,
        capture_vt,
        registry_vt,
        menu_opened_latch,
        menu_window,
        menu_window_vt,
    })
}
/// True if `vt` is a startup MessageBoxDialog the auto-accept should drive: the base MessageBoxDialog
/// vtable OR the CS::SaveRetryDialog subclass vtable (the wrapper 0x1407af9a0 overrides base ->
/// SaveRetryDialog AFTER the builder, so a base-only check bails once the override lands). bd
/// offline-title-modal-is-saveretrydialog.
pub(crate) fn is_startup_msgbox_vtable(vt: usize, base: usize) -> bool {
    vt == base + MSGBOX_DIALOG_VTABLE_RVA || vt == base + SAVE_RETRY_DIALOG_VTABLE_RVA
}
pub(crate) fn startup_modal_blocking_state() -> StartupModalBlockingState {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog = CONNECTION_ERROR_DIALOG.load(Ordering::SeqCst);
    if dialog == null {
        return StartupModalBlockingState::Clear;
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
    if base == null || !is_startup_msgbox_vtable(vt, base) {
        CONNECTION_ERROR_DIALOG.store(null, Ordering::SeqCst);
        return StartupModalBlockingState::Clear;
    }
    let closing = unsafe { safe_read_usize(dialog + MSGBOX_CLOSING_LATCH_3B0_OFFSET) }
        .map(|v| v & MSGBOX_LATCH_BYTE_MASK)
        .unwrap_or(MSGBOX_CLOSING_YES);
    if closing == MSGBOX_CLOSING_YES {
        CONNECTION_ERROR_DIALOG.store(null, Ordering::SeqCst);
        return StartupModalBlockingState::Clear;
    }
    StartupModalBlockingState::Blocking {
        dialog,
        vtable: vt,
        closing_latch: closing,
    }
}
pub(crate) unsafe fn profile_load_dialog_ready(
    base: usize,
    dialog: usize,
    want_slot: i32,
    log_pending: bool,
) -> Option<ProfileLoadDialogReady> {
    const PROFILE_LOAD_ACTIVATE_RVA: usize = 0x009a4670;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let pld_vt = base + PROFILE_LOAD_DIALOG_VTABLE_RVA;
    let dvt = if dialog != null {
        unsafe { safe_read_usize(dialog) }.unwrap_or(null)
    } else {
        null
    };
    if dvt != pld_vt {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: waiting for ProfileLoadDialog dialog=0x{dialog:x} vt=0x{dvt:x} want=0x{pld_vt:x}"
            ));
        }
        return None;
    }
    let lav =
        unsafe { safe_read_usize(dvt + DIALOG_LOAD_ACTIVATE_VTSLOT_A0_OFFSET) }.unwrap_or(null);
    if lav != base + PROFILE_LOAD_ACTIVATE_RVA {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: load_activate slot not ready lav=0x{lav:x} want=0x{:x} dvt=0x{dvt:x}",
                base + PROFILE_LOAD_ACTIVATE_RVA
            ));
        }
        return None;
    }
    let gdm = game_data_man_ptr_or_null();
    let player_game_data = if gdm != null {
        unsafe { safe_read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }.unwrap_or(null)
    } else {
        null
    };
    if player_game_data == null {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: PlayerGameData null gdm=0x{gdm:x} -- load_activate would assert"
            ));
        }
        return None;
    }
    let bound = unsafe { safe_read_i32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET) }
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    let cursor_now = unsafe { safe_read_i32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    let expected_slot = if want_slot == OWN_STEPPER_SLOT_NONE {
        cursor_now
    } else {
        want_slot
    };
    let cursor_target = if want_slot == OWN_STEPPER_SLOT_NONE {
        cursor_now
    } else if bound == OWN_STEPPER_CALL_INC as i32 {
        OWN_STEPPER_SLOT_ZERO
    } else {
        want_slot
    };
    if expected_slot < OWN_STEPPER_SLOT_ZERO
        || bound <= OWN_STEPPER_SLOT_ZERO
        || cursor_target < OWN_STEPPER_SLOT_ZERO
        || cursor_target >= bound
    {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: slot rows not ready/valid want={want_slot} expected={expected_slot} cursor_target={cursor_target} cursor={cursor_now} bound={bound} dialog=0x{dialog:x}"
            ));
        }
        return None;
    }
    let load_job_ctx = unsafe {
        safe_read_usize(dialog + core::mem::offset_of!(ProfileLoadDialogLayout, load_job_ctx))
    }
    .unwrap_or(null);
    if !unsafe { is_heap_aligned_ptr(load_job_ctx) } {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: load_job_ctx not ready dialog=0x{dialog:x} ctx=0x{load_job_ctx:x}"
            ));
        }
        return None;
    }
    let load_job_ctx_vt = unsafe { safe_read_usize(load_job_ctx) }.unwrap_or(null);
    if !vtable_in_game_image(load_job_ctx_vt, base) {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: load_job_ctx vtable invalid ctx=0x{load_job_ctx:x} vt=0x{load_job_ctx_vt:x} base=0x{base:x}"
            ));
        }
        return None;
    }
    Some(ProfileLoadDialogReady {
        dialog,
        dvt,
        bound,
        cursor_now,
        cursor_target,
        expected_slot,
        load_activate: lav,
        load_job_ctx,
        load_job_ctx_vt,
        player_game_data,
    })
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
    let gm = game_man_ptr_or_null();
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
    let slotmgr = game_data_man_ptr_or_null();
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
    // The THIRD menu-open popup ("Starting in offline mode", GR_System_Message 401170) is gated by
    // TitleFlowContext->notReleaseFlag55 = !Menu_IsEnableOnlineMode(). Force that getter false so the
    // game's own ctx-init (0x14082d0d0) writes notReleaseFlag55=1 each time, the title-flow offline step
    // (0x14082fda0) takes the clean no-popup branch, and the Continue/Load/NewGame rows build with ZERO
    // MessageBoxDialog builds. Race-free + offline-gated (Seamless online unaffected). bd
    // menu-open-3rd-popup-offline-mode-notice-2026-06-23 / er-effects-rs-yvf.
    let menu_online_off = patch_3byte_stub(
        base,
        MENU_ONLINE_MODE_DISABLE_RVA,
        MENU_ONLINE_MODE_EXPECTED_FIRST,
        ONLINE_DISABLE_STUB,
        "menu-online-mode-disable",
    );
    append_autoload_debug(format_args!(
        "online-disable: Menu_IsEnableOnlineMode@0x{:x} patched ok={menu_online_off} -> xor eax,eax;ret (notReleaseFlag55 becomes 1 -> no 'Starting in offline mode' popup -> title rows build)",
        base + MENU_ONLINE_MODE_DISABLE_RVA
    ));
    let _ = ONLINE_PREDICATE_DISABLE_RVA;
}
/// Force `CS::CSWindowImp::IsGameInForeground` (0x14266def0) to always return true (`mov al,1; ret`)
/// so the engine's flip pacer never applies the unfocused-window fps throttle -- the probe boots at
/// full speed regardless of focus (bd runtime-probe-unfocused-window-throttle). Same RWX/flush
/// pattern as the online-disable patch; validates the expected 0x40 prologue first.
pub(crate) fn apply_foreground_force() {
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!("foreground-force: module base unavailable"));
        return;
    };
    let target = (base + FOREGROUND_FORCE_RVA) as *mut u8;
    let existing = unsafe { *target };
    if existing != FOREGROUND_FORCE_EXPECTED_FIRST {
        append_autoload_debug(format_args!(
            "foreground-force: ABORT -- byte at 0x{:x} is 0x{existing:x}, expected 0x{FOREGROUND_FORCE_EXPECTED_FIRST:x}",
            base + FOREGROUND_FORCE_RVA
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
        append_autoload_debug(format_args!("foreground-force: VirtualProtect failed"));
        return;
    }
    let mut i = TITLE_OWNER_SCAN_START_ADDRESS;
    while i < ONLINE_DISABLE_PATCH_LEN {
        unsafe { *target.add(i) = FOREGROUND_FORCE_STUB[i] };
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
        "foreground-force: patched IsGameInForeground 0x{:x} -> mov al,1;ret (no unfocused fps throttle)",
        base + FOREGROUND_FORCE_RVA
    ));
}
/// Force the SaveLoad2 storage-select op gate to pass cold (bd b80-ROOTCAUSE-cold-no-user-signin):
/// patch the sign-in check to always return true and the user-index resolver to return 0, so the
/// select-op ctor (0x14240f1b0) builds the runnable and the load proceeds to SLLoadSession -> read
/// -> b80 RESIDENT. Save-safe (in-memory code patch; no save write). Called once from the cold-mount
/// attempt so normal play is unaffected unless a cold mount is requested.
pub(crate) fn apply_signin_force(base: usize) {
    let s = patch_3byte_stub(
        base,
        SIGNIN_FORCE_RVA,
        SIGNIN_FORCE_EXPECTED_FIRST,
        SIGNIN_FORCE_STUB,
        "signin-force",
    );
    let u = patch_3byte_stub(
        base,
        USERINDEX_FORCE_RVA,
        USERINDEX_FORCE_EXPECTED_FIRST,
        USERINDEX_FORCE_STUB,
        "userindex-force",
    );
    append_autoload_debug(format_args!(
        "signin-force: signin@0x{:x} ok={s} -> mov al,1;ret | userindex@0x{:x} ok={u} -> xor eax,eax;ret (select-op gate now passes: signed-in as user 0)",
        base + SIGNIN_FORCE_RVA,
        base + USERINDEX_FORCE_RVA
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
    let gm = game_man_ptr_or_null();
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
    let game_man = game_man_ptr_or_null();
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
    let game_man = game_man_ptr_or_null();
    let slot_mgr = game_data_man_ptr_or_null();
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
pub(crate) unsafe fn find_title_owner_by_vtable(module_base: usize) -> Option<*mut u8> {
    TITLE_OWNER_SCAN_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
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
                            TITLE_OWNER_SCAN_VTABLE_HITS.fetch_add(1, Ordering::SeqCst);
                            let cursor = chunk_base + i;
                            TITLE_OWNER_SCAN_LAST_CANDIDATE.store(cursor, Ordering::SeqCst);
                            // Validate the per-instance state-table pointer (rejects
                            // the stray .data match 0x1000ffc58); fault-tolerant.
                            let instance_table = unsafe {
                                safe_read_usize(cursor + TITLE_OWNER_INSTANCE_TABLE_OFFSET)
                            };
                            let state_value =
                                unsafe { safe_read_i32(cursor + TITLE_OWNER_STATE_OFFSET) };
                            TITLE_OWNER_SCAN_LAST_TABLE.store(
                                instance_table.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
                                Ordering::SeqCst,
                            );
                            TITLE_OWNER_SCAN_LAST_STATE_BITS.store(
                                state_value.map_or(usize::MAX, |s| s as u32 as usize),
                                Ordering::SeqCst,
                            );
                            let table_ok =
                                instance_table == Some(module_base + INNER_TITLE_STATE_TABLE_RVA);
                            let state_ok = state_value.is_some_and(|s| {
                                (TITLE_OWNER_MIN_STATE..=TITLE_OWNER_MAX_STATE).contains(&s)
                            });
                            if table_ok && state_ok {
                                return Some(cursor as *mut u8);
                            }
                            if !table_ok {
                                TITLE_OWNER_SCAN_TABLE_REJECTS.fetch_add(1, Ordering::SeqCst);
                            } else if !state_ok {
                                TITLE_OWNER_SCAN_STATE_REJECTS.fetch_add(1, Ordering::SeqCst);
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
