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

// ===== SAVE-SAFE verify-only OWN-LOAD buffer-feed probe (bd er-effects-rs-lds) ===============
//
// MECHANISM (static-validated 2026-06-22): hook the FSM-gated save read 0x67b100(rcx=out_buf,
// edx=size). When the one-shot gate `OWN_LOAD_GATE` is set, memcpy our sliced PLAINTEXT BND4 slot
// body (from `er_save_loader::bnd4::slot_body`) into out_buf for min(edx, body.len()) bytes and
// return al=1; otherwise call the original. Then call the native parser 0x67b290(slot) in-process
// UNCHANGED -- it allocs the buffer, invokes our hooked 0x67b100 (gets our bytes), and runs the
// REAL native parse (c30 write 0x67bd70 + stream deserialize + char-apply) with zero
// re-implementation. The gate is MANDATORY: 0x67b100 is SHARED with the native menu loader (4
// callers, only one is ours); we must never intercept the menu path. VERIFY is read-back only:
// GameMan+0xc30 (map id) + the PlayerGameData fingerprint. NO SetState5, NO autosave.

/// FSM-gated save read 0x67b100(rcx=out_buf, edx=size) -> al. The leaf read helper our parser
/// 0x67b290 invokes (and the native menu loader -- hence the mandatory gate).
const READ_67B100_RVA: usize = 0x67b100;

/// One-shot gate: true ONLY for the single 0x67b290(slot) call we make from `own_load_drive`. The
/// hook feeds our body + returns al=1 only while this is set; every other (native menu) read passes
/// straight through to the original.
static OWN_LOAD_GATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Trampoline to the original 0x67b100 (set on hook install).
static READ_67B100_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// One-shot install guard for the 0x67b100 detour.
static OWN_LOAD_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// The sliced plaintext slot body the hook feeds: a leaked `&'static [u8]`, exposed to the detour
/// as (ptr, len) atomics so the game-thread detour reads it lock-free. Set BEFORE arming the gate.
static OWN_LOAD_BODY_PTR: AtomicUsize = AtomicUsize::new(0);
static OWN_LOAD_BODY_LEN: AtomicUsize = AtomicUsize::new(0);
/// Count of bytes the gated hook fed into the engine buffer on the latched call (verify telemetry).
static OWN_LOAD_FED_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Gated detour for 0x67b100. While `OWN_LOAD_GATE` is set, copies our sliced plaintext slot body
/// into the engine-allocated out_buf (`rcx`) for `min(size, body.len())` bytes and returns al=1 --
/// the engine then parses OUR bytes instead of reading the FSM-gated iodev resident. Otherwise it
/// is a pure pass-through to the original (the native menu loader's reads are never disturbed).
pub(crate) unsafe extern "system" fn read_67b100_hook(out_buf: usize, size: u32) -> u8 {
    const FEED_SUCCESS_RET: u8 = 1;
    if OWN_LOAD_GATE.load(Ordering::SeqCst) {
        let body_ptr = OWN_LOAD_BODY_PTR.load(Ordering::SeqCst);
        let body_len = OWN_LOAD_BODY_LEN.load(Ordering::SeqCst);
        if out_buf != TITLE_OWNER_SCAN_START_ADDRESS && body_ptr != 0 && body_len != 0 {
            // Data-driven length: copy the smaller of the engine's requested size (its own edx) and
            // our body length -- never assume the 0x280000 literal (bd dont-hardcode-savefile-tied).
            let n = core::cmp::min(size as usize, body_len);
            unsafe {
                std::ptr::copy_nonoverlapping(body_ptr as *const u8, out_buf as *mut u8, n);
            }
            OWN_LOAD_FED_BYTES.store(n, Ordering::SeqCst);
            return FEED_SUCCESS_RET;
        }
    }
    let orig = READ_67B100_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return FEED_SUCCESS_RET;
    }
    let orig: unsafe extern "system" fn(usize, u32) -> u8 = unsafe { std::mem::transmute(orig) };
    unsafe { orig(out_buf, size) }
}

/// Install the gated 0x67b100 detour (MhHook + MH_Initialize + queue_enable + MH_ApplyQueued),
/// mirroring the `install_c30_writer_hook` precedent. Idempotent. The detour is harmless until the
/// gate is armed (pure pass-through), so installing it early is safe.
pub(crate) fn install_own_load_hook() -> bool {
    if OWN_LOAD_HOOK_INSTALLED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC {
        return true;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!("own-load: MH_Initialize failed: {status:?}"));
            return false;
        }
    }
    let Ok(read_addr) = game_rva(READ_67B100_RVA as u32) else {
        append_autoload_debug(format_args!("own-load: failed to resolve 0x67b100 rva"));
        return false;
    };
    match unsafe { MhHook::new(read_addr as *mut c_void, read_67b100_hook as *mut c_void) } {
        Ok(hook) => {
            READ_67B100_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!("own-load: queue_enable failed: {status:?}"));
                return false;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    OWN_LOAD_HOOK_INSTALLED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "own-load: hooked 0x{read_addr:x} (GATED feed of sliced .sl2 body; pass-through until armed)"
                    ));
                    true
                }
                status => {
                    append_autoload_debug(format_args!(
                        "own-load: MH_ApplyQueued failed: {status:?}"
                    ));
                    false
                }
            }
        }
        Err(status) => {
            append_autoload_debug(format_args!("own-load: MhHook::new failed: {status:?}"));
            false
        }
    }
}

// ===== WorldBlockRes::Update DIAGNOSTIC detour (worldblockres-phase-machine-drives-loadstate-to-0xa)
//
// PURPOSE: discriminate WHY the requested map block never reaches loadstate phase 0xa on the menu-free
// OWN-LOAD path. `WorldBlockRes::Update` (deobf 0x140614870) is the per-block phase state-machine
// (switch on the phase byte [this+0x35]); the 9->0xa transition fires ONLY when the FD4 file-load
// completion gate [this+0x2f]!=0. It is ticked per-block-per-frame by the FieldArea/WorldAreaRes
// block-update loop (FUN_14062f840 / FUN_14063a930) which our menu-free path may never run.
//
// The detour is OBSERVE-ONLY: it bumps a call counter, reads the phase byte ([+0x35]) and the gate
// byte ([+0x2f]) via FAULT-TOLERANT reads (never derefs raw), tracks the MAX phase seen and whether
// ANY block's gate was set, then calls the original (trampoline) and returns its return value
// UNCHANGED. No per-call logging (this is high-rate: ~33 blocks * per-frame) -- only atomics.
//
// READS:  wbr_update_calls==0 across the stall  => the FieldArea update loop is NOT ticking (cause 1).
//         calls>0 but max_phase<0xa & any_gate_set=false => loop ticks but the FD4 file-load never
//         completes -> the IO/CSFile path is the gap (cause 2).

/// `CS::WorldBlockRes::Update` real entry (deobf-grounded; the dump entry FUN_1406148e0 is +0x10).
const WORLDBLOCKRES_UPDATE_RVA: usize = 0x614870;
/// Phase byte the switch dispatches on: `this+0x35` (9 -> 0xa is the residency transition).
const WBR_PHASE_35_OFFSET: usize = 0x35;
/// FD4 file-load completion gate: `this+0x2f` (recomputed each tick; !=0 lets phase 9 advance to 0xa).
const WBR_GATE_2F_OFFSET: usize = 0x2f;

/// Total calls to `WorldBlockRes::Update` observed via the detour (per-block-per-frame; 0 == the
/// FieldArea update loop never ticked our block on this path).
pub(crate) static OWN_LOAD_WBR_UPDATE_CALLS: AtomicU64 = AtomicU64::new(0);
/// Max phase byte ([this+0x35]) seen across all observed calls. <0xa across the stall == the block's
/// resource-stream never reached residency.
pub(crate) static OWN_LOAD_WBR_MAX_PHASE: AtomicU64 = AtomicU64::new(0);
/// Whether ANY observed block had its FD4 completion gate ([this+0x2f]) set non-zero. false across the
/// stall == the FD4 file-load never completed for any block (the IO/CSFile gap).
pub(crate) static OWN_LOAD_WBR_ANY_GATE_SET: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// Count of successful OWN-LOAD m28 `AddDefaultFileLoadProcess` dispatch calls (one per cap, one-shot
/// per cap pointer). 0 == the lever never fired. Exposed as telemetry `oracle_own_m28_dispatch_fired`.
pub(crate) static OWN_LOAD_M28_DISPATCH_FIRED: AtomicUsize = AtomicUsize::new(0);
/// One-shot guard: FD4FileCap pointers we already dispatched `AddDefaultFileLoadProcess` for.
/// `AppendFileLoadProcessor` does NOT early-out on an already-present processor, so a double-call
/// would append a second processor -- this set makes each cap fire exactly once. Const-constructible
/// (`Mutex::new(Vec::new())`) so no lazy init is needed.
static OWN_LOAD_M28_DISPATCHED_CAPS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
/// Trampoline to the original `WorldBlockRes::Update` (set on hook install).
static WBR_UPDATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// GROUND-TRUTH cap-layout diagnostic: when `WorldBlockRes::Update`'s `this` (the REAL WBR, straight
/// from the engine) is at the stuck phase 2, dump its candidate cap fields READ-ONLY so we locate the
/// FD4FileCap on the authoritative object instead of reconstructing it from the resmgr container.
/// Throttled to the first few sightings (the hook fires ~500k times).
static WBR_PHASE2_DIAG_CALLS: AtomicUsize = AtomicUsize::new(0);
const WBR_PHASE2_DIAG_MAX: usize = 24;
const WBR_STUCK_PHASE: u8 = 2;
/// One-shot install guard for the `WorldBlockRes::Update` diagnostic detour.
static WBR_UPDATE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);

/// `__fastcall WorldBlockRes::Update(this)` diagnostic detour. `rcx` = WorldBlockRes* (`this`).
/// OBSERVE-ONLY: increments the call counter, fault-tolerantly reads [this+0x35] (phase) and
/// [this+0x2f] (gate), updates the max-phase / any-gate-set atomics, then ALWAYS calls the original
/// and returns its return value unchanged (the fn likely returns void/this; declaring usize and
/// passing through the original's return value is safe for both void and value returns). No load
/// behavior is altered and nothing is written into `this`.
pub(crate) unsafe extern "system" fn wbr_update_hook(this: usize) -> usize {
    OWN_LOAD_WBR_UPDATE_CALLS.fetch_add(1, Ordering::SeqCst);
    if this != TITLE_OWNER_SCAN_START_ADDRESS {
        if let Some(phase) = unsafe { safe_read_u8(this + WBR_PHASE_35_OFFSET) } {
            OWN_LOAD_WBR_MAX_PHASE.fetch_max(u64::from(phase), Ordering::SeqCst);
            // Ground-truth cap-layout dump on the REAL WBR at the stuck phase 2 (throttled, read-only).
            if phase == WBR_STUCK_PHASE {
                let n = WBR_PHASE2_DIAG_CALLS.fetch_add(1, Ordering::SeqCst);
                if n < WBR_PHASE2_DIAG_MAX {
                    let rd = |p: usize| unsafe { safe_read_usize(p) }.unwrap_or(0);
                    let info = rd(this + BLOCK_INNER_8_OFFSET);
                    let area = if info != 0 {
                        unsafe { safe_read_i32(info + BLOCK_AREA_C_OFFSET) }.unwrap_or(-1) & 0xff
                    } else {
                        -1
                    };
                    // Decisive phase-2->3 gate read on the REAL WBR: for BOTH caps, loadState as a
                    // BYTE (the engine does movzbl 0x88; ==4 complete) + the +0x90 byte-count + +0x78
                    // load-process. The block advances 2->3 only when BOTH caps reach loadState 4 with
                    // +0x90 != 0, so this shows exactly which cap is the holdout.
                    let cap0 = rd(this + WORLDBLOCKRES_FILECAP_40_OFFSET);
                    let cap1 = rd(this + WORLDBLOCKRES_FILECAP2_48_OFFSET);
                    let capb = |cap: usize, off: usize| -> i32 {
                        if cap != 0 {
                            unsafe { safe_read_u8(cap + off) }
                                .map(i32::from)
                                .unwrap_or(-1)
                        } else {
                            -1
                        }
                    };
                    let capq = |cap: usize, off: usize| -> usize {
                        if cap != 0 { rd(cap + off) } else { 0 }
                    };
                    let ls0 = capb(cap0, FILECAP_LOADSTATE_88_OFFSET);
                    let ls1 = capb(cap1, FILECAP_LOADSTATE_88_OFFSET);
                    let by0 = capq(cap0, 0x90);
                    let by1 = capq(cap1, 0x90);
                    let lp0 = capq(cap0, FILECAP_LOAD_PROCESS_78_OFFSET);
                    let lp1 = capq(cap1, FILECAP_LOAD_PROCESS_78_OFFSET);
                    let gate2f = unsafe { safe_read_u8(this + WBR_GATE_2F_OFFSET) }.unwrap_or(255);
                    let flag2d = unsafe { safe_read_u8(this + 0x2d) }.unwrap_or(255);
                    append_autoload_debug(format_args!(
                        "wbr-phase2: this=0x{this:x} area=0x{area:x} container=0x{:x} +0x2d={flag2d} +0x2f(gate)={gate2f} cap0=0x{cap0:x} ls0={ls0} by0=0x{by0:x} lp0=0x{lp0:x} | cap1=0x{cap1:x} ls1={ls1} by1=0x{by1:x} lp1=0x{lp1:x} #{n}",
                        rd(this + 0x18)
                    ));
                    // FD4FileCap header sweep (cap0) to locate the requested-resource NAME pointer
                    // (FD4ResCap-style name string near the start) + the +0xa0/+0xa8 fields, so we can
                    // tell whether the load completed empty because the request was built with no/blank
                    // file (our-path bug) or a real .dcx whose archive simply isn't mounted. Also probe
                    // the alt-gate object *(WBR+0x8) and its +0x28. All READ-ONLY.
                    if cap0 != 0 && n < 4 {
                        let info8 = rd(this + 0x08);
                        append_autoload_debug(format_args!(
                            "wbr-phase2-cap0: cap0=0x{cap0:x} +00=0x{:x} +08=0x{:x} +10=0x{:x} +18=0x{:x} +20=0x{:x} +28=0x{:x} +30=0x{:x} +38=0x{:x} +40=0x{:x} +48=0x{:x} +50=0x{:x} +98=0x{:x} +a0=0x{:x} +a8=0x{:x} | info8=0x{info8:x} info8+0x28=0x{:x} #{n}",
                            rd(cap0 + 0x00),
                            rd(cap0 + 0x08),
                            rd(cap0 + 0x10),
                            rd(cap0 + 0x18),
                            rd(cap0 + 0x20),
                            rd(cap0 + 0x28),
                            rd(cap0 + 0x30),
                            rd(cap0 + 0x38),
                            rd(cap0 + 0x40),
                            rd(cap0 + 0x48),
                            rd(cap0 + 0x50),
                            rd(cap0 + 0x98),
                            rd(cap0 + 0xa0),
                            rd(cap0 + 0xa8),
                            if info8 != 0 { rd(info8 + 0x28) } else { 0 }
                        ));
                    }
                }
            }
        }
        if let Some(gate) = unsafe { safe_read_u8(this + WBR_GATE_2F_OFFSET) } {
            if gate != 0 {
                OWN_LOAD_WBR_ANY_GATE_SET.store(true, Ordering::SeqCst);
            }
        }
    }
    let orig = WBR_UPDATE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    let orig: unsafe extern "system" fn(usize) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { orig(this) }
}

