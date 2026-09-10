//! Finding Seamless's session object without detouring anything inside `ersc.dll`.
//!
//! MinHook's five-byte patch into `ersc.dll` faults at `0x140010043` with no input given --
//! `show` (ersc+0x241a0) at ~50s and again at 29.5s on run `br-20260909-234803-535c`, the
//! lobby-key builder (ersc+0xad6e0) at 30.6s. That is a fact about writing bytes into a
//! Themida-protected module, not about observing one: Frida's `Interceptor` sits on that same
//! `ersc+0x241a0` for a whole session and hands over the menu object every time. So a detour-free
//! route is preferred here, not mandatory, and this module is one: a walk of `ersc.dll`'s own
//! writable sections, testing each qword against a signature strong enough to identify the
//! object.
//!
//! # Every narrowing here was bought by a live failure
//!
//! The signature started as "a small integer at `session+0x150`" and has been narrowed four times,
//! each time by a run it cost:
//!
//! | accepted | what it really was | what it cost |
//! |---|---|---|
//! | zeroed memory | a zeroed global | `state 0x00` forever, so the filter read an invasion as permanently in flight and refused every map warp for the run |
//! | `0x61`, `0x62`, `0x63` | a UTF-16 text buffer | the "session state" tracked the text as it changed |
//! | `0x3dfadb` | a 3-byte-misaligned window into a table of Wine pointers | four rejections, zero cancellations, `owner 0x0` for the run |
//! | `0x143c0cdc0` | `eldenring.exe+0x3c0cdc0`, the game's own `.data` | ersc+0x25871 locked a static global nothing unlocks; the main thread waited until the 30s stall watchdog fired |
//! | `0x860f90f8` | an allocation with no mutex at `+0x100` | one rejection judged, not cancelled, and the invasion proceeded |
//!
//! The last of those is why [`super::identifies_a_session`] now asks
//! [`super::lock_report::mutex_shape_identifies_a_session`]. The discriminator that caught it
//! already existed -- it just ran when an action was about to be driven, long after
//! [`cached_scan_for_session`] had latched the pointer and started revalidating it with the same
//! predicate that accepted it.
//!
//! It sits in its own file rather than in the parent because the parent is at the hard limit
//! `scripts/check-rust-file-sizes.py` enforces, which is the same reason `lock_report` moved.
//!
//! This reads only; it writes nothing into Seamless and patches no bytes.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::{
    addressable_session_pointer, ersc, identifies_a_session, osm_tag_matches,
    plausible_session_pointer,
};

/// A 32-bit read built from the two 16-bit reads `er-game-base` actually exposes.
#[cfg(windows)]
fn read_u32(addr: usize) -> Option<u32> {
    let low = unsafe { er_game_base::mem::safe_read_u16(addr) }? as u32;
    let high = unsafe { er_game_base::mem::safe_read_u16(addr + 2) }? as u32;
    Some(low | (high << 16))
}

/// PE constants for walking `ersc.dll`'s own section table at runtime.
const PE_LFANEW: usize = 0x3c;
const PE_NUMBER_OF_SECTIONS: usize = 6;
const PE_SIZE_OF_OPTIONAL_HEADER: usize = 20;
const PE_OPTIONAL_HEADER: usize = 24;
const SECTION_HEADER_SIZE: usize = 40;
const SECTION_VIRTUAL_SIZE: usize = 8;
const SECTION_VIRTUAL_ADDRESS: usize = 12;
const SECTION_CHARACTERISTICS: usize = 36;
const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;
/// Stop after this many candidate qwords, so a malformed header cannot turn this into a hang.
///
/// Sized against the module it has to cross rather than to a round number. Seamless v2.0.1's
/// writable sections total `0xafa0a5` bytes -- 10.98 MB, of which the `ERSC` section alone is
/// 10.94 MB -- which is 1,438,740 qwords. The previous `1 << 18` was 262,144 of them: 18.2% of the
/// image, and it said nothing when it ran out, so a scan that stopped 8.9 MB early was reported as
/// a scan that found nothing. An owner-bearing global past that cut could not be reached at all,
/// which is the standing cause of `owner 0x0` in open issue er-effects-rs-9i0g.
///
/// `1 << 22` is 4.2 million qwords, near three times what this build needs, so the next Seamless
/// can grow without silently truncating the scan again -- and it is still a bound, which is what
/// this constant is for.
pub(super) const SESSION_SCAN_QWORD_BUDGET: usize = 1 << 22;

/// How much is read per `ReadProcessMemory` call.
///
/// The scan used to issue one guarded read per candidate qword. Every one of those is a kernel
/// call -- `safe_read_usize` goes through `ReadProcessMemory` precisely so a bad address cannot
/// fault -- so covering the writable sections word by word is 1.44 million of them, and the
/// per-frame variant of this loop already put the game's main thread at 100% of a core with all
/// 108 others idle (reported live, 2026-09-04). A page at a time is 2,812 calls for the same
/// ground, and a page is the granularity at which the answer can differ anyway: mapped or not is a
/// property of the page, so a chunk larger than one would throw away readable memory whenever it
/// straddled an unmapped neighbour.
const SESSION_SCAN_CHUNK_BYTES: usize = 0x1000;

