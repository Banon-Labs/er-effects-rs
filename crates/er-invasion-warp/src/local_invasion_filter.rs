//! The local invasion filter, in the product DLL.
//!
//! Ports what the frida harness proved (`scripts/frida-ersc-session-trace.py`) into a shipped
//! feature:
//!
//! * the destination is readable at `CS::SosSignMan::SetMultiplayJoinData`, from
//!   `ServerPushJoinData+0x00`, before the player moves;
//! * rejecting a match by driving ERSC's "Cancel search" option is non-destructive -- the session
//!   walks `0x22 -> 0x00` and searching continues;
//! * the option actions share one signature, `(OSM, ctx, 1, 1)`, captured from real presses rather
//!   than inferred from a decompile -- though the static read below then showed `ctx` is never
//!   examined, which is what let the capture machinery be deleted.
//!
//! # What this deliberately does not do
//!
//! It does not fake an invasion, spoof session state, or enter `CSNetMan` / `QuickmatchManager` /
//! `CSBreakInPointManager`. It reads a destination the server already sent, and -- when the user
//! has asked for filtering -- invokes the same cancel the user could press by hand. Everything it
//! calls is a path the game runs anyway.
//!
//! # Why nothing in `ersc.dll` is hooked
//!
//! Asked directly whether the filter could avoid repeatedly cancelling, the binary answered no --
//! and answered something better instead. Static read of the shipped `ersc.dll`, first taken
//! 2026-08-05 and re-measured against the supported build:
//!
//! * `ersc+0x25850` ("Invade world") does one thing: require the session idle, take the mutex at
//!   `S+0x100`, bail if `S+0x14C == 0x7fffffff`, write `S+0x150 = 0xe`, release. Fifteen
//!   instructions on that path out of 31 in the whole `0x75`-byte function, the rest being the
//!   two fatal blocks it branches to. `ersc+0x258d0` ("Cancel search") is the same shape without
//!   the idle precondition, writing `0x23`. Neither queries anything.
//! * Across all 4903 functions in the unpacked `.text`, `0xe` reaches `S+0x150` at exactly one
//!   site -- the one above. There is no client-side candidate list to filter, because starting a
//!   search *is* that single store; everything after it happens inside the Themida-virtualised
//!   dispatcher and on the remote side. This is why `SetMultiplayJoinData` is not a late
//!   interception point but the first instant the destination exists on this machine, and why
//!   accept-then-reject is the only available shape.
//! * Both actions read **`rcx` only**. `rdx`, `r8` and `r9` are never touched. So the earlier plan
//!   -- hook the actions to capture a real press and replay its arguments -- was solving a problem
//!   that does not exist: `(OSM, 0, 1, 1)` is provably equivalent to what the engine passes.
//!
//! Every one of those findings survived the last Seamless update as a statement about the
//! mechanism, and none of them survived as a NUMBER: the addresses moved, the session fields moved
//! as a block, and the state enum was renumbered throughout. That is the reason the numbers above
//! live in [`ersc`] rather than in this prose, and the reason the module they describe is
//! identified by byte-checking the invade action before any of them is used.
//!
//! With the arguments unnecessary, the only thing still needed from Seamless is the OSM pointer.
//! Reading it out of a static would have meant hooking nothing in Seamless at all; that was
//! attempted and does not work (see [`ersc::NEXT_OBJECT_OFFSET`] for the candidate that looked
//! right and was not). So OSM is learned by observing it being passed to the menu builder.
//!
//! What that leaves is **two** detours: `CS::SosSignMan::SetMultiplayJoinData`, a game function,
//! where matches are judged; and `ersc!show`, the Seamless menu builder, which is observed
//! read-only -- it copies `rcx` and immediately runs the original with every argument untouched,
//! changing nothing and suppressing nothing. The two option actions are not hooked, and a rejection
//! invokes the same callback the user's own click invokes, with arguments the callee provably
//! ignores. `nothing_in_this_module_detours_ersc`'s successor test pins that budget so growing it
//! is a decision rather than a drift.
//!
//! `ersc.dll` is RELOCATABLE and has no fixed load address, so every ERSC address is
//! `module base + RVA` resolved at runtime and byte-checked before use. If Seamless is not
//! loaded the filter never arms: without a Seamless session there are no Seamless invasions
//! to filter.
//!
//! # Which Seamless build
//!
//! The latest seamless co-OP only. `ersc.dll` is third-party and the user updates it on their own
//! schedule; chasing every past build with its own address set is unbounded work on a moving
//! target, and it buys a co-op player nothing, because Seamless rotates the lobby-key salt on
//! release and so clients of different builds cannot see each other's sessions anyway.
//!
//! [`resolve_ersc_abi`] therefore picks from [`ersc::SUPPORTED`] -- currently one entry -- by
//! byte-checking the invade action, an entry point this module calls but never hooks, so its bytes
//! stay the shipped ones. Exactly one has to match; zero or two both refuse. The table shape stays
//! because Seamless will update again and the next build is another entry, not a rewrite.
//!
//! # Fail-closed direction
//!
//! Every uncertainty resolves toward not cancelling. Config missing or unparseable, OSM not
//! resolvable, ERSC absent, ERSC present but a build we have not measured, anchor unresolved --
//! all leave matches alone. The failure this guards against is silently cancelling other players'
//! invasions, which is worse than a filter that quietly does nothing. That is also why the byte
//! checks run all the way through each action's state write rather than stopping at a prologue:
//! five different functions share the option-action opening, and the write is the only
//! instruction that says which one this is.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use er_game_base::fnv1a::fnv1a64;
use er_invasion_warp_core::local_invasion::{
    InvasionAnchor, InvasionCandidate, LocalInvasionConfig, LocalInvasionMode, RejectReason,
    Verdict,
};
use er_invasion_warp_core::local_invasion_config::{
    CONFIG_FILE_NAME, DEFAULT_CONFIG_TOML, HotConfig,
};
use er_invasion_warp_core::param_row::PinAppearance;

// Which Seamless Co-op build is loaded, and everything that differs between them. The docs are on
// the file itself; a `///` here would be a second, competing source for the same module.
/// Which key does what, and what each press changes. Split out when this file crossed the
/// 3,200-line hard limit; the seam was already there.
mod hotkeys;

/// Both items are `#[cfg(windows)]` inside the module -- they read `GetAsyncKeyState` -- so the
/// re-export has to be gated too, or the host-target test build looks for names that do not exist.
#[cfg(windows)]
pub use hotkeys::{MarkKeys, warp_keys_in_force};

mod ersc;

// Driving Seamless's own option actions -- cancel, invade, and the stall recovery around them.
// Its own file because this one is at the size limit; the docs are on it.
mod actions;

// The reading taken immediately before any ERSC action is driven: who owns the `std::mutex` that
// action locks first. Its own file because this one is at the size limit; the docs are on it.
pub(crate) mod banner;
pub(crate) mod map_pins_view;
#[cfg(windows)]
pub mod menu_object;
#[cfg(windows)]
mod menu_seams;
pub use map_pins_view::{log_pin_tier_tally, pin_appearance_for, pin_choice_signature};
pub(crate) mod differential_scan;
pub(crate) mod lock_report;
pub(crate) mod session_scan;

use actions::{
    arm_self_recovery, cancel_match, cancel_stalled_attempt_inner, drive_pending_reinvade,
    log_refusal_once, watch_for_stall,
};
/// Called from outside this DLL and from `lynchpin_use`, so the names stay where their
/// callers already look for them.
pub use actions::{drive_invade_inline, drive_invade_with_owner, request_invade};
use lock_report::{
    HANDLER_ERSC_LOBBY_KEY, HANDLER_ERSC_SHOW, HANDLER_GAME_TASK, HANDLER_JOIN_DATA,
    cancel_row_refusal, enter_ersc_callback, enter_handler, inside_ersc_callback,
    lock_shape_refusal, mutex_shape_identifies_a_session, report_lock_preconditions,
};
use session_scan::cached_scan_for_session;

/// The four-argument shape of an ERSC option action. Only the first is read by the callee, which
/// the disassembly in the module docs establishes; the rest are passed as the constants the engine
/// itself passes so a stack trace through one of these looks exactly like a user's own click.
///
/// # Why the ABI carries `-unwind`
///
/// These functions throw. `ersc+0x258d0` reaches `_Throw_Cpp_error(5)` -- a real MSVC
/// `std::system_error` raised through `_CxxThrowException` -- whenever its `_Mtx_lock` reports the
/// session mutex busy, and nothing in the whole module catches it: a census of ersc.dll's exception
/// directory finds 403 catch clauses, every one of them in a record the linker emitted for a `.text`
/// function, and not one on the path between the throw and the top.
///
/// Declaring a function that unwinds with a non-unwinding ABI is undefined behaviour, not merely
/// unwise -- the Reference says so under `panic.unwind.ffi.undefined`, and RFC 2945 adds that such
/// an unwind is *not guaranteed* to abort. The abort observed on 2026-09-08 was one manifestation
/// of that, not a contract, and under `extern "system"` the compiler emits the call with no unwind
/// edge at all, so nothing in this crate's frames gets to clean up.
///
/// This is the honest half of the change. The frame that actually aborts is the detour, and every
/// detour in this workspace goes through `er_hook::UnionFn`, whose ABI is shared by twenty-six
/// DLLs -- retyping that is its own change with its own blast radius, tracked separately. What this
/// buys on its own is a defined call and destructors that run, which is what makes [`OurCall`]'s
/// `Drop` load-bearing rather than decorative.
type ErscActionFn = unsafe extern "system-unwind" fn(usize, usize, usize, usize) -> usize;

/// The last session state this module saw, so a transition to "cancelling" that we did not cause
/// can be recognised as the user's own Cancel search -- polled, rather than hooked.
static LAST_SESSION_STATE: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Set while the filter is itself driving ERSC, so our own cancel is not mistaken for the user's.
///
/// Never written directly -- go through [`OurCall::enter`], whose `Drop` clears it. It used to be a
/// pair of bare stores around each call, which was free only because the throw that escapes an
/// ERSC action kills the process before the second store can be missed. The moment that stops being
/// true -- an unwinding ABI on the detours, a refusal that returns early -- a skipped clear latches
/// this flag on for the life of the process, after which the filter reads every ERSC cancel as its
/// own and never stands down again.
static IN_OUR_CALL: AtomicBool = AtomicBool::new(false);

/// Holds [`IN_OUR_CALL`] for the duration of one call into ERSC, and clears it on the way out of
/// the scope however that happens.
struct OurCall;

impl OurCall {
    #[must_use]
    fn enter() -> Self {
        IN_OUR_CALL.store(true, Ordering::SeqCst);
        Self
    }
}

impl Drop for OurCall {
    fn drop(&mut self) {
        IN_OUR_CALL.store(false, Ordering::SeqCst);
    }
}
/// Armed by our own cancel: search again as soon as the session settles back to idle. Cleared the
/// moment the re-invade fires, so a session that never returns to idle cannot make this repeat.
static PENDING_REINVADE: AtomicBool = AtomicBool::new(false);
/// Attempts that ended without us cancelling them, and were restarted anyway. Counted separately
/// from `REINVADES` so "Seamless dropped it" is distinguishable from "we rejected it" in a log.
static SELF_RECOVERIES: AtomicUsize = AtomicUsize::new(0);
/// Set once the invade action has been found uncallable, and cleared the moment it is callable
/// again. The latch that stops the recovery loop re-arming against something that cannot fire.
///
/// Without it, `arm_self_recovery` sets `PENDING_REINVADE` every tick, `drive_pending_reinvade`
/// clears it again on an `ersc_action` that returns `None`, and the next tick sees an ended
/// attempt and repeats -- once per tick, forever, with a log line each time and the counter that
/// records a real restart never moving. Measured twice: 6056 lines with `rearmed=0` on
/// 2026-09-08, and 7523 on 2026-09-09.
///
/// The trigger both times was a Frida `Interceptor` on `ersc+0x25850` writing a trampoline over
/// the bytes this module byte-checks before calling. That is a debugging artefact, but the shape
/// is not: any Seamless build whose invade prologue differs from the measured one produces the
/// identical spin, on a user's machine, with no debugger in sight. So the fix belongs here and
/// not in the agent that happened to expose it.
static INVADE_ACTION_UNCALLABLE: AtomicBool = AtomicBool::new(false);
/// Attempts cancelled because a handshake step stopped progressing.
static STALL_RECOVERIES: AtomicUsize = AtomicUsize::new(0);
/// Stall detection state. Behind a mutex rather than atomics because the decision reads and writes
/// "which state, and since when" together; a torn read there would restart the clock at random.
static STALL_WATCHDOG: Mutex<crate::stall_watchdog::StallWatchdog> =
    Mutex::new(crate::stall_watchdog::StallWatchdog::new());
/// Slows the restart when Seamless is refusing attempts instantly -- the opposite failure to the
/// one the stall watchdog catches, and invisible to it because every state is held too short.
static RESTART_BACKOFF: Mutex<crate::restart_backoff::RestartBackoff> =
    Mutex::new(crate::restart_backoff::RestartBackoff::new());
/// Monotonic origin for the stall clock. The DLL log carries no timestamps, so elapsed time has to
/// come from somewhere in-process.
static PROCESS_START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
/// Ticks of the recurring game task since load, and the tick/millisecond stamp of the last logged
/// session transition — together, how long the session sat in the state it just left, measured two
/// independent ways.
///
/// # Why both, and not either one
///
/// Seamless's no-match retry dwells in `0x11` for a fixed interval before returning to `0x0d`, and
/// whether that interval is a frame count or a wall clock decides whether raising the frame rate
/// would shorten the wait — the difference between a usable idea and a void one. The two are
/// indistinguishable at a steady frame rate, which is exactly the condition the earlier reading was
/// taken under: ~600 ticks, nine times running, at unchanging fps. That is equally consistent with
/// both, so it settles nothing.
///
/// Recording both per transition makes any natural frame-rate variation within one run decide it —
/// a constant tick count across dwells at differing fps means a frame counter, a constant
/// millisecond count means a clock. The implied fps is printed alongside so a comparison between
/// two dwells that ran at the same rate is visibly inconclusive rather than silently over-read.
static TICKS: AtomicU64 = AtomicU64::new(0);
static LAST_TRANSITION_TICK: AtomicU64 = AtomicU64::new(0);
static LAST_TRANSITION_MS: AtomicU64 = AtomicU64::new(0);
/// Decides which rejections are worth announcing. Behind a mutex because the decision reads and
/// updates "what did we last say" together.
static REJECT_NOTICE: Mutex<er_invasion_warp_core::reject_notice::RejectNotice> =
    Mutex::new(er_invasion_warp_core::reject_notice::RejectNotice::new());
/// Set once the banner has failed, so a missing notice is reported one time instead of every 20
/// seconds for the rest of the session.
static NOTICE_FAILED: AtomicBool = AtomicBool::new(false);
/// Cleared by a cancel the user performed. Their cancel means "stop looking", and it has to beat
/// our re-arm or the filter would fight them.
static AUTO_SEARCH_ARMED: AtomicBool = AtomicBool::new(false);

static CANCELS: AtomicUsize = AtomicUsize::new(0);
static KEEPS: AtomicUsize = AtomicUsize::new(0);
static REINVADES: AtomicUsize = AtomicUsize::new(0);
/// Matches judged a rejection that were then not cancelled -- the invasion proceeded anyway.
///
/// The oracle that was missing, and its absence is why a broken filter looked like a working one
/// for a whole session. Every other state this module can be in is visible from the heartbeat, but
/// "armed, judging correctly, and enforcing nothing" was visible only to someone who read four
/// specific lines out of 408 and understood that `NOT cancelled` meant the feature was inert. The
/// user's report -- "if we were on the strictest settings, I didn't only invade locally. It might
/// be disabled?" -- is that gap stated from the player's seat.
///
/// A non-zero value here is the failure: the filter said no and the player went anyway. It belongs
/// beside `CANCELS`, because the two together are the only honest statement of what the filter did
/// -- a rejection count on its own cannot distinguish a match that was stopped from one that was
/// merely disapproved of.
static UNENFORCED_REJECTS: AtomicUsize = AtomicUsize::new(0);

static CONFIG: Mutex<Option<HotConfig>> = Mutex::new(None);

/// Trampoline to the original `SetMultiplayJoinData` -- the module's only detour, and it is on the
/// game, not on Seamless.
static ORIG_SET_JOIN_DATA: AtomicUsize = AtomicUsize::new(0);

/// Trampoline to the original `CS::CSSessionManager::JoinSession`.
static ORIG_JOIN_SESSION: AtomicUsize = AtomicUsize::new(0);

/// Install-once latch for the join refusal.
static JOIN_SESSION_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);

/// Set by a `Verdict::Reject` and consumed by the very next `JoinSession`, which the game makes 87
/// bytes later in the same caller.
///
/// It is a one-shot rather than a mode: `SosSignMan::JoinSession` calls `SetMultiplayJoinData` and
/// then `CSSessionManager::JoinSession` with nothing between them that can fail, so a latch set at
/// the judgement is consumed by the join it was set for. Anything that reaches the join without
/// passing the judgement -- a co-op sign, a summon, the player's own quickmatch -- finds it clear.
static REFUSE_NEXT_JOIN: AtomicBool = AtomicBool::new(false);

/// How many joins have been refused before the RPC was sent.
static JOINS_REFUSED: AtomicUsize = AtomicUsize::new(0);

/// Install-once latch. The installer runs from the recurring game task rather than `DllMain`
/// because MinHook must not run under the loader lock.
static JOIN_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Logged-once latch for a successful OSM resolve, so the log records the address the run used
/// without repeating it every frame.
static OSM_REPORTED: AtomicUsize = AtomicUsize::new(0);

/// Where the config lives: in the game directory, next to every other `er-*.toml`, so a user
/// editing it does not have to hunt for it.
fn config_path() -> PathBuf {
    er_game_base::log::game_directory_path().map_or_else(
        || PathBuf::from(CONFIG_FILE_NAME),
        |dir| dir.join(CONFIG_FILE_NAME),
    )
}

/// Write the documented default once, if absent, so the file exists to be edited.
pub fn ensure_config_file() {
    let path = config_path();
    if !path.exists() {
        match std::fs::write(&path, DEFAULT_CONFIG_TOML) {
            Ok(()) => crate::standalone_log(format_args!(
                "local-invasion: wrote the default config to {} (filter OFF until you enable it)",
                path.display()
            )),
            Err(error) => crate::standalone_log(format_args!(
                "local-invasion: could not write {}: {error} -- the filter stays OFF",
                path.display()
            )),
        }
    }
}

