//! The pure maths: world -> screen, how bold a path is, and what colour it gets.
//!
//! Everything here is `f32` in, `f32` out, with no game and no `windows` crate, so it is proven
//! by `cargo test` on the host. That split is deliberate: a projection that is off by a factor of
//! `aspect` looks, in game, exactly like "the overlay is broken", and diagnosing it from a
//! screenshot costs a launch. Here it costs a test.

// Every non-test consumer of this module is `cfg(windows)`, so on the host it is structurally
// dead and the allow is unavoidable. What keeps that from hiding a genuinely unused item -- the
// failure bd `host-build-cfg-gate-allow-pattern-hides-real-lints-2026-08-23` records against
// er-title-flow -- is that dead_code is still denied on the shipping target, where the windows
// modules are compiled and every one of these has a caller. A tighter `not(test)` variant was
// tried first and is wrong: it makes `cargo test` on the host fail for items whose only callers
// are cross-compiled, which is not a defect.
#![cfg_attr(not(windows), allow(dead_code))]

/// Camera-space `z` below which a point is behind (or on) the lens and cannot be projected.
///
/// Not `0.0`: a point exactly on the plane divides by zero and a point a micrometre in front of
/// it projects to somewhere past the horizon, which draws as a line shooting off screen. The
/// game's own near plane is typically much larger than this; this is only the arithmetic floor.
pub const NEAR_EPSILON: f32 = 0.05;

/// A camera reduced to what projection needs.
///
/// The three axes come from `CS::CSCam::viewMatrix` (`CSCam+0x10`), which despite the name is the
/// camera-to-world transform: rows 0..2 are the right/up/forward basis and row 3 is the eye
/// position in Havok space.
///
/// That direction is not an assumption. The game's own world-to-screen -- the overhead nameplate
/// path, `CSFeManImp::UpdateChrEnemyTagEntries` -> `FUN_1407763d0` -> `FloatMatrix4x4::TransformCopy`
/// (`0x140680910`) -- takes the matrix `GetViewMatrix` hands back and **`Invert`s** it before
/// multiplying a world position through. Inverting a matrix to reach camera space is only
/// necessary if it maps camera space out to the world. Had it been a true view matrix the game
/// would use it directly, and reading it as camera-to-world would put every path somewhere it
/// is not.
///
/// The axes are re-normalised by the reader in `crate::game::camera`, because this projection
/// takes the inverse by transposing the basis -- correct for an orthonormal one, silently
/// wrong for a scaled one. The engine's `Invert` handles the general case; normalising costs
/// three square roots a frame and removes the difference.
///
/// `fov_y` is the vertical field of view in RADIANS -- read out of `CS::CSPersCam::ToPerspective`
/// (`0x1403e9ac0`), which builds `cot(fov*0.5)` and divides only the X term by `aspect`. Getting
/// that backwards stretches the overlay horizontally by about 1.78 at 16:9, which is why the
/// convention is written down here rather than assumed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
    pub eye: [f32; 3],
    pub fov_y: f32,
    pub aspect: f32,
}

/// `a - b`, componentwise.
#[must_use]
pub fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `a + b * scale`.
#[must_use]
pub fn add_scaled(a: [f32; 3], b: [f32; 3], scale: f32) -> [f32; 3] {
    [
        a[0] + b[0] * scale,
        a[1] + b[1] * scale,
        a[2] + b[2] * scale,
    ]
}

#[must_use]
pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[must_use]
pub fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

/// Unit vector, or `None` for a vector too short to have a direction.
#[must_use]
pub fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let len = length(a);
    if len <= f32::EPSILON {
        return None;
    }
    Some([a[0] / len, a[1] / len, a[2] / len])
}

