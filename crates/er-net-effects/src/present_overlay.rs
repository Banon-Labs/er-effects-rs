use std::{
    ffi::c_void,
    sync::Mutex,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use hudhook::{
    ImguiRenderLoop, MessageFilter, RenderContext,
    hooks::dx12::ImguiDx12Hooks,
    imgui::{Context, DrawListMut, FontConfig, FontSource, Io, MouseButton, Ui},
};

use crate::{
    crash_telemetry,
    effects::effect_selector_text,
    input_suppression,
    log::net_effects_log,
    overlay_layout::{PANEL_PADDING, PANEL_ROUNDING, Rect, panel_layout, rect_contains},
};

static HUDHOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static HUDHOOK_RENDER_HITS: AtomicUsize = AtomicUsize::new(0);
static HUDHOOK_VISIBLE_HITS: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_COLLAPSED: AtomicBool = AtomicBool::new(START_COLLAPSED);
static OVERLAY_TOGGLE_CLICKS: AtomicUsize = AtomicUsize::new(0);
static OVERLAY_TOGGLE_KEYS: AtomicUsize = AtomicUsize::new(0);
/// The button the last frame committed to, shared rather than owned by the render-loop struct.
///
/// When this module is a guest there is no `NetEffectsOverlay` instance -- the host owns the only
/// render loop -- so the state the draw needs between frames cannot live in `self`.
static TOGGLE_RECT: Mutex<Option<Rect>> = Mutex::new(None);

/// The bar starts minimized to its button and stays out of the way until it is asked for.
const START_COLLAPSED: bool = true;

/// imgui's default font rasterizes at 13px. The bar is read at a glance while playing, from
/// further away than a desktop app, so it asks for 25% more.
const OVERLAY_FONT_SIZE_PX: f32 = 13.0 * 1.25;

const OVERLAY_TITLE: &str = "ER NET EFFECTS";
/// Shown while collapsed: click to maximize.
const EXPAND_MARK: &str = "[+]";
/// Shown while expanded: click to minimize.
const COLLAPSE_MARK: &str = "[-]";

const PANEL_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 0.68];
const BUTTON_HOVER_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.14];
const BUTTON_BORDER_COLOR: [f32; 4] = [0.62, 0.58, 0.48, 0.75];
const BUTTON_BORDER_HOVER_COLOR: [f32; 4] = [0.98, 0.92, 0.72, 1.0];
const TITLE_COLOR: [f32; 4] = [0.95, 0.90, 0.78, 1.0];
const LINE_COLOR: [f32; 4] = [0.96, 0.94, 0.88, 1.0];

/// Draw the bar into a host module's frame.
///
/// # Safety
///
/// `ui` is a live `&Ui` supplied by the overlay host for the duration of this call.
unsafe extern "C" fn guest_draw(frame: *const er_build_watermark_core::overlay_host::OverlayFrame) {
    // Adopt the host's imgui context and allocators before touching `ui`. Without this the
    // context global in this DLL is null and `ui.io()` faults on the first field read.
    // SAFETY: `frame` is the pointer the host just handed us, live for this call.
    let Some(ui) = (unsafe { er_build_watermark_core::overlay_host::adopt_frame(frame) }) else {
        return;
    };
    draw_bar(ui);
}

