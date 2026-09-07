//! The dependency-injection seam between this crate and its host DLL.
//!
//! Same pattern as `er_loading_portrait_core::host` / `er_quit_menu_core::host` /
//! `er_save_picker_core::host`: function pointers installed once at DLL attach, neutral
//! defaults until then, crate-internal wrappers bearing the exact names the moved code
//! already called.
//!
//! Every field below is one measured cross-call into a concept this crate does not own:
//!
//! * the debug log sink;
//! * `GameDataMan`, which the product resolves once and caches -- this crate walks it to the
//!   summary but does not own finding it;
//! * the save-source decisions (`experiments::save_redirect`), which decide whether a picked
//!   or loose `save_file` is driving this boot and which staged container it resolves to;
//! * the autoload slot decision (`experiments::continue_load::slot_resolution`), which folds
//!   the picker's pick, the trigger file and the config together;
//! * the per-slot name/stats/map/place-name caches, which belong to the ProfileSelect
//!   stats-text surface, not to the records -- they are fed from the same container bytes but
//!   are read by the rows, so the row surface keeps them.
//!
//! A cross-call whose only consumers are inside this crate is a move, not a seam entry (the
//! 2026-07-30 rule that no extracted crate reaches back into `er-quickload`).

use std::path::PathBuf;
use std::sync::OnceLock;

/// Host callbacks the ProfileSummary surface reads through the seam. Every field has a neutral
/// default (see [`ProfileSummaryHost::defaults`]); hosts overwrite the ones they own.
#[derive(Clone, Copy)]
pub struct ProfileSummaryHost {
    /// Structured debug logging sink (the product's `append_autoload_debug`).
    pub append_autoload_debug: fn(std::fmt::Arguments<'_>),

    /// The live `CS::GameDataMan`, or 0. The summary hangs off it at
    /// `GAME_DATA_MAN_PROFILE_SUMMARY_OFFSET`; walking that edge is this crate's job, but
    /// resolving the singleton is the product's.
    pub game_data_man_ptr_or_null: fn() -> usize,

    /// True when a picked / loose `save_file` source (not the default user save) is driving
    /// this boot. Owner: `experiments::save_redirect`.
    pub direct_save_file_source_active: fn() -> bool,

    /// The staged native save the game's own reads resolve to, never the user's read-only
    /// source file. Owner: `experiments::save_redirect`.
    pub active_save_file_for_system_quit: fn() -> Option<PathBuf>,

    /// The slot a direct-file source will load. Owner:
    /// `experiments::continue_load::slot_resolution`, which folds the missing-save picker's
    /// pick, the trigger file's `slot=N` and the DLL config together.
    pub native_fullread_slot: fn() -> i32,

    /// Refill the per-slot name / stats / saved-map / place-name caches from a container's
    /// bytes. Owner: the ProfileSelect stats-text surface (`title_resources_stats_text.rs`) --
    /// the rows read those caches, not the records, so a rebuilt summary with stale caches
    /// shows the picked character's level under the previous save's name. Returns the number
    /// of slots decoded.
    pub load_profile_slot_caches_from_bytes: fn(&[u8], &str) -> usize,
}

fn default_log(_args: std::fmt::Arguments<'_>) {}
fn default_null_ptr() -> usize {
    0
}
fn default_gate_off() -> bool {
    false
}
fn default_no_pathbuf() -> Option<PathBuf> {
    None
}
fn default_slot_zero() -> i32 {
    0
}
fn default_no_caches(_bytes: &[u8], _source: &str) -> usize {
    0
}

impl ProfileSummaryHost {
    /// Neutral defaults: no-op logging, no `GameDataMan`, no direct save source, no staged
    /// path, and no cache reload. An un-hosted crate therefore reads no summary and rebuilds
    /// nothing -- every entry point below fails closed rather than writing records into a
    /// pointer it guessed.
    pub const fn defaults() -> Self {
        Self {
            append_autoload_debug: default_log,
            game_data_man_ptr_or_null: default_null_ptr,
            direct_save_file_source_active: default_gate_off,
            active_save_file_for_system_quit: default_no_pathbuf,
            native_fullread_slot: default_slot_zero,
            load_profile_slot_caches_from_bytes: default_no_caches,
        }
    }
}

impl Default for ProfileSummaryHost {
    fn default() -> Self {
        Self::defaults()
    }
}

static DEFAULT_HOST: ProfileSummaryHost = ProfileSummaryHost::defaults();
static HOST: OnceLock<ProfileSummaryHost> = OnceLock::new();

/// Install the host seam once, at DLL attach, before any hook install or task spawn can run
/// moved code. Returns false (and changes nothing) if a host was already installed.
pub fn install_host(host: ProfileSummaryHost) -> bool {
    HOST.set(host).is_ok()
}

fn host() -> &'static ProfileSummaryHost {
    HOST.get().unwrap_or(&DEFAULT_HOST)
}

// --- crate-internal wrappers bearing the exact original product names -----------------

#[allow(dead_code)]
pub(crate) fn append_autoload_debug(args: std::fmt::Arguments<'_>) {
    (host().append_autoload_debug)(args)
}
#[allow(dead_code)]
pub(crate) fn game_data_man_ptr_or_null() -> usize {
    (host().game_data_man_ptr_or_null)()
}
#[allow(dead_code)]
pub(crate) fn direct_save_file_source_active() -> bool {
    (host().direct_save_file_source_active)()
}
#[allow(dead_code)]
pub(crate) fn active_save_file_for_system_quit() -> Option<PathBuf> {
    (host().active_save_file_for_system_quit)()
}
#[allow(dead_code)]
pub(crate) fn native_fullread_slot() -> i32 {
    (host().native_fullread_slot)()
}
#[allow(dead_code)]
pub(crate) fn load_profile_slot_caches_from_bytes(bytes: &[u8], source: &str) -> usize {
    (host().load_profile_slot_caches_from_bytes)(bytes, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defaults must fail closed. A crate with no host installed reads no `GameDataMan`,
    /// claims no direct save source and offers no staged path -- so nothing downstream can be
    /// tricked into writing records into address 0 or into rebuilding from a guessed file.
    #[test]
    fn defaults_read_nothing_and_claim_nothing() {
        let defaults = ProfileSummaryHost::defaults();
        assert_eq!((defaults.game_data_man_ptr_or_null)(), 0);
        assert!(!(defaults.direct_save_file_source_active)());
        assert_eq!((defaults.active_save_file_for_system_quit)(), None);
        assert_eq!(
            (defaults.load_profile_slot_caches_from_bytes)(&[], "test"),
            0
        );
    }
}
