//! Keeping the profile-renderer table safe to walk, so `05_010_ProfileSelect` can be opened.
//!
//! The window this crate's opener submits renders one character model per save slot, and the
//! native refresh that draws them ([`er_loading_portrait_core::PROFILE_RENDERER_REFRESH_RVA`],
//! 1.17.1 `0x1409ab820`) walks all ten entries of the renderer table and dereferences
//! `table[slot] + 0x754` for every slot whose profile record exists -- with no null check. That is
//! the `er-effects-rs-j3r` access violation.
//!
//! The table is set up by exactly one native site, the `TitleTopDialog` constructor, so an in-world
//! ProfileSelect opened by this crate runs the refresh against whatever the last teardown left
//! behind. Measured 2026-09-11 on a product-less profile: pressing Load Character took
//! `0xc0000005` at `eldenring.exe+0x9ab874` reading `0x754`, with `rdi = *(0x143d71940) = 0`.
//!
//! # What this does about it
//!
//! [`profile_table_guard_body`] is the decision the product has run for months, moved here so a
//! host without the loading-cover pipeline can run it too. On entry to the native refresh it scans
//! all ten slots and then does one of three things:
//!
//! * every slot valid -- latch [`PROFILE_TABLE_WAS_POPULATED`] (proof the engine and `ResMan` are
//!   up) and let the refresh run;
//! * every slot empty, and the engine has been seen up -- re-run the native table setup
//!   [`er_loading_portrait_core::PROFILE_TABLE_BUILDER_RVA`] and rescan. The latch matters: calling
//!   the builder before the engine is ready faults inside the builder itself, so an empty table at
//!   boot is left alone;
//! * any slot still null afterwards -- skip the refresh entirely for this call, which is the only
//!   way to stop it dereferencing the null it is about to read.
//!
//! # Two installers, deliberately
//!
//! The product installs this on a private `MhHook` and has done since before the hook union
//! existed. This crate installs it through [`er_hook::register_union_hook`], which its own
//! `Cargo.toml` requires of every detour it owns. The two never share a process --
//! `scripts/me3-dll-conflicts.toml` records the pair as a duplicate owner -- so the product's
//! installer is left exactly as it is rather than converted, and the body below is the single copy
//! of the decision both run.

use core::sync::atomic::{AtomicUsize, Ordering};

use er_game_base::mem::{game_module_base, game_rva_for_hook, safe_read_usize};
use er_game_base::stack::trace_first_game_caller_rva;
use er_loading_portrait_core::{
    PROFILE_RENDERER_REFRESH_RVA, PROFILE_SELECT_TABLE_DIAG_LAST, PROFILE_SELECT_TABLE_DIAG_ORIG,
    PROFILE_SELECT_TABLE_GUARD_SKIP_COUNT, PROFILE_SELECT_TABLE_GUARD_SKIP_LAST,
    PROFILE_SELECT_TABLE_REPAIR_COUNT, PROFILE_TABLE_ALL_SLOTS_MASK, PROFILE_TABLE_BUILDER_RVA,
    PROFILE_TABLE_WAS_POPULATED, TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
    TITLE_PROFILE_SLOT_COUNT, portrait_renderer_table_entry,
};
use er_title_flow::TITLE_OWNER_SCAN_START_ADDRESS;

use crate::host::{append_autoload_debug, append_crash_log};

/// Slot pointers read out of the renderer table, plus the masks describing them.
struct TableScan {
    ptrs: [usize; TITLE_PROFILE_SLOT_COUNT],
    valid_mask: u32,
    null_mask: u32,
}

/// Read all ten renderer slots. A slot counts as valid only when its first word is the renderer
/// vtable -- a non-null pointer to something else is not a renderer and would fault the same way.
///
/// # Safety
///
/// Reads through the fault-safe readers; `base` is the game module base.
unsafe fn scan_table(base: usize) -> TableScan {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let want_vtable = er_game_base::mem::game_data_addr(
        base,
        TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
        "TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA",
    );
    let mut ptrs = [0usize; TITLE_PROFILE_SLOT_COUNT];
    let mut valid_mask = 0u32;
    let mut null_mask = 0u32;
    for (slot, entry) in ptrs.iter_mut().enumerate() {
        let renderer = unsafe { safe_read_usize(portrait_renderer_table_entry(base, slot as i32)) }
            .unwrap_or(0);
        *entry = renderer;
        let valid = renderer != 0
            && renderer != null
            && unsafe { safe_read_usize(renderer) }.unwrap_or(0) == want_vtable;
        if valid {
            valid_mask |= 1 << slot;
        } else {
            null_mask |= 1 << slot;
        }
    }
    TableScan {
        ptrs,
        valid_mask,
        null_mask,
    }
}

