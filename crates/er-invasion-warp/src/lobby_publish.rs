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
//! PUBLISHING installs ONE read-only observer, on `SetLobbyData`, and alters nothing it sees. It
//! exists because the lobby id cannot be derived: the session struct's lobby field and the lobby
//! Seamless actually advertises on are DIFFERENT objects (captured live -- see
//! `SESSION_LOBBY_ID_OFFSET`), and three independent guards (ownership, the `lobby_type` marker,
//! and a read-back of the written value) all passed while describing the wrong one. Watching
//! Seamless declare its own advertisement is definitional rather than inferential.
//!
//! HUNT MODE needs one, because a filter has to be added between Seamless's last
//! `AddRequestLobbyList*` and the request going out, and only Seamless knows when that is. So
//! `RequestLobbyList` is detoured -- in `steamclient64.dll`'s interface vtable, NOT in `ersc.dll`.
//! That distinction is the point: ersc is Themida-protected and an inline patch there is
//! unproven-safe, whereas Steam's own interface is ordinary code this DLL already calls into.

use er_invasion_warp_core::invasion_warp::BlockKey;

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
/// NO LONGER USED FOR PUBLISHING, and kept only because other readers of the session struct still
/// reference it. Its docstring called itself a candidate and predicted exactly how it would fail;
/// it then failed that way. Captured live 2026-08-06, seconds apart:
///
/// ```text
///   ersc SetLobbyData -> 0x186000016d46abe   lobby_type, lobby_key, lobby_preferences, …
///   our publish       -> 0x186000016d0061f   er_invasion_warp_map
/// ```
///
/// Seamless builds a FRESH advertisement lobby each time the world opens to invaders; this field
/// lags on the previous one. Publishing now watches Seamless declare its advertisement instead of
/// deriving it -- see `live::advertisement_lobby`.
pub const SESSION_LOBBY_ID_OFFSET: usize = 0x178;

/// `ISteamMatchmaking::GetLobbyData` -- vtable slot 19.
///
/// READ ONLY, and that distinction is the whole reason this is allowed to name a Seamless key at
/// all: reading lobby data changes nothing for any other player, whereas writing or filtering on
/// one of their keys changes what everybody matches.
pub const GET_LOBBY_DATA_SLOT: usize = 19;

/// `ISteamMatchmaking::GetLobbyOwner` -- vtable slot 35.
///
/// THE CHECK THAT WAS MISSING. `SetLobbyData` persists only for a lobby's OWNER; a non-owner write
/// returns `true`, is accepted into the local copy, and is then replaced by the server's version.
/// Measured 2026-08-06 in one session, same code, two lobbies:
///
/// ```text
///   0x186000016b32b26  owner == us          -> er_invasion_warp_map = "m35_00_00_00"  persisted
///   0x18600001692080e  owner == 0x…12fd097f6 -> er_invasion_warp_map = ""             evaporated
/// ```
///
/// [`ADVERTISEMENT_MARKER_KEY`] cannot catch this: the second lobby carried the marker, because it
/// was a genuine host advertisement -- someone else's, the host being searched against.
pub const GET_LOBBY_OWNER_SLOT: usize = 35;

/// `ISteamUser::GetSteamID` -- vtable slot 2, for the other half of the ownership comparison.
pub const USER_GET_STEAM_ID_SLOT: usize = 2;

