//! Pure boot missing-save picker decisions shared with the host glue.
//!
//! The root product still owns runtime state, telemetry flushing, thread spawning and process exit.
//! This module keeps the values and decision table that are picker semantics rather than root hook
//! plumbing.

#[cfg(feature = "os-dialog")]
use crate::os_dialog::OsPickAbort;

/// Nothing has opened a boot picker (or this is not a missing-save boot).
pub const BOOT_PICKER_IDLE: usize = 0;
/// A surface owns the boot pick and is waiting on the user.
pub const BOOT_PICKER_OPEN: usize = 1;
/// A file cleared the shared validity predicate; the character sub-picker owns the rest.
pub const BOOT_PICKER_PICKED: usize = 2;
/// The user cancelled the boot OS dialog; the game is quitting.
pub const BOOT_PICKER_CANCEL_EXIT: usize = 3;
/// comdlg32 was unusable; the in-game browser took the pick over.
pub const BOOT_PICKER_FELL_BACK: usize = 4;

/// What an abandoned boot open means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootAbortAction {
    /// The user decided. Quit the game.
    QuitGame,
    /// We could not ask. Hand the pick to the in-game browser instead of acting on a choice nobody
    /// made.
    FallBackToInGame,
}

/// Map an abandoned OS open onto the boot intent's response.
///
/// Only a genuine user cancel quits. A comdlg32 failure, a refused re-entrant open and an exhausted
/// reopen bound all mean the dialog could not be used.
#[cfg(feature = "os-dialog")]
pub fn boot_abort_action(abort: OsPickAbort) -> BootAbortAction {
    match abort {
        OsPickAbort::Cancelled => BootAbortAction::QuitGame,
        OsPickAbort::Failed | OsPickAbort::NotOpened => BootAbortAction::FallBackToInGame,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE decision that can terminate a user's game, pinned. Only a cancel -- the one outcome
    /// that IS a user decision -- quits; every "we could not ask" outcome falls back to the
    /// in-game browser instead of acting on a choice nobody made.
    ///
    /// The table is exhaustive over [`OsPickAbort`] on purpose: a NEW way for an open to end with
    /// nothing staged must be classified here before it compiles, because the default that a
    /// catch-all would supply is "quit the user's game".
    #[cfg(feature = "os-dialog")]
    #[test]
    fn only_a_user_cancel_quits_the_game() {
        assert_eq!(
            boot_abort_action(OsPickAbort::Cancelled),
            BootAbortAction::QuitGame
        );
        assert_eq!(
            boot_abort_action(OsPickAbort::Failed),
            BootAbortAction::FallBackToInGame,
            "a comdlg32 defect -- or an exhausted reopen bound -- must never terminate the process"
        );
        assert_eq!(
            boot_abort_action(OsPickAbort::NotOpened),
            BootAbortAction::FallBackToInGame,
            "no dialog ever ran, so there is no user decision to act on; this thread cannot retry, \
             so the in-game browser takes the pick instead"
        );
    }

    /// Every boot state is distinct. They are exported as one telemetry field, so a collision
    /// would make two different outcomes indistinguishable in the only record that survives the
    /// process -- and one of those outcomes is a quit.
    #[test]
    fn the_boot_states_are_distinguishable_in_telemetry() {
        let states = [
            BOOT_PICKER_IDLE,
            BOOT_PICKER_OPEN,
            BOOT_PICKER_PICKED,
            BOOT_PICKER_CANCEL_EXIT,
            BOOT_PICKER_FELL_BACK,
        ];
        for (index, state) in states.iter().enumerate() {
            for other in &states[index + 1..] {
                assert_ne!(state, other, "two boot states share a telemetry value");
            }
        }
        assert_eq!(
            BOOT_PICKER_IDLE, 0,
            "a session that never reaches a missing-save boot must read as IDLE"
        );
    }
}
