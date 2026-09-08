//! Cancel discipline: when a press may leave the animation that is already playing.
//!
//! # The engine has none, and that is a measured fact rather than a suspicion
//!
//! Firing an attack is a write to `CSChrEventModule+0x18 requestAnimationId`, and **nothing on
//! that path consults what the creature is currently doing**. `CS::CSChrEventModule::Update`
//! (1.16.2 `0x14043a580`) gates the request on exactly five things, none of which is the current
//! animation:
//!
//! * `!CS::ChrIns::IsDead`;
//! * `componentContainer->actionFlag->actionAnimationFlags` bit 12 clear -- and that bit has three
//!   `or dword [rax+0x10], 0x1000` sites in the whole image, two of them inside `SetTextScale` /
//!   `SetFontSize` (debug text, a different structure at the same offset) and the third inside
//!   `HksAct` (`0x14040cbd0`), the Lua action-script host. It is script-driven state, not a
//!   per-swing marker;
//! * `!CS::CSChrThrowModule::IsInTrow`;
//! * `FUN_14043acc0`, which is not a cancel check at all -- its whole body compares the requested
//!   id against `PlayerCommonParam::animeID_MaterialItemPick`, `animeID_DropItemPick`,
//!   `animeID_SleepCollectorItemPick` and two `GameSystemCommonParam` treasure pickup ids, and
//!   defers those five to `actionFlag->field_0x28` instead of playing them;
//! * `field_0x70 == 0`.
//!
//! Everything else goes straight to `FUN_140c14400`, which formats `W_Event%04d` and hands it to
//! the behaviour graph -- which accepts it, mid-swing, every time. So the cancel the player sees
//! is not a bug in how this crate writes the field. It is the absence of any rule at all, and the
//! rule has to live here.
//!
//! # ...but it does know, one module along
//!
//! What the request path does not consult, the animation system writes down. TAE event type 0
//! (`ChrActionFlag`) with `FlagType` 86 sets bit 5 of
//! `CSChrActionRequestModule::taeCancels`, `CS::ChrIns::PreBehaviorSafe` clears that bit every
//! frame, and `CS::CSAiFunc::IsEnableCancelAttack` reads it to decide whether a creature's own AI
//! may chain out of the swing it is in. So the game already computes, per creature, per frame,
//! per animation, the exact predicate this layer needs -- for creatures, which is what a
//! possession is wearing. Reading that bit is [`Availability::Chainable`], and the answer is the
//! engine's rather than ours.
//!
//! It costs no game address: [`crate::possess::game::Chr::attack_cancel_allowed`] reads the field
//! and applies the same two tests the engine's own leaf predicate applies. See
//! [`crate::possess::layout::chr_action_request_module`] for the byte proof on both builds.
//!
//! # The offline window, which is the fallback and the report
//!
//! The same TimeAct event has a start and an end in animation-local seconds, and those are
//! readable offline from the corpus -- so the shipped table carries the start as
//! [`crate::moveset::table::Move::chain_from_cs`], and this layer compares it against
//! `CSChrTimeActModule::animQueue[readIdx].localTime`, the creature's own playhead. That is the
//! answer used when the module chain does not read, and it is also what lets
//! `er-npc-possess.derived.toml` say per move whether the game authored a real window for it or
//! whether the mod is going to make the player wait out the animation.
//!
//! A move with no window from either source is [`Availability::Committed`] for its whole length,
//! so a press during it waits for the animation to end rather than guessing. That is the
//! least-bad rule and it is never an interrupt.
//!
//! # What "its whole length" has to mean, and what it meant on 2026-09-02
//!
//! It has to mean the clip's length. It used to mean forever, and that is the bug the first live
//! run found: a possessed c4604 spent five and a half minutes standing still while every press
//! was held as "mid-attack" and dropped. Two things were wrong and both are fixed here.
//!
//! The first is that the creature was not animating at all and this layer could not tell.
//! `CSChrTimeActModule::animQueue[readIdx]` is only meaningful when `readIdx != writeIdx` --
//! `PreBehaviorSafe` sets them equal every frame and each animation actually driven pushes one
//! entry -- so an equal pair means nothing was driven and the entry is a leftover. Reading it
//! anyway reported the attack from minutes ago, forever. `Chr::current_anim_frame` now answers
//! `None` there, and `None` is [`Availability::Idle`]: a creature animating nothing has no swing
//! to protect, so the press fires.
//!
//! The second is that every `Committed` answer below rests on evidence with no expiry of its own
//! -- an engine bit that may never read, a window that was never measured -- so "unknown" could
//! outlive the animation it was describing. The clip's own `animLength` is now checked first, and
//! nothing may hold a press past it. The worst case is one clip, not one session.
//!
//! Both are the same mistake in different clothes: a fail-closed default on an oracle that cannot
//! read is indistinguishable, from the player's chair, from the mod not working. Which is why
//! [`Source`] exists and every branch says on whose authority it answered.
//!
//! The fallback is slightly more permissive than the engine, and only in one direction. The
//! table carries the window's start and not its end, so once the offline window has opened this
//! layer treats the rest of the clip as chainable, while the live bit goes false again when the
//! event's end time passes. Measured on c4500, most windows run to within a frame or two of the
//! clip's end, so the difference is a short tail at the very end of a recovery -- and it only
//! applies at all when the module pointer failed to read, which is the path where the choice is
//! between this and nothing. Carrying the end as a second column is the fix if it ever matters.
//!
//! # How much of the moveset this actually covers, and what the rest falls back to
//!
//! 5,086 of the 6,921 shipped moves carry a window. Split by what they are, that is **4,576 of
//! the 4,669 attacks (98.0%)** against 510 of the 2,252 movement moves -- and the split is the
//! shape of the data rather than a hole in it, because FlagType 86 is the flag for cancelling an
//! attack and a dodge is not one. So a press during an attack chains on the game's own window
//! almost always, and a press during a dodge usually waits the dodge out, which for a
//! half-second clip is what waiting means anyway.
//!
//! # What is read one frame late
//!
//! The possession ticks in `CSTaskGroupIndex::FrameBegin`. `CS::ChrIns::PreBehaviorSafe` clears
//! the transient `taeCancels` bits and the TimeAct events re-set them during the behaviour
//! update, both later in the same frame -- so every value this layer reads describes the window
//! the previous frame established. 16 ms against a window whose median width is 800 ms, and
//! acting on the window the last frame established is the correct reading of a press anyway.

