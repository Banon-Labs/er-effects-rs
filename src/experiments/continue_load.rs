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

use crate::input_blocker::{InputBlocker, InputFlags};
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
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

pub(crate) unsafe fn product_continue_action_ready(
    ready: &ProductCoreAutoloadReady,
    base: usize,
    gm: usize,
    slot: i32,
) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if slot < OWN_STEPPER_SLOT_ZERO
        || gm == null
        || ready.menu_opened_latch == OWN_STEPPER_MENU_OPENED_NO
    {
        return false;
    }
    let dialog_vt = unsafe { safe_read_usize(ready.title_dialog) }.unwrap_or(null);
    dialog_vt == base + TITLE_TOP_DIALOG_VTABLE_RVA
}
pub(crate) fn record_continue_candidate(item: usize, accept_predicate: usize, base: usize) {
    const MENU_ITEM_ACCEPT_IDLE_RVA: usize = 0x007add70;
    const MENU_ITEM_ACCEPT_NATIVE_RVA: usize = 0x007ad810;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if item == null {
        return;
    }
    MENU_CONTINUE_CANDIDATE_HITS.fetch_add(1, Ordering::SeqCst);
    MENU_CONTINUE_CANDIDATE_ITEM.store(item, Ordering::SeqCst);
    let prior = MENU_CONTINUE_CANDIDATE_LAST_ACCEPT.swap(accept_predicate, Ordering::SeqCst);
    if prior != null && prior != accept_predicate {
        MENU_CONTINUE_CANDIDATE_ACCEPT_CHANGES.fetch_add(1, Ordering::SeqCst);
        append_continue_trace(format_args!(
            "MENU-CONTINUE-CANDIDATE accept predicate changed item=0x{item:x} prior=0x{prior:x} now=0x{accept_predicate:x}"
        ));
    }
    if base != null && accept_predicate == base + MENU_ITEM_ACCEPT_NATIVE_RVA {
        MENU_CONTINUE_CANDIDATE_NATIVE_ACCEPT_HITS.fetch_add(1, Ordering::SeqCst);
    } else if base != null && accept_predicate == base + MENU_ITEM_ACCEPT_IDLE_RVA {
        MENU_CONTINUE_CANDIDATE_IDLE_ACCEPT_HITS.fetch_add(1, Ordering::SeqCst);
    } else {
        MENU_CONTINUE_CANDIDATE_OTHER_ACCEPT_HITS.fetch_add(1, Ordering::SeqCst);
    }
}
pub(crate) unsafe fn product_continue_item_action(base: usize) -> Option<NativeContinueItemAction> {
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let item = MENU_CONTINUE_ITEM.load(Ordering::SeqCst);
    if item == null {
        let candidate = MENU_CONTINUE_CANDIDATE_ITEM.load(Ordering::SeqCst);
        if candidate != null {
            append_autoload_debug(format_args!(
                "product-core-autoload: ignoring diagnostic Continue candidate=0x{candidate:x}; waiting for semantic native-accept MENU_CONTINUE_ITEM instead"
            ));
        }
        return None;
    }
    let item_vt = unsafe { safe_read_usize(item) }?;
    if item_vt != base + MENU_WINDOW_JOB_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "product-core-autoload: native Continue MenuWindowJob rejected item=0x{item:x} vt=0x{item_vt:x} expected=0x{:x}",
            base + MENU_WINDOW_JOB_VTABLE_RVA
        ));
        return None;
    }
    let functor = unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }?;
    if functor == null {
        return None;
    }
    let functor_vt = unsafe { safe_read_usize(functor) }?;
    let do_call = unsafe { safe_read_usize(functor_vt + DOCALL_VTABLE_SLOT_10) }?;
    if do_call != base + MENU_TITLE_CONTINUE_DOCALL_RVA {
        append_autoload_debug(format_args!(
            "product-core-autoload: native Continue MenuWindowJob rejected item=0x{item:x} functor=0x{functor:x} docall=0x{do_call:x} expected=0x{:x}",
            base + MENU_TITLE_CONTINUE_DOCALL_RVA
        ));
        return None;
    }
    const MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET: usize = 0xf8;
    const MENU_ITEM_ACCEPT_IDLE_RVA: usize = 0x007add70;
    const MENU_ITEM_ACCEPT_NATIVE_RVA: usize = 0x007ad810;
    let accept_predicate = unsafe { safe_read_usize(item + MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET) }?;
    record_continue_candidate(item, accept_predicate, base);
    if accept_predicate == base + MENU_ITEM_ACCEPT_IDLE_RVA {
        append_autoload_debug(format_args!(
            "product-core-autoload: native Continue MenuWindowJob rejected item=0x{item:x} accept_predicate=0x{accept_predicate:x} (constant false idle predicate) -- not a semantic accept-ready Continue item"
        ));
        return None;
    }
    if accept_predicate != base + MENU_ITEM_ACCEPT_NATIVE_RVA {
        append_autoload_debug(format_args!(
            "product-core-autoload: native Continue MenuWindowJob rejected item=0x{item:x} accept_predicate=0x{accept_predicate:x} expected native accept predicate 0x{:x}",
            base + MENU_ITEM_ACCEPT_NATIVE_RVA
        ));
        return None;
    }
    if MENU_CONTINUE_ITEM
        .compare_exchange(
            TITLE_OWNER_SCAN_START_ADDRESS,
            item,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
    {
        append_autoload_debug(format_args!(
            "product-core-autoload: promoted candidate native Continue MenuWindowJob item=0x{item:x} accept_predicate=0x{accept_predicate:x}"
        ));
    }
    let result = unsafe { safe_read_usize(item + MENU_ITEM_DIALOG_RESULT_130_OFFSET) }?;
    if result == null {
        return None;
    }
    let result_vt = unsafe { safe_read_usize(result) }?;
    if !vtable_in_game_image(result_vt, base) {
        append_autoload_debug(format_args!(
            "product-core-autoload: native Continue MenuWindowJob rejected item=0x{item:x} result=0x{result:x} result_vt=0x{result_vt:x}"
        ));
        return None;
    }
    Some(NativeContinueItemAction {
        item,
        result,
        result_vt,
        functor,
        do_call,
    })
}
pub(crate) unsafe fn submit_native_continue_item_action(
    action: NativeContinueItemAction,
    base: usize,
) -> Option<i32> {
    const MENU_ITEM_RESULT_MODE_UNKNOWN: i32 = i32::MIN;
    let diagnostic_mode = unsafe { safe_read_i32(action.result + MENU_ITEM_RESULT_MODE_58_OFFSET) }
        .unwrap_or(MENU_ITEM_RESULT_MODE_UNKNOWN);
    let event_handler =
        unsafe { safe_read_usize(action.result_vt + MENU_ITEM_RESULT_EVENT_SLOT_60_OFFSET) }?;
    if !vtable_in_game_image(event_handler, base) {
        append_autoload_debug(format_args!(
            "product-core-autoload: native Continue submit ABI rejected item=0x{:x} result=0x{:x} result_vt=0x{:x} event_handler=0x{event_handler:x} diagnostic_mode={diagnostic_mode}",
            action.item, action.result, action.result_vt
        ));
        return None;
    }
    const CONTINUE_WRAPPER_EVENT_WORDS: usize = 2;
    const CONTINUE_WRAPPER_EVENT_CODE_INDEX: usize = 0;
    const CONTINUE_WRAPPER_EVENT_PAYLOAD_INDEX: usize = 1;
    let native_submit = base + MENU_ITEM_SUBMIT_RVA;
    let fd4_event_constructor = base + FD4_EVENT_CONSTRUCTOR_RVA;
    let native_submit_fn: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(native_submit) };
    append_autoload_debug(format_args!(
        "product-core-autoload: native Continue submit ABI proven item=0x{:x} result=0x{:x} result_vt=0x{:x} event_handler=0x{event_handler:x} native_submit=0x{native_submit:x} fd4_event_ctor=0x{fd4_event_constructor:x} diagnostic_mode={diagnostic_mode} -- result+0x58 logged only, never used as readiness",
        action.item, action.result, action.result_vt
    ));
    unsafe { native_submit_fn(action.result) };
    append_autoload_debug(format_args!(
        "product-core-autoload: native Continue submit dispatcher returned after event_handler=0x{event_handler:x} -- modal-confirm wait remains disabled downstream until loaded evidence"
    ));
    Some(diagnostic_mode)
}
pub(crate) unsafe fn product_continue_entry_action(
    owner: usize,
    base: usize,
) -> Option<NativeContinueEntry> {
    const ROUTER_CURSOR_OFFSET: usize = DIALOG_SLOT_CURSOR_B0C_OFFSET;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let (_, continue_entry, cursor) = unsafe { dump_titletop_menu_entries(owner, base) };
    let entry = continue_entry.unwrap_or_else(|| MENU_CONTINUE_ENTRY.load(Ordering::SeqCst));
    let mut functor = MENU_CONTINUE_FUNCTOR.load(Ordering::SeqCst);
    let mut do_call = MENU_CONTINUE_DOCALL.load(Ordering::SeqCst);
    let mut router = MENU_CONTINUE_ROUTER.load(Ordering::SeqCst);
    let mut index = MENU_CONTINUE_INDEX.load(Ordering::SeqCst);
    let mut entry = entry;
    if entry == null || functor == null || do_call == null || index == null {
        return None;
    }
    let do_call_vtable = unsafe { safe_read_usize(functor) }.unwrap_or(null);
    if do_call_vtable == null || !vtable_in_game_image(do_call_vtable, base) {
        append_autoload_debug(format_args!(
            "product-core-autoload: native Continue row rejected functor=0x{functor:x} vt=0x{do_call_vtable:x} entry=0x{entry:x}"
        ));
        return None;
    }
    let live_cursor = unsafe { safe_read_i32(router + ROUTER_CURSOR_OFFSET) }.unwrap_or(cursor);
    Some(NativeContinueEntry {
        entry,
        functor,
        do_call,
        router,
        index,
        cursor: live_cursor,
    })
}
pub(crate) unsafe fn captured_continue_task_node(base: usize) -> usize {
    let node = MENU_CONTINUE_TASK_NODE.load(Ordering::SeqCst);
    if node == TITLE_OWNER_SCAN_START_ADDRESS {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    let update_rva = unsafe { task_node_update_rva(base, node) };
    if update_rva != TRACE_MENU_CONTINUE_WRAPPER_RVA as usize {
        append_autoload_debug(format_args!(
            "product-core-autoload: captured Continue task node 0x{node:x} rejected update_rva=0x{update_rva:x} expected=0x{:x}",
            TRACE_MENU_CONTINUE_WRAPPER_RVA as usize
        ));
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    node
}
pub(crate) unsafe fn drive_product_continue_post_click_dispatchers(base: usize, slot: i32) {
    let synth = &raw mut SYNTH_MMS_OWNER as *mut u8;
    unsafe {
        *synth.add(SYNTH_MMS_SKIP_APPLY_12A_OFFSET) = SYNTH_MMS_SKIP_APPLY_ON;
        *(synth.add(SYNTH_MMS_DESER_SLOT_12C_OFFSET) as *mut i32) = slot;
    }
    let synth_ptr = synth as usize;
    let dispatcher1: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + B80_DISPATCHER1_RVA) };
    let dispatcher2: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + B80_DISPATCHER2_RVA) };
    unsafe { dispatcher1(synth_ptr) };
    unsafe { dispatcher2(synth_ptr) };
}
pub(crate) unsafe fn product_continue_autoload_tick(
    owner: usize,
    base: usize,
    gm: usize,
    slot: i32,
    tick: u64,
    ready: &ProductCoreAutoloadReady,
) {
    const PRODUCT_CONTINUE_C30_ZERO: i32 = 0;
    const PRODUCT_CONTINUE_B80_MODAL_WAIT: i32 = 1;
    const PRODUCT_CONTINUE_NEW_GAME_BLOCKED: u8 = 1;
    const PRODUCT_CONTINUE_WAIT_LOG_TICKS: u64 = 30;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let phase = FULLREAD_PHASE.load(Ordering::SeqCst);
    let read_i32 = |off: usize| unsafe { safe_read_i32(gm + off) }.unwrap_or(GAME_MAN_C30_UNSET);

    if phase == FULLREAD_PHASE_DONE {
        return;
    }

    if phase == FULLREAD_PHASE_SUBMIT {
        // SWITCH-SAFETY (System->Quit->Load-Profile): for the in-world character switch (not a boot
        // autoload), the return-title chain we submitted is still tearing down the OLD world. Firing
        // the Continue-load now sets GameMan saveState/b80=2 and DoSaveStuff deserializes the picked
        // slot INTO the still-live world -> crash in CSGaitemImp::Deserialize (live 0x67141a). Defer
        // until the old world is actually gone (local player absent), so the load runs at a clean
        // title exactly like the boot autoload does. The boot path has no System-Quit phase, and at a
        // fresh title there is no local player, so this gate passes immediately there.
        // See bd system-quit-load-profile-trigger-RESOLVED.
        if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst) != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE
            && unsafe { PlayerIns::local_player_mut() }.is_ok()
        {
            if tick % PRODUCT_CONTINUE_WAIT_LOG_TICKS == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: SWITCH deferring Continue-load until old world torn down -- local player still present slot={slot} tick={tick}"
                ));
            }
            return;
        }
        if !unsafe { product_continue_action_ready(ready, base, gm, slot) } {
            if tick % PRODUCT_CONTINUE_WAIT_LOG_TICKS == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: Continue submit gated off dialog=0x{:x} menu_latch={} slot={slot} -- semantic menu readiness not stable",
                    ready.title_dialog, ready.menu_opened_latch
                ));
            }
            return;
        }
        let b80_before = read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
        if b80_before != OWN_STEPPER_B80_IDLE {
            if tick % PRODUCT_CONTINUE_WAIT_LOG_TICKS == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: waiting for native preview/load b80={b80_before} to become idle before Continue row fire -- no SetState5"
                ));
            }
            return;
        }
        let (profile_real, profile_map, profile_level, profile_name_len) =
            unsafe { profile_slot_fingerprint(slot) };
        if !profile_real {
            if tick % PRODUCT_CONTINUE_WAIT_LOG_TICKS == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: Continue slot profile is empty-like (slot={slot} map=0x{profile_map:x} level={profile_level} name_len={profile_name_len}); fail-closed with no native Load Game fallback, no legal-popup auto-accept, no Continue submit, and no input"
                ));
            }
            return;
        }
        let Some(action) = (unsafe { product_continue_item_action(base) }) else {
            if tick % PRODUCT_CONTINUE_WAIT_LOG_TICKS == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: waiting for native Continue MenuWindowJob result after open-menu dialog=0x{:x} slot={slot} -- no direct_load/direct_build/input fallback",
                    ready.title_dialog
                ));
            }
            return;
        };
        unsafe { *((gm + GAME_MAN_SLOT_SELECT_B78_OFFSET) as *mut i32) = slot };
        let set_save_slot: unsafe extern "system" fn(i32) =
            unsafe { std::mem::transmute(base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
        unsafe { set_save_slot(slot) };
        OWN_STEPPER_EXPECTED_SLOT.store(slot, Ordering::SeqCst);
        OWN_STEPPER_CONFIRMED.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
        OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_NOT_FIRED, Ordering::SeqCst);
        OWN_STEPPER_MOUNT_C30.store(GAME_MAN_C30_UNSET, Ordering::SeqCst);
        OWN_STEPPER_IO_WAS_SET.store(OWN_STEPPER_IO_WAS_SET_NO, Ordering::SeqCst);
        let Some(result_mode) = (unsafe { submit_native_continue_item_action(action, base) })
        else {
            return;
        };
        let b80 = read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
        let ac0 = read_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
        let b78 = read_i32(GAME_MAN_SLOT_SELECT_B78_OFFSET);
        let c30 = read_i32(GAME_MAN_SAVED_MAP_C30_OFFSET);
        let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
        append_autoload_debug(format_args!(
            "product-core-autoload: *** SUBMITTED native Continue MenuWindowJob result mode={result_mode} submit=0x{:x}(result=0x{:x}, result_vt=0x{:x}, item=0x{:x}, functor=0x{:x}, docall=0x{:x}) after set_save_slot({slot}) b78={b78} ac0={ac0} c30=0x{c30:x} b80={b80} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) dialog=0x{:x} menu_latch={} tick={tick} -- no input/direct_load/direct_build/raw deserialize/direct_confirm ***",
            base + MENU_ITEM_SUBMIT_RVA,
            action.result,
            action.result_vt,
            action.item,
            action.functor,
            action.do_call,
            ready.title_dialog,
            ready.menu_opened_latch
        ));
        timeline_event(
            "T_native_continue_action",
            tick,
            format_args!(
                "slot={slot} item=0x{:x} result=0x{:x} b80={b80}",
                action.item, action.result
            ),
        );
        FULLREAD_DRAIN_WAITS.store(null, Ordering::SeqCst);
        FULLREAD_PHASE.store(FULLREAD_PHASE_GUARD, Ordering::SeqCst);
        return;
    }

    if phase == FULLREAD_PHASE_GUARD {
        let expected = OWN_STEPPER_EXPECTED_SLOT.load(Ordering::SeqCst);
        let ac0 = read_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
        let c30 = read_i32(GAME_MAN_SAVED_MAP_C30_OFFSET);
        let b80 = read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
        let latched = OWN_STEPPER_MOUNT_C30.load(Ordering::SeqCst);
        let deser_ok = OWN_STEPPER_DESER_FIRED.load(Ordering::SeqCst) == OWN_STEPPER_DESER_FIRED_OK;
        let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
        let slot_identity = unsafe { requested_slot_identity(expected, c30) };
        let waits = FULLREAD_DRAIN_WAITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
        let c30_available =
            c30 == latched && c30 != GAME_MAN_C30_UNSET && c30 != PRODUCT_CONTINUE_C30_ZERO;
        let c30_sane = c30_available && (c30 != GAME_MAN_NEWGAME_DEFAULT_MAP || fp_real);
        let c30_loaded = c30 != GAME_MAN_C30_UNSET && c30 != PRODUCT_CONTINUE_C30_ZERO;
        let c30_loaded_sane = c30_loaded && (c30 != GAME_MAN_NEWGAME_DEFAULT_MAP || fp_real);
        let new_game_flag =
            unsafe { safe_read_usize(owner + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET) }
                .map(|v| v as u8)
                .unwrap_or(PRODUCT_CONTINUE_NEW_GAME_BLOCKED);
        let commit = native_fullread_commit_enabled();
        let b80_idle = b80 == OWN_STEPPER_B80_IDLE;
        let b80_modal_wait = b80 == PRODUCT_CONTINUE_B80_MODAL_WAIT;
        let native_confirmed =
            OWN_STEPPER_CONFIRMED.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS;
        let modal_disable_ready = commit
            && !native_confirmed
            && b80_modal_wait
            && fp_real
            && slot_identity.matches
            && ac0 == expected
            && expected != OWN_STEPPER_SLOT_NONE
            && c30_loaded_sane
            && new_game_flag == FULLREAD_OWNER_NEW_GAME_OK;
        if modal_disable_ready {
            let shim = &raw mut OWN_STEPPER_SHIM;
            unsafe { (*shim)[OWN_STEPPER_SHIM_OWNER_IDX] = owner };
            let shim_ptr = shim as usize;
            let confirm: unsafe extern "system" fn(usize) =
                unsafe { std::mem::transmute(base + CONTINUE_CONFIRM_RVA) };
            append_autoload_debug(format_args!(
                "product-core-autoload: MODAL-CONFIRM-DISABLED loaded evidence ac0={ac0} expected={expected} c30=0x{c30:x} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) slot_identity=true(profile=0x{:x} profile_map=0x{:x} profile_level={} profile_name_len={}) b80={b80} owner+0x284={new_game_flag} -> continue_confirm shim=0x{shim_ptr:x} owner=0x{owner:x} (no confirm input)",
                slot_identity.profile_summary,
                slot_identity.profile_map,
                slot_identity.profile_level,
                slot_identity.profile_name_len
            ));
            timeline_event(
                "T_modal_confirm_disabled",
                tick,
                format_args!("ac0={ac0} c30=0x{c30:x} b80={b80}"),
            );
            unsafe { confirm(shim_ptr) };
            OWN_STEPPER_CONFIRMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "product-core-autoload: STAGE2-SETSTATE5 fired via disabled modal confirm owner=0x{owner:x} -- native pump now streams the real world"
            ));
        }
        let native_confirmed =
            OWN_STEPPER_CONFIRMED.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS;
        let proceed = commit
            && (deser_ok || modal_disable_ready)
            && native_confirmed
            && fp_real
            && slot_identity.matches
            && ac0 == expected
            && expected != OWN_STEPPER_SLOT_NONE
            && (c30_sane || c30_loaded_sane)
            && (b80_idle || modal_disable_ready)
            && new_game_flag == FULLREAD_OWNER_NEW_GAME_OK;
        if waits % PRODUCT_CONTINUE_WAIT_LOG_TICKS == null as u64 || proceed {
            append_autoload_debug(format_args!(
                "product-core-autoload: Continue post-click GUARD waits={waits} commit={commit} deser_ok={deser_ok} native_confirmed={native_confirmed} ac0={ac0} expected={expected} c30=0x{c30:x} latched=0x{latched:x} c30_sane={c30_sane} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) slot_identity={} profile=0x{:x} profile_map=0x{:x} profile_level={} profile_name_len={} pgd_level={} pgd_name_len={} owner+0x284={new_game_flag} b80={b80} proceed={proceed} -- waiting for requested-slot native b80/c30 writer + native continue_confirm/SetState5",
                slot_identity.matches,
                slot_identity.profile_summary,
                slot_identity.profile_map,
                slot_identity.profile_level,
                slot_identity.profile_name_len,
                slot_identity.pgd_level,
                slot_identity.pgd_name_len
            ));
        }
        if !proceed {
            if waits >= FULLREAD_DRAIN_MAX {
                append_autoload_debug(format_args!(
                    "product-core-autoload: Continue post-click GUARD timeout waits={waits} commit={commit} deser_ok={deser_ok} ac0={ac0} expected={expected} c30=0x{c30:x} latched=0x{latched:x} c30_sane={c30_sane} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) slot_identity={} profile=0x{:x} profile_map=0x{:x} profile_level={} profile_name_len={} pgd_level={} pgd_name_len={} owner+0x284={new_game_flag} b80={b80} -- DONE (NO SetState5)",
                    slot_identity.matches,
                    slot_identity.profile_summary,
                    slot_identity.profile_map,
                    slot_identity.profile_level,
                    slot_identity.profile_name_len,
                    slot_identity.pgd_level,
                    slot_identity.pgd_name_len
                ));
                FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            return;
        }
        append_autoload_debug(format_args!(
            "product-core-autoload: STAGE2-MOUNT-COMMIT native Continue row guard pass ac0={ac0} expected={expected} c30=0x{c30:x} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) slot_identity=true owner+0x284={new_game_flag} b80={b80} -- native continue_confirm/SetState5 already fired"
        ));
        timeline_event("T_playgame", tick, format_args!("ac0={ac0} c30=0x{c30:x}"));
        FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
        OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
    }
}
pub(crate) unsafe fn fire_product_title_load_action(
    action: MenuActionNode,
    base: usize,
    tick: u64,
    slot: i32,
) {
    if OWN_STEPPER_TITLE_FIRED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let node = action.node;
    let node_vt = action.node_vt;
    let member_dialog = action.member_dialog;
    let member_fn = action.member_fn;
    let member_adjust = action.member_adjust;
    let window_item = action.window_item;
    OWN_STEPPER_EXPECTED_SLOT.store(slot, Ordering::SeqCst);
    OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_NOT_FIRED, Ordering::SeqCst);
    OWN_STEPPER_MOUNT_C30.store(GAME_MAN_C30_UNSET, Ordering::SeqCst);
    OWN_STEPPER_IO_WAS_SET.store(OWN_STEPPER_IO_WAS_SET_NO, Ordering::SeqCst);
    OWN_STEPPER_DIALOG.store(null, Ordering::SeqCst);
    OWN_STEPPER_SELECTOR_STEP.store(null, Ordering::SeqCst);
    OWN_STEPPER_SELECTOR_CTX.store(null, Ordering::SeqCst);
    reset_phase_timer(&OWN_STEPPER_S2_PHASE_STARTED_MS);
    let run: unsafe extern "system" fn(usize) = unsafe {
        std::mem::transmute::<usize, unsafe extern "system" fn(usize)>(
            base + MENU_MEMBER_FUNC_JOB_RUN_RVA,
        )
    };
    append_autoload_debug(format_args!(
        "product-core-autoload: *** FIRING native TitleTopDialog Load-Game run 0x{:x}(rcx=node=0x{node:x}) vt=0x{node_vt:x} member_dialog=0x{member_dialog:x} member_fn=0x{member_fn:x} member_adjust=0x{member_adjust:x} window_item=0x{window_item:x} slot={slot} tick={tick} -- no direct_build/forged ctx ***",
        base + MENU_MEMBER_FUNC_JOB_RUN_RVA
    ));
    timeline_event(
        "T_native_load_action",
        tick,
        format_args!("node=0x{node:x} member_fn=0x{member_fn:x}"),
    );
    unsafe { run(node) };
    append_autoload_debug(format_args!(
        "product-core-autoload: native TitleTopDialog Load-Game run returned; waiting for ProfileLoadDialog factory hook capture"
    ));
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
pub(crate) unsafe fn menu_input_probe(owner: usize, base: usize) {
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
/// It ONLY: (1) read-only checks whether the live TitleTopDialog menu/action is rendered and
/// semantically validated (TitleTopDialog vtable, [dialog+0xa48] registry, Load-Game
/// MenuMemberFuncJob node/action chain); (2) ONE-SHOT: fires that native run
/// MENU_MEMBER_FUNC_JOB_RUN_RVA (0x1409aaba0, rcx=node) -- which builds the LIVE registered
/// ProfileLoadDialog the native pump drives. After firing it observes (the caller keeps writing the
/// golden oracle as the native pump hopefully loads the char). Pure read-only until the single fire.
unsafe fn seed_profile_summary_slot_from_staged_save(
    base: usize,
    profile_summary: usize,
    slot: i32,
) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET: usize = 0x8;
    const PROFILE_SUMMARY_SLOT_DATA_OFFSET: usize = 0x18;
    const PROFILE_SUMMARY_SLOT_STRIDE: usize = 0x2a0;
    const SAVE_BODY_PLAYER_GAME_DATA_OFFSET: usize = 0xebae;
    const PROFILE_SUMMARY_NAME_BYTES: usize = 0x22;
    const PROFILE_SUMMARY_LEVEL_OFFSET: usize = 0x24;
    const PROFILE_SUMMARY_PLAYTIME_OFFSET: usize = 0x28;
    const PROFILE_SUMMARY_RUNE_MEMORY_OFFSET: usize = 0x2c;
    /// Native ProfileSummary slot layout: `FaceData` wrapper at slot+0x38; its inner
    /// `FaceDataBuffer` (`FACE` magic) starts at slot+0x40. 2026-06-27 native row dumps showed
    /// the staged SL2 inner `FaceDataBuffer` bytes match the native row exactly, but the saved
    /// `FaceData` wrapper header does not. Mirror `FUN_14025f9b0`: call
    /// `FaceData::CopyFromBuffer` instead of memcpy'ing the saved wrapper over the live slot.
    const PROFILE_SUMMARY_FACE_DATA_OFFSET: usize = 0x38;
    const FACE_DATA_COPY_FROM_BUFFER_RVA: usize = 0x00252f70;
    /// Native row builder passes slot+0x1a8 to the equipment renderer. Mirror `FUN_14025f9b0`
    /// by copying the saved `PlayerGameData.equipment.chr_asm` through the same native helper instead
    /// of leaving a zero/default `ChrAsm` that only proves renderer plumbing.
    const PROFILE_SUMMARY_CHR_ASM_OFFSET: usize = 0x1a8;
    const CHR_ASM_COPY_RVA: usize = 0x00245c00;
    const PROFILE_SUMMARY_GENDER_OFFSET: usize = 0x290;
    const PROFILE_SUMMARY_ARCHETYPE_OFFSET: usize = 0x291;
    const PROFILE_SUMMARY_STARTING_GIFT_OFFSET: usize = 0x292;
    const PROFILE_SUMMARY_FIELD_C4_OFFSET: usize = 0x293;
    if profile_summary <= NULL
        || slot < OWN_STEPPER_SLOT_ZERO
        || slot as usize >= TITLE_PROFILE_SLOT_COUNT
    {
        return false;
    }
    let Ok(save_path) = std::env::var("ER_EFFECTS_SAVE_FILE") else {
        append_autoload_debug(format_args!(
            "native-profile-capture: staged ProfileSummary seed unavailable -- ER_EFFECTS_SAVE_FILE unset"
        ));
        return false;
    };
    let Ok(mut save_bytes) = fs::read(&save_path) else {
        append_autoload_debug(format_args!(
            "native-profile-capture: staged ProfileSummary seed failed to read '{save_path}'"
        ));
        return false;
    };
    normalize_save_bytes_to_active_steam_id(base, &mut save_bytes, "native-profile-capture-seed");
    let Ok(body) = er_save_loader::bnd4::slot_body(&save_bytes, slot as usize) else {
        append_autoload_debug(format_args!(
            "native-profile-capture: staged ProfileSummary seed failed to locate USER_DATA{slot:03} in '{save_path}'"
        ));
        return false;
    };
    let min_name_len =
        SAVE_BODY_PLAYER_GAME_DATA_OFFSET + PGD_NAME_9C_OFFSET + PROFILE_SUMMARY_NAME_BYTES;
    let min_face_len = SAVE_BODY_PLAYER_GAME_DATA_OFFSET
        + PGD_FACE_DATA_OFFSET
        + FACE_DATA_BUFFER_OFFSET
        + FACE_DATA_BUFFER_TOTAL_SIZE;
    let min_chr_asm_len = SAVE_BODY_PLAYER_GAME_DATA_OFFSET
        + PGD_EQUIP_GAME_DATA_OFFSET
        + EQUIP_GAME_DATA_CHR_ASM_OFFSET
        + CHR_ASM_SIZE;
    if body.len() < min_name_len || body.len() < min_face_len || body.len() < min_chr_asm_len {
        append_autoload_debug(format_args!(
            "native-profile-capture: staged ProfileSummary seed body too short len={} for PGD offset 0x{SAVE_BODY_PLAYER_GAME_DATA_OFFSET:x} required_name=0x{min_name_len:x} required_face=0x{min_face_len:x} required_chr_asm=0x{min_chr_asm_len:x}",
            body.len()
        ));
        return false;
    }
    let pgd = body
        .as_ptr()
        .wrapping_add(SAVE_BODY_PLAYER_GAME_DATA_OFFSET) as usize;
    let slot_data = profile_summary
        + PROFILE_SUMMARY_SLOT_DATA_OFFSET
        + slot as usize * PROFILE_SUMMARY_SLOT_STRIDE;
    unsafe {
        core::ptr::write_bytes(slot_data as *mut u8, 0, PROFILE_SUMMARY_SLOT_STRIDE);
        core::ptr::copy_nonoverlapping(
            (pgd + PGD_NAME_9C_OFFSET) as *const u8,
            slot_data as *mut u8,
            PROFILE_SUMMARY_NAME_BYTES,
        );
        *(slot_data.wrapping_add(PROFILE_SUMMARY_LEVEL_OFFSET) as *mut i32) =
            *((pgd + PGD_LEVEL_68_OFFSET) as *const i32);
        *(slot_data.wrapping_add(PROFILE_SUMMARY_PLAYTIME_OFFSET) as *mut u32) = 0;
        *(slot_data.wrapping_add(PROFILE_SUMMARY_RUNE_MEMORY_OFFSET) as *mut i32) =
            *((pgd + PGD_RUNE_MEMORY_70_OFFSET) as *const i32);
        let copy_face_data_from_buffer: unsafe extern "system" fn(usize, usize) =
            std::mem::transmute(base + FACE_DATA_COPY_FROM_BUFFER_RVA);
        let copy_chr_asm: unsafe extern "system" fn(usize, usize) -> usize =
            std::mem::transmute(base + CHR_ASM_COPY_RVA);
        copy_face_data_from_buffer(
            slot_data.wrapping_add(PROFILE_SUMMARY_FACE_DATA_OFFSET),
            pgd + PGD_FACE_DATA_OFFSET + FACE_DATA_BUFFER_OFFSET,
        );
        copy_chr_asm(
            slot_data.wrapping_add(PROFILE_SUMMARY_CHR_ASM_OFFSET),
            pgd + PGD_EQUIP_GAME_DATA_OFFSET + EQUIP_GAME_DATA_CHR_ASM_OFFSET,
        );
        *(slot_data.wrapping_add(PROFILE_SUMMARY_GENDER_OFFSET) as *mut u8) =
            *((pgd + PGD_GENDER_BE_OFFSET) as *const u8);
        *(slot_data.wrapping_add(PROFILE_SUMMARY_ARCHETYPE_OFFSET) as *mut u8) =
            *((pgd + PGD_ARCHETYPE_BF_OFFSET) as *const u8);
        *(slot_data.wrapping_add(PROFILE_SUMMARY_STARTING_GIFT_OFFSET) as *mut u8) =
            *((pgd + PGD_STARTING_GIFT_C3_OFFSET) as *const u8);
        *(slot_data.wrapping_add(PROFILE_SUMMARY_FIELD_C4_OFFSET) as *mut u8) =
            *((pgd + 0xc4) as *const u8);
        *(profile_summary.wrapping_add(PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + slot as usize)
            as *mut u8) = 1;
    }
    let level = unsafe { *((slot_data + PROFILE_SUMMARY_LEVEL_OFFSET) as *const i32) };
    append_autoload_debug(format_args!(
        "native-profile-capture: staged ProfileSummary seed wrote slot={slot} from '{save_path}' pgd_off=0x{SAVE_BODY_PLAYER_GAME_DATA_OFFSET:x} slot_data=0x{slot_data:x} level={level} (scalar + native FaceData::CopyFromBuffer + native ChrAsm copy)"
    ));
    true
}

