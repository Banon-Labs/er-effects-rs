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

use crate::input_blocker::{InputBlocker, InputFlags};
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
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
            ClipCursor, EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
            WM_KEYDOWN, WM_KEYUP,
        },
    },
    core::{BOOL, PCSTR},
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, ffi::*, hooks::*, telemetry::*};

use super::*;

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
    // Named GameMan fields bound to the upstream typed layout (self-validating, dedups the
    // crate-level consts). The b73/b74/b75/bb8/bbc/bc0/bc4 flags read upstream-unnamed regions,
    // so they stay hand-decoded.
    const GAME_MAN_SAVE_SLOT_OFFSET: usize = core::mem::offset_of!(GameMan, save_slot);
    const GAME_MAN_REQUESTED_SAVE_SLOT_LOAD_INDEX_OFFSET: usize =
        core::mem::offset_of!(GameMan, requested_save_slot_load_index);
    const GAME_MAN_SAVE_STATE_OFFSET: usize = core::mem::offset_of!(GameMan, save_state);
    const GAME_MAN_FLAG_B72_OFFSET: usize = core::mem::offset_of!(GameMan, save_requested);
    const GAME_MAN_FLAG_B73_OFFSET: usize = GAME_MAN_FLAG_B73_PROBE_OFFSET;
    const GAME_MAN_FLAG_B74_OFFSET: usize = GAME_MAN_FLAG_B73_OFFSET + core::mem::size_of::<u8>();
    const GAME_MAN_FLAG_B75_OFFSET: usize = GAME_MAN_FLAG_B75_PROBE_OFFSET;
    const GAME_MAN_FLAG_BC4_OFFSET: usize = crate::GAME_MAN_FLAG_BC4_OFFSET;
    const GAME_MAN_FLAG_BB8_OFFSET: usize = GAME_MAN_FLAG_BC4_OFFSET
        - core::mem::size_of::<u32>()
        - core::mem::size_of::<u32>()
        - core::mem::size_of::<u32>();
    const GAME_MAN_FLAG_BBC_OFFSET: usize = GAME_MAN_FLAG_BB8_OFFSET + core::mem::size_of::<u32>();
    const GAME_MAN_FLAG_BC0_OFFSET: usize = GAME_MAN_FLAG_BBC_OFFSET + core::mem::size_of::<u32>();

    unsafe {
        let game_man = game_man_ptr_or_null() as *const u8;
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
    const MENU_CONTINUE_WRAPPER_RVA: u32 = TRACE_MENU_CONTINUE_WRAPPER_RVA;
    const MENU_NEW_OR_LOAD_WRAPPER_RVA: u32 = TRACE_MENU_NEW_OR_LOAD_WRAPPER_RVA;
    const MENU_OTHER_LOAD_WRAPPER_RVA: u32 = er_save_loader::MENU_OTHER_LOAD_WRAPPER_RVA;
    const SET_SAVE_SLOT_RVA: u32 = er_save_loader::SET_SAVE_SLOT_RVA;
    const SAVE_REQUEST_PROFILE_RVA: u32 = er_save_loader::SAVE_REQUEST_PROFILE_RVA;
    const REQUEST_SAVE_RVA: u32 = er_save_loader::REQUEST_SAVE_RVA;
    const CURRENT_SLOT_LOAD_RVA: u32 = 0x0067b570;
    const CONTINUE_LOAD_RVA: u32 = 0x0067b750;
    const COMBINED_LOAD_RVA: u32 = 0x0067b940;
    const MAP_LOAD_RVA: u32 = 0x0067bc10;
    const SAVE_LOAD_STATE_INIT_RVA: u32 = er_save_loader::SAVE_LOAD_STATE_INIT_RVA;

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
            "native_submit_7ac890",
            MENU_ITEM_SUBMIT_RVA as u32,
            native_submit_hook as *mut c_void,
            &NATIVE_SUBMIT_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "result_event_handler_746e80",
            RESULT_EVENT_HANDLER_RVA,
            result_event_handler_hook as *mut c_void,
            &RESULT_EVENT_HANDLER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "result_action_builder_746a00",
            RESULT_ACTION_BUILDER_RVA,
            result_action_builder_hook as *mut c_void,
            &RESULT_ACTION_BUILDER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "result_event_wrapper_builder_744a60",
            RESULT_EVENT_WRAPPER_BUILDER_RVA,
            result_event_wrapper_builder_hook as *mut c_void,
            &RESULT_EVENT_WRAPPER_BUILDER_ORIG,
        );
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
            "b80_dispatcher2_afb880_observe",
            B80_DISPATCHER2_RVA as u32,
            b80_dispatcher2_observe_hook as *mut c_void,
            &B80_DISPATCHER2_OBSERVE_ORIG,
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
        const CAP_LOAD_ACTIVATE_RVA: u32 = 0x009a4670;
        const CAP_LOAD_ACTIVATE2_RVA: u32 = 0x009ac760;
        const CAP_BUILDER_RVA: u32 = 0x00826510;
        const CAP_SELECTOR_TICK_RVA: u32 = PROFILE_LOAD_SELECTOR_TICK_RVA as u32;
        const CAP_MENU_DESER_RVA: u32 = ProfileLoadMenuRva::MenuDeser as u32;
        const CAP_DIALOG_FACTORY_RVA: u32 = LIVE_DIALOG_FACTORY_RVA as u32;
        create_continue_trace_hook(
            &mut hooks,
            "cap_setstate_b0d960",
            CAP_SETSTATE_RVA,
            cap_setstate_hook as *mut c_void,
            &CAP_SETSTATE_ORIG,
        );
        // NOTE: the continue_confirm 0x140b0e180 hook is NOT installed here. It is installed
        // UNCONDITIONALLY at process attach via install_system_quit_continue_confirm_hook
        // (mirroring the c30_writer precedent): the System->Quit switch needs it in every product
        // run, and installing a second MhHook on the same address would fail. That hook reproduces
        // this trace set's "CAP continue_confirm" line + OWN_STEPPER_CONFIRMED latch when tracing
        // is enabled, so trace runs see identical output.
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
        // MenuWindowJob ctor 0x1407ac8c0: latch semantic Continue items at construction before
        // the first updated/idle title input leaf can poison MENU_CONTINUE_ITEM.
        create_continue_trace_hook(
            &mut hooks,
            "cap_menu_window_job_ctor_7ac8c0",
            MENU_WINDOW_JOB_CTOR_RVA,
            menu_window_job_ctor_hook as *mut c_void,
            &MENU_WINDOW_JOB_CTOR_ORIG,
        );
        // MenuWindowJob native-accept ctor variant 0x1407acb00: observe/latch semantic Continue
        // rows built by the sibling constructor that also installs native accept 0x1407ad810.
        create_continue_trace_hook(
            &mut hooks,
            "cap_menu_window_job_native_ctor_b_7acb00",
            MENU_WINDOW_JOB_NATIVE_CTOR_B_RVA,
            menu_window_job_native_ctor_b_hook as *mut c_void,
            &MENU_WINDOW_JOB_NATIVE_CTOR_B_ORIG,
        );
        // MenuWindowJob idle ctor 0x1407acf80: static RE shows this neighboring constructor
        // installs the constant-false accept predicate 0x1407add70. Observe it separately so a
        // Continue-looking row with idle accept can be attributed to the disabled native path.
        create_continue_trace_hook(
            &mut hooks,
            "cap_menu_window_job_idle_ctor_7acf80",
            MENU_WINDOW_JOB_IDLE_CTOR_RVA,
            menu_window_job_idle_ctor_hook as *mut c_void,
            &MENU_WINDOW_JOB_IDLE_CTOR_ORIG,
        );
        // Title native-ready predicate 0x140733150: the native title builder calls this on
        // title_dialog+0x2610 before constructing native-accept rows. Observe the exact result
        // and state flags so product-core can wait for the native condition instead of promoting
        // idle rows.
        create_continue_trace_hook(
            &mut hooks,
            "cap_title_native_ready_733150",
            TITLE_NATIVE_READY_PREDICATE_RVA,
            title_native_ready_predicate_hook as *mut c_void,
            &TITLE_NATIVE_READY_PREDICATE_ORIG,
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

pub(crate) unsafe fn call_result_void1_original(
    original: &AtomicUsize,
    result: usize,
) -> Option<()> {
    let original = original.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let original: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(original) };
    unsafe { original(result) };
    Some(())
}

