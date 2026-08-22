use std::{
    ffi::c_void,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use fromsoftware_shared::FromStatic;

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
pub(super) static OWN_LOAD_GATE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// Trampoline to the original 0x67b100 (set on hook install).
static READ_67B100_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::OWN_LOAD_BODY_LEN;
/// The sliced plaintext slot body the hook feeds: a leaked `&'static [u8]`, exposed to the detour
/// as (ptr, len) atomics so the game-thread detour reads it lock-free. Set BEFORE arming the gate.
pub(crate) use er_telemetry::counters::OWN_LOAD_BODY_PTR;
/// Count of bytes the gated hook fed into the engine buffer on the latched call (verify telemetry).
pub(crate) use er_telemetry::counters::OWN_LOAD_FED_BYTES;
/// One-shot install guard for the 0x67b100 detour.
pub(crate) use er_telemetry::counters::OWN_LOAD_HOOK_INSTALLED;

/// Destination handed to the pass-through consumer call in [`run_native_load_consumer`].
///
/// `FUN_140e6e380` copies `min(dest_size, content_size)` bytes and then calls
/// `FUN_140e6f200` UNCONDITIONALLY, so the release does not depend on the destination being
/// big enough to hold the payload -- and the payload is discarded here regardless, because
/// the whole point of the feed is that the engine parses our bytes instead. Sized only to
/// keep the copy trivially bounded and stack-resident.
const CONSUMER_SCRATCH_BYTES: usize = 64;

/// Run the ORIGINAL `0x67b100` against a scratch destination, purely for the device side
/// effect the feed would otherwise swallow, and measure what it did to the shared job slot.
///
/// WHY THIS EXISTS. `0x67b100` is not just a read -- it is the CONSUMER of a completed
/// load, and the only unconditional caller of the device's request teardown:
///
/// ```text
///   FUN_14067b100(out, size)              gate: saveState == 3 && GameMan+0xdf0 == 0
///     -> FUN_140e6e380(iodev, out, size)  gate: +0x18 && +0x20
///                                               && FUN_14240a180(job) == 0
///                                               && FUN_14240a1f0(job) == 0x14
///          memcpy(out, content, n)
///          FUN_140e6f200(iodev)           <-- releases +0x10/+0x18/+0x20/+0x28
/// ```
///
/// The load side never releases itself: `FUN_140e6e080` case 0x14 with a zero `param_2` --
/// exactly the shape the b80 poll `0x679180(0, 0)` produces -- returns 0 having called
/// nothing, and it is that return which drives `GameMan+0xb80` to RESIDENT(3). So the
/// switch reload's SUBMIT -> DRAIN(b80 == 3) sequence ends with a COMPLETED load still
/// holding `iodev+0x18`/`+0x20`, and the release is owed by the very call this detour was
/// substituting. Swallowing it outright left that job in the slot forever, and the submit
/// builders gate on `iodev+0x10 == 0 && iodev+0x20 == 0` -- so after one profile switch NO
/// save in the process could be built again: not autosave, not rest, not quit-save, not the
/// Save Game row. The measured signature was `oracle_save_dispatch_last_decline_reason =
/// "load-job-latched-0x18+0x20"` repeating without end.
///
/// WHY IT CANNOT RACE A LIVE LOAD. Nothing here decides anything. The game's own guard
/// decides, on the game's own thread, at the same instant it would have run natively: the
/// release happens only when `FUN_14240a1f0(job) == 0x14` (terminal COMPLETE) and
/// `FUN_14240a180(job) == 0` (success). A job that is still in flight reports any other
/// state, `FUN_140e6e380` returns 0, and NOTHING is touched -- no timer, no frame count, no
/// re-derived predicate of ours that could disagree with the engine about whether a load
/// has finished. The outcome is measured either way and published, so a request left in
/// place is visible rather than assumed.
///
/// WHY A SCRATCH DESTINATION. The consumer's payload copy is a side effect we do not want:
/// the engine buffer must end up holding OUR bytes, byte for byte as before this change,
/// including whatever lies past `body.len()`. Pointing the copy at 64 bytes of stack leaves
/// the engine's allocation untouched while still paying the device debt.
unsafe fn run_native_load_consumer(origin: &str) {
    let orig = READ_67B100_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        // Only reachable if the detour ran before `install_own_load_hook` bound the
        // trampoline, which the install order rules out. Left silent rather than logged:
        // the feed below still works, and the stranded-request oracle would report it.
        return;
    }
    let before = er_save_suppress::sample_sl_request_slot();
    let mut scratch = [0_u8; CONSUMER_SCRATCH_BYTES];
    let consumer: unsafe extern "system" fn(usize, u32) -> u8 =
        unsafe { std::mem::transmute(orig) };
    let consumed =
        unsafe { consumer(scratch.as_mut_ptr() as usize, CONSUMER_SCRATCH_BYTES as u32) };
    let after = er_save_suppress::sample_sl_request_slot();
    let outcome = er_save_suppress::note_load_consumer(before, after, origin, true);
    if outcome == er_save_suppress::LOAD_CONSUMER_RELEASED {
        append_autoload_debug(format_args!(
            "own-load: native load consumer 0x67b100 ran before the feed (ret={consumed}) and \
             RELEASED the shared iodev job -- saves stay buildable across this load"
        ));
    }
}

