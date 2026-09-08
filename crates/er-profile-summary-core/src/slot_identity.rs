//! Does one `CS::ProfileSummary` record describe a real character?
//!
//! Host-portable on purpose: this is the predicate the whole autoload chain turns on -- it
//! decides whether the native Continue row has anything to load, whether a configured slot is
//! honoured or replaced by `best_active_slot`, and whether a picked container's re-read
//! succeeded. It used to be three lines buried inside an `unsafe fn` that dereferences live
//! game memory, so the only way to exercise it was to launch the game.

/// Lowest level a record may report and still be a real character.
///
/// A slot the player has never used reports level 0. A genuinely created character is level 1
/// at minimum -- the class templates start at 9..=10 and the level field counts the same Rune
/// Level the profile row prints, so 1 is below every reachable starting value and rejects only
/// the zeroed record.
pub const MIN_REAL_LEVEL: u32 = 1;

/// True when the record says this slot holds a character.
///
/// Deliberately not `saveSlotsStates[slot]` (the occupancy byte at `summary+0x8`): that byte is
/// a flag, `MarkProfileIndexAsUsed` (`0x140262250`) sets it without touching any record field,
/// and this DLL writes it itself, so it reads active even for a slot whose record is all
/// zeroes. Level plus a non-empty name are fields only a real deserialize (or this crate's own
/// rebuild) can have filled.
///
/// `name_empty` is the caller's `utf16_name_empty_like` verdict over the record's UTF-16 name
/// buffer; it is passed in rather than read here so the decision stays host-testable.
#[must_use]
pub fn record_is_real_character(level: u32, name_empty: bool) -> bool {
    level >= MIN_REAL_LEVEL && !name_empty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zeroed_record_is_not_a_character() {
        assert!(!record_is_real_character(0, true));
        assert!(
            !record_is_real_character(0, false),
            "level 0 alone is fatal"
        );
    }

    #[test]
    fn a_named_level_one_record_is_a_character() {
        assert!(record_is_real_character(MIN_REAL_LEVEL, false));
        assert!(record_is_real_character(713, false));
    }

    #[test]
    fn a_level_without_a_name_is_refused() {
        // The failure this guards: a record whose level survived from a previous save while the
        // name block was zeroed reads "loadable" and hands the Continue row a null character.
        assert!(!record_is_real_character(139, true));
    }
}
