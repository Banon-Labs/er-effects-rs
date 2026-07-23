use std::{
    ffi::c_void,
    sync::atomic::{AtomicUsize, Ordering},
};

use hudhook::{ImguiRenderLoop, hooks::dx12::ImguiDx12Hooks, imgui::Ui};

use crate::{crash_telemetry, effects::effect_selector_text, log::net_effects_log};

static HUDHOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static HUDHOOK_RENDER_HITS: AtomicUsize = AtomicUsize::new(0);
static HUDHOOK_VISIBLE_HITS: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn install_present_overlay_hook(hmodule_raw: usize) {
    if HUDHOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let hmodule = hudhook::windows::Win32::Foundation::HINSTANCE(hmodule_raw as *mut c_void);
    let result = hudhook::Hudhook::builder()
        .with::<ImguiDx12Hooks>(NetEffectsOverlay::default())
        .with_hmodule(hmodule)
        .build()
        .apply();
    match result {
        Ok(()) => {
            crash_telemetry::hudhook_apply_ok();
            net_effects_log(format_args!(
                "present-overlay: hudhook dx12 overlay installed"
            ));
        }
        Err(error) => {
            HUDHOOK_INSTALLED.store(0, Ordering::SeqCst);
            crash_telemetry::hudhook_apply_failed();
            net_effects_log(format_args!(
                "present-overlay: hudhook dx12 overlay install failed: {error:?}"
            ));
        }
    }
}

#[derive(Default)]
struct NetEffectsOverlay {
    initialized: bool,
}

impl ImguiRenderLoop for NetEffectsOverlay {
    fn render(&mut self, ui: &mut Ui) {
        crash_telemetry::hudhook_render_enter();
        self.render_inner(ui);
        crash_telemetry::hudhook_render_exit();
    }
}

impl NetEffectsOverlay {
    fn render_inner(&mut self, ui: &mut Ui) {
        if !self.initialized {
            self.initialized = true;
            crash_telemetry::hudhook_initialize();
            net_effects_log(format_args!(
                "present-overlay: hudhook render loop initialized"
            ));
        }
        let hits = HUDHOOK_RENDER_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        let display = ui.io().display_size;
        if hits == 1 {
            net_effects_log(format_args!(
                "present-overlay: hudhook first render display={:.0}x{:.0}",
                display[0], display[1]
            ));
        }
        let selector_text = effect_selector_text();
        if selector_text.trim().is_empty() {
            return;
        }
        let visible_hits = HUDHOOK_VISIBLE_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        crash_telemetry::hudhook_render_visible();
        if visible_hits == 1 {
            net_effects_log(format_args!(
                "present-overlay: hudhook first visible selector display={:.0}x{:.0} text='{selector_text}'",
                display[0], display[1]
            ));
        }

        draw_foreground_selector(ui, display, &selector_text);
    }
}

fn draw_foreground_selector(ui: &Ui, display: [f32; 2], selector_text: &str) {
    let lines = selector_lines(selector_text);
    if lines.is_empty() {
        return;
    }
    let draw_list = ui.get_foreground_draw_list();
    let width = 860.0_f32.min((display[0] - 96.0).max(420.0));
    let line_h = 26.0;
    let pad_x = 18.0;
    let pad_y = 14.0;
    let height = pad_y * 2.0 + line_h * (lines.len() as f32 + 1.0);
    let x = 48.0;
    let y = 48.0;

    draw_list
        .add_rect([x, y], [x + width, y + height], [0.0, 0.0, 0.0, 0.68])
        .filled(true)
        .rounding(6.0)
        .build();
    draw_shadowed_text(
        &draw_list,
        [x + pad_x, y + pad_y],
        [0.95, 0.90, 0.78, 1.0],
        "ER NET EFFECTS",
    );
    for (index, line) in lines.iter().enumerate() {
        draw_shadowed_text(
            &draw_list,
            [x + pad_x, y + pad_y + line_h * (index as f32 + 1.0)],
            [0.96, 0.94, 0.88, 1.0],
            line,
        );
    }
}

fn draw_shadowed_text(
    draw_list: &hudhook::imgui::DrawListMut<'_>,
    pos: [f32; 2],
    color: [f32; 4],
    text: impl AsRef<str>,
) {
    let text = text.as_ref();
    draw_list.add_text([pos[0] + 1.0, pos[1] + 1.0], [0.0, 0.0, 0.0, 1.0], text);
    draw_list.add_text(pos, color, text);
}

fn selector_lines(text: &str) -> Vec<String> {
    let parts = text.split(" | ").collect::<Vec<_>>();
    if parts.len() >= 4 {
        return vec![
            parts[0].to_owned(),
            parts[1].to_owned(),
            parts[2].to_owned(),
            parts[3..].join(" | "),
        ];
    }
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}
