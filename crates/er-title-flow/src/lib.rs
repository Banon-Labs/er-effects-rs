// er-title-flow: title/autoload/switch orchestration extracted from the root DLL
// (docs/plans/title-flow-crate-extraction.md). Flat include! layout preserved from
// the source module so diffs stay reviewable; `compat` is the single seam that maps
// the old root-crate glob imports onto this crate's own constants, the shared
// crates, and the `host` function-pointer boundary. title_load_step_hooks.rs is the
// verbatim head of the source title_tick_cover.rs, split at a function boundary so
// both files clear the repo's hard file-size gate.
#![allow(unused_imports)]
#![allow(clippy::missing_safety_doc)]

pub mod boot_hold;
pub mod compat;
pub mod constants_moved;
pub mod host;

pub use constants_moved::*;
pub use host::{TitleFlowHost, install_host};

include!("product_autoload_gates.rs");
include!("title_load_step_hooks.rs");
include!("title_tick_cover.rs");
include!("switch_slot_control.rs");
include!("profile_select_flow.rs");
include!("native_title_job.rs");
include!("dlc_roots_self_heal.rs");
