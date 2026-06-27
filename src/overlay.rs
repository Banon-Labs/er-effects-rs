use std::sync::{Arc, Mutex};

use debug::InputBlocker;
use eldenring::cs::PlayerIns;
use hudhook::{
    ImguiRenderLoop, MessageFilter,
    imgui::{Condition, Context, ImColor32, Ui},
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

fn draw_title_overlay_cover(ui: &Ui) {
    let [width, height] = ui.io().display_size;
    if width <= 1.0 || height <= 1.0 {
        return;
    }
    TITLE_OVERLAY_COVER_RENDER_CALLS.fetch_add(1, Ordering::SeqCst);
    TITLE_OVERLAY_COVER_LAST_DISPLAY_W.store(width as usize, Ordering::SeqCst);
    TITLE_OVERLAY_COVER_LAST_DISPLAY_H.store(height as usize, Ordering::SeqCst);
    let draw_list = ui.get_background_draw_list();
    let portrait_ready = title_portrait_source_ready();
    let source_tint = if portrait_ready {
        ImColor32::from_rgba(46, 34, 28, 242)
    } else {
        ImColor32::from_rgba(4, 6, 10, 232)
    };
    draw_list
        .add_rect([0.0, 0.0], [width, height], source_tint)
        .filled(true)
        .build();
    let portrait_min = [width * 0.31, height * 0.12];
    let portrait_max = [width * 0.69, height * 0.82];
    draw_list
        .add_rect(
            portrait_min,
            portrait_max,
            ImColor32::from_rgba(190, 156, 96, 210),
        )
        .rounding(18.0)
        .thickness(4.0)
        .build();
    draw_list
        .add_rect(
            [portrait_min[0] + 8.0, portrait_min[1] + 8.0],
            [portrait_max[0] - 8.0, portrait_max[1] - 8.0],
            ImColor32::from_rgba(38, 31, 26, 230),
        )
        .filled(true)
        .rounding(14.0)
        .build();
    let status = if portrait_ready {
        "Profile portrait source ready: SYSTEX_Menu_Profile00 / CSMenuProfModelRend"
    } else {
        "Waiting for RAM-backed profile portrait source"
    };
    draw_list.add_text(
        [width * 0.11, height * 0.86],
        ImColor32::from_rgba(232, 208, 154, 255),
        status,
    );
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
            draw_title_overlay_cover(ui);
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
