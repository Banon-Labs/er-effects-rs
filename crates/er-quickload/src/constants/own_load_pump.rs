// The path-B private-pump ("own the load") constant table moved verbatim into the
// er-title-flow crate (crates/er-title-flow/src/constants_own_load_pump.rs) with the
// autoload/title-flow slice. Only visibility changed (`pub(crate)` -> `pub`); the single
// `pub(crate) use er_title_flow::*;` shim in constants.rs re-exports the whole table into this
// module, so every `crate::constants::NAME` and flat-namespace reference resolves unchanged.
// This file is kept as the include! site so the constants.rs include list -- and the ownership
// row in docs/plans/crate-extraction-execution-roadmap.md -- stay where reviewers expect them.
