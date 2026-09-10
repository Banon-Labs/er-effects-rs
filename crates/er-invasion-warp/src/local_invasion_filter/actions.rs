//! Driving Seamless's own option actions: cancelling a match, starting a search, and getting a
//! stalled attempt moving again.
//!
//! Cut out of `local_invasion_filter` on 2026-09-10, when that file crossed the 3200-line hard
//! limit. The seam is the one the parent's own docs already draw: everything here answers "how does
//! this mod press a button Seamless drew", and nothing here decides which matches deserve it.
//! Judging a destination, identifying the session and telling the player what happened all stay
//! next door, the last of them in [`super::banner`].
//!
//! Every call out of this file is the option callback a player's own click invokes, passed
//! `(owner, 0, 1, 1)` because the callee reads `rcx` and nothing else. So the questions this file
//! exists to answer are all about the owner and the moment: whose `this` is safe to hand Seamless
//! ([`ersc_owner_or_refuse`], [`synthesized_owner`]), whether the session's own lock says the
//! action would survive being called now, and what to do when an attempt stops progressing
//! ([`watch_for_stall`], [`cancel_stalled_attempt`]). Each refusal leaves the match alone, which is
//! the fail-closed direction the parent documents.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::{
    AUTO_SEARCH_ARMED, CANCELS, INVADE_ACTION_UNCALLABLE, INVASION_ACTUALLY_HAPPENED,
    JOIN_IN_FLIGHT, NoSession, OurCall, PENDING_REINVADE, REINVADES, RESTART_BACKOFF, RejectReason,
    SELF_RECOVERIES, STALL_RECOVERIES, STALL_WATCHDOG, SeamlessSession, cancel_row_refusal, ersc,
    ersc_action, inside_ersc_callback, lock_shape_refusal, module_backing, not_identified_detail,
    note_state_after_our_action, now_ms, read_session_state, report_lock_preconditions,
    resolve_ersc_abi, resolve_session, session_guard_refuses, session_scan,
};

/// Refuse to invoke a Seamless action with a null `this`, and say why.
///
/// This is the guard for a crash that actually happened, twice, on 2026-09-04. Every ersc action
/// here is called as `action(session.osm, ..)`, so `osm` lands in RCX as the object the callee
/// dereferences immediately. `scan_for_session` resolves the session without hooking Seamless, but
/// it has no `osm` to hand -- only the `show` detour ever supplied one -- so it returns `osm: 0`,
/// and `cancel(0, 0, 1, 1)` walked straight into a null dereference inside `ersc.dll`.
///
/// The measured chain, from the crash records of two runs with the 19-DLL profile:
///   ersc.dll+0x258da  (cancel + 0xa, `context_rcx=0x0`, 0xc0000005)
///   er_invasion_warp.dll+0x9a6b / +0x9025   <- this filter
///   ersc.dll+0x2820a / +0x636e75 / +0x28a85e
/// followed by 23 x `0xc0000026` STATUS_INVALID_UNWIND_TARGET at `ntdll.dll+0x669a8` -- the unwind
/// out of the fault could not cross our detoured frames, because MinHook registers no unwind info
/// for its trampolines. So the process died with no fatal record and no DllMain detach, leaving a
/// zombie leader with ~128 lingering threads: the "hard kill" signature this investigation kept
/// meeting and could not explain.
///
/// A declined action costs one unfiltered match. A null `this` costs the session.
///
/// Not `cfg`-gated: its callers are not either, and the host build exercises their state machine.
fn ersc_owner_or_refuse(session: &SeamlessSession, what: &str) -> Option<usize> {
    // The identification, not just a guard on the call. `resolve_session` accepts a candidate on
    // `plausible_session_pointer` -- at least 0x10000 and 8-aligned -- plus a state field holding one
    // of four codes, tested across up to 2^18 candidate qwords, and it has already false-positived
    // live once (0x3dfadb, 2026-09-06, see the note on `identifies_a_session`). Four small integers
    // at known offsets is a weak signature over that many candidates.
    //
    // A live `_Mtx_internal_imp_t` at `session+0x100` is a much stronger one, and it is free: the
    // action's own first act is to lock it, so an object that fails this check is an object the
    // action would have thrown on. Measured on run br-20260908-185557-aa9c, where five invasions
    // produced two rejections and both refused here: `_Type` was not a shape MSVC's mutex
    // constructors write, and `osm_tag_matches` reported the `seamless` tag absent on the same
    // owner. That is the pointer being wrong, not the lock being held.
    if let Some((module, offset)) = module_backing(session.session) {
        log_refusal_once(
            &OWNER_REFUSAL_SAID,
            format_args!(
                "local-invasion: refusing to drive ERSC's {what} -- the session resolved to \
                 {:#x}, which is {module}+{offset:#x}, inside a loaded module. A session is \
                 allocated; it is never static image data. Handing this to Seamless is what wedged \
                 the game on 2026-09-08: ersc+0x25871 locked a mutex that was really \
                 eldenring.exe+0x3c0cdc0, nothing unlocks a static global, and the main thread \
                 waited until the 30-second stall watchdog fired.",
                session.session
            ),
        );
        return None;
    }
    if let Some(refusal) = lock_shape_refusal(session) {
        // The trailing sentence is chosen per arm. It used to be one fixed paragraph asserting
        // that identification had failed, appended to all five refusals -- and three of them do
        // not mean that. `lock_shape_refusal` reports a busy lock as well as a wrong shape, and a
        // busy lock is a moment, not a verdict about the pointer. Measured 2026-09-09 on run
        // br-20260909-212201-a69e: the crate drove its own request, hit "another thread holds the
        // session mutex", and the fixed suffix declared the session misidentified -- so the next
        // hour went to the identification instead of to a retry that would have worked.
        let detail = if refusal.contains("holds the session mutex") {
            "That is the lock being BUSY, not the pointer being wrong: the mutex header read back \
             and carries a shape MSVC's constructors write. It is retried on a later tick."
                .to_owned()
        } else {
            format!(
                "That is the pointer being wrong, not the action: the header at \
                 session+{SESSION_MUTEX_OFFSET:#x} is not one a live session carries, so driving \
                 ersc.dll with it would call into an object that is not a session. See open issue \
                 er-effects-rs-9i0g."
            )
        };
        log_refusal_once(
            &OWNER_REFUSAL_SAID,
            format_args!("local-invasion: refusing to drive ERSC's {what} -- {refusal}. {detail}"),
        );
        return None;
    }
    if session.osm != 0 {
        return Some(session.osm);
    }
    // No OSM was found, and one is not needed. See `synthesized_owner`.
    Some(synthesized_owner(session.session))
}

