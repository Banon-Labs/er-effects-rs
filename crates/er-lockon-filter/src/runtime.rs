//! The Windows half: install the detour, answer the lock-on system's ownership query, and write
//! down enough about what it asked to tell a filter that did nothing from one that had nothing
//! to do.
//!
//! # Why the log carries a census
//!
//! This feature can only be exercised by two players invading the same world, which is not a
//! state any offline check can produce. So the hook records the distinct `ChrType` values it was
//! asked about -- yours and each candidate's -- the first time it sees each one. That turns "the
//! filter never fired" into a readable answer: either your kind or the other invader's was not a
//! hostile phantom, and the log says which number turned up instead. Seamless Co-op is why that
//! is worth writing down rather than assuming: it runs its own session layer, and a roster walk
//! in an ordinary Seamless session has been measured typing remote players `Local` (0), a kind
//! the vanilla enum reserves for the player at the keyboard.
//!
//! The candidate half of that census speaks only for candidates that are other players. The first
//! version did not, and it was worse than useless: standing still in an ordinary world, with
//! nobody invading, it reported the `ChrType` and team byte of every kind of enemy that wandered
//! into lock-on range. Those lines cannot answer a question about two humans, and each one arrived
//! as a notification. [`is_player_ins`] is the gate, and it gates the rule too -- see [`hides`]
//! for why a non-player is never hidden however it is typed.
//!
//! The census has already paid for itself once. A run on 2026-09-07 recorded the local player at
//! `chr_type` 2 with `summonParamType` -12, and the rule at the time held neither value, so the
//! filter could not arm however many invaders were standing in the world. Those two log lines are
//! what the widened rule in [`crate::rules`] was derived against.

use std::ffi::c_void;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, Once};

use er_game_base::log::append_line;
use er_game_base::mem::{game_module_base, is_heap_aligned_ptr, read_bytes, vtable_in_game_image};
use er_game_base::prologue::{compared_mismatches, matches_masked};
use er_game_base::rva::{
    GAME_MAN_SINGLETON_RVA, WORLD_CHR_MAN_GLOBAL_RVA, WORLD_CHR_MAN_PLAYER_INS_OFFSET,
};
use er_hook::UnionFn;

use crate::rules::{
    HOSTILE_PHANTOM_MULTIPLAY_ROLES, HOSTILE_PHANTOM_SUMMON_PARAM_TYPES, HOSTILE_PHANTOMS,
    MAX_CHR_TYPE, SUMMON_PARAM_TYPE_UNKNOWN, hides, local_player_is_invading,
};
use crate::{
    CHR_INS_CHR_TYPE_OFFSET, CHR_INS_TEAM_TYPE_OFFSET, GAME_MAN_SUMMON_PARAM_TYPE_OFFSET,
    LOCK_ON_POINT_OWNER_PROLOGUE, LOCK_ON_POINT_OWNER_PROLOGUE_MASK, LOCK_ON_POINT_OWNER_RVA,
    LOG_FILE_NAME,
};

const DLL_PROCESS_ATTACH: u32 = 1;
/// The value an `original` slot holds before the trampoline is published.
const ORIGINAL_UNSET: usize = 0;
/// Big enough for the one prologue this DLL byte-checks.
const MAX_PROLOGUE_BYTES: usize = 16;
/// How many hidden candidates are logged in full before the line rate-limits.
const SUPPRESSION_LOG_LIMIT: u64 = 8;
/// After the first few, one line per this many, so a long invasion shows the filter still working
/// without the log growing to the size of the run.
const SUPPRESSION_LOG_INTERVAL: u64 = 4096;

static START: Once = Once::new();
static GAME_BASE: AtomicUsize = AtomicUsize::new(0);
/// Absolute address of the `WorldChrMan` singleton pointer on the running build, resolved once at
/// install so the hook does no address translation on the game thread.
static WORLD_CHR_MAN_GLOBAL: AtomicUsize = AtomicUsize::new(0);
/// Absolute address of the `GameMan` singleton pointer, resolved the same way. Zero when the
/// running build has no mapping for it, which stands the summon-param half of the local-player
/// test down without standing the filter down.
static GAME_MAN_GLOBAL: AtomicUsize = AtomicUsize::new(0);
static ORIGINAL_POINT_OWNER: AtomicUsize = AtomicUsize::new(ORIGINAL_UNSET);

