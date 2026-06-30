//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

#![allow(unused_imports)]

use std::{
    ffi::{CStr, c_void},
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

static TITLE_SCALEFORM_MEMORY_GFX: OnceLock<Vec<u8>> = OnceLock::new();
static TITLE_SCALEFORM_05_000_MEMORY_GFX: OnceLock<Vec<u8>> = OnceLock::new();

fn load_memory_gfx_from_env(var: &str, slot: &OnceLock<Vec<u8>>, label: &str) {
    let Ok(path) = std::env::var(var) else {
        return;
    };
    let trimmed = path.trim();
    if trimmed.is_empty() || slot.get().is_some() {
        return;
    }
    let embedded_bytes = if trimmed.eq_ignore_ascii_case("embedded:title-05-000-suppressed") {
        Some(TITLE_05_000_TEXT_SUPPRESSED_GFX)
    } else if trimmed.eq_ignore_ascii_case("embedded:minimal-magenta") {
        Some(TITLE_MINIMAL_MAGENTA_GFX)
    } else if trimmed.eq_ignore_ascii_case("embedded:minimal-magenta-counter") {
        Some(TITLE_MINIMAL_MAGENTA_COUNTER_GFX)
    } else {
        None
    };
    if let Some(bytes) = embedded_bytes {
        TITLE_SCALEFORM_MEMORY_GFX_BYTES.fetch_add(bytes.len(), Ordering::SeqCst);
        let _ = slot.set(bytes.to_vec());
        append_autoload_debug(format_args!(
            "title-resource-observer: loaded embedded memory-backed {label} selector='{}' bytes={}",
            trimmed,
            slot.get().map(|bytes| bytes.len()).unwrap_or(0)
        ));
        return;
    }
    match fs::read(trimmed) {
        Ok(bytes) if bytes.starts_with(b"GFX") => {
            TITLE_SCALEFORM_MEMORY_GFX_BYTES.fetch_add(bytes.len(), Ordering::SeqCst);
            let _ = slot.set(bytes);
            append_autoload_debug(format_args!(
                "title-resource-observer: loaded memory-backed {label} bytes={} from '{}'",
                slot.get().map(|bytes| bytes.len()).unwrap_or(0),
                trimmed
            ));
        }
        Ok(bytes) => {
            TITLE_SCALEFORM_MEMORY_GFX_FAILURES.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "title-resource-observer: refused memory-backed {label} '{}' bytes={} (missing GFX magic)",
                trimmed,
                bytes.len()
            ));
        }
        Err(err) => {
            TITLE_SCALEFORM_MEMORY_GFX_FAILURES.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "title-resource-observer: failed to read memory-backed {label} '{}': {err}",
                trimmed
            ));
        }
    }
}

fn load_title_scaleform_memory_gfx() {
    load_memory_gfx_from_env(
        "ER_EFFECTS_TITLE_RESOURCE_MEMORY_GFX",
        &TITLE_SCALEFORM_MEMORY_GFX,
        "05_001_title_logo GFX",
    );
    load_memory_gfx_from_env(
        "ER_EFFECTS_TITLE_05_000_MEMORY_GFX",
        &TITLE_SCALEFORM_05_000_MEMORY_GFX,
        "05_000_title GFX",
    );
}

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
    // Always exit to the no-modal Continue (offline modal suppression). This job is REPLACED either
    // way, so no real check / modal runs -> save-safe + online-safe.
    //
    // REGRESSION FIX (2026-06-30): a prior "PORTRAIT HOLD" held this job in a RUNNING state (>1, so
    // MenuJobResult::ShouldContinue keeps it polling) until the menu portrait was captured. That was
    // self-defeating: holding NetworkCheckJob stalls the title-flow check chain, so the SAVE-data
    // ShowProgressJob (the boot ProfileSummary read) never runs -> the profile stays empty -> the
    // autoload starts a NEW GAME instead of loading the real character (and the stalled flow crashed
    // the world-load). The hold waited on a capture that could not happen until the read it was
    // blocking completed. Runtime-confirmed: with the hold gone, the boot read fires (showprog PASS),
    // the real character loads, and the world reaches `player_present`. The portrait-capture timing is
    // owned DOWNSTREAM by `portrait_render_window` instead, which holds the load COMMIT after menu-open
    // (i.e. AFTER the boot read has populated the slot). bd autoload-regression-lookat-breaks-bootread-2026-06-30.
    let state = MENU_JOB_STATE_CONTINUE;
    // MenuJobResult::SetResult(result, state, 0): state @ +0 (i32), field1 @ +4 (i32). The native
    // SetResult 0x1407a91e0 only writes these two fields, so replicate inline. Readability-guarded.
    if result > null && unsafe { safe_read_usize(result) }.is_some() {
        unsafe {
            *(result as *mut i32) = state;
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

unsafe fn build_profile_select_cover_job(
    base: usize,
    rdx: usize,
    r8: usize,
    caller_rva: usize,
    source: &str,
) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == null || base == 0 {
        return;
    }
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
        "title-cover-part-b: BUILT non-returned custom cover {TITLE_CUSTOM_COVER_PROFILE_SELECT_NAME} via 0x{:x} from {source} -> ret=0x{cover_ret:x} job=0x{cover_job:x}; dummy={TITLE_CUSTOM_COVER_DUMMY_PROFILE_SYMBOL} target={TITLE_CUSTOM_COVER_SYSTEX_TARGET} renderer={TITLE_CUSTOM_COVER_PROFILE_RENDERER_CLASS}",
        base + TITLE_CUSTOM_COVER_PROFILE_SELECT_WRAPPER_RVA,
    ));
}

unsafe fn build_black_cover_job(base: usize, rdx: usize, caller_rva: usize, source: &str) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == null || base == 0 {
        return;
    }
    if TITLE_CUSTOM_COVER_BLACK_BUILDS.load(Ordering::SeqCst) != 0 {
        return;
    }
    let mut cover_slot = null;
    let cover_builder: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(base + TITLE_CUSTOM_COVER_BLACK_WRAPPER_RVA) };
    let cover_ret = unsafe { cover_builder((&raw mut cover_slot) as usize, rdx) };
    let cover_job = cover_slot;
    TITLE_CUSTOM_COVER_BLACK_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_BLACK_LAST_RET.store(cover_ret, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_BLACK_LAST_JOB.store(cover_job, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_BLACK_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-b: BUILT non-returned custom black cover {TITLE_CUSTOM_COVER_BLACK_NAME} via 0x{:x} from {source} -> ret=0x{cover_ret:x} job=0x{cover_job:x}; will be pumped above native title/PAB jobs",
        base + TITLE_CUSTOM_COVER_BLACK_WRAPPER_RVA,
    ));
}

pub(crate) unsafe extern "system" fn title_pab_information_visual_hook(
    out_slot: usize,
    rdx: usize,
    r8: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let base = game_module_base().unwrap_or(null);
    let caller_rva = trace_first_game_caller_rva();
    let orig = TITLE_PAB_INFORMATION_VISUAL_ORIG.load(Ordering::SeqCst);
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
    TITLE_PAB_INFORMATION_VISUAL_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    TITLE_PAB_INFORMATION_VISUAL_LAST_JOB.store(native_job, Ordering::SeqCst);
    TITLE_PAB_INFORMATION_VISUAL_LAST_WINDOW.store(native_window, Ordering::SeqCst);
    TITLE_PAB_INFORMATION_VISUAL_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-a: PRESERVED native {TITLE_PAB_INFORMATION_VISUAL_NAME} wrapper 0x{:x}; latched job=0x{native_job:x} window=0x{native_window:x} for PAB cover (out_slot=0x{out_slot:x} rdx=0x{rdx:x} r8=0x{r8:x} caller_rva=0x{caller_rva:x})",
        base + TITLE_NATIVE_MENU_VISUAL_TITLE_INFORMATION_RVA,
    ));
    native_ret
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
    append_autoload_debug(format_args!(
        "title-cover-part-b: independent 01_900_Black build disabled; prior pump proof stalled title flow (no completion epilogue)"
    ));

    append_autoload_debug(format_args!(
        "title-cover-part-a: PRESERVED native {TITLE_NATIVE_MENU_VISUAL_NAME} wrapper 0x{:x}/factory 0x{:x}; latched job=0x{native_job:x} window=0x{native_window:x} for render-only suppression (out_slot=0x{out_slot:x} prev=0x{prev_out:x} rdx=0x{rdx:x} r8=0x{r8:x} caller_rva=0x{caller_rva:x})",
        base + TITLE_NATIVE_MENU_VISUAL_BEGIN_TITLE_RVA,
        base + TITLE_NATIVE_MENU_VISUAL_FACTORY_RVA,
    ));
    native_ret
}

unsafe fn force_hide_title_logo_surface(
    base: usize,
    logo: usize,
    requested_visible: usize,
    source: &str,
) {
    if base == TITLE_OWNER_SCAN_START_ADDRESS
        || base == 0
        || logo == 0
        || logo == TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    let orig = TITLE_LOGO_SET_VISIBLE_ORIG.load(Ordering::SeqCst);
    let set_visible: unsafe extern "system" fn(usize, u8) =
        if orig != 0 && orig != HOOK_ORIGINAL_UNSET {
            unsafe { std::mem::transmute(orig) }
        } else {
            unsafe { std::mem::transmute(base + TITLE_LOGO_BACK_VIEW_PARTS_SET_VISIBLE_RVA) }
        };
    unsafe { set_visible(logo, 0) };
    let calls = TITLE_LOGO_GFX_HIDE_CALLS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    TITLE_LOGO_GFX_HIDE_LAST_LOGO.store(logo, Ordering::SeqCst);
    TITLE_LOGO_GFX_HIDE_LAST_CALLER_PHASE
        .store(OWN_STEPPER_PHASE.load(Ordering::SeqCst), Ordering::SeqCst);
    TITLE_LOGO_GFX_HIDE_LAST_REQUESTED_VISIBLE.store(requested_visible, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-a: forced {TITLE_LOGO_BACK_VIEW_PARTS_NAME}/{TITLE_LOGO_RESOURCE_NAME} hidden via {source} logo=0x{logo:x} requested_visible={requested_visible} hide_calls={calls}"
    ));
}

pub(crate) unsafe extern "system" fn title_logo_set_visible_force_hidden_hook(
    logo: usize,
    visible: u8,
) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let base = game_module_base().unwrap_or(null);
    unsafe { force_hide_title_logo_surface(base, logo, visible as usize, "SetVisible detour") };
}

pub(crate) unsafe extern "system" fn title_logo_ctor_force_hidden_hook(
    logo: usize,
    resource: usize,
    param_3: usize,
    param_4: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let base = game_module_base().unwrap_or(null);
    let orig = TITLE_LOGO_CTOR_ORIG.load(Ordering::SeqCst);
    let ret = if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(logo, resource, param_3, param_4) }
    } else {
        logo
    };
    unsafe { force_hide_title_logo_surface(base, logo, 0, "ctor detour") };
    ret
}

pub(crate) unsafe extern "system" fn title_top_start_login_hide_hook(
    dialog: usize,
    param_2: usize,
) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let base = game_module_base().unwrap_or(null);
    let orig = TITLE_TOP_START_LOGIN_HIDE_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(usize, usize) =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(dialog, param_2) };
    }
    if base == null || dialog == null || dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let logo = dialog + TITLE_LOGO_BACK_VIEW_PARTS_AA8_OFFSET;
    if unsafe { safe_read_usize(logo) }.is_none() {
        return;
    }
    let set_visible: unsafe extern "system" fn(usize, u8) =
        unsafe { std::mem::transmute(base + TITLE_LOGO_BACK_VIEW_PARTS_SET_VISIBLE_RVA) };
    unsafe { set_visible(logo, 0) };
    let calls = TITLE_LOGO_GFX_HIDE_CALLS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    TITLE_LOGO_GFX_HIDE_LAST_DIALOG.store(dialog, Ordering::SeqCst);
    TITLE_LOGO_GFX_HIDE_LAST_LOGO.store(logo, Ordering::SeqCst);
    TITLE_LOGO_GFX_HIDE_LAST_CALLER_PHASE
        .store(OWN_STEPPER_PHASE.load(Ordering::SeqCst), Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-a: hid {TITLE_LOGO_BACK_VIEW_PARTS_NAME}/{TITLE_LOGO_RESOURCE_NAME} after native TitleTopDialog start-login via 0x{:x} dialog=0x{dialog:x} logo=0x{logo:x} hide_calls={calls}",
        base + TITLE_LOGO_BACK_VIEW_PARTS_SET_VISIBLE_RVA,
    ));
}

pub(crate) unsafe extern "system" fn title_custom_cover_menu_window_run_hook(
    job: usize,
    load_params: usize,
    fd4_time: usize,
    menu_man: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = TITLE_CUSTOM_COVER_RUN_ORIG.load(Ordering::SeqCst);
    if orig == null || orig == HOOK_ORIGINAL_UNSET {
        return null;
    }
    let run: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    let ret = unsafe { run(job, load_params, fd4_time, menu_man) };
    if TITLE_CUSTOM_COVER_RUN_RECURSION.load(Ordering::SeqCst) != 0 {
        return ret;
    }
    let title_job = TITLE_NATIVE_MENU_VISUAL_NATIVE_JOB.load(Ordering::SeqCst);
    let pab_job = TITLE_PAB_INFORMATION_VISUAL_LAST_JOB.load(Ordering::SeqCst);
    let cover_job = TITLE_CUSTOM_COVER_BLACK_LAST_JOB.load(Ordering::SeqCst);
    let native_job = if job == title_job {
        title_job
    } else if job == pab_job {
        pab_job
    } else {
        null
    };
    if native_job == null
        || cover_job == null
        || cover_job == TITLE_OWNER_SCAN_START_ADDRESS
        || cover_job == native_job
    {
        return ret;
    }
    TITLE_CUSTOM_COVER_RUN_RECURSION.store(1, Ordering::SeqCst);
    let cover_ret = unsafe { run(cover_job, load_params, fd4_time, menu_man) };
    TITLE_CUSTOM_COVER_RUN_RECURSION.store(0, Ordering::SeqCst);
    let cover_window = unsafe { safe_read_usize(cover_job + 0x130) }.unwrap_or(null);
    let calls = TITLE_CUSTOM_COVER_RUN_CALLS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    let profile_value = TITLE_PROFILE_FACE_LAST_VALUE.load(Ordering::SeqCst);
    if profile_value != null && profile_value != HOOK_ORIGINAL_UNSET {
        let base = game_module_base().unwrap_or(null);
        if base != null {
            let set_position: unsafe extern "system" fn(usize, f32, f32) -> usize =
                unsafe { std::mem::transmute(base + TITLE_GFX_VALUE_SET_POSITION_RVA) };
            let set_scale: unsafe extern "system" fn(usize, *const f32) -> usize =
                unsafe { std::mem::transmute(base + TITLE_GFX_VALUE_SET_SCALE_RVA) };
            let scale = [3.2f32, 3.2f32];
            append_autoload_debug(format_args!(
                "title-cover-part-b: deferred transform after custom cover value=0x{profile_value:x} calls={calls}"
            ));
            unsafe { set_position(profile_value, 640.0, 360.0) };
            unsafe { set_scale(profile_value, scale.as_ptr()) };
            TITLE_PROFILE_FACE_TRANSFORM_APPLIED.store(1, Ordering::SeqCst);
            TITLE_PROFILE_FACE_OTHER_HIDDEN.store(9, Ordering::SeqCst);
        }
    }
    TITLE_CUSTOM_COVER_RUN_LAST_NATIVE_JOB.store(native_job, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_RUN_LAST_COVER_JOB.store(cover_job, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_RUN_LAST_COVER_WINDOW.store(cover_window, Ordering::SeqCst);
    TITLE_CUSTOM_COVER_RUN_LAST_RET.store(cover_ret, Ordering::SeqCst);
    if calls <= 16 || calls.is_power_of_two() {
        append_autoload_debug(format_args!(
            "title-cover-part-b: ran custom black cover {TITLE_CUSTOM_COVER_BLACK_NAME} job=0x{cover_job:x} alongside native title/PAB job=0x{native_job:x}; ret=0x{cover_ret:x} window=0x{cover_window:x} calls={calls}"
        ));
    }
    ret
}

unsafe fn read_native_dlstring_ascii_ptr(s: usize) -> usize {
    if s == 0 || s == TITLE_OWNER_SCAN_START_ADDRESS {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    let capacity = unsafe { safe_read_usize(s + 0x20) }.unwrap_or(0);
    if capacity <= 0xf {
        s + 0x8
    } else {
        unsafe { safe_read_usize(s + 0x8) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    }
}

unsafe fn bounded_ascii_contains(ptr: usize, needle: &[u8]) -> bool {
    if ptr == 0 || ptr == TITLE_OWNER_SCAN_START_ADDRESS || needle.is_empty() {
        return false;
    }
    let mut window = [0u8; 32];
    let mut n = 0usize;
    for i in 0..96usize {
        let Some(b) = (unsafe { safe_read_u8(ptr + i) }) else {
            break;
        };
        if b == 0 {
            break;
        }
        if n < window.len() {
            window[n] = b.to_ascii_lowercase();
            n += 1;
        } else {
            window.rotate_left(1);
            window[window.len() - 1] = b.to_ascii_lowercase();
        }
        let hay = &window[..n.min(window.len())];
        if hay.windows(needle.len()).any(|w| w == needle) {
            return true;
        }
    }
    false
}

unsafe fn copy_ascii_preview(ptr: usize, out: &mut [u8]) -> usize {
    if ptr == 0 || ptr == TITLE_OWNER_SCAN_START_ADDRESS || out.is_empty() {
        return 0;
    }
    let mut n = 0usize;
    while n + 1 < out.len() && n < 80 {
        let Some(b) = (unsafe { safe_read_u8(ptr + n) }) else {
            break;
        };
        if b == 0 {
            break;
        }
        out[n] = if b.is_ascii_graphic() || b == b' ' {
            b
        } else {
            b'?'
        };
        n += 1;
    }
    n
}

unsafe fn rewrite_native_dlstring_ascii(s: usize, value: &str) -> Option<usize> {
    if s == 0 || s == TITLE_OWNER_SCAN_START_ADDRESS || !value.is_ascii() {
        return None;
    }
    let len = value.len();
    let capacity = unsafe { safe_read_usize(s + 0x20) }?;
    if capacity < len {
        return None;
    }
    let dst = if capacity <= 0xf {
        s + 0x8
    } else {
        unsafe { safe_read_usize(s + 0x8) }?
    };
    if dst == 0 || dst == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    for (idx, byte) in value.as_bytes().iter().copied().enumerate() {
        unsafe { ((dst + idx) as *mut u8).write_volatile(byte) };
    }
    unsafe { ((dst + len) as *mut u8).write_volatile(0) };
    unsafe { ((s + 0x18) as *mut usize).write_volatile(len) };
    Some(dst)
}

unsafe fn sample_now_loading_helper(this: usize) {
    if this == 0 || this == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    NOW_LOADING_HELPER_LAST_THIS.store(this, Ordering::SeqCst);
    NOW_LOADING_HELPER_LAST_MENU_INDEX.store(
        unsafe { safe_read_usize(this + 0xd0) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
        Ordering::SeqCst,
    );
    NOW_LOADING_HELPER_LAST_REPLACE_TEX_INFO.store(
        unsafe { safe_read_usize(this + 0xd8) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
        Ordering::SeqCst,
    );
    NOW_LOADING_HELPER_LAST_REQUESTED_REPLACE_TEX_INFO.store(
        unsafe { safe_read_usize(this + 0xe0) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
        Ordering::SeqCst,
    );
    let request_done = unsafe { safe_read_u8(this + 0xec) }.unwrap_or(0) as usize;
    let load_done = unsafe { safe_read_u8(this + 0xed) }.unwrap_or(0) as usize;
    NOW_LOADING_HELPER_LAST_FLAGS.store(request_done | (load_done << 8), Ordering::SeqCst);
}

pub(crate) unsafe extern "system" fn now_loading_helper_ctor_hook(this: usize) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = NOW_LOADING_HELPER_CTOR_ORIG.load(Ordering::SeqCst);
    let ret = if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(this) }
    } else {
        this
    };
    let hits = NOW_LOADING_HELPER_CTOR_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    unsafe { sample_now_loading_helper(ret) };
    if hits <= 4 {
        append_autoload_debug(format_args!(
            "title-cover-part-b: observed CSNowLoadingHelperImp ctor this=0x{ret:x} hits={hits}; now-loading surface candidate for custom masquerade"
        ));
    }
    ret
}

pub(crate) unsafe extern "system" fn now_loading_helper_update_hook(this: usize, time: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = NOW_LOADING_HELPER_UPDATE_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(usize, usize) =
            unsafe { std::mem::transmute(orig) };
        unsafe { original(this, time) };
    }
    let hits = NOW_LOADING_HELPER_UPDATE_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    unsafe { sample_now_loading_helper(this) };
    if hits <= 8 || hits.is_power_of_two() {
        append_autoload_debug(format_args!(
            "title-cover-part-b: observed CSNowLoadingHelperImp update this=0x{this:x} hits={hits} menu_index=0x{:x} replace=0x{:x} requested=0x{:x} flags=0x{:x}",
            NOW_LOADING_HELPER_LAST_MENU_INDEX.load(Ordering::SeqCst),
            NOW_LOADING_HELPER_LAST_REPLACE_TEX_INFO.load(Ordering::SeqCst),
            NOW_LOADING_HELPER_LAST_REQUESTED_REPLACE_TEX_INFO.load(Ordering::SeqCst),
            NOW_LOADING_HELPER_LAST_FLAGS.load(Ordering::SeqCst),
        ));
    }
}

pub(crate) fn install_now_loading_helper_observer_hooks() {
    if NOW_LOADING_HELPER_HOOKS_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-b: now-loading observer MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(ctor) = game_rva(NOW_LOADING_HELPER_CTOR_RVA as u32) else {
        return;
    };
    let Ok(update) = game_rva(NOW_LOADING_HELPER_UPDATE_RVA as u32) else {
        return;
    };
    let mut ok = true;
    match unsafe {
        MhHook::new(
            ctor as *mut c_void,
            now_loading_helper_ctor_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            NOW_LOADING_HELPER_CTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            ok &= unsafe { hook.queue_enable() }.is_ok();
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "title-cover-part-b: now-loading ctor hook failed: {status:?}"
            ));
            ok = false;
        }
    }
    match unsafe {
        MhHook::new(
            update as *mut c_void,
            now_loading_helper_update_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            NOW_LOADING_HELPER_UPDATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            ok &= unsafe { hook.queue_enable() }.is_ok();
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "title-cover-part-b: now-loading update hook failed: {status:?}"
            ));
            ok = false;
        }
    }
    if !ok {
        return;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            NOW_LOADING_HELPER_HOOKS_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "title-cover-part-b: hooked CSNowLoadingHelperImp observer ctor=0x{ctor:x} update=0x{update:x}; observe-only"
            ));
        }
        status => append_autoload_debug(format_args!(
            "title-cover-part-b: now-loading observer MH_ApplyQueued failed: {status:?}"
        )),
    }
}

unsafe fn wide_ascii_contains_ci(ptr: usize, needle: &[u8]) -> bool {
    if ptr == 0 || ptr == TITLE_OWNER_SCAN_START_ADDRESS || needle.is_empty() {
        return false;
    }
    let mut hay = [0u8; 128];
    let mut n = 0usize;
    while n < hay.len() {
        let Some(ch) = (unsafe { safe_read_u16(ptr + n * core::mem::size_of::<u16>()) }) else {
            break;
        };
        if ch == 0 {
            break;
        }
        hay[n] = if ch <= 0x7f {
            (ch as u8).to_ascii_lowercase()
        } else {
            b'?'
        };
        n += 1;
    }
    hay[..n].windows(needle.len()).any(|w| w == needle)
}