/// Gated detour for 0x67b100. While `OWN_LOAD_GATE` is set, runs the ORIGINAL against a
/// scratch destination so the native load consumer performs its device teardown, then copies
/// our sliced plaintext slot body into the engine-allocated out_buf (`rcx`) for
/// `min(size, body.len())` bytes and returns al=1 -- the engine parses OUR bytes while the
/// device is left in the state the native read would have left it. Otherwise it is a pure
/// pass-through to the original (the native menu loader's reads are never disturbed).
pub(crate) unsafe extern "system" fn read_67b100_hook(out_buf: usize, size: u32) -> u8 {
    const FEED_SUCCESS_RET: u8 = 1;
    if OWN_LOAD_GATE.load(Ordering::SeqCst) {
        let body_ptr = OWN_LOAD_BODY_PTR.load(Ordering::SeqCst);
        let body_len = OWN_LOAD_BODY_LEN.load(Ordering::SeqCst);
        if out_buf != TITLE_OWNER_SCAN_START_ADDRESS && body_ptr != 0 && body_len != 0 {
            // Pay the substituted call's debt to the SL device FIRST, on the game's own
            // terms. Ordering matters only for readability -- the consumer writes to its own
            // scratch, never to `out_buf` -- but it puts the release where the native body
            // would have run it, before the payload the engine actually parses is decided.
            unsafe { run_native_load_consumer("own-load 0x67b100 feed") };
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
                    crate::mh::leak_installed_hook(hook);
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
const WORLDBLOCKRES_UPDATE_RVA: usize = WORLDBLOCKRES_UPDATE_RE_RVA;
/// Phase byte the switch dispatches on: `this+0x35` (9 -> 0xa is the residency transition).
const WBR_PHASE_35_OFFSET: usize = 0x35;
/// FD4 file-load completion gate: `this+0x2f` (recomputed each tick; !=0 lets phase 9 advance to 0xa).
const WBR_GATE_2F_OFFSET: usize = 0x2f;

/// Max phase byte ([this+0x35]) seen across all observed calls. <0xa across the stall == the block's
/// resource-stream never reached residency.
pub(crate) use er_telemetry::counters::OWN_LOAD_WBR_MAX_PHASE;
/// Total calls to `WorldBlockRes::Update` observed via the detour (per-block-per-frame; 0 == the
/// FieldArea update loop never ticked our block on this path).
pub(crate) use er_telemetry::counters::OWN_LOAD_WBR_UPDATE_CALLS;
/// Whether ANY observed block had its FD4 completion gate ([this+0x2f]) set non-zero. false across the
/// stall == the FD4 file-load never completed for any block (the IO/CSFile gap).
pub(crate) static OWN_LOAD_WBR_ANY_GATE_SET: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// Count of successful OWN-LOAD m28 `AddDefaultFileLoadProcess` dispatch calls (one per cap, one-shot
/// per cap pointer). 0 == the lever never fired. Exposed as telemetry `oracle_own_m28_dispatch_fired`.
pub(crate) use er_telemetry::counters::OWN_LOAD_M28_DISPATCH_FIRED;
/// One-shot guard: FD4FileCap pointers we already dispatched `AddDefaultFileLoadProcess` for.
/// `AppendFileLoadProcessor` does NOT early-out on an already-present processor, so a double-call
/// would append a second processor -- this set makes each cap fire exactly once. Const-constructible
/// (`Mutex::new(Vec::new())`) so no lazy init is needed.
#[allow(dead_code)] // Retained: One-shot guard for the m28 dispatch lever; kept with the AppendFileLoadProcessor finding its doc records.
static OWN_LOAD_M28_DISPATCHED_CAPS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
/// Trampoline to the original `WorldBlockRes::Update` (set on hook install).
static WBR_UPDATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// GROUND-TRUTH cap-layout diagnostic: when `WorldBlockRes::Update`'s `this` (the REAL WBR, straight
/// from the engine) is at the stuck phase 2, dump its candidate cap fields READ-ONLY so we locate the
/// FD4FileCap on the authoritative object instead of reconstructing it from the resmgr container.
/// Throttled to the first few sightings (the hook fires ~500k times).
pub(crate) use er_telemetry::counters::WBR_PHASE2_DIAG_CALLS;
const WBR_PHASE2_DIAG_MAX: usize = 24;
const WBR_STUCK_PHASE: u8 = 2;
/// One-shot install guard for the `WorldBlockRes::Update` diagnostic detour.
pub(crate) use er_telemetry::counters::WBR_UPDATE_HOOK_INSTALLED;

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
                            rd(cap0),
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
        if let Some(gate) = unsafe { safe_read_u8(this + WBR_GATE_2F_OFFSET) }
            && gate != 0
        {
            OWN_LOAD_WBR_ANY_GATE_SET.store(true, Ordering::SeqCst);
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
                    crate::mh::leak_installed_hook(hook);
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

// ===== _Common_Finalize OBSERVE-ONLY teardown oracle (Phase-3, bd PHASE3-render-release-is-CommonFinalize)
//
// `CS::InGameStep::_Common_Finalize` is at live/dump/deobf 0x140aed380 == RVA 0xaed380, byte-identical
// across the 1.16.2 dump, the deobf image, and the live module (shift 0). Ghidra-MCP ground-truthed on the
// 1.16.2 dump: STEP_MoveMap_Finish (0x140aec050) lists callee `_Common_Finalize@140aed380`, whose body
// frees GLOBAL_WorldChrMan, GLOBAL_CSDistViewManager (LOD), g_GxDrawContext (FUN_1419eaf90),
// GLOBAL_CSBulletManager/DmgMan/MapItemMan, WorldRes::ResetAreaInfoLists, unloads the loadlistlist
// FileCaps, and sets CSHavokMan+0x90+0x74; it uses the literal EventFlagCaller name
// "CS::InGameStep::_Common_Finalize". It DLPanics on null world singletons, so it is called ONLY when a
// live world exists -> it never fires on the boot autoload (load1) and increments once per SWITCH reload.
//
// DO NOT trust scripts/dump-deobf-shift.py for this VA: it FALSELY reports 0x140aed380 -> 0x140aed290
// (shift -0xf0), a content-region mismatch that lands 0xaed290 MID-INSTRUCTION inside the ADJACENT
// step-setter FUN_140aed270 (the "移動先ステップ未定義" MoveMap step-index helper, also called by
// STEP_MoveMap_Finish). The prior RVA 0xaed290 made MinHook write a 5-byte jmp over FUN_140aed270's live
// body, so when load1's MoveMap-finish executed it the corrupted stream CRASHED (bd
// phase3-v2-load1-crash-is-wrong-hook-rva-mid-instruction-not-env-2026-07-23). The 0xaed380 entry's first
// instruction is `mov [rsp+0x10],rbx` (a clean 5 bytes), so the trampoline relocation is safe.
//
// This detour is OBSERVE-ONLY: it bumps COMMON_FINALIZE_CALLS then ALWAYS calls the original. The counter
// is the Phase-3 teardown oracle -- flat across a broken in-place switch reload (0 finalizes), incrementing
// once per switch once the OUTGOING world is routed through the release. Never alters teardown behavior.
/// `_Common_Finalize` real entry (Ghidra-MCP-grounded on the 1.16.2 dump; base 0x140000000). NOT 0xaed290:
/// that was a false dump-deobf-shift match, mid-instruction in the neighbor FUN_140aed270 -- it crashed
/// load1's MoveMap-finish on 2026-07-23.
const COMMON_FINALIZE_RVA: usize = 0xaed380;
/// Trampoline to the original `_Common_Finalize` (set on hook install).
static COMMON_FINALIZE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);

/// `__fastcall _Common_Finalize(rcx=InGameStep*)` observe-only detour. Increments the teardown oracle,
/// then calls the original UNCHANGED (void return). Nothing in `param_1` is read or written by us.
pub(crate) unsafe extern "system" fn common_finalize_hook(ingame: usize) {
    er_telemetry::counters::COMMON_FINALIZE_CALLS.fetch_add(1, Ordering::SeqCst);
    let orig = COMMON_FINALIZE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return;
    }
    let orig: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(orig) };
    unsafe { orig(ingame) };
}

/// Install the OBSERVE-ONLY `_Common_Finalize` teardown-oracle detour (MhHook + MH_Initialize +
/// queue_enable + MH_ApplyQueued), mirroring `install_wbr_update_hook`. Idempotent. Pure pass-through, so
/// installing it unconditionally at attach never changes teardown behavior and works on any run (A/B safe).
pub(crate) fn install_common_finalize_hook() -> bool {
    if er_telemetry::counters::COMMON_FINALIZE_HOOK_INSTALLED.load(Ordering::SeqCst)
        == OWN_STEPPER_CALL_INC
    {
        return true;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "common-finalize: MH_Initialize failed: {status:?}"
            ));
            return false;
        }
    }
    let Ok(addr) = game_rva(COMMON_FINALIZE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "common-finalize: failed to resolve 0x{COMMON_FINALIZE_RVA:x} rva"
        ));
        return false;
    };
    match unsafe { MhHook::new(addr as *mut c_void, common_finalize_hook as *mut c_void) } {
        Ok(hook) => {
            COMMON_FINALIZE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "common-finalize: queue_enable failed: {status:?}"
                ));
                return false;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    er_telemetry::counters::COMMON_FINALIZE_HOOK_INSTALLED
                        .store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "common-finalize: hooked 0x{addr:x} (OBSERVE-ONLY teardown oracle; pure pass-through)"
                    ));
                    true
                }
                status => {
                    append_autoload_debug(format_args!(
                        "common-finalize: MH_ApplyQueued failed: {status:?}"
                    ));
                    false
                }
            }
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "common-finalize: MhHook::new failed: {status:?}"
            ));
            false
        }
    }
}

