//! Publish this host's current map on its own Steam lobby, so invaders can ASK for a location
//! instead of sampling and rejecting.
//!
//! # Why this exists, and what it is worth
//!
//! Seamless decides an invasion destination server-side and pushes it to the client. Filtering that
//! after the fact works (see [`crate::local_invasion_filter`]) but it is REJECTION SAMPLING, and
//! rejection sampling has no upper bound: measured 2026-08-06, a query returns 13 hosts worldwide
//! in one bracket, so reaching one specific host takes ~13 draws on average and the tail never
//! closes. Every rejection costs a full match negotiation and cancel.
//!
//! Narrowing the QUERY removes the sampling entirely. Seamless already does exactly this -- it
//! attaches five filters to its lobby-list request, every one of them a key some host published:
//!
//! ```text
//!   AddRequestLobbyListStringFilter("lobby_breakin_lobby_ykssr_199_6",       "true")
//!   AddRequestLobbyListStringFilter("matchmaking_breakin_lobby_ykssr_199_6", "4_3")
//!   AddRequestLobbyListStringFilter("lobby_type", "yknx3_seamless_master_lobby")
//!   AddRequestLobbyListNumericalFilter("ykssr_dlc", 1)
//!   AddRequestLobbyListStringFilter("lobby_key", "<sha256>")
//! ```
//!
//! None of them carries the host's LOCATION -- that is the gap this module fills, and the reason a
//! host must run this DLL for it to work at all: only a lobby's OWNER may call `SetLobbyData`, so
//! an invader cannot annotate someone else's lobby.
//!
//! # Why publishing here is safe for everyone else
//!
//! MEASURED 2026-08-06: a lobby that lacks a filtered key is EXCLUDED from the results (a baseline
//! of 13 lobbies went to 0 when one filter on an unpublished key was added, reproduced twice). That
//! cuts both ways and the second direction is the important one:
//!
//! * a vanilla Seamless invader never filters on our key, so publishing it changes NOTHING for
//!   them -- they still match this host exactly as before; and
//! * `lobby_key` and `lobby_type`, the two keys that decide who can see whom at all, are never
//!   written or filtered on here. Those stay Seamless's alone.
//!
//! So a host adopting this loses no reach. The cost falls entirely on an INVADER who chooses to
//! filter, and it is theirs to choose: filtering narrows their own results to hosts running this
//! DLL. That is why the invader half must be opt-in, never a default.
//!
//! # Republishing, and why it is not optional
//!
//! Seamless writes its whole advertisement ONCE, at `CreateLobby`, and never touches it again --
//! measured on a live host session: 7 `SetLobbyData` calls at creation, zero afterwards, including
//! across a Dried Finger use that visibly changed the host's invasion state. A location key written
//! only at creation would therefore go stale the moment the host walks to another map, and would
//! send invaders to where that host was twenty minutes ago. Steam permits the owner to rewrite its
//! own lobby data freely, so [`publish_current_map`] re-publishes whenever the block changes.
//!
//! # No hooks
//!
//! Nothing here is detoured. The interface comes from an exported accessor, the lobby id is already
//! in the Seamless session object this crate reads, and the block id comes from the same anchor the
//! location filter uses. That matters because `ersc.dll` is Themida-protected: an inline patch
//! there is unproven-safe, and this needs none.

use er_invasion_warp::invasion_warp::BlockKey;

/// The lobby key this DLL publishes the host's current map under.
///
/// Namespaced deliberately. It must not collide with a key Seamless might add later, and when it
/// shows up in someone else's capture it should be obvious where it came from. Seamless's own keys
/// are `lobby_*` / `ykssr_*` / `matchmaking_*`; nothing of ours may look like one of those.
pub const LOBBY_MAP_KEY: &str = "er_invasion_warp_map";

