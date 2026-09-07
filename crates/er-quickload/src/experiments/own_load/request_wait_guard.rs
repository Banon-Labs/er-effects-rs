//! `CS::InGameStep::STEP_RequestWait` guard -- the one place a loaded world's session is ended.
//!
//! What this fixes. A System->Quit switch that loads a foreign save reaches the world: the fresh
//! deserialize mounts the picked slot (`own-load-feed: c30 0xa010000->0x1c000000 ... level=11`), the
//! world-res entry is created, 111 blocks populate, and the world streams for ~10 seconds. Then it is
//! torn back to the title map -- `WORLD LOST #2` in run br-20260905-194101-bd9d, which is the black
//! screen the user sees.
//!
//! Why. From the named 1.16.2 decompile of `STEP_RequestWait` (0x140aecc10), the step dispatches on
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
//! `SetState(2 BeginLogo)`. That is the whole black screen, and `STEP_RequestWait` is its only trigger:
//! nothing else in the image stores 0 into `+0xd8`.
//!
//! A healthy load never reaches the `d8 == 2` arm, because it passes through the step while d8 is still
//! 1 and the `d8 == 1` arm advances to step 4 -- after which `STEP_MoveMap_Update` raising d8 to 2 has
//! no reader. Measured: br-20260904-165518-e3be sits at `committed=6 ig_d8=1` for its whole session and
//! loses no world, even though it too shows `ig_d8=2 menu_job=0x0` samples later. Our switch is
//! different in one way that matters: it mounts the map before firing `continue_confirm`, so the MoveMap
//! request can already be complete when `RequestWait` first ticks -- d8 is 2 on entry, the `d8 == 1`
//! advance is skipped, and the session-end arm runs against a NowLoading job that is null.
//!
//! What the guard does. On entry with `d8 == 2` and a null NowLoading job while a genuinely real map is
//! mounted, it rewrites d8 to 1 and lets the original run. The game then takes its own healthy `d8 == 1`
//! branch -- the same fade, the same `FUN_14067a320`, the same `FUN_140aed270(this, 4)` advance a normal
//! load takes. Nothing here calls a game function, and nothing skips one; the only write is to the
//! dispatch value, and only to a value the native code sets itself one step earlier.
//!
//! Why it cannot wedge a world that is not coming. Every correction spends one of a fixed budget
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
const REQUEST_CODE_SESSION_END: i32 = 2;

/// The title/new-game default map id. `c30` equal to this means no real world is mounted, so a session
/// end is the game doing its job and the guard must not touch it.
const C30_M10_DEFAULT: i32 = 0xa01_0000;

/// Cap on entry logging, so a step that ticks every frame cannot flood the debug log.
const MAX_ENTRY_LOGS: usize = 24;

static HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static ORIG: AtomicUsize = AtomicUsize::new(0);
static ENTRY_LOGS: AtomicUsize = AtomicUsize::new(0);
static TICK_LOGS: AtomicUsize = AtomicUsize::new(0);
static CORRECTIONS_MADE: AtomicUsize = AtomicUsize::new(0);
static MOVEMAP_INIT_REPORTS: AtomicUsize = AtomicUsize::new(0);

/// `GameMan.moveMapStepBlockId` (Ghidra offset 20). `STEP_MoveMap_Init` copies it into the
/// MoveMapStep's `mapId` and then writes 0xffffffff back over it.
const GAME_MAN_MOVEMAP_STEP_BLOCK_ID_14_OFFSET: usize = 0x14;

/// Cap on init reports. A map move happens on every warp, grace rest and death, and this is a
/// diagnostic, not a per-frame oracle.
const MAX_INIT_REPORTS: usize = 32;

