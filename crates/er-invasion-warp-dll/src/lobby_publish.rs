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
//! # Hooks: exactly one, and not in `ersc.dll`
//!
//! PUBLISHING detours nothing. The interface comes from an exported accessor, the lobby id is
//! already in the Seamless session object this crate reads, and the block id comes from the same
//! anchor the location filter uses.
//!
//! HUNT MODE needs one, because a filter has to be added between Seamless's last
//! `AddRequestLobbyList*` and the request going out, and only Seamless knows when that is. So
//! `RequestLobbyList` is detoured -- in `steamclient64.dll`'s interface vtable, NOT in `ersc.dll`.
//! That distinction is the point: ersc is Themida-protected and an inline patch there is
//! unproven-safe, whereas Steam's own interface is ordinary code this DLL already calls into.

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

/// Offset of a lobby `CSteamID` within Seamless's session object.
///
/// CANDIDATE, not gospel. A host session creates TWO lobbies -- one carrying the advertisement,
/// one carrying the members -- and this offset was assumed to be the former from the fact that it
/// is A lobby id. That assumption was never checked against the id ersc actually publishes to, and
/// publishing to the wrong one fails in the worst possible way: Steam accepts the write, our log
/// says published, and an invader filtering for the key matches nobody -- indistinguishable from
/// "nobody is online".
///
/// So the offset is only where the search STARTS. [`ADVERTISEMENT_MARKER_KEY`] decides whether it
/// is right, at runtime, on every publish.
pub const SESSION_LOBBY_ID_OFFSET: usize = 0x178;

/// `ISteamMatchmaking::GetLobbyData` -- vtable slot 19.
///
/// READ ONLY, and that distinction is the whole reason this is allowed to name a Seamless key at
/// all: reading lobby data changes nothing for any other player, whereas writing or filtering on
/// one of their keys changes what everybody matches.
pub const GET_LOBBY_DATA_SLOT: usize = 19;

/// The key whose presence PROVES a lobby is the one invaders query.
///
/// Seamless filters its lobby list on `lobby_type == yknx3_seamless_master_lobby`, so by definition
/// the only lobbies an invader can ever see are the ones carrying that pair. Reading it back off
/// our publish target turns "the offset is probably the advertisement lobby" into a fact checked on
/// every write -- and if the offset is ever wrong, or Seamless reorders its lobbies in a future
/// version, publishing REFUSES instead of writing somewhere nobody reads.
pub const ADVERTISEMENT_MARKER_KEY: &str = "lobby_type\0";
/// The value [`ADVERTISEMENT_MARKER_KEY`] carries on the advertisement lobby.
pub const ADVERTISEMENT_MARKER_VALUE: &str = "yknx3_seamless_master_lobby";

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

/// `ISteamMatchmaking::RequestLobbyList` -- vtable slot 4, where a filter must be added.
///
/// Filters accumulate and are CONSUMED by this call, so ours has to land between Seamless's last
/// `AddRequestLobbyList*` and the request going out. Proven live 2026-08-06: injecting one filter
/// in this function's prologue took a 13-lobby result set to 0, twice.
pub const REQUEST_LOBBY_LIST_SLOT: usize = 4;

/// `ISteamMatchmaking::AddRequestLobbyListStringFilter` -- vtable slot 5.
pub const ADD_STRING_FILTER_SLOT: usize = 5;

/// Which single location hunt mode should ask Steam for.
///
/// # Why ONE location and not a set
///
/// A Steam string filter is an EQUALITY test on one value -- there is no OR. Seamless attaches five
/// of them and they AND together. So hunt mode can narrow to one place, and asking for two would
/// silently return nothing at all (a lobby cannot equal both). Rather than let that happen, several
/// marked blocks REFUSE to hunt and say so; the reject filter still handles the multi-location case,
/// slowly but correctly.
///
/// # What it picks
///
/// * one marked block  -> that one, because marking a single place is unambiguous intent
/// * several marked    -> `None`; a string filter cannot express OR
/// * none marked       -> where the player is standing, which is "invade locally"
#[must_use]
pub fn hunt_filter_value(
    enabled: bool,
    marked_blocks: &[u32],
    current: Option<BlockKey>,
) -> Option<String> {
    if !enabled {
        return None;
    }
    match marked_blocks {
        [] => current.map(map_value),
        [only] => Some(map_value(BlockKey::from_raw(*only))),
        _ => None,
    }
}