/// A `this` of our own for ERSC's option actions, holding the session where they read it.
///
/// # Why this replaces the search that could not finish
///
/// Both driven actions were read end to end out of the installed `ersc.dll` (v2.0.1,
/// `scripts/disas-ersc.py --whole`), and they are 0x64 and 0x75 bytes:
///
/// ```text
/// cancel  ersc+0x258d0                     invade  ersc+0x25850
///   mov  rdi, [rcx + 0x58]                   mov  rdi, [rcx + 0x58]
///   lea  rsi, [rdi + 0x100]                  cmp  dword [rdi + 0x150], 1
///   ...  everything else via rdi             ...  everything else via rdi
/// ```
///
/// `rcx` is read exactly once, at `+0x58`, and never referenced again -- every later access is
/// `rdi`-relative, and `rdi` is the session. So the object ERSC calls `this` is, to these two
/// functions, nothing but a box with a session pointer at `+0x58`. Whose box it is has no bearing
/// on what they do.
///
/// That dissolves open issue er-effects-rs-9i0g rather than solving it. Finding Seamless's own
/// menu object was never the requirement; it was an assumption, and it cost four false-positive
/// identifications, a wedged main thread, two killed processes, a sweep of the whole address space
/// that turned up fifteen candidates, and a discriminator that then found none of the fifteen was
/// referenced by `ersc.dll` at all. None of that had to happen. The function was 0x64 bytes and
/// reading it settles the question in one look -- which is the argument for reading the binary
/// first, made at our own expense.
///
/// The shim is strictly safer than the object it replaces: it is ours, it lives for the process,
/// its lifetime cannot end under a call, and it cannot be a misidentification. The session pointer
/// written into it is still the one every check upstream has already agreed on -- this changes who
/// holds the session, never which session is held.
///
/// Allocated once and leaked on purpose: ERSC reads it during a call this thread is inside, so it
/// must outlive any borrow, and a process-lifetime allocation of 0x60 bytes is the cheapest way to
/// promise that.
fn synthesized_owner(session: usize) -> usize {
    /// Big enough to contain the one field ERSC reads, and aligned the way MSVC's `operator new`
    /// would align a real one.
    #[repr(C, align(16))]
    struct OwnerShim([u8; 0x60]);

    let shim = SYNTHESIZED_OWNER.load(Ordering::Acquire);
    let shim = if shim == 0 {
        let fresh = Box::into_raw(Box::new(OwnerShim([0u8; 0x60]))) as usize;
        match SYNTHESIZED_OWNER.compare_exchange(0, fresh, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => fresh,
            Err(winner) => {
                // Another thread won. Reclaim ours rather than leaking two.
                drop(unsafe { Box::from_raw(fresh as *mut OwnerShim) });
                winner
            }
        }
    } else {
        shim
    };
    // Rewritten before every use rather than once: the session is a heap object whose address
    // changes between matches, and a stale pointer here would hand ERSC a freed session -- the
    // one way this shim could be more dangerous than the search it replaces.
    // SAFETY: `shim` is our own 0x60-byte allocation, live for the process, and
    // `ersc::NEXT_OBJECT_OFFSET + 8` is 0x60.
    unsafe {
        std::ptr::write_unaligned((shim + ersc::NEXT_OBJECT_OFFSET) as *mut usize, session);
    }
    if !SYNTHESIZED_OWNER_SAID.swap(true, Ordering::SeqCst) {
        crate::standalone_log(format_args!(
            "local-invasion: driving ERSC through a synthesized owner at {shim:#x} carrying the \
             session at +{:#x}. Both actions read `rcx` exactly once, at that offset, and work \
             through the session for everything after -- read end to end out of ersc+0x258d0 \
             (0x64 bytes) and ersc+0x25850 (0x75 bytes). Seamless's own menu object is not \
             required and is no longer looked for (er-effects-rs-9i0g).",
            ersc::NEXT_OBJECT_OFFSET
        ));
    }
    shim
}