// ===== RequestMoveMap BlockId fix (render-handoff freeze root fix, bd er-effects-rs-um9g) ==========
//
// ROOT (RE render-handoff-freeze-worldreswait-loadlist-root-2026-07-18): our in-memory redirect load
// reaches STEP_PlayGame with a stale/`-1` target BlockId (GameMan+0xc30 was set too late for the
// Continue-confirm capture into TitleStep.field10_0xbc), so `InGameStep::RequestMoveMap` skips its
// `FormatV` that builds the world-res loadlist virtual path -> the dest WorldBlockRes is never created
// -> STEP_WorldResWait (mms_step 3) stalls forever -> the world never resumes, draw_group never
// re-enables, the loading cover never lifts (present-but-frozen). FIX (native-ownership, no field
// poking): hook RequestMoveMap and, when ARMED by our own load trigger, if its target BlockId `*param_2`
// is invalid, substitute the freshly-deserialized saved-map BlockId from GameMan+0xc30 (a valid BlockId
// as-is; same encoding, area byte = c30>>24). The game's own FormatV -> LoadlistInit ->
// ProcessMsbLoadLists -> world-stream -> STEP_Finish chain then runs natively and re-enables render.
/// Trampoline to the original `InGameStep::RequestMoveMap` (set on hook install).
static REQUEST_MOVE_MAP_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// ARM countdown: our load trigger sets this to `REQUEST_MOVE_MAP_ARM_WINDOW` right before it drives
/// SetState5/PlayGame. Each RequestMoveMap call while armed decrements it; the fixup fires on the FIRST
/// call whose target BlockId is actually invalid (disarming immediately), and a valid intervening call
/// merely decrements without consuming the arm. This is a WINDOW, not a one-shot: the earlier
/// disarm-on-first-call consumed the arm on a benign valid call (title/early RequestMoveMap) and missed
/// the actual load's stale `-1` call a few calls later, leaving WorldResWait stuck (bug found runtime
/// 2026-07-18, run boot-fix-validate-155035: calls=2 fixups=0, boot stuck at mms 3 for 66s).
pub(crate) use er_telemetry::counters::REQUEST_MOVE_MAP_ARM_COUNTDOWN;
/// One-shot install guard.
pub(crate) use er_telemetry::counters::REQUEST_MOVE_MAP_HOOK_INSTALLED;
/// How many RequestMoveMap calls after a load trigger stay eligible for the fixup. Generous enough to
/// skip benign intervening calls but bounded so the arm never leaks into unrelated later transitions.
const REQUEST_MOVE_MAP_ARM_WINDOW: usize = 8;
/// Times we substituted a valid c30 BlockId into an invalid `*param_2`
/// (telemetry oracle_request_move_map_hook_fixups). >=1 == the fix fired.
pub(crate) use er_telemetry::counters::REQUEST_MOVE_MAP_FIXUPS;
/// Total RequestMoveMap calls seen (telemetry oracle_request_move_map_hook_calls).
pub(crate) use er_telemetry::counters::REQUEST_MOVE_MAP_HOOK_CALLS;
/// Last (param_2-before, c30-substituted) pair, for telemetry/diagnosis.
pub(crate) use er_telemetry::counters::REQUEST_MOVE_MAP_LAST_BEFORE;
pub(crate) use er_telemetry::counters::REQUEST_MOVE_MAP_LAST_C30;

/// Arm the RequestMoveMap BlockId fixup for the next load. Call this at a load trigger (own-load
/// continue / boot autoload SetState5) AFTER the saved map is deserialized into GameMan+0xc30, so the
/// upcoming STEP_PlayGame -> RequestMoveMap gets a valid target BlockId even if the confirm handler
/// captured a stale one.
pub(crate) fn arm_request_move_map_fixup() {
    REQUEST_MOVE_MAP_ARM_COUNTDOWN.store(REQUEST_MOVE_MAP_ARM_WINDOW, Ordering::SeqCst);
}

/// `__fastcall InGameStep::RequestMoveMap(rcx=InGameStep*, rdx=BlockId* param_2, r8d, r9)` fix detour.
/// When armed, corrects an invalid target BlockId so FormatV builds the loadlist path. `param_2` points
/// to a writable caller-stack int in every caller (RE-verified), so `*param_2` is safe to write.
pub(crate) unsafe extern "system" fn request_move_map_fix_hook(
    ingame: usize,
    param2: usize,
    arg3: usize,
    arg4: usize,
) -> usize {
    REQUEST_MOVE_MAP_HOOK_CALLS.fetch_add(1, Ordering::SeqCst);
    let armed = REQUEST_MOVE_MAP_ARM_COUNTDOWN.load(Ordering::SeqCst);
    if armed > 0
        && param2 != TITLE_OWNER_SCAN_START_ADDRESS
        && let Some(before) = unsafe { safe_read_i32(param2) }
    {
        let before_u = before as u32;
        let before_area = (before_u >> 24) & 0xff;
        let invalid = before_u == u32::MAX || before_area >= REQUEST_MOVE_MAP_NONDEBUG_AREA_CEIL;
        let gm = game_man_ptr_or_null();
        let c30 = if gm != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(-1)
        } else {
            -1
        };
        let c30_u = c30 as u32;
        let c30_area = (c30_u >> 24) & 0xff;
        let c30_valid =
            c30_u != u32::MAX && c30 != 0 && c30_area < REQUEST_MOVE_MAP_NONDEBUG_AREA_CEIL;
        if invalid && c30_valid {
            // The actual load's stale/-1 target -- fix it and disarm (done for this load).
            unsafe {
                *(param2 as *mut i32) = c30;
            }
            REQUEST_MOVE_MAP_ARM_COUNTDOWN.store(0, Ordering::SeqCst);
            REQUEST_MOVE_MAP_FIXUPS.fetch_add(1, Ordering::SeqCst);
            REQUEST_MOVE_MAP_LAST_BEFORE.store(u64::from(before_u), Ordering::SeqCst);
            REQUEST_MOVE_MAP_LAST_C30.store(u64::from(c30_u), Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "request-move-map-fix: FIXED *param_2 0x{before_u:x}(area=0x{before_area:x} invalid) -> c30 0x{c30_u:x}(area=0x{c30_area:x}) ingame=0x{ingame:x} armed_left={armed} -- FormatV will now build the loadlist path"
            ));
        } else {
            // Benign intervening call (valid BlockId, or c30 not yet mounted): DO NOT consume the
            // arm -- just decrement the window so the real stale load call a few calls later is
            // still caught. (The earlier one-shot consumed the arm here and missed the load call.)
            REQUEST_MOVE_MAP_ARM_COUNTDOWN.store(armed - 1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "request-move-map-fix: armed passthrough param_2=0x{before_u:x}(area=0x{before_area:x} invalid={invalid}) c30=0x{c30_u:x}(valid={c30_valid}) ingame=0x{ingame:x} armed_left={} -- window decremented, arm kept",
                armed - 1
            ));
        }
    }
    let orig = REQUEST_MOVE_MAP_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    let orig: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { orig(ingame, param2, arg3, arg4) }
}

