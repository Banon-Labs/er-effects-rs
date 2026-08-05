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
//! `ersc.dll` is RELOCATABLE and has no fixed load address, so every ERSC address is `module base
//! + RVA` resolved at runtime and byte-checked before use. If Seamless is not loaded the filter
//! never arms: without a Seamless session there are no Seamless invasions to filter.
//!
//! # Fail-closed direction
//!
//! Every uncertainty resolves toward NOT cancelling. Config missing or unparseable, OSM not
//! resolvable, ERSC absent, anchor unresolved -- all leave matches alone. The failure this guards
//! against is silently cancelling other players' invasions, which is worse than a filter that
//! quietly does nothing.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use er_invasion_warp::local_invasion::{
    InvasionAnchor, InvasionCandidate, LocalInvasionConfig, LocationChoice, RejectReason, Verdict,
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
                "local-invasion: config loaded enabled={} mode={} named={} ids={} blocks={}",
                outcome.config.enabled,
                outcome.config.mode.as_str(),
                outcome.config.named_locations.len(),
                outcome.config.named_location_text_ids.len(),
                outcome.config.allowed_blocks.len(),
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
    if a != 0 {
        let first = OSM.swap(a, Ordering::SeqCst) == 0;
        if first {
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
    unsafe { core::mem::transmute::<usize, ErscActionFn>(orig)(a, b, c, d) }
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
    crate::standalone_log(format_args!(
        "local-invasion: session state {} -> {:#04x} {}",
        if previous == usize::MAX {
            "(first read)".to_owned()
        } else {
            format!("{previous:#04x} {}", state_name(previous as u32))
        },
        state,
        state_name(state),
    ));
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
    crate::standalone_log(format_args!(
        "local-invasion: session state {} -> {:#04x} {} (driven by us: {what})",
        if previous == usize::MAX {
            "(first read)".to_owned()
        } else {
            format!("{previous:#04x} {}", state_name(previous as u32))
        },
        state,
        state_name(state),
    ));
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
    judge_incoming_match(b);
    let orig = ORIG_SET_JOIN_DATA.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    unsafe { core::mem::transmute::<usize, ErscActionFn>(orig)(a, b, c, d) }
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

/// A hash of everything that can change a pin's tier, for the injection cache's key.
///
/// The map's param rows are built once and shared across views, keyed on the spawn catalog. That
/// key is right for the spawn set and WRONG for the icons, because the icon now depends on the
/// user's lists too -- so without this the rows survive a mark and the map never changes. Mixing
/// this in makes a mark invalidate exactly what a mark affects.
#[must_use]
pub fn pin_choice_signature() -> usize {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut mix = |value: u64| {
        hash ^= value;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    };
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

/// The single `PlaceName` text id for a destination block, or `-1` when unknown.
fn place_name_for_block(block: u32) -> i32 {
    place_names_for_block(block).first().copied().unwrap_or(-1)
}

/// Judge an incoming match and cancel it if the user's rules say so.
///
/// `join_data` is the `ServerPushJoinData*` from `SetMultiplayJoinData`'s second argument.
pub fn judge_incoming_match(join_data: usize) {
    let Some(config) = current_config() else {
        return;
    };
    if !config.enabled {
        return; // switched off: never touch a match
    }

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

    let candidate = InvasionCandidate {
        block: destination,
        place_name: place_name_for_block(destination),
    };
    match config.judge(&anchor, candidate) {
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
        }
        Verdict::Reject(reason) => {
            crate::standalone_log(format_args!(
                "local-invasion: REJECT {destination:#010x} ({reason:?}); anchor {:#010x} mode={}",
                anchor.block,
                config.mode.as_str()
            ));
            cancel_match(reason);
        }
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

/// `VK_INSERT`: mark the location you are standing in.
pub const VK_MARK: i32 = 0x2d;
/// `VK_DELETE`: un-mark it.
pub const VK_UNMARK: i32 = 0x2e;
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
        let mark = Self::edge(VK_MARK, &mut self.mark_was_down);
        let unmark = Self::edge(VK_UNMARK, &mut self.unmark_was_down);
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
            "local-invasion: {} {:#010x}{} -- now {} block(s) and {} name(s) marked{}",
            if adding { "MARKED" } else { "UN-MARKED" },
            anchor.block,
            if by_name { " by name" } else { "" },
            config.allowed_blocks.len(),
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
    install_join_hook();
    install_show_observer();
    if game_has_focus {
        keys.poll();
    } else {
        keys.forget();
    }
    // Everything below is Seamless-side and purely observational until a rejected match has
    // actually armed a re-search, so a run without Seamless loaded costs one failed module lookup.
    let Ok(session) = resolve_session() else {
        return;
    };
    trace_session_state(session.session);
    drive_pending_reinvade(session);
}

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

    #[test]
    fn the_mark_keys_are_insert_and_delete_and_are_distinct_from_the_warp_keys() {
        assert_eq!(VK_MARK, 0x2d, "VK_INSERT");
        assert_eq!(VK_UNMARK, 0x2e, "VK_DELETE");
        // Sharing a key with the warp driver would make the two pollers eat each other's
        // GetAsyncKeyState "pressed since last call" edge.
        for warp_key in [
            crate::drive::VK_WARP_NEAREST,
            crate::drive::VK_WARP_NEXT,
            crate::drive::VK_WARP_OTHER_AREA,
        ] {
            assert_ne!(VK_MARK, warp_key);
            assert_ne!(VK_UNMARK, warp_key);
        }
    }

    #[test]
    fn this_module_installs_exactly_two_detours_and_only_one_is_in_seamless() {
        // The budget, made explicit so growing it is a decision rather than a drift:
        //   ORIG_SET_JOIN_DATA -- the GAME's SetMultiplayJoinData, where matches are judged.
        //   ORIG_SHOW          -- ersc's menu builder, observation only, because OSM has no
        //                         static to read it out of (see NEXT_OBJECT_OFFSET's docs).
        // The two option ACTIONS are deliberately not hooked: they read `rcx` only, so calling
        // them with `(OSM, 0, 1, 1)` needs no captured arguments and therefore no detour.
        let source = include_str!("local_invasion_filter.rs");
        let orig_slots = source.matches("\nstatic ORIG_").count();
        assert_eq!(orig_slots, 2, "detour budget is two trampolines");
        assert!(source.contains("\nstatic ORIG_SET_JOIN_DATA"));
        assert!(source.contains("\nstatic ORIG_SHOW"));
        for banned in ["ORIG_INVADE_ACTION", "ORIG_CANCEL_ACTION"] {
            assert!(
                !source.contains(&format!("static {banned}")),
                "{banned}: the option actions must stay un-hooked -- they ignore every argument \
                 past rcx, so there is nothing to capture"
            );
        }
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
