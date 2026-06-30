#![allow(unused_imports)]

use std::{
    ffi::c_void,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use crate::input_blocker::InputBlocker;
use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};
use eldenring::{
    cs::{
        CSTaskGroupIndex, CSTaskImp, ChrInsExt, FaceData, FaceDataBuffer, GameDataMan, GameMan,
        PlayerGameData, PlayerIns,
    },
    dlkr::DLAllocator,
    fd4::FD4TaskData,
};
use er_effects_data::embedded_effects;
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoader};
use fromsoftware_shared::{F32Vector4, FromStatic, InstanceError, SharedTaskImpExt};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
        System::{
            LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA},
            Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery},
            SystemServices::DLL_PROCESS_ATTACH,
            Threading::GetCurrentProcessId,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_KEYDOWN,
            WM_KEYUP,
        },
    },
    core::{BOOL, PCSTR},
};

mod constants;
mod crashlog;
mod effects;
mod experiments;
mod ffi;
mod hooks;
mod input_blocker;
mod mh;
mod telemetry;

#[allow(unused_imports)]
use crate::{
    constants::*, crashlog::*, effects::*, experiments::*, ffi::*, hooks::*, telemetry::*,
};

// Constants/statics live in constants.rs; keep lib.rs focused on DLL entrypoints and task wiring.
#[derive(Default)]
pub(crate) struct SafeInputRuntime {
    loaded: bool,
    confirm_count: u32,
    pulses_sent: u32,
    interval_ticks: u64,
    initial_delay_ticks: u64,
    last_pulse_tick: u64,
    hooks_requested: bool,
    last_status: Option<String>,
}

pub(crate) struct EffectsState {
    calls: Vec<NamedEffectCall>,
    /// Parse error for the embedded `data/effects.json`, shown in the overlay
    /// instead of silently starting with an empty list.
    load_error: Option<String>,
    current_animation_id: Option<i32>,
    /// Latched when the expected appear animation is observed either as current or as a queue write
    /// between task ticks; runtime proof needs the semantic event, not a one-frame sampling race.
    expected_animation_seen: bool,
    applied_for_current_appear: bool,
    /// TimeAct queue write index at the previous tick; used to detect appear
    /// animations that were enqueued (and possibly finished) between ticks.
    last_write_idx: Option<u32>,
    manual_apply_requested: bool,
    remove_all_requested: bool,
    network_sync: bool,
    custom_call_id: i32,
    last_telemetry_write: Option<Instant>,
    last_driver_command: Option<String>,
    autoload: SaveLoader,
    game_task_ticks: u64,
    safe_input: SafeInputRuntime,
}

impl Default for EffectsState {
    fn default() -> Self {
        let (calls, load_error) = match embedded_effects() {
            Ok(effects) => (
                effects
                    .calls
                    .into_iter()
                    .map(named_call_from_spec)
                    .collect(),
                None,
            ),
            Err(error) => (
                Vec::new(),
                Some(format!("failed to parse embedded effects.json: {error}")),
            ),
        };

        Self {
            calls,
            load_error,
            current_animation_id: None,
            expected_animation_seen: false,
            applied_for_current_appear: false,
            last_write_idx: None,
            manual_apply_requested: false,
            remove_all_requested: false,
            network_sync: false,
            custom_call_id: CUSTOM_CALL_DEFAULT_ID,
            last_telemetry_write: None,
            last_driver_command: None,
            autoload: SaveLoader::from_env(),
            game_task_ticks: INITIAL_GAME_TASK_TICKS,
            safe_input: SafeInputRuntime::default(),
        }
    }
}

type DirectInput8CreateFn =
    unsafe extern "system" fn(HINSTANCE, u32, *const c_void, *mut *mut c_void, *mut c_void) -> i32;

static DIRECTINPUT8_CREATE_FORWARD: AtomicUsize = AtomicUsize::new(DIRECTINPUT_FORWARD_UNRESOLVED);

