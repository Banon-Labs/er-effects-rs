//! Putting the routes on screen, and joining whatever imgui already exists in the process.
//!
//! This DLL never installs a second `Present` hook. If another module in this workspace is
//! already hosting the overlay it registers as a guest and draws through it; if nobody is, it
//! hosts and dispatches guests itself. Two `Hudhook::apply()` calls in one process double-hook
//! `Present` and the second one silently renders nothing -- measured live on 2026-08-25, and the
//! reason `er_build_watermark_core::overlay_host` exists.

#![cfg(windows)]

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use er_build_watermark_core::overlay_host::{OverlayFrame, adopt_frame, register_with_host};
use hudhook::hooks::dx12::ImguiDx12Hooks;
use hudhook::imgui::{Context, Ui};
use hudhook::{ImguiRenderLoop, RenderContext};

// `geometry` is reached through the camera the draw reads each frame; the type itself is only
// named by the projection tests, which is why the import is test-gated rather than the maths.
#[cfg(test)]
use crate::geometry::{self, Camera};
use crate::log::path_log;
use crate::routes::{RouteShape, Snapshot};

/// The routes the game thread most recently computed. Replaced whole, never edited in place: the
/// render thread must never see half of one frame's paths and half of another's.
static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);

/// Frames this module has drawn into. `0` with the feature enabled means the overlay never
/// reached the swapchain, which is a different problem from an empty roster.
static DRAWS: AtomicUsize = AtomicUsize::new(0);
/// Line segments emitted on the most recent draw. Zero segments with a non-empty snapshot means
/// every route projected off-screen or behind the camera.
static LAST_SEGMENTS: AtomicUsize = AtomicUsize::new(0);
/// Set once the module is either hosting or registered as a guest.
static INSTALLED: AtomicUsize = AtomicUsize::new(0);

/// How many times each line is redrawn to fake a glow, and how much wider each pass is.
///
/// Three passes at falling opacity: a wide dim halo, a mid pass, then the solid core. imgui has no
/// blur, so a "glowing" line is stacked strokes -- which is also what keeps a one-pixel path
/// legible over bright terrain without making it opaque.
const GLOW_PASSES: [(f32, f32); 3] = [(3.2, 0.20), (1.9, 0.40), (1.0, 1.0)];

/// Publish this frame's routes. Called from the game thread.
pub(crate) fn publish(snapshot: Snapshot) {
    if let Ok(mut slot) = SNAPSHOT.lock() {
        *slot = Some(snapshot);
    }
}

/// Clear the overlay. Called when the feature is toggled off or the world goes away.
pub(crate) fn clear() {
    if let Ok(mut slot) = SNAPSHOT.lock() {
        *slot = None;
    }
}

pub(crate) fn draws() -> usize {
    DRAWS.load(Ordering::Relaxed)
}

pub(crate) fn last_segments() -> usize {
    LAST_SEGMENTS.load(Ordering::Relaxed)
}

pub(crate) fn installed() -> bool {
    INSTALLED.load(Ordering::Relaxed) != 0
}

