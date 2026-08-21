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
//! # What this deliberately does NOT do
//!
//! It does not fake an invasion, spoof session state, or enter `CSNetMan` / `QuickmatchManager` /
//! `CSBreakInPointManager`. It reads a destination the server already sent, and -- when the user
//! has asked for filtering -- invokes the same cancel the user could press by hand. Everything it
//! calls is a path the game runs anyway.
//!
//! # Why nothing in `ersc.dll` is hooked
//!
//! Asked directly whether the filter could avoid repeatedly cancelling, the binary answered no --
//! and answered something better instead. Static read of the shipped `ersc.dll` (v1.9.9),
//! 2026-08-05:
//!
//! * `ersc+0x243e0` ("Invade world") is nine instructions: take the mutex at `S+0xC0`, bail if
//!   `S+0x10C == 0x7fffffff`, write `S+0x110 = 0xd`, release. `ersc+0x24460` ("Cancel search") is
//!   the same shape writing `0x22`. Neither queries anything.
//! * Across all 4839 functions in the unpacked `.text`, `0xd` reaches `S+0x110` at exactly ONE
//!   site -- the one above. There is no client-side candidate list to filter, because starting a
//!   search *is* that single store; everything after it happens inside the Themida-virtualised
//!   dispatcher and on the remote side. This is why `SetMultiplayJoinData` is not a late
//!   interception point but the FIRST instant the destination exists on this machine, and why
//!   accept-then-reject is the only available shape.
//! * Both actions read **`rcx` only**. `rdx`, `r8` and `r9` are never touched. So the earlier plan
//!   -- hook the actions to capture a real press and replay its arguments -- was solving a problem
//!   that does not exist: `(OSM, 0, 1, 1)` is provably equivalent to what the engine passes.
//!
//! With the arguments unnecessary, the only thing still needed from Seamless is the OSM pointer.
//! Reading it out of a static would have meant hooking nothing in Seamless at all; that was
//! attempted and does not work (see [`ersc::NEXT_OBJECT_OFFSET`] for the candidate that looked
//! right and was not). So OSM is learned by observing it being passed to the menu builder.
//!
//! What that leaves is **two** detours: `CS::SosSignMan::SetMultiplayJoinData`, a GAME function,
//! where matches are judged; and `ersc!show`, the Seamless menu builder, which is observed
//! read-only -- it copies `rcx` and immediately runs the original with every argument untouched,
//! changing nothing and suppressing nothing. The two option ACTIONS are NOT hooked, and a rejection
//! invokes the same callback the user's own click invokes, with arguments the callee provably
//! ignores. `nothing_in_this_module_detours_ersc`'s successor test pins that budget so growing it
//! is a decision rather than a drift.
//!
//! `ersc.dll` is RELOCATABLE and has no fixed load address, so every ERSC address is
//! `module base + RVA` resolved at runtime and byte-checked before use. If Seamless is not
//! loaded the filter never arms: without a Seamless session there are no Seamless invasions
//! to filter.
//!
//! # Fail-closed direction
//!
//! Every uncertainty resolves toward NOT cancelling. Config missing or unparseable, OSM not
//! resolvable, ERSC absent, anchor unresolved -- all leave matches alone. The failure this guards
//! against is silently cancelling other players' invasions, which is worse than a filter that
//! quietly does nothing.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use er_game_base::fnv1a::{fnv1a64, fnv1a64_mix};
use er_invasion_warp::local_invasion::{
    InvasionAnchor, InvasionCandidate, LocalInvasionConfig, LocalInvasionMode, LocationChoice,
    RejectReason, Verdict,
};
use er_invasion_warp::local_invasion_config::{CONFIG_FILE_NAME, DEFAULT_CONFIG_TOML, HotConfig};
use er_invasion_warp::param_row::PinAppearance;

/// ERSC RVAs and their opening bytes, read out of the shipped `ersc.dll` (Seamless Co-op v1.9.9,
/// image base `0x180000000`) on 2026-08-05 -- not copied from a decompile listing. Every one is
/// byte-checked against the loaded module before it is hooked or called, so a Seamless update that
/// moves them disarms the filter instead of jumping into the middle of an instruction.
mod ersc {
    /// `show(void* OSM, int groupId)` -- the option-menu builder. The one entry point without an
    /// `endbr64` prologue, which makes it a cheap "is this the ersc.dll we measured" discriminator.
    /// Read, never hooked.
    pub const SHOW_RVA: usize = 0x2_2d30;
    pub const SHOW_PROLOGUE: &[u8] = &[
        0x55, 0x41, 0x57, 0x41, 0x56, 0x41, 0x55, 0x41, 0x54, 0x56, 0x57, 0x53,
    ];
    /// The "Invade world" option action -- `S+0x110 = 0xd`. Reads `rcx` only.
    pub const INVADE_ACTION_RVA: usize = 0x2_43e0;
    pub const INVADE_PROLOGUE: &[u8] = &[
        0xf3, 0x0f, 0x1e, 0xfa, 0x56, 0x57, 0x48, 0x83, 0xec, 0x28, 0x48, 0x8b, 0x79, 0x58,
    ];
    /// The "Cancel search" option action -- `S+0x110 = 0x22`. Reads `rcx` only.
    pub const CANCEL_ACTION_RVA: usize = 0x2_4460;
    pub const CANCEL_PROLOGUE: &[u8] = &[
        0xf3, 0x0f, 0x1e, 0xfa, 0x56, 0x57, 0x48, 0x83, 0xec, 0x28, 0x48, 0x8b, 0x79, 0x58,
    ];
    /// `BuildLobbyKey(ctx, std::string* out)` -- produces the `lobby_key` string.
    ///
    /// # Why this one matters more than it looks
    ///
    /// Seamless finds worlds with a Steam lobby-list query carrying exactly TWO match terms:
    /// `lobby_type == "yknx3_seamless_master_lobby"` and `lobby_key == <this string>`, the latter
    /// with `k_ELobbyComparisonEqual`. An exhaustive decode of the XOR-obfuscated string idiom
    /// across every plaintext function in `ersc.dll` finds only those two keys in the whole module
    /// -- no map, block, region, coordinate or radius anywhere. So two players who derive DIFFERENT
    /// `lobby_key` values are simply invisible to each other, no matter how well their levels,
    /// weapon levels or locations line up.
    ///
    /// The value is **64 lowercase hex characters** -- a SHA-256 digest, finalized at
    /// `ersc+0xac053` from the IV at `ersc+0x1bab00`. The 16-character `%016llX` string is only the
    /// PLAINTEXT that gets AES-encrypted and then hashed together with a bit from `ctx+0x229` and a
    /// 32-byte salt at `ersc+0x1dc58f`; printing 16 characters would be printing a heap pointer's
    /// worth of the wrong thing and would fabricate exactly the "it changed / it didn't" answer
    /// this probe exists to measure.
    ///
    /// The moment it re-derives is jittered (`time() + [ctx+0x918] + rand() % [ctx+0x920]`, at
    /// `ersc+0xaaf8d`), so a comparison between two machines must use the key that was live when
    /// the offers arrived, not merely the first one printed.
    pub const LOBBY_KEY_HEX_LEN: usize = 64;
    ///
    /// Called from `ersc+0xaafff` (the filter path, feeding
    /// `AddRequestLobbyListStringFilter(this, "lobby_key", value, 0)` at `ersc+0xab07e`) and from
    /// `ersc+0xab0ae` (the publish path, feeding `SetLobbyData`). Both hand it the same stack
    /// `std::string`. Observed, never altered: publishing a key of our own would change what every
    /// other Seamless client matches, which is not ours to do.
    pub const BUILD_LOBBY_KEY_RVA: usize = 0xa_bc20;
    pub const BUILD_LOBBY_KEY_PROLOGUE: &[u8] = &[
        0x55, 0x41, 0x57, 0x41, 0x56, 0x41, 0x55, 0x41, 0x54, 0x56, 0x57, 0x53, 0x48, 0x81, 0xec,
        0x48, 0x01, 0x00, 0x00,
    ];
    /// MSVC `std::string`: `{ union { char buf[16]; char* ptr; }; size_t size; size_t capacity; }`.
    /// A capacity of 16 or more means the bytes are on the heap and the first field is a pointer --
    /// which is always the case here, because the value is exactly 16 characters and the inline
    /// buffer holds at most 15 plus a terminator.
    pub const STD_STRING_SIZE_OFFSET: usize = 0x10;
    pub const STD_STRING_CAPACITY_OFFSET: usize = 0x18;
    pub const STD_STRING_HEAP_CAPACITY: usize = 0x10;

    /// `OSM+0x58` is the session object.
    ///
    /// A `.data` singleton holding OSM would have let this module hook NOTHING in Seamless. One
    /// was looked for and not found: the only `.data` global that is loaded and then dereferenced
    /// at `+0x58` is `ersc+0x21b228`, and it is read at 121 sites, written by a pair of adjacent
    /// CRT-shaped setters, and non-zero in the file -- a locale/allocator global that the search
    /// matched by coincidence, not the session. The `seamless` tag likewise lives inside longer
    /// strings (`seamless buddy system`, source paths), so there is no constructor to trace back
    /// to a singleton either. Recorded here so the next reader does not repeat the hunt.
    pub const NEXT_OBJECT_OFFSET: usize = 0x58;
    /// OSM carries the ASCII tag `seamless` here. Measured live 2026-08-04, and the only thing
    /// that distinguishes a real OSM from any other pointer-shaped value.
    pub const OSM_TAG_OFFSET: usize = 0x68;
    pub const OSM_TAG: &[u8] = b"seamless";
    /// Session state, the field the two option actions write.
    pub const SESSION_STATE_OFFSET: usize = 0x110;
    /// Guard field. `0x7fffffff` is the value both actions refuse to proceed past -- they take a
    /// fatal-error branch instead -- so the filter refuses too.
    pub const SESSION_GUARD_OFFSET: usize = 0x10c;
    pub const SESSION_GUARD_POISON: u32 = 0x7fff_ffff;
    /// Idle: the state a cancelled search settles back to, and the state `invade` requires.
    pub const SESSION_STATE_IDLE: u32 = 0x00;
    /// The state the "Invade world" action writes -- the one and only site in the whole unpacked
    /// `.text` that puts this value in the field.
    pub const SESSION_STATE_SEARCHING: u32 = 0x0d;
    /// The state "Cancel search" writes.
    ///
    /// NOT usable as a "the user cancelled" signal, which is what it was briefly used for: the
    /// static scan found SEVEN sites writing `0x22` here and only one is the Cancel action, so
    /// every internal abort looked like a user cancel. It survives as a label for the trace.
    pub const SESSION_STATE_CANCELLING: u32 = 0x22;
    /// The highest plausible session state, used to reject a pointer that is not a session at all.
    pub const SESSION_STATE_MAX: u32 = 0xff;
}

/// The four-argument shape of an ERSC option action. Only the first is read by the callee, which
/// the disassembly in the module docs establishes; the rest are passed as the constants the engine
/// itself passes so a stack trace through one of these looks exactly like a user's own click.
type ErscActionFn = unsafe extern "system" fn(usize, usize, usize, usize) -> usize;

/// The last session state this module saw, so a transition to "cancelling" that we did not cause
/// can be recognised as the USER's own Cancel search -- polled, rather than hooked.
static LAST_SESSION_STATE: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Set while the filter is itself driving ERSC, so our own cancel is not mistaken for the user's.
static IN_OUR_CALL: AtomicBool = AtomicBool::new(false);
/// Armed by our own cancel: search again as soon as the session settles back to idle. Cleared the
/// moment the re-invade fires, so a session that never returns to idle cannot make this repeat.
static PENDING_REINVADE: AtomicBool = AtomicBool::new(false);
/// Attempts that ended without us cancelling them, and were restarted anyway. Counted separately
/// from `REINVADES` so "Seamless dropped it" is distinguishable from "we rejected it" in a log.
static SELF_RECOVERIES: AtomicUsize = AtomicUsize::new(0);
/// Attempts cancelled because a handshake step stopped progressing.
static STALL_RECOVERIES: AtomicUsize = AtomicUsize::new(0);
/// Stall detection state. Behind a mutex rather than atomics because the decision reads and writes
/// "which state, and since when" together; a torn read there would restart the clock at random.
static STALL_WATCHDOG: Mutex<crate::stall_watchdog::StallWatchdog> =
    Mutex::new(crate::stall_watchdog::StallWatchdog::new());