/// Install the RequestMoveMap BlockId fix detour (MhHook + MH_Initialize + queue_enable +
/// MH_ApplyQueued), mirroring `install_wbr_update_hook`. Idempotent. Passthrough unless ARMED by our
/// own load trigger, so installing it unconditionally (product default) never affects normal play.
pub(crate) fn install_request_move_map_fix_hook() -> bool {
    if REQUEST_MOVE_MAP_HOOK_INSTALLED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC {
        return true;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "request-move-map-fix: MH_Initialize failed: {status:?}"
            ));
            return false;
        }
    }
    let Ok(addr) = game_rva(REQUEST_MOVE_MAP_RVA as u32) else {
        append_autoload_debug(format_args!(
            "request-move-map-fix: failed to resolve 0x{REQUEST_MOVE_MAP_RVA:x} rva"
        ));
        return false;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            request_move_map_fix_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            REQUEST_MOVE_MAP_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "request-move-map-fix: queue_enable failed: {status:?}"
                ));
                return false;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    REQUEST_MOVE_MAP_HOOK_INSTALLED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "request-move-map-fix: hooked 0x{addr:x} (armed-only BlockId fixup; passthrough otherwise)"
                    ));
                    true
                }
                status => {
                    append_autoload_debug(format_args!(
                        "request-move-map-fix: MH_ApplyQueued failed: {status:?}"
                    ));
                    false
                }
            }
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "request-move-map-fix: MhHook::new failed: {status:?}"
            ));
            false
        }
    }
}

// ===== STEP_WorldResWait streaming-settle DEFER-RELEASE hook (armed switch-reload dip fix) ==========
//
// ROOT (bd reload-overlap-fix-design-worldreswait-defer-release-on-streaming-settle-2026-07-24, RE
// synthesis Candidate A, ermaporch 1.16.2): the armed in-place switch reload releases movability at
// CS::MoveMapStep::STEP_WorldResWait (dump 0x140af9cf0) the instant the target WorldBlockRes reaches
// residency phase 0xa -- which does NOT wait on CS::CSWorldGeomMan geometry streaming. Because the mod's
// lightweight return-title SKIPS _Common_Finalize (persistent world), the geometry re-stream is deferred
// PAST that residency release, so the player becomes movable WHILE CSWorldGeomMan is still streaming =
// the ~20fps movable-while-streaming dip (stream_overlap oracle: epoch2 5/5 overlap @20fps).
//
// FIX LEVER: STEP_WorldResWait gates its ENTIRE ready branch (player warp FUN_140622d20 +0xb7b0/b7b8=pos
// +0xb7c0=1, and step advance FUN_140af9d90 requestedState++) on `FUN_140624bd0(fieldArea) != 0`. That
// gate (deobf 0x624bd0, byte-identical to the dump; the shift tool's -0xe0/-0xf0 is a FALSE match in this
// low-.text region -- resolved instead by unique prologue byte-search) is called EXCLUSIVELY from
// STEP_WorldResWait (xrefs: 1 code caller + 1 table ref), so hooking it affects ONLY the WorldResWait
// release and nothing else. While a switch reload is ARMED and geometry has not settled, we return AL=0
// (not-ready) so STEP_WorldResWait takes its OWN not-ready branch (native _CheckEternityLoading pumps the
// loading cover; no warp, no advance), then release with AL=1 once CSWorldGeomMan settles for K frames.
//
// SAFETY / why this is the safe hook point (vs faking the inner residency predicate FUN_14066d3e0, which
// has 3 callers, or hooking STEP_WorldResWait itself, whose true deobf entry the shift tool mis-resolves):
//   * We NEVER write WorldBlockRes phase/gate bytes (+0x35/+0x2f/+0x2d) -- the block reaches genuine
//     residency in its real object; we only defer the STEP transition the game keys on the gate's return.
//   * BEFORE residency we call the original every frame (identical to native: maintenance runs, residency
//     progresses), so the fix can never PREVENT residency from being reached.
//   * The original's ready path calls FUN_14066d610 (a NON-idempotent worldres pending-vector erase +
//     block+0x2c clear) exactly ONCE, on the first residency frame -- so once residency is SEEN we STOP
//     calling the original (returning our own hold value) to avoid repeat-popping/corrupting that vector.
//   * SCOPE (all three, else pure passthrough): worldreswait_hold_enabled() (default-OFF opt-in marker,
//     evaluated at arm) && switch_reload_active() (phase >= RETURN_TITLE_REQUESTED; IDLE on boot/load1) &&
//     SYSTEM_QUIT_ARM_PLAYER_WAS_ABSENT==0 (in-world switch, not a boot self-reload) && a per-switch ARM
//     latch set only from own_load_switch_reload_fire. On boot/load1 nothing arms -> pure passthrough (the
//     PHASE3 load1-softlock trap is avoided: we do NOT key on FRESH_DESER_COUNT, which fires on load1).
//   * BOUNDED FAIL-SOFT: if CSWorldGeomMan never settles within WORLDRESWAIT_HOLD_WAIT_MAX frames, release
//     anyway (today's in-place behavior) and latch RELEASED_ON_FAILSOFT -- a stalled settle can never
//     softlock the switch.

