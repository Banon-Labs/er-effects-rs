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
use crate::mh::{
    MH_ApplyQueued, MH_Initialize, MH_QueueDisableHook, MH_QueueEnableHook, MH_STATUS, MhHook,
};
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
/// Runtime-derived stripped 05_000_title movie (er-effects-rs-h7x): computed once at first
/// title file-open from the native MemoryFile's vanilla payload, then reused for every later
/// title visit. Lives for the process lifetime so the swapped-in data pointer stays valid for
/// as long as any native file object references it.
static TITLE_05_000_RUNTIME_STRIPPED: OnceLock<Vec<u8>> = OnceLock::new();

fn load_memory_gfx_from_env(var: &str, slot: &OnceLock<Vec<u8>>, label: &str) {
    let Ok(path) = std::env::var(var) else {
        return;
    };
    let trimmed = path.trim();
    if trimmed.is_empty() || slot.get().is_some() {
        return;
    }
    let embedded_bytes = if trimmed.eq_ignore_ascii_case("embedded:minimal-magenta") {
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
    // `vanilla`/`off`/`0` force the native on-disk 05_000 movie (diagnostic escape while autoload
    // stays on); checked before the env loader so the literal is never treated as a file path.
    // `embedded:title-05-000-suppressed` is the legacy selector for the product strip asset: it
    // now arms the same runtime derivation as the product default (the asset is no longer
    // embedded; it is derived from the game's own vanilla bytes at file-open).
    let env_05_000 = std::env::var("ER_EFFECTS_TITLE_05_000_MEMORY_GFX")
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    match env_05_000.as_str() {
        "vanilla" | "off" | "0" => {
            append_autoload_debug(format_args!(
                "title-resource-observer: 05_000_title strip forced vanilla via ER_EFFECTS_TITLE_05_000_MEMORY_GFX"
            ));
            return;
        }
        "embedded:title-05-000-suppressed" => {
            TITLE_05_000_RUNTIME_STRIP_ARMED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "title-resource-observer: 05_000_title runtime strip armed via legacy embedded selector (derived at file-open, {} edits)",
                er_gfx::title_05_000::TITLE_05_000_STRIP_EDITS.len()
            ));
            return;
        }
        _ => {}
    }
    load_memory_gfx_from_env(
        "ER_EFFECTS_TITLE_05_000_MEMORY_GFX",
        &TITLE_SCALEFORM_05_000_MEMORY_GFX,
        "05_000_title GFX",
    );
    // Product default (er-effects-rs-dl0/h7x): no env override -> arm the RUNTIME strip whenever
    // the product autoload owns the title flow. No embedded movie: the file-open hook derives the
    // stripped 05_000_title from the native MemoryFile's own vanilla payload via er-gfx.
    if TITLE_SCALEFORM_05_000_MEMORY_GFX.get().is_some() || !title_05_000_strip_default_enabled() {
        return;
    }
    TITLE_05_000_RUNTIME_STRIP_ARMED.store(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "title-resource-observer: product-default 05_000_title runtime strip armed ({} content-addressed edits, expect {} -> {} bytes on known vanilla)",
        er_gfx::title_05_000::TITLE_05_000_STRIP_EDITS.len(),
        er_gfx::title_05_000::VANILLA_LEN,
        er_gfx::title_05_000::STRIPPED_LEN
    ));
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
    // Scope the blanket product msgbox suppression to the SENSITIVE windows only (er-effects-rs-qwj):
    // boot autoload (pre-world -- connection-error / EULA / warning popups) and an ACTIVE
    // System->Quit->Load-Profile switch (any stray ProfileSelect load-confirm). Do NOT suppress during
    // free in-world play: the user's own menu confirmations -- notably the Quit Game / Return-to-Desktop
    // "are you sure?" dialog -- are legitimate product UI and MUST render, else those rows silently do
    // nothing because the suppression ate their confirmation MessageBox (observed: Quit Game / Return to
    // Desktop dead on the 2nd quit menu; a msgbox-skip fired ~18ms after the forwarded click). The
    // character-load zero-MessageBox proof is unaffected: boot + switch still suppress, and the quit
    // confirm is not on the character-load path.
    let in_world = IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES;
    let switch_active = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE
        || SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) != 0;
    if product_autoload_enabled() && (!in_world || switch_active) {
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
/// Deterministic clean-title active-save-slot override for the System-Quit->Load-Profile switch.
///
/// The clean-title reload is the game's NATIVE most-recent Continue: the ShowProgressJob save-data
/// delegate (the boot ProfileSummary read) derives+selects the MOST-RECENT save slot and writes it to
/// the active-slot field GameMan+0xac0, and the reload deserializes 0xac0 immediately afterward. On a
/// switch that makes it re-load the ORIGINAL character (proven 2026-07-02: picked slot 4 'Speed Bean'
/// but ac0 re-derived to 5 -> loaded 'Patches'). Repointing ac0 to the picked slot on a per-tick poll
/// LOSES the race -- the derivation and the load happen inside one game-task tick, so the tick-set
/// landed after the load committed. Calling this RIGHT AFTER the delegate (before the load) wins it
/// deterministically. Gated on a torn-down world (local player absent) so it only ever fires at the
/// clean-title reload, never while the old world is live -- where it would misdirect the return-title
/// quit-save to the picked slot. Save-safe: a pure active-slot write, no save-file mutation. See bd
/// system-quit-ac0-fix-insufficient-cleantitle-load-is-native-mostrecent-2026-07-02.
unsafe fn system_quit_repoint_active_slot_at_clean_title(source: &str) {
    if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        < SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
    {
        return;
    }
    let picked = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
    if picked == usize::MAX {
        return;
    }
    let picked = picked as i32;
    if picked < 0 {
        return;
    }
    // CLEAN-title only: an OLD world still up means the return-title quit-save has not run yet, and
    // ac0 selects the slot it writes -- repointing now would corrupt (overwrite) the picked slot.
    if unsafe { PlayerIns::local_player_mut() }.is_ok() {
        return;
    }
    let Ok(base) = game_module_base() else {
        return;
    };
    let gm = game_man_ptr_or_null();
    if gm == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let ac0_before = unsafe { safe_read_i32(gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) }
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    if ac0_before == picked {
        return;
    }
    let set_save_slot: unsafe extern "system" fn(i32) =
        unsafe { std::mem::transmute(base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
    unsafe { set_save_slot(picked) };
    let ac0_after = unsafe { safe_read_i32(gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) }
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    append_autoload_debug(format_args!(
        "system-quit-quickload: [{source}] DETERMINISTIC clean-title active-slot override ac0 {ac0_before}->{ac0_after} via set_save_slot({picked}) -- applied after the native most-recent derivation, before the reload deserialize, so the reload loads the PICKED slot"
    ));
}

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
            let ret = unsafe { call(rcx, rdx, r8, r9) };
            // The delegate above just selected the MOST-RECENT save slot into GameMan+0xac0. On a
            // System-Quit->Load-Profile switch the reload deserializes 0xac0 next, so override it to
            // the PICKED slot here -- after the native derivation, before the load. Deterministic, no
            // tick-race. No-ops off the switch path / while the old world is up (see the helper).
            unsafe { system_quit_repoint_active_slot_at_clean_title("show-progress-delegate") };
            return ret;
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
    // so a torn/rebuilding slot can't bind a bad pointer. Slot = the loaded character (er-effects-rs-j3r).
    let slot = portrait_loaded_slot();
    let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, slot)) }.unwrap_or(0);
    if !valid(r)
        || unsafe { safe_read_usize(r) }.unwrap_or(0)
            != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
    {
        return;
    }
    let off =
        unsafe { safe_read_usize(r + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET) }
            .unwrap_or(0);
    if !valid(off) {
        return;
    }
    let trc =
        unsafe { safe_read_usize(off + TITLE_CUSTOM_COVER_PROFILE_OFFSCREEN_TEX_RESCAP_OFFSET) }
            .unwrap_or(0);
    if !valid(trc) {
        return;
    }
    let bind_gx = unsafe { safe_read_usize(trc + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET) }
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
pub(crate) unsafe fn profile_lookat_realtime_draw_tick(base: usize, task_data: &FD4TaskData) {
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
        // TARGET-SLOT BINDING (frozen-on-prior-character fix, attribution soak 2026-07-03). This
        // draw tick (pump + rasterize + RT->SRV + readback + publish) used portrait_loaded_slot()
        // = ac0, which still names the OLD character until the switch deserialize flips it. In
        // windows where the flip came late, the whole tick bound the old slot's rebuilt (model-
        // less) renderer and published its STALE RT ~92 frames -- a static prior-character head,
        // exactly the user-observed freeze (publish[clean=92, no dominant skip class]); the
        // window-4 tear=39-40 storm was the two producers competing during the flip. Bind to
        // portrait_target_slot() -- the make-before-break source every other portrait site
        // (spare/retarget/display) already uses: selected slot from the confirm press (known
        // BEFORE ac0 flips), falling back to loaded/ac0 when no switch is pending (boot window
        // unchanged). Early-window table[target] is legitimately null (the spare nulled it), so
        // the tick idles on the bridge until the target build lands instead of driving the wrong
        // character.
        let slot = portrait_target_slot();
        // Tag the live portrait CHARACTER incarnation (slot + 1; 0 = unset) for the mask stale-reuse
        // desync semaphore: apply_depth_alpha_key records this on a fresh mask and trips
        // PROFILE_MASK_STALE_REUSE if a later frame reuses a mask computed for a different incarnation.
        crate::experiments::gpu_readback::PROFILE_PORTRAIT_INCARNATION.store(
            if slot >= 0 { slot as usize + 1 } else { 0 },
            Ordering::SeqCst,
        );
        let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, slot)) }.unwrap_or(0);
        // Pump-block attribution (run #7 stall): name the failing gate, don't skip silently.
        if r == 0 || r == null {
            PORTRAIT_PUMP_BLOCK_R.fetch_add(1, Ordering::SeqCst);
        } else if unsafe { safe_read_usize(r) }.unwrap_or(0)
            != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
        {
            PORTRAIT_PUMP_BLOCK_VTABLE.fetch_add(1, Ordering::SeqCst);
        }
        if r != 0
            && r != null
            && unsafe { safe_read_usize(r) }.unwrap_or(0)
                == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
        {
            let off = unsafe {
                safe_read_usize(r + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
            }
            .unwrap_or(0);
            if off == 0 || off == null {
                PORTRAIT_PUMP_BLOCK_OFF.fetch_add(1, Ordering::SeqCst);
            }
            // STABILITY GATE (subsequent-load crash + cascade fix, 2026-07-02, STATIC-RE grounded). Driving
            // the live model render / RT copy / readback while the game's Load Profile menu has multiple
            // character models live (all 10 thumbnails + its teardown churn) dereferenced a FREED render
            // object deep in the GX accessor chain (crash: game FUN_141214c80 -> FUN_141140ce0 read of
            // 0x7ffe00000011) AND read back the wrong character (cascade). Run the whole live-drive block
            // ONLY when the loaded character is the SINGLE live profile model -- i.e. past the menu, in the
            // stable target-only post-Continue window. During churn: skip entirely (leave the artwork up).
            let live_models = unsafe { count_live_profile_models(base) };
            let stable_target_only = off != 0 && off != null && live_models == 1;
            if off != 0 && off != null && !stable_target_only {
                PROFILE_MULTI_MODEL_PUBLISH_SKIPS.fetch_add(1, Ordering::SeqCst);
            }
            if off != 0 && off != null && live_models > 1 {
                PORTRAIT_PUMP_BLOCK_MULTI.fetch_add(1, Ordering::SeqCst);
            }
            // STATE-MACHINE PUMP -- runs even with the model DEAD (run anim-bind6 deadlock fix,
            // 2026-07-03). The update task is the renderer's engine-designed per-frame tick (state
            // machine + anim step + transforms); ResMan runs it continuously in the menu era but
            // under-schedules it post-Continue, and the kick's +0x755 reset->rebuild pipeline only
            // advances on these ticks. Gating the pump on a LIVE model deadlocked run #6: the
            // rebuild needed ticks, the gate needed the rebuild finished (rgba_version=1,
            // publish_skips=241). Pump every frame the renderer is vtable-valid and the table is
            // not in multi-model (menu) churn; the task bodies self-guard on model/X, so ticking
            // any state is engine-normal. Readback/publish/bind keep the stricter gates below.
            //
            // FREEZE-AFTER-CAPTURE RELAXED (bug #1 fix, er-effects-rs-l1x 2026-07-03). The old
            // per-window latch stopped this drive after the first keyed+clean publish because the
            // per-frame deep GX deref could race a game-thread renderer teardown: a renderer freed
            // between our vtable check and the deep deref (TOCTOU) surfaced as three crash flavors
            // (Scaleform dtor, GX-queue null, garbage-vtable RIP). That trade froze the portrait
            // ~6-13 frames into a ~400-frame window -- the user-visible bug #1. The race is now
            // closed structurally by the TEARDOWN FENCE instead of by not driving: the pump sets
            // its busy flag (PROFILE_IN_OUR_DRIVE) FIRST and only drives if
            // PROFILE_RENDERER_TEARDOWN_FENCE is down, while the game-thread teardown raises the
            // fence and waits for the busy flag to drop before any delete-enqueue runs (both
            // SeqCst -- one side always yields; see profile_renderer_teardown_spare_hook). The
            // PROFILE_BAKE_RGBA_CAPTURED latch itself is unchanged: publish/overlay/readback
            // consumers still key on "first capture landed"; it just no longer stops the drive.
            if portrait_render_drive_enabled() && off != 0 && off != null && live_models <= 1 {
                // BUILD-DURATION semaphore: one log line on the null->valid model transition. Run
                // #9 implies the mid-load async build takes ~13s (kick +16.8s -> stable gate first
                // passes ~+29.5s) from world-streaming contention -- vs the boot-era 133ms build on
                // an idle title screen. This stamps the exact completion so the theory is measured,
                // not inferred.
                {
                    static MODEL_WAS_LIVE: AtomicUsize = AtomicUsize::new(0);
                    let m = unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                        .unwrap_or(0);
                    let live_now = (m != 0 && m != null) as usize;
                    let was = MODEL_WAS_LIVE.swap(live_now, Ordering::SeqCst);
                    if live_now == 1 && was == 0 {
                        append_autoload_debug(format_args!(
                            "portrait-model-LIVE: model_ins=0x{m:x} on r=0x{r:x} (stamp this line's +ms against the build kick's for the async build duration)"
                        ));
                    }
                }
                let captured = PROFILE_DRAW_TASK_CTX.load(Ordering::SeqCst);
                let own = task_data as *const FD4TaskData as usize;
                // A captured engine ctx whose +8 delta-time reads 0 FREEZES the anim no matter how
                // often we pump (run #7: dt=0.0000, anim_t stuck at 0.153s) -- prefer our own live
                // draw-phase task_data whenever the captured dt is not a sane frame delta.
                let td = if captured != 0 && captured != null {
                    let cap_dt = f32::from_bits(
                        (unsafe { safe_read_usize(captured + 8) }.unwrap_or(0) & 0xffff_ffff)
                            as u32,
                    );
                    if cap_dt > 0.0 && cap_dt < 1.0 {
                        captured
                    } else {
                        own
                    }
                } else {
                    own
                };
                // NOTE (run #14 diagnostic): the anim entry's +0x54 field CYCLES 0.1->2.1->1.1
                // mod 3.0 -- the menu-context idle LOOPS natively (3.0s cycle); the earlier
                // "anim_t frozen at 2.550" was ALIASING (the motion log samples every ~6.0s = two
                // full loops, always landing on the same phase). No loop-restart is needed; the
                // sustained alpha_motion ~1000 is the idle's real (subtle) breathing amplitude,
                // and the early ~3237 spike is the one-off menu-pose -> idle transition.
                PROFILE_IN_OUR_DRIVE.store(true, Ordering::SeqCst);
                // Fence check MUST come after the busy-flag store (Dekker order): the teardown
                // either already sees us busy and is waiting (we bail out immediately), or it
                // raised the fence first and we never touch the renderer this frame.
                if PROFILE_RENDERER_TEARDOWN_FENCE.load(Ordering::SeqCst) != 0 {
                    PROFILE_IN_OUR_DRIVE.store(false, Ordering::SeqCst);
                    PROFILE_DRIVE_FENCE_SKIPS.fetch_add(1, Ordering::SeqCst);
                } else {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        // NATIVE SCENE-ALPHA KEYING: clear the offscreen RT to {0,0,0,0} before
                        // this frame's model draw. The draw below redraws ONLY the model, so the
                        // RT leaves this frame subject-only with alpha == model coverage -- the
                        // backdrop box (stale pixels the old skip-the-clear preserved) is gone
                        // and the publish keys on native alpha instead of depth masks.
                        if unsafe {
                            crate::experiments::gpu_readback::portrait_alpha0_clear(base, off)
                        } {
                            PROFILE_ALPHA0_CLEARS.fetch_add(1, Ordering::SeqCst);
                        }
                        let update: unsafe extern "system" fn(usize, usize) =
                            unsafe { core::mem::transmute(base + PROFILE_MODEL_UPDATE_TASK_RVA) };
                        unsafe { update(r, td) };
                        // The draw task is the fn per_frame_push_hook detours; calling the hook
                        // directly applies the look-at then runs the original body via its
                        // trampoline.
                        unsafe { per_frame_push_hook(r, td) };
                    }));
                    PROFILE_IN_OUR_DRIVE.store(false, Ordering::SeqCst);
                    PROFILE_PERFRAME_MODEL_DRAWS.fetch_add(1, Ordering::SeqCst);
                    // Animation-stall semaphore: this frame the drive actually rendered
                    // (animated). With the freeze relaxed this should track display frames ~1:1;
                    // drive << display in the window-reset snapshot means the head froze early.
                    PROFILE_DRIVE_FRAMES_WINDOW.fetch_add(1, Ordering::SeqCst);
                }
            }
            if stable_target_only {
                // (Removed 2026-07-03: the PER-SCENE ENVIRONMENT LEVER "proof pass" that wrote gamma(+0x60)=1.0
                // and exposure(filter+0x8c)=8.0 into the portrait tonemap filter every drive frame. That 8x
                // overexposure blew out the portrait for the few drive frames per window -- the user-observed
                // luminosity spike "a few frames pre/post transition" -- and its blown-out colours also broke
                // the mask/head IoU classification. The RE finding (filter = *(*( *(off+0x48) +0x38) +0xbf50),
                // exposure at +0x8c) is preserved in bd; the portrait now renders with the game's own tonemap.)
                // RE-RASTERIZE the posed model into OUR built renderer's offscreen RT each render-thread
                // frame. draw_step (the per-slot rasterize loop over the title table) does NOT include our
                // own-built renderer, and the engine only redraws on profile data-change -- so without this
                // the look-at bone writes never reach the RT and the captured head is a STALE render (proven:
                // cursor LEFT vs RIGHT dumps were 95% identical, head centroid did not move). The offscreen
                // thunk (FUN_140bb8ca0) submits FUN_140bb73a0(*(r+0xa8)) using the live global GxDrawContext;
                // we OWN this renderer (force_profile_render built it) so its model+deps are alive (unlike the
                // teardown-freed spared renderer this crashes on). Runs before the RT->SRV copy + readback so
                // they capture the fresh pose. Gated by the existing render-drive lever; bumps the hits oracle.
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
                // (The H2-vs-H3 deferred-readback diagnostic that lived here has been removed now that the
                // cause is settled: the cached-resource readback went stale [~10% nonblack] while a
                // fresh-resolved readback saw the head [~73%] -- see readback_offscreen_fast below. Keeping a
                // second per-frame readback would just waste GPU bandwidth on the now-known-bad cached path.)
                // DISABLED (2026-06-30): drive(r) = FUN_140bb8d90 -> FUN_140bb73a0 is ONLY a ClearRTV of the
                // offscreen RT (RE-confirmed by decompile), NOT a re-rasterize as the original author believed.
                // Running it every frame (render_drives~206) WIPES the offscreen RT to black every frame, so
                // the engine's ~4x genuine head renders get cleared on the ~200 intervening frames -> the
                // readback reads black ~97%. The engine's own offscreen pass does its own clear before its
                // draw, so removing OUR standalone clear cannot starve the engine renders -- it only stops us
                // erasing them. TEST: with this off the last engine-rendered head should PERSIST in the RT so
                // the readback returns it every frame. Keep the no-op behind the gate for telemetry parity.
                let _ = PROFILE_OFFSCREEN_DRIVE_RVA;
                if portrait_render_drive_enabled() {
                    PROFILE_RENDER_DRIVE_HITS.fetch_add(1, Ordering::SeqCst);
                }
                // PER-FRAME MODEL RASTERIZE (the actual fix). The ~4x head refresh is NOT pool contention
                // (free_min=18) nor a readback race (deferred read was also 4x) -- it is that the engine's
                // own profile UPDATE+DRAW CSEzUpdateTasks (FUN_140bba820 / FUN_140bba7d0) are under-scheduled
                // post-Continue (~4-19x/loading screen) by their ResMan driver, so the model only re-skins +
                // re-enqueues into the offscreen RT that few times. drive(r) above is ONLY a ClearRTV (RE-
                // confirmed: FUN_140bb73a0). Here we drive the real per-frame render ourselves, on the render
                // thread inside the live GX frame, passing OUR task's FD4TaskData as the `frame` arg (its +8
                // delta-time is the only scalar consumed; the GX submit routes via the global frame/GX ctx):
                //   1. UPDATE task FUN_140bba820(r, td): runs the FD4 stepper + refreshes model transform/anim.
                //   2. DRAW task (== per_frame_push_hook's target FUN_140bba7d0): we call per_frame_push_hook
                //      DIRECTLY so it applies the live look-at pose THEN calls the original body (skin submodels
                //      + GX-enqueue = the rasterize). Guard on model_ins(+0x778) && X(+0x948) (the state machine
                //      reached STEP_Wait_Play) so a half-built renderer can't fault the draw. catch_unwind so a
                //      bad frame degrades to the old behaviour instead of crashing the render thread.
                if portrait_render_drive_enabled() {
                    let model_ins =
                        unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                            .unwrap_or(0);
                    let loc = unsafe { safe_read_usize(r + 0x948) }.unwrap_or(0);
                    if model_ins != 0 && model_ins != null && loc != 0 && loc != null {
                        // REBUILD-DRIVER TRIPWIRE (see PORTRAIT_FACEDATA_NEQ_TICKS): sample the
                        // step-machine latches and re-run STEP_Wait_Play's own FaceData compare
                        // each drive frame. A ~100% mismatch rate convicts the FaceData loop (the
                        // step invalidates the model every tick we drive it); nonzero latch bytes
                        // convict a latch raiser.
                        PORTRAIT_DRIVE_TICKS.fetch_add(1, Ordering::SeqCst);
                        let l754 = unsafe { safe_read_u8(r + 0x754) }.unwrap_or(0xff);
                        let l755 = unsafe { safe_read_u8(r + 0x755) }.unwrap_or(0xff);
                        let l756 = unsafe { safe_read_u8(r + 0x756) }.unwrap_or(0xff);
                        let fd_neq = {
                            let get_buf: unsafe extern "system" fn(usize, u8) -> usize =
                                unsafe { core::mem::transmute(base + PROFILE_FACEDATA_BUFFER_RVA) };
                            let buf =
                                unsafe { get_buf(r + PROFILE_RENDERER_FACEDATA_OBJ_OFFSET, 1) };
                            if buf != 0 && buf != null {
                                let a = unsafe {
                                    std::slice::from_raw_parts(
                                        buf as *const u8,
                                        PROFILE_FACEDATA_CMP_LEN,
                                    )
                                };
                                let b = unsafe {
                                    std::slice::from_raw_parts(
                                        (r + PROFILE_RENDERER_FACEDATA_CMP_OFFSET) as *const u8,
                                        PROFILE_FACEDATA_CMP_LEN,
                                    )
                                };
                                a != b
                            } else {
                                false
                            }
                        };
                        if fd_neq {
                            PORTRAIT_FACEDATA_NEQ_TICKS.fetch_add(1, Ordering::SeqCst);
                        }
                        // IDLE-ANIM BIND (per model incarnation). The native pipeline binds anim
                        // id 0 = the STATIC menu pose, so the per-frame anim step below has nothing
                        // to move; re-bind a real idle on OUR renderer so the same step animates it
                        // at frame rate (RE: bd portrait-anim-bind-RE-corrects-6hz-gate-2026-07-03).
                        // Same call shape as the engine's binds (force=1, mode=0); success/failure
                        // judged by the +0x96c handle leaving the null sentinel -- exactly the gate
                        // the update task itself uses. Keyed to the live (renderer, anim-holder)
                        // pair, NOT a one-shot: the loading window rebuilds the model (run
                        // 20260703-074216 saw 2 pin moves after a one-shot bind, leaving the
                        // displayed model on the static pose). A fresh renderer or fresh X rebinds.
                        if PORTRAIT_ANIM_BOUND_RENDERER.load(Ordering::SeqCst) != r
                            || PORTRAIT_ANIM_BOUND_LOC.load(Ordering::SeqCst) != loc
                        {
                            let sentinel =
                                unsafe { safe_read_usize(base + PROFILE_ANIM_NULL_HANDLE_RVA) }
                                    .unwrap_or(0)
                                    & 0xffff_ffff;
                            PORTRAIT_ANIM_SENTINEL.store(sentinel, Ordering::SeqCst);
                            let handle_at = |r: usize| {
                                unsafe { safe_read_usize(r + PROFILE_ANIM_HANDLE_OFFSET) }
                                    .unwrap_or(0)
                                    & 0xffff_ffff
                            };
                            let before = handle_at(r);
                            PORTRAIT_ANIM_HANDLE_BEFORE.store(before, Ordering::SeqCst);
                            let id968_pre =
                                unsafe { safe_read_usize(r + 0x968) }.unwrap_or(0) & 0xffff_ffff;
                            let mut outcome = 2usize;
                            let mut bound_id = -1i32;
                            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let bind: unsafe extern "system" fn(usize, *const i32, u8, u8) =
                                    unsafe { core::mem::transmute(base + PROFILE_ANIM_BIND_RVA) };
                                for &id in PORTRAIT_IDLE_ANIM_IDS.iter() {
                                    PORTRAIT_ANIM_BIND_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
                                    unsafe { bind(r, &id, 1, 0) };
                                    let h = handle_at(r);
                                    PORTRAIT_ANIM_HANDLE.store(h, Ordering::SeqCst);
                                    if h != sentinel && h != 0xffff_ffff {
                                        bound_id = id;
                                        outcome = 1;
                                        break;
                                    }
                                }
                            }));
                            if outcome == 1 {
                                PORTRAIT_ANIM_BOUND_ID.store(bound_id as usize, Ordering::SeqCst);
                            }
                            PORTRAIT_ANIM_BIND_STATE.store(outcome, Ordering::SeqCst);
                            PORTRAIT_ANIM_BOUND_RENDERER.store(r, Ordering::SeqCst);
                            PORTRAIT_ANIM_BOUND_LOC.store(loc, Ordering::SeqCst);
                            append_autoload_debug(format_args!(
                                "portrait-anim-bind: r=0x{r:x} loc=0x{loc:x} latches={l754:x}/{l755:x}/{l756:x} fd_neq={fd_neq} id968_pre={id968_pre} sentinel=0x{sentinel:x} handle before=0x{before:x} after=0x{:x} -> {}",
                                PORTRAIT_ANIM_HANDLE.load(Ordering::SeqCst),
                                if outcome == 1 {
                                    format!("BOUND idle anim {bound_id}")
                                } else {
                                    "no candidate resolved (static pose kept)".to_owned()
                                },
                            ));
                        }
                        // (update+push live in the unconditional STATE-MACHINE PUMP above --
                        // running them here too would double-step the anim.)
                    }
                }
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
                    // LIVE TRACKING -- EVERY FRAME. FIX (2026-06-30): use readback_offscreen_fast, which
                    // RE-RESOLVES the live content RT fresh each frame (find_d3d12_resource(off)) -- the exact
                    // path the in-process RT sample uses (proven nonblack ~63% with the clear disabled) -- but
                    // copies via the cached RB_FAST_* objects so it still succeeds every frame. The previous
                    // readback_cached_content_rgba8 cached the RESOURCE once and went stale: it read black ~98%
                    // (the offscreen RT is recreated by the 1024 resize so the cached handle dangled), while
                    // the freshly-resolved RT held the head. We are inside the model_ins/loc + vtable validated
                    // block, so the per-frame resolve cannot race a teardown free.
                    if portrait_render_drive_enabled() {
                        // COHERENT color+depth (bug #3 fix): reads the color RT and its depth sibling on
                        // ONE fence and stashes the matching depth for apply_depth_alpha_key below, so the
                        // cutout is derived from the SAME frame as the head. Same return shape as
                        // readback_offscreen_fast; falls back to it (separate depth) if the coherent read
                        // fails -- so this can only add coherence, never regress.
                        if let Some((cw, ch, mut cpx, rt_cand)) =
                            unsafe { readback_offscreen_fast_coherent(off) }
                        {
                            // COLOR PROVENANCE (green-face wrong-buffer fix): the nest holds same-size
                            // same-format NON-final targets (material/G-buffer -- flat-green face,
                            // saturated-orange emissive), and keyed+tear cannot tell buffers apart. Only
                            // a color resolved from the scene bundle's own RTV is identity-proven; a
                            // scan-resolved frame must neither latch the pin nor display (bridge holds).
                            let color_from_bundle =
                                crate::experiments::gpu_readback::PROFILE_COLOR_SRC_BUNDLE_LAST
                                    .load(Ordering::SeqCst)
                                    != 0;
                            // DIAGNOSTIC: count readbacks + checker-classified frames, and one-shot dump a
                            // "checker" frame (slot 103) so we can SEE what the ~216 non-published frames
                            // actually contain (forge magenta/yellow placeholder vs black vs partial head).
                            PROFILE_READBACK_SOME.fetch_add(1, Ordering::SeqCst);
                            let is_checker = portrait_looks_like_checker(cw, ch, &cpx);
                            if is_checker {
                                PROFILE_READBACK_CHECKER.fetch_add(1, Ordering::SeqCst);
                                if PROFILE_CHECKER_DUMPED.swap(true, Ordering::SeqCst) != true {
                                    dump_portrait_rgba(103, cw, ch, &cpx);
                                }
                            } else if !color_from_bundle {
                                // Real (non-checker) content but scan-resolved: never pin, never
                                // display -- the bridge holds the last identity-proven frame.
                                PROFILE_PUBLISH_SKIPPED_UNPAIRED.fetch_add(1, Ordering::SeqCst);
                            }
                            if !is_checker && color_from_bundle {
                                // PIN the confirmed-head content RT candidate: subsequent scans prefer it
                                // outright, so the publish source can never flip to another slot's
                                // same-size RT mid-load (the cross-slot swap). A switch after first latch
                                // means the RT was genuinely recreated -- counted as the swap tripwire.
                                // (Bundle-provenance frames only: a scan-resolved candidate could latch
                                // the pin onto the material buffer and keep re-picking it all window.)
                                let prev = PROFILE_RT_PIN.swap(rt_cand, Ordering::SeqCst);
                                if prev != 0 && prev != rt_cand {
                                    let n = PROFILE_RT_PIN_SWITCHES.fetch_add(1, Ordering::SeqCst);
                                    // NEW MODEL came in (the content RT was recreated -- e.g. a System Quit
                                    // character switch): invalidate the depth masking plane so the cutout
                                    // recomputes for this model instead of reusing the previous character's
                                    // cached silhouette.
                                    invalidate_portrait_depth_mask();
                                    // Also drop the motion-metric history: a model switch produces a giant
                                    // one-off silhouette diff that is NOT animation (run 20260703-074216:
                                    // metric max 51049 was pin-move contamination, not motion).
                                    if let Ok(mut g) = PORTRAIT_MOTION_PREV_PLANES.lock() {
                                        *g = None;
                                    }
                                    if n < 4 {
                                        append_autoload_debug(format_args!(
                                            "live-feed: content-RT pin MOVED 0x{prev:x} -> 0x{rt_cand:x} -- new model, depth mask invalidated (switch #{})",
                                            n + 1
                                        ));
                                    }
                                }
                                let nb = portrait_center_nonblack(cw, ch, &cpx);
                                LOADING_BG_PORTRAIT_NONBLACK.store(nb as usize, Ordering::SeqCst);
                                LOADING_BG_PORTRAIT_IS_CHECKER.store(0, Ordering::SeqCst);
                                LOADING_BG_PORTRAIT_DIMS
                                    .store(((cw as usize) << 16) | (ch as usize), Ordering::SeqCst);
                                // ALPHA DIAGNOSTIC (one-shot, for the "full-alpha background" goal): sample
                                // the RT (R8G8B8A8) at a BACKGROUND corner vs the HEAD center, plus the
                                // alpha min/max across the frame. This decides the alpha path: if corner
                                // alpha==0 and center alpha==255 the RT already carries a clean per-pixel
                                // cutout (honor alpha in the composite -> transparent bg is nearly free); if
                                // alpha is 255 everywhere the bg is opaque (need a chroma-key or engine-side
                                // IBL/env suppression). Fires only on a confirmed non-checker head frame.
                                {
                                    static ALPHA_DIAG_LOGGED: AtomicUsize = AtomicUsize::new(0);
                                    let w = cw as usize;
                                    let h = ch as usize;
                                    if w > 16
                                        && h > 16
                                        && cpx.len() >= w * h * 4
                                        && ALPHA_DIAG_LOGGED.swap(1, Ordering::SeqCst) == 0
                                    {
                                        let at = |x: usize, y: usize| {
                                            let i = (y * w + x) * 4;
                                            (cpx[i], cpx[i + 1], cpx[i + 2], cpx[i + 3])
                                        };
                                        let corner = at(8, 8);
                                        let center = at(w / 2, h / 2);
                                        let (mut amin, mut amax) = (255u8, 0u8);
                                        let mut y = 0;
                                        while y < h {
                                            let mut x = 0;
                                            while x < w {
                                                let a = cpx[(y * w + x) * 4 + 3];
                                                if a < amin {
                                                    amin = a;
                                                }
                                                if a > amax {
                                                    amax = a;
                                                }
                                                x += 37;
                                            }
                                            y += 37;
                                        }
                                        append_autoload_debug(format_args!(
                                            "alpha-diag: {w}x{h} corner(bg) RGBA=({},{},{},{}) center(head) RGBA=({},{},{},{}) frame-alpha[min={amin} max={amax}]",
                                            corner.0,
                                            corner.1,
                                            corner.2,
                                            corner.3,
                                            center.0,
                                            center.1,
                                            center.2,
                                            center.3
                                        ));
                                    }
                                }
                                // NATIVE SCENE-ALPHA KEYING (strategy pivot 2026-07-03, replaces the
                                // depth-histogram mask): the pump now clears the offscreen RT with
                                // alpha 0 each frame (portrait_alpha0_clear) and redraws ONLY the
                                // model, so the RT's own alpha channel IS the mask -- backdrop
                                // geometry is never redrawn (it was stale pixels preserved by the
                                // old skip-the-clear behavior), and every depth-classification
                                // failure mode (wrong gap, continuous depth, wrong-place cutouts)
                                // is structurally gone. apply_depth_alpha_key is retired from this
                                // path; the keyed/share gate below now reads the native alpha.
                                let _ = apply_depth_alpha_key; // retained for reference, not called
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
                                // PIXEL-MOTION + FLICKER oracles (before the publish move). The
                                // lighting changes every frame (user 2026-07-03), so MOTION is judged
                                // on the depth-keyed ALPHA silhouette (lighting-immune: alpha comes
                                // from the depth buffer, applied to cpx above) and only across frames
                                // that BOTH carry a real cutout; the LUMA delta on the same grid is
                                // kept as the flicker gauge, not a motion oracle.
                                {
                                    const GW: usize = 32;
                                    let (w, h) = (cw as usize, ch as usize);
                                    if w >= GW && h >= GW && cpx.len() >= w * h * 4 {
                                        let mut alpha = vec![0u8; GW * GW];
                                        let mut luma = vec![0u8; GW * GW];
                                        let mut transparent_cells = 0usize;
                                        for gy in 0..GW {
                                            for gx in 0..GW {
                                                let p = ((gy * h / GW) * w + gx * w / GW) * 4;
                                                let l = (cpx[p] as u32 * 30
                                                    + cpx[p + 1] as u32 * 59
                                                    + cpx[p + 2] as u32 * 11)
                                                    / 100;
                                                luma[gy * GW + gx] = l as u8;
                                                let a = cpx[p + 3];
                                                alpha[gy * GW + gx] = a;
                                                if a < 128 {
                                                    transparent_cells += 1;
                                                }
                                            }
                                        }
                                        let keyed = transparent_cells > 0;
                                        let mad = |a: &[u8], b: &[u8]| {
                                            let sum: u64 = a
                                                .iter()
                                                .zip(b.iter())
                                                .map(|(x, y)| {
                                                    (*x as i32 - *y as i32).unsigned_abs() as u64
                                                })
                                                .sum();
                                            (sum * 1000 / a.len() as u64) as usize
                                        };
                                        if let Ok(mut prev) = PORTRAIT_MOTION_PREV_PLANES.lock() {
                                            if let Some((pa, pl, pkeyed)) = prev.as_ref() {
                                                let flicker = mad(pl, &luma);
                                                PORTRAIT_LUMA_FLICKER_LAST
                                                    .store(flicker, Ordering::SeqCst);
                                                PORTRAIT_LUMA_FLICKER_MAX
                                                    .fetch_max(flicker, Ordering::SeqCst);
                                                if keyed && *pkeyed {
                                                    let motion = mad(pa, &alpha);
                                                    PORTRAIT_MOTION_METRIC_LAST
                                                        .store(motion, Ordering::SeqCst);
                                                    PORTRAIT_MOTION_METRIC_MAX
                                                        .fetch_max(motion, Ordering::SeqCst);
                                                }
                                            }
                                            *prev = Some((alpha, luma, keyed));
                                        }
                                        // Sampled time series (~1 line/s at 60fps): motion (alpha)
                                        // vs flicker (luma) each publish window, plus the three
                                        // remaining pose-chain links -- anim entry playback clock
                                        // (entry = *(X+8) + (handle&0xffff)*0x68, time f32 @ +0x54;
                                        // advancing == the anim is really stepping), the dt fed to
                                        // the update task (*(td+8); 0 would freeze the anim
                                        // silently), and the offscreen scene-registered bit
                                        // (off+0x58; 1 == the engine re-renders the RT per frame).
                                        static MOTION_LOG_TICKS: AtomicUsize = AtomicUsize::new(0);
                                        let n = MOTION_LOG_TICKS.fetch_add(1, Ordering::SeqCst);
                                        if n % 60 == 0 {
                                            let r_now = unsafe {
                                                safe_read_usize(portrait_renderer_table_entry(
                                                    base,
                                                    portrait_loaded_slot(),
                                                ))
                                            }
                                            .unwrap_or(0);
                                            let (anim_t, dt, scene_reg) = if r_now != 0
                                                && r_now != null
                                            {
                                                let x = unsafe { safe_read_usize(r_now + 0x948) }
                                                    .unwrap_or(0);
                                                let h = unsafe {
                                                    safe_read_usize(
                                                        r_now + PROFILE_ANIM_HANDLE_OFFSET,
                                                    )
                                                }
                                                .unwrap_or(0)
                                                    & 0xffff;
                                                let anim_t = if x != 0 && x != null {
                                                    let entries = unsafe { safe_read_usize(x + 8) }
                                                        .unwrap_or(0);
                                                    if entries != 0 && entries != null {
                                                        f32::from_bits(
                                                            (unsafe {
                                                                safe_read_usize(
                                                                    entries + h * 0x68 + 0x54,
                                                                )
                                                            }
                                                            .unwrap_or(0)
                                                                & 0xffff_ffff)
                                                                as u32,
                                                        )
                                                    } else {
                                                        -1.0
                                                    }
                                                } else {
                                                    -1.0
                                                };
                                                let td =
                                                    PROFILE_DRAW_TASK_CTX.load(Ordering::SeqCst);
                                                let dt = if td != 0 && td != null {
                                                    f32::from_bits(
                                                        (unsafe { safe_read_usize(td + 8) }
                                                            .unwrap_or(0)
                                                            & 0xffff_ffff)
                                                            as u32,
                                                    )
                                                } else {
                                                    -1.0
                                                };
                                                let off_now = unsafe {
                                                    safe_read_usize(
                                                        r_now
                                                            + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET,
                                                    )
                                                }
                                                .unwrap_or(0);
                                                let scene_reg = if off_now != 0 && off_now != null {
                                                    unsafe { safe_read_u8(off_now + 0x58) }
                                                        .unwrap_or(0xff)
                                                } else {
                                                    0xff
                                                };
                                                (anim_t, dt, scene_reg)
                                            } else {
                                                (-1.0, -1.0, 0xff)
                                            };
                                            let dt_own = f32::from_bits(
                                                (unsafe {
                                                    safe_read_usize(
                                                        task_data as *const FD4TaskData as usize
                                                            + 8,
                                                    )
                                                }
                                                .unwrap_or(0)
                                                    & 0xffff_ffff)
                                                    as u32,
                                            );
                                            append_autoload_debug(format_args!(
                                                "portrait-motion[t{n}]: alpha_motion last={} max={} luma_flicker last={} max={} keyed={keyed} anim_t={anim_t:.3} dt_cap={dt:.4} dt_own={dt_own:.4} scene_reg={scene_reg}",
                                                PORTRAIT_MOTION_METRIC_LAST.load(Ordering::SeqCst),
                                                PORTRAIT_MOTION_METRIC_MAX.load(Ordering::SeqCst),
                                                PORTRAIT_LUMA_FLICKER_LAST.load(Ordering::SeqCst),
                                                PORTRAIT_LUMA_FLICKER_MAX.load(Ordering::SeqCst),
                                            ));
                                        }
                                    }
                                }
                                // The whole live-drive block is gated on the stable target-only state above,
                                // so this readback is the loaded character only. KEYED-GATE (never render
                                // an unmasked model, user 2026-07-03): only publish/freeze when the depth
                                // mask actually cut out background (a transparent pixel exists). An unmasked
                                // fail-open frame (all alpha 255, mask not ready yet) is skipped, so the
                                // display never freezes on an opaque IBL box -- and the make-before-break
                                // bridge keeps the PRIOR masked head (PROFILE_HAVE_KEYED_FRAME) on screen
                                // until THIS model produces its own masked frame, which then replaces it.
                                // MASK-FRACTION FLOOR (er-effects-rs-hi2, user saw a displayed head
                                // with NO mask): "any transparent pixel" let a PARTIAL mask through
                                // -- a frame that is 99% opaque IBL box with a few cut pixels passed
                                // keyed and displayed as unmasked. A real portrait mask cuts a
                                // substantial background fraction, so require a minimum transparent
                                // share; the 0 < share < floor band is counted separately (lowmask)
                                // to attribute partial-mask frames vs fully-unkeyed ones.
                                let total_px = (cpx.len() / 4).max(1);
                                let transparent_px =
                                    cpx.chunks_exact(4).filter(|px| px[3] < 128).count();
                                let share_pct = transparent_px * 100 / total_px;
                                let keyed = share_pct >= PORTRAIT_MIN_TRANSPARENT_PCT;
                                let partial_mask = !keyed && transparent_px > 0;
                                // Floor-evidence stats: the two sides of the floor per window --
                                // published minimum share (was the boundary frame barely passing?)
                                // and lowmask maximum (how close held frames came).
                                if keyed {
                                    PROFILE_PUBLISH_SHARE_MIN
                                        .fetch_min(share_pct, Ordering::SeqCst);
                                } else if partial_mask {
                                    PROFILE_LOWMASK_SHARE_MAX
                                        .fetch_max(share_pct, Ordering::SeqCst);
                                }
                                // TORN-READBACK gate (user 2026-07-03): the offscreen readback has no
                                // cross-queue sync vs the game's render of the RT, so a per-frame capture
                                // can be torn (scanline garbage) even though it is keyed. Score the
                                // vertical luma tearing over the masked head; publish only a CLEAN frame,
                                // else hold the prior clean head via the bridge (never flash garbage).
                                let tear = portrait_tear_score(&cpx, cw as usize, ch as usize);
                                PROFILE_TEAR_SCORE_LAST.store(tear, Ordering::SeqCst);
                                PROFILE_TEAR_SCORE_MAX.fetch_max(tear, Ordering::SeqCst);
                                // ADAPTIVE TEAR BASELINE (runs 6-7: speckled/stone-textured
                                // characters score a CONSTANT ~39-40 on every honest frame -- the
                                // vertical-luma metric reads their legitimate texture, and the
                                // absolute threshold starved whole windows, e.g. slot8 torn=149
                                // with 76%-share masks). Baseline = EMA of ACCEPTED frames only (a
                                // real tear never feeds it, so smooth characters keep the strict
                                // absolute gate); a window's first frame is capped at 5x the
                                // absolute threshold. Reset per window.
                                let ema = PROFILE_TEAR_EMA.load(Ordering::SeqCst);
                                let clean = if ema == 0 {
                                    tear <= PROFILE_TEAR_SCORE_THRESHOLD * 5
                                } else {
                                    tear <= PROFILE_TEAR_SCORE_THRESHOLD.max(ema * 2)
                                };
                                if clean {
                                    let next = if ema == 0 {
                                        tear.max(1)
                                    } else {
                                        (ema * 7 + tear.max(1)).div_ceil(8)
                                    };
                                    PROFILE_TEAR_EMA.store(next, Ordering::SeqCst);
                                }
                                // MASK-CORRECTNESS gate (user 2026-07-03: frames displayed whose
                                // backdrop was not keyed out right -- the share floor checks how
                                // MUCH the mask cut, this checks WHERE): the mask/head IoU of THIS
                                // frame (apply_depth_alpha_key ran just above) must clear the
                                // gross-mismatch bar or the frame holds on the bridge.
                                let iou_ok =
                                    crate::experiments::gpu_readback::PROFILE_MASK_HEAD_IOU_LAST
                                        .load(Ordering::SeqCst)
                                        >= crate::experiments::gpu_readback::MASK_HEAD_IOU_MIN;
                                if keyed && clean && iou_ok {
                                    PROFILE_TEAR_SCORE_CLEAN_MIN.fetch_min(tear, Ordering::SeqCst);
                                    PROFILE_PUBLISH_CLEAN.fetch_add(1, Ordering::SeqCst);
                                    // First-keyed latency (er-effects-rs-hi2): stamp the display-frame
                                    // index of this window's FIRST published frame -- how long the
                                    // bridge held the prior head before the new one took over.
                                    let _ = PROFILE_WINDOW_FIRST_KEYED_DISPLAY.compare_exchange(
                                        usize::MAX,
                                        PROFILE_DISPLAY_FRAMES_WINDOW.load(Ordering::SeqCst),
                                        Ordering::SeqCst,
                                        Ordering::SeqCst,
                                    );
                                    if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
                                        *g = Some((cw, ch, cpx));
                                    }
                                    LOADING_BG_PORTRAIT_RGBA_VERSION.fetch_add(1, Ordering::SeqCst);
                                    // Freeze the per-frame drive for this window (UAF fix) ...
                                    PROFILE_BAKE_RGBA_CAPTURED.store(1, Ordering::SeqCst);
                                    // ... and mark a keyed frame available for display (persists across the
                                    // window reset/retarget so the bridge holds until the next keyed frame).
                                    PROFILE_HAVE_KEYED_FRAME.store(1, Ordering::SeqCst);
                                    if PROFILE_LIVE_FEED_LOGGED.swap(1, Ordering::SeqCst) == 0 {
                                        append_autoload_debug(format_args!(
                                            "live-feed: published built RT content {cw}x{ch} (real head, !checker, keyed, clean tear={tear}, target-only) -> overlay (version bump)"
                                        ));
                                    }
                                } else if keyed && clean {
                                    // Mask cut ENOUGH but in the WRONG PLACE (IoU below the gross-
                                    // mismatch bar): the stale-silhouette / wrong-side masks the
                                    // user saw displayed as un-keyed backdrops. Held on the bridge.
                                    PROFILE_PUBLISH_SKIPPED_BADIOU.fetch_add(1, Ordering::SeqCst);
                                } else if keyed {
                                    // Keyed but TORN (offscreen RT read mid-GPU-write -- no cross-queue
                                    // sync): SKIP so the garbage never displays; the make-before-break
                                    // bridge holds the last CLEAN head. Validated safe as the product fix
                                    // (run autostep10m): clean frames score 1-7 and land constantly
                                    // (1957 published), torn frames are rare (one at tear=80) -- so the
                                    // skip catches them without ever starving the display. Regressions
                                    // surface as oracle_portrait_publish_skipped_torn climbing.
                                    let n =
                                        PROFILE_PUBLISH_SKIPPED_TORN.fetch_add(1, Ordering::SeqCst);
                                    if n % 64 == 0 {
                                        append_autoload_debug(format_args!(
                                            "portrait-tear: skipped torn keyed frame tear={tear} > {PROFILE_TEAR_SCORE_THRESHOLD} (max={}, #torn={})",
                                            PROFILE_TEAR_SCORE_MAX.load(Ordering::SeqCst),
                                            n + 1
                                        ));
                                    }
                                } else if partial_mask {
                                    // Mask exists but cuts almost nothing (< floor): the frame the
                                    // user previously SAW as an unmasked head. Held on the bridge.
                                    PROFILE_PUBLISH_SKIPPED_LOWMASK.fetch_add(1, Ordering::SeqCst);
                                } else {
                                    PROFILE_PUBLISH_SKIPPED_UNKEYED.fetch_add(1, Ordering::SeqCst);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // SPARED-RENDERER DRIVE DISABLED (subsequent-load cascade fix, 2026-07-02). The spared renderer's model
    // is FREED by the Continue teardown (re-attach CRASHES -- see the note below), so this drive rasterized a
    // STALE / garbage RT of the PREVIOUS character. During a character switch that stale RT competed with the
    // rebuilt-own target renderer in the readback scan, so the display flashed the old/other character before
    // the target resolved (user-observed "other char -> first char -> target" cascade) and the RT pin bounced
    // between the two. The live render now comes SOLELY from BUILDING OUR OWN renderer post-Continue
    // (force_profile_render_tick, which owns its model+deps with our lifetime), so the spare is no longer a
    // render source -- it stays only as the table-protection artifact its hook creates. Keeping the RVA + a
    // vtable read for reference; NOT calling the thunk.
    // (Re-attach history: run 2026-06-30 AV in the ResMan/offscreen-draw path +28ms after writing the model
    // into the spared renderer's +0x778 -- the teardown frees the model's deeper render deps. See bd
    // portrait-live-render-reattach-crashes-build-own-2026-06-30.)
    let _ = (
        LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst),
        null,
    );
    let _ = PROFILE_OFFSCREEN_DRIVE_RVA;
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
    // LOOK-BEFORE-BUILD: directly measure the GX subcontext pool's FREE depth this frame to settle whether
    // the ~4x head refresh is pool contention (pop fails 96%) or a readback/rasterize sync race. free =
    // (top - floor)/8; >0 means a subcontext is poppable. A min-free > 0 across the whole loading screen
    // refutes the contention theory (the pop never fails -> the black RT is a sync/rasterize problem).
    let floor = unsafe { safe_read_usize(ctx + GX_DRAW_CONTEXT_POOL_FLOOR_OFFSET) }.unwrap_or(0);
    let top = unsafe { safe_read_usize(ctx + GX_DRAW_CONTEXT_POOL_TOP_OFFSET) }.unwrap_or(0);
    if floor != 0 && top >= floor {
        let free = (top - floor) / 8;
        PROFILE_GX_POOL_FREE_LAST.store(free, Ordering::SeqCst);
        // monotonic min (CAS loop; only ever lowers)
        let mut cur = PROFILE_GX_POOL_FREE_MIN.load(Ordering::SeqCst);
        while free < cur {
            match PROFILE_GX_POOL_FREE_MIN.compare_exchange(
                cur,
                free,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(observed) => cur = observed,
            }
        }
    }
    let mask = unsafe { safe_read_usize(ctx + GX_DRAW_CONTEXT_POOL_USED_MASK_OFFSET) }.unwrap_or(0)
        & 0xffff_ffff;
    if mask != 0 {
        PROFILE_GX_POOL_USED_MASK.store(mask, Ordering::SeqCst);
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
            "lookat-phase-sweep: frame_begin={n} selected={}({}) selftest={} nowload={} loadbuilds={} render_drives={} hook_hits={} gx[samples={} nonempty={}] gxpool[free_min={} free_last={} N(maskpop)={}] rt[samples={} nonblack={} changed={}] readback[some={} checker={} defer_some={} defer_nonblack={}] modeldraws={} spared[ptr=0x{:x} model_ok={} draws={} hits={}] stage0[{}] phase_ticks[{}]",
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
            {
                let m = PROFILE_GX_POOL_FREE_MIN.load(Ordering::SeqCst);
                if m == usize::MAX { -1i64 } else { m as i64 }
            },
            PROFILE_GX_POOL_FREE_LAST.load(Ordering::SeqCst),
            (PROFILE_GX_POOL_USED_MASK.load(Ordering::SeqCst) as u32).count_ones(),
            PROFILE_LOOKAT_RT_SAMPLES.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_NONBLACK.load(Ordering::SeqCst),
            PROFILE_LOOKAT_RT_CHANGED.load(Ordering::SeqCst),
            PROFILE_READBACK_SOME.load(Ordering::SeqCst),
            PROFILE_READBACK_CHECKER.load(Ordering::SeqCst),
            PROFILE_READBACK_DEFERRED_SOME.load(Ordering::SeqCst),
            PROFILE_READBACK_DEFERRED_NONBLACK.load(Ordering::SeqCst),
            PROFILE_PERFRAME_MODEL_DRAWS.load(Ordering::SeqCst),
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
            let slot = portrait_loaded_slot();
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
            "lookat-pump-blocks: draws={} r_bad={} vt_bad={} off_bad={} multi={}",
            PROFILE_PERFRAME_MODEL_DRAWS.load(Ordering::SeqCst),
            PORTRAIT_PUMP_BLOCK_R.load(Ordering::SeqCst),
            PORTRAIT_PUMP_BLOCK_VTABLE.load(Ordering::SeqCst),
            PORTRAIT_PUMP_BLOCK_OFF.load(Ordering::SeqCst),
            PORTRAIT_PUMP_BLOCK_MULTI.load(Ordering::SeqCst),
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
pub(crate) unsafe fn profile_lookat_phase_draw_tick(phase_index: usize, task_data: &FD4TaskData) {
    if phase_index < LOOKAT_DRAW_PHASE_COUNT {
        PROFILE_LOOKAT_PHASE_TICKS[phase_index].fetch_add(1, Ordering::SeqCst);
    }
    if PROFILE_LOOKAT_SELECTED_PHASE.load(Ordering::SeqCst) != phase_index {
        return;
    }
    if let Ok(base) = game_module_base() {
        // Re-engage on every loading screen (subsequent-character-load fix): pause the draw/publish tick
        // ONLY during active gameplay, not permanently after the first world.
        if unsafe { portrait_pipeline_idle_in_gameplay(base) } {
            return;
        }
        unsafe { profile_lookat_realtime_draw_tick(base, task_data) };
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
    // CAPTURE the engine's live render context (param_2/frame) on its OWN calls only (not our re-drives),
    // so our per-frame draw can enqueue the model into the SAME offscreen pass the engine routes to. Our
    // draw-phase task_data routes to the wrong pass -> nothing renders into the portrait RT.
    if !PROFILE_IN_OUR_DRIVE.load(Ordering::SeqCst) && frame != 0 && frame != null {
        PROFILE_DRAW_TASK_CTX.store(frame, Ordering::SeqCst);
        if PROFILE_DRAW_TASK_CTX_LOGGED.fetch_add(1, Ordering::SeqCst) < 3 {
            let dt = unsafe { safe_read_usize(frame + 8) }.unwrap_or(0);
            append_autoload_debug(format_args!(
                "draw-task-ctx: engine called draw task with frame=0x{frame:x} *(frame+8)=0x{dt:x} (delta-time bits) renderer=0x{renderer:x}"
            ));
        }
    }
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
                    let own = portrait_loaded_slot();
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

/// Rotate a vector by an `(x,y,z,w)` quaternion. Used for extracting the portrait model's face direction
/// from the Head bone's model-space orientation without depending on any screen-space visual heuristic.
fn quat_rotate_vec3(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let [x, y, z, w] = q;
    let [vx, vy, vz] = v;
    let tx = 2.0 * (y * vz - z * vy);
    let ty = 2.0 * (z * vx - x * vz);
    let tz = 2.0 * (x * vy - y * vx);
    [
        vx + w * tx + (y * tz - z * ty),
        vy + w * ty + (z * tx - x * tz),
        vz + w * tz + (x * ty - y * tx),
    ]
}

/// Read the model transform's horizontal facing yaw from the `CSMenuAsmModelRend` row-major matrix at
/// `renderer+0x900`. The model's face is its local `-Z`; the matrix stores basis vectors by column, so the
/// Z axis lives at row0.z/row1.z/row2.z. Identity -> face direction `(0,0,-1)` -> yaw 0.
unsafe fn profile_model_matrix_facing_yaw(renderer: usize) -> Option<f32> {
    let read_f32 = |off: usize| -> Option<f32> {
        unsafe { safe_read_i32(renderer + off) }.map(|b| f32::from_bits(b as u32))
    };
    let zx = read_f32(PROFILE_RENDERER_MODEL_MATRIX_OFFSET + 0x8)?;
    let zz = read_f32(PROFILE_RENDERER_MODEL_MATRIX_OFFSET + 0x28)?;
    if !(zx.is_finite() && zz.is_finite()) {
        return None;
    }
    if zx * zx + zz * zz < 0.0001 {
        return None;
    }
    Some(zx.atan2(zz))
}

unsafe fn resolve_head_bone_index(skel: usize, count: usize) -> Option<usize> {
    let bones = unsafe { safe_read_usize(skel + HKA_SKELETON_BONES_DATA_OFFSET) }.unwrap_or(0);
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if bones == 0 || bones == null {
        return None;
    }
    for i in 0..count.min(LOOKAT_MAX_BONES) {
        let name_ptr =
            unsafe { safe_read_usize(bones + i * HKA_BONE_STRIDE + HKA_BONE_NAME_OFFSET) }?
                & !1usize;
        let Some(name) = (unsafe { read_bone_name(name_ptr) }) else {
            continue;
        };
        if name.eq_ignore_ascii_case(LOOKAT_BONE_HEAD) {
            return Some(i);
        }
    }
    None
}

/// Prefer the live Head bone's model-space quaternion when the model has already built: it captures the
/// actual face direction of the rendered pose (including any native idle/root orientation). Fall back to
/// the renderer model matrix while the skeleton is not live yet.
unsafe fn profile_model_facing_yaw(renderer: usize) -> Option<f32> {
    let holder = unsafe { profile_pose_holder(renderer) }?;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let skel = unsafe { safe_read_usize(holder + POSEHOLDER_SKELETON_OFFSET) }.unwrap_or(0);
    if !valid(skel) {
        return unsafe { profile_model_matrix_facing_yaw(renderer) };
    }
    let count = unsafe { safe_read_i32(skel + HKA_SKELETON_BONES_SIZE_OFFSET) }.unwrap_or(0);
    if count <= 0 || count as usize > LOOKAT_MAX_BONES {
        return unsafe { profile_model_matrix_facing_yaw(renderer) };
    }
    let head = match unsafe { resolve_head_bone_index(skel, count as usize) } {
        Some(idx) => idx,
        None => return unsafe { profile_model_matrix_facing_yaw(renderer) },
    };
    let model = unsafe { safe_read_usize(holder + POSEHOLDER_MODEL_BONE_DATA_OFFSET) }.unwrap_or(0);
    if !valid(model) {
        return unsafe { profile_model_matrix_facing_yaw(renderer) };
    }
    let Some(mut q) = (unsafe { read_quat(model + head * BONE_DATA_STRIDE + BONE_DATA_Q_OFFSET) })
    else {
        return unsafe { profile_model_matrix_facing_yaw(renderer) };
    };
    let len2 = q.iter().map(|v| v * v).sum::<f32>();
    if !(len2.is_finite() && len2 > 0.0001) {
        return unsafe { profile_model_matrix_facing_yaw(renderer) };
    }
    let inv_len = len2.sqrt().recip();
    for v in &mut q {
        *v *= inv_len;
    }
    // Elden Ring c0000 faces local -Z in the portrait renderer: identity pose + yaw 0 already shows the
    // character front-on. Convert the rotated face vector into the camera-orbit yaw whose target->camera
    // vector is `(-sin(yaw), 0, -cos(yaw))`.
    let face = quat_rotate_vec3(q, [0.0, 0.0, -1.0]);
    let xz2 = face[0] * face[0] + face[2] * face[2];
    if !(xz2.is_finite() && xz2 > 0.0001) {
        return unsafe { profile_model_matrix_facing_yaw(renderer) };
    }
    Some((-face[0]).atan2(-face[2]))
}

unsafe fn latched_profile_model_facing_yaw(renderer: usize, idx: usize) -> f32 {
    if idx >= TITLE_PROFILE_SLOT_COUNT {
        return 0.0;
    }
    {
        let guard = match PROFILE_CAM_FACE_YAW.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(yaw) = guard[idx] {
            return yaw;
        }
    }
    let Some(yaw) = (unsafe { profile_model_facing_yaw(renderer) }) else {
        return 0.0;
    };
    if !yaw.is_finite() {
        return 0.0;
    }
    let mut guard = match PROFILE_CAM_FACE_YAW.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let yaw = *guard[idx].get_or_insert(yaw);
    PROFILE_CAM_FACE_YAW_LATCHED_MASK.fetch_or(1usize << idx, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "profile-camera: latched model-facing yaw slot={idx} yaw_rad={yaw:.4}"
    ));
    yaw
}

/// CAMERA LEVER: override one profile renderer's orbit camera with a custom viewport (closer, model-facing
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
    // FACING: the engine baseline.yaw (latched from the engine's param-derived camera) ALREADY frames the
    // model FRONT-on -- the natural profile render shows the face. The detected model-facing yaw is the
    // model's intrinsic orientation, which is REDUNDANT with that baseline: adding it (here ~-π) orbits the
    // camera a further ~180deg to the BACK of the head (observed calib-6: facing latched -3.14, render = back
    // of head at every cursor position). So do NOT add it to the camera yaw; keep the detection for the
    // telemetry/log only. (If a future renderer's baseline does NOT face front, revisit -- but our own-built
    // renderer inherits the engine's front-facing param camera.)
    let _facing_yaw = unsafe { latched_profile_model_facing_yaw(renderer, idx) };
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

/// Reads `CSNowLoadingHelperImp::load_done` off the NowLoading singleton. WARNING (RE-corrected
/// 2026-07-02): despite the name this is a load-COMPLETE latch, not "loading screen visible" -- `Update`
/// copies it from `request_load_done` (raised by the map-load system), so it reads true AFTER the load
/// finishes and lingers into gameplay. Do NOT use it to decide the portrait overlay lifetime; kept for
/// telemetry/parity. Fault-guarded.
pub(crate) unsafe fn now_loading_active(base: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let helper = unsafe { safe_read_usize(base + RuntimeGlobalRva::NowLoadingSingleton as usize) }
        .unwrap_or(0);
    if helper == 0 || helper == null {
        return false;
    }
    let off = core::mem::offset_of!(CSNowLoadingHelperImp, load_done);
    unsafe { safe_read_usize(helper + off) }
        .map(|v| (v & 0xff) != 0)
        .unwrap_or(false)
}

/// Resolve the live `CSFakeLoadingScreenImp` (the render-pipeline cover plate) or 0. Singleton =
/// `*(base + FakeLoadingScreenSingleton)`. Fault-guarded.
pub(crate) unsafe fn fake_loading_screen_ptr(base: usize) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let helper =
        unsafe { safe_read_usize(base + RuntimeGlobalRva::FakeLoadingScreenSingleton as usize) }
            .unwrap_or(0);
    if helper == 0 || helper == null {
        0
    } else {
        helper
    }
}

/// True while the `CSFakeLoadingScreenImp` cover plate is VISIBLE: `visible` (+0x8) & 0xff. This is the
/// render-pipeline cover the game draws to HIDE the world teardown/rebuild during a map load. Fault-guarded.
pub(crate) unsafe fn fake_loading_screen_visible(base: usize) -> bool {
    let helper = unsafe { fake_loading_screen_ptr(base) };
    if helper == 0 {
        return false;
    }
    unsafe { safe_read_usize(helper + FAKE_LOADING_SCREEN_VISIBLE_OFFSET) }
        .map(|v| (v & 0xff) != 0)
        .unwrap_or(false)
}

/// The portrait build + draw pipeline must PAUSE only during ACTIVE GAMEPLAY -- the player has reached the
/// world AND the current load has COMPLETED (`load_done`, via now_loading_active) AND no loading cover is
/// up. It MUST re-engage for every subsequent loading screen (notably a System Quit -> Load Profile
/// character switch). The old gate was the bare `IN_WORLD_REACHED == YES` latch, which is set the first
/// time the player reaches the world and NEVER resets -> after the first load the build/draw ticks froze
/// forever, so the head only ever rendered on the FIRST character load (the subsequent-load bug, run
/// head-popfix-loaddone 2026-07-02: after the 2nd deserialize the whole pipeline was silent). Fault-guarded.
pub(crate) unsafe fn portrait_pipeline_idle_in_gameplay(base: usize) -> bool {
    // Also idle while the game's ProfileSelect (Load) menu is open: it renders its own portraits,
    // and our drive/readback stacking on top overflows the GX command queue (see the build gate in
    // maybe_build_profile_table_for_loading). Our pipeline is for the loading SCREEN, after the menu.
    if SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) != 0 {
        return true;
    }
    IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES
        && unsafe { now_loading_active(base) }
        && !unsafe { fake_loading_screen_visible(base) }
}

/// Count profile-table renderers that currently hold a LIVE character model (+0x778 valid). The game's
/// Load Profile menu builds all 10 (one per save), so this reads ~10 during the menu; our post-Continue
/// rebuild leaves only the loaded character's model live, so it reads 1 on the loading screen. The display
/// publish gates on `<= 1` to avoid reading back the wrong character while multiple models are live (the
/// subsequent-load cascade). Fault-guarded.
pub(crate) unsafe fn count_live_profile_models(base: usize) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    let mut n = 0usize;
    for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
        let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
        if valid(r)
            && unsafe { safe_read_usize(r) }.unwrap_or(0)
                == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                .map(|m| valid(m))
                .unwrap_or(false)
        {
            n += 1;
        }
    }
    n
}

