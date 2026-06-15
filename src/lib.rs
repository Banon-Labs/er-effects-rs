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
            Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
            System::{
                LibraryLoader::{GetModuleHandleA, GetProcAddress},
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
const INITIAL_GAME_TASK_TICKS: u64 = 0;
const GAME_TASK_TICK_INCREMENT: u64 = 1;
const SAFE_INPUT_MAX_CONFIRM_PULSES: u32 = 16;
const SAFE_INPUT_DEFAULT_INTERVAL_TICKS: u64 = 30;
const SAFE_INPUT_INITIAL_LAST_PULSE_TICK: u64 = 0;
const SAFE_INPUT_CONFIRM_HOOK_FRAMES: usize = 4;
const SAFE_INPUT_KEY_UP_STATE: i16 = 0;
const VK_RETURN_KEY: usize = 0x0d;
const VK_SPACE_KEY: usize = 0x20;
const KEYDOWN_LPARAM: isize = 1;
const KEYUP_LPARAM: isize = 0xc0000001u32 as isize;
const DIK_RETURN: usize = 0x1c;
const DIK_SPACE: usize = 0x39;
const DIRECT_INPUT_CREATE_DEVICE_VTBL_INDEX: usize = 3;
const DIRECT_INPUT_DEVICE_GET_STATE_VTBL_INDEX: usize = 9;
const HRESULT_SUCCESS_FLOOR: i32 = 0;
const SAFE_INPUT_DIRECT_INPUT_WAIT_TICKS: u64 = 300;
// The TitleStep ctor (0x140b0b1c0) stores this derived vtable to owner+0
// (`lea rax,[0x142b63bb0]; mov [rdi],rax` at 0x140b0b1e5). The previous value
// 0x02b63ba0 was off by 0x10 (the base/parent vtable), so the owner scan never
// matched the live object.
const TITLE_OWNER_VTABLE_RVA: usize = 0x02b63bb0;
const TITLE_OWNER_STATE_OFFSET: usize = 0x4c;
const TITLE_OWNER_SCAN_ALIGNMENT: usize = 8;
const TITLE_OWNER_SCAN_MAX_ADDRESS: usize = 0x0000_8000_0000_0000;
const TITLE_OWNER_TRACE_LIMIT: usize = 64;
/// How many `title_owner` calls to skip between full-memory owner scans.
///
/// The owner scan walks every committed region via `VirtualQuery`; running it
/// every frame while the owner does not yet exist (or cannot be matched)
/// collapses the game's frame rate. Throttling to roughly once per second at
/// 60 fps keeps a failed lookup from being user-visible.
const TITLE_OWNER_SCAN_CALL_INTERVAL: usize = 60;
const TITLE_OWNER_SCAN_COUNTDOWN_STEP: usize = 1;
const TITLE_OWNER_SCAN_COUNTDOWN_READY: usize = 0;
const TITLE_MENU_JOB_WAIT_RVA: usize = 0x00b0d400;
const TITLE_NATIVE_JOB_MIN_TICK: u64 = 170;
const MEM_COMMIT_NUMERIC: u32 = 0x1000;
const PAGE_NOACCESS_NUMERIC: u32 = 0x01;
const PAGE_GUARD_NUMERIC: u32 = 0x100;
const TRACE_MENU_CONTINUE_WRAPPER_RVA: u32 = 0x0082bac0;
const TRACE_MENU_NEW_OR_LOAD_WRAPPER_RVA: u32 = 0x0082ba80;
const TRACE_MENU_OTHER_LOAD_WRAPPER_RVA: u32 = 0x0082bb00;
const TRACE_MENU_TASK_UPDATE_WRAPPER_RVA: u32 = 0x0082a0f0;
const TRACE_MENU_TASK_UPDATE_TABLE_RVA: u32 = 0x02ac72a0;
const TRACE_TASK_ENQUEUE_RVA: u32 = 0x007a7b60;
const TRACE_UNKNOWN_TABLE_RVA: u32 = 0;
const MENU_TASK_STATE_PAYLOAD_PTR_OFFSET: usize = 0x30;
const MENU_TASK_STATE_DELAY_OFFSET: usize = 0x08;
const TASK_ENQUEUE_TRACE_LIMIT: usize = 256;
const NO_SAFE_INPUT_CONFIRM_FRAMES: usize = 0;
const SAFE_INPUT_CONFIRM_FRAME_DECREMENT: usize = 1;
const SAFE_INPUT_NO_CONFIRM_PULSES: u32 = 0;
const SAFE_INPUT_FIRST_PULSE_INDEX: u32 = 0;
const SAFE_INPUT_NEXT_PULSE_OFFSET: u32 = 1;
const SAFE_INPUT_POST_MAP_MIN_CONFIRM_COUNT: u32 = 5;
const SAFE_INPUT_INITIAL_DELAY_TICKS: u64 = 0;
const WINDOW_PID_UNSET: u32 = 0;
const ENUM_WINDOWS_STOP_NUMERIC: i32 = 0;
const ENUM_WINDOWS_CONTINUE_NUMERIC: i32 = 1;
const DIRECT_INPUT_FAILURE_HRESULT: i32 = -1;
const DIRECT_INPUT_KEY_DOWN_MASK: u8 = 0x80;
const MENU_TRACE_UNSEEN_SEQ: usize = 0;
const POST_MAP_CONTINUATION_STATE_QWORD: usize = 2;
const TITLE_OWNER_SCAN_START_ADDRESS: usize = 0;
const TITLE_OWNER_QUERY_FAILED_BYTES: usize = 0;
const PAGE_PROTECTION_NO_FLAGS: u32 = 0;
const TITLE_OWNER_MIN_STATE: i32 = 0;
const TITLE_OWNER_MAX_STATE: i32 = 11;
const TITLE_NATIVE_JOB_NOT_CALLED: usize = 0;
const TITLE_TRACE_SEQUENCE_INCREMENT: usize = 1;
const TITLE_NATIVE_JOB_TASK_DATA_ZERO: u8 = 0;
const TITLE_NATIVE_JOB_TASK_DATA_BYTES: usize = 16;
const TITLE_NATIVE_JOB_FRAME_DELTA_NUMERATOR: f32 = 1.0;
const TITLE_NATIVE_JOB_FRAME_RATE: f32 = 60.0;
const TITLE_NATIVE_JOB_DELTA_OFFSET_START: usize = 8;
const TITLE_NATIVE_JOB_DELTA_OFFSET_END: usize = 12;
const TITLE_NATIVE_JOB_CALLED_VALUE: usize = 1;
const TITLE_STEP_BEGIN_TITLE: i32 = 3;
const TITLE_STEP_PLAY_GAME: i32 = 5;
const TITLE_STEP_MENU_JOB_WAIT: i32 = 10;
const FORCE_PLAY_GAME_STATE_UNOBSERVED: i32 = -999;
/// One-shot "PlayGame requested" flag on the TitleStep owner. STEP_PlayGame only
/// runs its real load-trigger (`consume_owner300` 0x140ca89e0 on owner+0x300,
/// gated at 0x140b0d70c) when this byte is nonzero, then clears it. The menu
/// "Continue" selection normally sets it; we set it so the forced PlayGame step
/// actually starts the load instead of resetting via GameStepWait.
const TITLE_OWNER_PLAY_GAME_REQUEST_FLAG_OFFSET: usize = 0x3e1;
const TITLE_OWNER_PLAY_GAME_REQUEST_FLAG_SET: u8 = 1;
/// The save slot STEP_PlayGame actually loads. Its handler (0x140b0d5b0) reads
/// `mov eax,[owner+0xbc]` and feeds it through submit -> validate -> pair, which
/// writes the value to GameMan+0x14 (the load value). The +0xac0 save slot only
/// feeds global+0x1200, not the load pair — so this is the field to select.
const TITLE_OWNER_PLAY_GAME_SLOT_OFFSET: usize = 0xbc;
/// STEP_GameStepWait (handler 0x140b0cde0) waits on the load job at owner+0x2e8:
/// `cmp dword [job+0xd8],0 / jne wait`. Observe job+0xd8 while holding here to
/// learn whether anything drains the job (needs a pump) or it is static.
const TITLE_STEP_GAME_STEP_WAIT: i32 = 6;
const TITLE_OWNER_JOB_OFFSET: usize = 0x2e8;
const TITLE_OWNER_JOB_PENDING_OFFSET: usize = 0xd8;
const TITLE_JOB_OBSERVE_TICK_INTERVAL: u64 = 30;
const FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA: usize = 0x0067a810;
/// Global holding the GameMan pointer (`mov rax,[rip]` in set_save_slot 0x67a810
/// / save_slot_get 0x678ca0). Read-only diagnostics of the PlayGame load-pair
/// preconditions read GameMan through this.
const FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA: usize = 0x3d69918;
const FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET: usize = 0xac0;
const FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET: usize = 0x14;
const FORCE_PLAY_GAME_GM_PAIR_GATE_B28_OFFSET: usize = 0xb28;
const FORCE_PLAY_GAME_GM_VALIDATE_12D_OFFSET: usize = 0x12d;
const FORCE_PLAY_GAME_GM_VALIDATE_12E_OFFSET: usize = 0x12e;
const MENU_TASK_NULL_STATE_QWORD: usize = 0;
const MENU_TASK_NULL_PAYLOAD_PTR: usize = 0;
const MENU_TASK_STATE_PAYLOAD_CODE_OFFSET: usize = 4;
const MENU_TRACE_EVENT_INCREMENT: usize = 1;
const TASK_ENQUEUE_TRACE_INCREMENT: usize = 1;
static START_GAME_TASK: Once = Once::new();
static START_CONTINUE_TRACE: Once = Once::new();
static START_SAFE_INPUT_HOOKS: Once = Once::new();
static BOOTSTRAP_TELEMETRY_SEEN: AtomicUsize = AtomicUsize::new(BOOTSTRAP_TELEMETRY_UNSEEN);
static SAFE_INPUT_CONFIRM_FRAMES_REMAINING: AtomicUsize = AtomicUsize::new(0);

static MENU_CONTINUE_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(0);
static MENU_NEW_OR_LOAD_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(0);
static MENU_OTHER_LOAD_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(0);
static MENU_TASK_UPDATE_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(0);
static TASK_ENQUEUE_ORIG: AtomicUsize = AtomicUsize::new(0);
static SET_SAVE_SLOT_ORIG: AtomicUsize = AtomicUsize::new(0);
static SAVE_REQUEST_PROFILE_ORIG: AtomicUsize = AtomicUsize::new(0);
static REQUEST_SAVE_ORIG: AtomicUsize = AtomicUsize::new(0);
static CURRENT_SLOT_LOAD_ORIG: AtomicUsize = AtomicUsize::new(0);
static CONTINUE_LOAD_ORIG: AtomicUsize = AtomicUsize::new(0);
static COMBINED_LOAD_ORIG: AtomicUsize = AtomicUsize::new(0);
static MAP_LOAD_ORIG: AtomicUsize = AtomicUsize::new(0);
static SAVE_LOAD_STATE_INIT_ORIG: AtomicUsize = AtomicUsize::new(0);
static GET_ASYNC_KEY_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static GET_KEY_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static DIRECT_INPUT8_CREATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static DIRECT_INPUT_CREATE_DEVICE_ORIG: AtomicUsize = AtomicUsize::new(0);
static DIRECT_INPUT_GET_DEVICE_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
static TITLE_BOOTSTRAP_SEEN: AtomicUsize = AtomicUsize::new(0);
static TITLE_OWNER_PTR: AtomicUsize = AtomicUsize::new(0);
static TITLE_OWNER_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);
static TITLE_NATIVE_JOB_CALLED: AtomicUsize = AtomicUsize::new(0);
static FORCE_PLAY_GAME_CALLED: AtomicUsize = AtomicUsize::new(0);
static FORCE_PLAY_GAME_LAST_STATE: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(FORCE_PLAY_GAME_STATE_UNOBSERVED);
static TITLE_OWNER_SCAN_COUNTDOWN: AtomicUsize = AtomicUsize::new(0);
static SAFE_INPUT_CONFIRM_PULSE_SEQ: AtomicUsize = AtomicUsize::new(0);
static MENU_TRACE_EVENT_SEQ: AtomicUsize = AtomicUsize::new(0);
static MENU_TRACE_LAST_SEQ: AtomicUsize = AtomicUsize::new(0);
static MENU_TRACE_LAST_HOOK_RVA: AtomicUsize = AtomicUsize::new(0);
static MENU_TRACE_LAST_TABLE_RVA: AtomicUsize = AtomicUsize::new(0);
static MENU_TRACE_LAST_THIS: AtomicUsize = AtomicUsize::new(0);
static MENU_TRACE_LAST_STATE_QWORD: AtomicUsize = AtomicUsize::new(0);
static MENU_TRACE_LAST_PAYLOAD_PTR: AtomicUsize = AtomicUsize::new(0);
static TASK_ENQUEUE_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);

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

