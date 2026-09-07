//! Drawing the creature list.
//!
//! The present hook is not here. This module used to own the host-join as well, back when the
//! picker was the only thing this DLL drew; that half moved to [`crate::overlay`] when the
//! attack-set panel arrived, because two surfaces must share one `Present` hook and neither of
//! them should own the other's install. What is left here is the panel itself: given a live imgui
//! frame, paint the list if it is open.
//!
//! # `PANEL_DRAWS` and the reading it exists to prevent
//!
//! [`draw`] is called on every frame and returns immediately when the list is closed, which is
//! almost all of them. The counter therefore has to be incremented after that early return, or it
//! measures the swapchain rather than the picker -- which is exactly the confusion that made a
//! working picker look broken for a day. See the module docs of [`crate::overlay`] for the log
//! evidence.

#![cfg(windows)]

use std::sync::atomic::{AtomicUsize, Ordering};

use hudhook::imgui::{Condition, StyleColor, Ui};

use crate::overlay::FONT_SCALE;
use crate::picker::View;
use crate::picker::catalog::LABEL_MAX_CHARS;

/// Frames the list itself was built on -- not frames the overlay drew. Zero while the picker has
/// been open is the fault worth chasing; zero while it is closed is the picker being closed.
static PANEL_DRAWS: AtomicUsize = AtomicUsize::new(0);
/// Rows painted on the most recent draw.
static LAST_ROWS: AtomicUsize = AtomicUsize::new(0);

/// Where the panel sits, and how wide. First-use only -- the window is movable, so a player who
/// drags it keeps it there.
const PANEL_POSITION: [f32; 2] = [48.0, 96.0];
/// Widened by the same factor the font is scaled by. The width is a fixed number of pixels rather
/// than a text measure, so scaling the font without scaling this would clip every name at the same
/// place it used to fit -- the failure would look like truncated data rather than like a layout
/// constant.
const PANEL_WIDTH: f32 = 420.0 * FONT_SCALE;

/// The stable half of the window label. imgui takes everything after `###` as the window's ID and
/// draws none of it, which is what lets the visible half carry a changing cursor position without
/// making a new window on every keypress.
const PANEL_ID: &str = "###er-npc-possess-picker";

pub(crate) fn panel_draws() -> usize {
    PANEL_DRAWS.load(Ordering::Relaxed)
}

pub(crate) fn last_rows() -> usize {
    LAST_ROWS.load(Ordering::Relaxed)
}

/// Draw the current view onto a live imgui frame. A no-op while the list is closed.
pub(crate) fn draw(ui: &Ui) {
    // Lock-free fast path, and this is the common one: once the overlay is installed this runs on
    // every `Present` for the rest of the process, and the list is closed for almost all of them.
    // Taking the picker mutex 60-144 times a second to be told "closed" would contend with the
    // game thread's own per-frame tick for nothing.
    if !crate::picker::is_drawing() {
        LAST_ROWS.store(0, Ordering::Relaxed);
        return;
    }
    let Some(view) = crate::picker::view() else {
        LAST_ROWS.store(0, Ordering::Relaxed);
        return;
    };
    PANEL_DRAWS.fetch_add(1, Ordering::Relaxed);
    LAST_ROWS.store(view.rows.len(), Ordering::Relaxed);
    // `###` pins the window ID, and it is not decoration. imgui derives a window's identity from
    // its label, and this label carries the cursor position -- so without the suffix every step
    // of the list would be a brand new window: the panel would snap back to its default place and
    // size on every keypress, and imgui would accumulate a fresh saved state for each of the 408
    // titles. Everything after `###` is the id and is never drawn.
    let title = format!(
        "possess: pick a creature  {}/{}{PANEL_ID}",
        view.position, view.total
    );
    ui.window(title)
        .position(PANEL_POSITION, Condition::FirstUseEver)
        .size([PANEL_WIDTH, 0.0], Condition::Always)
        .collapsible(false)
        .resizable(false)
        .build(|| {
            // Scoped to this window: `set_window_font_scale` applies to the window imgui is
            // currently building, so it cannot leak into another shell's overlay drawing on the
            // same frame through the shared host.
            ui.set_window_font_scale(FONT_SCALE);
            draw_rows(ui, &view);
        });
}

fn draw_rows(ui: &Ui, view: &View) {
    for row in &view.rows {
        let creature = &row.creature;
        // The id is always shown beside the name. A player who already knows they want `c4500`
        // should not have to know it is called Flying Dragon, and the id is what goes in the
        // config file.
        let line = format!(
            "{}{:<width$} c{:04}  {}",
            if row.selected { "> " } else { "  " },
            creature.clipped_label(),
            creature.chr_id,
            creature.shape(),
            width = LABEL_MAX_CHARS,
        );
        if row.selected {
            let tint = ui.push_style_color(StyleColor::Text, [1.0, 0.85, 0.35, 1.0]);
            ui.text(&line);
            tint.pop();
        } else if creature.is_mute() {
            // Dimmed rather than hidden: becoming one gets you a body that cannot attack, and
            // that is worth seeing in the list rather than after the possession.
            let tint = ui.push_style_color(StyleColor::Text, [0.55, 0.55, 0.55, 1.0]);
            ui.text(&line);
            tint.pop();
        } else {
            ui.text(&line);
        }
    }
    ui.separator();
    match &view.selected {
        // A mute creature in the shipped table has zero moves and zero denials -- it is a variant
        // that owns a model but declares no animations of its own, so nothing was classified
        // rather than everything being withheld. Saying "0 animations were considered and all
        // withheld" was both self-contradictory and backwards.
        Some(creature) if creature.is_mute() => ui.text(format!(
            "c{:04} has no fireable move -- this variant declares no animations of its own",
            creature.chr_id
        )),
        Some(creature) => ui.text(format!(
            "c{:04}: {} moves, {} withheld  |  light {} heavy {} ranged {} movement {}",
            creature.chr_id,
            creature.moves,
            creature.denials,
            creature.buckets[0],
            creature.buckets[1],
            creature.buckets[2],
            creature.buckets[3],
        )),
        None => ui.text("no creatures -- the shipped moveset table is empty"),
    }
    ui.text("press your POSSESS hotkey to choose, the picker hotkey to close");
}