/// Whether the native refresh may run. `false` means skip it: a slot it is about to read is null.
///
/// Split from the detour so both installers share one decision and neither can drift from it.
///
/// # Safety
///
/// Called at the entry of the native refresh, on the menu thread.
pub unsafe fn profile_table_guard_body() -> bool {
    let Ok(base) = game_module_base() else {
        return true;
    };
    let scan = unsafe { scan_table(base) };
    let TableScan {
        ptrs,
        valid_mask,
        mut null_mask,
    } = scan;
    // Degraded is any null at all, including all-null. A healthy table is all ten valid, because
    // the native setup allocates all ten unconditionally.
    let degraded = null_mask != 0;
    let caller_rva = trace_first_game_caller_rva();
    let key = ((caller_rva & 0xffffff) << 20) | ((valid_mask as usize) << 10) | null_mask as usize;
    if degraded && PROFILE_SELECT_TABLE_DIAG_LAST.swap(key, Ordering::SeqCst) != key {
        append_crash_log(format_args!(
            "PROFILESELECT-TABLE-DIAG: degraded profile-renderer table before native builder (er-effects-rs-j3r) caller_rva=0x{caller_rva:x} valid_mask=0x{valid_mask:x} null_mask=0x{null_mask:x} entries=[0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x}]",
            ptrs[0],
            ptrs[1],
            ptrs[2],
            ptrs[3],
            ptrs[4],
            ptrs[5],
            ptrs[6],
            ptrs[7],
            ptrs[8],
            ptrs[9]
        ));
    } else if !degraded {
        PROFILE_SELECT_TABLE_DIAG_LAST.store(0, Ordering::SeqCst);
        // A fully valid table at refresh entry is the proof that the engine built renderers
        // successfully, which is what makes the repair below safe to attempt later.
        PROFILE_TABLE_WAS_POPULATED.store(1, Ordering::SeqCst);
    }
    if null_mask == PROFILE_TABLE_ALL_SLOTS_MASK
        && PROFILE_TABLE_WAS_POPULATED.load(Ordering::SeqCst) != 0
        && let Some(build_addr) = crate::scaleform_proxy::gated_game_fn(
            PROFILE_TABLE_BUILDER_RVA,
            "PROFILE_TABLE_BUILDER_RVA",
        )
    {
        // Safety: the native no-argument table setup, resolved from a pinned rva on a recognised
        // build. It tears down the existing ten (a no-op on an already-null table) and constructs
        // ten fresh renderers into the title table.
        let build: unsafe extern "system" fn() = unsafe { core::mem::transmute(build_addr) };
        unsafe { build() };
        let n = PROFILE_SELECT_TABLE_REPAIR_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        let rescan = unsafe { scan_table(base) };
        null_mask = rescan.null_mask;
        let revalid_mask = rescan.valid_mask;
        append_crash_log(format_args!(
            "PROFILESELECT-TABLE-REPAIR #{n}: fully-empty renderer table at native builder entry -> re-ran native table setup 0x{build_addr:x}; post-repair valid_mask=0x{revalid_mask:x} null_mask=0x{null_mask:x} (er-effects-rs-j3r)"
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
}

/// Make the profile-renderer table safe to open `05_010_ProfileSelect` against, building it if it
/// is empty. Returns whether the table is now fit to walk.
///
/// The opener needs this and the detour above cannot provide it: the detour only runs once the
/// native refresh is entered, and the refresh is only entered once the window has been submitted --
/// which is the thing the opener is deciding whether to do. Asking the host to build one does not
/// close the loop either, because the host entry is the loading-cover pipeline and a shell's
/// neutral default does nothing (measured 2026-09-11: four presses, each
/// `build_requested=false`, no picker).
///
/// # The readiness gate, and why the product's latch is not enough here
///
/// Calling the native setup before the engine and `ResMan` are up faults inside the builder --
/// observed at the title on 2026-06-29. The product gates on [`PROFILE_TABLE_WAS_POPULATED`], a
/// latch set when a fully valid table has been seen, which is a proxy for "the engine is up"
/// borrowed from the title menu having already built one. A product-less in-world load may never
/// have observed that, so the proxy answers no while the engine is plainly running.
///
/// The player being in the world is the stronger and more direct statement of the same fact: a
/// local `PlayerIns` exists only after the world has streamed, which is strictly later than the
/// engine coming up. Either signal opens the gate; neither alone would cover both callers.
///
/// # Safety
///
/// Menu thread, with the game module base.
pub unsafe fn ensure_profile_table_ready(base: usize) -> bool {
    let scan = unsafe { scan_table(base) };
    if scan.null_mask == 0 {
        return true;
    }
    if scan.null_mask != PROFILE_TABLE_ALL_SLOTS_MASK {
        // Partly populated. The builder tears the existing ten down before constructing fresh ones,
        // so running it here would destroy live renderers the menu is using. The refresh detour
        // handles this case by skipping instead.
        append_autoload_debug(format_args!(
            "profileselect-table-guard: renderer table is partly populated (null_mask=0x{:x}); not rebuilding, because the native setup tears down the slots that are still live",
            scan.null_mask
        ));
        return false;
    }
    let engine_seen_up = PROFILE_TABLE_WAS_POPULATED.load(Ordering::SeqCst) != 0;
    // Safety: the binding answers `Err` rather than faulting when no local player exists.
    let player_in_world = unsafe { eldenring::cs::PlayerIns::local_player_mut() }.is_ok();
    if !engine_seen_up && !player_in_world {
        append_autoload_debug(format_args!(
            "profileselect-table-guard: renderer table is empty and nothing says the engine is up (table_was_populated=false, player_in_world=false); not calling the native setup, which faults when it runs too early"
        ));
        return false;
    }
    let Some(build_addr) = crate::scaleform_proxy::gated_game_fn(
        PROFILE_TABLE_BUILDER_RVA,
        "PROFILE_TABLE_BUILDER_RVA",
    ) else {
        return false;
    };
    // Safety: the native no-argument table setup, resolved from a pinned rva on a recognised build.
    let build: unsafe extern "system" fn() = unsafe { core::mem::transmute(build_addr) };
    unsafe { build() };
    let n = PROFILE_SELECT_TABLE_REPAIR_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    let after = unsafe { scan_table(base) };
    append_autoload_debug(format_args!(
        "profileselect-table-repair #{n}: built the empty 10-slot renderer table via native setup 0x{build_addr:x} before opening ProfileSelect (engine_seen_up={engine_seen_up} player_in_world={player_in_world}); post-build valid_mask=0x{:x} null_mask=0x{:x}",
        after.valid_mask, after.null_mask
    ));
    if after.null_mask == 0 {
        PROFILE_TABLE_WAS_POPULATED.store(1, Ordering::SeqCst);
        return true;
    }
    false
}

/// The detour this crate installs. Same body as the product's, reached through the union.
///
/// # Safety
///
/// Installed by `er-hook`; the game calls it at the entry of the native refresh.
unsafe extern "system" fn profile_table_guard_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    // A panicking guard must not change what the game does, so a panic chains the original -- the
    // same fail-open the product's copy takes.
    let chain = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        profile_table_guard_body()
    }))
    .unwrap_or(true);
    if !chain {
        return 0;
    }
    let orig = PROFILE_SELECT_TABLE_DIAG_ORIG.load(Ordering::SeqCst);
    if orig == 0 || orig == usize::MAX {
        return 0;
    }
    // Safety: the union slot holds either the game trampoline or the next handler, both callable
    // under the union signature.
    let next: er_hook::UnionFn = unsafe { std::mem::transmute(orig) };
    unsafe { next(a, b, c, d) }
}
/// Raised once the guard is on the union, so a second host arming a character row does not add the
/// same handler again.
static GUARD_INSTALLED: AtomicUsize = AtomicUsize::new(0);