/// The process-lifetime shim allocation, or 0 before it is made.
static SYNTHESIZED_OWNER: AtomicUsize = AtomicUsize::new(0);
/// Said once, not per rejected match.
static SYNTHESIZED_OWNER_SAID: AtomicBool = AtomicBool::new(false);

/// One line per run of ticks that declined an armed search for a non-idle session.
static REINVADE_NOT_IDLE_SAID: AtomicBool = AtomicBool::new(false);

/// Latch for the identification refusal above, so a wrong pointer says so once rather than per frame.
static OWNER_REFUSAL_SAID: Mutex<Option<String>> = Mutex::new(None);

/// Where the session's `std::mutex` starts, named here because the refusal message quotes it.
/// Derived from the guard offset rather than written twice: `guard == mutex + _Count`, and `_Count`
/// is at `+0x4c` of an `_Mtx_internal_imp_t`.
const SESSION_MUTEX_OFFSET: usize = 0x100;

/// One line per distinct refusal, not one per frame.
///
/// `drive_pending_reinvade` runs on the game task, so an unchanged refusal was being written every
/// frame -- 22735 identical lines in the run that first exercised this. Keyed on the reason, so a
/// session whose shape changes still gets a fresh line, and the counters below stay honest about
/// how often the refusal actually fired.
pub(super) fn log_refusal_once(latch: &Mutex<Option<String>>, message: std::fmt::Arguments<'_>) {
    let text = format!("{message}");
    let mut guard = match latch.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.as_deref() == Some(text.as_str()) {
        return;
    }
    crate::standalone_log(format_args!("{text}"));
    *guard = Some(text);
}

/// Per-site latches for [`log_refusal_once`]. Separate so a cancel refusal cannot silence an invade
/// one that happens to read the same.
static CANCEL_REFUSAL_SAID: Mutex<Option<String>> = Mutex::new(None);
static INVADE_REFUSAL_SAID: Mutex<Option<String>> = Mutex::new(None);
static STALLED_REFUSAL_SAID: Mutex<Option<String>> = Mutex::new(None);

