//! A save picked after boot: giving it a record, then defending that record.
//!
//! The boot save-data job has already run by the time a user picks a container, so the
//! picked save has no `CS::ProfileSummary` record and the native Continue row has nothing
//! to load. These count the title-time re-read that writes one, the drift watch that
//! catches the game's own boot read overwriting it with the stale summary, and the
//! title-time deserialize a correct run never makes.
//!
//! Split out of `counters.rs` as a pure code move: nothing is renamed and no initial value
//! changes. Every name here is re-exported from `er_telemetry_core::counters` with a glob, so
//! each consumer still spells it `er_telemetry_core::counters::<name>`.

use std::sync::atomic::{AtomicU64, AtomicUsize};

// ---- picked/loose save source: title-time ProfileSummary re-read + deser accounting -------------
//
// A save the user picks after boot has no `CS::ProfileSummary` record: the boot save-data job that
// would have read one already ran and passed through. Without a record the native Continue row has
// nothing to load, which is why every picked save used to be routed into a title-time deserialize
// -- `0x14067b290`, a function whose only caller in the whole image is `CS::MoveMapStep::DoSaveStuff`
// (in-world). These counters make both halves of that visible.

/// Total autoload ticks that entered the direct-source summary refresh (throttle denominator).
pub static PICKED_SUMMARY_REFRESH_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Real re-read attempts made (a file read + record rewrite), not ticks. Capped.
pub static PICKED_SUMMARY_REFRESH_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
/// How the picked container's summary came to be readable, if it did:
/// 0 = not (yet) readable, 1 = the game's own boot save-data read had already populated it,
/// 2 = this DLL re-read it from the staged container at the title.
pub static PICKED_SUMMARY_REFRESH_STATE: AtomicUsize = AtomicUsize::new(0);
/// Bitmask of slots whose records the re-read rewrote (bit N = slot N).
pub static PICKED_SUMMARY_REFRESH_SLOT_MASK: AtomicUsize = AtomicUsize::new(0);

// ---- record drift watch: did something overwrite the body-derived records? -----------------------
//
// The container carries two descriptions of a slot -- the `USER_DATA010` summary table the game
// deserializes, and the body that actually loads -- and they can disagree (measured: 6 of 10 slots
// on `100-Lilbro/ER0000.co2`, run br-20260903-204517-82d2). This DLL writes the body-derived
// version; the game's own boot read can then overwrite it with the stale one, and the loading
// screen renders whoever the record names. These make that overwrite visible and counted.

/// Ticks on which the drift watch found the target slot's record naming a different character than
/// the container's body gives it. **A correct run reports 0.**
pub static PICKED_SUMMARY_RECORD_DRIFTS: AtomicUsize = AtomicUsize::new(0);
/// Body-derived record rewrites performed because of drift (capped by `REASSERT_MAX_REWRITES`).
pub static PICKED_SUMMARY_REASSERTS: AtomicUsize = AtomicUsize::new(0);
/// FNV-1a 64 of the target slot's name in the container body, plus its level -- the identity the
/// drift watch defends. `0` until a container has been read.
pub static PICKED_SUMMARY_BODY_NAME_HASH: AtomicU64 = AtomicU64::new(0);
/// Rune Level of the target slot in the container body. `0` until a container has been read.
pub static PICKED_SUMMARY_BODY_LEVEL: AtomicUsize = AtomicUsize::new(0);
/// `PICKED_SUMMARY_REFRESH_TICKS` at the refresh that armed the watch, plus 1 (`0` = unarmed).
pub static PICKED_SUMMARY_WATCH_ARMED_TICK: AtomicUsize = AtomicUsize::new(0);

/// Calls into the title-time save deserialize `0x14067b290` from the full-read chain.
///
/// **A correct run reports 0.** Non-zero means a save was deserialized at the boot title rather
/// than in-world from `CS::MoveMapStep::DoSaveStuff`, which is the crash this counter exists to
/// stop being silent about (`gaitemInsTable[-1]` AV at `0x67141a`). It counts the call being made,
/// not the call surviving, so a run that dies inside the deserialize still leaves the 1 behind.
pub static TITLE_TIME_DESER_CALLS: AtomicUsize = AtomicUsize::new(0);
/// Slot (+1, so 0 means "never") passed to the most recent title-time deserialize.
pub static TITLE_TIME_DESER_LAST_SLOT: AtomicUsize = AtomicUsize::new(0);

/// Times the switch retired its own `menuData+0x5d` return-title request before creating the
/// incoming world. Non-zero on a switch means the request was served and cleared; a switch that
/// completes with this at 0 left the request set, which is the black-screen precondition (the
/// incoming child inherits it, walks 18->20, and `STEP_GameStepWait` tears the world down).
pub static SWITCH_RETURN_TITLE_REQUEST_RETIRED_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Times a genuinely loaded world reverted to the title/new-game map default -- the black screen,
/// counted as a transition (real map id -> `FULLREAD_C30_M10_DEFAULT`) rather than as a level, so
/// the long stretch of every boot that legitimately sits at the default cannot trip it.
///
/// This is the run-stopping oracle for the second-load teardown. It is deliberately blind to how
/// the switch was driven, so a run driven through the real ProfileSelect rows and a run driven by
/// the diagnostic control file are scored by the same measurement.
pub static WORLD_LOST_TO_TITLE_COUNT: AtomicUsize = AtomicUsize::new(0);
