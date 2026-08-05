//! What destination a Seamless invasion actually chose, read straight out of `CSGameMan`.
//!
//! # Why this exists
//!
//! Seamless Co-op does not use retail matchmaking and does not use the `.aip` / MSB
//! `InvasionPoint` tables this crate's map surface is built from -- none of `ersc.dll`'s 286
//! resolved signature slots reach `CSBreakInPointManager` or `CS::QuickmatchManager`. So the
//! pins cannot tell anyone where a Seamless invader will land, and reading `ersc.dll` cannot
//! either: the code that picks a target sits inside the ~3.4% of its `.text` that Themida
//! encrypted.
//!
//! What IS readable is the answer, after the fact. An invader's destination is not a coordinate
//! -- it is an MSB **entry-point entity ID** plus a destination map, and both are handed to the
//! engine through `CSGameMan` where anything can read them. One invasion therefore tells us what
//! the encrypted code decided, which is the fact the static analysis could not produce.
//!
//! # Offsets, each byte-verified against `eldenring-deobf.bin` via its own named getter
//!
//! | field | offset | getter |
//! |---|---|---|
//! | entry-point entity id | `+0xaf0` | `GetNpcInvadeTargetEntryPoint` `0x140679650` |
//! | destination block id  | `+0xac8` | `GetTargetMapId` `0x140679630` |
//! | rapid-reentry flag    | `+0xafc` | `ShouldUseRapidReentry` `0x14067a150` |
//!
//! All three are `mov rax,[0x143d69918]` followed by a load at their offset, so the singleton
//! pointer is confirmed three times over. `FUN_140afcf60` -- the spawn resolver `ersc.dll` hooks
//! at `0x140af9d20` -- calls the first two, which is what puts these fields on the placement path
//! rather than merely near it.
//!
//! This module only READS. It never writes `CSGameMan` and never calls into the engine, because
//! the point is to find out what Seamless does, and a probe that perturbs the thing it measures
//! answers a different question.

use crate::invasion_warp::BlockKey;

/// `GLOBAL_CSGameMan` -- the pointer at `0x143d69918`.
///
/// Re-exported from the one place it is declared rather than repeated here: a second literal
/// is free to drift from the first, and an address that silently disagrees with itself is the
/// failure this repo's alias-drift gate exists to catch.
#[cfg(windows)]
pub use er_game_base::rva::GAME_MAN_SINGLETON_RVA;
/// `CSGameMan+0xaf0` -- `npcInvadeTargetEntryPoint`, the MSB entry point an invader spawns at.
pub const NPC_INVADE_TARGET_ENTRY_POINT_OFFSET: usize = 0xaf0;
/// `CSGameMan+0xac8` -- the destination `BlockId`.
pub const TARGET_MAP_ID_OFFSET: usize = 0xac8;
/// `CSGameMan+0xafc` -- `shouldUseRapidReentry`, the "Seek opponent" flag.
pub const SHOULD_USE_RAPID_REENTRY_OFFSET: usize = 0xafc;

/// Sentinel the engine leaves in the entry-point slot when nothing is targeted.
///
/// `GetNpcInvadeTargetEntryPoint` returns a raw `int`, and an unset slot reads as `-1` the same
/// way every other unset entity id in this engine does.
pub const ENTRY_POINT_NONE: i32 = -1;

/// One reading of the invasion destination fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvadeDestination {
    /// MSB entry-point entity id the invader will be placed at.
    pub entry_point: i32,
    /// Destination map.
    pub block: BlockKey,
    /// Whether the "Seek opponent" repeat-invasion flag is set.
    pub rapid_reentry: bool,
}

impl InvadeDestination {
    /// Whether this reading names an actual destination.
    ///
    /// Both halves are required: an entry point with no map, or a map with no entry point, is a
    /// half-written slot caught mid-update, not a destination. Reporting one would put a number
    /// in the log that no invasion ever used.
    #[must_use]
    pub const fn is_armed(&self) -> bool {
        self.entry_point != ENTRY_POINT_NONE && self.entry_point != 0 && !self.block.is_none()
    }
}

/// Decides when a reading is worth reporting.
///
/// Kept separate from the memory read so the reporting rule is testable with no game: the whole
/// value of this probe is that it emits exactly once per invasion, and a sampler that logged
/// every frame would bury the one line that matters under thousands of identical ones.
#[derive(Debug, Default)]
pub struct InvadeDestinationWatcher {
    last_reported: Option<InvadeDestination>,
}

impl InvadeDestinationWatcher {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            last_reported: None,
        }
    }

    /// Feed a reading; returns `Some` only when it is new and armed.
    ///
    /// A disarmed reading CLEARS the memo rather than being ignored, so invading the same place
    /// twice reports twice -- the slot returns to `-1` between invasions, and treating the second
    /// one as a duplicate would hide exactly the repeat case the user cares about.
    pub fn observe(&mut self, reading: InvadeDestination) -> Option<InvadeDestination> {
        if !reading.is_armed() {
            self.last_reported = None;
            return None;
        }
        if self.last_reported == Some(reading) {
            return None;
        }
        self.last_reported = Some(reading);
        Some(reading)
    }
}

