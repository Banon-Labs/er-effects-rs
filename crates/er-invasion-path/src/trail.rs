//! The run of world markers laid along a route, and the rule for when it is a new run.
//!
//! Ungated so `cargo test` compiles it: the pile-up this exists to prevent was a logic bug, not a
//! game-memory one, and a windows-gated module is never built by the host test run.

use crate::geometry;

/// One placed marker: where it is, and (on Windows) the handle that removes it.
///
/// The handle is `#[cfg(windows)]` and the position is not, so the pruning arithmetic -- which is
/// the part that can be wrong without crashing anything -- stays host-testable.
#[derive(Default)]
pub(crate) struct Placed {
    pub(crate) at: [f32; 3],
    #[cfg(windows)]
    pub(crate) handle: Option<crate::sfx::Marker>,
}

/// A trail of world markers, laid a few at a time.
///
/// # Why it is laid incrementally
///
/// The first version spawned a whole route's worth of markers in one pass and repeated that every
/// couple of seconds. Two things went wrong, both visible in the first live run: the markers piled
/// up, because each pass laid a fresh set on top of the last, and the whole trail appeared at once
/// for a route the player had already started walking away from.
///
/// A trail is now laid a few markers per pass, from your feet outwards, and it stops the moment
/// the route changes -- the far end of a route to somebody who is moving was never going to be
/// where they are by the time you got there.
#[derive(Default)]
pub(crate) struct Trail {
    /// Every marker position for the current route, in order from the player outwards.
    spots: Vec<[f32; 3]>,
    /// How many of them have been spawned. Laying stops when this reaches `spots.len()`.
    laid: usize,
    /// The markers currently in the world, in the order they were laid.
    placed: Vec<Placed>,
}

impl Trail {
    /// Point this trail at a new route, if it is actually a different one.
    ///
    /// Returns whether the trail was restarted. An unchanged route must not restart: the markers
    /// for it are already in the world, and re-laying them is precisely what made them accumulate.
    pub(crate) fn retarget(&mut self, spots: Vec<[f32; 3]>) -> bool {
        if same_route(&self.spots, &spots) {
            return false;
        }
        self.spots = spots;
        self.laid = 0;
        true
    }

    /// How many markers this trail is holding.
    pub(crate) fn placed_count(&self) -> usize {
        self.placed.len()
    }

    /// Which placed markers are far enough behind `player` to be clutter.
    ///
    /// "Behind" is by trail order, not by angle: the markers were laid from the player outwards,
    /// so anything before the nearest one is ground already covered. Using distance alone would
    /// tear down the far end of a route that doubles back past you -- which a corkscrew does
    /// constantly, and those are the stones you most need to see.
    pub(crate) fn stale_prefix(&self, player: [f32; 3], keep_behind: f32) -> usize {
        let Some(nearest) = self
            .placed
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let (a, b) = (
                    geometry::length(geometry::sub(a.at, player)),
                    geometry::length(geometry::sub(b.at, player)),
                );
                a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(index, _)| index)
        else {
            return 0;
        };
        // Everything before the nearest marker is behind you, minus a few kept so the trail does
        // not visibly end at your feet.
        let keep = if keep_behind.is_finite() && keep_behind > 0.0 {
            let spacing = self.spacing().max(f32::EPSILON);
            (keep_behind / spacing).ceil() as usize
        } else {
            0
        };
        nearest.saturating_sub(keep)
    }

    /// Distance between consecutive markers, measured from the trail itself.
    fn spacing(&self) -> f32 {
        self.spots
            .windows(2)
            .map(|pair| geometry::length(geometry::sub(pair[1], pair[0])))
            .find(|step| *step > f32::EPSILON)
            .unwrap_or(1.0)
    }

    /// The half-open range of spots the next pass should lay.
    ///
    /// Pure, and separate from the laying itself, so the batching is host-testable while the
    /// spawn that consumes it is not.
    pub(crate) fn batch_bounds(&self, batch: usize) -> (usize, usize) {
        let from = self.laid;
        (from, (from + batch).min(self.spots.len()))
    }

    pub(crate) fn finished(&self) -> bool {
        self.laid >= self.spots.len()
    }
}