/// Slows the restart when Seamless is refusing attempts instantly -- the opposite failure to the
/// one the stall watchdog catches, and invisible to it because every state is held too SHORT.
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
/// whether that interval is a FRAME COUNT or a WALL CLOCK decides whether raising the frame rate
/// would shorten the wait — the difference between a usable idea and a void one. The two are
/// indistinguishable at a steady frame rate, which is exactly the condition the earlier reading was
/// taken under: ~600 ticks, nine times running, at unchanging fps. That is equally consistent with
/// both, so it settles nothing.
///
/// Recording both per transition makes any natural frame-rate variation within one run decide it —
/// a constant tick count across dwells at differing fps means a frame counter, a constant
/// millisecond count means a clock. The implied fps is printed alongside so a comparison between
/// two dwells that ran at the SAME rate is visibly inconclusive rather than silently over-read.
static TICKS: AtomicU64 = AtomicU64::new(0);
static LAST_TRANSITION_TICK: AtomicU64 = AtomicU64::new(0);
static LAST_TRANSITION_MS: AtomicU64 = AtomicU64::new(0);
/// Decides which rejections are worth announcing. Behind a mutex because the decision reads and
/// updates "what did we last say" together.
static REJECT_NOTICE: Mutex<er_invasion_warp::reject_notice::RejectNotice> =
    Mutex::new(er_invasion_warp::reject_notice::RejectNotice::new());
/// Set once the banner has failed, so a missing notice is reported one time instead of every 20
/// seconds for the rest of the session.
static NOTICE_FAILED: AtomicBool = AtomicBool::new(false);
/// Cleared by a cancel the user performed. Their cancel means "stop looking", and it has to beat
/// our re-arm or the filter would fight them.
static AUTO_SEARCH_ARMED: AtomicBool = AtomicBool::new(false);

static CANCELS: AtomicUsize = AtomicUsize::new(0);
static KEEPS: AtomicUsize = AtomicUsize::new(0);
static REINVADES: AtomicUsize = AtomicUsize::new(0);

static CONFIG: Mutex<Option<HotConfig>> = Mutex::new(None);

/// Trampoline to the original `SetMultiplayJoinData` -- the module's ONLY detour, and it is on the
/// game, not on Seamless.
static ORIG_SET_JOIN_DATA: AtomicUsize = AtomicUsize::new(0);

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
                 reject_notice={} named={} ids={} blocks={} excluded={} mark={} unmark={}",
                outcome.config.enabled,
                outcome.config.mode.as_str(),
                outcome.config.hunt,
                // EVERY OPTION THAT CHANGES BEHAVIOUR MUST APPEAR HERE. These three were missing,
                // and the gap cost a live A/B on 2026-08-06: the file was edited mid-session to turn
                // `dll_users_only` on, this line duly reprinted -- proving the reload had happened --
                // but said nothing about the option that had just changed. Whether the new value had
                // parsed was unknowable until a lobby-pool line happened to appear a minute later.
                // "The config reloaded" is not the question anyone has; "what is in force now" is.
                outcome.config.dll_users_only,
                outcome.config.reject_notice,
                outcome.config.named_locations.len(),
                outcome.config.named_location_text_ids.len(),
                outcome.config.allowed_blocks.len(),
                // Exclusions beat everything else, so a forgotten one is the hardest rejection to
                // explain from the outside -- it looks identical to being in the wrong place.
                outcome.config.blocked_blocks.len(),
                // Which keys are actually live. Without this a mistyped name that happened to parse
                // into a DIFFERENT valid key looks exactly like the feature not working.
                er_invasion_warp::keybind::key_name(outcome.config.mark_key),
                er_invasion_warp::keybind::key_name(outcome.config.unmark_key),
            ));
        }
        // SAY THAT THE TYPED NAMES DO NOTHING YET. `named_locations` is parsed and stored but never
        // resolved to text ids, so it contributes nothing to a verdict -- and in `mode = "named"`
        // with no ids collected that means EVERY match is rejected, forever, for a user who did
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

/// The config currently in force, re-reading the file first.
fn current_config() -> Option<LocalInvasionConfig> {
    refresh_config();
    let guard = CONFIG.lock().ok()?;
    guard.as_ref().map(|hot| hot.current().clone())
}

// ---------------------------------------------------------------------------------------------
// Resolving Seamless's session, without hooking it
// ---------------------------------------------------------------------------------------------

/// The option-menu object and its session, resolved by reading.
#[derive(Clone, Copy)]
struct SeamlessSession {
    osm: usize,
    session: usize,
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
    // `a` IS the option-menu object: that is `show`'s first parameter, and the prologue at this
    // address was byte-checked before the hook went in. Storing it is therefore not a guess, and
    // it is deliberately NOT gated on a content check.
    //
    // It used to be gated on the `seamless` tag at `+0x68`, and that silently broke the whole
    // feature on 2026-08-05: a real match was judged and rejected, then `cannot cancel -- session
    // is not resolvable`, because the tag never matched and OSM was consequently never stored. The
    // tag had been measured ONCE, live, in one frida session; promoting a single observation to a
    // precondition is what turned it into a gate on the product path. It is now reported as a
    // diagnostic and believed by nothing.
    // Opening Seamless's menu is the user reaching for the controls, so the auto-search loop stands
    // down here -- before they have even chosen an option.
    //
    // This replaces inferring "the user cancelled" from the session reaching `0x22`, which was
    // wrong on its face: the static scan of ersc.dll found SEVEN sites writing `0x22` to `S+0x110`
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
    let mut first_capture = false;
    if a != 0 {
        first_capture = OSM.swap(a, Ordering::SeqCst) == 0;
        if first_capture {
            let session =
                unsafe { er_game_base::mem::safe_read_usize(a + ersc::NEXT_OBJECT_OFFSET) };
            crate::standalone_log(format_args!(
                "local-invasion: captured Seamless's option-menu object OSM=0x{a:x} (group={b}) \
                 session={:?} state={:?} tag_at+0x68={}",
                session.map(|s| format!("0x{s:x}")),
                session.and_then(read_session_state),
                if osm_tag_matches(a) {
                    "\"seamless\""
                } else {
                    "not the measured bytes (harmless -- nothing depends on it)"
                }
            ));
        }
    }
    let orig = ORIG_SHOW.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let result = unsafe { core::mem::transmute::<usize, ErscActionFn>(orig)(a, b, c, d) };
    // AFTER the original, not before it, and this is not a style preference -- it is the whole
    // difference between reading the rows and reading nothing. `show` is what CLEARS and APPENDS
    // the option vector, so on entry `+0x108`/`+0x110` still describe the previous (empty) menu.
    // The first live run reported `visible options: <unreadable>` from exactly that mistake, and
    // a manual /proc read moments later showed one populated 0x90-byte row sitting there. Same
    // `first` latch, so this still reports once per process.
    if first_capture {
        report_menu_seams(a);
    }
    result
}

/// Report WHICH MODULE owns the option-menu function pointers Seamless calls.
///
/// Read-only, once per process. ERSC resolves these by pattern scan at init and stores no absolute
/// game address anywhere in its image, so the owner was not decidable statically -- and the owner
/// decides where an added menu row would have to attach.
///
/// ANSWERED LIVE, 2026-08-17 (run `br-20260817-184836-d6a7`, user opened the lynchpin menu):
///
/// ```text
/// +0xa8 open_dialog   0x140e9e4f0   eldenring.exe+0xe9e4f0
/// +0xb0 clear_options 0x140800950   eldenring.exe+0x800950
/// +0xb8 append_option 0x140800840   eldenring.exe+0x800840
/// +0xe0 <not a menu fn> 0x13fff0f80 anonymous rwx region based 0x13fff0000
/// ```
///
/// The first three are GAME functions, so 1.16.2's zero shift makes those RVAs directly nameable
/// in the dump and an added row is a static-RE job rather than another runtime hunt. `+0xe0` was
/// guessed to be the teardown and is not: it points below the game image entirely, into a separate
/// anonymous region, so treat that offset as unmapped rather than as a fourth seam.
///
/// Module attribution is by base-address arithmetic on purpose. Under Wine every PE maps as
/// ANONYMOUS memory, so `/proc/<pid>/maps` carries no file name to match against and a
/// name-based lookup would report "unknown" for pointers that are plainly inside the game.
///
/// Nothing is written and nothing is called: this only reads pointers already sitting in an object
/// we hold.
#[cfg(windows)]
fn report_menu_seams(osm: usize) {
    /// `+0xa8` open dialog, `+0xb0` clear list, `+0xb8` append row; `+0xe0` probed and found NOT
    /// to be a menu function (see above) -- kept only so the report keeps saying so.
    const SEAMS: [(usize, &str); 4] = [
        (0xa8, "open_dialog"),
        (0xb0, "clear_options"),
        (0xb8, "append_option"),
        (0xe0, "teardown"),
    ];
    // Attributed against the only two modules that could own them, both of which this module
    // already resolves. A plausible in-image offset identifies the owner; an implausible one says
    // the pointer belongs to neither, which is itself the answer.
    const PLAUSIBLE_IMAGE_SIZE: usize = 0x0800_0000;
    let ersc = ersc_module_base();
    let game = er_game_base::mem::game_module_base().ok();
    let mut parts = Vec::new();
    for (offset, name) in SEAMS {
        let Some(pointer) = (unsafe { er_game_base::mem::safe_read_usize(osm + offset) }) else {
            parts.push(format!("{name}@+{offset:#x}=<unreadable>"));
            continue;
        };
        let owner = [("ersc.dll", ersc), ("eldenring.exe", game)]
            .into_iter()
            .filter_map(|(module, base)| base.map(|base| (module, base)))
            .find(|(_, base)| pointer >= *base && pointer - base < PLAUSIBLE_IMAGE_SIZE)
            .map_or_else(
                || "<neither module>".to_owned(),
                |(module, base)| format!("{module}+{:#x}", pointer - base),
            );
        parts.push(format!("{name}@+{offset:#x}=0x{pointer:x} ({owner})"));
    }
    // The visible-option vector, to confirm which group this menu is and how many rows it holds.
    let counts = (
        unsafe { er_game_base::mem::safe_read_usize(osm + 0x108) },
        unsafe { er_game_base::mem::safe_read_usize(osm + 0x110) },
    );
    let visible = match counts {
        (Some(begin), Some(end)) if end >= begin && begin != 0 => {
            format!("{} row(s)", (end - begin) / 0x90)
        }
        _ => "<unreadable>".to_owned(),
    };
    crate::standalone_log(format_args!(
        "local-invasion: menu seams -- {} | visible options: {visible}",
        parts.join(" ")
    ));
}

/// Why a session could not be resolved. Carried so a failed cancel names its cause instead of
/// being one generic line that fits three different bugs.
#[derive(Clone, Copy, Debug)]
enum NoSession {
    /// Seamless is not loaded.
    ErscAbsent,
    /// Loaded, but not the build these offsets were measured against.
    ErscUnrecognised,
    /// The Seamless menu has not been opened yet this session, so the object was never passed to
    /// anything we can see.
    MenuNeverOpened,
    /// OSM is held but `+0x58` does not lead to a session-shaped object -- a stale pointer.
    SessionUnreadable,
}

/// Resolve the option-menu object and its session, validating structurally.
///
/// Validation is on the SHAPE this module actually depends on -- `OSM+0x58` reads as a pointer, and
/// `S+0x110` holds a small state -- rather than on a remembered byte pattern. Those two are exactly
/// what a cancel needs to be safe, and unlike the tag they are load-bearing in the code below.
///
/// Nothing is cached beyond OSM, and even that is re-validated on every use: the session is a heap
/// allocation whose lifetime this module does not own, and a stale pointer is exactly the kind of
/// thing that turns a filter into a crash.
fn resolve_session() -> Result<SeamlessSession, NoSession> {
    let base = ersc_module_base().ok_or(NoSession::ErscAbsent)?;
    // Prove the module is the build these offsets were measured against before trusting any of it.
    //
    // The fingerprint reads `invade`, NOT `show`, and the difference is the whole reason this
    // function has a comment. `show` was the fingerprint until 2026-08-05, when a live run rejected
    // a match and then reported `ErscUnrecognised` -- because this module HOOKS `show`, and MinHook
    // had overwritten the very bytes being compared. The check was measuring its own detour and
    // concluding Seamless was a stranger. A fingerprint must be taken from something nobody
    // patches; `invade` is called but never hooked, so its prologue stays the shipped bytes for the
    // life of the process.
    if !prologue_matches(base + ersc::INVADE_ACTION_RVA, ersc::INVADE_PROLOGUE) {
        return Err(NoSession::ErscUnrecognised);
    }
    let osm = OSM.load(Ordering::SeqCst);
    if osm == 0 {
        return Err(NoSession::MenuNeverOpened);
    }
    let session = unsafe { er_game_base::mem::safe_read_usize(osm + ersc::NEXT_OBJECT_OFFSET) }
        .filter(|session| *session != 0)
        .filter(|session| read_session_state(*session).is_some())
        .ok_or(NoSession::SessionUnreadable)?;
    if OSM_REPORTED.swap(1, Ordering::SeqCst) == 0 {
        crate::standalone_log(format_args!(
            "local-invasion: Seamless session resolved -- OSM=0x{osm:x} session=0x{session:x}"
        ));
    }
    Ok(SeamlessSession { osm, session })
}