#[derive(Default)]
struct SafeInputRuntime {
    loaded: bool,
    confirm_count: u32,
    pulses_sent: u32,
    interval_ticks: u64,
    initial_delay_ticks: u64,
    last_pulse_tick: u64,
    hooks_requested: bool,
    last_status: Option<String>,
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
        "  \"autoload_require_title_bootstrap\": {},\n",
        state.autoload.requires_title_bootstrap()
    ));
    body.push_str(&format!(
        "  \"title_bootstrap_seen\": {},\n",
        TITLE_BOOTSTRAP_SEEN.load(Ordering::SeqCst) != TITLE_BOOTSTRAP_UNSEEN
    ));
    body.push_str(&format!(
        "  \"autoload_attempts\": {},\n",
        state.autoload.attempts()
    ));
    body.push_str(&format!(
        "  \"game_task_ticks\": {},\n",
        state.game_task_ticks
    ));
    body.push_str(&format!(
        "  \"safe_input_confirm_count\": {},\n",
        state.safe_input.confirm_count
    ));
    body.push_str(&format!(
        "  \"safe_input_pulses_sent\": {},\n",
        state.safe_input.pulses_sent
    ));
    body.push_str(&format!(
        "  \"safe_input_hooks_requested\": {},\n",
        state.safe_input.hooks_requested
    ));
    body.push_str(&format!(
        "  \"safe_input_hook_frames_remaining\": {},\n",
        SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst)
    ));
    body.push_str(&format!(
        "  \"safe_input_last_status\": {},\n",
        state.safe_input.last_status.as_ref().map_or_else(
            || "null".to_owned(),
            |status| format!("\"{}\"", json_escape(status))
        )
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
    if safe_input_path().exists() {
        START_SAFE_INPUT_HOOKS.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-safe-input-hooks".to_owned())
                .spawn(install_safe_input_hooks);
        });
    }
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
                    state.game_task_ticks += GAME_TASK_TICK_INCREMENT;
                    process_safe_input_request(&mut state);
                    process_autoload_request(&mut state);
                    write_telemetry_throttled(&mut state, false);
                    return;
                };

                let mut state = state_or_return(&state);
                state.game_task_ticks += GAME_TASK_TICK_INCREMENT;
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
        title_bootstrap_seen: TITLE_BOOTSTRAP_SEEN.load(Ordering::SeqCst) != TITLE_BOOTSTRAP_UNSEEN,
    };
    let _ = unsafe {
        state.autoload.process(game_man, context, |message| {
            append_autoload_debug(format_args!("{message}"))
        })
    };
}

