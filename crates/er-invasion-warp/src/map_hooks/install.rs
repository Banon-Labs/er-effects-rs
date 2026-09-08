//! Installing the three world-map detours.
//!
//! Split out of `map_hooks` on 2026-08-30, when that file stood 29 lines under the 3200-line
//! fail threshold in `scripts/check-rust-file-sizes.py`. The seam is a real one: everything here
//! runs once, from the game task thread, and decides only where a hook goes and whether it may go
//! there. What the hooks then do every frame -- the ctor handler, the row filter, the injection --
//! stays in the parent, which is what those three actually have in common with each other.
//!
//! The one invariant this file exists to HOLD: the three installs are independent. They used to
//! be a chain, and a single refused seam silently disarmed two unrelated features -- including the
//! softlock fix. See [`install_map_observers`], which spells out why, and do not re-couple them
//! when moving code past this point.

use super::*;

/// Whether the ctor hook is installed.
static CTOR_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);

/// Install the world-map observation hooks. Returns how many bound.
///
/// Every failure is logged and stepped over: losing an observer costs this run its evidence and
/// nothing else. Nothing here can disarm the already-proven warp.
///
/// # Safety
///
/// Call once, from the game task thread after the runtime is up.
#[cfg(windows)]
pub unsafe fn install_map_observers() -> usize {
    if CTOR_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return 0;
    }
    // Three independent hooks, installed independently.
    //
    // This used to be a chain: the ViewModel ctor was hooked first and the other two were
    // installed only from its success arm. So one refused seam disarmed two unrelated features
    // that had nothing to do with it, and did so silently -- the log carried the ctor's refusal
    // and not a word about the two installs that never happened.
    //
    // The ctor seam is not expected to refuse on 1.17 as things stand: its row is in the map
    // (0x8855b0 -> 0x8865a0) and its recorded 12-byte prologue was byte-checked against
    // `eldenring-deobf-1.17.bin` at the destination and matches, as do the other two. That is
    // precisely why the coupling has to go now rather than after it bites: a chain that happens
    // to work is one map regeneration, one recompiled prologue or one MinHook error away from
    // taking the softlock fix down with it, and the failure would be silent.
    //
    // The confirm interceptor is the one that makes the chaining actively harmful. It is the
    // SOFTLOCK FIX: it swallows a confirm carrying an id in our private 0x7F000000 band, which
    // the engine cannot resolve and which hangs the loading screen if it reaches
    // `CallLua_Warp`. Its handler is gated on that id band alone, so with no pins injected it is
    // a pure no-op -- there is no state it needs from the ctor hook and no cost to arming it.
    // Losing it because a different seam moved trades a missing feature for a frozen game.
    //
    // See bd `one-refused-hook-must-not-abort-the-installer-2026-08-30`.
    let mut bound = unsafe { install_viewmodel_ctor_hook() };
    bound += unsafe { install_row_filter_observer() };
    bound += unsafe { install_confirm_interceptor() };
    bound
}

/// Install the ViewModel-ctor hook -- the seam synthetic rows are injected from.
///
/// Losing it costs the map pins and nothing else: the confirm interceptor and the row-filter
/// observer are installed by the caller regardless.
///
/// # Safety
/// Game task thread.
#[cfg(windows)]
unsafe fn install_viewmodel_ctor_hook() -> usize {
    let address = match unsafe { verify_seam(&WORLDMAP_VIEWMODEL_CTOR) } {
        Ok(address) => address,
        Err(error) => {
            crate::standalone_log(format_args!(
                "map-hooks: REFUSED {} -- {error}; no pins will be injected. The other two map \
                 hooks are installed independently and are unaffected",
                WORLDMAP_VIEWMODEL_CTOR.name
            ));
            return 0;
        }
    };
    match unsafe {
        er_hook::register_union_hook(
            address,
            worldmap_viewmodel_ctor_hook as er_hook::UnionFn,
            &ORIG_WORLDMAP_VIEWMODEL_CTOR,
        )
    } {
        Ok(()) => {
            crate::standalone_log(format_args!(
                "map-hooks: ARMED {} @0x{address:x} (verified prologue) -- pins will be injected",
                WORLDMAP_VIEWMODEL_CTOR.name
            ));
            1
        }
        Err(status) => {
            crate::standalone_log(format_args!(
                "map-hooks: FAILED {} @0x{address:x} -- the address resolved and its prologue \
                 matched, but union registration returned {status:?}. That is MinHook refusing a \
                 verified address, not a moved function; the map surface stays absent and the \
                 F7/F8/F9 warp is unaffected",
                WORLDMAP_VIEWMODEL_CTOR.name
            ));
            0
        }
    }
}

/// Install the row-filter observer. Failure costs the visibility oracle and nothing else.
///
/// # Safety
/// Game task thread.
#[cfg(windows)]
unsafe fn install_row_filter_observer() -> usize {
    let seam = crate::map_seams::WORLDMAP_ROW_FILTER;
    let address = match unsafe { verify_seam(&seam) } {
        Ok(address) => address,
        Err(error) => {
            crate::standalone_log(format_args!(
                "map-hooks: REFUSED {} -- {error}; this run loses the visibility oracle only. \
                 Pins and pin selection are unaffected",
                seam.name
            ));
            return 0;
        }
    };
    match unsafe {
        er_hook::register_union_hook(
            address,
            worldmap_row_filter_hook as er_hook::UnionFn,
            &ORIG_ROW_FILTER,
        )
    } {
        Ok(()) => {
            crate::standalone_log(format_args!(
                "map-hooks: ARMED {} @0x{address:x} -- this is the visibility oracle",
                seam.name
            ));
            1
        }
        Err(status) => {
            crate::standalone_log(format_args!(
                "map-hooks: FAILED {} @0x{address:x} -- the address resolved and its prologue \
                 matched, but union registration returned {status:?}. Pins may still be fine, \
                 but this run cannot say whether they pass the filter",
                seam.name
            ));
            0
        }
    }
}

/// Install the confirm interceptor. Without it, selecting an injected pin softlocks, so a
/// failure here is logged loudly -- the pins are already in the list by then.
///
/// # Safety
/// Game task thread.
#[cfg(windows)]
unsafe fn install_confirm_interceptor() -> usize {
    let seam = crate::map_seams::WARP_JOB_ASSEMBLER;
    let address = match unsafe { verify_seam(&seam) } {
        Ok(address) => address,
        Err(error) => {
            crate::standalone_log(format_args!(
                "map-hooks: REFUSED {} -- {error}. WITHOUT THIS HOOK, SELECTING AN INJECTED PIN \
                 SOFTLOCKS",
                seam.name
            ));
            return 0;
        }
    };
    match unsafe {
        er_hook::register_union_hook(
            address,
            crate::map_confirm::warp_job_assembler_hook as er_hook::UnionFn,
            &crate::map_confirm::ORIG_WARP_JOB_ASSEMBLER,
        )
    } {
        Ok(()) => {
            crate::standalone_log(format_args!(
                "map-hooks: ARMED {} @0x{address:x} -- selecting an invasion pin is now answered \
                 by us instead of handing a synthetic id to Lua_Warp",
                seam.name
            ));
            1
        }
        Err(status) => {
            crate::standalone_log(format_args!(
                "map-hooks: FAILED {} @0x{address:x} -- the address resolved and its prologue \
                 matched, but union registration returned {status:?}. SELECTING AN INJECTED PIN \
                 WILL SOFTLOCK",
                seam.name
            ));
            0
        }
    }
}