unsafe fn copy_wide_ascii_preview(ptr: usize, out: &mut [u8]) -> usize {
    if ptr == 0 || ptr == TITLE_OWNER_SCAN_START_ADDRESS || out.is_empty() {
        return 0;
    }
    let mut n = 0usize;
    while n + 1 < out.len() && n < 96 {
        let Some(ch) = (unsafe { safe_read_u16(ptr + n * core::mem::size_of::<u16>()) }) else {
            break;
        };
        if ch == 0 {
            break;
        }
        out[n] = if (0x20..=0x7e).contains(&ch) {
            ch as u8
        } else {
            b'?'
        };
        n += 1;
    }
    n
}

/// Read an incoming `DLString<wchar_t>` (the producer's symbol arg) into a `Vec<u16>` (no trailing
/// NUL) plus its encodingType byte. SSO-aware: the data is a heap pointer at `+0x8` when capacity
/// `> 7`, otherwise inline at `+0x8`.
unsafe fn read_dlstring_u16(s: usize) -> Option<(Vec<u16>, u8)> {
    if s == 0 || s == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let capacity = unsafe { safe_read_usize(s + DLSTRING_U16_CAPACITY_OFFSET) }?;
    let length = unsafe { safe_read_usize(s + DLSTRING_U16_LENGTH_OFFSET) }?;
    if length > 4096 {
        return None; // implausible symbol length
    }
    let data_ptr = if capacity > DLSTRING_U16_SSO_THRESHOLD {
        unsafe { safe_read_usize(s + DLSTRING_U16_INLINE_OFFSET) }?
    } else {
        s + DLSTRING_U16_INLINE_OFFSET
    };
    if data_ptr == 0 || data_ptr == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let mut out = Vec::with_capacity(length);
    for i in 0..length {
        out.push(unsafe { safe_read_u16(data_ptr + i * core::mem::size_of::<u16>()) }?);
    }
    let encoding = unsafe { safe_read_u8(s + DLSTRING_U16_ENCODING_OFFSET) }.unwrap_or(1);
    Some((out, encoding))
}

/// Extract the bare GFx background texture symbol (e.g. `MENU_Load_00008`) from a now-loading TPF
/// path symbol like `menutpfbnd:/00_Solo/MENU_Load_00008.tpf`. The pump registers this bare name into
/// the Scaleform texture repository, so it must be exactly the symbol the loading GFx resolves.
/// Returns None when the path has no `MENU_Load_` segment (i.e. not a now-loading background).
fn extract_menu_load_tex_name(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase();
    let idx = lower.find("menu_load_")?;
    // Lowercasing ASCII preserves byte indices, so `idx` is valid in the original `path`.
    let tail = path.get(idx..)?;
    let name = tail.split('.').next().unwrap_or(tail);
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Bounded ASCII preview of a UTF-16 buffer for debug logging.
fn utf16_ascii_preview(units: &[u16]) -> String {
    units
        .iter()
        .take(64)
        .map(|&u| {
            if (0x20..=0x7e).contains(&u) {
                u as u8 as char
            } else {
                '?'
            }
        })
        .collect()
}

/// Absolute address of the profile renderer table entry for `slot` (`DAT_143d6d8d0[slot]`, the
/// `CSMenuProfModelRend*` for that ABSOLUTE save slot; offscreen tex index `slot*2`). Out-of-range
/// slots fall back to entry 0, preserving the historical table[0] behavior for `slot == 0` or unknown.
pub(crate) fn portrait_renderer_table_entry(base: usize, slot: i32) -> usize {
    let idx = if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot) {
        slot as usize
    } else {
        0
    };
    base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_TABLE_RVA + idx * core::mem::size_of::<usize>()
}

/// Walk the CSMenuProfModelRend chain for `slot` to its live portrait `CSGxTexture`, or 0 if the
/// renderer/offscreen/tex-rescap chain is not present (e.g. already torn down). Read-only.
unsafe fn sample_portrait_gxtexture(base: usize, slot: i32) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let renderer =
        unsafe { safe_read_usize(portrait_renderer_table_entry(base, slot)) }.unwrap_or(0);
    if renderer == 0 || renderer == null {
        return 0;
    }
    let vt = unsafe { safe_read_usize(renderer) }.unwrap_or(0);
    if vt != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA {
        return 0;
    }
    let offscreen = unsafe {
        safe_read_usize(renderer + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
    }
    .unwrap_or(0);
    if offscreen == 0 || offscreen == null {
        return 0;
    }
    let tex_rescap = unsafe {
        safe_read_usize(offscreen + TITLE_CUSTOM_COVER_PROFILE_OFFSCREEN_TEX_RESCAP_OFFSET)
    }
    .unwrap_or(0);
    if tex_rescap == 0 || tex_rescap == null {
        return 0;
    }
    unsafe { safe_read_usize(tex_rescap + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET) }
        .unwrap_or(0)
}

/// Re-bind the LIVE offscreen-RT CSGxTexture of our post-Continue built renderer into the now-loading
/// background container that the forge already injected. The now-loading background binds ~15-17s (BEFORE
/// our renderer's RT is live) and never re-binds, so the displayed container holds the forged checker; this
/// swaps our live GX into that container's first TexResCap every tick once the RT is up, and GFx -- which
/// re-samples the bound CSGxTexture each composite frame -- then shows the live animated portrait. The
/// CSGxTexture identity is stable while our feed window keeps the renderer alive, so this is idempotent
/// once latched. Read/validate-guarded; writes only the single GX pointer slot.
unsafe fn refresh_loading_bg_live_gx(base: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    // DISABLED (crashes): binding the built-own renderer's LIVE offscreen SRV into the now-loading
    // Scaleform container makes dxgi/vkd3d AV ~330ms later when the GFx sampler reads it (run 2026-06-30:
    // RE-BOUND +18003ms -> 0xc0000005 in vkd3d at +18336ms). The offscreen SRV is a render-target resource,
    // not valid as a Scaleform shader-resource (format/descriptor/state mismatch), so the container's
    // sampler faults. Native Scaleform GX rebind is a dead end (the menu-renderer variant UAF'd; this
    // built-own variant format-faults). The SAFE display path is the present-overlay D3D12 composite
    // (CopyTextureRegion, not a sampler) fed by a per-frame READBACK of the live built SRV -- see bd
    // portrait-live-render-reattach-crashes-build-own-2026-06-30. Kept gated-off here for reference.
    if true || !portrait_render_drive_enabled() {
        return;
    }
    if PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst) == 0 {
        return;
    }
    let cap = LOADING_BG_TEXTURE_REDIRECT_LAST_PORTRAIT.load(Ordering::SeqCst);
    if !valid(cap) {
        return;
    }
    // Resolve the LIVE SRV from our built target-slot renderer: table[slot] -> +0xa8 (offscreen) -> +0x10
    // (TexResCap) -> +GX = the sampleable CSGxTexture the engine re-renders each frame. Validate the vtable
    // so a torn/rebuilding slot can't bind a bad pointer.
    let slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, slot)) }.unwrap_or(0);
    if !valid(r)
        || unsafe { safe_read_usize(r) }.unwrap_or(0)
            != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
    {
        return;
    }
    let off = unsafe {
        safe_read_usize(r + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
    }
    .unwrap_or(0);
    if !valid(off) {
        return;
    }
    let trc = unsafe {
        safe_read_usize(off + TITLE_CUSTOM_COVER_PROFILE_OFFSCREEN_TEX_RESCAP_OFFSET)
    }
    .unwrap_or(0);
    if !valid(trc) {
        return;
    }
    let bind_gx =
        unsafe { safe_read_usize(trc + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET) }
            .unwrap_or(0);
    if !valid(bind_gx) {
        return;
    }
    let container = unsafe { safe_read_usize(cap + TPF_FILE_CAP_TEX_RESCAP_OFFSET) }.unwrap_or(0);
    if !valid(container) {
        return;
    }
    let count = unsafe { safe_read_usize(container + TPF_RESCAP_CONTAINER_COUNT_OFFSET) }
        .unwrap_or(0)
        & 0xffff_ffff;
    let array =
        unsafe { safe_read_usize(container + TPF_RESCAP_CONTAINER_ARRAY_OFFSET) }.unwrap_or(0);
    if count < 1 || !valid(array) {
        return;
    }
    let tex_rescap0 = unsafe { safe_read_usize(array) }.unwrap_or(0);
    if !valid(tex_rescap0) {
        return;
    }
    let cur =
        unsafe { safe_read_usize(tex_rescap0 + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET) }
            .unwrap_or(0);
    if cur == bind_gx {
        return; // already bound to the captured RT
    }
    unsafe {
        ((tex_rescap0 + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET) as *mut usize)
            .write_volatile(bind_gx)
    };
    LOADING_BG_LIVE_GX_BOUND.store(bind_gx, Ordering::SeqCst);
    let n = LOADING_BG_LIVE_GX_REBINDS.fetch_add(1, Ordering::SeqCst) + 1;
    if n == 1 {
        append_autoload_debug(format_args!(
            "loading-portrait: RE-BOUND captured (AddRef'd) portrait RT into the now-loading container -- bind_gx=0x{bind_gx:x} (was 0x{cur:x}) cap=0x{cap:x} container=0x{container:x}; loading screen samples the lifetime-safe portrait"
        ));
    }
}

/// Per-frame: keep the spared profile renderer drawing and capture the portrait once its model
/// finishes loading. After Continue the menu-owned offscreen-draw MenuJob stops, so we drive the
/// spared renderer's offscreen render ourselves each frame (`FUN_140bb8d90`); the global ResMan task
/// keeps loading/animating the model (`renderer+0x778`) automatically. Once the model has latched and
/// the GPU texture is uploaded, AddRef the `CSGxTexture` (+ its GPU child) so it survives, and cache
/// it for the now-loading forge (the next MENU_Load rotation displays the real portrait). One-shot.
/// Diagnostic: dump the captured portrait RGBA8 to `<debug-log-dir>/portrait-capture.bin`
/// (header: b"ERPX", u32 LE width, u32 LE height, then width*height*4 RGBA8) so the agent can
/// convert it to a PNG offline and visually confirm it is the loaded character's head (not the
/// depth buffer / garbage). Best-effort; gated by the same default-OFF readback path.
fn dump_portrait_rgba(slot: i32, width: u32, height: u32, px: &[u8]) {
    let dir = std::env::var("ER_EFFECTS_AUTOLOAD_DEBUG_PATH")
        .ok()
        .and_then(|p| PathBuf::from(p).parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let name = if slot >= 0 {
        format!("portrait-capture-slot{slot}.bin")
    } else {
        "portrait-capture.bin".to_string()
    };
    let path = dir.join(&name);
    if let Ok(mut f) = fs::File::create(&path) {
        // Encode through the erpx-rs crate (single source of truth for the ERPX container header),
        // so the on-disk format can never drift from the host-side decoder/`erpx2png` tool.
        let _ = erpx_rs::write_to(&mut f, width, height, px);
        append_autoload_debug(format_args!(
            "portrait-dump: slot={slot} wrote {width}x{height} ({} bytes) -> {name}",
            px.len()
        ));
    }
}

pub(crate) fn maybe_capture_portrait_gxtexture(base: usize, slot: i32) {
    if LOADING_BG_PORTRAIT_GX_KEPT.load(Ordering::SeqCst) != 0 {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == 0 || base == null {
        return;
    }
    let valid = |p: usize| p != 0 && p != null;
    // Prefer the spared renderer (alive past Continue); before Continue use the live table slot.
    let spared = LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst);
    let renderer = if valid(spared) {
        spared
    } else {
        unsafe { safe_read_usize(portrait_renderer_table_entry(base, slot)) }.unwrap_or(0)
    };
    if !valid(renderer) {
        return;
    }
    let vt = unsafe { safe_read_usize(renderer) }.unwrap_or(0);
    if vt != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA {
        return;
    }
    // NOTE: driving the menu offscreen render (FUN_140bb8d90) post-Continue crashes during world-load
    // (g_GxDrawContext invalid out of menu phase), and the character model never loads once the menu
    // phase ends -- so the in-loading-screen drive is disabled. The real no-delay path is to make the
    // ProfileSelect portrait render during the title phase (valid menu context) and capture it before
    // Continue. The spare + capture below stay safe (read-only) and fire only if the model ever loads.
    let _ = PROFILE_OFFSCREEN_DRIVE_RVA;
    let marked =
        unsafe { safe_read_u8(renderer + PROFILE_RENDERER_MARKED_DELETE_OFFSET) }.unwrap_or(1);
    let model =
        unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
    let offscreen = unsafe {
        safe_read_usize(renderer + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
    }
    .unwrap_or(0);
    let tex_rescap = if valid(offscreen) {
        unsafe {
            safe_read_usize(offscreen + TITLE_CUSTOM_COVER_PROFILE_OFFSCREEN_TEX_RESCAP_OFFSET)
        }
        .unwrap_or(0)
    } else {
        0
    };
    let gx = if valid(tex_rescap) {
        unsafe { safe_read_usize(tex_rescap + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET) }
            .unwrap_or(0)
    } else {
        0
    };
    let gpu = if valid(gx) {
        unsafe { safe_read_usize(gx + GX_TEXTURE_GPU_RESOURCE_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    // +0x754/+0x755 are the refresh's "load-requested" idempotency flags: 1 = the async character
    // model build was kicked for this slot, 0 = never requested (the Continue path may not set up the
    // profile model data, so the portrait would never render no matter how long we wait).
    let req754 = unsafe { safe_read_u8(renderer + 0x754) }.unwrap_or(0xff);
    let req755 = unsafe { safe_read_u8(renderer + 0x755) }.unwrap_or(0xff);
    let seen = LOADING_BG_PORTRAIT_GX_CAPTURE_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if seen <= 60 && seen % 4 == 1 {
        append_autoload_debug(format_args!(
            "loading-portrait-capture: spared=0x{spared:x} renderer=0x{renderer:x} marked={marked} req754={req754} req755={req755} model=0x{model:x} gx=0x{gx:x} gpu=0x{gpu:x} seen={seen}"
        ));
    }
    // Require the character model to have async-loaded (`+0x778`) so we capture a rendered portrait,
    // not a blank offscreen.
    if !(marked == 0 && valid(model) && valid(gx) && valid(gpu)) {
        return;
    }
    // Ready: keepalive the CSGxTexture and its GPU child so the teardown release cannot free them.
    let gx_rc =
        unsafe { &*((gx + GX_TEXTURE_REFCOUNT_OFFSET) as *const core::sync::atomic::AtomicI32) };
    gx_rc.fetch_add(0x10000, Ordering::SeqCst);
    let gpu_rc =
        unsafe { &*((gpu + GX_TEXTURE_REFCOUNT_OFFSET) as *const core::sync::atomic::AtomicI32) };
    gpu_rc.fetch_add(0x10000, Ordering::SeqCst);
    LOADING_BG_PORTRAIT_GX_KEPT.store(gx, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "loading-portrait: CAPTURED portrait CSGxTexture gx=0x{gx:x} gpu=0x{gpu:x} renderer=0x{renderer:x} -- kept alive for now-loading forge"
    ));
    // REAL PIXELS (gated): D3D12-read the rendered offscreen render target into CPU RGBA8 once, so
    // the now-loading forge can build its TPF from the actual character head instead of the checker
    // placeholder. Default OFF -> behavior is byte-identical to the proven checker path.
    if portrait_real_pixels_enabled() {
        // Scan from the OFFSCREEN render object (renderer+0xa8), not the gx sub-nest -- the real RT
        // hangs off the offscreen; the gx sub-nest holds only 1x1 vkd3d dummy textures.
        if let Some((w, h, px)) = unsafe { readback_offscreen_rgba8(offscreen) } {
            // `readback_offscreen_rgba8` already recorded LOADING_BG_PORTRAIT_FORMAT (the DXGI value).
            let nonblack = portrait_center_nonblack(w, h, &px);
            let is_checker = portrait_looks_like_checker(w, h, &px);
            LOADING_BG_PORTRAIT_NONBLACK.store(nonblack as usize, Ordering::SeqCst);
            LOADING_BG_PORTRAIT_IS_CHECKER.store(is_checker as usize, Ordering::SeqCst);
            LOADING_BG_PORTRAIT_DIMS.store(((w as usize) << 16) | (h as usize), Ordering::SeqCst);
            let bytes = px.len();
            dump_portrait_rgba(slot, w, h, &px);
            if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
                *g = Some((w, h, px));
            }
            append_autoload_debug(format_args!(
                "portrait-readback: dims={w}x{h} format={} nonblack={} is_checker={} (real-face proof = nonblack && !is_checker) bytes={bytes}",
                LOADING_BG_PORTRAIT_FORMAT.load(Ordering::SeqCst),
                nonblack as usize,
                is_checker as usize
            ));
        } else {
            append_autoload_debug(format_args!(
                "portrait-readback: readback_offscreen_rgba8 returned None (offscreen=0x{offscreen:x} gpu=0x{gpu:x})"
            ));
        }
    }
}

/// FORCE LIVE PROFILE PORTRAIT RENDER (diagnostic, `force_profile_render_enabled`). Runs each
/// menu-phase frame (no local player). One-shot: mark the target slot used
/// (`MarkProfileIndexAsUsed` -- the ONLY gate the refresh checks per STEP-0 RE: it sets
/// `ProfileSummary->saveSlotsStates[slot]=true` with no other side effect), then call the argless
/// profile-render refresh (`0x9aa680`), which equips ChrAsm + copies FaceData + kicks the async
/// character-model build that eventually sets `renderer+0x778`. The menu's OWN per-frame callbacks
/// then composite the live 3D head into the renderer's offscreen (no compositor call from us).
/// `maybe_capture_portrait_gxtexture` keeps the rendered gx once `+0x778` latches. Menu-phase only
/// (the user holds ProfileSelect; we never commit Continue) so there is no teardown/world-load crash
/// path -- this validates P1 (the model build) in isolation. Targets slot 0 (the staged single-profile
/// gold save's character). `slot` is the target save slot (0 for the staged single-profile gold
/// save; the autoload path passes its own target slot).
/// Read the OS mouse cursor -- which IS the menu cursor ER drives via `GetCursorPos` -- normalized to
/// the ER window client space: returns `(nx, ny)` where `(0,0)` is the window CENTER, `nx`/`ny` in
/// roughly `[-1, 1]` (left/up negative, right/down positive). `None` if the window or cursor can't be
/// resolved. Used to aim the portrait look-at at the cursor. (Cheap pure Win32; no game state touched.)
fn read_cursor_normalized() -> Option<(f32, f32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect};
    let hwnd = own_window()?;
    let mut pt = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut pt) }.is_err() {
        return None;
    }
    // Window rect is screen-space; ER's borderless window == its client area, so normalizing the
    // screen cursor against it gives the cursor position relative to the rendered image.
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return None;
    }
    let w = (rect.right - rect.left).max(1) as f32;
    let h = (rect.bottom - rect.top).max(1) as f32;
    let nx = ((pt.x - rect.left) as f32 / w) * 2.0 - 1.0;
    let ny = ((pt.y - rect.top) as f32 / h) * 2.0 - 1.0;
    // Clamp a little beyond the edges so an off-window cursor saturates rather than flailing.
    Some((nx.clamp(-1.5, 1.5), ny.clamp(-1.5, 1.5)))
}

/// CURSOR-SWEEP PROOF helper: warp the OS cursor to `(fx, fy)` as a fraction of the Elden Ring window's
/// client rect (`fx=0.10` left .. `0.90` right; `fy=0.5` mid-height), via `SetCursorPos`. This runs INSIDE
/// the game process, so it sets the same Wine cursor that [`read_cursor_normalized`]'s `GetCursorPos` reads
/// back -- a zero-foreign-input self-drive at the exact stage the look-at polls. Logs the first warp +
/// result. Best-effort: `None` if the window/SetCursorPos is unavailable (the proof then visibly fails:
/// the head won't move and the buckets won't fill).
fn drive_cursor_to_window_fraction(fx: f32, fy: f32) -> Option<()> {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, SetCursorPos};
    let hwnd = own_window()?;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return None;
    }
    let w = (rect.right - rect.left).max(1) as f32;
    let h = (rect.bottom - rect.top).max(1) as f32;
    let x = rect.left + (fx * w) as i32;
    let y = rect.top + (fy * h) as i32;
    let ok = unsafe { SetCursorPos(x, y) }.is_ok();
    if PROFILE_CURSOR_SWEEP_FIRST_WARP.swap(true, Ordering::SeqCst) != true {
        append_autoload_debug(format_args!(
            "cursor-sweep: first SetCursorPos({x},{y}) ok={ok} window=[{},{} {}x{}] frac=({fx},{fy})",
            rect.left, rect.top, w as i32, h as i32
        ));
    }
    ok.then_some(())
}

/// Hamilton product `a * b` of two `(x, y, z, w)` quaternions (w = scalar, matching `BoneData.q`).
fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// A small look rotation: `yaw` about the local Y axis then `pitch` about the local X axis, as a
/// `(x, y, z, w)` quaternion. (Which local axis actually reads as horizontal/vertical for the head
/// bone needs one runtime visual calibration; the `LOOKAT_*_SIGN` consts flip it without a code change.)
fn quat_from_yaw_pitch(yaw: f32, pitch: f32) -> [f32; 4] {
    let (sy, cy) = (yaw * 0.5).sin_cos();
    let (sp, cp) = (pitch * 0.5).sin_cos();
    let q_yaw = [0.0, sy, 0.0, cy];
    let q_pitch = [sp, 0.0, 0.0, cp];
    quat_mul(q_yaw, q_pitch)
}

/// Read a bounded null-terminated ASCII bone name from an `hkStringPtr` (low bit is an ownership flag,
/// masked by the caller). `None` on unmapped memory or non-UTF8 (bone names are ASCII; no lossy decode).
unsafe fn read_bone_name(ptr: usize) -> Option<String> {
    if ptr == 0 || ptr == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let mut bytes = Vec::with_capacity(32);
    for i in 0..64usize {
        let b = unsafe { safe_read_u8(ptr + i) }?;
        if b == 0 {
            break;
        }
        bytes.push(b);
    }
    if bytes.is_empty() {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Reach the live Havok `PoseHolder` from the renderer: `poseHolder = *(*(R+0x948)+0x20) + 0x48`,
/// guarded on the built model (`R+0x778`). `None` until the model + animation location are live.
unsafe fn profile_pose_holder(renderer: usize) -> Option<usize> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let model = unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }?;
    if !valid(model) {
        return None;
    }
    let x = unsafe { safe_read_usize(renderer + PROFILE_LOOKAT_ANIM_LOCATION_OFFSET) }?;
    if !valid(x) {
        return None;
    }
    let importer = unsafe { safe_read_usize(x + PROFILE_LOOKAT_IMPORTER_OFFSET) }?;
    if !valid(importer) {
        return None;
    }
    Some(importer + PROFILE_LOOKAT_POSEHOLDER_OFFSET)
}

/// Enumerate the skeleton's bones, dump names+indices ONCE per slot (diagnostic), and resolve the
/// Head/Neck/Spine2 indices by name. Returns `(head, neck, spine2)` indices (`-1` = not found).
unsafe fn dump_and_resolve_lookat_bones(bones: usize, count: usize, slot: i32) -> (i32, i32, i32) {
    let (mut head, mut neck, mut spine2) = (-1i32, -1i32, -1i32);
    let dump = (PROFILE_LOOKAT_BONES_DUMPED_MASK.load(Ordering::SeqCst) & (1usize << slot)) == 0;
    let mut dumped = String::new();
    for i in 0..count.min(LOOKAT_MAX_BONES) {
        let name_ptr =
            unsafe { safe_read_usize(bones + i * HKA_BONE_STRIDE + HKA_BONE_NAME_OFFSET) }
                .unwrap_or(0)
                & !1usize;
        let Some(name) = (unsafe { read_bone_name(name_ptr) }) else {
            continue;
        };
        if name.eq_ignore_ascii_case(LOOKAT_BONE_HEAD) {
            head = i as i32;
        } else if name.eq_ignore_ascii_case(LOOKAT_BONE_NECK) {
            neck = i as i32;
        } else if name.eq_ignore_ascii_case(LOOKAT_BONE_SPINE2) {
            spine2 = i as i32;
        }
        if dump {
            let _ = write!(dumped, "{i}:{name} ");
        }
    }
    if dump {
        PROFILE_LOOKAT_BONES_DUMPED_MASK.fetch_or(1usize << slot, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "lookat-bones: slot={slot} count={count} head={head} neck={neck} spine2={spine2} :: {dumped}"
        ));
    }
    (head, neck, spine2)
}

