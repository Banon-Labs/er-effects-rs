//! The dependency-injection seam between this feature crate and its host DLL.
//!
//! Same pattern as `er_loading_portrait::host`: function pointers installed once at DLL
//! attach, neutral defaults until then, crate-internal wrappers bearing the EXACT names
//! the moved code already calls.
//!
//! This seam is larger than `er-save-picker`'s because the quit menu genuinely shares
//! state with the rest of the product: the ProfileSummary save-swap ledger is read by the
//! loading-cover slot resolution, the portrait slot is read by the loading-screen
//! pipeline, and the save-suppression bypass belongs to `er-save-suppress`. Every field
//! below is one MEASURED cross-call with a consumer OUTSIDE the quit-menu feature -- a
//! cross-call whose only consumers are inside the feature is a MOVE, not a seam entry, per
//! the 2026-07-30 rule that no extracted crate reaches back into `er-effects-rs`.
//!
//! Fields land per slice: this scaffold carries the entries that cross with std/primitive
//! types. Entries needing a reshaped type (the save-swap ledger, the serialized-slot
//! reader) are specified in docs/plans/save-picker-crate-extraction.md and land with the
//! slice that moves their caller.

use std::path::PathBuf;
use std::sync::OnceLock;

/// Where a save-destination browser opens, and what loaded file it is saving beside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveDestOrigin {
    pub start_dir: PathBuf,
    pub loaded_file_name: String,
    pub loaded_path: PathBuf,
}

