//! The banner the player actually sees, and the one latch behind all three of its messages.
//!
//! Split out of `local_invasion_filter` on 2026-09-08 when that file crossed the 3200-line hard
//! limit. The cut is along a real seam rather than a convenient line number: everything here
//! answers "what does the player get told", and nothing here decides anything. The judging, the
//! session identification and the driving of ERSC's own actions all stay next door.
//!
//! All three announcements share [`super::REJECT_NOTICE`] on purpose, so the surface cannot
//! contradict itself about what it last said -- a rejection notice still on screen while an
//! arrival notice is written would read as the mod having rejected the invasion it just let
//! through.

use std::sync::atomic::Ordering;

use super::{NOTICE_FAILED, REJECT_NOTICE, RejectReason};

/// Host-side stub: there is no game to show a banner in, and the decision half is tested directly
/// against [`er_invasion_warp_core::reject_notice`] rather than through this.
#[cfg(not(windows))]
pub(super) fn announce_rejection(_enabled: bool, _destination: u32, _reason: RejectReason) {}

#[cfg(not(windows))]
pub(super) fn announce_verdict(_enabled: bool, _destination: u32, _reason: RejectReason) {}

/// Host-side stub.
#[cfg(not(windows))]
pub(super) fn announce_arrival(_enabled: bool, _destination: u32) {}

/// Report a destination that arrived while the filter was switched off.
///
/// Shares the one notice latch with the verdict banners, so the surface never contradicts itself
/// about what it last said.
#[cfg(windows)]
pub(super) fn announce_arrival(enabled: bool, destination: u32) {
    let announcement = {
        let mut guard = match REJECT_NOTICE.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let place = crate::place_name::place_name_for_block(destination);
        guard.observe_arrival(enabled, destination, place.as_deref())
    };
    let Some(text) = announcement else {
        return;
    };
    // SAFETY: game thread, inside the join-data hook -- the same context and surface as the
    // verdict banners.
    if !unsafe { crate::announce::show(&text) } {
        if NOTICE_FAILED.swap(true, Ordering::SeqCst) {
            return;
        }
        crate::standalone_log(format_args!(
            "local-invasion: could not show the arrival banner (\"{text}\") -- the message \
             functions did not verify, or the menu is not up yet."
        ));
    }
}

/// Host-side stub; the decision half is tested against [`er_invasion_warp_core::reject_notice`].
#[cfg(not(windows))]
pub(super) fn announce_success(_enabled: bool, _destination: u32) {}

/// Put a successful invasion on the same banner the rejections use.
///
/// Shares [`RejectNotice`] with [`announce_rejection`] on purpose: one banner, one memory of what
/// it last said. That is what lets an arrival clear the rejection latch, so a later rejection at
/// the same place is announced instead of being swallowed as a repeat.
#[cfg(windows)]
pub(super) fn announce_success(enabled: bool, destination: u32) {
    let announcement = {
        let mut guard = match REJECT_NOTICE.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let place = crate::place_name::place_name_for_block(destination);
        guard.observe_success(enabled, destination, place.as_deref())
    };
    let Some(text) = announcement else {
        return;
    };
    // SAFETY: game thread, inside the join-data hook -- the same context, and the same auto-closing
    // announcement surface, as the rejection banner.
    if !unsafe { crate::announce::show(&text) } {
        if NOTICE_FAILED.swap(true, Ordering::SeqCst) {
            return;
        }
        crate::standalone_log(format_args!(
            "local-invasion: could not show the success banner (\"{text}\") -- the message \
             functions did not verify, or the menu is not up yet. The invasion still happened; \
             only the on-screen notice is missing."
        ));
    }
}