/// Trampoline for the lobby-key observer.
static ORIG_BUILD_LOBBY_KEY: AtomicUsize = AtomicUsize::new(0);
/// Whether the lobby-key observer is installed.
static LOBBY_KEY_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// FNV-1a of the last key reported, so a re-key is one line and a steady key is silent.
static LAST_LOBBY_KEY_HASH: AtomicUsize = AtomicUsize::new(0);
/// How many times the key has been derived, and how many DISTINCT values were seen.
static LOBBY_KEY_DERIVATIONS: AtomicUsize = AtomicUsize::new(0);
static LOBBY_KEY_CHANGES: AtomicUsize = AtomicUsize::new(0);

/// Read an MSVC `std::string` as ASCII, or `None` if it is not the shape we expect.
///
/// Every read is fault-closed. The value is 16 hex characters, so the heap branch is the only one
/// that can carry it -- but the inline branch is handled anyway rather than assumed away, because
/// an assumption here would silently print nothing on a build whose string differs.
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
/// Runs the original first, then reads the string it produced. Reading BEFORE the call would see
/// an uninitialised buffer; reading after is the only ordering that can work, and it also means a
/// fault in our read cannot affect what Seamless publishes.
#[cfg(windows)]
unsafe extern "system" fn build_lobby_key_observer(
    ctx: usize,
    out: usize,
    c: usize,
    d: usize,
) -> usize {
    let orig = ORIG_BUILD_LOBBY_KEY.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    // SAFETY: the trampoline MinHook produced for a byte-verified prologue; same four-argument
    // shape the union dispatcher uses everywhere else in this module.
    let result = unsafe { core::mem::transmute::<usize, ErscActionFn>(orig)(ctx, out, c, d) };

    LOBBY_KEY_DERIVATIONS.fetch_add(1, Ordering::SeqCst);
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
             (size={size:?} capacity={capacity:?}) -- the std::string at ersc+0x{:x} is not what \
             this build expects, so the comparison is UNAVAILABLE rather than wrong. Do not treat \
             a missing line as 'the key did not change'.",
            ersc::LOBBY_KEY_HEX_LEN,
            ersc::BUILD_LOBBY_KEY_RVA
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
    let address = base + ersc::BUILD_LOBBY_KEY_RVA;
    if !prologue_matches(address, ersc::BUILD_LOBBY_KEY_PROLOGUE) {
        if LOBBY_KEY_HOOK_INSTALLED.swap(1, Ordering::SeqCst) == 0 {
            crate::standalone_log(format_args!(
                "local-invasion: ersc.dll @0x{base:x} does not carry the lobby-key builder this \
                 build measured -- NOT touching it. The lobby-key comparison is unavailable; \
                 everything else is unaffected."
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
/// injected AFTER this DLL, so at attach time the module does not exist; and MinHook must not run
/// under the loader lock.
#[cfg(windows)]
fn install_show_observer() -> usize {
    if SHOW_HOOK_INSTALLED.load(Ordering::SeqCst) != 0 {
        return 0;
    }
    let Some(base) = ersc_module_base() else {
        return 0; // Seamless not loaded (yet) -- retry next tick
    };
    let address = base + ersc::SHOW_RVA;
    // Prove the module is the build this RVA describes before writing a single byte into it. This
    // one CAN read `show`, because it runs exactly once and only before the hook exists -- unlike
    // the recurring check in `resolve_session`, which had to stop reading `show` for that reason.
    if !prologue_matches(address, ersc::SHOW_PROLOGUE) {
        if SHOW_HOOK_INSTALLED.swap(1, Ordering::SeqCst) == 0 {
            crate::standalone_log(format_args!(
                "local-invasion: ersc.dll @0x{base:x} does not match the RVAs this build was \
                 measured against -- NOT touching it. The filter stays inert."
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
fn osm_tag_matches(osm: usize) -> bool {
    ersc::OSM_TAG.iter().enumerate().all(|(index, byte)| {
        unsafe { er_game_base::mem::safe_read_u8(osm + ersc::OSM_TAG_OFFSET + index) }
            .is_some_and(|got| got == *byte)
    })
}

/// The session state, or `None` when the value is not one a session would hold -- which is also
/// how a wrong pointer is rejected.
fn read_session_state(session: usize) -> Option<u32> {
    let raw =
        unsafe { er_game_base::mem::safe_read_i32(session + ersc::SESSION_STATE_OFFSET) }? as u32;
    (raw <= ersc::SESSION_STATE_MAX).then_some(raw)
}

/// True when the session is in the state both option actions refuse to proceed past. They take a
/// fatal-error branch on it; this refuses instead.
fn session_guard_poisoned(session: usize) -> bool {
    unsafe { er_game_base::mem::safe_read_i32(session + ersc::SESSION_GUARD_OFFSET) }
        .is_none_or(|raw| raw as u32 == ersc::SESSION_GUARD_POISON)
}

/// Log every session-state transition, and arm the auto re-search when the USER starts one.
///
/// Added 2026-08-05 because three separate failures in a row were mis-attributed from a log that
/// only recorded this module's own decisions. The session state is the variable everything here
/// turns on, and it was the one thing never written down. A transition line costs a dword read per
/// frame and turns "why did nothing happen" from a guess into a reading.
///
/// # Why arming lives here, on one specific transition
///
/// It used to be "the session is not idle, so a search must be running, so arm" -- and that is why
/// standing down when the menu opened did nothing: you open the menu DURING a search, the loop
/// stood down, and one frame later the session was still non-idle so it armed straight back up. A
/// live log caught it, `stood down` followed immediately by `0x11 -> 0x0d` and another automatic
/// restart.
///
/// The replacement rests on a fact from the static scan rather than on inference: across all 4839
/// functions in the unpacked `.text`, `S+0x110 = 0x0d` is written at EXACTLY ONE site, inside the
/// Invade-world action. So a transition into `0x0d` means that action ran and nothing else, and the
/// only remaining question is who ran it. Ours are claimed by [`note_state_after_our_action`]
/// before this ever sees them, so an unclaimed one is the user pressing the option -- which is
/// precisely, and only, when riding along is wanted.
fn trace_session_state(session: usize) {
    let Some(state) = read_session_state(session) else {
        return;
    };
    let previous = LAST_SESSION_STATE.swap(state as usize, Ordering::SeqCst);
    if previous == state as usize {
        return;
    }
    log_transition(previous, state, None);
    note_attempt_progress(previous, state);
    if state == ersc::SESSION_STATE_SEARCHING {
        // A new hunt is a new question; whatever the last one turned into is spent.
        INVASION_ACTUALLY_HAPPENED.store(false, Ordering::SeqCst);
    }
    if state == ersc::SESSION_STATE_SEARCHING && !AUTO_SEARCH_ARMED.swap(true, Ordering::SeqCst) {
        crate::standalone_log(format_args!(
            "local-invasion: you started a search -- rejected matches will be cancelled and the \
             search restarted until one lands somewhere you want, or you cancel it yourself"
        ));
    }
}

/// Record the state our own call produced, so the transition tracer does not mistake it for the
/// user acting.
///
/// This is what makes "who pressed Invade world" answerable at all. Our restart writes `0x0d` the
/// same way the option does, on the same thread, so by the time the next frame polls there is
/// nothing left to distinguish them -- unless we claim it first, which is what this does.
fn note_state_after_our_action(session: usize, what: &str) {
    let Some(state) = read_session_state(session) else {
        return;
    };
    let previous = LAST_SESSION_STATE.swap(state as usize, Ordering::SeqCst);
    if previous == state as usize {
        return;
    }
    log_transition(previous, state, Some(what));
}

/// Feed the restart backoff the shape of the attempt, from transitions it already sees.
///
/// Three facts are all it needs, and each is a single transition:
///   * leaving idle  -> an attempt began, start the clock
///   * reaching `0x12` -> this one is a real search, so clear any accumulated penalty
///   * reaching idle -> the attempt is over; how long it lasted decides the delay
///
/// `0x12` is the progress marker rather than a later state because it is the first step past the
/// fast-fail path: the measured spin ran `0x0d -> 0x0e -> 0x11 -> 0x14 -> idle` and never touched
/// it, while every healthy attempt in the same run passed through it within ~150 ms.
fn note_attempt_progress(previous: usize, state: u32) {
    let Ok(mut backoff) = RESTART_BACKOFF.lock() else {
        return;
    };
    if previous == ersc::SESSION_STATE_IDLE as usize && state != ersc::SESSION_STATE_IDLE {
        backoff.attempt_started(now_ms());
    }
    if state == SESSION_STATE_OFFER_RECEIVED {
        backoff.attempt_made_progress();
    }
}

/// The first state past the fast-fail path -- reaching it means Seamless really searched.
///
/// Named here rather than in `ersc` because it is OUR progress marker, chosen from measurement,
/// not a constant recovered from Seamless's own code.
const SESSION_STATE_OFFER_RECEIVED: u32 = 0x12;

/// The destination of the match in flight, remembered so the success banner can name it at the
/// moment the join actually LANDS rather than when the server first offered it. `usize::MAX` = no
/// match pending.
static PENDING_SUCCESS_BLOCK: AtomicUsize = AtomicUsize::new(usize::MAX);

/// This attempt actually became an invasion: the engine reported `LobbyState::Client`, which is
/// written only when the join RPC SUCCEEDED and the P2P session exists.
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
/// The dwell is reported in ticks AND in milliseconds because those two disagree only when the
/// frame rate changes, and that disagreement is the entire measurement — see [`TICKS`]. The implied
/// rate is printed so a pair of dwells taken at the same frame rate reads as inconclusive instead of
/// being mistaken for agreement between the two.
///
/// "Ticks", not "frames": this counts calls to [`tick`], which the recurring game task is expected
/// to make once per frame. The printed rate is what exposes that expectation if it is ever wrong —
/// a figure nowhere near the game's actual frame rate means the two have come apart, and the tick
/// column stops meaning what it says.
fn log_transition(previous: usize, state: u32, driven_by: Option<&str>) {
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
            format!("{previous:#04x} {}", state_name(previous as u32))
        },
        state,
        state_name(state),
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
const fn state_name(state: u32) -> &'static str {
    match state {
        ersc::SESSION_STATE_IDLE => "IDLE",
        ersc::SESSION_STATE_SEARCHING => "SEARCHING",
        ersc::SESSION_STATE_CANCELLING => "CANCELLING",
        _ => "(unreversed)",
    }
}

// ---------------------------------------------------------------------------------------------
// The one detour -- on the game
// ---------------------------------------------------------------------------------------------

/// `CS::SosSignMan::SetMultiplayJoinData(this, ServerPushJoinData*)`.
///
/// The seam the whole feature hangs on: the destination is decided, the server has told us, and
/// the player has not moved. The judgement happens BEFORE the original runs, so a reject is
/// decided against the incoming data rather than against a `CSGameMan` that has already been
/// written.
#[cfg(windows)]
unsafe extern "system" fn set_join_data_hook(a: usize, b: usize, c: usize, d: usize) -> usize {
    // THE MISSING CLOCK. This instant is the only honest "we got a connection" marker we have:
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
/// Shares the SAME hot-reloaded snapshot the reject filter judges with, so the two halves can never
/// disagree about what the user asked for -- a hunt filtering for one place while the reject filter
/// judged against another would be indistinguishable from a broken filter.
#[must_use]
pub fn current_config_snapshot() -> Option<LocalInvasionConfig> {
    current_config()
}

/// The advertisement lobby's `CSteamID`, read out of the resolved Seamless session.
///
/// Exposed so [`crate::lobby_publish`] can publish on the host's own lobby WITHOUT hooking
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
    let block = unsafe { er_invasion_warp::warp::current_block_id(base) }?;
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

/// How one map pin should look, given what the filter would do with an invasion landing there.
///
/// Reuses [`LocalInvasionConfig::judge`] rather than re-deriving the rules, so the map cannot tell
/// a different story from the filter. The only thing it adds is separating "kept because the user
/// marked it" from "kept because the mode allows it", which `judge` already distinguishes by
/// reason and which is the distinction the three tiers exist to show.
///
/// A pin whose block is unknown, or a filter that is switched off, reports
/// [`PinAppearance::Eligible`]: with no rules in force nothing is being excluded, and claiming
/// otherwise would paint a map full of rejections for a player who has not asked for any.
#[must_use]
pub fn pin_appearance_for(block: Option<u32>) -> PinAppearance {
    let Some(block) = block else {
        return PinAppearance::Eligible;
    };
    let Some(config) = current_config() else {
        return PinAppearance::Eligible;
    };
    match config.choice_for(block) {
        LocationChoice::Chosen => PinAppearance::Chosen,
        LocationChoice::Untouched => PinAppearance::Eligible,
        LocationChoice::Excluded => PinAppearance::Rejected,
    }
}

/// A hash of everything that can change a pin's ICON, for the injection cache's key.
///
/// The map's param rows are built once and shared across views, keyed on the spawn catalog. That
/// key is right for the spawn set and WRONG for the icons, because the icon now depends on the
/// user's lists too -- so without this the rows survive a mark and the map never changes. Mixing
/// this in makes a mark invalidate exactly what a mark affects.
///
/// The invasion-attempt state is mixed in for the identical reason one step removed: it selects the
/// bright-or-dimmed half of each tier's frame pair, so a search starting or ending while the map is
/// ALREADY OPEN has to invalidate the same cache a mark does. Without it the dim would only ever
/// appear on the next map open, which is exactly the case a player is least likely to hit -- you
/// notice the pins are unclickable by trying them, with the map already in front of you.
#[must_use]
pub fn pin_choice_signature() -> usize {
    let mut hash = fnv1a64(b"");
    let mut mix = |value: u64| {
        hash = fnv1a64_mix(hash, value);
    };
    mix(u64::from(
        er_invasion_warp::warp::invasion_attempt_in_flight(),
    ));
    let Some(config) = current_config() else {
        return hash as usize;
    };
    mix(u64::from(config.enabled));
    for block in &config.allowed_blocks {
        mix(u64::from(*block));
        mix(1);
    }
    for block in &config.blocked_blocks {
        mix(u64::from(*block));
        mix(2);
    }
    hash as usize
}

/// Count the tiers an injection produced, so "the map looks the same" is answerable from the log.
///
/// Added after a live run where all three marker frames were provably installed (+66 bytes, three
/// 22-byte placements) and the map was re-injected four times, yet every pin looked identical --
/// and nothing recorded which tier any pin got, so the cause could not be named. The tier is the
/// output of this feature; not logging it repeated the exact mistake that cost three wrong
/// attributions on the filter earlier.
pub fn log_pin_tier_tally(chosen: usize, untouched: usize, excluded: usize) {
    let enabled = current_config().is_some_and(|config| config.enabled);
    crate::standalone_log(format_args!(
        "map-inject: pin tiers chosen={chosen} untouched={untouched} excluded={excluded} \
         (filter_enabled={enabled}). All-one-number means the map cannot show a difference: mark \
         somewhere with Insert or exclude it with Delete."
    ));
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

/// Explain a rejection caused by MISSING information rather than by a wrong location, having first
/// established WHICH information is missing.
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

    // THE READS COME FIRST, AND THE SWITCH GATES THE ACTION RATHER THAN THE BANNER.
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

    // EVERY name the destination carries, not one of them. This was `.first()` of the list --
    // over a `BTreeSet` that is the numerically smallest id -- while the anchor compared against
    // all of its own names, so a destination sharing a name through any other of its names was
    // rejected as `WrongPlaceName`.
    if !config.enabled {
        // Nothing was judged, so there is no verdict to report -- just the destination, stated as
        // the server's choice rather than as anything the mod approved.
        announce_arrival(config.reject_notice, destination);
        return;
    }

    let candidate = InvasionCandidate::new(destination, place_names_for_block(destination));
    match config.judge(&anchor, &candidate) {
        Verdict::Keep(reason) => {
            KEEPS.fetch_add(1, Ordering::SeqCst);
            // The search that just landed is over; nothing to re-arm.
            AUTO_SEARCH_ARMED.store(false, Ordering::SeqCst);
            PENDING_REINVADE.store(false, Ordering::SeqCst);
            crate::standalone_log(format_args!(
                "local-invasion: KEEP {destination:#010x} ({reason:?}); anchor {:#010x} with {} \
                 named location(s)",
                anchor.block,
                anchor.named_location_count()
            ));
            // The banner for this does NOT fire here. A kept match is a match we allowed, not an
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
            announce_rejection(config.reject_notice, destination, reason);
            cancel_match(reason);
        }
    }
}

/// Host-side stub: there is no game to show a banner in, and the decision half is tested directly
/// against [`er_invasion_warp::reject_notice`] rather than through this.
#[cfg(not(windows))]
fn announce_rejection(_enabled: bool, _destination: u32, _reason: RejectReason) {}

/// Host-side stub.
#[cfg(not(windows))]
fn announce_arrival(_enabled: bool, _destination: u32) {}

/// Report a destination that arrived while the filter was switched off.
///
/// Shares the one notice latch with the verdict banners, so the surface never contradicts itself
/// about what it last said.
#[cfg(windows)]
fn announce_arrival(enabled: bool, destination: u32) {
    let announcement = {
        let mut guard = match REJECT_NOTICE.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let place = crate::place_name::place_name_for_block(destination);
        guard.observe_arrival(enabled, destination, place.as_deref())
    };
    let Some(text) = announcement else {
        return;
    };
    // SAFETY: game thread, inside the join-data hook -- the same context and surface as the
    // verdict banners.
    if !unsafe { crate::announce::show(&text) } {
        if NOTICE_FAILED.swap(true, Ordering::SeqCst) {
            return;
        }
        crate::standalone_log(format_args!(
            "local-invasion: could not show the arrival banner (\"{text}\") -- the message \
             functions did not verify, or the menu is not up yet."
        ));
    }
}

/// Host-side stub; the decision half is tested against [`er_invasion_warp::reject_notice`].
#[cfg(not(windows))]
fn announce_success(_enabled: bool, _destination: u32) {}

/// Put a successful invasion on the same banner the rejections use.
///
/// Shares [`RejectNotice`] with [`announce_rejection`] on purpose: one banner, one memory of what
/// it last said. That is what lets an arrival clear the rejection latch, so a later rejection at
/// the same place is announced instead of being swallowed as a repeat.
#[cfg(windows)]
fn announce_success(enabled: bool, destination: u32) {
    let announcement = {
        let mut guard = match REJECT_NOTICE.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let place = crate::place_name::place_name_for_block(destination);
        guard.observe_success(enabled, destination, place.as_deref())
    };
    let Some(text) = announcement else {
        return;
    };
    // SAFETY: game thread, inside the join-data hook -- the same context, and the same auto-closing
    // announcement surface, as the rejection banner.
    if !unsafe { crate::announce::show(&text) } {
        if NOTICE_FAILED.swap(true, Ordering::SeqCst) {
            return;
        }
        crate::standalone_log(format_args!(
            "local-invasion: could not show the success banner (\"{text}\") -- the message \
             functions did not verify, or the menu is not up yet. The invasion still happened; \
             only the on-screen notice is missing."
        ));
    }
}

/// Put a rejection on the game's system-message banner, if the player asked for that.
///
/// The decision of WHETHER to speak lives in [`er_invasion_warp::reject_notice`] and is unit-tested
/// on the host; this only carries the answer to the screen. The notice is fed even when the option
/// is off so that turning it on mid-session does not announce a place the player was rejected from
/// minutes ago as though it had just happened.
///
/// Runs on the game thread, in the same call that judges the match -- which is the context
/// `showPopupMenu` expects, and it null-checks the menu manager itself, so a message raised before
/// the UI exists is dropped rather than faulting.
#[cfg(windows)]
fn announce_rejection(enabled: bool, destination: u32, reason: RejectReason) {
    let announcement = {
        let mut guard = match REJECT_NOTICE.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        // Resolve the area's own name for the banner. Done here rather than inside the notice so
        // that type stays testable off the game: this is a call into the message repository.
        //
        // `None` before the world map has been read this session, which is the same condition that
        // makes `area` mode fail closed -- the notice falls back to the block id, which is
        // unfriendly but true.
        let place = crate::place_name::place_name_for_block(destination);
        guard.observe(enabled, destination, reason, place.as_deref())
    };
    let Some(text) = announcement else {
        return;
    };
    // The game's own auto-closing announcement surface -- the "Grace discovered" one. NOT
    // `system_message`/`showPopupMenu`, which is a blocking modal with an OK button: shipping that
    // gave the user a dialog to dismiss per rejection, showing squares and then nothing, and the
    // unattended dialog held the session open long enough to trip the stall watchdog.
    //
    // SAFETY: game thread, inside the join-data hook. Writes the live view's embedded message,
    // which is exactly what the view's own Update does when it pops one. Both game functions are
    // byte-checked before use.
    if !unsafe { crate::announce::show(&text) } {
        // Once, not per rejection: a banner that cannot be shown is a missing convenience, and
        // saying so every 20 seconds would be its own spam.
        if NOTICE_FAILED.swap(true, Ordering::SeqCst) {
            return;
        }
        crate::standalone_log(format_args!(
            "local-invasion: could not show the rejection banner (\"{text}\") -- the message \
             functions did not verify, or the menu is not up yet. Rejections still work; only the \
             on-screen notice is missing."
        ));
    }
}

/// Drive ERSC's own "Cancel search" for a rejected match.
///
/// This calls the exact option callback the user's click calls, with `(OSM, 0, 1, 1)`. The zero is
/// not a guess: `ersc+0x24460` reads `rcx` and nothing else, so no captured argument is required
/// and none is invented. Everything past this point -- tearing the match down, returning the
/// session to idle -- is Seamless's own code doing what it always does.
fn cancel_match(reason: RejectReason) {
    let session = match resolve_session() {
        Ok(session) => session,
        Err(cause) => {
            crate::standalone_log(format_args!(
                "local-invasion: cannot cancel ({reason:?}) -- {cause:?}, so the match is LEFT \
                 ALONE and will land wherever the server sent it{}",
                match cause {
                    NoSession::MenuNeverOpened =>
                        ". Open Seamless's menu once (that is where the object is learned) and the \
                         next rejection will cancel.",
                    _ => "",
                }
            ));
            return;
        }
    };
    if session_guard_poisoned(session.session) {
        crate::standalone_log(format_args!(
            "local-invasion: cannot cancel ({reason:?}) -- the session is in the state ERSC's own \
             actions refuse to proceed past; leaving it alone rather than tripping its abort path"
        ));
        return;
    }
    let Some(cancel) = ersc_action(ersc::CANCEL_ACTION_RVA, ersc::CANCEL_PROLOGUE) else {
        return;
    };
    IN_OUR_CALL.store(true, Ordering::SeqCst);
    unsafe { cancel(session.osm, 0, 1, 1) };
    IN_OUR_CALL.store(false, Ordering::SeqCst);
    note_state_after_our_action(session.session, "cancel");
    let fired = CANCELS.fetch_add(1, Ordering::SeqCst) + 1;
    // Search again once the session settles. Armed here, fired from the tick -- ERSC's own tick
    // does not run while the session is idle, which is why the frida attempt to re-invade from
    // inside an ERSC callback never fired.
    if AUTO_SEARCH_ARMED.load(Ordering::SeqCst) {
        PENDING_REINVADE.store(true, Ordering::SeqCst);
    }
    crate::standalone_log(format_args!(
        "local-invasion: cancelled rejected match (#{fired}) -- session returns to idle{}",
        if AUTO_SEARCH_ARMED.load(Ordering::SeqCst) {
            " and the search restarts automatically"
        } else {
            "; auto re-search is disarmed, so this stops here"
        }
    ));
}

/// Fire the queued re-invade once the session is genuinely idle.
///
/// Disarms BEFORE calling, so a session that fails to leave idle costs one extra invade at most
/// rather than one per frame.
fn drive_pending_reinvade(session: SeamlessSession) {
    if !PENDING_REINVADE.load(Ordering::SeqCst) || !AUTO_SEARCH_ARMED.load(Ordering::SeqCst) {
        return;
    }
    // `invade` returns immediately unless the session is idle, so this is the same precondition
    // ERSC itself enforces -- checked here so a no-op call is not counted as a restart.
    if read_session_state(session.session) != Some(ersc::SESSION_STATE_IDLE) {
        return;
    }
    if session_guard_poisoned(session.session) {
        PENDING_REINVADE.store(false, Ordering::SeqCst);
        return;
    }
    let Some(invade) = ersc_action(ersc::INVADE_ACTION_RVA, ersc::INVADE_PROLOGUE) else {
        PENDING_REINVADE.store(false, Ordering::SeqCst);
        return;
    };
    PENDING_REINVADE.store(false, Ordering::SeqCst);
    IN_OUR_CALL.store(true, Ordering::SeqCst);
    unsafe { invade(session.osm, 0, 1, 1) };
    IN_OUR_CALL.store(false, Ordering::SeqCst);
    // Claim the `0x0d` we just caused, before the tracer can read it as the user pressing the
    // option and arm a loop that is already armed.
    note_state_after_our_action(session.session, "restart search");
    let count = REINVADES.fetch_add(1, Ordering::SeqCst) + 1;
    crate::standalone_log(format_args!(
        "local-invasion: search restarted automatically (#{count}) -- press Cancel search yourself \
         to stop"
    ));
}

/// Re-arm the search when an attempt died WITHOUT us cancelling it.
///
/// # The gap this closes
///
/// [`drive_pending_reinvade`] only ever fired for a match WE rejected, because `PENDING_REINVADE`
/// is set in [`cancel_match`] and nowhere else. Every other way an attempt can end -- a host that
/// vanished, a connection that never completed, a refusal from the far side -- left the session
/// sitting at idle with the loop still armed and nothing to restart it, so the player had to reach
/// for the finger again. Measured 2026-08-06: of ten `0x15 -> 0x22` unwinds in one session, only
/// four were ours; the other six ended the hunt silently.
///
/// The standing instruction is that the loop runs until the player uses the lynchpin again, so
/// "the session went idle on its own while we are still hunting" is a restart, not a stop.
///
/// # Why this cannot resume a search after a SUCCESSFUL invasion
///
/// A successful join looks identical in session state -- `KEEP` was followed by the same
/// `0x15 -> 0x22 -> 0x23 -> 0x00` unwind a rejection produces, so idle alone cannot tell them
/// apart. It does not have to: [`Verdict::Keep`] clears `AUTO_SEARCH_ARMED`, so a kept match
/// leaves the loop disarmed and this function returns immediately. The same is true of the
/// player's own cancel and of opening Seamless's menu, both of which disarm.
fn arm_self_recovery(session: SeamlessSession) {
    if !AUTO_SEARCH_ARMED.load(Ordering::SeqCst) || PENDING_REINVADE.load(Ordering::SeqCst) {
        return;
    }
    if read_session_state(session.session) != Some(ersc::SESSION_STATE_IDLE) {
        return;
    }
    // AN INVASION THAT HAPPENED IS NOT AN ATTEMPT THAT DIED.
    //
    // The doc above assumed `Verdict::Keep` would have disarmed the loop first. Measured
    // 2026-08-17, it does not: that session logged ZERO keeps and eleven rejects (`mode=area` with
    // no named locations rejects everything it judges), so the loop stayed armed through three real
    // invasions -- and this function restarted the hunt while the player was still on the loading
    // screen back to their own world. They arrived home coloured as an invader with a Seamless name
    // popup reading `[Unknown]`, because a fresh invasion was already in flight.
    //
    // So the disarm is taken from what the ENGINE did rather than from what our filter decided.
    if INVASION_ACTUALLY_HAPPENED.swap(false, Ordering::SeqCst) {
        // Disarm as a kept match would have: the hunt is over until the player asks for another.
        AUTO_SEARCH_ARMED.store(false, Ordering::SeqCst);
        if let Ok(mut backoff) = RESTART_BACKOFF.lock() {
            backoff.stand_down();
        }
        crate::standalone_log(format_args!(
            "local-invasion: that attempt became a real invasion (the session reached \
             LobbyState::Client) -- NOT restarting the hunt. Use the lynchpin again when you want \
             another one"
        ));
        return;
    }
    // HOW BADLY DID THE LAST ATTEMPT GO? Restarting instantly is right when Seamless actually
    // searched -- its own ~15s retry paces the loop and nothing here is felt. It is wrong when
    // Seamless refused instantly: measured 2026-08-06, eleven restarts in 38.9s during an area
    // transition, four times the normal query rate, because idle alone cannot tell a 15-second
    // search from a 33-millisecond refusal.
    {
        let now = now_ms();
        let Ok(mut backoff) = RESTART_BACKOFF.lock() else {
            return;
        };
        let delay = backoff.attempt_ended(now);
        if !backoff.may_restart(now) {
            // Held. Return WITHOUT arming. The next tick re-enters, finds no recorded start (the
            // attempt was already consumed), scores that as a normal attempt costing nothing, and
            // simply re-checks the hold -- so the delay elapses without accumulating further
            // penalty, and the restart fires on the first tick after it expires.
            if delay > 0 {
                crate::standalone_log(format_args!(
                    "local-invasion: that attempt was refused in under a second (#{} in a row) -- \
                     waiting {delay}ms before searching again, so a passing refusal does not turn \
                     into a query storm. The hunt is still on.",
                    backoff.consecutive()
                ));
            }
            return;
        }
    }
    PENDING_REINVADE.store(true, Ordering::SeqCst);
    let count = SELF_RECOVERIES.fetch_add(1, Ordering::SeqCst) + 1;
    crate::standalone_log(format_args!(
        "local-invasion: the attempt ended without us cancelling it (#{count}) -- restarting the \
         search, because you have not stopped hunting. Press Cancel search yourself to stop"
    ));
}

/// Cancel an attempt that has stopped progressing, so the loop can recover from a Seamless stall.
///
/// Seamless does not auto-cancel its own connection in these edge cases, which is why a hung
/// handshake otherwise sits forever. The action driven here is the same "Cancel search" the player
/// could press, and the restart afterwards is the ordinary one -- nothing here ends the hunt.
fn cancel_stalled_attempt(session: SeamlessSession, state: u32, held_ms: u64) {
    if session_guard_poisoned(session.session) {
        return;
    }
    let Some(cancel) = ersc_action(ersc::CANCEL_ACTION_RVA, ersc::CANCEL_PROLOGUE) else {
        return;
    };
    IN_OUR_CALL.store(true, Ordering::SeqCst);
    unsafe { cancel(session.osm, 0, 1, 1) };
    IN_OUR_CALL.store(false, Ordering::SeqCst);
    note_state_after_our_action(session.session, "cancel stalled attempt");
    if AUTO_SEARCH_ARMED.load(Ordering::SeqCst) {
        PENDING_REINVADE.store(true, Ordering::SeqCst);
    }
    let count = STALL_RECOVERIES.fetch_add(1, Ordering::SeqCst) + 1;
    crate::standalone_log(format_args!(
        "local-invasion: connection stalled at state {state:#04x} for {held_ms}ms (#{count}) -- \
         cancelled it. Seamless does not auto-cancel these, and a healthy handshake takes under \
         two seconds"
    ));
}

/// Feed the session state to the stall detector and act on what it says.
///
/// Deliberately state-driven rather than time-capped: `SEARCHING` means "nobody has matched yet"
/// and is unbounded by nature, so it is never timed. Only the brief handshake steps are.
fn watch_for_stall(session: SeamlessSession) {
    // ONLY RECOVER WHILE ACTUALLY HUNTING. If the loop is not armed there is nothing to recover,
    // and running anyway is how this cancelled a SUCCESSFUL invasion five seconds after accepting
    // it (2026-08-06): `Verdict::Keep` fired, the session sat in 0x15 loading the host's world,
    // and the watchdog called that a stalled handshake. From the player's seat the invasion
    // appeared and dismissed itself at once.
    //
    // Note this cannot be fixed by choosing better states to time: a successful join walks 0x22
    // and 0x23 exactly like a cancel does. Whether we are still hunting is the ONLY thing that
    // separates "this handshake is stuck" from "this invasion is under way", and `Verdict::Keep`
    // already clears the armed flag, as do the player's own cancel and opening Seamless's menu.
    if !AUTO_SEARCH_ARMED.load(Ordering::SeqCst) {
        if let Ok(mut guard) = STALL_WATCHDOG.lock() {
            guard.stand_down();
        }
        // The backoff stands down here TOO, on the same condition and in the same place, rather
        // than at each of the sites that disarm. There are three of those today -- a kept match,
        // the player's own cancel, opening Seamless's menu -- and a fourth added later would
        // silently miss a per-site call. This branch already runs every tick the loop is not
        // armed, so it cannot be forgotten. Without it, a hunt stopped mid-backoff would hand its
        // penalty to the next one the player starts.
        if let Ok(mut backoff) = RESTART_BACKOFF.lock() {
            backoff.stand_down();
        }
        return;
    }
    let Some(state) = read_session_state(session.session) else {
        return;
    };
    let now_ms = now_ms();
    let action = {
        let mut guard = match STALL_WATCHDOG.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.observe(state, now_ms)
    };
    if action == Some(crate::stall_watchdog::StallAction::CancelAndResearch) {
        cancel_stalled_attempt(session, state, crate::stall_watchdog::STALL_THRESHOLD_MS);
    }
}

/// Publish whether an invasion attempt is in flight, for the warp gate and the map's icon choice.
///
/// # Why "not idle" and not "== SEARCHING"
///
/// `0x0d` is only the FIRST state of an attempt. The sequence runs through `0x0e`, `0x11`, the
/// `0x12` offer, `0x13`, `0x14`, and a cancel unwinds via `0x22`/`0x23` -- and the player is just
/// as committed at every one of them as at `0x0d`. Gating on `SEARCHING` alone would unblock the
/// warp the instant a host was found, which is the worst possible moment for it: the destination
/// has been decided and the player is about to be moved there by Seamless.
///
/// Anything that is not [`ersc::SESSION_STATE_IDLE`] therefore counts, including the states no
/// instruction in ersc's plaintext `.text` writes (its middle is virtualised). That is the safe
/// direction for an unknown state: an unrecognised value means SOMETHING is happening, and the
/// honest response to "I do not know what this state is" is to leave the pins alone.
///
/// A state that cannot be read at all is treated as no attempt, matching the no-session case: a
/// read that fails is not evidence of an invasion.
#[cfg(windows)]
fn publish_invasion_attempt_state(session: usize) {
    let in_flight =
        read_session_state(session).is_some_and(|state| state != ersc::SESSION_STATE_IDLE);
    er_invasion_warp::warp::set_invasion_attempt_in_flight(in_flight);
}

/// True once the session has settled back to idle after a cancel.
#[must_use]
pub fn session_is_idle() -> bool {
    resolve_session()
        .ok()
        .and_then(|session| read_session_state(session.session))
        .is_some_and(|state| state == ersc::SESSION_STATE_IDLE)
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
/// read out of the shipped DLL. A Seamless update that moves these functions disarms the filter;
/// it must never make it call into the middle of an instruction.
fn ersc_action(rva: usize, prologue: &[u8]) -> Option<ErscActionFn> {
    let base = ersc_module_base().or_else(|| {
        crate::standalone_log(format_args!(
            "local-invasion: ersc.dll not loaded -- nothing to filter"
        ));
        None
    })?;
    let address = base + rva;
    if !prologue_matches(address, prologue) {
        crate::standalone_log(format_args!(
            "local-invasion: ersc+{rva:#x} does not start with the bytes this build expects -- \
             refusing to call it. The filter is disarmed until the RVAs are re-read against this \
             ersc.dll."
        ));
        return None;
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
            crate::standalone_log(format_args!(
                "local-invasion: judging matches at {} @0x{address:x}",
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

/// The keys that mark and un-mark, read from the config every poll.
///
/// They used to be the hard-coded constants `VK_INSERT`/`VK_DELETE`. A 60% keyboard has NEITHER,
/// which locked the marking feature out entirely for anyone using one -- so the pair now comes from
/// `mark_key` / `unmark_key` in the config, by name. Read per poll rather than latched at startup
/// so a hand-edit takes effect on the same hot reload as every other setting.
///
/// Falls back to the historical defaults when the config is unreadable: losing the config should
/// cost the player their lists, not their keyboard.
#[cfg(windows)]
fn mark_keys_in_force() -> (i32, i32) {
    current_config().map_or(
        (
            er_invasion_warp::keybind::VK_INSERT,
            er_invasion_warp::keybind::VK_DELETE,
        ),
        |config| (config.mark_key, config.unmark_key),
    )
}
/// `VK_SHIFT`: held, the mark keys act on the location's NAME instead of its exact block --
/// "everywhere that shares this name" rather than "this tile".
#[cfg(windows)]
const VK_SHIFT: i32 = 0x10;

#[cfg(windows)]
const KEY_DOWN_MASK: i16 = -0x8000;
#[cfg(windows)]
const KEY_PRESSED_SINCE_MASK: i16 = 0x0001;

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetAsyncKeyState(vkey: i32) -> i16;
}

/// Edge-detected mark keys.
///
/// Deliberately a private copy of the pattern in `drive.rs` rather than a shared one: both bits of
/// `GetAsyncKeyState` are consumed by a read, and the low "pressed since last call" bit is
/// PER-CALL, so two pollers sharing one key would eat each other's edge. These keys are distinct
/// from the warp driver's F7/F8/F9, so the two pollers never contend.
#[cfg(windows)]
#[derive(Default)]
pub struct MarkKeys {
    mark_was_down: bool,
    unmark_was_down: bool,
}

#[cfg(windows)]
impl MarkKeys {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            mark_was_down: false,
            unmark_was_down: false,
        }
    }

    fn edge(vkey: i32, was_down: &mut bool) -> bool {
        let state = unsafe { GetAsyncKeyState(vkey) };
        let down = (state & KEY_DOWN_MASK) != 0;
        let edge = (down && !*was_down) || (state & KEY_PRESSED_SINCE_MASK) != 0;
        *was_down = down;
        edge
    }

    /// Poll both keys and apply whatever they asked for.
    ///
    /// Shift is read with the DOWN bit only. Consuming its "pressed since" latch would make a
    /// held Shift look released on the second key press.
    fn poll(&mut self) {
        let (mark_key, unmark_key) = mark_keys_in_force();
        // BOTH edges are read every poll, even when the two keys are the same. `GetAsyncKeyState`
        // consumes its own "pressed since" latch per call, so skipping one read would eat the
        // other's edge -- and a config that names one key for both would then fire neither.
        let mark = Self::edge(mark_key, &mut self.mark_was_down);
        let unmark = if unmark_key == mark_key {
            false
        } else {
            Self::edge(unmark_key, &mut self.unmark_was_down)
        };
        if !mark && !unmark {
            return;
        }
        let by_name = (unsafe { GetAsyncKeyState(VK_SHIFT) } & KEY_DOWN_MASK) != 0;
        if mark {
            apply_mark(true, by_name);
        }
        if unmark {
            apply_mark(false, by_name);
        }
    }

    /// Forget the latches when the game does not have focus, so pressing Delete in another window
    /// does not silently edit the config.
    fn forget(&mut self) {
        self.mark_was_down = false;
        self.unmark_was_down = false;
    }
}

/// Add or remove the player's current location, by block or by name, and write the file.
#[cfg(windows)]
fn apply_mark(adding: bool, by_name: bool) {
    let Some(anchor) = current_anchor() else {
        crate::standalone_log(format_args!(
            "local-invasion: cannot mark -- the player's location is not readable right now"
        ));
        return;
    };
    let path = config_path();
    let Ok(mut guard) = CONFIG.lock() else { return };
    let hot = guard.get_or_insert_with(HotConfig::default);
    // Pick up any hand-edit first, so a keypress extends the file the user has rather than
    // overwriting it with a stale in-memory copy.
    let _ = hot.reload_if_changed(&path);
    let mut config = hot.current().clone();

    let changed = if by_name {
        let count = if adding {
            config.mark_place_names(&anchor)
        } else {
            config.unmark_place_names(&anchor)
        };
        if count == 0 && adding && anchor.named_location_count() == 0 {
            crate::standalone_log(format_args!(
                "local-invasion: {:#010x} has no place name on record, so there is nothing to mark \
                 by name. Open the world map once this session -- that is where the names are read \
                 from.",
                anchor.block
            ));
            return;
        }
        count > 0
    } else if adding {
        config.mark_block(anchor.block)
    } else {
        config.unmark_block(anchor.block)
    };

    if !changed {
        crate::standalone_log(format_args!(
            "local-invasion: {} {:#010x}{} -- already in that state, file untouched",
            if adding { "mark" } else { "un-mark" },
            anchor.block,
            if by_name { " by name" } else { "" }
        ));
        return;
    }

    match hot.save(&path, &config) {
        Ok(true) => crate::standalone_log(format_args!(
            "local-invasion: {} {:#010x}{} -- now {} chosen, {} excluded, {} name(s){}",
            if adding { "MARKED" } else { "EXCLUDED" },
            anchor.block,
            if by_name { " by name" } else { "" },
            config.allowed_blocks.len(),
            config.blocked_blocks.len(),
            config.named_location_text_ids.len(),
            if config.enabled {
                ""
            } else {
                " (the filter itself is still OFF -- set enabled = true)"
            }
        )),
        Ok(false) => crate::standalone_log(format_args!(
            "local-invasion: WROTE the config but it did not read back identically -- the mark may \
             not survive. This is a bug in the config writer, not in your file."
        )),
        Err(error) => crate::standalone_log(format_args!(
            "local-invasion: could not write {}: {error} -- the mark was NOT saved",
            path.display()
        )),
    }
}

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
    // Counted before any early return, so a dwell measured across a stretch where Seamless was not
    // resolvable still reflects the frames that actually passed.
    TICKS.fetch_add(1, Ordering::SeqCst);
    install_join_hook();
    install_show_observer();
    // Learns the live announcement view. Self-gating and idempotent: it needs the menu system,
    // which does not exist at attach, so it retries until it lands rather than failing silently
    // once. Costs one byte-check per tick until then.
    crate::announce::install();
    // Read back what the game measured for the last notice. Deliberately OUTSIDE the Seamless
    // early-return below: a notice can be on screen while the session is unresolvable, and the
    // whole point of this check is that it does not depend on the path that placed the notice.
    crate::announce::poll_measurement();
    // Read-only, and independent of the filter: it reports the one string that decides whether two
    // Seamless players can find each other at all.
    install_lobby_key_observer();
    if game_has_focus {
        keys.poll();
    } else {
        keys.forget();
    }
    // Phase 1 measurement, ABOVE the Seamless gate on purpose. These are ENGINE fields: they do
    // not depend on ERSC being resolvable, and the baseline of what they read during ordinary play
    // is exactly as valuable as what they read mid-attempt. Gating them behind `resolve_session`
    // would have recorded nothing at all until the player opened the Seamless menu.
    trace_join_progress(
        resolve_session()
            .ok()
            .and_then(|s| read_session_state(s.session)),
    );
    // Everything below is Seamless-side and purely observational until a rejected match has
    // actually armed a re-search, so a run without Seamless loaded costs one failed module lookup.
    let Ok(session) = resolve_session() else {
        // No session means no attempt, which is a DEFINITE answer rather than a failure to read
        // one: with Seamless absent or not yet up there is nothing to be mid-invasion of. Publish
        // it, so a session that goes away cannot strand the map dimmed and the warp refused.
        er_invasion_warp::warp::set_invasion_attempt_in_flight(false);
        return;
    };
    publish_invasion_attempt_state(session.session);
    trace_session_state(session.session);
    // Watch WHICH session fields the Themida VM writes, and when. This is the only way left to
    // learn the invasion state machine: its middle is virtualized, and a live dump proved there is
    // no plaintext to recover (ersc's .themida is 99.68% identical on disk and in memory, entropy
    // unchanged -- the original x86 does not exist at runtime).
    trace_session_field_writes(session.session);
    // Order matters. The stall detector may cancel, which lands the session at idle; self-recovery
    // then sees that idle and arms the restart; `drive_pending_reinvade` fires it. Running them in
    // this order recovers a stalled attempt within one tick of it settling rather than three.
    watch_for_stall(session);
    arm_self_recovery(session);
    drive_pending_reinvade(session);
}

/// Read the engine's own view of the join, and log it when it changes.
///
/// PHASE 1 IS MEASUREMENT ONLY: this decides nothing and cancels nothing. It exists to produce
/// the three traces the detector's threshold has to come from -- a healthy reject, a dead KEEP,
/// and a real invasion -- because the only numbers we have today describe the ERSC side, whose
/// middle states are virtualised and whose `0x15` is not a stage at all.
///
/// # Safety
/// Game task thread. Every read is fault-closed through `safe_read_*`; a null or stale singleton
/// yields `None` and the sample is skipped rather than faulting.
#[cfg(windows)]
fn trace_join_progress(ersc_state: Option<u32>) {
    let Some(progress) = read_join_progress() else {
        return;
    };
    let verdict = progress.verdict();
    // `Client` and nothing else. `call_for_warp` is tempting and WRONG: `WarpNextStageKick_` runs
    // for every warp including a plain fast travel, so latching on it would mark an ordinary grace
    // warp as an invasion.
    if progress.lobby_state == er_invasion_warp::join_progress::lobby_state::CLIENT
        && !INVASION_ACTUALLY_HAPPENED.swap(true, Ordering::SeqCst)
    {
        // The join landed. THIS is the moment "Invasion successful" is true -- measured at
        // 0.57-3.5s after join data on every real join, and never reached by a match that dies.
        let pending = PENDING_SUCCESS_BLOCK.swap(usize::MAX, Ordering::SeqCst);
        if pending != usize::MAX
            && let Some(config) = current_config()
        {
            announce_success(config.reject_notice, pending as u32);
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
    let ersc_claims_attempt = ersc_state.is_some_and(|state| state != ersc::SESSION_STATE_IDLE);
    if verdict == er_invasion_warp::join_progress::Verdict::Idle && ersc_claims_attempt {
        JOIN_PROGRESS_IDLE_SAMPLES.fetch_add(1, Ordering::Relaxed);
    }
    if JOIN_PROGRESS_LAST.swap(packed, Ordering::SeqCst) == packed {
        return;
    }
    let since_join = match JOIN_DATA_AT_MS.load(Ordering::SeqCst) {
        0 => String::new(),
        at => format!(" +{}ms since join data", now_ms().saturating_sub(at)),
    };
    crate::standalone_log(format_args!(
        "join-progress: ersc={} {progress}{since_join}",
        match ersc_state {
            Some(state) => format!("{state:#04x}"),
            None => "<unresolved>".to_owned(),
        }
    ));
}

/// One fault-closed sample of the engine-side join fields.
#[cfg(windows)]
fn read_join_progress() -> Option<er_invasion_warp::join_progress::JoinProgress> {
    use er_invasion_warp::join_progress as jp;
    use er_invasion_warp::warp::{
        SESSION_LOBBY_STATE_OFFSET, SESSION_MANAGER_GLOBAL_RVA, SESSION_PROTOCOL_STATE_OFFSET,
    };

    let base = er_game_base::mem::game_module_base().ok()?;
    let manager = unsafe { er_game_base::mem::safe_read_usize(base + SESSION_MANAGER_GLOBAL_RVA) }?;
    if manager == 0 {
        return None;
    }
    let game_man = unsafe { er_game_base::mem::safe_read_usize(base + jp::GAME_MAN_GLOBAL_RVA) }?;
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
/// Deliberately NOT `cfg(windows)`: it is plain arithmetic, and the tests that prove the window
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
/// States `0x0E`, `0x11`, `0x12`, `0x13` and `0x14` are written by NO instruction in ersc's
/// readable code -- a byte-anchored scan for `C7 /0 disp32=0x110 imm32` finds only
/// `{0,1,3,6,9,0xD,0x22,0x23}`, and the sole register-sourced write produces `0x0C`/`0x15`. The
/// rest come out of the Themida VM. Reading that code is not available: a live dump of the module
/// showed `.themida` is 99.68% byte-identical to disk with unchanged entropy, so the original
/// instructions never exist in memory to be recovered.
///
/// What IS available is the effect. Every field the VM writes is written into an object this
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
fn trace_session_field_writes(session: usize) {
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
    let state = unsafe { er_game_base::mem::safe_read_i32(session + ersc::SESSION_STATE_OFFSET) }
        .unwrap_or(-1);
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
fn trace_session_field_writes(_session: usize) {}

/// `(keeps, cancels, automatic re-searches)` so a run can be judged without reading the log.
#[must_use]
pub fn tallies() -> (usize, usize, usize) {
    (
        KEEPS.load(Ordering::SeqCst),
        CANCELS.load(Ordering::SeqCst),
        REINVADES.load(Ordering::SeqCst),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// EVERY option that changes behaviour has to show up in the `config loaded` line.
    ///
    /// Not a style rule -- it is the difference between a hot-reload you can verify and one you can
    /// only hope about. Measured cost of the gap, 2026-08-06: `dll_users_only` was toggled mid-run,
    /// the line reprinted (so the file had certainly been re-read), and it still did not say what
    /// the option was now set to. The A/B could not be confirmed until a `lobby-pool` line happened
    /// to appear on the next query. "The config reloaded" is a fact nobody needs; "here is what is
    /// in force" is the one they do.
    ///
    /// Checked against the struct's real field list rather than a copy of it, so adding a field and
    /// forgetting the log line fails HERE instead of silently on someone's live run.
    #[test]
    fn every_behaviour_changing_option_is_named_in_the_config_line() {
        let config_source = include_str!("../../er-invasion-warp/src/local_invasion.rs");
        let start = config_source
            .find("pub struct LocalInvasionConfig")
            .expect("the config struct");
        let body = &config_source[start..];
        let end = body.find("\n}").expect("end of the struct");
        let fields: Vec<&str> = body[..end]
            .lines()
            .filter_map(|line| line.trim().strip_prefix("pub "))
            // A field declaration, not the `pub struct ... {` header the scrape starts on: it must
            // carry a type, and its name must be a plain identifier.
            .filter(|rest| rest.contains(':'))
            .filter_map(|rest| rest.split(':').next())
            .map(str::trim)
            .filter(|name| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
            .collect();
        assert!(
            fields.len() >= 8,
            "field scrape found only {fields:?} -- the struct's shape changed and this test is no \
             longer reading it"
        );

        let source = include_str!("local_invasion_filter.rs");
        let line_start = source
            .find("local-invasion: config loaded")
            .expect("the config-loaded log line");
        // The format string plus its whole argument list, bounded by the call's own closing `));`
        // rather than by a character count -- a fixed window silently truncates the moment a
        // comment is added to the argument list, and then this test starts failing on fields that
        // are in fact present.
        let call_end = source[line_start..]
            .find("));")
            .expect("the log call must be closed");
        // COMMENTS ARE STRIPPED, and the check is for the READ ITSELF rather than the bare name.
        // Both guards earn their place: a first version of this test looked for the bare field name
        // anywhere in the call, and a negative control proved it toothless -- the explanatory
        // comment in the argument list mentions `dll_users_only`, so deleting the actual argument
        // left the test passing on prose alone. A gate that cannot fail is worse than no gate,
        // because it is mistaken for coverage.
        let call: String = source[line_start..line_start + call_end]
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        for field in fields {
            // The two keybinds are reported by NAME rather than by number, so they appear as
            // `key_name(outcome.config.mark_key)` -- still the read, just rendered for a human.
            assert!(
                call.contains(&format!("outcome.config.{field}")),
                "config option `{field}` never reaches the `config loaded` line, so a user who \
                 changes it cannot tell from the log whether it took effect"
            );
        }
    }

    /// The dwell figure exists to be compared against the game's real frame rate; one that rounds
    /// to zero on a plausible interval would read as "the task stopped ticking" and send the next
    /// reader hunting a hang that never happened.
    #[test]
    fn a_dwell_at_a_normal_frame_rate_reports_that_frame_rate() {
        // 600 ticks over 10s -- the shape of the measured `0x11` retry dwell at 60fps.
        assert_eq!(implied_fps(600, 10_000), Some(60));
        // The same wall-clock dwell at half the frame rate: HALF the ticks. This is the frame-vs-
        // clock discriminator in miniature -- if the retry is a clock, this is what the log shows.
        assert_eq!(implied_fps(300, 10_000), Some(30));
        // The same TICK dwell at half the frame rate: twice the wall clock. If the retry is a frame
        // counter, this is what the log shows instead. The two are distinguishable only because
        // both columns are recorded.
        assert_eq!(implied_fps(600, 20_000), Some(30));
    }

    /// Degenerate intervals must decline to divide rather than emit a misleading number: two
    /// transitions inside one tick say nothing about the frame rate.
    #[test]
    fn an_interval_too_short_to_divide_reports_nothing_rather_than_zero() {
        assert_eq!(implied_fps(0, 10_000), None, "no ticks elapsed");
        assert_eq!(implied_fps(600, 0), None, "no time elapsed");
        assert_eq!(implied_fps(0, 0), None);
    }

    /// A dwell shorter than a millisecond per tick must not be reported as zero fps -- it is a very
    /// FAST interval, and zero would read as a stall.
    #[test]
    fn a_sub_millisecond_dwell_never_reports_as_a_stalled_task() {
        assert_ne!(implied_fps(1, 1), Some(0));
        assert_eq!(implied_fps(1, 1), Some(1000));
    }

    #[test]
    fn the_default_mark_keys_are_insert_and_delete_and_are_distinct_from_the_warp_keys() {
        let defaults = er_invasion_warp::local_invasion::LocalInvasionConfig::default();
        assert_eq!(defaults.mark_key, 0x2d, "VK_INSERT");
        assert_eq!(defaults.unmark_key, 0x2e, "VK_DELETE");
        // Sharing a key with the warp driver would make the two pollers eat each other's
        // GetAsyncKeyState "pressed since last call" edge.
        //
        // Only the DEFAULTS can be checked here: the keys are configurable now, so a player is free
        // to name a warp key and collide on purpose. That is their choice to make and the log line
        // reports which keys are live, but the shipped defaults must not collide out of the box.
        for warp_key in [
            crate::drive::VK_WARP_NEAREST,
            crate::drive::VK_WARP_NEXT,
            crate::drive::VK_WARP_OTHER_AREA,
        ] {
            assert_ne!(defaults.mark_key, warp_key);
            assert_ne!(defaults.unmark_key, warp_key);
        }
    }

    /// A player who names a key must be able to SEE which key is live, or a typo that parsed into
    /// some other valid key is indistinguishable from the feature being broken.
    #[test]
    fn the_configured_keys_render_names_a_player_would_recognise() {
        let defaults = er_invasion_warp::local_invasion::LocalInvasionConfig::default();
        assert_eq!(
            er_invasion_warp::keybind::key_name(defaults.mark_key),
            "Insert"
        );
        assert_eq!(
            er_invasion_warp::keybind::key_name(defaults.unmark_key),
            "Delete"
        );
    }

    #[test]
    fn this_module_installs_exactly_three_detours_and_both_seamless_ones_are_read_only() {
        // The budget, made explicit so growing it is a decision rather than a drift:
        //   ORIG_SET_JOIN_DATA    -- the GAME's SetMultiplayJoinData, where matches are judged.
        //   ORIG_SHOW             -- ersc's menu builder, observation only, because OSM has no
        //                            static to read it out of (see NEXT_OBJECT_OFFSET's docs).
        //   ORIG_BUILD_LOBBY_KEY  -- ersc's lobby-key builder, observation only. Grown from two to
        //                            three deliberately: the key is the single value that decides
        //                            whether two Seamless players can see each other, it exists
        //                            only as a stack `std::string` inside one function, and no
        //                            field holds it afterwards -- so there is nothing to read
        //                            passively and a hook is the only way to observe it.
        // The two option ACTIONS are deliberately not hooked: they read `rcx` only, so calling
        // them with `(OSM, 0, 1, 1)` needs no captured arguments and therefore no detour.
        let source = include_str!("local_invasion_filter.rs");
        let orig_slots = source.matches("\nstatic ORIG_").count();
        assert_eq!(orig_slots, 3, "detour budget is three trampolines");
        assert!(source.contains("\nstatic ORIG_SET_JOIN_DATA"));
        assert!(source.contains("\nstatic ORIG_SHOW"));
        assert!(source.contains("\nstatic ORIG_BUILD_LOBBY_KEY"));
        for banned in ["ORIG_INVADE_ACTION", "ORIG_CANCEL_ACTION"] {
            assert!(
                !source.contains(&format!("static {banned}")),
                "{banned}: the option actions must stay un-hooked -- they ignore every argument \
                 past rcx, so there is nothing to capture"
            );
        }
    }

    /// The lobby-key observer must stay an OBSERVER. Altering `lobby_key` changes what every other
    /// Seamless client's filter matches, which is not this DLL's to do.
    ///
    /// AMENDED 2026-08-06, deliberately and narrowly. The original banned `SetLobbyData(` and
    /// `AddRequestLobbyListStringFilter(` outright, which was broader than its own stated reason:
    /// the harm it names is to `lobby_key`, and publishing a SEPARATE namespaced key does not touch
    /// it. Measured that day: a host publishes 7 keys and an invader filters on 5 of them, and a
    /// lobby lacking a filtered key is EXCLUDED (baseline 13 lobbies -> 0 with a filter on an
    /// unpublished key, reproduced). So one extra key is exactly how a location filter can exist,
    /// and it is invisible to vanilla Seamless players -- they never query it, and their own
    /// matching is on `lobby_key`, which stays untouched.
    ///
    /// What remains absolute: `lobby_key` itself is never written, and no filter is ever added on
    /// `lobby_key` or `lobby_type`. Those two decide who can see whom at all.
    #[test]
    fn the_lobby_key_is_never_published_or_altered() {
        let code = product_code();
        // The keys that decide MUTUAL VISIBILITY. Writing or filtering on either changes what other
        // players match, so they stay banned by name rather than by call.
        for reserved in ["lobby_key", "lobby_type"] {
            assert!(
                !code.contains(&format!("\"{reserved}\"")),
                "{reserved}: this key decides who can see whom -- publishing or filtering on it \
                 changes what every other Seamless player matches"
            );
        }
        // And the observer calls the original before reading, so a fault in our read can never
        // change the key Seamless publishes.
        let source = include_str!("local_invasion_filter.rs");
        let observer = source
            .split("unsafe extern \"system\" fn build_lobby_key_observer")
            .nth(1)
            .expect("the observer exists");
        let body = &observer[..observer.find("\n}\n").unwrap_or(observer.len())];
        let call_at = body.find("ErscActionFn").expect("calls the trampoline");
        let read_at = body
            .find("read_std_string")
            .expect("reads the produced string");
        assert!(
            call_at < read_at,
            "the original must run before the string is read"
        );
    }

    /// This module's SHIPPING code, with comments and the test module removed.
    ///
    /// Both exclusions are load-bearing and were learned by the guards failing on themselves:
    /// * COMMENTS, because these names appear throughout the documentation, where describing what
    ///   Seamless does is the entire point. A check that cannot tell prose from a call site either
    ///   fails on its own docs or forces the docs to go quiet about the mechanism.
    /// * THE TEST MODULE, because a ban list is written in code -- `["lobby_key", ...]` is a string
    ///   literal, so a guard scanning the whole file trips on the very list that defines it. Both
    ///   new guards failed exactly that way on first run.
    fn product_code() -> String {
        let source = include_str!("local_invasion_filter.rs");
        let shipping = source
            .split_once("#[cfg(test)]")
            .map_or(source, |(before, _)| before);
        shipping
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// No invasion target may ever be chosen by WHO the other player is.
    ///
    /// # The line, and why it is drawn here rather than left to judgement
    ///
    /// Measured 2026-08-06: a lobby query returns a real candidate SET (13 lobbies), ersc picks one
    /// (index 14, then 7), and `GetLobbyOwner` / `GetLobbyMemberByIndex` are both called on the
    /// result. So selecting a candidate by SteamID is not merely conceivable -- every primitive it
    /// needs is already in the process, it needs nothing from the host, and it would work today.
    ///
    /// That is exactly what makes it unacceptable. Its one advantage and its one abuse are the SAME
    /// property: needing nothing from the target. Any consent check would require the target to run
    /// this DLL, which removes the advantage entirely, so there is no version of it that is both
    /// useful and safe. And a location is somewhere a player chose to stand; an account is the
    /// player, everywhere, forever.
    ///
    /// The principle this encodes, which generalises past this one call: filtering may DECLINE, it
    /// may never SELECT A PERSON. Declining removes options from ourselves -- the worst case is not
    /// invading someone, which the user could do by hand. Selecting imposes on somebody who never
    /// opted in. `er_map` passes because a host is findable by location only if that host chose to
    /// broadcast it; consent is structural rather than a policy someone can quietly drop.
    #[test]
    fn no_invasion_target_is_ever_chosen_by_steam_id() {
        let code = product_code();
        // The primitives that turn a candidate set into a named person. Reading an owner to LOG it
        // would be equally targetable once the value exists, so the call itself is the line.
        for banned in [
            "GetLobbyOwner",
            "GetLobbyMemberByIndex",
            "GetNumLobbyMembers",
            "GetLobbyByIndex",
        ] {
            assert!(
                !code.contains(&format!("{banned}(")),
                "{banned}: choosing a lobby by who is in it targets a person who never opted in -- \
                 filtering may decline, it may never select a person"
            );
        }
        // And no allowlist of accounts, however it is spelled.
        for banned in [
            "steam_id_allow",
            "steamid_allow",
            "target_steam_id",
            "friend_steam_id",
        ] {
            assert!(
                !code.contains(banned),
                "{banned}: an account allowlist is person-targeting with extra steps"
            );
        }
    }

    /// The watched window must actually cover the fields the static read identified, or a VM write
    /// to one of them is invisible and the whole point of the tracing is lost.
    #[test]
    fn the_session_watch_window_covers_every_known_field() {
        let begin = SESSION_WATCH_BEGIN;
        let end = SESSION_WATCH_BEGIN + SESSION_WATCH_WORDS * 8;
        for (name, offset) in [
            ("state", ersc::SESSION_STATE_OFFSET),
            ("lobby id", 0x178),
            ("lobby owner", 0x180),
            ("offer block start", 0x190),
            ("seek flag", 0x1d4),
            ("seek latch", 0x1f0),
            ("offer block end", 0x220),
            ("lobby-key flag byte", 0x229),
        ] {
            assert!(
                offset >= begin && offset < end,
                "{name} at {offset:#x} is outside the watched window {begin:#x}..{end:#x}"
            );
        }
    }

    /// The window is read every frame, so it must stay small enough to be free.
    #[test]
    fn the_session_watch_window_stays_small() {
        const {
            assert!(
                SESSION_WATCH_WORDS * 8 <= 0x200,
                "a per-frame read of this size is no longer negligible"
            )
        };
    }

    /// The digest is SHA-256 hex. A probe that expected the 16-character `%016llX` intermediate
    /// would print a truncation of the wrong string and answer "did the key change" with noise --
    /// which is the one question this probe exists to answer.
    #[test]
    fn the_lobby_key_is_a_sha256_hex_digest_not_the_sixteen_char_intermediate() {
        assert_eq!(ersc::LOBBY_KEY_HEX_LEN, 64);
        assert_ne!(ersc::LOBBY_KEY_HEX_LEN, 16);
    }

    /// The prologue is what proves the module is the build these RVAs describe. A wrong-length or
    /// drifted constant would either fail to match (inert, fine) or match the wrong bytes (not).
    #[test]
    fn the_lobby_key_builder_prologue_is_the_measured_msvc_frame() {
        // push rbp; push r15..r12; push rsi; push rdi; push rbx; sub rsp, 0x148
        assert_eq!(
            ersc::BUILD_LOBBY_KEY_PROLOGUE,
            &[
                0x55, 0x41, 0x57, 0x41, 0x56, 0x41, 0x55, 0x41, 0x54, 0x56, 0x57, 0x53, 0x48, 0x81,
                0xec, 0x48, 0x01, 0x00, 0x00
            ]
        );
        assert_eq!(ersc::BUILD_LOBBY_KEY_RVA, 0xa_bc20);
        // Long enough that it cannot collide with the other two ersc prologues this module knows.
        assert!(ersc::BUILD_LOBBY_KEY_PROLOGUE.len() > ersc::SHOW_PROLOGUE.len());
        assert_ne!(ersc::BUILD_LOBBY_KEY_RVA, ersc::SHOW_RVA);
        assert_ne!(ersc::BUILD_LOBBY_KEY_RVA, ersc::INVADE_ACTION_RVA);
    }

    #[test]
    fn capturing_the_menu_object_is_not_gated_on_the_seamless_tag() {
        // Regression, 2026-08-05. `show_observer` used to store OSM only if `+0x68` held the ASCII
        // `seamless` tag. The tag had been measured ONCE in one live frida session; as a
        // precondition it never matched, OSM was never stored, and the feature failed exactly
        // where it was supposed to work -- the live log read `REJECT ...` immediately followed by
        // `cannot cancel -- session is not resolvable`. A single observation is evidence, not a
        // gate. The tag is now a diagnostic string and nothing branches on it.
        let source = include_str!("local_invasion_filter.rs");
        let observer = source
            .split_once("fn show_observer(")
            .expect("show_observer exists")
            .1
            .split_once("\n}")
            .expect("observer body")
            .0;
        assert!(
            observer.contains("OSM.swap(a,"),
            "the observer must store the pointer it was handed"
        );
        assert!(
            !observer.contains("if a != 0 && osm_tag_matches"),
            "storing OSM must not be conditional on the tag"
        );
        // And the resolver must validate the shape it actually depends on instead.
        let resolver = source
            .split_once("fn resolve_session(")
            .expect("resolve_session exists")
            .1
            .split_once("\n}")
            .expect("resolver body")
            .0;
        assert!(
            !resolver.contains("osm_tag_matches"),
            "no tag gate in the resolver either"
        );
        assert!(
            resolver.contains("read_session_state"),
            "validate the state field instead"
        );
    }

    #[test]
    fn self_recovery_cannot_resume_a_search_after_a_kept_match() {
        // THE HAZARD, and why this is a test rather than a comment. Restarting whenever the session
        // is idle is what makes the loop survive a Seamless stall -- but a SUCCESSFUL invasion
        // unwinds through exactly the same states as a failed one. Measured 2026-08-06: `KEEP` was
        // followed by `0x15 -> 0x22 -> 0x23 -> 0x00 IDLE`, identical to a rejection. So idle alone
        // cannot tell "the attempt died" from "you are standing in their world", and a restart on
        // the latter would yank the player back out into a search they never asked for.
        //
        // What separates them is that `Verdict::Keep` DISARMS the loop. That makes the disarm check
        // load-bearing rather than incidental, which is precisely the kind of thing a later tidy-up
        // reorders without noticing.
        let source = include_str!("local_invasion_filter.rs");
        let recovery = source
            .split_once("fn arm_self_recovery(")
            .expect("self-recovery exists")
            .1
            .split_once("\n}")
            .expect("self-recovery body")
            .0;
        assert!(
            recovery.contains("AUTO_SEARCH_ARMED"),
            "self-recovery must consult the armed flag -- it is the ONLY thing distinguishing a \
             dead attempt from a successful invasion, both of which sit at idle"
        );
        let armed_at = recovery.find("AUTO_SEARCH_ARMED").expect("checked above");
        let invade_at = recovery.find("PENDING_REINVADE.store(true");
        assert!(
            invade_at.is_none_or(|at| armed_at < at),
            "the armed check must come BEFORE arming a restart, or a kept match restarts once \
             before the guard is consulted"
        );
        // And the disarm on Keep is the other half of the same invariant.
        let keep = source
            .split_once("Verdict::Keep(reason) => {")
            .expect("keep arm exists")
            .1;
        assert!(
            keep[..keep.find("Verdict::Reject").unwrap_or(keep.len())]
                .contains("AUTO_SEARCH_ARMED.store(false"),
            "a kept match must disarm the loop; self-recovery relies on it"
        );
    }

    #[test]
    fn the_stall_detector_never_times_the_searching_state() {
        // SEARCHING means "nobody has matched yet" and is unbounded by nature -- three consecutive
        // live queries on 2026-08-06 returned 0, 0 and 1 lobbies. Timing it would cancel a healthy
        // search in a quiet bracket, which is the single most obvious way to get a stall detector
        // wrong. The rule lives in `stall_watchdog::is_transient`; this pins that the caller does
        // not reintroduce a timer of its own alongside it.
        let source = include_str!("local_invasion_filter.rs");
        let watcher = source
            .split_once("fn watch_for_stall(")
            .expect("stall watcher exists")
            .1
            .split_once("\n}")
            .expect("watcher body")
            .0;
        assert!(
            !watcher.contains("SESSION_STATE_SEARCHING"),
            "the watcher must not special-case SEARCHING -- the detector decides what is timed, \
             and it deliberately never times a state that is unbounded by nature"
        );
        assert!(
            watcher.contains("observe("),
            "the watcher feeds the detector rather than deciding staleness itself"
        );
        // THE REGRESSION THIS PINS, 2026-08-06: the watchdog cancelled a match it had just KEPT,
        // five seconds after accepting it, because the session dwells in 0x15 while loading into
        // the host's world. No choice of timed states fixes that -- a successful join walks 0x22
        // and 0x23 exactly like a cancel does -- so the arming gate is the whole defence.
        let armed_at = watcher
            .find("AUTO_SEARCH_ARMED")
            .expect("the watcher must only run while the hunt is armed");
        let observe_at = watcher.find("observe(").expect("checked above");
        assert!(
            armed_at < observe_at,
            "the armed check must come BEFORE any observation, or a kept match is timed as a \
             stalled handshake and the invasion is cancelled out from under the player"
        );
    }

    #[test]
    fn the_auto_search_arms_on_the_invade_transition_not_on_merely_being_busy() {
        // Regression, 2026-08-05, caught live. Arming used to be "state != IDLE, so a search must
        // be running". That made standing down when the menu opened useless: you open the menu
        // DURING a search, the loop stands down, and one frame later the session is still non-idle
        // so it re-arms. The log read `stood down` and then `0x11 -> 0x0d` with another automatic
        // restart immediately after.
        //
        // Arming now keys on the transition into SEARCHING, which is sound because the static scan
        // found `S+0x110 = 0x0d` written at exactly ONE site in the whole unpacked .text.
        let source = include_str!("local_invasion_filter.rs");
        // Assembled, not written out: a test that scans its own file finds its own assertion text.
        // That has now bitten twice in this module, so every needle here is built at runtime.
        assert!(
            !source.contains(&format!("fn observe_{}", "user_search")),
            "the not-idle heuristic must stay gone, not sit alongside the replacement"
        );
        let tracer = source
            .split_once("fn trace_session_state(")
            .expect("tracer exists")
            .1
            .split_once("\n}")
            .expect("tracer body")
            .0;
        assert!(
            tracer.contains("SESSION_STATE_SEARCHING") && tracer.contains("AUTO_SEARCH_ARMED"),
            "arming belongs on the SEARCHING transition"
        );
        assert!(
            !tracer.contains("!= ersc::SESSION_STATE_IDLE"),
            "not-idle must never again stand in for a search having been started"
        );
        // Our own restart writes the same value the option does, so it has to be claimed first or
        // the tracer credits the user for it.
        assert!(source.contains("fn note_state_after_our_action"));
        let reinvade = source
            .split_once("fn drive_pending_reinvade(")
            .expect("re-invade exists")
            .1
            .split_once("\n}")
            .expect("re-invade body")
            .0;
        assert!(
            reinvade.contains("note_state_after_our_action"),
            "an unclaimed restart is indistinguishable from the user pressing Invade world"
        );
    }

    #[test]
    fn starting_or_ending_an_invasion_attempt_invalidates_the_pin_cache() {
        // The coupling that makes the dim appear on a map that is ALREADY OPEN. `restyle_live_pins`
        // early-returns when this signature is unchanged, so without the attempt state mixed in the
        // pins would keep their idle frames for the whole search and only dim on the next map open
        // -- i.e. never, for the player who noticed by trying to click one.
        //
        // Restores the latch afterwards: it is process-global and every other test in this binary
        // reads it through the same accessor.
        let restore = er_invasion_warp::warp::invasion_attempt_in_flight();
        er_invasion_warp::warp::set_invasion_attempt_in_flight(false);
        let idle = pin_choice_signature();
        er_invasion_warp::warp::set_invasion_attempt_in_flight(true);
        let searching = pin_choice_signature();
        er_invasion_warp::warp::set_invasion_attempt_in_flight(false);
        let idle_again = pin_choice_signature();
        er_invasion_warp::warp::set_invasion_attempt_in_flight(restore);
        assert_ne!(
            idle, searching,
            "an attempt starting must invalidate the pin cache or the live map never dims"
        );
        assert_eq!(
            idle, idle_again,
            "and it must go back, or the map would never un-dim"
        );
    }

    #[test]
    fn nothing_writes_one_icon_id_across_every_param_row() {
        // Regression, 2026-08-05, caught live. The map's param rows are built with a per-location
        // icon and were then re-stamped with a SINGLE id over every row, so the tiers were
        // computed correctly (`chosen=3` then `chosen=96` in the log as marks were added) and then
        // flattened before anything rendered. Every write of the icon field must come from the
        // per-appearance helper.
        let source = include_str!("../src/map_hooks.rs");
        assert!(
            source.contains("invasion_pin_icon_id_for("),
            "the map must choose icons per location"
        );
        // Anchored on a string unique to the re-stamp. `PARAM_ICON_ID_OFFSET;` is not: an earlier
        // `use` of the same constant matched first and put the window over unrelated code.
        let restamp = source
            .split_once("let stamp_signature")
            .expect("re-stamp block")
            .1;
        let restamp = &restamp[..restamp.len().min(2_000)];
        assert!(
            !restamp.contains(&format!("invasion_pin_icon_id{}", "(")),
            "the re-stamp must not use the single-icon helper -- that is what flattened the tiers"
        );
        assert!(
            restamp.contains("for (index, row) in rows.iter_mut().enumerate()"),
            "the re-stamp has to be per row, with the index that identifies the location"
        );
    }

    #[test]
    fn the_recurring_build_fingerprint_never_reads_a_function_this_module_hooks() {
        // Regression, 2026-08-05. `resolve_session` fingerprinted ersc.dll by comparing `show`'s
        // opening bytes -- and this module HOOKS `show`. MinHook overwrote those bytes with its
        // jump, so from the first install onward the check compared Seamless against our own
        // detour, failed, and reported `ErscUnrecognised`. A live invasion was judged, rejected,
        // and then NOT cancelled because of it. Whatever the recurring check reads must be
        // something nothing patches.
        let source = include_str!("local_invasion_filter.rs");
        let resolver = source
            .split_once("fn resolve_session(")
            .expect("resolve_session exists")
            .1
            .split_once("\n}")
            .expect("resolver body")
            .0;
        assert!(
            !resolver.contains("SHOW_PROLOGUE"),
            "the recurring fingerprint must not read `show` -- it is hooked, so its bytes are ours"
        );
        assert!(
            resolver.contains("INVADE_PROLOGUE"),
            "fingerprint an entry point that is called but never hooked"
        );
        // And `invade` must in fact stay un-hooked, or this fix silently rots. The needle is
        // assembled rather than written out, because a test that scans its own file finds its own
        // assertion text -- which is exactly how this test first failed.
        assert!(!source.contains(&format!("static ORIG_{}", "INVADE_ACTION")));
    }

    #[test]
    fn the_ersc_action_prologues_are_the_bytes_read_out_of_the_shipped_dll() {
        // Read from Seamless Co-op v1.9.9 ersc.dll on 2026-08-05. If these ever need changing,
        // re-read the DLL -- do not adjust them to make a hook install.
        assert_eq!(
            &ersc::INVADE_PROLOGUE[..4],
            &[0xf3, 0x0f, 0x1e, 0xfa],
            "endbr64"
        );
        assert_eq!(
            &ersc::CANCEL_PROLOGUE[..4],
            &[0xf3, 0x0f, 0x1e, 0xfa],
            "endbr64"
        );
        // `show` is the one entry point WITHOUT endbr64, which is what makes it a usable
        // discriminator for "is this the ersc.dll we measured".
        assert_ne!(&ersc::SHOW_PROLOGUE[..4], &[0xf3, 0x0f, 0x1e, 0xfa]);
        assert!(
            ersc::INVADE_PROLOGUE.len() >= 8,
            "long enough to be specific"
        );
    }
}