pub(crate) unsafe fn native_load_tick(owner: usize, base: usize, n: u64) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    if native_profile_capture_enabled() {
        if NATIVE_LOAD_FIRED.load(Ordering::SeqCst) != NATIVE_LOAD_FIRED_NO {
            unsafe { sample_title_profile_portrait_source(base, OWN_STEPPER_SLOT_ZERO) };
            return;
        }
        let Some((title_dialog, _menu_window)) =
            (unsafe { locate_live_loadgame_node(owner, base) })
        else {
            if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
                append_autoload_debug(format_args!(
                    "native-profile-capture: waiting for live TitleTopDialog/ProfileSelect builder context (#{n})"
                ));
            }
            return;
        };
        const MENU_SYSTEM_SAVE_LOAD_GETTER_RVA: usize = 0x00256360;
        const GET_PROFILE_SUMMARY_RVA: usize = 0x002567b0;
        const MARK_PROFILE_INDEX_AS_USED_RVA: usize = 0x00262250;
        const NATIVE_LOAD_SAVE_DATA_RVA: usize = 0x0067b200;
        const TITLE_FLOW_CONTEXT_SAVE_INIT_RVA: usize = 0x0082d0d0;
        const MENU_SYSTEM_SAVE_SLOT_OFFSET: usize = 0x1200;
        const PROFILE_SELECT_JOB_BUILDER_RVA: usize = 0x009ad0e0;
        const MENU_JOB_QUEUE_RVA: usize = 0x007a9250;
        const TITLE_MENU_WINDOW_JOB_QUEUE_OFFSET: usize = 0x10;
        let capture =
            unsafe { safe_read_usize(title_dialog + DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET) }
                .unwrap_or(NULL);
        let title_window_base = title_dialog + 0x50;
        let mut save_init_flag: u8 = 0;
        let get_menu_system_save_load: unsafe extern "system" fn() -> usize =
            unsafe { std::mem::transmute(base + MENU_SYSTEM_SAVE_LOAD_GETTER_RVA) };
        let get_profile_summary: unsafe extern "system" fn() -> usize =
            unsafe { std::mem::transmute(base + GET_PROFILE_SUMMARY_RVA) };
        let mark_profile_index_as_used: unsafe extern "system" fn(usize, i32) -> u8 =
            unsafe { std::mem::transmute(base + MARK_PROFILE_INDEX_AS_USED_RVA) };
        let native_load_save_data: unsafe extern "system" fn(u32) -> usize =
            unsafe { std::mem::transmute(base + NATIVE_LOAD_SAVE_DATA_RVA) };
        let set_save_slot: unsafe extern "system" fn(i32) =
            unsafe { std::mem::transmute(base + er_save_loader::SET_SAVE_SLOT_RVA as usize) };
        let request_save: unsafe extern "system" fn(u8) =
            unsafe { std::mem::transmute(base + er_save_loader::REQUEST_SAVE_RVA as usize) };
        let save_request_profile: unsafe extern "system" fn(u8) = unsafe {
            std::mem::transmute(base + er_save_loader::SAVE_REQUEST_PROFILE_RVA as usize)
        };
        const GET_SAVE_SYSTEM_RVA: usize = 0x00e6e060;
        let get_save_system: unsafe extern "system" fn() -> usize =
            unsafe { std::mem::transmute(base + GET_SAVE_SYSTEM_RVA) };
        const NATIVE_LOAD_SAVE_DATA_POLL_RVA: usize = 0x00679180;
        const PROFILE_SUMMARY_POPULATE_SLOT_RVA: usize = 0x00262270;
        static NATIVE_PROFILE_READ_PHASE: AtomicUsize = AtomicUsize::new(0);
        static NATIVE_PROFILE_READ_LAST_POLL_STATUS: AtomicUsize = AtomicUsize::new(usize::MAX);
        let poll_save_load: unsafe extern "system" fn(u8, u32) -> i32 =
            unsafe { std::mem::transmute(base + NATIVE_LOAD_SAVE_DATA_POLL_RVA) };
        let populate_profile_summary_slot: unsafe extern "system" fn(usize, u32) -> usize =
            unsafe { std::mem::transmute(base + PROFILE_SUMMARY_POPULATE_SLOT_RVA) };
        let init_title_flow_context: unsafe extern "system" fn(usize, *mut u8, usize) =
            unsafe { std::mem::transmute(base + TITLE_FLOW_CONTEXT_SAVE_INIT_RVA) };
        let menu_system_save_load = unsafe { get_menu_system_save_load() };
        let profile_summary = unsafe { get_profile_summary() };
        let read_phase = NATIVE_PROFILE_READ_PHASE.load(Ordering::SeqCst);
        if read_phase == 0 {
            unsafe {
                set_save_slot(OWN_STEPPER_SLOT_ZERO);
                request_save(1);
                save_request_profile(1);
            }
            let marked_profile = if profile_summary != NULL {
                unsafe { mark_profile_index_as_used(profile_summary, OWN_STEPPER_SLOT_ZERO) }
            } else {
                0
            };
            let save_system_before = unsafe { get_save_system() };
            let native_read_requested = if profile_summary != NULL && marked_profile != 0 {
                unsafe { native_load_save_data(OWN_STEPPER_SLOT_ZERO as u32) }
            } else {
                0
            };
            let save_system_after = unsafe { get_save_system() };
            let read_ctx = unsafe { safe_read_usize(save_system_after + 0x18) }.unwrap_or(NULL);
            let read_job = unsafe { safe_read_usize(save_system_after + 0x20) }.unwrap_or(NULL);
            let read_slot = unsafe { safe_read_i32(save_system_after + 0x34) }.unwrap_or(-1);
            append_autoload_debug(format_args!(
                "native-profile-capture: phase0 SET_SLOT/REQUESTS + MARK/QUEUE native save read profile_summary=0x{profile_summary:x} marked={marked_profile} read_ret=0x{native_read_requested:x} save_sys_before=0x{save_system_before:x} save_sys_after=0x{save_system_after:x} handles[+18]=0x{read_ctx:x} [+20]=0x{read_job:x} slot_field=0x{read_slot:x} via 0x{:x} #{n}",
                base + NATIVE_LOAD_SAVE_DATA_RVA
            ));
            if native_read_requested != 0 {
                NATIVE_PROFILE_READ_PHASE.store(1, Ordering::SeqCst);
            }
            return;
        }
        let poll_status = unsafe { poll_save_load(0, 0) };
        let poll_status_key = poll_status as isize as usize;
        let last_poll_status =
            NATIVE_PROFILE_READ_LAST_POLL_STATUS.swap(poll_status_key, Ordering::SeqCst);
        if poll_status_key != last_poll_status || n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            let save_system = unsafe { get_save_system() };
            let read_ctx = unsafe { safe_read_usize(save_system + 0x18) }.unwrap_or(NULL);
            let read_job = unsafe { safe_read_usize(save_system + 0x20) }.unwrap_or(NULL);
            append_autoload_debug(format_args!(
                "native-profile-capture: phase1 native save read poll 0x{:x}(false,0) -> {poll_status} profile_summary=0x{profile_summary:x} handles[+18]=0x{read_ctx:x} [+20]=0x{read_job:x} #{n}",
                base + NATIVE_LOAD_SAVE_DATA_POLL_RVA
            ));
        }
        let seeded_from_staged_save = if poll_status == 0 {
            false
        } else if poll_status == 5 {
            unsafe {
                seed_profile_summary_slot_from_staged_save(
                    base,
                    profile_summary,
                    OWN_STEPPER_SLOT_ZERO,
                )
            }
        } else {
            false
        };
        if poll_status != 0 && !seeded_from_staged_save {
            return;
        }
        let populate_ret = if seeded_from_staged_save {
            1
        } else if profile_summary != NULL {
            unsafe { populate_profile_summary_slot(profile_summary, OWN_STEPPER_SLOT_ZERO as u32) }
        } else {
            0
        };
        if NATIVE_LOAD_FIRED.swap(NATIVE_LOAD_FIRED_YES, Ordering::SeqCst) != NATIVE_LOAD_FIRED_NO {
            return;
        }
        if capture != NULL && menu_system_save_load != NULL {
            unsafe {
                init_title_flow_context(
                    capture,
                    &mut save_init_flag as *mut u8,
                    menu_system_save_load + MENU_SYSTEM_SAVE_SLOT_OFFSET,
                )
            };
        }
        let mut job_ref: usize = NULL;
        let build_profile_select: unsafe extern "system" fn(
            usize,
            *mut usize,
            usize,
        ) -> *mut usize = unsafe { std::mem::transmute(base + PROFILE_SELECT_JOB_BUILDER_RVA) };
        let queue_job: unsafe extern "system" fn(usize, *mut usize) =
            unsafe { std::mem::transmute(base + MENU_JOB_QUEUE_RVA) };
        append_autoload_debug(format_args!(
            "native-profile-capture: *** SAVE READ COMPLETE poll=0 populate_ret=0x{populate_ret:x} profile_summary=0x{profile_summary:x}; INIT TFC 0x{:x}(capture=0x{capture:x}, flag={}, mss+0x1200=0x{:x}) then BUILD native ProfileSelect job 0x{:x}(title_dialog=0x{title_dialog:x}, out=&job_ref, title_dialog+0x50=0x{title_window_base:x}) then queue 0x{:x}(title_dialog+0x10, &job_ref) #{n} -- native 05_010_ProfileSelect path, no title accept/Continue ***",
            base + TITLE_FLOW_CONTEXT_SAVE_INIT_RVA,
            save_init_flag,
            menu_system_save_load + MENU_SYSTEM_SAVE_SLOT_OFFSET,
            base + PROFILE_SELECT_JOB_BUILDER_RVA,
            base + MENU_JOB_QUEUE_RVA
        ));
        unsafe {
            build_profile_select(title_dialog, &mut job_ref as *mut usize, title_window_base)
        };
        NATIVE_LOAD_LAST_NODE.store(job_ref, Ordering::SeqCst);
        NATIVE_LOAD_LAST_NODE_VTABLE.store(
            unsafe { safe_read_usize(job_ref) }.unwrap_or(NULL),
            Ordering::SeqCst,
        );
        NATIVE_LOAD_LAST_MEMBER_DIALOG.store(title_dialog, Ordering::SeqCst);
        NATIVE_LOAD_LAST_MEMBER_FN.store(base + PROFILE_SELECT_JOB_BUILDER_RVA, Ordering::SeqCst);
        NATIVE_LOAD_LAST_MEMBER_ADJUST.store(title_window_base, Ordering::SeqCst);
        if job_ref != NULL {
            unsafe {
                queue_job(
                    title_dialog + TITLE_MENU_WINDOW_JOB_QUEUE_OFFSET,
                    &mut job_ref as *mut usize,
                )
            };
        }
        unsafe { sample_title_profile_portrait_source(base, OWN_STEPPER_SLOT_ZERO) };
        return;
    }
    // Already fired: keep observing (oracle written by the caller's pass-through telemetry).
    if NATIVE_LOAD_FIRED.load(Ordering::SeqCst) != NATIVE_LOAD_FIRED_NO {
        unsafe { sample_title_profile_portrait_source(base, OWN_STEPPER_SLOT_ZERO) };
        if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            append_autoload_debug(format_args!(
                "native-load: FIRED -- observing native pump/profile renderer (#{n}); golden oracle written via telemetry"
            ));
        }
        return;
    }
    let Some(action) = (unsafe { title_menu_action_ready(owner, base) }) else {
        if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            append_autoload_debug(format_args!(
                "native-load: waiting for semantic Load-Game action readiness (#{n}) -- TitleTopDialog/registry/node/action not all validated yet"
            ));
        }
        return;
    };
    // ONE-SHOT fire. The semantic readiness helper already validated the node vtable, registry,
    // member fn, and factory chain; latch only after that validation succeeds.
    if NATIVE_LOAD_FIRED.swap(NATIVE_LOAD_FIRED_YES, Ordering::SeqCst) != NATIVE_LOAD_FIRED_NO {
        return;
    }
    let node = action.node;
    let node_vt = action.node_vt;
    let m_dlg = action.member_dialog;
    let m_fn = action.member_fn;
    let m_adj = action.member_adjust;
    NATIVE_LOAD_LAST_NODE.store(node, Ordering::SeqCst);
    NATIVE_LOAD_LAST_NODE_VTABLE.store(node_vt, Ordering::SeqCst);
    NATIVE_LOAD_LAST_MEMBER_DIALOG.store(m_dlg, Ordering::SeqCst);
    NATIVE_LOAD_LAST_MEMBER_FN.store(m_fn, Ordering::SeqCst);
    NATIVE_LOAD_LAST_MEMBER_ADJUST.store(m_adj, Ordering::SeqCst);
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
    unsafe { sample_title_profile_portrait_source(base, OWN_STEPPER_SLOT_ZERO) };
    append_autoload_debug(format_args!(
        "native-load: native Load-Game run returned -- observing native pump/profile renderer for golden oracle (#{n})"
    ));
}
/// Crash-on-not-loaded watchdog (privacy-policy-gated-on-character-presence-CONFIRMED-2026-06-23):
/// the Bandai-Namco privacy policy / new-game state shows ONLY when the active profile has no
/// character (profile_slot_active == 0). When a load is expected (not telemetry-only) and the profile
/// summary has been present but reports ZERO active slots for a settle window, the gold save did NOT
/// load -> abort instantly so the failure is loud + fast (no stall on the policy). profile_slot_active
/// != 0 is the single "save loaded" semaphore (redirect fired AND char present AND policy never builds).
pub(crate) unsafe fn save_load_watchdog() {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    if save_override_telemetry_only() {
        return;
    }
    let gdm = crate::game_data_man_ptr_or_null();
    if gdm == NULL {
        return;
    }
    let summary =
        unsafe { safe_read_usize(gdm + crate::SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(NULL);
    if summary == NULL {
        return; // profile summary not loaded yet -> still booting, do not count
    }
    // Profile-summary slot-active array offset == size_of::<usize>() (matches telemetry's read).
    let active = unsafe { safe_read_usize(summary + core::mem::size_of::<usize>()) }.unwrap_or(0);
    if active != 0 {
        SAVE_WATCHDOG_ZERO_FRAMES.store(0, Ordering::SeqCst); // char present -> save loaded
        // First gold load done: stop redirecting %APPDATA% so writes + later loads go to the real
        // default C: dir (the Z: write fails + would mutate the gold). One-shot.
        if !SAVE_FIRST_LOAD_DONE.swap(true, Ordering::SeqCst) {
            append_autoload_debug(format_args!(
                "save-override: FIRST-LOAD-DONE (profile_slot_active=0x{active:x}) -- reverting %APPDATA% redirect to the real default dir for writes + subsequent loads"
            ));
        }
        return;
    }
    let n = SAVE_WATCHDOG_ZERO_FRAMES.fetch_add(1, Ordering::SeqCst) + 1;
    if n == 1 {
        append_autoload_debug(format_args!(
            "save-override: watchdog -- profile summary present but ZERO active slots (no character); counting toward abort budget {SAVE_WATCHDOG_ZERO_BUDGET}"
        ));
    }
    if n >= SAVE_WATCHDOG_ZERO_BUDGET {
        append_autoload_debug(format_args!(
            "save-override: WATCHDOG ABORT -- profile summary reports ZERO active slots after {n} frames; the gold save did NOT load (no character -> privacy policy / new-game). Aborting."
        ));
        eprintln!(
            "er-effects: WATCHDOG ABORT -- gold save not loaded (no character in active profile); aborting."
        );
        std::process::abort();
    }
}
/// Resolve the full-read target slot: a configured OWN_STEPPER_SLOT (>=0, from the trigger-file
/// "slot=N"), else ER_EFFECTS_AUTOLOAD_SLOT (>=0), else FULLREAD_DEFAULT_SLOT (Banon = 0).
pub(crate) fn native_fullread_slot() -> i32 {
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
/// live TitleTopDialog menu action is semantically validated (same readiness helper as
/// native_load_tick: TitleTopDialog vtable, [dialog+0xa48] registry, Load-Game node/action chain),
/// it runs the full-save-read load chain as a per-frame phase
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
pub(crate) unsafe fn native_fullread_tick(owner: usize, base: usize, n: u64) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const WAIT_INC: usize = 1;
    let gm = game_man_ptr_or_null();
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
    let Some(action) = (unsafe { title_menu_action_ready(owner, base) }) else {
        if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            append_autoload_debug(format_args!(
                "native-fullread: waiting for semantic Load-Game action readiness (#{n}) gm=0x{gm:x} -- TitleTopDialog/registry/node/action not all validated yet"
            ));
        }
        return;
    };
    if gm == NULL {
        if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            append_autoload_debug(format_args!(
                "native-fullread: waiting for GameMan after menu action ready node=0x{:x} registry=0x{:x} (#{n})",
                action.node, action.registry
            ));
        }
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
        let game_data_man = game_data_man_ptr_or_null();
        let owner_obj = if game_data_man == NULL {
            NULL
        } else {
            unsafe { safe_read_usize(game_data_man + FULLREAD_OWNER_GDM_08_OFFSET) }.unwrap_or(NULL)
        };
        if owner_obj == NULL {
            append_autoload_debug(format_args!(
                "native-fullread: COMMIT ABORT -- continue_confirm owner (GameDataMan=0x{game_data_man:x}, offset=0x{:x}) is null -> DONE (no write)",
                FULLREAD_OWNER_GDM_08_OFFSET
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
pub(crate) unsafe fn profile_slot_fingerprint(slot: i32) -> (bool, i32, u32, usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const BAD_I32: i32 = -1;
    const ZERO_U32: u32 = 0;
    const NAME_LEN_NONE: usize = 0;
    const MIN_REAL_LEVEL: u32 = 1;
    const PROFILE_RECORD_BASE: usize = 0x18;
    const PROFILE_RECORD_STRIDE: usize = 0x2a0;
    const PROFILE_RECORD_LEVEL_OFFSET: usize = 0x24;
    const PROFILE_RECORD_MAP_OFFSET: usize = 0x30;
    if slot < OWN_STEPPER_SLOT_ZERO {
        return (false, BAD_I32, ZERO_U32, NAME_LEN_NONE);
    }
    let gdm = game_data_man_ptr_or_null();
    if gdm == NULL {
        return (false, BAD_I32, ZERO_U32, NAME_LEN_NONE);
    }
    let profile_summary =
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(NULL);
    if profile_summary == NULL {
        return (false, BAD_I32, ZERO_U32, NAME_LEN_NONE);
    }
    let slot_index = slot as usize;
    let rec = profile_summary + PROFILE_RECORD_BASE + slot_index * PROFILE_RECORD_STRIDE;
    let profile_map = unsafe { safe_read_usize(rec + PROFILE_RECORD_MAP_OFFSET) }
        .map(|value| value as u32 as i32)
        .unwrap_or(BAD_I32);
    let profile_level = unsafe { safe_read_usize(rec + PROFILE_RECORD_LEVEL_OFFSET) }
        .map(|value| value as u32)
        .unwrap_or(ZERO_U32);
    let (profile_name, profile_name_len) = unsafe { read_utf16_name_units(rec) };
    let profile_name_empty = utf16_name_empty_like(&profile_name, profile_name_len);
    (
        profile_level >= MIN_REAL_LEVEL && !profile_name_empty,
        profile_map,
        profile_level,
        profile_name_len,
    )
}
/// The save slot to auto-load: the ACTIVE slot holding the most-progressed real character (highest level;
/// lowest index on a tie). "Active/real" is judged by the RECORD-based `profile_slot_fingerprint`
/// (level>=1 && non-empty name) -- NOT the `profile_summary+0x8` active byte, which the DLL writes itself
/// (PROFILE_SLOT_ACTIVATE / seed) and so reads all-active even for a NULL slot. Returns
/// `OWN_STEPPER_SLOT_NONE` (-1) when NO slot holds a real character (or the profile summary is not yet
/// populated); callers MUST refuse to load on the sentinel -- never load a null slot (which spawns the
/// new-game intro cutscene + a null character).
pub(crate) unsafe fn best_active_slot() -> i32 {
    let mut best_slot = OWN_STEPPER_SLOT_NONE;
    let mut best_level: u32 = 0;
    let mut slot: i32 = OWN_STEPPER_SLOT_ZERO;
    while (slot as usize) < TITLE_PROFILE_SLOT_COUNT {
        let (is_real, _map, level, _name_len) = unsafe { profile_slot_fingerprint(slot) };
        if is_real && level > best_level {
            best_level = level;
            best_slot = slot;
        }
        slot += 1;
    }
    best_slot
}
/// Resolve the slot to actually load under the user's guards: honor a configured slot ONLY if it holds a
/// real character; otherwise fall back to `best_active_slot()` ("whatever is indicated as an active slot on
/// disk"). Returns `OWN_STEPPER_SLOT_NONE` when nothing is loadable so the caller refuses to load.
pub(crate) unsafe fn resolve_active_load_slot(configured: i32) -> i32 {
    if configured >= OWN_STEPPER_SLOT_ZERO && unsafe { profile_slot_fingerprint(configured).0 } {
        return configured;
    }
    unsafe { best_active_slot() }
}
pub(crate) unsafe fn requested_slot_identity(slot: i32, c30: i32) -> RequestedSlotIdentity {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const BAD_I32: i32 = -1;
    const ZERO_U32: u32 = 0;
    const NAME_LEN_NONE: usize = 0;
    const PROFILE_RECORD_BASE: usize = 0x18;
    const PROFILE_RECORD_STRIDE: usize = 0x2a0;
    const PROFILE_RECORD_LEVEL_OFFSET: usize = 0x24;
    const PROFILE_RECORD_MAP_OFFSET: usize = 0x30;
    let mut result = RequestedSlotIdentity {
        matches: false,
        profile_summary: NULL,
        profile_map: BAD_I32,
        profile_level: ZERO_U32,
        profile_name_len: NAME_LEN_NONE,
        pgd_level: ZERO_U32,
        pgd_name_len: NAME_LEN_NONE,
    };
    if slot < OWN_STEPPER_SLOT_ZERO {
        return result;
    }
    let gdm = game_data_man_ptr_or_null();
    if gdm == NULL {
        return result;
    }
    let pgd =
        unsafe { safe_read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }.unwrap_or(NULL);
    let profile_summary =
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(NULL);
    result.profile_summary = profile_summary;
    if pgd == NULL || profile_summary == NULL {
        return result;
    }
    let slot_index = slot as usize;
    let rec = profile_summary + PROFILE_RECORD_BASE + slot_index * PROFILE_RECORD_STRIDE;
    let profile_map = unsafe { safe_read_usize(rec + PROFILE_RECORD_MAP_OFFSET) }
        .map(|value| value as u32 as i32)
        .unwrap_or(BAD_I32);
    let profile_level = unsafe { safe_read_usize(rec + PROFILE_RECORD_LEVEL_OFFSET) }
        .map(|value| value as u32)
        .unwrap_or(ZERO_U32);
    let (profile_name, profile_name_len) = unsafe { read_utf16_name_units(rec) };
    let pgd_level = unsafe { safe_read_usize(pgd + PGD_LEVEL_68_OFFSET) }
        .map(|value| value as u32)
        .unwrap_or(ZERO_U32);
    let (pgd_name, pgd_name_len) = unsafe { read_utf16_name_units(pgd + PGD_NAME_9C_OFFSET) };
    let profile_name_empty = utf16_name_empty_like(&profile_name, profile_name_len);
    let pgd_name_empty = utf16_name_empty_like(&pgd_name, pgd_name_len);
    result.profile_map = profile_map;
    result.profile_level = profile_level;
    result.profile_name_len = profile_name_len;
    result.pgd_level = pgd_level;
    result.pgd_name_len = pgd_name_len;
    result.matches = profile_map == c30
        && profile_level == pgd_level
        && profile_name_len == pgd_name_len
        && !profile_name_empty
        && !pgd_name_empty
        && utf16_names_equal(&profile_name, &pgd_name, pgd_name_len);
    result
}
/// CHAR-FINGERPRINT save-write gate: returns (is_real, level, name_len) by reading the live
/// CS::PlayerGameData (GameDataMan `[base+0x3d5df38]` -> +0x08 -> PlayerGameData), the validated
/// reading (the same chain dump_load_correctness uses). A REAL mounted character has level>=1 AND
/// a non-empty-like 16-bit name (`"_"`, empty, and all-spaces are empty-like). Pure
/// fault-tolerant safe_read_usize -> never faults. Used to FAIL-CLOSED SetState(5): the c30
/// oracle is ambiguous (m10_01 collision), so the character actually present in PlayerGameData is
/// the decisive signal.
pub(crate) unsafe fn char_fingerprint(base: usize) -> (bool, u32, usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const ZERO_U32: u32 = 0;
    const MIN_REAL_LEVEL: u32 = 1;
    const NAME_LEN_NONE: usize = 0;
    let gdm = game_data_man_ptr_or_null();
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
    let (name_units, name_len) = unsafe { read_utf16_name_units(pgd + PGD_NAME_9C_OFFSET) };
    let is_real = level >= MIN_REAL_LEVEL && !utf16_name_empty_like(&name_units, name_len);
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
    let gm = game_man_ptr_or_null();
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
    let gdm = game_data_man_ptr_or_null();
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
/// Recipe Option 1 (genuine offline continue, flagless): drive the MoveMapList
/// dispatcher 0x140afb880 each frame with GameMan b73 set so it begins
/// current_slot_load and deserializes the REAL slot character (sets
/// GameMan+0x10=1), also building the world singletons. owner is a synthetic
/// buffer with +0x12c = slot. Never writes the force flag 0x143d856a0.
pub(crate) unsafe fn continue_drive_tick(module_base: usize, slot: i32, tick: u64) {
    // Log readiness before the fixed drive gate: recent runs exit before the
    // drive can fire, so the next runtime must tell us when GameMan first became
    // available instead of turning the gate into another blind threshold knob.
    let game_man = game_man_ptr_or_null();
    if game_man == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let first_seen_tick = match CONTINUE_DRIVE_GM_FIRST_SEEN_TICK.compare_exchange(
        CONTINUE_DRIVE_GM_FIRST_SEEN_UNSET,
        tick,
        Ordering::SeqCst,
        Ordering::SeqCst,
    ) {
        Ok(_) => {
            append_autoload_debug(format_args!(
                "continue_drive: GameMan first_seen tick={tick} gm=0x{game_man:x} after_gm_gate={CONTINUE_DRIVE_AFTER_GAME_MAN_TICKS}"
            ));
            tick
        }
        Err(existing) => existing,
    };
    let game_man_relative_gate =
        first_seen_tick.saturating_add(CONTINUE_DRIVE_AFTER_GAME_MAN_TICKS);
    let drive_gate_tick = core::cmp::max(CONTINUE_DRIVE_MIN_TICK, game_man_relative_gate);
    if tick < drive_gate_tick {
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
    let first_attempt = !CONTINUE_DRIVE_FIRST_ATTEMPT_LOGGED.swap(true, Ordering::SeqCst);
    if first_attempt {
        let b73_before = unsafe { *((game_man + GAME_MAN_B73_FLAG_OFFSET) as *const u8) };
        append_autoload_debug(format_args!(
            "continue_drive: FIRST dispatcher before slot={slot} b80={load_progress} b73={b73_before} real_done={real_done} map14={map14} tick={tick} gate_tick={drive_gate_tick}"
        ));
    }
    let dispatcher: unsafe extern "system" fn(*mut u8) -> usize =
        unsafe { std::mem::transmute(module_base + MOVEMAP_DISPATCHER_RVA) };
    let _ = unsafe { dispatcher(owner) };
    if first_attempt
        || tick % TITLE_JOB_OBSERVE_TICK_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64
    {
        let real_after = unsafe { *((game_man + GAME_MAN_REAL_LOAD_DONE_OFFSET) as *const i32) };
        let b80_after =
            unsafe { *((game_man + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const u8) };
        let b73_after = unsafe { *((game_man + GAME_MAN_B73_FLAG_OFFSET) as *const u8) };
        let map14_after =
            unsafe { *((game_man + FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET) as *const i32) };
        append_autoload_debug(format_args!(
            "continue_drive: drove dispatcher slot={slot} b80={b80_after} b73={b73_after} real_done={real_after} map14={map14_after} tick={tick}"
        ));
    }
}
