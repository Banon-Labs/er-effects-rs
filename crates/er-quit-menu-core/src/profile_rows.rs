//! The PROFILESELECT list cursor is a row index, not a PROFILESUMMARY slot.
//!
//! `05_010_ProfileSelect` lists only the character slots that exist. The game builds its row list
//! in `FUN_140875590` (1.16.2, byte-identical in `eldenring-deobf.bin` at the same VA) by walking
//! slots `0..10` and pushing a `CS::MenuSaveDataSummary` **only** where
//! `ProfileSummary->saveSlotsStates[slot]` is set:
//!
//! ```text
//!   iVar5 = 0;
//!   do {
//!     if (FUN_140261cd0(GetProfileSummary(), iVar5))   // saveSlotsStates[slot]
//!       push(FUN_1408752c0(&tmp, iVar5));              // row remembers its slot at +8
//!     iVar5 = iVar5 + 1;
//!   } while (iVar5 < 10);
//! ```
//!
//! So the row at list index `i` describes slot `rows[i]`, and `i == rows[i]` only when the
//! container's characters happen to run densely from slot 0. `CS::ProfileLoadDialog::load_activate`
//! (`0x1409a4670`) never confuses the two -- it reads the cursor, clamps it, fetches the row and
//! takes the slot from the row:
//!
//! ```text
//!   1409a46b9: call 0x140739e20            ; cursor = [dialog+0xa38+0xd4] == [dialog+0xb0c]
//!   1409a46c0: test eax,eax / js  ...      ; cursor < 0        -> row 0
//!   1409a46c4: cmp  eax,[rdi+0xb08] / jg   ; cursor >= bound   -> row 0
//!   1409a46d5: call qword [rax+0x90]       ; list = dialog->vt[+0x90](dialog)
//!   ...        call qword [r8 +0x20]       ; row  = list->vt[+0x20](list, row_index)
//!   ...        mov  eax,[rax+8]            ; slot = row->save_slot
//! ```
//!
//! This module owns the two pure decisions that sit either side of those native calls, so they are
//! `cargo test`-able instead of only observable in game:
//!
//! * [`profile_select_row_for_cursor`] -- the exact clamp above, so a caller asks the native row
//!   accessor for the same row the native activation would have used;
//! * [`preview_cursor_slot`] -- after a foreign save is previewed, which slot the cursor should be
//!   parked on.
//!
//! # The bug this closes
//!
//! `system_quit_profile_load_activate_hook` fed the raw cursor to `profile_slot_has_character` and
//! to the foreign-preview commit as though it were a slot. A save whose only character sits in slot
//! 3 previews as a one-row list, the user presses A on row 0, and the mod asked "does slot 0 hold a
//! character" -- it does not, so the pick was refused with `slot holds no character`, six times in a
//! row, while the native `load_activate` in the very same frame built the load job for slot 3
//! (`loadgame-builder: 0x140826510 built for slot=3`, measured 2026-08-25). Any container whose
//! characters are not dense from slot 0 was unloadable through the menu.

/// Character slots in one `ER0000.{sl2,co2}` container, and rows in a full ProfileSelect list.
pub const PROFILE_SLOT_COUNT: usize = 10;

/// The row index `CS::ProfileLoadDialog::load_activate` would read for `cursor`, given the dialog's
/// row count `bound` (`[dialog+0xb08]`).
///
/// Mirrors the native clamp exactly: a negative cursor or one at/past `bound` falls back to row 0.
/// `None` only for an empty list, where there is no row to activate at all.
#[must_use]
pub fn profile_select_row_for_cursor(cursor: i32, bound: i32) -> Option<i32> {
    if bound <= 0 {
        return None;
    }
    if cursor < 0 || cursor >= bound {
        return Some(0);
    }
    Some(cursor)
}

/// The slot a freshly previewed foreign save should park the ProfileSelect cursor on: the lowest
/// occupant of `slot_mask`.
///
/// `slot_mask` is the preview's own bitmap of the slots it wrote into `CS::ProfileSummary` (bit N =
/// slot N), so an unset bit is a slot the previewed save cannot load. `None` means the save
/// produced no rows, and a caller must not move the cursor at all.
#[must_use]
pub fn preview_cursor_slot(slot_mask: u32) -> Option<i32> {
    (0..PROFILE_SLOT_COUNT as i32).find(|slot| slot_mask & (1u32 << slot) != 0)
}

#[cfg(test)]
mod profile_row_tests {
    use super::*;

    #[test]
    fn a_single_occupant_in_slot_zero_parks_the_cursor_on_slot_zero() {
        assert_eq!(preview_cursor_slot(0x1), Some(0));
    }

    /// The reported bug's own save: `~/Downloads/ER0000.co2` previewed with `slot_mask=0x8`, and
    /// nothing moved the cursor off row 0 -- which resolved to slot 0, which held no character.
    #[test]
    fn a_single_occupant_in_slot_three_parks_the_cursor_on_slot_three() {
        assert_eq!(preview_cursor_slot(0x8), Some(3));
    }

    #[test]
    fn no_occupants_at_all_parks_the_cursor_nowhere() {
        assert_eq!(preview_cursor_slot(0x0), None);
    }

    #[test]
    fn several_occupants_park_the_cursor_on_the_lowest() {
        assert_eq!(preview_cursor_slot(0b00_1010_0100), Some(2));
        assert_eq!(preview_cursor_slot(0x3ff), Some(0));
        assert_eq!(preview_cursor_slot(1 << 9), Some(9));
    }

    /// Bits above the ten real slots are not slots. A caller that read a wider mask must not be
    /// handed an eleventh slot to write into `CS::ProfileSummary`.
    #[test]
    fn bits_past_the_ten_real_slots_are_not_occupants() {
        assert_eq!(preview_cursor_slot(1 << 10), None);
        assert_eq!(preview_cursor_slot(0xffff_fc00), None);
        assert_eq!(preview_cursor_slot((1 << 10) | (1 << 4)), Some(4));
    }

    #[test]
    fn an_in_range_cursor_is_its_own_row() {
        assert_eq!(profile_select_row_for_cursor(0, 1), Some(0));
        assert_eq!(profile_select_row_for_cursor(1, 2), Some(1));
        assert_eq!(profile_select_row_for_cursor(9, 10), Some(9));
    }

    /// Both native out-of-range branches (`js` on negative, `jg` on `cursor >= bound`) land on
    /// row 0, so a caller resolving the row must land there too rather than refusing.
    #[test]
    fn an_out_of_range_cursor_falls_back_to_the_first_row() {
        assert_eq!(profile_select_row_for_cursor(-1, 2), Some(0));
        assert_eq!(profile_select_row_for_cursor(2, 2), Some(0));
        assert_eq!(profile_select_row_for_cursor(i32::MAX, 3), Some(0));
        assert_eq!(profile_select_row_for_cursor(i32::MIN, 3), Some(0));
    }

    #[test]
    fn an_empty_list_has_no_row_to_activate() {
        assert_eq!(profile_select_row_for_cursor(0, 0), None);
        assert_eq!(profile_select_row_for_cursor(3, -1), None);
    }
}
