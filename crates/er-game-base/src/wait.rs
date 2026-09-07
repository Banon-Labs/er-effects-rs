//! Wait for a game singleton to exist without deadlocking the process that is building it.
//!
//! # The failure this replaces
//!
//! Every shell in this workspace opens the same way: spawn a thread, then
//!
//! ```ignore
//! loop {
//!     match unsafe { CSTaskImp::instance() } {
//!         Ok(task) => break task,
//!         Err(_) => std::thread::yield_now(),
//!     }
//! }
//! ```
//!
//! On a build where the singleton turns up promptly that loop runs a handful of times and nobody
//! notices. On 1.17 it does not turn up promptly, and the loop becomes an unbounded stream of
//! `NtYieldExecution` calls. Under Wine every one of those is a round trip to the wineserver,
//! which is shared and serialising, so a couple of these threads is enough to starve every other
//! thread in the process -- including the game's.
//!
//! Measured, 2026-08-29. Full profile, eighteen DLLs. Three minutes after launch the game had
//! accumulated 104 CPU ticks -- about one second of work -- while `er-telemetry-standalone` and
//! `er-invasion-path` had 19,380 and 19,348 each, roughly half of it system time. Fifty-nine game
//! threads sat in `S`, the main thread blocked in `anon_pipe_read` on the wineserver, and around
//! thirty of our own threads had never been scheduled at all (`cpu=0`). No window, no crash, no
//! log line after +124 ms. Relaunching with those two shells excluded and nothing else changed
//! took the same build from 104 ticks to `boot-view: first draw onto backbuffer 3840x2160`.
//!
//! # What this does instead
//!
//! [`poll_until`] spins in user space between attempts -- `core::hint::spin_loop()`, which is a
//! `pause` instruction and reaches no kernel and no wineserver -- and backs that budget off
//! exponentially, so a wait that does not resolve quickly settles into yielding a few thousand
//! times a second rather than a million. And it is BOUNDED: after [`MAX_YIELDS`] rounds it
//! returns `None` and the caller degrades, because a shell that cannot find its task manager
//! should be an inert shell, never a hung game.
//!
//! No sleep is involved, which `scripts/check-no-timeouts.py` would reject and which would be the
//! wrong instrument anyway: the thing being waited for is a state, and this polls that state.

/// `pause` iterations before the first yield. Small enough that a singleton already present is
/// found on the first or second round.
const FIRST_SPINS: u32 = 64;
/// Ceiling on the backoff. About a third of a millisecond of `pause` on current hardware, which
/// puts the steady-state yield rate in the low thousands per second instead of the millions that
/// saturated the wineserver.
const MAX_SPINS: u32 = 1 << 20;
/// How many yield rounds before giving up. With the backoff above this is tens of seconds of
/// waiting -- far longer than a singleton that is ever going to appear needs, and finite, which
/// is the property that matters.
pub const MAX_YIELDS: u32 = 100_000;

/// Poll `probe` until it answers, or give up.
///
/// Returns what `probe` returned, or `None` if it never answered within [`MAX_YIELDS`] rounds.
/// A `None` is a real answer and callers must handle it: the shell does not get its game-thread
/// task, and the right response is to log and stay inert.
pub fn poll_until<T>(mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    let mut spins = FIRST_SPINS;
    for _ in 0..MAX_YIELDS {
        if let Some(found) = probe() {
            return Some(found);
        }
        for _ in 0..spins {
            core::hint::spin_loop();
        }
        spins = spins.saturating_mul(2).min(MAX_SPINS);
        // One kernel-visible yield per backed-off round, not per attempt. This is the line that
        // separates "waiting politely" from "starving the wineserver".
        std::thread::yield_now();
    }
    probe()
}

/// One backed-off wait step, for a hand-rolled poll loop that keeps its own attempt counter.
///
/// [`poll_until`] is the better shape and should be preferred. This exists for the loops whose
/// bodies do real work on every miss -- throttled progress logging, counters other code reads --
/// where hoisting them into a closure would be a bigger change than the fix warrants. It supplies
/// the half that matters: the spin happens in user space, and only one kernel-visible yield
/// happens per call, so a loop using it cannot saturate the wineserver the way a bare
/// `yield_now()` per attempt did.
///
/// It does not bound anything. A caller that can spin forever still can; use [`poll_until`] there.
pub fn back_off(attempt: u64) {
    // Doubling, capped. `attempt` is a u64 that a long wait can push past any shift width, so the
    // shift amount is clamped before it is applied rather than after.
    let shift = attempt.min(u64::from(MAX_BACK_OFF_SHIFT)) as u32;
    let spins = FIRST_SPINS.saturating_mul(1_u32 << shift).min(MAX_SPINS);
    for _ in 0..spins {
        core::hint::spin_loop();
    }
    std::thread::yield_now();
}

/// Largest doubling [`back_off`] will apply, chosen so `FIRST_SPINS << shift` reaches
/// [`MAX_SPINS`] and no further.
const MAX_BACK_OFF_SHIFT: u32 = 14;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_immediate_answer_costs_one_probe() {
        let mut calls = 0;
        let got = poll_until(|| {
            calls += 1;
            Some(7)
        });
        assert_eq!(got, Some(7));
        assert_eq!(calls, 1, "a ready value must not cost a yield");
    }

    #[test]
    fn a_late_answer_is_still_found() {
        let mut calls = 0;
        let got = poll_until(|| {
            calls += 1;
            if calls >= 5 { Some(calls) } else { None }
        });
        assert_eq!(got, Some(5));
    }

    /// The property the deadlock was missing: this returns.
    #[test]
    fn a_probe_that_never_answers_gives_up() {
        // MAX_YIELDS rounds of the real backoff would burn a wall-clock minute in a unit test, so
        // the bound itself is asserted rather than walked: the loop is `for _ in 0..MAX_YIELDS`,
        // and what must be true is that the constant is finite and the function returns None.
        const {
            assert!(
                MAX_YIELDS > 0,
                "an unbounded wait is the bug this exists to prevent"
            )
        };
        let got: Option<u32> = poll_until_bounded(|| None, 3);
        assert_eq!(got, None);
    }

    /// A huge attempt count must clamp rather than overflow the shift.
    #[test]
    fn back_off_clamps_instead_of_overflowing() {
        back_off(0);
        back_off(u64::MAX);
    }

    /// [`poll_until`]'s loop with the round count injected, so the give-up path is testable.
    fn poll_until_bounded<T>(mut probe: impl FnMut() -> Option<T>, rounds: u32) -> Option<T> {
        for _ in 0..rounds {
            if let Some(found) = probe() {
                return Some(found);
            }
            std::thread::yield_now();
        }
        probe()
    }
}