/// Put the bar on screen, hosting the imgui context or joining whoever already does.
///
/// Both paths draw the identical bar. Which one is taken is decided by load order, and load
/// order is not something this crate gets to choose: me3 loads natives in profile order, so
/// `er_build_watermark.dll` (b) is mapped before `er_net_effects.dll` (n) and would otherwise
/// win the swapchain by alphabet. Before this, losing that race meant the bar was installed,
/// never rendered, and never logged an error -- the user simply found it gone.
pub(crate) fn install_present_overlay_hook(hmodule_raw: usize) {
    if HUDHOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }

    // Try to join an existing host before claiming anything. A guest costs no `Present` hook at
    // all, which is strictly better than a second one.
    if er_build_watermark_core::overlay_host::register_with_host(guest_draw) {
        crash_telemetry::hudhook_apply_ok();
        net_effects_log(format_args!(
            "present-overlay: another module hosts the imgui context; registered as a GUEST \
             (no second Present hook)"
        ));
        return;
    }

    // Nobody hosts one yet -- but the claim below waits for the game's window before it touches
    // the mutex, and every other would-be host is waiting on that same window. So the probe above
    // was taken before anyone could have designated themselves, and losing the mutex here means a
    // host appeared in between. Ask again on that path rather than giving up: a single stale probe
    // is precisely how this bar went missing.
    match er_build_watermark_core::claim_overlay_ownership() {
        er_build_watermark_core::OverlayClaim::Won => {}
        er_build_watermark_core::OverlayClaim::LostToAnotherModule => {
            if er_build_watermark_core::overlay_host::register_with_host_retrying(guest_draw) {
                crash_telemetry::hudhook_apply_ok();
                net_effects_log(format_args!(
                    "present-overlay: another module won the overlay while this one waited for \
                     the window; registered as a GUEST (no second Present hook)"
                ));
            } else {
                crash_telemetry::hudhook_apply_failed();
                net_effects_log(format_args!(
                    "present-overlay: a module owns the overlay but would not accept a guest -- \
                     the bar cannot be drawn. The host speaks a different overlay ABI than this \
                     DLL's {:#06x}; rebuild the whole profile from one tree.",
                    er_build_watermark_core::overlay_host::OVERLAY_ABI_TAG
                ));
            }
            return;
        }
        er_build_watermark_core::OverlayClaim::NoWindow => {
            crash_telemetry::hudhook_apply_failed();
            net_effects_log(format_args!(
                "present-overlay: this process never got a sized top-level window, so there is \
                 nothing to draw the bar on and no host to join. Not an ABI problem."
            ));
            return;
        }
    }

    let hmodule = hudhook::windows::Win32::Foundation::HINSTANCE(hmodule_raw as *mut c_void);
    let result = hudhook::Hudhook::builder()
        .with::<ImguiDx12Hooks>(NetEffectsOverlay)
        .with_hmodule(hmodule)
        .build()
        .apply();
    match result {
        Ok(()) => {
            er_build_watermark_core::overlay_host::become_host();
            crash_telemetry::hudhook_apply_ok();
            net_effects_log(format_args!(
                "present-overlay: hudhook dx12 overlay installed (this module HOSTS the imgui \
                 context; the watermark and any other overlay draw through it)"
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

/// Is the bar currently minimized to its single button?
pub(crate) fn overlay_collapsed() -> bool {
    OVERLAY_COLLAPSED.load(Ordering::Relaxed)
}

/// How many times the player has clicked the minimize/maximize button.
pub(crate) fn overlay_toggle_clicks() -> usize {
    OVERLAY_TOGGLE_CLICKS.load(Ordering::Relaxed)
}

/// How many times the bar was expanded or minimized from the keyboard or a driver command.
///
/// Emitted beside the click count rather than folded into it, because the two answer different
/// questions: a session with clicks proves the pointer was free, and a session with only keys is
/// the normal one. Both being zero while `hudhook_render_count` climbs is the exact signature of
/// a bar nobody could open.
pub(crate) fn overlay_toggle_keys() -> usize {
    OVERLAY_TOGGLE_KEYS.load(Ordering::Relaxed)
}

/// Expand the bar, or minimize it back -- the keyboard's half of the `[+]` button.
///
/// Lives here rather than in `effects` because `OVERLAY_COLLAPSED` is read by the render thread
/// every frame and is deliberately the one place the collapsed state lives; mirroring it into the
/// game-thread state would lag the render loop by a frame (see `effects::selector_input_state`).
pub(crate) fn toggle_collapsed_by_key() -> bool {
    let now_collapsed = !OVERLAY_COLLAPSED.fetch_xor(true, Ordering::Relaxed);
    OVERLAY_TOGGLE_KEYS.fetch_add(1, Ordering::Relaxed);
    net_effects_log(format_args!(
        "present-overlay: bar {} by key",
        if now_collapsed {
            "minimized"
        } else {
            "maximized"
        }
    ));
    now_collapsed
}

/// Put the bar into an explicit state, for the `er-net-effects-command.txt` driver.
///
/// The command path exists so the expand/collapse machine can be exercised without a pointer and
/// without a keyboard -- the click path had no such handle, which is part of why it went a whole
/// session unproven.
pub(crate) fn set_collapsed_by_command(collapsed: bool) {
    if OVERLAY_COLLAPSED.swap(collapsed, Ordering::Relaxed) == collapsed {
        return;
    }
    OVERLAY_TOGGLE_KEYS.fetch_add(1, Ordering::Relaxed);
    net_effects_log(format_args!(
        "present-overlay: bar {} by driver command",
        if collapsed { "minimized" } else { "maximized" }
    ));
}

/// The host-side render loop. Carries no state of its own: everything the draw needs between
/// frames lives in the statics above, because the guest path has no instance to hold it.
struct NetEffectsOverlay;

impl ImguiRenderLoop for NetEffectsOverlay {
    fn initialize<'a>(&'a mut self, ctx: &mut Context, _render_context: &'a mut dyn RenderContext) {
        // Runs before the atlas is built, and this is the first font added, so it becomes the
        // default one every `add_text` below draws with.
        ctx.fonts().add_font(&[FontSource::DefaultFontData {
            config: Some(FontConfig {
                size_pixels: OVERLAY_FONT_SIZE_PX,
                ..FontConfig::default()
            }),
        }]);
        net_effects_log(format_args!(
            "present-overlay: default font sized {OVERLAY_FONT_SIZE_PX}px"
        ));
    }

    fn render(&mut self, ui: &mut Ui) {
        draw_bar(ui);
        // This module hosts the context, so every other overlay in the process draws here or
        // nowhere. Dispatched after our own bar so the watermark stays on top of it.
        er_build_watermark_core::overlay_host::dispatch_guests(ui);
        er_build_watermark_core::draw_rows(ui, net_effects_log);
    }

    /// Keep the click that hits our own button away from the game.
    ///
    /// Elden Ring reads the mouse through DirectInput, which never touches this window procedure,
    /// so this alone does not stop the click becoming an attack -- `input_suppression` blanks the
    /// left button in the DirectInput state for that. This closes the legacy-message half of the
    /// same hole, and only while the pointer is inside the button.
    fn message_filter(&self, io: &Io) -> MessageFilter {
        match TOGGLE_RECT.lock().ok().and_then(|rect| *rect) {
            Some(rect) if rect_contains(rect, io.mouse_pos) => MessageFilter::InputMouse,
            _ => MessageFilter::empty(),
        }
    }
}

/// Draw the bar. The only drawing path, taken identically whether this module hosts the imgui
/// context or is a guest inside another module's render loop -- so the two cannot drift and a
/// bug can never be "only in the guest case".
fn draw_bar(ui: &Ui) {
    // Counted here, not in the host `render()`, because the guest path does not go through it.
    // Keeping the counters on the host path made `hudhook_render_count` read 0 for a bar that was
    // drawing perfectly well as a guest -- an oracle that reports the old bug's exact signature
    // for a working feature is worse than no oracle.
    crash_telemetry::hudhook_render_enter();
    let _guard = RenderExitGuard;
    {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            crash_telemetry::hudhook_initialize();
            net_effects_log(format_args!(
                "present-overlay: hudhook render loop initialized"
            ));
        });
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
        // Nothing on screen owns the pointer, so nothing may swallow a click.
        forget_button();
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

    let collapsed = OVERLAY_COLLAPSED.load(Ordering::Relaxed);
    let toggle = draw_foreground_selector(ui, display, &selector_text, collapsed);
    let hovered = rect_contains(toggle, ui.io().mouse_pos);
    if hovered && ui.is_mouse_clicked(MouseButton::Left) {
        let now_collapsed = !collapsed;
        OVERLAY_COLLAPSED.store(now_collapsed, Ordering::Relaxed);
        OVERLAY_TOGGLE_CLICKS.fetch_add(1, Ordering::Relaxed);
        net_effects_log(format_args!(
            "present-overlay: bar {} by mouse click",
            if now_collapsed {
                "minimized"
            } else {
                "maximized"
            }
        ));
    }
    if let Ok(mut rect) = TOGGLE_RECT.lock() {
        *rect = Some(toggle);
    }
    input_suppression::set_pointer_over_overlay(hovered);
}

/// Closes the render window however `draw_bar` leaves -- including its early return when the
/// selector text is empty, which a plain call at the end of the function would skip.
struct RenderExitGuard;

impl Drop for RenderExitGuard {
    fn drop(&mut self) {
        crash_telemetry::hudhook_render_exit();
    }
}

/// The bar is off screen: it owns no pointer, so it may not swallow a click.
fn forget_button() {
    if let Ok(mut rect) = TOGGLE_RECT.lock() {
        *rect = None;
    }
    input_suppression::set_pointer_over_overlay(false);
}

/// Draw the bar and return the rectangle that minimizes/maximizes it.
fn draw_foreground_selector(
    ui: &Ui,
    display: [f32; 2],
    selector_text: &str,
    collapsed: bool,
) -> Rect {
    let row_height = ui.current_font_size();
    let lines = if collapsed {
        Vec::new()
    } else {
        selector_lines(selector_text)
    };
    let mark = if collapsed {
        EXPAND_MARK
    } else {
        COLLAPSE_MARK
    };
    let mark_width = ui.calc_text_size(mark)[0];
    // The header always reserves room for the marker, so the panel does not resize when the
    // `[+]` and `[-]` glyphs differ in width.
    let header_width = ui.calc_text_size(OVERLAY_TITLE)[0] + PANEL_PADDING + mark_width;
    let content_width = lines.iter().fold(header_width, |widest, line| {
        widest.max(ui.calc_text_size(line)[0])
    });
    let layout = panel_layout(display, content_width, row_height, lines.len() + 1);
    let hovered = rect_contains(layout.toggle, ui.io().mouse_pos);

    let draw_list = ui.get_foreground_draw_list();
    draw_list
        .add_rect(
            [layout.panel[0], layout.panel[1]],
            [layout.panel[2], layout.panel[3]],
            PANEL_COLOR,
        )
        .filled(true)
        .rounding(PANEL_ROUNDING)
        .build();
    if hovered {
        draw_list
            .add_rect(
                [layout.toggle[0], layout.toggle[1]],
                [layout.toggle[2], layout.toggle[3]],
                BUTTON_HOVER_COLOR,
            )
            .filled(true)
            .rounding(PANEL_ROUNDING)
            .build();
    }
    // Outlined in both states: an unmarked region nobody knows is clickable is not a button.
    draw_list
        .add_rect(
            [layout.toggle[0], layout.toggle[1]],
            [layout.toggle[2], layout.toggle[3]],
            if hovered {
                BUTTON_BORDER_HOVER_COLOR
            } else {
                BUTTON_BORDER_COLOR
            },
        )
        .filled(false)
        .rounding(PANEL_ROUNDING)
        .build();

    draw_shadowed_text(
        &draw_list,
        [layout.text_x, layout.first_row_y],
        TITLE_COLOR,
        OVERLAY_TITLE,
    );
    draw_shadowed_text(
        &draw_list,
        [layout.inner_right - mark_width, layout.first_row_y],
        TITLE_COLOR,
        mark,
    );
    for (index, line) in lines.iter().enumerate() {
        draw_shadowed_text(
            &draw_list,
            [
                layout.text_x,
                layout.first_row_y + layout.row_advance * (index as f32 + 1.0),
            ],
            LINE_COLOR,
            line,
        );
    }

    layout.toggle
}

fn draw_shadowed_text(
    draw_list: &DrawListMut<'_>,
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
