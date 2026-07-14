
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
    selected_effect_index: Option<usize>,
    catalogs: Vec<EffectCatalog>,
    selected_catalog_index: Option<usize>,
    effect_hotkeys_effects_on: bool,
    effect_selector_overlay_visible: bool,
    effect_trigger_hotkeys: Vec<EffectTriggerHotkey>,
    effect_trigger_hotkeys_modified: Option<std::time::SystemTime>,
    effect_trigger_hotkeys_load_error: Option<String>,
    pending_effect_triggers: Vec<EffectTriggerHotkey>,
    effect_trigger_fire_count: u64,
    effect_trigger_last_key: Option<String>,
    effect_trigger_last_id: Option<i32>,
    effect_trigger_last_count: u32,
    effect_setting_last_id: Option<i32>,
    effect_setting_last_modified: Option<std::time::SystemTime>,
    effect_setting_live_updates: u64,
    effect_reapply_count: u64,
    effect_reapply_last_index: Option<usize>,
    last_telemetry_write: Option<Instant>,
    last_driver_command: Option<String>,
    autoload: SaveLoader,
    game_task_ticks: u64,
    safe_input: SafeInputRuntime,
}

impl Default for EffectsState {
    fn default() -> Self {
        let (mut calls, catalogs, load_error) = build_effect_catalog_state();
        let (effect_trigger_hotkeys, effect_trigger_hotkeys_load_error) = match load_effect_trigger_hotkeys() {
            Ok(hotkeys) => (hotkeys, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        let selected_effect_id = restore_selected_effect_id();
        let selected_effect_index = selected_effect_id.and_then(|id| {
            calls.iter().position(|call| match call.kind {
                EffectCallKind::SpEffect { id: call_id } => call_id == id,
            })
        });
        let restored_catalog_key = restore_selected_catalog_key();
        let selected_catalog_index = restored_catalog_key
            .as_deref()
            .and_then(|key| catalogs.iter().position(|catalog| catalog.source_key == key))
            .or_else(|| {
                selected_effect_index.and_then(|selected| {
                    catalogs.iter().position(|catalog| {
                        catalog
                            .call_indices
                            .iter()
                            .any(|call_index| *call_index == selected)
                    })
                })
            })
            .or_else(|| (!catalogs.is_empty()).then_some(0));
        let effect_hotkeys_effects_on = restore_effects_enabled() && selected_effect_index.is_some();
        if effect_hotkeys_effects_on
            && let Some(index) = selected_effect_index
            && let Some(call) = calls.get_mut(index)
        {
            call.enabled = true;
            call.remove_requested = false;
            call.active_seen_since_enable = false;
            call.apply_failed = false;
        }

        Self {
            calls,
            catalogs,
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
            selected_effect_index,
            selected_catalog_index,
            effect_hotkeys_effects_on,
            effect_selector_overlay_visible: false,
            effect_trigger_hotkeys,
            effect_trigger_hotkeys_modified: current_effect_trigger_hotkeys_modified(),
            effect_trigger_hotkeys_load_error,
            pending_effect_triggers: Vec::new(),
            effect_trigger_fire_count: 0,
            effect_trigger_last_key: None,
            effect_trigger_last_id: None,
            effect_trigger_last_count: 0,
            effect_setting_last_id: selected_effect_id,
            effect_setting_last_modified: current_effect_setting_modified(),
            effect_setting_live_updates: 0,
            effect_reapply_count: 0,
            effect_reapply_last_index: None,
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

    ensure_effect_hotkey_hook();

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