#[cfg(windows)]
impl Trail {
    /// Spawn the next few markers, keeping each handle so it can be removed again.
    ///
    /// # Safety
    ///
    /// Must be called on the game thread.
    pub(crate) unsafe fn lay_next(
        &mut self,
        fxr_id: u32,
        variant: crate::sfx::SpawnVariant,
        batch: usize,
    ) -> usize {
        let (from, to) = self.batch_bounds(batch);
        let mut placed = 0;
        for index in from..to {
            let at = self.spots[index];
            // SAFETY: game thread, as this function's own contract requires.
            let Some(handle) = (unsafe { crate::sfx::spawn_tracked(fxr_id, at, variant) }) else {
                // The SFX manager is not up. Leave the cursor where it is and try again next
                // pass rather than marking these positions laid when nothing was.
                break;
            };
            self.placed.push(Placed {
                at,
                handle: Some(handle),
            });
            placed += 1;
        }
        self.laid = from + placed;
        placed
    }

    /// Remove every marker this trail placed. Returns how many went.
    ///
    /// # Safety
    ///
    /// Must be called on the game thread.
    pub(crate) unsafe fn clear_placed(&mut self) -> usize {
        let mut removed = 0;
        for mut placed in std::mem::take(&mut self.placed) {
            if let Some(handle) = placed.handle.take() {
                // SAFETY: game thread; this handle came from `crate::sfx::spawn_tracked`.
                unsafe { crate::sfx::despawn(handle) };
                removed += 1;
            }
        }
        removed
    }

    /// Remove the markers you have already walked past. Returns how many went.
    ///
    /// # Safety
    ///
    /// Must be called on the game thread.
    pub(crate) unsafe fn prune_behind(&mut self, player: [f32; 3], keep_behind: f32) -> usize {
        let stale = self.stale_prefix(player, keep_behind);
        if stale == 0 {
            return 0;
        }
        let mut removed = 0;
        for mut placed in self.placed.drain(..stale).collect::<Vec<_>>() {
            if let Some(handle) = placed.handle.take() {
                // SAFETY: game thread; this handle came from `crate::sfx::spawn_tracked`.
                unsafe { crate::sfx::despawn(handle) };
                removed += 1;
            }
        }
        removed
    }
}