/// Drive ERSC's own "Cancel search" for a rejected match.
///
/// This calls the exact option callback the user's click calls, with `(OSM, 0, 1, 1)`. The zero is
/// not a guess: the cancel action reads `rcx` and nothing else -- so no
/// captured argument is required and none is invented. Everything past this point -- tearing the
/// match down, returning the session to idle -- is Seamless's own code doing what it always does.
///
/// Returns whether Seamless was actually driven. `false` means the match stands: the caller must
/// not tell the player it was rejected.
pub(super) fn cancel_match(reason: RejectReason) -> bool {
    let session = match resolve_session() {
        Ok(session) => session,
        Err(cause) => {
            // Latched. The patient retry re-arms this every tick while the session is merely not
            // ready yet, and an unlatched line here wrote 431 identical entries in one run.
            log_refusal_once(
                &CANCEL_REFUSAL_SAID,
                format_args!(
                    "local-invasion: cannot cancel ({reason:?}) -- {cause:?}, so the match is LEFT \
                 ALONE and will land wherever the server sent it{}",
                    match cause {
                        NoSession::SessionNotIdentified => not_identified_detail(),
                        _ => "",
                    }
                ),
            );
            return false;
        }
    };
    if let Some(why) = session_guard_refuses(session.abi, session.session) {
        crate::standalone_log(format_args!(
            "local-invasion: cannot cancel ({reason:?}) -- {why}. Leaving the session alone rather \
             than tripping its abort path"
        ));
        return false;
    }
    let Some(cancel) = ersc_action(
        session.abi,
        session.abi.cancel_action_rva,
        session.abi.cancel_prologue,
    ) else {
        return false;
    };
    let Some(owner) = ersc_owner_or_refuse(&session, "cancel") else {
        return false;
    };
    report_lock_preconditions(&session, owner, "cancel");
    if let Some(refusal) = lock_shape_refusal(&session) {
        log_refusal_once(
            &CANCEL_REFUSAL_SAID,
            format_args!(
                "local-invasion: cannot cancel ({reason:?}) -- {refusal}. The match is LEFT ALONE. \
                 The reading this refused on is in the line immediately above."
            ),
        );
        return false;
    }
    // Reported, never obeyed. The set behind this reading is ERSC's hide-PREDICATE at
    // `ersc+0x26b40` -- when Seamless draws its Cancel row -- and the cancel action at
    // `ersc+0x258d0` has no state precondition at all: read end to end it locks the mutex at
    // `session+0x100`, compares `[session+0x14c]` against `0x7fffffff`, and writes `0x23`. Nothing
    // there consults `+0x150`.
    //
    // Treating a UI rule as a safety rule cost the whole feature. Measured 2026-09-08 in one
    // evening: a Frida-driven cancel succeeded eight times from `state_before 22` (`0x16`), every
    // one landing on `state_after 35` (`0x23`), with no crash -- and on the very next run this
    // same reading refused `state 0x16` and the rejected invasion proceeded. The guard that does
    // matter is `session_guard_refuses`, checked above and left in force.
    if let Some(note) = cancel_row_refusal(&session) {
        log_refusal_once(
            &CANCEL_REFUSAL_SAID,
            format_args!(
                "local-invasion: cancelling ({reason:?}) from a state Seamless would not have \
                 drawn its own Cancel row in -- {note}. Driving it anyway: the action itself has \
                 no state precondition, and this state is measured to cancel cleanly."
            ),
        );
    }
    // The one state reading that still refuses, because it is not about the row: an idle session
    // while a join is in flight cannot be the real session -- the player is mid-search, so the
    // real one reads `state_searching`. Keeping a pointer proven wrong means every later rejection
    // refuses on the same reading, which is exactly what run br-20260909-000018-d691 did.
    //
    // Dropping the cache here rather than at the verdict is deliberate: `drive_pending_cancel`
    // re-arms this rejection for `CANCEL_RETRY_TICKS`, so the sweeper gets its seconds while the
    // rejection is still live. Invalidating at the verdict deleted the session at the moment it
    // was needed, which is the mistake this replaces.
    if read_session_state(session.abi, session.session) == Some(session.abi.state_idle)
        && JOIN_IN_FLIGHT.load(Ordering::SeqCst)
    {
        crate::standalone_log(format_args!(
            "local-invasion: dropping the cached session at {:#x} -- it reads idle while a join \
             is in flight, which the real session cannot do. The sweeper looks again; the \
             rejection stays armed meanwhile.",
            session.session
        ));
        // Windows-only: the scan it invalidates does not exist on the host, where these tests run.
        #[cfg(windows)]
        session_scan::invalidate_cached_session();
        return false;
    }
    // Nothing is refused here, and the reading that looked like it should refuse does not.
    //
    // Measured on run br-20260910-012622-fd23: four cancels, each driven immediately after a
    // `join-progress` line, and all four of those lines read `Progressing`. Two settled in ~1.6s
    // and two took ~30.2s, so that verdict does not separate them and a guard on it would have
    // refused every cancel this filter has ever driven. `JOIN_IN_FLIGHT` is worse still: it is set
    // on every match and cleared in exactly one place, `lobby_state::CLIENT`, which is the moment
    // a join lands -- a match this filter rejects never lands, so gating on it refuses the cancel
    // forever rather than for a tick.
    //
    // The 30s is not spent deciding whether to cancel; it is spent inside `0x23` afterwards. The
    // two fast cancels passed through `lobby=7 proto=1 joinCheck=30.0 -> proto=2 joinCheck=29.7`
    // and left 0x23 three tenths of a second into that countdown; the two slow ones never reached
    // `lobby=7` at all and sat out its full 30.0s. `joinCheck`/`waitInit` are f32 seconds, which is
    // why no `30000` immediate was ever found in ersc's `.text`.
    let _call = OurCall::enter();
    unsafe { cancel(owner, 0, 1, 1) };
    drop(_call);
    note_state_after_our_action(session, "cancel");
    let fired = CANCELS.fetch_add(1, Ordering::SeqCst) + 1;
    // Search again once the session settles. Armed here, fired from the tick -- ERSC's own tick
    // does not run while the session is idle, which is why the frida attempt to re-invade from
    // inside an ERSC callback never fired.
    //
    // The arm is unconditional, and that is the fix for the complaint that a rejection ends the
    // hunt: cancelling a match this filter rejected is the hunt, whoever started the search.
    //
    // It used to require `AUTO_SEARCH_ARMED` to already be true, and that flag is set in exactly
    // one other place -- the state tracker, on sampling the `0x01 -> 0x0e` edge. That edge took
    // 38ms on run br-20260909-233549-72f5, which is inside one tick at 40fps, so it is missable;
    // and a search Seamless started from its own menu never produces it for us at all. Measured on
    // run br-20260910-011752-2910: `REJECT 0x0b000000 (WrongBlock)` followed immediately by
    // `cancelled rejected match (#1) -- ... auto re-search is disarmed, so this stops here`. The
    // player then watched the invasion item time out and had to use it again, which is not a delay
    // before the next search -- it is no next search at all.
    //
    // Standing the loop down stays possible and stays the player's call: opening Seamless's own
    // menu clears the flag (`show_observer`), and that is a deliberate act, unlike a sampling miss.
    AUTO_SEARCH_ARMED.store(true, Ordering::SeqCst);
    PENDING_REINVADE.store(true, Ordering::SeqCst);
    crate::standalone_log(format_args!(
        "local-invasion: cancelled rejected match (#{fired}) -- session returns to idle and the \
         search restarts automatically. Press Cancel search yourself, or open Seamless's own menu, \
         to stand the loop down."
    ));
    true
}