/// Install the OBSERVE-ONLY `WorldBlockRes::Update` diagnostic detour (MhHook + MH_Initialize +
/// queue_enable + MH_ApplyQueued), mirroring `install_own_load_hook`. Idempotent. The detour is a
/// pure-read pass-through, so installing it early (when own_load is armed) leaves normal play
/// untouched and never changes load behavior.
pub(crate) fn install_wbr_update_hook() -> bool {
    if WBR_UPDATE_HOOK_INSTALLED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC {
        return true;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!("wbr-update: MH_Initialize failed: {status:?}"));
            return false;
        }
    }
    let Ok(update_addr) = game_rva(WORLDBLOCKRES_UPDATE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "wbr-update: failed to resolve 0x{WORLDBLOCKRES_UPDATE_RVA:x} rva"
        ));
        return false;
    };
    match unsafe { MhHook::new(update_addr as *mut c_void, wbr_update_hook as *mut c_void) } {
        Ok(hook) => {
            WBR_UPDATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!("wbr-update: queue_enable failed: {status:?}"));
                return false;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    WBR_UPDATE_HOOK_INSTALLED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "wbr-update: hooked 0x{update_addr:x} (OBSERVE-ONLY phase/gate diagnostic; pure pass-through)"
                    ));
                    true
                }
                status => {
                    append_autoload_debug(format_args!(
                        "wbr-update: MH_ApplyQueued failed: {status:?}"
                    ));
                    false
                }
            }
        }
        Err(status) => {
            append_autoload_debug(format_args!("wbr-update: MhHook::new failed: {status:?}"));
            false
        }
    }
}

/// Locate the on-disk save file (`.../EldenRing/<steamid>/ER0000.sl2` or `.co2`) and read its bytes.
/// The directory is built by the NATIVE builder 0x140e0e680 (`SAVE_DIR_BUILDER_RVA`) -- the same
/// path the engine uses -- so we never hardcode the user-data/steamid prefix. Inside that directory
/// we pick the save file by extension (`.sl2`/`.co2`) rather than assuming an exact filename, so the
/// probe works for vanilla and Seamless without a hardcoded name (bd dont-hardcode-savefile-tied).
unsafe fn own_load_read_sl2_bytes(base: usize) -> Option<Vec<u8>> {
    const REQ_DIR_SANE_MAX_CU: usize = 320;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Build the canonical save directory into a stack-resident MSVC stateful-allocator u16string
    // wrapper (allocator@+0, data@+0x08, size@+0x18, cap@+0x20) -- identical to the cold-char-mount
    // SAVE-DIR BUILD step, reusing the native builder so the path matches the engine's.
    let mut wrapper = [0u64; 8];
    let wbase = wrapper.as_mut_ptr() as usize;
    let alloc_getter: unsafe extern "system" fn() -> usize =
        unsafe { std::mem::transmute(base + SAVE_DIR_ALLOC_GETTER_RVA) };
    let allocator = unsafe { alloc_getter() };
    unsafe {
        *((wbase + U16STRING_ALLOC_OFFSET) as *mut usize) = allocator;
        *((wbase + U16STRING_CAP_OFFSET) as *mut usize) = U16STRING_SSO_CAP;
    }
    // The builder derefs the Steam interface (*0x143b48ff0) for the account id; bail (logging) if it
    // is null cold (Steam not live) rather than crashing.
    let steam_iface = unsafe { safe_read_usize(base + STEAM_INTERFACE_GUARD_RVA) }.unwrap_or(null);
    if steam_iface == null || allocator == null {
        append_autoload_debug(format_args!(
            "own-load: SAVE-DIR build skipped steam_iface=0x{steam_iface:x} allocator=0x{allocator:x} (need both non-null) -- cannot locate .sl2"
        ));
        return None;
    }
    let builder: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + SAVE_DIR_BUILDER_RVA) };
    unsafe { builder(wbase) };
    let dir_cap = unsafe { *((wbase + U16STRING_CAP_OFFSET) as *const usize) };
    let dir_size = unsafe { *((wbase + U16STRING_SIZE_OFFSET) as *const usize) };
    let dir_data = if dir_cap >= 8 {
        unsafe { *((wbase + U16STRING_DATA_OFFSET) as *const usize) }
    } else {
        wbase + U16STRING_DATA_OFFSET
    };
    // Decode the UTF-16 directory into a Rust path string (fault-safe, bounded).
    let mut dir = String::new();
    if dir_data != null && dir_size != 0 && dir_size <= REQ_DIR_SANE_MAX_CU {
        let words = dir_size.div_ceil(4);
        'decode: for w in 0..words {
            let Some(word) = (unsafe { safe_read_usize(dir_data + w * 8) }) else {
                break;
            };
            for b in 0..4 {
                let cu = ((word >> (b * 16)) & 0xffff) as u16;
                if cu == 0 || w * 4 + b >= dir_size {
                    break 'decode;
                }
                dir.push(char::from_u32(cu as u32).unwrap_or('?'));
            }
        }
    }
    if dir.is_empty() {
        append_autoload_debug(format_args!(
            "own-load: SAVE-DIR builder returned empty (cap={dir_cap} size={dir_size}) -- cannot locate .sl2"
        ));
        return None;
    }
    // The native dir uses backslashes (Windows under Proton); normalise for std::fs lookup.
    let dir_path = PathBuf::from(dir.replace('\\', "/"));
    // Pick the save file by extension, not a hardcoded name: prefer .sl2 (vanilla), then .co2
    // (Seamless). This matches whichever container the active runtime actually wrote.
    let paths: Vec<PathBuf> = std::fs::read_dir(&dir_path)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    let mut chosen: Option<PathBuf> = None;
    for ext in ["sl2", "co2"] {
        if let Some(p) = paths
            .iter()
            .find(|p| p.extension().and_then(|e| e.to_str()) == Some(ext))
        {
            chosen = Some(p.clone());
            break;
        }
    }
    let Some(path) = chosen else {
        append_autoload_debug(format_args!(
            "own-load: no .sl2/.co2 file under dir=\"{}\" -- cannot read save",
            dir_path.display()
        ));
        return None;
    };
    match std::fs::read(&path) {
        Ok(bytes) => {
            append_autoload_debug(format_args!(
                "own-load: read save file \"{}\" ({} bytes) for slicing",
                path.display(),
                bytes.len()
            ));
            Some(bytes)
        }
        Err(e) => {
            append_autoload_debug(format_args!(
                "own-load: failed to read save file \"{}\": {e}",
                path.display()
            ));
            None
        }
    }
}

/// How often (in own_stepper frames) the OWN-LOAD world-stream stall telemetry emits a throttled
/// debug line. The oracle_* atomics are refreshed EVERY frame; only the human-readable log is
/// throttled so a probe log shows the trend without flooding.
pub(crate) const OWN_LOAD_STREAM_LOG_INTERVAL: u64 = 30;
/// MoveMapStep step-machine state field offset (mms_state). The step machine commits its current
/// step at +0x48 (same layout as the title owner committed_state); STEP_WorldResWait == 3 is the
/// observed stall floor. Read [[InGameStep(owner+0x2e8)+0xe8]+0x48].
pub(crate) const MOVEMAPSTEP_STATE_48_OFFSET: usize = TITLE_OWNER_STATE_COMMITTED_OFFSET;

