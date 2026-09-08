//! The d180 leaf-tick counter.
//!
//! Moved from er-quickload `constants/loading_cover.rs` with the loading-cover crate extraction
//! and renamed to what it holds (the old filename predates the split of the title-cover constants
//! out of it). The root re-exports every name through its `constants.rs` glob.
//!
//! # What used to be here
//!
//! This module also carried the DETERMINISTIC menu input probe -- a schedule of injected Down/
//! Confirm taps at known frames (`INPUT_PROBE_DOWN_START`, `INPUT_PROBE_CONFIRM_START`, ...) plus
//! four `er_telemetry_core` counters. All of it was deleted on `main` by "Delete 26 gates that
//! could only ever be false, and the code behind them": the probe's enable gate could never be
//! true, so the schedule and its counters were unreachable instrumentation.
//!
//! This branch predates that deletion, so its move brought a copy of the probe into this crate.
//! Re-landing it here would have resurrected dead code into a fresh crate -- the exact outcome the
//! extraction exists to avoid -- so the merge follows the deletion instead. `main` now has zero
//! `INPUT_PROBE_` references anywhere, and this file is why that stays true.
//!
//! `MENU_D180_LEAF_TICKED` is deliberately not part of that deletion: it is still live, bumped by
//! `cap_menu_item_update_hook` and read in `experiments/trace/menu_constructor_capture.rs`.

use crate::prelude::*;

/// Count of genuine d180 leaf-Update ticks (bumped only by `cap_menu_item_update_hook` when the
/// ticked item classifies to dialog_factory). Distinct from `MENU_LOAD_GAME_ITEM`, which the static
/// sequence-iter walk can also set without d180 actually ticking.
pub static MENU_D180_LEAF_TICKED: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
