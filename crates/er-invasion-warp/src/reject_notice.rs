//! Decides WHEN a rejection is worth putting on screen, and what it should say.
//!
//! Kept separate from the code that displays it so the policy can be tested on a host with no game
//! attached. The display side is a game-thread call into `CSPopupMenu`; this half is arithmetic.
//!
//! # Why this is not simply "one banner per rejection"
//!
//! Measured 2026-08-06: Seamless retries roughly every 20 seconds, and during a hunt in a busy
//! bracket most of those retries end in a rejection at the SAME wrong place — the same host, or
//! the same popular area, coming back around. A banner every time would be wallpaper within a
//! minute, and a notification the player learns to ignore is worse than none: it costs screen space
//! and teaches them the mod is noisy.
//!
//! So a place is announced when it is NEW relative to the last thing announced, and consecutive
//! rejections at that same place stay silent. Moving to a different wrong place is genuinely new
//! information and is announced again, including a return to somewhere announced earlier — the
//! player's question is "where am I being sent right now", not "where have I ever been sent".

use core::fmt::Write as _;

use crate::invasion_warp::BlockKey;
use crate::local_invasion::RejectReason;

/// A few words naming WHY a destination was refused, for the banner.
///
/// The player can already see WHERE from the block name; what they cannot see is whether the mod
/// refused it on the rule they set, on a place they excluded by hand, or because it could not
/// resolve a name at all. Those lead to different actions — move, un-exclude, or open the map —
/// so collapsing them into a bare "rejected" wastes the notification.
///
/// Deliberately terse: the banner's width is bounded by the donor string's allocation, and an
/// overrun is silently truncated rather than wrapped.
#[must_use]
pub const fn reason_phrase(reason: RejectReason) -> &'static str {
    match reason {
        // The ordinary case during a hunt: right rules, wrong place.
        RejectReason::WrongBlock | RejectReason::WrongPlaceName => "elsewhere",
        RejectReason::NotNamed => "not on your list",
        // Actionable in a way the others are not: the map has not been opened, so no destination
        // has a name and everything fails closed. Saying "unnamed" would read as the game's fault.
        RejectReason::CandidateUnnamed => "open your map",
        RejectReason::NothingToMatchAgainst => "open your map",
        RejectReason::ExcludedByUser => "you excluded it",
    }
}

/// The last thing this banner said, so a repeat of it can stay quiet.
///
/// A success and a rejection are different announcements about the same place, which is why this
/// is one enum rather than two independent latches: an invasion that finally lands at a block the
/// player was repeatedly rejected from MUST speak, and a rejection after a success must speak too.
/// Two separate latches would have each suppressed the other's news.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Announced {
    Rejected(u32, RejectReason),
    Succeeded(u32),
    /// A destination the mod did not judge -- the filter's master switch is off, so this is the
    /// server's choice reported as-is.
    Arrived(u32),
}

/// Tracks what was last announced so repeats can be suppressed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RejectNotice {
    /// The most recent ANNOUNCEMENT. `None` before the first one.
    last_announced: Option<Announced>,
    /// Rejections suppressed since that announcement, for the telemetry line.
    suppressed: usize,
}