static SUPPRESSED: AtomicU64 = AtomicU64::new(0);
/// `ChrType` values seen as the local player, one bit each, so a kind is named once.
static SEEN_SELF_TYPES: AtomicU32 = AtomicU32::new(0);
/// `ChrType` values seen as a lock-on candidate, one bit each.
static SEEN_CANDIDATE_TYPES: AtomicU32 = AtomicU32::new(0);
/// Set once if a `chr_type` outside the representable range ever turns up, which would mean the
/// census is blind to it and the rule can never match it.
static SEEN_UNREPRESENTABLE_TYPE: AtomicBool = AtomicBool::new(false);
/// The `CS::PlayerIns` vtable, taken from the main player the first time it is read.
///
/// Every player in the world is a `PlayerIns` -- the RTTI carries one class for all of them
/// (`.?AVPlayerIns@CS@@`, 1.17.1 `0x143c85de8`), with no separate kind for a remote or a main
/// player -- while a non-player character is a `CS::EnemyIns` (`0x143c84e70`). So the main
/// player's own vtable word identifies the class for the whole session, and the test costs one
/// load and one compare with no address to translate between builds.
static PLAYER_INS_VTABLE: AtomicUsize = AtomicUsize::new(0);
/// Set once when a non-player candidate is first passed over, so the log explains its own
/// quiet rather than reading like a hook that never fired.
static SAID_NON_PLAYERS_ARE_SKIPPED: AtomicBool = AtomicBool::new(false);
/// `(chr_type, team_type)` pairings already reported, one bit per `chr_type << 8 | team`, hashed
/// down to a word. A pairing rather than two independent censuses because the question the team
/// byte is being read for is whether it says something `chr_type` does not: two characters with
/// the same kind and different teams is the answer that matters, and two separate sets cannot
/// express it.
static SEEN_SELF_PAIRINGS: AtomicU64 = AtomicU64::new(0);
static SEEN_CANDIDATE_PAIRINGS: AtomicU64 = AtomicU64::new(0);
/// `SessionManagerPlayerEntry` addresses already written down, for the local player and for
/// candidates alike, so the identity census costs one line per person rather than one per lock-on
/// point. One list across both roles because the question it answers -- which field separates you
/// from a fellow invader -- needs your own row beside theirs.
static SEEN_IDENTITY_ENTRIES: Mutex<Vec<usize>> = Mutex::new(Vec::new());
/// How many distinct candidates the identity census reports before it stops. One double invasion
/// needs three, and a cap keeps a long session from turning the log into a roster.
const IDENTITY_CENSUS_LIMIT: usize = 8;
/// The last `GameMan::summonParamType` the census reported, so the log carries one line per
/// change of multiplayer role rather than one per lock-on point. A bitmask would not do: the
/// roles are negative.
static LAST_SUMMON_PARAM_TYPE: AtomicI32 = AtomicI32::new(SUMMON_PARAM_TYPE_UNKNOWN);

pub(crate) unsafe extern "system" fn dll_main(
    _module: *mut c_void,
    reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        START.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-lockon-filter".to_owned())
                .spawn(install);
        });
    }
    crate::DLL_MAIN_SUCCESS
}

