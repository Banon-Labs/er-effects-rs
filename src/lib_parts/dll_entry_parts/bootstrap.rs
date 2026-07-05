
#[allow(unused_imports)]
use crate::{
    config::*, constants::*, crashlog::*, effects::*, experiments::*, ffi::*, hooks::*,
    telemetry::*,
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
            autoload: SaveLoader::new(configured_save_load_request()),
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
pub unsafe extern "C" fn DllMain(hmodule: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason != DLL_PROCESS_ATTACH {
        return DLL_MAIN_SUCCESS;
    }
    write_bootstrap_event(BOOTSTRAP_EVENT_DLL_MAIN_ATTACH, BOOTSTRAP_DETAIL_START);
    init_runtime_config(hmodule);

    // Record our own DLL base (+ SizeOfImage) so the crash logger can annotate a fault whose
    // RIP/return-addresses land in our relocated code as `self+0xRVA` instead of raw Wine
    // addresses the game-base resolver cannot decode. Pure PE-header read, no API/loader lock.
    record_self_dll_base(hmodule.0 as usize);

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

    // SAVE-SOURCE ENFORCEMENT / DEFAULT FALLBACK.
    // Explicit ER_EFFECTS_SAVE_FILE / er-effects.toml save_file sources install the scoped Win32
    // save-path redirect. If no explicit source is supplied, the active Steam user's default
    // %APPDATA%/EldenRing/<SteamID>/ER0000.sl2 is accepted and read normally. If neither exists,
    // enforce_save_override_or_abort shows a clear popup and exits before the title flow drifts into
    // a no-character state.
    let save_override_mode = enforce_save_override_or_abort();
    let missing_save_gate_pending = missing_save_selection_pending();
    match save_override_mode {
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
        SaveOverrideMode::DefaultUserSave => {
            if save_trace_enabled() {
                START_SAVE_REDIRECT.call_once(|| {
                    let _ = std::thread::Builder::new()
                        .name("er-effects-save-trace".to_owned())
                        .spawn(install_save_redirect_hooks);
                });
            }
        }
    }
    if missing_save_gate_pending {
        if let Ok(base) = game_module_base() {
            unsafe { install_title_setstate_trace_hook(base) };
        }
        std::thread::Builder::new()
            .name("er-effects-missing-save-progress-gate".to_owned())
            .spawn(install_show_progress_shortcircuit_hook)
            .ok();
        signal_missing_save_prompt_bootstrap_ready();
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

    // Audio-side startup/title-logo semaphore: log actual Wwise PostEvent IDs because this regression
    // can be heard without a reliable visual artifact. Read-only; forwards the event unchanged.
    START_SOUND_POST_EVENT_OBSERVER.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-sound-post-event".to_owned())
            .spawn(install_sound_post_event_observer_hook);
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

    // Stats-panel native text: arm the 05_010 GFX runtime edit (face box removed + `ErStats` field
    // added; served in-place by the Scaleform file-open observer) and install the row-populate hook
    // + the named-child binder hook (idempotent) so the character's attribute line renders in the
    // game's own MenuFont_01 in its own row field. Independent of the title-cover conditions below
    // -- it must run on every stats-panel product path, so it is gated on `stats_panel_enabled()`
    // directly (product lever; no per-feature env gate).
    if stats_panel_enabled() {
        START_PROFILE_STATS_TEXT.call_once(|| {
            PROFILE_05_010_RUNTIME_EDIT_ARMED.store(1, Ordering::SeqCst);
            let _ = std::thread::Builder::new()
                .name("er-effects-profile-stats-text".to_owned())
                .spawn(|| {
                    // The row-populate hook drives the per-slot attribute push; the named-child binder
                    // hook still runs the title-cover duties. Both are idempotent.
                    install_profile_row_populate_hook();
                    install_title_scene_obj_proxy_named_child_bind_hook();
                });
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
    // NOT on the portrait-lookat path: there the live present-overlay (below) owns the head display and the
    // game's own now-loading ARTWORK stays visible behind it (user choice -- keep the artwork). The forge is
    // only for the pure product-cover path where it IS the display surface. The swappable
    // build_loading_bg_replacement_tpf lever is retained for when we deliberately want to replace the
    // background texture on the head path; it is not wired in by default so the stock artwork renders.
    if !portrait_lookat_enabled() {
        START_LOADING_BG_REPLACE_BIND.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-loading-bg-portrait".to_owned())
                .spawn(install_loading_bg_replace_bind_hook);
        });
    }
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

    // Profile-renderer table guard (er-effects-rs-j3r): before the native per-slot thumbnail
    // builder runs, log a degraded 10-slot table, REBUILD a fully-empty one via the engine's own
    // table setup (only the TitleTopDialog ctor ever calls it natively, so nothing repopulates it
    // across our in-world ProfileSelect reopens -- the 3rd open crashed on the empty table), and
    // fail-soft skip the builder if a slot would still null-deref at [entry+0x754].
    START_PROFILE_SELECT_TABLE_DIAG.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-profileselect-table-diag".to_owned())
            .spawn(install_profile_select_table_diag_hook);
    });

    // System -> Quit Game buttons: always-on multi-slot layout patch plus cloned rows for native
    // 05_010_ProfileSelect and opening the env-provided save folder. Slot activation from that
    // injected in-world route is separately guarded by the System-Quit load flow.
    START_SYSTEM_QUIT_DUPLICATE_BUTTON_HOOK.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-system-quit-load".to_owned())
            .spawn(install_system_quit_duplicate_button_hook);
    });

    // Title Continue confirm guard (0x140b0e180): while a System->Quit->Load-Profile switch is
    // active, drive ONE fresh feed-deserialize of the PICKED slot before the confirm streams, so
    // the clean-title reload loads the picked character instead of re-streaming the stale
    // pre-switch state (bd system-quit-cleantitle-load-is-stale-restream-not-slot-source-2026-07-02).
    // Installed unconditionally (single MinHook per address -- this detour also carries the
    // continue-trace CAP logging); pure passthrough outside an active switch.
    START_SYSTEM_QUIT_CONTINUE_CONFIRM_HOOK.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-system-quit-continue-confirm".to_owned())
            .spawn(install_system_quit_continue_confirm_hook);
    });

    // READ-ONLY teardown-requester trace: EzChildStepBase::RequestFinish. Identifies WHO requests
    // the in-world MoveMapStep child's finish -- the post-switch reload bounce is a stale finish
    // request hitting the freshly-created map session (er-effects-rs-qwj investigation).
    START_SYSTEM_QUIT_CHILD_FINISH_TRACE_HOOK.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-system-quit-child-finish-trace".to_owned())
            .spawn(install_system_quit_child_finish_trace_hook);
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
