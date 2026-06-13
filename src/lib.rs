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

use debug::InputBlocker;
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use hudhook::{
    ImguiRenderLoop, MessageFilter,
    hooks::dx12::ImguiDx12Hooks,
    imgui::{Condition, Context, Ui},
    mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook},
    windows::{
        Win32::{
            Foundation::HINSTANCE,
            System::{LibraryLoader::GetModuleHandleA, SystemServices::DLL_PROCESS_ATTACH},
        },
        core::PCSTR,
    },
};

#[cfg(windows)]
unsafe extern "system" {
    fn RtlCaptureStackBackTrace(
        frames_to_skip: u32,
        frames_to_capture: u32,
        backtrace: *mut *mut c_void,
        backtrace_hash: *mut u32,
    ) -> u16;
}

const DLL_MAIN_SUCCESS: i32 = 1;
const APPEAR_ANIMATION_ID: i32 = 63010;
const OVERLAY_INITIAL_POSITION: [f32; 2] = [24.0, 24.0];
const OVERLAY_INITIAL_SIZE: [f32; 2] = [420.0, 420.0];
/// TimeAct animation IDs at or below this value mark unused/cleared queue
/// slots rather than a real animation.
const INVALID_ANIMATION_ID_FLOOR: i32 = 0;
const ANIM_QUEUE_SLOT_STEP: u32 = 1;
const ANIM_QUEUE_SCAN_FLOOR: u32 = 0;
const CUSTOM_CALL_DEFAULT_ID: i32 = 0;
const NEXT_INDEX_OFFSET: usize = 1;
const TITLE_BOOTSTRAP_UNSEEN: usize = 0;
const TITLE_BOOTSTRAP_SEEN_VALUE: usize = 1;
const STACK_TRACE_FRAME_COUNT: usize = 8;
const STACK_TRACE_FRAMES_TO_SKIP: u32 = 0;
const NULL_MODULE_BASE: usize = 0;
const HOOK_ORIGINAL_UNSET: usize = 0;
const HOOK_FALSE_RETURN: u8 = 0;
const BOOTSTRAP_TELEMETRY_UNSEEN: usize = 0;
const BOOTSTRAP_TELEMETRY_SEEN_VALUE: usize = 1;
const BOOTSTRAP_EVENT_DLL_MAIN_ATTACH: &str = "dllmain_attach";
const BOOTSTRAP_EVENT_CONTINUE_TRACE_REQUESTED: &str = "continue_trace_thread_requested";
const BOOTSTRAP_EVENT_GAME_TASK_REQUESTED: &str = "game_task_thread_requested";
const BOOTSTRAP_EVENT_OVERLAY_SKIPPED_AUTOLOAD: &str = "overlay_skipped_autoload_only";
const BOOTSTRAP_EVENT_GAME_TASK_THREAD_STARTED: &str = "game_task_thread_started";
const BOOTSTRAP_EVENT_GAME_TASK_INSTANCE_READY: &str = "game_task_instance_ready";
const BOOTSTRAP_EVENT_GAME_TASK_RECURRING_REGISTERED: &str = "game_task_recurring_registered";
const BOOTSTRAP_EVENT_TELEMETRY_WRITE: &str = "telemetry_write";
const BOOTSTRAP_EVENT_CONTINUE_TRACE_STARTED: &str = "continue_trace_started";
const BOOTSTRAP_EVENT_CONTINUE_TRACE_APPLIED: &str = "continue_trace_applied";
const BOOTSTRAP_EVENT_CONTINUE_TRACE_APPLY_FAILED: &str = "continue_trace_apply_failed";
const BOOTSTRAP_DETAIL_START: &str = "start";
const BOOTSTRAP_DETAIL_DONE: &str = "done";
const BOOTSTRAP_DETAIL_PLAYER_AVAILABLE: &str = "player_available";
const BOOTSTRAP_DETAIL_PLAYER_UNAVAILABLE: &str = "player_unavailable";
static START_GAME_TASK: Once = Once::new();
static START_CONTINUE_TRACE: Once = Once::new();
static BOOTSTRAP_TELEMETRY_SEEN: AtomicUsize = AtomicUsize::new(BOOTSTRAP_TELEMETRY_UNSEEN);

static MENU_CONTINUE_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(0);
static MENU_NEW_OR_LOAD_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(0);
static MENU_OTHER_LOAD_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(0);
static SET_SAVE_SLOT_ORIG: AtomicUsize = AtomicUsize::new(0);
static SAVE_REQUEST_PROFILE_ORIG: AtomicUsize = AtomicUsize::new(0);
static REQUEST_SAVE_ORIG: AtomicUsize = AtomicUsize::new(0);
static CURRENT_SLOT_LOAD_ORIG: AtomicUsize = AtomicUsize::new(0);
static CONTINUE_LOAD_ORIG: AtomicUsize = AtomicUsize::new(0);
static COMBINED_LOAD_ORIG: AtomicUsize = AtomicUsize::new(0);
static MAP_LOAD_ORIG: AtomicUsize = AtomicUsize::new(0);
static SAVE_LOAD_STATE_INIT_ORIG: AtomicUsize = AtomicUsize::new(0);
static TITLE_BOOTSTRAP_SEEN: AtomicUsize = AtomicUsize::new(0);

