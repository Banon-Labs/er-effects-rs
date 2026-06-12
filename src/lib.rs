use std::{
    ffi::c_void,
    sync::{Arc, Mutex, Once},
    time::Duration,
};

use debug::InputBlocker;
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, PlayerIns},
    fd4::FD4TaskData,
    util::system::wait_for_system_init,
};
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
use fromsoftware_shared::{SharedTaskImpExt, program::Program};
use hudhook::{
    ImguiRenderLoop, MessageFilter,
    hooks::dx12::ImguiDx12Hooks,
    imgui::{Condition, Context, Ui},
    windows::Win32::{Foundation::HINSTANCE, System::SystemServices::DLL_PROCESS_ATTACH},
};

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

static START_GAME_TASK: Once = Once::new();

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
                    state.manual_apply_requested = true;
                }
                if ui.button("Remove all listed effects") {
                    state.remove_all_requested = true;
                }

                ui.separator();
                ui.input_int("Custom SpEffect ID", &mut state.custom_call_id)
                    .build();
                if ui.button("Add custom call") {
                    add_custom_call(&mut state);
                }

                ui.separator();
                ui.text("Named calls");

                for call in &mut state.calls {
                    let was_enabled = call.enabled;
                    let label = format!("{} ({})", call.name, call.kind.label());
                    if ui.checkbox(&label, &mut call.enabled) && was_enabled && !call.enabled {
                        call.remove_requested = true;
                    }
                    ui.same_line();
                    ui.text(call_status_text(call));
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

    let state = Arc::new(Mutex::new(EffectsState::default()));
    START_GAME_TASK.call_once({
        let state = Arc::clone(&state);
        move || spawn_game_task(state)
    });

    debug::initialize::<ImguiDx12Hooks>(
        hmodule,
        reason,
        || {
            wait_for_system_init(&Program::current(), Duration::MAX)
                .expect("timed out waiting for Elden Ring systems");
        },
        EffectsOverlay { state },
    )
}

fn spawn_game_task(state: Arc<Mutex<EffectsState>>) {
    std::thread::spawn(move || {
        let cs_task =
            CSTaskImp::wait_for_instance(Duration::MAX).expect("timed out waiting for CSTaskImp");

        cs_task.run_recurring(
            move |_: &FD4TaskData| {
                let Ok(player) = (unsafe { PlayerIns::local_player_mut() }) else {
                    return;
                };

                let mut state = state_or_return(&state);
                let observation = observe_animation(player, state.last_write_idx);
                state.current_animation_id = observation.current_animation_id;
                state.last_write_idx = Some(observation.write_idx);

                remove_requested_calls(player, &mut state);

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

                refresh_call_status(player, &mut state);
            },
            CSTaskGroupIndex::FrameBegin,
        );
    });
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