/// EXPERIMENT (gated by `disable_loading_cover_enabled`): clamp the `CSFakeLoadingScreenImp` cover plate's
/// `visible` byte to 0 so the render pipeline skips drawing it -- exposing the world underneath during a
/// map load. Called every game-task frame; the map-load system raises `visible` once at load start and it
/// stays raised, so a per-frame write to 0 wins for the draw. Only writes when the byte is currently
/// non-zero (no needless writes), and only when a valid cover object is resolved. Reversible: with the gate
/// off this is never called and the game draws its cover normally. Counts writes into a RAM oracle so we
/// can confirm the clamp actually engaged. Fault-guarded (validated pointer + catch_unwind at the caller).
pub(crate) unsafe fn suppress_loading_cover_tick(base: usize) {
    if !disable_loading_cover_enabled() {
        return;
    }
    let helper = unsafe { fake_loading_screen_ptr(base) };
    if helper == 0 {
        return;
    }
    let vis_addr = helper + FAKE_LOADING_SCREEN_VISIBLE_OFFSET;
    let cur = unsafe { safe_read_u8(vis_addr) }.unwrap_or(0);
    if cur != 0 {
        unsafe { core::ptr::write_volatile(vis_addr as *mut u8, 0) };
        let n = LOADING_COVER_SUPPRESS_WRITES.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 4 {
            append_autoload_debug(format_args!(
                "loading-cover-experiment: cleared CSFakeLoadingScreenImp.visible (was {cur}) at 0x{vis_addr:x} (write #{n}) -- world drawn uncovered this frame"
            ));
        }
    }
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
    // ROOT FIX (2026-07-03, run gxguard2): do NOT build our portrait table while the game's own
    // ProfileSelect (Load Character) menu owns the portraits. That menu renders its own 10 profile
    // models; our builder adds a second 10 that stack in the SAME frame and blow past the fixed
    // 192-slot GX command queue -> reserve_command_queue_slot null-slot crash (0x1aeaf05) exactly
    // when the load menu appears after a few switches. Our table is only for the loading SCREEN,
    // which comes AFTER this menu closes -- so skip the build while it is open (owner != 0). The
    // now-loading flag briefly overlaps the still-open menu, which is why we were building too early.
    if SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) != 0 {
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
    // The loading-cover observer (CSNowLoadingHelperImp ctor/update) is the overlay's PRIMARY end-of-cover
    // signal (update pulses stop == the game dismissed the tips+bar screen). Install it here, at the start
    // of every loading window, instead of relying on the accept-byte-gated product path (which never fired
    // on the strip-default run -> hooks_installed=0 and the overlay had to lean on the in-world latch).
    install_now_loading_helper_observer_hooks();
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

/// Kick the ASYNC character-model build for ONE profile slot -- a faithful per-slot replica of the body
/// of the engine's global refresh (dump `FUN_1409aa7d0`), which we no longer call from the post-Continue
/// feed: the global form iterates all 10 slots and kicks every real+marked one, building EVERY save
/// character mid-load (the cross-slot portrait swap). Writing the +0x754/+0x755 latches on the other
/// renderers to mute the global refresh CRASHED (GX command-queue overflow; the latches only mean
/// "requested" on a CONFIGURED renderer). This replica performs the engine's exact per-slot sequence --
/// record lookup, ChrAsm/model-source config, FaceData copy, stream index, then the two request latches --
/// so the target slot builds exactly as the engine would build it, and the non-target renderers stay in
/// the natural never-configured state (flags 0, stepper idle -- the same state empty slots hold forever).
/// Returns true when the kick fired. Fault-guarded reads; skips when the slot was already requested.
pub(crate) unsafe fn kick_target_profile_slot(
    base: usize,
    summary: usize,
    renderer: usize,
    slot: i32,
) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    if !valid(summary) || !valid(renderer) || !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot)
    {
        return false;
    }
    // ONE KICK PER SLOT VALUE PER LOAD WINDOW (engine "refresh on profile-data change" semantics;
    // see PORTRAIT_KICK_SLOT_KEY). Re-kicking on a cadence poisoned the state machine (mid-pipeline
    // the model is dead + latches consumed, so the re-kick re-raised +0x754/+0x755 and Wait_Play
    // re-entered the rebuild state forever = the ~1/s rebuild storm, static portrait, shadow
    // flicker). But a blanket one-shot freezes the WRONG character: `portrait_loaded_slot()` (ac0)
    // can still hold the PREVIOUS session's slot when the first kick fires, and the storm's
    // accidental self-correction was the "swap to the actual character" the user always saw. Keying
    // the latch to the slot gives exactly one corrective kick when ac0 flips to the real slot --
    // a deterministic swap -- and no storm (the same slot never re-kicks). No live-model guard:
    // the corrective kick MUST fire on a live (wrong-record) model, exactly like the engine's
    // data-change refresh.
    if PORTRAIT_KICK_SLOT_KEY.load(Ordering::SeqCst) == (slot + 1) as usize
        && PORTRAIT_KICK_RENDERER.load(Ordering::SeqCst) == renderer
    {
        return false;
    }
    // Engine parity: kick only when BOTH request latches read 0 (a kick is not already in flight).
    if unsafe { safe_read_u8(renderer + 0x754) }.unwrap_or(1) != 0
        || unsafe { safe_read_u8(renderer + 0x755) }.unwrap_or(1) != 0
    {
        return false;
    }
    let record_of: unsafe extern "system" fn(usize, i32) -> usize =
        unsafe { core::mem::transmute(base + PROFILE_SUMMARY_RECORD_RVA) };
    let record = unsafe { record_of(summary, slot) };
    if !valid(record) {
        return false;
    }
    let set_model_source: unsafe extern "system" fn(usize, usize) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_MODEL_SOURCE_RVA) };
    let facedata_buffer: unsafe extern "system" fn(usize, u8) -> usize =
        unsafe { core::mem::transmute(base + PROFILE_FACEDATA_BUFFER_RVA) };
    let set_facedata: unsafe extern "system" fn(usize, usize) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_FACEDATA_RVA) };
    let set_byte290: unsafe extern "system" fn(usize, u8) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_BYTE290_RVA) };
    let set_flag_one: unsafe extern "system" fn(usize, u8) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_FLAG_ONE_RVA) };
    let set_byte294: unsafe extern "system" fn(usize, u8) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_BYTE294_RVA) };
    let set_stream_index: unsafe extern "system" fn(usize, u32) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_STREAM_INDEX_RVA) };
    let set_req_754: unsafe extern "system" fn(usize) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_REQ_754_RVA) };
    let set_req_755: unsafe extern "system" fn(usize) =
        unsafe { core::mem::transmute(base + PROFILE_RENDERER_SET_REQ_755_RVA) };
    let b290 = unsafe { safe_read_u8(record + 0x290) }.unwrap_or(0);
    let b294 = unsafe { safe_read_u8(record + 0x294) }.unwrap_or(0);
    // LATCH SEMANTICS (static RE 2026-07-03): the state machine is Wait_Request --754--> build
    // pipeline --> Wait_Play (live), and Wait_Play routes 755/756 to STEP_Finish_Play = a 6-tick
    // TEARDOWN (unregisters the offscreen scene, destroys the model, clears 755+756). So 754+755
    // together mean "tear down the CURRENT model, then rebuild" -- the engine's data-change
    // sequence for a LIVE renderer. On a renderer with NO model (our post-Continue case, machine
    // in Wait_Request) the 754 is consumed immediately and the still-armed 755 then DESTROYS the
    // freshly built model six ticks after it reaches Wait_Play, latches clear, dead forever (runs
    // #7/#8: 754 gone 96ms post-kick, ~9 live frames, rgba_version=1). Arm 755 only when there is
    // actually a model to tear down.
    let model_live =
        unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
    unsafe {
        set_model_source(renderer, record + 0x1a8);
        let fd = facedata_buffer(record + 0x38, 1);
        set_facedata(renderer, fd);
        set_byte290(renderer, b290);
        set_flag_one(renderer, 1);
        set_byte294(renderer, b294);
        set_stream_index(renderer, (slot as u32) * 2);
        set_req_754(renderer);
        if valid(model_live) {
            set_req_755(renderer);
        }
    }
    PORTRAIT_KICK_SLOT_KEY.store((slot + 1) as usize, Ordering::SeqCst);
    PORTRAIT_KICK_RENDERER.store(renderer, Ordering::SeqCst);
    let kicks = PROFILE_TARGET_KICKS.fetch_add(1, Ordering::SeqCst) + 1;
    if kicks <= 4 {
        append_autoload_debug(format_args!(
            "loading-portrait: per-slot build kick #{kicks} for LOADED slot {slot} (renderer=0x{renderer:x} record=0x{record:x}) -- global refresh not called, other slots stay unbuilt"
        ));
    }
    true
}

