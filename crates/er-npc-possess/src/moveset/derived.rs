//! `er-npc-possess.derived.toml` -- what the mod decided, and what it refused.
//!
//! # Why a whole second file
//!
//! A moveset layer that quietly drops half of a boss's attacks is indistinguishable, from the
//! player's chair, from one that is broken. Both look like "I press the button and nothing
//! interesting happens". So every animation the generator looked at appears here: the ones on
//! offer with their bucket, rank and reach, and the ones withheld with the reason they were
//! withheld. If a favourite attack is missing, this file says whether it was denied, and for what.
//!
//! # It is output, not input
//!
//! Nothing reads it back. Corrections go in `er-npc-possess.toml` under `[chr.cNNNN]`, where
//! `unusable = [...]`, `usable = [...]` and `pin = { r2 = 3046 }` are already parsed. That split
//! is deliberate: this file is rewritten on every possession, so an edit here would be lost the
//! next time the hotkey was pressed, and a config file that silently discards your edits is worse
//! than no config file. The header says so in the file itself, where somebody about to type into
//! it will actually see it.

// Pure text generation; ungated so `cargo test` proves it on the host.
#![cfg_attr(not(windows), allow(dead_code))]

use std::fmt::Write as _;

use crate::moveset::dispatch::{Dispatcher, Hand};
use crate::moveset::table::{Denial, Moveset, Throw};
use crate::settings::Bucket;

/// Rendered into the game directory beside `er-npc-possess.toml`.
pub(crate) fn render(chr_id: u32, moveset: &Moveset, summary: &str) -> String {
    let mut out = String::new();
    out.push_str(
        "# er-npc-possess.derived.toml -- WRITTEN BY THE MOD ON EVERY POSSESSION.\n\
         #\n\
         # EDITS HERE ARE LOST. This file is regenerated from scratch each time you possess\n\
         # something; it exists so you can see what the mod decided and why. To CHANGE a\n\
         # decision, put it in er-npc-possess.toml instead:\n\
         #\n\
         #   [chr.c4500]\n\
         #   unusable = [3013]        # never offer this animation\n\
         #   usable   = [3020]        # offer it anyway, despite the denial below\n\
         #   pin      = { r2 = 3012 } # this button always plays this animation\n\
         #\n\
         # Every animation the offline classifier considered is listed. Nothing is withheld\n\
         # without a reason printed next to it.\n\n",
    );
    let _ = writeln!(out, "[chr.c{chr_id:04}]");
    let _ = writeln!(out, "# {summary}");
    // Said in words, not left to be inferred from four zeroes in the counts above. A creature whose
    // whole moveset is `movement` answers every attack button with a walk clip, which from the
    // player's chair is indistinguishable from a mod that has stopped working -- and 25 of the 408
    // creatures in the shipped table really are like that (a Balloon Dummy has no attacks, and no
    // amount of fixing will give it any). The counts were already here and nobody read them as a
    // sentence; this is the sentence.
    if !moveset.moves.is_empty() && moveset.bucket(Bucket::Movement).count() == moveset.moves.len()
    {
        out.push_str(
            "# THIS CREATURE HAS NO ATTACK ANIMATIONS. Everything below is locomotion, so r1, r2\n\
             # and l1 have nothing of their own to fire -- with [mapping] unbound_inputs =\n\
             # \"promote\" they borrow from the movement list and play a step, and with \"deny\"\n\
             # they do nothing at all. Movement, the camera, the HUD and release still work.\n",
        );
    }
    if moveset.moves.is_empty() && moveset.denials.is_empty() {
        out.push_str(
            "# Nothing at all: this creature is not in the shipped table, or the table found\n\
             # nothing fireable on it. Variants that own a model but no animations of their own\n\
             # land here.\n",
        );
        return out;
    }

    for bucket in [
        Bucket::Light,
        Bucket::Heavy,
        Bucket::Ranged,
        Bucket::Movement,
    ] {
        let mut entries: Vec<_> = moveset.bucket(bucket).collect();
        if entries.is_empty() {
            continue;
        }
        entries.sort_by_key(|entry| entry.rank);
        let _ = writeln!(
            out,
            "\n# {} -- in the order repeated presses walk",
            bucket.name()
        );
        for entry in entries {
            let plays = if entry.played == entry.fire {
                String::new()
            } else {
                format!(" plays {}", entry.played)
            };
            // A grab says who it needs. `ThrowParam.DefChrId` is matched exactly against the
            // victim's `ChrIns::npcId`, so "grab" on its own would be a half-truth: c2120's grab
            // works on the player and on nothing else in the game. The range is the row's `Dist`.
            let grab = if entry.grab() {
                let victims: Vec<String> = entry
                    .throws
                    .iter()
                    .map(|throw| {
                        if throw.victim_is_player() {
                            format!("the player within {:.1}m", throw.range_m())
                        } else {
                            format!("c{:04} within {:.1}m", throw.victim_chr, throw.range_m())
                        }
                    })
                    .collect();
                // And whether it can actually complete. A grab whose only victim is the player
                // is refused for as long as the possession lasts: the neuter sets
                // `chrFlags1c5 & 0x10` on the player's body and `IsImmuneToAttack` reads exactly
                // that bit before the hit that would start the throw ever reaches `ApplyDamage`.
                // Saying "throws the player" and stopping there would send somebody hunting for a
                // bug in their own config. A row that also names a creature is not refused, so
                // the note is only added when every row is the player's.
                let refused = if entry.throws.iter().all(Throw::victim_is_player) {
                    " -- REFUSED while you are possessing, because your own body is the only \
                     victim it has and possession makes it invincible; the swing still plays"
                } else {
                    ""
                };
                format!(" GRAB, throws {}{refused}", victims.join(" or "))
            } else {
                String::new()
            };
            // The prefix is printed only when it is not the field write, because that is when it
            // is worth knowing: the move is fired through PlayAnimationByBehaviorName rather than
            // by writing a field, and the name it is fired under is the one shown.
            let by = if entry.prefix.is_field_write() {
                String::new()
            } else {
                format!(" via {}{:04}", entry.prefix.name(), entry.fire)
            };
            // Whether a press during this move can chain, or has to wait it out. The distinction
            // is the difference between a combo and a pause, so it is said per move rather than
            // left for the player to infer from how it feels. `chains from` is the start of the
            // animation's own TimeAct cancel window; "no chain window" means the animation
            // authors none and a press during it waits for the animation to end.
            let chain = match entry.chain_from_s() {
                Some(from) => format!(" chains from {from:.2}s"),
                None => String::from(" no chain window -- a press during it waits"),
            };
            let _ = writeln!(
                out,
                "{} = \"{} rank {} reach {}{plays}{grab}{by},{chain}\"",
                entry.fire,
                bucket.name(),
                entry.rank,
                entry.reach.name(),
            );
        }
    }

    if !moveset.denials.is_empty() {
        out.push_str("\n# withheld, and why\n");
        for (animation, reason) in &moveset.denials {
            let _ = writeln!(out, "{animation} = \"denied: {}\"", reason.name());
        }
        out.push_str(&explanations(&moveset.denials));
    }
    out
}