/// Re-read the config if it changed, logging the new state once per change.
fn refresh_config() {
    let path = config_path();
    let Ok(mut guard) = CONFIG.lock() else {
        return;
    };
    let hot = guard.get_or_insert_with(HotConfig::default);
    if let Some(outcome) = hot.reload_if_changed(&path) {
        if outcome.reverted_to_defaults {
            crate::standalone_log(format_args!(
                "local-invasion: config gone -- filter OFF (matches are left alone)"
            ));
        } else {
            crate::standalone_log(format_args!(
                "local-invasion: config loaded enabled={} mode={} hunt={} dll_users_only={} \
                 reject_notice={} map_pins={} steam_hooks={} ersc_observers={} \
                 ersc_show_observer={} ersc_lobby_key_observer={} ersc_invade_observer={} \
                 named={} ids={} blocks={} \
                 excluded={} mark={} unmark={} enable_toggle={} warp_nearest={} warp_next={} \
                 warp_other_area={}",
                outcome.config.enabled,
                outcome.config.mode.as_str(),
                outcome.config.hunt,
                // Every option that changes behaviour must appear here. These three were missing,
                // and the gap cost a live A/B on 2026-08-06: the file was edited mid-session to turn
                // `dll_users_only` on, this line duly reprinted -- proving the reload had happened --
                // but said nothing about the option that had just changed. Whether the new value had
                // parsed was unknowable until a lobby-pool line happened to appear a minute later.
                // "The config reloaded" is not the question anyone has; "what is in force now" is.
                outcome.config.dll_users_only,
                outcome.config.reject_notice,
                outcome.config.map_pins,
                // The four ersc_* switches are the ones that decide whether this DLL DETOURS
                // Seamless at all, and MinHook's patch into it faulted at 0x140010043. They
                // default off and the filter now resolves the session by scanning ersc's writable
                // data instead, so a run that has them on is a different program from the one the
                // 600s clean window was measured on -- which makes them the single most important
                // group of values on this line, not the least.
                outcome.config.steam_hooks,
                outcome.config.ersc_observers,
                outcome.config.ersc_show_observer,
                outcome.config.ersc_lobby_key_observer,
                outcome.config.ersc_invade_observer,
                outcome.config.named_locations.len(),
                outcome.config.named_location_text_ids.len(),
                outcome.config.allowed_blocks.len(),
                // Exclusions beat everything else, so a forgotten one is the hardest rejection to
                // explain from the outside -- it looks identical to being in the wrong place.
                outcome.config.blocked_blocks.len(),
                // Which keys are actually live. Without this a mistyped name that happened to parse
                // into a different valid key looks exactly like the feature not working.
                er_invasion_warp_core::keybind::key_name(outcome.config.mark_key),
                er_invasion_warp_core::keybind::key_name(outcome.config.unmark_key),
                er_invasion_warp_core::keybind::key_name(outcome.config.enable_toggle_key),
                er_invasion_warp_core::keybind::key_name(outcome.config.warp_nearest_key),
                er_invasion_warp_core::keybind::key_name(outcome.config.warp_next_key),
                er_invasion_warp_core::keybind::key_name(outcome.config.warp_other_area_key),
            ));
            warn_about_key_collisions(&outcome.config);
        }
        // Say that the typed names do nothing yet. `named_locations` is parsed and stored but never
        // resolved to text ids, so it contributes nothing to a verdict -- and in `mode = "named"`
        // with no ids collected that means every match is rejected, forever, for a user who did
        // exactly what the file told them to. The verdict itself is reported
        // (`NothingToMatchAgainst`), but nothing connected it to the names they typed.
        if !outcome.config.named_locations.is_empty() {
            crate::standalone_log(format_args!(
                "local-invasion: {} typed name(s) in `named_locations` are NOT being used -- \
                 resolving a place name string to its FMG text id is not implemented, so only ids \
                 collected by Shift+Insert are matched. In mode = \"named\" with no such ids, every \
                 match is rejected as NothingToMatchAgainst.",
                outcome.config.named_locations.len()
            ));
        }
        for issue in &outcome.issues {
            crate::standalone_log(format_args!(
                "local-invasion: config line {}: {}",
                issue.line, issue.message
            ));
        }
    }
}

/// Say so when two of this crate's own keys land on the same physical key.
///
/// Two pollers on one key is not a cosmetic clash. `GetAsyncKeyState`'s low bit means "pressed
/// since the previous call on this thread" and reading it consumes it, so whichever poller asks
/// first eats the edge and the other sees nothing -- intermittently, depending on ordering. That
/// is the least debuggable shape a keybinding bug can take, and now that every key is
/// configurable a player can produce it by hand in one edit.
///
/// A warning rather than a refusal: the config is the player's, and the mark keys and the warp
/// keys are read by different pollers in different situations, so a deliberate overlap is theirs
/// to make. What must not happen is it being silent.
fn warn_about_key_collisions(config: &LocalInvasionConfig) {
    let bindings = [
        ("mark_key", config.mark_key),
        ("unmark_key", config.unmark_key),
        ("warp_nearest_key", config.warp_nearest_key),
        ("warp_next_key", config.warp_next_key),
        ("warp_other_area_key", config.warp_other_area_key),
    ];
    for (index, (name, key)) in bindings.iter().enumerate() {
        for (other_name, other_key) in &bindings[index + 1..] {
            if key == other_key {
                crate::standalone_log(format_args!(
                    "local-invasion: {name} and {other_name} are BOTH {} -- two pollers on one key \
                     consume each other's press latch, so one of them will fire only sometimes. \
                     Give them different keys.",
                    er_invasion_warp_core::keybind::key_name(*key)
                ));
            }
        }
    }
}

/// The config currently in force, re-reading the file first.
fn current_config() -> Option<LocalInvasionConfig> {
    refresh_config();
    let guard = CONFIG.lock().ok()?;
    guard.as_ref().map(|hot| hot.current().clone())
}

// ---------------------------------------------------------------------------------------------
// Which Seamless build is loaded
// ---------------------------------------------------------------------------------------------

/// The recognised build, as `index + 1` into [`ersc::SUPPORTED`]. `0` = not resolved yet;
/// [`ABI_REFUSED`] = resolved to "none of the builds we know".
static ABI: AtomicUsize = AtomicUsize::new(0);
/// Distinct from "not resolved yet" so the refusal is reported exactly once rather than every
/// tick, and so a later tick does not silently retry a module already ruled out.
const ABI_REFUSED: usize = usize::MAX;

/// Which Seamless Co-op build is loaded, or `None` if it is one this module cannot drive.
///
/// # Fail-closed, and explicit about which build
///
/// Every entry in [`ersc::SUPPORTED`] is checked, and exactly one has to match. Zero matches is an
/// unrecognised build -- a Seamless the addresses below were never measured against -- and the
/// filter stays inert, which is the safe direction: a wrong address here would drive a live
/// multiplayer session with the wrong field offsets and cancel other players' invasions.
///
/// Two matches would mean the discriminator does not discriminate, and is refused just as hard.
/// It cannot happen for the two entries as they stand (measured: each invade pin occurs exactly
/// once in its own build and nowhere in the other), and the check exists so that adding a third
/// entry whose pin is too weak fails loudly instead of silently picking whichever came first.
///
/// The answer is cached because a loaded module cannot change identity mid-process. Callers that
/// are about to call into Seamless still byte-check the specific function first -- caching which
/// build it is does not cache permission to jump into it.
#[cfg(windows)]
fn resolve_ersc_abi() -> Option<&'static ersc::Abi> {
    match ABI.load(Ordering::SeqCst) {
        0 => {}
        ABI_REFUSED => return None,
        cached => return ersc::SUPPORTED.get(cached - 1),
    }
    let base = ersc_module_base()?;
    let mut matched: Option<usize> = None;
    let mut ambiguous = false;
    for (index, abi) in ersc::SUPPORTED.iter().enumerate() {
        if prologue_matches(base + abi.invade_action_rva, abi.invade_prologue) {
            ambiguous |= matched.is_some();
            matched = Some(index);
        }
    }
    match matched {
        Some(index) if !ambiguous => {
            let abi = &ersc::SUPPORTED[index];
            if ABI.swap(index + 1, Ordering::SeqCst) == 0 {
                crate::standalone_log(format_args!(
                    "local-invasion: ersc.dll @0x{base:x} recognised as Seamless Co-op v{} -- filter armed \
                     (show=+0x{:x} invade=+0x{:x} cancel=+0x{:x} lobby_key=+0x{:x}, session state \
                     at S+0x{:x}, idle={:#x} searching={:#x} cancelling={:#x})",
                    abi.version,
                    abi.show_rva,
                    abi.invade_action_rva,
                    abi.cancel_action_rva,
                    abi.build_lobby_key_rva,
                    abi.session_state_offset,
                    abi.state_idle,
                    abi.state_searching,
                    abi.state_cancelling,
                ));
            }
            Some(abi)
        }
        outcome => {
            if ABI.swap(ABI_REFUSED, Ordering::SeqCst) == 0 {
                let known: Vec<&str> = ersc::SUPPORTED.iter().map(|abi| abi.version).collect();
                let complaint = if outcome.is_some() {
                    "matches MORE THAN ONE of the builds below, so the discriminator is not \
                     discriminating and none of them can be trusted"
                } else {
                    "is not one of the builds below: the invade action is not at any of their \
                     addresses with any of their bytes"
                };
                crate::standalone_log(format_args!(
                    "local-invasion: ersc.dll @0x{base:x} {complaint}. Known: {}. The filter stays \
                     inert and will NOT cancel anything -- that is the fail-closed direction, \
                     because driving an unrecognised build with these field offsets would cancel \
                     other players' invasions. To measure the new build: uv run --with capstone \
                     python3 scripts/locate-ersc-entry-points.py",
                    known.join(", "),
                ));
            }
            None
        }
    }
}

#[cfg(not(windows))]
fn resolve_ersc_abi() -> Option<&'static ersc::Abi> {
    None
}

// ---------------------------------------------------------------------------------------------
// Resolving Seamless's session, without hooking it
// ---------------------------------------------------------------------------------------------

/// Whether the module at `base` is still the one whose identity was recorded on first use.
///
/// Reads three PE-header fields and nothing else. See [`resolve_session`] for why this may not
/// read code: every function this check has ever fingerprinted has since been hooked, twice by
/// this module and once by a debugger attached beside it, and each time the check read a
/// trampoline and called Seamless a stranger.
///
/// The first call records; every later call compares. A module that cannot be read at all fails,
/// because not knowing is not a licence to drive its code.
#[cfg(windows)]
fn module_identity_holds(base: usize) -> bool {
    /// `IMAGE_DOS_HEADER::e_lfanew`.
    const PE_LFANEW: usize = 0x3c;
    /// `IMAGE_NT_HEADERS::FileHeader::TimeDateStamp`.
    const PE_TIME_DATE_STAMP: usize = 8;
    /// `IMAGE_NT_HEADERS::OptionalHeader::AddressOfEntryPoint`.
    const PE_ENTRY_POINT: usize = 24 + 16;
    /// `IMAGE_NT_HEADERS::OptionalHeader::SizeOfImage`.
    const PE_SIZE_OF_IMAGE: usize = 24 + 56;
    /// Recorded identity, or 0 before the first successful read. Zero is not a valid combination
    /// of these three fields for a real module, so it doubles as the "not yet recorded" marker.
    static IDENTITY: AtomicUsize = AtomicUsize::new(0);

    let Some(lfanew) = (unsafe { er_game_base::mem::safe_read_usize(base + PE_LFANEW) }) else {
        return false;
    };
    let nt = base + (lfanew & 0xffff_ffff);
    let read = |offset: usize| unsafe { er_game_base::mem::safe_read_usize(nt + offset) };
    let (Some(stamp), Some(entry), Some(size)) = (
        read(PE_TIME_DATE_STAMP),
        read(PE_ENTRY_POINT),
        read(PE_SIZE_OF_IMAGE),
    ) else {
        return false;
    };
    // Folded rather than compared field by field: one word is one atomic, and the three fields
    // are only ever meaningful together.
    let identity =
        (stamp & 0xffff_ffff) ^ ((entry & 0xffff_ffff) << 16) ^ ((size & 0xffff_ffff) << 32);
    let identity = if identity == 0 { 1 } else { identity };
    match IDENTITY.compare_exchange(0, identity, Ordering::SeqCst, Ordering::SeqCst) {
        Ok(_) => true,
        Err(recorded) => recorded == identity,
    }
}

/// Host-side stub: there is no module to identify.
#[cfg(not(windows))]
fn module_identity_holds(_base: usize) -> bool {
    false
}

/// The option-menu object and its session, resolved by reading, plus the build they belong to.
#[derive(Clone, Copy)]
struct SeamlessSession {
    osm: usize,
    session: usize,
    abi: &'static ersc::Abi,
}

/// The option-menu object, observed once when Seamless builds its menu. Zero until then.
static OSM: AtomicUsize = AtomicUsize::new(0);
/// Trampoline for the one ERSC observer.
static ORIG_SHOW: AtomicUsize = AtomicUsize::new(0);
static SHOW_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);

/// `ersc!show(OSM, groupId)` -- Seamless building its option menu.
///
/// The single point where this module touches `ersc.dll`, and it is pure observation: it copies
/// the first argument and immediately runs the original with every argument untouched. It changes
/// no state, suppresses nothing, and returns exactly what Seamless returned. Its only purpose is
/// that OSM has no static to read it from, so the pointer has to be seen being passed.
#[cfg(windows)]
unsafe extern "system" fn show_observer(a: usize, b: usize, c: usize, d: usize) -> usize {
    // Stamp the thread, so the owner id in `report_lock_preconditions` has a name to resolve to
    // rather than staying a bare number.
    let _scope = enter_handler(HANDLER_ERSC_SHOW);
    let _ersc = enter_ersc_callback();
    // `a` is the option-menu object: that is `show`'s first parameter, and the prologue at this
    // address was byte-checked before the hook went in. Storing it is therefore not a guess, and
    // it is deliberately not gated on a content check.
    //
    // It used to be gated on the `seamless` tag at `+0x68`, and that silently broke the whole
    // feature on 2026-08-05: a real match was judged and rejected, then `cannot cancel -- session
    // is not resolvable`, because the tag never matched and OSM was consequently never stored. The
    // tag had been measured once, live, in one frida session; promoting a single observation to a
    // precondition is what turned it into a gate on the product path. It is now reported as a
    // diagnostic and believed by nothing.
    // Opening Seamless's menu is the user reaching for the controls, so the auto-search loop stands
    // down here -- before they have even chosen an option.
    //
    // This replaces inferring "the user cancelled" from the session reaching `0x22`, which was
    // wrong on its face: the static scan of ersc.dll found seven sites writing `0x22` to `S+0x110`
    // and only one of them is the Cancel-search action, so every internal abort read as a user
    // cancel. Menu-open is unambiguous, needs no new detour, and fails in the safe direction --
    // the worst case is that the loop stops when the user only wanted a look, which costs a
    // keypress, where the old rule's worst case was fighting them for control.
    //
    // `IN_OUR_CALL` guards the reentrant case: driving the cancel option can make Seamless rebuild
    // its own menu, which lands right back here. Without the guard this module would read its own
    // cancel as the user opening the menu and stand itself down after every single rejection.
    if a != 0
        && !IN_OUR_CALL.load(Ordering::SeqCst)
        && AUTO_SEARCH_ARMED.swap(false, Ordering::SeqCst)
    {
        PENDING_REINVADE.store(false, Ordering::SeqCst);
        crate::standalone_log(format_args!(
            "local-invasion: you opened Seamless's menu -- auto re-search stood down, the options \
             you see are Seamless's own and nothing here will act while you decide"
        ));
    }
    let first_capture = menu_object::capture_osm(a, &format!("its option menu (group={b})"));
    let orig = ORIG_SHOW.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let result = unsafe { core::mem::transmute::<usize, ErscActionFn>(orig)(a, b, c, d) };
    // After the original, not before it, and this is not a style preference -- it is the whole
    // difference between reading the rows and reading nothing. `show` is what clears and APPENDS
    // the option vector, so on entry `+0x108`/`+0x110` still describe the previous (empty) menu.
    // The first live run reported `visible options: <unreadable>` from exactly that mistake, and
    // a manual /proc read moments later showed one populated 0x90-byte row sitting there. Same
    // `first` latch, so this still reports once per process.
    if first_capture {
        menu_seams::report_menu_seams(a);
    }
    result
}

/// Why a session could not be resolved. Carried so a failed cancel names its cause instead of
/// being one generic line that fits three different bugs.
#[derive(Clone, Copy, Debug)]
enum NoSession {
    /// Seamless is not loaded.
    ErscAbsent,
    /// Loaded, but not the build these offsets were measured against.
    ErscUnrecognised,
    /// No session has been identified. Neither the shape scan nor the differential scan has a
    /// single answer, so there is nothing to drive an action through.
    ///
    /// This variant was called `MenuNeverOpened` until 2026-09-09, and the name was a lie that
    /// cost a live session. The message it printed told the player to open Seamless's menu, on the
    /// reasoning that the menu builder is where the object is learned -- but the `show` observer
    /// that would learn it has been disabled since MinHook's patch into `ersc.dll` was measured to
    /// fault at `0x140010043`, so opening the menu could not possibly help and the instruction sent
    /// the reader to do something with no path to working. What is actually true is narrower and
    /// actionable: the scans have not converged, and each invasion narrows them.
    SessionNotIdentified,
    /// OSM is held but `+0x58` does not lead to a session-shaped object -- a stale pointer.
    SessionUnreadable,
}

impl NoSession {
    /// Which of the four it was, for the trace.
    ///
    /// `join-progress` used to print a bare `ersc=<unresolved>`, which collapses four completely
    /// different situations into one word: Seamless not loaded at all, loaded but an unmeasured
    /// build, loaded and measured but the menu was never opened, and a stale session pointer. Only
    /// the last two are interesting and only one of them is a fault, so the bare form sent every
    /// reader who saw it -- me included, on 2026-09-03 -- looking for a resolver bug that may not
    /// exist. Measured that day: `ersc=<unresolved>` on every join-progress line of a run whose
    /// startup had already logged `recognised as Seamless Co-op v2.0.1 -- filter armed`, i.e. two
    /// of the four were already excluded by a line further up the same file and the trace still
    /// would not say which of the remaining two it was.
    fn label(self) -> &'static str {
        match self {
            Self::ErscAbsent => "<absent>",
            Self::ErscUnrecognised => "<unmeasured-build>",
            Self::SessionNotIdentified => "<session-not-identified>",
            Self::SessionUnreadable => "<session-unreadable>",
        }
    }
}