#[cfg(windows)]
mod native {
    use super::{
        GAME_MAN_SINGLETON_RVA, InvadeDestination, NPC_INVADE_TARGET_ENTRY_POINT_OFFSET,
        SHOULD_USE_RAPID_REENTRY_OFFSET, TARGET_MAP_ID_OFFSET,
    };
    use crate::invasion_warp::BlockKey;

    /// Read the invasion destination fields out of the live `CSGameMan`.
    ///
    /// Returns `None` before the singleton exists (title screen) or if any field is unreadable.
    /// Every read is fault-closed, so a bad pointer costs the reading rather than the game.
    ///
    /// # Safety
    /// Game task thread.
    #[must_use]
    pub unsafe fn read_invade_destination(base: usize) -> Option<InvadeDestination> {
        let game_man =
            unsafe { er_game_base::mem::safe_read_usize(base + GAME_MAN_SINGLETON_RVA) }?;
        if game_man == 0 {
            return None;
        }
        let entry_point = unsafe {
            er_game_base::mem::safe_read_i32(game_man + NPC_INVADE_TARGET_ENTRY_POINT_OFFSET)
        }?;
        let block = unsafe { er_game_base::mem::safe_read_i32(game_man + TARGET_MAP_ID_OFFSET) }?;
        let rapid =
            unsafe { er_game_base::mem::safe_read_u8(game_man + SHOULD_USE_RAPID_REENTRY_OFFSET) }?;
        Some(InvadeDestination {
            entry_point,
            block: BlockKey::from_raw(block as u32),
            rapid_reentry: rapid != 0,
        })
    }
}

#[cfg(windows)]
pub use native::read_invade_destination;

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(entry: i32, block: u32) -> InvadeDestination {
        InvadeDestination {
            entry_point: entry,
            block: BlockKey::from_raw(block),
            rapid_reentry: false,
        }
    }

    #[test]
    fn an_unset_entry_point_is_not_a_destination() {
        assert!(!reading(ENTRY_POINT_NONE, 0x0f00_0000).is_armed());
        assert!(!reading(0, 0x0f00_0000).is_armed());
    }

    #[test]
    fn an_entry_point_with_no_map_is_a_half_written_slot() {
        // Caught mid-update. Reporting it would put an id in the log no invasion ever used.
        assert!(!reading(10_001_950, crate::invasion_warp::BLOCK_KEY_NONE_RAW).is_armed());
    }

    #[test]
    fn a_full_reading_is_a_destination() {
        assert!(reading(10_001_950, 0x0f00_0000).is_armed());
    }

    #[test]
    fn the_first_armed_reading_reports() {
        let mut watcher = InvadeDestinationWatcher::new();
        assert_eq!(
            watcher.observe(reading(10_001_950, 0x0f00_0000)),
            Some(reading(10_001_950, 0x0f00_0000))
        );
    }

    #[test]
    fn the_same_reading_held_across_frames_reports_once() {
        let mut watcher = InvadeDestinationWatcher::new();
        let r = reading(10_001_950, 0x0f00_0000);
        assert!(watcher.observe(r).is_some());
        assert!(watcher.observe(r).is_none());
        assert!(watcher.observe(r).is_none());
    }

    #[test]
    fn invading_the_same_place_twice_reports_twice() {
        // THE CASE THIS PROBE EXISTS FOR. The slot returns to -1 between invasions; if a
        // disarmed reading did not clear the memo, the second invasion to the same spot would be
        // silently deduplicated -- and "does it send me to the same place again" is the question.
        let mut watcher = InvadeDestinationWatcher::new();
        let r = reading(10_001_950, 0x0f00_0000);
        assert!(watcher.observe(r).is_some());
        assert!(
            watcher
                .observe(reading(ENTRY_POINT_NONE, 0x0f00_0000))
                .is_none()
        );
        assert!(watcher.observe(r).is_some());
    }

    #[test]
    fn a_different_entry_point_in_the_same_map_reports() {
        let mut watcher = InvadeDestinationWatcher::new();
        assert!(watcher.observe(reading(10_001_950, 0x0f00_0000)).is_some());
        assert!(watcher.observe(reading(10_001_951, 0x0f00_0000)).is_some());
    }

    #[test]
    fn the_rapid_reentry_flag_is_part_of_the_reading() {
        // "Seek opponent" is the designed repeat-invasion path, so a run that used it must be
        // distinguishable from one that did not.
        let mut watcher = InvadeDestinationWatcher::new();
        let plain = reading(10_001_950, 0x0f00_0000);
        let mut seeking = plain;
        seeking.rapid_reentry = true;
        assert!(watcher.observe(plain).is_some());
        assert!(watcher.observe(seeking).is_some());
    }
}
