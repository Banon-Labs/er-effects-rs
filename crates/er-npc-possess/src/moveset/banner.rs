//! What the four buttons do right now, on screen, while you are wearing something.
//!
//! # The defect this exists to close
//!
//! The page keys worked from the day they landed and nobody could tell. A live session on
//! 2026-09-02 logged
//!
//! ```text
//! moveset: right attack set 2/17 -- r1 3008, r2 3002
//! moveset: left attack set 2/2 -- l1 3110, l2 nothing
//! ```
//!
//! while the player, watching the screen, reported "I still don't see any abilities to switch
//! between". Both statements are true: the feature fired, and its entire output went to a text
//! file beside the game executable and to `er-npc-possess.derived.toml`, neither of which is
//! visible from inside a full-screen game.
//!
//! That is worse than a missing feature, because the page is a mode. Being on set 7 of 17 with
//! nothing on screen saying so makes every button press a guess, and the only way back to a known
//! state is to release and re-possess. A mode with no indicator is a mode you cannot use.
//!
//! # What is in here and what is not
//!
//! This module is the state and the text, and both are pure: [`Banner::from_dispatcher`] reads a
//! [`Dispatcher`] and produces the lines, and the host tests assert them without a game. The
//! drawing is [`crate::overlay`], which is windows-only and paints these same strings onto the
//! process's one imgui context.
//!
//! # The published snapshot, and why it is not rebuilt per frame
//!
//! The render thread runs on `Present` -- 60 to 144 times a second -- and the game thread owns the
//! [`Dispatcher`] inside the possession engine's lock. Reaching across that lock from the renderer
//! would put the game's frame behind the overlay's; instead the game thread publishes a [`Banner`]
//! whenever the content changes, which is exactly three moments:
//!
//! * a possession starts,
//! * a page key turns a page,
//! * the possession ends (cleared).
//!
//! [`Banner`] is `Copy` and heap-free precisely so that the per-frame read is a lock, a struct
//! copy and an unlock. [`showing`] is an atomic ahead of it, so a session that never possesses
//! anything never touches the mutex at all.

// Pure state and formatting over the dispatcher; no game memory. Ungated so `cargo test` proves
// the text on the host, where none of the game bindings exist.
#![cfg_attr(not(windows), allow(dead_code))]

use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::moveset::dispatch::{Dispatcher, Hand, Input};
use crate::moveset::table::Reach;
use crate::settings::{Bucket, ButtonSettings};

/// How many drawn frames a hand's header stays highlighted after its page key turned it.
///
/// The complaint this module answers is "I cannot see the change happen", so the highlight is the
/// half of the feature with a deadline: long enough to be noticed by someone whose eyes were on
/// the creature rather than on the corner, short enough that two taps in a row read as two
/// separate events rather than as one continuous glow.
///
/// Frames rather than a wall-clock duration, and not only because `scripts/check-no-timeouts.py`
/// bans `Instant::elapsed()` as a gate (it names frame counters as the replacement). A frame
/// count is the right clock for something whose whole existence is "was this on screen long
/// enough to be seen": it cannot burn down while the game is minimised, stalled on a load, or
/// otherwise not presenting, which is exactly when a wall-clock highlight would expire unseen.
/// The cost is that its duration tracks framerate -- 3.0 s at 60 fps, 1.25 s at 144 -- and both
/// ends of that range sit inside the window described above, so it is a cost rather than a fault.
pub(crate) const FLASH_FRAMES: u32 = 180;

/// What one button leads with on the current page.
///
/// `lead` is `None` when the bucket that button is bound to is empty on this creature -- a real
/// and common state (25 of the 408 creatures in the shipped table have no attack animations at
/// all), and the reason the panel draws `--` rather than omitting the row. A button with nothing
/// behind it is information; a missing line is a rendering bug the player then has to rule out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Lead {
    pub(crate) input: Input,
    pub(crate) bucket: Bucket,
    /// The animation id the next press fires from a standing start, or `None` for an empty bucket.
    pub(crate) lead: Option<i32>,
    pub(crate) reach: Reach,
    /// This attack's `AtkParam` row carries a `throwTypeId` a `ThrowParam` row completes -- landing
    /// it starts a grab. Rare (153 of 9426 shipped moves) and worth a tag when it is true.
    pub(crate) grab: bool,
}