/// A named runtime effect call the overlay can trigger.
///
/// Adding a new call kind (e.g. an SFX/FXR spawn once `fromsoftware-rs`
/// exposes a wrapper for it) takes three mechanical steps:
/// 1. add a variant to `er_effects_data::EffectKindSpec` (the
///    `data/effects.json` schema),
/// 2. add the matching variant here plus arms in `label`/`apply`/`remove`/
///    `is_active`,
/// 3. map it in `call_kind_from_spec`.
/// The overlay and the game task dispatch exclusively through those four
/// methods, so nothing else needs to change.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EffectCallKind {
    SpEffect { id: i32 },
}

impl EffectCallKind {
    fn label(self) -> String {
        match self {
            Self::SpEffect { id } => format!("SpEffect {id}"),
        }
    }

    fn apply(self, player: &mut PlayerIns, network_sync: bool) {
        match self {
            Self::SpEffect { id } => {
                let dont_sync = !network_sync;
                player.apply_speffect(id, dont_sync);
            }
        }
    }

    fn remove(self, player: &mut PlayerIns) {
        match self {
            Self::SpEffect { id } => player.chr_ins.remove_speffect(id),
        }
    }

    /// Whether the call is currently in force on the player. The game's
    /// apply/remove calls return nothing, so the active-SpEffect list is the
    /// ground truth for surfacing success and failure in the overlay.
    fn is_active(self, player: &PlayerIns) -> bool {
        match self {
            Self::SpEffect { id } => player
                .chr_ins
                .special_effect
                .entries()
                .any(|entry| entry.param_id == id),
        }
    }
}

struct NamedEffectCall {
    name: String,
    kind: EffectCallKind,
    enabled: bool,
    remove_requested: bool,
    /// Live status, recomputed every game tick from the player's SpEffect
    /// list.
    active: bool,
    /// Set when an apply attempt did not take (e.g. the ID has no
    /// `SpEffectParam` row); cleared as soon as the effect shows up active.
    apply_failed: bool,
}

impl NamedEffectCall {
    fn new(name: String, kind: EffectCallKind, enabled: bool) -> Self {
        Self {
            name,
            kind,
            enabled,
            remove_requested: false,
            active: false,
            apply_failed: false,
        }
    }
}

fn call_kind_from_spec(kind: EffectKindSpec, id: i32) -> EffectCallKind {
    match kind {
        EffectKindSpec::SpEffect => EffectCallKind::SpEffect { id },
    }
}

fn named_call_from_spec(spec: EffectCallSpec) -> NamedEffectCall {
    let kind = call_kind_from_spec(spec.kind, spec.id);
    NamedEffectCall::new(spec.name, kind, spec.enabled)
}

fn bootstrap_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_BOOTSTRAP_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("er-effects-bootstrap.jsonl"))
}

fn bootstrap_state_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_BOOTSTRAP_STATE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("er-effects-bootstrap-state.json"))
}

fn write_bootstrap_event(stage: &str, detail: &str) {
    use std::io::Write;

    let event_path = bootstrap_path();
    let state_path = bootstrap_state_path();
    if let Some(parent) = event_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Some(parent) = state_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let payload = format!(
        "{{\"stage\":\"{}\",\"detail\":\"{}\"}}\n",
        json_escape(stage),
        json_escape(detail)
    );
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&event_path)
    {
        let _ = file.write_all(payload.as_bytes());
    }
    let _ = fs::write(state_path, payload);
}

struct EffectsState {
    calls: Vec<NamedEffectCall>,
    /// Parse error for the embedded `data/effects.json`, shown in the overlay
    /// instead of silently starting with an empty list.
    load_error: Option<String>,
    current_animation_id: Option<i32>,
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
            applied_for_current_appear: false,
            last_write_idx: None,
            manual_apply_requested: false,
            remove_all_requested: false,
            network_sync: false,
            custom_call_id: CUSTOM_CALL_DEFAULT_ID,
            last_telemetry_write: None,
            last_driver_command: None,
            autoload: SaveLoader::from_env(),
        }
    }
}

struct EffectsOverlay {
    state: Arc<Mutex<EffectsState>>,
}

impl ImguiRenderLoop for EffectsOverlay {
    fn initialize(&mut self, ctx: &mut Context, _render_context: &mut dyn hudhook::RenderContext) {
        // Elden Ring hides/captures the OS cursor under Proton. Draw ImGui's
        // software cursor so overlay hit-testing is visible during Linux smoke
        // tests and normal in-game use.
        ctx.io_mut().mouse_draw_cursor = true;
    }

