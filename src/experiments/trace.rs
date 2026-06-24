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
        const CAP_CONTINUE_CONFIRM_RVA: u32 = 0x00b0e180;
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
    const DIALOG_FACTORY_RVA: usize = LIVE_DIALOG_FACTORY_RVA;
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

/// Menu/FD4 insertion helper 0x1407a7b60(rcx=registry/builder, rdx=descriptor): passive capture of
/// the exact objects TitleTopDialog::open_menu inserts. This is intentionally generic: log the
/// original return plus a few qwords around rcx/rdx so the next static/runtime step can identify the
/// registry storage without guessing dialog fields or generic Sequence trees.
unsafe fn log_menu_insert_details(a: usize, b: usize, c: usize, d: usize, ret: usize) {
    let n = CAP_MENU_INSERT_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n < CAP_MENU_INSERT_LOG_FIRST {
        let q = |addr: usize, off: usize| -> usize {
            if addr == TITLE_OWNER_SCAN_START_ADDRESS {
                TITLE_OWNER_SCAN_START_ADDRESS
            } else {
                unsafe { safe_read_usize(addr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            }
        };
        let base = {
            let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
            if own != TITLE_OWNER_SCAN_START_ADDRESS {
                own
            } else {
                game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            }
        };
        let avt = q(a, CAP_MENU_INSERT_VTABLE_OFFSET);
        let bvt = q(b, CAP_MENU_INSERT_VTABLE_OFFSET);
        let rvt = q(ret, CAP_MENU_INSERT_VTABLE_OFFSET);
        let arva = if base != TITLE_OWNER_SCAN_START_ADDRESS {
            avt.wrapping_sub(base)
        } else {
            avt
        };
        let brva = if base != TITLE_OWNER_SCAN_START_ADDRESS {
            bvt.wrapping_sub(base)
        } else {
            bvt
        };
        let rrva = if base != TITLE_OWNER_SCAN_START_ADDRESS {
            rvt.wrapping_sub(base)
        } else {
            rvt
        };
        append_continue_trace(format_args!(
            "CAP menu_insert #{} rcx=0x{a:x} vt=0x{avt:x}(rva 0x{arva:x}) a8=0x{:x} a10=0x{:x} a18=0x{:x} a38=0x{:x} a50=0x{:x} rdx=0x{b:x} vt=0x{bvt:x}(rva 0x{brva:x}) b8=0x{:x} b10=0x{:x} b18=0x{:x} b38=0x{:x} r8=0x{c:x} r9=0x{d:x} ret=0x{ret:x} ret_vt=0x{rvt:x}(rva 0x{rrva:x}) ret8=0x{:x} ret10=0x{:x} ret18=0x{:x} {}",
            n,
            q(a, CAP_MENU_INSERT_QWORD_8_OFFSET),
            q(a, CAP_MENU_INSERT_QWORD_10_OFFSET),
            q(a, CAP_MENU_INSERT_QWORD_18_OFFSET),
            q(a, CAP_MENU_INSERT_QWORD_38_OFFSET),
            q(a, CAP_MENU_INSERT_QWORD_50_OFFSET),
            q(b, CAP_MENU_INSERT_QWORD_8_OFFSET),
            q(b, CAP_MENU_INSERT_QWORD_10_OFFSET),
            q(b, CAP_MENU_INSERT_QWORD_18_OFFSET),
            q(b, CAP_MENU_INSERT_QWORD_38_OFFSET),
            q(ret, CAP_MENU_INSERT_QWORD_8_OFFSET),
            q(ret, CAP_MENU_INSERT_QWORD_10_OFFSET),
            q(ret, CAP_MENU_INSERT_QWORD_18_OFFSET),
            trace_callers_summary()
        ));
    }
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
    OWN_STEPPER_CONFIRMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
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
    const ARG_Q0_OFFSET: usize = 0x0;
    const ARG_Q8_OFFSET: usize = 0x8;
    const ARG_Q10_OFFSET: usize = 0x10;
    const ARG_Q18_OFFSET: usize = 0x18;
    let q = |ptr: usize, off: usize| -> usize {
        if ptr != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        }
    };
    append_continue_trace(format_args!(
        "CAP load_activate2 this=0x{this:x}[0x{:x},0x{:x},0x{:x},0x{:x}] a1=0x{b:x}[0x{:x},0x{:x}] a2=0x{c:x}[0x{:x},0x{:x},0x{:x},0x{:x}] a3=0x{d:x}[0x{:x},0x{:x}] {} {}",
        q(this, ARG_Q0_OFFSET),
        q(this, ARG_Q8_OFFSET),
        q(this, ARG_Q10_OFFSET),
        q(this, ARG_Q18_OFFSET),
        q(b, ARG_Q0_OFFSET),
        q(b, ARG_Q8_OFFSET),
        q(c, ARG_Q0_OFFSET),
        q(c, ARG_Q8_OFFSET),
        q(c, ARG_Q10_OFFSET),
        q(c, ARG_Q18_OFFSET),
        q(d, ARG_Q0_OFFSET),
        q(d, ARG_Q8_OFFSET),
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
    let slot_i32 = slot as i32;
    let expected_slot = OWN_STEPPER_EXPECTED_SLOT.load(Ordering::SeqCst);
    let effective_slot = slot;
    append_continue_trace(format_args!(
        "CAP builder owner=0x{owner:x} slot={} effective_slot={} rdx=0x{rdx:x} r9=0x{r9:x} {} {}",
        slot_i32,
        effective_slot as i32,
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_cap_original(&CAP_BUILDER_ORIG, owner, rdx, effective_slot, r9) };
    if (live_dialog_enabled() || product_autoload_enabled())
        && ret != TITLE_OWNER_SCAN_START_ADDRESS
    {
        #[repr(C)]
        struct SelectorBuilderOwnerLayout {
            unknown_000: [u8; 0xf8],
            selector_ctx: usize,
        }
        const SELECTOR_CTX_OFFSET_F8: usize =
            core::mem::offset_of!(SelectorBuilderOwnerLayout, selector_ctx);
        const SELECTOR_STEP_VTABLE_RVA: usize = ProfileLoadMenuRva::SelectorStepVtable as usize;
        let step = unsafe { safe_read_usize(ret) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let step_vt = if step != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_usize(step) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let ctx = ret + SELECTOR_CTX_OFFSET_F8;
        if game_module_base()
            .ok()
            .is_some_and(|base| step_vt == base + SELECTOR_STEP_VTABLE_RVA)
        {
            OWN_STEPPER_SELECTOR_STEP.store(step, Ordering::SeqCst);
            OWN_STEPPER_SELECTOR_CTX.store(ctx, Ordering::SeqCst);
        }
        append_autoload_debug(format_args!(
            "own_stepper: builder ret(owner)=0x{ret:x} step=[owner]=0x{step:x} step_vt=0x{step_vt:x} ctx(owner+0xf8)=0x{ctx:x} slot={} effective_slot={} for native selector self-pump",
            slot_i32, effective_slot as i32
        ));
    }
    ret
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
        const SELECTOR_STEP_Q10_OFFSET: usize =
            core::mem::size_of::<usize>() + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q18_OFFSET: usize =
            SELECTOR_STEP_Q10_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q20_OFFSET: usize =
            SELECTOR_STEP_Q18_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q28_OFFSET: usize =
            SELECTOR_STEP_Q20_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q30_OFFSET: usize =
            SELECTOR_STEP_Q28_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q38_OFFSET: usize =
            SELECTOR_STEP_Q30_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q50_OFFSET: usize = SELECTOR_STEP_Q38_OFFSET
            + core::mem::size_of::<usize>()
            + core::mem::size_of::<usize>()
            + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q58_OFFSET: usize =
            SELECTOR_STEP_Q50_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q60_OFFSET: usize =
            SELECTOR_STEP_Q58_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_TASK_OFFSET: usize = SELECTOR_STEP_Q60_OFFSET
            + core::mem::size_of::<usize>()
            + core::mem::size_of::<usize>();
        let step_q = |off: usize| -> usize {
            if step != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(step + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            }
        };
        let step_task = step_q(SELECTOR_STEP_TASK_OFFSET);
        let step_task_vt = if step_task != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_usize(step_task) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        const PTR_Q0_OFFSET: usize = 0x0;
        const PTR_Q8_OFFSET: usize = 0x8;
        const PTR_Q10_OFFSET: usize = 0x10;
        const PTR_Q18_OFFSET: usize = 0x18;
        let ptr_q = |ptr: usize, off: usize| -> usize {
            if ptr != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            }
        };
        let step_q10 = step_q(SELECTOR_STEP_Q10_OFFSET);
        let step_q18 = step_q(SELECTOR_STEP_Q18_OFFSET);
        let step_q20 = step_q(SELECTOR_STEP_Q20_OFFSET);
        let step_q28 = step_q(SELECTOR_STEP_Q28_OFFSET);
        let step_q30 = step_q(SELECTOR_STEP_Q30_OFFSET);
        let step_q38 = step_q(SELECTOR_STEP_Q38_OFFSET);
        let step_q50 = step_q(SELECTOR_STEP_Q50_OFFSET);
        let step_q58 = step_q(SELECTOR_STEP_Q58_OFFSET);
        let step_q60 = step_q(SELECTOR_STEP_Q60_OFFSET);
        append_continue_trace(format_args!(
            "CAP selector_tick #{n} step=0x{step:x} ctx=0x{ctx:x} installed={installed} ctx_slot={ctx_slot} task=0x{step_task:x} task_vt=0x{step_task_vt:x} step_q=[0x{step_q10:x},0x{step_q18:x},0x{step_q20:x},0x{step_q28:x},0x{step_q30:x},0x{step_q38:x},0x{step_q50:x},0x{step_q58:x},0x{step_q60:x}] q50_obj=[0x{:x},0x{:x},0x{:x},0x{:x}] q60_obj=[0x{:x},0x{:x},0x{:x},0x{:x}] {}",
            ptr_q(step_q50, PTR_Q0_OFFSET),
            ptr_q(step_q50, PTR_Q8_OFFSET),
            ptr_q(step_q50, PTR_Q10_OFFSET),
            ptr_q(step_q50, PTR_Q18_OFFSET),
            ptr_q(step_q60, PTR_Q0_OFFSET),
            ptr_q(step_q60, PTR_Q8_OFFSET),
            ptr_q(step_q60, PTR_Q10_OFFSET),
            ptr_q(step_q60, PTR_Q18_OFFSET),
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
    let base = game_module_base().unwrap_or(NULL);
    if product_autoload_enabled()
        && base != NULL
        && OWN_STEPPER_TITLE_FIRED.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS
        && OWN_STEPPER_PHASE.load(Ordering::SeqCst) == OWN_STEPPER_PHASE_MENU
        && ret_vt == base + PROFILE_LOAD_DIALOG_VTABLE_RVA
    {
        OWN_STEPPER_DIALOG.store(ret, Ordering::SeqCst);
        own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_ACTIVATE);
        append_autoload_debug(format_args!(
            "product-core-autoload: native TitleTopDialog Load-Game factory returned ProfileLoadDialog=0x{ret:x} vt=0x{ret_vt:x}; captured by factory hook -> STAGE2 ACTIVATE"
        ));
    }
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
    {
        const Q0: usize = 0x0;
        const Q1: usize = 0x8;
        const Q2: usize = 0x10;
        const Q3: usize = 0x18;
        let q = |ptr: usize, off: usize| -> usize {
            if ptr != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            }
        };
        let io = game_module_base()
            .ok()
            .map(|base| unsafe { *((base + IODEV_GLOBAL_RVA) as *const usize) })
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let io18 = q(io, IODEV_REQHANDLE_18_OFFSET);
        let io20 = q(io, IODEV_REQHANDLE_20_OFFSET);
        append_continue_trace(format_args!(
            "CAP menu_deser RAW this=[0x{:x},0x{:x},0x{:x},0x{:x}] ctx=[0x{:x},0x{:x},0x{:x},0x{:x}] io18=0x{io18:x}[0x{:x},0x{:x},0x{:x},0x{:x}] io20=0x{io20:x}[0x{:x},0x{:x},0x{:x},0x{:x}]",
            q(this, Q0),
            q(this, Q1),
            q(this, Q2),
            q(this, Q3),
            q(ctx, Q0),
            q(ctx, Q1),
            q(ctx, Q2),
            q(ctx, Q3),
            q(io18, Q0),
            q(io18, Q1),
            q(io18, Q2),
            q(io18, Q3),
            q(io20, Q0),
            q(io20, Q1),
            q(io20, Q2),
            q(io20, Q3),
        ));
    }
    let ret = unsafe { call_cap_original(&CAP_MENU_DESER_ORIG, this, ctx, c, d) };
    append_continue_trace(format_args!(
        "CAP menu_deser LEAVE ret=0x{ret:x} {}",
        b80_mount_trace_summary()
    ));
    ret
}

/// Title native-ready predicate 0x140733150 hook. Static RE shows the original body is:
/// `state = this->vtable[0](this); return (state->flags_20 & 0x8f) != 0`. Re-implement that tiny
/// body exactly so the hook can record the returned state object/flags without making a second
/// native getter call or changing success semantics.
pub(crate) unsafe extern "system" fn title_native_ready_predicate_hook(this: usize) -> usize {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const STATE_FLAGS_20_OFFSET: usize = 0x20;
    const READY_MASK_8F: usize = 0x8f;
    type StateGetter = unsafe extern "system" fn(usize) -> usize;

    let caller_rva = trace_first_game_caller_rva();
    let vtable = unsafe { safe_read_usize(this) }.unwrap_or(NULL);
    let getter = if vtable != NULL {
        unsafe { safe_read_usize(vtable) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let state = if getter != NULL {
        let f: StateGetter = unsafe { std::mem::transmute(getter) };
        unsafe { f(this) }
    } else {
        NULL
    };
    let flags = if state != NULL {
        unsafe { safe_read_usize(state + STATE_FLAGS_20_OFFSET) }.unwrap_or(0) & 0xff
    } else {
        0
    };
    let masked = flags & READY_MASK_8F;
    let ret = if masked != 0 { 1 } else { 0 };

    TITLE_NATIVE_READY_PREDICATE_HITS.fetch_add(1, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_THIS.store(this, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_VTABLE.store(vtable, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_GETTER.store(getter, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_OBJECT.store(state, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_FLAGS.store(flags, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_MASKED.store(masked, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_RET.store(ret, Ordering::SeqCst);

    ret
}

/// MenuWindowJob ctor 0x1407ac8c0 hook: observe constructed menu jobs and latch the semantic
/// Continue item only when both the Continue action and native accept predicate are installed.
/// This avoids poisoning MENU_CONTINUE_ITEM with the first updated title input leaf, whose
/// accept predicate is the constant-false 0x1407add70 diagnostic dead end.
pub(crate) unsafe extern "system" fn menu_window_job_ctor_hook(
    out_slot: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let ret = unsafe { call_cap_original(&MENU_WINDOW_JOB_CTOR_ORIG, out_slot, b, c, d) };
    if !product_autoload_enabled() || out_slot == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != TITLE_OWNER_SCAN_START_ADDRESS {
            own
        } else {
            game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        }
    };
    if base == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET: usize = 0xf8;
    const MENU_ITEM_ACCEPT_NATIVE_RVA: usize = 0x007ad810;
    let item = unsafe { safe_read_usize(out_slot) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    if item == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor = unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor_vt = if functor != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let do_call = if functor_vt != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor_vt + DOCALL_VTABLE_SLOT_10) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let accept_predicate = unsafe { safe_read_usize(item + MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    MENU_WINDOW_JOB_CTOR_HITS.fetch_add(1, Ordering::SeqCst);
    MENU_WINDOW_JOB_CTOR_LAST_ITEM.store(item, Ordering::SeqCst);
    MENU_WINDOW_JOB_CTOR_LAST_VT.store(vt, Ordering::SeqCst);
    MENU_WINDOW_JOB_CTOR_LAST_FUNCTOR.store(functor, Ordering::SeqCst);
    MENU_WINDOW_JOB_CTOR_LAST_DOCALL.store(do_call, Ordering::SeqCst);
    MENU_WINDOW_JOB_CTOR_LAST_ACCEPT.store(accept_predicate, Ordering::SeqCst);
    let continue_candidate =
        vt == base + MENU_WINDOW_JOB_VTABLE_RVA && do_call == base + MENU_TITLE_CONTINUE_DOCALL_RVA;
    if continue_candidate {
        record_continue_candidate(item, accept_predicate, base);
    }
    let semantic_continue_item =
        continue_candidate && accept_predicate == base + MENU_ITEM_ACCEPT_NATIVE_RVA;
    if semantic_continue_item {
        MENU_WINDOW_JOB_CTOR_SEMANTIC_HITS.fetch_add(1, Ordering::SeqCst);
    }
    if semantic_continue_item
        && MENU_CONTINUE_ITEM
            .compare_exchange(
                TITLE_OWNER_SCAN_START_ADDRESS,
                item,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
    {
        append_continue_trace(format_args!(
            "MENU-WINDOW-CTOR captured semantic native Continue item=0x{item:x} out=0x{out_slot:x} vt=0x{vt:x} functor=0x{functor:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x} item_fields{{{}}} {}",
            unsafe { menu_item_action_summary(item) },
            trace_callers_summary()
        ));
        append_autoload_debug(format_args!(
            "product-core-autoload: constructor captured semantic native Continue MenuWindowJob item=0x{item:x} vt=0x{vt:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x}"
        ));
    }
    ret
}

/// MenuWindowJob native-accept ctor variant 0x1407acb00 hook: observe constructed menu jobs from
/// the sibling constructor that static RE shows installs the native accept predicate 0x1407ad810.
/// This is passive except for the same semantic pointer latch used by the existing 0x1407ac8c0
/// constructor hook: if the item is a Continue row with native accept, record its pointer so the
/// product path can later submit through native semantics.
pub(crate) unsafe extern "system" fn menu_window_job_native_ctor_b_hook(
    out_slot: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let caller_rva = trace_first_game_caller_rva();
    let ret = unsafe { call_cap_original(&MENU_WINDOW_JOB_NATIVE_CTOR_B_ORIG, out_slot, b, c, d) };
    if !product_autoload_enabled() || out_slot == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != TITLE_OWNER_SCAN_START_ADDRESS {
            own
        } else {
            game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        }
    };
    if base == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET: usize = 0xf8;
    const MENU_ITEM_ACCEPT_NATIVE_RVA: usize = 0x007ad810;
    let item = unsafe { safe_read_usize(out_slot) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    if item == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor = unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor_vt = if functor != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let do_call = if functor_vt != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor_vt + DOCALL_VTABLE_SLOT_10) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let accept_predicate = unsafe { safe_read_usize(item + MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_HITS.fetch_add(1, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_ITEM.store(item, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_OUT_SLOT.store(out_slot, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_VT.store(vt, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_FUNCTOR.store(functor, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_DOCALL.store(do_call, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_ACCEPT.store(accept_predicate, Ordering::SeqCst);
    let semantic_continue_item = vt == base + MENU_WINDOW_JOB_VTABLE_RVA
        && do_call == base + MENU_TITLE_CONTINUE_DOCALL_RVA
        && accept_predicate == base + MENU_ITEM_ACCEPT_NATIVE_RVA;
    if semantic_continue_item {
        MENU_WINDOW_JOB_NATIVE_CTOR_B_CONTINUE_HITS.fetch_add(1, Ordering::SeqCst);
        record_continue_candidate(item, accept_predicate, base);
        if MENU_CONTINUE_ITEM
            .compare_exchange(
                TITLE_OWNER_SCAN_START_ADDRESS,
                item,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            append_continue_trace(format_args!(
                "MENU-WINDOW-NATIVE-CTOR-B captured semantic native Continue item=0x{item:x} caller_rva=0x{caller_rva:x} out=0x{out_slot:x} vt=0x{vt:x} functor=0x{functor:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x} item_fields{{{}}} {}",
                unsafe { menu_item_action_summary(item) },
                trace_callers_summary()
            ));
            append_autoload_debug(format_args!(
                "product-core-autoload: native ctor B captured semantic native Continue MenuWindowJob item=0x{item:x} caller_rva=0x{caller_rva:x} accept_predicate=0x{accept_predicate:x}"
            ));
        }
    }
    ret
}

/// MenuWindowJob disabled/idle ctor 0x1407acf80 hook: observe constructed menu jobs whose accept
/// functor is the constant-false 0x1407add70 variant. Static RE of the constructor shows it builds
/// the same MenuWindowJob vtable but installs the idle predicate into item+0xf0/+0xf8; this hook
/// attributes Continue-looking candidates to that disabled native path without promoting or
/// submitting them.
pub(crate) unsafe extern "system" fn menu_window_job_idle_ctor_hook(
    out_slot: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let caller_rva = trace_first_game_caller_rva();
    let ret = unsafe { call_cap_original(&MENU_WINDOW_JOB_IDLE_CTOR_ORIG, out_slot, b, c, d) };
    if !product_autoload_enabled() || out_slot == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != TITLE_OWNER_SCAN_START_ADDRESS {
            own
        } else {
            game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        }
    };
    if base == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET: usize = 0xf8;
    let item = unsafe { safe_read_usize(out_slot) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    if item == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor = unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor_vt = if functor != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let do_call = if functor_vt != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor_vt + DOCALL_VTABLE_SLOT_10) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let accept_predicate = unsafe { safe_read_usize(item + MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    MENU_WINDOW_JOB_IDLE_CTOR_HITS.fetch_add(1, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_ITEM.store(item, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_VT.store(vt, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_FUNCTOR.store(functor, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_DOCALL.store(do_call, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_ACCEPT.store(accept_predicate, Ordering::SeqCst);
    let continue_candidate =
        vt == base + MENU_WINDOW_JOB_VTABLE_RVA && do_call == base + MENU_TITLE_CONTINUE_DOCALL_RVA;
    if continue_candidate {
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_HITS.fetch_add(1, Ordering::SeqCst);
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_ITEM.store(item, Ordering::SeqCst);
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_OUT_SLOT.store(out_slot, Ordering::SeqCst);
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_DOCALL.store(do_call, Ordering::SeqCst);
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_ACCEPT.store(accept_predicate, Ordering::SeqCst);
        record_continue_candidate(item, accept_predicate, base);
        append_continue_trace(format_args!(
            "MENU-WINDOW-IDLE-CTOR observed Continue-looking disabled item=0x{item:x} caller_rva=0x{caller_rva:x} vt=0x{vt:x} functor=0x{functor:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x} item_fields{{{}}} {}",
            unsafe { menu_item_action_summary(item) },
            trace_callers_summary()
        ));
    }
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
    if product_autoload_enabled()
        && item != TITLE_OWNER_SCAN_START_ADDRESS
        && base != TITLE_OWNER_SCAN_START_ADDRESS
    {
        const DOCALL_VTABLE_SLOT_10: usize = 0x10;
        const MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET: usize = 0xf8;
        const MENU_ITEM_ACCEPT_NATIVE_RVA: usize = 0x007ad810;
        let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let functor = unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let functor_vt = if functor != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_usize(functor) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let do_call = if functor_vt != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_usize(functor_vt + DOCALL_VTABLE_SLOT_10) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let accept_predicate =
            unsafe { safe_read_usize(item + MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        MENU_ITEM_UPDATE_HITS.fetch_add(1, Ordering::SeqCst);
        MENU_ITEM_UPDATE_LAST_ITEM.store(item, Ordering::SeqCst);
        MENU_ITEM_UPDATE_LAST_VT.store(vt, Ordering::SeqCst);
        MENU_ITEM_UPDATE_LAST_FUNCTOR.store(functor, Ordering::SeqCst);
        MENU_ITEM_UPDATE_LAST_DOCALL.store(do_call, Ordering::SeqCst);
        MENU_ITEM_UPDATE_LAST_ACCEPT.store(accept_predicate, Ordering::SeqCst);
        let continue_candidate = vt == base + MENU_WINDOW_JOB_VTABLE_RVA
            && do_call == base + MENU_TITLE_CONTINUE_DOCALL_RVA;
        if continue_candidate {
            record_continue_candidate(item, accept_predicate, base);
        }
        let semantic_continue_item =
            continue_candidate && accept_predicate == base + MENU_ITEM_ACCEPT_NATIVE_RVA;
        if semantic_continue_item {
            MENU_ITEM_UPDATE_SEMANTIC_HITS.fetch_add(1, Ordering::SeqCst);
        }
        if semantic_continue_item
            && MENU_CONTINUE_ITEM
                .compare_exchange(
                    TITLE_OWNER_SCAN_START_ADDRESS,
                    item,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
        {
            append_continue_trace(format_args!(
                "MENU-ITEM-UPDATE captured semantic native Continue item=0x{item:x} vt=0x{vt:x} functor=0x{functor:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x} item_fields{{{}}} {}",
                unsafe { menu_item_action_summary(item) },
                trace_callers_summary()
            ));
            append_autoload_debug(format_args!(
                "product-core-autoload: captured semantic native Continue MenuWindowJob item=0x{item:x} vt=0x{vt:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x}"
            ));
        }
    }
    if product_autoload_enabled()
        && item != TITLE_OWNER_SCAN_START_ADDRESS
        && item == MENU_CONTINUE_ITEM.load(Ordering::SeqCst)
    {
        let n =
            MENU_CONTINUE_ITEM_FIELD_LOG_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        const FIELD_LOG_0: usize = 0;
        const FIELD_LOG_8: usize = 8;
        const FIELD_LOG_30: usize = 30;
        const FIELD_LOG_60: usize = 60;
        const FIELD_LOG_120: usize = 120;
        if n == FIELD_LOG_0
            || n == FIELD_LOG_8
            || n == FIELD_LOG_30
            || n == FIELD_LOG_60
            || n == FIELD_LOG_120
        {
            append_continue_trace(format_args!(
                "MENU-ITEM-UPDATE Continue candidate fields tick_count={n} item=0x{item:x} item_fields{{{}}} {}",
                unsafe { menu_item_action_summary(item) },
                trace_callers_summary()
            ));
        }
    }
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
                if product_autoload_enabled() {
                    append_continue_trace(format_args!(
                        "MENU-ITEM-UPDATE #{n} item=0x{item:x} vt=0x{vt:x} item_fields{{{}}} {chain} load_game=false {}",
                        unsafe { menu_item_action_summary(item) },
                        trace_callers_summary()
                    ));
                } else {
                    append_continue_trace(format_args!(
                        "MENU-ITEM-UPDATE #{n} item=0x{item:x} vt=0x{vt:x} {chain} load_game=false {}",
                        trace_callers_summary()
                    ));
                }
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

fn format_optional_usize_hex(value: usize) -> String {
    if value == TITLE_OWNER_SCAN_START_ADDRESS {
        "null".to_owned()
    } else {
        format!("0x{value:x}")
    }
}

unsafe fn result_built_flag(result: usize) -> usize {
    const RESULT_BUILT_3B0_OFFSET: usize = 0x3b0;
    const U8_MASK: usize = 0xff;
    if result == TITLE_OWNER_SCAN_START_ADDRESS {
        TITLE_OWNER_SCAN_START_ADDRESS
    } else {
        unsafe { safe_read_usize(result + RESULT_BUILT_3B0_OFFSET) }
            .map_or(TITLE_OWNER_SCAN_START_ADDRESS, |value| value & U8_MASK)
    }
}

unsafe fn native_result_event_words(event: usize) -> (usize, usize) {
    const EVENT_WORD0_OFFSET: usize = 0;
    const EVENT_WORD1_OFFSET: usize = core::mem::size_of::<usize>();
    if event == TITLE_OWNER_SCAN_START_ADDRESS {
        return (
            TITLE_OWNER_SCAN_START_ADDRESS,
            TITLE_OWNER_SCAN_START_ADDRESS,
        );
    }
    let word0 = unsafe { safe_read_usize(event + EVENT_WORD0_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let word1 = unsafe { safe_read_usize(event + EVENT_WORD1_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    (word0, word1)
}

fn fd4_event_code_arg(raw_qword0: usize) -> (usize, usize) {
    const U32_MASK: usize = 0xffff_ffff;
    if raw_qword0 == TITLE_OWNER_SCAN_START_ADDRESS {
        return (
            TITLE_OWNER_SCAN_START_ADDRESS,
            TITLE_OWNER_SCAN_START_ADDRESS,
        );
    }
    (raw_qword0 & U32_MASK, (raw_qword0 >> 32) & U32_MASK)
}

pub(crate) unsafe extern "system" fn native_submit_hook(result: usize) {
    const TRACE_FIRST: usize = 16;
    let seq =
        NATIVE_SUBMIT_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) + OWN_STEPPER_CALL_INC;
    NATIVE_SUBMIT_LAST_RESULT.store(result, Ordering::SeqCst);
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "native_submit_7ac890 seq={seq} phase=ENTER result=0x{result:x} built={} {}",
            format_optional_usize_hex(unsafe { result_built_flag(result) }),
            trace_callers_summary()
        ));
    }
    let _ = unsafe { call_result_void1_original(&NATIVE_SUBMIT_ORIG, result) };
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "native_submit_7ac890 seq={seq} phase=LEAVE result=0x{result:x} built={} {}",
            format_optional_usize_hex(unsafe { result_built_flag(result) }),
            game_man_trace_summary()
        ));
    }
}

pub(crate) unsafe extern "system" fn result_event_handler_hook(result: usize, event: usize) {
    const TRACE_FIRST: usize = 16;
    let seq = RESULT_EVENT_HANDLER_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    RESULT_EVENT_LAST_RESULT.store(result, Ordering::SeqCst);
    RESULT_EVENT_LAST_EVENT.store(event, Ordering::SeqCst);
    let (event_raw_qword0, _) = unsafe { native_result_event_words(event) };
    let (fd4_code, fd4_arg) = fd4_event_code_arg(event_raw_qword0);
    RESULT_EVENT_LAST_RAW_QWORD0.store(event_raw_qword0, Ordering::SeqCst);
    RESULT_EVENT_LAST_FD4_CODE.store(fd4_code, Ordering::SeqCst);
    RESULT_EVENT_LAST_FD4_ARG.store(fd4_arg, Ordering::SeqCst);
    let built_before = unsafe { result_built_flag(result) };
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "result_event_handler_746e80 seq={seq} phase=ENTER result=0x{result:x} event=0x{event:x} event_raw_qword0={} fd4_code={} fd4_arg={} built_before={} {}",
            format_optional_usize_hex(event_raw_qword0),
            format_optional_usize_hex(fd4_code),
            format_optional_usize_hex(fd4_arg),
            format_optional_usize_hex(built_before),
            trace_callers_summary()
        ));
    }
    let _ = unsafe { call_result_void2_original(&RESULT_EVENT_HANDLER_ORIG, result, event) };
    let built_after = unsafe { result_built_flag(result) };
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "result_event_handler_746e80 seq={seq} phase=LEAVE result=0x{result:x} event=0x{event:x} built_after={} {}",
            format_optional_usize_hex(built_after),
            game_man_trace_summary()
        ));
    }
}

pub(crate) unsafe extern "system" fn result_event_wrapper_builder_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
) -> usize {
    const TRACE_FIRST: usize = 16;
    const RESULT_ACTION_BUILDER_TRACE_SIZE: usize = 0x360;
    let from_result_action_builder = callstack_contains_game_rva(
        RESULT_ACTION_BUILDER_RVA as usize,
        RESULT_ACTION_BUILDER_RVA as usize + RESULT_ACTION_BUILDER_TRACE_SIZE,
    );
    let result = unsafe { call_wrapper_builder_original(rcx, rdx, r8) }.unwrap_or(rcx);
    if from_result_action_builder {
        let seq = RESULT_ACTION_WRAPPER_BUILDER_HITS
            .fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
            + OWN_STEPPER_CALL_INC;
        let base = game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let ret_update_rva = if base == TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_OWNER_SCAN_START_ADDRESS
        } else {
            unsafe { task_node_update_rva(base, result) }
        };
        RESULT_ACTION_LAST_WRAPPER_BUILDER_RCX.store(rcx, Ordering::SeqCst);
        RESULT_ACTION_LAST_WRAPPER_BUILDER_RDX.store(rdx, Ordering::SeqCst);
        RESULT_ACTION_LAST_WRAPPER_BUILDER_R8.store(r8, Ordering::SeqCst);
        RESULT_ACTION_LAST_WRAPPER_BUILDER_RET.store(result, Ordering::SeqCst);
        RESULT_ACTION_LAST_WRAPPER_BUILDER_RET_UPDATE_RVA.store(ret_update_rva, Ordering::SeqCst);
        if seq <= TRACE_FIRST {
            append_continue_trace(format_args!(
                "result_event_wrapper_builder_744a60 seq={seq} rcx=0x{rcx:x} rdx=0x{rdx:x} r8=0x{r8:x} ret=0x{result:x} ret_update_rva={} -- passive wrapper-builder call from result action builder",
                format_optional_usize_hex(ret_update_rva)
            ));
        }
    }
    result
}

pub(crate) unsafe extern "system" fn result_action_builder_hook(result: usize, event: usize) {
    const TRACE_FIRST: usize = 16;
    let seq = RESULT_ACTION_BUILDER_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    RESULT_ACTION_LAST_RESULT.store(result, Ordering::SeqCst);
    RESULT_ACTION_LAST_EVENT.store(event, Ordering::SeqCst);
    let (event_word0, event_word1) = unsafe { native_result_event_words(event) };
    RESULT_ACTION_LAST_WORD0.store(event_word0, Ordering::SeqCst);
    RESULT_ACTION_LAST_WORD1.store(event_word1, Ordering::SeqCst);
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "result_action_builder_746a00 seq={seq} phase=ENTER result=0x{result:x} event=0x{event:x} event_word0={} event_word1={} built={} {}",
            format_optional_usize_hex(event_word0),
            format_optional_usize_hex(event_word1),
            format_optional_usize_hex(unsafe { result_built_flag(result) }),
            trace_callers_summary()
        ));
    }
    let _ = unsafe { call_result_void2_original(&RESULT_ACTION_BUILDER_ORIG, result, event) };
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "result_action_builder_746a00 seq={seq} phase=LEAVE result=0x{result:x} event=0x{event:x} built={} {}",
            format_optional_usize_hex(unsafe { result_built_flag(result) }),
            game_man_trace_summary()
        ));
    }
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

unsafe fn text_section_bounds(base: usize) -> Option<(usize, usize)> {
    let e_lfanew = unsafe { safe_read_usize(base + PE_DOS_LFANEW_OFFSET) }? & PE_U32_MASK;
    let nt = base + e_lfanew;
    let num_sections = unsafe { safe_read_usize(nt + PE_FILE_NUM_SECTIONS_OFFSET) }? & PE_U16_MASK;
    let size_opt = unsafe { safe_read_usize(nt + PE_FILE_SIZE_OPT_HEADER_OFFSET) }? & PE_U16_MASK;
    let sections = nt + PE_OPT_HEADER_OFFSET + size_opt;
    let mut index = PE_SECTION_SCAN_START;
    while index < num_sections {
        let header = sections + index * PE_SECTION_HEADER_SIZE;
        let name = unsafe { safe_read_usize(header) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        if name.to_le_bytes().starts_with(PE_TEXT_SECTION_NAME) {
            let vsize = unsafe { safe_read_usize(header + PE_SECTION_VSIZE_OFFSET) }? & PE_U32_MASK;
            let vaddr = unsafe { safe_read_usize(header + PE_SECTION_VADDR_OFFSET) }? & PE_U32_MASK;
            return Some((base + vaddr, vsize));
        }
        index += OWN_STEPPER_CALL_INC;
    }
    None
}

unsafe fn update_target_in_text(base: usize, update: usize) -> bool {
    if update < base {
        return false;
    }
    let Some((text_start, text_len)) = (unsafe { text_section_bounds(base) }) else {
        return false;
    };
    update >= text_start && update < text_start.saturating_add(text_len)
}

unsafe fn raw_task_node_update_rva(base: usize, node: usize) -> usize {
    const TASK_NODE_UPDATE_VTABLE_SLOT: usize = 0x10;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let Some(vtable) = (unsafe { safe_read_usize(node) }) else {
        return null;
    };
    let Some(update) = (unsafe { safe_read_usize(vtable + TASK_NODE_UPDATE_VTABLE_SLOT) }) else {
        return null;
    };
    if unsafe { update_target_in_text(base, update) } {
        update - base
    } else {
        null
    }
}

pub(crate) unsafe fn task_node_update_rva(base: usize, node: usize) -> usize {
    let direct = unsafe { raw_task_node_update_rva(base, node) };
    if direct != TITLE_OWNER_SCAN_START_ADDRESS {
        return direct;
    }
    let Some(shared_pointee) = (unsafe { safe_read_usize(node) }) else {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    };
    unsafe { raw_task_node_update_rva(base, shared_pointee) }
}

unsafe fn qword_window_summary(ptr: usize) -> String {
    const QWORDS: usize = 6;
    const START: usize = 0;
    const STEP: usize = 1;
    const STRIDE: usize = core::mem::size_of::<usize>();
    let mut out = String::new();
    let mut i = START;
    while i < QWORDS {
        let off = i * STRIDE;
        let value = unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let _ = core::fmt::write(&mut out, format_args!(" +0x{off:x}=0x{value:x}"));
        i += STEP;
    }
    out
}

unsafe fn menu_item_action_summary(ptr: usize) -> String {
    const OFFSETS: [usize; 14] = [
        0x0, 0x8, 0x10, 0x40, 0x50, 0x68, 0xa8, 0xb0, 0xe8, 0xf0, 0xf8, 0x100, 0x130, 0x138,
    ];
    let mut out = String::new();
    for off in OFFSETS {
        let value = unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let _ = core::fmt::write(&mut out, format_args!(" +0x{off:x}=0x{value:x}"));
        if value != TITLE_OWNER_SCAN_START_ADDRESS {
            let _ = core::fmt::write(
                &mut out,
                format_args!(" ->{{{}}}", unsafe { qword_window_summary(value) }),
            );
        }
    }
    out
}

unsafe fn task_node_raw_summary(ptr: usize) -> String {
    const QWORDS: usize = 8;
    const START: usize = 0;
    const STEP: usize = 1;
    const STRIDE: usize = core::mem::size_of::<usize>();
    let mut out = String::new();
    let mut first = TITLE_OWNER_SCAN_START_ADDRESS;
    let mut second = TITLE_OWNER_SCAN_START_ADDRESS;
    let mut i = START;
    while i < QWORDS {
        let off = i * STRIDE;
        let value = unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        if i == START {
            first = value;
        } else if i == STEP {
            second = value;
        }
        let _ = core::fmt::write(&mut out, format_args!(" +0x{off:x}=0x{value:x}"));
        i += STEP;
    }
    if first != TITLE_OWNER_SCAN_START_ADDRESS {
        let _ = core::fmt::write(
            &mut out,
            format_args!(" | *q0{{{}}}", unsafe { qword_window_summary(first) }),
        );
    }
    if second != TITLE_OWNER_SCAN_START_ADDRESS {
        let _ = core::fmt::write(
            &mut out,
            format_args!(" | *q8{{{}}}", unsafe { qword_window_summary(second) }),
        );
    }
    out
}

unsafe fn capture_continue_task_node_candidate(base: usize, candidate: usize, label: &str) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if candidate == null {
        return;
    }
    let update_rva = unsafe { task_node_update_rva(base, candidate) };
    if update_rva != TRACE_MENU_CONTINUE_WRAPPER_RVA as usize {
        return;
    }
    if MENU_CONTINUE_TASK_NODE
        .compare_exchange(null, candidate, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        append_continue_trace(format_args!(
            "CAP continue_task_node {label}=0x{candidate:x} update_rva=0x{update_rva:x} -- captured native Continue menu task wrapper"
        ));
        append_autoload_debug(format_args!(
            "product-core-autoload: captured native Continue task node from {label}=0x{candidate:x} update_rva=0x{update_rva:x}"
        ));
    }
}

unsafe fn capture_continue_member_node_candidate(base: usize, candidate: usize, label: &str) {
    const MEMBER_DIALOG_10: usize = core::mem::size_of::<usize>() + core::mem::size_of::<usize>();
    const MEMBER_FN_18: usize = 0x18;
    const MEMBER_ADJ_20: usize = 0x20;
    const JMP_HOPS: usize = 6;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if candidate == null {
        return;
    }
    let node_vt = unsafe { safe_read_usize(candidate) }.unwrap_or(null);
    if node_vt != base + MEMBERFUNCJOB_VTABLE_RVA {
        return;
    }
    let member_fn = unsafe { safe_read_usize(candidate + MEMBER_FN_18) }.unwrap_or(null);
    if member_fn == null {
        return;
    }
    let continue_wrapper = base + TRACE_MENU_CONTINUE_WRAPPER_RVA as usize;
    let mut target = member_fn;
    let mut hop = 0;
    while hop < JMP_HOPS && target != null {
        if target == continue_wrapper {
            let member_dialog =
                unsafe { safe_read_usize(candidate + MEMBER_DIALOG_10) }.unwrap_or(null);
            let member_adjust =
                unsafe { safe_read_usize(candidate + MEMBER_ADJ_20) }.unwrap_or(null);
            if MENU_CONTINUE_MEMBER_NODE
                .compare_exchange(null, candidate, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                append_continue_trace(format_args!(
                    "CAP continue_member_node {label}=0x{candidate:x} node_vt=0x{node_vt:x} member_dialog=0x{member_dialog:x} member_fn=0x{member_fn:x} member_adjust=0x{member_adjust:x} -- captured registered TitleTopDialog Continue MenuMemberFuncJob"
                ));
                append_autoload_debug(format_args!(
                    "product-core-autoload: captured registered TitleTopDialog Continue MenuMemberFuncJob from {label}=0x{candidate:x} member_fn=0x{member_fn:x}"
                ));
            }
            return;
        }
        match unsafe { decode_thunk_hop(target) } {
            Some(next) => target = next,
            None => break,
        }
        hop += 1;
    }
}

pub(crate) unsafe extern "system" fn task_enqueue_hook(
    arg0: *mut c_void,
    arg1: *mut c_void,
) -> *mut c_void {
    let caller_rva = trace_first_game_caller_rva();
    let trace_index = TASK_ENQUEUE_TRACE_COUNT
        .fetch_add(TASK_ENQUEUE_TRACE_INCREMENT, Ordering::SeqCst)
        + TASK_ENQUEUE_TRACE_INCREMENT;
    let should_trace = trace_index <= TASK_ENQUEUE_TRACE_LIMIT
        || SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst)
            > NO_SAFE_INPUT_CONFIRM_FRAMES;
    if should_trace {
        append_continue_trace(format_args!(
            "menu_task_enqueue seq={trace_index} phase=ENTER hook_rva=0x{:x} list={arg0:p} node={arg1:p} node_{} raw{{{}}} confirm_active={} pulse={} {} {}",
            TRACE_TASK_ENQUEUE_RVA,
            unsafe { object_vtable_summary(arg1) },
            unsafe { task_node_raw_summary(arg1 as usize) },
            SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst)
                > NO_SAFE_INPUT_CONFIRM_FRAMES,
            SAFE_INPUT_CONFIRM_PULSE_SEQ.load(Ordering::SeqCst),
            trace_callers_summary(),
            game_man_trace_summary()
        ));
    }
    let result = unsafe { call_task_enqueue_original(arg0, arg1) }.unwrap_or(arg1);
    let arg0_pointee = if arg0 as usize != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(arg0 as usize) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let generic_hit = TASK_ENQUEUE_GENERIC_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    TASK_ENQUEUE_GENERIC_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    TASK_ENQUEUE_GENERIC_LAST_ARG0.store(arg0 as usize, Ordering::SeqCst);
    TASK_ENQUEUE_GENERIC_LAST_ARG0_POINTEE.store(arg0_pointee, Ordering::SeqCst);
    TASK_ENQUEUE_GENERIC_LAST_ARG1.store(arg1 as usize, Ordering::SeqCst);
    TASK_ENQUEUE_GENERIC_LAST_RET.store(result as usize, Ordering::SeqCst);
    match generic_hit {
        1 => {
            TASK_ENQUEUE_GENERIC_SAMPLE0_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE0_ARG0.store(arg0 as usize, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE0_ARG0_POINTEE.store(arg0_pointee, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE0_ARG1.store(arg1 as usize, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE0_RET.store(result as usize, Ordering::SeqCst);
        }
        2 => {
            TASK_ENQUEUE_GENERIC_SAMPLE1_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE1_ARG0.store(arg0 as usize, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE1_ARG0_POINTEE.store(arg0_pointee, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE1_ARG1.store(arg1 as usize, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE1_RET.store(result as usize, Ordering::SeqCst);
        }
        _ => {}
    }
    const MENU_CONTINUE_IDLE_INSERT_CALLER_RVA: usize = 0x0076432c;
    const MENU_CONTINUE_IDLE_INSERT_CALLER_START_RVA: usize = 0x007642b0;
    const MENU_CONTINUE_IDLE_INSERT_CALLER_END_RVA: usize = 0x007643c0;
    let idle_ctor_out_slot =
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_OUT_SLOT.load(Ordering::SeqCst);
    let idle_ctor_item = MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_ITEM.load(Ordering::SeqCst);
    let arg0_points_to_idle_item = arg0_pointee == idle_ctor_item;
    const TASK_ENQUEUE_IDLE_MATCH_CALLER_EXACT: usize = 1;
    const TASK_ENQUEUE_IDLE_MATCH_CALLER_RANGE: usize = 2;
    const TASK_ENQUEUE_IDLE_MATCH_ARG0_OUT_SLOT: usize = 3;
    const TASK_ENQUEUE_IDLE_MATCH_ARG0_POINTEE: usize = 4;
    const TASK_ENQUEUE_IDLE_MATCH_ARG1_ITEM: usize = 5;
    let stack_contains_idle_caller = callstack_contains_game_rva(
        MENU_CONTINUE_IDLE_INSERT_CALLER_START_RVA,
        MENU_CONTINUE_IDLE_INSERT_CALLER_END_RVA,
    );
    let idle_match_kind = if caller_rva == MENU_CONTINUE_IDLE_INSERT_CALLER_RVA {
        TASK_ENQUEUE_IDLE_MATCH_CALLER_EXACT
    } else if stack_contains_idle_caller {
        TASK_ENQUEUE_IDLE_MATCH_CALLER_RANGE
    } else if idle_ctor_out_slot != TITLE_OWNER_SCAN_START_ADDRESS
        && arg0 as usize == idle_ctor_out_slot
    {
        TASK_ENQUEUE_IDLE_MATCH_ARG0_OUT_SLOT
    } else if idle_ctor_item != TITLE_OWNER_SCAN_START_ADDRESS && arg0_points_to_idle_item {
        TASK_ENQUEUE_IDLE_MATCH_ARG0_POINTEE
    } else if idle_ctor_item != TITLE_OWNER_SCAN_START_ADDRESS && arg1 as usize == idle_ctor_item {
        TASK_ENQUEUE_IDLE_MATCH_ARG1_ITEM
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let idle_continue_insert_match = idle_match_kind != TITLE_OWNER_SCAN_START_ADDRESS;
    if idle_continue_insert_match {
        TASK_ENQUEUE_GENERIC_IDLE_ITEM_MATCH_HITS.fetch_add(1, Ordering::SeqCst);
        TASK_ENQUEUE_GENERIC_IDLE_ITEM_LAST_MATCH_KIND.store(idle_match_kind, Ordering::SeqCst);
    }
    if idle_continue_insert_match {
        let hit = MENU_CONTINUE_IDLE_INSERT_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        let base = game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let arg1_update_rva = if base == TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_OWNER_SCAN_START_ADDRESS
        } else {
            unsafe { task_node_update_rva(base, arg1 as usize) }
        };
        let ret_update_rva = if base == TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_OWNER_SCAN_START_ADDRESS
        } else {
            unsafe { task_node_update_rva(base, result as usize) }
        };
        MENU_CONTINUE_IDLE_INSERT_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
        MENU_CONTINUE_IDLE_INSERT_LAST_ARG0.store(arg0 as usize, Ordering::SeqCst);
        MENU_CONTINUE_IDLE_INSERT_LAST_ARG1.store(arg1 as usize, Ordering::SeqCst);
        MENU_CONTINUE_IDLE_INSERT_LAST_RET.store(result as usize, Ordering::SeqCst);
        MENU_CONTINUE_IDLE_INSERT_LAST_ARG1_UPDATE_RVA.store(arg1_update_rva, Ordering::SeqCst);
        MENU_CONTINUE_IDLE_INSERT_LAST_RET_UPDATE_RVA.store(ret_update_rva, Ordering::SeqCst);
        if hit <= CAP_MENU_INSERT_LOG_FIRST as u64 {
            append_continue_trace(format_args!(
                "MENU-CONTINUE-IDLE-INSERT seq={hit} caller_rva=0x{caller_rva:x} arg0={arg0:p} arg1={arg1:p} arg1_update_rva={} ret={result:p} ret_update_rva={} -- passive disabled Continue insert edge via 0x{:x}",
                format_optional_usize_hex(arg1_update_rva),
                format_optional_usize_hex(ret_update_rva),
                TRACE_TASK_ENQUEUE_RVA
            ));
        }
    }
    const RESULT_ACTION_BUILDER_TRACE_SIZE: usize = 0x360;
    if callstack_contains_game_rva(
        RESULT_ACTION_BUILDER_RVA as usize,
        RESULT_ACTION_BUILDER_RVA as usize + RESULT_ACTION_BUILDER_TRACE_SIZE,
    ) {
        let hit = RESULT_ACTION_INSERT_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
            + OWN_STEPPER_CALL_INC;
        RESULT_ACTION_LAST_INSERT_ARG0.store(arg0 as usize, Ordering::SeqCst);
        RESULT_ACTION_LAST_INSERT_ARG1.store(arg1 as usize, Ordering::SeqCst);
        RESULT_ACTION_LAST_INSERT_RET.store(result as usize, Ordering::SeqCst);
        let base = game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let arg1_update_rva = if base == TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_OWNER_SCAN_START_ADDRESS
        } else {
            unsafe { task_node_update_rva(base, arg1 as usize) }
        };
        let ret_update_rva = if base == TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_OWNER_SCAN_START_ADDRESS
        } else {
            unsafe { task_node_update_rva(base, result as usize) }
        };
        RESULT_ACTION_LAST_INSERT_ARG1_UPDATE_RVA.store(arg1_update_rva, Ordering::SeqCst);
        RESULT_ACTION_LAST_INSERT_RET_UPDATE_RVA.store(ret_update_rva, Ordering::SeqCst);
        if hit <= CAP_MENU_INSERT_LOG_FIRST {
            append_continue_trace(format_args!(
                "result_action_builder_insert seq={hit} arg0={arg0:p} arg1={arg1:p} arg1_update_rva={} ret={result:p} ret_update_rva={} -- passive downstream action node insert via 0x{:x}",
                format_optional_usize_hex(arg1_update_rva),
                format_optional_usize_hex(ret_update_rva),
                TRACE_TASK_ENQUEUE_RVA
            ));
        }
    }
    if let Ok(base) = game_module_base() {
        unsafe { capture_continue_task_node_candidate(base, arg1 as usize, "arg1") };
        unsafe { capture_continue_task_node_candidate(base, result as usize, "ret") };
        unsafe { capture_continue_member_node_candidate(base, arg1 as usize, "arg1") };
        unsafe { capture_continue_member_node_candidate(base, result as usize, "ret") };
    }
    unsafe {
        log_menu_insert_details(
            arg0 as usize,
            arg1 as usize,
            TITLE_OWNER_SCAN_START_ADDRESS,
            TITLE_OWNER_SCAN_START_ADDRESS,
            result as usize,
        );
    }
    if should_trace {
        append_continue_trace(format_args!(
            "menu_task_enqueue seq={trace_index} phase=LEAVE ret={result:p} ret_{} raw{{{}}} {}",
            unsafe { object_vtable_summary(result) },
            unsafe { task_node_raw_summary(result as usize) },
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
        TITLE_HANDOFF_COMPLETE.store(TITLE_HANDOFF_COMPLETE_VALUE, Ordering::SeqCst);
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