fn install() {
    // A panic in a cdylib loaded into the game is otherwise anonymous: the message goes to a
    // stderr nobody reads and what survives is a 0xe06d7363 record naming the module.
    er_game_base::panic_report::report_panics_to("er-lockon-filter", log_message);
    er_hook::set_hook_logger(log_message);

    let mut attempts = 0_u64;
    // Bounded: an unbounded `loop { yield_now() }` in two other shells starved the wineserver and
    // hung a whole boot. See `er_game_base::wait`.
    let found = er_game_base::wait::poll_until(|| match game_module_base() {
        Ok(base) => Some(base),
        Err(err) => {
            if attempts == 0 || attempts.is_multiple_of(4096) {
                log_message(format_args!("install: waiting for game module base: {err}"));
            }
            attempts = attempts.saturating_add(1);
            None
        }
    });
    let Some(base) = found else {
        log_message(format_args!(
            "install: no game module base; nothing installed"
        ));
        return;
    };
    GAME_BASE.store(base, Ordering::SeqCst);

    let world_chr_man = er_game_base::mem::game_data_addr(
        base,
        WORLD_CHR_MAN_GLOBAL_RVA,
        "WORLD_CHR_MAN_GLOBAL_RVA",
    );
    if world_chr_man == 0 {
        log_message(format_args!(
            "install: the WorldChrMan singleton has no address on this build ({}); without it \
             there is no way to ask who the local player is, so nothing was installed",
            er_game_base::game_build::describe_build()
        ));
        return;
    }
    WORLD_CHR_MAN_GLOBAL.store(world_chr_man, Ordering::SeqCst);

    // The second half of the local-player test, and the one that stands down on its own. Without
    // `GameMan` the filter still works off `ChrType`; refusing to install over it would trade a
    // working feature for a missing corroboration.
    let game_man =
        er_game_base::mem::game_data_addr(base, GAME_MAN_SINGLETON_RVA, "GAME_MAN_SINGLETON_RVA");
    if game_man == 0 {
        log_message(format_args!(
            "install: the GameMan singleton has no address on this build ({}); the multiplayer \
             role cannot be read, so whether you are invading is decided by your ChrType alone",
            er_game_base::game_build::describe_build()
        ));
    }
    GAME_MAN_GLOBAL.store(game_man, Ordering::SeqCst);

    if !prologue_matches(base) {
        return;
    }
    // The unresolved address on purpose: `register_union_hook` resolves it for the running build
    // itself, and handing it an already-resolved one resolves twice -- which is silent, and lands
    // the detour on a third function whenever a region's shift equals the local spacing between
    // two entries. See `scripts/check-double-resolved-hook-targets.py`.
    let target = base + LOCK_ON_POINT_OWNER_RVA;
    match unsafe {
        er_hook::register_union_hook(
            target,
            lock_on_point_owner_hook as UnionFn,
            &ORIGINAL_POINT_OWNER,
        )
    } {
        Ok(()) => log_message(format_args!(
            "install: hooked the lock-on point-owner resolver (1.16.2 rva \
             0x{LOCK_ON_POINT_OWNER_RVA:x}). While you are a hostile phantom -- chr_type one of \
             [{}], or a multiplayer role that resolves to one -- another player is not offered to \
             lock-on if their chr_type is one of the same set or their multiplay role is one of \
             [{}]. The role is read because a Seamless Co-op session types a remote invader the \
             same as the host. No switch, no hotkey, no config: loading this DLL is the feature.",
            HOSTILE_PHANTOMS.describe(),
            HOSTILE_PHANTOM_MULTIPLAY_ROLES
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Err(status) => log_message(format_args!(
            "install: register_union_hook failed: {status:?}; the filter is inert this run"
        )),
    }
}

/// Read the live bytes at the detoured entry and compare them with the generated pin.
///
/// Resolved here and nowhere else. The pin describes the function's opening on both builds, so
/// the address it is read at has to be the running build's, while the address handed to the hook
/// API stays unresolved.
fn prologue_matches(base: usize) -> bool {
    let Some(address) = er_game_base::game_build::resolve_detour_address(
        base + LOCK_ON_POINT_OWNER_RVA,
        "LOCK_ON_POINT_OWNER_RVA",
    ) else {
        log_message(format_args!(
            "REFUSED the lock-on point-owner resolver (1.16.2 rva 0x{LOCK_ON_POINT_OWNER_RVA:x}): \
             no detour-safe mapping for {}. Nothing was read and nothing installed.",
            er_game_base::game_build::describe_build()
        ));
        return false;
    };
    let mut actual = [0_u8; MAX_PROLOGUE_BYTES];
    let window = &mut actual[..LOCK_ON_POINT_OWNER_PROLOGUE.len()];
    if !unsafe { read_bytes(address, window) } {
        log_message(format_args!(
            "DISARMED the lock-on point-owner resolver @0x{address:x}: prologue unreadable"
        ));
        return false;
    }
    if !matches_masked(
        window,
        &LOCK_ON_POINT_OWNER_PROLOGUE,
        &LOCK_ON_POINT_OWNER_PROLOGUE_MASK,
    ) {
        log_message(format_args!(
            "DISARMED the lock-on point-owner resolver @0x{address:x}: {} of {} compared bytes \
             differ (got {window:02x?}, want {:02x?}). The address was already resolved for this \
             build, so the likeliest cause is another mod detouring the same entry first.",
            compared_mismatches(
                window,
                &LOCK_ON_POINT_OWNER_PROLOGUE,
                &LOCK_ON_POINT_OWNER_PROLOGUE_MASK
            ),
            LOCK_ON_POINT_OWNER_PROLOGUE.len(),
            LOCK_ON_POINT_OWNER_PROLOGUE,
        ));
        return false;
    }
    true
}

/// `ChrIns *PointOwner(ActPnt *point)`, filtered.
///
/// The original runs first and its answer is what gets filtered: this hook decides nothing about
/// which handle a point carries, only whether the lock-on system is told about the character
/// behind it. Returning 0 is the original's own answer for a point with no character owner, so
/// every caller takes a branch it already has.
unsafe extern "system" fn lock_on_point_owner_hook(
    point: usize,
    second: usize,
    third: usize,
    fourth: usize,
) -> usize {
    let original = ORIGINAL_POINT_OWNER.load(Ordering::Acquire);
    if original == ORIGINAL_UNSET {
        // Unreachable: the union publishes the trampoline before it enables the hook. If it ever
        // did happen, 0 is the original's own no-owner answer, which is the only value that can
        // be returned without inventing a character.
        return 0;
    }
    // SAFETY: the slot holds a trampoline published by `register_union_hook`, which is the next
    // handler in the chain or the game's own entry, and carries this exact signature.
    let owner =
        unsafe { std::mem::transmute::<usize, UnionFn>(original)(point, second, third, fourth) };
    if owner == 0 {
        return owner;
    }
    let Some(candidate_chr_type) = chr_type_of(owner) else {
        return owner;
    };
    let Some(local_player) = main_player() else {
        return owner;
    };
    if local_player == owner {
        return owner;
    }
    let Some(self_chr_type) = chr_type_of(local_player) else {
        return owner;
    };
    let summon_param_type = summon_param_type();
    note_seen_local_player(self_chr_type, summon_param_type);
    note_team_pairing(
        &SEEN_SELF_PAIRINGS,
        Role::LocalPlayer,
        self_chr_type,
        local_player,
    );
    note_identity(Role::LocalPlayer, local_player, self_chr_type);
    // Everything below this line is about one other human. A non-player candidate is neither
    // filtered nor written down: the rule cannot apply to it, and a census of the enemies standing
    // near you answers nothing the question needs while emitting a line per kind of wildlife.
    let candidate_is_player = is_player_ins(owner, local_player);
    if !candidate_is_player {
        note_non_players_are_skipped();
        return owner;
    }
    note_seen_one(&SEEN_CANDIDATE_TYPES, candidate_chr_type, Role::Candidate);
    note_team_pairing(
        &SEEN_CANDIDATE_PAIRINGS,
        Role::Candidate,
        candidate_chr_type,
        owner,
    );
    note_identity(Role::Candidate, owner, candidate_chr_type);
    let candidate_multiplay_role = multiplay_role_of(owner);
    if !hides(
        self_chr_type,
        summon_param_type,
        candidate_chr_type,
        candidate_multiplay_role,
        candidate_is_player,
    ) {
        return owner;
    }
    note_hidden(
        self_chr_type,
        summon_param_type,
        candidate_chr_type,
        candidate_multiplay_role,
    );
    0
}

/// `PlayerIns+0x6b8` -- this character's `SessionManagerPlayerEntry`, or 0.
///
/// # Why the offset can be read rather than called
///
/// `CS::PlayerIns::GetSessionManagerPlayerEntry` is the whole of `mov rax,[rcx+0x6b8]; ret`, and
/// those eight bytes `48 8b 81 b8 06 00 00 c3` occur exactly once in `eldenring-deobf.bin`
/// (`0x140657b20`) and exactly once in `eldenring-deobf-1.17.1.bin` (`0x140658970`). A unique
/// identical body in both images is what says the constant inside it did not move, so reading the
/// field directly needs no call and no address translation. `er-player-name-filter` pins the same
/// offset.
const PLAYER_INS_SESSION_MANAGER_PLAYER_ENTRY_OFFSET: usize = 0x6b8;
/// `PlayerIns+0x580` -- this character's `PlayerGameData`.
///
/// `CS::PlayerIns::GetPlayerGameData` is the whole of `mov rax,[rcx+0x580]; ret`, and those eight
/// bytes `48 8b 81 80 05 00 00 c3` occur exactly once in `eldenring-deobf.bin` (`0x1406563d0`) and
/// exactly once in `eldenring-deobf-1.17.1.bin` (`0x140657220`). A unique identical body in both
/// images is what says the constant inside it did not move, so the field is read rather than the
/// vtable slot called.
const PLAYER_INS_PLAYER_GAME_DATA_OFFSET: usize = 0x580;
/// `PlayerGameData+229` -- the multiplayer role this person currently holds.
///
/// The live role, not the `preCeremonyMultiplayRole` the session entry carries: the engine's own
/// `CS::PlayerIns::GetMultiplayRole` (1.16.2 `0x140655fd0`) returns exactly this field, and
/// `MultiplayProperties` maps it to the `CharacterType` the engine would derive. It is the
/// per-person answer to the question `chr_type` failed to answer under Seamless Co-op.
const PLAYER_GAME_DATA_MULTIPLAY_ROLE_OFFSET: usize = 229;
/// `PlayerGameData+152` -- the `CharacterType` the session recorded for this person, which need
/// not agree with the `ChrIns::chr_type` the rule reads.
const PLAYER_GAME_DATA_CHR_TYPE_OFFSET: usize = 152;
/// `PlayerGameData+2705` -- the invasion item this person used, if any. A non-zero value names an
/// invader directly.
const PLAYER_GAME_DATA_INVASION_ITEM_TYPE_OFFSET: usize = 2705;
/// `PlayerGameData+2288` -- whether this `PlayerGameData` belongs to the player at the keyboard.
/// `er-player-name-filter` pins the same offset, which is what says this is the same struct.
const PLAYER_GAME_DATA_IS_MAIN_PLAYER_OFFSET: usize = 2288;
/// `SessionManagerPlayerEntry+0x10` -- the Steam ID of the person the entry belongs to.
///
/// The one field in the entry that is certainly about a specific human, which is why the census
/// leads with it. `er-player-name-filter` pins the same layout, and its
/// `session_manager_entry_layout_matches_copy_offsets` test is what holds the two crates together.
const SESSION_ENTRY_STEAM_ID_OFFSET: usize = 0x10;
/// `SessionManagerPlayerEntry+0x18` -- the `DLInplaceStr` holding the Steam persona name.
const SESSION_ENTRY_STEAM_NAME_OFFSET: usize = 0x18;
/// The backing pointer inside a `DLTX::DLString`.
const DL_STRING_BACKING_OFFSET: usize = 0x08;
/// The length, in text units, inside a `DLTX::DLString`.
const DL_STRING_LENGTH_OFFSET: usize = 0x10;
/// The inplace capacity of the entry's name string. A length past it is a string this census has
/// misread rather than a long name, so it is dropped instead of transcribed.
const STEAM_NAME_UTF16_CAPACITY: usize = 64;
/// `SessionManagerPlayerEntry+232` -- whether this player is the host of the session.
const SESSION_ENTRY_IS_HOST_OFFSET: usize = 232;
/// `SessionManagerPlayerEntry+233` -- whether this entry is the local player's own.
const SESSION_ENTRY_IS_LOCAL_PLAYER_OFFSET: usize = 233;
/// `SessionManagerPlayerEntry+252` -- the multiplayer role the session assigned before the
/// ceremony, which `MultiplayProperties` maps to the `ChrType` the engine would derive.
const SESSION_ENTRY_PRE_CEREMONY_ROLE_OFFSET: usize = 252;

/// What the session says one character is, independent of `chr_type`.
struct SessionIdentity {
    entry: usize,
    steam_id: u64,
    name: Option<String>,
    is_host: bool,
    is_local_player: bool,
    pre_ceremony_role: u8,
}

/// What `PlayerGameData` says one character is.
struct GameDataIdentity {
    multiplay_role: u8,
    chr_type: u8,
    invasion_item_type: u8,
    is_main_player: bool,
}

/// `PlayerGameData::multiplayRole` for one character, or `None` when it has no readable
/// `PlayerGameData`.
///
/// Two loads, because this one is on the hook's hot path -- once per lock-on point per frame --
/// while [`game_data_identity_of`] beside it reads four fields for a census line that is written
/// once per person.
fn multiplay_role_of(chr_ins: usize) -> Option<u8> {
    if !plausible_chr_ins(chr_ins) {
        return None;
    }
    let data = unsafe {
        er_game_base::mem::safe_read_usize(chr_ins + PLAYER_INS_PLAYER_GAME_DATA_OFFSET)
    }?;
    // SAFETY: a screen on the value, not a dereference.
    if data == 0 || !unsafe { is_heap_aligned_ptr(data) } {
        return None;
    }
    unsafe { er_game_base::mem::safe_read_u8(data + PLAYER_GAME_DATA_MULTIPLAY_ROLE_OFFSET) }
}

/// Read the four `PlayerGameData` fields the census reports.
///
/// Separate from [`session_identity_of`] because the two structures fail independently: a
/// character can have a `PlayerGameData` and no session entry while it is still being
/// constructed, and a census that dropped the whole line on either would report nothing in
/// exactly the moments worth reporting.
#[must_use]
fn game_data_identity_of(player_ins: usize) -> Option<GameDataIdentity> {
    if !plausible_chr_ins(player_ins) {
        return None;
    }
    let data = unsafe {
        er_game_base::mem::safe_read_usize(player_ins + PLAYER_INS_PLAYER_GAME_DATA_OFFSET)
    }?;
    // SAFETY: a screen on the value, not a dereference.
    if data == 0 || !unsafe { is_heap_aligned_ptr(data) } {
        return None;
    }
    Some(GameDataIdentity {
        multiplay_role: unsafe {
            er_game_base::mem::safe_read_u8(data + PLAYER_GAME_DATA_MULTIPLAY_ROLE_OFFSET)
        }?,
        chr_type: unsafe {
            er_game_base::mem::safe_read_u8(data + PLAYER_GAME_DATA_CHR_TYPE_OFFSET)
        }?,
        invasion_item_type: unsafe {
            er_game_base::mem::safe_read_u8(data + PLAYER_GAME_DATA_INVASION_ITEM_TYPE_OFFSET)
        }?,
        is_main_player: unsafe {
            er_game_base::mem::safe_read_u8(data + PLAYER_GAME_DATA_IS_MAIN_PLAYER_OFFSET)
        }? != 0,
    })
}

/// Read the session entry behind a `PlayerIns`.
///
/// # Why the census asks at all
///
/// The rule identifies a candidate by `ChrIns::chr_type`, and on 2026-09-10 that failed in the
/// only session that matters: the filter hid every `chr_type` 2 candidate for a whole invasion --
/// 4096 of them -- and the invader at the keyboard could still lock on to every fellow invader in
/// the world. The lock-on candidate walk in `LockTgtMan` skips a point whose owner resolves to
/// null before it asks `CS::ChrIns::CanTargetTeamType`, so a hidden candidate is genuinely out;
/// the ones that were locked therefore never matched the set. Under Seamless Co-op a remote
/// player has measured `chr_type` 0, which is also the host's, so no `chr_type` rule can separate
/// them. Tracked as bd er-effects-rs-hgfy.
///
/// # Why it now leads with the Steam ID
///
/// The first version read `isHost`, `isLocalPlayer` and `preCeremonyMultiplayRole` and nothing
/// else. Ghidra's typed `CS::SessionManagerPlayerEntry` puts all three exactly where it read
/// them, and they still answered nothing: six distinct entries in one invasion each reported
/// `is_host=true`, `pre_ceremony_role=0`, and most of them `is_local_player=true` as well. Three
/// booleans that say the same thing about everyone cannot name anybody. The Steam ID can, and
/// the persona name beside it lets the person at the keyboard match a census line to the player
/// they just locked on to.
///
/// Read, never acted on: nothing in `rules::hides` sees any of this.
#[must_use]
fn session_identity_of(player_ins: usize) -> Option<SessionIdentity> {
    if !plausible_chr_ins(player_ins) {
        return None;
    }
    let entry = unsafe {
        er_game_base::mem::safe_read_usize(
            player_ins + PLAYER_INS_SESSION_MANAGER_PLAYER_ENTRY_OFFSET,
        )
    }?;
    if entry == 0 {
        return None;
    }
    // SAFETY: a screen on the value, not a dereference -- the reads below all go through the
    // fault-closed reader.
    if !unsafe { is_heap_aligned_ptr(entry) } {
        return None;
    }
    let steam_id =
        unsafe { er_game_base::mem::safe_read_usize(entry + SESSION_ENTRY_STEAM_ID_OFFSET) }?
            as u64;
    let is_host =
        unsafe { er_game_base::mem::safe_read_u8(entry + SESSION_ENTRY_IS_HOST_OFFSET) }? != 0;
    let is_local_player =
        unsafe { er_game_base::mem::safe_read_u8(entry + SESSION_ENTRY_IS_LOCAL_PLAYER_OFFSET) }?
            != 0;
    let pre_ceremony_role =
        unsafe { er_game_base::mem::safe_read_u8(entry + SESSION_ENTRY_PRE_CEREMONY_ROLE_OFFSET) }?;
    Some(SessionIdentity {
        entry,
        steam_id,
        name: steam_name_of(entry),
        is_host,
        is_local_player,
        pre_ceremony_role,
    })
}

/// The persona name inside a session entry, transcribed one text unit at a time through the
/// fault-closed reader.
///
/// `er-player-name-filter` may already have rewritten it, so a masked name here is that crate
/// working rather than this one misreading.
fn steam_name_of(entry: usize) -> Option<String> {
    let string = entry + SESSION_ENTRY_STEAM_NAME_OFFSET;
    let backing = unsafe { er_game_base::mem::safe_read_usize(string + DL_STRING_BACKING_OFFSET) }?;
    let length = unsafe { er_game_base::mem::safe_read_usize(string + DL_STRING_LENGTH_OFFSET) }?;
    if backing == 0 || length == 0 || length > STEAM_NAME_UTF16_CAPACITY {
        return None;
    }
    let mut units = Vec::with_capacity(length);
    for index in 0..length {
        units.push(unsafe { er_game_base::mem::safe_read_u16(backing + index * 2) }?);
    }
    String::from_utf16(&units).ok()
}

/// Write down one character's session identity, once per person.
fn note_identity(role: Role, chr_ins: usize, chr_type: i32) {
    let Some(identity) = session_identity_of(chr_ins) else {
        return;
    };
    {
        let mut seen = SEEN_IDENTITY_ENTRIES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if seen.contains(&identity.entry) || seen.len() >= IDENTITY_CENSUS_LIMIT {
            return;
        }
        seen.push(identity.entry);
    }
    let team = match team_type_of(chr_ins) {
        Some(team) => team.to_string(),
        None => "unreadable".to_owned(),
    };
    let name = identity.name.as_deref().unwrap_or("<unreadable>");
    let game_data = match game_data_identity_of(chr_ins) {
        Some(data) => format!(
            "multiplay_role={} game_data_chr_type={} invasion_item_type={} is_main_player={}",
            data.multiplay_role, data.chr_type, data.invasion_item_type, data.is_main_player
        ),
        None => "player_game_data unreadable".to_owned(),
    };
    log_message(format_args!(
        "census: {} \"{name}\" steam_id={} chr_type={chr_type} team_type={team} {game_data} \
         is_host={} is_local_player={} pre_ceremony_role={}. Read for the census only -- no rule \
         consults any of it. The Steam ID and the multiplayer role are here because `chr_type` and \
         the three booleans beside it reported the same thing about every person in a live \
         invasion.",
        role.label(),
        identity.steam_id,
        identity.is_host,
        identity.is_local_player,
        identity.pre_ceremony_role
    ));
}

/// `GameMan::summonParamType` -- the multiplayer role the engine matched this session on, and the
/// field it derives the local player's `ChrType` from.
///
/// Read rather than called. `CS::GameMan::GetSummonParamType` is a null check and one load
/// (1.16.2 `0x140679300`, 1.17 `0x14067a150`), both of them reading `+0xd84` off the same
/// singleton in both builds, so calling it would add a translated call target for no answer the
/// two loads below do not already give.
fn summon_param_type() -> i32 {
    let global = GAME_MAN_GLOBAL.load(Ordering::Relaxed);
    if global == 0 {
        return SUMMON_PARAM_TYPE_UNKNOWN;
    }
    // SAFETY: `global` is an address inside the game image's own `.data`, resolved for this build
    // at install and mapped for the life of the process.
    let game_man = unsafe { (global as *const usize).read_volatile() };
    if !unsafe { is_heap_aligned_ptr(game_man) } {
        return SUMMON_PARAM_TYPE_UNKNOWN;
    }
    // SAFETY: the singleton outlives its published pointer and is far longer than this offset;
    // the game's own accessor reads exactly here behind exactly this null check.
    unsafe { ((game_man + GAME_MAN_SUMMON_PARAM_TYPE_OFFSET) as *const i32).read_volatile() }
}

/// The local player's `PlayerIns`, or `None` before the world has one.
///
/// Reads `WorldChrManImp + 0x1e508` directly rather than calling `GetMainPlayerIns`, which
/// answers the debug-camera override first: with a possession mod loaded that override is the
/// creature being worn, and the question here is about the person invading, not the body they
/// are driving.
fn main_player() -> Option<usize> {
    let global = WORLD_CHR_MAN_GLOBAL.load(Ordering::Relaxed);
    if global == 0 {
        return None;
    }
    // SAFETY: `global` is an address inside the game image's own `.data`, resolved for this build
    // at install and mapped for the life of the process.
    let world_chr_man = unsafe { (global as *const usize).read_volatile() };
    if !unsafe { is_heap_aligned_ptr(world_chr_man) } {
        return None;
    }
    // SAFETY: the singleton is a live object well over 0x1e510 bytes long once its pointer is
    // published, and this hook only runs from the lock-on update, which the game does not reach
    // before the world exists.
    let player = unsafe {
        ((world_chr_man + WORLD_CHR_MAN_PLAYER_INS_OFFSET) as *const usize).read_volatile()
    };
    plausible_chr_ins(player).then_some(player)
}

/// `ChrIns::chr_type`, read as the raw `i32` the field holds.
///
/// Deliberately not decoded into an enum. A Seamless session may type a character in a way the
/// vanilla enum never anticipated, and a raw integer can be surprised where a constructed enum
/// would be undefined behaviour.
fn chr_type_of(chr_ins: usize) -> Option<i32> {
    if !plausible_chr_ins(chr_ins) {
        return None;
    }
    // SAFETY: screened above, and every caller has it from the game's own handle lookup or from
    // the `WorldChrMan` singleton, on the game thread, inside a frame the world is live for.
    Some(unsafe { ((chr_ins + CHR_INS_CHR_TYPE_OFFSET) as *const i32).read_volatile() })
}

/// Is `candidate` a `CS::PlayerIns`, judged against the class the main player is an instance of?
///
/// A class test rather than a `chr_type` test on purpose. `chr_type` is the field a session layer
/// rewrites -- Seamless Co-op has been measured typing every remote player `Local` -- so it cannot
/// separate a player from anything; the vtable is written once by the constructor and no mod in
/// this profile replaces it. `main` supplies the reference rather than a pinned address, so the
/// comparison needs no per-build translation and cannot drift.
///
/// Answers `false` when the reference has not been captured yet, which stands the filter down for
/// that point rather than guessing. The main player is read first on every call into the hook, so
/// the reference exists from the first candidate onward.
fn is_player_ins(candidate: usize, main: usize) -> bool {
    let mut reference = PLAYER_INS_VTABLE.load(Ordering::Relaxed);
    if reference == 0 {
        let Some(main_vtable) = vtable_of(main) else {
            return false;
        };
        PLAYER_INS_VTABLE.store(main_vtable, Ordering::Relaxed);
        reference = main_vtable;
    }
    vtable_of(candidate) == Some(reference)
}

/// The object's vtable word, screened first.
fn vtable_of(chr_ins: usize) -> Option<usize> {
    if !plausible_chr_ins(chr_ins) {
        return None;
    }
    // SAFETY: `plausible_chr_ins` has already read this word and found it pointing inside the
    // game image, which is what makes the object a live C++ instance rather than a stale pointer.
    Some(unsafe { (chr_ins as *const usize).read_volatile() })
}

/// Say once that non-player candidates are being passed over.
///
/// Without this the log of a session where nobody invaded is indistinguishable from the log of a
/// session where the hook never ran, and the previous version of this census answered that by
/// naming every kind of enemy it saw -- a line per wildlife type, none of which can say anything
/// about two humans.
fn note_non_players_are_skipped() {
    if SAID_NON_PLAYERS_ARE_SKIPPED.swap(true, Ordering::Relaxed) {
        return;
    }
    log_message(format_args!(
        "census: the lock-on system is offering non-player characters, which are passed over \
         without being filtered or counted -- this rule is only ever about another player. From \
         here, a `census: a candidate ...` line means a real player was offered."
    ));
}

/// `ChrIns::team_type`, the raw byte. Census only -- no rule reads it.
fn team_type_of(chr_ins: usize) -> Option<u8> {
    if !plausible_chr_ins(chr_ins) {
        return None;
    }
    // SAFETY: same object and same screen as `chr_type_of`, one byte further into a struct the
    // game's own `GetTeamType` reads at this offset.
    Some(unsafe { ((chr_ins + CHR_INS_TEAM_TYPE_OFFSET) as *const u8).read_volatile() })
}

/// Cheap screen: heap-shaped, with a vtable inside the game image.
fn plausible_chr_ins(chr_ins: usize) -> bool {
    if !unsafe { is_heap_aligned_ptr(chr_ins) } {
        return false;
    }
    let base = GAME_BASE.load(Ordering::Relaxed);
    if base == 0 {
        return false;
    }
    // SAFETY: `is_heap_aligned_ptr` has ruled out the low reserve, and the pointer came from the
    // game's own handle lookup, so its first word is the object's vtable.
    let vtable = unsafe { (chr_ins as *const usize).read_volatile() };
    vtable_in_game_image(vtable, base)
}

/// Which side of the question a `ChrType` came from.
#[derive(Clone, Copy)]
enum Role {
    LocalPlayer,
    Candidate,
}

impl Role {
    fn label(self) -> &'static str {
        match self {
            Self::LocalPlayer => "local player",
            Self::Candidate => "candidate",
        }
    }
}

/// Name the local player's `ChrType`, once, and each multiplayer role once per change.
///
/// The candidate's half is not here: it is reported by the caller, and only for a candidate that
/// is another player.
fn note_seen_local_player(self_chr_type: i32, summon_param_type: i32) {
    note_seen_one(&SEEN_SELF_TYPES, self_chr_type, Role::LocalPlayer);
    note_summon_param_type(self_chr_type, summon_param_type);
}

/// Report the multiplayer role whenever it changes, which is once per session join rather than
/// once per point.
fn note_summon_param_type(self_chr_type: i32, summon_param_type: i32) {
    if LAST_SUMMON_PARAM_TYPE.load(Ordering::Relaxed) == summon_param_type {
        return;
    }
    if LAST_SUMMON_PARAM_TYPE.swap(summon_param_type, Ordering::Relaxed) == summon_param_type {
        return;
    }
    log_message(format_args!(
        "census: GameMan summon param type is {summon_param_type} ({}); with chr_type \
         {self_chr_type} that reads as {}",
        if summon_param_type == SUMMON_PARAM_TYPE_UNKNOWN {
            "unread".to_owned()
        } else if HOSTILE_PHANTOM_SUMMON_PARAM_TYPES.contains(&summon_param_type) {
            "an invasion role".to_owned()
        } else {
            "not an invasion role".to_owned()
        },
        if local_player_is_invading(self_chr_type, summon_param_type) {
            "invading"
        } else {
            "not invading"
        }
    ));
}

fn note_seen_one(seen: &AtomicU32, chr_type: i32, role: Role) {
    if !(0..=MAX_CHR_TYPE).contains(&chr_type) {
        if !SEEN_UNREPRESENTABLE_TYPE.swap(true, Ordering::Relaxed) {
            log_message(format_args!(
                "census: a {} has chr_type {chr_type}, outside the 0..={MAX_CHR_TYPE} range this \
                 filter can express, so it is never hidden",
                role.label()
            ));
        }
        return;
    }
    let bit = 1_u32 << chr_type;
    // Loaded before the read-modify-write so the steady state -- every kind already seen -- is a
    // plain read of a shared word rather than a write to it, on a path that runs once per lock-on
    // point per frame.
    if seen.load(Ordering::Relaxed) & bit != 0 {
        return;
    }
    if seen.fetch_or(bit, Ordering::Relaxed) & bit != 0 {
        return;
    }
    log_message(format_args!(
        "census: first {} with chr_type {chr_type}; the hostile-phantom set [{}] {} it",
        role.label(),
        HOSTILE_PHANTOMS.describe(),
        if HOSTILE_PHANTOMS.contains(chr_type) {
            "holds"
        } else {
            "does not hold"
        }
    ));
}

/// Report each `(chr_type, team_type)` pairing once per side.
///
/// The pairing is what makes this worth a log line. `chr_type` alone has already been measured
/// reading 0 for every remote player in a Seamless session, which is also what the host reads, so
/// the census cannot currently tell a fellow invader from the person an invader is there to fight.
/// If the team byte differs across two characters that share a `chr_type`, it is the discriminator
/// the rule is missing; if it tracks `chr_type` exactly, it is derived from it, as
/// `CS::ChrIns::InitTeamType` says it is in the vanilla path, and this line closes that question
/// rather than leaving it to be re-argued.
fn note_team_pairing(seen: &AtomicU64, role: Role, chr_type: i32, chr_ins: usize) {
    let Some(team) = team_type_of(chr_ins) else {
        return;
    };
    // 64 bits over a `chr_type` that is 0..=31 and a team byte that is 0..=78: too small to hold
    // every pairing, so it holds a mix and a collision costs one unlogged line, never a wrong one.
    let bit = 1_u64 << ((((chr_type as u32) << 3) ^ u32::from(team)) & 63);
    if seen.load(Ordering::Relaxed) & bit != 0 {
        return;
    }
    if seen.fetch_or(bit, Ordering::Relaxed) & bit != 0 {
        return;
    }
    log_message(format_args!(
        "census: a {} with chr_type {chr_type} has team_type {team}",
        role.label()
    ));
}

fn note_hidden(
    self_chr_type: i32,
    summon_param_type: i32,
    candidate_chr_type: i32,
    candidate_multiplay_role: Option<u8>,
) {
    let count = SUPPRESSED.fetch_add(1, Ordering::Relaxed) + 1;
    if count > SUPPRESSION_LOG_LIMIT && !count.is_multiple_of(SUPPRESSION_LOG_INTERVAL) {
        return;
    }
    let role = match candidate_multiplay_role {
        Some(role) => role.to_string(),
        None => "unreadable".to_owned(),
    };
    log_message(format_args!(
        "hidden: a chr_type {candidate_chr_type} multiplay_role {role} character is out of the \
         lock-on candidate set while you are chr_type {self_chr_type}, summon param type \
         {summon_param_type} ({count} so far)"
    ));
}

/// Where this run's log lands: the artifact directory the launcher named, else beside the game
/// executable.
///
/// This crate wrote straight into the game directory until 2026-09-09, and it is the reason the
/// question "did the filter fire in that invasion you ran last week" has no answer. A
/// game-directory artifact is single-slot: `begin_fresh_run` rotates `<name>` to `<name>.prev` and
/// truncates on the first write of each process, so two launches destroy the run before last.
/// Reconstructed from the launcher logs afterwards, 43 separate runs had loaded this DLL and every
/// one of their logs was gone except the last two -- and neither of those two was an invasion.
///
/// The knob is the one every other shell here honours, so the launcher already knows how to set
/// it: `ARTIFACT_ENV` in `scripts/er_artifact_env.py` maps it to this file name, and
/// `er-run-branch.py`'s selftest asserts that table covers every variable the Rust reads.
fn log_message(args: fmt::Arguments<'_>) {
    // The knob is spelled inline rather than through a `const` on purpose:
    // `scripts/er-artifact-redirect-audit.py` discovers every launcher knob by reading the Rust for
    // this exact call shape with a literal, so a name hidden behind a constant is a knob the audit
    // cannot see -- and an invisible knob is how this file went unredirected in the first place.
    let path = er_game_base::log::redirected_artifact_path(
        "ER_QUICKLOAD_LOCKON_FILTER_LOG_PATH",
        LOG_FILE_NAME,
    );
    append_line(&path, format_args!("{args}"));
}
