// The native-load / full-save-read / own-stepper autoload constant table moved verbatim into
// the er-title-flow crate (crates/er-title-flow/src/constants_autoload_state.rs) with the
// autoload/title-flow slice. Only visibility changed (`pub(crate)` -> `pub`); the single
// `pub(crate) use er_title_flow::*;` shim in constants.rs re-exports the whole table into this
// module, so every `crate::constants::NAME` and flat-namespace reference resolves unchanged.
//
// What did not move is the generated-prologue include below. Every `*_SIG` it emits is assembled
// by this crate's `build.rs` from named `iced-x86` instructions (see build-support/
// prologue_build.rs and scripts/check-prologue-bytes.py) and is consumed by root-crate quit-menu
// files -- `save_flow_boxes.rs` (MENU_JOB_EMIT_RESULT_SIG, the six SYSTEM_QUIT_MSGBOX_* sigs) and
// `system_quit_dialog_handlers.rs` (SAVE_REQUEST_RETRACT_B72/B73_SIG). Nothing in the moved table
// reads one, so carrying the include across would have moved the msgbox and save-request
// prologues -- and the build.rs that generates them -- into the title-flow crate as an accident of
// where the include happened to sit.
//
// Every `*_SIG` prologue below is assembled from named instructions by this crate's `build.rs`
// and, when a copy of `eldenring-deobf.bin` is present, compared against the real image at the
// same VA. Hand-typing them is what the generator exists to prevent: a prologue that is one byte
// wrong fails its own install-time check and disarms the hook silently.
include!(concat!(
    env!("OUT_DIR"),
    "/generated_autoload_state_prologues.rs"
));