/// What to say after "cannot cancel -- SessionNotIdentified", built from the scan's live state.
///
/// Not an instruction to the player. The one thing that narrows the differential scan is another
/// invasion, and the numbers say how close it is, so the line reports progress and leaves it at
/// that. Returns a `&'static str` because the refusal is latched and logged once; the counts are
/// rendered into a leaked string only on the pass that actually prints.
#[cfg(windows)]
fn not_identified_detail() -> &'static str {
    let (held, rounds) = differential_scan::progress();
    if held == 0 {
        return ". No candidates are recorded, so the sweeper has not managed an idle pass yet -- \
                nothing to narrow.";
    }
    Box::leak(
        format!(
            ". The differential scan is down to {held} candidate(s) after {rounds} join(s); it \
             adopts one only when a single candidate is left, and each invasion narrows it \
             further. Opening Seamless's menu does NOT help -- the observer that would learn the \
             object from it is disabled because detouring ersc.dll faults the game."
        )
        .into_boxed_str(),
    )
}

#[cfg(not(windows))]
fn not_identified_detail() -> &'static str {
    ""
}

/// Resolve the option-menu object and its session, validating structurally.
///
/// Validation is on the shape this module actually depends on -- `OSM+0x58` reads as a pointer, and
/// the session's state field holds a small state -- rather than on a remembered byte pattern. Those
/// two are exactly what a cancel needs to be safe, and unlike the tag they are load-bearing in the
/// code below.
///
/// Nothing is cached beyond OSM and the recognised build, and OSM is re-validated on every use: the
/// session is a heap allocation whose lifetime this module does not own, and a stale pointer is
/// exactly the kind of thing that turns a filter into a crash.
fn resolve_session() -> Result<SeamlessSession, NoSession> {
    let base = ersc_module_base().ok_or(NoSession::ErscAbsent)?;
    // Which build, and therefore which addresses, offsets and state codes. Refuses on anything
    // this module has not measured; see `resolve_ersc_abi`.
    let abi = resolve_ersc_abi().ok_or(NoSession::ErscUnrecognised)?;
    // Re-prove it on every use, rather than trusting the cached verdict alone -- but on the PE
    // header, not on code.
    //
    // The recurring fingerprint used to read a function's opening bytes, and that has now broken
    // this feature three separate times, each in the same way and each with a different function.
    // `show` until 2026-08-05, when this module's own detour on it made the check read our patch
    // and report `ErscUnrecognised` on a live rejection. Then `invade`, until it gained an
    // observer of its own on 2026-09-08. Then `cancel`, for about twenty minutes on the same
    // evening, until a Frida `Interceptor` was attached there to watch the player's own cancel --
    // and a real rejection came back `cannot cancel (WrongBlock) -- ErscUnrecognised` again.
    //
    // Chasing an entry point nobody has hooked yet is a losing game: any tool may hook any
    // function, ours included, and the check cannot tell a stranger's module from its own
    // trampoline. So it reads what no hook touches. `SizeOfImage`, `TimeDateStamp` and
    // `AddressOfEntryPoint` come out of the PE header, which MinHook and Frida both leave alone,
    // and together they identify the mapped file rather than the state of its code.
    //
    // What this still catches is the only thing the recurring check was ever for: the module at
    // this base being replaced by a different one mid-run. The build is proved once, by
    // `resolve_ersc_abi`'s discriminator, before any detour exists.
    if !module_identity_holds(base) {
        return Err(NoSession::ErscUnrecognised);
    }
    let osm = OSM.load(Ordering::SeqCst);
    if osm == 0 {
        // No detour supplied it, so go and find the session instead of giving up.
        //
        // `scan_for_session` recognises the object by its own state field rather than being handed
        // a pointer to it, so the filter keeps working with nothing hooked inside Seamless.
        let Some((slot, session, owner)) = cached_scan_for_session(base, abi) else {
            return Err(NoSession::SessionNotIdentified);
        };
        if OSM_REPORTED.swap(1, Ordering::SeqCst) == 0 {
            crate::standalone_log(format_args!(
                "local-invasion: session resolved WITHOUT hooking Seamless -- found at \
                 0x{session:x} via a pointer in ersc's own writable data at 0x{slot:x}, owner \
                 0x{owner:x}. This is the path that keeps the filter alive now that detouring \
                 ersc.dll with MinHook's five bytes faults at 0x140010043. An owner of 0 means the \
                 pointer WAS the session and nothing implies an owner -- the filter then judges \
                 and logs but declines to drive cancel/invade, because passing 0 as their first \
                 argument dereferences null inside ersc.dll."
            ));
        }
        return Ok(SeamlessSession {
            osm: owner,
            session,
            abi,
        });
    }
    let session = unsafe { er_game_base::mem::safe_read_usize(osm + ersc::NEXT_OBJECT_OFFSET) }
        .filter(|session| *session != 0)
        .filter(|session| read_session_state(abi, *session).is_some())
        .ok_or(NoSession::SessionUnreadable)?;
    if OSM_REPORTED.swap(1, Ordering::SeqCst) == 0 {
        crate::standalone_log(format_args!(
            "local-invasion: Seamless session resolved -- OSM=0x{osm:x} session=0x{session:x}"
        ));
    }
    Ok(SeamlessSession { osm, session, abi })
}

/// Trampoline for the lobby-key observer.
static ORIG_BUILD_LOBBY_KEY: AtomicUsize = AtomicUsize::new(0);
/// One-shot latch for the `ctx`-shape probe above.
static CTX_SHAPE_PROBED: AtomicBool = AtomicBool::new(false);
/// Whether the lobby-key observer is installed.
static LOBBY_KEY_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// FNV-1a of the last key reported, so a re-key is one line and a steady key is silent.
static LAST_LOBBY_KEY_HASH: AtomicUsize = AtomicUsize::new(0);
/// How many times the key has been derived, and how many distinct values were seen.
static LOBBY_KEY_DERIVATIONS: AtomicUsize = AtomicUsize::new(0);
static LOBBY_KEY_CHANGES: AtomicUsize = AtomicUsize::new(0);

/// Read an MSVC `std::string` as ASCII, or `None` if it is not the shape we expect.
///
/// Every read is fault-closed. The value is [`ersc::LOBBY_KEY_HEX_LEN`] characters, far past what
/// the inline buffer holds, so the heap branch is the only one that can carry it -- but the inline
/// branch is handled anyway rather than assumed away, because an assumption here would silently
/// print nothing on a build whose string differs.
#[cfg(windows)]
fn read_std_string(at: usize) -> Option<String> {
    let size = unsafe { er_game_base::mem::safe_read_usize(at + ersc::STD_STRING_SIZE_OFFSET) }?;
    let capacity =
        unsafe { er_game_base::mem::safe_read_usize(at + ersc::STD_STRING_CAPACITY_OFFSET) }?;
    // A key is 16 characters. Anything wildly longer is not the string this was written for, and
    // reading it would be a walk through memory on a guess.
    // A SHA-256 hex digest. Anything else is not the string this was written for, and reading it
    // would be a walk through memory on a guess.
    if size != ersc::LOBBY_KEY_HEX_LEN || capacity < size {
        return None;
    }
    let data = if capacity >= ersc::STD_STRING_HEAP_CAPACITY {
        unsafe { er_game_base::mem::safe_read_usize(at) }?
    } else {
        at
    };
    let mut out = String::with_capacity(size);
    for index in 0..size {
        let byte = unsafe { er_game_base::mem::safe_read_u8(data + index) }?;
        // Printable ASCII only: the value is hex digits, and refusing anything else keeps a wrong
        // pointer from spraying control bytes into the log.
        if !(0x20..0x7f).contains(&byte) {
            return None;
        }
        out.push(char::from(byte));
    }
    Some(out)
}

/// `BuildLobbyKey(ctx, out)` -- observed, never altered.
///
/// Runs the original first, then reads the string it produced. Reading before the call would see
/// an uninitialised buffer; reading after is the only ordering that can work, and it also means a
/// fault in our read cannot affect what Seamless publishes.
#[cfg(windows)]
unsafe extern "system" fn build_lobby_key_observer(
    ctx: usize,
    out: usize,
    c: usize,
    d: usize,
) -> usize {
    // Stamp the thread, so the owner id in `report_lock_preconditions` has a name to resolve to
    // rather than staying a bare number.
    let _scope = enter_handler(HANDLER_ERSC_LOBBY_KEY);
    let _ersc = enter_ersc_callback();
    let orig = ORIG_BUILD_LOBBY_KEY.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    // SAFETY: the trampoline MinHook produced for a byte-verified prologue; same four-argument
    // shape the union dispatcher uses everywhere else in this module.
    let result = unsafe { core::mem::transmute::<usize, ErscActionFn>(orig)(ctx, out, c, d) };

    LOBBY_KEY_DERIVATIONS.fetch_add(1, Ordering::SeqCst);

    // Can this detour replace the `show` one? That is the whole question keeping the
    // local-invasion filter alive, so it is asked here rather than argued about.
    //
    // `show` is the only thing this DLL patches that faults -- armed alone it dies at
    // 0x140010043 in ~25s, while this detour armed alone ran clean. But `show` is currently the
    // only source of `OSM`, and `resolve_session` needs `OSM` solely to reach
    // `[OSM + NEXT_OBJECT_OFFSET]`, the session. The session is self-identifying: `read_session_state`
    // returns `Some` only for a known state code at a known offset. So any pointer that reaches it
    // is as good as `OSM`, and this detour's first argument is a candidate nobody has tested.
    //
    // Two shapes are checked, once, and only reported: `ctx` being the session itself, and `ctx`
    // standing where `OSM` stands (session one hop away). A hit means the filter can be rebuilt on
    // a detour that does not crash; a miss rules this route out instead of leaving it as a hope.
    if !CTX_SHAPE_PROBED.swap(true, Ordering::SeqCst)
        && let Some(abi) = resolve_ersc_abi()
    {
        let direct = read_session_state(abi, ctx);
        let hop = unsafe { er_game_base::mem::safe_read_usize(ctx + ersc::NEXT_OBJECT_OFFSET) }
            .filter(|next| *next != 0)
            .and_then(|next| read_session_state(abi, next).map(|state| (next, state)));
        crate::standalone_log(format_args!(
            "local-invasion: lobby-key ctx=0x{ctx:x} -- is it the session? direct_state={direct:?}              one_hop={hop:?}. If either is Some, the filter can resolve its session WITHOUT the              `show` detour, which is the hook that crashes the game at 0x140010043 in ~25s. If              both are None this route is dead and the session must be found another way."
        ));
    }
    if let Some(key) = read_std_string(out) {
        let hash = fnv1a64(key.as_bytes()) as usize;
        if LAST_LOBBY_KEY_HASH.swap(hash, Ordering::SeqCst) != hash {
            let changes = LOBBY_KEY_CHANGES.fetch_add(1, Ordering::SeqCst) + 1;
            crate::standalone_log(format_args!(
                "local-invasion: LOBBY KEY = {key} (derivation #{}, distinct value \
                 #{changes}). ONE key serves both the lobby search filter and the publish, so \
                 whatever it partitions applies to co-op and invasions alike -- there is no \
                 invasion-only key in readable code. Two players whose keys differ never see each \
                 other; compare this line with your friend's. Observed only; nothing here \
                 publishes or alters a key.",
                LOBBY_KEY_DERIVATIONS.load(Ordering::SeqCst)
            ));
        }
    } else if LOBBY_KEY_DERIVATIONS.load(Ordering::SeqCst) == 1 {
        // Say what was actually there. "Could not read it" invites a guess; the length and
        // capacity say immediately whether the layout moved or the digest size changed.
        let size =
            unsafe { er_game_base::mem::safe_read_usize(out + ersc::STD_STRING_SIZE_OFFSET) };
        let capacity =
            unsafe { er_game_base::mem::safe_read_usize(out + ersc::STD_STRING_CAPACITY_OFFSET) };
        crate::standalone_log(format_args!(
            "local-invasion: the lobby key was derived but did not read back as {} hex characters \
             (size={size:?} capacity={capacity:?}) -- the std::string this build's lobby-key \
             builder wrote is not the shape expected, so the comparison is UNAVAILABLE rather \
             than wrong. Do not treat a missing line as 'the key did not change'.",
            ersc::LOBBY_KEY_HEX_LEN,
        ));
    }
    result
}

/// Install the lobby-key observer. Idempotent; returns 1 on success.
///
/// Separate from the `show` observer because it can fail independently: a Seamless build that moved
/// this function should cost the comparison, not the filter.
#[cfg(windows)]
fn install_lobby_key_observer() -> usize {
    if LOBBY_KEY_HOOK_INSTALLED.load(Ordering::SeqCst) != 0 {
        return 0;
    }
    let Some(base) = ersc_module_base() else {
        return 0; // Seamless not loaded yet -- retry next tick
    };
    let Some(abi) = resolve_ersc_abi() else {
        // `resolve_ersc_abi` already said, once, which builds are known and that none matched.
        LOBBY_KEY_HOOK_INSTALLED.store(1, Ordering::SeqCst);
        return 0;
    };
    let address = base + abi.build_lobby_key_rva;
    if !prologue_matches(address, abi.build_lobby_key_prologue) {
        if LOBBY_KEY_HOOK_INSTALLED.swap(1, Ordering::SeqCst) == 0 {
            crate::standalone_log(format_args!(
                "local-invasion: ersc.dll @0x{base:x} was recognised as Seamless Co-op v{} but does not carry \
                 that build's lobby-key builder at ersc+0x{:x} -- NOT touching it. The lobby-key \
                 comparison is unavailable; everything else is unaffected.",
                abi.version, abi.build_lobby_key_rva,
            ));
        }
        return 0;
    }
    if LOBBY_KEY_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return 0;
    }
    match unsafe {
        er_hook::register_union_hook(
            address,
            build_lobby_key_observer as er_hook::UnionFn,
            &ORIG_BUILD_LOBBY_KEY,
        )
    } {
        Ok(()) => {
            crate::standalone_log(format_args!(
                "local-invasion: observing ersc lobby-key builder @0x{address:x} (read-only). It \
                 reports the one string that decides whether two Seamless players can see each \
                 other at all."
            ));
            1
        }
        Err(error) => {
            crate::standalone_log(format_args!(
                "local-invasion: could not observe the ersc lobby-key builder: {error:?}"
            ));
            0
        }
    }
}

/// Install the one ERSC observer. Idempotent; returns 1 on success.
///
/// Deferred to the game task rather than `DllMain` for two reasons, either sufficient: ERSC is
/// injected after this DLL, so at attach time the module does not exist; and MinHook must not run
/// under the loader lock.
#[cfg(windows)]
fn install_show_observer() -> usize {
    if SHOW_HOOK_INSTALLED.load(Ordering::SeqCst) != 0 {
        return 0;
    }
    let Some(base) = ersc_module_base() else {
        return 0; // Seamless not loaded (yet) -- retry next tick
    };
    // Which build is loaded. Refuses -- loudly, once -- on one this module has not measured, so a
    // Seamless update disarms the filter rather than detouring an address that is now something
    // else entirely.
    let Some(abi) = resolve_ersc_abi() else {
        SHOW_HOOK_INSTALLED.store(1, Ordering::SeqCst);
        return 0;
    };
    let address = base + abi.show_rva;
    // Prove the module is the build this RVA describes before writing a single byte into it. This
    // one can read `show`, because it runs exactly once and only before the hook exists -- unlike
    // the recurring check in `resolve_session`, which had to stop reading `show` for that reason.
    if !prologue_matches(address, abi.show_prologue) {
        if SHOW_HOOK_INSTALLED.swap(1, Ordering::SeqCst) == 0 {
            // The version is the generated constant, not a literal: this line and the pins it is
            // talking about have to name the same build, and a hand-typed version beside a
            // repinned constant is a refusal that lies about why it refused.
            let supported = ersc::SUPPORTED_VERSION;
            crate::standalone_log(format_args!(
                "local-invasion: ersc.dll @0x{base:x} was recognised as Seamless Co-op v{} but does not carry \
                 that build's `show` at ersc+0x{:x} -- NOT touching it. The filter stays inert. \
                 This mod is measured against Seamless Co-op v{supported} and no other version: \
                 update to it, or, if yours is already newer, this mod has not been re-measured \
                 against your build yet. To see where the entry points went: uv run --with \
                 capstone python3 scripts/locate-ersc-entry-points.py",
                abi.version, abi.show_rva,
            ));
        }
        return 0;
    }
    if SHOW_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return 0;
    }
    match unsafe {
        er_hook::register_union_hook(address, show_observer as er_hook::UnionFn, &ORIG_SHOW)
    } {
        Ok(()) => {
            crate::standalone_log(format_args!(
                "local-invasion: observing ersc show @0x{address:x} (read-only; it is the only \
                 thing this DLL touches in Seamless, and only to learn the menu object's address)"
            ));
            1
        }
        Err(status) => {
            crate::standalone_log(format_args!(
                "local-invasion: union registration for ersc show failed: {status:?} -- the filter \
                 cannot find Seamless's session, so it will never cancel anything"
            ));
            0
        }
    }
}

/// The `seamless` ASCII tag at `OSM+0x68`. Without it any pointer-shaped value would pass.
pub(super) fn osm_tag_matches(osm: usize) -> bool {
    ersc::OSM_TAG.iter().enumerate().all(|(index, byte)| {
        unsafe { er_game_base::mem::safe_read_u8(osm + ersc::OSM_TAG_OFFSET + index) }
            .is_some_and(|got| got == *byte)
    })
}

/// The session state, or `None` when the value is not one a session would hold -- which is also
/// how a wrong pointer is rejected.
///
/// Takes the [`ersc::Abi`] rather than reading a module constant because a Seamless update has
/// already moved this field once. Reading the wrong one would not fault -- it would return a
/// plausible small number from a neighbouring field, and every decision below would be made on it.
fn read_session_state(abi: &ersc::Abi, session: usize) -> Option<u32> {
    let raw =
        unsafe { er_game_base::mem::safe_read_i32(session + abi.session_state_offset) }? as u32;
    (raw <= ersc::SESSION_STATE_MAX).then_some(raw)
}