/// Host callbacks the quit menu reads through the seam. Every field has a neutral default
/// (see [`QuitMenuHost::defaults`]); hosts overwrite the ones they own.
#[derive(Clone, Copy)]
pub struct QuitMenuHost {
    // --- logging ----------------------------------------------------------------------
    /// Structured debug logging sink (the product's `append_autoload_debug`).
    pub append_autoload_debug: fn(std::fmt::Arguments<'_>),
    /// Crash-log sink for the paths that must survive a hard fault.
    pub append_crash_log: fn(std::fmt::Arguments<'_>),

    // --- save source / redirect (owner: experiments::save_redirect, stays in product) --
    /// `%APPDATA%\EldenRing\<steamid>` when it is readable; `None` otherwise.
    pub default_save_root: fn() -> Option<std::path::PathBuf>,
    /// True when the active runtime is Seamless Co-op, so browsers offer `.co2` too.
    pub save_picker_seamless_mode_after_settle: fn(&str) -> bool,
    /// The loaded save's full path, or a reason string naming why it is unresolvable.
    pub system_quit_env_save_path: fn() -> Result<String, &'static str>,
    /// The loaded save's directory, or a reason string.
    pub system_quit_env_save_dir: fn() -> Result<String, &'static str>,
    /// Rewrite a foreign container's embedded Steam ID to the active user's, in place.
    pub normalize_save_bytes_to_active_steam_id: fn(&mut [u8]) -> bool,

    // --- loading-cover / autoload state (owner: product, genuinely shared) -------------
    /// The live `CS::ProfileSummary` pointer, or 0.
    pub system_quit_profile_summary_ptr: unsafe fn() -> usize,
    /// The loaded character's save slot (collapsed form; 0 when unknown).
    pub portrait_loaded_slot: fn() -> i32,
    /// The loaded slot ONLY when a real source names it; `None` otherwise.
    pub portrait_loaded_slot_confirmed: fn() -> Option<i32>,
    /// The slot the loading-screen pipeline should TARGET (selection-aware).
    pub portrait_target_slot: fn() -> i32,
    /// Rebuild the profile render table for a loading screen, if the state calls for it.
    pub maybe_build_profile_table_for_loading: unsafe fn(usize) -> bool,
    /// Drive one profile-renderer tick for `slot`. PRODUCT-owned: the loading-screen
    /// profile-model render drive is called from the task registration and the title tick,
    /// so it stays behind even though the quit menu also calls it.
    pub force_profile_render_tick: unsafe fn(usize, i32),
    /// True while a native loading screen is up.
    pub native_loading_screen_active: unsafe fn(usize) -> bool,

    // --- input ownership (owner: experiments::input_block, stays in product) -----------
    /// The GAME's main top-level window (dialog owner, dim geometry source). 0 = none.
    pub game_main_window: fn() -> usize,
    /// Release the input block immediately, before an irreversible quit.
    pub release_input_block_now: fn(),

    // --- save suppression (owner: er-save-suppress, already its own crate) ------------
    /// Take the one-shot bypass token that makes the Save Game row the ONLY path whose
    /// save enqueue is really forwarded. False when no token was available.
    pub take_save_write_bypass: fn(&'static str) -> bool,

    // --- product gates ----------------------------------------------------------------
    /// True on a product autoload run (as opposed to a bare diagnostic load).
    pub product_autoload_enabled: fn() -> bool,
    /// True while a switch-driven reload owns the flow.
    pub switch_reload_active: fn() -> bool,
    /// True when the configured picker surface is the OS-native common-file dialog.
    /// The dim overlay is a quit-menu concern, but the surface decision is product-wide.
    pub os_native_picker_active: fn() -> bool,
    /// Redact a Windows-form path for debug logs.
    pub windows_path_for_log: fn(&str) -> String,

    // --- OS-dialog System>Quit picker entrypoints -------------------------------------
    /// Recover the owning System dialog from a row action object.
    pub system_dialog_from_action_obj: unsafe fn(usize) -> usize,
    /// Restore the live ProfileSummary after a non-staging picker return.
    pub system_quit_save_swap_restore_profile_summary: unsafe fn(&str),
    /// Arm the original loaded save's ProfileSummary before a source browse.
    pub system_quit_save_swap_arm_original: fn(&str) -> bool,
    /// Start directory for the source browse surface.
    pub save_picker_start_dir: fn() -> Option<PathBuf>,
    /// Ingest a picked foreign save through the root product pipeline.
    pub system_quit_ingest_picked_save: unsafe fn(&str) -> bool,
    /// Start directory and default filename for the destination browser.
    pub save_dest_start_dir: fn() -> Option<SaveDestOrigin>,
    /// Stage the chosen save destination target in the product save-flow state machine.
    pub save_dest_set_target: fn(PathBuf, &'static str),
    /// True when the overwrite confirm recipe is buildable in the live game state.
    pub save_flow_box_recipe_available: fn() -> bool,
    /// Clear the save-flow confirm box state before a direct destination commit.
    pub save_flow_box_clear: fn(),
}

fn default_log(_args: std::fmt::Arguments<'_>) {}
fn default_gate_off() -> bool {
    false
}
fn default_no_root() -> Option<std::path::PathBuf> {
    None
}
fn default_seamless(_reason: &str) -> bool {
    false
}
fn default_no_save_path() -> Result<String, &'static str> {
    Err("no host installed")
}
fn default_normalize(_bytes: &mut [u8]) -> bool {
    false
}
unsafe fn default_summary_ptr() -> usize {
    0
}
fn default_slot_zero() -> i32 {
    0
}
fn default_slot_none() -> Option<i32> {
    None
}
unsafe fn default_build_table(_base: usize) -> bool {
    false
}
unsafe fn default_render_tick(_base: usize, _slot: i32) {}
unsafe fn default_loading_active(_base: usize) -> bool {
    false
}
fn default_window_none() -> usize {
    0
}
fn default_release_input() {}
fn default_take_bypass(_reason: &'static str) -> bool {
    false
}
unsafe fn default_dialog_from_action(_action_obj: usize) -> usize {
    0
}
unsafe fn default_restore_profile_summary(_reason: &str) {}
fn default_arm_original(_save_path: &str) -> bool {
    false
}
fn default_no_pathbuf() -> Option<PathBuf> {
    None
}
unsafe fn default_ingest_save(_selected_path: &str) -> bool {
    false
}
fn default_no_save_dest_origin() -> Option<SaveDestOrigin> {
    None
}
fn default_set_target(_path: PathBuf, _source: &'static str) {}
fn default_clear_save_flow_box() {}
fn default_windows_path_for_log(path: &str) -> String {
    path.to_owned()
}

impl QuitMenuHost {
    /// Neutral defaults: no-op logging, no save source, no summary, no slots, no window,
    /// and NO save-write bypass -- an un-hosted crate must never be able to authorise a
    /// real save.
    pub const fn defaults() -> Self {
        Self {
            append_autoload_debug: default_log,
            append_crash_log: default_log,
            default_save_root: default_no_root,
            save_picker_seamless_mode_after_settle: default_seamless,
            system_quit_env_save_path: default_no_save_path,
            system_quit_env_save_dir: default_no_save_path,
            normalize_save_bytes_to_active_steam_id: default_normalize,
            system_quit_profile_summary_ptr: default_summary_ptr,
            portrait_loaded_slot: default_slot_zero,
            portrait_loaded_slot_confirmed: default_slot_none,
            portrait_target_slot: default_slot_zero,
            maybe_build_profile_table_for_loading: default_build_table,
            force_profile_render_tick: default_render_tick,
            native_loading_screen_active: default_loading_active,
            game_main_window: default_window_none,
            release_input_block_now: default_release_input,
            take_save_write_bypass: default_take_bypass,
            product_autoload_enabled: default_gate_off,
            switch_reload_active: default_gate_off,
            os_native_picker_active: default_gate_off,
            windows_path_for_log: default_windows_path_for_log,
            system_dialog_from_action_obj: default_dialog_from_action,
            system_quit_save_swap_restore_profile_summary: default_restore_profile_summary,
            system_quit_save_swap_arm_original: default_arm_original,
            save_picker_start_dir: default_no_pathbuf,
            system_quit_ingest_picked_save: default_ingest_save,
            save_dest_start_dir: default_no_save_dest_origin,
            save_dest_set_target: default_set_target,
            save_flow_box_recipe_available: default_gate_off,
            save_flow_box_clear: default_clear_save_flow_box,
        }
    }
}

impl Default for QuitMenuHost {
    fn default() -> Self {
        Self::defaults()
    }
}

static DEFAULT_HOST: QuitMenuHost = QuitMenuHost::defaults();
static HOST: OnceLock<QuitMenuHost> = OnceLock::new();

/// Install the host seam ONCE, at DLL attach, BEFORE any hook install or task spawn can
/// run moved code. Returns false (and changes nothing) if a host was already installed.
pub fn install_host(host: QuitMenuHost) -> bool {
    HOST.set(host).is_ok()
}

fn host() -> &'static QuitMenuHost {
    HOST.get().unwrap_or(&DEFAULT_HOST)
}

// --- crate-internal wrappers bearing the EXACT original product names -----------------

#[allow(dead_code)]
pub(crate) fn append_autoload_debug(args: std::fmt::Arguments<'_>) {
    (host().append_autoload_debug)(args)
}
#[allow(dead_code)]
pub(crate) fn append_crash_log(args: std::fmt::Arguments<'_>) {
    (host().append_crash_log)(args)
}
#[allow(dead_code)]
pub(crate) fn default_save_root() -> Option<std::path::PathBuf> {
    (host().default_save_root)()
}
#[allow(dead_code)]
pub(crate) fn save_picker_seamless_mode_after_settle(reason: &str) -> bool {
    (host().save_picker_seamless_mode_after_settle)(reason)
}
#[allow(dead_code)]
pub(crate) fn system_quit_env_save_path() -> Result<String, &'static str> {
    (host().system_quit_env_save_path)()
}
#[allow(dead_code)]
pub(crate) fn system_quit_env_save_dir() -> Result<String, &'static str> {
    (host().system_quit_env_save_dir)()
}
#[allow(dead_code)]
pub(crate) fn normalize_save_bytes_to_active_steam_id(bytes: &mut [u8]) -> bool {
    (host().normalize_save_bytes_to_active_steam_id)(bytes)
}
#[allow(dead_code)]
pub(crate) unsafe fn system_quit_profile_summary_ptr() -> usize {
    unsafe { (host().system_quit_profile_summary_ptr)() }
}
#[allow(dead_code)]
pub(crate) fn portrait_loaded_slot() -> i32 {
    (host().portrait_loaded_slot)()
}
#[allow(dead_code)]
pub(crate) fn portrait_loaded_slot_confirmed() -> Option<i32> {
    (host().portrait_loaded_slot_confirmed)()
}
#[allow(dead_code)]
pub(crate) fn portrait_target_slot() -> i32 {
    (host().portrait_target_slot)()
}
#[allow(dead_code)]
pub(crate) unsafe fn maybe_build_profile_table_for_loading(base: usize) -> bool {
    unsafe { (host().maybe_build_profile_table_for_loading)(base) }
}
#[allow(dead_code)]
pub(crate) unsafe fn force_profile_render_tick(base: usize, slot: i32) {
    unsafe { (host().force_profile_render_tick)(base, slot) }
}
#[allow(dead_code)]
pub(crate) unsafe fn native_loading_screen_active(base: usize) -> bool {
    unsafe { (host().native_loading_screen_active)(base) }
}
#[allow(dead_code)]
pub(crate) fn game_main_window() -> usize {
    (host().game_main_window)()
}
#[allow(dead_code)]
pub(crate) fn release_input_block_now() {
    (host().release_input_block_now)()
}
#[allow(dead_code)]
pub(crate) fn take_save_write_bypass(reason: &'static str) -> bool {
    (host().take_save_write_bypass)(reason)
}
#[allow(dead_code)]
pub(crate) fn product_autoload_enabled() -> bool {
    (host().product_autoload_enabled)()
}
#[allow(dead_code)]
pub(crate) fn switch_reload_active() -> bool {
    (host().switch_reload_active)()
}
#[allow(dead_code)]
pub(crate) fn os_native_picker_active() -> bool {
    (host().os_native_picker_active)()
}
#[allow(dead_code)]
pub(crate) fn system_quit_windows_path_for_log(path: &str) -> String {
    (host().windows_path_for_log)(path)
}
#[allow(dead_code)]
pub(crate) unsafe fn system_dialog_from_action_obj(action_obj: usize) -> usize {
    unsafe { (host().system_dialog_from_action_obj)(action_obj) }
}
#[allow(dead_code)]
pub(crate) unsafe fn system_quit_save_swap_restore_profile_summary(reason: &str) {
    unsafe { (host().system_quit_save_swap_restore_profile_summary)(reason) }
}
#[allow(dead_code)]
pub(crate) fn system_quit_save_swap_arm_original(save_path: &str) -> bool {
    (host().system_quit_save_swap_arm_original)(save_path)
}
#[allow(dead_code)]
pub(crate) fn save_picker_start_dir() -> Option<PathBuf> {
    (host().save_picker_start_dir)()
}
#[allow(dead_code)]
pub(crate) unsafe fn system_quit_ingest_picked_save(selected_path: &str) -> bool {
    unsafe { (host().system_quit_ingest_picked_save)(selected_path) }
}
#[allow(dead_code)]
pub(crate) fn save_dest_start_dir() -> Option<SaveDestOrigin> {
    (host().save_dest_start_dir)()
}
#[allow(dead_code)]
pub(crate) fn save_dest_set_target(path: PathBuf, source: &'static str) {
    (host().save_dest_set_target)(path, source)
}
#[allow(dead_code)]
pub(crate) fn save_flow_box_recipe_available() -> bool {
    (host().save_flow_box_recipe_available)()
}
#[allow(dead_code)]
pub(crate) fn save_flow_box_clear() {
    (host().save_flow_box_clear)()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unhosted_quit_menu_can_never_authorise_a_real_save() {
        // The single most dangerous default in this seam: with no host, the one-shot
        // save-write bypass must be refused, so a crate loaded without a product cannot
        // push a write past `er-save-suppress`.
        assert!(!take_save_write_bypass("test"));
    }

    #[test]
    fn an_unhosted_quit_menu_reports_no_save_source_rather_than_a_guess() {
        assert!(system_quit_env_save_path().is_err());
        assert!(system_quit_env_save_dir().is_err());
        assert!(default_save_root().is_none());
        assert_eq!(game_main_window(), 0);
    }
}
