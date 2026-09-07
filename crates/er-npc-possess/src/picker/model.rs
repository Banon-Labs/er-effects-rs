//! Where the cursor is, and what a press moves it to. No game, no catalogue, no drawing.
//!
//! # 408 rows through however many fit
//!
//! The list is long and the input is four directions, so the whole design question is how few
//! presses it takes to reach an arbitrary row. Stepping one at a time is 204 presses on average
//! and nobody would use it. The answer here is a second axis on the same four buttons: up and
//! down step one row, left and right jump to the previous or next initial. The catalogue is
//! sorted by the name on screen, so "jump by initial" is the index a reader already has in their
//! head -- to reach Runebear you press right until the R rows appear and then step.
//!
//! Measured over the shipped catalogue: 24 distinct initials, the largest of them holding 60
//! creatures, so the worst case is 24 group jumps plus 60 steps and the typical one is a small
//! fraction of that. The tests below guard LOOSER bounds than those two numbers on purpose -- a
//! regenerated name table that shifts them by a few should not go red for nothing, but one that
//! collapses the alphabet into three groups should. It is also why the group axis is by name
//! rather than by chr-id band: the `c4xxx` band alone holds 137 creatures with nothing to aim at
//! inside it.
//!
//! # One variable, so the window cannot desync from the cursor
//!
//! A list widget usually stores a cursor and a scroll offset, and then has to keep them
//! consistent on every operation -- which is where list widgets go wrong. Here the window is
//! DERIVED: it is centred on the cursor and clamped to the ends. There is no second variable to
//! get out of step, every operation is "move the cursor", and [`Window`] is a pure function of
//! the cursor, the length and how many rows fit.
//!
//! The visible cost is that the list slides under a stationary highlight rather than the
//! highlight moving down a stationary list. For a picker that is the better behaviour anyway: the
//! thing you are choosing stays where your eye already is.

// Pure integer logic. Ungated so `cargo test` proves the wrap, the clamp and the group jumps on
// the host.
#![cfg_attr(not(windows), allow(dead_code))]

/// What one press asks the list to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Nav {
    Up,
    Down,
    /// To the first row of the previous initial, or to the first row of this one when the cursor
    /// is not already there. That second case is what makes a single press useful in the middle
    /// of a long group.
    PrevGroup,
    NextGroup,
}

/// The slice of the catalogue a draw should paint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Window {
    /// First visible row.
    pub(crate) top: usize,
    /// How many rows are visible. Less than `visible` only when the catalogue is shorter.
    pub(crate) count: usize,
}

/// The cursor, and nothing else.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PickerModel {
    cursor: usize,
}

impl PickerModel {
    /// Start on `cursor`, clamped into the list. Opening on whatever is already staged is the
    /// whole reason this takes a starting index rather than always beginning at row 0.
    pub(crate) fn at(cursor: usize, len: usize) -> Self {
        Self {
            cursor: if len == 0 { 0 } else { cursor.min(len - 1) },
        }
    }

    pub(crate) const fn cursor(self) -> usize {
        self.cursor
    }

    /// Apply one press. `groups` is one group key per row -- see
    /// [`crate::picker::catalog::groups`] -- and its length is the length of the list.
    ///
    /// Returns whether the cursor actually moved, so a press that changes nothing produces no log
    /// line and no redraw-worthy event.
    pub(crate) fn nav(&mut self, groups: &[u8], nav: Nav) -> bool {
        if groups.is_empty() {
            return false;
        }
        let before = self.cursor;
        // A cursor that outran the list -- the catalogue cannot change under a live picker today,
        // but a clamp here is one line and the alternative is an index panic in a draw.
        self.cursor = self.cursor.min(groups.len() - 1);
        self.cursor = match nav {
            // Wrapping, both ends. Every list in the game wraps, and the alternative is a cursor
            // that silently refuses a press at row 0.
            Nav::Up => {
                if self.cursor == 0 {
                    groups.len() - 1
                } else {
                    self.cursor - 1
                }
            }
            Nav::Down => {
                if self.cursor + 1 == groups.len() {
                    0
                } else {
                    self.cursor + 1
                }
            }
            Nav::PrevGroup => prev_group(groups, self.cursor),
            Nav::NextGroup => next_group(groups, self.cursor),
        };
        self.cursor != before
    }