/// [`read_session_state`], but strong enough to identify an object rather than merely read one.
///
/// `read_session_state` accepts any value `<= SESSION_STATE_MAX`, and zero passes that trivially.
/// As a read of a known session that is fine; as the signature `scan_for_session` matches on it is
/// useless, because zeroed memory is the most common thing in a writable section. Measured
/// 2026-09-04: the scan latched onto such an object, every subsequent read returned `0x00`, and
/// since `state_idle` is `0x01` the filter concluded an invasion attempt was permanently in
/// flight -- which is the gate `map_confirm` refuses warps on. The player could not warp to any
/// map marker for the entire session, and the log said only "an invasion attempt is in flight".
///
/// A real session at rest reads `state_idle`, so requiring a known state costs nothing and rejects
/// the haystack. Requiring merely non-zero does NOT: measured 2026-09-04, the scan then latched
/// onto a UTF-16 text buffer whose first character was a lowercase letter, and the "session state"
/// read `0x61`, `0x62`, `0x63` as the text changed -- `a`, `b`, `c`. `SESSION_STATE_MAX` is `0xff`,
/// so every byte value passes `read_session_state`; that is a range check, not an identity.
///
/// The four codes below are the ones this ABI actually reverses. A session caught mid-sequence in
/// an unreversed state is simply not identified this pass, and the scan runs again -- which costs
/// one more scan. Matching a string buffer costs the player every warp for the whole session.
///
/// And the value check is still not enough on its own -- see [`plausible_session_pointer`], which
/// this now requires first. Each of the three false positives above was answered by narrowing what
/// the field may contain; the third one proved the field was never the whole question, because the
/// address it was read from could not have been an object at all.
fn identifies_a_session(abi: &ersc::Abi, session: usize, discovering: bool) -> bool {
    // Ordered by cost, cheapest first, because the scan asks this of every qword in ersc's
    // writable data: two arithmetic tests, then two guarded reads, and only then the
    // `VirtualQuery` inside `plausible_session_pointer`. Written the other way round it made a
    // kernel call for every non-zero aligned word in 10.98 MB.
    addressable_session_pointer(session)
        && read_session_state(abi, session).is_some_and(|state| {
            // While a join is in flight the real session is never idle, and that is not a guess:
            // the invade action at ersc+0x25850 opens `cmp dword [rdi+0x150], 1` / `jne` -- it
            // refuses to start unless the state is idle -- and then writes `0xe` into that same
            // field before returning. So from the moment a search begins until it settles, the
            // object cannot read `state_idle`.
            //
            // Accepting idle during a join is what made the haystack unsearchable. Six candidates
            // have now been latched and rejected -- 0x3dfadb, 0x860f90f8, 0x451200, 0x45e00cb0,
            // 0xa2760038, 0xd2c0038 -- and every one of them sat at `0x01` for the whole run,
            // because `0x00000001` is one of the most common dwords in a process and the scan was
            // asking a question almost anything could answer. Requiring an active state at the one
            // moment we know a search is running shrinks the haystack by the ratio of "memory that
            // happens to hold 1" to "memory that happens to hold 0xe, 0x13 or 0x23".
            //
            // That argument is about discovery -- narrowing a haystack of a million candidates --
            // and it does not transfer to retention. Applied to the session already in hand it
            // does the opposite of its job: run br-20260908-212740-ea7c resolved the session,
            // drove ERSC's own cancel through it (`0x0e SEARCHING -> 0x23 CANCELLING`), and then
            // reported `MenuNeverOpened` for the very next rejection, because the join that made
            // the cancel necessary is also what made this rule refuse the pointer that would have
            // performed it. So `discovering` is false when the caller is re-checking the cached
            // answer, and the refusal applies only while choosing a new one.
            if discovering && JOIN_IN_FLIGHT.load(Ordering::SeqCst) && state == abi.state_idle {
                return false;
            }
            // The mirror of the rule above, and it closes the other half of the same hole. With no
            // join in flight there is nothing for a session to be doing, so a real one reads idle;
            // an object that reads `cancelling` or `searching` while the player is standing still
            // is holding a constant, not a state.
            //
            // Measured on run br-20260909-213454-2c38: the scan resolved `0x11500038` out of ERSC's
            // own writable data, and it read `0x23 cancelling` with no search anywhere in the
            // session. The crate then declined every drive through it -- correctly, since the
            // invade action only runs from idle -- so the pointer was useless and still occupied
            // the cache. Refusing it at discovery sends the sweeper back out instead.
            if discovering && !JOIN_IN_FLIGHT.load(Ordering::SeqCst) && state != abi.state_idle {
                return false;
            }
            state == abi.state_idle
                || state == abi.state_searching
                || state == abi.state_cancelling
                || state == abi.state_offer_received
        })
        // The `_Mtx_internal_imp_t` at `session+0x100`. Four small integers at known offsets is a
        // weak signature over a million candidates and it has now false-positived live four times;
        // this is the discriminator `ersc_owner_or_refuse` already used to catch the wrong pointer
        // after the scan had cached it. See `mutex_shape_identifies_a_session`.
        && mutex_shape_identifies_a_session(abi, session)
        && plausible_session_pointer(session)
}

/// The smallest address worth testing. Below this is the null page and the low reservations, never
/// an allocation.
const MIN_PLAUSIBLE_SESSION_POINTER: usize = 0x1_0000;

/// Every Seamless session pointer is at least pointer-aligned, and the one that broke the filter
/// was not aligned at all.
///
/// Measured live 2026-09-06, out of a run that judged four invasions, rejected all four and
/// cancelled none. [`scan_for_session`] had latched `0x3dfadb` as the session, reported through
/// `session resolved WITHOUT hooking Seamless -- found at 0x3dfadb ... owner 0x0`. Read back out of
/// the running process, that address is not an object: it is a three-byte-misaligned window into a
/// table of Wine pointers, and the four bytes at `+0x150` happen to read `01 00 00 00`, which is
/// this build's `state_idle` exactly. So it satisfied every value check above, permanently -- the
/// filter reported `ersc=0x01 IDLE` on all 53 `join-progress` samples of that run, across four
/// join-data pushes and a committed warp, and logged exactly one session-state line (the first
/// read) because the state it was watching could never change.
///
/// The cost was not a wrong reading. It was that [`scan_for_session`] then had a session it
/// believed in, which vetoed every real owner candidate (see there), so the owner stayed `0`,
/// [`ersc_owner_or_refuse`] declined, and all four rejections became
/// `NOT cancelled ... the invasion PROCEEDS`. The player was sent to `0x3c313600`, the filter
/// rejected it, and the next heartbeat records the player standing in it.
///
/// A session cannot live at an unaligned address. It carries a mutex sub-object and pointer fields,
/// so the allocator gives it at least pointer alignment and MSVC's `operator new` gives 16.
/// Requiring 8 is the weakest claim that is certainly true of every real session, so it cannot
/// reject one -- and it discards seven of every eight garbage qwords, this one among them, its low
/// bits being `0b011`.
///
/// Not `cfg`-gated: it is arithmetic, and it is what the host tests pin the measured addresses with.
/// Whether an address lies inside a loaded module's image rather than on the heap.
///
/// The session is allocated; it is never static data. Without this the scan accepted
/// `0x143c0cdc0` -- which is `eldenring.exe+0x3c0cdc0`, the game's own `.data` -- and every later
/// check waved it through: it is above `MIN_PLAUSIBLE_SESSION_POINTER`, it is 8-aligned, the dword
/// at `+0x150` happened to read `1`, and the bytes at `+0x100` happened to pass for an
/// `_Mtx_internal_imp_t`. Seamless's invade action then locked that "mutex" at `ersc+0x25871`, and
/// because nothing on earth unlocks a static game global, the game's main thread waited on it until
/// the stall watchdog fired thirty seconds later (run br-20260908-191656-17a9, `reason=main-thread-
/// stall`, `tid=392 first_thread=true` blocked at `ersc.dll+0x25876`).
///
/// A shape test cannot catch that, because the object is not garbage -- it is real game data that
/// happens to have the right numbers in the right places. What separates it from a session is where
/// it lives, and that is cheap to ask.
fn inside_a_loaded_module(candidate: usize) -> bool {
    module_backing(candidate).is_some()
}

/// The module an address belongs to and its offset within it, or `None` for heap and other
/// non-image memory.
///
/// Named rather than a bare bool so a refusal can say `eldenring.exe+0x3c0cdc0` instead of
/// `implausible`, which is the difference between a log line a reader can check and one they have
/// to take on faith. Lives in `er_game_base` because it needs the `windows` crate and this one does
/// not depend on it.
fn module_backing(candidate: usize) -> Option<(String, usize)> {
    er_game_base::mem::module_backing(candidate)
}

/// The free half of [`plausible_session_pointer`]: whether the value could address an allocation
/// at all.
///
/// Split out because the other half calls `VirtualQuery`, and [`scan_for_session`] asks about
/// every qword in 10.98 MB of writable data. A kernel call per candidate is what put the game's
/// main thread at 100% of a core; these two tests cost nothing and reject most of the haystack.
fn addressable_session_pointer(candidate: usize) -> bool {
    candidate >= MIN_PLAUSIBLE_SESSION_POINTER && candidate.is_multiple_of(align_of::<usize>())
}

fn plausible_session_pointer(candidate: usize) -> bool {
    addressable_session_pointer(candidate) && !inside_a_loaded_module(candidate)
}

/// True when the session is in the state every option action refuses to proceed past. They take a
/// fatal-error branch on it; this refuses instead.
fn session_guard_poisoned(abi: &ersc::Abi, session: usize) -> bool {
    unsafe { er_game_base::mem::safe_read_i32(session + abi.session_guard_offset) }
        .is_none_or(|raw| raw as u32 == ersc::SESSION_GUARD_POISON)
}

/// Log every session-state transition, and arm the auto re-search when the user starts one.
///
/// Added 2026-08-05 because three separate failures in a row were mis-attributed from a log that
/// only recorded this module's own decisions. The session state is the variable everything here
/// turns on, and it was the one thing never written down. A transition line costs a dword read per
/// frame and turns "why did nothing happen" from a guess into a reading.
///
/// # Why arming lives here, on one specific transition
///
/// It used to be "the session is not idle, so a search must be running, so arm" -- and that is why
/// standing down when the menu opened did nothing: you open the menu during a search, the loop
/// stood down, and one frame later the session was still non-idle so it armed straight back up. A
/// live log caught it, `stood down` followed immediately by `0x11 -> 0x0d` and another automatic
/// restart.
///
/// The replacement rests on a fact from the static scan rather than on inference: across the whole
/// unpacked `.text`, the searching code is written to the state field at exactly one site, inside
/// the Invade-world action -- `0x150 = 0x0e`, one site out of 4903 functions, measured
/// 2026-09-06; it held across the last update at the pre-renumber value too. So a transition
/// into it means
/// that action ran and nothing else, and the only remaining question is who ran it. Ours are
/// claimed by [`note_state_after_our_action`] before this ever sees them, so an unclaimed one is
/// the user pressing the option -- which is precisely, and only, when riding along is wanted.
fn trace_session_state(session: SeamlessSession) {
    let abi = session.abi;
    let Some(state) = read_session_state(abi, session.session) else {
        return;
    };
    let previous = LAST_SESSION_STATE.swap(state as usize, Ordering::SeqCst);
    if previous == state as usize {
        return;
    }
    log_transition(abi, previous, state, None);
    note_attempt_progress(abi, previous, state);
    if state == abi.state_searching {
        // A new hunt is a new question; whatever the last one turned into is spent.
        INVASION_ACTUALLY_HAPPENED.store(false, Ordering::SeqCst);
    }
    // `previous == usize::MAX` is the first reading of a session, not a transition into one. The
    // difference is the whole hunt: run br-20260908-212740-ea7c resolved a session whose first
    // read was already `0x0e`, armed the loop off that, timed five seconds, called the handshake
    // stalled and cancelled it -- before the player had used an invasion item. That left the
    // session parked at `0x23`, which is outside the set ERSC's own hide-predicate draws a Cancel
    // row for, so the real rejection minutes later could not have been cancelled even with a
    // session in hand. A search this mod did not watch begin is not a search it manages.
    if state == abi.state_searching
        && previous != usize::MAX
        && !AUTO_SEARCH_ARMED.swap(true, Ordering::SeqCst)
    {
        crate::standalone_log(format_args!(
            "local-invasion: you started a search -- rejected matches will be cancelled and the \
             search restarted until one lands somewhere you want, or you cancel it yourself"
        ));
    }
}

/// Record the state our own call produced, so the transition tracer does not mistake it for the
/// user acting.
///
/// This is what makes "who pressed Invade world" answerable at all. Our restart writes `0x0e` the
/// same way the option does, on the same thread, so by the time the next frame polls there is
/// nothing left to distinguish them -- unless we claim it first, which is what this does.
///
/// The number said `0x0d` here until 2026-09-08, left behind by the enum-wide renumber. The write
/// is `mov dword [rdi+0x150], 0xe` at `ersc+0x25886`, inside the invade action, and it is the only
/// site in the plaintext `.text` that puts that value in the field.
fn note_state_after_our_action(session: SeamlessSession, what: &str) {
    let Some(state) = read_session_state(session.abi, session.session) else {
        return;
    };
    let previous = LAST_SESSION_STATE.swap(state as usize, Ordering::SeqCst);
    if previous == state as usize {
        return;
    }
    log_transition(session.abi, previous, state, Some(what));
}

/// Feed the restart backoff the shape of the attempt, from transitions it already sees.
///
/// Three facts are all it needs, and each is a single transition:
///   * leaving idle  -> an attempt began, start the clock
///   * reaching [`ersc::Abi::state_offer_received`] -> this one is a real search, so clear any
///     accumulated penalty
///   * reaching idle -> the attempt is over; how long it lasted decides the delay
///
/// That state is the progress marker rather than a later one because it is the first step past the
/// fast-fail path: the measured spin ran four states back to idle without ever touching the
/// marker, while every healthy attempt in the same run passed through it within ~150 ms.
///
/// The last update renumbered the enum `+1`, so this is the one number carried across by
/// inference rather than read out of an instruction -- and carrying it is what keeps the marker
/// CORRECT: left unshifted it would land on the fast-fail path, clearing the penalty on exactly
/// the attempts that earned it.
fn note_attempt_progress(abi: &ersc::Abi, previous: usize, state: u32) {
    let Ok(mut backoff) = RESTART_BACKOFF.lock() else {
        return;
    };
    if previous == abi.state_idle as usize && state != abi.state_idle {
        backoff.attempt_started(now_ms());
    }
    if state == abi.state_offer_received {
        backoff.attempt_made_progress();
    }
}

/// The destination of the match in flight, remembered so the success banner can name it at the
/// moment the join actually lands rather than when the server first offered it. `usize::MAX` = no
/// match pending.
static PENDING_SUCCESS_BLOCK: AtomicUsize = AtomicUsize::new(usize::MAX);

/// This attempt actually became an invasion: the engine reported `LobbyState::Client`, which is
/// written only when the join RPC succeeded and the P2P session exists.
///
/// Measured 2026-08-17: every real join reached it 0.57-3.5s after join data, and not one of the
/// eleven rejected matches ever did -- those go `Joining(4) -> Closing(7) -> None(0)`. So this is
/// the discriminator that `Verdict::Keep` was standing in for, and unlike `Keep` it does not
/// depend on our filter having judged the match.
static INVASION_ACTUALLY_HAPPENED: AtomicBool = AtomicBool::new(false);

/// When `SetMultiplayJoinData` last fired, in [`now_ms`]. `0` = not since launch.
static JOIN_DATA_AT_MS: AtomicU64 = AtomicU64::new(0);
/// Packed last-logged engine reading, so the trace prints on change instead of every frame.
/// `u64::MAX` is "nothing logged yet", which is distinct from any real packing.
static JOIN_PROGRESS_LAST: AtomicU64 = AtomicU64::new(u64::MAX);
/// Frames sampled where the engine had nothing in flight while ERSC still claimed an attempt.
static JOIN_PROGRESS_IDLE_SAMPLES: AtomicUsize = AtomicUsize::new(0);

/// When the engine first reported a dead join while Seamless still claimed a match, in [`now_ms`].
/// `0` = not currently reporting one.
static DEAD_JOIN_SINCE_MS: AtomicU64 = AtomicU64::new(0);

/// A match this filter kept whose join has not yet landed or died.
///
/// Set at the accept, cleared the moment the engine reaches `lobbyState::CLIENT` -- the same
/// instant `INVASION_ACTUALLY_HAPPENED` latches, because that is when the invasion is real. If the
/// join dies while this is still set, the player never got the invasion they accepted, and the
/// hunt is re-armed rather than left stood down.
static KEPT_JOIN_PENDING: AtomicBool = AtomicBool::new(false);

/// How many hunts were resumed because an accepted join died before it landed.
static KEPT_JOIN_FAILURES: AtomicUsize = AtomicUsize::new(0);

/// How many stranded matches have been dropped without the player reaching for the item.
static DEAD_JOIN_RECOVERIES: AtomicUsize = AtomicUsize::new(0);

/// How long the engine must keep saying the join is dead before the match is dropped.
///
/// The reading is a fact rather than a timer -- `lobbyState` is `CreateFailed` or `JoinFailed`, so
/// the engine has already been out to Steam and been told no -- but a single frame sampled across
/// a state change is not worth acting on. Two thirds of a second is several frames at any rate the
/// game runs at and is still far below the point where the player would reach for the item.
const DEAD_JOIN_GRACE_MS: u64 = 650;

/// The same, for a session the engine has torn down rather than failed.
///
/// Longer because this reading is also what the normal rejection path passes through for a moment:
/// our refusal leaves `lobbyState` at `None` while Seamless takes about 1.3s to walk its own state
/// down, and a cancel driven into that window is a second cancel racing the first. Eight seconds
/// clears that flow by a wide margin and is still nothing against the 2029 seconds measured on run
/// br-20260910-021456-38bc.
const TORN_DOWN_GRACE_MS: u64 = 8_000;