    fn render(&mut self, ui: &mut Ui) {
        let blocker = InputBlocker::get_instance();
        unsafe {
            let _ = blocker.install_hooks();
        }
        blocker.block_from_io(ui.io());

        let mut state = state_or_return(&self.state);
        process_global_driver_command(&mut state);
        let player_available = if let Ok(player) = unsafe { PlayerIns::local_player_mut() } {
            process_driver_command(player, &mut state);
            refresh_call_status(player, &mut state);
            true
        } else {
            process_autoload_request(&mut state);
            false
        };
        write_telemetry_throttled(&mut state, player_available);

        ui.window("ER Effects")
            .position(OVERLAY_INITIAL_POSITION, Condition::FirstUseEver)
            .size(OVERLAY_INITIAL_SIZE, Condition::FirstUseEver)
            .build(|| {
                if let Some(error) = &state.load_error {
                    ui.text_wrapped(format!("effects.json error: {error}"));
                    ui.separator();
                }

                ui.text(format!(
                    "Current animation: {}",
                    state
                        .current_animation_id
                        .map_or_else(|| "unknown".to_owned(), |id| id.to_string())
                ));
                ui.text(format!("Appear trigger animation: {APPEAR_ANIMATION_ID}"));

                ui.separator();
                ui.checkbox(
                    "Sync effect calls over the network",
                    &mut state.network_sync,
                );

                if ui.button("Apply selected now") {
                    if let Ok(player) = unsafe { PlayerIns::local_player_mut() } {
                        apply_selected_calls(player, &mut state);
                        refresh_call_status(player, &mut state);
                    } else {
                        state.manual_apply_requested = true;
                    }
                }
                if ui.button("Remove all listed effects") {
                    if let Ok(player) = unsafe { PlayerIns::local_player_mut() } {
                        for call in &mut state.calls {
                            call.kind.remove(player);
                            call.remove_requested = false;
                        }
                        refresh_call_status(player, &mut state);
                    } else {
                        state.remove_all_requested = true;
                    }
                }

                ui.separator();
                ui.input_int("Custom SpEffect ID", &mut state.custom_call_id)
                    .build();
                if ui.button("Add custom call") {
                    add_custom_call(&mut state);
                }

                ui.separator();
                ui.text("Named calls");

                let network_sync = state.network_sync;
                let mut apply_requested_without_player = false;
                for call in &mut state.calls {
                    let label = format!("{} ({})", call.name, call.kind.label());
                    if ui.checkbox(&label, &mut call.enabled) {
                        if let Ok(player) = unsafe { PlayerIns::local_player_mut() } {
                            if call.enabled {
                                call.kind.apply(player, network_sync);
                                call.active = call.kind.is_active(player);
                                call.apply_failed = !call.active;
                            } else {
                                call.kind.remove(player);
                                call.remove_requested = false;
                                call.apply_failed = false;
                                call.active = call.kind.is_active(player);
                            }
                        } else if call.enabled {
                            apply_requested_without_player = true;
                        } else {
                            call.remove_requested = true;
                        }
                    }
                    ui.same_line();
                    ui.text(call_status_text(call));
                }
                if apply_requested_without_player {
                    state.manual_apply_requested = true;
                }
            });
    }

    fn message_filter(&self, io: &hudhook::imgui::Io) -> MessageFilter {
        if io.want_capture_mouse || io.want_capture_keyboard {
            MessageFilter::InputAll
        } else {
            MessageFilter::empty()
        }
    }
}

fn write_telemetry_throttled(state: &mut EffectsState, player_available: bool) {
    const TELEMETRY_INTERVAL: Duration = Duration::from_millis(250);

    let now = Instant::now();
    if state
        .last_telemetry_write
        .is_some_and(|last_write| now.duration_since(last_write) < TELEMETRY_INTERVAL)
    {
        return;
    }

    state.last_telemetry_write = Some(now);
    write_telemetry(state, player_available);
}

fn write_telemetry(state: &EffectsState, player_available: bool) {
    if BOOTSTRAP_TELEMETRY_SEEN
        .compare_exchange(
            BOOTSTRAP_TELEMETRY_UNSEEN,
            BOOTSTRAP_TELEMETRY_SEEN_VALUE,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
    {
        write_bootstrap_event(
            BOOTSTRAP_EVENT_TELEMETRY_WRITE,
            if player_available {
                BOOTSTRAP_DETAIL_PLAYER_AVAILABLE
            } else {
                BOOTSTRAP_DETAIL_PLAYER_UNAVAILABLE
            },
        );
    }

    let path = telemetry_path();
    let mut body = String::new();
    body.push_str("{\n");
    body.push_str(&format!("  \"player_available\": {player_available},\n"));
    body.push_str(&format!(
        "  \"current_animation_id\": {},\n",
        state
            .current_animation_id
            .map_or_else(|| "null".to_owned(), |id| id.to_string())
    ));
    body.push_str(&format!("  \"network_sync\": {},\n", state.network_sync));
    body.push_str(&format!(
        "  \"autoload_save_extension\": {},\n",
        state.autoload.save_extension().map_or_else(
            || "null".to_owned(),
            |extension| format!("\"{}\"", json_escape(extension))
        )
    ));
    body.push_str(&format!(
        "  \"autoload_slot\": {},\n",
        state
            .autoload
            .slot()
            .map_or_else(|| "null".to_owned(), |slot| slot.to_string())
    ));
    body.push_str(&format!(
        "  \"autoload_method\": \"{}\",\n",
        state.autoload.method().label()
    ));
    body.push_str(&format!(
        "  \"autoload_attempts\": {},\n",
        state.autoload.attempts()
    ));
    body.push_str(&format!(
        "  \"autoload_last_status\": {},\n",
        state.autoload.last_status().map_or_else(
            || "null".to_owned(),
            |status| format!("\"{}\"", json_escape(status))
        )
    ));
    write_game_man_telemetry(&mut body);
    body.push_str(&format!(
        "  \"last_driver_command\": {},\n",
        state.last_driver_command.as_ref().map_or_else(
            || "null".to_owned(),
            |command| format!("\"{}\"", json_escape(command))
        )
    ));
    body.push_str("  \"calls\": [\n");
    for (index, call) in state.calls.iter().enumerate() {
        let comma = if index + NEXT_INDEX_OFFSET == state.calls.len() {
            ""
        } else {
            ","
        };
        body.push_str(&format!(
            "    {{\"index\": {index}, \"name\": \"{}\", \"kind\": \"{}\", \"enabled\": {}, \"active\": {}, \"apply_failed\": {}}}{comma}\n",
            json_escape(&call.name),
            json_escape(&call.kind.label()),
            call.enabled,
            call.active,
            call.apply_failed,
        ));
    }
    body.push_str("  ]\n}\n");

    let tmp_path = path.with_extension("json.tmp");
    if fs::write(&tmp_path, body).is_ok() {
        let _ = fs::rename(tmp_path, path);
    }
}

fn write_game_man_telemetry(body: &mut String) {
    let Ok(game_man) = (unsafe { GameMan::instance() }) else {
        body.push_str("  \"game_man_available\": false,\n");
        return;
    };

    let telemetry = GameManTelemetry::from_game_man(game_man);
    body.push_str("  \"game_man_available\": true,\n");
    body.push_str(&format!("  \"game_save_slot\": {},\n", telemetry.save_slot));
    body.push_str(&format!(
        "  \"game_requested_save_slot_load_index\": {},\n",
        telemetry.requested_save_slot_load_index
    ));
    body.push_str(&format!(
        "  \"game_save_state\": {},\n",
        telemetry.save_state
    ));
    body.push_str(&format!(
        "  \"game_save_requested\": {},\n",
        telemetry.save_requested
    ));
}

fn telemetry_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_TELEMETRY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("er-effects-telemetry.json"))
}