/// Pack a normalized cursor `(cx, cy)` into a usize as two i16 milli-units for telemetry.
fn pack_cursor(cx: f32, cy: f32) -> usize {
    let xi = (cx.clamp(-2.0, 2.0) * 1000.0) as i16 as u16 as usize;
    let yi = (cy.clamp(-2.0, 2.0) * 1000.0) as i16 as u16 as usize;
    (xi << 16) | yi
}

/// LOOK-AT LEVER: rotate the loaded character's Head/Neck/Spine2 bones toward the mouse cursor so the
/// portrait's gaze (eyes welded to the Head bone) follows it. Per tick: reach the pose holder, resolve
/// + latch the base pose ONCE, read the cursor, write each bone's LOCAL quaternion = `base ⊗ delta`,
/// then mark every bone's model-space dirty + `isUpdated=false` so the render's `updateBoneModelSpace`
/// rebuilds the chain (and the head's children) before the offscreen draw. `renderer` must be a
/// validated live CSMenuProfModelRend. Returns true once a rotation was written.
unsafe fn apply_profile_lookat(renderer: usize, slot: i32) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let idx = slot as usize;
    if idx >= TITLE_PROFILE_SLOT_COUNT {
        return false;
    }
    let holder = match unsafe { profile_pose_holder(renderer) } {
        Some(h) => h,
        None => {
            // The engine refreshes a menu model's anim location-holder only intermittently (~6 Hz), so
            // `profile_pose_holder` returns None on ~89% of frames even though the model + its PoseHolder
            // persist. The caller only invokes us for a still-valid (vtable-checked) renderer, so a
            // transient None here is just the throttle -- KEEP the last resolved holder registered so the
            // draw-phase task can drive + recompute + redraw it EVERY frame (60 Hz tracking), decoupled
            // from the engine's throttled pose update. A genuinely stale holder (model rebuilt/torn down)
            // is dropped explicitly: the force-rebuild path clears PROFILE_LOOKAT_HOLDERS, and the
            // teardown spare hook owns post-Continue lifetime. Do NOT unregister on transient None.
            return false;
        }
    };
    let skel = unsafe { safe_read_usize(holder + POSEHOLDER_SKELETON_OFFSET) }.unwrap_or(0);
    if !valid(skel) {
        return false;
    }
    let bones = unsafe { safe_read_usize(skel + HKA_SKELETON_BONES_DATA_OFFSET) }.unwrap_or(0);
    let count = unsafe { safe_read_i32(skel + HKA_SKELETON_BONES_SIZE_OFFSET) }.unwrap_or(0);
    if !valid(bones) || count <= 0 || count as usize > LOOKAT_MAX_BONES {
        return false;
    }
    let count = count as usize;
    PROFILE_LOOKAT_BONE_COUNT.store(count, Ordering::SeqCst);
    // Resolve the Head/Neck/Spine2 indices once per slot (+ dump bone names once). The hook reads the
    // shared PROFILE_LOOKAT_*_IDX globals; per-slot caching here just avoids re-dumping the names.
    {
        let mut guard = match PROFILE_LOOKAT_SLOTS.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if guard[idx].is_none() {
            let (head, neck, spine2) = unsafe { dump_and_resolve_lookat_bones(bones, count, slot) };
            if head < 0 {
                return false; // head bone not found yet; retry next tick
            }
            guard[idx] = Some(LookatSlot {
                head,
                neck,
                spine2,
                head_base: [0.0; 4],
                neck_base: [0.0; 4],
                spine2_base: [0.0; 4],
                base_latched: false,
            });
            PROFILE_LOOKAT_HEAD_IDX.store(head as usize, Ordering::SeqCst);
            PROFILE_LOOKAT_NECK_IDX.store(
                if neck >= 0 { neck as usize } else { usize::MAX },
                Ordering::SeqCst,
            );
            PROFILE_LOOKAT_SPINE2_IDX.store(
                if spine2 >= 0 {
                    spine2 as usize
                } else {
                    usize::MAX
                },
                Ordering::SeqCst,
            );
        }
    }
    // FrameBegin role: resolve + cache the Head/Neck/Spine2 indices (above) and register the holder. The
    // drive ANGLE is published by the draw-phase task (cursor or selftest sinusoid) -- do NOT publish it
    // here too, or this FrameBegin cursor value would race/override the draw task's value within a frame
    // and the per-frame push hook would read the wrong angle. The pose WRITE happens in the per-frame push
    // hook (which propagates to the GPU-skinned submodels); install it here so it is live once a renderer is.
    PROFILE_LOOKAT_HOLDERS[idx].store(holder, Ordering::SeqCst);
    install_lookat_hook();
    install_per_frame_push_hook();
    PROFILE_LOOKAT_APPLY_CALLS.fetch_add(1, Ordering::SeqCst);
    true
}

/// Read a `BoneData` quaternion (4 f32 at `addr`) with fault-guarded reads; `None` on unmapped memory.
unsafe fn read_quat(addr: usize) -> Option<[f32; 4]> {
    Some([
        f32::from_bits(unsafe { safe_read_i32(addr) }? as u32),
        f32::from_bits(unsafe { safe_read_i32(addr + 4) }? as u32),
        f32::from_bits(unsafe { safe_read_i32(addr + 8) }? as u32),
        f32::from_bits(unsafe { safe_read_i32(addr + 12) }? as u32),
    ])
}

/// Compose the cursor look rotation onto a registered profile holder's Head/Neck/Spine2 LOCAL
/// quaternions (post-multiplied onto the current anim pose) and mark all bones model-space dirty, so the
/// `updateBoneModelSpace` original we are about to call rebuilds the final rendered pose with the
/// look-at baked in. Runs on the render thread inside the hook; every read is fault-guarded + bounded.
unsafe fn lookat_write_local(holder: usize) {
    // Realtime mode owns the write+recompute+draw from the draw-phase task (composing from a latched
    // base). The detour must then be a pure passthrough -- a second post-multiply here would double-apply
    // the rotation onto the same frame's local pose. See `profile_lookat_realtime_draw_tick`.
    if PROFILE_LOOKAT_REALTIME.load(Ordering::SeqCst) {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let local = unsafe { safe_read_usize(holder + POSEHOLDER_LOCAL_BONE_DATA_OFFSET) }.unwrap_or(0);
    let dirty = unsafe { safe_read_usize(holder + POSEHOLDER_DIRTY_FLAGS_OFFSET) }.unwrap_or(0);
    let skel = unsafe { safe_read_usize(holder + POSEHOLDER_SKELETON_OFFSET) }.unwrap_or(0);
    if !valid(local) || !valid(dirty) || !valid(skel) {
        return;
    }
    let count = unsafe { safe_read_i32(skel + HKA_SKELETON_BONES_SIZE_OFFSET) }.unwrap_or(0);
    if count <= 0 || count as usize > LOOKAT_MAX_BONES {
        return;
    }
    let count = count as usize;
    let yaw = f32::from_bits(PROFILE_LOOKAT_YAW_BITS.load(Ordering::SeqCst) as u32);
    let pitch = f32::from_bits(PROFILE_LOOKAT_PITCH_BITS.load(Ordering::SeqCst) as u32);
    let drives = [
        (
            PROFILE_LOOKAT_HEAD_IDX.load(Ordering::SeqCst),
            LOOKAT_HEAD_YAW_GAIN,
            LOOKAT_HEAD_PITCH_GAIN,
        ),
        (
            PROFILE_LOOKAT_NECK_IDX.load(Ordering::SeqCst),
            LOOKAT_NECK_YAW_GAIN,
            LOOKAT_NECK_PITCH_GAIN,
        ),
        (
            PROFILE_LOOKAT_SPINE2_IDX.load(Ordering::SeqCst),
            LOOKAT_SPINE2_YAW_GAIN,
            LOOKAT_SPINE2_PITCH_GAIN,
        ),
    ];
    let mut any = false;
    for (bidx, yg, pg) in drives {
        if bidx == usize::MAX || bidx >= count {
            continue;
        }
        let q0 = local + bidx * BONE_DATA_STRIDE + BONE_DATA_Q_OFFSET;
        let Some(cur) = (unsafe { read_quat(q0) }) else {
            continue;
        };
        let q = quat_mul(cur, quat_from_yaw_pitch(yaw * yg, pitch * pg));
        if !q.iter().all(|f| f.is_finite()) {
            continue;
        }
        unsafe {
            core::ptr::write_volatile(q0 as *mut f32, q[0]);
            core::ptr::write_volatile((q0 + 4) as *mut f32, q[1]);
            core::ptr::write_volatile((q0 + 8) as *mut f32, q[2]);
            core::ptr::write_volatile((q0 + 12) as *mut f32, q[3]);
        }
        any = true;
    }
    if any {
        for i in 0..count {
            let f = dirty + i * 4;
            let cur = unsafe { safe_read_i32(f) }.unwrap_or(0) as u32;
            unsafe {
                core::ptr::write_volatile(f as *mut u32, cur | POSE_DIRTY_MODEL_SPACE_BIT);
            }
        }
        unsafe {
            core::ptr::write_volatile((holder + POSEHOLDER_IS_UPDATED_OFFSET) as *mut u8, 0);
        }
        PROFILE_LOOKAT_HOOK_HITS.fetch_add(1, Ordering::SeqCst);
    }
}

/// Hook on `updateBoneModelSpace`: for a registered profile holder, write the look-at into the local
/// pose BEFORE the original recomputes model-space, so the rotation cascades into the rendered pose.
pub(crate) unsafe extern "system" fn update_bone_model_space_hook(holder: usize) {
    if holder != 0 {
        let ours = PROFILE_LOOKAT_HOLDERS
            .iter()
            .any(|h| h.load(Ordering::SeqCst) == holder);
        if ours {
            unsafe { lookat_write_local(holder) };
        }
    }
    let orig = PROFILE_LOOKAT_HOOK_ORIG.load(Ordering::SeqCst);
    if orig != TITLE_OWNER_SCAN_START_ADDRESS && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize) = unsafe { core::mem::transmute(orig) };
        unsafe { f(holder) };
    }
}

fn install_lookat_hook() {
    if PROFILE_LOOKAT_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "lookat-hook: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(target) = game_rva(UPDATE_BONE_MODEL_SPACE_RVA as u32) else {
        return;
    };
    match unsafe {
        MhHook::new(
            target as *mut c_void,
            update_bone_model_space_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            PROFILE_LOOKAT_HOOK_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_err() {
                append_autoload_debug(format_args!(
                    "lookat-hook: queue_enable failed for 0x{target:x}"
                ));
                return;
            }
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!("lookat-hook: MhHook::new failed: {status:?}"));
            return;
        }
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "lookat-hook: installed on updateBoneModelSpace 0x{target:x}"
        )),
        status => append_autoload_debug(format_args!(
            "lookat-hook: MH_ApplyQueued failed: {status:?}"
        )),
    }
}

/// Resolve the clean `updateBoneModelSpace` entry to recompute model-space from local bones WITHOUT
/// re-entering the look-at detour: prefer the hook trampoline (the saved original), else the raw RVA.
/// Pure SIMD math, touches no GX context, so it is safe to call from any phase.
unsafe fn lookat_recompute_fn() -> Option<unsafe extern "system" fn(usize)> {
    let orig = PROFILE_LOOKAT_HOOK_ORIG.load(Ordering::SeqCst);
    if orig != TITLE_OWNER_SCAN_START_ADDRESS && orig != HOOK_ORIGINAL_UNSET {
        return Some(unsafe { core::mem::transmute(orig) });
    }
    match game_rva(UPDATE_BONE_MODEL_SPACE_RVA as u32) {
        Ok(addr) => Some(unsafe { core::mem::transmute(addr) }),
        Err(_) => None,
    }
}

/// Per-frame look-at for ONE registered profile holder, driven from the draw-phase task: latch the clean
/// idle local quats once (drift-free base), write `base ⊗ delta(yaw,pitch)` into Head/Neck/Spine2 local
/// quats, mark all bones model-space-dirty + `isUpdated=false`, then recompute model-space so the draw
/// that follows skins from the rotated pose. Returns true if any bone was driven. Every read is bounded.
unsafe fn lookat_apply_realtime(holder: usize, slot_idx: usize, yaw: f32, pitch: f32) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let local = unsafe { safe_read_usize(holder + POSEHOLDER_LOCAL_BONE_DATA_OFFSET) }.unwrap_or(0);
    let dirty = unsafe { safe_read_usize(holder + POSEHOLDER_DIRTY_FLAGS_OFFSET) }.unwrap_or(0);
    let skel = unsafe { safe_read_usize(holder + POSEHOLDER_SKELETON_OFFSET) }.unwrap_or(0);
    if !valid(local) || !valid(dirty) || !valid(skel) {
        return false;
    }
    let count = unsafe { safe_read_i32(skel + HKA_SKELETON_BONES_SIZE_OFFSET) }.unwrap_or(0);
    if count <= 0 || count as usize > LOOKAT_MAX_BONES {
        return false;
    }
    let count = count as usize;
    // Pull this slot's resolved indices + latched base (copy out, release the lock before any game read).
    let (head, neck, spine2, mut base, latched) = {
        let guard = match PROFILE_LOOKAT_SLOTS.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        match guard[slot_idx] {
            Some(s) => (
                s.head,
                s.neck,
                s.spine2,
                [s.head_base, s.neck_base, s.spine2_base],
                s.base_latched,
            ),
            None => return false,
        }
    };
    // (bone index, yaw gain, pitch gain, base-slot)
    let drives = [
        (head, LOOKAT_HEAD_YAW_GAIN, LOOKAT_HEAD_PITCH_GAIN, 0usize),
        (neck, LOOKAT_NECK_YAW_GAIN, LOOKAT_NECK_PITCH_GAIN, 1usize),
        (
            spine2,
            LOOKAT_SPINE2_YAW_GAIN,
            LOOKAT_SPINE2_PITCH_GAIN,
            2usize,
        ),
    ];
    let q_addr = |bidx: i32| -> Option<usize> {
        if bidx < 0 || bidx as usize >= count {
            None
        } else {
            Some(local + bidx as usize * BONE_DATA_STRIDE + BONE_DATA_Q_OFFSET)
        }
    };
    // Latch the clean idle base ONCE (the slot is reset to None on each rebuild, so `local` here is the
    // freshly-rebuilt idle pose -- captured before this frame's look-at write contaminates it).
    if !latched {
        for (bidx, _, _, bslot) in drives {
            if let Some(addr) = q_addr(bidx) {
                if let Some(q) = unsafe { read_quat(addr) } {
                    base[bslot] = q;
                }
            }
        }
        let mut guard = match PROFILE_LOOKAT_SLOTS.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(s) = guard[slot_idx].as_mut() {
            s.head_base = base[0];
            s.neck_base = base[1];
            s.spine2_base = base[2];
            s.base_latched = true;
        }
    }
    let mut any = false;
    for (bidx, yg, pg, bslot) in drives {
        let Some(addr) = q_addr(bidx) else { continue };
        let q = quat_mul(base[bslot], quat_from_yaw_pitch(yaw * yg, pitch * pg));
        if !q.iter().all(|f| f.is_finite()) {
            continue;
        }
        unsafe {
            core::ptr::write_volatile(addr as *mut f32, q[0]);
            core::ptr::write_volatile((addr + 4) as *mut f32, q[1]);
            core::ptr::write_volatile((addr + 8) as *mut f32, q[2]);
            core::ptr::write_volatile((addr + 12) as *mut f32, q[3]);
        }
        any = true;
    }
    if !any {
        return false;
    }
    for i in 0..count {
        let f = dirty + i * 4;
        let cur = unsafe { safe_read_i32(f) }.unwrap_or(0) as u32;
        unsafe {
            core::ptr::write_volatile(f as *mut u32, cur | POSE_DIRTY_MODEL_SPACE_BIT);
        }
    }
    unsafe {
        core::ptr::write_volatile((holder + POSEHOLDER_IS_UPDATED_OFFSET) as *mut u8, 0);
    }
    // Recompute model-space from the local pose so the upcoming draw skins from the look-at rotation.
    if let Some(recompute) = unsafe { lookat_recompute_fn() } {
        unsafe { recompute(holder) };
    }
    PROFILE_LOOKAT_HOOK_HITS.fetch_add(1, Ordering::SeqCst);
    true
}