/// Monotonic milliseconds since the first call.
///
/// The DLL log carries no timestamps of its own, so every elapsed measurement in this module comes
/// from here.
fn now_ms() -> u64 {
    let start = *PROCESS_START.get_or_init(std::time::Instant::now);
    // Saturating into u64 ms: a process cannot run long enough to overflow, and a cast that could
    // wrap would hand the detector a clock that appears to jump backwards.
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Write one session-state transition, stamped with how long the previous state was held.
///
/// The dwell is reported in ticks and in milliseconds because those two disagree only when the
/// frame rate changes, and that disagreement is the entire measurement — see [`TICKS`]. The implied
/// rate is printed so a pair of dwells taken at the same frame rate reads as inconclusive instead of
/// being mistaken for agreement between the two.
///
/// "Ticks", not "frames": this counts calls to [`tick`], which the recurring game task is expected
/// to make once per frame. The printed rate is what exposes that expectation if it is ever wrong —
/// a figure nowhere near the game's actual frame rate means the two have come apart, and the tick
/// column stops meaning what it says.
fn log_transition(abi: &ersc::Abi, previous: usize, state: u32, driven_by: Option<&str>) {
    let now = now_ms();
    let tick = TICKS.load(Ordering::SeqCst);
    let since_tick = tick.saturating_sub(LAST_TRANSITION_TICK.swap(tick, Ordering::SeqCst));
    let since_ms = now.saturating_sub(LAST_TRANSITION_MS.swap(now, Ordering::SeqCst));
    let first = previous == usize::MAX;
    crate::standalone_log(format_args!(
        "local-invasion: session state {} -> {:#04x} {}{}{}",
        if first {
            "(first read)".to_owned()
        } else {
            format!("{previous:#04x} {}", state_name(abi, previous as u32))
        },
        state,
        state_name(abi, state),
        // Suppressed on the very first line, where the "previous state" is the whole process
        // lifetime rather than a dwell and the number would invite exactly the wrong reading.
        if first {
            String::new()
        } else {
            format!(
                " -- held {since_tick} ticks / {since_ms}ms{}",
                match implied_fps(since_tick, since_ms) {
                    Some(fps) => format!(" (~{fps} fps)"),
                    None => String::new(),
                }
            )
        },
        match driven_by {
            Some(what) => format!(" (driven by us: {what})"),
            None => String::new(),
        },
    ));
}

/// Ticks per second over a dwell, or `None` when the interval is too short to divide meaningfully.
///
/// Kept out of [`log_transition`] so the rounding is testable without a game attached — the figure
/// exists to be compared against the game's real frame rate, and one that silently rounds to zero
/// would read as "the task stopped ticking".
#[must_use]
const fn implied_fps(ticks: u64, ms: u64) -> Option<u64> {
    if ms == 0 || ticks == 0 {
        return None;
    }
    Some(ticks.saturating_mul(1000) / ms)
}

/// Names for the three session states this module has evidence for, so the trace is readable
/// without a lookup. Anything else prints as a bare number rather than a guessed label -- the
/// state machine lives inside the Themida-virtualised dispatcher and most of it is simply unknown.
///
/// A `match` over the [`ersc::Abi`]'s fields rather than over constants, because an update moves
/// the numbers these names belong to: printing `SEARCHING` beside a code that means something else
/// on the loaded build would put a wrong reading into the one log the next diagnosis starts from.
fn state_name(abi: &ersc::Abi, state: u32) -> &'static str {
    match state {
        _ if state == abi.state_idle => "IDLE",
        _ if state == abi.state_searching => "SEARCHING",
        _ if state == abi.state_cancelling => "CANCELLING",
        _ => "(unreversed)",
    }
}

// ---------------------------------------------------------------------------------------------
// The one detour -- on the game
// ---------------------------------------------------------------------------------------------

/// `CS::SosSignMan::SetMultiplayJoinData(this, ServerPushJoinData*)`.
///
/// The seam the whole feature hangs on: the destination is decided, the server has told us, and
/// the player has not moved. The judgement happens before the original runs, so a reject is
/// decided against the incoming data rather than against a `CSGameMan` that has already been
/// written.
#[cfg(windows)]
unsafe extern "system" fn set_join_data_hook(a: usize, b: usize, c: usize, d: usize) -> usize {
    // Stamp the thread, so the owner id in `report_lock_preconditions` has a name to resolve to
    // rather than staying a bare number. This handler is the one `cancel_match` runs under.
    let _scope = enter_handler(HANDLER_JOIN_DATA);
    // The missing clock. This instant is the only honest "we got a connection" marker we have:
    // the server has pushed join data and the destination is decided. Every stall measured on
    // 2026-08-16 (53s, 59s, 91s, 213s) is time spent after this line with nothing to show for it,
    // and nothing in this DLL was timing it. ERSC's own state cannot substitute -- `0x15` is a
    // published flag bit, not a handshake stage.
    JOIN_DATA_AT_MS.store(now_ms(), Ordering::SeqCst);
    JOIN_PROGRESS_LAST.store(u64::MAX, Ordering::SeqCst);
    judge_incoming_match(b);
    let orig = ORIG_SET_JOIN_DATA.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    unsafe { core::mem::transmute::<usize, ErscActionFn>(orig)(a, b, c, d) }
}

/// The user's current config, for [`crate::lobby_publish`]'s hunt mode.
///
/// Shares the same hot-reloaded snapshot the reject filter judges with, so the two halves can never
/// disagree about what the user asked for -- a hunt filtering for one place while the reject filter
/// judged against another would be indistinguishable from a broken filter.
#[must_use]
pub fn current_config_snapshot() -> Option<LocalInvasionConfig> {
    current_config()
}

/// The advertisement lobby's `CSteamID`, read out of the resolved Seamless session.
///
/// Exposed so [`crate::lobby_publish`] can publish on the host's own lobby without hooking
/// `CreateLobby`: session resolution already re-validates the module fingerprint and the session
/// pointer on every use, and duplicating that elsewhere would mean two places to get stale.
///
/// A host session creates two lobbies -- one carrying the published data, one carrying the members.
/// This is the former, which is the one every `SetLobbyData` call was observed targeting.
#[cfg(windows)]
#[must_use]
pub fn advertisement_lobby_id() -> Option<u64> {
    let session = resolve_session().ok()?;
    let raw = unsafe {
        er_game_base::mem::safe_read_usize(
            session.session + crate::lobby_publish::SESSION_LOBBY_ID_OFFSET,
        )
    }?;
    // A zero id is "no lobby yet", not a lobby whose id happens to be zero.
    (raw != 0).then_some(raw as u64)
}

// ---------------------------------------------------------------------------------------------
// Judgement
// ---------------------------------------------------------------------------------------------

/// Resolve the anchor: where the player is, and what that location is called.
///
/// Returns `None` when the player's block cannot be read, which leaves matches alone.
#[cfg(windows)]
fn current_anchor() -> Option<InvasionAnchor> {
    let base = er_game_base::mem::game_module_base().ok()?;
    let block = unsafe { er_invasion_warp_core::warp::current_block_id(base) }?;
    // Place names are resolved from the injected pin registry, which carries the `PlaceName` text
    // id each synthetic row was labelled with. When the map has not been opened this session the
    // registry is empty and the anchor simply has no names -- which is correct rather than
    // degraded: exact-block mode does not consult names at all, and the name-based modes fail
    // closed on an empty anchor (`RejectReason::NothingToMatchAgainst`) instead of matching
    // everything.
    Some(InvasionAnchor::new(block, place_names_for_block(block)))
}

#[cfg(not(windows))]
fn current_anchor() -> Option<InvasionAnchor> {
    None
}

/// `PlaceName` text ids known for a block, from the injected pin registry.
fn place_names_for_block(block: u32) -> Vec<i32> {
    crate::map_hooks::registry_place_names_for_block(block)
}

/// One latch per distinct explanation, so saying one thing never silences the others.
///
/// A single shared latch was the first version's defect: whichever cause happened to arrive first
/// spent it, and every later rejection -- with a different cause and a different fix -- went
/// unexplained for the rest of the session.
static SAID_EMPTY_NAMED_LIST: AtomicUsize = AtomicUsize::new(0);
static SAID_MAP_NEVER_OPENED: AtomicUsize = AtomicUsize::new(0);
static SAID_BLOCK_HAS_NO_NAME: AtomicUsize = AtomicUsize::new(0);

/// Explain a rejection caused by missing information rather than by a wrong location, having first
/// established which information is missing.
///
/// From the player's seat every one of these looks the same -- nobody is hosting there -- and each
/// has a different fix, or none. The first version of this asserted a single cause ("open your
/// world map") for all of them without checking anything, which meant it confidently gave the
/// wrong advice in the most common case and made a false claim about `named` mode on the way past.
/// Diagnosing by asserting is the same error as the frozen telemetry document: an instrument that
/// reports a conclusion it never measured.
///
/// The three real causes, distinguished by state this function actually reads:
///
/// * `named` mode with an empty id list -- nothing to compare against, and the map cannot help
///   because opening it populates the pin registry, never `named_location_text_ids`.
/// * the pin registry is entirely empty -- the world map has not been built this session, so no
///   block anywhere has a name. Opening the map once fixes every subsequent match.
/// * the registry has names but not for this block -- that location carries no named invasion pin.
///   Opening the map again changes nothing; only `exact` mode, or marking the place, will help.
///
/// Rejecting in all three cases stays correct. Accepting a destination whose location cannot be
/// verified would land the player exactly where they filtered against. What was wrong was doing it
/// silently, and then explaining it wrongly.
fn explain_missing_names(reason: RejectReason, mode: LocalInvasionMode, destination: u32) {
    if !matches!(
        reason,
        RejectReason::CandidateUnnamed | RejectReason::NothingToMatchAgainst
    ) {
        return;
    }
    // `named` mode reaches `NothingToMatchAgainst` from an empty CONFIG list, before any name is
    // consulted. Nothing about the map is involved, so none of the map advice applies.
    if reason == RejectReason::NothingToMatchAgainst && mode == LocalInvasionMode::NamedOnly {
        if SAID_EMPTY_NAMED_LIST.swap(1, Ordering::SeqCst) == 0 {
            crate::standalone_log(format_args!(
                "local-invasion: mode = \"named\" with an EMPTY list rejects everything, including \
                 the location you are standing in -- it is stricter than \"exact\", not looser. \
                 Mark a place with Shift+Insert, or add ids to named_location_text_ids, or switch \
                 mode."
            ));
        }
        return;
    }
    let named_blocks = crate::map_hooks::registry_named_block_count();
    if named_blocks == 0 {
        if SAID_MAP_NEVER_OPENED.swap(1, Ordering::SeqCst) == 0 {
            crate::standalone_log(format_args!(
                "local-invasion: no location has a name yet, so every name-based judgement fails \
                 closed. Names are read off the world map's own rows -- OPEN YOUR WORLD MAP ONCE \
                 and matches will judge normally. `exact` mode never needs them."
            ));
        }
        return;
    }
    if SAID_BLOCK_HAS_NO_NAME.swap(1, Ordering::SeqCst) == 0 {
        crate::standalone_log(format_args!(
            "local-invasion: {named_blocks} location(s) have names, but {destination:#010x} is not \
             one of them -- that block carries no named invasion pin, so `area` and `named` cannot \
             judge it and it will keep being rejected. Opening the map again will not change this: \
             use `exact`, or mark the place with Insert."
        ));
    }
}

/// Judge an incoming match and cancel it if the user's rules say so.
///
/// `join_data` is the `ServerPushJoinData*` from `SetMultiplayJoinData`'s second argument.
pub fn judge_incoming_match(join_data: usize) {
    let Some(config) = current_config() else {
        return;
    };

    // The reads come first, and the switch gates the action rather than the banner.
    //
    // This used to return here when the filter was off, which made the on-screen notice a
    // by-product of filtering: switch the mod off and the banner went with it. The banner is a
    // status surface in its own right -- where the server just sent you is worth saying whether or
    // not any rule was applied -- so only `cancel_match` is gated below. Nothing here writes to the
    // game, and the reads are fault-closed, so an off filter still touches no match.

    // `safe_read_i32` is the widest fault-tolerant read this base crate exposes; the block id is
    // a bit pattern, so the sign reinterpretation is meaningless and the cast is exact.
    let Some(destination) = (unsafe {
        er_game_base::mem::safe_read_i32(
            join_data + crate::map_seams::JOIN_DATA_DESTINATION_BLOCK_OFFSET,
        )
    })
    .map(|raw| raw as u32) else {
        crate::standalone_log(format_args!(
            "local-invasion: join data unreadable -- match left alone"
        ));
        return;
    };

    let Some(anchor) = current_anchor() else {
        crate::standalone_log(format_args!(
            "local-invasion: anchor unresolved -- match to {destination:#010x} left alone"
        ));
        return;
    };

    // Every name the destination carries, not one of them. This was `.first()` of the list --
    // over a `BTreeSet` that is the numerically smallest id -- while the anchor compared against
    // all of its own names, so a destination sharing a name through any other of its names was
    // rejected as `WrongPlaceName`.
    if !config.enabled {
        // Nothing was judged, so there is no verdict to report -- just the destination, stated as
        // the server's choice rather than as anything the mod approved.
        banner::announce_arrival(config.reject_notice, destination);
        // Named here as well as on a rejection. With `enabled = false` nothing is judged, so the
        // reject path never runs, and this is the only place the host read gets exercised.
        // A `None` needs no line here: `host_steam_id` already logged which read failed.
        if let Some(id) = host_steam_id() {
            crate::standalone_log(format_args!(
                "local-invasion: the host of this match is Steam id {id} ({id:#018x}), read from \
                 the Seamless session"
            ));
        }
        return;
    }

    // A match is the only moment the session is both needed and likely to exist, so this is
    // where the sweeper's budget is refilled. Spending passes on a timer at startup found nothing
    // and then stopped: run br-20260908-210312-80d7 reported `cannot cancel -- MenuNeverOpened`
    // for a rejection judged minutes after the last pass had been spent.
    #[cfg(windows)]
    session_scan::request_sweep_now();
    // A match is in flight from here until the join settles. While it is, a scan refuses an idle
    // candidate -- see `identifies_a_session` for why that is a fact about ERSC rather than a
    // guess. The cached answer is not thrown away here. It used to be, and that single line is
    // what cost run br-20260908-212740-ea7c its enforcement: the session was resolving at
    // `0x55080038` right up to the match, this cleared it, and the rejection two lines later
    // could not cancel because nothing was left to cancel through. `cached_scan_for_session`
    // already re-checks the pointer on every call and drops it if it stops identifying, so a dead
    // session is discarded on evidence rather than on the arrival of the event that needs it.
    JOIN_IN_FLIGHT.store(true, Ordering::SeqCst);
    // The join has started, so the real session has just left idle. Re-read the addresses recorded
    // while nothing was happening and keep only the ones that moved -- see `differential_scan` for
    // why that is the only test a look-alike object cannot pass.
    #[cfg(windows)]
    if let Some(abi) = resolve_ersc_abi()
        && let Some(session) = differential_scan::narrow_to_changed(abi)
    {
        crate::standalone_log(format_args!(
            "local-invasion: session identified by CHANGE at {session:#x} -- it read idle before \
             this join and an active state after it, which is what `ersc+0x25850` does to the \
             field and what nothing else in the address space did. Adopting it over the shape \
             scan's answer."
        ));
        session_scan::adopt_proven_session(session);
    }
    let candidate = InvasionCandidate::new(destination, place_names_for_block(destination));
    match config.judge(&anchor, &candidate) {
        Verdict::Keep(reason) => {
            KEEPS.fetch_add(1, Ordering::SeqCst);
            // The search that just landed is over; nothing to re-arm.
            //
            // Unless the join then dies, which is what `KEPT_JOIN_PENDING` is for. Reported live
            // 2026-09-10: "attempt, stall, wait 30 seconds, failed to invade, and invasion loop
            // canceled". Measured on run br-20260910-022619-1fa2 -- `KEEP 0x0f000000 (ExactBlock)`
            // at line 1193, `lobby=4 proto=6 rpc=5 -> Progressing` at 1194, and 30830ms later
            // everything zero. The hunt disarmed on the accept and nothing armed it again, so a
            // join the player never got to play ended their session's hunting outright.
            KEPT_JOIN_PENDING.store(true, Ordering::SeqCst);
            AUTO_SEARCH_ARMED.store(false, Ordering::SeqCst);
            PENDING_REINVADE.store(false, Ordering::SeqCst);
            crate::standalone_log(format_args!(
                "local-invasion: KEEP {destination:#010x} ({reason:?}); anchor {:#010x} with {} \
                 named location(s)",
                anchor.block,
                anchor.named_location_count()
            ));
            // The banner for this does not fire here. A kept match is a match we allowed, not an
            // invasion that happened: measured 2026-08-16, joins sat dead for 53-213s after this
            // exact instant. Saying "Invasion successful" at join time can therefore be a lie. It
            // is announced from the tick instead, when the engine reports `LobbyState::Client` and
            // the join has demonstrably landed.
            PENDING_SUCCESS_BLOCK.store(destination as usize, Ordering::SeqCst);
        }
        Verdict::Reject(reason) => {
            crate::standalone_log(format_args!(
                "local-invasion: REJECT {destination:#010x} ({reason:?}); anchor {:#010x} with {} \
                 named location(s), destination with {}, mode={}",
                anchor.block,
                anchor.named_location_count(),
                candidate.named_location_count(),
                config.mode.as_str()
            ));
            explain_missing_names(reason, config.mode, destination);
            // The host, named. Logged before it is put on a banner, because the read is new and a
            // wrong id printed to the player is worse than no id at all. `None` here is the honest
            // answer for a target list the engine has already emptied.
            if let Some(id) = host_steam_id() {
                crate::standalone_log(format_args!(
                    "local-invasion: the host of that match is Steam id {id} ({id:#018x}), read \
                     from the Seamless session while the negotiation is still open"
                ));
            }
            // The banner fires on the verdict, and the wording is what makes that honest.
            //
            // It used to fire here saying "Rejected", before the cancel was attempted, so a path
            // that then declined to cancel had already told the player the invasion was stopped --
            // reported live 2026-09-04: "the popup tells me I'm rejecting a location to invade but
            // it still invades it". The fix at the time was to move the banner behind a successful
            // cancel, and that traded one wrong answer for a worse one: a rejection that could not
            // be cancelled showed nothing, which is indistinguishable from the mod not being
            // loaded, and is how the whole evening of 2026-09-09 was spent.
            //
            // Both failures come from one banner trying to say two things. The verdict and the
            // enforcement are separate facts, and the player is owed the first one every single
            // time: this match is not local. Whether Seamless could be driven to drop it is the
            // second fact, and `drive_pending_cancel` still owns it.
            banner::announce_verdict(config.reject_notice, destination, reason);
            // Refuse the join itself rather than cancel it afterwards. `SosSignMan::JoinSession`
            // calls this seam and then `CSSessionManager::JoinSession`; once that second call has
            // run, `lobbyState` is `Joining` and the engine parks any disconnect request until the
            // Steam RPC resolves on its own -- 30.2s and 30.3s on the two rejections of run
            // br-20260910-012622-fd23 whose RPC never came back. See `CS_SESSION_MANAGER_JOIN_SESSION`.
            REFUSE_NEXT_JOIN.store(true, Ordering::SeqCst);
            arm_pending_cancel(destination, reason, config.reject_notice);
        }
    }
}