/// `FUN_140624bd0` real entry -- STEP_WorldResWait's residency gate. DEOBF RVA (base 0x140000000),
/// byte-identical to the 1.16.2 dump (shift 0). NOT the shift tool's 0x624af0 (-0xe0 mid-instruction
/// false match in this low-.text staircase region); resolved by unique prologue byte-search
/// (`48 89 5c 24 08 57 48 83 ec 30 48 8b d9 0f 29 74 24 20 48 8d 51 2c 0f 28 f1 48 8b 49 10`, 1 hit) and
/// disasm-verified (first insn `mov %rbx,0x8(%rsp)` = a clean 5-byte MinHook boundary).
const WORLDRESWAIT_GATE_RVA: usize = WORLDRESWAIT_FIELDAREA_GATE_RVA;
/// Consecutive settled frames required before releasing the hold (the design's K~8-16; mirrors the game's
/// own FieldArea stabilization countdown). Filters a one-frame settle blip on the persistent world.
const WORLDRESWAIT_HOLD_SETTLE_FRAMES: usize = 12;
/// Bounded fail-soft cap: max frames to hold waiting for CSWorldGeomMan to settle before releasing anyway
/// (fall back to today's in-place release; no softlock). Mirrors OUTGOING_TEARDOWN_WAIT_MAX (loaders.rs)
/// and stays far under the runtime cap (`.auto/runtime_timeout_cap_seconds`).
const WORLDRESWAIT_HOLD_WAIT_MAX: usize = 900;
/// Lowest plausible heap pointer -- filters null / freed CSWorldGeomMan singleton slots.
const WORLDRESWAIT_GEOM_HEAP_LO: usize = 0x10000;
/// CS::CSWorldGeomMan::Update field offsets (GHIDRA-CONFIRMED @0x1406d31f0; version-stable dump==live;
/// the exact fields crates/er-telemetry stream_overlap.rs samples). `+0xd0`(u8)=did-work-this-frame,
/// `+0xf0/0xf4/0xf8/0x100`(i32)=per-frame work accumulators, `+0x108/0x109`(u8)=all-blocks-ready flags.
/// (`+0x104` registered-block count is NONZERO at rest -- deliberately NOT gated on.)
const GEOM_DID_WORK_D0_OFFSET: usize = 0xd0;
const GEOM_WORK_ACC_F0_OFFSET: usize = 0xf0;
const GEOM_WORK_ACC_F4_OFFSET: usize = 0xf4;
const GEOM_WORK_ACC_F8_OFFSET: usize = 0xf8;
const GEOM_WORK_ACC_100_OFFSET: usize = 0x100;
const GEOM_ALL_READY_A_108_OFFSET: usize = 0x108;
const GEOM_ALL_READY_B_109_OFFSET: usize = 0x109;
/// Trampoline to the original `FUN_140624bd0` (set on hook install).
static WORLDRESWAIT_GATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);

/// True iff CS::CSWorldGeomMan reports geometry streaming SETTLED THIS frame: no work done this frame,
/// both all-blocks-ready flags set, and all per-frame work accumulators zero. Passive: resolves the
/// singleton via the eldenring reflection lookup (no vtable call, no D3D12) and fault-safe raw reads; a
/// faulted read or a torn-down singleton -> false (never a spurious settle).
fn worldreswait_geom_settled_once() -> bool {
    let geom = eldenring::cs::CSWorldGeomMan::instance_ptr()
        .map(|p| p as usize)
        .unwrap_or(0);
    if geom < WORLDRESWAIT_GEOM_HEAP_LO {
        return false;
    }
    let did_work = unsafe { safe_read_u8(geom + GEOM_DID_WORK_D0_OFFSET) };
    let ready_a = unsafe { safe_read_u8(geom + GEOM_ALL_READY_A_108_OFFSET) };
    let ready_b = unsafe { safe_read_u8(geom + GEOM_ALL_READY_B_109_OFFSET) };
    let f0 = unsafe { safe_read_i32(geom + GEOM_WORK_ACC_F0_OFFSET) };
    let f4 = unsafe { safe_read_i32(geom + GEOM_WORK_ACC_F4_OFFSET) };
    let f8 = unsafe { safe_read_i32(geom + GEOM_WORK_ACC_F8_OFFSET) };
    let f100 = unsafe { safe_read_i32(geom + GEOM_WORK_ACC_100_OFFSET) };
    matches!(did_work, Some(0))
        && matches!(ready_a, Some(1))
        && matches!(ready_b, Some(1))
        && matches!(f0, Some(0))
        && matches!(f4, Some(0))
        && matches!(f8, Some(0))
        && matches!(f100, Some(0))
}

/// The full switch-reload scope for the WorldResWait hold: this switch's release is ARMED to hold AND we
/// are inside a genuine in-world System->Quit switch reload. On boot/load1 (phase IDLE, nothing armed) or
/// a spurious title-phase self-reload (player was absent at arm) this is false -> pure passthrough.
fn worldreswait_hold_armed_and_scoped() -> bool {
    er_telemetry::counters::WORLDRESWAIT_HOLD_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
        && crate::experiments::gating::switch_reload_active()
        && er_telemetry::counters::SYSTEM_QUIT_ARM_PLAYER_WAS_ABSENT.load(Ordering::SeqCst) == 0
}

/// Call the original `FUN_140624bd0(rcx=FieldArea*, xmm1=f32) -> AL` (residency gate). Returns 0 if the
/// trampoline is not yet set (fail-safe not-ready). NOTE the second argument is the xmm1 FLOAT
/// STEP_WorldResWait passes in (`MOVSS XMM1,[param2+8]`), NOT an integer -- the gate saves it to xmm6 and
/// forwards it to FUN_14066da60. On Win64 an `f32` at argument position 1 is placed in xmm1 by the ABI,
/// so this signature forwards the float verbatim (the incoming rdx is unused by the gate, which recomputes
/// `&currentBlockId` = fieldArea+0x2c). Declaring it `f32` is what keeps the float from being clobbered
/// across our atomic/logging work before we call the original.
unsafe fn worldreswait_gate_call_orig(field_area: usize, load_delta: f32) -> u8 {
    let orig = WORLDRESWAIT_GATE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return 0;
    }
    let orig: unsafe extern "system" fn(usize, f32) -> u8 = unsafe { std::mem::transmute(orig) };
    unsafe { orig(field_area, load_delta) }
}