fn process_safe_input_request(state: &mut EffectsState) {
    if SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst) > NO_SAFE_INPUT_CONFIRM_FRAMES {
        SAFE_INPUT_CONFIRM_FRAMES_REMAINING
            .fetch_sub(SAFE_INPUT_CONFIRM_FRAME_DECREMENT, Ordering::SeqCst);
    }
    if !state.safe_input.loaded {
        load_safe_input_runtime(&mut state.safe_input);
    }
    if state.safe_input.confirm_count == SAFE_INPUT_NO_CONFIRM_PULSES
        || state.safe_input.pulses_sent >= state.safe_input.confirm_count
    {
        return;
    }
    if DIRECT_INPUT_GET_DEVICE_STATE_ORIG.load(Ordering::SeqCst) == HOOK_ORIGINAL_UNSET
        && state.game_task_ticks < SAFE_INPUT_DIRECT_INPUT_WAIT_TICKS
    {
        state.safe_input.last_status = Some(format!(
            "waiting for DirectInput GetDeviceState hook before confirm pulses tick={}",
            state.game_task_ticks
        ));
        return;
    }
    if state.safe_input.pulses_sent == SAFE_INPUT_FIRST_PULSE_INDEX {
        if state.game_task_ticks < state.safe_input.initial_delay_ticks {
            state.safe_input.last_status = Some(format!(
                "waiting for initial safe-input delay tick={} target={}",
                state.game_task_ticks, state.safe_input.initial_delay_ticks
            ));
            return;
        }
    } else if state
        .game_task_ticks
        .saturating_sub(state.safe_input.last_pulse_tick)
        < state.safe_input.interval_ticks
    {
        return;
    }

    let before_snapshot = menu_trace_snapshot();
    let gate_reason = if requires_post_map_final_confirm_gate(&state.safe_input) {
        if !is_post_map_continuation_gate(before_snapshot) {
            state.safe_input.last_status = Some(format!(
                "waiting for post-map continuation input gate before final confirm tick={} {}",
                state.game_task_ticks,
                before_snapshot.summary()
            ));
            return;
        }
        Some("post_map_continuation")
    } else {
        None
    };

    let pulse_seq = SAFE_INPUT_CONFIRM_PULSE_SEQ
        .fetch_add(SAFE_INPUT_NEXT_PULSE_OFFSET as usize, Ordering::SeqCst)
        + SAFE_INPUT_NEXT_PULSE_OFFSET as usize;
    if let Some(reason) = gate_reason {
        let line = format!(
            "input_gate[{reason}] state-gated input satisfied pulse={}/{} tick={} {} {}",
            state.safe_input.pulses_sent + SAFE_INPUT_NEXT_PULSE_OFFSET,
            state.safe_input.confirm_count,
            state.game_task_ticks,
            before_snapshot.summary(),
            game_man_trace_summary()
        );
        append_autoload_debug(format_args!("{line}"));
        append_continue_trace(format_args!("{line}"));
    }
    append_confirm_probe(
        "before_confirm",
        pulse_seq,
        state.game_task_ticks,
        before_snapshot,
        None,
    );

    match emit_confirm_pulse_to_own_window() {
        Ok(()) => {
            state.safe_input.pulses_sent += SAFE_INPUT_NEXT_PULSE_OFFSET;
            state.safe_input.last_pulse_tick = state.game_task_ticks;
            state.safe_input.last_status = Some(format!(
                "confirm pulse {}/{} via DirectInput/key-state hook + post_message",
                state.safe_input.pulses_sent, state.safe_input.confirm_count
            ));
            append_autoload_debug(format_args!(
                "safe_input_confirm pulse {}/{} tick={} hook_frames={}",
                state.safe_input.pulses_sent,
                state.safe_input.confirm_count,
                state.game_task_ticks,
                SAFE_INPUT_CONFIRM_HOOK_FRAMES
            ));
            let after_snapshot = menu_trace_snapshot();
            append_confirm_probe(
                "after_confirm",
                pulse_seq,
                state.game_task_ticks,
                after_snapshot,
                Some(after_snapshot.advanced_from(before_snapshot)),
            );
        }
        Err(error) => {
            state.safe_input.last_status = Some(error.clone());
            append_autoload_debug(format_args!("safe_input_confirm {error}"));
            let after_snapshot = menu_trace_snapshot();
            append_confirm_probe(
                "after_confirm_error",
                pulse_seq,
                state.game_task_ticks,
                after_snapshot,
                Some(after_snapshot.advanced_from(before_snapshot)),
            );
        }
    }
}

fn requires_post_map_final_confirm_gate(runtime: &SafeInputRuntime) -> bool {
    runtime.confirm_count >= SAFE_INPUT_POST_MAP_MIN_CONFIRM_COUNT
        && runtime.pulses_sent + SAFE_INPUT_NEXT_PULSE_OFFSET == runtime.confirm_count
}

fn is_post_map_continuation_gate(snapshot: MenuTraceSnapshot) -> bool {
    snapshot.seq > MENU_TRACE_UNSEEN_SEQ
        && snapshot.hook_rva == TRACE_MENU_OTHER_LOAD_WRAPPER_RVA as usize
        && snapshot.state_qword == POST_MAP_CONTINUATION_STATE_QWORD
}

fn load_safe_input_runtime(runtime: &mut SafeInputRuntime) {
    runtime.loaded = true;
    runtime.interval_ticks = SAFE_INPUT_DEFAULT_INTERVAL_TICKS;
    runtime.initial_delay_ticks = SAFE_INPUT_INITIAL_DELAY_TICKS;
    runtime.last_pulse_tick = SAFE_INPUT_INITIAL_LAST_PULSE_TICK;

    let path = safe_input_path();
    let Ok(contents) = fs::read_to_string(&path) else {
        runtime.last_status = Some(format!("safe input config not found at {}", path.display()));
        return;
    };

    for line in contents.lines().map(str::trim) {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "confirm_count" => {
                runtime.confirm_count = value
                    .trim()
                    .parse::<u32>()
                    .unwrap_or(SAFE_INPUT_NO_CONFIRM_PULSES)
                    .min(SAFE_INPUT_MAX_CONFIRM_PULSES);
            }
            "interval_ticks" => {
                runtime.interval_ticks = value
                    .trim()
                    .parse::<u64>()
                    .unwrap_or(SAFE_INPUT_DEFAULT_INTERVAL_TICKS)
                    .max(GAME_TASK_TICK_INCREMENT);
            }
            "initial_delay_ticks" | "first_pulse_min_tick" => {
                runtime.initial_delay_ticks = value
                    .trim()
                    .parse::<u64>()
                    .unwrap_or(SAFE_INPUT_INITIAL_DELAY_TICKS)
                    .max(SAFE_INPUT_INITIAL_DELAY_TICKS);
            }
            "backend" => {}
            _ => {}
        }
    }
    runtime.hooks_requested = true;
    runtime.last_status = Some(format!(
        "loaded safe input config {} confirm_count={} interval_ticks={} initial_delay_ticks={}",
        path.display(),
        runtime.confirm_count,
        runtime.interval_ticks,
        runtime.initial_delay_ticks
    ));
    append_autoload_debug(format_args!(
        "{}",
        runtime
            .last_status
            .as_deref()
            .unwrap_or("loaded safe input config")
    ));
}

