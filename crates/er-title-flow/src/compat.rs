// Compat seam: everything the moved cluster used to reach through the root
// crate's globs (`crate::*`, `crashlog::*`, `ffi::*`, `hooks::*`,
// `telemetry::*`, `experiments::*`) resolves here instead:
//  * `host::*` -- product functions crossing the seam as installed fn pointers;
//  * `constants_moved::*` -- the constants/statics/plain-data types moved
//    verbatim out of the root crate (the root re-imports them from us);
//  * `er_telemetry::counters::*` -- the shared telemetry statics (the root's
//    re-export lines were never definitions, so they are imports here too);
//  * `er_game_base` -- fault-safe RAM readers + game base/rva resolution and
//    the cross-cutting singleton RVA table (er-loading-portrait precedent);
//  * `er_loading_portrait` -- the portrait crate's published layout facts and
//    capture entry point the title flow already consumed pre-split.
pub use crate::constants_moved::*;
pub use crate::host::*;
pub use er_game_base::mem::*;
pub use er_game_base::rva::{
    CS_MENU_MAN_GLOBAL_RVA, CS_MENU_MAN_MENU_DATA_OFFSET, GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET,
};
pub use er_loading_portrait::{
    PORTRAIT_HOLD_MAX_TICKS, PROFILE_RENDERER_REFRESH_RVA, SLOT_MANAGER_CONTAINER_OFFSET,
    TITLE_CUSTOM_COVER_PROFILE_OFFSCREEN_TEX_RESCAP_OFFSET,
    TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET,
    TITLE_CUSTOM_COVER_PROFILE_RENDERER_TABLE_RVA, TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
    TITLE_PROFILE_SLOT_COUNT, maybe_capture_portrait_gxtexture,
};
pub use er_telemetry::counters::*;

// The moved cluster addresses a few product fns by their old submodule paths
// (`crate::compat::gating::...` etc. after the Stage A path rewrite). These
// shim modules forward to the same host fn-pointer seam as the flat names.
pub mod gating {
    pub(crate) use crate::host::{
        outgoing_teardown_enabled, outgoing_teardown_suppresses_holds,
        switch_reload_ownload_disabled,
    };
}
pub mod own_load {
    pub(crate) use crate::host::{own_load_switch_reload_fire, reset_switch_reload_latches};
}
pub mod trace {
    pub(crate) use crate::host::{
        blockres_stalecap_fix_enabled, map_mount_guard_flip_tick, run_ebl_mount_census,
    };
}

// Raw USER32/KERNEL32 externs the moved code calls directly. Declarations copied
// from the root crate's `ffi.rs` (same names, same signatures, same link
// attribute); an extern declaration carries no logic, so the two crates simply
// bind the same OS imports (er-game-base `mem.rs` precedent).
#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    pub fn GetSystemMenu(hwnd: *mut std::ffi::c_void, brevert: i32) -> *mut std::ffi::c_void;
    pub fn DeleteMenu(hmenu: *mut std::ffi::c_void, uposition: u32, uflags: u32) -> i32;
    pub fn ShowWindow(hwnd: *mut std::ffi::c_void, ncmdshow: i32) -> i32;
    pub fn UpdateWindow(hwnd: *mut std::ffi::c_void) -> i32;
    /// Fault-tolerant read: returns FALSE on unmapped/freed memory instead of
    /// raising an access violation.
    pub fn ReadProcessMemory(
        process: isize,
        base: *const std::ffi::c_void,
        buffer: *mut std::ffi::c_void,
        size: usize,
        read: *mut usize,
    ) -> i32;
}
