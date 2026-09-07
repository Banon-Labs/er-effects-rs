//! The arithmetic, with no game in the room.
//!
//! Eight values come out of the possessed creature's `CSChrDataModule` and eight go into the
//! HUD's `FrontEndViewValues`. Six of those are a straight copy; two are computed, and both
//! computations are transcribed from the game's own instructions rather than invented here. That
//! transcription is the whole of this module, which is why it is a pure function over a plain
//! struct and is tested on the host.
//!
//! # The two computed fields, and where they come from
//!
//! `UpdatePlayerComponents` does not read `maxRecoverableHp` or `hpMax` out of the data module --
//! it calls for them. Both callees are two-instruction leaves, byte-identical in 1.16.2 and 1.17,
//! so this crate performs the loads instead of resolving two more game addresses:
//!
//! | callee | 1.16.2 | 1.17 | whole body |
//! |---|---|---|---|
//! | `GetMaxRecoverableHp` | `0x140437280` | `0x1404377e0` | `cvttss2si eax,[rcx+0x160]` ; `add eax,[rcx+0x138]` ; `ret` |
//! | `GethpMaxUncapped` | `0x1404372c0` | `0x140437820` | `mov eax,[rcx+0x140]` ; `ret` |
//!
//! That keeps the mod's address budget where layer 3 left it: one resolved function, and it is
//! not one of these.
//!
//! # `hpMax` is a deficit
//!
//! The single most misreadable thing here. The game computes the view's `hpMax` as
//! `GethpMaxUncapped(data) - data.hpMax`, i.e. **uncapped minus capped** -- how much maximum HP
//! has been taken away, which the HUD draws as the darkened tail of the bar. The bar's width is
//! `hpMaxUncapped`. On a character with no reduction the two are equal and the field is zero.
//! For a possessed creature they are equal too, because `FUN_140438a50` writes both from the same
//! `NpcParam.hp`, so a boss shows a full undarkened bar. Reproducing the subtraction faithfully is
//! what makes that come out right; "fixing" it to report the max would darken the whole bar.
//!
//! # Creatures have stamina and do not have FP
//!
//! Measured against the shipped 1.17 `regulation.bin`, all 7,045 `NpcParam` rows:
//!
//! * `stamina` (`NpcParam+0xf4`) is **> 1 on 7,043 of them** -- 50 for most bosses, 100 for
//!   Gideon, 150 for the Grafted Scion. The two exceptions are degenerate rows with `hp == 0`.
//!   The stamina bar is therefore real, and it MOVES: `FUN_1404016d0` tops stamina up every frame
//!   from the character's own `GetStaminaRecoverySpeed`, and `EnemyIns::Update` calls it with no
//!   player gate at all.
//! * `mp` (`NpcParam+0x28`, the source of `fpMax`) is **0 on 6,989 of them** and on every boss
//!   checked. The 56 rows with any FP are almost all merchants. So the FP bar has nothing to show
//!   for essentially anything worth possessing.
//!
//! Note what that corrects: `CSChrDataModule`'s constructor defaults these to **1**, but
//! `FUN_140438a50` OVERWRITES that with the param, so the real reading for a creature is `0`, not
//! `1`. [`EMPTY_POOL_MAX`] is the threshold rather than an equality test precisely so both the
//! constructor default and the param zero are caught by one rule.
//!
//! # What an empty pool is turned into, and why not zero
//!
//! An empty pool is rendered as **`0` current out of a max clamped up to 1** -- an empty bar.
//! The two rejected alternatives:
//!
//! * Passing `1/1` through unchanged draws a full one-point bar, which reads as a bug rather than
//!   as an absence.
//! * Writing `0/0` is the honest value and is not safe to write. `UpdatePlayerComponents` only
//!   ever runs on the main player, whose `fpMax` is never zero, so every consumer downstream of
//!   this field is untested at zero -- and an integer `fp * width / fpMax` in any of them is a
//!   `#DE` rather than a wrong pixel. Nothing in this repo can rule that out without running the
//!   game, and a bar that looks empty is strictly better than a divide fault.

