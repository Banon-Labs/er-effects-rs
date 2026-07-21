use std::{
    ffi::c_void,
    fmt::Write as _,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, Once, OnceLock,
    },
    time::{Duration, Instant},
};

use std::os::windows::ffi::OsStrExt as _;

use crate::input_blocker::{InputBlocker, InputFlags};
use crate::mh::{MH_ApplyQueued, MH_Initialize, MhHook, MH_STATUS};
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use windows::{
    core::{BOOL, PCSTR},
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM},
        System::{
            LibraryLoader::{GetModuleHandleA, GetProcAddress},
            Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION},
            SystemServices::DLL_PROCESS_ATTACH,
            Threading::GetCurrentProcessId,
        },
        UI::WindowsAndMessaging::{
            ClipCursor, EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
            WM_KEYDOWN, WM_KEYUP,
        },
    },
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, ffi::*, hooks::*, telemetry::*};

use super::*;

/// The `direct_menu_load`/product_core path is experimental and currently distinct from the
/// known-good zero-input gold-load smoke path (`save_requested` + native Continue/PAB gates). Keep it
/// fail-closed unless an operator deliberately asks for that experiment; stale `ER_EFFECTS_AUTOLOAD_*`
/// env or release examples must not silently flip product smoke into the broken menu-core path.
pub(crate) fn experimental_direct_menu_load_enabled() -> bool {
    false
}
pub(crate) fn product_autoload_enabled() -> bool {
    PRODUCT_AUTOLOAD_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
}
/// Portrait render window: hold the autoload's own load-commit at the open main menu until the loaded
/// character's profile portrait has rendered, so the now-loading screen can show it.
///
/// DISABLED (2026-06-29): a runtime probe proved this BREAKS the core char-load -- kicking the refresh
/// (0x9aa680) at menu-open + holding the commit crashed during world-load (access-violation, run
/// product-continue-direct-20260629-104328), and the refresh gate fails there anyway (req754 stayed 0:
/// the ProfileSummary slot entry is not loaded at the open main menu, only when navigating to the
/// Load-Game/ProfileSelect submenu). The proven now-loading injection mechanism is unaffected. Leaving
/// the (gated-off) implementation in place for the record; do not re-enable without a safe render path.
pub(crate) fn portrait_render_window_enabled() -> bool {
    false
}
/// DEFAULT-OFF gate for the ProfileSelect load flow. When false (the default) `product_core_autoload_tick`
/// takes the PROVEN native Continue char-load commit, byte-for-byte unchanged. When the human flips
/// `PROFILE_SELECT_LOAD_FLOW_ENABLED` on to probe-test, the menu branch instead fires the title menu's
/// Load-Game row to open a LIVE `ProfileLoadDialog` (the render context in which the profile renderer's
/// per-slot refresh gate -- `ProfileSummary->saveSlotsStates[slot]` -- is satisfied), HOLDS the load
/// commit until the loaded character's portrait has rendered + been captured (so the now-loading screen
/// can display it), then drives the same STAGE2 commit (load_activate -> selector ->
/// continue_confirm/SetState5). Compile-time `const` so the OFF path is dead-code-eliminated.
pub(crate) fn profile_select_load_flow_enabled() -> bool {
    PROFILE_SELECT_LOAD_FLOW_ENABLED
}
/// Diagnostic mode for native ProfileSelect/profile-renderer portrait capture. This mode must not
/// arm product title-cover/custom-cover mutations or default Continue autoload; it only permits the
/// zero-host-input native menu open plus passive/native Load-Game row firing used by the capture
/// harness.
pub(crate) fn native_profile_capture_enabled() -> bool {
    false
}
/// Force the live profile-portrait 3D model render at the title/menu phase (where the GxDrawContext is
/// valid). The recurring task runs `force_profile_render_tick` each menu-phase frame: it marks the target
/// slot used (`MarkProfileIndexAsUsed`) then calls the argless profile-render refresh to kick the async
/// model build, and read-only-captures the rendered CSGxTexture once the model latches. Menu-phase only --
/// it does NOT commit Continue, so there is no teardown/world-load crash path.
///
/// DE-GATED to DEFAULT-ON for real (non-telemetry) runs (user 2026-06-30 "just a feature without a gate";
/// mirrors the native_continue/pab/splash de-gating precedent
/// `user-pref-too-many-env-file-gates-default-on-product`): the loading-screen portrait is now product
/// behavior, so it builds the model on every real autoload run without a staged flag. Master off:
/// `autoload_disabled()`; telemetry-only/native-capture runs stay off; env/file remain force-on overrides.
/// True on native Windows (NOT Wine/Proton). Wine's `ntdll` exports `wine_get_version`; native Windows
/// never does. Cached. Used to disable the character-profile RENDER-DRIVE on native Windows, where
/// driving the game's own offscreen model render mid-load crashes the strict D3D12 driver (bd
/// er-effects-rs-n4x, 2026-07-15). vkd3d/Proton tolerates it, so the drive stays on there.
pub(crate) fn is_native_windows() -> bool {
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    static CACHED: AtomicUsize = AtomicUsize::new(0); // 0=unknown, 1=native, 2=wine
    match CACHED.load(Ordering::SeqCst) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    let is_wine = unsafe { GetModuleHandleW(windows::core::w!("ntdll.dll")) }
        .ok()
        .map(|h| unsafe { GetProcAddress(h, windows::core::s!("wine_get_version")) }.is_some())
        .unwrap_or(false);
    CACHED.store(if is_wine { 2 } else { 1 }, Ordering::SeqCst);
    !is_wine
}