/// Why hunt mode is not filtering, in words a user can act on.
///
/// Separate from [`hunt_filter_value`] because "off" and "cannot express that" are different
/// situations and a silent `None` conflates them -- the second one is a user asking for something
/// the transport cannot do, and they deserve to be told rather than left wondering why nothing
/// narrowed.
#[must_use]
pub fn hunt_refusal(
    enabled: bool,
    marked_blocks: &[u32],
    current: Option<BlockKey>,
) -> Option<&'static str> {
    if !enabled {
        return None;
    }
    match marked_blocks {
        [] if current.is_none() => Some(
            "hunt: no marked location and your own block is unreadable, so there is nothing to ask \
             Steam for -- searching unfiltered",
        ),
        [_, _, ..] => Some(
            "hunt: more than one location is marked, and a Steam filter tests ONE value with no OR \
             -- asking for several would match nobody. Mark exactly one, or leave hunt off and let \
             the reject filter handle it",
        ),
        _ => None,
    }
}

/// The live half: resolve Steam, read where we are, and write the key if it changed.
#[cfg(windows)]
mod live {
    use super::{
        ADD_STRING_FILTER_SLOT, ADVERTISEMENT_MARKER_KEY, ADVERTISEMENT_MARKER_VALUE,
        GET_LOBBY_DATA_SLOT, LOBBY_MAP_KEY, MATCHMAKING_ACCESSOR, REQUEST_LOBBY_LIST_SLOT,
        SET_LOBBY_DATA_SLOT, hunt_filter_value, hunt_refusal, pending_publish,
    };
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
    /// `const char *GetLobbyData(this, CSteamID lobby, const char *key)` -- READ ONLY.
    type GetLobbyDataFn = unsafe extern "system" fn(usize, u64, *const u8) -> *const i8;

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

    /// Is this the lobby an invader's query can actually see?
    ///
    /// Seamless filters its lobby list on `lobby_type == yknx3_seamless_master_lobby`, so a lobby
    /// carrying that pair is BY DEFINITION one the query can return, and a lobby without it is
    /// invisible no matter what we write there. Reading it costs one call and converts a guessed
    /// struct offset into a fact re-checked on every publish.
    ///
    /// Reading a Seamless key is not the thing the guards forbid. Writing or filtering on
    /// `lobby_type` would change what every other player matches; reading it changes nothing for
    /// anyone, and is the only way to prove we are not writing into a lobby nobody queries.
    fn is_advertisement_lobby(iface: usize, lobby: u64) -> bool {
        let Some(read) = get_lobby_data(iface) else {
            return false;
        };
        let got = unsafe { read(iface, lobby, ADVERTISEMENT_MARKER_KEY.as_ptr()) };
        if got.is_null() {
            return false;
        }
        let value = unsafe { core::ffi::CStr::from_ptr(got.cast()) };
        value
            .to_str()
            .is_ok_and(|v| v == ADVERTISEMENT_MARKER_VALUE)
    }