/// Install the guard on the native profile-renderer refresh.
///
/// Only a host that does not already own this detour calls this. The product does own it, from its
/// own private `MhHook` installed at attach, and must not call this.
///
/// # Safety
///
/// Process attach or startup-hook context.
pub unsafe fn install_profile_table_guard() -> bool {
    // Once per process. Both character rows want this guard and two hosts can now arm rows in one
    // process, so without the latch the same handler would go onto the union twice and the native
    // refresh would run through it twice per call.
    if GUARD_INSTALLED
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return true;
    }
    let Ok(addr) = game_rva_for_hook(PROFILE_RENDERER_REFRESH_RVA as u32) else {
        GUARD_INSTALLED.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "profileselect-table-guard: failed to resolve the native profile refresh rva 0x{PROFILE_RENDERER_REFRESH_RVA:x}; ProfileSelect cannot be opened safely"
        ));
        return false;
    };
    match unsafe {
        er_hook::register_union_hook(
            addr,
            profile_table_guard_hook,
            &PROFILE_SELECT_TABLE_DIAG_ORIG,
        )
    } {
        Ok(()) => {
            append_autoload_debug(format_args!(
                "profileselect-table-guard: registered the native profile refresh 0x{addr:x} on the union; an empty renderer table is rebuilt and a still-degraded one skips the refresh"
            ));
            true
        }
        Err(status) => {
            append_autoload_debug(format_args!(
                "profileselect-table-guard: register_union_hook failed: {status:?}; ProfileSelect cannot be opened safely"
            ));
            false
        }
    }
}
