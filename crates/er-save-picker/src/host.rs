//! The dependency-injection seam between this feature crate and its host DLL.
//!
//! Same pattern as `er_loading_portrait::host`: function pointers installed once at DLL
//! attach, neutral defaults until then (so the crate is inert rather than wrong), and
//! crate-internal wrappers bearing the EXACT names the moved code already calls.
//!
//! Every field below is one MEASURED outbound reference from the files this crate will
//! own into the rest of `er-effects-rs`. Nothing else leaves the crate: drawing goes
//! through `er-loading-bar`, save-container parsing through `er-save-loader`, counters
//! through `er-telemetry`.

// `PickerCover::owner_hwnd` is an HWND read in exactly one place: `os_dialog::os_dialog_owner`
// (via the `PickerCover::owner_hwnd` method path, which is why a grep for `owner_hwnd()` finds
// nothing). That reader is gated TWICE -- the module is `#[cfg(feature = "os-dialog")]`, and its
// body is windows-only -- so the field and its getter are genuinely unreachable in two builds:
// a host build, and `--no-default-features`, which `scripts/check-rust-build.sh` checks on the
// SHIPPING target precisely so a feature-off configuration cannot rot unnoticed.
//
// Both conditions are named here rather than just `not(windows)`, because the earlier gate said
// only the first and the no-default-features build stayed red on a crate everyone had measured
// clean with default features. A shipping build WITH `os-dialog` still carries the full deny.
#![cfg_attr(any(not(windows), not(feature = "os-dialog")), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::model::PickerStatusMessage;

/// An opaque screen cover held for the lifetime of one OS-dialog interaction.
///
/// The dim overlay belongs to product (B) and must NOT dim the boot missing-save dialog
/// (user decision 2026-07-30), so this crate never constructs one -- it only accepts a
/// caller-supplied guard and drops it when the dialog interaction ends. `er-quit-menu`
/// passes a factory that arms its dim; the boot flow passes none.
///
/// The guard drops BEFORE the dialog claim, and spans the WHOLE reopen loop rather than
/// each individual dialog, so an invalid pick does not flash the game back at full
/// brightness between two dialogs.
pub struct PickerCover {
    owner_hwnd: usize,
    _guard: Box<dyn std::any::Any>,
}

impl PickerCover {
    /// Wrap a host-owned cover guard and the HWND that should own the common dialog.
    /// `owner_hwnd == 0` means the dialog falls back to the game window owner.
    pub fn new(owner_hwnd: usize, guard: Box<dyn std::any::Any>) -> Self {
        Self {
            owner_hwnd,
            _guard: guard,
        }
    }

    pub(crate) fn owner_hwnd(&self) -> usize {
        self.owner_hwnd
    }
}

/// Builds a cover for the named surface (`"load"` / `"save-as"`), or `None` for no cover.
pub type PickerCoverFactory = fn(&str) -> Option<PickerCover>;

/// Result of committing a boot-picker file+character selection through the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MissingSaveSelectionOutcome {
    Completed,
    Rejected(PickerStatusMessage),
}

