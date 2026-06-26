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

/// DIAGNOSTIC detour for the dialog builder 0x1409275b0 (4 register args rcx/rdx/r8/r9 -> dialog
/// in rax). Calls the original, then (pre-world, capped) logs the BUILT dialog's vtable/class +
/// the 4 args (the FMG message id is one of them) + caller, so we can identify the actual
/// connection-error dialog without guessing. Read-only; never mutates the dialog.
unsafe fn policy_tos_record_fields(record: usize) -> (usize, usize, usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if record == null {
        return (null, null, null);
    }
    let record_id = unsafe { safe_read_i32(record) }
        .map(|value| value.max(0) as usize)
        .unwrap_or(null);
    let stack_arg0 = unsafe { safe_read_i32(record + 0x4) }
        .map(|value| value.max(0) as usize)
        .unwrap_or(null);
    let backing_flag_ptr = unsafe { safe_read_usize(record + 0x8) }.unwrap_or(null);
    (record_id, stack_arg0, backing_flag_ptr)
}

/// Operator gate for zero-input ToS-modal suppression. Default OFF: the wrapper builds the
/// TosMultiLangDialog as the game normally would. When enabled (only on a profile where the
/// Terms of Service is already accepted), `policy_tos_title_ctor_wrapper_hook` skips the
/// build and returns null, so the unnecessary startup ToS modal is never constructed -- no
/// input, no auto-accept of an un-accepted policy, no MessageBox.
pub(crate) fn policy_tos_suppress_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_POLICY_TOS_SUPPRESS").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-policy-tos-suppress.txt")
        .exists()
}

