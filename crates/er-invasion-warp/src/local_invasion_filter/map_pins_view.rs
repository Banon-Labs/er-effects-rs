//! How invasion pins are drawn on the world map, and the tally line that reports it.
//!
//! Presentation, like [`super::banner`], and split off for the same reason: `local_invasion_filter`
//! crossed the 3200-line hard limit and the cut has to fall on a real seam. Nothing here decides
//! whether a match is kept or rejected -- it only answers what the map should look like given a
//! decision already made.

use super::{PinAppearance, current_config};
use er_game_base::fnv1a::{fnv1a64, fnv1a64_mix};
use er_invasion_warp_core::local_invasion::LocationChoice;

/// How one map pin should look, given what the filter would do with an invasion landing there.
///
/// Reuses [`LocalInvasionConfig::judge`] rather than re-deriving the rules, so the map cannot tell
/// a different story from the filter. The only thing it adds is separating "kept because the user
/// marked it" from "kept because the mode allows it", which `judge` already distinguishes by
/// reason and which is the distinction the three tiers exist to show.
///
/// A pin whose block is unknown, or a filter that is switched off, reports
/// [`PinAppearance::Eligible`]: with no rules in force nothing is being excluded, and claiming
/// otherwise would paint a map full of rejections for a player who has not asked for any.
#[must_use]
pub fn pin_appearance_for(block: Option<u32>) -> PinAppearance {
    let Some(block) = block else {
        return PinAppearance::Eligible;
    };
    let Some(config) = current_config() else {
        return PinAppearance::Eligible;
    };
    match config.choice_for(block) {
        LocationChoice::Chosen => PinAppearance::Chosen,
        LocationChoice::Untouched => PinAppearance::Eligible,
        LocationChoice::Excluded => PinAppearance::Rejected,
    }
}

/// A hash of everything that can change a pin's icon, for the injection cache's key.
///
/// The map's param rows are built once and shared across views, keyed on the spawn catalog. That
/// key is right for the spawn set and wrong for the icons, because the icon now depends on the
/// user's lists too -- so without this the rows survive a mark and the map never changes. Mixing
/// this in makes a mark invalidate exactly what a mark affects.
///
/// The invasion-attempt state is mixed in for the identical reason one step removed: it selects the
/// bright-or-dimmed half of each tier's frame pair, so a search starting or ending while the map is
/// already open has to invalidate the same cache a mark does. Without it the dim would only ever
/// appear on the next map open, which is exactly the case a player is least likely to hit -- you
/// notice the pins are unclickable by trying them, with the map already in front of you.
#[must_use]
pub fn pin_choice_signature() -> usize {
    let mut hash = fnv1a64(b"");
    let mut mix = |value: u64| {
        hash = fnv1a64_mix(hash, value);
    };
    mix(u64::from(
        er_invasion_warp_core::warp::invasion_attempt_in_flight(),
    ));
    let Some(config) = current_config() else {
        return hash as usize;
    };
    mix(u64::from(config.enabled));
    for block in &config.allowed_blocks {
        mix(u64::from(*block));
        mix(1);
    }
    for block in &config.blocked_blocks {
        mix(u64::from(*block));
        mix(2);
    }
    hash as usize
}

/// Count the tiers an injection produced, so "the map looks the same" is answerable from the log.
///
/// Added after a live run where all three marker frames were provably installed (+66 bytes, three
/// 22-byte placements) and the map was re-injected four times, yet every pin looked identical --
/// and nothing recorded which tier any pin got, so the cause could not be named. The tier is the
/// output of this feature; not logging it repeated the exact mistake that cost three wrong
/// attributions on the filter earlier.
pub fn log_pin_tier_tally(chosen: usize, untouched: usize, excluded: usize) {
    let enabled = current_config().is_some_and(|config| config.enabled);
    crate::standalone_log(format_args!(
        "map-inject: pin tiers chosen={chosen} untouched={untouched} excluded={excluded} \
         (filter_enabled={enabled}). All-one-number means the map cannot show a difference: mark \
         somewhere with Insert or exclude it with Delete."
    ));
}
