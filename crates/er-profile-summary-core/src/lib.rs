//! `CS::ProfileSummary`: reading and writing the ten in-memory character records.
//!
//! # What this is
//!
//! The game deserializes this table once per boot -- `CS::ProfileSummary::Deserialize`
//! (`0x140261f00` -> `0x140261f10`: ten occupancy bytes into `summary+0x8`, then ten records of
//! `0x2a0` from `summary+0x18` via `0x140261cf0`), reached from the boot common-data load
//! (`0x1402570c0`). Everything the player then sees about which characters exist reads that
//! table: the title's native Continue row, the System>Quit "Load Character" /
//! "Load Character from File" rows, the `05_010_ProfileSelect` list, and the loading-screen
//! portrait.
//!
//! # Why it is one crate
//!
//! Because until this crate existed it was one concept smeared across four files of the
//! `er-quickload` shim, each holding one verb of it:
//!
//! | was in | held |
//! |---|---|
//! | `continue_load/slot_resolution.rs` | `profile_slot_fingerprint` -- what a record says |
//! | `loading_cover/loading_cover_save_slot.rs` | the summary pointer, and the serialized-save reader that fills a record |
//! | `quit_menu/save_swap_profile_table.rs` | `write_profile_summary_records_from_save_bytes` -- rebuilding the whole table |
//! | `continue_load/picked_summary_refresh.rs` | the boot re-read that uses all three |
//!
//! The record layout itself is not here: it is `er_game_base::profile_summary`, whose typed
//! `ProfileSummaryRecord` / `ProfileSummaryLayout` carry the compile-time asserts that pin the
//! reverse-engineered 1.16.2 ABI. This crate is the behaviour above that layout.
//!
//! # Seam
//!
//! Product state crosses as injected function pointers: see [`host::install_host`]. This crate
//! must not depend on the root crate, and must not depend on `er-title-flow` (which is its
//! natural consumer -- an edge that way would close a cycle).

// Not re-exported flat: `install_host` / the host struct are named the same in every feature
// crate, and the product glob-imports this crate's root next to `er_loading_portrait_core`'s.
// Install through `er_profile_summary_core::host::install_host`.
pub mod host;

/// Pure, host-testable: is a record a real character?
pub mod slot_identity;
pub use slot_identity::*;

/// Pure, host-testable: when may the picked-save re-read run again, and when has it given up?
pub mod refresh_policy;
pub use refresh_policy::*;

/// Pure, host-testable: has something overwritten the body-derived records with a different
/// character, and may we re-assert them?
pub mod reassert_policy;
pub use reassert_policy::*;

#[cfg(windows)]
pub mod face_data;
#[cfg(windows)]
pub use face_data::*;

#[cfg(windows)]
pub mod live_records;
#[cfg(windows)]
pub use live_records::*;

#[cfg(windows)]
pub mod serialized_slot;
#[cfg(windows)]
pub use serialized_slot::*;

#[cfg(windows)]
pub mod save_bytes_records;
#[cfg(windows)]
pub use save_bytes_records::*;

#[cfg(windows)]
pub mod picked_refresh;
#[cfg(windows)]
pub use picked_refresh::*;
