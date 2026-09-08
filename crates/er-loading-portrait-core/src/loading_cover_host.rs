//! The second dependency-injection seam between this crate and its host DLL, added by the
//! loading-cover extraction.
//!
//! [`super::host::PortraitHost`] carries the portrait/stats pipeline's product callbacks and is
//! installed from the root DLL's `DllMain`. The loading-cover modules moved here later need a
//! different and larger set of product functions (Win32 hook plumbing, crash-log call
//! attribution, product gates, save-source state), and the `PortraitHost` literal in
//! `lib_parts/dll_entry_parts/bootstrap.rs` is the spine several parallel extractions hang off:
//! adding fields to it would edit that file. So this seam is separate, and the root installs it
//! from `experiments/startup_hooks/loading_cover/mod.rs`'s `ensure_loading_cover_host()`, which
//! every facade entry point into the moved code calls first. Installation is therefore
//! guaranteed-before-first-use without touching `DllMain`.
//!
//! Until a host installs, every seam answers a neutral default (logging is a no-op, gates are
//! off, lookups report "nothing"), so the crate is inert rather than wrong.
// The `pub(crate)` seam wrappers exist for the feature modules, every one of which is
// `#[cfg(windows)]`. On a host build those modules are compiled out, so the wrappers are unused by
// construction rather than by neglect. Scoped to `not(windows)` deliberately: the shipping target
// keeps full dead-code enforcement over this file.
#![cfg_attr(not(windows), allow(dead_code))]

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::AtomicUsize;

use er_hook::MhHook;
use windows::Win32::Foundation::HWND;

/// Product callbacks the moved loading-cover modules read through the seam. Every field has a
/// neutral default (see [`LoadingCoverHost::defaults`]); hosts overwrite the ones they own.
#[derive(Clone, Copy)]
pub struct LoadingCoverHost {
    /// RVA of the first return address inside the game image on the current call stack, or 0.
    /// The product's `crashlog::trace_first_game_caller_rva`.
    pub trace_first_game_caller_rva: fn() -> usize,
    /// `GetProcAddress` over an already-loaded module, by NUL-terminated ASCII name.
    pub resolve_module_proc: fn(&[u8], &[u8]) -> Result<*mut c_void, String>,
    /// The game's own top-level window, when one exists yet.
    pub game_main_window: fn() -> Option<HWND>,
    /// Queue one absolute-address MinHook detour and publish its trampoline into `original`.
    ///
    /// # Safety
    /// `target` and `hook_impl` must be valid, compatible code addresses; the caller owns
    /// MinHook initialisation and the eventual `MH_ApplyQueued`.
    pub create_absolute_hook:
        unsafe fn(&mut Vec<MhHook>, &str, *mut c_void, *mut c_void, &AtomicUsize),
    /// Append one `"name": value,` line to the telemetry JSON body being built.
    pub push_json_usize: fn(&mut String, &str, usize),
    /// Milliseconds since the DLL debug log's own epoch (its first line, near DLL_PROCESS_ATTACH).
    /// This is a different epoch from `boot_view_epoch_ms`; the clock map states the offset.
    pub process_log_elapsed_ms: fn() -> u128,
    /// Milliseconds since the boot-view epoch, but only if that clock has already been anchored --
    /// `None` rather than starting it, so a caller that merely wants to stamp an event cannot move
    /// the origin of the whole run's timeline.
    pub boot_view_epoch_ms_if_anchored: fn() -> Option<u64>,
    /// The game's own `CSFakeLoadingScreenImp` cover plate, read out of the live singleton.
    pub fake_loading_screen_visible: unsafe fn(usize) -> bool,
}

fn default_trace_first_game_caller_rva() -> usize {
    0
}
fn default_resolve_module_proc(_module: &[u8], _proc: &[u8]) -> Result<*mut c_void, String> {
    Err("no loading-cover host installed".to_owned())
}
fn default_game_main_window() -> Option<HWND> {
    None
}
/// # Safety
/// Trivially safe: it installs nothing and dereferences nothing.
unsafe fn default_create_absolute_hook(
    _hooks: &mut Vec<MhHook>,
    _name: &str,
    _target: *mut c_void,
    _hook_impl: *mut c_void,
    _original: &AtomicUsize,
) {
}
fn default_push_json_usize(_body: &mut String, _name: &str, _value: usize) {}
fn default_process_log_elapsed_ms() -> u128 {
    0
}
fn default_boot_view_epoch_ms_if_anchored() -> Option<u64> {
    None
}
/// # Safety
/// Trivially safe: it reads nothing.
unsafe fn default_fake_loading_screen_visible(_base: usize) -> bool {
    false
}

impl LoadingCoverHost {
    /// Neutral defaults: no call attribution, no symbol resolution, no window, no hooks.
    pub const fn defaults() -> Self {
        Self {
            trace_first_game_caller_rva: default_trace_first_game_caller_rva,
            resolve_module_proc: default_resolve_module_proc,
            game_main_window: default_game_main_window,
            create_absolute_hook: default_create_absolute_hook,
            push_json_usize: default_push_json_usize,
            process_log_elapsed_ms: default_process_log_elapsed_ms,
            boot_view_epoch_ms_if_anchored: default_boot_view_epoch_ms_if_anchored,
            fake_loading_screen_visible: default_fake_loading_screen_visible,
        }
    }
}

impl Default for LoadingCoverHost {
    fn default() -> Self {
        Self::defaults()
    }
}

static DEFAULT_HOST: LoadingCoverHost = LoadingCoverHost::defaults();
static HOST: OnceLock<LoadingCoverHost> = OnceLock::new();

/// Install the loading-cover seam. Idempotent: returns false (and changes nothing) when a host is
/// already installed, which is what makes the root's `ensure_loading_cover_host()` cheap to call
/// from every facade entry point.
pub fn install_loading_cover_host(host: LoadingCoverHost) -> bool {
    HOST.set(host).is_ok()
}

fn host() -> &'static LoadingCoverHost {
    HOST.get().unwrap_or(&DEFAULT_HOST)
}

// --- crate-internal wrappers bearing the exact original product names/signatures ------

pub(crate) fn trace_first_game_caller_rva() -> usize {
    (host().trace_first_game_caller_rva)()
}
pub(crate) fn safe_input_proc(module: &[u8], proc: &[u8]) -> Result<*mut c_void, String> {
    (host().resolve_module_proc)(module, proc)
}
pub(crate) fn own_window() -> Option<HWND> {
    (host().game_main_window)()
}
/// # Safety
/// Same contract as [`LoadingCoverHost::create_absolute_hook`].
pub(crate) unsafe fn create_absolute_hook(
    hooks: &mut Vec<MhHook>,
    name: &str,
    target: *mut c_void,
    hook_impl: *mut c_void,
    original: &AtomicUsize,
) {
    unsafe { (host().create_absolute_hook)(hooks, name, target, hook_impl, original) }
}
pub(crate) fn push_json_usize(body: &mut String, name: &str, value: usize) {
    (host().push_json_usize)(body, name, value)
}
pub(crate) fn process_log_elapsed_ms() -> u128 {
    (host().process_log_elapsed_ms)()
}
pub(crate) fn boot_view_epoch_ms_if_anchored() -> Option<u64> {
    (host().boot_view_epoch_ms_if_anchored)()
}
/// # Safety
/// Same contract as [`LoadingCoverHost::fake_loading_screen_visible`]: `base` must be the game
/// module base.
pub(crate) unsafe fn fake_loading_screen_visible(base: usize) -> bool {
    unsafe { (host().fake_loading_screen_visible)(base) }
}
