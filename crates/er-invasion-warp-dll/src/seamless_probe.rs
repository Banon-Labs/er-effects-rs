//! Report the destination a Seamless invasion chose, once per invasion.
//!
//! Seamless Co-op runs its own invasion system: none of `ersc.dll`'s 286 resolved signature
//! slots reach `CSBreakInPointManager` or `CS::QuickmatchManager`, so the world-map pins this
//! crate injects -- built from the `.aip` table and MSB `InvasionPoint` regions -- are a VANILLA
//! invasion surface and say nothing about where a Seamless invader lands. The code that picks
//! the target is inside the Themida-encrypted part of `ersc.dll` and cannot be read statically.
//!
//! It does not have to be read. The decision's OUTPUT is written to `CSGameMan` in the clear:
//! an MSB entry-point entity id at `+0xaf0` and a destination map at `+0xac8`, both consumed by
//! `FUN_140afcf60`, the spawn resolver `ersc.dll` hooks. One invasion therefore yields the fact
//! that the static analysis could not.

/// Sample the invasion destination and log it when it changes.
///
/// # Safety
/// Game task thread. Every read is fault-closed; nothing is written and nothing is called.
#[cfg(windows)]
pub(crate) unsafe fn sample_invade_destination() {
    use er_invasion_warp::seamless_invade_probe::{
        InvadeDestinationWatcher, read_invade_destination,
    };

    // Session-scoped so the "same place twice" case is visible across a whole play session.
    static WATCHER: std::sync::Mutex<InvadeDestinationWatcher> =
        std::sync::Mutex::new(InvadeDestinationWatcher::new());

    let Ok(base) = er_game_base::mem::game_module_base() else {
        return;
    };
    let Some(reading) = (unsafe { read_invade_destination(base) }) else {
        return;
    };
    let mut watcher = match WATCHER.lock() {
        Ok(watcher) => watcher,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(fresh) = watcher.observe(reading) else {
        return;
    };
    drop(watcher);

    crate::standalone_log(format_args!(
        "seamless-invade: destination CHOSEN -- entry point entity id {} ({:#010x}) in map {} \
         (block {:#010x}), rapid_reentry(\"Seek opponent\")={}. This is what Seamless picked, \
         read from CSGameMan+0xaf0/+0xac8 AFTER its encrypted matchmaking decided. It is an MSB \
         ENTRY POINT, not a coordinate, and it is NOT drawn from the .aip or MSB InvasionPoint \
         tables the map pins use.",
        fresh.entry_point,
        fresh.entry_point as u32,
        fresh.block,
        fresh.block.raw(),
        fresh.rapid_reentry,
    ));
}

/// Host-build stub: there is no game to read, so this does nothing.
///
/// # Safety
/// Nothing to uphold -- the body is empty. `unsafe` only so the signature matches the
/// `cfg(windows)` twin above, which does read game memory.
#[cfg(not(windows))]
pub(crate) unsafe fn sample_invade_destination() {}