/// `SeamlessSession +0x1d8` -- the Steam id of the host this attempt is negotiating with.
///
/// # How this offset was established
///
/// Not by reading a struct definition. The DLL's own session field-change telemetry caught the
/// write twice in run br-20260910-042516-3b5d, at the same transition both times: entering state
/// `0x16` writes `+0x1d0` and `+0x1d8` in one step, and the return to idle zeroes both. `+0x1d0`
/// takes the same value `+0x1a8` held one state earlier, which is the lobby; `+0x1d8` takes a
/// value whose high 32 bits are `0x01100001`, which is what every SteamID64 for an individual
/// account begins with. The two invasions in that run gave two different ids
/// (`0x011000013cfa853f` and `0x0110000109940bbb`) against two different lobbies, so the field
/// carries an identity rather than a constant.
///
/// The timing is what makes it usable for a verdict: `SetMultiplayJoinData` fires after that
/// transition, not before -- in the log the announce line sits three lines below the `0x16` field
/// change -- so the id is already present when a match is judged, on the accept path and the
/// reject path alike.
#[cfg(windows)]
const SESSION_HOST_STEAM_ID_OFFSET: usize = 0x1d8;

/// The Steam id of the host this attempt is negotiating with, or `None`.
///
/// # Why the route through `CSNetMan` is gone rather than corrected
///
/// It walked `CSNetMan -> BreakInManager -> targets`, and `[netMan+0xa8]` read 0 on a real
/// invasion. That was not a drifted offset: an RTTI sweep of the live process -- every type
/// descriptor in the image, matched by name -- found no `CSBreakInManager` class in this build at
/// all. The break-in state belongs to `FNBreakInImpl@FromNet`, reached through the `FNClientImpl`
/// at `CSNetMan+0x00`, so no correction to `0xa8` could have worked. Seamless already holds the
/// answer one dereference away, so the game-side walk buys nothing.
/// Host-side stub: there is no Seamless session off the game, so no match has a host.
#[cfg(not(windows))]
fn host_steam_id() -> Option<u64> {
    None
}

#[cfg(windows)]
fn host_steam_id() -> Option<u64> {
    let session = match resolve_session() {
        Ok(session) => session,
        Err(cause) => {
            crate::standalone_log(format_args!(
                "local-invasion: the host cannot be named -- no Seamless session ({cause:?})"
            ));
            return None;
        }
    };
    // SAFETY: the fault-closed reader returns None rather than faulting on a bad address.
    let raw = unsafe {
        er_game_base::mem::safe_read_usize(session.session + SESSION_HOST_STEAM_ID_OFFSET)
    };
    match raw {
        Some(id) if id != 0 => Some(id as u64),
        other => {
            crate::standalone_log(format_args!(
                "local-invasion: the host cannot be named -- session={:#x}                  +{SESSION_HOST_STEAM_ID_OFFSET:#x} read {other:?}, so the engine has not written                  the host yet or this is not the session that holds it",
                session.session
            ));
            None
        }
    }
}

/// Whether a match is between its join data and its settlement.
///
/// Read by [`identifies_a_session`], which refuses an idle session while it is set. Cleared when
/// the engine's join reaches a terminal state, so the ordinary between-invasions case -- where a
/// real session legitimately reads idle -- is unaffected.
static JOIN_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Ticks a rejection waits for the sweeper to produce a session before it is counted unenforced.
///
/// The sweeper takes a few seconds and a join takes ten or more, so this is a window that fits
/// inside the time the match is still cancellable, not a hopeful retry loop.
const CANCEL_RETRY_TICKS: usize = 600;
/// How long the current rejection has been waiting.
static CANCEL_RETRIES: AtomicUsize = AtomicUsize::new(0);

/// A rejection judged but not yet cancelled, waiting for the game task to drive it.
///
/// # Why the cancel does not fire where the verdict is reached
///
/// [`judge_incoming_match`] runs inside `set_join_data_hook`, a detour on the game's own
/// `CS::SosSignMan::SetMultiplayJoinData`. Driving an ERSC action from there means calling into
/// `ersc.dll` from a frame the game entered, in whatever session state the server's offer left
/// behind -- and that state is measurably outside the set ERSC's hide-predicate draws its Cancel row
/// for. Arming here and firing from the recurring `CSTaskImp` task moves the call to a frame that
/// owns nothing of Seamless's, and lets [`cancel_row_refusal`] see a settled state rather than one
/// mid-transition.
///
/// One slot, not a queue: a second rejection arriving before the first has fired replaces it. The
/// newest offer is the one the server is waiting on, and cancelling a match that has already been
/// superseded would drive the action for nothing.
static PENDING_CANCEL: Mutex<Option<PendingCancel>> = Mutex::new(None);

/// The three things the deferred cancel needs to finish the job the verdict started.
#[derive(Clone, Copy)]
struct PendingCancel {
    destination: u32,
    reason: RejectReason,
    /// Whether the player asked for the on-screen notice, captured with the verdict rather than
    /// re-read at fire time: the config can change between the two, and the banner belongs to the
    /// decision it describes.
    notice: bool,
}

/// Record a rejection for the game task to act on.
fn arm_pending_cancel(destination: u32, reason: RejectReason, notice: bool) {
    let mut guard = match PENDING_CANCEL.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(previous) = guard.replace(PendingCancel {
        destination,
        reason,
        notice,
    }) {
        crate::standalone_log(format_args!(
            "local-invasion: a second rejection arrived before the first was driven -- \
             {:#010x} ({:?}) is dropped in favour of {destination:#010x} ({reason:?}). The server \
             is waiting on the newer offer; cancelling a superseded one drives ERSC for nothing.",
            previous.destination, previous.reason
        ));
    }
}

/// Fire the armed cancel from the game task, and announce only what actually happened.
///
/// The banner is emitted here rather than at the verdict for the reason recorded on
/// [`cancel_match`]'s return value: a notice that says "rejected" when nothing was cancelled is
/// the one signal the player has, and it used to be able to lie.
fn drive_pending_cancel() {
    let armed = {
        let mut guard = match PENDING_CANCEL.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.take()
    };
    let Some(armed) = armed else {
        return;
    };
    if cancel_match(armed.reason) {
        banner::announce_rejection(armed.notice, armed.destination, armed.reason);
        return;
    }
    // A session that is not resolvable yet is not the same as a refusal, and discarding the
    // rejection on the first tick threw away the whole window in which it could still be
    // cancelled. The sweeper runs on its own thread and takes seconds; the join takes ten or
    // more, so re-arming costs nothing and buys the only chance this rejection has.
    //
    // Bounded, because a rejection that can never be driven must still end as an honest
    // unenforced count rather than sitting armed forever and silently replacing the next one.
    if matches!(resolve_session(), Err(NoSession::SessionNotIdentified)) {
        let waited = CANCEL_RETRIES.fetch_add(1, Ordering::SeqCst) + 1;
        if waited <= CANCEL_RETRY_TICKS {
            let mut guard = match PENDING_CANCEL.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = Some(armed);
            return;
        }
    }
    CANCEL_RETRIES.store(0, Ordering::SeqCst);
    UNENFORCED_REJECTS.fetch_add(1, Ordering::SeqCst);
    crate::standalone_log(format_args!(
        "local-invasion: NOT cancelled {:#010x} ({:?}) -- the match was judged a rejection but \
         Seamless was not driven, so the invasion PROCEEDS. No banner is shown: a notice saying \
         the filter could not act is the failure dressed as a feature, and it was rejected on \
         sight -- \"you literally added feature text encapsulating in short 'I am useless'\". The \
         reason is logged immediately above this line.",
        armed.destination, armed.reason
    ));
}

/// Publish whether an invasion attempt is in flight, for the warp gate and the map's icon choice.
///
/// # Why "not idle" and not "== SEARCHING"
///
/// Searching is only the first state of an attempt. The sequence runs `0x0e` through `0x0f`,
/// `0x12`, the `0x13` offer, `0x14`, `0x15`, and a cancel unwinds via `0x23`/`0x24`
/// -- and the player is just as committed at every one of them as at the
/// first. Gating on `SEARCHING` alone would unblock the warp the instant a host was found, which
/// is the worst possible moment for it: the destination has been decided and the player is about
/// to be moved there by Seamless.
///
/// Anything that is not [`ersc::Abi::state_idle`] therefore counts, including the states no
/// instruction in ersc's plaintext `.text` writes (its middle is virtualised). That is the safe
/// direction for an unknown state: an unrecognised value means something is happening, and the
/// honest response to "I do not know what this state is" is to leave the pins alone.
///
/// A state that cannot be read at all is treated as no attempt, matching the no-session case: a
/// read that fails is not evidence of an invasion.
#[cfg(windows)]
fn publish_invasion_attempt_state(session: SeamlessSession) {
    // A zero state is "NOTHING", not "AN ATTEMPT". The rule below is "anything that is not idle
    // counts", which is the right conservative direction for an unknown state -- but zero is not
    // unknown, it is uninitialised, and treating it as an invasion in progress locks the warp gate
    // shut forever. That is exactly what a player hit on 2026-09-04: every map marker refused with
    // "an invasion attempt is in flight", from a session whose state never left 0x00.
    let in_flight = read_session_state(session.abi, session.session)
        .is_some_and(|state| state != 0 && state != session.abi.state_idle);
    er_invasion_warp_core::warp::set_invasion_attempt_in_flight(in_flight);
}

/// The gate the popup skip uses: the state of the session reached from the option-menu object
/// Seamless itself handed over, with no scan, no cache and no discard anywhere in the path.
///
/// # Why this is not [`session_is_idle`]
///
/// Measured on run `br-20260909-233549-72f5`. `session_is_idle` goes through `resolve_session`,
/// which -- with `OSM` unset -- falls back to `cached_scan_for_session`, a heuristic that
/// recognises a session by the shape of its state field. In that run the scan picked a look-alike,
/// one invade was driven on it, and four samples later the same pointer was discarded with "this
/// pointer is not one". From that moment `session_is_idle` answered false forever, so the popup
/// skip fired exactly once and every later use of the item silently took the pass-through branch
/// and showed the dialog. The Frida prototype never had that failure because it read the session
/// from the object `show` was called with, which is what this reads.
///
/// `false` when the menu object has not been seen yet: refusing to skip shows the dialog, which is
/// the harmless direction. Guessing idle from an unknown pointer would swallow the prompt that
/// leaves an invasion.
#[cfg(windows)]
#[must_use]
pub fn menu_object_session_is_idle() -> bool {
    let osm = OSM.load(Ordering::SeqCst);
    if osm == 0 {
        return false;
    }
    let Some(abi) = resolve_ersc_abi() else {
        return false;
    };
    // SAFETY: fault-closed read of the hop `show`'s own argument leads through.
    let Some(session) =
        (unsafe { er_game_base::mem::safe_read_usize(osm + ersc::NEXT_OBJECT_OFFSET) })
    else {
        return false;
    };
    read_session_state(abi, session).is_some_and(|state| state == abi.state_idle)
}

/// Whether the auto re-search loop is running.
///
/// The popup skip asks, because a player opening the invasion item while a hunt is in flight is
/// reaching for Cancel, not for another search.
#[must_use]
pub fn auto_search_armed() -> bool {
    AUTO_SEARCH_ARMED.load(Ordering::SeqCst)
}

/// Stand the auto re-search loop down, because the player asked for the menu.
pub fn stand_down_auto_search() {
    if AUTO_SEARCH_ARMED.swap(false, Ordering::SeqCst) {
        PENDING_REINVADE.store(false, Ordering::SeqCst);
        crate::standalone_log(format_args!(
            "local-invasion: you opened the invasion item's own menu -- auto re-search stood down, \
             so the row in front of you is Seamless's and nothing here will act while you decide"
        ));
    }
}

/// The gate the popup skip actually calls: the menu object's own session when Seamless has handed
/// one over, and otherwise the filter's non-detour resolution.
///
/// # Why there is a fallback at all
///
/// Without one the feature is dead rather than cautious. `OSM` is set only by a detour inside
/// `ersc.dll`, and installing the `show` detour to get it killed the game 29.5s into run
/// `br-20260909-234159-0a54` -- a fault inside the fault handler, so the loading screen never
/// finished. With that detour correctly gone, `menu_object_session_is_idle` can only ever answer
/// false, so every dialog is let through and the skip never fires once.
///
/// [`session_is_idle`] is the weaker source -- it can ride a scanned look-alike that is later
/// discarded, which is how the skip fired exactly once on run `br-20260909-233549-72f5` -- but
/// weaker is not useless: that same route drove the first use of that run correctly, and it needs
/// no detour. So it is consulted second, never first, and the caller says which source answered.
#[cfg(windows)]
#[must_use]
pub fn popup_skip_gate_is_idle() -> (bool, &'static str) {
    if OSM.load(Ordering::SeqCst) != 0 {
        return (
            menu_object_session_is_idle(),
            "the menu object Seamless handed over",
        );
    }
    // A discard that happened before the menu object arrived says nothing about the object that
    // arrived after it. The latch exists to distrust a scanned pointer once the scan has been
    // wrong; `OSM` is not a scan result -- `capture_osm` only stores a pointer whose `+0x58` leads
    // to an object carrying a live session state, handed over by Seamless calling its own action.
    //
    // Without this the latch is a run-long death sentence for the popup skip: the scan is running
    // from boot and discards long before the player first opens the item, so by the time the real
    // object is adopted the fallback is already disabled and `popup_skip_gate_is_idle` answers
    // "nothing trustworthy" for the rest of the session. That is a bug this module introduced on
    // 2026-09-09 in the same change that added the latch.
    if OSM.load(Ordering::SeqCst) != 0 {
        SESSION_EVER_DISCARDED.store(0, Ordering::SeqCst);
    }
    if SESSION_EVER_DISCARDED.load(Ordering::SeqCst) != 0 {
        return (
            false,
            "nothing trustworthy -- no menu object, and the filter has already discarded a scanned \
             session as \"not one\", so its later answers may not suppress a dialog",
        );
    }
    (
        session_is_idle(),
        "the filter's own session (no menu object seen -- weaker, but not yet discarded)",
    )
}

/// Host-side stub: there is no Seamless to read a session out of.
#[cfg(not(windows))]
#[must_use]
pub fn menu_object_session_is_idle() -> bool {
    false
}

/// Host-side stub, as [`menu_object_session_is_idle`].
#[cfg(not(windows))]
#[must_use]
pub fn popup_skip_gate_is_idle() -> (bool, &'static str) {
    (false, "not windows")
}

/// True once the session has settled back to idle after a cancel.
#[must_use]
pub fn session_is_idle() -> bool {
    resolve_session()
        .ok()
        .and_then(|session| read_session_state(session.abi, session.session).zip(Some(session)))
        .is_some_and(|(state, session)| state == session.abi.state_idle)
}

// ---------------------------------------------------------------------------------------------
// ERSC resolution
// ---------------------------------------------------------------------------------------------

/// ERSC's runtime base. `ersc.dll` is RELOCATABLE -- there is no fixed load address -- so every
/// ERSC address in this module is `this + RVA`, resolved fresh. `None` means Seamless is not
/// loaded, in which case there are no Seamless invasions to filter and the feature stays inert.
#[cfg(windows)]
fn ersc_module_base() -> Option<usize> {
    unsafe extern "system" {
        fn GetModuleHandleA(name: *const u8) -> isize;
    }
    let handle = unsafe { GetModuleHandleA(c"ersc.dll".as_ptr().cast()) };
    (handle != 0).then_some(handle as usize)
}

#[cfg(not(windows))]
fn ersc_module_base() -> Option<usize> {
    None
}

/// Resolve one ERSC action, refusing to hand back a pointer whose opening bytes are not the ones
/// read out of the build this module recognised. A Seamless update that moves these functions
/// disarms the filter; it must never make it call into the middle of an instruction.
///
/// The `prologue` here is not a prologue in the "opening few bytes" sense: it runs all the way
/// through the action's state write. Five different functions share the first fourteen
/// bytes, so a short check would prove only that some option action is at this address -- and
/// calling the wrong one cancels other players' invasions.
/// Action rvas whose byte check has already been reported as failing.
///
/// # Why this is latched
///
/// The refusal below is a property of the loaded `ersc.dll`, not of the moment: once a build's
/// bytes do not match, they do not match on the next tick either. It was logged unconditionally
/// from a per-tick caller, and run br-20260909-005147-56c1 came back with 6061 identical copies of
/// it -- half of a 12826-line log. A message that repeats six thousand times is not a diagnostic,
/// it is what buries one. Tracked as bd er-effects-rs-m86t.
///
/// Keyed by rva so a second action refusing for a different reason still gets its own line, and
/// cleared when the bytes read correctly again so a recovery is reported once too.
static REFUSED_ACTION_RVAS: std::sync::Mutex<std::collections::BTreeSet<usize>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Record that `rva` refuses, and answer whether this is the first time since it last worked.
fn first_refusal_of(rva: usize) -> bool {
    match REFUSED_ACTION_RVAS.lock() {
        Ok(mut refused) => refused.insert(rva),
        // A poisoned latch must not silence the refusal: saying it twice is recoverable, saying it
        // never is the bug this exists to fix.
        Err(poisoned) => poisoned.into_inner().insert(rva),
    }
}

/// Forget a refusal, and answer whether one was standing.
fn clear_refusal_of(rva: usize) -> bool {
    match REFUSED_ACTION_RVAS.lock() {
        Ok(mut refused) => refused.remove(&rva),
        Err(poisoned) => poisoned.into_inner().remove(&rva),
    }
}

