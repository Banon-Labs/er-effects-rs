// Sample the ending-request evaluator's inputs at its own entry, not after it has decided.
//
// `CS::MoveMapStep`'s per-frame advancer computes one boolean `cVar10` from nine terms, writes it
// to `menuData+0x5e`, and `case 0: if (cVar10 == 0) return;` -- so `cVar10` alone decides whether
// the MoveMap child leaves the resident `STEP_MoveMap(18)` and walks to its terminal. The child
// reaching its terminal is what lets `InGameStep` drain `+0xd8` through its session-end arm, and
// `STEP_GameStepWait` then does `SetMapId(0xff,0xff,0xff,0xff)`. That is the black screen.
//
// Why a third sampler. Both previous ones read the inputs from outside this function and both
// reported every term zero at a teardown that unquestionably happened:
//
// * `MMS-CLEANUP` samples on the child's Cleanup entry -- measured ~2 s downstream of the decision;
// * `CVAR10 RISE` samples on the game-task tick that notices the `menuData+0x5e` 0->1 edge. Closer,
//   and still downstream: it polls a value this function has already written. Run 2026-09-06
//   10:04:07, on a world that had loaded and was playable, it reported
//   `rt5d=0 warp=0 b7c=0 b7d=0 force=0 session_proto=6 dead_reset=-1` -- which is consistent with
//   "the terms were transient and had already cleared", and therefore proves nothing.
//
// A term that is set for one frame is invisible to any sampler that runs on a later frame. Entry
// to the evaluator is the only place the inputs are the ones the decision is made from.
//
// How the address was established, because the usual route refuses it.
// `scripts/map-rvas-1162-to-1170.py 0x140afa6d0` returns UNMAPPED (111 shape matches, none at the
// nearest anchor's delta). It was identified by call GRAPH: `L"EnableBot"` occurs twice in the 1.17
// image and only `0x142bfdfd0` has xrefs; of its two referents the 137-byte `0x140e7e580` is an
// exact size match for 1.16.2 `CS::CSEzSelectBot::IsBotEnabled` (137 B), and the single caller of
// that in the MoveMap region decompiles carrying `L"CSEzSelectBot.MoveMapStep"`.
// `scripts/verify-rva-map-1170.py` then confirmed the pair on bytes, independently of how it was
// found: Identical-whole, ratio 1.000 over 972 instructions, both-entries, `PDATA:0x11b0/0x11b0`.
//
// Read-only. The detour samples, calls the original unchanged, and writes nothing. It cannot
// change which branch the game takes; a guard that corrects `cVar10` would be a different thing
// and would need its own evidence, which is what this exists to produce.

// `include!`d into the crate root beside the other live-game-memory modules, so the constant
// tables (`constants_return_title.rs` et al), `append_autoload_debug` and `game_man_ptr_or_null`
// are already in scope and are not imported here.
#[allow(unused_imports)]
use crate::compat::*;

// Only what the flat include! namespace does not already carry. `c_void`, `AtomicUsize`,
// `Ordering` and the four `er_hook` items are imported by `product_autoload_gates.rs`, and a
// second `use` of the same name in the same module is E0252.
use core::sync::atomic::AtomicI32;

use er_game_base::mem::{safe_read_i32, safe_read_u8, safe_read_usize};
use er_game_base::rva::CS_MENU_MAN_GLOBAL_RVA;

/// A null game pointer, spelled the way the rest of the tree spells it.
const NULL: usize = usize::MIN;

/// Smallest address treated as a real heap/module pointer rather than a small integer.
const PLAUSIBLE_PTR_MIN: usize = 0x10000;

/// The `menuData+0x5e` value before this call. `-1` is the unread sentinel and never matches an edge.
static PREV_5E: AtomicI32 = AtomicI32::new(-1);

/// Cap on rise reports. The rise that matters is the first one; a per-frame log here would sit on
/// the game thread inside the MoveMap advancer, which is the worst place in the frame to do IO.
const MAX_RISE_LOGS: usize = 8;
static RISE_LOGS: AtomicUsize = AtomicUsize::new(0);

/// Cap on liveness lines. Enough to prove the detour runs and that `menuData` resolves from it,
/// without putting per-frame IO inside the MoveMap advancer.
const MAX_LIVENESS_LOGS: usize = 6;
static CALLS: AtomicUsize = AtomicUsize::new(0);

static HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static ORIG: AtomicUsize = AtomicUsize::new(0);