impl Lead {
    /// `  r1  light     3000 close` -- one button, padded so the four line up as a column.
    ///
    /// The animation id is the identity and there is nothing better to print: the game has no name
    /// for an animation, and the id is exactly what the log line and `er-npc-possess.derived.toml`
    /// both use, so a player comparing the three is comparing the same token.
    pub(crate) fn line(&self) -> String {
        let mut out = format!("  {:<3} {:<9} ", self.input.name(), self.bucket.name());
        match self.lead {
            Some(fire) => {
                let _ = write!(out, "{fire}");
                // Reach is why a press can do nothing at the distance you are standing at, so it
                // earns its six characters. `Unknown` is the generator declining to measure, and
                // such a move is offered in every band -- printing "unknown" there would read as a
                // fault rather than as "no restriction", so it is left off.
                if self.reach != Reach::Unknown {
                    let _ = write!(out, " {}", self.reach.name());
                }
                if self.grab {
                    out.push_str(" grab");
                }
            }
            None => out.push_str("--"),
        }
        out
    }
}

/// One hand: the set it is on, out of how many, and what its two buttons do on that set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BannerHand {
    pub(crate) hand: Hand,
    /// 1-based, as the player is shown it and as the log prints it.
    pub(crate) page: u16,
    pub(crate) pages: u16,
    pub(crate) leads: [Lead; 2],
}

impl BannerHand {
    /// `R 2/17`, `R 1/1 (one set)`, or `R no attack sets` -- three states, three sentences.
    ///
    /// The last two are spelled out rather than left as a bare `1/1` and a blank, because a player
    /// who presses the page key and sees no change needs to be told which reason it is: the key is
    /// unbound, this creature has a single set, or it has no moveset at all. `1/1` alone says none
    /// of the three, and a missing line says nothing while looking like a rendering fault.
    pub(crate) fn header(&self) -> String {
        let initial = match self.hand {
            Hand::Right => 'R',
            Hand::Left => 'L',
        };
        // Only [`Banner::absent`] produces zero. `Dispatcher::pages` floors at one, so a real
        // creature -- even one whose every animation was denied -- never lands here.
        if self.pages == 0 {
            format!("{initial} no attack sets")
        } else if self.pages <= 1 {
            format!("{initial} 1/1 (one set)")
        } else {
            format!("{initial} {}/{}", self.page, self.pages)
        }
    }
}

/// Everything the on-screen panel draws, as of the last time the content changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Banner {
    pub(crate) chr_id: u32,
    /// The creature's name, or `""` when the catalogue does not name this id.
    pub(crate) name: &'static str,
    /// Carried so the footer can name the keys that page, live, rather than the shipped defaults.
    /// A player who rebound them needs the panel to say what they bound.
    pub(crate) buttons: ButtonSettings,
    pub(crate) hands: [BannerHand; 2],
    /// The hand whose page just turned, if this publish was a page turn.
    pub(crate) flash: Option<Hand>,
}

impl Banner {
    /// Read the dispatcher's current page state. Pure -- no globals, no game memory.
    pub(crate) fn from_dispatcher(
        chr_id: u32,
        name: &'static str,
        dispatcher: &Dispatcher,
        flash: Option<Hand>,
    ) -> Self {
        let buttons = dispatcher.buttons();
        let lead_of = |input: Input| {
            let entry = dispatcher.leads_with(input);
            Lead {
                input,
                bucket: input.bucket(buttons),
                lead: entry.map(|entry| entry.fire),
                reach: entry.map_or(Reach::Unknown, |entry| entry.reach),
                grab: entry.is_some_and(|entry| !entry.throws.is_empty()),
            }
        };
        let hand_of = |hand: Hand| {
            let inputs = hand.inputs();
            BannerHand {
                hand,
                page: dispatcher.page(hand) + 1,
                pages: dispatcher.pages(hand),
                leads: [lead_of(inputs[0]), lead_of(inputs[1])],
            }
        };
        Self {
            chr_id,
            name,
            buttons,
            hands: [hand_of(Hand::Right), hand_of(Hand::Left)],
            flash,
        }
    }