pub(crate) unsafe fn call_result_void2_original(
    original: &AtomicUsize,
    result: usize,
    event: usize,
) -> Option<()> {
    let original = original.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let original: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(original) };
    unsafe { original(result, event) };
    Some(())
}

pub(crate) unsafe fn call_wrapper_builder_original(
    rcx: usize,
    rdx: usize,
    r8: usize,
) -> Option<usize> {
    let original = RESULT_EVENT_WRAPPER_BUILDER_ORIG.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let original: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(original) };
    Some(unsafe { original(rcx, rdx, r8) })
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
    let gm = game_man_ptr_or_null();
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

pub(crate) unsafe extern "system" fn b80_dispatcher2_observe_hook(this: usize) -> u8 {
    if this != TITLE_OWNER_SCAN_START_ADDRESS {
        B80_NATIVE_DISPATCHER_OWNER.store(this, Ordering::SeqCst);
    }
    let count = B80_DISPATCHER2_OBSERVE_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    let before = b80_mount_trace_summary();
    let ret = unsafe {
        let orig = B80_DISPATCHER2_OBSERVE_ORIG.load(Ordering::SeqCst);
        if orig == HOOK_ORIGINAL_UNSET {
            TITLE_OWNER_SCAN_START_ADDRESS as u8
        } else {
            let f: unsafe extern "system" fn(usize) -> u8 = std::mem::transmute(orig);
            f(this)
        }
    };
    if count < MENU_ITEM_UPDATE_LOG_MAX
        || before.contains("b80=1")
        || before.contains("b80=2")
        || before.contains("b80=3")
    {
        append_continue_trace(format_args!(
            "b80_dispatcher2_afb880 OBS this=0x{this:x} ret={ret} before{{{before}}} after{{{}}} {}",
            b80_mount_trace_summary(),
            trace_callers_summary()
        ));
    }
    ret
}

