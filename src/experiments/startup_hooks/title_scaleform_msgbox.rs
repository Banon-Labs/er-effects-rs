use std::{
    ffi::{CStr, c_void},
    fmt::Write as _,
    fs,
    path::Path,
    sync::{
        Arc, Mutex, Once, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, UNIX_EPOCH},
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
        UI::{
            Controls::Dialogs::{
                GetOpenFileNameW, OFN_DONTADDTORECENT, OFN_EXPLORER, OFN_FILEMUSTEXIST,
                OFN_HIDEREADONLY, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
            },
            WindowsAndMessaging::{
                ClipCursor, EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
                WM_KEYDOWN, WM_KEYUP,
            },
        },
    },
    core::{BOOL, PCSTR, PCWSTR},
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
/// Runtime-derived stats-panel 05_010_profileselect movie: computed once at first ProfileSelect
/// file-open from the native MemoryFile's vanilla payload, then reused for every later open.
/// Process-lifetime for the same data-pointer-validity reason as the 05_000 buffer above.
static PROFILE_05_010_RUNTIME_EDITED: OnceLock<Vec<u8>> = OnceLock::new();

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

// ENV-GATE RATIONALE: ER_EFFECTS_TITLE_05_000_MEMORY_GFX is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
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