    /// The panel for a creature with no shipped moveset: four buttons and nothing behind any of
    /// them.
    ///
    /// Publishing this rather than publishing nothing is the whole point. `mode = "lock_on"` will
    /// happily wear a character the offline table has never heard of, and for that possession the
    /// four attack buttons genuinely do nothing -- but an absent panel is indistinguishable, from
    /// the player's chair, from a panel that failed to draw. That confusion is the defect this
    /// module exists to end, so it must not be reintroduced on the one creature where the answer
    /// is "there is nothing to show".
    pub(crate) fn absent(chr_id: u32, name: &'static str, buttons: ButtonSettings) -> Self {
        let lead_of = |input: Input| Lead {
            input,
            bucket: input.bucket(buttons),
            lead: None,
            reach: Reach::Unknown,
            grab: false,
        };
        let hand_of = |hand: Hand| {
            let inputs = hand.inputs();
            BannerHand {
                hand,
                page: 0,
                pages: 0,
                leads: [lead_of(inputs[0]), lead_of(inputs[1])],
            }
        };
        Self {
            chr_id,
            name,
            buttons,
            hands: [hand_of(Hand::Right), hand_of(Hand::Left)],
            flash: None,
        }
    }

    /// `c4500 Flying Dragon`, or just `c4500` for an id the catalogue does not name.
    pub(crate) fn title(&self) -> String {
        if self.name.is_empty() {
            format!("c{:04}", self.chr_id)
        } else {
            format!("c{:04} {}", self.chr_id, self.name)
        }
    }

    /// The keys that page, as they are bound right now, and only for hands that have a second
    /// set.
    ///
    /// `None` when neither hand can be paged on this creature. A footer naming the key that pages
    /// a set which does not exist is worse than no footer: it is an instruction that does not
    /// work, on the one creature where the headers already say `one set`.
    pub(crate) fn footer(&self) -> Option<String> {
        let mut parts = Vec::new();
        for hand in &self.hands {
            if hand.pages <= 1 {
                continue;
            }
            let (initial, binding) = match hand.hand {
                Hand::Right => (
                    'R',
                    crate::settings::binding_text(
                        self.buttons.page_right,
                        self.buttons.pad_page_right,
                    ),
                ),
                Hand::Left => (
                    'L',
                    crate::settings::binding_text(
                        self.buttons.page_left,
                        self.buttons.pad_page_left,
                    ),
                ),
            };
            parts.push(format!("{initial} = {binding}"));
        }
        if parts.is_empty() {
            return None;
        }
        Some(format!("page   {}", parts.join("   ")))
    }
}

/// `[hud] pages`, mirrored where the render thread can read it without the config lock.
///
/// Refreshed from the config every tick rather than latched at possession start, so turning the
/// indicator off in the file makes it disappear on the next frame like every other live setting --
/// rather than on the next possession, which is behaviour a player would report as "the setting
/// does nothing".
static ENABLED: AtomicBool = AtomicBool::new(true);
/// Lock-free "is there anything to draw", so a session that never possesses anything never takes
/// [`PUBLISHED`] at all.
///
/// The order around it is the POINT: [`publish`] stores the banner and then raises this, and
/// [`clear`] lowers this and then drops the banner. Both put the false answer on the side that
/// costs nothing. A false positive is therefore impossible -- this is never true while
/// [`PUBLISHED`] is `None`, so the renderer never takes the lock to be told there is nothing
/// there. A false negative can last exactly one frame, at the instant of a publish, and its whole
/// consequence is that the panel appears 8-16 ms later than it could have.
static SHOWING: AtomicBool = AtomicBool::new(false);
static PUBLISHED: Mutex<Option<Published>> = Mutex::new(None);

/// The banner plus the drawn frames of header highlight it has left.
struct Published {
    banner: Banner,
    flash_frames: u32,
}

fn published() -> std::sync::MutexGuard<'static, Option<Published>> {
    // Poisoning means a previous publish panicked. This is a text panel; refusing to draw it for
    // the rest of the session would be a worse outcome than drawing whatever the panicking tick
    // left behind, and `report_panics_to` has already logged the panic itself.
    PUBLISHED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Mirror `[hud] pages` for the render thread. Called every tick.
pub(crate) fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Is there a panel to draw? Checked by the renderer before it takes any lock, and by the caller
/// that decides whether the overlay has to exist yet.
pub(crate) fn showing() -> bool {
    SHOWING.load(Ordering::Relaxed) && ENABLED.load(Ordering::Relaxed)
}

