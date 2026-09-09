//! Find Seamless's session by what changes when the player invades, not by what it looks like.
//!
//! # Why a shape scan cannot answer this
//!
//! Every previous attempt asked "does this object look like a session" -- a state field holding one
//! of four codes, an `_Mtx_internal_imp_t` at `+0x100`, a plausible pointer. Measured
//! 2026-09-08 by [`super::session_scan::scan_address_space_for_active_session`], 671 MB of the
//! running game holds **25,192** objects that answer yes. The signature is four small integers at
//! known offsets, and a process this size contains that pattern by accident tens of thousands of
//! times.
//!
//! The five candidates latched and then disproved live say the same thing in a smaller number, and
//! they say something else as well: `0x55080038`, `0xa4450038`, `0x1f0a0038`, `0xa2760038`,
//! `0xd2c0038` all end in the same sixteen bits. That is one repeating heap structure being found
//! over and over, not five coincidences.
//!
//! # The property no impostor can fake
//!
//! `ersc+0x25850`, the invade action, opens `cmp dword [rdi + 0x150], 1` / `jne <return>` and then
//! writes `0xe` into that same field. So the real session is idle before the player invades and
//! active immediately after, and that transition is caused by the player rather than merely
//! observed. An object that reads `0x01` forever satisfies every static check and fails this one.
//!
//! This module therefore records the addresses that read idle while nothing is happening, and when
//! a join is in flight it re-reads exactly those addresses and keeps only the ones that moved. The
//! survivors intersect across invasions, so each attempt narrows the set further rather than
//! starting over.
//!
//! Reads only. Nothing is written into the game, no thread is suspended, and the walk runs on the
//! sweeper's worker thread for the reason recorded in [`super::session_scan`]: a full pass is
//! hundreds of thousands of `ReadProcessMemory` round trips under Wine, which froze the frame for
//! 3.5 seconds every 11 when it ran on the game thread.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::ersc;

/// Candidate addresses recorded at idle, awaiting a join to disprove them.
static CANDIDATES: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// How many joins have been used to narrow the set. Reported so a survivor found after one join is
/// not mistaken for one that survived several.
static NARROWING_ROUNDS: AtomicUsize = AtomicUsize::new(0);

/// Ceiling on recorded candidates. A full pass over the game's committed private memory finds
/// tens of thousands of shape matches, and every one costs 8 bytes here plus one read per join.
/// Beyond this the snapshot is refused rather than truncated: a truncated set can silently exclude
/// the real session, which is the one failure this module exists to prevent.
const MAX_CANDIDATES: usize = 1 << 21;

/// How few survivors have to remain before each one is named in the log.
///
/// Measured 2026-09-08, run br-20260908-220803-8a62: one invasion took 179,476 candidates to 2. At
/// that size the addresses themselves are the finding, and printing them costs two lines rather
/// than another invasion.
const SURVIVORS_WORTH_NAMING: usize = 8;

/// The states this build's actions write once an attempt is under way, from `ersc::Abi` plus the
/// three the cancel row is drawn for that the ABI does not name individually (read out of ERSC's
/// own hide-predicate at `ersc+0x26b40`).
fn is_active_state(abi: &ersc::Abi, state: u32) -> bool {
    state == abi.state_searching
        || state == abi.state_offer_received
        || state == abi.state_cancelling
        || state == 0x0f
        || state == 0x10
        || state == 0x12
}

/// How many candidates are currently held, and how many joins have narrowed them.
pub(super) fn progress() -> (usize, usize) {
    let held = CANDIDATES.lock().map(|c| c.len()).unwrap_or(0);
    (held, NARROWING_ROUNDS.load(Ordering::SeqCst))
}

/// Forget everything. Used when the set empties, which means the real session was never in it.
pub(super) fn reset() {
    if let Ok(mut candidates) = CANDIDATES.lock() {
        candidates.clear();
    }
    NARROWING_ROUNDS.store(0, Ordering::SeqCst);
}

#[cfg(windows)]
pub(super) fn snapshot_idle_candidates(abi: &ersc::Abi) -> usize {
    let found = super::session_scan::walk_private_memory(abi, |address, state| {
        state == abi.state_idle && super::mutex_shape_identifies_a_session(abi, address)
    });
    let Ok(mut candidates) = CANDIDATES.lock() else {
        return 0;
    };
    if found.len() > MAX_CANDIDATES {
        crate::standalone_log(format_args!(
            "local-invasion: differential scan refused -- {} idle candidates is past the {} ceiling, \
             and truncating the list could drop the real session. Nothing is recorded this pass.",
            found.len(),
            MAX_CANDIDATES
        ));
        return 0;
    }
    *candidates = found;
    NARROWING_ROUNDS.store(0, Ordering::SeqCst);
    candidates.len()
}

/// Re-read the recorded addresses and keep only those now reading an active state.
///
/// Returns the session once exactly one candidate survives. While more than one survives it
/// returns `None` and keeps them, so the next invasion narrows further.
#[cfg(windows)]
pub(super) fn narrow_to_changed(abi: &ersc::Abi) -> Option<usize> {
    let Ok(mut candidates) = CANDIDATES.lock() else {
        return None;
    };
    if candidates.is_empty() {
        return None;
    }
    let before = candidates.len();
    candidates.retain(|address| {
        super::session_scan::read_state_at(abi, *address)
            .is_some_and(|state| is_active_state(abi, state))
    });
    let rounds = NARROWING_ROUNDS.fetch_add(1, Ordering::SeqCst) + 1;
    let after = candidates.len();
    if after == 0 {
        crate::standalone_log(format_args!(
            "local-invasion: differential scan emptied -- all {before} idle candidates stayed idle \
             through this join, so the real session was not among them. The snapshot is taken \
             again before the next attempt."
        ));
        NARROWING_ROUNDS.store(0, Ordering::SeqCst);
        return None;
    }
    crate::standalone_log(format_args!(
        "local-invasion: differential scan round {rounds}: {before} -> {after} candidate(s) \
         changed out of idle when this join started. A candidate survives only by moving when the \
         player invades, which is the one thing a look-alike object cannot do."
    ));
    // Name them once the set is small enough to read. Two survivors is the expected shape -- the
    // session and something that mirrors its state field -- and their distance apart is what says
    // which, so the address and the gap are both worth having in the log before another invasion
    // is spent narrowing further.
    if after <= SURVIVORS_WORTH_NAMING {
        let mut previous: Option<usize> = None;
        for address in candidates.iter() {
            let state = super::session_scan::read_state_at(abi, *address);
            let gap = previous.map(|last| address.saturating_sub(last));
            crate::standalone_log(format_args!(
                "local-invasion: differential survivor {address:#x} state={} {}",
                match state {
                    Some(value) => format!("{value:#04x}"),
                    None => "unreadable".to_string(),
                },
                match gap {
                    Some(bytes) => format!("(+{bytes:#x} from the previous survivor)"),
                    None => "(first)".to_string(),
                }
            ));
            previous = Some(*address);
        }
    }
    (after == 1).then(|| candidates[0])
}

#[cfg(not(windows))]
pub(super) fn snapshot_idle_candidates(_abi: &ersc::Abi) -> usize {
    0
}

#[cfg(not(windows))]
pub(super) fn narrow_to_changed(_abi: &ersc::Abi) -> Option<usize> {
    None
}