/// REALTIME LOOK-AT DRAW TICK -- registered as a recurring task in a DRAW phase
/// (`CSTaskGroupIndex::GameSceneDraw`), so it runs on the render thread INSIDE an actively-recording GX
/// frame (unlike the FrameBegin game task, where the GX subcontext pool is still empty -> a black no-op).
/// Each frame: read the live cursor, drive every registered profile holder's Head/Neck/Spine2 toward it
/// (drift-free `base ⊗ delta`) + recompute model-space, then call the profile draw step to rasterize ALL
/// portraits' offscreen RTs with the fresh pose. The engine only redraws thumbnails on profile
/// data-change, so without this they track the cursor only at the ~4s model-rebuild cadence; here they
/// track every frame. The draw step fail-closes (the GX pool pop returns 0 -> no-op) if a phase ever
/// lacks a live frame, so it can never crash from being driven off a recording frame.
pub(crate) unsafe fn profile_lookat_realtime_draw_tick(base: usize) {
    if !portrait_lookat_enabled() {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == 0 || base == null {
        return;
    }
    // The 0x1653350 detour stays a passthrough (the per-frame PUSH hook owns the pose write now).
    PROFILE_LOOKAT_REALTIME.store(true, Ordering::SeqCst);
    // Ensure the per-frame push hook is installed -- it writes our pose into the importer + lets the
    // engine propagate it to the GPU-skinned submodels each frame (the actual head movement).
    install_per_frame_push_hook();
    let frame = PROFILE_LOOKAT_DRAW_FRAME.fetch_add(1, Ordering::SeqCst);
    // PUBLISH the drive angle for the per-frame push hook to consume: a deterministic SINUSOID in selftest
    // (zero-input, reproducible -> the pixel oracle proves the head moves with the driven angle), else the
    // live cursor (the product input). The pose WRITE happens in the push hook; here we only publish + draw.
    let (yaw, pitch) = if PROFILE_CURSOR_SWEEP_ON.load(Ordering::SeqCst) {
        // CURSOR-TRACKING PROOF: deterministically warp the OS cursor to a held L/C/R position over the ER
        // window, THEN read it back through the SAME GetCursorPos path the product uses, and drive the head
        // from that read cursor (no sinusoid). Zero foreign input: the DLL self-drives the cursor at the
        // exact stage the look-at polls it. The yaw lands in a left/center/right bucket -> the bucket dump
        // below captures the head at each real cursor position.
        let hold = (frame / CURSOR_SWEEP_HOLD_FRAMES) % CURSOR_SWEEP_TARGETS_X.len();
        drive_cursor_to_window_fraction(CURSOR_SWEEP_TARGETS_X[hold], 0.5);
        let (cx, cy) = read_cursor_normalized().unwrap_or((0.0, 0.0));
        PROFILE_LOOKAT_LAST_CURSOR.store(pack_cursor(cx, cy), Ordering::SeqCst);
        (cx * LOOKAT_YAW_SIGN, cy * LOOKAT_PITCH_SIGN)
    } else if PROFILE_LOOKAT_SELFTEST_ON.load(Ordering::SeqCst) {
        let t = frame as f32 * LOOKAT_SELFTEST_W;
        (
            t.sin() * LOOKAT_SELFTEST_YAW_AMP * LOOKAT_YAW_SIGN,
            (t * 0.7).sin() * LOOKAT_SELFTEST_PITCH_AMP * LOOKAT_PITCH_SIGN,
        )
    } else {
        let (cx, cy) = read_cursor_normalized().unwrap_or((0.0, 0.0));
        PROFILE_LOOKAT_LAST_CURSOR.store(pack_cursor(cx, cy), Ordering::SeqCst);
        (cx * LOOKAT_YAW_SIGN, cy * LOOKAT_PITCH_SIGN)
    };
    PROFILE_LOOKAT_YAW_BITS.store(yaw.to_bits() as usize, Ordering::SeqCst);
    PROFILE_LOOKAT_PITCH_BITS.store(pitch.to_bits() as usize, Ordering::SeqCst);
    // Rasterize all profile offscreen RTs on the render thread inside the live GX frame, so the pose the
    // push hook propagated this frame is re-rendered (the engine does not redraw thumbnails per frame).
    // The draw step skips null slots and fail-closes if the GX pool is empty, so it is safe every frame.
    // draw_step (FUN_1409aa3e0 -> per-slot FUN_140bb73a0) is a CLEAR-render-target, NOT a rasterize
    // (FUN_141e8af80 = ClearRTV; RE-confirmed). Post-Continue the offscreen is a SINGLE texture (RT==SRV,
    // proven: find_d3d12_resource(off)==find_d3d12_resource(srv_gx)), so clearing it every frame WIPES the
    // rendered head before GFx samples it -> the now-loading background reads mostly-black. Once our own
    // table is built, SKIP the clear so the last-rendered portrait persists in the sampleable texture.
    if PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst) == 0 {
        let draw_step: unsafe extern "system" fn() =
            unsafe { core::mem::transmute(base + PROFILE_DRAW_STEP_RVA) };
        unsafe { draw_step() };
        PROFILE_LOOKAT_RENDER_DRIVES.fetch_add(1, Ordering::SeqCst);
    }
    // FORCE THE RT->SRV RESOLVE: the engine's per-frame resolve almost never fires post-Continue (the
    // offscreen RENDER TARGET holds the rendered head but the sampleable SRV the forge binds stays black),
    // so D3D12-copy the target slot's RT into its SRV every render-thread frame. src = renderer+0xa8
    // (offscreen; find_d3d12_resource reaches the content RT), dst = offscreen+0x10's CSGxTexture (the SRV
    // GFx samples). Render-thread context (same as the readback), bounded + fail-closed.
    {
        let slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
        let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, slot)) }.unwrap_or(0);
        if r != 0
            && r != null
            && unsafe { safe_read_usize(r) }.unwrap_or(0)
                == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
        {
            let off = unsafe {
                safe_read_usize(r + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
            }
            .unwrap_or(0);
            if off != 0 && off != null {
                let trc = unsafe {
                    safe_read_usize(off + TITLE_CUSTOM_COVER_PROFILE_OFFSCREEN_TEX_RESCAP_OFFSET)
                }
                .unwrap_or(0);
                let srv_gx = if trc != 0 && trc != null {
                    unsafe {
                        safe_read_usize(trc + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET)
                    }
                    .unwrap_or(0)
                } else {
                    0
                };
                // src_start = off (the offscreen nest, which contains BOTH the content RT and the SRV);
                // the copy resolves the SRV from srv_gx and then the largest OTHER texture in off as the
                // content source, so the RT/SRV ambiguity is handled inside the copy.
                if srv_gx != 0 && srv_gx != null {
                    if unsafe { copy_offscreen_rt_to_srv(off, srv_gx) } {
                        PROFILE_RT_SRV_COPIES.fetch_add(1, Ordering::SeqCst);
                    }
                    // One-shot dump of the EXCLUDING-SRV content texture (slot 102) so we can SEE whether
                    // the largest non-SRV texture in the offscreen nest is the portrait (and at what res).
                    if PROFILE_CONTENT_EXCL_DUMPED.swap(1, Ordering::SeqCst) == 0 {
                        if let Some((cw, ch, cpx)) =
                            unsafe { readback_excluding_rgba8(off, srv_gx) }
                        {
                            dump_portrait_rgba(102, cw, ch, &cpx);
                        } else {
                            PROFILE_CONTENT_EXCL_DUMPED.store(0, Ordering::SeqCst);
                        }
                    }
                    // LIVE TRACKING -- EVERY FRAME, now SCAN-FREE. readback_cached_content_rgba8 resolves the
                    // content RT via the DETERMINISTIC GX wrapper chain ONCE (no memory scan/QI -> nothing to
                    // race the teardown free), caches it AddRef'd, then re-copies it each frame. So the head
                    // tracks the look-at smoothly without the prior scan-vs-teardown AV. Bumps the RGBA
                    // version each frame -> maybe_reforge_loading_portrait re-uploads -> the displayed
                    // loading-screen head updates per frame.
                    if portrait_render_drive_enabled() {
                        if let Some((cw, ch, cpx)) =
                            unsafe { readback_cached_content_rgba8(off, srv_gx) }
                        {
                            if !portrait_looks_like_checker(cw, ch, &cpx) {
                                let nb = portrait_center_nonblack(cw, ch, &cpx);
                                LOADING_BG_PORTRAIT_NONBLACK.store(nb as usize, Ordering::SeqCst);
                                LOADING_BG_PORTRAIT_IS_CHECKER.store(0, Ordering::SeqCst);
                                LOADING_BG_PORTRAIT_DIMS.store(
                                    ((cw as usize) << 16) | (ch as usize),
                                    Ordering::SeqCst,
                                );
                                // MOUSE-TRACK PROOF (selftest): one-shot dump the LIVE head at three
                                // held yaw buckets so the look-left/center/look-right poses are
                                // visually inspectable. The selftest sinusoid sweeps `yaw` across
                                // [-1,1] each period, so all three buckets fill within one loading
                                // window. In product the same PROFILE_LOOKAT_YAW_BITS atomic is set
                                // from the normalized cursor, so distinct poses here = the head pose
                                // tracks the drive signal. Dump from `&cpx` BEFORE it moves into the
                                // overlay lock below.
                                if PROFILE_LOOKAT_SELFTEST_ON.load(Ordering::SeqCst)
                                    || PROFILE_CURSOR_SWEEP_ON.load(Ordering::SeqCst)
                                {
                                    let bucket = if yaw <= -0.5 {
                                        Some(0usize)
                                    } else if yaw >= 0.5 {
                                        Some(2usize)
                                    } else if yaw.abs() <= 0.15 {
                                        Some(1usize)
                                    } else {
                                        None
                                    };
                                    if let Some(b) = bucket {
                                        let prev = PROFILE_LOOKAT_TRACK_BUCKETS
                                            .fetch_or(1 << b, Ordering::SeqCst);
                                        if prev & (1 << b) == 0 {
                                            dump_portrait_rgba(200 + b as i32, cw, ch, &cpx);
                                        }
                                    }
                                }
                                if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
                                    *g = Some((cw, ch, cpx));
                                }
                                LOADING_BG_PORTRAIT_RGBA_VERSION
                                    .fetch_add(1, Ordering::SeqCst);
                                // Gate the present-overlay on (now there is a real live head to show).
                                PROFILE_BAKE_RGBA_CAPTURED.store(1, Ordering::SeqCst);
                                if PROFILE_LIVE_FEED_LOGGED.swap(1, Ordering::SeqCst) == 0 {
                                    append_autoload_debug(format_args!(
                                        "live-feed: published built RT content {cw}x{ch} (real head, !checker) -> overlay (version bump); present-overlay will composite the LIVE head"
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // POST-CONTINUE: the spared renderer is NOT in the menu table (the draw step above skips it), so
    // rasterize it directly via the offscreen-draw thunk (fn(renderer) -> renders *(renderer+0xa8)). This
    // is the persistent-model path; whether it produces pixels post-Continue is the keepalive question the
    // oracle answers. Validate the vtable before calling so a stale spared pointer can't fault the thunk.
    let spared = LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst);
    if spared != 0
        && spared != null
        && unsafe { safe_read_usize(spared) }.unwrap_or(0)
            == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
    {
        // NOTE: re-attaching the captured model into the spared renderer's +0x778 was tried and CRASHES
        // (run 2026-06-30: AV in the ResMan/offscreen-draw path +28ms after the write) -- the teardown frees
        // the model's deeper render deps even though its vtable still reads valid. Dead end. The live render
        // comes from BUILDING OUR OWN renderers post-Continue (force_profile_render_tick driven from a
        // FrameBegin task), which own their model+deps with our lifetime. See bd
        // portrait-live-render-reattach-crashes-build-own-2026-06-30.
        let thunk: unsafe extern "system" fn(usize) =
            unsafe { core::mem::transmute(base + PROFILE_OFFSCREEN_DRIVE_RVA) };
        unsafe { thunk(spared) };
        PROFILE_PERFRAME_SPARED_DRAWS.fetch_add(1, Ordering::SeqCst);
    }
    // Q4 KEEPALIVE ORACLE: read the GX render-pass queue (non-destructively) each draw frame to learn
    // whether a GX pass is queued -- the precondition for any offscreen render producing pixels. Sanity:
    // it should be non-empty during the menu (things render); the decisive question is whether it stays
    // non-empty during the now-loading screen (post-Continue).
    unsafe { profile_gx_queue_sample(base) };
    // IN-PROCESS PIXEL ORACLE (selftest only): after the draw, sample the live slot's offscreen RT and
    // record nonblack% + same-slot hash-change% -- the numbers that replace the human eyeball. Called
    // every frame but self-gates on a live model (no readback cost when none is present), so it catches
    // the sparse frames a menu model actually exists. The LOOKAT_RT_SAMPLE_INTERVAL const is retained for
    // reference but no longer throttles (model presence is the natural throttle).
    let _ = LOOKAT_RT_SAMPLE_INTERVAL;
    if PROFILE_LOOKAT_SELFTEST_ON.load(Ordering::SeqCst) {
        unsafe { profile_lookat_rt_sample(base) };
    }
}

/// Q4 keepalive oracle: read the GX render-pass queue head/tail (non-destructively -- NO pop) to detect
/// whether a GX pass is queued this frame (the precondition the offscreen draw checks via FUN_1419e5850).
/// g_GxDrawContext may be a pointer-global (heap ctx) or the struct itself; resolve defensively and fall
/// back to the global address. All reads fault-guarded.
unsafe fn profile_gx_queue_sample(base: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let global = base + GX_DRAW_CONTEXT_RVA;
    let readable = |c: usize| {
        valid(c)
            && unsafe { safe_read_usize(c + GX_DRAW_CONTEXT_QUEUE_HEAD_OFFSET) }.is_some()
            && unsafe { safe_read_usize(c + GX_DRAW_CONTEXT_QUEUE_TAIL_OFFSET) }.is_some()
    };
    // Primary: g_GxDrawContext holds the context pointer (the game passes it directly as the ctx base).
    let mut ctx = unsafe { safe_read_usize(global) }.unwrap_or(0);
    if !readable(ctx) {
        ctx = global; // fallback: the global IS the context struct
    }
    if !readable(ctx) {
        return;
    }
    let head = unsafe { safe_read_usize(ctx + GX_DRAW_CONTEXT_QUEUE_HEAD_OFFSET) }.unwrap_or(0);
    let tail = unsafe { safe_read_usize(ctx + GX_DRAW_CONTEXT_QUEUE_TAIL_OFFSET) }.unwrap_or(0);
    PROFILE_GX_QUEUE_SAMPLES.fetch_add(1, Ordering::SeqCst);
    if head != tail {
        PROFILE_GX_QUEUE_NONEMPTY.fetch_add(1, Ordering::SeqCst);
    }
}

/// Pixel oracle sample: scan for the FIRST slot whose model is currently live (model_ins present), read
/// back its offscreen RT (AFTER the draw step) and record nonblack + whether the content hash changed vs
/// the previous sample OF THE SAME SLOT. Sampling the live slot (not a fixed one) is required because the
/// engine keeps barely one menu model built at a time (cycling); "changed" is gated to same-slot so a
/// slot switch (different character) is not mistaken for head motion. Only does the (costly) readback when
/// a live model exists, so it is free when none is present. Read-only + fault-guarded.
unsafe fn profile_lookat_rt_sample(base: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let mut chosen = usize::MAX;
    let mut off = 0usize;
    // Prefer the POST-Continue spared renderer (the persistent model) when it is set + live; it is not in
    // the menu table, so the table scan below would miss it. Use a dedicated sample index (10) so the
    // same-slot "changed" gate treats it as its own stream.
    let spared = LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst);
    if valid(spared)
        && unsafe { safe_read_usize(spared) }.unwrap_or(0)
            == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
    {
        let model =
            unsafe { safe_read_usize(spared + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
        if valid(model) {
            PROFILE_SPARED_MODEL_OK.fetch_add(1, Ordering::SeqCst);
            let o = unsafe {
                safe_read_usize(spared + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
            }
            .unwrap_or(0);
            if valid(o) {
                chosen = TITLE_PROFILE_SLOT_COUNT; // dedicated "spared" stream index
                off = o;
            }
        }
    }
    for s in (chosen == usize::MAX)
        .then_some(0..TITLE_PROFILE_SLOT_COUNT)
        .into_iter()
        .flatten()
    {
        let r =
            unsafe { safe_read_usize(portrait_renderer_table_entry(base, s as i32)) }.unwrap_or(0);
        if !valid(r)
            || unsafe { safe_read_usize(r) }.unwrap_or(0)
                != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
        {
            continue;
        }
        // model present?
        if !valid(unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0)) {
            continue;
        }
        let o = unsafe {
            safe_read_usize(r + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
        }
        .unwrap_or(0);
        if valid(o) {
            chosen = s;
            off = o;
            break;
        }
    }
    if chosen == usize::MAX {
        return; // no live model this frame -> no readback cost
    }
    let Some((w, h, px)) = (unsafe { readback_offscreen_rgba8(off) }) else {
        return;
    };
    PROFILE_LOOKAT_RT_SAMPLES.fetch_add(1, Ordering::SeqCst);
    if portrait_center_nonblack(w, h, &px) {
        PROFILE_LOOKAT_RT_NONBLACK.fetch_add(1, Ordering::SeqCst);
    }
    // ALPHA vs RGB: max RGB and max alpha over the same center region. If rgb_max>0 but alpha_max==0 the
    // RT has a portrait that GFx will composite as fully transparent (the "renders black despite content"
    // signature). Decides the color-space/alpha question without a screenshot.
    {
        let (wq, hq) = (w as usize, h as usize);
        if wq > 0 && hq > 0 && px.len() >= wq * hq * 4 {
            let (cx, cy) = (wq / 2, hq / 2);
            let (x0, x1) = (cx.saturating_sub(32), (cx + 32).min(wq));
            let (y0, y1) = (cy.saturating_sub(32), (cy + 32).min(hq));
            let (mut rgb_max, mut a_max) = (0u8, 0u8);
            for y in y0..y1 {
                for x in x0..x1 {
                    let idx = (y * wq + x) * 4;
                    rgb_max = rgb_max.max(px[idx]).max(px[idx + 1]).max(px[idx + 2]);
                    a_max = a_max.max(px[idx + 3]);
                }
            }
            PROFILE_LOOKAT_RT_RGB_MAX.store(rgb_max as usize, Ordering::SeqCst);
            PROFILE_LOOKAT_RT_ALPHA_MAX.store(a_max as usize, Ordering::SeqCst);
            // One-shot dump of the readback "content" RT (slot 100) on a frame where it actually has
            // content, so we can visually confirm whether it is the portrait or a scratch/world RT.
            if rgb_max > 24 && PROFILE_RT_CONTENT_DUMPED.swap(1, Ordering::SeqCst) == 0 {
                dump_portrait_rgba(100, w, h, &px);
            }
        }
    }
    // SAMPLEABLE-TEXTURE READBACK: read the texture actually BOUND into the now-loading container (what
    // GFx samples) and compare to the render target above. Same render-thread context (safe). If the RT
    // has content but this reads black, the bound CSGxTexture is a separate/unresolved resource.
    {
        let cap = LOADING_BG_TEXTURE_REDIRECT_LAST_PORTRAIT.load(Ordering::SeqCst);
        let mut bgx = 0usize;
        if valid(cap) {
            let container =
                unsafe { safe_read_usize(cap + TPF_FILE_CAP_TEX_RESCAP_OFFSET) }.unwrap_or(0);
            if valid(container) {
                let array =
                    unsafe { safe_read_usize(container + TPF_RESCAP_CONTAINER_ARRAY_OFFSET) }
                        .unwrap_or(0);
                if valid(array) {
                    let trc0 = unsafe { safe_read_usize(array) }.unwrap_or(0);
                    if valid(trc0) {
                        bgx = unsafe {
                            safe_read_usize(trc0 + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET)
                        }
                        .unwrap_or(0);
                    }
                }
            }
        }
        if valid(bgx) {
            if let Some((bw, bh, bpx)) = unsafe { readback_offscreen_rgba8(bgx) } {
                let (wq, hq) = (bw as usize, bh as usize);
                if wq > 0 && hq > 0 && bpx.len() >= wq * hq * 4 {
                    let (cx, cy) = (wq / 2, hq / 2);
                    let (x0, x1) = (cx.saturating_sub(32), (cx + 32).min(wq));
                    let (y0, y1) = (cy.saturating_sub(32), (cy + 32).min(hq));
                    let (mut rgb_max, mut a_max) = (0u8, 0u8);
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let idx = (y * wq + x) * 4;
                            rgb_max = rgb_max.max(bpx[idx]).max(bpx[idx + 1]).max(bpx[idx + 2]);
                            a_max = a_max.max(bpx[idx + 3]);
                        }
                    }
                    PROFILE_BOUND_GX_RGB_MAX.store(rgb_max as usize, Ordering::SeqCst);
                    PROFILE_BOUND_GX_ALPHA_MAX.store(a_max as usize, Ordering::SeqCst);
                    // One-shot dump of the bound SRV (slot 101) once we've also captured the content RT,
                    // so the two can be compared side by side (is the SRV black? is the RT the portrait?).
                    if PROFILE_RT_CONTENT_DUMPED.load(Ordering::SeqCst) != 0
                        && PROFILE_SRV_DUMPED.swap(1, Ordering::SeqCst) == 0
                    {
                        dump_portrait_rgba(101, bw, bh, &bpx);
                    }
                }
            }
        }
    }
    // Cheap strided FNV-1a hash of the RT to detect frame-to-frame content change without storing pixels.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let step = (px.len() / 4096).max(1);
    let mut i = 0;
    while i < px.len() {
        hash ^= px[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += step;
    }
    let h32 = (hash as usize) & 0xffff_ffff;
    let last_slot = PROFILE_LOOKAT_RT_LASTSLOT.swap(chosen, Ordering::SeqCst);
    let last_hash = PROFILE_LOOKAT_RT_LASTHASH.swap(h32, Ordering::SeqCst);
    // Count motion only when the same slot was sampled consecutively (so a slot switch isn't "motion").
    if last_slot == chosen && h32 != last_hash {
        PROFILE_LOOKAT_RT_CHANGED.fetch_add(1, Ordering::SeqCst);
    }
}

/// DRAW-PHASE SWEEP diagnostic, run from a FrameBegin task (ticks every frame). Throttled: (1) re-read
/// the live phase selector `er-effects-lookat-phase.txt` (a single integer index 0..LOOKAT_DRAW_PHASE_COUNT)
/// into `PROFILE_LOOKAT_SELECTED_PHASE` so the active draw phase can be switched without recompiling; and
/// (2) log each candidate phase's per-frame tick count + the draw count, so one run reveals which phases
/// actually tick per-frame at the menu (the world-gated GameSceneDraw does not). No-op unless look-at is on.
/// Walk the look-at resolution chain for a fixed probe slot (0) and bump per-stage validity counters, so
/// the sweep log pinpoints exactly which deref drops from ~100% to ~11% (instead of guessing). Read-only.
unsafe fn profile_lookat_stage_probe(base: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    PROFILE_LOOKAT_STAGE_OK[7].fetch_add(1, Ordering::SeqCst); // frames probed
    let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, 0)) }.unwrap_or(0);
    if !valid(r)
        || unsafe { safe_read_usize(r) }.unwrap_or(0)
            != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
    {
        return;
    }
    PROFILE_LOOKAT_STAGE_OK[0].fetch_add(1, Ordering::SeqCst);
    let model = unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
    if valid(model) {
        PROFILE_LOOKAT_STAGE_OK[1].fetch_add(1, Ordering::SeqCst);
    }
    let x = unsafe { safe_read_usize(r + PROFILE_LOOKAT_ANIM_LOCATION_OFFSET) }.unwrap_or(0);
    if !valid(x) {
        return;
    }
    PROFILE_LOOKAT_STAGE_OK[2].fetch_add(1, Ordering::SeqCst);
    let importer = unsafe { safe_read_usize(x + PROFILE_LOOKAT_IMPORTER_OFFSET) }.unwrap_or(0);
    if !valid(importer) {
        return;
    }
    PROFILE_LOOKAT_STAGE_OK[3].fetch_add(1, Ordering::SeqCst);
    let holder = importer + PROFILE_LOOKAT_POSEHOLDER_OFFSET;
    let skel = unsafe { safe_read_usize(holder + POSEHOLDER_SKELETON_OFFSET) }.unwrap_or(0);
    if !valid(skel) {
        return;
    }
    PROFILE_LOOKAT_STAGE_OK[4].fetch_add(1, Ordering::SeqCst);
    let local = unsafe { safe_read_usize(holder + POSEHOLDER_LOCAL_BONE_DATA_OFFSET) }.unwrap_or(0);
    if valid(local) {
        PROFILE_LOOKAT_STAGE_OK[5].fetch_add(1, Ordering::SeqCst);
    }
    let count = unsafe { safe_read_i32(skel + HKA_SKELETON_BONES_SIZE_OFFSET) }.unwrap_or(0);
    if count > 0 && count as usize <= LOOKAT_MAX_BONES {
        PROFILE_LOOKAT_STAGE_OK[6].fetch_add(1, Ordering::SeqCst);
    }
}

pub(crate) fn profile_lookat_phase_diag_tick() {
    if !portrait_lookat_enabled() {
        return;
    }
    if let Ok(base) = game_module_base() {
        unsafe { profile_lookat_stage_probe(base) };
    }
    let n = PROFILE_LOOKAT_PHASE_DIAG_COUNTER.fetch_add(1, Ordering::SeqCst);
    if n % 60 == 0 {
        // Live phase selector: a single integer in er-effects-lookat-phase.txt picks the active draw phase.
        let path = game_directory_path()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("er-effects-lookat-phase.txt");
        if let Ok(s) = std::fs::read_to_string(&path) {
            if let Ok(idx) = s.trim().parse::<usize>() {
                if idx < LOOKAT_DRAW_PHASE_COUNT {
                    PROFILE_LOOKAT_SELECTED_PHASE.store(idx, Ordering::SeqCst);
                }
            }
        }
        // Refresh the cached selftest flag here (throttled) so the draw task never does a per-frame stat.
        PROFILE_LOOKAT_SELFTEST_ON.store(portrait_lookat_selftest_enabled(), Ordering::SeqCst);
        PROFILE_CURSOR_SWEEP_ON.store(portrait_cursor_sweep_enabled(), Ordering::SeqCst);
    }
    if n % 240 == 0 {
        let ticks: Vec<String> = (0..LOOKAT_DRAW_PHASE_COUNT)
            .map(|i| {
                format!(
                    "{}={}",
                    LOOKAT_DRAW_PHASE_NAMES[i],
                    PROFILE_LOOKAT_PHASE_TICKS[i].load(Ordering::SeqCst)
                )
            })
            .collect();
        let stages: Vec<String> = (0..PROFILE_LOOKAT_STAGE_COUNT)
            .map(|i| {
                format!(
                    "{}={}",
                    PROFILE_LOOKAT_STAGE_NAMES[i],
                    PROFILE_LOOKAT_STAGE_OK[i].load(Ordering::SeqCst)
                )
            })
            .collect();
        append_autoload_debug(format_args!(
            "lookat-phase-sweep: frame_begin={n} selected={}({}) selftest={} nowload={} loadbuilds={} render_drives={} hook_hits={} gx[samples={} nonempty={}] rt[samples={} nonblack={} changed={}] spared[ptr=0x{:x} model_ok={} draws={} hits={}] stage0[{}] phase_ticks[{}]",
            PROFILE_LOOKAT_SELECTED_PHASE.load(Ordering::SeqCst),
            LOOKAT_DRAW_PHASE_NAMES[PROFILE_LOOKAT_SELECTED_PHASE.load(Ordering::SeqCst)],
            PROFILE_LOOKAT_SELFTEST_ON.load(Ordering::SeqCst) as u8,
            game_module_base()
                .map(|b| unsafe { now_loading_active(b) } as u8)
                .unwrap_or(0),
            PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RENDER_DRIVES.load(Ordering::SeqCst),
            PROFILE_LOOKAT_HOOK_HITS.load(Ordering::SeqCst),
            PROFILE_GX_QUEUE_SAMPLES.load(Ordering::SeqCst),
            PROFILE_GX_QUEUE_NONEMPTY.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_SAMPLES.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_NONBLACK.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_CHANGED.load(Ordering::SeqCst),
            LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst),
            PROFILE_SPARED_MODEL_OK.load(Ordering::SeqCst),
            PROFILE_PERFRAME_SPARED_DRAWS.load(Ordering::SeqCst),
            PROFILE_PERFRAME_HOOK_HITS.load(Ordering::SeqCst),
            stages.join(" "),
            ticks.join(" ")
        ));
    }
    // Dense post-Continue capture: the now-loading window between the teardown-spare and world-load is
    // only ~2s on a fast gold-save load, far shorter than the 240-tick coarse sweep above. Once a renderer
    // has actually been spared (LOADING_BG_PORTRAIT_SPARED_RENDERER != 0), emit a compact sweep every 20
    // ticks so the post-Continue rasterization (model_ok / rt changed) is sampled inside that brief window.
    if LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst) != 0 && n % 20 == 0 {
        let spared_ptr = LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst);
        // Raw live read of renderer+model_ins: distinguishes the field being ZEROED (renderer detached from
        // its model) from a DANGLING pointer (field intact but the model object behind it freed).
        let model_raw =
            unsafe { safe_read_usize(spared_ptr + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
        // Liveness probe of the model OBJECT captured at record-time: read its first qword (vtable). If
        // the object is still mapped/live its vtable reads as a plausible pointer; if freed/unmapped the
        // read fails (cap_vt=0). This decides whether re-attaching cap_model into renderer+0x778 could
        // restore the portrait (object alive) or whether the model must be rebuilt/refcounted (freed).
        let cap_model = PROFILE_SPARE_CANDIDATE_MODEL.load(Ordering::SeqCst);
        let cap_vt = unsafe { safe_read_usize(cap_model) }.unwrap_or(0);
        // Scan the (re)built profile table: how many of the 10 slots now hold a valid CSMenuProfModelRend
        // (built[r]) and how many of those have a live model_ins (built[m]). This is the DIRECT measure of
        // whether our own builder's fresh renderers are constructing + latching their own models post-
        // Continue -- independent of the spared (empty) renderer the rest of this line reports.
        let null = TITLE_OWNER_SCAN_START_ADDRESS;
        let (mut built_r, mut built_m) = (0u32, 0u32);
        if let Ok(b) = game_module_base() {
            for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
                let r =
                    unsafe { safe_read_usize(portrait_renderer_table_entry(b, s)) }.unwrap_or(0);
                if r != 0
                    && r != null
                    && unsafe { safe_read_usize(r) }.unwrap_or(0)
                        == b + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
                {
                    built_r += 1;
                    let m = unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                        .unwrap_or(0);
                    if m != 0 && m != null {
                        built_m += 1;
                    }
                }
            }
        }
        // CHAIN DIAGNOSTIC: for the autoload target slot's BUILT renderer, walk renderer -> +0xa8
        // (CSEzOffscreenRend) -> +0x10 (CSRuntimeTexResCap) -> +GX (CSGxTexture) -- the exact texture the
        // forge re-bind should publish. And read the bound container's CURRENT first-TexResCap GX. If
        // chain_gx != bound_gx, the re-bind is publishing the wrong (stale menu) texture, not our live RT.
        let (mut ch_r, mut ch_off, mut ch_trc, mut ch_gx, mut bound_gx) =
            (0usize, 0usize, 0usize, 0usize, 0usize);
        if let Ok(b) = game_module_base() {
            let slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(b, slot)) }.unwrap_or(0);
            if r != 0 && r != null {
                ch_r = r;
                ch_off = unsafe {
                    safe_read_usize(r + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
                }
                .unwrap_or(0);
                if ch_off != 0 && ch_off != null {
                    ch_trc = unsafe {
                        safe_read_usize(
                            ch_off + TITLE_CUSTOM_COVER_PROFILE_OFFSCREEN_TEX_RESCAP_OFFSET,
                        )
                    }
                    .unwrap_or(0);
                    if ch_trc != 0 && ch_trc != null {
                        ch_gx = unsafe {
                            safe_read_usize(
                                ch_trc + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET,
                            )
                        }
                        .unwrap_or(0);
                    }
                }
            }
            // Bound container's first TexResCap GX (what the loading screen actually samples).
            let cap = LOADING_BG_TEXTURE_REDIRECT_LAST_PORTRAIT.load(Ordering::SeqCst);
            if cap != 0 && cap != null {
                let container =
                    unsafe { safe_read_usize(cap + TPF_FILE_CAP_TEX_RESCAP_OFFSET) }.unwrap_or(0);
                if container != 0 && container != null {
                    let array =
                        unsafe { safe_read_usize(container + TPF_RESCAP_CONTAINER_ARRAY_OFFSET) }
                            .unwrap_or(0);
                    if array != 0 && array != null {
                        let trc0 = unsafe { safe_read_usize(array) }.unwrap_or(0);
                        if trc0 != 0 && trc0 != null {
                            bound_gx = unsafe {
                                safe_read_usize(
                                    trc0 + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET,
                                )
                            }
                            .unwrap_or(0);
                        }
                    }
                }
            }
        }
        append_autoload_debug(format_args!(
            "loading-portrait-chain: built_slot_r=0x{ch_r:x} off=0x{ch_off:x} trc=0x{ch_trc:x} chain_gx=0x{ch_gx:x} | bound_gx=0x{bound_gx:x} copies={} rt[rgb_max={} alpha_max={}] boundtex[rgb_max={} alpha_max={}]",
            PROFILE_RT_SRV_COPIES.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_RGB_MAX.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_ALPHA_MAX.load(Ordering::SeqCst),
            PROFILE_BOUND_GX_RGB_MAX.load(Ordering::SeqCst),
            PROFILE_BOUND_GX_ALPHA_MAX.load(Ordering::SeqCst),
        ));
        append_autoload_debug(format_args!(
            "lookat-spared-sweep: frame={n} nowload={} loadbuilds={} built[r={built_r} m={built_m}] rebind[n={} gx=0x{:x}] model_raw=0x{model_raw:x} cap_model=0x{cap_model:x} cap_vt=0x{cap_vt:x} spared[ptr=0x{:x} model_ok={} draws={} hits={}] rt[samples={} nonblack={} changed={}]",
            game_module_base()
                .map(|b| unsafe { now_loading_active(b) } as u8)
                .unwrap_or(0),
            PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst),
            LOADING_BG_LIVE_GX_REBINDS.load(Ordering::SeqCst),
            LOADING_BG_LIVE_GX_BOUND.load(Ordering::SeqCst),
            LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst),
            PROFILE_SPARED_MODEL_OK.load(Ordering::SeqCst),
            PROFILE_PERFRAME_SPARED_DRAWS.load(Ordering::SeqCst),
            PROFILE_PERFRAME_HOOK_HITS.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_SAMPLES.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_NONBLACK.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_CHANGED.load(Ordering::SeqCst),
        ));
    }
}

