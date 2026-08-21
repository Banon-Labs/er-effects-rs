//! The dependency-injection seam between this feature crate and its host DLL.
//!
//! The moved pipeline used to read product state (gates, slot resolution, logging, the
//! shared boot-view rasterizer) directly out of the er-effects-rs flat namespace. Those
//! reads now go through function pointers installed once at DLL attach via
//! [`install_host`] (the same pattern as `er_d3d12_compositor::set_frame_provider`).
//! Crate-internal wrapper fns keep the EXACT original names/signatures, so the moved code
//! compiles unchanged. Until a host installs, every seam answers a neutral default
//! (logging is a no-op, all gates are off, slot lookups report "nothing loaded"), so the
//! crate is inert rather than wrong.
// The `pub(crate)` seam wrappers at the bottom of this file exist for the feature modules,
// every one of which is `#[cfg(windows)]`. On a host build those modules are compiled out, so
// the wrappers are unused BY CONSTRUCTION rather than by neglect -- deleting them would delete
// the seam. This is scoped to `not(windows)` deliberately: the shipping target
// (x86_64-pc-windows-msvc) keeps full dead-code enforcement over this file.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::layout::STATS_ATTR_COUNT;

/// The rendered boot/loading frame: CPU RGBA plus where to place it on a `bw`x`bh`
/// backbuffer. (Moved from er-effects-rs `experiments/gpu_readback/boot_progress.rs`;
/// the product's shared boot-view rasterizer still constructs it, through the shim.)
pub struct BootViewFrame {
    pub rgba: Vec<u8>,
    pub w: usize,
    pub h: usize,
    pub dx: usize,
    pub dy: usize,
}