/// Put a rejection on the game's system-message banner, if the player asked for that.
///
/// The decision of whether to speak lives in [`er_invasion_warp_core::reject_notice`] and is unit-tested
/// on the host; this only carries the answer to the screen. The notice is fed even when the option
/// is off so that turning it on mid-session does not announce a place the player was rejected from
/// minutes ago as though it had just happened.
///
/// Runs on the game thread, in the same call that judges the match -- which is the context
/// `showPopupMenu` expects, and it null-checks the menu manager itself, so a message raised before
/// the UI exists is dropped rather than faulting.
#[cfg(windows)]
pub(super) fn announce_rejection(enabled: bool, destination: u32, reason: RejectReason) {
    let announcement = {
        let mut guard = match REJECT_NOTICE.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        // Resolve the area's own name for the banner. Done here rather than inside the notice so
        // that type stays testable off the game: this is a call into the message repository.
        //
        // `None` before the world map has been read this session, which is the same condition that
        // makes `area` mode fail closed -- the notice falls back to the block id, which is
        // unfriendly but true.
        let place = crate::place_name::place_name_for_block(destination);
        guard.observe(enabled, destination, reason, place.as_deref())
    };
    let Some(text) = announcement else {
        return;
    };
    // The game's own auto-closing announcement surface -- the "Grace discovered" one. Not
    // `system_message`/`showPopupMenu`, which is a blocking modal with an OK button: shipping that
    // gave the user a dialog to dismiss per rejection, showing squares and then nothing, and the
    // unattended dialog held the session open long enough to trip the stall watchdog.
    //
    // SAFETY: game thread, inside the join-data hook. Writes the live view's embedded message,
    // which is exactly what the view's own Update does when it pops one. Both game functions are
    // byte-checked before use.
    if !unsafe { crate::announce::show(&text) } {
        // Once, not per rejection: a banner that cannot be shown is a missing convenience, and
        // saying so every 20 seconds would be its own spam.
        if NOTICE_FAILED.swap(true, Ordering::SeqCst) {
            return;
        }
        crate::standalone_log(format_args!(
            "local-invasion: could not show the rejection banner (\"{text}\") -- the message \
             functions did not verify, or the menu is not up yet. Rejections still work; only the \
             on-screen notice is missing."
        ));
    }
}

/// Say, the moment a match is judged, that it is not one the filter wanted.
///
/// # Why this is separate from [`announce_rejection`]
///
/// Because the verdict and the enforcement are two facts and one banner cannot carry both without
/// lying about one of them. That has now been got wrong in both directions on live sessions:
/// announcing "Rejected" at the verdict told the player an invasion had been stopped when the
/// cancel then failed and it proceeded (2026-09-04), and moving the banner behind a successful
/// cancel meant an uncancellable rejection showed nothing at all, which reads exactly like the mod
/// not being loaded (2026-09-09).
///
/// So this one states only what is certainly true at the instant it fires -- this match is not
/// local -- and never claims anything was stopped. `announce_rejection` still fires from
/// `drive_pending_cancel` when a cancel actually lands, and that one may say so.
///
/// Deduplicated by destination, because a rejection is judged once but the tick can revisit it.
#[cfg(windows)]
pub(super) fn announce_verdict(enabled: bool, destination: u32, reason: RejectReason) {
    if !enabled {
        return;
    }
    if LAST_VERDICT_BLOCK.swap(destination, Ordering::SeqCst) == destination {
        return;
    }
    let place = crate::place_name::place_name_for_block(destination)
        .unwrap_or_else(|| format!("{destination:#010x}"));
    // Short because the announce field is 1728px wide and the first attempt at a message like this
    // measured 1729px, so it was placed successfully and never rendered.
    let text = format!("Not local: {place}");
    // SAFETY: game thread, inside the join-data hook -- the context `announce_rejection` shows
    // from, and `show` byte-checks both game functions before using them.
    if !unsafe { crate::announce::show(&text) } {
        if NOTICE_FAILED.swap(true, Ordering::SeqCst) {
            return;
        }
        crate::standalone_log(format_args!(
            "local-invasion: could not show the verdict banner (\"{text}\") -- reason {reason:?}"
        ));
    }
}

/// The last destination a verdict banner named, so a re-judged match does not repeat it.
#[cfg(windows)]
static LAST_VERDICT_BLOCK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