// Pure state machine over observations; ungated so `cargo test` proves it on the host.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::moveset::dispatch::Input;

/// What the creature says it is doing this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Playing {
    /// `animQueue[readIdx].animId`.
    pub(crate) animation: i32,
    /// `animQueue[readIdx].localTime`, seconds into the clip. `None` when the field could not be
    /// read, which is the same answer a missing module pointer gives.
    pub(crate) elapsed_s: Option<f32>,
    /// `animQueue[readIdx].animLength`, how long the clip runs. The bound on how long any answer
    /// here can hold a press: once the playhead is past the end, the clip is over whatever the
    /// other two sources say.
    pub(crate) length_s: Option<f32>,
    /// Is the animation being played one of this creature's shipped moves?
    ///
    /// The positive test, and it replaced "the id is below 3000" on 2026-09-02. A threshold
    /// cannot work here: the id the engine reports is a raw TimeAct id in a per-creature space
    /// that the table does not enumerate, and it is routinely five or seven digits. A possessed
    /// Battlemage idles in 43000 and spawns through 3009000/3009500 -- so a ceiling test called
    /// it "permanently mid-attack" and refused every press on a creature with thirteen melee
    /// attacks. Asking whether the id is one this crate could have fired cannot be surprised by
    /// a band nobody has seen.
    pub(crate) is_known_move: bool,
    /// `CSChrActionRequestModule::taeCancels`, resolved through the engine's own predicate.
    /// `Some(true)` is the game saying a chain is allowed right now. `None` is "did not read",
    /// which is why this is not a `bool`.
    pub(crate) cancel_allowed: Option<bool>,
}