/// The save slot whose portrait the loading-screen pipeline should build / capture / display / spare:
/// the character the game ACTUALLY loaded (`GameMan.save_slot` = ac0), the single ground truth on a
/// boot most-recent Continue AND on our switch deserialize. Falls back to the autoload hint
/// `OWN_STEPPER_SLOT`, then 0, only pre-load when ac0 is not yet a valid slot. The raw
/// `OWN_STEPPER_SLOT` is `-1` on a most-recent boot (title.rs:113 returns early without setting it) and
/// collapsed to slot 0, so the pipeline built/captured slot 0's portrait for a non-slot-0 character
/// (wrong on load 1) and captured nothing once its gate stopped matching (blank on load 2). Routing
/// EVERY portrait site through this one loaded-character source is the er-effects-rs-j3r correlation fix.
pub(crate) fn portrait_loaded_slot() -> i32 {
    portrait_loaded_slot_confirmed().unwrap_or(0)
}

/// The loaded slot ONLY when a real source names it (ac0 or the autoload stepper hint) -- `None`
/// while neither is valid yet. The BUILD KICK must use this form: the old fallback-to-0 kicked a
/// SLOT-0 build ~340ms before ac0 flipped to the real slot (run anim-bind5, kicks #1 slot0 /
/// #2 slot5), and with the rebuild storm fixed that foreign model now PERSISTS -- the
/// `count_live_profile_models == 1` stability gate then blocks the whole live-drive/publish/anim
/// pipeline for the rest of the load (1 motion sample all window). Display-side readers may still
/// use the collapsed `portrait_loaded_slot()` form (with no model built, a wrong slot reads inert).
pub(crate) fn portrait_loaded_slot_confirmed() -> Option<i32> {
    let ac0 = (unsafe { eldenring::cs::GameMan::instance() })
        .map(|gm| er_save_loader::GameManSaveAccess::save_slot(gm))
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&ac0) {
        return Some(ac0);
    }
    let own = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&own) {
        return Some(own);
    }
    None
}

/// TORN-READBACK score: average absolute VERTICAL luma step across the masked (alpha != 0, i.e. head)
/// region of a readback RGBA frame. A clean face render varies smoothly row-to-row (small steps); a
/// torn readback (rows captured mid-GPU-write, no cross-queue sync) has random per-row discontinuities
/// (large steps -> the scanline garbage the user saw). Returns 0..255. Columns are subsampled by 2 for
/// cost; every row is compared so single-row tears still register. 0 when there is no masked content.
pub(crate) fn portrait_tear_score(cpx: &[u8], w: usize, h: usize) -> usize {
    if w < 2 || h < 2 || cpx.len() < w * h * 4 {
        return 0;
    }
    let luma = |i: usize| -> i32 {
        let p = i * 4;
        (cpx[p] as i32 * 30 + cpx[p + 1] as i32 * 59 + cpx[p + 2] as i32 * 11) / 100
    };
    let mut sum = 0u64;
    let mut n = 0u64;
    let mut y = 1;
    while y < h {
        let mut x = 0;
        while x < w {
            let i = y * w + x;
            // Only score head pixels (alpha != 0). The mask sets background alpha to 0, so a torn
            // frame's head region is where the scanline garbage shows.
            if cpx[i * 4 + 3] != 0 {
                let d = (luma(i) - luma((y - 1) * w + x)).unsigned_abs() as u64;
                sum += d;
                n += 1;
            }
            x += 2;
        }
        y += 1;
    }
    if n == 0 { 0 } else { (sum / n) as usize }
}

/// The slot whose portrait the loading-screen pipeline should TARGET (spare + render + display): the
/// character the user just SELECTED for a System->Quit->Load switch
/// (`SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT`, set at the confirm press -- known BEFORE the deserialize
/// flips ac0), falling back to `portrait_loaded_slot()` (ac0 / the boot autoload hint) when no switch
/// selection is pending. This is what lets the loading portrait show the NEWLY-selected character
/// during the pre-continue window instead of the still-resident old one: at the confirm the new slot's
/// renderer is already built + live in the ProfileSelect table, so we can spare/render IT, while ac0
/// still names the old character until the reload deserializes.
pub(crate) fn portrait_target_slot() -> i32 {
    let sel = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
    if sel <= i32::MAX as usize {
        let sel = sel as i32;
        if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&sel) {
            return sel;
        }
    }
    portrait_loaded_slot()
}

/// Fail-fast CHARACTER-IDENTITY semaphore for the loading-screen portrait (er-effects-rs-j3r; user
/// directive 2026-07-02: verify IN-GAME, from RAM identity -- NOT rendered pixels -- that the
/// character our portrait code renders is the one the game actually loaded). Two INDEPENDENT sources:
///   OUR side  = the ProfileSummary save RECORD of the slot our portrait targets (`render_target_slot`
///               = `portrait_loaded_slot()`): its stored character NAME + saved MAP (record+0x30).
///   GAME side = the LIVE loaded character: PlayerGameData NAME (`char_fingerprint`) + GameMan c30 map.
/// The save-record table and the in-world character live in distinct memory, so a wrong-slot render (or
/// a wrong-character load) makes them disagree -- NON-tautological even though our target derives from
/// ac0 (a slot index): this compares the CHARACTER stored in that slot against who is actually resident.
/// Determines "is it the expected slot" without any pixel readback (the user's constraint: pixels are
/// too slow / the wrong tool). On a mismatch (a real character is loaded but its NAME/MAP != our target
/// slot's record): record the oracle + a crash-log line and, on diagnostic runs, deliberately fault so
/// the regression STOPS THE RUN EARLY. Gated on a real loaded character AND a real record, so pre-load
/// transients and empty slots never fire.
unsafe fn portrait_render_slot_semaphore(base: usize, render_target_slot: i32) {
    // New-game / not-yet-resolved saved-map sentinel; excluded from the map check so a transient c30
    // during the loading screen cannot false-fire.
    const DEFAULT_MAP_C30: i32 = 0x0a01_0000;
    // ProfileSummary record layout (bd native-full-save-read-slot-resolve-chain-observe-recipe-2026):
    // records start at summary+0x18, stride 0x2a0; NAME at record+0, saved MAP at record+0x30.
    const PROFILE_RECORD_BASE: usize = 0x18;
    const PROFILE_RECORD_STRIDE: usize = 0x2a0;
    const PROFILE_RECORD_MAP_OFFSET: usize = 0x30;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;

    // GAME side: require a REAL loaded character before asserting anything.
    if !unsafe { char_fingerprint(base).0 } {
        return; // no real character loaded yet -- pre-load transient.
    }
    let gdm = game_data_man_ptr_or_null();
    if gdm == null {
        return;
    }
    let pgd =
        unsafe { safe_read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }.unwrap_or(null);
    if pgd == null {
        return;
    }
    let (live_name, live_len) = unsafe { read_utf16_name_units(pgd + PGD_NAME_9C_OFFSET) };
    let gm = game_man_ptr_or_null();
    let live_map = if gm != null {
        unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(-1)
    } else {
        -1
    };

    // OUR side: the save-RECORD identity of the slot our portrait code targets.
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&render_target_slot) {
        return;
    }
    let profile_summary =
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(null);
    if profile_summary == null {
        return;
    }
    let rec =
        profile_summary + PROFILE_RECORD_BASE + render_target_slot as usize * PROFILE_RECORD_STRIDE;
    let (our_name, our_len) = unsafe { read_utf16_name_units(rec) };
    if utf16_name_empty_like(&our_name, our_len) {
        return; // our target slot stores no real character -- nothing meaningful to compare.
    }
    let our_map = unsafe { safe_read_i32(rec + PROFILE_RECORD_MAP_OFFSET) }.unwrap_or(-1);

    // Compare RAM identities. Name is the character identity; the saved map is a second discriminator,
    // checked only when BOTH are real resolved maps (so a default/transient c30 can't false-fire).
    let name_match = our_len == live_len && our_name[..our_len] == live_name[..live_len];
    let both_real_map =
        our_map > 0 && our_map != DEFAULT_MAP_C30 && live_map > 0 && live_map != DEFAULT_MAP_C30;
    let map_mismatch = both_real_map && our_map != live_map;
    if name_match && !map_mismatch {
        return; // our portrait's character == the loaded character (RAM identity match).
    }
    let cond = ((!name_match) as usize) | ((map_mismatch as usize) << 1);
    PORTRAIT_RENDER_SEMAPHORE_STATE.store(
        ((render_target_slot as u32 as usize) << 16)
            | ((our_map as u32 as usize & 0xff) << 8)
            | cond,
        Ordering::SeqCst,
    );
    if PORTRAIT_RENDER_SEMAPHORE_LOGGED.swap(1, Ordering::SeqCst) == 0 {
        append_crash_log(format_args!(
            "PORTRAIT-IDENTITY-SEMAPHORE FAIL: our portrait targets slot={render_target_slot} (record name_len={our_len} map=0x{our_map:x}) but the LOADED character is name_len={live_len} map=0x{live_map:x} -- name_match={name_match} map_mismatch={map_mismatch}. Our portrait is not the loaded character (er-effects-rs-j3r); deliberate fail-fast fault follows"
        ));
        append_autoload_debug(format_args!(
            "PORTRAIT-IDENTITY-SEMAPHORE FAIL: target_slot={render_target_slot} record(name_len={our_len} map=0x{our_map:x}) vs loaded(name_len={live_len} map=0x{live_map:x}) name_match={name_match} map_mismatch={map_mismatch}"
        ));
    }
    if crate::crashlog::crash_logger_enabled() {
        // Deliberate null-page fault: crash_vectored_handler logs full context, returns
        // EXCEPTION_CONTINUE_SEARCH, and the run terminates -- the fail-fast the user asked for.
        unsafe {
            core::ptr::write_volatile(PORTRAIT_RENDER_SEMAPHORE_FAULT_ADDR as *mut u8, 0u8);
        }
    }
}

/// ProfileSummary save-record layout (bd native-full-save-read-slot-resolve-chain-observe-recipe):
/// per-slot records start at `summary+0x18`, stride `0x2a0`; character NAME at record+0.
const PROFILE_SUMMARY_RECORD_BASE: usize = 0x18;
const PROFILE_SUMMARY_RECORD_STRIDE: usize = 0x2a0;

/// True if ProfileSummary slot `slot` holds a real character (non-empty saved name). Used to gate the
/// human-driven in-world Load-Profile pick so activating an EMPTY slot never arms a switch (which
/// would tear the world down to a clean title and then fail the fresh deserialize, stranding the game
/// at a blank title). Reads the same save-record table the identity semaphore uses -- fault-guarded,
/// returns false on any unreadable pointer so an empty/unknown slot is treated as "no character".
unsafe fn profile_slot_has_character(slot: i32) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot) {
        return false;
    }
    let gdm = game_data_man_ptr_or_null();
    if gdm == null {
        return false;
    }
    let profile_summary =
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(null);
    if profile_summary == null {
        return false;
    }
    let rec = profile_summary
        + PROFILE_SUMMARY_RECORD_BASE
        + slot as usize * PROFILE_SUMMARY_RECORD_STRIDE;
    let (name, len) = unsafe { read_utf16_name_units(rec) };
    !utf16_name_empty_like(&name, len)
}