/// Arm a search for the next game tick, from outside this DLL.
///
/// Backs `er_invasion_warp_request_invade`; see that export for why the caller may not simply call
/// the invade action itself. Both flags are needed: `PENDING_REINVADE` is what
/// `drive_pending_reinvade` drains, and `AUTO_SEARCH_ARMED` is what it checks first -- setting
/// only one arms a request that is refused on the same tick it is made.
#[cfg(windows)]
pub fn request_invade() -> bool {
    // The precondition is a session, not the hooked menu object. This used to demand `OSM != 0`,
    // which is only ever set by a detour on `ersc.dll` -- and every such detour is disabled here
    // because installing one faults the game at `eldenring.exe+0x10043`. So the gate could never
    // open under the shipping configuration, and the export reported "nothing to drive" while the
    // drive path itself was perfectly able to run: `ersc_owner_or_refuse` synthesizes a `this` of
    // its own when no menu object was captured (see `synthesized_owner` -- both actions read `rcx`
    // exactly once, at `+0x58`, so a box of ours serves), and needs only a session to put in it.
    //
    // Asking `resolve_session` is therefore both correct and stricter in the way that matters: it
    // re-validates the pointer on every call rather than trusting a cached one.
    if let Err(cause) = resolve_session() {
        crate::standalone_log(format_args!(
            "local-invasion: a search was requested from outside this DLL, but no session is \
             resolvable right now ({cause:?}) -- nothing to drive."
        ));
        return false;
    }
    AUTO_SEARCH_ARMED.store(true, Ordering::SeqCst);
    PENDING_REINVADE.store(true, Ordering::SeqCst);
    crate::standalone_log(format_args!(
        "local-invasion: a search was requested from outside this DLL -- armed for the next game \
         tick, where the invade action runs on the thread that owns it."
    ));
    true
}

/// Start the search now, on the calling thread, instead of arming it for the game tick.
///
/// # Why the inline form has to exist
///
/// [`request_invade`] hands the call to `drive_pending_reinvade` on the game task tick, and that
/// hand-off is a deadlock when the caller is a detour that Seamless is already inside. Measured on
/// run `br-20260909-234803-535c`: the player used the item a second time, so the game's own goods
/// path was in `ersc.dll` holding the session mutex, our tick called the invade action for the
/// armed request on another thread, and the log stops dead after `about to drive ERSC invade` with
/// the process alive at 122 threads and `cpu_ticks_in_3000ms=9`.
///
/// The Frida prototype never hit that because it called the action from the replacement itself --
/// the thread already inside Seamless, so there was only ever one entrant. This is that shape.
///
/// Only call it from a frame Seamless drove into. From a game-task tick, use [`request_invade`].
#[cfg(windows)]
pub fn drive_invade_inline(why: &str) -> bool {
    let Ok(session) = resolve_session() else {
        return false;
    };
    if read_session_state(session.abi, session.session) != Some(session.abi.state_idle) {
        return false;
    }
    if session_guard_refuses(session.abi, session.session).is_some() {
        return false;
    }
    let Some(invade) = ersc_action(
        session.abi,
        session.abi.invade_action_rva,
        session.abi.invade_prologue,
    ) else {
        return false;
    };
    let Some(owner) = ersc_owner_or_refuse(&session, why) else {
        return false;
    };
    if let Some(refusal) = lock_shape_refusal(&session) {
        crate::standalone_log(format_args!(
            "local-invasion: declining to start the search inline ({why}) -- {refusal}"
        ));
        return false;
    }
    let _call = OurCall::enter();
    // SAFETY: byte-verified action address for the recognised build, called with the owner
    // `ersc_owner_or_refuse` validated, on the thread Seamless is already on.
    unsafe { invade(owner, 0, 1, 1) };
    drop(_call);
    // Same reason as `drive_invade_with_owner`: a search this DLL drove is one it watched
    // begin, so it owns the loop rather than hoping the tracker samples the edge.
    AUTO_SEARCH_ARMED.store(true, Ordering::SeqCst);
    note_state_after_our_action(session, why);
    crate::standalone_log(format_args!(
        "local-invasion: started the search inline ({why}) on the calling thread -- one entrant,          so it cannot contend with the game's own goods path the way an armed request did"
    ));
    true
}

