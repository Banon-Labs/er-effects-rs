//! The two predicates that answer "does the product's loading cover own the screen right now",
//! and the title force-hide question that follows from it.
//!
//! Split out of `counters.rs` as a pure code move: both read only the `BOOT_VIEW_*` /
//! `SYSTEM_QUIT_*` latches defined in the parent module, and both are re-exported from
//! `er_telemetry_core::counters`, so every call site is unchanged.

use std::sync::atomic::Ordering;

use super::{
    BOOT_VIEW_DRAW_HITS, BOOT_VIEW_DRAW_HITS_ARM_BASELINE, BOOT_VIEW_OWN_MENU_LOAD_ACTIVE,
    BOOT_VIEW_RELEASE_READY_MS, BOOT_VIEW_STOP_LOAD_WITNESS, BOOT_VIEW_STOPPED,
    SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT,
};

/// Is the native loading screen currently on screen one the product's loading cover owns?
///
/// # Why this question has to be asked at all
///
/// The cover is not a replacement for every vanilla loading screen. It covers the dead early-boot
/// gap, and it covers a System->Quit -> Load Character switch, which re-arms it through
/// `rearm_boot_progress_for_own_menu_load`. Everything else the engine puts a `CS::LoadingScreen`
/// up for -- a world-map fast travel, a death respawn, a legacy-dungeon transition -- is the game's
/// own screen and always has been.
///
/// Without this distinction `NATIVE_LS_EXPOSURE_FRAMES` files those screens as the vanilla
/// flash-through defect. Measured 2026-08-30, run `dll:d37d919a`: a fast travel at 589 409 ms
/// (`02_120_WorldMap` -> confirm `01_010_MessageBox` -> `warp_requested=true` -> `02_903_NowLoading2`)
/// produced 331 exposure frames filed as `gate=4 (cover-stopped-or-nothing-to-draw)` in a session
/// with zero character reloads (`oracle_current_load_epoch = 0`, no rearm line in 6.8 M lines).
/// Read literally that says the cover failed on a second load; there was no second load.
///
/// # Why the answer must not come from the loading screen itself
///
/// The tempting signal -- "a native loading screen is up, so re-arm" -- is the one that must never
/// be used, and the release predicate is why. On a fast travel the player never leaves, so
/// `boot_view_player_loaded()` is already true, and the native bar still reaches 998; a cover armed
/// on that signal is releasable on its first frame (a cover flash over gameplay), and with
/// `BOOT_VIEW_RELEASE_REQUIRE_CONFIRM` set it is releasable never, because the fresh-deser count it
/// waits on only bumps when a character deserializes. That second case is an opaque full-screen
/// cover over live gameplay until `BOOT_VIEW_BACKSTOP_LIFETIME_MS`.
///
/// # The three ways the cover can own the screen, and why they cannot hide a real hole
///
/// `SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT` is documented as the authoritative total-load witness
/// -- exactly one increment per forwarded `continue_confirm`, boot included -- and a warp does not
/// forward one. It bumps at the confirm, i.e. before the character load's screen appears, so the
/// er-effects-rs-q6vk shape (a switch whose cover released early during the return-to-title
/// teardown, leaving the character load bare) still answers true here and stays filed as gate 4.
pub fn cover_owns_current_loading_screen() -> bool {
    // Armed: either the boot window, or a switch that re-armed it. `BOOT_VIEW_STOPPED` is cleared
    // by `boot_view_reset_cover_window`, so this is live state and not a one-shot.
    if BOOT_VIEW_STOPPED.load(Ordering::SeqCst) == 0 {
        return true;
    }
    // A System->Quit switch is in flight. Cleared at the stop, so it covers only the armed span --
    // the delta below is what carries the case where the cover stopped too early.
    if BOOT_VIEW_OWN_MENU_LOAD_ACTIVE.load(Ordering::SeqCst) != 0 {
        return true;
    }
    // A world load was requested after the cover last let go.
    SYSTEM_QUIT_CONTINUE_CONFIRM_ALLOW_COUNT.load(Ordering::SeqCst)
        != BOOT_VIEW_STOP_LOAD_WITNESS.load(Ordering::SeqCst)
}

