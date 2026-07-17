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
    _hooks: &mut Vec<MhHook>,
    name: &str,
    rva: u32,
    hook_impl: *mut c_void,
    original: &'static AtomicUsize,
) {
    let Ok(addr) = game_rva(rva) else {
        append_continue_trace(format_args!("hook {name}: failed to resolve rva=0x{rva:x}"));
        return;
    };
    // UNION (2026-07-16): these diagnostic trace observers hook the SAME menu functions as product
    // hooks (e.g. cap_load_activate on 0x9a4670). Register through the union so the trace CHAINS with
    // the product handler instead of racing it for the single MinHook slot -- the trace no longer
    // silently steals (or loses) the address depending on install order.
    let handler_fn: crate::mh::UnionFn =
        unsafe { std::mem::transmute::<*mut c_void, crate::mh::UnionFn>(hook_impl) };
    match unsafe { crate::mh::register_union_hook(addr, handler_fn, original) } {
        Ok(()) => append_continue_trace(format_args!("hook {name}: unioned on 0x{addr:x}")),
        Err(status) => append_continue_trace(format_args!(
            "hook {name}: union register failed at 0x{addr:x}: {status:?}"
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
        // MoveMapStep child EDGE hooks (3rd-load root, 2026-07-16). InGameStep step 6
        // STEP_MoveMap_Init CREATES the MoveMapStep child; step 8 STEP_MoveMap_Finish fires when its
        // load COMPLETES. On the softlock Init fires but Finish never does -- that absence IS the
        // semaphore. These fire once per world load (edge, not per-frame) so they add no timing
        // perturbation to the Windows-native race (unlike detouring the hot Execute pump 0x140b0bd60,
        // which froze the title machine, submit.rs run 305). RVAs ground-truthed dump->deobf via the
        // shift tool (dump 0x140aec210/0x140aec140 -> deobf, shift -0xf0, content-unique).
        create_continue_trace_hook(
            &mut hooks,
            "mms_step_init_aec120",
            MMS_STEP_INIT_RVA,
            mms_step_init_hook as *mut c_void,
            &MMS_STEP_INIT_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "mms_step_finish_aec050",
            MMS_STEP_FINISH_RVA,
            mms_step_finish_hook as *mut c_void,
            &MMS_STEP_FINISH_ORIG,
        );
        // The CHILD's own STEP_Cleanup (MoveMapStep step 18->19 exit, dump 0x140af5840 -> deobf, shift
        // -0xf0 content-unique). Fires the instant a MoveMapStep child leaves the STEP_MoveMap resident
        // step toward Finish. Logs the GameMan load-in signals at that exact moment so a SUCCESSFUL load
        // reveals which input drives the advance -- on the re-load lock the incoming child never reaches
        // this hook (parks at 18), so its ABSENCE (with a matching MMS-INIT ptr) is itself the signal.
        create_continue_trace_hook(
            &mut hooks,
            "mms_child_cleanup_af5750",
            MMS_CHILD_CLEANUP_RVA,
            mms_child_cleanup_hook as *mut c_void,
            &MMS_CHILD_CLEANUP_ORIG,
        );
        // WORLD-RES POPULATE source-builder (deobf 0x66bb10): the ONE function that (re)creates the
        // +0xce0 per-block res the WorldResWait stall waits on. It early-outs when its input MSB-list
        // count (arg2+0x10) is 0. Logging that count per load is the decisive divergence semaphore --
        // full on the fresh boot (load 1), 0 for the dest on the in-game reload (load 2). Read-only.
        create_continue_trace_hook(
            &mut hooks,
            "populate_blocks_lists_66bb10",
            POPULATE_BLOCKS_LISTS_RVA,
            populate_blocks_lists_hook as *mut c_void,
            &POPULATE_BLOCKS_LISTS_ORIG,
        );
        // Load-state ENTRY ctor (0x6610e0): decisive load1-vs-load2 probe for whether the destination
        // area-0x1c load-state entry is re-created on the reload (absence on load 2 == the resident-reuse
        // root). Read-only, 2 register args (rcx=entry, rdx=descNode) so 4-arg forwarding is safe.
        // NOTE: we deliberately do NOT hook the world BLOCK ctor 0x62ec00 -- it takes its count/base as
        // STACK args (0x68/0x70(%rsp)); a 4-register forwarding hook loses them and corrupts every block's
        // load-state slice -> AV (runtime-proven 2026-07-17, crash in the 0x61-0x62 worldres region).
        create_continue_trace_hook(
            &mut hooks,
            "worldres_entry_ctor_6610e0",
            WORLDRES_ENTRY_CTOR_RVA,
            worldres_entry_ctor_hook as *mut c_void,
            &WORLDRES_ENTRY_CTOR_ORIG,
        );
        // The REAL block-res getter (WITH the search key) -- the determining measurement the keyless
        // oracle blk_ls could not give. Change-detected so the hot path is not flooded.
        create_continue_trace_hook(
            &mut hooks,
            "worldres_blockres_getter_62f470",
            WORLDRES_BLOCKRES_GETTER_RVA,
            worldres_blockres_getter_hook as *mut c_void,
            &WORLDRES_BLOCKRES_GETTER_ORIG,
        );
        // FIX: WorldBlockRes phase-2 handler -- force a bounded teardown/reload retry when the block's
        // file cap is loaded but its data +0x90 is null (the determined reload stall). Inert unless armed.
        create_continue_trace_hook(
            &mut hooks,
            "worldres_blockres_phase2_6157f0",
            WORLDRES_BLOCKRES_PHASE2_RVA,
            blockres_phase2_hook as *mut c_void,
            &BLOCKRES_PHASE2_ORIG,
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

/// MoveMapStep child STEP_Cleanup deobf RVA (dump 0x140af5840, shift -0xf0 content-unique). Fires when a
/// child leaves the resident STEP_MoveMap(18) toward Finish -- the load-in completion (or teardown) edge.
const MMS_CHILD_CLEANUP_RVA: u32 = 0xaf5750;
pub(crate) static MMS_CHILD_CLEANUP_ORIG: AtomicUsize = AtomicUsize::new(0);

/// Logs the GameMan load-in signals at the moment a MoveMapStep child advances out of STEP_MoveMap. On a
/// SUCCESSFUL switch-load this names the input that drives the incoming child to Finish; on the re-load
/// lock the incoming child (matching the MMS-INIT ptr) never reaches this hook. `this` = the MoveMapStep.
pub(crate) unsafe extern "system" fn mms_child_cleanup_hook(
    this: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    if BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.load(Ordering::SeqCst) != 0 {
        let gm = game_man_ptr_or_null();
        let rd = |off: usize| {
            if gm != 0 {
                unsafe { safe_read_u8(gm + off) }.map(|v| v as i32).unwrap_or(-1)
            } else {
                -1
            }
        };
        // Also read the return-title byte (menuData+0x5d) + force latch (0x3d856a0) at the advance edge --
        // warp/b7c/b7d proved 0 even on a SUCCESSFUL advance, so the driver is one of these (or session).
        let menudata = game_rva(CS_MENU_MAN_GLOBAL_RVA as u32)
            .ok()
            .and_then(|p| unsafe { safe_read_usize(p) })
            .filter(|&m| m > 0x10000)
            .and_then(|m| unsafe { safe_read_usize(m + CS_MENU_MAN_MENU_DATA_OFFSET) })
            .filter(|&d| d > 0x10000);
        let rt5d = menudata
            .and_then(|d| unsafe { safe_read_u8(d + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET) })
            .map(|v| v as i32)
            .unwrap_or(-1);
        let force = game_rva(ENDING_REQUEST_FORCE_FLAG_3D856A0_RVA as u32)
            .ok()
            .and_then(|p| unsafe { safe_read_u8(p) })
            .map(|v| v as i32)
            .unwrap_or(-1);
        append_autoload_debug(format_args!(
            "MMS-CLEANUP: child(mms)=0x{this:x} leaving STEP_MoveMap -> Cleanup; warp={} b7c={} b7d={} rt5d={rt5d} force={force} -- what drove the advance (compare to the lock where the incoming child never reaches here)",
            rd(GAME_MAN_WARP_REQUESTED_10_OFFSET),
            rd(GAME_MAN_ENDING_FLAG_B7C_OFFSET),
            rd(GAME_MAN_ENDING_FLAG_B7D_OFFSET)
        ));
    }
    unsafe { mms_call_original(&MMS_CHILD_CLEANUP_ORIG, this, b, c, d) }
}

/// STEP_MoveMap_Init deobf RVA (dump 0x140aec210, shift -0xf0 content-unique). Creates the child.
const MMS_STEP_INIT_RVA: u32 = 0xaec120;
/// STEP_MoveMap_Finish deobf RVA (dump 0x140aec140, shift -0xf0 content-unique). Load complete.
const MMS_STEP_FINISH_RVA: u32 = 0xaec050;
pub(crate) static MMS_STEP_INIT_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static MMS_STEP_FINISH_ORIG: AtomicUsize = AtomicUsize::new(0);

/// Pass-through: call the chained original (union trampoline) with the received ABI. The step
/// executors are `fn(InGameStep*, FD4TaskData*)`; the union passes 4 regs and the callee ignores
/// the extra two, so forwarding all four is ABI-safe. Returns the original's value (void executors
/// leave rax undefined; the pump ignores it).
unsafe fn mms_call_original(orig: &AtomicUsize, a: usize, b: usize, c: usize, d: usize) -> usize {
    let original = orig.load(Ordering::SeqCst);
    if original == 0 {
        return 0;
    }
    let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(original) };
    unsafe { f(a, b, c, d) }
}

/// STEP_MoveMap_Init (InGameStep step 6): the MoveMapStep child is (re)created + RegisterStepTask'd
/// here. Edge semaphore: increments per world load. Logs only while an own-menu switch is active
/// (BOOT_VIEW_OWN_MENU_LOAD_ACTIVE) so normal-play map moves don't spam.
pub(crate) unsafe extern "system" fn mms_step_init_hook(
    this: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let ret = unsafe { mms_call_original(&MMS_STEP_INIT_ORIG, this, b, c, d) };
    let n = SWITCH_ORACLE_MMS_INIT_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.load(Ordering::SeqCst) != 0 {
        let mms = unsafe { safe_read_usize(this + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) }.unwrap_or(0);
        append_autoload_debug(format_args!(
            "MMS-INIT #{n}: InGameStep=0x{this:x} child(mms)=0x{mms:x} -- MoveMapStep child created+registered (step 6)"
        ));
    }
    // STEP-3 WORLD-RES REBUILD (init-point fix): on a SUBSEQUENT load (the autoload->reload of the
    // same save), the per-block world-res load-state for the destination block is never created, so
    // STEP_WorldResWait (child step 3) stalls with blk_ls=0. The reactive rebuild at the stall AVs
    // (ResetAreaResLists mid-stream). This runs the game's own ProcessMsbLoadLists HERE -- right after
    // STEP_MoveMap_Init created the child, BEFORE the world streams -- exactly where _Common_Initialize
    // legitimately calls it, so ResetAreaResLists is safe. Instruments unconditionally on a reload;
    // fires the corrective call only under the diagnostic gate until proven.
    unsafe { step3_init_worldres_rebuild(this) };
    ret
}

/// One-shot latch (per DLL load) + count for the init-point world-res rebuild (runtime semaphore).
static STEP3_INIT_REBUILD_FIRED: AtomicUsize = AtomicUsize::new(0);
static STEP3_INIT_REBUILD_COUNT: AtomicUsize = AtomicUsize::new(0);

pub(crate) static POPULATE_BLOCKS_LISTS_ORIG: AtomicUsize = AtomicUsize::new(0);

/// DECISIVE DIVERGENCE PROBE: log the input MSB-list block count `*(rdx+0x10)` every time PopulateLists'
/// source-builder runs, tagged with IN_WORLD (load 1 = false, subsequent reloads = true). Hypothesis: the
/// fresh boot passes a non-zero count (rebuilds all block-res incl the dest); the in-game reload passes 0
/// (the source list is empty for the dest -> +0xce0 never rebuilt -> WORLD RES WAIT stall). Read-only,
/// forwards to the original. `this` (rcx) = builder receiver, `list` (rdx) = the input MSB block list.
pub(crate) unsafe extern "system" fn populate_blocks_lists_hook(
    this: usize,
    list: usize,
    c: usize,
    d: usize,
) -> usize {
    let in_world = IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES;
    let count = if list > 0x10000 {
        unsafe { safe_read_i32(list + POPULATE_BLOCKS_LIST_INPUT_COUNT_10_OFFSET) }.unwrap_or(-1)
    } else {
        -2
    };
    let n = POPULATE_BLOCKS_LISTS_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    // Log every call while an own-menu load is active, plus the first several always, so both the
    // fresh-boot populate (load 1) and the reload populate (load 2) are captured for comparison.
    if BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.load(Ordering::SeqCst) != 0 || n <= 40 {
        append_autoload_debug(format_args!(
            "POPULATE-BLOCKS #{n}: input_block_count={count} in_world={in_world} this=0x{this:x} list=0x{list:x} -- (count==0 => builds NO +0xce0 block-res; the WORLD RES WAIT root)"
        ));
    }
    unsafe { mms_call_original(&POPULATE_BLOCKS_LISTS_ORIG, this, list, c, d) }
}
static POPULATE_BLOCKS_LISTS_CALLS: AtomicUsize = AtomicUsize::new(0);

pub(crate) static WORLDRES_ENTRY_CTOR_ORIG: AtomicUsize = AtomicUsize::new(0);
static WORLDRES_ENTRY_CTOR_1C_HITS: AtomicUsize = AtomicUsize::new(0);

/// DECISIVE: the load-state ENTRY constructor. `entry`=rcx, `desc`=rdx (descriptor node whose first
/// dword is the BlockId key written to entry+0x8). Logs when an entry is created for an area-0x1c block,
/// tagged with IN_WORLD -- so load 1 (in_world=false) vs load 2 (in_world=true) shows whether the
/// 0x1c000000 load-state entry is (re)created on the reload. If it fires on load 1 but NOT load 2, the
/// reconcile skips creating the destination entry on the resident-block reload == the stall's root.
pub(crate) unsafe extern "system" fn worldres_entry_ctor_hook(
    entry: usize,
    desc: usize,
    c: usize,
    d: usize,
) -> usize {
    let block_id = if desc > 0x10000 {
        unsafe { safe_read_i32(desc) }.unwrap_or(-1) as u32
    } else {
        0
    };
    if (block_id >> 24) == 0x1c {
        let in_world = IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES;
        let n = WORLDRES_ENTRY_CTOR_1C_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        append_autoload_debug(format_args!(
            "WORLDRES-ENTRY-CTOR #{n}: block_id=0x{block_id:x} entry=0x{entry:x} in_world={in_world} -- load-state entry CREATED for area 0x1c (absent on load 2 == the stall root)"
        ));
    }
    unsafe { mms_call_original(&WORLDRES_ENTRY_CTOR_ORIG, entry, desc, c, d) }
}

pub(crate) static WORLDRES_BLOCKRES_GETTER_ORIG: AtomicUsize = AtomicUsize::new(0);
static WORLDRES_GETTER_LAST_1C: AtomicUsize = AtomicUsize::new(usize::MAX);

pub(crate) static BLOCKRES_PHASE2_ORIG: AtomicUsize = AtomicUsize::new(0);
static BLOCKRES_STALECAP_RETRIES: AtomicUsize = AtomicUsize::new(0);
const BLOCKRES_STALECAP_MAX_RETRIES: usize = 6;

// ENV-GATE RATIONALE: ER_EFFECTS_BLOCKRES_STALECAP_FIX is an explicit diagnostic gate for the
// stale-file-cap reload fix while it is runtime-validated; default off until proven, then it becomes
// the ungated product fix. The hook is inert (pure pass-through) unless this is armed.
fn blockres_stalecap_fix_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_BLOCKRES_STALECAP_FIX").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-blockres-stalecap-fix.txt")
        .exists()
}

/// FIX (determined-root): the WorldBlockRes phase-2 handler parks at phase 2 on the reload when the
/// block's primary file cap reports loaded (status 0x04) but its data ptr +0x90 is null (file resident
/// from load 1, re-load short-circuits without re-attaching data). Detect that EXACT condition after the
/// original handler runs and, only on a SUBSEQUENT load (IN_WORLD_REACHED==YES, so the first autoload is
/// never touched), force the block's phase +0x35 to 5 (the game's own teardown/reload retry) so it
/// releases the stale cap and re-loads fresh. Bounded retries so a genuinely un-evictable file cannot
/// spin forever. `bres`=rcx (block-res).
pub(crate) unsafe extern "system" fn blockres_phase2_hook(
    bres: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let ret = unsafe { mms_call_original(&BLOCKRES_PHASE2_ORIG, bres, b, c, d) };
    if !blockres_stalecap_fix_enabled()
        || IN_WORLD_REACHED.load(Ordering::SeqCst) != IN_WORLD_REACHED_YES
        || bres <= 0x10000
    {
        return ret;
    }
    let phase = unsafe { safe_read_u8(bres + BLOCKRES_PHASE_35_OFFSET) }.unwrap_or(0xff);
    let gate = unsafe { safe_read_u8(bres + BLOCKRES_GATE_2F_OFFSET) }.unwrap_or(0);
    if phase != 2 || gate == 0 {
        return ret;
    }
    let fc = unsafe { safe_read_usize(bres + BLOCKRES_PRIMARY_FILECAP_40_OFFSET) }.unwrap_or(0);
    if fc <= 0x10000 {
        return ret;
    }
    let status = unsafe { safe_read_u8(fc + FILECAP_STATUS_88_OFFSET) }
        .map(|v| v as i32)
        .unwrap_or(-1);
    let data = unsafe { safe_read_usize(fc + FILECAP_DATA_90_OFFSET) }.unwrap_or(0);
    // The determined stall: cap reports LOADED (0x88==0x04) but its data (+0x90) is null -- the
    // teardown between loads freed the data yet left the "loaded" status, so the phase-2 machine
    // (deobf 0x1406157f0) short-circuits (it advances to phase 3 only when status==4 AND data!=0, and
    // its own phase=5 retry does NOT re-read because the cap still says loaded -- run4 proved forcing
    // phase=5 leaves data null). So clear the stale "loaded" status on BOTH file caps (+0x40 primary,
    // +0x48 secondary) and restart the block load (phase +0x35 = 0) so the block re-requests the caps
    // and they re-issue a fresh FD4 read that re-attaches data. Bounded so an un-evictable file cannot
    // spin. RUN-VALIDATED root: step3-run4-filecap-status4-data-null-resident-shortcircuit-2026-07-17.
    if status == FILECAP_STATUS_LOADED && data == 0 {
        let n = BLOCKRES_STALECAP_RETRIES.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= BLOCKRES_STALECAP_MAX_RETRIES {
            for coff in [BLOCKRES_PRIMARY_FILECAP_40_OFFSET, BLOCKRES_SECOND_FILECAP_48_OFFSET] {
                if let Some(cap) =
                    unsafe { safe_read_usize(bres + coff) }.filter(|&v| v > 0x10000)
                {
                    unsafe { *((cap + FILECAP_STATUS_88_OFFSET) as *mut u8) = 0 };
                }
            }
            unsafe { *((bres + BLOCKRES_PHASE_35_OFFSET) as *mut u8) = 0 };
            append_autoload_debug(format_args!(
                "BLOCKRES-STALECAP-FIX #{n}: block-res=0x{bres:x} cap=0x{fc:x} status=0x04 data=null -> cleared both cap +0x88 and reset block phase=0 to force a fresh FD4 re-read"
            ));
        } else if n == BLOCKRES_STALECAP_MAX_RETRIES + 1 {
            append_autoload_debug(format_args!(
                "BLOCKRES-STALECAP-FIX: retry cap ({BLOCKRES_STALECAP_MAX_RETRIES}) hit for block-res=0x{bres:x} cap=0x{fc:x}; cap-status reset did not re-attach data (needs teardown-eviction or direct cap re-load)"
            ));
        }
    }
    ret
}

/// DETERMINING MEASUREMENT: the REAL WorldResWait block-res getter, called WITH the search key (rdx),
/// unlike the SWITCH-ORACLE's keyless call. `area_res`=rcx (WorldAreaRes), `key_ptr`=rdx (int* BlockId).
/// For area-0x1c keys, log (on change) whether the getter FINDS the 0x1c000000 entry and, if so, the
/// found WorldBlockRes's +0x2d(ready)/+0x35(phase). This splits the stall's true cause deterministically:
///   found=0            -> the 0x1c000000 WorldBlockRes is NOT in this area's +0xce0 (key-miss / wrong area);
///   found=1, 2d==0/35!=0xa -> entry found but the block LOAD never completes (ready/phase never advance).
/// Comparing in_world=false (load 1, works) vs in_world=true (load 2, stall) isolates the determining
/// difference. Read-only, forwards to the original; change-detected so it does not flood the hot path.
pub(crate) unsafe extern "system" fn worldres_blockres_getter_hook(
    area_res: usize,
    key_ptr: usize,
    c: usize,
    d: usize,
) -> usize {
    let ret = unsafe { mms_call_original(&WORLDRES_BLOCKRES_GETTER_ORIG, area_res, key_ptr, c, d) };
    let key = if key_ptr > 0x10000 {
        unsafe { safe_read_i32(key_ptr) }.unwrap_or(-1) as u32
    } else {
        0
    };
    if (key >> 24) == 0x1c {
        let count = if area_res > 0x10000 {
            unsafe { safe_read_i32(area_res + 0xcd8) }.unwrap_or(-1)
        } else {
            -1
        };
        // Read the block-res load-state (getter return) + the exact phase-2->3 gate inputs from the
        // decompiled FUN_1406158d0: gate byte +0x2f; the block's two FD4FileCap slots at +0x40 (blockres[8])
        // and +0x48 (blockres[9]); for the primary cap +0x88 load-status (0x04=loaded) and +0x90 data ptr.
        // HYPOTHESIS: on the reload the primary cap is loaded (0x88==0x04) but its data +0x90 is NULL, so
        // the phase-2 handler cannot advance and parks at 2 (the determined stall cause).
        let (d2d, d35, g2f, fc8, fc8_88, fc8_90, fc9, fc9_88) = if ret > 0x10000 {
            let fc8 = unsafe { safe_read_usize(ret + 0x40) }.unwrap_or(0);
            let fc9 = unsafe { safe_read_usize(ret + 0x48) }.unwrap_or(0);
            (
                unsafe { safe_read_u8(ret + 0x2d) }.map(|v| v as i32).unwrap_or(-1),
                unsafe { safe_read_u8(ret + 0x35) }.map(|v| v as i32).unwrap_or(-1),
                unsafe { safe_read_u8(ret + 0x2f) }.map(|v| v as i32).unwrap_or(-1),
                fc8,
                if fc8 > 0x10000 {
                    unsafe { safe_read_u8(fc8 + 0x88) }.map(|v| v as i32).unwrap_or(-1)
                } else {
                    -1
                },
                if fc8 > 0x10000 {
                    unsafe { safe_read_usize(fc8 + 0x90) }.unwrap_or(0)
                } else {
                    0
                },
                fc9,
                if fc9 > 0x10000 {
                    unsafe { safe_read_u8(fc9 + 0x88) }.map(|v| v as i32).unwrap_or(-1)
                } else {
                    -1
                },
            )
        } else {
            (-1, -1, -1, 0, -1, 0, 0, -1)
        };
        let found = usize::from(ret != 0);
        let packed = found
            | ((d2d as u32 as usize & 0xff) << 1)
            | ((d35 as u32 as usize & 0xff) << 9)
            | ((g2f as u32 as usize & 0x3) << 17)
            | (usize::from(fc8_90 != 0) << 19)
            | ((fc8_88 as u32 as usize & 0xff) << 20);
        if WORLDRES_GETTER_LAST_1C.swap(packed, Ordering::Relaxed) != packed {
            let in_world = IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES;
            append_autoload_debug(format_args!(
                "WORLDRES-GETTER 0x1c: key=0x{key:x} count={count} found={found} ret=0x{ret:x} +0x2d(ready)={d2d} +0x35(phase)={d35} +0x2f(gate)={g2f} fc8=0x{fc8:x} fc8_88(status)={fc8_88} fc8_90(data)=0x{fc8_90:x} fc9=0x{fc9:x} fc9_88={fc9_88} in_world={in_world} -- phase-2 stalls if gate set + status 0x04 but data +0x90 null"
            ));
        }
    }
    ret
}

// ENV-GATE RATIONALE: ER_EFFECTS_STEP3_INIT_FIX is an explicit diagnostic gate for the init-point
// world-res rebuild while it is being runtime-validated; the INSTRUMENTATION logs unconditionally on a
// reload, only the corrective native call is gated. Default off until proven, then it becomes the
// ungated product fix.
fn step3_init_rebuild_call_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_STEP3_INIT_FIX").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-step3-init-fix.txt")
        .exists()
}