/// Publish what the buttons do now. `Banner::flash` names the hand whose page key caused this, if
/// any, and gives the header its [`FLASH_FRAMES`] of highlight.
pub(crate) fn publish(banner: Banner) {
    let flash_frames = if banner.flash.is_some() {
        FLASH_FRAMES
    } else {
        0
    };
    *published() = Some(Published {
        banner,
        flash_frames,
    });
    SHOWING.store(true, Ordering::Relaxed);
}

/// Take the panel down. Called on release, on shutdown, and on any teardown that ends a
/// possession -- a banner left up after the creature is gone describes buttons that no longer do
/// anything.
pub(crate) fn clear() {
    SHOWING.store(false, Ordering::Relaxed);
    *published() = None;
}

/// The current panel, and whether the hand named by `Banner::flash` is still highlighted. `None`
/// when nothing is possessed or `[hud] pages` is off.
///
/// Consumes one frame of that highlight, which is why it is `take_` and not `get_`: exactly one
/// caller may call it per drawn frame, and that caller is the renderer. Counting here rather than
/// on the game thread is deliberate -- the highlight is spent by being seen, so a frame the
/// overlay did not draw (minimised, mid-load, `[hud] pages` off for a moment) must not spend any
/// of it. The early return above is what makes the last of those true.
pub(crate) fn take_frame() -> Option<(Banner, bool)> {
    if !showing() {
        return None;
    }
    let mut guard = published();
    let published = guard.as_mut()?;
    let lit = published.flash_frames > 0;
    published.flash_frames = published.flash_frames.saturating_sub(1);
    Some((published.banner, lit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::moveset::dispatch::PageTurn;
    use crate::moveset::table::parse_line;
    use crate::settings::MappingSettings;

    /// Two right-hand sets and one left-hand set, so the paged header, the one-set header and an
    /// empty bucket are all exercised by one creature.
    fn dispatcher() -> Dispatcher {
        let moveset = parse_line("9001 3000:0:0:1 3001:0:1:1 3010:1:0:1 3011:1:1:1 3100:2:0:3")
            .expect("well-formed test line")
            .1;
        Dispatcher::new(
            moveset,
            MappingSettings::default(),
            ButtonSettings::default(),
        )
    }

    /// The whole complaint in one assertion: the set the player is on has to be in the text.
    #[test]
    fn the_header_names_the_set_and_how_many_there_are() {
        let mut engine = dispatcher();
        let banner = Banner::from_dispatcher(9001, "Test Beast", &engine, None);
        assert_eq!(banner.hands[0].header(), "R 1/2");
        assert_eq!(banner.title(), "c9001 Test Beast");

        assert_eq!(
            engine.turn_page(Hand::Right),
            PageTurn::Turned { page: 2, pages: 2 }
        );
        let turned = Banner::from_dispatcher(9001, "Test Beast", &engine, Some(Hand::Right));
        assert_eq!(turned.hands[0].header(), "R 2/2");
        assert_eq!(turned.flash, Some(Hand::Right));
    }

    /// A hand with one set must say so. `1/1` on its own is indistinguishable from a key that
    /// never fired, which is the failure mode this whole module exists to end.
    #[test]
    fn a_hand_with_one_set_says_so_rather_than_printing_a_bare_one_of_one() {
        let engine = dispatcher();
        let banner = Banner::from_dispatcher(9001, "", &engine, None);
        assert_eq!(banner.hands[1].header(), "L 1/1 (one set)");
        // ...and an unnamed id falls back to the number rather than to an empty title.
        assert_eq!(banner.title(), "c9001");
    }

    /// An empty bucket draws a row saying it is empty. Omitting the row would make a bound button
    /// with nothing behind it look like a panel that failed to render.
    #[test]
    fn a_button_whose_bucket_is_empty_draws_a_dash_rather_than_vanishing() {
        let engine = dispatcher();
        let banner = Banner::from_dispatcher(9001, "", &engine, None);
        let movement = banner.hands[1].leads[1];
        assert_eq!(movement.bucket, Bucket::Movement);
        assert_eq!(movement.lead, None);
        assert!(
            movement.line().ends_with("--"),
            "an empty bucket must draw a dash: {}",
            movement.line()
        );
        // And a bucket with something in it prints the id the log line and the derived file print.
        let light = banner.hands[0].leads[0];
        assert_eq!(light.lead, Some(3000));
        assert!(light.line().contains("3000"), "{}", light.line());
        assert!(light.line().contains("light"), "{}", light.line());
    }

    /// The footer names the keys that are bound, not the shipped defaults, mentions only the
    /// hands that actually have a second set, and disappears when neither does.
    #[test]
    fn the_footer_names_the_bound_page_keys_and_is_omitted_when_nothing_pages() {
        let engine = dispatcher();
        let footer = Banner::from_dispatcher(9001, "", &engine, None)
            .footer()
            .expect("the right hand has two sets, so the footer applies");
        assert!(footer.contains("R = Right"), "{footer}");
        assert!(
            !footer.contains("L ="),
            "the left hand has one set, so naming its page key would be an instruction that does \
             nothing: {footer}"
        );

        let single = parse_line("9002 3000:0:0:1")
            .expect("well-formed test line")
            .1;
        let engine = Dispatcher::new(
            single,
            MappingSettings::default(),
            ButtonSettings::default(),
        );
        assert_eq!(
            Banner::from_dispatcher(9002, "", &engine, None).footer(),
            None,
            "a creature with one set on each hand has no page key to advertise"
        );
    }

    /// A creature the shipped table has never heard of still gets a panel, and that panel says so
    /// on all four buttons. Drawing nothing there would be the original defect wearing a different
    /// hat: the player cannot tell "this creature has no attacks" from "the panel is broken".
    #[test]
    fn a_creature_with_no_shipped_moveset_gets_a_panel_that_says_so() {
        let banner = Banner::absent(1234, "", ButtonSettings::default());
        for hand in &banner.hands {
            assert_eq!(
                hand.header().chars().skip(2).collect::<String>(),
                "no attack sets"
            );
            for lead in &hand.leads {
                assert_eq!(lead.lead, None);
                assert!(lead.line().ends_with("--"), "{}", lead.line());
            }
        }
        assert_eq!(
            banner.footer(),
            None,
            "there is no set to page to, so naming the page key would be an instruction that \
             cannot work"
        );
    }

    /// `[hud] pages = false` must hide the panel without the possession having to end, and turning
    /// it back on must restore it -- the publish is not thrown away, only withheld.
    ///
    /// One test rather than several because all of these drive the same process-wide globals, and
    /// `cargo test` runs test functions in parallel threads; splitting them would make them race
    /// each other for the one `PUBLISHED`.
    #[test]
    fn the_config_switch_hides_the_panel_live_and_the_flash_decays_over_drawn_frames() {
        let mut engine = dispatcher();
        set_enabled(true);
        publish(Banner::from_dispatcher(9001, "", &engine, None));
        assert!(showing());
        let (_, lit) = take_frame().expect("published");
        assert!(
            !lit,
            "a possession start is not a change, so nothing highlights"
        );

        set_enabled(false);
        assert!(!showing());
        assert!(
            take_frame().is_none(),
            "the switch must hide it immediately"
        );

        set_enabled(true);
        assert!(
            take_frame().is_some(),
            "and give it back without a republish"
        );

        // A page turn lights the hand, and it stays lit for exactly FLASH_FRAMES drawn frames --
        // not one more, so two taps in a row read as two events.
        engine.turn_page(Hand::Right);
        publish(Banner::from_dispatcher(
            9001,
            "",
            &engine,
            Some(Hand::Right),
        ));
        for frame in 0..FLASH_FRAMES {
            let (banner, lit) = take_frame().expect("published");
            assert!(lit, "frame {frame} of {FLASH_FRAMES} must still be lit");
            assert_eq!(banner.flash, Some(Hand::Right));
        }
        let (_, lit) = take_frame().expect("the panel outlives its highlight");
        assert!(!lit, "the highlight ends; the panel does not");

        // Hiding it does not spend the highlight: a frame nobody drew is a frame nobody saw.
        publish(Banner::from_dispatcher(
            9001,
            "",
            &engine,
            Some(Hand::Right),
        ));
        set_enabled(false);
        for _ in 0..FLASH_FRAMES {
            assert!(take_frame().is_none());
        }
        set_enabled(true);
        let (_, lit) = take_frame().expect("published");
        assert!(lit, "the highlight was withheld, not burned");

        clear();
        assert!(!showing());
        assert!(take_frame().is_none());
    }
}