fn command_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_COMMAND_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("er-effects-command.txt"))
}

fn json_escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\t' => "\\t".chars().collect::<Vec<_>>(),
            character => vec![character],
        })
        .collect()
}

fn call_status_text(call: &NamedEffectCall) -> &'static str {
    if call.active {
        "[active]"
    } else if call.apply_failed {
        "[apply failed]"
    } else {
        "[inactive]"
    }
}

fn add_custom_call(state: &mut EffectsState) {
    let id = state.custom_call_id;
    let kind = EffectCallKind::SpEffect { id };
    if state.calls.iter().any(|call| call.kind == kind) {
        return;
    }
    state
        .calls
        .push(NamedEffectCall::new(format!("Custom {id}"), kind, true));
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

    let state = Arc::new(Mutex::new(EffectsState::default()));

    let direct_autoload_configured = {
        let state = state_or_return(&state);
        state.autoload.method() == SaveLoadMethod::DirectMenuLoad && state.autoload.slot().is_some()
    };
    if trace_continue_enabled() || direct_autoload_configured {
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

    let autoload_without_overlay = state_or_return(&state).autoload.slot().is_some();
    if autoload_without_overlay {
        write_bootstrap_event(
            BOOTSTRAP_EVENT_OVERLAY_SKIPPED_AUTOLOAD,
            BOOTSTRAP_DETAIL_DONE,
        );
        return DLL_MAIN_SUCCESS;
    }

    debug::initialize::<ImguiDx12Hooks>(
        hmodule,
        reason,
        || {
            let _ = wait_for_task_instance();
        },
        EffectsOverlay { state },
    )
}

fn wait_for_task_instance() -> &'static CSTaskImp {
    loop {
        match unsafe { CSTaskImp::instance() } {
            Ok(instance) => return instance,
            Err(InstanceError::NotFound(_)) | Err(InstanceError::Null(_)) => {
                std::thread::yield_now()
            }
        }
    }
}

fn spawn_game_task(state: Arc<Mutex<EffectsState>>) {
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

        cs_task.run_recurring(
            move |_: &FD4TaskData| {
                let Ok(player) = (unsafe { PlayerIns::local_player_mut() }) else {
                    let mut state = state_or_return(&state);
                    process_autoload_request(&mut state);
                    write_telemetry_throttled(&mut state, false);
                    return;
                };

                let mut state = state_or_return(&state);
                let observation = observe_animation(player, state.last_write_idx);
                state.current_animation_id = observation.current_animation_id;
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
    });
}

fn process_autoload_request(state: &mut EffectsState) {
    if state.autoload.completed() || state.autoload.slot().is_none() {
        return;
    }

    let Ok(game_man) = (unsafe { GameMan::instance_mut() }) else {
        return;
    };

    let Ok(game_module_base) = game_module_base() else {
        return;
    };

    let context = SaveLoadContext {
        game_module_base,
        title_bootstrap_seen: TITLE_BOOTSTRAP_SEEN.load(Ordering::SeqCst) != TITLE_BOOTSTRAP_UNSEEN,
    };
    let _ = unsafe {
        state.autoload.process(game_man, context, |message| {
            append_autoload_debug(format_args!("{message}"))
        })
    };
}

fn game_module_base() -> Result<usize, String> {
    let module = unsafe { GetModuleHandleA(PCSTR::null()) }
        .map_err(|error| format!("failed to resolve game module: {error}"))?;
    Ok(module.0 as usize)
}