/// `ISteamMatchmaking::SetLobbyData` -- vtable slot 20.
///
/// Cross-checked two ways rather than taken from the SDK header alone: static RE of `ersc.dll`
/// finds its publish call emitted as `call qword [rax+0xa0]` (0xa0 / 8 == 20), and a live capture
/// of that slot showed `(this, lobbyId, "lobby_type", "yknx3_seamless_master_lobby")` -- the
/// argument shape SetLobbyData has and its neighbours do not.
pub const SET_LOBBY_DATA_SLOT: usize = 20;

/// The exported accessor that hands out `ISteamMatchmaking` without hooking anything.
///
/// Seamless obtains the interface once at startup and thereafter calls vtable slots directly, so
/// there is no flat-API traffic to piggyback on. Calling the accessor ourselves returns the same
/// process-wide singleton, which is why this module needs no detour and no boot-time race.
pub const MATCHMAKING_ACCESSOR: &str = "SteamAPI_SteamMatchmaking_v009\0";

/// Offset of the advertisement lobby's `CSteamID` within Seamless's session object.
///
/// The session already carries it, so the lobby this DLL publishes on is READ rather than captured
/// by hooking `CreateLobby`. A host session was observed creating two lobbies -- one carrying the
/// published data, one carrying the members -- and this is the former.
pub const SESSION_LOBBY_ID_OFFSET: usize = 0x178;

/// How the block id is spelled on the wire.
///
/// The engine's own debug spelling (`m60_51_36_00`), because both sides must format identically for
/// an EQUALITY filter to match and a canonical human-readable form is far harder to get subtly
/// wrong than a raw integer whose endianness and BCD index byte have both bitten this repo before.
#[must_use]
pub fn map_value(block: BlockKey) -> String {
    block.to_string()
}

/// What to publish now, given what was last published.
///
/// Returns `None` when the advertisement is already correct, so the common case costs nothing and
/// a host standing still does not rewrite its lobby every tick.
///
/// A block of `None` -- no resolvable location -- publishes NOTHING rather than clearing the key.
/// Clearing would be worse than staleness: an invader filtering for a location would silently stop
/// matching a host who is still perfectly reachable, and the failure would look like "nobody is
/// online" rather than like a bug.
#[must_use]
pub fn pending_publish(current: Option<BlockKey>, last_published: Option<&str>) -> Option<String> {
    let value = map_value(current?);
    if last_published == Some(value.as_str()) {
        return None;
    }
    Some(value)
}