/// Find Seamless's session object without detouring anything in `ersc.dll`.
///
/// Why this exists. Writing MinHook's five bytes into `ersc.dll` faults at `0x140010043` with no
/// input given -- `show` (ersc+0x241a0) at ~50s, the lobby-key builder (ersc+0xad6e0) at 30.6s.
/// Reading the module is fine; patching it is not, which is what a protected module does. Neither
/// function has a direct caller in `.text` either, both being dispatched indirectly, so the
/// pointer cannot simply be read off a call site.
///
/// What is left is that the session identifies itself. `read_session_state` returns `Some` only for
/// a known state code at a known offset of a known build, which is a strong enough signature to
/// recognise the object without being handed it. So walk `ersc.dll`'s own WRITABLE sections -- its
/// globals, where a long-lived object's pointer will be parked -- and test each qword as a
/// candidate. Two shapes are accepted, matching what `resolve_session` does with `OSM`: the pointer
/// is the session, or the session is one hop away at `+ NEXT_OBJECT_OFFSET`.
///
/// This reads only; it writes nothing into Seamless and patches no bytes.
#[cfg(windows)]
fn scan_for_session(base: usize, abi: &ersc::Abi) -> Option<(usize, usize, usize)> {
    let lfanew = unsafe { er_game_base::mem::safe_read_usize(base + PE_LFANEW) }? & 0xffff_ffff;
    let nt = base + lfanew;
    let sections = unsafe { er_game_base::mem::safe_read_u16(nt + PE_NUMBER_OF_SECTIONS) }?;
    let optional = unsafe { er_game_base::mem::safe_read_u16(nt + PE_SIZE_OF_OPTIONAL_HEADER) }?;
    let table = nt + PE_OPTIONAL_HEADER + optional as usize;
    let mut budget = SESSION_SCAN_QWORD_BUDGET;
    // One allocation for the whole scan, reused per page.
    let mut buffer = vec![0u8; SESSION_SCAN_CHUNK_BYTES];
    // The two answers this scan can produce, kept separately because the weaker one used to be
    // able to veto the stronger one. See the shape-B arm below for what that cost.
    //
    // `bare`: a global that points straight at something identifying itself as a session. One
    // condition, and it names no owner, so it can only ever yield `owner: 0`.
    // `owned`: a global pointing at an object whose `+ NEXT_OBJECT_OFFSET` identifies itself as a
    // session -- the exact relation `resolve_session`'s detour half uses -- so that object is the
    // OSM. Two linked conditions, and it is the only shape that lets the filter act.
    let mut bare: Option<(usize, usize)> = None;
    let mut owned: Option<(usize, usize, usize)> = None;
    for index in 0..sections as usize {
        let header = table + index * SECTION_HEADER_SIZE;
        // Composed from two u16 reads: `er-game-base` exposes `safe_read_u8`, `safe_read_u16` and
        // `safe_read_usize`, and widening its public surface for one header field is not worth it.
        let characteristics = read_u32(header + SECTION_CHARACTERISTICS)?;
        if characteristics & IMAGE_SCN_MEM_WRITE == 0 {
            continue;
        }
        let virtual_size = read_u32(header + SECTION_VIRTUAL_SIZE)?;
        let virtual_address = read_u32(header + SECTION_VIRTUAL_ADDRESS)?;
        let start = base + virtual_address as usize;
        let end = start + virtual_size as usize;
        let mut cursor = start;
        while cursor < end && budget > 0 {
            let span = SESSION_SCAN_CHUNK_BYTES.min(end - cursor);
            let page = &mut buffer[..span];
            // A page that will not read back is skipped whole. Mapped-or-not is a property of the
            // page, so probing it word by word would pay 512 kernel calls to learn the same thing
            // once -- and an unmapped page inside a committed section is ordinary, not a fault.
            if !unsafe { er_game_base::mem::read_bytes(cursor, page) } {
                cursor += span;
                continue;
            }
            for (word_index, word) in page.as_chunks::<8>().0.iter().enumerate() {
                if budget == 0 {
                    break;
                }
                budget -= 1;
                let candidate = usize::from_le_bytes(*word);
                // The free half only. The `VirtualQuery` half runs inside `identifies_a_session`,
                // after the field reads have already rejected almost everything.
                if !addressable_session_pointer(candidate) {
                    continue;
                }
                let slot = cursor + word_index * 8;
                if identifies_a_session(abi, candidate, true) {
                    // The pointer is the session, and this shape names no owner -- but the owner
                    // still exists somewhere, so remember the session and keep looking rather than
                    // returning a `0` owner that disables cancel/invade for the whole run. This is
                    // the shape that actually matched on this machine (session 0x1801b2560 via
                    // slot 0x18021a640), so returning early here is the difference between a
                    // filter that can cancel a rejected match and one that can only complain.
                    if bare.is_none() {
                        bare = Some((slot, candidate));
                    }
                    continue;
                }
                if let Some(next) = unsafe {
                    er_game_base::mem::safe_read_usize(candidate + ersc::NEXT_OBJECT_OFFSET)
                } && next != 0
                    && identifies_a_session(abi, next, true)
                {
                    // This shape hands back the owner too, and throwing it away is what made the
                    // scan path deadly. `resolve_session`'s detour half derives the session as
                    // `*(osm + NEXT_OBJECT_OFFSET)` -- the exact relation just matched here -- so
                    // `candidate` is the OSM. Every ersc action is invoked as
                    // `action(osm, ..)`, and returning `osm: 0` meant `cancel(0, 0, 1, 1)`
                    // dereferenced null inside `ersc.dll` at `+0x258da`, killing the process with
                    // no crash record (the unwind could not cross our MinHook frames).
                    //
                    // The VETO that used to be here is gone. This arm additionally required
                    // `found.map(|(_, session)| session == next).unwrap_or(true)` -- the owner had
                    // to point at the session a previous bare hit had already accepted. As
                    // corroboration that is sound; as a requirement it hands a single wrong bare
                    // hit a veto over every real owner in the image, because a real OSM points at
                    // the real session and the real session is not the wrong one. That is exactly
                    // what happened on 2026-09-06 (see `plausible_session_pointer`): one garbage
                    // qword matched first, and the filter spent the whole run unable to cancel.
                    // Corroboration is now a reason to stop early, never a reason to reject.
                    if !plausible_session_pointer(candidate) {
                        continue;
                    }
                    if osm_tag_matches(candidate)
                        || bare.is_some_and(|(_, session)| session == next)
                    {
                        return Some((slot, next, candidate));
                    }
                    if owned.is_none() {
                        owned = Some((slot, next, candidate));
                    }
                }
            }
            cursor += span;
        }
    }
    if budget == 0 && !SCAN_BUDGET_EXHAUSTED.swap(true, Ordering::SeqCst) {
        crate::standalone_log(format_args!(
            "local-invasion: the session scan ran out of budget after \
             {SESSION_SCAN_QWORD_BUDGET} qwords, so it did NOT cross all of ersc.dll's writable \
             data. A result below is what was found in the part that was reached, and a missing \
             owner may simply be past the cut."
        ));
    }
    // An owner beats a bare session, and a bare session beats nothing at all. The middle case is
    // the filter judging every match and logging while declining to drive Seamless (see
    // `ersc_owner_or_refuse`); only the first case can actually cancel a rejected match.
    owned.or_else(|| bare.map(|(slot, session)| (slot, session, 0)))
}

