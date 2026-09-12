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

use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use er_tpf::{DdsHeaderMode, DdsImage, Tpf};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM},
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
};

#[allow(unused_imports)]
use crate::compat::*;
#[allow(unused_imports)]
use crate::compat::*;

pub fn arm_product_autoload_from_request(request: &SaveLoader) {
    // Product autoload is the release/default behavior. Do not make it depend on smoke-only env
    // variables, `er-quickload-autoload.txt`, or the experimental DirectMenuLoad method: the title/menu
    // visual suppression is also default-on for real runs, so leaving the load driver unarmed creates
    // a release soft lock (hidden native menu with no product-core load tick). Explicit no-autoload,
    // telemetry-only, and native-profile-capture runs remain opt-out/diagnostic paths.
    //
    // Product autoload stays armed even during a missing-save boot: this arm runs once at DllMain,
    // and gating it on the (then-pending) missing-save latch would leave it unarmed forever, so the
    // load never resumes after the pick (observed 2026-07-07: the redirect activated but the boot
    // never advanced to a world load). The world-load drive is instead gated dynamically in
    // `own_stepper_enabled()` on `missing_save_selection_pending()`, which re-enables the frame the
    // pick clears the latch. The loading bar advances normally and sticks at the save-check (the
    // ShowProgressJob continue-loop) with the overlay picker on top; the pick resumes it.
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
        // flag too -- that is what makes own_stepper_patch_once install the detour so our handler
        // runs each frame. own_load takes precedence inside the handler (like cold_char_mount).
        OWN_LOAD_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.own_load_continue() {
        // The final guarded world-stream step rides on the same own_load probe (own_load_drive runs
        // the proven verify-only parse, then fires the guarded continue). Arm own_load too so the
        // probe actually runs even if only own_load_continue was set in the autoload file.
        OWN_LOAD_CONTINUE_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_LOAD_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.own_dispatch() {
        // The m28 direct-enqueue lever rides the same own-load path: it only fires after our
        // continue_confirm sets OWN_LOAD_CONTINUE_FIRED. Arm own_load + own_load_continue too so the
        // path that sets that flag actually runs when only own_dispatch was set in the autoload file.
        OWN_DISPATCH_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_LOAD_CONTINUE_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_LOAD_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.own_load_install_job() {
        // The LoadGame-job install lever rides the same own-load path: it runs instead of the
        // continue_confirm/SetState5 step at the end of own_load_drive. Arm own_load (+ own_stepper,
        // which installs the idx10 detour that runs own_load_drive) so the probe actually runs even if
        // only own_load_install_job was set in the autoload file. Deliberately does not arm
        // own_load_continue (the save-writing SetState5 lever): this is the non-SetState5 alternative.
        OWN_LOAD_INSTALL_JOB_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_LOAD_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.own_load_pump() {
        // Path B private-pump lever ("own the load"): builds the LoadGame job with real mss-derived ctx
        // then ticks its Run privately each frame to completion + drives the transition on Success. Rides
        // the same own-load path: it runs instead of the install/continue step at the end of
        // own_load_drive. Arm own_load (+ own_stepper, which installs the idx10 detour that runs
        // own_load_drive) so the probe actually runs even if only own_load_pump was set in the autoload
        // file. Does not arm own_load_continue here -- the pump fires the guarded SetState5 transition
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
        // native_fullread / cold_char_mount / native-continue paths and the experimental menu-driven
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
pub fn product_core_ready_blocker_label(blocker: usize) -> &'static str {
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
pub unsafe fn selectbot_probe_once(module_base: usize, tick: u64) {
    if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL != TITLE_OWNER_SCAN_START_ADDRESS as u64 {
        return;
    }
    // Owner-independent module globals: sample these always. After the latch
    // advances the inner TitleStep to Finish (state 11 -> -1) the inner owner is
    // torn down, but `pump_ran` (does the outer MenuLoop spin up?) and the latch
    // byte live in module globals, so we must still capture them post-cascade.
    // Every one of these three is a resolved address read through a fault-tolerant reader, and
    // both halves are load-bearing on 1.17. Resolution stops the read landing on whatever now
    // occupies a moved global; the fault-tolerant reader is what makes a refusal survivable,
    // because `game_data_addr` answers 0 when it will not translate and a raw `*(0 as *const _)`
    // turns a refusal into a crash. A probe must not be able to kill the boot it is observing.
    let registry = read_global_ptr(
        module_base,
        SELECTBOT_REGISTRY_GLOBAL_RVA,
        "SELECTBOT_REGISTRY_GLOBAL_RVA",
    );
    let load_gate = read_global_u8(
        module_base,
        SELECTBOT_LOAD_GATE_RVA,
        "SELECTBOT_LOAD_GATE_RVA",
    );
    let input_manager = read_global_ptr(
        module_base,
        SELECTBOT_INPUT_MANAGER_GLOBAL_RVA,
        "SELECTBOT_INPUT_MANAGER_GLOBAL_RVA",
    );
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
    // Lever-1 title-accept experiment: set the proceed latch [0x143d856a0]=1 once,
    // only while the inner owner is confirmed at MenuJobWait (state 10), so the
    // native MenuJobWait handler advances itself to state 11 (Finish) on its next
    // tick. Sampling continues above so the cascade (state, pump_ran, registry) is
    // observed after the write. Gated separately from the read-only probe.
    if title_proceed_gate_enabled()
        && state == TITLE_STEP_MENU_JOB_WAIT_STATE
        && !TITLE_PROCEED_GATE_FIRED.swap(true, Ordering::SeqCst)
    {
        let stored = unsafe {
            write_global_u8(
                module_base,
                SELECTBOT_LOAD_GATE_RVA,
                "SELECTBOT_LOAD_GATE_RVA",
                TITLE_PROCEED_GATE_SET_VALUE,
            )
        };
        let after = read_global_u8(
            module_base,
            SELECTBOT_LOAD_GATE_RVA,
            "SELECTBOT_LOAD_GATE_RVA",
        );
        append_autoload_debug(format_args!(
            "title_proceed_gate: stored={stored} value={after} at state {state} tick={tick}"
        ));
    }
    // Lever-2 (option c): satisfy the global menu-accept side-effect zero-input. At the parked
    // press-any-button title (state 10), set the global accept byte 0x144589bdc=1 once so the
    // native TitleTopDialog::update runs the open-menu registrar on its own next tick -- the
    // natural advance (builds Continue/Load + transfers focus -> select-layer/router_this), which
    // a direct registrar self-fire could not do without spawning a competing dialog that reverted.
    // Not an input event (this is the decoded accept flag, like the ToS-accepted flag). Gated off
    // by default. Sampling above continues so the cascade (menu_opened, router_this) is observed.
    if title_accept_byte_gate_enabled()
        && state == TITLE_STEP_MENU_JOB_WAIT_STATE
        && !TITLE_ACCEPT_BYTE_GATE_FIRED.swap(true, Ordering::SeqCst)
    {
        let stored = unsafe {
            write_global_u8(
                module_base,
                TITLE_GLOBAL_ACCEPT_BYTE_RVA,
                "TITLE_GLOBAL_ACCEPT_BYTE_RVA",
                TITLE_PROCEED_GATE_SET_VALUE,
            )
        };
        let after = read_global_u8(
            module_base,
            TITLE_GLOBAL_ACCEPT_BYTE_RVA,
            "TITLE_GLOBAL_ACCEPT_BYTE_RVA",
        );
        append_autoload_debug(format_args!(
            "title_accept_byte_gate: stored={stored} value={after} at state {state} tick={tick} -- zero-input natural menu-open"
        ));
    }
}
/// Recipe A: arm the game's own built-in title autoload with zero input.
///
/// The save-manager per-frame update `0x14067f5d0` performs an autoload when the
/// save slot (`GameMan+0xac0`) is set and the force flag `0x143d856a0` is non-zero
/// — it primes the world/streaming subsystems through the game's own state
/// machine (which `force_play_game` bypassed). So we set the slot via the native
/// setter `0x67a810` and raise the force flag once, then let the engine load.
/// The earlier crash from raising that flag came from leaving the slot at -1 (a
/// Finish teardown with no load armed); arming the slot first is the fix.
pub unsafe fn native_autoload_once(module_base: usize, slot: i32, tick: u64) {
    if tick < TITLE_NATIVE_JOB_MIN_TICK {
        return;
    }
    let game_man = game_man_ptr_or_null();
    if game_man == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let save_state =
        unsafe { *((game_man + GAME_MAN_SAVE_STATE_B80_OFFSET) as *const u8) };
    if NATIVE_AUTOLOAD_ARMED.load(Ordering::SeqCst) {
        // Observe the load cascade after arming.
        if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
            let slot_now =
                unsafe { *((game_man + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
            let load14 =
                unsafe { *((game_man + FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET) as *const i32) };
            let latch = read_global_u8(
                module_base,
                SELECTBOT_LOAD_GATE_RVA,
                "SELECTBOT_LOAD_GATE_RVA",
            );
            let b72 = unsafe { *((game_man + GAME_MAN_ARM_FLAG_B72_OFFSET) as *const u8) };
            let csfeman = unsafe {
                *((er_game_base::mem::game_data_addr(
                    module_base,
                    CSFEMAN_SINGLETON_RVA,
                    "CSFEMAN_SINGLETON_RVA",
                )) as *const usize)
            };
            append_autoload_debug(format_args!(
                "native_autoload: observe slot={slot_now} b80={save_state} load14={load14} latch={latch} b72={b72} csfeman=0x{csfeman:x} tick={tick}"
            ));
        }
        return;
    }
    if save_state != TITLE_NATIVE_JOB_TASK_DATA_ZERO {
        append_autoload_debug(format_args!(
            "native_autoload: SL device busy (saveState b80={save_state}) before arm; skipping tick={tick}"
        ));
        return;
    }
    // Corrected recipe (native-continue-and-slotn-recipe-2026): the latch
    // 0x143d856a0 must stay clear; the arm flag is [GameMan+0xb72]=1. (The old
    // code set the latch to 1, which the disasm proves aborts the load.)
    let latch_before = read_global_u8(
        module_base,
        SELECTBOT_LOAD_GATE_RVA,
        "SELECTBOT_LOAD_GATE_RVA",
    );
    let set_save_slot: unsafe extern "system" fn(i32) = unsafe {
        std::mem::transmute(
            match title_fn(
                FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA,
                "FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA",
            ) {
                Some(address) => address,
                None => return,
            },
        )
    };
    unsafe { set_save_slot(slot) };
    let slot_after = unsafe { *((game_man + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
    unsafe {
        *((game_man + GAME_MAN_ARM_FLAG_B72_OFFSET) as *mut u8) = TITLE_PROCEED_GATE_SET_VALUE;
    }
    NATIVE_AUTOLOAD_ARMED.store(true, Ordering::SeqCst);
    let csfeman = unsafe {
        *((er_game_base::mem::game_data_addr(
            module_base,
            CSFEMAN_SINGLETON_RVA,
            "CSFEMAN_SINGLETON_RVA",
        )) as *const usize)
    };
    append_autoload_debug(format_args!(
        "native_autoload: armed slot={slot_after} b72=1 latch_left={latch_before} b80={save_state} csfeman=0x{csfeman:x} tick={tick}"
    ));
}
/// Ask the engine to close the title dialog a switch left standing over the world.
///
/// # What this used to call, and why it could not work
///
/// It called `TITLE_TOP_DIALOG_CLEANUP_RVA` (1.16.2 `0x1409a8890`) on the dialog at owner+0xe0.
/// That function is `CS::TitleTopDialog::~TitleTopDialog`: it stores the `TitleTopDialog` vtable
/// back into the object, frees the +0xd60 allocation, destroys six `CSScaleformValue`s and chains
/// to `~MenuWindow` at `FUN_1407430c0`. It deregisters nothing and closes nothing. The object
/// stays in the owner's `DLFixedVector<MenuWindow*>` at owner+0xe0 with its reference count still
/// 1, so the engine keeps pumping an object whose destructor has run.
///
/// Measured on the live process 2026-09-11 19:44 (pid 3031799, the 19:39 run): the call ran at
/// +27947ms on `dialog=0x318ca880` and returned `0x1429d11d8`, which is the vtable it had just
/// written back. Six minutes later owner+0xe0 still held `0x318ca880`, owner+0x128 still read 1,
/// and `0x318ca880+0x3b0` -- the `MenuWindow::Close` re-entry latch -- still read 0, so the window
/// had never been closed once. `PRESS ANY BUTTON` and the publisher footer were on screen over a
/// loaded character for that whole stretch.
///
/// # Why it now only looks
///
/// It was changed on 2026-09-11 to ask the engine's own `CloseAsFailed(MenuWindow*)` instead, and
/// that was tried live the same evening. It did exactly half a teardown and the half it did was
/// worse than doing nothing: `title-dialog-close: ... dialog=0x16a66280 owner_window_count 1->1`.
/// `CloseAsFailed` sets a terminal result and calls `MenuWindow::Close`; the deregistration lives
/// in `FUN_1407ada40`, which runs only when the window's `MenuWindowJob` is run, and after a switch
/// commits that job is not run at all. So `PRESS ANY BUTTON` and the footer went away and a
/// registered, undrawn window stayed behind holding the player's input. The user's report of that
/// build: "I got spat into the title screen with the logo, but the footer and the press any button
/// were not visible. I cannot appear to do anything."
///
/// # What the same run showed the real defect to be
///
/// The title dialog is not a leftover from before the switch. It is built during the switch,
/// because the switch drops to the title on its way through: the log's world-lost line
/// (`c30 0xe000000 -> 0xa010000`) at `+136606ms`, the commit at `+136781ms`, and `T_controllable` with
/// `LOAD-CORRECTNESS name="Vagabond" level=9` at `+138722ms`. The character does load; the title it
/// passes through builds a dialog that the reload then comes up underneath. That is
/// `oracle_world_lost_to_title`, and it is the thing to fix. A window closed after the fact is a
/// symptom being tidied away, and this tidying cost the player their controls.
///
/// So this gate reports the orphan and its count and leaves it alone. The number it prints is the
/// same field `oracle_title_owner_menu_window_count` samples.
pub unsafe fn cleanup_title_dialog_after_world_once(module_base: usize, frame: u64) {
    static CLOSE_REQUESTED_FOR_DIALOG: AtomicUsize =
        AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
    static NO_OWNER_LOGGED: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
    static WRONG_VTABLE_LOGGED: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
    if !cleanup_title_dialog_after_world_enabled() {
        return;
    }
    let Some(owner_ptr) = (unsafe { title_owner(module_base) }) else {
        // Once, not every frame: this runs on the game task and the old body could only ever log
        // here a single time because its own latch had already burned.
        if NO_OWNER_LOGGED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
            == TITLE_OWNER_SCAN_START_ADDRESS
        {
            append_autoload_debug(format_args!(
                "title-dialog-close: skipped frame={frame} no title owner"
            ));
        }
        return;
    };
    let owner_addr = owner_ptr as usize;
    let dialog = unsafe { safe_read_usize(owner_addr + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    if dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    if CLOSE_REQUESTED_FOR_DIALOG.load(Ordering::SeqCst) == dialog {
        return;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let expected_vt = er_game_base::mem::game_data_addr(
        module_base,
        TITLE_TOP_DIALOG_VTABLE_RVA,
        "TITLE_TOP_DIALOG_VTABLE_RVA",
    );
    if dialog_vt != expected_vt {
        if WRONG_VTABLE_LOGGED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
            == TITLE_OWNER_SCAN_START_ADDRESS
        {
            append_autoload_debug(format_args!(
                "title-dialog-close: skipped frame={frame} dialog=0x{dialog:x} vt=0x{dialog_vt:x} expected=0x{expected_vt:x}"
            ));
        }
        return;
    }
    let window_count = unsafe {
        safe_read_usize(owner_addr + TITLE_OWNER_MENU_WINDOW_COUNT_128_OFFSET)
    }
    .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    CLOSE_REQUESTED_FOR_DIALOG.store(dialog, Ordering::SeqCst);
    let mut remaining_slots = TITLE_OWNER_SCAN_START_ADDRESS;
    let mut idx = PROFILE_MODEL_REND_SLOT_START;
    while idx < PROFILE_MODEL_REND_TABLE_SLOTS {
        let slot = er_game_base::mem::game_data_addr_offset(
            module_base,
            PROFILE_MODEL_REND_TABLE_RVA,
            "PROFILE_MODEL_REND_TABLE_RVA",
            idx * PROFILE_MODEL_REND_TABLE_STRIDE,
        );
        let ptr = unsafe { safe_read_usize(slot) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        if ptr != TITLE_OWNER_SCAN_START_ADDRESS {
            remaining_slots += PROFILE_MODEL_REND_SLOT_STEP;
        }
        idx += PROFILE_MODEL_REND_SLOT_STEP;
    }
    append_autoload_debug(format_args!(
        "title-dialog-orphan: observed frame={frame} owner=0x{owner_addr:x} dialog=0x{dialog:x} owner_window_count={window_count} remaining_profile_rend_slots={remaining_slots} -- left alone on purpose; a close with no reaper is worse than the orphan"
    ));
}
/// Autonomous press-any-button -> open-menu (zero-input): drive the title to the open main menu
/// ourselves so a run needs no real button press. When the live TitleTopDialog (owner+0xe0) is settled
/// in the FD4 `Loop` state with the menu-opened latch (dialog+0xa40) still 0, call the native open-menu
/// registrar `0x1409b24e0(rcx=dialog)` -- the exact action a button press triggers -- to open the menu
/// (sets a40=1). Requires online-disable (`er-quickload-offline.txt`) so the connection modal is skipped
/// and the SM reaches Loop. One-shot. Then `maybe_fire_tfc_continue` (gated a40==1) fires Continue. No
/// input. (Same self-fire the own_stepper STAGE1d uses, extracted for the tfc flow.)
pub unsafe fn maybe_auto_open_menu(base: usize) {
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
    if dialog == 0
        || dialog_vt
            != er_game_base::mem::game_data_addr(
                base,
                TITLE_TOP_DIALOG_VTABLE_RVA,
                "TITLE_TOP_DIALOG_VTABLE_RVA",
            )
    {
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
    // Require the dialog settled in Loop: the registrar internally set_state(TextFadeOut) re-checks
    // node flags&0x8f>=2 and bails if not settled (FadeIn would no-op / corrupt). Read-only probe.
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 = unsafe {
        std::mem::transmute(
            match title_fn(
                TITLE_TOP_DIALOG_IS_IN_STATE_RVA,
                "TITLE_TOP_DIALOG_IS_IN_STATE_RVA",
            ) {
                Some(address) => address,
                None => return,
            },
        )
    };
    let in_loop = unsafe {
        is_in_state(
            sm,
            er_game_base::mem::game_data_addr(
                base,
                TITLE_STATE_DESC_LOOP_RVA,
                "TITLE_STATE_DESC_LOOP_RVA",
            ),
        )
    } != OWN_STEPPER_FALSE;
    if !in_loop {
        return;
    }
    // Route the registrar in-place (zero-input): the native open-menu call sites write a "mode" byte at
    // [*(base+TITLE_MENU_TRANSITION_SINGLETON_RVA)]+0 before jumping to the registrar -- press-accept
    // 0x1409b1260 sets it =1 (open main menu in place), pump/back paths set it =0. A bare open_menu with
    // the byte left stale may route the registrar into an error-modal branch. Replicate the press-accept
    // set (subagent-C static RE: product native-open with this byte set reached the menu with 0 msgbox).
    // Null-/readability-guarded; no save write, no input. bd er-effects-rs-0ye + title-accept-to-registrar-narrow-path-143d5dea8.
    let transition_singleton = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            TITLE_MENU_TRANSITION_SINGLETON_RVA,
            "TITLE_MENU_TRANSITION_SINGLETON_RVA",
        ))
    }
    .unwrap_or(null);
    if transition_singleton != null && unsafe { safe_read_usize(transition_singleton) }.is_some() {
        unsafe { *(transition_singleton as *mut u8) = TITLE_MENU_TRANSITION_FLAG_SET_VALUE };
        append_autoload_debug(format_args!(
            "tfc-auto-open: set menu-transition mode byte [*(0x{:x})]+0=1 before open-menu (route registrar in-place)",
            er_game_base::mem::game_data_addr(
                base,
                TITLE_MENU_TRANSITION_SINGLETON_RVA,
                "TITLE_MENU_TRANSITION_SINGLETON_RVA"
            )
        ));
    }
    let open_menu: unsafe extern "system" fn(usize) = unsafe {
        std::mem::transmute(
            match title_fn(
                TITLE_TOP_DIALOG_OPEN_MENU_RVA,
                "TITLE_TOP_DIALOG_OPEN_MENU_RVA",
            ) {
                Some(address) => address,
                None => return,
            },
        )
    };
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        open_menu(dialog)
    }));
    TFC_AUTO_MENU_OPENED.store(1, Ordering::SeqCst);
    let _ = null;
    append_autoload_debug(format_args!(
        "tfc-auto-open: fired open-menu registrar 0x{:x}(dialog=0x{dialog:x}) on Loop+a40==0 (panicked={}) -- autonomous press-any-button equivalent, NO input",
        er_game_base::mem::game_data_addr(
            base,
            TITLE_TOP_DIALOG_OPEN_MENU_RVA,
            "TITLE_TOP_DIALOG_OPEN_MENU_RVA"
        ),
        r.is_err()
    ));
}
/// Zero-input natural menu-open (the row-building path). At the parked press-any-button title
/// (TitleTopDialog settled in "Loop", menu not yet open a40==0), set the decoded global menu-accept
/// byte 0x144589bdc=1 once so the game's own `TitleTopDialog::update` accept-gate runs the open-menu
/// registrar in its native frame -- which posts the Continue/Load/NewGame MenuJob chain and drains it
/// (MenuWindow::Update 0x140745520) in the same native flow, so the rows actually build. A direct
/// registrar self-fire (`maybe_auto_open_menu`) only posts the chain; the native update does not drain
/// a chain it did not open itself, so the rows never build (continue-scan = 0 nodes; bd
/// rowbuild-mechanism-incontext-openmenu-2026-06-23 + title-global-accept-byte-144589bdc). This is the
/// decoded accept flag the input pipeline sets on press -- Not a synthesized DInput/keystate/XInput
/// event -> still `simulated_button_presses_total == 0`. Save-safe (menu-UI build, no save write). The
/// ToS/language over-trigger this byte caused in 2026-06 is now neutralized by the offline-mode +
/// Menu_IsEnableOnlineMode patches, so it should reach the main menu cleanly; the msgbox/policy oracles
/// will catch any regression. One-shot via TITLE_ACCEPT_BYTE_GATE_FIRED, latched only after the gating
/// passes so a not-yet-settled title does not consume the shot.
pub unsafe fn maybe_set_title_accept_byte(base: usize) {
    // Missing-save picker gate: do not arm the zero-input menu-open while the user still has not
    // chosen a save. This accept byte makes the native registrar build the Continue/Load/NewGame
    // rows in its own update frame; if that happens before the pick installs the save redirect, the
    // game's save-check finds no loadable save and constructs the Continue row through the disabled
    // MenuWindowJob ctor (idle accept predicate 0x1407add70) instead of the native-accept ctor
    // (0x1407ad810). product_continue then refuses that idle Continue forever ("ignoring diagnostic
    // Continue candidate ... waiting for semantic native-accept MENU_CONTINUE_ITEM") and the row is
    // never rebuilt -> soft-lock at the title right after the pick. Deferring the arm until the pick
    // clears `missing_save_selection_pending()` (redirect active, save-check hold released) makes the
    // menu build once with the save present, so Continue comes up enabled and fires. This is the same
    // dynamic missing-save gating the load-drive already documents in `arm_product_autoload_from_request`.
    // Returns before the one-shot latch so the shot is preserved for the post-pick frame; the per-frame
    // product tick re-calls this until the arm succeeds. See bd
    // missing-save-picker-disabled-continue-soft-lock-2026-07-07.
    if missing_save_selection_pending() {
        return;
    }
    // NOTE: do not gate this on the picked slot appearing in ProfileSummary. That looks correct (the
    // Continue row builds disabled because ProfileSummary is still empty ~1s after the pick), but it
    // DEADLOCKS: runtime-proven 2026-07-07 that opening the menu here is itself what makes the game
    // read the picked save into ProfileSummary. Blocking the open until ProfileSummary is populated
    // means it is never populated -> the menu never opens (44s+ of "waiting to arm", latch=0). The
    // Continue row must be built enabled some other way (rebuild after population, or populate
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
    if dialog == 0
        || dialog_vt
            != er_game_base::mem::game_data_addr(
                base,
                TITLE_TOP_DIALOG_VTABLE_RVA,
                "TITLE_TOP_DIALOG_VTABLE_RVA",
            )
    {
        return;
    }
    // Require the dialog settled in Loop first (read-only probe of the live state by name, no side
    // effects). This must precede the a40 "already open" shortcut below: on a 2nd-switch return-title
    // teardown the a40 latch is transiently != 0 while the dialog is not yet in Loop, and taking the
    // shortcut there would consume the TITLE_ACCEPT_BYTE_GATE_FIRED one-shot before the title ever parks
    // at press-any-button -- so the accept byte is never set, the menu never opens, and the consecutive
    // switch soft-locks at the covered title (root-caused from the log delta 2026-07-15: switch #2 had no
    // "set [..]=1 on settled TitleTopDialog" line, jumping straight to press button ready). Returning here
    // while not-in-Loop preserves the one-shot so the byte is set once the title genuinely settles.
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 = unsafe {
        std::mem::transmute(
            match title_fn(
                TITLE_TOP_DIALOG_IS_IN_STATE_RVA,
                "TITLE_TOP_DIALOG_IS_IN_STATE_RVA",
            ) {
                Some(address) => address,
                None => return,
            },
        )
    };
    let in_loop = unsafe {
        is_in_state(
            sm,
            er_game_base::mem::game_data_addr(
                base,
                TITLE_STATE_DESC_LOOP_RVA,
                "TITLE_STATE_DESC_LOOP_RVA",
            ),
        )
    } != OWN_STEPPER_FALSE;
    if !in_loop {
        return; // not settled (e.g. return-title teardown) -> wait; do not consume the one-shot
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
    let press_start_context = if press_start_vt
        == er_game_base::mem::game_data_addr(
            base,
            SCENE_OBJ_PROXY_VTABLE_RVA,
            "SCENE_OBJ_PROXY_VTABLE_RVA",
        ) {
        unsafe { safe_read_usize(press_start_proxy + SCENE_OBJ_PROXY_CONTEXT_20_OFFSET) }
            .unwrap_or(0)
    } else {
        0
    };
    if press_start_vt
        == er_game_base::mem::game_data_addr(
            base,
            SCENE_OBJ_PROXY_VTABLE_RVA,
            "SCENE_OBJ_PROXY_VTABLE_RVA",
        )
    {
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
    // The store that moved. This byte is the whole zero-input menu-open: the game's own
    // `TitleTopDialog::update` reads it, runs the open-menu registrar in its native frame, and
    // that native frame is what builds and drains the Continue/Load/NewGame rows. On 1.17 the
    // global moved +0x4080 (0x4589bdc -> 0x458dc5c) and this store was raw, so it wrote a
    // neighbouring byte, the log said `set [0x144589bdc]=1` as if it had worked, and the title
    // sat at press button forever "waiting for native a40/menu-open latch". A store is the one
    // access that cannot be allowed through unresolved: reading a moved global returns nonsense,
    // writing one corrupts whatever now lives there.
    let stored = unsafe {
        write_global_u8(
            base,
            TITLE_GLOBAL_ACCEPT_BYTE_RVA,
            "TITLE_GLOBAL_ACCEPT_BYTE_RVA",
            TITLE_PROCEED_GATE_SET_VALUE,
        )
    };
    if !stored {
        // Refused: say so once rather than arming a lever that cannot fire. The one-shot latch is
        // already taken, so this reports the arm that will not happen instead of retrying forever.
        if first_arm {
            append_autoload_debug(format_args!(
                "title-accept-byte: REFUSED -- TITLE_GLOBAL_ACCEPT_BYTE_RVA has no verified address for this build; the zero-input menu-open cannot be armed"
            ));
        }
        return;
    }
    if first_arm {
        append_autoload_debug(format_args!(
            "title-accept-byte: set TITLE_GLOBAL_ACCEPT_BYTE_RVA=1 on settled TitleTopDialog (Loop, a40==0) -- zero-input NATURAL menu-open (registrar runs in native update frame -> Continue/Load/NewGame rows build + drain); will retry until native a40/menu-open latch flips"
        ));
    }
}
/// Connection-state offline lever (zero-input, save-safe) -- the milestone-3 fix. The title's
/// network/session event handlers (`CSLuaEventScriptImitation::On{LanCutError,DisconnectGameServer,
/// FailedGetBlockNum,NpServerSignOut,DisconnectEOSServer,...}`) build the "Cannot connect to network /
/// connection lost / network error" `GR_System_Message` MessageBoxDialogs that our offline pab boot
/// raises at menu-open. Each handler is guarded by `if (IsInOnlineMode()) { if
/// (IsServerConnectionEnabled() && ...) { build popup } }`, which reduces to two `GameMan` bytes:
/// `isInOnlineMode = [GameMan+0xBC8]`, `serverConnectionEnabled = [GameMan+0xBC9]`
/// (`GameMan = *(base+GAME_SAVE_SLOT_SINGLETON_RVA)`; getter `0x14067a030` is `mov rax,[0x143d69918];
/// movzx eax,[rax+0xBC8]; ret` -- Verified by deobf disasm). NOTE the existing online-disable patches
/// that getter's return value, but the handlers consult the bytes (directly / via getters our patch
/// does not cover), so the patch alone does not gate them. Forcing both bytes to 0 each title frame
/// short-circuits the whole connection-loss family at the source (the guard fails -> no popup is ever
/// enqueued -- not suppressed, not dismissed). Pure offline state, no save write, no input. Readable-
/// guarded so a not-yet-initialized GameMan can never fault the game thread. bd er-effects-rs-0ye
/// (subagent-D GR_System_Message gate, subagent-B premise: modals are network notices not SaveRetry).
pub unsafe fn force_offline_connection_bytes(base: usize) {
    const IS_IN_ONLINE_MODE_BC8_OFFSET: usize = 0xBC8;
    const SERVER_CONNECTION_ENABLED_BC9_OFFSET: usize = 0xBC9;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let game_man = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            GAME_SAVE_SLOT_SINGLETON_RVA,
            "GAME_SAVE_SLOT_SINGLETON_RVA",
        ))
    }
    .unwrap_or(null);
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
/// See `fire_tfc_continue_enabled`. Runs from the recurring game task; self-gates and fires once.
/// Pure in-process field writes (no input, no native call) -- the native menu pump's selector
/// (`0x1409a8eb0`) picks up `tfc+0x14c==1` on its next tick and dispatches the load through the
/// engine's own job pump (the proven user-Continue path, which avoids the FixOrderJobSequence
/// overflow that killed the factory-direct `own_load_pump`). Logs before/after so a probe sees the
/// exact write.
pub unsafe fn maybe_fire_tfc_continue(base: usize) {
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
    // Require the settled main-menu state (STEP_MenuJobWait), i.e. press-any-button -> BeginLogo done.
    let committed = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    if committed != TITLE_STEP_MENU_JOB_WAIT {
        return;
    }
    // Require "the rest of GameMan is set up": the GetSaveSlot singleton (*(base+0x3d69918)) non-null.
    let gm_singleton = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            GAME_SAVE_SLOT_SINGLETON_RVA,
            "GAME_SAVE_SLOT_SINGLETON_RVA",
        ))
    }
    .unwrap_or(0);
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
    if dialog == 0
        || dialog_vt
            != er_game_base::mem::game_data_addr(
                base,
                TITLE_TOP_DIALOG_VTABLE_RVA,
                "TITLE_TOP_DIALOG_VTABLE_RVA",
            )
    {
        return;
    }
    // Require the main menu to be open, not the bare press-any-button screen. State 10
    // (MenuJobWait) occurs at both; the open-menu registrar 0x1409b24e0 sets the menu-opened latch
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
    // NOTE: this function is not the active load-commit path in the product autoload (the native
    // accept-byte drain is). The real portrait render window is implemented in product_core_autoload_tick.
    let tfc = unsafe { safe_read_usize(dialog + DIALOG_OWNER_CTX_A38_OFFSET) }.unwrap_or(0);
    if !(tfc > OWNER_CTX_MIN_PLAUSIBLE_PTR && tfc < OWNER_CTX_MAX_PLAUSIBLE_PTR) {
        return;
    }
    // Readiness gate on dialog+0x50 (the load MenuWindowJob's push target -- selector sets r8=dialog+
    // 0x50). A run showed its count field (dialog+0x50+0x48 = dialog+0x98) can hold garbage (a pointer
    // ~0x7fff..., not a small count) -> dialog+0x50 is not yet a valid/ready vector in our self-opened
    // flow, and firing then crashes (insert reads garbage count -> 'out of memory'). Fire only when the
    // count is a plausible small value with room (< 8); else wait (do not consume the one-shot) and
    // retry next frame. If it never becomes valid we simply never fire (no crash). bd
    // dialog-plus0x50-not-a-vector-built-job-miscontextualized-2026-06-23.
    let load_vec_count = unsafe {
        safe_read_usize(dialog + DIALOG_MENUWINDOW_VEC_50_OFFSET + DLFIXEDVECTOR_COUNT_48_OFFSET)
    }
    .unwrap_or(usize::MAX);
    if load_vec_count >= 8 {
        let waits = TFC_LOAD_VEC_WAIT_TICKS.fetch_add(1, Ordering::SeqCst);
        if waits.is_multiple_of(120) {
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
    // Set the save slot on mss first (builder reads mss+0x1200 as the factory r8), then the dispatch
    // bit -- mirroring the native confirm handler 0x1409a9250's two key writes.
    // Guard (user spec 3+4 "any slot active" / "never load a slot if none active"): resolve the active
    // slot holding a real character instead of blindly loading the configured slot. The gold save's
    // configured slot 0 is a NULL slot -> loading it spawns the new-game intro cutscene + a null character.
    // resolve_active_load_slot() validates via the contamination-free record fingerprint and falls back to
    // the best active slot; OWN_STEPPER_SLOT_NONE means nothing loadable (or profile records not ready).
    let configured = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    let want_slot = unsafe { resolve_active_load_slot(configured) };
    if want_slot < OWN_STEPPER_SLOT_ZERO {
        let waits = TFC_LOAD_VEC_WAIT_TICKS.fetch_add(1, Ordering::SeqCst);
        if waits.is_multiple_of(120) {
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
    // Force the dispatcher's build branch: clear tfc+0x18c (IsNotReleaseFlag55 0x14082cd60 `cmpb
    // $0,0x18c(rcx)`). The open-menu path sets this nonzero after press-any-button, which makes the
    // load dispatcher 0x1409b3070 take its abort branch (empty job, no load -- the builder 0x9ac760
    // never fired). Clearing it guarantees the real LoadGame build. bd dispatcher-abort-branch-force-
    // tfc-18c-zero-2026-06-23.
    let nrf_before = unsafe { safe_read_usize(tfc + TFC_NOT_RELEASE_FLAG_18C_OFFSET) }
        .map(|v| (v & 0xff) as u8)
        .unwrap_or(0xff);
    unsafe { *((tfc + TFC_NOT_RELEASE_FLAG_18C_OFFSET) as *mut u8) = TFC_NOT_RELEASE_FLAG_CLEAR };
    mark_tfc_forced_continue_handoff();
    // Let the recurring world-stream observer log through the loading screen.
    append_autoload_debug(format_args!(
        "fire-tfc-continue: SET *(tfc+0x{:x})=1 (was {before}) + mss+0x{:x}=slot {want_slot} (tfc=0x{tfc:x} dialog=0x{dialog:x} owner=0x{owner:x} mss={mss:?} gm_singleton=0x{gm_singleton:x}) -- now INVOKING selector 0x{:x} (NO input)",
        TFC_DISPATCH_STATE_14C_OFFSET,
        MSS_SAVE_SLOT_1200_OFFSET,
        er_game_base::mem::game_data_addr(
            base,
            TITLE_CONTINUE_SELECTOR_RVA,
            "TITLE_CONTINUE_SELECTOR_RVA"
        )
    ));
    // Invoke the Continue-item selector that consumes tfc+0x14c (it is not pumped from the idle menu).
    // Selector 0x1409a8eb0(rcx = &dialog_slot = owner+0xe0, rdx = out MenuJobResult*): reads
    // *(rcx)->dialog, *(dialog+0xa38)->tfc, *(tfc+0x14c)==1 -> load branch -> sets r8=dialog+0x50 +
    // calls the load dispatcher 0x1409b3070 (proper ChainMenuJobs enqueue). Wrapped in catch_unwind
    // (a Rust panic is caught; a hardware AV is not). Keeps simulated_button_presses_total = 0.
    let dialog_slot = owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET;
    let mut out_job: [usize; 4] = [0; 4];
    let out_ptr = out_job.as_mut_ptr() as usize;
    let selector: unsafe extern "system" fn(usize, usize) -> usize = unsafe {
        std::mem::transmute(
            match title_fn(TITLE_CONTINUE_SELECTOR_RVA, "TITLE_CONTINUE_SELECTOR_RVA") {
                Some(address) => address,
                None => return,
            },
        )
    };
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
        er_game_base::mem::game_data_addr(
            base,
            TITLE_CONTINUE_LOAD_DISPATCHER_RVA,
            "TITLE_CONTINUE_LOAD_DISPATCHER_RVA"
        )
    ));
    // Install the built job as currentTopMenuJob (CSPopupMenu+0xB0) via CS::MenuJob::Assign, so the
    // native per-frame menu pump runs its Run in context -- the fix for the self-pump menu-jumping (our
    // ExecuteMenuJob/drain-wrapper attempts ran the job out of context and never deserialized). The
    // selector/dispatcher only build + return the job (out_job[0]); the native flow normally installs
    // it into a pump-drained slot. We replicate that install. bd menu-job-install-mechanism-2026-06-23
    // + inject-job-into-native-pump-slots-recipe-2026-06-23. No input.
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
    // (removed the dialog+0x50 count-reset hack: a live TitleTopDialog's +0x98 count is provably
    // always 0..8 -- the garbage we saw means we read a non-LIVE/transient object, so zeroing it just
    // masks the real lifecycle problem and would corrupt a valid dialog's window list. The readiness
    // gate above (count<8) + the vtable/a40 gates are the correct fail-closed guard. bd
    // forge-breaks-lifecycle-native-confirm-is-correct-context-2026-06-23.)
    let _ = DIALOG_MENUWINDOW_VEC_50_OFFSET;
    let _ = DLFIXEDVECTOR_COUNT_48_OFFSET;
    // Target = owner+0x130, the title flow's active MenuJob slot that STEP_MenuJobWait runs
    // ExecuteMenuJob(&owner+0x130) on every frame (the title's own per-frame pump, definitely live at
    // the title menu -- unlike currentTopMenuJob+0xB0 which a run showed is EMPTY/unused by the title).
    // owner+0x130 is a MenuJob* slot (PushBackJob AV'd there because it is not a FixOrderJobSequence;
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
    let assign: unsafe extern "system" fn(usize, usize, usize) = unsafe {
        std::mem::transmute(
            match title_fn(MENU_JOB_ASSIGN3_RVA, "MENU_JOB_ASSIGN3_RVA") {
                Some(address) => address,
                None => return,
            },
        )
    };
    let scratch_ptr = (&raw mut scratch) as usize;
    let src_ptr = (&raw mut src) as usize;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        assign(dest, scratch_ptr, src_ptr)
    }));
    let new_top = unsafe { safe_read_usize(dest) }.unwrap_or(0);
    append_autoload_debug(format_args!(
        "fire-tfc-continue: *** INSTALLED job=0x{job:x} into owner+0x130 (STEP_MenuJobWait active slot) via Assign 0x{:x} (tfc+0x18c was {nrf_before}->0; owner=0x{owner:x} dest=0x{dest:x} old_top=0x{old_top:x} new_top=0x{new_top:x} panicked={}) -- STEP_MenuJobWait should pump it IN CONTEXT. Watch oracle: c30 real, player present, now_loading ***",
        er_game_base::mem::game_data_addr(base, MENU_JOB_ASSIGN3_RVA, "MENU_JOB_ASSIGN3_RVA"),
        r.is_err()
    ));
}
/// Install the `TitleTopDialog::update` hook once so the Continue build runs in the pump's live
/// frame. Gated by `fire_tfc_continue_enabled` at the call site.
///
/// # A bare detour, because argument 2 is a float
///
/// `update(this /*rcx*/, float delta /*xmm1*/, const u8* input /*r8*/)`, read out of the image:
/// `0x1409aac2a` spills `xmm6`, `0x1409aac40` does `movaps xmm6, xmm1` before anything writes
/// that register, and the value goes back out as argument 2 at `0x1409aae21` and `0x1409aae2c`.
/// It is single precision -- one callee stores it `movss [rbp-0x20], xmm6` (`0x1407457ec`) and
/// the other accumulates it `addss xmm6, [rdi+0x188]` (`0x14082d79f`). Argument 3 is a pointer,
/// dereferenced at `0x1409aac4e` (`movzx eax, byte ptr [r8]`) before any write. There is no
/// argument 4: its home slot holds a spilled `rbx`.
///
/// This used to go through `create_continue_trace_hook`, which transmuted the detour to
/// `er_hook::UnionFn` -- four `usize`s -- and handed it to the hook union. The union dispatcher
/// forwards integer registers only, so it neither receives `xmm1` nor passes it on, and the
/// detour's declared `f32` would have been read from whatever the dispatcher body happened to
/// leave there. The float made the union unusable, not optional, so this installs its own
/// [`MhHook`] with the true signature, the same call this repo already makes for
/// `er-npc-possess`'s `CSFeManImp::UpdatePlayerComponents` and `er-loading-portrait-core`'s
/// `loading_screen_update_hook`. `scripts/check-union-hook-abi.py` is the gate that keeps a float
/// handler off the union from here on.
pub unsafe fn install_title_update_hook(base: usize) {
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
    // Unresolved on purpose: `MhHook::new` owns the single 1.16.2 -> 1.17 resolve, which is why
    // this is `game_rva_for_hook` and not `game_data_addr`. Resolving here and again inside the
    // hook API is the double-resolve `scripts/check-double-resolved-hook-targets.py` refuses.
    let Ok(address) = er_game_base::mem::game_rva_for_hook(TITLE_TOP_DIALOG_UPDATE_RVA as u32)
    else {
        append_autoload_debug(format_args!(
            "title-update-hook: no game module base; TitleTopDialog::update is not hooked"
        ));
        return;
    };
    let hook = match unsafe {
        MhHook::new(
            address as *mut c_void,
            title_update_detour as *mut c_void,
        )
    } {
        Ok(hook) => hook,
        Err(status) => {
            append_autoload_debug(format_args!(
                "title-update-hook: MhHook::new on TitleTopDialog::update failed: {status:?}"
            ));
            return;
        }
    };
    TITLE_UPDATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
    if let Err(status) = unsafe { hook.queue_enable() } {
        TITLE_UPDATE_ORIG.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "title-update-hook: queue_enable failed: {status:?}"
        ));
        return;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "title-update-hook: INSTALLED on TitleTopDialog::update 0x{:x} -- in-context Continue build armed; bare detour, not the union, because argument 2 is a float in xmm1",
            er_game_base::mem::game_data_addr(
                base,
                TITLE_TOP_DIALOG_UPDATE_RVA,
                "TITLE_TOP_DIALOG_UPDATE_RVA"
            )
        )),
        status => {
            TITLE_UPDATE_ORIG.store(0, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "title-update-hook: MH_ApplyQueued failed: {status:?}"
            ));
        }
    }
    // The handle is deliberately dropped here without ceremony: `MhHook` is three raw pointers
    // with no `Drop`, and MinHook owns the installed detour keyed by target address -- so letting
    // the handle go does not uninstall the hook. The `std::mem::forget` that used to sit here was
    // a no-op that said otherwise, which is what `clippy::forget_non_drop` flags. An explicit
    // `drop(hook)` would be the same no-op under a different lint (`clippy::drop_non_drop`), so
    // the binding simply ends with the function.
}
/// Gated, fail-closed, one-shot readiness advance past press-any-button. Reads the built job at
/// `[step+0x130]`; once it is a valid in-image job (we are at press-any-button) and has settled, sets
/// `[job+0x1e8]=2` so the job's own predicate (0x1407a9200) completes it through the native path. Logs
/// the job struct on first sighting so the run self-confirms the offsets. Zero input.
pub unsafe fn pab_advance_try(step: usize) {
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
        return; // unreadable/garbage press-count -> do not write or latch; keep waiting
    }
    if count >= PAB_PRESS_COUNT_SATISFIED {
        // Already satisfied (a real press or prior advance) -> latch, nothing to do.
        PAB_ADVANCE_FIRED.store(1, Ordering::SeqCst);
        return;
    }
    // Readiness advance (zero-input): satisfy the job's own completion predicate.
    unsafe {
        *((job + PAB_JOB_PRESS_COUNT_1E8_OFFSET) as *mut u32) = PAB_PRESS_COUNT_SATISFIED;
    }
    PAB_ADVANCE_FIRED.store(1, Ordering::SeqCst);
    let after = unsafe { safe_read_i32(job + PAB_JOB_PRESS_COUNT_1E8_OFFSET) }.unwrap_or(-1) as u32;
    append_autoload_debug(format_args!(
        "pab-advance: *** SET [job+0x1e8]={PAB_PRESS_COUNT_SATISFIED} (was {count}, now {after}) job=0x{job:x} keycode=0x{keycode:x} settle={settle} -- readiness-gated press-any-button advance, ZERO input ***"
    ));
}
/// Install the press-any-button node-update hook once (minhook, mirroring `install_title_update_hook`).
/// Gated by `pab_advance_enabled` at the call site; the detour self-gates too (pass-through until armed).
pub unsafe fn install_pab_advance_hook(base: usize) {
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
            er_game_base::mem::game_data_addr(
                base,
                PAB_NODE_UPDATE_RVA as usize,
                "PAB_NODE_UPDATE_RVA"
            )
        )),
        status => append_autoload_debug(format_args!(
            "pab-advance-hook: MH_ApplyQueued failed: {status:?}"
        )),
    }
    std::mem::forget(hooks);
}