/// The `[pages]` block: which attack set each hand is on, and what its two buttons lead with.
///
/// # Why the page needs a report at all
///
/// The page is the one piece of moveset state the player moves, and it is invisible: nothing on
/// screen says whether right arrow has been pressed three times or none. Every other decision in
/// this file was made offline and is fixed for the possession; this one changes under the player's
/// hand, so it is the one most worth being able to look up.
///
/// `leads with` is what the next press gives from a standing start. A press made mid-combo or at an
/// unusual distance can still land elsewhere -- the rank cursor walks on from the page, and the
/// `context` model filters by reach -- and the note below says so rather than letting the block
/// read as a promise.
pub(crate) fn pages_block(dispatcher: &Dispatcher) -> String {
    let mut out = String::from(
        "\n# ---------------------------------------------------------------------------\n\
         # [pages] -- the attack SET each hand is on.\n\
         #\n\
         # A creature has up to sixty attacks and four buttons. The page is a standing offset into\n\
         # each bucket, moved only by you: the RIGHT arrow pages r1/r2 and the LEFT arrow pages\n\
         # l1/l2, matching the two armament-swap keys vanilla puts there -- a possessed creature\n\
         # has no armaments, so both keys are otherwise dead. Rebind them with [buttons] page_left\n\
         # / page_right (and pad_page_left / pad_page_right).\n\
         #\n\
         # `leads with` is the move the next press gives from a standing start. Repeated presses\n\
         # still walk on from there within the combo window, and [mapping] model = \"context\"\n\
         # filters by reach, so a press at an odd distance can land on a different one.\n\n",
    );
    out.push_str("[pages]\n");
    for hand in Hand::ALL {
        let pages = dispatcher.pages(hand);
        let mut leads = Vec::new();
        for input in hand.inputs() {
            let bucket = input.bucket(dispatcher.buttons());
            match dispatcher.leads_with(input) {
                Some(entry) => leads.push(format!(
                    "{} leads with {} ({} rank {})",
                    input.name(),
                    entry.fire,
                    bucket.name(),
                    entry.rank
                )),
                None => leads.push(format!(
                    "{} has nothing ({} is empty on this creature)",
                    input.name(),
                    bucket.name()
                )),
            }
        }
        if pages <= 1 {
            let _ = writeln!(
                out,
                "{} = \"1/1 -- one set only, so the page key does nothing here: {}\"",
                hand.name(),
                leads.join(", ")
            );
        } else {
            let _ = writeln!(
                out,
                "{} = \"{}/{pages} -- {}\"",
                hand.name(),
                dispatcher.page(hand) + 1,
                leads.join(", ")
            );
        }
    }
    out
}