/// Walk a route and return a point every `spacing` metres along it, start and end included.
///
/// This is where the markers go. Placing one at every navmesh waypoint instead would clump them:
/// the engine emits waypoints where the mesh geometry demands a turn, so a route across open
/// ground is a handful of points tens of metres apart while a doorway is half a dozen inside two
/// metres. Even spacing is what reads as a trail rather than as debris.
///
/// The end point is always included even when it does not land on the spacing grid, because the
/// last marker is the one that says "here", and dropping it because the route length was not a
/// multiple of the spacing is the one omission a player would notice.
///
/// Returns at most `max` points; a route longer than `max * spacing` is truncated rather than
/// thinned, so the markers nearest you -- the ones you are about to walk past -- stay put.
pub fn resample(points: &[[f32; 3]], spacing: f32, max: usize) -> Vec<[f32; 3]> {
    if points.len() < 2 || !spacing.is_finite() || spacing <= 0.0 || max == 0 {
        return points.iter().copied().take(max).collect();
    }
    let mut out = vec![points[0]];
    // Distance already walked past the last emitted marker.
    let mut carried = 0.0f32;
    for pair in points.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let segment = sub(to, from);
        let span = length(segment);
        if !span.is_finite() || span <= f32::EPSILON {
            continue;
        }
        let Some(direction) = normalize(segment) else {
            continue;
        };
        let mut travelled = spacing - carried;
        while travelled <= span {
            if out.len() >= max {
                return out;
            }
            out.push(add_scaled(from, direction, travelled));
            travelled += spacing;
        }
        carried = span - (travelled - spacing);
    }
    let last = points[points.len() - 1];
    // Skipped only when a marker is already on the destination, which happens when the route
    // length is a whole multiple of the spacing. The threshold is a few centimetres rather than
    // a fraction of the spacing: half a spacing looks like a sensible "close enough" and quietly
    // deletes a real destination marker that merely fell one metre short of the grid.
    const COINCIDENT_METERS: f32 = 0.05;
    let duplicate = out
        .last()
        .is_some_and(|previous| length(sub(last, *previous)) <= COINCIDENT_METERS);
    if !duplicate && out.len() < max {
        out.push(last);
    }
    out
}

#[must_use]
pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

impl Camera {
    /// World position -> camera space, `+z` forward.
    #[must_use]
    pub fn to_view(self, world: [f32; 3]) -> [f32; 3] {
        let rel = sub(world, self.eye);
        [
            dot(rel, self.right),
            dot(rel, self.up),
            dot(rel, self.forward),
        ]
    }

    /// Camera-space point -> pixels, or `None` when it is behind the lens.
    ///
    /// `screen` is imgui's `display_size`, so the result is already in the coordinate space the
    /// draw list wants (origin top-left, `y` down).
    #[must_use]
    pub fn view_to_screen(&self, view: [f32; 3], screen: [f32; 2]) -> Option<[f32; 2]> {
        // Each guard screens NaN first and only then compares: a NaN reaching the compare
        // would pass a naive `<` and go on to produce NaN pixel coordinates, which imgui takes
        // as vertex positions.
        //
        // `< NEAR_EPSILON`, not `<=`: `project_segment` trims a crossing segment to exactly this
        // plane, and rejecting the point it just produced would drop the whole segment -- the
        // behaviour the trim exists to prevent.
        let depth = view[2];
        if !depth.is_finite() || depth < NEAR_EPSILON {
            return None;
        }
        let tan_half = (self.fov_y * 0.5).tan();
        if !tan_half.is_finite() || tan_half <= f32::EPSILON {
            return None;
        }
        // `ToPerspective` divides only the X term by the aspect ratio; an aspect of zero would be
        // a division by zero rather than a merely wrong picture, so it is screened here.
        if !self.aspect.is_finite() || self.aspect <= f32::EPSILON {
            return None;
        }
        let cot = 1.0 / tan_half;
        let ndc_x = view[0] / depth * cot / self.aspect;
        let ndc_y = view[1] / depth * cot;
        let point = [
            (ndc_x * 0.5 + 0.5) * screen[0],
            (0.5 - ndc_y * 0.5) * screen[1],
        ];
        if !point[0].is_finite() || !point[1].is_finite() {
            return None;
        }
        Some(point)
    }

    /// World position -> pixels.
    #[must_use]
    pub fn project(&self, world: [f32; 3], screen: [f32; 2]) -> Option<[f32; 2]> {
        self.view_to_screen(self.to_view(world), screen)
    }

