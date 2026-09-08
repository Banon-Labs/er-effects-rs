//! The own-stepper idx6 (`STEP_GameStepWait`) handler and its `RequestedSlotIdentity` check moved
//! to the er-title-flow crate (crates/er-title-flow/src/own_stepper_idx6_memory.rs) with the
//! autoload/title-flow slice.
//!
//! This file is kept as the module's site so the `#[path]` declaration in `experiments/mod.rs` --
//! and the ownership row in docs/plans/crate-extraction-execution-roadmap.md -- stay where
//! reviewers expect them. It deliberately carries no `pub(crate) use er_title_flow::*;` of its
//! own: `experiments/title.rs` already re-exports that whole crate into `experiments`, so a
//! second glob here re-exports nothing and is an unused import under `warnings = "deny"`.
