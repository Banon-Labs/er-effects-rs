//! Product (B): the customized post-autoload vanilla System>Quit menu.
//!
//! S7 has moved the pure decision core for rows, save-destination identity/commit helper
//! decisions, and save-flow confirm-box decisions into this crate. Runtime-native hook surfaces
//! still live in `er-effects-rs` shims until S8/S9.
//!
//! Planned contents, moved from the root DLL in slices (line counts are from the extraction plan,
//! and the per-file product/diagnostic split is in the plan doc -- several of these files move
//! only in part):
//! * `rows` -- `startup_hooks/quit_menu/system_quit_row_identity.rs` (921, ~100% product): the
//!   `QuitRow`/`QuitRowFacts`/`QuitRowVerdict` resolver and the single gate on the
//!   irreversible `ExitProcess(0)`. Fully host-testable; the cleanest thing here.
//! * `dialog_handlers` -- `startup_hooks/quit_menu/system_quit_dialog_handlers.rs` (1601): the
//!   AddCancelButton row CLONER, the row ROUTER (which is also what suppresses the native
//!   Return-to-Desktop action), the Save Game label substitution and its save calls, the
//!   ProfileSelect submit, and the PR-#103 Scaleform ctor/dtor double-free skip.
//! * `browse` -- `startup_hooks/quit_menu/save_picker_menu.rs` (944): the in-game
//!   `05_010_ProfileSelect` browse surface (row staging, activation, menu-pump rebuild and
//!   resubmit, browse stats lines, the list-builder re-stage hook).
//! * `surface` -- `startup_hooks/save_picker/save_picker_surface.rs` (222): THE one place that decides
//!   which picker surface opens, plus the destination decisions both surfaces share.
//! * `dim` -- `startup_hooks/quit_menu/save_picker_dim_overlay.rs` (876): the layered GDI window
//!   that covers the game while an OS dialog is up. IN-GAME QUIT-MENU CASE ONLY.
//! * `os_entry` -- the two System>Quit entrypoints in the product shim
//!   `startup_hooks/save_picker/save_picker_os_dialog.rs` (`os_open_save_picker_load`,
//!   `os_open_save_dest_picker`). The comdlg32 mechanism they call now lives in
//!   `er-save-picker::os_dialog`; this crate supplies the dim as its cover.
//! * `save_flow` -- `startup_hooks/save_flow_boxes.rs` (790), `save_dest_identity.rs`
//!   (465), `save_dest_commit.rs` (1286), and the `save_flow_tick` stage machine currently
//!   at `experiments/lifecycle.rs`.
//! * `profile_preview` -- the quit-menu half of
//!   `startup_hooks/quit_menu/save_swap_profile_table.rs` (1050): the foreign-save ProfileSummary
//!   preview, the swap/restore, and the recommit after the return-title save. Its
//!   `force_profile_render_tick` half is PRODUCT (the loading-screen profile-model render
//!   drive, called from the task registration and the title tick) and stays behind.
//! * `install` -- `install_system_quit_duplicate_button_hook`, today in
//!   `startup_hooks/diagnostics/layout_global_hooks.rs`: the single entry point that installs the
//!   whole feature.
//!
//! # Boundary this crate does NOT cross: save suppression
//!
//! "The Save Game menu row and the flow it drives" is this crate. "Save suppression and
//! save-redirect internals" is NOT: `er-save-suppress` is already its own crate with its
//! own standalone `er-save-disable-dll`, and `experiments/save_redirect/*` stays in the
//! product. This crate only asks, through the seam, for the one-shot bypass that makes the
//! Save Game row the single path that really writes.
//!
//! Product state crosses the seam as injected function pointers: see
//! [`host::install_host`]. This crate must not depend on the root crate.

pub mod host;
pub use host::*;

// S7 decision-core modules moved from the product DLL. Product callsites keep
// stable shim names until the hooked surfaces move in S8.
pub mod rows;
pub mod save_dest_commit;
pub mod save_dest_identity;
pub mod save_flow_boxes;

#[cfg(windows)]
pub mod dim;
#[cfg(windows)]
pub use dim::*;
#[cfg(windows)]
pub mod os_dialog;
#[cfg(windows)]
pub use os_dialog::*;

pub use rows::*;
pub use save_dest_commit::*;
pub use save_dest_identity::*;
pub use save_flow_boxes::*;