fn ersc_action(abi: &ersc::Abi, rva: usize, prologue: &[u8]) -> Option<ErscActionFn> {
    if inside_ersc_callback() {
        crate::standalone_log(format_args!(
            "local-invasion: refusing to hand out ersc+{rva:#x} -- this thread is inside a callback \
             ersc.dll made, so calling back into it would run its code from a frame it entered, \
             with whatever locks and half-finished state that call left behind. The action is \
             declined, not queued: the tick will reach it on a frame of the game's own."
        ));
        return None;
    }
    let base = ersc_module_base().or_else(|| {
        crate::standalone_log(format_args!(
            "local-invasion: ersc.dll not loaded -- nothing to filter"
        ));
        None
    })?;
    let address = base + rva;
    if !prologue_matches(address, prologue) {
        if first_refusal_of(rva) {
            crate::standalone_log(format_args!(
                "local-invasion: ersc+{rva:#x} does not hold the {} bytes this module measured for \
                 {} -- refusing to call it. The filter is disarmed until the RVAs are re-read \
                 against this ersc.dll: uv run --with capstone python3 \
                 scripts/locate-ersc-entry-points.py. This line is printed once per change, not \
                 once per tick.",
                prologue.len(),
                abi.version,
            ));
        }
        return None;
    }
    if clear_refusal_of(rva) {
        crate::standalone_log(format_args!(
            "local-invasion: ersc+{rva:#x} reads as itself again -- the action is callable"
        ));
    }
    Some(unsafe { core::mem::transmute::<usize, ErscActionFn>(address) })
}

fn prologue_matches(address: usize, expected: &[u8]) -> bool {
    expected.iter().enumerate().all(|(index, byte)| {
        unsafe { er_game_base::mem::safe_read_u8(address + index) }.is_some_and(|got| got == *byte)
    })
}

// ---------------------------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------------------------

/// `CS::CSSessionManager::JoinSession(this, MultiplayType, FnVector<byte>* lobbyIds, int limit)`.
///
/// Returns `false` without issuing the Steam join RPC when the match was just rejected, and calls
/// the original otherwise. Refusing here is what makes a rejection instant: after the original has
/// run there is no path back that does not wait out the RPC.
#[cfg(windows)]
unsafe extern "system" fn join_session_hook(a: usize, b: usize, c: usize, d: usize) -> usize {
    if REFUSE_NEXT_JOIN.swap(false, Ordering::SeqCst) {
        let refused = JOINS_REFUSED.fetch_add(1, Ordering::SeqCst) + 1;
        crate::standalone_log(format_args!(
            "local-invasion: refused the join before the RPC was sent (#{refused}) -- the engine \
             stays at its current lobby state with nothing outstanding, so the search resumes \
             without the 30s the parked disconnect would have cost"
        ));
        return 0;
    }
    let orig = ORIG_JOIN_SESSION.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    // SAFETY: the union stored the trampoline for this exact target.
    unsafe { core::mem::transmute::<usize, er_hook::UnionFn>(orig)(a, b, c, d) }
}

/// Hook `CS::CSSessionManager::JoinSession`. Idempotent; returns 1 on success.
///
/// Its absence is not fatal the way `install_join_hook`'s is: without it a rejection still cancels
/// through Seamless, it just costs the player the engine's own 30s wait.
#[cfg(windows)]
fn install_join_session_hook() -> usize {
    if JOIN_SESSION_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return 0;
    }
    let seam = crate::map_seams::CS_SESSION_MANAGER_JOIN_SESSION;
    let address = match unsafe { crate::map_seams::verify_seam(&seam) } {
        Ok(address) => address,
        Err(error) => {
            crate::standalone_log(format_args!(
                "local-invasion: {error} -- rejections will still cancel, but each one costs the \
                 engine's own 30s join wait"
            ));
            return 0;
        }
    };
    match unsafe {
        er_hook::register_union_hook(
            address,
            join_session_hook as er_hook::UnionFn,
            &ORIG_JOIN_SESSION,
        )
    } {
        Ok(()) => {
            crate::standalone_log(format_args!(
                "local-invasion: refusing rejected joins at {} @0x{address:x} (the seam's own \
                 address; the HOOK TRANSLATED line above names where the detour went)",
                seam.name
            ));
            1
        }
        Err(status) => {
            crate::standalone_log(format_args!(
                "local-invasion: union registration for {} failed: {status:?} -- rejections fall \
                 back to cancelling after the join, which costs 30s when the RPC does not resolve",
                seam.name
            ));
            0
        }
    }
}

/// Hook `CS::SosSignMan::SetMultiplayJoinData`. Idempotent; returns 1 on success.
///
/// This is the hook that makes the feature exist. Without it the filter never sees a match and the
/// whole module is decoration -- so its failure is logged as a failure, not a note.
#[cfg(windows)]
fn install_join_hook() -> usize {
    if JOIN_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return 0;
    }
    let seam = crate::map_seams::SET_MULTIPLAY_JOIN_DATA;
    let address = match unsafe { crate::map_seams::verify_seam(&seam) } {
        Ok(address) => address,
        Err(error) => {
            crate::standalone_log(format_args!(
                "local-invasion: {error} -- WITHOUT THIS HOOK THE FILTER NEVER SEES A MATCH"
            ));
            return 0;
        }
    };
    match unsafe {
        er_hook::register_union_hook(
            address,
            set_join_data_hook as er_hook::UnionFn,
            &ORIG_SET_JOIN_DATA,
        )
    } {
        Ok(()) => {
            // Say which address this is. `address` is the seam's own 1.16.2 address;
            // `register_union_hook` resolves it for the running build before installing anything
            // and logs its own `HOOK TRANSLATED` line naming where the detour actually went. This
            // line used to print the untranslated value with no qualifier, directly beneath that
            // translation line -- so on 1.17 the log read `... -> 0x1406fc370` followed by
            // `judging matches at ... @0x1406fb520`, which is exactly the shape of a hook left
            // behind on a stale address. It cost the 2026-09-06 investigation its opening
            // hypothesis: the detour was correct the whole time and had already fired four times.
            crate::standalone_log(format_args!(
                "local-invasion: judging matches at {} @0x{address:x} (the seam's own address; the \
                 HOOK TRANSLATED line above names where the detour went on this build)",
                seam.name
            ));
            1
        }
        Err(status) => {
            crate::standalone_log(format_args!(
                "local-invasion: union registration for {} failed: {status:?} -- THE FILTER IS \
                 INERT; every match will land wherever the server sends it",
                seam.name
            ));
            0
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Hotkeys
// ---------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------
// Per-frame entry point
// ---------------------------------------------------------------------------------------------

/// One tick of the filter, called from the DLL's recurring game task.
///
/// Everything here is cheap and idempotent: two install latches, a hotkey poll, and a queued
/// re-invade that only does work when one is actually pending.
///
/// # Safety
///
/// Game task thread, with the runtime up.
#[cfg(windows)]
pub unsafe fn tick(keys: &mut MarkKeys, game_has_focus: bool) {
    // Stamp the thread, so the owner id in `report_lock_preconditions` has a name to resolve to
    // rather than staying a bare number. This handler is the one `drive_pending_reinvade` and
    // `cancel_stalled_attempt` run under.
    let _scope = enter_handler(HANDLER_GAME_TASK);
    // Counted before any early return, so a dwell measured across a stretch where Seamless was not
    // resolvable still reflects the frames that actually passed.
    TICKS.fetch_add(1, Ordering::SeqCst);
    install_join_hook();
    install_join_session_hook();
    // The two detours this DLL places inside `ersc.dll`, both withheld by one key. Read once per
    // tick rather than cached at attach, because the config is re-read when a match arrives and a
    // player mid-A/B should not have to restart the game to move the switch.
    //
    // Defaulting to on when the snapshot is unavailable keeps the pre-config behaviour: the
    // snapshot is absent before the first successful read, and the `show` observer is what finds
    // the Seamless menu object, so failing closed here would disarm the filter on every launch
    // during the window where nothing has gone wrong yet.
    struct ErscObservers {
        show: bool,
        lobby_key: bool,
        invade: bool,
    }
    let ersc_observers = current_config_snapshot().map(|config| ErscObservers {
        show: config.ersc_observers && config.ersc_show_observer,
        lobby_key: config.ersc_observers && config.ersc_lobby_key_observer,
        invade: config.ersc_observers && config.ersc_invade_observer,
    });
    // The two seams observe the same pointer and neither is guaranteed to run, so they have
    // separate keys rather than one: `show` sees the object when the player opens Seamless's
    // menu, the invade action when they use an invasion item, and a player who only ever uses
    // the item gets nothing at all from `show`.
    if ersc_observers.as_ref().map(|c| c.invade).unwrap_or(true) {
        menu_object::install_invade_observer();
    }
    if ersc_observers.as_ref().map(|c| c.show).unwrap_or(true) {
        install_show_observer();
    }
    // Learns the live announcement view. Self-gating and idempotent: it needs the menu system,
    // which does not exist at attach, so it retries until it lands rather than failing silently
    // once. Costs one byte-check per tick until then.
    crate::announce::install();
    // Read back what the game measured for the last notice. Deliberately outside the Seamless
    // early-return below: a notice can be on screen while the session is unresolvable, and the
    // whole point of this check is that it does not depend on the path that placed the notice.
    crate::announce::poll_measurement();
    // Read-only, and independent of the filter: it reports the one string that decides whether two
    // Seamless players can find each other at all.
    if ersc_observers.as_ref().map(|c| c.lobby_key).unwrap_or(true) {
        install_lobby_key_observer();
    }
    if game_has_focus {
        keys.poll();
    } else {
        keys.forget();
    }
    // Phase 1 measurement, above the Seamless gate on purpose. These are engine fields: they do
    // not depend on ERSC being resolvable, and the baseline of what they read during ordinary play
    // is exactly as valuable as what they read mid-attempt. Gating them behind `resolve_session`
    // would have recorded nothing at all until the player opened the Seamless menu.
    trace_join_progress(match resolve_session() {
        Ok(s) => read_session_state(s.abi, s.session)
            .map(|state| (state, s.abi.state_idle))
            .ok_or("<state-unreadable>"),
        Err(reason) => Err(reason.label()),
    });
    // Before the early return below, and that placement is the whole point. A rejection is armed
    // from the join-data detour and driven here, and it used to sit under a `resolve_session`
    // guard -- so a session that stopped resolving between the verdict and this tick stranded the
    // armed cancel forever. Nothing said so: `drive_pending_cancel` never ran, so it never
    // reached the line that counts an unenforced rejection, and the heartbeat printed
    // `UNENFORCED=0` while holding one.
    //
    // Measured live, run br-20260908-201137-0567: one `REJECT 0x3d302b00 (WrongBlock)`, then nine
    // consecutive `ersc=<menu-never-opened>` samples, `UNENFORCED=0`, no `NOT cancelled` line,
    // and the invasion proceeded. The filter judged the match correctly and dropped the verdict on
    // the floor.
    //
    // `cancel_match` resolves the session itself and says plainly when it cannot, which is the
    // honest failure this path was missing. Attempting and reporting beats returning early.
    drive_pending_cancel();
    // Everything below is Seamless-side and purely observational until a rejected match has
    // actually armed a re-search, so a run without Seamless loaded costs one failed module lookup.
    let Ok(session) = resolve_session() else {
        // No session means no attempt, which is a definite answer rather than a failure to read
        // one: with Seamless absent or not yet up there is nothing to be mid-invasion of. Publish
        // it, so a session that goes away cannot strand the map dimmed and the warp refused.
        er_invasion_warp_core::warp::set_invasion_attempt_in_flight(false);
        return;
    };
    publish_invasion_attempt_state(session);
    trace_session_state(session);
    // Watch which session fields the Themida VM writes, and when. This is the only way left to
    // learn the invasion state machine: its middle is virtualized, and a live dump proved there is
    // no plaintext to recover (ersc's .themida is 99.68% identical on disk and in memory, entropy
    // unchanged -- the original x86 does not exist at runtime).
    trace_session_field_writes(session);
    // Order matters. The stall detector may cancel, which lands the session at idle; self-recovery
    // then sees that idle and arms the restart; `drive_pending_reinvade` fires it. Running them in
    // this order recovers a stalled attempt within one tick of it settling rather than three.
    watch_for_stall(session);
    arm_self_recovery(session);
    drive_pending_reinvade(session);
}

/// Read the engine's own view of the join, and log it when it changes.
///
/// Phase 1 is measurement ONLY: this decides nothing and cancels nothing. It exists to produce
/// the three traces the detector's threshold has to come from -- a healthy reject, a dead keep,
/// and a real invasion -- because the only numbers we have today describe the ERSC side, whose
/// middle states are virtualised and whose `0x15` is not a stage at all.
///
/// # Safety
/// Game task thread. Every read is fault-closed through `safe_read_*`; a null or stale singleton
/// yields `None` and the sample is skipped rather than faulting.
///
/// `session` carries the Seamless state alongside the value that build calls idle, because the two
/// only mean anything together: an update renumbered the enum once already, so a bare state code
/// that is idle on one build is an active state on another.
/// Discard a cached session whose state cannot move while the engine's join demonstrably does.
///
/// # The check this makes executable
///
/// A real Seamless session walks `0x0e, 0x0f, 0x12, 0x13, ...` through a join. Five separate
/// objects have now been accepted by the scan and then sat at one value for an entire invasion --
/// `0x3dfadb`, `0x860f90f8`, `0x451200`, `0x45e00cb0`, `0xa2760038` -- and the tell was identical
/// every time and free to read: the `ersc=` column of `join-progress` frozen while `lobby` and
/// `proto` advance. Run br-20260908-210559-6f26 is the cleanest instance: nine readings, all
/// `0x01`, while lobby went `0 -> 4 -> 6` and proto cycled through 14.3 seconds of join.
///
/// Every static check tried so far is a snapshot -- a value at an offset, a mutex shape, a
/// `_Count` -- and each one was defeated by memory that happened to hold the right numbers. This
/// is not a snapshot: it asks the candidate to do something only the real object can do. A wrong
/// pointer cannot pass it, because passing requires tracking a state machine it is not part of.
///
/// Recorded as a `bd` memory the same morning and left as prose, which is why it fired five times
/// instead of once. A diagnostic that lives only in prose gets rediscovered; one that lives in the
/// code gets enforced.
fn note_session_liveness(ersc_state: Option<u32>, lobby: u32, protocol: u32) {
    let Some(state) = ersc_state else {
        // Nothing cached to invalidate, and no reading to judge.
        LIVENESS_STALE_SAMPLES.store(0, Ordering::SeqCst);
        return;
    };
    // A session Seamless handed over is never judged by this heuristic. `OSM` is only ever set by
    // `capture_osm`, which requires `+0x58` to lead to an object carrying a live session state --
    // so the pointer came from Seamless calling its own action, not from a scan guessing. The
    // heuristic exists to disown a guess, and applying it to a handed-over pointer disowns the
    // real thing.
    //
    // Measured on run `br-20260910-003946-b9b0`, which adopted the real object at
    // `OSM=0x466ad518 session=0x466ac930`: the session sat at state `0x16` while the engine's
    // lobby/protocol advanced, this heuristic called it "not a session" four times, and the log
    // recorded `0x16 -> 0x23 CANCELLING -- held 5995 ticks / 181264ms` and again `8873 ticks /
    // 215212ms`. `0x16` is a state the real session genuinely holds for the length of an invasion
    // -- Frida saw the same session enter the `0x24` writer with `state_on_entry=0x16` -- so a
    // multi-minute stay there is a player fighting, not a dead pointer.
    if OSM.load(Ordering::SeqCst) != 0 {
        LIVENESS_STALE_SAMPLES.store(0, Ordering::SeqCst);
        return;
    }
    let engine = ((lobby as usize) << 16) | (protocol as usize);
    let previous_engine = LIVENESS_LAST_ENGINE.swap(engine, Ordering::SeqCst);
    let previous_state = LIVENESS_LAST_STATE.swap(state as usize, Ordering::SeqCst);
    // Only samples where the engine moved are evidence. A quiet moment proves nothing about
    // either party, and counting it would eventually discard a healthy session that was simply
    // idle -- which is the normal state between invasions.
    if previous_engine == engine || previous_engine == usize::MAX {
        return;
    }
    if previous_state != state as usize {
        LIVENESS_STALE_SAMPLES.store(0, Ordering::SeqCst);
        return;
    }
    let stale = LIVENESS_STALE_SAMPLES.fetch_add(1, Ordering::SeqCst) + 1;
    if stale < LIVENESS_STALE_LIMIT {
        return;
    }
    LIVENESS_STALE_SAMPLES.store(0, Ordering::SeqCst);
    log_refusal_once(
        &LIVENESS_REFUSAL_SAID,
        format_args!(
            "local-invasion: DISCARDING the cached session -- its state has read {state:#x} \
             across {stale} samples in which the engine's own join advanced (lobby/proto changed \
             every time). A real session moves through 0x0e/0x0f/0x12/0x13 during a join, so this \
             pointer is not one; five earlier candidates failed exactly this way while passing \
             every static check. Re-scanning."
        ),
    );
    #[cfg(windows)]
    session_scan::invalidate_cached_session();
    // The popup skip's fallback stops here, permanently for this session. A discard is the filter
    // saying this pointer was never a session; re-scanning may hand back another look-alike, and
    // the skip is the one consumer that cannot survive a wrong answer -- a false idle swallows the
    // dialog that leaves an invasion, which trapped a player on 2026-09-09
    // (bd auto-dismiss-must-be-scoped-not-every-dialog-2026-09-09). Judging and logging may go on
    // guessing; suppressing a dialog may not.
    SESSION_EVER_DISCARDED.store(1, Ordering::SeqCst);
}

/// Whether any cached session has ever been discarded as "not a session" this run. Latched, never
/// cleared: once the scan has been wrong once, its later answers are not trustworthy enough to
/// suppress a dialog on.
static SESSION_EVER_DISCARDED: AtomicUsize = AtomicUsize::new(0);

/// `session+0x238` -- an f64 in the upper half of the qword, advancing once per frame while the
/// session sits in a state. Measured climbing 0.386s -> 14.1s across one stuck `0x23` on run
/// br-20260910-012230-666e.
const SESSION_COOLDOWN_OFFSET: usize = 0x238;
/// The last whole second reported for that clock, so it is logged once a second rather than once
/// a frame.
static COOLDOWN_LAST_WHOLE_SECOND: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Engine samples with an unchanged session state before the cache is discarded. The measured
/// runs showed nine in a row, so four is decisive without being twitchy.
const LIVENESS_STALE_LIMIT: usize = 4;
static LIVENESS_STALE_SAMPLES: AtomicUsize = AtomicUsize::new(0);
static LIVENESS_LAST_ENGINE: AtomicUsize = AtomicUsize::new(usize::MAX);
static LIVENESS_LAST_STATE: AtomicUsize = AtomicUsize::new(usize::MAX);
static LIVENESS_REFUSAL_SAID: Mutex<Option<String>> = Mutex::new(None);

/// Re-arm the hunt when a join this filter accepted dies before it lands.
///
/// Accepting a match stands the hunt down, which is right while the invasion is coming. It is wrong
/// once the join has failed: the player accepted an invasion, never got it, and was left with no
/// search running and no indication why.
///
/// The clearing rule is what keeps this from fighting the deliberate disarm. `KEPT_JOIN_PENDING`
/// is cleared at `lobbyState::CLIENT`, the same instant `INVASION_ACTUALLY_HAPPENED` latches, so a
/// join that lands is never re-armed here -- the disarm at the end of a real invasion is taken from
/// what the engine did and stays that way.
#[cfg(windows)]
fn resume_the_hunt_if_an_accepted_join_died(
    progress: &er_invasion_warp_core::join_progress::JoinProgress,
) {
    if !progress.is_finished() || !KEPT_JOIN_PENDING.swap(false, Ordering::SeqCst) {
        return;
    }
    let count = KEPT_JOIN_FAILURES.fetch_add(1, Ordering::SeqCst) + 1;
    AUTO_SEARCH_ARMED.store(true, Ordering::SeqCst);
    PENDING_REINVADE.store(true, Ordering::SeqCst);
    crate::standalone_log(format_args!(
        "local-invasion: the match this filter accepted never landed (#{count}) -- {progress}. \
         Accepting stands the hunt down, so without this the player is left with no search running \
         after an invasion they never got to play."
    ));
}

/// Drop a match the engine has already failed, without waiting for the player.
///
/// # The complaint
///
/// Reported live 2026-09-10: "Seamless claiming a live match means I have to use the item to leave
/// before I can invade again". That is the state this clears. Seamless keeps its session at an
/// active value after the engine's join has died, and nothing in the game takes it back down, so
/// the player is holding a match that cannot become an invasion and the only way out is to spend
/// the item on a cancel.
///
/// # Why this can act where the stall watchdog cannot
///
/// `crate::stall_watchdog` returns before it reads anything unless `AUTO_SEARCH_ARMED`, because it
/// is part of the hunt loop -- a search the player started themselves, or one whose loop has stood
/// down, gets no watchdog at all. And its trigger is a dwell: five seconds in a timed state, which
/// is a guess that something is wrong.
///
/// This is not a guess and not a dwell. `Verdict::Failed` means `lobbyState` is `CreateFailed` or
/// `JoinFailed` -- the engine went out to Steam and was told no -- so the attempt is over as a
/// matter of fact, whoever started it. Until the `Failed` verdict existed this reading was
/// `Verdict::Idle`, which also means "the engine never started", and the two cannot be told apart:
/// acting on `Idle` would have cancelled attempts that had not begun yet.
///
/// The grace window exists only so a frame sampled across a transition cannot fire it.
#[cfg(windows)]
fn drop_a_match_the_engine_has_already_failed(
    progress: &er_invasion_warp_core::join_progress::JoinProgress,
    ersc_claims_attempt: bool,
) {
    use er_invasion_warp_core::join_progress::Verdict;
    // `Failed` is the engine saying Steam told it no. `Idle` here is narrower than it sounds: the
    // verdict only reads `Idle` with `lobbyState == None`, no outstanding RPC and both phase timers
    // clear, which is `DisconnectCleanup` having run. Either way the engine has no session, and
    // Seamless is holding a match against one.
    let engine_has_no_session =
        progress.lobby_state == er_invasion_warp_core::join_progress::lobby_state::NONE;
    let grace = match progress.verdict() {
        Verdict::Failed => DEAD_JOIN_GRACE_MS,
        Verdict::Idle => TORN_DOWN_GRACE_MS,
        Verdict::Progressing | Verdict::Committed => {
            DEAD_JOIN_SINCE_MS.store(0, Ordering::SeqCst);
            return;
        }
    };
    if !ersc_claims_attempt {
        DEAD_JOIN_SINCE_MS.store(0, Ordering::SeqCst);
        return;
    }
    let now = now_ms();
    let since =
        match DEAD_JOIN_SINCE_MS.compare_exchange(0, now, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => now,
            Err(first) => first,
        };
    let held = now.saturating_sub(since);
    if held < grace {
        return;
    }
    let Ok(session) = resolve_session() else {
        return;
    };
    let Some(state) = read_session_state(session.abi, session.session) else {
        return;
    };
    if state == session.abi.state_idle {
        DEAD_JOIN_SINCE_MS.store(0, Ordering::SeqCst);
        return;
    }
    // Cleared before the cancel rather than after it, so a cancel that is refused -- a poisoned
    // guard, a lock the wrong shape -- re-arms the window instead of latching this off forever.
    DEAD_JOIN_SINCE_MS.store(0, Ordering::SeqCst);
    let count = DEAD_JOIN_RECOVERIES.fetch_add(1, Ordering::SeqCst) + 1;
    crate::standalone_log(format_args!(
        "local-invasion: dropping a match the engine has no session for (#{count}) -- {progress}, \
         held {held}ms while Seamless still read {state:#04x}. Nothing in the game takes that back \
         down, so without this the player has to spend the invasion item to leave before invading \
         again."
    ));
    cancel_stalled_attempt_inner(session, state, held, engine_has_no_session);
}

#[cfg(windows)]
fn trace_join_progress(session: Result<(u32, u32), &'static str>) {
    let ersc_state = session.ok().map(|(state, _)| state);
    let idle_state = session.map_or(u32::MAX, |(_, idle)| idle);
    let Some(progress) = read_join_progress() else {
        return;
    };
    note_session_liveness(
        ersc_state,
        progress.lobby_state as u32,
        progress.protocol_state as u32,
    );
    // `Client` and nothing else. `call_for_warp` is tempting and WRONG: `WarpNextStageKick_` runs
    // for every warp including a plain fast travel, so latching on it would mark an ordinary grace
    // warp as an invasion.
    if progress.lobby_state == er_invasion_warp_core::join_progress::lobby_state::CLIENT {
        // The join landed, so the session is allowed to read idle again.
        JOIN_IN_FLIGHT.store(false, Ordering::SeqCst);
        // And the accept is settled: from here `INVASION_ACTUALLY_HAPPENED` owns the disarm.
        KEPT_JOIN_PENDING.store(false, Ordering::SeqCst);
    }
    resume_the_hunt_if_an_accepted_join_died(&progress);
    if progress.lobby_state == er_invasion_warp_core::join_progress::lobby_state::CLIENT
        && !INVASION_ACTUALLY_HAPPENED.swap(true, Ordering::SeqCst)
    {
        // The join landed. This is the moment "Invasion successful" is true -- measured at
        // 0.57-3.5s after join data on every real join, and never reached by a match that dies.
        let pending = PENDING_SUCCESS_BLOCK.swap(usize::MAX, Ordering::SeqCst);
        if pending != usize::MAX
            && let Some(config) = current_config()
        {
            banner::announce_success(config.reject_notice, pending as u32);
        }
    }
    // Pack the reading so an unchanged frame costs one atomic compare and no formatting.
    let packed = (u64::from(progress.lobby_state as u32) << 40)
        | (u64::from(progress.protocol_state as u32) << 24)
        | (u64::from(u8::from(progress.join_request_handle != 0)) << 16)
        | (u64::from(u8::from(progress.join_check_remain > 0.0)) << 8)
        | u64::from(u8::from(progress.call_for_warp));
    // Only an attempt ERSC actually claims can be a stalled one. An unresolvable session is not
    // evidence of anything, so it never counts.
    let ersc_claims_attempt = ersc_state.is_some_and(|state| state != idle_state);
    if progress.is_finished() && ersc_claims_attempt {
        JOIN_PROGRESS_IDLE_SAMPLES.fetch_add(1, Ordering::Relaxed);
    }
    drop_a_match_the_engine_has_already_failed(&progress, ersc_claims_attempt);
    if JOIN_PROGRESS_LAST.swap(packed, Ordering::SeqCst) == packed {
        return;
    }
    let since_join = match JOIN_DATA_AT_MS.load(Ordering::SeqCst) {
        0 => String::new(),
        at => format!(" +{}ms since join data", now_ms().saturating_sub(at)),
    };
    crate::standalone_log(format_args!(
        "join-progress: ersc={} {progress}{since_join}",
        match session {
            Ok((state, _)) => format!("{state:#04x}"),
            Err(reason) => reason.to_owned(),
        }
    ));
}

/// One fault-closed sample of the engine-side join fields.
#[cfg(windows)]
fn read_join_progress() -> Option<er_invasion_warp_core::join_progress::JoinProgress> {
    use er_invasion_warp_core::join_progress as jp;
    use er_invasion_warp_core::warp::{
        SESSION_LOBBY_STATE_OFFSET, SESSION_MANAGER_GLOBAL_RVA, SESSION_PROTOCOL_STATE_OFFSET,
    };

    let base = er_game_base::mem::game_module_base().ok()?;
    let manager = unsafe {
        er_game_base::mem::safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            SESSION_MANAGER_GLOBAL_RVA,
            "SESSION_MANAGER_GLOBAL_RVA",
        ))
    }?;
    if manager == 0 {
        return None;
    }
    // Resolved for the running build, like the session-manager read directly above it -- the two
    // sat side by side reading the same kind of global and only one of them asked. GameMan moved
    // 0x3d69918 -> 0x3d6d988 on 1.17, so the raw form returned a neighbouring global and the
    // call-for-warp byte was read out of it.
    let game_man = unsafe {
        er_game_base::mem::safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            jp::GAME_MAN_GLOBAL_RVA,
            "GAME_MAN_GLOBAL_RVA",
        ))
    }?;
    let call_for_warp = if game_man == 0 {
        false
    } else {
        unsafe { er_game_base::mem::safe_read_u8(game_man + jp::GAME_MAN_CALL_FOR_WARP_OFFSET) }
            .is_some_and(|byte| byte != 0)
    };
    Some(jp::JoinProgress {
        lobby_state: unsafe {
            er_game_base::mem::safe_read_i32(manager + SESSION_LOBBY_STATE_OFFSET)
        }?,
        protocol_state: unsafe {
            er_game_base::mem::safe_read_i32(manager + SESSION_PROTOCOL_STATE_OFFSET)
        }?,
        join_request_handle: unsafe {
            er_game_base::mem::safe_read_i32(manager + jp::SESSION_JOIN_REQUEST_HANDLE_OFFSET)
        }?,
        join_check_remain: unsafe {
            er_game_base::mem::safe_read_f32(manager + jp::SESSION_JOIN_CHECK_REMAIN_OFFSET)
        }?,
        wait_init_remain: unsafe {
            er_game_base::mem::safe_read_f32(manager + jp::SESSION_WAIT_INIT_REMAIN_OFFSET)
        }?,
        call_for_warp,
    })
}

