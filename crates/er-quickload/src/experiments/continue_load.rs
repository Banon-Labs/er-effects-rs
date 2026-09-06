//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

use super::*;

// `picked_summary_refresh` moved to `er_profile_summary_core::picked_refresh` with the
// ProfileSummary crate extraction. Re-exported HERE, from the module path the product's spine
// (`lib_parts/dll_entry_parts/bootstrap.rs`, `er_title_flow`'s seam install) already names, so the
// move is invisible to every caller.
pub(crate) use er_profile_summary_core::picked_refresh::{
    direct_source_slot_summary_real, refresh_boot_default_profile_summary,
    refresh_direct_source_profile_summary,
};

mod product_continue;
pub(crate) use product_continue::*;

mod slot_resolution;
pub(crate) use slot_resolution::*;
