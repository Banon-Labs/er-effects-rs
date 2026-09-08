//! The `[hud]` block of `er-npc-possess.derived.toml` -- what the retarget decided, and why.
//!
//! Same contract as [`crate::moveset::derived`]: this file is output, rewritten on every
//! possession, and corrections go in `er-npc-possess.toml` instead. It exists for one question a
//! player will otherwise have to guess at -- "why is my FP bar empty?" -- whose answer is not a
//! bug but a param value, and which is invisible from the chair.

// Pure text generation; ungated so `cargo test` proves it on the host.
#![cfg_attr(not(windows), allow(dead_code))]

use std::fmt::Write as _;

use crate::hud::vitals::{Pool, Source};

/// Why the retarget is not running, when it is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Off {
    /// `[hud] enabled = false`.
    Disabled,
    /// The running build is one nobody has measured the offsets on.
    UnmeasuredBuild,
    /// The detour was refused -- no verified address for this build, or MinHook declined.
    HookRefused,
    /// The creature's `CSChrDataModule` did not read back.
    Unreadable,
}

impl Off {
    const fn reason(self) -> &'static str {
        match self {
            Self::Disabled => "[hud] enabled = false in er-npc-possess.toml",
            Self::UnmeasuredBuild => {
                "this game build has no measured FrontEndViewValues/CSChrDataModule offsets, so \
                 the offsets were REFUSED rather than guessed"
            }
            Self::HookRefused => {
                "the CSFeManImp::UpdatePlayerComponents detour was refused for this build -- see \
                 the HOOK REFUSED line in er-npc-possess.log"
            }
            Self::Unreadable => "the creature's CSChrDataModule did not read back",
        }
    }
}

/// What the retarget did, for one possession.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Decision {
    /// Running, with the creature's vitals at the moment possession started.
    Driving(Source),
    /// Not running.
    Off(Off),
}

/// Render the `[hud]` block. Appended to the moveset report by the driver.
#[must_use]
pub(crate) fn render(chr_id: u32, decision: &Decision) -> String {
    let mut out = String::from(
        "\n# ---------------------------------------------------------------------------\n\
         # [hud] -- which character the HP / FP / stamina bars are reading.\n\
         #\n\
         # Runes, equipment, the great rune and the spell slots are NOT retargeted and keep\n\
         # showing the real player. That is deliberate: the creature has no PlayerGameData and\n\
         # no armament slots, so pointing those at it would empty them.\n",
    );
    match decision {
        Decision::Off(off) => {
            let _ = writeln!(out, "\n[hud]");
            let _ = writeln!(
                out,
                "# The bars are showing YOUR character, not c{chr_id:04}."
            );
            let _ = writeln!(out, "retargeted = false");
            let _ = writeln!(out, "reason = \"{}\"", off.reason());
        }
        Decision::Driving(source) => {
            let view = source.view();
            let _ = writeln!(out, "\n[hud]");
            let _ = writeln!(out, "retargeted = true");
            let _ = writeln!(
                out,
                "# as read from c{chr_id:04} when the possession started"
            );
            let _ = writeln!(out, "hp = \"{}/{}\"", view.player_hp, view.hp_max_uncapped);
            if view.hp_max != 0 {
                let _ = writeln!(
                    out,
                    "hp_max_lost = {}  # darkened tail of the bar",
                    view.hp_max
                );
            }
            let _ = writeln!(
                out,
                "fp = \"{}/{}\"  # {}",
                view.fp,
                view.fp_max,
                pool_note(source.fp_pool(), "mp", source.fp_max)
            );
            let _ = writeln!(
                out,
                "stamina = \"{}/{}\"  # {}",
                view.stamina,
                view.stamina_max,
                pool_note(source.stamina_pool(), "stamina", source.stamina_max)
            );
            if source.fp_pool() == Pool::Empty || source.stamina_pool() == Pool::Empty {
                out.push_str(
                    "# An emptied bar is not a failure to read the creature -- it is the value\n\
                     # NpcParam actually carries. Almost no creature has FP: `mp` is 0 on 6,989\n\
                     # of the 7,045 shipped rows and on every boss. The bar is drawn empty rather\n\
                     # than left at the 1/1 the module constructor defaults to, because a full\n\
                     # one-point bar reads as a bug.\n",
                );
            }
        }
    }
    out
}