/// Read the loadlist virtual-path (DLString wchar, ASCII low byte) at `InGameStep+0x210/0x220` so the
/// log reveals which MAP the loadlist points at (the DEST m28 vs a STALE m60) -- the decisive datum for
/// whether `fcap` is correct at init time.
fn read_ingamestep_vpath(this: usize) -> (usize, usize, String) {
    let base = unsafe { safe_read_usize(this + INGAMESTEP_WORLDLOADLIST_VPATH_BASE_210_OFFSET) }
        .unwrap_or(0);
    let size = unsafe { safe_read_usize(this + INGAMESTEP_WORLDLOADLIST_VPATH_SIZE_220_OFFSET) }
        .unwrap_or(0);
    let mut s = String::new();
    if base > 0x10000 && size > 0 && size < 200 {
        for i in 0..size.min(72) {
            let byte = unsafe { safe_read_u8(base + i * 2) }.unwrap_or(0);
            if byte == 0 {
                break;
            }
            s.push(if (0x20..0x7f).contains(&byte) {
                byte as char
            } else {
                '?'
            });
        }
    }
    (base, size, s)
}

/// The init-point world-res rebuild. `this` = InGameStep (the STEP_MoveMap_Init executor's arg). Runs
/// only on a SUBSEQUENT load (IN_WORLD_REACHED==YES, so the first autoload's init is untouched).
/// Replicates `_Common_Initialize`'s call verbatim: ProcessMsbLoadLists(&worldInfoOwner @ this+0x250,
/// fcap @ *(this+0x238), dlc02 @ *(this+0x240)). Instruments first (flushed) so an AV or a stale-fcap
/// is diagnosable from the log.
unsafe fn step3_init_worldres_rebuild(this: usize) {
    if IN_WORLD_REACHED.load(Ordering::SeqCst) != IN_WORLD_REACHED_YES {
        return; // first autoload -- never touch it
    }
    if this < 0x10000 {
        return;
    }
    let embed_worldio = this + INGAMESTEP_WORLDINFO_OWNER_EMBED_250_OFFSET;
    let fcap = unsafe { safe_read_usize(this + INGAMESTEP_LOADLISTLIST_FILECAP_238_OFFSET) }
        .unwrap_or(0);
    let dlc02 =
        unsafe { safe_read_usize(this + INGAMESTEP_LOADLISTLIST_DLC02_240_OFFSET) }.unwrap_or(0);
    let (vbase, vsize, vpath) = read_ingamestep_vpath(this);
    // Cross-check: the WorldInfoOwner reached via the child chain (MoveMapStep->FieldArea->+0x10),
    // which the SWITCH-ORACLE uses -- log both so we can confirm the embedded +0x250 is the right arg.
    let mms = unsafe { safe_read_usize(this + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) }.unwrap_or(0);
    let fa = if mms > 0x10000 {
        unsafe { safe_read_usize(mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    let chain_wio = if fa > 0x10000 {
        unsafe { safe_read_usize(fa + WORLDRES_RESMGR_10_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    let cur_block = if fa > 0x10000 {
        unsafe { safe_read_i32(fa + FIELDAREA_CURRENT_BLOCK_ID_2C_OFFSET) }.unwrap_or(-1) as u32
    } else {
        u32::MAX
    };
    let call_enabled = step3_init_rebuild_call_enabled();
    append_autoload_debug(format_args!(
        "STEP3-INIT-REBUILD probe: InGameStep=0x{this:x} embed_worldio(+0x250)=0x{embed_worldio:x} chain_worldio=0x{chain_wio:x} fcap(+0x238)=0x{fcap:x} dlc02(+0x240)=0x{dlc02:x} vpath(+0x210)=0x{vbase:x} vsize={vsize} vpath='{vpath}' cur_block=0x{cur_block:x} area=0x{:x} call_enabled={call_enabled}",
        (cur_block >> 24) & 0xff
    ));
    if !call_enabled || fcap < 0x10000 {
        return;
    }
    if STEP3_INIT_REBUILD_FIRED.swap(1, Ordering::SeqCst) != 0 {
        return; // one-shot per DLL load (single reload per run during validation)
    }
    let Ok(addr) = game_rva(WORLDINFO_PROCESS_MSB_LOADLISTS_RVA) else {
        return;
    };
    let count = STEP3_INIT_REBUILD_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    // Pass dlc02 = 0 (NOT *(this+0x240)): the callee null-checks dlc02 and base-game areas have no DLC
    // loadlist; passing the raw field AV'd (2026-07-17). Address is now the corrected deobf 0x66b1d0.
    let _ = dlc02;
    append_autoload_debug(format_args!(
        "STEP3-INIT-REBUILD PRE-CALL #{count}: ProcessMsbLoadLists(0x{embed_worldio:x}, 0x{fcap:x}, dlc02=0) @ 0x{addr:x} -- init-time (pre-stream), replicating _Common_Initialize"
    ));
    let process_msb_loadlists: unsafe extern "system" fn(usize, usize, usize) =
        unsafe { core::mem::transmute(addr) };
    unsafe { process_msb_loadlists(embed_worldio, fcap, 0) };
    append_autoload_debug(format_args!(
        "STEP3-INIT-REBUILD POST-CALL #{count}: returned OK (no AV) -- world-res lists rebuilt for the destination at init time"
    ));
}

/// STEP_MoveMap_Finish (InGameStep step 8): the MoveMap load COMPLETED. Edge semaphore -- its
/// ABSENCE while MMS-INIT fired is the 3rd-load softlock (child never finished, step 7 self-looped).
pub(crate) unsafe extern "system" fn mms_step_finish_hook(
    this: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let n = SWITCH_ORACLE_MMS_FINISH_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.load(Ordering::SeqCst) != 0 {
        append_autoload_debug(format_args!(
            "MMS-FINISH #{n}: InGameStep=0x{this:x} -- MoveMap load COMPLETE (step 8); requestCode now drains 1->0, world enters"
        ));
    }
    unsafe { mms_call_original(&MMS_STEP_FINISH_ORIG, this, b, c, d) }
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