/// One candidate draw-phase task tick (registered once per phase index). Always bumps that phase's
/// per-frame tick counter (for the sweep), and drives the realtime look-at draw ONLY when this phase is
/// the selected active one -- so exactly one phase rasterizes per frame regardless of how many are registered.
pub(crate) unsafe fn profile_lookat_phase_draw_tick(phase_index: usize) {
    if phase_index < LOOKAT_DRAW_PHASE_COUNT {
        PROFILE_LOOKAT_PHASE_TICKS[phase_index].fetch_add(1, Ordering::SeqCst);
    }
    if PROFILE_LOOKAT_SELECTED_PHASE.load(Ordering::SeqCst) != phase_index {
        return;
    }
    if let Ok(base) = game_module_base() {
        unsafe { profile_lookat_realtime_draw_tick(base) };
    }
}

/// HOOK on the per-frame per-model PUSH task (deobf 0x140bba6e0). For our profile renderers, write the
/// cursor/sinusoid Head/Neck/Spine2 rotation into the importer PoseHolder (+ recompute its model-space)
/// BEFORE the original runs, so the original's submodel propagation (FUN_1409e9ac0) copies OUR pose into
/// every submodel's modelSpaceBoneData -- the buffer the GPU actually skins from -- using the engine's
/// own (correct) `frame` arg. This is the fix for "head doesn't move": our prior code wrote the importer
/// PoseHolder but never propagated to the submodels. Fires per model per frame (only when the model is
/// live), so it naturally tracks the engine's model build/teardown cycling.
pub(crate) unsafe extern "system" fn per_frame_push_hook(renderer: usize, frame: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if portrait_lookat_enabled() && renderer != 0 && renderer != null {
        if let Ok(base) = game_module_base() {
            let vt_ok = unsafe { safe_read_usize(renderer) }.unwrap_or(0)
                == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA;
            if vt_ok {
                // Map renderer -> slot index (the look-at indices/base are cached per slot by the
                // FrameBegin apply_profile_lookat); skip if this renderer isn't in the profile table.
                let mut slot = usize::MAX;
                for s in 0..TITLE_PROFILE_SLOT_COUNT {
                    if unsafe { safe_read_usize(portrait_renderer_table_entry(base, s as i32)) }
                        .unwrap_or(0)
                        == renderer
                    {
                        slot = s;
                        break;
                    }
                }
                // Post-Continue the menu table is torn down, so the SPARED renderer isn't in it: map it to
                // its original autoload slot, whose cached look-at indices (base re-latches) we reuse.
                if slot == usize::MAX
                    && renderer == LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst)
                {
                    let own = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
                    if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&own) {
                        slot = own as usize;
                    }
                }
                if slot != usize::MAX {
                    if let Some(holder) = unsafe { profile_pose_holder(renderer) } {
                        let yaw =
                            f32::from_bits(PROFILE_LOOKAT_YAW_BITS.load(Ordering::SeqCst) as u32);
                        let pitch =
                            f32::from_bits(PROFILE_LOOKAT_PITCH_BITS.load(Ordering::SeqCst) as u32);
                        if unsafe { lookat_apply_realtime(holder, slot, yaw, pitch) } {
                            PROFILE_PERFRAME_HOOK_HITS.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
            }
        }
    }
    let orig = PROFILE_PERFRAME_HOOK_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, usize) = unsafe { core::mem::transmute(orig) };
        unsafe { f(renderer, frame) };
    }
}

fn install_per_frame_push_hook() {
    if PROFILE_PERFRAME_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "perframe-push-hook: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(target) = game_rva(PROFILE_PER_FRAME_PUSH_RVA as u32) else {
        return;
    };
    match unsafe { MhHook::new(target as *mut c_void, per_frame_push_hook as *mut c_void) } {
        Ok(hook) => {
            PROFILE_PERFRAME_HOOK_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_err() {
                append_autoload_debug(format_args!(
                    "perframe-push-hook: queue_enable failed for 0x{target:x}"
                ));
                return;
            }
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "perframe-push-hook: MhHook::new failed: {status:?}"
            ));
            return;
        }
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "perframe-push-hook: installed on per-frame push 0x{target:x} (submodel pose propagation)"
        )),
        status => append_autoload_debug(format_args!(
            "perframe-push-hook: MH_ApplyQueued failed: {status:?}"
        )),
    }
}

/// CAMERA LEVER: override one profile renderer's orbit camera with a custom viewport (closer, off-axis
/// framing), proving the lever on the still dump. Replicates the tail of the engine's own camera routine
/// `FUN_140bbe190` WITHOUT its `MenuOffscrRendParam` read (so it never clobbers our override): latch the
/// engine baseline once, write the orbit fields from `baseline + offsets`, rebuild the view matrix via
/// the engine builder, copy it into the renderer's matrix slot, then push the CSPersCam into the
/// offscreen render. Re-applied every tick so a refresh that re-runs the engine setup can't win.
/// `renderer` must already be a validated live CSMenuProfModelRend (vtable checked by the caller).
/// Returns true once the camera was pushed. See bd `camera-lever-RE-VERIFIED-offsets-and-call-addrs-2026-06-29`.
unsafe fn apply_profile_camera_override(base: usize, renderer: usize, slot: i32) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if renderer == 0 || renderer == null {
        return false;
    }
    let idx = slot as usize;
    if idx >= TITLE_PROFILE_SLOT_COUNT {
        return false;
    }
    let read_f32 = |off: usize| -> Option<f32> {
        unsafe { safe_read_i32(renderer + off) }.map(|b| f32::from_bits(b as u32))
    };
    // The push dereferences the offscreen-render pointer at renderer+0xa8; if it is not populated yet
    // (or has been torn down) skip entirely, so the engine push can never fault on a null offscreen.
    if unsafe {
        safe_read_usize(renderer + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
    }
    .unwrap_or(0)
        == 0
    {
        return false;
    }
    // Latch the engine baseline ONCE per slot, BEFORE the first override write, so all overrides derive
    // from an immutable baseline. The lock is never held across a game call.
    let baseline = {
        let mut guard = match PROFILE_CAM_BASELINE.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if guard[idx].is_none() {
            let (Some(tx), Some(ty), Some(tz), Some(dist), Some(pitch), Some(yaw), Some(fov)) = (
                read_f32(PROFILE_CAM_TARGET_OFFSET),
                read_f32(PROFILE_CAM_TARGET_OFFSET + 4),
                read_f32(PROFILE_CAM_TARGET_OFFSET + 8),
                read_f32(PROFILE_CAM_DISTANCE_OFFSET),
                read_f32(PROFILE_CAM_PITCH_OFFSET),
                read_f32(PROFILE_CAM_YAW_OFFSET),
                read_f32(PROFILE_CAM_FOV_OFFSET),
            ) else {
                return false;
            };
            // The engine frames the head at a real positive distance with a real fov, and the target /
            // angles are finite. If anything is not set yet (0 / NaN), skip latching this tick and retry
            // once the ctor camera setup has run -- so a degenerate baseline is never captured.
            if !(dist.is_finite()
                && dist > 0.001
                && fov.is_finite()
                && fov > 0.0
                && tx.is_finite()
                && ty.is_finite()
                && tz.is_finite()
                && pitch.is_finite()
                && yaw.is_finite())
            {
                return false;
            }
            guard[idx] = Some(ProfileCamBaseline {
                target: [tx, ty, tz],
                distance: dist,
                pitch,
                yaw,
                fov,
            });
            PROFILE_CAM_LATCHED_MASK.fetch_or(1usize << idx, Ordering::SeqCst);
        }
        guard[idx].unwrap()
    };
    // Custom viewport derived from the immutable baseline.
    let target = baseline.target;
    let distance = baseline.distance * PROFILE_CAM_DISTANCE_SCALE;
    let pitch = baseline.pitch + PROFILE_CAM_PITCH_DELTA_RAD;
    let yaw = baseline.yaw + PROFILE_CAM_YAW_DELTA_RAD;
    let fov = baseline.fov * PROFILE_CAM_FOV_SCALE;
    // Write the orbit fields (mirrors `FUN_140bbe190`'s field writes, minus the param read). The
    // renderer is a validated live object spanning well past +0xa24, so direct volatile writes are safe.
    unsafe {
        core::ptr::write_volatile(
            (renderer + PROFILE_CAM_TARGET_OFFSET) as *mut f32,
            target[0],
        );
        core::ptr::write_volatile(
            (renderer + PROFILE_CAM_TARGET_OFFSET + 4) as *mut f32,
            target[1],
        );
        core::ptr::write_volatile(
            (renderer + PROFILE_CAM_TARGET_OFFSET + 8) as *mut f32,
            target[2],
        );
        core::ptr::write_volatile((renderer + PROFILE_CAM_TARGET_W_OFFSET) as *mut f32, 1.0);
        core::ptr::write_volatile(
            (renderer + PROFILE_CAM_DISTANCE_OFFSET) as *mut f32,
            distance,
        );
        core::ptr::write_volatile((renderer + PROFILE_CAM_PITCH_OFFSET) as *mut f32, pitch);
        core::ptr::write_volatile((renderer + PROFILE_CAM_YAW_OFFSET) as *mut f32, yaw);
        core::ptr::write_volatile((renderer + PROFILE_CAM_FOV_OFFSET) as *mut f32, fov);
    }
    // Rebuild the view matrix with the engine's own builder (correct handedness/basis), then copy the 16
    // floats into the renderer's matrix slot (== the CSPersCam view matrix).
    let build: unsafe extern "system" fn(usize, *mut f32) -> *mut f32 =
        unsafe { core::mem::transmute(base + PROFILE_CAM_BUILD_MATRIX_RVA) };
    let mut matrix = [0f32; 16];
    unsafe { build(renderer, matrix.as_mut_ptr()) };
    if !matrix.iter().all(|f| f.is_finite()) {
        PROFILE_CAM_LAST_MATRIX_OK.store(0, Ordering::SeqCst);
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            matrix.as_ptr(),
            (renderer + PROFILE_CAM_VIEW_MATRIX_OFFSET) as *mut f32,
            16,
        );
    }
    // Push the CSPersCam into the offscreen render so the next offscreen frame uses our camera.
    let push: unsafe extern "system" fn(usize, usize) =
        unsafe { core::mem::transmute(base + PROFILE_CAM_PUSH_RVA) };
    unsafe { push(renderer, renderer + PROFILE_CAM_PERSCAM_OFFSET) };
    PROFILE_CAM_APPLY_CALLS.fetch_add(1, Ordering::SeqCst);
    PROFILE_CAM_LAST_SLOT.store(idx, Ordering::SeqCst);
    PROFILE_CAM_LAST_MATRIX_OK.store(1, Ordering::SeqCst);
    true
}

/// True while the engine's now-loading screen is active (reads the NowLoading singleton the telemetry
/// uses: helper = *(base+NowLoadingSingleton); flag = *(helper+loading_flag) & 0xff). Fault-guarded.
pub(crate) unsafe fn now_loading_active(base: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let helper = unsafe { safe_read_usize(base + RuntimeGlobalRva::NowLoadingSingleton as usize) }
        .unwrap_or(0);
    if helper == 0 || helper == null {
        return false;
    }
    let off = core::mem::offset_of!(NowLoadingHelperLayout, loading_flag);
    unsafe { safe_read_usize(helper + off) }
        .map(|v| (v & 0xff) != 0)
        .unwrap_or(false)
}

/// True while the "fake" loading screen (the Continue->world transition cover) is VISIBLE: helper =
/// *(base+FakeLoadingScreenSingleton); visible = *(helper+0x8) & 0xff. This is the continuous signal for
/// the menu->world loading screen the portrait belongs on -- distinct from `now_loading_active`, which reads
/// the in-world NowLoading streaming singleton and stays 0 during this menu-background phase. Fault-guarded.
pub(crate) unsafe fn fake_loading_screen_visible(base: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let helper =
        unsafe { safe_read_usize(base + RuntimeGlobalRva::FakeLoadingScreenSingleton as usize) }
            .unwrap_or(0);
    if helper == 0 || helper == null {
        return false;
    }
    unsafe { safe_read_usize(helper + 0x8) }
        .map(|v| (v & 0xff) != 0)
        .unwrap_or(false)
}

/// POST-CONTINUE PORTRAIT: when the now-loading screen is up but the profile-renderer title table has been
/// torn down (native-continue is menu-free, so the menu never built it, or Continue tore it down), call
/// the engine's own builder ONCE to repopulate the 10-slot table. The existing mark+refresh feed +
/// per-frame look-at hook + draw + pixel oracle then re-engage on the loading screen automatically (they
/// all key off this table). Latched per load (reset when now-loading drops) so there's no per-frame churn.
pub(crate) unsafe fn maybe_build_profile_table_for_loading(base: usize) {
    if !portrait_lookat_enabled() {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == 0 || base == null {
        return;
    }
    // If the table is already populated (menu built it, or our own build already ran), leave it -- the
    // existing mark+refresh feed + look-at + draw + oracle drive it. A live table also RE-ARMS the latch:
    // a subsequent Continue teardown empties it again and we rebuild our own for that load window.
    let t0 = unsafe { safe_read_usize(portrait_renderer_table_entry(base, 0)) }.unwrap_or(0);
    let populated = t0 != 0
        && t0 != null
        && unsafe { safe_read_usize(t0) }.unwrap_or(0)
            == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA;
    if populated {
        PROFILE_TABLE_EMPTY_STREAK.store(0, Ordering::SeqCst);
        PROFILE_TABLE_WAS_POPULATED.store(1, Ordering::SeqCst);
        PROFILE_LOADSCREEN_REBUILT.store(0, Ordering::SeqCst);
        return;
    }
    // Table is EMPTY this tick -- count the streak. The menu's own teardown+rebuild is synchronous, so a
    // sustained-empty table across ticks means the Continue teardown ran with no menu rebuild (we've left
    // the menu into the load), which happens ~17s -- well before the now-loading flag flips (~21s on the
    // fast gold-save load). Build as soon as EITHER signal fires so ResMan has time to build the model.
    let streak = PROFILE_TABLE_EMPTY_STREAK.fetch_add(1, Ordering::SeqCst) + 1;
    if PROFILE_LOADSCREEN_REBUILT.load(Ordering::SeqCst) != 0 {
        return; // already built our table for this load window
    }
    // HARD SAFETY: never call the builder until the menu has built a table at least once. At the title
    // screen the table is empty too, but the engine/ResMan are not up and the builder access-violates.
    if PROFILE_TABLE_WAS_POPULATED.load(Ordering::SeqCst) == 0 {
        return;
    }
    let nowload = unsafe { now_loading_active(base) };
    if !(nowload || streak >= PROFILE_TABLE_EMPTY_STREAK_BUILD_THRESHOLD) {
        return;
    }
    // Build it via the engine's own 10-slot builder (teardown is a no-op on a null table). Each fresh
    // CSMenuProfModelRend self-registers its ResMan model build/draw tasks, so it builds + OWNS its own
    // model with our lifetime -- not borrowed from the torn-down menu. Self-contained off process-lifetime
    // singletons (RE-confirmed).
    let builder: unsafe extern "system" fn() =
        unsafe { core::mem::transmute(base + PROFILE_TABLE_BUILDER_RVA) };
    unsafe { builder() };
    // Kick the model build THIS tick: the mark+refresh feed that requests the async character-model build
    // only runs every 240 ticks (counter % 240 == 0). The post-Continue now-loading window is shorter than
    // 240 ticks, so without this the freshly-built renderers are never fed -> they stay model-less (m=0).
    // Resetting the counter to 0 makes the feed fire on the very next pass through force_profile_render_tick.
    PROFILE_FORCE_TICK_COUNTER.store(0, Ordering::SeqCst);
    // Open the post-Continue feed window so the mark+refresh runs frequently (not just every 240 ticks) and
    // drives the async ResMan model build to completion + keeps it latched through the loading screen.
    PROFILE_LOADSCREEN_FEED_TICKS.store(PROFILE_LOADSCREEN_FEED_WINDOW_TICKS, Ordering::SeqCst);
    PROFILE_LOADSCREEN_REBUILT.store(1, Ordering::SeqCst);
    PROFILE_LOADSCREEN_TABLE_BUILDS.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "loading-portrait: empty profile table (trigger={} streak={streak}) -> called builder 0x{:x} to build our own renderers for the post-Continue portrait",
        if nowload {
            "now-loading"
        } else {
            "empty-streak"
        },
        base + PROFILE_TABLE_BUILDER_RVA
    ));
}

