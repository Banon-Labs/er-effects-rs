//! The dependency-injection seam between this feature crate and its host DLL.
//!
//! The moved title/autoload/switch cluster used to call product functions (gates, slot
//! resolution, logging, hook installers, the own-stepper drivers) directly out of the
//! er-effects-rs flat namespace. Those calls now go through function pointers installed
//! once at DLL attach via [`install_host`] (the er-loading-portrait `PortraitHost`
//! precedent). Crate-internal wrapper fns keep the EXACT original names/signatures, so
//! the moved code compiles unchanged. Until a host installs, every seam answers a
//! neutral default (logging is a no-op, all gates are off, lookups report "nothing"),
//! so the crate is inert rather than wrong.
//!
//! The two detour wrappers (`title_update_detour`, `pab_node_update_detour`) exist so
//! the moved hook-install code can take their addresses; the game only ever reaches
//! them through hooks created by `create_continue_trace_hook`, which is itself a seam
//! fn (default: no hook created), so the detour defaults are unreachable pre-install.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::AtomicUsize;

use er_hook::MhHook;

use crate::constants_moved::{MenuActionNode, ProductCoreAutoloadReady};

/// Product callbacks the title-flow cluster reads through the seam. Every field has a
/// neutral default (see [`TitleFlowHost::defaults`]); the host DLL overwrites all of
/// them at attach.
#[derive(Clone, Copy)]
pub struct TitleFlowHost {
    /// Structured debug logging sink (the product's `append_autoload_debug`).
    pub append_autoload_debug: fn(std::fmt::Arguments<'_>),
    /// Crash-log sink for product faults/investigation breadcrumbs.
    pub append_crash_log: fn(std::fmt::Arguments<'_>),
    /// Structured timeline event sink (the product's `timeline_event`).
    pub timeline_event: fn(&str, u64, std::fmt::Arguments<'_>),
    /// Directory of the game executable (log/artifact root); `None` when unresolvable.
    pub game_directory_path: fn() -> Option<PathBuf>,
    /// The `CS::GameDataMan` singleton pointer, or 0.
    pub game_data_man_ptr_or_null: fn() -> usize,
    /// The `CS::GameMan` singleton pointer, or 0.
    pub game_man_ptr_or_null: fn() -> usize,
    /// The runtime heap allocator pointer, or 0.
    pub runtime_heap_allocator_ptr_or_null: fn() -> usize,
    /// `InGameStep` request-code -> name mapping.
    pub ingamestep_request_code_name: fn(i32) -> &'static str,
    /// MoveMapStep child step index -> name mapping.
    pub movemapstep_step_name: fn(i32) -> &'static str,
    /// SimpleTitleStep state -> name mapping.
    pub title_step_state_name: fn(i32) -> &'static str,
    // --- product gates (env/runtime mode policy) -------------------------------------
    pub autoload_disabled: fn() -> bool,
    pub cleanup_title_dialog_after_world_enabled: fn() -> bool,
    pub experimental_direct_menu_load_enabled: fn() -> bool,
    pub fire_tfc_continue_enabled: fn() -> bool,
    pub force_profile_render_enabled: fn() -> bool,
    pub native_profile_capture_enabled: fn() -> bool,
    pub product_autoload_enabled: fn() -> bool,
    pub profile_select_load_flow_enabled: fn() -> bool,
    pub title_accept_byte_gate_enabled: fn() -> bool,
    pub title_proceed_gate_enabled: fn() -> bool,
    pub pab_advance_enabled: fn() -> bool,
    pub outgoing_teardown_enabled: fn() -> bool,
    pub outgoing_teardown_suppresses_holds: fn() -> bool,
    pub switch_reload_ownload_disabled: fn() -> bool,
    /// Title-anim speedup multiplier (1.0 = neutral).
    pub title_anim_speedup_factor: fn() -> f32,
    // --- save-source policy ----------------------------------------------------------
    pub direct_save_file_source_active: fn() -> bool,
    pub missing_save_selection_pending: fn() -> bool,
    pub save_override_telemetry_only: fn() -> bool,
    // --- hook/patch helpers ----------------------------------------------------------
    /// MinHook create+queue wrapper (the product's `create_continue_trace_hook`).
    pub create_continue_trace_hook:
        unsafe fn(&mut Vec<MhHook>, &str, u32, *mut c_void, &'static AtomicUsize),
    pub install_auto_accept_hook: fn(),
    // --- menu/dialog observation -----------------------------------------------------
    pub decode_thunk_hop: unsafe fn(usize) -> Option<usize>,
    pub scan_dialog_for_loadgame: unsafe fn(usize, usize) -> (Option<usize>, Option<usize>),
    pub resolve_menu_system_save_load: unsafe fn(usize) -> Option<usize>,
    // --- product continue/load drivers -----------------------------------------------
    pub fire_product_title_load_action: unsafe fn(MenuActionNode, usize, u64, i32),
    pub product_continue_action_ready:
        unsafe fn(&ProductCoreAutoloadReady, usize, usize, i32) -> bool,
    pub product_continue_autoload_tick:
        unsafe fn(usize, usize, usize, i32, u64, &ProductCoreAutoloadReady),
    pub native_fullread_tick: unsafe fn(usize, usize, u64),
    pub resolve_active_load_slot: unsafe fn(i32) -> i32,
    pub mark_tfc_forced_continue_handoff: fn(),
    pub own_stepper_enter_s2_phase: fn(usize),
    pub own_stepper_stage2: unsafe fn(usize, usize, usize, i32, u64, usize),
    pub own_load_switch_reload_fire: unsafe fn(usize, usize, usize, i32, u64) -> bool,
    pub reset_switch_reload_latches: fn(),
    // --- map-mount / blockres trace helpers ------------------------------------------
    pub blockres_stalecap_fix_enabled: fn() -> bool,
    pub map_mount_guard_flip_tick: fn(bool, i32, i64),
    pub run_ebl_mount_census: fn(&str),
    // --- loading-screen / profile-table state ----------------------------------------
    pub fake_loading_screen_visible: unsafe fn(usize) -> bool,
    pub now_loading_active: unsafe fn(usize) -> bool,
    pub force_profile_render_tick: unsafe fn(usize, i32),
    pub system_quit_save_swap_recommit_after_return_title_save: fn(),
    pub portrait_retarget_and_rearm_for_switch: unsafe fn(i32, &str),
    // --- detours whose ADDRESSES the moved hook installers take ----------------------
    pub title_update_detour: unsafe extern "system" fn(usize, f32, usize),
    pub pab_node_update_detour: unsafe extern "system" fn(usize, usize, usize, usize) -> usize,
}

fn default_log(_args: std::fmt::Arguments<'_>) {}
fn default_timeline_event(_name: &str, _frame: u64, _fields: std::fmt::Arguments<'_>) {}
fn default_game_directory_path() -> Option<PathBuf> {
    None
}
fn default_ptr_or_null() -> usize {
    0
}
fn default_name(_v: i32) -> &'static str {
    ""
}
fn default_gate_off() -> bool {
    false
}
fn default_factor_neutral() -> f32 {
    1.0
}
unsafe fn default_create_continue_trace_hook(
    _hooks: &mut Vec<MhHook>,
    _name: &str,
    _rva: u32,
    _hook_impl: *mut c_void,
    _original: &'static AtomicUsize,
) {
}
fn default_unit() {}
fn default_unit_str(_source: &str) {}
unsafe fn default_decode_thunk_hop(_addr: usize) -> Option<usize> {
    None
}
unsafe fn default_scan_dialog_for_loadgame(
    _owner: usize,
    _base: usize,
) -> (Option<usize>, Option<usize>) {
    (None, None)
}
unsafe fn default_resolve_menu_system_save_load(_base: usize) -> Option<usize> {
    None
}
unsafe fn default_fire_product_title_load_action(
    _action: MenuActionNode,
    _base: usize,
    _tick: u64,
    _slot: i32,
) {
}
unsafe fn default_product_continue_action_ready(
    _ready: &ProductCoreAutoloadReady,
    _base: usize,
    _gm: usize,
    _slot: i32,
) -> bool {
    false
}
unsafe fn default_product_continue_autoload_tick(
    _owner: usize,
    _base: usize,
    _gm: usize,
    _slot: i32,
    _tick: u64,
    _ready: &ProductCoreAutoloadReady,
) {
}
unsafe fn default_native_fullread_tick(_owner: usize, _base: usize, _n: u64) {}
unsafe fn default_own_load_switch_reload_fire(
    _base: usize,
    _gm: usize,
    _owner: usize,
    _picked: i32,
    _n: u64,
) -> bool {
    false
}
fn default_map_mount_guard_flip_tick(_in_world: bool, _mms_step: i32, _sf: i64) {}
unsafe fn default_resolve_active_load_slot(configured: i32) -> i32 {
    configured
}
fn default_own_stepper_enter_s2_phase(_phase: usize) {}
unsafe fn default_own_stepper_stage2(
    _owner: usize,
    _base: usize,
    _gm: usize,
    _want_slot: i32,
    _n: u64,
    _framectx: usize,
) {
}
unsafe fn default_base_bool(_base: usize) -> bool {
    false
}
unsafe fn default_force_profile_render_tick(_base: usize, _slot: i32) {}
unsafe fn default_portrait_retarget_and_rearm_for_switch(_selected_slot: i32, _source: &str) {}
unsafe extern "system" fn default_title_update_detour(_dialog: usize, _delta: f32, _input: usize) {}
unsafe extern "system" fn default_pab_node_update_detour(
    _step: usize,
    _rdx: usize,
    _r8: usize,
    _r9: usize,
) -> usize {
    0
}

impl TitleFlowHost {
    /// Neutral defaults: no-op logging, all gates off, no pointers/hooks/drives.
    pub const fn defaults() -> Self {
        Self {
            append_autoload_debug: default_log,
            append_crash_log: default_log,
            timeline_event: default_timeline_event,
            game_directory_path: default_game_directory_path,
            game_data_man_ptr_or_null: default_ptr_or_null,
            game_man_ptr_or_null: default_ptr_or_null,
            runtime_heap_allocator_ptr_or_null: default_ptr_or_null,
            ingamestep_request_code_name: default_name,
            movemapstep_step_name: default_name,
            title_step_state_name: default_name,
            autoload_disabled: default_gate_off,
            cleanup_title_dialog_after_world_enabled: default_gate_off,
            experimental_direct_menu_load_enabled: default_gate_off,
            fire_tfc_continue_enabled: default_gate_off,
            force_profile_render_enabled: default_gate_off,
            native_profile_capture_enabled: default_gate_off,
            product_autoload_enabled: default_gate_off,
            profile_select_load_flow_enabled: default_gate_off,
            title_accept_byte_gate_enabled: default_gate_off,
            title_proceed_gate_enabled: default_gate_off,
            pab_advance_enabled: default_gate_off,
            outgoing_teardown_enabled: default_gate_off,
            outgoing_teardown_suppresses_holds: default_gate_off,
            switch_reload_ownload_disabled: default_gate_off,
            title_anim_speedup_factor: default_factor_neutral,
            direct_save_file_source_active: default_gate_off,
            missing_save_selection_pending: default_gate_off,
            save_override_telemetry_only: default_gate_off,
            create_continue_trace_hook: default_create_continue_trace_hook,
            install_auto_accept_hook: default_unit,
            decode_thunk_hop: default_decode_thunk_hop,
            scan_dialog_for_loadgame: default_scan_dialog_for_loadgame,
            resolve_menu_system_save_load: default_resolve_menu_system_save_load,
            fire_product_title_load_action: default_fire_product_title_load_action,
            product_continue_action_ready: default_product_continue_action_ready,
            product_continue_autoload_tick: default_product_continue_autoload_tick,
            native_fullread_tick: default_native_fullread_tick,
            resolve_active_load_slot: default_resolve_active_load_slot,
            mark_tfc_forced_continue_handoff: default_unit,
            own_stepper_enter_s2_phase: default_own_stepper_enter_s2_phase,
            own_stepper_stage2: default_own_stepper_stage2,
            own_load_switch_reload_fire: default_own_load_switch_reload_fire,
            reset_switch_reload_latches: default_unit,
            blockres_stalecap_fix_enabled: default_gate_off,
            map_mount_guard_flip_tick: default_map_mount_guard_flip_tick,
            run_ebl_mount_census: default_unit_str,
            fake_loading_screen_visible: default_base_bool,
            now_loading_active: default_base_bool,
            force_profile_render_tick: default_force_profile_render_tick,
            system_quit_save_swap_recommit_after_return_title_save: default_unit,
            portrait_retarget_and_rearm_for_switch: default_portrait_retarget_and_rearm_for_switch,
            title_update_detour: default_title_update_detour,
            pab_node_update_detour: default_pab_node_update_detour,
        }
    }
}

impl Default for TitleFlowHost {
    fn default() -> Self {
        Self::defaults()
    }
}

static DEFAULT_HOST: TitleFlowHost = TitleFlowHost::defaults();
static HOST: OnceLock<TitleFlowHost> = OnceLock::new();

/// Install the host seam ONCE, at DLL attach, BEFORE any hook install or task spawn can
/// run moved code. Returns false (and changes nothing) if a host was already installed.
pub fn install_host(host: TitleFlowHost) -> bool {
    HOST.set(host).is_ok()
}

fn host() -> &'static TitleFlowHost {
    HOST.get().unwrap_or(&DEFAULT_HOST)
}

// --- crate-internal wrappers bearing the EXACT original product names/signatures ------

pub(crate) fn append_autoload_debug(args: std::fmt::Arguments<'_>) {
    (host().append_autoload_debug)(args)
}
pub(crate) fn append_crash_log(args: std::fmt::Arguments<'_>) {
    (host().append_crash_log)(args)
}
pub(crate) fn timeline_event(name: &str, frame: u64, fields: std::fmt::Arguments<'_>) {
    (host().timeline_event)(name, frame, fields)
}
pub(crate) fn game_directory_path() -> Option<PathBuf> {
    (host().game_directory_path)()
}
pub(crate) fn game_data_man_ptr_or_null() -> usize {
    (host().game_data_man_ptr_or_null)()
}
pub(crate) fn game_man_ptr_or_null() -> usize {
    (host().game_man_ptr_or_null)()
}
pub(crate) fn runtime_heap_allocator_ptr_or_null() -> usize {
    (host().runtime_heap_allocator_ptr_or_null)()
}
pub(crate) fn ingamestep_request_code_name(v: i32) -> &'static str {
    (host().ingamestep_request_code_name)(v)
}
pub(crate) fn movemapstep_step_name(idx: i32) -> &'static str {
    (host().movemapstep_step_name)(idx)
}
pub(crate) fn title_step_state_name(v: i32) -> &'static str {
    (host().title_step_state_name)(v)
}
pub(crate) fn autoload_disabled() -> bool {
    (host().autoload_disabled)()
}
pub(crate) fn cleanup_title_dialog_after_world_enabled() -> bool {
    (host().cleanup_title_dialog_after_world_enabled)()
}
pub(crate) fn experimental_direct_menu_load_enabled() -> bool {
    (host().experimental_direct_menu_load_enabled)()
}
pub(crate) fn fire_tfc_continue_enabled() -> bool {
    (host().fire_tfc_continue_enabled)()
}
pub(crate) fn force_profile_render_enabled() -> bool {
    (host().force_profile_render_enabled)()
}
pub(crate) fn native_profile_capture_enabled() -> bool {
    (host().native_profile_capture_enabled)()
}
pub(crate) fn product_autoload_enabled() -> bool {
    (host().product_autoload_enabled)()
}
pub(crate) fn profile_select_load_flow_enabled() -> bool {
    (host().profile_select_load_flow_enabled)()
}
pub(crate) fn title_accept_byte_gate_enabled() -> bool {
    (host().title_accept_byte_gate_enabled)()
}
pub(crate) fn title_proceed_gate_enabled() -> bool {
    (host().title_proceed_gate_enabled)()
}
pub(crate) fn pab_advance_enabled() -> bool {
    (host().pab_advance_enabled)()
}
pub(crate) fn outgoing_teardown_enabled() -> bool {
    (host().outgoing_teardown_enabled)()
}
pub(crate) fn outgoing_teardown_suppresses_holds() -> bool {
    (host().outgoing_teardown_suppresses_holds)()
}
pub(crate) fn switch_reload_ownload_disabled() -> bool {
    (host().switch_reload_ownload_disabled)()
}
pub(crate) unsafe fn own_load_switch_reload_fire(
    base: usize,
    gm: usize,
    owner: usize,
    picked: i32,
    n: u64,
) -> bool {
    unsafe { (host().own_load_switch_reload_fire)(base, gm, owner, picked, n) }
}
pub(crate) fn reset_switch_reload_latches() {
    (host().reset_switch_reload_latches)()
}
pub(crate) fn blockres_stalecap_fix_enabled() -> bool {
    (host().blockres_stalecap_fix_enabled)()
}
pub(crate) fn map_mount_guard_flip_tick(in_world: bool, mms_step: i32, sf: i64) {
    (host().map_mount_guard_flip_tick)(in_world, mms_step, sf)
}
pub(crate) fn run_ebl_mount_census(src: &str) {
    (host().run_ebl_mount_census)(src)
}
pub(crate) fn title_anim_speedup_factor() -> f32 {
    (host().title_anim_speedup_factor)()
}
pub(crate) fn direct_save_file_source_active() -> bool {
    (host().direct_save_file_source_active)()
}
pub(crate) fn missing_save_selection_pending() -> bool {
    (host().missing_save_selection_pending)()
}
pub(crate) fn save_override_telemetry_only() -> bool {
    (host().save_override_telemetry_only)()
}
pub(crate) unsafe fn create_continue_trace_hook(
    _hooks: &mut Vec<MhHook>,
    name: &str,
    rva: u32,
    hook_impl: *mut c_void,
    original: &'static AtomicUsize,
) {
    unsafe { (host().create_continue_trace_hook)(_hooks, name, rva, hook_impl, original) }
}
pub(crate) fn install_auto_accept_hook() {
    (host().install_auto_accept_hook)()
}
pub(crate) unsafe fn decode_thunk_hop(addr: usize) -> Option<usize> {
    unsafe { (host().decode_thunk_hop)(addr) }
}
pub(crate) unsafe fn scan_dialog_for_loadgame(
    owner: usize,
    base: usize,
) -> (Option<usize>, Option<usize>) {
    unsafe { (host().scan_dialog_for_loadgame)(owner, base) }
}
pub(crate) unsafe fn resolve_menu_system_save_load(base: usize) -> Option<usize> {
    unsafe { (host().resolve_menu_system_save_load)(base) }
}
pub(crate) unsafe fn fire_product_title_load_action(
    action: MenuActionNode,
    base: usize,
    tick: u64,
    slot: i32,
) {
    unsafe { (host().fire_product_title_load_action)(action, base, tick, slot) }
}
pub(crate) unsafe fn product_continue_action_ready(
    ready: &ProductCoreAutoloadReady,
    base: usize,
    gm: usize,
    slot: i32,
) -> bool {
    unsafe { (host().product_continue_action_ready)(ready, base, gm, slot) }
}
pub(crate) unsafe fn product_continue_autoload_tick(
    owner: usize,
    base: usize,
    gm: usize,
    slot: i32,
    tick: u64,
    ready: &ProductCoreAutoloadReady,
) {
    unsafe { (host().product_continue_autoload_tick)(owner, base, gm, slot, tick, ready) }
}
pub(crate) unsafe fn native_fullread_tick(owner: usize, base: usize, n: u64) {
    unsafe { (host().native_fullread_tick)(owner, base, n) }
}
pub(crate) unsafe fn resolve_active_load_slot(configured: i32) -> i32 {
    unsafe { (host().resolve_active_load_slot)(configured) }
}
pub(crate) fn mark_tfc_forced_continue_handoff() {
    (host().mark_tfc_forced_continue_handoff)()
}
pub(crate) fn own_stepper_enter_s2_phase(phase: usize) {
    (host().own_stepper_enter_s2_phase)(phase)
}
pub(crate) unsafe fn own_stepper_stage2(
    owner: usize,
    base: usize,
    gm: usize,
    want_slot: i32,
    n: u64,
    framectx: usize,
) {
    unsafe { (host().own_stepper_stage2)(owner, base, gm, want_slot, n, framectx) }
}
pub(crate) unsafe fn fake_loading_screen_visible(base: usize) -> bool {
    unsafe { (host().fake_loading_screen_visible)(base) }
}
pub(crate) unsafe fn now_loading_active(base: usize) -> bool {
    unsafe { (host().now_loading_active)(base) }
}
pub(crate) unsafe fn force_profile_render_tick(base: usize, _slot: i32) {
    unsafe { (host().force_profile_render_tick)(base, _slot) }
}
pub(crate) fn system_quit_save_swap_recommit_after_return_title_save() {
    (host().system_quit_save_swap_recommit_after_return_title_save)()
}
pub(crate) unsafe fn portrait_retarget_and_rearm_for_switch(selected_slot: i32, source: &str) {
    unsafe { (host().portrait_retarget_and_rearm_for_switch)(selected_slot, source) }
}
/// Address-taken detour shim: forwards to the product detour installed via the host.
pub(crate) unsafe extern "system" fn title_update_detour(dialog: usize, delta: f32, input: usize) {
    unsafe { (host().title_update_detour)(dialog, delta, input) }
}
/// Address-taken detour shim: forwards to the product detour installed via the host.
pub(crate) unsafe extern "system" fn pab_node_update_detour(
    step: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    unsafe { (host().pab_node_update_detour)(step, rdx, r8, r9) }
}