/// Draw the current snapshot onto a live imgui frame.
fn draw(ui: &Ui) {
    DRAWS.fetch_add(1, Ordering::Relaxed);
    let Ok(slot) = SNAPSHOT.lock() else {
        return;
    };
    let Some(snapshot) = slot.as_ref() else {
        LAST_SEGMENTS.store(0, Ordering::Relaxed);
        return;
    };
    // The camera is read here rather than carried in the snapshot: a path is drawn in screen
    // space, so a camera one frame stale makes the whole overlay swim across the screen whenever
    // the player turns. The route points are world-space and may be a frame old without anyone
    // noticing; the camera may not.
    // SAFETY: a read of the camera singleton; returns None whenever it is not up.
    let Some(camera) = (unsafe { crate::game::camera() }) else {
        LAST_SEGMENTS.store(0, Ordering::Relaxed);
        return;
    };
    let screen = ui.io().display_size;
    // The foreground list, so the paths sit above the game and above any imgui window another
    // overlay in this process happens to be drawing.
    let draw_list = ui.get_foreground_draw_list();
    let mut segments = 0usize;

    for route in &snapshot.routes {
        let points: Vec<[f32; 3]> = match &route.shape {
            RouteShape::Walk(points) => points.clone(),
            RouteShape::Arrow(arrow) => vec![
                arrow.tail,
                arrow.tip,
                arrow.left_barb,
                arrow.tip,
                arrow.right_barb,
            ],
        };
        for pair in points.windows(2) {
            let [from, to] = [pair[0], pair[1]];
            let Some((a, b)) = camera.project_segment(from, to, screen) else {
                continue;
            };
            for (width_scale, alpha_scale) in GLOW_PASSES {
                draw_list
                    .add_line(
                        a,
                        b,
                        [
                            route.color[0],
                            route.color[1],
                            route.color[2],
                            route.alpha * alpha_scale,
                        ],
                    )
                    .thickness(route.stroke_px * width_scale)
                    .build();
            }
            segments += 1;
        }
        // A dot where the route ends, so a path whose far end is behind a hill still says where it
        // was going rather than simply stopping.
        if let RouteShape::Walk(points) = &route.shape
            && let Some(last) = points.last()
            && let Some(point) = camera.project(*last, screen)
        {
            draw_list
                .add_circle(
                    point,
                    route.stroke_px * 1.8,
                    [route.color[0], route.color[1], route.color[2], route.alpha],
                )
                .filled(true)
                .build();
        }
    }
    LAST_SEGMENTS.store(segments, Ordering::Relaxed);
}

/// The guest entry point: adopt the host's imgui and draw.
///
/// # Safety
///
/// `frame` is the pointer the overlay host just passed, live for the duration of this call.
unsafe extern "C" fn guest_draw(frame: *const OverlayFrame) {
    // Adopt the host's context and allocators before touching `ui`. imgui's current context is a
    // per-DLL global, so this module's copy is null until this runs and `ui.io()` would fault.
    // SAFETY: `frame` is the host's live pointer.
    let Some(ui) = (unsafe { adopt_frame(frame) }) else {
        return;
    };
    draw(ui);
}

/// This module's own render loop, used only when nothing else in the process hosts one.
struct PathOverlay;

impl ImguiRenderLoop for PathOverlay {
    fn initialize<'a>(&'a mut self, _ctx: &mut Context, _render: &'a mut dyn RenderContext) {
        path_log(format_args!("overlay: render loop initialized"));
    }

    fn render(&mut self, ui: &mut Ui) {
        // Guests first and before any early return: this module hosts the only imgui context in
        // the process, so returning early here draws nothing for every other overlay too.
        er_build_watermark_core::overlay_host::dispatch_guests(ui);
        draw(ui);
        // The watermark is not a guest -- it never registers one, because its loser path assumes
        // whichever module hosts will carry its rows directly. `er-net-effects` and the watermark's
        // own render loop both do; this one did not, so a run where this module won the overlay had
        // no watermark at all and `er-build-watermark.log` fell silent after its claim line.
        er_build_watermark_core::draw_rows(ui, path_log);
    }
}