/// The live half: resolve Steam, read where we are, and write the key if it changed.
#[cfg(windows)]
mod live {
    use super::{LOBBY_MAP_KEY, MATCHMAKING_ACCESSOR, SET_LOBBY_DATA_SLOT, pending_publish};
    use er_invasion_warp::invasion_warp::BlockKey;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleA(name: *const u8) -> isize;
        fn GetProcAddress(module: isize, name: *const u8) -> usize;
    }

    /// `ISteamMatchmaking* SteamAPI_SteamMatchmaking_v009(void)`.
    type MatchmakingAccessor = unsafe extern "system" fn() -> usize;
    /// `bool SetLobbyData(this, CSteamID lobby, const char *key, const char *value)`.
    ///
    /// The `CSteamID` is an 8-byte POD passed BY VALUE, which is why it is a plain `u64` here and
    /// not a pointer -- getting that wrong would shift every argument after it.
    type SetLobbyDataFn = unsafe extern "system" fn(usize, u64, *const u8, *const u8) -> bool;

    /// Cached interface pointer. Steam hands out a process-wide singleton, so this is resolved once
    /// rather than per publish.
    static MATCHMAKING: AtomicUsize = AtomicUsize::new(0);
    /// What this host last told the world, so an unchanged map costs nothing.
    static LAST_PUBLISHED: Mutex<Option<String>> = Mutex::new(None);
    static PUBLISHES: AtomicUsize = AtomicUsize::new(0);
    static REFUSALS: AtomicUsize = AtomicUsize::new(0);

    fn matchmaking() -> Option<usize> {
        let cached = MATCHMAKING.load(Ordering::SeqCst);
        if cached != 0 {
            return Some(cached);
        }
        let module = unsafe { GetModuleHandleA(c"steam_api64.dll".as_ptr().cast()) };
        if module == 0 {
            return None;
        }
        let accessor = unsafe { GetProcAddress(module, MATCHMAKING_ACCESSOR.as_ptr()) };
        if accessor == 0 {
            return None;
        }
        let iface = unsafe { core::mem::transmute::<usize, MatchmakingAccessor>(accessor)() };
        if iface == 0 {
            return None;
        }
        MATCHMAKING.store(iface, Ordering::SeqCst);
        Some(iface)
    }

    /// `SetLobbyData` off the interface's vtable.
    ///
    /// Read rather than imported, because Seamless reaches Steam the same way and the flat export
    /// of this method is not what the running process uses.
    fn set_lobby_data(iface: usize) -> Option<SetLobbyDataFn> {
        let vtable = unsafe { er_game_base::mem::safe_read_usize(iface) }?;
        let slot = unsafe {
            er_game_base::mem::safe_read_usize(vtable + SET_LOBBY_DATA_SLOT * size_of::<usize>())
        }?;
        (slot != 0).then(|| unsafe { core::mem::transmute::<usize, SetLobbyDataFn>(slot) })
    }

    fn current_block() -> Option<BlockKey> {
        let base = er_game_base::mem::game_module_base().ok()?;
        let raw = unsafe { er_invasion_warp::warp::current_block_id(base) }?;
        Some(BlockKey::from_raw(raw))
    }

    /// Publish this host's map if it changed. Safe to call every tick.
    ///
    /// Every failure path is a silent no-op ON PURPOSE. Not being findable by location is a missing
    /// convenience; a DLL that panics or spams because Steam was not ready would be a broken game.
    pub fn publish_current_map() {
        let Some(value) = pending_publish(current_block(), last_published().as_deref()) else {
            return;
        };
        let Some(iface) = matchmaking() else {
            REFUSALS.fetch_add(1, Ordering::SeqCst);
            return;
        };
        let Some(lobby) = crate::local_invasion_filter::advertisement_lobby_id() else {
            // No lobby yet -- the host has not opened to invaders. Nothing to advertise on.
            return;
        };
        let Some(write) = set_lobby_data(iface) else {
            REFUSALS.fetch_add(1, Ordering::SeqCst);
            return;
        };
        let key = format!("{LOBBY_MAP_KEY}\0");
        let payload = format!("{value}\0");
        let ok = unsafe { write(iface, lobby, key.as_ptr(), payload.as_ptr()) };
        if ok {
            *LAST_PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()) = Some(value.clone());
            let n = PUBLISHES.fetch_add(1, Ordering::SeqCst) + 1;
            crate::standalone_log(format_args!(
                "lobby-publish: {LOBBY_MAP_KEY} = {value} on lobby {lobby:#x} (#{n})"
            ));
        } else {
            // Steam refused. Do NOT record it as published, or a transient failure would be
            // remembered as success and never retried.
            REFUSALS.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn last_published() -> Option<String> {
        LAST_PUBLISHED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// `(publishes, refusals)`, so a run can be judged without reading the log.
    #[must_use]
    pub fn tally() -> (usize, usize) {
        (
            PUBLISHES.load(Ordering::SeqCst),
            REFUSALS.load(Ordering::SeqCst),
        )
    }
}

#[cfg(windows)]
pub use live::{publish_current_map, tally};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_value_is_the_engines_own_map_spelling() {
        assert_eq!(
            map_value(BlockKey::from_parts(60, 51, 36, 0)),
            "m60_51_36_00"
        );
        // The DLC area, because an invader filtering for a Shadow-of-the-Erdtree location must
        // produce a byte-identical string to the host standing there.
        assert_eq!(
            map_value(BlockKey::from_parts(61, 46, 43, 0)),
            "m61_46_43_00"
        );
    }

    #[test]
    fn a_two_digit_index_survives_the_bcd_round_trip() {
        // The index byte is BCD-packed for overworld areas. A raw-byte spelling would publish
        // "m60_46_39_10" as index 0x10 == 16, and host and invader would never match.
        let block = BlockKey::from_parts(60, 46, 39, 10);
        assert_eq!(map_value(block), "m60_46_39_10");
    }

    #[test]
    fn nothing_is_published_while_the_advertisement_is_already_correct() {
        let here = BlockKey::from_parts(60, 51, 36, 0);
        assert_eq!(pending_publish(Some(here), Some("m60_51_36_00")), None);
    }

    #[test]
    fn moving_to_another_map_republishes() {
        let there = BlockKey::from_parts(61, 46, 43, 0);
        assert_eq!(
            pending_publish(Some(there), Some("m60_51_36_00")),
            Some("m61_46_43_00".to_owned())
        );
    }

    #[test]
    fn the_first_publish_happens_with_nothing_recorded() {
        let here = BlockKey::from_parts(60, 51, 36, 0);
        assert_eq!(
            pending_publish(Some(here), None),
            Some("m60_51_36_00".to_owned())
        );
    }

    /// Losing the location must not RETRACT it. A host whose anchor briefly fails to resolve is
    /// still reachable, and clearing the key would make them vanish from a filtered search in a way
    /// that reads as "nobody online" rather than as a fault.
    #[test]
    fn an_unresolvable_location_publishes_nothing_rather_than_clearing() {
        assert_eq!(pending_publish(None, Some("m60_51_36_00")), None);
        assert_eq!(pending_publish(None, None), None);
    }

    /// This module's shipping code, comments and tests removed. Same reasoning as the twin helper
    /// in `local_invasion_filter`: a ban list is written in code, so a guard that scans its own
    /// test module trips on the very list that defines it.
    fn product_code() -> String {
        let source = include_str!("lobby_publish.rs");
        let shipping = source
            .split_once("#[cfg(test)]")
            .map_or(source, |(before, _)| before);
        shipping
            .lines()
            .filter(|line| !line.trim_start().starts_with("//!"))
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Publishing our own key is permitted; touching the two that decide mutual visibility is not.
    ///
    /// This is the same boundary `local_invasion_filter` enforces, restated here because the ban
    /// has to travel with the code that could break it -- a guard living in another file protects
    /// that file, not this one.
    #[test]
    fn the_keys_that_decide_visibility_are_never_written_here() {
        let code = product_code();
        for reserved in ["lobby_key", "lobby_type"] {
            assert!(
                !code.contains(&format!("\"{reserved}\"")),
                "{reserved}: this key decides who can see whom -- writing it would change what \
                 every other Seamless player matches"
            );
        }
    }

    /// The consent line, restated where it could be crossed.
    ///
    /// A module holding a live `ISteamMatchmaking` pointer is exactly where person-targeting would
    /// be easiest to add: the interface that publishes is the interface that can enumerate lobby
    /// owners and members. Filtering may DECLINE, it may never SELECT A PERSON.
    #[test]
    fn no_person_targeting_primitive_is_reachable_from_here() {
        let code = product_code();
        for banned in [
            "GetLobbyOwner",
            "GetLobbyMemberByIndex",
            "GetNumLobbyMembers",
            "GetLobbyByIndex",
        ] {
            assert!(
                !code.contains(&format!("{banned}(")),
                "{banned}: selecting by who is in a lobby targets someone who never opted in"
            );
        }
    }

    /// Our key must not be mistakable for one of Seamless's, in either direction.
    #[test]
    fn the_published_key_is_namespaced_away_from_seamless() {
        for theirs in ["lobby_", "ykssr_", "matchmaking_"] {
            assert!(
                !LOBBY_MAP_KEY.starts_with(theirs),
                "{LOBBY_MAP_KEY} collides with Seamless's own {theirs}* namespace"
            );
        }
        assert!(
            LOBBY_MAP_KEY.starts_with("er_"),
            "ours should be obviously ours"
        );
    }
}