/// Reset the per-switch log budgets. Called at the `continue_confirm` commit, the instant after which
/// the incoming world's `RequestWait` can tick, so each switch gets its own window of ticks recorded
/// rather than being silenced by the previous one.
pub(crate) fn arm_request_wait_guard_for_switch() {
    ENTRY_LOGS.store(0, Ordering::SeqCst);
    TICK_LOGS.store(0, Ordering::SeqCst);
    CORRECTIONS_MADE.store(0, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "requestwait-observer: armed for this switch -- every STEP_RequestWait tick is logged with its d8, and a session-end is REPORTED, never converted"
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
    // Log every tick, not only the session-end arm. The decompile says the `d8 == 1` arm is what
    // advances out of this step (`FUN_140aed270(this, 4)`), and a load that keeps its world is
    // believed to leave through that arm before `STEP_MoveMap_Update` ever raises d8 to 2 -- the
    // boot-load run br-20260904-165518-e3be sat at `ig_d8=1` for its whole session and lost no world.
    // If our switch never ticks this step at d8 == 1, that is the divergence, and it is invisible in
    // a log that only records d8 == 2.
    let ticks = TICK_LOGS.fetch_add(1, Ordering::SeqCst);
    if ticks < MAX_ENTRY_LOGS {
        append_autoload_debug(format_args!(
            "requestwait-tick #{}: d8={d8} ({}) -- d8==1 advances out of this step, d8==2 ends the session once the NowLoading job is gone",
            ticks + 1,
            match d8 {
                0 => "fade, stay in step",
                1 => "ADVANCE to step 4",
                2 => "session-end arm",
                _ => "other",
            }
        ));
    }
    if d8 == REQUEST_CODE_SESSION_END {
        let nowloading = nowloading_job();
        let c30 = mounted_map_id();
        let world_is_real = c30 != C30_M10_DEFAULT && c30 != 0 && c30 != -1;
        let logs = ENTRY_LOGS.fetch_add(1, Ordering::SeqCst);
        if logs < MAX_ENTRY_LOGS {
            append_autoload_debug(format_args!(
                "requestwait-observer: STEP_RequestWait tick d8=2 (the session-end arm) nowloading798=0x{nowloading:x} c30=0x{c30:x} world_is_real={world_is_real} -- the native code clears InGameStep+0xd8 here iff nowloading798 == 0"
            ));
        }
        if nowloading == 0 && world_is_real {
            // Observe only. This used to rewrite d8 2 -> 1 so the native code would take its
            // `d8 == 1` arm instead of ending the session. It did stop the black screen, and it was
            // still wrong: that arm calls `FUN_140aed270(this, 4)` and advances the step, which after
            // the MoveMap has already completed starts a second load. Measured on run
            // br-20260905-211954-d2a7 -- MoveMap init #2 (the switch's real load) got a correct
            // destination block 0x1c000000, and a third init nine seconds later, caused by this
            // conversion, found `GameMan+0x14` already consumed and cleared to 0xffffffff, so
            // STEP_WorldResWait waited forever on block ff/ff/ff/ff. Trading a black screen for a
            // permanent stall is not a fix, and a guard that re-drives a load the game had already
            // finished has no business in the product.
            //
            // The defect is upstream of both arms: this step should not be reached with d8 == 2 at
            // all. A load that keeps its world leaves via the d8 == 1 arm first (br-20260904-165518-e3be
            // sits at ig_d8 = 1 for its whole session), so the question the tick log above exists to
            // answer is whether our switch ever ticks this step while d8 is still 1.
            let n = CORRECTIONS_MADE.fetch_add(1, Ordering::SeqCst) + 1;
            if n <= MAX_ENTRY_LOGS {
                append_autoload_debug(format_args!(
                    "requestwait-guard: OBSERVED session-end #{n} -- the native code is about to store 0 into InGameStep+0xd8 on a REAL world (c30=0x{c30:x}) whose NowLoading job is gone, and STEP_GameStepWait will turn that into SetMapId(0xff,0xff,0xff,0xff). NOT intervening: rewriting d8 here re-drives the load"
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

/// Log `GameMan+0x14` (moveMapStepBlockId) at `STEP_MoveMap_Init` entry -- the value that init is
/// about to copy into the MoveMapStep's `mapId` (+0xdc) and then clear.
///
/// Read-only on purpose. The slot deserialize `FUN_14067b290` (1.16.2 0x14067b290, the function
/// `own_load_feed_deserialize` already drives) ends with `SetMoveMapStepBlockId(GameMan+0xc30)` and
/// `warpRequested = true`: the native flow sets this field itself, from the map id the save's own
/// bytes just wrote into `+0xc30`. So the correct question is not "who supplies the block" but
/// "why is it missing at the init that matters", and a second write from us would hide the answer.
/// Measured live on the stalled run br-20260905-201903-c409: MoveMapStep+0xdc was 0xffffffff, so
/// `STEP_WorldResWait` waited on block ff/ff/ff/ff forever.
pub(crate) fn report_destination_block_at_init() {
    let hits = MOVEMAP_INIT_REPORTS.fetch_add(1, Ordering::SeqCst) + 1;
    if hits > MAX_INIT_REPORTS {
        return;
    }
    let gm = game_man_ptr_or_null();
    let block = if gm > 0x10000 {
        unsafe { safe_read_i32(gm + GAME_MAN_MOVEMAP_STEP_BLOCK_ID_14_OFFSET) }.unwrap_or(-1)
    } else {
        -1
    };
    let saved = if gm > 0x10000 {
        unsafe { safe_read_i32(gm + er_title_flow::GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(-1)
    } else {
        -1
    };
    append_autoload_debug(format_args!(
        "movemap-init-block #{hits}: GameMan+0x14 (moveMapStepBlockId) = 0x{block:x}, GameMan+0xc30 (the save's map) = 0x{saved:x} -- this init copies +0x14 into MoveMapStep+0xdc and then clears it. 0xffffffff here means the load has no destination and STEP_WorldResWait will wait forever"
    ));
}