/// The eight measurable `cVar10` terms, read in one pass.
///
/// The ninth, `CSEzSelectBot::IsBotEnabled()`, is deliberately absent: its whole body reads the
/// `"EnableBot"` debug property with a `false` default, and a retail build ships no debug
/// properties, so it is a constant here and a call into it from a detour would be pure risk.
struct Cvar10Inputs {
    rt5d: i32,
    warp: i32,
    b7c: i32,
    b7d: i32,
    force: i32,
    session_proto: i32,
    dead_reset: i32,
    /// `CSEventMan` itself, because a `dead_reset` of -1 is ambiguous without it -- run 2026-09-06
    /// measured `eventman=0x0` and the whole term stayed unobserved behind that single sentinel.
    event_man: usize,
    md_5e: i32,
}

fn menu_data(base: usize) -> Option<usize> {
    unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            CS_MENU_MAN_GLOBAL_RVA,
            "CS_MENU_MAN_GLOBAL_RVA",
        ))
    }
    .filter(|&m| m > PLAUSIBLE_PTR_MIN)
    .and_then(|m| unsafe { safe_read_usize(m + CS_MENU_MAN_MENU_DATA_OFFSET) })
    .filter(|&d| d > PLAUSIBLE_PTR_MIN)
}

fn sample_inputs(base: usize) -> Cvar10Inputs {
    let gm = game_man_ptr_or_null();
    let byte_at = |ptr: usize, off: usize| -> i32 {
        if ptr <= PLAUSIBLE_PTR_MIN {
            return -1;
        }
        unsafe { safe_read_u8(ptr + off) }.map_or(-1, i32::from)
    };
    let md = menu_data(base);
    let session_proto = {
        let manager = er_game_base::mem::read_global_ptr(
            base,
            er_game_base::rva::CS_SESSION_MANAGER_GLOBAL_RVA,
            "CS_SESSION_MANAGER_GLOBAL_RVA",
        );
        if manager == NULL {
            -1
        } else {
            unsafe { safe_read_i32(manager + CS_SESSION_MANAGER_PROTOCOL_STATE_10_OFFSET) }
                .unwrap_or(-1)
        }
    };
    let event_man = er_game_base::mem::read_global_ptr(
        base,
        er_game_base::rva::CS_EVENT_MAN_GLOBAL_RVA,
        "CS_EVENT_MAN_GLOBAL_RVA",
    );
    let dead_reset = if event_man == NULL {
        -1
    } else {
        unsafe { safe_read_usize(event_man + CS_EVENT_MAN_DEAD_RESET_10_OFFSET) }
            .filter(|&s| s > PLAUSIBLE_PTR_MIN)
            .and_then(|s| unsafe { safe_read_i32(s + CS_EVENT_DEAD_RESET_STATE_8_OFFSET) })
            .unwrap_or(-1)
    };
    Cvar10Inputs {
        rt5d: md.map_or(-1, |d| {
            byte_at(d, CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET)
        }),
        warp: byte_at(gm, GAME_MAN_WARP_REQUESTED_10_OFFSET),
        b7c: byte_at(gm, GAME_MAN_ENDING_FLAG_B7C_OFFSET),
        b7d: byte_at(gm, GAME_MAN_ENDING_FLAG_B7D_OFFSET),
        force: unsafe {
            safe_read_u8(er_game_base::mem::game_data_addr(
                base,
                ENDING_REQUEST_FORCE_FLAG_3D856A0_RVA,
                "ENDING_REQUEST_FORCE_FLAG_3D856A0_RVA",
            ))
        }
        .map_or(-1, i32::from),
        session_proto,
        dead_reset,
        event_man,
        md_5e: md.map_or(-1, |d| byte_at(d, CS_MENU_DATA_ENDING_FLAG_5E_OFFSET)),
    }
}