pub(crate) unsafe extern "system" fn b80_deserialize_hook(slot: i32) -> i32 {
    append_continue_trace(format_args!(
        "b80_deserialize_67b290 ENTER slot={slot} {}",
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_b80_initiator_original(&B80_DESERIALIZE_ORIG, slot) };
    const B80_DESERIALIZE_SUCCESS_RET: i32 = 1;
    const C30_ZERO: i32 = 0;
    let gm = game_man_ptr_or_null();
    if ret == B80_DESERIALIZE_SUCCESS_RET && gm != TITLE_OWNER_SCAN_START_ADDRESS {
        let c30 = unsafe { *((gm + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) };
        let ac0 = unsafe { *((gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
        if c30 != GAME_MAN_C30_UNSET && c30 != C30_ZERO {
            OWN_STEPPER_MOUNT_C30.store(c30, Ordering::SeqCst);
            OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_OK, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "b80_deserialize_67b290: latched native post-click deserialize success slot={slot} ac0={ac0} c30=0x{c30:x}"
            ));
        }
    }
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
    const C30_WRITER_FULL_SAVE_SIZE: u32 = 0x280000;
    const C30_WRITER_SUCCESS_RET: usize = 1;
    const C30_AFTER_ZERO: i32 = 0;
    let c30_after = unsafe { *((game_man + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) };
    if ret == C30_WRITER_SUCCESS_RET
        && size == C30_WRITER_FULL_SAVE_SIZE
        && c30_after != C30_AFTER_ZERO
    {
        OWN_STEPPER_MOUNT_C30.store(c30_after, Ordering::SeqCst);
        OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_OK, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "c30_writer_67bd70: latched full-save native deser success c30=0x{c30_after:x} size=0x{size:x}"
        ));
    }
    if do_log {
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