fn safe_input_path() -> PathBuf {
    std::env::var_os("ER_EFFECTS_SAFE_INPUT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            game_directory_path()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("er-effects-safe-input.txt")
        })
}

unsafe extern "system" fn find_own_window_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut window_pid = WINDOW_PID_UNSET;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut window_pid as *mut u32)) };
    let current_pid = unsafe { GetCurrentProcessId() };
    if window_pid == current_pid && unsafe { IsWindowVisible(hwnd).as_bool() } {
        let output = lparam.0 as *mut HWND;
        if !output.is_null() {
            unsafe { *output = hwnd };
        }
        return BOOL(ENUM_WINDOWS_STOP_NUMERIC);
    }
    BOOL(ENUM_WINDOWS_CONTINUE_NUMERIC)
}

fn own_window() -> Option<HWND> {
    let mut hwnd = HWND::default();
    unsafe {
        let _ = EnumWindows(
            Some(find_own_window_callback),
            LPARAM((&mut hwnd as *mut HWND).cast::<()>() as isize),
        );
    }
    if hwnd.0.is_null() { None } else { Some(hwnd) }
}

fn emit_confirm_pulse_to_own_window() -> Result<(), String> {
    SAFE_INPUT_CONFIRM_FRAMES_REMAINING.store(SAFE_INPUT_CONFIRM_HOOK_FRAMES, Ordering::SeqCst);
    let hwnd = own_window().ok_or_else(|| "no visible process window for safe input".to_owned())?;
    for key in [VK_RETURN_KEY, VK_SPACE_KEY] {
        unsafe { PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(key), LPARAM(KEYDOWN_LPARAM)) }
            .map_err(|error| format!("PostMessageW keydown {key:#x} failed: {error}"))?;
        unsafe { PostMessageW(Some(hwnd), WM_KEYUP, WPARAM(key), LPARAM(KEYUP_LPARAM)) }
            .map_err(|error| format!("PostMessageW keyup {key:#x} failed: {error}"))?;
    }
    Ok(())
}

fn safe_input_proc(module: &[u8], proc: &[u8]) -> Result<*mut c_void, String> {
    let module = unsafe { GetModuleHandleA(PCSTR(module.as_ptr())) }
        .map_err(|error| format!("GetModuleHandleA failed: {error}"))?;
    let proc = unsafe { GetProcAddress(module, PCSTR(proc.as_ptr())) }
        .ok_or_else(|| "GetProcAddress returned null".to_owned())?;
    Ok(proc as *mut c_void)
}

unsafe fn create_absolute_hook(
    hooks: &mut Vec<MhHook>,
    name: &str,
    target: *mut c_void,
    hook_impl: *mut c_void,
    original: &AtomicUsize,
) {
    match unsafe { MhHook::new(target, hook_impl) } {
        Ok(hook) => {
            original.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "safe_input hook {name}: queue_enable failed: {status:?}"
                ));
            } else {
                append_autoload_debug(format_args!(
                    "safe_input hook {name}: target={target:p} trampoline={:p}",
                    hook.trampoline()
                ));
                hooks.push(hook);
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "safe_input hook {name}: create failed at {target:p}: {status:?}"
        )),
    }
}

unsafe fn create_and_apply_single_hook(
    name: &str,
    target: *mut c_void,
    hook_impl: *mut c_void,
    original: &AtomicUsize,
) {
    if original.load(Ordering::SeqCst) != HOOK_ORIGINAL_UNSET {
        return;
    }
    let mut hooks = Vec::new();
    unsafe { create_absolute_hook(&mut hooks, name, target, hook_impl, original) };
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!("safe_input hook {name} applied")),
        status => append_autoload_debug(format_args!(
            "safe_input hook {name}: MH_ApplyQueued failed: {status:?}"
        )),
    }
    std::mem::forget(hooks);
}

fn install_safe_input_hooks() {
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!("safe_input MH_Initialize failed: {status:?}"));
            return;
        }
    }

    let mut hooks = Vec::new();
    match safe_input_proc(b"user32.dll\0", b"GetAsyncKeyState\0") {
        Ok(target) => unsafe {
            create_absolute_hook(
                &mut hooks,
                "GetAsyncKeyState",
                target,
                get_async_key_state_hook as *mut c_void,
                &GET_ASYNC_KEY_STATE_ORIG,
            )
        },
        Err(error) => append_autoload_debug(format_args!(
            "safe_input GetAsyncKeyState resolve failed: {error}"
        )),
    }
    match safe_input_proc(b"user32.dll\0", b"GetKeyState\0") {
        Ok(target) => unsafe {
            create_absolute_hook(
                &mut hooks,
                "GetKeyState",
                target,
                get_key_state_hook as *mut c_void,
                &GET_KEY_STATE_ORIG,
            )
        },
        Err(error) => append_autoload_debug(format_args!(
            "safe_input GetKeyState resolve failed: {error}"
        )),
    }
    match safe_input_proc(b"dinput8.dll\0", b"DirectInput8Create\0") {
        Ok(target) => unsafe {
            create_absolute_hook(
                &mut hooks,
                "DirectInput8Create",
                target,
                direct_input8_create_hook as *mut c_void,
                &DIRECT_INPUT8_CREATE_ORIG,
            )
        },
        Err(error) => append_autoload_debug(format_args!(
            "safe_input DirectInput8Create resolve failed: {error}"
        )),
    }

    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "safe_input hooks applied count={}",
            hooks.len()
        )),
        status => {
            append_autoload_debug(format_args!("safe_input MH_ApplyQueued failed: {status:?}"))
        }
    }
    std::mem::forget(hooks);
}

fn is_safe_input_confirm_key(vkey: i32) -> bool {
    matches!(vkey as usize, VK_RETURN_KEY | VK_SPACE_KEY)
}

fn safe_input_key_state_override(vkey: i32, original_value: i16) -> i16 {
    if is_safe_input_confirm_key(vkey)
        && SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst) > NO_SAFE_INPUT_CONFIRM_FRAMES
    {
        original_value | i16::MIN
    } else {
        original_value
    }
}

unsafe extern "system" fn get_async_key_state_hook(vkey: i32) -> i16 {
    type GetAsyncKeyState = unsafe extern "system" fn(i32) -> i16;
    let original = GET_ASYNC_KEY_STATE_ORIG.load(Ordering::SeqCst);
    let original_value = if original == HOOK_ORIGINAL_UNSET {
        SAFE_INPUT_KEY_UP_STATE
    } else {
        let original: GetAsyncKeyState = unsafe { std::mem::transmute(original) };
        unsafe { original(vkey) }
    };
    safe_input_key_state_override(vkey, original_value)
}

unsafe extern "system" fn get_key_state_hook(vkey: i32) -> i16 {
    type GetKeyState = unsafe extern "system" fn(i32) -> i16;
    let original = GET_KEY_STATE_ORIG.load(Ordering::SeqCst);
    let original_value = if original == HOOK_ORIGINAL_UNSET {
        SAFE_INPUT_KEY_UP_STATE
    } else {
        let original: GetKeyState = unsafe { std::mem::transmute(original) };
        unsafe { original(vkey) }
    };
    safe_input_key_state_override(vkey, original_value)
}

