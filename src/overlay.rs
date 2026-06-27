use std::sync::{Arc, Mutex};

use debug::InputBlocker;
use eldenring::cs::PlayerIns;
use hudhook::{
    ImguiRenderLoop, MessageFilter,
    imgui::{Condition, Context, Ui},
};

use std::sync::atomic::Ordering;

use crate::*;

pub(crate) struct EffectsOverlay {
    state: Arc<Mutex<EffectsState>>,
}

impl EffectsOverlay {
    pub(crate) fn new(state: Arc<Mutex<EffectsState>>) -> Self {
        Self { state }
    }
}

fn title_portrait_source_ready() -> bool {
    TITLE_CUSTOM_COVER_PROFILE_SOURCE_RENDERER_VTABLE.load(Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
        && TITLE_CUSTOM_COVER_PROFILE_SOURCE_OFFSCREEN_REND.load(Ordering::SeqCst)
            != TITLE_OWNER_SCAN_START_ADDRESS
        && TITLE_CUSTOM_COVER_PROFILE_SOURCE_TEX_RESCAP.load(Ordering::SeqCst)
            != TITLE_OWNER_SCAN_START_ADDRESS
}

fn sample_title_portrait_source_for_telemetry(ui: &Ui) {
    let [width, height] = ui.io().display_size;
    if width <= 1.0 || height <= 1.0 || !title_portrait_source_ready() {
        return;
    }

    // Telemetry-only: no generic fullscreen/text scaffold. A keepable cover must come from
    // native visible-surface evidence or a future real portrait texture bridge.
    TITLE_OVERLAY_COVER_LAST_DISPLAY_W.store(width as usize, Ordering::SeqCst);
    TITLE_OVERLAY_COVER_LAST_DISPLAY_H.store(height as usize, Ordering::SeqCst);
    let tex_rescap = TITLE_CUSTOM_COVER_PROFILE_SOURCE_TEX_RESCAP.load(Ordering::SeqCst);
    let gx_texture =
        unsafe { safe_read_usize(tex_rescap + TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET) }
            .unwrap_or(0);
    let texture_resource = if gx_texture != 0 {
        unsafe { safe_read_usize(gx_texture + TITLE_CUSTOM_COVER_GX_TEXTURE_RESOURCE_OFFSET) }
            .unwrap_or(0)
    } else {
        0
    };
    if gx_texture != 0 {
        TITLE_OVERLAY_COVER_LAST_GX_TEXTURE.store(gx_texture, Ordering::SeqCst);
    }
    if texture_resource != 0 {
        TITLE_OVERLAY_COVER_LAST_TEXTURE_RESOURCE.store(texture_resource, Ordering::SeqCst);
    }
    if gx_texture != 0 && texture_resource != 0 {
        TITLE_OVERLAY_COVER_TEXTURE_BOUND.store(1, Ordering::SeqCst);
    }
}

impl ImguiRenderLoop for EffectsOverlay {
    fn initialize(&mut self, ctx: &mut Context, _render_context: &mut dyn hudhook::RenderContext) {
        // Elden Ring hides/captures the OS cursor under Proton. Draw ImGui's
        // software cursor so overlay hit-testing is visible during Linux smoke
        // tests and normal in-game use.
        ctx.io_mut().mouse_draw_cursor = true;
    }

    fn render(&mut self, ui: &mut Ui) {
        // render_liveness_probe() temporarily disabled: isolating whether the
        // boot-crash is the render-thread probe vs the game-task scan.
        let blocker = InputBlocker::get_instance();
        unsafe {
            let _ = blocker.install_hooks();
        }
        // During the zero-input autoload/own-stepper probe, HARD-block every input source
        // (DInput keyboard+mouse + XInput gamepad) so a focused window cannot contaminate the
        // result -- the run must prove the load happens with no real input at any layer. NOTE:
        // under the offline launcher this render loop does NOT run at the title, so the game
        // task drives the same block too (enforce_input_block_now); this branch just covers the
        // in-game/overlay case. Otherwise fall back to the overlay's want-capture heuristic.
        if block_input_enabled() {
            enforce_input_block_now();
        } else {
            blocker.block_from_io(ui.io());
        }

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
        if product_autoload_enabled() && !player_available {
            sample_title_portrait_source_for_telemetry(ui);
        }
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