unsafe extern "system" fn movemap_advancer_hook(this: usize) {
    let base = er_game_base::mem::game_module_base().unwrap_or(NULL);
    let entry = if base == NULL {
        None
    } else {
        Some(sample_inputs(base))
    };

    let orig = ORIG.load(Ordering::SeqCst);
    if orig != NULL {
        let orig: unsafe extern "system" fn(usize) = unsafe { core::mem::transmute(orig) };
        unsafe { orig(this) };
    }

    // The edge is read after the original, so the pair (entry sample, resulting cVar10) belongs to
    // one call. That is the whole point: a sampler on any other schedule can only report the state
    // some number of frames after the decision.
    let Some(entry) = entry else { return };
    let after = menu_data(base)
        .and_then(|d| unsafe { safe_read_u8(d + CS_MENU_DATA_ENDING_FLAG_5E_OFFSET) })
        .map_or(-1, i32::from);
    let prev = PREV_5E.swap(after, Ordering::SeqCst);
    // LIVENESS, because run 2026-09-06 10:36 produced `CVAR10 RISE` from the game-task sampler and
    // not one line from this detour -- which has three possible causes and no way to tell them
    // apart from silence: the detour never fires, `menuData` is unreadable from here so `after` is
    // always -1, or the 0->1 transition never lands on a call this hook sees. A handful of early
    // calls printing their own (prev, after) answers all three at once.
    let calls = CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    if calls <= MAX_LIVENESS_LOGS {
        append_autoload_debug(format_args!(
            "cvar10-entry: call #{calls} prev_5e={prev} after_5e={after} md_5e_on_entry={} (liveness -- proves the detour fires and whether menuData is readable here)",
            entry.md_5e
        ));
    }
    // `prev == -1` is an edge here, and treating it as "not an edge" cost a run. Measured
    // 2026-09-06 10:37:50: call #1 entered with `menuData+0x5e == 0` and left with it 1 -- the
    // evaluator's first invocation is the one that raises the ending request, so the only 0->1
    // transition this detour will ever see has the unread sentinel as its predecessor.
    if !(after == 1 && (prev == 0 || prev == -1)) {
        return;
    }
    let n = RISE_LOGS.fetch_add(1, Ordering::SeqCst) + 1;
    if n > MAX_RISE_LOGS {
        return;
    }
    append_autoload_debug(format_args!(
        "cvar10-entry #{n}: menuData+0x5e 0->1 on THIS advancer call -- inputs sampled at its ENTRY, \
         not after: rt5d={} warp={} b7c={} b7d={} force={} session_proto={}(ending=4) \
         dead_reset={}(ending=2, eventman=0x{:x}) md_5e_on_entry={}. \
         Any term reading 1 here is the ending request; all zero means the evaluator's ninth term \
         (CSEzSelectBot::IsBotEnabled, the EnableBot debug property) or a term this list is missing",
        entry.rt5d,
        entry.warp,
        entry.b7c,
        entry.b7d,
        entry.force,
        entry.session_proto,
        entry.dead_reset,
        entry.event_man,
        entry.md_5e,
    ));
}

/// Install the read-only detour. Idempotent.
pub fn install_movemap_advancer_probe() -> bool {
    if HOOK_INSTALLED.load(Ordering::SeqCst) != 0 {
        return true;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "cvar10-entry: MH_Initialize failed: {status:?}"
            ));
            return false;
        }
    }
    let Ok(addr) = er_game_base::mem::game_rva_for_hook(MOVEMAP_ADVANCER_RVA as u32) else {
        append_autoload_debug(format_args!(
            "cvar10-entry: REFUSED -- no verified 1.17 mapping for the MoveMap advancer rva 0x{MOVEMAP_ADVANCER_RVA:x}; not hooking"
        ));
        return false;
    };
    match unsafe { MhHook::new(addr as *mut c_void, movemap_advancer_hook as *mut c_void) } {
        Ok(hook) => {
            ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "cvar10-entry: queue_enable failed: {status:?}"
                ));
                return false;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    // MinHook has owned the detour since `MH_ApplyQueued`, and `MhHook` is
                    // three raw pointers with no `Drop`, so letting the handle fall out of
                    // scope uninstalls nothing. `drop(hook)` says that but is
                    // `clippy::drop_non_drop` (it broke CI on PR #406); `mem::forget` would be
                    // `forget_non_drop`. The shim states the intent once in
                    // `crate::mh::leak_installed_hook`, which does not cross this seam.
                    let _installed_and_owned_by_minhook = hook;
                    HOOK_INSTALLED.store(1, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        // `addr` is the pre-translation 1.16.2 address: `game_rva_for_hook` returns
                        // base+rva by design and `MhHook::new` owns the single 1.16.2 -> 1.17
                        // resolve. Printing `addr` alone reads as "hooked the stale address" and
                        // cost a teardown on 2026-09-06 before the `HOOK TRANSLATED` line was
                        // checked. Say which number this is.
                        "cvar10-entry: hooked the MoveMap ending-request evaluator, requested 1.16.2 0x{addr:x} -- MhHook::new resolves it for the running build, see the HOOK TRANSLATED line above for the address actually detoured (read-only; logs the inputs on the call that raises menuData+0x5e)"
                    ));
                    true
                }
                status => {
                    append_autoload_debug(format_args!(
                        "cvar10-entry: MH_ApplyQueued failed: {status:?}"
                    ));
                    false
                }
            }
        }
        Err(status) => {
            append_autoload_debug(format_args!("cvar10-entry: MhHook::new failed: {status:?}"));
            false
        }
    }
}
