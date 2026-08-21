//! The world-map confirm interception: what happens when a player selects an invasion pin.
//!
//! Split out of `map_hooks` because it answers a different question from the rest of that module.
//! Everything there is about GETTING PINS ONTO THE MAP -- appending rows, sampling a donor param,
//! projecting coordinates, naming places. This is the one seam on the way back OUT, where a
//! selection the player made turns into an action, and it is the seam both the softlock fix and the
//! warp policy live on.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Trampoline to the original warp-job assembler.
pub(crate) static ORIG_WARP_JOB_ASSEMBLER: AtomicUsize = AtomicUsize::new(0);
/// Confirms recognised as ours, and how many of those issued a warp.
///
/// Under the current [`er_invasion_warp::warp::WarpPolicy`] the second is expected to stay at
/// **zero forever** -- invasion locations are markers, so every confirm is refused. `intercepted`
/// rising with `warped` flat is the CORRECT signature, not a regression. It is still counted
/// because a non-zero value would mean the policy gate was bypassed, which is worth being able to
/// see without reading the log.
static CONFIRMS_INTERCEPTED: AtomicUsize = AtomicUsize::new(0);
static CONFIRMS_WARPED: AtomicUsize = AtomicUsize::new(0);

/// Union handler for the warp-job assembler `FUN_1407a04f0`.
///
/// THIS IS THE SOFTLOCK FIX. All five confirm routes funnel through here, and `R8` points at the
/// bonfire entity id BEFORE any MenuJob is allocated. Without this hook, selecting an injected
/// pin hands our synthetic id to the native grace warp, which passes it to
/// `CSLuaEventManImp::CallLua_Warp`; Lua cannot resolve it, the stage transition never completes,
/// and the game hangs on the loading screen. That is exactly what a live run did.
///
/// On recognising one of ours we ask [`er_invasion_warp::warp::request_invasion_warp`] and return a
/// NULL job. Swallowing is safe: the callers' `Clone` (0x1407a7b60) and enqueue (0x1407a9250) both
/// NULL-check, and the engine itself returns a NULL job on its own no-SpecialEffect path -- so a
/// NULL out-slot is a state the callers already handle.
///
/// # The swallow is NOT the warp being disabled
///
/// These are two independent things and conflating them would be a real bug. The swallow exists
/// because a synthetic id reaching `CallLua_Warp` HANGS THE GAME, and it must keep happening
/// whatever the warp policy is. Separately, `request_invasion_warp` now always answers
/// [`er_invasion_warp::warp::WarpError::NotAWarpDestination`] -- invasion locations are markers,
/// not fast-travel points -- so the ordinary outcome here is the refusal branch below: the map
/// stays open and the player does not move. The `Ok` branch is retained rather than deleted
/// because the swallow, not the warp, is what this hook is FOR.
///
/// # Safety
/// Installed by the union on a byte-verified prologue; ABI is
/// `(outJobSlot, menuOwner+0x50, const u32* entityId, MenuString* name)`.
#[cfg(windows)]
pub(crate) unsafe extern "system" fn warp_job_assembler_hook(
    out_job_slot: usize,
    menu_owner: usize,
    entity_id_ptr: usize,
    name: usize,
) -> usize {
    let entity_id = if entity_id_ptr != 0 {
        unsafe { er_game_base::mem::safe_read_i32(entity_id_ptr) }
    } else {
        None
    };
    let registry_ptr = crate::map_hooks::INJECTED_REGISTRY.load(Ordering::SeqCst);
    // THE SWALLOW IS GATED ON THE ID BAND, NEVER ON THE LOOKUP SUCCEEDING.
    //
    // It used to require BOTH `registry_ptr != 0` AND a successful `target_for_entity_id`, with the
    // NULL-job return nested inside that `if let Some(target)`. So a synthetic id that the registry
    // could not resolve -- or any confirm arriving before the registry pointer was published -- fell
    // straight through to the original assembler, which is the softlock this hook exists to prevent:
    // the 0x7F000000-band id reaches CSLuaEventManImp::CallLua_Warp, Lua cannot resolve it, and the
    // stage transition never completes.
    //
    // The invariant is one-directional and does not depend on our bookkeeping being correct: an id in
    // our private band is MEANINGLESS TO THE ENGINE, so the native path can never do anything useful
    // with it, only hang. A lookup miss is our bug; letting it through converts our bug into the
    // user's frozen game. Refusing the warp and returning a NULL job leaves the map open and the
    // player in control, which is the strictly better failure.
    if let Some(entity_id) = entity_id
        && er_invasion_warp::map_surface::is_invasion_entity_id(entity_id)
    {
        let target = if registry_ptr != 0 {
            // SAFETY: the registry was leaked at injection time and is never freed or mutated.
            let registry: &er_invasion_warp::map_surface::InvasionRowRegistry =
                unsafe { &*(registry_ptr as *const _) };
            registry.target_for_entity_id(entity_id).copied()
        } else {
            None
        }
        // A live top-up hands out ids PAST the registry's dense range, so the registry cannot know
        // them. Without this fallback such a pin drew on the map and then did nothing when
        // selected -- the swallow below would fire with no destination.
        .or_else(|| crate::map_hooks::top_up_target_for_entity_id(entity_id));
        if let Some(target) = target {
            CONFIRMS_INTERCEPTED.fetch_add(1, Ordering::SeqCst);
            match unsafe { er_invasion_warp::warp::request_invasion_warp(&target) } {
                Ok(outcome) => {
                    CONFIRMS_WARPED.fetch_add(1, Ordering::SeqCst);
                    crate::standalone_log(format_args!(
                        "map-confirm: invasion pin entity_id={entity_id:#x} -> LOCAL warp to \
                         block {} point {} (origin {:#010x} requested {:#010x} effective \
                         {:#010x} spawn_flag={} session gate: {}); native grace warp SWALLOWED. \
                         REQUESTED ONLY -- arrival is judged separately",
                        target.block,
                        target.point_index,
                        outcome.origin_block,
                        outcome.requested_block,
                        outcome.effective_block,
                        outcome.spawn_flag,
                        outcome.session_gate.describe()
                    ));
                    // Hand it to the driver's arrival watcher. Without this the line above was
                    // the ONLY record of a map warp, and it reports that the spawn slot latched,
                    // not that the player arrived -- so a warp that did nothing was
                    // indistinguishable from one that worked.
                    crate::drive::note_external_warp(outcome);
                }
                Err(error) => {
                    crate::standalone_log(format_args!(
                        "map-confirm: invasion pin entity_id={entity_id:#x} REFUSED: {error}; \
                         native warp still swallowed rather than sending a synthetic id to \
                         Lua_Warp (which softlocks)"
                    ));
                }
            }
        } else {
            // An id in our band that we cannot map back to a target. Our bookkeeping is wrong --
            // the registry was not published before the map opened, or the id was never registered
            // -- but that is OUR problem to diagnose, and it must not become a hang. Counted as an
            // interception because the confirm WAS ours and the native path WAS suppressed.
            //
            // The tallies alone no longer distinguish this case: since the warp policy refuses
            // every destination, `intercepted > warped` is now the NORMAL state and it used to be
            // this bug's signature. The UNRESOLVED log line below is the discriminator -- it says
            // the registry and the injected rows disagree, which the refusal line does not.
            CONFIRMS_INTERCEPTED.fetch_add(1, Ordering::SeqCst);
            crate::standalone_log(format_args!(
                "map-confirm: invasion pin entity_id={entity_id:#x} UNRESOLVED \
                 (registry_ptr={registry_ptr:#x}) -- no target for an id in our own band. \
                 Native warp SWALLOWED anyway: handing a synthetic id to Lua_Warp hangs the \
                 game on the loading screen, so a refused warp with the map still open is the \
                 better failure. This line means the injected registry and the injected rows \
                 disagree -- fix that, not this guard"
            ));
        }
        // NULL job on EVERY path that recognised one of our ids -- warped, refused, or
        // unresolved. Letting the native assembler run with a synthetic id is the softlock, so
        // it is never the fallback, and this return is deliberately outside the lookup so no
        // future edit can nest it back inside one.
        if out_job_slot != 0 {
            unsafe { *(out_job_slot as *mut usize) = 0 };
        }
        return out_job_slot;
    }

    let orig = ORIG_WARP_JOB_ASSEMBLER.load(Ordering::SeqCst);
    if orig == 0 {
        // No trampoline: refuse rather than fabricate a job pointer.
        if out_job_slot != 0 {
            unsafe { *(out_job_slot as *mut usize) = 0 };
        }
        return out_job_slot;
    }
    type AssemblerFn = unsafe extern "system" fn(usize, usize, usize, usize) -> usize;
    let original: AssemblerFn = unsafe { core::mem::transmute(orig) };
    unsafe { original(out_job_slot, menu_owner, entity_id_ptr, name) }
}

/// Confirm-hook tallies: `(intercepted, warped)`.
#[must_use]
pub fn confirm_tallies() -> (usize, usize) {
    (
        CONFIRMS_INTERCEPTED.load(Ordering::SeqCst),
        CONFIRMS_WARPED.load(Ordering::SeqCst),
    )
}