fn game_rva(rva: u32) -> Result<usize, String> {
    Ok(game_module_base()? + rva as usize)
}

fn append_autoload_debug(args: std::fmt::Arguments<'_>) {
    use std::io::Write;

    let path = std::env::var("ER_EFFECTS_AUTOLOAD_DEBUG_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("er-effects-autoload-debug.log"));
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{args}");
    }
}

fn trace_continue_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TRACE_CONTINUE").as_deref(),
        Ok("1")
    ) || trace_continue_default_path().exists()
        || PathBuf::from("er-effects-trace-continue.txt").exists()
}

fn trace_continue_default_path() -> PathBuf {
    game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-trace-continue.txt")
}

fn continue_trace_log_path() -> PathBuf {
    std::env::var("ER_EFFECTS_TRACE_CONTINUE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            game_directory_path()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("er-effects-continue-trace.log")
        })
}

fn game_directory_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
}

fn append_continue_trace(args: std::fmt::Arguments<'_>) {
    use std::io::Write;

    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(continue_trace_log_path())
    {
        let _ = writeln!(file, "{args}");
    }
}

#[cfg(windows)]
fn trace_callers_summary() -> String {
    let mut frames = [std::ptr::null_mut::<c_void>(); STACK_TRACE_FRAME_COUNT];
    let captured = unsafe {
        RtlCaptureStackBackTrace(
            STACK_TRACE_FRAMES_TO_SKIP,
            frames.len() as u32,
            frames.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    } as usize;
    let module_base = unsafe { GetModuleHandleA(PCSTR::null()) }
        .ok()
        .map(|module| module.0 as usize)
        .unwrap_or(NULL_MODULE_BASE);

    let callers = frames
        .iter()
        .take(captured)
        .enumerate()
        .map(|(index, frame)| {
            let address = *frame as usize;
            if module_base != NULL_MODULE_BASE && address >= module_base {
                format!("#{index}=0x{:x}", address - module_base)
            } else {
                format!("#{index}=0x{address:x}")
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("callers=[{callers}]")
}

#[cfg(not(windows))]
fn trace_callers_summary() -> String {
    "callers=[]".to_owned()
}

fn game_man_trace_summary() -> String {
    const GAME_MAN_GLOBAL_RVA: u32 = 0x03d69918;
    const GAME_MAN_SAVE_SLOT_OFFSET: usize = 0xac0;
    const GAME_MAN_REQUESTED_SAVE_SLOT_LOAD_INDEX_OFFSET: usize = 0xb78;
    const GAME_MAN_SAVE_STATE_OFFSET: usize = 0xb80;
    const GAME_MAN_FLAG_B72_OFFSET: usize = 0xb72;
    const GAME_MAN_FLAG_B73_OFFSET: usize = 0xb73;
    const GAME_MAN_FLAG_B74_OFFSET: usize = 0xb74;
    const GAME_MAN_FLAG_B75_OFFSET: usize = 0xb75;
    const GAME_MAN_FLAG_BB8_OFFSET: usize = 0xbb8;
    const GAME_MAN_FLAG_BC4_OFFSET: usize = 0xbc4;
    const GAME_MAN_FLAG_BBC_OFFSET: usize = 0xbbc;
    const GAME_MAN_FLAG_BC0_OFFSET: usize = 0xbc0;

    unsafe {
        let Ok(global) = game_rva(GAME_MAN_GLOBAL_RVA) else {
            return "gm_global_unresolved".to_owned();
        };
        let game_man = *(global as *const *const u8);
        if game_man.is_null() {
            return "gm=null".to_owned();
        }

        let read_i32 = |offset: usize| *(game_man.add(offset) as *const i32);
        let read_u8 = |offset: usize| *game_man.add(offset);
        format!(
            "gm={game_man:p} slot={} req_idx={} state={} flags{{b72={},b73={},b74={},b75={},bb8={}}} bc4={} bbc={} bc0={}",
            read_i32(GAME_MAN_SAVE_SLOT_OFFSET),
            read_i32(GAME_MAN_REQUESTED_SAVE_SLOT_LOAD_INDEX_OFFSET),
            read_i32(GAME_MAN_SAVE_STATE_OFFSET),
            read_u8(GAME_MAN_FLAG_B72_OFFSET),
            read_u8(GAME_MAN_FLAG_B73_OFFSET),
            read_u8(GAME_MAN_FLAG_B74_OFFSET),
            read_u8(GAME_MAN_FLAG_B75_OFFSET),
            read_u8(GAME_MAN_FLAG_BB8_OFFSET),
            read_i32(GAME_MAN_FLAG_BC4_OFFSET),
            read_i32(GAME_MAN_FLAG_BBC_OFFSET),
            read_i32(GAME_MAN_FLAG_BC0_OFFSET),
        )
    }
}

unsafe fn create_continue_trace_hook(
    hooks: &mut Vec<MhHook>,
    name: &str,
    rva: u32,
    hook_impl: *mut c_void,
    original: &AtomicUsize,
) {
    let Ok(addr) = game_rva(rva) else {
        append_continue_trace(format_args!("hook {name}: failed to resolve rva=0x{rva:x}"));
        return;
    };

    match unsafe { MhHook::new(addr as *mut c_void, hook_impl) } {
        Ok(hook) => {
            original.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_continue_trace(format_args!("hook {name}: queue_enable failed: {status:?}"));
            } else {
                append_continue_trace(format_args!(
                    "hook {name}: target=0x{addr:x} trampoline={:p}",
                    hook.trampoline()
                ));
                hooks.push(hook);
            }
        }
        Err(status) => append_continue_trace(format_args!(
            "hook {name}: create failed at 0x{addr:x}: {status:?}"
        )),
    }
}

fn install_continue_trace_hooks() {
    write_bootstrap_event(
        BOOTSTRAP_EVENT_CONTINUE_TRACE_STARTED,
        BOOTSTRAP_DETAIL_START,
    );
    // Local Proton executable RVAs. The shared Ghidra 1.16.1 function starts are
    // currently +0xf0 for these text symbols; these RVAs are verified against
    // /home/banon/.local/share/Steam/.../eldenring.exe sha256
    // 34102b1c08bb5f769a724427a6f70fe29b3b732c31cf73693f861c48d3492ddb.
    const MENU_CONTINUE_WRAPPER_RVA: u32 = 0x0082bac0;
    const MENU_NEW_OR_LOAD_WRAPPER_RVA: u32 = 0x0082ba80;
    const MENU_OTHER_LOAD_WRAPPER_RVA: u32 = 0x0082bb00;
    const SET_SAVE_SLOT_RVA: u32 = 0x0067a810;
    const SAVE_REQUEST_PROFILE_RVA: u32 = 0x0067a420;
    const REQUEST_SAVE_RVA: u32 = 0x0067a520;
    const CURRENT_SLOT_LOAD_RVA: u32 = 0x0067b570;
    const CONTINUE_LOAD_RVA: u32 = 0x0067b750;
    const COMBINED_LOAD_RVA: u32 = 0x0067b940;
    const MAP_LOAD_RVA: u32 = 0x0067bc10;
    const SAVE_LOAD_STATE_INIT_RVA: u32 = 0x0067b030;

    append_continue_trace(format_args!(
        "install_continue_trace_hooks begin {}",
        game_man_trace_summary()
    ));

    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_continue_trace(format_args!("MH_Initialize failed: {status:?}"));
            return;
        }
    }

    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "menu_continue_wrapper",
            MENU_CONTINUE_WRAPPER_RVA,
            menu_continue_wrapper_hook as *mut c_void,
            &MENU_CONTINUE_WRAPPER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "menu_new_or_load_wrapper",
            MENU_NEW_OR_LOAD_WRAPPER_RVA,
            menu_new_or_load_wrapper_hook as *mut c_void,
            &MENU_NEW_OR_LOAD_WRAPPER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "menu_other_load_wrapper",
            MENU_OTHER_LOAD_WRAPPER_RVA,
            menu_other_load_wrapper_hook as *mut c_void,
            &MENU_OTHER_LOAD_WRAPPER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "set_save_slot",
            SET_SAVE_SLOT_RVA,
            set_save_slot_hook as *mut c_void,
            &SET_SAVE_SLOT_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "save_request_profile",
            SAVE_REQUEST_PROFILE_RVA,
            save_request_profile_hook as *mut c_void,
            &SAVE_REQUEST_PROFILE_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "request_save",
            REQUEST_SAVE_RVA,
            request_save_hook as *mut c_void,
            &REQUEST_SAVE_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "current_slot_load_67b570",
            CURRENT_SLOT_LOAD_RVA,
            current_slot_load_hook as *mut c_void,
            &CURRENT_SLOT_LOAD_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "continue_load_67b750",
            CONTINUE_LOAD_RVA,
            continue_load_hook as *mut c_void,
            &CONTINUE_LOAD_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "combined_load_67b940",
            COMBINED_LOAD_RVA,
            combined_load_hook as *mut c_void,
            &COMBINED_LOAD_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "map_load_67bc10",
            MAP_LOAD_RVA,
            map_load_hook as *mut c_void,
            &MAP_LOAD_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "save_load_state_init_67b030",
            SAVE_LOAD_STATE_INIT_RVA,
            save_load_state_init_hook as *mut c_void,
            &SAVE_LOAD_STATE_INIT_ORIG,
        );
    }

    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            write_bootstrap_event(
                BOOTSTRAP_EVENT_CONTINUE_TRACE_APPLIED,
                BOOTSTRAP_DETAIL_DONE,
            );
            append_continue_trace(format_args!(
                "install_continue_trace_hooks applied count={} {}",
                hooks.len(),
                game_man_trace_summary()
            ));
        }
        status => {
            let detail = format!("MH_ApplyQueued failed: {status:?}");
            write_bootstrap_event(BOOTSTRAP_EVENT_CONTINUE_TRACE_APPLY_FAILED, &detail);
            append_continue_trace(format_args!("{detail}"));
        }
    }

    std::mem::forget(hooks);
}