pub(crate) unsafe fn force_profile_render_tick(base: usize, _slot: i32) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == 0 || base == null {
        return;
    }
    // RE-ENGAGE on every loading screen (subsequent-character-load fix): pause the build pipeline ONLY
    // during active gameplay, not permanently after the first world -- so a System Quit character switch's
    // loading screen re-builds + re-captures the NEW character's portrait.
    if unsafe { portrait_pipeline_idle_in_gameplay(base) } {
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
    // SKIP on the portrait-lookat path: the live present-overlay owns the display there, so uploading into
    // the native forged texture would paint a SECOND head. Overlay-only (user choice 2026-06-30).
    if !portrait_lookat_enabled() {
        unsafe { maybe_reforge_loading_portrait(base) };
    }
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
    // SLOT->NAME dump, once per run (er-effects-rs-hi2 attribution): the anomaly hypothesis is
    // character-specific (Patches' boot/menu-path lifecycle differs on reload), so per-window
    // anomalies must be joinable to WHICH character each retarget slot holds -- readable here from
    // the ProfileSummary records the pipeline already uses.
    if PROFILE_SLOT_NAMES_DUMPED.load(Ordering::SeqCst) == 0 {
        // Only consume the one-shot once at least one REAL name is readable: this runs before the
        // boot ProfileSummary save read (~+16s), and latching on the pre-read table logged ten
        // "(empty)" slots (run 2026-07-03 ~21:14). Keep retrying until the records are populated.
        let mut names: Vec<String> = Vec::with_capacity(TITLE_PROFILE_SLOT_COUNT);
        let mut any_real = false;
        for s in 0..TITLE_PROFILE_SLOT_COUNT {
            let rec = summary + PROFILE_SUMMARY_RECORD_BASE + s * PROFILE_SUMMARY_RECORD_STRIDE;
            let (units, len) = unsafe { read_utf16_name_units(rec) };
            let name = if utf16_name_empty_like(&units, len) {
                "(empty)".to_owned()
            } else {
                any_real = true;
                String::from_utf16_lossy(&units[..len])
            };
            names.push(format!("{s}={name}"));
        }
        if any_real {
            PROFILE_SLOT_NAMES_DUMPED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!("profile-slot-names: {}", names.join(" ")));
        }
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
    // CORRELATION FIX (er-effects-rs-j3r): render the slot the game ACTUALLY loaded (ac0), via the
    // shared `portrait_loaded_slot*` source used by every portrait site (build/capture/spare).
    // CONFIRMED-ONLY (run anim-bind5, 2026-07-03): before ac0/stepper name the slot, do NOTHING --
    // the old fallback-to-0 kicked a foreign slot-0 build ~340ms early; storm-free that model
    // persisted and the single-model stability gate starved the drive/publish/anim pipeline for the
    // whole load. The lever loops below are no-ops with no model built, so skipping the tick is safe.
    let Some(target_slot) = portrait_loaded_slot_confirmed() else {
        return;
    };
    // FAIL-FAST SEMAPHORE: assert the slot we're about to render IS the loaded character
    // (er-effects-rs-j3r). With the correlation fix above, condition A (wrong-slot) is structurally
    // satisfied and stands as a regression tripwire; condition B (null loaded-slot renderer) stays a
    // live guard against the 3rd-open null-deref class.
    unsafe { portrait_render_slot_semaphore(base, target_slot) };
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
            // PER-SLOT kick replica (not the engine's GLOBAL refresh, which would kick EVERY marked
            // slot and build all the save's characters mid-load -> the cross-slot portrait swap).
            if unsafe { kick_target_profile_slot(base, summary, r, s) } {
                kicked += 1;
                kicked_mask |= 1 << s;
            }
        }
        if kicked > 0 {
            // Drive the freshly-requested build to completion + keep it latched through the loading screen.
            PROFILE_LOADSCREEN_FEED_TICKS
                .store(PROFILE_LOADSCREEN_FEED_WINDOW_TICKS, Ordering::SeqCst);
            if PROFILE_REAL_SLOT_KICK_LOGGED.swap(1, Ordering::SeqCst) == 0 {
                append_autoload_debug(format_args!(
                    "force-profile-render: IMMEDIATE build kick -- {kicked} real slot(s) (mask=0x{kicked_mask:x}) became available with req754=0; marked + per-slot kicked off-cadence + opened feed window (summary=0x{summary:x})"
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
            let r_valid = valid(r)
                && unsafe { safe_read_usize(r) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA;
            if force_rebuild && r_valid {
                unsafe {
                    core::ptr::write_volatile((r + 0x754) as *mut u8, 0);
                    core::ptr::write_volatile((r + 0x755) as *mut u8, 0);
                }
            }
            let _ = unsafe { mark(summary, s) };
            // PER-SLOT kick replica in place of the engine's GLOBAL refresh: the global form kicked
            // every marked slot (all the save's characters) -- the cross-slot portrait swap source.
            // Idempotent via the +0x754/+0x755 gate inside, so the feed cadence just re-tries until
            // the record is real and then no-ops.
            if r_valid {
                let _ = unsafe { kick_target_profile_slot(base, summary, r, s) };
            }
            marked += 1;
        }
        // TRIPWIRE oracle: count non-target renderers holding a live model during our feed window.
        // Expected 0 -- any foreign model on the loading screen is the swap-bug precondition.
        let mut foreign = 0usize;
        for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
            if s == target_slot {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if valid(r)
                && unsafe { safe_read_usize(r) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
                && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0)
                    != 0
            {
                foreign += 1;
            }
        }
        PROFILE_FOREIGN_MODELS_MAX.fetch_max(foreign, Ordering::SeqCst);
        if log_this {
            append_autoload_debug(format_args!(
                "force-profile-render: build cycle (counter={counter}) force_rebuild={force_rebuild} feed_window={feed_window} -- marked {marked} real slot(s) + per-slot kicked (summary=0x{summary:x} foreign_models={foreign})"
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
            match PROFILE_CAM_FACE_YAW.lock() {
                Ok(mut g) => *g = [None; 10],
                Err(p) => *p.into_inner() = [None; 10],
            }
            PROFILE_CAM_FACE_YAW_LATCHED_MASK.store(0, Ordering::SeqCst);
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
                // SPARE PRE-RECORD: capture the target slot's renderer as the spare candidate on a frame
                // where its model is actually BUILT (+0x778 valid), so the teardown-spare hook can protect
                // this exact renderer through Continue even though the menu cycles model_ins. Uses
                // portrait_target_slot() so that once the user confirms a switch (SELECTED_SLOT set), the
                // candidate re-records for the NEWLY-selected character, not the still-resident old ac0.
                let target = portrait_target_slot();
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
                if s == portrait_loaded_slot()
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
    // TEARDOWN FENCE (freeze relaxation, er-effects-rs-l1x): raise the fence BEFORE any
    // delete-enqueue below (both the orphan reclaim and the native table teardown in original()),
    // then wait out a render-thread pump caught mid-drive. The pump is one model update+draw
    // (sub-ms), so the 10ms cap is generous; a timeout is counted, not fatal -- worst case equals
    // the OLD per-frame TOCTOU exposure for exactly one frame instead of every frame. The fence is
    // lowered at the end of this hook, after the native teardown returns.
    PROFILE_RENDERER_TEARDOWN_FENCE.store(1, Ordering::SeqCst);
    if PROFILE_IN_OUR_DRIVE.load(Ordering::SeqCst) {
        PROFILE_TEARDOWN_FENCE_WAITS.fetch_add(1, Ordering::SeqCst);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(10);
        while PROFILE_IN_OUR_DRIVE.load(Ordering::SeqCst) {
            if std::time::Instant::now() > deadline {
                PROFILE_TEARDOWN_FENCE_TIMEOUTS.fetch_add(1, Ordering::SeqCst);
                break;
            }
            std::thread::yield_now();
        }
    }
    // REPEATED-SWITCH GX OVERFLOW FIX (0x1aeaf05, ~switch #4): destroy the PRIOR window's spared
    // renderer now, on the game thread, before sparing this switch's renderer. The load-complete
    // reset (render thread) moved it into PROFILE_SPARE_ORPHAN instead of dropping it; the spare
    // excluded it from the native delete (nulled its table slot), so without this it stayed alive
    // with its ResMan offscreen draw task filling the 192-slot GX command queue every frame,
    // accumulating +1 leaked renderer per switch. delay_delete_enqueue_renderer is the exact native
    // delete path (vtable-guarded), run here on the correct thread.
    let orphan = PROFILE_SPARE_ORPHAN.swap(0, Ordering::SeqCst);
    if orphan != 0 {
        let deleted = unsafe { delay_delete_enqueue_renderer(orphan) };
        // Ownership ledger: discharge our responsibility for the spared renderer (paired with the
        // ownership_take at the spare site). Released whether or not the enqueue took -- either we
        // handed it to delay-delete or it was already stale/gone; either way it is no longer ours.
        ownership_release(OwnedClass::SparedRenderer);
        append_autoload_debug(format_args!(
            "loading-portrait: reclaimed prior spared renderer 0x{orphan:x} via CSDelayDeleteMan enqueued={deleted} (repeated-switch GX command-queue leak fix)"
        ));
    }
    // Gate on the look-at/portrait feature OR product autoload -- the native-continue path does NOT set
    // PRODUCT_AUTOLOAD_ARMED, so gating on product_autoload alone never spared anything there.
    if LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst) == 0
        && (product_autoload_enabled() || portrait_lookat_enabled())
    {
        if let Ok(base) = game_module_base() {
            // The slot we render (er-effects-rs-j3r): the newly-selected character on a switch
            // (SELECTED_SLOT), else the loaded slot (ac0). portrait_target_slot() is what makes the
            // loading portrait show the character just picked, not the one still resident.
            let slot = portrait_target_slot();
            // Prefer the PRE-RECORDED candidate (captured at the menu on a model-built frame -- robust to
            // the menu's model_ins cycling). Find its table slot and protect it. Fall back to reading
            // table[slot] + a model-built guard if no candidate was recorded.
            let candidate = PROFILE_SPARE_CANDIDATE.load(Ordering::SeqCst);
            let target_te = portrait_renderer_table_entry(base, slot);
            // Honor the pre-recorded candidate ONLY if it still sits in the TARGET slot. A candidate
            // captured for the old character before a switch confirm must not be spared over the
            // newly-selected one -- in that case fall back to table[target] (its model is built, the
            // menu rendered all 10 slots). Prevents the loading portrait showing the prior character.
            let candidate_in_target =
                valid(candidate) && unsafe { safe_read_usize(target_te) }.unwrap_or(0) == candidate;
            let (renderer, table, spared_slot) = if candidate_in_target {
                (candidate, target_te, slot)
            } else {
                let r = unsafe { safe_read_usize(target_te) }.unwrap_or(0);
                let model_built = valid(r)
                    && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                        .map(|m| valid(m))
                        .unwrap_or(false);
                (if model_built { r } else { 0 }, target_te, slot)
            };
            if valid(renderer)
                && unsafe { safe_read_usize(renderer) }.unwrap_or(0)
                    == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
            {
                LOADING_BG_PORTRAIT_SPARED_RENDERER.store(renderer, Ordering::SeqCst);
                PROFILE_RENDERER_SPARE_HITS.fetch_add(1, Ordering::SeqCst);
                // Ownership ledger: we just excluded this renderer from the native delete, so WE own
                // its destruction now. Paired with the ownership_release on the drain path below.
                ownership_take(OwnedClass::SparedRenderer);
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
    // Native teardown done -- the table entries are delete-enqueued/nulled, so the next pump
    // invocation's per-frame table re-read + vtable probe fails closed until the new window's
    // rebuild. Safe to let the drive back in.
    PROFILE_RENDERER_TEARDOWN_FENCE.store(0, Ordering::SeqCst);
}

/// Diagnostic + REPAIR detour on the native profile-portrait builder (`FUN_1409aa7d0` =
/// `PROFILE_RENDERER_REFRESH_RVA`). The builder derefs `table[slot]+0x754` with NO null check for
/// every slot whose profile record exists (Ghidra: `FUN_140261c30(summary,slot) != 0` gates the
/// walk, the entry itself is never checked), and its 10-slot table setup is called from exactly ONE
/// native site -- the TitleTopDialog constructor -- so our cloned in-world ProfileSelect reopens run
/// it against whatever the last teardown left; the 3rd in-session open found the table fully empty
/// and AV'd at `[null+0x754]` (er-effects-rs-j3r). Three layers, all fault-guarded + catch_unwind:
///   1. DIAG: log the full table once per distinct degraded (mask, caller) pattern.
///   2. REPAIR: a FULLY-empty table (the proven crash state) is rebuilt via the engine's own no-arg
///      setup (`PROFILE_TABLE_BUILDER_RVA`; its internal teardown is a no-op on an all-null table),
///      satisfying the native invariant exactly as the TitleTopDialog ctor would. Gated on
///      `PROFILE_TABLE_WAS_POPULATED` (engine/ResMan up -- the setup AVs at boot title) and on
///      fully-empty ONLY: a MIXED table is the intentional teardown-spare state during Continue
///      loading and must not be rebuilt over.
///   3. GUARD: if any slot is still null/invalid after the (possible) repair, SKIP chaining the
///      original this call (fail-soft; the per-frame builder retries) instead of letting the native
///      walk AV.
pub(crate) unsafe extern "system" fn profile_select_table_diag_hook() {
    let chain = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(base) = game_module_base() else {
            return true;
        };
        let null = TITLE_OWNER_SCAN_START_ADDRESS;
        let scan_table = |ptrs: &mut [usize; TITLE_PROFILE_SLOT_COUNT]| -> (u32, u32) {
            let mut null_mask = 0u32;
            let mut valid_mask = 0u32;
            for s in 0..TITLE_PROFILE_SLOT_COUNT {
                let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s as i32)) }
                    .unwrap_or(0);
                ptrs[s] = r;
                let is_valid = r != 0
                    && r != null
                    && unsafe { safe_read_usize(r) }.unwrap_or(0)
                        == base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA;
                if is_valid {
                    valid_mask |= 1 << s;
                } else {
                    null_mask |= 1 << s;
                }
            }
            (valid_mask, null_mask)
        };
        let mut ptrs = [0usize; TITLE_PROFILE_SLOT_COUNT];
        let (valid_mask, mut null_mask) = scan_table(&mut ptrs);
        // Degraded = ANY slot lost its renderer while the builder is about to run. A HEALTHY table
        // is all 10 valid (native setup allocs all 10 unconditionally); any null is the crash-prone
        // state, INCLUDING all-null (the fully-empty table that caused the 3rd-open crash -- the
        // earlier "mixed only" check missed it). Log per distinct (mask, caller) so it never spams.
        let degraded = null_mask != 0;
        let caller_rva = crate::crashlog::trace_first_game_caller_rva();
        let key =
            ((caller_rva & 0xffffff) << 20) | ((valid_mask as usize) << 10) | null_mask as usize;
        if degraded && PROFILE_SELECT_TABLE_DIAG_LAST.swap(key, Ordering::SeqCst) != key {
            append_crash_log(format_args!(
                "PROFILESELECT-TABLE-DIAG: degraded profile-renderer table before native builder (er-effects-rs-j3r) caller_rva=0x{caller_rva:x} valid_mask=0x{valid_mask:x} null_mask=0x{null_mask:x} entries=[0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x}]",
                ptrs[0], ptrs[1], ptrs[2], ptrs[3], ptrs[4], ptrs[5], ptrs[6], ptrs[7], ptrs[8],
                ptrs[9]
            ));
        } else if !degraded {
            PROFILE_SELECT_TABLE_DIAG_LAST.store(0, Ordering::SeqCst);
            // A fully-valid table at builder entry proves the engine built renderers successfully --
            // the same "engine/ResMan up" evidence the loading-screen path latches; latching it here
            // too arms the repair even when the loading-portrait feature is disabled.
            PROFILE_TABLE_WAS_POPULATED.store(1, Ordering::SeqCst);
        }
        if null_mask == PROFILE_TABLE_ALL_SLOTS_MASK
            && PROFILE_TABLE_WAS_POPULATED.load(Ordering::SeqCst) != 0
        {
            let build: unsafe extern "system" fn() =
                unsafe { core::mem::transmute(base + PROFILE_TABLE_BUILDER_RVA) };
            unsafe { build() };
            let n = PROFILE_SELECT_TABLE_REPAIR_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            let (revalid_mask, renull_mask) = scan_table(&mut ptrs);
            null_mask = renull_mask;
            append_crash_log(format_args!(
                "PROFILESELECT-TABLE-REPAIR #{n}: fully-empty renderer table at native builder entry -> re-ran native table setup 0x{:x}; post-repair valid_mask=0x{revalid_mask:x} null_mask=0x{renull_mask:x} (er-effects-rs-j3r)",
                base + PROFILE_TABLE_BUILDER_RVA
            ));
            append_autoload_debug(format_args!(
                "profileselect-table-repair #{n}: rebuilt empty 10-slot renderer table via native setup before the native builder walked it; post-repair valid_mask=0x{revalid_mask:x} (er-effects-rs-j3r)"
            ));
        }
        if null_mask != 0 {
            let n = PROFILE_SELECT_TABLE_GUARD_SKIP_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            let skip_key = ((caller_rva & 0xffffff) << 10) | null_mask as usize;
            if PROFILE_SELECT_TABLE_GUARD_SKIP_LAST.swap(skip_key, Ordering::SeqCst) != skip_key {
                append_crash_log(format_args!(
                    "PROFILESELECT-TABLE-GUARD SKIP #{n}: null/invalid renderer slots remain (null_mask=0x{null_mask:x}) -- skipping the native builder this call so it cannot AV at [null+0x754] (er-effects-rs-j3r)"
                ));
            }
            return false;
        }
        true
    }))
    // A panicked diagnostic keeps the pre-hook behavior: chain the original.
    .unwrap_or(true);
    if !chain {
        return;
    }
    let orig = PROFILE_SELECT_TABLE_DIAG_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return;
    }
    let f: unsafe extern "system" fn() = unsafe { std::mem::transmute(orig) };
    unsafe { f() };
}

pub(crate) fn install_profile_select_table_diag_hook() {
    if PROFILE_SELECT_TABLE_DIAG_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "profileselect-table-diag: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(target) = game_rva(PROFILE_RENDERER_REFRESH_RVA as u32) else {
        return;
    };
    match unsafe {
        MhHook::new(
            target as *mut c_void,
            profile_select_table_diag_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            PROFILE_SELECT_TABLE_DIAG_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_err() {
                append_autoload_debug(format_args!(
                    "profileselect-table-diag: queue_enable failed for 0x{target:x}"
                ));
                return;
            }
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "profileselect-table-diag: MhHook::new failed: {status:?}"
            ));
            return;
        }
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            PROFILE_SELECT_TABLE_DIAG_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "profileselect-table-diag: hooked native profile builder 0x{target:x} (read-only table-state trace)"
            ));
        }
        status => append_autoload_debug(format_args!(
            "profileselect-table-diag: MH_ApplyQueued failed: {status:?}"
        )),
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
/// THE SWAPPABLE LOADING-BACKGROUND LEVER (retained, NOT wired in by default). Building the TPF served here
/// replaces the game's `MENU_Load_*` now-loading background artwork. Currently a fully TRANSPARENT (RGBA
/// 0,0,0,0) 64x64 texture (Scaleform stretches it to fill; for a real image build at the native ~1024x1024
/// so it is not upscaled). PROVEN 2026-07-02 that the 3D world is NOT rendered during a map load, so a
/// transparent background reads BLACK, not passthrough. This is kept for when we deliberately want to
/// replace the loading background: call it from build_portrait_tpf on the desired path and install the forge
/// there. By default the stock artwork is left enabled (user choice).
#[allow(dead_code)]
fn build_loading_bg_replacement_tpf(symbol: &str) -> Option<Vec<u8>> {
    let dds = er_tpf::DdsImage {
        width: 64,
        height: 64,
        pixels: vec![0u8; 64 * 64 * 4],
    }
    .to_dds_bytes_with(er_tpf::DdsHeaderMode::LegacyRgba8);
    er_tpf::Tpf::single_pc(symbol, dds, 1).build().ok()
}

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

/// Product-default 05_000_title strip WITHOUT embedded bytes (er-effects-rs-h7x). `file` is what
/// the native FileOpener just returned for `data0:/menu/05_000_title.gfx`; per the rescap static
/// RE (`FUN_140ce8320`, bd `native-memoryfile-wrapper-expects-gfx-rescap-2026-06-28`) that is a
/// Scaleform MemoryFile whose data/len fields point at the vanilla movie payload owned by
/// `GLOBAL_GfxRepository` (the file object never frees the payload -- the proven synthetic
/// construct path already relied on that). Derive the stripped movie from that payload with
/// `er_gfx::title_05_000::strip` (all-or-nothing content-addressed edits, output verified against
/// the validated-asset fingerprint for the known vanilla input), cache it for the process
/// lifetime, and swap the native file's data/len/cursor onto the cached buffer. ANY failure
/// leaves the native file untouched and returns it as-is: fail-closed to the vanilla title UI,
/// never a crash, never a half-stripped movie.
unsafe fn title_05_000_swap_to_stripped(base: usize, file: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if file == 0 || file == null || file == HOOK_ORIGINAL_UNSET {
        return false;
    }
    let fail = |reason: core::fmt::Arguments<'_>| {
        TITLE_05_000_RUNTIME_STRIP_FAILURES.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "title-resource-observer: 05_000 runtime strip FAIL-CLOSED (serving native vanilla): {reason}"
        ));
        false
    };
    let vtable = unsafe { safe_read_usize(file) }.unwrap_or(0);
    if vtable != base + SCALEFORM_MEMORY_FILE_VTABLE_RVA {
        return fail(format_args!(
            "unexpected file vtable 0x{vtable:x} (want MemoryFile 0x{:x})",
            base + SCALEFORM_MEMORY_FILE_VTABLE_RVA
        ));
    }
    let stripped = match TITLE_05_000_RUNTIME_STRIPPED.get() {
        Some(cached) => cached,
        None => {
            let data =
                unsafe { safe_read_usize(file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) }.unwrap_or(0);
            let len =
                unsafe { safe_read_i32(file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) }.unwrap_or(0);
            if data == 0 || data == null || !(64..=0x0100_0000).contains(&len) {
                return fail(format_args!(
                    "implausible payload data=0x{data:x} len={len}"
                ));
            }
            let len = len as usize;
            // Probe both ends through the guarded reader before the bulk copy; the payload is one
            // contiguous repository allocation, so readable ends imply a readable middle.
            let magic_ok = unsafe { safe_read_u8(data) } == Some(b'G')
                && unsafe { safe_read_u8(data + 1) } == Some(b'F')
                && unsafe { safe_read_u8(data + 2) } == Some(b'X')
                && unsafe { safe_read_u8(data + len - 1) }.is_some();
            if !magic_ok {
                return fail(format_args!(
                    "payload at 0x{data:x} len={len} is unreadable or not GFX-magic"
                ));
            }
            let vanilla = unsafe { core::slice::from_raw_parts(data as *const u8, len) };
            TITLE_05_000_RUNTIME_STRIP_INPUT_LEN.store(len, Ordering::SeqCst);
            let known = er_gfx::title_05_000::is_known_vanilla(vanilla);
            TITLE_05_000_RUNTIME_STRIP_INPUT_CLASS
                .store(if known { 1 } else { 2 }, Ordering::SeqCst);
            match er_gfx::title_05_000::strip(vanilla) {
                Ok(out) => {
                    TITLE_05_000_RUNTIME_STRIP_OUTPUT_LEN.store(out.len(), Ordering::SeqCst);
                    let validated = out.len() == er_gfx::title_05_000::STRIPPED_LEN
                        && er_gfx::title_05_000::fnv1a64(&out)
                            == er_gfx::title_05_000::STRIPPED_FNV1A64;
                    TITLE_05_000_RUNTIME_STRIP_OUTPUT_VALIDATED
                        .store(if validated { 1 } else { 2 }, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "title-resource-observer: 05_000 runtime strip derived in={len} out={} known_vanilla={known} out_fnv=0x{:016x}",
                        out.len(),
                        er_gfx::title_05_000::fnv1a64(&out)
                    ));
                    TITLE_05_000_RUNTIME_STRIPPED.get_or_init(|| out)
                }
                Err(err) => {
                    return fail(format_args!("in={len} known_vanilla={known}: {err}"));
                }
            }
        }
    };
    unsafe {
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_DATA_OFFSET) as *mut usize,
            stripped.as_ptr() as usize,
        );
        core::ptr::write(
            (file + SCALEFORM_MEMORY_FILE_LEN_OFFSET) as *mut u32,
            stripped.len() as u32,
        );
        core::ptr::write((file + SCALEFORM_MEMORY_FILE_CURSOR_OFFSET) as *mut u32, 0);
    }
    TITLE_05_000_RUNTIME_STRIP_SERVES.fetch_add(1, Ordering::SeqCst);
    // Keep the established product-strip oracles counting regardless of mechanism (the
    // construct-from-embedded path incremented both of these).
    TITLE_SCALEFORM_MEMORY_GFX_REPLACEMENTS.fetch_add(1, Ordering::SeqCst);
    TITLE_SCALEFORM_05_000_MEMORY_GFX_REPLACEMENTS.fetch_add(1, Ordering::SeqCst);
    TITLE_SCALEFORM_MEMORY_GFX_LAST_FILE.store(file, Ordering::SeqCst);
    true
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
                    if is_title_05_000 {
                        TITLE_SCALEFORM_05_000_MEMORY_GFX_REPLACEMENTS
                            .fetch_add(1, Ordering::SeqCst);
                    }
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
            let native = unsafe { f(loader, url, flags) };
            // Product-default runtime strip (er-effects-rs-h7x): derive the stripped title
            // movie from the native file's own vanilla payload and swap it in place. On any
            // failure the untouched native file is returned (vanilla title UI, fail-closed).
            if is_title_05_000 && TITLE_05_000_RUNTIME_STRIP_ARMED.load(Ordering::SeqCst) != 0 {
                memory_replacement = unsafe { title_05_000_swap_to_stripped(base, native) };
            }
            native
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

#[repr(C, align(8))]
struct SystemQuitMenuHelpLabelScratch {
    bytes: [u8; MENU_HELP_LABEL_SIZE],
}

#[repr(C, align(8))]
struct SystemQuitRootProxyScratch {
    bytes: [u8; MENU_WINDOW_ROOT_PROXY_SCRATCH_SIZE],
}

fn system_quit_list_slot_addr(list: usize, slot: usize) -> usize {
    list.wrapping_add((0usize.wrapping_sub(list)) & 7)
        .wrapping_add(slot * std::mem::size_of::<usize>())
}

unsafe fn system_quit_menu_window_set_visible_and_flags(
    base: usize,
    window: usize,
    visible: bool,
    source: &str,
) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    if window < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window visibility skipped -- window=0x{window:x} not heap-like"
        ));
        return false;
    }
    let window_vt = unsafe { safe_read_usize(window) }.unwrap_or(NULL);
    if window_vt < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window visibility skipped -- window=0x{window:x} vt=0x{window_vt:x} invalid"
        ));
        return false;
    }
    let mut scratch = SystemQuitRootProxyScratch {
        bytes: [0; MENU_WINDOW_ROOT_PROXY_SCRATCH_SIZE],
    };
    let Ok(root_proxy_ctor_addr) = game_rva(MENU_WINDOW_ROOT_PROXY_CTOR_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window visibility skipped -- failed to resolve root proxy ctor rva 0x{MENU_WINDOW_ROOT_PROXY_CTOR_RVA:x}"
        ));
        return false;
    };
    let Ok(set_visible_addr) = game_rva(TITLE_PRESS_START_SET_VISIBLE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window visibility skipped -- failed to resolve SetVisible rva 0x{TITLE_PRESS_START_SET_VISIBLE_RVA:x}"
        ));
        return false;
    };
    let Ok(dtor_addr) = game_rva(MENU_WINDOW_ROOT_PROXY_SCRATCH_DTOR_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window visibility skipped -- failed to resolve root proxy scratch dtor rva 0x{MENU_WINDOW_ROOT_PROXY_SCRATCH_DTOR_RVA:x}"
        ));
        return false;
    };
    let root_proxy_ctor: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(root_proxy_ctor_addr) };
    let set_visible: unsafe extern "system" fn(usize, u8) =
        unsafe { std::mem::transmute(set_visible_addr) };
    let dtor: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(dtor_addr) };
    let scratch_ptr = scratch.bytes.as_mut_ptr() as usize;
    let root_proxy = unsafe { root_proxy_ctor(window, scratch_ptr) };
    if root_proxy != scratch_ptr {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window root-proxy ctor returned unexpected 0x{root_proxy:x} scratch=0x{scratch_ptr:x}; still using returned proxy"
        ));
    }
    unsafe { set_visible(root_proxy, u8::from(visible)) };
    unsafe { dtor(scratch_ptr + 0x28) };

    let menu_id = unsafe { safe_read_u16(window + 0x180) }.unwrap_or(u16::MAX);
    let cs_menu_man = unsafe { safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }.unwrap_or(NULL);
    let mut flags_before = NULL;
    let mut flags_after = NULL;
    if menu_id < 0x47 && cs_menu_man >= HEAP_LO {
        let flags_addr = cs_menu_man + 0x90 + menu_id as usize;
        if let Some(flags) = unsafe { safe_read_u8(flags_addr) } {
            flags_before = flags as usize;
            let new_flags = if visible {
                flags | TITLE_NATIVE_MENU_VISUAL_VISIBLE_FLAGS_MASK
            } else {
                flags & 1
            };
            unsafe { (flags_addr as *mut u8).write_volatile(new_flags) };
            flags_after = new_flags as usize;
        }
    }
    append_autoload_debug(format_args!(
        "system-quit-dup: {source} top-window visibility window=0x{window:x} vt=0x{window_vt:x} visible={visible} root_proxy=0x{root_proxy:x} menu_id=0x{menu_id:x} flags=0x{flags_before:x}->0x{flags_after:x}"
    ));
    true
}

fn system_quit_read_wide_resource_name(ptr: usize) -> String {
    const MAX_UNITS: usize = 64;
    if ptr < 0x10000 {
        return String::new();
    }
    let mut units = Vec::new();
    for idx in 0..MAX_UNITS {
        let unit = unsafe { safe_read_u16(ptr + idx * 2) }.unwrap_or(0);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    String::from_utf16_lossy(&units)
}

unsafe fn system_quit_hide_real_system_windows(base: usize, source: &str) {
    let top = SYSTEM_QUIT_INGAME_TOP_WINDOW.load(Ordering::SeqCst);
    let option = SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst);
    let profile = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    if profile == 0 || SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) != 0 {
        return;
    }
    let hid_top = if top != 0 && top != profile {
        unsafe { system_quit_menu_window_set_visible_and_flags(base, top, false, source) }
    } else {
        false
    };
    let hid_option = if option != 0 && option != profile && option != top {
        unsafe { system_quit_menu_window_set_visible_and_flags(base, option, false, source) }
    } else {
        false
    };
    if hid_top || hid_option {
        SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.store(1, Ordering::SeqCst);
        SYSTEM_QUIT_HIDE_REAL_WINDOWS_COUNT.fetch_add(1, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "system-quit-dup: real-system-window hide source={source} top=0x{top:x} option=0x{option:x} profile=0x{profile:x} hid_top={hid_top} hid_option={hid_option}"
    ));
}

unsafe fn system_quit_reset_profile_select_state(source: &str) {
    SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_SELECT_WINDOW.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_FIRED.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_TOP_WINDOW.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_PROFILE_WINDOW.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_LIST.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_TOP_MENU_ID.store(usize::MAX, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: reset ProfileSelect hide state source={source}"
    ));
}

pub(crate) unsafe fn system_quit_submit_direct_return_title_chain(
    base: usize,
    system_dialog: usize,
    source: &str,
) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    if SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.load(Ordering::SeqCst) != 0 {
        return true;
    }
    if system_dialog < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-quickload: direct return-title chain abort source={source} -- system_dialog=0x{system_dialog:x} not heap-like"
        ));
        return false;
    }
    let queue = system_dialog + 0x10;
    let list = system_dialog + 0x50;
    SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_LAST_DIALOG.store(system_dialog, Ordering::SeqCst);
    let Ok(ready_addr) = game_rva(MENU_JOB_QUEUE_READY_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: direct return-title chain abort source={source} -- queue-ready rva 0x{MENU_JOB_QUEUE_READY_RVA:x} unresolved"
        ));
        return false;
    };
    let ready_fn: unsafe extern "system" fn(usize) -> u8 =
        unsafe { std::mem::transmute(ready_addr) };
    let queue_ready = unsafe { ready_fn(queue) } != 0;
    SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_LAST_QUEUE_READY
        .store(queue_ready as usize, Ordering::SeqCst);
    if !queue_ready {
        let waits = SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_READY_BLOCK_COUNT
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        if waits <= 3 || waits % 60 == 0 {
            let head = unsafe { safe_read_usize(queue) }.unwrap_or(NULL);
            let pending6 = unsafe { safe_read_usize(queue + 0x30) }.unwrap_or(NULL);
            append_autoload_debug(format_args!(
                "system-quit-quickload: direct return-title chain WAIT source={source} waits={waits} queue not ready dialog=0x{system_dialog:x} queue=0x{queue:x} head=0x{head:x} field6=0x{pending6:x}"
            ));
        }
        return false;
    }
    // Fire the NATIVE return-title REQUEST (FUN_14067a490, live 0x67a3a0) -- the missing piece. It sets
    // GameMan.saveRequested = true and GameMan+0xbc4 = 1 (== GAME_MAN_RETURN_TITLE_JOB_PREDICATE_READY).
    // WITHOUT it, bc4 stays 0, so (a) the game never recognizes a return-to-title is pending and never
    // saves+tears down the world, and (b) our final functor (title.rs, gated on bc4==READY) never fires,
    // leaving the submitted chain job orphaned in a queue that stops being pumped once the menus close.
    // Observed 2026-07-01: OK -> menus closed but still in-world, same char, functor_call_count=0,
    // bc4=0, native_quit_action_count=0. The native Quit-Game does this request AND the build+submit
    // below; we were doing only the build+submit. It is a plain GameMan field write (+ FUN_14080dd00),
    // safe to call from this menu-pump-owned path. Fire once. See bd
    // system-quit-loadjob-success-commits-phantom-load-2026-07-01.
    if SYSTEM_QUIT_QUICKLOAD_RETURN_TITLE_REQUEST_COUNT.load(Ordering::SeqCst) == 0 {
        match game_rva(SYSTEM_QUIT_RETURN_TITLE_REQUEST_RVA) {
            Ok(req_addr) => {
                let request_fn: unsafe extern "system" fn() =
                    unsafe { std::mem::transmute(req_addr) };
                unsafe { request_fn() };
                SYSTEM_QUIT_QUICKLOAD_RETURN_TITLE_REQUEST_COUNT.fetch_add(1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-quickload: native return-title REQUEST fired 0x{req_addr:x} source={source} -- set saveRequested + bc4=1 so the world saves+tears down and the final functor can fire"
                ));
            }
            Err(_) => append_autoload_debug(format_args!(
                "system-quit-quickload: return-title request rva 0x{SYSTEM_QUIT_RETURN_TITLE_REQUEST_RVA:x} unresolved source={source}"
            )),
        }
    }
    let Ok(builder_addr) = game_rva(SYSTEM_QUIT_RETURN_TITLE_CHAIN_BUILDER_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: direct return-title chain abort source={source} -- builder rva 0x{SYSTEM_QUIT_RETURN_TITLE_CHAIN_BUILDER_RVA:x} unresolved"
        ));
        return false;
    };
    let Ok(submit_addr) = game_rva(MENU_JOB_SUBMIT_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: direct return-title chain abort source={source} -- submit rva 0x{MENU_JOB_SUBMIT_RVA:x} unresolved"
        ));
        return false;
    };
    let builder: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(builder_addr) };
    let submit: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(submit_addr) };
    let mut job_slot: usize = 0;
    let job_slot_ptr = (&raw mut job_slot) as usize;
    unsafe { builder(job_slot_ptr, list) };
    let job = job_slot;
    if job < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-quickload: direct return-title chain builder produced no plausible job source={source} dialog=0x{system_dialog:x} list=0x{list:x} job=0x{job:x}"
        ));
        return false;
    }
    SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-quickload: direct return-title chain SUBMIT source={source} builder=0x{builder_addr:x} submit=0x{submit_addr:x} dialog=0x{system_dialog:x} queue=0x{queue:x} list=0x{list:x} job=0x{job:x}; waiting for real title menu rebuild before Continue fallback"
    ));
    unsafe { submit(queue, job_slot_ptr) };
    true
}

unsafe fn system_quit_restore_real_system_windows(base: usize, source: &str) {
    if SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) == 0 {
        unsafe { system_quit_reset_profile_select_state(source) };
        return;
    }
    let top = SYSTEM_QUIT_INGAME_TOP_WINDOW.load(Ordering::SeqCst);
    let option = SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst);
    let profile = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if phase != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE {
        let system_dialog = SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.load(Ordering::SeqCst);
        let submitted =
            unsafe { system_quit_submit_direct_return_title_chain(base, system_dialog, source) };
        SYSTEM_QUIT_SKIP_RESTORE_AFTER_QUICKLOAD_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-dup: skip restore real windows after quickload handoff source={source} phase={phase} profile=0x{profile:x} top=0x{top:x} option=0x{option:x} direct_chain_submitted={submitted}; leaving old System UI hidden during native transition"
        ));
        if submitted {
            SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.store(0, Ordering::SeqCst);
            unsafe { system_quit_reset_profile_select_state(source) };
        }
        return;
    }
    let restored_top = if top != 0 {
        unsafe { system_quit_menu_window_set_visible_and_flags(base, top, true, source) }
    } else {
        false
    };
    let restored_option = if option != 0 && option != top {
        unsafe { system_quit_menu_window_set_visible_and_flags(base, option, true, source) }
    } else {
        false
    };
    append_autoload_debug(format_args!(
        "system-quit-dup: restore real windows source={source} profile=0x{profile:x} top=0x{top:x} option=0x{option:x} restored_top={restored_top} restored_option={restored_option}"
    ));
    unsafe { system_quit_reset_profile_select_state(source) };
    if restored_top || restored_option {
        SYSTEM_QUIT_RESTORE_REAL_WINDOWS_COUNT.fetch_add(1, Ordering::SeqCst);
    }
}

pub(crate) unsafe fn system_quit_profile_select_top_menu_tick() {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let hidden = SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) != 0;
    let profile = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    if !hidden {
        return;
    }
    if profile == 0 {
        // ProfileSelect has closed. Do NOT submit the return-title chain from this game-task tick:
        // that runs concurrently with the game's own menu/Scaleform pump and corrupts it (observed:
        // non-deterministic execute-fault jumping into Scaleform string data). The close is done in
        // menu-pump ownership by the native confirm transition (dialog+0x1e8=Success pops the
        // ProfileSelect window job) and the return-title submit is done in menu-pump ownership from
        // the MenuWindowJob::Run hook. See bd system-quit-return-title-scaleform-race-2026-07-01.
        return;
    }
    let list = SYSTEM_QUIT_TOP_HIDE_LIST.load(Ordering::SeqCst);
    if list == 0 {
        return;
    }
    let count = unsafe { safe_read_usize(list + 0x48) }.unwrap_or(0);
    let still_present = (0..count.min(8)).any(|idx| {
        unsafe { safe_read_usize(system_quit_list_slot_addr(list, idx)) }.unwrap_or(NULL) == profile
    });
    if still_present {
        return;
    }
    if let Ok(base) = game_module_base() {
        unsafe { system_quit_restore_real_system_windows(base, "restore-real-profile-left-list") };
    } else {
        unsafe { system_quit_reset_profile_select_state("restore-real-profile-left-list-no-base") };
    }
}