/// The host build has no `ersc.dll` image to walk, so there is nothing to find.
///
/// `resolve_session` is deliberately not `cfg`-gated -- its state machine is what the host tests
/// exercise -- so the scanner needs a host half or the whole crate fails to build off Windows.
#[cfg(not(windows))]
fn scan_for_session(_base: usize, _abi: &ersc::Abi) -> Option<(usize, usize, usize)> {
    None
}

/// [`scan_for_session`], but at most once per session rather than once per call.
///
/// The scan is not cheap and this caller is hot. `scan_for_session` crosses all 10.98 MB of
/// Seamless's writable sections, and `resolve_session` runs from the filter's tick. It used to
/// return the instant it matched, which hid the cost; the owner search added on 2026-09-04 keeps
/// scanning past the first hit to find the owning object, so every call walks the whole region --
/// and the game's main thread went to 100% of a core in state `R` while all 108 other threads sat
/// idle. Reported live as a hard lock, minutes after the change.
///
/// Two things answer that, and both are needed. A page-at-a-time read makes one sweep 2,812 kernel
/// calls instead of 1.44 million. And the answer is cached both ways: a hit until the session
/// stops identifying itself, a miss for [`SESSION_SCAN_RETRY_CALLS`] calls. The miss half is the
/// newer one and it is what makes the full-coverage budget affordable -- until it existed, only
/// the budget running out 2 MB in kept a sweep-per-frame survivable.
#[cfg(windows)]
pub(super) fn cached_scan_for_session(
    base: usize,
    abi: &'static ersc::Abi,
) -> Option<(usize, usize, usize)> {
    let session = CACHED_SESSION.load(Ordering::SeqCst);
    let owner = CACHED_OWNER.load(Ordering::SeqCst);
    let cached = (session != 0 && identifies_a_session(abi, session, false))
        .then(|| (CACHED_SLOT.load(Ordering::SeqCst), session, owner));
    // An owner is a complete answer. A session with no owner is not: it is the shape that lets the
    // filter judge and log while `ersc_owner_or_refuse` declines every cancel, which is the whole
    // of open issue er-effects-rs-9i0g. So a bare hit stays usable and stays provisional -- it is
    // returned meanwhile, and the sweeper keeps looking for the owner behind it.
    if cached.is_none() || owner == 0 {
        request_sweep(base, abi);
    }
    cached
}

/// Ask the sweeper thread for another pass, and start it if it is not running.
///
/// # Why the game thread does not do this itself
///
/// It did until 2026-09-08, and the cost was measured on run br-20260908-204210-6513: a
/// 3.5-second freeze every 11 seconds -- six in sixty seconds -- during which the game task
/// advanced between one and five ticks. It is visible as gaps in `er-telemetry-timeseries.jsonl`,
/// whose median sample spacing is 218 ms; the frame-delta oracle in the same file saturates at
/// exactly 50.0 ms and so cannot report a stall of this size at all, which is why the gaps between
/// samples are the measurement rather than the samples.
///
/// Reading the pages in bulk fixed the wrong half of the cost. Crossing 10.98 MB is now 2,812
/// calls, but [`identifies_a_session`] still asks the kernel about every candidate -- the state
/// field, then the mutex `_Type`, then `_Count` -- and those pointers land on the heap, outside
/// the page in hand, so they cannot be served from the buffer. Aligned non-zero qwords are common
/// in 1.4 million of them, so one sweep is hundreds of thousands of `ReadProcessMemory` round
/// trips.
///
/// None of that needs the game thread. They are reads of process memory, and another thread does
/// them just as well while the game keeps drawing. The game thread now only validates the cached
/// answer, which is three reads.
/// Forget the cached session and ask for a fresh sweep.
///
/// Called by the liveness check when a candidate proves it cannot be a session. Clearing the
/// session alone is enough -- the game thread's next call sees an empty cache and demands a
/// sweep -- but the budget is refilled here too, because the moment this fires is exactly a moment
/// when the answer is needed.
/// Adopt a session proved by [`super::differential_scan`], overriding whatever the shape scan
/// latched.
///
/// The shape scan answers "this object looks like a session" and has been wrong five times running.
/// The differential scan answers "this object moved out of idle when the player invaded", which is
/// causal rather than descriptive, so its answer wins outright and stops the sweeper spending
/// further passes.
///
/// # It does not stop the sweeper any more
///
/// It used to: `CACHED_OWNER` to 0 and `SWEEP_BUDGET` to 0 on the next two lines, which settled
/// the session and abandoned the owner in the same breath. An owner of 0 is what
/// `ersc_owner_or_refuse` declines every cancel on, so proving the session by change ended with
/// the filter still unable to drive ERSC -- measured on run br-20260910-171939-d923, which
/// adopted `0x868728b8` six times and never once looked for what holds it.
///
/// A proven session makes that search EASIER, not unnecessary. [`owner_among`] asks which of a
/// candidate set some other object holds at `+ NEXT_OBJECT_OFFSET`, and its weakness is a wide
/// set: 13,221 of 13,223 idle-shaped objects had a holder on run br-20260909-211819-87a5, so the
/// test says nothing when asked of everything. Asked of ONE proven address it has no such
/// weakness -- the answer is a holder or nothing.
#[cfg(windows)]
pub(super) fn adopt_proven_session(session: usize) {
    CACHED_SESSION.store(session, Ordering::SeqCst);
    CACHED_OWNER.store(0, Ordering::SeqCst);
    // Hand the search to the sweeper rather than running it here: `adopt_proven_session` is
    // called from the game thread as a join starts, and `owner_among` is a walk of every
    // committed private region.
    PROVEN_SESSION_WANTING_OWNER.store(session, Ordering::SeqCst);
    SWEEP_BUDGET.store(SESSION_SCAN_MAX_SWEEPS, Ordering::SeqCst);
    raise_sweep_request();
}