unsafe fn directinput8_create_forward() -> Option<DirectInput8CreateFn> {
    let cached = DIRECTINPUT8_CREATE_FORWARD.load(Ordering::SeqCst);
    if cached != DIRECTINPUT_FORWARD_UNRESOLVED {
        return Some(unsafe { std::mem::transmute::<usize, DirectInput8CreateFn>(cached) });
    }
    let module = unsafe { LoadLibraryA(PCSTR(DINPUT8_SYSTEM_DLL.as_ptr())) }.ok()?;
    let proc = unsafe { GetProcAddress(module, PCSTR(DIRECTINPUT8_CREATE_SYMBOL.as_ptr())) }?;
    let addr = proc as usize;
    DIRECTINPUT8_CREATE_FORWARD.store(addr, Ordering::SeqCst);
    Some(unsafe { std::mem::transmute::<usize, DirectInput8CreateFn>(addr) })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// This is the DINPUT8.dll proxy export Elden Ring imports. It forwards to the
/// system DINPUT8 implementation after our repo-built DLL is loaded as dinput8.dll.
pub unsafe extern "system" fn DirectInput8Create(
    hinst: HINSTANCE,
    version: u32,
    riid: *const c_void,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> i32 {
    let Some(forward) = (unsafe { directinput8_create_forward() }) else {
        return DIRECTINPUT_FORWARD_ERROR_MOD_NOT_FOUND;
    };
    unsafe { forward(hinst, version, riid, out, outer) }
}

#[unsafe(no_mangle)]
/// # Safety
///
/// This is called by Windows when the DLL is loaded. Do not call it directly.
pub unsafe extern "C" fn DllMain(_hmodule: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason != DLL_PROCESS_ATTACH {
        return DLL_MAIN_SUCCESS;
    }
    write_bootstrap_event(BOOTSTRAP_EVENT_DLL_MAIN_ATTACH, BOOTSTRAP_DETAIL_START);

    // Boot profiler: spawn the independent CPU sampler FIRST so it captures the engine-init threads
    // during the pre-CSTaskImp-instance gap (the largest uninstrumented boot window). Read-only by
    // default (QueryThreadCycleTime/GetThreadTimes, no thread suspension); RIP sampling is a separate
    // opt-in sub-switch. Gated OFF unless ER_EFFECTS_PROFILE=1 / er-effects-profile.txt.
    if profiler_enabled() {
        START_BOOT_PROFILER.call_once(spawn_boot_profiler);
    }

    // Install the crash/exit logger first so it can observe an exit or access
    // violation from any later subsystem. Opt-in; off by default.
    if crash_logger_enabled() {
        install_crash_logger();
    }

    // SAVE-SOURCE ENFORCEMENT (save-override-no-default-fallback-mandatory-env-2026-06-23).
    // The DLL must NEVER assume / read the default user save directory. Unless this is a pure
    // telemetry/observe-only run (loads nothing), a valid `ER_EFFECTS_SAVE_FILE` MUST be present or
    // the process ABORTS here -- before the title flow or any save IO runs. On success, install the
    // CreateFileW/CopyFileW save-path redirect (scoped Win32 hook) so every save artifact (.sl2/.co2/
    // .bak, read AND write) is served from the env-provided directory instead of the default dir.
    match enforce_save_override_or_abort() {
        // Telemetry-only: install the hooks ONLY when the save-trace gate is on (diagnostics only --
        // no redirect dir, so the detours just log and pass through). Lets us trace the working
        // vanilla save-read (char-present save in the real appdata, no redirect).
        SaveOverrideMode::TelemetryOnly => {
            if save_trace_enabled() {
                START_SAVE_REDIRECT.call_once(|| {
                    let _ = std::thread::Builder::new()
                        .name("er-effects-save-trace".to_owned())
                        .spawn(install_save_redirect_hooks);
                });
            }
        }
        SaveOverrideMode::Redirect => {
            START_SAVE_REDIRECT.call_once(|| {
                let _ = std::thread::Builder::new()
                    .name("er-effects-save-redirect".to_owned())
                    .spawn(install_save_redirect_hooks);
            });
        }
    }

    let initial_state = EffectsState::default();
    arm_product_autoload_from_request(&initial_state.autoload);
    let state = Arc::new(Mutex::new(initial_state));

    // Splash-skip: apply the clean BeginLogo branch-flip as early as possible,
    // from a thread, so it lands before the title state machine runs state 2.
    if splash_skip_enabled() {
        START_SPLASH_SKIP.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-splash-skip".to_owned())
                .spawn(apply_splash_skip);
        });
    }

    // Online-disable: patch GameMan::IsOnlineMode -> always-offline so the boot never attempts
    // online login and the "Unable to start in online mode" modal is never raised -- the headless
    // autoload reaches the real title/main-menu directly. Same early-attach pattern as splash-skip.
    if online_disable_enabled() {
        START_ONLINE_DISABLE.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-online-disable".to_owned())
                .spawn(apply_online_disable);
        });
    }

    // Foreground-force: ALWAYS ON (user directive 2026-06-21 -- "if it works, keep it on"),
    // independent of online-disable. The unfocused-window fps throttle hits during boot (before any
    // cold-mount runs), so patch it at attach so the game always runs full speed regardless of which
    // window holds focus. Verified to make a cold probe boot at 60fps unfocused (was ~6fps). Benign:
    // it only removes the background throttle/auto-pause; input is blocked during probes anyway.
    START_FOREGROUND_FORCE.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-foreground-force".to_owned())
            .spawn(apply_foreground_force);
    });

    // Passive title-resource observer is deliberately independent of the cover/hide bundle: recent
    // branches have kept the stock logo invisible, so resource-path proof must not depend on any
    // visual/logo-hide state.
    if title_menu_resource_observer_enabled() {
        START_TITLE_MENU_RESOURCE_ACQUIRE_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-resource-observer".to_owned())
                .spawn(install_title_menu_resource_acquire_observer_hook);
        });
    }

    // Title-cover masquerade Part A: install the BeginTitle `05_000_Title` hook as early as
    // splash/foreground patches, before STEP_BeginTitle can build the native title Scaleform. This
    // does NOT touch STEP_Wait or CSMenuMan+0x21; it preserves the native MenuWindowJob and hides
    // only its draw bit from the MenuWindowJob::Run/FadeIn path.
    if title_native_menu_visual_suppression_enabled() {
        START_TITLE_NATIVE_MENU_VISUAL_SUPPRESS.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-cover-part-a".to_owned())
                .spawn(install_title_native_menu_visual_suppression_hook);
        });
        START_TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-cover-render".to_owned())
                .spawn(install_title_native_menu_visual_render_suppression_hook);
        });
        START_TITLE_LOGO_FORCE_HIDDEN.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-logo-force-hidden".to_owned())
                .spawn(install_title_logo_force_hidden_hooks);
        });
        START_TITLE_LOGO_START_LOGIN_HIDE.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-logo-start-login-hide".to_owned())
                .spawn(install_title_logo_start_login_hide_hook);
        });
        START_TITLE_PAB_INFORMATION_COVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-pab-cover".to_owned())
                .spawn(install_title_pab_information_visual_hook);
        });
        START_TITLE_GFX_VALUE_SET_VISIBLE.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-gfx-visible".to_owned())
                .spawn(install_title_gfx_value_set_visible_hook);
        });
        START_TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-child-bind".to_owned())
                .spawn(install_title_scene_obj_proxy_named_child_bind_hook);
        });
        START_TITLE_SCALEFORM_BIND_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-bind-observer".to_owned())
                .spawn(install_title_scaleform_bind_observer_hook);
        });
        START_TITLE_MENU_RESOURCE_ACQUIRE_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-resource-observer".to_owned())
                .spawn(install_title_menu_resource_acquire_observer_hook);
        });
        // Do not install the independent custom-cover MenuWindowJob pump here. Runtime artifact
        // product-continue-direct-20260628-121039 proved that pumping a separate 01_900_Black job
        // keeps job+0x130 live and stalls the title flow before player/world. Future cover work must
        // use an epilogue-neutral path (mutate an already-scheduled title surface/resource, or prove
        // explicit completion semantics before adding an independent MenuWindowJob).
        START_TITLE_FLOW_CONTEXT_RECORD_REGULATION.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-tfc-record-fix".to_owned())
                .spawn(install_title_flow_context_record_regulation_fix_hook);
        });
    } else if title_resource_memory_gfx_enabled() {
        // Branch-owned `05_001_Title_Logo` replacement: keep TitleBack visible, but hide the later
        // title text layers (`PRESS ANY BUTTON` / Continue-ish title information) so the custom
        // resource is not overdrawn by native text. Do not install the TitleBack/logo hide hooks here.
        START_TITLE_PAB_INFORMATION_COVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-text-latch".to_owned())
                .spawn(install_title_pab_information_visual_hook);
        });
        START_TITLE_GFX_VALUE_SET_VISIBLE.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-text-gfx-visible".to_owned())
                .spawn(install_title_gfx_value_set_visible_hook);
        });
        START_TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-text-child-bind".to_owned())
                .spawn(install_title_scene_obj_proxy_named_child_bind_hook);
        });
        START_TITLE_SCALEFORM_BIND_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-title-text-bind-observer".to_owned())
                .spawn(install_title_scaleform_bind_observer_hook);
        });
    } else if native_profile_capture_enabled() {
        // Native ProfileSelect diagnostic: install only the passive Scaleform bind observer. Do not
        // install title-cover/custom-cover hooks; this mode is specifically meant to prove native
        // ProfileSelect/profile-renderer provenance without the product cover mutation path.
        START_TITLE_SCALEFORM_BIND_OBSERVER.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-native-profile-bind-observer".to_owned())
                .spawn(install_title_scaleform_bind_observer_hook);
        });
    }

    // Now-loading background portrait forge: install the replace-bind hook early (well before the
    // ~17s now-loading-screen lifecycle) so it is resident when the first MENU_Load_ background is
    // produced. The hook self-gates on product_autoload_enabled() + the MENU_Load_ symbol and is
    // fail-open (any non-matching symbol or build/alloc failure tail-calls the original), so
    // installing it unconditionally is inert outside the product autoload path. Route-independent.
    START_LOADING_BG_REPLACE_BIND.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-loading-bg-portrait".to_owned())
            .spawn(install_loading_bg_replace_bind_hook);
    });
    // D3D12 PRESENT OVERLAY: the deterministic display path -- draw the captured portrait directly onto the
    // swapchain backbuffer when the now-loading screen is up (the in-pipeline forge/Scaleform routes cannot
    // drive the displayed image). Install only on the portrait path (diagnostic), via the dummy-swapchain
    // vtable technique. Phase 1 is log-only (proves the hook fires) before any backbuffer write.
    if portrait_lookat_enabled() {
        START_PRESENT_OVERLAY.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-present-overlay".to_owned())
                .spawn(install_present_overlay_hook);
        });
    }
    // Portrait-renderer teardown SPARE hook: keep the loaded character's portrait renderer alive past the
    // Continue teardown so we can drive realtime look-at + render it post-Continue (the persistent-model
    // path -- the cycling menu can't show a stable portrait). The hook self-gates on product_autoload and
    // only spares a renderer whose model is BUILT (the blank-renderer misfire is guarded in the hook).
    START_PROFILE_RENDERER_TEARDOWN_SPARE.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-portrait-spare".to_owned())
            .spawn(install_profile_renderer_teardown_spare_hook);
    });

    // MenuWindow latch: install the SceneObjProxy ctor hook (0x14074a700) as early as the
    // splash-skip / online-disable patches, from a thread, so it lands BEFORE the title state
    // machine builds the title dialog during boot. On each VALID call it latches rdx (the engine-
    // verified host MenuWindow*) for the live-dialog Load-Game path; pure latch + passthrough.
    // OPT-IN (off by default): only install when `menu_window_latch_enabled()` is set
    // (env ER_EFFECTS_MENU_WINDOW_LATCH=1 OR GAME_DIR file er-effects-menu-window-latch.txt).
    // When off, the hook is never installed (no MinHook, no detour) -- a clean run has neither.
    if menu_window_latch_enabled() || product_autoload_enabled() || native_profile_capture_enabled()
    {
        START_MENU_WINDOW_LATCH.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-menu-window-latch".to_owned())
                .spawn(install_menu_window_latch_hook);
        });
    }

    // Native/asset-backed policy-window oracle: hook the TosTitle constructor early in product
    // autoload runs. Any hit means the Privacy/ToS surface was constructed and the runtime proof is
    // invalid; this is detection only, never auto-accept.
    if product_autoload_enabled() {
        START_POLICY_TOS_TITLE_HOOK.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-policy-oracle".to_owned())
                .spawn(install_policy_tos_title_hook);
        });
        START_SERVER_STATUS_HOOK.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-server-status-oracle".to_owned())
                .spawn(install_server_status_hook);
        });
    }

    // SAVE-SAFE c30-writer diagnostic: install the MinHook on the SOLE GameMan+0xc30
    // writer 0x67bd70 UNCONDITIONALLY at process attach (same early-attach pattern as the
    // MenuWindow latch). Pure passthrough + log of the c30-write gate, c30 before/after,
    // and a window of the resident save buffer -- NO SetState5, NO save write, harmless.
    // OPT-IN (off by default): only install when `c30_writer_diag_enabled()` is set
    // (env ER_EFFECTS_C30_DIAG=1 OR GAME_DIR file er-effects-c30-diag.txt). When off, the
    // hook is never installed (no MinHook, no detour on the hot 0x67bd70 deserialize path).
    if c30_writer_diag_enabled() {
        START_C30_WRITER_HOOK.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-c30-writer-hook".to_owned())
                .spawn(install_c30_writer_hook);
        });
    }

    if safe_input_path().exists() {
        START_SAFE_INPUT_HOOKS.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-safe-input-hooks".to_owned())
                .spawn(install_safe_input_hooks);
        });
    }
    if trace_continue_enabled() && !continue_trace_disabled() {
        write_bootstrap_event(
            BOOTSTRAP_EVENT_CONTINUE_TRACE_REQUESTED,
            BOOTSTRAP_DETAIL_START,
        );
        START_CONTINUE_TRACE.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-continue-trace".to_owned())
                .spawn(install_continue_trace_hooks);
        });
    }
    write_bootstrap_event(BOOTSTRAP_EVENT_GAME_TASK_REQUESTED, BOOTSTRAP_DETAIL_START);
    START_GAME_TASK.call_once({
        let state = Arc::clone(&state);
        move || spawn_game_task(state)
    });

    write_bootstrap_event(
        BOOTSTRAP_EVENT_OVERLAY_SKIPPED_AUTOLOAD,
        BOOTSTRAP_DETAIL_DONE,
    );
    DLL_MAIN_SUCCESS
}