unsafe fn call_wrapper_original(original: &AtomicUsize, this: *mut c_void) -> Option<*mut c_void> {
    let original = original.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let original: unsafe extern "system" fn(*mut c_void) -> *mut c_void =
        unsafe { std::mem::transmute(original) };
    Some(unsafe { original(this) })
}

unsafe fn call_bool3_original(original: &AtomicUsize, arg0: i32, arg1: u8, arg2: u8) -> Option<u8> {
    let original = original.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let original: unsafe extern "system" fn(i32, u8, u8) -> u8 =
        unsafe { std::mem::transmute(original) };
    Some(unsafe { original(arg0, arg1, arg2) })
}

unsafe extern "system" fn menu_continue_wrapper_hook(this: *mut c_void) -> *mut c_void {
    append_continue_trace(format_args!(
        "ENTER menu_continue_wrapper this={this:p} {}",
        game_man_trace_summary()
    ));
    let result =
        unsafe { call_wrapper_original(&MENU_CONTINUE_WRAPPER_ORIG, this) }.unwrap_or(this);
    append_continue_trace(format_args!(
        "LEAVE menu_continue_wrapper ret={result:p} {}",
        game_man_trace_summary()
    ));
    result
}

unsafe extern "system" fn menu_new_or_load_wrapper_hook(this: *mut c_void) -> *mut c_void {
    append_continue_trace(format_args!(
        "ENTER menu_new_or_load_wrapper this={this:p} {}",
        game_man_trace_summary()
    ));
    let result =
        unsafe { call_wrapper_original(&MENU_NEW_OR_LOAD_WRAPPER_ORIG, this) }.unwrap_or(this);
    append_continue_trace(format_args!(
        "LEAVE menu_new_or_load_wrapper ret={result:p} {}",
        game_man_trace_summary()
    ));
    result
}