/// A session proved by change whose holder has not been found yet, or 0.
///
/// Written by [`adopt_proven_session`] on the game thread, read and cleared by the sweeper.
#[cfg(windows)]
static PROVEN_SESSION_WANTING_OWNER: AtomicUsize = AtomicUsize::new(0);

/// Said-once latch for the miss above, so a pending owner costs one line rather than one a pass.
#[cfg(windows)]
static PROVEN_OWNER_MISS_SAID: AtomicBool = AtomicBool::new(false);

/// # The differential snapshot is deliberately not cleared here
///
/// It used to be, on the first line of this function, and that one line is why run
/// br-20260909-194041-6558 could not cancel. The order it produced: the sweeper armed the
/// differential scan with 12,129 idle objects; the shape scan then latched `0xce40038`; the
/// liveness check discarded it, correctly, because its state read `0x1` across four samples in
/// which the engine's join advanced -- and that discard came through here and deleted all 12,129.
/// The player invaded seconds later, `narrow_to_changed` found an empty list and returned `None`
/// without a word, and the rejection was reported `MenuNeverOpened` while the invasion landed in
/// the wrong block.
///
/// The two are independent evidence about the same question, and the shape scan being wrong says
/// nothing about the snapshot -- the snapshot's whole claim is "these objects read idle a moment
/// ago", which no later discovery about a different pointer can falsify. Worse, it cannot be
/// rebuilt on demand: it can only be taken while the session is idle, so a snapshot destroyed
/// during a join is gone for exactly the invasion that needed it. `snapshot_idle_candidates`
/// replaces it wholesale on the next armed pass, and `narrow_to_changed` clears it itself when a
/// join empties it, which is the one event that does prove the real session was never in it.
#[cfg(windows)]
pub(super) fn invalidate_cached_session() {
    CACHED_SESSION.store(0, Ordering::SeqCst);
    CACHED_OWNER.store(0, Ordering::SeqCst);
    CACHED_SLOT.store(0, Ordering::SeqCst);
    SWEEP_BUDGET.store(SESSION_SCAN_MAX_SWEEPS, Ordering::SeqCst);
    // Raised after the budget, never before: the sweeper re-reads the budget the moment it wakes,
    // so waking it first would show it the count it was already waiting out.
    raise_sweep_request();
}

#[cfg(windows)]
pub(super) fn request_sweep_now() {
    // Spend a fresh round of passes. Called when a match is being judged, which is the only
    // moment the answer is actually needed -- see the note on `SWEEP_BUDGET`.
    SWEEP_BUDGET.store(SESSION_SCAN_MAX_SWEEPS, Ordering::SeqCst);
    raise_sweep_request();
}

/// What ends the sweeper's wait between passes.
#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pacing {
    /// A request, and nothing else. Used when the budget owes no passes, so no interval can make
    /// the next look any more likely to succeed than this one was.
    UntilRequested,
    /// A request, or [`SESSION_SCAN_SWEEP_INTERVAL`] -- whichever lands first. The interval is the
    /// pacing that keeps a worker thread off a whole core while passes are still owed.
    AtMostOnePassPerInterval,
}

/// How many sweep requests have been raised in this process.
///
/// A count rather than a flag, so the sweeper can tell a request that arrived while it was
/// scanning from one that has not arrived at all. It only ever has to compare two readings, so
/// wrapping is not a case worth handling.
#[cfg(windows)]
static SWEEP_REQUESTS: (std::sync::Mutex<u64>, std::sync::Condvar) =
    (std::sync::Mutex::new(0), std::sync::Condvar::new());