/// Join the process's overlay, hosting it if nobody else does.
pub(crate) fn install(hmodule_raw: usize) {
    if INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    if register_with_host(guest_draw) {
        path_log(format_args!(
            "overlay: another module hosts the imgui context; registered as a GUEST (no second \
             Present hook)"
        ));
        return;
    }
    // The claim below waits for the game's window before touching the mutex, and every other
    // would-be host waits on that same window. The probe above therefore ran before anyone could
    // have designated themselves host, so losing the mutex here means one appeared in between --
    // ask again rather than giving up on a stale answer.
    match er_build_watermark_core::claim_overlay_ownership() {
        er_build_watermark_core::OverlayClaim::Won => {}
        er_build_watermark_core::OverlayClaim::LostToAnotherModule => {
            if er_build_watermark_core::overlay_host::register_with_host_retrying(guest_draw) {
                // Installed stays set: this module is joined to an overlay and must not run the
                // install path again.
                path_log(format_args!(
                    "overlay: another module won the overlay while this one waited for the \
                     window; registered as a GUEST (no second Present hook)"
                ));
            } else {
                INSTALLED.store(0, Ordering::SeqCst);
                path_log(format_args!(
                    "overlay: a module owns the overlay but would not accept a guest -- paths \
                     cannot be drawn. The host speaks a different overlay ABI than this DLL's \
                     {:#06x}; rebuild the whole profile from one tree.",
                    er_build_watermark_core::overlay_host::OVERLAY_ABI_TAG
                ));
            }
            return;
        }
        er_build_watermark_core::OverlayClaim::NoWindow => {
            INSTALLED.store(0, Ordering::SeqCst);
            path_log(format_args!(
                "overlay: this process never got a sized top-level window, so there is nothing \
                 to draw paths on and no host to join. Not an ABI problem."
            ));
            return;
        }
    }
    let hmodule = hudhook::windows::Win32::Foundation::HINSTANCE(hmodule_raw as *mut c_void);
    match hudhook::Hudhook::builder()
        .with::<ImguiDx12Hooks>(PathOverlay)
        .with_hmodule(hmodule)
        .build()
        .apply()
    {
        Ok(()) => {
            er_build_watermark_core::overlay_host::become_host();
            path_log(format_args!(
                "overlay: hudhook dx12 overlay installed (this module HOSTS the imgui context)"
            ));
        }
        Err(error) => {
            INSTALLED.store(0, Ordering::SeqCst);
            path_log(format_args!(
                "overlay: hudhook dx12 install failed: {error:?}"
            ));
        }
    }
}

/// Screen-space extent of a projected route, for tests and telemetry that must not depend on
/// imgui being present.
#[cfg(test)]
pub(crate) fn projected_segment_count(
    camera: &Camera,
    points: &[[f32; 3]],
    screen: [f32; 2],
) -> usize {
    points
        .windows(2)
        .filter(|pair| camera.project_segment(pair[0], pair[1], screen).is_some())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Camera {
        Camera {
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            forward: [0.0, 0.0, 1.0],
            eye: [0.0, 0.0, 0.0],
            fov_y: std::f32::consts::FRAC_PI_2,
            aspect: 16.0 / 9.0,
        }
    }

    #[test]
    fn a_route_running_away_from_the_camera_draws_every_segment() {
        let points: Vec<[f32; 3]> = (1..=10).map(|step| [0.0, 0.0, step as f32 * 4.0]).collect();
        assert_eq!(
            projected_segment_count(&camera(), &points, [1920.0, 1080.0]),
            9
        );
    }

    #[test]
    fn a_route_entirely_behind_the_camera_draws_nothing() {
        let points: Vec<[f32; 3]> = (1..=5).map(|step| [0.0, 0.0, step as f32 * -4.0]).collect();
        assert_eq!(
            projected_segment_count(&camera(), &points, [1920.0, 1080.0]),
            0
        );
    }

    #[test]
    fn the_glow_passes_end_on_a_solid_core() {
        let (widest, faintest) = GLOW_PASSES[0];
        let (narrowest, solid) = GLOW_PASSES[GLOW_PASSES.len() - 1];
        assert!(widest > narrowest, "the halo must be wider than the core");
        assert!(faintest < solid, "the halo must be fainter than the core");
        assert_eq!(solid, 1.0, "the core must draw at the route's own alpha");
        assert_eq!(
            narrowest, 1.0,
            "the core must draw at the route's own width"
        );
    }

    #[test]
    fn the_arrow_is_drawn_as_a_shaft_and_two_barbs() {
        let arrow = geometry::arrow([0.0, 0.0, 0.0], [0.0, 0.0, 10.0], 3.0, [0.0, 1.0, 0.0])
            .expect("a direction");
        // The polyline the draw builds: tail->tip, tip->left, left->tip is skipped by returning to
        // the tip between barbs, tip->right. Five points, four segments, and a visible head.
        let points = [
            arrow.tail,
            arrow.tip,
            arrow.left_barb,
            arrow.tip,
            arrow.right_barb,
        ];
        assert_eq!(points.len(), 5);
        assert_eq!(points[1], points[3], "both barbs must start at the tip");
    }
}