pub(crate) unsafe extern "system" fn system_quit_menu_window_job_run_hook(
    job: usize,
    load_params: usize,
    fd4_time: usize,
    menu_man: usize,
) -> usize {
    let orig = SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return load_params;
    }
    let filename_ptr = unsafe { safe_read_usize(job + 0x60) }.unwrap_or(0);
    let filename = system_quit_read_wide_resource_name(filename_ptr);
    let original: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    let ret = unsafe { original(job, load_params, fd4_time, menu_man) };
    if matches!(
        filename.as_str(),
        "02_000_IngameTop"
            | "02_040_OptionSetting"
            | "02_041_OptionSetting_Trial"
            | "05_010_ProfileSelect"
    ) {
        let owner = unsafe { safe_read_usize(job + 0x130) }.unwrap_or(0);
        let owner_vt = if owner != 0 {
            unsafe { safe_read_usize(owner) }.unwrap_or(0)
        } else {
            0
        };
        let owner_id = if owner != 0 {
            unsafe { safe_read_u16(owner + 0x180) }.unwrap_or(u16::MAX)
        } else {
            u16::MAX
        };
        let list = unsafe { safe_read_usize(job + 0x50) }.unwrap_or(0);
        let prev = match filename.as_str() {
            "02_000_IngameTop" => SYSTEM_QUIT_INGAME_TOP_WINDOW.swap(owner, Ordering::SeqCst),
            "02_040_OptionSetting" | "02_041_OptionSetting_Trial" => {
                SYSTEM_QUIT_OPTION_SETTING_WINDOW.swap(owner, Ordering::SeqCst)
            }
            "05_010_ProfileSelect" => {
                SYSTEM_QUIT_PROFILE_SELECT_WINDOW.swap(owner, Ordering::SeqCst)
            }
            _ => 0,
        };
        let log_idx = SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_LOG_COUNT.fetch_add(1, Ordering::SeqCst);
        if log_idx < 64 || filename == "05_010_ProfileSelect" {
            append_autoload_debug(format_args!(
                "system-quit-dup: MenuWindowJob::Run resource='{filename}' job=0x{job:x} owner=0x{owner:x} owner_vt=0x{owner_vt:x} owner_id=0x{owner_id:x} prev=0x{prev:x} list_field=0x{list:x} ret=0x{ret:x}"
            ));
        }
        if filename == "05_010_ProfileSelect" {
            if let Ok(base) = game_module_base() {
                if owner == 0 {
                    unsafe {
                        system_quit_restore_real_system_windows(
                            base,
                            "restore-real-profile-owner-cleared",
                        )
                    };
                } else {
                    unsafe {
                        system_quit_hide_real_system_windows(
                            base,
                            "hide-real-after-profile-select-run",
                        )
                    };
                }
            }
        }
    }
    // ABORT the half-started in-world load transition. Pressing OK on ProfileSelect natively arms
    // GameMan.saveState/b80=2 (in-world load via deserialize 0x67b290) BEFORE any hook we control; our
    // load guard skips the deserialize so nothing loads, but the game still advances to saveState=3
    // ("loading") and STICKS at a loading screen -- and that stuck load blocks the game/menu pump from
    // running the queued return-title chain (observed: functor_call_count=0, player still present).
    // While the FIRST-world System-Quit transition is active AND the old world is still up (local
    // player present), force saveState back to idle (0) so the load machine stops and the return-title
    // can run. RANGE-gated on [CONFIRMED, AUTOLOAD_HANDOFF) -- NOT `!= IDLE`: the clean-title reload runs
    // at AUTOLOAD_HANDOFF, and its OWN deserialize allocates a NEW PlayerIns so `local_player_mut()`
    // flips back to Ok (world_up=true). A `!= IDLE` gate would REOPEN here and zero the RELOAD's own
    // saveState=2/3 mid-deserialize, yanking the load out from under a half-built FE/player -> the native
    // GFx text setter then dispatches the uninitialized object (the +39672ms garbage-vtable AV on the
    // 2nd in-process load). Excluding AUTOLOAD_HANDOFF leaves the reload's load untouched, exactly like a
    // boot autoload (phase IDLE, this branch never fires). Plain field write (not a menu/Scaleform call)
    // -> safe from the menu pump. See bd system-quit-load-profile-NOCRASH-milestone-2026-07-01.
    let sq_abort_phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&sq_abort_phase)
        && unsafe { PlayerIns::local_player_mut() }.is_ok()
    {
        let gm = game_man_ptr_or_null();
        if gm != 0 && gm != TITLE_OWNER_SCAN_START_ADDRESS {
            let ss_ptr = (gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *mut i32;
            if let Some(ss) = unsafe { safe_read_i32(gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) } {
                if ss == 2 || ss == 3 {
                    unsafe { *ss_ptr = 0 };
                    let n = SYSTEM_QUIT_INWORLD_LOAD_ABORT_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                    if n <= 8 || n % 120 == 0 {
                        append_autoload_debug(format_args!(
                            "system-quit-quickload: aborted stuck in-world load transition #{n} saveState={ss}->0 (old world still up) so return-title chain can run"
                        ));
                    }
                }
            }
        }
    }
    // MENU-PUMP-OWNED return-title submit. This hook IS the game's menu pump executing a
    // MenuWindowJob, so submitting the return-title chain from here (rather than from the concurrent
    // game-task tick) runs it in the menu pump's own frame and eliminates the Scaleform race that
    // produced the non-deterministic execute-fault crashes. Fire once ProfileSelect has closed (its
    // window cleared) during a return-title transition; the submit self-gates on queue-ready and
    // one-shots via the submit count. See bd system-quit-return-title-scaleform-race-2026-07-01.
    if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst) != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE
        && SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst) == 0
        && SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.load(Ordering::SeqCst) == 0
    {
        if let Ok(base) = game_module_base() {
            let system_dialog =
                SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.load(Ordering::SeqCst);
            if system_dialog != 0 && system_dialog != TITLE_OWNER_SCAN_START_ADDRESS {
                let _ = unsafe {
                    system_quit_submit_direct_return_title_chain(
                        base,
                        system_dialog,
                        "menu-pump-run-hook",
                    )
                };
            }
        }
    }
    ret
}

unsafe fn system_quit_open_profile_load_dialog(action_obj: usize) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- module base unavailable"
        ));
        return false;
    };
    let system_dialog = unsafe { safe_read_usize(action_obj + 0x8) }.unwrap_or(NULL);
    if system_dialog < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- action=0x{action_obj:x} dialog=0x{system_dialog:x} is not heap-like"
        ));
        return false;
    }
    let scene_proxy = system_dialog + SYSTEM_QUIT_DIALOG_SCENE_PROXY_1200_OFFSET;
    let scene_proxy_vt = unsafe { safe_read_usize(scene_proxy) }.unwrap_or(NULL);
    let want_scene_proxy_vt = base + SCENE_OBJ_PROXY_VTABLE_RVA;
    if scene_proxy_vt != want_scene_proxy_vt {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- dialog=0x{system_dialog:x} scene_proxy=dialog+0x{SYSTEM_QUIT_DIALOG_SCENE_PROXY_1200_OFFSET:x}=0x{scene_proxy:x} vt=0x{scene_proxy_vt:x} want=0x{want_scene_proxy_vt:x}"
        ));
        return false;
    }
    // Native title/menu route callers pass `owner + 0x50` as the MenuWindowJob's
    // field2_0x50 list argument. MenuWindowJob::Run later appends the loaded
    // owning MenuWindow to this DLFixedVector via FUN_140733ff0. Passing the
    // SceneObjProxy backref here is wrong: it lets the resource load start, then
    // asserts in DLFixedVector.inl line 0x296 when Run appends to a full/wrong
    // object.
    let menu_window_list = system_dialog + 0x50;
    let menu_window_list_count = unsafe { safe_read_usize(menu_window_list + 0x48) }.unwrap_or(!0);
    if menu_window_list_count >= 8 {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- candidate menu_window_list=dialog+0x50=0x{menu_window_list:x} count@+0x48={menu_window_list_count} would overflow DLFixedVector<8>"
        ));
        return false;
    }
    let Ok(wrapper_addr) = game_rva(PROFILE_SELECT_WRAPPER_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- failed to resolve ProfileSelect wrapper rva 0x{PROFILE_SELECT_WRAPPER_RVA:x}"
        ));
        return false;
    };
    let Ok(submit_addr) = game_rva(MENU_JOB_SUBMIT_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route abort -- failed to resolve menu-job submit rva 0x{MENU_JOB_SUBMIT_RVA:x}"
        ));
        return false;
    };
    let job_slot = &SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT as *const AtomicUsize as usize;
    SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT.store(NULL, Ordering::SeqCst);
    let wrapper: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(wrapper_addr) };
    append_autoload_debug(format_args!(
        "system-quit-dup: profile-load route FIRE 05_010_ProfileSelect wrapper 0x{wrapper_addr:x}(rcx=job_slot=0x{job_slot:x}, rdx=menu_window_list=dialog+0x50=0x{menu_window_list:x} count={menu_window_list_count}, r8=scene_proxy=0x{scene_proxy:x}) from system_dialog=0x{system_dialog:x}"
    ));
    let ret = unsafe { wrapper(job_slot, menu_window_list, scene_proxy) };
    let job = SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT.load(Ordering::SeqCst);
    let job_vt = if job >= HEAP_LO {
        unsafe { safe_read_usize(job) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if job < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: profile-load route 05_010 wrapper returned=0x{ret:x} job_slot=0x{job_slot:x} job=0x{job:x} job_vt=0x{job_vt:x}; no job to submit"
        ));
        return false;
    }
    let submit: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(submit_addr) };
    let submit_queue = system_dialog + 0x10;
    SYSTEM_QUIT_TOP_HIDE_ARMED_LIST.store(menu_window_list, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG.store(system_dialog, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.store(system_dialog, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: profile-load route SUBMIT job=0x{job:x} job_vt=0x{job_vt:x} via 0x{submit_addr:x}(queue=dialog+0x10=0x{submit_queue:x}, job_slot=0x{job_slot:x}); armed ProfileSelect list observer=0x{menu_window_list:x} -- no slot activation/no load"
    ));
    unsafe { submit(submit_queue, job_slot) };
    let job_after_submit = SYSTEM_QUIT_PROFILE_LOAD_JOB_SLOT.load(Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: profile-load route submitted 05_010 wrapper job; job_slot_after=0x{job_after_submit:x}"
    ));
    true
}

pub(crate) unsafe extern "system" fn system_quit_menu_window_list_push_hook(
    list: usize,
    window: usize,
) -> u8 {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    let orig = SYSTEM_QUIT_WINDOW_LIST_PUSH_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: MenuWindow list push trampoline unset for list=0x{list:x} window=0x{window:x} -- fail-closed return 0"
        ));
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(orig) };
    let ret = unsafe { original(list, window) };
    let armed_list = SYSTEM_QUIT_TOP_HIDE_ARMED_LIST.load(Ordering::SeqCst);
    let system_dialog = SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG.load(Ordering::SeqCst);
    if armed_list == 0 || armed_list != list || system_dialog == 0 {
        return ret;
    }
    SYSTEM_QUIT_TOP_HIDE_ARMED_LIST.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG.store(0, Ordering::SeqCst);
    let count = unsafe { safe_read_usize(list + 0x48) }.unwrap_or(0);
    let slot0 = unsafe { safe_read_usize(system_quit_list_slot_addr(list, 0)) }.unwrap_or(NULL);
    let slot1 = if count > 1 {
        unsafe { safe_read_usize(system_quit_list_slot_addr(list, 1)) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let top_window = slot0;
    let top_vt = if top_window >= HEAP_LO {
        unsafe { safe_read_usize(top_window) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let top_id = if top_window >= HEAP_LO {
        unsafe { safe_read_u16(top_window + 0x180) }.unwrap_or(u16::MAX)
    } else {
        u16::MAX
    };
    append_autoload_debug(format_args!(
        "system-quit-dup: ProfileSelect append observed list=0x{list:x} dialog=0x{system_dialog:x} count={count} slot0/top=0x{slot0:x} top_vt=0x{top_vt:x} top_id=0x{top_id:x} slot1=0x{slot1:x} appended_window=0x{window:x} ret={ret}"
    ));
    SYSTEM_QUIT_TOP_HIDE_PROFILE_WINDOW.store(window, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_LIST.store(list, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_TOP_MENU_ID.store(top_id as usize, Ordering::SeqCst);
    ret
}

pub(crate) unsafe extern "system" fn system_quit_noop_desktop_action_hook(
    action_obj: usize,
) -> usize {
    let recorded_action = SYSTEM_QUIT_NOOP_ACTION_LAST_OBJECT.load(Ordering::SeqCst);
    if action_obj != 0 && action_obj == recorded_action {
        let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
        if phase != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE {
            append_autoload_debug(format_args!(
                "system-quit-dup: cloned quick-load action re-entry ignored action=0x{action_obj:x} phase={phase}; native handoff already armed"
            ));
            return 0;
        }
        SYSTEM_QUIT_NOOP_SELECTION_COUNT.fetch_add(1, Ordering::SeqCst);
        let opened = unsafe { system_quit_open_profile_load_dialog(action_obj) };
        append_autoload_debug(format_args!(
            "system-quit-dup: cloned quick-load action selected action=0x{action_obj:x} opened={opened}; suppressing native Quit Game row action until ProfileSelect confirms slot"
        ));
        return 0;
    }
    let orig = SYSTEM_QUIT_NOOP_ACTION_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: Quit Game action trampoline is unset for action=0x{action_obj:x} -- fail-open return 0"
        ));
        return 0;
    }
    let original: unsafe extern "system" fn(usize) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { original(action_obj) }
}

pub(crate) unsafe extern "system" fn system_quit_duplicate_add_cancel_button_hook(
    dialog: usize,
    label: usize,
    action_fn: usize,
    enabled_fn: usize,
    keyguide_fn: usize,
) -> usize {
    let orig = SYSTEM_QUIT_DUPLICATE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: original AddCancelButton trampoline is unset -- fail-open return 0"
        ));
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    let caller_match = callstack_contains_game_rva(
        SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA
            .saturating_sub(SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES),
        SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA + SYSTEM_QUIT_DUPLICATE_CALLER_WINDOW_BYTES,
    );
    let before =
        unsafe { safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET) }
            .unwrap_or(0);
    let ret = unsafe { original(dialog, label, action_fn, enabled_fn, keyguide_fn) };
    if caller_match {
        let after_native =
            unsafe { safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET) }
                .unwrap_or(0);
        if after_native < 0x10 {
            if SYSTEM_QUIT_NOOP_ACTION_INSTALLED.load(Ordering::SeqCst)
                != SYSTEM_QUIT_NOOP_ACTION_INSTALLED_YES
            {
                append_autoload_debug(format_args!(
                    "system-quit-dup: matched Quit Game call but quick-load action hook is not installed; skipping cloned Load Game row"
                ));
                return ret;
            }
            let Ok(linehelp_addr) = game_rva(GET_GR_LINEHELP_ENTRY_RVA) else {
                append_autoload_debug(format_args!(
                    "system-quit-dup: failed to resolve GetGR_LineHelp rva 0x{GET_GR_LINEHELP_ENTRY_RVA:x}; skipping third row"
                ));
                return ret;
            };
            let Ok(label_dtor_addr) = game_rva(MENU_HELP_LABEL_DTOR_RVA) else {
                append_autoload_debug(format_args!(
                    "system-quit-dup: failed to resolve MenuHelpLabelComponent dtor rva 0x{MENU_HELP_LABEL_DTOR_RVA:x}; skipping third row"
                ));
                return ret;
            };
            let get_linehelp: unsafe extern "system" fn(usize, u32) -> usize =
                unsafe { std::mem::transmute(linehelp_addr) };
            let label_dtor: unsafe extern "system" fn(usize) =
                unsafe { std::mem::transmute(label_dtor_addr) };
            let mut label_storage =
                std::mem::MaybeUninit::<SystemQuitMenuHelpLabelScratch>::uninit();
            let load_label = label_storage.as_mut_ptr() as usize;
            unsafe {
                get_linehelp(load_label, SYSTEM_QUIT_LOAD_LINEHELP_ID);
                get_linehelp(
                    load_label + MENU_HELP_LABEL_HELP_OFFSET,
                    SYSTEM_QUIT_LOAD_LINEHELP_ID,
                );
            }
            let dup_ret =
                unsafe { original(dialog, load_label, action_fn, enabled_fn, keyguide_fn) };
            unsafe { label_dtor(load_label) };
            let after_dup = unsafe {
                safe_read_usize(dialog + PROPERTY_EDIT_DIALOG_PROPERTY_COUNT_1AF0_OFFSET)
            }
            .unwrap_or(0);
            let properties = dialog + PROPERTY_EDIT_DIALOG_PROPERTIES_1268_OFFSET;
            let aligned_properties = (properties + 0x7) & !0x7;
            let row_index = after_dup.saturating_sub(1);
            let third_row = aligned_properties + EDIT_PROPERTY_SIZE.saturating_mul(row_index);
            let third_controller =
                unsafe { safe_read_usize(third_row + EDIT_PROPERTY_CONTROLLER_OFFSET) }
                    .unwrap_or(0);
            let third_action = if third_controller != 0 {
                unsafe {
                    safe_read_usize(
                        third_controller + PROPERTY_NEW_BUTTON_CONTROLLER_ACTION_OBJECT_OFFSET,
                    )
                }
                .unwrap_or(0)
            } else {
                0
            };
            if third_action != 0 {
                SYSTEM_QUIT_NOOP_ACTION_LAST_OBJECT.store(third_action, Ordering::SeqCst);
            }
            SYSTEM_QUIT_DUPLICATE_COUNT.fetch_add(1, Ordering::SeqCst);
            SYSTEM_QUIT_DUPLICATE_LAST_COUNT_BEFORE.store(before, Ordering::SeqCst);
            SYSTEM_QUIT_DUPLICATE_LAST_COUNT_AFTER.store(after_dup, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "system-quit-dup: added cloned quick-load AddCancelButton label=GR_LineHelp:{SYSTEM_QUIT_LOAD_LINEHELP_ID} dialog=0x{dialog:x} count {before}->{after_native}->{after_dup} ret=0x{ret:x} dup_ret=0x{dup_ret:x} row=0x{third_row:x} controller=0x{third_controller:x} action=0x{third_action:x}"
            ));
        } else {
            append_autoload_debug(format_args!(
                "system-quit-dup: matched Quit Game call but count after native is {after_native}, not duplicating"
            ));
        }
    }
    ret
}

/// Scaleform handler CONSTRUCTOR hook (`FUN_1411a8890`, deobf 0x1411a8870). rcx = the object being
/// constructed (the 0x58 handler embedded at container+0x40), rdx = parent. Records the object as
/// live, then forwards to the original ctor (which returns the object pointer). Read-only w.r.t.
/// game state; only maintains our live-set. See SCALEFORM_HANDLER_LIVE.
pub(crate) unsafe extern "system" fn scaleform_handler_ctor_hook(
    obj: usize,
    parent: usize,
) -> usize {
    let orig = SCALEFORM_HANDLER_CTOR_ORIG.load(Ordering::SeqCst);
    SCALEFORM_HANDLER_CTORS.fetch_add(1, Ordering::SeqCst);
    if obj != 0 {
        if let Ok(mut live) = SCALEFORM_HANDLER_LIVE.lock() {
            // Cap guard: if a genuine leak fills the table, stop growing (drop tracking of the
            // oldest) so the probe can't OOM -- the double-free detection still works for recent objs.
            if live.len() >= SCALEFORM_HANDLER_LIVE_CAP {
                live.remove(0);
            }
            live.push(obj);
        }
    }
    let _ = parent;
    if orig == HOOK_ORIGINAL_UNSET || orig == 0 {
        return obj;
    }
    let f: unsafe extern "system" fn(usize, usize) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { f(obj, parent) }
}

/// Scaleform handler inner DESTRUCTOR hook (`FUN_1411a8920`, deobf 0x1411a8900). rcx = the object.
/// If the object is in our live-set -> a normal teardown: remove it and forward to the original.
/// If it is NOT live -> a DOUBLE-FREE (the repeated-switch ProfileSelect UAF): the original would
/// walk this object's now-garbage intrusive list and crash. Log it and RETURN WITHOUT forwarding,
/// so the freed list is never dereferenced. Safe: an already-destructed object needs no second
/// teardown. This both names the bug (counter + last-obj oracle + debug line) and stops the crash.
pub(crate) unsafe extern "system" fn scaleform_handler_dtor_hook(obj: usize) {
    let orig = SCALEFORM_HANDLER_DTOR_ORIG.load(Ordering::SeqCst);
    SCALEFORM_HANDLER_DTORS.fetch_add(1, Ordering::SeqCst);
    let live = if obj == 0 {
        false
    } else if let Ok(mut set) = SCALEFORM_HANDLER_LIVE.lock() {
        if let Some(pos) = set.iter().rposition(|&a| a == obj) {
            set.swap_remove(pos);
            true
        } else {
            false
        }
    } else {
        // Lock poisoned/unavailable: fail SAFE toward forwarding (treat as live) so we never skip a
        // legitimate destructor on a lock hiccup -- the crash is rarer than the lock being fine.
        true
    };
    if !live {
        let n = SCALEFORM_HANDLER_DOUBLE_FREES.fetch_add(1, Ordering::SeqCst) + 1;
        SCALEFORM_HANDLER_LAST_DOUBLE_FREE_OBJ.store(obj, Ordering::SeqCst);
        if n <= 32 {
            let parent = unsafe { safe_read_usize(obj + 0x18) }.unwrap_or(0);
            let list_head = unsafe { safe_read_usize(obj + 0x28) }.unwrap_or(0);
            let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
            append_crash_log(format_args!(
                "scaleform-handler-guard: DOUBLE-FREE #{n} of handler obj=0x{obj:x} container=0x{:x} parent(+0x18)=0x{parent:x} list_head(+0x28)=0x{list_head:x} quickload_phase={phase} -- SKIPPED inner dtor (would have walked freed list) to prevent the ProfileSelect UAF crash",
                obj.wrapping_sub(0x40)
            ));
        }
        return;
    }
    if orig == HOOK_ORIGINAL_UNSET || orig == 0 {
        return;
    }
    let f: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(orig) };
    unsafe { f(obj) };
}

/// Read CSDelayDeleteMan's pending count (+0x40) and high-water (+0x44) via the singleton pointer
/// at DELAY_DELETE_MAN_SINGLETON_PTR_RVA. Returns `(pending, highwater)` or None if the singleton is
/// null/unresolved or the read is implausible (a wrong RVA/layout -> the count fails the sane bound).
/// This is the repeated-switch overflow oracle: pending climbing ~+10/switch means the delay-delete
/// pump is not draining the torn-down profile renderers, whose still-registered draw tasks then keep
/// filling the GX command queue.
pub(crate) unsafe fn delay_delete_pending() -> Option<(usize, usize)> {
    let base = game_rva(0).ok()?;
    let man = unsafe { safe_read_usize(base + DELAY_DELETE_MAN_SINGLETON_PTR_RVA) }?;
    if man < 0x10000 {
        return None;
    }
    let pending = unsafe { safe_read_i32(man + DELAY_DELETE_MAN_PENDING_COUNT_OFFSET) }?;
    let highwater = unsafe { safe_read_i32(man + DELAY_DELETE_MAN_PENDING_HIGHWATER_OFFSET) }?;
    if !(0..=DELAY_DELETE_MAN_PENDING_SANE_MAX as i32).contains(&pending) {
        return None;
    }
    Some((pending as usize, highwater.max(0) as usize))
}

/// OWNERSHIP LEDGER -- record that we took manual ownership of a native object (we are now
/// responsible for releasing it). Pair EVERY `ownership_take` with exactly one `ownership_release`
/// on the discharge path; a bare `store(0)`/overwrite that drops the pointer without a release is
/// the leak this ledger exists to catch.
pub(crate) fn ownership_take(class: OwnedClass) {
    let i = class as usize;
    let taken = OWNED_TAKEN[i].fetch_add(1, Ordering::SeqCst) + 1;
    let released = OWNED_RELEASED[i].load(Ordering::SeqCst);
    OWNED_MAX_OUTSTANDING[i].fetch_max(taken.saturating_sub(released), Ordering::SeqCst);
}

/// OWNERSHIP LEDGER -- record that we handed a native-owned object back to its native lifecycle
/// (e.g. delete-enqueued it). Only call on the REAL discharge path, never on an incidental pointer
/// clear, so the ledger stays an honest leak detector.
pub(crate) fn ownership_release(class: OwnedClass) {
    OWNED_RELEASED[class as usize].fetch_add(1, Ordering::SeqCst);
}

/// Current taken-but-not-released count for a class.
pub(crate) fn ownership_outstanding(class: OwnedClass) -> usize {
    let i = class as usize;
    OWNED_TAKEN[i]
        .load(Ordering::SeqCst)
        .saturating_sub(OWNED_RELEASED[i].load(Ordering::SeqCst))
}

/// OWNERSHIP LEDGER -- assert every class stays within its bound; on breach, latch the violation
/// oracle and log loudly. Called at each switch boundary (cheap enough to call per-frame). Returns
/// true iff all classes are within bound. A breach means a native-owned object was taken without a
/// paired release (the spared-renderer leak class) -- caught at the FIRST offending switch, not at a
/// downstream crash.
pub(crate) fn ownership_ledger_check(context: &str) -> bool {
    let mut ok = true;
    for i in 0..OWNED_CLASS_COUNT {
        let taken = OWNED_TAKEN[i].load(Ordering::SeqCst);
        let released = OWNED_RELEASED[i].load(Ordering::SeqCst);
        let outstanding = taken.saturating_sub(released);
        if outstanding > OWNED_CLASS_BOUND[i] {
            ok = false;
            OWNED_LEDGER_VIOLATIONS.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "OWNERSHIP-LEDGER VIOLATION ({context}): class '{}' outstanding={outstanding} > bound={} (taken={taken} released={released}) -- a native-owned object was taken without a paired release (the spared-renderer leak class)",
                OWNED_CLASS_NAMES[i], OWNED_CLASS_BOUND[i]
            ));
        }
    }
    ok
}

/// Destroy a previously-spared portrait renderer via CSDelayDeleteMan -- the exact native path the
/// profile-renderer teardown (`FUN_1409b2f00`) uses for the other 9 renderers each teardown (marks
/// the object's +0x756 byte, enqueues it, freed on the delete pump when the GPU is done). Vtable-
/// guarded so a stale/freed/garbage pointer is never enqueued. MUST run on the game/menu thread (the
/// same thread the native teardown runs on -- the manager's list is mutated without locks). Returns
/// true if the object was enqueued for deletion.
pub(crate) unsafe fn delay_delete_enqueue_renderer(renderer: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if renderer == 0 || renderer == null {
        return false;
    }
    let Ok(base) = game_module_base() else {
        return false;
    };
    // Only a LIVE profile renderer (correct vtable) -- never a freed/garbage pointer.
    if unsafe { safe_read_usize(renderer) }.unwrap_or(0)
        != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
    {
        return false;
    }
    let man = unsafe { safe_read_usize(base + DELAY_DELETE_MAN_SINGLETON_PTR_RVA) }.unwrap_or(0);
    if man < 0x10000 {
        return false;
    }
    let Ok(enqueue) = game_rva(DELAY_DELETE_ENQUEUE_RVA as u32) else {
        return false;
    };
    let f: unsafe extern "system" fn(usize, usize) -> u8 = unsafe { std::mem::transmute(enqueue) };
    unsafe { f(man, renderer) };
    PROFILE_SPARE_ORPHANS_DELETED.fetch_add(1, Ordering::SeqCst);
    true
}

/// Format an `AtomicUsize` low-water value: `usize::MAX` is the never-sampled sentinel.
fn fmt_lowwater(v: usize) -> String {
    if v == usize::MAX {
        "unsampled".to_string()
    } else {
        v.to_string()
    }
}

/// Bump the GX command-queue producer histogram for `key` (lock-free open addressing; a full table
/// counts drops instead of evicting so the hot producers stay attributed).
fn gx_cmd_queue_hist_bump(key: usize) {
    if key == 0 {
        return;
    }
    let mut idx = (key >> 4) % GX_CMD_QUEUE_HIST_SLOTS;
    for _ in 0..GX_CMD_QUEUE_HIST_SLOTS {
        let cur = GX_CMD_QUEUE_HIST_KEYS[idx].load(Ordering::Relaxed);
        if cur == key {
            GX_CMD_QUEUE_HIST_COUNTS[idx].fetch_add(1, Ordering::Relaxed);
            return;
        }
        if cur == 0 {
            match GX_CMD_QUEUE_HIST_KEYS[idx].compare_exchange(
                0,
                key,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    GX_CMD_QUEUE_HIST_COUNTS[idx].fetch_add(1, Ordering::Relaxed);
                    return;
                }
                Err(actual) if actual == key => {
                    GX_CMD_QUEUE_HIST_COUNTS[idx].fetch_add(1, Ordering::Relaxed);
                    return;
                }
                Err(_) => {}
            }
        }
        idx = (idx + 1) % GX_CMD_QUEUE_HIST_SLOTS;
    }
    GX_CMD_QUEUE_HIST_DROPPED.fetch_add(1, Ordering::Relaxed);
}

