//! `CS::InGameStep::STEP_RequestWait` guard -- the one place a loaded world's session is ended.
//!
//! WHAT THIS FIXES. A System->Quit switch that loads a foreign save reaches the world: the fresh
//! deserialize mounts the picked slot (`own-load-feed: c30 0xa010000->0x1c000000 ... level=11`), the
//! world-res entry is created, 111 blocks populate, and the world streams for ~10 seconds. Then it is
//! torn back to the title map -- `WORLD LOST #2` in run br-20260905-194101-bd9d, which is the black
//! screen the user sees.
//!
//! WHY. From the named 1.16.2 decompile of `STEP_RequestWait` (0x140aecc10), the step dispatches on
//! `InGameStep+0xd8` (`requestCode`):
//!
//! ```text
//!   d8 == 0 -> fade + loadingScreenData.field_0x11 = 1, stay in this step
//!   d8 == 1 -> fade + FUN_14067a320(mode) + FUN_140aed270(this, 4)   <- ADVANCES out of RequestWait
//!   d8 == 2 -> field_0x6b0 = 1, field_0x11 = 1,
//!              if (CSMenuMan+0x798 != 0) return;                      <- NowLoading job alive: stay
//!              *(u32*)(this + 0xd8) = 0;                              <- ELSE END THE SESSION
//! ```
//!
//! and `STEP_GameStepWait` (bd `setstate-beginlogo-is-gamestepwait-b7c-b7d-not-menudata-5e-2026-09-04`)
//! then reads `d8 == 0` with `GameMan+0xb7c`/`+0xb7d` clear and does `SetMapId(0xff,0xff,0xff,0xff)` +
//! `SetState(2 BeginLogo)`. That is the whole black screen, and `STEP_RequestWait` is its ONLY trigger:
//! nothing else in the image stores 0 into `+0xd8`.
//!
//! A HEALTHY load never reaches the `d8 == 2` arm, because it passes through the step while d8 is still
//! 1 and the `d8 == 1` arm advances to step 4 -- after which `STEP_MoveMap_Update` raising d8 to 2 has
//! no reader. Measured: br-20260904-165518-e3be sits at `committed=6 ig_d8=1` for its whole session and
//! loses no world, even though it too shows `ig_d8=2 menu_job=0x0` samples later. Our switch is
//! different in one way that matters: it mounts the map BEFORE firing `continue_confirm`, so the MoveMap
//! request can already be complete when `RequestWait` first ticks -- d8 is 2 on entry, the `d8 == 1`
//! advance is skipped, and the session-end arm runs against a NowLoading job that is null.
//!
//! WHAT THE GUARD DOES. On entry with `d8 == 2` and a null NowLoading job while a genuinely real map is
//! mounted, it rewrites d8 to 1 and lets the ORIGINAL run. The game then takes its own healthy `d8 == 1`
//! branch -- the same fade, the same `FUN_14067a320`, the same `FUN_140aed270(this, 4)` advance a normal
//! load takes. Nothing here calls a game function, and nothing skips one; the only write is to the
//! dispatch value, and only to a value the native code sets itself one step earlier.
//!
//! WHY IT CANNOT WEDGE A WORLD THAT IS NOT COMING. Every correction spends one of a fixed budget
//! (`MAX_CORRECTIONS`) armed per switch. When the budget is gone the native store runs untouched and the
//! game returns to the title exactly as it does today. A guard that could suppress the teardown forever
//! would convert a black screen into a hang, which is worse; this one converts it into at most a few
//! frames of delay before the same outcome.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::super::{game_module_base, safe_read_i32, safe_read_usize};
use crate::constants::{
    CS_MENU_MAN_GLOBAL_RVA, CSMENUMAN_NOWLOADING_JOB_798_OFFSET, INGAMESTEP_REQUEST_CODE_D8_OFFSET,
    STEP_REQUEST_WAIT_RVA, game_man_ptr_or_null,
};
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use crate::telemetry::append_autoload_debug;

/// `requestCode` values `STEP_RequestWait` dispatches on. `ADVANCE` is the one whose arm leaves the
/// step (to step 4); `SESSION_END` is the one whose arm clears `+0xd8`.
const REQUEST_CODE_ADVANCE: i32 = 1;
const REQUEST_CODE_SESSION_END: i32 = 2;

/// The title/new-game default map id. `c30` equal to this means no real world is mounted, so a session
/// end is the game doing its job and the guard must not touch it.
const C30_M10_DEFAULT: i32 = 0xa01_0000;

/// How many session-ends one switch may convert. Small on purpose: the healthy path needs ONE (the
/// first RequestWait tick after `continue_confirm`), and a world that is genuinely not arriving must be
/// allowed to end rather than hang.
const MAX_CORRECTIONS: usize = 4;

/// Cap on entry logging, so a step that ticks every frame cannot flood the debug log.
const MAX_ENTRY_LOGS: usize = 24;

static HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static ORIG: AtomicUsize = AtomicUsize::new(0);
static CORRECTIONS_LEFT: AtomicUsize = AtomicUsize::new(0);
static ENTRY_LOGS: AtomicUsize = AtomicUsize::new(0);
static CORRECTIONS_MADE: AtomicUsize = AtomicUsize::new(0);

/// Arm the guard for ONE switch. Called at the `continue_confirm` commit, the instant after which the
/// incoming world's `RequestWait` can tick.
pub(crate) fn arm_request_wait_guard_for_switch() {
    CORRECTIONS_LEFT.store(MAX_CORRECTIONS, Ordering::SeqCst);
    ENTRY_LOGS.store(0, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "requestwait-guard: ARMED for this switch (budget={MAX_CORRECTIONS} session-end conversions); a d8==2 tick with a null NowLoading job while a real map is mounted will be rewritten to d8==1 so the game takes its own advance-to-step-4 branch"
    ));
}