pub(crate) fn wait_for_task_instance() -> &'static CSTaskImp {
    let mut wait_attempts = 0_u64;
    loop {
        match unsafe { CSTaskImp::instance() } {
            Ok(instance) => return instance,
            Err(InstanceError::NotFound(_)) | Err(InstanceError::Null(_)) => {
                wait_attempts = wait_attempts.saturating_add(1);
                if wait_attempts == 1 || wait_attempts % TASK_INSTANCE_WAIT_LOG_INTERVAL == 0 {
                    let detail = format!("attempts={wait_attempts}");
                    write_bootstrap_event(BOOTSTRAP_EVENT_GAME_TASK_WAITING_INSTANCE, &detail);
                }
                std::thread::yield_now()
            }
        }
    }
}

pub(crate) fn spawn_game_task(state: Arc<Mutex<EffectsState>>) {
    std::thread::spawn(move || {
        write_bootstrap_event(
            BOOTSTRAP_EVENT_GAME_TASK_THREAD_STARTED,
            BOOTSTRAP_DETAIL_START,
        );
        let cs_task = wait_for_task_instance();
        write_bootstrap_event(
            BOOTSTRAP_EVENT_GAME_TASK_INSTANCE_READY,
            BOOTSTRAP_DETAIL_DONE,
        );
        // Boot-phase marker: CSTaskImp resolved -> bounds the end of the pre-instance engine-init
        // gap (the largest uninstrumented boot window) in the same [+Nms] timeline the renderer parses.
        if profiler_enabled() {
            append_autoload_debug(format_args!("boot-phase: cstask_instance_ready"));
        }

        cs_task.run_recurring(
            move |task_data: &FD4TaskData| {
                // Boot-phase marker: first frame our recurring task actually ticks.
                if profiler_enabled()
                    && BOOT_FIRST_FRAME_LOGGED
                        .swap(GAME_TASK_TICK_INCREMENT as usize, Ordering::SeqCst)
                        == 0
                {
                    append_autoload_debug(format_args!("boot-phase: first_game_frame"));
                }
                // Bisect kill-switch: do nothing per frame. Isolates "our task
                // body crashes the title ~19s" from "the DLL's mere presence".
                if inert_mode() {
                    return;
                }
                // Hardware write-watchpoint on GameMan+0xc30: (re)arm each frame until
                // the save-mount write is caught, so the VEH logs the exact writer. Runs
                // HARD input block (DInput keyboard+mouse + XInput gamepad), driven from the
                // game task so it is active even when no render callback is running
                // (it does not under the offline launcher at the title). Runs every frame the
                // task ticks -- before the player check -- so a focused window cannot inject any
                // real input during the zero-input own-stepper/autoload probe. Pure suppression,
                // never synthesis.
                if block_input_enabled() {
                    enforce_input_block_now();
                } else {
                    release_input_block_now();
                }
                // D3D12 PRESENT OVERLAY: once the GX device is up, find the game's live swapchain and hook
                // its REAL Present (the dummy-swapchain vtable differs under vkd3d-proton). Self-gated
                // (portrait path only, one-shot on success, bounded retries) so it's cheap every frame.
                if let Ok(base) = game_module_base() {
                    unsafe { try_install_game_present_hook(base) };
                }
                // before the player check so it arms at the title (pre-load), independent
                // of the active observe/own-stepper mode.
                if c30_watch_enabled() {
                    if let Ok(base) = game_module_base() {
                        let frame = C30_WATCH_FRAME_COUNTER
                            .fetch_add(C30_WATCH_HIT_INCREMENT, Ordering::SeqCst)
                            as u64;
                        unsafe { maybe_arm_c30_watch(base, frame) };
                    }
                }
                // RECURRING world-stream observer (own-load-stream-observer-must-be-recurring-task-2026-06-22).
                // Internally no-ops until own_load_continue_fire sets OWN_LOAD_CONTINUE_FIRED, so it
                // costs nothing during normal play and never spams. After continue_confirm/SetState5
                // fires, own_stepper_idx10 (a TITLE-PHASE task) STOPS ticking, so this per-frame game
                // task is the ONLY place that keeps logging the world-stream pump THROUGH the loading
                // screen. Runs BEFORE the player check so it ticks while there is no player yet (the
                // loading-screen frames are exactly when player_present is false). Pure reads only.
                // GOLDEN baseline mode (golden_observe_enabled) ALSO drives the observer even though our
                // continue never fired, so a NORMAL user-driven vanilla load is captured for diffing
                // against the menu-free OWN-LOAD stall. The observer self-gates and re-resolves the
                // owner->InGameStep->MoveMapStep chain live from OWN_LOAD_OWNER_CACHED (filled by
                // own_stepper_idx10 each title frame in golden mode). OBSERVE-ONLY: no load is fired.
                // OBSERVE-ONLY WorldBlockRes::Update diagnostic detour (worldblockres-phase-machine-
                // drives-loadstate-to-0xa-2026-06-22): installed ONCE (idempotent) whenever a diagnostic
                // OWN-LOAD / golden-observe context is armed, so normal play is untouched. The detour is a
                // pure-read pass-through (bumps a call counter + tracks max phase/gate atomics, then calls
                // the original and returns its value), so installing early is harmless and never alters
                // load behavior. It answers: is WorldBlockRes::Update ticked at all on our path, and do
                // any blocks' phase ([+0x35]) / FD4 gate ([+0x2f]) advance.
                if own_load_enabled()
                    || own_load_continue_enabled()
                    || own_load_pump_enabled()
                    || golden_observe_enabled()
                {
                    install_wbr_update_hook();
                }
                if (own_load_enabled() && OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst))
                    || golden_observe_enabled()
                {
                    if let Ok(base) = game_module_base() {
                        let gm = game_man_ptr_or_null();
                        let player_present = unsafe { PlayerIns::local_player_mut() }.is_ok();
                        unsafe { own_load_stream_observe_recurring(base, gm, player_present) };
                    }
                }
                // PATH B PRIVATE PUMP (own_load_pump): if own_load_pump_fire built+armed the LoadGame job,
                // tick its Run privately EVERY frame here (the game thread) -- replicating native
                // ExecuteMenuJob's call shape (zero-init MenuJobResult + FD4Time carrying the frame delta)
                // -- to drive self-build -> deser -> m28 stream, then SetState5 on Success. Self-gates on
                // OWN_LOAD_PUMP_JOB != 0 / OWN_LOAD_PUMP_DONE, so it costs nothing until armed+built and
                // never re-pumps once terminal. Must run THROUGH the loading screen (player absent), so it
                // is here in the recurring game task, before the player check. Pure native call + reads.
                if own_load_pump_enabled() {
                    if let Ok(base) = game_module_base() {
                        let gm = game_man_ptr_or_null();
                        let frame_delta = task_data.delta_time.time;
                        unsafe { own_load_pump_tick(base, gm, frame_delta) };
                    }
                }
                // DIRECT "Continue pressed" trigger: at the settled main menu (post press-any-button,
                // GameMan set up), write the exact bit the native selector consumes
                // (*(TitleFlowContext+0x14c)=1), invoke the selector to BUILD the LoadGame job, and
                // PushBackJob it to the dialog queue. Self-gates + fires once; no input. Then DRAIN the
                // queue each frame (FUN_1407a90f0) so the posted job runs to completion (deser+world).
                if fire_tfc_continue_enabled() {
                    if let Ok(base) = game_module_base() {
                        // Autonomous press-any-button: self-fire the open-menu registrar when the
                        // title settles (zero-input), so no real button press is needed.
                        unsafe { maybe_auto_open_menu(base) };
                        // The Continue BUILD now runs IN-CONTEXT from the hooked TitleTopDialog::update
                        // detour (the pump's live-dialog frame), NOT from this game task -- that timing
                        // was the mis-context cause. Install the hook once; the detour fires the build.
                        unsafe { install_title_update_hook(base) };
                        let frame_delta = task_data.delta_time.time;
                        unsafe { tfc_continue_drain_tick(base, frame_delta) };
                    }
                }
                // GOLDEN-PATH zero-input boot -> open menu (DECOUPLED from fire_tfc_continue): the
                // readiness-gated press-any-button advance (hook 0x1407ad1c0 -> set [job+0x1e8]=2)
                // gets PAST press-any-button with no input, then the menu opens with NO selector fire,
                // so an observe run can reach the menu cleanly. bd
                // press-any-button-golden-lever-job1e8-readiness-2026-06-23.
                //
                // The menu OPEN is driven the NATIVE way: set the decoded global accept byte
                // 0x144589bdc=1 once at the settled title so the game's OWN TitleTopDialog::update
                // accept-gate runs the open-menu registrar in its native frame -- which POSTS the
                // Continue/Load/NewGame MenuJob chain AND drains it (MenuWindow::Update) in the same
                // flow, so the rows actually build. A direct registrar self-fire (maybe_auto_open_menu)
                // only POSTED the chain; the native update does not drain a chain it did not open, so
                // the rows never built (continue-scan = 0 nodes, stage 3). Zero-input (decoded accept
                // flag, not a synthesized event). bd er-effects-rs-e9e + rowbuild-mechanism-incontext-
                // openmenu-2026-06-23.
                if pab_advance_enabled() {
                    if let Ok(base) = game_module_base() {
                        unsafe { install_pab_advance_hook(base) };
                        if !native_profile_capture_enabled() {
                            unsafe { maybe_set_title_accept_byte(base) };
                        }
                    }
                }
                // Now-loading helper observer: attach only after the native title accept byte fired.
                // Attach-time detours on CSNowLoadingHelperImp exited before readiness; this delayed
                // install avoids touching the loading helper until the title path has already advanced.
                if product_autoload_enabled()
                    && TITLE_ACCEPT_BYTE_GATE_FIRED.load(Ordering::SeqCst)
                    && NOW_LOADING_HELPER_HOOKS_INSTALLED.load(Ordering::SeqCst) == 0
                {
                    install_now_loading_helper_observer_hooks();
                }
                // Title transition fast-forward (pab_dismiss -> menu_open): scale the title
                // frame-delta so the FadeIn/TextFadeOut/menu Scaleform animation reaches its end
                // frame in fewer wall-clock frames. Default-on product behavior for real runs (the
                // detour self-gates per frame); install once. bd er-effects-rs-urw.
                if title_anim_speedup_enabled() {
                    if let Ok(base) = game_module_base() {
                        unsafe { install_title_anim_speed_hook(base) };
                        // READ-ONLY native state-transition timeline (menu-build-overlap lever
                        // "look before acting" instrument): logs every SetState(owner,int) with a
                        // timestamp so we learn exactly when BeginTitle(3) fires and whether the
                        // 05_000_Title build has headroom to start earlier. Save-safe pass-through.
                        unsafe { install_title_setstate_trace_hook(base) };
                    }
                }
                // OFFLINE connection-state lever (milestone-3 fix): force GameMan+0xBC8/0xBC9 = 0 each
                // title frame so the connection-loss event handlers -- which build the GR_System_Message
                // "Cannot connect to network / connection lost" MessageBoxDialogs our offline boot
                // raises at menu-open -- short-circuit at their `IsInOnlineMode() &&
                // IsServerConnectionEnabled()` guard before enqueuing any popup. Gated by the offline
                // flag (this only forces state the offline boot already intends). bd er-effects-rs-0ye.
                if online_disable_enabled() {
                    // MILESTONE-3 FIX: short-circuit the offline title-flow check jobs to their
                    // no-modal exits so the title flow never enqueues a GR_System_Message MessageBox.
                    // ShowProgressJob::Run is the shared chokepoint for the save/network/sign-in/login
                    // check steps (the 3 observed modals); NetworkCheckJob::Run is the separate J6 job.
                    // Installed once, before menu-open. Offline-gated (no effect on an online check).
                    install_network_check_shortcircuit_hook();
                    install_show_progress_shortcircuit_hook();
                    if let Ok(base) = game_module_base() {
                        unsafe { force_offline_connection_bytes(base) };
                    }
                }
                // DIAGNOSTIC (gated by er-effects-grsysmsg-log.txt): log the GR_System_Message ids the
                // title flow fetches after menu-open, to DEFINITIVELY name the menu-open MessageBoxDialogs
                // (connection 4101/4102/4190 vs save 70000/4191) instead of guessing. Self-gates once.
                // Also install whenever a save load is expected (not telemetry-only / not trace):
                // the same GetGR_System_Message hook carries the corrupted-save SEMAPHORE
                // (oracle_corrupted_save_seen_id), so a load probe records the "save data is corrupted"
                // popup as RAM-read telemetry instead of a one-off on-screen image.
                if grsysmsg_log_enabled()
                    || (!save_override_telemetry_only() && !save_trace_enabled())
                {
                    install_gr_sysmsg_log_hook();
                }
                // Anti-anti-debug (ported from ProDebug, correct base): neutralize FromSoft's
                // timed anti-debug so debug exceptions / our INT3 breakpoints reach our VEH.
                // Runs ONCE, BEFORE arming breakpoints, from the game task (game up, .text
                // decrypted) -- our own controlled timing, not the LazyLoader's.
                if anti_antidebug_enabled() {
                    if let Ok(base) = game_module_base() {
                        unsafe { apply_anti_antidebug_once(base) };
                    }
                }
                // Software (INT3) breakpoints from er-effects-breakpoints.txt: install once.
                // The VEH (crash logger) logs every hit's register/stack context + re-arms.
                if sw_breakpoints_enabled() {
                    if let Ok(base) = game_module_base() {
                        unsafe { install_sw_breakpoints_once(base) };
                    }
                }
                // STAY-ACTIVE: force ER's input-accept flag so a virtual gamepad keeps driving the
                // menus while ER is UNFOCUSED (user can work elsewhere during a golden capture). ER
                // clears [DLUID+0x88d] each frame when it isn't GetActiveWindow; re-set it to 1.
                if stay_active_enabled() {
                    if let Ok(base) = game_module_base() {
                        // DLUID (input-device-manager) singleton VA 0x14485dc18.
                        const DLUID_SINGLETON_RVA: usize =
                            RuntimeGlobalRva::DluidInputManager as usize;
                        #[repr(C)]
                        struct DluidInputManagerLayout {
                            unknown_000: [u8; 0x88d],
                            input_active: u8,
                        }
                        const DLUID_INPUT_ACTIVE_FLAG_OFFSET: usize =
                            core::mem::offset_of!(DluidInputManagerLayout, input_active);
                        const INPUT_ACTIVE: u8 = true as u8;
                        const NULL_DLUID: usize = NULL_MODULE_BASE;
                        let dluid = unsafe { safe_read_usize(base + DLUID_SINGLETON_RVA) }
                            .unwrap_or(NULL_DLUID);
                        // Defensive: only write once the flag byte is confirmed READABLE (so a
                        // not-yet-initialized or bad singleton ptr can never fault the game thread).
                        if dluid != NULL_DLUID
                            && unsafe { safe_read_usize(dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET) }
                                .is_some()
                        {
                            unsafe {
                                *((dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET) as *mut u8) =
                                    INPUT_ACTIVE
                            };
                        }
                    }
                }
                let Ok(player) = (unsafe { PlayerIns::local_player_mut() }) else {
                    let mut state = state_or_return(&state);
                    state.game_task_ticks += GAME_TASK_TICK_INCREMENT;
                    // Install the MessageBoxDialog builder hook for native telemetry. Product
                    // autoload must NOT auto-accept: every pre/post-load message box is a hard
                    // investigation trigger whose semantic side effect must be skipped directly.
                    // The legacy OK-handler dismiss path remains only for non-product probes.
                    if online_disable_enabled() {
                        install_auto_accept_hook();
                        if !product_autoload_enabled() {
                            force_dismiss_startup_dialog();
                        }
                    }
                    // Observe the natural flow PAST the modal: tap Confirm (game's own input).
                    if auto_confirm_enabled() {
                        auto_confirm_tap();
                    }
                    // Bisect kill-switch: lock + tick only, NO filesystem I/O
                    // (no telemetry write, no experiments). Discriminates "our
                    // per-frame file I/O stalls the title" (lite survives) from
                    // "any per-frame work trips a budget" (lite still exits).
                    if lite_mode() {
                        return;
                    }
                    // Product autoload: run the native title open-menu predicate + minimal
                    // native save-load core from the recurring game task, before the idx10
                    // MenuJobWait hook path is needed. This bypasses title-accept/input
                    // injection while still advancing the data-driven PressStart/PRESS BUTTON
                    // component through its native open-menu registrar; readiness is checked
                    // inside product_core_autoload_tick.
                    if product_autoload_enabled() {
                        PRODUCT_CORE_CALLSITE_TICKS.fetch_add(1, Ordering::SeqCst);
                        let base_result = game_module_base();
                        if base_result.is_ok() {
                            PRODUCT_CORE_CALLSITE_BASE_OK_TICKS.fetch_add(1, Ordering::SeqCst);
                        }
                        let slot_result = state.autoload.slot();
                        if let Some(slot) = slot_result {
                            PRODUCT_CORE_CALLSITE_SLOT_OK_TICKS.fetch_add(1, Ordering::SeqCst);
                            PRODUCT_CORE_CALLSITE_LAST_SLOT.store(slot as usize, Ordering::SeqCst);
                        }
                        if let (Ok(base), Some(slot)) = (base_result, slot_result) {
                            unsafe {
                                product_core_autoload_tick(base, slot, state.game_task_ticks)
                            };
                            // Per-frame: capture the live character portrait CSGxTexture while the
                            // ProfileSelect renderer still exists (it is torn down at Continue), so
                            // the now-loading background forge can display the real portrait. One-shot.
                            // Read the autoload's TARGET slot's renderer table entry, not a hardcoded 0.
                            maybe_capture_portrait_gxtexture(base, slot);
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // FORCE LIVE PROFILE PORTRAIT RENDER (diagnostic, default-OFF): while the user
                    // holds the ProfileSelect/Load-Game screen (valid menu render context, NO
                    // Continue commit), mark the target slot used + kick the async character-model
                    // build so the renderer renders the live 3D head into its offscreen. Menu-phase
                    // only -> no Continue/teardown/world-load crash path. The capture keeps the gx
                    // once the model latches (+0x778). Validates P1 (the build) in isolation.
                    if force_profile_render_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe {
                                force_profile_render_tick(base, FORCE_PROFILE_RENDER_MANUAL_SLOT)
                            };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // OWN-THE-STEPPER: patch the idx10 step-fn slot to our handler so
                    // the FD4 scheduler runs OUR code in-context (step 1: verify the
                    // control point with a logging pass-through).
                    // OWN-STEPPER and the SEPARATE observe-only NATIVE-LOAD gate both install the
                    // idx10 patch so OUR handler runs each frame. own_stepper_idx10 dispatches to
                    // the native-load (observe-only, no forcing) path when native_load_enabled().
                    if own_stepper_enabled()
                        || native_load_enabled()
                        || native_continue_enabled()
                        || native_fullread_enabled()
                        || own_load_enabled()
                    {
                        if let Ok(base) = game_module_base() {
                            unsafe { own_stepper_patch_once(base) };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Pure observe: log the title->menu->load transition each interval
                    // with NO forcing, to capture what the REAL button press does.
                    if observe_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe { title_observe_tick(base, state.game_task_ticks) };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Read-only: log the native autoload-arm preconditions
                    // (especially [slotmgr+0x8]) to decide the zero-input path.
                    if arm_probe_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe { arm_precondition_probe(base, state.game_task_ticks) };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Lever 2: zero-input title-accept via input-event injection
                    // (staged probe -> fill -> inject) to bootstrap the front-end.
                    if title_accept_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe {
                                title_accept_tick(
                                    base,
                                    state.game_task_ticks,
                                    title_accept_inject_enabled(),
                                )
                            };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Corrected play-game submit: on the live FE-host at state 10,
                    // SetState(5) with a packed map (not raw state/slot like the old
                    // force_play_game) so the existing pump builds CSFeMan + loads.
                    if submit_play_game_enabled() {
                        if let (Ok(base), Some(slot)) = (game_module_base(), state.autoload.slot())
                        {
                            unsafe {
                                submit_play_game_once(base, slot, state.game_task_ticks, task_data)
                            };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Per-frame native arm: re-set the slot each frame + latch so
                    // the save-mgr update can arm before the title resets the slot.
                    if native_arm_loop_enabled() {
                        if let (Ok(base), Some(slot)) = (game_module_base(), state.autoload.slot())
                        {
                            unsafe { native_arm_loop_tick(base, slot, state.game_task_ticks) };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Recipe Option 1 (flagless): drive the genuine offline
                    // continue (MoveMapList dispatcher + b73) to load the REAL slot.
                    if continue_drive_enabled() {
                        if let (Ok(base), Some(slot)) = (game_module_base(), state.autoload.slot())
                        {
                            unsafe { continue_drive_tick(base, slot, state.game_task_ticks) };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Recipe B (flagless): drive the outer IngameInit once + pump
                    // the InGameStep. Self-contained -- skips the other autoload
                    // branches to avoid double-submit. Needs the live FD4TaskData.
                    if ingameinit_drive_enabled() {
                        if let (Ok(base), Some(slot)) = (game_module_base(), state.autoload.slot())
                        {
                            unsafe {
                                ingameinit_drive_tick(base, slot, state.game_task_ticks, task_data)
                            };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    process_safe_input_request(&mut state);
                    process_autoload_request(&mut state);
                    // Direct-drive the orphaned InGameStep load once force_play_game
                    // has reached GameStepWait (run 305: hooking the step pump froze
                    // the title, so we call its Execute directly with the live ctx).
                    if ingamestep_pump_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe { ingamestep_pump_tick(base, task_data) };
                        }
                    }
                    write_telemetry_throttled(&mut state, false);
                    return;
                };

                let mut state = state_or_return(&state);
                state.game_task_ticks += GAME_TASK_TICK_INCREMENT;
                // In-world: latch OFF the startup popup auto-accept (in-game dialogs need real
                // choices), optionally clean stale title-dialog render resources, then run the
                // one-shot correctness dump.
                IN_WORLD_REACHED.store(IN_WORLD_REACHED_YES, Ordering::SeqCst);
                if own_stepper_enabled()
                    || native_load_enabled()
                    || native_continue_enabled()
                    || native_fullread_enabled()
                {
                    if let Ok(base) = game_module_base() {
                        unsafe {
                            cleanup_title_dialog_after_world_once(base, state.game_task_ticks)
                        };
                    }
                }
                // In-world correctness oracle: on the FIRST frame the local player exists, log
                // the load-correctness record + the T_controllable timeline marker ONCE. Fires
                // for both a native-menu load (observe) and a DLL-driven load (own-stepper), so
                // the two records are directly comparable (field-for-field == correct load).
                if (own_stepper_enabled()
                    || observe_enabled()
                    || native_load_enabled()
                    || native_continue_enabled()
                    || native_fullread_enabled())
                    && LOAD_CORRECTNESS_DUMPED
                        .swap(GAME_TASK_TICK_INCREMENT as usize, Ordering::SeqCst)
                        == LOAD_CORRECTNESS_NOT_DUMPED
                {
                    if let Ok(base) = game_module_base() {
                        timeline_event(
                            "T_controllable",
                            state.game_task_ticks,
                            format_args!("player=1"),
                        );
                        unsafe { dump_load_correctness(base, state.game_task_ticks) };
                    }
                }
                let observation = observe_animation(player, state.last_write_idx);
                state.current_animation_id = observation.current_animation_id;
                if observation.current_animation_id == Some(APPEAR_ANIMATION_ID)
                    || observation.appear_newly_queued
                {
                    state.expected_animation_seen = true;
                }
                state.last_write_idx = Some(observation.write_idx);

                remove_requested_calls(player, &mut state);
                process_driver_command(player, &mut state);

                let appear_playing = observation.current_animation_id == Some(APPEAR_ANIMATION_ID);
                if !appear_playing {
                    state.applied_for_current_appear = false;
                }

                let should_apply_for_appear = (observation.appear_newly_queued || appear_playing)
                    && !state.applied_for_current_appear;
                let should_apply = should_apply_for_appear || state.manual_apply_requested;
                state.manual_apply_requested = false;

                if should_apply_for_appear {
                    state.applied_for_current_appear = true;
                }

                if should_apply {
                    apply_selected_calls(player, &mut state);
                }

                process_global_driver_command(&mut state);
                refresh_call_status(player, &mut state);
                write_telemetry_throttled(&mut state, true);
            },
            CSTaskGroupIndex::FrameBegin,
        );
        write_bootstrap_event(
            BOOTSTRAP_EVENT_GAME_TASK_RECURRING_REGISTERED,
            BOOTSTRAP_DETAIL_DONE,
        );
        // REALTIME PORTRAIT LOOK-AT draw-phase SWEEP: register the realtime draw task in EACH candidate
        // DRAW phase, so it runs on the render thread inside an actively-recording GX frame (where the
        // profile draw step's GX subcontext-pool pop succeeds -- FrameBegin, above, is before the frame
        // records, so a draw there is a black no-op). Each registration bumps its own per-frame tick
        // counter; only the phase whose index == PROFILE_LOOKAT_SELECTED_PHASE actually rasterizes, so
        // exactly one phase draws per frame. The active phase is switchable live via
        // er-effects-lookat-phase.txt (no recompile), to find one that ticks per-frame at the menu
        // (GameSceneDraw measured ~11% -- world-gated). We own these tasks (cancel() is a fromsoftware-rs
        // no-op + self-leaked Arc), so the chosen one persists past Continue = the loading-screen port.
        // Order MUST match constants::LOOKAT_DRAW_PHASE_NAMES.
        let lookat_phases = [
            CSTaskGroupIndex::Draw_Pre,
            CSTaskGroupIndex::GraphicsStep,
            CSTaskGroupIndex::DrawStep,
            CSTaskGroupIndex::DrawBegin,
            CSTaskGroupIndex::GameSceneDraw,
            CSTaskGroupIndex::AdhocDraw,
            CSTaskGroupIndex::DrawEnd,
            CSTaskGroupIndex::Draw_Post,
        ];
        for (i, phase) in lookat_phases.into_iter().enumerate() {
            cs_task.run_recurring(
                move |_task_data: &FD4TaskData| unsafe { profile_lookat_phase_draw_tick(i) },
                phase,
            );
        }
        // Sweep diagnostic + live selector re-read, paced by a FrameBegin task (ticks every frame).
        cs_task.run_recurring(
            move |_task_data: &FD4TaskData| profile_lookat_phase_diag_tick(),
            CSTaskGroupIndex::FrameBegin,
        );
    });
}

pub(crate) fn process_autoload_request(state: &mut EffectsState) {
    if state.autoload.completed() || state.autoload.slot().is_none() {
        return;
    }

    let Ok(game_man) = (unsafe { GameMan::instance_mut() }) else {
        return;
    };

    let Ok(game_module_base) = game_module_base() else {
        return;
    };

    if selectbot_probe_enabled() || title_proceed_gate_enabled() || title_accept_byte_gate_enabled()
    {
        // selectbot_probe_once samples the SelectBot/pump state each title-idle
        // frame; when ER_EFFECTS_TITLE_PROCEED_GATE is set it ALSO fires the
        // one-shot title-accept latch write (lever 1) at state 10, and when
        // ER_EFFECTS_TITLE_ACCEPT_BYTE is set it fires lever 2 (global accept
        // byte 0x144589bdc) for the zero-input natural menu-open. Returns
        // without completing the autoload so sampling continues across the
        // cascade.
        unsafe { selectbot_probe_once(game_module_base, state.game_task_ticks) };
        return;
    }

    if native_autoload_enabled() {
        // Recipe A: arm the game's own built-in title autoload (slot + force flag)
        // and let the save-manager update perform the load with zero input.
        if let Some(slot) = state.autoload.slot() {
            unsafe { native_autoload_once(game_module_base, slot, state.game_task_ticks) };
        }
        return;
    }

    if force_play_game_enabled() {
        if let Some(slot) = state.autoload.slot() {
            unsafe { call_force_play_game_once(game_module_base, slot, state.game_task_ticks) };
        }
        return;
    }

    if native_title_job_enabled()
        && !unsafe { call_native_title_job_once(game_module_base, state.game_task_ticks) }
    {
        return;
    }

    let context = SaveLoadContext {
        game_module_base,
        title_handoff_complete: TITLE_HANDOFF_COMPLETE.load(Ordering::SeqCst)
            != TITLE_HANDOFF_INCOMPLETE,
        // BYPASS arming signal: engine filled enough to build the LoadGame job at the title (GameDataMan
        // -> mss -> plausible TitleFlowContext), without waiting for the press-any-button handoff.
        loadgame_build_ctx_ready: unsafe {
            crate::experiments::loadgame_build_ctx_ready(game_module_base)
        },
    };
    let _ = unsafe {
        state.autoload.process(game_man, context, |message| {
            append_autoload_debug(format_args!("{message}"))
        })
    };
}

pub(crate) fn state_or_return(
    state: &Arc<Mutex<EffectsState>>,
) -> std::sync::MutexGuard<'_, EffectsState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) struct AnimationObservation {
    current_animation_id: Option<i32>,
    /// True when the appear animation was written into the TimeAct queue
    /// since the previous tick. This catches plays that are too short to be
    /// observed as the "current" slot between two task ticks.
    appear_newly_queued: bool,
    write_idx: u32,
}

/// Reads the player's TimeAct animation state.
///
/// The TimeAct module keeps a 10-slot circular buffer of animations:
/// `read_idx` is the last animation played or updated and `write_idx` is the
/// slot the next animation will be written to. The current animation is the
/// `read_idx` slot; additionally, every slot written since the previous tick
/// (`last_write_idx..write_idx`) is checked for the appear animation. A
/// re-application can occur when a queued appear animation is seen both as
/// newly queued and later as current — SpEffect application is idempotent, so
/// missing a trigger is the worse failure mode.
pub(crate) fn observe_animation(
    player: &PlayerIns,
    last_write_idx: Option<u32>,
) -> AnimationObservation {
    let time_act = &player.chr_ins.modules.time_act;
    let queue_len = time_act.anim_queue.len() as u32;
    let read_slot = (time_act.read_idx % queue_len) as usize;
    let current_animation_id = valid_animation_id(time_act.anim_queue[read_slot].anim_id);
    let write_idx = time_act.write_idx;

    let mut appear_newly_queued = false;
    if let Some(last_write_idx) = last_write_idx {
        let mut index = last_write_idx;
        // Bounded to one lap of the circular buffer in case the write index
        // jumped by more than the queue length between ticks.
        let mut remaining = queue_len;
        while index != write_idx && remaining > ANIM_QUEUE_SCAN_FLOOR {
            let slot = (index % queue_len) as usize;
            if time_act.anim_queue[slot].anim_id == APPEAR_ANIMATION_ID {
                appear_newly_queued = true;
            }
            index = index.wrapping_add(ANIM_QUEUE_SLOT_STEP);
            remaining -= ANIM_QUEUE_SLOT_STEP;
        }
    }

    AnimationObservation {
        current_animation_id,
        appear_newly_queued,
        write_idx,
    }
}

pub(crate) fn valid_animation_id(anim_id: i32) -> Option<i32> {
    (anim_id > INVALID_ANIMATION_ID_FLOOR).then_some(anim_id)
}

pub(crate) fn process_global_driver_command(state: &mut EffectsState) {
    let path = command_path();
    let Ok(raw_command) = fs::read_to_string(&path) else {
        return;
    };
    let command = raw_command.trim();
    if !command.starts_with("load_slot ") {
        return;
    }
    let _ = fs::remove_file(path);

    let parts: Vec<_> = command.split_whitespace().collect();
    state.last_driver_command = Some(match parts.as_slice() {
        ["load_slot", slot] => match slot.parse() {
            Ok(slot) => {
                state.autoload.queue_direct_menu_load(slot);
                process_autoload_request(state);
                format!("ok: {command}")
            }
            Err(error) => format!("error: {command}: invalid slot: {error}"),
        },
        _ => format!("error: {command}: expected load_slot <index>"),
    });
}