// Pure; ungated so `cargo test` proves it on the host with no game running.
#![cfg_attr(not(windows), allow(dead_code))]

/// At or below this, a pool is treated as absent rather than nearly empty.
///
/// One rather than zero because there are two ways to be empty and this catches both: the
/// `CSChrDataModule` constructor's default of `1`, and the `0` that `FUN_140438a50` writes from
/// `NpcParam.mp` for a creature that has no FP. A real pool is never this small -- the smallest
/// non-zero `mp` in the shipped param table is 50.
pub(crate) const EMPTY_POOL_MAX: i32 = 1;

/// The floor an empty pool's maximum is clamped to. See the module docs for why this is not 0.
const EMPTY_POOL_FLOOR: i32 = 1;

/// The eight fields read from the possessed creature's `CSChrDataModule`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Source {
    /// `+0x138 hp`.
    pub(crate) hp: i32,
    /// `+0x13c hpMax` -- the effective maximum, the capped one.
    pub(crate) hp_max: i32,
    /// `+0x140 hpMaxUncapped` -- the bar's width.
    pub(crate) hp_max_uncapped: i32,
    /// `+0x160 recoverableHpLeft`, the rally pool, the one float in the set.
    pub(crate) recoverable_hp: f32,
    /// `+0x148 fp`.
    pub(crate) fp: i32,
    /// `+0x14c fpMax`.
    pub(crate) fp_max: i32,
    /// `+0x154 stamina`.
    pub(crate) stamina: i32,
    /// `+0x158 staminaMax`.
    pub(crate) stamina_max: i32,
}

/// The eight ints written into `FrontEndViewValues`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct View {
    pub(crate) player_hp: i32,
    pub(crate) max_recoverable_hp: i32,
    /// The lost max, not the max. See the module docs.
    pub(crate) hp_max: i32,
    pub(crate) hp_max_uncapped: i32,
    pub(crate) fp: i32,
    pub(crate) fp_max: i32,
    pub(crate) stamina: i32,
    pub(crate) stamina_max: i32,
}

/// Whether a pool is worth drawing, which is a thing the derived report says out loud.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pool {
    /// The creature has a real pool; the bar shows it.
    Populated,
    /// `NpcParam` gave it nothing (or nothing but the constructor default); the bar is emptied.
    Empty,
}

impl Pool {
    /// Classify one maximum.
    #[must_use]
    pub(crate) const fn of(max: i32) -> Self {
        if max <= EMPTY_POOL_MAX {
            Self::Empty
        } else {
            Self::Populated
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Populated => "populated",
            Self::Empty => "empty",
        }
    }
}

impl Source {
    /// Turn the creature's vitals into the eight ints the HUD reads.
    #[must_use]
    pub(crate) fn view(&self) -> View {
        let (fp, fp_max) = Self::pool(self.fp, self.fp_max);
        let (stamina, stamina_max) = Self::pool(self.stamina, self.stamina_max);
        View {
            player_hp: self.hp,
            max_recoverable_hp: self.max_recoverable_hp(),
            // `sub` on two i32s, exactly as the game does it. Wrapping rather than saturating,
            // because the instruction wraps: a saturating stand-in would diverge from the game on
            // exactly the inputs where the difference could be noticed, and this field's whole
            // job is to be what the game would have computed.
            hp_max: self.hp_max_uncapped.wrapping_sub(self.hp_max),
            hp_max_uncapped: self.hp_max_uncapped,
            fp,
            fp_max,
            stamina,
            stamina_max,
        }
    }