/// True when the operator force-DISABLED the native-Windows profile render-drive (env
/// `ER_EFFECTS_ALLOW_NATIVE_PROFILE_DRIVE=0`). The drive is now DEFAULT-ON for native (see the gates below):
/// the isolated overlay owns its own D3D12 device and the 8-frame settle gate keeps the crash-prone
/// model-drive blocked, so the portrait pipeline is runtime-proven safe on native (2026-07-15, zero AVs
/// across 7 boots, animated head captured + displayed). This env is now only a diagnostic force-OFF escape.
// ENV-GATE RATIONALE: ER_EFFECTS_ALLOW_NATIVE_PROFILE_DRIVE=0 force-DISABLES the (now default-on) native
// profile render-drive; it is a diagnostic escape hatch only and never writes a save or perturbs the mount.
fn native_profile_drive_disabled() -> bool {
    false
}

// ENV-GATE RATIONALE: ER_EFFECTS_FORCE_PROFILE_RENDER=1 force-ENABLES the profile portrait render-drive
// even on telemetry-only/no-load save-override runs (where it is otherwise off); diagnostic force-ON
// override only. Does not write a save; simply keeps the portrait render pipeline active for the probe.
pub(crate) fn force_profile_render_enabled() -> bool {
    if autoload_disabled() || native_profile_capture_enabled() {
        return false;
    }
    if is_native_windows() && native_profile_drive_disabled() {
        return false;
    }
    !save_override_telemetry_only()
}
/// DEFAULT-OFF gate for the live-portrait D3D12 readback. When on, the moment
/// `maybe_capture_portrait_gxtexture` pins the rendered offscreen `CSGxTexture`
/// (`LOADING_BG_PORTRAIT_GX_KEPT`), the DLL reads back that render target into CPU RGBA8
/// (`readback_offscreen_rgba8`) and stores it in `LOADING_BG_PORTRAIT_RGBA`, so the now-loading forge
/// can build its TPF from the REAL rendered character head instead of the magenta/yellow checker
/// placeholder. It also drives the profile offscreen size-table patch (currently base 512x512 with
/// native x2 supersample, expected 1024x1024 RT), so the portrait renders at the configured source resolution.
///
/// DE-GATED to DEFAULT-ON for real (non-telemetry) runs (user 2026-06-30 "just a feature without a gate";
/// mirrors the de-gating precedent `user-pref-too-many-env-file-gates-default-on-product`). Master off:
/// `autoload_disabled()`; telemetry-only/native-capture runs stay off; env/file remain force-on overrides.
pub(crate) fn portrait_real_pixels_enabled() -> bool {
    if autoload_disabled() || native_profile_capture_enabled() {
        return false;
    }
    !save_override_telemetry_only()
}
/// DEFAULT-OFF gate for the RENDER-THREAD offscreen drive (the keepalive keystone). When on, the
/// Present hook (`present_hook`, render thread, every frame, fires during the loading screen) drives the
/// profile renderer's offscreen draw (`PROFILE_OFFSCREEN_DRIVE_RVA` -> reads g_GxDrawContext, submits to
/// the GX pool) for the spared/built slot-0 renderer, so the loaded character's 3D head is actually
/// RENDERED into the offscreen RT after the menu's own render driver dies post-Continue. Without this the
/// model builds but is never drawn -> the RT holds a placeholder checker (oracle_loading_bg_portrait_is_
/// checker=True). The game-task drive renders BLACK / crashes (wrong thread + frame phase); the render
/// thread inside the Present hook is the surviving point.
///
/// DE-GATED to DEFAULT-ON for real (non-telemetry) runs (user 2026-06-30 "just a feature without a gate";
/// mirrors the de-gating precedent `user-pref-too-many-env-file-gates-default-on-product`). The earlier
/// "risky/unproven" caveat is retired: runtime-proven safe across the 2026-06-30 smokes (145-168 per-frame
/// Present-hook composites, no crash). This also runs the per-frame depth-alpha-key + CPU-blend composite.
/// Master off: `autoload_disabled()`; telemetry-only/native-capture runs stay off; env/file remain
/// force-on overrides.
pub(crate) fn portrait_render_drive_enabled() -> bool {
    if autoload_disabled() || native_profile_capture_enabled() {
        return false;
    }
    if is_native_windows() && native_profile_drive_disabled() {
        return false;
    }
    !save_override_telemetry_only()
}
/// DEFAULT-OFF gate for the portrait LOOK-AT lever (head/eyes follow the mouse cursor). When on, the
/// per-tick `force_profile_render_tick` reaches the loaded character's Havok pose holder and rotates the
/// Head/Neck/Spine2 bone local quaternions toward the cursor (ER eyes are welded to the Head bone, so
/// the eyes track as the head turns). Also selects OVERLAY-ONLY display (the live present-overlay owns the
/// loading-screen surface; the native forge/re-forge is suppressed so there is only ONE head). Requires
/// `force_profile_render` (the render that builds the model + drives the pose).
///
/// DE-GATED to DEFAULT-ON for real (non-telemetry) runs (user 2026-06-30 "just a feature without a gate";
/// mirrors the de-gating precedent `user-pref-too-many-env-file-gates-default-on-product`). Runtime-proven
/// safe across the 2026-06-30 smokes. Master off: `autoload_disabled()`; telemetry-only/native-capture
/// runs stay off; env/file remain force-on overrides. (The zero-input test drivers -- lookat-selftest,
/// cursor-sweep, force-rebuild -- stay OFF by default; product look-at tracks the real cursor.)
pub(crate) fn portrait_lookat_enabled() -> bool {
    if autoload_disabled() || native_profile_capture_enabled() {
        return false;
    }
    !save_override_telemetry_only()
}
/// DEFAULT-OFF experiment: suppress the game's `CSFakeLoadingScreenImp` cover plate during map loads so the
/// world renders uncovered ("no loading screen -- watch it pop in"). While set, the game task clamps the
/// cover's `visible` byte to 0 each frame. This is ORTHOGONAL to the portrait overlay -- the overlay keeps
/// its own gates, so the two can be toggled independently. Fully reversible: unset the file/env and the
/// game draws its cover normally. Exploratory visual experiment, not a product feature -- if we keep it,
/// tie it to autoload state instead of a standalone gate. Env `ER_EFFECTS_DISABLE_LOADING_COVER=1` OR
/// GAME_DIR file `er-effects-disable-loading-cover.txt`.
pub(crate) fn disable_loading_cover_enabled() -> bool {
    false
}
/// DEFAULT-OFF: when set, `force_profile_render_tick` does the DESTRUCTIVE periodic rebuild -- every ~240
/// ticks it CLEARS each renderer's build latch (+0x754/+0x755) + resets the look-at slot cache, forcing a
/// FRESH async model build. That churn leaves the models in a not-live (rebuilding) state most of the time,
/// which makes the realtime look-at draw fail ~88% of frames -> flicker. So it is OFF by default: the model
/// builds ONCE (idempotent mark+refresh) and PERSISTS, so the pose-holder stays live every frame and the
/// portrait tracks the cursor smoothly. Flip this on briefly (then off) only to force a fresh rebuild that
/// re-captures the post-FaceData face. Mirrors `portrait_lookat_enabled` (env OR file).
pub(crate) fn portrait_force_rebuild_enabled() -> bool {
    false
}
/// DEFAULT-OFF self-validation: when set, the realtime draw task drives Head/Neck/Spine2 from a
/// DETERMINISTIC SINUSOID (frame-counter based) instead of GetCursorPos -- zero-input, reproducible, no
/// human mouse -- and reads back the portrait offscreen RT each sample to record nonblack% + hash-change%
/// as in-process telemetry semaphores (oracle_profile_lookat_rt_*). PASS = nonblack≈100% (no flicker) AND
/// changed≈100% under the sinusoid (the rendered head moves with the driven angle) AND render_drives≈frames
/// (per-frame redraw). This replaces the human-eyeball oracle. Mirrors `portrait_lookat_enabled`.
pub(crate) fn portrait_lookat_selftest_enabled() -> bool {
    false
}
/// DEFAULT-OFF cursor-tracking PROOF: when set, the realtime draw task deterministically self-drives the
/// OS cursor (`SetCursorPos`) through held left/center/right positions over the Elden Ring window, then
/// reads it back through the SAME `GetCursorPos` path the product uses and drives the head from that read
/// cursor (NO sinusoid shortcut). It dumps the live head at each held cursor position
/// (`portrait-capture-slot{200,201,202}.bin`), so the three distinct poses prove the head tracks the
/// ACTUAL cursor input -- zero foreign input (the DLL warps the cursor itself, at the exact stage the game
/// polls). Takes precedence over `selftest`. Mirrors `portrait_lookat_enabled` (env OR file).
pub(crate) fn portrait_cursor_sweep_enabled() -> bool {
    false
}
/// Kill-switch to skip installing the continue_trace hooks (bisecting a ~19s
/// title crash caused by our DLL). When set, the continue/load-flow hooks are
/// not installed even if autoload is configured.
/// Bisect kill-switch: when set, the recurring game task does nothing each
/// frame, so we can tell whether the per-frame task body or the DLL's mere
/// presence is what terminates the title ~19s in.
pub(crate) fn inert_mode() -> bool {
    false
}
/// Bisect kill-switch: the recurring task does lock + tick only, with no
/// filesystem I/O. Lets us tell whether the per-frame file I/O (telemetry write)
/// is what stalls the title vs. any per-frame work at all.
pub(crate) fn lite_mode() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_NO_CONTINUE_TRACE is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn continue_trace_disabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_TRACE_CONTINUE is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn trace_continue_enabled() -> bool {
    product_autoload_enabled()
}
/// DE-GATED (deprecate-env-marker-gate-allowlists-2026-07-19): the menu-task-update trace was a
/// diagnostic env/marker probe. Env/marker feature gates are forbidden, so this passive trace is
/// retired (permanently off); re-add via a non-env/marker diagnostic mechanism if needed.
pub(crate) fn trace_menu_task_update_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_AUTOLOAD_NATIVE_TITLE_JOB is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn native_title_job_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_AUTOLOAD_FORCE_PLAY_GAME is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn force_play_game_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_SELECTBOT_PROBE is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn selectbot_probe_enabled() -> bool {
    false
}
/// Operator gate for the zero-input global-accept-byte title-advance lever (option c). Default OFF.
// ENV-GATE RATIONALE: ER_EFFECTS_TITLE_ACCEPT_BYTE is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn title_accept_byte_gate_enabled() -> bool {
    false
}
/// Operator gate for lever-3 (narrow registrar advance): set the menu-transition singleton flag
/// 0x143d5dea8->+0=1 before the validated open-menu self-fire, replicating the native title
/// press-accept handler so the menu opens in place without the ToS over-trigger. Default OFF;
/// used together with own_stepper + self-fire.
pub(crate) fn title_registrar_advance_gate_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_TITLE_PROCEED_GATE is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn title_proceed_gate_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_INGAMESTEP_PUMP is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn ingamestep_pump_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_NATIVE_AUTOLOAD is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn native_autoload_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_OBSERVE is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn observe_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_OWN_STEPPER is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn own_stepper_enabled() -> bool {
    if missing_save_selection_pending() {
        return false;
    }
    product_autoload_enabled()
        || OWN_STEPPER_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
}
/// OBSERVE-ONLY NATIVE-LOAD gate (corrected-autoload-design-observe-not-force-native-load-2026).
/// OFF by default; enable via env `ER_EFFECTS_NATIVE_LOAD=1` OR a GAME_DIR file
/// `er-effects-native-load.txt`. Mirrors `own_stepper_enabled` (env OR file). When ON, the idx10
/// handler installs the patch (so OUR handler runs each frame) but does NOT force the title state
/// machine: it lets OWN_STEPPER_ORIG_IDX10 pass-through advance the native boot naturally (the user
/// drives past press-any-button + modals in this hybrid test), and ONCE the live TitleTopDialog
/// menu is rendered + settled, it fires the native Load-Game MenuMemberFuncJob node's run
/// 0x1409aaba0 exactly once -- testing whether that loads the real char in a NATURAL (non-forced)
/// menu. NO SetState(2/3), NO beginlogo-gate clear, NO registrar self-fire, NO direct_build /
/// cold_char_mount. De-risks design step 4.
pub(crate) fn native_load_enabled() -> bool {
    false
}
/// OBSERVE-ONLY NATIVE-CONTINUE gate (PATH B, autoload-path-B-drive-native-load-chosen-2026-06-22).
/// OFF by default; enable via env `ER_EFFECTS_NATIVE_CONTINUE=1` OR a GAME_DIR file
/// `er-effects-native-continue.txt`. Mirrors `native_load_enabled` (env OR file). When ON, the idx10
/// handler installs the patch (so OUR handler runs each frame) but does NOT force the title state
/// machine: it lets OWN_STEPPER_ORIG_IDX10 pass-through advance the native boot naturally (the user
/// drives past press-any-button + modals in this hybrid test, OR the own-stepper opens the menu),
/// and ONCE the live TitleTopDialog menu is rendered + settled, it fires the native CONTINUE
/// (load-most-recent) MenuMemberFuncJob node's run 0x1409aaba0 exactly once -- which drives the FULL
/// native load (parse + world-asset streaming + spawn). NO SetState(2/3), NO beginlogo-gate clear,
/// NO registrar self-fire, NO direct_build / cold_char_mount. Observe + one-shot fire only.
/// Single explicit OFF kill-switch for the always-on product autoload (most-recent native Continue
/// + the readiness press-any-button advance that gets us to the title menu). Autoload is the DEFAULT
/// DLL behavior (user directive 2026-06-24 "Autoload should always be the default dll behavior";
/// product contract `autoload-dll-product-requirements`: "always-on -- no opt-in gate; users install
/// the DLL knowingly and read docs"). Set `ER_EFFECTS_NO_AUTOLOAD=1` or drop
/// `er-effects-no-autoload.txt` next to eldenring.exe to suppress it (overlay-only use, or a session
/// that should not auto-Continue). Mirrors the splash-skip de-gating precedent
/// (`user-pref-too-many-env-file-gates-default-on-product-2026-06-23`).
pub(crate) fn autoload_disabled() -> bool {
    // DE-GATED (deprecate-env-marker-gate-allowlists-2026-07-19): the always-on product autoload no
    // longer honors an ER_EFFECTS_NO_AUTOLOAD env/marker opt-out -- env/marker feature gates are
    // forbidden. Autoload is unconditionally the default DLL behavior (product default was already on).
    // If a non-autoload/overlay-only mode is needed, drive it from me3 profile config, not an env/marker.
    false
}
/// PRODUCT DIRECTION (2026-07-04): the ProfileSelect / Load-Game menu shows a **stats panel** instead
/// of the character portrait in each 128x128 save-slot face box. When this is on (the product default)
/// the stats-panel pipeline runs: a neutral background texture is injected into each
/// `SYSTEX_Menu_ProfileNN` slot and each visible `menu_dummyprofileface_NN` bind is redirected to it, so
/// the box shows our background; the character's attributes are then drawn as native `MenuFont_01` text
/// (see bd `profile-select-stats-panel-goal-plan-2026-07-03`,
/// `profile-select-05010-layout-fonts-RE-2026-07-04`, `profileselect-native-settext-RE-2026-07-04`).
///
/// IMPORTANT -- this does NOT blank the character render. The portrait render pipeline
/// (`force_profile_render_enabled` etc.) ALSO produces the LOADING-SCREEN portrait of the loaded
/// character (via the offscreen readback -> now-loading forge, a DIFFERENT consumer than the
/// ProfileSelect box DISPLAY bind). Blanking the render to hide the ProfileSelect portraits also killed
/// the loading-screen portrait (user-reported 2026-07-04). Since the ProfileSelect boxes are hidden by
/// the DISPLAY-bind redirect regardless of whether the render ran, the render stays ON (the crash-free
/// one-slot render feeds the loading-screen portrait) and only the box display is redirected.
///
/// This is a PRODUCT-LEVEL lever tied to autoload state (not a per-feature knob): default-ON for any
/// real product autoload run, OFF for telemetry-only/observe and native-capture runs. A single DISABLE
/// override turns the stats panel off for A/B, mirroring `autoload_disabled()`'s `ER_EFFECTS_NO_AUTOLOAD`
/// shape: env `ER_EFFECTS_NO_STATS_PANEL=1` OR the GAME_DIR file `er-effects-no-stats-panel.txt`.
pub(crate) fn stats_panel_enabled() -> bool {
    if autoload_disabled() || native_profile_capture_enabled() || save_override_telemetry_only() {
        return false;
    }
    true
}
// ENV-GATE RATIONALE: ER_EFFECTS_NATIVE_CONTINUE is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn native_continue_enabled() -> bool {
    if autoload_disabled() || native_profile_capture_enabled() {
        return false;
    }
    !save_override_telemetry_only()
}
/// OBSERVE-ONLY NATIVE FULL-SAVE-READ gate (native-full-save-read-slot-resolve-chain-observe-recipe-2026).
/// OFF by default; enable via env `ER_EFFECTS_NATIVE_FULLREAD=1` OR a GAME_DIR file
/// `er-effects-native-fullread.txt`. Mirrors `native_load_enabled` (env OR file). When ON, the idx10
/// handler installs the patch (so OUR handler runs each frame) but does NOT force the title state
/// machine: it lets OWN_STEPPER_ORIG_IDX10 pass-through advance the native boot naturally (the user
/// drives past press-any-button + modals in this hybrid test), and ONCE the live TitleTopDialog menu
/// is rendered + settled, it runs the native full-save-read load chain directly at the live menu --
/// where the FD4 IO worker pool is LIVE so the submit drains (SUBMIT -> DRAIN_POLL -> DESER -> GUARD
/// -> CONFIRM). NO SetState forcing for boot, NO selector-step pump (probe-12 crash). The sole save
/// write (continue_confirm 0x140b0e180 -> SetState5) is HARD-gated behind the step-6 guard AND the
/// separate commit sub-gate `native_fullread_commit_enabled` (default = VERIFY-ONLY).
pub(crate) fn native_fullread_enabled() -> bool {
    false
}
/// COMMIT sub-gate for the native full-save-read chain (REQUIRED to actually fire continue_confirm
/// 0x140b0e180 -> SetState5, the SOLE save write). OFF by default; enable via env
/// `ER_EFFECTS_FULLREAD_COMMIT=1` OR a GAME_DIR file `er-effects-fullread-commit.txt`. Without it the
/// chain stops at the step-6 GUARD (deserialize + guard + log only): save-safe, NO continue_confirm,
/// NO SetState5. This lets a first test run VERIFY-ONLY (default) before any save write.
pub(crate) fn native_fullread_commit_enabled() -> bool {
    product_autoload_enabled()
}
/// OPT-IN post-world native TitleTopDialog cleanup. Static trace of 0x1409a8890 shows this is the
/// real dialog cleanup body: it clears active-screen renderers and releases dialog-owned resources.
/// It fires only after PlayerIns exists, so it cannot participate in save/load success.
pub(crate) fn cleanup_title_dialog_after_world_enabled() -> bool {
    product_autoload_enabled()
}
/// OPT-IN gate for the MenuWindow-latch diagnostic hook (SceneObjProxy ctor 0x14074a700).
/// OFF by default: a clean run installs NO MinHook / NO detour for this. Enable only when
/// the latch is needed, via env `ER_EFFECTS_MENU_WINDOW_LATCH=1` OR a GAME_DIR file
/// `er-effects-menu-window-latch.txt`. Mirrors `own_stepper_enabled` (env OR file).
/// Rationale: this hook was previously installed UNCONDITIONALLY at process-attach and was
/// NOT present in the prior working cold-mount run; gating it lets us isolate hook-induced
/// mount perturbation (see bd probe11 caveat).
pub(crate) fn menu_window_latch_enabled() -> bool {
    false
}
/// Explicit opt-in to let the injected in-world System -> Quit Game -> ProfileSelect route perform
/// the native slot-load activation. Default OFF because the prior live attempt crashed inside
/// CSGaitemImp::Deserialize at live/deobf 0x14067141a; default behavior logs the selected cursor and
/// suppresses the activation so profile-selection investigation stays save-safe.
pub(crate) fn system_quit_profile_load_activation_allowed() -> bool {
    // MUST stay false. INVERTED SENSE (bd arm-commit-gap-rootcause-my-flip-of-activation-allowed-inverted):
    // this is the opt-in to the NATIVE-FORWARD slot activation, which CRASHES in CSGaitemImp::Deserialize
    // (0x14067141a). When it is FALSE, system_quit_ownership_repro.rs's `if !activation_allowed` gate runs
    // the SAVE-SAFE DIRECT-ARM path (system_quit_arm_quickload_autoload -> advances the quickload phase ->
    // return-title + own_load reload). Flipping this to true (a prior regression) SKIPPED the direct-arm,
    // so load2 armed activate_count=1 but never committed (phase stayed IDLE, ProfileSelect pumped open,
    // fresh_deser=0). The direct-arm is NOT harness-gated -- it fires for any in-range ProfileSelect pick.
    false
}
/// OPT-IN gate for the c30-writer diagnostic hook (hot deserialize-internal 0x67bd70).
/// OFF by default: a clean run installs NO MinHook / NO detour for this. Enable only when
/// the diagnostic is needed, via env `ER_EFFECTS_C30_DIAG=1` OR a GAME_DIR file
/// `er-effects-c30-diag.txt`. Mirrors `own_stepper_enabled` (env OR file).
/// Rationale: a trampoline on the HOT 0x67bd70 deserialize path may itself perturb the
/// mount (b80 stuck / crash); gating it lets us run without it to isolate (bd probe11).
pub(crate) fn c30_writer_diag_enabled() -> bool {
    false
}
/// PASSIVE own-stepper: do NOT force the menu (no SetState(2)/self-fire) and do NOT block input.
/// The user navigates to Load Game once (the input that surfaces the input-gated d180); the
/// capture hooks grab d180; then STAGE 2 drives mount->confirm->load. This both PROVES the load
/// (correct + faster than manual slot-select) and lets the iterator log the menu-structure change
/// so the pump-switch can be replayed zero-input later. File: er-effects-passive.txt.
pub(crate) fn own_stepper_passive_enabled() -> bool {
    false
}
/// DETERMINISTIC MENU INPUT PROBE (er-effects-input-probe.txt / ER_EFFECTS_INPUT_PROBE). After the
/// menu opens, inject one Down tap then (after an observation window) one Confirm tap, at frames WE
/// choose -- so we know exactly the frame to break on. Decisive question: does the Load-Game leaf
/// d180 tick its leaf Update on HIGHLIGHT alone (Down, no Confirm yet), or only at Confirm? Targeted
/// input used purely as a MEASUREMENT oracle (NOT the zero-input deliverable).
pub(crate) fn input_probe_enabled() -> bool {
    false
}
/// SELF-DRIVEN GAMEPAD NAV INJECTION (er-effects-inject-nav.txt / ER_EFFECTS_INJECT_NAV). When on,
/// the input block stays engaged PAST menu-open (user input fully suppressed) and the XInput hook
/// fabricates a D-pad Down nav schedule at the gamepad poll source, cycling the title-menu cursor
/// so the input/focus-gated row populate fires and the row-push/csmenu-ctor hooks capture its
/// trigger -- uncontaminated by user input. Capture-only (Down nav, never Confirm).
pub(crate) fn inject_nav_enabled() -> bool {
    false
}