    fn get_lobby_data(iface: usize) -> Option<GetLobbyDataFn> {
        let vtable = unsafe { er_game_base::mem::safe_read_usize(iface) }?;
        let slot = unsafe {
            er_game_base::mem::safe_read_usize(vtable + GET_LOBBY_DATA_SLOT * size_of::<usize>())
        }?;
        (slot != 0).then(|| unsafe { core::mem::transmute::<usize, GetLobbyDataFn>(slot) })
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
        // REFUSE rather than write somewhere nobody queries. The struct offset that produced this
        // lobby id was an assumption; this is the check that makes being wrong LOUD instead of
        // silent, because the silent failure mode is a filter matching nobody while every log line
        // says success.
        if !is_advertisement_lobby(iface, lobby) {
            if REFUSALS.fetch_add(1, Ordering::SeqCst) == 0 {
                crate::standalone_log(format_args!(
                    "lobby-publish: REFUSED -- lobby {lobby:#x} does not carry Seamless's own \
                     advertisement marker, so an invader's query could never see it. Publishing \
                     there would look like success and match nobody."
                ));
            }
            return;
        }
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

    // -----------------------------------------------------------------------------------------
    // HUNT MODE: narrow the outgoing query instead of rejecting its answers
    // -----------------------------------------------------------------------------------------

    /// Trampoline to Steam's own `RequestLobbyList`.
    static ORIG_REQUEST_LOBBY_LIST: AtomicUsize = AtomicUsize::new(0);
    static HUNT_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
    static FILTERS_ADDED: AtomicUsize = AtomicUsize::new(0);
    /// So a refusal is explained once rather than every query.
    static HUNT_REFUSAL_SAID: AtomicUsize = AtomicUsize::new(0);

    type AddStringFilterFn = unsafe extern "system" fn(usize, *const u8, *const u8, i32);

    /// `k_ELobbyComparisonEqual`. The only comparison that makes sense for a map id, and the one
    /// Seamless uses for every string filter it attaches.
    const LOBBY_COMPARISON_EQUAL: i32 = 0;

    /// Add our location filter to the query Seamless is about to send.
    ///
    /// Runs BEFORE the original, because filters are accumulated and then consumed by this call.
    ///
    /// Adding a filter narrows OUR OWN results and nothing else -- no other player's matching
    /// changes, nothing is published, no game state is written. The cost is entirely ours: hosts
    /// without this DLL do not carry the key, so they stop being visible while hunt is on. That is
    /// why it is opt-in and why the refusal above explains itself rather than failing silently.
    /// `SteamAPICall_t RequestLobbyList(this)` takes only `this`, but the union dispatcher's
    /// trampoline type is fixed at four arguments. Declaring the extra three and ignoring them is
    /// ABI-safe under Win64 fastcall -- they are simply registers the callee never reads -- and it
    /// is what lets this share the same hook machinery as every other detour in this DLL.
    unsafe extern "system" fn request_lobby_list_hook(
        iface: usize,
        b: usize,
        c: usize,
        d: usize,
    ) -> usize {
        if let Some(value) = hunt_target() {
            if let Some(add) = add_string_filter(iface) {
                let key = format!("{LOBBY_MAP_KEY}\0");
                let payload = format!("{value}\0");
                unsafe {
                    add(
                        iface,
                        key.as_ptr(),
                        payload.as_ptr(),
                        LOBBY_COMPARISON_EQUAL,
                    )
                };
                let n = FILTERS_ADDED.fetch_add(1, Ordering::SeqCst) + 1;
                if n == 1 {
                    crate::standalone_log(format_args!(
                        "hunt: asking Steam for hosts at {value} only (#{n}) -- hosts without this \
                         DLL do not publish the key and will not be returned"
                    ));
                }
            }
        }
        let orig = ORIG_REQUEST_LOBBY_LIST.load(Ordering::SeqCst);
        if orig == 0 {
            return 0;
        }
        unsafe { core::mem::transmute::<usize, er_hook::UnionFn>(orig)(iface, b, c, d) }
    }

    fn add_string_filter(iface: usize) -> Option<AddStringFilterFn> {
        let vtable = unsafe { er_game_base::mem::safe_read_usize(iface) }?;
        let slot = unsafe {
            er_game_base::mem::safe_read_usize(vtable + ADD_STRING_FILTER_SLOT * size_of::<usize>())
        }?;
        (slot != 0).then(|| unsafe { core::mem::transmute::<usize, AddStringFilterFn>(slot) })
    }

    /// The one location to ask for, or `None` to leave the query alone.
    fn hunt_target() -> Option<String> {
        let config = crate::local_invasion_filter::current_config_snapshot()?;
        let marked: Vec<u32> = config.allowed_blocks.iter().copied().collect();
        if let Some(why) = hunt_refusal(config.hunt, &marked, current_block()) {
            if HUNT_REFUSAL_SAID.swap(1, Ordering::SeqCst) == 0 {
                crate::standalone_log(format_args!("{why}"));
            }
            return None;
        }
        hunt_filter_value(config.hunt, &marked, current_block())
    }

    /// Install the query-narrowing hook. Idempotent; only ever called when hunt is configured on.
    pub fn install_hunt_hook() -> usize {
        if HUNT_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
            return 0;
        }
        let Some(iface) = matchmaking() else {
            HUNT_HOOK_INSTALLED.store(0, Ordering::SeqCst);
            return 0;
        };
        let Some(vtable) = (unsafe { er_game_base::mem::safe_read_usize(iface) }) else {
            return 0;
        };
        let Some(address) = (unsafe {
            er_game_base::mem::safe_read_usize(
                vtable + REQUEST_LOBBY_LIST_SLOT * size_of::<usize>(),
            )
        }) else {
            return 0;
        };
        match unsafe {
            er_hook::register_union_hook(
                address,
                request_lobby_list_hook as er_hook::UnionFn,
                &ORIG_REQUEST_LOBBY_LIST,
            )
        } {
            Ok(()) => 1,
            Err(status) => {
                crate::standalone_log(format_args!(
                    "hunt: could not hook RequestLobbyList: {status:?} -- hunt mode is INERT and \
                     every query will go out unfiltered"
                ));
                0
            }
        }
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
pub use live::{install_hunt_hook, publish_current_map, tally};

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
    /// REFINED 2026-08-06, because the blanket version was both too broad and, once the marker read
    /// was added, accidentally passing.
    ///
    /// Too broad: the harm these keys carry is in WRITING or FILTERING on them -- that changes what
    /// every other player matches. READING one changes nothing for anybody, and reading `lobby_type`
    /// is the only way to prove we are not publishing into a lobby no invader queries.
    ///
    /// Accidentally passing: the marker constant is `"lobby_type\0"`, which does not contain the
    /// substring `"lobby_type"` WITH its closing quote, so a literal-substring ban let it through by
    /// luck rather than by rule. A guard that passes for the wrong reason is worse than no guard,
    /// because it will keep passing when the reason stops holding.
    ///
    /// So this pins the rule instead: `lobby_key` never appears at all, and `lobby_type` appears
    /// ONLY in the marker declaration that the read uses.
    #[test]
    fn the_keys_that_decide_visibility_are_read_but_never_written_here() {
        let code = product_code();
        assert!(
            !code.contains("lobby_key"),
            "lobby_key decides who can see whom -- this module has no business naming it at all"
        );
        // `lobby_type` is permitted exactly once, as the marker the advertisement check READS.
        let mentions: Vec<&str> = code
            .lines()
            .filter(|line| line.contains("lobby_type"))
            .collect();
        assert_eq!(
            mentions.len(),
            1,
            "lobby_type may appear only in the marker declaration, found: {mentions:?}"
        );
        assert!(
            mentions[0].contains("ADVERTISEMENT_MARKER_KEY"),
            "the one lobby_type mention must be the read marker, not a write: {}",
            mentions[0]
        );
        // And the only key this module ever WRITES is its own.
        let writes: Vec<&str> = code
            .lines()
            .filter(|line| line.contains("write(iface"))
            .collect();
        for line in &writes {
            assert!(
                line.contains("key.as_ptr()"),
                "the publish call must write the key built from LOBBY_MAP_KEY: {line}"
            );
        }
        assert_eq!(writes.len(), 1, "exactly one write site: {writes:?}");
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

    // -----------------------------------------------------------------------------------------
    // HUNT MODE
    // -----------------------------------------------------------------------------------------

    const HERE: u32 = 0x3C_33_2B_00; // m60_51_43_00
    const THERE: u32 = 0x3D_2E_2B_00; // m61_46_43_00

    #[test]
    fn hunt_is_inert_until_the_user_asks_for_it() {
        let standing = BlockKey::from_raw(HERE);
        assert_eq!(hunt_filter_value(false, &[], Some(standing)), None);
        assert_eq!(hunt_filter_value(false, &[THERE], Some(standing)), None);
        assert_eq!(hunt_refusal(false, &[THERE, HERE], Some(standing)), None);
    }

    #[test]
    fn with_nothing_marked_hunt_asks_for_where_you_are_standing() {
        let standing = BlockKey::from_raw(HERE);
        assert_eq!(
            hunt_filter_value(true, &[], Some(standing)),
            Some("m60_51_43_00".to_owned())
        );
    }

    #[test]
    fn one_marked_location_is_the_one_asked_for() {
        let standing = BlockKey::from_raw(HERE);
        // The MARK wins over where you stand -- marking a place is the user naming a destination.
        assert_eq!(
            hunt_filter_value(true, &[THERE], Some(standing)),
            Some("m61_46_43_00".to_owned())
        );
    }

    /// THE TRANSPORT CONSTRAINT, pinned so nobody "improves" this into a silent no-match.
    ///
    /// A Steam string filter is an equality test on ONE value and the filters AND together, so
    /// asking for two locations returns nothing at all -- a lobby cannot equal both. Refusing and
    /// saying why beats issuing a query that is guaranteed empty.
    #[test]
    fn several_marked_locations_refuse_rather_than_matching_nobody() {
        let standing = BlockKey::from_raw(HERE);
        assert_eq!(
            hunt_filter_value(true, &[HERE, THERE], Some(standing)),
            None
        );
        let why = hunt_refusal(true, &[HERE, THERE], Some(standing)).expect("explains itself");
        assert!(
            why.contains("no OR"),
            "the reason must name the real cause: {why}"
        );
        assert!(
            why.contains("Mark exactly one"),
            "and tell the user what to do instead: {why}"
        );
    }

    #[test]
    fn hunt_with_no_mark_and_no_readable_block_says_so_instead_of_filtering() {
        assert_eq!(hunt_filter_value(true, &[], None), None);
        let why = hunt_refusal(true, &[], None).expect("explains itself");
        assert!(
            why.contains("unfiltered"),
            "must say the search is unnarrowed: {why}"
        );
    }

    /// Host and invader must produce byte-identical strings or the equality filter never matches.
    #[test]
    fn the_hunted_value_is_spelled_exactly_as_the_published_one() {
        let block = BlockKey::from_raw(THERE);
        assert_eq!(
            hunt_filter_value(true, &[THERE], None),
            Some(map_value(block))
        );
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