unsafe extern "system" fn menu_other_load_wrapper_hook(this: *mut c_void) -> *mut c_void {
    append_continue_trace(format_args!(
        "ENTER menu_other_load_wrapper this={this:p} {}",
        game_man_trace_summary()
    ));
    let result =
        unsafe { call_wrapper_original(&MENU_OTHER_LOAD_WRAPPER_ORIG, this) }.unwrap_or(this);
    append_continue_trace(format_args!(
        "LEAVE menu_other_load_wrapper ret={result:p} {}",
        game_man_trace_summary()
    ));
    result
}

unsafe extern "system" fn set_save_slot_hook(slot: i32) {
    append_continue_trace(format_args!(
        "ENTER set_save_slot slot={slot} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = SET_SAVE_SLOT_ORIG.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(i32) = unsafe { std::mem::transmute(original) };
        unsafe { original(slot) };
    }
    append_continue_trace(format_args!(
        "LEAVE set_save_slot {}",
        game_man_trace_summary()
    ));
}

unsafe extern "system" fn save_request_profile_hook(enabled: u8) {
    append_continue_trace(format_args!(
        "ENTER save_request_profile enabled={enabled} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = SAVE_REQUEST_PROFILE_ORIG.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(u8) = unsafe { std::mem::transmute(original) };
        unsafe { original(enabled) };
    }
    append_continue_trace(format_args!(
        "LEAVE save_request_profile {}",
        game_man_trace_summary()
    ));
}

unsafe extern "system" fn request_save_hook(enabled: u8) {
    append_continue_trace(format_args!(
        "ENTER request_save enabled={enabled} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = REQUEST_SAVE_ORIG.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(u8) = unsafe { std::mem::transmute(original) };
        unsafe { original(enabled) };
    }
    append_continue_trace(format_args!(
        "LEAVE request_save {}",
        game_man_trace_summary()
    ));
}

unsafe extern "system" fn current_slot_load_hook(arg0: i32, arg1: u8, arg2: u8) -> u8 {
    append_continue_trace(format_args!(
        "ENTER current_slot_load_67b570 arg0={arg0} arg1={arg1} arg2={arg2} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let ret = unsafe { call_bool3_original(&CURRENT_SLOT_LOAD_ORIG, arg0, arg1, arg2) }
        .unwrap_or(HOOK_FALSE_RETURN);
    append_continue_trace(format_args!(
        "LEAVE current_slot_load_67b570 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

unsafe extern "system" fn continue_load_hook(slot: i32, arg1: u8, arg2: u8) -> u8 {
    append_continue_trace(format_args!(
        "ENTER continue_load_67b750 slot={slot} arg1={arg1} arg2={arg2} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let ret = unsafe { call_bool3_original(&CONTINUE_LOAD_ORIG, slot, arg1, arg2) }
        .unwrap_or(HOOK_FALSE_RETURN);
    append_continue_trace(format_args!(
        "LEAVE continue_load_67b750 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

unsafe extern "system" fn combined_load_hook(slot: i32, arg1: u8, arg2: u8) -> u8 {
    append_continue_trace(format_args!(
        "ENTER combined_load_67b940 slot={slot} arg1={arg1} arg2={arg2} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let ret = unsafe { call_bool3_original(&COMBINED_LOAD_ORIG, slot, arg1, arg2) }
        .unwrap_or(HOOK_FALSE_RETURN);
    append_continue_trace(format_args!(
        "LEAVE combined_load_67b940 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

unsafe extern "system" fn map_load_hook() -> u8 {
    append_continue_trace(format_args!(
        "ENTER map_load_67bc10 {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = MAP_LOAD_ORIG.load(Ordering::SeqCst);
    let ret = if original == HOOK_ORIGINAL_UNSET {
        HOOK_FALSE_RETURN
    } else {
        let original: unsafe extern "system" fn() -> u8 = unsafe { std::mem::transmute(original) };
        unsafe { original() }
    };
    if ret != HOOK_FALSE_RETURN {
        TITLE_BOOTSTRAP_SEEN.store(TITLE_BOOTSTRAP_SEEN_VALUE, Ordering::SeqCst);
    }
    append_continue_trace(format_args!(
        "LEAVE map_load_67bc10 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

unsafe extern "system" fn save_load_state_init_hook() -> u8 {
    append_continue_trace(format_args!(
        "ENTER save_load_state_init_67b030 {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = SAVE_LOAD_STATE_INIT_ORIG.load(Ordering::SeqCst);
    let ret = if original == HOOK_ORIGINAL_UNSET {
        HOOK_FALSE_RETURN
    } else {
        let original: unsafe extern "system" fn() -> u8 = unsafe { std::mem::transmute(original) };
        unsafe { original() }
    };
    append_continue_trace(format_args!(
        "LEAVE save_load_state_init_67b030 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

fn state_or_return(state: &Arc<Mutex<EffectsState>>) -> std::sync::MutexGuard<'_, EffectsState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct AnimationObservation {
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
fn observe_animation(player: &PlayerIns, last_write_idx: Option<u32>) -> AnimationObservation {
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

fn valid_animation_id(anim_id: i32) -> Option<i32> {
    (anim_id > INVALID_ANIMATION_ID_FLOOR).then_some(anim_id)
}

fn process_global_driver_command(state: &mut EffectsState) {
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

fn process_driver_command(player: &mut PlayerIns, state: &mut EffectsState) {
    let path = command_path();
    let Ok(raw_command) = fs::read_to_string(&path) else {
        return;
    };
    let _ = fs::remove_file(path);

    execute_and_record_driver_command(player, state, raw_command.trim());
}

fn execute_and_record_driver_command(
    player: &mut PlayerIns,
    state: &mut EffectsState,
    command: &str,
) {
    if command.is_empty() {
        return;
    }

    state.last_driver_command = Some(match execute_driver_command(player, state, command) {
        Ok(()) => format!("ok: {command}"),
        Err(error) => format!("error: {command}: {error}"),
    });
}

fn execute_driver_command(
    player: &mut PlayerIns,
    state: &mut EffectsState,
    command: &str,
) -> Result<(), String> {
    let parts: Vec<_> = command.split_whitespace().collect();
    match parts.as_slice() {
        ["apply_all"] => {
            apply_selected_calls(player, state);
            refresh_call_status(player, state);
            Ok(())
        }
        ["remove_all"] => {
            for call in &mut state.calls {
                call.kind.remove(player);
                call.enabled = false;
                call.remove_requested = false;
                call.apply_failed = false;
            }
            refresh_call_status(player, state);
            Ok(())
        }
        ["apply", index] => set_call_enabled(player, state, parse_call_index(index)?, true),
        ["remove", index] => set_call_enabled(player, state, parse_call_index(index)?, false),
        ["set", index, "on"] => set_call_enabled(player, state, parse_call_index(index)?, true),
        ["set", index, "off"] => set_call_enabled(player, state, parse_call_index(index)?, false),
        ["toggle", index] => {
            let index = parse_call_index(index)?;
            let enabled = !state
                .calls
                .get(index)
                .ok_or_else(|| format!("call index {index} out of range"))?
                .enabled;
            set_call_enabled(player, state, index, enabled)
        }
        _ => Err("expected apply_all, remove_all, apply <index>, remove <index>, set <index> on|off, toggle <index>, or load_slot <index> before player load".to_owned()),
    }
}

fn parse_call_index(index: &str) -> Result<usize, String> {
    index
        .parse()
        .map_err(|error| format!("invalid call index {index:?}: {error}"))
}

fn set_call_enabled(
    player: &mut PlayerIns,
    state: &mut EffectsState,
    index: usize,
    enabled: bool,
) -> Result<(), String> {
    let call = state
        .calls
        .get_mut(index)
        .ok_or_else(|| format!("call index {index} out of range"))?;

    call.enabled = enabled;
    if enabled {
        call.kind.apply(player, state.network_sync);
        call.active = call.kind.is_active(player);
        call.apply_failed = !call.active;
    } else {
        call.kind.remove(player);
        call.remove_requested = false;
        call.apply_failed = false;
        call.active = call.kind.is_active(player);
    }

    Ok(())
}

fn remove_requested_calls(player: &mut PlayerIns, state: &mut EffectsState) {
    if state.remove_all_requested {
        for call in &mut state.calls {
            call.kind.remove(player);
            call.remove_requested = false;
            call.apply_failed = false;
        }
        state.remove_all_requested = false;
        return;
    }

    for call in &mut state.calls {
        if call.remove_requested {
            call.kind.remove(player);
            call.remove_requested = false;
            call.apply_failed = false;
        }
    }
}

fn apply_selected_calls(player: &mut PlayerIns, state: &mut EffectsState) {
    let network_sync = state.network_sync;
    for call in state.calls.iter_mut().filter(|call| call.enabled) {
        call.kind.apply(player, network_sync);
        // The game call reports nothing, so check the active list directly.
        call.apply_failed = !call.kind.is_active(player);
    }
}

fn refresh_call_status(player: &PlayerIns, state: &mut EffectsState) {
    for call in &mut state.calls {
        call.active = call.kind.is_active(player);
        if call.active {
            call.apply_failed = false;
        }
    }
}