    /// The rows to paint, centred on the cursor and clamped to the ends.
    pub(crate) const fn window(self, len: usize, visible: usize) -> Window {
        if len == 0 || visible == 0 {
            return Window { top: 0, count: 0 };
        }
        if len <= visible {
            return Window { top: 0, count: len };
        }
        let half = visible / 2;
        let top = if self.cursor < half {
            0
        } else if self.cursor - half > len - visible {
            len - visible
        } else {
            self.cursor - half
        };
        Window {
            top,
            count: visible,
        }
    }
}

/// The first row of the group `index` belongs to.
fn group_start(groups: &[u8], index: usize) -> usize {
    let key = groups[index];
    let mut start = index;
    while start > 0 && groups[start - 1] == key {
        start -= 1;
    }
    start
}

/// First row of the next group, wrapping to row 0 past the end.
fn next_group(groups: &[u8], index: usize) -> usize {
    let key = groups[index];
    for (offset, candidate) in groups.iter().enumerate().skip(index + 1) {
        if *candidate != key {
            return offset;
        }
    }
    0
}

/// First row of the previous group -- or the first row of this group when the cursor is somewhere
/// in the middle of it.
fn prev_group(groups: &[u8], index: usize) -> usize {
    let start = group_start(groups, index);
    if start != index {
        return start;
    }
    if start == 0 {
        // Wrapping backwards off the top lands on the last group, not on the last ROW: pressing
        // left from the top of the alphabet should show you the end of it, not one row of it.
        return group_start(groups, groups.len() - 1);
    }
    group_start(groups, start - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker::catalog;

    /// `AABBBC` -- three groups of different sizes, which is enough shape to catch an off-by-one
    /// at either edge of a group.
    const SHAPE: &[u8] = b"AABBBC";

    #[test]
    fn stepping_wraps_at_both_ends() {
        let mut model = PickerModel::at(0, SHAPE.len());
        assert!(model.nav(SHAPE, Nav::Up));
        assert_eq!(model.cursor(), SHAPE.len() - 1, "up from the top wraps");
        assert!(model.nav(SHAPE, Nav::Down));
        assert_eq!(model.cursor(), 0, "down from the bottom wraps");
    }

    #[test]
    fn stepping_moves_exactly_one_row() {
        let mut model = PickerModel::at(2, SHAPE.len());
        assert!(model.nav(SHAPE, Nav::Down));
        assert_eq!(model.cursor(), 3);
        assert!(model.nav(SHAPE, Nav::Up));
        assert_eq!(model.cursor(), 2);
    }

    #[test]
    fn next_group_lands_on_the_first_row_of_the_next_initial() {
        let mut model = PickerModel::at(0, SHAPE.len());
        assert!(model.nav(SHAPE, Nav::NextGroup));
        assert_eq!(model.cursor(), 2, "A -> B");
        assert!(model.nav(SHAPE, Nav::NextGroup));
        assert_eq!(model.cursor(), 5, "B -> C");
        assert!(model.nav(SHAPE, Nav::NextGroup));
        assert_eq!(model.cursor(), 0, "C wraps to the top");
    }

    /// The half-press that makes the group axis usable in the middle of a long run.
    #[test]
    fn prev_group_from_inside_a_group_goes_to_that_groups_first_row() {
        let mut model = PickerModel::at(4, SHAPE.len());
        assert!(model.nav(SHAPE, Nav::PrevGroup));
        assert_eq!(model.cursor(), 2, "mid-B -> top of B");
        assert!(model.nav(SHAPE, Nav::PrevGroup));
        assert_eq!(model.cursor(), 0, "top of B -> top of A");
    }

    #[test]
    fn prev_group_from_the_first_row_wraps_to_the_start_of_the_last_group() {
        let mut model = PickerModel::at(0, SHAPE.len());
        assert!(model.nav(SHAPE, Nav::PrevGroup));
        assert_eq!(model.cursor(), 5, "the start of C, not the last row");
    }

    #[test]
    fn a_single_group_absorbs_every_group_press_without_moving() {
        let flat = b"AAAA";
        let mut model = PickerModel::at(0, flat.len());
        assert!(!model.nav(flat, Nav::NextGroup), "nowhere to go");
        assert_eq!(model.cursor(), 0);
        assert!(!model.nav(flat, Nav::PrevGroup));
        assert_eq!(model.cursor(), 0);
    }

    #[test]
    fn an_empty_list_absorbs_every_press() {
        let mut model = PickerModel::default();
        for nav in [Nav::Up, Nav::Down, Nav::PrevGroup, Nav::NextGroup] {
            assert!(!model.nav(&[], nav));
            assert_eq!(model.cursor(), 0);
        }
    }

    #[test]
    fn the_window_is_the_whole_list_when_it_fits() {
        let model = PickerModel::at(3, 6);
        assert_eq!(model.window(6, 10), Window { top: 0, count: 6 });
    }

    #[test]
    fn the_window_centres_on_the_cursor_and_clamps_at_both_ends() {
        let len = 100;
        let visible = 11;
        assert_eq!(
            PickerModel::at(0, len).window(len, visible),
            Window { top: 0, count: 11 },
            "at the top the cursor is not centred, because there is nothing above it"
        );
        assert_eq!(
            PickerModel::at(50, len).window(len, visible),
            Window { top: 45, count: 11 }
        );
        assert_eq!(
            PickerModel::at(99, len).window(len, visible),
            Window { top: 89, count: 11 },
            "at the bottom the window stops rather than running off the end"
        );
    }

    /// The invariant a derived window exists to guarantee: whatever the cursor, it is on screen.
    #[test]
    fn the_cursor_is_always_inside_the_window() {
        for len in [1usize, 2, 7, 10, 11, 408] {
            for visible in [1usize, 2, 10, 15, 500] {
                for cursor in 0..len {
                    let window = PickerModel::at(cursor, len).window(len, visible);
                    assert!(
                        cursor >= window.top && cursor < window.top + window.count,
                        "len={len} visible={visible} cursor={cursor} window={window:?}"
                    );
                    assert!(window.top + window.count <= len);
                }
            }
        }
    }

    #[test]
    fn a_zero_height_window_paints_nothing_rather_than_panicking() {
        assert_eq!(
            PickerModel::at(3, 10).window(10, 0),
            Window { top: 0, count: 0 }
        );
        assert_eq!(
            PickerModel::at(0, 0).window(0, 10),
            Window { top: 0, count: 0 }
        );
    }

    /// The claim in the module docs, measured against the catalogue that actually ships rather
    /// than asserted. If a future name table produces one enormous group, this goes red and the
    /// design has to change instead of the picker quietly becoming unusable.
    #[test]
    fn every_creature_is_reachable_in_a_bounded_number_of_presses() {
        let groups = catalog::groups();
        let distinct: std::collections::BTreeSet<u8> = groups.iter().copied().collect();
        assert!(
            distinct.len() <= 40,
            "{} groups is more than a player will page through",
            distinct.len()
        );
        let mut longest = 0usize;
        let mut run = 0usize;
        let mut previous = None;
        for key in groups {
            if Some(key) == previous {
                run += 1;
            } else {
                run = 1;
                previous = Some(key);
            }
            longest = longest.max(run);
        }
        assert!(
            longest <= 80,
            "the largest initial holds {longest} creatures, which is too many to step through"
        );
    }

    /// Walking the group axis forward must visit every group and come back to the start, or the
    /// jump is not an index -- it is a cycle with rows outside it that no press can reach.
    #[test]
    fn walking_the_group_axis_visits_every_group_of_the_shipped_catalogue() {
        let groups = catalog::groups();
        let distinct: std::collections::BTreeSet<u8> = groups.iter().copied().collect();
        let mut model = PickerModel::at(0, groups.len());
        let mut seen = std::collections::BTreeSet::new();
        // Exactly one jump per group: the last one is what wraps back to row 0, so an extra
        // iteration would leave the cursor on group two and prove nothing about the wrap.
        for _ in 0..distinct.len() {
            seen.insert(groups[model.cursor()]);
            model.nav(groups, Nav::NextGroup);
        }
        assert_eq!(seen, distinct);
        assert_eq!(model.cursor(), 0, "the walk returns to the top");
    }

    #[test]
    fn opening_at_a_row_past_the_end_clamps_rather_than_panicking() {
        assert_eq!(PickerModel::at(999, 10).cursor(), 9);
        assert_eq!(PickerModel::at(999, 0).cursor(), 0);
    }
}
