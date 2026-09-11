//! Loading-portrait armor oracle: product facade (empty by design).
//!
//! The unsafe sampling + telemetry-publication half moved to
//! `er_loading_portrait_core::portrait_equip_oracle` with the loading-cover crate extraction; the
//! pure classification half has lived in `er_loading_portrait_core::portrait_equip` since the
//! portrait crate split. Both are already in this flat namespace through
//! `experiments/startup_hooks.rs`'s `pub(crate) use er_loading_portrait_core::*` glob, so a
//! `pub(crate) use` shim here would re-export nothing and rustc would flag it (the same reason
//! `experiments/gpu_readback.rs` carries no shim). The file stays as the navigation marker for the
//! path the code used to occupy.