/// Top-N GX producer histogram entries as `0x<rva>[+self] x<count>`, count-descending. `+self`
/// marks submissions whose call chain passed through our DLL (our pipeline caused them).
pub(crate) fn gx_cmd_queue_hist_top(n: usize) -> String {
    let mut entries: Vec<(usize, usize)> = (0..GX_CMD_QUEUE_HIST_SLOTS)
        .filter_map(|i| {
            let key = GX_CMD_QUEUE_HIST_KEYS[i].load(Ordering::Relaxed);
            let count = GX_CMD_QUEUE_HIST_COUNTS[i].load(Ordering::Relaxed);
            (key != 0 && count != 0).then_some((key, count))
        })
        .collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries
        .iter()
        .take(n)
        .map(|(key, count)| {
            let rva = key & !GX_CMD_QUEUE_SELF_TAG;
            let self_tag = if key & GX_CMD_QUEUE_SELF_TAG != 0 {
                "+self"
            } else {
                ""
            };
            format!("0x{rva:x}{self_tag} x{count}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Thin entry hook on the GX drain pump `FUN_141b3bdc0` (deobf 0x1b3bda0): latch its context
/// (param_1, the object holding the 109-bucket per-frame slot-range table) and forward. The bucket
/// table is what `gx_cmd_queue_bucket_summary` reads; the pump itself is untouched.
pub(crate) unsafe extern "system" fn gx_cmd_pump_hook(
    ctx: usize,
    param2: usize,
    param3: i32,
    param4: u32,
) {
    GX_CMD_PUMP_CTX.store(ctx, Ordering::Relaxed);
    let orig = GX_CMD_PUMP_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET || orig == 0 {
        return;
    }
    let f: unsafe extern "system" fn(usize, usize, i32, u32) = unsafe { std::mem::transmute(orig) };
    unsafe { f(ctx, param2, param3, param4) }
}

/// Nonzero per-bucket widths from the pump context's 109-bucket slot-range table as
/// `idx:width, ...` (begin at ctx+0x30+idx*0x18, end at +0x34). The bucket whose width GROWS
/// across switches is the retained-producer class behind the 0x1aeaf05 overflow. Empty string
/// until the pump context has been latched.
pub(crate) fn gx_cmd_queue_bucket_summary() -> String {
    let ctx = GX_CMD_PUMP_CTX.load(Ordering::Relaxed);
    if ctx == 0 {
        return String::new();
    }
    let mut parts = Vec::new();
    for idx in 0..GX_CMD_QUEUE_BUCKET_COUNT {
        let begin = unsafe {
            safe_read_i32(ctx + GX_CMD_QUEUE_BUCKET_BEGIN_OFFSET + idx * GX_CMD_QUEUE_BUCKET_STRIDE)
        }
        .unwrap_or(0);
        let end = unsafe {
            safe_read_i32(ctx + GX_CMD_QUEUE_BUCKET_END_OFFSET + idx * GX_CMD_QUEUE_BUCKET_STRIDE)
        }
        .unwrap_or(0);
        let width = end.saturating_sub(begin);
        // Widths above the slot capacity are torn/stale reads (this walker races the render
        // thread; run 10e's post-crash telemetry read showed multi-million "widths") -- skip them.
        if width > 0 && width <= GX_CMD_QUEUE_BUCKET_WIDTH_SANE_MAX {
            parts.push(format!("{idx}:{width}"));
        }
    }
    parts.join(", ")
}

/// Sample the command-byte arena's remaining space (arena at queue+0x40; remaining =
/// limit@+0x20 - align4(cursor_lo@+0x28), per the FUN_141c48e80 decompile) and fold it into the
/// cumulative + per-switch low-water. Returns the sampled remaining for the caller's own logging,
/// or None on unreadable fields.
unsafe fn gx_cmd_arena_sample_remaining(queue: usize) -> Option<i64> {
    let arena = queue + GX_CMD_QUEUE_ARENA_OFFSET;
    let limit = unsafe { safe_read_i32(arena + GX_CMD_ARENA_LIMIT_OFFSET) }?;
    let cursor_lo = unsafe { safe_read_i32(arena + GX_CMD_ARENA_CURSOR_OFFSET) }?;
    let aligned = (cursor_lo.wrapping_add(3)) & !3;
    let remaining = i64::from(limit) - i64::from(aligned);
    let clamped = remaining.max(0) as usize;
    GX_CMD_ARENA_MIN_REMAINING.fetch_min(clamped, Ordering::Relaxed);
    GX_CMD_ARENA_SWITCH_MIN_REMAINING.fetch_min(clamped, Ordering::Relaxed);
    Some(remaining)
}

/// Telemetry-only wrapper for `reserve_command_queue_slot` (deobf 0x141aeae60): the fixed 192-slot
/// GX command queue whose full-queue null-slot write is the repeated-switch crash at rva 0x1aeaf05
/// (reproduced at switch #4, run autostep10c-directarm-20260703-145348). Tracks occupancy
/// high-water (cumulative + per-switch), total reserves, and a producer histogram keyed by the
/// first game-.text caller outside the enqueue-wrapper band (self-tagged when our DLL is in the
/// chain), and dumps the top producers as the queue nears the edge -- so the overflow run NAMES the
/// accumulating producer. ALWAYS forwards unchanged: the 5ae3965 drop-on-overflow guard corrupted
/// the render (c2794d9) and must not return.
pub(crate) unsafe extern "system" fn gx_reserve_cmd_queue_slot_hook(
    queue: usize,
    param2: usize,
    param3: i32,
    param4: u32,
    param5: u32,
) -> usize {
    let count = unsafe { safe_read_i32(queue + GX_CMD_QUEUE_COUNT_OFFSET) }.unwrap_or(-1);
    let cap = unsafe { safe_read_i32(queue + GX_CMD_QUEUE_CAP_OFFSET) }.unwrap_or(-1);
    if count >= 0 {
        GX_CMD_QUEUE_MAX_FILL.fetch_max(count as usize, Ordering::Relaxed);
        GX_CMD_QUEUE_SWITCH_MAX_FILL.fetch_max(count as usize, Ordering::Relaxed);
    }
    if cap > 0 {
        GX_CMD_QUEUE_CAP_SEEN.store(cap as usize, Ordering::Relaxed);
    }
    GX_CMD_QUEUE_SUBMITS.fetch_add(1, Ordering::Relaxed);
    let (producer, self_in_stack) =
        stack_producer_rva(GX_CMD_QUEUE_WRAPPER_RVA_MIN..GX_CMD_QUEUE_WRAPPER_RVA_MAX);
    let key = if self_in_stack {
        producer | GX_CMD_QUEUE_SELF_TAG
    } else {
        producer
    };
    gx_cmd_queue_hist_bump(key);
    let arena_remaining = unsafe { gx_cmd_arena_sample_remaining(queue) };
    // Peak-frame bucket snapshot: the growth only materializes in teardown/reload frames (run 10e),
    // so capture the bucket composition as the per-switch high-water climbs, not just near cap.
    if count >= 0 {
        let count_us = count as usize;
        let last = GX_CMD_QUEUE_PEAK_LAST_LOGGED.load(Ordering::Relaxed);
        if count_us >= GX_CMD_QUEUE_PEAK_LOG_MIN
            && count_us >= last + GX_CMD_QUEUE_PEAK_LOG_STEP
            && GX_CMD_QUEUE_PEAK_LAST_LOGGED
                .compare_exchange(last, count_us, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        {
            append_autoload_debug(format_args!(
                "gx-cmdqueue: PEAK count={count}/{cap} arena_remaining={} buckets: {}",
                arena_remaining.unwrap_or(-1),
                gx_cmd_queue_bucket_summary()
            ));
        }
    }
    if cap > 0 && count >= 0 && count as usize >= (cap as usize) - GX_CMD_QUEUE_NEARFULL_MARGIN {
        let hits = GX_CMD_QUEUE_NEARFULL_HITS.fetch_add(1, Ordering::Relaxed);
        if hits % GX_CMD_QUEUE_NEARFULL_LOG_EVERY == 0 {
            append_autoload_debug(format_args!(
                "gx-cmdqueue: NEAR-FULL count={count}/{cap} (hit #{hits}) queue=0x{queue:x} top producers: {} | buckets: {}",
                gx_cmd_queue_hist_top(8),
                gx_cmd_queue_bucket_summary()
            ));
        }
    }
    let orig = GX_RESERVE_CMD_QUEUE_SLOT_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET || orig == 0 {
        // Fail-open is impossible here (the caller needs a real slot buffer); this branch can only
        // be reached if MinHook called the detour before the trampoline store, which queue_enable
        // ordering prevents. Keep a loud log so an impossible state is visible, not silent.
        append_autoload_debug(format_args!(
            "gx-cmdqueue: trampoline unset in detour (queue=0x{queue:x}) -- forwarding impossible"
        ));
        return 0;
    }
    let f: unsafe extern "system" fn(usize, usize, i32, u32, u32) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { f(queue, param2, param3, param4, param5) }
}

/// Install the GX command-queue producer telemetry hooks (never alter queue behavior): the
/// reserve-slot occupancy/histogram wrapper plus the thin pump-context latch for the bucket table.
fn install_gx_cmd_queue_telemetry() {
    if GX_RESERVE_CMD_QUEUE_SLOT_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "gx-cmdqueue: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let (Ok(addr), Ok(pump_addr)) = (
        game_rva(GX_RESERVE_CMD_QUEUE_SLOT_RVA as u32),
        game_rva(GX_CMD_PUMP_RVA as u32),
    ) else {
        append_autoload_debug(format_args!(
            "gx-cmdqueue: failed to resolve rvas 0x{GX_RESERVE_CMD_QUEUE_SLOT_RVA:x}/0x{GX_CMD_PUMP_RVA:x}"
        ));
        return;
    };
    let mut ok = true;
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            gx_reserve_cmd_queue_slot_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            GX_RESERVE_CMD_QUEUE_SLOT_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            ok &= unsafe { hook.queue_enable() }.is_ok();
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "gx-cmdqueue: MhHook::new(reserve) failed: {status:?}"
            ));
            ok = false;
        }
    }
    match unsafe { MhHook::new(pump_addr as *mut c_void, gx_cmd_pump_hook as *mut c_void) } {
        Ok(hook) => {
            GX_CMD_PUMP_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            ok &= unsafe { hook.queue_enable() }.is_ok();
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "gx-cmdqueue: MhHook::new(pump) failed: {status:?}"
            ));
            ok = false;
        }
    }
    if ok && matches!(unsafe { MH_ApplyQueued() }, MH_STATUS::MH_OK) {
        GX_RESERVE_CMD_QUEUE_SLOT_INSTALLED.store(1, Ordering::SeqCst);
        GX_CMD_PUMP_INSTALLED.store(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "gx-cmdqueue: producer telemetry hooked reserve_command_queue_slot 0x{addr:x} + pump 0x{pump_addr:x} (occupancy high-water + caller histogram + bucket table; forwards always)"
        ));
    } else {
        append_autoload_debug(format_args!(
            "gx-cmdqueue: queue_enable/MH_ApplyQueued failed (reserve 0x{addr:x}, pump 0x{pump_addr:x})"
        ));
    }
}

/// Install the Scaleform handler ctor/dtor lifecycle guard (repeated-switch ProfileSelect UAF).
fn install_scaleform_handler_lifecycle_guard() {
    if SCALEFORM_HANDLER_TRACE_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "scaleform-handler-guard: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let (Ok(ctor_addr), Ok(dtor_addr)) = (
        game_rva(SCALEFORM_HANDLER_CTOR_RVA as u32),
        game_rva(SCALEFORM_HANDLER_DTOR_RVA as u32),
    ) else {
        append_autoload_debug(format_args!(
            "scaleform-handler-guard: failed to resolve ctor/dtor rvas 0x{SCALEFORM_HANDLER_CTOR_RVA:x}/0x{SCALEFORM_HANDLER_DTOR_RVA:x}"
        ));
        return;
    };
    let mut ok = true;
    match unsafe {
        MhHook::new(
            ctor_addr as *mut c_void,
            scaleform_handler_ctor_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SCALEFORM_HANDLER_CTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            ok &= unsafe { hook.queue_enable() }.is_ok();
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "scaleform-handler-guard: MhHook::new(ctor) failed: {status:?}"
            ));
            ok = false;
        }
    }
    match unsafe {
        MhHook::new(
            dtor_addr as *mut c_void,
            scaleform_handler_dtor_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SCALEFORM_HANDLER_DTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            ok &= unsafe { hook.queue_enable() }.is_ok();
            std::mem::forget(hook);
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "scaleform-handler-guard: MhHook::new(dtor) failed: {status:?}"
            ));
            ok = false;
        }
    }
    if !ok {
        return;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            SCALEFORM_HANDLER_TRACE_INSTALLED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "scaleform-handler-guard: hooked ctor 0x{ctor_addr:x} + inner dtor 0x{dtor_addr:x}; live-set double-free guard armed (skips freed-object destructs)"
            ));
        }
        status => append_autoload_debug(format_args!(
            "scaleform-handler-guard: MH_ApplyQueued failed: {status:?}"
        )),
    }
}

fn install_system_quit_menu_window_job_run_hook() {
    if SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-dup: MH_Initialize for MenuWindowJob::Run hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(MENU_WINDOW_JOB_RUN_RVA as u32) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve MenuWindowJob::Run rva 0x{MENU_WINDOW_JOB_RUN_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_menu_window_job_run_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_ORIG
                .store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-dup: queue_enable MenuWindowJob::Run hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_INSTALLED.store(
                        SYSTEM_QUIT_MENU_WINDOW_JOB_RUN_INSTALLED_YES,
                        Ordering::SeqCst,
                    );
                    append_autoload_debug(format_args!(
                        "system-quit-dup: hooked MenuWindowJob::Run 0x{addr:x}; will map System/ProfileSelect resources to real MenuWindow pointers"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-dup: MH_ApplyQueued MenuWindowJob::Run hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-dup: MhHook::new MenuWindowJob::Run hook failed: {status:?}"
        )),
    }
}

fn install_system_quit_window_list_push_hook() {
    if SYSTEM_QUIT_WINDOW_LIST_PUSH_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_WINDOW_LIST_PUSH_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-dup: MH_Initialize for MenuWindow list push hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(MENU_WINDOW_LIST_PUSH_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve MenuWindow list push rva 0x{MENU_WINDOW_LIST_PUSH_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_menu_window_list_push_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_WINDOW_LIST_PUSH_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-dup: queue_enable MenuWindow list push hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_WINDOW_LIST_PUSH_INSTALLED
                        .store(SYSTEM_QUIT_WINDOW_LIST_PUSH_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-dup: hooked MenuWindow list push 0x{addr:x}; will record ProfileSelect append/list for Back/removal restore state"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-dup: MH_ApplyQueued MenuWindow list push hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-dup: MhHook::new MenuWindow list push hook failed: {status:?}"
        )),
    }
}

fn install_system_quit_noop_action_hook() {
    if SYSTEM_QUIT_NOOP_ACTION_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_NOOP_ACTION_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-dup: MH_Initialize for no-op action hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_RETURN_TITLE_ACTION_DO_CALL_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve Quit Game action invoke rva 0x{SYSTEM_QUIT_RETURN_TITLE_ACTION_DO_CALL_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_noop_desktop_action_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_NOOP_ACTION_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-dup: queue_enable no-op action hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_NOOP_ACTION_INSTALLED
                        .store(SYSTEM_QUIT_NOOP_ACTION_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-dup: hooked Quit Game action invoke 0x{addr:x}; recorded cloned quick-load action object will route to ProfileSelect"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-dup: MH_ApplyQueued no-op action hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-dup: MhHook::new no-op action hook failed: {status:?}"
        )),
    }
}

pub(crate) unsafe extern "system" fn system_quit_profile_load_activate_hook(
    dialog: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let orig = SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: ProfileLoadDialog activation trampoline unset for dialog=0x{dialog:x} -- fail-closed return 0"
        ));
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    let base = game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let vt = unsafe { safe_read_usize(dialog) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let expected_vt = if base != TITLE_OWNER_SCAN_START_ADDRESS {
        base + PROFILE_LOAD_DIALOG_VTABLE_RVA
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let hidden = SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) != 0;
    let profile_window = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    let system_quit_profile_active = hidden && profile_window != 0 && vt == expected_vt;
    if !system_quit_profile_active {
        return unsafe { original(dialog, b, c, d) };
    }

    let cursor = unsafe { safe_read_i32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }.unwrap_or(-1);
    let bound = unsafe { safe_read_i32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET) }.unwrap_or(-1);
    SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_DIALOG.store(dialog, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_CURSOR.store(cursor as usize, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_BOUND.store(bound as usize, Ordering::SeqCst);

    SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_COUNT.fetch_add(1, Ordering::SeqCst);

    // PRODUCT PATH (human-driven pick): the slot activation IS the load confirmation. A human's A on
    // a slot must load that character; the old flow instead forwarded into the native confirm ->
    // MessageBox -> OK -> load-job chain, but the product msgbox path SUPPRESSES that "load this
    // profile?" MessageBox before it renders, so a human never gets an OK to press and every A just
    // re-opens+re-suppresses the confirm -- the pick stalls, no load-job Run, no arm (observed
    // 2026-07-02: 24 activations, zero loads). Arm the save-safe switch DIRECTLY here and natively
    // cancel-close ProfileSelect, satisfying the confirm's only semantic side effect (user chose to
    // load this profile) with ZERO MessageBox and zero extra input. Repeatable: the continue_confirm
    // hook returns the phase to IDLE after each reload, so the next pick re-arms cleanly.
    //
    // The repro autopilot takes this SAME direct-arm path as a human pick. Its old scripted
    // double-A confirm chain (A pick -> confirm MessageBox -> A OK -> load-job Run -> arm) is
    // unreachable after the FIRST completed switch: that switch's arm latches PRODUCT_AUTOLOAD_ARMED,
    // whose msgbox suppression then eats the confirm box the second A needs, so every later pick
    // stalled (observed autostep10b 2026-07-03: switch #1 confirmed via the OK chain, switch #2
    // suppressed msgbox-skip #2/#3 and held 20 min). It also no longer matched the human flow this
    // autopilot exists to reproduce. Remaining gates: skip on the native-forward opt-in, when a
    // switch is already in flight (phase != IDLE), for an out-of-range cursor, or for an EMPTY slot
    // (arming an empty slot would tear down to a clean title then fail the deserialize).
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if !system_quit_profile_load_activation_allowed()
        && phase == SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE
        && (0..bound).contains(&cursor)
    {
        if !unsafe { profile_slot_has_character(cursor) } {
            append_autoload_debug(format_args!(
                "system-quit-dup: ProfileSelect slot activation IGNORED dialog=0x{dialog:x} cursor={cursor} bound={bound} -- slot holds no character; not arming a switch (would strand the game at a blank title)"
            ));
            return unsafe { original(dialog, b, c, d) };
        }
        unsafe { system_quit_arm_quickload_autoload(cursor, "ProfileSelectSlotActivate") };
        // The arm only takes when the preserved System dialog is present; on success it advances the
        // phase past IDLE. If it took, cancel-close ProfileSelect ourselves (no confirm-lambda runs on
        // this direct path) so the menu-pump return-title chain tears the world down + reloads the
        // picked slot at a clean title. If it did NOT take, fall through to the native activation so
        // the pick is not silently dropped.
        if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst) != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE {
            if let Ok(close_addr) = game_rva(SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA) {
                let close_fn: unsafe extern "system" fn(usize) =
                    unsafe { std::mem::transmute(close_addr) };
                unsafe { close_fn(dialog) };
                SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_COUNT.fetch_add(1, Ordering::SeqCst);
            }
            append_autoload_debug(format_args!(
                "system-quit-dup: ProfileSelect slot activation ARMED save-safe switch dialog=0x{dialog:x} cursor={cursor} bound={bound}; cancel-closed ProfileSelect -> return-title + clean-title fresh-deserialize of slot {cursor} (zero MessageBox)"
            ));
            return 0;
        }
        append_autoload_debug(format_args!(
            "system-quit-dup: ProfileSelect slot activation direct-arm did NOT take (no preserved System dialog) dialog=0x{dialog:x} cursor={cursor}; forwarding native activation"
        ));
    }

    append_autoload_debug(format_args!(
        "system-quit-dup: ProfileSelect slot activation dialog ALLOWED dialog=0x{dialog:x} cursor={cursor} bound={bound} profile_window=0x{profile_window:x} phase={phase}; forwarding native (load-job Run remains guarded)"
    ));
    unsafe { original(dialog, b, c, d) }
}

/// Advance the System->Quit repro autopilot to `next`, resetting the phase-local tick and the
/// waiting-log latch.
fn sq_repro_transition(next: usize) {
    SQ_REPRO_STATE.store(next, Ordering::SeqCst);
    SQ_REPRO_STATE_TICK.store(0, Ordering::SeqCst);
    SQ_REPRO_STATE_TAPS.store(0, Ordering::SeqCst);
}

/// Log `msg` exactly once for the current repro phase (`SQ_REPRO_STATE_TAPS` latches it), used when
/// a phase has issued all its edges and is now HOLDING until its transition is observed. Not a retry
/// budget -- a boolean latch so the "waiting" line is not spammed.
fn sq_repro_waiting_once(msg: &str) {
    if SQ_REPRO_STATE_TAPS.swap(1, Ordering::SeqCst) == 0 {
        append_autoload_debug(format_args!(
            "sq-repro: {msg} (holding until observed; no re-tap)"
        ));
    }
}

/// Cumulative ProfileSelect OK-confirm count (cancel-close BLOCK + ALLOW). Legacy fallback signal:
/// the CONFIRM state's primary advance is the direct-arm phase observation; this count (an INCREASE
/// over the per-switch baseline, so switch #2 does not trip on switch #1's residual) only fires if
/// the pick fell through to the native confirm-box -> OK -> load-job chain.
fn sq_repro_confirm_count() -> usize {
    SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_BLOCK_COUNT.load(Ordering::SeqCst)
        + SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ALLOW_COUNT.load(Ordering::SeqCst)
}

/// The ProfileSelect slot the current switch loads (clamped to the target table).
fn sq_repro_target_slot() -> i32 {
    let i = SQ_REPRO_SWITCH_INDEX.load(Ordering::SeqCst);
    SQ_REPRO_TARGET_SLOTS[i.min(SQ_REPRO_TARGET_SLOTS.len() - 1)]
}

/// How many back-to-back switches to drive. Defaults to `SQ_REPRO_TARGET_SWITCHES` (2); overridable
/// via `ER_EFFECTS_SQ_REPRO_SWITCHES` (clamped to [1, target-table length]) so a 1-switch baseline
/// can be run with the identical code path to isolate the two-switch regression.
fn sq_repro_target_switches() -> usize {
    let n = std::env::var("ER_EFFECTS_SQ_REPRO_SWITCHES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(SQ_REPRO_TARGET_SWITCHES);
    n.clamp(1, SQ_REPRO_TARGET_SLOTS.len())
}

/// Enter a switch: capture the confirm-count baseline and clear the per-switch menu-window/cursor
/// signals so the state machine re-detects them fresh for this switch (they hold stale pointers from
/// the prior switch otherwise). Called before OPEN_MENU for every switch.
fn sq_repro_begin_switch() {
    SQ_REPRO_CONFIRM_BASELINE.store(sq_repro_confirm_count(), Ordering::SeqCst);
    SYSTEM_QUIT_INGAME_TOP_WINDOW.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_OPTION_SETTING_WINDOW.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_SELECT_WINDOW.store(0, Ordering::SeqCst);
    SQ_REPRO_INITIAL_CURSOR.store(usize::MAX, Ordering::SeqCst);
    SQ_REPRO_WAIT_RELOAD_FRAMES.store(0, Ordering::SeqCst);
    // GX command-queue growth curve: log the finished switch's occupancy high-water + top
    // producers, then reset the per-switch high-water so each switch reports its own peak (the
    // 0x1aeaf05 overflow shows as this peak climbing toward cap across switches).
    let switch_peak = GX_CMD_QUEUE_SWITCH_MAX_FILL.swap(0, Ordering::SeqCst);
    let switch_arena_min = GX_CMD_ARENA_SWITCH_MIN_REMAINING.swap(usize::MAX, Ordering::SeqCst);
    GX_CMD_QUEUE_PEAK_LAST_LOGGED.store(0, Ordering::SeqCst);
    let (dd_pending, dd_highwater) = unsafe { delay_delete_pending() }
        .map(|(p, h)| (p as i64, h as i64))
        .unwrap_or((-1, -1));
    // Ownership-conservation check: if any native-owned class is over its bound, this is where the
    // spared-renderer leak would have surfaced (switch #2), long before the GX queue overflow crash.
    ownership_ledger_check("switch-boundary");
    append_autoload_debug(format_args!(
        "gx-cmdqueue: switch boundary -- prev-switch peak {switch_peak}/{} arena_min_remaining={} delaydelete_pending={dd_pending} (highwater {dd_highwater}) spared_outstanding={} ledger_violations={} (cumulative max {}, arena min {}, reserves {}) top producers: {} | buckets: {}",
        GX_CMD_QUEUE_CAP_SEEN.load(Ordering::SeqCst),
        fmt_lowwater(switch_arena_min),
        ownership_outstanding(OwnedClass::SparedRenderer),
        OWNED_LEDGER_VIOLATIONS.load(Ordering::SeqCst),
        GX_CMD_QUEUE_MAX_FILL.load(Ordering::SeqCst),
        fmt_lowwater(GX_CMD_ARENA_MIN_REMAINING.load(Ordering::SeqCst)),
        GX_CMD_QUEUE_SUBMITS.load(Ordering::SeqCst),
        gx_cmd_queue_hist_top(8),
        gx_cmd_queue_bucket_summary()
    ));
}

/// Fabricated gamepad wButtons for a phase that issues a FIXED list of button edges ONCE, in order,
/// then holds. `tick` is phase-local; each edge occupies one `INJECT_NAV_CYCLE` (the RE-grounded
/// edge hold+gap -- edge-triggered menu nav needs a multi-frame hold to register one step). Returns
/// `(wButtons_this_frame, holding)`: `holding` is true once every edge has been issued, so the
/// caller waits on an OBSERVED transition (never a timer or budget) to advance.
fn sq_repro_edges(tick: usize, edges: &[u16]) -> (u16, bool) {
    let edge_index = tick / INJECT_NAV_CYCLE;
    if edge_index >= edges.len() {
        return (0, true);
    }
    let asserted = (tick % INJECT_NAV_CYCLE) < INJECT_NAV_TAP_LEN;
    (if asserted { edges[edge_index] } else { 0 }, false)
}

/// SELF-DRIVEN System->Quit->Load-Profile REPRO AUTOPILOT tick (gated by `system_quit_repro_enabled`).
/// Runs every game-task frame. The input block stays engaged in-world (see `block_input_enabled`) so
/// the fabricated gamepad is the ONLY input and no human press can contaminate the repro. Drives the
/// user's EXACT Xbox controller sequence by writing `SQ_REPRO_XINPUT_BUTTONS` (read by the XInput
/// poll hook -- the stage the game reads a gamepad from), advancing ONLY on observed menu-window /
/// cursor / activate transitions (never timers or tap budgets):
///   START -> IngameTop; UP,A -> OptionSetting; LB,DOWN,A -> ProfileSelect; one DOWN/UP off the
///   current save; A,A -> load armed -> DONE (block released; native pump drives return-title +
///   reload). Each phase issues its KNOWN edges once then HOLDS; a genuinely missed edge self-
///   reports (stuck waiting) instead of being papered over by a re-tap.
pub(crate) unsafe fn system_quit_repro_tick() {
    if !system_quit_repro_enabled() {
        return;
    }
    let state = SQ_REPRO_STATE.load(Ordering::SeqCst);
    if state == SQ_REPRO_STATE_DONE {
        return;
    }
    // Driven entirely via the XInput poll hook; keep the DInput keyboard stamp clear every frame so
    // no stale key leaks while the block zeroes the real keyboard.
    crate::input_blocker::InputBlocker::get_instance().set_injected_key(DIK_NONE);
    let set_pad = |b: u16| SQ_REPRO_XINPUT_BUTTONS.store(b as usize, Ordering::SeqCst);
    let tick = SQ_REPRO_STATE_TICK.fetch_add(1, Ordering::SeqCst);

    match state {
        SQ_REPRO_STATE_WAIT_WORLD => {
            set_pad(0);
            let in_world = IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES;
            if in_world && tick >= SQ_REPRO_WORLD_SETTLE_TICKS {
                sq_repro_begin_switch();
                append_autoload_debug(format_args!(
                    "sq-repro: in-world settled ({SQ_REPRO_WORLD_SETTLE_TICKS} ticks) -> OPEN_MENU switch #{}/{} target_slot={}; START (XInput 0x{XINPUT_GAMEPAD_START:04x}) to open the escape/system menu",
                    SQ_REPRO_SWITCH_INDEX.load(Ordering::SeqCst) + 1,
                    sq_repro_target_switches(),
                    sq_repro_target_slot()
                ));
                sq_repro_transition(SQ_REPRO_STATE_OPEN_MENU);
            } else if !in_world {
                // Not in-world yet (boot autoload still loading): hold the settle counter at 0.
                SQ_REPRO_STATE_TICK.store(0, Ordering::SeqCst);
            }
        }
        SQ_REPRO_STATE_OPEN_MENU => {
            let ingame_top = SYSTEM_QUIT_INGAME_TOP_WINDOW.load(Ordering::SeqCst);
            if ingame_top != 0 {
                append_autoload_debug(format_args!(
                    "sq-repro: IngameTop opened window=0x{ingame_top:x} (escape/system menu) -> TO_SYSTEM (UP, A into the quit submenu)"
                ));
                set_pad(0);
                sq_repro_transition(SQ_REPRO_STATE_TO_SYSTEM);
                return;
            }
            let (btn, holding) = sq_repro_edges(tick, &[XINPUT_GAMEPAD_START]);
            if holding {
                sq_repro_waiting_once("OPEN_MENU: START issued, waiting for 02_000_IngameTop");
            }
            set_pad(btn);
        }
        SQ_REPRO_STATE_TO_SYSTEM => {
            let option_setting = SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst);
            if option_setting != 0 {
                append_autoload_debug(format_args!(
                    "sq-repro: OptionSetting opened window=0x{option_setting:x} (quit submenu) -> TO_PROFILE (LB, DOWN, A to activate the cloned Load-Profile row)"
                ));
                set_pad(0);
                sq_repro_transition(SQ_REPRO_STATE_TO_PROFILE);
                return;
            }
            let (btn, holding) = sq_repro_edges(tick, &[XINPUT_GAMEPAD_DPAD_UP, XINPUT_GAMEPAD_A]);
            if holding {
                sq_repro_waiting_once("TO_SYSTEM: UP+A issued, waiting for 02_040_OptionSetting");
            }
            set_pad(btn);
        }
        SQ_REPRO_STATE_TO_PROFILE => {
            let profile = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
            if profile != 0 {
                append_autoload_debug(format_args!(
                    "sq-repro: ProfileSelect opened window=0x{profile:x} (cloned Load-Profile row activated) -> TO_SLOT (move cursor off the current save)"
                ));
                set_pad(0);
                SQ_REPRO_INITIAL_CURSOR.store(usize::MAX, Ordering::SeqCst);
                sq_repro_transition(SQ_REPRO_STATE_TO_SLOT);
                return;
            }
            let (btn, holding) = sq_repro_edges(
                tick,
                &[
                    XINPUT_GAMEPAD_LEFT_SHOULDER,
                    XINPUT_GAMEPAD_DPAD_DOWN,
                    XINPUT_GAMEPAD_A,
                ],
            );
            if holding {
                sq_repro_waiting_once(
                    "TO_PROFILE: LB+DOWN+A issued, waiting for 05_010_ProfileSelect",
                );
            }
            set_pad(btn);
        }
        SQ_REPRO_STATE_TO_SLOT => {
            // Drive the ProfileSelect cursor to THIS switch's EXPLICIT target slot (not "one off
            // current"), so switch #2 lands on a real, distinct character regardless of which slot the
            // prior reload made current. DOWN increments the cursor index, UP decrements (verified:
            // switch #1 UP moved cursor 5->4). Recompute the direction each frame so an overshoot
            // self-corrects. Stop + CONFIRM when the cursor equals the target.
            let dialog = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
            let cursor = if dialog != 0 && dialog != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_i32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }.unwrap_or(-1)
            } else {
                -1
            };
            let target = sq_repro_target_slot();
            if cursor < 0 {
                // ProfileSelect not fully built yet; hold neutral.
                set_pad(0);
                return;
            }
            if cursor == target {
                append_autoload_debug(format_args!(
                    "sq-repro: ProfileSelect cursor={cursor} == target_slot={target} (switch #{}) -> CONFIRM (A)",
                    SQ_REPRO_SWITCH_INDEX.load(Ordering::SeqCst) + 1
                ));
                set_pad(0);
                sq_repro_transition(SQ_REPRO_STATE_CONFIRM);
                return;
            }
            let dir = if cursor < target {
                XINPUT_GAMEPAD_DPAD_DOWN
            } else {
                XINPUT_GAMEPAD_DPAD_UP
            };
            // Step one clean edge per INJECT_NAV_CYCLE toward the target (tap then gap = one cursor
            // step); keep stepping until cursor == target. No fixed edge budget -- advance on the
            // observed cursor value, so a missed step just re-issues next cycle.
            let btn = if (tick % INJECT_NAV_CYCLE) < INJECT_NAV_TAP_LEN {
                dir
            } else {
                INJECT_NAV_NO_BUTTONS
            };
            if tick % (INJECT_NAV_CYCLE * 8) == 0 {
                append_autoload_debug(format_args!(
                    "sq-repro: TO_SLOT stepping cursor={cursor} -> target_slot={target} dir=0x{dir:04x}"
                ));
            }
            set_pad(btn);
        }
        SQ_REPRO_STATE_CONFIRM => {
            // The user's pick: ONE A on the highlighted slot. The activate hook direct-arms the
            // save-safe switch and native cancel-closes ProfileSelect (the product path -- no confirm
            // MessageBox exists; the suppression eats it before UI allocation). DONE is gated on the
            // arm being OBSERVED: the arm advances SYSTEM_QUIT_QUICKLOAD_PHASE past IDLE (phase is
            // reliably IDLE on CONFIRM entry -- continue_confirm resets it at each reload's commit,
            // long before WAIT_RELOAD's load_done+settle gates admit the next switch). The legacy
            // confirm-count predicate is kept as a fallback for the direct-arm-did-not-take native
            // forward (confirm box -> OK -> load-job chain). On either signal, release the pad; the
            // native pump drives the return-title -> autoload.
            let armed = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
                != SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE;
            if armed || sq_repro_confirm_count() > SQ_REPRO_CONFIRM_BASELINE.load(Ordering::SeqCst)
            {
                let switch_index = SQ_REPRO_SWITCH_INDEX.load(Ordering::SeqCst);
                let more = switch_index + 1 < sq_repro_target_switches();
                append_autoload_debug(format_args!(
                    "sq-repro: switch #{}/{} load CONFIRMED via {} (confirmed_block={} confirmed_allow={} activate={} baseline={}). {}",
                    switch_index + 1,
                    sq_repro_target_switches(),
                    if armed {
                        "direct-arm"
                    } else {
                        "OK-confirm chain"
                    },
                    SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_BLOCK_COUNT.load(Ordering::SeqCst),
                    SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ALLOW_COUNT.load(Ordering::SeqCst),
                    SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_COUNT.load(Ordering::SeqCst),
                    SQ_REPRO_CONFIRM_BASELINE.load(Ordering::SeqCst),
                    if more {
                        "native pump drives return-title + reload; then WAIT_RELOAD -> next switch"
                    } else {
                        "SELF-DRIVE COMPLETE; releasing block, native pump drives return-title + autoload"
                    }
                ));
                set_pad(0);
                if more {
                    sq_repro_transition(SQ_REPRO_STATE_WAIT_RELOAD);
                } else {
                    SQ_REPRO_STATE.store(SQ_REPRO_STATE_DONE, Ordering::SeqCst);
                }
                return;
            }
            let (btn, holding) = sq_repro_edges(tick, &[XINPUT_GAMEPAD_A]);
            if holding {
                sq_repro_waiting_once(
                    "CONFIRM: A (pick) issued, waiting for direct-arm/load-confirm",
                );
            }
            set_pad(btn);
        }
        SQ_REPRO_STATE_WAIT_RELOAD => {
            // Between two back-to-back switches. Hold neutral while THIS switch's reload runs
            // (return-title tears down the old world, clean-title continue_confirm drives the fresh
            // picked-slot deserialize, SetState5 streams the new world). Advance to the next switch
            // only once the reload has COMMITTED (fresh-deser count reached this switch's number) AND
            // the NEW world is up (local player present) AND it has settled. Settle is counted from
            // the moment both hold (tick reset while the world is still down/loading), so it settles
            // the NEW world, not the residual old one.
            set_pad(0);
            let switch_index = SQ_REPRO_SWITCH_INDEX.load(Ordering::SeqCst);
            let expected_deser = switch_index + 1;
            let deser = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(Ordering::SeqCst);
            let player_up = unsafe { PlayerIns::local_player_mut() }.is_ok();
            // PlayerIns-present alone is a LOADING-SCREEN/TITLE FALSE POSITIVE (PlayerIns exists during
            // the reload before the world is interactive). Require the reload committed (fresh-deser),
            // the player present, AND the new load COMPLETE.
            //
            // STALL FIX (2026-07-03, autostep10 run: switch #1 hung here 21 min): `now_loading_active`
            // was used with INVERTED polarity. Despite its name it is a load-COMPLETE latch (RE-corrected
            // 2026-07-02): it reads FALSE while the map streams and flips TRUE when the load finishes,
            // then LINGERS true in gameplay. The old gate treated now_loading==true as "still on a loading
            // screen" and held -- so the instant switch #1's load completed (latch true) it hung forever.
            // Correct polarity (matches composite_portrait_inner's `loading = !load_done`): the world is
            // still loading while the latch is FALSE, done when it is TRUE. Advance only when the latch is
            // TRUE (load done), the fresh-deser count reached this switch, the player is up, and the cover
            // is gone. The lingering-true-from-the-previous-load risk is covered by fresh_deser (must
            // reach THIS switch's count) plus the settle wait below.
            let base = game_rva(0).unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let base_ok = base != TITLE_OWNER_SCAN_START_ADDRESS;
            let load_done = base_ok && unsafe { now_loading_active(base) };
            let fake_cover = base_ok && unsafe { fake_loading_screen_visible(base) };
            let loading = !base_ok || !load_done || fake_cover;
            if deser < expected_deser || !player_up || loading {
                // Still tearing down / at title / streaming: hold the settle clock at 0 so it starts
                // only when the NEW world is up AND interactive (not a loading-screen false positive).
                SQ_REPRO_STATE_TICK.store(0, Ordering::SeqCst);
                // Periodic GATE dump (er-effects-rs-qwj): this state once stalled with switch #1
                // stable and fresh-deser == expected, so one of these gates was lying. Name the
                // culprit with data, not a single opaque waiting line.
                let waited = SQ_REPRO_WAIT_RELOAD_FRAMES.fetch_add(1, Ordering::SeqCst);
                if waited % SQ_REPRO_WAIT_RELOAD_LOG_EVERY == 0 {
                    append_autoload_debug(format_args!(
                        "sq-repro: WAIT_RELOAD gates (switch #{}/{} waited_frames={waited}): fresh_deser={deser}/{expected_deser} player_up={player_up} load_done={load_done} fake_cover={fake_cover}",
                        switch_index + 1,
                        sq_repro_target_switches()
                    ));
                }
                return;
            }
            if tick >= SQ_REPRO_WORLD_SETTLE_TICKS {
                let next = switch_index + 1;
                SQ_REPRO_SWITCH_INDEX.store(next, Ordering::SeqCst);
                sq_repro_begin_switch();
                append_autoload_debug(format_args!(
                    "sq-repro: switch #{}/{} reload committed (fresh_deser={deser}) + new world settled -> arming switch #{}/{} target_slot={}; OPEN_MENU",
                    switch_index + 1,
                    sq_repro_target_switches(),
                    next + 1,
                    sq_repro_target_switches(),
                    sq_repro_target_slot()
                ));
                sq_repro_transition(SQ_REPRO_STATE_OPEN_MENU);
            }
        }
        _ => {
            set_pad(0);
        }
    }
}