/// Product callbacks the portrait/stats pipeline reads through the seam. Every field has
/// a neutral default (see [`PortraitHost::defaults`]); hosts overwrite the ones they own.
#[derive(Clone, Copy)]
pub struct PortraitHost {
    /// Structured debug logging sink (the product's `append_autoload_debug`).
    pub append_autoload_debug: fn(std::fmt::Arguments<'_>),
    /// Telemetry note for a fresh loading-screen portrait capture (w, h, pixels).
    pub note_ls_portrait_capture: fn(u32, u32, &[u8]) -> bool,
    /// Directory of the game executable (log/artifact root); `None` when unresolvable.
    pub game_directory_path: fn() -> Option<PathBuf>,
    /// Product gate: composite the captured portrait onto the loading screen.
    pub portrait_overlay_enabled: fn() -> bool,
    /// Product gate: render-thread offscreen drive (the keepalive keystone).
    pub portrait_render_drive_enabled: fn() -> bool,
    /// Product gate: read back REAL pixels from the profile RT.
    pub portrait_real_pixels_enabled: fn() -> bool,
    /// Product gate: the self-driving System->Quit repro harness owns the run.
    pub system_quit_repro_enabled: fn() -> bool,
    /// True when RenderDoc is hooking D3D12 (render-thread hooks stand down).
    pub renderdoc_active: fn() -> bool,
    /// The loaded character's save slot (collapsed form; 0 when unknown).
    pub portrait_loaded_slot: fn() -> i32,
    /// The loaded slot ONLY when a real source names it; `None` otherwise.
    pub portrait_loaded_slot_confirmed: fn() -> Option<i32>,
    /// The slot the loading-screen pipeline should TARGET (selection-aware).
    pub portrait_target_slot: fn() -> i32,
    /// Torn-readback score (0..255) over the masked head region of an RGBA frame.
    pub portrait_tear_score: fn(&[u8], usize, usize) -> usize,
    /// Count of profile-table renderers currently holding a live character model.
    pub count_live_profile_models: unsafe fn(usize) -> usize,
    /// `CSNowLoadingHelperImp::load_done` latch (load-COMPLETE, not "screen visible").
    pub now_loading_active: unsafe fn(usize) -> bool,
    /// True while the portrait pipeline must idle (active gameplay, menus own portraits).
    pub portrait_pipeline_idle_in_gameplay: unsafe fn(usize) -> bool,
    /// True while the DLL-drawn startup save picker owns the screen.
    pub save_picker_overlay_active: fn() -> bool,
    /// Milliseconds since the boot-view epoch (monotonic timestamp source).
    pub boot_view_epoch_ms: fn() -> u64,
    /// Highest-level real save slot, or -1 (`OWN_STEPPER_SLOT_NONE`) when none.
    pub best_active_slot: unsafe fn() -> i32,
    /// Populate the per-slot stats cache from the on-disk `.sl2` (once per session).
    pub ensure_profile_slot_stats_cached: unsafe fn(usize) -> bool,
    /// The eight attributes of the character in save `slot`, or `None`.
    pub profile_slot_attributes: fn(i32) -> Option<[i32; STATS_ATTR_COUNT]>,
    /// The STORED effective max vitals `[hp, fp, stamina]` of the character in save
    /// `slot` (the save's `MaxHealth`/`MaxFP`/`MaxSP` == runtime `current_max_*`),
    /// or `None` when the slot is empty/unreadable. Same `.sl2` cache as the
    /// attributes; values are read, never derived (bd er-effects-rs-qic7).
    pub profile_slot_vitals: fn(i32) -> Option<[u32; 3]>,
    /// The highest weapon upgrade level (`matchmakingWeaponLevel`) of the character in
    /// save `slot`, or `None` when the slot is empty/unreadable or the byte is
    /// implausible. Same `.sl2` cache as the attributes and vitals. `Some(0)` is a real
    /// answer (nothing upgraded) and is distinct from `None` ("we do not know").
    pub profile_slot_weapon_level: fn(i32) -> Option<u8>,
    /// The `CS::GameDataMan` singleton pointer, or 0.
    pub game_data_man_ptr_or_null: fn() -> usize,
    /// The shared boot/loading-screen rasterizer (bar + picker + portrait + stats in the
    /// product; a portrait+stats-only provider in the standalone DLL host).
    pub boot_view_render_frame: fn(usize, usize) -> BootViewFrame,
}

fn default_log(_args: std::fmt::Arguments<'_>) {}
fn default_note_ls_portrait_capture(_w: u32, _h: u32, _px: &[u8]) -> bool {
    false
}
fn default_game_directory_path() -> Option<PathBuf> {
    None
}
fn default_gate_off() -> bool {
    false
}
fn default_slot_zero() -> i32 {
    0
}
fn default_slot_none() -> Option<i32> {
    None
}
fn default_tear_score(_cpx: &[u8], _w: usize, _h: usize) -> usize {
    0
}
unsafe fn default_count_live_profile_models(_base: usize) -> usize {
    0
}
unsafe fn default_now_loading_active(_base: usize) -> bool {
    false
}
unsafe fn default_pipeline_idle(_base: usize) -> bool {
    false
}
/// Mirrors the product implementation: ms since the first call in this process.
fn default_boot_view_epoch_ms() -> u64 {
    static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
    let epoch = *EPOCH.get_or_init(std::time::Instant::now);
    epoch.elapsed().as_millis().min(u64::MAX as u128) as u64
}
/// Product `OWN_STEPPER_SLOT_NONE` (= `!0` = -1): no real slot.
unsafe fn default_best_active_slot() -> i32 {
    -1
}
unsafe fn default_ensure_profile_slot_stats_cached(_base: usize) -> bool {
    false
}
fn default_profile_slot_attributes(_slot: i32) -> Option<[i32; STATS_ATTR_COUNT]> {
    None
}
fn default_profile_slot_vitals(_slot: i32) -> Option<[u32; 3]> {
    None
}
fn default_profile_slot_weapon_level(_slot: i32) -> Option<u8> {
    None
}
fn default_game_data_man_ptr_or_null() -> usize {
    0
}
fn default_boot_view_render_frame(_bw: usize, _bh: usize) -> BootViewFrame {
    BootViewFrame {
        rgba: Vec::new(),
        w: 0,
        h: 0,
        dx: 0,
        dy: 0,
    }
}

impl PortraitHost {
    /// Neutral defaults: no-op logging, all gates off, no slots/stats/frames.
    pub const fn defaults() -> Self {
        Self {
            append_autoload_debug: default_log,
            note_ls_portrait_capture: default_note_ls_portrait_capture,
            game_directory_path: default_game_directory_path,
            portrait_overlay_enabled: default_gate_off,
            portrait_render_drive_enabled: default_gate_off,
            portrait_real_pixels_enabled: default_gate_off,
            system_quit_repro_enabled: default_gate_off,
            renderdoc_active: default_gate_off,
            portrait_loaded_slot: default_slot_zero,
            portrait_loaded_slot_confirmed: default_slot_none,
            portrait_target_slot: default_slot_zero,
            portrait_tear_score: default_tear_score,
            count_live_profile_models: default_count_live_profile_models,
            now_loading_active: default_now_loading_active,
            portrait_pipeline_idle_in_gameplay: default_pipeline_idle,
            save_picker_overlay_active: default_gate_off,
            boot_view_epoch_ms: default_boot_view_epoch_ms,
            best_active_slot: default_best_active_slot,
            ensure_profile_slot_stats_cached: default_ensure_profile_slot_stats_cached,
            profile_slot_attributes: default_profile_slot_attributes,
            profile_slot_vitals: default_profile_slot_vitals,
            profile_slot_weapon_level: default_profile_slot_weapon_level,
            game_data_man_ptr_or_null: default_game_data_man_ptr_or_null,
            boot_view_render_frame: default_boot_view_render_frame,
        }
    }
}

impl Default for PortraitHost {
    fn default() -> Self {
        Self::defaults()
    }
}

static DEFAULT_HOST: PortraitHost = PortraitHost::defaults();
static HOST: OnceLock<PortraitHost> = OnceLock::new();

/// Install the host seam ONCE, at DLL attach, BEFORE any hook install or task spawn can
/// run moved code. Returns false (and changes nothing) if a host was already installed.
pub fn install_host(host: PortraitHost) -> bool {
    HOST.set(host).is_ok()
}

fn host() -> &'static PortraitHost {
    HOST.get().unwrap_or(&DEFAULT_HOST)
}

// --- crate-internal wrappers bearing the EXACT original product names/signatures ------

pub(crate) fn append_autoload_debug(args: std::fmt::Arguments<'_>) {
    (host().append_autoload_debug)(args)
}
pub(crate) fn note_ls_portrait_capture(w: u32, h: u32, px: &[u8]) -> bool {
    (host().note_ls_portrait_capture)(w, h, px)
}
pub(crate) fn game_directory_path() -> Option<PathBuf> {
    (host().game_directory_path)()
}
pub(crate) fn portrait_overlay_enabled() -> bool {
    (host().portrait_overlay_enabled)()
}
pub(crate) fn portrait_render_drive_enabled() -> bool {
    (host().portrait_render_drive_enabled)()
}
pub(crate) fn portrait_real_pixels_enabled() -> bool {
    (host().portrait_real_pixels_enabled)()
}
pub(crate) fn system_quit_repro_enabled() -> bool {
    (host().system_quit_repro_enabled)()
}
pub(crate) fn renderdoc_active() -> bool {
    (host().renderdoc_active)()
}
pub(crate) fn portrait_loaded_slot() -> i32 {
    (host().portrait_loaded_slot)()
}
pub(crate) fn portrait_loaded_slot_confirmed() -> Option<i32> {
    (host().portrait_loaded_slot_confirmed)()
}
pub(crate) fn portrait_target_slot() -> i32 {
    (host().portrait_target_slot)()
}
pub(crate) fn portrait_tear_score(cpx: &[u8], w: usize, h: usize) -> usize {
    (host().portrait_tear_score)(cpx, w, h)
}
pub(crate) unsafe fn count_live_profile_models(base: usize) -> usize {
    unsafe { (host().count_live_profile_models)(base) }
}
pub(crate) unsafe fn now_loading_active(base: usize) -> bool {
    unsafe { (host().now_loading_active)(base) }
}
pub(crate) unsafe fn portrait_pipeline_idle_in_gameplay(base: usize) -> bool {
    unsafe { (host().portrait_pipeline_idle_in_gameplay)(base) }
}
pub(crate) fn save_picker_overlay_active() -> bool {
    (host().save_picker_overlay_active)()
}
pub(crate) fn boot_view_epoch_ms() -> u64 {
    (host().boot_view_epoch_ms)()
}
pub(crate) unsafe fn best_active_slot() -> i32 {
    unsafe { (host().best_active_slot)() }
}
pub(crate) unsafe fn ensure_profile_slot_stats_cached(base: usize) -> bool {
    unsafe { (host().ensure_profile_slot_stats_cached)(base) }
}
pub(crate) fn profile_slot_attributes(slot: i32) -> Option<[i32; STATS_ATTR_COUNT]> {
    (host().profile_slot_attributes)(slot)
}
pub(crate) fn profile_slot_vitals(slot: i32) -> Option<[u32; 3]> {
    (host().profile_slot_vitals)(slot)
}
pub(crate) fn profile_slot_weapon_level(slot: i32) -> Option<u8> {
    (host().profile_slot_weapon_level)(slot)
}
pub(crate) fn game_data_man_ptr_or_null() -> usize {
    (host().game_data_man_ptr_or_null)()
}
pub(crate) fn boot_view_render_frame(bw: usize, bh: usize) -> BootViewFrame {
    (host().boot_view_render_frame)(bw, bh)
}