pub(crate) unsafe fn force_profile_render_tick(base: usize, _slot: i32) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == 0 || base == null {
        return;
    }
    let valid = |p: usize| p != 0 && p != null;
    // POST-CONTINUE PORTRAIT: before the table-ready guard below (which would early-return on the
    // torn-down post-Continue table), repopulate the table during now-loading so the rest of this tick
    // (mark+refresh feed) and the look-at/draw/oracle run on the loading screen.
    unsafe { maybe_build_profile_table_for_loading(base) };
    // VISIBILITY: once our built renderer's offscreen RT is live, swap it into the now-loading background
    // container the forge already injected (the background binds BEFORE our renderer exists and never
    // re-binds, so the live RT must be pushed into the displayed container after the fact).
    unsafe { refresh_loading_bg_live_gx(base) };
    // Once the real IBL-lit menu portrait has been baked into LOADING_BG_PORTRAIT_RGBA, re-forge the first
    // (displayed) now-loading rti so the loading screen swaps the checker for the real character portrait.
    unsafe { maybe_reforge_loading_portrait(base) };
    // HIGHER-RES (one-shot, EARLY -- runs before the table-ready guard below so it lands before
    // TitleTopDialog constructs the renderers). Patch each per-slot offscreen base-size entry that
    // still holds the init value (128x128, written by FUN_1400a7bb0) to 1024x1024 base AND zero the
    // per-slot supersample-enable byte (+0x8) so the env-dependent x2 is off -> a predictable
    // 1024x1024 RT. The .data table is writable (the game's own init writes it), so a direct volatile
    // write suffices. Self-validating: only entries still holding the exact init value are touched.
    if portrait_real_pixels_enabled() && PROFILE_SIZE_PATCHED.swap(1, Ordering::SeqCst) == 0 {
        let table = base + PROFILE_OFFSCREEN_SIZE_TABLE_RVA;
        let mut patched = 0u32;
        for s in 0..10usize {
            let row = table + s * PROFILE_OFFSCREEN_SIZE_TABLE_STRIDE;
            if unsafe { safe_read_usize(row) } == Some(PROFILE_OFFSCREEN_SIZE_INIT) {
                unsafe {
                    core::ptr::write_volatile(
                        row as *mut u64,
                        PROFILE_OFFSCREEN_SIZE_TARGET as u64,
                    );
                    core::ptr::write_volatile(
                        (row + PROFILE_OFFSCREEN_SIZE_SUPERSAMPLE_FLAG_OFFSET) as *mut u8,
                        0,
                    );
                }
                patched += 1;
            }
        }
        append_autoload_debug(format_args!(
            "higher-res: patched {patched}/10 offscreen-size entries -> 1024x1024 base, supersample off"
        ));
    }
    // ProfileSummary = GameDataMan -> slot-manager container.
    let gdm = game_data_man_ptr_or_null();
    if !valid(gdm) {
        return;
    }
    let summary = unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(0);
    if !valid(summary) {
        return;
    }
    // GUARD (crash fix): only call refresh once the renderer table is LIVE -- it is populated at
    // TitleTopDialog ctor (main menu), NOT at early title. Calling refresh before the table exists
    // AVs inside refresh (observed crash rva 0x9aa6d4 = refresh+0x54 at +8939ms). Require slot-0's
    // table entry to be a valid CSMenuProfModelRend before marking/refreshing.
    let probe = unsafe { safe_read_usize(portrait_renderer_table_entry(base, 0)) }.unwrap_or(0);
    if !valid(probe)
        || unsafe { safe_read_usize(probe) }.unwrap_or(0)
            != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
    {
        return;
    }
    // IMMEDIATE BUILD KICK (regression fix -- goal issue 1, grounded in the 06-29 vs 06-30 capture diff):
    // the 240-tick / feed cadence below can fire BEFORE the native boot ProfileSummary read makes the
    // autoload target slot real (~+17s). When it does, the mark loop marks 0 real slots, refresh requests
    // nothing, the renderer's +0x754 "load-requested" latch stays 0, and the model never builds in the
    // brief now-loading window -> nothing to capture (06-30 runs: req754=0 req755=0 model=0x0). 06-29 runs
    // that captured a portrait marked the slot WHILE refresh ran (req755=1 -> model=0x<nonzero>); the
    // all-slots-mark removal (correctly gated on a real fingerprint to avoid contaminating empty slots'
    // saveSlotsStates) lost that build-request for slot 0 because the cadence no longer coincides with the
    // moment the slot goes real. So here, edge-triggered: the instant a slot's fingerprint is real AND its
    // renderer's +0x754 is still 0, mark + refresh it immediately (off-cadence) and open the feed window to
    // drive the async build to completion. Idempotent -- once +0x754 latches to 1 this no-ops, so no churn.
    // Only marks REAL slots (post-read), identical to the cadence loop's gate, so it can't pre-empt the read.
    // ONLY THE LOADED SLOT (2026-06-30, user: a DIFFERENT character showed on the loading screen). The
    // save holds multiple characters (all 10 slots build models), and the slot-0 readback grabbed a
    // neighbouring slot's identical-size 1024 RT -> wrong face. Build + mark ONLY the autoload target slot
    // so the loaded character (Banon, slot 0) is the ONLY portrait model that exists -> no wrong-slot grab.
    let target_slot = {
        let s = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
        if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&s) {
            s
        } else {
            0
        }
    };
    {
        let mark: unsafe extern "system" fn(usize, i32) -> u8 =
            unsafe { core::mem::transmute(base + PROFILE_MARK_SLOT_USED_RVA) };
        let mut kicked = 0u32;
        let mut kicked_mask = 0u32;
        for s in 0..10i32 {
            if s != target_slot {
                continue;
            }
            if !unsafe { profile_slot_fingerprint(s).0 } {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if !valid(r)
                || unsafe { safe_read_usize(r) }.unwrap_or(0)
                    != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                continue;
            }
            // +0x754 = the refresh's "load-requested" idempotency latch. 0 = the async model build was
            // never kicked for this slot -> kick it now. Non-zero -> already requested, skip.
            if unsafe { safe_read_u8(r + 0x754) }.unwrap_or(0xff) != 0 {
                continue;
            }
            let _ = unsafe { mark(summary, s) };
            kicked += 1;
            kicked_mask |= 1 << s;
        }
        if kicked > 0 {
            let refresh: unsafe extern "system" fn() =
                unsafe { core::mem::transmute(base + PROFILE_RENDERER_REFRESH_RVA) };
            unsafe { refresh() };
            // Drive the freshly-requested build to completion + keep it latched through the loading screen.
            PROFILE_LOADSCREEN_FEED_TICKS.store(PROFILE_LOADSCREEN_FEED_WINDOW_TICKS, Ordering::SeqCst);
            if PROFILE_REAL_SLOT_KICK_LOGGED.swap(1, Ordering::SeqCst) == 0 {
                append_autoload_debug(format_args!(
                    "force-profile-render: IMMEDIATE build kick -- {kicked} real slot(s) (mask=0x{kicked_mask:x}) became available with req754=0; marked + refreshed off-cadence + opened feed window (summary=0x{summary:x})"
                ));
            }
        }
    }
    // MODEL BUILD: every ~240 ticks, mark all 10 profile slots used + call the refresh that kicks the
    // async character-model build. refresh is IDEMPOTENT per slot via the +0x754 "load-requested" latch,
    // so by default this builds each model ONCE and then leaves it -- the model stays LIVE every frame,
    // which is what the realtime look-at draw needs (an invalid/rebuilding pose-holder fails the draw).
    //
    // DESTRUCTIVE REBUILD (default OFF, `portrait_force_rebuild_enabled`): clear each build latch
    // (+0x754/+0x755) + reset the look-at slot cache to force a FRESH build. The churn leaves models
    // not-live most of the time (~88% draw failures -> flicker), so it is opt-in: flip it on briefly to
    // re-capture the post-FaceData face (an early build before LOAD GAME loads FaceData = default head),
    // then off. See `portrait_force_rebuild_enabled` and bd portrait-lookat-realtime-drawphase-design.
    let counter = PROFILE_FORCE_TICK_COUNTER.fetch_add(1, Ordering::SeqCst);
    // Post-Continue feed window: while it is open, run the (idempotent) mark+refresh every 8 ticks so the
    // freshly-built renderers' async model build is driven to completion and stays latched -- the once-per-
    // 240 baseline is too sparse for the brief now-loading window. Outside the window keep the 240 cadence.
    let feed_window = PROFILE_LOADSCREEN_FEED_TICKS.load(Ordering::SeqCst) > 0;
    if feed_window {
        PROFILE_LOADSCREEN_FEED_TICKS.fetch_sub(1, Ordering::SeqCst);
    }
    if counter % 240 == 0 || (feed_window && counter % 8 == 0) {
        let log_this = counter % 240 == 0; // throttle the in-window feed log to once per 240
        let force_rebuild = portrait_force_rebuild_enabled();
        let mark: unsafe extern "system" fn(usize, i32) -> u8 =
            unsafe { core::mem::transmute(base + PROFILE_MARK_SLOT_USED_RVA) };
        let mut marked = 0u32;
        for s in 0..10i32 {
            // ONLY the autoload target slot (the loaded character). Building every real slot rendered all
            // the save's other characters, and the slot-0 readback grabbed a wrong one -> wrong face shown.
            if s != target_slot {
                continue;
            }
            // Real-character gate (per the native boot ProfileSummary read: level>=1 + non-empty name).
            // Never mark before the read populates the slot (can't pre-empt it / contaminate saveSlotsStates).
            if !unsafe { profile_slot_fingerprint(s).0 } {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if force_rebuild
                && valid(r)
                && unsafe { safe_read_usize(r) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                unsafe {
                    core::ptr::write_volatile((r + 0x754) as *mut u8, 0);
                    core::ptr::write_volatile((r + 0x755) as *mut u8, 0);
                }
            }
            let _ = unsafe { mark(summary, s) };
            marked += 1;
        }
        let refresh: unsafe extern "system" fn() =
            unsafe { core::mem::transmute(base + PROFILE_RENDERER_REFRESH_RVA) };
        unsafe { refresh() };
        if log_this {
            append_autoload_debug(format_args!(
                "force-profile-render: build cycle (counter={counter}) force_rebuild={force_rebuild} feed_window={feed_window} -- marked {marked} real slot(s) + refreshed (summary=0x{summary:x})"
            ));
        }
        // Only when we forced a fresh build: drop the cached look-at indices/base so they re-resolve and
        // re-latch the idle base from the fresh skeleton. Without a forced rebuild the model (and its
        // skeleton) persist, so KEEP the cache -> the look-at keeps driving every frame with no re-resolve gap.
        if force_rebuild {
            match PROFILE_LOOKAT_SLOTS.lock() {
                Ok(mut g) => *g = [None; 10],
                Err(p) => *p.into_inner() = [None; 10],
            }
            // The models are being rebuilt -> the cached PoseHolder pointers are about to go stale. Drop
            // them so they re-resolve against the fresh skeletons (and re-latch a clean base) before the
            // sticky-keep path above starts driving them again.
            for h in PROFILE_LOOKAT_HOLDERS.iter() {
                h.store(0, Ordering::SeqCst);
            }
        }
    }
    // ~80 ticks AFTER each rebuild kick, reset the dump mask so the freshly-rebuilt models (not the
    // stale pre-clear model_ins) get re-dumped. Each cycle's dumps overwrite the per-slot files.
    if counter % 240 == 80 {
        PROFILE_SLOT_DUMP_MASK.store(0, Ordering::SeqCst);
    }
    // CAMERA LEVER: every tick, override each live renderer's orbit camera with our custom viewport.
    // Re-applied so a refresh that re-runs the engine camera setup can't win; the dump loop below then
    // captures the custom-framed RT. Gated under the same `portrait_real_pixels` diagnostic as the dump.
    if portrait_real_pixels_enabled() {
        for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if valid(r)
                && unsafe { safe_read_usize(r) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                unsafe { apply_profile_camera_override(base, r, s) };
            }
        }
    }
    // LOOK-AT LEVER: every tick, rotate each live renderer's Head/Neck/Spine2 bones toward the mouse
    // cursor so the portrait's gaze follows it (eyes are welded to the Head bone). Separate gate from
    // the camera/dump so the riskier bone-write path can be toggled on its own.
    if portrait_lookat_enabled() {
        for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if valid(r)
                && unsafe { safe_read_usize(r) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                // FrameBegin role (this task): REGISTER the holder + resolve Head/Neck/Spine2 indices +
                // publish the cursor. The per-frame write+recompute+DRAW that makes the head track the
                // cursor in realtime now happens in `profile_lookat_realtime_draw_tick`, a separate
                // recurring task in the GameSceneDraw phase (render thread, inside a live GX frame). The
                // old per-tick game-task offscreen drive rendered black (FrameBegin = before the GX frame
                // records); the draw-phase task is the fix.
                unsafe { apply_profile_lookat(r, s) };
                // SPARE PRE-RECORD: capture the autoload target slot's renderer as the spare candidate on
                // a frame where its model is actually BUILT (+0x778 valid), so the teardown-spare hook can
                // protect this exact renderer through Continue even though the menu cycles model_ins. The
                // long menu dwell makes catching a built frame reliable.
                let target = {
                    let own = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
                    if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&own) {
                        own
                    } else {
                        0
                    }
                };
                if s == target
                    && PROFILE_SPARE_CANDIDATE.load(Ordering::SeqCst) == 0
                    && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                        .map(|m| valid(m))
                        .unwrap_or(false)
                {
                    PROFILE_SPARE_CANDIDATE.store(r, Ordering::SeqCst);
                    let model = unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                        .unwrap_or(0);
                    PROFILE_SPARE_CANDIDATE_MODEL.store(model, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "loading-portrait: pre-recorded spare candidate renderer=0x{r:x} slot={s} model_ins=0x{model:x} (model built at menu)"
                    ));
                }
            }
        }
    }
    // Per-slot: once a slot's model (+0x778) has built, readback its COLOR offscreen RT and dump to
    // portrait-capture-slot{N}.bin ONCE (tracked via PROFILE_SLOT_DUMP_MASK). Inspect the 10 dumps
    // offline and match to the known disk characters to map renderer-slot -> character.
    if portrait_real_pixels_enabled() {
        for s in 0..10i32 {
            let bit = 1usize << s;
            if PROFILE_SLOT_DUMP_MASK.load(Ordering::SeqCst) & bit != 0 {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if !valid(r)
                || unsafe { safe_read_usize(r) }.unwrap_or(0)
                    != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                continue;
            }
            let model =
                unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
            if !valid(model) {
                continue;
            }
            let off = unsafe {
                safe_read_usize(r + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
            }
            .unwrap_or(0);
            if !valid(off) {
                continue;
            }
            // LIGHTING residency oracle: envObj = renderer+0x760; *(envObj) is the registered IBL
            // env-region id, non-zero ONLY if the GILM env map was resident when the IBL built.
            let env_obj =
                unsafe { safe_read_usize(r + PROFILE_RENDERER_ENV_REGION_OFFSET) }.unwrap_or(0);
            let ibl_region = if valid(env_obj) {
                unsafe { safe_read_usize(env_obj) }.unwrap_or(0)
            } else {
                0
            };
            if let Some((w, h, px)) = unsafe { readback_offscreen_rgba8(off) } {
                let nb = portrait_center_nonblack(w, h, &px);
                let checker = portrait_looks_like_checker(w, h, &px);
                // BAKE SOURCE: store the TARGET slot's menu portrait into LOADING_BG_PORTRAIT_RGBA so the
                // now-loading forge bakes IT into the static TPF (the proven decode-time display path) AND the
                // present-overlay composite (gated on PROFILE_BAKE_RGBA_CAPTURED) displays it. ONLY latch on a
                // REAL FACE: nonblack alone false-passes the magenta/white checker (an unrendered RT or our
                // cover placeholder) -- latching that is exactly what put a center checker square on screen and
                // made oracle_..._gx_nonblack a false success. Requiring !checker means we keep re-checking each
                // dump cycle and latch only once a real shaded head has actually rendered into the offscreen
                // (which needs the render-thread offscreen drive -- see portrait_render_drive). One-shot via swap.
                if s == OWN_STEPPER_SLOT.load(Ordering::SeqCst)
                    && nb
                    && !checker
                    && PROFILE_BAKE_RGBA_CAPTURED.swap(1, Ordering::SeqCst) == 0
                {
                    let _ = ibl_region;
                    dump_portrait_rgba(110, w, h, &px);
                    if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
                        *g = Some((w, h, px.clone()));
                    }
                    append_autoload_debug(format_args!(
                        "loading-portrait: BAKE-CAPTURED real menu portrait slot={s} dims={w}x{h} ibl_region=0x{ibl_region:x} -> LOADING_BG_PORTRAIT_RGBA (forge will bake it)"
                    ));
                }
                dump_portrait_rgba(s, w, h, &px);
                PROFILE_SLOT_DUMP_MASK.fetch_or(bit, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "profile-slot-dump: slot={s} renderer=0x{r:x} model=0x{model:x} dims={w}x{h} nonblack={} env_obj=0x{env_obj:x} ibl_region=0x{ibl_region:x}",
                    nb as u8
                ));
            }
        }
    }
}

/// Hook on the CSMenuProfModelRend teardown-all (`FUN_1409b2f00`). One-shot: before the original
/// runs, save slot-0's renderer and null its table entry so the original's null-guarded delete
/// enqueue skips it -- sparing the loaded character's portrait renderer from the Continue teardown so
/// we can keep rendering it into the now-loading screen. The original then tears down slots 1-9.
pub(crate) unsafe extern "system" fn profile_renderer_teardown_spare_hook() {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    // Gate on the look-at/portrait feature OR product autoload -- the native-continue path does NOT set
    // PRODUCT_AUTOLOAD_ARMED, so gating on product_autoload alone never spared anything there.
    if LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst) == 0
        && (product_autoload_enabled() || portrait_lookat_enabled())
    {
        if let Ok(base) = game_module_base() {
            let slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
            let slot = if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot) {
                slot
            } else {
                0
            };
            // Prefer the PRE-RECORDED candidate (captured at the menu on a model-built frame -- robust to
            // the menu's model_ins cycling). Find its table slot and protect it. Fall back to reading
            // table[slot] + a model-built guard if no candidate was recorded.
            let candidate = PROFILE_SPARE_CANDIDATE.load(Ordering::SeqCst);
            let (renderer, table, spared_slot) = if valid(candidate) {
                // locate the candidate's table entry so we can null it
                let mut found = (candidate, 0usize, slot);
                for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
                    let te = portrait_renderer_table_entry(base, s);
                    if unsafe { safe_read_usize(te) }.unwrap_or(0) == candidate {
                        found = (candidate, te, s);
                        break;
                    }
                }
                found
            } else {
                let te = portrait_renderer_table_entry(base, slot);
                let r = unsafe { safe_read_usize(te) }.unwrap_or(0);
                let model_built = valid(r)
                    && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                        .map(|m| valid(m))
                        .unwrap_or(false);
                (if model_built { r } else { 0 }, te, slot)
            };
            if valid(renderer)
                && unsafe { safe_read_usize(renderer) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                LOADING_BG_PORTRAIT_SPARED_RENDERER.store(renderer, Ordering::SeqCst);
                PROFILE_RENDERER_SPARE_HITS.fetch_add(1, Ordering::SeqCst);
                // Null the table entry so the original's null-guarded delete-enqueue skips it.
                if table != 0 {
                    unsafe { (table as *mut usize).write_volatile(0) };
                }
                // Re-latch the look-at base from the post-Continue model (a different model instance).
                if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&spared_slot) {
                    let mut guard = match PROFILE_LOOKAT_SLOTS.lock() {
                        Ok(g) => g,
                        Err(p) => p.into_inner(),
                    };
                    if let Some(s) = guard[spared_slot as usize].as_mut() {
                        s.base_latched = false;
                    }
                }
                let model_at_spare =
                    unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                        .unwrap_or(0);
                append_autoload_debug(format_args!(
                    "loading-portrait: SPARED slot{spared_slot} renderer=0x{renderer:x} (candidate=0x{candidate:x}) model_ins=0x{model_at_spare:x} from teardown -- drive look-at + render it post-Continue"
                ));
            }
        }
    }
    let orig = PROFILE_RENDERER_TEARDOWN_HOOK_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn() = unsafe { std::mem::transmute(orig) };
        unsafe { f() };
    }
}

pub(crate) fn install_profile_renderer_teardown_spare_hook() {
    if PROFILE_RENDERER_TEARDOWN_HOOK_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "loading-portrait: teardown-spare MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(target) = game_rva(PROFILE_RENDERER_TEARDOWN_RVA as u32) else {
        return;
    };
    match unsafe {
        MhHook::new(
            target as *mut c_void,
            profile_renderer_teardown_spare_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            PROFILE_RENDERER_TEARDOWN_HOOK_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_err() {
                append_autoload_debug(format_args!(
                    "loading-portrait: teardown-spare queue_enable failed for 0x{target:x}"
                ));
                return;
            }
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "loading-portrait: teardown-spare MhHook::new failed: {status:?}"
            ));
            return;
        }
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            PROFILE_RENDERER_TEARDOWN_HOOK_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "loading-portrait: hooked profile-renderer teardown 0x{target:x} to spare slot0 for the now-loading portrait"
            ));
        }
        status => append_autoload_debug(format_args!(
            "loading-portrait: teardown-spare MH_ApplyQueued failed: {status:?}"
        )),
    }
}

/// Build a distinctive POC test-image TPF (magenta/yellow checker) whose single texture is named
/// EXACTLY `symbol`, so the CSScaleform pump's name-registration binds it to the now-loading image.
/// (Real loaded-character portrait pixels are a follow-up; this proves the injection + object shape.)
fn build_portrait_test_tpf(symbol: &str) -> Option<Vec<u8>> {
    // 1024x1024 to MATCH the captured menu-portrait dims, so once the real portrait is read back we can
    // D3D12-upload it straight into THIS (already-registered) displayed texture (the Scaleform pump binds
    // the name only on the first bind, so a re-forge can't swap it -- we overwrite the live pixels instead).
    let dds = er_tpf::DdsImage::checker(1024, 1024, 64, [255, 0, 255, 255], [255, 255, 0, 255])
        .to_dds_bytes_with(er_tpf::DdsHeaderMode::LegacyRgba8);
    er_tpf::Tpf::single_pc(symbol, dds, 1).build().ok()
}

/// Build the now-loading background TPF named exactly `symbol`. When `portrait_real_pixels_enabled()`
/// AND a live portrait readback is available (`LOADING_BG_PORTRAIT_RGBA` is `Some`), build the TPF
/// from the REAL rendered character-head RGBA8 pixels (uncompressed legacy-RGBA8 DDS). The engine
/// rebuilds a correct SRV from these bytes at `CreateTpfResCap` time -- the same mechanism that makes
/// the checker display correctly. Otherwise (default, or no capture yet) fall back to the proven
/// magenta/yellow checker, byte-for-byte unchanged.
fn build_portrait_tpf(symbol: &str) -> Option<Vec<u8>> {
    // ONE-HEAD CONSOLIDATION: when the live build-own path is active, the present-overlay composite is the
    // SOLE deterministic display. Baking the real head into the forge TPF here produces a SECOND head (it
    // displays when the forge wins the bind race -- user-observed). So in render-drive mode the forge stays
    // a neutral checker background; the overlay draws the one head on top.
    if portrait_real_pixels_enabled() && !portrait_render_drive_enabled() {
        if let Ok(slot) = LOADING_BG_PORTRAIT_RGBA.lock() {
            if let Some((w, h, px)) = slot.as_ref() {
                let dds = er_tpf::DdsImage {
                    width: *w,
                    height: *h,
                    pixels: px.clone(),
                }
                .to_dds_bytes_with(er_tpf::DdsHeaderMode::LegacyRgba8);
                return er_tpf::Tpf::single_pc(symbol, dds, 1).build().ok();
            }
        }
    }
    build_portrait_test_tpf(symbol)
}

/// `FUN_140d69880` (deobf `LOADING_BG_REPLACE_BIND_RVA`) full-replace: the producer's "bind a
/// TpfFileCap to this rti from the symbol" step. For the now-loading background symbols
/// `MENU_Load_NNNNN`, build our own portrait TPF named exactly the symbol, materialize it through the
/// game's in-memory `CreateTpfResCap` factory, wrap it in a freshly-allocated `TpfFileCap`
/// (loadState=4), set it + the symbol on the rti, and return 1 -- so the producer lists the rti and
/// the unmodified per-frame CSScaleform pump registers our texture name, making GFx composite the
/// portrait as the loading-screen background. Every other symbol (and any build/alloc failure)
/// tail-calls the original, so the stock random background renders unchanged.
pub(crate) unsafe extern "system" fn loading_bg_replace_bind_hook(rti: usize, symbol: usize) -> u8 {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = LOADING_BG_TEXTURE_REDIRECT_ORIG.load(Ordering::SeqCst);
    let call_orig = move || -> u8 {
        if orig != null && orig != HOOK_ORIGINAL_UNSET {
            let f: unsafe extern "system" fn(usize, usize) -> u8 =
                unsafe { std::mem::transmute(orig) };
            unsafe { f(rti, symbol) }
        } else {
            0
        }
    };
    let total = LOADING_BG_REPLACE_BIND_TOTAL_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    // Fire on the portrait-lookat path too, not just product autoload: the native-continue smoke arms the
    // portrait via portrait_lookat_enabled() and does NOT set product_autoload_enabled() (observed pae=false
    // on the MENU_Load binds), so gating on pae alone never forged. Mirrors the teardown-spare gating fix.
    let pae = product_autoload_enabled() || portrait_lookat_enabled();
    let sym = unsafe { read_dlstring_u16(symbol) };
    // Diagnostic: log the first calls' symbols (ungated) so we can confirm whether the now-loading
    // MENU_Load_ background symbols actually reach this bind function and how they decode.
    if total <= 48 {
        let (preview, len) = match &sym {
            Some((u, _)) => (utf16_ascii_preview(u), u.len()),
            None => ("<read-fail>".to_string(), 0),
        };
        append_autoload_debug(format_args!(
            "loading-portrait-probe: call#{total} pae={pae} rti=0x{rti:x} symlen={len} sym='{preview}'"
        ));
    }
    if !pae || rti == 0 || rti == null {
        return call_orig();
    }
    let Some((units, encoding)) = sym else {
        return call_orig();
    };
    let Ok(sym_string) = String::from_utf16(&units) else {
        return call_orig();
    };
    // The producer symbol is a virtual TPF path, e.g. "menutpfbnd:/00_Solo/MENU_Load_00008.tpf".
    // Extract the bare GFx image symbol ("MENU_Load_00008"); skip anything that is not a now-loading
    // background.
    let Some(tex_name) = extract_menu_load_tex_name(&sym_string) else {
        return call_orig();
    };
    let attempts = LOADING_BG_TEXTURE_REDIRECT_ATTEMPTS.fetch_add(1, Ordering::SeqCst) + 1;
    let Ok(base) = game_module_base() else {
        return call_orig();
    };
    let Some(cap) = (unsafe { forge_into_rti(base, rti, &tex_name, encoding, symbol) }) else {
        return call_orig();
    };
    let commits = LOADING_BG_TEXTURE_REDIRECT_COMMITS.fetch_add(1, Ordering::SeqCst) + 1;
    LOADING_BG_TEXTURE_REDIRECT_LAST_SYMBOL_MATCH.store(1, Ordering::SeqCst);
    LOADING_BG_TEXTURE_REDIRECT_LAST_PORTRAIT.store(cap, Ordering::SeqCst);
    // Remember the FIRST (displayed) rti + its name/encoding so we can RE-FORGE it once the real portrait
    // is baked (the sprite commits to this first bind, which happens before the portrait is captured).
    if LOADING_BG_FIRST_RTI
        .compare_exchange(0, rti, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        LOADING_BG_FIRST_ENCODING.store(encoding as usize, Ordering::SeqCst);
        if let Ok(mut g) = LOADING_BG_FIRST_TEX_NAME.lock() {
            *g = Some(tex_name.clone());
        }
    }
    let baked = LOADING_BG_PORTRAIT_RGBA
        .lock()
        .map(|g| g.is_some())
        .unwrap_or(false);
    if commits <= 8 {
        append_autoload_debug(format_args!(
            "loading-portrait: forged now-loading background symbol='{sym_string}' -> cap=0x{cap:x} baked_rgba={baked} tpf commits={commits} attempts={attempts}"
        ));
    }
    1
}

