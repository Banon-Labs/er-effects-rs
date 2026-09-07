//! Which load path the title-time autoload tick takes.
//!
//! # The fork this exists to narrow
//!
//! There are two ways this mod can put a character in the world from the title, and until
//! 2026-08-26 the choice between them was made by one predicate -- "is a direct/loose save file
//! the source?" -- which is not the question that decides whether either path can work.
//!
//! * [`TitleAutoloadRoute::NativeContinue`] fires the game's own Continue row. The native chain
//!   then runs `SetState5` -> world load -> `CS::MoveMapStep::DoSaveStuff`, which deserialises the
//!   save **in-world**, natively, with every precondition its own caller establishes. This is what
//!   the default save has always done, and it works.
//!
//! * [`TitleAutoloadRoute::TitleTimeFullRead`] is ours: submit / drain / **deserialise at the
//!   title**, which calls `0x14067b290`. That function has exactly one caller in the whole image,
//!   `CS::MoveMapStep::DoSaveStuff`, reachable only from `MoveMapStep::Update` -- the in-world
//!   step. Calling it from the boot title is calling it outside every precondition its only caller
//!   sets up, and a picked save died there (`gaitemInsTable[-1]` AV at `0x67141a`). Four separate
//!   attempts to gate those preconditions each moved the fault one step later and never removed it,
//!   which is what a wrong-place call looks like from the inside.
//!
//! So the route is not "picked save vs default save". It is "can the native Continue row be used
//! at all?", and the answer to that is whether the picked slot's `CS::ProfileSummary` record is
//! real -- because the native Continue path reads the summary to decide what to load, and the boot
//! save-data job never populated it for a container the user picked afterwards.
//!
//! The full-read stays as the narrow fallback for the one case that has no summary and no way to
//! get one (an explicit loose `save_file` at boot, before any summary exists). Everything else must
//! go down the same native path the default save does.

/// The two ways a title-time autoload tick can put the picked character in the world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleAutoloadRoute {
    /// Drive the game's own Continue row and let `MoveMapStep::DoSaveStuff` deserialise in-world.
    NativeContinue,
    /// Our own submit/drain/deserialise at the title. Narrow fallback only -- see the module docs.
    TitleTimeFullRead,
}

impl TitleAutoloadRoute {
    /// True when this route performs a deserialise at the title (the thing
    /// `oracle_title_time_deser_calls` counts and a correct run reports as 0).
    pub const fn deserialises_at_title(self) -> bool {
        matches!(self, Self::TitleTimeFullRead)
    }
}

/// Pick the route for one autoload tick.
///
/// `direct_save_file_source` is `direct_save_file_source_active()`: an explicit loose `save_file`
/// or an in-game picker selection has installed direct-file staging.
///
/// `picked_slot_summary_real` is `profile_slot_fingerprint(slot).0` for the slot the direct source
/// will load -- level >= 1 and a non-empty name in the live `CS::ProfileSummary` record. Note this
/// is not `saveSlotsStates[slot]`: `MarkProfileIndexAsUsed` sets that flag and nothing else, so a
/// marked slot can still have a blank record, which is exactly the empty-profile dead end the
/// native Continue path stalls in.
pub const fn title_autoload_route(
    direct_save_file_source: bool,
    picked_slot_summary_real: bool,
) -> TitleAutoloadRoute {
    if !direct_save_file_source {
        // The default save. Untouched behaviour: the native Continue row has always driven this.
        return TitleAutoloadRoute::NativeContinue;
    }
    if picked_slot_summary_real {
        // The summary re-read landed, so the native Continue row has something real to load and
        // the picked save takes the same in-world deserialise the default save does.
        return TitleAutoloadRoute::NativeContinue;
    }
    // No usable summary: the Continue row would sit on an empty profile forever. Fall back to the
    // title-time full read, which reads the staged bytes itself.
    TitleAutoloadRoute::TitleTimeFullRead
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_save_always_uses_the_native_continue_row() {
        // The default-save path is the one that has always worked; neither input may divert it.
        assert_eq!(
            title_autoload_route(false, false),
            TitleAutoloadRoute::NativeContinue
        );
        assert_eq!(
            title_autoload_route(false, true),
            TitleAutoloadRoute::NativeContinue
        );
    }

    #[test]
    fn a_picked_save_with_a_real_summary_joins_the_native_continue_path() {
        assert_eq!(
            title_autoload_route(true, true),
            TitleAutoloadRoute::NativeContinue
        );
    }

    #[test]
    fn a_picked_save_with_no_usable_summary_falls_back_to_the_title_time_full_read() {
        assert_eq!(
            title_autoload_route(true, false),
            TitleAutoloadRoute::TitleTimeFullRead
        );
    }

    /// The regression this change exists to prevent: before it, any direct-file source took the
    /// title-time deserialise, including one whose summary was perfectly readable.
    #[test]
    fn a_readable_summary_is_what_removes_the_title_time_deserialise() {
        assert!(!title_autoload_route(true, true).deserialises_at_title());
        assert!(title_autoload_route(true, false).deserialises_at_title());
        assert!(!title_autoload_route(false, false).deserialises_at_title());
    }

    /// Exhaustive over the whole input space: exactly one of the four combinations deserialises at
    /// the title, so no future edit can widen the fallback without failing this.
    #[test]
    fn exactly_one_input_combination_deserialises_at_the_title() {
        let deserialising = [(false, false), (false, true), (true, false), (true, true)]
            .into_iter()
            .filter(|(direct, real)| title_autoload_route(*direct, *real).deserialises_at_title())
            .collect::<Vec<_>>();
        assert_eq!(deserialising, vec![(true, false)]);
    }
}
