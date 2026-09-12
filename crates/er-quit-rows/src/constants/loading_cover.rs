// === Deterministic menu input probe: re-export facade ===========================================
// The probe schedule constants (INPUT_PROBE_DOWN_START, INPUT_PROBE_CONFIRM_START, ...) and the
// d180 leaf-tick counter MENU_D180_LEAF_TICKED moved to
// `er_loading_portrait_core::menu_input_probe` with the loading-cover crate extraction, under a
// name that matches what they are; the file name here predates the title-cover constants being
// split out of it.
//
// `constants.rs` already carries `pub(crate) use er_loading_portrait_core::*;`, so every name is
// back in this flat namespace without a shim here; a `pub(crate) use` in this file would re-export
// nothing and rustc would flag it as unused. This file stays as the navigation marker for the path
// the constants used to occupy.