    /// `GetMaxRecoverableHp`, inlined: `cvttss2si(recoverableHpLeft) + hp`.
    ///
    /// `as i32` is the right transcription of `cvttss2si` for every value the game can produce
    /// here -- both truncate toward zero. They differ only on NaN and on magnitudes past
    /// `i32::MAX`, where the instruction yields the integer indefinite value and Rust saturates;
    /// a rally pool is a small positive float, so nothing in range distinguishes them.
    #[must_use]
    fn max_recoverable_hp(&self) -> i32 {
        let recoverable = self.recoverable_hp as i32;
        recoverable.wrapping_add(self.hp)
    }

    /// The empty-pool rule, applied to one `(current, max)` pair. See the module docs.
    #[must_use]
    const fn pool(current: i32, max: i32) -> (i32, i32) {
        match Pool::of(max) {
            Pool::Populated => (current, max),
            // Current zeroed so the bar reads empty; max floored so nothing downstream divides by
            // zero. `max` here is <= 1 by construction, so the floor only ever raises 0 to 1 and
            // leaves 1 alone -- it can never shrink a real pool.
            Pool::Empty => (0, EMPTY_POOL_FLOOR),
        }
    }

    /// Does this creature have an FP bar worth drawing?
    #[must_use]
    pub(crate) const fn fp_pool(&self) -> Pool {
        Pool::of(self.fp_max)
    }