pub(crate) unsafe extern "system" fn policy_tos_title_ctor_wrapper_hook(
    record: usize,
    rdx: usize,
    r8: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let (record_id, stack_arg0, backing_flag_ptr) = unsafe { policy_tos_record_fields(record) };
    let original_this = record.saturating_sub(POLICY_TOS_TITLE_WRAPPER_THIS_ADJUST);
    let original_vtable = if original_this != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(original_this) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let caller_rva = trace_first_game_caller_rva();
    let backing_flag_value = if backing_flag_ptr != null {
        unsafe { safe_read_usize(backing_flag_ptr) }.unwrap_or(0)
    } else {
        0
    };
    let orig = POLICY_TOS_TITLE_CTOR_WRAPPER_ORIG.load(Ordering::SeqCst);
    let ret = if policy_tos_suppress_enabled() {
        // Replace the native "show ToS" stepper with our own no-op: skip building the
        // TosMultiLangDialog and return null, mimicking the wrapper's native allocation-
        // failure path (caller-tolerated). The ToS ctor 0x1409b5970 -- whose only caller is
        // this wrapper -- never runs, so the policy/ToS ctor hook never fires and
        // POLICY_TOS_TITLE_TOTAL_BUILDS stays 0: the unnecessary startup modal is never
        // constructed. Zero input, no auto-accept.
        POLICY_TOS_TITLE_SUPPRESSED_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "policy-oracle: SUPPRESSED TosMultiLangDialog build (wrapper 0x{:x}) -> returned null (native alloc-fail path) record=0x{record:x} backing_flag_ptr=0x{backing_flag_ptr:x} backing_flag_value={backing_flag_value} -- zero-input ToS-modal suppression",
            game_module_base().unwrap_or(null) + POLICY_TOS_TITLE_CTOR_WRAPPER_RVA as usize,
        ));
        POLICY_TOS_MODAL_SUPPRESSED_RETURN
    } else if orig == HOOK_ORIGINAL_UNSET {
        null
    } else {
        let f: unsafe extern "system" fn(usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(record, rdx, r8) }
    };
    POLICY_TOS_TITLE_WRAPPER_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_RECORD.store(record, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_THIS.store(original_this, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_VTABLE.store(original_vtable, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_RECORD_ID.store(record_id, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_STACK_ARG0.store(stack_arg0, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_BACKING_FLAG_PTR.store(backing_flag_ptr, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_RET.store(ret, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    ret
}

pub(crate) unsafe extern "system" fn policy_tos_selector_wrapper_hook(record: usize) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let owner = if record != null {
        unsafe { safe_read_usize(record) }.unwrap_or(null)
    } else {
        null
    };
    let requested_flag = if owner != null {
        unsafe { safe_read_i32(owner + 0x29c8) }
            .map(|value| value.max(0) as usize)
            .unwrap_or(null)
    } else {
        null
    };
    let selector_arg = if owner != null { owner + 0x29d0 } else { null };
    let original_this = record.saturating_sub(POLICY_TOS_TITLE_WRAPPER_THIS_ADJUST);
    let original_vtable = if original_this != null {
        unsafe { safe_read_usize(original_this) }.unwrap_or(null)
    } else {
        null
    };
    let caller_rva = trace_first_game_caller_rva();
    let orig = POLICY_TOS_SELECTOR_WRAPPER_ORIG.load(Ordering::SeqCst);
    let ret = if orig == HOOK_ORIGINAL_UNSET {
        null
    } else {
        let f: unsafe extern "system" fn(usize) -> usize = unsafe { std::mem::transmute(orig) };
        unsafe { f(record) }
    };
    POLICY_TOS_SELECTOR_WRAPPER_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_RECORD.store(record, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_ORIGINAL_THIS.store(original_this, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_ORIGINAL_VTABLE.store(original_vtable, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_OWNER.store(owner, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_REQUESTED_FLAG.store(requested_flag, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_SELECTOR_ARG.store(selector_arg, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_RET.store(ret, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    ret
}

pub(crate) unsafe extern "system" fn policy_tos_selector_ctor_hook(
    this: usize,
    rdx: usize,
    r8: usize,
    selector_arg: usize,
    requested_flag_ptr: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let requested_flag_value = if requested_flag_ptr != null {
        unsafe { safe_read_i32(requested_flag_ptr) }
            .map(|value| value.max(0) as usize)
            .unwrap_or(null)
    } else {
        null
    };
    let owner = selector_arg.saturating_sub(0x29d0);
    let caller_rva = trace_first_game_caller_rva();
    let orig = POLICY_TOS_SELECTOR_CTOR_ORIG.load(Ordering::SeqCst);
    let ret = if orig == HOOK_ORIGINAL_UNSET {
        null
    } else {
        let f: unsafe extern "system" fn(usize, usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(this, rdx, r8, selector_arg, requested_flag_ptr) }
    };
    let object = if ret != null { ret } else { this };
    let vt = if object != null {
        unsafe { safe_read_usize(object) }.unwrap_or(null)
    } else {
        null
    };
    let stored_selector_arg = if object != null {
        unsafe { safe_read_usize(object + 0x1260) }.unwrap_or(null)
    } else {
        null
    };
    let stored_requested_flag_ptr = if object != null {
        unsafe { safe_read_usize(object + 0x1268) }.unwrap_or(null)
    } else {
        null
    };
    POLICY_TOS_SELECTOR_CTOR_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_THIS.store(object, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_VTABLE.store(vt, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_OWNER.store(owner, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_PTR.store(requested_flag_ptr, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_VALUE
        .store(requested_flag_value, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_SELECTOR_ARG.store(selector_arg, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_STORED_SELECTOR_ARG.store(stored_selector_arg, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_STORED_REQUESTED_FLAG_PTR
        .store(stored_requested_flag_ptr, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_RET.store(ret, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    ret
}

unsafe fn policy_tos_flag_value(owner: usize) -> (usize, usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let flag_ptr = if owner != null {
        unsafe { safe_read_usize(owner + 0x29c0) }.unwrap_or(null)
    } else {
        null
    };
    let flag_value = if flag_ptr != null {
        unsafe { safe_read_i32(flag_ptr) }
            .map(|value| value.max(0) as usize)
            .unwrap_or(null)
    } else {
        null
    };
    (flag_ptr, flag_value)
}

pub(crate) unsafe extern "system" fn policy_tos_flag_setter_hook(
    owner: usize,
    value: i32,
    force: u8,
) {
    let caller_rva = trace_first_game_caller_rva();
    let orig = POLICY_TOS_FLAG_SETTER_ORIG.load(Ordering::SeqCst);
    let (_, before) = unsafe { policy_tos_flag_value(owner) };
    if orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, i32, u8) = unsafe { std::mem::transmute(orig) };
        unsafe { f(owner, value, force) };
    }
    let (_, after) = unsafe { policy_tos_flag_value(owner) };
    POLICY_TOS_FLAG_SETTER_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    POLICY_TOS_FLAG_SETTER_LAST_OWNER.store(owner, Ordering::SeqCst);
    POLICY_TOS_FLAG_SETTER_LAST_VALUE.store(value.max(0) as usize, Ordering::SeqCst);
    POLICY_TOS_FLAG_SETTER_LAST_FORCE.store(force as usize, Ordering::SeqCst);
    POLICY_TOS_FLAG_SETTER_LAST_BEFORE.store(before, Ordering::SeqCst);
    POLICY_TOS_FLAG_SETTER_LAST_AFTER.store(after, Ordering::SeqCst);
    POLICY_TOS_FLAG_SETTER_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
}

pub(crate) unsafe extern "system" fn policy_tos_status_predicate_hook(this: usize) -> u8 {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let caller_rva = trace_first_game_caller_rva();
    let orig = POLICY_TOS_STATUS_PREDICATE_ORIG.load(Ordering::SeqCst);
    let ret = if orig == HOOK_ORIGINAL_UNSET {
        0
    } else {
        let f: unsafe extern "system" fn(usize) -> u8 = unsafe { std::mem::transmute(orig) };
        unsafe { f(this) }
    };
    let owner = unsafe { safe_read_usize(this + core::mem::size_of::<usize>()) }.unwrap_or(null);
    let (flag_ptr, flag_value) = unsafe { policy_tos_flag_value(owner) };
    POLICY_TOS_STATUS_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    POLICY_TOS_STATUS_LAST_THIS.store(this, Ordering::SeqCst);
    POLICY_TOS_STATUS_LAST_OWNER.store(owner, Ordering::SeqCst);
    POLICY_TOS_STATUS_LAST_FLAG_PTR.store(flag_ptr, Ordering::SeqCst);
    POLICY_TOS_STATUS_LAST_FLAG_VALUE.store(flag_value, Ordering::SeqCst);
    POLICY_TOS_STATUS_LAST_RET.store(ret as usize, Ordering::SeqCst);
    POLICY_TOS_STATUS_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    ret
}

pub(crate) unsafe extern "system" fn policy_tos_title_ctor_hook(
    this: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
    stack_arg0: usize,
    backing_flag_ptr: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let caller_rva = trace_first_game_caller_rva();
    let orig = POLICY_TOS_TITLE_CTOR_ORIG.load(Ordering::SeqCst);
    let ret = if orig == HOOK_ORIGINAL_UNSET {
        null
    } else {
        let f: unsafe extern "system" fn(usize, usize, usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(this, rdx, r8, r9, stack_arg0, backing_flag_ptr) }
    };
    let base = game_module_base().unwrap_or(null);
    let object = if ret != null { ret } else { this };
    let vt = if object != null {
        unsafe { safe_read_usize(object) }.unwrap_or(null)
    } else {
        null
    };
    let stored_backing_flag_ptr = if object != null {
        unsafe { safe_read_usize(object + 0x29c0) }.unwrap_or(null)
    } else {
        null
    };
    let backing_flag_value = if stored_backing_flag_ptr != null {
        unsafe { safe_read_i32(stored_backing_flag_ptr) }
            .map(|value| value.max(0) as usize)
            .unwrap_or(null)
    } else {
        null
    };
    let requested_flag_value = if object != null {
        unsafe { safe_read_i32(object + 0x29c8) }
            .map(|value| value.max(0) as usize)
            .unwrap_or(null)
    } else {
        null
    };
    POLICY_TOS_TITLE_LAST_THIS.store(object, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_VTABLE.store(vt, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_ARG_RDX.store(rdx, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_ARG_R8.store(r8, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_ARG_R9.store(r9, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_STACK_ARG0.store(stack_arg0, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_BACKING_FLAG_PTR.store(backing_flag_ptr, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_STORED_BACKING_FLAG_PTR.store(stored_backing_flag_ptr, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_BACKING_FLAG_VALUE.store(backing_flag_value, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_REQUESTED_FLAG_VALUE.store(requested_flag_value, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    POLICY_TOS_TITLE_TOTAL_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    write_policy_oracle_snapshot("tos_title_ctor");
    append_autoload_debug(format_args!(
        "policy-oracle: TosTitle ctor 0x{:x} built object=0x{object:x} vt=0x{vt:x} expected_vt=0x{:x} args(rdx=0x{rdx:x} r8=0x{r8:x} r9=0x{r9:x} stack0=0x{stack_arg0:x} backing_flag_ptr=0x{backing_flag_ptr:x}) stored_backing_flag_ptr=0x{stored_backing_flag_ptr:x} backing_flag_value={backing_flag_value} requested_flag_value={requested_flag_value} text_path=0x{:x} -- native/asset-backed Privacy/ToS surface regression",
        base + POLICY_TOS_TITLE_CTOR_RVA as usize,
        base + POLICY_TOS_TITLE_VTABLE_RVA,
        base + POLICY_TOS_TITLE_TEXT_PATH_RVA
    ));
    ret
}

pub(crate) fn install_policy_tos_title_hook() {
    if POLICY_TOS_TITLE_HOOK_INSTALLED.load(Ordering::SeqCst) != POLICY_TOS_TITLE_HOOK_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "policy-oracle: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(wrapper_addr) = game_rva(POLICY_TOS_TITLE_CTOR_WRAPPER_RVA) else {
        append_autoload_debug(format_args!(
            "policy-oracle: failed to resolve ToS ctor wrapper rva"
        ));
        return;
    };
    if let Ok(hook) = unsafe {
        MhHook::new(
            wrapper_addr as *mut c_void,
            policy_tos_title_ctor_wrapper_hook as *mut c_void,
        )
    } {
        POLICY_TOS_TITLE_CTOR_WRAPPER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            append_autoload_debug(format_args!(
                "policy-oracle: queue_enable ToS ctor wrapper failed: {status:?}"
            ));
        } else {
            std::mem::forget(hook);
        }
    }
    let Ok(selector_wrapper_addr) = game_rva(POLICY_TOS_SELECTOR_WRAPPER_RVA) else {
        append_autoload_debug(format_args!(
            "policy-oracle: failed to resolve ToS selector wrapper rva"
        ));
        return;
    };
    if let Ok(hook) = unsafe {
        MhHook::new(
            selector_wrapper_addr as *mut c_void,
            policy_tos_selector_wrapper_hook as *mut c_void,
        )
    } {
        POLICY_TOS_SELECTOR_WRAPPER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            append_autoload_debug(format_args!(
                "policy-oracle: queue_enable ToS selector wrapper failed: {status:?}"
            ));
        } else {
            std::mem::forget(hook);
        }
    }
    let Ok(selector_ctor_addr) = game_rva(POLICY_TOS_SELECTOR_CTOR_RVA) else {
        append_autoload_debug(format_args!(
            "policy-oracle: failed to resolve ToS selector ctor rva"
        ));
        return;
    };
    if let Ok(hook) = unsafe {
        MhHook::new(
            selector_ctor_addr as *mut c_void,
            policy_tos_selector_ctor_hook as *mut c_void,
        )
    } {
        POLICY_TOS_SELECTOR_CTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            append_autoload_debug(format_args!(
                "policy-oracle: queue_enable ToS selector ctor failed: {status:?}"
            ));
        } else {
            std::mem::forget(hook);
        }
    }
    let Ok(predicate_addr) = game_rva(POLICY_TOS_STATUS_PREDICATE_RVA) else {
        append_autoload_debug(format_args!(
            "policy-oracle: failed to resolve ToS status predicate rva"
        ));
        return;
    };
    if let Ok(hook) = unsafe {
        MhHook::new(
            predicate_addr as *mut c_void,
            policy_tos_status_predicate_hook as *mut c_void,
        )
    } {
        POLICY_TOS_STATUS_PREDICATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            append_autoload_debug(format_args!(
                "policy-oracle: queue_enable ToS status predicate failed: {status:?}"
            ));
        } else {
            std::mem::forget(hook);
        }
    }
    let Ok(flag_setter_addr) = game_rva(POLICY_TOS_FLAG_SETTER_RVA) else {
        append_autoload_debug(format_args!(
            "policy-oracle: failed to resolve ToS flag setter rva"
        ));
        return;
    };
    if let Ok(hook) = unsafe {
        MhHook::new(
            flag_setter_addr as *mut c_void,
            policy_tos_flag_setter_hook as *mut c_void,
        )
    } {
        POLICY_TOS_FLAG_SETTER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            append_autoload_debug(format_args!(
                "policy-oracle: queue_enable ToS flag setter failed: {status:?}"
            ));
        } else {
            std::mem::forget(hook);
        }
    }
    let Ok(ctor_addr) = game_rva(POLICY_TOS_TITLE_CTOR_RVA) else {
        append_autoload_debug(format_args!(
            "policy-oracle: failed to resolve TosTitle ctor rva"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            ctor_addr as *mut c_void,
            policy_tos_title_ctor_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            POLICY_TOS_TITLE_CTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "policy-oracle: queue_enable TosTitle ctor failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    POLICY_TOS_TITLE_HOOK_INSTALLED
                        .store(POLICY_TOS_TITLE_HOOK_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "policy-oracle: hooked TosTitle ctor 0x{ctor_addr:x}, ctor wrapper 0x{wrapper_addr:x}, selector wrapper 0x{selector_wrapper_addr:x}, selector ctor 0x{selector_ctor_addr:x}, status predicate 0x{predicate_addr:x}, and flag setter 0x{flag_setter_addr:x} (native Privacy/ToS surface oracle)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "policy-oracle: MH_ApplyQueued TosTitle ctor failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "policy-oracle: MhHook::new TosTitle ctor failed: {status:?}"
        )),
    }
}

pub(crate) fn server_status_text_id_is_product_failure(text_id: usize) -> bool {
    matches!(
        text_id,
        SERVER_STATUS_CHECKING_NETWORK_TEXT_ID
            | SERVER_STATUS_LOGGING_IN_TEXT_ID
            | SERVER_STATUS_RETRIEVING_DATA_TEXT_ID
            | SERVER_STATUS_SAVING_DATA_TEXT_ID
    )
}

pub(crate) unsafe extern "system" fn server_status_formatter_hook(
    record_slot: usize,
    out_text: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let record = unsafe { safe_read_usize(record_slot) }.unwrap_or(null);
    if record != null {
        let state = unsafe { safe_read_i32(record + SERVER_STATUS_RECORD_STATE_OFFSET) }
            .unwrap_or(-1)
            .max(0) as usize;
        let text_id = unsafe { safe_read_i32(record + SERVER_STATUS_RECORD_TEXT_ID_OFFSET) }
            .unwrap_or(-1)
            .max(0) as usize;
        if server_status_text_id_is_product_failure(text_id) {
            SERVER_STATUS_TOTAL_SEEN.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            SERVER_STATUS_LAST_STATE.store(state, Ordering::SeqCst);
            SERVER_STATUS_LAST_TEXT_ID.store(text_id, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "server-status-oracle: state={state} text_id={text_id} via formatter 0x{:x} -- invalid online/login status semaphore {}",
                game_module_base().unwrap_or(null) + SERVER_STATUS_FORMATTER_RVA as usize,
                trace_callers_summary()
            ));
        }
    }
    let orig = SERVER_STATUS_FORMATTER_ORIG.load(Ordering::SeqCst);
    if orig == null {
        return out_text;
    }
    let f: unsafe extern "system" fn(usize, usize) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { f(record_slot, out_text) }
}

pub(crate) fn install_server_status_hook() {
    if SERVER_STATUS_HOOK_INSTALLED.load(Ordering::SeqCst) != SERVER_STATUS_HOOK_NOT_INSTALLED {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "server-status-oracle: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(formatter_addr) = game_rva(SERVER_STATUS_FORMATTER_RVA) else {
        append_autoload_debug(format_args!(
            "server-status-oracle: failed to resolve formatter rva"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            formatter_addr as *mut c_void,
            server_status_formatter_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SERVER_STATUS_FORMATTER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "server-status-oracle: queue_enable formatter failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SERVER_STATUS_HOOK_INSTALLED
                        .store(SERVER_STATUS_HOOK_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "server-status-oracle: hooked formatter 0x{formatter_addr:x} (server/login semaphore oracle)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "server-status-oracle: MH_ApplyQueued formatter failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "server-status-oracle: MhHook::new formatter failed: {status:?}"
        )),
    }
}

/// Read a DLW (UTF-16 / char16_t) `basic_string` at `s` and return up to `max_chars` of its text.
/// Layout: [+0x10]=length (chars), [+0x18]=capacity (chars); the text is inline at `s` when capacity
/// < 8, else `*(s)` points at the heap buffer. Every read is fault-guarded so a garbage Spec field can
/// never AV the game thread. UTF-16 lossy decode (the repo no-lossy lint targets from_utf8_lossy only).
unsafe fn read_dlw_string(s: usize, max_chars: usize) -> Option<String> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if s <= null {
        return None;
    }
    let length = unsafe { safe_read_usize(s + 0x10) }?;
    let capacity = unsafe { safe_read_usize(s + 0x18) }?;
    if length == null || length > 4096 {
        return None;
    }
    let take = length.min(max_chars);
    let text_ptr = if capacity < 8 {
        s
    } else {
        unsafe { safe_read_usize(s) }?
    };
    if text_ptr <= null {
        return None;
    }
    let mut buf: Vec<u16> = Vec::with_capacity(take);
    for i in 0..take {
        let w = (unsafe { safe_read_usize(text_ptr + i * 2) }? & 0xffff) as u16;
        if w == 0 {
            break;
        }
        buf.push(w);
    }
    if buf.is_empty() {
        return None;
    }
    Some(String::from_utf16_lossy(&buf))
}

/// Diagnostic: dump the MessageBoxDialog builder Spec (`r8`) to NAME the modal's message. The text id
/// is NOT in rdx/r9 (a pointer pair 0x40 apart) and is NOT fetched via GetGR_System_Message at build
/// time, so read it straight from the Spec. Tries the reported MenuString offset (+0x8e0) plus a scan
/// of early offsets for any embedded/pointed-to DLW string. Read-only; logs each decoded string.
unsafe fn dump_msgbox_spec(c: usize, n: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if c <= null {
        return;
    }
    if let Some(text) =
        unsafe { read_dlw_string(unsafe { safe_read_usize(c + 0x8e0) }.unwrap_or(null), 80) }
    {
        append_autoload_debug(format_args!("spec #{n}: text@*(r8+0x8e0)=\"{text}\""));
    }
    let mut off = 0usize;
    while off < 0x120 {
        // Inline DLW string at r8+off.
        if let Some(text) = unsafe { read_dlw_string(c + off, 80) } {
            append_autoload_debug(format_args!("spec #{n}: inline[r8+0x{off:x}]=\"{text}\""));
        }
        // Pointer-to-DLW-string at r8+off.
        if let Some(ptr) = unsafe { safe_read_usize(c + off) } {
            if let Some(text) = unsafe { read_dlw_string(ptr, 80) } {
                append_autoload_debug(format_args!("spec #{n}: *[r8+0x{off:x}]=\"{text}\""));
            }
        }
        off += 8;
    }
}

pub(crate) unsafe extern "system" fn msgbox_builder_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if product_autoload_enabled() {
        MSGBOX_LAST_ARG_RCX.store(a, Ordering::SeqCst);
        MSGBOX_LAST_ARG_RDX.store(b, Ordering::SeqCst);
        MSGBOX_LAST_ARG_R8.store(c, Ordering::SeqCst);
        MSGBOX_LAST_ARG_R9.store(d, Ordering::SeqCst);
        MSGBOX_TOTAL_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        if IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES {
            MSGBOX_POSTLOAD_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        }
        let n = MSGBOX_BUILDER_LOG.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        if n < MSGBOX_BUILDER_LOG_MAX {
            append_autoload_debug(format_args!(
                "msgbox-skip #{n}: product autoload suppressed MessageBoxDialog builder before UI allocation but counted it as oracle failure args(rcx=0x{a:x} rdx=0x{b:x} r8=0x{c:x} r9=0x{d:x}) {}",
                trace_callers_summary()
            ));
        }
        return null;
    }
    let orig = MSGBOX_BUILDER_ORIG.load(Ordering::SeqCst);
    let ret = if orig != null {
        let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(a, b, c, d) }
    } else {
        null
    };
    if ret != null {
        let base = {
            let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
            if own != null {
                own
            } else {
                game_module_base().unwrap_or(null)
            }
        };
        let vt = unsafe { safe_read_usize(ret) }.unwrap_or(null);
        let is_msgbox = vt == base + MSGBOX_DIALOG_VTABLE_RVA;
        let in_world = IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES;
        // CAPTURE the startup MessageBoxDialog (connection-error / EULA / warning) pre-world so
        // the game task can dismiss it via the real OK handler. Post-load/in-world dialogs are
        // NEVER auto-dismissed; they are only latched for telemetry so the oracle fails instead of
        // reporting a false 1400 when a blocking popup remains on screen.
        if is_msgbox {
            MSGBOX_LAST_DIALOG.store(ret, Ordering::SeqCst);
            MSGBOX_LAST_ARG_RCX.store(a, Ordering::SeqCst);
            MSGBOX_LAST_ARG_RDX.store(b, Ordering::SeqCst);
            MSGBOX_LAST_ARG_R8.store(c, Ordering::SeqCst);
            MSGBOX_LAST_ARG_R9.store(d, Ordering::SeqCst);
            MSGBOX_TOTAL_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            if in_world {
                MSGBOX_POSTLOAD_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            } else {
                CONNECTION_ERROR_DIALOG.store(ret, Ordering::SeqCst);
            }
        }
        let n = MSGBOX_BUILDER_LOG.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        if n < MSGBOX_BUILDER_LOG_MAX {
            let vt_rva = vt.wrapping_sub(base);
            append_autoload_debug(format_args!(
                "msgbox-builder #{n}: dialog=0x{ret:x} vt=0x{vt:x} vt_rva=0x{vt_rva:x} captured={is_msgbox} in_world={in_world} args(rcx=0x{a:x} rdx=0x{b:x} r8=0x{c:x} r9=0x{d:x}) {}",
                trace_callers_summary()
            ));
            // NAME the modal: read its message text straight from the Spec (r8=c).
            unsafe { dump_msgbox_spec(c, n) };
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
    if base == null || !is_startup_msgbox_vtable(vt, base) {
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
    // Drive the dialog Decided + OK + fade-complete BEFORE the OK-handler so (a) the title-flow's
    // modal-build poll ([dialog+0x25e8]>0 at 0x1407b04f5) treats it as resolved and PROCEEDS to the
    // menu, and (b) the OK-handler's fade gate (commit only when fade_current<=fade_target) fires THIS
    // frame -> instant commit/close, no fade-in render = no flash (vs the ~20 OnDecide frames before).
    // The dialog is vtable-validated above (base MessageBoxDialog OR SaveRetryDialog). bd
    // press-any-button-golden-lever-job1e8-readiness-2026-06-23 + offline-title-modal-is-saveretrydialog.
    unsafe {
        *((dialog + MSGBOX_STATE_25E8_OFFSET) as *mut i32) = MSGBOX_STATE_DECIDED;
        *((dialog + MSGBOX_RESULT_BUTTON_25E0_OFFSET) as *mut i32) = MSGBOX_OK_BUTTON;
    }
    if let Some(fade_target_bits) =
        unsafe { safe_read_i32(dialog + MSGBOX_FADE_TARGET_2300_OFFSET) }
    {
        unsafe {
            *((dialog + MSGBOX_FADE_CURRENT_1278_OFFSET) as *mut i32) = fade_target_bits;
        }
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

/// Diagnostic gate (GAME_DIR file `er-effects-grsysmsg-log.txt` or `ER_EFFECTS_GRSYSMSG_LOG=1`):
/// arm the GR_System_Message id-logger so a probe can DEFINITIVELY name which message(s) the
/// menu-open MessageBoxDialogs carry (instead of guessing connection vs save). Reusable tool.
pub(crate) fn grsysmsg_log_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_GRSYSMSG_LOG").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-grsysmsg-log.txt")
            .exists()
}

static GR_SYSMSG_LOG_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static GR_SYSMSG_LOG_ORIG: AtomicUsize = AtomicUsize::new(0);
static GR_SYSMSG_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
/// `CS::GetGR_System_Message` (deobf entry 0x140762e30): `MenuString* (rcx=out, edx=int messageId)`.
/// The dump labels it 0x140762e40 but that is MID-INSTRUCTION (inside `movq $-2,[rsp+0x28]`); the real
/// MSVC prologue (`mov [rsp+8],rcx; push rdi; sub rsp,0x30`) is at 0x140762e30 -- VERIFIED by deobf
/// boundary disasm (prev fn ret+int3 at 0x140762e26/27, then this prologue). Body reads FMG repo
/// [0x143d7d4f8], applies the +0x384 variant, builds the MenuString.
// CORRECTED 2026-06-23 (corrupted-save-re-findings): 0x762e30 is GetTextEmbedImageName (it does
// id += 900, uses a different singleton) -- NOT GetGR_System_Message. The real getter is deobf
// 0x140762d50 (dump 0x140762e40 - 0xf0 region shift): it loads L"GR_System_Message"+L"SM" and calls
// MsgRepository::GetAndFormat with the id in edx. Hooking the WRONG fn is why the 401106 corrupted-
// save id was never seen (oracle stayed 0). This RVA must be the real getter for the semaphore.
const GR_SYSTEM_MESSAGE_RVA: u32 = 0x762d50;
const GR_SYSMSG_LOG_MAX: usize = 64;

/// DIAGNOSTIC detour for GetGR_System_Message 0x140762e40. Once the main menu has opened (skip the
/// boot-time message flood), log the integer message id (the `edx`/`rdx` arg) + first game caller RVA
/// for each call, capped. The id maps 1:1 to GR_System_Message_win64 (e.g. 4101 "Cannot connect to
/// network", 4102 "connection to game server lost", 4190 "network error", 70000 save-data notice,
/// 4191 "Failed to save game"), so the menu-open modals can be named without guessing. Read-only
/// passthrough; never mutates.
/// GR_System_Message ids the game fetches when it builds a "save data is corrupted" dialog (verified
/// from menu.msgbnd GR_System_Message_win64.fmg). 4191/4192/4193/401106 = "Failed to save game --
/// save data is corrupted"; 401721 = "Failed to load save data -- corrupted"; 401107 = "delete
/// corrupted data and create a new save?". Detecting any of these in GetGR_System_Message IS the
/// memory-read semaphore for the corrupted-save popup (privacy-policy/char-presence-CONFIRMED loop).
pub(crate) const CORRUPTED_SAVE_MSG_IDS: &[i32] = &[4191, 4192, 4193, 401106, 401107, 401721];
/// The corrupted-save message id last seen (0 = none). Exposed as `oracle_corrupted_save_seen_id`.
pub(crate) static CORRUPTED_SAVE_SEEN_ID: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(0);

pub(crate) unsafe extern "system" fn gr_sysmsg_log_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    // Corrupted-save SEMAPHORE: always check (independent of the menu-open-gated logging below) so a
    // load probe records the corrupted-save popup as RAM-read telemetry, not just an on-screen image.
    let msg_id_now = (rdx & 0xffff_ffff) as i32;
    if CORRUPTED_SAVE_MSG_IDS.contains(&msg_id_now)
        && CORRUPTED_SAVE_SEEN_ID.swap(msg_id_now, Ordering::SeqCst) != msg_id_now
    {
        append_autoload_debug(format_args!(
            "save-override: CORRUPTED-SAVE SEMAPHORE -- GetGR_System_Message id={msg_id_now} (save data is corrupted dialog); the gold save was read but rejected on validate/write"
        ));
    }
    if TFC_AUTO_MENU_OPENED.load(Ordering::SeqCst) != 0 {
        let n = GR_SYSMSG_LOG_COUNT.fetch_add(1, Ordering::SeqCst);
        if n < GR_SYSMSG_LOG_MAX {
            let msg_id = (rdx & 0xffff_ffff) as i32;
            let caller_rva = trace_first_game_caller_rva();
            append_autoload_debug(format_args!(
                "grsysmsg #{n}: id={msg_id} caller_rva=0x{caller_rva:x} out=0x{rcx:x}"
            ));
        }
    }
    let orig = GR_SYSMSG_LOG_ORIG.load(Ordering::SeqCst);
    if orig == TITLE_OWNER_SCAN_START_ADDRESS {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { f(rcx, rdx, r8, r9) }
}

/// Install the GR_System_Message id-logger once (MinHook on 0x140762e40), mirroring the auto-accept
/// builder-hook precedent. Caller-gated by `grsysmsg_log_enabled()`.
pub(crate) fn install_gr_sysmsg_log_hook() {
    if GR_SYSMSG_LOG_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "grsysmsg-log: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(GR_SYSTEM_MESSAGE_RVA) else {
        append_autoload_debug(format_args!("grsysmsg-log: failed to resolve rva"));
        return;
    };
    match unsafe { MhHook::new(addr as *mut c_void, gr_sysmsg_log_hook as *mut c_void) } {
        Ok(hook) => {
            GR_SYSMSG_LOG_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "grsysmsg-log: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    append_autoload_debug(format_args!(
                        "grsysmsg-log: hooked GetGR_System_Message 0x{addr:x} (log id+caller after menu-open)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "grsysmsg-log: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => {
            append_autoload_debug(format_args!("grsysmsg-log: MhHook::new failed: {status:?}"))
        }
    }
}

/// CS::NetworkCheckJob::Run RVA (deobf entry 0x140821310). Signature
/// `MenuJobResult*(rcx=job, rdx=MenuJobResult* result, r8=FD4Time*)`. Entry prologue
/// (push rbp/rsi/rdi/r14/r15; lea rbp; sub rsp) is a clean MinHook target (disasm-verified).
const NETWORK_CHECK_JOB_RUN_RVA: u32 = 0x821310;
/// `FD4::FD4TimeTemplate<float>::vftable` (deobf 0x1429c8e48) -- the value Run's common-return path
/// writes to `*(param_3)` in every leaf (RVA read from the deobf disasm of the clean leaf).
const FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA: usize = 0x29c8e48;
/// `MenuJobState::Continue` (the no-modal result), verified from the deobf clean leaf (`lea edx,[r8+1]`).
const MENU_JOB_STATE_CONTINUE: i32 = 1;

static NETWORK_CHECK_SHORTCIRCUIT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static NETWORK_CHECK_SHORTCIRCUIT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// THE MILESTONE-3 FIX (zero-input, save-safe). `CS::NetworkCheckJob::Run` is a title-flow MenuJob the
/// TitleTopDialog registrar chains UNCONDITIONALLY at menu-open. Offline, its Steam-holder check
/// (FUN_140cab320: all 3 holders field@0x10==2) and EOS check (FUN_140ddfb90) never pass, so every
/// decision-tree leaf builds a GR_System_Message MessageBoxDialog -- EXCEPT one leaf that does
/// `MenuJobResult::SetResult(Continue)` with no modal (decompile-verified). This detour REPLACES Run
/// with exactly that clean leaf, skipping the entire tree, so ZERO modals are ever enqueued regardless
/// of CSNetMan/CSCheatEOS readiness. The original is never called (its only outputs are the result +
/// the FD4Time vtable, both replicated). No input, no save write; only armed when offline is forced,
/// so it never alters an online (Seamless Co-op) network check. bd er-effects-rs-0ye.
pub(crate) unsafe extern "system" fn network_check_job_run_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let result = rdx;
    // MenuJobResult::SetResult(result, Continue, 0): state @ +0 (i32), field1 @ +4 (i32). The native
    // SetResult 0x1407a91e0 only writes these two fields, so replicate inline. Readability-guarded.
    if result > null && unsafe { safe_read_usize(result) }.is_some() {
        unsafe {
            *(result as *mut i32) = MENU_JOB_STATE_CONTINUE;
            *((result + 4) as *mut i32) = 0;
        }
    }
    // param_3->base._vfptr = FD4::FD4TimeTemplate<float>::vftable (Run's common-return sets this).
    if let Ok(base) = game_module_base() {
        if r8 > null && unsafe { safe_read_usize(r8) }.is_some() {
            unsafe { *(r8 as *mut usize) = base + FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA };
        }
    }
    if NETWORK_CHECK_SHORTCIRCUIT_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) == null {
        append_autoload_debug(format_args!(
            "network-check-shortcircuit: forced CS::NetworkCheckJob::Run -> MenuJobResult(Continue) result=0x{rdx:x} fd4time=0x{r8:x} -- no GR_System_Message modal enqueued (offline)"
        ));
    }
    let _ = (rcx, r9);
    result
}

/// Install the NetworkCheckJob::Run short-circuit ONCE (MinHook on 0x140821310), mirroring the
/// auto-accept builder-hook precedent. Must arm before menu-open; caller-gated (offline only).
pub(crate) fn install_network_check_shortcircuit_hook() {
    if NETWORK_CHECK_SHORTCIRCUIT_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "network-check-shortcircuit: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(NETWORK_CHECK_JOB_RUN_RVA) else {
        append_autoload_debug(format_args!(
            "network-check-shortcircuit: failed to resolve rva"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            network_check_job_run_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "network-check-shortcircuit: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    append_autoload_debug(format_args!(
                        "network-check-shortcircuit: hooked CS::NetworkCheckJob::Run 0x{addr:x} -- offline modal suppression armed"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "network-check-shortcircuit: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "network-check-shortcircuit: MhHook::new failed: {status:?}"
        )),
    }
}

/// CS::ShowProgressJob::Run RVA (deobf entry 0x1408349c0; dump 0x140834ab0, region shift -0xf0,
/// clean prologue disasm-verified). Signature `MenuJobResult*(rcx=ShowProgressJob, rdx=MenuJobResult*
/// result, r8=FD4Time*)` -- IDENTICAL to NetworkCheckJob::Run.
const SHOW_PROGRESS_JOB_RUN_RVA: u32 = 0x8349c0;
/// `MenuJobState::Success` (=2; Continue=1). Verified from FUN_1407a7340's `SetResult(.,Success,0)`
/// clean leaf (deobf `lea edx,[r8+2]`). A passing check returns Success -> `ShouldContinue` (state>1)
/// true -> ShowProgressJob::Run propagates it -> flow ADVANCES (no modal). Forcing Continue(1) would
/// loop the timed job; Success(2) completes it cleanly.
const MENU_JOB_STATE_SUCCESS: i32 = 2;

static SHOW_PROGRESS_SHORTCIRCUIT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static SHOW_PROGRESS_SHORTCIRCUIT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Original CS::ShowProgressJob::Run trampoline (MinHook). Needed so the SAVE-data progressType can be
/// PASSED THROUGH to its real delegate -- that delegate IS the boot ProfileSummary read (SLLoadSession
/// -> ER0000.sl2). Blanket-suppressing every type (the prior behavior) killed the save read, leaving
/// an empty profile -> Bandai privacy policy. bd boot-profile-read-STEP_InitMenu-blocked-by-showprogress-shortcircuit-2026-06-23.
static SHOW_PROGRESS_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// ShowProgressJob progressType at [job+0x18] (RE-confirmed). 10 = save-data check/load (MUST run its
/// delegate); 20=network, 30/31=sign-in, 60=login (offline-modal types we still short-circuit).
const SHOW_PROGRESS_TYPE_OFFSET: usize = 0x18;
const SHOW_PROGRESS_SAVE_TYPE: u32 = 10;
static SHOW_PROGRESS_TYPE_LOGGED: AtomicUsize = AtomicUsize::new(0);

/// THE MILESTONE-3 FIX, part 2 (zero-input, save-safe). `CS::ShowProgressJob::Run` (deobf 0x1408349c0)
/// is the SHARED Run for the offline title-flow check steps (save=10/network=20/sign-in=30,31/
/// login=60) the registrar chains at menu-open. Each runs a check delegate (job+0x20, slot +0x10);
/// offline the delegate returns an ERROR result, which ShowProgressJob::Run propagates so the pump
/// enqueues a GR_System_Message MessageBox. The 3 observed menu-open modals all come from these
/// ShowProgressJobs (NOT NetworkCheckJob, which is a separate job already hooked). This detour REPLACES
/// Run with a passing-check exit: result = {state=Success, field1=0} (exactly what FUN_1407a7340's
/// SetResult(Success) clean leaf yields) + the FD4Time vtable, skipping the delegate -> the job
/// completes successfully, the flow advances, and ZERO modals are enqueued. One hook covers all the
/// check steps. Offline-gated (no effect on an online Seamless Co-op check). bd er-effects-rs-0ye.
pub(crate) unsafe extern "system" fn show_progress_job_run_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let result = rdx;
    // progressType ([job+0x18], low 32 bits). 10 = the SAVE-data check/load: its delegate is the boot
    // ProfileSummary read, so it MUST run -- pass it through to the original. Suppressing it (as the
    // prior blanket short-circuit did) leaves the profile empty -> privacy policy, and the save is
    // never read. All other types (network/sign-in/login) still get the Success short-circuit so the
    // offline connection modals stay suppressed.
    let ptype = if rcx > null {
        unsafe { safe_read_usize(rcx + SHOW_PROGRESS_TYPE_OFFSET) }
            .map(|v| (v & 0xffff_ffff) as u32)
    } else {
        None
    };
    let raw10 = if rcx > null {
        unsafe { safe_read_usize(rcx + 0x10) }
    } else {
        None
    };
    let d = SHOW_PROGRESS_TYPE_LOGGED.fetch_add(1, Ordering::SeqCst);
    if d < 16 {
        append_autoload_debug(format_args!(
            "show-progress: progressType[+0x18]={ptype:?} field[+0x10]={raw10:x?} result=0x{rdx:x} (save_type={SHOW_PROGRESS_SAVE_TYPE})"
        ));
    }
    if ptype == Some(SHOW_PROGRESS_SAVE_TYPE) {
        let orig = SHOW_PROGRESS_ORIG.load(Ordering::SeqCst);
        if orig != HOOK_ORIGINAL_UNSET {
            if d < 16 {
                append_autoload_debug(format_args!(
                    "show-progress: PASS-THROUGH save-data progressType {SHOW_PROGRESS_SAVE_TYPE} -> original delegate (boot ProfileSummary read fires)"
                ));
            }
            let call: unsafe extern "system" fn(usize, usize, usize, usize) -> usize = unsafe {
                std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(usize, usize, usize, usize) -> usize,
                >(orig)
            };
            return unsafe { call(rcx, rdx, r8, r9) };
        }
    }
    if result > null && unsafe { safe_read_usize(result) }.is_some() {
        unsafe {
            *(result as *mut i32) = MENU_JOB_STATE_SUCCESS;
            *((result + 4) as *mut i32) = 0;
        }
    }
    if let Ok(base) = game_module_base() {
        if r8 > null && unsafe { safe_read_usize(r8) }.is_some() {
            unsafe { *(r8 as *mut usize) = base + FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA };
        }
    }
    if SHOW_PROGRESS_SHORTCIRCUIT_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) == null {
        append_autoload_debug(format_args!(
            "show-progress-shortcircuit: forced CS::ShowProgressJob::Run -> MenuJobResult(Success) result=0x{rdx:x} fd4time=0x{r8:x} -- offline title-flow check modal(s) suppressed at the shared chokepoint"
        ));
    }
    let _ = (rcx, r9);
    result
}

/// Install the ShowProgressJob::Run short-circuit ONCE (MinHook on 0x1408349c0). Must arm before
/// menu-open; caller-gated (offline only).
pub(crate) fn install_show_progress_shortcircuit_hook() {
    if SHOW_PROGRESS_SHORTCIRCUIT_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "show-progress-shortcircuit: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SHOW_PROGRESS_JOB_RUN_RVA) else {
        append_autoload_debug(format_args!(
            "show-progress-shortcircuit: failed to resolve rva"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            show_progress_job_run_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            // Store the trampoline BEFORE enabling so the SAVE-data progressType can be passed through
            // to the original delegate (the boot ProfileSummary read).
            SHOW_PROGRESS_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "show-progress-shortcircuit: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    append_autoload_debug(format_args!(
                        "show-progress-shortcircuit: hooked CS::ShowProgressJob::Run 0x{addr:x} -- save-type passthrough + offline-check modal suppression armed"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "show-progress-shortcircuit: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "show-progress-shortcircuit: MhHook::new failed: {status:?}"
        )),
    }
}

/// LATCH detour for the CS::SceneObjProxy ctor 0x14074a700 (rcx=proxy[this], rdx=MenuWindow*,
/// r8/r9 forwarded). Disasm-verified: the ctor does `mov %rdx,%rbx` (0x14074a720) then
/// `mov %rbx,0x20(%rsi)` (0x14074a735) -- so the incoming RDX is the engine-verified MenuWindow it
/// stores at proxy+0x20 (probe-6 proved the OLD TitleTopDialog-factory rdx was a std::function
/// delegate, NOT the MenuWindow). Runtime showed the old MenuWindow/MenuWindowProxy vtable constants
/// are stale for this ctor's engine-provided rdx, but static disassembly still proves the game stores
/// rdx as proxy+0x20. Treat the engine-provided heap-aligned rdx as the trust boundary and OVERWRITE
/// LATCHED_MENU_WINDOW on EVERY valid call (most-recent live host window wins -- the title's host
/// window is latched by the time STAGE2 runs). Then pure passthrough: call the original trampoline
/// with ALL args preserved + return its result, never perturbing the build.
/// bd live-dialog-probe6-factory-fires-returns-dialog-rdx-not-menuwindow-2026.
pub(crate) unsafe extern "system" fn scene_obj_proxy_ctor_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    const CANDIDATE_ALIGNED: usize = 0;
    const HEAP_LO: usize = 0x10000;
    const PTR_ALIGN_MASK: usize = 0x7;
    const SCENE_OBJ_PROXY_CTOR_LOG_MAX: usize = 32;
    const SCENE_OBJ_PROXY_CTOR_HIT_INC: usize = 1;
    static SCENE_OBJ_PROXY_CTOR_HITS: AtomicUsize = AtomicUsize::new(0);

    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let menu_window = rdx;
    let hit = SCENE_OBJ_PROXY_CTOR_HITS.fetch_add(SCENE_OBJ_PROXY_CTOR_HIT_INC, Ordering::SeqCst);
    let pvt = unsafe { safe_read_usize(menu_window) }.unwrap_or(null);
    if menu_window != null
        && menu_window >= HEAP_LO
        && (menu_window & PTR_ALIGN_MASK) == CANDIDATE_ALIGNED
    {
        LATCHED_MENU_WINDOW.store(menu_window, Ordering::SeqCst);
        if hit < SCENE_OBJ_PROXY_CTOR_LOG_MAX {
            append_autoload_debug(format_args!(
                "menuwindow-latch: 0x14074a700 ACCEPT #{hit} rdx=0x{menu_window:x} first=0x{pvt:x} (engine-stored proxy+0x20 candidate)"
            ));
        }
    } else if hit < SCENE_OBJ_PROXY_CTOR_LOG_MAX {
        append_autoload_debug(format_args!(
            "menuwindow-latch: 0x14074a700 REJECT #{hit} rdx=0x{menu_window:x} first=0x{pvt:x} (not heap-aligned)"
        ));
    }
    let orig = SCENE_OBJ_PROXY_CTOR_ORIG.load(Ordering::SeqCst);
    if orig == null {
        return null;
    }
    let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { f(rcx, rdx, r8, r9) }
}

/// Detour for BeginTitle's `05_000_Title` visual wrapper (deobf 0x14081f9f0). Static RE shows the
/// wrapper constructs a CSScaleformLoadInfo with filename `05_000_Title` and calls factory
/// 0x1407acbf0 to allocate/return a MenuWindowJob. For the title-cover masquerade we now preserve
/// that native MenuWindowJob and only latch it for the render-only FadeIn suppressor below. This keeps
/// TitleStep, FixOrderJobSequence, native Continue, STEP_PlayGame, and the resident-UI CSMenuMan+0x21
/// gate untouched; the draw bit is cleared later only for this preserved native title window.
pub(crate) unsafe extern "system" fn title_native_menu_visual_begin_title_hook(
    out_slot: usize,
    rdx: usize,
    r8: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let base = game_module_base().unwrap_or(null);
    let prev_out = if out_slot != null {
        unsafe { safe_read_usize(out_slot) }.unwrap_or(null)
    } else {
        null
    };
    let caller_rva = trace_first_game_caller_rva();
    TITLE_NATIVE_MENU_VISUAL_SUPPRESSED_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_LAST_OUT_SLOT.store(out_slot, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_LAST_PREV_OUT.store(prev_out, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_LAST_ARG_RDX.store(rdx, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_LAST_ARG_R8.store(r8, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);

    let orig = TITLE_NATIVE_MENU_VISUAL_SUPPRESS_ORIG.load(Ordering::SeqCst);
    let mut native_ret = out_slot;
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let native_wrapper: unsafe extern "system" fn(usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        native_ret = unsafe { native_wrapper(out_slot, rdx, r8) };
    }
    let native_job = if out_slot != null {
        unsafe { safe_read_usize(out_slot) }.unwrap_or(null)
    } else {
        null
    };
    let native_window = if native_job != null {
        unsafe { safe_read_usize(native_job + 0x130) }.unwrap_or(null)
    } else {
        null
    };
    TITLE_NATIVE_MENU_VISUAL_NATIVE_JOB.store(native_job, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_NATIVE_WINDOW.store(native_window, Ordering::SeqCst);

    // Part B probe/warmup: construct the existing ProfileSelect dummy-profile surface through the
    // native wrapper into a private slot, but do not return or install it in the BeginTitle out slot.
    // Static RE of FUN_14081f7e0 shows this wrapper only prepares CSScaleformLoadInfo for
    // `05_010_ProfileSelect` and forwards to the same MenuWindowJob factory; preserving `native_job`
    // keeps the native title/Continue sequence authoritative while making the custom-cover resource
    // path observable for telemetry.
    if base != null {
        let mut cover_slot = null;
        let cover_builder: unsafe extern "system" fn(usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(base + TITLE_CUSTOM_COVER_PROFILE_SELECT_WRAPPER_RVA) };
        let cover_ret = unsafe { cover_builder((&raw mut cover_slot) as usize, rdx, r8) };
        let cover_job = cover_slot;
        TITLE_CUSTOM_COVER_PROFILE_SELECT_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_RET.store(cover_ret, Ordering::SeqCst);
        TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_JOB.store(cover_job, Ordering::SeqCst);
        TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "title-cover-part-b: BUILT non-returned custom cover {TITLE_CUSTOM_COVER_PROFILE_SELECT_NAME} via 0x{:x} -> ret=0x{cover_ret:x} job=0x{cover_job:x}; dummy={TITLE_CUSTOM_COVER_DUMMY_PROFILE_SYMBOL} target={TITLE_CUSTOM_COVER_SYSTEX_TARGET} renderer={TITLE_CUSTOM_COVER_PROFILE_RENDERER_CLASS}; native {TITLE_NATIVE_MENU_VISUAL_NAME} job remains authoritative",
            base + TITLE_CUSTOM_COVER_PROFILE_SELECT_WRAPPER_RVA,
        ));
    }

    append_autoload_debug(format_args!(
        "title-cover-part-a: PRESERVED native {TITLE_NATIVE_MENU_VISUAL_NAME} wrapper 0x{:x}/factory 0x{:x}; latched job=0x{native_job:x} window=0x{native_window:x} for render-only suppression (out_slot=0x{out_slot:x} prev=0x{prev_out:x} rdx=0x{rdx:x} r8=0x{r8:x} caller_rva=0x{caller_rva:x})",
        base + TITLE_NATIVE_MENU_VISUAL_BEGIN_TITLE_RVA,
        base + TITLE_NATIVE_MENU_VISUAL_FACTORY_RVA,
    ));
    native_ret
}

pub(crate) unsafe extern "system" fn title_native_menu_visual_window_fadein_hook(
    window: usize,
    param_2: usize,
    param_3: usize,
    param_4: usize,
) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let native_fadein: unsafe extern "system" fn(usize, usize, usize, usize) =
            unsafe { std::mem::transmute(orig) };
        unsafe { native_fadein(window, param_2, param_3, param_4) };
    }

    let caller_rva = trace_first_game_caller_rva();
    // Do not gate on the caller RVA here: MinHook/trampoline unwinding can hide the direct
    // MenuWindowJob::Run return address. The preserved native window pointer is the stronger RAM
    // identity oracle, and the caller RVA remains telemetry only.
    let native_job = TITLE_NATIVE_MENU_VISUAL_NATIVE_JOB.load(Ordering::SeqCst);
    let mut native_window = TITLE_NATIVE_MENU_VISUAL_NATIVE_WINDOW.load(Ordering::SeqCst);
    if native_window == null && native_job != null {
        native_window = unsafe { safe_read_usize(native_job + 0x130) }.unwrap_or(null);
        TITLE_NATIVE_MENU_VISUAL_NATIVE_WINDOW.store(native_window, Ordering::SeqCst);
    }
    if native_window == null || window != native_window {
        return;
    }

    let Some(menu_id) = (unsafe { safe_read_u16(window + 0x180) }) else {
        return;
    };
    if menu_id >= 0x47 {
        return;
    }
    let base = game_module_base().unwrap_or(null);
    let cs_menu_man = if base != null {
        unsafe { safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }.unwrap_or(null)
    } else {
        null
    };
    if cs_menu_man == null {
        return;
    }
    let flags_addr = cs_menu_man + 0x90 + menu_id as usize;
    let Some(flags_before) = (unsafe { safe_read_u8(flags_addr) }) else {
        return;
    };
    let flags_after = flags_before & !TITLE_NATIVE_MENU_VISUAL_VISIBLE_FLAGS_MASK;
    if flags_after == flags_before {
        return;
    }
    unsafe { (flags_addr as *mut u8).write_volatile(flags_after) };
    TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESSED_WINDOWS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_WINDOW.store(window, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_FLAGS_BEFORE.store(flags_before as usize, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_FLAGS_AFTER.store(flags_after as usize, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-a: render-suppressed preserved native {TITLE_NATIVE_MENU_VISUAL_NAME} window=0x{window:x} menu_id={menu_id} flags 0x{flags_before:02x}->0x{flags_after:02x} via CSMenuMan+0x90 caller_rva=0x{caller_rva:x}"
    ));
}

/// Install the Part-A title visual suppression hook once. It must run at process attach before
/// STEP_BeginTitle; installing from the recurring game task can be too late for the first title build.
pub(crate) fn install_title_native_menu_visual_suppression_hook() {
    if TITLE_NATIVE_MENU_VISUAL_SUPPRESS_INSTALLED.load(Ordering::SeqCst)
        != TITLE_NATIVE_MENU_VISUAL_SUPPRESS_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-a: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(begin_title_addr) = game_rva(TITLE_NATIVE_MENU_VISUAL_BEGIN_TITLE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-cover-part-a: failed to resolve BeginTitle visual wrapper rva 0x{TITLE_NATIVE_MENU_VISUAL_BEGIN_TITLE_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            begin_title_addr as *mut c_void,
            title_native_menu_visual_begin_title_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_NATIVE_MENU_VISUAL_SUPPRESS_ORIG
                .store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-a: queue_enable BeginTitle wrapper failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    TITLE_NATIVE_MENU_VISUAL_SUPPRESS_INSTALLED.store(
                        TITLE_NATIVE_MENU_VISUAL_SUPPRESS_INSTALLED_YES,
                        Ordering::SeqCst,
                    );
                    append_autoload_debug(format_args!(
                        "title-cover-part-a: hooked BeginTitle visual wrapper 0x{begin_title_addr:x}; native {TITLE_NATIVE_MENU_VISUAL_NAME} MenuWindowJob will be replaced by {TITLE_CUSTOM_COVER_PROFILE_SELECT_NAME}, STEP_Wait/CSMenuMan+0x21 untouched"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-a: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-a: MhHook::new BeginTitle wrapper failed: {status:?}"
        )),
    }
}

pub(crate) fn install_title_native_menu_visual_render_suppression_hook() {
    if TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED.load(Ordering::SeqCst)
        != TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-a: render MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(fadein_addr) = game_rva(TITLE_NATIVE_MENU_VISUAL_WINDOW_FADEIN_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-cover-part-a: failed to resolve MenuWindowJob FadeIn helper rva 0x{TITLE_NATIVE_MENU_VISUAL_WINDOW_FADEIN_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            fadein_addr as *mut c_void,
            title_native_menu_visual_window_fadein_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_ORIG
                .store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-a: queue_enable FadeIn helper failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED.store(
                        TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED_YES,
                        Ordering::SeqCst,
                    );
                    append_autoload_debug(format_args!(
                        "title-cover-part-a: hooked MenuWindowJob FadeIn helper 0x{fadein_addr:x}; preserved native {TITLE_NATIVE_MENU_VISUAL_NAME} will clear visible flags mask 0x{TITLE_NATIVE_MENU_VISUAL_VISIBLE_FLAGS_MASK:x} from CSMenuMan+0x90 when Run returns at rva 0x{TITLE_NATIVE_MENU_VISUAL_WINDOW_FADEIN_RUN_CALLER_RVA:x}"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-a: render MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-a: MhHook::new FadeIn helper failed: {status:?}"
        )),
    }
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