/// Should the title's own visuals (`TitleBackViewParts` / `05_001_Title_Logo`, `PressStart`, the
/// title text surfaces) still be forced hidden?
///
/// Every one of those force-hide detours exists for one reason: while the product cover owns the
/// screen, the vanilla title underneath must not flash through. None of them has any purpose once
/// the cover is not drawing -- and left latched they are strictly harmful, because suppressing the
/// title with nothing composited over it renders a black screen with no affordance to leave it.
///
/// The bug this PREDICATE exists to close (2026-09-04, Load Character from File).
///
/// The release used to be spelled `BOOT_VIEW_RELEASE_READY_MS != 0` at one of the three sites and
/// not spelled at all at the other two. But that latch is only set by the cover's confirm-gated
/// semantic release. The cover has a second ending -- the composite-time cap
/// (`BOOT_VIEW_STOP_REASON_FPS_BAIL`) and the absolute backstop -- which latches `BOOT_VIEW_STOPPED`
/// and stamps `BOOT_VIEW_STOP_MS` while leaving `RELEASE_READY_MS` at 0 forever.
///
/// Measured on the black-screen run: `oracle_boot_view_stop_reason=2` at `stop_ms=2042119`,
/// `release_ready_ms=0`, `release_held_for_confirm=285`, `boot_view_draw_after_stop=0`. The world
/// was then torn down and the title rebuilt into a process still answering the game's
/// `SetVisible(logo, 1)` with a 0: `title_logo_gfx_visibility=false`,
/// `title_press_start_gfx_any_hidden=true`. Nothing drawing, no logo, no press button.
///
/// So the condition is "the cover is still drawing", by either ending. `BOOT_VIEW_STOPPED` is that
/// latch and `boot_view_reset_cover_window` clears it on every rearm, so a later switch re-arms the
/// suppression normally -- this widens when the suppression lifts, never whether it can come back.
pub fn title_visual_suppression_active() -> bool {
    // The cover reached its semantic release and handed the screen over.
    if BOOT_VIEW_RELEASE_READY_MS.load(Ordering::SeqCst) != 0 {
        return false;
    }
    // The cover is not compositing at all. Whatever ended it, there is no longer anything in front
    // of the title for the suppression to protect.
    if BOOT_VIEW_STOPPED.load(Ordering::SeqCst) != 0 {
        return false;
    }
    // An armed cover that never draws must not suppress (2026-09-04). The two latches above both
    // describe a cover that ran and then ended. Neither describes the third case, which is the one
    // that hangs the game: a cover that was RE-armed and then never composited a single frame.
    //
    // `boot_view_reset_cover_window` clears BOOT_VIEW_STOPPED on every rearm -- deliberately, so a
    // later switch gets its suppression back. But a switch rearms the cover at the title, and if
    // that new epoch then stalls before drawing, both latches sit at 0 forever and this predicate
    // answers "suppress" for the rest of the process with nothing in front of the title.
    //
    // Measured, run br-20260904-234301-412b: the cover stopped at stop_ms=31794 (reason 3), was
    // rearmed by the switch, and stuck at milestone_idx=2 (title ready) with epoch_live=0. The
    // force-hide detours then fired 8,932 times through +185s -- and
    // `oracle_title_logo_gfx_hide_last_requested_visible = 1` says the game was asking for the title
    // to be visible while we answered by hiding it. Press button was among the hidden components, so
    // the title could not be advanced by any accept signal, from the product, the harness, or a
    // human. That is the stall that blocked every agent-driven route (bd er-effects-rs-tkfb).
    //
    // So: suppression requires a cover that has actually put pixels up for the current epoch. Zero
    // draws in this epoch means there is nothing to protect and the title must be left alone.
    if BOOT_VIEW_DRAW_HITS.load(Ordering::SeqCst)
        <= BOOT_VIEW_DRAW_HITS_ARM_BASELINE.load(Ordering::SeqCst)
    {
        return false;
    }
    true
}
