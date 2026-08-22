#[allow(unused_imports)]
use crate::{config::*, constants::*, crashlog::*, experiments::*, ffi::*, hooks::*, telemetry::*};

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
    current_animation_id: Option<i32>,
    /// Latched when the expected appear animation is observed either as current or as a queue write
    /// between task ticks; runtime proof needs the semantic event, not a one-frame sampling race.
    expected_animation_seen: bool,
    #[allow(dead_code)] // Retained: Written by the appear-animation latch; nothing reads it back yet.
    applied_for_current_appear: bool,
    /// TimeAct queue write index at the previous tick; used to detect appear
    /// animations that were enqueued (and possibly finished) between ticks.
    last_write_idx: Option<u32>,
    last_telemetry_write: Option<Instant>,
    last_driver_command: Option<String>,
    autoload: SaveLoader,
    game_task_ticks: u64,
    safe_input: SafeInputRuntime,
}

impl Default for EffectsState {
    fn default() -> Self {
        Self {
            current_animation_id: None,
            expected_animation_seen: false,
            applied_for_current_appear: false,
            last_write_idx: None,
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

fn game_main_window_handle_usize() -> usize {
    game_main_window().0 as usize
}

unsafe fn system_dialog_from_action_obj_usize(action_obj: usize) -> usize {
    unsafe { safe_read_usize(action_obj + SYSTEM_QUIT_ACTION_OBJECT_DIALOG_08_OFFSET) }.unwrap_or(0)
}

fn save_dest_start_dir_for_quit_menu() -> Option<er_quit_menu::SaveDestOrigin> {
    let origin = save_dest_start_dir()?;
    Some(er_quit_menu::SaveDestOrigin {
        start_dir: origin.start_dir,
        loaded_file_name: origin.loaded_file_name,
        loaded_path: origin.loaded_path,
    })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// This is called by Windows when the DLL is loaded. Do not call it directly.
pub unsafe extern "C" fn DllMain(hmodule: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason != DLL_PROCESS_ATTACH {
        return DLL_MAIN_SUCCESS;
    }
    // Route the shared er-hook union/registry-collision logging to this DLL's autoload debug log --
    // the exact sink the union used before it moved into the er-hook crate. Installed here, before any
    // hook is registered, so no union-chain or collision line is ever missed.
    er_hook::set_hook_logger(crate::telemetry::append_autoload_debug);
    // Portrait crate split: wire the er-loading-portrait seam to the real product fns
    // BEFORE any hook install or task spawn can execute moved code (the crate's neutral
    // defaults would otherwise gate the whole pipeline off). Pure fn-pointer writes.
    er_loading_portrait::install_host(er_loading_portrait::PortraitHost {
        append_autoload_debug: crate::telemetry::append_autoload_debug,
        note_ls_portrait_capture: crate::telemetry::note_ls_portrait_capture,
        game_directory_path: crate::telemetry::game_directory_path,
        portrait_overlay_enabled: crate::experiments::portrait_overlay_enabled,
        portrait_render_drive_enabled: crate::experiments::portrait_render_drive_enabled,
        portrait_real_pixels_enabled: crate::experiments::portrait_real_pixels_enabled,
        system_quit_repro_enabled: crate::experiments::system_quit_repro_enabled,
        renderdoc_active: crate::experiments::renderdoc_active,
        portrait_loaded_slot: crate::experiments::portrait_loaded_slot,
        portrait_loaded_slot_confirmed: crate::experiments::portrait_loaded_slot_confirmed,
        portrait_target_slot: crate::experiments::portrait_target_slot,
        portrait_tear_score: crate::experiments::portrait_tear_score,
        count_live_profile_models: crate::experiments::count_live_profile_models,
        now_loading_active: crate::experiments::now_loading_active,
        portrait_pipeline_idle_in_gameplay: crate::experiments::portrait_pipeline_idle_in_gameplay,
        save_picker_overlay_active: crate::experiments::save_picker_overlay_active,
        boot_view_epoch_ms: crate::experiments::boot_view_epoch_ms,
        best_active_slot: crate::experiments::best_active_slot,
        ensure_profile_slot_stats_cached: crate::experiments::ensure_profile_slot_stats_cached,
        profile_slot_attributes: crate::experiments::profile_slot_attributes,
        profile_slot_vitals: crate::experiments::profile_slot_vitals,
        profile_slot_weapon_level: crate::experiments::profile_slot_weapon_level,
        game_data_man_ptr_or_null: crate::constants::game_data_man_ptr_or_null,
        boot_view_render_frame: crate::experiments::boot_view_render_frame,
    });
    // Save-picker crate split: wire product (A)'s seam before any hook install or task spawn can
    // execute moved picker code. Pure fn-pointer writes; linking the crate still arms nothing by
    // itself.
    er_save_picker::install_host(er_save_picker::SavePickerHost {
        append_autoload_debug: crate::telemetry::append_autoload_debug,
        missing_save_selection_pending: crate::experiments::missing_save_selection_pending,
        complete_missing_save_selection_from_picker:
            crate::experiments::complete_missing_save_selection_from_picker,
        save_picker_seamless_mode_after_settle:
            crate::experiments::save_picker_seamless_mode_after_settle,
        picker_start_dir: crate::experiments::save_picker_title_start_dir,
        remember_picker_dir: crate::config::remember_preferred_save_picker_dir,
        game_main_window: game_main_window_handle_usize,
        save_file_core_hooks_live: crate::experiments::save_file_core_hooks_live,
        windows_path_for_log: crate::experiments::system_quit_windows_path_for_log,
        save_dest_commit_window_armed: crate::experiments::save_dest_commit_window_armed,
    });
    // Quit-menu crate split: wire product (B)'s seam before its moved hook code can run.
    // This slice moves the OS-dialog dim overlay; the rest of the seam stays on neutral
    // defaults until the corresponding hooked surfaces move.
    er_quit_menu::install_host(er_quit_menu::QuitMenuHost {
        append_autoload_debug: crate::telemetry::append_autoload_debug,
        append_crash_log: crate::telemetry::append_crash_log,
        game_main_window: game_main_window_handle_usize,
        os_native_picker_active: crate::experiments::os_native_picker_active,
        windows_path_for_log: crate::experiments::system_quit_windows_path_for_log,
        system_dialog_from_action_obj: system_dialog_from_action_obj_usize,
        system_quit_save_swap_restore_profile_summary:
            crate::experiments::system_quit_save_swap_restore_profile_summary,
        system_quit_save_swap_arm_original:
            crate::experiments::system_quit_save_swap_arm_original,
        save_picker_start_dir: crate::experiments::save_picker_start_dir,
        system_quit_ingest_picked_save: crate::experiments::system_quit_ingest_picked_save,
        save_dest_start_dir: save_dest_start_dir_for_quit_menu,
        save_dest_set_target: crate::experiments::save_dest_set_target,
        save_flow_box_recipe_available: crate::experiments::save_flow_box_recipe_available,
        save_flow_box_clear: crate::experiments::save_flow_box_clear,
        ..er_quit_menu::QuitMenuHost::defaults()
    });
    // Title-flow crate split: wire the er-title-flow seam to the real product fns, same
    // rules as the portrait seam above (installed before any hook install or task spawn
    // can execute moved code; pure fn-pointer writes).
    er_title_flow::install_host(er_title_flow::TitleFlowHost {
        append_autoload_debug: crate::telemetry::append_autoload_debug,
        append_crash_log: crate::telemetry::append_crash_log,
        timeline_event: crate::telemetry::timeline_event,
        game_directory_path: crate::telemetry::game_directory_path,
        game_data_man_ptr_or_null: crate::constants::game_data_man_ptr_or_null,
        game_man_ptr_or_null: crate::constants::game_man_ptr_or_null,
        runtime_heap_allocator_ptr_or_null: crate::constants::runtime_heap_allocator_ptr_or_null,
        ingamestep_request_code_name: crate::constants::ingamestep_request_code_name,
        movemapstep_step_name: crate::constants::movemapstep_step_name,
        title_step_state_name: crate::constants::title_step_state_name,
        autoload_disabled: crate::experiments::autoload_disabled,
        cleanup_title_dialog_after_world_enabled:
            crate::experiments::cleanup_title_dialog_after_world_enabled,
        experimental_direct_menu_load_enabled:
            crate::experiments::experimental_direct_menu_load_enabled,
        fire_tfc_continue_enabled: crate::experiments::fire_tfc_continue_enabled,
        force_profile_render_enabled: crate::experiments::force_profile_render_enabled,
        native_profile_capture_enabled: crate::experiments::native_profile_capture_enabled,
        product_autoload_enabled: crate::experiments::product_autoload_enabled,
        profile_select_load_flow_enabled: crate::experiments::profile_select_load_flow_enabled,
        title_accept_byte_gate_enabled: crate::experiments::title_accept_byte_gate_enabled,
        title_proceed_gate_enabled: crate::experiments::title_proceed_gate_enabled,
        pab_advance_enabled: crate::experiments::pab_advance_enabled,
        outgoing_teardown_enabled: crate::experiments::outgoing_teardown_enabled,
        outgoing_teardown_suppresses_holds:
            crate::experiments::outgoing_teardown_suppresses_holds,
        switch_reload_ownload_disabled: crate::experiments::switch_reload_ownload_disabled,
        title_anim_speedup_factor: crate::experiments::title_anim_speedup_factor,
        direct_save_file_source_active: crate::experiments::direct_save_file_source_active,
        missing_save_selection_pending: crate::experiments::missing_save_selection_pending,
        save_override_telemetry_only: crate::experiments::save_override_telemetry_only,
        create_continue_trace_hook: crate::experiments::create_continue_trace_hook,
        install_auto_accept_hook: crate::experiments::install_auto_accept_hook,
        decode_thunk_hop: crate::experiments::decode_thunk_hop,
        scan_dialog_for_loadgame: crate::experiments::scan_dialog_for_loadgame,
        resolve_menu_system_save_load: crate::experiments::resolve_menu_system_save_load,
        fire_product_title_load_action: crate::experiments::fire_product_title_load_action,
        product_continue_action_ready: crate::experiments::product_continue_action_ready,
        product_continue_autoload_tick: crate::experiments::product_continue_autoload_tick,
        native_fullread_tick: crate::experiments::native_fullread_tick,
        resolve_active_load_slot: crate::experiments::resolve_active_load_slot,
        mark_tfc_forced_continue_handoff: crate::experiments::mark_tfc_forced_continue_handoff,
        own_stepper_enter_s2_phase: crate::experiments::own_stepper_enter_s2_phase,
        own_stepper_stage2: crate::experiments::own_stepper_stage2,
        own_load_switch_reload_fire: crate::experiments::own_load_switch_reload_fire,
        reset_switch_reload_latches: crate::experiments::reset_switch_reload_latches,
        blockres_stalecap_fix_enabled: crate::experiments::blockres_stalecap_fix_enabled,
        map_mount_guard_flip_tick: crate::experiments::map_mount_guard_flip_tick,
        run_ebl_mount_census: crate::experiments::run_ebl_mount_census,
        fake_loading_screen_visible: crate::experiments::fake_loading_screen_visible,
        now_loading_active: crate::experiments::now_loading_active,
        force_profile_render_tick: crate::experiments::force_profile_render_tick,
        system_quit_save_swap_recommit_after_return_title_save:
            crate::experiments::system_quit_save_swap_recommit_after_return_title_save,
        portrait_retarget_and_rearm_for_switch:
            crate::experiments::portrait_retarget_and_rearm_for_switch,
        title_update_detour: crate::experiments::title_update_detour,
        pab_node_update_detour: crate::experiments::pab_node_update_detour,
    });
    er_d3d12_compositor::set_frame_provider(boot_view_d3d12_compositor_frame);
    write_bootstrap_event(BOOTSTRAP_EVENT_DLL_MAIN_ATTACH, BOOTSTRAP_DETAIL_START);
    init_runtime_config(hmodule);

    // Record our own DLL base (+ SizeOfImage) so the crash logger can annotate a fault whose
    // RIP/return-addresses land in our relocated code as `self+0xRVA` instead of raw Wine
    // addresses the game-base resolver cannot decode. Pure PE-header read, no API/loader lock.
    record_self_dll_base(hmodule.0 as usize);

    // SAVE-DISABLE SUPPRESSION (save-game-flow WP1): swallow native SL save enqueues and
    // answer the status poll with success, so the System->Quit "Save Game" row's scoped
    // one-shot bypass is the ONLY path that really writes while suppression is armed. This
    // is dangerous as a partial slice: without the WP2/WP3 arming+commit path, unconditional
    // install would silently suppress every native save. Keep the product hook default-off
    // behind er-effects.toml `save_suppression_enabled = true`; when unset, the save-flow
    // tick already runs degraded fail-open and fires native saves without a bypass token.
    // Spawned on its own thread (no hook work inside DllMain); when enabled, the attach-time
    // spawn beats the boot-time system-slot save (proven by the standalone
    // er-save-disable-dll's validated run). No product code hooks 0xe6fb50/0xe6e430/0x67a980
    // elsewhere, so there is no ordering constraint; the product only *calls* 0xe6f200 as a
    // finalizer, which is compatible. GraphicsConfig.xml is untouched: suppression sits on
    // the SL container funnel only. NEVER load er_save_disable.dll together with this DLL in
    // one me3 profile -- two MinHook instances would double-detour the same prologues.
    if save_suppression_enabled() {
        START_SAVE_SUPPRESS.call_once(spawn_save_suppress_install);
    } else {
        append_autoload_debug(format_args!(
            "save-suppress: disabled by default config guard (set save_suppression_enabled=true to opt in); native saves are not suppressed"
        ));
        // ATTRIBUTION WITHOUT SUPPRESSION (2026-08-04). The save-lane observers are read-only --
        // each calls its original and only counts -- but they used to be reachable only through
        // `install`, which ARMS suppression. So in the default configuration, which is the one every
        // user runs, `oracle_save_dispatch_last_decline_reason` read `unsampled`: the single field
        // that names why the lane refused a save was unavailable precisely where the refusal
        // matters. `GameMan+0xb72`/`+0xb73` latching after a reload is the `FUN_140afa6d0` case-7
        // blocker, and without this the only way to explain it is to guess.
        START_SAVE_OBSERVERS.call_once(spawn_save_observers_only);
    }

    // SAVE-DESTINATION WRITE-OPEN REDIRECT CORE (save-game-flow WP3): the "save somewhere else"
    // path diverts the native writer's single container write-open, which means the CreateFileW
    // detour must exist in EVERY save mode -- including the default game-owned APPDATA mode, where
    // `install_save_redirect_hooks` deliberately installs nothing. The detour body is
    // pass-through-safe without a redirect dir (it only observes), and the Wine-only free-space
    // overrides stay behind their own gate. Own thread: no hook work inside DllMain.
    START_SAVE_FILE_OPS_CORE.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-save-file-core".to_owned())
            .spawn(install_save_file_core_hooks);
    });

    // Boot profiler: spawn the independent CPU sampler FIRST so it captures the engine-init threads
    // during the pre-CSTaskImp-instance gap (the largest uninstrumented boot window). Read-only by
    // default (QueryThreadCycleTime/GetThreadTimes, no thread suspension); RIP sampling is a separate
    // opt-in sub-switch. Gated OFF unless ER_EFFECTS_PROFILE=1 / er-effects-profile.txt.
    if er_boot_profiler::profiler_enabled() {
        START_BOOT_PROFILER
            .call_once(|| er_boot_profiler::spawn_boot_profiler(append_autoload_debug));
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
        // The in-game missing-save picker (save_picker_menu.rs) rides the native no-save title:
        // the SetState detour denies only the world-entry states (4/5) while the selection is
        // pending, and the show-progress hook lets the save-data job complete naturally so the
        // title menu becomes interactive. Both hooks also serve normal boots; install them here
        // so the earliest title states are already covered.
        if let Ok(base) = game_module_base() {
            unsafe { install_title_setstate_trace_hook(base) };
        }
        std::thread::Builder::new()
            .name("er-effects-missing-save-progress-gate".to_owned())
            .spawn(install_show_progress_shortcircuit_hook)
            .ok();
    }

    let initial_state = EffectsState::default();
    arm_product_autoload_from_request(&initial_state.autoload);
    let state = Arc::new(Mutex::new(initial_state));
    // Publish the handle so a non-game-task thread can flush telemetry before it ends the process.
    // Without this the telemetry file can only ever describe events the game task lived to see, and
    // the boot picker's cancel path deliberately outlives it.
    publish_effects_state(&state);

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

    // Foreground-force REMOVED (user directive 2026-07-16): the product feature must NOT force the
    // game's foreground state. Patching CS::CSWindowImp::IsGameInForeground to always-true made the
    // game behave as if focused, which captures/confines the OS cursor onto the window when a load
    // completes and the world comes up -- unwanted product behavior. The old 2026-06-21 "keep it on"
    // directive (full-speed unfocused probes) is superseded; the game now uses its real focus state.

    // Audio-side startup/title-logo semaphore: log actual Wwise PostEvent IDs because this regression
    // can be heard without a reliable visual artifact. Read-only; forwards the event unchanged.
    START_SOUND_POST_EVENT_OBSERVER.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-sound-post-event".to_owned())
            .spawn(install_sound_post_event_observer_hook);
    });