/// May a press be honoured right now?
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Availability {
    /// Nothing is playing that a press could cancel -- idle, locomotion, a turn.
    Idle,
    /// An attack is playing and has already done its work. A follow-up here is a chain.
    Chainable,
    /// An attack is playing and leaving it now would cancel a hit that has not landed.
    Committed,
}

impl Availability {
    /// Everything except [`Self::Committed`] lets a press through.
    ///
    /// [`Self::Idle`] and [`Self::Chainable`] are kept apart even though they answer this the
    /// same way, because they are different facts about the creature and the difference is what
    /// a future log line or report would need. Collapsing them into a bool here would throw that
    /// away at the only point it is known.
    pub(crate) const fn accepts_a_press(self) -> bool {
        !matches!(self, Self::Committed)
    }
}

/// Which source decided, so a run can say what the oracle read and not only what it concluded.
///
/// The 2026-09-02 run had to be diagnosed from four log lines and a hypothesis, because the
/// resolver said "committed" without ever saying on whose authority. Every branch below names
/// itself now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// The TimeAct queue held nothing driven this frame: the creature is animating nothing.
    NotAnimating,
    /// A fresh entry the shipped table does not know: an idle loop, locomotion, a hit reaction.
    /// Whatever it is, this crate did not fire it and does not own it.
    NotOurMove,
    /// The playhead is at or past the clip's own length.
    ClipFinished,
    /// `CSChrActionRequestModule::taeCancels` answered.
    Engine,
    /// The engine did not answer; the shipped table's offline window did.
    Table,
    /// Neither answered. Committed until the clip ends, which is bounded but is a guess about
    /// nothing rather than a measurement.
    Unmeasured,
}

impl Source {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::NotAnimating => "not-animating",
            Self::NotOurMove => "not-our-move",
            Self::ClipFinished => "clip-finished",
            Self::Engine => "engine-taecancels",
            Self::Table => "table-window",
            Self::Unmeasured => "unmeasured",
        }
    }
}

/// What the resolver decided and who decided it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Reading {
    pub(crate) availability: Availability,
    pub(crate) source: Source,
}

/// Resolve what the creature will accept, from what the engine says and what the table measured.
///
/// The engine wins when it answers. `cancel_allowed` is the same predicate the game's own AI is
/// gated on, evaluated by the game, for this creature, on this frame; the table's window is an
/// offline reading of the TimeAct event that sets that bit, so when the two could disagree the
/// live one is the one that is right. The table is what answers when the module chain does not
/// read, and it is what the derived report can print before a possession has even started.
///
/// `chain_from_s` is the moment the playing move's window opens, or `None` when the generator
/// found no window on it.
///
/// Failing open on an unreadable clock is deliberate. `None` for `playing` means the TimeAct
/// module pointer did not read, which is exactly the state the crate was in before this layer
/// existed; answering [`Availability::Committed`] there would buffer every press behind a reading
/// that is never going to arrive and leave the buttons dead. A creature that is genuinely stuck is
/// [`crate::moveset::watchdog`]'s problem and always was.
pub(crate) fn resolve(playing: Option<Playing>, chain_from_s: Option<f32>) -> Reading {
    let read = |availability, source| Reading {
        availability,
        source,
    };
    let Some(playing) = playing else {
        return read(Availability::Idle, Source::NotAnimating);
    };
    if !playing.is_known_move {
        // Not one of this creature's clips: an idle loop, locomotion, a hit reaction, a gimmick
        // the world started. None of those is a swing this mod owns, so none of them may hold a
        // press.
        return read(Availability::Idle, Source::NotOurMove);
    }
    // The clip is over. Nothing below may hold a press past this, and that is the difference
    // between "wait for this swing to finish" and "wait forever": every other branch here can
    // answer `Committed` on evidence that has no expiry of its own, so the expiry is imposed
    // here, from the clip's own length.
    if let (Some(elapsed), Some(length)) = (playing.elapsed_s, playing.length_s)
        && elapsed.is_finite()
        && elapsed >= length
    {
        return read(Availability::Chainable, Source::ClipFinished);
    }
    if let Some(allowed) = playing.cancel_allowed {
        return read(
            if allowed {
                Availability::Chainable
            } else {
                Availability::Committed
            },
            Source::Engine,
        );
    }
    match (chain_from_s, playing.elapsed_s) {
        // `>=` and not `>`: the window opening on the exact frame a press lands should let it
        // through, the same inclusive reading the combo window uses.
        (Some(from), Some(elapsed)) if elapsed.is_finite() && elapsed >= from => {
            read(Availability::Chainable, Source::Table)
        }
        (Some(_), Some(_)) => read(Availability::Committed, Source::Table),
        _ => read(Availability::Committed, Source::Unmeasured),
    }
}