/// The same file, for a spawn that never produced a creature to report on.
///
/// # Why the refusal goes here rather than only in the log
///
/// `mode = "spawn"` is the one mode where the player picks a number out of thin air, and the number
/// they picked is the thing most likely to be wrong. A refusal that lives only in
/// `er-npc-possess.log` asks them to go and find a log; this file is already the place they are
/// told to look after a press, it is already regenerated on every press, and it is named in the
/// same breath as the config they need to edit. So a bad `chr_id` explains itself in the file
/// beside the file that set it.
///
/// It overwrites the previous possession's moveset report, which is correct: this file always
/// describes the most recent press, and the most recent press produced no moveset.
pub(crate) fn render_spawn_refusal(chr_id: u32, reason: &str) -> String {
    let mut out = String::new();
    out.push_str(
        "# er-npc-possess.derived.toml -- WRITTEN BY THE MOD ON EVERY POSSESSION.\n\
         #\n\
         # EDITS HERE ARE LOST. This file is regenerated from scratch each time you press the\n\
         # possess hotkey.\n\
         #\n\
         # THE LAST PRESS DID NOT PRODUCE A CREATURE. [target] mode = \"spawn\" asks the game to\n\
         # create the character named below, and this is what happened instead. Change the id in\n\
         # er-npc-possess.toml under [spawn] and press again.\n\
         #\n\
         # Any four-digit chr id is spawnable -- assets load on demand and nothing checks the id\n\
         # against the current map -- so an id that does not work is one with no chrbnd of its own,\n\
         # not one that is merely far away.\n\n",
    );
    let _ = writeln!(out, "[spawn.c{chr_id:04}]");
    let _ = writeln!(out, "refused = {reason:?}");
    out
}

/// One prose line per denial reason actually present, so the codes above mean something without
/// the player having to find this crate's source.
fn explanations(denials: &[(i32, Denial)]) -> String {
    let mut seen = Vec::new();
    for (_, reason) in denials {
        if !seen.contains(reason) {
            seen.push(*reason);
        }
    }
    seen.sort_unstable();
    let mut out = String::from("#\n");
    for reason in seen {
        let _ = writeln!(out, "#   {:<20} {}", reason.name(), describe(reason));
    }
    out
}

