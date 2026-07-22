//! Destination for the ~900 telemetry atomic counters/latches being inverted
//! out of the product's `experiments/*` + `constants/*` trees.
//!
//! OWNERSHIP INVERSION (in progress): today these atomics are DEFINED in the
//! product and telemetry merely mirrors them through `crate::*` glob imports.
//! The target state is that they are DEFINED here (`pub` statics) and the
//! product write-sites reference `er_telemetry::counters::X`, so telemetry never
//! reaches up into product for state.
//!
//! This module currently holds only the counters that the standalone read-side
//! tick needs; the bulk migration (own_load / move_probe / rawinput / profile /
//! depth families) lands file-group by file-group per the plan's Step 3.

use std::sync::atomic::AtomicU64;

/// Number of standalone read-side ticks that have executed (proves the game-thread
/// callback is live in the telemetry-only DLL). Owned here from the start.
pub static STANDALONE_TICKS: AtomicU64 = AtomicU64::new(0);