/// `__fastcall FUN_140624bd0(rcx=FieldArea*, xmm1=f32 load_delta) -> AL` defer-release detour. Pure
/// passthrough unless a switch reload is ARMED + scoped. While held it returns AL=0 so STEP_WorldResWait
/// keeps looping in its native not-ready branch (loading cover pumped, no warp, no advance); it releases
/// with AL=1 once CSWorldGeomMan settles for K frames or the fail-soft cap expires. See the section
/// header. `load_delta` is the xmm1 float the caller passes in -- declared as a real `f32` param so the
/// ABI keeps it in xmm1 and we forward it to the original untouched.
pub(crate) unsafe extern "system" fn worldreswait_gate_hook(
    field_area: usize,
    load_delta: f32,
) -> u8 {
    er_telemetry::counters::WORLDRESWAIT_GATE_HOOK_CALLS.fetch_add(1, Ordering::SeqCst);
    // Fast path: not an armed/scoped switch reload -> behave EXACTLY as the original (load1/boot safe).
    if !worldreswait_hold_armed_and_scoped() {
        return unsafe { worldreswait_gate_call_orig(field_area, load_delta) };
    }
    // Armed. Until residency is SEEN, run the real gate every frame so maintenance runs + residency
    // progresses (native-identical), and the gate's one-shot FUN_14066d610 residency-pop fires exactly
    // once (on the frame residency is first reached).
    if er_telemetry::counters::WORLDRESWAIT_RESIDENCY_SEEN.load(Ordering::SeqCst) == 0 {
        let real = unsafe { worldreswait_gate_call_orig(field_area, load_delta) };
        if real == 0 {
            return 0; // not resident yet -- identical to native (STEP keeps loading)
        }
        // Residency just reached (the ONE legit pop happened inside the original). Begin the hold; do NOT
        // call the original again on subsequent frames (that would repeat the non-idempotent pop).
        er_telemetry::counters::WORLDRESWAIT_RESIDENCY_SEEN.store(1, Ordering::SeqCst);
        er_telemetry::counters::WORLDRESWAIT_HOLD_ENGAGED.store(1, Ordering::SeqCst);
        er_telemetry::counters::WORLDRESWAIT_HOLD_WAIT_TICKS.store(0, Ordering::SeqCst);
        er_telemetry::counters::WORLDRESWAIT_SETTLE_STREAK.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "worldreswait-hold: RESIDENCY reached while armed -- deferring STEP_WorldResWait warp+advance until CSWorldGeomMan settles (fieldArea=0x{field_area:x}); loading cover held (native _CheckEternityLoading pumps)"
        ));
        // fall through to the hold decision this same frame (defer immediately)
    }
    // Holding: decide release-on-settle / fail-soft / keep-holding. Never call the original here.
    let waited =
        er_telemetry::counters::WORLDRESWAIT_HOLD_WAIT_TICKS.fetch_add(1, Ordering::SeqCst) + 1;
    if worldreswait_geom_settled_once() {
        let streak =
            er_telemetry::counters::WORLDRESWAIT_SETTLE_STREAK.fetch_add(1, Ordering::SeqCst) + 1;
        if streak >= WORLDRESWAIT_HOLD_SETTLE_FRAMES {
            er_telemetry::counters::WORLDRESWAIT_HOLD_ARMED.store(0, Ordering::SeqCst);
            er_telemetry::counters::WORLDRESWAIT_RELEASED_ON_SETTLE.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "worldreswait-hold: RELEASE on settle (streak={streak} held_frames_total={} waited={waited}) -- CSWorldGeomMan settled; STEP_WorldResWait warps+advances now (movability begins at genuine world-stable)",
                er_telemetry::counters::WORLDRESWAIT_HELD_FRAMES.load(Ordering::SeqCst)
            ));
            return 1; // release: STEP_WorldResWait ready branch (warp + advance) runs once
        }
    } else {
        er_telemetry::counters::WORLDRESWAIT_SETTLE_STREAK.store(0, Ordering::SeqCst);
    }
    if waited >= WORLDRESWAIT_HOLD_WAIT_MAX {
        er_telemetry::counters::WORLDRESWAIT_HOLD_ARMED.store(0, Ordering::SeqCst);
        er_telemetry::counters::WORLDRESWAIT_RELEASED_ON_FAILSOFT.store(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "worldreswait-hold: FAIL-SOFT release after {waited} frames without CSWorldGeomMan settle (held_frames_total={}) -- falling back to today's in-place release; no softlock, no regression",
            er_telemetry::counters::WORLDRESWAIT_HELD_FRAMES.load(Ordering::SeqCst)
        ));
        return 1; // fail-soft release (today's behavior; STEP warps+advances)
    }
    er_telemetry::counters::WORLDRESWAIT_HELD_FRAMES.fetch_add(1, Ordering::SeqCst);
    0 // hold: STEP_WorldResWait takes its native not-ready branch (_CheckEternityLoading; no warp/advance)
}

/// Arm the WorldResWait defer-release hold for THIS switch reload. Call from `own_load_switch_reload_fire`
/// at the SetState5/continue point (the same site as `arm_request_move_map_fixup`), so the hold covers the
/// upcoming STEP_WorldResWait. Gated by the default-OFF opt-in marker + the switch scope, so on load1/boot
/// or when the marker is absent it is a no-op and the gate hook stays a pure passthrough.
pub(crate) fn arm_worldreswait_hold() {
    if !crate::experiments::gating::worldreswait_hold_enabled() {
        return;
    }
    if !crate::experiments::gating::switch_reload_active() {
        return;
    }
    if er_telemetry::counters::SYSTEM_QUIT_ARM_PLAYER_WAS_ABSENT.load(Ordering::SeqCst) != 0 {
        return;
    }
    er_telemetry::counters::WORLDRESWAIT_RESIDENCY_SEEN.store(0, Ordering::SeqCst);
    er_telemetry::counters::WORLDRESWAIT_HOLD_WAIT_TICKS.store(0, Ordering::SeqCst);
    er_telemetry::counters::WORLDRESWAIT_SETTLE_STREAK.store(0, Ordering::SeqCst);
    er_telemetry::counters::WORLDRESWAIT_HOLD_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "worldreswait-hold: ARMED for switch reload (marker on, in-world switch) -- STEP_WorldResWait release will defer until CSWorldGeomMan settles (cap {WORLDRESWAIT_HOLD_WAIT_MAX} frames, K={WORLDRESWAIT_HOLD_SETTLE_FRAMES})"
    ));
}

/// Clear the per-switch WorldResWait hold latches so each new switch gets a fresh hold. Called from
/// `reset_switch_reload_latches` on every switch arm. The run-cumulative outcome counters (ENGAGED,
/// HELD_FRAMES, RELEASED_ON_*) are deliberately NOT reset here so a run's outcome stays attributable.
pub(crate) fn reset_worldreswait_hold_latches() {
    er_telemetry::counters::WORLDRESWAIT_HOLD_ARMED.store(0, Ordering::SeqCst);
    er_telemetry::counters::WORLDRESWAIT_RESIDENCY_SEEN.store(0, Ordering::SeqCst);
    er_telemetry::counters::WORLDRESWAIT_HOLD_WAIT_TICKS.store(0, Ordering::SeqCst);
    er_telemetry::counters::WORLDRESWAIT_SETTLE_STREAK.store(0, Ordering::SeqCst);
}

/// Install the STEP_WorldResWait gate defer-release detour (MhHook + MH_Initialize + queue_enable +
/// MH_ApplyQueued), mirroring `install_request_move_map_fix_hook`. Idempotent. Pure passthrough unless a
/// switch reload is ARMED + scoped, so installing it unconditionally (product default) never affects
/// normal gameplay map transitions, boot, or load1.
pub(crate) fn install_worldreswait_gate_hook() -> bool {
    if er_telemetry::counters::WORLDRESWAIT_GATE_HOOK_INSTALLED.load(Ordering::SeqCst)
        == OWN_STEPPER_CALL_INC
    {
        return true;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "worldreswait-gate: MH_Initialize failed: {status:?}"
            ));
            return false;
        }
    }
    let Ok(addr) = game_rva(WORLDRESWAIT_GATE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "worldreswait-gate: failed to resolve 0x{WORLDRESWAIT_GATE_RVA:x} rva"
        ));
        return false;
    };
    match unsafe { MhHook::new(addr as *mut c_void, worldreswait_gate_hook as *mut c_void) } {
        Ok(hook) => {
            WORLDRESWAIT_GATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "worldreswait-gate: queue_enable failed: {status:?}"
                ));
                return false;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    er_telemetry::counters::WORLDRESWAIT_GATE_HOOK_INSTALLED
                        .store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "worldreswait-gate: hooked 0x{addr:x} (armed-only STEP_WorldResWait defer-release; passthrough otherwise)"
                    ));
                    true
                }
                status => {
                    append_autoload_debug(format_args!(
                        "worldreswait-gate: MH_ApplyQueued failed: {status:?}"
                    ));
                    false
                }
            }
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "worldreswait-gate: MhHook::new failed: {status:?}"
            ));
            false
        }
    }
}