/// Build a now-loading TPF (baking LOADING_BG_PORTRAIT_RGBA if captured, else the checker), materialize it
/// through the game's in-memory CreateTpfResCap factory, wrap it in a fresh TpfFileCap, and bind it to
/// `rti`. `substr_symbol != 0` copies that DLString into the rti's symbol field (the initial forge);
/// pass 0 to leave the rti's existing symbol (a RE-FORGE of an already-bound rti). Returns the cap on
/// success. The PINs/refcount bump match the original forge so the CSScaleform GC can't free our graph.
unsafe fn forge_into_rti(
    base: usize,
    rti: usize,
    tex_name: &str,
    encoding: u8,
    substr_symbol: usize,
) -> Option<usize> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let name_z: Vec<u16> = tex_name.encode_utf16().chain(core::iter::once(0)).collect();
    let tpf_bytes = build_portrait_tpf(tex_name)?;
    let tpf_repo = unsafe { safe_read_usize(base + GLOBAL_TPF_REPOSITORY_RVA) }.unwrap_or(0);
    if tpf_repo == 0 {
        return None;
    }
    let create_rescap: unsafe extern "system" fn(
        usize,
        *const u16,
        *const u8,
        u64,
        u8,
        u32,
    ) -> usize = unsafe { std::mem::transmute(base + CREATE_TPF_RESCAP_RVA) };
    let container = unsafe {
        create_rescap(
            tpf_repo,
            name_z.as_ptr(),
            tpf_bytes.as_ptr(),
            tpf_bytes.len() as u64,
            0,
            0,
        )
    };
    if container == 0 || container == null {
        return None;
    }
    let main_heap = unsafe { safe_read_usize(base + GLOBAL_MAIN_HEAP_ALLOCATOR_RVA) }.unwrap_or(0);
    if main_heap == 0 {
        return None;
    }
    let heap_alloc: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(base + GAME_HEAP_ALLOC_RVA) };
    let cap = unsafe { heap_alloc(TPF_FILE_CAP_ALLOC_SIZE, TPF_FILE_CAP_ALLOC_ALIGN, main_heap) };
    if cap == 0 {
        return None;
    }
    let cap_ctor: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(base + TPF_FILE_CAP_CTOR_RVA) };
    unsafe { cap_ctor(cap, 0) };
    unsafe {
        ((cap + TPF_FILE_CAP_LOAD_STATE_OFFSET) as *mut u8)
            .write_volatile(TPF_FILE_CAP_LOADED_STATE)
    };
    let prev_flags = unsafe { safe_read_u8(cap + TPF_FILE_CAP_FLAGS_OFFSET) }.unwrap_or(0);
    unsafe {
        ((cap + TPF_FILE_CAP_FLAGS_OFFSET) as *mut u8)
            .write_volatile(prev_flags | TPF_FILE_CAP_READY_FLAG_BIT)
    };
    unsafe { ((cap + TPF_FILE_CAP_TEX_RESCAP_OFFSET) as *mut usize).write_volatile(container) };
    unsafe { ((rti + REPLACE_TEX_INFO_ENCODING_OFFSET) as *mut u8).write_volatile(encoding) };
    if substr_symbol != 0 {
        let substr: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(base + DLSTRING_WCHAR_SUBSTR_RVA) };
        unsafe {
            substr(
                rti + REPLACE_TEX_INFO_SYMBOL_OFFSET,
                substr_symbol,
                0,
                usize::MAX,
            )
        };
    }
    unsafe { ((rti + REPLACE_TEX_INFO_TPF_FILE_CAP_OFFSET) as *mut usize).write_volatile(cap) };
    unsafe { ((rti + REPLACE_TEX_INFO_READY_OFFSET) as *mut u8).write_volatile(0) };
    let rc = unsafe {
        &*((rti + REPLACE_TEX_INFO_REFCOUNT_OFFSET) as *const core::sync::atomic::AtomicI32)
    };
    rc.fetch_add(0x10000, Ordering::SeqCst);
    Some(cap)
}

/// Once the real portrait is baked into LOADING_BG_PORTRAIT_RGBA, OVERWRITE the displayed now-loading
/// background texture's PIXELS in place via D3D12 upload. Re-forging a new cap doesn't work -- the
/// Scaleform pump registers the texture by NAME only on the first bind and won't re-read a swapped cap --
/// so we keep the first forged (1024x1024 checker) texture the pump already bound and just replace its
/// pixels with the captured 1024 portrait. One-shot. Render/D3D12 work mirrors the slot-dump readback
/// which runs safely from this same game-thread task.
pub(crate) unsafe fn maybe_reforge_loading_portrait(base: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    if PROFILE_BAKE_RGBA_CAPTURED.load(Ordering::SeqCst) == 0 {
        return;
    }
    // LIVE RE-UPLOAD (version-gated): re-upload the displayed now-loading texture whenever the live feed
    // publishes a new frame (version advanced) so the loading-screen head TRACKS the look-at. The earlier
    // re-upload crash was the READBACK's D3D12 object SCAN racing teardown (now fixed: cached resource, no
    // re-scan); the upload writes into the already-bound, stable now-loading texture (the one-shot upload
    // persisted crash-free through the whole loading screen), so per-version re-upload is safe.
    let cur_ver = LOADING_BG_PORTRAIT_RGBA_VERSION.load(Ordering::SeqCst);
    if LOADING_BG_REFORGE_VERSION.load(Ordering::SeqCst) == cur_ver {
        return; // already uploaded this frame's content
    }
    // The DISPLAYED texture is the one the Scaleform sprite samples by NAME from GLOBAL_TexRepository --
    // NOT the forge's source container GX. Look it up: GetResCap(GLOBAL_TexRepository, name).gxTexture.
    let tex_repo = unsafe { safe_read_usize(base + GLOBAL_TEX_REPOSITORY_RVA) }.unwrap_or(0);
    if !valid(tex_repo) {
        return;
    }
    let tex_name = match LOADING_BG_FIRST_TEX_NAME.lock() {
        Ok(g) => match g.as_ref() {
            Some(s) => s.clone(),
            None => return,
        },
        Err(_) => return,
    };
    let name_z: Vec<u16> = tex_name.encode_utf16().chain(core::iter::once(0)).collect();
    let get_res_cap: unsafe extern "system" fn(usize, *const u16) -> usize =
        unsafe { std::mem::transmute(base + TEX_REPOSITORY_GET_RES_CAP_RVA) };
    let res_cap = unsafe { get_res_cap(tex_repo, name_z.as_ptr()) };
    if !valid(res_cap) {
        return;
    }
    let gx = unsafe { safe_read_usize(res_cap + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET) }
        .unwrap_or(0);
    if !valid(gx) {
        return;
    }
    // Snapshot the captured portrait pixels.
    let snapshot = match LOADING_BG_PORTRAIT_RGBA.lock() {
        Ok(g) => g.clone(),
        Err(_) => return,
    };
    let Some((w, h, px)) = snapshot else {
        return;
    };
    let ok = unsafe { upload_rgba_to_texture(gx, w, h, &px) };
    // VERIFY: read back the SAME gx right after the upload. If it now reads the portrait, the upload DID
    // land in this texture (so any remaining checker on screen means Scaleform samples a DIFFERENT copy);
    // if it still reads the checker (bright magenta/yellow, rgb~255 with high variance), find_d3d12_resource
    // picked the wrong same-size texture and the upload missed -> fixable by targeting deterministically.
    let mut verify_rgb = (0u8, 0u8, 0u8);
    if let Some((vw, vh, vpx)) = unsafe { readback_offscreen_rgba8(gx) } {
        let n = (vw as usize) * (vh as usize);
        if vpx.len() >= n * 4 {
            let (cx, cy) = (vw as usize / 2, vh as usize / 2);
            let idx = (cy * vw as usize + cx) * 4;
            verify_rgb = (vpx[idx], vpx[idx + 1], vpx[idx + 2]);
        }
    }
    // Advance the version latch ONLY on a successful upload (dims matched). On failure we leave the latch
    // behind so a later same-version frame can retry once -- but since the version only advances when the
    // live feed publishes a NEW frame, this never per-frame-hammers (the old dim-mismatch crash). Same dims
    // (1024) throughout the build-own path, so ok is reliably true.
    if ok {
        LOADING_BG_REFORGE_VERSION.store(cur_ver, Ordering::SeqCst);
    }
    if LOADING_BG_REFORGE_LOGGED.swap(1, Ordering::SeqCst) == 0 {
        append_autoload_debug(format_args!(
            "loading-portrait: UPLOADED real portrait {w}x{h} into displayed now-loading texture gx=0x{gx:x} ok={ok} verify_center_rgb=({},{},{}) (loading screen now shows the LIVE character, re-uploads per version)",
            verify_rgb.0, verify_rgb.1, verify_rgb.2
        ));
    }
    let _ = base;
}

pub(crate) fn install_loading_bg_replace_bind_hook() {
    if LOADING_BG_TEXTURE_REDIRECT_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "loading-portrait: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(target) = game_rva(LOADING_BG_REPLACE_BIND_RVA as u32) else {
        return;
    };
    match unsafe {
        MhHook::new(
            target as *mut c_void,
            loading_bg_replace_bind_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            LOADING_BG_TEXTURE_REDIRECT_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_err() {
                append_autoload_debug(format_args!(
                    "loading-portrait: queue_enable failed for replace-bind 0x{target:x}"
                ));
                return;
            }
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "loading-portrait: replace-bind MhHook::new failed: {status:?}"
            ));
            return;
        }
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            LOADING_BG_TEXTURE_REDIRECT_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "loading-portrait: hooked now-loading replace-bind 0x{target:x}; will forge a portrait TPF for {LOADING_BG_SYMBOL_PREFIX}NNNNN backgrounds under product autoload"
            ));
        }
        status => append_autoload_debug(format_args!(
            "loading-portrait: MH_ApplyQueued failed: {status:?}"
        )),
    }
}

pub(crate) unsafe extern "system" fn title_menu_resource_acquire_observer_hook(
    this: usize,
    load_params: usize,
    param3: u8,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let filename_ptr = if load_params != 0 && load_params != null {
        unsafe { safe_read_usize(load_params + 0x8) }.unwrap_or(null)
    } else {
        null
    };
    let hit = TITLE_MENU_RESOURCE_ACQUIRE_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    let caller_rva = trace_first_game_caller_rva();
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_THIS.store(this, Ordering::SeqCst);
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_LOAD_PARAMS.store(load_params, Ordering::SeqCst);
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_FILENAME_PTR.store(filename_ptr, Ordering::SeqCst);
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_PARAM3.store(param3 as usize, Ordering::SeqCst);
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    let is_title_logo = unsafe { wide_ascii_contains_ci(filename_ptr, b"05_001_title_logo") }
        || unsafe { wide_ascii_contains_ci(filename_ptr, b"05_001_title") };

    let orig = TITLE_MENU_RESOURCE_ACQUIRE_ORIG.load(Ordering::SeqCst);
    let ret = if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, usize, u8) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(this, load_params, param3) }
    } else {
        null
    };
    TITLE_MENU_RESOURCE_ACQUIRE_LAST_RET.store(ret, Ordering::SeqCst);

    if is_title_logo {
        let logo_hit = TITLE_MENU_RESOURCE_ACQUIRE_LOGO_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        let mut name = [0u8; 128];
        let name_len = unsafe { copy_wide_ascii_preview(filename_ptr, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: AcquireMenuResource title-logo hit={logo_hit} total={hit} this=0x{this:x} load_params=0x{load_params:x} filename_ptr=0x{filename_ptr:x} filename='{name}' param3={param3} ret=0x{ret:x} caller_rva=0x{caller_rva:x}; observe-only"
        ));
    } else if hit <= 24 {
        let mut name = [0u8; 96];
        let name_len = unsafe { copy_wide_ascii_preview(filename_ptr, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: AcquireMenuResource sample total={hit} filename='{name}' ret=0x{ret:x} caller_rva=0x{caller_rva:x}"
        ));
    }
    ret
}

unsafe fn construct_title_scaleform_memory_file(
    base: usize,
    url: usize,
    bytes: &[u8],
) -> Option<usize> {
    if bytes.is_empty() || bytes.len() > u32::MAX as usize {
        return None;
    }
    let memory_global = unsafe { safe_read_usize(base + SCALEFORM_MEMORY_GLOBAL_RVA) }?;
    let memory_vtable = unsafe { safe_read_usize(memory_global) }?;
    let alloc_fn = unsafe { safe_read_usize(memory_vtable + 0x50) }?;
    if alloc_fn == 0 || alloc_fn == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let alloc: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(alloc_fn) };
    let file = unsafe { alloc(memory_global, SCALEFORM_MEMORY_FILE_SIZE, 0) };
    if file == 0 || file == TITLE_OWNER_SCAN_START_ADDRESS {
        return None;
    }
    let dlstring_copy: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(base + SCALEFORM_DLSTRING_CHAR_COPY_RVA) };
    unsafe {
        core::ptr::write(file as *mut usize, base + SCALEFORM_MEMORY_FILE_VTABLE_RVA);
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_REFCOUNT_OFFSET) as *mut u32,
            1,
        );
        dlstring_copy(file + SCALEFORM_MEMORY_FILE_NAME_OFFSET, url);
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) as *mut usize,
            bytes.as_ptr() as usize,
        );
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) as *mut u32,
            bytes.len() as u32,
        );
        core::ptr::write((file + SCALEFORM_MEMORY_FILE_CURSOR_OFFSET) as *mut u32, 0);
        core::ptr::write((file + SCALEFORM_MEMORY_FILE_VALID_OFFSET) as *mut u8, 1);
    }
    Some(file)
}

pub(crate) unsafe extern "system" fn title_scaleform_file_open_observer_hook(
    loader: usize,
    url: usize,
    flags: u32,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let hit = TITLE_SCALEFORM_FILE_OPEN_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    let caller_rva = trace_first_game_caller_rva();
    TITLE_SCALEFORM_FILE_OPEN_LAST_LOADER.store(loader, Ordering::SeqCst);
    TITLE_SCALEFORM_FILE_OPEN_LAST_URL_PTR.store(url, Ordering::SeqCst);
    TITLE_SCALEFORM_FILE_OPEN_LAST_FLAGS.store(flags as usize, Ordering::SeqCst);
    TITLE_SCALEFORM_FILE_OPEN_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    let is_title_logo = unsafe { bounded_ascii_contains(url, b"05_001_title_logo") }
        || unsafe { bounded_ascii_contains(url, b"05_001_title") };
    let is_title_05_000 = unsafe { bounded_ascii_contains(url, b"05_000_title") };

    let base = game_module_base().unwrap_or(null);
    let mut memory_replacement = false;
    let mut memory_label = "";
    let memory_bytes = if is_title_logo {
        memory_label = "05_001_title_logo";
        TITLE_SCALEFORM_MEMORY_GFX.get().map(Vec::as_slice)
    } else if is_title_05_000 {
        memory_label = "05_000_title";
        TITLE_SCALEFORM_05_000_MEMORY_GFX.get().map(Vec::as_slice)
    } else {
        None
    };
    let orig = TITLE_SCALEFORM_FILE_OPEN_ORIG.load(Ordering::SeqCst);
    let ret = if base != null {
        if let Some(bytes) = memory_bytes {
            match unsafe { construct_title_scaleform_memory_file(base, url, bytes) } {
                Some(file) => {
                    memory_replacement = true;
                    TITLE_SCALEFORM_MEMORY_GFX_REPLACEMENTS.fetch_add(1, Ordering::SeqCst);
                    TITLE_SCALEFORM_MEMORY_GFX_LAST_FILE.store(file, Ordering::SeqCst);
                    file
                }
                None => {
                    TITLE_SCALEFORM_MEMORY_GFX_FAILURES.fetch_add(1, Ordering::SeqCst);
                    if orig != null && orig != HOOK_ORIGINAL_UNSET {
                        let f: unsafe extern "system" fn(usize, usize, u32) -> usize =
                            unsafe { std::mem::transmute(orig) };
                        unsafe { f(loader, url, flags) }
                    } else {
                        null
                    }
                }
            }
        } else if orig != null && orig != HOOK_ORIGINAL_UNSET {
            let f: unsafe extern "system" fn(usize, usize, u32) -> usize =
                unsafe { std::mem::transmute(orig) };
            unsafe { f(loader, url, flags) }
        } else {
            null
        }
    } else if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, usize, u32) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(loader, url, flags) }
    } else {
        null
    };
    let ret_vtable = if ret != null && ret != HOOK_ORIGINAL_UNSET {
        unsafe { safe_read_usize(ret) }.unwrap_or(null)
    } else {
        null
    };
    TITLE_SCALEFORM_FILE_OPEN_LAST_RET.store(ret, Ordering::SeqCst);
    TITLE_SCALEFORM_FILE_OPEN_LAST_RET_VTABLE.store(ret_vtable, Ordering::SeqCst);

    if is_title_logo || is_title_05_000 {
        let logo_hit = if is_title_logo {
            TITLE_SCALEFORM_FILE_OPEN_LOGO_HITS.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            0
        };
        let mut name = [0u8; 128];
        let name_len = unsafe { copy_ascii_preview(url, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: Scaleform file-open title-memory label={memory_label} logo_hit={logo_hit} total={hit} loader=0x{loader:x} url=0x{url:x} '{name}' flags=0x{flags:x} ret=0x{ret:x} ret_vtable=0x{ret_vtable:x} caller_rva=0x{caller_rva:x} memory_replacement={memory_replacement} total_memory_bytes={}",
            TITLE_SCALEFORM_MEMORY_GFX_BYTES.load(Ordering::SeqCst)
        ));
    } else if hit <= 24 {
        let mut name = [0u8; 96];
        let name_len = unsafe { copy_ascii_preview(url, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: Scaleform file-open sample total={hit} url='{name}' flags=0x{flags:x} ret=0x{ret:x} ret_vtable=0x{ret_vtable:x} caller_rva=0x{caller_rva:x}"
        ));
    }
    ret
}

pub(crate) unsafe extern "system" fn title_scaleform_resource_ctor_observer_hook(
    out_resource: usize,
    loader_data: usize,
    file_type: u32,
    url: usize,
    file_obj: usize,
    external_flag: u8,
    heap_arg: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let hit = TITLE_SCALEFORM_RESOURCE_CTOR_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    let caller_rva = trace_first_game_caller_rva();
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_OUT.store(out_resource, Ordering::SeqCst);
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_URL_PTR.store(url, Ordering::SeqCst);
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_FILE.store(file_obj, Ordering::SeqCst);
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    let is_title_logo = unsafe { bounded_ascii_contains(url, b"05_001_title_logo") }
        || unsafe { bounded_ascii_contains(url, b"05_001_title") };

    let orig = TITLE_SCALEFORM_RESOURCE_CTOR_ORIG.load(Ordering::SeqCst);
    let ret = if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, usize, u32, usize, usize, u8, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe {
            f(
                out_resource,
                loader_data,
                file_type,
                url,
                file_obj,
                external_flag,
                heap_arg,
            )
        }
    } else {
        null
    };
    let movie_data = if ret != null && ret != HOOK_ORIGINAL_UNSET {
        unsafe { safe_read_usize(ret + 0x40) }.unwrap_or(null)
    } else {
        null
    };
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_RET.store(ret, Ordering::SeqCst);
    TITLE_SCALEFORM_RESOURCE_CTOR_LAST_MOVIE_DATA.store(movie_data, Ordering::SeqCst);

    if is_title_logo {
        let logo_hit = TITLE_SCALEFORM_RESOURCE_CTOR_LOGO_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        let mut name = [0u8; 128];
        let name_len = unsafe { copy_ascii_preview(url, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: Scaleform resource-ctor title-logo hit={logo_hit} total={hit} out=0x{out_resource:x} url=0x{url:x} '{name}' file=0x{file_obj:x} file_type={file_type} external_flag={external_flag} ret=0x{ret:x} movie_data=0x{movie_data:x} caller_rva=0x{caller_rva:x}; observe-only"
        ));
    } else if hit <= 24 {
        let mut name = [0u8; 96];
        let name_len = unsafe { copy_ascii_preview(url, &mut name) };
        let name = core::str::from_utf8(&name[..name_len]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-resource-observer: Scaleform resource-ctor sample total={hit} url='{name}' file=0x{file_obj:x} file_type={file_type} ret=0x{ret:x} movie_data=0x{movie_data:x} caller_rva=0x{caller_rva:x}"
        ));
    }
    ret
}

pub(crate) fn install_title_menu_resource_acquire_observer_hook() {
    if TITLE_MENU_RESOURCE_ACQUIRE_INSTALLED.load(Ordering::SeqCst) != 0
        && TITLE_SCALEFORM_FILE_OPEN_INSTALLED.load(Ordering::SeqCst) != 0
        && TITLE_SCALEFORM_RESOURCE_CTOR_INSTALLED.load(Ordering::SeqCst) != 0
    {
        return;
    }
    load_title_scaleform_memory_gfx();
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-resource-observer: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(TITLE_MENU_RESOURCE_ACQUIRE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-resource-observer: failed to resolve AcquireMenuResource rva 0x{TITLE_MENU_RESOURCE_ACQUIRE_RVA:x}"
        ));
        return;
    };
    let Ok(file_open_addr) = game_rva(TITLE_SCALEFORM_FILE_OPEN_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-resource-observer: failed to resolve Scaleform file-open rva 0x{TITLE_SCALEFORM_FILE_OPEN_RVA:x}"
        ));
        return;
    };
    let Ok(resource_ctor_addr) = game_rva(TITLE_SCALEFORM_RESOURCE_CTOR_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-resource-observer: failed to resolve Scaleform resource-ctor rva 0x{TITLE_SCALEFORM_RESOURCE_CTOR_RVA:x}"
        ));
        return;
    };
    let mut ok = true;
    if TITLE_MENU_RESOURCE_ACQUIRE_INSTALLED.load(Ordering::SeqCst) == 0 {
        match unsafe {
            MhHook::new(
                addr as *mut c_void,
                title_menu_resource_acquire_observer_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                TITLE_MENU_RESOURCE_ACQUIRE_ORIG
                    .store(hook.trampoline() as usize, Ordering::SeqCst);
                ok &= unsafe { hook.queue_enable() }.is_ok();
                std::mem::forget(hook);
            }
            Err(status) => {
                append_autoload_debug(format_args!(
                    "title-resource-observer: AcquireMenuResource MhHook::new failed: {status:?}"
                ));
                ok = false;
            }
        }
    }
    if TITLE_SCALEFORM_FILE_OPEN_INSTALLED.load(Ordering::SeqCst) == 0 {
        match unsafe {
            MhHook::new(
                file_open_addr as *mut c_void,
                title_scaleform_file_open_observer_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                TITLE_SCALEFORM_FILE_OPEN_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                ok &= unsafe { hook.queue_enable() }.is_ok();
                std::mem::forget(hook);
            }
            Err(status) => {
                append_autoload_debug(format_args!(
                    "title-resource-observer: Scaleform file-open MhHook::new failed: {status:?}"
                ));
                ok = false;
            }
        }
    }
    if TITLE_SCALEFORM_RESOURCE_CTOR_INSTALLED.load(Ordering::SeqCst) == 0 {
        match unsafe {
            MhHook::new(
                resource_ctor_addr as *mut c_void,
                title_scaleform_resource_ctor_observer_hook as *mut c_void,
            )
        } {
            Ok(hook) => {
                TITLE_SCALEFORM_RESOURCE_CTOR_ORIG
                    .store(hook.trampoline() as usize, Ordering::SeqCst);
                ok &= unsafe { hook.queue_enable() }.is_ok();
                std::mem::forget(hook);
            }
            Err(status) => {
                append_autoload_debug(format_args!(
                    "title-resource-observer: Scaleform resource-ctor MhHook::new failed: {status:?}"
                ));
                ok = false;
            }
        }
    }
    if !ok {
        return;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            TITLE_MENU_RESOURCE_ACQUIRE_INSTALLED.store(1, Ordering::SeqCst);
            TITLE_SCALEFORM_FILE_OPEN_INSTALLED.store(1, Ordering::SeqCst);
            TITLE_SCALEFORM_RESOURCE_CTOR_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "title-resource-observer: hooked AcquireMenuResource 0x{addr:x}, Scaleform file-open 0x{file_open_addr:x}, resource-ctor 0x{resource_ctor_addr:x}; observe-only"
            ));
        }
        status => append_autoload_debug(format_args!(
            "title-resource-observer: MH_ApplyQueued failed: {status:?}"
        )),
    }
}