unsafe fn install_direct_input_create_device_hook(direct_input: *mut c_void) {
    if direct_input.is_null()
        || DIRECT_INPUT_CREATE_DEVICE_ORIG.load(Ordering::SeqCst) != HOOK_ORIGINAL_UNSET
    {
        return;
    }
    let vtable = unsafe { *(direct_input as *const *const *mut c_void) };
    if vtable.is_null() {
        return;
    }
    let target = unsafe { *vtable.add(DIRECT_INPUT_CREATE_DEVICE_VTBL_INDEX) };
    if target.is_null() {
        return;
    }
    unsafe {
        create_and_apply_single_hook(
            "IDirectInput8::CreateDevice",
            target,
            direct_input_create_device_hook as *mut c_void,
            &DIRECT_INPUT_CREATE_DEVICE_ORIG,
        )
    };
}

unsafe fn install_direct_input_get_state_hook(device: *mut c_void) {
    if device.is_null()
        || DIRECT_INPUT_GET_DEVICE_STATE_ORIG.load(Ordering::SeqCst) != HOOK_ORIGINAL_UNSET
    {
        return;
    }
    let vtable = unsafe { *(device as *const *const *mut c_void) };
    if vtable.is_null() {
        return;
    }
    let target = unsafe { *vtable.add(DIRECT_INPUT_DEVICE_GET_STATE_VTBL_INDEX) };
    if target.is_null() {
        return;
    }
    unsafe {
        create_and_apply_single_hook(
            "IDirectInputDevice8::GetDeviceState",
            target,
            direct_input_get_device_state_hook as *mut c_void,
            &DIRECT_INPUT_GET_DEVICE_STATE_ORIG,
        )
    };
}

unsafe extern "system" fn direct_input8_create_hook(
    instance: HINSTANCE,
    version: u32,
    riidltf: *const c_void,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> i32 {
    type DirectInput8Create = unsafe extern "system" fn(
        HINSTANCE,
        u32,
        *const c_void,
        *mut *mut c_void,
        *mut c_void,
    ) -> i32;
    let original = DIRECT_INPUT8_CREATE_ORIG.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return DIRECT_INPUT_FAILURE_HRESULT;
    }
    let original: DirectInput8Create = unsafe { std::mem::transmute(original) };
    let hr = unsafe { original(instance, version, riidltf, out, outer) };
    if hr >= HRESULT_SUCCESS_FLOOR && !out.is_null() {
        let direct_input = unsafe { *out };
        unsafe { install_direct_input_create_device_hook(direct_input) };
    }
    hr
}

unsafe extern "system" fn direct_input_create_device_hook(
    this: *mut c_void,
    guid: *const c_void,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> i32 {
    type CreateDevice =
        unsafe extern "system" fn(*mut c_void, *const c_void, *mut *mut c_void, *mut c_void) -> i32;
    let original = DIRECT_INPUT_CREATE_DEVICE_ORIG.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return DIRECT_INPUT_FAILURE_HRESULT;
    }
    let original: CreateDevice = unsafe { std::mem::transmute(original) };
    let hr = unsafe { original(this, guid, out, outer) };
    if hr >= HRESULT_SUCCESS_FLOOR && !out.is_null() {
        let device = unsafe { *out };
        unsafe { install_direct_input_get_state_hook(device) };
    }
    hr
}

unsafe extern "system" fn direct_input_get_device_state_hook(
    this: *mut c_void,
    data_len: u32,
    data: *mut c_void,
) -> i32 {
    type GetDeviceState = unsafe extern "system" fn(*mut c_void, u32, *mut c_void) -> i32;
    let original = DIRECT_INPUT_GET_DEVICE_STATE_ORIG.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return DIRECT_INPUT_FAILURE_HRESULT;
    }
    let original: GetDeviceState = unsafe { std::mem::transmute(original) };
    let hr = unsafe { original(this, data_len, data) };
    if hr >= HRESULT_SUCCESS_FLOOR
        && !data.is_null()
        && SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst) > NO_SAFE_INPUT_CONFIRM_FRAMES
        && data_len as usize > DIK_SPACE
    {
        let state = unsafe { std::slice::from_raw_parts_mut(data as *mut u8, data_len as usize) };
        state[DIK_RETURN] |= DIRECT_INPUT_KEY_DOWN_MASK;
        state[DIK_SPACE] |= DIRECT_INPUT_KEY_DOWN_MASK;
    }
    hr
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

fn trace_menu_task_update_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TRACE_MENU_TASK_UPDATE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-trace-menu-task-update.txt")
        .exists()
}

fn native_title_job_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_AUTOLOAD_NATIVE_TITLE_JOB").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-native-title-job.txt")
        .exists()
}

fn force_play_game_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_AUTOLOAD_FORCE_PLAY_GAME").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-force-play-game.txt")
        .exists()
}