/// Locate the on-disk save file (`.../EldenRing/<steamid>/ER0000.sl2` or `.co2`) and read its bytes.
/// The directory is built by the NATIVE builder 0x140e0e680 (`SAVE_DIR_BUILDER_RVA`) -- the same
/// path the engine uses -- so we never hardcode the user-data/steamid prefix. Inside that directory
/// we pick the save file by extension (`.sl2`/`.co2`) rather than assuming an exact filename, so the
/// probe works for vanilla and Seamless without a hardcoded name (bd dont-hardcode-savefile-tied).
/// Optional per-switch cross-file save-source override for the programmatic (file,slot) switch. Reads a
/// game-dir control file (`er-effects-switch-save-file.txt`) whose contents are a single save path the
/// game can open (a Windows path, e.g. `A:\...\150-Banon\ER0000.sl2`). None when absent/empty. The
/// harness writes the target FILE here before writing the slot; own_load_read_sl2_bytes reads it FIRST.
fn switch_save_file_override() -> Option<String> {
    let path = game_directory_path()?.join("er-effects-switch-save-file.txt");
    let contents = std::fs::read_to_string(path).ok()?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

pub(crate) unsafe fn own_load_read_sl2_bytes(base: usize) -> Option<Vec<u8>> {
    const REQ_DIR_SANE_MAX_CU: usize = 320;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // A staged-source rejection is terminal for this process: the source and stage root are
    // write-once, so retrying the same resolver cannot change its answer. Correct callers check the
    // terminal latch before entering. Reaching this branch means a caller regressed; record the
    // identical recurrence semaphore and do no disk/native-builder work.
    if own_load_save_rejection_terminal() {
        let fingerprint = own_load_save_rejection_fingerprint();
        let observation = record_own_load_save_rejection(fingerprint);
        let repeats = own_load_save_repeated_identical_rejections();
        if repeats <= 8 || repeats.is_power_of_two() {
            append_autoload_debug(format_args!(
                "own-load: TERMINAL save rejection re-entered fingerprint=0x{fingerprint:016x} observation={observation:?} repeated_identical={repeats} -- fail-closed; resolver not retried"
            ));
        }
        return None;
    }
    // Deterministic runtime proof for YK0J: reject this Rust-side resolution exactly once while the
    // native game load continues from its already-populated private stage. No file is removed or
    // renamed, so the read-only configured source and game-owned active write target stay intact.
    if let Some(source) = configured_save_file() {
        let candidates = active_default_save_file_names();
        if let Some((fingerprint, observation)) = own_load_save_rejection_probe(&source, candidates)
        {
            append_autoload_debug(format_args!(
                "own-load: YK0J PROBE armed source='{}' candidates={:?} fingerprint=0x{fingerprint:016x}; forced one terminal unresolvable-save verdict observation={observation:?} (native staged load remains intact)",
                source.display(),
                candidates
            ));
            return None;
        }
    }
    // STAGED-SAVE DIRECT READ (feed-deserialize softlock fix, er-effects-rs, 2026-07-03). When a
    // save is staged/redirected via DLL config / `ER_EFFECTS_SAVE_FILE`, read THAT file directly instead of the
    // native-builder + `fs::read_dir` path below. Reasons this is the correct + robust source:
    //   * It is the EXACT file the CreateFileW redirect rewrites the game's save I/O onto (root +
    //     `EldenRing/<steamid>/ER0000.sl2`), so its bytes are what the game itself reads/writes.
    //   * It is PRE-POPULATED before launch, so the read never depends on the game's save-dir being
    //     written yet -- the timing coupling that softlocked the switch when the portrait pipeline's
    //     per-frame slack was removed (the feed-deserialize fired before any save existed and the
    //     native builder's real AppData dir was empty because the redirect bypasses `read_dir`).
    //   * The native builder returns the real `AppData\Roaming\EldenRing\<steamid>` dir, which the
    //     redirect does NOT cover for directory enumeration -> `read_dir` saw an empty dir and the
    //     confirm blocked (lib.rs:198: the DLL must never read the default user save dir).
    // Same std::fs access the redirect enforcer already uses successfully from this DLL under Proton
    // (save_redirect::save_override_redirect_root_w). The full multi-slot save is returned; the
    // caller slices the picked slot exactly as it does for the native-builder bytes.
    //
    // RUNTIME FOREIGN PICK OVERRIDE (Load-Save-Profiles pick-override fix, er-effects-rs, 2026-07-15):
    // When the human-driven "Load Save Profiles" path has COMMITTED a foreign slot this switch,
    // `system_quit_save_swap_prepare_selected_slot` has already overwritten the game-owned ACTIVE
    // `%APPDATA%/EldenRing/<steamid>/ER0000.{sl2,co2}` file with the picked slot's bytes (and re-committed
    // it after the return-title save). Those committed bytes -- NOT the configured `save_file` -- are what
    // the user just picked, so the feed MUST read the committed file here. The configured `save_file`
    // stays the INITIAL boot autoload default (it wins only when no runtime pick is committed): reading it
    // over a fresh foreign commit is exactly the pick-override bug (preview showed the picked character but
    // the game loaded the config default). Read the committed path DIRECTLY -- same std::fs source as the
    // configured-direct branch below -- because in direct mode the CreateFileW redirect does not cover the
    // native builder's `read_dir` enumeration (see the native-builder note below), so relying on the dir
    // walk to observe the just-committed bytes is timing/redirect fragile.
    // RUNTIME CROSS-FILE OVERRIDE (programmatic (file,slot) switch, 2026-07-18). The harness sets the
    // target save FILE for THIS switch via a game-dir control file (er-effects-switch-save-file.txt = a
    // Windows path the game can open, e.g. A:\...\150-Banon\ER0000.sl2). Read it FIRST so a programmatic
    // cross-file switch loads an ARBITRARY vanilla file READ-ONLY, in-memory (the source is only read;
    // the caller slices the picked slot exactly like the boot TOML save_file path). Absent/empty -> fall
    // through to the existing committed-foreign / configured precedence (within-file switches leave it
    // unset). Same std::fs read the redirect enforcer + configured-direct branch already use.
    if let Some(override_path) = switch_save_file_override() {
        match std::fs::read(&override_path) {
            Ok(mut bytes)
                if bytes.len() as u64 == crate::experiments::SAVE_OVERRIDE_EXPECTED_BYTES =>
            {
                normalize_save_bytes_to_active_steam_id(
                    base,
                    &mut bytes,
                    "own-load-switch-file-override",
                );
                append_autoload_debug(format_args!(
                    "own-load: read SWITCH FILE OVERRIDE \"{}\" ({} bytes) for slicing (programmatic cross-file (file,slot) switch overrides configured save_file for this load)",
                    override_path,
                    bytes.len()
                ));
                return Some(bytes);
            }
            Ok(bytes) => {
                append_autoload_debug(format_args!(
                    "own-load: switch file override \"{}\" wrong size ({} bytes != {}) -- falling back to committed/configured",
                    override_path,
                    bytes.len(),
                    crate::experiments::SAVE_OVERRIDE_EXPECTED_BYTES
                ));
            }
            Err(e) => {
                append_autoload_debug(format_args!(
                    "own-load: switch file override \"{}\" read failed ({e}) -- falling back to committed/configured",
                    override_path
                ));
            }
        }
    }
    if let Some(committed_path) = system_quit_committed_foreign_save_path() {
        match std::fs::read(&committed_path) {
            Ok(mut bytes)
                if bytes.len() as u64 == crate::experiments::SAVE_OVERRIDE_EXPECTED_BYTES =>
            {
                normalize_save_bytes_to_active_steam_id(
                    base,
                    &mut bytes,
                    "own-load-committed-foreign",
                );
                append_autoload_debug(format_args!(
                    "own-load: read COMMITTED FOREIGN save \"{}\" ({} bytes) for slicing (Load-Save-Profiles pick overrides configured save_file for this load)",
                    committed_path,
                    bytes.len()
                ));
                return Some(bytes);
            }
            Ok(bytes) => {
                append_autoload_debug(format_args!(
                    "own-load: committed foreign save \"{}\" wrong size ({} bytes != {}) -- falling back to configured/native save-dir",
                    committed_path,
                    bytes.len(),
                    crate::experiments::SAVE_OVERRIDE_EXPECTED_BYTES
                ));
            }
            Err(e) => {
                append_autoload_debug(format_args!(
                    "own-load: committed foreign save \"{}\" read failed ({e}) -- falling back to configured/native save-dir",
                    committed_path
                ));
            }
        }
    }
    if let Some(path) = configured_or_default_save_file() {
        match std::fs::read(&path) {
            Ok(mut bytes)
                if bytes.len() as u64 == crate::experiments::SAVE_OVERRIDE_EXPECTED_BYTES =>
            {
                normalize_save_bytes_to_active_steam_id(base, &mut bytes, "own-load-staged-config");
                append_autoload_debug(format_args!(
                    "own-load: read STAGED save \"{}\" ({} bytes) for slicing (configured direct -- redirect-consistent, timing-independent)",
                    path.display(),
                    bytes.len()
                ));
                return Some(bytes);
            }
            Ok(bytes) => {
                append_autoload_debug(format_args!(
                    "own-load: staged configured save \"{}\" wrong size ({} bytes != {}) -- falling back to native save-dir builder",
                    path.display(),
                    bytes.len(),
                    crate::experiments::SAVE_OVERRIDE_EXPECTED_BYTES
                ));
            }
            Err(e) => {
                append_autoload_debug(format_args!(
                    "own-load: staged configured save \"{}\" read failed ({e}) -- falling back to native save-dir builder",
                    path.display()
                ));
            }
        }
    }
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
    // Asymmetric container rule (user spec 2026-08-02, bd
    // `save-container-mode-lock-is-asymmetric-seamless-takes-both-2026-08-02`): Seamless loads
    // `ER0000.co2` OR `ER0000.sl2`, preferring the co-op container; vanilla loads ONLY `.sl2` and
    // must never silently pick up a Seamless `.co2`. If no candidate is present, behave as "no
    // save" so the configured-save / missing-save picker path can ask the user for a file.
    let candidates = active_default_save_file_names();
    let Some(path) = candidates
        .iter()
        .map(|name| dir_path.join(name))
        .find(|path| {
            std::fs::metadata(path)
                .map(|meta| {
                    meta.is_file() && meta.len() == crate::experiments::SAVE_OVERRIDE_EXPECTED_BYTES
                })
                .unwrap_or(false)
        })
    else {
        if missing_save_selection_pending() {
            append_autoload_debug(format_args!(
                "own-load: no loadable save under dir=\"{}\" while the missing-save picker is pending -- deferred without consuming the terminal rejection",
                dir_path.display()
            ));
            return None;
        }
        if !direct_save_file_source_active() {
            append_autoload_debug(format_args!(
                "own-load: no loadable default-user save under dir=\"{}\" -- tried {:?}; no staged source is active, so this ordinary absence is not a terminal staged-source rejection",
                dir_path.display(),
                candidates
            ));
            return None;
        }
        let fingerprint = own_load_save_rejection_signature(&dir_path, candidates);
        let observation = record_own_load_save_rejection(fingerprint);
        append_autoload_debug(format_args!(
            "own-load: no loadable save under dir=\"{}\" -- tried {:?} for the active mode; TERMINAL fail-closed rejection fingerprint=0x{fingerprint:016x} observation={observation:?} (no retry; restart/picker-source replacement required)",
            dir_path.display(),
            candidates
        ));
        return None;
    };
    match std::fs::read(&path) {
        Ok(mut bytes) => {
            normalize_save_bytes_to_active_steam_id(base, &mut bytes, "own-load-native-dir");
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

pub(crate) use er_title_flow::OWN_LOAD_STREAM_LOG_INTERVAL;
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
    if frames.is_multiple_of(OWN_LOAD_STREAM_LOG_INTERVAL) {
        let ig = ingame.unwrap_or(null);
        let mms = movemapstep.unwrap_or(null);
        let rm = resmgr.unwrap_or(null);
        append_autoload_debug(format_args!(
            "own-load-stream: frame={frames} (n={n}) owner_state={owner_state} owner_req={owner_req_state} ingame=0x{ig:x} mms=0x{mms:x} mms_state={mms_state} resmgr=0x{rm:x} block_count={block_count} req_coord=0x{req_coord:x} io_inflight=0x{io_inflight:x} io_reqhandle=0x{io_reqhandle:x} c30=0x{c30:x}"
        ));
    }
}

/// Diagnostic throttle for `own_load_m28_dispatch`: log the first HEAD entries, then every INTERVALth.
pub(crate) use er_telemetry::counters::OWN_LOAD_M28_DISPATCH_DIAG_CALLS;
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
        || diag_n.is_multiple_of(OWN_LOAD_M28_DISPATCH_DIAG_INTERVAL);
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
    if let (Some(rm), true) = (resmgr, block_count != OWN_LOAD_STREAM_FIELD_UNREAD)
        && block_count > 0
    {
        scan_chain_ok = true;
        let base_arr = rm + RESMGR_BLOCK_ARRAY_B3030_OFFSET;
        let n = block_count.min(OBSERVER_BLOCK_SCAN_CAP);
        let mut i: i64 = 0;
        while i < n {
            let slot = base_arr + (i as usize) * BLOCK_ENTRY_STRIDE;
            if let Some(block) = deref(slot)
                && let Some(inner) = deref(block + BLOCK_INNER_8_OFFSET)
                && let Some(area_u8) = unsafe { safe_read_usize(inner + BLOCK_AREA_C_OFFSET) }
                    .map(|v| (v as u32 & TARGET_AREA_FROM_COORD_MASK) as u8)
            {
                if area_u8 == target_area {
                    target_block_count += 1;
                    // `block` IS the matched player-area (m28, 0x1c) WorldBlockRes.
                    // Drive its FD4FileCap(s) to residency via the direct-enqueue lever.
                    // Double-gated: own_dispatch armed AND our OWN-LOAD continue fired.
                    if own_dispatch_enabled() && OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst) {
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
            i += 1;
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
    if frames.is_multiple_of(OWN_LOAD_STREAM_LOG_INTERVAL) {
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
