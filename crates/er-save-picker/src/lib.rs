//! Product (A): the DLL-drawn boot save picker, its shared row model, and the OS-native
//! common-file-dialog mechanism.
//!
//! The save-picker crate extraction has moved the host-testable row model, config keys,
//! slot parser, reusable OS common-file-dialog mechanism, and DLL-drawn boot overlay here.
//! Product entrypoints still live under `er-effects-rs/src/experiments/startup_hooks/save_picker/`
//! until the remaining quit-menu seams are extracted.
//!
//! Contents and remaining planned moves:
//! * `model` -- moved from `experiments/save_picker.rs`: `SavePickerModel`,
//!   `PickerIntent`, `PickerRow`, `PickerEntry`, `PickerActivation`, `PickRejection`, the
//!   dense row layout (`entry_row_base` and everything derived from it), drive/page
//!   cycling, `save_picker_accepts` / `save_picker_extension_accepted`, the civil-time
//!   helpers, `truncate_utf16`, and the process-wide `ACTIVE_SAVE_PICKER` slot. Pure
//!   filesystem logic with ~960 lines of its own tests -- host-runnable, which is the
//!   point: the cancel/reopen state machine is only exercisable by launching the whole
//!   game today.
//! * `slots` -- `SaveSlotInfo` + `parse_save_character_slots`, moved from
//!   `startup_hooks/loading_cover/loading_cover_save_slot.rs` because they are picker-owned
//!   offline save-container parsing.
//! * `overlay` -- moved from `experiments/gpu_readback/save_picker_overlay.rs`: the
//!   arm/disarm lifecycle keyed off the missing-save hold, both input paths (the
//!   render-thread `GetAsyncKeyState`/XInput poll and the dedicated `WH_KEYBOARD_LL`
//!   thread), the file stage and the character sub-stage, the CPU compositor
//!   (`overlay_save_picker_onto`), the explicit [`overlay::arm_boot_picker`] entrypoint,
//!   and the deferred pick completion that runs the redirect install on the game-task
//!   thread.
//! * `os_dialog` -- moved from the mechanism half of
//!   `startup_hooks/save_picker/save_picker_os_dialog.rs`: `os_dialog_run`,
//!   `os_pick_validated`, `classify_os_outcome`, `should_reopen`, `os_dialog_filter`,
//!   `OsDialogClaim`, `os_dialog_owner`, `os_pick_path_from_buffer`, `OsPickOutcome`. It
//!   converts strings and calls comdlg32, with product state supplied through host callbacks
//!   and a caller-supplied cover factory. Its two System>Quit entrypoints
//!   (`os_open_save_picker_load`, `os_open_save_dest_picker`) are not here -- they are
//!   quit-menu callers and still live in the product shim for now.
//! * `config` -- the three picker keys and their plumbing, moved out of
//!   `er-effects-rs/src/config.rs`: `preferred_save_picker_dir`,
//!   `autoupdate_preferred_picker_dir` and `os_native_save_picker` (with its
//!   `use_os_file_picker` / `save_picker.os_native` aliases), their parse + validation,
//!   the generated boilerplate doc text, and `remember_preferred_save_picker_dir`. Only
//!   picker code reads them, so they move with the picker; the product's `er-effects.toml`
//!   parser keeps one file and delegates those keys here.
//! * `surface` -- the pure surface/outcome routing types (`PickerOpenRequest`,
//!   `PickerSurface`, `PickerOpenOutcome`) and destination target routing (`DestRoute`).
//!   The host product still opens native menu windows and stages save-flow state.
//! * `boot` -- boot missing-save telemetry state values and the pure OS-dialog abort
//!   decision table; the host product still owns the dialog thread, telemetry flush, and
//!   process exit.
//!
//! # The screen cover is the CALLER's decision, not this crate's
//!
//! Under the 2026-07-30 user decision the dim belongs to product (B) and the boot dialog
//! must NOT be dimmed. The extracted `os_pick_validated` preserves that caller-decides
//! shape through the [`host::PickerCoverFactory`] seam: the System>Quit product shim passes
//! a factory that arms its dim, and the boot flow passes [`os_dialog::no_picker_cover`].
//! Same behavior, expressed across a crate boundary instead of an in-crate enum.
//!
//! # The OS-native surface is a REQUIREMENT, not an option
//!
//! We never force a user onto an in-game picker we built (user principle 2026-07-30).
//! Both places that draw one -- this crate's boot picker and `er-quit-menu`'s in-game
//! browse rows -- must offer the OS-native dialog as a selectable surface. That is why the
//! comdlg32 mechanism and the `os_native_save_picker` config key live HERE, in the crate
//! both products depend on, rather than being duplicated: `er-quit-menu` gets the fallback
//! surface through its one-way dependency on this crate.
//!
//! # Linking this crate is not arming it
//!
//! `er-quit-menu` statically links this crate, so a profile containing ONLY the standalone
//! product-(B) DLL still offers the OS-native surface. It must NOT thereby acquire (A)'s
//! boot missing-save behavior. Two mechanisms, and only the second is a guarantee:
//!
//! 1. the `boot-flow` cargo feature, which `er-quit-menu` turns off. This isolates the
//!    standalone-(B) build, but cargo UNIFIES features across a build graph, so in the
//!    product DLL -- which wants `boot-flow` -- `er-quit-menu` gets it too. A feature alone
//!    therefore cannot carry the requirement.
//! 2. an explicit arm entry point. Nothing in the boot flow installs a hook, spawns a
//!    thread or arms a model until a host calls it; `er-quit-menu` never does. This holds
//!    in every build, feature unification included, and is the real guarantee.
//!
//! # Product state crosses the seam as injected function pointers
//!
//! See [`host::install_host`]. This crate must not depend on the root crate.

pub mod boot;
pub mod config;
pub mod host;
pub mod model;
#[cfg(feature = "os-dialog")]
pub mod os_dialog;
#[cfg(feature = "boot-flow")]
pub mod overlay;
pub mod slots;
pub mod surface;

pub use boot::*;
pub use config::*;
pub use host::*;
pub use model::*;
#[cfg(feature = "os-dialog")]
pub use os_dialog::*;
pub use slots::*;
pub use surface::*;