/// [`drive_invade_inline`], but driving Seamless through an owner the caller has -- the real
/// option-menu object -- instead of one `ersc_owner_or_refuse` synthesizes.
///
/// # Why the synthesized owner is not good enough here
///
/// It was measured wedging the game. Run `br-20260910-000334-453e`, from the log in order:
/// `REFUSED an option-menu object handed in at 0x1` (the register capture came back as `1`, so
/// nothing was adopted), then `driving ERSC through a synthesized owner at 0x54fc3a70`, then the
/// process sitting at 122 threads and `cpu_ticks_in_3000ms=8` -- blocked on a lock.
///
/// The Frida prototype passed the object `show` was called with and never wedged. A synthesized
/// box satisfies `rcx+0x58` and nothing else, which is enough for the reads the action makes on
/// the way in and not enough for whatever it does after.
///
/// The owner is still validated, not trusted: `+0x58` must lead to an object whose state reads a
/// code this build defines, the session must be idle, and the lock shape must be sane. A caller
/// handing in a wrong pointer gets `false`, not a wedge.
#[cfg(windows)]
pub fn drive_invade_with_owner(owner: usize, why: &str) -> bool {
    if owner == 0 {
        return false;
    }
    let Some(abi) = resolve_ersc_abi() else {
        return false;
    };
    // The one hop the actions make. A pointer that does not lead to a session here is not the
    // menu object, whatever else it may be.
    let Some(session) =
        (unsafe { er_game_base::mem::safe_read_usize(owner + ersc::NEXT_OBJECT_OFFSET) })
            .filter(|session| *session != 0)
    else {
        return false;
    };
    if read_session_state(abi, session) != Some(abi.state_idle) {
        return false;
    }
    if session_guard_refuses(abi, session).is_some() {
        return false;
    }
    let resolved = SeamlessSession {
        osm: owner,
        session,
        abi,
    };
    if let Some(refusal) = lock_shape_refusal(&resolved) {
        crate::standalone_log(format_args!(
            "local-invasion: declining to drive ERSC through the handed-in owner 0x{owner:x} \
             ({why}) -- {refusal}"
        ));
        return false;
    }
    let Some(invade) = ersc_action(abi, abi.invade_action_rva, abi.invade_prologue) else {
        return false;
    };
    let _call = OurCall::enter();
    // SAFETY: byte-verified action for the recognised build, called with the real menu object the
    // caller observed, on the thread Seamless is already on -- the Frida prototype's exact shape.
    unsafe { invade(owner, 0, 1, 1) };
    drop(_call);
    note_state_after_our_action(resolved, why);
    crate::standalone_log(format_args!(
        "local-invasion: drove ERSC invade through the REAL menu object 0x{owner:x} ({why}), \
         session 0x{session:x} -- no synthesized owner, one entrant, same as the Frida prototype"
    ));
    true
}

/// Host-side stub.
#[cfg(not(windows))]
pub fn drive_invade_with_owner(_owner: usize, _why: &str) -> bool {
    false
}

/// Host-side stub.
#[cfg(not(windows))]
pub fn drive_invade_inline(_why: &str) -> bool {
    false
}

/// Host-side stub.
#[cfg(not(windows))]
pub fn request_invade() -> bool {
    false
}

/// Can the invade action be called at all right now, and say so exactly once per change.
///
/// Wraps [`ersc_action`] so the recovery loop can ask the question without the answer costing a
/// log line per tick. `ersc_action` already reports an unrecognised prologue, but it reports it on
/// every call, which is the other half of what made the spin unreadable: 6061 identical refusal
/// lines interleaved with 6056 identical restart lines.
///
/// Reports the transition in both directions. A latch that only ever says "broken" leaves a
/// recovered session looking dead, which is the same false negative one level up.
fn invade_action_callable(abi: &ersc::Abi) -> bool {
    let callable = {
        // A thread inside an ersc callback is a temporary refusal, not a broken build --
        // `ersc_action` declines there too, and latching on it would stand the hunt down for the
        // rest of the session over one badly-timed tick.
        if inside_ersc_callback() {
            return !INVADE_ACTION_UNCALLABLE.load(Ordering::SeqCst);
        }
        ersc_action(abi, abi.invade_action_rva, abi.invade_prologue).is_some()
    };
    if callable {
        if INVADE_ACTION_UNCALLABLE.swap(false, Ordering::SeqCst) {
            crate::standalone_log(format_args!(
                "local-invasion: the invade action reads as itself again -- the hunt can restart \
                 searches from here"
            ));
        }
        return true;
    }
    if !INVADE_ACTION_UNCALLABLE.swap(true, Ordering::SeqCst) {
        crate::standalone_log(format_args!(
            "local-invasion: NOT restarting the search -- ersc+{:#x} does not hold the bytes this \
             module measured, so the invade action cannot be called. Cancelling still works (it \
             reads a different address), so rejected matches are cancelled and the hunt stops \
             there. This line is printed once per change, not once per tick.",
            abi.invade_action_rva
        ));
    }
    false
}