/// The trailing comment on an `fp`/`stamina` line.
fn pool_note(pool: Pool, param_field: &str, raw_max: i32) -> String {
    match pool {
        Pool::Populated => format!("{}, NpcParam.{param_field} = {raw_max}", pool.name()),
        Pool::Empty => format!(
            "{}: NpcParam.{param_field} = {raw_max}, so the bar is drawn empty",
            pool.name()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The block must be a valid TOML table header plus keys, and must say which character the
    /// numbers came from.
    #[test]
    fn a_driving_decision_reports_the_creatures_own_numbers() {
        let out = render(2130, &Decision::Driving(a_boss()));
        assert!(out.contains("[hud]"), "{out}");
        assert!(out.contains("retargeted = true"), "{out}");
        assert!(out.contains("hp = \"2100/2521\""), "{out}");
        assert!(out.contains("stamina = \"40/50\""), "{out}");
        assert!(out.contains("c2130"), "{out}");
        // No darkened tail on a creature, so the line is omitted rather than printed as 0.
        assert!(!out.contains("hp_max_lost"), "{out}");
    }

    /// The FP explanation must appear, because it is the one number a player will read as broken.
    #[test]
    fn an_empty_fp_pool_explains_itself_with_the_param_value() {
        let out = render(2130, &Decision::Driving(a_boss()));
        assert!(out.contains("fp = \"0/1\""), "{out}");
        assert!(out.contains("NpcParam.mp = 0"), "{out}");
        assert!(out.contains("drawn empty"), "{out}");
        // Stamina is populated, so it must not carry the empty note.
        assert!(out.contains("stamina = \"40/50\""), "{out}");
        assert!(out.contains("NpcParam.stamina = 50"), "{out}");
    }

    /// A reduced maximum prints the deficit line, and only then.
    #[test]
    fn a_lost_maximum_is_reported_as_the_darkened_tail() {
        let cursed = Source {
            hp_max: 1800,
            ..a_boss()
        };
        let out = render(2130, &Decision::Driving(cursed));
        assert!(out.contains("hp_max_lost = 721"), "{out}");
    }

    /// Every off-reason renders, is distinct, and says the bars are showing the player.
    #[test]
    fn every_off_reason_is_distinct_and_says_the_bars_are_the_players() {
        let mut seen = Vec::new();
        for off in [
            Off::Disabled,
            Off::UnmeasuredBuild,
            Off::HookRefused,
            Off::Unreadable,
        ] {
            let out = render(4500, &Decision::Off(off));
            assert!(out.contains("retargeted = false"), "{out}");
            assert!(out.contains("showing YOUR character"), "{out}");
            assert!(out.contains(off.reason()), "{out}");
            seen.push(off.reason());
        }
        let mut unique = seen.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), seen.len(), "{seen:?}");
    }

    /// The block is APPENDED to the moveset report, so it must open with a separator and never
    /// re-open a table the moveset file already opened.
    #[test]
    fn the_block_starts_a_fresh_section_so_it_can_be_appended() {
        let out = render(4500, &Decision::Off(Off::Disabled));
        assert!(out.starts_with('\n'), "{out:?}");
        // The `[hud]` header must come after the comment banner, so appending it under a
        // `[chr.cNNNN]` table cannot smuggle keys into that table.
        let header = out.find("[hud]").expect("has a header");
        let first_key = out.find("retargeted").expect("has a key");
        assert!(header < first_key, "{out}");
    }
}