    /// Project a world-space segment, trimming it at the near plane.
    ///
    /// A polyline drawn by projecting endpoints independently and skipping the ones that fail is
    /// not merely incomplete -- when one end is behind the camera the surviving end connects to
    /// whatever the next visible point is, so the path visibly jumps across the screen as the
    /// player turns. Trimming instead keeps the line ending where the world actually leaves view.
    #[must_use]
    pub fn project_segment(
        &self,
        from: [f32; 3],
        to: [f32; 3],
        screen: [f32; 2],
    ) -> Option<([f32; 2], [f32; 2])> {
        let (mut a, mut b) = (self.to_view(from), self.to_view(to));
        let (a_in, b_in) = (a[2] > NEAR_EPSILON, b[2] > NEAR_EPSILON);
        if !a_in && !b_in {
            return None;
        }
        if !a_in || !b_in {
            // Parameter along a->b where `z` crosses the near plane. The denominator cannot be
            // zero here: one endpoint is strictly above the plane and the other is not.
            let t = (NEAR_EPSILON - a[2]) / (b[2] - a[2]);
            let clipped = [
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                NEAR_EPSILON,
            ];
            if a_in { b = clipped } else { a = clipped }
        }
        Some((
            self.view_to_screen(a, screen)?,
            self.view_to_screen(b, screen)?,
        ))
    }
}

/// How close a target has to be for its path to be drawn at full strength, in metres.
pub const DEFAULT_BOLD_AT_METERS: f32 = 20.0;
/// The distance at which a path has faded to [`MIN_ALPHA`], in metres.
pub const DEFAULT_FAINT_AT_METERS: f32 = 150.0;
/// A fully faded path is still visible. Fading to nothing would make a distant player and a
/// player with no route look identical -- both would show no line at all.
pub const MIN_ALPHA: f32 = 0.22;
/// Stroke width of a fully faded path, in pixels.
pub const MIN_STROKE_PX: f32 = 1.6;
/// Stroke width of a path at [`DEFAULT_BOLD_AT_METERS`] or nearer, in pixels.
pub const MAX_STROKE_PX: f32 = 5.5;

/// `1.0` for a target at or inside `bold_at`, falling to `0.0` at `faint_at`.
///
/// Linear in distance rather than in squared distance: the eye reads a path's weight as a proxy
/// for "how far do I still have to run", and that is a distance, not its square.
#[must_use]
pub fn boldness(distance_meters: f32, bold_at: f32, faint_at: f32) -> f32 {
    if !distance_meters.is_finite() {
        return 0.0;
    }
    if distance_meters <= bold_at {
        return 1.0;
    }
    // A config that inverts or collapses the two thresholds must not produce NaN; treat anything
    // past the near threshold as fully faded. NaN is screened before the compare, not by negating
    // it, so the intent reads as written.
    if !faint_at.is_finite() || !bold_at.is_finite() || faint_at <= bold_at {
        return 0.0;
    }
    (1.0 - (distance_meters - bold_at) / (faint_at - bold_at)).clamp(0.0, 1.0)
}

/// Stroke width in pixels for a given [`boldness`].
#[must_use]
pub fn stroke_px(boldness: f32) -> f32 {
    MIN_STROKE_PX + (MAX_STROKE_PX - MIN_STROKE_PX) * boldness.clamp(0.0, 1.0)
}

/// Alpha for a given [`boldness`], never fully transparent.
#[must_use]
pub fn alpha(boldness: f32) -> f32 {
    MIN_ALPHA + (1.0 - MIN_ALPHA) * boldness.clamp(0.0, 1.0)
}

/// Distinct RGB for path `index`.
///
/// Hues are spread by the golden angle rather than by `index / total`, so a path keeps its colour
/// when another player joins or dies. Dividing the circle by `total` would recolour every path
/// the instant the roster changed, and the player's mental "the red one is the host" would break
/// mid-fight -- which is precisely when it is being relied on.
#[must_use]
pub fn path_color(index: usize) -> [f32; 3] {
    /// 360 / phi, the angle that fills the hue circle most evenly for any prefix length.
    const GOLDEN_ANGLE_DEGREES: f32 = 137.507_76;
    /// The first hue. 20 degrees is orange -- warm, and clear of the game's own blue-white UI.
    const FIRST_HUE_DEGREES: f32 = 20.0;
    let hue = (FIRST_HUE_DEGREES + GOLDEN_ANGLE_DEGREES * index as f32).rem_euclid(360.0);
    // High saturation reads as a colour even at 22% alpha over bright terrain; full value keeps
    // it legible against the game's dark ground.
    hsv_to_rgb(hue, 0.85, 1.0)
}