/// Fire the queued re-invade once the session is genuinely idle.
///
/// Disarms before calling, so a session that fails to leave idle costs one extra invade at most
/// rather than one per frame.
pub(super) fn drive_pending_reinvade(session: SeamlessSession) {
    if !PENDING_REINVADE.load(Ordering::SeqCst) || !AUTO_SEARCH_ARMED.load(Ordering::SeqCst) {
        return;
    }
    // `invade` returns immediately unless the session is idle, so this is the same precondition
    // ERSC itself enforces -- checked here so a no-op call is not counted as a restart.
    let state = read_session_state(session.abi, session.session);
    if state != Some(session.abi.state_idle) {
        // Said once, because the silent version of this return swallowed a whole self-driven
        // attempt: run br-20260909-212533-0b91 armed a search through
        // `er_invasion_warp_request_invade`, logged `armed for the next game tick`, and then
        // printed nothing at all -- the request stayed pending and this line declined it on every
        // tick without a word. A request that is accepted and then never acted on has to say why.
        if !REINVADE_NOT_IDLE_SAID.swap(true, Ordering::SeqCst) {
            crate::standalone_log(format_args!(
                "local-invasion: an armed search is not being driven -- the resolved session reads                  {} and the invade action only runs from {:#04x} idle. The request stays pending                  and is retried; if this is the last word in the log, the resolved pointer is not                  a session that ever goes idle.",
                match state {
                    Some(value) => format!("{value:#04x}"),
                    None => "unreadable".to_owned(),
                },
                session.abi.state_idle
            ));
        }
        return;
    }
    REINVADE_NOT_IDLE_SAID.store(false, Ordering::SeqCst);
    if session_guard_refuses(session.abi, session.session).is_some() {
        PENDING_REINVADE.store(false, Ordering::SeqCst);
        return;
    }
    let Some(invade) = ersc_action(
        session.abi,
        session.abi.invade_action_rva,
        session.abi.invade_prologue,
    ) else {
        PENDING_REINVADE.store(false, Ordering::SeqCst);
        return;
    };
    PENDING_REINVADE.store(false, Ordering::SeqCst);
    let Some(owner) = ersc_owner_or_refuse(&session, "invade") else {
        return;
    };
    report_lock_preconditions(&session, owner, "invade");
    if let Some(refusal) = lock_shape_refusal(&session) {
        log_refusal_once(
            &INVADE_REFUSAL_SAID,
            format_args!(
                "local-invasion: not restarting the search -- {refusal}. The reading this refused \
                 on is in the line immediately above."
            ),
        );
        return;
    }
    let _call = OurCall::enter();
    unsafe { invade(owner, 0, 1, 1) };
    drop(_call);
    // Claim the searching state we just caused, before the tracer can read it as the user pressing
    // the option and arm a loop that is already armed.
    note_state_after_our_action(session, "restart search");
    let count = REINVADES.fetch_add(1, Ordering::SeqCst) + 1;
    crate::standalone_log(format_args!(
        "local-invasion: search restarted automatically (#{count}) -- press Cancel search yourself \
         to stop"
    ));
}

/// Re-arm the search when an attempt died without us cancelling it.
///
/// # The gap this closes
///
/// [`drive_pending_reinvade`] only ever fired for a match we rejected, because `PENDING_REINVADE`
/// is set in [`cancel_match`] and nowhere else. Every other way an attempt can end -- a host that
/// vanished, a connection that never completed, a refusal from the far side -- left the session
/// sitting at idle with the loop still armed and nothing to restart it, so the player had to reach
/// for the finger again. Measured 2026-08-06: of ten `0x15 -> 0x22` unwinds in one session, only
/// four were ours; the other six ended the hunt silently.
///
/// The standing instruction is that the loop runs until the player uses the lynchpin again, so
/// "the session went idle on its own while we are still hunting" is a restart, not a stop.
///
/// # Why this cannot resume a search after a successful invasion
///
/// A successful join looks identical in session state -- `KEEP` was followed by the same
/// `0x15 -> 0x22 -> 0x23 -> 0x00` unwind a rejection produces, so idle alone cannot tell them
/// apart. It does not have to: [`Verdict::Keep`] clears `AUTO_SEARCH_ARMED`, so a kept match
/// leaves the loop disarmed and this function returns immediately. The same is true of the
/// player's own cancel and of opening Seamless's menu, both of which disarm.
pub(super) fn arm_self_recovery(session: SeamlessSession) {
    if !AUTO_SEARCH_ARMED.load(Ordering::SeqCst) || PENDING_REINVADE.load(Ordering::SeqCst) {
        return;
    }
    if read_session_state(session.abi, session.session) != Some(session.abi.state_idle) {
        return;
    }
    // An invasion that happened is not an attempt that died.
    //
    // The doc above assumed `Verdict::Keep` would have disarmed the loop first. Measured
    // 2026-08-17, it does not: that session logged zero keeps and eleven rejects (`mode=area` with
    // no named locations rejects everything it judges), so the loop stayed armed through three real
    // invasions -- and this function restarted the hunt while the player was still on the loading
    // screen back to their own world. They arrived home coloured as an invader with a Seamless name
    // popup reading `[Unknown]`, because a fresh invasion was already in flight.
    //
    // So the disarm is taken from what the engine did rather than from what our filter decided.
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
    // Is there an action to restart with at all? Asked here, where the decision to re-arm is made,
    // rather than in `drive_pending_reinvade`, where it arrives too late to prevent the spin: an
    // uncallable action there clears the flag silently, and this function re-arms on the next tick
    // having learned nothing. Probing costs one prologue comparison on a path that already reads
    // session state every tick.
    if !invade_action_callable(session.abi) {
        return;
    }
    // How badly did the last attempt go? Restarting instantly is right when Seamless actually
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
            // Held. Return without arming. The next tick re-enters, finds no recorded start (the
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
    cancel_stalled_attempt_inner(session, state, held_ms, false);
}

