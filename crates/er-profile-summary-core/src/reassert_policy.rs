//! Does the live `CS::ProfileSummary` still describe the character that will actually load?
//!
//! # The assumption this exists to retire
//!
//! `picked_refresh`'s module doc used to say the records and the bodies "agree by construction",
//! because both come from the staged container. They do not. A container carries two descriptions
//! of each slot: the `USER_DATA010` profile-summary table, which is what
//! `CS::ProfileSummary::Deserialize` reads, and the slot body, which is what actually deserializes
//! into the character. Nothing keeps them in step, and a container assembled by tooling can
//! disagree with itself. Measured 2026-09-03 on `100-Lilbro/ER0000.co2` (6 of 10 slots disagree):
//!
//! ```text
//! slot 4 body='Hero'  lvl=7 bodymap=0x0e000000 | USER_DATA010 name='Vagabond' lvl=9 block=0x1c000000
//! slot 8 body='Prophet' lvl=7                  | USER_DATA010 name='Pro'      lvl=7
//! ```
//!
//! `Hero` loaded; the loading screen showed `Vagabond`'s face, name and level, because the portrait
//! and the stats panel both read the record. The body is the truth about what loads, so the record
//! has to be made to agree with it.
//!
//! # Why a watch rather than one rewrite
//!
//! The DLL's body-derived rewrite is not the last writer. In run br-20260903-204517-82d2 it landed
//! at +13523ms and the game's own boot ProfileSummary read fired 617ms later and overwrote it, and
//! `refresh_direct_source_profile_summary` had already latched `PICKED_SUMMARY_REFRESH_STATE` so it
//! never looked again. Whoever writes last wins, so the only durable answer is to keep checking for
//! a bounded window and re-assert on drift.

use er_game_base::fnv1a::{FNV1A64_OFFSET_BASIS, fnv1a64_extend};

/// One record's identity, as cheaply as a live read can establish it.
///
/// Name plus level, never the map: a record's `+0x30` is written from `GetCurrentMapId` when the
/// game fills it and from the body's saved `BlockId` when this DLL does, and those two legitimately
/// differ on 65 of 726 corpus slots. A map term here would fire on characters that are perfectly
/// fine (the same trap `portrait_render_slot_semaphore` documents at length).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecordIdentity {
    /// FNV-1a 64 over the record's UTF-16 name units, up to but excluding the terminator.
    pub name_hash: u64,
    /// Rune Level as the record reports it. `0` means "no character here".
    pub level: u32,
}

impl RecordIdentity {
    /// True when this identity describes a character at all -- a named record at level >= 1.
    #[must_use]
    pub fn is_character(&self) -> bool {
        self.level >= crate::slot_identity::MIN_REAL_LEVEL && self.name_hash != EMPTY_NAME_HASH
    }
}

/// The hash a zeroed or terminator-first name produces: no units hashed, i.e. the offset basis.
///
/// Taken from the shared owner rather than written out again. `scripts/check-fnv1a-owner.py`
/// rejects a second copy of the basis or the prime anywhere outside `er-game-base`, because two
/// copies of a constant are two things that can disagree -- and this file learned that the hard
/// way: its first version spelled both literals locally, passed a truncated local gate read, and
/// went red in CI.
pub const EMPTY_NAME_HASH: u64 = FNV1A64_OFFSET_BASIS;

/// The name hash both sides of the drift comparison use: FNV-1a 64 over UTF-16 code units in
/// little-endian byte order, through `er_game_base::fnv1a`.
///
/// The only property that matters is that it is the same function for the live record's units and
/// for the container body's re-encoded name, so it delegates rather than reimplementing.
#[must_use]
pub fn name_hash_utf16(units: &[u16]) -> u64 {
    let mut hash = FNV1A64_OFFSET_BASIS;
    for unit in units {
        hash = fnv1a64_extend(hash, &unit.to_le_bytes());
    }
    hash
}

/// How many times the body-derived rewrite may be re-asserted for one boot.
///
/// The rewrite is a ~26 MB container read plus ten record writes, and every re-assert means
/// something else wrote the records after we did. Four is enough for the observed single clobber
/// with room for a retry, and small enough that a genuine disagreement cannot become a per-frame
/// file read.
pub const REASSERT_MAX_REWRITES: usize = 4;

/// How long after the successful refresh the watch stays armed, in autoload ticks (~1 per frame).
///
/// The window it has to cover is "the game's boot ProfileSummary deserialize might still fire",
/// which in the measured run was 617ms. It must not stay armed into gameplay: once in world, the
/// game rewrites the loaded slot's record from the live character, so a level-up would look like
/// drift and this would start rewriting records under a playing character.
pub const REASSERT_WATCH_TICKS: usize = 900;

/// What the drift watch should do this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReassertStep {
    /// Nothing to do: no expectation recorded, the record still agrees, or the watch has expired.
    Hold,
    /// Something overwrote the records with a different character. Re-assert, as rewrite N (1-based).
    Rewrite(usize),
    /// The rewrite budget is spent; leave the records alone and say so once.
    Exhausted,
}