/// SAVE-SAFE per-frame OWN-LOAD world-stream stall telemetry. PURE READS ONLY (safe_read_*; never
/// changes load behavior). Walks the deepest world-load pump chain each frame and publishes the
/// values to the OWN_LOAD_STREAM_* oracle atomics, plus a throttled human-readable debug line, so a
/// probe log reveals whether ANY value advances over time (progress) or all are frozen (genuine
/// stall). Gated to the own_load path only -- the caller invokes this exclusively inside the
/// `own_load_enabled()` branch, so it never spams during normal play.
///
/// Chain (full-pipeline-traced-to-worldreswait-map-block-streaming):
///   title_owner+0x48 = committed/live title state (5 == PlayGame after SetState5)
///   title_owner+0x4c = requested/next title state
///   InGameStep = [title_owner+0x2e8] (load_job); MoveMapStep = [InGameStep+0xe8]
///   mms_state   = [MoveMapStep+0x48]      (STEP_WorldResWait == 3 == the stall floor)
///   resmgr      = [[MoveMapStep+0xf0]+0x10]; block_count = [resmgr+0xb3140]
///   req_coord   = [[MoveMapStep+0xf0]+0x2c]
///   iodev       = [base+IODEV_GLOBAL_RVA]; inflight = [iodev+0x10]; reqhandle = [iodev+0x20]
///   c30         = [gm+0xc30]
pub(crate) unsafe fn own_load_stream_telemetry(base: usize, gm: usize, title_owner: usize, n: u64) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Fault-tolerant deref helper: returns Some(child) only on a real, non-null read.
    let deref = |addr: usize| -> Option<usize> {
        match unsafe { safe_read_usize(addr) } {
            Some(v) if v != null => Some(v),
            _ => None,
        }
    };
    // Title owner state fields (owner+0x48 committed, owner+0x4c requested).
    let owner_state = if title_owner != null {
        unsafe { safe_read_i32(title_owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD)
    } else {
        OWN_LOAD_STREAM_FIELD_UNREAD
    };
    let owner_req_state = if title_owner != null {
        unsafe { safe_read_i32(title_owner + TITLE_OWNER_STATE_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD)
    } else {
        OWN_LOAD_STREAM_FIELD_UNREAD
    };
    // InGameStep (owner+0x2e8) -> MoveMapStep (+0xe8) -> mms_state (+0x48).
    let ingame = if title_owner != null {
        deref(title_owner + TITLE_OWNER_JOB_OFFSET)
    } else {
        None
    };
    let movemapstep = ingame.and_then(|ig| deref(ig + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET));
    let mms_state = match movemapstep {
        Some(mms) => unsafe { safe_read_i32(mms + MOVEMAPSTEP_STATE_48_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    // World-resource manager chain: resmgr = [[MoveMapStep+0xf0]+0x10]; block_count = [resmgr+0xb3140].
    let resmgr = movemapstep
        .and_then(|mms| deref(mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET))
        .and_then(|wrm| deref(wrm + WORLDRES_RESMGR_10_OFFSET));
    let block_count = match resmgr {
        Some(rm) => unsafe { safe_read_i32(rm + RESMGR_BLOCK_COUNT_B3140_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    // Requested world coord/map-id ([[MoveMapStep+0xf0]+0x2c]).
    let req_coord = match movemapstep.and_then(|mms| deref(mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET)) {
        Some(wrm) => unsafe { safe_read_usize(wrm + WORLDRES_COORD_2C_OFFSET) }
            .map(|v| v as i64)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    // IO device inflight / started-request handle.
    let iodev = deref(base + IODEV_GLOBAL_RVA);
    let io_inflight = match iodev {
        Some(dev) => unsafe { safe_read_usize(dev + IODEV_INFLIGHT_10_OFFSET) }
            .map(|v| v as i64)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    let io_reqhandle = match iodev {
        Some(dev) => unsafe { safe_read_usize(dev + IODEV_REQHANDLE_20_OFFSET) }
            .map(|v| v as i64)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    // GameMan+0xc30 saved-map id (the streamed map).
    let c30 = if gm != null {
        unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD)
    } else {
        OWN_LOAD_STREAM_FIELD_UNREAD
    };
    // Publish every frame (the oracle_* fields are the machine-readable progress signal).
    OWN_LOAD_STREAM_OWNER_STATE.store(owner_state, Ordering::SeqCst);
    OWN_LOAD_STREAM_OWNER_REQ_STATE.store(owner_req_state, Ordering::SeqCst);
    OWN_LOAD_STREAM_MMS_STATE.store(mms_state, Ordering::SeqCst);
    OWN_LOAD_STREAM_BLOCK_COUNT.store(block_count, Ordering::SeqCst);
    OWN_LOAD_STREAM_REQ_COORD.store(req_coord, Ordering::SeqCst);
    OWN_LOAD_STREAM_IO_INFLIGHT.store(io_inflight, Ordering::SeqCst);
    OWN_LOAD_STREAM_IO_REQHANDLE.store(io_reqhandle, Ordering::SeqCst);
    OWN_LOAD_STREAM_C30.store(c30, Ordering::SeqCst);
    let frames = OWN_LOAD_STREAM_FRAMES.fetch_add(1, Ordering::SeqCst);
    // Throttled human-readable trend line.
    if frames % OWN_LOAD_STREAM_LOG_INTERVAL == 0 {
        let ig = ingame.unwrap_or(null);
        let mms = movemapstep.unwrap_or(null);
        let rm = resmgr.unwrap_or(null);
        append_autoload_debug(format_args!(
            "own-load-stream: frame={frames} (n={n}) owner_state={owner_state} owner_req={owner_req_state} ingame=0x{ig:x} mms=0x{mms:x} mms_state={mms_state} resmgr=0x{rm:x} block_count={block_count} req_coord=0x{req_coord:x} io_inflight=0x{io_inflight:x} io_reqhandle=0x{io_reqhandle:x} c30=0x{c30:x}"
        ));
    }
}

/// Diagnostic throttle for `own_load_m28_dispatch`: log the first HEAD entries, then every INTERVALth.
static OWN_LOAD_M28_DISPATCH_DIAG_CALLS: AtomicUsize = AtomicUsize::new(0);
const OWN_LOAD_M28_DISPATCH_DIAG_HEAD: usize = 8;
const OWN_LOAD_M28_DISPATCH_DIAG_INTERVAL: usize = 600;

/// OWN-LOAD m28 direct-enqueue lever (adddefaultfileloadprocess-lever-viable-2026-06-22). `block` is
/// the matched player-area (m28, 0x1c) `WorldBlockRes`. For each of its FD4FileCap slots (+0x40 primary,
/// +0x48 optional second) this: skips null caps, skips caps already resident (`loadState +0x88 == 4`),
/// skips caps we already dispatched (one-shot per cap pointer), reads the cap's EXISTING
/// `FD4FileLoadProcess*` at +0x78, then calls `FD4::FD4FileCap::AddDefaultFileLoadProcess(cap, lp)`,
/// which builds the processor internally and self-enqueues IO to the already-live FD4 workers. Every
/// pointer read is fault-tolerant (`deref` / `safe_read_*`) and the native call is wrapped in
/// `catch_unwind` so a fault can never unwind across the FFI boundary into the FD4 task. SAVE-SAFE:
/// reaches only world-asset file-load streaming (RequestDCX -> RSResourceFileRequest ->
/// GLOBAL_LoadManager); it does NOT touch save IO and cannot autosave.
unsafe fn own_load_m28_dispatch(
    base: usize,
    block: usize,
    deref: &impl Fn(usize) -> Option<usize>,
) {
    // Throttled diagnostics: this helper runs once per matched player-area block per observer frame,
    // so logging every frame would flood the log. Log the first few entries and then every
    // OWN_LOAD_M28_DISPATCH_DIAG_INTERVAL-th, plus ALWAYS on an actual dispatch / panic / lp-null.
    // Seeing ANY of these lines proves the gate passed (own_dispatch armed + continue fired) and the
    // helper was entered -- so it disambiguates "gate off" (no line at all) from "caps null/resident"
    // (skip lines), which is exactly why the lever was a silent no-op on the first clean run.
    let diag_n = OWN_LOAD_M28_DISPATCH_DIAG_CALLS.fetch_add(1, Ordering::SeqCst);
    let diag = diag_n < OWN_LOAD_M28_DISPATCH_DIAG_HEAD
        || diag_n % OWN_LOAD_M28_DISPATCH_DIAG_INTERVAL == 0;
    // The resmgr 0xb3030 array entry `block` is a WRAPPER (WorldBlockData), NOT the WorldBlockRes --
    // its +0x40/+0x48 are unrelated wrapper fields (observed null at runtime). The real WorldBlockRes
    // (FD4FileCaps at +0x40/+0x48, phase byte at +0x35) is what the engine reaches via the native
    // getter `block->vtable[+0x10](block)` (canonical scanner 0x14066d3e0; phase handlers 0x1406157f0 /
    // 0x140615340). SAFETY PIVOT (confirmed 2x: process_exited_before_ready @ ~767 game-task ticks with
    // NO diag line written -> the getter CALL itself AV-faulted BEFORE any logging; a hardware AV is not
    // a Rust panic so catch_unwind cannot contain it; matches the prior menu-free getter-fault memory).
    // So we do NOT call the getter. This pass is VERIFY-ONLY / READ-ONLY: capture the getter address
    // (for static disasm) and sweep the wrapper's fields to locate the WorldBlockRes pointer by
    // signature (a field P where P+0x35 is a small phase byte and P+0x40/+0x48 are FD4FileCaps). NO
    // native call is made here, so this cannot crash the game. The real dispatch is re-enabled once the
    // WBR field offset is grounded from this sweep + the getter disassembly.
    let Some(vtbl) = deref(block) else {
        return;
    };
    let getter_addr = deref(vtbl + BLOCK_LOADSTATE_GETTER_VT_10_OFFSET).unwrap_or(0);
    if diag {
        let q = |off: usize| deref(block + off).unwrap_or(0);
        append_autoload_debug(format_args!(
            "own-load-m28-dispatch: VERIFY block=0x{block:x} vtbl=0x{vtbl:x} getter@vt+0x10=0x{getter_addr:x} base=0x{base:x} call#{diag_n}"
        ));
        append_autoload_debug(format_args!(
            "own-load-m28-dispatch: WRAP-SWEEP block=0x{block:x} +00=0x{:x} +08=0x{:x} +10=0x{:x} +18=0x{:x} +20=0x{:x} +28=0x{:x} +30=0x{:x} +38=0x{:x} +40=0x{:x} +48=0x{:x} +50=0x{:x} +58=0x{:x} +60=0x{:x} +68=0x{:x} +70=0x{:x} +78=0x{:x} +80=0x{:x} +88=0x{:x} +90=0x{:x} +98=0x{:x} +a0=0x{:x} +a8=0x{:x}",
            q(0x00),
            q(0x08),
            q(0x10),
            q(0x18),
            q(0x20),
            q(0x28),
            q(0x30),
            q(0x38),
            q(0x40),
            q(0x48),
            q(0x50),
            q(0x58),
            q(0x60),
            q(0x68),
            q(0x70),
            q(0x78),
            q(0x80),
            q(0x88),
            q(0x90),
            q(0x98),
            q(0xa0),
            q(0xa8)
        ));
        // Container layout (decoded from getter 0x14062f470): WorldBlockRes elements live in an inline
        // array at *(block+0xce0), count *(block+0xcd8), stride 0xb98; caps at element+0x40/+0x48. Dump
        // count, array base, and element-0's phase/caps + cap0's loadState(+0x88)/lp(+0x78) READ-ONLY to
        // confirm the layout before enabling the array-iteration dispatch.
        let count =
            unsafe { safe_read_i32(block + WORLDBLOCK_CONTAINER_COUNT_CD8_OFFSET) }.unwrap_or(-1);
        let arr = deref(block + WORLDBLOCK_CONTAINER_ARRAY_CE0_OFFSET).unwrap_or(0);
        let elem0 = arr; // element 0 = arr + 0*0xb98
        let e_phase =
            unsafe { safe_read_i32(elem0 + BLOCK_LOADSTATE_PHASE_35_OFFSET) }.unwrap_or(-1) & 0xff;
        let cap0 = deref(elem0 + WORLDBLOCKRES_FILECAP_40_OFFSET).unwrap_or(0);
        let cap1 = deref(elem0 + WORLDBLOCKRES_FILECAP2_48_OFFSET).unwrap_or(0);
        let c0_ls = unsafe { safe_read_i32(cap0 + FILECAP_LOADSTATE_88_OFFSET) }.unwrap_or(-1);
        let c0_lp = deref(cap0 + FILECAP_LOAD_PROCESS_78_OFFSET).unwrap_or(0);
        let c0_90 = deref(cap0 + 0x90).unwrap_or(0);
        append_autoload_debug(format_args!(
            "own-load-m28-dispatch: CONTAINER block=0x{block:x} count={count} arr=0x{arr:x} stride=0x{stride:x} elem0=0x{elem0:x} elem0_phase=0x{e_phase:x} cap0(+0x40)=0x{cap0:x} cap1(+0x48)=0x{cap1:x} | cap0+0x78(lp)=0x{c0_lp:x} cap0+0x88(loadState)={c0_ls} cap0+0x90=0x{c0_90:x}",
            stride = WORLDBLOCKRES_ELEM_STRIDE_B98
        ));
    }
}

/// SAVE-SAFE RECURRING world-stream observer, called from the per-frame GAME TASK (NOT the
/// title-phase own_stepper_idx10, which stops ticking once SetState5 starts the title->ingame
/// transition). Runs when `OWN_LOAD_CONTINUE_FIRED` (our menu-free OWN-LOAD path) OR
/// `golden_observe_enabled()` (GOLDEN baseline mode observing a user-driven vanilla load) is set, so it
/// never spams during normal play. PURE READS ONLY (safe_read_*; never changes load behavior). In
/// golden mode `OWN_LOAD_OWNER_CACHED` is filled by own_stepper_idx10 each title frame and the cached
/// InGameStep stays 0, so the live `ingame_cached == 0` re-derivation below resolves the chain fresh
/// every frame as the vanilla load builds the world.
///
/// It re-reads the world-stream from the CACHED title owner + InGameStep (snapshotted at fire time),
/// NOT from a fresh own_stepper owner, so it keeps observing through the whole loading screen:
///   owner       = OWN_LOAD_OWNER_CACHED              (cached at continue_confirm fire)
///   InGameStep  = OWN_LOAD_INGAMESTEP_CACHED         (== owner+0x2e8 at fire; non-null at frame 0)
///   MoveMapStep = [InGameStep+0xe8]                  (INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET)
///   mms_state   = [MoveMapStep+0x48]                 (MOVEMAPSTEP_STATE_48_OFFSET; 3 == WorldResWait)
///   resmgr      = [[MoveMapStep+0xf0]+0x10]          (MOVEMAPSTEP_WORLDRES_F0_OFFSET / WORLDRES_RESMGR_10_OFFSET)
///   block_count = [resmgr+0xb3140]                   (RESMGR_BLOCK_COUNT_B3140_OFFSET)
///   owner_state = [owner+0x48]                       (TITLE_OWNER_STATE_COMMITTED_OFFSET)
///   c30         = [gm+0xc30]                          (GAME_MAN_SAVED_MAP_C30_OFFSET)
///   player_present is resolved by the caller (WorldChrMan/PlayerIns) and passed in.
///
/// `frame=N` advances every active frame (OWN_LOAD_STREAM_RECUR_FRAMES) so a probe sees whether
/// mms_state advances/sticks and whether block_count stays 0 vs grows ACROSS the loading screen.
/// Publishes the SAME oracle_own_load_stream_* fields so they keep updating through the load.
pub(crate) unsafe fn own_load_stream_observe_recurring(
    base: usize,
    gm: usize,
    player_present: bool,
) {
    // Run after our own continue_confirm fired (OWN-LOAD path) OR in GOLDEN baseline mode (observing a
    // user-driven vanilla load). Golden mode supplies `owner` via own_stepper_idx10's per-frame cache
    // and leaves the cached InGameStep at 0, so the `ingame_cached == 0` fallback below re-derives the
    // chain LIVE each frame. Either way this stays pure-read and never changes load behavior.
    if !OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst) && !golden_observe_enabled() {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let deref = |addr: usize| -> Option<usize> {
        match unsafe { safe_read_usize(addr) } {
            Some(v) if v != null && v != 0 => Some(v),
            _ => None,
        }
    };
    let owner = OWN_LOAD_OWNER_CACHED.load(Ordering::SeqCst);
    let ingame_cached = OWN_LOAD_INGAMESTEP_CACHED.load(Ordering::SeqCst);
    // owner+0x48 committed state (5 == PlayGame/streaming after SetState5).
    let owner_state = if owner != null && owner != 0 {
        unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD)
    } else {
        OWN_LOAD_STREAM_FIELD_UNREAD
    };
    let owner_req_state = if owner != null && owner != 0 {
        unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD)
    } else {
        OWN_LOAD_STREAM_FIELD_UNREAD
    };
    // Prefer the cached InGameStep; if that snapshot is null/0, re-derive from the cached owner.
    let ingame = if ingame_cached != null && ingame_cached != 0 {
        Some(ingame_cached)
    } else if owner != null && owner != 0 {
        deref(owner + TITLE_OWNER_JOB_OFFSET)
    } else {
        None
    };
    let movemapstep = ingame.and_then(|ig| deref(ig + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET));
    let mms_state = match movemapstep {
        Some(mms) => unsafe { safe_read_i32(mms + MOVEMAPSTEP_STATE_48_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    let resmgr = movemapstep
        .and_then(|mms| deref(mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET))
        .and_then(|wrm| deref(wrm + WORLDRES_RESMGR_10_OFFSET));
    let block_count = match resmgr {
        Some(rm) => unsafe { safe_read_i32(rm + RESMGR_BLOCK_COUNT_B3140_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    let req_coord = match movemapstep.and_then(|mms| deref(mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET)) {
        Some(wrm) => unsafe { safe_read_usize(wrm + WORLDRES_COORD_2C_OFFSET) }
            .map(|v| v as i64)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    let iodev = deref(base + IODEV_GLOBAL_RVA);
    let io_inflight = match iodev {
        Some(dev) => unsafe { safe_read_usize(dev + IODEV_INFLIGHT_10_OFFSET) }
            .map(|v| v as i64)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    let io_reqhandle = match iodev {
        Some(dev) => unsafe { safe_read_usize(dev + IODEV_REQHANDLE_20_OFFSET) }
            .map(|v| v as i64)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    let c30 = if gm != null && gm != 0 {
        unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD)
    } else {
        OWN_LOAD_STREAM_FIELD_UNREAD
    };
    let player_present_i64 = i64::from(player_present);
    // play_game_submit handoff discriminators (PURE READS, no call). InGameStep+0xd8 = pending phase,
    // InGameStep+0x100 = requested BlockId. req_blockid == saved BlockId means play_game_submit ran;
    // 0/unset means it did not. UNREAD if the InGameStep handle is null.
    let ingame_phase = match ingame {
        Some(ig) => unsafe { safe_read_i32(ig + INGAMESTEP_PHASE_D8_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    let req_blockid = match ingame {
        Some(ig) => unsafe { safe_read_usize(ig + INGAMESTEP_REQ_BLOCKID_100_OFFSET) }
            .map(|v| i64::from(v as u32))
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    // resmgr block-array scan (PURE READS, NO block->vtable call this round). The target areaId is
    // DERIVED from req_coord (the low dword's high byte), not hardcoded. We count how many registered
    // blocks match the target (presence == the registration-vs-streaming discriminator) and collect
    // the first OBSERVER_AREAID_SAMPLE_MAX distinct areaIds for the log. Scan is clamped to
    // min(block_count, OBSERVER_BLOCK_SCAN_CAP) and every deref is null/fault-tolerant.
    let target_area: u8 = if req_coord != OWN_LOAD_STREAM_FIELD_UNREAD {
        (((req_coord as u32) >> TARGET_AREA_FROM_COORD_SHIFT) & TARGET_AREA_FROM_COORD_MASK) as u8
    } else {
        0
    };
    let mut target_block_count: i64 = 0;
    let mut distinct_areaids: Vec<u8> = Vec::with_capacity(OBSERVER_AREAID_SAMPLE_MAX);
    let mut scan_chain_ok = false;
    if let (Some(rm), true) = (resmgr, block_count != OWN_LOAD_STREAM_FIELD_UNREAD) {
        if block_count > 0 {
            scan_chain_ok = true;
            let base_arr = rm + RESMGR_BLOCK_ARRAY_B3030_OFFSET;
            let n = block_count.min(OBSERVER_BLOCK_SCAN_CAP);
            let mut i: i64 = 0;
            while i < n {
                let slot = base_arr + (i as usize) * BLOCK_ENTRY_STRIDE;
                if let Some(block) = deref(slot) {
                    if let Some(inner) = deref(block + BLOCK_INNER_8_OFFSET) {
                        if let Some(area_u8) =
                            unsafe { safe_read_usize(inner + BLOCK_AREA_C_OFFSET) }
                                .map(|v| (v as u32 & TARGET_AREA_FROM_COORD_MASK) as u8)
                        {
                            if area_u8 == target_area {
                                target_block_count += 1;
                                // `block` IS the matched player-area (m28, 0x1c) WorldBlockRes.
                                // Drive its FD4FileCap(s) to residency via the direct-enqueue lever.
                                // Double-gated: own_dispatch armed AND our OWN-LOAD continue fired.
                                if own_dispatch_enabled()
                                    && OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst)
                                {
                                    unsafe {
                                        own_load_m28_dispatch(base, block, &deref);
                                    }
                                }
                            }
                            if distinct_areaids.len() < OBSERVER_AREAID_SAMPLE_MAX
                                && !distinct_areaids.contains(&area_u8)
                            {
                                distinct_areaids.push(area_u8);
                            }
                        }
                    }
                }
                i += 1;
            }
        }
    }
    let target_block_present: i64 = if scan_chain_ok {
        i64::from(target_block_count > 0)
    } else {
        OWN_LOAD_STREAM_FIELD_UNREAD
    };
    // Publish every frame (the oracle_* fields are the machine-readable progress signal); these now
    // keep updating THROUGH the loading screen because this runs in the recurring game task.
    OWN_LOAD_STREAM_OWNER_STATE.store(owner_state, Ordering::SeqCst);
    OWN_LOAD_STREAM_OWNER_REQ_STATE.store(owner_req_state, Ordering::SeqCst);
    OWN_LOAD_STREAM_MMS_STATE.store(mms_state, Ordering::SeqCst);
    OWN_LOAD_STREAM_BLOCK_COUNT.store(block_count, Ordering::SeqCst);
    OWN_LOAD_STREAM_REQ_COORD.store(req_coord, Ordering::SeqCst);
    OWN_LOAD_STREAM_IO_INFLIGHT.store(io_inflight, Ordering::SeqCst);
    OWN_LOAD_STREAM_IO_REQHANDLE.store(io_reqhandle, Ordering::SeqCst);
    OWN_LOAD_STREAM_C30.store(c30, Ordering::SeqCst);
    OWN_LOAD_STREAM_PLAYER_PRESENT.store(player_present_i64, Ordering::SeqCst);
    OWN_LOAD_STREAM_INGAME_PHASE.store(ingame_phase, Ordering::SeqCst);
    OWN_LOAD_STREAM_REQ_BLOCKID.store(req_blockid, Ordering::SeqCst);
    OWN_LOAD_STREAM_TARGET_BLOCK_PRESENT.store(target_block_present, Ordering::SeqCst);
    // WorldBlockRes::Update diagnostic atomics (updated by the wbr_update_hook detour). These tell us
    // whether the per-block phase machine is ticked AT ALL on our path, and how far any block's phase
    // advanced / whether the FD4 completion gate ever fired -- the cause-1-vs-cause-2 discriminator.
    let wbr_calls = OWN_LOAD_WBR_UPDATE_CALLS.load(Ordering::SeqCst);
    let wbr_max_phase = OWN_LOAD_WBR_MAX_PHASE.load(Ordering::SeqCst);
    let wbr_any_gate_set = OWN_LOAD_WBR_ANY_GATE_SET.load(Ordering::SeqCst);
    let frames = OWN_LOAD_STREAM_RECUR_FRAMES.fetch_add(1, Ordering::SeqCst);
    if frames % OWN_LOAD_STREAM_LOG_INTERVAL == 0 {
        let ig = ingame.unwrap_or(null);
        let mms = movemapstep.unwrap_or(null);
        let rm = resmgr.unwrap_or(null);
        append_autoload_debug(format_args!(
            "own-load-stream: frame={frames} (recurring) owner=0x{owner:x} owner_state={owner_state} owner_req={owner_req_state} ingame=0x{ig:x} mms=0x{mms:x} mms_state={mms_state} resmgr=0x{rm:x} block_count={block_count} req_coord=0x{req_coord:x} io_inflight=0x{io_inflight:x} io_reqhandle=0x{io_reqhandle:x} c30=0x{c30:x} player_present={player_present} wbr_update_calls={wbr_calls} wbr_max_phase=0x{wbr_max_phase:x} wbr_any_gate_set={wbr_any_gate_set}"
        ));
        // MENUJOB-SLOT CHECK (autoload-map-orchestrator-menujob): the native Continue installs the
        // deser->map-load CS::MenuJob at owner+0x130, which STEP_MenuJobWait ticks via ExecuteMenuJob.
        // Our SetState5 shortcut installs nothing there -> predict owner+0x130 == NULL. A null here while
        // the native path has a non-null MenuJob (MenuJobResult-family vtable) confirms the lever =
        // install a MenuJob at owner+0x130. READ-ONLY (no write, no call).
        let menujob = if owner != null {
            deref(owner + 0x130).unwrap_or(0)
        } else {
            0
        };
        let menujob_vt = if menujob != 0 {
            deref(menujob).unwrap_or(0)
        } else {
            0
        };
        // Job STATE sweep (menujob-lever-is-START-not-build): owner+0x130 is non-null on our path, so
        // the job is PRE-BUILT; the gap is whether the Continue-confirm STARTED it. Dump the job's
        // header (state field is ~+0x10 per FUN_1407915b0's dispatch on *(this+0x10); sweep neighbors to
        // be offset-robust). If the state never advances on our path (stays idle) while native advances
        // 0->1->2->3->5->6, the lever is the START, not a build. READ-ONLY.
        let js = |off: usize| {
            if menujob != 0 {
                unsafe { safe_read_i32(menujob + off) }.unwrap_or(-1)
            } else {
                -1
            }
        };
        append_autoload_debug(format_args!(
            "own-load-menujob: frame={frames} owner=0x{owner:x} owner+0x130=0x{menujob:x} vt=0x{menujob_vt:x} state[+08]={} [+10]={} [+14]={} [+18]={} [+20]={} (job pre-built; watching if it ever STARTS)",
            js(0x08),
            js(0x10),
            js(0x14),
            js(0x18),
            js(0x20)
        ));
        // Second registration-vs-streaming line: did play_game_submit's handoff run (ingame_phase /
        // req_blockid) and is the coord-derived target block REGISTERED (target_block_present) among
        // the scanned areaIds? Absent target block => registration gap; present but stuck => streaming.
        let present = target_block_present == i64::from(true);
        append_autoload_debug(format_args!(
            "own-load-blocks: frame={frames} ingame_phase={ingame_phase} req_blockid=0x{req_blockid:x} target_area=0x{target_area:x} target_block_present={present} target_block_count={target_block_count} areaids={distinct_areaids:02x?}"
        ));
    }
}

/// Restore `GLOBAL_CSGaitem` to constructor-pristine (empty gaitemInsTable + full free-queue) at a
/// clean title BEFORE the switch reload's fresh deserialize, so char#2's deserialize does not
/// exhaust the free-queue on char#1's leaked items (the AV at live 0x67141a, bd
/// system-quit-postswitch-crash-gaitem-freequeue-exhaustion-2026-07-02). Mechanism: sweep all
/// 0x1400 gaitemInsTable slots; for each occupied slot call the NATIVE per-item release
/// RemoveCSGaitemIns(gaitem, &entries[i].unindexedGaItemHandle) -- it destructs+deallocates the ins
/// (no leak) and returns index i to freeTableIdxQueue. This is the exact primitive the native
/// world/inventory teardown uses; we drive it because our lightweight return-title chain skips it.
///
/// SAVE-SAFETY / correctness preconditions (the CALLER must guarantee, and this fn re-checks what it
/// can): the old world is torn down (local player absent) so nothing live holds POINTERS to these
/// ins objects -- PlayerGameData/inventory hold only integer handles, which char#2's deserialize
/// overwrites. Structural validation (heap-aligned singleton, head/end within [0,0x1400)) fails
/// closed rather than sweeping a bogus pointer. Returns Some((released, slack_before, slack_after))
/// on success (slack = 0x13ff - free_count; healthy = slack_after 0), None if it declined.
pub(crate) unsafe fn own_load_reset_gaitem_singleton(base: usize) -> Option<(u32, u32, u32)> {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const RING_USABLE: u32 = (CSGAITEM_TABLE_CAPACITY as u32) - 1; // 0x13ff (one sentinel slot)
    let gaitem = unsafe { safe_read_usize(base + GLOBAL_CSGAITEM_SINGLETON_RVA) }.unwrap_or(NULL);
    if gaitem == NULL || !unsafe { is_heap_aligned_ptr(gaitem) } {
        append_autoload_debug(format_args!(
            "gaitem-reset: GLOBAL_CSGaitem not resident/aligned (0x{gaitem:x}) -- declining pristine-restore (no-op)"
        ));
        return None;
    }
    let free_count = |head: u32, end: u32| -> u32 {
        // Ring distance head..end over capacity 0x1400 = number of poppable free indices.
        end.wrapping_sub(head)
            .wrapping_add(CSGAITEM_TABLE_CAPACITY as u32)
            % (CSGAITEM_TABLE_CAPACITY as u32)
    };
    let head0 =
        unsafe { safe_read_i32(gaitem + CSGAITEM_FREE_QUEUE_HEAD_OFFSET) }.unwrap_or(-1) as u32;
    let end0 =
        unsafe { safe_read_i32(gaitem + CSGAITEM_FREE_QUEUE_END_OFFSET) }.unwrap_or(-1) as u32;
    if head0 as usize >= CSGAITEM_TABLE_CAPACITY || end0 as usize >= CSGAITEM_TABLE_CAPACITY {
        append_autoload_debug(format_args!(
            "gaitem-reset: free-queue head/end out of range (head=0x{head0:x} end=0x{end0:x} cap=0x{:x}) -- singleton not the expected CSGaitemImp; declining (no-op)",
            CSGAITEM_TABLE_CAPACITY
        ));
        return None;
    }
    let slack_before = RING_USABLE.saturating_sub(free_count(head0, end0));
    let remove_ins: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(base + CSGAITEM_REMOVE_INS_RVA) };
    let mut released: u32 = 0;
    for i in 0..CSGAITEM_TABLE_CAPACITY {
        let slot = gaitem + CSGAITEM_INS_TABLE_OFFSET + i * core::mem::size_of::<usize>();
        let ins = unsafe { safe_read_usize(slot) }.unwrap_or(NULL);
        if ins == NULL {
            continue;
        }
        // &entries[i].unindexedGaItemHandle -- its embedded index maps back to slot i (ctor seeds it,
        // alloc preserves it), so RemoveCSGaitemIns frees gaitemInsTable[i] and returns index i.
        let handle_ptr = gaitem + CSGAITEM_ENTRIES_OFFSET + i * CSGAITEM_ENTRY_STRIDE;
        unsafe { remove_ins(gaitem, handle_ptr) };
        released += 1;
    }
    let head1 =
        unsafe { safe_read_i32(gaitem + CSGAITEM_FREE_QUEUE_HEAD_OFFSET) }.unwrap_or(-1) as u32;
    let end1 =
        unsafe { safe_read_i32(gaitem + CSGAITEM_FREE_QUEUE_END_OFFSET) }.unwrap_or(-1) as u32;
    let slack_after = RING_USABLE.saturating_sub(free_count(head1, end1));
    SYSTEM_QUIT_GAITEM_RESET_INVOCATIONS.fetch_add(1, Ordering::SeqCst);
    SYSTEM_QUIT_GAITEM_RESET_RELEASED_COUNT.fetch_add(released as usize, Ordering::SeqCst);
    SYSTEM_QUIT_GAITEM_RESET_LAST_SLACK_BEFORE.store(slack_before as usize, Ordering::SeqCst);
    SYSTEM_QUIT_GAITEM_RESET_LAST_SLACK_AFTER.store(slack_after as usize, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "gaitem-reset: pristine-restore gaitem=0x{gaitem:x} released={released} free-queue head/end 0x{head0:x}/0x{end0:x} -> 0x{head1:x}/0x{end1:x} slack {slack_before}->{slack_after} (0=full); native RemoveCSGaitemIns 0x{:x} per occupied slot",
        base + CSGAITEM_REMOVE_INS_RVA
    ));
    Some((released, slack_before, slack_after))
}

/// SYNCHRONOUS fresh picked-slot feed-deserialize for the System->Quit->Load-Profile switch (the
/// continue_confirm hook calls this BEFORE forwarding, so the c30/PGD the confirm streams belong to
/// the PICKED slot -- bd system-quit-cleantitle-load-is-stale-restream-not-slot-source-2026-07-02).
/// Same proven mechanism as `own_load_drive` steps 1-4: read the on-disk save (native SAVE-DIR
/// builder path -- post-first-load the redirect has reverted, so this is the file the quit-save
/// just wrote), slice slot `want_slot`'s plaintext body, arm the gated 0x67b100 read detour, call
/// the native parser 0x67b290(slot) in-process. Returns true only when the parse produced a real
/// c30 + a real PlayerGameData fingerprint. Save-safe: read-only on the .sl2 (no SetState5, no
/// save write; the deserialize also repoints GameMan+0xac0 to `want_slot` as its normal byproduct).
pub(crate) unsafe fn own_load_feed_deserialize(base: usize, gm: usize, want_slot: i32) -> bool {
    const C30_ZERO: i32 = 0;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if gm == null || want_slot < OWN_STEPPER_SLOT_ZERO {
        append_autoload_debug(format_args!(
            "own-load-feed: rejected gm=0x{gm:x} slot={want_slot} -- need GameMan + explicit slot (no-write)"
        ));
        return false;
    }
    let Some(sl2_bytes) = (unsafe { own_load_read_sl2_bytes(base) }) else {
        return false;
    };
    let body: &[u8] = match er_save_loader::bnd4::slot_body(&sl2_bytes, want_slot as usize) {
        Ok(b) => b,
        Err(e) => {
            append_autoload_debug(format_args!(
                "own-load-feed: slot_body(slot={want_slot}) failed: {e:?} -- ABORT (no-write)"
            ));
            return false;
        }
    };
    // Leak the sliced body so it stays valid for the detour to memcpy (one bounded copy per switch).
    let leaked: &'static [u8] = Box::leak(body.to_vec().into_boxed_slice());
    OWN_LOAD_BODY_PTR.store(leaked.as_ptr() as usize, Ordering::SeqCst);
    OWN_LOAD_BODY_LEN.store(leaked.len(), Ordering::SeqCst);
    if !install_own_load_hook() {
        append_autoload_debug(format_args!(
            "own-load-feed: hook install failed -- ABORT (no-write)"
        ));
        return false;
    }
    let c30_before =
        unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(GAME_MAN_C30_UNSET);
    OWN_LOAD_GATE.store(true, Ordering::SeqCst);
    let parser: unsafe extern "system" fn(i32) -> i32 =
        unsafe { std::mem::transmute(base + DESERIALIZE_SLOT_RVA) };
    let pret = unsafe { parser(want_slot) };
    OWN_LOAD_GATE.store(false, Ordering::SeqCst);
    let fed = OWN_LOAD_FED_BYTES.load(Ordering::SeqCst);
    let c30 =
        unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(GAME_MAN_C30_UNSET);
    let ac0 = unsafe { safe_read_i32(gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) }
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
    let c30_real = c30 != GAME_MAN_C30_UNSET && c30 != C30_ZERO && c30 != FULLREAD_C30_M10_DEFAULT;
    let ok = c30_real && fp_real;
    if ok {
        OWN_STEPPER_MOUNT_C30.store(c30, Ordering::SeqCst);
        OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_OK, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "own-load-feed: parser 0x{:x}(slot={want_slot}) ret={pret} fed_bytes=0x{fed:x} c30 0x{c30_before:x}->0x{c30:x} c30_real={c30_real} ac0={ac0} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) ok={ok} (read-only deserialize; NO SetState5, NO save write)",
        base + DESERIALIZE_SLOT_RVA
    ));
    ok
}

/// SAVE-SAFE verify-only OWN-LOAD buffer-feed drive (one-shot, phased). Reads the .sl2 from disk,
/// slices slot `want_slot`'s plaintext body, installs+arms the gated 0x67b100 hook, calls the native
/// parser 0x67b290(slot) in-process so it parses OUR body, then reads back GameMan+0xc30 + the
/// PlayerGameData fingerprint. NO SetState5, NO autosave, NO continue_confirm. Records presses==0.
pub(crate) unsafe fn own_load_drive(base: usize, gm: usize, owner: usize, want_slot: i32, n: u64) {
    const PHASE_INIT: usize = 0;
    const PHASE_DONE: usize = 1;
    const C30_ZERO: i32 = 0;
    static OWN_LOAD_PHASE: AtomicUsize = AtomicUsize::new(PHASE_INIT);
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let phase = OWN_LOAD_PHASE.load(Ordering::SeqCst);
    // Publish phase+1 so the readiness watcher tears down on terminal completion (PHASE_DONE -> 2).
    OWN_LOAD_PHASE_PUB.store(phase + 1, Ordering::SeqCst);
    if phase != PHASE_INIT {
        return;
    }
    if gm == null {
        return;
    }
    if want_slot < OWN_STEPPER_SLOT_ZERO {
        append_autoload_debug(format_args!(
            "own-load: needs an EXPLICIT slot (slot={want_slot}); set slot=N in er-effects-autoload.txt -- ABORT (no-write)"
        ));
        OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        return;
    }
    // (1) Read + slice the plaintext slot body. er_save_loader::bnd4 is the only glue: the engine's
    // read path is FSM-gated, so OWN-LOAD must hand it the buffer itself (bd reuse-native-fns).
    let Some(sl2_bytes) = (unsafe { own_load_read_sl2_bytes(base) }) else {
        OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        return;
    };
    let body: &[u8] = match er_save_loader::bnd4::slot_body(&sl2_bytes, want_slot as usize) {
        Ok(b) => b,
        Err(e) => {
            append_autoload_debug(format_args!(
                "own-load: slot_body(slot={want_slot}) failed: {e:?} -- ABORT (no-write)"
            ));
            OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
            return;
        }
    };
    // Leak the sliced body so it outlives this frame and stays valid for the detour to memcpy. One
    // copy of the (small fraction of the) save -- never the whole file -- kept for the session.
    let leaked: &'static [u8] = Box::leak(body.to_vec().into_boxed_slice());
    OWN_LOAD_BODY_PTR.store(leaked.as_ptr() as usize, Ordering::SeqCst);
    OWN_LOAD_BODY_LEN.store(leaked.len(), Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "own-load: sliced slot {want_slot} body len=0x{:x} (expected 0x{:x}) -> install+arm gate, call native parser 0x{:x}",
        leaked.len(),
        er_save_loader::bnd4::SLOT_BODY_LEN,
        base + DESERIALIZE_SLOT_RVA
    ));
    // (2) Install the gated 0x67b100 detour (harmless pass-through until armed).
    if !install_own_load_hook() {
        append_autoload_debug(format_args!(
            "own-load: hook install failed -- ABORT (no-write)"
        ));
        OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        return;
    }
    let c30_before = unsafe { *((gm + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) };
    // (3) Set the gate, call native 0x67b290(slot) in-process, clear the gate. 0x67b290 does NOT
    // re-check b80 after the read (static-confirmed), so our al=1 + body flow into the native parse.
    OWN_LOAD_GATE.store(true, Ordering::SeqCst);
    let parser: unsafe extern "system" fn(i32) -> i32 =
        unsafe { std::mem::transmute(base + DESERIALIZE_SLOT_RVA) };
    let pret = unsafe { parser(want_slot) };
    OWN_LOAD_GATE.store(false, Ordering::SeqCst);
    let fed = OWN_LOAD_FED_BYTES.load(Ordering::SeqCst);
    // (4) VERIFY (read-back only): GameMan+0xc30 (map id) + the PlayerGameData char fingerprint.
    let c30 = unsafe { *((gm + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) };
    let ac0 = unsafe { *((gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
    let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
    let c30_real = c30 != GAME_MAN_C30_UNSET && c30 != C30_ZERO && c30 != FULLREAD_C30_M10_DEFAULT;
    if c30_real && fp_real {
        OWN_STEPPER_MOUNT_C30.store(c30, Ordering::SeqCst);
        OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_OK, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "own-load: VERIFY parser 0x{:x}(slot={want_slot}) ret={pret} fed_bytes=0x{fed:x} c30 0x{c30_before:x}->0x{c30:x} c30_real={c30_real} ac0={ac0} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) presses=0 (NO SetState5/NO save write)",
        base + DESERIALIZE_SLOT_RVA
    ));
    unsafe { dump_load_correctness(base, n) };
    // OWNER DIAGNOSTIC (er-effects-rs-mr2, save-safe pure reads): the prior continue crash used the
    // WRONG owner (*(GameDataMan+0x8)). Log EVERY continue_confirm owner candidate + each one's
    // +0x284 (new-game flag) byte so a VERIFY-ONLY run reveals which is the SetState-able title
    // owner BEFORE we ever fire continue_confirm. This is independent of the gated continue step.
    //   title  = the threaded SetState-able title owner the caller validated (own_stepper_idx10),
    //   recipe = *(base + CONTINUE_MANAGER_GLOBAL_RVA + 8)  (the native-fullread COMMIT recipe's literal),
    //   mgr_vt = *(base + CONTINUE_MANAGER_GLOBAL_RVA)      (the manager object's vtable ptr),
    //   gdm8   = *(GameDataMan + 0x8)                       (the prior crash owner).
    let read284 = |obj: usize| -> u8 {
        if obj == null {
            0
        } else {
            unsafe { safe_read_usize(obj + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET) }
                .map(|v| v as u8)
                .unwrap_or(0)
        }
    };
    let recipe_owner = unsafe {
        safe_read_usize(base + CONTINUE_MANAGER_GLOBAL_RVA + FULLREAD_OWNER_GDM_08_OFFSET)
    }
    .unwrap_or(null);
    let manager_vtable =
        unsafe { safe_read_usize(base + CONTINUE_MANAGER_GLOBAL_RVA) }.unwrap_or(null);
    let game_data_man = game_data_man_ptr_or_null();
    let gdm8 = if game_data_man == null {
        null
    } else {
        unsafe { safe_read_usize(game_data_man + FULLREAD_OWNER_GDM_08_OFFSET) }.unwrap_or(null)
    };
    append_autoload_debug(format_args!(
        "own-load-OWNER-DIAG: title=0x{owner:x} (+284={}) recipe=0x{recipe_owner:x} (+284={}) mgr_vt=0x{manager_vtable:x} gdm8=0x{gdm8:x} (+284={})",
        read284(owner),
        read284(recipe_owner),
        read284(gdm8)
    ));
    // (5) FINAL STEP. Two mutually-exclusive armed levers (both OFF by default; verify-only is the
    // default). The LoadGame-JOB INSTALL lever (own_load_install_job) takes precedence: it is the
    // SAVE-SAFE, NON-SetState5 path (build + install the LoadGame MenuJob into owner+0x130 so
    // STEP_MenuJobWait ticks it -> self-build -> deser -> world stream; no SetState5, no save write).
    // Only if it is NOT armed do we fall back to the legacy GUARDED continue_confirm/SetState5 lever
    // (own_load_continue), which is SAVE-WRITING (SetState5 autosaves) behind the hard c30/fp guard.
    // PATH B (own_load_pump) takes precedence: BUILD the LoadGame job with REAL mss-derived ctx, then
    // privately pump its Run every frame from the recurring game task to completion (deser -> m28 stream)
    // and drive the transition on Success. No owner+0x130 install, no queue, no dialog -- the proven
    // menu-free "own the load". SAVE-SAFE at build (only the final SetState5 transition writes, gated).
    if own_load_pump_enabled() {
        unsafe { own_load_pump_fire(base, owner, c30, c30_real, fp_real, fp_level, n) };
    } else if own_load_install_job_enabled() {
        unsafe { own_load_install_job_fire(base, owner, c30, c30_real, fp_real, fp_level, n) };
    } else if own_load_continue_enabled() {
        unsafe { own_load_continue_fire(base, owner, c30, c30_real, fp_real, fp_level, n) };
    }
    OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
    OWN_LOAD_PHASE_PUB.store(PHASE_DONE + 1, Ordering::SeqCst);
}

/// OWN-LOAD FINAL STEP (er-effects-rs-mr2): after the PROVEN verify-only parse mounted a REAL c30 +
/// real character, fire the GUARDED native `continue_confirm` 0x140b0e180 -> `SetState5` 0x140b0d960
/// to stream the character into the PLAYABLE world. `continue_confirm` reads owner = [rcx+8] off
/// the shim, reads GameMan+0xc30 (already REAL from our parse) into owner+0xbc, then
/// SetState(owner, 5) -> the per-frame title-flow step machine streams the world.
///
/// OWNER (er-effects-rs-mr2 fix): the owner MUST be the SetState-able TITLE owner threaded in from
/// `own_stepper_idx10` (the validated title-flow object), NOT *(GameDataMan+0x8). The prior crash
/// passed *(GameDataMan+0x8) (a DIFFERENT object) into continue_confirm and crashed inside
/// SetState5. The OWNER DIAGNOSTIC in the verify path logs all candidates for cross-checking.
///
/// SAVE-SAFETY ABSOLUTE (SetState5 AUTOSAVES). HARD GUARD before firing -- ABORT with a logged
/// no-write if ANY fails:
///   * `c30_real` (c30 != 0xa010000 m10-default AND != 0xffffffff unset AND != 0): same flag the
///     verify path computed -- never fire SetState5 on an unverified/default c30 (the prior crash
///     cause -- real char streamed to the wrong map then autosaved over).
///   * `fp_real`: the PlayerGameData char fingerprint is real (level/stats non-default).
///   * `title_owner` non-null AND title_owner+0x284 (new-game flag) == 0 (continue_confirm's LOAD
///     branch; non-zero would take the NewGame path -- fail closed).
/// Keeps `simulated_button_presses_total = 0`: this is a pure in-process native call, no input.
unsafe fn own_load_continue_fire(
    base: usize,
    title_owner: usize,
    c30: i32,
    c30_real: bool,
    fp_real: bool,
    fp_level: u32,
    n: u64,
) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Hard c30 + fingerprint guard (absolute save-safety backstop). NOTE: unlike the native-fullread
    // COMMIT path (which needs a level>=10 floor to reject the level-9 NEW-GAME PREVIEW), OWN-LOAD has
    // a STRONGER per-slot signal: `c30_real` means GameMan+0xc30 became the slot's REAL map
    // (0x1c000000 etc.), NOT the new-game default 0xa010000 -- so a real save is proven directly.
    // `fp_real` already requires level>=1 AND a non-empty name (see char_fingerprint), so it admits
    // legitimate LOW-LEVEL real characters (e.g. a level-7 Hero-class save) that a >=10 floor would
    // wrongly reject. c30_real + fp_real is the correct, save-safe gate here.
    if !(c30_real && fp_real) {
        append_autoload_debug(format_args!(
            "own-load-continue: GUARD FAIL (c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}) -- NO continue_confirm, NO SetState5, NO save write -> ABORT (save-safe)"
        ));
        return;
    }
    // OWNER = the SetState-able TITLE owner threaded in from own_stepper_idx10 (NOT *(GameDataMan+0x8),
    // which caused the prior crash). It is the validated title-flow object the DLL already SetState's.
    if title_owner == null {
        append_autoload_debug(format_args!(
            "own-load-continue: ABORT -- threaded title_owner is null -> no write"
        ));
        return;
    }
    let new_game_flag = match unsafe {
        safe_read_usize(title_owner + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET)
    } {
        Some(v) => v as u8,
        None => {
            append_autoload_debug(format_args!(
                "own-load-continue: ABORT -- title_owner+0x284 (new-game flag) unreadable (title_owner=0x{title_owner:x}) -> no write"
            ));
            return;
        }
    };
    if new_game_flag != FULLREAD_OWNER_NEW_GAME_OK {
        append_autoload_debug(format_args!(
            "own-load-continue: ABORT -- title_owner+0x284={new_game_flag} != 0 (continue_confirm LOAD branch requires the new-game flag clear) -> no write"
        ));
        return;
    }
    // GUARD PASSED. Build the {[OWNER_IDX]=title_owner} shim and fire the native continue_confirm.
    let shim = &raw mut OWN_STEPPER_SHIM;
    unsafe { (*shim)[OWN_STEPPER_SHIM_OWNER_IDX] = title_owner };
    let shim_ptr = shim as usize;
    let confirm: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + CONTINUE_CONFIRM_RVA) };
    append_autoload_debug(format_args!(
        "own-load-continue: *** GUARD PASS -- COMMIT continue_confirm 0x{:x}(shim=0x{shim_ptr:x} title_owner=0x{title_owner:x}) c30=0x{c30:x} level={fp_level} title_owner+0x284=0 -- continue_confirm fires SetState5 internally (AUTOSAVES) presses=0 ***",
        base + CONTINUE_CONFIRM_RVA
    ));
    timeline_event(
        "T_own_load_continue",
        n,
        format_args!("c30=0x{c30:x} level={fp_level}"),
    );
    unsafe { confirm(shim_ptr) };
    // Cache the pointers the RECURRING world-stream observer needs, then arm it. own_stepper_idx10 (a
    // TITLE-PHASE task) STOPS ticking once SetState5 starts this transition, so the title `owner` and
    // its InGameStep (owner+0x2e8) will no longer be threaded in. Snapshot them HERE (InGameStep was
    // already non-null at frame 0) so the recurring game task can keep walking owner->InGameStep->
    // MoveMapStep through the whole loading screen. (own-load-stream-observer-must-be-recurring-task-2026-06-22)
    OWN_LOAD_OWNER_CACHED.store(title_owner, Ordering::SeqCst);
    let ingame_cached = unsafe { safe_read_usize(title_owner + TITLE_OWNER_JOB_OFFSET) }
        .filter(|&v| v != null)
        .unwrap_or(0);
    OWN_LOAD_INGAMESTEP_CACHED.store(ingame_cached, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_FIRED.store(true, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "own-load-continue: continue_confirm returned -- native pump now streams the real world (#{n}); recurring world-stream observer ARMED (owner=0x{title_owner:x} ingame=0x{ingame_cached:x}) -> DONE"
    ));
}

/// Snapshot of the `owner+0x130` MenuJob slot for the before/after vtable-flip + self-build evidence.
/// All pure fault-tolerant reads -- never changes load behavior.
fn own_load_install_job_slot_snapshot(slot_addr: usize) -> (usize, usize, usize, u8, usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // The job pointer currently in the slot.
    let job = unsafe { safe_read_usize(slot_addr) }.unwrap_or(null);
    if job == null {
        return (null, null, null, 0, null);
    }
    let vtable = unsafe { safe_read_usize(job) }.unwrap_or(null);
    let inner_seq = unsafe { safe_read_usize(job + MENUJOB_INNER_SEQ_70_OFFSET) }.unwrap_or(null);
    let built_flag = unsafe { safe_read_usize(job + MENUJOB_BUILT_FLAG_68_OFFSET) }
        .map(|v| v as u8)
        .unwrap_or(0);
    let current_job_index =
        unsafe { safe_read_usize(job + MENUJOB_CURRENT_JOB_INDEX_10_OFFSET) }.unwrap_or(null);
    (job, vtable, inner_seq, built_flag, current_job_index)
}

/// OWN-LOAD FINAL STEP -- LoadGame-JOB INSTALL lever (`own_load_install_job`). The SAVE-SAFE,
/// NON-SetState5 alternative to `own_load_continue_fire`: after the PROVEN verify-only parse mounted a
/// REAL c30 + real character, BUILD the native LoadGame `CS::MenuJobWithContext<LoadJobContext>` and
/// INSTALL it into the title owner's `+0x130` MenuJob slot, replacing the idle `IfElseJob`.
/// `CS::TitleStep::STEP_MenuJobWait` already ticks `ExecuteMenuJob(&owner->+0x130)` every frame, so the
/// installed job then self-builds (its `Run` builds the inner FixOrderJobSequence on the first tick:
/// `+0x68`/`+0x70` flip), deserializes the save, and streams the world -- WITHOUT `SetState5`.
///
/// SAVE-SAFETY ABSOLUTE: NO `SetState5`, NO autosave, NO save write. The BUILD factory only allocates +
/// copies a template; the first-tick deser step (`FUN_14082c330`) only READS the save
/// (`AllocateAligned` -> read -> `SetSaveSlot` -> decrypt -> `ReadBytes` -> dealloc) up to world-stream.
/// Static-verified against the runtime dump. Same hard c30/fp guard as the continue lever is kept as a
/// belt-and-braces precondition even though no write occurs. Keeps `simulated_button_presses_total = 0`.
///
/// ARG SOURCING (static RE, 2026-06-22): the BUILD factory `FUN_140826510(out, ctx_parent, slot,
/// owner_ctx)` needs only `out` (our local) + `slot` (the int slot) for the deser/map self-build; the
/// `ctx_parent`/`owner_ctx` args are the OUTER profile-selection UI context, stored as lambda captures
/// whose every build-path deref is null-guarded -- so we pass them as 0. RESIDUAL RISK: if the engine's
/// `EnableProfileSelection` release flag is set AND the outer sequence ticks the profile-selection
/// sub-job, a captured-null deref could fault -- watch the install-fire log for that. The two native
/// calls are wrapped in `catch_unwind` (catches a Rust-unwinding panic; a hardware AV is NOT caught).
unsafe fn own_load_install_job_fire(
    base: usize,
    title_owner: usize,
    c30: i32,
    c30_real: bool,
    fp_real: bool,
    fp_level: u32,
    n: u64,
) {
    const NO_CTX: usize = 0;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Belt-and-braces guard (no write occurs, but never act on an unverified parse).
    if !(c30_real && fp_real) {
        append_autoload_debug(format_args!(
            "own-load-install-job: GUARD FAIL (c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}) -- NO build/install -> ABORT (save-safe)"
        ));
        return;
    }
    if title_owner == null {
        append_autoload_debug(format_args!(
            "own-load-install-job: ABORT -- threaded title_owner is null -> no install (save-safe)"
        ));
        return;
    }
    let want_slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst) as i32;
    let slot_addr = title_owner + TITLE_OWNER_MENUJOB_SLOT_130_OFFSET;
    // BEFORE: dump owner+0x130 (the idle IfElseJob it replaces). Pure reads.
    let (b_job, b_vt, b_seq, b_built, b_idx) = own_load_install_job_slot_snapshot(slot_addr);
    append_autoload_debug(format_args!(
        "own-load-install-job: BEFORE slot=owner+0x130=0x{slot_addr:x} job=0x{b_job:x} vt=0x{b_vt:x} (expect IfElseJob dump 0x{:x}) +0x68_built={b_built} +0x70_seq=0x{b_seq:x} +0x10_idx=0x{b_idx:x} -- BUILD 0x{:x}(out,ctx=0,slot={want_slot},owner_ctx=0) presses=0",
        MENUJOB_IFELSE_VTABLE_DUMP_VA,
        base + LOADGAME_JOB_BUILD_RVA,
    ));
    // (a) BUILD the LoadGame MenuJobWithContext into a local DLRefCountPtr (the factory writes the job
    //     ptr into *out with refcount 1). Win64 fastcall (out, ctx_parent, save_slot, owner_ctx).
    // Justify the transmute: LOADGAME_JOB_BUILD_RVA is the prologue-grounded live entry of the menu-heap
    // LoadGame-job factory; the signature matches the static decompile of FUN_140826510.
    let build: unsafe extern "system" fn(*mut usize, usize, i32, usize) -> *mut usize =
        unsafe { std::mem::transmute(base + LOADGAME_JOB_BUILD_RVA) };
    let mut built_job: usize = 0;
    let build_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        build(&raw mut built_job, NO_CTX, want_slot, NO_CTX)
    }));
    match build_ret {
        Ok(_) => {}
        Err(_) => {
            append_autoload_debug(format_args!(
                "own-load-install-job: BUILD PANICKED (caught) -- NO install -> ABORT (save-safe)"
            ));
            return;
        }
    }
    if built_job == null || built_job == 0 {
        append_autoload_debug(format_args!(
            "own-load-install-job: BUILD returned a NULL job (built_job=0x{built_job:x}) -- NO install -> ABORT (save-safe)"
        ));
        return;
    }
    let built_vt = unsafe { safe_read_usize(built_job) }.unwrap_or(null);
    append_autoload_debug(format_args!(
        "own-load-install-job: BUILD OK job=0x{built_job:x} vt=0x{built_vt:x} (expect LoadGame dump 0x{:x}) -- INSTALL via assign 0x{:x}(slot=0x{slot_addr:x}, src=&job)",
        MENUJOB_LOADGAME_VTABLE_DUMP_VA,
        base + MENUJOB_ASSIGN_RVA,
    ));
    // (b) APPEND our built job into the owner+0x130 MenuJobQueue via PushBackJob (NOT a slot-overwrite).
    //     owner+0x130 is a CS::MenuJobQueue (active job +0x130, ring +0x138, count +0x178). The prior
    //     move-assign overwrite ORPHANED the title IfElseJob's sibling CS::MenuWindowJobs -> AV at
    //     CS::DLFixedVector::push_back 0x140733fea. PushBackJob(queue_base=&owner+0x130, src=&built_job)
    //     appends behind the still-active IfElseJob (no tear, AtomicIncrements the job, does not zero
    //     src); STEP_MenuJobWait's ExecuteMenuJob then pops + ticks our queued job.
    // Justify the transmute: MENUJOB_PUSHBACK_RVA is the prologue-grounded live entry of
    // CS::MenuJobQueue::PushBackJob (FUN_1407a9254).
    let queue_count_before =
        unsafe { safe_read_i32(slot_addr + MENUJOB_QUEUE_COUNT_178_OFFSET) }.unwrap_or(-1);
    let pushback: unsafe extern "system" fn(*mut usize, *mut usize) -> *mut usize =
        unsafe { std::mem::transmute(base + MENUJOB_PUSHBACK_RVA) };
    let install_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        pushback(slot_addr as *mut usize, &raw mut built_job)
    }));
    if install_ret.is_err() {
        append_autoload_debug(format_args!(
            "own-load-install-job: PUSHBACK PANICKED (caught) after build (job=0x{built_job:x}) -> ABORT"
        ));
        return;
    }
    // AFTER: the active job at owner+0x130 should be UNCHANGED (still the IfElseJob) -- our job is in the
    // ring; the queue count at +0x178 should have grown by 1. Pure reads.
    let (a_job, a_vt, a_seq, a_built, a_idx) = own_load_install_job_slot_snapshot(slot_addr);
    let queue_count_after =
        unsafe { safe_read_i32(slot_addr + MENUJOB_QUEUE_COUNT_178_OFFSET) }.unwrap_or(-1);
    OWN_LOAD_INSTALL_JOB_FIRED.fetch_add(1, Ordering::SeqCst);
    // Cache the owner so the recurring world-stream observer keeps logging through the loading screen
    // (own_stepper_idx10 stops once the title transitions). Mirror own_load_continue_fire's caching.
    OWN_LOAD_OWNER_CACHED.store(title_owner, Ordering::SeqCst);
    let ingame_cached = unsafe { safe_read_usize(title_owner + TITLE_OWNER_JOB_OFFSET) }
        .filter(|&v| v != null)
        .unwrap_or(0);
    OWN_LOAD_INGAMESTEP_CACHED.store(ingame_cached, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_FIRED.store(true, Ordering::SeqCst);
    timeline_event(
        "T_own_load_install_job",
        n,
        format_args!("c30=0x{c30:x} level={fp_level}"),
    );
    append_autoload_debug(format_args!(
        "own-load-install-job: *** APPENDED -- AFTER queue=owner+0x130=0x{slot_addr:x} active_job=0x{a_job:x} vt=0x{a_vt:x} (active stays IfElseJob dump 0x{:x}, NOT torn) active+0x68={a_built} +0x70=0x{a_seq:x} +0x10_idx=0x{a_idx:x} | queue_count {queue_count_before}->{queue_count_after} (expect +1) | our_job=0x{built_job:x} (LoadGame dump 0x{:x}) ingame=0x{ingame_cached:x} -- STEP_MenuJobWait pops+ticks queued job -> self-build -> deser -> world stream (NO SetState5/NO save write) presses=0 (#{n}) -> DONE ***",
        MENUJOB_IFELSE_VTABLE_DUMP_VA, MENUJOB_LOADGAME_VTABLE_DUMP_VA,
    ));
    let _ = (b_seq, b_idx, b_built, b_vt, b_job);
}

/// Resolve `mss = GameDataMan->menuSystemSaveLoad = *(*(base + GAME_DATA_MAN_GLOBAL_RVA) +
/// GAME_DATA_MAN_MENU_SAVELOAD_60_OFFSET)` (static-verified: `GetMenuSystemSaveLoad` 0x140256410 is
/// exactly `GLOBAL_GameDataMan->menuSystemSaveLoad`). Returns `None` (never `null`/`0`) on any
/// fault-tolerant read failure. Pure reads.
pub(crate) unsafe fn resolve_menu_system_save_load(base: usize) -> Option<usize> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let gdm = unsafe { safe_read_usize(base + GAME_DATA_MAN_GLOBAL_RVA) }
        .filter(|&v| v != null && v != 0)?;
    unsafe { safe_read_usize(gdm + GAME_DATA_MAN_MENU_SAVELOAD_60_OFFSET) }
        .filter(|&v| v != null && v != 0)
}

/// The "engine filled enough to drive our own load" gate -- distinct from "GameMan instance pointer
/// resolved" (`game_man_instance_resolved`), which flips true at BootPhase4, LONG before the load
/// machinery is usable. True iff GameDataMan + menuSystemSaveLoad (mss) resolve AND the TitleFlowContext
/// at `mss+0xa38` is a PLAUSIBLE heap pointer. The plausibility range matters: before the GameFlow
/// constructs the TitleFlowContext it reads back as uninitialized garbage (e.g. 0x8080808080808080),
/// which a `!= 0` check would wrongly accept -- then the LoadGame job's first `Run` derefs it and
/// access-violates (the ~25s AV observed when arming at the bare title). When this returns true, the
/// native LoadGame job (`own_load_pump_fire`) can be built + pumped without that crash. The bypass arms
/// its own-load on THIS, not on `game_man_instance_resolved`.
/// (loadgame-build-ctx-ready-precondition-2026-06-22)
pub(crate) unsafe fn loadgame_build_ctx_ready(base: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // CORRECTED (bd loadgame-owner-ctx-is-DIALOG-a38-not-mss-CORRECTION-2026-06-22): the buildable
    // TitleFlowContext is `*(CS::TitleTopDialog+0xa38)`, NOT `*(mss+0xa38)` (the mss reading was a red
    // herring -- r13 at the golden factory site is the dialog). Read it off the live dialog
    // (owner+0xe0, vtable-gated) via the cached title owner, so this arming signal matches exactly the
    // ctx `own_load_pump_fire` builds with.
    let owner = TITLE_OWNER_PTR.load(Ordering::SeqCst);
    if owner == null || owner == 0 {
        return false;
    }
    let dialog = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(0);
    if dialog == 0 {
        return false;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(0);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return false;
    }
    let ctx = unsafe { safe_read_usize(dialog + DIALOG_OWNER_CTX_A38_OFFSET) }.unwrap_or(0);
    if !(ctx > OWNER_CTX_MIN_PLAUSIBLE_PTR && ctx < OWNER_CTX_MAX_PLAUSIBLE_PTR) {
        return false;
    }
    // Native `FUN_14082d090` checks this singleton before comparing regulation versions; our readiness
    // predicate must not claim the title/load context is usable before the same singleton exists.
    let regulation_manager =
        unsafe { safe_read_usize(base + GLOBAL_CS_REGULATION_MANAGER_RVA) }.unwrap_or(0);
    regulation_manager != 0 && regulation_manager != null
}

/// PATH B "OWN THE LOAD" -- BUILD the LoadGame job with REAL mss-derived ctx, store its pointer for the
/// recurring per-frame private pump. The menu-free alternative to BOTH the owner+0x130 install (a
/// proven dead end) and the SetState5-only continue (reached the loading screen but never mounted m28).
///
/// We BUILD via `FUN_140826510(out, ctx_parent=mss+0x50, save_slot, owner_ctx=*(mss+0xa38))` -- the REAL
/// non-null ctx from the golden Continue trace (the prior ctx=0 build AV'd when the outer
/// profile-selection sub-job dereffed the captured null). We do NOT install the job anywhere (no
/// owner+0x130, no MenuJobQueue, no CSMenuMan dialog). Instead the recurring game task ticks its `Run`
/// privately every frame (see `own_load_pump_tick`) until it self-builds + deserializes + map-streams
/// (m28 mount) and reaches `state==Success`, then drives the title->ingame transition once.
///
/// SAVE-SAFETY ABSOLUTE: BUILD only allocates + copies a template (no save write); the first-tick deser
/// step (`FUN_14082c330`) only READS the save up to world-stream. NO SetState5 here. The same hard
/// c30/fp guard as the other levers is kept as a belt-and-braces precondition even though no write
/// occurs at build time. The transition (the only save-writing step) is separately gated in
/// `own_load_pump_tick`. Keeps `simulated_button_presses_total = 0`.
unsafe fn own_load_pump_fire(
    base: usize,
    title_owner: usize,
    c30: i32,
    c30_real: bool,
    fp_real: bool,
    fp_level: u32,
    n: u64,
) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Belt-and-braces guard (no write occurs at build, but never act on an unverified parse).
    if !(c30_real && fp_real) {
        append_autoload_debug(format_args!(
            "own-load-pump: GUARD FAIL (c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}) -- NO build -> ABORT (save-safe)"
        ));
        return;
    }
    if title_owner == null {
        append_autoload_debug(format_args!(
            "own-load-pump: ABORT -- threaded title_owner is null -> no build (save-safe)"
        ));
        return;
    }
    if OWN_LOAD_PUMP_JOB.load(Ordering::SeqCst) != 0 {
        // Already built+armed (own_load_drive is one-shot, but guard against a re-entrant fire).
        return;
    }
    // CORRECTED ctx source (bd loadgame-owner-ctx-is-DIALOG-a38-not-mss-CORRECTION-2026-06-22): the
    // LoadGame factory's owner_ctx (r9) and ctx_parent (rdx) come from the live CS::TitleTopDialog,
    // NOT from CSMenuSystemSaveLoad. The golden factory site reads `mov 0xa38(%r13),%r9` where r13 IS
    // the dialog (the prior mss+0xa38 reading misidentified r13 as mss and read back garbage -> the AV).
    // Locate the live dialog at owner+0xe0 (vtable-gated, same recipe as locate_live_loadgame_node).
    let dialog = unsafe { safe_read_usize(title_owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }
        .filter(|&v| v != null && v != 0)
        .unwrap_or(0);
    let dialog_vt = if dialog != 0 {
        unsafe { safe_read_usize(dialog) }.unwrap_or(0)
    } else {
        0
    };
    if dialog == 0 || dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "own-load-pump: ABORT -- live TitleTopDialog not up (owner+0x{:x}=0x{dialog:x} vt=0x{dialog_vt:x} want 0x{:x}) -> no build (save-safe)",
            TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return;
    }
    let ctx_parent = dialog + DIALOG_CTX_PARENT_50_OFFSET;
    // owner_ctx = *(dialog+0xa38) = CS::TitleFlowContext (written UNCONDITIONALLY by the dialog ctor
    // 0x1409a82d0, so it is valid at the settled press-any-button title -- unlike mss+0xa38 which read
    // back uninitialized garbage). FAIL CLOSED (no build) if it is not a plausible heap pointer:
    // passing NULL is exactly what AV'd before, and a real ctx is the whole point of the correction.
    let raw_owner_ctx =
        unsafe { safe_read_usize(dialog + DIALOG_OWNER_CTX_A38_OFFSET) }.unwrap_or(0);
    if !(raw_owner_ctx > OWNER_CTX_MIN_PLAUSIBLE_PTR && raw_owner_ctx < OWNER_CTX_MAX_PLAUSIBLE_PTR)
    {
        append_autoload_debug(format_args!(
            "own-load-pump: ABORT -- owner_ctx *(dialog+0x{:x})=0x{raw_owner_ctx:x} is not a plausible TitleFlowContext (dialog=0x{dialog:x}) -> no build (save-safe)",
            DIALOG_OWNER_CTX_A38_OFFSET
        ));
        return;
    }
    let owner_ctx = raw_owner_ctx;
    let want_slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "own-load-pump: BUILD 0x{:x}(out, ctx_parent=dialog+0x{:x}=0x{ctx_parent:x}, slot={want_slot}, owner_ctx=*(dialog+0x{:x})=0x{owner_ctx:x}) dialog=0x{dialog:x} -- CORRECTED dialog-derived ctx (golden Continue args) presses=0",
        base + LOADGAME_JOB_BUILD_RVA,
        DIALOG_CTX_PARENT_50_OFFSET,
        DIALOG_OWNER_CTX_A38_OFFSET,
    ));
    // BUILD the LoadGame MenuJobWithContext into a local DLRefCountPtr (factory writes the job ptr into
    // *out with refcount 1). Win64 fastcall (out, ctx_parent, save_slot:i32, owner_ctx).
    // Justify the transmute: LOADGAME_JOB_BUILD_RVA is the prologue-grounded live entry of the menu-heap
    // LoadGame-job factory; the signature matches the static decompile of FUN_140826510.
    let build: unsafe extern "system" fn(*mut usize, usize, i32, usize) -> *mut usize =
        unsafe { std::mem::transmute(base + LOADGAME_JOB_BUILD_RVA) };
    let mut built_job: usize = 0;
    let build_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        build(&raw mut built_job, ctx_parent, want_slot, owner_ctx)
    }));
    if build_ret.is_err() {
        append_autoload_debug(format_args!(
            "own-load-pump: BUILD PANICKED (caught) -- NO pump -> ABORT (save-safe)"
        ));
        return;
    }
    if built_job == null || built_job == 0 {
        append_autoload_debug(format_args!(
            "own-load-pump: BUILD returned a NULL job (built_job=0x{built_job:x}) -- NO pump -> ABORT (save-safe)"
        ));
        return;
    }
    let built_vt = unsafe { safe_read_usize(built_job) }.unwrap_or(null);
    let built_flag = unsafe { safe_read_usize(built_job + MENUJOB_BUILT_FLAG_68_OFFSET) }
        .map(|v| v as u8)
        .unwrap_or(0);
    // Arm the recurring private pump: publish the job ptr + cache owner/InGameStep (mirror the other
    // levers) so the recurring observer keeps logging through the loading screen, and set
    // OWN_LOAD_CONTINUE_FIRED so own_load_stream_observe_recurring runs each frame. Do NOT install the
    // job anywhere -- the recurring task pumps Run directly.
    OWN_LOAD_PUMP_JOB.store(built_job, Ordering::SeqCst);
    OWN_LOAD_OWNER_CACHED.store(title_owner, Ordering::SeqCst);
    let ingame_cached = unsafe { safe_read_usize(title_owner + TITLE_OWNER_JOB_OFFSET) }
        .filter(|&v| v != null)
        .unwrap_or(0);
    OWN_LOAD_INGAMESTEP_CACHED.store(ingame_cached, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_FIRED.store(true, Ordering::SeqCst);
    timeline_event(
        "T_own_load_pump_build",
        n,
        format_args!("c30=0x{c30:x} level={fp_level}"),
    );
    append_autoload_debug(format_args!(
        "own-load-pump: *** BUILT job=0x{built_job:x} vt=0x{built_vt:x} (expect LoadGame dump 0x{:x}) +0x68_built={built_flag} -- ARMED private per-frame pump (NO owner+0x130 install, NO queue, NO dialog) ingame=0x{ingame_cached:x} -- recurring task will tick Run each frame -> self-build -> deser -> m28 stream -> SetState5 transition on Success presses=0 (#{n}) ***",
        MENUJOB_LOADGAME_VTABLE_DUMP_VA,
    ));
}

/// PATH B per-frame PRIVATE PUMP (runs from the recurring game task each frame, gated). If a LoadGame
/// job was built+armed by `own_load_pump_fire`, tick its `Run` exactly the way the native
/// `ExecuteMenuJob` does -- a zero-init `MenuJobResult` and an `FD4Time` carrying the frame delta -- so
/// the job self-builds, deserializes, and map-streams the world WITHOUT the menu system. When the job
/// reaches `state==Success` (deser+map done, m28 mounted), drive the title->ingame transition ONCE via
/// the guarded `continue_confirm`/SetState5 (the same save-safe guard as `own_load_continue_fire`), then
/// latch `OWN_LOAD_PUMP_DONE` so we never re-pump or re-transition.
///
/// SAVE-SAFETY: the pump itself (build+deser+map-stream) is READ-only up to world-stream. The ONLY
/// save-writing step is the final SetState5 transition, which stays HARD-gated on the verified parse
/// (`c30_real && fp_real`, re-checked from the live GameMan+0xc30 and char fingerprint) + the title
/// owner's new-game flag clear -- mirroring `own_load_continue_fire`. No save write before the world is
/// confirmed loading. Every native call is wrapped in `catch_unwind` (a Rust panic is caught; a hardware
/// AV is not). Keeps `simulated_button_presses_total = 0`.
pub(crate) unsafe fn own_load_pump_tick(base: usize, gm: usize, frame_delta: f32) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let job = OWN_LOAD_PUMP_JOB.load(Ordering::SeqCst);
    if job == 0 || job == null {
        return;
    }
    if OWN_LOAD_PUMP_DONE.load(Ordering::SeqCst) {
        return;
    }
    // Build the call buffers exactly as native ExecuteMenuJob/STEP_MenuJobWait do: a zero-init
    // MenuJobResult (8 bytes) and an FD4Time (16 bytes) whose +0x8 f32 holds the frame delta (Run only
    // reads time+8; it writes the FD4Time vtable into time+0 itself). We over-size both buffers to a
    // qword to keep them aligned and writable.
    let mut result: [u8; FD4_TIME_SIZE] = [0u8; FD4_TIME_SIZE]; // >= MENUJOB_RESULT_SIZE; zero state.
    let mut time: [u8; FD4_TIME_SIZE] = [0u8; FD4_TIME_SIZE];
    // Write the f32 frame delta at time+0x8 (Run advances the map-stream sub-job on this).
    time[FD4_TIME_DELTA_8_OFFSET..FD4_TIME_DELTA_8_OFFSET + core::mem::size_of::<f32>()]
        .copy_from_slice(&frame_delta.to_le_bytes());
    let result_ptr = result.as_mut_ptr() as usize;
    let time_ptr = time.as_mut_ptr() as usize;
    // Run(this /*rcx*/, result /*rdx*/, time /*r8*/, param4 /*r9*/) -> *MenuJobResult.
    // Justify the transmute: LOADGAME_JOB_RUN_RVA is the prologue-grounded live entry of the LoadGame
    // MenuJobWithContext::Run (vtable+0x10), signature per the static decompile of FUN_140826e40.
    let run: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(base + LOADGAME_JOB_RUN_RVA) };
    let run_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        run(job, result_ptr, time_ptr, 0)
    }));
    let fired = OWN_LOAD_PUMP_FIRED.fetch_add(1, Ordering::SeqCst) + 1;
    if run_ret.is_err() {
        // A Rust-level panic in Run -> stop pumping (latch done) so we do not re-fault every frame.
        OWN_LOAD_PUMP_DONE.store(true, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "own-load-pump: Run PANICKED (caught) at pump #{fired} (job=0x{job:x}) -> latch DONE, no transition (save-safe)"
        ));
        return;
    }
    // Read back the result state (+0x0) and the inner deser sub-code (+0x4).
    let state = i32::from_le_bytes([
        result[MENUJOB_RESULT_STATE_0_OFFSET],
        result[MENUJOB_RESULT_STATE_0_OFFSET + 1],
        result[MENUJOB_RESULT_STATE_0_OFFSET + 2],
        result[MENUJOB_RESULT_STATE_0_OFFSET + 3],
    ]);
    let subcode = i32::from_le_bytes([
        result[MENUJOB_RESULT_SUBCODE_4_OFFSET],
        result[MENUJOB_RESULT_SUBCODE_4_OFFSET + 1],
        result[MENUJOB_RESULT_SUBCODE_4_OFFSET + 2],
        result[MENUJOB_RESULT_SUBCODE_4_OFFSET + 3],
    ]);
    OWN_LOAD_PUMP_STATE.store(i64::from(state), Ordering::SeqCst);
    OWN_LOAD_PUMP_SUBCODE.store(i64::from(subcode), Ordering::SeqCst);
    // Job header diagnostics: +0x68 built flag flips 0->1 on self-build, +0x70 inner-seq ptr 0->built.
    let built_flag = unsafe { safe_read_usize(job + MENUJOB_BUILT_FLAG_68_OFFSET) }
        .map(|v| v as u8)
        .unwrap_or(0);
    let inner_seq = unsafe { safe_read_usize(job + MENUJOB_INNER_SEQ_70_OFFSET) }.unwrap_or(null);
    // Throttled log (every OWN_LOAD_STREAM_LOG_INTERVAL pumps), plus the first pump.
    if fired == 1 || fired % OWN_LOAD_STREAM_LOG_INTERVAL == 0 {
        append_autoload_debug(format_args!(
            "own-load-pump: pump #{fired} Run(job=0x{job:x}) state={state} (1=Continue 2=Success 3=Failed) subcode={subcode} (deser 5/2/6) +0x68_built={built_flag} +0x70_seq=0x{inner_seq:x} delta={frame_delta}"
        ));
    }
    if state <= MENUJOB_STATE_CONTINUE {
        // Still working (Continue) -- keep pumping next frame.
        return;
    }
    // Terminal: Success (2) or Failed (3). Latch DONE so we stop pumping regardless of the transition.
    OWN_LOAD_PUMP_DONE.store(true, Ordering::SeqCst);
    if state == MENUJOB_STATE_FAILED {
        append_autoload_debug(format_args!(
            "own-load-pump: pump #{fired} reached state=Failed(3) subcode={subcode} -- deser/map FAILED -> NO transition, latch DONE (save-safe)"
        ));
        return;
    }
    // state == Success: the job deserialized + map-streamed (m28). Drive the title->ingame transition
    // ONCE via the guarded SetState5. RE-VERIFY the parse from LIVE state (the build+pump can change
    // GameMan+0xc30) so the save-write transition is gated exactly like own_load_continue_fire.
    let owner = OWN_LOAD_OWNER_CACHED.load(Ordering::SeqCst);
    let c30_live = if gm != null && gm != 0 {
        unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    let c30_real =
        c30_live != GAME_MAN_C30_UNSET && c30_live != 0 && c30_live != FULLREAD_C30_M10_DEFAULT;
    let (fp_real, fp_level, _fp_name_len) = unsafe { char_fingerprint(base) };
    append_autoload_debug(format_args!(
        "own-load-pump: *** pump #{fired} reached state=Success(2) subcode={subcode} -- deser+map-stream DONE (m28 mounted); driving title->ingame transition ONCE (owner=0x{owner:x} c30_live=0x{c30_live:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}) ***"
    ));
    // SAVE-SAFE PROBE: if the verify-only gate is set, the pump has proven the corrected dialog-ctx
    // build reached Success (no AV) with the world map-streamed -- STOP HERE without the save-writing
    // SetState5 transition, so this can run against the real save with zero write risk.
    if own_load_pump_verify_only() {
        append_autoload_debug(format_args!(
            "own-load-pump: VERIFY-ONLY gate set -- reached Success(2) subcode={subcode} (corrected dialog-ctx build+pump OK, no AV); SKIPPING SetState5 transition -> NO save write, latch DONE (save-safe)"
        ));
        return;
    }
    // The transition is the SAME guarded continue_confirm/SetState5 path the legacy lever uses; it
    // re-checks c30_real && fp_real + the owner new-game flag internally and ABORTs (no write) on any
    // failure. Pass the live-re-verified c30 so the guard reflects the post-pump state.
    unsafe {
        own_load_continue_fire(base, owner, c30_live, c30_real, fp_real, fp_level, fired);
    }
}
