//! Marking every eligible item refill / no-refill, and deciding which way a press goes.
//!
//! # Why this does not call the function the storage box calls
//!
//! `SetItemReplenishState` (1.16.2 `0x140786430`) is what the storage-box UI invokes, and it is a
//! toggle, not a setter:
//!
//! ```c
//! bVar2 = ShouldReplenishItem(tracker, itemId);
//! SetState(tracker, itemId, !bVar2);        // flips whatever was there
//! ```
//!
//! Looping that over every item SCATTERS the states -- each item lands on the opposite of whatever
//! it happened to be -- which is not "mark everything" and looks, from the player's side, exactly
//! like the feature half-working. It also `DLPanic`s when `GLOBAL_CSMenuMan` is null.
//!
//! One level down, `CS::ItemReplenishStateTracker::SetState` (`0x14023dd80`) is the absolute
//! setter. It is idempotent, and it self-FILTERS: its first act is `GetEquipParamReplenishType`
//! with an early return on `None`, so passing an ineligible id is a safe no-op and this module
//! needs no eligibility table of its own. It also skips the `CSMenuMan` check, which is why the
//! null checks below are ours to do.
//!
//! # The ceiling that makes this dangerous
//!
//! The tracker is a `DLFixedVector` of 2048 entries. Both insertion paths -- `InsertSorted`
//! (`0x14023df20`) and the append path (`0x14023e270`) -- carry the same guard:
//!
//! ```c
//! if (0x800 < param_1->count + 1U) { DLPanic(..., "out of memory.", ...); }  // does not return
//! ```
//!
//! `DLPanic` does not return: overflowing this vector crashes the game outright. An offline census
//! of the stock 1.16.2 regulation (`scripts/regulation-autoreplenish-census.py`) counts 449
//! eligible rows against that 2048, so the feature fits with room to spare -- but "fits on the
//! stock regulation" is not "cannot overflow". A modded regulation can add rows, and a weapon id
//! carries its reinforcement level, so the set of ids that can legally reach the tracker is not
//! bounded by the row count alone. [`INSERT_CEILING`] is the belt: this module stops inserting and
//! says so in the log rather than letting the game die.

// Every item below is consumed by `runtime`, which is windows-only, plus the tests. Scoped
// to this module and to `dead_code` alone -- Not a crate-level blanket that would also
// swallow `unused_imports` and hide a real lint (bd
// host-build-cfg-gate-allow-pattern-hides-real-lints).
#![cfg_attr(not(windows), allow(dead_code))]

/// `entries` is `ItemReplenishStateEntry[2048]`; `count + 1 > 0x800` is the DLPanic.
pub(crate) const TRACKER_CAPACITY: u64 = 0x800;
/// Stop inserting here rather than at the capacity itself.
///
/// The margin is not superstition. The vanilla storage-box UI inserts into this same vector, and
/// so does the game's own restore path; leaving the last slots free means a player who fills the
/// tracker with our feature can still toggle an item by hand afterwards without the game dying on
/// the insert. Crashing the game is a far worse outcome than marking 32 fewer items.
pub(crate) const INSERT_MARGIN: u64 = 32;
/// The highest `count` at which this module will still insert a new entry.
pub(crate) const INSERT_CEILING: u64 = TRACKER_CAPACITY - INSERT_MARGIN;

/// The offline census figure: 71 `EquipParamWeapon` + 378 `EquipParamGoods` on stock 1.16.2,
/// measured by `scripts/regulation-autoreplenish-census.py`.
const STOCK_ELIGIBLE_ROWS: u64 = 449;

// Compile-time, deliberately not a unit test. A ceiling at or above the vector's capacity is a
// DLPanic -- the game dying, not a wrong answer -- so it must fail the build rather than wait for
// someone to run the suite.
const _: () = assert!(
    INSERT_CEILING < TRACKER_CAPACITY,
    "the insert ceiling must leave real headroom under the DLPanic"
);
const _: () = assert!(
    STOCK_ELIGIBLE_ROWS < INSERT_CEILING,
    "the stock eligible set must fit under the insert ceiling"
);

/// Which way a press should go, given what the tracker currently holds.
///
/// The rule: **if every eligible item is already on, turn everything off; otherwise turn everything
/// on.** From any mixed state the first press is therefore always "turn everything on", which is
/// the predictable behaviour, and the cycle corrects itself rather than drifting.
///
/// The alternative -- a `bool` in the DLL remembering which way the last press went -- desyncs the
/// moment the player toggles a single item in the storage box, reloads a save, or loads a different
/// character. This function keeps no state at all, which is what makes it correct across all three.
#[must_use]
pub(crate) const fn next_target_state(eligible: u32, currently_on: u32) -> bool {
    // No eligible items at all (params not streamed, or a regulation with none): "turn on" is the
    // harmless answer, and the caller will find nothing to write.
    if eligible == 0 {
        return true;
    }
    currently_on < eligible
}

/// What one press did, for the log line and the tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MarkOutcome {
    /// Eligible ids considered.
    pub(crate) eligible: u32,
    /// Entries already present that were flipped in place.
    pub(crate) flipped: u32,
    /// Entries newly inserted through the native setter.
    pub(crate) inserted: u32,
    /// Ids already in the wanted state, so nothing was written.
    pub(crate) unchanged: u32,
    /// Ids skipped because inserting them would have approached the DLPanic ceiling.
    pub(crate) skipped_full: u32,
}

impl MarkOutcome {
    pub(crate) const fn wrote_anything(self) -> bool {
        self.flipped > 0 || self.inserted > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fully_on_tracker_turns_everything_off() {
        assert!(!next_target_state(449, 449));
    }

    #[test]
    fn a_fully_off_tracker_turns_everything_on() {
        assert!(next_target_state(449, 0));
    }

    /// The self-correcting half: any mixture goes on first, not "whatever is the majority".
    #[test]
    fn a_mixed_tracker_turns_everything_on_first() {
        assert!(next_target_state(449, 1));
        assert!(next_target_state(449, 448));
        assert!(next_target_state(449, 224));
    }

    /// Two presses from a mixed state must land on on then off -- a real cycle, not a stall.
    #[test]
    fn pressing_twice_from_a_mixed_state_cycles() {
        let first = next_target_state(449, 200);
        assert!(first, "first press turns everything on");
        // After the first press every eligible item is on.
        let second = next_target_state(449, 449);
        assert!(!second, "second press turns everything off");
        // And after that, none are.
        assert!(next_target_state(449, 0), "third press turns them back on");
    }

    #[test]
    fn no_eligible_items_is_not_a_crash() {
        assert!(next_target_state(0, 0));
    }

    #[test]
    fn an_outcome_that_wrote_nothing_says_so() {
        assert!(!MarkOutcome::default().wrote_anything());
        assert!(
            MarkOutcome {
                flipped: 1,
                ..MarkOutcome::default()
            }
            .wrote_anything()
        );
        assert!(
            MarkOutcome {
                inserted: 1,
                ..MarkOutcome::default()
            }
            .wrote_anything()
        );
        assert!(
            !MarkOutcome {
                unchanged: 400,
                skipped_full: 49,
                ..MarkOutcome::default()
            }
            .wrote_anything(),
            "unchanged and skipped are not writes"
        );
    }
}