    /// ...and a stamina bar. True for all but two shipped `NpcParam` rows.
    #[must_use]
    pub(crate) const fn stamina_pool(&self) -> Pool {
        Pool::of(self.stamina_max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Margit-shaped creature, from the real param row `21300014` (hp 2521, mp 0, stamina 50).
    fn a_boss() -> Source {
        Source {
            hp: 2100,
            hp_max: 2521,
            hp_max_uncapped: 2521,
            recoverable_hp: 0.0,
            fp: 0,
            fp_max: 0,
            stamina: 40,
            stamina_max: 50,
        }
    }

    /// The straight copies, and that possessing a boss puts its numbers on the bar.
    #[test]
    fn a_boss_drives_the_hp_and_stamina_bars_with_its_own_numbers() {
        let view = a_boss().view();
        assert_eq!(view.player_hp, 2100);
        assert_eq!(
            view.hp_max_uncapped, 2521,
            "the bar is the creature's width"
        );
        assert_eq!(view.stamina, 40);
        assert_eq!(view.stamina_max, 50);
    }

    /// The deficit. Equal capped and uncapped maxima must give zero, i.e. no darkened tail --
    /// which is the case for every creature, because both come from `NpcParam.hp`.
    #[test]
    fn hp_max_is_the_lost_maximum_and_is_zero_when_nothing_is_lost() {
        assert_eq!(a_boss().view().hp_max, 0);
        // ...and it reports the shortfall when there is one, the way a cursed player's bar does.
        let cursed = Source {
            hp_max: 1800,
            hp_max_uncapped: 2521,
            ..a_boss()
        };
        assert_eq!(cursed.view().hp_max, 721);
        assert_eq!(
            cursed.view().hp_max_uncapped,
            2521,
            "the width does not shrink; the dark segment grows"
        );
    }

    /// The rally end is `trunc(recoverableHpLeft) + hp`, transcribing `GetMaxRecoverableHp`.
    #[test]
    fn the_rally_end_is_the_truncated_pool_added_to_current_hp() {
        let rallying = Source {
            hp: 500,
            recoverable_hp: 123.9,
            ..a_boss()
        };
        assert_eq!(
            rallying.view().max_recoverable_hp,
            623,
            "truncates, not rounds"
        );
        // Nothing to rally: the segment collapses onto current HP rather than sitting behind it.
        assert_eq!(a_boss().view().max_recoverable_hp, 2100);
    }

    /// The empty-pool rule, on the value the params actually carry. `mp` is 0 for every boss, so
    /// this is the common case rather than an edge case.
    #[test]
    fn an_fp_pool_of_zero_becomes_an_empty_bar_and_never_a_zero_maximum() {
        let view = a_boss().view();
        assert_eq!(view.fp, 0);
        assert_eq!(
            view.fp_max, 1,
            "the maximum is floored to 1; a zero maximum is what could divide-fault"
        );
        assert_eq!(a_boss().fp_pool(), Pool::Empty);
    }

    /// The other way to be empty: the constructor default of 1, which would otherwise draw a
    /// full one-point bar. Both routes must land on the same emptied result.
    #[test]
    fn a_pool_left_at_the_constructor_default_of_one_is_also_emptied() {
        let defaulted = Source {
            fp: 1,
            fp_max: 1,
            stamina: 1,
            stamina_max: 1,
            ..a_boss()
        };
        let view = defaulted.view();
        assert_eq!((view.fp, view.fp_max), (0, 1), "not a full 1/1 bar");
        assert_eq!((view.stamina, view.stamina_max), (0, 1));
        assert_eq!(defaulted.fp_pool(), Pool::Empty);
        assert_eq!(defaulted.stamina_pool(), Pool::Empty);
    }

    /// The threshold must not eat a real pool. The smallest non-zero `mp` in the shipped table is
    /// 50, and the smallest `stamina` is well above 1, so anything at 2 or more passes through
    /// untouched.
    #[test]
    fn a_real_pool_passes_through_untouched() {
        let merchant = Source {
            fp: 30,
            fp_max: 50,
            ..a_boss()
        };
        let view = merchant.view();
        assert_eq!((view.fp, view.fp_max), (30, 50));
        assert_eq!(merchant.fp_pool(), Pool::Populated);
        // The boundary itself: 2 is the smallest populated maximum.
        assert_eq!(Pool::of(2), Pool::Populated);
        assert_eq!(Pool::of(EMPTY_POOL_MAX), Pool::Empty);
        assert_eq!(Pool::of(0), Pool::Empty);
        assert_eq!(Pool::of(-1), Pool::Empty, "a negative max is not a pool");
    }

    /// Stamina is populated for 7,043 of the 7,045 shipped rows, so the ordinary outcome is that
    /// the rule does not fire on it -- and the two degenerate rows still land somewhere sane.
    #[test]
    fn stamina_is_normally_populated_and_the_degenerate_rows_still_behave() {
        assert_eq!(a_boss().stamina_pool(), Pool::Populated);
        let degenerate = Source {
            hp: 0,
            hp_max: 0,
            hp_max_uncapped: 0,
            stamina: 0,
            stamina_max: 0,
            ..a_boss()
        };
        let view = degenerate.view();
        assert_eq!((view.stamina, view.stamina_max), (0, 1));
        assert_eq!(view.hp_max, 0, "no width, no deficit");
    }

    /// The arithmetic must not panic on absurd inputs. A despawning creature can be read
    /// mid-teardown, and a debug-build overflow panic inside a per-frame detour would take the
    /// game down for a value nobody was going to look at.
    #[test]
    fn extreme_values_wrap_rather_than_panic() {
        let nonsense = Source {
            hp: i32::MAX,
            hp_max: i32::MIN,
            hp_max_uncapped: i32::MAX,
            recoverable_hp: f32::MAX,
            fp: i32::MIN,
            fp_max: i32::MAX,
            stamina: i32::MIN,
            stamina_max: i32::MAX,
        };
        let view = nonsense.view();
        // No assertion about which wrapped value comes out -- only that producing it is not a
        // panic, which is the property the detour depends on.
        assert_eq!(view.hp_max_uncapped, i32::MAX);
        assert_eq!(view.fp_max, i32::MAX, "a huge max is still a real pool");
        // NaN truncates to 0 under `as`, so the rally end is just current HP.
        let nan = Source {
            recoverable_hp: f32::NAN,
            hp: 10,
            ..a_boss()
        };
        assert_eq!(nan.view().max_recoverable_hp, 10);
    }

    /// Every pool name is distinct, so the derived report cannot print the same word for both
    /// states.
    #[test]
    fn the_pool_states_have_distinct_names() {
        assert_ne!(Pool::Populated.name(), Pool::Empty.name());
    }
}