pub(crate) unsafe extern "system" fn title_scaleform_bind_observer_hook(owner: usize, pair: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let symbol_ptr = unsafe { read_native_dlstring_ascii_ptr(pair) };
    let target_ptr = unsafe { read_native_dlstring_ascii_ptr(pair + 0x30) };
    let hit = TITLE_SCALEFORM_BIND_OBSERVER_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    TITLE_SCALEFORM_BIND_OBSERVER_LAST_OWNER.store(owner, Ordering::SeqCst);
    TITLE_SCALEFORM_BIND_OBSERVER_LAST_PAIR.store(pair, Ordering::SeqCst);
    TITLE_SCALEFORM_BIND_OBSERVER_LAST_SYMBOL_PTR.store(symbol_ptr, Ordering::SeqCst);
    TITLE_SCALEFORM_BIND_OBSERVER_LAST_TARGET_PTR.store(target_ptr, Ordering::SeqCst);
    let interesting = unsafe { bounded_ascii_contains(symbol_ptr, b"menu_") }
        || unsafe { bounded_ascii_contains(target_ptr, b"systex") }
        || unsafe { bounded_ascii_contains(symbol_ptr, b"title") }
        || unsafe { bounded_ascii_contains(symbol_ptr, b"profile") };
    if unsafe { bounded_ascii_contains(target_ptr, b"systex") } {
        TITLE_SCALEFORM_BIND_OBSERVER_SYSTEX_HITS.fetch_add(1, Ordering::SeqCst);
    }
    let mut rewritten_visible_profile_surface = false;
    if unsafe { bounded_ascii_contains(symbol_ptr, b"menu_dummyprofileface_01") }
        && unsafe { bounded_ascii_contains(target_ptr, b"systex_menu_profile00") }
    {
        if let Some(new_symbol_ptr) =
            unsafe { rewrite_native_dlstring_ascii(pair, TITLE_PROFILE_VISIBLE_SURFACE_SYMBOL) }
        {
            rewritten_visible_profile_surface = true;
            TITLE_PROFILE_VISIBLE_SURFACE_BIND_REWRITES.fetch_add(1, Ordering::SeqCst);
            TITLE_PROFILE_VISIBLE_SURFACE_BIND_LAST_OWNER.store(owner, Ordering::SeqCst);
            TITLE_PROFILE_VISIBLE_SURFACE_BIND_LAST_PAIR.store(pair, Ordering::SeqCst);
            TITLE_PROFILE_VISIBLE_SURFACE_BIND_LAST_SYMBOL_PTR
                .store(new_symbol_ptr, Ordering::SeqCst);
        }
        // er-tpf Tier-4 DRAW redirect (ONE-SHOT, fail-closed): once our in-memory cover texture is
        // registered in GLOBAL_TexRepository (ER_TPF_COVER_REGISTERED), repoint THIS visible profile
        // bind's TARGET DLString (pair+0x30) from `systex_menu_profile00` to our unique key. The native
        // bind then resolves our key; the Scaleform repo misses and bridges to GLOBAL_TexRepository by
        // name (FUN_140d66220 -> CS::TexRepositoryImp::GetResCap) -> our magenta cover wraps + binds to
        // the visible surface above PRESS ANY BUTTON. Until registered, the target is left native (the
        // real portrait draws) -- no harm; the one-shot is only consumed on a committed rewrite.
        if ER_TPF_COVER_REGISTERED.load(Ordering::SeqCst) != 0
            && ER_TPF_COVER_TARGET_REWRITE_FIRED.swap(1, Ordering::SeqCst) == 0
        {
            let rewrote =
                unsafe { rewrite_native_dlstring_ascii(pair + 0x30, ER_TPF_COVER_SYSTEX_KEY) }
                    .is_some();
            if rewrote {
                ER_TPF_COVER_BOUND.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "er-tpf-cover: REDIRECTED visible profile bind target -> '{ER_TPF_COVER_SYSTEX_KEY}' owner=0x{owner:x} pair=0x{pair:x} -- in-memory cover now resolves on the visible surface (rescap=0x{:x})",
                    ER_TPF_COVER_LAST_RESCAP.load(Ordering::SeqCst)
                ));
            } else {
                // capacity too small / unreadable -> un-consume the one-shot so a later bind can retry.
                ER_TPF_COVER_TARGET_REWRITE_FIRED.store(0, Ordering::SeqCst);
                ER_TPF_COVER_FAILURES.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    if interesting && hit <= 128 {
        let mut sym = [0u8; 96];
        let mut tgt = [0u8; 96];
        let sn = unsafe { copy_ascii_preview(symbol_ptr, &mut sym) };
        let tn = unsafe { copy_ascii_preview(target_ptr, &mut tgt) };
        let sym = core::str::from_utf8(&sym[..sn]).unwrap_or("?");
        let tgt = core::str::from_utf8(&tgt[..tn]).unwrap_or("?");
        append_autoload_debug(format_args!(
            "title-cover-part-b: observed native Scaleform bind owner=0x{owner:x} pair=0x{pair:x} symbol='{sym}' target='{tgt}' rewritten_visible_profile_surface={rewritten_visible_profile_surface} hit={hit}"
        ));
    }
    let orig = TITLE_SCALEFORM_BIND_OBSERVER_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, usize) = unsafe { std::mem::transmute(orig) };
        unsafe { f(owner, pair) };
    }
}

pub(crate) unsafe extern "system" fn title_flow_context_record_regulation_fix_hook(tfc: usize) {
    let base = game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let before = if tfc > OWNER_CTX_MIN_PLAUSIBLE_PTR && tfc < OWNER_CTX_MAX_PLAUSIBLE_PTR {
        unsafe { safe_read_i32(tfc + TFC_REGULATION_VERSION_148_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    let orig = TITLE_FLOW_CONTEXT_RECORD_REGULATION_ORIG.load(Ordering::SeqCst);
    if orig != TITLE_OWNER_SCAN_START_ADDRESS && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(orig) };
        unsafe { f(tfc) };
    }
    let after_orig = if tfc > OWNER_CTX_MIN_PLAUSIBLE_PTR && tfc < OWNER_CTX_MAX_PLAUSIBLE_PTR {
        unsafe { safe_read_i32(tfc + TFC_REGULATION_VERSION_148_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    let reg_manager =
        unsafe { safe_read_usize(base + GLOBAL_CS_REGULATION_MANAGER_RVA) }.unwrap_or(0);
    let manager44 = if reg_manager > OWNER_CTX_MIN_PLAUSIBLE_PTR
        && reg_manager < OWNER_CTX_MAX_PLAUSIBLE_PTR
    {
        unsafe { safe_read_i32(reg_manager + REGULATION_MANAGER_VERSION_44_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    if tfc > OWNER_CTX_MIN_PLAUSIBLE_PTR
        && tfc < OWNER_CTX_MAX_PLAUSIBLE_PTR
        && manager44 > 0
        && after_orig < manager44
    {
        unsafe {
            ((tfc + TFC_REGULATION_VERSION_148_OFFSET) as *mut i32).write_volatile(manager44)
        };
        TITLE_FLOW_CONTEXT_RECORD_REGULATION_FIXUPS
            .fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    let after_fix = if tfc > OWNER_CTX_MIN_PLAUSIBLE_PTR && tfc < OWNER_CTX_MAX_PLAUSIBLE_PTR {
        unsafe { safe_read_i32(tfc + TFC_REGULATION_VERSION_148_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    append_autoload_debug(format_args!(
        "title-flow-context-record-fix: tfc=0x{tfc:x} before={before} after_orig={after_orig} after_fix={after_fix} manager44={manager44}"
    ));
}

pub(crate) fn install_title_flow_context_record_regulation_fix_hook() {
    if TITLE_FLOW_CONTEXT_RECORD_REGULATION_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-flow-context-record-fix: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(TITLE_FLOW_CONTEXT_RECORD_REGULATION_VERSION_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-flow-context-record-fix: failed to resolve record rva 0x{TITLE_FLOW_CONTEXT_RECORD_REGULATION_VERSION_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            title_flow_context_record_regulation_fix_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_FLOW_CONTEXT_RECORD_REGULATION_ORIG
                .store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-flow-context-record-fix: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    TITLE_FLOW_CONTEXT_RECORD_REGULATION_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-flow-context-record-fix: hooked native record helper 0x{addr:x}"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-flow-context-record-fix: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-flow-context-record-fix: MhHook::new failed: {status:?}"
        )),
    }
}

pub(crate) fn install_title_scaleform_bind_observer_hook() {
    if TITLE_SCALEFORM_BIND_OBSERVER_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-b: bind observer MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(TITLE_SCALEFORM_BIND_OBSERVER_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-cover-part-b: failed to resolve Scaleform bind observer rva 0x{TITLE_SCALEFORM_BIND_OBSERVER_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            title_scaleform_bind_observer_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_SCALEFORM_BIND_OBSERVER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-b: queue_enable bind observer failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    TITLE_SCALEFORM_BIND_OBSERVER_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-cover-part-b: hooked passive Scaleform bind observer 0x{addr:x}; no product bind calls added"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-b: bind observer MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-b: MhHook::new bind observer failed: {status:?}"
        )),
    }
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
    TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESSED_WINDOWS
        .fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_WINDOW.store(window, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_FLAGS_BEFORE
        .store(flags_before as usize, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_FLAGS_AFTER.store(flags_after as usize, Ordering::SeqCst);
    TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-cover-part-a: render-suppressed preserved native {TITLE_NATIVE_MENU_VISUAL_NAME} window=0x{window:x} menu_id={menu_id} flags 0x{flags_before:02x}->0x{flags_after:02x} via CSMenuMan+0x90 caller_rva=0x{caller_rva:x}"
    ));
}

unsafe fn title_child_name_matches(name_ptr: usize) -> bool {
    if name_ptr == 0 || name_ptr == TITLE_OWNER_SCAN_START_ADDRESS {
        return false;
    }
    let Ok(name) = (unsafe { CStr::from_ptr(name_ptr as *const i8).to_str() }) else {
        return false;
    };
    matches!(
        name,
        "PressStart"
            | "StaticSystemText_101000"
            | "PRESS BUTTON"
            | "CopyrightText"
            | "ProgressInfo"
            | "Install_ProgressInfo"
            | "StaticSystemText_100100"
            | "Info"
    )
}

unsafe fn title_profile_list_container_matches(name_ptr: usize) -> bool {
    if name_ptr == 0 || name_ptr == TITLE_OWNER_SCAN_START_ADDRESS {
        return false;
    }
    let Ok(name) = (unsafe { CStr::from_ptr(name_ptr as *const i8).to_str() }) else {
        return false;
    };
    name == "ProfileList/ItemList/ItemList/ItemList"
}

fn record_title_text_gfx_value(value: usize) {
    if value == 0 || value == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    for slot in TITLE_TEXT_GFX_VALUES.iter() {
        if slot.load(Ordering::SeqCst) == value {
            return;
        }
    }
    for slot in TITLE_TEXT_GFX_VALUES.iter() {
        if slot
            .compare_exchange(
                TITLE_OWNER_SCAN_START_ADDRESS,
                value,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            TITLE_TEXT_GFX_VALUE_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            return;
        }
    }
}

pub(crate) unsafe extern "system" fn title_scene_obj_proxy_named_child_bind_hook(
    parent: usize,
    out_proxy: usize,
    name_ptr: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_ORIG.load(Ordering::SeqCst);
    if orig == null || orig == HOOK_ORIGINAL_UNSET {
        return out_proxy;
    }
    let f: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    let ret = unsafe { f(parent, out_proxy, name_ptr) };
    if unsafe { title_profile_list_container_matches(name_ptr) } {
        TITLE_PROFILE_FACE_BIND_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        TITLE_PROFILE_FACE_LAST_PROXY.store(out_proxy, Ordering::SeqCst);
        TITLE_PROFILE_FACE_LAST_VALUE.store(out_proxy, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "title-cover-part-b: recorded ProfileSelect container receiver=out_proxy name='ProfileList/ItemList/ItemList/ItemList' proxy=0x{out_proxy:x} parent=0x{parent:x} ret=0x{ret:x}"
        ));
    }
    if unsafe { title_child_name_matches(name_ptr) } {
        let context = unsafe { safe_read_usize(out_proxy + SCENE_OBJ_PROXY_CONTEXT_20_OFFSET) }
            .unwrap_or(null);
        let value = out_proxy + 0x18;
        TITLE_PRESS_START_BIND_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        TITLE_PRESS_START_BIND_LAST_PARENT.store(parent, Ordering::SeqCst);
        TITLE_PRESS_START_BIND_LAST_OUT.store(out_proxy, Ordering::SeqCst);
        TITLE_PRESS_START_BIND_LAST_NAME.store(name_ptr, Ordering::SeqCst);
        TITLE_PRESS_START_BIND_LAST_CONTEXT.store(context, Ordering::SeqCst);
        TITLE_PRESS_START_GFX_VALUE.store(value, Ordering::SeqCst);
        record_title_text_gfx_value(value);
        let base = game_module_base().unwrap_or(null);
        if base != null {
            let set_visible: unsafe extern "system" fn(usize, u8) =
                unsafe { std::mem::transmute(base + TITLE_PRESS_START_SET_VISIBLE_RVA) };
            unsafe { set_visible(out_proxy, 0) };
            let calls = TITLE_PRESS_START_BIND_HIDE_CALLS
                .fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
                + OWN_STEPPER_CALL_INC;
            if calls <= 8 {
                let name = unsafe { CStr::from_ptr(name_ptr as *const i8) }.to_string_lossy();
                append_autoload_debug(format_args!(
                    "title-cover-part-a: named-child bind hid {name} out_proxy=0x{out_proxy:x} parent=0x{parent:x} context=0x{context:x} value=0x{value:x} calls={calls}"
                ));
            }
        }
    }
    ret
}

pub(crate) fn install_title_scene_obj_proxy_named_child_bind_hook() {
    if TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-a: named-child bind MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-cover-part-a: failed to resolve named-child bind rva 0x{TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            title_scene_obj_proxy_named_child_bind_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_ORIG
                .store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-a: queue_enable named-child bind failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-cover-part-a: hooked named-child SceneObjProxy binder 0x{addr:x}; PressStart/StaticSystemText will be hidden at bind time"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-a: named-child bind MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-a: MhHook::new named-child bind failed: {status:?}"
        )),
    }
}

pub(crate) unsafe extern "system" fn title_gfx_value_set_visible_hook(
    value: usize,
    visible: u8,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let orig = TITLE_GFX_VALUE_SET_VISIBLE_ORIG.load(Ordering::SeqCst);
    if orig == null || orig == HOOK_ORIGINAL_UNSET {
        return value;
    }
    let single_target = TITLE_PRESS_START_GFX_VALUE.load(Ordering::SeqCst);
    let in_text_hide_set = TITLE_TEXT_GFX_VALUES.iter().any(|slot| {
        let target = slot.load(Ordering::SeqCst);
        target != null && target != 0 && value == target
    });
    let forced_visible = if (single_target != null && single_target != 0 && value == single_target)
        || in_text_hide_set
    {
        TITLE_PRESS_START_GFX_FORCE_FALSE_CALLS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_VALUE.store(value, Ordering::SeqCst);
        TITLE_PRESS_START_GFX_FORCE_FALSE_LAST_REQUESTED.store(visible as usize, Ordering::SeqCst);
        0
    } else {
        visible
    };
    let f: unsafe extern "system" fn(usize, u8) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { f(value, forced_visible) }
}

pub(crate) fn install_title_gfx_value_set_visible_hook() {
    if TITLE_GFX_VALUE_SET_VISIBLE_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-a: GFx visibility MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(TITLE_GFX_VALUE_SET_VISIBLE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-cover-part-a: failed to resolve GFx visibility setter rva 0x{TITLE_GFX_VALUE_SET_VISIBLE_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            title_gfx_value_set_visible_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_GFX_VALUE_SET_VISIBLE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-a: queue_enable GFx visibility setter failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    TITLE_GFX_VALUE_SET_VISIBLE_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-cover-part-a: hooked GFx visibility setter 0x{addr:x}; only PressStart value will be forced hidden"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-a: GFx visibility MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-a: MhHook::new GFx visibility setter failed: {status:?}"
        )),
    }
}

pub(crate) fn install_title_custom_cover_run_hook() {
    if TITLE_CUSTOM_COVER_RUN_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-b: MenuWindowJob::Run MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(run_addr) = game_rva(MENU_WINDOW_JOB_RUN_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-cover-part-b: failed to resolve MenuWindowJob::Run rva 0x{MENU_WINDOW_JOB_RUN_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            run_addr as *mut c_void,
            title_custom_cover_menu_window_run_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_CUSTOM_COVER_RUN_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-b: queue_enable MenuWindowJob::Run failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    TITLE_CUSTOM_COVER_RUN_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-cover-part-b: hooked MenuWindowJob::Run 0x{run_addr:x}; ProfileSelect cover will run alongside preserved native title job"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-b: MenuWindowJob::Run MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-b: MhHook::new MenuWindowJob::Run failed: {status:?}"
        )),
    }
}

pub(crate) fn install_title_logo_force_hidden_hooks() {
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-a: logo-force MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    if TITLE_LOGO_SET_VISIBLE_INSTALLED.load(Ordering::SeqCst) == 0 {
        match game_rva(TITLE_LOGO_BACK_VIEW_PARTS_SET_VISIBLE_RVA as u32) {
            Ok(addr) => match unsafe {
                MhHook::new(
                    addr as *mut c_void,
                    title_logo_set_visible_force_hidden_hook as *mut c_void,
                )
            } {
                Ok(hook) => {
                    TITLE_LOGO_SET_VISIBLE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                    if let Err(status) = unsafe { hook.queue_enable() } {
                        append_autoload_debug(format_args!(
                            "title-cover-part-a: queue_enable logo SetVisible failed: {status:?}"
                        ));
                    } else if unsafe { MH_ApplyQueued() } == MH_STATUS::MH_OK {
                        std::mem::forget(hook);
                        TITLE_LOGO_SET_VISIBLE_INSTALLED.store(1, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "title-cover-part-a: hooked {TITLE_LOGO_BACK_VIEW_PARTS_NAME} SetVisible 0x{addr:x}; forcing visible=false"
                        ));
                    }
                }
                Err(status) => append_autoload_debug(format_args!(
                    "title-cover-part-a: MhHook::new logo SetVisible failed: {status:?}"
                )),
            },
            Err(_) => append_autoload_debug(format_args!(
                "title-cover-part-a: failed to resolve logo SetVisible rva 0x{TITLE_LOGO_BACK_VIEW_PARTS_SET_VISIBLE_RVA:x}"
            )),
        }
    }
    if TITLE_LOGO_CTOR_INSTALLED.load(Ordering::SeqCst) == 0 {
        match game_rva(TITLE_LOGO_BACK_VIEW_PARTS_CTOR_RVA as u32) {
            Ok(addr) => match unsafe {
                MhHook::new(
                    addr as *mut c_void,
                    title_logo_ctor_force_hidden_hook as *mut c_void,
                )
            } {
                Ok(hook) => {
                    TITLE_LOGO_CTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                    if let Err(status) = unsafe { hook.queue_enable() } {
                        append_autoload_debug(format_args!(
                            "title-cover-part-a: queue_enable logo ctor failed: {status:?}"
                        ));
                    } else if unsafe { MH_ApplyQueued() } == MH_STATUS::MH_OK {
                        std::mem::forget(hook);
                        TITLE_LOGO_CTOR_INSTALLED.store(1, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "title-cover-part-a: hooked {TITLE_LOGO_BACK_VIEW_PARTS_NAME} ctor 0x{addr:x}; hiding immediately after construction"
                        ));
                    }
                }
                Err(status) => append_autoload_debug(format_args!(
                    "title-cover-part-a: MhHook::new logo ctor failed: {status:?}"
                )),
            },
            Err(_) => append_autoload_debug(format_args!(
                "title-cover-part-a: failed to resolve logo ctor rva 0x{TITLE_LOGO_BACK_VIEW_PARTS_CTOR_RVA:x}"
            )),
        }
    }
}

pub(crate) fn install_title_logo_start_login_hide_hook() {
    if TITLE_TOP_START_LOGIN_HIDE_INSTALLED.load(Ordering::SeqCst)
        != TITLE_TOP_START_LOGIN_HIDE_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-a: start-login MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(start_login_addr) = game_rva(TITLE_TOP_START_LOGIN_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-cover-part-a: failed to resolve TitleTopDialog start-login rva 0x{TITLE_TOP_START_LOGIN_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            start_login_addr as *mut c_void,
            title_top_start_login_hide_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_TOP_START_LOGIN_HIDE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-a: queue_enable start-login hide failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    TITLE_TOP_START_LOGIN_HIDE_INSTALLED
                        .store(TITLE_TOP_START_LOGIN_HIDE_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-cover-part-a: hooked TitleTopDialog start-login 0x{start_login_addr:x}; will hide {TITLE_LOGO_BACK_VIEW_PARTS_NAME}/{TITLE_LOGO_RESOURCE_NAME} after native SetVisible(1)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-a: start-login MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-a: MhHook::new start-login hide failed: {status:?}"
        )),
    }
}

/// Install the Part-A title visual suppression hook once. It must run at process attach before
/// STEP_BeginTitle; installing from the recurring game task can be too late for the first title build.
pub(crate) fn install_title_pab_information_visual_hook() {
    if TITLE_PAB_INFORMATION_VISUAL_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-cover-part-a: PAB/TitleInformation MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(TITLE_NATIVE_MENU_VISUAL_TITLE_INFORMATION_RVA as u32) else {
        append_autoload_debug(format_args!(
            "title-cover-part-a: failed to resolve PAB/TitleInformation wrapper rva 0x{TITLE_NATIVE_MENU_VISUAL_TITLE_INFORMATION_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            title_pab_information_visual_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            TITLE_PAB_INFORMATION_VISUAL_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "title-cover-part-a: queue_enable PAB/TitleInformation wrapper failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    TITLE_PAB_INFORMATION_VISUAL_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-cover-part-a: hooked PAB/TitleInformation wrapper 0x{addr:x}; native {TITLE_PAB_INFORMATION_VISUAL_NAME} preserved and covered"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "title-cover-part-a: PAB/TitleInformation MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "title-cover-part-a: MhHook::new PAB/TitleInformation wrapper failed: {status:?}"
        )),
    }
}

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