pub(crate) unsafe extern "system" fn system_quit_profile_load_confirmed_hook(
    action_obj: usize,
) -> usize {
    let orig = SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: ProfileLoadDialog confirmed-load trampoline unset for action=0x{action_obj:x} -- fail-closed return 0"
        ));
        return 0;
    }
    let original: unsafe extern "system" fn(usize) -> usize = unsafe { std::mem::transmute(orig) };
    let dialog =
        unsafe { safe_read_usize(action_obj + 0x8) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let profile_window = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    let system_quit_profile_active = dialog != TITLE_OWNER_SCAN_START_ADDRESS
        && profile_window != 0
        && dialog == profile_window
        && SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) != 0;
    if !system_quit_profile_active {
        return unsafe { original(action_obj) };
    }

    if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst) >= SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED
        && SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_BLOCK_COUNT.load(Ordering::SeqCst) != 0
    {
        // Close ProfileSelect via the NATIVE cancel/back close (FUN_1407ac980: SetResult(Failed) +
        // window close vmethod) instead of arming the confirm-LOAD. Arming the load (writing
        // load_job_ctx+0x14c=2, the coupled Success-close path) makes the game enter an IN-WORLD
        // load/warp transition (GameMan.saveState/b80 -> 2 -> DoSaveStuff). Even with the actual
        // deserialize skipped by the FUN_14067b290 guard, that half-started transition sticks the game
        // at a loading screen and BLOCKS the return-title chain from ever running (observed 2026-07-01:
        // stuck, return_title functor_call_count=0, save_state=3, player still present). The cancel-close
        // pops the ProfileSelect window WITHOUT starting any load, so the menu-pump return-title chain
        // tears the world down cleanly and the autoload loads the picked slot at a clean title. This
        // runs in menu-pump ownership (this IS the native confirm callback) and one-shot -- not the racy
        // game-task tick. See bd system-quit-load-profile-6runs-state-2026-07-01.
        let load_job_ctx = unsafe { safe_read_usize(dialog + 0x1cc8) }.unwrap_or(0);
        if dialog != 0 && dialog != TITLE_OWNER_SCAN_START_ADDRESS {
            match game_rva(SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA) {
                Ok(close_addr) => {
                    let close_fn: unsafe extern "system" fn(usize) =
                        unsafe { std::mem::transmute(close_addr) };
                    unsafe { close_fn(dialog) };
                    SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_COUNT.fetch_add(1, Ordering::SeqCst);
                }
                Err(_) => append_autoload_debug(format_args!(
                    "system-quit-dup: confirm cancel-close ABORT -- failed to resolve close rva 0x{SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA:x}"
                )),
            }
        }
        SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-dup: ProfileSelect confirm CANCEL-CLOSED action=0x{action_obj:x} dialog=0x{dialog:x} load_job_ctx=0x{load_job_ctx:x}; NO load-mode armed -> no in-world load transition -> return-title tears down + autoload loads at clean title"
        ));
        return 0;
    }

    SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: ProfileSelect confirmed-load transition ALLOWED action=0x{action_obj:x} dialog=0x{dialog:x}; actual load/deser is guarded at LoadJobContext::Run"
    ));
    unsafe { original(action_obj) }
}

unsafe fn system_quit_arm_quickload_autoload(selected_slot: i32, source: &str) {
    const NO_SLOT: usize = usize::MAX;
    if selected_slot < 0 {
        append_autoload_debug(format_args!(
            "system-quit-quickload: not arming autoload from {source} -- invalid selected_slot={selected_slot}"
        ));
        return;
    }
    let system_dialog = SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.load(Ordering::SeqCst);
    if system_dialog == 0 || system_dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.store(NO_SLOT, Ordering::SeqCst);
        SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-quickload: not arming direct native chain from {source} -- missing preserved original System dialog selected_slot={selected_slot}"
        ));
        return;
    }
    // DISABLED (2026-07-01): the CSGaitemImp deserialize/lookup/finalize guards only ever CORRUPT
    // the gaitem singleton -- emptying gaitemInsTable handles left a garbage non-canonical entry that
    // crashed GetGaitemIns->GetGaitemHandle (live 0x6710c0). They were a doomed attempt to make the
    // in-world load "safe"; we now BLOCK the in-world load-job entirely (see the robust gate in
    // system_quit_profile_load_job_run_hook) and return to title + autoload instead, so no in-world
    // gaitem deserialize should run. Leaving them installed would additionally corrupt the AUTOLOAD's
    // own post-title load whenever it deserializes while phase is still 1..3. Not installing them lets
    // every real deserialize run natively. (Install fns retained for reference / bisecting.)
    // Install the load-ONLY guard so the picked slot is not deserialized into the still-live world
    // when the native confirm arms the load; it forwards the real load at a clean title (autoload).
    install_system_quit_inworld_load_guard();
    // Install the in-world load REQUEST guard: neutralizes the native RequestLoadSlot (FUN_14067b2f0)
    // so GameMan.saveState/b80 never reaches 2 during the switch. This is the TRUE source of the
    // NowLoading transition that froze the menu pump; blocking it here (not reactively) lets the
    // menu-pump-owned return-title chain run + tear the world down. Forwarded at a clean title.
    install_system_quit_request_load_slot_guard();
    // Re-arm the continue_confirm guard's one-shot: the upcoming clean-title confirm must drive a
    // fresh deserialize of THIS switch's picked slot before it streams (the hook itself is installed
    // unconditionally at attach; see install_system_quit_continue_confirm_hook).
    SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.store(0, Ordering::SeqCst);
    // Re-arm the return-title one-shots so EVERY switch (not just the first) tears the world down.
    // Both are consumed by the first switch and never reset otherwise, so a second switch in the same
    // session would skip the native return-title REQUEST (`== 0` gate, sets saveRequested+bc4=1) and
    // the final-functor submit (compare_exchange 0->1 gate), leaving the second switch stuck in-world.
    // Resetting them here (the per-switch arm point) is the durable fix for repeatable switching
    // (er-effects-rs-qwj). SUBMIT_COUNT is intentionally NOT reset: title.rs uses it as a `> 0` enable
    // and it re-increments before the final functor needs it.
    // BISECT 2026-07-02: these two resets regressed even the SINGLE-switch reload (base f59b2af
    // passes, adding them causes a SECOND title bounce after the load / new-game flash). Disabled
    // while isolating; a switch-#2-safe re-arm will be reinstated once the mechanism is understood.
    // SYSTEM_QUIT_QUICKLOAD_RETURN_TITLE_REQUEST_COUNT.store(0, Ordering::SeqCst);
    // SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.store(selected_slot as usize, Ordering::SeqCst);
    // PORTRAIT RETARGET (user 2026-07-03): the user just confirmed a NEW character for load, so the
    // loading-screen portrait should render THAT character, not the one still resident (ac0). Make it
    // before-break: retarget the spare/render to the selected slot (portrait_target_slot now returns
    // it) and RE-ENGAGE the drive (clear the per-window freeze) so the new model renders + gets its
    // depth mask -- but do NOT touch LOADING_BG_PORTRAIT_RGBA / PROFILE_HAVE_KEYED_FRAME, so the prior
    // masked head keeps displaying until the new model's first KEYED frame replaces it (no opaque
    // flash, no blank). Clear the stale spare candidate (captured for the old character before this
    // confirm) so the teardown-spare re-targets the new slot, and drop the depth-mask cache so the new
    // silhouette is computed fresh rather than bridged from the old head.
    PROFILE_SPARE_CANDIDATE.store(0, Ordering::SeqCst);
    PROFILE_SPARE_CANDIDATE_MODEL.store(0, Ordering::SeqCst);
    PROFILE_BAKE_RGBA_CAPTURED.store(0, Ordering::SeqCst);
    invalidate_portrait_depth_mask();
    PROFILE_PORTRAIT_RETARGETS.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "loading-portrait: RETARGET to selected slot {selected_slot} at confirm (make-before-break: drive re-engaged, prior masked head holds until the new keyed frame; source={source})"
    ));
    SYSTEM_QUIT_QUICKLOAD_PHASE.store(SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED, Ordering::SeqCst);
    OWN_STEPPER_SLOT.store(selected_slot, Ordering::SeqCst);
    PRODUCT_AUTOLOAD_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_MENU, Ordering::SeqCst);
    TFC_CONTINUE_FIRED.store(0, Ordering::SeqCst);
    TFC_LOAD_VEC_WAIT_TICKS.store(0, Ordering::SeqCst);
    OWN_STEPPER_MENU_OPENED.store(OWN_STEPPER_MENU_OPENED_NO, Ordering::SeqCst);
    TITLE_ACCEPT_BYTE_GATE_FIRED.store(false, Ordering::SeqCst);
    TITLE_OWNER_PTR.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    TITLE_OWNER_SCAN_COUNTDOWN.store(TITLE_OWNER_SCAN_COUNTDOWN_READY, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_FIRED.store(false, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_LAST_TITLE_OWNER.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_AUTOLOAD_HANDOFF_COUNT.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_LOAD_JOB_POST_RETURN_TITLE_FIRED.store(0, Ordering::SeqCst);
    PROFILE_REFRESH_KICKED.store(0, Ordering::SeqCst);
    PORTRAIT_RENDER_WINDOW_DONE.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_QUICKLOAD_PHASE.store(
        SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED,
        Ordering::SeqCst,
    );
    append_autoload_debug(format_args!(
        "system-quit-quickload: armed product Continue autoload selected_slot={selected_slot} source={source}; will direct-submit native return-title chain once ProfileSelect closes system_dialog=0x{system_dialog:x}"
    ));
}

pub(crate) unsafe extern "system" fn system_quit_gaitem_finalize_hook(gaitem: usize) {
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase)
    {
        let skips = SYSTEM_QUIT_GAITEM_FINALIZE_SKIP_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        append_autoload_debug(format_args!(
            "system-quit-quickload: CSGaitemImp finalize SKIPPED during return-title transition #{skips} phase={phase} gaitem=0x{gaitem:x}; avoids post-deserialize singleton-state assert while native return-title job advances"
        ));
        return;
    }
    SYSTEM_QUIT_GAITEM_FINALIZE_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_GAITEM_FINALIZE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-quickload: CSGaitemImp finalize trampoline unset phase={phase} gaitem=0x{gaitem:x}; fail-closed skip"
        ));
        return;
    }
    let original: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(orig) };
    unsafe { original(gaitem) };
}

pub(crate) unsafe extern "system" fn system_quit_gaitem_lookup_hook(
    gaitem: usize,
    out_handle: usize,
    in_handle: usize,
) -> usize {
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase)
    {
        if out_handle != 0 && out_handle != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *(out_handle as *mut u32) = 0 };
        }
        let empties = SYSTEM_QUIT_GAITEM_LOOKUP_EMPTY_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if empties <= 16 || empties % 64 == 0 {
            let input = unsafe { safe_read_i32(in_handle) }.unwrap_or(0) as u32;
            append_autoload_debug(format_args!(
                "system-quit-quickload: CSGaitemImp lookup EMPTIED during return-title transition #{empties} phase={phase} gaitem=0x{gaitem:x} out=0x{out_handle:x} in=0x{in_handle:x} input=0x{input:x}; avoids ChrAsm equipment lookup assert while stream remains consumed"
            ));
        }
        return out_handle;
    }
    SYSTEM_QUIT_GAITEM_LOOKUP_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_GAITEM_LOOKUP_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-quickload: CSGaitemImp lookup trampoline unset phase={phase} gaitem=0x{gaitem:x}; returning empty"
        ));
        if out_handle != 0 && out_handle != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *(out_handle as *mut u32) = 0 };
        }
        return out_handle;
    }
    let original: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { original(gaitem, out_handle, in_handle) }
}

/// Guard on the load-only routine `FUN_14067b380(slot)`. While the in-world System->Quit->Load-Profile
/// transition is active (phase in CONFIRMED..AUTOLOAD_HANDOFF) AND the old world is still up (local
/// player present), skip the deserialize+warp and report success -- so `DoSaveStuff` completes (clears
/// its pending slot) and ProfileSelect closes, but nothing loads into the live world. At a clean title
/// (player absent, or phase past the transition) it forwards to the real load so the autoload works.
pub(crate) unsafe extern "system" fn system_quit_inworld_load_skip_hook(slot: i32) -> usize {
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    let in_transition = (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED
        ..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase);
    let world_up = unsafe { PlayerIns::local_player_mut() }.is_ok();
    if in_transition && world_up {
        let n = SYSTEM_QUIT_INWORLD_LOAD_SKIP_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        append_autoload_debug(format_args!(
            "system-quit-quickload: in-world load SKIPPED #{n} slot={slot} phase={phase} (old world still up) -- ProfileSelect close proceeds; return-title tears down; autoload loads at clean title"
        ));
        // FUN_14067b380 returns 1 on success; report success without deserializing so DoSaveStuff's
        // caller advances (it then clears MoveMapStep+0x12c) instead of retrying the in-world load.
        return 1;
    }
    SYSTEM_QUIT_INWORLD_LOAD_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_INWORLD_LOAD_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return 0;
    }
    let original: unsafe extern "system" fn(i32) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { original(slot) }
}

pub(crate) fn install_system_quit_inworld_load_guard() {
    if SYSTEM_QUIT_INWORLD_LOAD_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MH_Initialize for in-world load guard failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_INWORLD_LOAD_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve in-world load rva 0x{SYSTEM_QUIT_INWORLD_LOAD_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_inworld_load_skip_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_INWORLD_LOAD_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-quickload: queue_enable in-world load guard failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_INWORLD_LOAD_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-quickload: hooked in-world load routine 0x{addr:x}; picked-slot deserialize skipped while old world up, forwarded at clean title"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: MH_ApplyQueued in-world load guard failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-quickload: MhHook::new in-world load guard failed: {status:?}"
        )),
    }
}

/// Guard on the native in-world load REQUEST `CS::GameMan::RequestLoadSlot(slot)` (FUN_14067b2f0, live
/// 0x67b200). This is the TRUE source of GameMan.saveState/b80=2 for an explicit-slot in-world load:
/// the per-frame MoveMapStep load steps call it once the confirmed ProfileSelect chain pushes the map
/// machine into loading, and it sets saveState=2, which starts the 02_904_NowLoading transition that
/// freezes the menu pump so the queued return-title chain can never run. During the in-world
/// System->Quit->Load-Profile transition (phase active AND old world still up / local player present)
/// we return "not armed" (0) WITHOUT calling the original, so saveState never reaches 2: no NowLoading,
/// the pump keeps running, and the menu-pump-owned return-title chain tears the world down. Once the
/// world is gone (player absent) or the switch is idle, we forward to the real request -- so the
/// clean-title autoload and any normal load work. The boot/Continue autoload uses the distinct sentinel
/// variants (FUN_14067b290 slot 10 / FUN_14067b570 slot 0xb), which this hook does not touch. See bd
/// system-quit-loadjob-success-commits-phantom-load-2026-07-01.
pub(crate) unsafe extern "system" fn system_quit_request_load_slot_hook(slot: u32) -> usize {
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    // Range-gate like the sibling system_quit_inworld_load_skip_hook (NOT `!= IDLE`): the clean-title
    // reload runs at AUTOLOAD_HANDOFF and re-creates a present player, so a `!= IDLE` gate would
    // neutralize the RELOAD's own RequestLoadSlot mid-load. Neutralize only during the first-world
    // transition [CONFIRMED, AUTOLOAD_HANDOFF); forward natively at AUTOLOAD_HANDOFF so the reload loads.
    let switch_active = (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED
        ..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase);
    let world_up = unsafe { PlayerIns::local_player_mut() }.is_ok();
    if switch_active && world_up {
        let n = SYSTEM_QUIT_REQUEST_LOAD_SLOT_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 8 || n % 120 == 0 {
            append_autoload_debug(format_args!(
                "system-quit-quickload: in-world load REQUEST neutralized #{n} slot={slot} phase={phase} (old world still up) -- saveState/b80 kept idle so no NowLoading; return-title tears down + autoload loads at clean title"
            ));
        }
        // RequestLoadSlot returns 0 when it declines to arm (saveState!=0 or profile check fails). We
        // return the same "not armed" result so the caller MoveMapStep treats it as no-load-yet instead
        // of entering the in-world load transition.
        return 0;
    }
    SYSTEM_QUIT_REQUEST_LOAD_SLOT_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_REQUEST_LOAD_SLOT_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return 0;
    }
    let original: unsafe extern "system" fn(u32) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { original(slot) }
}

pub(crate) fn install_system_quit_request_load_slot_guard() {
    if SYSTEM_QUIT_REQUEST_LOAD_SLOT_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MH_Initialize for RequestLoadSlot guard failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_REQUEST_LOAD_SLOT_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve RequestLoadSlot rva 0x{SYSTEM_QUIT_REQUEST_LOAD_SLOT_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_request_load_slot_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_REQUEST_LOAD_SLOT_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-quickload: queue_enable RequestLoadSlot guard failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_REQUEST_LOAD_SLOT_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-quickload: hooked in-world load request RequestLoadSlot 0x{addr:x}; saveState/b80=2 arm neutralized while old world up, forwarded at clean title"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: MH_ApplyQueued RequestLoadSlot guard failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-quickload: MhHook::new RequestLoadSlot guard failed: {status:?}"
        )),
    }
}

/// Guard on the native title Continue confirm `0x140b0e180` (rcx = the {[+8]=owner} shim; reads
/// GameMan+0xc30 -> owner+0xbc -> SetState(5); picks NO slot). Static RE 2026-07-02 proved the
/// post-switch clean-title reload streams the PRE-SWITCH GameMan/PlayerGameData state: no fresh
/// deserialize of the picked slot runs anywhere on that path, so the resident (original) character
/// gets re-streamed -- the wrong-character bug. While a System->Quit->Load-Profile switch is active
/// this hook drives ONE fresh synchronous feed-deserialize of the PICKED slot
/// (`own_load_feed_deserialize`: on-disk read -> gated 0x67b100 feed -> native parser 0x67b290)
/// BEFORE forwarding, so ac0/c30/PGD all become the picked slot and the confirm streams the right
/// character. Fail-closed: if the fresh deserialize cannot be proven, the confirm is BLOCKED --
/// streaming stale state would load the wrong character and the post-load autosave would then write
/// it back into the picked slot. Boot autoloads and normal play (phase IDLE) pass through
/// untouched. See bd system-quit-cleantitle-load-is-stale-restream-not-slot-source-2026-07-02.
pub(crate) unsafe extern "system" fn system_quit_continue_confirm_hook(
    shim: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    // Continue-trace compat: this unconditional hook replaced the trace-set `cap_continue_confirm`
    // hook on the same address (two MinHooks on one target fail -- the install_c30_writer_hook
    // precedent), so reproduce its logging + confirm latch exactly when tracing is on.
    if trace_continue_enabled() && !continue_trace_disabled() {
        let owner = if shim != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe {
                safe_read_usize(shim + OWN_STEPPER_SHIM_OWNER_IDX * core::mem::size_of::<usize>())
            }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        append_continue_trace(format_args!(
            "CAP continue_confirm this=0x{shim:x} owner=0x{owner:x} {} {}",
            trace_callers_summary(),
            b80_mount_trace_summary()
        ));
        OWN_STEPPER_CONFIRMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    // Inclusive of AUTOLOAD_HANDOFF (unlike the in-world guards' half-open range): the clean-title
    // reload's confirm fires at TITLE_OWNER_SEEN or AUTOLOAD_HANDOFF and the fresh deserialize is
    // exactly what phase 4 needs; the one-shot DONE latch prevents repeats after success.
    let switch_active = (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED
        ..=SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase);
    if switch_active && SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst) == 0 {
        let selected = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
        let world_up = unsafe { PlayerIns::local_player_mut() }.is_ok();
        if world_up {
            // A title-flow confirm while the old world is still up is not a state we ever drive;
            // never deserialize into a live world (that is the crash the whole switch avoids).
            // Forward and log loudly -- the in-world load guards protect the load paths.
            append_autoload_debug(format_args!(
                "system-quit-quickload: continue_confirm called while OLD WORLD STILL UP phase={phase} selected={selected} shim=0x{shim:x} -- forwarding WITHOUT fresh deserialize (unexpected caller)"
            ));
        } else if selected >= TITLE_PROFILE_SLOT_COUNT {
            let n = SYSTEM_QUIT_CONTINUE_CONFIRM_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            append_autoload_debug(format_args!(
                "system-quit-quickload: continue_confirm BLOCKED #{n} -- switch active (phase={phase}) but no valid picked slot ({selected}); refusing to stream stale pre-switch state"
            ));
            return 0;
        } else {
            let slot = selected as i32;
            let base = game_rva(0).unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let gm = game_man_ptr_or_null();
            append_autoload_debug(format_args!(
                "system-quit-quickload: continue_confirm intercepted at clean title phase={phase} -> restore gaitem singleton + fresh feed-deserialize of PICKED slot {slot} before stream (shim=0x{shim:x})"
            ));
            // Release char#1's leaked gaitems back to the free-queue at this clean title (player
            // absent) BEFORE the reload deserialize, else char#2's deserialize exhausts the queue
            // and OOB-dispatches gaitemInsTable[-1] (the AV at live 0x67141a). Native per-item
            // release; declines fail-closed if the singleton looks wrong (then the deserialize may
            // still crash, but we never sweep a bogus pointer).
            if base != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { own_load_reset_gaitem_singleton(base) };
            }
            if base != TITLE_OWNER_SCAN_START_ADDRESS
                && unsafe { own_load_feed_deserialize(base, gm, slot) }
            {
                SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.store(1, Ordering::SeqCst);
                let n = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT
                    .fetch_add(1, Ordering::SeqCst)
                    + 1;
                // LOAD COMMITTED -> get out of the way. The forwarded continue_confirm below fires
                // SetState5, which streams the picked character. Return the switch machine to IDLE so
                // the product-core autoload's switch branch STOPS (title.rs: it keeps arming
                // GameMan+0xb78 = an in-world MoveMapStep load of the slot, and keeps re-driving the
                // title, while phase >= RETURN_TITLE_REQUESTED). Left armed, that redundant b78 load
                // competes with this SetState5 stream, stalls the title owner at state 6, and bounces
                // the freshly-loaded world back to the title ~4s later (the post-load instability the
                // earlier single-switch milestone missed -- it tore down before the bounce). IDLE also
                // makes the in-world load guards inert (they gate on [CONFIRMED, AUTOLOAD_HANDOFF)), so
                // the native world stream is unobstructed, and leaves the session clean for the next
                // switch (also the durable fix for the post-switch hygiene issue er-effects-rs-qwj).
                SYSTEM_QUIT_QUICKLOAD_PHASE
                    .store(SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE, Ordering::SeqCst);
                // CLEAR the stale in-world load arm. product-core armed GameMan+0xb78 = slot MANY
                // times before this confirm (title.rs, phase 3-4). Phase -> IDLE stops FURTHER arming
                // but leaves b78 = slot RESIDENT; once our SetState5 world comes up, the in-world
                // MoveMapStep loader reads that stale b78 and fires a REDUNDANT second load of the same
                // slot -> a second CSGaitemImp::Deserialize with the free-queue already populated by
                // our load -> the 0x67141a exhaustion crash (observed +41705ms). With phase IDLE the
                // in-world guards are inert, so clear b78 to -1 (native "no requested slot") ourselves.
                if gm != TITLE_OWNER_SCAN_START_ADDRESS {
                    unsafe {
                        *((gm + GAME_MAN_REQUESTED_SLOT_B78_OFFSET) as *mut i32) =
                            OWN_STEPPER_SLOT_NONE;
                    }
                }
                // CLEAR the return-title "rebuild the title" request flags the final functor set for
                // this switch's teardown. They are LEVEL flags nothing resets, so once the reloaded
                // world comes up the still-set menuData+0x5d re-requests the quit-to-title
                // (GameMan.save_requested flips true again ~3.6s later, proven by gm-snap) and bounces
                // the freshly-loaded world back to the title. The teardown they were needed for is done
                // by now (we are at the clean-title Continue), so undo them.
                let menu_man = unsafe { safe_read_usize(base + CS_MENU_MAN_GLOBAL_RVA) }
                    .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                if menu_man != TITLE_OWNER_SCAN_START_ADDRESS
                    && unsafe { is_heap_aligned_ptr(menu_man) }
                {
                    if let Some(menu_data) =
                        unsafe { safe_read_usize(menu_man + CS_MENU_MAN_MENU_DATA_OFFSET) }
                    {
                        if menu_data != TITLE_OWNER_SCAN_START_ADDRESS
                            && unsafe { is_heap_aligned_ptr(menu_data) }
                        {
                            unsafe {
                                *((menu_data + CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET)
                                    as *mut u8) = 0;
                            }
                        }
                    }
                }
                unsafe { *((base + RETURN_TITLE_REBUILD_FLAG_DAT_RVA) as *mut u8) = 0 };
                // Also clear GameMan.save_requested defensively (typed): the return-title REQUEST set
                // it for the teardown; a residual true would drive an immediate quit-save on the reload.
                // AND clear GameMan.warp_requested: the fresh full deserialize we just ran (native
                // parser 0x67b290 = dump FUN_14067b380) UNCONDITIONALLY sets warp_requested=true as a
                // "warp reload pending" flag. On the normal in-world load the MoveMapStep warp machine
                // consumes it, but our SetState5 forward is a fresh title->world stream that never does;
                // MoveMapStep::CheckReturnToTitle (dump FUN_140afa7c0) then reads warp_requested==true
                // every frame as a return-to-title trigger and bounces the freshly-loaded world back to
                // the title ~4s later (proven: gm-snap shows warp_requested=true for the whole reloaded
                // world vs false on the healthy boot load). warp_requested=false is the correct in-world
                // steady state, so clearing it matches the boot load and does not affect which char loads.
                if let Ok(gm_typed) = unsafe { eldenring::cs::GameMan::instance_mut() } {
                    er_save_loader::GameManSaveAccess::set_save_requested(gm_typed, false);
                    er_save_loader::GameManSaveAccess::set_warp_requested(gm_typed, false);
                }
                // REPEATABLE-SWITCH STATE RESTORE (er-effects-rs-qwj). The switch-#1 works but
                // switch-#2-stalls symptom is a pure precondition mismatch: these three return-title
                // one-shots are CONSUMED by this switch's teardown and gate the NEXT switch --
                // RETURN_TITLE_REQUEST_COUNT (native return-title REQUEST fires only when ==0,
                // startup_hooks 6922), DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT (menu-pump submit only
                // when ==0, 7162), FINAL_FUNCTOR_CALL_COUNT (final-functor compare_exchange 0->1,
                // title.rs 1690). Left set, switch #2 skips its return-title REQUEST + submit and
                // never tears the world down (observed: stuck at title state 10/10, bc4=0). Restoring
                // them to boot-fresh here makes every switch byte-identical to the first. This is the
                // SAFE edge (unlike the disabled arm-time reset above, which re-fires during teardown
                // and double-submits -> the single-switch bounce that regressed it): it runs once per
                // switch (fresh-deser latch), AFTER this switch's return-title machinery is fully
                // consumed, and alongside phase -> IDLE, so no return-title path reads a 0 count until
                // the next switch arms (all those gates require phase != IDLE).
                SYSTEM_QUIT_QUICKLOAD_RETURN_TITLE_REQUEST_COUNT.store(0, Ordering::SeqCst);
                SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT.store(0, Ordering::SeqCst);
                SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT.store(0, Ordering::SeqCst);
                // Restore the per-switch MENU-WINDOW state to boot-fresh too -- the visual analogue of
                // the one-shots above (er-effects-rs-qwj). These trackers hold this switch's now-destroyed
                // IngameTop/OptionSetting/ProfileSelect windows; left stale, the NEXT switch's quit menu
                // (a) does not hide behind ProfileSelect (the hide keys off a valid tracked window, but
                // the stale pointer's vtable is zeroed on the torn-down window -> hid_top=false, so the
                // quit menu renders on top) and (b) its Quit Game / Return-to-Desktop rows act dead
                // because the menu is layered over a stale ProfileSelect. Resetting here -- the same
                // trackers the autopilot's sq_repro_begin_switch clears before each switch -- makes the
                // next quit-menu open repopulate them fresh via the MenuWindowJob::Run hook, so the hide
                // + input behave identically to the first switch. (Manual B-to-back had the same effect
                // by forcing a fresh window; this makes it automatic.)
                unsafe {
                    system_quit_reset_profile_select_state("post-switch-commit-menu-hygiene")
                };
                SYSTEM_QUIT_INGAME_TOP_WINDOW.store(0, Ordering::SeqCst);
                SYSTEM_QUIT_OPTION_SETTING_WINDOW.store(0, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-quickload: fresh picked-slot deserialize OK #{n} slot={slot} -- forwarding continue_confirm so SetState5 streams; phase -> IDLE + cleared GameMan+0xb78=-1 + cleared return-title rebuild flags (menuData+0x5d, DAT, save_requested, warp_requested) + RESET return-title one-shots (request/submit/final-functor) so the NEXT switch starts boot-fresh (er-effects-rs-qwj repeatable switching)"
                ));
            } else {
                let n = SYSTEM_QUIT_CONTINUE_CONFIRM_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
                append_autoload_debug(format_args!(
                    "system-quit-quickload: continue_confirm BLOCKED #{n} -- fresh deserialize of picked slot {slot} FAILED (see own-load-feed line); refusing to stream stale pre-switch state"
                ));
                return 0;
            }
        }
    }
    SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_CONTINUE_CONFIRM_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { original(shim, b, c, d) }
}