const fn describe(reason: Denial) -> &'static str {
    match reason {
        Denial::NotFireable => {
            "the behaviour graph names this event but no transition consumes it; firing it \
             would do nothing at all"
        }
        Denial::NoClip => "a transition consumes it, but the state it lands on plays no animation",
        Denial::NoDamageWindow => {
            "it plays, but its TimeAct opens no hitbox -- a flourish or an unused take"
        }
        Denial::MissingAtkRow => {
            "its attack row is absent from AtkParam_Npc, so it would swing through you"
        }
        Denial::SpEffectOnly => "everything it does is apply a status effect; it is not an attack",
        Denial::UnresolvedBehavior => {
            "its TimeAct names a behaviour id BehaviorParam has no row for"
        }
        Denial::UnusableAtRuntime => {
            "the watchdog saw this one leave the creature stuck with no way out"
        }
        Denial::ThrowResultClip => {
            "this is the clip the THROW system plays once a grab has landed, not one you can \
             fire -- the game reaches it by the bare name W_ThrowAtk and picks it from a \
             ThrowParam row. The grab itself is the attack marked GRAB above"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::moveset::table::parse_line;

    fn moveset(line: &str) -> Moveset {
        parse_line(line).expect("well-formed test line").1
    }

    fn dispatcher(line: &str) -> Dispatcher {
        Dispatcher::new(
            moveset(line),
            crate::settings::MappingSettings::default(),
            crate::settings::ButtonSettings::default(),
        )
    }

    /// The counts said `light=0 heavy=0 ranged=0 movement=12` and the player read it as nothing at
    /// all. This is the sentence that says it out loud, and the assertion that it is there.
    #[test]
    fn a_creature_with_only_locomotion_is_told_so_in_words() {
        let text = render(
            130,
            &moveset("130 6000:3:0:0:2 6001:3:1:0:2"),
            "moves=2 (light=0 heavy=0 ranged=0 movement=2) denied=0",
        );
        assert!(text.contains("HAS NO ATTACK ANIMATIONS"), "{text}");
        // ...and a creature that has attacks must not be told it has none.
        let armed = render(4500, &moveset("4500 3000:0:0:1 6000:3:0:0:2"), "summary");
        assert!(!armed.contains("HAS NO ATTACK ANIMATIONS"), "{armed}");
    }

    /// The page is the only piece of moveset state the player moves, and nothing on screen shows
    /// it. The report is where they look it up, so what it says has to be the truth about the live
    /// dispatcher rather than a fixed line of prose.
    #[test]
    fn the_pages_block_names_the_current_set_and_what_each_button_leads_with() {
        let mut engine = dispatcher("4500 3000:0:0:1 3001:0:1:1 3002:0:2:1 3010:1:0:1");
        let first = pages_block(&engine);
        assert!(first.contains("right = \"1/3"), "{first}");
        assert!(first.contains("r1 leads with 3000"), "{first}");
        engine.turn_page(Hand::Right);
        let second = pages_block(&engine);
        assert!(second.contains("right = \"2/3"), "{second}");
        assert!(second.contains("r1 leads with 3001"), "{second}");
        // The left hand has nothing here, so it must say the key does nothing rather than
        // offering a page turn that would not happen.
        assert!(second.contains("left = \"1/1"), "{second}");
        assert!(
            second.contains("the page key does nothing here"),
            "{second}"
        );
    }

    #[test]
    fn every_denial_is_printed_with_its_reason() {
        let text = render(
            4500,
            &moveset("4500 3000:0:0:1 !3013:3 !3014:1 !3025:4"),
            "summary",
        );
        assert!(
            text.contains("3013 = \"denied: no-damage-window\""),
            "{text}"
        );
        assert!(text.contains("3014 = \"denied: not-fireable\""), "{text}");
        assert!(
            text.contains("3025 = \"denied: missing-atk-row\""),
            "{text}"
        );
    }

    #[test]
    fn a_reason_that_appears_gets_an_explanation_and_one_that_does_not_stays_out() {
        let text = render(4500, &moveset("4500 3000:0:0:1 !3013:3"), "summary");
        assert!(text.contains("no-damage-window   "), "{text}");
        assert!(
            !text.contains("speffect-only  "),
            "an absent reason must not be explained: {text}"
        );
    }

    #[test]
    fn an_animation_that_plays_something_else_says_so() {
        let text = render(4500, &moveset("4500 3110=3000:2:0:3"), "summary");
        assert!(text.contains("plays 3000"), "{text}");
    }

    /// A grab has to say who it can be used on, because `ThrowParam.DefChrId` is an exact match
    /// and "grab" alone tells the player nothing they can act on.
    #[test]
    fn a_grab_is_labelled_with_the_victim_it_needs_and_the_range() {
        let text = render(2120, &moveset("2120 3022g0,100:0:0:1"), "summary");
        assert!(text.contains("GRAB"), "{text}");
        assert!(text.contains("the player within 10.0m"), "{text}");
    }

    #[test]
    fn a_grab_with_two_victims_lists_both() {
        let text = render(4280, &moveset("4280 3006g0,100+3300,55:0:0:1"), "summary");
        assert!(
            text.contains("the player within 10.0m or c3300 within 5.5m"),
            "{text}"
        );
    }

    /// A player-only grab cannot complete while the possession is running -- the neuter's
    /// invincibility bit is the same one `IsImmuneToAttack` reads, and it drops the hit before
    /// `ApplyDamage` can hand `throwTypeId` to the throw system. Saying "throws the player" and
    /// stopping there sends somebody hunting for a bug in their own config.
    #[test]
    fn a_player_only_grab_says_it_is_refused_while_possessing() {
        let text = render(2120, &moveset("2120 3022g0,100:0:0:1"), "summary");
        assert!(text.contains("REFUSED while you are possessing"), "{text}");
        assert!(text.contains("invincible"), "{text}");
    }

    /// ...but a grab that also names a creature victim is not refused, so it must not carry the
    /// note. Getting this backwards would tell the player their working grab is broken.
    #[test]
    fn a_grab_that_can_take_a_creature_victim_is_not_marked_refused() {
        let text = render(4280, &moveset("4280 3006g0,100+3300,55:0:0:1"), "summary");
        assert!(!text.contains("REFUSED"), "{text}");
    }

    /// The 4000-band clips are the thing players go looking for by name, so their denial has to
    /// explain the mechanism rather than say "not fireable" and leave it there.
    #[test]
    fn a_throw_result_clip_is_denied_with_an_explanation_of_the_throw_system() {
        let text = render(2120, &moveset("2120 3022g0,100:0:0:1 !4100:10"), "summary");
        assert!(
            text.contains("4100 = \"denied: throw-result-clip\""),
            "{text}"
        );
        assert!(text.contains("W_ThrowAtk"), "{text}");
    }

    #[test]
    fn the_header_says_edits_are_lost_and_names_the_file_that_is_read() {
        let text = render(4500, &moveset("4500 3000:0:0:1"), "summary");
        assert!(text.contains("EDITS HERE ARE LOST"));
        assert!(text.contains("er-npc-possess.toml"));
        assert!(text.contains("[chr.c4500]"));
    }

    /// A refused spawn has to name the id that was refused and the reason, and it has to still be
    /// a TOML file -- a player copying a block out of it into the real config gets a rejection they
    /// cannot explain otherwise.
    #[test]
    fn a_refused_spawn_names_the_id_and_the_reason_and_still_parses() {
        let text = render_spawn_refusal(
            9998,
            "the asset step machine never reached a loaded state after 5000 ms",
        );
        assert!(text.contains("[spawn.c9998]"), "{text}");
        assert!(text.contains("EDITS HERE ARE LOST"), "{text}");
        assert!(text.contains("DID NOT PRODUCE A CREATURE"), "{text}");
        let doc = crate::toml::Document::parse(&text);
        assert_eq!(
            doc.scalar("spawn.c9998", "refused"),
            Some("the asset step machine never reached a loaded state after 5000 ms")
        );
    }

    /// The refusal must not blame distance or the current map: any four-digit id is spawnable, and
    /// sending the player off to stand nearer something would be sending them after the wrong
    /// cause.
    #[test]
    fn the_refusal_says_residency_is_not_the_reason() {
        let text = render_spawn_refusal(4500, "whatever");
        assert!(text.contains("assets load on demand"), "{text}");
        assert!(text.contains("not one that is merely far away"), "{text}");
    }

    #[test]
    fn a_creature_with_nothing_gets_a_file_that_explains_the_nothing() {
        let text = render(3201, &Moveset::default(), "summary");
        assert!(text.contains("[chr.c3201]"));
        assert!(text.contains("Nothing at all"), "{text}");
    }

    /// The player has to be able to tell a move that chains from one that has to be waited out,
    /// because those feel like two different mods and only one of them is a combo.
    #[test]
    fn a_move_says_whether_it_has_a_real_chain_window_or_has_to_be_waited_out() {
        let text = render(4500, &moveset("4500 3000w947:0:0:1 3034:0:1:0"), "summary");
        assert!(text.contains("chains from 9.47s"), "{text}");
        assert!(
            text.contains("3034 = \"light rank 1 reach unknown, no chain window"),
            "{text}"
        );
    }

    #[test]
    fn buckets_are_printed_in_rank_order() {
        let text = render(
            4500,
            &moveset("4500 3002:0:2:1 3000:0:0:1 3001:0:1:1"),
            "summary",
        );
        let position = |needle: &str| text.find(needle).expect("present");
        assert!(position("3000 =") < position("3001 ="));
        assert!(position("3001 =") < position("3002 ="));
    }

    /// The file must survive being handed back to this crate's own TOML reader -- otherwise a
    /// player who copies a block out of it into the real config gets a rejection they cannot
    /// explain.
    #[test]
    fn the_rendered_file_parses_as_toml() {
        let text = render(
            4500,
            &moveset("4500 3000:0:0:1 3110=3000:2:0:3 !3013:3"),
            "moves=2 denied=1",
        );
        let doc = crate::toml::Document::parse(&text);
        assert_eq!(
            doc.scalar("chr.c4500", "3000"),
            Some("light rank 0 reach close, no chain window -- a press during it waits")
        );
        assert_eq!(
            doc.scalar("chr.c4500", "3013"),
            Some("denied: no-damage-window")
        );
    }
}