impl RejectNotice {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            last_announced: None,
            suppressed: 0,
        }
    }

    /// Feed one rejected destination. Returns the text to display, or `None` to stay silent.
    ///
    /// `enabled` is threaded through rather than checked by the caller so that a disabled notice
    /// still advances the state — otherwise turning it on mid-session would announce a stale place
    /// the player was rejected from minutes ago.
    /// `place` is the area's own name when the world map has been read this session, and `None`
    /// before that. It is passed in rather than looked up here so this type stays testable off the
    /// game: resolving a name means calling into the engine's message repository.
    pub fn observe(
        &mut self,
        enabled: bool,
        block: u32,
        reason: RejectReason,
        place: Option<&str>,
    ) -> Option<String> {
        let repeat = self.last_announced == Some(Announced::Rejected(block, reason));
        self.last_announced = Some(Announced::Rejected(block, reason));
        if repeat {
            self.suppressed = self.suppressed.saturating_add(1);
            return None;
        }
        self.suppressed = 0;
        if !enabled {
            return None;
        }
        let mut text = String::new();
        // Short on purpose: the banner's width is bounded by the donor string's allocation, and a
        // message that overflows is silently truncated rather than wrapped. WHERE and WHY, because
        // the place alone does not tell the player whether to move, un-exclude somewhere, or open
        // their map.
        //
        // The NAME when there is one. A block id is precise and unreadable -- being told you were
        // rejected from `m60_50_39_00` on a banner that closes itself in a couple of seconds is a
        // lookup task, not a notification. The id remains the fallback rather than being dropped,
        // because it is the only thing available before the map has been opened, and a rejection
        // with no place at all would be strictly worse than an unfriendly one.
        match place {
            Some(place) if !place.is_empty() => {
                let _ = write!(text, "Rejected {place} ({})", reason_phrase(reason));
            }
            _ => {
                let _ = write!(
                    text,
                    "Rejected {} ({})",
                    BlockKey::from_raw(block),
                    reason_phrase(reason)
                );
            }
        }
        Some(text)
    }

    /// Feed a destination the filter ACCEPTED. Returns the text to display, or `None`.
    ///
    /// THE BUG THIS FIXES: the banner announced every rejection and then said nothing when the
    /// hunt finally succeeded, so the last thing left on screen was a rejection -- the player was
    /// told where they were NOT going and never told they had arrived. Worse, the rejection latch
    /// was never cleared by the success, so a later rejection at that same block stayed silent as
    /// a "repeat" of an announcement from before the invasion that happened in between.
    ///
    /// Same shape as [`Self::observe`]: state advances even when the notice is disabled, and the
    /// place name is resolved by the caller so this type stays testable off the game.
    pub fn observe_success(
        &mut self,
        enabled: bool,
        block: u32,
        place: Option<&str>,
    ) -> Option<String> {
        let repeat = self.last_announced == Some(Announced::Succeeded(block));
        self.last_announced = Some(Announced::Succeeded(block));
        // A success ends the run of rejections it followed; the count belongs to that run.
        self.suppressed = 0;
        if repeat || !enabled {
            return None;
        }
        let mut text = String::new();
        // Says the outcome first and the place second, exactly like the rejection line, so the two
        // read as the same banner reporting opposite results rather than as two unrelated messages.
        match place {
            Some(place) if !place.is_empty() => {
                let _ = write!(text, "Invasion successful: {place}");
            }
            // Before the world map has been read nothing has a name, so the id is the fallback --
            // the same trade the rejection line makes, for the same reason.
            _ => {
                let _ = write!(text, "Invasion successful: {}", BlockKey::from_raw(block));
            }
        }
        Some(text)
    }

    /// Feed a destination that arrived while the filter was SWITCHED OFF.
    ///
    /// The banner is not a by-product of filtering. With the master switch off nothing is judged,
    /// so there is no verdict to report -- but where the server just sent the player is still the
    /// single most useful thing this surface can say, and it is what the player will want when
    /// this grows into a fuller readout.
    ///
    /// Deliberately worded as a statement of fact rather than approval: the mod did not choose
    /// this destination and must not appear to have blessed it.
    pub fn observe_arrival(
        &mut self,
        enabled: bool,
        block: u32,
        place: Option<&str>,
    ) -> Option<String> {
        let repeat = self.last_announced == Some(Announced::Arrived(block));
        self.last_announced = Some(Announced::Arrived(block));
        self.suppressed = 0;
        if repeat || !enabled {
            return None;
        }
        let mut text = String::new();
        match place {
            Some(place) if !place.is_empty() => {
                let _ = write!(text, "Invading {place}");
            }
            _ => {
                let _ = write!(text, "Invading {}", BlockKey::from_raw(block));
            }
        }
        Some(text)
    }

    /// How many rejections have been suppressed since the last announcement.
    #[must_use]
    pub const fn suppressed(&self) -> usize {
        self.suppressed
    }

    /// Forget what was last announced, so the next rejection speaks up again.
    ///
    /// Called when a search ends — a new hunt is a new question, and the first rejection of it is
    /// worth hearing even if it lands somewhere announced during the previous one.
    pub fn reset(&mut self) {
        self.last_announced = None;
        self.suppressed = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMGRAVE: u32 = 0x3c2a_2400; // m60_42_36_00
    const ELSEWHERE: u32 = 0x1501_0000; // m21_01_00_00

    #[test]
    fn the_first_rejection_at_a_place_is_announced() {
        let mut notice = RejectNotice::new();
        let text = notice
            .observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None)
            .expect("announced");
        assert!(text.contains("m60_42_36_00"), "names the place: {text}");
        assert!(text.starts_with("Rejected"), "says what happened: {text}");
    }

    #[test]
    fn a_named_area_is_printed_instead_of_the_block_id() {
        let mut notice = RejectNotice::new();
        let text = notice
            .observe(
                true,
                LIMGRAVE,
                RejectReason::WrongPlaceName,
                Some("Limgrave"),
            )
            .expect("announced");
        assert!(text.contains("Limgrave"), "names the area: {text}");
        assert!(
            !text.contains("m60_42_36_00"),
            "the raw block id must not survive alongside the name -- the banner is one short line \
             and the id is the part a player cannot read: {text}"
        );
        assert!(text.contains("elsewhere"), "still says why: {text}");
    }

    #[test]
    fn the_block_id_is_the_fallback_when_nothing_has_a_name_yet() {
        // Before the world map is opened NOTHING has a name, which is the same condition that makes
        // `area` mode fail closed. A rejection with no place at all would be worse than an
        // unfriendly one, so the id stays as the fallback rather than being dropped.
        let mut notice = RejectNotice::new();
        let text = notice
            .observe(true, LIMGRAVE, RejectReason::NothingToMatchAgainst, None)
            .expect("announced");
        assert!(text.contains("m60_42_36_00"), "{text}");
        assert!(text.contains("open your map"), "{text}");
    }

    #[test]
    fn an_empty_name_falls_back_rather_than_printing_nothing() {
        // A resolver that returns Some("") would otherwise render "Rejected  (elsewhere)", which
        // reads as a bug in the mod rather than as a rejection.
        let mut notice = RejectNotice::new();
        let text = notice
            .observe(true, LIMGRAVE, RejectReason::WrongPlaceName, Some(""))
            .expect("announced");
        assert!(text.contains("m60_42_36_00"), "{text}");
    }

    /// THE POINT OF THE MODULE. Seamless retries every ~20s and the same wrong place recurs; a
    /// banner every time is wallpaper, and a notification the player ignores is worse than none.
    #[test]
    fn consecutive_rejections_at_the_same_place_stay_silent() {
        let mut notice = RejectNotice::new();
        assert!(
            notice
                .observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None)
                .is_some()
        );
        for _ in 0..20 {
            assert_eq!(
                notice.observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None),
                None
            );
        }
        assert_eq!(notice.suppressed(), 20);
    }

    #[test]
    fn a_different_place_is_new_information_and_is_announced() {
        let mut notice = RejectNotice::new();
        notice.observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None);
        let text = notice
            .observe(true, ELSEWHERE, RejectReason::WrongPlaceName, None)
            .expect("announced");
        assert!(text.contains("m21_01_00_00"), "{text}");
        assert_eq!(
            notice.suppressed(),
            0,
            "the suppressed run resets on a change"
        );
    }

    /// Returning somewhere announced earlier is announced again: the player's question is where
    /// they are being sent NOW, not where they have ever been sent.
    #[test]
    fn returning_to_an_earlier_place_announces_again() {
        let mut notice = RejectNotice::new();
        notice.observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None);
        notice.observe(true, ELSEWHERE, RejectReason::WrongPlaceName, None);
        assert!(
            notice
                .observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None)
                .is_some()
        );
    }

    /// Disabled must still ADVANCE the state. Otherwise switching the option on mid-session would
    /// announce a place the player was rejected from minutes ago, as though it had just happened.
    #[test]
    fn disabled_stays_silent_but_still_tracks_where_we_are() {
        let mut notice = RejectNotice::new();
        assert_eq!(
            notice.observe(false, LIMGRAVE, RejectReason::WrongPlaceName, None),
            None
        );
        // Same place, now enabled: still silent, because we already know we are being sent there.
        assert_eq!(
            notice.observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None),
            None
        );
        // Somewhere new, though, is genuinely new.
        assert!(
            notice
                .observe(true, ELSEWHERE, RejectReason::WrongPlaceName, None)
                .is_some()
        );
    }

    /// A fresh search is a fresh question; the first rejection of it is worth hearing again.
    #[test]
    fn a_reset_makes_the_next_rejection_speak_up() {
        let mut notice = RejectNotice::new();
        notice.observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None);
        assert_eq!(
            notice.observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None),
            None
        );
        notice.reset();
        assert!(
            notice
                .observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None)
                .is_some()
        );
        assert_eq!(notice.suppressed(), 0);
    }

    /// The banner is width-bounded by the donor string's allocation, so the text has to stay short
    /// or it is silently truncated. Checked across EVERY reason, not one -- the bound is only
    /// meaningful if it holds for the longest phrase, and adding a wordier reason later is exactly
    /// how this would regress unnoticed.
    #[test]
    fn every_message_stays_short_enough_for_the_banner() {
        for reason in [
            RejectReason::WrongBlock,
            RejectReason::WrongPlaceName,
            RejectReason::NotNamed,
            RejectReason::CandidateUnnamed,
            RejectReason::NothingToMatchAgainst,
            RejectReason::ExcludedByUser,
        ] {
            let mut notice = RejectNotice::new();
            let text = notice
                .observe(true, LIMGRAVE, reason, None)
                .expect("announced");
            assert!(
                text.chars().count() <= 40,
                "banner text must stay short; {reason:?} gave {} chars: {text}",
                text.chars().count()
            );
            assert!(
                text.contains("m60_42_36_00"),
                "still names the place: {text}"
            );
        }
    }

    /// The reason is the whole point of the notice: WHERE alone does not tell the player whether to
    /// move, un-exclude a place, or open their map.
    #[test]
    fn the_message_names_why_not_just_where() {
        let mut notice = RejectNotice::new();
        let excluded = notice
            .observe(true, LIMGRAVE, RejectReason::ExcludedByUser, None)
            .expect("announced");
        assert!(excluded.contains("excluded"), "{excluded}");
        let mut notice = RejectNotice::new();
        let elsewhere = notice
            .observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None)
            .expect("announced");
        assert_ne!(
            excluded, elsewhere,
            "different causes must read differently"
        );
    }

    /// A different REASON at the same place is new information -- the place stopped being refused
    /// for the old cause -- so it is announced rather than suppressed as a repeat.
    #[test]
    fn a_new_reason_at_the_same_place_is_announced() {
        let mut notice = RejectNotice::new();
        assert!(
            notice
                .observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None)
                .is_some()
        );
        assert!(
            notice
                .observe(true, LIMGRAVE, RejectReason::ExcludedByUser, None)
                .is_some()
        );
    }

    #[test]
    fn a_success_is_announced_with_the_place_name() {
        let mut notice = RejectNotice::new();
        let text = notice
            .observe_success(true, LIMGRAVE, Some("Limgrave"))
            .expect("announced");
        assert!(
            text.starts_with("Invasion successful"),
            "says what happened: {text}"
        );
        assert!(text.contains("Limgrave"), "names the place: {text}");
    }

    #[test]
    fn a_success_falls_back_to_the_block_id_like_a_rejection_does() {
        let mut notice = RejectNotice::new();
        let text = notice
            .observe_success(true, LIMGRAVE, None)
            .expect("announced");
        assert!(text.contains("m60_42_36_00"), "{text}");
        let empty = RejectNotice::new()
            .observe_success(true, LIMGRAVE, Some(""))
            .expect("announced");
        assert!(
            empty.contains("m60_42_36_00"),
            "an empty name must not print nothing: {empty}"
        );
    }

    #[test]
    fn a_success_clears_the_rejection_latch_so_a_later_rejection_speaks() {
        // THE REPORTED BUG. Rejected at a place, invaded successfully, rejected at that same place
        // again: the third event is news and was being swallowed as a repeat of the first.
        let mut notice = RejectNotice::new();
        assert!(
            notice
                .observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None)
                .is_some()
        );
        assert!(notice.observe_success(true, ELSEWHERE, None).is_some());
        assert!(
            notice
                .observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None)
                .is_some(),
            "a rejection after a success is new information, not a repeat"
        );
    }

    #[test]
    fn a_success_after_rejections_is_always_announced() {
        let mut notice = RejectNotice::new();
        for _ in 0..5 {
            notice.observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None);
        }
        assert!(
            notice
                .observe_success(true, LIMGRAVE, Some("Limgrave"))
                .is_some(),
            "arriving where you were previously rejected is the whole point of the hunt"
        );
    }

    #[test]
    fn the_same_success_twice_stays_quiet() {
        let mut notice = RejectNotice::new();
        assert!(notice.observe_success(true, LIMGRAVE, None).is_some());
        assert!(
            notice.observe_success(true, LIMGRAVE, None).is_none(),
            "one arrival, one banner"
        );
    }

    #[test]
    fn a_success_clears_the_suppressed_run_it_ended() {
        let mut notice = RejectNotice::new();
        notice.observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None);
        notice.observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None);
        assert_eq!(notice.suppressed(), 1);
        notice.observe_success(true, LIMGRAVE, None);
        assert_eq!(notice.suppressed(), 0, "the run of rejections is over");
    }

    #[test]
    fn a_disabled_notice_still_advances_on_success() {
        // Turning the option on mid-session must not replay an arrival from minutes ago.
        let mut notice = RejectNotice::new();
        assert!(notice.observe_success(false, LIMGRAVE, None).is_none());
        assert!(
            notice.observe_success(true, LIMGRAVE, None).is_none(),
            "the state advanced while silent, so this is still the same arrival"
        );
    }

    #[test]
    fn an_arrival_is_announced_when_the_filter_is_switched_off() {
        let mut notice = RejectNotice::new();
        let text = notice
            .observe_arrival(true, LIMGRAVE, Some("Limgrave"))
            .expect("announced");
        assert!(text.contains("Limgrave"), "names the place: {text}");
        assert!(
            !text.contains("successful") && !text.contains("Rejected"),
            "an unjudged arrival must claim neither a verdict nor an outcome: {text}"
        );
    }

    #[test]
    fn an_arrival_falls_back_to_the_block_id_like_the_others() {
        let mut notice = RejectNotice::new();
        let text = notice
            .observe_arrival(true, LIMGRAVE, None)
            .expect("announced");
        assert!(text.contains("m60_42_36_00"), "{text}");
    }

    #[test]
    fn the_same_arrival_twice_stays_quiet_but_a_new_one_speaks() {
        let mut notice = RejectNotice::new();
        assert!(notice.observe_arrival(true, LIMGRAVE, None).is_some());
        assert!(notice.observe_arrival(true, LIMGRAVE, None).is_none());
        assert!(notice.observe_arrival(true, ELSEWHERE, None).is_some());
    }

    #[test]
    fn an_arrival_and_a_verdict_do_not_suppress_each_other() {
        // The three kinds share one latch, so each must count as news after either other kind.
        let mut notice = RejectNotice::new();
        assert!(notice.observe_arrival(true, LIMGRAVE, None).is_some());
        assert!(
            notice
                .observe(true, LIMGRAVE, RejectReason::WrongPlaceName, None)
                .is_some(),
            "a rejection after an arrival at the same block is a different statement"
        );
        assert!(
            notice.observe_success(true, LIMGRAVE, None).is_some(),
            "and so is an arrival that became a real invasion"
        );
    }

    #[test]
    fn a_disabled_notice_still_advances_on_arrival() {
        let mut notice = RejectNotice::new();
        assert!(notice.observe_arrival(false, LIMGRAVE, None).is_none());
        assert!(
            notice.observe_arrival(true, LIMGRAVE, None).is_none(),
            "turning the notice on must not replay an arrival from minutes ago"
        );
    }
}