pub(crate) fn install_system_quit_continue_confirm_hook() {
    if SYSTEM_QUIT_CONTINUE_CONFIRM_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MH_Initialize for continue_confirm guard failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(CONTINUE_CONFIRM_RVA as u32) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve continue_confirm rva 0x{CONTINUE_CONFIRM_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_continue_confirm_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_CONTINUE_CONFIRM_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-quickload: queue_enable continue_confirm guard failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_CONTINUE_CONFIRM_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-quickload: hooked title Continue confirm 0x{addr:x}; active switch drives a fresh picked-slot deserialize before SetState5 (fail-closed)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: MH_ApplyQueued continue_confirm guard failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-quickload: MhHook::new continue_confirm guard failed: {status:?}"
        )),
    }
}

/// READ-ONLY trace on `EzChildStepBase::RequestFinish` (`EZ_CHILD_STEP_REQUEST_FINISH_RVA`). The
/// quit-to-title teardown ends the in-world MoveMapStep session through this one-shot; the
/// post-switch reload bounce is the SAME call arriving against the freshly-created MoveMapStep
/// child right after streaming completes. Logs which InGameStep child wrapper is being finished
/// (stay/movemap) plus the first game-image caller RVA, so the stale requester can be identified.
pub(crate) unsafe extern "system" fn system_quit_child_finish_request_hook(wrapper: usize) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let n = SYSTEM_QUIT_CHILD_FINISH_TRACE_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 64 {
            let mut owner = TITLE_OWNER_PTR.load(Ordering::SeqCst);
            if owner == TITLE_OWNER_SCAN_START_ADDRESS {
                owner = TITLE_SETSTATE_TRACE_LAST_OWNER.load(Ordering::SeqCst);
            }
            let ig = if owner != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(owner + TITLE_STEP_IN_GAME_STEP_2E8_OFFSET) }.unwrap_or(0)
            } else {
                0
            };
            let kind = if ig != 0 && wrapper == ig + IN_GAME_STEP_MOVE_MAP_WRAPPER_E0_OFFSET {
                "MOVEMAP-CHILD"
            } else if ig != 0 && wrapper == ig + IN_GAME_STEP_STAY_WRAPPER_B8_OFFSET {
                "stay-child"
            } else {
                "other"
            };
            let child =
                unsafe { safe_read_usize(wrapper + EZ_CHILD_STEP_STEPPER_OFFSET) }.unwrap_or(0);
            let caller_rva = crate::crashlog::trace_first_game_caller_rva();
            append_autoload_debug(format_args!(
                "child-finish-request #{n}: kind={kind} wrapper=0x{wrapper:x} child=0x{child:x} ig=0x{ig:x} caller_rva=0x{caller_rva:x}"
            ));
        }
    }));
    let orig = SYSTEM_QUIT_CHILD_FINISH_TRACE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return;
    }
    let original: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(orig) };
    unsafe { original(wrapper) }
}

pub(crate) fn install_system_quit_child_finish_trace_hook() {
    if SYSTEM_QUIT_CHILD_FINISH_TRACE_INSTALLED.load(Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "child-finish-request: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(EZ_CHILD_STEP_REQUEST_FINISH_RVA) else {
        append_autoload_debug(format_args!(
            "child-finish-request: failed to resolve rva 0x{EZ_CHILD_STEP_REQUEST_FINISH_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_child_finish_request_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_CHILD_FINISH_TRACE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "child-finish-request: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_CHILD_FINISH_TRACE_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "child-finish-request: hooked EzChildStepBase::RequestFinish 0x{addr:x} -- read-only teardown-requester trace armed"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "child-finish-request: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "child-finish-request: MhHook::new failed: {status:?}"
        )),
    }
}

pub(crate) unsafe extern "system" fn system_quit_gaitem_deserialize_hook(
    gaitem: usize,
    input_stream: usize,
) {
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase)
    {
        let skips = SYSTEM_QUIT_GAITEM_DESERIALIZE_SKIP_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if skips <= 8 || skips % 64 == 0 {
            append_autoload_debug(format_args!(
                "system-quit-quickload: CSGaitemImp::Deserialize SKIPPED during return-title transition #{skips} phase={phase} gaitem=0x{gaitem:x} input_stream=0x{input_stream:x}; lets native return-title load job advance without in-world inventory deserialize crash"
            ));
        }
        return;
    }
    SYSTEM_QUIT_GAITEM_DESERIALIZE_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_GAITEM_DESERIALIZE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-quickload: CSGaitemImp::Deserialize trampoline unset phase={phase} gaitem=0x{gaitem:x}; fail-closed skip"
        ));
        return;
    }
    let original: unsafe extern "system" fn(usize, usize) = unsafe { std::mem::transmute(orig) };
    unsafe { original(gaitem, input_stream) };
}

pub(crate) unsafe extern "system" fn system_quit_gameman_load_save_hook(
    game_man: usize,
    save_arg: usize,
    load_kind: u32,
) -> usize {
    let phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    if (SYSTEM_QUIT_QUICKLOAD_PHASE_CONFIRMED..SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF)
        .contains(&phase)
    {
        let blocks = SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        append_autoload_debug(format_args!(
            "system-quit-quickload: GameMan load-save BLOCKED during return-title transition #{blocks} phase={phase} game_man=0x{game_man:x} save_arg=0x{save_arg:x} load_kind=0x{load_kind:x}; prevents in-world CSGaitemImp::Deserialize crash before title rebuild"
        ));
        return 0;
    }
    SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
    let orig = SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-quickload: GameMan load-save trampoline unset phase={phase} game_man=0x{game_man:x}; fail-closed return 0"
        ));
        return 0;
    }
    let original: unsafe extern "system" fn(usize, usize, u32) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { original(game_man, save_arg, load_kind) }
}

pub(crate) unsafe extern "system" fn system_quit_profile_load_job_run_hook(
    job: usize,
    result: usize,
    fd4_time: usize,
    d: usize,
) -> usize {
    let orig = SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-dup: ProfileLoadDialog load-job trampoline unset for job=0x{job:x} -- fail-closed result=0x{result:x}"
        ));
        if result > TITLE_OWNER_SCAN_START_ADDRESS && unsafe { safe_read_usize(result) }.is_some() {
            unsafe {
                *(result as *mut i32) = MENU_JOB_STATE_SUCCESS;
                *((result + 4) as *mut i32) = 0;
            }
        }
        return result;
    }
    let original: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    let profile_window = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    let list = unsafe { safe_read_usize(job + 0x50) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let profile_id = unsafe { safe_read_i32(job + 0x58) }.unwrap_or(-1);
    let context_arg =
        unsafe { safe_read_usize(job + 0x60) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_JOB.store(job, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_LIST.store(list, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_PROFILE_ID.store(profile_id as usize, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_LAST_CONTEXT_ARG.store(context_arg, Ordering::SeqCst);
    // ROBUST block gate: block ANY ProfileLoad job while our injected in-world Load-Profile UI is up
    // (real System windows hidden + our ProfileSelect window present). The prior `list ==
    // profile_window + 0x50` match was fragile: when it failed (observed 2026-07-01), the in-world
    // deserialize ran, our gaitem guards corrupted CSGaitemImp::gaitemInsTable, and it crashed in
    // GetGaitemIns->GetGaitemHandle (live 0x6710c0) BEFORE the per-tick native close could pop
    // ProfileSelect. The only load job that runs while our injected ProfileSelect is showing IS our
    // flow's load, so hidden+profile-present is a sufficient and robust discriminator. `list` is
    // still captured above for telemetry.
    let _ = list;
    let system_quit_profile_active =
        profile_window != 0 && SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) != 0;
    if !system_quit_profile_active {
        return unsafe { original(job, result, fd4_time, d) };
    }

    if system_quit_profile_load_activation_allowed() {
        SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_ALLOW_COUNT.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "system-quit-dup: ProfileSelect load-job Run ALLOWED job=0x{job:x} list=0x{list:x} profile_id={profile_id}; forwarding native load path (known crash risk: CSGaitemImp::Deserialize rva 0x67141a)"
        ));
        return unsafe { original(job, result, fd4_time, d) };
    }

    SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_BLOCK_COUNT.fetch_add(1, Ordering::SeqCst);
    unsafe { system_quit_arm_quickload_autoload(profile_id, "ProfileSelectLoadJobRun") };
    if result > TITLE_OWNER_SCAN_START_ADDRESS && unsafe { safe_read_usize(result) }.is_some() {
        unsafe {
            // Success(2), terminal: the load-job is the SECOND link in the native chain the slot
            // activation submits (msgbox -> loadjob -> confirm-lambda FUN_1409a4ee0). Returning Success
            // lets the chain advance to the confirm-lambda, which our confirm hook cancel-closes
            // (natively pops ProfileSelect) so the menu-pump return-title chain can submit. Returning
            // Failed(3) instead ABORTS the chain -> the confirm-lambda never runs -> ProfileSelect never
            // closes -> return-title never submits (verified live 2026-07-01). The in-world load is NOT
            // committed here: the actual saveState/b80=2 arm is the native RequestLoadSlot FUN_14067b2f0,
            // which system_quit_request_load_slot_hook neutralizes during the switch. See bd
            // system-quit-loadjob-success-commits-phantom-load-2026-07-01.
            *(result as *mut i32) = MENU_JOB_STATE_SUCCESS;
            *((result + 4) as *mut i32) = 0;
        }
    }
    if let Ok(base) = game_module_base() {
        if fd4_time > TITLE_OWNER_SCAN_START_ADDRESS
            && unsafe { safe_read_usize(fd4_time) }.is_some()
        {
            unsafe { *(fd4_time as *mut usize) = base + FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA };
        }
    }
    append_autoload_debug(format_args!(
        "system-quit-dup: ProfileSelect load-job Run BLOCKED save-safe job=0x{job:x} result=0x{result:x} list=0x{list:x} profile_id={profile_id} context_arg=0x{context_arg:x}; returning Success to advance the chain to the confirm cancel-close (in-world saveState=2 arm is blocked at RequestLoadSlot); no captured LoadJob is retained or replayed"
    ));
    result
}

pub(crate) fn install_system_quit_gaitem_finalize_hook() {
    let installed = SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED.load(Ordering::SeqCst);
    if installed == SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED_YES {
        return;
    }
    if installed == SYSTEM_QUIT_GAITEM_FINALIZE_DISABLED {
        let addr = SYSTEM_QUIT_GAITEM_FINALIZE_ADDR.load(Ordering::SeqCst);
        if addr != 0 {
            match unsafe { MH_QueueEnableHook(addr as *mut c_void) } {
                MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
                    MH_STATUS::MH_OK => {
                        SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED
                            .store(SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED_YES, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "system-quit-quickload: re-enabled CSGaitemImp finalize hook 0x{addr:x}; transition finalize skipped until native title handoff"
                        ));
                    }
                    status => append_autoload_debug(format_args!(
                        "system-quit-quickload: MH_ApplyQueued re-enable CSGaitemImp finalize hook failed: {status:?}"
                    )),
                },
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: queue re-enable CSGaitemImp finalize hook failed: {status:?}"
                )),
            }
        }
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MH_Initialize for CSGaitemImp finalize hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_GAITEM_FINALIZE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve CSGaitemImp finalize rva 0x{SYSTEM_QUIT_GAITEM_FINALIZE_RVA:x}"
        ));
        return;
    };
    SYSTEM_QUIT_GAITEM_FINALIZE_ADDR.store(addr, Ordering::SeqCst);
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_gaitem_finalize_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_GAITEM_FINALIZE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-quickload: queue_enable CSGaitemImp finalize hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED
                        .store(SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-quickload: hooked CSGaitemImp finalize 0x{addr:x}; transition finalize skipped until native title handoff"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: MH_ApplyQueued CSGaitemImp finalize hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-quickload: MhHook::new CSGaitemImp finalize hook failed: {status:?}"
        )),
    }
}

pub(crate) fn disable_system_quit_gaitem_finalize_hook(source: &str) {
    if SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED_YES
    {
        return;
    }
    let addr = SYSTEM_QUIT_GAITEM_FINALIZE_ADDR.load(Ordering::SeqCst);
    if addr == 0 {
        return;
    }
    match unsafe { MH_QueueDisableHook(addr as *mut c_void) } {
        MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
            MH_STATUS::MH_OK => {
                SYSTEM_QUIT_GAITEM_FINALIZE_INSTALLED
                    .store(SYSTEM_QUIT_GAITEM_FINALIZE_DISABLED, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-quickload: disabled CSGaitemImp finalize hook 0x{addr:x} before native Continue source={source}"
                ));
            }
            status => append_autoload_debug(format_args!(
                "system-quit-quickload: MH_ApplyQueued disable CSGaitemImp finalize hook failed source={source}: {status:?}"
            )),
        },
        status => append_autoload_debug(format_args!(
            "system-quit-quickload: queue_disable CSGaitemImp finalize hook failed source={source}: {status:?}"
        )),
    }
}

pub(crate) fn install_system_quit_gaitem_lookup_hook() {
    let installed = SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED.load(Ordering::SeqCst);
    if installed == SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED_YES {
        return;
    }
    if installed == SYSTEM_QUIT_GAITEM_LOOKUP_DISABLED {
        let addr = SYSTEM_QUIT_GAITEM_LOOKUP_ADDR.load(Ordering::SeqCst);
        if addr != 0 {
            match unsafe { MH_QueueEnableHook(addr as *mut c_void) } {
                MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
                    MH_STATUS::MH_OK => {
                        SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED
                            .store(SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED_YES, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "system-quit-quickload: re-enabled CSGaitemImp lookup hook 0x{addr:x}; transition equipment handle lookups empty until native title handoff"
                        ));
                    }
                    status => append_autoload_debug(format_args!(
                        "system-quit-quickload: MH_ApplyQueued re-enable CSGaitemImp lookup hook failed: {status:?}"
                    )),
                },
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: queue re-enable CSGaitemImp lookup hook failed: {status:?}"
                )),
            }
        }
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MH_Initialize for CSGaitemImp lookup hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_GAITEM_LOOKUP_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve CSGaitemImp lookup rva 0x{SYSTEM_QUIT_GAITEM_LOOKUP_RVA:x}"
        ));
        return;
    };
    SYSTEM_QUIT_GAITEM_LOOKUP_ADDR.store(addr, Ordering::SeqCst);
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_gaitem_lookup_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_GAITEM_LOOKUP_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-quickload: queue_enable CSGaitemImp lookup hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED
                        .store(SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-quickload: hooked CSGaitemImp lookup 0x{addr:x}; transition equipment handle lookups empty until native title handoff"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: MH_ApplyQueued CSGaitemImp lookup hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-quickload: MhHook::new CSGaitemImp lookup hook failed: {status:?}"
        )),
    }
}

pub(crate) fn disable_system_quit_gaitem_lookup_hook(source: &str) {
    if SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED_YES
    {
        return;
    }
    let addr = SYSTEM_QUIT_GAITEM_LOOKUP_ADDR.load(Ordering::SeqCst);
    if addr == 0 {
        return;
    }
    match unsafe { MH_QueueDisableHook(addr as *mut c_void) } {
        MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
            MH_STATUS::MH_OK => {
                SYSTEM_QUIT_GAITEM_LOOKUP_INSTALLED
                    .store(SYSTEM_QUIT_GAITEM_LOOKUP_DISABLED, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-quickload: disabled CSGaitemImp lookup hook 0x{addr:x} before native Continue source={source}"
                ));
            }
            status => append_autoload_debug(format_args!(
                "system-quit-quickload: MH_ApplyQueued disable CSGaitemImp lookup hook failed source={source}: {status:?}"
            )),
        },
        status => append_autoload_debug(format_args!(
            "system-quit-quickload: queue_disable CSGaitemImp lookup hook failed source={source}: {status:?}"
        )),
    }
}

pub(crate) fn install_system_quit_gaitem_deserialize_hook() {
    let installed = SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED.load(Ordering::SeqCst);
    if installed == SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED_YES {
        return;
    }
    if installed == SYSTEM_QUIT_GAITEM_DESERIALIZE_DISABLED {
        let addr = SYSTEM_QUIT_GAITEM_DESERIALIZE_ADDR.load(Ordering::SeqCst);
        if addr != 0 {
            match unsafe { MH_QueueEnableHook(addr as *mut c_void) } {
                MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
                    MH_STATUS::MH_OK => {
                        SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED.store(
                            SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED_YES,
                            Ordering::SeqCst,
                        );
                        append_autoload_debug(format_args!(
                            "system-quit-quickload: re-enabled CSGaitemImp::Deserialize hook 0x{addr:x}; transition inventory deserialize leaf skipped until native title handoff"
                        ));
                    }
                    status => append_autoload_debug(format_args!(
                        "system-quit-quickload: MH_ApplyQueued re-enable CSGaitemImp::Deserialize hook failed: {status:?}"
                    )),
                },
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: queue re-enable CSGaitemImp::Deserialize hook failed: {status:?}"
                )),
            }
        }
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MH_Initialize for CSGaitemImp::Deserialize hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_GAITEM_DESERIALIZE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve CSGaitemImp::Deserialize rva 0x{SYSTEM_QUIT_GAITEM_DESERIALIZE_RVA:x}"
        ));
        return;
    };
    SYSTEM_QUIT_GAITEM_DESERIALIZE_ADDR.store(addr, Ordering::SeqCst);
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_gaitem_deserialize_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_GAITEM_DESERIALIZE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-quickload: queue_enable CSGaitemImp::Deserialize hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED.store(
                        SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED_YES,
                        Ordering::SeqCst,
                    );
                    append_autoload_debug(format_args!(
                        "system-quit-quickload: hooked CSGaitemImp::Deserialize 0x{addr:x}; transition inventory deserialize leaf skipped until native title handoff"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: MH_ApplyQueued CSGaitemImp::Deserialize hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-quickload: MhHook::new CSGaitemImp::Deserialize hook failed: {status:?}"
        )),
    }
}

pub(crate) fn disable_system_quit_gaitem_deserialize_hook(source: &str) {
    if SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED_YES
    {
        return;
    }
    let addr = SYSTEM_QUIT_GAITEM_DESERIALIZE_ADDR.load(Ordering::SeqCst);
    if addr == 0 {
        return;
    }
    match unsafe { MH_QueueDisableHook(addr as *mut c_void) } {
        MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
            MH_STATUS::MH_OK => {
                SYSTEM_QUIT_GAITEM_DESERIALIZE_INSTALLED
                    .store(SYSTEM_QUIT_GAITEM_DESERIALIZE_DISABLED, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-quickload: disabled CSGaitemImp::Deserialize hook 0x{addr:x} before native Continue source={source}"
                ));
            }
            status => append_autoload_debug(format_args!(
                "system-quit-quickload: MH_ApplyQueued disable CSGaitemImp::Deserialize hook failed source={source}: {status:?}"
            )),
        },
        status => append_autoload_debug(format_args!(
            "system-quit-quickload: queue_disable CSGaitemImp::Deserialize hook failed source={source}: {status:?}"
        )),
    }
}

pub(crate) fn install_system_quit_gameman_load_save_hook() {
    let installed = SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED.load(Ordering::SeqCst);
    if installed == SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED_YES {
        return;
    }
    if installed == SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_DISABLED {
        let addr = SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ADDR.load(Ordering::SeqCst);
        if addr != 0 {
            match unsafe { MH_QueueEnableHook(addr as *mut c_void) } {
                MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
                    MH_STATUS::MH_OK => {
                        SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED.store(
                            SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED_YES,
                            Ordering::SeqCst,
                        );
                        append_autoload_debug(format_args!(
                            "system-quit-quickload: re-enabled GameMan load-save hook 0x{addr:x}; transition loads blocked until native title handoff"
                        ));
                    }
                    status => append_autoload_debug(format_args!(
                        "system-quit-quickload: MH_ApplyQueued re-enable GameMan load-save hook failed: {status:?}"
                    )),
                },
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: queue re-enable GameMan load-save hook failed: {status:?}"
                )),
            }
        }
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-quickload: MH_Initialize for GameMan load-save hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-quickload: failed to resolve GameMan load-save rva 0x{SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_RVA:x}"
        ));
        return;
    };
    SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ADDR.store(addr, Ordering::SeqCst);
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_gameman_load_save_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-quickload: queue_enable GameMan load-save hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED.store(
                        SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED_YES,
                        Ordering::SeqCst,
                    );
                    append_autoload_debug(format_args!(
                        "system-quit-quickload: hooked GameMan load-save 0x{addr:x}; transition loads blocked until native title handoff"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-quickload: MH_ApplyQueued GameMan load-save hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-quickload: MhHook::new GameMan load-save hook failed: {status:?}"
        )),
    }
}

pub(crate) fn disable_system_quit_gameman_load_save_hook(source: &str) {
    if SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED_YES
    {
        return;
    }
    let addr = SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_ADDR.load(Ordering::SeqCst);
    if addr == 0 {
        return;
    }
    match unsafe { MH_QueueDisableHook(addr as *mut c_void) } {
        MH_STATUS::MH_OK => match unsafe { MH_ApplyQueued() } {
            MH_STATUS::MH_OK => {
                SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_INSTALLED
                    .store(SYSTEM_QUIT_GAMEMAN_LOAD_SAVE_DISABLED, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-quickload: disabled GameMan load-save hook 0x{addr:x} before native Continue source={source}"
                ));
            }
            status => append_autoload_debug(format_args!(
                "system-quit-quickload: MH_ApplyQueued disable GameMan load-save hook failed source={source}: {status:?}"
            )),
        },
        status => append_autoload_debug(format_args!(
            "system-quit-quickload: queue_disable GameMan load-save hook failed source={source}: {status:?}"
        )),
    }
}

fn install_system_quit_profile_load_activate_hook() {
    if SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-dup: MH_Initialize for ProfileLoadDialog activation hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve ProfileLoadDialog activation rva 0x{SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_profile_load_activate_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_ORIG
                .store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-dup: queue_enable ProfileLoadDialog activation hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_INSTALLED.store(
                        SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_INSTALLED_YES,
                        Ordering::SeqCst,
                    );
                    append_autoload_debug(format_args!(
                        "system-quit-dup: hooked ProfileLoadDialog activation 0x{addr:x}; injected in-world ProfileSelect can build confirmation dialog"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-dup: MH_ApplyQueued ProfileLoadDialog activation hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-dup: MhHook::new ProfileLoadDialog activation hook failed: {status:?}"
        )),
    }
}

fn install_system_quit_profile_load_confirmed_hook() {
    if SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-dup: MH_Initialize for ProfileLoadDialog confirmed-load hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve ProfileLoadDialog confirmed-load rva 0x{SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_profile_load_confirmed_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_ORIG
                .store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-dup: queue_enable ProfileLoadDialog confirmed-load hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_INSTALLED.store(
                        SYSTEM_QUIT_PROFILE_LOAD_CONFIRMED_INSTALLED_YES,
                        Ordering::SeqCst,
                    );
                    append_autoload_debug(format_args!(
                        "system-quit-dup: hooked ProfileLoadDialog confirmed-load transition 0x{addr:x}; transition is allowed after load-job guard"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-dup: MH_ApplyQueued ProfileLoadDialog confirmed-load hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-dup: MhHook::new ProfileLoadDialog confirmed-load hook failed: {status:?}"
        )),
    }
}

fn install_system_quit_profile_load_job_run_hook() {
    if SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_INSTALLED.load(Ordering::SeqCst)
        != SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-dup: MH_Initialize for ProfileLoadDialog load-job Run hook failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve ProfileLoadDialog load-job Run rva 0x{SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_profile_load_job_run_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_ORIG
                .store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-dup: queue_enable ProfileLoadDialog load-job Run hook failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_INSTALLED.store(
                        SYSTEM_QUIT_PROFILE_LOAD_JOB_RUN_INSTALLED_YES,
                        Ordering::SeqCst,
                    );
                    append_autoload_debug(format_args!(
                        "system-quit-dup: hooked ProfileLoadDialog load-job Run 0x{addr:x}; actual in-world load/deser is blocked by default"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-dup: MH_ApplyQueued ProfileLoadDialog load-job Run hook failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-dup: MhHook::new ProfileLoadDialog load-job Run hook failed: {status:?}"
        )),
    }
}

fn apply_system_quit_multislot_layout_patch() {
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!(
            "system-quit-dup: component-index patch skipped -- module base unavailable"
        ));
        return;
    };
    let target = (base + SYSTEM_QUIT_COMPONENT_INDEX_PATCH_RVA) as *mut u8;
    let existing = unsafe { *target };
    if existing == SYSTEM_QUIT_COMPONENT_INDEX_REPLACEMENT_MULTI_SLOT {
        append_autoload_debug(format_args!(
            "system-quit-dup: component-index patch already applied at 0x{:x} value=0x{existing:x}",
            base + SYSTEM_QUIT_COMPONENT_INDEX_PATCH_RVA
        ));
        return;
    }
    if existing != SYSTEM_QUIT_COMPONENT_INDEX_EXPECTED_GAME_END {
        append_autoload_debug(format_args!(
            "system-quit-dup: component-index patch ABORT -- byte at 0x{:x} is 0x{existing:x}, expected 0x{SYSTEM_QUIT_COMPONENT_INDEX_EXPECTED_GAME_END:x}",
            base + SYSTEM_QUIT_COMPONENT_INDEX_PATCH_RVA
        ));
        return;
    }
    let mut old_protect = PAGE_PROTECT_UNSET;
    let protect_ok = unsafe {
        VirtualProtect(
            target as *mut c_void,
            SYSTEM_QUIT_COMPONENT_INDEX_PATCH_LEN,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if protect_ok == HOOK_FALSE_RETURN as i32 {
        append_autoload_debug(format_args!(
            "system-quit-dup: component-index patch VirtualProtect failed"
        ));
        return;
    }
    unsafe {
        *target = SYSTEM_QUIT_COMPONENT_INDEX_REPLACEMENT_MULTI_SLOT;
    }
    let mut restored = PAGE_PROTECT_UNSET;
    unsafe {
        VirtualProtect(
            target as *mut c_void,
            SYSTEM_QUIT_COMPONENT_INDEX_PATCH_LEN,
            old_protect,
            &mut restored,
        )
    };
    append_autoload_debug(format_args!(
        "system-quit-dup: patched Quit Game component index 0x{:x} 0x{SYSTEM_QUIT_COMPONENT_INDEX_EXPECTED_GAME_END:x}->0x{SYSTEM_QUIT_COMPONENT_INDEX_REPLACEMENT_MULTI_SLOT:x} (multi-slot layout proof)",
        base + SYSTEM_QUIT_COMPONENT_INDEX_PATCH_RVA
    ));
}

/// Install the System -> Quit Game duplicate-button proof hook once. Opt-in only; the detour is a
/// pass-through for every `AddCancelButton` call except the second call from the Quit Game tab
/// builder, where it invokes the original trampoline a second time with the same native args.
pub(crate) fn install_system_quit_duplicate_button_hook() {
    apply_system_quit_multislot_layout_patch();
    install_scaleform_handler_lifecycle_guard();
    // Telemetry-only successor to the removed 5ae3965 overflow guard (dropping command lists on
    // overflow corrupts the render -- c2794d9): never alters queue behavior, only names which
    // producer's submissions grow per switch so the 0x1aeaf05 overflow can be fixed at its source.
    install_gx_cmd_queue_telemetry();
    install_system_quit_menu_window_job_run_hook();
    install_system_quit_window_list_push_hook();
    install_system_quit_noop_action_hook();
    install_system_quit_profile_load_activate_hook();
    install_system_quit_profile_load_confirmed_hook();
    install_system_quit_profile_load_job_run_hook();
    if SYSTEM_QUIT_DUPLICATE_INSTALLED.load(Ordering::SeqCst) != SYSTEM_QUIT_DUPLICATE_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "system-quit-dup: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SYSTEM_QUIT_DUPLICATE_ADD_CANCEL_BUTTON_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: failed to resolve AddCancelButton rva 0x{SYSTEM_QUIT_DUPLICATE_ADD_CANCEL_BUTTON_RVA:x}"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            system_quit_duplicate_add_cancel_button_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SYSTEM_QUIT_DUPLICATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "system-quit-dup: queue_enable AddCancelButton failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SYSTEM_QUIT_DUPLICATE_INSTALLED
                        .store(SYSTEM_QUIT_DUPLICATE_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "system-quit-dup: hooked AddCancelButton 0x{addr:x}; will clone Quit Game row as quick-load from GR_LineHelp:{SYSTEM_QUIT_LOAD_LINEHELP_ID} at caller rva 0x{SYSTEM_QUIT_DUPLICATE_TARGET_RETURN_RVA:x}"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "system-quit-dup: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "system-quit-dup: MhHook::new AddCancelButton failed: {status:?}"
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