/// MOVEMENT-PROOF probe (`er-effects-prove-movement.txt`). When staged, authorizes the in-DLL
/// can-move probe to inject a forward stick in-world AND forces XInput slot 0 "connected" so the game
/// polls it (else, with no physical pad, the injected stick never lands). Proof-only / diagnostic.
pub(crate) fn prove_movement_enabled() -> bool {
    // DECOUPLED TOGGLE: the can-move probe (drive the player FORWARD >=60 frames + confirm
    // CAN_MOVE_CONFIRMED) is part of the load2 test-drive; enable it when the input-harness DLL is
    // present (presence-gated, not marker/env). Without it sq-repro's WAIT_WORLD never advances. bd
    // load2-testdrive-move60-then-menu-load-driver-degated-2026-07-19.
    harness_dll_present()
}

/// AUTONOMOUS-PROOF FOREGROUND (`er-effects-probe-foreground.txt`). Gameplay MOVEMENT input is only
/// processed while the ER window is FOCUSED (stay-active/DLUID+0x88d covers menus, not locomotion). For
/// an unattended proof (user away, ER can own the foreground) this authorizes the can-move probe to
/// force ER foreground while injecting so the walk actually registers. OFF by default so it NEVER
/// steals focus in a user-present session. Cached.
pub(crate) fn probe_foreground_enabled() -> bool {
    // DISABLED (user 2026-07-20): do NOT force ER to the foreground during runs -- it steals the user's
    // mouse to ER's screen. The focus-override (IsEnableControlOnDisactiveWindow->1) + DLUID+0x88d
    // stay_active already make the injected pad input apply while ER is UNFOCUSED (run 202410 proved the
    // char moved while unfocused), so foreground-forcing is unnecessary. bd
    // user-stop-forcing-foreground-focus-override-makes-it-unneeded-disable-probe-foreground-2026-07-20.
    false
}
/// SELF-DRIVEN SYSTEM->QUIT->LOAD-PROFILE REPRO AUTOPILOT (er-effects-system-quit-repro.txt /
/// ER_EFFECTS_SYSTEM_QUIT_REPRO). OFF by default. When on, after the boot autoload reaches the
/// world, the DLL keeps the input block engaged and injects a scripted DInput keyboard sequence --
/// gated on OBSERVED menu-window transitions (IngameTop / OptionSetting / ProfileSelect), never on
/// timers -- to open the escape/system menu, activate the cloned Load-Profile (Quit Game) row, move
/// the ProfileSelect cursor to a non-current slot, and confirm. This drives the exact user flow with
/// zero human input so the switch bug (return-title reload crash / wrong-slot) reproduces
/// deterministically. Diagnostic repro harness, not a product lever.
/// True when the separate `er_input_harness_dll.dll` is loaded in the process (i.e. listed in the ME3
/// profile). This is the DECOUPLED TOGGLE for the load2 flow (bd
/// harness-orchestrates-product-exposes-primitives-boundary / load2-flow-decoupled-into-harness-dll):
/// the product ships with the load2 driver INERT; including the harness DLL in the profile turns it on.
/// This is a runtime module-presence check (`GetModuleHandle`), NOT a marker file or env var -- it
/// passes check-marker-file-gates / check-env-gate-comments because it gates on real process state,
/// exactly the "conditional INCLUSION, not conditional gating" the user asked for.
pub(crate) fn harness_dll_present() -> bool {
    static CACHED: AtomicUsize = AtomicUsize::new(0); // 0 = not-yet-seen, 1 = present
    if CACHED.load(Ordering::Relaxed) == 1 {
        return true;
    }
    let present = unsafe { GetModuleHandleA(PCSTR(b"er_input_harness_dll.dll\0".as_ptr())) }
        .map(|h| !h.is_invalid())
        .unwrap_or(false);
    if present {
        CACHED.store(1, Ordering::Relaxed);
    }
    present
}
pub(crate) fn system_quit_repro_enabled() -> bool {
    harness_dll_present()
}
/// DISPROVEN/LEGACY menu-drive escape hatch -- deliberately OFF by default and HARD to trigger.
///
/// The own_stepper "title-confirm" Load drive (fire_titletop_load_entry + the d180-locate walk) was
/// built on a MISIDENTIFIED function: RTTI on the dearxan-deobfuscated image proved 0x14078e1c0 is
/// `CommandSelectDialog::Update` (an in-game dialog), NOT the title menu's confirm router, so its
/// offsets (cursor [+0xb0c], rows [+0x1290]) do NOT apply to the TitleTopDialog at owner+0xe0
/// (RTTI vt 0x142b26468). See bd rtti-correction-0x14078e1c0-is-commandselectdialog-not-title-
/// confirm-2026. We keep the code (it still has diagnostic value) but it must NEVER be the default
/// path: a fresh session running plain own_stepper must not take this wrong route. The trigger name
/// is intentionally obscure so it cannot be stumbled into -- enable ONLY to revisit the dead path.
pub(crate) fn legacy_menu_drive_enabled() -> bool {
    false
}
/// WORLD-RES STREAMING-DRIVER COLD-BUILD PROBE gate (env ER_EFFECTS_WORLDRES_COLDBUILD /
/// er-effects-worldres-coldbuild.txt). OFF by default. When on, own_stepper runs a ONE-SHOT,
/// SAVE-SAFE probe at the parked title that cold-builds the CSEmkResManImp streaming driver
/// (0x143d7c088) + registers the stream worker (0x144842d40) via the CSResStep tick getter
/// 0x140cd6c50 with a stub `this` -- NO SetState, NO world load, zero save-write risk. See bd
/// emk-resman-streaming-driver-coldbuild-stub-lever-2026.
pub(crate) fn worldres_coldbuild_probe_enabled() -> bool {
    false
}
/// COLD CHAR-MOUNT experiment gate (env ER_EFFECTS_COLD_CHAR_MOUNT / er-effects-cold-char-mount.txt,
/// OFF by default). The DECISIVE save-data experiment (save-io-infra-present-cold-char-mount-is-the-
/// decisive-untested-experiment-2026): with the stream worker REGISTERED, can the b80 save-IO read
/// drain to resident so 0x67b290 mounts the real char -- zero-input, SAVE-SAFE (reads the save,
/// applies char to memory; NO SetState, NO save write).
pub(crate) fn cold_char_mount_enabled() -> bool {
    COLD_CHAR_MOUNT_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
}
/// SAVE-SAFE verify-only OWN-LOAD buffer-feed gate. OFF by default; enable via the reliable
/// autoload-file channel (`own_load=1` in er-effects-autoload.txt -> `OWN_LOAD_FILE_ARMED`), env
/// `ER_EFFECTS_OWN_LOAD=1`, or a GAME_DIR file `er-effects-own-load.txt`. When ON, `own_load_drive`
/// hooks the FSM-gated save read 0x67b100, feeds it our sliced plaintext .sl2 slot body, calls the
/// native parser 0x67b290(slot) in-process, then reads back GameMan+0xc30 + the PlayerGameData
/// fingerprint. NO SetState5, NO autosave, NO continue_confirm.
pub(crate) fn own_load_enabled() -> bool {
    OWN_LOAD_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
}
/// GOLDEN BASELINE world-stream observe mode (er-effects-golden-observe.txt / ER_EFFECTS_GOLDEN_OBSERVE).
/// OFF by default; purely ADDITIVE and OBSERVE-ONLY -- it fires NO continue/SetState5/load of any kind.
/// When armed, the SAME recurring world-stream observer (`own_load_stream_observe_recurring`) runs on a
/// NORMAL (vanilla, menu-driven) load too, so we can capture a GOLDEN baseline to diff against the
/// menu-free OWN-LOAD stall. On a vanilla load neither `OWN_LOAD_CONTINUE_FIRED` nor the cached
/// pointers from our continue_confirm are set, so golden mode instead has `own_stepper_idx10` cache the
/// live TITLE owner into `OWN_LOAD_OWNER_CACHED` every title frame (the owner pointer is stable), and
/// the observer re-derives InGameStep/MoveMapStep LIVE from that owner each frame (its existing
/// `ingame_cached == 0` fallback) as the vanilla load builds the world.
pub(crate) fn golden_observe_enabled() -> bool {
    false
}
/// Whether the FINAL guarded `continue_confirm`/`SetState5` world-stream step is armed. SAVE-WRITING
/// when it fires (`SetState5` autosaves), so it stays OFF by default: `own_load_drive` is verify-only
/// unless this is explicitly armed via the autoload-file channel (`own_load_continue=1` in
/// er-effects-autoload.txt -> `OWN_LOAD_CONTINUE_FILE_ARMED`), env `ER_EFFECTS_OWN_LOAD_CONTINUE=1`,
/// or a GAME_DIR file `er-effects-own-load-continue.txt`. The hard c30/fingerprint guard inside
/// `own_load_drive` is the absolute save-safety backstop even when this is armed.
pub(crate) fn own_load_continue_enabled() -> bool {
    OWN_LOAD_CONTINUE_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
}
/// Whether the OWN-LOAD m28 direct-enqueue lever (`AddDefaultFileLoadProcess`) is ARMED. This is the
/// arming gate ONLY; the lever additionally requires `OWN_LOAD_CONTINUE_FIRED` (our menu-free path
/// actually fired) at fire time, so on a vanilla native menu load -- where that flag is never set --
/// it can NEVER dispatch even if armed. Arm via the autoload-file channel (`own_dispatch=1` in
/// er-effects-autoload.txt -> `OWN_DISPATCH_FILE_ARMED`), env `ER_EFFECTS_OWN_DISPATCH=1`, or a
/// GAME_DIR file `er-effects-own-dispatch.txt`. SAVE-SAFE: reaches only world-asset file-load
/// streaming (RequestDCX -> RSResourceFileRequest -> GLOBAL_LoadManager), never save IO.
pub(crate) fn own_dispatch_enabled() -> bool {
    OWN_DISPATCH_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
}
/// Whether the menu-free LoadGame-JOB INSTALL lever is ARMED. When set (alongside `own_load`, which
/// makes `own_load_drive` run), the verify-only parse is followed by BUILD (`FUN_140826510`) +
/// INSTALL (`FUN_1407a9560`) of the native LoadGame `MenuJobWithContext` into the title owner's
/// `+0x130` MenuJob slot -- replacing the idle `IfElseJob` so `STEP_MenuJobWait` ticks it (self-build
/// -> deser -> world stream). This is the NON-SetState5 alternative to `own_load_continue`: no
/// `SetState5`, no autosave, no save write (build + first-tick deser only READ the save). OFF by
/// default; arm via the autoload-file channel (`own_load_install_job=1` ->
/// `OWN_LOAD_INSTALL_JOB_FILE_ARMED`), env `ER_EFFECTS_OWN_LOAD_INSTALL_JOB=1`, or a GAME_DIR file
/// `er-effects-own-load-install-job.txt`.
pub(crate) fn own_load_install_job_enabled() -> bool {
    OWN_LOAD_INSTALL_JOB_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
}
/// Whether the PATH B menu-free PRIVATE-PUMP lever (`own_load_pump`) is ARMED. When set (alongside
/// `own_load`, which makes `own_load_drive` run the verify-only parse), the parse is followed by BUILD
/// of the LoadGame `MenuJobWithContext` with REAL mss-derived ctx; the recurring game task then ticks
/// its `Run` privately every frame to completion (deser -> map stream -> m28 mount) and, once it reaches
/// `state==Success`, fires the guarded SetState5 transition ONCE. This is the "own the load" rebuild --
/// no owner+0x130 install, no CSMenuMan dialog, no queue. OFF by default; arm via the autoload-file
/// channel (`own_load_pump=1` -> `OWN_LOAD_PUMP_FILE_ARMED`), env `ER_EFFECTS_OWN_LOAD_PUMP=1`, or a
/// GAME_DIR file `er-effects-own-load-pump.txt`.
pub(crate) fn own_load_pump_enabled() -> bool {
    OWN_LOAD_PUMP_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
}
/// SAVE-SAFE PROBE GATE for `own_load_pump`: when set, the pump runs the corrected BUILD + per-frame
/// `Run` (deser -> map-stream, all READ-only up to world-stream per the path-b spec) but, on reaching
/// `state==Success`, LOGS the result and latches DONE WITHOUT firing the save-writing SetState5
/// transition. This isolates the dialog-ctx correction (does the build no longer AV? does the pump
/// progress to Success?) with ZERO save write -- so it can run against the user's real save with no
/// swap and no autosave risk. OFF by default; env `ER_EFFECTS_OWN_LOAD_PUMP_VERIFY=1` or a GAME_DIR
/// file `er-effects-own-load-pump-verify.txt`.
pub(crate) fn own_load_pump_verify_only() -> bool {
    false
}
/// DIRECT "Continue pressed" trigger (bd LIVE-continue-chain-via-selector-NOT-confirm-handler):
/// once the title is at the settled main menu (STEP_MenuJobWait) after press-any-button AND
/// GameMan/GameDataMan is set up, write the exact bit the native Continue path consumes --
/// `*(TitleFlowContext+0x14c) = 1` (+ the save slot at `mss+0x1200`) -- so the native selector
/// `0x1409a8eb0` dispatches the load through the engine's own pump. ZERO simulated input: a pure
/// in-process field write replicating the confirm handler's side effects. OFF by default; arm via
/// env `ER_EFFECTS_FIRE_TFC_CONTINUE=1` or a GAME_DIR file `er-effects-fire-tfc-continue.txt`.
pub(crate) fn fire_tfc_continue_enabled() -> bool {
    false
}
/// Direct ProfileLoadDialog build mode (er-effects-direct-build.txt / ER_EFFECTS_DIRECT_BUILD).
/// OFF by default: a plain own_stepper run stays the safe read-only scan; the native dialog build
/// (which leads to a guarded SetState(5) save-write via STAGE 2) fires only when deliberately
/// enabled, so the first native-build run is a deliberate, save-backed experiment.
pub(crate) fn direct_build_enabled() -> bool {
    false
}
