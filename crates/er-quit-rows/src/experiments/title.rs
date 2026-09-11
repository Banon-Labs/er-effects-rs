//! Title/autoload/switch flow now lives in the er-title-flow crate
//! (docs/plans/title-flow-crate-extraction.md). This shim preserves the old
//! `experiments::title::*` namespace for every root-crate call site.

pub(crate) use er_title_flow::*;
