// === er-tpf Tier-4 in-memory texture wire-up: re-export facade ==================================
// The whole block (CREATE_TPF_RES_CAP_RVA, GLOBAL_TPF_REPOSITORY_RVA, GLOBAL_TEX_REPOSITORY_RVA,
// ER_TPF_COVER_*) moved to `er_loading_portrait_core::tpf_textures` with the loading-cover crate
// extraction, and `CREATE_TPF_RESCAP_RVA` moved out of `constants/anti_debug.rs` with it (one
// literal declaration per game address -- `scripts/check-rva-alias-drift.py`).
//
// `constants.rs` already carries `pub(crate) use er_loading_portrait_core::*;`, so every name is
// back in this flat namespace without a shim here; a `pub(crate) use` in this file would re-export
// nothing and rustc would flag it as unused. This file stays as the navigation marker for the path
// the constants used to occupy.