#[cfg(windows)]
fn sweep_requests_raised() -> u64 {
    *SWEEP_REQUESTS
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Publish a request and wake the sweeper if it is waiting on one.
#[cfg(windows)]
fn raise_sweep_request() {
    let (raised, wake) = &SWEEP_REQUESTS;
    {
        let mut count = raised
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *count = count.wrapping_add(1);
    }
    wake.notify_all();
}

/// Block until a request newer than `seen` is raised, and update `seen` to what was observed.
///
/// This is what the sweeper waits on instead of sleeping. The game thread raises a request at the
/// two moments the answer is actually wanted -- a match being judged, and a cached session being
/// invalidated -- so the wait ends on that event rather than on a clock, and a whole interval is no
/// longer spent holding a rejection back from the session that would enforce it.
#[cfg(windows)]
fn await_sweep_request(seen: &mut u64, pacing: Pacing) {
    let (raised, wake) = &SWEEP_REQUESTS;
    let mut count = raised
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    while *count == *seen {
        match pacing {
            Pacing::UntilRequested => {
                count = wake
                    .wait(count)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            Pacing::AtMostOnePassPerInterval => {
                let (next, outcome) = wake
                    .wait_timeout(count, SESSION_SCAN_SWEEP_INTERVAL)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                count = next;
                if outcome.timed_out() {
                    break;
                }
            }
        }
    }
    *seen = *count;
}

#[cfg(windows)]
fn request_sweep(base: usize, abi: &'static ersc::Abi) {
    if SWEEP_REQUESTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("er-invasion-warp-session-scan".to_owned())
        .spawn(move || sweep_until_answered(base, abi));
    if spawned.is_err() {
        // Left armed rather than retried in a loop: a process that cannot spawn a thread has
        // worse problems, and the filter degrades to "no session" rather than to a stall.
        SWEEP_REQUESTED.store(false, Ordering::SeqCst);
    }
}

/// Find the session by what it is doing, anywhere in the process.
///
/// # Why the module scan cannot answer this
///
/// [`scan_for_session`] looks for a pointer to the session parked in `ersc.dll`'s own writable
/// data. Seven runs say it is not there: six candidates were latched and every one proved fake --
/// `0x3dfadb`, `0x860f90f8`, `0x451200`, `0x45e00cb0`, `0xa2760038`, `0xd2c0038` -- and the
/// seventh run, with idle refused during a join, found nothing at all. That is a measurement, not
/// a gap, and no further tightening of the same search can turn it around.
///
/// So stop looking for a pointer and look for the object. During a join it identifies itself by
/// three facts at once, taken from the actions' own disassembly rather than from inference:
///
/// * `+0x150` holds an active state. The invade action opens `cmp dword [rdi+0x150], 1` / `jne` --
///   it refuses to run unless idle -- and then writes `0xe` there. So from the start of a search
///   until it settles, that field cannot read `state_idle`.
/// * `+0x100` is an `_Mtx_internal_imp_t` carrying `_Mtx_try`, the bit MSVC's `std::mutex`
///   constructor writes.
/// * `_Count` at `+0x14c` is 0 or 1, because the lock is not recursive.
///
/// Together those are a far narrower signature than "a dword that reads 1", which is what the
/// module scan was really asking and why almost anything could answer it.
///
/// # Cost, and why it is affordable here and nowhere else
///
/// This reads committed private memory a chunk at a time and tests candidates inside the buffer --
/// no kernel call per candidate, which is what made the earlier per-qword form a 3.5-second freeze
/// every 11 seconds. It runs only on the sweeper thread, only while a match is in flight, and only
/// after the cheap module scan has failed.
#[cfg(windows)]
fn scan_address_space_for_active_session(abi: &ersc::Abi) -> Option<usize> {
    /// `_Mtx_try`, and `_Count` at `mutex + 0x4c`.
    const MTX_TRY: u32 = 0x02;
    const MTX_RECURSIVE: u32 = 0x100;
    const MTX_COUNT: usize = 0x4c;
    const MUTEX_AT: usize = 0x100;

    let active = [
        abi.state_searching,
        abi.state_offer_received,
        abi.state_cancelling,
        // The three the cancel row is drawn for that the ABI does not name individually; read out
        // of ERSC's own hide-predicate at `ersc+0x26b40`.
        0x0f,
        0x10,
        0x12,
    ];
    let found = walk_private_memory(abi, |address, session_state| {
        if !active.contains(&session_state) {
            return false;
        }
        let Some(kind) = read_u32(address + MUTEX_AT) else {
            return false;
        };
        let Some(count) = read_u32(address + MUTEX_AT + MTX_COUNT) else {
            return false;
        };
        let recursive = (kind & MTX_RECURSIVE) != 0;
        (kind & MTX_TRY) != 0 && (recursive || count <= 1)
    });
    crate::standalone_log(format_args!(
        "local-invasion: active-session scan found {} object(s) with an active state at +{:#x} and \
         a std::mutex at +{MUTEX_AT:#x}{}. A count in the thousands is the shape scan failing, not \
         succeeding -- see `differential_scan`, which asks instead which of them MOVES when the \
         player invades.",
        found.len(),
        abi.session_state_offset,
        match found.first() {
            Some(first) => format!(", first {first:#x}"),
            None => String::new(),
        }
    ));
    // One is an identification. Several is not, and driving the wrong one is what has killed this
    // process twice, so several is reported and refused.
    match found.len() {
        1 => Some(found[0]),
        _ => None,
    }
}

/// Which of `candidates` is owned -- that is, which one some other object holds at its `+0x58`.
///
/// # The discriminator the crate was missing, taken from what Frida actually did
///
/// Both driven ERSC actions read `rcx` exactly once, as `mov rdi, [rcx + 0x58]`, so the object
/// they are called with is a box holding the session at that offset. The session therefore has a
/// holder somewhere in memory, and a look-alike -- a heap qword that merely reads like a session
/// -- generally does not. That is a fact about the object graph, not about its bytes, which is
/// what makes it able to separate candidates that every value check calls identical.
///
/// This is the half `scripts/frida/ersc-handoff.js` implemented as `ownerOf` and the crate did
/// not. Every previous run resolved a session with `owner 0x0` and then declined every action;
/// the sessions Frida drove were the ones its `invade` hook handed over as `rcx`, which is why
/// cancel worked under Frida on 2026-09-09 and has never worked without it.
///
/// One pass over committed private memory, testing every aligned qword against the candidate set,
/// and it returns `(session, owner)` only when exactly one candidate has a holder -- several is
/// not an identification, and this module drives ERSC with the answer.
///
/// Runs on the sweeper thread. A pass is hundreds of thousands of reads and must never touch the
/// game thread; see the note on `walk_private_memory`.
#[cfg(windows)]
pub(super) fn owner_among(candidates: &[usize]) -> Option<(usize, usize)> {
    const MEM_COMMIT: u32 = 0x1000;
    const MEM_PRIVATE: u32 = 0x2_0000;
    const MBI_SIZE: usize = 48;
    const MAX_REGIONS: usize = 1 << 16;
    const MAX_REGION_BYTES: usize = 64 << 20;
    const CHUNK: usize = 64 * 1024;

    unsafe extern "system" {
        fn VirtualQuery(
            address: *const core::ffi::c_void,
            buffer: *mut core::ffi::c_void,
            length: usize,
        ) -> usize;
    }

    if candidates.is_empty() {
        return None;
    }
    let wanted: std::collections::HashSet<usize> = candidates.iter().copied().collect();
    let mut hits: Vec<(usize, usize)> = Vec::new();
    let mut info = [0u8; MBI_SIZE];
    let mut buffer = vec![0u8; CHUNK];
    let mut address: usize = 0x1_0000;
    for _ in 0..MAX_REGIONS {
        let wrote = unsafe {
            VirtualQuery(
                address as *const core::ffi::c_void,
                info.as_mut_ptr().cast(),
                MBI_SIZE,
            )
        };
        if wrote == 0 {
            break;
        }
        let field = |at: usize, width: usize| -> usize {
            let mut value = 0usize;
            for index in 0..width {
                value |= (info[at + index] as usize) << (index * 8);
            }
            value
        };
        let base = field(0x00, 8);
        let size = field(0x18, 8);
        let state = field(0x20, 4) as u32;
        let kind = field(0x28, 4) as u32;
        if size == 0 {
            break;
        }
        if state == MEM_COMMIT && kind == MEM_PRIVATE && size <= MAX_REGION_BYTES {
            let end = base + size;
            let mut cursor = base;
            while cursor < end {
                let span = CHUNK.min(end - cursor);
                let window = &mut buffer[..span];
                if unsafe { er_game_base::mem::read_bytes(cursor, window) } {
                    let mut offset = 0usize;
                    while offset + 8 <= span {
                        let value = usize::from_le_bytes(
                            window[offset..offset + 8].try_into().expect("eight bytes"),
                        );
                        // The holder is the address `0x58` below the qword that carries the
                        // session, and it must itself be a plausible heap object -- otherwise the
                        // hit is a stray copy of the pointer rather than the box ERSC is called
                        // with.
                        if wanted.contains(&value) {
                            let at = cursor + offset;
                            if at >= ersc::NEXT_OBJECT_OFFSET {
                                let owner = at - ersc::NEXT_OBJECT_OFFSET;
                                if unsafe { er_game_base::mem::is_heap_aligned_ptr(owner) } {
                                    hits.push((value, owner));
                                }
                            }
                        }
                        offset += 8;
                    }
                }
                cursor += span.saturating_sub(8).max(8);
            }
        }
        let Some(next) = base.checked_add(size) else {
            break;
        };
        address = next;
    }
    let sessions: std::collections::HashSet<usize> = hits.iter().map(|(s, _)| *s).collect();
    crate::standalone_log(format_args!(
        "local-invasion: owner scan over {} survivor(s) found {} holder(s) naming {} distinct \
         session(s){}",
        candidates.len(),
        hits.len(),
        sessions.len(),
        match hits.first() {
            Some((session, owner)) => format!(", first session {session:#x} owner {owner:#x}"),
            None => String::new(),
        }
    ));
    // One session is an identification. Several holders naming the same session are fine -- a box
    // can be referenced more than once -- so the count that has to be one is the session count.
    if sessions.len() != 1 {
        return None;
    }
    hits.first().copied()
}

/// Read the session state field at `address`, or `None` if it is not readable.
///
/// One dword, so the differential scan can re-test thousands of recorded addresses without paying
/// for the mutex shape a second time -- the shape was already true when the address was recorded,
/// and what is being asked now is whether the value moved.
#[cfg(windows)]
pub(super) fn read_state_at(abi: &ersc::Abi, address: usize) -> Option<u32> {
    read_u32(address + abi.session_state_offset)
}

/// Walk the game's committed private memory and collect every 8-aligned address `keep` accepts.
///
/// Factored out of [`scan_address_space_for_active_session`] rather than written twice: the two
/// callers ask different questions of the same bytes, and a second copy of the region walk is a
/// second place for the chunk overlap to be got wrong. `keep` receives the candidate address and
/// the dword already read from its state field, so the common case costs no extra read.
#[cfg(windows)]
pub(super) fn walk_private_memory(
    abi: &ersc::Abi,
    mut keep: impl FnMut(usize, u32) -> bool,
) -> Vec<usize> {
    const MEM_COMMIT: u32 = 0x1000;
    const MEM_PRIVATE: u32 = 0x2_0000;
    const MBI_SIZE: usize = 48;
    const MBI_BASE: usize = 0x00;
    const MBI_REGION_SIZE: usize = 0x18;
    const MBI_STATE: usize = 0x20;
    const MBI_TYPE: usize = 0x28;
    const MAX_REGIONS: usize = 1 << 16;
    const MAX_REGION_BYTES: usize = 64 << 20;
    const CHUNK: usize = 64 * 1024;
    const OBJECT_SPAN: usize = 0x160;

    unsafe extern "system" {
        fn VirtualQuery(
            address: *const core::ffi::c_void,
            buffer: *mut core::ffi::c_void,
            length: usize,
        ) -> usize;
    }

    let mut info = [0u8; MBI_SIZE];
    let mut buffer = vec![0u8; CHUNK];
    let mut address: usize = 0x1_0000;
    let mut found: Vec<usize> = Vec::new();
    for _ in 0..MAX_REGIONS {
        let wrote = unsafe {
            VirtualQuery(
                address as *const core::ffi::c_void,
                info.as_mut_ptr().cast(),
                MBI_SIZE,
            )
        };
        if wrote == 0 {
            break;
        }
        let field = |at: usize, width: usize| -> usize {
            let mut value = 0usize;
            for index in 0..width {
                value |= (info[at + index] as usize) << (index * 8);
            }
            value
        };
        let base = field(MBI_BASE, 8);
        let size = field(MBI_REGION_SIZE, 8);
        let state = field(MBI_STATE, 4) as u32;
        let kind = field(MBI_TYPE, 4) as u32;
        if size == 0 {
            break;
        }
        if state == MEM_COMMIT && kind == MEM_PRIVATE && size <= MAX_REGION_BYTES {
            let end = base + size;
            let mut cursor = base;
            while cursor < end {
                let span = CHUNK.min(end - cursor);
                let window = &mut buffer[..span];
                if unsafe { er_game_base::mem::read_bytes(cursor, window) } {
                    let mut offset = 0usize;
                    while offset + OBJECT_SPAN <= span {
                        let at = offset + abi.session_state_offset;
                        let session_state =
                            u32::from_le_bytes(window[at..at + 4].try_into().expect("four bytes"));
                        if keep(cursor + offset, session_state) {
                            found.push(cursor + offset);
                        }
                        offset += 8;
                    }
                }
                cursor += span.saturating_sub(OBJECT_SPAN).max(8);
            }
        }
        let Some(next) = base.checked_add(size) else {
            break;
        };
        address = next;
    }
    found
}

/// Sweep until an owner is found or the attempts run out, publishing each answer as it lands.
///
/// Blocks on [`await_sweep_request`] between passes rather than spinning: the thing being looked
/// for appears when Seamless builds it, which is a human-scale event, and a sweep costs seconds of
/// one core.
#[cfg(windows)]
fn sweep_until_answered(base: usize, abi: &'static ersc::Abi) {
    let mut seen = sweep_requests_raised();
    loop {
        // Spend a pass only when one is owed. A fixed run of passes at startup is what broke this
        // on run br-20260908-210312-80d7: all of them were spent in the first two minutes, the
        // session did not exist yet, and by the time a match arrived the sweeper had stopped for
        // good -- `cannot cancel (WrongBlock) -- MenuNeverOpened`, with the session never found.
        // The budget is refilled by `request_sweep_now`, called where a match is judged, because
        // that is the only moment the answer is needed and the only moment it is likely to exist.
        if SWEEP_BUDGET.load(Ordering::SeqCst) == 0 {
            // Nothing to pace: with no passes owed, the only thing that can make another one worth
            // running is a request, so this parks on the request itself rather than waking on a
            // timer to re-read a value nothing has touched.
            await_sweep_request(&mut seen, Pacing::UntilRequested);
            continue;
        }
        // Taken before the pass, so a request that arrives while this one is scanning is still
        // newer than what the wait below is comparing against and ends that wait immediately.
        seen = sweep_requests_raised();
        SWEEP_BUDGET.fetch_sub(1, Ordering::SeqCst);
        // A session proved by change is the narrowest question this thread can be asked, so it
        // is asked first. `owner_among` over one address either names the holder or says nothing;
        // there is no wide-set ambiguity to weigh, and until it answers the filter can judge a
        // rejection but not cancel it.
        let proven = PROVEN_SESSION_WANTING_OWNER.load(Ordering::SeqCst);
        if proven != 0 {
            match owner_among(&[proven]) {
                Some((session, owner)) => {
                    crate::standalone_log(format_args!(
                        "local-invasion: owner {owner:#x} found for the change-proven session \
                         {session:#x} -- it is the object holding it at +{:#x}, which is the \
                         `this` ERSC's own actions take. Cancel can be driven now.",
                        ersc::NEXT_OBJECT_OFFSET
                    ));
                    CACHED_SLOT.store(0, Ordering::SeqCst);
                    CACHED_OWNER.store(owner, Ordering::SeqCst);
                    CACHED_SESSION.store(session, Ordering::SeqCst);
                    PROVEN_SESSION_WANTING_OWNER.store(0, Ordering::SeqCst);
                    SWEEP_BUDGET.store(0, Ordering::SeqCst);
                    return;
                }
                // Nothing holds it yet, or the holder is in a region this pass could not read.
                // The address stays pending: the budget above bounds how many passes it costs,
                // and a session with no holder is the state this whole module exists to leave.
                None => {
                    if !PROVEN_OWNER_MISS_SAID.swap(true, Ordering::SeqCst) {
                        crate::standalone_log(format_args!(
                            "local-invasion: no object in committed private memory holds the \
                             change-proven session {proven:#x} at +{:#x}. Retrying while passes \
                             are owed; until one is found the filter judges but cannot cancel.",
                            ersc::NEXT_OBJECT_OFFSET
                        ));
                    }
                }
            }
        }
        // Record what reads idle right now, so the next invasion can disprove it. This has to
        // happen before the player invades, because the whole test is that the real session moves
        // out of idle at that moment and an impostor does not -- a set snapshotted after the fact
        // has already lost the distinction it exists to make.
        let (held, rounds) = super::differential_scan::progress();
        // A narrowed set is the best question available: ask which survivor is owned before
        // spending this pass on another shape scan. `owner_among` is the discriminator Frida
        // supplied through its `invade` hook and the crate never had, so it goes first once there
        // is a set small enough to cross-reference.
        if rounds == 0 {
            let recorded = super::differential_scan::snapshot_idle_candidates(abi);
            if recorded != held {
                crate::standalone_log(format_args!(
                    "local-invasion: differential scan armed -- {recorded} object(s) currently read \
                     idle with a session-shaped mutex. Invade once and everything that did not move \
                     is eliminated."
                ));
            }
        }
        // No `rounds` gate. It used to require a join first, on the reasoning that the owner scan
        // breaks a tie between survivors -- but ownership is not a tiebreak, it is the
        // identification, and the idle snapshot taken at boot is already a candidate set. So the
        // question is asked of whatever is recorded, which lets it answer before the player has
        // invaded even once. That matters more than the cost of the pass: waiting for a join made
        // the feature depend on an invasion happening first, and the first invasion is exactly the
        // one that lands in the wrong world.
        let recorded_now = super::differential_scan::survivors();
        if recorded_now.len() > OWNER_SCAN_MAX_CANDIDATES {
            if !OWNER_SCAN_TOO_WIDE_SAID.swap(true, Ordering::SeqCst) {
                crate::standalone_log(format_args!(
                    "local-invasion: owner scan declined -- {} candidates is past the                      {OWNER_SCAN_MAX_CANDIDATES} it can answer at. Being pointed at is common:                      measured on run br-20260909-211819-87a5, 13,221 of 13,223 idle-shaped objects                      had a holder, so the test says nothing until a join has narrowed the set.",
                    recorded_now.len()
                ));
            }
        } else if !recorded_now.is_empty()
            && let Some((session, owner)) = owner_among(&recorded_now)
        {
            crate::standalone_log(format_args!(
                "local-invasion: session {session:#x} identified by OWNERSHIP -- it is the one                  differential survivor that another object holds at +{:#x}, and its holder                  {owner:#x} is the `this` ERSC's own actions are called with. Adopting both.",
                ersc::NEXT_OBJECT_OFFSET
            ));
            CACHED_SLOT.store(0, Ordering::SeqCst);
            CACHED_OWNER.store(owner, Ordering::SeqCst);
            CACHED_SESSION.store(session, Ordering::SeqCst);
            SWEEP_BUDGET.store(0, Ordering::SeqCst);
            return;
        }
        // The cheap search first: a pointer parked in Seamless's own data. When that finds
        // nothing -- which seven runs say is the normal case -- ask the harder question instead.
        let module_scan = scan_for_session(base, abi);
        let by_behaviour = if module_scan.is_none() {
            scan_address_space_for_active_session(abi).map(|session| (0, session, 0))
        } else {
            None
        };
        if let Some((slot, session, owner)) = module_scan.or(by_behaviour) {
            CACHED_SLOT.store(slot, Ordering::SeqCst);
            CACHED_OWNER.store(owner, Ordering::SeqCst);
            // Published LAST: it is the field the game thread validates, so nothing may observe a
            // session paired with another sweep's slot or owner.
            CACHED_SESSION.store(session, Ordering::SeqCst);
            if owner != 0 {
                return;
            }
            // A bare session is provisional, so this thread keeps its post rather than retiring
            // on it. Returning here is what made the owner scan above dead code: the sweeper
            // exited on the first thing the shape scan latched -- routinely a look-alike, five
            // times running -- and never woke again, so a set narrowed by a later join had
            // nothing left to cross-reference it. Measured on run br-20260909-202153-5a46: a join
            // took 12,493 candidates to 8 and no owner scan line was ever written, because this
            // thread had been gone since boot.
            //
            // The synthesized owner still stands (`rcx` is read exactly once, at `+0x58`, so a box
            // of our own serves for driving), and it is why a bare session is usable at all. What
            // it cannot do is tell a real session from a look-alike, which is what the owner scan
            // is for -- and that needs this loop alive.
            await_sweep_request(&mut seen, Pacing::AtMostOnePassPerInterval);
            continue;
        }
        await_sweep_request(&mut seen, Pacing::AtMostOnePassPerInterval);
    }
    #[allow(unreachable_code)]
    if !SCAN_SWEEPS_SPENT_SAID.swap(true, Ordering::SeqCst) {
        crate::standalone_log(format_args!(
            "local-invasion: the session scan has run its {SESSION_SCAN_MAX_SWEEPS} sweeps and \
             stops here. It was looking for the OSM -- an object whose +0x58 is the session -- and \
             no run has yet found one anywhere in ersc.dll's writable data, so cancel and invade \
             stay declined (er-effects-rs-9i0g). Whatever session is cached now is final for this \
             process."
        ));
    }
}

/// Host half: there is no ersc image to scan, so there is nothing to cache either.
#[cfg(not(windows))]
pub(super) fn cached_scan_for_session(
    base: usize,
    abi: &ersc::Abi,
) -> Option<(usize, usize, usize)> {
    scan_for_session(base, abi)
}

/// How narrow the candidate set has to be before the owner scan is worth running.
///
/// Ownership identifies the session only among candidates that are already few. The boot-time
/// attempt was measured on run br-20260909-211819-87a5 and refuted outright: 13,223 objects read
/// idle with a session-shaped mutex, and 2,100,436 holders named 13,221 distinct sessions among
/// them -- 99.98% of the set is pointed at by something, so "is it owned" separates nothing.
///
/// After a join the differential scan leaves 8 to 27, which is where the question becomes worth
/// asking. The ceiling is set well above the largest of those and far below the idle set, so it
/// runs exactly where it can answer and nowhere else.
#[cfg(windows)]
const OWNER_SCAN_MAX_CANDIDATES: usize = 64;

/// One line when the set is too wide, not one per pass.
#[cfg(windows)]
static OWNER_SCAN_TOO_WIDE_SAID: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
static CACHED_SLOT: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static CACHED_SESSION: AtomicUsize = AtomicUsize::new(0);
#[cfg(windows)]
static CACHED_OWNER: AtomicUsize = AtomicUsize::new(0);
/// Said once, not once per scan: a truncated scan repeats for as long as the session is missing.
static SCAN_BUDGET_EXHAUSTED: AtomicBool = AtomicBool::new(false);
/// How many full sweeps this process may ever run.
///
/// A sweep crosses 10.98 MB with a page read per 4 KB and a `VirtualQuery` per candidate that
/// survives the field checks, and it runs on the game task. One is affordable and unavoidable;
/// one every [`SESSION_SCAN_RETRY_CALLS`] calls forever is a periodic frame-time spike, which is
/// what the retry loop became the moment bare hits were made provisional -- the owner is `0` on
/// every run so far, so the upgrade attempt never succeeds and never stops asking.
///
/// A cap rather than a longer interval, because the thing being searched for does not arrive
/// gradually: either Seamless has parked something findable in its writable data or it has not,
/// and the answer does not improve on the twentieth look. Eight leaves room for the session to be
/// built partway through a session while bounding the whole process to eight spikes.
// Not `cfg`-gated: it is a number, and the host guard that keeps it small has to be able to read
// it. The statics beside it are gated because they are only touched on the scanning path.
pub(super) const SESSION_SCAN_MAX_SWEEPS: usize = 8;
/// Said once, when the cap is reached.
#[cfg(windows)]
static SCAN_SWEEPS_SPENT_SAID: AtomicBool = AtomicBool::new(false);
/// Whether the sweeper thread has been started.
#[cfg(windows)]
static SWEEP_REQUESTED: AtomicBool = AtomicBool::new(false);
/// Passes still owed. Refilled on demand rather than spent on a timer at startup.
#[cfg(windows)]
static SWEEP_BUDGET: AtomicUsize = AtomicUsize::new(SESSION_SCAN_MAX_SWEEPS);
/// The longest the sweeper waits between passes while it still owes some.
///
/// A cap on the wait, not the wait itself: [`await_sweep_request`] ends the instant a request is
/// raised, and this only bounds how long a pass the budget already paid for can sit behind a
/// request that never comes. Long enough that eight of them span a couple of minutes of play --
/// the window in which Seamless would build a session -- and short enough that one built early is
/// picked up while the player is still in the same fight.
#[cfg(windows)]
const SESSION_SCAN_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);