/// What happened to a press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Held {
    /// Nothing was waiting; this press is now.
    Queued,
    /// A press was already waiting and this one took its place.
    Replaced,
}

/// What the frame owes the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Release {
    Nothing,
    /// The buffered press may fire now.
    Fire(Input),
    /// The buffered press waited too long and is gone.
    Expired(Input),
}

/// One buffered press, and only one.
///
/// A player who mashes six times through a long wind-up must not get six attacks out the other
/// side, so the sixth press replaces the fifth rather than joining a queue. And a press that is
/// never released has to rot: `window_ms` after it landed it is dropped, so an attack you gave up
/// on does not arrive seconds later while you are walking away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Buffer {
    held: Option<(Input, u64)>,
    window_ms: u32,
}

impl Buffer {
    pub(crate) const fn new(window_ms: u32) -> Self {
        Self {
            held: None,
            window_ms,
        }
    }

    pub(crate) const fn is_holding(&self) -> bool {
        self.held.is_some()
    }

    /// Take a press that cannot be honoured yet.
    pub(crate) const fn hold(&mut self, input: Input, now_ms: u64) -> Held {
        let outcome = if self.held.is_some() {
            Held::Replaced
        } else {
            Held::Queued
        };
        self.held = Some((input, now_ms));
        outcome
    }

    /// One frame. Expiry is checked first, and that ordering is the setting doing its job: a
    /// press that has outlived its window is gone even on the very frame the animation ends,
    /// because otherwise a ten-second clip would deliver every press ever made during it.
    pub(crate) fn release(&mut self, now_ms: u64, availability: Availability) -> Release {
        let Some((input, pressed_ms)) = self.held else {
            return Release::Nothing;
        };
        if now_ms.saturating_sub(pressed_ms) > u64::from(self.window_ms) {
            self.held = None;
            return Release::Expired(input);
        }
        if availability.accepts_a_press() {
            self.held = None;
            return Release::Fire(input);
        }
        Release::Nothing
    }