/// `CSMenuMan+0x798` -- the NowLoading MenuJob. `STEP_RequestWait` returns early while it is non-null,
/// so a non-null read means the native code will not end the session this tick.
fn nowloading_job() -> usize {
    let Ok(base) = game_module_base() else {
        return 0;
    };
    unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            CS_MENU_MAN_GLOBAL_RVA,
            "CS_MENU_MAN_GLOBAL_RVA",
        ))
    }
    .filter(|&m| m > 0x10000)
    .and_then(|m| unsafe { safe_read_usize(m + CSMENUMAN_NOWLOADING_JOB_798_OFFSET) })
    .unwrap_or(0)
}

/// `GameMan+0xc30` -- the mounted map id, or the m10 default when no world is up.
fn mounted_map_id() -> i32 {
    let gm = game_man_ptr_or_null();
    if gm <= 0x10000 {
        return C30_M10_DEFAULT;
    }
    unsafe { safe_read_i32(gm + er_title_flow::GAME_MAN_SAVED_MAP_C30_OFFSET) }
        .unwrap_or(C30_M10_DEFAULT)
}

unsafe extern "system" fn step_request_wait_hook(in_game_step: usize) {
    let d8 =
        unsafe { safe_read_i32(in_game_step + INGAMESTEP_REQUEST_CODE_D8_OFFSET) }.unwrap_or(-1);
    if d8 == REQUEST_CODE_SESSION_END {
        let nowloading = nowloading_job();
        let c30 = mounted_map_id();
        let world_is_real = c30 != C30_M10_DEFAULT && c30 != 0 && c30 != -1;
        let logs = ENTRY_LOGS.fetch_add(1, Ordering::SeqCst);
        if logs < MAX_ENTRY_LOGS {
            append_autoload_debug(format_args!(
                "requestwait-guard: STEP_RequestWait tick d8=2 (the session-end arm) nowloading798=0x{nowloading:x} c30=0x{c30:x} world_is_real={world_is_real} budget={} -- the native code clears InGameStep+0xd8 here iff nowloading798 == 0",
                CORRECTIONS_LEFT.load(Ordering::SeqCst)
            ));
        }
        if nowloading == 0 && world_is_real {
            // `fetch_update` rather than a load/store pair: RequestWait runs on the game thread, but
            // the budget is the only thing standing between "converts a race" and "hangs forever", so
            // it is spent atomically.
            let spent = CORRECTIONS_LEFT
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1).filter(|_| left > 0)
                })
                .is_ok();
            if spent {
                unsafe {
                    *((in_game_step + INGAMESTEP_REQUEST_CODE_D8_OFFSET) as *mut i32) =
                        REQUEST_CODE_ADVANCE;
                }
                let made = CORRECTIONS_MADE.fetch_add(1, Ordering::SeqCst) + 1;
                append_autoload_debug(format_args!(
                    "requestwait-guard: CONVERTED session-end -> advance #{made}: rewrote InGameStep+0xd8 2->1 on a REAL world (c30=0x{c30:x}) whose NowLoading job is already gone, so STEP_RequestWait takes its own d8==1 branch (FUN_140aed270(this,4)) instead of storing 0 and handing STEP_GameStepWait the SetMapId(0xff,0xff,0xff,0xff) teardown. budget left={}",
                    CORRECTIONS_LEFT.load(Ordering::SeqCst)
                ));
            } else if logs < MAX_ENTRY_LOGS {
                append_autoload_debug(format_args!(
                    "requestwait-guard: budget exhausted -- letting the native session-end run (c30=0x{c30:x}); a world that has not arrived after {MAX_CORRECTIONS} conversions must be allowed to return to the title rather than hang"
                ));
            }
        }
    }
    let orig = ORIG.load(Ordering::SeqCst);
    if orig == 0 {
        return;
    }
    let orig: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(orig) };
    unsafe { orig(in_game_step) }
}

/// Install the detour. Idempotent, and harmless until `arm_request_wait_guard_for_switch` gives it a budget.
pub(crate) fn install_request_wait_guard() -> bool {
    if HOOK_INSTALLED.load(Ordering::SeqCst) != 0 {
        return true;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "requestwait-guard: MH_Initialize failed: {status:?}"
            ));
            return false;
        }
    }
    let Ok(addr) = er_game_base::mem::game_rva_for_hook(STEP_REQUEST_WAIT_RVA as u32) else {
        append_autoload_debug(format_args!(
            "requestwait-guard: failed to resolve STEP_RequestWait rva 0x{STEP_REQUEST_WAIT_RVA:x}"
        ));
        return false;
    };
    match unsafe { MhHook::new(addr as *mut c_void, step_request_wait_hook as *mut c_void) } {
        Ok(hook) => {
            ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "requestwait-guard: queue_enable failed: {status:?}"
                ));
                return false;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    crate::mh::leak_installed_hook(hook);
                    HOOK_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "requestwait-guard: hooked STEP_RequestWait at 0x{addr:x} (pass-through until a switch arms it)"
                    ));
                    true
                }
                status => {
                    append_autoload_debug(format_args!(
                        "requestwait-guard: MH_ApplyQueued failed: {status:?}"
                    ));
                    false
                }
            }
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "requestwait-guard: MhHook::new failed: {status:?}"
            ));
            false
        }
    }
}