/// Are these two marker runs the same trail?
///
/// Compared position by position rather than by route identity, because the navmesh returns a
/// fresh answer every refresh and two answers to the same question differ in the last decimal
/// place without differing anywhere a player could see. Treating those as a new trail is what
/// re-laid markers on top of themselves.
fn same_route(previous: &[[f32; 3]], next: &[[f32; 3]]) -> bool {
    /// How far a marker may move before the trail counts as a different one.
    ///
    /// Was 1.0 m, which counted an ordinary re-plan as a new trail and tore down a perfectly good
    /// set of stones to lay a near-identical one. A target walking about produces routes that
    /// wobble by several metres without going anywhere different, and those must not restart the
    /// trail. Below the marker spacing, so a genuinely different route still does.
    const MOVED_METERS: f32 = 5.0;
    previous.len() == next.len()
        && previous
            .iter()
            .zip(next)
            .all(|(a, b)| geometry::length(geometry::sub(*a, *b)) <= MOVED_METERS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(count: usize) -> Vec<[f32; 3]> {
        (0..count).map(|i| [i as f32 * 4.0, 0.0, 0.0]).collect()
    }

    /// The bug from the first live run: every pass laid a fresh set of markers over the last, so
    /// they accumulated until the trail was a wall.
    #[test]
    fn an_unchanged_route_does_not_lay_a_second_trail_over_the_first() {
        let mut trail = Trail::default();
        assert!(trail.retarget(route(6)));
        trail.laid = trail.spots.len();
        assert!(
            !trail.retarget(route(6)),
            "an unchanged route must not restart"
        );
        assert!(trail.finished(), "and must not lay another marker");
    }

    /// Navmesh answers to the same question differ in the last decimal without differing anywhere
    /// a player could see. Those must not count as a new trail either.
    #[test]
    fn navmesh_jitter_is_not_a_new_route() {
        let mut trail = Trail::default();
        trail.retarget(route(4));
        let jittered: Vec<[f32; 3]> = route(4)
            .into_iter()
            .map(|p| [p[0] + 0.01, p[1] - 0.02, p[2] + 0.005])
            .collect();
        assert!(!trail.retarget(jittered));
    }

    /// A route that genuinely moved restarts, and restarting stops the old laying immediately --
    /// which is the point: the far end of a route to somebody who is moving is already wrong.
    #[test]
    fn a_moved_route_restarts_and_abandons_the_rest_of_the_old_one() {
        let mut trail = Trail::default();
        trail.retarget(route(10));
        trail.laid = 3;
        let moved: Vec<[f32; 3]> = route(10)
            .into_iter()
            .map(|p| [p[0], p[1], p[2] + 25.0])
            .collect();
        assert!(trail.retarget(moved));
        assert_eq!(trail.laid, 0, "laying restarts from the player's feet");
    }

    /// A different number of markers is a different route even if the shared prefix matches.
    #[test]
    fn a_shorter_or_longer_route_is_a_new_route() {
        let mut trail = Trail::default();
        trail.retarget(route(8));
        assert!(trail.retarget(route(5)));
    }

    #[test]
    fn markers_are_laid_a_few_at_a_time_from_the_players_feet_outwards() {
        let mut trail = Trail::default();
        trail.retarget(route(7));
        assert_eq!(trail.batch_bounds(3), (0, 3));
        trail.laid = 3;
        assert_eq!(trail.batch_bounds(3), (3, 6));
        trail.laid = 6;
        // The tail is short, and asking for more than remains must not over-read.
        assert_eq!(trail.batch_bounds(3), (6, 7));
        trail.laid = 7;
        assert!(trail.finished());
        assert_eq!(trail.batch_bounds(3), (7, 7));
    }

    /// Markers you have walked past are clutter. "Behind" is decided by trail order, not by
    /// distance: a route that doubles back past you -- which a corkscrew does on every turn --
    /// would otherwise have its far end torn down as "near", and that is the half you need.
    #[test]
    fn markers_before_the_nearest_one_count_as_behind_you() {
        let mut trail = Trail::default();
        trail.retarget(route(10));
        for index in 0..10 {
            trail.placed.push(Placed {
                at: [index as f32 * 4.0, 0.0, 0.0],
                #[cfg(windows)]
                handle: None,
            });
        }
        // Standing at the 5th marker, keeping nothing behind.
        assert_eq!(trail.stale_prefix([20.0, 0.0, 0.0], 0.0), 5);
    }

    #[test]
    fn a_few_markers_are_kept_behind_so_the_trail_does_not_end_at_your_feet() {
        let mut trail = Trail::default();
        trail.retarget(route(10));
        for index in 0..10 {
            trail.placed.push(Placed {
                at: [index as f32 * 4.0, 0.0, 0.0],
                #[cfg(windows)]
                handle: None,
            });
        }
        // 8 m of keep-behind at 4 m spacing is two markers, so 5 - 2 = 3 go.
        assert_eq!(trail.stale_prefix([20.0, 0.0, 0.0], 8.0), 3);
    }

    /// A corkscrew passes near its own earlier turns. Order, not proximity, must decide.
    #[test]
    fn a_route_that_doubles_back_does_not_lose_its_far_end() {
        let mut trail = Trail::default();
        // Out along X, then back to near the start: the last marker is closest to the player.
        let spiral = [
            [0.0, 0.0, 0.0],
            [4.0, 0.0, 0.0],
            [8.0, 0.0, 0.0],
            [8.0, -4.0, 4.0],
            [1.0, -8.0, 1.0],
        ];
        trail.retarget(spiral.to_vec());
        for at in spiral {
            trail.placed.push(Placed {
                at,
                #[cfg(windows)]
                handle: None,
            });
        }
        // The player is at the start. The nearest marker is index 0, so nothing is behind.
        assert_eq!(trail.stale_prefix([0.0, 0.0, 0.0], 0.0), 0);
        assert_eq!(trail.placed_count(), 5, "the descent survives");
    }

    #[test]
    fn a_trail_with_nothing_placed_has_nothing_behind_you() {
        let trail = Trail::default();
        assert_eq!(trail.stale_prefix([0.0, 0.0, 0.0], 4.0), 0);
        assert_eq!(trail.placed_count(), 0);
    }

    #[test]
    fn an_empty_route_is_finished_immediately() {
        let trail = Trail::default();
        assert!(trail.finished());
        assert_eq!(trail.batch_bounds(4), (0, 0));
    }
}