/// Drives the native TitleStep state machine to `STEP_PlayGame` once.
///
/// Live zero-input probes showed the game parks at `STEP_BeginTitle`
/// (PRESS ANY BUTTON) with GameMan ready but the MoveMapList load dispatcher
/// inactive, so directly setting the continue flags is a no-op. Static RE maps
/// the TitleStep handler table: index 5 (`STEP_PlayGame`, 0x140b0d5b0) reads the
/// selected save slot and submits the native load job. This selects slot `slot`
/// via the menu set-slot primitive and advances the owner's state field so the
/// game's own title task dispatches `STEP_PlayGame` on the next frame — no host
/// input and no synthetic load-primitive calls. We only act once the owner has
/// reached `STEP_BeginTitle`, which guarantees `STEP_InitMenu` already built the
/// menu object `STEP_PlayGame` depends on.
unsafe fn call_force_play_game_once(module_base: usize, slot: i32, tick: u64) -> bool {
    if tick < TITLE_NATIVE_JOB_MIN_TICK {
        return false;
    }
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        return false;
    };
    let state_before = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    // Log every TitleStep state transition so we can see whether the forced
    // STEP_PlayGame write sticks and advances (5 -> 6 GameStepWait -> load) or
    // gets reset by the title task / a different owner instance.
    let last_state = FORCE_PLAY_GAME_LAST_STATE.swap(state_before, Ordering::SeqCst);
    if state_before != last_state {
        // Read GameMan+0x14 (the load value pair writes) each transition: if it
        // becomes nonnegative when PlayGame runs (5 -> 6), the pair chain
        // succeeded and the gap is downstream (GameStepWait/job); if it stays -1,
        // submit/validate/pair never wrote it.
        let gm = unsafe { *((module_base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) as *const usize) };
        let load14 = if gm != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((gm + FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET) as *const i32) }
        } else {
            DIRECT_INPUT_FAILURE_HRESULT
        };
        append_autoload_debug(format_args!(
            "force_play_game: observed state {last_state}->{state_before} load14={load14} tick={tick}"
        ));
    }
    if FORCE_PLAY_GAME_CALLED.load(Ordering::SeqCst) != TITLE_NATIVE_JOB_NOT_CALLED {
        // Already drove the state once; keep observing transitions (logged above).
        // While parked in GameStepWait, periodically report the load job's pending
        // field so we can see whether anything drains it.
        if state_before == TITLE_STEP_GAME_STEP_WAIT {
            let job = unsafe { *(owner.add(TITLE_OWNER_JOB_OFFSET) as *const usize) };
            if job != TITLE_OWNER_SCAN_START_ADDRESS {
                let pending =
                    unsafe { *((job + TITLE_OWNER_JOB_PENDING_OFFSET) as *const i32) };
                if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                    append_autoload_debug(format_args!(
                        "force_play_game: gamestepwait job={job:#x} job_d8={pending} tick={tick}"
                    ));
                }
                // NOTE: calling the menu-task update wrapper (0x82a0f0) directly on
                // this job crashed the game (autoload-live-playgame-v10) -- the job
                // is not the right `this` / reentrancy. Pumping must go through the
                // game's own task runner; do not force-orphan the job.
            }
        }
        return true;
    }
    // The live title idles at STEP_MenuJobWait (the input-wait state shown as
    // PRESS ANY BUTTON); STEP_BeginTitle is the alternate stable pre-load step.
    // Both run after STEP_InitMenu built the menu object PlayGame needs.
    if state_before != TITLE_STEP_BEGIN_TITLE && state_before != TITLE_STEP_MENU_JOB_WAIT {
        return false;
    }
    let set_save_slot: unsafe extern "system" fn(i32) =
        unsafe { std::mem::transmute(module_base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
    unsafe { set_save_slot(slot) };
    // Read-only diagnostic: log the PlayGame load-pair preconditions so we can
    // see which one blocks (pair skips writing GameMan+0x14 unless b28==0; the
    // validate step gates on 12d/12e).
    let game_man_ptr =
        unsafe { *((module_base + FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA) as *const usize) };
    if game_man_ptr != TITLE_OWNER_SCAN_START_ADDRESS {
        let gm = game_man_ptr as *const u8;
        let ac0 = unsafe { *(gm.add(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
        let load14 = unsafe { *(gm.add(FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET) as *const i32) };
        let b28 = unsafe { *gm.add(FORCE_PLAY_GAME_GM_PAIR_GATE_B28_OFFSET) };
        let f12d = unsafe { *gm.add(FORCE_PLAY_GAME_GM_VALIDATE_12D_OFFSET) };
        let f12e = unsafe { *gm.add(FORCE_PLAY_GAME_GM_VALIDATE_12E_OFFSET) };
        append_autoload_debug(format_args!(
            "force_play_game: gm={game_man_ptr:#x} ac0={ac0} load14={load14} b28={b28} f12d={f12d} f12e={f12e}"
        ));
    }
    unsafe {
        *(owner.add(TITLE_OWNER_PLAY_GAME_REQUEST_FLAG_OFFSET) as *mut u8) =
            TITLE_OWNER_PLAY_GAME_REQUEST_FLAG_SET;
    }
    // Select the slot STEP_PlayGame loads: its handler reads owner+0xbc and the
    // pair step writes it to GameMan+0x14. Without this it stays -1 and pair bails.
    unsafe { *(owner.add(TITLE_OWNER_PLAY_GAME_SLOT_OFFSET) as *mut i32) = slot };
    unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *mut i32) = TITLE_STEP_PLAY_GAME };
    let state_after = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    FORCE_PLAY_GAME_CALLED.store(TITLE_NATIVE_JOB_CALLED_VALUE, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "force_play_game: set slot={slot} state {state_before}->{state_after} (STEP_PlayGame) tick={tick}"
    ));
    true
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

unsafe fn find_title_owner_by_vtable(module_base: usize) -> Option<*mut u8> {
    let target_vtable = module_base.checked_add(TITLE_OWNER_VTABLE_RVA)?;
    let mut address = TITLE_OWNER_SCAN_START_ADDRESS;
    while address < TITLE_OWNER_SCAN_MAX_ADDRESS {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let queried = unsafe {
            VirtualQuery(
                Some(address as *const c_void),
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if queried == TITLE_OWNER_QUERY_FAILED_BYTES {
            break;
        }

        let base = info.BaseAddress as usize;
        let size = info.RegionSize;
        let next = base.saturating_add(size);
        let state = info.State.0;
        let protect = info.Protect.0;
        if state == MEM_COMMIT_NUMERIC
            && protect & (PAGE_NOACCESS_NUMERIC | PAGE_GUARD_NUMERIC) == PAGE_PROTECTION_NO_FLAGS
            && size >= TITLE_OWNER_STATE_OFFSET + std::mem::size_of::<i32>()
        {
            let end = next.saturating_sub(TITLE_OWNER_STATE_OFFSET + std::mem::size_of::<i32>());
            let mut cursor = base;
            while cursor <= end {
                let vtable = unsafe { *(cursor as *const usize) };
                if vtable == target_vtable {
                    let state_value =
                        unsafe { *((cursor + TITLE_OWNER_STATE_OFFSET) as *const i32) };
                    if (TITLE_OWNER_MIN_STATE..=TITLE_OWNER_MAX_STATE).contains(&state_value) {
                        return Some(cursor as *mut u8);
                    }
                }
                cursor = cursor.saturating_add(TITLE_OWNER_SCAN_ALIGNMENT);
            }
        }

        if next <= address {
            break;
        }
        address = next;
    }
    None
}

unsafe fn title_owner(module_base: usize) -> Option<*mut u8> {
    let cached = TITLE_OWNER_PTR.load(Ordering::SeqCst) as *mut u8;
    if !cached.is_null() {
        return Some(cached);
    }
    // Throttle the full-memory scan: until the owner exists it would otherwise
    // run every frame and cripple FPS (observed ~2 task ticks/s).
    let countdown = TITLE_OWNER_SCAN_COUNTDOWN.load(Ordering::SeqCst);
    if countdown > TITLE_OWNER_SCAN_COUNTDOWN_READY {
        TITLE_OWNER_SCAN_COUNTDOWN.fetch_sub(TITLE_OWNER_SCAN_COUNTDOWN_STEP, Ordering::SeqCst);
        return None;
    }
    TITLE_OWNER_SCAN_COUNTDOWN.store(TITLE_OWNER_SCAN_CALL_INTERVAL, Ordering::SeqCst);
    let found = unsafe { find_title_owner_by_vtable(module_base) }?;
    TITLE_OWNER_PTR.store(found as usize, Ordering::SeqCst);
    let state_value = unsafe { *(found.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    append_autoload_debug(format_args!(
        "native_title_job: captured title owner={found:p} state={state_value}"
    ));
    Some(found)
}

unsafe fn call_native_title_job_once(module_base: usize, tick: u64) -> bool {
    if TITLE_NATIVE_JOB_CALLED.load(Ordering::SeqCst) != TITLE_NATIVE_JOB_NOT_CALLED {
        return true;
    }
    if tick < TITLE_NATIVE_JOB_MIN_TICK {
        let count = TITLE_OWNER_TRACE_COUNT
            .fetch_add(TITLE_TRACE_SEQUENCE_INCREMENT, Ordering::SeqCst)
            + TITLE_TRACE_SEQUENCE_INCREMENT;
        if count <= TITLE_OWNER_TRACE_LIMIT {
            append_autoload_debug(format_args!(
                "native_title_job: waiting for min tick tick={tick} target={TITLE_NATIVE_JOB_MIN_TICK}"
            ));
        }
        return false;
    }
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        let count = TITLE_OWNER_TRACE_COUNT
            .fetch_add(TITLE_TRACE_SEQUENCE_INCREMENT, Ordering::SeqCst)
            + TITLE_TRACE_SEQUENCE_INCREMENT;
        if count <= TITLE_OWNER_TRACE_LIMIT {
            append_autoload_debug(format_args!(
                "native_title_job: waiting for title owner at tick={tick}"
            ));
        }
        return false;
    };

    let state_before = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    let mut task_data = [TITLE_NATIVE_JOB_TASK_DATA_ZERO; TITLE_NATIVE_JOB_TASK_DATA_BYTES];
    let frame_delta = TITLE_NATIVE_JOB_FRAME_DELTA_NUMERATOR / TITLE_NATIVE_JOB_FRAME_RATE;
    task_data[TITLE_NATIVE_JOB_DELTA_OFFSET_START..TITLE_NATIVE_JOB_DELTA_OFFSET_END]
        .copy_from_slice(&frame_delta.to_le_bytes());
    let title_menu_job: unsafe extern "system" fn(*mut u8, *mut c_void) =
        unsafe { std::mem::transmute(module_base + TITLE_MENU_JOB_WAIT_RVA) };
    append_autoload_debug(format_args!(
        "native_title_job: ENTER owner={owner:p} state_before={state_before} tick={tick}"
    ));
    unsafe { title_menu_job(owner, task_data.as_mut_ptr().cast()) };
    let state_after = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    TITLE_NATIVE_JOB_CALLED.store(TITLE_NATIVE_JOB_CALLED_VALUE, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "native_title_job: LEAVE owner={owner:p} state_after={state_after} tick={tick}"
    ));
    true
}

#[derive(Clone, Copy)]
struct MenuTraceSnapshot {
    seq: usize,
    hook_rva: usize,
    table_rva: usize,
    this_ptr: usize,
    state_qword: usize,
    payload_ptr: usize,
}

impl MenuTraceSnapshot {
    fn advanced_from(self, previous: Self) -> bool {
        self.seq != previous.seq
            || self.hook_rva != previous.hook_rva
            || self.table_rva != previous.table_rva
            || self.this_ptr != previous.this_ptr
            || self.state_qword != previous.state_qword
            || self.payload_ptr != previous.payload_ptr
    }

    fn barrier_id(self) -> String {
        format!(
            "hook_0x{:x}/table_{}",
            self.hook_rva,
            trace_rva_label(self.table_rva)
        )
    }

    fn summary(self) -> String {
        format!(
            "last_menu_seq={} hook_rva=0x{:x} table_rva={} this=0x{:x} state_qword=0x{:x} payload_ptr=0x{:x}",
            self.seq,
            self.hook_rva,
            trace_rva_label(self.table_rva),
            self.this_ptr,
            self.state_qword,
            self.payload_ptr
        )
    }
}

fn menu_trace_snapshot() -> MenuTraceSnapshot {
    MenuTraceSnapshot {
        seq: MENU_TRACE_LAST_SEQ.load(Ordering::SeqCst),
        hook_rva: MENU_TRACE_LAST_HOOK_RVA.load(Ordering::SeqCst),
        table_rva: MENU_TRACE_LAST_TABLE_RVA.load(Ordering::SeqCst),
        this_ptr: MENU_TRACE_LAST_THIS.load(Ordering::SeqCst),
        state_qword: MENU_TRACE_LAST_STATE_QWORD.load(Ordering::SeqCst),
        payload_ptr: MENU_TRACE_LAST_PAYLOAD_PTR.load(Ordering::SeqCst),
    }
}

fn trace_rva_label(rva: usize) -> String {
    if rva == TRACE_UNKNOWN_TABLE_RVA as usize {
        "unknown".to_owned()
    } else {
        format!("0x{rva:x}")
    }
}

fn append_confirm_probe(
    phase: &str,
    pulse_seq: usize,
    tick: u64,
    snapshot: MenuTraceSnapshot,
    advanced_after_pulse: Option<bool>,
) {
    let advanced =
        advanced_after_pulse.map_or_else(|| "unknown".to_owned(), |value| value.to_string());
    let line = format!(
        "confirm_probe phase={phase} pulse={pulse_seq} tick={tick} menu_condition[unknown_confirmable_modal] barrier_id={} observed_after_pulse={advanced} confirm_active={} {} {}",
        snapshot.barrier_id(),
        SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst) > NO_SAFE_INPUT_CONFIRM_FRAMES,
        snapshot.summary(),
        game_man_trace_summary()
    );
    append_autoload_debug(format_args!("{line}"));
    append_continue_trace(format_args!("{line}"));
}

unsafe fn menu_task_state_summary(this: *mut c_void) -> (usize, usize, String) {
    if this.is_null() {
        return (
            MENU_TASK_NULL_STATE_QWORD,
            MENU_TASK_NULL_PAYLOAD_PTR,
            "task_state{null=true}".to_owned(),
        );
    }
    let base = this.cast::<u8>();
    let state_qword = unsafe { *(base.cast::<usize>()) };
    let state_code = unsafe { *(base.cast::<i32>()) };
    let state_payload = unsafe { *(base.add(MENU_TASK_STATE_PAYLOAD_CODE_OFFSET).cast::<i32>()) };
    let delay_bits = unsafe { *(base.add(MENU_TASK_STATE_DELAY_OFFSET).cast::<u32>()) };
    let payload_ptr = unsafe { *(base.add(MENU_TASK_STATE_PAYLOAD_PTR_OFFSET).cast::<usize>()) };
    (
        state_qword,
        payload_ptr,
        format!(
            "task_state{{qword=0x{state_qword:x},code={state_code},payload={state_payload},delay_bits=0x{delay_bits:x},payload_ptr=0x{payload_ptr:x}}}"
        ),
    )
}

fn record_menu_trace_snapshot(
    seq: usize,
    hook_rva: u32,
    table_rva: u32,
    this: *mut c_void,
    state_qword: usize,
    payload_ptr: usize,
) {
    MENU_TRACE_LAST_SEQ.store(seq, Ordering::SeqCst);
    MENU_TRACE_LAST_HOOK_RVA.store(hook_rva as usize, Ordering::SeqCst);
    MENU_TRACE_LAST_TABLE_RVA.store(table_rva as usize, Ordering::SeqCst);
    MENU_TRACE_LAST_THIS.store(this as usize, Ordering::SeqCst);
    MENU_TRACE_LAST_STATE_QWORD.store(state_qword, Ordering::SeqCst);
    MENU_TRACE_LAST_PAYLOAD_PTR.store(payload_ptr, Ordering::SeqCst);
}

unsafe fn append_menu_semaphore_trace(
    hook_name: &str,
    phase: &str,
    hook_rva: u32,
    table_rva: u32,
    this: *mut c_void,
) {
    let seq = MENU_TRACE_EVENT_SEQ.fetch_add(MENU_TRACE_EVENT_INCREMENT, Ordering::SeqCst)
        + MENU_TRACE_EVENT_INCREMENT;
    let (state_qword, payload_ptr, task_state) = unsafe { menu_task_state_summary(this) };
    record_menu_trace_snapshot(seq, hook_rva, table_rva, this, state_qword, payload_ptr);
    append_continue_trace(format_args!(
        "menu_semaphore seq={seq} phase={phase} hook={hook_name} hook_rva=0x{hook_rva:x} table_rva={} this={this:p} barrier_id=hook_0x{hook_rva:x}/table_{} confirm_active={} pulse={} {} {} {}",
        trace_rva_label(table_rva as usize),
        trace_rva_label(table_rva as usize),
        SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst) > NO_SAFE_INPUT_CONFIRM_FRAMES,
        SAFE_INPUT_CONFIRM_PULSE_SEQ.load(Ordering::SeqCst),
        task_state,
        trace_callers_summary(),
        game_man_trace_summary()
    ));
}

unsafe fn object_vtable_summary(ptr: *mut c_void) -> String {
    if ptr.is_null() {
        return "vtable_rva=null".to_owned();
    }
    let vtable = unsafe { *(ptr as *const usize) };
    let rva = game_module_base()
        .ok()
        .and_then(|module_base| vtable.checked_sub(module_base));
    rva.map_or_else(
        || format!("vtable=0x{vtable:x} vtable_rva=unknown"),
        |value| format!("vtable=0x{vtable:x} vtable_rva=0x{value:x}"),
    )
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
        let requested_slot_index = read_i32(GAME_MAN_REQUESTED_SAVE_SLOT_LOAD_INDEX_OFFSET);
        let save_state = read_i32(GAME_MAN_SAVE_STATE_OFFSET);
        format!(
            "gm={game_man:p} slot={} req_idx={} b78={} state={} b80={} flags{{b72={},b73={},b74={},b75={},bb8={}}} bbc={} bc0={} bc4={}",
            read_i32(GAME_MAN_SAVE_SLOT_OFFSET),
            requested_slot_index,
            requested_slot_index,
            save_state,
            save_state,
            read_u8(GAME_MAN_FLAG_B72_OFFSET),
            read_u8(GAME_MAN_FLAG_B73_OFFSET),
            read_u8(GAME_MAN_FLAG_B74_OFFSET),
            read_u8(GAME_MAN_FLAG_B75_OFFSET),
            read_u8(GAME_MAN_FLAG_BB8_OFFSET),
            read_i32(GAME_MAN_FLAG_BBC_OFFSET),
            read_i32(GAME_MAN_FLAG_BC0_OFFSET),
            read_i32(GAME_MAN_FLAG_BC4_OFFSET),
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
        if trace_menu_task_update_enabled() {
            create_continue_trace_hook(
                &mut hooks,
                "menu_task_update_wrapper",
                TRACE_MENU_TASK_UPDATE_WRAPPER_RVA,
                menu_task_update_wrapper_hook as *mut c_void,
                &MENU_TASK_UPDATE_WRAPPER_ORIG,
            );
        } else {
            append_continue_trace(format_args!(
                "menu_task_update_wrapper trace skipped by default; set ER_EFFECTS_TRACE_MENU_TASK_UPDATE=1 for invasive pump diagnostics"
            ));
        }
        create_continue_trace_hook(
            &mut hooks,
            "task_enqueue_7a7b60",
            TRACE_TASK_ENQUEUE_RVA,
            task_enqueue_hook as *mut c_void,
            &TASK_ENQUEUE_ORIG,
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

unsafe fn call_task_enqueue_original(arg0: *mut c_void, arg1: *mut c_void) -> Option<*mut c_void> {
    let original = TASK_ENQUEUE_ORIG.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let original: unsafe extern "system" fn(*mut c_void, *mut c_void) -> *mut c_void =
        unsafe { std::mem::transmute(original) };
    Some(unsafe { original(arg0, arg1) })
}

unsafe extern "system" fn menu_continue_wrapper_hook(this: *mut c_void) -> *mut c_void {
    unsafe {
        append_menu_semaphore_trace(
            "menu_continue_wrapper",
            "ENTER",
            TRACE_MENU_CONTINUE_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            this,
        )
    };
    let result =
        unsafe { call_wrapper_original(&MENU_CONTINUE_WRAPPER_ORIG, this) }.unwrap_or(this);
    unsafe {
        append_menu_semaphore_trace(
            "menu_continue_wrapper",
            "LEAVE",
            TRACE_MENU_CONTINUE_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            result,
        )
    };
    result
}

unsafe extern "system" fn menu_new_or_load_wrapper_hook(this: *mut c_void) -> *mut c_void {
    unsafe {
        append_menu_semaphore_trace(
            "menu_new_or_load_wrapper",
            "ENTER",
            TRACE_MENU_NEW_OR_LOAD_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            this,
        )
    };
    let result =
        unsafe { call_wrapper_original(&MENU_NEW_OR_LOAD_WRAPPER_ORIG, this) }.unwrap_or(this);
    unsafe {
        append_menu_semaphore_trace(
            "menu_new_or_load_wrapper",
            "LEAVE",
            TRACE_MENU_NEW_OR_LOAD_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            result,
        )
    };
    result
}

unsafe extern "system" fn menu_other_load_wrapper_hook(this: *mut c_void) -> *mut c_void {
    unsafe {
        append_menu_semaphore_trace(
            "menu_other_load_wrapper",
            "ENTER",
            TRACE_MENU_OTHER_LOAD_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            this,
        )
    };
    let result =
        unsafe { call_wrapper_original(&MENU_OTHER_LOAD_WRAPPER_ORIG, this) }.unwrap_or(this);
    unsafe {
        append_menu_semaphore_trace(
            "menu_other_load_wrapper",
            "LEAVE",
            TRACE_MENU_OTHER_LOAD_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            result,
        )
    };
    result
}

unsafe extern "system" fn menu_task_update_wrapper_hook(this: *mut c_void) -> *mut c_void {
    unsafe {
        append_menu_semaphore_trace(
            "menu_task_update_wrapper",
            "ENTER",
            TRACE_MENU_TASK_UPDATE_WRAPPER_RVA,
            TRACE_MENU_TASK_UPDATE_TABLE_RVA,
            this,
        )
    };
    let result =
        unsafe { call_wrapper_original(&MENU_TASK_UPDATE_WRAPPER_ORIG, this) }.unwrap_or(this);
    unsafe {
        append_menu_semaphore_trace(
            "menu_task_update_wrapper",
            "LEAVE",
            TRACE_MENU_TASK_UPDATE_WRAPPER_RVA,
            TRACE_MENU_TASK_UPDATE_TABLE_RVA,
            result,
        )
    };
    result
}

unsafe extern "system" fn task_enqueue_hook(arg0: *mut c_void, arg1: *mut c_void) -> *mut c_void {
    let trace_index = TASK_ENQUEUE_TRACE_COUNT
        .fetch_add(TASK_ENQUEUE_TRACE_INCREMENT, Ordering::SeqCst)
        + TASK_ENQUEUE_TRACE_INCREMENT;
    let should_trace = trace_index <= TASK_ENQUEUE_TRACE_LIMIT
        || SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst)
            > NO_SAFE_INPUT_CONFIRM_FRAMES;
    if should_trace {
        append_continue_trace(format_args!(
            "menu_task_enqueue seq={trace_index} phase=ENTER hook_rva=0x{:x} list={arg0:p} node={arg1:p} node_{} confirm_active={} pulse={} {} {}",
            TRACE_TASK_ENQUEUE_RVA,
            unsafe { object_vtable_summary(arg1) },
            SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst)
                > NO_SAFE_INPUT_CONFIRM_FRAMES,
            SAFE_INPUT_CONFIRM_PULSE_SEQ.load(Ordering::SeqCst),
            trace_callers_summary(),
            game_man_trace_summary()
        ));
    }
    let result = unsafe { call_task_enqueue_original(arg0, arg1) }.unwrap_or(arg1);
    if should_trace {
        append_continue_trace(format_args!(
            "menu_task_enqueue seq={trace_index} phase=LEAVE ret={result:p} {}",
            game_man_trace_summary()
        ));
    }
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