/// Host callbacks the picker reads through the seam. Every field has a neutral default
/// (see [`SavePickerHost::defaults`]); hosts overwrite the ones they own.
#[derive(Clone, Copy)]
pub struct SavePickerHost {
    /// Structured debug logging sink (the product's `append_autoload_debug`).
    pub append_autoload_debug: fn(std::fmt::Arguments<'_>),

    // --- boot missing-save flow -------------------------------------------------------
    /// True while the boot is held at the save-check because no save was resolved. This is
    /// the ONE latch that arms, gates and disarms the boot picker
    /// (`save_redirect::path_hooks::missing_save_selection_pending`).
    pub missing_save_selection_pending: fn() -> bool,
    /// Commit a picked container: validate it, activate the save redirect, install the
    /// redirect hooks and release the save-check hold. `Rejected` carries the visible reason and
    /// keeps the picker up (`save_redirect::path_hooks::complete_missing_save_selection_from_picker`).
    pub complete_missing_save_selection_from_picker: fn(&Path) -> MissingSaveSelectionOutcome,
    /// True when the active runtime is Seamless Co-op, so the browser offers `.co2` as
    /// well as `.sl2` (`save_redirect::path_hooks::save_picker_seamless_mode_after_settle`).
    pub save_picker_seamless_mode_after_settle: fn(&str) -> bool,
    /// Where the boot browser opens: the remembered directory when it is still valid, else
    /// the default save root, else the Wine system drive. Windows-form path.
    pub picker_start_dir: fn() -> PathBuf,
    /// Persist a validated pick's directory as the remembered start directory, when the
    /// user's config allows it (`config::autoupdate_preferred_picker_dir_enabled` +
    /// `config::remember_preferred_save_picker_dir`).
    pub remember_picker_dir: fn(&Path),

    // --- OS common-file-dialog mechanism ----------------------------------------------
    /// The GAME's main top-level window (`experiments::input_block::game_main_window`).
    /// 0 = none.
    ///
    /// This is the dialog's FALLBACK `hwndOwner`, not its usual one. Since 2026-07-31 a
    /// System>Quit open owns the dialog to the DIM COVER instead, because a window is always
    /// above the window that owns it and that is the only way to make "the picker is in front
    /// of the blur" structural rather than a race with comdlg32's window creation. The cover
    /// is in turn an owned popup of THIS window, so the chain is game < cover < dialog. The
    /// game window is still the owner wherever no cover is up -- the missing-save boot arm,
    /// which raises none, and an arm whose cover did not come up in time.
    pub game_main_window: fn() -> usize,
    /// H4 gate: refuse to open a dialog while the core `CreateFileW` detour is still
    /// settling, because installing a MinHook suspends every thread and a thread parked in
    /// comdlg32 holding a shell/heap critical section is the one deadlock candidate
    /// (`experiments::save_file_core_hooks_live`).
    pub save_file_core_hooks_live: fn() -> bool,
    /// Redact a Windows-form path for the debug log
    /// (`system_quit_windows_path_for_log`).
    pub windows_path_for_log: fn(&str) -> String,
    /// True while a destination-commit window is armed. Log-only context on the dialog
    /// OPENED line (`save_dest_commit_window_armed`).
    pub save_dest_commit_window_armed: fn() -> bool,
}

fn default_log(_args: std::fmt::Arguments<'_>) {}
fn default_gate_off() -> bool {
    false
}
fn default_complete(_path: &Path) -> MissingSaveSelectionOutcome {
    MissingSaveSelectionOutcome::Rejected(PickerStatusMessage::new(
        "SAVE NOT AVAILABLE",
        "No host accepted this save selection.",
    ))
}
fn default_seamless(_reason: &str) -> bool {
    false
}
/// No host: browse nothing rather than guessing at a drive root.
fn default_picker_start_dir() -> PathBuf {
    PathBuf::new()
}
fn default_remember_picker_dir(_dir: &Path) {}
fn default_game_main_window() -> usize {
    0
}
fn default_windows_path_for_log(path: &str) -> String {
    path.to_owned()
}

impl SavePickerHost {
    /// Neutral defaults: no-op logging, never pending, every commit refused, vanilla
    /// flavor, no start directory, no owner window, and the H4 gate CLOSED (so an
    /// un-hosted crate never opens a modal dialog over a game it knows nothing about).
    pub const fn defaults() -> Self {
        Self {
            append_autoload_debug: default_log,
            missing_save_selection_pending: default_gate_off,
            complete_missing_save_selection_from_picker: default_complete,
            save_picker_seamless_mode_after_settle: default_seamless,
            picker_start_dir: default_picker_start_dir,
            remember_picker_dir: default_remember_picker_dir,
            game_main_window: default_game_main_window,
            save_file_core_hooks_live: default_gate_off,
            windows_path_for_log: default_windows_path_for_log,
            save_dest_commit_window_armed: default_gate_off,
        }
    }
}

impl Default for SavePickerHost {
    fn default() -> Self {
        Self::defaults()
    }
}

static DEFAULT_HOST: SavePickerHost = SavePickerHost::defaults();
static HOST: OnceLock<SavePickerHost> = OnceLock::new();

/// Install the host seam ONCE, at DLL attach, BEFORE any hook install or task spawn can
/// run moved code. Returns false (and changes nothing) if a host was already installed.
pub fn install_host(host: SavePickerHost) -> bool {
    HOST.set(host).is_ok()
}

fn host() -> &'static SavePickerHost {
    HOST.get().unwrap_or(&DEFAULT_HOST)
}

// --- crate-internal wrappers bearing the EXACT original product names -----------------

#[allow(dead_code)]
pub(crate) fn append_autoload_debug(args: std::fmt::Arguments<'_>) {
    (host().append_autoload_debug)(args)
}
#[allow(dead_code)]
pub(crate) fn missing_save_selection_pending() -> bool {
    (host().missing_save_selection_pending)()
}
#[allow(dead_code)]
pub(crate) fn complete_missing_save_selection_from_picker(
    path: &Path,
) -> MissingSaveSelectionOutcome {
    (host().complete_missing_save_selection_from_picker)(path)
}
#[allow(dead_code)]
pub(crate) fn save_picker_seamless_mode_after_settle(reason: &str) -> bool {
    (host().save_picker_seamless_mode_after_settle)(reason)
}
#[allow(dead_code)]
pub(crate) fn save_picker_title_start_dir() -> PathBuf {
    (host().picker_start_dir)()
}
#[allow(dead_code)]
pub(crate) fn remember_preferred_save_picker_dir(dir: &Path) {
    (host().remember_picker_dir)(dir)
}
#[allow(dead_code)]
pub(crate) fn game_main_window() -> usize {
    (host().game_main_window)()
}
#[allow(dead_code)]
pub(crate) fn save_file_core_hooks_live() -> bool {
    (host().save_file_core_hooks_live)()
}
#[allow(dead_code)]
pub(crate) fn system_quit_windows_path_for_log(path: &str) -> String {
    (host().windows_path_for_log)(path)
}
#[allow(dead_code)]
pub(crate) fn save_dest_commit_window_armed() -> bool {
    (host().save_dest_commit_window_armed)()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uninstalled_seam_refuses_rather_than_pretends() {
        // No host installed: the picker must report "nothing pending", refuse every commit
        // and keep the H4 dialog gate CLOSED, so a crate loaded without a host cannot
        // half-drive a save selection or throw a modal over an unknown window.
        assert!(!missing_save_selection_pending());
        assert!(matches!(
            complete_missing_save_selection_from_picker(Path::new("Z:\\nowhere\\ER0000.sl2")),
            MissingSaveSelectionOutcome::Rejected(_)
        ));
        assert!(!save_picker_seamless_mode_after_settle("test"));
        assert_eq!(save_picker_title_start_dir(), PathBuf::new());
        assert!(!save_file_core_hooks_live());
        assert_eq!(game_main_window(), 0);
    }

    #[test]
    fn a_path_with_no_redactor_is_logged_unchanged_rather_than_dropped() {
        assert_eq!(
            system_quit_windows_path_for_log("Z:\\saves\\ER0000.sl2"),
            "Z:\\saves\\ER0000.sl2"
        );
    }
}
