//! What the game thread hands to the render thread, and the colour bookkeeping behind it.
//!
//! Pure data and pure logic, so the part that decides *which player is which colour* is proven by
//! `cargo test` rather than by squinting at two similar oranges in a screenshot.

// Windows-only in practice; ungated so the assignment logic below stays host-testable.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::geometry::{self, Arrow};

/// What to draw for one player.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RouteShape {
    /// A walkable route: world-space points, already lifted clear of the ground.
    Walk(Vec<[f32; 3]>),
    /// No walkable route exists, so an arrow leaves the player's body pointing at the target.
    Arrow(Arrow),
}

/// One player's overlay, ready to project.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Route {
    pub(crate) shape: RouteShape,
    pub(crate) color: [f32; 3],
    pub(crate) alpha: f32,
    pub(crate) stroke_px: f32,
}

impl Route {
    /// Build a route with its weight derived from how far away the target is.
    pub(crate) fn new(
        shape: RouteShape,
        color_slot: usize,
        distance_meters: f32,
        bold_at: f32,
        faint_at: f32,
    ) -> Self {
        let boldness = geometry::boldness(distance_meters, bold_at, faint_at);
        Self {
            shape,
            color: geometry::path_color(color_slot),
            alpha: geometry::alpha(boldness),
            stroke_px: geometry::stroke_px(boldness),
        }
    }
}

/// The whole overlay for one frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Snapshot {
    pub(crate) routes: Vec<Route>,
}

/// Hands each player a colour slot and keeps it for as long as they are in the session.
///
/// # Why this is not just an enumeration index
///
/// Colouring by position in the roster means the roster's order decides the colour, and the
/// roster is sorted by distance -- so two players swapping places as they run would swap colours
/// mid-fight. The whole point of "N players, N colours" is that a colour identifies a person, so
/// a slot is bound to a `FieldInsHandle` and released only when that player is gone.
#[derive(Debug, Default)]
pub(crate) struct Palette {
    /// `slots[i]` is the handle currently holding colour `i`, or `None` when free.
    slots: Vec<Option<u64>>,
}

impl Palette {
    /// The colour slot for `handle`, allocating the lowest free one on first sight.
    pub(crate) fn slot_for(&mut self, handle: u64) -> usize {
        if let Some(index) = self.slots.iter().position(|held| *held == Some(handle)) {
            return index;
        }
        if let Some(index) = self.slots.iter().position(Option::is_none) {
            self.slots[index] = Some(handle);
            return index;
        }
        self.slots.push(Some(handle));
        self.slots.len() - 1
    }

    /// Release every slot whose player is no longer in `present`.
    ///
    /// Called once per roster read. Without it a session that churned through phantoms would keep
    /// allocating new hues until the colours started repeating.
    pub(crate) fn retain(&mut self, present: &[u64]) {
        for slot in &mut self.slots {
            if let Some(handle) = *slot
                && !present.contains(&handle)
            {
                *slot = None;
            }
        }
        // Trailing free slots are dropped so the next player gets a low, well-separated hue
        // rather than the next index past a long-dead one.
        while self.slots.last().is_some_and(Option::is_none) {
            self.slots.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_player_keeps_their_colour_across_frames() {
        let mut palette = Palette::default();
        let first = palette.slot_for(0xaaaa);
        let second = palette.slot_for(0xbbbb);
        assert_ne!(first, second);
        // Re-reading the roster in the other order must not swap them.
        assert_eq!(palette.slot_for(0xbbbb), second);
        assert_eq!(palette.slot_for(0xaaaa), first);
    }

    #[test]
    fn distance_order_does_not_decide_colour() {
        let mut palette = Palette::default();
        let host = palette.slot_for(0x1111);
        let phantom = palette.slot_for(0x2222);
        // The host runs past the phantom; the roster order flips, the colours must not.
        palette.retain(&[0x2222, 0x1111]);
        assert_eq!(palette.slot_for(0x2222), phantom);
        assert_eq!(palette.slot_for(0x1111), host);
    }

    #[test]
    fn a_departed_player_frees_their_colour_for_the_next_arrival() {
        let mut palette = Palette::default();
        let first = palette.slot_for(0xaaaa);
        let second = palette.slot_for(0xbbbb);
        palette.retain(&[0xbbbb]);
        // The newcomer takes the freed low slot rather than a third hue.
        assert_eq!(palette.slot_for(0xcccc), first);
        assert_eq!(palette.slot_for(0xbbbb), second);
    }

    #[test]
    fn an_emptied_session_does_not_grow_the_palette_forever() {
        let mut palette = Palette::default();
        for handle in 0..32u64 {
            palette.slot_for(handle);
        }
        palette.retain(&[]);
        assert_eq!(palette.slot_for(0xffff), 0, "slots leaked across a session");
    }

    #[test]
    fn a_nearer_route_is_bolder_than_a_distant_one() {
        let near = Route::new(RouteShape::Walk(vec![[0.0; 3]]), 0, 5.0, 20.0, 150.0);
        let far = Route::new(RouteShape::Walk(vec![[0.0; 3]]), 0, 140.0, 20.0, 150.0);
        assert!(near.stroke_px > far.stroke_px);
        assert!(near.alpha > far.alpha);
        assert_eq!(near.color, far.color, "weight must not change the hue");
    }
}
