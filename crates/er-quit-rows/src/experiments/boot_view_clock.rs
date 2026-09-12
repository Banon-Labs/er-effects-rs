//! The boot-view clock, and nothing else.
//!
//! Every `*_ms` field in this DLL's telemetry is measured against one origin, and this is it: a
//! single `Instant` anchored the first time boot-view code asks for the time. The readers live here
//! rather than in `gpu_readback::boot_progress` because the clock outlived the thing it was named
//! after -- the System>Quit switch guards, the input block, the own-stepper and the native
//! loading-screen exposure all stamp against it, and none of them draws a cover. Leaving the clock
//! inside the cover module made `loading-cover` a feature seven unrelated callers depended on.
//!
//! The cover itself -- the bar, the portrait, the stat block, the compositor -- stays behind
//! `loading-cover`. This module is compiled either way.

/// The origin. Anchored on the first [`boot_view_epoch_ms`] call and never re-anchored.
static BOOT_VIEW_EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Milliseconds since the boot-view epoch, anchoring it on the first call.
pub(crate) fn boot_view_epoch_ms() -> u64 {
    let epoch = *BOOT_VIEW_EPOCH.get_or_init(std::time::Instant::now);
    epoch.elapsed().as_millis().min(u64::MAX as u128) as u64
}

/// The same clock as [`boot_view_epoch_ms`], read without starting it: `None` until boot-view code
/// has anchored the epoch.
///
/// The distinction is not pedantry. `boot_view_epoch_ms` anchors on first call, so a caller outside
/// the boot view that happens to run first would silently move the origin of the clock every
/// telemetry `*_ms` field is measured against -- rewriting the meaning of the whole run's timeline
/// to stamp one event. Callers that only want to read the timeline (the in-game menu open stamp,
/// the clock map) use this and simply decline to stamp while the clock does not exist yet.
pub(crate) fn boot_view_epoch_ms_if_anchored() -> Option<u64> {
    BOOT_VIEW_EPOCH
        .get()
        .map(|epoch| epoch.elapsed().as_millis().min(u64::MAX as u128) as u64)
}