/// As above, with the option to drive the action in a state where Seamless hides its own Cancel
/// row.
///
/// `engine_has_no_session` is the only thing that unlocks that, and it is not a preference. The
/// row predicate exists so this mod does not drive an action outside the preconditions its author
/// arranged for a live session -- but when `CSSessionManagerImp` reads `lobbyState == None` with
/// no outstanding RPC and both phase timers clear, `DisconnectCleanup` has already run and there
/// is no live session left to protect. What remains is Seamless holding a match against a session
/// the engine destroyed, which is the state the player cannot leave without spending the item.
///
/// Measured on run br-20260910-022111-750c: five attempts to clear it, every one refused with
/// `the session is in state 0x16, and ERSC's own hide-predicate draws its Cancel row only for
/// [0xe, 0xf, 0x10, 0x12]`, while the player stayed stuck. The guard was right about the row and
/// wrong about the situation.
///
/// The guard-poison and mutex-shape refusals above are not relaxed by this: those are about
/// whether the call is safe to make, not about whether the row is on screen.
/// Host-side stub: there is no ERSC action to drive off the game.
#[cfg(not(windows))]
pub(super) fn cancel_stalled_attempt_inner(
    _session: SeamlessSession,
    _state: u32,
    _held_ms: u64,
    _engine_has_no_session: bool,
) {
}

#[cfg(windows)]
pub(super) fn cancel_stalled_attempt_inner(
    session: SeamlessSession,
    state: u32,
    held_ms: u64,
    engine_has_no_session: bool,
) {
    if session_guard_refuses(session.abi, session.session).is_some() {
        return;
    }
    let Some(cancel) = ersc_action(
        session.abi,
        session.abi.cancel_action_rva,
        session.abi.cancel_prologue,
    ) else {
        return;
    };
    let Some(owner) = ersc_owner_or_refuse(&session, "cancel") else {
        return;
    };
    report_lock_preconditions(&session, owner, "cancel (stalled attempt)");
    if let Some(refusal) = lock_shape_refusal(&session) {
        log_refusal_once(
            &STALLED_REFUSAL_SAID,
            format_args!(
                "local-invasion: not cancelling the stalled attempt -- {refusal}. The reading this \
                 refused on is in the line immediately above."
            ),
        );
        return;
    }
    // The Cancel row is not the only way out, and where it is withdrawn the right answer is the
    // other row rather than overriding the refusal.
    //
    // `OPTIONSELECT_LEAVEWORLD` (`ersc+0x259d0`, menu 3's only row) is hidden by its predicate
    // `ersc+0x26ac0` only at idle, so Seamless draws it in every state where the Cancel row's
    // `ersc+0x26b40` has withdrawn -- `0x16` included, which is the state a stranded player is
    // actually in. Both actions do the same thing: take the session mutex, write `0x23` to
    // `session+0x150`, unlock. So this drives a row the player could have clicked, which is the
    // invariant the refusal exists to protect, instead of driving one they could not.
    let mut action = cancel;
    let mut what = "cancel";
    if let Some(refusal) = cancel_row_refusal(&session) {
        let leave_world = ersc_action(
            session.abi,
            session.abi.leave_world_action_rva,
            session.abi.leave_world_prologue,
        );
        match leave_world {
            // `Leave world` is itself withdrawn at idle, and there is nothing to leave from there.
            Some(leave) if state != session.abi.state_idle => {
                action = leave;
                what = "leave world";
                crate::standalone_log(format_args!(
                    "local-invasion: {refusal} -- driving OPTIONSELECT_LEAVEWORLD instead, which \
                     ERSC does draw in this state and which performs the same 0x23 transition."
                ));
            }
            _ => {
                if !engine_has_no_session {
                    log_refusal_once(
                        &STALLED_REFUSAL_SAID,
                        format_args!(
                            "local-invasion: not cancelling the stalled attempt -- {refusal}"
                        ),
                    );
                    return;
                }
                crate::standalone_log(format_args!(
                    "local-invasion: driving the cancel anyway -- {refusal}, and \
                     OPTIONSELECT_LEAVEWORLD did not resolve either. The engine has already torn \
                     its session down, so there is no live session for that predicate to protect \
                     and leaving it alone strands the player."
                ));
            }
        }
    }
    let _call = OurCall::enter();
    unsafe { action(owner, 0, 1, 1) };
    drop(_call);
    note_state_after_our_action(session, what);
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
pub(super) fn watch_for_stall(session: SeamlessSession) {
    // Only recover while actually hunting. If the loop is not armed there is nothing to recover,
    // and running anyway is how this cancelled a successful invasion five seconds after accepting
    // it (2026-08-06): `Verdict::Keep` fired, the session sat in 0x15 loading the host's world,
    // and the watchdog called that a stalled handshake. From the player's seat the invasion
    // appeared and dismissed itself at once.
    //
    // Note this cannot be fixed by choosing better states to time: a successful join walks 0x22
    // and 0x23 exactly like a cancel does. Whether we are still hunting is the only thing that
    // separates "this handshake is stuck" from "this invasion is under way", and `Verdict::Keep`
    // already clears the armed flag, as do the player's own cancel and opening Seamless's menu.
    if !AUTO_SEARCH_ARMED.load(Ordering::SeqCst) {
        if let Ok(mut guard) = STALL_WATCHDOG.lock() {
            guard.stand_down();
        }
        // The backoff stands down here too, on the same condition and in the same place, rather
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
    let Some(state) = read_session_state(session.abi, session.session) else {
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