    /// Drop whatever is waiting, without firing it.
    pub(crate) const fn forget(&mut self) {
        self.held = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A creature whose `taeCancels` did not read, so only the offline window can answer.
    /// Long enough that the clip-length bound never fires by accident in these cases.
    const fn playing(animation: i32, elapsed_s: f32) -> Option<Playing> {
        Some(Playing {
            animation,
            elapsed_s: Some(elapsed_s),
            length_s: Some(9_999.0),
            is_known_move: animation >= 3000,
            cancel_allowed: None,
        })
    }

    /// A creature the engine answered for.
    const fn asked(animation: i32, elapsed_s: f32, allowed: bool) -> Option<Playing> {
        Some(Playing {
            animation,
            elapsed_s: Some(elapsed_s),
            length_s: Some(9_999.0),
            is_known_move: animation >= 3000,
            cancel_allowed: Some(allowed),
        })
    }

    #[test]
    fn the_engines_own_answer_beats_the_offline_window_in_both_directions() {
        // Table says "not yet" (0.4 of 0.5), engine says the window is open. The engine is
        // evaluating the very TimeAct event the table read offline, on this frame, so it wins.
        assert_eq!(
            resolve(asked(3000, 0.4, true), Some(0.5)).availability,
            Availability::Chainable
        );
        // ...and the other way. Table says open, engine says no.
        assert_eq!(
            resolve(asked(3000, 5.0, true), None).availability,
            Availability::Chainable
        );
        assert_eq!(
            resolve(asked(3000, 5.0, false), Some(0.5)).availability,
            Availability::Committed
        );
    }

    #[test]
    fn the_engine_cannot_make_a_neutral_animation_cancellable_because_there_is_nothing_to_cancel() {
        assert_eq!(
            resolve(asked(0, 0.0, true), None).availability,
            Availability::Idle
        );
    }

    #[test]
    fn a_neutral_animation_is_idle_however_the_window_reads() {
        // Below the attack band: idle, locomotion or a turn. Nothing to cancel.
        assert_eq!(
            resolve(playing(2999, 0.0), Some(9.0)).availability,
            Availability::Idle
        );
        assert_eq!(
            resolve(playing(0, 0.0), None).availability,
            Availability::Idle
        );
    }

    #[test]
    fn an_attack_is_committed_until_its_window_opens_and_chainable_after() {
        assert_eq!(
            resolve(playing(3000, 0.4), Some(0.5)).availability,
            Availability::Committed
        );
        assert_eq!(
            resolve(playing(3000, 0.5), Some(0.5)).availability,
            Availability::Chainable,
            "the window is inclusive on the frame it opens"
        );
        assert_eq!(
            resolve(playing(3000, 5.0), Some(0.5)).availability,
            Availability::Chainable
        );
    }

    #[test]
    fn a_move_with_no_measured_window_stays_committed_for_its_whole_length() {
        // The honest answer for a move the generator could not measure: never interrupt it, wait
        // for it to end. `Chainable` here would be a guess dressed as a measurement.
        assert_eq!(
            resolve(playing(3000, 0.0), None).availability,
            Availability::Committed
        );
        assert_eq!(
            resolve(playing(3000, 600.0), None).availability,
            Availability::Committed
        );
    }

    #[test]
    fn an_unreadable_clock_fails_open_rather_than_deadening_every_button() {
        assert_eq!(resolve(None, Some(0.5)).availability, Availability::Idle);
        // The id read but the time did not: still committed, because the window cannot be
        // evaluated -- but the animation is known to be playing, so waiting is right.
        assert_eq!(
            resolve(
                Some(Playing {
                    animation: 3000,
                    elapsed_s: None,
                    length_s: None,
                    is_known_move: true,
                    cancel_allowed: None,
                }),
                Some(0.5)
            )
            .availability,
            Availability::Committed
        );
    }

    #[test]
    fn a_nan_clock_is_treated_as_unmeasured() {
        assert_eq!(
            resolve(playing(3000, f32::NAN), Some(0.5)).availability,
            Availability::Committed
        );
        assert_eq!(
            resolve(playing(3000, f32::INFINITY), Some(0.5)).availability,
            Availability::Committed
        );
    }

    /// The 2026-09-02 regression. A creature that is animating nothing must resolve `Idle`, not
    /// `Committed`. `None` reaches this function for two different reasons -- the module pointer
    /// did not read, and the TimeAct queue held nothing driven this frame -- and both mean the
    /// same thing to a player: there is no swing to protect, so the press must fire now.
    #[test]
    fn a_creature_animating_nothing_is_idle_and_says_which_of_the_two_reasons_it_was() {
        let reading = resolve(None, Some(0.5));
        assert_eq!(reading.availability, Availability::Idle);
        assert_eq!(reading.source, Source::NotAnimating);
    }

    /// The bound that turns "wait forever" into "wait one clip". Every branch below the length
    /// test can answer `Committed` on evidence with no expiry of its own -- an engine bit that
    /// never reads, a window that was never measured -- so the clip's own length is what stops
    /// the answer outliving the animation it describes.
    #[test]
    fn nothing_can_hold_a_press_past_the_end_of_the_clip_it_is_protecting() {
        let past_the_end = |cancel_allowed| {
            Some(Playing {
                animation: 3000,
                elapsed_s: Some(4.0),
                length_s: Some(3.5),
                is_known_move: true,
                cancel_allowed,
            })
        };
        for cancel_allowed in [None, Some(false)] {
            let reading = resolve(past_the_end(cancel_allowed), None);
            assert_eq!(
                reading.availability,
                Availability::Chainable,
                "cancel_allowed={cancel_allowed:?}"
            );
            assert_eq!(reading.source, Source::ClipFinished);
        }
        // ...and inside the clip the same unmeasured creature is still protected.
        let inside = Some(Playing {
            animation: 3000,
            elapsed_s: Some(1.0),
            length_s: Some(3.5),
            is_known_move: true,
            cancel_allowed: None,
        });
        let reading = resolve(inside, None);
        assert_eq!(reading.availability, Availability::Committed);
        assert_eq!(reading.source, Source::Unmeasured);
    }

    /// Every branch names itself, because the last time this resolver was wrong it took a live
    /// run plus a hypothesis to find out which one had fired.
    #[test]
    fn every_branch_reports_the_source_that_decided_it() {
        assert_eq!(resolve(None, None).source, Source::NotAnimating);
        assert_eq!(resolve(playing(0, 0.0), None).source, Source::NotOurMove);
        assert_eq!(resolve(asked(3000, 0.1, true), None).source, Source::Engine);
        assert_eq!(resolve(playing(3000, 1.0), Some(0.5)).source, Source::Table);
        assert_eq!(resolve(playing(3000, 1.0), None).source, Source::Unmeasured);
    }

    #[test]
    fn a_press_that_cannot_be_honoured_waits_and_fires_when_the_animation_lets_go() {
        let mut buffer = Buffer::new(1000);
        assert_eq!(buffer.hold(Input::R1, 0), Held::Queued);
        assert_eq!(
            buffer.release(100, Availability::Committed),
            Release::Nothing,
            "still mid-swing"
        );
        assert_eq!(
            buffer.release(200, Availability::Chainable),
            Release::Fire(Input::R1)
        );
        assert_eq!(buffer.release(300, Availability::Idle), Release::Nothing);
        assert!(!buffer.is_holding());
    }

    #[test]
    fn the_animation_ending_releases_a_buffered_press_just_as_a_chain_window_does() {
        let mut buffer = Buffer::new(1000);
        buffer.hold(Input::R2, 0);
        assert_eq!(
            buffer.release(500, Availability::Idle),
            Release::Fire(Input::R2)
        );
    }

    #[test]
    fn mashing_six_times_leaves_one_press_waiting_and_not_six() {
        let mut buffer = Buffer::new(1000);
        assert_eq!(buffer.hold(Input::R1, 0), Held::Queued);
        for step in 1..6 {
            assert_eq!(buffer.hold(Input::R2, step * 10), Held::Replaced);
        }
        assert_eq!(
            buffer.release(100, Availability::Idle),
            Release::Fire(Input::R2),
            "the last press is the one that survives"
        );
        assert_eq!(buffer.release(110, Availability::Idle), Release::Nothing);
    }

    #[test]
    fn a_press_nobody_came_back_for_expires_instead_of_arriving_late() {
        let mut buffer = Buffer::new(1000);
        buffer.hold(Input::L1, 0);
        assert_eq!(
            buffer.release(1000, Availability::Committed),
            Release::Nothing,
            "the window is inclusive"
        );
        assert_eq!(
            buffer.release(1001, Availability::Committed),
            Release::Expired(Input::L1)
        );
        assert!(!buffer.is_holding());
    }

    #[test]
    fn expiry_beats_a_window_that_opens_on_the_same_frame() {
        let mut buffer = Buffer::new(100);
        buffer.hold(Input::L2, 0);
        assert_eq!(
            buffer.release(5_000, Availability::Idle),
            Release::Expired(Input::L2),
            "a ten-second clip must not deliver every press ever made during it"
        );
    }

    #[test]
    fn forgetting_drops_the_press_without_firing_it() {
        let mut buffer = Buffer::new(1000);
        buffer.hold(Input::R1, 0);
        buffer.forget();
        assert_eq!(buffer.release(10, Availability::Idle), Release::Nothing);
    }
}