/// How many frames the engine looked idle while ERSC still claimed an attempt.
#[must_use]
pub fn join_progress_idle_samples() -> usize {
    JOIN_PROGRESS_IDLE_SAMPLES.load(Ordering::Relaxed)
}

/// The window of the session object that is watched for VM writes.
///
/// Chosen to span every field the static read identified plus the unexplained space between them:
/// state `+0x110`, the lobby id `+0x178` and owner `+0x180`, the per-offer block `+0x190..0x227`,
/// the `+0x1D4` / `+0x1F0` latches Seek writes, and the `+0x229` flag the lobby key mixes in.
/// Deliberately not `cfg(windows)`: it is plain arithmetic, and the tests that prove the window
/// still covers every known field have to run on the host build like every other test here.
const SESSION_WATCH_BEGIN: usize = 0x100;
const SESSION_WATCH_WORDS: usize = 0x30; // 0x30 * 8 = 0x180 bytes -> 0x100..0x280

/// Previous snapshot, and which session it came from.
#[cfg(windows)]
static SESSION_SNAPSHOT: Mutex<Option<(usize, [u64; SESSION_WATCH_WORDS])>> = Mutex::new(None);

/// How many field-change lines have been written, so a churning field cannot flood the log.
#[cfg(windows)]
static SESSION_FIELD_LINES: AtomicUsize = AtomicUsize::new(0);
/// The cap. Generous enough to cover a whole invasion sequence, small enough to stay readable.
#[cfg(windows)]
const SESSION_FIELD_LINE_BUDGET: usize = 400;

/// Report which session fields changed since the last frame, with the state they changed under.
///
/// # Why this exists
///
/// States `0x0E`, `0x11`, `0x12`, `0x13` and `0x14` are written by no instruction in ersc's
/// readable code -- a byte-anchored scan for `C7 /0 disp32=0x110 imm32` finds only
/// `{0,1,3,6,9,0xD,0x22,0x23}`, and the sole register-sourced write produces `0x0C`/`0x15`. The
/// rest come out of the Themida VM. Reading that code is not available: a live dump of the module
/// showed `.themida` is 99.68% byte-identical to disk with unchanged entropy, so the original
/// instructions never exist in memory to be recovered.
///
/// What is available is the effect. Every field the VM writes is written into an object this
/// module already holds a pointer to, so diffing that object per frame maps the state machine
/// empirically -- which fields move together, which precede a transition, which carry a
/// destination -- without reading a single VM instruction.
///
/// Pure observation: it reads and logs, and writes nothing back.
///
/// # Safety
/// Game task thread; every read is fault-closed and the window is a fixed span of an object the
/// caller already validated.
#[cfg(windows)]
fn trace_session_field_writes(seamless: SeamlessSession) {
    let session = seamless.session;
    if SESSION_FIELD_LINES.load(Ordering::SeqCst) >= SESSION_FIELD_LINE_BUDGET {
        return;
    }
    let mut current = [0_u64; SESSION_WATCH_WORDS];
    for (index, slot) in current.iter_mut().enumerate() {
        let at = session + SESSION_WATCH_BEGIN + index * 8;
        // A fault-closed read that fails leaves the slot zero. That could masquerade as a change,
        // so a failed read abandons the whole snapshot rather than inventing a transition.
        let Some(value) = (unsafe { er_game_base::mem::safe_read_usize(at) }) else {
            return;
        };
        *slot = value as u64;
    }

    let mut guard = match SESSION_SNAPSHOT.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let previous = match guard.as_ref() {
        // A different session object is a different machine; its first frame is a baseline, not a
        // set of changes.
        Some((owner, _)) if *owner != session => None,
        Some((_, snapshot)) => Some(*snapshot),
        None => None,
    };
    *guard = Some((session, current));
    drop(guard);

    let Some(previous) = previous else {
        return;
    };
    let changed: Vec<(usize, u64, u64)> = (0..SESSION_WATCH_WORDS)
        .filter(|index| previous[*index] != current[*index])
        .map(|index| {
            (
                SESSION_WATCH_BEGIN + index * 8,
                previous[index],
                current[index],
            )
        })
        .collect();
    if changed.is_empty() {
        return;
    }
    let state =
        unsafe { er_game_base::mem::safe_read_i32(session + seamless.abi.session_state_offset) }
            .unwrap_or(-1);
    // `+0x238` alone is not a field change worth a line: it is a clock, and it ticks once a frame.
    //
    // Measured on run br-20260910-012230-666e, where the second cancel stuck at 0x23 and this
    // logger wrote one line per tick for the whole wait. The upper half of the qword decodes as an
    // f64 running 0.386 -> 0.443 -> ... -> 14.1 seconds and still climbing, one step per tick, so
    // the "change" is a cooldown advancing normally rather than anything a reader needs told 500
    // times. Reporting it as a cooldown once a second says strictly more in 1/40th the lines.
    if changed.len() == 1 && changed[0].0 == SESSION_COOLDOWN_OFFSET {
        let seconds = f64::from_bits(changed[0].2 & 0xffff_ffff_0000_0000);
        let whole = seconds as usize;
        if COOLDOWN_LAST_WHOLE_SECOND.swap(whole, Ordering::SeqCst) != whole {
            // The meaning is per-state and must not be asserted across all of them. This line
            // first fired at state 0x16 saying "this is the wait the player sees after a cancel",
            // which is false there -- 0x16 is being in an invasion, and its clock is how long the
            // player has been fighting.
            let meaning = if state == seamless.abi.state_cancelling as i32 {
                "an armed re-invade waits for idle, so this clock is the wait after a cancel"
            } else if state == seamless.abi.state_searching as i32 {
                "this clock is how long the search has been running"
            } else {
                "what this clock measures in this state has not been established"
            };
            crate::standalone_log(format_args!(
                "local-invasion: the session has been in state {state:#04x} for {whole}s -- \
                 `session+0x{SESSION_COOLDOWN_OFFSET:x}` is a clock, not a field change, and it \
                 advances once a frame. {meaning}."
            ));
        }
        return;
    }
    let line = SESSION_FIELD_LINES.fetch_add(1, Ordering::SeqCst) + 1;
    crate::standalone_log(format_args!(
        "local-invasion: session fields changed at state {state:#04x} -- {changed:x?} \
         (offset, before, after). These are writes this DLL did not make; the ones at offsets with \
         no readable writer came from the Themida VM. Line {line}/{SESSION_FIELD_LINE_BUDGET}."
    ));
    if line == SESSION_FIELD_LINE_BUDGET {
        crate::standalone_log(format_args!(
            "local-invasion: session field tracing has hit its {SESSION_FIELD_LINE_BUDGET}-line \
             budget and will stay quiet from here. Raise SESSION_FIELD_LINE_BUDGET if a longer \
             sequence is needed; the cap exists so one churning field cannot bury the run."
        ));
    }
}

#[cfg(not(windows))]
fn trace_session_field_writes(_session: SeamlessSession) {}

/// `(keeps, cancels, automatic re-searches, unenforced rejections)` so a run can be judged without
/// reading the log.
///
/// The fourth number is the one that says whether the filter worked, as opposed to whether it ran.
/// See [`UNENFORCED_REJECTS`]: any value above zero means a match this module rejected went ahead
/// regardless, which from the player's seat is indistinguishable from the mod being switched off.
#[must_use]
pub fn tallies() -> (usize, usize, usize, usize) {
    (
        KEEPS.load(Ordering::SeqCst),
        CANCELS.load(Ordering::SeqCst),
        REINVADES.load(Ordering::SeqCst),
        UNENFORCED_REJECTS.load(Ordering::SeqCst),
    )
}

#[cfg(test)]
mod tests;