/// Decide this tick.
///
/// `expected` is the identity the container's body gives the target slot (`None` until a container
/// has been read). `live` is what the record says right now. `ticks_since_refresh` counts autoload
/// ticks since the refresh that recorded `expected`.
///
/// A live record that is not a character is deliberately `Hold`, not `Rewrite`: a zeroed record is
/// what a native teardown looks like mid-flight, and re-asserting into one would race the game's
/// own write rather than correct it. Only a record that confidently describes a different
/// character is drift worth acting on.
#[must_use]
pub fn reassert_step(
    expected: Option<RecordIdentity>,
    live: RecordIdentity,
    rewrites: usize,
    ticks_since_refresh: usize,
) -> ReassertStep {
    let Some(expected) = expected else {
        return ReassertStep::Hold;
    };
    if !expected.is_character() || !live.is_character() || live == expected {
        return ReassertStep::Hold;
    }
    if ticks_since_refresh > REASSERT_WATCH_TICKS {
        return ReassertStep::Hold;
    }
    if rewrites >= REASSERT_MAX_REWRITES {
        return ReassertStep::Exhausted;
    }
    ReassertStep::Rewrite(rewrites + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(name: &str, level: u32) -> RecordIdentity {
        let units: Vec<u16> = name.encode_utf16().collect();
        RecordIdentity {
            name_hash: name_hash_utf16(&units),
            level,
        }
    }

    #[test]
    fn the_empty_name_hash_constant_is_the_hash_of_no_units() {
        assert_eq!(name_hash_utf16(&[]), EMPTY_NAME_HASH);
        assert!(!RecordIdentity::default().is_character());
    }

    #[test]
    fn the_measured_clobber_is_drift_and_is_rewritten() {
        // Run br-20260903-204517-82d2: we wrote slot 4 as body 'Hero' RL7; the game's boot
        // ProfileSummary read replaced it with USER_DATA010's 'Vagabond' RL9 617ms later.
        let expected = ident("Hero", 7);
        let live = ident("Vagabond", 9);
        assert_eq!(
            reassert_step(Some(expected), live, 0, 40),
            ReassertStep::Rewrite(1)
        );
    }

    #[test]
    fn an_agreeing_record_is_never_rewritten() {
        let hero = ident("Hero", 7);
        assert_eq!(reassert_step(Some(hero), hero, 0, 0), ReassertStep::Hold);
        // Same name, different level is still drift -- the level is half the identity.
        assert_eq!(
            reassert_step(Some(hero), ident("Hero", 8), 0, 0),
            ReassertStep::Rewrite(1)
        );
    }

    #[test]
    fn nothing_happens_before_a_container_has_been_read() {
        assert_eq!(
            reassert_step(None, ident("Vagabond", 9), 0, 0),
            ReassertStep::Hold
        );
    }

    #[test]
    fn a_zeroed_live_record_is_left_alone() {
        // A native teardown zeroes the record before rewriting it. Re-asserting into that window
        // races the game instead of correcting it, and the next tick sees the real value anyway.
        assert_eq!(
            reassert_step(Some(ident("Hero", 7)), RecordIdentity::default(), 0, 10),
            ReassertStep::Hold
        );
        assert_eq!(
            reassert_step(Some(ident("Hero", 7)), ident("Hero", 0), 0, 10),
            ReassertStep::Hold
        );
    }

    #[test]
    fn the_watch_expires_so_it_cannot_reach_gameplay() {
        let expected = ident("Hero", 7);
        let levelled = ident("Hero", 8);
        assert_eq!(
            reassert_step(Some(expected), levelled, 0, REASSERT_WATCH_TICKS),
            ReassertStep::Rewrite(1)
        );
        assert_eq!(
            reassert_step(Some(expected), levelled, 0, REASSERT_WATCH_TICKS + 1),
            ReassertStep::Hold,
            "a character who levelled up in world must not be rewritten from a boot-time file read"
        );
    }

    #[test]
    fn the_rewrite_budget_is_finite_and_says_so() {
        let expected = ident("Hero", 7);
        let live = ident("Vagabond", 9);
        assert_eq!(
            reassert_step(Some(expected), live, REASSERT_MAX_REWRITES - 1, 0),
            ReassertStep::Rewrite(REASSERT_MAX_REWRITES)
        );
        assert_eq!(
            reassert_step(Some(expected), live, REASSERT_MAX_REWRITES, 0),
            ReassertStep::Exhausted
        );
    }

    #[test]
    fn different_names_hash_differently_including_the_truncations_seen_in_the_corpus() {
        // 'Prophet' vs its USER_DATA010 truncation 'Pro', and 'Astrologer' vs 'Astro' -- both real
        // pairs from the measured container, and both must read as different characters.
        for (body, stored) in [("Prophet", "Pro"), ("Astrologer", "Astro")] {
            assert_ne!(
                name_hash_utf16(&body.encode_utf16().collect::<Vec<_>>()),
                name_hash_utf16(&stored.encode_utf16().collect::<Vec<_>>()),
                "{body} vs {stored}"
            );
        }
    }
}