/// HSV (`h` in degrees, `s`/`v` in `0..=1`) -> RGB triple.
#[must_use]
pub fn hsv_to_rgb(hue_degrees: f32, saturation: f32, value: f32) -> [f32; 3] {
    let hue = hue_degrees.rem_euclid(360.0) / 60.0;
    let chroma = value * saturation;
    let second = chroma * (1.0 - ((hue % 2.0) - 1.0).abs());
    let (r, g, b) = match hue as u32 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let base = value - chroma;
    [r + base, g + base, b + base]
}

/// The world-space skeleton of the "no route" arrow: a shaft plus two barbs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arrow {
    pub tail: [f32; 3],
    pub tip: [f32; 3],
    pub left_barb: [f32; 3],
    pub right_barb: [f32; 3],
}

/// Build the arrow that leaves the player's body pointing at `target`.
///
/// The direction is the full 3D direction, not its horizontal projection: a host directly above
/// you in a tower and a host directly ahead of you are the difference between climbing and
/// running, and flattening the arrow would tell you they are the same. Returns `None` when the
/// two positions coincide and there is no direction to point.
#[must_use]
pub fn arrow(
    origin: [f32; 3],
    target: [f32; 3],
    length_meters: f32,
    up: [f32; 3],
) -> Option<Arrow> {
    /// Length of the head as a fraction of the shaft.
    const BARB_FRACTION: f32 = 0.28;
    /// How far each barb opens sideways, as a fraction of the head's length.
    const BARB_SPREAD: f32 = 0.6;
    let direction = normalize(sub(target, origin))?;
    let tip = add_scaled(origin, direction, length_meters);
    // A barb axis perpendicular to the shaft. `up` is the camera's up vector, so the head opens
    // towards the viewer and stays a visible arrowhead instead of collapsing to a line when the
    // shaft points straight at the camera.
    let side = normalize(cross(direction, up)).unwrap_or([0.0, 0.0, 0.0]);
    let back = add_scaled(tip, direction, -length_meters * BARB_FRACTION);
    let spread = length_meters * BARB_FRACTION * BARB_SPREAD;
    Some(Arrow {
        tail: origin,
        tip,
        left_barb: add_scaled(back, side, spread),
        right_barb: add_scaled(back, side, -spread),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A camera at the origin looking down `+z`, 90 degrees vertical, 16:9.
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

    const SCREEN: [f32; 2] = [1920.0, 1080.0];

    #[test]
    fn a_point_straight_ahead_lands_dead_centre() {
        let point = camera()
            .project([0.0, 0.0, 10.0], SCREEN)
            .expect("in front");
        assert!((point[0] - 960.0).abs() < 0.01, "x was {}", point[0]);
        assert!((point[1] - 540.0).abs() < 0.01, "y was {}", point[1]);
    }

    #[test]
    fn a_point_behind_the_camera_does_not_project() {
        assert!(camera().project([0.0, 0.0, -10.0], SCREEN).is_none());
        assert!(camera().project([0.0, 0.0, 0.0], SCREEN).is_none());
    }

    #[test]
    fn vertical_fov_reaches_the_screen_edge_before_horizontal() {
        // At 90 degrees vertical, y == z puts a point exactly on the top edge.
        let top = camera()
            .project([0.0, 10.0, 10.0], SCREEN)
            .expect("in front");
        assert!(top[1].abs() < 0.01, "top edge y was {}", top[1]);
        // The same offset horizontally must not reach the edge, because X is divided by aspect.
        let side = camera()
            .project([10.0, 0.0, 10.0], SCREEN)
            .expect("in front");
        let expected = 960.0 + 960.0 / (16.0 / 9.0);
        assert!(
            (side[0] - expected).abs() < 0.01,
            "x was {}; aspect is being applied to the wrong axis",
            side[0]
        );
    }

    #[test]
    fn a_segment_crossing_the_near_plane_is_trimmed_not_dropped() {
        let (near, far) = camera()
            .project_segment([0.0, 0.0, -5.0], [0.0, 0.0, 20.0], SCREEN)
            .expect("partially visible");
        // Both survive, and the trimmed end sits at screen centre because the segment runs
        // straight down the view axis.
        assert!((near[0] - 960.0).abs() < 0.01);
        assert!((far[0] - 960.0).abs() < 0.01);
    }

    #[test]
    fn a_segment_entirely_behind_the_camera_is_dropped() {
        assert!(
            camera()
                .project_segment([0.0, 0.0, -5.0], [0.0, 0.0, -20.0], SCREEN)
                .is_none()
        );
    }

    #[test]
    fn boldness_is_full_up_close_and_zero_at_the_far_threshold() {
        assert_eq!(boldness(0.0, 20.0, 150.0), 1.0);
        assert_eq!(boldness(20.0, 20.0, 150.0), 1.0);
        assert_eq!(boldness(150.0, 20.0, 150.0), 0.0);
        assert_eq!(boldness(1000.0, 20.0, 150.0), 0.0);
        let middle = boldness(85.0, 20.0, 150.0);
        assert!((middle - 0.5).abs() < 0.01, "midpoint was {middle}");
    }

    #[test]
    fn inverted_thresholds_do_not_produce_nan() {
        // With `bold_at` past `faint_at` the ramp has no width, so everything is either inside
        // the near threshold (full) or past it (nothing). Neither may be NaN: a NaN alpha reaches
        // imgui as a vertex colour and paints garbage.
        for distance in [0.0_f32, 50.0, 150.0, 200.0, 10_000.0] {
            let value = boldness(distance, 150.0, 20.0);
            assert!(value.is_finite(), "boldness({distance}) was {value}");
            assert!(
                (0.0..=1.0).contains(&value),
                "boldness({distance}) was {value}"
            );
        }
        assert_eq!(boldness(200.0, 150.0, 20.0), 0.0);
        assert!(boldness(f32::NAN, 20.0, 150.0).is_finite());
    }

    #[test]
    fn a_closer_path_is_drawn_thicker_and_more_opaque() {
        let near = boldness(5.0, 20.0, 150.0);
        let far = boldness(140.0, 20.0, 150.0);
        assert!(stroke_px(near) > stroke_px(far));
        assert!(alpha(near) > alpha(far));
        assert!(alpha(far) >= MIN_ALPHA, "a far path must stay visible");
    }

    #[test]
    fn each_path_gets_a_distinguishable_colour_that_does_not_move() {
        let first = path_color(0);
        // Adding a fourth player must not recolour the first three.
        assert_eq!(first, path_color(0));
        for index in 1..6 {
            let other = path_color(index);
            let distance: f32 = (0..3).map(|c| (first[c] - other[c]).powi(2)).sum();
            assert!(
                distance > 0.05,
                "colour {index} is too close to colour 0: {other:?}"
            );
        }
    }

    /// Markers must be evenly spaced, not one per navmesh waypoint: the engine emits waypoints
    /// where the mesh turns, so a doorway gets six inside two metres and open ground gets none
    /// for forty.
    #[test]
    fn markers_land_at_an_even_spacing_along_a_straight_run() {
        let route = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]];
        let points = resample(&route, 2.5, 64);
        assert_eq!(points.len(), 5);
        for (index, point) in points.iter().enumerate() {
            assert!((point[0] - index as f32 * 2.5).abs() < 1e-3, "{point:?}");
        }
    }

    /// Spacing is measured along the route, so a corner does not swallow a marker.
    #[test]
    fn spacing_carries_across_a_corner_rather_than_restarting() {
        let route = [[0.0, 0.0, 0.0], [3.0, 0.0, 0.0], [3.0, 0.0, 3.0]];
        let points = resample(&route, 2.0, 64);
        for pair in points.windows(2) {
            let step = length(sub(pair[1], pair[0]));
            assert!(step <= 2.0 + 1e-3, "markers {step} apart");
        }
        assert!(points.len() >= 3);
    }

    /// The last marker is the one that says "here". Dropping it because the route length was not
    /// a multiple of the spacing is the omission a player would actually notice.
    #[test]
    fn the_destination_always_gets_a_marker() {
        let route = [[0.0, 0.0, 0.0], [7.0, 0.0, 0.0]];
        let points = resample(&route, 3.0, 64);
        let last = points.last().copied().expect("at least one");
        assert!(length(sub(last, [7.0, 0.0, 0.0])) < 1e-3, "{last:?}");
    }

    /// ...but not stacked on top of a marker that is already there.
    #[test]
    fn a_destination_on_the_grid_is_not_marked_twice() {
        let route = [[0.0, 0.0, 0.0], [6.0, 0.0, 0.0]];
        let points = resample(&route, 3.0, 64);
        assert_eq!(points.len(), 3);
    }

    /// A long route truncates rather than thinning, so the markers nearest you -- the ones you
    /// are about to walk past -- keep their spacing.
    #[test]
    fn a_long_route_is_capped_from_the_far_end() {
        let route = [[0.0, 0.0, 0.0], [1000.0, 0.0, 0.0]];
        let points = resample(&route, 1.0, 8);
        assert_eq!(points.len(), 8);
        assert!((points[7][0] - 7.0).abs() < 1e-3);
    }

    /// Degenerate input must not spin: a zero-length route, a zero spacing, a NaN.
    #[test]
    fn degenerate_routes_terminate_and_return_something_sane() {
        assert!(resample(&[], 1.0, 8).is_empty());
        assert_eq!(resample(&[[1.0, 2.0, 3.0]], 1.0, 8).len(), 1);
        let repeated = [[0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]];
        assert!(resample(&repeated, 1.0, 8).len() <= 2);
        assert_eq!(resample(&[[0.0; 3], [5.0, 0.0, 0.0]], 0.0, 8).len(), 2);
        assert_eq!(resample(&[[0.0; 3], [5.0, 0.0, 0.0]], f32::NAN, 8).len(), 2);
    }

    #[test]
    fn hsv_hits_the_primaries() {
        let red = hsv_to_rgb(0.0, 1.0, 1.0);
        assert!((red[0] - 1.0).abs() < 0.001 && red[1] < 0.001 && red[2] < 0.001);
        let green = hsv_to_rgb(120.0, 1.0, 1.0);
        assert!(green[0] < 0.001 && (green[1] - 1.0).abs() < 0.001 && green[2] < 0.001);
        let blue = hsv_to_rgb(240.0, 1.0, 1.0);
        assert!(blue[0] < 0.001 && blue[1] < 0.001 && (blue[2] - 1.0).abs() < 0.001);
    }

    #[test]
    fn the_arrow_points_at_the_target_and_keeps_its_height_difference() {
        let arrow =
            arrow([0.0, 0.0, 0.0], [0.0, 30.0, 30.0], 4.0, [0.0, 1.0, 0.0]).expect("distinct");
        let direction = normalize(sub(arrow.tip, arrow.tail)).expect("a direction");
        // Straight up-and-forward at 45 degrees: the arrow must climb, not lie flat.
        assert!(
            direction[1] > 0.6,
            "arrow flattened; y component was {}",
            direction[1]
        );
        assert!((length(sub(arrow.tip, arrow.tail)) - 4.0).abs() < 0.001);
    }

    #[test]
    fn an_arrow_to_your_own_position_has_no_direction() {
        assert!(arrow([1.0, 2.0, 3.0], [1.0, 2.0, 3.0], 4.0, [0.0, 1.0, 0.0]).is_none());
    }

    #[test]
    fn the_arrow_head_is_behind_the_tip_and_opens_to_both_sides() {
        let arrow =
            arrow([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], 5.0, [0.0, 1.0, 0.0]).expect("a direction");
        assert!(
            arrow.left_barb[0] < arrow.tip[0],
            "barbs must trail the tip"
        );
        assert!(arrow.right_barb[0] < arrow.tip[0]);
        let spread = length(sub(arrow.left_barb, arrow.right_barb));
        assert!(spread > 0.5, "the head collapsed; spread was {spread}");
    }
}
