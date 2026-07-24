use std::{
    ffi::c_void,
    fmt::Write as _,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, Once, OnceLock,
    },
    time::{Duration, Instant},
};

use std::os::windows::ffi::OsStrExt as _;

use crate::input_blocker::{InputBlocker, InputFlags};
use crate::mh::{MH_ApplyQueued, MH_Initialize, MhHook, MH_STATUS};
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use er_tpf::{DdsHeaderMode, DdsImage, Tpf};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use windows::{
    core::{BOOL, PCSTR},
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM},
        System::{
            LibraryLoader::{GetModuleHandleA, GetProcAddress},
            Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION},
            SystemServices::DLL_PROCESS_ATTACH,
            Threading::GetCurrentProcessId,
        },
        UI::WindowsAndMessaging::{
            ClipCursor, EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
            WM_KEYDOWN, WM_KEYUP,
        },
    },
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, ffi::*, hooks::*, telemetry::*};

use super::*;

pub(crate) fn arm_product_autoload_from_request(request: &SaveLoader) {
    // Do not write debug/file logs here: this arm path runs before the product's happy-path
    // autoload should perform any filesystem I/O. Keep request-specific diagnostics in later
    // telemetry/probe phases after startup is safely past the arming edge.
    // Product autoload is the release/default behavior. Do not make it depend on smoke-only env
    // variables, `er-effects-autoload.txt`, or the experimental DirectMenuLoad method: the title/menu
    // visual suppression is also default-on for real runs, so leaving the load driver unarmed creates
    // a release soft lock (hidden native menu with no product-core load tick). Explicit no-autoload,
    // telemetry-only, and native-profile-capture runs remain opt-out/diagnostic paths.
    //
    // Product autoload stays ARMED even during a missing-save boot: this arm runs ONCE at DllMain,
    // and gating it on the (then-pending) missing-save latch would leave it unarmed forever, so the
    // load never resumes after the pick (observed 2026-07-07: the redirect activated but the boot
    // never advanced to a world load). The world-LOAD drive is instead gated DYNAMICALLY in
    // `own_stepper_enabled()` on `missing_save_selection_pending()`, which re-enables the frame the
    // pick clears the latch. The loading bar advances normally and sticks at the save-check (the
    // ShowProgressJob CONTINUE-loop) with the overlay picker on top; the pick resumes it.
    if !autoload_disabled() && !save_override_telemetry_only() && !native_profile_capture_enabled()
    {
        PRODUCT_AUTOLOAD_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }

    // Arm additional menu-free path flags from the reliable autoload-file channel, independent of slot
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
    if let Some(slot) = request.slot() {
        if slot < OWN_STEPPER_SLOT_ZERO {
            return;
        }

        // OWN_STEPPER_SLOT is the shared target slot for the menu-free own_stepper /
        // native_fullread / cold_char_mount / native-continue paths AND the experimental menu-driven
        // product_core path. Set it whenever a valid slot is configured, regardless of method, so the
        // known-good zero-input smoke path does not depend on a fragile env-method side effect.
        OWN_STEPPER_SLOT.store(slot, Ordering::SeqCst);
    }
    if request.method() == SaveLoadMethod::DirectMenuLoad && experimental_direct_menu_load_enabled()
    {
        // Kept as an explicit diagnostic/direct-menu compatibility path. The release/default arm above
        // is what makes a plain ME3-loaded DLL work without hidden env vars.
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
    // Missing-save picker gate: do NOT arm the zero-input menu-open while the user still has not
    // chosen a save. This accept byte makes the native registrar build the Continue/Load/NewGame
    // rows in its own update frame; if that happens BEFORE the pick installs the save redirect, the
    // game's save-check finds no loadable save and constructs the Continue row through the DISABLED
    // MenuWindowJob ctor (idle accept predicate 0x1407add70) instead of the native-accept ctor
    // (0x1407ad810). product_continue then refuses that idle Continue forever ("ignoring diagnostic
    // Continue candidate ... waiting for semantic native-accept MENU_CONTINUE_ITEM") and the row is
    // never rebuilt -> soft-lock at the title right after the pick. Deferring the arm until the pick
    // clears `missing_save_selection_pending()` (redirect active, save-check hold released) makes the
    // menu build once with the save present, so Continue comes up enabled and fires. This is the same
    // dynamic missing-save gating the load-drive already documents in `arm_product_autoload_from_request`.
    // Returns BEFORE the one-shot latch so the shot is preserved for the post-pick frame; the per-frame
    // product tick re-calls this until the arm succeeds. See bd
    // missing-save-picker-disabled-continue-soft-lock-2026-07-07.
    if missing_save_selection_pending() {
        return;
    }
    // NOTE: do NOT gate this on the picked slot appearing in ProfileSummary. That looks correct (the
    // Continue row builds DISABLED because ProfileSummary is still empty ~1s after the pick), but it
    // DEADLOCKS: runtime-proven 2026-07-07 that opening the menu here is itself what makes the game
    // read the picked save into ProfileSummary. Blocking the open until ProfileSummary is populated
    // means it is never populated -> the menu never opens (44s+ of "waiting to arm", latch=0). The
    // Continue row must be built ENABLED some other way (rebuild after population, or populate
    // ProfileSummary before the open), tracked separately -- never by waiting here.
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
    // Require the dialog SETTLED in Loop FIRST (read-only probe of the live state by name, no side
    // effects). This MUST precede the a40 "already open" shortcut below: on a 2nd-switch return-title
    // teardown the a40 latch is transiently != 0 WHILE the dialog is NOT yet in Loop, and taking the
    // shortcut there would consume the TITLE_ACCEPT_BYTE_GATE_FIRED one-shot before the title ever parks
    // at press-any-button -- so the accept byte is never set, the menu never opens, and the consecutive
    // switch soft-locks at the covered title (root-caused from the log delta 2026-07-15: switch #2 had no
    // "set [..]=1 on settled TitleTopDialog" line, jumping straight to PRESS BUTTON ready). Returning here
    // while not-in-Loop preserves the one-shot so the byte is set once the title genuinely settles.
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
    let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) } != OWN_STEPPER_FALSE;
    if !in_loop {
        return; // not settled (e.g. return-title teardown) -> wait; do NOT consume the one-shot
    }
    // Only at the parked press-any-button (menu not yet open): a40 latch == 0.
    let a40 = unsafe { safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET) }
        .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
        .unwrap_or(1);
    if a40 != OWN_STEPPER_MENU_OPENED_NO {
        TITLE_ACCEPT_BYTE_GATE_FIRED.store(true, Ordering::SeqCst); // genuinely open in Loop -> nothing to do
        return;
    }
    let first_arm = !TITLE_ACCEPT_BYTE_GATE_FIRED.swap(true, Ordering::SeqCst);
    let press_start_proxy = dialog + TITLE_PRESS_START_SCENE_PROXY_B78_OFFSET;
    let press_start_vt = unsafe { safe_read_usize(press_start_proxy) }.unwrap_or(0);
    let press_start_context = if press_start_vt == base + SCENE_OBJ_PROXY_VTABLE_RVA {
        unsafe { safe_read_usize(press_start_proxy + SCENE_OBJ_PROXY_CONTEXT_20_OFFSET) }
            .unwrap_or(0)
    } else {
        0
    };
    if press_start_vt == base + SCENE_OBJ_PROXY_VTABLE_RVA {
        unsafe {
            hide_title_press_start_proxy(base, dialog, press_start_proxy, press_start_context)
        };
    }
    if native_profile_capture_enabled() {
        const TITLE_CURSOR_LOAD_GAME: i32 = 1;
        let before = unsafe { safe_read_i32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }.unwrap_or(-1);
        unsafe { *((dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) as *mut i32) = TITLE_CURSOR_LOAD_GAME };
        append_autoload_debug(format_args!(
            "title-accept-byte: native-profile-capture set TitleTopDialog cursor [dialog+0xb0c] {before}->1 before native accept byte"
        ));
    }
    unsafe {
        *((base + TITLE_GLOBAL_ACCEPT_BYTE_RVA) as *mut u8) = TITLE_PROCEED_GATE_SET_VALUE;
    }
    if first_arm {
        append_autoload_debug(format_args!(
            "title-accept-byte: set [0x{:x}]=1 on settled TitleTopDialog (Loop, a40==0) -- zero-input NATURAL menu-open (registrar runs in native update frame -> Continue/Load/NewGame rows build + drain); will retry until native a40/menu-open latch flips",
            base + TITLE_GLOBAL_ACCEPT_BYTE_RVA
        ));
    }
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
    // NOTE: this function is NOT the active load-commit path in the product autoload (the native
    // accept-byte drain is). The real portrait render window is implemented in product_core_autoload_tick.
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
    // GUARD (user spec 3+4 "any slot active" / "never load a slot if none active"): resolve the ACTIVE
    // slot holding a real character instead of blindly loading the configured slot. The gold save's
    // configured slot 0 is a NULL slot -> loading it spawns the new-game INTRO cutscene + a null character.
    // resolve_active_load_slot() validates via the contamination-free RECORD fingerprint and falls back to
    // the best active slot; OWN_STEPPER_SLOT_NONE means nothing loadable (or profile records not ready).
    let configured = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    let want_slot = unsafe { resolve_active_load_slot(configured) };
    if want_slot < OWN_STEPPER_SLOT_ZERO {
        let waits = TFC_LOAD_VEC_WAIT_TICKS.fetch_add(1, Ordering::SeqCst);
        if waits % 120 == 0 {
            append_autoload_debug(format_args!(
                "fire-tfc-continue: REFUSE to fire -- no ACTIVE save slot (configured={configured}; profile records not real/ready). Never loading a null slot (would spawn the new-game intro). waits={waits}"
            ));
        }
        return;
    }
    if want_slot != configured {
        append_autoload_debug(format_args!(
            "fire-tfc-continue: configured slot {configured} is null/inactive -> loading best ACTIVE slot {want_slot} instead (user guard: load an active slot, never a null one)"
        ));
    }
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