/// The exported accessor for `ISteamUser`.
///
/// Version-suffixed and resolved by name like the matchmaking one. `v021` is what this build
/// exports; a miss returns `None` and publishing refuses rather than assuming we are the owner.
pub const USER_ACCESSOR: &str = "SteamAPI_SteamUser_v021\0";

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
///
/// # Exclusions are honoured here too
///
/// `excluded_blocks` is consulted for the same reason [`LocalInvasionConfig::judge`] checks it
/// first: an exclusion is the strongest thing the user can say about a place. Without this, hunt
/// read only the marked list and could aim the Steam query straight at a location the reject
/// filter was standing by to cancel -- the two halves steering to opposite places while sharing
/// one config snapshot. That is worse than either half alone, because the query would succeed and
/// every match it returned would then be thrown away.
#[must_use]
pub fn hunt_filter_value(
    enabled: bool,
    marked_blocks: &[u32],
    excluded_blocks: &[u32],
    current: Option<BlockKey>,
) -> Option<String> {
    if !enabled {
        return None;
    }
    let wanted: Vec<u32> = marked_blocks
        .iter()
        .copied()
        .filter(|block| !excluded_blocks.contains(block))
        .collect();
    match wanted.as_slice() {
        [] => current
            .filter(|here| !excluded_blocks.contains(&here.raw()))
            .map(map_value),
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
    excluded_blocks: &[u32],
    current: Option<BlockKey>,
) -> Option<&'static str> {
    if !enabled {
        return None;
    }
    // Same exclusion pass as `hunt_filter_value`, so the refusal explains the set that function
    // actually acted on rather than the raw marked list.
    let wanted: Vec<u32> = marked_blocks
        .iter()
        .copied()
        .filter(|block| !excluded_blocks.contains(block))
        .collect();
    match wanted.as_slice() {
        [] if marked_blocks.is_empty()
            && current.is_some_and(|here| excluded_blocks.contains(&here.raw())) =>
        {
            Some(
                "hunt: the location you are standing in is EXCLUDED, and nothing else is marked -- \
                 there is no place to ask Steam for. Searching unfiltered; mark somewhere with \
                 Insert, or un-exclude here",
            )
        }
        [] if !marked_blocks.is_empty() => Some(
            "hunt: every marked location is also excluded, so there is nothing left to ask Steam \
             for -- searching unfiltered. An exclusion beats a mark",
        ),
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
        GET_LOBBY_DATA_SLOT, GET_LOBBY_OWNER_SLOT, LOBBY_MAP_KEY, MATCHMAKING_ACCESSOR,
        REQUEST_LOBBY_LIST_SLOT, SET_LOBBY_DATA_SLOT, USER_ACCESSOR, USER_GET_STEAM_ID_SLOT,
        hunt_filter_value, hunt_refusal, pending_publish,
    };
    use er_invasion_warp_core::invasion_warp::BlockKey;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

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
    /// `CSteamID GetLobbyOwner(this, CSteamID lobby)` -- returned through a HIDDEN SRET POINTER.
    ///
    /// The 8-byte return does not come back in a register here; the caller passes a slot for it.
    /// Declaring it `-> u64` puts our lobby argument where the sret pointer belongs and the callee
    /// writes through it: measured as an access violation on `0xbeef`.
    type GetLobbyOwnerFn = unsafe extern "system" fn(usize, *mut u64, u64) -> usize;
    /// `CSteamID GetSteamID(this)` on `ISteamUser` -- same sret convention.
    type GetSteamIdFn = unsafe extern "system" fn(usize, *mut u64) -> usize;
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
        let raw = unsafe { er_invasion_warp_core::warp::current_block_id(base) }?;
        Some(BlockKey::from_raw(raw))
    }

    /// Do WE own this lobby? Only an owner's `SetLobbyData` survives the server.
    ///
    /// `None` means the question could not be answered -- an unresolvable interface, a missing
    /// export, a vtable that would not read. The caller must treat that as "not ours", because
    /// assuming ownership is how the silent-write bug happened in the first place.
    fn we_own(iface: usize, lobby: u64) -> Option<bool> {
        let owner = lobby_owner(iface, lobby)?;
        let me = local_steam_id()?;
        Some(owner != 0 && owner == me)
    }

    /// `GetLobbyOwner`, called through the hidden-return-pointer convention.
    ///
    /// An 8-byte `CSteamID` return is NOT a plain integer return in this build: it goes through an
    /// sret pointer, the same shape `GetLobbyByIndex` needs. Calling it as `-> u64` put a sentinel
    /// in the argument slot and faulted on `0xbeef`.
    fn lobby_owner(iface: usize, lobby: u64) -> Option<u64> {
        let vtable = unsafe { er_game_base::mem::safe_read_usize(iface) }?;
        let slot = unsafe {
            er_game_base::mem::safe_read_usize(vtable + GET_LOBBY_OWNER_SLOT * size_of::<usize>())
        }?;
        if slot == 0 {
            return None;
        }
        let get = unsafe { core::mem::transmute::<usize, GetLobbyOwnerFn>(slot) };
        let mut out: u64 = 0;
        unsafe { get(iface, &raw mut out, lobby) };
        Some(out)
    }

    /// This client's own `CSteamID`, via `ISteamUser::GetSteamID` -- same sret convention.
    fn local_steam_id() -> Option<u64> {
        let module = unsafe { GetModuleHandleA(c"steam_api64.dll".as_ptr().cast()) };
        if module == 0 {
            return None;
        }
        let accessor = unsafe { GetProcAddress(module, USER_ACCESSOR.as_ptr()) };
        if accessor == 0 {
            return None;
        }
        let user = unsafe { core::mem::transmute::<usize, MatchmakingAccessor>(accessor)() };
        if user == 0 {
            return None;
        }
        let vtable = unsafe { er_game_base::mem::safe_read_usize(user) }?;
        let slot = unsafe {
            er_game_base::mem::safe_read_usize(vtable + USER_GET_STEAM_ID_SLOT * size_of::<usize>())
        }?;
        if slot == 0 {
            return None;
        }
        let get = unsafe { core::mem::transmute::<usize, GetSteamIdFn>(slot) };
        let mut out: u64 = 0;
        unsafe { get(user, &raw mut out) };
        (out != 0).then_some(out)
    }

    /// Read our key back off the lobby, so a publish is only counted when the value is THERE.
    ///
    /// `SetLobbyData` returning `true` proved nothing: the non-owner write returned true and
    /// vanished. This is the direct measurement of the effect rather than the call.
    fn published_value(iface: usize, lobby: u64) -> Option<String> {
        let read = get_lobby_data(iface)?;
        let key = format!("{LOBBY_MAP_KEY}\0");
        let got = unsafe { read(iface, lobby, key.as_ptr()) };
        // Steam's RETURN is a foreign pointer for exactly the same reason its arguments are, and
        // `is_null` is exactly the guard that let `0x011000010e05acda` into `strlen` on the
        // argument side of this file. Documented to be `""` on a missing key is not the same as
        // observed to be a valid pointer, and ersc.dll sits between us and Steam here.
        let value =
            unsafe { er_game_base::mem::safe_read_cstr(got as usize, MAX_LOBBY_VALUE_LEN) }?;
        String::from_utf8(value).ok()
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
        // Same foreign-pointer rule as `published_value` above: fault-safe read, not a null check.
        let value = unsafe { er_game_base::mem::safe_read_cstr(got as usize, MAX_LOBBY_VALUE_LEN) };
        value.as_deref() == Some(ADVERTISEMENT_MARKER_VALUE.as_bytes())
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
        // OBSERVED, not derived. The session struct's lobby field pointed at a different lobby
        // from the one Seamless advertises on -- captured live, two ids seconds apart -- so our
        // key was never on the lobby invaders query. `None` here means Seamless has not declared
        // an advertisement yet, which is the ordinary state until the world is opened.
        let Some(lobby) = advertisement_lobby() else {
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
        // OWNERSHIP, before anything is written. Only the owner's lobby data survives the server,
        // and while an invader is searching this session field holds the HOST's lobby -- writing
        // there returns true and evaporates.
        if we_own(iface, lobby) != Some(true) {
            if REFUSALS.fetch_add(1, Ordering::SeqCst) == 0 {
                crate::standalone_log(format_args!(
                    "lobby-publish: REFUSED -- lobby {lobby:#x} is not ours (owner \
                     {:#x}, we are {:#x}). Only a lobby's owner can publish to it; writing here \
                     would return success and be discarded by the server. This is the ordinary \
                     state while SEARCHING -- that field holds the host's lobby, not ours.",
                    lobby_owner(iface, lobby).unwrap_or(0),
                    local_steam_id().unwrap_or(0),
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
        // Our own write goes through the same vtable slot we observe, so silence the observer for
        // its duration -- otherwise publishing could be mistaken for Seamless declaring a lobby.
        IN_OUR_OWN_WRITE.store(true, Ordering::SeqCst);
        let ok = unsafe { write(iface, lobby, key.as_ptr(), payload.as_ptr()) };
        IN_OUR_OWN_WRITE.store(false, Ordering::SeqCst);
        if !ok {
            // Steam refused. Do NOT record it as published, or a transient failure would be
            // remembered as success and never retried.
            REFUSALS.fetch_add(1, Ordering::SeqCst);
            return;
        }
        // READ IT BACK. `ok` is what the call SAID; this is what the lobby HAS. The non-owner
        // write said true and left nothing behind, so the return value alone is not evidence and
        // a counter built on it reports publishes that never happened.
        match published_value(iface, lobby) {
            Some(ref got) if got == &value => {
                *LAST_PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()) = Some(value.clone());
                let n = PUBLISHES.fetch_add(1, Ordering::SeqCst) + 1;
                crate::standalone_log(format_args!(
                    "lobby-publish: {LOBBY_MAP_KEY} = {value} on lobby {lobby:#x} (#{n}, read back)"
                ));
            }
            other => {
                REFUSALS.fetch_add(1, Ordering::SeqCst);
                crate::standalone_log(format_args!(
                    "lobby-publish: REFUSED -- wrote {LOBBY_MAP_KEY} = {value} to lobby \
                     {lobby:#x} and Steam accepted it, but reading it back gives {other:?}. The \
                     write did not stick; this host is NOT findable by location."
                ));
            }
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

    // ---------------------------------------------------------------------------------------
    // WHICH LOBBY IS THE ADVERTISEMENT: observed, not derived
    // ---------------------------------------------------------------------------------------

    /// The lobby id Seamless last wrote its own `lobby_type` marker to.
    ///
    /// # Why this replaced a struct offset
    ///
    /// `SESSION_LOBBY_ID_OFFSET` was a guess that its own docstring flagged as a guess, and it was
    /// wrong. Captured live 2026-08-06, in the same seconds:
    ///
    /// ```text
    ///   ersc SetLobbyData -> 0x186000016d46abe   lobby_type, lobby_key, lobby_preferences, …
    ///   our publish       -> 0x186000016d0061f   er_invasion_warp_map
    /// ```
    ///
    /// Two different lobbies. Seamless builds a FRESH advertisement lobby every time the world is
    /// opened to invaders, while the session field still points at the previous one -- so our key
    /// has never been on the lobby an invader queries, in any run.
    ///
    /// Neither guard could catch it. The owner check passes because we own both. The
    /// [`ADVERTISEMENT_MARKER_KEY`] check passes because both carry `lobby_type`. Even the
    /// read-back passes: it proves the value reached *a* lobby, not the right one. Three
    /// independent confirmations, all green, all describing the wrong object.
    ///
    /// So this stops deriving the answer and OBSERVES it. Seamless itself declares which lobby is
    /// the advertisement, by writing the marker to it; watching that write is definitional rather
    /// than inferential, and it needs no offset, no ownership reasoning, and no candidate probing.
    static ADVERTISEMENT_LOBBY: AtomicU64 = AtomicU64::new(0);
    /// Suppresses our own `SetLobbyData` calls in the observer, so publishing our key can never be
    /// mistaken for Seamless declaring an advertisement.
    static IN_OUR_OWN_WRITE: AtomicBool = AtomicBool::new(false);
    static ORIG_SET_LOBBY_DATA: AtomicUsize = AtomicUsize::new(0);
    static ORIG_ADD_STRING_FILTER: AtomicUsize = AtomicUsize::new(0);
    /// What Seamless computed before any substitution, so the toggle can be undone as well as
    /// applied. Seamless never republishes it, so this is the only copy that exists.
    static VANILLA_LOBBY_KEY: Mutex<Option<String>> = Mutex::new(None);
    /// Which pool the CURRENTLY ADVERTISED lobby is in, as opposed to which one the config asks
    /// for. A difference between the two is what `reapply_pool_if_toggled` exists to close.
    static POOL_APPLIED: AtomicBool = AtomicBool::new(false);
    static FILTER_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
    /// Set once the pool swap has been reported, so the log says it one time rather than on every
    /// publish and every query.
    static POOL_ANNOUNCED: AtomicBool = AtomicBool::new(false);
    static SET_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
    /// One decline explanation per process; the retry itself is every tick.
    static MATCHMAKING_DECLINE_LOGGED: AtomicBool = AtomicBool::new(false);
    static SET_HOOK_LIVE: AtomicUsize = AtomicUsize::new(0);

    /// Watch Seamless declare its advertisement lobby. Observation only: every call is passed
    /// straight through, nothing is altered, and our own writes are ignored.
    unsafe extern "system" fn set_lobby_data_hook(
        iface: usize,
        lobby: usize,
        key: usize,
        value: usize,
    ) -> usize {
        // Same rule as `pooled_key_for`: these pointers are Seamless's, so they are read through
        // `safe_read_cstr` rather than `CStr::from_ptr`. This hook has never been seen to crash,
        // but it takes the identical `key`/`value` pair from the identical caller as the hook
        // that did, so leaving one of the pair unguarded would just be waiting for the other
        // half of the same bug.
        if !IN_OUR_OWN_WRITE.load(Ordering::SeqCst) {
            let key_bytes = unsafe { er_game_base::mem::safe_read_cstr(key, MAX_LOBBY_KEY_LEN) };
            if key_bytes.as_deref()
                == Some(ADVERTISEMENT_MARKER_KEY.trim_end_matches('\0').as_bytes())
            {
                let value_bytes =
                    unsafe { er_game_base::mem::safe_read_cstr(value, MAX_LOBBY_VALUE_LEN) };
                if value_bytes.as_deref() == Some(ADVERTISEMENT_MARKER_VALUE.as_bytes()) {
                    let lobby = lobby as u64;
                    let previous = ADVERTISEMENT_LOBBY.swap(lobby, Ordering::SeqCst);
                    if previous != lobby {
                        // A NEW advertisement lobby means whatever we published before is on a
                        // dead one. Clearing this forces a republish rather than leaving the new
                        // lobby bare because the map has not changed since.
                        *LAST_PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()) = None;
                        crate::standalone_log(format_args!(
                            "lobby-publish: Seamless declared its advertisement lobby {lobby:#x} \
                             (was {previous:#x}); republishing there"
                        ));
                    }
                }
            }
        }
        // Move this player into the DLL's own matchmaking pool, if they asked for that. Seamless
        // publishes its `lobby_key` through this exact call, so substituting here is the publish
        // half of the separation; `add_string_filter_hook` is the search half, and both go through
        // `pooled_key` so they can never disagree about which pool we are in.
        let substituted = pooled_key_for(key, value);
        let value = substituted.as_ref().map_or(value, |s| s.as_ptr() as usize);

        let orig = ORIG_SET_LOBBY_DATA.load(Ordering::SeqCst);
        if orig == 0 {
            return 0;
        }
        unsafe { core::mem::transmute::<usize, er_hook::UnionFn>(orig)(iface, lobby, key, value) }
    }

    /// Seamless's own key for the value that decides which pool a lobby belongs to.
    pub const LOBBY_KEY_NAME: &str = "lobby_key";

    /// Steam's own ceiling on a lobby-data KEY (`k_nMaxLobbyKeyLength`). Used as the read bound
    /// in [`pooled_key_for`] so a junk pointer that happens to land in mapped memory cannot walk
    /// the process looking for a NUL that was never there.
    const MAX_LOBBY_KEY_LEN: usize = 255;

    /// Steam's ceiling on a lobby-data VALUE (`k_cubChatMetadataMax`). Same purpose as
    /// [`MAX_LOBBY_KEY_LEN`]; the real `lobby_key` value is 16 characters, so this is a bound
    /// rather than an expectation.
    const MAX_LOBBY_VALUE_LEN: usize = 8192;

    /// If this call carries Seamless's `lobby_key` and the user opted into the DLL pool, the
    /// replacement to pass instead.
    ///
    /// Returns an owned NUL-terminated buffer that the caller must keep alive across the call —
    /// Steam copies the string during it, but not before.
    fn pooled_key_for(key: usize, value: usize) -> Option<std::ffi::CString> {
        // These two pointers come from Seamless/Steam, not from us, and a NULL check is not
        // enough to make them safe to hand to `CStr::from_ptr`. Both of the crashes reported on
        // 2026-08-23 were a garbage NON-NULL `key` (`0x011000010e05acda` and
        // `0x0110000107be5e2c`) walking into `strlen` and taking the game down from inside this
        // exact call -- see bd `ersc-steam-garbage-key-ptr-crashes-lobby-publish-2026-08-24`.
        // `safe_read_cstr` reads through `ReadProcessMemory`, which fails closed on an unmapped
        // page instead of faulting, and refuses a run with no terminator in range. It rejects
        // null itself, so the old `key == 0 || value == 0` guard is gone rather than duplicated.
        let key_bytes = unsafe { er_game_base::mem::safe_read_cstr(key, MAX_LOBBY_KEY_LEN) }?;
        if key_bytes != LOBBY_KEY_NAME.as_bytes() {
            return None;
        }
        let value_bytes = unsafe { er_game_base::mem::safe_read_cstr(value, MAX_LOBBY_VALUE_LEN) }?;
        let original = std::str::from_utf8(&value_bytes).ok()?;
        // Remember what Seamless computed, so the toggle can be applied or undone on a lobby that
        // already exists. Seamless publishes its advertisement ONCE at CreateLobby and never again,
        // so without a recorded original there is nothing to convert back to and no way to move an
        // existing lobby into the pool -- see `reapply_pool_if_toggled`.
        if !original.is_empty() {
            let mut remembered = VANILLA_LOBBY_KEY.lock().unwrap_or_else(|e| e.into_inner());
            if remembered.as_deref() != Some(original) {
                *remembered = Some(original.to_owned());
            }
        }
        let config = crate::local_invasion_filter::current_config_snapshot()?;
        let pooled =
            er_invasion_warp_core::lobby_pool::pooled_lobby_key(config.dll_users_only, original)?;
        if !POOL_ANNOUNCED.swap(true, Ordering::SeqCst) {
            crate::standalone_log(format_args!(
                "lobby-pool: dll_users_only is ON -- Seamless's match key {} becomes {}. You will \
                 only meet players running this DLL with the same option, for hosting AND \
                 invading; the vanilla population is invisible in both directions.",
                &original[..original.len().min(12)],
                &pooled[..pooled.len().min(12)],
            ));
        }
        std::ffi::CString::new(pooled).ok()
    }

    /// Move an EXISTING advertisement into or out of the DLL pool when the option is toggled.
    ///
    /// # Why the toggle needs this at all
    ///
    /// The two halves of the swap do not have the same reach. The SEARCH half is live: the next
    /// query filters on whatever the config says right now. The PUBLISH half rides Seamless's own
    /// `SetLobbyData`, and Seamless writes its advertisement exactly ONCE, at `CreateLobby` --
    /// measured on a live host session as 7 calls at creation and zero afterwards.
    ///
    /// So without this, flipping the option while already hosting leaves the player SEARCHING in
    /// one pool while still ADVERTISING in the other: invisible to the friend they turned it on
    /// for, and still visible to the strangers they turned it on to avoid. Silently asymmetric,
    /// which is the worst shape a matchmaking switch can take.
    ///
    /// We own the advertisement lobby, so we can write the key ourselves rather than wait for a
    /// lobby that will never be rebuilt. Safe to call every tick: it does nothing unless the
    /// effective value actually differs from what is currently published.
    pub fn reapply_pool_if_toggled() {
        let Some(config) = crate::local_invasion_filter::current_config_snapshot() else {
            return;
        };
        let wanted = config.dll_users_only;
        // Nothing to convert to or from until Seamless has published at least once.
        let vanilla = {
            let guard = VANILLA_LOBBY_KEY.lock().unwrap_or_else(|e| e.into_inner());
            guard.clone()
        };
        let Some(vanilla) = vanilla else {
            return;
        };
        if POOL_APPLIED.load(Ordering::SeqCst) == wanted {
            return;
        }
        let Some(iface) = matchmaking() else {
            return;
        };
        let Some(lobby) = advertisement_lobby() else {
            return;
        };
        // Only the owner's lobby data survives the server; while searching, that field is the
        // HOST's lobby and a write there would return true and evaporate.
        if we_own(iface, lobby) != Some(true) {
            return;
        }
        let Some(write) = set_lobby_data(iface) else {
            return;
        };
        let value = if wanted {
            match er_invasion_warp_core::lobby_pool::pooled_lobby_key(true, &vanilla) {
                Some(pooled) => pooled,
                None => return,
            }
        } else {
            vanilla.clone()
        };
        let key = format!("{LOBBY_KEY_NAME}\0");
        let payload = format!("{value}\0");
        // Our own write goes through the hook that performs the substitution; suppress it so the
        // value is not transformed twice.
        IN_OUR_OWN_WRITE.store(true, Ordering::SeqCst);
        let ok = unsafe { write(iface, lobby, key.as_ptr(), payload.as_ptr()) };
        IN_OUR_OWN_WRITE.store(false, Ordering::SeqCst);
        if !ok {
            return;
        }
        POOL_APPLIED.store(wanted, Ordering::SeqCst);
        crate::standalone_log(format_args!(
            "lobby-pool: dll_users_only turned {} -- re-advertised lobby {lobby:#x} under the {} \
             match key, so the search and the advertisement are in the same pool",
            if wanted { "ON" } else { "OFF" },
            if wanted { "DLL-pool" } else { "vanilla" },
        ));
    }

    /// `ISteamMatchmaking::AddRequestLobbyListStringFilter` — the search half of the pool swap.
    ///
    /// Seamless filters on `lobby_key` here with `k_ELobbyComparisonEqual`, so a substitution makes
    /// the outgoing query ask for our pool instead of the vanilla one.
    unsafe extern "system" fn add_string_filter_hook(
        iface: usize,
        key: usize,
        value: usize,
        comparison: usize,
    ) -> usize {
        let substituted = pooled_key_for(key, value);
        let value = substituted.as_ref().map_or(value, |s| s.as_ptr() as usize);
        let orig = ORIG_ADD_STRING_FILTER.load(Ordering::SeqCst);
        if orig == 0 {
            return 0;
        }
        unsafe {
            core::mem::transmute::<usize, er_hook::UnionFn>(orig)(iface, key, value, comparison)
        }
    }

    /// Install the search-side substitution. Idempotent; retries until Steam is resolvable.
    pub fn install_pool_filter_hook() -> usize {
        if FILTER_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
            return 0;
        }
        let Some(iface) = matchmaking() else {
            FILTER_HOOK_INSTALLED.store(0, Ordering::SeqCst);
            return 0;
        };
        let Some(vtable) = (unsafe { er_game_base::mem::safe_read_usize(iface) }) else {
            FILTER_HOOK_INSTALLED.store(0, Ordering::SeqCst);
            return 0;
        };
        let Some(address) = (unsafe {
            er_game_base::mem::safe_read_usize(vtable + ADD_STRING_FILTER_SLOT * size_of::<usize>())
        }) else {
            FILTER_HOOK_INSTALLED.store(0, Ordering::SeqCst);
            return 0;
        };
        if address == 0 {
            FILTER_HOOK_INSTALLED.store(0, Ordering::SeqCst);
            return 0;
        }
        match unsafe {
            er_hook::register_union_hook(
                address,
                add_string_filter_hook as er_hook::UnionFn,
                &ORIG_ADD_STRING_FILTER,
            )
        } {
            Ok(()) => {
                crate::standalone_log(format_args!(
                    "lobby-pool: hooked AddRequestLobbyListStringFilter at {address:#x}"
                ));
                1
            }
            Err(status) => {
                crate::standalone_log(format_args!(
                    "lobby-pool: could not hook AddRequestLobbyListStringFilter: {status:?}"
                ));
                0
            }
        }
    }

    /// Install the advertisement observer. Idempotent; retries until Steam is resolvable.
    pub fn install_advertisement_observer() -> usize {
        if SET_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
            return 0;
        }
        let Some(iface) = matchmaking() else {
            // SAY SO, ONCE. This path returns silently and retries every tick, which makes the
            // absence of an install line ambiguous: it reads identically to `steam_hooks = false`.
            // Measured 2026-09-04, that ambiguity cost a whole A/B arm -- a 110s run with
            // `steam_hooks = true` produced no install line, and only reading this function showed
            // the three detours had never armed at all, so the arm re-tested `map_pins` and said
            // nothing about Steam. A declined install that explains itself is the difference
            // between a null result and a wasted run.
            if !MATCHMAKING_DECLINE_LOGGED.swap(true, Ordering::SeqCst) {
                crate::standalone_log(format_args!(
                    "lobby-publish: the three Steam detours are NOT installed -- \
                     ISteamMatchmaking is not resolvable yet (steam_api64.dll absent, or its \
                     accessor has not returned an interface). This is a retry, not a refusal: it \
                     is attempted again every tick. `steam_hooks` is ON; if it were OFF this line \
                     would not appear at all, which is how to tell the two apart."
                ));
            }
            SET_HOOK_INSTALLED.store(0, Ordering::SeqCst);
            return 0;
        };
        let Some(vtable) = (unsafe { er_game_base::mem::safe_read_usize(iface) }) else {
            SET_HOOK_INSTALLED.store(0, Ordering::SeqCst);
            return 0;
        };
        let Some(address) = (unsafe {
            er_game_base::mem::safe_read_usize(vtable + SET_LOBBY_DATA_SLOT * size_of::<usize>())
        }) else {
            SET_HOOK_INSTALLED.store(0, Ordering::SeqCst);
            return 0;
        };
        match unsafe {
            er_hook::register_union_hook(
                address,
                set_lobby_data_hook as er_hook::UnionFn,
                &ORIG_SET_LOBBY_DATA,
            )
        } {
            Ok(()) => {
                SET_HOOK_LIVE.store(1, Ordering::SeqCst);
                1
            }
            Err(status) => {
                crate::standalone_log(format_args!(
                    "lobby-publish: could not observe SetLobbyData: {status:?} -- the advertisement \
                     lobby cannot be identified, so publishing will REFUSE rather than guess"
                ));
                0
            }
        }
    }

    /// The advertisement lobby, or `None` if Seamless has not declared one this session.
    ///
    /// `None` is a refusal, never a fallback to the session field: guessing is what put our key on
    /// the wrong lobby for every run before this.
    #[must_use]
    pub fn advertisement_lobby() -> Option<u64> {
        let lobby = ADVERTISEMENT_LOBBY.load(Ordering::SeqCst);
        (lobby != 0).then_some(lobby)
    }

    /// Trampoline to Steam's own `RequestLobbyList`.
    static ORIG_REQUEST_LOBBY_LIST: AtomicUsize = AtomicUsize::new(0);
    /// "An install has been ATTEMPTED and should not be attempted again", not "it worked". Cleared
    /// on the transient failures so the next tick retries; deliberately left set when the hook
    /// itself was refused, because that will not get better by trying every frame.
    static HUNT_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
    /// "The detour is LIVE." Separate from the attempt flag because the oracle must not report a
    /// failed install as a hooked run -- that is precisely the silent failure hunt mode can have.
    static HUNT_HOOK_LIVE: AtomicUsize = AtomicUsize::new(0);
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
        if let Some(value) = hunt_target()
            && let Some(add) = add_string_filter(iface)
        {
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
        // Exclusions bind hunt as well as the reject filter. Reading only the marked list let the
        // two halves aim at opposite places while sharing one config snapshot.
        let excluded: Vec<u32> = config.blocked_blocks.iter().copied().collect();
        if let Some(why) = hunt_refusal(config.hunt, &marked, &excluded, current_block()) {
            if HUNT_REFUSAL_SAID.swap(1, Ordering::SeqCst) == 0 {
                crate::standalone_log(format_args!("{why}"));
            }
            return None;
        }
        hunt_filter_value(config.hunt, &marked, &excluded, current_block())
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
            // Transient: the interface pointer is real but the vtable is not readable yet. Clear
            // the attempt flag or hunt mode would be permanently inert after one unlucky tick.
            HUNT_HOOK_INSTALLED.store(0, Ordering::SeqCst);
            return 0;
        };
        let Some(address) = (unsafe {
            er_game_base::mem::safe_read_usize(
                vtable + REQUEST_LOBBY_LIST_SLOT * size_of::<usize>(),
            )
        }) else {
            HUNT_HOOK_INSTALLED.store(0, Ordering::SeqCst);
            return 0;
        };
        match unsafe {
            er_hook::register_union_hook(
                address,
                request_lobby_list_hook as er_hook::UnionFn,
                &ORIG_REQUEST_LOBBY_LIST,
            )
        } {
            Ok(()) => {
                HUNT_HOOK_LIVE.store(1, Ordering::SeqCst);
                1
            }
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

    /// `(the detour is live, queries we narrowed)` -- the invader half of the same judgement.
    #[must_use]
    pub fn hunt_tally() -> (bool, usize) {
        (
            HUNT_HOOK_LIVE.load(Ordering::SeqCst) != 0,
            FILTERS_ADDED.load(Ordering::SeqCst),
        )
    }
}

#[cfg(windows)]
pub use live::{
    advertisement_lobby, hunt_tally, install_advertisement_observer, install_hunt_hook,
    install_pool_filter_hook, publish_current_map, reapply_pool_if_toggled, tally,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunt_never_aims_at_a_location_the_user_excluded() {
        // The two halves share one config snapshot, so they must not steer to opposite places.
        // Hunt read only the marked list, so an excluded-but-marked block aimed the Steam query at
        // exactly the destination the reject filter was standing by to cancel: the query succeeds
        // and every match it returns is thrown away, which is worse than either half alone.
        let block = 0x0f00_0000;
        assert_eq!(hunt_filter_value(true, &[block], &[block], None), None);
    }

    #[test]
    fn hunt_does_not_ask_for_the_block_the_player_is_standing_in_when_it_is_excluded() {
        // The no-marks case falls back to "where I am", which must respect an exclusion the same
        // way -- otherwise standing in an excluded place silently re-enables it.
        let here = BlockKey::from_parts(60, 51, 36, 0);
        assert_eq!(
            hunt_filter_value(true, &[], &[here.raw()], Some(here)),
            None
        );
        // Un-excluded, the same input DOES hunt -- so the guard is the exclusion, not the fallback.
        assert_eq!(
            hunt_filter_value(true, &[], &[], Some(here)),
            Some("m60_51_36_00".to_string())
        );
    }

    #[test]
    fn one_mark_surviving_exclusion_is_still_a_single_unambiguous_target() {
        // Two marks would refuse (no OR in a Steam filter), but if an exclusion removes one of
        // them the intent is unambiguous again and hunt should proceed rather than refuse.
        let keep = 0x0f00_0000;
        let drop = 0x3c35_3800;
        assert_eq!(
            hunt_filter_value(true, &[keep, drop], &[drop], None),
            Some(map_value(BlockKey::from_raw(keep)))
        );
    }

    #[test]
    fn every_mark_being_excluded_is_explained_rather_than_silently_unfiltered() {
        let a = 0x0f00_0000;
        let b = 0x3c35_3800;
        let why = hunt_refusal(true, &[a, b], &[a, b], None).expect("must explain");
        assert!(
            why.contains("excluded"),
            "the refusal has to name the cause: {why}"
        );
        assert_eq!(hunt_filter_value(true, &[a, b], &[a, b], None), None);
    }

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
        // NARROWED 2026-08-06. This used to ban `lobby_key` outright, and that was the right rule
        // until the DLL pool existed: substituting it is now a FEATURE, opt-in, and the whole point
        // of it is to change who can see whom.
        //
        // What must not come back is an UNCONDITIONAL touch. The substitution is legitimate only
        // because `pooled_lobby_key` returns `None` unless the user set `dll_users_only`, so every
        // mention of the key has to route through that one function -- which fails closed, is
        // unit-tested, and is the only place the decision lives. A direct write or filter of
        // `lobby_key` anywhere else would silently move players between pools without asking.
        let key_lines: Vec<&str> = code
            .lines()
            .filter(|line| line.contains("lobby_key") && !line.trim_start().starts_with("//"))
            .collect();
        assert!(
            !key_lines.is_empty(),
            "the pool substitution names lobby_key; if it no longer does, this guard is stale"
        );
        for line in &key_lines {
            let routed = line.contains("LOBBY_KEY_NAME")
                || line.contains("pooled_lobby_key")
                || line.contains("pooled_key_for");
            assert!(
                routed,
                "lobby_key may only be touched through the opt-in pool path, which fails closed \
                 when the user has not asked for it: {line}"
            );
        }
        assert!(
            code.contains("config.dll_users_only"),
            "the substitution must be gated on the user's own opt-in, never unconditional"
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
        // TWO write sites, and the count is pinned so a third has to be justified rather than
        // appear: `publish_current_map` writes our own map key, and `reapply_pool_if_toggled`
        // re-advertises the match key when the user flips `dll_users_only` on an existing lobby.
        // The second exists because Seamless publishes its advertisement once at CreateLobby and
        // never again, so a toggle would otherwise leave the player searching in one pool while
        // still advertising in the other.
        assert_eq!(
            writes.len(),
            2,
            "expected the map-key publish and the pool re-advertise, found: {writes:?}"
        );
        assert!(
            code.contains("fn reapply_pool_if_toggled"),
            "the second write site must be the toggle re-advertise, and it must stay gated on \
             ownership and on the user's own opt-in"
        );
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
        assert_eq!(hunt_filter_value(false, &[], &[], Some(standing)), None);
        assert_eq!(
            hunt_filter_value(false, &[THERE], &[], Some(standing)),
            None
        );
        assert_eq!(
            hunt_refusal(false, &[THERE, HERE], &[], Some(standing)),
            None
        );
    }

    #[test]
    fn with_nothing_marked_hunt_asks_for_where_you_are_standing() {
        let standing = BlockKey::from_raw(HERE);
        assert_eq!(
            hunt_filter_value(true, &[], &[], Some(standing)),
            Some("m60_51_43_00".to_owned())
        );
    }

    #[test]
    fn one_marked_location_is_the_one_asked_for() {
        let standing = BlockKey::from_raw(HERE);
        // The MARK wins over where you stand -- marking a place is the user naming a destination.
        assert_eq!(
            hunt_filter_value(true, &[THERE], &[], Some(standing)),
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
            hunt_filter_value(true, &[HERE, THERE], &[], Some(standing)),
            None
        );
        let why = hunt_refusal(true, &[HERE, THERE], &[], Some(standing)).expect("explains itself");
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
        assert_eq!(hunt_filter_value(true, &[], &[], None), None);
        let why = hunt_refusal(true, &[], &[], None).expect("explains itself");
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
            hunt_filter_value(true, &[THERE], &[], None),
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