    install_title_visual_startup_hooks();
    install_profile_and_system_quit_hooks();
    install_boot_diagnostics_and_trace_hooks();
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

/// Wire the er-save-suppress seams to the product sinks and install the suppression hooks
/// off the loader lock. Log sink -> the autoload debug log; publish sink -> deliberate
/// no-op, because the product's periodic telemetry writer already exports the suppress and
/// bypass counters as `oracle_save_*` fields on its own 250 ms cadence (a second snapshot
/// writer would just double the file traffic).
fn spawn_save_suppress_install() {
    let _ = std::thread::Builder::new()
        .name("er-effects-save-suppress".to_owned())
        .spawn(|| {
            er_save_suppress::set_log_sink(crate::telemetry::append_autoload_debug);
            er_save_suppress::set_publish_sink(|| {});
            // Spin until the game module resolves (same loop shape as the standalone
            // er-save-disable-dll installer): MinHook and the prologue verification need
            // the image mapped before install can bind anything.
            let mut attempts = 0_u64;
            loop {
                match er_game_base::mem::game_module_base() {
                    Ok(_) => break,
                    Err(err) => {
                        if attempts == 0 || attempts.is_multiple_of(SAVE_SUPPRESS_WAIT_LOG_INTERVAL) {
                            crate::telemetry::append_autoload_debug(format_args!(
                                "save-suppress: waiting for game module base: {err}"
                            ));
                        }
                        attempts = attempts.saturating_add(1);
                        std::thread::yield_now();
                    }
                }
            }
            // The product opt-in is the er-effects.toml guard above, not an env var or a
            // standalone-DLL positive-control lever. Once opted in, `false` = never disarm.
            let installed = er_save_suppress::install(false);
            crate::telemetry::append_autoload_debug(format_args!(
                "save-suppress: install done hooks={installed}/{} armed={} -- all native \
                 saves suppressed; the Save Game row's one-shot bypass is the only real writer",
                er_save_suppress::SUPPRESSOR_HOOKS,
                er_save_suppress::is_armed()
            ));
        });
}

/// Bind the save-lane observers WITHOUT arming suppression.
///
/// Same thread shape and same module-base wait as [`spawn_save_suppress_install`] -- MinHook and the
/// prologue verification both need the image mapped -- but it installs only the read-only observer
/// batch. No suppressor is bound, `ARMED` is never set, and every native save proceeds exactly as it
/// does today. The single thing that changes is that a refused save now says why.
fn spawn_save_observers_only() {
    let _ = std::thread::Builder::new()
        .name("er-effects-save-observers".to_owned())
        .spawn(|| {
            er_save_suppress::set_log_sink(crate::telemetry::append_autoload_debug);
            let mut attempts = 0_u64;
            loop {
                match er_game_base::mem::game_module_base() {
                    Ok(_) => break,
                    Err(err) => {
                        if attempts == 0 || attempts.is_multiple_of(SAVE_SUPPRESS_WAIT_LOG_INTERVAL) {
                            crate::telemetry::append_autoload_debug(format_args!(
                                "save-observers: waiting for game module base: {err}"
                            ));
                        }
                        attempts = attempts.saturating_add(1);
                        std::thread::yield_now();
                    }
                }
            }
            let bound = er_save_suppress::install_observers_only();
            crate::telemetry::append_autoload_debug(format_args!(
                "save-observers: bound {bound}/4 save-lane attribution observers with suppression \
                 DISARMED -- native saves are untouched; oracle_save_dispatch_last_decline_reason \
                 now names a refusal instead of reading 'unsampled'"
            ));
        });
}

/// Throttle for the (in practice never-taken) module-base wait log in
/// `spawn_save_suppress_install`.
const SAVE_SUPPRESS_WAIT_LOG_INTERVAL: u64 = 4096;

pub(crate) fn wait_for_task_instance() -> &'static CSTaskImp {
    let mut wait_attempts = 0_u64;
    loop {
        match unsafe { CSTaskImp::instance() } {
            Ok(instance) => return instance,
            Err(InstanceError::NotFound(_)) | Err(InstanceError::Null(_)) => {
                wait_attempts = wait_attempts.saturating_add(1);
                if wait_attempts == 1 || wait_attempts.is_multiple_of(TASK_INSTANCE_WAIT_LOG_INTERVAL) {
                    let detail = format!("attempts={wait_attempts}");
                    write_bootstrap_event(BOOTSTRAP_EVENT_GAME_TASK_WAITING_INSTANCE, &detail);
                }
                std::thread::yield_now()
            }
        }
    }
}
